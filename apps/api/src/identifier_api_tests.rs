//! The short work item identifier (`KEY-n`) in the answers the workspace page and the proposal
//! page read, in the format the item answers use. Drives the real router against a scratch
//! database (skipped without `OPENPR_TEST_DATABASE_URL`).
#![allow(clippy::indexing_slicing, clippy::too_many_lines)]

use crate::item_api_tests::{Api, exec};
use serde_json::json;
use uuid::Uuid;

#[tokio::test]
async fn feed_and_proposal_links_carry_the_item_identifier() {
    let Some(api) = Api::new("identifier_answers").await else {
        return;
    };
    let item = api.create_item("named by key", json!({})).await;
    let shown = api.get(&format!("/api/v1/issues/{item}"), api.alice).await;
    assert_eq!(shown.code(), Some(0), "{}", shown.body);
    let identifier = shown.data()["identifier"].as_str().expect("identifier").to_string();
    assert!(identifier.starts_with("QLB-"), "{identifier}");

    // The workspace page feed.
    let moved = api
        .put(
            &format!("/api/v1/issues/{item}"),
            api.alice,
            json!({"assignee_id": api.bob}),
        )
        .await;
    assert_eq!(moved.code(), Some(0), "{}", moved.body);
    let feed = api.get("/api/v1/my/activities", api.bob).await;
    assert_eq!(feed.code(), Some(0), "{}", feed.body);
    let entries = feed.data()["items"].as_array().expect("items");
    assert!(!entries.is_empty(), "{}", feed.body);
    for entry in entries {
        assert_eq!(entry["identifier"], json!(identifier), "{}", feed.body);
    }

    // The linked work items of a proposal.
    let proposal = Uuid::new_v4().to_string();
    exec(
        &api.db,
        "INSERT INTO proposals (id, title, proposal_type, status, author_id, author_type, content, workspace_id) \
         VALUES ($1, 'a proposal', 'feature', 'approved', $2, 'human', 'body', $3)",
        vec![proposal.clone().into(), api.alice.to_string().into(), api.ws.into()],
    )
    .await;
    exec(
        &api.db,
        "INSERT INTO proposal_issue_links (proposal_id, issue_id, created_at) VALUES ($1, $2, now())",
        vec![proposal.clone().into(), item.into()],
    )
    .await;
    let links = api
        .get(&format!("/api/v1/proposals/{proposal}/issues"), api.alice)
        .await;
    assert_eq!(links.code(), Some(0), "{}", links.body);
    assert_eq!(
        links.data()["items"][0]["identifier"],
        json!(identifier),
        "{}",
        links.body
    );
    assert_eq!(links.data()["items"][0]["issue_id"], json!(item));
    api.finish().await;
}
