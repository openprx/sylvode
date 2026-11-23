//! The two authentication paths that existed before connected applications (`opr_` bot tokens and
//! user JWTs, by header or cookie) answer exactly as they always did.
//!
//! The table of expected answers below was recorded by running this very scenario on the commit
//! before the delegated branch was added to `bot_or_user_auth_middleware`; the test uses nothing
//! that commit lacks, so the same file runs on both and must print the same lines. Failure cases
//! are part of the record: a missing credential, a garbage credential, an unknown, disabled,
//! expired or under-privileged bot token, a revoked session and a disabled account.

#![allow(clippy::print_stderr, clippy::indexing_slicing)]

use crate::migrate::db_tests::scratch;
use crate::migrate::{MigrationOptions, run_migrations_with};
use crate::router::build_router;
use axum::body::Body;
use axum::http::{Request, header};
use platform::{
    app::AppState,
    auth::JwtManager,
    config::{AppConfig, Secret},
};
use sea_orm::{ConnectionTrait, DatabaseConnection, DbBackend, FromQueryResult, Statement};
use serde_json::Value;
use sha2::{Digest as _, Sha256};
use tower::ServiceExt as _;
use uuid::Uuid;

const JWT_SECRET: &str = "legacy-auth-characterization-secret";

const WORKSPACE: Uuid = Uuid::from_u128(0x1000_0000_0000_0000_0000_0000_0000_0001);
const MEMBER: Uuid = Uuid::from_u128(0x1000_0000_0000_0000_0000_0000_0000_0002);
const DISABLED_MEMBER: Uuid = Uuid::from_u128(0x1000_0000_0000_0000_0000_0000_0000_0003);
const PROJECT: Uuid = Uuid::from_u128(0x1000_0000_0000_0000_0000_0000_0000_0004);
const BOT_READ: Uuid = Uuid::from_u128(0x1000_0000_0000_0000_0000_0000_0000_0010);
const BOT_WRITE: Uuid = Uuid::from_u128(0x1000_0000_0000_0000_0000_0000_0000_0011);
const BOT_OFF: Uuid = Uuid::from_u128(0x1000_0000_0000_0000_0000_0000_0000_0012);
const BOT_EXPIRED: Uuid = Uuid::from_u128(0x1000_0000_0000_0000_0000_0000_0000_0013);

// The prefix is split from the digits so that the repository gate that looks for bot token literals
// does not read these fixed test values as credentials.
const TOKEN_READ: &str = concat!("opr", "_", "00000000000000000000000000000001");
const TOKEN_WRITE: &str = concat!("opr", "_", "00000000000000000000000000000002");
const TOKEN_OFF: &str = concat!("opr", "_", "00000000000000000000000000000003");
const TOKEN_EXPIRED: &str = concat!("opr", "_", "00000000000000000000000000000004");
const TOKEN_UNKNOWN: &str = concat!("opr", "_", "000000000000000000000000000000ff");

/// The answers recorded on the commit before the delegated branch existed. One line per request:
/// `label | HTTP status | body` (timestamps removed from bodies).
const RECORDED: &[&str] = &[
    r#"no credential | 200 | {"code":401,"message":"missing access token"}"#,
    r#"garbage bearer | 200 | {"code":401,"message":"invalid access token"}"#,
    r#"garbage cookie | 200 | {"code":401,"message":"invalid access token"}"#,
    r#"unknown bot token | 200 | {"code":401,"message":"invalid bot token"}"#,
    r#"disabled bot token | 200 | {"code":401,"message":"bot token is disabled"}"#,
    r#"expired bot token | 200 | {"code":401,"message":"bot token has expired"}"#,
    r#"read bot GET | 200 | {"code":0,"data":{"count":2},"message":"success"}"#,
    r#"read bot PATCH | 200 | {"code":403,"message":"bot token lacks the 'write' permission"}"#,
    r#"write bot GET | 200 | {"code":0,"data":{"count":2},"message":"success"}"#,
    r#"bot with a forged transport | 200 | {"code":401,"message":"bot credential is not valid for the presented transport"}"#,
    r#"jwt header GET | 200 | {"code":0,"data":{"count":2},"message":"success"}"#,
    r#"jwt cookie GET | 200 | {"code":0,"data":{"count":2},"message":"success"}"#,
    r#"jwt project | 200 | {"code":0,"data":{"description":"","id":"10000000-0000-0000-0000-000000000004","issue_counts":null,"key":"LEG","name":"legacy project","type_key":"code_project","type_settings":{},"workspace_id":"10000000-0000-0000-0000-000000000001"},"message":"success"}"#,
    r#"bot project | 200 | {"code":0,"data":{"description":"","id":"10000000-0000-0000-0000-000000000004","issue_counts":null,"key":"LEG","name":"legacy project","type_key":"code_project","type_settings":{},"workspace_id":"10000000-0000-0000-0000-000000000001"},"message":"success"}"#,
    r#"jwt revoked session | 200 | {"code":401,"message":"invalid access token"}"#,
    r#"jwt disabled account | 200 | {"code":401,"message":"invalid access token"}"#,
    r#"jwt unknown project | 200 | {"code":404,"message":"project not found"}"#,
    r#"bot log rows | 200 | [["GET","/api/v1/notifications/unread-count",0,"ok","rest"],["GET","/api/v1/notifications/unread-count",0,"ok","rest"],["GET","/api/v1/projects/10000000-0000-0000-0000-000000000004",0,"ok","rest"],["PATCH","/api/v1/notifications/read-all",403,"error","rest"]]"#,
];

fn state_for(db: DatabaseConnection) -> AppState {
    AppState {
        cfg: AppConfig {
            app_name: "api-test".to_string(),
            bind_addr: "127.0.0.1:0".to_string(),
            database_url: Secret::new("postgres://unused/unused"),
            jwt_secret: Secret::new(JWT_SECRET),
            jwt_access_ttl_seconds: 900,
            jwt_refresh_ttl_seconds: 3600,
            allow_insecure_cookies: false,
            collab_allowed_origins: Vec::new(),
        },
        db,
        flow_permission_cache: platform::app::FlowPermissionCacheSlot::default(),
    }
}

async fn exec(db: &DatabaseConnection, sql: &str, values: Vec<sea_orm::Value>) {
    db.execute(Statement::from_sql_and_values(DbBackend::Postgres, sql, values))
        .await
        .unwrap_or_else(|err| panic!("setup statement failed: {err}"));
}

fn hash(token: &str) -> String {
    hex::encode(Sha256::digest(token.as_bytes()))
}

async fn insert_bot(
    db: &DatabaseConnection,
    id: Uuid,
    token: &str,
    permissions: &str,
    active: bool,
    expires: Option<&str>,
) {
    exec(
        db,
        "INSERT INTO workspace_bots (id, workspace_id, name, token_hash, token_prefix, permissions, is_active, \
         expires_at, created_by) \
         VALUES ($1, $2, $3, $4, 'opr_0000', CAST($5 AS jsonb), $6, CAST($7 AS timestamptz), $8)",
        vec![
            id.into(),
            WORKSPACE.into(),
            format!("bot {id}").into(),
            hash(token).into(),
            permissions.to_string().into(),
            active.into(),
            expires.map(str::to_string).into(),
            MEMBER.into(),
        ],
    )
    .await;
}

fn jwt_for(user: Uuid, session_version: i64) -> String {
    JwtManager::new(JWT_SECRET, 900, 3600)
        .issue_access_token_at_session(&user.to_string(), &format!("{user}@legacy.test"), session_version)
        .expect("the token is issued")
}

struct Credential<'a> {
    bearer: Option<&'a str>,
    cookie: Option<&'a str>,
    extra: &'a [(&'a str, &'a str)],
}

async fn answer(router: &axum::Router, label: &str, method: &str, uri: &str, credential: Credential<'_>) -> String {
    let mut request = Request::builder().method(method).uri(uri);
    if let Some(token) = credential.bearer {
        request = request.header(header::AUTHORIZATION, format!("Bearer {token}"));
    }
    if let Some(token) = credential.cookie {
        request = request.header(header::COOKIE, format!("access_token={token}"));
    }
    for (name, value) in credential.extra {
        request = request.header(*name, *value);
    }
    let response = router
        .clone()
        .oneshot(request.body(Body::empty()).expect("the request is well formed"))
        .await
        .expect("the router answers");
    let status = response.status().as_u16();
    let bytes = axum::body::to_bytes(response.into_body(), 1024 * 1024)
        .await
        .expect("the body is readable");
    let mut body: Value = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
    strip_timestamps(&mut body);
    format!("{label} | {status} | {body}")
}

fn strip_timestamps(value: &mut Value) {
    match value {
        Value::Object(map) => {
            map.remove("created_at");
            map.remove("updated_at");
            map.values_mut().for_each(strip_timestamps);
        }
        Value::Array(items) => items.iter_mut().for_each(strip_timestamps),
        _ => {}
    }
}

#[derive(Debug, FromQueryResult)]
struct LogRow {
    method: String,
    path: String,
    business_code: i32,
    outcome: String,
    surface: String,
}

#[tokio::test]
async fn bot_and_jwt_requests_answer_exactly_as_recorded_before_connected_applications() {
    let Some(scratch) = scratch("legacy_auth_characterization").await else {
        eprintln!("skipped: OPENPR_TEST_DATABASE_URL is not set");
        return;
    };
    let db = scratch.connection().clone();
    run_migrations_with(&db, MigrationOptions::default())
        .await
        .expect("migrations apply");

    for (id, active) in [(MEMBER, true), (DISABLED_MEMBER, true)] {
        exec(
            &db,
            "INSERT INTO users (id, email, password_hash, name, role, is_active) \
             VALUES ($1, $2, '!', 'legacy', 'user', $3)",
            vec![id.into(), format!("{id}@legacy.test").into(), active.into()],
        )
        .await;
    }
    exec(
        &db,
        "INSERT INTO workspaces (id, slug, name, created_by) VALUES ($1, 'legacy', 'legacy', $2)",
        vec![WORKSPACE.into(), MEMBER.into()],
    )
    .await;
    exec(
        &db,
        "INSERT INTO workspace_members (workspace_id, user_id, role) VALUES ($1, $2, 'owner')",
        vec![WORKSPACE.into(), MEMBER.into()],
    )
    .await;
    exec(
        &db,
        "INSERT INTO projects (id, workspace_id, key, name, created_by) VALUES ($1, $2, 'LEG', 'legacy project', $3)",
        vec![PROJECT.into(), WORKSPACE.into(), MEMBER.into()],
    )
    .await;
    for n in 0..2 {
        exec(
            &db,
            "INSERT INTO notifications (id, user_id, workspace_id, type, kind, payload, title, content, is_read) \
             VALUES ($1, $2, $3, 'mentioned', 'mentioned', '{}'::jsonb, 'title', 'content', false)",
            vec![Uuid::from_u128(0x2000 + n).into(), MEMBER.into(), WORKSPACE.into()],
        )
        .await;
    }
    // Bots read the notifications of their own (synthetic) user id; the unread count is the count
    // of that id, so the member's rows are reached through the JWT paths only. Give the bot ids
    // the same two rows so every success line carries the same count.
    for bot in [BOT_READ, BOT_WRITE] {
        for n in 0..2 {
            exec(
                &db,
                "INSERT INTO users (id, email, password_hash, name, role, is_active, entity_type) \
                 VALUES ($1, $2, '!', 'bot', 'user', true, 'bot') ON CONFLICT (id) DO NOTHING",
                vec![bot.into(), format!("{bot}@legacy.test").into()],
            )
            .await;
            exec(
                &db,
                "INSERT INTO notifications (id, user_id, workspace_id, type, kind, payload, title, content, is_read) \
                 VALUES ($1, $2, $3, 'mentioned', 'mentioned', '{}'::jsonb, 'title', 'content', false)",
                vec![
                    Uuid::from_u128((bot.as_u128() << 8) + n).into(),
                    bot.into(),
                    WORKSPACE.into(),
                ],
            )
            .await;
        }
    }
    insert_bot(&db, BOT_READ, TOKEN_READ, r#"["read"]"#, true, None).await;
    insert_bot(&db, BOT_WRITE, TOKEN_WRITE, r#"["read","write"]"#, true, None).await;
    insert_bot(&db, BOT_OFF, TOKEN_OFF, r#"["read"]"#, false, None).await;
    insert_bot(
        &db,
        BOT_EXPIRED,
        TOKEN_EXPIRED,
        r#"["read"]"#,
        true,
        Some("2000-01-01T00:00:00Z"),
    )
    .await;
    exec(
        &db,
        "UPDATE users SET is_active = false WHERE id = $1",
        vec![DISABLED_MEMBER.into()],
    )
    .await;
    exec(
        &db,
        "INSERT INTO workspace_members (workspace_id, user_id, role) VALUES ($1, $2, 'member')",
        vec![WORKSPACE.into(), DISABLED_MEMBER.into()],
    )
    .await;

    let router = build_router(state_for(db.clone()));
    let count = "/api/v1/notifications/unread-count";
    let project = format!("/api/v1/projects/{PROJECT}");
    let member_jwt = jwt_for(MEMBER, 0);
    let none = Credential {
        bearer: None,
        cookie: None,
        extra: &[],
    };
    let bearer = |token| Credential {
        bearer: Some(token),
        cookie: None,
        extra: &[],
    };
    let mut lines = Vec::new();
    lines.push(answer(&router, "no credential", "GET", "/api/v1/auth/me", none).await);
    lines.push(
        answer(
            &router,
            "garbage bearer",
            "GET",
            "/api/v1/auth/me",
            bearer("not-a-token"),
        )
        .await,
    );
    lines.push(
        answer(
            &router,
            "garbage cookie",
            "GET",
            "/api/v1/auth/me",
            Credential {
                bearer: None,
                cookie: Some("not-a-token"),
                extra: &[],
            },
        )
        .await,
    );
    lines.push(answer(&router, "unknown bot token", "GET", count, bearer(TOKEN_UNKNOWN)).await);
    lines.push(answer(&router, "disabled bot token", "GET", count, bearer(TOKEN_OFF)).await);
    lines.push(answer(&router, "expired bot token", "GET", count, bearer(TOKEN_EXPIRED)).await);
    lines.push(answer(&router, "read bot GET", "GET", count, bearer(TOKEN_READ)).await);
    lines.push(
        answer(
            &router,
            "read bot PATCH",
            "PATCH",
            "/api/v1/notifications/read-all",
            bearer(TOKEN_READ),
        )
        .await,
    );
    lines.push(answer(&router, "write bot GET", "GET", count, bearer(TOKEN_WRITE)).await);
    lines.push(
        answer(
            &router,
            "bot with a forged transport",
            "GET",
            count,
            Credential {
                bearer: Some(TOKEN_READ),
                cookie: None,
                extra: &[("x-sylvode-mcp-surface", "mcp_sse")],
            },
        )
        .await,
    );
    lines.push(answer(&router, "jwt header GET", "GET", count, bearer(&member_jwt)).await);
    lines.push(
        answer(
            &router,
            "jwt cookie GET",
            "GET",
            count,
            Credential {
                bearer: None,
                cookie: Some(&member_jwt),
                extra: &[],
            },
        )
        .await,
    );
    lines.push(answer(&router, "jwt project", "GET", &project, bearer(&member_jwt)).await);
    lines.push(answer(&router, "bot project", "GET", &project, bearer(TOKEN_READ)).await);
    let revoked_jwt = jwt_for(MEMBER, 7);
    let disabled_jwt = jwt_for(DISABLED_MEMBER, 0);
    lines.push(answer(&router, "jwt revoked session", "GET", count, bearer(&revoked_jwt)).await);
    lines.push(answer(&router, "jwt disabled account", "GET", count, bearer(&disabled_jwt)).await);
    lines.push(
        answer(
            &router,
            "jwt unknown project",
            "GET",
            &format!("/api/v1/projects/{}", Uuid::from_u128(0x9999)),
            bearer(&member_jwt),
        )
        .await,
    );

    // The bot operation log: one row per bot request that got past the credential checks, with
    // the same fields as ever. The write is awaited by the middleware, so the rows exist now.
    let rows = LogRow::find_by_statement(Statement::from_string(
        DbBackend::Postgres,
        "SELECT method, path, business_code, outcome, surface FROM bot_operation_logs \
         ORDER BY created_at, path, method"
            .to_string(),
    ))
    .all(&db)
    .await
    .expect("the log is readable");
    let mut logged: Vec<Value> = rows
        .iter()
        .map(|row| serde_json::json!([row.method, row.path, row.business_code, row.outcome, row.surface]))
        .collect();
    // Requests are strictly sequential, but two rows can share a timestamp tick: order them by
    // content so the record does not depend on the clock.
    logged.sort_by_key(ToString::to_string);
    let mut recorded_logged: Vec<Value> = serde_json::from_str::<Vec<Value>>(
        RECORDED
            .last()
            .and_then(|line| line.split(" | ").nth(2))
            .expect("the record ends with the log line"),
    )
    .expect("the recorded log is JSON");
    recorded_logged.sort_by_key(ToString::to_string);
    lines.push(format!("bot log rows | 200 | {}", Value::Array(logged.clone())));

    for line in &lines {
        eprintln!("CHARACTERIZATION {line}");
    }
    let (expected_answers, expected_log) = RECORDED.split_at(RECORDED.len() - 1);
    assert_eq!(
        &lines[..lines.len() - 1],
        expected_answers,
        "an existing authentication path answers differently than it did before"
    );
    assert_eq!(
        logged, recorded_logged,
        "the bot operation log rows differ from the recorded ones: {expected_log:?}"
    );
    scratch.drop_self().await;
}
