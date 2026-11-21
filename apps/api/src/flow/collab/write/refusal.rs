//! How a refused isolated attempt is answered: the pure classifier from what the worker host
//! observed to one of a few answers, and the counters and the log line that let an operator tell a
//! starved host from a document that is really over a limit.
//!
//! The classification is the table of `FLOW-RETRYABLE-BUSY-DESIGN.md` section 3:
//!
//! | Case | Answer |
//! |---|---|
//! | any ceiling on an incremental attempt whose check said `NotSequential` | busy, `rebase` |
//! | wall kill with the worker having used under 80 % of the CPU ceiling | busy, `resend` |
//! | wall kill otherwise, CPU kill, memory kill | the content limit, unchanged, not retryable |
//! | setup timeout after under 80 % of the setup CPU budget | busy, `resend` |
//! | setup timeout otherwise | defect, `server_rejected` (only reached when the full window was tried and timed out too, see `write::decide_window`) |
//! | spawn without resources, a SIGKILL the host did not send | busy, `resend` |
//! | crash, bad frame, other spawn failure | defect, `server_rejected` |
//!
//! A refusal that says "busy" never says that the content is fine for good: it says that this
//! attempt could not be decided for a load reason and nothing was applied. A document that really
//! exceeds a limit keeps the non-retryable content answer.
//!
//! No I/O happens here: [`classify`] reads its arguments and nothing else, so every row of the
//! table has a unit test (below).

use std::sync::atomic::{AtomicU64, Ordering};

use collab_core::CollabError;
use collab_core::isolation::wire::WindowDecision;
use collab_core::isolation::{
    DECODE_APPLY_CPU_MS_MAX, DECODE_APPLY_WALL_MS_MAX, HostFailureKind, ISOLATED_APPLY_MEMORY_BYTES_MAX,
    IsolatedApplyError, WindowMode, WorkerObservation,
};
use collab_core::window_check::FallbackReason;
use uuid::Uuid;

use crate::flow::collab::frame::{
    SERVER_REJECTED_REASON_ISOLATED_APPLY_FAILED, SERVER_REJECTED_REASON_SETUP_EXHAUSTED,
};

/// A wall kill of a worker that had used less CPU than this was starved, not heavy: 80 % of the
/// `decode_apply_cpu_ms_max` ceiling, in microseconds. A worker over it had the CPU and the job was
/// big, which is the content answer.
pub const STARVED_CPU_MAX_US: u64 = DECODE_APPLY_CPU_MS_MAX * 1_000 * 4 / 5;

/// The share of the setup CPU budget (`[flow] setup_cpu_budget_ms`), in percent, below which a setup
/// that timed out by wall clock was starved and not heavy.
///
/// The threshold is not the budget itself: the setup guard lets a setup run to its budget plus one
/// text (and the load), so a heavy honest page whose setup reaches the guard consumes the whole
/// budget, and a worker that was given half a core for it exceeds the wall timeout with CPU a little
/// over the budget. Only a setup that consumed less than 80 % of the budget at the timeout cannot
/// have been stopped by the guard, so it was the host that did not schedule it.
pub const SETUP_STARVED_BUDGET_PERCENT: u64 = 80;

/// The CPU, in microseconds, under which a setup timeout is starvation, for a setup CPU budget of
/// `setup_cpu_budget_ms`.
#[must_use]
pub const fn setup_starved_below_us(setup_cpu_budget_ms: u64) -> u64 {
    setup_cpu_budget_ms * 1_000 * SETUP_STARVED_BUDGET_PERCENT / 100
}

/// Whether a refused attempt is a setup that timed out having consumed its CPU budget (or an
/// unknown amount of CPU, which is never taken for starvation): the setup is unbounded for this
/// base, which the host answers by deciding the write with the full window (no facts computed) and
/// putting the document on the full window for a while (`write::decide_window`).
#[must_use]
pub fn is_setup_exhausted(err: &IsolatedApplyError, observation: &WorkerObservation, setup_cpu_budget_ms: u64) -> bool {
    matches!(
        err,
        IsolatedApplyError::HostFailure(failure) if failure.kind == HostFailureKind::SetupTimeout
    ) && observation
        .setup_cpu_us
        .is_none_or(|cpu_us| cpu_us >= setup_starved_below_us(setup_cpu_budget_ms))
}

/// `retry_after_ms` of each busy answer: a lower bound on the next attempt.
const RETRY_AFTER_NEEDS_SEQUENTIAL_MS: u64 = 0;
const RETRY_AFTER_CPU_STARVED_MS: u64 = 250;
const RETRY_AFTER_SPAWN_RESOURCES_MS: u64 = 500;
const RETRY_AFTER_FOREIGN_KILL_MS: u64 = 1_000;
const RETRY_AFTER_SETUP_STARVED_MS: u64 = 1_000;

/// Why a write was refused as busy; the diagnostic `details.cause` of the answer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BusyCause {
    /// The worker did not get the CPU it needed inside the wall deadline.
    CpuStarved,
    /// The update is concurrent with the head, so the incremental check could not decide it
    /// cheaply; the same edit sent as a sequential update could.
    NeedsSequential,
    /// The worker's setup was starved of CPU and did not finish in time.
    SetupStarved,
    /// The host is short of resources (could not spawn the worker, or killed it).
    WorkerResources,
}

impl BusyCause {
    pub const fn wire_value(self) -> &'static str {
        match self {
            Self::CpuStarved => "cpu_starved",
            Self::NeedsSequential => "needs_sequential",
            Self::SetupStarved => "setup_starved",
            Self::WorkerResources => "worker_resources",
        }
    }
}

/// What the caller does with a busy answer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RetryMode {
    /// Send the same update again, under the same `update_id`.
    Resend,
    /// Synchronise to the head, re-encode the unsent local edits as one new sequential update and
    /// send that.
    Rebase,
}

impl RetryMode {
    pub const fn wire_value(self) -> &'static str {
        match self {
            Self::Resend => "resend",
            Self::Rebase => "rebase",
        }
    }
}

/// What is known of the surface that submitted the update: whether its next attempt can be a
/// sequential one. `rebase` is promised only to a surface that can keep it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum RebasePolicy {
    /// The client re-encodes its unsent edits on `rebase` (the WebSocket `update` frame).
    ClientRebases,
    /// The server rebuilds the update from the head itself and submits it again (the REST content
    /// command).
    ServerRebuilds,
    /// A resend of this write is the same bytes against whatever the head is: a `rebase` could not
    /// be followed, so the caller gets the content answer where a sequential update would have been
    /// needed. Every other caller of the write path.
    #[default]
    None,
}

impl RebasePolicy {
    const fn can_rebase(self) -> bool {
        matches!(self, Self::ClientRebases | Self::ServerRebuilds)
    }
}

/// The answer to one refused isolated attempt.
#[derive(Debug)]
pub enum Refusal {
    /// `server_draining{apply_busy}`: refused for a load reason, nothing applied, retry.
    Busy {
        cause: BusyCause,
        mode: RetryMode,
        retry_after_ms: u64,
    },
    /// `limit_exceeded`: a resource ceiling the content reached, not retryable.
    Content { limit_kind: &'static str, limit: u64 },
    /// `server_rejected{reason}`: a defect, not retryable.
    Defect { reason: &'static str },
    /// The worker's own in-band rejection, built exactly as a direct in-process call would.
    Collab(CollabError),
    /// This build has no isolation boundary (`ADR-0014`).
    Unsupported { os: &'static str },
}

/// The deliberate faults of the classifier, for the mutant tests.
#[cfg(test)]
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum RefusalMutant {
    /// A CPU-ceiling kill is answered as busy: a document that really needs more than 50 ms of CPU
    /// would be retried until a client cap and never get its content answer.
    CpuKillIsBusy,
}

#[cfg(test)]
thread_local! {
    pub static REFUSAL_MUTANT: std::cell::Cell<Option<RefusalMutant>> = const { std::cell::Cell::new(None) };
}

#[cfg(test)]
fn mutant_is(mutant: RefusalMutant) -> bool {
    REFUSAL_MUTANT.with(std::cell::Cell::get) == Some(mutant)
}

const fn content_cpu() -> Refusal {
    Refusal::Content {
        limit_kind: "decode_apply_cpu_ms",
        limit: DECODE_APPLY_CPU_MS_MAX,
    }
}

const fn busy(cause: BusyCause, mode: RetryMode, retry_after_ms: u64) -> Refusal {
    Refusal::Busy {
        cause,
        mode,
        retry_after_ms,
    }
}

/// Classifies one refused isolated attempt.
///
/// `requested` is the mode the host asked the worker for in that attempt: [`WindowMode::Incremental`]
/// only when `[flow] incremental_window` is on and the document was not cooling, so a first attempt
/// on a cooling document, the full-window retry of a divergence and every attempt with the switch
/// off are [`WindowMode::Full`] and never get `rebase`.
///
/// `rebase` is answered if and only if all of these hold, which is what prevents a retry loop:
/// the attempt was requested in the incremental mode, the worker's decision record says the check
/// fell back for `NotSequential` (which the check tests only after the facts exist for this exact
/// base and the import was causally complete, so concurrency is the only reason), the refusal is a
/// resource ceiling, and the surface can follow a `rebase` (`policy`). A rebased update depends on
/// the whole head, so unless another writer commits in between it is sequential and its own refusal
/// cannot be `NotSequential` again.
///
/// `setup_cpu_budget_ms` is the configured CPU budget of the worker's setup; a setup timeout below
/// 80 % of it is starvation ([`setup_starved_below_us`]).
pub fn classify(
    err: IsolatedApplyError,
    observation: &WorkerObservation,
    requested: WindowMode,
    policy: RebasePolicy,
    setup_cpu_budget_ms: u64,
) -> Refusal {
    let needs_sequential = requested == WindowMode::Incremental
        && policy.can_rebase()
        && observation.decision == Some(WindowDecision::Fallback(FallbackReason::NotSequential));
    let rebase = || {
        busy(
            BusyCause::NeedsSequential,
            RetryMode::Rebase,
            RETRY_AFTER_NEEDS_SEQUENTIAL_MS,
        )
    };
    match err {
        IsolatedApplyError::Collab(err) => Refusal::Collab(err),
        IsolatedApplyError::UnsupportedPlatform { os } => Refusal::Unsupported { os },
        IsolatedApplyError::CpuCeiling | IsolatedApplyError::WallCeiling | IsolatedApplyError::MemoryCeiling
            if needs_sequential =>
        {
            rebase()
        }
        IsolatedApplyError::WallCeiling => match observation.metered_cpu_us {
            Some(cpu_us) if cpu_us < STARVED_CPU_MAX_US => {
                busy(BusyCause::CpuStarved, RetryMode::Resend, RETRY_AFTER_CPU_STARVED_MS)
            }
            // The worker had the CPU, or the kernel did not say: the job is heavy.
            _ => Refusal::Content {
                limit_kind: "decode_apply_wall_ms",
                limit: DECODE_APPLY_WALL_MS_MAX,
            },
        },
        IsolatedApplyError::CpuCeiling => {
            #[cfg(test)]
            if mutant_is(RefusalMutant::CpuKillIsBusy) {
                return busy(BusyCause::CpuStarved, RetryMode::Resend, RETRY_AFTER_CPU_STARVED_MS);
            }
            content_cpu()
        }
        IsolatedApplyError::MemoryCeiling => Refusal::Content {
            limit_kind: "isolated_apply_memory_bytes",
            limit: ISOLATED_APPLY_MEMORY_BYTES_MAX,
        },
        IsolatedApplyError::HostFailure(failure) => match failure.kind {
            HostFailureKind::SetupTimeout => match observation.setup_cpu_us {
                Some(cpu_us) if cpu_us < setup_starved_below_us(setup_cpu_budget_ms) => {
                    busy(BusyCause::SetupStarved, RetryMode::Resend, RETRY_AFTER_SETUP_STARVED_MS)
                }
                // A setup that burned its time as CPU is not bounded for this base: a defect once
                // the full window was tried as well (a setup that timed out without facts is the
                // load alone). An unknown figure is never taken for starvation.
                _ => Refusal::Defect {
                    reason: SERVER_REJECTED_REASON_SETUP_EXHAUSTED,
                },
            },
            HostFailureKind::SpawnResources => busy(
                BusyCause::WorkerResources,
                RetryMode::Resend,
                RETRY_AFTER_SPAWN_RESOURCES_MS,
            ),
            HostFailureKind::ForeignKill => busy(
                BusyCause::WorkerResources,
                RetryMode::Resend,
                RETRY_AFTER_FOREIGN_KILL_MS,
            ),
            HostFailureKind::Spawn | HostFailureKind::Crash | HostFailureKind::Protocol => Refusal::Defect {
                reason: SERVER_REJECTED_REASON_ISOLATED_APPLY_FAILED,
            },
        },
    }
}

// ---- Counters and the log line ----
//
// The API has no metrics endpoint; the counters are process-wide and their running totals ride on
// the log line (the way `WINDOW_CHECK_DIVERGENCES` does), so a log pipeline can chart them.

static BUSY_CPU_STARVED: AtomicU64 = AtomicU64::new(0);
static BUSY_NEEDS_SEQUENTIAL: AtomicU64 = AtomicU64::new(0);
static BUSY_SETUP_STARVED: AtomicU64 = AtomicU64::new(0);
static BUSY_WORKER_RESOURCES: AtomicU64 = AtomicU64::new(0);
static LIMIT_CPU: AtomicU64 = AtomicU64::new(0);
static LIMIT_WALL: AtomicU64 = AtomicU64::new(0);
static LIMIT_MEMORY: AtomicU64 = AtomicU64::new(0);
static DEFECT_SETUP_EXHAUSTED: AtomicU64 = AtomicU64::new(0);
static DEFECT_APPLY_FAILED: AtomicU64 = AtomicU64::new(0);

/// `flow_apply_busy_total{cause}`.
#[cfg(test)]
pub fn busy_total(cause: BusyCause) -> u64 {
    busy_counter(cause).load(Ordering::Relaxed)
}

/// `flow_apply_limit_total{kind}`, by the `limit_kind` of the content answer.
#[cfg(test)]
pub fn limit_total(limit_kind: &str) -> u64 {
    limit_counter(limit_kind).map_or(0, |counter| counter.load(Ordering::Relaxed))
}

/// `flow_apply_defect_total{reason}`.
#[cfg(test)]
pub fn defect_total(reason: &str) -> u64 {
    defect_counter(reason).map_or(0, |counter| counter.load(Ordering::Relaxed))
}

const fn busy_counter(cause: BusyCause) -> &'static AtomicU64 {
    match cause {
        BusyCause::CpuStarved => &BUSY_CPU_STARVED,
        BusyCause::NeedsSequential => &BUSY_NEEDS_SEQUENTIAL,
        BusyCause::SetupStarved => &BUSY_SETUP_STARVED,
        BusyCause::WorkerResources => &BUSY_WORKER_RESOURCES,
    }
}

fn limit_counter(limit_kind: &str) -> Option<&'static AtomicU64> {
    match limit_kind {
        "decode_apply_cpu_ms" => Some(&LIMIT_CPU),
        "decode_apply_wall_ms" => Some(&LIMIT_WALL),
        "isolated_apply_memory_bytes" => Some(&LIMIT_MEMORY),
        _ => None,
    }
}

fn defect_counter(reason: &str) -> Option<&'static AtomicU64> {
    match reason {
        SERVER_REJECTED_REASON_SETUP_EXHAUSTED => Some(&DEFECT_SETUP_EXHAUSTED),
        SERVER_REJECTED_REASON_ISOLATED_APPLY_FAILED => Some(&DEFECT_APPLY_FAILED),
        _ => None,
    }
}

/// What stopped the worker, for the log line.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Stopped {
    /// `cpu`, `wall`, `memory`, `setup`, `host` or `worker` (the worker's own in-band rejection).
    pub ceiling: &'static str,
    pub host_failure_kind: Option<HostFailureKind>,
}

impl Stopped {
    pub const fn of(err: &IsolatedApplyError) -> Self {
        let (ceiling, host_failure_kind) = match err {
            IsolatedApplyError::CpuCeiling => ("cpu", None),
            IsolatedApplyError::WallCeiling => ("wall", None),
            IsolatedApplyError::MemoryCeiling => ("memory", None),
            IsolatedApplyError::HostFailure(failure) => (
                match failure.kind {
                    HostFailureKind::SetupTimeout => "setup",
                    _ => "host",
                },
                Some(failure.kind),
            ),
            IsolatedApplyError::Collab(_) => ("worker", None),
            IsolatedApplyError::UnsupportedPlatform { .. } => ("unsupported", None),
        };
        Self {
            ceiling,
            host_failure_kind,
        }
    }
}

/// The identity of one refused attempt, for the log line.
pub struct AttemptLog<'a> {
    pub document_id: Uuid,
    /// `None` for an attempt that is not a write (the history diff).
    pub update_id: Option<Uuid>,
    /// `None` for an attempt that is not a write.
    pub base_head_seq: Option<i64>,
    pub requested: WindowMode,
    pub cooling: bool,
    pub observation: &'a WorkerObservation,
}

/// Counts the refusal and writes its one `flow::apply_refusal` line: `warn` for busy and content,
/// `error` for a defect. Never any content bytes. Returns the running total of the counter just
/// incremented (`None` for a refusal that has no counter: in-band rejections, an unsupported
/// platform). The worker's in-band rejections and an unsupported platform write no line here: the
/// first is the ordinary content check, the second is logged where it is mapped.
pub fn record(attempt: &AttemptLog<'_>, stopped: Stopped, refusal: &Refusal) -> Option<u64> {
    let (classification, answer, total) = match refusal {
        Refusal::Busy { cause, mode, .. } => (
            "busy",
            format!(
                "server_draining/apply_busy/{}/{}",
                cause.wire_value(),
                mode.wire_value()
            ),
            busy_counter(*cause).fetch_add(1, Ordering::Relaxed) + 1,
        ),
        Refusal::Content { limit_kind, .. } => (
            "content",
            format!("limit_exceeded/{limit_kind}"),
            limit_counter(limit_kind).map_or(0, |counter| counter.fetch_add(1, Ordering::Relaxed) + 1),
        ),
        Refusal::Defect { reason } => (
            "defect",
            format!("server_rejected/{reason}"),
            defect_counter(reason).map_or(0, |counter| counter.fetch_add(1, Ordering::Relaxed) + 1),
        ),
        Refusal::Collab(_) | Refusal::Unsupported { .. } => return None,
    };
    let decision = attempt
        .observation
        .decision
        .map_or_else(|| "none".to_owned(), WindowDecision::label);
    let host_failure_kind = stopped.host_failure_kind.map(|kind| format!("{kind:?}"));
    let setup_guard = attempt.observation.setup_guard.map(|fired| fired.name().to_owned());
    let requested_mode = match attempt.requested {
        WindowMode::Incremental => "incremental",
        WindowMode::Full => "full",
    };
    macro_rules! line {
        ($level:ident) => {
            tracing::$level!(
                target: "flow::apply_refusal",
                document_id = %attempt.document_id,
                update_id = attempt.update_id.map(|id| id.to_string()),
                base_head_seq = attempt.base_head_seq,
                ceiling = stopped.ceiling,
                host_failure_kind = host_failure_kind.as_deref(),
                requested_mode,
                cooling = attempt.cooling,
                decision = decision.as_str(),
                metered_cpu_us = attempt.observation.metered_cpu_us,
                metered_wall_us = attempt.observation.metered_wall_us,
                setup_cpu_us = attempt.observation.setup_cpu_us,
                setup_guard = setup_guard.as_deref(),
                classification,
                answer = answer.as_str(),
                total,
                "isolated_apply_refused"
            )
        };
    }
    if classification == "defect" {
        line!(error);
    } else {
        line!(warn);
    }
    Some(total)
}

#[cfg(test)]
mod tests {
    use super::*;
    use collab_core::isolation::HostFailure;

    fn observed(cpu_us: Option<u64>, decision: Option<WindowDecision>) -> WorkerObservation {
        WorkerObservation {
            metered_wall_us: Some(100_000),
            metered_cpu_us: cpu_us,
            setup_cpu_us: None,
            decision,
            setup_guard: None,
        }
    }

    /// The setup CPU budget of the tests: the default.
    const BUDGET_MS: u64 = 2_500;

    /// [`super::classify`] under the default setup CPU budget.
    fn classify(
        err: IsolatedApplyError,
        observation: &WorkerObservation,
        requested: WindowMode,
        policy: RebasePolicy,
    ) -> Refusal {
        super::classify(err, observation, requested, policy, BUDGET_MS)
    }

    const NOT_SEQUENTIAL: WindowDecision = WindowDecision::Fallback(FallbackReason::NotSequential);

    fn host_failure(kind: HostFailureKind) -> IsolatedApplyError {
        IsolatedApplyError::HostFailure(HostFailure::new(kind, "test"))
    }

    fn setup_timeout(setup_cpu_us: Option<u64>) -> (IsolatedApplyError, WorkerObservation) {
        (
            host_failure(HostFailureKind::SetupTimeout),
            WorkerObservation {
                setup_cpu_us,
                ..WorkerObservation::default()
            },
        )
    }

    fn assert_busy(refusal: &Refusal, cause: BusyCause, mode: RetryMode, after: u64) {
        match refusal {
            Refusal::Busy {
                cause: got_cause,
                mode: got_mode,
                retry_after_ms,
            } => assert_eq!(
                (*got_cause, *got_mode, *retry_after_ms),
                (cause, mode, after),
                "{refusal:?}"
            ),
            other => panic!("expected busy {cause:?}/{mode:?}, got {other:?}"),
        }
    }

    fn assert_content(refusal: &Refusal, kind: &str, limit: u64) {
        match refusal {
            Refusal::Content { limit_kind, limit: got } => {
                assert_eq!((*limit_kind, *got), (kind, limit), "{refusal:?}");
            }
            other => panic!("expected the content limit {kind}, got {other:?}"),
        }
    }

    fn assert_defect(refusal: &Refusal, reason: &str) {
        match refusal {
            Refusal::Defect { reason: got } => assert_eq!(*got, reason),
            other => panic!("expected the defect {reason}, got {other:?}"),
        }
    }

    const REBASES: RebasePolicy = RebasePolicy::ClientRebases;
    const INCREMENTAL: WindowMode = WindowMode::Incremental;

    // Row 1: any ceiling on an incremental attempt that was not sequential.
    #[test]
    fn row_1_a_ceiling_on_a_non_sequential_incremental_attempt_asks_for_a_rebase() {
        for (err, cpu_us) in [
            (IsolatedApplyError::CpuCeiling, 52_000),
            (IsolatedApplyError::WallCeiling, 90_000),
            (IsolatedApplyError::WallCeiling, 3_000),
            (IsolatedApplyError::MemoryCeiling, 10_000),
        ] {
            let label = format!("{err:?} after {cpu_us} us");
            let refusal = classify(err, &observed(Some(cpu_us), Some(NOT_SEQUENTIAL)), INCREMENTAL, REBASES);
            assert_busy(&refusal, BusyCause::NeedsSequential, RetryMode::Rebase, 0);
            assert!(matches!(refusal, Refusal::Busy { .. }), "{label}");
        }
        // The REST content command rebuilds on the server: also a rebase.
        let refusal = classify(
            IsolatedApplyError::CpuCeiling,
            &observed(Some(52_000), Some(NOT_SEQUENTIAL)),
            INCREMENTAL,
            RebasePolicy::ServerRebuilds,
        );
        assert_busy(&refusal, BusyCause::NeedsSequential, RetryMode::Rebase, 0);
    }

    // Row 2: a wall kill of a worker that did not get the CPU.
    #[test]
    fn row_2_a_wall_kill_with_little_cpu_is_starvation_and_is_resent() {
        for cpu_us in [0, 3_000, STARVED_CPU_MAX_US - 1] {
            let refusal = classify(
                IsolatedApplyError::WallCeiling,
                &observed(Some(cpu_us), None),
                INCREMENTAL,
                REBASES,
            );
            assert_busy(&refusal, BusyCause::CpuStarved, RetryMode::Resend, 250);
        }
        assert_eq!(STARVED_CPU_MAX_US, 40_000, "80 percent of the 50 ms ceiling");
    }

    // Row 3: a wall kill of a worker that had the CPU, or whose CPU is unknown.
    #[test]
    fn row_3_a_wall_kill_with_the_cpu_used_or_unknown_is_the_content_limit() {
        for cpu_us in [Some(STARVED_CPU_MAX_US), Some(60_000), None] {
            let refusal = classify(
                IsolatedApplyError::WallCeiling,
                &observed(cpu_us, None),
                INCREMENTAL,
                REBASES,
            );
            assert_content(&refusal, "decode_apply_wall_ms", DECODE_APPLY_WALL_MS_MAX);
        }
    }

    // Row 4: a CPU kill is the content limit, whatever the policy.
    #[test]
    fn row_4_a_cpu_kill_is_the_content_limit() {
        for policy in [REBASES, RebasePolicy::None] {
            let refusal = classify(
                IsolatedApplyError::CpuCeiling,
                &observed(Some(52_000), Some(WindowDecision::Incremental)),
                INCREMENTAL,
                policy,
            );
            assert_content(&refusal, "decode_apply_cpu_ms", DECODE_APPLY_CPU_MS_MAX);
        }
    }

    // Row 5: a memory kill is the content limit.
    #[test]
    fn row_5_a_memory_kill_is_the_content_limit() {
        let refusal = classify(
            IsolatedApplyError::MemoryCeiling,
            &observed(Some(10_000), None),
            INCREMENTAL,
            REBASES,
        );
        assert_content(&refusal, "isolated_apply_memory_bytes", ISOLATED_APPLY_MEMORY_BYTES_MAX);
    }

    // Row 6: a setup timeout after little CPU.
    #[test]
    fn row_6_a_setup_timeout_with_little_setup_cpu_is_starvation_and_is_resent() {
        for cpu_us in [0, 400_000, setup_starved_below_us(BUDGET_MS) - 1] {
            let (err, observation) = setup_timeout(Some(cpu_us));
            let refusal = classify(err, &observation, INCREMENTAL, REBASES);
            assert_busy(&refusal, BusyCause::SetupStarved, RetryMode::Resend, 1_000);
        }
    }

    // B-1: the starvation threshold is a share of the configured setup budget, not the budget.
    #[test]
    fn b_1_the_starved_setup_threshold_is_80_percent_of_the_configured_budget() {
        assert_eq!(setup_starved_below_us(2_500), 2_000_000);
        assert_eq!(setup_starved_below_us(500), 400_000);
        assert_eq!(setup_starved_below_us(4_000), 3_200_000);
        // The review's case: a setup that consumed 2.6 s of CPU before the 5 s timeout, with the
        // default budget, is not starvation (it reached the guard) ...
        let (err, observation) = setup_timeout(Some(2_600_000));
        assert!(is_setup_exhausted(&err, &observation, 2_500));
        assert_defect(
            &classify(err, &observation, INCREMENTAL, REBASES),
            "isolated_setup_exhausted",
        );
        // ... and a setup that consumed 2.0 s of it (80 %) is not starvation either; 1.99 s is.
        for (cpu_us, starved) in [(1_999_999, true), (2_000_000, false), (2_499_999, false)] {
            let (err, observation) = setup_timeout(Some(cpu_us));
            assert_eq!(!is_setup_exhausted(&err, &observation, 2_500), starved, "{cpu_us} us");
            let refusal = super::classify(err, &observation, INCREMENTAL, REBASES, 2_500);
            if starved {
                assert_busy(&refusal, BusyCause::SetupStarved, RetryMode::Resend, 1_000);
            } else {
                assert_defect(&refusal, "isolated_setup_exhausted");
            }
        }
        // A raised budget raises the threshold: 3 s of CPU is starvation under a 4 s budget.
        let (err, observation) = setup_timeout(Some(3_000_000));
        assert!(!is_setup_exhausted(&err, &observation, 4_000));
        assert_busy(
            &super::classify(err, &observation, INCREMENTAL, REBASES, 4_000),
            BusyCause::SetupStarved,
            RetryMode::Resend,
            1_000,
        );
        // An unknown CPU is never starvation, and nothing but a setup timeout is a setup that
        // exhausted its budget.
        let (err, observation) = setup_timeout(None);
        assert!(is_setup_exhausted(&err, &observation, 2_500));
        assert!(!is_setup_exhausted(
            &host_failure(HostFailureKind::Crash),
            &observation,
            2_500
        ));
        assert!(!is_setup_exhausted(
            &IsolatedApplyError::CpuCeiling,
            &observation,
            2_500
        ));
    }

    // Row 7: a setup timeout after a lot of CPU, or an unknown amount.
    #[test]
    fn row_7_a_setup_timeout_with_high_or_unknown_setup_cpu_is_a_defect() {
        for cpu_us in [
            Some(setup_starved_below_us(BUDGET_MS)),
            Some(2_500_000),
            Some(4_900_000),
            None,
        ] {
            let (err, observation) = setup_timeout(cpu_us);
            let refusal = classify(err, &observation, INCREMENTAL, REBASES);
            assert_defect(&refusal, "isolated_setup_exhausted");
        }
    }

    // Row 8: a spawn that failed for lack of a resource.
    #[test]
    fn row_8_a_spawn_without_resources_is_busy_and_is_resent() {
        let refusal = classify(
            host_failure(HostFailureKind::SpawnResources),
            &WorkerObservation::default(),
            INCREMENTAL,
            REBASES,
        );
        assert_busy(&refusal, BusyCause::WorkerResources, RetryMode::Resend, 500);
    }

    // Row 9: a SIGKILL the host did not send.
    #[test]
    fn row_9_a_foreign_sigkill_is_busy_and_is_resent() {
        let refusal = classify(
            host_failure(HostFailureKind::ForeignKill),
            &WorkerObservation::default(),
            INCREMENTAL,
            REBASES,
        );
        assert_busy(&refusal, BusyCause::WorkerResources, RetryMode::Resend, 1_000);
    }

    // Row 10: a crash, a bad frame, any other spawn failure.
    #[test]
    fn row_10_a_crash_a_bad_frame_or_another_spawn_failure_is_a_defect() {
        for kind in [
            HostFailureKind::Crash,
            HostFailureKind::Protocol,
            HostFailureKind::Spawn,
        ] {
            let refusal = classify(host_failure(kind), &WorkerObservation::default(), INCREMENTAL, REBASES);
            assert_defect(&refusal, "isolated_apply_failed");
        }
    }

    // Rows 11 and 12 (a diverged attempt, a contradicted full-window retry) are decided before the
    // classifier, by `decide_window`; see `window_oracle_tests` in `write.rs`.

    // Row 13: the full-window retry after a divergence hits a ceiling: rows 2-5, never row 1.
    // Row 14: per-document cooling makes the first attempt Full: rows 2-5, never row 1.
    // `[flow] incremental_window = false` is the same.
    #[test]
    fn rows_13_and_14_a_full_attempt_is_never_asked_to_rebase() {
        for (err, cpu_us, expected) in [
            (IsolatedApplyError::CpuCeiling, 52_000, "decode_apply_cpu_ms"),
            (IsolatedApplyError::WallCeiling, 90_000, "decode_apply_wall_ms"),
            (IsolatedApplyError::MemoryCeiling, 10_000, "isolated_apply_memory_bytes"),
        ] {
            let refusal = classify(
                err,
                &observed(Some(cpu_us), Some(NOT_SEQUENTIAL)),
                WindowMode::Full,
                REBASES,
            );
            match refusal {
                Refusal::Content { limit_kind, .. } => assert_eq!(limit_kind, expected),
                other => panic!("a full attempt got {other:?}"),
            }
        }
        // A starved full attempt is still starvation, resent.
        let refusal = classify(
            IsolatedApplyError::WallCeiling,
            &observed(Some(3_000), Some(NOT_SEQUENTIAL)),
            WindowMode::Full,
            REBASES,
        );
        assert_busy(&refusal, BusyCause::CpuStarved, RetryMode::Resend, 250);
    }

    // Row 15: the setup work guard fired, so there were no facts: the decision is `FactsAbsent`.
    // Row 16: killed before the check decided: no decision record.
    #[test]
    fn rows_15_and_16_without_facts_or_a_decision_a_ceiling_is_never_row_1() {
        for decision in [
            Some(WindowDecision::Fallback(FallbackReason::FactsAbsent)),
            Some(WindowDecision::Undecided),
            Some(WindowDecision::Incremental),
            Some(WindowDecision::Fallback(FallbackReason::StaleFacts)),
            Some(WindowDecision::Fallback(FallbackReason::WorkBudget)),
            Some(WindowDecision::Fallback(FallbackReason::GlobalStructure)),
            None,
        ] {
            let refusal = classify(
                IsolatedApplyError::CpuCeiling,
                &observed(Some(52_000), decision),
                INCREMENTAL,
                REBASES,
            );
            assert_content(&refusal, "decode_apply_cpu_ms", DECODE_APPLY_CPU_MS_MAX);
        }
    }

    // Row 17: unchanged.
    #[test]
    fn row_17_in_band_rejections_and_an_unsupported_platform_are_unchanged() {
        let in_band = classify(
            IsolatedApplyError::Collab(CollabError::CausalDependencyMissing),
            &observed(Some(1_000), Some(NOT_SEQUENTIAL)),
            INCREMENTAL,
            REBASES,
        );
        assert!(matches!(in_band, Refusal::Collab(CollabError::CausalDependencyMissing)));
        let unsupported = classify(
            IsolatedApplyError::UnsupportedPlatform { os: "macos" },
            &WorkerObservation::default(),
            INCREMENTAL,
            REBASES,
        );
        assert!(matches!(unsupported, Refusal::Unsupported { os: "macos" }));
    }

    #[test]
    fn a_surface_that_cannot_rebase_gets_the_content_answer_of_rows_3_to_5() {
        let policy = RebasePolicy::None;
        let refusal = classify(
            IsolatedApplyError::CpuCeiling,
            &observed(Some(52_000), Some(NOT_SEQUENTIAL)),
            INCREMENTAL,
            policy,
        );
        assert_content(&refusal, "decode_apply_cpu_ms", DECODE_APPLY_CPU_MS_MAX);
        let refusal = classify(
            IsolatedApplyError::WallCeiling,
            &observed(Some(90_000), Some(NOT_SEQUENTIAL)),
            INCREMENTAL,
            policy,
        );
        assert_content(&refusal, "decode_apply_wall_ms", DECODE_APPLY_WALL_MS_MAX);
        let refusal = classify(
            IsolatedApplyError::MemoryCeiling,
            &observed(Some(10_000), Some(NOT_SEQUENTIAL)),
            INCREMENTAL,
            policy,
        );
        assert_content(&refusal, "isolated_apply_memory_bytes", ISOLATED_APPLY_MEMORY_BYTES_MAX);
        // Starvation is a resend for every surface.
        let refusal = classify(
            IsolatedApplyError::WallCeiling,
            &observed(Some(3_000), None),
            INCREMENTAL,
            policy,
        );
        assert_busy(&refusal, BusyCause::CpuStarved, RetryMode::Resend, 250);
    }

    #[test]
    fn row_1_precedes_row_2() {
        // A starved wall kill of a non-sequential update is asked to rebase: a sequential resend is
        // cheaper than the same bytes again.
        let refusal = classify(
            IsolatedApplyError::WallCeiling,
            &observed(Some(1_000), Some(NOT_SEQUENTIAL)),
            INCREMENTAL,
            REBASES,
        );
        assert_busy(&refusal, BusyCause::NeedsSequential, RetryMode::Rebase, 0);
    }

    #[test]
    fn an_unknown_cpu_is_content_and_never_busy() {
        let refusal = classify(
            IsolatedApplyError::WallCeiling,
            &observed(None, None),
            INCREMENTAL,
            REBASES,
        );
        assert_content(&refusal, "decode_apply_wall_ms", DECODE_APPLY_WALL_MS_MAX);
    }

    #[test]
    fn the_wire_vocabulary_is_the_contracts() {
        assert_eq!(BusyCause::CpuStarved.wire_value(), "cpu_starved");
        assert_eq!(BusyCause::NeedsSequential.wire_value(), "needs_sequential");
        assert_eq!(BusyCause::SetupStarved.wire_value(), "setup_starved");
        assert_eq!(BusyCause::WorkerResources.wire_value(), "worker_resources");
        assert_eq!(RetryMode::Resend.wire_value(), "resend");
        assert_eq!(RetryMode::Rebase.wire_value(), "rebase");
    }

    /// Runs `body` with `mutant` active on this thread and reports whether `body` caught it.
    fn caught_by(mutant: RefusalMutant, body: impl FnOnce() + std::panic::UnwindSafe) -> bool {
        REFUSAL_MUTANT.with(|active| active.set(Some(mutant)));
        let outcome = std::panic::catch_unwind(body);
        REFUSAL_MUTANT.with(|active| active.set(None));
        outcome.is_err()
    }

    /// The mutant "a CPU-ceiling kill is treated as busy" is caught by the row 4 tests.
    #[test]
    fn the_mutant_that_treats_a_cpu_kill_as_busy_is_caught() {
        assert!(caught_by(
            RefusalMutant::CpuKillIsBusy,
            row_4_a_cpu_kill_is_the_content_limit
        ));
        assert!(caught_by(
            RefusalMutant::CpuKillIsBusy,
            rows_15_and_16_without_facts_or_a_decision_a_ceiling_is_never_row_1
        ));
        // Without the mutant the same bodies pass.
        row_4_a_cpu_kill_is_the_content_limit();
    }

    #[test]
    fn a_refusal_is_counted_once_and_its_total_rides_on_the_log_line() {
        let observation = observed(Some(3_000), None);
        let attempt = AttemptLog {
            document_id: Uuid::new_v4(),
            update_id: Some(Uuid::new_v4()),
            base_head_seq: Some(7),
            requested: INCREMENTAL,
            cooling: false,
            observation: &observation,
        };
        let refusal = Refusal::Busy {
            cause: BusyCause::CpuStarved,
            mode: RetryMode::Resend,
            retry_after_ms: 250,
        };
        let before = busy_total(BusyCause::CpuStarved);
        let stopped = Stopped::of(&IsolatedApplyError::WallCeiling);
        assert_eq!(stopped.ceiling, "wall");
        let total = record(&attempt, stopped, &refusal).expect("a busy refusal is counted");
        assert!(total > before);
        assert_eq!(busy_total(BusyCause::CpuStarved), total);
        // The content limit and the defect have their own counters.
        let limits_before = limit_total("decode_apply_cpu_ms");
        let content = Refusal::Content {
            limit_kind: "decode_apply_cpu_ms",
            limit: DECODE_APPLY_CPU_MS_MAX,
        };
        let total = record(&attempt, Stopped::of(&IsolatedApplyError::CpuCeiling), &content)
            .expect("a content refusal is counted");
        assert!(total > limits_before);
        assert_eq!(limit_total("decode_apply_cpu_ms"), total);
        let defects_before = defect_total("isolated_apply_failed");
        let defect = Refusal::Defect {
            reason: SERVER_REJECTED_REASON_ISOLATED_APPLY_FAILED,
        };
        let crashed =
            IsolatedApplyError::HostFailure(collab_core::isolation::HostFailure::new(HostFailureKind::Crash, "test"));
        let total = record(&attempt, Stopped::of(&crashed), &defect).expect("a defect is counted");
        assert!(total > defects_before);
        assert_eq!(defect_total("isolated_apply_failed"), total);
        let collab = Refusal::Collab(CollabError::CausalDependencyMissing);
        assert_eq!(
            record(
                &attempt,
                Stopped::of(&IsolatedApplyError::Collab(CollabError::CausalDependencyMissing)),
                &collab
            ),
            None
        );
    }
}
