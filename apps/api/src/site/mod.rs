//! The public site (contract C3 section 6): the site model and its console administration, binding
//! projections, visitor and operator credentials, and the site API `/api/site/v1/{slug}` with its
//! own layer stack.

mod access;
pub mod admin;
pub(crate) mod drafts;
pub mod http;
pub mod layouts;
mod projection;
pub mod public;
mod records;
mod session;
mod store;

pub use store::SiteRow;

use sea_orm::ConnectionTrait;
use uuid::Uuid;

use crate::error::ApiError;

/// Called by the workspace member removal inside its transaction: the user's site roles and
/// operator sessions at the workspace's sites go with the membership.
///
/// # Errors
/// The database error of the deletion.
pub async fn remove_member_site_access(
    db: &impl ConnectionTrait,
    workspace_id: Uuid,
    user_id: Uuid,
) -> Result<(), ApiError> {
    store::remove_member(db, workspace_id, user_id).await
}
