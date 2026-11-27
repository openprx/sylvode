//! Work item changes shared by the issue handler and the task reply routing.

use platform::timestamp::Timestamp;
use sea_orm::{ConnectionTrait, DbBackend, DbErr, FromQueryResult, Statement};
use serde_json::{Value, json};
use uuid::Uuid;

use crate::{
    error::ApiError,
    services::{
        bot_identity::is_bot_user,
        notifications::{self, NewNotification},
    },
    webhook_trigger::{TriggerContext, WebhookEvent},
};

/// The work item a change is about, with where it lives.
#[derive(Debug, Clone, Copy, FromQueryResult)]
pub struct IssueRef {
    pub id: Uuid,
    pub project_id: Uuid,
    pub workspace_id: Uuid,
}

/// A state change that was made; the trigger context is for the caller to send after commit.
#[derive(Debug)]
pub struct StateChange {
    pub from: String,
    pub to: String,
    pub trigger: TriggerContext,
}

/// What [`notify_assignment`] needs to know about an assignment.
#[derive(Debug, Clone, Copy)]
pub struct NotifyAssignment<'a> {
    pub workspace_id: Uuid,
    pub project_id: Uuid,
    pub issue_id: Uuid,
    pub title: &'a str,
    pub assignee_id: Uuid,
    pub actor_id: Uuid,
}

fn issue_link(workspace_id: Uuid, project_id: Uuid, issue_id: Uuid) -> String {
    format!("/workspace/{workspace_id}/projects/{project_id}/issues/{issue_id}")
}

/// Tells a human assignee that an issue was assigned to them.
///
/// Nothing is written when the assignee is the actor (your own actions never notify you) or a bot
/// (a bot is told through its task, not a notification). The recipient's preferences decide the
/// rest (see [`notifications::create`]).
///
/// # Errors
/// A database failure.
pub async fn notify_assignment<C: ConnectionTrait>(db: &C, input: NotifyAssignment<'_>) -> Result<(), ApiError> {
    if input.assignee_id == input.actor_id || is_bot_user(db, input.assignee_id).await? {
        return Ok(());
    }
    notifications::create(
        db,
        NewNotification {
            user_id: input.assignee_id,
            workspace_id: Some(input.workspace_id),
            kind: notifications::ASSIGNMENT,
            title_key: "notification.issueAssignedTitle",
            content: format!("issue_assigned:{}", input.title),
            link: Some(issue_link(input.workspace_id, input.project_id, input.issue_id)),
            related_issue_id: Some(input.issue_id),
            related_comment_id: None,
            related_project_id: None,
            payload: json!({}),
        },
    )
    .await?;
    Ok(())
}

/// What [`notify_state_change`] and [`notify_priority_change`] need to know about a change.
#[derive(Debug, Clone, Copy)]
pub struct NotifyChange<'a> {
    pub workspace_id: Uuid,
    pub project_id: Uuid,
    pub issue_id: Uuid,
    pub title: &'a str,
    pub assignee_id: Uuid,
    pub actor_id: Uuid,
    pub old: &'a str,
    pub new: &'a str,
}

/// Tells a human assignee that the state of their work item changed.
///
/// Nothing is written when the assignee made the change or is a bot.
///
/// # Errors
/// A database failure.
pub async fn notify_state_change<C: ConnectionTrait>(db: &C, input: NotifyChange<'_>) -> Result<(), ApiError> {
    notify_change(
        db,
        input,
        notifications::ISSUE_STATE_CHANGED,
        "notification.issueStateChangedTitle",
    )
    .await
}

/// Tells a human assignee that the priority of their work item changed.
///
/// Nothing is written when the assignee made the change or is a bot.
///
/// # Errors
/// A database failure.
pub async fn notify_priority_change<C: ConnectionTrait>(db: &C, input: NotifyChange<'_>) -> Result<(), ApiError> {
    notify_change(
        db,
        input,
        notifications::ISSUE_PRIORITY_CHANGED,
        "notification.issuePriorityChangedTitle",
    )
    .await
}

async fn notify_change<C: ConnectionTrait>(
    db: &C,
    input: NotifyChange<'_>,
    kind: &'static str,
    title_key: &'static str,
) -> Result<(), ApiError> {
    if input.assignee_id == input.actor_id || is_bot_user(db, input.assignee_id).await? {
        return Ok(());
    }
    notifications::create(
        db,
        NewNotification {
            user_id: input.assignee_id,
            workspace_id: Some(input.workspace_id),
            kind,
            title_key,
            content: format!("{kind}:{}|{}|{}", input.title, input.old, input.new),
            link: Some(issue_link(input.workspace_id, input.project_id, input.issue_id)),
            related_issue_id: Some(input.issue_id),
            related_comment_id: None,
            related_project_id: None,
            payload: json!({ "old": input.old, "new": input.new }),
        },
    )
    .await?;
    Ok(())
}

/// Writes one row of the issue activity log.
///
/// This is the only writer of the log: every change of a work item records its row here, in the
/// transaction that makes the change.
///
/// # Errors
/// A database failure.
pub async fn insert_activity<C: ConnectionTrait>(
    db: &C,
    issue: &IssueRef,
    actor_id: Uuid,
    action: &str,
    detail: &Value,
    at: Timestamp,
) -> Result<(), ApiError> {
    write_activity(db, issue.id, actor_id, action, detail, at).await?;
    Ok(())
}

/// Writes one activity row of a work item known by its id only; the project and the workspace are
/// read from the item in the same statement. Nothing is written for an id that is not a work item.
///
/// # Errors
/// A database failure.
pub async fn write_activity<C: ConnectionTrait>(
    db: &C,
    issue_id: Uuid,
    actor_id: Uuid,
    action: &str,
    detail: &Value,
    at: Timestamp,
) -> Result<(), DbErr> {
    db.execute(Statement::from_sql_and_values(
        DbBackend::Postgres,
        r"
            INSERT INTO activities (
                id, workspace_id, project_id, issue_id, user_id, action, detail, created_at,
                resource_type, resource_id, event_type, actor_id, payload
            )
            SELECT $1::uuid, p.workspace_id, wi.project_id, wi.id, $3::uuid, $4::text, $5::jsonb, $6::timestamptz,
                   'issue', wi.id, $4::text, $3::uuid, $5::jsonb
            FROM work_items wi
            INNER JOIN projects p ON p.id = wi.project_id
            WHERE wi.id = $2
        ",
        vec![
            Uuid::new_v4().into(),
            issue_id.into(),
            actor_id.into(),
            action.to_string().into(),
            detail.to_string().into(),
            at.into(),
        ],
    ))
    .await?;
    Ok(())
}

/// Records that a label was put on (`added`) or taken off a work item.
///
/// # Errors
/// A database failure.
pub async fn record_label_change<C: ConnectionTrait>(
    db: &C,
    issue_id: Uuid,
    actor_id: Uuid,
    label_id: Uuid,
    added: bool,
) -> Result<(), DbErr> {
    #[derive(FromQueryResult)]
    struct LabelName {
        name: String,
    }
    let name = LabelName::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "SELECT name FROM labels WHERE id = $1",
        vec![label_id.into()],
    ))
    .one(db)
    .await?
    .map(|row| row.name)
    .unwrap_or_default();
    write_activity(
        db,
        issue_id,
        actor_id,
        if added { "label_added" } else { "label_removed" },
        &json!({ "label_id": label_id, "label_name": name }),
        Timestamp::now(),
    )
    .await
}

/// Moves a work item to `state_key`, recording the activity. `None` when it is already there.
///
/// The key is not validated here: the caller has checked it against the project's workflow. A
/// state change never assigns the work item.
pub async fn set_state<C: ConnectionTrait>(
    db: &C,
    issue: &IssueRef,
    actor_id: Uuid,
    state_key: &str,
) -> Result<Option<StateChange>, ApiError> {
    #[derive(FromQueryResult)]
    struct Current {
        state: String,
    }
    let current = Current::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "SELECT state FROM work_items WHERE id = $1 FOR UPDATE",
        vec![issue.id.into()],
    ))
    .one(db)
    .await?
    .ok_or_else(|| ApiError::NotFound("issue not found".to_string()))?;
    if current.state == state_key {
        return Ok(None);
    }
    let now = Timestamp::now();
    db.execute(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "UPDATE work_items SET state = $2, updated_at = $3 WHERE id = $1",
        vec![issue.id.into(), state_key.to_string().into(), now.into()],
    ))
    .await?;
    insert_activity(
        db,
        issue,
        actor_id,
        "status_changed",
        &json!({ "from": current.state, "to": state_key }),
        now,
    )
    .await?;
    Ok(Some(StateChange {
        trigger: TriggerContext {
            event: WebhookEvent::IssueStateChanged,
            workspace_id: issue.workspace_id,
            project_id: issue.project_id,
            actor_id,
            issue_id: Some(issue.id),
            comment_id: None,
            label_id: None,
            sprint_id: None,
            changes: Some(json!({ "state": { "old": current.state, "new": state_key } })),
            mentions: Vec::new(),
            extra_data: None,
        },
        from: current.state,
        to: state_key.to_string(),
    }))
}
