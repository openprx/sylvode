//! Layouts: stored `sv.layout.v1` specs, their placements and per-user preferences.
//!
//! Validation, capability analysis and the slot table live in the `sv-layout` crate;
//! this module owns storage, access rules and the HTTP handlers.

pub(crate) mod access;
pub(crate) mod apply;
pub mod handlers;
pub mod placements;
mod store;
