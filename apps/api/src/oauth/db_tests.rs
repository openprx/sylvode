//! Token validation, revocation, audit records and the database rate window against PostgreSQL.
#![allow(
    clippy::expect_used,
    clippy::unwrap_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::print_stderr,
    clippy::too_many_lines,
    clippy::similar_names,
    clippy::doc_markdown,
    clippy::items_after_statements,
    clippy::assertions_on_constants
)]

use platform::config::Secret;
use sea_orm::{DatabaseConnection, DbBackend, FromQueryResult, Statement, TransactionTrait};
use uuid::Uuid;

use super::audit::{self, AuditEvent, AuditRecord, DetailValue};
use super::grants::{
    Actor, RevokeReason, revoke_all_user_grants, revoke_client_grants, revoke_grant, revoke_user_workspace_grants,
    revoke_workspace_grants,
};
use super::rate::{RateBucket, RateError, RateKey, enforce, enforce_with_limit};
use super::scopes::{ProjectScope, Scope, TokenResource};
use super::test_support::{Start, count, oauth_scratch_or_skip, seed_access_token, seed_grant};
use super::tokens::{TokenKind, mint};
use super::validate::{AudienceRule, ForwardCredential, RejectReason, validate_access_token};
use crate::routes::context::tenant_fixture::{Tenant, exec, seed_tenant};

const READ_SCOPES: [&str; 2] = ["projects:read", "work_items:read"];
const FORWARD: &str = "forward-secret-for-tests-0123456789abcdef";

fn api_rule() -> AudienceRule<'static> {
    AudienceRule::Api(ForwardCredential::absent())
}

async fn reason(db: &DatabaseConnection, raw: &str, rule: AudienceRule<'_>) -> RejectReason {
    validate_access_token(db, raw, rule)
        .await
        .expect_err("the token is refused")
        .reason
}

async fn grant_status(db: &DatabaseConnection, grant: Uuid) -> (String, Option<String>) {
    #[derive(FromQueryResult)]
    struct Row {
        status: String,
        revoke_reason: Option<String>,
    }
    let row = Row::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "SELECT status, revoke_reason FROM oauth_grants WHERE id = $1",
        vec![grant.into()],
    ))
    .one(db)
    .await
    .expect("query")
    .expect("grant exists");
    (row.status, row.revoke_reason)
}

async fn fixture(db: &DatabaseConnection, tag: &str) -> (Tenant, Uuid, Uuid, String) {
    let tenant = seed_tenant(db, tag).await;
    let grant = seed_grant(db, tenant.member_id, tenant.workspace_id, "all", &READ_SCOPES).await;
    let (token_id, raw) = seed_access_token(db, grant, "api", &READ_SCOPES).await;
    (tenant, grant, token_id, raw)
}

#[tokio::test]
async fn a_live_token_validates_with_its_scopes_projects_and_resource() {
    let scratch = oauth_scratch_or_skip!("val_ok", Start::Complete);
    let db = &scratch.db;
    let (tenant, grant, token_id, raw) = fixture(db, "ok").await;
    let validated = validate_access_token(db, &raw, api_rule()).await.expect("valid");
    assert_eq!(validated.token_id, token_id);
    assert_eq!(validated.grant_id, grant);
    assert_eq!(&*validated.client_id, "ramflux");
    assert_eq!(validated.user_id, tenant.member_id);
    assert_eq!(validated.workspace_id, tenant.workspace_id);
    assert_eq!(validated.resource, TokenResource::Api);
    assert!(validated.scopes.contains(Scope::ProjectsRead) && validated.scopes.contains(Scope::WorkItemsRead));
    assert!(!validated.scopes.contains(Scope::TasksRead));
    assert_eq!(validated.projects, ProjectScope::All);
    assert!(validated.user_email.ends_with("@tenant.test"));

    // A selected grant carries its project ids.
    let selected = seed_grant(db, tenant.member_id, tenant.workspace_id, "selected", &READ_SCOPES).await;
    exec(
        db,
        "INSERT INTO oauth_grant_projects (grant_id, project_id) VALUES ($1, $2)",
        vec![selected.into(), tenant.project_id.into()],
    )
    .await;
    let (_, raw) = seed_access_token(db, selected, "api", &READ_SCOPES).await;
    let validated = validate_access_token(db, &raw, api_rule()).await.expect("valid");
    assert!(validated.projects.contains(tenant.project_id));
    assert!(!validated.projects.contains(Uuid::new_v4()));
    // A selected grant with no projects reaches none.
    let empty = seed_grant(db, tenant.member_id, tenant.workspace_id, "selected", &READ_SCOPES).await;
    let (_, raw) = seed_access_token(db, empty, "api", &READ_SCOPES).await;
    let validated = validate_access_token(db, &raw, api_rule()).await.expect("valid");
    assert!(!validated.projects.contains(tenant.project_id));
    scratch.drop_self().await;
}

#[tokio::test]
async fn malformed_and_unknown_values_are_refused() {
    let scratch = oauth_scratch_or_skip!("val_unknown", Start::Complete);
    let db = &scratch.db;
    let (_, _, _, raw) = fixture(db, "un").await;
    assert_eq!(reason(db, "", api_rule()).await, RejectReason::Malformed);
    assert_eq!(reason(db, "opr_abcdef", api_rule()).await, RejectReason::Malformed);
    assert_eq!(
        reason(db, &raw.to_uppercase(), api_rule()).await,
        RejectReason::Malformed
    );
    assert_eq!(
        reason(db, &raw[..raw.len() - 1], api_rule()).await,
        RejectReason::Malformed
    );
    // Right shape, wrong kind of secret.
    let refresh = mint(TokenKind::Refresh).expect("mint");
    assert_eq!(
        reason(db, refresh.raw.expose(), api_rule()).await,
        RejectReason::Malformed
    );
    // Right shape, never issued.
    let stranger = mint(TokenKind::Access).expect("mint");
    assert_eq!(
        reason(db, stranger.raw.expose(), api_rule()).await,
        RejectReason::UnknownToken
    );
    // A refresh token stored under its own hash is not an access token even when presented with
    // the access prefix: the lookup is restricted to kind = 'access'.
    scratch.drop_self().await;
}

#[tokio::test]
async fn token_state_failures() {
    let scratch = oauth_scratch_or_skip!("val_token_state", Start::Complete);
    let db = &scratch.db;
    let (_, grant, token_id, raw) = fixture(db, "ts").await;

    exec(
        db,
        "UPDATE oauth_tokens SET revoked_at = now() WHERE id = $1",
        vec![token_id.into()],
    )
    .await;
    assert_eq!(reason(db, &raw, api_rule()).await, RejectReason::TokenRevoked);
    exec(
        db,
        "UPDATE oauth_tokens SET revoked_at = NULL, expires_at = now() WHERE id = $1",
        vec![token_id.into()],
    )
    .await;
    assert_eq!(
        reason(db, &raw, api_rule()).await,
        RejectReason::TokenExpired,
        "the instant of expiry is expired"
    );
    // A refused token never ends its grant: only the token is at fault.
    assert_eq!(grant_status(db, grant).await, ("active".into(), None));
    scratch.drop_self().await;
}

/// Each live-state failure of the grant side refuses the token and ends the grant with the reason
/// the failure names.
#[tokio::test]
async fn grant_side_failures_refuse_and_revoke_with_the_matching_reason() {
    let scratch = oauth_scratch_or_skip!("val_grant_state", Start::Complete);
    let db = &scratch.db;

    type Break = (&'static str, RejectReason, Option<&'static str>);
    let cases: [Break; 6] = [
        (
            "UPDATE oauth_grants SET expires_at = now() WHERE id = $1",
            RejectReason::GrantExpired,
            Some("expired"),
        ),
        (
            "UPDATE oauth_clients SET status = 'disabled' WHERE client_id = 'ramflux'",
            RejectReason::ClientDisabled,
            Some("client_disabled"),
        ),
        (
            "UPDATE users SET is_active = false WHERE id = (SELECT user_id FROM oauth_grants WHERE id = $1)",
            RejectReason::UserInactive,
            Some("account_disabled"),
        ),
        (
            "UPDATE users SET session_version = session_version + 1 WHERE id = (SELECT user_id FROM oauth_grants WHERE id = $1)",
            RejectReason::SessionVersionChanged,
            Some("password_changed"),
        ),
        (
            "DELETE FROM workspace_members WHERE user_id = (SELECT user_id FROM oauth_grants WHERE id = $1)",
            RejectReason::MembershipMissing,
            Some("membership_removed"),
        ),
        (
            "INSERT INTO workspace_delegation_settings (workspace_id, allow_connected_apps) SELECT workspace_id, false FROM oauth_grants WHERE id = $1",
            RejectReason::WorkspaceDisallows,
            Some("workspace_disabled"),
        ),
    ];
    for (n, (sql, expected, revoked_with)) in cases.into_iter().enumerate() {
        let (_, grant, _, raw) = fixture(db, &format!("g{n}")).await;
        exec(db, sql, vec![grant.into()]).await;
        assert_eq!(reason(db, &raw, api_rule()).await, expected);
        let (status, why) = grant_status(db, grant).await;
        assert_eq!(status, "revoked", "{expected:?}");
        assert_eq!(why.as_deref(), revoked_with, "{expected:?}");
        assert_eq!(
            count(
                db,
                "SELECT count(*) AS n FROM oauth_tokens WHERE grant_id = $1 AND revoked_at IS NULL",
                vec![grant.into()]
            )
            .await,
            0,
            "every token of the grant is revoked"
        );
        assert_eq!(
            count(
                db,
                "SELECT count(*) AS n FROM oauth_audit_events WHERE grant_id = $1 AND event = 'grant_revoked'",
                vec![grant.into()]
            )
            .await,
            1
        );
        // The next use fails on the revoked token alone.
        assert_eq!(reason(db, &raw, api_rule()).await, RejectReason::TokenRevoked);
        if n == 1 {
            exec(
                db,
                "UPDATE oauth_clients SET status = 'active' WHERE client_id = 'ramflux'",
                vec![],
            )
            .await;
        }
    }

    // A grant that is already revoked is refused without a second revocation.
    let (_, grant, token_id, raw) = fixture(db, "gr").await;
    exec(
        db,
        "UPDATE oauth_grants SET status = 'revoked', revoked_at = now(), revoke_reason = 'user_revoked' WHERE id = $1",
        vec![grant.into()],
    )
    .await;
    assert_eq!(reason(db, &raw, api_rule()).await, RejectReason::GrantNotActive);
    assert_eq!(
        count(
            db,
            "SELECT count(*) AS n FROM oauth_audit_events WHERE grant_id = $1",
            vec![grant.into()]
        )
        .await,
        0
    );
    let _ = token_id;
    scratch.drop_self().await;
}

#[tokio::test]
async fn audience_rules_in_the_database_path() {
    let scratch = oauth_scratch_or_skip!("val_audience", Start::Complete);
    let db = &scratch.db;
    let tenant = seed_tenant(db, "au").await;
    let grant = seed_grant(db, tenant.member_id, tenant.workspace_id, "all", &READ_SCOPES).await;
    let (_, api_token) = seed_access_token(db, grant, "api", &READ_SCOPES).await;
    let (_, mcp_token) = seed_access_token(db, grant, "mcp", &READ_SCOPES).await;
    let secret = Secret::new(FORWARD);
    let valid = ForwardCredential::new(Some(&secret), Some("mcp-server"), Some(FORWARD));
    let wrong = ForwardCredential::new(Some(&secret), Some("mcp-server"), Some("not-the-secret"));
    let nameless = ForwardCredential::new(Some(&secret), None, Some(FORWARD));

    assert!(
        validate_access_token(db, &api_token, AudienceRule::Api(ForwardCredential::absent()))
            .await
            .is_ok()
    );
    assert!(
        validate_access_token(db, &api_token, AudienceRule::Api(valid))
            .await
            .is_ok()
    );
    assert!(
        validate_access_token(db, &mcp_token, AudienceRule::Api(valid))
            .await
            .is_ok()
    );
    assert!(
        validate_access_token(db, &mcp_token, AudienceRule::McpForwarded(valid))
            .await
            .is_ok()
    );
    for rule in [
        AudienceRule::Api(ForwardCredential::absent()),
        AudienceRule::Api(wrong),
        AudienceRule::Api(nameless),
        AudienceRule::McpForwarded(wrong),
        AudienceRule::McpForwarded(nameless),
        AudienceRule::McpForwarded(ForwardCredential::absent()),
    ] {
        assert_eq!(
            reason(db, &mcp_token, rule).await,
            RejectReason::AudienceMismatch,
            "{rule:?}"
        );
    }
    assert_eq!(
        reason(db, &api_token, AudienceRule::McpForwarded(valid)).await,
        RejectReason::AudienceMismatch
    );
    // An audience mismatch is not a reason to end the grant.
    assert_eq!(grant_status(db, grant).await, ("active".into(), None));
    scratch.drop_self().await;
}

#[tokio::test]
async fn revoke_grant_is_idempotent_and_revokes_every_token() {
    let scratch = oauth_scratch_or_skip!("rev_idem", Start::Complete);
    let db = &scratch.db;
    let (tenant, grant, _, raw) = fixture(db, "ri").await;
    seed_access_token(db, grant, "mcp", &READ_SCOPES).await;
    exec(
        db,
        "INSERT INTO oauth_tokens (id, token_hash, kind, grant_id, client_id, scopes, expires_at) \
         VALUES ($1, $2, 'refresh', $3, 'ramflux', ARRAY['projects:read'], now() + interval '30 days')",
        vec![Uuid::new_v4().into(), format!("{:0>64}", "aa").into(), grant.into()],
    )
    .await;

    let tx = db.begin().await.expect("tx");
    assert!(
        revoke_grant(&tx, grant, RevokeReason::UserRevoked, Actor::User(tenant.member_id))
            .await
            .expect("revoke")
    );
    assert!(
        !revoke_grant(&tx, grant, RevokeReason::ClientRevoked, Actor::System)
            .await
            .expect("second revoke"),
        "the second call reports that nothing changed"
    );
    tx.commit().await.expect("commit");

    #[derive(FromQueryResult)]
    struct Row {
        status: String,
        revoke_reason: Option<String>,
        revoked_by: Option<Uuid>,
        stamped: bool,
    }
    let row = Row::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "SELECT status, revoke_reason, revoked_by, revoked_at IS NOT NULL AS stamped FROM oauth_grants WHERE id = $1",
        vec![grant.into()],
    ))
    .one(db)
    .await
    .expect("query")
    .expect("row");
    assert_eq!(row.status, "revoked");
    assert_eq!(
        row.revoke_reason.as_deref(),
        Some("user_revoked"),
        "the first reason stands"
    );
    assert_eq!(row.revoked_by, Some(tenant.member_id));
    assert!(row.stamped);
    assert_eq!(
        count(
            db,
            "SELECT count(*) AS n FROM oauth_tokens WHERE grant_id = $1 AND revoked_at IS NULL",
            vec![grant.into()]
        )
        .await,
        0
    );
    assert_eq!(
        count(
            db,
            "SELECT count(*) AS n FROM oauth_tokens WHERE grant_id = $1",
            vec![grant.into()]
        )
        .await,
        3,
        "tokens are marked, not deleted"
    );
    assert_eq!(reason(db, &raw, api_rule()).await, RejectReason::TokenRevoked);

    // Exactly one audit row, naming the reason and the actor, with no secret in it.
    #[derive(FromQueryResult)]
    struct Audit {
        actor_id: Option<Uuid>,
        user_id: Option<Uuid>,
        detail: serde_json::Value,
    }
    let rows = Audit::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "SELECT actor_id, user_id, detail FROM oauth_audit_events WHERE grant_id = $1 AND event = 'grant_revoked'",
        vec![grant.into()],
    ))
    .all(db)
    .await
    .expect("audit query");
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].actor_id, Some(tenant.member_id));
    assert_eq!(rows[0].user_id, Some(tenant.member_id));
    assert_eq!(rows[0].detail, serde_json::json!({"reason": "user_revoked"}));
    assert!(!rows[0].detail.to_string().contains(&raw));

    // An unknown grant is not an error.
    let tx = db.begin().await.expect("tx");
    assert!(
        !revoke_grant(&tx, Uuid::new_v4(), RevokeReason::Expired, Actor::System)
            .await
            .expect("no-op")
    );
    tx.rollback().await.expect("rollback");
    scratch.drop_self().await;
}

#[tokio::test]
async fn a_rolled_back_revocation_leaves_the_grant_active() {
    let scratch = oauth_scratch_or_skip!("rev_rollback", Start::Complete);
    let db = &scratch.db;
    let (_, grant, _, raw) = fixture(db, "rb").await;
    let tx = db.begin().await.expect("tx");
    revoke_grant(&tx, grant, RevokeReason::PasswordChanged, Actor::System)
        .await
        .expect("revoke");
    tx.rollback().await.expect("rollback");
    assert_eq!(grant_status(db, grant).await, ("active".into(), None));
    assert!(validate_access_token(db, &raw, api_rule()).await.is_ok());
    assert_eq!(
        count(
            db,
            "SELECT count(*) AS n FROM oauth_audit_events WHERE grant_id = $1",
            vec![grant.into()]
        )
        .await,
        0
    );
    scratch.drop_self().await;
}

#[tokio::test]
async fn revoke_grant_leaves_tokens_that_expired_long_ago_to_the_prune() {
    let scratch = oauth_scratch_or_skip!("rev_old_tokens", Start::Complete);
    let db = &scratch.db;
    let (tenant, grant, _, _) = fixture(db, "ro").await;
    for (label, hash, expires) in [
        ("live", "01", "now() + interval '10 minutes'"),
        ("expired_hours_ago", "02", "now() - interval '12 hours'"),
        ("expired_days_ago", "03", "now() - interval '3 days'"),
    ] {
        exec(
            db,
            &format!(
                "INSERT INTO oauth_tokens (id, token_hash, kind, grant_id, client_id, scopes, expires_at, last_used_at) \
                 VALUES ($1, $2, 'refresh', $3, 'ramflux', ARRAY['projects:read'], {expires}, NULL)"
            ),
            vec![Uuid::new_v4().into(), format!("{:0>64}", format!("{hash}{label}")).into(), grant.into()],
        )
        .await;
    }
    let tx = db.begin().await.expect("tx");
    assert!(
        revoke_grant(&tx, grant, RevokeReason::UserRevoked, Actor::User(tenant.member_id))
            .await
            .expect("revoke")
    );
    tx.commit().await.expect("commit");
    let unrevoked = count(
        db,
        "SELECT count(*) AS n FROM oauth_tokens WHERE grant_id = $1 AND revoked_at IS NULL",
        vec![grant.into()],
    )
    .await;
    assert_eq!(
        unrevoked, 1,
        "only the token that expired days ago is left unmarked (it is unusable, and the daily prune deletes it)"
    );
    assert_eq!(
        count(
            db,
            "SELECT count(*) AS n FROM oauth_tokens WHERE grant_id = $1 AND revoked_at IS NULL \
             AND expires_at > now() - interval '1 day'",
            vec![grant.into()]
        )
        .await,
        0,
        "no live or recently expired token survives"
    );
    scratch.drop_self().await;
}

#[tokio::test]
async fn revoke_grant_notifies_listeners_after_commit_only() {
    let scratch = oauth_scratch_or_skip!("rev_notify", Start::Complete);
    let db = &scratch.db;
    let (_, grant, _, _) = fixture(db, "nf").await;

    let mut listener = sea_orm::sqlx::postgres::PgListener::connect_with(db.get_postgres_connection_pool())
        .await
        .expect("listener connects");
    listener
        .listen(super::grants::GRANT_CHANGED_CHANNEL)
        .await
        .expect("listen");

    let tx = db.begin().await.expect("tx");
    revoke_grant(&tx, grant, RevokeReason::UserRevoked, Actor::System)
        .await
        .expect("revoke");
    let early = tokio::time::timeout(std::time::Duration::from_millis(300), listener.recv()).await;
    assert!(early.is_err(), "nothing is delivered before the commit");
    tx.commit().await.expect("commit");
    let notification = tokio::time::timeout(std::time::Duration::from_secs(5), listener.recv())
        .await
        .expect("delivered after the commit")
        .expect("notification");
    assert_eq!(notification.payload(), grant.to_string());

    // A second revocation of the same grant sends nothing.
    let tx = db.begin().await.expect("tx");
    revoke_grant(&tx, grant, RevokeReason::UserRevoked, Actor::System)
        .await
        .expect("revoke");
    tx.commit().await.expect("commit");
    assert!(
        tokio::time::timeout(std::time::Duration::from_millis(300), listener.recv())
            .await
            .is_err()
    );
    scratch.drop_self().await;
}

#[tokio::test]
async fn bulk_revocation_selects_by_user_workspace_and_client() {
    let scratch = oauth_scratch_or_skip!("rev_bulk", Start::Complete);
    let db = &scratch.db;
    let a = seed_tenant(db, "ba").await;
    let b = seed_tenant(db, "bb").await;
    let a1 = seed_grant(db, a.member_id, a.workspace_id, "all", &READ_SCOPES).await;
    let a2 = seed_grant(db, a.member_id, a.workspace_id, "all", &READ_SCOPES).await;
    let b1 = seed_grant(db, b.member_id, b.workspace_id, "all", &READ_SCOPES).await;
    let status = |g| async move { grant_status(db, g).await.0 };

    let tx = db.begin().await.expect("tx");
    let revoked = revoke_user_workspace_grants(
        &tx,
        a.member_id,
        a.workspace_id,
        RevokeReason::MembershipRemoved,
        Actor::System,
    )
    .await
    .expect("revoke");
    tx.commit().await.expect("commit");
    assert_eq!(revoked.len(), 2);
    assert_eq!(
        (status(a1).await, status(a2).await, status(b1).await),
        ("revoked".into(), "revoked".into(), "active".into())
    );

    let a3 = seed_grant(db, a.member_id, a.workspace_id, "all", &READ_SCOPES).await;
    let tx = db.begin().await.expect("tx");
    assert_eq!(
        revoke_all_user_grants(&tx, a.member_id, RevokeReason::PasswordChanged, Actor::System)
            .await
            .expect("revoke"),
        vec![a3]
    );
    assert!(
        revoke_all_user_grants(&tx, a.member_id, RevokeReason::PasswordChanged, Actor::System)
            .await
            .expect("again")
            .is_empty()
    );
    tx.commit().await.expect("commit");

    let tx = db.begin().await.expect("tx");
    assert_eq!(
        revoke_workspace_grants(&tx, b.workspace_id, RevokeReason::WorkspaceDisabled, Actor::System)
            .await
            .expect("revoke"),
        vec![b1]
    );
    tx.commit().await.expect("commit");

    let c1 = seed_grant(db, a.member_id, a.workspace_id, "all", &READ_SCOPES).await;
    let c2 = seed_grant(db, b.member_id, b.workspace_id, "all", &READ_SCOPES).await;
    let tx = db.begin().await.expect("tx");
    let mut revoked = revoke_client_grants(&tx, "ramflux", RevokeReason::ClientDisabled, Actor::System)
        .await
        .expect("revoke");
    tx.commit().await.expect("commit");
    revoked.sort();
    let mut expected = vec![c1, c2];
    expected.sort();
    assert_eq!(revoked, expected);
    assert_eq!(grant_status(db, c1).await.1.as_deref(), Some("client_disabled"));
    scratch.drop_self().await;
}

#[tokio::test]
async fn audit_rows_are_written_with_their_fields_and_sampled_rejections_are_limited() {
    let scratch = oauth_scratch_or_skip!("audit", Start::Complete);
    let db = &scratch.db;
    let tenant = seed_tenant(db, "ad").await;
    let (grant, token) = (Uuid::new_v4(), Uuid::new_v4());
    let ip: std::net::IpAddr = "192.0.2.44".parse().unwrap();
    let record = AuditRecord::new(AuditEvent::ConsentGranted)
        .with_client("ramflux")
        .with_grant(grant)
        .with_token(token)
        .with_user(tenant.member_id)
        .with_workspace(tenant.workspace_id)
        .with_actor(tenant.member_id)
        .with_client_ip(ip)
        .with_user_agent("Ramflux/1.0\r\n(test)")
        .with_detail("projects", DetailValue::Count(2))
        .with_detail("mode", DetailValue::Code("selected"));
    audit::write(db, &record).await.expect("write");

    #[derive(FromQueryResult)]
    struct Row {
        event: String,
        client_id: Option<String>,
        grant_id: Option<Uuid>,
        token_id: Option<Uuid>,
        user_id: Option<Uuid>,
        workspace_id: Option<Uuid>,
        actor_id: Option<Uuid>,
        ip: Option<String>,
        user_agent: Option<String>,
        detail: serde_json::Value,
    }
    let row = Row::find_by_statement(Statement::from_string(
        DbBackend::Postgres,
        "SELECT event, client_id, grant_id, token_id, user_id, workspace_id, actor_id, host(client_ip) AS ip, \
         user_agent, detail FROM oauth_audit_events WHERE event = 'consent_granted'"
            .to_owned(),
    ))
    .one(db)
    .await
    .expect("query")
    .expect("row");
    assert_eq!(row.event, "consent_granted");
    assert_eq!(row.client_id.as_deref(), Some("ramflux"));
    assert_eq!((row.grant_id, row.token_id), (Some(grant), Some(token)));
    assert_eq!(
        (row.user_id, row.workspace_id, row.actor_id),
        (
            Some(tenant.member_id),
            Some(tenant.workspace_id),
            Some(tenant.member_id)
        )
    );
    assert_eq!(row.ip.as_deref(), Some("192.0.2.44"));
    assert_eq!(row.user_agent.as_deref(), Some("Ramflux/1.0(test)"));
    assert_eq!(row.detail, serde_json::json!({"projects": 2, "mode": "selected"}));

    // Sampled rejections: one row per token per minute.
    let rejected = |token: Option<Uuid>, ip: &str| {
        let mut record = AuditRecord::new(AuditEvent::TokenRejected)
            .with_client_ip(ip.parse().unwrap())
            .with_detail("reason", DetailValue::Code("token_expired"));
        if let Some(token) = token {
            record = record.with_token(token);
        }
        record
    };
    assert!(
        audit::write_token_rejected_sampled(db, &rejected(Some(token), "192.0.2.1"))
            .await
            .expect("first")
    );
    assert!(
        !audit::write_token_rejected_sampled(db, &rejected(Some(token), "192.0.2.2"))
            .await
            .expect("same token")
    );
    assert!(
        audit::write_token_rejected_sampled(db, &rejected(Some(Uuid::new_v4()), "192.0.2.1"))
            .await
            .expect("other token")
    );
    assert!(
        audit::write_token_rejected_sampled(db, &rejected(None, "192.0.2.9"))
            .await
            .expect("unknown token, new address")
    );
    assert!(
        !audit::write_token_rejected_sampled(db, &rejected(None, "192.0.2.9"))
            .await
            .expect("unknown token, same address")
    );
    assert!(
        audit::write_token_rejected_sampled(db, &rejected(None, "192.0.2.10"))
            .await
            .expect("unknown token, other address")
    );
    exec(
        db,
        "UPDATE oauth_audit_events SET occurred_at = now() - interval '2 minutes' WHERE event = 'token_rejected'",
        vec![],
    )
    .await;
    assert!(
        audit::write_token_rejected_sampled(db, &rejected(Some(token), "192.0.2.1"))
            .await
            .expect("a minute later")
    );
    scratch.drop_self().await;
}

#[tokio::test]
async fn the_database_rate_window_admits_the_limit_then_refuses_then_resets() {
    let scratch = oauth_scratch_or_skip!("rate", Start::Complete);
    let db = &scratch.db;
    let key = RateKey::from_ip("192.0.2.77".parse().unwrap());
    let other = RateKey::from_ip("192.0.2.78".parse().unwrap());
    let bucket = RateBucket::Decision;

    for hit in 1..=bucket.limit() {
        enforce(db, bucket, &key)
            .await
            .unwrap_or_else(|err| panic!("hit {hit} is within the limit: {err}"));
    }
    let Err(RateError::Limited(limited)) = enforce(db, bucket, &key).await else {
        panic!("the hit over the limit is refused");
    };
    assert!(
        (1..=60).contains(&limited.retry_after_secs),
        "{}",
        limited.retry_after_secs
    );
    // Another key and another bucket are counted separately.
    enforce(db, bucket, &other).await.expect("another address");
    enforce(db, RateBucket::Authorize, &key).await.expect("another bucket");
    // The stored key is a hash, never the address.
    assert_eq!(
        count(
            db,
            "SELECT count(*) AS n FROM oauth_rate_hits WHERE key_hash LIKE '%192%'",
            vec![]
        )
        .await,
        0
    );
    assert_eq!(
        count(
            db,
            "SELECT count(*) AS n FROM oauth_rate_hits WHERE bucket = 'decision' AND key_hash = $1",
            vec![key.as_hex().into()]
        )
        .await,
        1
    );

    // A window that has run out starts again.
    exec(
        db,
        "UPDATE oauth_rate_hits SET window_started = now() - interval '61 seconds' WHERE key_hash = $1",
        vec![key.as_hex().into()],
    )
    .await;
    enforce(db, bucket, &key).await.expect("the next window admits again");

    // Counting is exact under concurrency: with a limit of 5, exactly five of twenty admit.
    let racing = RateKey::from_id(Uuid::new_v4());
    let results =
        futures_util::future::join_all((0..20).map(|_| enforce_with_limit(db, RateBucket::Revoke, &racing, 5))).await;
    assert_eq!(results.iter().filter(|r| r.is_ok()).count(), 5);
    scratch.drop_self().await;
}
