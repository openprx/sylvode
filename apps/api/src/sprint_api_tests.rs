//! Sprint rules through the real router: the lifecycle, the single active sprint, dates, names,
//! permissions and reading one sprint. Uses the fixture of `item_api_tests` (`alice` owns the
//! workspace, `bob` is a member, `carol` belongs to another workspace) against a scratch
//! `PostgreSQL` (skipped without `OPENPR_TEST_DATABASE_URL`).
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
    pub(super) fn sprints_uri(&self) -> String {
        format!("/api/v1/projects/{}/sprints", self.project)
    }

    /// Creates a sprint as alice; panics unless it is accepted.
    pub(super) async fn new_sprint(&self, name: &str, extra: Value) -> Uuid {
        let mut body = json!({ "name": name });
        if let (Some(target), Some(source)) = (body.as_object_mut(), extra.as_object()) {
            for (key, value) in source {
                target.insert(key.clone(), value.clone());
            }
        }
        let answer = self.post(&self.sprints_uri(), self.alice, body).await;
        assert_eq!(answer.code(), Some(0), "create sprint {name}: {}", answer.body);
        Uuid::parse_str(answer.data()["id"].as_str().expect("id")).expect("uuid")
    }

    pub(super) async fn set_status(&self, sprint: Uuid, user: Uuid, status: &str) -> Answer {
        self.put(&format!("/api/v1/sprints/{sprint}"), user, json!({ "status": status }))
            .await
    }

    pub(super) async fn sprint_status(&self, sprint: Uuid) -> String {
        let answer = self.get(&format!("/api/v1/sprints/{sprint}"), self.alice).await;
        answer.data()["status"].as_str().expect("status").to_string()
    }
}

#[tokio::test]
async fn create_refuses_end_before_start() {
    let Some(api) = Api::new("sprint_create_dates").await else {
        return;
    };
    let refused = api
        .post(
            &api.sprints_uri(),
            api.alice,
            json!({"name": "backwards", "start_date": "2030-02-10", "end_date": "2030-02-01"}),
        )
        .await;
    assert_eq!(refused.code(), Some(400), "{}", refused.body);
    assert_eq!(refused.body["message"], json!("end_date must not be before start_date"));
    // The same day is fine.
    api.new_sprint("one day", json!({"start_date": "2030-02-01", "end_date": "2030-02-01"}))
        .await;
    api.finish().await;
}

#[tokio::test]
async fn edit_refuses_end_before_stored_start() {
    let Some(api) = Api::new("sprint_edit_dates").await else {
        return;
    };
    let sprint = api.new_sprint("dated", json!({"start_date": "2030-02-10"})).await;
    let uri = format!("/api/v1/sprints/{sprint}");
    let refused = api.put(&uri, api.alice, json!({"end_date": "2030-02-01"})).await;
    assert_eq!(refused.code(), Some(400), "{}", refused.body);
    // The other way round: a start after the stored end.
    let ended = api.put(&uri, api.alice, json!({"end_date": "2030-02-20"})).await;
    assert_eq!(ended.code(), Some(0), "{}", ended.body);
    let refused = api.put(&uri, api.alice, json!({"start_date": "2030-03-01"})).await;
    assert_eq!(refused.code(), Some(400), "{}", refused.body);
    api.finish().await;
}

#[tokio::test]
async fn clearing_a_date_is_allowed() {
    let Some(api) = Api::new("sprint_clear_date").await else {
        return;
    };
    let sprint = api
        .new_sprint("cleared", json!({"start_date": "2030-02-01", "end_date": "2030-02-10"}))
        .await;
    let uri = format!("/api/v1/sprints/{sprint}");
    let cleared = api.put(&uri, api.alice, json!({"end_date": null})).await;
    assert_eq!(cleared.code(), Some(0), "{}", cleared.body);
    assert!(cleared.data()["end_date"].is_null(), "{}", cleared.body);
    assert_eq!(cleared.data()["start_date"], json!("2030-02-01"));
    let both = api
        .put(&uri, api.alice, json!({"start_date": null, "end_date": null}))
        .await;
    assert_eq!(both.code(), Some(0), "{}", both.body);
    assert!(both.data()["start_date"].is_null());
    // `null` on a field that cannot be cleared is refused.
    let refused = api.put(&uri, api.alice, json!({"name": null})).await;
    assert_eq!(refused.code(), Some(400), "{}", refused.body);
    assert_eq!(refused.body["message"], json!("name cannot be null"));
    api.finish().await;
}

#[tokio::test]
async fn unknown_fields_are_refused() {
    let Some(api) = Api::new("sprint_unknown_field").await else {
        return;
    };
    let created = api
        .post(&api.sprints_uri(), api.alice, json!({"name": "x", "goal": "ship"}))
        .await;
    assert_eq!(created.status, StatusCode::UNPROCESSABLE_ENTITY, "{}", created.body);
    let sprint = api.new_sprint("typed", json!({})).await;
    let updated = api
        .put(&format!("/api/v1/sprints/{sprint}"), api.alice, json!({"goal": "ship"}))
        .await;
    assert_eq!(updated.status, StatusCode::UNPROCESSABLE_ENTITY, "{}", updated.body);
    api.finish().await;
}

#[tokio::test]
async fn allowed_transitions_succeed() {
    let Some(api) = Api::new("sprint_transitions_ok").await else {
        return;
    };
    let ran = api.new_sprint("ran", json!({})).await;
    for status in ["active", "completed"] {
        let answer = api.set_status(ran, api.alice, status).await;
        assert_eq!(answer.code(), Some(0), "-> {status}: {}", answer.body);
        assert_eq!(answer.data()["status"], json!(status));
    }
    let dropped = api.new_sprint("dropped", json!({})).await;
    assert_eq!(api.set_status(dropped, api.alice, "cancelled").await.code(), Some(0));
    let stopped = api.new_sprint("stopped", json!({})).await;
    assert_eq!(api.set_status(stopped, api.alice, "active").await.code(), Some(0));
    assert_eq!(api.set_status(stopped, api.alice, "cancelled").await.code(), Some(0));
    assert_eq!(api.sprint_status(stopped).await, "cancelled");
    api.finish().await;
}

#[tokio::test]
async fn refused_transitions_answer_400() {
    let Some(api) = Api::new("sprint_transitions_refused").await else {
        return;
    };
    let completed = api.new_sprint("completed", json!({})).await;
    api.set_status(completed, api.alice, "active").await;
    api.set_status(completed, api.alice, "completed").await;
    let cancelled = api.new_sprint("cancelled", json!({})).await;
    api.set_status(cancelled, api.alice, "cancelled").await;
    let active = api.new_sprint("active", json!({})).await;
    api.set_status(active, api.alice, "active").await;
    let planned = api.new_sprint("planned", json!({})).await;

    for (sprint, from, to) in [
        (active, "active", "planned"),
        (planned, "planned", "completed"),
        (completed, "completed", "active"),
        (completed, "completed", "cancelled"),
        (completed, "completed", "planned"),
        (cancelled, "cancelled", "planned"),
        (cancelled, "cancelled", "active"),
    ] {
        let answer = api.set_status(sprint, api.alice, to).await;
        assert_eq!(answer.code(), Some(400), "{from} -> {to}: {}", answer.body);
        assert_eq!(
            answer.body["message"],
            json!(format!("sprint status cannot change from {from} to {to}"))
        );
        assert_eq!(api.sprint_status(sprint).await, from, "{from} -> {to} changed it");
    }
    let unknown = api.set_status(planned, api.alice, "bogus").await;
    assert_eq!(unknown.code(), Some(400), "{}", unknown.body);
    api.finish().await;
}

#[tokio::test]
async fn creating_completed_or_cancelled_is_refused() {
    let Some(api) = Api::new("sprint_create_status").await else {
        return;
    };
    for status in ["completed", "cancelled", "bogus"] {
        let answer = api
            .post(&api.sprints_uri(), api.alice, json!({"name": status, "status": status}))
            .await;
        assert_eq!(answer.code(), Some(400), "{status}: {}", answer.body);
        assert_eq!(
            answer.body["message"],
            json!("a new sprint can only be planned or active")
        );
    }
    api.finish().await;
}

#[tokio::test]
async fn same_status_is_a_noop_without_event() {
    let Some(api) = Api::new("sprint_same_status").await else {
        return;
    };
    api.add_webhook_for(r#"["sprint.started","sprint.completed"]"#).await;
    let sprint = api.new_sprint("noop", json!({})).await;
    assert_eq!(api.set_status(sprint, api.alice, "active").await.code(), Some(0));
    api.payloads("sprint.started", 1).await;
    // Repeating the status is accepted, even for a member, and sends nothing.
    for user in [api.alice, api.bob] {
        let again = api.set_status(sprint, user, "active").await;
        assert_eq!(again.code(), Some(0), "{}", again.body);
    }
    api.payloads("sprint.started", 1).await;
    // Name, description and dates stay editable in every status.
    let edited = api
        .put(
            &format!("/api/v1/sprints/{sprint}"),
            api.bob,
            json!({"description": "edited while active", "end_date": "2031-01-01"}),
        )
        .await;
    assert_eq!(edited.code(), Some(0), "{}", edited.body);
    api.finish().await;
}

#[tokio::test]
async fn second_active_sprint_is_409_naming_the_active_one() {
    let Some(api) = Api::new("sprint_second_active").await else {
        return;
    };
    let first = api.new_sprint("Sprint One", json!({"status": "active"})).await;
    // Create active while another is active.
    let created = api
        .post(
            &api.sprints_uri(),
            api.alice,
            json!({"name": "Sprint Two", "status": "active"}),
        )
        .await;
    assert_eq!(created.code(), Some(409), "{}", created.body);
    let message = created.body["message"].as_str().expect("message");
    assert!(
        message.contains("Sprint One") && message.contains(&first.to_string()),
        "{message}"
    );
    // Start a planned one while another is active.
    let second = api.new_sprint("Sprint Two", json!({})).await;
    let started = api.set_status(second, api.alice, "active").await;
    assert_eq!(started.code(), Some(409), "{}", started.body);
    assert!(
        started.body["message"]
            .as_str()
            .expect("message")
            .contains("Sprint One")
    );
    assert_eq!(api.sprint_status(second).await, "planned");
    // Once the first is finished, the second can start.
    api.set_status(first, api.alice, "completed").await;
    assert_eq!(api.set_status(second, api.alice, "active").await.code(), Some(0));
    api.finish().await;
}

#[tokio::test]
async fn concurrent_starts_leave_one_active() {
    let Some(api) = Api::new("sprint_concurrent_start").await else {
        return;
    };
    let a = api.new_sprint("A", json!({})).await;
    let b = api.new_sprint("B", json!({})).await;
    let (first, second) = tokio::join!(
        api.set_status(a, api.alice, "active"),
        api.set_status(b, api.alice, "active")
    );
    let codes = [first.code(), second.code()];
    assert!(
        codes.contains(&Some(0)) && codes.contains(&Some(409)),
        "exactly one start succeeds: {} / {}",
        first.body,
        second.body
    );
    let active: i64 = api
        .db_count(
            "SELECT COUNT(*)::bigint AS n FROM sprints WHERE project_id = $1 AND status = 'active'",
            api.project,
        )
        .await;
    assert_eq!(active, 1);
    // Two creations as active race the same way.
    api.set_status(if first.code() == Some(0) { a } else { b }, api.alice, "completed")
        .await;
    let uri = api.sprints_uri();
    let creations = tokio::join!(
        api.post(&uri, api.alice, json!({"name": "C", "status": "active"})),
        api.post(&uri, api.alice, json!({"name": "D", "status": "active"}))
    );
    let codes = [creations.0.code(), creations.1.code()];
    assert!(codes.contains(&Some(0)) && codes.contains(&Some(409)), "{codes:?}");
    api.finish().await;
}

#[tokio::test]
async fn member_can_create_and_rename_but_not_change_status_or_delete() {
    let Some(api) = Api::new("sprint_member_permissions").await else {
        return;
    };
    let created = api
        .post(&api.sprints_uri(), api.bob, json!({"name": "by a member"}))
        .await;
    assert_eq!(created.code(), Some(0), "{}", created.body);
    let sprint = Uuid::parse_str(created.data()["id"].as_str().expect("id")).expect("uuid");
    let uri = format!("/api/v1/sprints/{sprint}");
    let renamed = api.put(&uri, api.bob, json!({"name": "renamed by a member"})).await;
    assert_eq!(renamed.code(), Some(0), "{}", renamed.body);
    let started = api.set_status(sprint, api.bob, "active").await;
    assert_eq!(started.code(), Some(403), "{}", started.body);
    assert_eq!(api.sprint_status(sprint).await, "planned");
    let active_at_creation = api
        .post(
            &api.sprints_uri(),
            api.bob,
            json!({"name": "born active", "status": "active"}),
        )
        .await;
    assert_eq!(active_at_creation.code(), Some(403), "{}", active_at_creation.body);
    let deleted = api.call(Method::DELETE, &uri, api.bob, None).await;
    assert_eq!(deleted.code(), Some(403), "{}", deleted.body);
    assert_eq!(api.get(&uri, api.alice).await.code(), Some(0), "still there");
    // An owner may start and delete.
    assert_eq!(api.set_status(sprint, api.alice, "active").await.code(), Some(0));
    api.set_status(sprint, api.alice, "cancelled").await;
    let deleted = api.call(Method::DELETE, &uri, api.alice, None).await;
    assert_eq!(deleted.code(), Some(0), "{}", deleted.body);
    api.finish().await;
}

#[tokio::test]
async fn names_are_unique_ignoring_case() {
    let Some(api) = Api::new("sprint_names").await else {
        return;
    };
    let bug = api.new_sprint("Bug Bash", json!({})).await;
    let twin = api
        .post(&api.sprints_uri(), api.alice, json!({"name": "bug bash"}))
        .await;
    assert_eq!(twin.code(), Some(409), "{}", twin.body);
    let other = api.new_sprint("Other", json!({})).await;
    let renamed = api
        .put(
            &format!("/api/v1/sprints/{other}"),
            api.alice,
            json!({"name": "BUG BASH"}),
        )
        .await;
    assert_eq!(renamed.code(), Some(409), "{}", renamed.body);
    // Renaming a sprint to its own name in another case is its own business.
    let own = api
        .put(
            &format!("/api/v1/sprints/{bug}"),
            api.alice,
            json!({"name": "BUG BASH"}),
        )
        .await;
    assert_eq!(own.code(), Some(0), "{}", own.body);
    api.finish().await;
}

#[tokio::test]
async fn get_sprint_returns_the_sprint_and_404_for_unknown() {
    let Some(api) = Api::new("sprint_get").await else {
        return;
    };
    let sprint = api.new_sprint("readable", json!({"description": "d"})).await;
    let got = api.get(&format!("/api/v1/sprints/{sprint}"), api.bob).await;
    assert_eq!(got.code(), Some(0), "{}", got.body);
    assert_eq!(got.data()["id"], json!(sprint));
    assert_eq!(got.data()["name"], json!("readable"));
    assert_eq!(got.data()["description"], json!("d"));
    let missing = api.get(&format!("/api/v1/sprints/{}", Uuid::new_v4()), api.bob).await;
    assert_eq!(missing.code(), Some(404), "{}", missing.body);
    api.finish().await;
}

#[tokio::test]
async fn get_sprint_is_404_for_non_member() {
    let Some(api) = Api::new("sprint_get_outsider").await else {
        return;
    };
    let sprint = api.new_sprint("private", json!({})).await;
    let got = api.get(&format!("/api/v1/sprints/{sprint}"), api.carol).await;
    assert_eq!(got.code(), Some(404), "{}", got.body);
    api.finish().await;
}
