//! Scope-wide, actor-and-request-bound operation identities shared by every personal mutation.
use crate::error::ApiError;
use sea_orm::{ConnectionTrait, DbBackend, FromQueryResult, Statement};
use serde_json::Value;
use uuid::Uuid;
#[derive(FromQueryResult)]
struct Receipt {
    actor_id: Uuid,
    request_body: Value,
    result: Value,
    object_id: Uuid,
    deleted: bool,
}
pub async fn replay<C: ConnectionTrait>(
    db: &C,
    scope: Uuid,
    actor: Uuid,
    key: &str,
    body: &Value,
) -> Result<Option<Value>, ApiError> {
    if key.trim().is_empty() || key.len() > 128 {
        return Err(ApiError::invalid_update("operation key must contain1..128 bytes"));
    }
    let receipt=Receipt::find_by_statement(Statement::from_sql_and_values(DbBackend::Postgres,"SELECT actor_id,request_body,result,object_id,false AS deleted FROM flow_scope_command_receipts WHERE scope_id=$1 AND idempotency_key=$2 UNION ALL SELECT actor_id,request_body,result,object_id,true AS deleted FROM flow_purge_receipts WHERE scope_id=$1 AND idempotency_key=$2",vec![scope.into(),key.into()])).one(db).await?;
    if let Some(receipt) = receipt {
        if receipt.actor_id != actor || receipt.request_body != *body {
            return Err(ApiError::idempotency_key_body_drift("scope operation identity changed"));
        }
        if receipt.deleted {
            // A deletion summary has no surviving object to authorize. Only its original
            // authenticated actor can replay it; active-user and scope lifecycle still apply.
            let active = db
                .query_one(Statement::from_sql_and_values(
                    DbBackend::Postgres,
                    "SELECT id FROM users WHERE id=$1 AND is_active",
                    vec![actor.into()],
                ))
                .await?;
            if active.is_none() || super::scope::get(db, scope).await?.lifecycle_status != "active" {
                return Err(ApiError::not_found("Flow receipt not found"));
            }
            return Ok(Some(receipt.result));
        }
        if body["operation"] == "mount_remove"
            && let Some(destination) = body["destination_workspace_id"]
                .as_str()
                .and_then(|v| Uuid::parse_str(v).ok())
            && super::scope_mounts::destination_can_manage(db, destination, actor).await?
        {
            return Ok(Some(receipt.result));
        }
        super::scope::authorize(
            db,
            receipt.object_id,
            actor,
            super::collab::authz::PermissionLevel::View,
        )
        .await?;
        let mut result = receipt.result;
        if let Some(object) = result.get_mut("object") {
            if object.get("object_type").and_then(Value::as_str) == Some("collection") {
                super::collections::redact_collection_content(db, receipt.object_id, &mut object["semantic_content"])
                    .await?;
            }
            if !super::collections::secret_record_ids(db, &[receipt.object_id])
                .await?
                .is_empty()
            {
                object["semantic_content"] = super::collections::redacted_semantic_content();
            }
        }
        return Ok(Some(result));
    }
    Ok(None)
}
pub async fn lock_and_replay<C: ConnectionTrait>(
    tx: &C,
    scope: Uuid,
    actor: Uuid,
    key: &str,
    body: &Value,
) -> Result<Option<Value>, ApiError> {
    tx.execute(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "SELECT pg_advisory_xact_lock(hashtextextended($1,0))",
        vec![format!("{scope}:{key}").into()],
    ))
    .await?;
    replay(tx, scope, actor, key, body).await
}
#[allow(clippy::too_many_arguments)]
pub async fn record<C: ConnectionTrait>(
    tx: &C,
    scope: Uuid,
    actor: Uuid,
    key: &str,
    body: Value,
    object: Uuid,
    event: Uuid,
    result: Value,
) -> Result<(), ApiError> {
    tx.execute(Statement::from_sql_and_values(DbBackend::Postgres,"INSERT INTO flow_scope_command_receipts(scope_id,idempotency_key,actor_id,request_body,object_id,event_id,result) VALUES($1,$2,$3,$4,$5,$6,$7)",vec![scope.into(),key.into(),actor.into(),body.into(),object.into(),event.into(),result.into()])).await?;
    Ok(())
}
