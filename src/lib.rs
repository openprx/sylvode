//! Sylvode Webhook (formerly `OpenPR` Webhook).
//!
//! The service is built as a library with two thin executables: `sylvode-webhook`, the canonical
//! name, and `openpr-webhook`, the legacy name kept for the whole Sylvode 1.x line. Both call [`run`], so
//! there is exactly one code path; the only difference is the [`Invocation`] each passes, which
//! decides whether the one-line deprecation notice is written to stderr.

mod app;
mod background;
mod callback;
mod compat;
mod config;
mod dispatcher;
mod handler;
mod process;
mod replay;
mod signature;
mod tunnel;

pub use app::run;
pub use compat::Invocation;

pub(crate) struct AppState {
    pub config: config::Config,
    pub background: background::Background,
    pub replay: tokio::sync::Mutex<replay::ReplayGuard>,
}
