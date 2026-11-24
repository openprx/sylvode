//! The one place a notification is created.
//!
//! [`create`] applies the recipient's preferences, writes the inbox row and, when e-mail delivery
//! is set up and the recipient wants an immediate e-mail, queues the message in the e-mail outbox
//! in the same transaction. The worker sends from the outbox (see `apps/worker/src/email.rs`), so a
//! notification never waits for a mail server and a rolled-back change never sends an e-mail.
//!
//! No other code inserts into `notifications`.

use platform::{config::EmailConfig, timestamp::Timestamp};
use sea_orm::{ConnectionTrait, DatabaseConnection, DbBackend, FromQueryResult, Statement, TransactionTrait};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;

use super::email_text::{self, DigestEntry, EmailContent, Language};
use crate::error::ApiError;

/// A person was assigned a work item.
pub const ASSIGNMENT: &str = "assignment";
/// A person was mentioned.
pub const MENTIONED: &str = "mentioned";
/// The state of a work item changed.
pub const ISSUE_STATE_CHANGED: &str = "issue_state_changed";
/// The priority of a work item changed.
pub const ISSUE_PRIORITY_CHANGED: &str = "issue_priority_changed";

/// Notifications listed in one digest; the rest is counted.
pub const DIGEST_MAX_ENTRIES: i64 = 50;
/// Users one digest tick handles; the next tick takes the rest.
const DIGEST_BATCH: i64 = 100;

/// What a user chose about notifications (`users.notification_prefs`).
///
/// `email_notification` is the master switch of e-mail. With it on, `daily_digest` replaces the
/// immediate e-mails by one e-mail a day. `mention_only` limits notifications, in the inbox and by
/// e-mail, to mentions and alerts. `language` is the language of the e-mails.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NotificationPreferences {
    pub email_notification: bool,
    pub mention_only: bool,
    pub daily_digest: bool,
    #[serde(default)]
    pub language: Language,
}

impl Default for NotificationPreferences {
    /// The values a new account is stored with (`migrations/0034`), in English.
    fn default() -> Self {
        Self {
            email_notification: true,
            mention_only: false,
            daily_digest: true,
            language: Language::En,
        }
    }
}

impl NotificationPreferences {
    /// Reads a stored value; anything unreadable counts as the defaults.
    pub fn from_stored(value: Value) -> Self {
        serde_json::from_value(value).unwrap_or_default()
    }

    /// Whether a notification of `kind` is created at all.
    fn admits(&self, kind: &str) -> bool {
        !self.mention_only || kind == MENTIONED || kind.starts_with("alert_")
    }

    /// Whether each notification is e-mailed as it is created.
    const fn wants_immediate_email(&self) -> bool {
        self.email_notification && !self.daily_digest
    }
}

/// Everything a notification is made of.
#[derive(Debug, Clone)]
pub struct NewNotification<'a> {
    pub user_id: Uuid,
    pub workspace_id: Option<Uuid>,
    /// Stored as the notification's type and kind.
    pub kind: &'a str,
    /// The i18n key of the title shown in the inbox.
    pub title_key: &'a str,
    /// `key:argument`, read by the inbox and by the e-mail text.
    pub content: String,
    /// Site-relative link (starts with `/`).
    pub link: Option<String>,
    pub related_issue_id: Option<Uuid>,
    pub related_comment_id: Option<Uuid>,
    pub related_project_id: Option<Uuid>,
    pub payload: Value,
}

#[derive(FromQueryResult)]
struct Recipient {
    entity_type: String,
    email: String,
    is_active: bool,
    notification_prefs: Value,
}

/// Creates a notification for a user and queues its e-mail when one is due.
///
/// Returns `None` when nothing was created: the user does not exist or chose `mention_only` and
/// the kind is neither a mention nor an alert. E-mail is queued only when `[email]` is enabled,
/// the user is a human with an active account and an address, and the preferences ask for an
/// immediate e-mail. Bots never get e-mail.
///
/// # Errors
/// A database failure.
pub async fn create<C: ConnectionTrait>(db: &C, notification: NewNotification<'_>) -> Result<Option<Uuid>, ApiError> {
    let email = crate::config::runtime().email.as_ref();
    create_with(db, notification, email.map(|email| email.public_url.as_str())).await
}

/// [`create`] with the e-mail setting given: `Some(public_url)` when e-mail is enabled.
pub async fn create_with<C: ConnectionTrait>(
    db: &C,
    notification: NewNotification<'_>,
    email_public_url: Option<&str>,
) -> Result<Option<Uuid>, ApiError> {
    let Some(recipient) = Recipient::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "SELECT entity_type, email, is_active, notification_prefs FROM users WHERE id = $1",
        vec![notification.user_id.into()],
    ))
    .one(db)
    .await?
    else {
        return Ok(None);
    };
    let prefs = NotificationPreferences::from_stored(recipient.notification_prefs);
    if !prefs.admits(notification.kind) {
        return Ok(None);
    }
    let id = Uuid::new_v4();
    db.execute(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "INSERT INTO notifications (id, user_id, workspace_id, type, kind, payload, title, content, link, \
         related_issue_id, related_comment_id, related_project_id, is_read, created_at) \
         VALUES ($1, $2, $3, $4, $4, $5::jsonb, $6, $7, $8, $9, $10, $11, false, now())",
        vec![
            id.into(),
            notification.user_id.into(),
            notification.workspace_id.into(),
            notification.kind.to_owned().into(),
            notification.payload.clone().into(),
            notification.title_key.to_owned().into(),
            notification.content.clone().into(),
            notification.link.clone().into(),
            notification.related_issue_id.into(),
            notification.related_comment_id.into(),
            notification.related_project_id.into(),
        ],
    ))
    .await?;

    let wants_email = recipient.entity_type == "human"
        && recipient.is_active
        && !recipient.email.trim().is_empty()
        && prefs.wants_immediate_email();
    if let (true, Some(public_url)) = (wants_email, email_public_url) {
        let url = notification
            .link
            .as_deref()
            .and_then(|link| email_text::absolute_link(public_url, link));
        let mail =
            email_text::notification_email(prefs.language, notification.kind, &notification.content, url.as_deref());
        queue_email(db, notification.user_id, "notification", Some(id), &mail).await?;
    }
    Ok(Some(id))
}

/// Puts a message into the outbox. One message per notification: a repeat is ignored.
///
/// # Errors
/// A database failure.
pub async fn queue_email<C: ConnectionTrait>(
    db: &C,
    user_id: Uuid,
    kind: &str,
    notification_id: Option<Uuid>,
    mail: &EmailContent,
) -> Result<(), ApiError> {
    db.execute(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "INSERT INTO email_outbox (user_id, kind, notification_id, subject, body_text) \
         VALUES ($1, $2, $3, $4, $5) \
         ON CONFLICT (notification_id) WHERE notification_id IS NOT NULL DO NOTHING",
        vec![
            user_id.into(),
            kind.to_owned().into(),
            notification_id.into(),
            mail.subject.clone().into(),
            mail.body.clone().into(),
        ],
    ))
    .await?;
    Ok(())
}

/// The language of a user's e-mails (English when unreadable or unknown).
///
/// # Errors
/// A database failure.
pub async fn language_of<C: ConnectionTrait>(db: &C, user_id: Uuid) -> Result<Language, ApiError> {
    #[derive(FromQueryResult)]
    struct Row {
        notification_prefs: Value,
    }
    let row = Row::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "SELECT notification_prefs FROM users WHERE id = $1",
        vec![user_id.into()],
    ))
    .one(db)
    .await?;
    Ok(row.map_or_else(Language::default, |row| {
        NotificationPreferences::from_stored(row.notification_prefs).language
    }))
}

#[derive(FromQueryResult)]
struct DigestCandidate {
    id: Uuid,
    notification_prefs: Value,
}

#[derive(FromQueryResult)]
struct DigestItem {
    kind: String,
    content: String,
    link: Option<String>,
}

#[derive(FromQueryResult)]
struct CountRow {
    count: i64,
}

/// The users due a digest at `now`: human, active, with an address, `email_notification` and
/// `daily_digest` on, whose last digest is before today's boundary (`digest_hour_utc`) that `now`
/// has passed. The boundary is computed in UTC.
const DIGEST_DUE_FILTER: &str = "u.entity_type = 'human' AND u.is_active AND btrim(u.email) <> '' \
     AND u.notification_prefs->>'email_notification' = 'true' AND u.notification_prefs->>'daily_digest' = 'true' \
     AND $1::timestamptz >= bound.at AND (u.last_digest_at IS NULL OR u.last_digest_at < bound.at)";

const DIGEST_BOUNDARY: &str = "(SELECT (date_trunc('day', $1::timestamptz AT TIME ZONE 'UTC') + make_interval(hours => $2::int)) \
     AT TIME ZONE 'UTC' AS at) bound";

/// Queues the daily digest of every user who is due one at `now`; returns how many e-mails were
/// queued.
///
/// The unread notifications created since the user's last digest (since 24 hours before the
/// boundary for a first digest) are listed, up to [`DIGEST_MAX_ENTRIES`], the rest counted. A user
/// with nothing unread gets no e-mail and only the time of the last digest moves. The e-mail and
/// the new time are written in one transaction, and a user another worker holds is skipped, so
/// several workers can run this at once.
///
/// # Errors
/// A database failure.
pub async fn queue_due_digests(
    db: &DatabaseConnection,
    email: &EmailConfig,
    now: Timestamp,
) -> Result<usize, ApiError> {
    let hour = i32::from(email.digest_hour_utc);
    let candidates = DigestCandidate::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        format!(
            "SELECT u.id, u.notification_prefs FROM users u, {DIGEST_BOUNDARY} \
             WHERE {DIGEST_DUE_FILTER} ORDER BY u.id LIMIT {DIGEST_BATCH}"
        ),
        vec![now.into(), hour.into()],
    ))
    .all(db)
    .await?;
    let mut queued = 0;
    for candidate in candidates {
        let tx = db.begin().await?;
        // Re-check under a row lock: another worker may have handled this user since the list.
        let claimed = CountRow::find_by_statement(Statement::from_sql_and_values(
            DbBackend::Postgres,
            format!(
                "SELECT count(*) AS count FROM (SELECT u.id FROM users u, {DIGEST_BOUNDARY} \
                 WHERE u.id = $3 AND {DIGEST_DUE_FILTER} FOR UPDATE OF u SKIP LOCKED) due"
            ),
            vec![now.into(), hour.into(), candidate.id.into()],
        ))
        .one(&tx)
        .await?
        .is_some_and(|row| row.count == 1);
        if !claimed {
            tx.rollback().await?;
            continue;
        }
        let since = "COALESCE((SELECT last_digest_at FROM users WHERE id = $1), \
                     ((date_trunc('day', $2::timestamptz AT TIME ZONE 'UTC') + make_interval(hours => $3::int)) \
                     AT TIME ZONE 'UTC') - interval '1 day')";
        let items = DigestItem::find_by_statement(Statement::from_sql_and_values(
            DbBackend::Postgres,
            format!(
                "SELECT type AS kind, content, link FROM notifications \
                 WHERE user_id = $1 AND NOT is_read AND created_at > {since} \
                 ORDER BY created_at DESC, id LIMIT {DIGEST_MAX_ENTRIES}"
            ),
            vec![candidate.id.into(), now.into(), hour.into()],
        ))
        .all(&tx)
        .await?;
        if !items.is_empty() {
            let total = CountRow::find_by_statement(Statement::from_sql_and_values(
                DbBackend::Postgres,
                format!(
                    "SELECT count(*) AS count FROM notifications \
                     WHERE user_id = $1 AND NOT is_read AND created_at > {since}"
                ),
                vec![candidate.id.into(), now.into(), hour.into()],
            ))
            .one(&tx)
            .await?
            .map_or(0, |row| row.count);
            let urls: Vec<Option<String>> = items
                .iter()
                .map(|item| {
                    item.link
                        .as_deref()
                        .and_then(|link| email_text::absolute_link(&email.public_url, link))
                })
                .collect();
            let entries: Vec<DigestEntry<'_>> = items
                .iter()
                .zip(&urls)
                .map(|(item, url)| DigestEntry {
                    kind: &item.kind,
                    content: &item.content,
                    url: url.as_deref(),
                })
                .collect();
            let listed = i64::try_from(entries.len()).unwrap_or(DIGEST_MAX_ENTRIES);
            let more = usize::try_from((total - listed).max(0)).unwrap_or(0);
            let language = NotificationPreferences::from_stored(candidate.notification_prefs).language;
            let mail = email_text::digest_email(language, &entries, more);
            queue_email(&tx, candidate.id, "digest", None, &mail).await?;
            queued += 1;
        }
        tx.execute(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "UPDATE users SET last_digest_at = $2 WHERE id = $1",
            vec![candidate.id.into(), now.into()],
        ))
        .await?;
        tx.commit().await?;
    }
    Ok(queued)
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn stored_preferences_without_a_language_read_as_english() {
        let prefs = NotificationPreferences::from_stored(json!({
            "email_notification": true, "mention_only": false, "daily_digest": true
        }));
        assert_eq!(prefs, NotificationPreferences::default());
        let zh = NotificationPreferences::from_stored(json!({
            "email_notification": false, "mention_only": true, "daily_digest": false, "language": "zh"
        }));
        assert_eq!(zh.language, Language::Zh);
        assert!(zh.mention_only && !zh.email_notification);
    }

    #[test]
    fn unreadable_preferences_count_as_the_defaults() {
        assert_eq!(
            NotificationPreferences::from_stored(json!({"bogus": 1})),
            NotificationPreferences::default()
        );
        assert_eq!(
            NotificationPreferences::from_stored(Value::Null),
            NotificationPreferences::default()
        );
    }

    #[test]
    fn the_preferences_request_refuses_unknown_keys_and_languages() {
        assert!(
            serde_json::from_value::<NotificationPreferences>(json!({
                "email_notification": true, "mention_only": false, "daily_digest": false, "extra": 1
            }))
            .is_err()
        );
        assert!(
            serde_json::from_value::<NotificationPreferences>(json!({
                "email_notification": true, "mention_only": false, "daily_digest": false, "language": "fr"
            }))
            .is_err()
        );
    }

    #[test]
    fn mention_only_admits_mentions_and_alerts() {
        let prefs = NotificationPreferences {
            mention_only: true,
            ..NotificationPreferences::default()
        };
        assert!(prefs.admits(MENTIONED));
        assert!(prefs.admits("alert_opened"));
        assert!(prefs.admits("alert_source_throttled"));
        assert!(!prefs.admits(ASSIGNMENT));
        assert!(!prefs.admits(ISSUE_STATE_CHANGED));
        assert!(!prefs.admits(ISSUE_PRIORITY_CHANGED));
        assert!(NotificationPreferences::default().admits(ASSIGNMENT));
    }

    #[test]
    fn immediate_e_mail_needs_the_switch_on_and_the_digest_off() {
        let mut prefs = NotificationPreferences::default();
        assert!(!prefs.wants_immediate_email(), "the stored default is the digest");
        prefs.daily_digest = false;
        assert!(prefs.wants_immediate_email());
        prefs.email_notification = false;
        assert!(!prefs.wants_immediate_email());
    }
}
