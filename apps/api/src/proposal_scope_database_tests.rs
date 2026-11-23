use crate::migrate::db_tests::{Scratch, scratch};
use crate::migrate::{MigrationOptions, run_migrations_with};
use api::routes::proposal::{ListProposalsQuery, get_proposal, governance_tick, list_proposals};
use axum::extract::{Extension, Query, State};
use axum::response::IntoResponse;
use platform::timestamp::Timestamp;
use platform::{
    app::AppState,
    auth::{JwtClaims, TokenType},
    config::{AppConfig, Secret},
};
use sea_orm::{ConnectionTrait, DbBackend, Statement};
use time::Duration;
use uuid::Uuid;

macro_rules! scratch_or_skip {
    ($label:expr) => {
        match scratch($label).await {
            Some(scratch) => scratch,
            None => {
                eprintln!("skipped: OPENPR_TEST_DATABASE_URL is not set");
                return;
            }
        }
    };
}

fn state_for(scratch: &Scratch) -> AppState {
    AppState {
        cfg: AppConfig {
            app_name: "api-test".to_string(),
            bind_addr: "127.0.0.1:0".to_string(),
            database_url: Secret::new(scratch.url()),
            jwt_secret: Secret::new("proposal-scope-test-secret"),
            jwt_access_ttl_seconds: 900,
            jwt_refresh_ttl_seconds: 3600,
            allow_insecure_cookies: false,
            collab_allowed_origins: Vec::new(),
        },
        db: scratch.connection().clone(),
        flow_permission_cache: platform::app::FlowPermissionCacheSlot::default(),
    }
}

fn claims_for(user_id: Uuid) -> JwtClaims {
    JwtClaims {
        sub: user_id.to_string(),
        email: format!("{user_id}@example.test"),
        token_type: TokenType::Access,
        iat: 0,
        exp: 0,
        sv: 0,
    }
}

fn empty_query() -> ListProposalsQuery {
    ListProposalsQuery {
        status: None,
        proposal_type: None,
        domain: None,
        page: None,
        per_page: Some(100),
        sort: None,
    }
}

async fn body_of(response: axum::response::Response) -> serde_json::Value {
    let bytes = axum::body::to_bytes(response.into_body(), 1024 * 1024)
        .await
        .expect("the response body is readable");
    serde_json::from_slice(&bytes).expect("the response body is JSON")
}

/// One workspace with one member and one proposal in it.
struct Tenant {
    workspace_id: Uuid,
    user_id: Uuid,
    proposal_id: String,
}

async fn seed_tenant(state: &AppState, label: &str, status: &str) -> Tenant {
    let workspace_id = Uuid::new_v4();
    let user_id = Uuid::new_v4();
    let proposal_id = format!("PROP-{label}-{}", Uuid::new_v4());

    state
        .db
        .execute(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "INSERT INTO users (id, email, name, password_hash, role) VALUES ($1, $2, $3, 'x', 'user')",
            vec![user_id.into(), format!("{label}@example.test").into(), label.into()],
        ))
        .await
        .expect("the user is created");
    state
        .db
        .execute(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "INSERT INTO workspaces (id, name, slug, created_by) VALUES ($1, $2, $3, $4)",
            vec![
                workspace_id.into(),
                label.into(),
                format!("{label}-{workspace_id}").into(),
                user_id.into(),
            ],
        ))
        .await
        .expect("the workspace is created");
    state
        .db
        .execute(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "INSERT INTO workspace_members (workspace_id, user_id, role) VALUES ($1, $2, 'owner')",
            vec![workspace_id.into(), user_id.into()],
        ))
        .await
        .expect("the membership is created");
    state
        .db
        .execute(Statement::from_sql_and_values(
            DbBackend::Postgres,
            r#"
                INSERT INTO proposals (
                    id, title, proposal_type, status, author_id, author_type, content,
                    domains, voting_rule, cycle_template, workspace_id,
                    created_at, submitted_at, voting_started_at
                ) VALUES (
                    $1, $2, 'feature'::proposal_type, $3::proposal_status, $4, 'human'::author_type, $5,
                    '["product"]'::jsonb, 'simple_majority'::voting_rule, 'rapid'::cycle_template, $6,
                    $7, $7, $8
                )
            "#,
            vec![
                proposal_id.clone().into(),
                format!("proposal for {label}").into(),
                status.into(),
                user_id.to_string().into(),
                "content that is comfortably longer than the fifty character minimum".into(),
                workspace_id.into(),
                Timestamp::now().into(),
                (Timestamp::now() - Duration::hours(48)).into(),
            ],
        ))
        .await
        .expect("the proposal is created");

    Tenant {
        workspace_id,
        user_id,
        proposal_id,
    }
}

#[derive(sea_orm::FromQueryResult)]
struct CountRow {
    count: i64,
}

async fn decision_count(state: &AppState, proposal_id: &str) -> i64 {
    use sea_orm::FromQueryResult;
    CountRow::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "SELECT COUNT(*)::bigint AS count FROM decisions WHERE proposal_id = $1",
        vec![proposal_id.to_string().into()],
    ))
    .one(&state.db)
    .await
    .expect("the decision count is readable")
    .map_or(0, |row| row.count)
}

/// Governance proposals used to live in one instance wide namespace with no tenant column, so
/// every authenticated caller could list every other tenant's proposals.
#[tokio::test]
async fn a_member_of_one_workspace_cannot_read_another_workspace() {
    let scratch = scratch_or_skip!("proposal_scope");
    let state = state_for(&scratch);
    run_migrations_with(&state.db, MigrationOptions::default())
        .await
        .expect("the scratch database migrates");

    let alpha = seed_tenant(&state, "alpha", "open").await;
    let beta = seed_tenant(&state, "beta", "open").await;

    let listed = list_proposals(
        State(state.clone()),
        Extension(claims_for(alpha.user_id)),
        Query(empty_query()),
    )
    .await
    .expect("the list endpoint answers")
    .into_response();
    let body = body_of(listed).await;
    let items = body["data"]["items"].as_array().cloned().unwrap_or_default();
    let ids: Vec<&str> = items.iter().filter_map(|item| item["id"].as_str()).collect();
    assert_eq!(
        ids,
        vec![alpha.proposal_id.as_str()],
        "the list must contain only the caller's workspace"
    );
    assert_eq!(
        items
            .first()
            .and_then(|item| item["workspace_id"].as_str())
            .and_then(|id| Uuid::parse_str(id).ok()),
        Some(alpha.workspace_id)
    );

    let cross_tenant = get_proposal(
        State(state.clone()),
        Extension(claims_for(alpha.user_id)),
        axum::extract::Path(beta.proposal_id.clone()),
    )
    .await;
    assert!(
        cross_tenant.is_err(),
        "reading another workspace's proposal must not succeed"
    );

    scratch.drop_self().await;
}

/// `GET /api/v1/proposals/{id}` used to finalize an expired vote inline: it updated the
/// proposal, inserted a decision and moved trust scores, all from a request that was not even
/// authenticated. The read must not write, and the worker tick must do the settling.
#[tokio::test]
async fn reading_an_expired_proposal_does_not_settle_it_but_the_worker_tick_does() {
    let scratch = scratch_or_skip!("proposal_settlement");
    let state = state_for(&scratch);
    run_migrations_with(&state.db, MigrationOptions::default())
        .await
        .expect("the scratch database migrates");

    // Voting started 48 hours ago on a `rapid` cycle, whose window is one hour.
    let tenant = seed_tenant(&state, "settle", "voting").await;

    let response = get_proposal(
        State(state.clone()),
        Extension(claims_for(tenant.user_id)),
        axum::extract::Path(tenant.proposal_id.clone()),
    )
    .await
    .expect("the read answers")
    .into_response();
    let body = body_of(response).await;
    assert_eq!(body["data"]["proposal"]["status"], "voting");
    assert_eq!(
        decision_count(&state, &tenant.proposal_id).await,
        0,
        "a read must not create a decision"
    );

    governance_tick(&state).await.expect("the worker tick runs");
    assert_eq!(
        decision_count(&state, &tenant.proposal_id).await,
        1,
        "the worker tick settles the expired proposal"
    );

    // Running it again must not produce a second decision.
    governance_tick(&state).await.expect("the worker tick is idempotent");
    assert_eq!(decision_count(&state, &tenant.proposal_id).await, 1);

    scratch.drop_self().await;
}
