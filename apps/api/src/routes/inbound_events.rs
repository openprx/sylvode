//! The handlers of the alert chain's inbound endpoint and of the alert facts, as `main.rs`
//! registers them.
//!
//! The implementation lives in `inbound::ingest` and `inbound::alert_view`; the inbound route's own
//! source-credential layer is `inbound::auth::inbound_source_auth`.

pub use crate::inbound::alert_view::get_issue_alert;
pub use crate::inbound::ingest::post_event;
