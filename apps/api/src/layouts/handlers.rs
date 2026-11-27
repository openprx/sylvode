//! Handlers of the layout routes: CRUD, the validator endpoint and the catalogues.

use axum::{
    Extension, Json,
    extract::{Path, Query, State},
    response::IntoResponse,
};
use platform::{app::AppState, auth::JwtClaims};
use sea_orm::TransactionTrait;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sv_layout::{Analysis, Context, LayoutError, Options, SlotDescriptor, Validated};
use uuid::Uuid;

use super::{
    access::{Principal, ScopeKind, authenticated, require_read, require_write, resolve_scope, resolve_stored_scope},
    store::{self, LayoutRow, LayoutSummary},
};
use crate::{
    error::ApiError,
    middleware::bot_auth::BotAuthContext,
    request::nullable,
    response::{ApiResponse, PageQuery, PaginatedData},
};

/// `Layout` of contract C3: the summary, the stored spec and its capability analysis.
#[derive(Debug, Serialize)]
pub struct Layout {
    #[serde(flatten)]
    summary: LayoutSummary,
    spec: Value,
    /// Computed from the stored spec on every read; `null` when the spec no longer validates
    /// against the current catalogue.
    analysis: Option<Analysis>,
}

fn layout_body(row: &LayoutRow) -> Layout {
    let analysis = validate_value(&row.spec, &Options::default()).ok().map(|v| v.analysis);
    Layout {
        summary: LayoutSummary::from(row),
        spec: row.spec.clone(),
        analysis,
    }
}

/// Validates a spec value; the serialised size is checked first, like an uploaded document.
pub(super) fn validate_value(spec: &Value, opts: &Options<'_>) -> Result<Validated, Vec<LayoutError>> {
    serde_json::to_vec(spec).map_or_else(
        |_| sv_layout::validate(spec, opts),
        |bytes| sv_layout::validate_bytes(&bytes, opts),
    )
}

/// The refusal of an invalid spec: `layout_invalid` with every error and its node path.
pub(super) fn invalid(code: &'static str, errors: &[LayoutError]) -> ApiError {
    ApiError::Coded {
        status: 400,
        code,
        message: "the layout does not validate".to_string(),
        details: Some(json!({ "errors": errors })),
    }
}

fn console_validate(spec: &Value, slot: Option<&SlotDescriptor>) -> Result<Validated, ApiError> {
    validate_value(
        spec,
        &Options {
            slot,
            context: Context::Console,
        },
    )
    .map_err(|errors| invalid("layout_invalid", &errors))
}

/// A layout stored by the install flow is read-only here.
fn ensure_user_layout(row: &LayoutRow) -> Result<(), ApiError> {
    if row.installation_id.is_some() || row.source == "package" {
        return Err(ApiError::conflict_coded(
            "layout_managed_by_package",
            "this layout belongs to a package installation",
            None,
        ));
    }
    Ok(())
}

fn layout_not_found() -> ApiError {
    ApiError::not_found("layout not found")
}

/// Loads a layout and resolves its scope; both failures read as `not_found`.
async fn load_with_scope(state: &AppState, id: Uuid) -> Result<(LayoutRow, super::access::Scope), ApiError> {
    let row = store::load_layout(&state.db, id).await?.ok_or_else(layout_not_found)?;
    let scope = resolve_stored_scope(&state.db, &row.scope_kind, row.scope_id)
        .await
        .map_err(|_| layout_not_found())?;
    Ok((row, scope))
}

#[derive(Debug, Deserialize)]
pub struct ListQuery {
    scope_kind: String,
    scope_id: Uuid,
    #[serde(flatten)]
    page: PageQuery,
}

/// `GET /api/v1/layouts?scope_kind=&scope_id=`
pub async fn list_layouts(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    bot: Option<Extension<BotAuthContext>>,
    Query(query): Query<ListQuery>,
) -> Result<impl IntoResponse, ApiError> {
    let who = Principal::new(claims, bot);
    let scope = resolve_scope(&state.db, ScopeKind::parse(&query.scope_kind)?, query.scope_id).await?;
    require_read(&state, &who, &scope).await?;
    let paging = query.page.resolve(50, 200);
    let (rows, total) = store::list_layouts(
        &state.db,
        scope.kind.as_str(),
        scope.id,
        paging.per_page,
        paging.offset(),
    )
    .await?;
    let items = rows.iter().map(LayoutSummary::from).collect();
    Ok(ApiResponse::success(PaginatedData::page(items, total, paging)))
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CreateLayoutRequest {
    scope_kind: String,
    scope_id: Uuid,
    spec: Value,
}

/// `POST /api/v1/layouts`
pub async fn create_layout(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    bot: Option<Extension<BotAuthContext>>,
    Json(req): Json<CreateLayoutRequest>,
) -> Result<impl IntoResponse, ApiError> {
    let who = Principal::new(claims, bot);
    let scope = resolve_scope(&state.db, ScopeKind::parse(&req.scope_kind)?, req.scope_id).await?;
    require_write(&state, &who, &scope).await?;
    let validated = console_validate(&req.spec, None)?;
    let row = store::insert_layout(
        &state.db,
        scope.workspace_id,
        scope.kind.as_str(),
        scope.id,
        &validated.layout.key,
        &req.spec,
        "user",
        who.user_id()?,
    )
    .await?;
    Ok(ApiResponse::success(layout_body(&row)))
}

/// `GET /api/v1/layouts/{id}`
pub async fn get_layout(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    bot: Option<Extension<BotAuthContext>>,
    Path(id): Path<Uuid>,
) -> Result<impl IntoResponse, ApiError> {
    let who = Principal::new(claims, bot);
    let (row, scope) = load_with_scope(&state, id).await?;
    require_read(&state, &who, &scope).await?;
    if row.installation_id.is_some() {
        return Err(layout_not_found());
    }
    Ok(ApiResponse::success(layout_body(&row)))
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UpdateLayoutRequest {
    spec: Value,
    expected_version: i32,
}

/// `PATCH /api/v1/layouts/{id}`
pub async fn update_layout(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    bot: Option<Extension<BotAuthContext>>,
    Path(id): Path<Uuid>,
    Json(req): Json<UpdateLayoutRequest>,
) -> Result<impl IntoResponse, ApiError> {
    let who = Principal::new(claims, bot);
    let (row, scope) = load_with_scope(&state, id).await?;
    require_write(&state, &who, &scope).await?;
    ensure_user_layout(&row)?;
    let validated = console_validate(&req.spec, None)?;

    let tx = state.db.begin().await?;
    // A layout that sits in slots must still fit each of them.
    for slot_id in store::slots_using_layout(&tx, id).await? {
        if let Some(slot) = sv_layout::slot(&slot_id) {
            let opts = Options {
                slot: Some(&slot),
                context: Context::Console,
            };
            if let Err(errors) = validate_value(&req.spec, &opts) {
                return Err(invalid("slot_node_not_allowed", &errors));
            }
        }
    }
    let updated = store::update_layout(&tx, id, req.expected_version, &validated.layout.key, &req.spec)
        .await?
        .ok_or_else(|| {
            ApiError::conflict_coded(
                "version_conflict",
                "the layout changed since it was read",
                Some(json!({ "current_version": row.version })),
            )
        })?;
    tx.commit().await?;
    Ok(ApiResponse::success(layout_body(&updated)))
}

/// `DELETE /api/v1/layouts/{id}`: archives; its placements stop resolving.
pub async fn delete_layout(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    bot: Option<Extension<BotAuthContext>>,
    Path(id): Path<Uuid>,
) -> Result<impl IntoResponse, ApiError> {
    let who = Principal::new(claims, bot);
    let (row, scope) = load_with_scope(&state, id).await?;
    require_write(&state, &who, &scope).await?;
    ensure_user_layout(&row)?;
    store::archive_layout(&state.db, id).await?;
    Ok(ApiResponse::success(json!({ "id": id, "status": "archived" })))
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
#[allow(
    clippy::option_option,
    reason = "absent, null and a value are three different requests"
)]
pub struct ValidateRequest {
    spec: Value,
    #[serde(default, deserialize_with = "nullable")]
    slot_id: Option<Option<String>>,
    #[serde(default, deserialize_with = "nullable")]
    site_id: Option<Option<Uuid>>,
    #[serde(default, deserialize_with = "nullable")]
    scope_kind: Option<Option<String>>,
    #[serde(default, deserialize_with = "nullable")]
    scope_id: Option<Option<Uuid>>,
}

/// `POST /api/v1/layouts/validate`: always answers; the verdict is in `valid`.
pub async fn validate_layout(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    bot: Option<Extension<BotAuthContext>>,
    Json(req): Json<ValidateRequest>,
) -> Result<impl IntoResponse, ApiError> {
    let who = Principal::new(claims, bot);
    authenticated(&who)?;
    if req.site_id.flatten().is_some() {
        return Err(ApiError::bad_request_coded(
            "site_context_unsupported",
            "site layouts are validated when their site page is saved",
        ));
    }
    // A named scope must be one the caller can read; the verdict itself does not depend on it.
    if let (Some(Some(kind)), Some(Some(id))) = (&req.scope_kind, req.scope_id) {
        let scope = resolve_scope(&state.db, ScopeKind::parse(kind)?, id).await?;
        require_read(&state, &who, &scope).await?;
    }
    let slot = match req.slot_id.flatten() {
        Some(slot_id) => Some(
            sv_layout::slot(&slot_id)
                .ok_or_else(|| ApiError::bad_request_coded("slot_unknown", "no slot has this id"))?,
        ),
        None => None,
    };
    let opts = Options {
        slot: slot.as_ref(),
        context: Context::Console,
    };
    Ok(ApiResponse::success(match validate_value(&req.spec, &opts) {
        Ok(validated) => json!({ "valid": true, "errors": [], "analysis": validated.analysis }),
        Err(errors) => json!({ "valid": false, "errors": errors, "analysis": null }),
    }))
}

/// `GET /api/v1/layouts/components`
#[allow(clippy::unused_async, reason = "axum handlers are async functions")]
pub async fn list_components(
    Extension(claims): Extension<JwtClaims>,
    bot: Option<Extension<BotAuthContext>>,
) -> Result<impl IntoResponse, ApiError> {
    authenticated(&Principal::new(claims, bot))?;
    Ok(ApiResponse::success(
        json!({ "version": 1, "components": sv_layout::descriptors() }),
    ))
}

/// `GET /api/v1/slots`
#[allow(clippy::unused_async, reason = "axum handlers are async functions")]
pub async fn list_slots(
    Extension(claims): Extension<JwtClaims>,
    bot: Option<Extension<BotAuthContext>>,
) -> Result<impl IntoResponse, ApiError> {
    authenticated(&Principal::new(claims, bot))?;
    Ok(ApiResponse::success(json!({ "slots": sv_layout::slots() })))
}
