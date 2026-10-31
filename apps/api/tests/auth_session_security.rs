//! Session revocation, first-admin bootstrap, and workspace user search.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::too_many_lines,
    clippy::panic,
    clippy::print_stderr,
    clippy::indexing_slicing,
    clippy::similar_names
)]

use std::future::Future;

use api::routes::admin::{ResetPasswordRequest, ToggleUserStatusRequest, reset_user_password, toggle_user_status};
use api::routes::auth::{LoginRequest, RefreshRequest, RegisterRequest, login, refresh, register};
use api::routes::member::{SearchUsersQuery, search_users};
use axum::Json;
use axum::body::to_bytes;
use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use platform::app::{AppState, FlowPermissionCacheSlot};
use platform::auth::{JwtClaims, TokenType};
use platform::config::{AppConfig, Secret};
use sea_orm::{ConnectionTrait, Database, DatabaseConnection, DbBackend, Statement};
use serde_json::{Value, json};
use uuid::Uuid;

const TEST_DATABASE_URL_ENV: &str = "OPENPR_TEST_DATABASE_URL";
const JWT_SECRET: &str = "auth-session-security-test-secret";

struct Scratch {
    db: DatabaseConnection,
    name: String,
    admin_url: String,
}

impl Scratch {
    async fn drop_self(self) {
        let Self { db, name, admin_url } = self;
        drop(db);
        let Ok(admin) = Database::connect(&admin_url).await else {
            return;
        };
        let _ = admin
            .execute_unprepared(&format!("DROP DATABASE IF EXISTS \"{name}\" WITH (FORCE)"))
            .await;
    }
}

async fn scratch(label: &str) -> Option<Scratch> {
    let admin_url = std::env::var(TEST_DATABASE_URL_ENV).ok()?;
    let admin = Database::connect(&admin_url)
        .await
        .unwrap_or_else(|err| panic!("{TEST_DATABASE_URL_ENV} is set but unusable: {err}"));
    let name = format!("sylvode_auth_sec_{label}");
    let quoted = format!("\"{name}\"");
    admin
        .execute_unprepared(&format!("DROP DATABASE IF EXISTS {quoted} WITH (FORCE)"))
        .await
        .unwrap_or_else(|err| panic!("could not reset scratch database {name}: {err}"));
    admin
        .execute_unprepared(&format!("CREATE DATABASE {quoted}"))
        .await
        .unwrap_or_else(|err| panic!("could not create scratch database {name}: {err}"));
    let (prefix, _) = admin_url.rsplit_once('/')?;
    let db = Database::connect(format!("{prefix}/{name}"))
        .await
        .unwrap_or_else(|err| panic!("could not connect to scratch database {name}: {err}"));
    migrate(&db).await;
    Some(Scratch { db, name, admin_url })
}

async fn migrate(db: &DatabaseConnection) {
    let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/../../migrations");
    let mut files: Vec<std::path::PathBuf> = std::fs::read_dir(dir)
        .expect("migrations directory is readable")
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| path.extension().is_some_and(|ext| ext == "sql"))
        .collect();
    files.sort();
    for path in files {
        let sql = std::fs::read_to_string(&path).expect("a migration file is readable");
        db.execute_unprepared(&sql)
            .await
            .unwrap_or_else(|err| panic!("applying {} failed: {err}", path.display()));
    }
}

fn state_for(db: DatabaseConnection) -> AppState {
    AppState {
        cfg: AppConfig {
            app_name: "auth-session-security".to_string(),
            bind_addr: "127.0.0.1:0".to_string(),
            database_url: Secret::new("postgres://unused/unused"),
            jwt_secret: Secret::new(JWT_SECRET),
            jwt_access_ttl_seconds: 900,
            jwt_refresh_ttl_seconds: 3600,
            default_author_id: None,
            allow_insecure_cookies: false,
            collab_allowed_origins: Vec::new(),
        },
        db,
        flow_permission_cache: FlowPermissionCacheSlot::default(),
    }
}

async fn body_json(response: Response) -> Value {
    let status = response.status();
    let bytes = to_bytes(response.into_body(), 1024 * 1024)
        .await
        .expect("response body reads");
    let parsed: Value =
        serde_json::from_slice(&bytes).unwrap_or_else(|_| json!({"raw": String::from_utf8_lossy(&bytes)}));
    assert_eq!(status, StatusCode::OK, "{parsed}");
    parsed
}

fn register_request(email: &str, password: &str, name: &str) -> RegisterRequest {
    serde_json::from_value(json!({"email": email, "password": password, "name": name})).expect("register request")
}

async fn call_register(state: &AppState, email: &str) -> Value {
    into_json(
        register(
            State(state.clone()),
            HeaderMap::new(),
            Json(register_request(email, "password1", "Ada")),
        )
        .await,
    )
    .await
}

async fn into_json<T: IntoResponse>(result: Result<T, api::error::ApiError>) -> Value {
    let response = match result {
        Ok(ok) => ok.into_response(),
        Err(err) => err.into_response(),
    };
    body_json(response).await
}

#[tokio::test]
async fn concurrent_first_registrations_produce_one_admin() {
    let Some(scratch) = scratch("bootstrap").await else {
        eprintln!("skipped: {TEST_DATABASE_URL_ENV} is not set");
        return;
    };
    let state = state_for(scratch.db.clone());
    let left = state.clone();
    let right = state.clone();
    let first = tokio::spawn(async move { call_register(&left, "first@example.com").await });
    let second = tokio::spawn(async move { call_register(&right, "second@example.com").await });
    let (left_body, right_body) = tokio::join!(flatten(first), flatten(second));
    let bodies = [&left_body, &right_body];
    let success = bodies.iter().find(|body| body["code"] == 0);
    let failure = bodies.iter().find(|body| body["code"] != 0);
    assert!(
        success.is_some() && failure.is_some(),
        "left={left_body} right={right_body}"
    );
    assert_eq!(success.expect("one success")["data"]["user"]["role"], "admin");
    assert_eq!(failure.expect("one refusal")["code"], 403);

    let admins: i64 = scratch
        .db
        .query_one(Statement::from_string(
            DbBackend::Postgres,
            "SELECT COUNT(*)::bigint AS total FROM users WHERE role = 'admin'".to_string(),
        ))
        .await
        .expect("count query")
        .expect("count row")
        .try_get("", "total")
        .expect("count");
    assert_eq!(admins, 1);
    scratch.drop_self().await;
}

#[tokio::test]
async fn deactivation_and_password_reset_reject_existing_refresh_tokens() {
    let Some(scratch) = scratch("revoke").await else {
        eprintln!("skipped: {TEST_DATABASE_URL_ENV} is not set");
        return;
    };
    let state = state_for(scratch.db.clone());
    let created = call_register(&state, "ada@example.com").await;
    assert_eq!(created["code"], 0, "{created}");
    let user_id = Uuid::parse_str(created["data"]["user"]["id"].as_str().expect("user id")).expect("uuid");
    let refresh_token = created["data"]["tokens"]["refresh_token"]
        .as_str()
        .expect("refresh token")
        .to_string();

    let refreshed = refresh_with(&state, &refresh_token).await;
    assert_eq!(refreshed["code"], 0, "{refreshed}");

    let reset_body = into_json(
        reset_user_password(
            State(state.clone()),
            Path(user_id),
            Json(ResetPasswordRequest {
                new_password: "password2".to_string(),
            }),
        )
        .await,
    )
    .await;
    assert_eq!(reset_body["code"], 0, "{reset_body}");

    let stale = refresh_with(&state, &refresh_token).await;
    assert_eq!(stale["code"], 401, "{stale}");

    let relogin_body = into_json(
        login(
            State(state.clone()),
            Json(
                serde_json::from_value::<LoginRequest>(json!({"email": "ada@example.com", "password": "password2"}))
                    .expect("login"),
            ),
        )
        .await,
    )
    .await;
    assert_eq!(relogin_body["code"], 0, "{relogin_body}");
    let new_refresh = relogin_body["data"]["tokens"]["refresh_token"]
        .as_str()
        .expect("new refresh")
        .to_string();

    let disabled = into_json(
        toggle_user_status(
            State(state.clone()),
            Path(user_id),
            Json(ToggleUserStatusRequest { is_active: Some(false) }),
        )
        .await,
    )
    .await;
    assert_eq!(disabled["code"], 0, "{disabled}");

    let after_disable = refresh_with(&state, &new_refresh).await;
    assert_eq!(after_disable["code"], 401, "{after_disable}");

    let login_disabled = into_json(
        login(
            State(state.clone()),
            Json(
                serde_json::from_value::<LoginRequest>(json!({"email": "ada@example.com", "password": "password2"}))
                    .expect("login"),
            ),
        )
        .await,
    )
    .await;
    assert_eq!(login_disabled["code"], 401, "{login_disabled}");
    scratch.drop_self().await;
}

#[tokio::test]
async fn workspace_user_search_stays_inside_the_workspace_unless_the_caller_is_an_instance_admin() {
    let Some(scratch) = scratch("search").await else {
        eprintln!("skipped: {TEST_DATABASE_URL_ENV} is not set");
        return;
    };
    let state = state_for(scratch.db.clone());
    let workspace_id = Uuid::new_v4();
    let workspace_admin = Uuid::new_v4();
    let instance_admin = Uuid::new_v4();
    let member = Uuid::new_v4();
    let outsider = Uuid::new_v4();
    for (id, email, name, role) in [
        (workspace_admin, "ws-admin@example.com", "Workspace Admin", "user"),
        (instance_admin, "root@example.com", "Root Admin", "admin"),
        (member, "member@example.com", "Member", "user"),
        (outsider, "outsider@example.com", "Outsider", "user"),
    ] {
        scratch
            .db
            .execute(Statement::from_sql_and_values(
                DbBackend::Postgres,
                "INSERT INTO users (id, email, password_hash, name, role, is_active) VALUES ($1, $2, '!', $3, $4, true)",
                vec![id.into(), email.into(), name.into(), role.into()],
            ))
            .await
            .expect("insert user");
    }
    scratch
        .db
        .execute(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "INSERT INTO workspaces (id, slug, name, created_by) VALUES ($1, $2, 'search ws', $3)",
            vec![
                workspace_id.into(),
                format!("ws-{workspace_id}").into(),
                workspace_admin.into(),
            ],
        ))
        .await
        .expect("insert workspace");
    for (user_id, role) in [
        (workspace_admin, "admin"),
        (instance_admin, "admin"),
        (member, "member"),
    ] {
        scratch
            .db
            .execute(Statement::from_sql_and_values(
                DbBackend::Postgres,
                "INSERT INTO workspace_members (workspace_id, user_id, role) VALUES ($1, $2, $3)",
                vec![workspace_id.into(), user_id.into(), role.into()],
            ))
            .await
            .expect("insert member");
    }

    let scoped = search(
        &state,
        workspace_admin,
        workspace_id,
        Some("outsider@example.com".to_string()),
    )
    .await;
    assert_eq!(scoped["code"], 0, "{scoped}");
    assert!(
        scoped["data"]["items"].as_array().expect("items").is_empty(),
        "{scoped}"
    );

    let global = search(
        &state,
        instance_admin,
        workspace_id,
        Some("outsider@example.com".to_string()),
    )
    .await;
    assert_eq!(global["code"], 0, "{global}");
    let emails = global["data"]["items"]
        .as_array()
        .expect("items")
        .iter()
        .filter_map(|item| item["email"].as_str())
        .collect::<Vec<_>>();
    assert_eq!(emails, vec!["outsider@example.com"]);
    scratch.drop_self().await;
}

async fn refresh_with(state: &AppState, token: &str) -> Value {
    let request: RefreshRequest = serde_json::from_value(json!({"refresh_token": token})).expect("refresh request");
    into_json(refresh(State(state.clone()), HeaderMap::new(), Json(request)).await).await
}

async fn search(state: &AppState, user_id: Uuid, workspace_id: Uuid, term: Option<String>) -> Value {
    into_json(
        search_users(
            State(state.clone()),
            axum::Extension(JwtClaims {
                sub: user_id.to_string(),
                email: format!("{user_id}@example.com"),
                token_type: TokenType::Access,
                iat: 0,
                exp: 0,
                sv: 0,
            }),
            Path(workspace_id),
            Query(SearchUsersQuery {
                search: term,
                limit: Some(20),
            }),
        )
        .await,
    )
    .await
}

async fn flatten(joined: impl Future<Output = Result<Value, tokio::task::JoinError>>) -> Value {
    joined.await.expect("register task")
}
