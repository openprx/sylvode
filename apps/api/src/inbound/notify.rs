//! Notifications of the alert chain's milestones.
//!
//! Every row has `workspace_id` set, so the OAuth outbox trigger on `notifications` carries it to
//! a connected application unchanged. Content follows the existing `key:argument` convention and
//! contains Sylvode-built values only: the work item title of an alert is itself built from
//! Sylvode values (see `description`), never from the event's free text.

use sea_orm::{ConnectionTrait, DbBackend, FromQueryResult, Statement};
use serde_json::{Value, json};
use uuid::Uuid;

use super::{auth::SourceRow, severity::Severity};
use crate::{
    error::ApiError,
    services::notifications::{self, NewNotification},
};

/// The kinds of notification the alert chain writes. The wire `type` is [`Self::as_type`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AlertNotification {
    Opened,
    Escalated,
    TaskCompleted,
    TaskFailed,
    Suppressed,
    SourceThrottled,
}

impl AlertNotification {
    pub const fn as_type(self) -> &'static str {
        match self {
            Self::Opened => "alert_opened",
            Self::Escalated => "alert_escalated",
            Self::TaskCompleted => "alert_task_completed",
            Self::TaskFailed => "alert_task_failed",
            Self::Suppressed => "alert_suppressed",
            Self::SourceThrottled => "alert_source_throttled",
        }
    }

    /// The i18n key of the notification title.
    pub const fn title_key(self) -> &'static str {
        match self {
            Self::Opened => "notification.alertOpenedTitle",
            Self::Escalated => "notification.alertEscalatedTitle",
            Self::TaskCompleted => "notification.alertTaskCompletedTitle",
            Self::TaskFailed => "notification.alertTaskFailedTitle",
            Self::Suppressed => "notification.alertSuppressedTitle",
            Self::SourceThrottled => "notification.alertSourceThrottledTitle",
        }
    }
}

/// `alert_opened:<SEVERITY>:<work item title>`.
pub fn opened_content(severity: Severity, title: &str) -> String {
    format!("alert_opened:{}:{title}", severity.label())
}

/// `alert_escalated:<SEVERITY>:<work item title>`.
pub fn escalated_content(severity: Severity, title: &str) -> String {
    format!("alert_escalated:{}:{title}", severity.label())
}

/// `alert_task_completed:<title>` or `alert_task_failed:<title>`.
pub fn task_outcome_content(kind: AlertNotification, title: &str) -> String {
    format!("{}:{title}", kind.as_type())
}

/// The `payload` of an alert notification.
pub fn payload(
    alert_id: Option<Uuid>,
    severity: Option<Severity>,
    kind: Option<&str>,
    source_name: &str,
    task_id: Option<Uuid>,
) -> Value {
    json!({
        "alert_id": alert_id,
        "severity": severity.map(Severity::as_str),
        "kind": kind,
        "source_name": source_name,
        "task_id": task_id,
    })
}

#[derive(FromQueryResult)]
struct IdRow {
    id: Uuid,
}

/// The ids among `candidates` that are current human members of the workspace, in the order given.
pub async fn current_human_members<C: ConnectionTrait>(
    db: &C,
    workspace_id: Uuid,
    candidates: &[Uuid],
) -> Result<Vec<Uuid>, ApiError> {
    if candidates.is_empty() {
        return Ok(Vec::new());
    }
    let rows = IdRow::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "SELECT u.id FROM users u \
           JOIN workspace_members m ON m.user_id = u.id AND m.workspace_id = $1 \
          WHERE u.id = ANY($2) AND u.is_active AND u.entity_type = 'human'",
        vec![workspace_id.into(), candidates.to_vec().into()],
    ))
    .all(db)
    .await?;
    let members: Vec<Uuid> = rows.into_iter().map(|row| row.id).collect();
    Ok(candidates.iter().copied().filter(|id| members.contains(id)).collect())
}

/// Where a notification points.
pub struct Subject {
    pub workspace_id: Uuid,
    pub project_id: Uuid,
    pub work_item_id: Option<Uuid>,
}

/// Creates one notification of `kind` for each recipient that is a current human member of the
/// workspace. Returns how many were created (a recipient's preferences may decline one).
pub async fn send<C: ConnectionTrait>(
    db: &C,
    kind: AlertNotification,
    subject: &Subject,
    recipients: &[Uuid],
    content: &str,
    payload: &Value,
) -> Result<usize, ApiError> {
    let recipients = current_human_members(db, subject.workspace_id, recipients).await?;
    let link = subject.work_item_id.map(|issue| {
        format!(
            "/workspace/{}/projects/{}/issues/{issue}",
            subject.workspace_id, subject.project_id
        )
    });
    let mut written = 0;
    for user_id in &recipients {
        let created = notifications::create(
            db,
            NewNotification {
                user_id: *user_id,
                workspace_id: Some(subject.workspace_id),
                kind: kind.as_type(),
                title_key: kind.title_key(),
                content: content.to_owned(),
                link: link.clone(),
                related_issue_id: subject.work_item_id,
                related_comment_id: None,
                related_project_id: Some(subject.project_id),
                payload: payload.clone(),
            },
        )
        .await?;
        written += usize::from(created.is_some());
    }
    Ok(written)
}

/// Tells the person a source acts for that the source is being throttled, at most once an hour.
pub async fn source_throttled<C: ConnectionTrait>(db: &C, source: &SourceRow) -> Result<(), ApiError> {
    let Some(acting) = source.acting_user_id else {
        return Ok(());
    };
    let claimed = IdRow::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "UPDATE inbound_sources SET last_throttle_notice_at = now() \
          WHERE id = $1 AND (last_throttle_notice_at IS NULL OR last_throttle_notice_at < now() - interval '1 hour') \
          RETURNING id",
        vec![source.id.into()],
    ))
    .one(db)
    .await?;
    if claimed.is_none() {
        return Ok(());
    }
    send(
        db,
        AlertNotification::SourceThrottled,
        &Subject {
            workspace_id: source.workspace_id,
            project_id: source.project_id,
            work_item_id: None,
        },
        &[acting],
        &format!("alert_source_throttled:{}", source.name),
        &payload(None, None, None, &source.name, None),
    )
    .await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn types_and_title_keys_follow_the_documented_names() {
        for (kind, name, key) in [
            (
                AlertNotification::Opened,
                "alert_opened",
                "notification.alertOpenedTitle",
            ),
            (
                AlertNotification::Escalated,
                "alert_escalated",
                "notification.alertEscalatedTitle",
            ),
            (
                AlertNotification::TaskCompleted,
                "alert_task_completed",
                "notification.alertTaskCompletedTitle",
            ),
            (
                AlertNotification::TaskFailed,
                "alert_task_failed",
                "notification.alertTaskFailedTitle",
            ),
            (
                AlertNotification::Suppressed,
                "alert_suppressed",
                "notification.alertSuppressedTitle",
            ),
            (
                AlertNotification::SourceThrottled,
                "alert_source_throttled",
                "notification.alertSourceThrottledTitle",
            ),
        ] {
            assert_eq!(kind.as_type(), name);
            assert_eq!(kind.title_key(), key);
        }
    }

    #[test]
    fn content_uses_the_key_argument_convention() {
        assert_eq!(
            opened_content(Severity::High, "[HIGH] s · k"),
            "alert_opened:HIGH:[HIGH] s · k"
        );
        assert_eq!(
            escalated_content(Severity::Critical, "[CRITICAL] s · k"),
            "alert_escalated:CRITICAL:[CRITICAL] s · k"
        );
        assert_eq!(
            task_outcome_content(AlertNotification::TaskFailed, "t"),
            "alert_task_failed:t"
        );
    }
}
