//! Plugins and the restaurant template forms through the real router: one active version per plugin
//! key, and the restaurant order line, which calculates its total itself. Uses the fixture
//! of `item_api_tests` against a scratch `PostgreSQL` (skipped without `OPENPR_TEST_DATABASE_URL`).
#![allow(
    clippy::indexing_slicing,
    clippy::too_many_lines,
    clippy::similar_names,
    clippy::items_after_statements,
    clippy::doc_markdown
)]

use super::item_api_tests::{Api, exec};
use serde_json::{Value, json};
use uuid::Uuid;

/// The smallest valid WebAssembly module (magic and version).
const EMPTY_WASM_BASE64: &str = "AGFzbQEAAAA=";

fn manifest(key: &str, version: &str) -> Value {
    json!({
        "schema_version": "openpr.plugin.v1",
        "key": key,
        "name": "Test plugin",
        "version": version,
        "capabilities": {
            "hooks": [{"kind": "formula", "form_key": "order_line", "field_key": "line_total"}],
            "runtime": {"timeout_ms": 500, "fuel": 100_000, "memory_bytes": 1_048_576}
        }
    })
}

impl Api {
    pub(super) async fn install_plugin(&self, key: &str, version: &str, status: &str) -> Uuid {
        let answer = self
            .post(
                &format!("/api/v1/projects/{}/plugins", self.project),
                self.alice,
                json!({"manifest": manifest(key, version), "wasm_base64": EMPTY_WASM_BASE64, "status": status}),
            )
            .await;
        assert_eq!(answer.code(), Some(0), "install {key} {version}: {}", answer.body);
        Uuid::parse_str(answer.data()["id"].as_str().expect("id")).expect("uuid")
    }

    pub(super) async fn plugin_status(&self, id: Uuid) -> String {
        let answer = self.get(&format!("/api/v1/plugins/{id}"), self.alice).await;
        answer.data()["status"].as_str().expect("status").to_string()
    }

    pub(super) async fn active_versions(&self, key: &str) -> i64 {
        self.db_count(
            &format!("SELECT COUNT(*)::bigint AS n FROM plugins WHERE project_id = $1 AND key = '{key}' AND status = 'active'"),
            self.project,
        )
        .await
    }
}

#[tokio::test]
async fn activating_a_version_disables_the_other_active_version() {
    let Some(api) = Api::new("plugin_activate_disables").await else {
        return;
    };
    let one = api.install_plugin("calc", "1.0.0", "active").await;
    let two = api.install_plugin("calc", "2.0.0", "disabled").await;
    assert_eq!(
        (api.plugin_status(one).await, api.plugin_status(two).await),
        ("active".into(), "disabled".into())
    );
    let answer = api
        .call(
            axum::http::Method::PATCH,
            &format!("/api/v1/plugins/{two}"),
            api.alice,
            Some(json!({"status": "active"})),
        )
        .await;
    assert_eq!(answer.code(), Some(0), "{} {}", answer.status, answer.body);
    assert_eq!(api.plugin_status(one).await, "disabled");
    assert_eq!(api.plugin_status(two).await, "active");
    assert_eq!(api.active_versions("calc").await, 1);
    // The disabled version left a plugin.updated event; another key is not touched.
    let events = api
        .db_count(
            &format!("SELECT COUNT(*)::bigint AS n FROM business_events WHERE event_type = 'plugin.updated' AND payload->>'plugin_id' = '{one}'"),
            api.project,
        )
        .await;
    assert_eq!(events, 1, "one plugin.updated for the version that was disabled");
    let other = api.install_plugin("other", "1.0.0", "active").await;
    assert_eq!(api.plugin_status(other).await, "active");
    assert_eq!(api.plugin_status(two).await, "active");
    api.finish().await;
}

#[tokio::test]
async fn installing_an_active_version_disables_the_previous() {
    let Some(api) = Api::new("plugin_install_disables").await else {
        return;
    };
    let one = api.install_plugin("calc", "1.0.0", "active").await;
    let two = api.install_plugin("calc", "2.0.0", "active").await;
    assert_eq!(api.plugin_status(one).await, "disabled");
    assert_eq!(api.plugin_status(two).await, "active");
    assert_eq!(api.active_versions("calc").await, 1);
    // A disabled install changes nothing about the active one.
    api.install_plugin("calc", "3.0.0", "disabled").await;
    assert_eq!(api.plugin_status(two).await, "active");
    api.finish().await;
}

#[tokio::test]
async fn only_one_version_of_a_key_can_be_active_at_the_database_level() {
    let Some(api) = Api::new("plugin_one_active_index").await else {
        return;
    };
    let one = api.install_plugin("calc", "1.0.0", "active").await;
    let two = api.install_plugin("calc", "2.0.0", "disabled").await;
    // Whatever path writes the row, a second active version of the key is refused, so the hooks
    // (which load every active row of the project) cannot run two versions.
    let refused = api
        .db
        .execute(sea_orm::Statement::from_sql_and_values(
            sea_orm::DbBackend::Postgres,
            "UPDATE plugins SET status = 'active' WHERE id = $1",
            vec![two.into()],
        ))
        .await;
    assert!(refused.is_err(), "the index refuses a second active version");
    assert_eq!(api.plugin_status(one).await, "active");
    api.finish().await;
}

use sea_orm::ConnectionTrait as _;

async fn forms_by_key(api: &Api, project: Uuid) -> std::collections::HashMap<String, String> {
    let answer = api.get(&format!("/api/v1/projects/{project}/forms"), api.alice).await;
    assert_eq!(answer.code(), Some(0), "{}", answer.body);
    let items = answer.data()["items"]
        .as_array()
        .or_else(|| answer.data().as_array())
        .expect("forms")
        .clone();
    items
        .iter()
        .map(|form| {
            (
                form["key"].as_str().expect("key").to_string(),
                form["id"].as_str().expect("id").to_string(),
            )
        })
        .collect()
}

async fn new_record(api: &Api, form: &str, values: Value) -> Value {
    let answer = api
        .post(
            &format!("/api/v1/forms/{form}/records"),
            api.alice,
            json!({ "values": values }),
        )
        .await;
    assert_eq!(answer.code(), Some(0), "record in {form}: {}", answer.body);
    answer.data().clone()
}

/// The restaurant template package's form files, in dependency order.
const RESTAURANT_FORMS: [&str; 5] = [
    include_str!("../../../packages/builtin/restaurant-ordering/contents/forms/table.json"),
    include_str!("../../../packages/builtin/restaurant-ordering/contents/forms/sku.json"),
    include_str!("../../../packages/builtin/restaurant-ordering/contents/forms/menu_category.json"),
    include_str!("../../../packages/builtin/restaurant-ordering/contents/forms/order.json"),
    include_str!("../../../packages/builtin/restaurant-ordering/contents/forms/order_line.json"),
];

#[tokio::test]
async fn a_restaurant_order_line_computes_the_total_from_quantity_and_unit_price() {
    let Some(api) = Api::new("restaurant_line_total").await else {
        return;
    };
    let created = api
        .post(
            &format!("/api/v1/workspaces/{}/projects", api.ws),
            api.alice,
            json!({"key": "REST", "name": "Restaurant"}),
        )
        .await;
    assert_eq!(created.code(), Some(0), "{}", created.body);
    let project = Uuid::parse_str(created.data()["id"].as_str().expect("id")).expect("uuid");
    // The forms of the package source, created like any user would create them.
    for source in RESTAURANT_FORMS {
        let form: Value = serde_json::from_str(source).expect("form file");
        let made = api
            .post(&format!("/api/v1/projects/{project}/forms"), api.alice, form)
            .await;
        assert_eq!(made.code(), Some(0), "{}", made.body);
    }

    let forms = forms_by_key(&api, project).await;
    let order_line = forms.get("order_line").expect("order_line form").clone();
    // Records for the relations the order line needs; the values below are the real inputs.
    let table = new_record(&api, &forms["table"], json!({"table_no": "3"})).await;
    let sku_record = new_record(
        &api,
        &forms["sku"],
        json!({"sku": "NOO-1", "name": "Noodles", "price": "9.99"}),
    )
    .await;
    let order = new_record(
        &api,
        &forms["order"],
        json!({"order_no": "A-1", "table_id": table["id"], "status": "draft"}),
    )
    .await;
    let mut totals = Vec::new();
    for (quantity, price) in [(2, "9.99"), (5, "1.10"), (3, "12.34")] {
        let line = new_record(
            &api,
            &order_line,
            json!({
                "order_id": order["id"], "sku_id": sku_record["id"], "sku_name": "Noodles",
                "quantity": quantity, "unit_price": price
            }),
        )
        .await;
        totals.push(line["values"]["line_total"]["decimal"].clone());
    }
    assert_eq!(
        totals,
        [json!("19.98"), json!("5.50"), json!("37.02")],
        "the total follows the inputs, it is not a constant"
    );
    api.finish().await;
}

#[tokio::test]
async fn the_active_version_index_exists_and_old_duplicates_are_repaired() {
    let Some(api) = Api::new("plugin_single_active_data").await else {
        return;
    };
    // Rebuild the state before 0119: no index, two active versions of one key.
    exec(&api.db, "DROP INDEX plugins_one_active_version", vec![]).await;
    let [older, newer] = [Uuid::new_v4(), Uuid::new_v4()];
    for (id, version, updated) in [(older, "1.0.0", "2030-01-01"), (newer, "2.0.0", "2030-01-02")] {
        exec(
            &api.db,
            "INSERT INTO plugins (id, workspace_id, project_id, key, name, version, manifest, status, updated_at) \
             VALUES ($1, $2, $3, 'dup', 'dup', $4, '{}'::jsonb, 'active', $5::timestamptz)",
            vec![
                id.into(),
                api.ws.into(),
                api.project.into(),
                version.into(),
                updated.into(),
            ],
        )
        .await;
    }
    let sql = std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../migrations/0119_plugin_single_active_version.sql"
    ))
    .expect("the migration file");
    // 0119 also edits a scenario template row; 0141 has dropped that table since, so the
    // (idempotent) 0029 file brings it back for this stand-alone run.
    let scenario_templates = std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../migrations/0029_scenario_templates.sql"
    ))
    .expect("the 0029 file");
    api.db
        .execute_unprepared(&scenario_templates)
        .await
        .expect("0029 recreates scenario_templates");
    api.db.execute_unprepared(&sql).await.expect("0119 runs");
    assert_eq!(
        (api.plugin_status(older).await, api.plugin_status(newer).await),
        ("disabled".into(), "active".into())
    );
    api.db.execute_unprepared(&sql).await.expect("0119 runs again");
    assert_eq!(api.active_versions("dup").await, 1);
    api.finish().await;
}
