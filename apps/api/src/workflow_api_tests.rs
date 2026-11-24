//! Workflow state positions through the real router: an insert or a move into a taken position
//! shifts the others, an out-of-range position appends and a negative one is a 400, never a 500.
//! Uses the fixture of `item_api_tests` against a scratch `PostgreSQL` (skipped without
//! `OPENPR_TEST_DATABASE_URL`).
#![allow(clippy::indexing_slicing)]

use crate::item_api_tests::{Answer, Api};
use serde_json::{Value, json};
use uuid::Uuid;

/// Creates a custom workflow as alice; it starts with `backlog`, `todo`, `in_progress` and `done` at 0..=3.
async fn new_workflow(api: &Api) -> Uuid {
    let made = api
        .post(
            &format!("/api/v1/workspaces/{}/workflows", api.ws),
            api.alice,
            json!({"name": "custom"}),
        )
        .await;
    assert_eq!(made.code(), Some(0), "create workflow: {}", made.body);
    Uuid::parse_str(made.data()["id"].as_str().expect("id")).expect("uuid")
}

async fn add_state(api: &Api, workflow: Uuid, key: &str, position: Option<Value>) -> Answer {
    let mut body = json!({"key": key, "display_name": key});
    if let Some(position) = position {
        body["position"] = position;
    }
    api.post(&format!("/api/v1/workflows/{workflow}/states"), api.alice, body)
        .await
}

/// `(key, id, position)` of the workflow's states in position order.
async fn layout(api: &Api, workflow: Uuid) -> Vec<(String, String, i64)> {
    let answer = api.get(&format!("/api/v1/workflows/{workflow}"), api.alice).await;
    assert_eq!(answer.code(), Some(0), "{}", answer.body);
    let mut states: Vec<(String, String, i64)> = answer.data()["states"]
        .as_array()
        .expect("states")
        .iter()
        .map(|s| {
            (
                s["key"].as_str().expect("key").to_string(),
                s["id"].as_str().expect("id").to_string(),
                s["position"].as_i64().expect("position"),
            )
        })
        .collect();
    states.sort_by_key(|(_, _, position)| *position);
    states
}

fn keys(layout: &[(String, String, i64)]) -> Vec<&str> {
    layout.iter().map(|(key, _, _)| key.as_str()).collect()
}

fn contiguous(layout: &[(String, String, i64)]) -> bool {
    layout
        .iter()
        .enumerate()
        .all(|(index, (_, _, position))| i64::try_from(index).is_ok_and(|index| index == *position))
}

fn id_of(layout: &[(String, String, i64)], key: &str) -> String {
    layout
        .iter()
        .find(|(k, _, _)| k == key)
        .map(|(_, id, _)| id.clone())
        .expect("state exists")
}

#[tokio::test]
async fn creating_at_a_taken_position_shifts_the_others() {
    let Some(api) = Api::new("wf_create_shift").await else {
        return;
    };
    let workflow = new_workflow(&api).await;
    let inserted = add_state(&api, workflow, "x", Some(json!(1))).await;
    assert_eq!(inserted.code(), Some(0), "{}", inserted.body);
    assert_eq!(inserted.data()["position"], json!(1));
    let after = layout(&api, workflow).await;
    assert_eq!(keys(&after), ["backlog", "x", "todo", "in_progress", "done"]);
    assert!(contiguous(&after), "{after:?}");
    let first = add_state(&api, workflow, "front", Some(json!(0))).await;
    assert_eq!(first.data()["position"], json!(0), "{}", first.body);
    // Past the end, and no position at all, append.
    let appended = add_state(&api, workflow, "z", Some(json!(99))).await;
    assert_eq!(appended.data()["position"], json!(6), "{}", appended.body);
    let plain = add_state(&api, workflow, "y", None).await;
    assert_eq!(plain.data()["position"], json!(7), "{}", plain.body);
    let after = layout(&api, workflow).await;
    assert_eq!(
        keys(&after),
        ["front", "backlog", "x", "todo", "in_progress", "done", "z", "y"]
    );
    assert!(contiguous(&after), "{after:?}");
    api.finish().await;
}

#[tokio::test]
async fn updating_to_a_taken_position_moves_the_state() {
    let Some(api) = Api::new("wf_update_move").await else {
        return;
    };
    let workflow = new_workflow(&api).await;
    let start = layout(&api, workflow).await;
    let moved = api
        .put(
            &format!("/api/v1/workflow-states/{}", id_of(&start, "backlog")),
            api.alice,
            json!({"position": 2}),
        )
        .await;
    assert_eq!(moved.code(), Some(0), "{}", moved.body);
    assert_eq!(moved.data()["position"], json!(2));
    let after = layout(&api, workflow).await;
    assert_eq!(keys(&after), ["todo", "in_progress", "backlog", "done"]);
    assert!(contiguous(&after), "{after:?}");
    let back = api
        .put(
            &format!("/api/v1/workflow-states/{}", id_of(&start, "done")),
            api.alice,
            json!({"position": 0}),
        )
        .await;
    assert_eq!(back.code(), Some(0), "{}", back.body);
    let after = layout(&api, workflow).await;
    assert_eq!(keys(&after), ["done", "todo", "in_progress", "backlog"]);
    assert!(contiguous(&after), "{after:?}");
    // Past the end lands last; the other fields of the request still apply.
    let last = api
        .put(
            &format!("/api/v1/workflow-states/{}", id_of(&start, "done")),
            api.alice,
            json!({"position": 50, "display_name": "Finished"}),
        )
        .await;
    assert_eq!(last.data()["position"], json!(3), "{}", last.body);
    assert_eq!(last.data()["display_name"], json!("Finished"));
    let after = layout(&api, workflow).await;
    assert_eq!(keys(&after), ["todo", "in_progress", "backlog", "done"]);
    assert!(contiguous(&after), "{after:?}");
    api.finish().await;
}

#[tokio::test]
async fn negative_position_is_400() {
    let Some(api) = Api::new("wf_negative").await else {
        return;
    };
    let workflow = new_workflow(&api).await;
    let refused = add_state(&api, workflow, "b", Some(json!(-1))).await;
    assert_eq!(refused.code(), Some(400), "{}", refused.body);
    assert_eq!(refused.body["message"], json!("position must be 0 or greater"));
    let start = layout(&api, workflow).await;
    let refused = api
        .put(
            &format!("/api/v1/workflow-states/{}", id_of(&start, "todo")),
            api.alice,
            json!({"position": -5}),
        )
        .await;
    assert_eq!(refused.code(), Some(400), "{}", refused.body);
    assert_eq!(layout(&api, workflow).await, start, "a refused request changes nothing");
    api.finish().await;
}

#[tokio::test]
async fn no_500_on_any_position() {
    let Some(api) = Api::new("wf_no_500").await else {
        return;
    };
    let workflow = new_workflow(&api).await;
    for (n, position) in [0, 0, 1, i32::MAX, i32::MIN, -1, 7].into_iter().enumerate() {
        let added = add_state(&api, workflow, &format!("s{n}"), Some(json!(position))).await;
        assert!(
            matches!(added.code(), Some(0 | 400)),
            "create at {position}: {}",
            added.body
        );
    }
    let all = layout(&api, workflow).await;
    let id = id_of(&all, "todo");
    for position in [0, 3, i32::MAX, i32::MIN, -1, 0, 2] {
        let moved = api
            .put(
                &format!("/api/v1/workflow-states/{id}"),
                api.alice,
                json!({"position": position}),
            )
            .await;
        assert!(
            matches!(moved.code(), Some(0 | 400)),
            "update to {position}: {}",
            moved.body
        );
    }
    let after = layout(&api, workflow).await;
    assert_eq!(after.len(), all.len());
    assert!(contiguous(&after), "{after:?}");
    api.finish().await;
}
