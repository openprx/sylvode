//! Milestone rules through the real router: names, permissions, assignment of items, progress,
//! the list filter, deletion and the webhooks. Uses the fixture of `item_api_tests` (`alice` owns
//! the workspace, `bob` is a member, `carol` belongs to another workspace) against a scratch
//! `PostgreSQL` (skipped without `OPENPR_TEST_DATABASE_URL`).
#![allow(
    clippy::indexing_slicing,
    clippy::too_many_lines,
    clippy::similar_names,
    clippy::items_after_statements,
    clippy::doc_markdown
)]

use super::item_api_tests::{Api, exec};
use axum::http::{Method, StatusCode};
use sea_orm::{ConnectionTrait, DbBackend, Statement};
use serde_json::{Value, json};
use uuid::Uuid;

impl Api {
    fn milestones_uri(&self) -> String {
        format!("/api/v1/workspaces/{}/milestones", self.ws)
    }

    /// Creates a milestone as alice; panics unless it is accepted.
    async fn new_milestone(&self, name: &str) -> Uuid {
        let answer = self
            .post(&self.milestones_uri(), self.alice, json!({"name": name}))
            .await;
        assert_eq!(answer.code(), Some(0), "create milestone {name}: {}", answer.body);
        Uuid::parse_str(answer.data()["id"].as_str().expect("id")).expect("uuid")
    }

    /// A second project of the main workspace.
    async fn extra_project(&self, key: &str) -> Uuid {
        let id = Uuid::new_v4();
        exec(
            &self.db,
            "INSERT INTO projects (id, workspace_id, key, name, created_by, type_key) \
             VALUES ($1, $2, $3, 'second', $4, 'code_project')",
            vec![id.into(), self.ws.into(), key.to_string().into(), self.alice.into()],
        )
        .await;
        id
    }

    async fn item_in(&self, project: Uuid, title: &str, extra: Value) -> Uuid {
        let mut body = json!({"title": title});
        if let (Some(target), Some(source)) = (body.as_object_mut(), extra.as_object()) {
            for (key, value) in source {
                target.insert(key.clone(), value.clone());
            }
        }
        let answer = self
            .post(&format!("/api/v1/projects/{project}/issues"), self.alice, body)
            .await;
        assert_eq!(answer.code(), Some(0), "create item {title}: {}", answer.body);
        Uuid::parse_str(answer.data()["id"].as_str().expect("id")).expect("uuid")
    }

    async fn item_milestone(&self, item: Uuid) -> Value {
        let answer = self.get(&format!("/api/v1/issues/{item}"), self.alice).await;
        answer.data()["milestone_id"].clone()
    }

    /// The main workspace of `carol`, which `alice` is not a member of.
    async fn other_workspace(&self) -> Uuid {
        let row = self
            .db
            .query_one(Statement::from_sql_and_values(
                DbBackend::Postgres,
                "SELECT workspace_id FROM workspace_members WHERE user_id = $1",
                vec![self.carol.into()],
            ))
            .await
            .expect("query")
            .expect("carol has a workspace");
        row.try_get("", "workspace_id").expect("workspace_id")
    }
}

const MILESTONE_EVENTS: &str =
    r#"["milestone.created","milestone.updated","milestone.deleted","issue.updated","issue.created"]"#;

#[tokio::test]
async fn milestone_crud_and_names_ignore_case() {
    let Some(api) = Api::new("milestone_crud").await else {
        return;
    };
    let created = api
        .post(
            &api.milestones_uri(),
            api.alice,
            json!({"name": "  Beta ", "description": "first release", "target_date": "2030-06-30"}),
        )
        .await;
    assert_eq!(created.code(), Some(0), "{}", created.body);
    let milestone = created.data().clone();
    assert_eq!(milestone["name"], json!("Beta"));
    assert_eq!(milestone["description"], json!("first release"));
    assert_eq!(milestone["target_date"], json!("2030-06-30"));
    assert_eq!(milestone["status"], json!("open"));
    assert_eq!(milestone["workspace_id"], json!(api.ws.to_string()));
    assert_eq!(milestone["progress"], json!({"total": 0, "done": 0, "by_project": []}));
    let id = milestone["id"].as_str().expect("id").to_string();

    // The name is taken ignoring case, by create and by rename.
    let taken = api
        .post(&api.milestones_uri(), api.alice, json!({"name": "beta"}))
        .await;
    assert_eq!(taken.code(), Some(409), "{}", taken.body);
    assert_eq!(taken.body["error_code"], json!("milestone_name_taken"));
    let other = api.new_milestone("Gamma").await;
    let rename = api
        .put(
            &format!("/api/v1/milestones/{other}"),
            api.alice,
            json!({"name": "BETA"}),
        )
        .await;
    assert_eq!(rename.code(), Some(409), "{}", rename.body);
    // Renaming to its own name in another case is fine.
    let own = api
        .put(
            &format!("/api/v1/milestones/{other}"),
            api.alice,
            json!({"name": "GAMMA"}),
        )
        .await;
    assert_eq!(own.code(), Some(0), "{}", own.body);

    // Name rules.
    for body in [json!({"name": "   "}), json!({"name": "x".repeat(101)})] {
        let refused = api.post(&api.milestones_uri(), api.alice, body).await;
        assert_eq!(refused.code(), Some(400), "{}", refused.body);
    }
    let bad_date = api
        .post(
            &api.milestones_uri(),
            api.alice,
            json!({"name": "Dated", "target_date": "soon"}),
        )
        .await;
    assert_eq!(bad_date.code(), Some(400), "{}", bad_date.body);
    let unknown = api
        .post(
            &api.milestones_uri(),
            api.alice,
            json!({"name": "Typed", "goal": "ship"}),
        )
        .await;
    assert_eq!(unknown.status, StatusCode::UNPROCESSABLE_ENTITY, "{}", unknown.body);

    // Edit: description, clearing the date, closing.
    let uri = format!("/api/v1/milestones/{id}");
    let edited = api
        .put(
            &uri,
            api.alice,
            json!({"description": "second look", "target_date": null}),
        )
        .await;
    assert_eq!(edited.code(), Some(0), "{}", edited.body);
    assert_eq!(edited.data()["description"], json!("second look"));
    assert!(edited.data()["target_date"].is_null(), "{}", edited.body);
    let closed = api.put(&uri, api.alice, json!({"status": "closed"})).await;
    assert_eq!(closed.code(), Some(0), "{}", closed.body);
    assert_eq!(closed.data()["status"], json!("closed"));
    for body in [
        json!({"name": null}),
        json!({"status": null}),
        json!({"description": null}),
        json!({"status": "archived"}),
        json!({}),
    ] {
        let refused = api.put(&uri, api.alice, body.clone()).await;
        assert_eq!(refused.code(), Some(400), "{body}: {}", refused.body);
    }

    // The list filters by status; a member reads.
    let open = api.get(&format!("{}?status=open", api.milestones_uri()), api.bob).await;
    let names: Vec<&str> = open.data()["items"]
        .as_array()
        .expect("items")
        .iter()
        .filter_map(|m| m["name"].as_str())
        .collect();
    assert_eq!(names, vec!["GAMMA"], "{}", open.body);
    let closed_list = api
        .get(&format!("{}?status=closed", api.milestones_uri()), api.bob)
        .await;
    assert_eq!(closed_list.data()["items"].as_array().expect("items").len(), 1);
    let all = api.get(&api.milestones_uri(), api.bob).await;
    assert_eq!(all.data()["items"].as_array().expect("items").len(), 2);
    let bad = api.get(&format!("{}?status=done", api.milestones_uri()), api.bob).await;
    assert_eq!(bad.code(), Some(400), "{}", bad.body);

    // Reading one, then deleting it.
    let one = api.get(&uri, api.bob).await;
    assert_eq!(one.code(), Some(0), "{}", one.body);
    assert_eq!(one.data()["items_by_project"], json!([]));
    assert_eq!(one.data()["truncated"], json!(false));
    let deleted = api.call(Method::DELETE, &uri, api.alice, None).await;
    assert_eq!(deleted.code(), Some(0), "{}", deleted.body);
    let gone = api.get(&uri, api.alice).await;
    assert_eq!(gone.code(), Some(404), "{}", gone.body);
    api.finish().await;
}

#[tokio::test]
async fn member_cannot_create() {
    let Some(api) = Api::new("milestone_member").await else {
        return;
    };
    let milestone = api.new_milestone("Beta").await;
    let uri = format!("/api/v1/milestones/{milestone}");
    let create = api.post(&api.milestones_uri(), api.bob, json!({"name": "Mine"})).await;
    assert_eq!(create.code(), Some(403), "{}", create.body);
    let edit = api.put(&uri, api.bob, json!({"status": "closed"})).await;
    assert_eq!(edit.code(), Some(403), "{}", edit.body);
    let delete = api.call(Method::DELETE, &uri, api.bob, None).await;
    assert_eq!(delete.code(), Some(403), "{}", delete.body);
    // Another workspace's member sees nothing of it, whatever the route.
    let read = api.get(&uri, api.carol).await;
    assert!(matches!(read.code(), Some(403 | 404)), "{}", read.body);
    let list = api.get(&api.milestones_uri(), api.carol).await;
    assert!(matches!(list.code(), Some(403 | 404)), "{}", list.body);
    let still = api.get(&uri, api.alice).await;
    assert_eq!(still.data()["status"], json!("open"));
    api.finish().await;
}

#[tokio::test]
async fn item_in_other_workspace_refused() {
    let Some(api) = Api::new("milestone_other_ws").await else {
        return;
    };
    let foreign = Uuid::new_v4();
    exec(
        &api.db,
        "INSERT INTO milestones (id, workspace_id, name) VALUES ($1, $2, 'Theirs')",
        vec![foreign.into(), api.other_workspace().await.into()],
    )
    .await;
    let message = "milestone must be an open milestone of this workspace";
    let create = api
        .post(
            &format!("/api/v1/projects/{}/issues", api.project),
            api.alice,
            json!({"title": "x", "milestone_id": foreign}),
        )
        .await;
    assert_eq!(create.code(), Some(400), "{}", create.body);
    assert_eq!(create.body["message"], json!(message));
    let item = api.create_item("plain", json!({})).await;
    let update = api
        .put(
            &format!("/api/v1/issues/{item}"),
            api.alice,
            json!({"milestone_id": foreign}),
        )
        .await;
    assert_eq!(update.code(), Some(400), "{}", update.body);
    assert_eq!(update.body["message"], json!(message));
    // A milestone that does not exist answers the same.
    let missing = api
        .put(
            &format!("/api/v1/issues/{item}"),
            api.alice,
            json!({"milestone_id": Uuid::new_v4()}),
        )
        .await;
    assert_eq!(missing.body["message"], json!(message), "{}", missing.body);
    assert!(api.item_milestone(item).await.is_null());
    api.finish().await;
}

#[tokio::test]
async fn closed_milestone_refused_on_assign() {
    let Some(api) = Api::new("milestone_closed").await else {
        return;
    };
    let milestone = api.new_milestone("Beta").await;
    let item = api.create_item("in beta", json!({"milestone_id": milestone})).await;
    assert_eq!(api.item_milestone(item).await, json!(milestone.to_string()));
    let closed = api
        .put(
            &format!("/api/v1/milestones/{milestone}"),
            api.alice,
            json!({"status": "closed"}),
        )
        .await;
    assert_eq!(closed.code(), Some(0), "{}", closed.body);

    let message = "milestone must be an open milestone of this workspace";
    let create = api
        .post(
            &format!("/api/v1/projects/{}/issues", api.project),
            api.alice,
            json!({"title": "late", "milestone_id": milestone}),
        )
        .await;
    assert_eq!(create.code(), Some(400), "{}", create.body);
    assert_eq!(create.body["message"], json!(message));
    let other = api.create_item("other", json!({})).await;
    let assign = api
        .put(
            &format!("/api/v1/issues/{other}"),
            api.bob,
            json!({"milestone_id": milestone}),
        )
        .await;
    assert_eq!(assign.code(), Some(400), "{}", assign.body);

    // The item already in it keeps it, and may be edited (the same value is not an assignment).
    let edit = api
        .put(
            &format!("/api/v1/issues/{item}"),
            api.bob,
            json!({"title": "renamed", "milestone_id": milestone}),
        )
        .await;
    assert_eq!(edit.code(), Some(0), "{}", edit.body);
    assert_eq!(edit.data()["milestone_id"], json!(milestone.to_string()));
    // Leaving a closed milestone is allowed.
    let cleared = api
        .put(
            &format!("/api/v1/issues/{item}"),
            api.bob,
            json!({"milestone_id": null}),
        )
        .await;
    assert_eq!(cleared.code(), Some(0), "{}", cleared.body);
    assert!(cleared.data()["milestone_id"].is_null(), "{}", cleared.body);
    // Reopened, it takes items again.
    api.put(
        &format!("/api/v1/milestones/{milestone}"),
        api.alice,
        json!({"status": "open"}),
    )
    .await;
    let assign = api
        .put(
            &format!("/api/v1/issues/{other}"),
            api.bob,
            json!({"milestone_id": milestone}),
        )
        .await;
    assert_eq!(assign.code(), Some(0), "{}", assign.body);
    api.finish().await;
}

#[tokio::test]
async fn assigning_records_activity_and_event() {
    let Some(api) = Api::new("milestone_assign").await else {
        return;
    };
    api.add_webhook_for(MILESTONE_EVENTS).await;
    let first = api.new_milestone("Alpha").await;
    let second = api.new_milestone("Beta").await;
    let item = api.create_item("moving", json!({})).await;
    let uri = format!("/api/v1/issues/{item}");

    api.put(&uri, api.bob, json!({"milestone_id": first})).await;
    api.put(&uri, api.bob, json!({"milestone_id": second})).await;
    // The same value again records nothing.
    api.put(&uri, api.bob, json!({"milestone_id": second, "priority": "high"}))
        .await;
    api.put(&uri, api.bob, json!({"milestone_id": null})).await;
    let activity = api.activity_details(item, "milestone_changed").await;
    assert_eq!(
        activity,
        vec![
            json!({"from": null, "to": first.to_string()}),
            json!({"from": first.to_string(), "to": second.to_string()}),
            json!({"from": second.to_string(), "to": null}),
        ]
    );
    let events = api.payloads("issue.updated", 4).await;
    let changes: Vec<Value> = events
        .iter()
        .filter_map(|e| e["data"]["changes"].get("milestone_id").cloned())
        .collect();
    assert_eq!(changes.len(), 3, "{events:?}");
    assert_eq!(changes[0], json!({"old": null, "new": first.to_string()}));
    assert_eq!(changes[2], json!({"old": second.to_string(), "new": null}));
    // Item payloads carry the milestone.
    assert!(
        events.iter().all(|e| e["data"]["issue"].get("milestone_id").is_some()),
        "{events:?}"
    );
    api.finish().await;
}

#[tokio::test]
async fn list_filters_by_milestone() {
    let Some(api) = Api::new("milestone_filter").await else {
        return;
    };
    let milestone = api.new_milestone("Beta").await;
    let inside = api.create_item("inside", json!({"milestone_id": milestone})).await;
    let outside = api.create_item("outside", json!({})).await;
    let base = format!("/api/v1/projects/{}/issues", api.project);
    let ids = |answer: &super::item_api_tests::Answer| -> Vec<String> {
        answer.data()["items"]
            .as_array()
            .expect("items")
            .iter()
            .filter_map(|i| i["id"].as_str().map(str::to_string))
            .collect()
    };
    let with = api.get(&format!("{base}?milestone_id={milestone}"), api.bob).await;
    assert_eq!(ids(&with), vec![inside.to_string()], "{}", with.body);
    assert_eq!(with.data()["items"][0]["milestone_id"], json!(milestone.to_string()));
    let none = api.get(&format!("{base}?milestone_id=none"), api.bob).await;
    assert_eq!(ids(&none), vec![outside.to_string()], "{}", none.body);
    let bad = api.get(&format!("{base}?milestone_id=soon"), api.bob).await;
    assert_eq!(bad.code(), Some(400), "{}", bad.body);
    api.finish().await;
}

#[tokio::test]
async fn progress_counts_finished_items() {
    let Some(api) = Api::new("milestone_progress").await else {
        return;
    };
    let second = api.extra_project("ZED").await;
    let milestone = api.new_milestone("Beta").await;
    for (title, state) in [("a", "todo"), ("b", "done"), ("c", "done")] {
        api.item_in(api.project, title, json!({"milestone_id": milestone, "state": state}))
            .await;
    }
    api.item_in(second, "z1", json!({"milestone_id": milestone, "state": "in_progress"}))
        .await;
    api.item_in(second, "z2", json!({"milestone_id": milestone, "state": "done"}))
        .await;
    // An item of no milestone does not count.
    api.item_in(second, "z3", json!({"state": "done"})).await;

    let one = api.get(&format!("/api/v1/milestones/{milestone}"), api.bob).await;
    assert_eq!(one.code(), Some(0), "{}", one.body);
    let progress = &one.data()["progress"];
    assert_eq!(progress["total"], json!(5));
    assert_eq!(progress["done"], json!(3));
    let by_project = progress["by_project"].as_array().expect("by_project");
    assert_eq!(by_project.len(), 2, "{progress}");
    assert_eq!(by_project[0]["project_key"], json!("QLB"));
    assert_eq!(
        (by_project[0]["total"].clone(), by_project[0]["done"].clone()),
        (json!(3), json!(2))
    );
    assert_eq!(by_project[1]["project_key"], json!("ZED"));
    assert_eq!(by_project[1]["project_id"], json!(second.to_string()));
    assert_eq!(
        (by_project[1]["total"].clone(), by_project[1]["done"].clone()),
        (json!(2), json!(1))
    );

    // The detail lists the items by project with their identifiers and done flags.
    let groups = one.data()["items_by_project"].as_array().expect("groups");
    assert_eq!(groups.len(), 2);
    assert_eq!(groups[0]["project_key"], json!("QLB"));
    let first_items = groups[0]["items"].as_array().expect("items");
    assert_eq!(first_items.len(), 3);
    assert_eq!(first_items[0]["identifier"], json!("QLB-1"));
    assert_eq!(first_items[0]["title"], json!("a"));
    assert_eq!(first_items[0]["state"], json!("todo"));
    assert_eq!(first_items[0]["done"], json!(false));
    assert_eq!(first_items[1]["done"], json!(true));
    assert_eq!(groups[1]["items"].as_array().expect("items").len(), 2);

    // The list carries the same progress.
    let list = api.get(&api.milestones_uri(), api.bob).await;
    assert_eq!(list.data()["items"][0]["progress"]["done"], json!(3));
    assert_eq!(list.data()["items"][0]["progress"]["total"], json!(5));
    api.finish().await;
}

#[tokio::test]
async fn delete_clears_items_with_events() {
    let Some(api) = Api::new("milestone_delete").await else {
        return;
    };
    api.add_webhook_for(MILESTONE_EVENTS).await;
    let second = api.extra_project("ZED").await;
    let milestone = api.new_milestone("Beta").await;
    let keep = api.new_milestone("Keep").await;
    let a = api.create_item("a", json!({"milestone_id": milestone})).await;
    let b = api.item_in(second, "b", json!({"milestone_id": milestone})).await;
    let kept = api.create_item("kept", json!({"milestone_id": keep})).await;
    // The creation events are not what is asserted below; wait for them to be logged first.
    api.payloads("issue.created", 3).await;
    api.payloads("milestone.created", 2).await;

    let deleted = api
        .call(
            Method::DELETE,
            &format!("/api/v1/milestones/{milestone}"),
            api.alice,
            None,
        )
        .await;
    assert_eq!(deleted.code(), Some(0), "{}", deleted.body);

    assert!(api.item_milestone(a).await.is_null());
    assert!(api.item_milestone(b).await.is_null());
    assert_eq!(api.item_milestone(kept).await, json!(keep.to_string()));
    for item in [a, b] {
        assert_eq!(
            api.activity_details(item, "milestone_changed").await,
            vec![json!({"from": milestone.to_string(), "to": null})],
            "activity of {item}"
        );
    }
    assert!(api.activity_details(kept, "milestone_changed").await.is_empty());

    // One issue.updated per item, in the item's own project, and the milestone.deleted event.
    let updated = api.payloads("issue.updated", 2).await;
    let mut seen: Vec<(String, Value)> = updated
        .iter()
        .map(|e| {
            (
                e["data"]["issue"]["id"].as_str().expect("issue id").to_string(),
                e["data"]["changes"]["milestone_id"].clone(),
            )
        })
        .collect();
    seen.sort_by(|x, y| x.0.cmp(&y.0));
    let mut expected: Vec<(String, Value)> = [a, b]
        .iter()
        .map(|id| (id.to_string(), json!({"old": milestone.to_string(), "new": null})))
        .collect();
    expected.sort_by(|x, y| x.0.cmp(&y.0));
    assert_eq!(seen, expected);
    let projects: Vec<&str> = updated.iter().filter_map(|e| e["project"]["key"].as_str()).collect();
    assert!(projects.contains(&"QLB") && projects.contains(&"ZED"), "{projects:?}");
    let events = api.payloads("milestone.deleted", 1).await;
    assert_eq!(events[0]["data"]["milestone"]["name"], json!("Beta"));
    assert_eq!(events[0]["data"]["cleared_item_count"], json!(2));
    assert_eq!(events[0]["project"]["key"], json!("WORKSPACE"));
    api.finish().await;
}

#[tokio::test]
async fn milestone_events_carry_changes() {
    let Some(api) = Api::new("milestone_events").await else {
        return;
    };
    api.add_webhook_for(MILESTONE_EVENTS).await;
    let milestone = api.new_milestone("Beta").await;
    let uri = format!("/api/v1/milestones/{milestone}");
    api.put(&uri, api.alice, json!({"name": "Beta 2", "status": "closed"}))
        .await;
    // Nothing changes: no event.
    api.put(&uri, api.alice, json!({"name": "Beta 2"})).await;

    let created = api.payloads("milestone.created", 1).await;
    assert_eq!(created[0]["data"]["milestone"]["name"], json!("Beta"));
    assert_eq!(created[0]["actor"]["name"], json!("alice"));
    let updated = api.payloads("milestone.updated", 1).await;
    assert_eq!(updated[0]["data"]["milestone"]["name"], json!("Beta 2"));
    assert_eq!(
        updated[0]["data"]["changes"],
        json!({"name": {"old": "Beta", "new": "Beta 2"}, "status": {"old": "open", "new": "closed"}})
    );
    api.finish().await;
}

#[tokio::test]
async fn the_list_pages() {
    let Some(api) = Api::new("milestone_paging").await else {
        return;
    };
    for name in ["m1", "m2", "m3", "m4", "m5"] {
        api.new_milestone(name).await;
    }
    let closed = api.new_milestone("m6").await;
    api.put(
        &format!("/api/v1/milestones/{closed}"),
        api.alice,
        json!({"status": "closed"}),
    )
    .await;
    let names = |answer: &super::item_api_tests::Answer| -> Vec<String> {
        answer.data()["items"]
            .as_array()
            .expect("items")
            .iter()
            .filter_map(|m| m["name"].as_str().map(str::to_string))
            .collect()
    };
    let base = api.milestones_uri();

    let first = api.get(&format!("{base}?page=1&per_page=4"), api.bob).await;
    assert_eq!(first.code(), Some(0), "{}", first.body);
    assert_eq!(names(&first), vec!["m1", "m2", "m3", "m4"]);
    let data = first.data();
    assert_eq!(
        (
            data["total"].clone(),
            data["page"].clone(),
            data["per_page"].clone(),
            data["total_pages"].clone()
        ),
        (json!(6), json!(1), json!(4), json!(2))
    );
    let second = api.get(&format!("{base}?page=2&per_page=4"), api.bob).await;
    assert_eq!(names(&second), vec!["m5", "m6"]);
    // Past the last page: no items, the counts stay right.
    let past = api.get(&format!("{base}?page=3&per_page=4"), api.bob).await;
    assert!(names(&past).is_empty(), "{}", past.body);
    assert_eq!(past.data()["total"], json!(6));
    // The status filter counts only its own rows.
    let open = api.get(&format!("{base}?status=open&per_page=2"), api.bob).await;
    assert_eq!(names(&open), vec!["m1", "m2"]);
    assert_eq!(
        (open.data()["total"].clone(), open.data()["total_pages"].clone()),
        (json!(5), json!(3))
    );
    // Defaults, and out-of-range values as the work item list treats them.
    let default = api.get(&base, api.bob).await;
    assert_eq!(
        (default.data()["page"].clone(), default.data()["per_page"].clone()),
        (json!(1), json!(15))
    );
    let big = api.get(&format!("{base}?per_page=1000"), api.bob).await;
    assert_eq!(big.data()["per_page"], json!(100));
    let accepted = api.get(&format!("{base}?per_page=100"), api.bob).await;
    assert_eq!(accepted.data()["per_page"], json!(100));
    let low = api.get(&format!("{base}?page=0&per_page=0"), api.bob).await;
    assert_eq!(
        (low.data()["page"].clone(), low.data()["per_page"].clone()),
        (json!(1), json!(1))
    );
    // An unknown query parameter is still refused.
    let unknown = api.get(&format!("{base}?sort=name"), api.bob).await;
    assert_eq!(unknown.status, StatusCode::BAD_REQUEST, "{}", unknown.body);
    api.finish().await;
}
