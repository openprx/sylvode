//! A project-scoped bot (`project_scope = 'projects'`) is held to its level in the project a request
//! targets: `read` for safe methods, `comment` for the comment routes, `write` for the rest; the
//! default level covers the projects without a grant; the token permissions stay a ceiling. Also the
//! `project-access` endpoints and `auth/me` for a bot token, through the real handlers and the real
//! bot authentication.
//!
//! Needs `OPENPR_TEST_DATABASE_URL`; without it every test reports that it was skipped.
#![cfg(test)]
#![allow(
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::too_many_lines,
    clippy::literal_string_with_formatting_args
)]

use std::collections::BTreeMap;

use axum::{Router, middleware::from_fn_with_state, routing::get};
use sea_orm::DatabaseConnection;
use serde_json::{Value, json};
use uuid::Uuid;

use super::scoped_bot::Level;
use super::scoped_bot_tests::{Cast, Rows, allowed, call, cast, path_of, refused, router, rows_of};
use crate::middleware::bot_auth::bot_or_user_auth_middleware;
use crate::routes::context::tenant_fixture::{count, exec, seed_tenant, state_for};
use crate::scratch_or_skip;
use crate::services::bots::{self, NewBot};

/// A bot of the cast's workspace with a token, the given permissions, and a project scope.
async fn scoped_bot(
    db: &DatabaseConnection,
    c: &Cast,
    name: &str,
    permissions: &[&str],
    default_level: &str,
    grants: &[(Uuid, &str)],
) -> (String, Uuid) {
    let tx = sea_orm::TransactionTrait::begin(db).await.expect("begin");
    let bot = bots::create(
        &tx,
        NewBot {
            workspace_id: c.w.tenant.workspace_id,
            created_by: c.w.owner,
            name,
            permissions: permissions.iter().map(|value| (*value).to_string()).collect(),
            transport_surface: "mcp_stdio".to_string(),
            expires_at: None,
        },
    )
    .await
    .expect("scoped bot");
    tx.commit().await.expect("commit");
    exec(
        db,
        "UPDATE workspace_bots SET project_scope = 'projects', default_project_level = $2 WHERE id = $1",
        vec![bot.id.into(), default_level.to_string().into()],
    )
    .await;
    for (project, level) in grants {
        exec(
            db,
            "INSERT INTO bot_project_grants (bot_id, project_id, level) VALUES ($1, $2, $3)",
            vec![bot.id.into(), (*project).into(), (*level).to_string().into()],
        )
        .await;
    }
    (bot.token, bot.id)
}

/// A request and the level it needs.
struct Case {
    needs: Level,
    method: &'static str,
    pattern: &'static str,
    values: fn(&Rows, Uuid) -> BTreeMap<&'static str, String>,
}

fn cases() -> Vec<Case> {
    vec![
        Case {
            needs: Level::Read,
            method: "GET",
            pattern: "/api/v1/issues/{issue_id}/comments",
            values: |r, _| BTreeMap::from([("issue_id", r.issue.to_string())]),
        },
        Case {
            needs: Level::Read,
            method: "GET",
            pattern: "/api/v1/projects/{project_id}/board",
            values: |_, p| BTreeMap::from([("project_id", p.to_string())]),
        },
        Case {
            needs: Level::Comment,
            method: "POST",
            pattern: "/api/v1/issues/{issue_id}/comments",
            values: |r, _| BTreeMap::from([("issue_id", r.issue.to_string())]),
        },
        Case {
            needs: Level::Comment,
            method: "PUT",
            pattern: "/api/v1/comments/{id}",
            values: |r, _| BTreeMap::from([("id", r.comment.to_string())]),
        },
        Case {
            needs: Level::Comment,
            method: "DELETE",
            pattern: "/api/v1/comments/{id}",
            values: |r, _| BTreeMap::from([("id", r.comment.to_string())]),
        },
        Case {
            needs: Level::Comment,
            method: "POST",
            pattern: "/api/v2/flow/objects/{object_id}/comments",
            values: |r, _| BTreeMap::from([("object_id", r.flow_object.expect("object").to_string())]),
        },
        Case {
            needs: Level::Write,
            method: "PUT",
            pattern: "/api/v1/issues/{id}",
            values: |r, _| BTreeMap::from([("id", r.issue.to_string())]),
        },
        Case {
            needs: Level::Write,
            method: "POST",
            pattern: "/api/v1/projects/{project_id}/issues",
            values: |_, p| BTreeMap::from([("project_id", p.to_string())]),
        },
        Case {
            needs: Level::Write,
            method: "POST",
            pattern: "/api/v2/flow/objects/{object_id}/commands",
            values: |r, _| BTreeMap::from([("object_id", r.flow_object.expect("object").to_string())]),
        },
    ]
}

async fn matrix_against(app: &Router, token: &str, projects: &[(Uuid, &Rows, Level)], label: &str) {
    for case in cases() {
        for (project, rows, level) in projects {
            let path = path_of(case.pattern, &(case.values)(rows, *project));
            let answer = call(app, case.method, &path, token, Some(json!({}))).await;
            assert_eq!(
                allowed(&answer),
                *level >= case.needs,
                "{label}: {} {} needs {:?}, bot has {level:?}: {answer:?}",
                case.method,
                case.pattern,
                case.needs
            );
            if !allowed(&answer) {
                assert!(refused(&answer), "{label}: one refusal for everything: {answer:?}");
                assert_eq!(answer.1["message"], super::scoped_bot::REFUSAL);
            }
        }
    }
}

/// Route classes x levels x methods, the comment routes, and the default level for a project
/// without a grant.
#[tokio::test]
async fn scoped_bot_levels_decide_each_route() {
    let scratch = scratch_or_skip!("scoped_bot_matrix");
    let db = &scratch.db;
    let c = cast(db, "m1").await;
    let fresh = crate::services::bot_connection_tests::second_project(db, &c.w, "Sm14").await;
    let on = rows_of(db, &c, c.on, "on").await;
    let off = rows_of(db, &c, c.off, "off").await;
    let never = rows_of(db, &c, c.never, "never").await;
    let other = rows_of(db, &c, fresh, "fresh").await;
    let (token, bot) = scoped_bot(
        db,
        &c,
        "scoped",
        &["read", "write"],
        "none",
        &[(c.on, "write"), (c.off, "comment"), (c.never, "read")],
    )
    .await;
    let app = router(db, &[]);

    matrix_against(
        &app,
        &token,
        &[
            (c.on, &on, Level::Write),
            (c.off, &off, Level::Comment),
            (c.never, &never, Level::Read),
            (fresh, &other, Level::None),
        ],
        "default none",
    )
    .await;

    // The default level covers the projects without a grant, and only those.
    for (default, level) in [("read", Level::Read), ("comment", Level::Comment)] {
        exec(
            db,
            "UPDATE workspace_bots SET default_project_level = $2 WHERE id = $1",
            vec![bot.into(), default.to_string().into()],
        )
        .await;
        matrix_against(
            &app,
            &token,
            &[
                (c.on, &on, Level::Write),
                (c.off, &off, Level::Comment),
                (c.never, &never, Level::Read),
                (fresh, &other, level),
            ],
            default,
        )
        .await;
    }

    // A grant below the default lowers the level for that project.
    exec(
        db,
        "UPDATE bot_project_grants SET level = 'read' WHERE bot_id = $1 AND project_id = $2",
        vec![bot.into(), c.off.into()],
    )
    .await;
    matrix_against(&app, &token, &[(c.off, &off, Level::Read)], "grant below default").await;
    scratch.drop_self().await;
}

/// Workspace-wide listings (class `Refused`) open for safe methods when the default level is at
/// least `read`; administration never opens; no-project and own routes are unchanged.
#[tokio::test]
async fn refused_class_follows_the_default_level_and_admin_never_opens() {
    let scratch = scratch_or_skip!("scoped_bot_listings");
    let db = &scratch.db;
    let c = cast(db, "m2").await;
    let (token, bot) = scoped_bot(db, &c, "lister", &["read", "write", "admin"], "none", &[]).await;
    let app = router(db, &[]);
    let ws = c.w.tenant.workspace_id;
    let listing = "/api/v1/my/issues";
    let collab = "/api/v2/flow/collab/tickets";
    let members = format!("/api/v1/workspaces/{ws}/members");
    let search_without_project = "/api/v1/search?q=x";

    for (default, listing_open) in [("none", false), ("read", true), ("comment", true)] {
        exec(
            db,
            "UPDATE workspace_bots SET default_project_level = $2 WHERE id = $1",
            vec![bot.into(), default.to_string().into()],
        )
        .await;
        assert_eq!(
            allowed(&call(&app, "GET", listing, &token, None).await),
            listing_open,
            "listing at default {default}"
        );
        assert!(
            refused(&call(&app, "POST", collab, &token, Some(json!({}))).await),
            "an unsafe Refused route at default {default}"
        );
        assert!(
            refused(&call(&app, "GET", &members, &token, None).await),
            "an Admin route at default {default}, even for an admin token"
        );
        assert!(
            refused(&call(&app, "POST", &members, &token, Some(json!({}))).await),
            "an Admin route at default {default}, even for an admin token"
        );
        assert!(
            refused(&call(&app, "GET", search_without_project, &token, None).await),
            "search without a project at default {default}"
        );
        for path in ["/api/v1/notifications", "/api/v1/auth/me", "/api/v1/project-types"] {
            assert!(
                allowed(&call(&app, "GET", path, &token, None).await),
                "{path} at default {default}"
            );
        }
    }
    scratch.drop_self().await;
}

/// The token's own permissions stay a ceiling: a `read` token with a `write` grant never writes.
#[tokio::test]
async fn the_token_permission_is_a_ceiling() {
    let scratch = scratch_or_skip!("scoped_bot_ceiling");
    let db = &scratch.db;
    let c = cast(db, "m3").await;
    let on = rows_of(db, &c, c.on, "on").await;
    let (read_only, _) = scoped_bot(db, &c, "read only", &["read"], "none", &[(c.on, "write")]).await;
    let app = router(db, &[]);
    for case in cases() {
        let path = path_of(case.pattern, &(case.values)(&on, c.on));
        let answer = call(&app, case.method, &path, &read_only, Some(json!({}))).await;
        assert_eq!(
            allowed(&answer),
            case.needs == Level::Read,
            "{} {}: {answer:?}",
            case.method,
            case.pattern
        );
    }
    scratch.drop_self().await;
}

fn user_token(state: &platform::app::AppState, user: Uuid) -> String {
    platform::auth::JwtManager::new(state.cfg.jwt_secret.expose(), 900, 3600)
        .issue_access_token(&user.to_string(), &format!("{user}@tenant.test"))
        .expect("a user token")
}

/// The real handlers behind the real bot authentication.
fn real_router(db: &DatabaseConnection) -> Router {
    use crate::routes::{auth, bot};
    let state = state_for(db.clone());
    let layer = || from_fn_with_state(state.clone(), bot_or_user_auth_middleware);
    Router::new()
        .route("/api/v1/auth/me", get(auth::me).route_layer(layer()))
        .route(
            "/api/v1/workspaces/{workspace_id}/bots",
            get(bot::list_bots).route_layer(layer()),
        )
        .route(
            "/api/v1/workspaces/{workspace_id}/bots/{bot_id}/project-access",
            get(bot::get_project_access)
                .put(bot::put_project_access)
                .route_layer(layer()),
        )
        .with_state(state)
}

/// `auth/me`: a bot token names its bot, workspace, effective permissions and scope; a person gets
/// `null`. A project-scoped bot is never an administrator: `admin` counts as `write`.
#[tokio::test]
async fn auth_me_describes_a_bot_token() {
    let scratch = scratch_or_skip!("scoped_bot_me");
    let db = &scratch.db;
    let c = cast(db, "m4").await;
    let app = real_router(db);
    let ws = c.w.tenant.workspace_id;

    let user = call(&app, "GET", "/api/v1/auth/me", &user_token(&c.w.state, c.w.owner), None).await;
    assert_eq!(user.0, 200, "{user:?}");
    assert_eq!(user.1["data"]["bot"], Value::Null);
    assert_eq!(user.1["data"]["user"]["id"], c.w.owner.to_string());

    let (admin_token, admin_bot) = scoped_bot(db, &c, "scoped admin", &["admin"], "none", &[]).await;
    let scoped = call(&app, "GET", "/api/v1/auth/me", &admin_token, None).await;
    assert_eq!(scoped.0, 200, "{scoped:?}");
    assert_eq!(
        scoped.1["data"]["bot"],
        json!({
            "bot_id": admin_bot,
            "workspace_id": ws,
            "permissions": ["write"],
            "project_scope": "projects"
        })
    );

    let plain = call(&app, "GET", "/api/v1/auth/me", &c.plain_token, None).await;
    assert_eq!(plain.1["data"]["bot"]["project_scope"], "workspace");
    assert_eq!(plain.1["data"]["bot"]["workspace_id"], ws.to_string());
    assert_eq!(plain.1["data"]["bot"]["permissions"], json!(["read", "write"]));

    let runner = call(&app, "GET", "/api/v1/auth/me", &c.token, None).await;
    assert_eq!(runner.1["data"]["bot"]["bot_id"], c.bot.to_string());
    assert_eq!(runner.1["data"]["bot"]["project_scope"], "workspace");
    scratch.drop_self().await;
}

/// The business code of an answer: the API answers errors with HTTP 200 and the code in the body.
fn code(answer: &(u16, Value)) -> i64 {
    answer.1["code"].as_i64().unwrap_or_else(|| i64::from(answer.0))
}

fn access(scope: &str, default: &str, grants: &[(Uuid, &str)]) -> Value {
    json!({
        "project_scope": scope,
        "default_level": default,
        "grants": grants.iter().map(|(p, l)| json!({"project_id": p, "level": l})).collect::<Vec<_>>(),
    })
}

/// `project-access`: replace in one transaction, validation, managed bots, who may call it, the audit
/// event, and the list answer.
#[tokio::test]
async fn project_access_endpoints() {
    let scratch = scratch_or_skip!("scoped_bot_access_api");
    let db = &scratch.db;
    let c = cast(db, "m5").await;
    let app = real_router(db);
    let ws = c.w.tenant.workspace_id;
    let owner = user_token(&c.w.state, c.w.owner);
    let (_, bot) = scoped_bot(db, &c, "target", &["read", "write"], "none", &[]).await;
    let url = |bot: Uuid| format!("/api/v1/workspaces/{ws}/bots/{bot}/project-access");

    // A new bot is workspace-wide (the helper made it project-scoped; reset it to a fresh bot's state).
    exec(
        db,
        "UPDATE workspace_bots SET project_scope = 'workspace' WHERE id = $1",
        vec![bot.into()],
    )
    .await;
    let got = call(&app, "GET", &url(bot), &owner, None).await;
    assert_eq!(got.1["data"], access("workspace", "none", &[]));

    // put_replaces_grants
    let mut first = vec![(c.on, "write"), (c.off, "comment")];
    first.sort_by_key(|(project, _)| *project);
    let put = call(
        &app,
        "PUT",
        &url(bot),
        &owner,
        Some(access("projects", "read", &[(c.on, "write"), (c.off, "comment")])),
    )
    .await;
    assert_eq!(code(&put), 0, "{put:?}");
    assert_eq!(put.1["data"], access("projects", "read", &first));
    let second = [(c.never, "read")];
    let put = call(
        &app,
        "PUT",
        &url(bot),
        &owner,
        Some(access("projects", "comment", &second)),
    )
    .await;
    assert_eq!(code(&put), 0, "{put:?}");
    let got = call(&app, "GET", &url(bot), &owner, None).await;
    assert_eq!(got.1["data"], access("projects", "comment", &second));
    assert_eq!(
        count(
            db,
            "SELECT COUNT(*)::bigint AS count FROM bot_project_grants WHERE bot_id = $1",
            vec![bot.into()]
        )
        .await,
        1,
        "the first set of grants is gone"
    );
    assert_eq!(
        count(
            db,
            "SELECT COUNT(*)::bigint AS count FROM business_events \
             WHERE event_type = 'bot.project_access_changed' AND aggregate_id = $1 AND workspace_id = $2 \
               AND actor_id = $3",
            vec![bot.to_string().into(), ws.into(), c.w.owner.into()]
        )
        .await,
        2
    );

    // The list answer carries the scope, the default level and the number of grants.
    let list = call(&app, "GET", &format!("/api/v1/workspaces/{ws}/bots"), &owner, None).await;
    let row = list.1["data"]
        .as_array()
        .expect("rows")
        .iter()
        .find(|row| row["id"] == bot.to_string())
        .expect("the bot is listed")
        .clone();
    assert_eq!(row["project_scope"], "projects");
    assert_eq!(row["default_project_level"], "comment");
    assert_eq!(row["grant_count"], 1);

    // Back to workspace-wide clears the grants.
    let put = call(&app, "PUT", &url(bot), &owner, Some(access("workspace", "none", &[]))).await;
    assert_eq!(code(&put), 0, "{put:?}");
    assert_eq!(put.1["data"], access("workspace", "none", &[]));

    // Validation: every rejection leaves the stored access as it was.
    let foreign = seed_tenant(db, "m5x").await.project_id;
    let before = call(&app, "GET", &url(bot), &owner, None).await.1["data"].clone();
    for (label, body) in [
        (
            "unknown project",
            access("projects", "none", &[(Uuid::new_v4(), "read")]),
        ),
        (
            "project of another workspace",
            access("projects", "none", &[(foreign, "read")]),
        ),
        (
            "duplicate project",
            access("projects", "none", &[(c.on, "read"), (c.on, "write")]),
        ),
        ("grant level none", access("projects", "none", &[(c.on, "none")])),
        ("grant level admin", access("projects", "none", &[(c.on, "admin")])),
        ("default level write", access("projects", "write", &[])),
        ("unknown scope", access("all", "none", &[])),
        (
            "workspace scope with grants",
            access("workspace", "none", &[(c.on, "read")]),
        ),
        ("workspace scope with a default", access("workspace", "read", &[])),
        (
            "unknown field",
            json!({"project_scope": "workspace", "default_level": "none", "grants": [], "x": 1}),
        ),
    ] {
        let put = call(&app, "PUT", &url(bot), &owner, Some(body)).await;
        assert!(
            matches!(code(&put), 400 | 422),
            "{label}: expected a rejection, got {put:?}"
        );
        assert_eq!(
            call(&app, "GET", &url(bot), &owner, None).await.1["data"],
            before,
            "{label}: stored access changed"
        );
    }

    // An unknown bot, and a bot of another workspace.
    assert_eq!(code(&call(&app, "GET", &url(Uuid::new_v4()), &owner, None).await), 404);
    // managed_bot_409: a runner bot's reach is its connections'.
    for method in ["GET", "PUT"] {
        let body = (method == "PUT").then(|| access("workspace", "none", &[]));
        let answer = call(&app, method, &url(c.bot), &owner, body).await;
        assert_eq!(code(&answer), 409, "{method}: {answer:?}");
        assert!(
            answer.1["message"]
                .as_str()
                .expect("message")
                .starts_with("managed by a project bot connection"),
            "{answer:?}"
        );
    }

    // Who may call: a workspace member bot may not (not an owner or admin); a scoped bot is refused
    // by its scope, whatever its permissions.
    let body = access("workspace", "none", &[]);
    let member_bot = call(&app, "PUT", &url(bot), &c.plain_token, Some(body.clone())).await;
    assert_eq!(code(&member_bot), 403, "{member_bot:?}");
    let (scoped_admin, _) = scoped_bot(db, &c, "scoped admin 2", &["admin"], "comment", &[]).await;
    let scoped = call(&app, "PUT", &url(bot), &scoped_admin, Some(body)).await;
    assert_eq!(code(&scoped), 403, "{scoped:?}");
    assert_eq!(scoped.1["message"], super::scoped_bot::REFUSAL);
    scratch.drop_self().await;
}

/// A new scope takes effect on the next request, with no cache in between.
#[tokio::test]
async fn a_changed_access_applies_to_the_next_request() {
    let scratch = scratch_or_skip!("scoped_bot_access_live");
    let db = &scratch.db;
    let c = cast(db, "m6").await;
    let app = real_router(db);
    let ws = c.w.tenant.workspace_id;
    let owner = user_token(&c.w.state, c.w.owner);
    let (token, bot) = scoped_bot(db, &c, "live", &["read", "write"], "none", &[]).await;
    // The table-driven router answers `ok` for an allowed request, behind the same authentication.
    let table = router(db, &[]);
    let row = rows_of(db, &c, c.on, "live").await;
    let write = format!("/api/v1/issues/{}", row.issue);
    assert!(refused(&call(&table, "PUT", &write, &token, Some(json!({}))).await));
    let url = format!("/api/v1/workspaces/{ws}/bots/{bot}/project-access");
    let put = call(
        &app,
        "PUT",
        &url,
        &owner,
        Some(access("projects", "none", &[(c.on, "write")])),
    )
    .await;
    assert_eq!(code(&put), 0, "{put:?}");
    assert!(allowed(&call(&table, "PUT", &write, &token, Some(json!({}))).await));
    let put = call(&app, "PUT", &url, &owner, Some(access("projects", "read", &[]))).await;
    assert_eq!(code(&put), 0, "{put:?}");
    assert!(refused(&call(&table, "PUT", &write, &token, Some(json!({}))).await));
    scratch.drop_self().await;
}
