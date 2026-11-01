#![allow(clippy::items_after_statements)]
//! Archived Flow objects the caller can see, plus a permission-checked permanent delete.
//!
//! Archive and restore stay on the existing lifecycle commands. Permanent delete removes the
//! row. Child rows cascade, so a page is deleted only when every descendant is already archived
//! and the caller has full access to each of them.

use chrono::{DateTime, Utc};
use sea_orm::{ConnectionTrait, DatabaseConnection, DbBackend, FromQueryResult, Statement, TransactionTrait};
use serde::Serialize;
use serde_json::Value;
use uuid::Uuid;

use super::appearance::{self, PageAppearance};
use super::collab::authz::{self, PermissionLevel};
use super::policy::{self, FlowReadContext};
use crate::error::ApiError;
use platform::app::AppState;

const TRASH_DEPTH_MAX: i64 = 32;
const PAGE_MAX: u64 = 100;
const SCAN_BATCH: i64 = 100;
const SCAN_MAX: u64 = 1_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PurgeRefusal {
    NotArchived,
    ActiveDescendant,
    InsufficientPermission,
}

#[derive(Debug, Serialize)]
pub struct TrashItem {
    pub id: Uuid,
    pub parent_id: Option<Uuid>,
    pub object_type: String,
    pub title: String,
    pub archived_at: String,
    #[serde(skip_serializing_if = "PageAppearance::is_empty")]
    pub appearance: PageAppearance,
}

#[derive(Debug, Serialize)]
pub struct TrashPage {
    pub items: Vec<TrashItem>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub next_cursor: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct PurgeOutcome {
    pub deleted_id: Uuid,
    pub deleted_count: u64,
}

/// `nodes` includes the addressed object. Every node must be archived, and every node must be
/// at full access, before the database cascade is allowed to remove the subtree.
#[must_use]
pub fn decide_purge(nodes: &[(bool, PermissionLevel)]) -> Result<(), PurgeRefusal> {
    let Some((root_archived, root_level)) = nodes.first() else {
        return Err(PurgeRefusal::NotArchived);
    };
    if !*root_archived {
        return Err(PurgeRefusal::NotArchived);
    }
    if *root_level < PermissionLevel::FullAccess {
        return Err(PurgeRefusal::InsufficientPermission);
    }
    for (archived, level) in nodes.iter().skip(1) {
        if *level < PermissionLevel::FullAccess {
            return Err(PurgeRefusal::InsufficientPermission);
        }
        if !*archived {
            return Err(PurgeRefusal::ActiveDescendant);
        }
    }
    Ok(())
}

pub async fn list_trash(
    state: &AppState,
    access: &FlowReadContext,
    cursor: Option<&str>,
    limit: Option<u64>,
) -> Result<Option<TrashPage>, ApiError> {
    let limit = match limit {
        None => 50,
        Some(0 | 101..) => {
            return Err(ApiError::invalid_update(format!(
                "limit must be between 1 and {PAGE_MAX}"
            )));
        }
        Some(value) => value,
    };
    let mut after = cursor.map(decode_cursor).transpose()?;
    let mut items = Vec::new();
    let mut examined = 0_u64;
    let mut exhausted = false;
    let needed = usize::try_from(limit).unwrap_or(usize::MAX).saturating_add(1);
    while items.len() < needed && examined < SCAN_MAX {
        let batch = fetch_archived_batch(&state.db, access.workspace_id(), after).await?;
        if batch.is_empty() {
            exhausted = true;
            break;
        }
        let ids: Vec<Uuid> = batch.iter().map(|row| row.id).collect();
        let Some(visible) = policy::authorize_flow_objects(state, access, &ids, PermissionLevel::View).await? else {
            return Ok(None);
        };
        for (row, allowed) in batch.into_iter().zip(visible) {
            examined = examined.saturating_add(1);
            if let Some(stamp) = row.archived_at {
                after = Some((stamp, row.id));
            }
            if allowed {
                items.push(item_of(row));
            }
            if items.len() >= needed || examined >= SCAN_MAX {
                break;
            }
        }
    }
    if !policy::ensure_epoch_current(state, access).await? {
        return Ok(None);
    }
    let limit_usize = usize::try_from(limit).unwrap_or(usize::MAX);
    let next_cursor = if items.len() > limit_usize {
        items.truncate(limit_usize);
        items.last().map(|item| format!("{}|{}", item.archived_at, item.id))
    } else if !exhausted && examined >= SCAN_MAX {
        after.map(|(stamp, id)| format!("{}|{}", stamp.to_rfc3339(), id))
    } else {
        None
    };
    Ok(Some(TrashPage { items, next_cursor }))
}

async fn fetch_archived_batch<C: ConnectionTrait>(
    db: &C,
    workspace_id: Uuid,
    after: Option<(DateTime<Utc>, Uuid)>,
) -> Result<Vec<TrashRow>, ApiError> {
    let (after_archived, after_id) = match after {
        Some((stamp, id)) => (Some(stamp), id),
        None => (None, Uuid::nil()),
    };
    Ok(TrashRow::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "SELECT fo.id, fo.parent_id, fo.object_type, fo.archived_at, fo.appearance, \
                COALESCE(p.title, '') AS title \
         FROM flow_objects fo \
         LEFT JOIN flow_object_projections p ON p.object_id = fo.id \
         WHERE fo.workspace_id = $1 \
           AND fo.lifecycle_status = 'archived' \
           AND NOT flow_is_system_navigator_root(fo.object_type, fo.parent_id, fo.governance_metadata) \
           AND ($2::timestamptz IS NULL OR (fo.archived_at, fo.id) < ($2, $3)) \
         ORDER BY fo.archived_at DESC, fo.id DESC \
         LIMIT $4",
        vec![
            workspace_id.into(),
            after_archived.into(),
            after_id.into(),
            SCAN_BATCH.into(),
        ],
    ))
    .all(db)
    .await?)
}

pub async fn purge_archived(
    db: &DatabaseConnection,
    workspace_id: Uuid,
    object_id: Uuid,
    principal_kind: &str,
    principal_id: Uuid,
    role: &str,
) -> Result<PurgeOutcome, ApiError> {
    let txn = db.begin().await?;
    let nodes = DescendantRow::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "WITH RECURSIVE tree AS ( \
            SELECT id, lifecycle_status, 0 AS depth, ARRAY[id]::uuid[] AS path \
            FROM flow_objects \
            WHERE id = $1 AND workspace_id = $2 \
            UNION ALL \
            SELECT child.id, child.lifecycle_status, tree.depth + 1, tree.path || child.id \
            FROM tree \
            JOIN flow_objects child \
              ON child.parent_id = tree.id AND child.workspace_id = $2 \
            WHERE tree.depth < $3 AND NOT child.id = ANY(tree.path) \
         ) \
         SELECT id, lifecycle_status, depth FROM tree ORDER BY depth, id",
        vec![object_id.into(), workspace_id.into(), TRASH_DEPTH_MAX.into()],
    ))
    .all(&txn)
    .await?;
    if nodes.is_empty() {
        return Err(ApiError::not_found("flow object not found".to_string()));
    }
    if nodes.iter().any(|node| node.depth >= TRASH_DEPTH_MAX) {
        return Err(ApiError::invalid_update(
            "this page tree is too deep to permanently delete".to_string(),
        ));
    }
    let locked = DescendantRow::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "SELECT id, lifecycle_status, 0 AS depth FROM flow_objects \
         WHERE workspace_id = $1 AND id = ANY(string_to_array($2, ',')::uuid[]) \
         FOR UPDATE",
        vec![
            workspace_id.into(),
            uuid_list(&nodes.iter().map(|node| node.id).collect::<Vec<_>>()).into(),
        ],
    ))
    .all(&txn)
    .await?;
    if locked.len() != nodes.len() {
        return Err(ApiError::not_found("flow object not found".to_string()));
    }
    // `nodes` is depth order, so the addressed object stays first for [`decide_purge`].
    let mut judged = Vec::with_capacity(nodes.len());
    for node in &nodes {
        let level =
            authz::effective_permission(&txn, workspace_id, node.id, principal_kind, principal_id, role).await?;
        let archived = locked
            .iter()
            .find(|row| row.id == node.id)
            .is_some_and(|row| row.lifecycle_status == "archived");
        judged.push((archived, level));
    }
    match decide_purge(&judged) {
        Ok(()) => {}
        Err(PurgeRefusal::NotArchived) => {
            return Err(ApiError::invalid_update(
                "only an archived object can be permanently deleted".to_string(),
            ));
        }
        Err(PurgeRefusal::ActiveDescendant) => {
            return Err(ApiError::invalid_update_with_details(
                "archive every subpage before permanently deleting this page",
                serde_json::json!({ "reason": "active_descendant" }),
            ));
        }
        Err(PurgeRefusal::InsufficientPermission) => {
            return Err(ApiError::policy_rejected(
                "full access is required to permanently delete this page",
            ));
        }
    }
    let deleted = txn
        .execute(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "DELETE FROM flow_objects WHERE id = $1 AND workspace_id = $2 AND lifecycle_status = 'archived'",
            vec![object_id.into(), workspace_id.into()],
        ))
        .await?
        .rows_affected();
    if deleted == 0 {
        return Err(ApiError::not_found("flow object not found".to_string()));
    }
    txn.commit().await?;
    let deleted_count = u64::try_from(nodes.len()).unwrap_or(deleted);
    Ok(PurgeOutcome {
        deleted_id: object_id,
        deleted_count,
    })
}

fn item_of(row: TrashRow) -> TrashItem {
    TrashItem {
        id: row.id,
        parent_id: row.parent_id,
        object_type: row.object_type,
        title: row.title,
        archived_at: row
            .archived_at
            .map(|stamp| stamp.to_rfc3339())
            .unwrap_or_else(|| Utc::now().to_rfc3339()),
        appearance: appearance::parse_stored(&row.appearance),
    }
}

fn decode_cursor(cursor: &str) -> Result<(DateTime<Utc>, Uuid), ApiError> {
    let Some((stamp, id)) = cursor.rsplit_once('|') else {
        return Err(ApiError::invalid_update("invalid trash cursor".to_string()));
    };
    let archived_at = DateTime::parse_from_rfc3339(stamp)
        .map(|stamp| stamp.with_timezone(&Utc))
        .map_err(|_| ApiError::invalid_update("invalid trash cursor".to_string()))?;
    let id = Uuid::parse_str(id).map_err(|_| ApiError::invalid_update("invalid trash cursor".to_string()))?;
    Ok((archived_at, id))
}

fn uuid_list(ids: &[Uuid]) -> String {
    ids.iter().map(ToString::to_string).collect::<Vec<_>>().join(",")
}

#[derive(Debug, FromQueryResult)]
struct TrashRow {
    id: Uuid,
    parent_id: Option<Uuid>,
    object_type: String,
    title: String,
    archived_at: Option<DateTime<Utc>>,
    appearance: Value,
}

#[derive(Debug, FromQueryResult)]
struct DescendantRow {
    id: Uuid,
    lifecycle_status: String,
    depth: i64,
}

#[cfg(test)]
mod tests {
    use super::PurgeRefusal;
    use super::decide_purge;
    use crate::flow::collab::authz::PermissionLevel;

    #[test]
    fn purge_requires_an_archived_tree_and_full_access_on_every_node() {
        assert_eq!(decide_purge(&[(true, PermissionLevel::FullAccess)]), Ok(()));
        assert_eq!(
            decide_purge(&[(false, PermissionLevel::FullAccess)]),
            Err(PurgeRefusal::NotArchived)
        );
        assert_eq!(
            decide_purge(&[(true, PermissionLevel::Edit)]),
            Err(PurgeRefusal::InsufficientPermission)
        );
        assert_eq!(
            decide_purge(&[
                (true, PermissionLevel::FullAccess),
                (false, PermissionLevel::FullAccess)
            ]),
            Err(PurgeRefusal::ActiveDescendant)
        );
        assert_eq!(
            decide_purge(&[(true, PermissionLevel::FullAccess), (true, PermissionLevel::Edit)]),
            Err(PurgeRefusal::InsufficientPermission)
        );
    }
}
