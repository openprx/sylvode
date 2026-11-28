//! Instance-administrator user creation (agent types, no `agent_config`) and the first-run
//! `setup-status` answer, through the real router against a scratch `PostgreSQL` (skipped without
//! `OPENPR_TEST_DATABASE_URL`).
#![allow(
    clippy::indexing_slicing,
    clippy::too_many_lines,
    clippy::doc_markdown,
    clippy::print_stderr
)]

use super::item_api_tests::{Api, exec};
use axum::http::StatusCode;
use serde_json::json;

impl Api {
    async fn column_exists(&self, table: &str, column: &str) -> bool {
        !self
            .db_strings(&format!(
                "SELECT column_name::text AS v FROM information_schema.columns \
                 WHERE table_schema = current_schema() AND table_name = '{table}' AND column_name = '{column}'"
            ))
            .await
            .is_empty()
    }

    /// Makes alice an instance administrator.
    async fn alice_is_instance_admin(&self) {
        exec(
            &self.db,
            "UPDATE users SET role = 'admin' WHERE id = $1",
            vec![self.alice.into()],
        )
        .await;
    }
}

#[tokio::test]
async fn every_agent_type_can_be_created_and_updated() {
    let Some(api) = Api::new("admin_agent_types").await else {
        return;
    };
    api.alice_is_instance_admin().await;
    for agent_type in ["runner", "openclaw", "openprx", "webhook", "custom"] {
        let created = api
            .post(
                "/api/v1/admin/users",
                api.alice,
                json!({
                    "name": format!("bot {agent_type}"),
                    "email": format!("{agent_type}@bots.test"),
                    "entity_type": "bot",
                    "agent_type": agent_type,
                }),
            )
            .await;
        assert_eq!(created.code(), Some(0), "{agent_type}: {}", created.body);
        assert_eq!(created.data()["agent_type"], json!(agent_type));
        assert!(created.data().get("agent_config").is_none(), "{}", created.body);
        let id = created.data()["id"].as_str().expect("id").to_string();
        let shown = api.get(&format!("/api/v1/admin/users/{id}"), api.alice).await;
        assert_eq!(shown.data()["user"]["agent_type"], json!(agent_type), "{}", shown.body);
        assert!(shown.data()["user"].get("agent_config").is_none(), "{}", shown.body);
        let changed = api
            .put(
                &format!("/api/v1/admin/users/{id}"),
                api.alice,
                json!({"agent_type": "custom"}),
            )
            .await;
        assert_eq!(changed.code(), Some(0), "{}", changed.body);
        assert_eq!(changed.data()["agent_type"], json!("custom"));
    }
    let listed = api.get("/api/v1/admin/users?entity_type=bot", api.alice).await;
    assert_eq!(
        listed.data()["items"].as_array().expect("items").len(),
        5,
        "{}",
        listed.body
    );
    api.finish().await;
}

#[tokio::test]
async fn an_unknown_agent_type_is_refused_with_the_list() {
    let Some(api) = Api::new("admin_unknown_agent_type").await else {
        return;
    };
    api.alice_is_instance_admin().await;
    let refused = api
        .post(
            "/api/v1/admin/users",
            api.alice,
            json!({"name": "x", "email": "x@bots.test", "entity_type": "bot", "agent_type": "robot"}),
        )
        .await;
    assert_eq!(refused.code(), Some(400), "{}", refused.body);
    let message = refused.body["message"].as_str().expect("message");
    for agent_type in ["runner", "openclaw", "openprx", "webhook", "custom"] {
        assert!(message.contains(agent_type), "{message}");
    }
    let stored = api
        .db_strings("SELECT email AS v FROM users WHERE email = 'x@bots.test'")
        .await;
    assert!(stored.is_empty(), "{stored:?}");
    api.finish().await;
}

#[tokio::test]
async fn agent_config_is_an_unknown_field() {
    let Some(api) = Api::new("admin_agent_config").await else {
        return;
    };
    api.alice_is_instance_admin().await;
    let created = api
        .post(
            "/api/v1/admin/users",
            api.alice,
            json!({
                "name": "cfg", "email": "cfg@bots.test", "entity_type": "bot",
                "agent_type": "runner", "agent_config": {"endpoint": "x"}
            }),
        )
        .await;
    assert_eq!(created.status, StatusCode::UNPROCESSABLE_ENTITY, "{}", created.body);
    let target = api.add_member("plain bot", "bot").await;
    let updated = api
        .put(
            &format!("/api/v1/admin/users/{target}"),
            api.alice,
            json!({"agent_type": "runner", "agent_config": {}}),
        )
        .await;
    assert_eq!(updated.status, StatusCode::UNPROCESSABLE_ENTITY, "{}", updated.body);
    assert!(
        !api.column_exists("users", "agent_config").await,
        "the column is dropped"
    );
    api.finish().await;
}

#[tokio::test]
async fn setup_status_is_true_only_until_the_first_registration() {
    let Some(scratch) = crate::migrate::db_tests::scratch("setup_status").await else {
        eprintln!("skipped: OPENPR_TEST_DATABASE_URL is not set");
        return;
    };
    let db = scratch.connection().clone();
    crate::migrate::run_migrations(&db).await.expect("migrations apply");
    let state = platform::app::AppState {
        cfg: platform::config::AppConfig {
            app_name: "api-test".to_string(),
            bind_addr: "127.0.0.1:0".to_string(),
            database_url: platform::config::Secret::new("postgres://unused/unused"),
            jwt_secret: platform::config::Secret::new("setup-status-test-secret"),
            jwt_access_ttl_seconds: 900,
            jwt_refresh_ttl_seconds: 3600,
            allow_insecure_cookies: false,
            collab_allowed_origins: Vec::new(),
        },
        db,
        flow_permission_cache: platform::app::FlowPermissionCacheSlot::default(),
    };
    let router = crate::router::build_router(state);
    let call = |method: &'static str, uri: &'static str, body: Option<serde_json::Value>| {
        let router = router.clone();
        async move {
            use tower::ServiceExt as _;
            let mut request = axum::http::Request::builder().method(method).uri(uri);
            if body.is_some() {
                request = request.header(axum::http::header::CONTENT_TYPE, "application/json");
            }
            let request = request
                .body(body.map_or_else(axum::body::Body::empty, |value| {
                    axum::body::Body::from(value.to_string())
                }))
                .expect("request");
            let response = router.oneshot(request).await.expect("answer");
            let bytes = axum::body::to_bytes(response.into_body(), 1024 * 1024)
                .await
                .expect("body");
            serde_json::from_slice::<serde_json::Value>(&bytes).unwrap_or(serde_json::Value::Null)
        }
    };
    let before = call("GET", "/api/v1/auth/setup-status", None).await;
    assert_eq!(before["code"], json!(0), "{before}");
    assert_eq!(before["data"], json!({"needs_setup": true}));
    let registered = call(
        "POST",
        "/api/v1/auth/register",
        Some(json!({"name": "First", "email": "first@setup.test", "password": "long enough"})),
    )
    .await;
    assert_eq!(registered["code"], json!(0), "{registered}");
    let after = call("GET", "/api/v1/auth/setup-status", None).await;
    assert_eq!(after["data"], json!({"needs_setup": false}));
    scratch.drop_self().await;
}

#[tokio::test]
async fn the_flow_feature_answers_and_stores_the_bridge_switch() {
    let Some(api) = Api::new("flow_bridge_switch").await else {
        return;
    };
    let uri = format!("/api/v1/workspaces/{}/features/flow", api.ws);
    let before = api.get(&uri, api.alice).await;
    assert_eq!(before.code(), Some(0), "{}", before.body);
    assert_eq!(before.data()["bridge_enabled"], json!(true), "{}", before.body);

    let off = api
        .put(
            &uri,
            api.alice,
            json!({"bridge_enabled": false, "idempotency_key": "bridge-switch-off-0001"}),
        )
        .await;
    assert_eq!(off.code(), Some(0), "{}", off.body);
    assert_eq!(off.data()["bridge_enabled"], json!(false));
    assert_eq!(
        off.data()["flow_enabled"],
        json!(true),
        "the other settings stay: {}",
        off.body
    );
    let after = api.get(&uri, api.alice).await;
    assert_eq!(after.data()["bridge_enabled"], json!(false), "{}", after.body);

    let stored = api
        .db_strings(&format!(
            "SELECT bridge_enabled::text AS v FROM flow_workspace_settings WHERE workspace_id = '{}'",
            api.ws
        ))
        .await;
    assert_eq!(stored, vec!["false".to_string()]);

    // A member may read it but not change it; an empty change is refused.
    let member = api
        .put(
            &uri,
            api.bob,
            json!({"bridge_enabled": true, "idempotency_key": "bridge-switch-on-0002"}),
        )
        .await;
    assert_ne!(member.code(), Some(0), "{}", member.body);
    let empty = api
        .put(&uri, api.alice, json!({"idempotency_key": "bridge-switch-none-0003"}))
        .await;
    assert_ne!(empty.code(), Some(0), "{}", empty.body);
    let on = api
        .put(
            &uri,
            api.alice,
            json!({"bridge_enabled": true, "idempotency_key": "bridge-switch-on-0004"}),
        )
        .await;
    assert_eq!(on.data()["bridge_enabled"], json!(true), "{}", on.body);
    api.finish().await;
}
