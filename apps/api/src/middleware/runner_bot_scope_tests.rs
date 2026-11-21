//! A runner bot's token reaches only the routes of `RUNNER_BOT_ROUTES`, and a route that names a
//! project or an issue only for a project the bot has an enabled connection for. An ordinary bot is
//! not affected. Every entry of the table is exercised through the real bot authentication.
//!
//! Needs `OPENPR_TEST_DATABASE_URL`; without it every test reports that it was skipped.
#![cfg(test)]
#![allow(clippy::expect_used, clippy::panic, clippy::indexing_slicing, clippy::too_many_lines)]

use std::collections::BTreeMap;

use axum::{
    Router,
    body::Body,
    http::{Method, Request, header},
    middleware::from_fn_with_state,
    routing::MethodRouter,
};
use sea_orm::DatabaseConnection;
use serde_json::Value;
use tower::ServiceExt;
use uuid::Uuid;

use super::runner_bot_scope::{RUNNER_BOT_ROUTES, Scope, lookup};
use crate::middleware::bot_auth::bot_or_user_auth_middleware;
use crate::oauth::delegated::Resolver;
use crate::routes::context::tenant_fixture::{exec, state_for};
use crate::scratch_or_skip;
use crate::services::bot_connection_tests::{
    W, block_token, existing_bot, expect_connect, new_bot, second_project, set_enabled, world,
};
use crate::services::bots::{self, NewBot};

/// The router under test: every table pattern plus the extra patterns, each answering `ok` behind
/// the real authentication, registered per route as `main.rs` does.
fn router(db: &DatabaseConnection, extra: &[(&'static str, Method)]) -> Router {
    let state = state_for(db.clone());
    let mut by_pattern: BTreeMap<&'static str, MethodRouter<platform::app::AppState>> = BTreeMap::new();
    let all = RUNNER_BOT_ROUTES
        .iter()
        .map(|entry| {
            (
                entry.pattern,
                Method::from_bytes(entry.method.as_bytes()).expect("method"),
            )
        })
        .chain(extra.iter().cloned());
    for (pattern, method) in all {
        let filter = axum::routing::MethodFilter::try_from(method).expect("filter");
        let router = by_pattern.remove(pattern).unwrap_or_default();
        by_pattern.insert(pattern, router.on(filter, || async { "ok" }));
    }
    let mut app = Router::new();
    for (pattern, methods) in by_pattern {
        app = app.route(
            pattern,
            methods.route_layer(from_fn_with_state(state.clone(), bot_or_user_auth_middleware)),
        );
    }
    app.with_state(state)
}

/// The concrete path of `pattern`, with each `{name}` filled in from `values`, else a fresh id.
fn path_of(pattern: &str, values: &BTreeMap<&str, String>) -> String {
    let mut out = String::new();
    let mut rest = pattern;
    while let Some(start) = rest.find('{') {
        out.push_str(&rest[..start]);
        let end = rest[start..].find('}').expect("closing brace") + start;
        let name = &rest[start + 1..end];
        out.push_str(&values.get(name).cloned().unwrap_or_else(|| Uuid::new_v4().to_string()));
        rest = &rest[end + 1..];
    }
    out.push_str(rest);
    out
}

async fn call(app: &Router, method: &str, path: &str, token: &str) -> (u16, Value) {
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method(method)
                .uri(path)
                .header(header::AUTHORIZATION, format!("Bearer {token}"))
                .header("x-sylvode-mcp-surface", "mcp_stdio")
                .body(Body::empty())
                .expect("request"),
        )
        .await
        .expect("answer");
    let status = response.status().as_u16();
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("body");
    let json = serde_json::from_slice(&bytes).unwrap_or_else(|_| Value::String(String::from_utf8_lossy(&bytes).into()));
    (status, json)
}

fn allowed(answer: &(u16, Value)) -> bool {
    answer.0 == 200 && answer.1 == Value::String("ok".into())
}

fn refused(answer: &(u16, Value)) -> bool {
    answer.1["code"] == 403 && answer.1["message"] == super::runner_bot_scope::REFUSAL || answer.0 == 403
}

struct Cast {
    w: W,
    /// The runner bot's token.
    token: String,
    /// An ordinary bot's token, same workspace.
    plain_token: String,
    /// Connected and enabled / connected and disabled / never connected.
    on: Uuid,
    off: Uuid,
    never: Uuid,
}

impl Cast {
    /// The values a path of project `project` takes.
    async fn values(&self, db: &DatabaseConnection, project: Uuid) -> BTreeMap<&'static str, String> {
        let issue = Uuid::new_v4();
        exec(
            db,
            "INSERT INTO work_items (id, project_id, title, state, created_by) VALUES ($1, $2, 'scope', 'todo', $3)",
            vec![issue.into(), project.into(), self.w.owner.into()],
        )
        .await;
        let identifier = crate::services::bot_connection_tests::text_of(
            db,
            "SELECT p.key || '-' || wi.sequence_number AS v FROM work_items wi \
             JOIN projects p ON p.id = wi.project_id WHERE wi.id = $1",
            vec![issue.into()],
        )
        .await;
        BTreeMap::from([
            ("project_id", project.to_string()),
            ("id", String::new()), // filled per resolver below
            ("issue_id", issue.to_string()),
            ("identifier", identifier),
            ("__issue", issue.to_string()),
        ])
    }
}

async fn cast(db: &DatabaseConnection, tag: &str) -> Cast {
    let w = world(db, tag).await;
    let on_project = w.tenant.project_id;
    let off = second_project(db, &w, &format!("S{tag}2")).await;
    let never = second_project(db, &w, &format!("S{tag}3")).await;
    let first = expect_connect(&w, on_project, new_bot("Scoped bot")).await;
    let token = block_token(first.config_block.as_ref().expect("a new bot shows its block"));
    let bot = first.connection.bot.id;
    let second = expect_connect(&w, off, existing_bot(bot)).await;
    set_enabled(&w, off, second.connection.version, false)
        .await
        .expect("disable");
    let tx = sea_orm::TransactionTrait::begin(db).await.expect("begin");
    let plain = bots::create(
        &tx,
        NewBot {
            workspace_id: w.tenant.workspace_id,
            created_by: w.owner,
            name: "plain bot",
            permissions: vec!["read".to_string(), "write".to_string()],
            transport_surface: "mcp_stdio".to_string(),
            expires_at: None,
        },
    )
    .await
    .expect("plain bot");
    tx.commit().await.expect("commit");
    Cast {
        w,
        token,
        plain_token: plain.token,
        on: on_project,
        off,
        never,
    }
}

/// The path for one table entry against `project`: the resolver decides which value is the id.
async fn entry_path(c: &Cast, db: &DatabaseConnection, pattern: &str, scope: Scope, project: Uuid) -> String {
    let mut values = c.values(db, project).await;
    let issue = values["__issue"].clone();
    match scope {
        Scope::Project(Resolver::Project("id")) => {
            values.insert("id", project.to_string());
        }
        Scope::Project(Resolver::Issue("id")) => {
            values.insert("id", issue);
        }
        _ => {
            values.insert("id", Uuid::new_v4().to_string());
        }
    }
    path_of(pattern, &values)
}

#[tokio::test]
async fn every_route_of_the_table_is_allowed_for_a_connected_project_and_refused_for_others() {
    let scratch = scratch_or_skip!("runner_scope_table");
    let db = &scratch.db;
    let c = cast(db, "t1").await;
    let app = router(db, &[]);
    assert!(!RUNNER_BOT_ROUTES.is_empty());
    for entry in RUNNER_BOT_ROUTES {
        let what = format!("{} {}", entry.method, entry.pattern);
        match entry.scope {
            Scope::NoProject => {
                let path = entry_path(&c, db, entry.pattern, entry.scope, c.never).await;
                assert!(
                    allowed(&call(&app, entry.method, &path, &c.token).await),
                    "{what}: allowed without a project"
                );
            }
            Scope::Project(_) => {
                let on = entry_path(&c, db, entry.pattern, entry.scope, c.on).await;
                assert!(
                    allowed(&call(&app, entry.method, &on, &c.token).await),
                    "{what}: allowed for a connected, enabled project"
                );
                for (label, project) in [("disabled", c.off), ("never connected", c.never)] {
                    let path = entry_path(&c, db, entry.pattern, entry.scope, project).await;
                    let answer = call(&app, entry.method, &path, &c.token).await;
                    assert!(
                        refused(&answer),
                        "{what}: refused for a {label} project, got {answer:?}"
                    );
                }
                // An id that does not exist is refused the same way.
                let unknown = path_of(entry.pattern, &BTreeMap::from([("identifier", "NOPE-1".to_string())]));
                let answer = call(&app, entry.method, &unknown, &c.token).await;
                assert!(refused(&answer), "{what}: refused for an unknown id, got {answer:?}");
            }
        }
        // An ordinary bot is not scoped, whatever the project.
        let path = entry_path(&c, db, entry.pattern, entry.scope, c.never).await;
        assert!(
            allowed(&call(&app, entry.method, &path, &c.plain_token).await),
            "{what}: an ordinary bot is unchanged"
        );
    }
    assert!(lookup("GET", "/api/v1/projects/{id}").is_some());
    scratch.drop_self().await;
}

#[tokio::test]
async fn a_route_that_is_not_in_the_table_is_refused_for_a_runner_bot_only() {
    let scratch = scratch_or_skip!("runner_scope_closed");
    let db = &scratch.db;
    let c = cast(db, "t2").await;
    let extra = [
        ("/api/v1/workspaces/{workspace_id}/members", Method::GET),
        ("/api/v1/workspaces/{workspace_id}/webhooks", Method::POST),
        ("/api/v1/projects/{id}", Method::DELETE),
        ("/api/v1/projects/{project_id}/issues", Method::DELETE),
    ];
    let app = router(db, &extra);
    for (pattern, method) in extra {
        let mut values = c.values(db, c.on).await;
        values.insert("workspace_id", c.w.tenant.workspace_id.to_string());
        values.insert("id", c.on.to_string());
        let path = path_of(pattern, &values);
        let answer = call(&app, method.as_str(), &path, &c.token).await;
        assert!(
            refused(&answer),
            "{method} {pattern}: refused for a runner bot, got {answer:?}"
        );
        assert!(
            allowed(&call(&app, method.as_str(), &path, &c.plain_token).await),
            "{method} {pattern}: an ordinary bot is unchanged"
        );
    }
    // The refusal does not say which part failed.
    let answer = call(
        &app,
        "GET",
        &path_of("/api/v1/workspaces/{workspace_id}/members", &BTreeMap::new()),
        &c.token,
    )
    .await;
    assert_eq!(answer.1["message"], super::runner_bot_scope::REFUSAL);
    scratch.drop_self().await;
}
