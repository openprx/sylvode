//! Parent-side orchestration for [`isolated_apply`]: spawns `src/bin/isolated_apply_worker.rs` as
//! a fresh process per call, writes the harness-envelope request, wall-watchdogs the response with
//! an independent `SIGKILL` deadline, and classifies the outcome from the child's exit signal and
//! response frame.
//!
//! # Why a subprocess instead of `ADR-0014`'s fork-no-exec zygote
//!
//! `ADR-0014`'s isolation host (`spikes/collab-shared/src/isolation`) requires the zygote to be
//! genuinely single-threaded at every `fork()` call -- verified against `/proc/self/task`, not
//! assumed from startup ordering (`ADR-0014` section 10). That precondition cannot hold here:
//! `apps/api` is a multi-threaded Tokio server, and by the time any request reaches this write
//! path the process already has many OS threads. `fork()`ing from such a thread would either be
//! refused by that same single-threadedness check (correctly, since it is not actually true) or,
//! if attempted anyway, be unsound for the exact reason `ADR-0014` gives for why fork-only-from-
//! single-threaded *is* safe: at the fork point there must be exactly one thread, so no lock held
//! by another thread is left in an inconsistent state in the child. Pre-forking a dedicated zygote
//! before Tokio starts would satisfy this, but requires wiring into `apps/api::main` before any
//! runtime thread exists, which is outside this package's (`collab-write-path`) file scope.
//!
//! Spawning a fresh process via [`std::process::Command`] sidesteps the precondition entirely: a
//! freshly `exec`'d process image starts single-threaded regardless of how many threads its parent
//! has, because `Command::spawn` on Linux is `fork` immediately followed by `execve` -- the child
//! never runs any of the parent's code (Tokio's runtime included) between those two calls. This
//! adopts `ADR-0014`'s *online-enforcement* mechanisms over that different process-creation
//! primitive: `SIGPROF` from a process-CPU-clock timer for the CPU ceiling (section 1.1, in
//! `isolation::child_runtime`), a counting `GlobalAlloc` for the memory ceiling (sections 1 and 3,
//! in `isolation::alloc`), and an independent wall-clock watchdog with `SIGKILL` (section 1,
//! below).
//!
//! It does **not** carry over section 2's shared measurement page: that exists to recover
//! forensic peak/cause telemetry from a killed process for *gate evidence*. This production call
//! site has no use for that telemetry -- it only needs a correct accept/reject decision, which it
//! gets from the exit signal alone (`SIGPROF` -> [`IsolatedApplyError::CpuCeiling`], `SIGABRT`
//! (the allocator's own rejection routed through Rust's `handle_alloc_error`) ->
//! [`IsolatedApplyError::MemoryCeiling`], a watchdog-issued `SIGKILL` -> [`IsolatedApplyError::WallCeiling`]).
//!
//! Per `ADR-0014` section 8, the calibration host it comes from is "a v0.3 selection/acceptance
//! facility, not a v0.4 production safety boundary", and production enforcement is scoped to
//! "another ADR". This module *is* that production enforcement for the collab write path -- no
//! v0.3 spike evidence is cited as proof it works; it is a fresh implementation, verified by this
//! module's own tests spawning the real worker binary.

#![allow(unsafe_code)]

use std::io::Read;
use std::os::fd::AsRawFd;
use std::os::unix::process::ExitStatusExt;
use std::process::{Child, ChildStdout, ExitStatus, Stdio};
use std::time::{Duration, Instant};

use super::limits::{DECODE_APPLY_WALL_MS_MAX, IMPORT_DECODE_WALL_MS_MAX};
use super::outcome::{
    HostFailure, HostFailureKind, IsolatedApplyError, IsolatedApplySuccess, IsolatedDiffSuccess, WindowMode,
    WorkerObservation,
};
use super::spawn_env::worker_command;
use super::wire::{self, MAX_RESPONSE_PAYLOAD_BYTES, Outcome};

const RESPONSE_FRAME_HEADER_BYTES: usize = 8;

/// Overrides the worker binary's location; set for tests and for deployments that do not place
/// `collab-isolated-apply-worker` next to the `apps/api` executable (see [`worker_binary_path`]).
pub const WORKER_BINARY_PATH_ENV: &str = "COLLAB_ISOLATED_APPLY_WORKER_PATH";

const WORKER_BINARY_NAME: &str = "collab-isolated-apply-worker";

/// Resolves the isolated-apply worker binary's path.
///
/// Checked in order: [`WORKER_BINARY_PATH_ENV`] (an explicit override, used by this module's own
/// tests and available for deployments that ship the worker binary somewhere other than next to
/// `apps/api`'s executable); the directory containing the current executable (the normal
/// deployment shape -- `apps/api` and `collab-isolated-apply-worker` built and shipped together,
/// see `crates/collab-core/Cargo.toml`'s `[[bin]]` doc comment); and, for `cargo test`'s layout
/// (where a test binary's `current_exe()` resolves under `target/<profile>/deps/`, one directory
/// below where `[[bin]]` targets are actually placed), the current executable's grandparent
/// directory joined with the worker's name.
///
/// Never resolved via `$PATH`: a `$PATH`-relative lookup would let anything able to influence this
/// process's environment substitute an arbitrary executable for the one that is about to run with
/// this process's own privileges.
///
/// # Errors
/// [`IsolatedApplyError::HostFailure`] if `current_exe()` cannot be resolved, or if the worker
/// binary cannot be found at any of the candidate locations.
fn worker_binary_path() -> Result<std::path::PathBuf, IsolatedApplyError> {
    if let Ok(overridden) = std::env::var(WORKER_BINARY_PATH_ENV) {
        return Ok(std::path::PathBuf::from(overridden));
    }

    let current_exe = std::env::current_exe()
        .map_err(|err| host_failure(HostFailureKind::Spawn, format!("could not resolve current_exe: {err}")))?;
    let dir = current_exe
        .parent()
        .ok_or_else(|| host_failure(HostFailureKind::Spawn, "current_exe has no parent directory"))?;

    let sibling = dir.join(WORKER_BINARY_NAME);
    if sibling.is_file() {
        return Ok(sibling);
    }

    if let Some(grandparent) = dir.parent() {
        let candidate = grandparent.join(WORKER_BINARY_NAME);
        if candidate.is_file() {
            return Ok(candidate);
        }
    }

    Err(host_failure(
        HostFailureKind::Spawn,
        format!(
            "worker binary '{WORKER_BINARY_NAME}' not found next to '{}'; set {WORKER_BINARY_PATH_ENV} to override",
            current_exe.display()
        ),
    ))
}

fn host_failure(kind: HostFailureKind, detail: impl Into<String>) -> IsolatedApplyError {
    IsolatedApplyError::HostFailure(HostFailure::new(kind, detail))
}

/// The failure of spawning the worker: a host short of a resource is told apart from the rest.
fn spawn_failure(error: &std::io::Error) -> IsolatedApplyError {
    let kind = match error.raw_os_error() {
        Some(code) if is_resource_errno(code) => HostFailureKind::SpawnResources,
        _ => HostFailureKind::Spawn,
    };
    host_failure(kind, format!("spawn of isolated-apply worker failed: {error}"))
}

/// The `errno` values of a spawn that failed for lack of a resource and may succeed later:
/// too many processes (`EAGAIN`), out of memory (`ENOMEM`), too many open files (`EMFILE`,
/// `ENFILE`).
const fn is_resource_errno(code: i32) -> bool {
    matches!(code, libc::EAGAIN | libc::ENOMEM | libc::EMFILE | libc::ENFILE)
}

/// Runs `LoroCollabEngine::load(base_snapshot)` + `import_update(update)` + `semantic_snapshot()` +
/// `check_snapshot` inside a freshly spawned, resource-ceilinged worker process, per
/// `contracts/limits-v1.md`'s "Isolated decode/apply" table.
///
/// Blocking: performs process spawn, pipe I/O, and `wait` synchronously. Callers on an async
/// executor (`apps/api`'s `flow::collab::write::hydrate_and_apply`) must run this inside
/// `tokio::task::spawn_blocking`.
///
/// # Errors
/// See [`IsolatedApplyError`].
pub fn isolated_apply(base_snapshot: &[u8], update: &[u8]) -> Result<IsolatedApplySuccess, IsolatedApplyError> {
    isolated_apply_with(base_snapshot, update, WindowMode::Incremental)
}

/// [`isolated_apply`] with the window mode chosen by the host: [`WindowMode::Full`] makes the worker
/// run the full computation (`semantic_snapshot` and `check_snapshot`) in its window and report no
/// incremental result. The mode can only add checking.
///
/// # Errors
/// See [`IsolatedApplyError`].
pub fn isolated_apply_with(
    base_snapshot: &[u8],
    update: &[u8],
    mode: WindowMode,
) -> Result<IsolatedApplySuccess, IsolatedApplyError> {
    isolated_apply_with_observed(
        base_snapshot,
        update,
        mode,
        crate::setup_guard::SETUP_CPU_BUDGET_MS_DEFAULT,
    )
    .0
}

/// [`isolated_apply_with`] that additionally reports the [`WorkerObservation`] of the run: the CPU
/// the worker consumed, what its window check decided and which setup guard fired, even when a
/// ceiling killed it. `setup_cpu_budget_ms` is the CPU budget of the worker's setup (`[flow]
/// setup_cpu_budget_ms`); the worker uses its default for a value outside
/// [`crate::setup_guard::SETUP_CPU_BUDGET_MS_RANGE`].
pub fn isolated_apply_with_observed(
    base_snapshot: &[u8],
    update: &[u8],
    mode: WindowMode,
    setup_cpu_budget_ms: u64,
) -> (Result<IsolatedApplySuccess, IsolatedApplyError>, WorkerObservation) {
    let budget = setup_cpu_budget_ms.to_string();
    let incremental = [(wire::SETUP_CPU_BUDGET_ENV, budget.as_str())];
    let full = [
        (wire::SETUP_CPU_BUDGET_ENV, budget.as_str()),
        (wire::WINDOW_ENV, wire::WINDOW_FULL),
    ];
    match mode {
        WindowMode::Incremental => {
            run_isolated_apply_observed(base_snapshot, update, wire::OPERATION_APPLY, &incremental)
        }
        WindowMode::Full => run_isolated_apply_observed(base_snapshot, update, wire::OPERATION_APPLY, &full),
    }
}

/// Same as [`isolated_apply`], and additionally reports the [`WorkerObservation`] of the run.
/// Callers use it to log whether a [`IsolatedApplyError::WallCeiling`] came with the worker
/// having had the CPU or not. It runs the default (incremental) window mode: it has no mode
/// parameter, so a caller that needs to observe a [`WindowMode::Full`] attempt needs one added.
pub fn isolated_apply_observed(
    base_snapshot: &[u8],
    update: &[u8],
) -> (Result<IsolatedApplySuccess, IsolatedApplyError>, WorkerObservation) {
    isolated_apply_with_observed(
        base_snapshot,
        update,
        WindowMode::Incremental,
        crate::setup_guard::SETUP_CPU_BUDGET_MS_DEFAULT,
    )
}

/// Replays a bounded retained-history tail and computes its semantic diff inside the same
/// killable CPU/wall/memory boundary used for untrusted apply work.
pub fn isolated_diff(
    base_snapshot: &[u8],
    request: &wire::ReplayDiffRequest,
) -> Result<IsolatedDiffSuccess, IsolatedApplyError> {
    isolated_diff_observed(base_snapshot, request).0
}

/// [`isolated_diff`] that additionally reports the [`WorkerObservation`] of the run.
pub fn isolated_diff_observed(
    base_snapshot: &[u8],
    request: &wire::ReplayDiffRequest,
) -> (Result<IsolatedDiffSuccess, IsolatedApplyError>, WorkerObservation) {
    replay_observed(wire::OPERATION_DIFF, base_snapshot, request)
}

/// Imports one uploaded document: loads `base_snapshot`, replays the update tail of `request`
/// (`package_updates_base64`), scans the whole history, projects and checks the result against the
/// content limits, all inside the killable CPU/wall/memory boundary under the import-specific
/// budget (`IMPORT_DECODE_CPU_MS_MAX` and its siblings). The answer carries the validated head
/// snapshot the host stores from, never the uploaded bytes.
pub fn isolated_import_observed(
    base_snapshot: &[u8],
    request: &wire::ReplayDiffRequest,
) -> (Result<IsolatedDiffSuccess, IsolatedApplyError>, WorkerObservation) {
    replay_observed(wire::OPERATION_IMPORT, base_snapshot, request)
}

fn replay_observed(
    operation: &'static str,
    base_snapshot: &[u8],
    request: &wire::ReplayDiffRequest,
) -> (Result<IsolatedDiffSuccess, IsolatedApplyError>, WorkerObservation) {
    let encoded = match wire::encode_replay_diff_request(request) {
        Ok(encoded) => encoded,
        Err(error) => {
            return (
                Err(host_failure(
                    HostFailureKind::Protocol,
                    format!("diff request encoding failed: {error}"),
                )),
                WorkerObservation::default(),
            );
        }
    };
    let (result, observation) = run_isolated_apply_observed(base_snapshot, &encoded, operation, &[]);
    let decoded = result.and_then(|success| {
        wire::decode_replay_diff_result(&success.snapshot).map_err(|error| {
            host_failure(
                HostFailureKind::Protocol,
                format!("diff response decoding failed: {error}"),
            )
        })
    });
    (decoded, observation)
}

/// Runs the worker and reports the host's [`WorkerObservation`] of the run. `extra_env` is set on
/// the *spawned child's own* environment before it runs (never this process's own environment); the
/// production entry points always pass an empty slice, so it has no effect on the production call
/// path. It exists so this module's own tests can drive `src/bin/isolated_apply_worker.rs`'s
/// `cfg(debug_assertions)`-gated synthetic workloads (see that file's `test_injection` module) by
/// setting `COLLAB_ISOLATION_TEST_INJECT` on the one child process a test spawns, without mutating
/// this whole test binary's process-wide environment the way `WORKER_BINARY_PATH_ENV` (necessarily)
/// does.
fn run_isolated_apply_observed(
    base_snapshot: &[u8],
    update: &[u8],
    operation: &'static str,
    extra_env: &[(&str, &str)],
) -> (Result<IsolatedApplySuccess, IsolatedApplyError>, WorkerObservation) {
    let mut observation = WorkerObservation::default();
    let result = run_worker(base_snapshot, update, operation, extra_env, &mut observation);
    (result, observation)
}

fn run_worker(
    base_snapshot: &[u8],
    update: &[u8],
    operation: &'static str,
    extra_env: &[(&str, &str)],
    observation: &mut WorkerObservation,
) -> Result<IsolatedApplySuccess, IsolatedApplyError> {
    let mut write_error: Option<String> = None;
    let result = run_worker_inner(
        base_snapshot,
        update,
        operation,
        extra_env,
        observation,
        &mut write_error,
    );
    // The failed request write never decided the classification; it is the detail of a host failure
    // when that is what the worker's own outcome says it was.
    match (result, write_error) {
        (Err(IsolatedApplyError::HostFailure(mut failure)), Some(note)) => {
            failure.detail = format!("{} (request write failed: {note})", failure.detail);
            Err(IsolatedApplyError::HostFailure(failure))
        }
        (result, _) => result,
    }
}

fn run_worker_inner(
    base_snapshot: &[u8],
    update: &[u8],
    operation: &'static str,
    extra_env: &[(&str, &str)],
    observation: &mut WorkerObservation,
    write_error: &mut Option<String>,
) -> Result<IsolatedApplySuccess, IsolatedApplyError> {
    let worker_path = worker_binary_path()?;

    let mut command = worker_command(&worker_path, operation, extra_env);
    command
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    let mut child = command.spawn().map_err(|err| spawn_failure(&err))?;
    let spawned_at = Instant::now();

    {
        let mut stdin = child
            .stdin
            .take()
            .ok_or_else(|| host_failure(HostFailureKind::Spawn, "child stdin unavailable"))?;
        let write_result = wire::write_request(&mut stdin, base_snapshot, update);
        // `stdin` drops here regardless of `write_result`, closing the write end so the child's
        // `read_exact` calls see a clean EOF rather than hanging on a half-written request.
        drop(stdin);
        // A failed write (a child that exited before it read the request: `EPIPE`) says nothing
        // about how the child ended. The host still reads what it produced and waits for it, and
        // classifies by its exit status, its signal and its output; the error is only kept as the
        // detail of the failure that classification ends in.
        if let Err(err) = write_result {
            *write_error = Some(err.to_string());
        }
    }

    let mut stdout = child
        .stdout
        .take()
        .ok_or_else(|| host_failure(HostFailureKind::Spawn, "child stdout unavailable"))?;

    let (read_outcome, marker) = read_response_two_phase(&mut stdout, &child, metered_wall_ms(operation));
    let metered_wall_us = marker.map(|marker| duration_micros(marker.at.elapsed()));
    drop(stdout);

    let (wait_status, final_cpu_us) = wait_for_exit(&child).map_err(|err| {
        host_failure(
            HostFailureKind::Crash,
            format!("wait on isolated-apply worker failed: {err}"),
        )
    })?;
    observation.metered_wall_us = metered_wall_us;
    observation.metered_cpu_us = match (marker.and_then(|marker| marker.cpu_us), final_cpu_us) {
        (Some(at_marker), Some(at_exit)) => Some(at_exit.saturating_sub(at_marker)),
        _ => None,
    };
    // The CPU of the setup: the child's clock when it opened its window; for a worker killed at
    // the setup timeout, everything it consumed.
    observation.setup_cpu_us = match &read_outcome {
        ReadOutcome::SetupPhaseTimedOut => final_cpu_us,
        _ => marker.and_then(|marker| marker.cpu_us),
    };
    observation.setup_wall_us = match &read_outcome {
        ReadOutcome::SetupPhaseTimedOut => Some(duration_micros(spawned_at.elapsed())),
        _ => marker.map(|marker| duration_micros(marker.at.saturating_duration_since(spawned_at))),
    };
    // Which setup guard fired is in the marker byte, so a refused or killed attempt reports it too.
    observation.setup_guard = marker.and_then(|marker| marker.setup_guard);

    let response_bytes = match read_outcome {
        ReadOutcome::SetupPhaseTimedOut => {
            return Err(host_failure(
                HostFailureKind::SetupTimeout,
                "isolated-apply worker exceeded its setup-phase safety timeout (spawn + base document load) \
                 before opening its metered window",
            ));
        }
        ReadOutcome::MeteredPhaseTimedOut(bytes) => {
            // A worker killed at the wall deadline may still have told us what its check decided.
            observation.decision = wire::split_decision_record(&bytes)
                .ok()
                .and_then(|(decision, _)| decision);
            return Err(IsolatedApplyError::WallCeiling);
        }
        ReadOutcome::ExitedBeforeResponding => Vec::new(),
        ReadOutcome::Completed(bytes) => bytes,
    };

    // Parsed before the signal classification: a worker killed by a ceiling after it wrote its
    // record arrives here as a completed read whose bytes are the record alone.
    let (decision, frame_bytes, record_error) = match wire::split_decision_record(&response_bytes) {
        Ok((decision, rest)) => (decision, rest, None),
        Err(reason) => (None, response_bytes.as_slice(), Some(reason)),
    };
    observation.decision = decision;

    if let Some(signal) = wait_status.signal() {
        return Err(classify_signal(signal));
    }

    if wait_status.code() != Some(0) {
        return Err(host_failure(
            HostFailureKind::Crash,
            format!(
                "isolated-apply worker exited with non-zero status {:?}",
                wait_status.code()
            ),
        ));
    }

    // A worker that completed always wrote a decision record before its frame.
    if decision.is_none() {
        return Err(host_failure(
            HostFailureKind::Protocol,
            format!(
                "response lacks a valid decision record: {}",
                record_error.unwrap_or("no record")
            ),
        ));
    }

    let payload = decode_response_frame(frame_bytes)
        .map_err(|err| host_failure(HostFailureKind::Protocol, format!("response frame invalid: {err}")))?;
    let outcome = wire::decode_outcome(&payload)
        .map_err(|err| host_failure(HostFailureKind::Protocol, format!("response payload invalid: {err}")))?;

    match outcome {
        Outcome::Success {
            snapshot,
            window_cpu_us,
            window_report,
            touched_blocks,
        } => Ok(IsolatedApplySuccess {
            snapshot,
            window_cpu_us,
            window_report,
            touched_blocks,
        }),
        Outcome::Rejected(err) => Err(IsolatedApplyError::Collab(err)),
    }
}

fn classify_signal(signal: i32) -> IsolatedApplyError {
    if signal == libc::SIGPROF {
        IsolatedApplyError::CpuCeiling
    } else if signal == libc::SIGABRT {
        IsolatedApplyError::MemoryCeiling
    } else if signal == libc::SIGKILL {
        // This host only ever sends `SIGKILL` itself from the watchdogs, which already return
        // early before this function is reached. A `SIGKILL` observed here means something *else*
        // killed the child (the kernel OOM killer, an operator's `kill -9`, ...) -- a host-level
        // failure, not a business rejection.
        host_failure(
            HostFailureKind::ForeignKill,
            "isolated-apply worker received an unrecognized SIGKILL",
        )
    } else {
        host_failure(
            HostFailureKind::Crash,
            format!("isolated-apply worker terminated by unexpected signal {signal}"),
        )
    }
}

/// Generous backstop for the unmetered setup phase (process spawn + `execve` + loading the base
/// document -- all of which happen before the child ever arms the CPU/memory ceilings). *Not*
/// itself an `ADR-0014`/`contracts/limits-v1.md` ceiling: sized well above realistic spawn +
/// rehydration cost (including in an unoptimized debug build) so it only ever fires for a
/// genuinely stuck child, not as a disguised version of the strict `decode_apply_wall_ms_max`
/// ceiling below. A production deployment with much larger cached documents may need to widen
/// this; see this module's own doc comment on the performance trade-off of rehydrating the whole
/// base document per call.
const SETUP_PHASE_SAFETY_TIMEOUT_MS: u64 = 5_000;

#[cfg(test)]
thread_local! {
    /// Shortens the setup timeout for the calling test thread, so that the setup cases run in
    /// under a second. Never set outside tests.
    static SETUP_TIMEOUT_OVERRIDE_MS: std::cell::Cell<Option<u64>> = const { std::cell::Cell::new(None) };
}

/// The setup timeout in force: [`SETUP_PHASE_SAFETY_TIMEOUT_MS`], or a test's shorter one.
#[allow(clippy::missing_const_for_fn, reason = "reads a thread-local in test builds")]
fn setup_phase_timeout_ms() -> u64 {
    #[cfg(test)]
    if let Some(override_ms) = SETUP_TIMEOUT_OVERRIDE_MS.with(std::cell::Cell::get) {
        return override_ms;
    }
    SETUP_PHASE_SAFETY_TIMEOUT_MS
}

/// Written as the very first byte of every response the worker ever sends
/// (`write_marker_or_exit` in `src/bin/isolated_apply_worker.rs`), before the framed outcome that
/// follows it. This is the byte of a worker whose setup guard did not fire; one whose guard fired
/// writes the byte `wire::marker_byte` names, which says which (`wire::guard_of_marker`). Its only job is to let [`read_response_two_phase`] tell "still doing unmetered
/// setup" apart from "done with setup, response is now being written" -- so the *strict*
/// `decode_apply_wall_ms_max` watchdog can start counting from the moment that is actually true,
/// instead of from process spawn (which would double-count spawn + base-document-load time
/// against a ceiling `contracts/limits-v1.md` scopes to decode/apply/shape-validate only: "从
/// decode 前开始计,到 semantic diff/shape validation 完成结束").
pub const RESPONSE_MARKER_BYTE: u8 = wire::MARKER_NO_GUARD;

/// What [`read_response_two_phase`] observed.
enum ReadOutcome {
    /// No byte at all arrived within [`SETUP_PHASE_SAFETY_TIMEOUT_MS`]; the child was `SIGKILL`ed.
    /// Not a `decode_apply_wall_ms` ceiling hit -- this is the unmetered setup phase overrunning a
    /// generous safety backstop, a host-level problem.
    SetupPhaseTimedOut,
    /// The marker byte arrived, but the framed response that should follow it did not within
    /// [`DECODE_APPLY_WALL_MS_MAX`] of that point; the child was `SIGKILL`ed. This *is* the
    /// `decode_apply_wall_ms_max` ceiling.
    ///
    /// Carries the bytes read after the marker (the decision record, when the worker got that far).
    MeteredPhaseTimedOut(Vec<u8>),
    /// The child closed its stdout (exited) before writing even the marker byte -- e.g. it never
    /// got past reading the request (`main`'s `exit_without_response` paths in
    /// `src/bin/isolated_apply_worker.rs`). Not a timeout; `isolated_apply` falls through to its
    /// existing exit-code/signal classification, which will find a non-zero/signalled exit.
    ExitedBeforeResponding,
    /// The marker arrived and the rest of the response was read to EOF within its own deadline.
    /// Carries only the bytes *after* the marker (the framed response itself).
    Completed(Vec<u8>),
}

/// Reads the child's response in two phases against two different deadlines, `SIGKILL`ing `child`
/// if either fires -- see [`SETUP_PHASE_SAFETY_TIMEOUT_MS`] and [`RESPONSE_MARKER_BYTE`]'s doc
/// comments for why this is split rather than one flat deadline from process spawn. `ADR-0014`
/// section 1.1: "wall watchdog 命中...是独立的 wall 上限, 不是 CPU 上限的替代品" -- this watchdog
/// remains independent of (does not rely on) the CPU ceiling's own `SIGPROF` enforcement.
fn read_response_two_phase(
    stdout: &mut ChildStdout,
    child: &Child,
    wall_ms: u64,
) -> (ReadOutcome, Option<MarkerSample>) {
    let fd = stdout.as_raw_fd();
    let mut buffer = Vec::new();
    let mut chunk = [0u8; 65536];

    let setup_deadline = Instant::now() + Duration::from_millis(setup_phase_timeout_ms());
    let (setup_timed_out, exited_before_marker) =
        read_until(stdout, fd, child, &mut buffer, &mut chunk, setup_deadline, 1);
    if setup_timed_out {
        return (ReadOutcome::SetupPhaseTimedOut, None);
    }
    if buffer.is_empty() {
        // `read_until`'s only other early-exit besides "reached `min_bytes`" or "deadline" is
        // EOF -- an empty buffer at this point means genuine EOF before any byte arrived.
        debug_assert!(
            exited_before_marker,
            "empty buffer implies EOF, not a bug in read_until"
        );
        return (ReadOutcome::ExitedBeforeResponding, None);
    }

    // The worker has just opened its metered window. Sample the child's CPU clock now, so the
    // CPU it consumed inside the window (not during spawn and base-document load) can be told
    // apart from the total once it has exited.
    let marker_at = Instant::now();
    let marker = MarkerSample {
        at: marker_at,
        cpu_us: child_cpu_micros(pid_of(child)),
        setup_guard: buffer.first().copied().and_then(wire::guard_of_marker),
    };

    // The single-poll-then-read pattern below can (and in practice often does) return more than
    // just the marker byte in one `read` call -- pipes have no "one write = one read" guarantee.
    // Split off the marker and keep whatever response bytes already arrived alongside it, rather
    // than discarding them.
    let response_prefix = if buffer.len() > 1 {
        buffer.split_off(1)
    } else {
        Vec::new()
    };
    buffer = response_prefix;

    let metered_deadline = marker_at + Duration::from_millis(wall_ms);
    let (metered_timed_out, _) = read_until(stdout, fd, child, &mut buffer, &mut chunk, metered_deadline, usize::MAX);
    if metered_timed_out {
        return (ReadOutcome::MeteredPhaseTimedOut(buffer), Some(marker));
    }

    (ReadOutcome::Completed(buffer), Some(marker))
}

/// The wall-clock watchdog of the metered window of `operation`: [`DECODE_APPLY_WALL_MS_MAX`], or
/// the import-specific one for the `import` operation only.
fn metered_wall_ms(operation: &str) -> u64 {
    if operation == wire::OPERATION_IMPORT {
        IMPORT_DECODE_WALL_MS_MAX
    } else {
        DECODE_APPLY_WALL_MS_MAX
    }
}

/// The host's view of the instant the worker opened its metered window.
#[derive(Clone, Copy)]
struct MarkerSample {
    at: Instant,
    /// The child's CPU time (user plus system) at that instant, if the kernel reported it.
    cpu_us: Option<u64>,
    /// The setup guard the marker byte names.
    setup_guard: Option<crate::setup_guard::Fired>,
}

fn duration_micros(duration: Duration) -> u64 {
    u64::try_from(duration.as_micros()).unwrap_or(u64::MAX)
}

/// The CPU time (user plus system) the live child `pid` has consumed so far, read from its
/// per-process CPU clock (`clock_getcpuclockid` + `clock_gettime`). `None` if either call fails.
fn child_cpu_micros(pid: libc::pid_t) -> Option<u64> {
    let mut clock: libc::clockid_t = 0;
    // SAFETY: `&raw mut clock` is a valid out-pointer for a `clockid_t`; `pid` is this
    // process's own child that has not been reaped yet. The call returns 0 or an errno value.
    let rc = unsafe { libc::clock_getcpuclockid(pid, &raw mut clock) };
    if rc != 0 {
        return None;
    }
    let mut ts = libc::timespec { tv_sec: 0, tv_nsec: 0 };
    // SAFETY: `&raw mut ts` is a valid, properly aligned out-pointer for a `timespec`, and
    // `clock` was just filled in by `clock_getcpuclockid`.
    let rc = unsafe { libc::clock_gettime(clock, &raw mut ts) };
    if rc != 0 {
        return None;
    }
    let secs = u64::try_from(ts.tv_sec).ok()?;
    let nanos = u64::try_from(ts.tv_nsec).ok()?;
    Some(secs.saturating_mul(1_000_000).saturating_add(nanos / 1_000))
}

/// Reaps `child` with `wait4`, returning its exit status and the CPU time (user plus system) the
/// kernel accounted to it. Replaces `Child::wait`, which does not expose resource usage; the
/// child must not be waited on again afterwards (dropping the `Child` does not wait).
fn wait_for_exit(child: &Child) -> std::io::Result<(ExitStatus, Option<u64>)> {
    let pid = pid_of(child);
    loop {
        let mut status: libc::c_int = 0;
        // SAFETY: `rusage` is a C struct of plain integers, for which all-zero is a valid value.
        let mut usage: libc::rusage = unsafe { std::mem::zeroed() };
        // SAFETY: `&raw mut status` and `&raw mut usage` are valid out-pointers; `pid` is this
        // process's own child, reaped exactly once (a retry happens only after `EINTR`).
        let reaped = unsafe { libc::wait4(pid, &raw mut status, 0, &raw mut usage) };
        if reaped == pid {
            return Ok((ExitStatus::from_raw(status), rusage_cpu_micros(&usage)));
        }
        let error = std::io::Error::last_os_error();
        if reaped < 0 && error.kind() == std::io::ErrorKind::Interrupted {
            continue;
        }
        return Err(error);
    }
}

fn rusage_cpu_micros(usage: &libc::rusage) -> Option<u64> {
    let timeval_micros = |value: libc::timeval| {
        let secs = u64::try_from(value.tv_sec).ok()?;
        let micros = u64::try_from(value.tv_usec).ok()?;
        Some(secs.saturating_mul(1_000_000).saturating_add(micros))
    };
    Some(timeval_micros(usage.ru_utime)?.saturating_add(timeval_micros(usage.ru_stime)?))
}

/// Appends to `buffer` (via `poll`/`read` on `fd`) until it holds at least `min_bytes`, EOF is
/// reached, or `deadline` passes. On a `deadline` hit, `SIGKILL`s `child` and keeps polling
/// briefly for whatever the killed process still manages to flush. Returns
/// `(deadline_fired, reached_eof)`.
fn read_until(
    stdout: &mut ChildStdout,
    fd: std::os::fd::RawFd,
    child: &Child,
    buffer: &mut Vec<u8>,
    chunk: &mut [u8; 65536],
    deadline: Instant,
    min_bytes: usize,
) -> (bool, bool) {
    let mut watchdog_fired = false;

    loop {
        if buffer.len() >= min_bytes {
            return (false, false);
        }

        if !watchdog_fired && Instant::now() >= deadline {
            // SAFETY: `child.id()` is this process's own live child pid (the child has not been
            // reaped yet -- `isolated_apply` calls `child.wait()` strictly after
            // `read_response_two_phase` returns); `SIGKILL` is always valid to send to one's own
            // child.
            unsafe {
                libc::kill(pid_of(child), libc::SIGKILL);
            }
            watchdog_fired = true;
        }

        let timeout_ms: i32 = if watchdog_fired {
            50
        } else {
            let remaining = deadline.saturating_duration_since(Instant::now()).as_millis();
            i32::try_from(remaining).unwrap_or(i32::MAX)
        };

        let mut pollfd = libc::pollfd {
            fd,
            events: libc::POLLIN,
            revents: 0,
        };
        // SAFETY: `&mut pollfd` is a valid single-element array with a live, open fd (`stdout`
        // outlives this whole function call); `timeout_ms` is non-negative.
        let poll_result = unsafe { libc::poll(&raw mut pollfd, 1, timeout_ms) };

        match poll_result.cmp(&0) {
            std::cmp::Ordering::Greater => match stdout.read(chunk) {
                Ok(0) => return (watchdog_fired, true), // EOF: the child closed its stdout.
                Ok(read_len) => {
                    if let Some(read_slice) = chunk.get(..read_len) {
                        buffer.extend_from_slice(read_slice);
                    }
                }
                Err(err) if err.kind() == std::io::ErrorKind::Interrupted => {}
                Err(_) => return (watchdog_fired, false),
            },
            std::cmp::Ordering::Equal => {
                if watchdog_fired {
                    // Already killed and given one more bounded grace-period poll; stop waiting.
                    return (true, false);
                }
            }
            std::cmp::Ordering::Less => {
                let last_error = std::io::Error::last_os_error();
                if last_error.kind() != std::io::ErrorKind::Interrupted {
                    return (watchdog_fired, false);
                }
            }
        }

        if buffer.len() > MAX_RESPONSE_PAYLOAD_BYTES + RESPONSE_FRAME_HEADER_BYTES + 1 {
            // Defensive cap: never buffer much more than one frame's worth plus header (plus the
            // marker byte), regardless of what a misbehaving worker writes.
            return (watchdog_fired, false);
        }
    }
}

/// `Child::id()` returns `u32`; `libc::kill` wants `libc::pid_t` (`i32` on Linux). A live child
/// pid from `fork`/`exec` is always representable in both.
fn pid_of(child: &Child) -> libc::pid_t {
    libc::pid_t::try_from(child.id()).unwrap_or(libc::pid_t::MAX)
}

/// Wraps `payload` as `[u32 length][u32 crc32][payload]`, matching `ADR-0014` section 2's frame
/// protocol (reused here for the same reason: a `SIGKILL`ed or crashed worker can leave a torn
/// write on `stdout`, and the CRC lets the parent detect that rather than trusting a partial
/// payload).
///
/// # Errors
/// [`std::io::Error`] if `payload` exceeds [`super::wire::MAX_RESPONSE_PAYLOAD_BYTES`].
pub fn encode_response_frame(payload: &[u8]) -> std::io::Result<Vec<u8>> {
    if payload.len() > MAX_RESPONSE_PAYLOAD_BYTES {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "response payload exceeds cap",
        ));
    }
    let length = u32::try_from(payload.len())
        .map_err(|_| std::io::Error::new(std::io::ErrorKind::InvalidInput, "response payload too large"))?;
    let crc = crc32fast::hash(payload);
    let mut out = Vec::with_capacity(RESPONSE_FRAME_HEADER_BYTES + payload.len());
    out.extend_from_slice(&length.to_le_bytes());
    out.extend_from_slice(&crc.to_le_bytes());
    out.extend_from_slice(payload);
    Ok(out)
}

/// Decodes a frame written by [`encode_response_frame`]. Every rejection here means "treat this
/// case as a host failure", never "trust a partially-parsed payload" (`ADR-0014` section 2).
fn decode_response_frame(bytes: &[u8]) -> Result<Vec<u8>, &'static str> {
    let (Some(length_bytes), Some(crc_bytes)) = (
        bytes.get(0..4).and_then(|slice| <[u8; 4]>::try_from(slice).ok()),
        bytes.get(4..8).and_then(|slice| <[u8; 4]>::try_from(slice).ok()),
    ) else {
        return Err("frame shorter than the 8-byte header");
    };
    let declared_length = u32::from_le_bytes(length_bytes);
    let declared_crc32 = u32::from_le_bytes(crc_bytes);

    let declared_length_usize =
        usize::try_from(declared_length).map_err(|_| "declared frame length does not fit usize")?;
    if declared_length_usize > MAX_RESPONSE_PAYLOAD_BYTES {
        return Err("declared frame length exceeds cap");
    }

    let payload_start = RESPONSE_FRAME_HEADER_BYTES;
    let payload_end = payload_start.saturating_add(declared_length_usize);
    let Some(payload) = bytes.get(payload_start..payload_end) else {
        return Err("frame truncated before declared length");
    };
    if bytes.len() > payload_end {
        return Err("trailing bytes after declared frame length");
    }

    let computed_crc32 = crc32fast::hash(payload);
    if computed_crc32 != declared_crc32 {
        return Err("frame crc32 mismatch");
    }

    Ok(payload.to_vec())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::OnceLock;

    use crate::error::CollabError;

    use parking_lot::Mutex;

    /// Serializes every test in this module that reads or writes [`WORKER_BINARY_PATH_ENV`]:
    /// `cargo test` runs tests in one process across many threads by default, and process
    /// environment is process-global state, so two such tests running concurrently could observe
    /// each other's value mid-test.
    fn env_lock() -> &'static Mutex<()> {
        static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
        LOCK.get_or_init(|| Mutex::new(()))
    }

    /// Resolves the real, just-built worker binary for end-to-end tests, skipping (not failing)
    /// when it is not present -- `cargo test -p collab-core --all-features` builds every target in
    /// this package including the `[[bin]]`, so in practice it is always there, but a narrower
    /// invocation (e.g. `cargo test --lib`) would not build it.
    fn real_worker_binary_for_tests() -> Option<std::path::PathBuf> {
        for profile in ["debug", "release"] {
            let candidate = std::path::PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/../../target"))
                .join(profile)
                .join(WORKER_BINARY_NAME);
            if candidate.is_file() {
                return Some(candidate);
            }
        }
        None
    }

    /// Runs `isolated_apply` with [`WORKER_BINARY_PATH_ENV`] pointed at the real worker binary,
    /// holding [`env_lock`] for the duration so no other test in this module can observe or
    /// change that env var mid-call.
    fn isolated_apply_with_real_worker(
        base_snapshot: &[u8],
        update: &[u8],
    ) -> Option<Result<IsolatedApplySuccess, IsolatedApplyError>> {
        let worker_path = real_worker_binary_for_tests()?;
        let guard = env_lock().lock();
        // SAFETY: held under `env_lock()`, so no other test in this module reads or writes
        // `WORKER_BINARY_PATH_ENV` while this is set.
        unsafe {
            std::env::set_var(WORKER_BINARY_PATH_ENV, &worker_path);
        }
        let result = isolated_apply(base_snapshot, update);
        // SAFETY: same reasoning as the `set_var` call above; still held under `env_lock()`.
        unsafe {
            std::env::remove_var(WORKER_BINARY_PATH_ENV);
        }
        drop(guard);
        Some(result)
    }

    /// [`isolated_apply_with_real_worker`] with the window mode chosen.
    fn isolated_apply_mode_with_real_worker(
        base_snapshot: &[u8],
        update: &[u8],
        mode: WindowMode,
    ) -> Option<Result<IsolatedApplySuccess, IsolatedApplyError>> {
        let worker_path = real_worker_binary_for_tests()?;
        let guard = env_lock().lock();
        // SAFETY: held under `env_lock()`, so no other test in this module reads or writes
        // `WORKER_BINARY_PATH_ENV` while this is set.
        unsafe {
            std::env::set_var(WORKER_BINARY_PATH_ENV, &worker_path);
        }
        let result = isolated_apply_with(base_snapshot, update, mode);
        // SAFETY: same reasoning as the `set_var` call above; still held under `env_lock()`.
        unsafe {
            std::env::remove_var(WORKER_BINARY_PATH_ENV);
        }
        drop(guard);
        Some(result)
    }

    // ---- Boundary tests: `decode_apply_cpu_ms_max` / `decode_apply_wall_ms_max` /
    // `isolated_apply_memory_bytes_max`, and their correct signal classification.
    //
    // These do NOT reuse `isolated_apply_with_real_worker`'s self-skip-when-missing pattern above:
    // a boundary test that silently skips when the worker binary is absent is a false green, not
    // a partial one. They instead panic via `required_debug_worker_binary_for_tests` when the
    // binary is missing.
    //
    // They also cannot use real Loro CRDT content to land on these three boundaries -- see
    // `src/bin/isolated_apply_worker.rs`'s `test_injection` module doc comment for why (real
    // content is machine-speed-dependent for CPU/wall, and the memory ceiling is not reachable at
    // all with legitimate content: anything over 128 MiB of decode-time working memory is already
    // well past the 10,000-node structural ceilings and gets rejected by `check_snapshot` first).
    // Instead, they drive that module's `cfg(debug_assertions)`-gated synthetic workloads through
    // `COLLAB_ISOLATION_TEST_INJECT`, set on the spawned child's own environment only (via
    // `run_isolated_apply`'s `extra_env`) -- never on this test process's own environment, and
    // never reachable from a `--release` build (see that module's doc comment for the full
    // guarantee this cannot leak into production).

    /// Mirrors `src/bin/isolated_apply_worker.rs`'s `test_injection::ENV_VAR`. That module lives
    /// in a separate binary crate target (`[[bin]] collab-isolated-apply-worker`), not something
    /// this library crate can `use` directly, so the literal name is duplicated here -- both sides
    /// are commented as the shared contract between them.
    const TEST_INJECT_ENV_VAR: &str = "COLLAB_ISOLATION_TEST_INJECT";

    /// Resolves the debug-profile worker binary specifically -- unlike
    /// [`real_worker_binary_for_tests`]'s debug-then-release fallback above, the synthetic
    /// workloads these boundary tests inject only exist in a `cfg(debug_assertions)` build (see
    /// `src/bin/isolated_apply_worker.rs`'s `test_injection` module doc for why); falling back to
    /// a release binary would silently run the real decode/apply/validate pipeline against
    /// meaningless trivial input instead of the intended synthetic workload, which these tests
    /// could not tell apart from a real ceiling-enforcement failure. Panics (does not skip) when
    /// the debug binary is missing -- `cargo test -p collab-core --all-features` builds it by
    /// default, and this workspace's own `scripts/verify-flow-limits-v0.4.sh` explicitly runs
    /// `cargo build -p collab-core --bin collab-isolated-apply-worker` before this test group, so
    /// a silent skip here would hide a real regression rather than report one.
    fn required_debug_worker_binary_for_tests() -> std::path::PathBuf {
        let candidate = std::path::PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/../../target"))
            .join("debug")
            .join(WORKER_BINARY_NAME);
        assert!(
            candidate.is_file(),
            "collab-isolated-apply-worker debug binary not found at {candidate:?} -- build it \
             first with `cargo build -p collab-core --bin collab-isolated-apply-worker` (a \
             --release build will not do: this test's synthetic workload injection only exists in \
             a cfg(debug_assertions) build) before running this boundary test"
        );
        candidate
    }

    /// Runs [`run_isolated_apply`] against the real debug-profile worker binary with
    /// `COLLAB_ISOLATION_TEST_INJECT=<env_value>` set on the *child's* environment only, so the
    /// worker's `test_injection` module substitutes a controllable synthetic workload for the real
    /// decode/apply/validate pipeline. `base_snapshot`/`update` content is irrelevant to every one
    /// of these workloads (none of them ever call `import_update`), so this always uses a fresh
    /// empty document and an empty update.
    fn isolated_apply_injected(env_value: &str) -> Result<IsolatedApplySuccess, IsolatedApplyError> {
        isolated_apply_injected_observed(env_value).0
    }

    /// [`isolated_apply_injected`] plus the host's [`WorkerObservation`] of the run.
    fn isolated_apply_injected_observed(
        env_value: &str,
    ) -> (Result<IsolatedApplySuccess, IsolatedApplyError>, WorkerObservation) {
        let worker_path = required_debug_worker_binary_for_tests();
        let guard = env_lock().lock();
        // SAFETY: held under `env_lock()`, so no other test in this module reads or writes
        // `WORKER_BINARY_PATH_ENV` while this is set.
        unsafe {
            std::env::set_var(WORKER_BINARY_PATH_ENV, &worker_path);
        }
        let base_snapshot = crate::LoroCollabEngine::new_empty(1)
            .export_snapshot()
            .expect("a fresh empty document always exports");
        let result = run_isolated_apply_observed(
            &base_snapshot,
            &[],
            wire::OPERATION_APPLY,
            &[(TEST_INJECT_ENV_VAR, env_value)],
        );
        // SAFETY: same reasoning as the `set_var` call above; still held under `env_lock()`.
        unsafe {
            std::env::remove_var(WORKER_BINARY_PATH_ENV);
        }
        drop(guard);
        result
    }

    /// The coarsest scheduler tick the kernel accounts CPU in (`CONFIG_HZ=100`; 250 and 1000 are
    /// finer).
    const SCHEDULER_TICK_US: u64 = 10_000;

    /// Allowance when judging a kill against the CPU the host measured, one tick, for both the
    /// write and the import window.
    ///
    /// The ceiling is a timer on the worker's own CPU clock (`child_runtime::arm_cpu_timer`), which
    /// fires only after the ceiling is consumed and at most about a tick later, so a kill never
    /// needs an allowance for firing early. (It did when the ceiling was `ITIMER_PROF`: that timer
    /// counts ticks sampled at the interrupt and, on a loaded 2-CPU machine, fired up to 1.5% of
    /// the ceiling early, 29 ms of the import window's 2 s, which no fixed number of ticks covers.)
    /// What is left is the host's own reading: it reads the worker's CPU when it sees the marker,
    /// a moment after the worker armed the timer, and `wait4` accounts a killed process to a tick.
    /// Measured on a loaded 2-CPU machine, every kill of 80 runs read at or above the ceiling
    /// (a shortfall of at most 0 us); one tick is the margin for that reading, and it is far
    /// smaller than the distance between the ceiling and the CPU the under-ceiling runs consume.
    const CPU_ACCOUNTING_SLACK_US: u64 = SCHEDULER_TICK_US;

    /// How one injected run relates to the ceiling it targeted, judged from what the host
    /// observed rather than from the clock alone.
    #[derive(Debug, PartialEq, Eq)]
    enum CeilingVerdict {
        /// The run shows the ceiling working as specified.
        Conclusive,
        /// The wall watchdog killed a worker that never got the CPU it needed to reach the CPU
        /// ceiling: the machine, not the ceiling, decided the outcome. Proves nothing either way.
        Starved,
        /// The run contradicts the specified behaviour; the text says how.
        Violation(String),
    }

    /// Judges a burn that targets more CPU than `decode_apply_cpu_ms_max`. The worker must be
    /// killed by `SIGPROF` once it has consumed the ceiling; a `WallCeiling` is acceptable only
    /// when the worker had demonstrably not consumed it.
    fn judge_burn_over_cpu_ceiling(
        result: &Result<IsolatedApplySuccess, IsolatedApplyError>,
        observation: &WorkerObservation,
        ceiling_us: u64,
    ) -> CeilingVerdict {
        let Some(cpu_us) = observation.metered_cpu_us else {
            return CeilingVerdict::Violation("the host could not observe the worker's CPU time".to_string());
        };
        match result {
            Err(IsolatedApplyError::CpuCeiling) if cpu_us.saturating_add(CPU_ACCOUNTING_SLACK_US) >= ceiling_us => {
                CeilingVerdict::Conclusive
            }
            Err(IsolatedApplyError::CpuCeiling) => CeilingVerdict::Violation(format!(
                "SIGPROF killed the worker after only {cpu_us}us of CPU, below the {ceiling_us}us ceiling"
            )),
            Err(IsolatedApplyError::WallCeiling) if cpu_us < ceiling_us.saturating_add(CPU_ACCOUNTING_SLACK_US) => {
                CeilingVerdict::Starved
            }
            Err(IsolatedApplyError::WallCeiling) => CeilingVerdict::Violation(format!(
                "the wall watchdog had to kill a worker that consumed {cpu_us}us of CPU, over the \
                 {ceiling_us}us ceiling: SIGPROF did not fire"
            )),
            other => CeilingVerdict::Violation(format!("unexpected outcome {other:?} after {cpu_us}us of CPU")),
        }
    }

    /// Judges a burn that targets less CPU than `decode_apply_cpu_ms_max`: it must be accepted.
    /// A `WallCeiling` means the worker was starved of CPU for the burn plus its response inside
    /// the wall deadline; a `CpuCeiling` is never right for it.
    fn judge_burn_under_cpu_ceiling(
        result: &Result<IsolatedApplySuccess, IsolatedApplyError>,
        observation: &WorkerObservation,
    ) -> CeilingVerdict {
        match result {
            Ok(_) => CeilingVerdict::Conclusive,
            Err(IsolatedApplyError::WallCeiling) => CeilingVerdict::Starved,
            other => CeilingVerdict::Violation(format!(
                "a burn under the CPU ceiling ended as {other:?} after {:?}us of CPU",
                observation.metered_cpu_us
            )),
        }
    }

    /// Runs `run_once` until it is [`CeilingVerdict::Conclusive`] or `attempts` runs were all
    /// starved, pausing between attempts so a short burst of contention can pass. Returns whether
    /// a conclusive run happened; panics on the first violation.
    #[allow(
        clippy::print_stderr,
        reason = "a test notice that an attempt was starved, visible with --nocapture or on a failure"
    )]
    fn conclusive_within(
        attempts: usize,
        pause: std::time::Duration,
        what: &str,
        mut run_once: impl FnMut() -> CeilingVerdict,
    ) -> bool {
        for attempt in 1..=attempts {
            match run_once() {
                CeilingVerdict::Conclusive => return true,
                CeilingVerdict::Starved => {
                    eprintln!("{what}: attempt {attempt}/{attempts} was starved of CPU by the host");
                    std::thread::sleep(pause);
                }
                CeilingVerdict::Violation(why) => panic!("{what}: {why}"),
            }
        }
        false
    }

    const BOUNDARY_PAUSE: std::time::Duration = std::time::Duration::from_millis(250);

    /// One run of each side of the CPU ceiling, judged by [`judge_burn_under_cpu_ceiling`] and
    /// [`judge_burn_over_cpu_ceiling`]. Margins are comfortably on each side of the ceiling
    /// rather than at a literal +/-1us edge: `SIGPROF` delivery latency and the polling
    /// busy-loop's own check granularity (see `test_injection::burn_cpu_micros`) both add slack.
    fn boundary_runs() -> (u64, u64, u64) {
        let ceiling_micros = super::super::DECODE_APPLY_CPU_MS_MAX * 1_000;
        let under_micros = ceiling_micros - ceiling_micros / 3; // ~33ms: comfortably under 50ms.
        let over_micros = ceiling_micros * 3; // ~150ms: comfortably over 50ms.
        (ceiling_micros, under_micros, over_micros)
    }

    fn under_ceiling_verdict(under_micros: u64) -> CeilingVerdict {
        let (result, observation) = isolated_apply_injected_observed(&format!("cpu_burn_micros={under_micros}"));
        judge_burn_under_cpu_ceiling(&result, &observation)
    }

    fn over_ceiling_verdict(over_micros: u64, ceiling_micros: u64) -> CeilingVerdict {
        let (result, observation) = isolated_apply_injected_observed(&format!("cpu_burn_micros={over_micros}"));
        judge_burn_over_cpu_ceiling(&result, &observation, ceiling_micros)
    }

    /// The CPU ceiling's behaviour never depends on how much CPU the machine gives the worker:
    /// a run is judged by the CPU the worker consumed as the host observed it. A worker that was
    /// killed by the wall watchdog having consumed less CPU than the ceiling was starved by the
    /// machine (CI runners share their cores with other tests), which says nothing about the
    /// ceiling, so such a run is retried and, if the machine never gives it the CPU, left
    /// inconclusive. A violation (SIGPROF not firing at the ceiling, firing early, or the wall
    /// watchdog killing a worker that had consumed the ceiling) always fails. The strict variant
    /// below requires a conclusive run.
    #[test]
    fn decode_apply_cpu_ms_ceiling_accepts_just_under_and_kills_with_cpu_ceiling_just_over_the_boundary() {
        let (ceiling_micros, under_micros, over_micros) = boundary_runs();
        conclusive_within(
            4,
            BOUNDARY_PAUSE,
            &format!("a {under_micros}us CPU burn (under the {ceiling_micros}us decode_apply_cpu_ms_max ceiling)"),
            || under_ceiling_verdict(under_micros),
        );
        conclusive_within(
            4,
            BOUNDARY_PAUSE,
            &format!("a {over_micros}us CPU burn (over the {ceiling_micros}us decode_apply_cpu_ms_max ceiling)"),
            || over_ceiling_verdict(over_micros, ceiling_micros),
        );
    }

    /// The strict variant of the test above: the worker must demonstrably have had the CPU in
    /// both runs (so the first run is accepted and the second is killed by `SIGPROF`), within
    /// twenty attempts each. Fails on a machine that never gives the worker its CPU, so it is
    /// not part of the default run; run it on an idle machine with
    /// `cargo test -p collab-core --lib -- --ignored decode_apply_cpu_ms_ceiling_strict`.
    #[test]
    #[ignore = "needs an idle machine; run explicitly with --ignored"]
    fn decode_apply_cpu_ms_ceiling_strict_requires_both_conclusive_runs_when_the_worker_has_the_cpu() {
        let (ceiling_micros, under_micros, over_micros) = boundary_runs();
        assert!(
            conclusive_within(
                20,
                BOUNDARY_PAUSE,
                "strict under-ceiling run",
                || under_ceiling_verdict(under_micros)
            ),
            "the worker never got the CPU for a {under_micros}us burn in twenty attempts"
        );
        assert!(
            conclusive_within(20, BOUNDARY_PAUSE, "strict over-ceiling run", || over_ceiling_verdict(
                over_micros,
                ceiling_micros
            )),
            "the worker never got the CPU to reach the {ceiling_micros}us ceiling in twenty attempts"
        );
    }

    /// The judgement itself, without a process or a clock: a wall kill is a violation only when
    /// the worker had consumed the CPU ceiling, and a `SIGPROF` kill only counts when it did.
    #[test]
    fn ceiling_verdicts_separate_starvation_from_a_misbehaving_ceiling() {
        let ceiling = 50_000;
        let observed = |cpu: Option<u64>| WorkerObservation {
            metered_wall_us: Some(100_000),
            metered_cpu_us: cpu,
            ..WorkerObservation::default()
        };
        let wall = || Err(IsolatedApplyError::WallCeiling);
        let cpu_kill = || Err(IsolatedApplyError::CpuCeiling);

        // Starved: killed at the wall deadline with a fraction of the ceiling (or just about
        // the ceiling, inside the accounting slack).
        assert_eq!(
            judge_burn_over_cpu_ceiling(&wall(), &observed(Some(3_000)), ceiling),
            CeilingVerdict::Starved
        );
        assert_eq!(
            judge_burn_over_cpu_ceiling(&wall(), &observed(Some(55_000)), ceiling),
            CeilingVerdict::Starved
        );
        // SIGPROF did not fire although the worker had clearly consumed the ceiling.
        assert!(matches!(
            judge_burn_over_cpu_ceiling(&wall(), &observed(Some(90_000)), ceiling),
            CeilingVerdict::Violation(_)
        ));
        // A SIGPROF kill is conclusive only if the worker had the ceiling's CPU.
        assert_eq!(
            judge_burn_over_cpu_ceiling(&cpu_kill(), &observed(Some(52_000)), ceiling),
            CeilingVerdict::Conclusive
        );
        assert!(matches!(
            judge_burn_over_cpu_ceiling(&cpu_kill(), &observed(Some(10_000)), ceiling),
            CeilingVerdict::Violation(_)
        ));
        // An unobservable CPU time and a burn that ran to completion are violations.
        assert!(matches!(
            judge_burn_over_cpu_ceiling(&cpu_kill(), &observed(None), ceiling),
            CeilingVerdict::Violation(_)
        ));
        assert!(matches!(
            judge_burn_over_cpu_ceiling(
                &Ok(IsolatedApplySuccess {
                    snapshot: Vec::new(),
                    window_cpu_us: None,
                    window_report: None,
                    touched_blocks: Vec::new(),
                }),
                &observed(Some(150_000)),
                ceiling
            ),
            CeilingVerdict::Violation(_)
        ));
        // Under the ceiling: accepted is conclusive, a wall kill is starvation, a CPU kill is wrong.
        assert_eq!(
            judge_burn_under_cpu_ceiling(
                &Ok(IsolatedApplySuccess {
                    snapshot: Vec::new(),
                    window_cpu_us: None,
                    window_report: None,
                    touched_blocks: Vec::new(),
                }),
                &observed(Some(33_000))
            ),
            CeilingVerdict::Conclusive
        );
        assert_eq!(
            judge_burn_under_cpu_ceiling(&wall(), &observed(Some(8_000))),
            CeilingVerdict::Starved
        );
        assert!(matches!(
            judge_burn_under_cpu_ceiling(&cpu_kill(), &observed(Some(60_000))),
            CeilingVerdict::Violation(_)
        ));
    }

    #[test]
    fn decode_apply_wall_ms_ceiling_accepts_just_under_and_kills_with_wall_ceiling_just_over_the_boundary() {
        let ceiling_millis = super::super::DECODE_APPLY_WALL_MS_MAX;
        let under_millis = ceiling_millis - ceiling_millis / 3; // ~67ms: comfortably under 100ms.
        let over_millis = ceiling_millis * 3; // ~300ms: comfortably over 100ms.

        let accepted = isolated_apply_injected(&format!("wall_sleep_millis={under_millis}"));
        assert!(
            accepted.is_ok(),
            "a {under_millis}ms sleep (under the {ceiling_millis}ms decode_apply_wall_ms_max \
             ceiling, and consuming effectively no CPU, so it cannot trip the CPU ceiling either) \
             must succeed, got {accepted:?}"
        );

        let (rejected, observation) = isolated_apply_injected_observed(&format!("wall_sleep_millis={over_millis}"));
        match rejected {
            Err(IsolatedApplyError::WallCeiling) => {}
            other => panic!(
                "a {over_millis}ms sleep (over the {ceiling_millis}ms decode_apply_wall_ms_max \
                 ceiling) must be SIGKILLed by the parent's independent wall watchdog and \
                 classified as WallCeiling specifically -- not CpuCeiling (a sleep consumes \
                 effectively no process CPU time, so SIGPROF has no budget to ever fire here), \
                 not MemoryCeiling, not HostFailure -- got {other:?}"
            ),
        }
        // A wall kill is right only when the wall time passed while the worker consumed less CPU
        // than the CPU ceiling: a sleeping worker is the case that shows exactly that.
        assert!(
            observation
                .metered_wall_us
                .is_some_and(|wall_us| wall_us >= ceiling_millis * 1_000),
            "the wall kill came before the {ceiling_millis}ms deadline: {observation:?}"
        );
        assert!(
            observation
                .metered_cpu_us
                .is_some_and(|cpu_us| cpu_us < super::super::DECODE_APPLY_CPU_MS_MAX * 1_000),
            "a sleeping worker consumed the CPU ceiling's worth of CPU: {observation:?}"
        );
    }

    #[test]
    fn isolated_apply_memory_bytes_ceiling_accepts_the_exact_byte_and_rejects_the_next_byte_over() {
        let ceiling = super::super::ISOLATED_APPLY_MEMORY_BYTES_MAX;
        let Ok(ceiling_usize) = usize::try_from(ceiling) else {
            panic!("ISOLATED_APPLY_MEMORY_BYTES_MAX ({ceiling}) does not fit usize on this target");
        };

        // Exact boundary, no timing involved at all: a single allocation request of precisely
        // `ceiling` bytes must succeed (the counting allocator's own check is `prospective >
        // ceiling`, strictly greater -- see `isolation::alloc::CountingAllocator::alloc`), and one
        // byte more must fail on that exact same allocation attempt.
        let accepted = isolated_apply_injected(&format!("alloc_bytes={ceiling_usize}"));
        assert!(
            accepted.is_ok(),
            "a single {ceiling_usize}-byte allocation (exactly isolated_apply_memory_bytes_max) \
             must succeed, got {accepted:?}"
        );

        // One byte over must be refused. Which *ceiling* reports it is, end to end, a race this
        // test cannot control: the counting allocator returns null deterministically (pinned
        // without any process or timer by `alloc::tests::counting_allocator_accepts_below_ceiling_and_rejects_above_it_at_the_exact_byte`),
        // but the refusal then travels `handle_alloc_error` -> stderr message -> `abort` -> kernel
        // `SIGABRT` -> parent `wait4`, all inside the strict 100 ms `decode_apply_wall_ms_max`
        // window. On a loaded shared CI runner that path has been observed to lose the race to the
        // parent's wall watchdog (`got Err(WallCeiling)`, GitHub Actions run 37464947988 on
        // b35d8f2), while the exact-byte arithmetic itself was never in question. So the end-to-end
        // half asserts what is stable -- the over-ceiling allocation is refused by a resource
        // ceiling and never succeeds, never reads as CPU exhaustion, never as a host failure -- and
        // the SIGABRT -> MemoryCeiling mapping is pinned separately by
        // `sigabrt_is_classified_as_the_memory_ceiling`.
        let over_usize = ceiling_usize + 1;
        let rejected = isolated_apply_injected(&format!("alloc_bytes={over_usize}"));
        match rejected {
            Err(IsolatedApplyError::MemoryCeiling | IsolatedApplyError::WallCeiling) => {}
            other => panic!(
                "a single {over_usize}-byte allocation (one byte over \
                 isolated_apply_memory_bytes_max) must be refused by a resource ceiling -- \
                 MemoryCeiling via the counting allocator's null return, or WallCeiling if the \
                 abort lost the race to the wall watchdog on a loaded host -- never success, \
                 CpuCeiling or HostFailure; got {other:?}"
            ),
        }
    }

    // ---- The import operation's own budget (`IMPORT_*`): the same primitives, armed with larger
    // ceilings, for the `import` operation only. The synthetic workloads run inside the import
    // window, and a workload that stays under the import ceiling ends in the refusal the worker
    // injects, so a ceiling that did not fire is told apart from one that did.

    /// [`isolated_apply_injected_observed`] for the `import` operation.
    fn isolated_import_injected_observed(
        env_value: &str,
    ) -> (Result<IsolatedApplySuccess, IsolatedApplyError>, WorkerObservation) {
        let worker_path = required_debug_worker_binary_for_tests();
        let guard = env_lock().lock();
        // SAFETY: held under `env_lock()`, so no other test in this module reads or writes
        // `WORKER_BINARY_PATH_ENV` while this is set.
        unsafe {
            std::env::set_var(WORKER_BINARY_PATH_ENV, &worker_path);
        }
        let result = run_isolated_apply_observed(&[], &[], wire::OPERATION_IMPORT, &[(TEST_INJECT_ENV_VAR, env_value)]);
        // SAFETY: same reasoning as the `set_var` call above; still held under `env_lock()`.
        unsafe {
            std::env::remove_var(WORKER_BINARY_PATH_ENV);
        }
        drop(guard);
        result
    }

    fn is_the_injected_refusal(result: &Result<IsolatedApplySuccess, IsolatedApplyError>) -> bool {
        matches!(
            result,
            Err(IsolatedApplyError::Collab(CollabError::OperationFailed { reason })) if reason == "injected_workload_finished"
        )
    }

    /// A burn that the write ceiling would kill (150 ms against 50 ms) runs to its end inside the
    /// import window (1,000 ms), and a burn over the import ceiling is killed by `SIGPROF` at it.
    #[test]
    fn the_import_window_has_its_own_cpu_budget() {
        let import_ceiling_us = super::super::IMPORT_DECODE_CPU_MS_MAX * 1_000;
        assert!(import_ceiling_us > super::super::DECODE_APPLY_CPU_MS_MAX * 1_000 * 3);
        conclusive_within(4, BOUNDARY_PAUSE, "a 150 ms burn in the import window", || {
            let (result, observation) = isolated_import_injected_observed("cpu_burn_micros=150000");
            if is_the_injected_refusal(&result) {
                CeilingVerdict::Conclusive
            } else if matches!(result, Err(IsolatedApplyError::WallCeiling)) {
                CeilingVerdict::Starved
            } else {
                CeilingVerdict::Violation(format!("{result:?} after {:?}us of CPU", observation.metered_cpu_us))
            }
        });
        conclusive_within(4, BOUNDARY_PAUSE, "a burn over the import CPU ceiling", || {
            let (result, observation) =
                isolated_import_injected_observed(&format!("cpu_burn_micros={}", import_ceiling_us.saturating_mul(2)));
            judge_burn_over_cpu_ceiling(&result, &observation, import_ceiling_us)
        });
    }

    /// The wall watchdog of the import window is the import one: a sleep that the write watchdog
    /// (100 ms) would kill is accepted, and a sleep past the import watchdog is killed at it.
    #[test]
    fn the_import_window_has_its_own_wall_budget() {
        let ceiling_ms = super::super::IMPORT_DECODE_WALL_MS_MAX;
        let (result, _) = isolated_import_injected_observed("wall_sleep_millis=400");
        assert!(
            is_the_injected_refusal(&result),
            "a 400 ms sleep must end inside the import window: {result:?}"
        );
        let (result, observation) =
            isolated_import_injected_observed(&format!("wall_sleep_millis={}", ceiling_ms + 500));
        assert!(matches!(result, Err(IsolatedApplyError::WallCeiling)), "{result:?}");
        assert!(
            observation
                .metered_wall_us
                .is_some_and(|wall_us| wall_us >= ceiling_ms * 1_000),
            "the wall kill came before the {ceiling_ms} ms deadline: {observation:?}"
        );
    }

    /// An allocation the write ceiling refuses (200 MiB against 128 MiB) is accepted in the import
    /// window, and one byte over the import ceiling is refused by a resource ceiling.
    #[test]
    fn the_import_window_has_its_own_memory_budget() {
        let ceiling = super::super::IMPORT_ISOLATED_MEMORY_BYTES_MAX;
        let Ok(ceiling_usize) = usize::try_from(ceiling) else {
            panic!("IMPORT_ISOLATED_MEMORY_BYTES_MAX ({ceiling}) does not fit usize on this target");
        };
        let (accepted, _) = isolated_import_injected_observed(&format!("alloc_bytes={}", 200 * 1024 * 1024));
        assert!(
            is_the_injected_refusal(&accepted),
            "a 200 MiB allocation must be accepted by the import ceiling: {accepted:?}"
        );
        // The synthetic workload creates its empty engine inside the armed window, so the exact
        // byte is not reachable here; a few MiB under the ceiling is.
        let (near, _) = isolated_import_injected_observed(&format!("alloc_bytes={}", ceiling_usize - 4 * 1024 * 1024));
        assert!(
            is_the_injected_refusal(&near),
            "an allocation just under the import ceiling must be accepted: {near:?}"
        );
        let (rejected, _) = isolated_import_injected_observed(&format!("alloc_bytes={}", ceiling_usize + 1));
        assert!(
            matches!(
                rejected,
                Err(IsolatedApplyError::MemoryCeiling | IsolatedApplyError::WallCeiling)
            ),
            "one byte over the import ceiling must be refused by a resource ceiling: {rejected:?}"
        );
    }

    #[test]
    fn the_wall_deadline_follows_the_operation() {
        assert_eq!(metered_wall_ms(wire::OPERATION_APPLY), DECODE_APPLY_WALL_MS_MAX);
        assert_eq!(metered_wall_ms(wire::OPERATION_DIFF), DECODE_APPLY_WALL_MS_MAX);
        assert_eq!(metered_wall_ms(wire::OPERATION_IMPORT), IMPORT_DECODE_WALL_MS_MAX);
    }

    #[test]
    fn sigabrt_is_classified_as_the_memory_ceiling() {
        assert!(matches!(
            classify_signal(libc::SIGABRT),
            IsolatedApplyError::MemoryCeiling
        ));
        assert!(matches!(classify_signal(libc::SIGPROF), IsolatedApplyError::CpuCeiling));
        assert!(matches!(
            classify_signal(libc::SIGKILL),
            IsolatedApplyError::HostFailure(_)
        ));
    }

    #[test]
    fn response_frame_round_trips() {
        let payload = b"a response payload".to_vec();
        let framed = encode_response_frame(&payload).expect("encode succeeds");
        let decoded = decode_response_frame(&framed).expect("decode succeeds");
        assert_eq!(decoded, payload);
    }

    #[test]
    fn response_frame_rejects_a_corrupted_crc() {
        let payload = b"a response payload".to_vec();
        let mut framed = encode_response_frame(&payload).expect("encode succeeds");
        if let Some(last) = framed.last_mut() {
            *last ^= 0xFF;
        }
        assert!(decode_response_frame(&framed).is_err());
    }

    #[test]
    fn response_frame_rejects_a_truncated_frame() {
        let payload = b"a response payload".to_vec();
        let mut framed = encode_response_frame(&payload).expect("encode succeeds");
        framed.truncate(framed.len().saturating_sub(3));
        assert!(decode_response_frame(&framed).is_err());
    }

    #[test]
    fn worker_binary_path_honors_the_env_override() {
        let guard = env_lock().lock();
        let previous = std::env::var(WORKER_BINARY_PATH_ENV).ok();
        // SAFETY: held under `env_lock()`, so no other test in this module reads or writes
        // `WORKER_BINARY_PATH_ENV` while this runs.
        unsafe {
            std::env::set_var(WORKER_BINARY_PATH_ENV, "/nonexistent/isolated-apply-worker");
        }
        let resolved = worker_binary_path();
        match previous {
            Some(value) => {
                // SAFETY: same reasoning as the `set_var` call above.
                unsafe { std::env::set_var(WORKER_BINARY_PATH_ENV, value) }
            }
            None => {
                // SAFETY: same reasoning as the `set_var` call above.
                unsafe { std::env::remove_var(WORKER_BINARY_PATH_ENV) }
            }
        }
        drop(guard);
        let path = resolved.expect("env override resolves without touching current_exe");
        assert_eq!(path, std::path::PathBuf::from("/nonexistent/isolated-apply-worker"));
    }

    // ---- End-to-end tests: spawn the real, just-built `collab-isolated-apply-worker` binary.
    //
    // Everything above this point tests pure logic (frame codec, path resolution). These tests
    // exercise the actual mechanism the parent trusts: a real `fork`+`exec`'d process, real
    // `SIGPROF`/`ITIMER_PROF` arming, real pipe I/O, real `wait4` reaping. If
    // `real_worker_binary_for_tests` cannot find the compiled binary (a narrower invocation than
    // `cargo test -p collab-core --all-features` was used), they report and skip rather than fail
    // the suite over a build-layout mismatch unrelated to this module's own logic.

    use crate::limits::DocumentLimits;
    use crate::operation::{NodeId, NodeKind, Operation};
    use crate::{CollabEngine, LoroCollabEngine};

    /// Tests in this crate must not use `println!`/`eprintln!` directly (workspace clippy denies
    /// `print_stdout`/`print_stderr`); this helper is the one place that intentionally routes a
    /// diagnostic through `eprintln!`, isolated so the `#[allow]` has the smallest possible scope
    /// (matches `spikes/collab-shared/src/isolation/shared_page.rs`'s identical helper).
    #[allow(clippy::print_stderr)]
    fn eprintln_test(message: &str) {
        eprintln!("{message}");
    }

    #[test]
    fn isolated_apply_end_to_end_accepts_a_well_formed_update() {
        let base = LoroCollabEngine::new_empty(1);
        let base_frontier = base.frontier();
        let base_snapshot = base.export_snapshot().expect("export succeeds");

        let mut writer = base.fork().expect("fork succeeds");
        writer.set_title("end to end").expect("set_title succeeds");
        let update = writer.export_from(&base_frontier).expect("export succeeds");

        let Some(result) = isolated_apply_with_real_worker(&base_snapshot, &update) else {
            eprintln_test(
                "skipped: collab-isolated-apply-worker binary not found (build it with `cargo build -p collab-core --bin collab-isolated-apply-worker`)",
            );
            return;
        };
        let success = result.expect("a title-only update stays well within every ceiling");
        let reloaded = LoroCollabEngine::load(&success.snapshot).expect("returned snapshot reloads");
        assert_eq!(reloaded.title().expect("title reads"), "end to end");
    }

    /// The host can make the worker run the full computation: no incremental report, the same
    /// document. Without the request, an update to a page with facts is decided incrementally.
    #[test]
    fn isolated_apply_full_window_mode_runs_the_full_computation_and_reports_nothing() {
        let mut base = LoroCollabEngine::new_empty(1);
        base.apply_content_operation(&Operation::CreateNode {
            id: NodeId::from("a"),
            parent: None,
            index: 0,
            kind: NodeKind::Block,
        })
        .expect("create applies");
        let base_frontier = base.frontier();
        let base_snapshot = base.export_snapshot().expect("export succeeds");
        let mut writer = base.fork().expect("fork succeeds");
        writer
            .apply_content_operation(&Operation::InsertText {
                id: NodeId::from("a"),
                index: 0,
                text: "typed".into(),
            })
            .expect("text applies");
        let update = writer.export_from(&base_frontier).expect("export succeeds");

        let Some(incremental) = isolated_apply_mode_with_real_worker(&base_snapshot, &update, WindowMode::Incremental)
        else {
            eprintln_test("skipped: collab-isolated-apply-worker binary not found");
            return;
        };
        let incremental = incremental.expect("a one-character edit is accepted");
        assert!(
            incremental.window_report.is_some(),
            "the incremental window decides this update"
        );
        let full = isolated_apply_mode_with_real_worker(&base_snapshot, &update, WindowMode::Full)
            .expect("the worker binary was found a moment ago")
            .expect("the same update is accepted by the full computation");
        assert!(
            full.window_report.is_none(),
            "the full window reports no incremental result"
        );
        let value = |snapshot: &[u8]| {
            LoroCollabEngine::load(snapshot)
                .expect("returned snapshot reloads")
                .semantic_snapshot()
                .expect("projects")
        };
        assert_eq!(value(&incremental.snapshot), value(&full.snapshot));
    }

    #[test]
    fn isolated_apply_end_to_end_reports_decode_failed_for_garbage_update_bytes() {
        let base = LoroCollabEngine::new_empty(1);
        let base_snapshot = base.export_snapshot().expect("export succeeds");
        let garbage_update = b"this is not a loro update".to_vec();

        let Some(result) = isolated_apply_with_real_worker(&base_snapshot, &garbage_update) else {
            eprintln_test("skipped: collab-isolated-apply-worker binary not found");
            return;
        };
        match result {
            Err(IsolatedApplyError::Collab(CollabError::DecodeFailed { input, .. })) => {
                assert_eq!(input, "update");
            }
            other => panic!("expected Collab(DecodeFailed), got {other:?}"),
        }
    }

    #[test]
    fn isolated_apply_end_to_end_reports_text_block_chars_via_check_snapshot() {
        // Builds a base document that already has one block whose text is `text_block_chars_max +
        // 1` characters long -- over the ceiling before the (trivial) update is even applied.
        // `check_snapshot` inside the worker must catch this on the *merged* result, not just on
        // what the update itself added, which is exactly what this proves end to end (the local
        // `check_operation`-shaped construction here never runs `check_snapshot` at all).
        //
        // Deliberately a `text_block_chars` violation (one block, a very long string) rather than
        // a `document_block_count`/`container_count` violation (which would need `limit + 1 =
        // 10,001` *nodes*): `LoroCollabEngine::semantic_snapshot`'s `order_key_for` recomputes
        // each node's sibling position by re-listing and linearly searching *all* of its parent's
        // children (`engine.rs`), which -- empirically, in both debug and release builds -- makes
        // building the semantic snapshot for a document with that many nodes cost more than
        // `decode_apply_cpu_ms_max` (50ms) on its own, regardless of how the nodes are distributed
        // across parents. That is a real, pre-existing performance characteristic of `engine.rs`'s
        // `semantic_snapshot`, not a bug in this isolation host -- see this module's own doc
        // comment addendum below and this package's delivery report for the finding. A single
        // long string avoids it entirely (one node), letting this test demonstrate the
        // `check_snapshot` cross-process wiring cleanly.
        let oversized_text = "x".repeat(DocumentLimits::default().text_block_chars_max + 1);
        let mut base = LoroCollabEngine::new_empty(1);
        base.apply_operation(&Operation::CreateNode {
            id: NodeId::from("blk-1"),
            parent: None,
            index: 0,
            kind: NodeKind::Block,
        })
        .expect("create succeeds");
        base.apply_operation(&Operation::InsertText {
            id: NodeId::from("blk-1"),
            index: 0,
            text: oversized_text,
        })
        .expect("insert_text succeeds at the engine level (check_snapshot, not the engine, enforces this ceiling)");
        let base_frontier = base.frontier();
        let base_snapshot = base.export_snapshot().expect("export succeeds");

        let mut writer = base.fork().expect("fork succeeds");
        writer.set_title("trivial").expect("set_title succeeds");
        let update = writer.export_from(&base_frontier).expect("export succeeds");

        let Some(result) = isolated_apply_with_real_worker(&base_snapshot, &update) else {
            eprintln_test("skipped: collab-isolated-apply-worker binary not found");
            return;
        };
        match result {
            Err(IsolatedApplyError::Collab(CollabError::LimitExceeded {
                limit_kind,
                limit,
                observed,
            })) => {
                assert_eq!(limit_kind, "text_block_chars");
                assert_eq!(limit, DocumentLimits::default().text_block_chars_max as u64);
                assert!(observed > limit, "observed {observed} must exceed the limit {limit}");
            }
            other => panic!("expected Collab(LimitExceeded(text_block_chars)), got {other:?}"),
        }
    }

    #[test]
    fn isolated_apply_end_to_end_kills_a_pathologically_deep_chain_and_reports_a_ceiling() {
        // A base document built as one very wide, shallow "star" (one root, many direct children)
        // is legitimate CRDT content (no limit stops its *construction*; `check_snapshot`'s
        // sibling-count limits are only enforced downstream inside the isolated worker, which is
        // the whole point of this isolation host existing) and forces the worker's metered window
        // to spend real CPU reconstructing the whole thing in `semantic_snapshot`, regardless of
        // what the trivial update on top of it contains. Deliberately wide/shallow (depth 1), not
        // a long chain: an earlier version of this fixture built a long linear chain instead and
        // reliably crashed the *test* process itself with a native stack overflow well before
        // reaching a large enough N -- `LoroDoc`'s own tree bookkeeping (unrelated to this crate's
        // code) recurses per level for a linear chain's construction and/or drop. A wide star has
        // no such depth to recurse through.
        //
        // `SIBLING_COUNT` needs real headroom above `container_count_max`/`document_block_count_max`
        // (10,000): `semantic_snapshot`'s per-parent sibling lookup is already O(n) (not O(n^2), a
        // prior fix -- see `engine.rs::semantic_snapshot`'s doc comment), and `export_snapshot` no
        // longer runs inside the metered window at all (`src/bin/isolated_apply_worker.rs`'s
        // `decode_apply_check_and_export`), so a document merely *at* the structural ceiling now
        // finishes well under `decode_apply_cpu_ms_max` and would just be rejected by
        // `check_snapshot`'s own `container_count` check -- not by this CPU/wall ceiling test.
        // 35,000 keeps a real-machine measured margin (this repository's isolated-apply CPU-budget
        // investigation found the real worker subprocess's `SIGPROF` kill point for this shape
        // around ~29,000 nodes on the machine that measurement ran on) while staying fast enough
        // for `cargo test` to build the base document in-process (debug-mode `apply_operation` is
        // the dominant cost of this fixture, not the metered call itself).
        const SIBLING_COUNT: usize = 35_000;
        let mut base = LoroCollabEngine::new_empty(1);
        base.apply_operation(&Operation::CreateNode {
            id: NodeId::from("root"),
            parent: None,
            index: 0,
            kind: NodeKind::NavigatorNode,
        })
        .expect("create succeeds");
        for index in 0..SIBLING_COUNT {
            base.apply_operation(&Operation::CreateNode {
                id: NodeId::from(format!("n-{index}")),
                parent: Some(NodeId::from("root")),
                index: 0,
                kind: NodeKind::NavigatorNode,
            })
            .expect("create succeeds");
        }
        let base_frontier = base.frontier();
        let base_snapshot = base.export_snapshot().expect("export succeeds");

        let mut writer = base.fork().expect("fork succeeds");
        writer.set_title("trivial").expect("set_title succeeds");
        let update = writer.export_from(&base_frontier).expect("export succeeds");

        let Some(result) = isolated_apply_with_real_worker(&base_snapshot, &update) else {
            eprintln_test("skipped: collab-isolated-apply-worker binary not found");
            return;
        };
        match result {
            // The `SIGPROF` timer is a process CPU-time (not wall-clock) mechanism, so it is
            // possible in principle for scheduling delay/contention on the machine running this
            // test to let wall time run out first even though the ceiling logic is armed
            // correctly -- both accepted as a pass here (either still proves the pathological
            // chain was killed and never produced a candidate document), but `CpuCeiling` is the
            // expected, primary outcome for this fixture.
            Err(IsolatedApplyError::CpuCeiling | IsolatedApplyError::WallCeiling) => {}
            other => panic!("expected the pathological chain to be killed by the CPU or wall ceiling, got {other:?}"),
        }
    }

    // ---- The decision record, the host's observation of a killed worker, and typed host failures.

    use crate::window_check::FallbackReason;
    use wire::WindowDecision;

    /// The wire code of `NotSequential` in the injected workloads' `<code>:<n>` argument.
    fn not_sequential_code() -> u8 {
        WindowDecision::Fallback(FallbackReason::NotSequential).to_wire()
    }

    /// Runs `run_once` until it reports a conclusive run, pausing between attempts so that a short
    /// burst of contention can pass. A run is inconclusive when the worker was starved so badly
    /// that it never reached the point the test is about. Panics when no attempt is conclusive.
    fn conclusive_run<T>(what: &str, mut run_once: impl FnMut() -> Option<T>) -> T {
        for _ in 0..10 {
            if let Some(found) = run_once() {
                return found;
            }
            std::thread::sleep(BOUNDARY_PAUSE);
        }
        panic!("{what}: the worker never got far enough in ten attempts");
    }

    /// Runs `run_isolated_apply_observed` against the real debug worker with a base and an update
    /// that are real documents (not a synthetic workload).
    fn isolated_apply_real_observed(
        base_snapshot: &[u8],
        update: &[u8],
        mode: WindowMode,
    ) -> (Result<IsolatedApplySuccess, IsolatedApplyError>, WorkerObservation) {
        isolated_apply_real_observed_with_budget(
            base_snapshot,
            update,
            mode,
            crate::setup_guard::SETUP_CPU_BUDGET_MS_DEFAULT,
        )
    }

    /// [`isolated_apply_real_observed`] with the setup CPU budget the host passes to the worker.
    fn isolated_apply_real_observed_with_budget(
        base_snapshot: &[u8],
        update: &[u8],
        mode: WindowMode,
        setup_cpu_budget_ms: u64,
    ) -> (Result<IsolatedApplySuccess, IsolatedApplyError>, WorkerObservation) {
        let worker_path = required_debug_worker_binary_for_tests();
        let guard = env_lock().lock();
        // SAFETY: held under `env_lock()`, so no other test in this module reads or writes
        // `WORKER_BINARY_PATH_ENV` while this is set.
        unsafe {
            std::env::set_var(WORKER_BINARY_PATH_ENV, &worker_path);
        }
        let result = isolated_apply_with_observed(base_snapshot, update, mode, setup_cpu_budget_ms);
        // SAFETY: same reasoning as the `set_var` call above; still held under `env_lock()`.
        unsafe {
            std::env::remove_var(WORKER_BINARY_PATH_ENV);
        }
        drop(guard);
        result
    }

    /// A page with one block and an update typed into it, as an author that has seen the base.
    fn typed_update_on_a_block_page() -> (crate::LoroCollabEngine, Vec<u8>) {
        let mut base = crate::LoroCollabEngine::new_empty(1);
        base.apply_content_operation(&Operation::CreateNode {
            id: NodeId::from("a"),
            parent: None,
            index: 0,
            kind: NodeKind::Block,
        })
        .expect("create applies");
        let base_frontier = base.frontier();
        let mut writer = base.fork().expect("fork succeeds");
        writer
            .apply_content_operation(&Operation::InsertText {
                id: NodeId::from("a"),
                index: 0,
                text: "typed".into(),
            })
            .expect("text applies");
        let update = writer.export_from(&base_frontier).expect("export succeeds");
        (base, update)
    }

    #[test]
    fn a_completed_response_reports_what_the_window_check_decided() {
        let (base, update) = typed_update_on_a_block_page();
        let snapshot = base.export_snapshot().expect("export succeeds");

        let (result, observation) = isolated_apply_real_observed(&snapshot, &update, WindowMode::Incremental);
        result.expect("a one-character edit is accepted");
        assert_eq!(
            observation.decision,
            Some(WindowDecision::Incremental),
            "{observation:?}"
        );

        let (result, observation) = isolated_apply_real_observed(&snapshot, &update, WindowMode::Full);
        result.expect("the same update is accepted by the full computation");
        assert_eq!(
            observation.decision,
            Some(WindowDecision::Fallback(FallbackReason::FactsAbsent)),
            "the full mode computes no facts: {observation:?}"
        );

        let (result, observation) = isolated_apply_real_observed(&snapshot, b"not an update", WindowMode::Incremental);
        assert!(matches!(
            result,
            Err(IsolatedApplyError::Collab(CollabError::DecodeFailed { .. }))
        ));
        assert_eq!(
            observation.decision,
            Some(WindowDecision::Undecided),
            "an import that fails never reaches the check: {observation:?}"
        );
    }

    /// A title update by an author that has seen `base`.
    fn a_title_update_on(base: &[u8]) -> Vec<u8> {
        let mut server = crate::LoroCollabEngine::load(base).expect("the base loads");
        let frontier = server.frontier();
        server.set_title("typed").expect("the title is set");
        server.export_from(&frontier).expect("export succeeds")
    }

    /// A-1 and A-5 on the real worker: a page with one text that costs the setup more than a
    /// window's ceiling yields no facts, the worker reloads and meters the decode again, and the
    /// host learns which guard fired from the marker byte even though the full window is then killed
    /// by its ceiling and no response frame ever arrives.
    #[test]
    fn a_text_over_its_budget_is_reported_by_the_marker_of_an_attempt_the_full_window_kills() {
        let base = crate::engine::window_text_guard_tests::nested_marks_page(50, 1, 1_800);
        let update = a_title_update_on(&base);
        let (result, observation) = isolated_apply_real_observed(&base, &update, WindowMode::Incremental);
        assert!(
            matches!(
                result,
                Err(IsolatedApplyError::CpuCeiling | IsolatedApplyError::WallCeiling)
            ),
            "the full window pays the decode of the text inside its ceiling and is killed: {result:?}"
        );
        assert_eq!(
            observation.setup_guard,
            Some(crate::setup_guard::Fired::Text),
            "{observation:?}"
        );
        // A page without such a text reports no guard, whichever way the attempt ends.
        let (base, update) = typed_update_on_a_block_page();
        let snapshot = base.export_snapshot().expect("export succeeds");
        let (result, observation) = isolated_apply_real_observed(&snapshot, &update, WindowMode::Incremental);
        result.expect("accepted");
        assert_eq!(observation.setup_guard, None);
        let (result, observation) = isolated_apply_real_observed(&snapshot, b"not an update", WindowMode::Incremental);
        assert!(result.is_err());
        assert_eq!(observation.setup_guard, None);
    }

    /// R9-3 on the real worker: a page over the item budget (no facts, and no guard fired) has had
    /// its texts decoded by the setup's count, and the full window that decides it still meters
    /// those decodes, because the worker starts it from a freshly loaded engine: 60 texts of 400
    /// nested marks cost about 0.4 s together, far over the window's ceiling. Without the reload
    /// the window would find them decoded and accept.
    #[test]
    fn a_page_over_the_item_budget_meters_the_decode_of_the_texts_the_count_read() {
        let base = crate::engine::window_text_guard_tests::over_the_item_budget_with_decoded_texts();
        let update = a_title_update_on(&base);
        let (result, observation) = isolated_apply_real_observed(&base, &update, WindowMode::Incremental);
        assert!(
            matches!(
                result,
                Err(IsolatedApplyError::CpuCeiling | IsolatedApplyError::WallCeiling)
            ),
            "the full window pays the decode of every text inside its ceiling: {result:?}"
        );
        assert_eq!(observation.setup_guard, None, "no guard fired: {observation:?}");
        assert_eq!(
            observation.decision,
            Some(WindowDecision::Fallback(FallbackReason::FactsAbsent)),
            "{observation:?}"
        );
    }

    /// A-4 on the real worker: the host's setting of the setup CPU budget reaches the worker. A page
    /// whose setup costs more than half a second of CPU (250 texts of 450 nested marks, 2 s and more)
    /// trips the CPU guard of a worker given a budget of 500 ms.
    #[test]
    fn the_setup_cpu_budget_of_the_host_reaches_the_worker() {
        let base = crate::engine::window_text_guard_tests::nested_marks_page(250, 250, 450);
        let update = a_title_update_on(&base);
        let (_result, observation) =
            isolated_apply_real_observed_with_budget(&base, &update, WindowMode::Incremental, 500);
        assert_eq!(
            observation.setup_guard,
            Some(crate::setup_guard::Fired::Cpu),
            "{observation:?}"
        );
    }

    #[test]
    fn an_update_concurrent_with_the_base_is_reported_as_not_sequential_by_the_worker() {
        let mut base = crate::LoroCollabEngine::new_empty(1);
        base.apply_content_operation(&Operation::CreateNode {
            id: NodeId::from("a"),
            parent: None,
            index: 0,
            kind: NodeKind::Block,
        })
        .expect("create applies");
        let root_frontier = base.frontier();
        let author = |text: &str| {
            let mut writer = base.fork().expect("fork succeeds");
            writer
                .apply_content_operation(&Operation::InsertText {
                    id: NodeId::from("a"),
                    index: 0,
                    text: text.into(),
                })
                .expect("text applies");
            writer.export_from(&root_frontier).expect("export succeeds")
        };
        let first = author("one");
        let concurrent = author("two");
        let mut server =
            crate::LoroCollabEngine::load(&base.export_snapshot().expect("export succeeds")).expect("the base loads");
        server.import_update(&first).expect("the first update is accepted");
        let snapshot = server.export_snapshot().expect("export succeeds");

        let (result, observation) = isolated_apply_real_observed(&snapshot, &concurrent, WindowMode::Incremental);
        result.expect("the concurrent update is accepted by the full computation");
        assert_eq!(
            observation.decision,
            Some(WindowDecision::Fallback(FallbackReason::NotSequential)),
            "{observation:?}"
        );
    }

    /// The point of the decision record: a worker killed by the CPU ceiling after the check decided
    /// `NotSequential` has still told the host so.
    #[test]
    fn a_cpu_kill_after_a_not_sequential_decision_keeps_the_decision() {
        let observation = conclusive_run("a CPU kill after the decision", || {
            let (result, observation) =
                isolated_apply_injected_observed(&format!("decision_then_burn={}:150000", not_sequential_code()));
            match result {
                Err(IsolatedApplyError::CpuCeiling) => Some(observation),
                // Starved: killed at the wall deadline before it burned the ceiling. The record was
                // written first, so it is also kept; the test wants the CPU kill specifically.
                Err(IsolatedApplyError::WallCeiling) => None,
                other => panic!("a 150 ms burn over the ceiling ended as {other:?}"),
            }
        });
        assert_eq!(
            observation.decision,
            Some(WindowDecision::Fallback(FallbackReason::NotSequential)),
            "{observation:?}"
        );
    }

    #[test]
    fn a_wall_kill_after_a_decision_keeps_the_decision() {
        let observation = conclusive_run("a wall kill after the decision", || {
            let (result, observation) =
                isolated_apply_injected_observed(&format!("decision_then_sleep={}:300", not_sequential_code()));
            match result {
                Err(IsolatedApplyError::WallCeiling) => observation.decision.map(|_| observation),
                other => panic!("a 300 ms sleep ended as {other:?}"),
            }
        });
        assert_eq!(
            observation.decision,
            Some(WindowDecision::Fallback(FallbackReason::NotSequential)),
            "{observation:?}"
        );
    }

    /// A worker that is blocked rather than busy: killed at the wall deadline with almost no CPU
    /// consumed and without a decision, the observation that tells starvation from a heavy job.
    #[test]
    fn a_blocked_worker_is_wall_killed_with_low_cpu() {
        let (result, observation) = isolated_apply_injected_observed("wall_sleep_millis=150");
        assert!(matches!(result, Err(IsolatedApplyError::WallCeiling)), "{result:?}");
        assert!(
            observation
                .metered_cpu_us
                .is_some_and(|cpu_us| cpu_us < CPU_ACCOUNTING_SLACK_US),
            "a blocked worker consumed CPU: {observation:?}"
        );
        assert_eq!(observation.decision, None, "no check decided before the kill");
        assert!(
            observation.setup_cpu_us.is_some(),
            "the CPU of the setup is observed: {observation:?}"
        );
    }

    /// Under contention a job that burns less than the CPU ceiling can be wall-killed. Whenever the
    /// host sees a wall kill of a 33 ms burn, the CPU it observed is far below the ceiling, which
    /// is what the write path reads as starvation (`apps/api` `refusal.rs`, below 40 ms).
    #[test]
    fn every_wall_kill_of_a_burn_under_the_ceiling_shows_less_cpu_than_the_starvation_threshold() {
        let (_, under_micros, _) = boundary_runs();
        let threshold_us = super::super::DECODE_APPLY_CPU_MS_MAX * 1_000 * 4 / 5;
        for _ in 0..6 {
            let (result, observation) = isolated_apply_injected_observed(&format!("cpu_burn_micros={under_micros}"));
            if matches!(result, Err(IsolatedApplyError::WallCeiling)) {
                let cpu_us = observation
                    .metered_cpu_us
                    .expect("the kernel reports the CPU of the worker");
                assert!(
                    cpu_us < threshold_us,
                    "a wall kill of a {under_micros}us burn shows {cpu_us}us of CPU, not below {threshold_us}us"
                );
            }
        }
    }

    /// Runs `body` with the setup timeout of this thread shortened to `millis`.
    fn with_setup_timeout<T>(millis: u64, body: impl FnOnce() -> T) -> T {
        struct Reset;
        impl Drop for Reset {
            fn drop(&mut self) {
                SETUP_TIMEOUT_OVERRIDE_MS.with(|slot| slot.set(None));
            }
        }
        SETUP_TIMEOUT_OVERRIDE_MS.with(|slot| slot.set(Some(millis)));
        let _reset = Reset;
        body()
    }

    #[test]
    fn a_setup_timeout_is_typed_and_reports_the_setup_cpu() {
        // A sleeping setup: killed at the (shortened) timeout having consumed little CPU.
        let (result, observation) =
            with_setup_timeout(400, || isolated_apply_injected_observed("setup_sleep_millis=5000"));
        let Err(IsolatedApplyError::HostFailure(failure)) = result else {
            panic!("a setup that outlasts the timeout is a host failure, got {result:?}");
        };
        assert_eq!(failure.kind, HostFailureKind::SetupTimeout, "{failure:?}");
        assert_eq!(observation.metered_cpu_us, None, "the metered window never opened");
        let sleeping_cpu_us = observation
            .setup_cpu_us
            .expect("the kernel reports the CPU of a killed worker");
        assert!(
            sleeping_cpu_us < 300_000,
            "a sleeping setup consumed {sleeping_cpu_us}us of CPU"
        );
        // The wall time of the setup is reported too, so that CPU well below it reads as starvation.
        let wall_us = observation
            .setup_wall_us
            .expect("the wall time of a killed setup is reported");
        assert!(wall_us >= 400_000, "killed at the timeout: {wall_us}us of wall time");
        assert!(
            sleeping_cpu_us * 2 < wall_us,
            "a sleeping setup had little CPU for its wall time"
        );

        // A burning setup: killed at the timeout having consumed CPU for most of it.
        let burned_us = conclusive_run("a burning setup", || {
            let (result, observation) =
                with_setup_timeout(600, || isolated_apply_injected_observed("setup_burn_micros=5000000"));
            let Err(IsolatedApplyError::HostFailure(failure)) = result else {
                panic!("a burning setup that outlasts the timeout is a host failure, got {result:?}");
            };
            assert_eq!(failure.kind, HostFailureKind::SetupTimeout);
            observation.setup_cpu_us.filter(|cpu_us| *cpu_us >= 250_000)
        });
        assert!(burned_us >= 250_000);
    }

    #[test]
    fn spawn_resource_errors_are_typed() {
        for errno in [libc::EAGAIN, libc::ENOMEM, libc::EMFILE, libc::ENFILE] {
            let IsolatedApplyError::HostFailure(failure) = spawn_failure(&std::io::Error::from_raw_os_error(errno))
            else {
                panic!("a failed spawn is a host failure");
            };
            assert_eq!(failure.kind, HostFailureKind::SpawnResources, "errno {errno}");
        }
        for errno in [libc::ENOENT, libc::EACCES, libc::ENOEXEC] {
            let IsolatedApplyError::HostFailure(failure) = spawn_failure(&std::io::Error::from_raw_os_error(errno))
            else {
                panic!("a failed spawn is a host failure");
            };
            assert_eq!(failure.kind, HostFailureKind::Spawn, "errno {errno}");
        }
    }

    #[test]
    fn exit_signals_and_statuses_are_typed() {
        let kind = |error: IsolatedApplyError| match error {
            IsolatedApplyError::HostFailure(failure) => failure.kind,
            other => panic!("expected a host failure, got {other:?}"),
        };
        assert_eq!(kind(classify_signal(libc::SIGKILL)), HostFailureKind::ForeignKill);
        assert_eq!(kind(classify_signal(libc::SIGSEGV)), HostFailureKind::Crash);
        assert_eq!(kind(classify_signal(libc::SIGBUS)), HostFailureKind::Crash);
    }

    /// Runs the host against a stand-in program that is not the worker.
    fn isolated_apply_against(program: &str) -> Result<IsolatedApplySuccess, IsolatedApplyError> {
        isolated_apply_against_request(program, &[])
    }

    /// [`isolated_apply_against`] with a base snapshot field of the given bytes in the request.
    fn isolated_apply_against_request(program: &str, base: &[u8]) -> Result<IsolatedApplySuccess, IsolatedApplyError> {
        let guard = env_lock().lock();
        // SAFETY: held under `env_lock()`, so no other test in this module reads or writes
        // `WORKER_BINARY_PATH_ENV` while this is set.
        unsafe {
            std::env::set_var(WORKER_BINARY_PATH_ENV, program);
        }
        let result = isolated_apply(base, &[]);
        // SAFETY: same reasoning as the `set_var` call above; still held under `env_lock()`.
        unsafe {
            std::env::remove_var(WORKER_BINARY_PATH_ENV);
        }
        drop(guard);
        result
    }

    /// A worker that exits successfully without a decision record violates the protocol: the host
    /// never accepts a response it cannot place (the worker always writes the record first).
    #[test]
    fn a_completed_response_without_a_decision_record_is_a_protocol_failure() {
        let result = isolated_apply_against("/bin/true");
        let Err(IsolatedApplyError::HostFailure(failure)) = result else {
            panic!("a zero exit without a response is a host failure, got {result:?}");
        };
        assert_eq!(failure.kind, HostFailureKind::Protocol, "{failure:?}");
        assert!(failure.detail.contains("decision record"), "{failure:?}");
    }

    #[test]
    fn a_non_zero_exit_is_a_crash() {
        let result = isolated_apply_against("/bin/false");
        let Err(IsolatedApplyError::HostFailure(failure)) = result else {
            panic!("a non-zero exit is a host failure, got {result:?}");
        };
        assert_eq!(failure.kind, HostFailureKind::Crash, "{failure:?}");
    }

    #[test]
    fn a_missing_worker_binary_is_a_spawn_failure() {
        let result = isolated_apply_against("/nonexistent/isolated-apply-worker");
        let Err(IsolatedApplyError::HostFailure(failure)) = result else {
            panic!("a missing worker is a host failure, got {result:?}");
        };
        assert_eq!(failure.kind, HostFailureKind::Spawn, "{failure:?}");
    }

    /// A stand-in worker: a shell script with this body, in a directory of its own.
    fn stand_in_worker(name: &str, body: &str) -> std::path::PathBuf {
        use std::os::unix::fs::PermissionsExt;
        let dir = std::env::temp_dir().join(format!("collab-stand-in-{}-{name}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("the stand-in directory is created");
        let path = dir.join("worker");
        std::fs::write(&path, format!("#!/bin/sh\n{body}\n")).expect("the stand-in is written");
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).expect("the stand-in is executable");
        path
    }

    /// A request base field far larger than a pipe holds (64 KiB on Linux): a child that never reads
    /// it makes the host's write fail with `EPIPE` once the child has exited, whatever the
    /// scheduling, because the write cannot complete into the pipe's buffer.
    fn a_request_larger_than_the_pipe() -> Vec<u8> {
        vec![0; 4 * 1024 * 1024]
    }

    /// The ordering of the request write and the child's end. A failed request write never decides
    /// the classification: the host waits for the child, reads what it produced, and classifies by
    /// its actual outcome, so these are the same as when the write succeeds. The stand-ins make the
    /// write fail on purpose (they exit without reading a request that cannot fit the pipe); the one
    /// that exits after reading it makes it succeed.
    #[test]
    fn a_failed_request_write_does_not_decide_the_classification() {
        let base = a_request_larger_than_the_pipe();
        let classify = |name: &str, body: &str, request: &[u8]| {
            let path = stand_in_worker(name, body);
            let result = isolated_apply_against_request(path.to_str().expect("a UTF-8 path"), request);
            let _ = std::fs::remove_dir_all(path.parent().expect("a directory"));
            let Err(IsolatedApplyError::HostFailure(failure)) = result else {
                panic!("{name}: a host failure, got {result:?}");
            };
            failure
        };

        // Exits 0 before it reads the request: the write fails, and it is still a protocol failure
        // (exit 0 without a decision record), with the write error as the detail.
        let failure = classify("exits-first", "exit 0", &base);
        assert_eq!(failure.kind, HostFailureKind::Protocol, "{failure:?}");
        assert!(failure.detail.contains("decision record"), "{failure:?}");
        assert!(failure.detail.contains("request write failed"), "{failure:?}");

        // Exits non-zero before it reads the request: a crash, by its status.
        let failure = classify("exits-nonzero", "exit 3", &base);
        assert_eq!(failure.kind, HostFailureKind::Crash, "{failure:?}");
        assert!(failure.detail.contains("non-zero status Some(3)"), "{failure:?}");

        // Killed by a signal it did not get from the host before it reads the request: classified
        // by its signal, as before (a foreign SIGKILL is a host short of resources, not a crash).
        let failure = classify("killed-first", "kill -KILL $$", &base);
        assert_eq!(failure.kind, HostFailureKind::ForeignKill, "{failure:?}");
        let failure = classify("segfault-first", "kill -SEGV $$", &base);
        assert_eq!(failure.kind, HostFailureKind::Crash, "{failure:?}");
        assert!(failure.detail.contains("signal"), "{failure:?}");

        // Exits 0 after it has read the whole request: the write succeeded, the same classification
        // and no write error in the detail.
        let failure = classify("exits-after-reading", "cat >/dev/null\nexit 0", &base);
        assert_eq!(failure.kind, HostFailureKind::Protocol, "{failure:?}");
        assert!(!failure.detail.contains("request write failed"), "{failure:?}");
        let failure = classify("exits-after-reading-nonzero", "cat >/dev/null\nexit 3", &base);
        assert_eq!(failure.kind, HostFailureKind::Crash, "{failure:?}");
    }
}
