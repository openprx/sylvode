//! Scope-native writes reuse the same CRDT preparation and isolated Linux apply boundary as v1.
//! Only ownership fencing and transactional event source selection differ.
#[derive(FromQueryResult)]
struct AcceptedDocumentHead {
    head_seq: i64,
}

use super::{
    collab::{
        authz::PermissionLevel,
        runtime,
        write::{self, AcceptOutcome, Accepted, HydrateOutcome},
    },
    scope,
};
use crate::error::ApiError;
use crate::flow::json_value::FlowJson;
use base64::{Engine, engine::general_purpose::STANDARD as BASE64};
use sea_orm::{ConnectionTrait, DatabaseConnection, DbBackend, FromQueryResult, Statement, TransactionTrait};
use serde_json::{Value, json};
use uuid::Uuid;

pub struct UpdateIntent<'a> {
    pub user: Uuid,
    pub update: Uuid,
    pub client: &'a str,
    pub key: Option<&'a str>,
    pub expected: Option<&'a [u8]>,
    pub command_body: Option<Value>,
}
pub async fn accept(
    db: &DatabaseConnection,
    object: Uuid,
    bytes: Vec<u8>,
    intent: UpdateIntent<'_>,
) -> Result<AcceptOutcome, ApiError> {
    accept_inner(db, object, bytes, intent, None, false, 5, None).await
}
pub async fn accept_model(
    db: &DatabaseConnection,
    object: Uuid,
    bytes: Vec<u8>,
    intent: UpdateIntent<'_>,
    model: u32,
) -> Result<AcceptOutcome, ApiError> {
    accept_inner(db, object, bytes, intent, None, false, model, None).await
}
pub(crate) async fn accept_typed(
    db: &DatabaseConnection,
    object: Uuid,
    bytes: Vec<u8>,
    intent: UpdateIntent<'_>,
    typed: super::scope_collections::TypedCommit,
) -> Result<AcceptOutcome, ApiError> {
    if intent.key.is_none() || intent.command_body.is_none() {
        return Err(ApiError::invalid_update("typed mutations require a request identity"));
    }
    accept_inner(db, object, bytes, intent, Some(typed), false, 5, None).await
}
async fn accept_inner(
    db: &DatabaseConnection,
    object: Uuid,
    bytes: Vec<u8>,
    intent: UpdateIntent<'_>,
    typed: Option<super::scope_collections::TypedCommit>,
    editor_init: bool,
    editor_model: u32,
    shared: Option<super::block_shares::SharedWritePermit>,
) -> Result<AcceptOutcome, ApiError> {
    let UpdateIntent {
        user,
        update,
        client,
        key,
        expected,
        command_body,
    } = intent;
    let identity = scope::identity(db, object).await?;
    let record_typed = typed
        .as_ref()
        .is_some_and(super::scope_collections::TypedCommit::is_record);
    if record_typed
        && typed
            .as_ref()
            .and_then(super::scope_collections::TypedCommit::record_id)
            != Some(object)
    {
        return Err(ApiError::policy_rejected("typed Record target identity changed"));
    }

    if identity.workspace_id.is_some() && !editor_init && !record_typed && shared.is_none() {
        return Err(ApiError::invalid_update(
            "workspace updates must use the workspace application service",
        ));
    }
    if editor_init && identity.object_type != "page" {
        return Err(ApiError::policy_rejected("only a Page can initialize editor prose"));
    }
    if matches!(identity.object_type.as_str(), "collection" | "record") && typed.is_none() {
        return Err(ApiError::policy_rejected(
            "collection and record documents require server-only typed commands",
        ));
    }
    if identity.lifecycle_status != "active" || scope::get(db, identity.scope_id).await?.lifecycle_status != "active" {
        return Err(ApiError::policy_rejected("archived content cannot accept updates"));
    }
    let document = Document::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "SELECT id FROM collab_documents WHERE object_id=$1",
        vec![object.into()],
    ))
    .one(db)
    .await?
    .ok_or_else(|| ApiError::not_found("document not found"))?
    .id;
    if let Some(workspace) = identity.workspace_id {
        runtime::runtime().ensure_workspace_accepting(workspace)?;
    }
    let _coordinator = runtime::runtime()
        .coordinator
        .acquire(document)
        .await
        .map_err(|_| ApiError::policy_rejected("document is busy; retry the same update identity"))?;
    for _ in 0..3 {
        let minimum = typed
            .as_ref()
            .map_or(PermissionLevel::Edit, super::scope_collections::TypedCommit::minimum);
        let proof = if let Some(permit) = shared.as_ref() {
            permit.authorize(db, object, user).await?
        } else {
            scope::authorize(db, object, user, minimum).await?
        };
        if let Some(prior) = prior(db, document, update).await? {
            if prior.content_hash != super::collab::bootstrap::content_hash(&bytes) {
                return Err(ApiError::idempotency_key_body_drift("update identity changed bytes"));
            }
            return Ok(AcceptOutcome::Accepted(prior.accepted(update)));
        }
        let prepared =
            match write::hydrate_and_apply(db, &runtime::runtime().cache, document, update, &bytes, expected).await? {
                HydrateOutcome::Prepared(p) => p,
                HydrateOutcome::Rejected(result) => return Ok(result),
            };
        if let Some(permit) = shared.as_ref() {
            permit.validate_candidate(&prepared.candidate)?;
        }
        let minimum = prepared
            .candidate
            .minimum_editor_model()
            .map_err(|e| super::command::map_collab_error(&e))?;
        if editor_model < minimum {
            return Err(super::scope_editor::model_error(editor_model, minimum));
        }
        let should_advance = match super::collab::snapshot::evaluate(
            &prepared.observed.tail_stats(),
            runtime::runtime().snapshot.last_rebuild_wall_ms(document),
        ) {
            super::collab::snapshot::Trigger::Hard => {
                match super::collab::snapshot::advance(db, document, &runtime::runtime().snapshot).await? {
                    super::collab::snapshot::AdvanceOutcome::Advanced
                    | super::collab::snapshot::AdvanceOutcome::NothingToAdvance => continue,
                    super::collab::snapshot::AdvanceOutcome::Contended => {
                        return Err(ApiError::policy_rejected(
                            "snapshot checkpoint contention; retry same update",
                        ));
                    }
                }
            }
            super::collab::snapshot::Trigger::Soft => true,
            super::collab::snapshot::Trigger::None => false,
        };
        let started = std::time::Instant::now();
        let tx = db.begin().await?;
        if editor_init {
            super::scope_editor::require_write(&tx, &identity).await?;
        } else if let Some(workspace) = identity.workspace_id {
            super::policy::require_flow_enabled_on(&tx, workspace).await?;
        } else {
            scope::require_personal_write(&tx).await?;
        }
        if let Some(permit) = shared.as_ref() {
            permit.fence(&tx, &proof).await?;
        } else {
            scope::fence(&tx, &proof).await?;
        }
        if scope::identity(&tx, object).await?.lifecycle_status != "active"
            || scope::get(&tx, identity.scope_id).await?.lifecycle_status != "active"
        {
            return Err(ApiError::policy_rejected("archived content cannot accept updates"));
        }
        if let (Some(key), Some(body)) = (key, command_body.as_ref())
            && shared.is_none()
            && let Some(result) =
                super::scope_commands::lock_and_replay(&tx, identity.scope_id, user, key, body).await?
        {
            let event = Uuid::parse_str(result.field("event_id").as_str().ok_or_else(ApiError::internal)?)
                .map_err(|_| ApiError::internal())?;
            tx.rollback().await?;
            return Ok(AcceptOutcome::Accepted(Accepted {
                update_id: update,
                head_seq: result.field("accepted_seq").as_i64().ok_or_else(ApiError::internal)?,
                head_frontier: BASE64
                    .decode(result.field("head_frontier").as_str().ok_or_else(ApiError::internal)?)
                    .map_err(|_| ApiError::internal())?,
                projection_seq: result.field("projection_seq").as_i64().ok_or_else(ApiError::internal)?,
                event_id: event,
                before_frontier: Vec::new(),
                should_advance_snapshot: false,
            }));
        }
        if let Some(typed) = typed.as_ref() {
            super::scope_collections::lock_projection(&tx, typed, user).await?;
        }
        if shared.is_some() {
            // Lifecycle commands lock the object before their durable share-suspension trigger.
            // Keep the same object→document→share order, including workspace archives.
            let row = tx
                .query_one(Statement::from_sql_and_values(
                    DbBackend::Postgres,
                    "SELECT lifecycle_status FROM flow_objects WHERE id=$1 FOR UPDATE",
                    vec![object.into()],
                ))
                .await?
                .ok_or_else(ApiError::internal)?;
            if row.try_get::<String>("", "lifecycle_status")? != "active" {
                return Err(ApiError::policy_rejected("shared source unavailable"));
            }
        }
        write::set_locked_phase_statement_budgets(&tx, 1).await?;

        let head = AcceptedDocumentHead::find_by_statement(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT head_seq FROM collab_documents WHERE id=$1 FOR UPDATE",
            vec![document.into()],
        ))
        .one(&tx)
        .await?
        .ok_or_else(ApiError::internal)?;
        if head.head_seq != prepared.observed.head_seq {
            tx.rollback().await?;
            continue;
        }
        if let Some(prior) = prior(&tx, document, update).await? {
            tx.rollback().await?;
            if prior.content_hash != super::collab::bootstrap::content_hash(&bytes) {
                return Err(ApiError::idempotency_key_body_drift("update identity changed bytes"));
            }
            return Ok(AcceptOutcome::Accepted(prior.accepted(update)));
        }
        if let Some(permit) = shared.as_ref() {
            permit.lock_after_head(&tx, head.head_seq).await?;
        }
        if editor_init {
            super::block_registry::carry_tree_block_identities(&tx, object, &prepared.logical_blocks).await?;
        }
        let seq = head.head_seq + 1;
        let event_key = format!("content:{document}:{update}");
        let event = if editor_init || identity.workspace_id.is_some() {
            super::scope_event::insert_content_accepted(
                &tx,
                &identity,
                user,
                document,
                seq,
                key.unwrap_or(&event_key),
                if editor_init {
                    "editor_initialized"
                } else {
                    "typed_record_update"
                },
            )
            .await?
        } else {
            super::scope_event::insert(&tx,identity.scope_id,user,"flow.content.accepted","flow_document",document,&event_key,json!({"object_id":object,"document_id":document,"accepted_seq":seq,"projection_seq":seq,"changed_block_ids":[]}),json!({"semantic_summary":{"action":"content_update"}}),Some((document,seq))).await?
        };
        tx.execute(Statement::from_sql_and_values(DbBackend::Postgres,r"INSERT INTO collab_updates(document_id,seq,update_id,content_hash,idempotency_key,before_frontier,after_frontier,bytes,actor_id,origin_surface,origin_client_id,projection_seq,event_id)
   VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9,'web',$10,$2,$11)",vec![document.into(),seq.into(),update.into(),prepared.content_hash_hex.clone().into(),key.map(str::to_string).into(),prepared.observed.head_frontier.clone().into(),prepared.after_frontier.clone().into(),bytes.clone().into(),user.into(),client.into(),event.into()])).await?;
        // The same bounded snapshot+tail model as workspace Flow. Accepted history remains available
        // until the shared compactor's acknowledged retention boundary, never a private checkpoint.
        tx.execute(Statement::from_sql_and_values(DbBackend::Postgres,"UPDATE collab_documents SET head_seq=$2,head_frontier=$3,byte_count=byte_count+$4,update_count=update_count+1,updated_at=now() WHERE id=$1",vec![document.into(),seq.into(),prepared.after_frontier.clone().into(),i64::try_from(bytes.len()).unwrap_or(i64::MAX).into()])).await?;
        tx.execute(Statement::from_sql_and_values(DbBackend::Postgres,"UPDATE flow_object_projections SET document_seq=$2,document_frontier=$3,title=$4,state=$5,plain_text=$6,updated_at=now() WHERE object_id=$1",vec![object.into(),seq.into(),prepared.after_frontier.clone().into(),prepared.title.clone().into(),prepared.state_json.clone().into(),prepared.plain_text.clone().into()])).await?;
        super::body_assets::reconcile_accepted(&tx, object, seq, &prepared.asset_references).await?;
        super::child_pages::reconcile_accepted(&tx, object, Some(user), seq, &prepared.child_references).await?;
        super::block_registry::reconcile_accepted(
            &tx,
            object,
            seq,
            &prepared.logical_blocks,
            super::block_registry::RegistrationAllowance::within(started, write::LOCKED_PHASE_STAGING_BUDGET),
        )
        .await?;
        if let Some(permit) = shared.as_ref() {
            permit.record(&tx).await?;
        }
        if let Some(typed) = typed.as_ref() {
            super::scope_collections::sync(&tx, typed, object, user, seq, &prepared.candidate, event).await?;
        }
        tx.execute(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "UPDATE flow_objects SET updated_at=now(),updated_by=$2 WHERE id=$1",
            vec![object.into(), user.into()],
        ))
        .await?;
        if let (Some(key), Some(body)) = (key, command_body.as_ref())
            && shared.is_none()
        {
            let mut result = scope::accepted(scope::object_view(&tx, object, user).await?, event);
            if let Some(typed) = typed.as_ref() {
                typed.decorate(&mut result);
            }
            super::scope_commands::record(&tx, identity.scope_id, user, key, body.clone(), object, event, result)
                .await?;
        }
        if let Err(err) = tx.commit().await {
            tracing::warn!(%err,%document,%update,"scope write commit outcome unknown; retry same update identity");
            return Ok(AcceptOutcome::Rejected(write::Rejected {
                update_id: Some(update),
                code: super::collab::frame::RejectedCode::ServerDraining,
                recoverable: true,
                write_state: super::collab::frame::WriteState::Unknown,
                details: Some(json!({"reason":"contention","retry_after_ms":1000})),
                current_seq: None,
                current_frontier: None,
            }));
        }
        if let Some(workspace) = identity.workspace_id {
            let accepted = Accepted {
                update_id: update,
                head_seq: seq,
                head_frontier: prepared.after_frontier.clone(),
                projection_seq: seq,
                event_id: event,
                before_frontier: prepared.observed.head_frontier.clone(),
                should_advance_snapshot: false,
            };
            let request = write::UpdateRequest {
                rebase_policy: crate::flow::collab::write::RebasePolicy::None,
                document_id: document,
                update_id: update,
                bytes: bytes.clone(),
                idempotency_key: key.map(str::to_owned),
                event_idempotency_key: key.map(str::to_owned),
                origin_client_id: Some(client.to_string()),
                message: None,
                actor_id: user,
                actor_is_bot: false,
                workspace_id: workspace,
                checked_epoch: proof.revision,
                expected_frontier: expected.map(<[u8]>::to_vec),
                origin: super::event_origin::CommandOrigin::first_request_from(super::event_origin::EventSurface::Rest),
            };
            super::collab::fanout::note_local_delivery(document, seq);
            if let Err(error) = super::collab::fanout::publish_document_update(db, workspace, document, seq).await {
                tracing::warn!(%error,%document,"committed editor initialization fanout notice unavailable");
            }
            write::broadcast_committed_update(&runtime::runtime().registry, None, &request, &accepted);
        }
        if let Ok(engine) = prepared.candidate.fork() {
            runtime::runtime().cache.put(
                document,
                engine,
                prepared.observed.format_version.clone(),
                seq,
                prepared.after_frontier.clone(),
                prepared.decoded_bytes_hint,
            );
        }
        if should_advance {
            super::collab::snapshot::spawn_background(&runtime::runtime().snapshot, db.clone(), document);
        }
        return Ok(AcceptOutcome::Accepted(Accepted {
            update_id: update,
            head_seq: seq,
            head_frontier: prepared.after_frontier.clone(),
            projection_seq: seq,
            event_id: event,
            before_frontier: prepared.observed.head_frontier.clone(),
            should_advance_snapshot: false,
        }));
    }
    Err(ApiError::policy_rejected(
        "document changed during preparation; retry same update identity",
    ))
}
#[derive(FromQueryResult)]
struct Document {
    id: Uuid,
}
#[derive(FromQueryResult)]
struct Prior {
    seq: i64,
    after_frontier: Vec<u8>,
    before_frontier: Vec<u8>,
    event_id: Uuid,
    content_hash: String,
}
impl Prior {
    fn accepted(self, update: Uuid) -> Accepted {
        Accepted {
            update_id: update,
            head_seq: self.seq,
            head_frontier: self.after_frontier,
            projection_seq: self.seq,
            event_id: self.event_id,
            before_frontier: self.before_frontier,
            should_advance_snapshot: false,
        }
    }
}
async fn prior<C: ConnectionTrait>(db: &C, document: Uuid, update: Uuid) -> Result<Option<Prior>, ApiError> {
    Ok(Prior::find_by_statement(Statement::from_sql_and_values(DbBackend::Postgres,"SELECT seq,after_frontier,before_frontier,event_id,content_hash FROM collab_updates WHERE document_id=$1 AND update_id=$2",vec![document.into(),update.into()])).one(db).await?)
}
pub fn limits() -> Value {
    let mut value = serde_json::to_value(super::collab::limits::effective_limits()).unwrap_or_else(|_| json!({}));
    if let Some(map) = value.as_object_mut() {
        map.insert("version".to_string(), json!("2"));
        if let Some(limit) = map.remove("connections_per_workspace_max") {
            map.insert("connections_per_scope_max".to_string(), limit);
        }
    }
    value
}
pub async fn bootstrap_model(db: &DatabaseConnection, object: Uuid, user: Uuid, model: u32) -> Result<Value, ApiError> {
    let value = bootstrap(db, object, user).await?;
    super::scope_editor::require_bootstrap_model(&value, model)?;
    Ok(value)
}
pub async fn bootstrap(db: &DatabaseConnection, object: Uuid, user: Uuid) -> Result<Value, ApiError> {
    let proof = scope::authorize(db, object, user, PermissionLevel::View).await?;
    let identity = scope::identity(db, object).await?;
    if matches!(identity.object_type.as_str(), "collection" | "record") {
        return Err(ApiError::policy_rejected(
            "collection and record documents are server-only; use typed collection endpoints",
        ));
    }
    let document = Document::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "SELECT id FROM collab_documents WHERE object_id=$1",
        vec![object.into()],
    ))
    .one(db)
    .await?
    .ok_or_else(|| ApiError::not_found("document not found"))?
    .id;
    super::scope_editor::require_admitted(db, object).await?;
    let boot = super::collab::bootstrap::load(db, document).await?;
    let tail:Vec<Value>=boot.tail_updates.iter().map(|u|json!({"seq":u.seq,"update_id":u.update_id,"bytes":BASE64.encode(&u.bytes),"before_frontier":BASE64.encode(&u.before_frontier),"after_frontier":BASE64.encode(&u.after_frontier)})).collect();
    if !scope::proof_current(db, &proof).await? {
        return Err(super::policy::authorization_read_unstable());
    }
    Ok(
        json!({"scope_id":proof.scope_id,"object_id":object,"document_id":document,"engine":boot.engine,"format_version":boot.format_version,"snapshot_seq":boot.snapshot_seq,"head_seq":boot.head_seq,"snapshot_base64":BASE64.encode(&boot.snapshot),"tail_updates":tail,"head_frontier":BASE64.encode(&boot.head_frontier),"limits":limits(),"websocket_path":"/api/v2/flow/collab/ws"}),
    )
}

pub async fn command_replay<C: ConnectionTrait>(
    db: &C,
    scope_id: Uuid,
    actor: Uuid,
    key: &str,
    body: &Value,
) -> Result<Option<Value>, ApiError> {
    super::scope_commands::replay(db, scope_id, actor, key, body).await
}

/// Preserve the shared stable error discriminant and the exact commit-state classification.
/// A commit-unknown response never invites a new update identity or re-encoding.
pub(crate) fn map_rejection(rejected: &write::Rejected) -> ApiError {
    let mut error = super::command::map_write_rejection(rejected);
    if let ApiError::Typed { details, .. } = &mut error {
        let mut fields = details.take().and_then(|v| v.as_object().cloned()).unwrap_or_default();
        fields.insert("write_state".into(), json!(rejected.write_state));
        fields.insert("recoverable".into(), json!(rejected.recoverable));
        if let Some(id) = rejected.update_id {
            fields.insert("update_id".into(), json!(id));
        }
        *details = Some(Value::Object(fields));
    }
    error
}

pub(crate) async fn accept_editor_init(
    db: &DatabaseConnection,
    object: Uuid,
    user: Uuid,
    update: Uuid,
    bytes: Vec<u8>,
    key: &str,
    expected: &[u8],
    body: Value,
) -> Result<AcceptOutcome, ApiError> {
    accept_inner(
        db,
        object,
        bytes,
        UpdateIntent {
            user,
            update,
            client: &format!("rest:{user}"),
            key: Some(key),
            expected: Some(expected),
            command_body: Some(body),
        },
        None,
        true,
        5,
        None,
    )
    .await
}

/// Only a server-generated fixed-member typed command can obtain the sealed permit.
pub(crate) async fn accept_shared(
    db: &DatabaseConnection,
    object: Uuid,
    bytes: Vec<u8>,
    intent: UpdateIntent<'_>,
    permit: super::block_shares::SharedWritePermit,
) -> Result<AcceptOutcome, ApiError> {
    accept_inner(db, object, bytes, intent, None, false, 5, Some(permit)).await
}
