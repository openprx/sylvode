//! Scope-native typed application adapter. Field validation, operation planning, typed SQL query
//! construction, projection synchronization and semantic event planning are shared with v1.
use super::{
    collab::{authz::PermissionLevel, bootstrap, write::AcceptOutcome},
    collections::{self, CollectionCommandType, RecordQueryPayload},
    scope,
};
use crate::error::ApiError;
use base64::{Engine, engine::general_purpose::STANDARD as BASE64};
use collab_core::{CollabEngine, LoroCollabEngine, NodeKind, Operation};
use platform::app::AppState;
use sea_orm::{ConnectionTrait, DatabaseTransaction, DbBackend, Statement, TransactionTrait};
use serde::Deserialize;
use serde_json::{Map, Value, json};
use std::collections::BTreeSet;
use uuid::Uuid;

/// Server-created commit capability; never deserialized from a request or obtainable by a raw
/// CRDT client. The command planner must validate typed intent before constructing it.
pub(crate) struct TypedCommit {
    collection: Uuid,
    record: Option<Uuid>,
    command: String,
    payload: Value,
    embed: Option<EmbedCommit>,
    record_guard: Option<super::scope_record_context::RecordSchemaGuard>,
}
struct EmbedCommit {
    collection: Uuid,
    document: Uuid,
    block: Uuid,
    title: String,
    schema: Value,
    display: Value,
}
impl TypedCommit {
    pub(crate) const fn is_record(&self) -> bool {
        self.record.is_some()
    }
    pub(crate) const fn record_id(&self) -> Option<Uuid> {
        self.record
    }
    pub(crate) const fn minimum(&self) -> PermissionLevel {
        if self.record.is_some() || self.embed.is_some() {
            PermissionLevel::Edit
        } else {
            PermissionLevel::FullAccess
        }
    }
}
pub(crate) async fn lock_projection(tx: &DatabaseTransaction, typed: &TypedCommit, user: Uuid) -> Result<(), ApiError> {
    if typed.embed.is_some() {
        return Ok(());
    }

    let sql = if typed.record.is_some() {
        "SELECT collection_id FROM flow_collection_projections WHERE collection_id=$1 FOR SHARE"
    } else {
        "SELECT collection_id FROM flow_collection_projections WHERE collection_id=$1 FOR UPDATE"
    };
    tx.execute(Statement::from_sql_and_values(
        DbBackend::Postgres,
        sql,
        vec![typed.collection.into()],
    ))
    .await?;
    if typed.record.is_some() && typed.record_guard.is_none() {
        let parent = scope::identity(tx, typed.collection).await?;
        if parent.lifecycle_status != "active" {
            return Err(ApiError::policy_rejected(
                "Record container is unavailable for content changes",
            ));
        }
        scope::authorize(tx, typed.collection, user, PermissionLevel::Edit).await?;
    }
    if let (Some(record), Some(guard)) = (typed.record, typed.record_guard.as_ref()) {
        super::scope_record_context::validate_guard(tx, typed.collection, record, user, guard, &typed.payload).await?;
    }
    if let Some(properties) = typed.payload.get("properties").and_then(Value::as_object) {
        collections::validate_properties_against_fields(tx, typed.collection, properties).await?;
        validate_relation_access(tx, typed.collection, properties, user).await?;
    }
    Ok(())
}
async fn validate_relation_access<C: ConnectionTrait>(
    db: &C,
    collection: Uuid,
    properties: &Map<String, Value>,
    user: Uuid,
) -> Result<(), ApiError> {
    for (field, value) in properties {
        let field =
            Uuid::parse_str(field).map_err(|_| ApiError::invalid_update("record property key must be a field UUID"))?;
        let relation=db.query_one(Statement::from_sql_and_values(DbBackend::Postgres,"SELECT field_type='relation' AS relation FROM flow_field_projections WHERE collection_id=$1 AND field_id=$2",vec![collection.into(),field.into()])).await?.and_then(|r|r.try_get::<bool>("","relation").ok()).unwrap_or(false);
        if relation && let Some(values) = value.as_array() {
            for id in values {
                let id = id
                    .as_str()
                    .and_then(|s| Uuid::parse_str(s).ok())
                    .ok_or_else(|| ApiError::invalid_update("relation target must be an object identity"))?;
                if scope::authorize(db, id, user, PermissionLevel::View).await.is_err() {
                    return Err(ApiError::invalid_update("relation target is unavailable"));
                }
            }
        }
    }
    Ok(())
}
pub(crate) async fn sync(
    tx: &DatabaseTransaction,
    typed: &TypedCommit,
    object: Uuid,
    user: Uuid,
    seq: i64,
    candidate: &LoroCollabEngine,
    event: Uuid,
) -> Result<(), ApiError> {
    let target = typed.record.unwrap_or(typed.collection);
    if target != object {
        return Err(ApiError::policy_rejected("typed commit target mismatch"));
    }
    if let Some(embed) = typed.embed.as_ref() {
        let identity = scope::identity(tx, object).await?;
        super::collab::authz::ensure_scope_parent_can_adopt_child(tx, identity.scope_id, object).await?;
        scope::insert_object_with_identity(
            tx,
            identity.scope_id,
            user,
            "collection",
            Some(object),
            &embed.title,
            Some(&embed.schema),
            embed.collection,
            embed.document,
        )
        .await?;
        tx.execute(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "UPDATE flow_collection_projections SET display_settings=$2 WHERE collection_id=$1",
            vec![embed.collection.into(), embed.display.clone().into()],
        ))
        .await?;
        tx.execute(Statement::from_sql_and_values(DbBackend::Postgres,"INSERT INTO flow_relations(id,scope_id,workspace_id,relation_type,source_object_id,target_object_id,position_key,properties,created_by) VALUES($1,$2,NULL,'embeds',$3,$4,$5,$6,$7)",vec![Uuid::new_v4().into(),identity.scope_id.into(),object.into(),embed.collection.into(),embed.block.to_string().into(),json!({"block_id":embed.block}).into(),user.into()])).await?;
        tx.execute(Statement::from_sql_and_values(DbBackend::Postgres,"UPDATE business_events SET metadata=metadata||$2::jsonb,payload=payload||$3::jsonb WHERE id=$1",vec![event.into(),json!({"command_result":{"collection_id":embed.collection,"collection_document_id":embed.document,"block_id":embed.block},"semantic_summary":{"action":"create_collection_embed"}}).into(),json!({"collection_id":embed.collection,"block_id":embed.block,"relation_type":"embeds"}).into()])).await?;
        return Ok(());
    }
    let sync = typed.record.map_or(
        collections::ProjectionSync::Collection {
            collection_id: typed.collection,
        },
        |record| collections::ProjectionSync::Record {
            collection_id: typed.collection,
            record_id: record,
        },
    );
    let event_plan = collections::semantic_event_rewrite(tx, &typed.command, &typed.payload, sync, seq).await?;
    match sync {
        collections::ProjectionSync::Collection { collection_id } => {
            collections::sync_collection_projection(tx, collection_id, seq, candidate).await?;
        }
        collections::ProjectionSync::Record {
            collection_id,
            record_id,
        } => collections::sync_record_projection(tx, collection_id, record_id, seq, candidate).await?,
    }
    let mut payload = event_plan.payload;
    if typed.record_guard.is_some()
        && let Some(values) = payload.as_object_mut()
    {
        values.remove("collection_id");
    }
    payload
        .as_object_mut()
        .ok_or_else(ApiError::internal)?
        .insert("object_id".to_owned(), json!(object));
    tx.execute(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "UPDATE business_events SET event_type=$2,aggregate_type=$3,aggregate_id=$4,payload=$5 WHERE id=$1",
        vec![
            event.into(),
            event_plan.event_type.into(),
            event_plan.aggregate_type.into(),
            event_plan.aggregate_id.into(),
            payload.into(),
        ],
    ))
    .await?;
    tx.execute(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "UPDATE event_dispatch SET event_type=$2,document_id=NULL,accepted_seq=NULL WHERE event_id=$1",
        vec![event.into(), event_plan.event_type.into()],
    ))
    .await?;
    Ok(())
}
async fn collection_identity(state: &AppState, collection: Uuid) -> Result<scope::ObjectIdentity, ApiError> {
    let identity = scope::identity(&state.db, collection).await?;
    if identity.object_type != "collection" || identity.lifecycle_status != "active" {
        return Err(ApiError::invalid_update("target must be an active Collection"));
    }
    Ok(identity)
}
pub async fn describe(state: &AppState, collection: Uuid, user: Uuid, at_seq: Option<i64>) -> Result<Value, ApiError> {
    let proof = scope::authorize(&state.db, collection, user, PermissionLevel::View).await?;
    collection_identity(state, collection).await?;
    let projection = collections::collection_read_row(&state.db, collection).await?;
    if at_seq.is_some_and(|seq| seq != projection.schema_seq) {
        return Err(ApiError::invalid_update("Collection schema sequence is not current"));
    }
    let mut fields = collections::collection_fields(&state.db, collection, true).await?;
    fields.retain(|field| !collections::field_is_restricted(field));
    let views = collections::collection_views(&state.db, collection).await?;
    let candidates=state.db.query_all(Statement::from_sql_and_values(DbBackend::Postgres,"SELECT r.record_id FROM flow_record_projections r JOIN flow_objects o ON o.id=r.record_id WHERE r.collection_id=$1 AND o.lifecycle_status='active' ORDER BY r.record_id LIMIT 10001",vec![collection.into()])).await?;
    let mut count = 0;
    for row in candidates {
        let record: Uuid = row.try_get("", "record_id")?;
        if scope::authorize(&state.db, record, user, PermissionLevel::View)
            .await
            .is_ok()
        {
            count += 1;
        }
    }
    if !scope::proof_current(&state.db, &proof).await? {
        return Err(super::policy::authorization_read_unstable());
    }
    Ok(
        json!({"object":scope::object_view(&state.db,collection,user).await?,"fields":fields,"views":views,"record_count":count,"schema_seq":projection.schema_seq,"projection_seq":projection.schema_seq,"client_crdt_enabled":false,"field_secrecy_enabled":projection.field_secrecy_enabled}),
    )
}
pub async fn query(
    state: &AppState,
    collection: Uuid,
    user: Uuid,
    request: &RecordQueryPayload,
) -> Result<Value, ApiError> {
    let proof = scope::authorize(&state.db, collection, user, PermissionLevel::View).await?;
    collection_identity(state, collection).await?;
    let planned = collections::record_query_candidates(state, collection, request, false).await?;
    let requested = if request.field_ids.is_empty() {
        planned
            .all_fields
            .iter()
            .filter(|f| !collections::field_is_restricted(f))
            .map(|f| f.field_id)
            .collect::<BTreeSet<_>>()
    } else {
        request.field_ids.iter().copied().collect()
    };
    for field in &requested {
        collections::query_field(&planned.all_fields, *field)?;
    }
    let mut items = Vec::new();
    let mut consumed = planned.offset;
    let mut has_more = false;
    let mut projection_seq = planned.collection.schema_seq;
    for (offset, candidate) in planned.candidates.iter().enumerate() {
        if items.len() >= usize::try_from(request.limit).unwrap_or(100) {
            has_more = true;
            break;
        }
        consumed = planned
            .offset
            .saturating_add(u32::try_from(offset + 1).unwrap_or(u32::MAX));
        let record = match scope::object_view(&state.db, candidate.record_id, user).await {
            Ok(record) => record,
            Err(err) if err.kind() == crate::error::ApiErrorKind::NotFound => continue,
            Err(err) => return Err(err),
        };
        let properties = candidate.properties.as_object().ok_or_else(ApiError::internal)?;
        let values = properties
            .iter()
            .filter(|(key, _)| {
                Uuid::parse_str(key)
                    .ok()
                    .is_some_and(|field| requested.contains(&field))
            })
            .map(|(key, value)| (key.clone(), value.clone()))
            .collect::<Map<_, _>>();
        projection_seq = projection_seq.max(candidate.document_seq);
        items.push(json!({"record":record,"values_by_field_id":values}));
    }
    if planned.candidates.len() >= 10_000 {
        has_more = true;
    }
    let next = if has_more {
        Some(collections::encode_query_cursor(
            state.cfg.jwt_secret.expose(),
            &collections::RecordQueryCursor {
                fingerprint: planned.fingerprint,
                offset: consumed,
            },
        )?)
    } else {
        None
    };
    let fields = planned
        .all_fields
        .into_iter()
        .filter(|f| requested.contains(&f.field_id) && !collections::field_is_restricted(f))
        .collect::<Vec<_>>();
    if !scope::proof_current(&state.db, &proof).await? {
        return Err(super::policy::authorization_read_unstable());
    }
    Ok(
        json!({"fields":fields,"items":items,"next_cursor":next,"projection_seq":projection_seq,"schema_seq":planned.collection.schema_seq}),
    )
}
#[derive(Deserialize)]
struct RecordInput {
    #[serde(default)]
    record_id: Option<Uuid>,
    #[serde(default)]
    properties: Map<String, Value>,
    #[serde(default)]
    body: Option<String>,
}
#[derive(Deserialize)]
struct RecordPatch {
    record_id: Uuid,
    #[serde(default)]
    properties: Map<String, Value>,
    body: Option<String>,
}
pub async fn execute(
    state: &AppState,
    collection: Uuid,
    user: Uuid,
    kind: CollectionCommandType,
    payload: Value,
    key: &str,
    expected: Option<&str>,
    message: Option<&str>,
) -> Result<Value, ApiError> {
    execute_inner(state, collection, user, kind, payload, key, (expected, message), None).await
}
pub(crate) async fn execute_record_authorized(
    state: &AppState,
    user: Uuid,
    kind: CollectionCommandType,
    payload: Value,
    key: &str,
    expected: Option<&str>,
    message: Option<&str>,
    authority: super::scope_record_context::RecordAuthority,
) -> Result<Value, ApiError> {
    if authority.proof.object_id != authority.record()
        || authority.proof.principal_id != user
        || !matches!(
            kind,
            CollectionCommandType::RecordPatch | CollectionCommandType::RecordArchive
        )
    {
        return Err(ApiError::policy_rejected("Record authority target mismatch"));
    }
    let collection = authority.collection();
    execute_inner(
        state,
        collection,
        user,
        kind,
        payload,
        key,
        (expected, message),
        Some(authority),
    )
    .await
}
async fn execute_inner(
    state: &AppState,
    collection: Uuid,
    user: Uuid,
    kind: CollectionCommandType,
    payload: Value,
    key: &str,
    options: (Option<&str>, Option<&str>),
    record_authority: Option<super::scope_record_context::RecordAuthority>,
) -> Result<Value, ApiError> {
    let (expected, message) = options;
    if kind == CollectionCommandType::CreateCollectionEmbed {
        return execute_embed(state, collection, user, payload, key, expected, message).await;
    }
    let identity = if record_authority.is_some() && kind == CollectionCommandType::RecordArchive {
        let identity = scope::identity(&state.db, collection).await?;
        if identity.object_type != "collection" {
            return Err(ApiError::not_found("Record container is unavailable"));
        }
        identity
    } else {
        collection_identity(state, collection).await?
    };
    let schema_command = matches!(
        kind,
        CollectionCommandType::FieldCreate
            | CollectionCommandType::FieldUpdate
            | CollectionCommandType::FieldArchive
            | CollectionCommandType::FieldReorder
            | CollectionCommandType::ViewCreate
            | CollectionCommandType::ViewUpdate
            | CollectionCommandType::ViewReorder
    );
    let minimum = if schema_command {
        PermissionLevel::FullAccess
    } else {
        PermissionLevel::Edit
    };
    let proof = if let Some(authority) = record_authority.as_ref() {
        authority.proof.clone()
    } else {
        scope::authorize(&state.db, collection, user, minimum).await?
    };
    let body=record_authority.as_ref().map_or_else(||json!({"operation":"typed_command","collection_id":collection,"command":kind.wire_name(),"payload":payload,"expected_frontier":expected,"message":message}),|authority|authority.canonical_body.clone());
    if let Some(result) = super::scope_commands::replay(&state.db, identity.scope_id, user, key, &body).await? {
        return Ok(result);
    }
    if kind == CollectionCommandType::RecordCreate {
        return create_record(state, collection, user, payload, key, body, &proof).await;
    }
    if kind == CollectionCommandType::RecordArchive {
        let record = payload
            .get("record_id")
            .and_then(Value::as_str)
            .and_then(|s| Uuid::parse_str(s).ok())
            .ok_or_else(|| ApiError::invalid_update("record_id required"))?;
        let record_identity = scope::identity(&state.db, record).await?;
        if record_identity.parent_id != Some(collection) || record_identity.object_type != "record" {
            return Err(ApiError::not_found("record not found"));
        }
        let record_proof = scope::authorize(&state.db, record, user, PermissionLevel::Edit).await?;
        let tx = state.db.begin().await?;
        if identity.workspace_id.is_none() {
            scope::require_personal_write(&tx).await?;
        }
        scope::lock_change(&tx, &record_proof).await?;
        if let Some(result) = super::scope_commands::lock_and_replay(&tx, identity.scope_id, user, key, &body).await? {
            tx.commit().await?;
            return Ok(result);
        }
        if let Some(authority) = record_authority.as_ref() {
            let guard = authority.schema_guard();
            tx.execute(Statement::from_sql_and_values(
                DbBackend::Postgres,
                "SELECT collection_id FROM flow_collection_projections WHERE collection_id=$1 FOR SHARE",
                vec![collection.into()],
            ))
            .await?;
            super::scope_record_context::validate_guard(&tx, collection, record, user, &guard, &payload).await?;
            let head = tx
                .query_one(Statement::from_sql_and_values(
                    DbBackend::Postgres,
                    "SELECT head_seq,head_frontier FROM collab_documents WHERE object_id=$1 FOR UPDATE",
                    vec![record.into()],
                ))
                .await?
                .ok_or_else(|| ApiError::not_found("Record document not found"))?;
            let frontier: Vec<u8> = head.try_get("", "head_frontier")?;
            let expected = expected
                .and_then(|value| BASE64.decode(value).ok())
                .ok_or_else(|| ApiError::invalid_update("expected_frontier is required"))?;
            if frontier != expected {
                return Err(ApiError::stale_frontier("Record changed before archive", None, None));
            }
        }
        super::repository::set_objects_lifecycle(&tx, &[record], "archived", Some(chrono::Utc::now()), Some(user))
            .await?;
        scope::advance_revision(&tx, identity.scope_id).await?;
        let event = super::scope_event::insert_for_object(
            &tx,
            &record_identity,
            user,
            "flow.record.archived",
            "flow_record",
            record,
            key,
            if record_authority.is_some() {
                json!({"object_id":record,"record_id":record,"status":"archived"})
            } else {
                json!({"object_id":record,"collection_id":collection,"record_id":record,"status":"archived"})
            },
            json!({"idempotency_body":body}),
        )
        .await?;
        let result = scope::accepted(scope::object_view(&tx, record, user).await?, event);
        super::scope_commands::record(&tx, identity.scope_id, user, key, body, record, event, result.clone()).await?;
        tx.commit().await?;
        return Ok(result);
    }
    let (object, document, operations, typed) = if schema_command {
        let view = scope::object_view(&state.db, collection, user).await?;
        let document = Uuid::parse_str(
            view.get("document_id")
                .and_then(Value::as_str)
                .ok_or_else(ApiError::internal)?,
        )
        .map_err(|_| ApiError::internal())?;
        let ops = collections::collection_operations(state, document, kind, &payload).await?;
        (
            collection,
            document,
            ops,
            TypedCommit {
                collection,
                record: None,
                command: kind.wire_name().to_string(),
                payload: payload.clone(),
                embed: None,
                record_guard: None,
            },
        )
    } else if kind == CollectionCommandType::RecordPatch {
        let patch: RecordPatch =
            serde_json::from_value(payload.clone()).map_err(|_| ApiError::invalid_update("invalid record patch"))?;
        if patch.properties.is_empty() && patch.body.is_none() {
            return Err(ApiError::invalid_update("record_patch changes nothing"));
        }
        collections::validate_properties_against_fields(&state.db, collection, &patch.properties).await?;
        let record = scope::identity(&state.db, patch.record_id).await?;
        if record.parent_id != Some(collection)
            || record.scope_id != identity.scope_id
            || record.object_type != "record"
        {
            return Err(ApiError::not_found("record not found"));
        }
        scope::authorize(&state.db, record.id, user, PermissionLevel::Edit).await?;
        let view = scope::object_view(&state.db, record.id, user).await?;
        let document = Uuid::parse_str(
            view.get("document_id")
                .and_then(Value::as_str)
                .ok_or_else(ApiError::internal)?,
        )
        .map_err(|_| ApiError::internal())?;
        let boot = bootstrap::load(&state.db, document).await?;
        let mut engine = LoroCollabEngine::load(&boot.snapshot).map_err(|e| super::command::map_collab_error(&e))?;
        for update in boot.tail_updates {
            engine
                .import_update(&update.bytes)
                .map_err(|e| super::command::map_collab_error(&e))?;
        }
        let semantic = engine
            .semantic_snapshot()
            .map_err(|e| super::command::map_collab_error(&e))?;
        let mut operations = Vec::new();
        for (index, (field, value)) in patch.properties.into_iter().enumerate() {
            let id = std::sync::Arc::<str>::from(field);
            if !semantic.nodes.contains_key(&id) {
                operations.push(Operation::CreateNode {
                    id: id.clone(),
                    parent: None,
                    index: u32::try_from(index).unwrap_or(u32::MAX),
                    kind: NodeKind::RecordProperty,
                });
            }
            operations.push(Operation::SetProperty {
                id,
                key: "value".into(),
                value: serde_json::to_string(&value).map_err(|_| ApiError::internal())?,
            });
        }
        if let Some(body) = patch.body {
            let id = std::sync::Arc::<str>::from("record:body");
            if !semantic.nodes.contains_key(&id) {
                operations.push(Operation::CreateNode {
                    id: id.clone(),
                    parent: None,
                    index: u32::MAX,
                    kind: NodeKind::Block,
                });
            }
            operations.push(Operation::SetProperty {
                id,
                key: "body".into(),
                value: body,
            });
        }
        (
            record.id,
            document,
            operations,
            TypedCommit {
                collection,
                record: Some(record.id),
                command: kind.wire_name().to_string(),
                payload: payload.clone(),
                embed: None,
                record_guard: record_authority
                    .as_ref()
                    .map(super::scope_record_context::RecordAuthority::schema_guard),
            },
        )
    } else {
        return Err(ApiError::unsupported_protocol(
            "typed embed creation and record queries use their dedicated application paths",
        ));
    };
    let boot = bootstrap::load(&state.db, document).await?;
    let bytes = collections::export_operations(&boot, &operations)?;
    let update = super::collab::write::replay_stable_update_id(document, key);
    let expected = expected
        .map(|s| {
            BASE64
                .decode(s)
                .map_err(|_| ApiError::invalid_update("invalid expected_frontier"))
        })
        .transpose()?;
    match super::scope_write::accept_typed(
        &state.db,
        object,
        bytes,
        super::scope_write::UpdateIntent {
            user,
            update,
            client: &format!("rest:{user}"),
            key: Some(key),
            expected: expected.as_deref(),
            command_body: Some(body),
        },
        typed,
    )
    .await?
    {
        AcceptOutcome::Accepted(receipt) => Ok(scope::accepted(
            scope::object_view(&state.db, object, user).await?,
            receipt.event_id,
        )),
        AcceptOutcome::Rejected(rejected) => Err(super::scope_write::map_rejection(&rejected)),
    }
}
async fn create_record(
    state: &AppState,
    collection: Uuid,
    user: Uuid,
    payload: Value,
    key: &str,
    body: Value,
    proof: &scope::AuthorizationProof,
) -> Result<Value, ApiError> {
    let input: RecordInput =
        serde_json::from_value(payload).map_err(|_| ApiError::invalid_update("invalid record input"))?;
    collections::validate_properties_against_fields(&state.db, collection, &input.properties).await?;
    let engine = collections::build_record_engine(&input.properties, input.body.as_deref())?;
    let semantic = engine
        .semantic_snapshot()
        .map_err(|e| super::command::map_collab_error(&e))?;
    collab_core::limits::check_snapshot(&semantic, &super::collab::limits::document_limits())
        .map_err(|e| super::command::map_collab_error(&e.into()))?;
    let snapshot = engine
        .export_snapshot()
        .map_err(|e| super::command::map_collab_error(&e))?;
    let frontier = engine.frontier().as_bytes().to_vec();
    let record = input.record_id.unwrap_or_else(Uuid::new_v4);
    let document = Uuid::new_v4();
    let tx = state.db.begin().await?;
    scope::require_personal_write(&tx).await?;
    scope::fence(&tx, proof).await?;
    if let Some(result) = super::scope_commands::lock_and_replay(&tx, proof.scope_id, user, key, &body).await? {
        tx.commit().await?;
        return Ok(result);
    }
    tx.execute(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "SELECT collection_id FROM flow_collection_projections WHERE collection_id=$1 FOR SHARE",
        vec![collection.into()],
    ))
    .await?;
    super::collab::authz::ensure_scope_parent_can_adopt_child(&tx, proof.scope_id, collection).await?;
    collections::validate_properties_against_fields(&tx, collection, &input.properties).await?;
    validate_relation_access(&tx, collection, &input.properties, user).await?;
    tx.execute(Statement::from_sql_and_values(DbBackend::Postgres,"INSERT INTO flow_objects(id,scope_id,workspace_id,object_type,parent_id,created_by) VALUES($1,$2,NULL,'record',$3,$4)",vec![record.into(),proof.scope_id.into(),collection.into(),user.into()])).await?;
    super::repository::insert_collab_document(
        &tx,
        &super::repository::NewCollabDocument {
            id: document,
            object_id: record,
            format_version: "loro-1".into(),
            snapshot,
            frontier: frontier.clone(),
        },
    )
    .await?;
    super::repository::insert_projection(
        &tx,
        &super::repository::NewProjection {
            object_id: record,
            document_seq: 0,
            document_frontier: frontier,
            title: String::new(),
            state: super::projection::state_json(&semantic).map_err(|_| ApiError::internal())?,
            plain_text: String::new(),
        },
    )
    .await?;
    tx.execute(Statement::from_sql_and_values(DbBackend::Postgres,"INSERT INTO flow_record_projections(record_id,collection_id,document_id,properties,has_body,document_seq) VALUES($1,$2,$3,'{}'::jsonb,false,0)",vec![record.into(),collection.into(),document.into()])).await?;
    collections::sync_record_projection(&tx, collection, record, 0, &engine).await?;
    let event = super::scope_event::insert(
        &tx,
        proof.scope_id,
        user,
        "flow.record.created",
        "flow_record",
        record,
        key,
        json!({"object_id":record,"collection_id":collection,"record_id":record,"seq":0}),
        json!({"idempotency_body":body}),
        None,
    )
    .await?;
    let result = scope::accepted(scope::object_view(&tx, record, user).await?, event);
    super::scope_commands::record(&tx, proof.scope_id, user, key, body, record, event, result.clone()).await?;
    tx.commit().await?;
    Ok(result)
}

impl TypedCommit {
    pub(crate) fn decorate(&self, result: &mut Value) {
        if self.record_guard.is_some()
            && let Some(object) = result.get_mut("object")
        {
            super::scope_record_context::decorate_record_view(object);
        }
        if let Some(output) = result.as_object_mut() {
            if self.record_guard.is_some() {
                output.insert("affected_object_ids".into(), json!([self.record]));
            }
            if let Some(embed) = self.embed.as_ref() {
                output.insert("command_result".into(),json!({"collection_id":embed.collection,"collection_document_id":embed.document,"block_id":embed.block}));
                output.insert("affected_object_ids".into(), json!([self.collection, embed.collection]));
            }
        }
    }
}
async fn execute_embed(
    state: &AppState,
    page: Uuid,
    user: Uuid,
    payload: Value,
    key: &str,
    expected: Option<&str>,
    message: Option<&str>,
) -> Result<Value, ApiError> {
    let identity = scope::identity(&state.db, page).await?;
    if identity.object_type != "page" || identity.lifecycle_status != "active" {
        return Err(ApiError::invalid_update(
            "create_collection_embed requires an active page",
        ));
    }
    scope::authorize(&state.db, page, user, PermissionLevel::Edit).await?;
    let body = json!({"operation":"typed_command","collection_id":page,"command":"create_collection_embed","payload":payload,"expected_frontier":expected,"message":message});
    if let Some(result) = super::scope_commands::replay(&state.db, identity.scope_id, user, key, &body).await? {
        return Ok(result);
    }
    let input: collections::CreateCollectionEmbedPayload =
        serde_json::from_value(payload.clone()).map_err(|_| ApiError::invalid_update("invalid collection embed"))?;
    let title = collections::validate_label(&input.title, "collection title")?;
    let schema = json!({"initial_fields":input.initial_fields,"initial_view":input.initial_view});
    let mut validation = LoroCollabEngine::new_empty(1);
    collections::apply_initial_collection_schema(&mut validation, &schema)?;
    let view = scope::object_view(&state.db, page, user).await?;
    let document = Uuid::parse_str(
        view.get("document_id")
            .and_then(Value::as_str)
            .ok_or_else(ApiError::internal)?,
    )
    .map_err(|_| ApiError::internal())?;
    let boot = bootstrap::load(&state.db, document).await?;
    let (parent, index) = match input.after_block_id.as_deref() {
        Some(after) => collections::embed_position_after(&boot, after)?,
        None => (None, u32::MAX),
    };
    let collection = Uuid::new_v4();
    let collection_document = Uuid::new_v4();
    let block = Uuid::new_v4();
    let id = std::sync::Arc::<str>::from(block.to_string());
    let operations = vec![
        Operation::CreateNode {
            id: id.clone(),
            parent,
            index,
            kind: NodeKind::Block,
        },
        Operation::SetProperty {
            id: id.clone(),
            key: "embed_type".into(),
            value: "collection".into(),
        },
        Operation::SetProperty {
            id,
            key: "embed_object_id".into(),
            value: collection.to_string(),
        },
    ];
    let bytes = collections::export_operations(&boot, &operations)?;
    let update = super::collab::write::replay_stable_update_id(document, key);
    let expected = expected
        .map(|s| {
            BASE64
                .decode(s)
                .map_err(|_| ApiError::invalid_update("invalid expected frontier"))
        })
        .transpose()?;
    let typed = TypedCommit {
        collection: page,
        record: None,
        command: "create_collection_embed".into(),
        payload,
        record_guard: None,
        embed: Some(EmbedCommit {
            collection,
            document: collection_document,
            block,
            title,
            schema,
            display: Value::Object(input.display_settings),
        }),
    };
    match super::scope_write::accept_typed(
        &state.db,
        page,
        bytes,
        super::scope_write::UpdateIntent {
            user,
            update,
            client: &format!("rest:{user}"),
            key: Some(key),
            expected: expected.as_deref(),
            command_body: Some(body),
        },
        typed,
    )
    .await?
    {
        AcceptOutcome::Accepted(receipt) => {
            let row = state
                .db
                .query_one(Statement::from_sql_and_values(
                    DbBackend::Postgres,
                    "SELECT result FROM flow_scope_command_receipts WHERE scope_id=$1 AND idempotency_key=$2",
                    vec![identity.scope_id.into(), key.into()],
                ))
                .await?
                .ok_or_else(ApiError::internal)?;
            let mut result: Value = row.try_get("", "result")?;
            result
                .as_object_mut()
                .ok_or_else(ApiError::internal)?
                .insert("object".into(), scope::object_view(&state.db, page, user).await?);
            result
                .as_object_mut()
                .ok_or_else(ApiError::internal)?
                .insert("event_id".into(), json!(receipt.event_id));
            Ok(result)
        }
        AcceptOutcome::Rejected(rejected) => Err(super::scope_write::map_rejection(&rejected)),
    }
}
