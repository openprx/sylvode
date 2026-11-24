//! Webhook events added to the subscribable list, through the real router: the sprint events, the
//! form record events' dispatch rows, and the subscription rules. Uses the fixture of
//! `item_api_tests` (`alice` owns the workspace, `bob` is a member) against a scratch `PostgreSQL`
//! (skipped without `OPENPR_TEST_DATABASE_URL`).
#![allow(
    clippy::indexing_slicing,
    clippy::too_many_lines,
    clippy::similar_names,
    clippy::items_after_statements,
    clippy::doc_markdown
)]

use super::item_api_tests::{Api, exec};
use axum::http::Method;
use serde_json::{Value, json};
use uuid::Uuid;

const SPRINT_EVENTS: &str =
    r#"["sprint.created","sprint.updated","sprint.started","sprint.cancelled","sprint.completed","sprint.deleted"]"#;

#[tokio::test]
async fn the_subscribable_list_takes_the_new_events_and_still_refuses_an_unknown_one() {
    let Some(api) = Api::new("webhook_events_subscribe").await else {
        return;
    };
    let uri = format!("/api/v1/workspaces/{}/webhooks", api.ws);
    // A literal public address: the outbound check needs no name resolution for it.
    let url = "https://8.8.8.8/hook";
    let events: Vec<&str> = api::entities::webhook::WEBHOOK_EVENTS.to_vec();
    let created = api
        .post(
            &uri,
            api.alice,
            json!({ "name": "everything", "url": url, "events": events }),
        )
        .await;
    assert_eq!(created.code(), Some(0), "{}", created.body);
    assert_eq!(created.data()["events"].as_array().map(Vec::len), Some(events.len()));
    for event in [
        "sprint.cancelled",
        "milestone.deleted",
        "form.record.updated",
        "flow.object.created",
    ] {
        assert!(
            created.data()["events"].as_array().unwrap().iter().any(|e| e == event),
            "{event}"
        );
    }
    for bad in [
        "sprint.bogus",
        "flow.object.purged",
        "form.record.recalculated",
        "veto.exercise",
    ] {
        let refused = api
            .post(&uri, api.alice, json!({ "name": "bad", "url": url, "events": [bad] }))
            .await;
        assert_eq!(refused.code(), Some(400), "{bad}: {}", refused.body);
        assert!(
            refused.body["message"]
                .as_str()
                .is_some_and(|text| text.contains("Invalid event type")),
            "{bad}: {}",
            refused.body
        );
    }
    api.finish().await;
}

#[tokio::test]
async fn sprints_send_created_updated_started_cancelled_completed_and_deleted() {
    let Some(api) = Api::new("webhook_events_sprint").await else {
        return;
    };
    api.add_webhook_for(SPRINT_EVENTS).await;
    let sprint = api
        .new_sprint("Alpha", json!({ "description": "first", "start_date": "2031-01-01" }))
        .await;
    let created = api.payloads("sprint.created", 1).await;
    assert_eq!(created[0]["event"], "sprint.created");
    assert_eq!(created[0]["data"]["sprint"]["id"], json!(sprint));
    assert_eq!(created[0]["data"]["sprint"]["name"], "Alpha");
    assert_eq!(created[0]["data"]["sprint"]["status"], "planned");
    assert_eq!(created[0]["actor"]["id"], json!(api.alice));

    // Only what the request changes is reported, old and new.
    let uri = format!("/api/v1/sprints/{sprint}");
    let edited = api
        .put(
            &uri,
            api.bob,
            json!({ "name": "Beta", "description": "first", "end_date": "2031-02-01" }),
        )
        .await;
    assert_eq!(edited.code(), Some(0), "{}", edited.body);
    let updated = api.payloads("sprint.updated", 1).await;
    assert_eq!(updated[0]["data"]["sprint"]["name"], "Beta");
    assert_eq!(
        updated[0]["data"]["changes"],
        json!({
            "name": { "old": "Alpha", "new": "Beta" },
            "end_date": { "old": null, "new": "2031-02-01" },
        })
    );
    // The same values again change nothing and send nothing.
    let again = api
        .put(&uri, api.bob, json!({ "name": "Beta", "end_date": "2031-02-01" }))
        .await;
    assert_eq!(again.code(), Some(0), "{}", again.body);
    api.payloads("sprint.updated", 1).await;

    // A status change alone is a started event, not an update; with a name change it is both.
    assert_eq!(api.set_status(sprint, api.alice, "active").await.code(), Some(0));
    api.payloads("sprint.started", 1).await;
    api.payloads("sprint.updated", 1).await;

    // Cancelling reports the rollover of the unfinished items, as completing does.
    let next = api.new_sprint("Next", json!({})).await;
    api.payloads("sprint.created", 2).await;
    let open = api.sprint_item("open", sprint, "todo").await;
    let cancelled = api
        .put(
            &uri,
            api.alice,
            json!({ "status": "cancelled", "rollover_sprint_id": next }),
        )
        .await;
    assert_eq!(cancelled.code(), Some(0), "{}", cancelled.body);
    assert_eq!(api.sprint_of(open).await, Some(next));
    let sent = api.payloads("sprint.cancelled", 1).await;
    assert_eq!(sent[0]["data"]["sprint"]["status"], "cancelled");
    assert_eq!(
        sent[0]["data"]["rollover"],
        json!({ "moved_count": 1, "target_sprint_id": next })
    );
    api.payloads("sprint.completed", 0).await;

    // Deleting sends the sprint as it was; the row is gone by then.
    let doomed = api
        .new_sprint(
            "Doomed",
            json!({ "description": "bye", "start_date": "2031-03-01", "end_date": "2031-03-31" }),
        )
        .await;
    let deleted = api
        .call(Method::DELETE, &format!("/api/v1/sprints/{doomed}"), api.alice, None)
        .await;
    assert_eq!(deleted.code(), Some(0), "{}", deleted.body);
    let sent = api.payloads("sprint.deleted", 1).await;
    assert_eq!(sent[0]["event"], "sprint.deleted");
    let gone = &sent[0]["data"]["sprint"];
    assert_eq!(gone["id"], json!(doomed));
    assert_eq!(gone["name"], "Doomed");
    assert_eq!(gone["description"], "bye");
    assert_eq!(gone["status"], "planned");
    assert_eq!(
        (gone["start_date"].clone(), gone["end_date"].clone()),
        (json!("2031-03-01"), json!("2031-03-31"))
    );
    assert!(gone.get("project_id").is_none());
    // A refused delete (a member) sends nothing.
    let other = api.new_sprint("Kept", json!({})).await;
    let refused = api
        .call(Method::DELETE, &format!("/api/v1/sprints/{other}"), api.bob, None)
        .await;
    assert_eq!(refused.code(), Some(403), "{}", refused.body);
    api.payloads("sprint.deleted", 1).await;
    api.finish().await;
}

async fn dispatch_rows(api: &Api, event_type: &str) -> i64 {
    api.db_count(
        &format!(
            "SELECT COUNT(*)::bigint AS n FROM event_dispatch WHERE workspace_id = $1 AND event_type = '{event_type}'"
        ),
        api.ws,
    )
    .await
}

#[tokio::test]
async fn form_record_routes_write_the_dispatch_rows_and_the_changed_field_keys() {
    let Some(api) = Api::new("webhook_events_form").await else {
        return;
    };
    let form = Uuid::new_v4();
    exec(
        &api.db,
        "INSERT INTO project_forms (id, workspace_id, project_id, key, name, schema, created_by) \
         VALUES ($1, $2, $3, 'staff', 'Staff', $4, $5)",
        vec![
            form.into(),
            api.ws.into(),
            api.project.into(),
            json!({"version": "openpr.form.schema.v1", "fields": [
                {"field_id": "fld_text", "key": "text", "type": "text"},
                {"field_id": "fld_salary", "key": "salary", "type": "text"}
            ]})
            .into(),
            api.alice.into(),
        ],
    )
    .await;
    let changed_fields = |event_type: &'static str| {
        let api = &api;
        async move {
            let text = api
                .db_count_text(
                    &format!(
                        "SELECT (metadata->'changed_fields')::text AS n FROM business_events \
                          WHERE workspace_id = $1 AND event_type = '{event_type}' ORDER BY created_at DESC LIMIT 1"
                    ),
                    api.ws,
                )
                .await;
            serde_json::from_str::<Value>(&text).unwrap_or(Value::Null)
        }
    };

    let created = api
        .post(
            &format!("/api/v1/forms/{form}/records"),
            api.alice,
            json!({"values": {"text": "hello", "salary": "9000"}}),
        )
        .await;
    assert_eq!(created.code(), Some(0), "{}", created.body);
    let record = created.data()["id"].as_str().expect("record id").to_owned();
    assert_eq!(dispatch_rows(&api, "form.record.created").await, 1);
    assert_eq!(changed_fields("form.record.created").await, json!(["salary", "text"]));

    // Only the field whose value differs is named.
    let updated = api
        .call(
            Method::PATCH,
            &format!("/api/v1/form-records/{record}"),
            api.alice,
            Some(json!({"values": {"text": "hello", "salary": "9500"}})),
        )
        .await;
    assert_eq!(updated.code(), Some(0), "{}", updated.body);
    assert_eq!(dispatch_rows(&api, "form.record.updated").await, 1);
    assert_eq!(changed_fields("form.record.updated").await, json!(["salary"]));

    let archived = api
        .call(
            Method::DELETE,
            &format!("/api/v1/form-records/{record}"),
            api.alice,
            None,
        )
        .await;
    assert_eq!(archived.code(), Some(0), "{}", archived.body);
    assert_eq!(dispatch_rows(&api, "form.record.archived").await, 1);
    let restored = api
        .post(&format!("/api/v1/form-records/{record}/restore"), api.alice, json!({}))
        .await;
    assert_eq!(restored.code(), Some(0), "{}", restored.body);
    assert_eq!(dispatch_rows(&api, "form.record.restored").await, 1);

    // The derived events of a write (a recalculation, a table change) are not dispatched.
    let total = api
        .db_count(
            "SELECT COUNT(*)::bigint AS n FROM event_dispatch WHERE workspace_id = $1 AND event_type LIKE 'form.%'",
            api.ws,
        )
        .await;
    assert_eq!(total, 4);
    // A refused write leaves no event: a person outside the workspace cannot create a record.
    let refused = api
        .post(
            &format!("/api/v1/forms/{form}/records"),
            api.carol,
            json!({"values": {"text": "no"}}),
        )
        .await;
    assert_ne!(refused.code(), Some(0));
    assert_eq!(dispatch_rows(&api, "form.record.created").await, 1);
    api.finish().await;
}
