//! The answers of a refused isolated attempt, end to end through `accept_update` and the REST
//! content command against a real database: `apply_busy` with `resend` and with `rebase`, the REST
//! rebuild and its cap, the divergence refusal, and the content answers that stay as they were.
//!
//! The refusals are scripted (`scripted_attempts`): each attempt on the registered document takes
//! the next entry, a refusal with a chosen observation of the worker or an acceptance applied in
//! this process. That is the host's side of a starved or non-sequential worker without the
//! Linux-only worker process, so these tests run on every platform that has the database.

#![allow(clippy::cast_possible_wrap, clippy::cast_possible_truncation)]

use axum::response::IntoResponse;
use collab_core::isolation::wire::WindowDecision;
use collab_core::isolation::{HostFailure, HostFailureKind, IsolatedApplyError, WindowMode, WorkerObservation};
use collab_core::window_check::{FallbackReason, WindowReport};
use sea_orm::{ConnectionTrait, DbBackend, FromQueryResult, Statement};
use serde_json::{Value, json};
use uuid::Uuid;

use super::{
    AppState, DocumentCoordinator, SessionRegistry, SnapshotAdvancer, TEST_DATABASE_URL_ENV, command_input,
    count_collab_updates, create_page, head_seq_of, scratch, seed_workspace, state_for,
};
use crate::error::{ApiError, ApiErrorKind, ServerDrainingReason};
use crate::flow::collab::authz;
use crate::flow::collab::cache::WarmCache;
use crate::flow::collab::frame::RejectedCode;
use crate::flow::collab::write::scripted_attempts::{self, Scripted};
use crate::flow::collab::write::{AcceptOutcome, RebasePolicy, UpdateRequest, accept_update, replay_stable_update_id};
use crate::flow::command::execute_command;

struct Fixture {
    /// These tests exercise the incremental window (experimental, off by default), so the thread
    /// that runs them has `[flow] incremental_window = true`.
    _incremental: Option<crate::config::test_support::IncrementalWindowOn>,
    state: AppState,
    workspace_id: Uuid,
    owner_id: Uuid,
    object_id: Uuid,
    document_id: Uuid,
}

async fn fixture(scratch: &super::Scratch) -> Fixture {
    fixture_with(scratch, Some(crate::config::test_support::enable_incremental_window())).await
}

/// A fixture on the default configuration (the incremental window off).
async fn default_fixture(scratch: &super::Scratch) -> Fixture {
    fixture_with(scratch, None).await
}

async fn fixture_with(
    scratch: &super::Scratch,
    incremental: Option<crate::config::test_support::IncrementalWindowOn>,
) -> Fixture {
    let state = state_for(scratch.db.clone());
    let (workspace_id, owner_id) = seed_workspace(&state).await;
    let (object_id, document_id) = create_page(&state, workspace_id, owner_id).await;
    Fixture {
        _incremental: incremental,
        state,
        workspace_id,
        owner_id,
        object_id,
        document_id,
    }
}

fn observed(cpu_us: Option<u64>, decision: Option<WindowDecision>) -> WorkerObservation {
    WorkerObservation {
        metered_wall_us: Some(100_000),
        metered_cpu_us: cpu_us,
        setup_cpu_us: None,
        decision,
        setup_guard: None,
    }
}

const NOT_SEQUENTIAL: WindowDecision = WindowDecision::Fallback(FallbackReason::NotSequential);

/// A CPU kill after the check decided the update was not sequential: row 1 of the classification.
fn needs_sequential() -> Scripted {
    Scripted::Refuse(
        IsolatedApplyError::CpuCeiling,
        observed(Some(52_000), Some(NOT_SEQUENTIAL)),
    )
}

/// A wall kill of a worker that had barely any CPU: row 2.
fn starved() -> Scripted {
    Scripted::Refuse(IsolatedApplyError::WallCeiling, observed(Some(3_000), None))
}

fn host_failure(kind: HostFailureKind, setup_cpu_us: Option<u64>) -> Scripted {
    Scripted::Refuse(
        IsolatedApplyError::HostFailure(HostFailure::new(kind, "scripted")),
        WorkerObservation {
            setup_cpu_us,
            ..WorkerObservation::default()
        },
    )
}

fn a_report_the_projection_contradicts() -> WindowReport {
    WindowReport {
        blocks: 9_999,
        navigators: 0,
        chars: 0,
        depth_bound: 1,
        longest_bound: 0,
    }
}

/// One real title update against the document's current head.
async fn a_title_update(f: &Fixture, title: &str) -> Vec<u8> {
    use collab_core::{CollabEngine, LoroCollabEngine};
    #[derive(FromQueryResult)]
    struct Row {
        snapshot: Vec<u8>,
    }
    let row = Row::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "SELECT snapshot FROM collab_documents WHERE id = $1",
        vec![f.document_id.into()],
    ))
    .one(&f.state.db)
    .await
    .expect("snapshot query runs")
    .expect("document row exists");
    let mut engine = LoroCollabEngine::load(&row.snapshot).expect("snapshot loads");
    let frontier = engine.frontier();
    engine.set_title(title).expect("title sets");
    engine.export_from(&frontier).expect("update exports")
}

async fn submit(f: &Fixture, update_id: Uuid, bytes: Vec<u8>, policy: RebasePolicy) -> AcceptOutcome {
    submit_raw(f, update_id, bytes, policy)
        .await
        .expect("accept does not error")
}

/// [`submit`] that hands back the error of the write when there is one.
async fn submit_raw(
    f: &Fixture,
    update_id: Uuid,
    bytes: Vec<u8>,
    policy: RebasePolicy,
) -> Result<AcceptOutcome, ApiError> {
    let epoch = authz::read_epoch(&f.state.db, f.workspace_id)
        .await
        .expect("epoch reads");
    accept_update(
        &f.state.db,
        &WarmCache::new(),
        &DocumentCoordinator::new(),
        &SessionRegistry::new(),
        &SnapshotAdvancer::new(),
        10,
        None,
        UpdateRequest {
            rebase_policy: policy,
            origin: crate::flow::event_origin::CommandOrigin::first_request_from(
                crate::flow::event_origin::EventSurface::Rest,
            ),
            document_id: f.document_id,
            update_id,
            bytes,
            idempotency_key: None,
            event_idempotency_key: None,
            origin_client_id: Some("busy-test".to_string()),
            message: None,
            actor_id: f.owner_id,
            actor_is_bot: false,
            workspace_id: f.workspace_id,
            checked_epoch: epoch,
            expected_frontier: None,
        },
    )
    .await
}

fn rejection(outcome: AcceptOutcome) -> crate::flow::collab::write::Rejected {
    match outcome {
        AcceptOutcome::Rejected(rejected) => rejected,
        AcceptOutcome::Accepted(accepted) => panic!("expected a rejection, got an acceptance at {}", accepted.head_seq),
    }
}

/// The `apply_busy` answer the client was built against, field by field.
fn assert_busy(rejected: &crate::flow::collab::write::Rejected, update_id: Uuid, mode: &str, after: u64, cause: &str) {
    assert_eq!(rejected.code, RejectedCode::ServerDraining);
    assert!(rejected.recoverable);
    assert_eq!(rejected.write_state, crate::flow::collab::frame::WriteState::NotApplied);
    assert_eq!(rejected.update_id, Some(update_id));
    assert_eq!(
        rejected.details,
        Some(json!({"reason": "apply_busy", "retry_after_ms": after, "retry_mode": mode, "cause": cause}))
    );
}

async fn count_events(state: &AppState, idempotency_key: &str) -> i64 {
    #[derive(FromQueryResult)]
    struct Row {
        n: i64,
    }
    Row::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "SELECT count(*) AS n FROM business_events WHERE idempotency_key = $1 AND event_type = 'flow.content.accepted'",
        vec![idempotency_key.into()],
    ))
    .one(&state.db)
    .await
    .expect("event count query runs")
    .expect("a count row")
    .n
}

async fn body_of(error: ApiError) -> Value {
    let response = error.into_response();
    let bytes = axum::body::to_bytes(response.into_body(), 1 << 20)
        .await
        .expect("the body reads");
    serde_json::from_slice(&bytes).expect("the body is JSON")
}

fn details_of(error: &ApiError) -> &Value {
    let ApiError::Typed {
        details: Some(details), ..
    } = error
    else {
        panic!("a typed error with details, got {error:?}");
    };
    details
}

// ---------------------------------------------------------------------------------------------
// accept_update: the answers
// ---------------------------------------------------------------------------------------------

/// Rows 1, 2, 6, 8, 9 as the write path answers them to a surface that can rebase (the WebSocket's
/// policy): the exact wire shape of the busy answer, and nothing written.
#[tokio::test]
async fn a_load_refusal_is_answered_apply_busy_with_its_retry_mode_and_nothing_is_written() {
    let scratch = scratch_or_skip!("busy-answers");
    let f = fixture(&scratch).await;
    let head_before = head_seq_of(&f.state, f.document_id).await;
    let cases = [
        (needs_sequential(), "rebase", 0, "needs_sequential"),
        (starved(), "resend", 250, "cpu_starved"),
        (
            host_failure(HostFailureKind::SetupTimeout, Some(400_000)),
            "resend",
            1_000,
            "setup_starved",
        ),
        (
            host_failure(HostFailureKind::SpawnResources, None),
            "resend",
            500,
            "worker_resources",
        ),
        (
            host_failure(HostFailureKind::ForeignKill, None),
            "resend",
            1_000,
            "worker_resources",
        ),
    ];
    for (scripted, mode, after, cause) in cases {
        scripted_attempts::enable(f.document_id, vec![scripted]);
        let update_id = Uuid::new_v4();
        let bytes = a_title_update(&f, &format!("busy {cause}")).await;
        let rejected = rejection(submit(&f, update_id, bytes, RebasePolicy::ClientRebases).await);
        assert_busy(&rejected, update_id, mode, after, cause);
        assert_eq!(count_collab_updates(&f.state, f.document_id, update_id).await, 0);
        assert_eq!(head_seq_of(&f.state, f.document_id).await, head_before, "{cause}");
        assert_eq!(
            scripted_attempts::modes(f.document_id),
            [WindowMode::Incremental],
            "{cause}: one attempt, no retry inside accept_update"
        );
    }
    scratch.drop_self().await;
}

/// The wire shape serialised: the frame the WebSocket sends for this rejection has exactly the
/// fields the client reads.
#[tokio::test]
async fn the_busy_frame_serialises_to_the_shape_the_client_was_built_against() {
    use crate::flow::collab::frame::{Frame, PROTOCOL_VERSION};

    let scratch = scratch_or_skip!("busy-frame-shape");
    let f = fixture(&scratch).await;
    scripted_attempts::enable(f.document_id, vec![needs_sequential()]);
    let update_id = Uuid::new_v4();
    let bytes = a_title_update(&f, "frame shape").await;
    let rejected = rejection(submit(&f, update_id, bytes, RebasePolicy::ClientRebases).await);
    let frame = Frame::Rejected {
        protocol_version: PROTOCOL_VERSION,
        document_id: f.document_id,
        update_id: rejected.update_id,
        code: rejected.code,
        recoverable: rejected.recoverable,
        write_state: rejected.write_state,
        details: rejected.details,
        current_seq: rejected.current_seq,
        current_frontier: None,
        audit_event_id: None,
    };
    let wire = serde_json::to_value(&frame).expect("the frame serialises");
    assert_eq!(wire["type"], "rejected");
    assert_eq!(wire["protocol_version"], 1);
    assert_eq!(wire["document_id"], f.document_id.to_string());
    assert_eq!(wire["update_id"], update_id.to_string());
    assert_eq!(wire["code"], "server_draining");
    assert_eq!(wire["recoverable"], true);
    assert_eq!(wire["write_state"], "not_applied");
    assert_eq!(
        wire["details"],
        json!({"reason": "apply_busy", "retry_after_ms": 0, "retry_mode": "rebase", "cause": "needs_sequential"})
    );
    scratch.drop_self().await;
}

/// A surface that cannot follow a `rebase` (every other caller of the write path) never gets one:
/// a refusal that needs a sequential update is the content answer it was before, and starvation is
/// a `resend`.
#[tokio::test]
async fn a_surface_that_cannot_rebase_gets_the_content_answer_and_resend_for_starvation() {
    let scratch = scratch_or_skip!("busy-no-rebase");
    let f = fixture(&scratch).await;

    scripted_attempts::enable(f.document_id, vec![needs_sequential()]);
    let rejected = rejection(
        submit(
            &f,
            Uuid::new_v4(),
            a_title_update(&f, "no rebase").await,
            RebasePolicy::None,
        )
        .await,
    );
    assert_eq!(rejected.code, RejectedCode::LimitExceeded);
    assert!(!rejected.recoverable);
    let details = rejected.details.expect("limit details");
    assert_eq!(details["limit_kind"], "decode_apply_cpu_ms");

    scripted_attempts::enable(f.document_id, vec![starved()]);
    let update_id = Uuid::new_v4();
    let rejected = rejection(
        submit(
            &f,
            update_id,
            a_title_update(&f, "no rebase, starved").await,
            RebasePolicy::None,
        )
        .await,
    );
    assert_busy(&rejected, update_id, "resend", 250, "cpu_starved");
    scratch.drop_self().await;
}

/// The content answers are unchanged: a document that really is over a limit gets the
/// non-retryable `limit_exceeded` with the frozen `limit_kind`, whatever the surface.
#[tokio::test]
async fn the_content_limits_are_unchanged_and_not_retryable() {
    let scratch = scratch_or_skip!("busy-content");
    let f = fixture(&scratch).await;
    let heavy_wall = Scripted::Refuse(IsolatedApplyError::WallCeiling, observed(Some(60_000), None));
    let cases = [
        // A CPU kill whose check never said NotSequential (facts absent: the setup guard fired).
        (
            Scripted::Refuse(
                IsolatedApplyError::CpuCeiling,
                observed(
                    Some(52_000),
                    Some(WindowDecision::Fallback(FallbackReason::FactsAbsent)),
                ),
            ),
            "decode_apply_cpu_ms",
        ),
        // Killed before the check decided: no decision record.
        (
            Scripted::Refuse(IsolatedApplyError::CpuCeiling, observed(Some(52_000), None)),
            "decode_apply_cpu_ms",
        ),
        (heavy_wall, "decode_apply_wall_ms"),
        (
            Scripted::Refuse(
                IsolatedApplyError::MemoryCeiling,
                observed(Some(10_000), Some(WindowDecision::Incremental)),
            ),
            "isolated_apply_memory_bytes",
        ),
    ];
    for (scripted, expected) in cases {
        scripted_attempts::enable(f.document_id, vec![scripted]);
        let rejected = rejection(
            submit(
                &f,
                Uuid::new_v4(),
                a_title_update(&f, expected).await,
                RebasePolicy::ClientRebases,
            )
            .await,
        );
        assert_eq!(rejected.code, RejectedCode::LimitExceeded, "{expected}");
        assert!(!rejected.recoverable, "{expected}");
        let details = rejected.details.expect("limit details");
        assert_eq!(details["limit_kind"], expected);
        assert_eq!(details["observed"], details["limit"].as_u64().expect("a limit") + 1);
    }
    scratch.drop_self().await;
}

/// Defects of the isolation host are `server_rejected` with a named reason, never `internal`.
#[tokio::test]
async fn a_defect_of_the_isolation_host_is_server_rejected_with_a_named_reason() {
    let scratch = scratch_or_skip!("busy-defects");
    let f = fixture(&scratch).await;
    let cases = [
        (host_failure(HostFailureKind::Crash, None), "isolated_apply_failed"),
        (host_failure(HostFailureKind::Protocol, None), "isolated_apply_failed"),
        (host_failure(HostFailureKind::Spawn, None), "isolated_apply_failed"),
    ];
    for (scripted, reason) in cases {
        scripted_attempts::enable(f.document_id, vec![scripted]);
        let update_id = Uuid::new_v4();
        let rejected = rejection(
            submit(
                &f,
                update_id,
                a_title_update(&f, reason).await,
                RebasePolicy::ClientRebases,
            )
            .await,
        );
        assert_eq!(rejected.code, RejectedCode::ServerRejected, "{reason}");
        assert!(!rejected.recoverable);
        assert_eq!(rejected.write_state, crate::flow::collab::frame::WriteState::NotApplied);
        assert_eq!(rejected.update_id, Some(update_id));
        assert_eq!(rejected.details, Some(json!({ "reason": reason })));
    }
    scratch.drop_self().await;
}

/// A busy answer is retried with the same `update_id` and commits once.
#[tokio::test]
async fn busy_then_resend_with_the_same_update_id_commits_once() {
    let scratch = scratch_or_skip!("busy-resend-once");
    let f = fixture(&scratch).await;
    let head_before = head_seq_of(&f.state, f.document_id).await;
    scripted_attempts::enable(f.document_id, vec![starved(), starved()]);
    let update_id = Uuid::new_v4();
    let bytes = a_title_update(&f, "resent").await;
    for attempt in 1..=2 {
        let rejected = rejection(submit(&f, update_id, bytes.clone(), RebasePolicy::ClientRebases).await);
        assert_busy(&rejected, update_id, "resend", 250, "cpu_starved");
        assert_eq!(
            count_collab_updates(&f.state, f.document_id, update_id).await,
            0,
            "{attempt}"
        );
    }
    // The script is used up: the third send is applied.
    let AcceptOutcome::Accepted(accepted) = submit(&f, update_id, bytes.clone(), RebasePolicy::ClientRebases).await
    else {
        panic!("the third send is accepted");
    };
    assert_eq!(accepted.head_seq, head_before + 1);
    // A fourth send of the same update answers the original receipt and writes nothing more.
    let AcceptOutcome::Accepted(again) = submit(&f, update_id, bytes, RebasePolicy::ClientRebases).await else {
        panic!("a resend of a committed update answers its receipt");
    };
    assert_eq!(again.head_seq, accepted.head_seq);
    assert_eq!(count_collab_updates(&f.state, f.document_id, update_id).await, 1);
    assert_eq!(head_seq_of(&f.state, f.document_id).await, head_before + 1);
    scratch.drop_self().await;
}

/// A divergence the host cannot decide again is `server_rejected{window_divergence}`, produced on
/// the first call without any retry inside `accept_update`.
#[tokio::test]
async fn a_divergence_is_server_rejected_and_does_not_retry_inside_accept_update() {
    let scratch = scratch_or_skip!("busy-divergence");
    let f = fixture(&scratch).await;
    scripted_attempts::enable(
        f.document_id,
        vec![
            Scripted::InProcessReporting(a_report_the_projection_contradicts()),
            Scripted::InProcessReporting(a_report_the_projection_contradicts()),
        ],
    );
    let update_id = Uuid::new_v4();
    let bytes = a_title_update(&f, "diverged").await;
    let epoch = authz::read_epoch(&f.state.db, f.workspace_id)
        .await
        .expect("epoch reads");
    let request = UpdateRequest {
        rebase_policy: RebasePolicy::ClientRebases,
        origin: crate::flow::event_origin::CommandOrigin::first_request_from(
            crate::flow::event_origin::EventSurface::Rest,
        ),
        document_id: f.document_id,
        update_id,
        bytes,
        idempotency_key: None,
        event_idempotency_key: None,
        origin_client_id: None,
        message: None,
        actor_id: f.owner_id,
        actor_is_bot: false,
        workspace_id: f.workspace_id,
        checked_epoch: epoch,
        expected_frontier: None,
    };
    let result = accept_update(
        &f.state.db,
        &WarmCache::new(),
        &DocumentCoordinator::new(),
        &SessionRegistry::new(),
        &SnapshotAdvancer::new(),
        10,
        None,
        request,
    )
    .await;
    let Err(error) = result else {
        panic!("a divergence is refused");
    };
    assert_eq!(error.kind(), ApiErrorKind::ServerRejected);
    assert_eq!(details_of(&error), &json!({"reason": "window_divergence"}));
    assert_eq!(
        scripted_attempts::modes(f.document_id),
        [WindowMode::Incremental, WindowMode::Full],
        "the incremental attempt and the one full retry, then no more"
    );
    assert_eq!(count_collab_updates(&f.state, f.document_id, update_id).await, 0);
    scratch.drop_self().await;
}

// ---------------------------------------------------------------------------------------------
// The REST content command
// ---------------------------------------------------------------------------------------------

fn insert_block(id: &str) -> Value {
    json!({"block_id": id, "index": 0, "text": format!("TEXT{id}")})
}

/// Row 1 on the REST surface: the server rebuilds the command from the head and submits it again,
/// under the same update id and key, and the command commits once: one `collab_updates` row, one
/// event.
#[tokio::test]
async fn the_rest_command_rebuilds_internally_after_a_rebase_answer_and_commits_once() {
    let scratch = scratch_or_skip!("busy-rest-rebuild");
    let f = fixture(&scratch).await;
    scripted_attempts::enable(f.document_id, vec![needs_sequential(), Scripted::InProcess]);
    let key = Uuid::new_v4().to_string();
    let change = execute_command(
        &f.state,
        command_input(f.object_id, f.owner_id, "insert_block", insert_block("one"), &key),
    )
    .await
    .expect("the command commits after one internal rebuild");
    assert_eq!(
        scripted_attempts::modes(f.document_id),
        [WindowMode::Incremental, WindowMode::Incremental],
        "one refused attempt and one rebuild"
    );
    let update_id = replay_stable_update_id(f.document_id, &key);
    assert_eq!(count_collab_updates(&f.state, f.document_id, update_id).await, 1);
    assert_eq!(count_events(&f.state, &key).await, 1);
    assert_eq!(head_seq_of(&f.state, f.document_id).await, change.accepted_seq);
    scratch.drop_self().await;
}

/// The cap: three attempts in all (two rebuilds), then the caller gets the busy answer, always
/// `resend` over REST, and nothing is written.
#[tokio::test]
async fn the_rest_rebuild_is_capped_and_the_caller_sees_apply_busy_resend() {
    let scratch = scratch_or_skip!("busy-rest-cap");
    let f = fixture(&scratch).await;
    let head_before = head_seq_of(&f.state, f.document_id).await;
    scripted_attempts::enable(
        f.document_id,
        vec![needs_sequential(), needs_sequential(), needs_sequential()],
    );
    let key = Uuid::new_v4().to_string();
    let Err(error) = execute_command(
        &f.state,
        command_input(f.object_id, f.owner_id, "insert_block", insert_block("capped"), &key),
    )
    .await
    else {
        panic!("three refusals in a row end in the busy answer");
    };
    assert_eq!(
        scripted_attempts::modes(f.document_id).len(),
        3,
        "two rebuilds, not more"
    );
    assert_eq!(
        error.kind(),
        ApiErrorKind::ServerDraining(ServerDrainingReason::ApplyBusy)
    );
    assert_eq!(
        details_of(&error),
        &json!({"retry_after_ms": 0, "retry_mode": "resend", "cause": "needs_sequential"})
    );
    let body = body_of(error).await;
    assert_eq!(body["code"], 409);
    assert_eq!(body["error_code"], "server_draining");
    assert_eq!(
        body["details"],
        json!({"reason": "apply_busy", "retry_after_ms": 0, "retry_mode": "resend", "cause": "needs_sequential"})
    );
    let update_id = replay_stable_update_id(f.document_id, &key);
    assert_eq!(count_collab_updates(&f.state, f.document_id, update_id).await, 0);
    assert_eq!(count_events(&f.state, &key).await, 0);
    assert_eq!(head_seq_of(&f.state, f.document_id).await, head_before);

    // The caller resends under the same key: the script is used up, so it commits, once.
    execute_command(
        &f.state,
        command_input(f.object_id, f.owner_id, "insert_block", insert_block("capped"), &key),
    )
    .await
    .expect("the resend commits");
    assert_eq!(count_collab_updates(&f.state, f.document_id, update_id).await, 1);
    assert_eq!(count_events(&f.state, &key).await, 1);
    scratch.drop_self().await;
}

/// Starvation over REST is not rebuilt inside the command (the same bytes would be sent again for
/// no reason): the caller gets `apply_busy` with `resend` at once.
#[tokio::test]
async fn a_starved_rest_command_is_not_rebuilt_and_answers_apply_busy_resend() {
    let scratch = scratch_or_skip!("busy-rest-starved");
    let f = fixture(&scratch).await;
    scripted_attempts::enable(f.document_id, vec![starved()]);
    let key = Uuid::new_v4().to_string();
    let Err(error) = execute_command(
        &f.state,
        command_input(f.object_id, f.owner_id, "insert_block", insert_block("starved"), &key),
    )
    .await
    else {
        panic!("a starved attempt is refused");
    };
    assert_eq!(scripted_attempts::modes(f.document_id).len(), 1);
    assert_eq!(
        details_of(&error),
        &json!({"retry_after_ms": 250, "retry_mode": "resend", "cause": "cpu_starved"})
    );
    let body = body_of(error).await;
    assert_eq!(body["details"]["reason"], "apply_busy");
    scratch.drop_self().await;
}

/// The content answers over REST: unchanged `limit_exceeded`, with the frozen `limit_kind`.
#[tokio::test]
async fn the_rest_content_limits_are_unchanged() {
    let scratch = scratch_or_skip!("busy-rest-content");
    let f = fixture(&scratch).await;
    scripted_attempts::enable(
        f.document_id,
        vec![Scripted::Refuse(
            IsolatedApplyError::CpuCeiling,
            observed(Some(52_000), Some(WindowDecision::Incremental)),
        )],
    );
    let Err(error) = execute_command(
        &f.state,
        command_input(
            f.object_id,
            f.owner_id,
            "insert_block",
            insert_block("heavy"),
            &Uuid::new_v4().to_string(),
        ),
    )
    .await
    else {
        panic!("a CPU kill is refused");
    };
    assert_eq!(error.kind(), ApiErrorKind::LimitExceeded);
    assert_eq!(details_of(&error)["limit_kind"], "decode_apply_cpu_ms");
    assert_eq!(
        scripted_attempts::modes(f.document_id).len(),
        1,
        "no rebuild for a content limit"
    );
    scratch.drop_self().await;
}

/// A divergence over REST: `server_rejected{window_divergence}`, 500 envelope, no rebuild.
#[tokio::test]
async fn a_rest_divergence_is_server_rejected_window_divergence() {
    let scratch = scratch_or_skip!("busy-rest-divergence");
    let f = fixture(&scratch).await;
    scripted_attempts::enable(
        f.document_id,
        vec![
            Scripted::InProcessReporting(a_report_the_projection_contradicts()),
            Scripted::InProcessReporting(a_report_the_projection_contradicts()),
        ],
    );
    let Err(error) = execute_command(
        &f.state,
        command_input(
            f.object_id,
            f.owner_id,
            "insert_block",
            insert_block("diverged"),
            &Uuid::new_v4().to_string(),
        ),
    )
    .await
    else {
        panic!("a divergence is refused");
    };
    assert_eq!(error.kind(), ApiErrorKind::ServerRejected);
    assert_eq!(details_of(&error), &json!({"reason": "window_divergence"}));
    assert_eq!(scripted_attempts::modes(f.document_id).len(), 2);
    let body = body_of(error).await;
    assert_eq!(body["error_code"], "server_rejected");
    assert_eq!(body["code"], 500);
    scratch.drop_self().await;
}

// ---------------------------------------------------------------------------------------------
// A-1: the backstop of the setup guards
// ---------------------------------------------------------------------------------------------

/// A setup that timed out having used its CPU budget.
fn setup_exhausted(setup_cpu_us: Option<u64>) -> Scripted {
    host_failure(HostFailureKind::SetupTimeout, setup_cpu_us)
}

/// A CPU kill of the full window, after the setup guard had fired (no facts).
fn full_window_cpu_kill() -> Scripted {
    Scripted::Refuse(
        IsolatedApplyError::CpuCeiling,
        WorkerObservation {
            setup_guard: Some(collab_core::setup_guard::Fired::Text),
            ..observed(
                Some(52_000),
                Some(WindowDecision::Fallback(FallbackReason::FactsAbsent)),
            )
        },
    )
}

/// The review's end state, no longer permanent: a setup that timed out with its CPU used does not
/// answer `server_rejected{isolated_setup_exhausted}` on every write. The write is decided again with
/// the full window (here: accepted), the document is on the full window for the following writes,
/// and one attempt each is all they cost.
#[tokio::test]
async fn a_setup_that_used_its_cpu_budget_is_decided_by_the_full_window_and_the_document_cools() {
    for (cpu, label) in [
        (Some(4_900_000), "high"),
        (Some(2_000_000), "at 80 percent"),
        (None, "unknown"),
    ] {
        let scratch = scratch_or_skip!("backstop-accepted");
        let f = fixture(&scratch).await;
        scripted_attempts::enable(f.document_id, vec![setup_exhausted(cpu), Scripted::InProcess]);
        let outcome = submit(
            &f,
            Uuid::new_v4(),
            a_title_update(&f, "backstop").await,
            RebasePolicy::ClientRebases,
        )
        .await;
        assert!(
            matches!(outcome, AcceptOutcome::Accepted(_)),
            "{label}: accepted by the full window"
        );
        assert_eq!(
            scripted_attempts::modes(f.document_id),
            [WindowMode::Incremental, WindowMode::Full],
            "{label}: the second attempt is the full window"
        );
        // The next writes take the full window directly.
        for round in 0..2 {
            let outcome = submit(
                &f,
                Uuid::new_v4(),
                a_title_update(&f, &format!("after the backstop {round}")).await,
                RebasePolicy::ClientRebases,
            )
            .await;
            assert!(matches!(outcome, AcceptOutcome::Accepted(_)), "{label}: write {round}");
        }
        assert_eq!(
            scripted_attempts::modes(f.document_id),
            [
                WindowMode::Incremental,
                WindowMode::Full,
                WindowMode::Full,
                WindowMode::Full
            ],
            "{label}: one full attempt per write, no incremental setup"
        );
        scratch.drop_self().await;
    }
}

/// When the full window refuses as well the write gets the answer the full window produces: the
/// content limit, not the defect and not a server error. Only a full window that times out in its
/// own setup (the load alone) is the defect.
#[tokio::test]
async fn when_the_full_window_refuses_after_the_backstop_the_answer_is_its_own() {
    let scratch = scratch_or_skip!("backstop-refused");
    let f = fixture(&scratch).await;
    let head_before = head_seq_of(&f.state, f.document_id).await;

    scripted_attempts::enable(
        f.document_id,
        vec![setup_exhausted(Some(4_900_000)), full_window_cpu_kill()],
    );
    let update_id = Uuid::new_v4();
    let rejected = rejection(
        submit(
            &f,
            update_id,
            a_title_update(&f, "limit").await,
            RebasePolicy::ClientRebases,
        )
        .await,
    );
    assert_eq!(rejected.code, RejectedCode::LimitExceeded);
    assert!(!rejected.recoverable);
    assert_eq!(rejected.write_state, crate::flow::collab::frame::WriteState::NotApplied);
    assert_eq!(rejected.update_id, Some(update_id));
    assert_eq!(
        rejected.details.expect("limit details")["limit_kind"],
        "decode_apply_cpu_ms"
    );
    assert_eq!(
        scripted_attempts::modes(f.document_id),
        [WindowMode::Incremental, WindowMode::Full]
    );
    assert_eq!(head_seq_of(&f.state, f.document_id).await, head_before);
    scratch.drop_self().await;

    // The full window timing out in its own setup is the defect: nothing is left to try.
    let scratch = scratch_or_skip!("backstop-both-time-out");
    let f = fixture(&scratch).await;
    scripted_attempts::enable(
        f.document_id,
        vec![setup_exhausted(Some(4_900_000)), setup_exhausted(Some(4_900_000))],
    );
    let rejected = rejection(
        submit(
            &f,
            Uuid::new_v4(),
            a_title_update(&f, "defect").await,
            RebasePolicy::ClientRebases,
        )
        .await,
    );
    assert_eq!(rejected.code, RejectedCode::ServerRejected);
    assert_eq!(rejected.details, Some(json!({ "reason": "isolated_setup_exhausted" })));
    scratch.drop_self().await;
}

/// A starved setup is not the backstop: it is the busy answer, one attempt, no cooldown.
#[tokio::test]
async fn a_starved_setup_is_not_the_backstop() {
    let scratch = scratch_or_skip!("backstop-starved");
    let f = fixture(&scratch).await;
    scripted_attempts::enable(f.document_id, vec![setup_exhausted(Some(1_999_999))]);
    let update_id = Uuid::new_v4();
    let rejected = rejection(
        submit(
            &f,
            update_id,
            a_title_update(&f, "starved").await,
            RebasePolicy::ClientRebases,
        )
        .await,
    );
    assert_busy(&rejected, update_id, "resend", 1_000, "setup_starved");
    assert_eq!(scripted_attempts::modes(f.document_id), [WindowMode::Incremental]);
    scratch.drop_self().await;
}

/// A document whose accepted state cannot be read back is `server_rejected{document_base_unreadable}`
/// on every write, not an internal error that the WebSocket reports as the retryable `contention`.
#[tokio::test]
async fn a_base_that_does_not_load_is_server_rejected_with_a_stable_reason() {
    let scratch = scratch_or_skip!("base-unreadable");
    let f = fixture(&scratch).await;
    let update = a_title_update(&f, "before the damage").await;
    let garbage = b"this is not a loro snapshot".to_vec();
    f.state
        .db
        .execute(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "UPDATE collab_documents SET snapshot = $1, snapshot_checksum = $2 WHERE id = $3",
            vec![
                garbage.clone().into(),
                crate::flow::collab::write::content_hash(&garbage).into(),
                f.document_id.into(),
            ],
        ))
        .await
        .expect("the snapshot is replaced");
    for round in 0..2 {
        let error = submit_raw(&f, Uuid::new_v4(), update.clone(), RebasePolicy::ClientRebases)
            .await
            .err()
            .expect("a base that does not load is an error of the write");
        assert_eq!(error.kind(), ApiErrorKind::ServerRejected, "attempt {round}");
        assert_eq!(details_of(&error), &json!({ "reason": "document_base_unreadable" }));
    }
    scratch.drop_self().await;
}

/// A panic on the blocking thread that runs the window is `server_rejected{isolated_apply_failed}`:
/// deterministic for the base and the update, so never the retryable `contention`.
#[tokio::test]
async fn a_panic_in_the_window_task_is_server_rejected_and_not_contention() {
    let scratch = scratch_or_skip!("window-panic");
    let f = fixture(&scratch).await;
    scripted_attempts::enable(f.document_id, vec![Scripted::Panic]);
    let bytes = a_title_update(&f, "panic").await;
    let error = submit_raw(&f, Uuid::new_v4(), bytes, RebasePolicy::ClientRebases)
        .await
        .err()
        .expect("a panic of the window task is an error of the write");
    assert_eq!(error.kind(), ApiErrorKind::ServerRejected);
    assert_eq!(details_of(&error), &json!({ "reason": "isolated_apply_failed" }));
    scratch.drop_self().await;
}

// ---------------------------------------------------------------------------------------------
// The default configuration: the incremental window is off
// ---------------------------------------------------------------------------------------------

/// With the default configuration (`[flow] incremental_window` unset) the write takes the full
/// window only, which is what the worker did before the incremental window existed: one attempt in
/// `WindowMode::Full`, no facts, so a refusal that needs a sequential update is the content answer
/// and nothing is ever asked to rebase.
#[tokio::test]
async fn with_the_default_configuration_every_write_takes_the_full_window() {
    assert!(!crate::config::incremental_window(), "the default configuration");
    assert_eq!(super::super::configured_window_mode(), WindowMode::Full);
    let scratch = scratch_or_skip!("default-full-window");
    let f = default_fixture(&scratch).await;
    scripted_attempts::enable(f.document_id, vec![Scripted::InProcess, needs_sequential()]);
    let outcome = submit(
        &f,
        Uuid::new_v4(),
        a_title_update(&f, "default").await,
        RebasePolicy::ClientRebases,
    )
    .await;
    assert!(matches!(outcome, AcceptOutcome::Accepted(_)));
    // A CPU kill whose decision record says "not sequential" cannot happen in the full mode, and
    // if it did the answer would be the content limit, never a rebase.
    let rejected = rejection(
        submit(
            &f,
            Uuid::new_v4(),
            a_title_update(&f, "default two").await,
            RebasePolicy::ClientRebases,
        )
        .await,
    );
    assert_eq!(rejected.code, RejectedCode::LimitExceeded);
    assert_eq!(
        scripted_attempts::modes(f.document_id),
        [WindowMode::Full, WindowMode::Full]
    );
    scratch.drop_self().await;
}
