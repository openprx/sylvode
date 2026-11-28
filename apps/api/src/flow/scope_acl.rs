//! Personal grant/inheritance commands share one authority lock and one transactional receipt.
//! Workspace authority mutations continue through their original workspace adapter/fence.
use super::{collab::authz::PermissionLevel, grants::GrantRequest, scope};
use crate::error::{ApiError, ApiErrorKind};
use crate::flow::json_value::FlowJson;
use sea_orm::{ConnectionTrait, DatabaseConnection, DbBackend, Statement, TransactionTrait};
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};
use uuid::Uuid;
pub struct Mutation {
    pub grants: Option<Vec<GrantRequest>>,
    pub inheritance: Option<bool>,
    pub dry_run: bool,
    pub confirm_self_lockout: bool,
    pub idempotency_key: String,
}
async fn level<C: ConnectionTrait>(db: &C, object: Uuid, user: Uuid) -> Result<PermissionLevel, ApiError> {
    match scope::authorize(db, object, user, PermissionLevel::View).await {
        Ok(proof) => Ok(proof.level),
        Err(err) if matches!(err.kind(), ApiErrorKind::NotFound | ApiErrorKind::Forbidden) => {
            Ok(PermissionLevel::Denied)
        }
        Err(err) => Err(err),
    }
}
pub async fn mutate(db: &DatabaseConnection, object: Uuid, user: Uuid, input: Mutation) -> Result<Value, ApiError> {
    let proof = scope::authorize(db, object, user, PermissionLevel::View).await?;
    if proof.level < PermissionLevel::FullAccess {
        return Err(ApiError::forbidden(
            "full_access is required to manage object authority",
        ));
    }
    let identity = scope::identity(db, object).await?;
    if identity.workspace_id.is_some() {
        return Err(ApiError::invalid_update(
            "workspace authority changes require their integration adapter",
        ));
    }
    if input
        .grants
        .as_ref()
        .is_some_and(|g| g.len() > super::collab::authz::OBJECT_GRANTS_MAX)
    {
        return Err(ApiError::invalid_update("too many grants"));
    }
    let requested = input.grants.as_ref().map(|grants| {
        grants
            .iter()
            .map(|g| json!({"principal_kind":g.principal_kind,"principal_id":g.principal_id,"level":g.level}))
            .collect::<Vec<_>>()
    });
    let operation = if input.inheritance.is_some() {
        "set_inheritance"
    } else {
        "replace_grants"
    };
    let body = json!({"operation":operation,"object_id":object,"grants":requested,"inherit_from_parent":input.inheritance,"dry_run":input.dry_run,"confirm_self_lockout":input.confirm_self_lockout});
    let tx = db.begin().await?;
    scope::lock_change(&tx, &proof).await?;
    if let Some(result) =
        super::scope_commands::lock_and_replay(&tx, identity.scope_id, user, &input.idempotency_key, &body).await?
    {
        tx.commit().await?;
        return Ok(result);
    }
    scope::authorize(&tx, object, user, PermissionLevel::FullAccess).await?;
    let old_inherit = tx
        .query_one(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT inherit_from_parent FROM flow_objects WHERE id=$1",
            vec![object.into()],
        ))
        .await?
        .ok_or_else(|| ApiError::not_found("flow object not found"))?
        .try_get::<bool>("", "inherit_from_parent")?;
    let rows = tx
        .query_all(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT principal_kind,principal_id,level FROM flow_object_grants WHERE object_id=$1",
            vec![object.into()],
        ))
        .await?;
    let mut old = BTreeMap::new();
    let mut affected = BTreeSet::new();
    for row in rows {
        let kind = row.try_get::<String>("", "principal_kind")?;
        let id = row.try_get::<Uuid>("", "principal_id")?;
        let value = row.try_get::<String>("", "level")?;
        if kind == "user" {
            affected.insert(id);
        }
        old.insert((kind, id), value);
    }
    let mut next = BTreeMap::new();
    if let Some(grants) = &input.grants {
        for grant in grants {
            if grant.principal_kind != "user"
                || PermissionLevel::parse_grant_level(&grant.level).is_none()
                || next
                    .insert((grant.principal_kind.clone(), grant.principal_id), grant.level.clone())
                    .is_some()
            {
                return Err(ApiError::invalid_update("invalid or duplicate personal grant"));
            }
            affected.insert(grant.principal_id);
        }
    } else {
        next = old.clone();
    }
    let reduction = (!input.inheritance.unwrap_or(old_inherit) || old_inherit)
        && next.iter().all(|(principal, value)| {
            old.get(principal)
                .and_then(|old| PermissionLevel::parse_grant_level(old))
                .is_some_and(|old| PermissionLevel::parse_grant_level(value).is_some_and(|new| new <= old))
        });
    if !reduction {
        scope::require_personal_write(&tx).await?;
    }
    let mut before = BTreeMap::new();
    for principal in &affected {
        before.insert(*principal, level(&tx, object, *principal).await?);
    }
    if let Some(inherit) = input.inheritance {
        tx.execute(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "UPDATE flow_objects SET inherit_from_parent=$2,updated_at=now(),updated_by=$3 WHERE id=$1",
            vec![object.into(), inherit.into(), user.into()],
        ))
        .await?;
    }
    if input.grants.is_some() {
        tx.execute(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "DELETE FROM flow_object_grants WHERE object_id=$1",
            vec![object.into()],
        ))
        .await?;
        for ((kind, principal), value) in &next {
            let inserted=tx.execute(Statement::from_sql_and_values(DbBackend::Postgres,"INSERT INTO flow_object_grants(id,scope_id,workspace_id,object_id,principal_kind,principal_id,level,granted_by) SELECT $1,$2,NULL,$3,$4,u.id,$6,$7 FROM users u WHERE u.id=$5 AND u.is_active",vec![Uuid::new_v4().into(),identity.scope_id.into(),object.into(),kind.clone().into(),(*principal).into(),value.clone().into(),user.into()])).await?;
            if inserted.rows_affected() != 1 {
                return Err(ApiError::invalid_update(
                    "grant principal must be an active global user",
                ));
            }
        }
    }
    let after = level(&tx, object, user).await?;
    let loses = after < PermissionLevel::FullAccess;
    if loses && !input.confirm_self_lockout {
        return Err(ApiError::policy_rejected("confirm_self_lockout is required"));
    }
    let mut changes = Vec::new();
    for principal in affected {
        let next_level = level(&tx, object, principal).await?;
        let old_level = before.get(&principal).copied().unwrap_or(PermissionLevel::Denied);
        if old_level != next_level {
            changes.push(json!({"principal_kind":"user","principal_id":principal,"before_level":old_level.as_wire(),"after_level":next_level.as_wire()}));
        }
    }
    let permission_changes = json!({"caller":{"before_level":proof.level.as_wire(),"after_level":after.as_wire(),"loses_full_access":loses},"affected":changes});
    let inherit = input.inheritance.unwrap_or(old_inherit);
    if input.dry_run {
        tx.rollback().await?;
        let mut result = json!({"applied":false,"event_id":null,"permission_changes":permission_changes});
        if input.inheritance.is_some() {
            *result.field_mut("inherit_from_parent")? = json!(inherit);
        }
        return Ok(result);
    }
    let revision = scope::advance_revision(&tx, identity.scope_id).await?;
    let event_type = if input.inheritance.is_some() {
        "flow.permission.inheritance_changed"
    } else {
        "flow.permission.grants_replaced"
    };
    let event = super::scope_event::insert_for_object(
        &tx,
        &identity,
        user,
        event_type,
        "flow_object",
        object,
        &input.idempotency_key,
        json!({"object_id":object,"inherit_from_parent":inherit,"changed_principal_count":changes.len()}),
        json!({}),
    )
    .await?;
    let mut result =
        json!({"applied":true,"event_id":event,"authz_revision":revision,"permission_changes":permission_changes});
    if input.inheritance.is_some() {
        *result.field_mut("inherit_from_parent")? = json!(inherit);
    }
    super::scope_commands::record(
        &tx,
        identity.scope_id,
        user,
        &input.idempotency_key,
        body,
        object,
        event,
        result.clone(),
    )
    .await?;
    tx.commit().await?;
    Ok(result)
}
