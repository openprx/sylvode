//! Writing a comment on a work item, shared by the comment handler and the task reply routing.

use platform::timestamp::Timestamp;
use sea_orm::{ConnectionTrait, DbBackend, Statement};
use uuid::Uuid;

use crate::error::ApiError;

/// A comment that was just stored.
#[derive(Debug, Clone, Copy)]
pub struct InsertedComment {
    pub id: Uuid,
    pub created_at: Timestamp,
}

/// Inserts a comment row. It extracts no mentions and writes no notification and no task: what the
/// caller does with mentions, if anything, is the caller's business.
pub async fn insert_comment<C: ConnectionTrait>(
    db: &C,
    issue_id: Uuid,
    author_id: Uuid,
    body: &str,
) -> Result<InsertedComment, ApiError> {
    let id = Uuid::new_v4();
    let created_at = Timestamp::now();
    db.execute(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "INSERT INTO comments (id, work_item_id, author_id, body, created_at, updated_at) VALUES ($1, $2, $3, $4, $5, $6)",
        vec![
            id.into(),
            issue_id.into(),
            author_id.into(),
            body.to_string().into(),
            created_at.into(),
            created_at.into(),
        ],
    ))
    .await?;
    Ok(InsertedComment { id, created_at })
}
