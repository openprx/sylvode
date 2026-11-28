//! The public plugin index (contract M15, `sv.plugin-index.v1`).
//!
//! The refresher fetches the operator's configured sources; the cache holds validated documents;
//! the revocation sync applies what an index revokes; the read routes and the image route serve the
//! Market's Public tab.
//!
//! Trust model (contract section 2): the index is read over HTTPS from the operator's configured
//! URL and is not signed. Nothing in it installs anything. It only lists, and it can revoke: a key
//! or version it marks `revoked` stops the matching workspace keys, packages and installations
//! (`sync`). Every request goes through the outbound address policy (`fetch`).

pub mod admin;
mod cache;
mod download;
mod fetch;
pub mod handlers;
pub mod import;
mod refresh;
mod store;
mod sync;

pub use refresh::{RefreshOutcome, refresh_source, spawn_refresher};

/// The default page size of the plugin list and its maximum.
const DEFAULT_PER_PAGE: i64 = 20;
const MAX_PER_PAGE: i64 = 100;
