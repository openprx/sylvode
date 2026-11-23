//! Work item answers and update semantics through the real router.
//!
//! Every test drives `build_router` with user tokens against a scratch PostgreSQL database that has
//! every migration applied (skipped without `OPENPR_TEST_DATABASE_URL`).
//!
//! The cast: `alice` owns workspace `ws`, `bob` is a member of it, and `carol` belongs to another
//! workspace only. `ws` has project `QLB`.
#![allow(
    clippy::indexing_slicing,
    clippy::print_stderr,
    clippy::too_many_lines,
    clippy::similar_names,
    clippy::items_after_statements,
    clippy::doc_markdown
)]

use super::migration_runner_database_tests::{Scratch, scratch};
use super::{MigrationOptions, build_router, run_migrations_with};
use axum::Router;
use axum::body::Body;
use axum::http::{Method, Request, StatusCode, header};
use platform::{
    app::AppState,
    auth::JwtManager,
    config::{AppConfig, Secret},
};
use sea_orm::{ConnectionTrait, DatabaseConnection, DbBackend, Statement};
use serde_json::{Value, json};
use tower::ServiceExt as _;
use uuid::Uuid;

const JWT_SECRET: &str = "item-api-test-secret";

struct Api {
    scratch: Scratch,
    db: DatabaseConnection,
    router: Router,
    ws: Uuid,
    alice: Uuid,
    bob: Uuid,
    project: Uuid,
}

struct Answer {
    status: StatusCode,
    body: Value,
}

impl Answer {
    fn code(&self) -> Option<i64> {
        self.body.get("code").and_then(Value::as_i64)
    }

    fn data(&self) -> &Value {
        &self.body["data"]
    }
}

async fn exec(db: &DatabaseConnection, sql: &str, values: Vec<sea_orm::Value>) {
    db.execute(Statement::from_sql_and_values(DbBackend::Postgres, sql, values))
        .await
        .unwrap_or_else(|err| panic!("setup statement failed: {err}"));
}

impl Api {
    async fn new(label: &str) -> Option<Self> {
        let Some(scratch) = scratch(label).await else {
            eprintln!("skipped: OPENPR_TEST_DATABASE_URL is not set");
            return None;
        };
        let db = scratch.connection().clone();
        run_migrations_with(&db, MigrationOptions::default())
            .await
            .expect("migrations apply");
        let state = AppState {
            cfg: AppConfig {
                app_name: "api-test".to_string(),
                bind_addr: "127.0.0.1:0".to_string(),
                database_url: Secret::new("postgres://unused/unused"),
                jwt_secret: Secret::new(JWT_SECRET),
                jwt_access_ttl_seconds: 900,
                jwt_refresh_ttl_seconds: 3600,
                default_author_id: None,
                allow_insecure_cookies: false,
                collab_allowed_origins: Vec::new(),
            },
            db: db.clone(),
            flow_permission_cache: platform::app::FlowPermissionCacheSlot::default(),
        };
        let router = build_router(state);
        let [ws, ws_other, alice, bob, carol, project] = std::array::from_fn(|_| Uuid::new_v4());
        for (id, name) in [(alice, "alice"), (bob, "bob"), (carol, "carol")] {
            exec(
                &db,
                "INSERT INTO users (id, email, password_hash, name, role, is_active) VALUES ($1, $2, '!', $3, 'user', true)",
                vec![id.into(), format!("{name}-{id}@items.test").into(), name.to_string().into()],
            )
            .await;
        }
        for (id, slug) in [(ws, "main"), (ws_other, "other")] {
            exec(
                &db,
                "INSERT INTO workspaces (id, slug, name, created_by) VALUES ($1, $2, $3, $4)",
                vec![
                    id.into(),
                    format!("{slug}-{id}").into(),
                    format!("ws {slug}").into(),
                    alice.into(),
                ],
            )
            .await;
        }
        for (workspace, user, role) in [(ws, alice, "owner"), (ws, bob, "member"), (ws_other, carol, "owner")] {
            exec(
                &db,
                "INSERT INTO workspace_members (workspace_id, user_id, role) VALUES ($1, $2, $3)",
                vec![workspace.into(), user.into(), role.into()],
            )
            .await;
        }
        exec(
            &db,
            "INSERT INTO projects (id, workspace_id, key, name, created_by, type_key) \
             VALUES ($1, $2, 'QLB', 'qa loop', $3, 'code_project')",
            vec![project.into(), ws.into(), alice.into()],
        )
        .await;
        Some(Self {
            scratch,
            db,
            router,
            ws,
            alice,
            bob,
            project,
        })
    }

    async fn finish(self) {
        self.scratch.drop_self().await;
    }

    fn token(user: Uuid) -> String {
        JwtManager::new(JWT_SECRET, 900, 3600)
            .issue_access_token(&user.to_string(), &format!("{user}@items.test"))
            .expect("the token is issued")
    }

    async fn call(&self, method: Method, uri: &str, user: Uuid, body: Option<Value>) -> Answer {
        let mut request = Request::builder()
            .method(method)
            .uri(uri)
            .header(header::AUTHORIZATION, format!("Bearer {}", Self::token(user)));
        if body.is_some() {
            request = request.header(header::CONTENT_TYPE, "application/json");
        }
        let request = request
            .body(body.map_or_else(Body::empty, |value| Body::from(value.to_string())))
            .expect("the request is well formed");
        let response = self.router.clone().oneshot(request).await.expect("the router answers");
        let status = response.status();
        let bytes = axum::body::to_bytes(response.into_body(), 8 * 1024 * 1024)
            .await
            .expect("the body is readable");
        Answer {
            status,
            body: serde_json::from_slice(&bytes).unwrap_or(Value::Null),
        }
    }

    async fn get(&self, uri: &str, user: Uuid) -> Answer {
        self.call(Method::GET, uri, user, None).await
    }

    async fn post(&self, uri: &str, user: Uuid, body: Value) -> Answer {
        self.call(Method::POST, uri, user, Some(body)).await
    }

    async fn put(&self, uri: &str, user: Uuid, body: Value) -> Answer {
        self.call(Method::PUT, uri, user, Some(body)).await
    }

    /// Creates an item as alice and returns its id.
    async fn create_item(&self, title: &str, extra: Value) -> Uuid {
        let mut body = json!({"title": title});
        if let (Some(target), Some(source)) = (body.as_object_mut(), extra.as_object()) {
            for (key, value) in source {
                target.insert(key.clone(), value.clone());
            }
        }
        let answer = self
            .post(&format!("/api/v1/projects/{}/issues", self.project), self.alice, body)
            .await;
        assert_eq!(answer.code(), Some(0), "create failed: {}", answer.body);
        Uuid::parse_str(answer.data()["id"].as_str().expect("id")).expect("uuid")
    }

    async fn create_label(&self, name: &str) -> Uuid {
        let id = Uuid::new_v4();
        exec(
            &self.db,
            "INSERT INTO labels (id, workspace_id, name, color) VALUES ($1, $2, $3, '#112233')",
            vec![id.into(), self.ws.into(), name.to_string().into()],
        )
        .await;
        id
    }
}

fn assert_identifier_and_labels_array(item: &Value, identifier: &str, context: &str) {
    assert_eq!(item["identifier"], json!(identifier), "{context}: {item}");
    assert!(item["labels"].is_array(), "{context}: labels is not an array: {item}");
}

#[tokio::test]
async fn every_item_answer_carries_identifier_and_labels_array() {
    let Some(api) = Api::new("item_view_everywhere").await else {
        return;
    };
    // create
    let created = api
        .post(
            &format!("/api/v1/projects/{}/issues", api.project),
            api.alice,
            json!({"title": "first", "assignee_id": api.alice}),
        )
        .await;
    assert_eq!(created.status, StatusCode::OK);
    assert_eq!(created.code(), Some(0), "{}", created.body);
    assert_identifier_and_labels_array(created.data(), "QLB-1", "create");
    assert_eq!(created.data()["labels"], json!([]));
    let first = Uuid::parse_str(created.data()["id"].as_str().expect("id")).expect("uuid");
    let second = api.create_item("second", json!({})).await;

    // get
    let got = api.get(&format!("/api/v1/issues/{first}"), api.bob).await;
    assert_identifier_and_labels_array(got.data(), "QLB-1", "get");
    assert_eq!(got.data()["labels"], json!([]));

    // two labels attached through the batch route come back sorted by name
    let (zeta, alpha) = (api.create_label("zeta").await, api.create_label("alpha").await);
    let attached = api
        .post(
            &format!("/api/v1/issues/{first}/labels/batch"),
            api.bob,
            json!({"label_ids": [zeta, alpha]}),
        )
        .await;
    assert_eq!(attached.code(), Some(0), "{}", attached.body);
    let got = api.get(&format!("/api/v1/issues/{first}"), api.alice).await;
    let names: Vec<&str> = got.data()["labels"]
        .as_array()
        .expect("labels")
        .iter()
        .filter_map(|label| label["name"].as_str())
        .collect();
    assert_eq!(names, ["alpha", "zeta"], "{}", got.body);

    // update: answered from the transaction that wrote it, labels included
    let updated = api
        .put(
            &format!("/api/v1/issues/{first}"),
            api.alice,
            json!({"title": "first, renamed"}),
        )
        .await;
    assert_eq!(updated.code(), Some(0), "{}", updated.body);
    assert_identifier_and_labels_array(updated.data(), "QLB-1", "update");
    assert_eq!(updated.data()["title"], json!("first, renamed"));
    assert_eq!(updated.data()["labels"].as_array().map(Vec::len), Some(2));

    // list
    let listed = api
        .get(&format!("/api/v1/projects/{}/issues?per_page=50", api.project), api.bob)
        .await;
    let items = listed.data()["items"].as_array().expect("items");
    assert_eq!(items.len(), 2, "{}", listed.body);
    for item in items {
        let expected = if item["id"] == json!(first) { "QLB-1" } else { "QLB-2" };
        assert_identifier_and_labels_array(item, expected, "list");
    }
    let without_labels = items.iter().find(|item| item["id"] == json!(second)).expect("second");
    assert_eq!(
        without_labels["labels"],
        json!([]),
        "an item without labels has an empty array"
    );

    // board
    let board = api
        .get(&format!("/api/v1/projects/{}/board", api.project), api.bob)
        .await;
    let board_items: Vec<&Value> = board.data()["columns"]
        .as_array()
        .expect("columns")
        .iter()
        .flat_map(|column| column["issues"].as_array().expect("issues").iter())
        .collect();
    assert_eq!(board_items.len(), 2, "{}", board.body);
    for item in board_items {
        let expected = if item["id"] == json!(first) { "QLB-1" } else { "QLB-2" };
        assert_eq!(item["identifier"], json!(expected), "board: {item}");
    }

    // my issues (the first item is assigned to alice)
    let mine = api.get("/api/v1/my/issues", api.alice).await;
    let mine_items = mine.data()["items"].as_array().expect("items");
    assert_eq!(mine_items.len(), 1, "{}", mine.body);
    assert_eq!(mine_items[0]["identifier"], json!("QLB-1"), "my issues: {}", mine.body);

    // search
    let found = api.get("/api/v1/search?q=second&type=issue", api.bob).await;
    let results = found.data()["results"].as_array().expect("results");
    assert_eq!(results.len(), 1, "{}", found.body);
    assert_eq!(results[0]["identifier"], json!("QLB-2"), "search: {}", found.body);

    // by identifier
    let by_identifier = api.get("/api/v1/issues/by-identifier/qlb-2", api.bob).await;
    assert_identifier_and_labels_array(by_identifier.data(), "QLB-2", "by-identifier");
    assert_eq!(by_identifier.data()["id"], json!(second));

    api.finish().await;
}

impl Api {
    async fn create_sprint(&self, name: &str) -> Uuid {
        let id = Uuid::new_v4();
        exec(
            &self.db,
            "INSERT INTO sprints (id, project_id, name) VALUES ($1, $2, $3)",
            vec![id.into(), self.project.into(), name.to_string().into()],
        )
        .await;
        id
    }

    /// The `detail` of every activity of `action` on `issue`, oldest first.
    async fn activity_details(&self, issue: Uuid, action: &str) -> Vec<Value> {
        let rows = self
            .db
            .query_all(Statement::from_sql_and_values(
                DbBackend::Postgres,
                "SELECT detail::text AS detail FROM activities WHERE issue_id = $1 AND action = $2 ORDER BY created_at, id",
                vec![issue.into(), action.to_string().into()],
            ))
            .await
            .expect("activities are readable");
        rows.iter()
            .map(|row| {
                let text: String = row.try_get("", "detail").expect("detail column");
                serde_json::from_str(&text).expect("detail is json")
            })
            .collect()
    }

    /// The stored assignee, sprint and due date of `issue`; `true` means the column is NULL.
    async fn stored_nulls(&self, issue: Uuid) -> (bool, bool, bool) {
        let row = self
            .db
            .query_one(Statement::from_sql_and_values(
                DbBackend::Postgres,
                "SELECT assignee_id IS NULL AS a, sprint_id IS NULL AS s, due_at IS NULL AS d FROM work_items WHERE id = $1",
                vec![issue.into()],
            ))
            .await
            .expect("row is readable")
            .expect("the item exists");
        (
            row.try_get("", "a").expect("a"),
            row.try_get("", "s").expect("s"),
            row.try_get("", "d").expect("d"),
        )
    }
}

const DUE: &str = "2030-01-02T03:04:05Z";

#[tokio::test]
async fn update_with_null_clears_assignee_sprint_and_due_date() {
    let Some(api) = Api::new("item_update_null").await else {
        return;
    };
    let sprint = api.create_sprint("s1").await;
    let full = json!({"assignee_id": api.bob, "sprint_id": sprint, "due_at": DUE});

    // Each field alone.
    for (field, nulls_after) in [
        ("assignee_id", (true, false, false)),
        ("sprint_id", (false, true, false)),
        ("due_at", (false, false, true)),
    ] {
        let item = api.create_item(&format!("clear {field}"), full.clone()).await;
        assert_eq!(api.stored_nulls(item).await, (false, false, false), "{field}: seeded");
        let answer = api
            .put(&format!("/api/v1/issues/{item}"), api.alice, json!({ field: null }))
            .await;
        assert_eq!(answer.code(), Some(0), "{field}: {}", answer.body);
        assert!(answer.data()[field].is_null(), "{field}: {}", answer.body);
        assert_eq!(api.stored_nulls(item).await, nulls_after, "{field}: stored");
    }

    // All three together, recorded in the activity log with `to: null`.
    let item = api.create_item("clear all", full).await;
    let answer = api
        .put(
            &format!("/api/v1/issues/{item}"),
            api.alice,
            json!({"assignee_id": null, "sprint_id": null, "due_at": null}),
        )
        .await;
    assert_eq!(answer.code(), Some(0), "{}", answer.body);
    assert_eq!(api.stored_nulls(item).await, (true, true, true));
    let assigned = api.activity_details(item, "assigned").await;
    assert_eq!(assigned.len(), 1, "{assigned:?}");
    assert_eq!(assigned[0]["from"], json!(api.bob));
    assert!(assigned[0]["to"].is_null(), "{assigned:?}");
    let sprint_changed = api.activity_details(item, "sprint_changed").await;
    assert_eq!(sprint_changed.len(), 1, "{sprint_changed:?}");
    assert_eq!(sprint_changed[0]["from"], json!(sprint));
    assert!(sprint_changed[0]["to"].is_null(), "{sprint_changed:?}");

    // Clearing what is already clear is accepted and records nothing more.
    let again = api
        .put(
            &format!("/api/v1/issues/{item}"),
            api.alice,
            json!({"assignee_id": null}),
        )
        .await;
    assert_eq!(again.code(), Some(0), "{}", again.body);
    assert_eq!(api.activity_details(item, "assigned").await.len(), 1);
    api.finish().await;
}

#[tokio::test]
async fn update_with_absent_fields_changes_nothing_else() {
    let Some(api) = Api::new("item_update_absent").await else {
        return;
    };
    let sprint = api.create_sprint("s1").await;
    let item = api
        .create_item(
            "keep",
            json!({"description": "body", "priority": "high", "assignee_id": api.bob, "sprint_id": sprint, "due_at": DUE}),
        )
        .await;
    let before = api.get(&format!("/api/v1/issues/{item}"), api.alice).await;
    let answer = api
        .put(
            &format!("/api/v1/issues/{item}"),
            api.alice,
            json!({"title": "kept, renamed"}),
        )
        .await;
    assert_eq!(answer.code(), Some(0), "{}", answer.body);
    let after = api.get(&format!("/api/v1/issues/{item}"), api.alice).await;
    for field in ["description", "priority", "state", "assignee_id", "sprint_id", "due_at"] {
        assert_eq!(after.data()[field], before.data()[field], "{field} changed");
    }
    assert_eq!(after.data()["title"], json!("kept, renamed"));
    assert!(api.activity_details(item, "assigned").await.is_empty());
    assert!(api.activity_details(item, "sprint_changed").await.is_empty());
    api.finish().await;
}

#[tokio::test]
async fn update_with_null_on_a_field_that_cannot_be_cleared_is_400() {
    let Some(api) = Api::new("item_update_null_title").await else {
        return;
    };
    let item = api.create_item("stay", json!({})).await;
    for field in ["title", "description", "state", "priority"] {
        let answer = api
            .put(&format!("/api/v1/issues/{item}"), api.alice, json!({ field: null }))
            .await;
        assert_eq!(answer.code(), Some(400), "{field}: {}", answer.body);
        assert_eq!(
            answer.body["message"],
            json!(format!("{field} cannot be null")),
            "{field}: {}",
            answer.body
        );
    }
    let still = api.get(&format!("/api/v1/issues/{item}"), api.alice).await;
    assert_eq!(still.data()["title"], json!("stay"));
    api.finish().await;
}

#[tokio::test]
async fn update_and_create_with_an_unknown_field_are_refused() {
    let Some(api) = Api::new("item_unknown_field").await else {
        return;
    };
    let item = api.create_item("typed", json!({})).await;
    let refused = api
        .put(&format!("/api/v1/issues/{item}"), api.alice, json!({"goal": "x"}))
        .await;
    assert_eq!(refused.status, StatusCode::UNPROCESSABLE_ENTITY, "{}", refused.body);
    // A known field next to an unknown one changes nothing.
    let refused = api
        .put(
            &format!("/api/v1/issues/{item}"),
            api.alice,
            json!({"title": "never written", "stat": "todo"}),
        )
        .await;
    assert!(!refused.status.is_success(), "{} {}", refused.status, refused.body);
    let still = api.get(&format!("/api/v1/issues/{item}"), api.alice).await;
    assert_eq!(still.data()["title"], json!("typed"));
    let refused = api
        .post(
            &format!("/api/v1/projects/{}/issues", api.project),
            api.alice,
            json!({"title": "x", "status": "todo"}),
        )
        .await;
    assert!(!refused.status.is_success(), "{} {}", refused.status, refused.body);
    api.finish().await;
}

#[tokio::test]
async fn list_filters_by_sprint_and_by_none() {
    let Some(api) = Api::new("item_list_sprint").await else {
        return;
    };
    let (s1, s2) = (api.create_sprint("s1").await, api.create_sprint("s2").await);
    let in_s1 = api.create_item("in s1", json!({"sprint_id": s1})).await;
    let in_s2 = api.create_item("in s2", json!({"sprint_id": s2})).await;
    let backlog = api.create_item("backlog", json!({})).await;
    let ids_for = |filter: String| {
        let uri = format!("/api/v1/projects/{}/issues?{filter}", api.project);
        let api = &api;
        async move {
            let answer = api.get(&uri, api.bob).await;
            assert_eq!(answer.code(), Some(0), "{uri}: {}", answer.body);
            let mut ids: Vec<String> = answer.data()["items"]
                .as_array()
                .expect("items")
                .iter()
                .map(|item| item["id"].as_str().expect("id").to_string())
                .collect();
            ids.sort();
            ids
        }
    };
    assert_eq!(ids_for(format!("sprint_id={s1}")).await, [in_s1.to_string()]);
    assert_eq!(ids_for(format!("sprint_id={s2}")).await, [in_s2.to_string()]);
    assert_eq!(ids_for("sprint_id=none".to_string()).await, [backlog.to_string()]);
    let mut all = vec![in_s1.to_string(), in_s2.to_string(), backlog.to_string()];
    all.sort();
    assert_eq!(ids_for(String::new()).await, all);
    // A sprint filter combines with the others.
    let state = api.get(&format!("/api/v1/issues/{in_s1}"), api.bob).await.data()["state"]
        .as_str()
        .expect("state")
        .to_string();
    assert_eq!(
        ids_for(format!("sprint_id={s1}&state={state}&priority=medium")).await,
        [in_s1.to_string()]
    );
    api.finish().await;
}

#[tokio::test]
async fn list_with_an_unknown_filter_or_a_bad_sprint_id_is_refused() {
    let Some(api) = Api::new("item_list_refused").await else {
        return;
    };
    api.create_item("one", json!({})).await;
    let base = format!("/api/v1/projects/{}/issues", api.project);
    let unknown = api.get(&format!("{base}?foo=1"), api.bob).await;
    assert_eq!(unknown.status, StatusCode::BAD_REQUEST, "{}", unknown.body);
    let bogus = api.get(&format!("{base}?sprint_id=bogus"), api.bob).await;
    assert_eq!(bogus.code(), Some(400), "{}", bogus.body);
    assert_eq!(bogus.body["message"], json!("sprint_id must be a UUID or none"));
    api.finish().await;
}

const ALL_ISSUE_EVENTS: &str = r#"["issue.created","issue.updated","issue.assigned","issue.state_changed","issue.deleted","label.added","label.removed","comment.created","comment.deleted"]"#;

impl Api {
    /// A member of the main workspace of its own.
    async fn add_member(&self, name: &str, entity_type: &str) -> Uuid {
        let id = Uuid::new_v4();
        exec(
            &self.db,
            "INSERT INTO users (id, email, password_hash, name, role, is_active, entity_type) \
             VALUES ($1, $2, '!', $3, 'user', true, $4)",
            vec![
                id.into(),
                format!("{name}-{id}@items.test").into(),
                name.to_string().into(),
                entity_type.to_string().into(),
            ],
        )
        .await;
        exec(
            &self.db,
            "INSERT INTO workspace_members (workspace_id, user_id, role) VALUES ($1, $2, 'member')",
            vec![self.ws.into(), id.into()],
        )
        .await;
        id
    }

    /// Subscribes a receiver to the issue, label and comment events of the main workspace. The
    /// address is never reached (loopback is refused by the outbound policy); the delivery log
    /// keeps the payload that would have been sent.
    async fn add_webhook(&self) {
        exec(
            &self.db,
            "INSERT INTO webhooks (id, workspace_id, name, url, secret, events, active, created_by) \
             VALUES ($1, $2, 'receiver', 'http://127.0.0.1:9/hook', 'test-secret', $3::jsonb, true, $4)",
            vec![
                Uuid::new_v4().into(),
                self.ws.into(),
                ALL_ISSUE_EVENTS.into(),
                self.alice.into(),
            ],
        )
        .await;
    }

    /// The payloads logged for `event`, once at least `expected` are there (deliveries are sent
    /// from spawned tasks) and a moment longer so an extra one would be counted.
    async fn payloads(&self, event: &str, expected: usize) -> Vec<Value> {
        let read = || async {
            let rows = self
                .db
                .query_all(Statement::from_sql_and_values(
                    DbBackend::Postgres,
                    "SELECT d.payload::text AS payload FROM webhook_deliveries d \
                     JOIN webhooks w ON w.id = d.webhook_id \
                     WHERE w.workspace_id = $1 AND d.event_type = $2 ORDER BY d.created_at, d.id",
                    vec![self.ws.into(), event.to_string().into()],
                ))
                .await
                .expect("deliveries are readable");
            rows.iter()
                .map(|row| {
                    let text: String = row.try_get("", "payload").expect("payload column");
                    serde_json::from_str::<Value>(&text).expect("payload is json")
                })
                .collect::<Vec<_>>()
        };
        for _ in 0..200 {
            if read().await.len() >= expected {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        }
        tokio::time::sleep(std::time::Duration::from_millis(300)).await;
        let found = read().await;
        assert_eq!(found.len(), expected, "{event} deliveries: {found:?}");
        found
    }

    async fn notifications_of(&self, user: Uuid) -> i64 {
        self.db
            .query_one(Statement::from_sql_and_values(
                DbBackend::Postgres,
                "SELECT COUNT(*)::bigint AS n FROM notifications WHERE user_id = $1 AND kind = 'assignment'",
                vec![user.into()],
            ))
            .await
            .expect("count")
            .expect("row")
            .try_get("", "n")
            .expect("n")
    }
}

#[tokio::test]
async fn create_with_human_assignee_notifies_once() {
    let Some(api) = Api::new("item_notify_create").await else {
        return;
    };
    let item = api
        .create_item("assigned at creation", json!({"assignee_id": api.bob}))
        .await;
    assert_eq!(api.notifications_of(api.bob).await, 1);
    assert_eq!(api.notifications_of(api.alice).await, 0);
    let related: Uuid = api
        .db
        .query_one(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT related_issue_id FROM notifications WHERE user_id = $1",
            vec![api.bob.into()],
        ))
        .await
        .expect("query")
        .expect("row")
        .try_get("", "related_issue_id")
        .expect("related");
    assert_eq!(related, item);
    api.finish().await;
}

#[tokio::test]
async fn create_assigned_to_self_notifies_nobody() {
    let Some(api) = Api::new("item_notify_self_create").await else {
        return;
    };
    api.create_item("mine", json!({"assignee_id": api.alice})).await;
    assert_eq!(api.notifications_of(api.alice).await, 0);
    assert_eq!(api.notifications_of(api.bob).await, 0);
    api.finish().await;
}

#[tokio::test]
async fn update_self_assignment_notifies_nobody() {
    let Some(api) = Api::new("item_notify_self_update").await else {
        return;
    };
    let item = api.create_item("mine later", json!({})).await;
    let answer = api
        .put(
            &format!("/api/v1/issues/{item}"),
            api.alice,
            json!({"assignee_id": api.alice}),
        )
        .await;
    assert_eq!(answer.code(), Some(0), "{}", answer.body);
    assert_eq!(api.notifications_of(api.alice).await, 0);
    api.finish().await;
}

#[tokio::test]
async fn assigning_a_bot_writes_no_notification() {
    let Some(api) = Api::new("item_notify_bot").await else {
        return;
    };
    let bot = api.add_member("helper", "bot").await;
    api.create_item("for the bot at creation", json!({"assignee_id": bot}))
        .await;
    let item = api.create_item("for the bot later", json!({})).await;
    let answer = api
        .put(
            &format!("/api/v1/issues/{item}"),
            api.alice,
            json!({"assignee_id": bot}),
        )
        .await;
    assert_eq!(answer.code(), Some(0), "{}", answer.body);
    assert_eq!(api.notifications_of(bot).await, 0);
    api.finish().await;
}

#[tokio::test]
async fn reassignment_notifies_the_new_assignee_only() {
    let Some(api) = Api::new("item_notify_reassign").await else {
        return;
    };
    let dana = api.add_member("dana", "human").await;
    let item = api.create_item("passed on", json!({"assignee_id": api.bob})).await;
    assert_eq!(
        (api.notifications_of(api.bob).await, api.notifications_of(dana).await),
        (1, 0)
    );
    let answer = api
        .put(
            &format!("/api/v1/issues/{item}"),
            api.alice,
            json!({"assignee_id": dana}),
        )
        .await;
    assert_eq!(answer.code(), Some(0), "{}", answer.body);
    assert_eq!(
        (api.notifications_of(api.bob).await, api.notifications_of(dana).await),
        (1, 1)
    );
    // The same assignee again changes nothing and notifies nobody.
    api.put(
        &format!("/api/v1/issues/{item}"),
        api.alice,
        json!({"assignee_id": dana}),
    )
    .await;
    assert_eq!(api.notifications_of(dana).await, 1);
    api.finish().await;
}

#[tokio::test]
async fn issue_events_carry_the_identifier_key() {
    let Some(api) = Api::new("item_event_key").await else {
        return;
    };
    api.add_webhook().await;
    let item = api.create_item("keyed", json!({})).await;
    let label = api.create_label("tag").await;
    let added = api
        .post(&format!("/api/v1/issues/{item}/labels/{label}"), api.alice, json!({}))
        .await;
    assert_eq!(added.code(), Some(0), "{}", added.body);
    let commented = api
        .post(
            &format!("/api/v1/issues/{item}/comments"),
            api.alice,
            json!({"body": "hello"}),
        )
        .await;
    assert_eq!(commented.code(), Some(0), "{}", commented.body);
    let state = api
        .put(
            &format!("/api/v1/issues/{item}"),
            api.alice,
            json!({"priority": "high"}),
        )
        .await;
    assert_eq!(state.code(), Some(0), "{}", state.body);

    let created = api.payloads("issue.created", 1).await;
    assert_eq!(created[0]["data"]["issue"]["key"], json!("QLB-1"), "{}", created[0]);
    let updated = api.payloads("issue.updated", 1).await;
    assert_eq!(updated[0]["data"]["issue"]["key"], json!("QLB-1"), "{}", updated[0]);
    let label_added = api.payloads("label.added", 1).await;
    assert_eq!(
        label_added[0]["data"]["issue"]["key"],
        json!("QLB-1"),
        "{}",
        label_added[0]
    );
    let comment_created = api.payloads("comment.created", 1).await;
    assert_eq!(
        comment_created[0]["data"]["issue"]["key"],
        json!("QLB-1"),
        "{}",
        comment_created[0]
    );
    api.finish().await;
}

#[tokio::test]
async fn sprint_and_due_date_changes_emit_issue_updated() {
    let Some(api) = Api::new("item_event_sprint_due").await else {
        return;
    };
    api.add_webhook().await;
    let sprint = api.create_sprint("s1").await;
    let item = api.create_item("moving", json!({})).await;
    api.payloads("issue.created", 1).await;
    let moved = api
        .put(
            &format!("/api/v1/issues/{item}"),
            api.alice,
            json!({"sprint_id": sprint}),
        )
        .await;
    assert_eq!(moved.code(), Some(0), "{}", moved.body);
    // Deliveries are sent from spawned tasks: each is awaited before the next change, so the log
    // order is the order of the changes.
    api.payloads("issue.updated", 1).await;
    let dated = api
        .put(&format!("/api/v1/issues/{item}"), api.alice, json!({"due_at": DUE}))
        .await;
    assert_eq!(dated.code(), Some(0), "{}", dated.body);
    api.payloads("issue.updated", 2).await;
    let cleared = api
        .put(
            &format!("/api/v1/issues/{item}"),
            api.alice,
            json!({"sprint_id": null, "due_at": null}),
        )
        .await;
    assert_eq!(cleared.code(), Some(0), "{}", cleared.body);

    let updated = api.payloads("issue.updated", 3).await;
    let changes = |index: usize| &updated[index]["data"]["changes"];
    assert_eq!(
        changes(0)["sprint_id"],
        json!({"old": null, "new": sprint}),
        "{}",
        updated[0]
    );
    assert!(changes(0).get("due_at").is_none());
    assert_eq!(changes(1)["due_at"]["old"], Value::Null, "{}", updated[1]);
    assert!(
        changes(1)["due_at"]["new"]
            .as_str()
            .is_some_and(|text| text.starts_with("2030-01-02T03:04:05")),
        "{}",
        updated[1]
    );
    assert_eq!(
        changes(2)["sprint_id"],
        json!({"old": sprint, "new": null}),
        "{}",
        updated[2]
    );
    assert_eq!(changes(2)["due_at"]["new"], Value::Null, "{}", updated[2]);
    // The issue object carries the due date too.
    assert!(updated[1]["data"]["issue"]["due_at"].is_string(), "{}", updated[1]);
    api.finish().await;
}

#[tokio::test]
async fn create_with_human_assignee_emits_issue_assigned() {
    let Some(api) = Api::new("item_event_assigned_create").await else {
        return;
    };
    api.add_webhook().await;
    api.create_item("assigned at creation", json!({"assignee_id": api.bob}))
        .await;
    let assigned = api.payloads("issue.assigned", 1).await;
    let assignee = json!([api.bob.to_string()]);
    assert_eq!(
        assigned[0]["data"]["changes"]["assignee_ids"]["new"], assignee,
        "{}",
        assigned[0]
    );
    assert_eq!(assigned[0]["data"]["changes"]["assignee_ids"]["added"], assignee);
    assert_eq!(assigned[0]["data"]["issue"]["assignee_ids"], assignee);
    api.finish().await;
}

#[tokio::test]
async fn unassign_emits_issue_assigned_with_empty_new() {
    let Some(api) = Api::new("item_event_unassign").await else {
        return;
    };
    api.add_webhook().await;
    let item = api.create_item("to unassign", json!({"assignee_id": api.bob})).await;
    api.payloads("issue.assigned", 1).await;
    let answer = api
        .put(
            &format!("/api/v1/issues/{item}"),
            api.alice,
            json!({"assignee_id": null}),
        )
        .await;
    assert_eq!(answer.code(), Some(0), "{}", answer.body);
    let assigned = api.payloads("issue.assigned", 2).await;
    let changes = &assigned[1]["data"]["changes"]["assignee_ids"];
    assert_eq!(changes["new"], json!([]), "{}", assigned[1]);
    assert_eq!(changes["removed"], json!([api.bob.to_string()]), "{}", assigned[1]);
    assert_eq!(assigned[1]["data"]["issue"]["assignee_ids"], json!([]));
    // Nobody is told, and no notification is written, for an unassignment.
    assert_eq!(api.notifications_of(api.bob).await, 1);
    api.finish().await;
}

#[tokio::test]
async fn issue_deleted_payload_lists_labels_and_identifier() {
    let Some(api) = Api::new("item_event_deleted").await else {
        return;
    };
    api.add_webhook().await;
    let item = api.create_item("short lived", json!({})).await;
    let label = api.create_label("tag").await;
    let added = api
        .post(&format!("/api/v1/issues/{item}/labels/{label}"), api.alice, json!({}))
        .await;
    assert_eq!(added.code(), Some(0), "{}", added.body);
    let deleted = api
        .call(Method::DELETE, &format!("/api/v1/issues/{item}"), api.alice, None)
        .await;
    assert_eq!(deleted.code(), Some(0), "{}", deleted.body);
    let payloads = api.payloads("issue.deleted", 1).await;
    let issue = &payloads[0]["data"]["issue"];
    assert_eq!(issue["key"], json!("QLB-1"), "{}", payloads[0]);
    assert_eq!(issue["label_ids"], json!([label.to_string()]), "{}", payloads[0]);
    assert_eq!(issue["title"], json!("short lived"));
    api.finish().await;
}

#[tokio::test]
async fn comment_deleted_payload_keeps_author_and_original_timestamps() {
    let Some(api) = Api::new("item_event_comment_deleted").await else {
        return;
    };
    api.add_webhook().await;
    let item = api.create_item("discussed", json!({})).await;
    let created = api
        .post(
            &format!("/api/v1/issues/{item}/comments"),
            api.bob,
            json!({"body": "to be deleted"}),
        )
        .await;
    assert_eq!(created.code(), Some(0), "{}", created.body);
    let comment = created.data()["id"].as_str().expect("comment id").to_string();
    // Deleted by the workspace owner, so the author is not the actor.
    tokio::time::sleep(std::time::Duration::from_millis(1100)).await;
    let deleted = api
        .call(Method::DELETE, &format!("/api/v1/comments/{comment}"), api.alice, None)
        .await;
    assert_eq!(deleted.code(), Some(0), "{}", deleted.body);

    let payloads = api.payloads("comment.deleted", 1).await;
    let data = &payloads[0]["data"];
    assert_eq!(data["comment"]["author_name"], json!("bob"), "{}", payloads[0]);
    assert_eq!(data["comment"]["author_id"], json!(api.bob), "{}", payloads[0]);
    assert_eq!(data["comment"]["content"], json!("to be deleted"));
    assert_eq!(
        data["comment"]["created_at"],
        created.data()["created_at"],
        "{}",
        payloads[0]
    );
    assert_eq!(
        data["comment"]["updated_at"],
        created.data()["updated_at"],
        "{}",
        payloads[0]
    );
    assert_eq!(data["issue"]["key"], json!("QLB-1"), "{}", payloads[0]);
    assert_eq!(data["issue"]["title"], json!("discussed"));
    assert_eq!(data["issue"]["id"], json!(item));
    api.finish().await;
}

#[tokio::test]
async fn mention_notification_links_the_comment() {
    let Some(api) = Api::new("item_mention_comment").await else {
        return;
    };
    let item = api.create_item("mentioned in", json!({})).await;
    let created = api
        .post(
            &format!("/api/v1/issues/{item}/comments"),
            api.alice,
            json!({"body": "look @bob", "mentions": [api.bob]}),
        )
        .await;
    assert_eq!(created.code(), Some(0), "{}", created.body);
    let comment = created.data()["id"].as_str().expect("comment id").to_string();
    let linked = |kind: &'static str| {
        let (db, bob) = (api.db.clone(), api.bob);
        async move {
            db.query_all(Statement::from_sql_and_values(
                DbBackend::Postgres,
                "SELECT related_comment_id::text AS c FROM notifications WHERE user_id = $1 AND kind = $2",
                vec![bob.into(), kind.into()],
            ))
            .await
            .expect("notifications are readable")
            .iter()
            .map(|row| row.try_get::<Option<String>>("", "c").expect("c"))
            .collect::<Vec<_>>()
        }
    };
    assert_eq!(linked("mentioned").await, [Some(comment.clone())]);

    // An edit that mentions someone new links the edited comment too.
    let dana = api.add_member("dana", "human").await;
    let edited = api
        .put(
            &format!("/api/v1/comments/{comment}"),
            api.alice,
            json!({"body": "look @dana", "mentions": [dana]}),
        )
        .await;
    assert_eq!(edited.code(), Some(0), "{}", edited.body);
    let for_dana = api
        .db
        .query_all(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT related_comment_id::text AS c FROM notifications WHERE user_id = $1 AND kind = 'mentioned'",
            vec![dana.into()],
        ))
        .await
        .expect("notifications are readable");
    assert_eq!(for_dana.len(), 1);
    assert_eq!(
        for_dana[0].try_get::<Option<String>>("", "c").expect("c"),
        Some(comment)
    );
    api.finish().await;
}
