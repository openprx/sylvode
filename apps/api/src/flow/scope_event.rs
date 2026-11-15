//! Independent Flow event source. Legacy adapters retain Workspace source/replay namespaces.
#[derive(FromQueryResult)]
struct ExistingScopeEvent {
    id: Uuid,
    event_type: String,
    aggregate_type: String,
    aggregate_id: String,
    actor_id: Option<Uuid>,
}

use crate::error::ApiError;
use sea_orm::{ConnectionTrait, DbBackend, FromQueryResult, Statement};
use serde_json::{Value, json};
use uuid::Uuid;

#[allow(clippy::too_many_arguments)]
pub async fn insert<C: ConnectionTrait>(
    tx: &C,
    scope: Uuid,
    user: Uuid,
    event_type: &str,
    aggregate_type: &str,
    aggregate: Uuid,
    key: &str,
    payload: Value,
    metadata: Value,
    accepted: Option<(Uuid, i64)>,
) -> Result<Uuid, ApiError> {
    insert_with_delivery(
        tx,
        scope,
        user,
        event_type,
        aggregate_type,
        aggregate,
        key,
        payload,
        metadata,
        accepted,
        true,
    )
    .await
}
#[allow(clippy::too_many_arguments)]
async fn insert_with_delivery<C: ConnectionTrait>(
    tx: &C,
    scope: Uuid,
    user: Uuid,
    event_type: &str,
    aggregate_type: &str,
    aggregate: Uuid,
    key: &str,
    payload: Value,
    metadata: Value,
    accepted: Option<(Uuid, i64)>,
    dispatch: bool,
) -> Result<Uuid, ApiError> {
    #[derive(FromQueryResult)]
    struct Row {
        id: Uuid,
    }
    let proposed = Uuid::new_v4();
    let inserted=Row::find_by_statement(Statement::from_sql_and_values(DbBackend::Postgres,r"
 INSERT INTO business_events(id,workspace_id,source_scope_kind,source_flow_scope_id,event_type,aggregate_type,aggregate_id,actor_id,source,payload,metadata,idempotency_key)
 VALUES($1,NULL,'flow',$2,$3,$4,$5,$6,$7,$8,$9,$10)
 ON CONFLICT(source_flow_scope_id,idempotency_key) WHERE source_scope_kind='flow' AND idempotency_key IS NOT NULL DO NOTHING RETURNING id",vec![proposed.into(),scope.into(),event_type.into(),aggregate_type.into(),aggregate.to_string().into(),user.into(),json!({"surface":"rest","source_scope_kind":"flow","scope_id":scope}).into(),payload.into(),metadata.into(),key.into()])).one(tx).await?;
    if let Some(inserted) = inserted {
        let (document, seq) = accepted.map_or((None, None), |(d, s)| (Some(d), Some(s)));
        if dispatch {
            tx.execute(Statement::from_sql_and_values(DbBackend::Postgres,
   "INSERT INTO event_dispatch(id,event_id,workspace_id,source_flow_scope_id,event_type,document_id,accepted_seq,max_attempts) VALUES($1,$2,NULL,$3,$4,$5,$6,$7)",
   vec![Uuid::new_v4().into(),inserted.id.into(),scope.into(),event_type.into(),document.into(),seq.into(),crate::config::runtime().flow.dispatch_max_attempts.into()])).await?;
        }
        return Ok(inserted.id);
    }

    let existing=ExistingScopeEvent::find_by_statement(Statement::from_sql_and_values(DbBackend::Postgres,"SELECT id,event_type,aggregate_type,aggregate_id,actor_id FROM business_events WHERE source_scope_kind='flow' AND source_flow_scope_id=$1 AND idempotency_key=$2",vec![scope.into(),key.into()])).one(tx).await?.ok_or_else(ApiError::internal)?;
    if existing.event_type != event_type
        || existing.aggregate_type != aggregate_type
        || existing.aggregate_id != aggregate.to_string()
        || existing.actor_id != Some(user)
    {
        return Err(ApiError::idempotency_key_body_drift("event operation identity changed"));
    }
    Ok(existing.id)
}

/// Integration adapter for shared application services. Existing legacy writes retain their
/// historical Workspace namespace; independent writes use the canonical Flow source namespace.
#[allow(clippy::too_many_arguments)]
pub async fn insert_for_object<C: ConnectionTrait>(
    tx: &C,
    object: &super::scope::ObjectIdentity,
    user: Uuid,
    event_type: &str,
    aggregate_type: &str,
    aggregate: Uuid,
    key: &str,
    payload: Value,
    metadata: Value,
) -> Result<Uuid, ApiError> {
    insert_for_object_delivery(
        tx,
        object,
        user,
        event_type,
        aggregate_type,
        aggregate,
        key,
        payload,
        metadata,
        true,
        None,
    )
    .await
}
#[allow(clippy::too_many_arguments)]
async fn insert_for_object_delivery<C: ConnectionTrait>(
    tx: &C,
    object: &super::scope::ObjectIdentity,
    user: Uuid,
    event_type: &str,
    aggregate_type: &str,
    aggregate: Uuid,
    key: &str,
    payload: Value,
    metadata: Value,
    dispatch: bool,
    accepted: Option<(Uuid, i64)>,
) -> Result<Uuid, ApiError> {
    if let Some(workspace) = object.workspace_id {
        let outcome = crate::events::insert_flow_event(
            tx,
            crate::events::BusinessEventInput {
                workspace_id: workspace,
                project_id: object.project_id,
                event_type: event_type.into(),
                aggregate_type: aggregate_type.into(),
                aggregate_id: aggregate.to_string(),
                actor_id: Some(user),
                source: json!({"surface":"rest"}),
                payload,
                metadata,
                correlation_id: Some(Uuid::new_v4()),
                causation_id: None,
                idempotency_key: Some(key.into()),
            },
            if dispatch {
                Some(crate::events::FlowDispatchSpec {
                    max_attempts: crate::config::runtime().flow.dispatch_max_attempts,
                    document_id: accepted.map(|v| v.0),
                    accepted_seq: accepted.map(|v| v.1),
                })
            } else {
                None
            },
        )
        .await?;
        if !outcome.was_new {
            let row = tx
                .query_one(Statement::from_sql_and_values(
                    DbBackend::Postgres,
                    "SELECT event_type,aggregate_type,aggregate_id,actor_id FROM business_events WHERE id=$1",
                    vec![outcome.event_id.into()],
                ))
                .await?
                .ok_or_else(ApiError::internal)?;
            if row.try_get::<String>("", "event_type")? != event_type
                || row.try_get::<String>("", "aggregate_type")? != aggregate_type
                || row.try_get::<String>("", "aggregate_id")? != aggregate.to_string()
                || row.try_get::<Option<Uuid>>("", "actor_id")? != Some(user)
            {
                return Err(ApiError::idempotency_key_body_drift(
                    "legacy event operation identity changed",
                ));
            }
        }
        return Ok(outcome.event_id);
    }
    insert_with_delivery(
        tx,
        object.scope_id,
        user,
        event_type,
        aggregate_type,
        aggregate,
        key,
        payload,
        metadata,
        accepted,
        dispatch,
    )
    .await
}

/// Unpublished asset metadata is audit-only; a public object grant does not publish every upload.
#[allow(clippy::too_many_arguments)]
pub async fn insert_object_audit<C: ConnectionTrait>(
    tx: &C,
    object: &super::scope::ObjectIdentity,
    user: Uuid,
    event_type: &str,
    key: &str,
    payload: Value,
) -> Result<Uuid, ApiError> {
    insert_for_object_delivery(
        tx,
        object,
        user,
        event_type,
        "flow_object",
        object.id,
        key,
        payload,
        json!({}),
        false,
        None,
    )
    .await
}

/// Accepted editor initialization shares the legacy or personal event namespace and ordering.
pub async fn insert_content_accepted<C: ConnectionTrait>(
    tx: &C,
    object: &super::scope::ObjectIdentity,
    user: Uuid,
    document: Uuid,
    seq: i64,
    key: &str,
    action: &str,
) -> Result<Uuid, ApiError> {
    insert_for_object_delivery(tx,object,user,"flow.content.accepted","flow_document",document,key,json!({"object_id":object.id,"document_id":document,"accepted_seq":seq,"projection_seq":seq,"changed_block_ids":[]}),json!({"semantic_summary":{"action":action}}),true,Some((document,seq))).await
}
