//! The result types of [`super::isolated_apply`] / [`super::isolated_diff`], kept free of any
//! platform `cfg` so callers (`apps/api`'s collab write path and object diff) can name and match on
//! them on every target `apps/api` compiles for. Only the enforcing host itself
//! (`isolation::host`, Linux-only per `ADR-0014`'s platform matrix) produces the resource-ceiling
//! variants; on every other target the entry points return [`IsolatedApplyError::UnsupportedPlatform`].

use crate::error::CollabError;

use super::wire;

#[derive(Debug)]
pub struct IsolatedApplySuccess {
    pub snapshot: Vec<u8>,
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
    /// any ceiling. Not a business rejection; callers should treat this as an internal error.
    HostFailure(String),
    /// This build targets a platform the isolation boundary does not exist on. `ADR-0014` declares
    /// the enforcement platform matrix as Linux (`ITIMER_PROF`, `/proc/self/task`, `wait`-family
    /// signal classification), so on any other target nothing was decoded or applied: there is
    /// deliberately no in-process fallback, which would run untrusted updates outside the
    /// CPU/wall/memory ceilings. Deterministic and permanent for this process. `os` is
    /// [`std::env::consts::OS`] of the build.
    UnsupportedPlatform { os: &'static str },
}
