//! The inbound event endpoint through the production router, with `[inbound]` enabled.
//!
//! `inbound::db_tests` builds a router of its own with the source layer only, so it cannot see the
//! layers `build_router` puts around the route, among them the catalogue guard that once refused
//! the endpoint for every caller. These tests send the requests through `build_router_with_inbound`,
//! the router the server runs, against a scratch PostgreSQL database with every migration applied.
#![cfg(test)]
#![allow(clippy::expect_used, clippy::unwrap_used, clippy::print_stderr, clippy::doc_markdown)]

use crate::migrate::db_tests::scratch;
use crate::migrate::{MigrationOptions, run_migrations_with};
use crate::router::build_router_with_inbound;
use api::inbound::INBOUND_EVENTS_PATH;
use api::inbound::credentials::mint_token;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use platform::{
    app::AppState,
    config::{AppConfig, InboundConfig, Secret},
    timestamp::Timestamp,
};
use sea_orm::{ConnectionTrait, DatabaseConnection, DbBackend, Statement};
use serde_json::{Value, json};
use tower::ServiceExt as _;
use uuid::Uuid;

async fn exec(db: &DatabaseConnection, sql: &str, values: Vec<sea_orm::Value>) {
    db.execute(Statement::from_sql_and_values(DbBackend::Postgres, sql, values))
        .await
        .unwrap_or_else(|err| panic!("setup statement failed: {err}"));
}

fn state_for(db: DatabaseConnection) -> AppState {
    AppState {
        cfg: AppConfig {
            app_name: "inbound-router-test".to_string(),
            bind_addr: "127.0.0.1:0".to_string(),
            database_url: Secret::new("postgres://unused/unused"),
            jwt_secret: Secret::new("inbound-router-test-secret-built-for-tests"),
            jwt_access_ttl_seconds: 900,
            jwt_refresh_ttl_seconds: 3600,
            allow_insecure_cookies: false,
            collab_allowed_origins: Vec::new(),
        },
        db,
        flow_permission_cache: platform::app::FlowPermissionCacheSlot::default(),
    }
}

fn event() -> Value {
    json!({
        "spec": "sylvode.inbound_event.v1",
        "event_id": "router-1",
        "occurred_at": Timestamp::now().to_rfc3339(),
        "kind": "waf.attack_detected",
        "severity": "high",
        "dedup_key": "waf:router",
        "title": "SQL injection burst on /login",
        "summary": "1,240 blocked requests",
        "subject": { "host": "shop.example.com" },
        "payload": { "requests": 1240 },
    })
}

async fn post(router: &axum::Router, token: Option<&str>) -> (StatusCode, Value) {
    let mut builder = Request::post(INBOUND_EVENTS_PATH).header("content-type", "application/json");
    if let Some(token) = token {
        builder = builder.header("authorization", format!("Bearer {token}"));
    }
    let request = builder.body(Body::from(serde_json::to_vec(&event()).unwrap())).unwrap();
    let response = router.clone().oneshot(request).await.unwrap();
    let status = response.status();
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX).await.unwrap();
    (status, serde_json::from_slice(&bytes).unwrap_or(Value::Null))
}

/// A valid source credential passes the catalogue guard and the source layer; a missing credential
/// and a bot token get the layer's 401. Not covered by `inbound::db_tests`, whose router has no guard.
#[tokio::test]
async fn the_production_router_accepts_a_source_credential_and_refuses_the_rest() {
    let Some(scratch) = scratch("inbound_router").await else {
        eprintln!("skipped: OPENPR_TEST_DATABASE_URL is not set");
        return;
    };
    let db = scratch.connection().clone();
    run_migrations_with(&db, MigrationOptions::default())
        .await
        .expect("migrations apply");

    let [admin, workspace, project, source] = std::array::from_fn(|_| Uuid::new_v4());
    exec(
        &db,
        "INSERT INTO users (id, email, password_hash, name, role, is_active) VALUES ($1, $2, '!', 'admin', 'user', true)",
        vec![admin.into(), format!("{admin}@inbound-router.test").into()],
    )
    .await;
    exec(
        &db,
        "INSERT INTO workspaces (id, slug, name, created_by) VALUES ($1, $2, 'inbound router', $3)",
        vec![workspace.into(), format!("ws-{workspace}").into(), admin.into()],
    )
    .await;
    exec(
        &db,
        "INSERT INTO workspace_members (workspace_id, user_id, role) VALUES ($1, $2, 'admin')",
        vec![workspace.into(), admin.into()],
    )
    .await;
    exec(
        &db,
        "INSERT INTO projects (id, workspace_id, key, name, created_by) VALUES ($1, $2, 'INR', 'inbound router', $3)",
        vec![project.into(), workspace.into(), admin.into()],
    )
    .await;
    let minted = mint_token().expect("entropy is available");
    // No signature is required, so the credential alone decides the request.
    exec(
        &db,
        "INSERT INTO inbound_sources (id, workspace_id, project_id, name, token_hash, token_prefix, \
         signing_secret, require_signature, acting_user_id, created_by) \
         VALUES ($1, $2, $3, 'router test source', $4, $5, 'unused', false, $6, $6)",
        vec![
            source.into(),
            workspace.into(),
            project.into(),
            minted.hash.clone().into(),
            minted.display_prefix.clone().into(),
            admin.into(),
        ],
    )
    .await;

    let config = InboundConfig {
        enabled: true,
        ..InboundConfig::default()
    };
    let router = build_router_with_inbound(state_for(db.clone()), None, config);

    let (status, body) = post(&router, Some(&minted.raw)).await;
    assert!(
        status.is_success(),
        "a valid source credential was refused: {status} {body}"
    );

    let (status, body) = post(&router, None).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED, "{body}");
    assert_eq!(body["message"], "invalid_credentials");

    let bot_token = format!("opr_{}", "a".repeat(64));
    let (status, body) = post(&router, Some(&bot_token)).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED, "{body}");
    assert_eq!(body["message"], "invalid_credentials");

    scratch.drop_self().await;
}
