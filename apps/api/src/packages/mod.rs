//! Packages: the package store (publisher keys, signed uploads, shared blobs) and its routes.
//! Contract C3 section 4. Verification always runs on the stored or uploaded bytes through
//! `sv-package`; nothing a package contains is trusted before its signature has verified against
//! a registered, active key of the uploading workspace (or the built-in `sv` key).

mod access;
pub mod boot;
pub mod handlers;
mod keys;
mod store;
mod verify;
