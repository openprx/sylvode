//! Project export and import through the real router: the document exported from one project is
//! imported into another, and the answers, refusals and permissions are checked. Uses the fixture
//! of `item_api_tests` (`alice` owns the workspace, `bob` is a member, `carol` is outside it)
//! against a scratch `PostgreSQL` (skipped without `OPENPR_TEST_DATABASE_URL`).
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

impl Api {
    pub(super) fn import_uri(&self) -> String {
        format!("/api/v1/workspaces/{}/import/project", self.ws)
    }

    /// Another project of the workspace, with `existing` items already in it.
    pub(super) async fn second_project(&self, key: &str, existing: usize) -> Uuid {
        let project = Uuid::new_v4();
        exec(
            &self.db,
            "INSERT INTO projects (id, workspace_id, key, name, created_by, type_key) \
             VALUES ($1, $2, $3, 'destination', $4, 'code_project')",
            vec![
                project.into(),
                self.ws.into(),
                key.to_string().into(),
                self.alice.into(),
            ],
        )
        .await;
        for index in 0..existing {
            self.post(
                &format!("/api/v1/projects/{project}/issues"),
                self.alice,
                json!({"title": format!("already here {index}")}),
            )
            .await;
        }
        project
    }

    /// The exported JSON document of `project`.
    pub(super) async fn export_document(&self, project: Uuid) -> Value {
        let answer = self
            .get(&format!("/api/v1/export/project/{project}?format=json"), self.alice)
            .await;
        assert_eq!(answer.code(), Some(0), "{}", answer.body);
        assert_eq!(answer.data()["format"], json!("json"));
        serde_json::from_str(answer.data()["content"].as_str().expect("content")).expect("a json document")
    }

    /// `(title, state, priority, due_at)` of every item of `project`, by sequence.
    pub(super) async fn items_of(&self, project: Uuid) -> Vec<Value> {
        let answer = self
            .get(
                &format!("/api/v1/projects/{project}/issues?per_page=100&sort_by=created_at&sort_order=asc"),
                self.alice,
            )
            .await;
        let mut items = answer.data()["items"].as_array().expect("items").clone();
        items.sort_by_key(|item| {
            item["identifier"]
                .as_str()
                .unwrap_or_default()
                .rsplit('-')
                .next()
                .and_then(|n| n.parse::<i64>().ok())
        });
        items
    }
}

/// A source project with items in several states, labels, comments, a sprint and assignees.
async fn populated(api: &Api) -> Uuid {
    let sprint = api.create_sprint("s1").await;
    let label = api.label_id("tag", "#112233").await;
    let backlog = api
        .create_item(
            "Plan the \"launch\"",
            json!({"description": "line one\nline two, with a comma", "priority": "low"}),
        )
        .await;
    let doing = api
        .create_item(
            "Build it",
            json!({"state": "in_progress", "priority": "high", "assignee_id": api.bob, "sprint_id": sprint, "due_at": "2030-03-04T00:00:00Z"}),
        )
        .await;
    let done = api
        .create_item(
            "Ship it",
            json!({"state": "done", "priority": "urgent", "due_at": "2030-05-06T00:00:00Z"}),
        )
        .await;
    api.post(&format!("/api/v1/issues/{doing}/labels/{label}"), api.alice, json!({}))
        .await;
    for (item, user, body) in [
        (backlog, api.alice, "first thoughts"),
        (doing, api.bob, "on it"),
        (doing, api.alice, "thanks, \"great\""),
        (done, api.bob, "released"),
    ] {
        let answer = api
            .post(&format!("/api/v1/issues/{item}/comments"), user, json!({"body": body}))
            .await;
        assert_eq!(answer.code(), Some(0), "{}", answer.body);
    }
    // Fixed, distinct comment timestamps, so the round trip can show they are kept.
    for (index, body) in ["first thoughts", "on it", "thanks, \"great\"", "released"]
        .iter()
        .enumerate()
    {
        exec(
            &api.db,
            "UPDATE comments SET created_at = $1::timestamptz, updated_at = $1::timestamptz WHERE body = $2",
            vec![
                format!("2030-02-0{}T10:00:00Z", index + 1).into(),
                (*body).to_string().into(),
            ],
        )
        .await;
    }
    api.project
}

#[tokio::test]
async fn export_json_round_trips_into_another_project() {
    let Some(api) = Api::new("export_round_trip").await else {
        return;
    };
    let source = populated(&api).await;
    let document = api.export_document(source).await;
    assert_eq!(document["format"], json!("sylvode.project_export"));
    assert_eq!(document["version"], json!(1));
    assert_eq!(document["project"]["key"], json!("QLB"));
    assert_eq!(document["issues"].as_array().map(Vec::len), Some(3));
    assert_eq!(document["comments"].as_array().map(Vec::len), Some(4));
    // Assignees, labels and sprints are not part of the document.
    let text = document.to_string();
    for absent in ["assignee", "sprint", "labels", "tag"] {
        assert!(!text.contains(absent), "{absent} is in the document");
    }

    // Import into a project that already has two items, so every identifier is new.
    let target = api.second_project("DST", 2).await;
    let answer = api
        .post(
            &api.import_uri(),
            api.alice,
            json!({"project_key": "DST", "document": document}),
        )
        .await;
    assert_eq!(answer.code(), Some(0), "{}", answer.body);
    assert_eq!(answer.data()["issues_created"], json!(3));
    assert_eq!(answer.data()["comments_created"], json!(4));
    assert_eq!(answer.data()["project_key"], json!("DST"));
    assert_eq!(
        answer.data()["identifiers"],
        json!({"QLB-1": "DST-3", "QLB-2": "DST-4", "QLB-3": "DST-5"}),
        "{}",
        answer.body
    );

    let source_items = api.items_of(source).await;
    let target_items: Vec<Value> = api.items_of(target).await.into_iter().skip(2).collect();
    assert_eq!(target_items.len(), 3);
    for (from, to) in source_items.iter().zip(&target_items) {
        for field in ["title", "description", "state", "priority", "due_at"] {
            assert_eq!(from[field], to[field], "{field} of {}", from["identifier"]);
        }
        // What the document does not carry starts empty.
        assert!(to["assignee_id"].is_null(), "{to}");
        assert!(to["sprint_id"].is_null(), "{to}");
        assert_eq!(to["labels"], json!([]), "{to}");
        assert_eq!(to["created_by"], json!(api.alice));
    }
    // The comments: same bodies and original timestamps, authored by the importer.
    for (from, to) in source_items.iter().zip(&target_items) {
        let read = |item: &Value| {
            let id = item["id"].as_str().expect("id").to_string();
            let api = &api;
            async move {
                let answer = api.get(&format!("/api/v1/issues/{id}/comments"), api.alice).await;
                let mut comments: Vec<(String, String)> = answer.data()["items"]
                    .as_array()
                    .expect("items")
                    .iter()
                    .map(|c| {
                        (
                            c["content"]
                                .as_str()
                                .or_else(|| c["body"].as_str())
                                .expect("body")
                                .to_string(),
                            c["created_at"].as_str().expect("created_at").to_string(),
                        )
                    })
                    .collect();
                comments.sort();
                comments
            }
        };
        let (source_comments, target_comments) = (read(from).await, read(to).await);
        assert_eq!(source_comments, target_comments, "comments of {}", from["identifier"]);
    }
    let authors = api
        .db_strings(&format!(
            "SELECT DISTINCT author_id::text AS v FROM comments c JOIN work_items wi ON wi.id = c.work_item_id \
             WHERE wi.project_id = '{target}'"
        ))
        .await;
    assert_eq!(authors, [api.alice.to_string()]);

    // The export of the target is the same content again (the first two items were already there).
    let again = api.export_document(target).await;
    assert_eq!(again["issues"].as_array().map(Vec::len), Some(5));
    // Imported rows send no events and no notifications.
    api.finish().await;
}

#[tokio::test]
async fn export_csv_escapes_quotes_in_every_field() {
    let Some(api) = Api::new("export_csv").await else {
        return;
    };
    populated(&api).await;
    let answer = api
        .get(&format!("/api/v1/export/project/{}?format=csv", api.project), api.alice)
        .await;
    assert_eq!(answer.code(), Some(0), "{}", answer.body);
    assert_eq!(answer.data()["format"], json!("csv"));
    assert_eq!(answer.data()["filename"], json!("project_QLB_export.csv"));
    let content = answer.data()["content"].as_str().expect("content");
    let mut lines = content.lines();
    assert_eq!(
        lines.next(),
        Some("Identifier,Title,State,Priority,Due,Description,Created At,Updated At")
    );
    // The title has quotes, the description a newline and a comma: every field is quoted and a
    // quote inside is doubled, so the row still has eight fields.
    assert!(
        content.contains(
            "\"QLB-1\",\"Plan the \"\"launch\"\"\",\"backlog\",\"low\",\"\",\"line one\nline two, with a comma\","
        ),
        "{content}"
    );
    let first_row_fields = content
        .split("\"QLB-2\"")
        .next()
        .expect("rows")
        .matches("\",\"")
        .count();
    assert!(first_row_fields >= 7, "{content}");
    assert!(
        content.contains("\"QLB-2\",\"Build it\",\"in_progress\",\"high\",\"2030-03-04T00:00:00+00:00\""),
        "{content}"
    );
    let bad = api
        .get(&format!("/api/v1/export/project/{}?format=xml", api.project), api.alice)
        .await;
    assert_eq!(bad.code(), Some(400), "{}", bad.body);
    api.finish().await;
}

#[tokio::test]
async fn export_is_for_members_of_the_workspace() {
    let Some(api) = Api::new("export_access").await else {
        return;
    };
    api.create_item("secret", json!({})).await;
    let outsider = api
        .get(
            &format!("/api/v1/export/project/{}?format=json", api.project),
            api.carol,
        )
        .await;
    assert_ne!(outsider.code(), Some(0), "{}", outsider.body);
    assert!(!outsider.body.to_string().contains("secret"));
    let member = api
        .get(&format!("/api/v1/export/project/{}?format=json", api.project), api.bob)
        .await;
    assert_eq!(member.code(), Some(0), "{}", member.body);
    let missing = api
        .get(
            &format!("/api/v1/export/project/{}?format=json", Uuid::new_v4()),
            api.alice,
        )
        .await;
    assert_eq!(missing.code(), Some(404), "{}", missing.body);
    api.finish().await;
}

#[tokio::test]
async fn import_refuses_a_state_outside_the_workflow_and_writes_nothing() {
    let Some(api) = Api::new("import_refuses_state").await else {
        return;
    };
    let target = api.second_project("DST", 0).await;
    let document = json!({
        "format": "sylvode.project_export", "version": 1,
        "issues": [
            {"identifier": "X-1", "title": "fine", "state": "todo"},
            {"identifier": "X-2", "title": "bad state", "state": "limbo"},
            {"identifier": "X-3", "title": "bad priority", "state": "todo", "priority": "huge"}
        ],
        "comments": [{"issue_identifier": "X-1", "body": "ok", "created_at": "2030-01-01T00:00:00Z"}]
    });
    let answer = api
        .post(
            &api.import_uri(),
            api.alice,
            json!({"project_key": "DST", "document": document}),
        )
        .await;
    assert_eq!(answer.code(), Some(400), "{}", answer.body);
    assert_eq!(
        answer.body["message"],
        json!("import refused: 2 problems"),
        "{}",
        answer.body
    );
    let errors = answer.body["details"]["errors"].as_array().expect("details.errors");
    assert_eq!(errors.len(), 2, "{}", answer.body);
    assert!(errors[0].as_str().expect("text").contains("limbo"));
    assert_eq!(
        api.db_count(
            "SELECT COUNT(*)::bigint AS n FROM work_items WHERE project_id = $1",
            target
        )
        .await,
        0,
        "the valid issue was not written either"
    );
    assert_eq!(api.db_count("SELECT COUNT(*)::bigint AS n FROM comments c JOIN work_items wi ON wi.id = c.work_item_id WHERE wi.project_id = $1", target).await, 0);
    api.finish().await;
}

#[tokio::test]
async fn owner_and_admin_may_import_member_may_not() {
    let Some(api) = Api::new("import_permissions").await else {
        return;
    };
    let target = api.second_project("DST", 0).await;
    let one = |title: &str| {
        json!({"project_key": "DST", "document": {"format": "sylvode.project_export", "version": 1,
            "issues": [{"identifier": "A-1", "title": title, "state": "todo"}], "comments": []}})
    };
    let member = api.post(&api.import_uri(), api.bob, one("by a member")).await;
    assert_eq!(member.code(), Some(403), "{}", member.body);
    assert_eq!(member.body["message"], json!("only owners and admins can import"));
    let owner = api.post(&api.import_uri(), api.alice, one("by the owner")).await;
    assert_eq!(owner.code(), Some(0), "{}", owner.body);
    exec(
        &api.db,
        "UPDATE workspace_members SET role = 'admin' WHERE user_id = $1",
        vec![api.bob.into()],
    )
    .await;
    let admin = api.post(&api.import_uri(), api.bob, one("by an admin")).await;
    assert_eq!(admin.code(), Some(0), "{}", admin.body);
    assert_eq!(
        api.db_count(
            "SELECT COUNT(*)::bigint AS n FROM work_items WHERE project_id = $1",
            target
        )
        .await,
        2
    );
    // Someone outside the workspace is told nothing.
    let outsider = api.post(&api.import_uri(), api.carol, one("by a stranger")).await;
    assert_ne!(outsider.code(), Some(0), "{}", outsider.body);
    // An unknown project is a 404, and the old project-creating form is gone.
    let unknown = api
        .post(
            &api.import_uri(),
            api.alice,
            json!({"project_key": "NOPE", "document": {"format": "sylvode.project_export", "version": 1}}),
        )
        .await;
    assert_eq!(unknown.code(), Some(404), "{}", unknown.body);
    let legacy = api
        .post(
            &api.import_uri(),
            api.alice,
            json!({"project_name": "New", "issues": []}),
        )
        .await;
    assert!(
        !legacy.status.is_success() || legacy.code() != Some(0),
        "{}",
        legacy.body
    );
    api.finish().await;
}

#[tokio::test]
async fn import_refuses_an_unknown_format() {
    let Some(api) = Api::new("import_unknown_format").await else {
        return;
    };
    let target = api.second_project("DST", 0).await;
    for document in [
        json!({"format": "other", "version": 1, "issues": []}),
        json!({"format": "sylvode.project_export", "version": 2, "issues": []}),
    ] {
        let answer = api
            .post(
                &api.import_uri(),
                api.alice,
                json!({"project_key": "DST", "document": document}),
            )
            .await;
        assert_eq!(answer.code(), Some(400), "{}", answer.body);
        assert_eq!(
            answer.body["message"],
            json!("unsupported export format; expected sylvode.project_export version 1"),
            "{}",
            answer.body
        );
    }
    assert_eq!(
        api.db_count(
            "SELECT COUNT(*)::bigint AS n FROM work_items WHERE project_id = $1",
            target
        )
        .await,
        0
    );
    api.finish().await;
}
