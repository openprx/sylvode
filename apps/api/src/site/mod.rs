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
use serde_json::{Value, json};
use uuid::Uuid;

use crate::error::ApiError;

/// The `projection_affected` warnings of a form schema save (contract C3 section 6.2): one per
/// site binding of the form whose saved projection loses fields under the new schema, or still
/// shows fields whose label, type, options, requiredness or masking change. The save itself is not
/// refused; the site drops fields that no longer qualify at read.
///
/// # Errors
/// The database error of reading the bindings.
pub async fn projection_affected(
    db: &impl ConnectionTrait,
    form_id: Uuid,
    title_template: &str,
    old_schema: &Value,
    new_schema: &Value,
) -> Result<Vec<Value>, ApiError> {
    let bindings = store::bindings_of_form(db, form_id, false).await?;
    if bindings.is_empty() {
        return Ok(Vec::new());
    }
    let (Ok(old), Ok(new)) = (projection::site_fields(old_schema), projection::site_fields(new_schema)) else {
        return Ok(Vec::new());
    };
    Ok(bindings
        .iter()
        .filter_map(|binding| {
            let (lost, changed) = projection::affected(&binding.projection, title_template, &old, &new);
            (!lost.is_empty() || !changed.is_empty()).then(|| {
                json!({
                    "path": "",
                    "code": "projection_affected",
                    "site_id": binding.site_id,
                    "binding": binding.binding_key,
                    "lost": lost,
                    "changed": changed,
                })
            })
        })
        .collect())
}

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
