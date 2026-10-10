//! Server-side process isolation for `LoroCollabEngine::import_update`, per `ADR-0014` and
//! `contracts/limits-v1.md`'s "Isolated decode/apply" table (`decode_apply_cpu_ms_max` = 50ms,
//! `decode_apply_wall_ms_max` = 100ms, `isolated_apply_memory_bytes_max` = 128 MiB).
//!
//! [`isolated_apply`] is the entry point `apps/api`'s `flow::collab::write::hydrate_and_apply`
//! calls. See [`host`]'s module doc for the full design: a fresh worker process per call, why that
//! (not `ADR-0014`'s fork-no-exec zygote) is the right primitive inside a live multi-threaded
//! async server, and exactly what is/isn't carried over from the `spikes/collab-shared`
//! calibration host.
//!
//! - [`wire`] -- the request/response codec (pure, `unsafe`-free).
//! - [`alloc`] -- the counting `GlobalAlloc` that is the online memory-ceiling enforcer. Only
//!   installed as the process's actual global allocator by `src/bin/isolated_apply_worker.rs`.
//! - [`child_runtime`] -- worker-side primitives: single-threadedness verification, the
//!   `RLIMIT_AS` backstop, arming/disarming the `SIGPROF` CPU-ceiling timer.
//! - [`host`] -- parent-side orchestration: spawn, wall watchdog, response-frame decode,
//!   exit-signal classification.
//!
//! `alloc`, `child_runtime`, and `host` are compiled only for Linux, the platform matrix
//! `ADR-0014` declares. On every other target [`isolated_apply`] and [`isolated_diff`] keep their
//! signatures but always return [`IsolatedApplyError::UnsupportedPlatform`].
//!
//! # Why `unsafe_code` is allowed here
//!
//! The workspace root `Cargo.toml` sets `unsafe_code = "deny"` via `[workspace.lints.rust]`, cast
//! as a crate-level `-D unsafe_code` flag by Cargo -- not `forbid`, so it can be locally
//! overridden. This module tree is the only place in `collab-core` that needs raw
//! `setitimer`/`sigaction`/`setrlimit`/`kill`/`poll`-family syscalls; every individual `unsafe`
//! block still carries its own `// SAFETY:` comment (the workspace clippy lint
//! `undocumented_unsafe_blocks = "deny"` is *not* overridden and still applies here).
#![allow(unsafe_code, clippy::too_long_first_doc_paragraph)]

// `alloc`, `child_runtime`, and `host` are the enforcing boundary itself and exist only on Linux
// (`ADR-0014`: "P2 平台矩阵在 gate 里显式声明为 Linux"). Everything a caller names -- the entry
// points' signatures, their result types, the wire codec, and the frozen ceilings -- is
// platform-neutral, so `apps/api` compiles on every release target; off Linux the entry points
// come from `unsupported` and always refuse.
#[cfg(target_os = "linux")]
pub mod alloc;
#[cfg(target_os = "linux")]
pub mod child_runtime;
#[cfg(target_os = "linux")]
pub mod host;
mod limits;
mod outcome;
#[cfg(not(target_os = "linux"))]
mod unsupported;
pub mod wire;

#[cfg(target_os = "linux")]
pub use host::{WORKER_BINARY_PATH_ENV, isolated_apply, isolated_diff};
pub use outcome::{IsolatedApplyError, IsolatedApplySuccess, IsolatedDiffSuccess};
#[cfg(not(target_os = "linux"))]
pub use unsupported::{isolated_apply, isolated_diff};
// Single source of truth for these three (`isolation::limits`'s own module doc explains why):
// `host`, `alloc`, and `child_runtime` all `use` them from there rather than declaring their own
// copies, so this re-export and every enforcement site name the identical constant.
pub use limits::{DECODE_APPLY_CPU_MS_MAX, DECODE_APPLY_WALL_MS_MAX, ISOLATED_APPLY_MEMORY_BYTES_MAX};
