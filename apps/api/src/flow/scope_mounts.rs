//! Non-owning placements. Destination authority is an audience dependency, never source ownership.
#[derive(FromQueryResult)]
struct MountPublisherRole {
    role: String,
}

use super::{
    collab::authz::{self, PermissionLevel},
    scope,
};
use crate::error::ApiError;
use sea_orm::{ConnectionTrait, DatabaseConnection, DbBackend, FromQueryResult, Statement, TransactionTrait};
use serde::{Deserialize, Serialize};
use serde_json::json;
use uuid::Uuid;
#[derive(Serialize, Deserialize, FromQueryResult)]
pub struct MountView {
    pub id: Uuid,
    pub source_object_id: Uuid,
    pub destination_workspace_id: Uuid,
    pub destination_project_id: Option<Uuid>,
    pub level: String,
    pub status: String,
}
#[derive(Deserialize)]
pub struct MountInput {
    pub destination_workspace_id: Uuid,
    pub destination_project_id: Option<Uuid>,
    pub level: String,
    pub idempotency_key: String,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ChangeMountInput {
    pub expected_level: String,
    pub level: String,
    pub idempotency_key: String,
}

fn valid_level(level: &str) -> bool {
    matches!(level, "view" | "comment" | "edit")
}

async fn require_enabled<C: ConnectionTrait>(db: &C) -> Result<(), ApiError> {
    scope::require_personal_write(db).await?;
    if !scope::capabilities(db).await?.external_mounts {
        return Err(ApiError::typed_with_details(
            crate::error::ApiErrorKind::FeatureDisabled,
            "external mounts are not enabled",
            json!({"reason":"external_mounts_disabled"}),
        ));
    }
    Ok(())
}

pub async fn list(db: &DatabaseConnection, user: Uuid, object: Uuid) -> Result<Vec<MountView>, ApiError> {
    let proof = scope::authorize(db, object, user, PermissionLevel::FullAccess).await?;
    let rows=MountView::find_by_statement(Statement::from_sql_and_values(DbBackend::Postgres,"SELECT m.id,m.source_object_id,m.destination_workspace_id,m.destination_project_id,m.grant_level AS level,CASE WHEN o.lifecycle_status='active' THEN 'active' ELSE 'source_archived' END AS status FROM flow_mounts m JOIN flow_objects o ON o.id=m.source_object_id WHERE m.source_object_id=$1 ORDER BY m.created_at,m.id",vec![object.into()])).all(db).await?;
    if !scope::proof_current(db, &proof).await? {
        return Err(super::policy::authorization_read_unstable());
    }
    Ok(rows)
}
pub async fn create(
    db: &DatabaseConnection,
    user: Uuid,
    object: Uuid,
    input: MountInput,
) -> Result<MountView, ApiError> {
    if !valid_level(&input.level) || input.idempotency_key.trim().is_empty() || input.idempotency_key.len() > 128 {
        return Err(ApiError::invalid_update(
            "invalid mount capability or operation identity",
        ));
    }
    let identity = scope::identity(db, object).await?;
    if identity.workspace_id.is_some() {
        return Err(ApiError::invalid_update(
            "non-owning mounts currently require a personal source",
        ));
    }
    let tx = db.begin().await?;
    require_enabled(&tx).await?;
    // Membership mutations take the same workspace row first, even before Flow settings exist.
    tx.execute(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "SELECT id FROM workspaces WHERE id=$1 FOR SHARE",
        vec![input.destination_workspace_id.into()],
    ))
    .await?;
    tx.execute(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "INSERT INTO flow_workspace_settings(workspace_id) VALUES($1) ON CONFLICT DO NOTHING",
        vec![input.destination_workspace_id.into()],
    ))
    .await?;
    authz::lock_epoch_for_share(&tx, input.destination_workspace_id).await?;

    let role = MountPublisherRole::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "SELECT role FROM workspace_members WHERE workspace_id=$1 AND user_id=$2",
        vec![input.destination_workspace_id.into(), user.into()],
    ))
    .one(&tx)
    .await?
    .ok_or_else(|| ApiError::forbidden("destination publishing permission required"))?;
    if !matches!(role.role.as_str(), "owner" | "admin") {
        return Err(ApiError::forbidden("destination publishing permission required"));
    }
    tx.execute(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "SELECT scope_id FROM flow_scope_settings WHERE scope_id=$1 FOR UPDATE",
        vec![identity.scope_id.into()],
    ))
    .await?;
    if !scope::authorize(&tx, object, user, PermissionLevel::FullAccess)
        .await?
        .resource_active
    {
        return Err(ApiError::not_found("mount source unavailable"));
    }
    if let Some(project) = input.destination_project_id {
        ensure_project(&tx, input.destination_workspace_id, project).await?;
    }
    let body = json!({"operation":"mount_create","object_id":object,"destination_workspace_id":input.destination_workspace_id,"destination_project_id":input.destination_project_id,"level":input.level});
    if let Some(result) =
        super::scope_commands::lock_and_replay(&tx, identity.scope_id, user, &input.idempotency_key, &body).await?
    {
        tx.commit().await?;
        return serde_json::from_value(result).map_err(|_| ApiError::internal());
    }

    let mount=MountView::find_by_statement(Statement::from_sql_and_values(DbBackend::Postgres,"INSERT INTO flow_mounts(source_object_id,destination_workspace_id,destination_project_id,grant_level,created_by) VALUES($1,$2,$3,$4,$5) ON CONFLICT(source_object_id,destination_workspace_id,destination_project_id) DO UPDATE SET grant_level=flow_mounts.grant_level RETURNING id,source_object_id,destination_workspace_id,destination_project_id,grant_level AS level,'active'::text AS status",vec![object.into(),input.destination_workspace_id.into(),input.destination_project_id.into(),input.level.clone().into(),user.into()])).one(&tx).await?.ok_or_else(ApiError::internal)?;
    if mount.level != input.level {
        return Err(ApiError::idempotency_key_body_drift(
            "existing mount has a different capability; remove placement explicitly before changing it",
        ));
    }
    scope::advance_revision(&tx, identity.scope_id).await?;
    let event = super::scope_event::insert(
        &tx,
        identity.scope_id,
        user,
        "flow.object.mounted",
        "flow_object",
        object,
        &input.idempotency_key,
        json!({"object_id":object,"mount_id":mount.id}),
        json!({"idempotency_body":body}),
        None,
    )
    .await?;
    super::scope_commands::record(
        &tx,
        identity.scope_id,
        user,
        &input.idempotency_key,
        body,
        object,
        event,
        serde_json::to_value(&mount).map_err(|_| ApiError::internal())?,
    )
    .await?;
    tx.commit().await?;
    Ok(mount)
}
async fn ensure_project<C: ConnectionTrait>(db: &C, workspace: Uuid, project: Uuid) -> Result<(), ApiError> {
    db.query_one(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "SELECT id FROM projects WHERE id=$1 AND workspace_id=$2",
        vec![project.into(), workspace.into()],
    ))
    .await?
    .ok_or_else(|| ApiError::not_found("mount destination unavailable"))?;
    Ok(())
}

/// Change a placement under the same authority fence as creation. The previous level is CAS-bound
/// so two controllers reviewing the same placement cannot unknowingly overwrite each other.
pub async fn change(
    db: &DatabaseConnection,
    user: Uuid,
    object: Uuid,
    mount: Uuid,
    input: ChangeMountInput,
) -> Result<MountView, ApiError> {
    if !valid_level(&input.expected_level)
        || !valid_level(&input.level)
        || input.idempotency_key.trim().is_empty()
        || input.idempotency_key.len() > 128
    {
        return Err(ApiError::invalid_update("invalid mount permission change"));
    }
    let identity = scope::identity(db, object).await?;
    scope::authorize(db, object, user, PermissionLevel::FullAccess).await?;
    let binding = db
        .query_one(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT destination_workspace_id FROM flow_mounts WHERE source_object_id=$1 AND id=$2",
            vec![object.into(), mount.into()],
        ))
        .await?
        .ok_or_else(|| ApiError::not_found("placement not found"))?;
    let destination: Uuid = binding.try_get("", "destination_workspace_id")?;
    let body = json!({"operation":"mount_change","object_id":object,"mount_id":mount,
        "expected_level":input.expected_level,"level":input.level});
    let tx = db.begin().await?;
    require_enabled(&tx).await?;
    tx.execute(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "SELECT id FROM workspaces WHERE id=$1 FOR SHARE",
        vec![destination.into()],
    ))
    .await?;
    authz::lock_epoch_for_share(&tx, destination).await?;
    if !destination_can_manage(&tx, destination, user).await? {
        return Err(ApiError::forbidden("destination publishing permission required"));
    }
    tx.execute(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "SELECT scope_id FROM flow_scope_settings WHERE scope_id=$1 FOR UPDATE",
        vec![identity.scope_id.into()],
    ))
    .await?;
    if !scope::authorize(&tx, object, user, PermissionLevel::FullAccess)
        .await?
        .resource_active
    {
        return Err(ApiError::not_found("mount source unavailable"));
    }
    if let Some(result) =
        super::scope_commands::lock_and_replay(&tx, identity.scope_id, user, &input.idempotency_key, &body).await?
    {
        tx.commit().await?;
        return serde_json::from_value(result).map_err(|_| ApiError::internal());
    }
    let view = MountView::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "UPDATE flow_mounts SET grant_level=$5
         WHERE source_object_id=$1 AND id=$2 AND destination_workspace_id=$3 AND grant_level=$4
         RETURNING id,source_object_id,destination_workspace_id,destination_project_id,
                   grant_level AS level,'active'::text AS status",
        vec![
            object.into(),
            mount.into(),
            destination.into(),
            input.expected_level.into(),
            input.level.into(),
        ],
    ))
    .one(&tx)
    .await?
    .ok_or_else(|| {
        ApiError::policy_rejected_with_details(
            "Mount permission changed",
            json!({"reason":"mount_permission_changed","write_state":"not_applied"}),
        )
    })?;
    scope::advance_revision(&tx, identity.scope_id).await?;
    let event = super::scope_event::insert(
        &tx,
        identity.scope_id,
        user,
        "flow.object.mount_permission_changed",
        "flow_object",
        object,
        &input.idempotency_key,
        json!({"object_id":object,"mount_id":mount,"level":view.level}),
        json!({"idempotency_body":body}),
        None,
    )
    .await?;
    super::scope_commands::record(
        &tx,
        identity.scope_id,
        user,
        &input.idempotency_key,
        body,
        object,
        event,
        serde_json::to_value(&view).map_err(|_| ApiError::internal())?,
    )
    .await?;
    tx.commit().await?;
    Ok(view)
}

pub async fn destination_can_manage<C: ConnectionTrait>(db: &C, workspace: Uuid, user: Uuid) -> Result<bool, ApiError> {
    Ok(db
        .query_one(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT user_id FROM workspace_members WHERE workspace_id=$1 AND user_id=$2 AND role IN ('owner','admin')",
            vec![workspace.into(), user.into()],
        ))
        .await?
        .is_some())
}
pub async fn can_remove(db: &DatabaseConnection, workspace: Uuid, object: Uuid, user: Uuid) -> Result<bool, ApiError> {
    if destination_can_manage(db, workspace, user).await? {
        return Ok(true);
    }
    match scope::authorize(db, object, user, PermissionLevel::FullAccess).await {
        Ok(_) => Ok(true),
        Err(e)
            if matches!(
                e.kind(),
                crate::error::ApiErrorKind::NotFound | crate::error::ApiErrorKind::Forbidden
            ) =>
        {
            Ok(false)
        }
        Err(e) => Err(e),
    }
}
pub async fn remove(db: &DatabaseConnection, user: Uuid, object: Uuid, mount: Uuid) -> Result<(), ApiError> {
    let identity = scope::identity(db, object).await?;
    let key = format!("mount_remove:{mount}");
    if let Some(prior)=db.query_one(Statement::from_sql_and_values(DbBackend::Postgres,"SELECT request_body FROM flow_scope_command_receipts WHERE scope_id=$1 AND idempotency_key=$2 AND actor_id=$3",vec![identity.scope_id.into(),key.clone().into(),user.into()])).await? {
        let old=prior.try_get::<serde_json::Value>("","request_body")?;
        if old.get("destination_workspace_id").is_none(){
            // Early v2 source-only remove receipts did not capture destination authority.
            // Preserve their source-FullAccess replay; never bless a target role retroactively.
            scope::authorize(db,object,user,PermissionLevel::FullAccess).await?;
            if super::scope_commands::replay(db,identity.scope_id,user,&key,&old).await?.is_some(){return Ok(());}
        }
    }
    // On retry, the placement row is gone. Its actor-bound receipt retains the destination
    // identity needed to reauthorize cleanup without granting source management.
    let row=db.query_one(Statement::from_sql_and_values(DbBackend::Postgres,"SELECT destination_workspace_id FROM flow_mounts WHERE id=$1 AND source_object_id=$2 UNION ALL SELECT (request_body->>'destination_workspace_id')::uuid FROM flow_scope_command_receipts WHERE scope_id=$3 AND idempotency_key=$4 AND actor_id=$5",vec![mount.into(),object.into(),identity.scope_id.into(),key.clone().into(),user.into()])).await?.ok_or_else(||ApiError::not_found("placement not found"))?;
    let destination = row.try_get::<Uuid>("", "destination_workspace_id")?;
    let tx = db.begin().await?;
    tx.execute(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "SELECT id FROM workspaces WHERE id=$1 FOR SHARE",
        vec![destination.into()],
    ))
    .await?;
    authz::lock_epoch_for_share(&tx, destination).await?;
    let target_manager = destination_can_manage(&tx, destination, user).await?;
    tx.execute(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "SELECT scope_id FROM flow_scope_settings WHERE scope_id=$1 FOR UPDATE",
        vec![identity.scope_id.into()],
    ))
    .await?;
    if !target_manager {
        scope::authorize(&tx, object, user, PermissionLevel::FullAccess).await?;
    }
    let body =
        json!({"operation":"mount_remove","object_id":object,"mount_id":mount,"destination_workspace_id":destination});
    if super::scope_commands::lock_and_replay(&tx, identity.scope_id, user, &key, &body)
        .await?
        .is_some()
    {
        tx.commit().await?;
        return Ok(());
    }
    tx.execute(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "DELETE FROM flow_mounts WHERE id=$1 AND source_object_id=$2",
        vec![mount.into(), object.into()],
    ))
    .await?;
    scope::advance_revision(&tx, identity.scope_id).await?;
    let event = super::scope_event::insert(
        &tx,
        identity.scope_id,
        user,
        "flow.object.unmounted",
        "flow_object",
        object,
        &key,
        json!({"object_id":object,"mount_id":mount}),
        json!({"idempotency_body":body}),
        None,
    )
    .await?;
    super::scope_commands::record(
        &tx,
        identity.scope_id,
        user,
        &key,
        body,
        object,
        event,
        json!({"removed":true}),
    )
    .await?;
    tx.commit().await?;
    Ok(())
}

/// General destination deletion follows the same audience -> source-scope order as mount writes.
/// The caller deletes the destination in this transaction; canonical source rows are never touched.
pub async fn fence_destination_delete(
    tx: &sea_orm::DatabaseTransaction,
    workspace: Uuid,
    project: Option<Uuid>,
) -> Result<(), ApiError> {
    authz::advance_epoch_if_present(tx, workspace).await?;
    let scopes=tx.query_all(Statement::from_sql_and_values(DbBackend::Postgres,
        "SELECT DISTINCT o.scope_id FROM flow_mounts m JOIN flow_objects o ON o.id=m.source_object_id WHERE m.destination_workspace_id=$1 AND ($2::uuid IS NULL OR m.destination_project_id=$2) ORDER BY o.scope_id",
        vec![workspace.into(),project.into()])).await?;
    for row in scopes {
        let scope_id: Uuid = row.try_get("", "scope_id")?;
        tx.execute(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT scope_id FROM flow_scope_settings WHERE scope_id=$1 FOR UPDATE",
            vec![scope_id.into()],
        ))
        .await?;
        scope::advance_revision(tx, scope_id).await?;
    }
    Ok(())
}
