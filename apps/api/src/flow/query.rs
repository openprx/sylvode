//! Read paths: `GET .../flow/objects`, `GET .../flow/objects/{id}` and
//! `GET .../flow/objects/{id}/history`.
//!
//! Every read here comes from `flow_objects` + `collab_documents` + `flow_object_projections`
//! (or, for history, `collab_updates` + `business_events`); nothing decodes a CRDT snapshot on the
//! read path.

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD as BASE64_URL;
use collab_core::isolation::IsolatedApplyError;
use platform::app::AppState;
use platform::timestamp::Timestamp;
use serde_json::json;
use uuid::Uuid;

use crate::error::ApiError;

use super::collab::frame::SERVER_REJECTED_REASON_UNSUPPORTED_PLATFORM;
use super::collab::{limits, runtime};
use super::model::{FlowFeatureView, FlowObjectView, ProjectionLagItem, ProjectionLagResponse};
use super::policy::{self, FlowReadContext};
use super::repository::{
    self, FlowSettingsRow, ObjectViewRow, ProjectionLagAggregateFilter, ProjectionLagFilter, ProjectionLagRow,
};

pub const DEFAULT_LIST_LIMIT: u64 = 50;
pub const MAX_LIST_LIMIT: u64 = 100;
pub const MAX_NAVIGATOR_DEPTH: u64 = 20;

/// Computes lag for both the projection-lag and search surfaces. Keeping the subtraction here
/// prevents the two public APIs from silently acquiring different negative/corrupt-row behavior.
#[must_use]
pub(crate) const fn projection_lag(head_seq: i64, projection_seq: i64) -> i64 {
    head_seq.saturating_sub(projection_seq)
}

/// Each authorized-scan batch is one `page_limit_max`-sized page of *candidate* rows —
/// `limits-v1.md`'s own reasoning for `authorized_scan_rows_max` ("最多 overfetch 10 个最大页")
/// is ten of these, not an independently chosen tuning constant.
const SCAN_BATCH_SIZE: u64 = MAX_LIST_LIMIT;

/// Advances the running count of *candidate* rows checked so far and returns
/// `limit_exceeded`/`scan_budget` the instant it would exceed `limits::AUTHORIZED_SCAN_ROWS_MAX` —
/// `limits-v1.md`: "最多 overfetch 10 个最大页；授权过滤后不足一页也不得无界扫描". `examined` is
/// counted *before* policy filtering runs on the row (`limits-v1.md`: "Scan budget 统计
/// policy-filter 前实际检查 rows"), so this must be called for every candidate a scan loop looks
/// at, whether or not that candidate turns out to be policy-visible.
///
/// The error's `observed` is always exactly `AUTHORIZED_SCAN_ROWS_MAX`, never the caller's true
/// candidate count beyond it. `limits-v1.md` also says "不向 caller 返回过滤前 count" — the
/// pre-filter row count is exactly the number that would tell a caller how many rows exist that
/// policy would have rejected them from seeing, which is the thing this ceiling exists to keep
/// from leaking. Echoing back the fixed ceiling instead of the real scan depth satisfies
/// `error-mapping-v1.md`'s convention of naming what was exceeded (every other `limit_exceeded`
/// case here does the same) without revealing anything data-dependent: it is the same public
/// number for every caller, on every request, win or lose — already published verbatim in
/// `Bootstrap.limits.authorized_scan_rows_max` — so it carries zero information beyond "the scan
/// hit its ceiling".
fn check_scan_budget(examined: u64) -> Result<(), ApiError> {
    if examined > limits::AUTHORIZED_SCAN_ROWS_MAX {
        return Err(ApiError::limit_exceeded(
            "authorized scan budget exceeded before enough policy-visible rows were found",
            "scan_budget",
            Some(json!(limits::AUTHORIZED_SCAN_ROWS_MAX)),
            Some(json!(limits::AUTHORIZED_SCAN_ROWS_MAX)),
            None,
        ));
    }
    Ok(())
}

/// Projection-lag counterpart of [`scan_objects_within_budget`]. Every candidate batch is sent
/// through the shared batch evaluator before a row can reach either `items` or the aggregate
/// input. Hidden rows advance only the internal scan cursor and candidate budget.
async fn scan_projection_lag_within_budget(
    state: &AppState,
    access: &FlowReadContext,
    mut filter: ProjectionLagFilter,
    needed: usize,
) -> Result<Option<Vec<ProjectionLagRow>>, ApiError> {
    let mut accepted = Vec::new();
    let mut examined = 0_u64;
    loop {
        filter.limit = SCAN_BATCH_SIZE;
        let batch = repository::list_projection_lag_candidates(&state.db, &filter).await?;
        let batch_len = batch.len();
        if batch_len == 0 {
            return Ok(Some(accepted));
        }
        let object_ids: Vec<Uuid> = batch.iter().map(|row| row.object_id).collect();
        let Some(visible) =
            policy::authorize_flow_objects(state, access, &object_ids, super::collab::authz::PermissionLevel::View)
                .await?
        else {
            return Ok(None);
        };
        if visible.len() != batch_len {
            return Err(ApiError::internal());
        }
        for (row, is_visible) in batch.into_iter().zip(visible) {
            examined = examined.saturating_add(1);
            check_scan_budget(examined)?;
            filter.after = Some((row.created_at, row.object_id));
            if is_visible {
                accepted.push(row);
                if accepted.len() >= needed {
                    return Ok(Some(accepted));
                }
            }
        }
        if (batch_len as u64) < SCAN_BATCH_SIZE {
            return Ok(Some(accepted));
        }
    }
}

/// Clamps a caller-supplied `limit` query parameter.
///
/// Rejects anything outside `1..=100` rather than silently clamping it — `limits-v1.md`'s
/// boundary-testing convention throughout this contract is exact-accepted / one-past-rejected,
/// not silent clamping.
pub fn validate_limit(limit: Option<u64>) -> Result<u64, ApiError> {
    match limit {
        None => Ok(DEFAULT_LIST_LIMIT),
        Some(0) => Err(ApiError::invalid_update("limit must be at least 1".to_string())),
        Some(value) if value > MAX_LIST_LIMIT => Err(ApiError::limit_exceeded(
            format!("limit must be at most {MAX_LIST_LIMIT}"),
            "page_size",
            Some(json!(MAX_LIST_LIMIT)),
            Some(json!(value)),
            None,
        )),
        Some(value) => Ok(value),
    }
}

pub fn object_view_from_row(row: ObjectViewRow) -> FlowObjectView {
    let projection_lag = row.document_seq - row.projection_document_seq;
    FlowObjectView {
        id: row.id,
        workspace_id: row.workspace_id,
        project_id: row.project_id,
        parent_id: row.parent_id,
        object_type: row.object_type,
        lifecycle_status: row.lifecycle_status,
        governance_metadata: row.governance_metadata,
        title: row.projection_title,
        semantic_content: row.projection_state,
        document_id: row.document_id,
        document_seq: row.document_seq,
        frontier: base64::engine::general_purpose::STANDARD.encode(&row.document_frontier),
        projection_seq: row.projection_document_seq,
        projection_lag,
        created_at: row.created_at.to_rfc3339(),
        updated_at: row.updated_at.to_rfc3339(),
        archived_at: row.archived_at.map(platform::timestamp::Timestamp::to_rfc3339),
        appearance: super::appearance::parse_stored(&row.appearance),
    }
}

/// `render` query parameter values (`rest-api-v1.md`: `render=semantic_json|markdown`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Render {
    SemanticJson,
    Markdown,
}

impl Render {
    pub fn parse(raw: Option<&str>) -> Result<Self, ApiError> {
        match raw {
            None | Some("semantic_json") => Ok(Self::SemanticJson),
            Some("markdown") => Ok(Self::Markdown),
            Some(other) => Err(ApiError::invalid_update(format!(
                "render must be semantic_json or markdown, got '{other}'"
            ))),
        }
    }
}

/// Maps an [`isolated_diff`](collab_core::isolation::isolated_diff) refusal to the REST error the
/// object diff endpoint returns, classified like a write's refusal
/// (`flow::collab::write::refusal`, a full-window attempt that cannot rebase). The three resource
/// ceilings are `limit_exceeded` with their frozen `limit_kind`, except a wall kill of a worker that
/// the host did not give the CPU, which is `server_draining{apply_busy}` with `retry_mode = resend`;
/// a corrupt retained history is `resync_required` (via `history_invalid`); a host short of
/// resources is `apply_busy` too, and a broken isolation host is `server_rejected` with a named
/// reason. `UnsupportedPlatform` (a non-Linux build, `ADR-0014`'s platform matrix) is
/// `server_rejected` carrying only the safe classification
/// [`SERVER_REJECTED_REASON_UNSUPPORTED_PLATFORM`]: it is deterministic and permanent for this
/// process, so it must not read as retryable.
pub(crate) fn isolated_diff_error(
    error: IsolatedApplyError,
    observation: &collab_core::isolation::WorkerObservation,
    object_id: Uuid,
    history_invalid: impl FnOnce() -> ApiError,
) -> ApiError {
    isolated_replay_error(error, observation, object_id, ReplayBudget::Write, history_invalid)
}

/// The ceilings of the window a refused replay ran under, which its `limit_exceeded` answer names.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ReplayBudget {
    /// The write window: the diff of retained history (`decode_apply_*`).
    Write,
    /// The import window of an uploaded document (`IMPORT_*`).
    Import,
}

/// [`isolated_diff_error`] for the window of `budget`: the same classification, with the message
/// and the `limit` of a ceiling kill taken from the budget the worker ran under.
pub(crate) fn isolated_replay_error(
    error: IsolatedApplyError,
    observation: &collab_core::isolation::WorkerObservation,
    object_id: Uuid,
    budget: ReplayBudget,
    history_invalid: impl FnOnce() -> ApiError,
) -> ApiError {
    use super::collab::write::refusal::{AttemptLog, RebasePolicy, Refusal, Stopped, classify, record};

    let stopped = Stopped::of(&error);
    if let IsolatedApplyError::HostFailure(failure) = &error {
        tracing::warn!(detail = failure.detail.as_str(), %object_id, "diff replay isolation host failed");
    }
    let refusal = classify(
        error,
        observation,
        collab_core::isolation::WindowMode::Full,
        RebasePolicy::None,
        crate::config::runtime().flow.setup_cpu_budget_ms,
    );
    record(
        &AttemptLog {
            document_id: object_id,
            update_id: None,
            base_head_seq: None,
            requested: collab_core::isolation::WindowMode::Full,
            cooling: false,
            observation,
        },
        stopped,
        &refusal,
    );
    match refusal {
        Refusal::Content { limit_kind, limit } => {
            let (message, limit) = match (budget, limit_kind) {
                (ReplayBudget::Write, "decode_apply_cpu_ms") => ("diff replay exceeded the isolated CPU budget", limit),
                (ReplayBudget::Write, "decode_apply_wall_ms") => {
                    ("diff replay exceeded the isolated wall-clock budget", limit)
                }
                (ReplayBudget::Write, _) => ("diff replay exceeded the isolated memory budget", limit),
                (ReplayBudget::Import, "decode_apply_cpu_ms") => (
                    "package document import exceeded the isolated CPU budget",
                    collab_core::isolation::IMPORT_DECODE_CPU_MS_MAX,
                ),
                (ReplayBudget::Import, "decode_apply_wall_ms") => (
                    "package document import exceeded the isolated wall-clock budget",
                    collab_core::isolation::IMPORT_DECODE_WALL_MS_MAX,
                ),
                (ReplayBudget::Import, _) => (
                    "package document import exceeded the isolated memory budget",
                    collab_core::isolation::IMPORT_ISOLATED_MEMORY_BYTES_MAX,
                ),
            };
            ApiError::limit_exceeded(
                message,
                limit_kind,
                Some(json!(limit)),
                Some(json!(limit.saturating_add(1))),
                None,
            )
        }
        Refusal::Busy {
            cause,
            mode,
            retry_after_ms,
        } => ApiError::server_busy(cause.wire_value(), mode.wire_value(), retry_after_ms),
        Refusal::Defect { reason } => ApiError::server_rejected(reason),
        Refusal::Collab(_) => history_invalid(),
        Refusal::Unsupported { os } => {
            tracing::error!(
                os,
                %object_id,
                "replay refused: the isolated-apply boundary requires Linux (ADR-0014)"
            );
            ApiError::server_rejected(SERVER_REJECTED_REASON_UNSUPPORTED_PLATFORM)
        }
    }
}

pub struct ProjectionLagParams {
    pub workspace_id: Uuid,
    pub project_id: Option<Uuid>,
    pub cursor: Option<String>,
    pub limit: Option<u64>,
}

/// Aggregates the complete already-policy-filtered query scope.
///
/// `p95_lag` uses the nearest-rank definition: sort ascending and select rank
/// `ceil(0.95 * n)` (one-based). An empty page returns `(0, 0)`; for 1 through 19 samples that
/// rank is the final sample, so p95 equals the scope maximum. Both values are computed only after
/// authorization, but before cursor/page slicing: page navigation therefore never changes either
/// aggregate, and an inaccessible object's lag cannot change an aggregate field.
#[cfg(test)]
fn projection_lag_aggregates(items: &[ProjectionLagItem]) -> (i64, i64) {
    if items.is_empty() {
        return (0, 0);
    }
    let mut lags: Vec<i64> = items.iter().map(|item| item.lag).collect();
    lags.sort_unstable();
    let max_lag = lags.last().copied().unwrap_or(0);
    let rank = lags.len().saturating_mul(95).div_ceil(100);
    let p95_lag = lags.get(rank.saturating_sub(1)).copied().unwrap_or(0);
    (max_lag, p95_lag)
}

/// `GET /workspaces/{workspace_id}/flow/projection-lag` domain read.
pub async fn get_projection_lag(
    state: &AppState,
    access: &FlowReadContext,
    params: ProjectionLagParams,
) -> Result<Option<ProjectionLagResponse>, ApiError> {
    if params.workspace_id != access.workspace_id() {
        return Err(ApiError::internal());
    }
    runtime::runtime().ensure_workspace_accepting(params.workspace_id)?;
    let limit = validate_limit(params.limit)?;
    let after = params.cursor.as_deref().map(decode_cursor).transpose()?;
    let limit_usize = usize::try_from(limit).unwrap_or(usize::MAX);
    let filter = ProjectionLagFilter {
        workspace_id: params.workspace_id,
        project_id: params.project_id,
        after,
        limit: 0,
    };
    let Some(mut rows) =
        scan_projection_lag_within_budget(state, access, filter, limit_usize.saturating_add(1)).await?
    else {
        return Ok(None);
    };
    let aggregate = repository::aggregate_projection_lag(
        &state.db,
        &ProjectionLagAggregateFilter {
            workspace_id: params.workspace_id,
            project_id: params.project_id,
            actor_id: access.actor_id(),
            principal_kind: access.principal_kind().as_str(),
            is_human_admin: access.is_human_admin(),
            max_chain_nodes: i64::try_from(super::collab::authz::MAX_CHAIN_NODES).unwrap_or(i64::MAX),
            tree_depth_max: i64::try_from(super::collab::authz::TREE_DEPTH_MAX).unwrap_or(i64::MAX),
        },
    )
    .await?;
    if !policy::ensure_epoch_current(state, access).await? {
        return Ok(None);
    }
    let next_cursor = if rows.len() > limit_usize {
        rows.truncate(limit_usize);
        rows.last().map(|row| encode_cursor(row.created_at, row.object_id))
    } else {
        None
    };
    let items: Vec<ProjectionLagItem> = rows
        .into_iter()
        .map(|row| ProjectionLagItem {
            object_id: row.object_id,
            head_seq: row.head_seq,
            projection_seq: row.projection_seq,
            lag: projection_lag(row.head_seq, row.projection_seq),
        })
        .collect();
    Ok(Some(ProjectionLagResponse {
        max_lag: aggregate.max_lag,
        p95_lag: aggregate.p95_lag,
        items,
        next_cursor,
    }))
}

/// Row-to-wire mapping shared by the `GET` handler and `command::set_flow_feature`'s response.
///
/// `None` (never-provisioned workspace) maps to the column defaults with `updated_at`/`updated_by`
/// left `null` (see [`FlowFeatureView`]'s doc comment).
pub fn feature_view_from_row(row: Option<FlowSettingsRow>) -> FlowFeatureView {
    row.map_or_else(
        || FlowFeatureView {
            flow_enabled: true,
            default_member_level: "edit".to_string(),
            bridge_enabled: true,
            authz_epoch: 0,
            updated_at: None,
            updated_by: None,
        },
        |row| FlowFeatureView {
            flow_enabled: row.flow_enabled,
            default_member_level: row.default_member_level,
            bridge_enabled: row.bridge_enabled,
            authz_epoch: row.authz_epoch,
            updated_at: Some(row.updated_at.to_rfc3339()),
            updated_by: row.updated_by,
        },
    )
}

/// `GET /api/v1/workspaces/{workspace_id}/features/flow`.
pub async fn get_flow_feature(state: &AppState, workspace_id: Uuid) -> Result<FlowFeatureView, ApiError> {
    let row = repository::fetch_flow_settings(&state.db, workspace_id).await?;
    Ok(feature_view_from_row(row))
}

fn encode_cursor(created_at: Timestamp, id: Uuid) -> String {
    BASE64_URL.encode(format!("{}|{id}", created_at.to_rfc3339()))
}

fn decode_cursor(raw: &str) -> Result<(Timestamp, Uuid), ApiError> {
    let bytes = BASE64_URL
        .decode(raw)
        .map_err(|_| ApiError::invalid_update("cursor is not valid".to_string()))?;
    let text = String::from_utf8(bytes).map_err(|_| ApiError::invalid_update("cursor is not valid".to_string()))?;
    let (created_at_raw, id_raw) = text
        .split_once('|')
        .ok_or_else(|| ApiError::invalid_update("cursor is not valid".to_string()))?;
    let created_at = Timestamp::parse_rfc3339(created_at_raw)
        .map_err(|_| ApiError::invalid_update("cursor is not valid".to_string()))?;
    let id = Uuid::parse_str(id_raw).map_err(|_| ApiError::invalid_update("cursor is not valid".to_string()))?;
    Ok((created_at, id))
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]
mod tests {
    use super::*;
    use crate::error::ApiErrorKind;

    /// The object diff's `UnsupportedPlatform` mapping (constructed directly, so it runs on
    /// Linux): `server_rejected` carrying only the safe classification, never `Internal` and never
    /// the history-invalid `resync_required` a client would retry through.
    #[test]
    fn isolated_diff_unsupported_platform_maps_to_server_rejected_with_its_reason() {
        let mapped = isolated_diff_error(
            IsolatedApplyError::UnsupportedPlatform { os: "macos" },
            &collab_core::isolation::WorkerObservation::default(),
            Uuid::new_v4(),
            || ApiError::resync_required("requested history cannot be reconstructed", None),
        );
        assert_eq!(mapped.kind(), ApiErrorKind::ServerRejected);
        let ApiError::Typed { details, .. } = mapped else {
            panic!("server_rejected must be a typed error");
        };
        let details = details.expect("server_rejected carries details");
        let object = details.as_object().expect("details is an object");
        assert_eq!(object.keys().collect::<Vec<_>>(), vec!["reason"]);
        assert_eq!(object["reason"], "isolated_apply_unsupported_platform");
    }

    fn diff_error(error: IsolatedApplyError, observation: collab_core::isolation::WorkerObservation) -> ApiError {
        isolated_diff_error(error, &observation, Uuid::new_v4(), || {
            ApiError::resync_required("requested history cannot be reconstructed", None)
        })
    }

    fn typed_details(error: &ApiError) -> serde_json::Value {
        let ApiError::Typed { details, .. } = error else {
            panic!("a typed error, got {error:?}");
        };
        details.clone().expect("the error carries details")
    }

    /// A wall kill of a diff replay whose worker had barely any CPU is the starvation answer:
    /// `apply_busy` with `resend`, HTTP envelope 409.
    #[tokio::test]
    async fn a_starved_diff_replay_is_apply_busy_with_resend() {
        use axum::response::IntoResponse;
        let error = diff_error(
            IsolatedApplyError::WallCeiling,
            collab_core::isolation::WorkerObservation {
                metered_cpu_us: Some(3_000),
                ..collab_core::isolation::WorkerObservation::default()
            },
        );
        assert_eq!(
            error.kind(),
            ApiErrorKind::ServerDraining(crate::error::ServerDrainingReason::ApplyBusy)
        );
        assert_eq!(
            typed_details(&error),
            serde_json::json!({"retry_after_ms": 250, "retry_mode": "resend", "cause": "cpu_starved"})
        );
        let response = error.into_response();
        let bytes = axum::body::to_bytes(response.into_body(), 1 << 20).await.unwrap();
        let body: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(body["code"], 409);
        assert_eq!(body["error_code"], "server_draining");
        assert_eq!(body["details"]["reason"], "apply_busy");
        assert_eq!(body["details"]["retry_mode"], "resend");
    }

    /// The diff replay is never asked to rebase, and the content limits are unchanged.
    #[test]
    fn a_diff_replay_keeps_its_content_limits_and_never_rebases() {
        use collab_core::isolation::wire::WindowDecision;
        use collab_core::window_check::FallbackReason;
        let heavy = collab_core::isolation::WorkerObservation {
            metered_cpu_us: Some(60_000),
            decision: Some(WindowDecision::Fallback(FallbackReason::NotSequential)),
            ..collab_core::isolation::WorkerObservation::default()
        };
        for (error, kind) in [
            (IsolatedApplyError::CpuCeiling, "decode_apply_cpu_ms"),
            (IsolatedApplyError::WallCeiling, "decode_apply_wall_ms"),
            (IsolatedApplyError::MemoryCeiling, "isolated_apply_memory_bytes"),
        ] {
            let mapped = diff_error(error, heavy);
            assert_eq!(mapped.kind(), ApiErrorKind::LimitExceeded, "{kind}");
            assert_eq!(typed_details(&mapped)["limit_kind"], kind);
        }
    }

    /// Host failures follow rows 6-10 of the classification.
    #[test]
    fn a_diff_replay_host_failure_is_busy_or_a_named_defect() {
        use collab_core::isolation::{HostFailure, HostFailureKind, WorkerObservation};
        let failure = |kind| IsolatedApplyError::HostFailure(HostFailure::new(kind, "test"));
        let busy = diff_error(failure(HostFailureKind::SpawnResources), WorkerObservation::default());
        assert_eq!(
            typed_details(&busy),
            serde_json::json!({"retry_after_ms": 500, "retry_mode": "resend", "cause": "worker_resources"})
        );
        let busy = diff_error(failure(HostFailureKind::ForeignKill), WorkerObservation::default());
        assert_eq!(typed_details(&busy)["retry_after_ms"], 1_000);
        let starved_setup = diff_error(
            failure(HostFailureKind::SetupTimeout),
            WorkerObservation {
                setup_cpu_us: Some(100_000),
                ..WorkerObservation::default()
            },
        );
        assert_eq!(typed_details(&starved_setup)["cause"], "setup_starved");
        let exhausted = diff_error(failure(HostFailureKind::SetupTimeout), WorkerObservation::default());
        assert_eq!(exhausted.kind(), ApiErrorKind::ServerRejected);
        assert_eq!(
            typed_details(&exhausted),
            serde_json::json!({"reason": "isolated_setup_exhausted"})
        );
        for kind in [
            HostFailureKind::Crash,
            HostFailureKind::Protocol,
            HostFailureKind::Spawn,
        ] {
            let defect = diff_error(failure(kind), WorkerObservation::default());
            assert_eq!(defect.kind(), ApiErrorKind::ServerRejected);
            assert_eq!(
                typed_details(&defect),
                serde_json::json!({"reason": "isolated_apply_failed"})
            );
        }
    }

    /// `limits-v1.md`'s `authorized_scan_rows_max` (1,000) exact boundary: a scan that examines
    /// exactly the ceiling's worth of candidate rows is still within budget — the ceiling is a
    /// "would need one more row" trigger, not an "at the ceiling" one.
    #[test]
    fn check_scan_budget_accepts_the_exact_authorized_scan_rows_max_boundary() {
        assert!(check_scan_budget(limits::AUTHORIZED_SCAN_ROWS_MAX).is_ok());
    }

    /// One candidate row past the ceiling is rejected as `limit_exceeded`/`scan_budget`, and both
    /// `limit` and `observed` in the response are the fixed ceiling itself — never the caller's
    /// true candidate count, which would tell them how many rows exist that they are not
    /// authorized to see (`limits-v1.md`: "不向 caller 返回过滤前 count"). This is asserted at
    /// `AUTHORIZED_SCAN_ROWS_MAX + 1` specifically (not some larger number) so the boundary itself
    /// — not just "eventually rejects" — is what is pinned.
    #[test]
    fn check_scan_budget_rejects_one_row_past_the_authorized_scan_rows_max_boundary() {
        let err =
            check_scan_budget(limits::AUTHORIZED_SCAN_ROWS_MAX + 1).expect_err("one row past the ceiling must reject");
        let ApiError::Typed { kind, details, .. } = err else {
            panic!("expected ApiError::Typed, got a differently-shaped ApiError");
        };
        assert_eq!(kind.stable_code(), ApiErrorKind::LimitExceeded.stable_code());
        let details = details.expect("limit_exceeded always carries structured details");
        assert_eq!(details["limit_kind"], "scan_budget");
        assert_eq!(details["limit"], limits::AUTHORIZED_SCAN_ROWS_MAX);
        assert_eq!(
            details["observed"],
            limits::AUTHORIZED_SCAN_ROWS_MAX,
            "observed must equal the fixed ceiling, not a real pre-filter row count -- that would \
             leak how many unauthorized rows exist beyond it"
        );
    }

    /// A scan budget one below the ceiling never trips, regardless of how many more candidates
    /// remain to be examined after it -- `examined` alone (not "examined so far this request minus
    /// something") is what the ceiling compares against.
    #[test]
    fn check_scan_budget_accepts_one_row_before_the_authorized_scan_rows_max_boundary() {
        assert!(check_scan_budget(limits::AUTHORIZED_SCAN_ROWS_MAX - 1).is_ok());
    }

    fn lag_item(lag: i64) -> ProjectionLagItem {
        ProjectionLagItem {
            object_id: Uuid::nil(),
            head_seq: lag,
            projection_seq: 0,
            lag,
        }
    }

    #[test]
    fn projection_lag_p95_is_nearest_rank_with_explicit_small_sample_behavior() {
        assert_eq!(projection_lag_aggregates(&[]), (0, 0));
        assert_eq!(projection_lag_aggregates(&[lag_item(7)]), (7, 7));
        assert_eq!(
            projection_lag_aggregates(&(0..19).map(lag_item).collect::<Vec<_>>()),
            (18, 18),
            "fewer than twenty samples use the maximum as nearest-rank p95"
        );

        let mut twenty: Vec<ProjectionLagItem> = (0..19).map(lag_item).collect();
        twenty.push(lag_item(10_000));
        assert_eq!(
            projection_lag_aggregates(&twenty),
            (10_000, 18),
            "at twenty samples the single largest outlier is above nearest-rank p95"
        );
    }
}
