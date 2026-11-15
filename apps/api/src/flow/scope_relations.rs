//! Ordinary reversible references inside one canonical scope.
//!
//! Both endpoints require edit;
//! embeddings are created only by the typed atomic embed command. Reads reuse opaque cursors
//! and never expose identifiers/properties for an inaccessible opposite endpoint.
use super::{
    collab::authz::PermissionLevel,
    relations::{self, ListRelationsParams, RelationDirection},
    scope,
};
use crate::error::{ApiError, ApiErrorKind};
use crate::flow::json_value::FlowJson;
use chrono::{DateTime, Utc};
use platform::app::AppState;
use sea_orm::{ConnectionTrait, DbBackend, FromQueryResult, Statement, TransactionTrait};
use serde_json::{Value, json};
use uuid::Uuid;
#[derive(FromQueryResult)]
struct StoredEdge {
    id: Uuid,
    source_object_id: Uuid,
    target_object_id: Uuid,
    relation_type: String,
    position_key: String,
    properties: Value,
    created_at: DateTime<Utc>,
}
pub async fn execute(
    state: &AppState,
    object: Uuid,
    user: Uuid,
    kind: &str,
    payload: Value,
    key: &str,
    expected: Option<&str>,
) -> Result<Value, ApiError> {
    if expected.is_some() {
        return Err(ApiError::invalid_update(
            "link/unlink advance no document head and reject expected_frontier",
        ));
    }
    let source = scope::identity(&state.db, object).await?;
    let source_proof = scope::authorize(&state.db, object, user, PermissionLevel::Edit).await?;
    if source.workspace_id.is_some() {
        return Err(ApiError::invalid_update(
            "legacy relations require their integration adapter",
        ));
    }
    let body = json!({"operation":kind,"object_id":object,"payload":payload});
    if let Some(result) = super::scope_commands::replay(&state.db, source.scope_id, user, key, &body).await? {
        return Ok(result);
    }
    let (target, relation_type, properties, position, requested_relation) = if kind == "link" {
        let input: relations::LinkPayload =
            serde_json::from_value(payload).map_err(|_| ApiError::invalid_update("invalid link payload"))?;
        if !relations::valid_relation_type(&input.relation_type) || !input.properties.is_object() {
            return Err(ApiError::invalid_update(
                "relation_type must be a registered identifier and properties an object",
            ));
        }
        if input.relation_type == "embeds" {
            return Err(ApiError::invalid_update(
                "embeds are server-managed by create_collection_embed",
            ));
        }
        (
            input.target_object_id,
            input.relation_type,
            input.properties,
            input.position_key.unwrap_or_default(),
            None,
        )
    } else if kind == "unlink" {
        let input: relations::UnlinkPayload =
            serde_json::from_value(payload).map_err(|_| ApiError::invalid_update("invalid unlink payload"))?;
        let row=StoredEdge::find_by_statement(Statement::from_sql_and_values(DbBackend::Postgres,"SELECT id,scope_id,source_object_id,target_object_id,relation_type,position_key,properties,created_at FROM flow_relations WHERE id=$1 AND source_object_id=$2 AND scope_id=$3",vec![input.relation_id.into(),object.into(),source.scope_id.into()])).one(&state.db).await?.ok_or_else(||ApiError::not_found("flow relation not found"))?;
        if row.relation_type == "embeds" {
            return Err(ApiError::invalid_update(
                "embedded content references require their document command",
            ));
        }
        (
            row.target_object_id,
            row.relation_type,
            row.properties,
            row.position_key,
            Some(row.id),
        )
    } else {
        return Err(ApiError::invalid_update("relation command must be link or unlink"));
    };
    let target_identity = scope::identity(&state.db, target).await?;
    let target_proof = scope::authorize(&state.db, target, user, PermissionLevel::Edit).await?;
    if target_identity.scope_id != source.scope_id {
        return Err(ApiError::invalid_update(
            "ordinary reference endpoints must belong to the same Flow scope",
        ));
    }
    if kind == "link" && (!source_proof.resource_active || !target_proof.resource_active) {
        return Err(ApiError::invalid_update("link participants must be active"));
    }
    let tx = state.db.begin().await?;
    if kind == "link" {
        scope::require_personal_write(&tx).await?;
    }
    scope::fence_many(&tx, &[source_proof, target_proof]).await?;
    if let Some(result) = super::scope_commands::lock_and_replay(&tx, source.scope_id, user, key, &body).await? {
        tx.commit().await?;
        return Ok(result);
    }
    tx.query_all(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "SELECT id FROM flow_objects WHERE id=ANY($1) AND scope_id=$2 ORDER BY id FOR UPDATE",
        vec![vec![object, target].into(), source.scope_id.into()],
    ))
    .await?;
    let relation = if kind == "link" {
        let relation = Uuid::new_v4();
        let inserted=tx.execute(Statement::from_sql_and_values(DbBackend::Postgres,"INSERT INTO flow_relations(id,scope_id,workspace_id,relation_type,source_object_id,target_object_id,position_key,properties,created_by) VALUES($1,$2,NULL,$3,$4,$5,$6,$7,$8) ON CONFLICT(scope_id,source_object_id,target_object_id,relation_type) DO NOTHING",vec![relation.into(),source.scope_id.into(),relation_type.clone().into(),object.into(),target.into(),position.into(),properties.into(),user.into()])).await?;
        if inserted.rows_affected() != 1 {
            return Err(ApiError::invalid_update("that relation identity already exists"));
        }
        relation
    } else {
        let relation = requested_relation.ok_or_else(ApiError::internal)?;
        let removed = tx
            .execute(Statement::from_sql_and_values(
                DbBackend::Postgres,
                "DELETE FROM flow_relations WHERE id=$1 AND source_object_id=$2 AND scope_id=$3",
                vec![relation.into(), object.into(), source.scope_id.into()],
            ))
            .await?;
        if removed.rows_affected() != 1 {
            return Err(ApiError::not_found("flow relation not found"));
        }
        relation
    };
    let event=super::scope_event::insert_for_object(&tx,&source,user,if kind=="link"{"flow.relation.linked"}else{"flow.relation.unlinked"},"flow_relation",relation,key,json!({"object_id":object,"relation_id":relation,"source_object_id":object,"target_object_id":target,"relation_type":relation_type}),json!({})).await?;
    let mut result = scope::accepted(scope::object_view(&tx, object, user).await?, event);
    *result.field_mut("affected_object_ids")? = json!([object, target]);
    *result.field_mut("command_result")? = if kind == "link" {
        json!({"relation":{"relation_id":relation,"relation_type":relation_type,"source_object_id":object,"target_object_id":target},"existing_document_cardinality":0})
    } else {
        json!({"removed":true,"event_id":event,"existing_document_cardinality":0})
    };
    super::scope_commands::record(&tx, source.scope_id, user, key, body, object, event, result.clone()).await?;
    tx.commit().await?;
    Ok(result)
}
pub async fn list(state: &AppState, object: Uuid, user: Uuid, params: ListRelationsParams) -> Result<Value, ApiError> {
    let proof = scope::authorize(&state.db, object, user, PermissionLevel::View).await?;
    let limit = super::query::validate_limit(params.limit)?;
    let after = params
        .cursor
        .as_deref()
        .map(|v| relations::decode_cursor(state.cfg.jwt_secret.expose(), v))
        .transpose()?;
    if params
        .relation_type
        .as_deref()
        .is_some_and(|v| !relations::valid_relation_type(v))
    {
        return Err(ApiError::invalid_update("invalid relation type filter"));
    }
    let direction = match params.direction {
        RelationDirection::Outgoing => "outgoing",
        RelationDirection::Incoming => "incoming",
        RelationDirection::Both => "both",
    };
    let rows=StoredEdge::find_by_statement(Statement::from_sql_and_values(DbBackend::Postgres,"SELECT id,scope_id,source_object_id,target_object_id,relation_type,position_key,properties,created_at FROM flow_relations WHERE scope_id=$1 AND (($3='both' AND (source_object_id=$2 OR target_object_id=$2)) OR ($3='outgoing' AND source_object_id=$2) OR ($3='incoming' AND target_object_id=$2)) AND ($4::text IS NULL OR relation_type=$4) AND ($5::timestamptz IS NULL OR (created_at,id)>($5,$6)) ORDER BY created_at,id LIMIT $7",vec![proof.scope_id.into(),object.into(),direction.into(),params.relation_type.into(),after.map(|v|v.0).into(),after.map(|v|v.1).into(),i64::try_from(limit+1).unwrap_or(101).into()])).all(&state.db).await?;
    let more = rows.len() > usize::try_from(limit).unwrap_or(100);
    let mut items = Vec::new();
    let mut last = None;
    for row in rows.into_iter().take(usize::try_from(limit).unwrap_or(100)) {
        last = Some((row.created_at, row.id));
        let outgoing = row.source_object_id == object;
        let other = if outgoing {
            row.target_object_id
        } else {
            row.source_object_id
        };
        match scope::object_view(&state.db, other, user).await {
            Ok(view) => {
                let mut other_object = json!({"id":other,"scope_id":view.field("scope_id"),"object_type":view.field("object_type"),"title":view.field("title"),"lifecycle_status":view.field("lifecycle_status")});
                if let Some(project) = view.field("project_id").as_str() {
                    *other_object.field_mut("project_id")? = json!(project);
                }
                items.push(json!({"visibility":"visible","relation_id":row.id,"relation_type":row.relation_type,"direction":if outgoing{"outgoing"}else{"incoming"},"position_key":row.position_key,"properties":row.properties,"created_at":row.created_at.to_rfc3339(),"other_object":other_object}));
            }
            Err(err) if matches!(err.kind(), ApiErrorKind::NotFound | ApiErrorKind::Forbidden) => {
                items.push(json!({"visibility":"unavailable"}));
            }
            Err(err) => return Err(err),
        }
    }
    if !scope::proof_current(&state.db, &proof).await? {
        return Err(super::policy::authorization_read_unstable());
    }
    let cursor = if more {
        last.map(|(created, id)| relations::encode_cursor(state.cfg.jwt_secret.expose(), created, id))
            .transpose()?
    } else {
        None
    };
    Ok(json!({"items":items,"next_cursor":cursor}))
}
