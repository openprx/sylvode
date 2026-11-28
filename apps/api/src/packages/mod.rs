//! Packages: the package store (publisher keys, signed uploads, shared blobs) and its routes.
//!
//! Contract C3 section 4. Verification always runs on the stored or uploaded bytes through
//! `sv-package`; nothing a package contains is trusted before its signature has verified against
//! a registered, active key of the uploading workspace (or the built-in `sv` key).

mod access;
pub mod boot;
pub mod handlers;
pub mod index;
mod keys;
pub mod open;
pub mod search;
pub mod store;
mod submit_gate;
pub(crate) mod verify;

/// Takes a share lock on an active publisher key; see `keys::lock_active`.
///
/// # Errors
/// A database failure.
pub async fn keys_lock_active(
    db: &impl sea_orm::ConnectionTrait,
    id: uuid::Uuid,
) -> Result<bool, crate::error::ApiError> {
    keys::lock_active(db, id).await
}
