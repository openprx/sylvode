//! Creation of a work item row, shared by `create_issue` and the alert chain.

use platform::timestamp::Timestamp;
use sea_orm::{ConnectionTrait, DbBackend, DbErr, Statement};
use uuid::Uuid;

/// The columns a new work item is created with. Everything else takes the table's defaults, the
/// per-project sequence number included.
pub struct NewWorkItem<'a> {
    pub id: Uuid,
    pub project_id: Uuid,
    pub title: &'a str,
    pub description: &'a str,
    pub state: &'a str,
    pub priority: &'a str,
    pub assignee_id: Option<Uuid>,
    pub due_at: Option<Timestamp>,
    pub created_by: Uuid,
    pub sprint_id: Option<Uuid>,
    pub now: Timestamp,
}

/// Inserts one work item. The caller has already validated every value against the project.
pub async fn insert_work_item<C: ConnectionTrait>(db: &C, item: &NewWorkItem<'_>) -> Result<(), DbErr> {
    db.execute(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "INSERT INTO work_items (id, project_id, title, description, state, priority, assignee_id, due_at, \
         created_by, created_at, updated_at, sprint_id) \
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12)",
        vec![
            item.id.into(),
            item.project_id.into(),
            item.title.into(),
            item.description.into(),
            item.state.into(),
            item.priority.into(),
            item.assignee_id.into(),
            item.due_at.into(),
            item.created_by.into(),
            item.now.into(),
            item.now.into(),
            item.sprint_id.into(),
        ],
    ))
    .await?;
    Ok(())
}
