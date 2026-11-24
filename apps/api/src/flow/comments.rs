//! Comment threads on a Flow object.
//!
//! Listing needs `view`. Creating and replying need `comment`. An author with `comment` may edit
//! or delete their own row. Resolving a top-level thread needs `edit`, and so does deleting
//! someone else's row only when the caller has `full_access`. Replies are one level deep and stay
//! on the same object. Mentions are people picked by id plus `@Name` typed in the body (resolved by `scope_social`), each stored
//! as `{id, label}`. Notifications require current object access and never create membership.

use platform::timestamp::Timestamp;
use sea_orm::{ConnectionTrait, DatabaseConnection, DbBackend, FromQueryResult, Statement};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;

use super::collab::authz::PermissionLevel;
use crate::{
    error::ApiError,
    services::mentions::{self, Audience, Mention, MentionedPerson, Person, PersonState},
};

const BODY_MAX_CHARS: usize = 8_000;
const BLOCK_ID_MAX_CHARS: usize = 128;
const MENTION_CAP: usize = 20;
const LIST_CAP: u64 = 500;

#[derive(Debug, Clone)]
pub struct CommentActor {
    pub user_id: Uuid,
    pub level: PermissionLevel,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NewComment {
    pub body: String,
    pub block_id: Option<String>,
    pub parent_id: Option<Uuid>,
    #[serde(default)]
    pub mentions: Vec<Uuid>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct FlowCommentView {
    #[serde(default)]
    pub anchor_status: Option<String>,
    pub id: Uuid,
    pub object_id: Uuid,
    pub block_id: Option<String>,
    pub parent_id: Option<Uuid>,
    pub author_id: Option<Uuid>,
    /// The author as a person to show; `null` when no author remains.
    pub author: Option<Person>,
    pub body: String,
    pub mentions: Vec<MentionedPerson>,
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
pub(crate) struct CommentRow {
    id: Uuid,
    object_id: Uuid,
    block_id: Option<String>,
    parent_id: Option<Uuid>,
    author_id: Option<Uuid>,
    body: String,
    pub(crate) mentions: Value,
    created_at: Timestamp,
    updated_at: Timestamp,
    resolved_at: Option<Timestamp>,
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
    super::block_anchors::complete_registration(db, object_id).await?;
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
    let mut items: Vec<_> = rows
        .into_iter()
        .take(usize::try_from(LIST_CAP).unwrap_or(usize::MAX))
        .map(view_of)
        .collect();
    super::block_anchors::decorate(db, &mut items).await?;
    decorate_people(db, &mut items).await?;
    Ok(FlowCommentList { items, truncated })
}

pub async fn create_comment(
    db: &DatabaseConnection,
    workspace_id: Uuid,
    object: Uuid,
    actor: &CommentActor,
    input: NewComment,
) -> Result<FlowCommentView, ApiError> {
    if super::scope::identity(db, object).await?.workspace_id != Some(workspace_id) {
        return Err(ApiError::not_found("flow object not found"));
    }
    super::scope_social::create_comment(db, object, actor.user_id, input, None).await
}
pub async fn update_comment(
    db: &DatabaseConnection,
    workspace_id: Uuid,
    object: Uuid,
    comment: Uuid,
    actor: &CommentActor,
    body: &str,
    mentions: Option<Vec<Uuid>>,
) -> Result<FlowCommentView, ApiError> {
    if super::scope::identity(db, object).await?.workspace_id != Some(workspace_id) {
        return Err(ApiError::not_found("flow object not found"));
    }
    super::scope_social::update_comment(db, object, comment, actor.user_id, body, mentions, None).await
}
pub async fn delete_comment(
    db: &DatabaseConnection,
    object: Uuid,
    comment: Uuid,
    actor: &CommentActor,
) -> Result<(), ApiError> {
    super::scope_social::delete_comment(db, object, comment, actor.user_id, None).await
}
pub async fn set_resolved(
    db: &DatabaseConnection,
    object: Uuid,
    comment: Uuid,
    actor: &CommentActor,
    resolved: bool,
) -> Result<FlowCommentView, ApiError> {
    super::scope_social::resolve_comment(db, object, comment, actor.user_id, resolved, None).await
}

pub(crate) async fn create_comment_on<C: ConnectionTrait>(
    db: &C,
    workspace_id: Option<Uuid>,
    scope_id: Option<Uuid>,
    object_id: Uuid,
    actor: &CommentActor,
    input: NewComment,
) -> Result<FlowCommentView, ApiError> {
    if actor.level < PermissionLevel::Comment {
        return Err(ApiError::forbidden("comment permission is required".to_string()));
    }
    let body = normalize_body(&input.body)?;
    let mentions = label_mentions(db, &normalize_mentions(&input.mentions)?).await?;
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
    let now = Timestamp::now();
    let mentions_json = mentions::to_json(&mentions);
    let row = CommentRow::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "INSERT INTO flow_comments ( \
            id, workspace_id, object_id, block_id, parent_id, author_id, body, mentions, created_at, updated_at,scope_id \
         ) VALUES ($1, $2, $3, $4, $5, $6, $7, $8::jsonb, $9, $9,$10) \
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
            scope_id.into(),
        ],
    ))
    .one(db)
    .await?
    .ok_or_else(ApiError::internal)?;
    super::block_anchors::bind_new(db, id, object_id, row.block_id.as_deref(), row.parent_id).await?;
    let mut view = view_of(row);
    super::block_anchors::decorate(db, std::slice::from_mut(&mut view)).await?;
    decorate_people(db, std::slice::from_mut(&mut view)).await?;
    Ok(view)
}

pub(crate) async fn update_comment_on<C: ConnectionTrait>(
    db: &C,
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
        Some(ids) => label_mentions(db, &normalize_mentions(&ids)?).await?,
        None => mentions::from_json(&existing.mentions),
    };
    let mentions_json = mentions::to_json(&next_mentions);
    let now = Timestamp::now();
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
    let mut view = view_of(row);
    super::block_anchors::decorate(db, std::slice::from_mut(&mut view)).await?;
    decorate_people(db, std::slice::from_mut(&mut view)).await?;
    Ok(view)
}

pub(crate) async fn delete_comment_on<C: ConnectionTrait>(
    db: &C,
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
    let now = Timestamp::now();
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

pub(crate) async fn set_resolved_on<C: ConnectionTrait>(
    db: &C,
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
    let now = Timestamp::now();
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
    let mut view = view_of(row);
    super::block_anchors::decorate(db, std::slice::from_mut(&mut view)).await?;
    decorate_people(db, std::slice::from_mut(&mut view)).await?;
    Ok(view)
}

pub(crate) async fn load_visible<C: ConnectionTrait>(
    db: &C,
    object_id: Uuid,
    comment_id: Uuid,
) -> Result<CommentRow, ApiError> {
    CommentRow::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "SELECT id, object_id, block_id, parent_id, author_id, body, mentions, \
         created_at, updated_at, resolved_at, resolved_by \
         FROM flow_comments WHERE id = $1 AND object_id = $2 AND deleted_at IS NULL FOR UPDATE",
        vec![comment_id.into(), object_id.into()],
    ))
    .one(db)
    .await?
    .ok_or_else(|| ApiError::not_found("flow comment not found".to_string()))
}

async fn load_parent<C: ConnectionTrait>(db: &C, parent_id: Uuid) -> Result<ParentRow, ApiError> {
    ParentRow::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "SELECT object_id, parent_id, block_id, (deleted_at IS NOT NULL) AS deleted \
         FROM flow_comments WHERE id = $1 FOR SHARE",
        vec![parent_id.into()],
    ))
    .one(db)
    .await?
    .ok_or_else(|| ApiError::not_found("flow comment not found".to_string()))
}

pub(crate) fn normalize_body(raw: &str) -> Result<String, ApiError> {
    let body = raw.trim();
    let chars = body.chars().count();
    if chars == 0 || chars > BODY_MAX_CHARS {
        return Err(ApiError::invalid_update(
            "comment body must be between 1 and 8000 characters".to_string(),
        ));
    }
    Ok(body.to_string())
}

pub(crate) fn normalize_block_id(raw: Option<String>) -> Result<Option<String>, ApiError> {
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

pub(crate) fn normalize_mentions(raw: &[Uuid]) -> Result<Vec<Uuid>, ApiError> {
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

/// The stored mentions for picked ids, in the order given. An id without an account is dropped.
async fn label_mentions<C: ConnectionTrait>(db: &C, ids: &[Uuid]) -> Result<Vec<Mention>, ApiError> {
    let known = mentions::existing_users(db, ids).await?;
    Ok(ids
        .iter()
        .filter_map(|id| known.iter().find(|user| user.id == *id))
        .map(|user| Mention {
            id: user.id,
            label: user.name.trim().to_string(),
        })
        .collect())
}

/// The ids already stored as mentions of a comment (empty for a comment that is gone).
pub(crate) async fn stored_mention_ids<C: ConnectionTrait>(
    db: &C,
    object_id: Uuid,
    comment_id: Uuid,
) -> Result<Vec<Uuid>, ApiError> {
    let row = db
        .query_one(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT mentions FROM flow_comments WHERE id = $1 AND object_id = $2 AND deleted_at IS NULL",
            vec![comment_id.into(), object_id.into()],
        ))
        .await?;
    let Some(row) = row else {
        return Ok(Vec::new());
    };
    let stored: Value = row.try_get("", "mentions")?;
    Ok(mentions::ids(&mentions::from_json(&stored)))
}

fn view_of(row: CommentRow) -> FlowCommentView {
    FlowCommentView {
        anchor_status: None,
        id: row.id,
        object_id: row.object_id,
        block_id: row.block_id,
        parent_id: row.parent_id,
        author_id: row.author_id,
        // Placeholders: `decorate_people` replaces them with the people as they are now.
        author: row.author_id.map(|id| Person {
            id,
            name: None,
            is_bot: false,
            state: PersonState::Removed,
        }),
        body: row.body,
        mentions: mentions::from_json(&row.mentions)
            .into_iter()
            .map(|mention| MentionedPerson {
                person: Person {
                    id: mention.id,
                    name: Some(mention.label.clone()).filter(|label| !label.trim().is_empty()),
                    is_bot: false,
                    state: PersonState::Removed,
                },
                label: mention.label,
            })
            .collect(),
        created_at: row.created_at.to_rfc3339(),
        updated_at: row.updated_at.to_rfc3339(),
        resolved_at: row.resolved_at.map(platform::timestamp::Timestamp::to_rfc3339),
        resolved_by: row.resolved_by,
    }
}

/// Shows the authors and mentioned people of `views` as they are now. The names a placeholder
/// carries are the stored labels, kept for people who have left.
pub(crate) async fn decorate_people<C: ConnectionTrait>(db: &C, views: &mut [FlowCommentView]) -> Result<(), ApiError> {
    let mut ids = Vec::new();
    let mut hints = std::collections::HashMap::new();
    for person in views.iter().flat_map(|view| {
        view.author
            .iter()
            .chain(view.mentions.iter().map(|mentioned| &mentioned.person))
    }) {
        ids.push(person.id);
        if let Some(name) = &person.name {
            hints.insert(person.id, name.clone());
        }
    }
    let shown = mentions::people(db, Audience::Anyone, &ids, &hints).await?;
    for view in views {
        for person in view
            .author
            .iter_mut()
            .chain(view.mentions.iter_mut().map(|mentioned| &mut mentioned.person))
        {
            if let Some(current) = shown.get(&person.id) {
                person.clone_from(current);
            }
        }
    }
    Ok(())
}

const MENTION_CANDIDATE_CAP: i64 = 50;
const MENTION_QUERY_CHARS_MAX: usize = 80;

#[derive(Debug, Clone, Serialize)]
pub struct MentionCandidate {
    pub user_id: Uuid,
    pub name: String,
    pub kind: String,
}

#[derive(Debug, Serialize)]
pub struct MentionCandidateList {
    pub items: Vec<MentionCandidate>,
}

/// Workspace members, plus users who are not members but hold a grant on this page or an
/// ancestor it still inherits from.
///
/// The scoped application service independently validates notification recipients; this list is only the picker.
pub async fn list_mention_candidates(
    db: &DatabaseConnection,
    workspace_id: Uuid,
    object_id: Uuid,
    query: &str,
) -> Result<MentionCandidateList, ApiError> {
    let query = normalize_mention_query(query)?;
    let members = candidate_rows(
        db,
        "SELECT u.id AS user_id, COALESCE(u.name, '') AS name, 'member' AS kind \
         FROM workspace_members wm \
         JOIN users u ON u.id = wm.user_id \
         WHERE wm.workspace_id = $1 \
           AND ($2 = '' OR strpos(lower(COALESCE(u.name, '')), lower($2)) > 0) \
         ORDER BY name, u.id \
         LIMIT $3",
        vec![workspace_id.into(), query.clone().into(), MENTION_CANDIDATE_CAP.into()],
    )
    .await?;
    let guests = guest_rows(db, workspace_id, object_id, &query, MENTION_CANDIDATE_CAP).await?;
    let mut items = members;
    for guest in guests {
        if items.iter().any(|item| item.user_id == guest.user_id) {
            continue;
        }
        items.push(guest);
        if items.len() >= usize::try_from(MENTION_CANDIDATE_CAP).unwrap_or(50) {
            break;
        }
    }
    Ok(MentionCandidateList { items })
}

/// Users who are not members but hold a grant on this page or an ancestor it still inherits from,
/// whose name contains `query` (all of them for an empty query).
async fn guest_rows(
    db: &DatabaseConnection,
    workspace_id: Uuid,
    object_id: Uuid,
    query: &str,
    cap: i64,
) -> Result<Vec<MentionCandidate>, ApiError> {
    candidate_rows(
        db,
        "WITH RECURSIVE chain AS ( \
            SELECT id, parent_id, inherit_from_parent, 0 AS depth \
            FROM flow_objects \
            WHERE id = $1 AND workspace_id = $2 \
            UNION ALL \
            SELECT parent.id, parent.parent_id, parent.inherit_from_parent, chain.depth + 1 \
            FROM chain \
            JOIN flow_objects parent \
              ON parent.id = chain.parent_id AND parent.workspace_id = $2 \
            WHERE chain.inherit_from_parent AND chain.depth < 32 \
         ) \
         SELECT DISTINCT u.id AS user_id, COALESCE(u.name, '') AS name, 'guest' AS kind \
         FROM flow_object_grants g \
         JOIN chain ON chain.id = g.object_id \
         JOIN users u ON u.id = g.principal_id \
         WHERE g.principal_kind = 'user' \
           AND NOT EXISTS ( \
               SELECT 1 FROM workspace_members wm \
               WHERE wm.workspace_id = $2 AND wm.user_id = u.id \
           ) \
           AND ($3 = '' OR strpos(lower(COALESCE(u.name, '')), lower($3)) > 0) \
         ORDER BY name, u.id \
         LIMIT $4",
        vec![object_id.into(), workspace_id.into(), query.into(), cap.into()],
    )
    .await
}

/// The people `@Name` in a comment of a workspace page can name: members and guests of the page.
pub(crate) async fn workspace_name_pool(
    db: &DatabaseConnection,
    workspace_id: Uuid,
    object_id: Uuid,
) -> Result<Vec<mentions::Member>, ApiError> {
    let mut pool = mentions::workspace_members(db, workspace_id).await?;
    for guest in guest_rows(db, workspace_id, object_id, "", i64::try_from(LIST_CAP).unwrap_or(500)).await? {
        pool.push(mentions::Member {
            id: guest.user_id,
            name: guest.name,
        });
    }
    Ok(pool)
}

pub(crate) fn normalize_mention_query(query: &str) -> Result<String, ApiError> {
    // Check the raw query. `trim` deletes a trailing newline, which would hide the control character.
    let trimmed = query.trim();
    if query.chars().any(char::is_control) || trimmed.chars().count() > MENTION_QUERY_CHARS_MAX {
        return Err(ApiError::invalid_update(
            "mention query must be at most 80 characters".to_string(),
        ));
    }
    Ok(trimmed.to_string())
}

async fn candidate_rows(
    db: &DatabaseConnection,
    sql: &str,
    values: Vec<sea_orm::Value>,
) -> Result<Vec<MentionCandidate>, ApiError> {
    #[derive(FromQueryResult)]
    struct Row {
        user_id: Uuid,
        name: String,
        kind: String,
    }
    let rows = Row::find_by_statement(Statement::from_sql_and_values(DbBackend::Postgres, sql, values))
        .all(db)
        .await?;
    Ok(rows
        .into_iter()
        .map(|row| MentionCandidate {
            user_id: row.user_id,
            name: row.name,
            kind: row.kind,
        })
        .collect())
}

#[cfg(test)]
mod tests {
    use super::{CommentRow, decorate_people, normalize_mention_query, view_of};
    use crate::routes::context::tenant_fixture::exec;
    use crate::scratch_or_skip;
    use platform::timestamp::Timestamp;
    use serde_json::json;
    use uuid::Uuid;

    #[test]
    fn mention_query_rejects_control_characters() {
        assert_eq!(normalize_mention_query("  Ada ").ok().as_deref(), Some("Ada"));
        assert!(normalize_mention_query("ada\n").is_err());
    }

    #[tokio::test]
    async fn mentions_carry_their_label() {
        let scratch = scratch_or_skip!("flow_comment_people");
        let db = &scratch.db;
        let (alice, bob, gone) = (Uuid::new_v4(), Uuid::new_v4(), Uuid::new_v4());
        for (id, name) in [(alice, "Alice"), (bob, "Bob Renamed")] {
            exec(
                db,
                "INSERT INTO users (id, email, password_hash, name, role, is_active) \
                 VALUES ($1, $2, '!', $3, 'user', true)",
                vec![id.into(), format!("{id}@flow.test").into(), name.into()],
            )
            .await;
        }
        let row = |resolved_by: Option<Uuid>| CommentRow {
            id: Uuid::new_v4(),
            object_id: Uuid::new_v4(),
            block_id: None,
            parent_id: None,
            author_id: Some(alice),
            body: "hi @Bob Dee".to_string(),
            mentions: json!([{"id": bob, "label": "Bob Dee"}, {"id": gone, "label": "Gil Gone"}]),
            created_at: Timestamp::now(),
            updated_at: Timestamp::now(),
            resolved_at: resolved_by.map(|_| Timestamp::now()),
            resolved_by,
        };
        let mut views = vec![view_of(row(Some(bob))), view_of(row(None))];
        decorate_people(db, &mut views).await.expect("people are readable");
        let shown = serde_json::to_value(&views).expect("json");
        assert_eq!(
            shown[0]["mentions"],
            json!([
                {"id": bob, "name": "Bob Renamed", "is_bot": false, "state": "active", "label": "Bob Dee"},
                {"id": gone, "name": "Gil Gone", "is_bot": false, "state": "removed", "label": "Gil Gone"}
            ])
        );
        scratch.drop_self().await;
    }
}
