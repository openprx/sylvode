//! Label rules through the real router: colours, names ignoring case, idempotent attach and
//! detach, the events they send, and project creation from every built-in template. Uses the
//! fixture of `item_api_tests` (`alice` owns the workspace) against a scratch `PostgreSQL`
//! (skipped without `OPENPR_TEST_DATABASE_URL`).
#![allow(
    clippy::indexing_slicing,
    clippy::too_many_lines,
    clippy::similar_names,
    clippy::items_after_statements,
    clippy::doc_markdown
)]

use super::item_api_tests::{Answer, Api};
use axum::http::{Method, StatusCode};
use serde_json::{Value, json};
use uuid::Uuid;

impl Api {
    pub(super) fn labels_uri(&self) -> String {
        format!("/api/v1/workspaces/{}/labels", self.ws)
    }

    pub(super) async fn new_label(&self, body: Value) -> Answer {
        self.post(&self.labels_uri(), self.alice, body).await
    }

    pub(super) async fn label_id(&self, name: &str, color: &str) -> Uuid {
        let answer = self.new_label(json!({"name": name, "color": color})).await;
        assert_eq!(answer.code(), Some(0), "{}", answer.body);
        Uuid::parse_str(answer.data()["id"].as_str().expect("id")).expect("uuid")
    }

    pub(super) async fn attached(&self, item: Uuid) -> Vec<String> {
        let answer = self.get(&format!("/api/v1/issues/{item}/labels"), self.alice).await;
        answer.data()["items"]
            .as_array()
            .expect("items")
            .iter()
            .map(|label| label["name"].as_str().expect("name").to_string())
            .collect()
    }
}

#[tokio::test]
async fn default_label_colour_is_hex() {
    let Some(api) = Api::new("label_default_colour").await else {
        return;
    };
    let created = api.new_label(json!({"name": "plain"})).await;
    assert_eq!(created.code(), Some(0), "{}", created.body);
    assert_eq!(created.data()["color"], json!("#6b7280"));
    // A client that sends an explicit null for the colour (the MCP tool does) gets the default too.
    let nulled = api.new_label(json!({"name": "nulled", "color": null})).await;
    assert_eq!(nulled.data()["color"], json!("#6b7280"), "{}", nulled.body);
    // A label inserted without a colour (older code paths) gets the database default.
    super::item_api_tests::exec(
        &api.db,
        "INSERT INTO labels (id, workspace_id, name) VALUES ($1, $2, 'raw')",
        vec![Uuid::new_v4().into(), api.ws.into()],
    )
    .await;
    let listed = api.get(&api.labels_uri(), api.alice).await;
    for label in listed.data()["items"].as_array().expect("items") {
        let color = label["color"].as_str().expect("color");
        assert!(
            color.len() == 7 && color.starts_with('#') && color[1..].chars().all(|c| c.is_ascii_hexdigit()),
            "{color}"
        );
    }
    api.finish().await;
}

#[tokio::test]
async fn invalid_colour_is_400_on_create_and_update() {
    let Some(api) = Api::new("label_bad_colour").await else {
        return;
    };
    let label = api.label_id("coloured", "#112233").await;
    for bad in ["#gray", "red", "#fff", "#12345g", "112233", "#1122334", ""] {
        let created = api.new_label(json!({"name": format!("bad {bad}"), "color": bad})).await;
        assert_eq!(created.code(), Some(400), "create {bad:?}: {}", created.body);
        assert_eq!(created.body["message"], json!("color must be a hex colour #rrggbb"));
        let updated = api
            .put(&format!("/api/v1/labels/{label}"), api.alice, json!({"color": bad}))
            .await;
        assert_eq!(updated.code(), Some(400), "update {bad:?}: {}", updated.body);
    }
    let nulled = api
        .put(&format!("/api/v1/labels/{label}"), api.alice, json!({"color": null}))
        .await;
    assert_eq!(nulled.code(), Some(400), "{}", nulled.body);
    assert_eq!(nulled.body["message"], json!("color cannot be null"));
    let still = api.get(&api.labels_uri(), api.alice).await;
    assert_eq!(still.data()["items"][0]["color"], json!("#112233"));
    api.finish().await;
}

#[tokio::test]
async fn colour_is_stored_lower_case() {
    let Some(api) = Api::new("label_lower_colour").await else {
        return;
    };
    let created = api.new_label(json!({"name": "shouty", "color": "#AABBCC"})).await;
    assert_eq!(created.data()["color"], json!("#aabbcc"), "{}", created.body);
    let label = Uuid::parse_str(created.data()["id"].as_str().expect("id")).expect("uuid");
    let updated = api
        .put(
            &format!("/api/v1/labels/{label}"),
            api.alice,
            json!({"color": " #DDEEFF "}),
        )
        .await;
    assert_eq!(updated.data()["color"], json!("#ddeeff"), "{}", updated.body);
    assert_eq!(
        api.db_count_text("SELECT color AS n FROM labels WHERE id = $1", label)
            .await,
        "#ddeeff"
    );
    api.finish().await;
}

#[tokio::test]
async fn unknown_fields_and_nulls_are_refused() {
    let Some(api) = Api::new("label_unknown_fields").await else {
        return;
    };
    let created = api.new_label(json!({"name": "x", "project_id": Uuid::new_v4()})).await;
    assert_eq!(created.status, StatusCode::UNPROCESSABLE_ENTITY, "{}", created.body);
    let label = api.label_id("typed", "#112233").await;
    let uri = format!("/api/v1/labels/{label}");
    let updated = api.put(&uri, api.alice, json!({"colour": "#112233"})).await;
    assert_eq!(updated.status, StatusCode::UNPROCESSABLE_ENTITY, "{}", updated.body);
    for field in ["name", "description"] {
        let nulled = api.put(&uri, api.alice, json!({ field: null })).await;
        assert_eq!(nulled.code(), Some(400), "{field}: {}", nulled.body);
        assert_eq!(nulled.body["message"], json!(format!("{field} cannot be null")));
    }
    api.finish().await;
}

#[tokio::test]
async fn names_are_unique_ignoring_case_on_create_and_rename() {
    let Some(api) = Api::new("label_names").await else {
        return;
    };
    let bug = api.label_id("Bug", "#112233").await;
    let twin = api.new_label(json!({"name": "bug"})).await;
    assert_eq!(twin.code(), Some(409), "{}", twin.body);
    let other = api.label_id("Other", "#112233").await;
    let renamed = api
        .put(&format!("/api/v1/labels/{other}"), api.alice, json!({"name": "BUG"}))
        .await;
    assert_eq!(renamed.code(), Some(409), "{}", renamed.body);
    // A label may change the case of its own name.
    let own = api
        .put(&format!("/api/v1/labels/{bug}"), api.alice, json!({"name": "BUG"}))
        .await;
    assert_eq!(own.code(), Some(0), "{}", own.body);
    // Two racing creations: exactly one wins.
    let uri = api.labels_uri();
    let (a, b) = tokio::join!(
        api.post(&uri, api.alice, json!({"name": "Racing"})),
        api.post(&uri, api.alice, json!({"name": "racing"}))
    );
    let codes = [a.code(), b.code()];
    assert!(codes.contains(&Some(0)) && codes.contains(&Some(409)), "{codes:?}");
    api.finish().await;
}

#[tokio::test]
async fn attach_twice_succeeds_and_emits_one_label_added() {
    let Some(api) = Api::new("label_attach_twice").await else {
        return;
    };
    api.add_webhook().await;
    let item = api.create_item("tagged", json!({})).await;
    let label = api.label_id("tag", "#112233").await;
    let uri = format!("/api/v1/issues/{item}/labels/{label}");
    for _ in 0..2 {
        let answer = api.post(&uri, api.alice, json!({})).await;
        assert_eq!(answer.code(), Some(0), "{}", answer.body);
    }
    assert_eq!(api.attached(item).await, ["tag"]);
    let added = api.payloads("label.added", 1).await;
    assert_eq!(added[0]["data"]["label"]["name"], json!("tag"));
    api.finish().await;
}

#[tokio::test]
async fn detach_unattached_succeeds_without_event() {
    let Some(api) = Api::new("label_detach_unattached").await else {
        return;
    };
    api.add_webhook().await;
    let item = api.create_item("untagged", json!({})).await;
    let label = api.label_id("tag", "#112233").await;
    let uri = format!("/api/v1/issues/{item}/labels/{label}");
    let answer = api.call(Method::DELETE, &uri, api.alice, None).await;
    assert_eq!(answer.code(), Some(0), "{}", answer.body);
    api.payloads("label.removed", 0).await;
    // A real detach sends one event; repeating it sends none.
    api.post(&uri, api.alice, json!({})).await;
    api.payloads("label.added", 1).await;
    for _ in 0..2 {
        let answer = api.call(Method::DELETE, &uri, api.alice, None).await;
        assert_eq!(answer.code(), Some(0), "{}", answer.body);
    }
    api.payloads("label.removed", 1).await;
    api.finish().await;
}

#[tokio::test]
async fn batch_attach_emits_label_added_per_new_label() {
    let Some(api) = Api::new("label_batch_events").await else {
        return;
    };
    api.add_webhook().await;
    let item = api.create_item("batched", json!({})).await;
    let (one, two, three) = (
        api.label_id("one", "#111111").await,
        api.label_id("two", "#222222").await,
        api.label_id("three", "#333333").await,
    );
    // `two` is already there, so only `one` and `three` are new.
    api.post(&format!("/api/v1/issues/{item}/labels/{two}"), api.alice, json!({}))
        .await;
    api.payloads("label.added", 1).await;
    let answer = api
        .post(
            &format!("/api/v1/issues/{item}/labels/batch"),
            api.bob,
            json!({"label_ids": [one, two, three, one]}),
        )
        .await;
    assert_eq!(answer.code(), Some(0), "{}", answer.body);
    let added = api.payloads("label.added", 3).await;
    let mut names: Vec<&str> = added
        .iter()
        .map(|payload| payload["data"]["label"]["name"].as_str().expect("name"))
        .collect();
    names.sort_unstable();
    assert_eq!(names, ["one", "three", "two"]);
    assert_eq!(api.attached(item).await.len(), 3);
    api.finish().await;
}

#[tokio::test]
async fn deleting_an_attached_label_emits_label_removed_per_item() {
    let Some(api) = Api::new("label_delete_events").await else {
        return;
    };
    api.add_webhook().await;
    let (first, second, loose) = (
        api.create_item("first", json!({})).await,
        api.create_item("second", json!({})).await,
        api.create_item("untouched", json!({})).await,
    );
    let label = api.label_id("doomed", "#ABCDEF").await;
    for item in [first, second] {
        api.post(&format!("/api/v1/issues/{item}/labels/{label}"), api.alice, json!({}))
            .await;
    }
    api.payloads("label.added", 2).await;
    let deleted = api
        .call(Method::DELETE, &format!("/api/v1/labels/{label}"), api.alice, None)
        .await;
    assert_eq!(deleted.code(), Some(0), "{}", deleted.body);

    let removed = api.payloads("label.removed", 2).await;
    let mut keys: Vec<String> = removed
        .iter()
        .map(|payload| payload["data"]["issue"]["key"].as_str().expect("key").to_string())
        .collect();
    keys.sort();
    assert_eq!(keys, ["QLB-1", "QLB-2"]);
    for payload in &removed {
        assert_eq!(payload["data"]["label"]["name"], json!("doomed"), "{payload}");
        assert_eq!(payload["data"]["label"]["color"], json!("#abcdef"), "{payload}");
        assert_eq!(payload["data"]["label"]["id"], json!(label));
        assert!(payload["data"]["issue"]["title"].is_string());
    }
    let _ = loose;
    api.finish().await;
}
