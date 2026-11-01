#![allow(clippy::items_after_statements)]
//! Restore a page to an accepted history sequence as a new forward change.
//!
//! The target sequence is a Loro frontier, not a truncated log. `LoroDoc::revert_to` appends
//! the inverse of everything after that frontier, so a peer with a concurrent edit imports the
//! delta and both replicas keep the other's new operations.

use base64::Engine;
use base64::engine::general_purpose::STANDARD as BASE64;
use collab_core::{CollabEngine, Frontier};
use sea_orm::{ConnectionTrait, DatabaseConnection, DbBackend, FromQueryResult, Statement};
use serde::Serialize;
use uuid::Uuid;

use super::collab::runtime;
use super::collab::write::{self, AcceptOutcome};
use super::command::{self, map_write_rejection};
use super::document;
use super::event_origin::CommandOrigin;
use super::model::FlowObjectView;
use super::query;
use super::repository::{self, ObjectViewRow};
use crate::error::ApiError;

const IDEMPOTENCY_KEY_MAX_BYTES: usize = 128;
const MESSAGE_MAX_CHARS: usize = 500;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RestorePlan {
    Noop,
    UpdateAfter(i64),
    BeforeFirstUpdate,
    Snapshot,
    NotRetained,
    Corrupt,
}

#[derive(Debug, Serialize)]
pub struct RestoreVersionOutcome {
    pub object: FlowObjectView,
    pub target_seq: i64,
    pub head_seq: i64,
    pub changed: bool,
}

pub struct RestoreVersionRequest {
    pub object_id: Uuid,
    pub actor_id: Uuid,
    pub actor_is_bot: bool,
    pub checked_epoch: i64,
    pub target_seq: i64,
    pub idempotency_key: String,
    pub message: Option<String>,
    pub origin_client_id: String,
    pub origin: CommandOrigin,
}

/// Chooses which stored frontier represents `target` given whether those rows still exist.
///
/// `target_update_present` is the `collab_updates` row at `target`. `first_update_present` is
/// the row at sequence 1, used when the caller asks for the empty document at sequence 0.
#[must_use]
pub fn plan_page_restore(
    target: i64,
    head_seq: i64,
    snapshot_seq: i64,
    target_update_present: bool,
    first_update_present: bool,
) -> RestorePlan {
    if target == head_seq {
        return RestorePlan::Noop;
    }
    if target == 0 {
        if first_update_present {
            return RestorePlan::BeforeFirstUpdate;
        }
        if snapshot_seq == 0 {
            return RestorePlan::Snapshot;
        }
        return RestorePlan::NotRetained;
    }
    if target_update_present {
        return RestorePlan::UpdateAfter(target);
    }
    if target == snapshot_seq {
        return RestorePlan::Snapshot;
    }
    if target < snapshot_seq {
        RestorePlan::NotRetained
    } else {
        RestorePlan::Corrupt
    }
}

pub async fn restore_page_version(
    state: &platform::app::AppState,
    workspace_id: Uuid,
    request: RestoreVersionRequest,
) -> Result<RestoreVersionOutcome, ApiError> {
    validate_request(&request)?;
    let head = load_head(&state.db, workspace_id, request.object_id).await?;
    if head.object_type != "page" {
        return Err(ApiError::invalid_update(
            "only a page can be restored to a history sequence".to_string(),
        ));
    }
    if head.lifecycle_status != "active" {
        return Err(ApiError::invalid_update(
            "restore a version of an active page".to_string(),
        ));
    }
    if request.target_seq < 0 {
        return Err(ApiError::invalid_update(
            "history sequence must be non-negative".to_string(),
        ));
    }
    if request.target_seq > head.head_seq {
        let frontier = BASE64.encode(&head.head_frontier);
        return Err(ApiError::stale_frontier(
            "history sequence is beyond the current document head",
            Some(head.head_seq),
            Some(&frontier),
        ));
    }
    if let Some(existing) = repository::find_idempotent_event(&state.db, workspace_id, &request.idempotency_key).await?
    {
        if existing.event_type != "flow.content.accepted" || existing.aggregate_id != head.document_id.to_string() {
            return Err(ApiError::idempotency_key_body_drift(
                "idempotency_key was already used for a different operation",
            ));
        }
        let view = load_view(&state.db, request.object_id).await?;
        return Ok(outcome_of(view, request.target_seq, false));
    }

    let target_present = update_exists(&state.db, head.document_id, request.target_seq).await?;
    let first_present = if request.target_seq == 0 {
        update_exists(&state.db, head.document_id, 1).await?
    } else {
        false
    };
    let plan = plan_page_restore(
        request.target_seq,
        head.head_seq,
        head.snapshot_seq,
        target_present,
        first_present,
    );
    let frontier_bytes = match plan {
        RestorePlan::Noop => {
            let view = load_view(&state.db, request.object_id).await?;
            return Ok(outcome_of(view, request.target_seq, false));
        }
        RestorePlan::NotRetained => {
            return Err(ApiError::invalid_update_with_details(
                "that history sequence is no longer retained",
                serde_json::json!({ "reason": "history_not_retained" }),
            ));
        }
        RestorePlan::Corrupt => {
            tracing::error!(
                document_id = %head.document_id,
                target_seq = request.target_seq,
                "restore history row is missing inside the retained tail"
            );
            return Err(ApiError::resync_required(
                "requested history cannot be reconstructed",
                None,
            ));
        }
        RestorePlan::Snapshot => head.snapshot_frontier.clone(),
        RestorePlan::BeforeFirstUpdate => frontier_column(&state.db, head.document_id, 1, false).await?,
        RestorePlan::UpdateAfter(seq) => frontier_column(&state.db, head.document_id, seq, true).await?,
    };

    let mut engine = document::load_head_engine(&state.db, head.document_id).await?;
    let base = engine.frontier();
    engine
        .revert_to_frontier(&Frontier::from_bytes(frontier_bytes))
        .map_err(|err| map_write_or_collab(&err))?;
    if engine.frontier() == base {
        let view = load_view(&state.db, request.object_id).await?;
        return Ok(outcome_of(view, request.target_seq, false));
    }
    let bytes = engine
        .export_from(&base)
        .map_err(|err| command::map_collab_error(&err))?;
    if bytes.is_empty() {
        let view = load_view(&state.db, request.object_id).await?;
        return Ok(outcome_of(view, request.target_seq, false));
    }

    let seq = request.target_seq;
    let message = Some(
        request
            .message
            .clone()
            .unwrap_or_else(|| format!("Restore page to seq {seq}")),
    );
    let collab = runtime::runtime();
    let outcome = write::accept_update(
        &state.db,
        &collab.cache,
        &collab.coordinator,
        &collab.registry,
        &collab.snapshot,
        crate::config::runtime().flow.dispatch_max_attempts,
        None,
        write::UpdateRequest {
            document_id: head.document_id,
            update_id: write::replay_stable_update_id(head.document_id, &request.idempotency_key),
            bytes,
            idempotency_key: Some(request.idempotency_key.clone()),
            event_idempotency_key: Some(request.idempotency_key.clone()),
            origin_client_id: Some(request.origin_client_id.clone()),
            message,
            actor_id: request.actor_id,
            actor_is_bot: request.actor_is_bot,
            workspace_id,
            checked_epoch: request.checked_epoch,
            expected_frontier: None,
            origin: request.origin,
        },
    )
    .await?;
    let accepted = match outcome {
        AcceptOutcome::Accepted(accepted) => accepted,
        AcceptOutcome::Rejected(rejected) => return Err(map_write_rejection(&rejected)),
    };
    if accepted.should_advance_snapshot {
        super::collab::snapshot::spawn_background(&collab.snapshot, state.db.clone(), head.document_id);
    }
    let view = load_view(&state.db, request.object_id).await?;
    Ok(outcome_of(view, request.target_seq, true))
}

fn validate_request(request: &RestoreVersionRequest) -> Result<(), ApiError> {
    let key_bytes = request.idempotency_key.len();
    if !(1..=IDEMPOTENCY_KEY_MAX_BYTES).contains(&key_bytes) {
        return Err(ApiError::invalid_update(format!(
            "idempotency_key must be 1-{IDEMPOTENCY_KEY_MAX_BYTES} bytes"
        )));
    }
    if let Some(message) = &request.message
        && message.chars().count() > MESSAGE_MAX_CHARS
    {
        return Err(ApiError::invalid_update(format!(
            "message must be at most {MESSAGE_MAX_CHARS} characters"
        )));
    }
    Ok(())
}

fn map_write_or_collab(err: &collab_core::CollabError) -> ApiError {
    command::map_collab_error(err)
}

fn outcome_of(view: ObjectViewRow, target_seq: i64, changed: bool) -> RestoreVersionOutcome {
    let head_seq = view.document_seq;
    RestoreVersionOutcome {
        object: query::object_view_from_row(view),
        target_seq,
        head_seq,
        changed,
    }
}

async fn load_view(db: &DatabaseConnection, object_id: Uuid) -> Result<ObjectViewRow, ApiError> {
    repository::fetch_object_view(db, object_id)
        .await?
        .ok_or_else(ApiError::internal)
}

#[derive(Debug, FromQueryResult)]
struct HeadRow {
    document_id: Uuid,
    object_type: String,
    lifecycle_status: String,
    head_seq: i64,
    head_frontier: Vec<u8>,
    snapshot_seq: i64,
    snapshot_frontier: Vec<u8>,
}

async fn load_head(db: &DatabaseConnection, workspace_id: Uuid, object_id: Uuid) -> Result<HeadRow, ApiError> {
    HeadRow::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "SELECT cd.id AS document_id, fo.object_type, fo.lifecycle_status, \
                cd.head_seq, cd.head_frontier, cd.snapshot_seq, cd.snapshot_frontier \
         FROM flow_objects fo \
         JOIN collab_documents cd ON cd.object_id = fo.id \
         WHERE fo.id = $1 AND fo.workspace_id = $2",
        vec![object_id.into(), workspace_id.into()],
    ))
    .one(db)
    .await?
    .ok_or_else(|| ApiError::not_found("flow object not found".to_string()))
}

async fn update_exists(db: &DatabaseConnection, document_id: Uuid, seq: i64) -> Result<bool, ApiError> {
    if seq <= 0 {
        return Ok(false);
    }
    let row = db
        .query_one(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT 1 FROM collab_updates WHERE document_id = $1 AND seq = $2",
            vec![document_id.into(), seq.into()],
        ))
        .await?;
    Ok(row.is_some())
}

async fn frontier_column(
    db: &DatabaseConnection,
    document_id: Uuid,
    seq: i64,
    after: bool,
) -> Result<Vec<u8>, ApiError> {
    #[derive(FromQueryResult)]
    struct Row {
        before_frontier: Vec<u8>,
        after_frontier: Vec<u8>,
    }
    let row = Row::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "SELECT before_frontier, after_frontier FROM collab_updates WHERE document_id = $1 AND seq = $2",
        vec![document_id.into(), seq.into()],
    ))
    .one(db)
    .await?
    .ok_or_else(|| ApiError::resync_required("requested history cannot be reconstructed", None))?;
    Ok(if after { row.after_frontier } else { row.before_frontier })
}

#[cfg(test)]
mod tests {
    use super::{RestorePlan, plan_page_restore};

    #[test]
    fn restore_plan_is_a_noop_at_the_head_and_refuses_compacted_history() {
        assert_eq!(plan_page_restore(4, 4, 1, true, true), RestorePlan::Noop);
        assert_eq!(plan_page_restore(3, 4, 1, true, true), RestorePlan::UpdateAfter(3));
        assert_eq!(plan_page_restore(0, 4, 0, false, true), RestorePlan::BeforeFirstUpdate);
        assert_eq!(plan_page_restore(0, 4, 0, false, false), RestorePlan::Snapshot);
        assert_eq!(plan_page_restore(1, 4, 2, false, true), RestorePlan::NotRetained);
        assert_eq!(plan_page_restore(2, 4, 2, false, true), RestorePlan::Snapshot);
        assert_eq!(plan_page_restore(3, 4, 2, false, true), RestorePlan::Corrupt);
    }
}
