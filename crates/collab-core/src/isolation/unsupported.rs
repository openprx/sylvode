//! Entry points for targets without the `ADR-0014` isolation boundary (every target except Linux).
//!
//! Same signatures as `isolation::host`'s, so callers compile unchanged; both always refuse with
//! [`IsolatedApplyError::UnsupportedPlatform`] without touching their input. No in-process apply
//! fallback exists on purpose: decoding an untrusted update here would bypass the CPU/wall/memory
//! ceilings the boundary exists to enforce.

use super::outcome::{IsolatedApplyError, IsolatedApplySuccess, IsolatedDiffSuccess, WindowMode, WorkerObservation};
use super::wire;

/// Always [`IsolatedApplyError::UnsupportedPlatform`] on this target.
///
/// # Errors
/// Always.
pub const fn isolated_apply(_base_snapshot: &[u8], _update: &[u8]) -> Result<IsolatedApplySuccess, IsolatedApplyError> {
    Err(unsupported())
}

/// Always [`IsolatedApplyError::UnsupportedPlatform`] on this target.
///
/// # Errors
/// Always.
pub const fn isolated_apply_with(
    _base_snapshot: &[u8],
    _update: &[u8],
    _mode: WindowMode,
) -> Result<IsolatedApplySuccess, IsolatedApplyError> {
    Err(unsupported())
}

/// Always [`IsolatedApplyError::UnsupportedPlatform`] on this target, with no observation.
pub const fn isolated_apply_observed(
    _base_snapshot: &[u8],
    _update: &[u8],
) -> (Result<IsolatedApplySuccess, IsolatedApplyError>, WorkerObservation) {
    (Err(unsupported()), observation())
}

/// Always [`IsolatedApplyError::UnsupportedPlatform`] on this target, with no observation.
pub const fn isolated_apply_with_observed(
    _base_snapshot: &[u8],
    _update: &[u8],
    _mode: WindowMode,
    _setup_cpu_budget_ms: u64,
) -> (Result<IsolatedApplySuccess, IsolatedApplyError>, WorkerObservation) {
    (Err(unsupported()), observation())
}

/// Always [`IsolatedApplyError::UnsupportedPlatform`] on this target, with no observation.
pub const fn isolated_diff_observed(
    _base_snapshot: &[u8],
    _request: &wire::ReplayDiffRequest,
) -> (Result<IsolatedDiffSuccess, IsolatedApplyError>, WorkerObservation) {
    (Err(unsupported()), observation())
}

const fn observation() -> WorkerObservation {
    WorkerObservation {
        metered_wall_us: None,
        metered_cpu_us: None,
        setup_cpu_us: None,
        setup_wall_us: None,
        decision: None,
        setup_guard: None,
    }
}

/// Always [`IsolatedApplyError::UnsupportedPlatform`] on this target.
///
/// # Errors
/// Always.
pub const fn isolated_diff(
    _base_snapshot: &[u8],
    _request: &wire::ReplayDiffRequest,
) -> Result<IsolatedDiffSuccess, IsolatedApplyError> {
    Err(unsupported())
}

const fn unsupported() -> IsolatedApplyError {
    IsolatedApplyError::UnsupportedPlatform {
        os: std::env::consts::OS,
    }
}
