//! Handlers of the installation routes (contract C1 §6, C3 §5).

use std::time::Instant;

use axum::{
    Extension, Json,
    extract::{Path, Query, State},
    response::IntoResponse,
};
use platform::{app::AppState, auth::JwtClaims};
use sea_orm::{ConnectionTrait, DbBackend, FromQueryResult, Statement, TransactionTrait};
use serde::Deserialize;
use serde_json::{Value, json};
use uuid::Uuid;

use super::access::{self, Requester};
use super::approve::{ApproveRequest, approve};
use super::artefacts;
use super::rate;
use super::review::{Existing, preview};
use super::store::{self, InstallationRow};
use super::validate::{self, Check};
use crate::catalogue::{Refusal, RefusalCode};
use crate::error::ApiError;
use crate::extensions::rules;
use crate::middleware::bot_auth::BotAuthContext;
use crate::packages::open::{self, Opened, StoredPackage};
use crate::response::ApiResponse;

fn refuse(code: RefusalCode) -> ApiError {
    Refusal::new(code, None).into()
}

fn not_found() -> ApiError {
    ApiError::not_found("installation not found")
}

/// `value` (a JSON object) with one more member.
fn with_field(mut value: Value, key: &str, member: Value) -> Value {
    if let Some(object) = value.as_object_mut() {
        object.insert(key.to_owned(), member);
    }
    value
}

fn empty_object() -> Value {
    json!({})
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ScopeInput {
    kind: String,
    #[serde(default)]
    project_id: Option<Uuid>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RequestBody {
    package_digest: String,
    scope: ScopeInput,
    #[serde(default = "empty_object")]
    bind_map: Value,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PreviewBody {
    package_digest: String,
    scope: ScopeInput,
    #[serde(default = "empty_object")]
    bind_map: Value,
    /// Reserved for the site lane's `public` list of the preview.
    #[serde(default)]
    #[allow(dead_code, reason = "accepted so the preview body matches contract C3 §5.1")]
    site_id: Option<Uuid>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UpgradeBody {
    package_digest: String,
    #[serde(default = "empty_object")]
    bind_map: Value,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GenerationBody {
    generation: i32,
}

/// What a request, an upgrade and a preview all start from.
struct Prepared {
    stored: StoredPackage,
    opened: Opened,
    check: Check,
}

async fn project_exists(state: &AppState, workspace_id: Uuid, project_id: Uuid) -> Result<(), ApiError> {
    state
        .db
        .query_one(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT 1 FROM projects WHERE id = $1 AND workspace_id = $2",
            vec![project_id.into(), workspace_id.into()],
        ))
        .await?
        .map(|_| ())
        .ok_or_else(|| ApiError::not_found("project not found"))
}

async fn prepare(
    state: &AppState,
    workspace_id: Uuid,
    project_id: Option<Uuid>,
    digest: &str,
    bind_map: &Value,
) -> Result<Prepared, ApiError> {
    let stored = open::find(&state.db, workspace_id, digest)
        .await?
        .ok_or_else(|| ApiError::not_found("package not found"))?;
    if !stored.installable || stored.key_status != "active" {
        return Err(refuse(RefusalCode::InstallationPackageRevoked));
    }
    let opened = open::open(&state.db, workspace_id, &stored).await?;
    let check = validate::check(state, workspace_id, project_id, &opened, bind_map).await?;
    Ok(Prepared { stored, opened, check })
}

fn scope_of(input: &ScopeInput) -> Result<Option<Uuid>, ApiError> {
    validate::scope_error(&input.kind, input.project_id).map_or(Ok(input.project_id), |message| {
        Err(ApiError::bad_request_coded("scope_invalid", message))
    })
}

fn requested_ids(opened: &Opened) -> Vec<String> {
    let mut ids: Vec<String> = opened
        .package
        .manifest
        .requests
        .iter()
        .map(|request| request.id.clone())
        .collect();
    ids.sort();
    ids.dedup();
    ids
}

/// The `Installation` shape of contract C3 §5.2, with the grants that stopped being honoured.
async fn installation_json(db: &impl ConnectionTrait, row: &InstallationRow) -> Result<Value, ApiError> {
    let generations = store::generations_of(db, row.id).await?;
    let active = generations
        .iter()
        .find(|generation| Some(generation.generation) == row.active_generation && generation.status == "approved");
    let grants = match active {
        Some(active) => store::grants_of(db, active.id).await?,
        None => Vec::new(),
    };
    let changed: Vec<Value> = grants
        .iter()
        .filter(|grant| grant.current_digest.as_deref() != Some(grant.plugin_ops_digest.as_str()))
        .map(|grant| json!({"capability": grant.capability, "selector": grant.selector}))
        .collect();
    // Whether the administrator who approved the active generation still is one; `false` asks for a
    // new approval (contract C1 §3, N-1).
    let approver_active = match active.and_then(|generation| generation.reviewed_by) {
        Some(approver) => Some(
            super::live::approver_is_admin(db, row.workspace_id, approver)
                .await
                .map_err(|_| ApiError::internal())?,
        ),
        None => active.map(|_| false),
    };
    Ok(json!({
        "id": row.id,
        "approver_active": approver_active,
        "workspace_id": row.workspace_id,
        "scope": {"kind": row.scope_kind, "project_id": row.project_id},
        "package_key": row.package_key,
        "status": row.status,
        "disabled_reason": row.disabled_reason,
        "active_generation": row.active_generation,
        "generations": generations.iter().map(store::GenerationRow::to_json).collect::<Vec<_>>(),
        "grants": grants.iter().map(|grant| json!({"capability": grant.capability, "selector": grant.selector})).collect::<Vec<_>>(),
        "changed_grants": changed,
        "created_at": row.created_at,
    }))
}

async fn reload_rules(state: &AppState) {
    if let Err(error) = rules::refresh(&state.db).await {
        tracing::warn!(target: "sv::extensions", %error, "the plugin rule registry was not refreshed");
    }
}

/// `POST /api/v1/workspaces/{workspace_id}/installations`: stores the installation and generation 1
/// as pending. Nothing the package brings is created before an administrator approves.
pub async fn request_installation(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    bot: Option<Extension<BotAuthContext>>,
    Path(workspace_id): Path<Uuid>,
    Json(body): Json<RequestBody>,
) -> Result<impl IntoResponse, ApiError> {
    let requester = access::requester(&state, &claims, bot, workspace_id).await?;
    rate::requests().hit(workspace_id, Instant::now())?;
    let project_id = scope_of(&body.scope)?;
    if let Some(project) = project_id {
        project_exists(&state, workspace_id, project).await?;
    }
    let prepared = prepare(&state, workspace_id, project_id, &body.package_digest, &body.bind_map).await?;
    if !prepared.check.valid() {
        return Err(validate::invalid(&prepared.check.problems));
    }
    let manifest = &prepared.opened.package.manifest;
    let Requester { kind, id } = requester;
    let tx = state.db.begin().await?;
    let installation = store::insert_installation(&tx, workspace_id, project_id, &manifest.id, kind, id).await?;
    store::insert_generation(
        &tx,
        installation.id,
        1,
        prepared.stored.id,
        &requested_ids(&prepared.opened),
        &body.bind_map,
        kind,
        id,
    )
    .await?;
    tx.commit().await?;
    tracing::info!(target: "sv::extensions", installation = %installation.id, %workspace_id, "installation requested");
    Ok(ApiResponse::success(installation_json(&state.db, &installation).await?))
}

/// `POST /api/v1/installations/{installation_id}/generations`: a new version of the package of the
/// installation, pending while the active generation keeps serving.
pub async fn upgrade_installation(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    bot: Option<Extension<BotAuthContext>>,
    Path(installation_id): Path<Uuid>,
    Json(body): Json<UpgradeBody>,
) -> Result<impl IntoResponse, ApiError> {
    let probe = store::load_installation(&state.db, installation_id)
        .await?
        .ok_or_else(not_found)?;
    let requester = access::requester(&state, &claims, bot, probe.workspace_id).await?;
    rate::requests().hit(probe.workspace_id, Instant::now())?;
    let prepared = prepare(
        &state,
        probe.workspace_id,
        probe.project_id,
        &body.package_digest,
        &body.bind_map,
    )
    .await?;
    if prepared.stored.package_key != probe.package_key {
        return Err(refuse(RefusalCode::InstallationPackageMismatch));
    }
    if !prepared.check.valid() {
        return Err(validate::invalid(&prepared.check.problems));
    }
    let Requester { kind, id } = requester;
    let tx = state.db.begin().await?;
    let installation = store::lock_installation(&tx, installation_id)
        .await?
        .ok_or_else(not_found)?;
    if installation.status == "revoked" {
        return Err(refuse(RefusalCode::InstallationRevoked));
    }
    let generations = store::generations_of(&tx, installation_id).await?;
    if generations
        .iter()
        .any(|generation| generation.package_digest == prepared.stored.digest && generation.status != "rejected")
    {
        return Err(refuse(RefusalCode::InstallationExists));
    }
    let next = store::next_generation(&tx, installation_id).await?;
    store::insert_generation(
        &tx,
        installation_id,
        next,
        prepared.stored.id,
        &requested_ids(&prepared.opened),
        &body.bind_map,
        kind,
        id,
    )
    .await?;
    tx.commit().await?;
    Ok(ApiResponse::success(installation_json(&state.db, &installation).await?))
}

/// `POST /api/v1/workspaces/{workspace_id}/installations/preview`: the `InstallPreview`, no write.
pub async fn preview_installation(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    bot: Option<Extension<BotAuthContext>>,
    Path(workspace_id): Path<Uuid>,
    Json(body): Json<PreviewBody>,
) -> Result<impl IntoResponse, ApiError> {
    access::requester(&state, &claims, bot, workspace_id).await?;
    rate::previews().hit(workspace_id, Instant::now())?;
    let project_id = scope_of(&body.scope)?;
    if let Some(project) = project_id {
        project_exists(&state, workspace_id, project).await?;
    }
    let prepared = prepare(&state, workspace_id, project_id, &body.package_digest, &body.bind_map).await?;
    let existing = existing_of(&state, workspace_id, project_id, &prepared.stored.package_key).await?;
    let generation = existing.next_generation;
    Ok(ApiResponse::success(preview(
        &prepared.opened,
        &prepared.check,
        &body.scope.kind,
        generation,
        &existing.view,
        &json!({}),
    )))
}

struct ExistingState {
    view: Existing,
    next_generation: i32,
}

async fn existing_of(
    state: &AppState,
    workspace_id: Uuid,
    project_id: Option<Uuid>,
    package_key: &str,
) -> Result<ExistingState, ApiError> {
    let Some(row) = store::find_live(&state.db, workspace_id, project_id, package_key).await? else {
        return Ok(ExistingState {
            view: Existing::default(),
            next_generation: 1,
        });
    };
    let generations = store::generations_of(&state.db, row.id).await?;
    let active_requests = generations
        .iter()
        .find(|generation| Some(generation.generation) == row.active_generation && generation.status == "approved")
        .map(|generation| generation.requested_capabilities.iter().cloned().collect())
        .unwrap_or_default();
    Ok(ExistingState {
        next_generation: generations
            .iter()
            .map(|generation| generation.generation)
            .max()
            .unwrap_or(0)
            + 1,
        view: Existing {
            installation_id: Some(row.id),
            active_generation: row.active_generation,
            active_requests,
        },
    })
}

/// `GET /api/v1/workspaces/{workspace_id}/installations`
pub async fn list_installations(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    Path(workspace_id): Path<Uuid>,
) -> Result<impl IntoResponse, ApiError> {
    access::admin_user(&state, &claims, workspace_id).await?;
    let rows = store::list_installations(&state.db, workspace_id).await?;
    let mut items = Vec::with_capacity(rows.len());
    for row in &rows {
        items.push(installation_json(&state.db, row).await?);
    }
    Ok(ApiResponse::success(json!({ "items": items, "total": items.len() })))
}

/// `GET /api/v1/installations/{installation_id}`
pub async fn get_installation(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    Path(installation_id): Path<Uuid>,
) -> Result<impl IntoResponse, ApiError> {
    let (row, _) = access::installation_for_admin(&state, &claims, installation_id).await?;
    let answer = installation_json(&state.db, &row).await?;
    // What a disable or revoke would stop, so the confirmation can name it before the change.
    let forms = forms_using_rules(&state.db, &row).await?;
    Ok(ApiResponse::success(with_field(
        answer,
        "forms_using_rules",
        Value::Array(forms),
    )))
}

/// `GET /api/v1/installations/{installation_id}/review`: the preview of the pending generation.
///
/// With none pending it previews the active one, computed from the package's verified bytes and
/// the current state.
///
/// `changed_grants` lists grants that must be approved again.
pub async fn review_installation(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    Path(installation_id): Path<Uuid>,
) -> Result<impl IntoResponse, ApiError> {
    let (row, _) = access::installation_for_admin(&state, &claims, installation_id).await?;
    let generations = store::generations_of(&state.db, row.id).await?;
    let subject = generations
        .iter()
        .find(|generation| generation.status == "pending")
        .or_else(|| {
            generations
                .iter()
                .find(|generation| Some(generation.generation) == row.active_generation)
        })
        .ok_or_else(not_found)?;
    let stored = open::find_by_row(&state.db, row.workspace_id, subject.package_row_id)
        .await?
        .ok_or_else(|| ApiError::not_found("package not found"))?;
    let opened = open::open(&state.db, row.workspace_id, &stored).await?;
    let check = validate::check(&state, row.workspace_id, row.project_id, &opened, &subject.bind_map).await?;
    let existing = existing_of(&state, row.workspace_id, row.project_id, &row.package_key).await?;
    let installation = installation_json(&state.db, &row).await?;
    Ok(ApiResponse::success(preview(
        &opened,
        &check,
        &row.scope_kind,
        subject.generation,
        &existing.view,
        &json!({
            "installation": installation,
            "pending": subject.status == "pending",
        }),
    )))
}

/// `POST /api/v1/installations/{installation_id}/approve`
pub async fn approve_installation(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    Path(installation_id): Path<Uuid>,
    Json(body): Json<ApproveRequest>,
) -> Result<impl IntoResponse, ApiError> {
    let (_, admin) = access::installation_for_admin(&state, &claims, installation_id).await?;
    approve(&state, admin, installation_id, &body).await?;
    let row = store::load_installation(&state.db, installation_id)
        .await?
        .ok_or_else(not_found)?;
    tracing::info!(target: "sv::extensions", installation = %installation_id, generation = body.generation, "installation approved");
    Ok(ApiResponse::success(installation_json(&state.db, &row).await?))
}

/// `POST /api/v1/installations/{installation_id}/reject`
pub async fn reject_generation(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    Path(installation_id): Path<Uuid>,
    Json(body): Json<GenerationBody>,
) -> Result<impl IntoResponse, ApiError> {
    let (_, admin) = access::installation_for_admin(&state, &claims, installation_id).await?;
    let tx = state.db.begin().await?;
    let row = store::lock_installation(&tx, installation_id)
        .await?
        .ok_or_else(not_found)?;
    if row.status == "revoked" {
        return Err(refuse(RefusalCode::InstallationRevoked));
    }
    let generation = store::generation(&tx, installation_id, body.generation)
        .await?
        .filter(|generation| generation.status == "pending")
        .ok_or_else(|| refuse(RefusalCode::InstallationGenerationStale))?;
    store::review_generation(&tx, generation.id, "rejected", admin).await?;
    tx.commit().await?;
    Ok(ApiResponse::success(installation_json(&state.db, &row).await?))
}

/// The forms whose schemas reference a rule of the installation's package: writes to them stop
/// while the installation is not active, so the confirmation of a disable or revoke lists them.
async fn forms_using_rules(db: &impl ConnectionTrait, row: &InstallationRow) -> Result<Vec<Value>, ApiError> {
    #[derive(FromQueryResult)]
    struct Form {
        id: Uuid,
        project_id: Uuid,
        key: String,
        name: String,
    }
    let forms = Form::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "SELECT id, project_id, key, name FROM project_forms \
         WHERE workspace_id = $1 AND ($2::uuid IS NULL OR project_id = $2) \
           AND schema::text LIKE '%\"package\": \"' || $3::text || '\"%' ORDER BY name",
        vec![
            row.workspace_id.into(),
            row.project_id.into(),
            row.package_key.clone().into(),
        ],
    ))
    .all(db)
    .await?;
    Ok(forms
        .into_iter()
        .map(|form| json!({"id": form.id, "project_id": form.project_id, "key": form.key, "name": form.name}))
        .collect())
}

/// Disables, enables or revokes, under the installation's row lock.
enum Change {
    Disable,
    Enable,
    Revoke,
}

async fn change_status(
    state: &AppState,
    claims: &JwtClaims,
    installation_id: Uuid,
    change: Change,
) -> Result<Value, ApiError> {
    let (_, admin) = access::installation_for_admin(state, claims, installation_id).await?;
    let tx = state.db.begin().await?;
    let row = store::lock_installation(&tx, installation_id)
        .await?
        .ok_or_else(not_found)?;
    if row.status == "revoked" {
        return Err(refuse(RefusalCode::InstallationRevoked));
    }
    match change {
        Change::Disable => {
            if row.status != "active" {
                return Err(refuse(RefusalCode::InstallationNotActive));
            }
            tx.execute(Statement::from_sql_and_values(
                DbBackend::Postgres,
                "UPDATE installations SET status = 'disabled', disabled_reason = 'admin', updated_at = now() WHERE id = $1",
                vec![installation_id.into()],
            ))
            .await?;
        }
        Change::Enable => enable(&tx, &row).await?,
        Change::Revoke => {
            tx.execute(Statement::from_sql_and_values(
                DbBackend::Postgres,
                "UPDATE installations SET status = 'revoked', disabled_reason = NULL, revoked_at = now(), \
                   revoked_by = $2, updated_at = now() WHERE id = $1",
                vec![installation_id.into(), admin.into()],
            ))
            .await?;
            artefacts::delete_all(&tx, installation_id).await?;
        }
    }
    tx.commit().await?;
    reload_rules(state).await;
    let after = store::load_installation(&state.db, installation_id)
        .await?
        .ok_or_else(not_found)?;
    let answer = installation_json(&state.db, &after).await?;
    if matches!(change, Change::Enable) {
        return Ok(answer);
    }
    let forms = forms_using_rules(&state.db, &after).await?;
    Ok(with_field(answer, "forms_using_rules", Value::Array(forms)))
}

/// Enabling needs a package that is still installable and, after `approver_lost`, an approver who is
/// an administrator again (otherwise the installation must be approved again).
async fn enable(tx: &impl ConnectionTrait, row: &InstallationRow) -> Result<(), ApiError> {
    if row.status != "disabled" {
        return Err(refuse(RefusalCode::InstallationNotActive));
    }
    let generations = store::generations_of(tx, row.id).await?;
    let active = generations
        .iter()
        .find(|generation| Some(generation.generation) == row.active_generation && generation.status == "approved")
        .ok_or_else(|| refuse(RefusalCode::InstallationNotActive))?;
    let stored = open::find_by_row(tx, row.workspace_id, active.package_row_id)
        .await?
        .ok_or_else(|| ApiError::not_found("package not found"))?;
    if !stored.installable || stored.key_status != "active" {
        return Err(refuse(RefusalCode::InstallationPackageRevoked));
    }
    super::approve::ensure_key_active(tx, row.workspace_id, &stored).await?;
    if row.disabled_reason.as_deref() == Some("approver_lost") {
        let approver_ok = match active.reviewed_by {
            Some(approver) => super::live::approver_is_admin(tx, row.workspace_id, approver)
                .await
                .map_err(|_| ApiError::internal())?,
            None => false,
        };
        if !approver_ok {
            return Err(refuse(RefusalCode::InstallationApproverLost));
        }
    }
    tx.execute(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "UPDATE installations SET status = 'active', disabled_reason = NULL, updated_at = now() WHERE id = $1",
        vec![row.id.into()],
    ))
    .await?;
    Ok(())
}

/// `POST /api/v1/installations/{installation_id}/disable`
pub async fn disable_installation(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    Path(installation_id): Path<Uuid>,
) -> Result<impl IntoResponse, ApiError> {
    Ok(ApiResponse::success(
        change_status(&state, &claims, installation_id, Change::Disable).await?,
    ))
}

/// `POST /api/v1/installations/{installation_id}/enable`
pub async fn enable_installation(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    Path(installation_id): Path<Uuid>,
) -> Result<impl IntoResponse, ApiError> {
    Ok(ApiResponse::success(
        change_status(&state, &claims, installation_id, Change::Enable).await?,
    ))
}

/// `POST /api/v1/installations/{installation_id}/revoke`: terminal. The next host call or
/// invocation of the installation is refused; its layouts and placements are deleted.
pub async fn revoke_installation(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    Path(installation_id): Path<Uuid>,
) -> Result<impl IntoResponse, ApiError> {
    Ok(ApiResponse::success(
        change_status(&state, &claims, installation_id, Change::Revoke).await?,
    ))
}

#[derive(Debug, Deserialize)]
pub struct InvocationsQuery {
    #[serde(default)]
    limit: Option<i64>,
}

#[derive(FromQueryResult)]
struct InvocationRow {
    id: Uuid,
    generation: i32,
    world: String,
    entry: String,
    principal_kind: String,
    principal_id: Option<Uuid>,
    input_digest: String,
    input_size: i32,
    output_digest: Option<String>,
    output_size: Option<i32>,
    outcome: String,
    error_code: Option<String>,
    duration_ms: i32,
    fuel_used: Option<i64>,
    host_calls: i32,
    log_lines: i32,
    created_at: platform::timestamp::Timestamp,
}

/// `GET /api/v1/installations/{installation_id}/invocations`: the redacted log, newest first.
pub async fn list_invocations(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    Path(installation_id): Path<Uuid>,
    Query(query): Query<InvocationsQuery>,
) -> Result<impl IntoResponse, ApiError> {
    let (row, _) = access::installation_for_admin(&state, &claims, installation_id).await?;
    let limit = query.limit.unwrap_or(50).clamp(1, 200);
    let rows = InvocationRow::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "SELECT id, generation, world, entry, principal_kind, principal_id, input_digest, input_size, \
                output_digest, output_size, outcome, error_code, duration_ms, fuel_used, host_calls, log_lines, \
                created_at \
         FROM extension_invocations WHERE installation_id = $1 ORDER BY created_at DESC, id LIMIT $2",
        vec![row.id.into(), limit.into()],
    ))
    .all(&state.db)
    .await?;
    let items: Vec<Value> = rows
        .into_iter()
        .map(|row| {
            json!({
                "id": row.id, "generation": row.generation, "world": row.world, "entry": row.entry,
                "principal_kind": row.principal_kind, "principal_id": row.principal_id,
                "input_digest": row.input_digest, "input_size": row.input_size,
                "output_digest": row.output_digest, "output_size": row.output_size,
                "outcome": row.outcome, "error_code": row.error_code, "duration_ms": row.duration_ms,
                "fuel_used": row.fuel_used, "host_calls": row.host_calls, "log_lines": row.log_lines,
                "created_at": row.created_at,
            })
        })
        .collect();
    Ok(ApiResponse::success(json!({ "items": items })))
}
