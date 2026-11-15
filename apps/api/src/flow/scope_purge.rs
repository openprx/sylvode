//! Explicit archived-subtree deletion. Source ownership never follows destination mounts.
use super::{collab::authz::PermissionLevel, scope};
use crate::error::ApiError;
use platform::app::AppState;
use sea_orm::{ConnectionTrait, DbBackend, FromQueryResult, Statement, TransactionTrait};
use serde_json::{Value, json};
use uuid::Uuid;
#[derive(FromQueryResult)]
struct Node {
    id: Uuid,
    lifecycle_status: String,
    depth: i64,
    system_root: bool,
}
async fn tree<C: ConnectionTrait>(db: &C, scope: Uuid, object: Uuid) -> Result<Vec<Node>, ApiError> {
    Ok(Node::find_by_statement(Statement::from_sql_and_values(DbBackend::Postgres,
 "WITH RECURSIVE tree AS(SELECT id,lifecycle_status,0::bigint AS depth,ARRAY[id]::uuid[] AS path,flow_is_system_navigator_root(object_type,parent_id,governance_metadata) AS system_root FROM flow_objects WHERE id=$1 AND scope_id=$2 UNION ALL SELECT c.id,c.lifecycle_status,t.depth+1,t.path||c.id,flow_is_system_navigator_root(c.object_type,c.parent_id,c.governance_metadata) FROM tree t JOIN flow_objects c ON c.parent_id=t.id AND c.scope_id=$2 WHERE t.depth<32 AND NOT c.id=ANY(t.path)) SELECT id,lifecycle_status,depth,system_root FROM tree ORDER BY depth,id",vec![object.into(),scope.into()])).all(db).await?)
}
pub async fn purge(state: &AppState, object: Uuid, actor: Uuid, key: &str) -> Result<Value, ApiError> {
    let body = json!({"operation":"purge","object_id":object});
    // Resolve only a receipt belonging to this actor; other users cannot probe deleted identities.
    let old = state
        .db
        .query_one(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT scope_id FROM flow_purge_receipts WHERE object_id=$1 AND actor_id=$2 AND idempotency_key=$3",
            vec![object.into(), actor.into(), key.into()],
        ))
        .await?;
    let scope_id = if let Some(row) = old {
        row.try_get::<Uuid>("", "scope_id")?
    } else {
        scope::identity(&state.db, object).await?.scope_id
    };
    if let Some(result) = super::scope_commands::replay(&state.db, scope_id, actor, key, &body).await? {
        return Ok(result);
    }
    let mut proof = scope::authorize(&state.db, object, actor, PermissionLevel::FullAccess).await?;
    if scope::legacy_workspace(&state.db, scope_id).await?.is_some() {
        return Err(ApiError::invalid_update(
            "legacy purge requires its integration adapter",
        ));
    }
    let nodes = tree(&state.db, scope_id, object).await?;
    if nodes.is_empty() {
        return Err(ApiError::not_found("Flow object not found"));
    }
    if nodes.iter().any(|n| n.depth >= 32 || n.system_root) {
        return Err(ApiError::invalid_update(
            "system roots and truncated trees cannot be purged",
        ));
    }
    for node in &nodes {
        let dependent = scope::authorize(&state.db, node.id, actor, PermissionLevel::FullAccess).await?;
        if dependent.revision != proof.revision {
            return Err(super::policy::authorization_read_unstable());
        }
        proof.audience_revisions.extend(dependent.audience_revisions);
    }
    proof.audience_revisions.sort_unstable();
    proof.audience_revisions.dedup();
    let tx = state.db.begin().await?;
    scope::lock_change(&tx, &proof).await?;
    if let Some(result) = super::scope_commands::lock_and_replay(&tx, scope_id, actor, key, &body).await? {
        tx.commit().await?;
        return Ok(result);
    }
    let current = tree(&tx, scope_id, object).await?;
    if current.len() != nodes.len() || current.iter().zip(&nodes).any(|(a, b)| a.id != b.id) {
        return Err(ApiError::policy_rejected(
            "purge subtree changed; retry with the same intent",
        ));
    }
    let ids = current.iter().map(|n| n.id).collect::<Vec<_>>();
    tx.query_all(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "SELECT id FROM flow_objects WHERE id=ANY($1) ORDER BY id FOR UPDATE",
        vec![ids.into()],
    ))
    .await?;
    for node in &current {
        scope::authorize(&tx, node.id, actor, PermissionLevel::FullAccess).await?;
        if node.system_root || node.lifecycle_status != "archived" {
            return Err(ApiError::invalid_update_with_details(
                "archive every object in the subtree before permanent deletion",
                json!({"reason":"active_descendant"}),
            ));
        }
    }
    let result = json!({"deleted_id":object,"deleted_count":current.len()});
    super::scope_event::insert_object_audit(
        &tx,
        &scope::identity(&tx, object).await?,
        actor,
        "flow.object.purged",
        key,
        json!({"object_id":object,"deleted_count":current.len()}),
    )
    .await?;
    tx.execute(Statement::from_sql_and_values(DbBackend::Postgres,"INSERT INTO flow_purge_receipts(scope_id,idempotency_key,actor_id,object_id,request_body,result) VALUES($1,$2,$3,$4,$5,$6)",vec![scope_id.into(),key.into(),actor.into(),object.into(),body.into(),result.clone().into()])).await?;
    scope::advance_revision(&tx, scope_id).await?;
    tx.execute(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "DELETE FROM flow_objects WHERE id=$1 AND scope_id=$2 AND lifecycle_status='archived'",
        vec![object.into(), scope_id.into()],
    ))
    .await?;
    tx.commit().await?;
    Ok(result)
}

/// Conservative retention: only eligible leaves are removed. A parent is never a shortcut for
/// deleting active/unexpired descendants, and source lineage remains retained by its FK policy.
pub async fn retention_leaf(
    db: &sea_orm::DatabaseConnection,
    object: Uuid,
    now: chrono::DateTime<chrono::Utc>,
) -> Result<bool, ApiError> {
    let identity = match scope::identity(db, object).await {
        Ok(value) => value,
        Err(error) if error.kind() == crate::error::ApiErrorKind::NotFound => return Ok(false),
        Err(error) => return Err(error),
    };
    let tx = db.begin().await?;
    if let Some(workspace) = identity.workspace_id {
        super::collab::authz::lock_epoch_for_update(&tx, workspace).await?;
    } else {
        tx.execute(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT scope_id FROM flow_scope_settings WHERE scope_id=$1 FOR UPDATE",
            vec![identity.scope_id.into()],
        ))
        .await?;
    }
    let row=tx.query_one(Statement::from_sql_and_values(DbBackend::Postgres,
        "SELECT id FROM flow_objects o WHERE o.id=$1 AND o.scope_id=$2 AND o.lifecycle_status='archived' AND o.permanent_cleanup_after<$3 AND NOT flow_is_system_navigator_root(o.object_type,o.parent_id,o.governance_metadata) AND NOT EXISTS(SELECT 1 FROM flow_objects c WHERE c.parent_id=o.id) AND NOT EXISTS(SELECT 1 FROM flow_object_lineage l WHERE l.source_object_id=o.id) AND NOT EXISTS(SELECT 1 FROM flow_conversion_previews p WHERE p.source_object_id=o.id) AND NOT EXISTS(SELECT 1 FROM flow_conversion_jobs j WHERE j.source_object_id=o.id) FOR UPDATE OF o",
        vec![object.into(),identity.scope_id.into(),now.into()])).await?;
    if row.is_none() {
        tx.rollback().await?;
        return Ok(false);
    }
    if tx
        .query_one(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT id FROM flow_objects WHERE parent_id=$1 LIMIT 1",
            vec![object.into()],
        ))
        .await?
        .is_some()
    {
        tx.rollback().await?;
        return Ok(false);
    }
    // Parent FK insertion takes a conflicting key-share lock, so no child can appear after this
    // explicit no-descendant check and before deletion. Scope fences also serialize typed writes.
    scope::advance_revision(&tx, identity.scope_id).await?;
    let deleted = tx
        .execute(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "DELETE FROM flow_objects WHERE id=$1",
            vec![object.into()],
        ))
        .await?
        .rows_affected();
    tx.commit().await?;
    Ok(deleted == 1)
}
