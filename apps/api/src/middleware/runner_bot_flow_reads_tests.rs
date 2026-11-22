//! The Flow reads of a project, through the real handlers behind the real bot authentication: a
//! runner bot connected to a project lists, searches and reads the trash of that project's Flow
//! objects and of nothing else, and is refused when the request names no connected project. An
//! ordinary bot is unchanged.
//!
//! Rows are seeded directly (no Flow write path), so these tests do not need the isolated apply
//! worker. Needs `OPENPR_TEST_DATABASE_URL`; without it every test reports that it was skipped.
#![cfg(test)]
#![allow(clippy::expect_used, clippy::panic, clippy::indexing_slicing, clippy::too_many_lines)]

use std::collections::BTreeSet;

use axum::{Router, middleware::from_fn_with_state, routing::get};
use sea_orm::DatabaseConnection;
use serde_json::Value;
use uuid::Uuid;

use super::runner_bot_scope_tests::{Cast, call, cast, refused};
use crate::middleware::bot_auth::bot_or_user_auth_middleware;
use crate::routes::context::tenant_fixture::{exec, state_for};
use crate::routes::flow::{get_flow_search, get_flow_trash, list_flow_objects};
use crate::scratch_or_skip;

fn flow_router(db: &DatabaseConnection) -> Router {
    let state = state_for(db.clone());
    let layer = || from_fn_with_state(state.clone(), bot_or_user_auth_middleware);
    Router::new()
        .route(
            "/api/v1/workspaces/{workspace_id}/flow/objects",
            get(list_flow_objects).route_layer(layer()),
        )
        .route(
            "/api/v1/workspaces/{workspace_id}/flow/search",
            get(get_flow_search).route_layer(layer()),
        )
        .route(
            "/api/v1/workspaces/{workspace_id}/flow/trash",
            get(get_flow_trash).route_layer(layer()),
        )
        .with_state(state)
}

/// The answer's code: the API answers an error with its code in the envelope.
fn code_of(status: u16, body: &Value) -> u64 {
    body["code"].as_u64().unwrap_or_else(|| u64::from(status))
}

struct Seed {
    /// Active objects titled `needle ...`, archived ones, per place.
    active: BTreeSet<String>,
    archived: BTreeSet<String>,
}

/// One page of `workspace`'s Flow in `project` (or in no project), seeded straight into the tables:
/// the object, its collaborative document row at `seq`, its projection and its search index row.
async fn seed_object(
    db: &DatabaseConnection,
    workspace: Uuid,
    project: Option<Uuid>,
    title: &str,
    archived: bool,
    seq: i64,
) -> Uuid {
    let id = Uuid::new_v4();
    exec(
        db,
        "INSERT INTO flow_objects (id, workspace_id, object_type, project_id, parent_id, lifecycle_status, archived_at) \
         VALUES ($1, $2, 'page', $3, flow_ensure_navigator_root($2, $3), \
                 CASE WHEN $4 THEN 'archived' ELSE 'active' END, CASE WHEN $4 THEN now() END)",
        vec![id.into(), workspace.into(), project.into(), archived.into()],
    )
    .await;
    exec(
        db,
        "INSERT INTO collab_documents (object_id, format_version, snapshot, snapshot_frontier, head_seq, head_frontier) \
         VALUES ($1, 'loro-1', '\\x'::bytea, '\\x'::bytea, $2, '\\x'::bytea)",
        vec![id.into(), seq.into()],
    )
    .await;
    exec(
        db,
        "INSERT INTO flow_object_projections (object_id, document_seq, document_frontier, title, state, plain_text) \
         VALUES ($1, $2, '\\x'::bytea, $3, '{}'::jsonb, $3)",
        vec![id.into(), seq.into(), title.to_string().into()],
    )
    .await;
    exec(
        db,
        "INSERT INTO flow_search_index (object_id, indexed_seq, indexed_frontier, title, plain_text) \
         SELECT object_id, document_seq, document_frontier, title, plain_text \
           FROM flow_object_projections WHERE object_id = $1",
        vec![id.into()],
    )
    .await;
    id
}

struct World {
    c: Cast,
    app: Router,
    /// The enabled project's objects, the never-connected project's, the disabled project's and the
    /// workspace's own (no project).
    on: Seed,
    never: Seed,
    off: Seed,
    none: Seed,
    workspace: Uuid,
}

fn ids(seed: &Seed) -> BTreeSet<String> {
    seed.active.union(&seed.archived).cloned().collect()
}

async fn place(db: &DatabaseConnection, workspace: Uuid, project: Option<Uuid>, tag: &str, seq: i64) -> Seed {
    let mut seed = Seed {
        active: BTreeSet::new(),
        archived: BTreeSet::new(),
    };
    for n in 0..3 {
        let id = seed_object(db, workspace, project, &format!("needle {tag} page {n}"), false, seq).await;
        seed.active.insert(id.to_string());
    }
    for n in 0..2 {
        let id = seed_object(db, workspace, project, &format!("needle {tag} gone {n}"), true, seq).await;
        seed.archived.insert(id.to_string());
    }
    seed
}

async fn world(db: &DatabaseConnection, tag: &str) -> World {
    let c = cast(db, tag).await;
    let workspace = c.w.tenant.workspace_id;
    exec(
        db,
        "INSERT INTO flow_workspace_settings (workspace_id, flow_enabled) VALUES ($1, true) \
         ON CONFLICT (workspace_id) DO UPDATE SET flow_enabled = true",
        vec![workspace.into()],
    )
    .await;
    // The projects the bot cannot read carry the higher sequence numbers, so a frontier that
    // included them would show.
    let on = place(db, workspace, Some(c.on), "alpha", 1).await;
    let never = place(db, workspace, Some(c.never), "bravo", 9).await;
    let off = place(db, workspace, Some(c.off), "charlie", 9).await;
    let none = place(db, workspace, None, "delta", 9).await;
    World {
        app: flow_router(db),
        c,
        on,
        never,
        off,
        none,
        workspace,
    }
}

fn items_of(body: &Value, key: &[&str]) -> BTreeSet<String> {
    body["data"]["items"]
        .as_array()
        .unwrap_or_else(|| panic!("no items: {body}"))
        .iter()
        .map(|item| {
            let mut at = item;
            for part in key {
                at = &at[*part];
            }
            at.as_str().unwrap_or_else(|| panic!("no id in {item}")).to_string()
        })
        .collect()
}

/// Every page of a read, one row per page: the ids, with the bodies kept to check what they carry.
async fn pages(w: &World, token: &str, route: &str, query: &str, key: &[&str]) -> (BTreeSet<String>, Vec<Value>) {
    let mut found = BTreeSet::new();
    let mut bodies = Vec::new();
    let mut cursor: Option<String> = None;
    for _ in 0..20 {
        let mut full = format!("/api/v1/workspaces/{}/flow/{route}?limit=1&{query}", w.workspace);
        if let Some(cursor) = &cursor {
            full = format!("{full}&cursor={}", cursor.replace('+', "%2B"));
        }
        let (status, body) = call(&w.app, "GET", &full, token, None).await;
        assert_eq!(status, 200, "{route} {query}: {body}");
        assert_eq!(body["code"], 0, "{route} {query}: {body}");
        found.extend(items_of(&body, key));
        let next = body["data"]["next_cursor"].as_str().map(str::to_owned);
        bodies.push(body);
        match next {
            Some(next) => cursor = Some(next),
            None => return (found, bodies),
        }
    }
    panic!("{route} {query} did not end");
}

fn no_leak(bodies: &[Value], others: &[&Seed], tags: &[&str]) {
    let text = bodies.iter().map(Value::to_string).collect::<String>();
    for seed in others {
        for id in ids(seed) {
            assert!(!text.contains(&id), "{id} leaked: {text}");
        }
    }
    for tag in tags {
        assert!(!text.contains(tag), "{tag} leaked: {text}");
    }
}

/// A runner bot connected to the project lists, searches and reads the trash of that project, page
/// by page, and every page carries only that project: not the other projects', not the
/// workspace's own objects, not their titles, ids, or sequence numbers.
#[tokio::test]
async fn a_runner_bot_reads_the_flow_of_its_connected_project_only() {
    let scratch = scratch_or_skip!("runner_flow_reads");
    let db = &scratch.db;
    let w = world(db, "fr1").await;
    let token = w.c.token.clone();
    let on = w.c.on;
    let others = [&w.never, &w.off, &w.none];
    let tags = ["bravo", "charlie", "delta"];

    // The object listing: the project's active objects, then with the archived ones.
    let (listed, bodies) = pages(&w, &token, "objects", &format!("project_id={on}"), &["id"]).await;
    assert_eq!(listed, w.on.active, "objects");
    no_leak(&bodies, &others, &tags);
    let (listed, bodies) = pages(
        &w,
        &token,
        "objects",
        &format!("project_id={on}&include_archived=true"),
        &["id"],
    )
    .await;
    assert_eq!(listed, ids(&w.on), "objects with the archived ones");
    no_leak(&bodies, &others, &tags);

    // The search: every hit and every page of the project, the index frontier of the project only.
    let (found, bodies) = pages(
        &w,
        &token,
        "search",
        &format!("q=needle&project_id={on}"),
        &["object", "id"],
    )
    .await;
    assert_eq!(found, w.on.active, "search");
    no_leak(&bodies, &others, &tags);
    for body in &bodies {
        assert_eq!(body["data"]["index_frontier"]["head_seq"], 1, "{body}");
        assert_eq!(body["data"]["index_frontier"]["indexed_seq"], 1, "{body}");
        assert_eq!(body["data"]["index_frontier"]["stale"], false, "{body}");
        let keys = body["data"]
            .as_object()
            .expect("data")
            .keys()
            .cloned()
            .collect::<BTreeSet<_>>();
        let allowed: BTreeSet<String> = ["index_frontier", "items", "next_cursor"]
            .into_iter()
            .map(String::from)
            .collect();
        assert!(keys.is_subset(&allowed), "no count field enters the response: {body}");
    }
    // A query that matches the other projects' titles only finds nothing, and says nothing more.
    let (status, body) = call(
        &w.app,
        "GET",
        &format!("/api/v1/workspaces/{}/flow/search?q=bravo&project_id={on}", w.workspace),
        &token,
        None,
    )
    .await;
    assert_eq!(status, 200, "{body}");
    assert!(body["data"]["items"].as_array().expect("items").is_empty(), "{body}");
    assert_eq!(
        body["data"]["index_frontier"]["head_seq"], 1,
        "the frontier is the project's own: {body}"
    );

    // The trash: the project's archived objects.
    let (trashed, bodies) = pages(&w, &token, "trash", &format!("project_id={on}"), &["id"]).await;
    assert_eq!(trashed, w.on.archived, "trash");
    no_leak(&bodies, &others, &tags);

    scratch.drop_self().await;
}

/// The request is refused unless it names a project the bot is connected to and enabled for; the
/// refusal is the one answer of the scope middleware for every way of not naming it.
#[tokio::test]
async fn a_runner_bot_is_refused_without_a_connected_project() {
    let scratch = scratch_or_skip!("runner_flow_refusals");
    let db = &scratch.db;
    let w = world(db, "fr2").await;
    let token = w.c.token.clone();
    let on = w.c.on;
    let ws = w.workspace;
    let all_foreign = [&w.never, &w.off, &w.none, &w.on];

    for route in ["objects", "search", "trash"] {
        let base = format!("/api/v1/workspaces/{ws}/flow/{route}");
        let q = if route == "search" { "q=needle&" } else { "" };
        let upper = on.to_string().to_uppercase();
        for (what, query) in [
            ("no project", String::new()),
            ("an empty project", "project_id=".to_string()),
            ("a malformed project", "project_id=nope".to_string()),
            ("a missing project", format!("project_id={}", Uuid::new_v4())),
            ("the disabled project", format!("project_id={}", w.c.off)),
            ("the never connected project", format!("project_id={}", w.c.never)),
            ("unprojected", "unprojected=true".to_string()),
            ("all visible", "all_visible=true".to_string()),
            ("an encoded key", format!("project%5Fid={on}")),
            ("another spelling of the id", format!("project_id={upper}")),
        ] {
            let (status, body) = call(&w.app, "GET", &format!("{base}?{q}{query}"), &token, None).await;
            assert!(refused(&(status, body.clone())), "{route} with {what}: {status} {body}");
        }
        // Naming a project twice: the scope middleware reads the first, the handler must not read
        // the other one; whatever answer it gives carries nothing of the other project.
        for (first, second) in [(on, w.c.never), (w.c.never, on), (on, w.c.off)] {
            let (status, body) = call(
                &w.app,
                "GET",
                &format!("{base}?{q}project_id={first}&project_id={second}"),
                &token,
                None,
            )
            .await;
            if first == on {
                assert_ne!(
                    code_of(status, &body),
                    0,
                    "{route}: two projects must not be a request: {body}"
                );
            } else {
                assert!(refused(&(status, body.clone())), "{route}: {status} {body}");
            }
            no_leak(&[body], &[&w.never, &w.off, &w.none], &[]);
        }
        let (status, body) = call(
            &w.app,
            "GET",
            &format!("{base}?{q}project_id={on}&project%5Fid={}", w.c.never),
            &token,
            None,
        )
        .await;
        assert_ne!(
            code_of(status, &body),
            0,
            "{route}: an encoded second key must not be read: {body}"
        );
        no_leak(&[body], &[&w.never, &w.off, &w.none], &[]);
        // The project of the connection with another selector at the same time is not a request.
        let (status, body) = call(
            &w.app,
            "GET",
            &format!("{base}?{q}project_id={on}&unprojected=true"),
            &token,
            None,
        )
        .await;
        if route != "trash" {
            assert_eq!(code_of(status, &body), 400, "{route}: {body}");
        }
        no_leak(&[body], &[&w.never, &w.off, &w.none], &[]);
        let _ = all_foreign;
    }
    // A search with both a project and all_visible is not an all-visible search.
    let (status, body) = call(
        &w.app,
        "GET",
        &format!("/api/v1/workspaces/{ws}/flow/search?q=needle&project_id={on}&all_visible=true"),
        &token,
        None,
    )
    .await;
    assert_eq!(code_of(status, &body), 400, "{body}");
    no_leak(&[body], &[&w.never, &w.off, &w.none], &[]);

    // The path names another workspace: the connected project is not found there.
    let other = Uuid::new_v4();
    for route in ["objects", "search", "trash"] {
        let q = if route == "search" { "q=needle&" } else { "" };
        let (status, body) = call(
            &w.app,
            "GET",
            &format!("/api/v1/workspaces/{other}/flow/{route}?{q}project_id={on}"),
            &token,
            None,
        )
        .await;
        assert_ne!(code_of(status, &body), 0, "{route}: {body}");
        no_leak(&[body], &[&w.never, &w.off, &w.none, &w.on], &[]);
    }
    scratch.drop_self().await;
}

/// An ordinary bot is not touched by the scope: without `project_id` it reads the whole workspace's
/// Flow as before, and with `project_id` it reads that project, whichever project it is.
#[tokio::test]
async fn an_ordinary_bot_reads_the_flow_as_before() {
    let scratch = scratch_or_skip!("runner_flow_plain");
    let db = &scratch.db;
    let w = world(db, "fr3").await;
    let token = w.c.plain_token.clone();
    let ws = w.workspace;
    let get = |route: &'static str, query: String| {
        let app = w.app.clone();
        let token = token.clone();
        async move {
            let (status, body) = call(
                &app,
                "GET",
                &format!("/api/v1/workspaces/{ws}/flow/{route}?{query}"),
                &token,
                None,
            )
            .await;
            assert_eq!(status, 200, "{route} {query}: {body}");
            body
        }
    };
    // Every project and the workspace's own objects, with no project named.
    let everything: BTreeSet<String> = [&w.on, &w.never, &w.off, &w.none]
        .iter()
        .flat_map(|seed| seed.active.iter().cloned())
        .collect();
    let mut found = BTreeSet::new();
    let mut cursor: Option<String> = None;
    loop {
        let mut query = "limit=100".to_string();
        if let Some(cursor) = &cursor {
            query = format!("{query}&cursor={}", cursor.replace('+', "%2B"));
        }
        let body = get("objects", query).await;
        found.extend(items_of(&body, &["id"]));
        match body["data"]["next_cursor"].as_str() {
            Some(next) => cursor = Some(next.to_string()),
            None => break,
        }
    }
    assert!(
        everything.is_subset(&found),
        "an ordinary bot lists the whole workspace"
    );
    // The same bot with a project: that project, even one a runner bot would be refused.
    let body = get("objects", format!("project_id={}&limit=100", w.c.never)).await;
    assert_eq!(items_of(&body, &["id"]), w.never.active);
    let body = get("search", format!("q=needle&project_id={}", w.c.off)).await;
    assert_eq!(items_of(&body, &["object", "id"]), w.off.active);
    let body = get("search", "q=needle&unprojected=true".to_string()).await;
    assert_eq!(items_of(&body, &["object", "id"]), w.none.active);
    let body = get("trash", format!("project_id={}", w.c.never)).await;
    assert_eq!(items_of(&body, &["id"]), w.never.archived);
    let body = get("trash", String::new()).await;
    let trashed = items_of(&body, &["id"]);
    for seed in [&w.on, &w.never, &w.off, &w.none] {
        assert!(
            seed.archived.is_subset(&trashed),
            "the workspace trash lists every project"
        );
    }
    scratch.drop_self().await;
}
