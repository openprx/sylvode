//! The result types of [`super::isolated_apply`] / [`super::isolated_diff`], kept free of any
//! platform `cfg` so callers (`apps/api`'s collab write path and object diff) can name and match on
//! them on every target `apps/api` compiles for. Only the enforcing host itself
//! (`isolation::host`, Linux-only per `ADR-0014`'s platform matrix) produces the resource-ceiling
//! variants; on every other target the entry points return [`IsolatedApplyError::UnsupportedPlatform`].

use crate::error::CollabError;

use super::wire;

/// How the worker's metered window decides the structural limits.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum WindowMode {
    /// Incrementally against the facts of the base when the check can be certain, with the full
    /// computation otherwise.
    #[default]
    Incremental,
    /// Always the full computation: `semantic_snapshot` and `check_snapshot` on the whole document.
    Full,
}

#[derive(Debug)]
pub struct IsolatedApplySuccess {
    pub snapshot: Vec<u8>,
    /// Process CPU time the worker spent inside its metered window, in microseconds, as reported
    /// by the worker (`CLOCK_PROCESS_CPUTIME_ID`). Telemetry only; `None` if it was not reported.
    pub window_cpu_us: Option<u64>,
    /// What the worker's incremental window check established about the accepted document, when
    /// it decided (`None` when the full computation did). The host compares it with its own
    /// projection of the document.
    pub window_report: Option<crate::window_check::WindowReport>,
    /// The container ids of the editor blocks the update touched, sent with a report: the host
    /// measures the decode cost of their texts in the candidate document before it accepts it.
    pub touched_blocks: Vec<String>,
}

pub type IsolatedDiffSuccess = wire::ReplayDiffResult;

/// Why [`super::isolated_apply`] did not return a candidate document.
#[derive(Debug)]
pub enum IsolatedApplyError {
    /// The worker reported an ordinary, in-band rejection: `import_update` failed to decode, or
    /// the merged result violated a `check_snapshot` structural ceiling. Identical in shape to
    /// what a direct in-process call would have produced.
    Collab(CollabError),
    /// The worker was terminated by its own `SIGPROF` timer before it could report anything else.
    CpuCeiling,
    /// This host's independent wall-clock watchdog `SIGKILL`ed the worker before it produced a
    /// response.
    WallCeiling,
    /// The worker aborted itself (`SIGABRT`): the counting allocator (or the `RLIMIT_AS`
    /// backstop) rejected an allocation that would have crossed the memory ceiling.
    MemoryCeiling,
    /// The isolation mechanism itself did not function as intended -- could not spawn the worker,
    /// its response frame/payload was corrupt, or it exited/was killed for a reason unrelated to
    /// any ceiling. Not a business rejection. [`HostFailure::kind`] says which of a few failure
    /// kinds it was, because a host short of resources is not a defect of the mechanism.
    HostFailure(HostFailure),
    /// This build targets a platform the isolation boundary does not exist on. `ADR-0014` declares
    /// the enforcement platform matrix as Linux (a process-CPU-clock timer, `/proc/self/task`, `wait`-family
    /// signal classification), so on any other target nothing was decoded or applied: there is
    /// deliberately no in-process fallback, which would run untrusted updates outside the
    /// CPU/wall/memory ceilings. Deterministic and permanent for this process. `os` is
    /// [`std::env::consts::OS`] of the build.
    UnsupportedPlatform { os: &'static str },
}

/// Why the isolation host failed, typed so that the caller can tell a host short of resources (a
/// retry may succeed) from a defect (it will not).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HostFailure {
    pub kind: HostFailureKind,
    /// Diagnostic text for the log; never part of an answer to a client.
    pub detail: String,
}

impl HostFailure {
    pub fn new(kind: HostFailureKind, detail: impl Into<String>) -> Self {
        Self {
            kind,
            detail: detail.into(),
        }
    }
}

impl std::fmt::Display for HostFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{:?}: {}", self.kind, self.detail)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HostFailureKind {
    /// The worker could not be spawned for lack of a resource (`EAGAIN`, `ENOMEM`, `EMFILE`,
    /// `ENFILE`).
    SpawnResources,
    /// The worker could not be spawned for another reason (missing binary, permissions, ...).
    Spawn,
    /// The worker did not open its metered window within the setup timeout and was killed.
    SetupTimeout,
    /// The worker was killed by a `SIGKILL` this host did not send (the kernel's OOM killer, an
    /// operator).
    ForeignKill,
    /// The worker crashed (a signal other than the ceilings'), or exited with a non-zero status.
    Crash,
    /// The worker's response was missing, truncated or invalid.
    Protocol,
}

/// What the host itself observed of one worker run between the moment the worker opened its
/// metered window (the marker byte) and the moment the worker ended. The worker cannot report this
/// about itself when a ceiling kills it, so the host reads it from the kernel: the child's CPU
/// clock at the marker and the final `wait4` resource usage, and from the worker's own decision
/// record (`wire::WindowDecision`) what its window check decided.
///
/// It tells a wall-watchdog kill caused by a slow workload apart from one caused by the host
/// not giving the worker CPU: a worker that was killed at the wall deadline with
/// [`Self::metered_cpu_us`] far below `decode_apply_cpu_ms_max` was starved, not heavy.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct WorkerObservation {
    /// Wall-clock microseconds from the marker byte to the end of the response read (or to the
    /// watchdog kill). `None` if the worker never opened its metered window.
    pub metered_wall_us: Option<u64>,
    /// CPU microseconds (user plus system) the worker consumed between the marker byte and its
    /// exit. `None` if the window never opened or the kernel did not report the figures.
    pub metered_cpu_us: Option<u64>,
    /// CPU microseconds the worker had consumed when it opened its metered window, that is, its
    /// setup (spawn, base load, prewarm, base facts). For a worker killed at the setup timeout, its
    /// whole CPU time. `None` if the kernel did not report it.
    pub setup_cpu_us: Option<u64>,
    /// Wall-clock microseconds from the spawn of the worker to the moment it opened its metered
    /// window; for a worker killed at the setup timeout, to the kill. With [`Self::setup_cpu_us`] it
    /// tells a setup that was not scheduled (CPU far below wall) from one that burned its time.
    /// `None` if the worker was never spawned.
    pub setup_wall_us: Option<u64>,
    /// What the worker's window check decided, from its decision record. `None` when the worker
    /// wrote no record (killed before the check decided, or a failure before the window).
    pub decision: Option<wire::WindowDecision>,
    /// The setup guard that fired in the worker (`crate::setup_guard`), from its marker byte: the
    /// base took more work than that guard allows, so there were no base facts and the full
    /// computation decided. Not an error; telemetry for the host's log, reported for every attempt
    /// whose worker opened its metered window, whether it was accepted, refused or killed.
    pub setup_guard: Option<crate::setup_guard::Fired>,
}
