//! The two reads that tell a caller which project owns a comment or a form attachment
//! (`GET /comments/{id}`, `GET /form-attachments/{id}`), through the real router. Uses the fixture
//! of `item_api_tests` (`alice` owns the workspace, `carol` belongs to another workspace) against
//! a scratch `PostgreSQL` (skipped without `OPENPR_TEST_DATABASE_URL`).
#![allow(clippy::indexing_slicing)]

use crate::item_api_tests::{Api, exec};
use axum::http::Method;
use serde_json::json;
use uuid::Uuid;

#[tokio::test]
async fn a_comment_is_readable_with_its_project() {
    let Some(api) = Api::new("owner_comment").await else {
        return;
    };
    let item = api.create_item("discussed", json!({})).await;
    let made = api
        .post(
            &format!("/api/v1/issues/{item}/comments"),
            api.bob,
            json!({"content": "first words"}),
        )
        .await;
    assert_eq!(made.code(), Some(0), "{}", made.body);
    let comment = made.data()["id"].as_str().expect("id").to_string();

    let answer = api.get(&format!("/api/v1/comments/{comment}"), api.alice).await;
    assert_eq!(answer.code(), Some(0), "{}", answer.body);
    let data = answer.data();
    assert_eq!(data["id"], json!(comment));
    assert_eq!(data["work_item_id"], json!(item));
    assert_eq!(data["project_id"], json!(api.project));
    assert_eq!(data["author_id"], json!(api.bob));
    assert_eq!(data["author_name"], json!("bob"));
    assert_eq!(data["content"], json!("first words"));
    assert!(
        data["created_at"].is_string() && data["updated_at"].is_string(),
        "{data}"
    );
    assert!(data.get("author_email").is_none(), "{data}");

    // The edit shows in the next read.
    let edited = api
        .put(
            &format!("/api/v1/comments/{comment}"),
            api.bob,
            json!({"content": "edited"}),
        )
        .await;
    assert_eq!(edited.code(), Some(0), "{}", edited.body);
    let again = api.get(&format!("/api/v1/comments/{comment}"), api.bob).await;
    assert_eq!(again.data()["content"], json!("edited"), "{}", again.body);

    // Unknown id and a person of another workspace.
    let missing = api
        .get(&format!("/api/v1/comments/{}", Uuid::new_v4()), api.alice)
        .await;
    assert_eq!(missing.code(), Some(404), "{}", missing.body);
    let outsider = api.get(&format!("/api/v1/comments/{comment}"), api.carol).await;
    assert_eq!(outsider.code(), Some(404), "{}", outsider.body);
    assert!(!outsider.body.to_string().contains("first words"), "{}", outsider.body);
    api.finish().await;
}

#[tokio::test]
async fn a_form_attachment_is_readable_with_its_form_and_project() {
    let Some(api) = Api::new("owner_attachment").await else {
        return;
    };
    let made = api
        .post(
            &format!("/api/v1/projects/{}/forms", api.project),
            api.alice,
            json!({"key": "intake", "name": "Intake"}),
        )
        .await;
    assert_eq!(made.code(), Some(0), "{}", made.body);
    let form = Uuid::parse_str(made.data()["id"].as_str().expect("id")).expect("uuid");
    let attachment = Uuid::new_v4();
    exec(
        &api.db,
        "INSERT INTO form_attachments (id, workspace_id, project_id, form_id, field_id, field_key, file_name, storage_key) \
         VALUES ($1, $2, $3, $4, 'fld_files', 'files', 'plan.pdf', 'objects/plan.pdf')",
        vec![
            attachment.into(),
            api.ws.into(),
            api.project.into(),
            form.into(),
        ],
    )
    .await;

    let answer = api
        .get(&format!("/api/v1/form-attachments/{attachment}"), api.bob)
        .await;
    assert_eq!(answer.code(), Some(0), "{}", answer.body);
    let data = answer.data();
    assert_eq!(data["id"], json!(attachment));
    assert_eq!(data["form_id"], json!(form));
    assert_eq!(data["project_id"], json!(api.project));
    assert_eq!(data["workspace_id"], json!(api.ws));
    assert_eq!(data["file_name"], json!("plan.pdf"));
    assert!(data["archived_at"].is_null(), "{data}");

    // An archived attachment stays readable: it is the one a restore has to resolve.
    let archived = api
        .call(
            Method::DELETE,
            &format!("/api/v1/form-attachments/{attachment}"),
            api.alice,
            None,
        )
        .await;
    assert_eq!(archived.code(), Some(0), "{}", archived.body);
    let after = api
        .get(&format!("/api/v1/form-attachments/{attachment}"), api.alice)
        .await;
    assert_eq!(after.code(), Some(0), "{}", after.body);
    assert!(after.data()["archived_at"].is_string(), "{}", after.body);

    let missing = api
        .get(&format!("/api/v1/form-attachments/{}", Uuid::new_v4()), api.alice)
        .await;
    assert_eq!(missing.code(), Some(404), "{}", missing.body);
    let outsider = api
        .get(&format!("/api/v1/form-attachments/{attachment}"), api.carol)
        .await;
    assert_eq!(outsider.code(), Some(404), "{}", outsider.body);
    assert!(!outsider.body.to_string().contains("plan.pdf"), "{}", outsider.body);
    api.finish().await;
}
