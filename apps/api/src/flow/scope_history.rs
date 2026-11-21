//! Bounded historical reads share the isolated replay boundary with workspace Flow.
//! Sequence frontiers must still be retained; a missing historical row never means head.
use super::{
    collab::{authz::PermissionLevel, bootstrap},
    scope,
};
use crate::error::ApiError;
use crate::flow::json_value::FlowJson;
use base64::{Engine, engine::general_purpose::STANDARD};
use platform::app::AppState;
use sea_orm::{ConnectionTrait, DbBackend, Statement};
use serde_json::{Value, json};
use uuid::Uuid;

pub async fn reconstruct(
    state: &AppState,
    object: Uuid,
    actor: Uuid,
    from: i64,
    to: i64,
) -> Result<(collab_core::isolation::wire::ReplayDiffResult, Vec<u8>, Vec<u8>), ApiError> {
    if from < 0 || to < from {
        return Err(ApiError::invalid_update(
            "history sequences must satisfy 0 <= from_seq <= to_seq",
        ));
    }
    let proof = scope::authorize(&state.db, object, actor, PermissionLevel::View).await?;
    let document = state
        .db
        .query_one(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT id FROM collab_documents WHERE object_id=$1",
            vec![object.into()],
        ))
        .await?
        .ok_or_else(|| ApiError::not_found("Flow document not found"))?
        .try_get::<Uuid>("", "id")?;
    let boot = bootstrap::load(&state.db, document).await?;
    if to > boot.head_seq {
        return Err(ApiError::stale_frontier(
            "requested sequence is beyond the current head",
            Some(boot.head_seq),
            Some(&STANDARD.encode(&boot.head_frontier)),
        ));
    }
    let snapshot_frontier = boot
        .tail_updates
        .first()
        .map_or_else(|| boot.head_frontier.clone(), |row| row.before_frontier.clone());
    let frontier = |seq| {
        retained_frontier(
            &state.db,
            document,
            seq,
            boot.head_seq,
            boot.snapshot_seq,
            &boot.head_frontier,
            &snapshot_frontier,
        )
    };
    let from_frontier = frontier(from).await?;
    let to_frontier = frontier(to).await?;
    let decoded = boot
        .snapshot
        .len()
        .saturating_add(boot.tail_updates.iter().map(|r| r.bytes.len()).sum());
    bootstrap::check_bootstrap_decoded_bytes(u64::try_from(decoded).unwrap_or(u64::MAX))?;
    let request = collab_core::isolation::wire::ReplayDiffRequest {
        snapshot_frontier,
        updates: boot
            .tail_updates
            .into_iter()
            .map(|r| collab_core::isolation::wire::ReplayDiffUpdate {
                bytes: r.bytes,
                before_frontier: r.before_frontier,
                after_frontier: r.after_frontier,
            })
            .collect(),
        from_frontier: from_frontier.clone(),
        to_frontier: to_frontier.clone(),
        package_updates_base64: None,
    };
    let snapshot = boot.snapshot;
    let (result, observation) =
        tokio::task::spawn_blocking(move || collab_core::isolation::isolated_diff_observed(&snapshot, &request))
            .await
            .map_err(|_| ApiError::internal())?;
    let result = result.map_err(|error| {
        super::query::isolated_diff_error(error, &observation, object, || {
            ApiError::resync_required("requested history cannot be reconstructed", None)
        })
    })?;
    if !scope::proof_current(&state.db, &proof).await? {
        return Err(super::policy::authorization_read_unstable());
    }
    Ok((result, from_frontier, to_frontier))
}
async fn retained_frontier(
    db: &sea_orm::DatabaseConnection,
    document: Uuid,
    seq: i64,
    head: i64,
    snapshot: i64,
    head_frontier: &[u8],
    snapshot_frontier: &[u8],
) -> Result<Vec<u8>, ApiError> {
    if seq == head {
        return Ok(head_frontier.to_vec());
    }
    if seq == snapshot {
        return Ok(snapshot_frontier.to_vec());
    }
    let row = db
        .query_one(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT before_frontier,after_frontier FROM collab_updates WHERE document_id=$1 AND seq=$2",
            vec![document.into(), (if seq == 0 { 1 } else { seq }).into()],
        ))
        .await?;
    row.map(|r| {
        r.try_get("", if seq == 0 { "before_frontier" } else { "after_frontier" })
            .map_err(ApiError::from)
    })
    .transpose()?
    .ok_or_else(|| {
        ApiError::invalid_update_with_details(
            "that history sequence is no longer retained",
            json!({"reason":"history_not_retained"}),
        )
    })
}
pub async fn diff(
    state: &AppState,
    object: Uuid,
    actor: Uuid,
    from: i64,
    to: i64,
    render: super::query::Render,
) -> Result<Value, ApiError> {
    let proof = scope::authorize(&state.db, object, actor, PermissionLevel::View).await?;
    let (result, from_frontier, to_frontier) = reconstruct(state, object, actor, from, to).await?;
    // Historical typed values use today's schema policy, never the historical policy.
    let private = !super::collections::secret_record_ids(&state.db, &[object])
        .await?
        .is_empty()
        || super::collections::has_collection_secrecy(&state.db, object).await?;
    let rendered = (!private && render == super::query::Render::Markdown)
        .then(|| super::projection::render_markdown(&result.to_title, &result.to_snapshot));
    let semantic = if private {
        super::collections::redacted_semantic_content()
    } else {
        json!({"title":(result.from_title!=result.to_title).then(||json!({"before":result.from_title,"after":result.to_title})),"nodes":result.semantic_diff})
    };
    if !scope::proof_current(&state.db, &proof).await? {
        return Err(super::policy::authorization_read_unstable());
    }
    Ok(
        json!({"object_id":object,"from_seq":from,"to_seq":to,"from_frontier":STANDARD.encode(from_frontier),"to_frontier":STANDARD.encode(to_frontier),"semantic_diff":semantic,"rendered":rendered}),
    )
}

pub async fn object_at(state: &AppState, object: Uuid, actor: Uuid, seq: i64) -> Result<Value, ApiError> {
    let proof = scope::authorize(&state.db, object, actor, PermissionLevel::View).await?;
    let mut view = scope::object_view(&state.db, object, actor).await?;
    let (result, _, frontier) = reconstruct(state, object, actor, seq, seq).await?;
    *view.field_mut("title")? = json!(result.to_title);
    *view.field_mut("semantic_content")? =
        serde_json::to_value(result.to_snapshot).map_err(|_| ApiError::internal())?;
    *view.field_mut("document_seq")? = json!(seq);
    *view.field_mut("frontier")? = json!(STANDARD.encode(frontier));
    *view.field_mut("projection_seq")? = json!(seq);
    *view.field_mut("projection_lag")? = json!(0);
    if view.field("object_type") == "collection" {
        super::collections::redact_collection_content(&state.db, object, view.field_mut("semantic_content")?).await?;
    }
    if view.field("object_type") == "record"
        && !super::collections::secret_record_ids(&state.db, &[object])
            .await?
            .is_empty()
    {
        *view.field_mut("semantic_content")? = super::collections::redacted_semantic_content();
    }
    if !scope::proof_current(&state.db, &proof).await? {
        return Err(super::policy::authorization_read_unstable());
    }
    Ok(view)
}

/// Page restore appends a forward CRDT change. Typed documents remain server-command-only.
pub async fn restore_page(
    state: &AppState,
    object: Uuid,
    actor: Uuid,
    seq: i64,
    key: &str,
    message: Option<String>,
) -> Result<Value, ApiError> {
    use collab_core::CollabEngine;
    let identity = scope::identity(&state.db, object).await?;
    let proof = scope::authorize(&state.db, object, actor, PermissionLevel::Edit).await?;
    if identity.object_type != "page" {
        return Err(ApiError::invalid_update(
            "only a page can be restored to a history sequence",
        ));
    }
    if !proof.resource_active {
        return Err(ApiError::invalid_update("restore a version of an active page"));
    }
    if seq < 0 || message.as_ref().is_some_and(|m| m.chars().count() > 500) {
        return Err(ApiError::invalid_update(
            "restore requires a non-negative sequence and a message of at most 500 characters",
        ));
    }
    let body = json!({"operation":"restore_version","object_id":object,"seq":seq,"message":message});
    if let Some(result) = super::scope_commands::replay(&state.db, identity.scope_id, actor, key, &body).await? {
        return restore_outcome(result, seq);
    }
    let document = state
        .db
        .query_one(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT id FROM collab_documents WHERE object_id=$1",
            vec![object.into()],
        ))
        .await?
        .ok_or_else(|| ApiError::not_found("Flow document not found"))?
        .try_get::<Uuid>("", "id")?;
    let boot = bootstrap::load(&state.db, document).await?;
    if seq > boot.head_seq {
        return Err(ApiError::stale_frontier(
            "requested sequence is beyond the current head",
            Some(boot.head_seq),
            Some(&STANDARD.encode(&boot.head_frontier)),
        ));
    }
    let snapshot_frontier = boot
        .tail_updates
        .first()
        .map_or_else(|| boot.head_frontier.clone(), |r| r.before_frontier.clone());
    let target = retained_frontier(
        &state.db,
        document,
        seq,
        boot.head_seq,
        boot.snapshot_seq,
        &boot.head_frontier,
        &snapshot_frontier,
    )
    .await?;
    bootstrap::check_bootstrap_decoded_bytes(
        u64::try_from(
            boot.snapshot
                .len()
                .saturating_add(boot.tail_updates.iter().map(|r| r.bytes.len()).sum()),
        )
        .unwrap_or(u64::MAX),
    )?;
    let mut engine =
        collab_core::LoroCollabEngine::load(&boot.snapshot).map_err(|e| super::command::map_collab_error(&e))?;
    for row in boot.tail_updates {
        engine
            .import_update(&row.bytes)
            .map_err(|e| super::command::map_collab_error(&e))?;
    }
    let base = engine.frontier();
    engine
        .revert_to_frontier(&collab_core::Frontier::from_bytes(target))
        .map_err(|e| super::command::map_collab_error(&e))?;
    engine
        .initialize_page_editor()
        .map_err(|e| super::command::map_collab_error(&e))?;
    if crate::flow::frontier::same(engine.frontier().as_bytes(), base.as_bytes())? {
        use sea_orm::TransactionTrait;
        let tx = state.db.begin().await?;
        scope::require_personal_write(&tx).await?;
        scope::fence(&tx, &proof).await?;
        if let Some(result) = super::scope_commands::lock_and_replay(&tx, identity.scope_id, actor, key, &body).await? {
            tx.commit().await?;
            return restore_outcome(result, seq);
        }
        let current = tx
            .query_one(Statement::from_sql_and_values(
                DbBackend::Postgres,
                "SELECT head_frontier FROM collab_documents WHERE id=$1 FOR SHARE",
                vec![document.into()],
            ))
            .await?
            .ok_or_else(|| ApiError::not_found("Flow document not found"))?
            .try_get::<Vec<u8>>("", "head_frontier")?;
        if !crate::flow::frontier::same(&current, base.as_bytes())? {
            return Err(ApiError::stale_frontier(
                "document changed while preparing restore",
                None,
                None,
            ));
        }
        // A no-op still reserves its request identity, so the same key cannot later restore another
        // sequence. The event is audit-only; no content dispatch or document update is invented.
        let event = super::scope_event::insert_object_audit(
            &tx,
            &identity,
            actor,
            "flow.version.restore_noop",
            key,
            json!({"object_id":object,"target_seq":seq}),
        )
        .await?;
        let result = json!({"object":scope::object_view(&tx,object,actor).await?,"target_seq":seq,"head_seq":boot.head_seq,"changed":false});
        super::scope_commands::record(&tx, identity.scope_id, actor, key, body, object, event, result.clone()).await?;
        tx.commit().await?;
        return Ok(result);
    }
    let bytes = engine
        .export_from(&base)
        .map_err(|e| super::command::map_collab_error(&e))?;
    let update = super::collab::write::replay_stable_update_id(document, key);
    match super::scope_write::accept(
        &state.db,
        object,
        bytes,
        super::scope_write::UpdateIntent {
            user: actor,
            update,
            client: &format!("rest:{actor}"),
            key: Some(key),
            expected: Some(base.as_bytes()),
            command_body: Some(body.clone()),
        },
    )
    .await?
    {
        super::collab::write::AcceptOutcome::Accepted(_accepted) => {
            // Return the transaction's immutable receipt view, not a later racing document head.
            let result = super::scope_commands::replay(&state.db, identity.scope_id, actor, key, &body)
                .await?
                .ok_or_else(ApiError::internal)?;
            restore_outcome(result, seq)
        }
        super::collab::write::AcceptOutcome::Rejected(rejected) => Err(super::scope_write::map_rejection(&rejected)),
    }
}
fn restore_outcome(mut result: Value, seq: i64) -> Result<Value, ApiError> {
    if result.get("changed").is_some() {
        return Ok(result);
    }
    let head = result
        .get("head_seq")
        .cloned()
        .unwrap_or_else(|| result.field("object").field("document_seq").clone());
    let object = result
        .get_mut("object")
        .map(Value::take)
        .ok_or_else(ApiError::internal)?;
    Ok(json!({"object":object,"target_seq":seq,"head_seq":head,"changed":true}))
}
