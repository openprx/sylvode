//! A runner bot's token reaches a route only by the route's class, and a route that resolves to a
//! project only for a project the bot has an enabled connection for. An ordinary bot is not
//! affected. Every route of the table is exercised through the real bot authentication, and every
//! kind of resolver is exercised with real rows in a connected, a disabled and a never-connected
//! project.
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

use super::runner_bot_scope::{Class, REFUSAL, RUNNER_BOT_ROUTES};
use crate::middleware::bot_auth::bot_or_user_auth_middleware;
use crate::routes::context::tenant_fixture::{exec, seed_bot_user, state_for};
use crate::scratch_or_skip;
use crate::services::bot_connection_tests::{
    W, block_token, existing_bot, expect_connect, new_bot, second_project, set_enabled, text_of, world,
};
use crate::services::bots::{self, NewBot};

/// The router under test: every table pattern plus the extra ones, each answering `ok` behind the
/// real authentication, registered per route as `main.rs` does.
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
        let fallback = if name.starts_with('*') {
            "x".to_string()
        } else {
            Uuid::new_v4().to_string()
        };
        out.push_str(&values.get(name).cloned().unwrap_or(fallback));
        rest = &rest[end + 1..];
    }
    out.push_str(rest);
    out
}

async fn call(app: &Router, method: &str, path: &str, token: &str, body: Option<Value>) -> (u16, Value) {
    let mut builder = Request::builder()
        .method(method)
        .uri(path)
        .header(header::AUTHORIZATION, format!("Bearer {token}"))
        .header("x-sylvode-mcp-surface", "mcp_stdio");
    if body.is_some() {
        builder = builder.header(header::CONTENT_TYPE, "application/json");
    }
    let payload = body.map_or_else(Body::empty, |json| Body::from(json.to_string()));
    let response = app
        .clone()
        .oneshot(builder.body(payload).expect("request"))
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
    (answer.1["code"] == 403 && answer.1["message"] == REFUSAL) || answer.0 == 403
}

struct Cast {
    w: W,
    /// The runner bot's token and id, and an ordinary bot's token, same workspace.
    token: String,
    bot: Uuid,
    plain_token: String,
    /// Connected and enabled / connected and disabled / never connected.
    on: Uuid,
    off: Uuid,
    never: Uuid,
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
        bot,
        plain_token: plain.token,
        on: on_project,
        off,
        never,
    }
}

/// Every route of the table, called with ids that exist nowhere: no project and own-resource routes
/// pass, admin and refused routes are refused, and a project-resolved route is refused because its
/// target is not in any connected project. An ordinary bot passes every one of them.
#[tokio::test]
async fn every_route_is_handled_by_its_class() {
    let scratch = scratch_or_skip!("runner_scope_table");
    let db = &scratch.db;
    let c = cast(db, "t1").await;
    let app = router(db, &[]);
    let mut counts: BTreeMap<&str, usize> = BTreeMap::new();
    for entry in RUNNER_BOT_ROUTES {
        let what = format!("{} {} ({})", entry.method, entry.pattern, entry.class.name());
        let path = path_of(entry.pattern, &BTreeMap::new());
        let runner = call(
            &app,
            entry.method,
            &path,
            &c.token,
            Some(Value::Object(serde_json::Map::new())),
        )
        .await;
        match entry.class {
            Class::NoProject | Class::Own => assert!(allowed(&runner), "{what}: {runner:?}"),
            Class::Admin | Class::Refused | Class::Project(_) => assert!(refused(&runner), "{what}: {runner:?}"),
        }
        let plain = call(
            &app,
            entry.method,
            &path,
            &c.plain_token,
            Some(Value::Object(serde_json::Map::new())),
        )
        .await;
        assert!(allowed(&plain), "{what}: an ordinary bot is unchanged: {plain:?}");
        *counts.entry(entry.class.name()).or_default() += 1;
    }
    assert_eq!(counts.values().sum::<usize>(), RUNNER_BOT_ROUTES.len());
    scratch.drop_self().await;
}

/// Routes that are not in the table are refused for a runner bot, and only for it.
#[tokio::test]
async fn a_route_that_is_not_in_the_table_is_refused_for_a_runner_bot_only() {
    let scratch = scratch_or_skip!("runner_scope_closed");
    let db = &scratch.db;
    let c = cast(db, "t2").await;
    let extra = [
        ("/api/v1/unclassified/{thing}", Method::GET),
        ("/api/v1/unclassified/{thing}", Method::POST),
    ];
    let app = router(db, &extra);
    for (pattern, method) in extra {
        let path = path_of(pattern, &BTreeMap::new());
        assert!(
            refused(&call(&app, method.as_str(), &path, &c.token, None).await),
            "{method} {pattern}"
        );
        assert!(
            allowed(&call(&app, method.as_str(), &path, &c.plain_token, None).await),
            "{method} {pattern}: an ordinary bot is unchanged"
        );
    }
    scratch.drop_self().await;
}

type Make = Box<dyn Fn(Uuid, &Rows) -> BTreeMap<&'static str, String>>;

struct Rows {
    issue: Uuid,
    identifier: String,
    comment: Uuid,
    sprint: Uuid,
    form: Uuid,
    flow_object: Option<Uuid>,
}

/// Rows of one project: an issue (and its identifier), a comment, a sprint, a form and a Flow object.
async fn rows_of(db: &DatabaseConnection, c: &Cast, project: Uuid, tag: &str) -> Rows {
    let issue = Uuid::new_v4();
    exec(
        db,
        "INSERT INTO work_items (id, project_id, title, state, created_by) VALUES ($1, $2, 'scope', 'todo', $3)",
        vec![issue.into(), project.into(), c.w.owner.into()],
    )
    .await;
    let identifier = text_of(
        db,
        "SELECT p.key || '-' || wi.sequence_number AS v FROM work_items wi \
         JOIN projects p ON p.id = wi.project_id WHERE wi.id = $1",
        vec![issue.into()],
    )
    .await;
    let comment = Uuid::new_v4();
    exec(
        db,
        "INSERT INTO comments (id, work_item_id, author_id, body) VALUES ($1, $2, $3, 'hi')",
        vec![comment.into(), issue.into(), c.w.owner.into()],
    )
    .await;
    let sprint = Uuid::new_v4();
    exec(
        db,
        "INSERT INTO sprints (id, project_id, name) VALUES ($1, $2, $3)",
        vec![sprint.into(), project.into(), format!("s-{tag}").into()],
    )
    .await;
    let form = Uuid::new_v4();
    exec(
        db,
        "INSERT INTO project_forms (id, workspace_id, project_id, key, name) VALUES ($1, $2, $3, 'bugs', 'Bugs')",
        vec![form.into(), c.w.tenant.workspace_id.into(), project.into()],
    )
    .await;
    // A project's pages hang under the project's navigator root.
    let object = Uuid::new_v4();
    exec(
        db,
        "INSERT INTO flow_objects (id, workspace_id, object_type, project_id, parent_id) \
         VALUES ($1, $2, 'page', $3, flow_ensure_navigator_root($2, $3))",
        vec![object.into(), c.w.tenant.workspace_id.into(), project.into()],
    )
    .await;
    Rows {
        issue,
        identifier,
        comment,
        sprint,
        form,
        flow_object: Some(object),
    }
}

/// Every kind of resolver, with real rows: allowed in the connected project, refused in the disabled
/// one and in the one that was never connected.
#[tokio::test]
async fn each_resolver_allows_the_connected_project_only() {
    let scratch = scratch_or_skip!("runner_scope_resolvers");
    let db = &scratch.db;
    let c = cast(db, "t3").await;
    let app = router(db, &[]);
    let on = rows_of(db, &c, c.on, "on").await;
    let off = rows_of(db, &c, c.off, "off").await;
    let never = rows_of(db, &c, c.never, "never").await;

    // One representative route per resolver: (method, pattern, the values for the target).
    let cases: Vec<(&str, &str, Make)> = vec![
        (
            "GET",
            "/api/v1/projects/{project_id}/context",
            Box::new(|p, _| BTreeMap::from([("project_id", p.to_string())])),
        ),
        (
            "GET",
            "/api/v1/issues/{issue_id}/comments",
            Box::new(|_, r| BTreeMap::from([("issue_id", r.issue.to_string())])),
        ),
        (
            "GET",
            "/api/v1/issues/by-identifier/{identifier}",
            Box::new(|_, r| BTreeMap::from([("identifier", r.identifier.clone())])),
        ),
        (
            "PUT",
            "/api/v1/comments/{id}",
            Box::new(|_, r| BTreeMap::from([("id", r.comment.to_string())])),
        ),
        (
            "PUT",
            "/api/v1/sprints/{id}",
            Box::new(|_, r| BTreeMap::from([("id", r.sprint.to_string())])),
        ),
        (
            "GET",
            "/api/v1/forms/{form_id}",
            Box::new(|_, r| BTreeMap::from([("form_id", r.form.to_string())])),
        ),
        (
            "GET",
            "/api/v2/flow/objects/{object_id}",
            Box::new(|_, r| BTreeMap::from([("object_id", r.flow_object.expect("object").to_string())])),
        ),
    ];
    for (method, pattern, make) in &cases {
        for (project, rows, expect_allowed, label) in [
            (c.on, &on, true, "connected"),
            (c.off, &off, false, "disabled"),
            (c.never, &never, false, "never connected"),
        ] {
            let path = path_of(pattern, &make(project, rows));
            let answer = call(&app, method, &path, &c.token, None).await;
            assert_eq!(
                allowed(&answer),
                expect_allowed,
                "{method} {pattern} for the {label} project: {answer:?}"
            );
            assert!(allowed(&call(&app, method, &path, &c.plain_token, None).await));
        }
    }

    // The project named in a body, and in the query.
    for (project, expect_allowed) in [(c.on, true), (c.off, false), (c.never, false)] {
        let body =
            serde_json::json!({"object_type": "page", "project_id": project, "title": "t", "idempotency_key": "k"});
        let answer = call(&app, "POST", "/api/v2/flow/objects", &c.token, Some(body)).await;
        assert_eq!(allowed(&answer), expect_allowed, "body project {project}: {answer:?}");
        let path = format!("/api/v1/search?q=x&project_id={project}");
        let answer = call(&app, "GET", &path, &c.token, None).await;
        assert_eq!(allowed(&answer), expect_allowed, "query project {project}: {answer:?}");
    }
    // A body that names no project, or only a workspace-level parent, is refused; so is no query.
    for body in [
        serde_json::json!({"object_type": "page", "title": "t"}),
        serde_json::json!({"object_type": "page", "parent_object_id": Uuid::new_v4()}),
    ] {
        assert!(refused(
            &call(&app, "POST", "/api/v2/flow/objects", &c.token, Some(body)).await
        ));
    }
    assert!(refused(&call(&app, "GET", "/api/v1/search?q=x", &c.token, None).await));
    // The body still reaches the handler intact.
    let echoed = call(
        &app,
        "POST",
        "/api/v2/flow/objects",
        &c.token,
        Some(serde_json::json!({"project_id": c.on})),
    )
    .await;
    assert!(allowed(&echoed));

    // Proposals: the one project the linked issues belong to; the bot's own while unlinked; else no.
    let by = |author: Uuid, linked: Option<Uuid>| {
        let id = Uuid::new_v4().to_string();
        let sql_proposal = (id.clone(), author.to_string());
        async move {
            exec(
                db,
                "INSERT INTO proposals (id, title, proposal_type, author_id, author_type, content, workspace_id) \
                 VALUES ($1, 't', 'feature', $2, 'human', 'c', $3)",
                vec![
                    sql_proposal.0.clone().into(),
                    sql_proposal.1.into(),
                    c.w.tenant.workspace_id.into(),
                ],
            )
            .await;
            if let Some(issue) = linked {
                exec(
                    db,
                    "INSERT INTO proposal_issue_links (proposal_id, issue_id) VALUES ($1, $2)",
                    vec![sql_proposal.0.clone().into(), issue.into()],
                )
                .await;
            }
            id
        }
    };
    let proposals = [
        (
            by(c.w.owner, Some(on.issue)).await,
            true,
            "linked to the connected project",
        ),
        (
            by(c.w.owner, Some(off.issue)).await,
            false,
            "linked to the disabled project",
        ),
        (
            by(c.w.owner, Some(never.issue)).await,
            false,
            "linked to the unconnected project",
        ),
        (by(c.bot, None).await, true, "unlinked, authored by the bot"),
        (by(c.w.owner, None).await, false, "unlinked, authored by someone else"),
    ];
    for (id, expect_allowed, label) in proposals {
        let answer = call(&app, "GET", &format!("/api/v1/proposals/{id}"), &c.token, None).await;
        assert_eq!(allowed(&answer), expect_allowed, "proposal {label}: {answer:?}");
    }
    let spread = by(c.w.owner, Some(on.issue)).await;
    exec(
        db,
        "INSERT INTO proposal_issue_links (proposal_id, issue_id) VALUES ($1, $2)",
        vec![spread.clone().into(), never.issue.into()],
    )
    .await;
    assert!(refused(
        &call(&app, "GET", &format!("/api/v1/proposals/{spread}"), &c.token, None).await
    ));

    // A Flow scope is allowed only when it is the bot's own personal scope.
    let personal = Uuid::new_v4();
    exec(
        db,
        "INSERT INTO flow_scopes (id, kind, owner_user_id) VALUES ($1, 'personal', $2)",
        vec![personal.into(), c.bot.into()],
    )
    .await;
    let foreign_scope = Uuid::new_v4();
    let someone = seed_bot_user(db, &c.w.tenant).await;
    exec(
        db,
        "INSERT INTO flow_scopes (id, kind, owner_user_id) VALUES ($1, 'personal', $2)",
        vec![foreign_scope.into(), someone.into()],
    )
    .await;
    for (scope, expect_allowed) in [(personal, true), (foreign_scope, false), (Uuid::new_v4(), false)] {
        let answer = call(
            &app,
            "GET",
            &format!("/api/v2/flow/scopes/{scope}/health"),
            &c.token,
            None,
        )
        .await;
        assert_eq!(allowed(&answer), expect_allowed, "scope {scope}: {answer:?}");
    }
    let _ = (&never.form, &off.flow_object);
    scratch.drop_self().await;
}

/// A runner bot calling a task that is not its own gets exactly the answer it gets for a task that
/// does not exist, on the protocol routes and on the callback routes: the scope middleware lets
/// every task route through and the handlers answer 404 for both, so task ids are no oracle.
#[tokio::test]
async fn another_bots_task_and_a_missing_task_get_one_answer() {
    use crate::routes::ai_task_protocol as proto;
    use axum::routing::{get, post};
    let scratch = scratch_or_skip!("runner_scope_task_oracle");
    let db = &scratch.db;
    let c = cast(db, "t4").await;
    let state = state_for(db.clone());
    let other_bot = seed_bot_user(db, &c.w.tenant).await;
    let theirs = crate::services::ai_task_fixture::seed_task(db, &c.w.tenant, other_bot, "processing", 1, 5).await;
    let own = crate::services::ai_task_fixture::seed_task(db, &c.w.tenant, c.bot, "processing", 1, 5).await;
    let layer = || from_fn_with_state(state.clone(), bot_or_user_auth_middleware);
    let app = Router::new()
        .route("/api/v1/ai/tasks/{task_id}", get(proto::get_task).route_layer(layer()))
        .route(
            "/api/v1/ai/tasks/{task_id}/ack",
            post(proto::ack_task).route_layer(layer()),
        )
        .route(
            "/api/v1/ai/tasks/{task_id}/progress",
            post(proto::report_task_progress).route_layer(layer()),
        )
        .route(
            "/api/v1/ai/tasks/{task_id}/complete",
            post(proto::complete_task_with_reply).route_layer(layer()),
        )
        .route(
            "/api/v1/ai/tasks/{task_id}/fail",
            post(proto::fail_task_with_reply).route_layer(layer()),
        )
        .with_state(state.clone());
    let calls = [
        ("GET", "/api/v1/ai/tasks/{id}?attempt=1", serde_json::json!({})),
        (
            "POST",
            "/api/v1/ai/tasks/{id}/ack?attempt=1",
            serde_json::json!({"plan": "p"}),
        ),
        (
            "POST",
            "/api/v1/ai/tasks/{id}/progress?attempt=1",
            serde_json::json!({"note": "n"}),
        ),
        (
            "POST",
            "/api/v1/ai/tasks/{id}/complete?attempt=1",
            serde_json::json!({"outcome": "done", "summary": "s"}),
        ),
        (
            "POST",
            "/api/v1/ai/tasks/{id}/fail?attempt=1",
            serde_json::json!({"reason": "r", "retryable": false}),
        ),
    ];
    for (method, template, body) in calls {
        let ask = |id: Uuid| {
            let path = template.replace("{id}", &id.to_string());
            let app = app.clone();
            let token = c.token.clone();
            let body = body.clone();
            async move { call(&app, method, &path, &token, Some(body)).await }
        };
        let missing = ask(Uuid::new_v4()).await;
        let foreign = ask(theirs).await;
        assert_eq!(
            missing, foreign,
            "{method} {template}: a task of another bot and a missing task differ"
        );
        assert!(
            missing.1.to_string().to_lowercase().contains("not found") || missing.1["code"] == 404,
            "{method} {template}: both answer 404, got {missing:?}"
        );
        // The bot's own task is not refused by the scope or as someone else's.
        let mine = ask(own).await;
        assert_ne!(mine.1["code"], 404, "{method} {template}: own task {mine:?}");
        assert!(
            !refused(&mine),
            "{method} {template}: own task refused by scope {mine:?}"
        );
    }
    scratch.drop_self().await;
}
