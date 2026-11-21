//! The Sylvode runner.
//!
//! The service is built as a library with one thin executable, `sylvode-runner`, which calls [`run`].
//!
//! The service manages executors as process groups and handles `SIGTERM`, so it is Unix only. On
//! any other target this crate is empty (the cfg below) and the executables only print that the
//! platform is not supported, so that `cargo build --workspace` still succeeds there.

// Every module below needs Unix process groups and signals; see the crate documentation.
#![cfg(unix)]

mod ai_task;
mod app;
mod background;
mod config;
mod dispatcher;
mod executor_permissions;
mod handler;
mod handshake;
mod process;
mod replay;
mod runner;
mod signature;
#[cfg(test)]
mod test_support;
mod tunnel;

pub use app::run;

pub(crate) struct AppState {
    pub config: config::Config,
    pub background: background::Background,
    pub replay: tokio::sync::Mutex<replay::ReplayGuard>,
    pub runner: runner::RunnerRuntime,
    pub handshake: handshake::Handshake,
    pub sessions: Option<std::sync::Arc<runner::session::SessionManager>>,
}
