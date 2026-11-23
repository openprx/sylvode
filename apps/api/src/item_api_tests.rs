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
