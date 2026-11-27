//! The activity log, comments and agent tasks of a work item: every change writes its entry, and
//! the lists come newest first in pages. Real router, scratch `PostgreSQL` database (skipped without
//! `OPENPR_TEST_DATABASE_URL`).
#![allow(clippy::indexing_slicing, clippy::too_many_lines)]

use crate::item_api_tests::{Api, exec};
use axum::http::Method;
use serde_json::{Value, json};

fn actions(list: &Value) -> Vec<String> {
    list["items"]
        .as_array()
        .expect("items")
        .iter()
        .map(|entry| entry["action"].as_str().expect("action").to_string())
        .collect()
}

#[tokio::test]
async fn every_change_of_a_work_item_writes_its_activity() {
    let Some(api) = Api::new("detail_every_activity").await else {
        return;
    };
    let issue = api.create_item("First title", json!({})).await;
    let label = api.create_label("urgent").await;
    let sprint = api.create_sprint("S1").await;

    let edit = api
        .put(
            &format!("/api/v1/issues/{issue}"),
            api.alice,
            json!({
                "title": "Second title",
                "description": "new text",
                "priority": "high",
                "state": "in_progress",
                "assignee_id": api.bob,
                "sprint_id": sprint,
                "due_at": "2030-01-01T00:00:00Z",
            }),
        )
        .await;
    assert_eq!(edit.code(), Some(0), "{}", edit.body);

    let label_uri = format!("/api/v1/issues/{issue}/labels/{label}");
    assert_eq!(api.post(&label_uri, api.alice, json!({})).await.code(), Some(0));
    // Attaching again changes nothing and writes nothing.
    assert_eq!(api.post(&label_uri, api.alice, json!({})).await.code(), Some(0));
    assert_eq!(
        api.call(Method::DELETE, &label_uri, api.alice, None).await.code(),
        Some(0)
    );
    let batch = api
        .post(
            &format!("/api/v1/issues/{issue}/labels/batch"),
            api.alice,
            json!({ "label_ids": [label] }),
        )
        .await;
    assert_eq!(batch.code(), Some(0), "{}", batch.body);

    let comment = api
        .post(
            &format!("/api/v1/issues/{issue}/comments"),
            api.bob,
            json!({ "content": "hello" }),
        )
        .await;
    assert_eq!(comment.code(), Some(0), "{}", comment.body);
    let comment_id = comment.data()["id"].as_str().expect("comment id").to_string();
    let edited = api
        .put(
            &format!("/api/v1/comments/{comment_id}"),
            api.bob,
            json!({ "content": "hello again" }),
        )
        .await;
    assert_eq!(edited.code(), Some(0), "{}", edited.body);
    let deleted = api
        .call(Method::DELETE, &format!("/api/v1/comments/{comment_id}"), api.bob, None)
        .await;
    assert_eq!(deleted.code(), Some(0), "{}", deleted.body);

    let list = api
        .get(&format!("/api/v1/issues/{issue}/activities?per_page=100"), api.alice)
        .await;
    assert_eq!(list.code(), Some(0), "{}", list.body);
    let mut seen = actions(list.data());
    seen.sort();
    let mut expected = vec![
        "created",
        "title_changed",
        "description_changed",
        "priority_changed",
        "status_changed",
        "assigned",
        "sprint_changed",
        "due_date_changed",
        "label_added",
        "label_removed",
        "label_added",
        "comment_added",
        "comment_edited",
        "comment_deleted",
    ];
    expected.sort_unstable();
    assert_eq!(seen, expected, "{}", list.data());

    let labels = api.activity_details(issue, "label_added").await;
    assert_eq!(labels.len(), 2);
    assert_eq!(labels[0]["label_name"], "urgent");
    assert_eq!(labels[0]["label_id"], label.to_string());
    let added = api.activity_details(issue, "comment_added").await;
    assert_eq!(added[0]["comment_id"], comment_id);
    let title = api.activity_details(issue, "title_changed").await;
    assert_eq!(title[0], json!({"from": "First title", "to": "Second title"}));
    api.finish().await;
}

#[tokio::test]
async fn comments_are_newest_first_and_paged() {
    let Some(api) = Api::new("detail_comment_paging").await else {
        return;
    };
    let issue = api.create_item("Talk", json!({})).await;
    for n in 1..=25 {
        let answer = api
            .post(
                &format!("/api/v1/issues/{issue}/comments"),
                api.alice,
                json!({ "content": format!("c{n}") }),
            )
            .await;
        assert_eq!(answer.code(), Some(0), "{}", answer.body);
    }
    let first = api.get(&format!("/api/v1/issues/{issue}/comments"), api.alice).await;
    let data = first.data();
    assert_eq!((data["total"].as_i64(), data["page"].as_i64()), (Some(25), Some(1)));
    assert_eq!(
        (data["per_page"].as_i64(), data["total_pages"].as_i64()),
        (Some(20), Some(2))
    );
    let items = data["items"].as_array().expect("items");
    assert_eq!(items.len(), 20);
    assert_eq!(items[0]["body"], "c25", "the newest comment is first");
    assert_eq!(items[19]["body"], "c6");
    let second = api
        .get(&format!("/api/v1/issues/{issue}/comments?page=2"), api.alice)
        .await;
    let items = second.data()["items"].as_array().expect("items");
    assert_eq!(items.len(), 5);
    assert_eq!(items[0]["body"], "c5");
    assert_eq!(items[4]["body"], "c1");
    api.finish().await;
}

#[tokio::test]
async fn activity_is_newest_first_and_paged_beyond_one_hundred_entries() {
    let Some(api) = Api::new("detail_activity_paging").await else {
        return;
    };
    let issue = api.create_item("Busy", json!({})).await;
    exec(
        &api.db,
        "INSERT INTO activities (id, workspace_id, project_id, issue_id, user_id, action, detail, created_at, \
                resource_type, resource_id, event_type, actor_id, payload) \
         SELECT gen_random_uuid(), $2, $3, $1, $4, 'status_changed', jsonb_build_object('n', n), \
                now() + make_interval(mins => n), 'issue', $1, 'status_changed', $4, '{}'::jsonb \
         FROM generate_series(1, 120) AS n",
        vec![issue.into(), api.ws.into(), api.project.into(), api.alice.into()],
    )
    .await;
    let first = api.get(&format!("/api/v1/issues/{issue}/activities"), api.alice).await;
    let data = first.data();
    assert_eq!(data["per_page"], 30);
    assert_eq!(data["total"], 121, "120 rows and the creation");
    assert_eq!(data["total_pages"], 5);
    let items = data["items"].as_array().expect("items");
    assert_eq!(items.len(), 30);
    assert_eq!(items[0]["detail"]["n"], 120, "the newest entry is first");
    let last = api
        .get(&format!("/api/v1/issues/{issue}/activities?page=5"), api.alice)
        .await;
    let items = last.data()["items"].as_array().expect("items");
    assert_eq!(items.len(), 1);
    assert_eq!(items[0]["action"], "created", "the oldest entry is last");
    api.finish().await;
}
