//! Flow comment threads and public page links.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::too_many_lines,
    clippy::panic,
    clippy::print_stderr,
    clippy::indexing_slicing,
    clippy::similar_names
)]

use std::net::{IpAddr, Ipv4Addr};

use api::routes::flow::{
    CommentPath, ShareClientIp, ShareLinkPath, TokenPath, delete_flow_comment, get_flow_comments,
    get_public_flow_share, list_flow_share_links, patch_flow_comment, post_flow_comment, post_flow_share_link,
    resolve_flow_comment, revoke_flow_share_link,
};
use axum::Json;
use axum::body::to_bytes;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::IntoResponse;
use platform::app::{AppState, FlowPermissionCacheSlot};
use platform::auth::{JwtClaims, TokenType};
use platform::config::{AppConfig, Secret};
use sea_orm::{ConnectionTrait, Database, DatabaseConnection, DbBackend, FromQueryResult, Statement};
use serde::Deserialize;
use serde_json::{Value, json};
use uuid::Uuid;

const TEST_DATABASE_URL_ENV: &str = "OPENPR_TEST_DATABASE_URL";

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
    let name = format!("sylvode_flow_social_{label}");
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
            app_name: "flow-comments-shares".to_string(),
            bind_addr: "127.0.0.1:0".to_string(),
            database_url: Secret::new("postgres://unused/unused"),
            jwt_secret: Secret::new("flow-comments-shares-test-secret"),
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

fn claims(user_id: Uuid) -> axum::Extension<JwtClaims> {
    axum::Extension(JwtClaims {
        sub: user_id.to_string(),
        email: format!("{user_id}@example.com"),
        token_type: TokenType::Access,
        iat: 0,
        exp: 0,
        sv: 0,
    })
}

async fn into_json<T, E>(result: Result<T, E>) -> Value
where
    T: IntoResponse,
    E: IntoResponse,
{
    let response = match result {
        Ok(ok) => ok.into_response(),
        Err(err) => err.into_response(),
    };
    let status = response.status();
    let bytes = to_bytes(response.into_body(), 1024 * 1024)
        .await
        .expect("response body reads");
    let parsed: Value =
        serde_json::from_slice(&bytes).unwrap_or_else(|_| json!({"raw": String::from_utf8_lossy(&bytes)}));
    assert_eq!(status, StatusCode::OK, "{parsed}");
    parsed
}

async fn exec(db: &DatabaseConnection, sql: &str, values: Vec<sea_orm::Value>) {
    db.execute(Statement::from_sql_and_values(DbBackend::Postgres, sql, values))
        .await
        .expect("statement");
}

#[derive(FromQueryResult)]
struct RootRow {
    id: Uuid,
}

async fn seed_user(db: &DatabaseConnection, email: &str, name: &str) -> Uuid {
    let id = Uuid::new_v4();
    exec(
        db,
        "INSERT INTO users (id, email, password_hash, name, role, is_active) VALUES ($1, $2, '', $3, 'user', true)",
        vec![id.into(), email.into(), name.into()],
    )
    .await;
    id
}

#[tokio::test]
async fn comments_and_public_links_follow_object_permissions() {
    let Some(scratch) = scratch("perms").await else {
        eprintln!("{TEST_DATABASE_URL_ENV} unset; skipping flow comment and share test");
        return;
    };
    let state = state_for(scratch.db.clone());
    let db = &state.db;
    let owner = seed_user(db, "owner@example.com", "Owner").await;
    let commenter = seed_user(db, "commenter@example.com", "Commenter").await;
    let editor = seed_user(db, "editor@example.com", "Editor").await;
    let viewer = seed_user(db, "viewer@example.com", "Viewer").await;
    let outsider = seed_user(db, "outsider@example.com", "Outsider").await;
    let workspace_id = Uuid::new_v4();
    let page_id = Uuid::new_v4();
    let child_id = Uuid::new_v4();
    exec(
        db,
        "INSERT INTO workspaces (id, slug, name, created_by) VALUES ($1, $2, 'Flow', $3)",
        vec![workspace_id.into(), format!("ws-{workspace_id}").into(), owner.into()],
    )
    .await;
    for (user, role) in [
        (owner, "owner"),
        (commenter, "member"),
        (editor, "member"),
        (viewer, "member"),
    ] {
        exec(
            db,
            "INSERT INTO workspace_members (workspace_id, user_id, role) VALUES ($1, $2, $3)",
            vec![workspace_id.into(), user.into(), role.into()],
        )
        .await;
    }
    exec(
        db,
        "INSERT INTO flow_workspace_settings (workspace_id, flow_enabled, default_member_level) VALUES ($1, true, 'view')",
        vec![workspace_id.into()],
    )
    .await;
    let navigator_id = RootRow::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "SELECT id FROM flow_objects WHERE workspace_id = $1 AND object_type = 'navigator' AND parent_id IS NULL",
        vec![workspace_id.into()],
    ))
    .one(db)
    .await
    .expect("navigator query")
    .expect("workspace navigator root")
    .id;
    exec(
        db,
        "INSERT INTO flow_objects (id, workspace_id, parent_id, object_type, inherit_from_parent, lifecycle_status, created_by) \
         VALUES ($1, $2, $3, 'page', false, 'active', $4)",
        vec![page_id.into(), workspace_id.into(), navigator_id.into(), owner.into()],
    )
    .await;
    exec(
        db,
        "INSERT INTO flow_objects (id, workspace_id, parent_id, object_type, inherit_from_parent, lifecycle_status, created_by) \
         VALUES ($1, $2, $3, 'page', false, 'active', $4)",
        vec![child_id.into(), workspace_id.into(), page_id.into(), owner.into()],
    )
    .await;
    exec(
        db,
        "INSERT INTO flow_object_projections (object_id, document_seq, document_frontier, title, plain_text) \
         VALUES ($1, 1, $2, $3, $4)",
        vec![
            page_id.into(),
            vec![0_u8].into(),
            "Parent <b>title</b>".into(),
            "Hello <script>alert(1)</script> &amp; world".into(),
        ],
    )
    .await;
    exec(
        db,
        "INSERT INTO flow_object_projections (object_id, document_seq, document_frontier, title, plain_text) \
         VALUES ($1, 1, $2, 'Child', $3)",
        vec![child_id.into(), vec![0_u8].into(), "child secret body".into()],
    )
    .await;
    for (user, level) in [(commenter, "comment"), (editor, "edit"), (viewer, "view")] {
        exec(
            db,
            "INSERT INTO flow_object_grants (id, workspace_id, object_id, principal_kind, principal_id, level, granted_by) \
             VALUES ($1, $2, $3, 'user', $4, $5, $6)",
            vec![
                Uuid::new_v4().into(),
                workspace_id.into(),
                page_id.into(),
                user.into(),
                level.into(),
                owner.into(),
            ],
        )
        .await;
    }

    let listed = into_json(get_flow_comments(State(state.clone()), claims(viewer), None, Path(page_id)).await).await;
    assert_eq!(listed["code"], 0, "{listed}");
    assert_eq!(listed["data"]["items"].as_array().expect("items").len(), 0);

    let denied = into_json(
        post_flow_comment(
            State(state.clone()),
            claims(viewer),
            None,
            Path(page_id),
            Json(body_json(json!({"body": "nope"}))),
        )
        .await,
    )
    .await;
    assert_eq!(denied["error_code"], "not_found", "{denied}");

    let created = into_json(
        post_flow_comment(
            State(state.clone()),
            claims(commenter),
            None,
            Path(page_id),
            Json(body_json(json!({"body": " hello ", "mentions": [editor, outsider]}))),
        )
        .await,
    )
    .await;
    assert_eq!(created["code"], 0, "{created}");
    let comment_id = created["data"]["id"].as_str().expect("comment id").to_string();
    assert_eq!(notification_count(db, editor).await, 1);
    assert_eq!(notification_count(db, outsider).await, 0);

    let reply = into_json(
        post_flow_comment(
            State(state.clone()),
            claims(commenter),
            None,
            Path(page_id),
            Json(body_json(json!({"body": "reply", "parent_id": comment_id}))),
        )
        .await,
    )
    .await;
    assert_eq!(reply["code"], 0, "{reply}");
    assert_eq!(reply["data"]["parent_id"], comment_id);

    let edited = into_json(
        patch_flow_comment(
            State(state.clone()),
            claims(commenter),
            None,
            Path(comment_path(&page_id, &comment_id)),
            Json(body_json(json!({"body": "edited"}))),
        )
        .await,
    )
    .await;
    assert_eq!(edited["data"]["body"], "edited", "{edited}");

    let editor_note = into_json(
        post_flow_comment(
            State(state.clone()),
            claims(editor),
            None,
            Path(page_id),
            Json(body_json(json!({"body": "editor note"}))),
        )
        .await,
    )
    .await;
    let editor_comment = editor_note["data"]["id"].as_str().expect("editor comment").to_string();
    let cannot_edit = into_json(
        patch_flow_comment(
            State(state.clone()),
            claims(commenter),
            None,
            Path(comment_path(&page_id, &editor_comment)),
            Json(body_json(json!({"body": "stolen"}))),
        )
        .await,
    )
    .await;
    assert_eq!(cannot_edit["error_code"], "forbidden", "{cannot_edit}");

    let cannot_resolve = into_json(
        resolve_flow_comment(
            State(state.clone()),
            claims(commenter),
            None,
            Path(comment_path(&page_id, &editor_comment)),
            Json(body_json(json!({"resolved": true}))),
        )
        .await,
    )
    .await;
    assert_eq!(cannot_resolve["error_code"], "forbidden", "{cannot_resolve}");
    let resolved = into_json(
        resolve_flow_comment(
            State(state.clone()),
            claims(editor),
            None,
            Path(comment_path(&page_id, &editor_comment)),
            Json(body_json(json!({"resolved": true}))),
        )
        .await,
    )
    .await;
    assert!(resolved["data"]["resolved_at"].is_string(), "{resolved}");

    let removed = into_json(
        delete_flow_comment(
            State(state.clone()),
            claims(commenter),
            None,
            Path(comment_path(&page_id, &comment_id)),
        )
        .await,
    )
    .await;
    assert_eq!(removed["code"], 0, "{removed}");
    let after_delete =
        into_json(get_flow_comments(State(state.clone()), claims(viewer), None, Path(page_id)).await).await;
    let visible = after_delete["data"]["items"].as_array().expect("items");
    assert!(
        visible
            .iter()
            .all(|item| item["id"] != comment_id && item["parent_id"] != comment_id)
    );

    let owner_denied_link = into_json(
        post_flow_share_link(
            State(state.clone()),
            claims(commenter),
            None,
            Path(page_id),
            Json(body_json(json!({}))),
        )
        .await,
    )
    .await;
    assert_eq!(owner_denied_link["error_code"], "forbidden", "{owner_denied_link}");

    let link = into_json(
        post_flow_share_link(
            State(state.clone()),
            claims(owner),
            None,
            Path(page_id),
            Json(body_json(json!({}))),
        )
        .await,
    )
    .await;
    assert_eq!(link["code"], 0, "{link}");
    let token = link["data"]["token"].as_str().expect("token").to_string();
    let link_id = link["data"]["link"]["id"].as_str().expect("link id").to_string();
    let listed_links =
        into_json(list_flow_share_links(State(state.clone()), claims(owner), None, Path(page_id)).await).await;
    let rendered = listed_links.to_string();
    assert!(!rendered.contains(&token), "list must not echo the raw token");

    let public = into_json(
        get_public_flow_share(
            State(state.clone()),
            Path(token_path(&token)),
            ShareClientIp(IpAddr::V4(Ipv4Addr::new(203, 0, 113, 10))),
        )
        .await,
    )
    .await;
    assert_eq!(public["data"]["plain_text"], "Hello alert(1) & world", "{public}");
    assert_eq!(public["data"]["title"], "Parent title", "{public}");
    assert!(!public.to_string().contains("child secret body"));
    assert!(public.get("data").and_then(|data| data.get("workspace_id")).is_none());

    let revoked = into_json(
        revoke_flow_share_link(
            State(state.clone()),
            claims(owner),
            None,
            Path(share_path(&page_id, &link_id)),
        )
        .await,
    )
    .await;
    assert_eq!(revoked["code"], 0, "{revoked}");
    let gone = into_json(
        get_public_flow_share(
            State(state.clone()),
            Path(token_path(&token)),
            ShareClientIp(IpAddr::V4(Ipv4Addr::new(203, 0, 113, 11))),
        )
        .await,
    )
    .await;
    assert_eq!(gone["error_code"], "not_found", "{gone}");

    let expiring = into_json(
        post_flow_share_link(
            State(state.clone()),
            claims(owner),
            None,
            Path(page_id),
            Json(body_json(json!({"expires_at": "2027-06-01T00:00:00Z"}))),
        )
        .await,
    )
    .await;
    let expiring_token = expiring["data"]["token"].as_str().expect("expiring token").to_string();
    let expiring_id = expiring["data"]["link"]["id"]
        .as_str()
        .expect("expiring id")
        .to_string();
    exec(
        db,
        "UPDATE flow_share_links SET expires_at = now() - interval '1 minute' WHERE id = $1",
        vec![Uuid::parse_str(&expiring_id).expect("link uuid").into()],
    )
    .await;
    let expired = into_json(
        get_public_flow_share(
            State(state.clone()),
            Path(token_path(&expiring_token)),
            ShareClientIp(IpAddr::V4(Ipv4Addr::new(203, 0, 113, 12))),
        )
        .await,
    )
    .await;
    assert_eq!(expired["error_code"], "not_found", "{expired}");

    let limited_ip = ShareClientIp(IpAddr::V4(Ipv4Addr::new(203, 0, 113, 60)));
    let fresh = into_json(
        post_flow_share_link(
            State(state.clone()),
            claims(owner),
            None,
            Path(page_id),
            Json(body_json(json!({}))),
        )
        .await,
    )
    .await;
    let fresh_token = fresh["data"]["token"].as_str().expect("fresh token").to_string();
    let mut limited = json!(null);
    for _ in 0..61 {
        limited =
            into_json(get_public_flow_share(State(state.clone()), Path(token_path(&fresh_token)), limited_ip).await)
                .await;
    }
    assert_eq!(limited["error_code"], "limit_exceeded", "{limited}");

    scratch.drop_self().await;
}

fn body_json<T: for<'de> Deserialize<'de>>(value: Value) -> T {
    serde_json::from_value(value).expect("request json")
}

fn comment_path(object_id: &Uuid, comment_id: &str) -> CommentPath {
    CommentPath {
        object_id: *object_id,
        comment_id: Uuid::parse_str(comment_id).expect("comment id"),
    }
}

fn token_path(token: &str) -> TokenPath {
    TokenPath {
        token: token.to_string(),
    }
}

fn share_path(object_id: &Uuid, link_id: &str) -> ShareLinkPath {
    ShareLinkPath {
        object_id: *object_id,
        link_id: Uuid::parse_str(link_id).expect("link id"),
    }
}

async fn notification_count(db: &DatabaseConnection, user_id: Uuid) -> i64 {
    #[derive(FromQueryResult)]
    struct Count {
        n: i64,
    }
    Count::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "SELECT count(*)::bigint AS n FROM notifications WHERE user_id = $1 AND type = 'mentioned'",
        vec![user_id.into()],
    ))
    .one(db)
    .await
    .expect("count")
    .expect("row")
    .n
}
