//! Owner routes of the public plugin index: refresh a source now, hide or show a source for the
//! workspace, and the audit log (contract M15, section 3).
//!
//! All of them sit behind the console session middleware, so no bot or delegated token reaches
//! them. Refresh and hide change what the whole workspace sees, so they are the owner's; the event
//! log is for owners and admins.

use std::collections::HashMap;
use std::sync::LazyLock;
use std::time::{Duration, Instant};

use axum::{
    Extension, Json,
    extract::{Path, Query, State},
    response::IntoResponse,
};
use parking_lot::Mutex;
use platform::{app::AppState, auth::JwtClaims, config::IndexSource};
use serde::Deserialize;
use serde_json::{Value, json};
use uuid::Uuid;

use super::{
    handlers::{source_not_found, source_status},
    refresh::{RefreshOutcome, refresh_source},
    store,
};
use crate::{
    error::ApiError,
    packages::access::Caller,
    response::{ApiResponse, PageQuery, PaginatedData},
};

/// A forced refresh of one source is allowed once per this long.
const REFRESH_COOLDOWN: Duration = Duration::from_mins(1);
const DEFAULT_EVENTS_PER_PAGE: i64 = 50;
const MAX_EVENTS_PER_PAGE: i64 = 200;

/// When each source was last refreshed on request, in this process.
static LAST_FORCED: LazyLock<Mutex<HashMap<String, Instant>>> = LazyLock::new(|| Mutex::new(HashMap::new()));

/// The configured source `id`, hidden or not.
fn configured_source(id: &str) -> Result<IndexSource, ApiError> {
    crate::config::runtime()
        .packages
        .index_sources
        .iter()
        .find(|source| source.id == id)
        .cloned()
        .ok_or_else(source_not_found)
}

/// Takes the source's slot for a forced refresh, or says how long to wait.
fn claim_refresh(source_id: &str, now: Instant) -> Result<(), Duration> {
    let mut last = LAST_FORCED.lock();
    last.retain(|_, at| now.duration_since(*at) < REFRESH_COOLDOWN);
    let waited = last
        .get(source_id)
        .map(|at| REFRESH_COOLDOWN.saturating_sub(now.duration_since(*at)));
    if waited.is_none() {
        last.insert(source_id.to_owned(), now);
    }
    drop(last);
    waited.map_or(Ok(()), Err)
}

const fn outcome_name(outcome: RefreshOutcome) -> &'static str {
    match outcome {
        RefreshOutcome::Skipped => "skipped",
        RefreshOutcome::Unchanged => "unchanged",
        RefreshOutcome::Updated => "updated",
        RefreshOutcome::Failed => "failed",
    }
}

/// `POST /sources/{source}/refresh`: fetches the source now, at most once a minute per source.
///
/// Workspace owner only. The answer is the source's status after the refresh, with `outcome`
/// (`skipped` when another process held the source).
pub async fn refresh_now(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    Path((workspace_id, source_id)): Path<(Uuid, String)>,
) -> Result<impl IntoResponse, ApiError> {
    let caller = Caller::resolve(&state, &claims, None, workspace_id).await?;
    if !caller.is_owner() {
        return Err(ApiError::forbidden(
            "only the workspace owner can refresh a plugin index source",
        ));
    }
    let source = configured_source(&source_id)?;
    if let Err(wait) = claim_refresh(&source.id, Instant::now()) {
        return Err(ApiError::Coded {
            status: 429,
            code: "index_refresh_rate_limited",
            message: "a source can be refreshed on request once a minute".to_string(),
            details: Some(json!({ "retry_after_secs": wait.as_secs().saturating_add(1) })),
        });
    }
    let outcome = refresh_source(&state, &source, true).await?;
    let mut status = source_status(&state, workspace_id, &source).await?;
    if let Some(object) = status.as_object_mut() {
        object.insert("outcome".into(), json!(outcome_name(outcome)));
    }
    tracing::info!(%workspace_id, source = %source.id, outcome = outcome_name(outcome), "plugin index refreshed on request");
    Ok(ApiResponse::success(status))
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HiddenRequest {
    hidden: bool,
}

/// `PUT /sources/{source}/hidden`: hides or shows a configured source for this workspace.
///
/// Workspace owner only. Hiding removes the source from the Public tab and stops imports from it;
/// packages already imported are untouched, and revocations from the source still apply.
pub async fn set_hidden(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    Path((workspace_id, source_id)): Path<(Uuid, String)>,
    Json(body): Json<HiddenRequest>,
) -> Result<impl IntoResponse, ApiError> {
    let caller = Caller::resolve(&state, &claims, None, workspace_id).await?;
    if !caller.is_owner() {
        return Err(ApiError::forbidden(
            "only the workspace owner can hide a plugin index source",
        ));
    }
    let source = configured_source(&source_id)?;
    store::set_hidden(&state.db, workspace_id, &source.id, body.hidden).await?;
    tracing::info!(%workspace_id, source = %source.id, hidden = body.hidden, "plugin index source visibility changed");
    Ok(ApiResponse::success(
        source_status(&state, workspace_id, &source).await?,
    ))
}

#[derive(Debug, Deserialize)]
pub struct EventsQuery {
    page: Option<i64>,
    per_page: Option<i64>,
}

/// `GET /events?page=`: the audit log of the index for this workspace.
///
/// Imports, refusals, trusted keys, revocations applied and the refresh failures of the configured
/// sources, newest first. Workspace owner or admin.
pub async fn list_events(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    Path(workspace_id): Path<Uuid>,
    Query(query): Query<EventsQuery>,
) -> Result<impl IntoResponse, ApiError> {
    let caller = Caller::resolve(&state, &claims, None, workspace_id).await?;
    caller.require_admin()?;
    let paging = PageQuery {
        page: query.page,
        per_page: query.per_page,
    }
    .resolve(DEFAULT_EVENTS_PER_PAGE, MAX_EVENTS_PER_PAGE);
    let configured: Vec<String> = crate::config::runtime()
        .packages
        .index_sources
        .iter()
        .map(|source| source.id.clone())
        .collect();
    let (rows, total) =
        store::list_events(&state.db, workspace_id, &configured, paging.per_page, paging.offset()).await?;
    let items: Vec<Value> = rows
        .into_iter()
        .map(|row| {
            json!({
                "id": row.id,
                "source": row.source_id,
                "action": row.action,
                "actor_id": row.actor_id,
                "details": row.details,
                "created_at": row.created_at,
            })
        })
        .collect();
    Ok(ApiResponse::success(PaginatedData::page(items, total, paging)))
}
