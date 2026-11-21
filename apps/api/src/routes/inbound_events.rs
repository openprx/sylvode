//! The handler of the alert chain's inbound endpoint, as `main.rs` registers it.
//!
//! The implementation lives in `inbound::ingest`; the route's own source-credential layer is
//! `inbound::auth::inbound_source_auth`.

pub use crate::inbound::ingest::post_event;
