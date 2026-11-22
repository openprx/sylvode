//! Work item changes shared by the issue handler and the task reply routing.

use platform::timestamp::Timestamp;
use sea_orm::{ConnectionTrait, DbBackend, FromQueryResult, Statement};
use serde_json::{Value, json};
use uuid::Uuid;

use crate::{
    error::ApiError,
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

/// Writes one row of the issue activity log.
pub async fn insert_activity<C: ConnectionTrait>(
    db: &C,
    issue: &IssueRef,
    actor_id: Uuid,
    action: &str,
    detail: &Value,
    at: Timestamp,
) -> Result<(), ApiError> {
    let detail_text = detail.to_string();
    db.execute(Statement::from_sql_and_values(
        DbBackend::Postgres,
        r"
            INSERT INTO activities (
                id, workspace_id, project_id, issue_id, user_id, action, detail, created_at,
                resource_type, resource_id, event_type, actor_id, payload
            )
            VALUES ($1, $2, $3, $4, $5, $6, $7::jsonb, $8, $9, $10, $11, $12, $13::jsonb)
        ",
        vec![
            Uuid::new_v4().into(),
            issue.workspace_id.into(),
            issue.project_id.into(),
            issue.id.into(),
            actor_id.into(),
            action.to_string().into(),
            detail_text.clone().into(),
            at.into(),
            "issue".into(),
            issue.id.into(),
            action.to_string().into(),
            actor_id.into(),
            detail_text.into(),
        ],
    ))
    .await?;
    Ok(())
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
