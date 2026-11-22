//! Sylvode Flow's server-side domain logic, layered per `versions/v0.4-flow-alpha.md` ("Rust 与
//! API": "`apps/api/src/flow/` 分为 model、command、query、repository、projection、policy").
//!
//! - [`model`]: wire-shape response types (`FlowObjectView`, `AcceptedChange`, list/history
//!   response envelopes) shared by every handler in `crate::routes::flow`.
//! - [`policy`]: the workspace-membership + `flow_enabled` gate every handler runs first.
//! - [`command`]: the shared object command dispatcher and object creation path.
//! - [`collections`]: v0.6 Collection schema/view/Record commands and synchronous typed
//!   projections; every Record remains an independent collaborative document.
//! - [`grants`]: `ADR-0012`'s v0.5 authorization surface — `flow_object_grants` and the
//!   `inherit_from_parent` boundary, with the §4.1 self-lockout guard and the §3.1 epoch lock.
//! - [`move_object`]: `ADR-0012` §4's cross-parent move, and the only command in the frozen v0.5
//!   set that walks `ADR-0013` §2's multi-document ordered-lock path.
//! - [`query`]: the three read paths — get, list, history.
//! - [`repository`]: hand-written parameterized SQL against `flow_objects` / `collab_documents` /
//!   `flow_object_projections` / `collab_updates` / `business_events` / `event_dispatch` (no
//!   SeaORM entities, matching every module past migration 0024).
//! - [`projection`]: turns a `collab_core::SemanticSnapshot` into the JSON/plain-text/markdown
//!   shapes the projection table and the REST responses use.
//!
//! `apps/api/src/routes/flow.rs` is the thin axum-handler layer on top of this module: request
//! parsing/validation of HTTP-specific shape (path/query extraction) lives there, while this
//! module owns the domain rules (idempotency, workspace/project/parent validation, the CRDT
//! document lifecycle, and the SQL).

pub mod appearance;
pub mod bridge;
pub mod collab;
pub mod collections;
pub mod command;
pub mod comments;
pub mod document;
pub mod event_origin;
pub mod event_policy;
pub mod export;
pub mod frontier;
pub mod grants;
pub mod import;
pub mod import_replay;
pub mod maintenance;
pub mod model;
pub mod move_object;
pub mod operations;
pub mod package;
pub mod package_import;
pub mod policy;
pub mod projection;
pub mod public_page;
pub mod query;
pub mod relations;
pub mod repository;
pub mod restore;
pub mod rollback;
pub mod scope;
pub mod scope_acl;
pub mod scope_assets;
pub mod scope_collab;
pub mod scope_collections;
pub mod scope_commands;
pub mod scope_editor;
pub mod scope_event;
pub mod scope_export;
pub mod scope_history;
pub mod scope_import;
pub mod scope_mounts;
pub mod scope_package;
pub mod scope_purge;
pub mod scope_record_context;
pub mod scope_relations;
pub mod scope_runtime;
pub mod scope_shares;
pub mod scope_social;
pub mod scope_write;
pub mod search;
pub mod shares;
pub mod trash;

/// `retry_after_ms` for a request whose identical twin (same idempotency key, same body) is still
/// being executed: retrying after this delay returns the first run's stored result. Reported as
/// `server_draining{reason:"contention"}` because the in-flight run holds the idempotency claim
/// the retry has to wait on.
pub(crate) const IN_FLIGHT_RETRY_AFTER_MS: u64 = 1_000;

pub mod scope_move;
pub mod scope_navigation_query;

pub mod scope_operations;

pub mod json_value;

pub mod scope_presence;

// Phase2 body asset service candidate; HTTP routes and model5 admission are not enabled yet.
pub mod block_anchors;
pub mod block_registry;
pub mod block_shares;
pub mod body_assets;

pub mod child_pages;

// Independent next sub-batch; no HTTP registration until the complete two-Page transaction gate.
pub mod page_transfer_contract;
pub mod page_transfer_ir;
pub mod page_transfer_prepare;
pub mod page_transfers;
pub mod portable_graph_package;
pub mod portable_page_package;
