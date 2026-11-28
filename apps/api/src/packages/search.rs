//! The market search of the workspace's packages (contract C3 section 4.4).
//!
//! `GET /api/v1/workspaces/{w}/market/search` lists the same packages as the package list (the
//! built-in ones and the workspace's own, nothing of another workspace), filtered by text, kind and
//! layout slot, with paging, and adds whether each package is installed.

use axum::{
    Extension,
    extract::{Path, Query, State},
    response::IntoResponse,
};
use platform::{app::AppState, auth::JwtClaims};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::{
    access::Caller,
    handlers::checked_filters,
    store::{self, PackageSummary},
};
use crate::{
    error::ApiError,
    middleware::bot_auth::BotAuthContext,
    response::{ApiResponse, PageQuery, PaginatedData},
};

#[derive(Debug, Deserialize)]
pub struct SearchQuery {
    q: Option<String>,
    kind: Option<String>,
    /// A layout slot id the package places a layout in.
    slot: Option<String>,
    /// The project whose installations `installed` refers to; the workspace scope when absent.
    project_id: Option<Uuid>,
    // Not `#[serde(flatten)]`, see `ListQuery`.
    page: Option<i64>,
    per_page: Option<i64>,
}

/// A package of the search: its summary plus whether a live installation exists at the scope.
#[derive(Debug, Serialize)]
pub struct SearchItem {
    #[serde(flatten)]
    summary: PackageSummary,
    installed: bool,
    /// Only for a workspace owner or admin, who may read installations.
    #[serde(skip_serializing_if = "Option::is_none")]
    installation_id: Option<Uuid>,
}

/// The slot grammar of a manifest (`sv-package` `dotted`, 64 characters).
fn slot_ok(slot: &str) -> bool {
    let b = slot.as_bytes();
    !b.is_empty()
        && b.len() <= 64
        && b.first().is_some_and(u8::is_ascii_lowercase)
        && b.iter()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(c, b'.' | b'_' | b'-'))
}

/// `GET /api/v1/workspaces/{w}/market/search?q=&kind=&slot=&project_id=&page=&per_page=`
pub async fn search_packages(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    bot: Option<Extension<BotAuthContext>>,
    Path(workspace_id): Path<Uuid>,
    Query(query): Query<SearchQuery>,
) -> Result<impl IntoResponse, ApiError> {
    let caller = Caller::resolve(&state, &claims, bot, workspace_id).await?;
    let q = checked_filters(query.kind.as_deref(), query.q.as_deref())?;
    if let Some(slot) = query.slot.as_deref()
        && !slot_ok(slot)
    {
        return Err(ApiError::bad_request_coded(
            "slot_invalid",
            "slot is not a valid slot id",
        ));
    }
    if let Some(project_id) = query.project_id {
        // Installations of a project are an administrator's view (the installation routes are
        // admin-only), and a bot of the whole workspace has the same reach.
        if !caller.is_admin() && !caller.is_bot() {
            return Err(ApiError::forbidden(
                "only workspace owners and admins can ask for the installations of a project",
            ));
        }
        if !store::project_in_workspace(&state.db, workspace_id, project_id).await? {
            return Err(ApiError::not_found("project not found"));
        }
    }
    let paging = PageQuery {
        page: query.page,
        per_page: query.per_page,
    }
    .resolve(50, 200);
    let (rows, total) = store::list_visible(
        &state.db,
        workspace_id,
        query.kind.as_deref(),
        q,
        query.slot.as_deref(),
        paging.per_page,
        paging.offset(),
    )
    .await?;
    let keys: Vec<String> = rows.iter().map(|row| row.package_key.clone()).collect();
    let live = store::live_installations(&state.db, workspace_id, query.project_id, &keys).await?;
    let show_ids = caller.is_admin();
    let items: Vec<SearchItem> = rows
        .iter()
        .map(|row| {
            let found = live.iter().find(|(key, _)| *key == row.package_key);
            SearchItem {
                summary: PackageSummary::from(row),
                installed: found.is_some(),
                installation_id: found.filter(|_| show_ids).map(|(_, id)| *id),
            }
        })
        .collect();
    Ok(ApiResponse::success(PaginatedData::page(items, total, paging)))
}
