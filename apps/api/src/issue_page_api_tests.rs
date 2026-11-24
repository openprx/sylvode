//! Links between work items and Flow pages through the real router: link, list, unlink, the
//! limit, idempotence, what a caller who cannot read a page learns, the trash and purge rules, the
//! activity entries, the webhook payloads, the page side and a project-scoped bot. Uses the
//! fixture of `item_api_tests` (`alice` owns the workspace, `bob` is a member, `carol` owns
//! another workspace) against a scratch `PostgreSQL` (skipped without `OPENPR_TEST_DATABASE_URL`).
//! The pages are seeded straight into the tables, so the tests also run where the collaborative
//! engine cannot.
#![allow(
    clippy::indexing_slicing,
    clippy::too_many_lines,
    clippy::similar_names,
    clippy::items_after_statements,
    clippy::doc_markdown
)]

use super::item_api_tests::{Answer, Api, exec};
use axum::http::{Method, StatusCode};
use sea_orm::{ConnectionTrait, DbBackend, Statement};
use serde_json::{Value, json};
use uuid::Uuid;

impl Api {
    fn pages_uri(item: Uuid) -> String {
        format!("/api/v1/issues/{item}/pages")
    }

    /// A page of `workspace`, seeded with its collaborative document row and projection.
    async fn seed_page(&self, workspace: Uuid, title: &str) -> Uuid {
        self.seed_page_in(workspace, None, title).await
    }

    async fn seed_page_in(&self, workspace: Uuid, project: Option<Uuid>, title: &str) -> Uuid {
        let id = Uuid::new_v4();
        exec(
            &self.db,
            "INSERT INTO flow_objects (id, workspace_id, project_id, object_type, parent_id) \
             VALUES ($1, $2, $3, 'page', flow_ensure_navigator_root($2, $3))",
            vec![id.into(), workspace.into(), project.into()],
        )
        .await;
        self.seed_content(id, title).await;
        id
    }

    async fn seed_content(&self, id: Uuid, title: &str) {
        exec(
            &self.db,
            "INSERT INTO collab_documents (object_id, format_version, snapshot, snapshot_frontier, head_seq, head_frontier) \
             VALUES ($1, 'loro-1', '\\x'::bytea, '\\x'::bytea, 1, '\\x'::bytea)",
            vec![id.into()],
        )
        .await;
        exec(
            &self.db,
            "INSERT INTO flow_object_projections (object_id, document_seq, document_frontier, title, state, plain_text) \
             VALUES ($1, 1, '\\x'::bytea, $2, '{}'::jsonb, $2)",
            vec![id.into(), title.to_string().into()],
        )
        .await;
    }

    /// A personal page of `owner`, placed into the main workspace at the `view` level.
    async fn seed_mounted_personal_page(&self, owner: Uuid, title: &str, destination: Uuid) -> Uuid {
        let page = self.seed_personal_page(owner, title).await;
        exec(
            &self.db,
            "INSERT INTO flow_mounts (source_object_id, destination_workspace_id, grant_level, created_by) \
             VALUES ($1, $2, 'view', $3)",
            vec![page.into(), destination.into(), owner.into()],
        )
        .await;
        page
    }

    async fn seed_personal_page(&self, owner: Uuid, title: &str) -> Uuid {
        let existing = self
            .db
            .query_one(Statement::from_sql_and_values(
                DbBackend::Postgres,
                "SELECT s.id AS scope, (SELECT o.id FROM flow_objects o WHERE o.scope_id = s.id AND o.parent_id IS NULL) AS root \
                 FROM flow_scopes s WHERE s.kind = 'personal' AND s.owner_user_id = $1",
                vec![owner.into()],
            ))
            .await
            .expect("query");
        let (scope, root) = if let Some(row) = existing {
            (
                row.try_get::<Uuid>("", "scope").expect("scope"),
                row.try_get::<Uuid>("", "root").expect("root"),
            )
        } else {
            let scope = Uuid::new_v4();
            exec(
                &self.db,
                "INSERT INTO flow_scopes (id, kind, owner_user_id) VALUES ($1, 'personal', $2)",
                vec![scope.into(), owner.into()],
            )
            .await;
            exec(
                &self.db,
                "INSERT INTO flow_scope_settings (scope_id) VALUES ($1)",
                vec![scope.into()],
            )
            .await;
            let root = Uuid::new_v4();
            exec(
                &self.db,
                "INSERT INTO flow_objects (id, scope_id, workspace_id, object_type, created_by, governance_metadata) \
                 VALUES ($1, $2, NULL, 'navigator', $3, '{\"system_role\":\"workspace_navigator_root\"}'::jsonb)",
                vec![root.into(), scope.into(), owner.into()],
            )
            .await;
            (scope, root)
        };
        let id = Uuid::new_v4();
        exec(
            &self.db,
            "INSERT INTO flow_objects (id, scope_id, workspace_id, object_type, parent_id, created_by) \
             VALUES ($1, $2, NULL, 'page', $3, $4)",
            vec![id.into(), scope.into(), root.into(), owner.into()],
        )
        .await;
        self.seed_content(id, title).await;
        id
    }

    /// Cuts the page off from the workspace baseline: only owners/admins and explicit grants reach it.
    async fn restrict(&self, page: Uuid) {
        exec(
            &self.db,
            "UPDATE flow_objects SET inherit_from_parent = false WHERE id = $1",
            vec![page.into()],
        )
        .await;
    }

    async fn grant(&self, workspace: Uuid, page: Uuid, user: Uuid, level: &str) {
        exec(
            &self.db,
            "INSERT INTO flow_object_grants (workspace_id, object_id, principal_kind, principal_id, level) \
             VALUES ($1, $2, 'user', $3, $4)",
            vec![workspace.into(), page.into(), user.into(), level.to_string().into()],
        )
        .await;
    }

    async fn link(&self, item: Uuid, page: Uuid, user: Uuid) -> Answer {
        self.post(&Self::pages_uri(item), user, json!({"object_id": page}))
            .await
    }

    async fn unlink(&self, item: Uuid, page: Uuid, user: Uuid) -> Answer {
        self.call(Method::DELETE, &format!("{}/{page}", Self::pages_uri(item)), user, None)
            .await
    }

    async fn links(&self, item: Uuid, user: Uuid) -> Vec<Value> {
        let answer = self.get(&Self::pages_uri(item), user).await;
        assert_eq!(answer.code(), Some(0), "{}", answer.body);
        answer.data().as_array().expect("an array").clone()
    }

    async fn link_rows(&self, item: Uuid) -> i64 {
        self.db_count(
            "SELECT COUNT(*)::bigint AS n FROM work_item_page_links WHERE work_item_id = $1",
            item,
        )
        .await
    }

    async fn page_activities(&self, item: Uuid, user: Uuid, action: &str) -> Vec<Value> {
        let answer = self.get(&format!("/api/v1/issues/{item}/activities"), user).await;
        assert_eq!(answer.code(), Some(0), "{}", answer.body);
        answer.data()["items"]
            .as_array()
            .expect("items")
            .iter()
            .filter(|entry| entry["action"] == json!(action))
            .map(|entry| entry["detail"].clone())
            .collect()
    }

    async fn rename_page(&self, page: Uuid, title: &str) {
        exec(
            &self.db,
            "UPDATE flow_object_projections SET title = $2 WHERE object_id = $1",
            vec![page.into(), title.to_string().into()],
        )
        .await;
    }

    async fn set_archived(&self, page: Uuid, archived: bool) {
        exec(
            &self.db,
            "UPDATE flow_objects SET lifecycle_status = CASE WHEN $2 THEN 'archived' ELSE 'active' END, \
                                     archived_at = CASE WHEN $2 THEN now() END WHERE id = $1",
            vec![page.into(), archived.into()],
        )
        .await;
    }

    async fn other_workspace_id(&self) -> Uuid {
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

#[tokio::test]
async fn link_list_unlink_and_idempotence() {
    let Some(api) = Api::new("page_links_basic").await else {
        return;
    };
    let item = api.create_item("Write the spec", json!({})).await;
    let page = api.seed_page(api.ws, "Spec").await;

    // A new item has no links, in the single answer and in the list answer.
    assert!(api.links(item, api.alice).await.is_empty());
    let single = api.get(&format!("/api/v1/issues/{item}"), api.alice).await;
    assert_eq!(single.data()["page_links"], json!([]));
    assert!(single.data().get("page_link_count").is_none(), "{}", single.body);

    let linked = api.link(item, page, api.alice).await;
    assert_eq!(linked.code(), Some(0), "{}", linked.body);
    let link = linked.data().clone();
    assert_eq!(link["object_id"], json!(page.to_string()));
    assert_eq!(link["title"], json!("Spec"));
    assert_eq!(link["access"], json!("visible"));
    assert_eq!(link["lifecycle"], json!("active"));
    assert!(link["linked_at"].is_string(), "{link}");

    // The same pair again: the same answer, nothing new stored.
    let again = api.link(item, page, api.alice).await;
    assert_eq!(again.code(), Some(0), "{}", again.body);
    assert_eq!(again.data(), &link);
    assert_eq!(api.link_rows(item).await, 1);
    assert_eq!(api.page_activities(item, api.alice, "page_linked").await.len(), 1);

    // The item answers carry the link: the single answer the links, the list the count.
    let single = api.get(&format!("/api/v1/issues/{item}"), api.bob).await;
    assert_eq!(single.data()["page_links"][0]["title"], json!("Spec"));
    let list = api
        .get(&format!("/api/v1/projects/{}/issues", api.project), api.bob)
        .await;
    let row = &list.data()["items"][0];
    assert_eq!(row["page_link_count"], json!(1), "{}", list.body);
    assert!(row.get("page_links").is_none(), "{}", list.body);
    let by_identifier = api.get("/api/v1/issues/by-identifier/QLB-1", api.alice).await;
    assert_eq!(
        by_identifier.data()["page_links"][0]["object_id"],
        json!(page.to_string())
    );

    // Titles are read live: a rename shows at once.
    api.rename_page(page, "Spec v2").await;
    assert_eq!(api.links(item, api.bob).await[0]["title"], json!("Spec v2"));

    // Unlinking: once removes, the second time is a no-op with the same answer.
    let removed = api.unlink(item, page, api.bob).await;
    assert_eq!(removed.code(), Some(0), "{}", removed.body);
    assert_eq!(removed.data(), &json!({}));
    let again = api.unlink(item, page, api.bob).await;
    assert_eq!(again.code(), Some(0), "{}", again.body);
    assert_eq!(again.data(), &json!({}));
    assert!(api.links(item, api.alice).await.is_empty());
    assert_eq!(api.page_activities(item, api.alice, "page_unlinked").await.len(), 1);

    // The activity entries carry the page, resolved for the reader.
    let entries = api.page_activities(item, api.alice, "page_linked").await;
    assert_eq!(entries[0]["object_id"], json!(page.to_string()));
    assert_eq!(
        entries[0]["page"],
        json!({"title": "Spec v2", "access": "visible", "lifecycle": "active"})
    );
    api.finish().await;
}

#[tokio::test]
async fn what_can_be_linked() {
    let Some(api) = Api::new("page_links_rules").await else {
        return;
    };
    let item = api.create_item("Item", json!({})).await;
    let other_ws = api.other_workspace_id().await;
    let foreign = api.seed_page(other_ws, "Not for you").await;

    // A page nobody told alice about, a missing object and a page of another workspace answer the
    // same: not found.
    for object in [foreign, Uuid::new_v4()] {
        let answer = api.link(item, object, api.alice).await;
        assert_eq!(answer.code(), Some(404), "{}", answer.body);
        assert_eq!(answer.body["message"], json!("page not found"));
    }
    // Only a page: the workspace's navigator root is an object but not a page.
    let root: String = api
        .db_count_text("SELECT flow_ensure_navigator_root($1, NULL)::text AS n", api.ws)
        .await;
    let root = Uuid::parse_str(&root).expect("uuid");
    let answer = api.link(item, root, api.alice).await;
    assert_eq!(answer.code(), Some(400), "{}", answer.body);

    // Readable through a grant but belonging to another workspace: refused for this item.
    api.grant(other_ws, foreign, api.alice, "view").await;
    let answer = api.link(item, foreign, api.alice).await;
    assert_eq!(answer.code(), Some(400), "{}", answer.body);
    assert_eq!(answer.body["message"], json!("page is not available in this workspace"));

    // A personal page placed into the workspace links; one that is not placed is not found.
    let placed = api.seed_mounted_personal_page(api.bob, "Bob's notes", api.ws).await;
    let answer = api.link(item, placed, api.alice).await;
    assert_eq!(answer.code(), Some(0), "{}", answer.body);
    assert_eq!(answer.data()["title"], json!("Bob's notes"));
    let unplaced = api.seed_personal_page(api.bob, "Private notes").await;
    let answer = api.link(item, unplaced, api.alice).await;
    assert_eq!(answer.code(), Some(404), "{}", answer.body);
    // The owner of an unplaced personal page can read it, but it is not in this workspace.
    let answer = api.link(item, unplaced, api.bob).await;
    assert_eq!(answer.code(), Some(400), "{}", answer.body);
    assert_eq!(answer.body["message"], json!("page is not available in this workspace"));

    // The body takes the id and nothing else; a person outside the workspace cannot link at all.
    let unknown = api
        .post(
            &Api::pages_uri(item),
            api.alice,
            json!({"object_id": placed, "title": "x"}),
        )
        .await;
    assert_eq!(unknown.status, StatusCode::UNPROCESSABLE_ENTITY, "{}", unknown.body);
    let outsider = api.link(item, placed, api.carol).await;
    assert_eq!(outsider.code(), Some(404), "{}", outsider.body);
    let listing = api.get(&Api::pages_uri(item), api.carol).await;
    assert_eq!(listing.code(), Some(404), "{}", listing.body);
    api.finish().await;
}

#[tokio::test]
async fn at_most_fifty_pages_per_item() {
    let Some(api) = Api::new("page_links_limit").await else {
        return;
    };
    let item = api.create_item("Item", json!({})).await;
    let mut pages = Vec::new();
    for n in 0..51 {
        pages.push(api.seed_page(api.ws, &format!("Page {n}")).await);
    }
    for page in &pages[..50] {
        let answer = api.link(item, *page, api.alice).await;
        assert_eq!(answer.code(), Some(0), "{}", answer.body);
    }
    let over = api.link(item, pages[50], api.alice).await;
    assert_eq!(over.code(), Some(400), "{}", over.body);
    assert_eq!(api.link_rows(item).await, 50);
    // Linking a page that is already there is still fine at the limit.
    let again = api.link(item, pages[0], api.alice).await;
    assert_eq!(again.code(), Some(0), "{}", again.body);
    assert_eq!(api.links(item, api.alice).await.len(), 50);
    api.unlink(item, pages[3], api.alice).await;
    let room = api.link(item, pages[50], api.alice).await;
    assert_eq!(room.code(), Some(0), "{}", room.body);
    api.finish().await;
}

#[tokio::test]
async fn a_caller_who_cannot_read_a_page_never_learns_its_title() {
    let Some(api) = Api::new("page_links_restricted").await else {
        return;
    };
    let item = api.create_item("Item", json!({})).await;
    let secret = api.seed_page(api.ws, "Salary review").await;
    let open = api.seed_page(api.ws, "Handbook").await;
    api.restrict(secret).await;

    // Bob cannot link what he cannot read, and cannot tell it exists.
    let refused = api.link(item, secret, api.bob).await;
    assert_eq!(refused.code(), Some(404), "{}", refused.body);
    assert!(!refused.body.to_string().contains("Salary"), "{}", refused.body);
    // The workspace owner can.
    assert_eq!(api.link(item, secret, api.alice).await.code(), Some(0));
    assert_eq!(api.link(item, open, api.alice).await.code(), Some(0));

    // Bob sees the link, not the title; every surface agrees.
    let links = api.links(item, api.bob).await;
    let by_id = |id: Uuid| {
        links
            .iter()
            .find(|link| link["object_id"] == json!(id.to_string()))
            .cloned()
    };
    let restricted = by_id(secret).expect("the restricted link is listed");
    assert_eq!(restricted["access"], json!("restricted"));
    assert!(restricted["title"].is_null(), "{restricted}");
    assert_eq!(restricted["lifecycle"], json!("active"));
    assert_eq!(by_id(open).expect("listed")["title"], json!("Handbook"));
    let single = api.get(&format!("/api/v1/issues/{item}"), api.bob).await;
    assert!(!single.body.to_string().contains("Salary"), "{}", single.body);
    let entries = api.page_activities(item, api.bob, "page_linked").await;
    assert_eq!(entries.len(), 2);
    assert!(
        !serde_json::to_string(&entries).expect("json").contains("Salary"),
        "{entries:?}"
    );
    assert!(
        entries
            .iter()
            .any(|entry| entry["page"] == json!({"title": null, "access": "restricted", "lifecycle": "active"})),
        "{entries:?}"
    );
    // Alice reads it all.
    let links = api.links(item, api.alice).await;
    assert!(
        links.iter().any(|link| link["title"] == json!("Salary review")),
        "{links:?}"
    );

    // A grant makes it readable for Bob at once; Bob can drop the link either way.
    api.grant(api.ws, secret, api.bob, "view").await;
    let links = api.links(item, api.bob).await;
    assert!(
        links.iter().any(|link| link["title"] == json!("Salary review")),
        "{links:?}"
    );
    api.finish().await;
}

#[tokio::test]
async fn a_member_may_unlink_a_page_they_cannot_read() {
    let Some(api) = Api::new("page_links_unlink_blind").await else {
        return;
    };
    let item = api.create_item("Item", json!({})).await;
    let secret = api.seed_page(api.ws, "Hidden").await;
    api.restrict(secret).await;
    assert_eq!(api.link(item, secret, api.alice).await.code(), Some(0));
    let removed = api.unlink(item, secret, api.bob).await;
    assert_eq!(removed.code(), Some(0), "{}", removed.body);
    assert_eq!(api.link_rows(item).await, 0);
    api.finish().await;
}

#[tokio::test]
async fn trash_keeps_links_and_purge_or_delete_removes_them() {
    let Some(api) = Api::new("page_links_trash").await else {
        return;
    };
    let item = api.create_item("Item", json!({})).await;
    let page = api.seed_page(api.ws, "Plan").await;
    let doomed = api.seed_page(api.ws, "Draft").await;
    assert_eq!(api.link(item, page, api.alice).await.code(), Some(0));
    assert_eq!(api.link(item, doomed, api.alice).await.code(), Some(0));

    // In the trash: the link stays, flagged; the title is still there for who may read it.
    api.set_archived(page, true).await;
    let link = api
        .links(item, api.bob)
        .await
        .into_iter()
        .find(|l| l["object_id"] == json!(page.to_string()))
        .expect("kept");
    assert_eq!(link["lifecycle"], json!("archived"));
    assert_eq!(link["access"], json!("visible"));
    assert_eq!(link["title"], json!("Plan"));
    // Restored: shown as before.
    api.set_archived(page, false).await;
    let link = api
        .links(item, api.bob)
        .await
        .into_iter()
        .find(|l| l["object_id"] == json!(page.to_string()))
        .expect("kept");
    assert_eq!(link["lifecycle"], json!("active"));

    // Purged: the link is gone without an activity entry; the earlier entry says the page is gone.
    api.set_archived(doomed, true).await;
    exec(&api.db, "DELETE FROM flow_objects WHERE id = $1", vec![doomed.into()]).await;
    let links = api.links(item, api.alice).await;
    assert_eq!(links.len(), 1, "{links:?}");
    assert_eq!(api.page_activities(item, api.alice, "page_unlinked").await.len(), 0);
    let entries = api.page_activities(item, api.alice, "page_linked").await;
    assert!(
        entries
            .iter()
            .any(|entry| entry["page"] == json!({"title": null, "access": "removed", "lifecycle": null})),
        "{entries:?}"
    );

    // Deleting the item removes its links.
    let deleted = api
        .call(Method::DELETE, &format!("/api/v1/issues/{item}"), api.alice, None)
        .await;
    assert_eq!(deleted.code(), Some(0), "{}", deleted.body);
    let left = api
        .db_count(
            "SELECT COUNT(*)::bigint AS n FROM work_item_page_links WHERE object_id = $1",
            page,
        )
        .await;
    assert_eq!(left, 0);
    api.finish().await;
}

#[tokio::test]
async fn the_page_side_lists_readable_items_and_counts_the_others() {
    let Some(api) = Api::new("page_links_page_side").await else {
        return;
    };
    let other_ws = api.other_workspace_id().await;
    // A personal page placed into both workspaces: the readers of each see their own side.
    let page = api.seed_mounted_personal_page(api.bob, "Shared notes", api.ws).await;
    exec(
        &api.db,
        "INSERT INTO flow_mounts (source_object_id, destination_workspace_id, grant_level, created_by) \
         VALUES ($1, $2, 'view', $3)",
        vec![page.into(), other_ws.into(), api.bob.into()],
    )
    .await;
    let carol_project = Uuid::new_v4();
    exec(
        &api.db,
        "INSERT INTO projects (id, workspace_id, key, name, created_by, type_key) \
         VALUES ($1, $2, 'OTH', 'other', $3, 'code_project')",
        vec![carol_project.into(), other_ws.into(), api.carol.into()],
    )
    .await;
    let mine = api.create_item("Mine", json!({})).await;
    let theirs = api
        .post(
            &format!("/api/v1/projects/{carol_project}/issues"),
            api.carol,
            json!({"title": "Theirs"}),
        )
        .await;
    let theirs = Uuid::parse_str(theirs.data()["id"].as_str().expect("id")).expect("uuid");
    assert_eq!(api.link(mine, page, api.bob).await.code(), Some(0));
    let linked = api.link(theirs, page, api.carol).await;
    assert_eq!(linked.code(), Some(0), "{}", linked.body);

    let uri = format!("/api/v2/flow/objects/{page}/work-items");
    let seen = api.get(&uri, api.bob).await;
    assert_eq!(seen.code(), Some(0), "{}", seen.body);
    let items = seen.data()["items"].as_array().expect("items");
    assert_eq!(items.len(), 1, "{}", seen.body);
    assert_eq!(items[0]["identifier"], json!("QLB-1"));
    assert_eq!(items[0]["title"], json!("Mine"));
    assert_eq!(items[0]["state"], json!("backlog"));
    assert_eq!(items[0]["project_id"], json!(api.project.to_string()));
    assert_eq!(seen.data()["hidden_count"], json!(1));
    assert!(!seen.body.to_string().contains("Theirs"), "{}", seen.body);

    let theirs_view = api.get(&uri, api.carol).await;
    assert_eq!(theirs_view.data()["items"][0]["identifier"], json!("OTH-1"));
    assert_eq!(theirs_view.data()["hidden_count"], json!(1));

    // Someone who cannot read the page learns nothing about it.
    let stranger = api.get(&uri, api.alice).await;
    assert_eq!(
        stranger.code(),
        Some(0),
        "alice reads the placed page: {}",
        stranger.body
    );
    let secret = api.seed_page(api.ws, "Closed").await;
    api.restrict(secret).await;
    assert_eq!(api.link(mine, secret, api.alice).await.code(), Some(0));
    let closed = api
        .get(&format!("/api/v2/flow/objects/{secret}/work-items"), api.bob)
        .await;
    assert_eq!(closed.code(), Some(404), "{}", closed.body);
    let missing = api
        .get(&format!("/api/v2/flow/objects/{}/work-items", Uuid::new_v4()), api.bob)
        .await;
    assert_eq!(missing.code(), Some(404), "{}", missing.body);
    api.finish().await;
}

#[tokio::test]
async fn link_events_carry_the_change_and_the_item_page_links() {
    let Some(api) = Api::new("page_links_webhook").await else {
        return;
    };
    api.add_webhook_for(r#"["issue.updated"]"#).await;
    let item = api.create_item("Item", json!({})).await;
    let page = api.seed_page(api.ws, "Spec").await;
    let secret = api.seed_page(api.ws, "Closed").await;
    api.restrict(secret).await;
    let placed = api.seed_mounted_personal_page(api.bob, "Notes", api.ws).await;

    assert_eq!(api.link(item, page, api.alice).await.code(), Some(0));
    // The same pair again sends nothing; so does unlinking a pair that is not linked.
    assert_eq!(api.link(item, page, api.alice).await.code(), Some(0));
    assert_eq!(api.unlink(item, Uuid::new_v4(), api.alice).await.code(), Some(0));
    assert_eq!(api.link(item, secret, api.alice).await.code(), Some(0));
    assert_eq!(api.link(item, placed, api.alice).await.code(), Some(0));
    assert_eq!(api.unlink(item, page, api.alice).await.code(), Some(0));

    let payloads = api.payloads("issue.updated", 4).await;
    let changes: Vec<Value> = payloads
        .iter()
        .map(|p| p["data"]["changes"]["page_links"].clone())
        .collect();
    assert!(
        changes.contains(&json!({"added": [page.to_string()], "removed": []})),
        "{changes:?}"
    );
    assert!(
        changes.contains(&json!({"added": [secret.to_string()], "removed": []})),
        "{changes:?}"
    );
    assert!(
        changes.contains(&json!({"added": [placed.to_string()], "removed": []})),
        "{changes:?}"
    );
    assert!(
        changes.contains(&json!({"added": [], "removed": [page.to_string()]})),
        "{changes:?}"
    );

    // The issue object carries the links; a title goes out only for a page every ordinary member
    // can read (not for a restricted page, not for a personal page placed into the workspace).
    let last = &payloads[payloads.len() - 1]["data"]["issue"]["page_links"];
    let titles: Vec<(String, Value)> = last
        .as_array()
        .expect("page_links")
        .iter()
        .map(|link| {
            (
                link["object_id"].as_str().expect("id").to_string(),
                link["title"].clone(),
            )
        })
        .collect();
    assert_eq!(titles.len(), 2, "{last}");
    assert!(titles.contains(&(secret.to_string(), Value::Null)), "{titles:?}");
    assert!(titles.contains(&(placed.to_string(), Value::Null)), "{titles:?}");
    let first = &payloads[0]["data"]["issue"]["page_links"][0];
    assert_eq!(first["title"], json!("Spec"), "{first}");
    api.finish().await;
}

#[tokio::test]
async fn a_project_scoped_bot_reads_with_read_and_links_with_write() {
    use api::services::bots::{self, NewBot};
    let Some(api) = Api::new("page_links_bot").await else {
        return;
    };
    let item = api.create_item("Item", json!({})).await;
    let page = api.seed_page_in(api.ws, Some(api.project), "Spec").await;
    assert_eq!(api.link(item, page, api.alice).await.code(), Some(0));

    let tx = sea_orm::TransactionTrait::begin(&api.db).await.expect("begin");
    let bot = bots::create(
        &tx,
        NewBot {
            workspace_id: api.ws,
            created_by: api.alice,
            name: "scoped",
            permissions: vec!["read".to_string(), "write".to_string()],
            transport_surface: "rest".to_string(),
            expires_at: None,
        },
    )
    .await
    .expect("bot");
    tx.commit().await.expect("commit");
    exec(
        &api.db,
        "UPDATE workspace_bots SET project_scope = 'projects', default_project_level = 'read' WHERE id = $1",
        vec![bot.id.into()],
    )
    .await;
    let call = |method: Method, uri: String, body: Option<Value>| {
        let router = api.router.clone();
        let token = bot.token.clone();
        async move {
            use axum::body::Body;
            use axum::http::{Request, header};
            use tower::ServiceExt as _;
            let mut request = Request::builder()
                .method(method)
                .uri(uri)
                .header(header::AUTHORIZATION, format!("Bearer {token}"));
            if body.is_some() {
                request = request.header(header::CONTENT_TYPE, "application/json");
            }
            let response = router
                .oneshot(
                    request
                        .body(body.map_or_else(Body::empty, |v| Body::from(v.to_string())))
                        .expect("request"),
                )
                .await
                .expect("answer");
            let bytes = axum::body::to_bytes(response.into_body(), 1 << 20).await.expect("body");
            serde_json::from_slice::<Value>(&bytes).unwrap_or(Value::Null)
        }
    };

    // Read level: lists the links and the items of the page; cannot link or unlink.
    let listed = call(Method::GET, Api::pages_uri(item), None).await;
    assert_eq!(listed["code"], json!(0), "{listed}");
    assert_eq!(listed["data"][0]["title"], json!("Spec"));
    let second = api.seed_page(api.ws, "Second").await;
    let refused = call(Method::POST, Api::pages_uri(item), Some(json!({"object_id": second}))).await;
    assert_eq!(refused["code"], json!(403), "{refused}");
    let refused = call(Method::DELETE, format!("{}/{page}", Api::pages_uri(item)), None).await;
    assert_eq!(refused["code"], json!(403), "{refused}");

    // Write level in the project: links and unlinks.
    exec(
        &api.db,
        "INSERT INTO bot_project_grants (bot_id, project_id, level) VALUES ($1, $2, 'write')",
        vec![bot.id.into(), api.project.into()],
    )
    .await;
    let linked = call(Method::POST, Api::pages_uri(item), Some(json!({"object_id": second}))).await;
    assert_eq!(linked["code"], json!(0), "{linked}");
    assert_eq!(linked["data"]["title"], json!("Second"));
    let removed = call(Method::DELETE, format!("{}/{second}", Api::pages_uri(item)), None).await;
    assert_eq!(removed["code"], json!(0), "{removed}");

    // The page side: a bot reads only the items of projects where its level reaches read.
    let hidden_project = Uuid::new_v4();
    exec(
        &api.db,
        "INSERT INTO projects (id, workspace_id, key, name, created_by, type_key) \
         VALUES ($1, $2, 'HID', 'hidden', $3, 'code_project')",
        vec![hidden_project.into(), api.ws.into(), api.alice.into()],
    )
    .await;
    let hidden_item = api
        .post(
            &format!("/api/v1/projects/{hidden_project}/issues"),
            api.alice,
            json!({"title": "Hidden item"}),
        )
        .await;
    let hidden_item = Uuid::parse_str(hidden_item.data()["id"].as_str().expect("id")).expect("uuid");
    assert_eq!(api.link(hidden_item, page, api.alice).await.code(), Some(0));
    exec(
        &api.db,
        "UPDATE workspace_bots SET default_project_level = 'none' WHERE id = $1",
        vec![bot.id.into()],
    )
    .await;
    exec(
        &api.db,
        "UPDATE bot_project_grants SET level = 'read' WHERE bot_id = $1",
        vec![bot.id.into()],
    )
    .await;
    let side = call(Method::GET, format!("/api/v2/flow/objects/{page}/work-items"), None).await;
    assert_eq!(side["code"], json!(0), "{side}");
    let items = side["data"]["items"].as_array().expect("items");
    assert_eq!(items.len(), 1, "{side}");
    assert_eq!(items[0]["identifier"], json!("QLB-1"));
    assert_eq!(side["data"]["hidden_count"], json!(1));
    api.finish().await;
}

#[tokio::test]
async fn a_connected_application_sees_every_link_as_restricted() {
    use api::services::issue_view::{PageAccess, PageCaller, page_links};
    let Some(api) = Api::new("page_links_delegated").await else {
        return;
    };
    let item = api.create_item("Item", json!({})).await;
    let page = api.seed_page(api.ws, "Spec").await;
    assert_eq!(api.link(item, page, api.alice).await.code(), Some(0));
    let caller = |delegated| PageCaller {
        actor_id: api.alice,
        role: "owner".to_string(),
        is_bot: false,
        workspace_id: api.ws,
        delegated,
    };
    let person = page_links(&api.db, &caller(false), item).await.expect("links");
    assert_eq!(person[0].access, PageAccess::Visible);
    assert_eq!(person[0].title.as_deref(), Some("Spec"));
    let application = page_links(&api.db, &caller(true), item).await.expect("links");
    assert_eq!(application.len(), 1);
    assert_eq!(application[0].access, PageAccess::Restricted);
    assert!(application[0].title.is_none());
    api.finish().await;
}
