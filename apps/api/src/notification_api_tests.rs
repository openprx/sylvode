//! Notification preferences, e-mail queuing, digests and the state/priority notifications, through
//! the real router and the notification helper, against a scratch `PostgreSQL` (skipped without
//! `OPENPR_TEST_DATABASE_URL`). Uses the fixture of `item_api_tests` (`alice` owns the workspace,
//! `bob` is a member).
#![allow(
    clippy::indexing_slicing,
    clippy::too_many_lines,
    clippy::similar_names,
    clippy::items_after_statements,
    clippy::doc_markdown
)]

use super::item_api_tests::{Api, exec};
use api::services::notifications::{self, NewNotification};
use axum::http::Method;
use platform::{
    config::{EmailConfig, EmailTls},
    timestamp::Timestamp,
};
use sea_orm::{ConnectionTrait, DbBackend, Statement};
use serde_json::{Value, json};
use uuid::Uuid;

const PUBLIC_URL: &str = "https://sv.example.com";

fn email_config(hour: u8) -> EmailConfig {
    EmailConfig {
        smtp_host: "smtp.example.com".to_string(),
        smtp_port: 587,
        tls: EmailTls::Starttls,
        username: None,
        password: None,
        from: "Sylvode <sylvode@example.com>".to_string(),
        ca_file: None,
        digest_hour_utc: hour,
        public_url: PUBLIC_URL.to_string(),
    }
}

impl Api {
    async fn set_prefs(&self, user: Uuid, prefs: Value) {
        exec(
            &self.db,
            "UPDATE users SET notification_prefs = $2::jsonb WHERE id = $1",
            vec![user.into(), prefs.to_string().into()],
        )
        .await;
    }

    async fn immediate(&self, user: Uuid, language: &str) {
        self.set_prefs(
            user,
            json!({"email_notification": true, "mention_only": false, "daily_digest": false, "language": language}),
        )
        .await;
    }

    async fn count(&self, sql: &str, user: Uuid) -> i64 {
        self.db_count(sql, user).await
    }

    async fn notify(&self, user: Uuid, kind: &str, content: &str, email: bool) -> Option<Uuid> {
        let link = format!(
            "/workspace/{}/projects/{}/issues/{}",
            self.ws,
            self.project,
            Uuid::new_v4()
        );
        notifications::create_with(
            &self.db,
            NewNotification {
                user_id: user,
                workspace_id: Some(self.ws),
                kind,
                title_key: "notification.test",
                content: content.to_string(),
                link: Some(link),
                related_issue_id: None,
                related_comment_id: None,
                related_project_id: None,
                payload: json!({}),
            },
            email.then_some(PUBLIC_URL),
        )
        .await
        .expect("create")
    }

    async fn inbox(&self, user: Uuid) -> i64 {
        self.count(
            "SELECT COUNT(*)::bigint AS n FROM notifications WHERE user_id = $1",
            user,
        )
        .await
    }

    async fn outbox(&self, user: Uuid) -> i64 {
        self.count(
            "SELECT COUNT(*)::bigint AS n FROM email_outbox WHERE user_id = $1",
            user,
        )
        .await
    }

    async fn outbox_rows(&self, user: Uuid) -> Vec<(String, String, String)> {
        self.db
            .query_all(Statement::from_sql_and_values(
                DbBackend::Postgres,
                "SELECT kind, subject, body_text FROM email_outbox WHERE user_id = $1 ORDER BY created_at, id",
                vec![user.into()],
            ))
            .await
            .expect("rows")
            .iter()
            .map(|row| {
                (
                    row.try_get("", "kind").expect("kind"),
                    row.try_get("", "subject").expect("subject"),
                    row.try_get("", "body_text").expect("body"),
                )
            })
            .collect()
    }

    async fn kinds(&self, user: Uuid) -> Vec<String> {
        self.db
            .query_all(Statement::from_sql_and_values(
                DbBackend::Postgres,
                "SELECT kind FROM notifications WHERE user_id = $1 ORDER BY created_at, id",
                vec![user.into()],
            ))
            .await
            .expect("rows")
            .iter()
            .map(|row| row.try_get("", "kind").expect("kind"))
            .collect()
    }
}

#[tokio::test]
async fn notification_respects_mention_only() {
    let Some(api) = Api::new("notif_mention_only").await else {
        return;
    };
    api.set_prefs(
        api.bob,
        json!({"email_notification": false, "mention_only": true, "daily_digest": false}),
    )
    .await;
    assert!(
        api.notify(api.bob, "assignment", "issue_assigned:T", false)
            .await
            .is_none()
    );
    assert!(
        api.notify(api.bob, "issue_state_changed", "issue_state_changed:T|a|b", false)
            .await
            .is_none()
    );
    assert!(
        api.notify(api.bob, "issue_priority_changed", "issue_priority_changed:T|a|b", false)
            .await
            .is_none()
    );
    assert!(
        api.notify(api.bob, "mentioned", "mentioned_by:Alice", false)
            .await
            .is_some()
    );
    assert!(
        api.notify(api.bob, "alert_opened", "alert_opened:HIGH:T", false)
            .await
            .is_some()
    );
    assert_eq!(api.kinds(api.bob).await, vec!["mentioned", "alert_opened"]);
    api.set_prefs(
        api.bob,
        json!({"email_notification": false, "mention_only": false, "daily_digest": false}),
    )
    .await;
    assert!(
        api.notify(api.bob, "assignment", "issue_assigned:T", false)
            .await
            .is_some()
    );
    api.finish().await;
}

#[tokio::test]
async fn immediate_email_is_queued_once() {
    let Some(api) = Api::new("notif_immediate").await else {
        return;
    };
    api.immediate(api.bob, "en").await;
    let id = api
        .notify(api.bob, "mentioned", "mentioned_by:Alice", true)
        .await
        .expect("created");
    let rows = api.outbox_rows(api.bob).await;
    assert_eq!(rows.len(), 1);
    let (kind, subject, body) = &rows[0];
    assert_eq!(kind, "notification");
    assert_eq!(subject, "[Sylvode] Alice mentioned you");
    assert!(body.contains("Open it: https://sv.example.com/workspace/"), "{body}");
    // The same notification is never queued twice.
    let again = api::services::email_text::notification_email(
        api::services::email_text::Language::En,
        "mentioned",
        "mentioned_by:Alice",
        None,
    );
    notifications::queue_email(&api.db, api.bob, "notification", Some(id), &again)
        .await
        .expect("ignored");
    assert_eq!(api.outbox(api.bob).await, 1);
    // E-mail off at runtime: the inbox row is written, nothing is queued.
    api.notify(api.bob, "mentioned", "mentioned_by:Alice", false)
        .await
        .expect("created");
    assert_eq!((api.inbox(api.bob).await, api.outbox(api.bob).await), (2, 1));
    // The language of the preferences picks the text.
    api.immediate(api.bob, "zh").await;
    api.notify(api.bob, "mentioned", "mentioned_by:Alice", true)
        .await
        .expect("created");
    let rows = api.outbox_rows(api.bob).await;
    assert_eq!(rows[1].1, "[Sylvode] Alice 提到了你");
    // The master switch off: no e-mail.
    api.set_prefs(
        api.bob,
        json!({"email_notification": false, "mention_only": false, "daily_digest": false}),
    )
    .await;
    api.notify(api.bob, "mentioned", "mentioned_by:Alice", true)
        .await
        .expect("created");
    assert_eq!(api.outbox(api.bob).await, 2);
    api.finish().await;
}

#[tokio::test]
async fn bots_get_no_email() {
    let Some(api) = Api::new("notif_bots").await else {
        return;
    };
    let bot = api.add_member("helper", "bot").await;
    api.immediate(bot, "en").await;
    assert!(
        api.notify(bot, "mentioned", "mentioned_by:Alice", true).await.is_some(),
        "the inbox row is kept"
    );
    assert_eq!(api.outbox(bot).await, 0);
    // A deactivated person gets none either.
    api.immediate(api.bob, "en").await;
    exec(
        &api.db,
        "UPDATE users SET is_active = false WHERE id = $1",
        vec![api.bob.into()],
    )
    .await;
    api.notify(api.bob, "mentioned", "mentioned_by:Alice", true).await;
    assert_eq!(api.outbox(api.bob).await, 0);
    api.finish().await;
}

#[tokio::test]
async fn digest_replaces_immediate_email() {
    let Some(api) = Api::new("notif_digest").await else {
        return;
    };
    // Stored defaults: e-mail on, digest on. Nothing is queued as notifications arrive.
    for index in 0..55 {
        api.notify(api.bob, "assignment", &format!("issue_assigned:Item {index}"), true)
            .await
            .expect("created");
    }
    assert_eq!(api.outbox(api.bob).await, 0);
    // Before today's boundary nothing is due.
    let early = Timestamp::parse_rfc3339("2031-01-01T10:00:00Z").expect("time");
    assert_eq!(
        notifications::queue_due_digests(&api.db, &email_config(23), early)
            .await
            .expect("digest"),
        0
    );
    // A user on immediate e-mail, one with the switch off and a bot are not on the digest.
    api.immediate(api.alice, "en").await;
    api.notify(api.alice, "mentioned", "mentioned_by:Bob", false)
        .await
        .expect("created");
    let bot = api.add_member("helper", "bot").await;
    api.notify(bot, "mentioned", "mentioned_by:Bob", false)
        .await
        .expect("created");
    // Past the boundary: one digest for bob, 50 listed and 5 counted.
    let now = Timestamp::now();
    assert_eq!(
        notifications::queue_due_digests(&api.db, &email_config(0), now)
            .await
            .expect("digest"),
        1
    );
    let rows = api.outbox_rows(api.bob).await;
    assert_eq!(rows.len(), 1);
    let (kind, subject, body) = &rows[0];
    assert_eq!(kind, "digest");
    assert_eq!(subject, "[Sylvode] Your daily summary: 55 unread notifications");
    assert_eq!(body.matches("- You were assigned: ").count(), 50, "{body}");
    assert!(body.contains("...and 5 more."), "{body}");
    assert!(body.contains("https://sv.example.com/workspace/"));
    assert_eq!((api.outbox(api.alice).await, api.outbox(bot).await), (0, 0));
    // Handled for today: a second tick queues nothing.
    assert_eq!(
        notifications::queue_due_digests(&api.db, &email_config(0), now)
            .await
            .expect("digest"),
        0
    );
    assert_eq!(api.outbox(api.bob).await, 1);
    // Nothing unread since the last digest: no e-mail, and the time still moves.
    exec(
        &api.db,
        "UPDATE users SET last_digest_at = now() - interval '2 days' WHERE id = $1",
        vec![api.bob.into()],
    )
    .await;
    exec(
        &api.db,
        "UPDATE notifications SET is_read = true WHERE user_id = $1",
        vec![api.bob.into()],
    )
    .await;
    assert_eq!(
        notifications::queue_due_digests(&api.db, &email_config(0), now)
            .await
            .expect("digest"),
        0
    );
    assert_eq!(api.outbox(api.bob).await, 1);
    let moved = api
        .db_count(
            "SELECT (last_digest_at > now() - interval '1 minute')::int::bigint AS n FROM users WHERE id = $1",
            api.bob,
        )
        .await;
    assert_eq!(moved, 1);
    api.finish().await;
}

async fn assign_to_bob(api: &Api) -> Uuid {
    api.create_item("Watched", json!({"assignee_id": api.bob})).await
}

fn update(item: Uuid) -> String {
    format!("/api/v1/issues/{item}")
}

#[tokio::test]
async fn state_change_notifies_the_assignee() {
    let Some(api) = Api::new("notif_state").await else {
        return;
    };
    let item = assign_to_bob(&api).await;
    let before = api.inbox(api.bob).await;
    let answer = api.put(&update(item), api.alice, json!({"state": "in_progress"})).await;
    assert_eq!(answer.code(), Some(0), "{}", answer.body);
    assert_eq!(api.inbox(api.bob).await, before + 1);
    let row = api
        .db
        .query_one(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT kind, title, content, link, related_issue_id, payload::text AS payload FROM notifications \
             WHERE user_id = $1 AND kind = 'issue_state_changed'",
            vec![api.bob.into()],
        ))
        .await
        .expect("query")
        .expect("the notification");
    assert_eq!(
        row.try_get::<String>("", "title").expect("title"),
        "notification.issueStateChangedTitle"
    );
    let content: String = row.try_get("", "content").expect("content");
    assert!(content.starts_with("issue_state_changed:Watched|"), "{content}");
    assert!(content.ends_with("|in_progress"), "{content}");
    assert_eq!(row.try_get::<Uuid>("", "related_issue_id").expect("issue"), item);
    assert!(
        row.try_get::<String>("", "link")
            .expect("link")
            .ends_with(&item.to_string())
    );
    // Writing the same state again changes nothing and notifies nobody.
    let again = api.put(&update(item), api.alice, json!({"state": "in_progress"})).await;
    assert_eq!(again.code(), Some(0), "{}", again.body);
    assert_eq!(api.inbox(api.bob).await, before + 1);
    api.finish().await;
}

#[tokio::test]
async fn priority_change_notifies_the_assignee() {
    let Some(api) = Api::new("notif_priority").await else {
        return;
    };
    let item = assign_to_bob(&api).await;
    let before = api.inbox(api.bob).await;
    let answer = api.put(&update(item), api.alice, json!({"priority": "urgent"})).await;
    assert_eq!(answer.code(), Some(0), "{}", answer.body);
    assert_eq!(
        api.kinds(api.bob).await.last().map(String::as_str),
        Some("issue_priority_changed")
    );
    assert_eq!(api.inbox(api.bob).await, before + 1);
    let content = api
        .db_count_text(
            "SELECT content AS n FROM notifications WHERE user_id = $1 AND kind = 'issue_priority_changed'",
            api.bob,
        )
        .await;
    assert!(
        content.starts_with("issue_priority_changed:Watched|") && content.ends_with("|urgent"),
        "{content}"
    );
    api.finish().await;
}

#[tokio::test]
async fn own_change_notifies_nobody() {
    let Some(api) = Api::new("notif_own").await else { return };
    let item = assign_to_bob(&api).await;
    let before = api.inbox(api.bob).await;
    // bob changes his own item: no notification for bob, and nobody else is told.
    let answer = api
        .put(
            &update(item),
            api.bob,
            json!({"state": "in_progress", "priority": "urgent"}),
        )
        .await;
    assert_eq!(answer.code(), Some(0), "{}", answer.body);
    assert_eq!(api.inbox(api.bob).await, before);
    assert_eq!(api.inbox(api.alice).await, 0);
    // An item without an assignee, and one assigned to a bot, notify nobody either.
    let unassigned = api.create_item("Nobody", json!({})).await;
    let answer = api
        .put(&update(unassigned), api.alice, json!({"state": "in_progress"}))
        .await;
    assert_eq!(answer.code(), Some(0), "{}", answer.body);
    let bot = api.add_member("helper", "bot").await;
    let bot_item = api.create_item("Bot's", json!({"assignee_id": bot})).await;
    let answer = api
        .put(&update(bot_item), api.alice, json!({"priority": "urgent"}))
        .await;
    assert_eq!(answer.code(), Some(0), "{}", answer.body);
    assert_eq!(api.inbox(bot).await, 0);
    assert_eq!(api.inbox(api.alice).await, 0);
    api.finish().await;
}

#[tokio::test]
async fn reassigning_while_changing_the_state_sends_the_assignment_only() {
    let Some(api) = Api::new("notif_reassign").await else {
        return;
    };
    let item = api.create_item("Moving", json!({})).await;
    let answer = api
        .put(
            &update(item),
            api.alice,
            json!({"assignee_id": api.bob, "state": "in_progress"}),
        )
        .await;
    assert_eq!(answer.code(), Some(0), "{}", answer.body);
    assert_eq!(api.kinds(api.bob).await, vec!["assignment"]);
    api.finish().await;
}

#[tokio::test]
async fn mention_only_suppresses_them() {
    let Some(api) = Api::new("notif_suppressed").await else {
        return;
    };
    let item = assign_to_bob(&api).await;
    api.set_prefs(
        api.bob,
        json!({"email_notification": false, "mention_only": true, "daily_digest": false}),
    )
    .await;
    let before = api.inbox(api.bob).await;
    let answer = api
        .put(
            &update(item),
            api.alice,
            json!({"state": "in_progress", "priority": "urgent"}),
        )
        .await;
    assert_eq!(answer.code(), Some(0), "{}", answer.body);
    assert_eq!(api.inbox(api.bob).await, before);
    api.finish().await;
}

#[tokio::test]
async fn preferences_answer_carries_the_language_and_the_availability_of_email() {
    let Some(api) = Api::new("notif_prefs_api").await else {
        return;
    };
    let uri = "/api/v1/auth/me/preferences";
    let initial = api.get(uri, api.bob).await;
    assert_eq!(initial.code(), Some(0), "{}", initial.body);
    assert_eq!(
        initial.data()["notification_prefs"],
        json!({"email_notification": true, "mention_only": false, "daily_digest": true, "language": "en"})
    );
    assert_eq!(
        initial.data()["email_available"],
        json!(false),
        "the test process has no [email]"
    );
    let saved = api
        .put(
            uri,
            api.bob,
            json!({"notification_prefs": {"email_notification": true, "mention_only": true, "daily_digest": false, "language": "zh"}}),
        )
        .await;
    assert_eq!(saved.code(), Some(0), "{}", saved.body);
    assert_eq!(saved.data()["notification_prefs"]["language"], json!("zh"));
    let reread = api.get(uri, api.bob).await;
    assert_eq!(reread.data()["notification_prefs"], saved.data()["notification_prefs"]);
    // The language is optional on the wire and defaults to English.
    let without = api
        .put(
            uri,
            api.bob,
            json!({"notification_prefs": {"email_notification": false, "mention_only": false, "daily_digest": false}}),
        )
        .await;
    assert_eq!(without.code(), Some(0), "{}", without.body);
    assert_eq!(without.data()["notification_prefs"]["language"], json!("en"));
    // An unknown language or key is refused and stores nothing.
    for bad in [
        json!({"notification_prefs": {"email_notification": true, "mention_only": false, "daily_digest": false, "language": "fr"}}),
        json!({"notification_prefs": {"email_notification": true, "mention_only": false, "daily_digest": false, "extra": 1}}),
    ] {
        let refused = api.put(uri, api.bob, bad).await;
        assert!(refused.status.is_client_error(), "{} {}", refused.status, refused.body);
    }
    let kept = api.get(uri, api.bob).await;
    assert_eq!(kept.data()["notification_prefs"]["language"], json!("en"));
    api.finish().await;
}

#[tokio::test]
async fn the_test_email_endpoint_is_for_instance_admins_and_needs_e_mail() {
    let Some(api) = Api::new("notif_test_mail").await else {
        return;
    };
    let uri = "/api/v1/admin/email/test";
    let denied = api.call(Method::POST, uri, api.bob, Some(json!({}))).await;
    assert_eq!(denied.code(), Some(403), "{}", denied.body);
    exec(
        &api.db,
        "UPDATE users SET role = 'admin' WHERE id = $1",
        vec![api.alice.into()],
    )
    .await;
    let off = api.call(Method::POST, uri, api.alice, Some(json!({}))).await;
    assert_eq!(off.code(), Some(409), "{}", off.body);
    assert_eq!(off.body["error_code"], json!("feature_disabled"));
    assert_eq!(api.outbox(api.alice).await, 0);
    // What the handler queues once e-mail is on: a test message in the caller's language.
    api.set_prefs(
        api.alice,
        json!({"email_notification": true, "mention_only": false, "daily_digest": true, "language": "zh"}),
    )
    .await;
    api::routes::email_admin::queue_test_email(&api.db, api.alice)
        .await
        .expect("queued");
    let rows = api.outbox_rows(api.alice).await;
    assert_eq!(rows.len(), 1);
    assert_eq!((rows[0].0.as_str(), rows[0].1.as_str()), ("test", "[Sylvode] 测试邮件"));
    api.finish().await;
}

#[tokio::test]
async fn deleting_a_notification_or_a_user_removes_the_queued_email() {
    let Some(api) = Api::new("notif_cascade").await else {
        return;
    };
    api.immediate(api.bob, "en").await;
    let id = api
        .notify(api.bob, "mentioned", "mentioned_by:Alice", true)
        .await
        .expect("created");
    assert_eq!(api.outbox(api.bob).await, 1);
    exec(&api.db, "DELETE FROM notifications WHERE id = $1", vec![id.into()]).await;
    assert_eq!(api.outbox(api.bob).await, 0);
    api::routes::email_admin::queue_test_email(&api.db, api.bob)
        .await
        .expect("queued");
    assert_eq!(api.outbox(api.bob).await, 1);
    exec(&api.db, "DELETE FROM users WHERE id = $1", vec![api.bob.into()]).await;
    let left = api
        .db
        .query_one(Statement::from_string(
            DbBackend::Postgres,
            "SELECT COUNT(*)::bigint AS n FROM email_outbox".to_string(),
        ))
        .await
        .expect("count")
        .expect("row")
        .try_get::<i64>("", "n")
        .expect("n");
    assert_eq!(left, 0);
    api.finish().await;
}
