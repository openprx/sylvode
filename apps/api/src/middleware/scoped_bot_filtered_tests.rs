//! The `Filtered` routes (project list, search, the caller's own issues and activities), through the
//! real handlers behind the real bot authentication: a runner bot connected to projects A and B, with
//! a disabled connection to D and none to C, sees the data of A and B only, in every row, count and
//! page total. A project-scoped bot sees the projects it can read, an ordinary bot everything, and
//! the workspace-level routes that stay closed are still refused.
//!
//! Needs `OPENPR_TEST_DATABASE_URL`; without it every test reports that it was skipped.
#![cfg(test)]
#![allow(clippy::expect_used, clippy::panic, clippy::indexing_slicing, clippy::too_many_lines)]

use std::collections::BTreeSet;

use axum::{Router, middleware::from_fn_with_state, routing::get};
use sea_orm::DatabaseConnection;
use serde_json::Value;
use uuid::Uuid;

use super::scoped_bot_projects_tests::scoped_bot;
use super::scoped_bot_tests::{Cast, allowed, call, cast, refused, router};
use crate::middleware::bot_auth::bot_or_user_auth_middleware;
use crate::routes::context::tenant_fixture::{exec, state_for};
use crate::routes::my::{get_my_activities, get_my_issues};
use crate::routes::project::list_projects;
use crate::routes::search::search;
use crate::scratch_or_skip;
use crate::services::bot_connection_tests::{existing_bot, expect_connect, second_project};

fn real_router(db: &DatabaseConnection) -> Router {
    let state = state_for(db.clone());
    let layer = || from_fn_with_state(state.clone(), bot_or_user_auth_middleware);
    Router::new()
        .route(
            "/api/v1/workspaces/{workspace_id}/projects",
            get(list_projects).route_layer(layer()),
        )
        .route("/api/v1/search", get(search).route_layer(layer()))
        .route("/api/v1/my/issues", get(get_my_issues).route_layer(layer()))
        .route("/api/v1/my/activities", get(get_my_activities).route_layer(layer()))
        .with_state(state)
}

/// The cast plus a second connected project: `a` and `b` are connected and enabled, `d` is
/// connected but disabled, `c` was never connected. Every project carries the search needle in its
/// description, and an issue assigned to the runner bot with a comment and an activity entry.
struct World {
    c: Cast,
    a: Uuid,
    b: Uuid,
    c_never: Uuid,
    d_off: Uuid,
}

async fn world(db: &DatabaseConnection, tag: &str) -> World {
    let c = cast(db, tag).await;
    let b = second_project(db, &c.w, &format!("S{tag}4")).await;
    expect_connect(&c.w, b, existing_bot(c.bot)).await;
    let world = World {
        a: c.on,
        b,
        c_never: c.never,
        d_off: c.off,
        c,
    };
    for project in [world.a, world.b, world.c_never, world.d_off] {
        exec(
            db,
            "UPDATE projects SET description = 'needle-project' WHERE id = $1",
            vec![project.into()],
        )
        .await;
        // Two issues per project, so a page of one leaves a second page.
        for n in 0..2 {
            let issue = Uuid::new_v4();
            exec(
                db,
                "INSERT INTO work_items (id, project_id, title, state, created_by, assignee_id) \
                 VALUES ($1, $2, $3, 'todo', $4, $5)",
                vec![
                    issue.into(),
                    project.into(),
                    format!("needle issue {n}").into(),
                    world.c.w.owner.into(),
                    world.c.bot.into(),
                ],
            )
            .await;
            exec(
                db,
                "INSERT INTO comments (id, work_item_id, author_id, body) VALUES ($1, $2, $3, 'needle comment')",
                vec![Uuid::new_v4().into(), issue.into(), world.c.w.owner.into()],
            )
            .await;
            exec(
                db,
                "INSERT INTO activities (resource_type, resource_id, issue_id, event_type, actor_id, payload) \
                 VALUES ('issue', $1, $1, 'created', $2, '{}'::jsonb)",
                vec![issue.into(), world.c.w.owner.into()],
            )
            .await;
        }
    }
    world
}

fn set(ids: &[Uuid]) -> BTreeSet<String> {
    ids.iter().map(ToString::to_string).collect()
}

fn field_set(items: &Value, field: &str) -> BTreeSet<String> {
    items
        .as_array()
        .expect("an array")
        .iter()
        .map(|item| item[field].as_str().expect("a string field").to_string())
        .collect()
}

fn results_of(body: &Value, kind: &str, field: &str) -> BTreeSet<String> {
    body["data"]["results"]
        .as_array()
        .expect("results")
        .iter()
        .filter(|entry| entry["type"] == kind)
        .map(|entry| entry[field].as_str().expect("field").to_string())
        .collect()
}

fn leaks(body: &Value, hidden: &[Uuid]) {
    let text = body.to_string();
    for id in hidden {
        assert!(!text.contains(&id.to_string()), "{id} leaked: {text}");
    }
}

/// `projects.list`: the connected projects only, with and without a page window; the total of a
/// page is the number of connected projects.
#[tokio::test]
async fn the_project_list_of_a_runner_bot_holds_its_connected_projects_only() {
    let scratch = scratch_or_skip!("filtered_projects");
    let db = &scratch.db;
    let w = world(db, "fp").await;
    let app = real_router(db);
    let ws = w.c.w.tenant.workspace_id;
    let hidden = [w.c_never, w.d_off];

    let (status, body) = call(
        &app,
        "GET",
        &format!("/api/v1/workspaces/{ws}/projects"),
        &w.c.token,
        None,
    )
    .await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(field_set(&body["data"]["items"], "id"), set(&[w.a, w.b]), "{body}");
    leaks(&body, &hidden);

    let mut seen = BTreeSet::new();
    for page in 1..=2 {
        let path = format!("/api/v1/workspaces/{ws}/projects?page={page}&per_page=1");
        let (status, body) = call(&app, "GET", &path, &w.c.token, None).await;
        assert_eq!(status, 200, "{body}");
        assert_eq!(
            body["data"]["total"], 2,
            "the total counts the connected projects only: {body}"
        );
        assert_eq!(body["data"]["total_pages"], 2, "{body}");
        assert_eq!(body["data"]["items"].as_array().expect("items").len(), 1);
        leaks(&body, &hidden);
        seen.extend(field_set(&body["data"]["items"], "id"));
    }
    assert_eq!(seen, set(&[w.a, w.b]));

    // An ordinary bot still lists every project of the workspace.
    let (_, body) = call(
        &app,
        "GET",
        &format!("/api/v1/workspaces/{ws}/projects"),
        &w.c.plain_token,
        None,
    )
    .await;
    let all = field_set(&body["data"]["items"], "id");
    assert!(all.is_superset(&set(&[w.a, w.b, w.c_never, w.d_off])), "{body}");
    scratch.drop_self().await;
}

/// `search.all` and `work_items.search`: issues, comments and projects of the connected projects
/// only; a named project that is not connected is a 404, connected or not.
#[tokio::test]
async fn the_search_of_a_runner_bot_reaches_its_connected_projects_only() {
    let scratch = scratch_or_skip!("filtered_search");
    let db = &scratch.db;
    let w = world(db, "fs").await;
    let app = real_router(db);
    let hidden = [w.c_never, w.d_off];

    let (status, body) = call(&app, "GET", "/api/v1/search?q=needle", &w.c.token, None).await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(results_of(&body, "issue", "project_id"), set(&[w.a, w.b]), "{body}");
    assert_eq!(results_of(&body, "comment", "project_id"), set(&[w.a, w.b]), "{body}");
    assert_eq!(results_of(&body, "project", "id"), set(&[w.a, w.b]), "{body}");
    // 2 projects, 4 issues, 4 comments: the total is the number of rows shown.
    assert_eq!(body["data"]["total"], 10, "{body}");
    leaks(&body, &hidden);

    let (_, body) = call(
        &app,
        "GET",
        "/api/v1/search?q=needle&type=issue&limit=3",
        &w.c.token,
        None,
    )
    .await;
    assert_eq!(body["data"]["total"], 3, "{body}");
    leaks(&body, &hidden);

    let (status, body) = call(
        &app,
        "GET",
        &format!("/api/v1/search?q=needle&project_id={}", w.b),
        &w.c.token,
        None,
    )
    .await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(results_of(&body, "issue", "project_id"), set(&[w.b]), "{body}");
    for project in hidden {
        let (status, body) = call(
            &app,
            "GET",
            &format!("/api/v1/search?q=needle&project_id={project}"),
            &w.c.token,
            None,
        )
        .await;
        assert_eq!(
            (status, &body["code"]),
            (200, &Value::from(404)),
            "an unconnected project is not found: {body}"
        );
    }

    // An ordinary bot searches the whole workspace.
    let (_, body) = call(
        &app,
        "GET",
        "/api/v1/search?q=needle&type=issue",
        &w.c.plain_token,
        None,
    )
    .await;
    assert_eq!(
        results_of(&body, "issue", "project_id"),
        set(&[w.a, w.b, w.c_never, w.d_off]),
        "{body}"
    );
    scratch.drop_self().await;
}

/// "My issues" and "my activities": the bot's own rows of the connected projects, the count and
/// the page total included.
#[tokio::test]
async fn my_issues_and_my_activities_of_a_runner_bot_hold_its_connected_projects_only() {
    let scratch = scratch_or_skip!("filtered_my");
    let db = &scratch.db;
    let w = world(db, "fm").await;
    let app = real_router(db);
    let hidden = [w.c_never, w.d_off];

    for path in ["/api/v1/my/issues", "/api/v1/my/activities"] {
        let (status, body) = call(&app, "GET", path, &w.c.token, None).await;
        assert_eq!(status, 200, "{path}: {body}");
        assert_eq!(
            body["data"]["total"], 4,
            "{path}: two issues in each connected project: {body}"
        );
        assert_eq!(
            field_set(&body["data"]["items"], "project_id"),
            set(&[w.a, w.b]),
            "{path}"
        );
        leaks(&body, &hidden);

        let mut rows = 0;
        for page in 1..=2 {
            let (_, body) = call(&app, "GET", &format!("{path}?page={page}&per_page=2"), &w.c.token, None).await;
            assert_eq!(body["data"]["total"], 4, "{path} page {page}: {body}");
            assert_eq!(body["data"]["total_pages"], 2, "{path} page {page}: {body}");
            rows += body["data"]["items"].as_array().expect("items").len();
            leaks(&body, &hidden);
        }
        assert_eq!(rows, 4, "{path}: both pages together");
        let (_, beyond) = call(&app, "GET", &format!("{path}?page=3&per_page=2"), &w.c.token, None).await;
        assert!(beyond["data"]["items"].as_array().expect("items").is_empty(), "{path}");
    }
    // An ordinary bot's own issues span every project.
    let (_, body) = call(&app, "GET", "/api/v1/my/issues", &w.c.plain_token, None).await;
    assert_eq!(body["data"]["total"], 0, "{body}");
    scratch.drop_self().await;
}

/// A project-scoped bot sees the projects whose grant, else default level, reaches `read`.
#[tokio::test]
async fn a_project_scoped_bot_sees_the_projects_it_can_read() {
    let scratch = scratch_or_skip!("filtered_scoped");
    let db = &scratch.db;
    let w = world(db, "fq").await;
    let app = real_router(db);
    let ws = w.c.w.tenant.workspace_id;
    let projects = format!("/api/v1/workspaces/{ws}/projects");

    let (only_b, _) = scoped_bot(db, &w.c, "only b", &["read"], "none", &[(w.b, "read")]).await;
    let (_, body) = call(&app, "GET", &projects, &only_b, None).await;
    assert_eq!(field_set(&body["data"]["items"], "id"), set(&[w.b]), "{body}");
    let (_, body) = call(&app, "GET", "/api/v1/search?q=needle&type=issue", &only_b, None).await;
    assert_eq!(results_of(&body, "issue", "project_id"), set(&[w.b]), "{body}");
    let (_, body) = call(
        &app,
        "GET",
        &format!("/api/v1/search?q=needle&project_id={}", w.a),
        &only_b,
        None,
    )
    .await;
    assert_eq!(body["code"], 404, "{body}");

    let (most, _) = scoped_bot(db, &w.c, "most", &["read"], "read", &[(w.a, "write")]).await;
    let (_, body) = call(&app, "GET", &projects, &most, None).await;
    assert_eq!(
        field_set(&body["data"]["items"], "id"),
        set(&[w.a, w.b, w.c_never, w.d_off]),
        "{body}"
    );
    scratch.drop_self().await;
}

/// The workspace-level routes that stay closed are still refused for a runner bot, and a Filtered
/// route is never open to an unsafe method.
#[tokio::test]
async fn the_other_workspace_routes_stay_refused_for_a_runner_bot() {
    let scratch = scratch_or_skip!("filtered_refused");
    let db = &scratch.db;
    let w = world(db, "fr").await;
    let stub = router(db, &[]);
    let ws = w.c.w.tenant.workspace_id;
    for path in [
        format!("/api/v1/workspaces/{ws}/activities"),
        format!("/api/v1/workspaces/{ws}/labels"),
        "/api/v1/proposals".to_string(),
        "/api/v1/decision-domains".to_string(),
        "/api/v1/trust-scores".to_string(),
        format!("/api/v1/workspaces/{ws}/flow/navigator"),
    ] {
        assert!(
            refused(&call(&stub, "GET", &path, &w.c.token, None).await),
            "GET {path}"
        );
        assert!(
            allowed(&call(&stub, "GET", &path, &w.c.plain_token, None).await),
            "{path}: plain bot"
        );
    }
    // Reaching a Filtered route through an unsafe method is not a classified route at all.
    for route in ["/api/v1/search", "/api/v1/my/issues", "/api/v1/my/activities"] {
        let path = format!("{route}?q=x");
        let app = router(db, &[(route, axum::http::Method::POST)]);
        assert!(
            refused(&call(&app, "POST", &path, &w.c.token, Some(serde_json::json!({}))).await),
            "POST {path}"
        );
    }
    scratch.drop_self().await;
}
