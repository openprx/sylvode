//! Moving work items between sprints on behalf of a sprint change (completion, cancellation or
//! deletion): the writes happen inside the caller's transaction, the events after its commit.

use platform::{app::AppState, timestamp::Timestamp};
use sea_orm::{ConnectionTrait, DbBackend, FromQueryResult, Statement};
use serde_json::json;
use uuid::Uuid;

use crate::{
    error::ApiError,
    services::issues::{IssueRef, insert_activity},
    webhook_trigger::{TriggerContext, WebhookEvent, trigger_webhooks},
};

/// A work item that leaves the sprint `from`.
#[derive(Debug, Clone, Copy, FromQueryResult)]
pub struct MovedItem {
    pub id: Uuid,
    pub from: Uuid,
}

/// Where the moved items live and who moves them.
#[derive(Debug, Clone, Copy)]
pub struct MoveScope {
    pub workspace_id: Uuid,
    pub project_id: Uuid,
    pub actor_id: Uuid,
}

/// The items of `sprint` whose state is not one of `finished_state_keys` (finished items stay in
/// the sprint), in sequence order, locked until the transaction ends.
///
/// # Errors
/// A database failure.
pub async fn lock_unfinished_items(
    db: &impl ConnectionTrait,
    sprint: Uuid,
    finished_state_keys: &[String],
) -> Result<Vec<MovedItem>, ApiError> {
    Ok(MovedItem::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        r#"
            SELECT wi.id, wi.sprint_id AS "from"
            FROM work_items wi
            WHERE wi.sprint_id = $1 AND NOT (wi.state = ANY($2))
            ORDER BY wi.sequence_number
            FOR UPDATE
        "#,
        vec![sprint.into(), finished_state_keys.to_vec().into()],
    ))
    .all(db)
    .await?)
}

/// Every item of `sprint`, in sequence order, locked until the transaction ends.
///
/// # Errors
/// A database failure.
pub async fn lock_all_items(db: &impl ConnectionTrait, sprint: Uuid) -> Result<Vec<MovedItem>, ApiError> {
    lock_unfinished_items(db, sprint, &[]).await
}

/// Puts `items` into the sprint `to` (the backlog when `None`) and records one `sprint_changed`
/// activity for each.
///
/// # Errors
/// A database failure.
pub async fn move_items(
    db: &impl ConnectionTrait,
    scope: MoveScope,
    items: &[MovedItem],
    to: Option<Uuid>,
    now: Timestamp,
) -> Result<(), ApiError> {
    if items.is_empty() {
        return Ok(());
    }
    let ids: Vec<Uuid> = items.iter().map(|item| item.id).collect();
    db.execute(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "UPDATE work_items SET sprint_id = $1, updated_at = $2 WHERE id = ANY($3)",
        vec![to.into(), now.into(), ids.into()],
    ))
    .await?;
    for item in items {
        let issue = IssueRef {
            id: item.id,
            project_id: scope.project_id,
            workspace_id: scope.workspace_id,
        };
        insert_activity(
            db,
            &issue,
            scope.actor_id,
            "sprint_changed",
            &json!({"from": item.from.to_string(), "to": to.map(|id| id.to_string())}),
            now,
        )
        .await?;
    }
    Ok(())
}

/// Sends one `issue.updated` per moved item, in the order given, once the move is committed.
pub fn emit_moves(state: &AppState, scope: MoveScope, items: &[MovedItem], to: Option<Uuid>) {
    for item in items {
        trigger_webhooks(
            state.clone(),
            TriggerContext {
                event: WebhookEvent::IssueUpdated,
                workspace_id: scope.workspace_id,
                project_id: scope.project_id,
                actor_id: scope.actor_id,
                issue_id: Some(item.id),
                comment_id: None,
                label_id: None,
                sprint_id: None,
                changes: Some(json!({
                    "sprint_id": {
                        "old": item.from.to_string(),
                        "new": to.map(|id| id.to_string()),
                    }
                })),
                mentions: Vec::new(),
                extra_data: None,
            },
        );
    }
}
