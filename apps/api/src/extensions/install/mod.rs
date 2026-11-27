//! Installations of packages (contract C1 §6, C3 §5): request, review, approve, disable, revoke,
//! the invoke route, and the live view the host loads before every host call.
//!
//! * [`live`]: the installation as it is now (status, active generation, grants, approver).
//! * [`store`]: SQL. [`access`]: who may call. [`rate`]: the per-workspace limits.
//! * [`validate`], [`review`]: what a package needs and the `InstallPreview`.
//! * [`approve`], [`artefacts`]: the approval transaction and the layouts it creates.
//! * [`handlers`], [`invoke`]: the routes.

pub mod access;
pub mod approve;
pub mod artefacts;
pub mod handlers;
pub mod invoke;
pub mod live;
pub mod rate;
pub mod review;
pub mod store;
pub mod validate;
