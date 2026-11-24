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
mod doctor;
#[cfg(test)]
mod doctor_tests;
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

/// The tmux driver, for the integration test that runs it against a real tmux (feature
/// `test-support`); it is not part of any interface.
#[cfg(feature = "test-support")]
#[doc(hidden)]
pub mod tmux_for_tests {
    pub use crate::runner::tmux::*;
}

pub(crate) struct AppState {
    pub config: config::Config,
    pub background: background::Background,
    pub replay: tokio::sync::Mutex<replay::ReplayGuard>,
    pub runner: runner::RunnerRuntime,
    pub handshake: handshake::Handshake,
    pub sessions: Option<std::sync::Arc<runner::session::SessionManager>>,
}

/// The pieces that decide what a launched CLI gets for MCP, for the end-to-end test in the MCP
/// server's crate that starts the real server from them (feature `test-support`); it is not part
/// of any interface.
#[cfg(feature = "test-support")]
#[doc(hidden)]
pub mod mcp_launch_for_tests {
    pub use crate::config::{AiTaskCallback, Config};
    pub use crate::runner::checks::BotCreds;
    pub use crate::runner::files::StateDir;
    pub use crate::runner::launch::{
        LaunchFiles, SEARCH_MODE_TOOLS, SERVER_NAME, WRITE_TOOL, claude_headless_argv, codex_headless_argv,
        session_argv,
    };
    pub use crate::runner::tmux::Slot;
}
