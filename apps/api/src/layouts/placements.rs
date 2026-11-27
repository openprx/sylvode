//! Handlers of the placement routes: where layouts sit in slots, how a slot resolves for the
//! viewer, and the viewer's own preferences.

use std::collections::BTreeMap;

use axum::{
    Extension, Json,
    extract::{Path, Query, State},
    response::IntoResponse,
};
use platform::{app::AppState, auth::JwtClaims};
use sea_orm::{ConnectionTrait, DbBackend, FromQueryResult, Statement, TransactionTrait};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sv_layout::{Context, Options, SlotDescriptor, SlotKind};
use uuid::Uuid;

use super::{
    access::{
        Principal, Scope, ScopeKind, require_person, require_read, require_write, resolve_project, resolve_scope,
        resolve_stored_scope,
    },
    handlers::{invalid, validate_value},
    store::{self, PlacementChange, PrefsChange},
};
use crate::{error::ApiError, middleware::bot_auth::BotAuthContext, request::nullable, response::ApiResponse};

/// The longest placement `condition`, as the column check.
const MAX_CONDITION_CHARS: usize = 1024;

fn placement_not_found() -> ApiError {
    ApiError::not_found("placement not found")
}

/// A `condition` is display-only CEL in the `layout_when` environment: not empty, within the column
/// limit, and accepted by the `sv-rules` compiler. A refusal carries the `cel_*` code and the path
/// `/condition`.
fn check_condition(condition: &str) -> Result<(), ApiError> {
    if condition.trim().is_empty() || condition.chars().count() > MAX_CONDITION_CHARS {
        return Err(ApiError::bad_request_coded(
            "condition_invalid",
            "condition must be 1 to 1024 characters",
        ));
    }
    sv_rules::compile(condition, sv_rules::Env::LayoutWhen).map_err(|e| ApiError::Coded {
        status: 400,
        code: e.code,
        message: "the condition does not compile".to_string(),
        details: Some(serde_json::json!({ "errors": [{ "path": "/condition", "code": e.code }] })),
    })?;
    Ok(())
}

/// A slot's capacity at one scope: a `single` slot has one enabled occupant, a `list` slot up to
/// `max_entries`. `other_enabled` is the number of other enabled placements there.
pub(super) fn check_capacity(slot: &SlotDescriptor, other_enabled: i64) -> Result<(), ApiError> {
    let limit = match slot.kind {
        SlotKind::Single => 1,
        SlotKind::List => i64::from(slot.max_entries),
        SlotKind::Keyed => return Ok(()),
    };
    if other_enabled < limit {
        return Ok(());
    }
    Err(match slot.kind {
        SlotKind::Single => ApiError::conflict_coded("slot_occupied", "the slot already has an occupant here", None),
        _ => ApiError::conflict_coded(
            "slot_full",
            format!("the slot holds at most {limit} entries at one scope"),
            None,
        ),
    })
}

pub(super) fn known_slot(slot_id: &str) -> Result<SlotDescriptor, ApiError> {
    // A slot with no capacity (the `page` slot) is never placed into, so it reads as unknown here.
    sv_layout::slot(slot_id)
        .filter(|slot| slot.max_entries > 0)
        .ok_or_else(|| ApiError::bad_request_coded("slot_unknown", "no slot has this id"))
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CreatePlacementRequest {
    slot_id: String,
    scope_kind: String,
    scope_id: Uuid,
    layout_id: Uuid,
    position: Option<i32>,
    condition: Option<String>,
}

/// `POST /api/v1/placements`
pub async fn create_placement(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    bot: Option<Extension<BotAuthContext>>,
    Json(req): Json<CreatePlacementRequest>,
) -> Result<impl IntoResponse, ApiError> {
    let who = Principal::new(claims, bot);
    let slot = known_slot(&req.slot_id)?;
    let kind = ScopeKind::parse(&req.scope_kind)?;
    if !slot.scope_kinds.contains(&kind.as_str()) {
        return Err(ApiError::bad_request_coded(
            "slot_scope_not_allowed",
            "the slot does not accept placements at this scope kind",
        ));
    }
    let scope = resolve_scope(&state.db, kind, req.scope_id).await?;
    require_write(&state, &who, &scope).await?;
    if let Some(condition) = &req.condition {
        check_condition(condition)?;
    }

    let layout = store::load_layout(&state.db, req.layout_id)
        .await?
        .filter(|row| row.status == "active" && row.installation_id.is_none())
        .filter(|row| scope.lies_inside(&row.scope_kind, row.scope_id, row.workspace_id))
        .ok_or_else(|| ApiError::not_found("layout not found"))?;
    let opts = Options {
        slot: Some(&slot),
        context: Context::Console,
    };
    if let Err(errors) = validate_value(&layout.spec, &opts) {
        return Err(invalid("slot_node_not_allowed", &errors));
    }

    let tx = state.db.begin().await?;
    store::lock_slot_scope(&tx, &req.slot_id, kind.as_str(), scope.id).await?;
    let others = store::count_enabled(&tx, &req.slot_id, kind.as_str(), scope.id, None).await?;
    check_capacity(&slot, others)?;
    let row = store::insert_placement(
        &tx,
        scope.workspace_id,
        &req.slot_id,
        kind.as_str(),
        scope.id,
        layout.id,
        req.position.unwrap_or(0),
        req.condition.as_deref(),
        who.user_id()?,
    )
    .await?;
    tx.commit().await?;
    Ok(ApiResponse::success(store::Placement::from(row)))
}

/// Loads a placement and the scope it sits at; both failures read as `not_found`.
async fn load_with_scope(state: &AppState, id: Uuid) -> Result<(store::PlacementRow, Scope), ApiError> {
    let row = store::load_placement(&state.db, id)
        .await?
        .ok_or_else(placement_not_found)?;
    let scope = resolve_stored_scope(&state.db, &row.scope_kind, row.scope_id)
        .await
        .map_err(|_| placement_not_found())?;
    Ok((row, scope))
}

fn ensure_user_placement(row: &store::PlacementRow) -> Result<(), ApiError> {
    if row.installation_id.is_some() {
        return Err(ApiError::conflict_coded(
            "layout_managed_by_package",
            "this placement belongs to a package installation",
            None,
        ));
    }
    Ok(())
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
#[allow(
    clippy::option_option,
    reason = "absent, null and a value are three different requests"
)]
pub struct UpdatePlacementRequest {
    position: Option<i32>,
    enabled: Option<bool>,
    #[serde(default, deserialize_with = "nullable")]
    condition: Option<Option<String>>,
}

/// `PUT /api/v1/placements/{id}`
pub async fn update_placement(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    bot: Option<Extension<BotAuthContext>>,
    Path(id): Path<Uuid>,
    Json(req): Json<UpdatePlacementRequest>,
) -> Result<impl IntoResponse, ApiError> {
    let who = Principal::new(claims, bot);
    let (row, scope) = load_with_scope(&state, id).await?;
    require_write(&state, &who, &scope).await?;
    ensure_user_placement(&row)?;
    if let Some(Some(condition)) = &req.condition {
        check_condition(condition)?;
    }
    let slot = known_slot(&row.slot_id)?;

    let tx = state.db.begin().await?;
    store::lock_slot_scope(&tx, &row.slot_id, &row.scope_kind, row.scope_id).await?;
    if req.enabled == Some(true) && !row.enabled {
        let others = store::count_enabled(&tx, &row.slot_id, &row.scope_kind, row.scope_id, Some(row.id)).await?;
        check_capacity(&slot, others)?;
    }
    let updated = store::update_placement(
        &tx,
        id,
        &PlacementChange {
            position: req.position,
            enabled: req.enabled,
            condition: req.condition,
        },
    )
    .await?;
    tx.commit().await?;
    Ok(ApiResponse::success(store::Placement::from(updated)))
}

/// `DELETE /api/v1/placements/{id}`
pub async fn delete_placement(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    bot: Option<Extension<BotAuthContext>>,
    Path(id): Path<Uuid>,
) -> Result<impl IntoResponse, ApiError> {
    let who = Principal::new(claims, bot);
    let (row, scope) = load_with_scope(&state, id).await?;
    require_write(&state, &who, &scope).await?;
    ensure_user_placement(&row)?;
    store::delete_placement(&state.db, id).await?;
    Ok(ApiResponse::success(json!({ "id": id })))
}

#[derive(Debug, Deserialize)]
#[allow(clippy::struct_field_names, reason = "the field names are the query parameter names")]
pub struct ResolveQuery {
    project_id: Uuid,
    form_id: Option<Uuid>,
    record_id: Option<Uuid>,
}

#[derive(Debug, Serialize)]
struct Entry {
    placement_id: Uuid,
    layout_id: Uuid,
    layout_key: String,
    spec: Value,
    position: i32,
    condition: Option<String>,
    collapsed: bool,
    installation_id: Option<Uuid>,
}

#[derive(Debug, Serialize)]
struct Resolved {
    slot: SlotDescriptor,
    entries: Vec<Entry>,
    /// Active installations of the scope that the entries reference, by package key. Empty until
    /// the install flow stores package placements.
    installations: BTreeMap<String, Uuid>,
}

/// Checks that the form is in the project and the record in the form, else `not_found`.
async fn bind_context(
    db: &impl ConnectionTrait,
    project_id: Uuid,
    form_id: Option<Uuid>,
    record_id: Option<Uuid>,
) -> Result<(), ApiError> {
    #[derive(FromQueryResult)]
    struct Hit {
        #[allow(dead_code, reason = "the row's existence is the answer")]
        id: Uuid,
    }
    let not_found = || ApiError::not_found("context not found");
    if let Some(form_id) = form_id {
        let form = Hit::find_by_statement(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT id FROM project_forms WHERE id = $1 AND project_id = $2",
            vec![form_id.into(), project_id.into()],
        ))
        .one(db)
        .await?;
        if form.is_none() {
            return Err(not_found());
        }
    }
    if let Some(record_id) = record_id {
        let Some(form_id) = form_id else {
            return Err(ApiError::bad_request_coded(
                "context_invalid",
                "record_id needs form_id",
            ));
        };
        let record = Hit::find_by_statement(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT id FROM form_records WHERE id = $1 AND form_id = $2 AND project_id = $3",
            vec![record_id.into(), form_id.into(), project_id.into()],
        ))
        .one(db)
        .await?;
        if record.is_none() {
            return Err(not_found());
        }
    }
    Ok(())
}

/// `GET /api/v1/slots/{slot_id}/placements?project_id=&form_id=&record_id=`
pub async fn resolve_slot(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    bot: Option<Extension<BotAuthContext>>,
    Path(slot_id): Path<String>,
    Query(query): Query<ResolveQuery>,
) -> Result<impl IntoResponse, ApiError> {
    let who = Principal::new(claims, bot);
    let slot = sv_layout::slot(&slot_id).ok_or_else(|| ApiError::not_found("slot not found"))?;
    let project = resolve_project(&state.db, query.project_id).await?;
    require_read(&state, &who, &project).await?;
    bind_context(&state.db, query.project_id, query.form_id, query.record_id).await?;
    for (member, given) in [
        ("form_id", query.form_id.is_some()),
        ("record_id", query.record_id.is_some()),
    ] {
        if slot.context_schema.contains_key(member) && !given {
            return Err(ApiError::bad_request_coded(
                "context_invalid",
                format!("this slot needs {member}"),
            ));
        }
    }
    let rows = store::resolve_slot(
        &state.db,
        &slot_id,
        project.workspace_id,
        query.project_id,
        query.form_id,
        who.user_id()?,
    )
    .await?;
    let entries = rows
        .into_iter()
        .take(slot.max_entries as usize)
        .map(|row| Entry {
            placement_id: row.placement_id,
            layout_id: row.layout_id,
            layout_key: row.layout_key,
            spec: row.spec,
            position: row.position,
            condition: row.condition,
            collapsed: row.collapsed,
            installation_id: row.installation_id,
        })
        .collect();
    Ok(ApiResponse::success(Resolved {
        slot,
        entries,
        installations: BTreeMap::new(),
    }))
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
#[allow(
    clippy::option_option,
    reason = "absent, null and a value are three different requests"
)]
pub struct PrefsRequest {
    hidden: Option<bool>,
    collapsed: Option<bool>,
    #[serde(default, deserialize_with = "nullable")]
    position: Option<Option<i32>>,
}

/// `PUT /api/v1/me/placements/{id}/prefs`: only for a placement the caller can resolve.
pub async fn put_prefs(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    bot: Option<Extension<BotAuthContext>>,
    Path(id): Path<Uuid>,
    Json(req): Json<PrefsRequest>,
) -> Result<impl IntoResponse, ApiError> {
    let who = Principal::new(claims, bot);
    let user_id = require_person(&who)?;
    let (row, scope) = load_with_scope(&state, id).await?;
    require_read(&state, &who, &scope).await?;
    if row.installation_id.is_some() {
        return Err(placement_not_found());
    }
    let prefs = store::upsert_prefs(
        &state.db,
        user_id,
        id,
        &PrefsChange {
            hidden: req.hidden,
            collapsed: req.collapsed,
            position: req.position,
        },
    )
    .await?;
    Ok(ApiResponse::success(prefs))
}
