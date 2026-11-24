//! Mentions without ids: typed `@Name`, stored `{id, label}`, people shown live, and no id in any
//! text meant for display. Drives the real router against a scratch database (skipped without
//! `OPENPR_TEST_DATABASE_URL`).
//!
//! The cast comes from `item_api_tests`: `alice` owns workspace `ws`, `bob` is a member.
#![allow(
    clippy::indexing_slicing,
    clippy::too_many_lines,
    clippy::similar_names,
    clippy::items_after_statements
)]

use crate::item_api_tests::{Api, exec};
use sea_orm::{ConnectionTrait, DbBackend, Statement};
use serde_json::{Value, json};
use uuid::Uuid;

/// Whether `text` contains something shaped like an id.
fn has_id(text: &str) -> bool {
    let bytes = text.as_bytes();
    bytes.windows(36).any(|window| {
        window.iter().enumerate().all(|(at, byte)| match at {
            8 | 13 | 18 | 23 => *byte == b'-',
            _ => byte.is_ascii_hexdigit(),
        })
    })
}

/// Every `name`, `label` and `author_name` string anywhere in `value` (the fields a client shows).
fn shown_texts(value: &Value, out: &mut Vec<String>) {
    match value {
        Value::Object(map) => {
            for (key, inner) in map {
                if matches!(key.as_str(), "name" | "label" | "author_name")
                    && let Some(text) = inner.as_str()
                {
                    out.push(text.to_string());
                }
                shown_texts(inner, out);
            }
        }
        Value::Array(items) => items.iter().for_each(|item| shown_texts(item, out)),
        _ => {}
    }
}

fn assert_no_id_shown(value: &Value, context: &str) {
    let mut texts = Vec::new();
    shown_texts(value, &mut texts);
    for text in texts {
        assert!(!has_id(&text), "{context}: an id is shown as `{text}` in {value}");
    }
}

impl Api {
    async fn rename(&self, user: Uuid, name: &str) {
        exec(
            &self.db,
            "UPDATE users SET name = $2 WHERE id = $1",
            vec![user.into(), name.to_string().into()],
        )
        .await;
    }

    async fn comment(&self, issue: Uuid, as_user: Uuid, body: &str, picked: &[Uuid]) -> Value {
        let answer = self
            .post(
                &format!("/api/v1/issues/{issue}/comments"),
                as_user,
                json!({"body": body, "mentions": picked}),
            )
            .await;
        assert_eq!(answer.code(), Some(0), "{}", answer.body);
        answer.data().clone()
    }

    /// `(content, related comment)` of every `mentioned` notification of `user`.
    async fn mention_notices(&self, user: Uuid) -> Vec<(String, Option<String>)> {
        self.db
            .query_all(Statement::from_sql_and_values(
                DbBackend::Postgres,
                "SELECT content, related_comment_id::text AS c FROM notifications \
                 WHERE user_id = $1 AND kind = 'mentioned' ORDER BY created_at, id",
                vec![user.into()],
            ))
            .await
            .expect("notifications are readable")
            .iter()
            .map(|row| {
                (
                    row.try_get("", "content").expect("content"),
                    row.try_get("", "c").expect("c"),
                )
            })
            .collect()
    }

    async fn listed(&self, issue: Uuid) -> Vec<Value> {
        let answer = self.get(&format!("/api/v1/issues/{issue}/comments"), self.bob).await;
        assert_eq!(answer.code(), Some(0), "{}", answer.body);
        answer.data()["items"].as_array().expect("items").clone()
    }
}

#[tokio::test]
async fn typed_at_name_becomes_a_mention_and_notifies() {
    let Some(api) = Api::new("mention_typed").await else {
        return;
    };
    let dana = api.add_member("Dana Dee", "human").await;
    let item = api.create_item("typed", json!({})).await;
    let created = api.comment(item, api.alice, "ping @dana dee, please", &[]).await;
    assert_eq!(
        created["mentions"],
        json!([{"id": dana, "name": "Dana Dee", "is_bot": false, "state": "active"}]),
        "{created}"
    );
    assert_eq!(
        created["author"],
        json!({"id": api.alice, "name": "alice", "is_bot": false, "state": "active"})
    );
    // The same answer on every read.
    let listed = api.listed(item).await;
    assert_eq!(listed[0]["mentions"], created["mentions"], "{listed:?}");
    assert_eq!(listed[0]["author"], created["author"]);
    let one = api
        .get(
            &format!("/api/v1/comments/{}", created["id"].as_str().expect("id")),
            api.bob,
        )
        .await;
    assert_eq!(one.data()["mentions"], created["mentions"], "{}", one.body);
    // Stored as {id, label}.
    let stored = api
        .db_count_text(
            "SELECT mentions::text AS n FROM comments WHERE id = $1",
            Uuid::parse_str(created["id"].as_str().expect("id")).expect("uuid"),
        )
        .await;
    assert_eq!(
        serde_json::from_str::<Value>(&stored).expect("json"),
        json!([{"id": dana, "label": "Dana Dee"}])
    );
    // Dana is told, by the name of the author.
    assert_eq!(
        api.mention_notices(dana).await,
        vec![(
            "mentioned_by:alice".to_string(),
            created["id"].as_str().map(str::to_string)
        )]
    );
    // Nobody who was not named is told, and the author's own name is not a notification.
    assert!(api.mention_notices(api.bob).await.is_empty());
    let own = api.comment(item, api.alice, "note to @alice", &[]).await;
    assert_eq!(own["mentions"].as_array().map(Vec::len), Some(1), "{own}");
    assert!(api.mention_notices(api.alice).await.is_empty());
    // A typed bot name is a mention too, shown as a bot.
    let bot = api.add_member("Helper", "bot").await;
    let with_bot = api.comment(item, api.alice, "@Helper take a look", &[]).await;
    assert_eq!(
        with_bot["mentions"],
        json!([{"id": bot, "name": "Helper", "is_bot": true, "state": "active"}])
    );
    api.finish().await;
}

#[tokio::test]
async fn ambiguous_name_is_not_resolved() {
    let Some(api) = Api::new("mention_ambiguous").await else {
        return;
    };
    let first = api.add_member("Sam", "human").await;
    let second = api.add_member("sam", "human").await;
    let item = api.create_item("ambiguous", json!({})).await;
    let typed = api.comment(item, api.alice, "@Sam can you look?", &[]).await;
    assert_eq!(typed["mentions"], json!([]), "{typed}");
    assert!(api.mention_notices(first).await.is_empty());
    assert!(api.mention_notices(second).await.is_empty());
    // Picking one settles it.
    let picked = api.comment(item, api.alice, "@Sam can you look?", &[second]).await;
    assert_eq!(picked["mentions"][0]["id"], json!(second), "{picked}");
    assert_eq!(picked["mentions"].as_array().map(Vec::len), Some(1));
    assert_eq!(api.mention_notices(second).await.len(), 1);
    assert!(api.mention_notices(first).await.is_empty());
    // A person outside the workspace cannot be picked.
    let outsider = api.comment(item, api.alice, "hello", &[api.carol]).await;
    assert_eq!(outsider["mentions"], json!([]), "{outsider}");
    assert!(api.mention_notices(api.carol).await.is_empty());
    api.finish().await;
}

#[tokio::test]
async fn mentions_survive_rename_and_show_current_name() {
    let Some(api) = Api::new("mention_rename").await else {
        return;
    };
    let dana = api.add_member("Dana Dee", "human").await;
    let item = api.create_item("rename", json!({})).await;
    api.comment(item, api.alice, "@Dana Dee", &[]).await;
    api.rename(dana, "Dana Renamed").await;
    let listed = api.listed(item).await;
    assert_eq!(listed[0]["mentions"][0]["name"], json!("Dana Renamed"), "{listed:?}");
    assert_eq!(listed[0]["mentions"][0]["id"], json!(dana));
    // The label keeps what the text said when it was written.
    let stored = api
        .db_count_text("SELECT mentions::text AS n FROM comments WHERE work_item_id = $1", item)
        .await;
    assert_eq!(
        serde_json::from_str::<Value>(&stored).expect("json"),
        json!([{"id": dana, "label": "Dana Dee"}])
    );
    api.finish().await;
}

#[tokio::test]
async fn removed_user_has_state_removed() {
    let Some(api) = Api::new("mention_removed").await else {
        return;
    };
    let left = api.add_member("Lee Left", "human").await;
    let gone = api.add_member("Gil Gone", "human").await;
    let item = api.create_item("removed", json!({})).await;
    api.comment(item, api.alice, "@Lee Left and @Gil Gone", &[]).await;
    // One leaves the workspace; the other's account disappears altogether.
    exec(
        &api.db,
        "DELETE FROM workspace_members WHERE user_id = ANY($1)",
        vec![vec![left, gone].into()],
    )
    .await;
    exec(
        &api.db,
        "DELETE FROM notifications WHERE user_id = $1",
        vec![gone.into()],
    )
    .await;
    exec(&api.db, "DELETE FROM users WHERE id = $1", vec![gone.into()]).await;
    let listed = api.listed(item).await;
    let mentions = &listed[0]["mentions"];
    assert_eq!(
        mentions,
        &json!([
            {"id": left, "name": "Lee Left", "is_bot": false, "state": "removed"},
            // No account is left to read: the stored label stands in for the name.
            {"id": gone, "name": "Gil Gone", "is_bot": false, "state": "removed"},
        ]),
        "{listed:?}"
    );
    // The author who left is shown as removed too, with no id as a name.
    exec(
        &api.db,
        "DELETE FROM workspace_members WHERE user_id = $1",
        vec![api.bob.into()],
    )
    .await;
    let by_bob = api.comment_as_removed_author(item).await;
    assert_eq!(by_bob["author"]["state"], json!("removed"), "{by_bob}");
    assert_no_id_shown(&by_bob, "removed author");
    api.finish().await;
}

impl Api {
    /// Bob wrote a comment while a member; he is no longer one when it is read (by alice).
    async fn comment_as_removed_author(&self, issue: Uuid) -> Value {
        exec(
            &self.db,
            "INSERT INTO workspace_members (workspace_id, user_id, role) VALUES ($1, $2, 'member')",
            vec![self.ws.into(), self.bob.into()],
        )
        .await;
        self.comment(issue, self.bob, "from bob", &[]).await;
        exec(
            &self.db,
            "DELETE FROM workspace_members WHERE workspace_id = $1 AND user_id = $2",
            vec![self.ws.into(), self.bob.into()],
        )
        .await;
        let answer = self.get(&format!("/api/v1/issues/{issue}/comments"), self.alice).await;
        answer.data()["items"]
            .as_array()
            .expect("items")
            .iter()
            .find(|c| c["body"] == json!("from bob"))
            .expect("bob's comment")
            .clone()
    }
}

#[tokio::test]
async fn an_edit_tells_only_the_people_it_adds() {
    let Some(api) = Api::new("mention_edit").await else {
        return;
    };
    let dana = api.add_member("Dana Dee", "human").await;
    let item = api.create_item("edit", json!({})).await;
    let created = api.comment(item, api.alice, "@bob first", &[]).await;
    let id = created["id"].as_str().expect("id").to_string();
    let edited = api
        .put(
            &format!("/api/v1/comments/{id}"),
            api.alice,
            json!({"body": "@bob and @Dana Dee", "mentions": []}),
        )
        .await;
    assert_eq!(edited.code(), Some(0), "{}", edited.body);
    let names: Vec<&str> = edited.data()["mentions"]
        .as_array()
        .expect("mentions")
        .iter()
        .filter_map(|m| m["name"].as_str())
        .collect();
    assert_eq!(names, ["bob", "Dana Dee"], "{}", edited.body);
    assert_eq!(api.mention_notices(api.bob).await.len(), 1, "bob is told once");
    assert_eq!(api.mention_notices(dana).await.len(), 1);
    // Taking a name out of the text takes the mention out.
    let trimmed = api
        .put(
            &format!("/api/v1/comments/{id}"),
            api.alice,
            json!({"body": "only @bob now"}),
        )
        .await;
    assert_eq!(
        trimmed.data()["mentions"].as_array().map(Vec::len),
        Some(1),
        "{}",
        trimmed.body
    );
    api.finish().await;
}

#[tokio::test]
async fn blank_name_actor_leaves_the_content_without_a_name_and_no_id() {
    let Some(api) = Api::new("mention_blank_actor").await else {
        return;
    };
    api.rename(api.alice, "").await;
    let item = api.create_item("blank", json!({})).await;
    let created = api.comment(item, api.alice, "hi @bob", &[]).await;
    assert_eq!(
        api.mention_notices(api.bob).await,
        vec![("mentioned_by:".to_string(), created["id"].as_str().map(str::to_string))]
    );
    // The author has no name: the answer says so with null, never with an id.
    assert_eq!(
        created["author"],
        json!({"id": api.alice, "name": null, "is_bot": false, "state": "active"}),
        "{created}"
    );
    assert_no_id_shown(&created, "blank author");
    // The inbox row carries the key only.
    let titles = api
        .db
        .query_all(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT title FROM notifications WHERE user_id = $1 AND kind = 'mentioned'",
            vec![api.bob.into()],
        ))
        .await
        .expect("titles");
    for row in titles {
        let title: String = row.try_get("", "title").expect("title");
        assert!(!has_id(&title), "{title}");
    }
    api.finish().await;
}

#[tokio::test]
async fn webhook_mentions_are_id_and_name_pairs() {
    let Some(api) = Api::new("mention_webhook").await else {
        return;
    };
    api.add_webhook_for(r#"["comment.created","comment.updated","comment.deleted"]"#)
        .await;
    let dana = api.add_member("Dana Dee", "human").await;
    let item = api.create_item("hooks", json!({})).await;
    let created = api.comment(item, api.alice, "hello @Dana Dee", &[]).await;
    let id = created["id"].as_str().expect("id").to_string();
    let payloads = api.payloads("comment.created", 1).await;
    assert_eq!(
        payloads[0]["data"]["mentions"],
        json!([{"id": dana, "name": "Dana Dee"}]),
        "{}",
        payloads[0]
    );
    assert_eq!(payloads[0]["data"]["comment"]["author_name"], json!("alice"));
    let edited = api
        .put(
            &format!("/api/v1/comments/{id}"),
            api.alice,
            json!({"body": "hello @Dana Dee again"}),
        )
        .await;
    assert_eq!(edited.code(), Some(0), "{}", edited.body);
    let updated = api.payloads("comment.updated", 1).await;
    assert_eq!(
        updated[0]["data"]["mentions"],
        json!([{"id": dana, "name": "Dana Dee"}]),
        "{}",
        updated[0]
    );
    let deleted = api
        .call(
            axum::http::Method::DELETE,
            &format!("/api/v1/comments/{id}"),
            api.alice,
            None,
        )
        .await;
    assert_eq!(deleted.code(), Some(0), "{}", deleted.body);
    let gone = api.payloads("comment.deleted", 1).await;
    assert_eq!(
        gone[0]["data"]["mentions"],
        json!([{"id": dana, "name": "Dana Dee"}]),
        "{}",
        gone[0]
    );
    api.finish().await;
}

#[tokio::test]
async fn issue_activities_carry_the_people_they_name() {
    let Some(api) = Api::new("mention_activity_people").await else {
        return;
    };
    let item = api.create_item("assigned", json!({})).await;
    let gone = api.add_member("Gil Gone", "human").await;
    let bot = api.add_member("Helper", "bot").await;
    for assignee in [json!(gone), json!(bot), json!(null)] {
        let answer = api
            .put(
                &format!("/api/v1/issues/{item}"),
                api.alice,
                json!({"assignee_id": assignee}),
            )
            .await;
        assert_eq!(answer.code(), Some(0), "{}", answer.body);
    }
    exec(
        &api.db,
        "DELETE FROM workspace_members WHERE user_id = $1",
        vec![gone.into()],
    )
    .await;
    let answer = api.get(&format!("/api/v1/issues/{item}/activities"), api.bob).await;
    assert_eq!(answer.code(), Some(0), "{}", answer.body);
    let people = &answer.data()["people"];
    assert_eq!(people[api.alice.to_string()]["name"], json!("alice"), "{}", answer.body);
    assert_eq!(people[api.alice.to_string()]["state"], json!("active"));
    assert_eq!(people[bot.to_string()]["is_bot"], json!(true), "{}", answer.body);
    assert_eq!(people[bot.to_string()]["name"], json!("Helper"));
    assert_eq!(people[gone.to_string()]["state"], json!("removed"), "{}", answer.body);
    assert!(answer.data()["items"].as_array().is_some_and(|items| !items.is_empty()));
    assert_no_id_shown(people, "activity people");
    api.finish().await;
}

#[tokio::test]
async fn proposal_comments_resolve_typed_names_and_show_people() {
    let Some(api) = Api::new("mention_proposal").await else {
        return;
    };
    let proposal = Uuid::new_v4().to_string();
    exec(
        &api.db,
        "INSERT INTO proposals (id, title, proposal_type, status, author_id, author_type, content, workspace_id) \
         VALUES ($1, 'a proposal', 'feature', 'draft', $2, 'human', 'body', $3)",
        vec![proposal.clone().into(), api.alice.to_string().into(), api.ws.into()],
    )
    .await;
    let created = api
        .post(
            &format!("/api/v1/proposals/{proposal}/comments"),
            api.alice,
            json!({"content": "what do you think, @bob?"}),
        )
        .await;
    assert_eq!(created.code(), Some(0), "{}", created.body);
    assert_eq!(
        created.data()["mentions"],
        json!([{"id": api.bob, "name": "bob", "is_bot": false, "state": "active"}]),
        "{}",
        created.body
    );
    assert_eq!(created.data()["author"]["name"], json!("alice"));
    let notices = api.mention_notices(api.bob).await;
    assert_eq!(notices, vec![("mentioned_by:alice".to_string(), None)]);
    let link: String = api
        .db_count_text(
            "SELECT link AS n FROM notifications WHERE user_id = $1 AND kind = 'mentioned'",
            api.bob,
        )
        .await;
    assert_eq!(link, format!("/proposals/{proposal}"));
    let listed = api
        .get(&format!("/api/v1/proposals/{proposal}/comments"), api.bob)
        .await;
    assert_eq!(listed.code(), Some(0), "{}", listed.body);
    assert_eq!(listed.data()["items"][0]["mentions"], created.data()["mentions"]);
    assert_eq!(listed.data()["items"][0]["author"], created.data()["author"]);
    assert_no_id_shown(listed.data(), "proposal comments");
    api.finish().await;
}

#[tokio::test]
async fn more_than_twenty_mentions_are_refused() {
    let Some(api) = Api::new("mention_cap").await else {
        return;
    };
    let item = api.create_item("cap", json!({})).await;
    let mut body = String::new();
    for n in 0..21 {
        let name = format!("member{n:02}x");
        api.add_member(&name, "human").await;
        body.push('@');
        body.push_str(&name);
        body.push(' ');
    }
    let answer = api
        .post(
            &format!("/api/v1/issues/{item}/comments"),
            api.alice,
            json!({"body": body}),
        )
        .await;
    assert_ne!(answer.code(), Some(0), "{}", answer.body);
    assert_eq!(api.listed(item).await.len(), 0);
    api.finish().await;
}
