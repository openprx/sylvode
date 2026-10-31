//! Comment threads on a Flow object.
//!
//! Listing needs `view`. Creating and replying need `comment`. An author with `comment` may edit
//! or delete their own row. Resolving a top-level thread needs `edit`, and so does deleting
//! someone else's row only when the caller has `full_access`. Replies are one level deep and stay
//! on the same object. Mentions are an explicit user-id list; only workspace members are notified.

use chrono::{DateTime, Utc};
use sea_orm::{ConnectionTrait, DatabaseConnection, DbBackend, FromQueryResult, Statement};
use serde::Serialize;
use serde_json::{Value, json};
use uuid::Uuid;

use super::collab::authz::PermissionLevel;
use crate::error::ApiError;

const BODY_MAX_CHARS: usize = 8_000;
const BLOCK_ID_MAX_CHARS: usize = 128;
const MENTION_CAP: usize = 20;
const LIST_CAP: u64 = 500;

#[derive(Debug, Clone)]
pub struct CommentActor {
    pub user_id: Uuid,
    pub level: PermissionLevel,
}

#[derive(Debug)]
pub struct NewComment {
    pub body: String,
    pub block_id: Option<String>,
    pub parent_id: Option<Uuid>,
    pub mentions: Vec<Uuid>,
}

#[derive(Debug, Serialize)]
pub struct FlowCommentView {
    pub id: Uuid,
    pub object_id: Uuid,
    pub block_id: Option<String>,
    pub parent_id: Option<Uuid>,
    pub author_id: Option<Uuid>,
    pub body: String,
    pub mentions: Vec<Uuid>,
    pub created_at: String,
    pub updated_at: String,
    pub resolved_at: Option<String>,
    pub resolved_by: Option<Uuid>,
}

#[derive(Debug, Serialize)]
pub struct FlowCommentList {
    pub items: Vec<FlowCommentView>,
    pub truncated: bool,
}

#[derive(Debug, FromQueryResult)]
struct CommentRow {
    id: Uuid,
    object_id: Uuid,
    block_id: Option<String>,
    parent_id: Option<Uuid>,
    author_id: Option<Uuid>,
    body: String,
    mentions: Value,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
    resolved_at: Option<DateTime<Utc>>,
    resolved_by: Option<Uuid>,
}

#[derive(Debug, FromQueryResult)]
struct ParentRow {
    object_id: Uuid,
    parent_id: Option<Uuid>,
    block_id: Option<String>,
    deleted: bool,
}

pub async fn list_comments(db: &DatabaseConnection, object_id: Uuid) -> Result<FlowCommentList, ApiError> {
    let limit = i64::try_from(LIST_CAP.saturating_add(1)).unwrap_or(i64::MAX);
    let rows = CommentRow::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "SELECT id, object_id, block_id, parent_id, author_id, body, mentions, \
         created_at, updated_at, resolved_at, resolved_by \
         FROM flow_comments \
         WHERE object_id = $1 AND deleted_at IS NULL \
           AND (parent_id IS NULL OR parent_id IN ( \
                SELECT id FROM flow_comments \
                WHERE object_id = $1 AND deleted_at IS NULL AND parent_id IS NULL)) \
         ORDER BY created_at ASC, id ASC \
         LIMIT $2",
        vec![object_id.into(), limit.into()],
    ))
    .all(db)
    .await?;
    let truncated = u64::try_from(rows.len()).unwrap_or(u64::MAX) > LIST_CAP;
    let items = rows
        .into_iter()
        .take(usize::try_from(LIST_CAP).unwrap_or(usize::MAX))
        .map(view_of)
        .collect();
    Ok(FlowCommentList { items, truncated })
}

pub async fn create_comment(
    db: &DatabaseConnection,
    workspace_id: Uuid,
    object_id: Uuid,
    actor: &CommentActor,
    input: NewComment,
) -> Result<FlowCommentView, ApiError> {
    if actor.level < PermissionLevel::Comment {
        return Err(ApiError::forbidden("comment permission is required".to_string()));
    }
    let body = normalize_body(&input.body)?;
    let mentions = normalize_mentions(&input.mentions)?;
    let (parent_id, block_id) = match input.parent_id {
        Some(parent_id) => {
            let parent = load_parent(db, parent_id).await?;
            if parent.object_id != object_id || parent.deleted {
                return Err(ApiError::not_found("flow comment not found".to_string()));
            }
            if parent.parent_id.is_some() {
                return Err(ApiError::invalid_update("replies are only one level deep".to_string()));
            }
            (Some(parent_id), parent.block_id)
        }
        None => (None, normalize_block_id(input.block_id)?),
    };
    let id = Uuid::new_v4();
    let now = Utc::now();
    let mentions_json = json!(mentions.iter().map(Uuid::to_string).collect::<Vec<_>>());
    let row = CommentRow::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "INSERT INTO flow_comments ( \
            id, workspace_id, object_id, block_id, parent_id, author_id, body, mentions, created_at, updated_at \
         ) VALUES ($1, $2, $3, $4, $5, $6, $7, $8::jsonb, $9, $9) \
         RETURNING id, object_id, block_id, parent_id, author_id, body, mentions, \
                   created_at, updated_at, resolved_at, resolved_by",
        vec![
            id.into(),
            workspace_id.into(),
            object_id.into(),
            block_id.into(),
            parent_id.into(),
            actor.user_id.into(),
            body.clone().into(),
            mentions_json.into(),
            now.into(),
        ],
    ))
    .one(db)
    .await?
    .ok_or_else(ApiError::internal)?;
    if let Err(error) = notify_mentions(db, workspace_id, object_id, actor.user_id, &mentions).await {
        tracing::warn!(%error, %object_id, comment_id = %id, "flow comment mention notification failed");
    }
    Ok(view_of(row))
}

pub async fn update_comment(
    db: &DatabaseConnection,
    workspace_id: Uuid,
    object_id: Uuid,
    comment_id: Uuid,
    actor: &CommentActor,
    body: &str,
    mentions: Option<Vec<Uuid>>,
) -> Result<FlowCommentView, ApiError> {
    if actor.level < PermissionLevel::Comment {
        return Err(ApiError::forbidden("comment permission is required".to_string()));
    }
    let existing = load_visible(db, object_id, comment_id).await?;
    if existing.author_id != Some(actor.user_id) {
        return Err(ApiError::forbidden("only the author can edit this comment".to_string()));
    }
    let body = normalize_body(body)?;
    let next_mentions = match mentions {
        Some(ids) => normalize_mentions(&ids)?,
        None => parse_mentions(&existing.mentions),
    };
    let previous = parse_mentions(&existing.mentions);
    let mentions_json = json!(next_mentions.iter().map(Uuid::to_string).collect::<Vec<_>>());
    let now = Utc::now();
    let row = CommentRow::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "UPDATE flow_comments SET body = $3, mentions = $4::jsonb, updated_at = $5 \
         WHERE id = $1 AND object_id = $2 AND deleted_at IS NULL \
         RETURNING id, object_id, block_id, parent_id, author_id, body, mentions, \
                   created_at, updated_at, resolved_at, resolved_by",
        vec![
            comment_id.into(),
            object_id.into(),
            body.into(),
            mentions_json.into(),
            now.into(),
        ],
    ))
    .one(db)
    .await?
    .ok_or_else(|| ApiError::not_found("flow comment not found".to_string()))?;
    let fresh: Vec<Uuid> = next_mentions.into_iter().filter(|id| !previous.contains(id)).collect();
    if let Err(error) = notify_mentions(db, workspace_id, object_id, actor.user_id, &fresh).await {
        tracing::warn!(%error, %object_id, %comment_id, "flow comment mention notification failed");
    }
    Ok(view_of(row))
}

pub async fn delete_comment(
    db: &DatabaseConnection,
    object_id: Uuid,
    comment_id: Uuid,
    actor: &CommentActor,
) -> Result<(), ApiError> {
    let existing = load_visible(db, object_id, comment_id).await?;
    let owns = existing.author_id == Some(actor.user_id) && actor.level >= PermissionLevel::Comment;
    let privileged = actor.level >= PermissionLevel::FullAccess;
    if !owns && !privileged {
        return Err(ApiError::forbidden("you cannot delete this comment".to_string()));
    }
    let now = Utc::now();
    let result = db
        .execute(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "UPDATE flow_comments SET deleted_at = $3, updated_at = $3 \
             WHERE object_id = $1 AND deleted_at IS NULL AND (id = $2 OR parent_id = $2)",
            vec![object_id.into(), comment_id.into(), now.into()],
        ))
        .await?;
    if result.rows_affected() == 0 {
        return Err(ApiError::not_found("flow comment not found".to_string()));
    }
    Ok(())
}

pub async fn set_resolved(
    db: &DatabaseConnection,
    object_id: Uuid,
    comment_id: Uuid,
    actor: &CommentActor,
    resolved: bool,
) -> Result<FlowCommentView, ApiError> {
    let existing = load_visible(db, object_id, comment_id).await?;
    if existing.parent_id.is_some() {
        return Err(ApiError::invalid_update(
            "only a top-level comment can be resolved".to_string(),
        ));
    }
    let author = existing.author_id == Some(actor.user_id) && actor.level >= PermissionLevel::Comment;
    if actor.level < PermissionLevel::Edit && !author {
        return Err(ApiError::forbidden(
            "edit permission is required to resolve a comment".to_string(),
        ));
    }
    let now = Utc::now();
    let (resolved_at, resolved_by) = if resolved {
        (Some(now), Some(actor.user_id))
    } else {
        (None, None)
    };
    let row = CommentRow::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "UPDATE flow_comments SET resolved_at = $3, resolved_by = $4, updated_at = $5 \
         WHERE id = $1 AND object_id = $2 AND deleted_at IS NULL AND parent_id IS NULL \
         RETURNING id, object_id, block_id, parent_id, author_id, body, mentions, \
                   created_at, updated_at, resolved_at, resolved_by",
        vec![
            comment_id.into(),
            object_id.into(),
            resolved_at.into(),
            resolved_by.into(),
            now.into(),
        ],
    ))
    .one(db)
    .await?
    .ok_or_else(|| ApiError::not_found("flow comment not found".to_string()))?;
    Ok(view_of(row))
}

async fn load_visible(db: &DatabaseConnection, object_id: Uuid, comment_id: Uuid) -> Result<CommentRow, ApiError> {
    CommentRow::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "SELECT id, object_id, block_id, parent_id, author_id, body, mentions, \
         created_at, updated_at, resolved_at, resolved_by \
         FROM flow_comments WHERE id = $1 AND object_id = $2 AND deleted_at IS NULL",
        vec![comment_id.into(), object_id.into()],
    ))
    .one(db)
    .await?
    .ok_or_else(|| ApiError::not_found("flow comment not found".to_string()))
}

async fn load_parent(db: &DatabaseConnection, parent_id: Uuid) -> Result<ParentRow, ApiError> {
    ParentRow::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "SELECT object_id, parent_id, block_id, (deleted_at IS NOT NULL) AS deleted \
         FROM flow_comments WHERE id = $1",
        vec![parent_id.into()],
    ))
    .one(db)
    .await?
    .ok_or_else(|| ApiError::not_found("flow comment not found".to_string()))
}

fn normalize_body(raw: &str) -> Result<String, ApiError> {
    let body = raw.trim();
    let chars = body.chars().count();
    if chars == 0 || chars > BODY_MAX_CHARS {
        return Err(ApiError::invalid_update(
            "comment body must be between 1 and 8000 characters".to_string(),
        ));
    }
    Ok(body.to_string())
}

fn normalize_block_id(raw: Option<String>) -> Result<Option<String>, ApiError> {
    let Some(raw) = raw else {
        return Ok(None);
    };
    let block_id = raw.trim();
    if block_id.is_empty() {
        return Ok(None);
    }
    if block_id.chars().count() > BLOCK_ID_MAX_CHARS {
        return Err(ApiError::invalid_update(
            "block_id must be at most 128 characters".to_string(),
        ));
    }
    Ok(Some(block_id.to_string()))
}

fn normalize_mentions(raw: &[Uuid]) -> Result<Vec<Uuid>, ApiError> {
    let mut mentions = Vec::new();
    for id in raw {
        if mentions.contains(id) {
            continue;
        }
        mentions.push(*id);
        if mentions.len() > MENTION_CAP {
            return Err(ApiError::invalid_update(
                "a comment can mention at most 20 people".to_string(),
            ));
        }
    }
    Ok(mentions)
}

fn parse_mentions(value: &Value) -> Vec<Uuid> {
    let Some(items) = value.as_array() else {
        return Vec::new();
    };
    items
        .iter()
        .filter_map(Value::as_str)
        .filter_map(|text| Uuid::parse_str(text).ok())
        .collect()
}

fn view_of(row: CommentRow) -> FlowCommentView {
    FlowCommentView {
        id: row.id,
        object_id: row.object_id,
        block_id: row.block_id,
        parent_id: row.parent_id,
        author_id: row.author_id,
        body: row.body,
        mentions: parse_mentions(&row.mentions),
        created_at: row.created_at.to_rfc3339(),
        updated_at: row.updated_at.to_rfc3339(),
        resolved_at: row.resolved_at.map(|stamp| stamp.to_rfc3339()),
        resolved_by: row.resolved_by,
    }
}

async fn notify_mentions(
    db: &DatabaseConnection,
    workspace_id: Uuid,
    object_id: Uuid,
    actor_id: Uuid,
    mention_user_ids: &[Uuid],
) -> Result<(), ApiError> {
    if mention_user_ids.is_empty() {
        return Ok(());
    }
    let actor_name = actor_label(db, actor_id).await?;
    let link = format!("/workspace/{workspace_id}/flow/{object_id}");
    let now = Utc::now();
    for mention_user_id in mention_user_ids {
        if *mention_user_id == actor_id {
            continue;
        }
        let member = db
            .query_one(Statement::from_sql_and_values(
                DbBackend::Postgres,
                "SELECT 1 FROM workspace_members WHERE workspace_id = $1 AND user_id = $2",
                vec![workspace_id.into(), (*mention_user_id).into()],
            ))
            .await?;
        if member.is_none() {
            continue;
        }
        db.execute(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "INSERT INTO notifications ( \
                id, user_id, workspace_id, type, kind, payload, title, content, link, \
                related_issue_id, related_comment_id, is_read, created_at \
             ) VALUES ($1, $2, $3, $4, $5, $6::jsonb, $7, $8, $9, NULL, NULL, false, $10)",
            vec![
                Uuid::new_v4().into(),
                (*mention_user_id).into(),
                workspace_id.into(),
                "mentioned".into(),
                "mentioned".into(),
                json!({}).into(),
                "notification.mentionedTitle".into(),
                format!("mentioned_by:{actor_name}").into(),
                link.clone().into(),
                now.into(),
            ],
        ))
        .await?;
    }
    Ok(())
}

async fn actor_label(db: &DatabaseConnection, actor_id: Uuid) -> Result<String, ApiError> {
    #[derive(FromQueryResult)]
    struct NameRow {
        name: String,
    }
    let name = NameRow::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "SELECT name FROM users WHERE id = $1",
        vec![actor_id.into()],
    ))
    .one(db)
    .await?
    .map_or_else(|| "someone".to_string(), |row| row.name);
    let collapsed: String = name.chars().filter(|ch| !ch.is_control()).take(80).collect();
    if collapsed.is_empty() {
        Ok("someone".to_string())
    } else {
        Ok(collapsed)
    }
}
