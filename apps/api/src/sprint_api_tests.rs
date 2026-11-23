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

impl Api {
    /// An item of the sprint in the given workflow state.
    pub(super) async fn sprint_item(&self, title: &str, sprint: Uuid, state: &str) -> Uuid {
        self.create_item(title, json!({"sprint_id": sprint, "state": state}))
            .await
    }

    /// The sprint of each item (`None` for the backlog).
    pub(super) async fn sprint_of(&self, item: Uuid) -> Option<Uuid> {
        let answer = self.get(&format!("/api/v1/issues/{item}"), self.alice).await;
        answer.data()["sprint_id"]
            .as_str()
            .map(|id| Uuid::parse_str(id).expect("uuid"))
    }

    pub(super) async fn complete(&self, sprint: Uuid, extra: Value) -> Answer {
        let mut body = json!({"status": "completed"});
        if let (Some(target), Some(source)) = (body.as_object_mut(), extra.as_object()) {
            for (key, value) in source {
                target.insert(key.clone(), value.clone());
            }
        }
        self.put(&format!("/api/v1/sprints/{sprint}"), self.alice, body).await
    }
}

#[tokio::test]
async fn completing_moves_unfinished_items_to_backlog() {
    let Some(api) = Api::new("sprint_complete_backlog").await else {
        return;
    };
    let sprint = api.new_sprint("S1", json!({"status": "active"})).await;
    let finished = api.sprint_item("done one", sprint, "done").await;
    let open = api.sprint_item("open one", sprint, "todo").await;
    let started = api.sprint_item("started one", sprint, "in_progress").await;

    let answer = api.complete(sprint, json!({})).await;
    assert_eq!(answer.code(), Some(0), "{}", answer.body);
    assert_eq!(answer.data()["rollover"]["moved_count"], json!(2), "{}", answer.body);
    assert!(
        answer.data()["rollover"]["target_sprint_id"].is_null(),
        "{}",
        answer.body
    );
    assert_eq!(api.sprint_of(finished).await, Some(sprint), "the finished item stays");
    assert_eq!(api.sprint_of(open).await, None);
    assert_eq!(api.sprint_of(started).await, None);
    for item in [open, started] {
        let moves = api.activity_details(item, "sprint_changed").await;
        assert_eq!(moves.len(), 1, "{moves:?}");
        assert_eq!(moves[0]["from"], json!(sprint));
        assert!(moves[0]["to"].is_null(), "{moves:?}");
    }
    assert!(api.activity_details(finished, "sprint_changed").await.is_empty());
    // A later read of the sprint has no rollover field.
    let read = api.get(&format!("/api/v1/sprints/{sprint}"), api.alice).await;
    assert!(read.data().get("rollover").is_none(), "{}", read.body);
    api.finish().await;
}

#[tokio::test]
async fn completing_moves_unfinished_items_to_a_planned_sprint() {
    let Some(api) = Api::new("sprint_complete_rollover").await else {
        return;
    };
    let sprint = api.new_sprint("S1", json!({"status": "active"})).await;
    let next = api.new_sprint("S2", json!({})).await;
    let finished = api.sprint_item("done one", sprint, "done").await;
    let open = api.sprint_item("open one", sprint, "todo").await;

    let answer = api.complete(sprint, json!({"rollover_sprint_id": next})).await;
    assert_eq!(answer.code(), Some(0), "{}", answer.body);
    assert_eq!(answer.data()["rollover"]["moved_count"], json!(1), "{}", answer.body);
    assert_eq!(answer.data()["rollover"]["target_sprint_id"], json!(next));
    assert_eq!(api.sprint_of(open).await, Some(next));
    assert_eq!(api.sprint_of(finished).await, Some(sprint));
    let moves = api.activity_details(open, "sprint_changed").await;
    assert_eq!(moves[0]["to"], json!(next), "{moves:?}");
    // An explicit null is the backlog.
    let item = api.sprint_item("another", next, "todo").await;
    api.set_status(next, api.alice, "active").await;
    let answer = api.complete(next, json!({"rollover_sprint_id": null})).await;
    assert_eq!(answer.code(), Some(0), "{}", answer.body);
    assert_eq!(answer.data()["rollover"]["moved_count"], json!(2), "{}", answer.body);
    assert_eq!(api.sprint_of(item).await, None);
    api.finish().await;
}

#[tokio::test]
async fn rollover_to_an_active_or_foreign_sprint_is_400() {
    let Some(api) = Api::new("sprint_rollover_refused").await else {
        return;
    };
    let sprint = api.new_sprint("S1", json!({"status": "active"})).await;
    let item = api.sprint_item("stays", sprint, "todo").await;
    let completed = api.new_sprint("old", json!({})).await;
    api.set_status(completed, api.alice, "cancelled").await;
    // A sprint of another project.
    let other_project = Uuid::new_v4();
    super::item_api_tests::exec(
        &api.db,
        "INSERT INTO projects (id, workspace_id, key, name, created_by, type_key) \
         VALUES ($1, $2, 'OTH', 'other', $3, 'code_project')",
        vec![other_project.into(), api.ws.into(), api.alice.into()],
    )
    .await;
    let foreign = Uuid::new_v4();
    super::item_api_tests::exec(
        &api.db,
        "INSERT INTO sprints (id, project_id, name) VALUES ($1, $2, 'foreign')",
        vec![foreign.into(), other_project.into()],
    )
    .await;
    for (label, target) in [
        ("itself (active)", sprint),
        ("a cancelled sprint", completed),
        ("a foreign sprint", foreign),
        ("an unknown sprint", Uuid::new_v4()),
    ] {
        let answer = api.complete(sprint, json!({"rollover_sprint_id": target})).await;
        assert_eq!(answer.code(), Some(400), "{label}: {}", answer.body);
        assert_eq!(
            answer.body["message"],
            json!("rollover sprint must be another planned sprint of the same project")
        );
    }
    // Nothing happened: still active, the item still in it.
    assert_eq!(api.sprint_status(sprint).await, "active");
    assert_eq!(api.sprint_of(item).await, Some(sprint));
    api.finish().await;
}

#[tokio::test]
async fn rollover_without_completion_is_400() {
    let Some(api) = Api::new("sprint_rollover_alone").await else {
        return;
    };
    let sprint = api.new_sprint("S1", json!({})).await;
    let next = api.new_sprint("S2", json!({})).await;
    let uri = format!("/api/v1/sprints/{sprint}");
    for body in [
        json!({"rollover_sprint_id": next}),
        json!({"name": "renamed", "rollover_sprint_id": next}),
        json!({"status": "active", "rollover_sprint_id": next}),
    ] {
        let answer = api.put(&uri, api.alice, body.clone()).await;
        assert_eq!(answer.code(), Some(400), "{body}: {}", answer.body);
        assert_eq!(
            answer.body["message"],
            json!("rollover_sprint_id is only valid when completing or cancelling a sprint")
        );
    }
    assert_eq!(api.sprint_status(sprint).await, "planned");
    api.finish().await;
}

#[tokio::test]
async fn cancelling_moves_unfinished_items() {
    let Some(api) = Api::new("sprint_cancel_moves").await else {
        return;
    };
    let planned = api.new_sprint("planned one", json!({})).await;
    let next = api.new_sprint("next", json!({})).await;
    let open = api.sprint_item("open", planned, "todo").await;
    let finished = api.sprint_item("finished", planned, "done").await;
    let answer = api
        .put(
            &format!("/api/v1/sprints/{planned}"),
            api.alice,
            json!({"status": "cancelled", "rollover_sprint_id": next}),
        )
        .await;
    assert_eq!(answer.code(), Some(0), "{}", answer.body);
    assert_eq!(answer.data()["rollover"]["moved_count"], json!(1), "{}", answer.body);
    assert_eq!(api.sprint_of(open).await, Some(next));
    assert_eq!(api.sprint_of(finished).await, Some(planned));
    // An active sprint cancelled without a target sends its unfinished items to the backlog.
    api.set_status(next, api.alice, "active").await;
    let answer = api.set_status(next, api.alice, "cancelled").await;
    assert_eq!(answer.code(), Some(0), "{}", answer.body);
    assert_eq!(answer.data()["rollover"]["moved_count"], json!(1), "{}", answer.body);
    assert_eq!(api.sprint_of(open).await, None);
    api.finish().await;
}

#[tokio::test]
async fn finished_means_terminal_or_done_or_completed_category() {
    let Some(api) = Api::new("sprint_finished_rule").await else {
        return;
    };
    // A project workflow with a `completed`-category state, a terminal state in the `active`
    // category and two states that are not finished.
    let workflow = Uuid::new_v4();
    super::item_api_tests::exec(
        &api.db,
        "INSERT INTO workflows (id, workspace_id, name) VALUES ($1, $2, 'custom')",
        vec![workflow.into(), api.ws.into()],
    )
    .await;
    for (position, (key, category, initial, terminal)) in [
        ("open", "active", true, false),
        ("doing", "active", false, false),
        ("shipped", "completed", false, false),
        ("dropped", "active", false, true),
        ("accepted", "done", false, false),
    ]
    .into_iter()
    .enumerate()
    {
        super::item_api_tests::exec(
            &api.db,
            "INSERT INTO workflow_states (workflow_id, key, display_name, category, position, is_initial, is_terminal) \
             VALUES ($1, $2, $2, $3, $4, $5, $6)",
            vec![
                workflow.into(),
                key.into(),
                category.into(),
                i32::try_from(position + 1).expect("small").into(),
                initial.into(),
                terminal.into(),
            ],
        )
        .await;
    }
    super::item_api_tests::exec(
        &api.db,
        "UPDATE projects SET workflow_id = $1 WHERE id = $2",
        vec![workflow.into(), api.project.into()],
    )
    .await;
    let sprint = api.new_sprint("S1", json!({"status": "active"})).await;
    let mut stays = Vec::new();
    for state in ["shipped", "dropped", "accepted"] {
        stays.push(api.sprint_item(state, sprint, state).await);
    }
    let moves = [
        api.sprint_item("open", sprint, "open").await,
        api.sprint_item("doing", sprint, "doing").await,
    ];
    let answer = api.complete(sprint, json!({})).await;
    assert_eq!(answer.code(), Some(0), "{}", answer.body);
    assert_eq!(answer.data()["rollover"]["moved_count"], json!(2), "{}", answer.body);
    for item in stays {
        assert_eq!(api.sprint_of(item).await, Some(sprint));
    }
    for item in moves {
        assert_eq!(api.sprint_of(item).await, None);
    }
    api.finish().await;
}

#[tokio::test]
async fn deleting_a_sprint_moves_items_with_activity() {
    let Some(api) = Api::new("sprint_delete_moves").await else {
        return;
    };
    let sprint = api.new_sprint("doomed", json!({})).await;
    let first = api.sprint_item("first", sprint, "todo").await;
    let finished = api.sprint_item("finished", sprint, "done").await;
    let deleted = api
        .call(Method::DELETE, &format!("/api/v1/sprints/{sprint}"), api.alice, None)
        .await;
    assert_eq!(deleted.code(), Some(0), "{}", deleted.body);
    // Every item, finished or not, leaves a sprint that no longer exists.
    for item in [first, finished] {
        assert_eq!(api.sprint_of(item).await, None);
        let moves = api.activity_details(item, "sprint_changed").await;
        assert_eq!(moves.len(), 1, "{moves:?}");
        assert_eq!(moves[0]["from"], json!(sprint));
        assert!(moves[0]["to"].is_null());
    }
    api.finish().await;
}

#[tokio::test]
async fn completion_emits_sprint_completed_with_rollover_then_issue_updated_per_item() {
    let Some(api) = Api::new("sprint_complete_events").await else {
        return;
    };
    api.add_webhook_for(r#"["sprint.completed","issue.updated"]"#).await;
    let sprint = api.new_sprint("S1", json!({"status": "active"})).await;
    let next = api.new_sprint("S2", json!({})).await;
    let first = api.sprint_item("first", sprint, "todo").await;
    let second = api.sprint_item("second", sprint, "in_progress").await;
    api.sprint_item("done", sprint, "done").await;

    let answer = api.complete(sprint, json!({"rollover_sprint_id": next})).await;
    assert_eq!(answer.code(), Some(0), "{}", answer.body);

    let completed = api.payloads("sprint.completed", 1).await;
    assert_eq!(
        completed[0]["data"]["rollover"],
        json!({"moved_count": 2, "target_sprint_id": next}),
        "{}",
        completed[0]
    );
    assert_eq!(completed[0]["data"]["sprint"]["status"], json!("completed"));
    let updated = api.payloads("issue.updated", 2).await;
    let mut moved: Vec<(String, Value)> = updated
        .iter()
        .map(|payload| {
            (
                payload["data"]["issue"]["id"].as_str().expect("id").to_string(),
                payload["data"]["changes"]["sprint_id"].clone(),
            )
        })
        .collect();
    moved.sort_by(|a, b| a.0.cmp(&b.0));
    let mut expected = vec![
        (first.to_string(), json!({"old": sprint, "new": next})),
        (second.to_string(), json!({"old": sprint, "new": next})),
    ];
    expected.sort_by(|a, b| a.0.cmp(&b.0));
    assert_eq!(moved, expected);
    // Starting the next sprint sends no completion and no item event.
    api.finish().await;
}
