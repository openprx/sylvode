pub mod extension;
pub mod library;
pub mod preferences;

use std::collections::HashMap;

use axum::{
    Extension, Json,
    extract::{Path, State},
    response::IntoResponse,
};
use base64::{Engine as _, engine::general_purpose::STANDARD};
use platform::timestamp::Timestamp;
use platform::{app::AppState, auth::JwtClaims};
use sea_orm::{ConnectionTrait, DatabaseTransaction, DbBackend, FromQueryResult, Statement, TransactionTrait};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tokio::sync::Semaphore;
use uuid::Uuid;

use crate::{
    error::{ApiError, ApiErrorKind},
    forms::{
        permissions::{
            ensure_field_read_policy_allows, ensure_field_write_policy_allows, ensure_record_scope_allows_created_by,
            form_action_allowed, role_is_form_admin,
        },
        schema::parse_fields,
    },
    middleware::bot_auth::require_workspace_access_from_auth,
    plugins::{
        manifest::PluginRuntimePolicy,
        package::{
            APPLY, Contribution, EnabledContribution, Manifest, PREVIEW, Placement, WASM_BYTES, WEB_BYTES, bad,
            decode_asset, default_placements, enabled_contributions, normalize_placements, parse_guest_amount,
            read_decimal, read_order_inputs, sha256, validate_key,
        },
        package_v3::{
            AnyManifest, ManifestV3, Profile, SCHEMA_V3, Slot, UI_DISPLAY, merge_v3_layout, normalize_v3_placements,
            parse_manifest, plugin_error, validate_v3_grants,
        },
        runtime::{PluginFailureKind, PluginRuntimeOutput, invoke_wasm_plugin},
    },
    response::ApiResponse,
    routes::form::{self, FormResponse, RecordResponse, UpdateRecordRequest},
};

static EXECUTIONS: Semaphore = Semaphore::const_new(4);

fn statement(sql: &str, values: Vec<sea_orm::Value>) -> Statement {
    Statement::from_sql_and_values(DbBackend::Postgres, sql, values)
}

fn enabled() -> Result<(), ApiError> {
    if std::env::var("SV_FORM_PLUGIN_PACKAGES_ENABLED").as_deref() == Ok("1") {
        Ok(())
    } else {
        Err(ApiError::typed(
            ApiErrorKind::FeatureDisabled,
            "Form plugin packages are disabled",
        ))
    }
}

/// Protocol 3, the `ui` profile and every extension endpoint sit behind this switch.
pub(crate) fn extensions_enabled() -> bool {
    std::env::var("SV_FORM_PLUGIN_EXTENSIONS_ENABLED").as_deref() == Ok("1")
}

pub(crate) fn require_extensions() -> Result<(), ApiError> {
    if extensions_enabled() {
        Ok(())
    } else {
        Err(plugin_error(
            ApiErrorKind::FeatureDisabled,
            "extensions_disabled",
            "Form plugin extensions are disabled",
        ))
    }
}

pub(crate) fn contribution_unavailable() -> ApiError {
    plugin_error(
        ApiErrorKind::NotFound,
        "contribution_unavailable",
        "plugin contribution is not available",
    )
}

pub(crate) fn is_v3(manifest: &Value) -> bool {
    manifest.get("schema_version").and_then(Value::as_str) == Some(SCHEMA_V3)
}

/// Stored v3 manifests passed validation on the way in; a decoding failure is corruption.
pub(crate) fn stored_manifest_v3(manifest: &Value) -> Result<ManifestV3, ApiError> {
    ManifestV3::deserialize(manifest).map_err(|_| ApiError::Internal)
}

#[derive(FromQueryResult)]
struct LayoutRow {
    key: String,
    contribution_id: String,
    enabled: bool,
    position: i16,
}

/// Project layout rows of protocol 3 contributions, grouped by plugin key.
pub(crate) async fn stored_layouts<C: ConnectionTrait>(
    db: &C,
    project_id: Uuid,
    key: Option<&str>,
) -> Result<HashMap<String, Vec<Placement>>, ApiError> {
    let rows = LayoutRow::find_by_statement(statement(
        "SELECT key,contribution_id,enabled,position FROM form_plugin_contribution_layouts
          WHERE project_id=$1 AND ($2::text IS NULL OR key=$2) ORDER BY key,contribution_id",
        vec![project_id.into(), key.map(str::to_owned).into()],
    ))
    .all(db)
    .await?;
    let mut layouts: HashMap<String, Vec<Placement>> = HashMap::new();
    for row in rows {
        layouts.entry(row.key).or_default().push(Placement {
            id: row.contribution_id,
            enabled: row.enabled,
            position: u8::try_from(row.position).map_err(|_| ApiError::Internal)?,
        });
    }
    Ok(layouts)
}

pub(crate) fn v3_layout(manifest: &ManifestV3, stored: &HashMap<String, Vec<Placement>>) -> Vec<Placement> {
    merge_v3_layout(manifest, stored.get(&manifest.key).map_or(&[], Vec::as_slice))
}

/// Placements as published for one package: protocol 3 from the project layout table, protocol 2
/// from the per-package layout row, protocol 1 none.
fn placements_json(
    manifest: Option<&Value>,
    stored_v2: Option<Value>,
    stored_v3: &HashMap<String, Vec<Placement>>,
) -> Result<Option<Value>, ApiError> {
    match manifest {
        Some(manifest) if is_v3(manifest) => {
            let placements = v3_layout(&stored_manifest_v3(manifest)?, stored_v3);
            serde_json::to_value(placements)
                .map(Some)
                .map_err(|_| ApiError::Internal)
        }
        Some(manifest) => layout_json(manifest.pointer("/ui/contributions"), stored_v2),
        None => Ok(None),
    }
}

fn layouts_enabled() -> bool {
    std::env::var("SV_FORM_PLUGIN_LAYOUTS_ENABLED").as_deref() == Ok("1")
}

fn require_layouts_flag() -> Result<(), ApiError> {
    if layouts_enabled() {
        Ok(())
    } else {
        Err(ApiError::typed(
            ApiErrorKind::FeatureDisabled,
            "project plugin layouts are disabled",
        ))
    }
}

fn layouts_for(declarations: &Value, stored: Option<Value>) -> Result<Vec<Placement>, ApiError> {
    let contributions: Vec<Contribution> =
        serde_json::from_value(declarations.clone()).map_err(|_| ApiError::Internal)?;
    stored.map_or_else(
        || Ok(default_placements(&contributions)),
        |stored| serde_json::from_value(stored).map_err(|_| ApiError::Internal),
    )
}

fn layout_json(declarations: Option<&Value>, stored: Option<Value>) -> Result<Option<Value>, ApiError> {
    declarations
        .map(|declarations| {
            let placements = layouts_for(declarations, stored)?;
            serde_json::to_value(placements).map_err(|_| ApiError::Internal)
        })
        .transpose()
}

async fn admin(state: &AppState, claims: &JwtClaims, project_id: Uuid) -> Result<(Uuid, Uuid), ApiError> {
    enabled()?;
    let project = state
        .db
        .query_one(statement(
            "SELECT workspace_id FROM projects WHERE id=$1",
            vec![project_id.into()],
        ))
        .await?
        .ok_or_else(|| ApiError::not_found("project not found"))?;
    let workspace_id = project.try_get("", "workspace_id")?;
    let (actor_id, role, _) = require_workspace_access_from_auth(state, claims, None, workspace_id).await?;
    if !role_is_form_admin(&role) {
        return Err(ApiError::forbidden(
            "only workspace owners and admins can manage project plugin packages",
        ));
    }
    Ok((workspace_id, actor_id))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InstallRequest {
    manifest: Value,
    #[serde(default)]
    wasm_base64: Option<String>,
    web_base64: String,
}

/// A package that passed every install check, ready to be stored.
struct ValidatedPackage {
    key: String,
    version: String,
    manifest: Value,
    package_sha256: String,
    wasm: Vec<u8>,
    html: String,
}

#[derive(FromQueryResult, Serialize)]
struct Package {
    id: Uuid,
    manifest: Value,
    // The database result and the public JSON use the same column name.
    #[allow(clippy::struct_field_names)]
    package_sha256: String,
    installed_by: Uuid,
    installed_at: Timestamp,
    #[serde(skip_serializing_if = "Option::is_none")]
    library_source_id: Option<Uuid>,
    #[serde(skip_serializing_if = "Option::is_none")]
    placements: Option<Value>,
}

impl Package {
    fn with_layouts(mut self, stored_v3: &HashMap<String, Vec<Placement>>) -> Result<Self, ApiError> {
        self.placements = placements_json(Some(&self.manifest), self.placements.take(), stored_v3)?;
        Ok(self)
    }

    /// Resolves placements for a single package, reading the v3 layout rows when needed.
    async fn with_layout<C: ConnectionTrait>(self, db: &C, project_id: Uuid) -> Result<Self, ApiError> {
        let stored_v3 = if is_v3(&self.manifest) {
            let key = self
                .manifest
                .get("key")
                .and_then(Value::as_str)
                .ok_or(ApiError::Internal)?;
            stored_layouts(db, project_id, Some(key)).await?
        } else {
            HashMap::new()
        };
        self.with_layouts(&stored_v3)
    }
}

#[derive(FromQueryResult, Serialize, Deserialize)]
struct Binding {
    key: String,
    package_id: Option<Uuid>,
    generation: i64,
    grants: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    placements: Option<Value>,
    #[serde(skip)]
    declarations: Option<Value>,
    /// The bound manifest when it uses protocol 3.
    #[serde(skip)]
    manifest_v3: Option<Value>,
}

const BINDING_FIELDS: &str = "b.key,b.package_id,b.generation,b.grants,l.placements,
    p.manifest->'ui'->'contributions' AS declarations,
    CASE WHEN p.manifest->>'schema_version'='openpr.plugin.v3' THEN p.manifest END AS manifest_v3";

impl Binding {
    fn with_layouts(mut self, stored_v3: &HashMap<String, Vec<Placement>>) -> Result<Self, ApiError> {
        self.placements = if let Some(manifest) = &self.manifest_v3 {
            placements_json(Some(manifest), None, stored_v3)?
        } else {
            layout_json(self.declarations.as_ref(), self.placements.take())?
        };
        Ok(self)
    }
}

#[derive(FromQueryResult, Serialize)]
struct BindingActivity {
    key: String,
    request_id: Uuid,
    actor_id: Uuid,
    created_at: Timestamp,
    reason: Option<String>,
    previous_package_id: Option<Uuid>,
    previous_version: Option<String>,
    previous_grants: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    previous_placements: Option<Value>,
    package_id: Option<Uuid>,
    version: Option<String>,
    generation: i64,
    grants: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    placements: Option<Value>,
}

#[derive(Serialize)]
struct Catalog {
    items: Vec<Package>,
    bindings: Vec<Binding>,
    binding_history: Vec<BindingActivity>,
    layout_supported: bool,
    extension_supported: bool,
}

pub async fn list(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    Path(project_id): Path<Uuid>,
) -> Result<impl IntoResponse, ApiError> {
    admin(&state, &claims, project_id).await?;
    let stored_v3 = stored_layouts(&state.db, project_id, None).await?;
    let items = Package::find_by_statement(statement(
        "SELECT p.id,p.manifest,p.package_sha256,p.installed_by,p.installed_at,p.library_source_id,l.placements
           FROM form_plugin_packages p LEFT JOIN form_plugin_layouts l ON l.package_id=p.id
          WHERE p.project_id=$1 AND p.library_revoked_at IS NULL ORDER BY p.key,p.installed_at DESC",
        vec![project_id.into()],
    ))
    .all(&state.db)
    .await?
    .into_iter()
    .map(|package| package.with_layouts(&stored_v3))
    .collect::<Result<Vec<_>, _>>()?;
    let bindings = Binding::find_by_statement(statement(
        &format!(
            "SELECT {BINDING_FIELDS}
           FROM form_plugin_bindings b
           LEFT JOIN form_plugin_packages p ON p.id=b.package_id
           LEFT JOIN form_plugin_layouts l ON l.package_id=b.package_id
          WHERE b.project_id=$1 ORDER BY b.key"
        ),
        vec![project_id.into()],
    ))
    .all(&state.db)
    .await?
    .into_iter()
    .map(|binding| binding.with_layouts(&stored_v3))
    .collect::<Result<Vec<_>, _>>()?;
    let binding_history = BindingActivity::find_by_statement(statement(
        "WITH activity AS (
            SELECT c.*,
                   lag(c.response) OVER (PARTITION BY c.key ORDER BY (c.response->>'generation')::bigint) AS previous
              FROM form_plugin_binding_changes c WHERE c.project_id=$1
        )
         SELECT a.key,a.request_id,a.actor_id,a.created_at,a.request->>'reason' AS reason,
                older.id AS previous_package_id,older.version AS previous_version,
                ARRAY(SELECT jsonb_array_elements_text(COALESCE(a.previous->'grants','[]'::jsonb))) AS previous_grants,
                a.previous->'placements' AS previous_placements,
                newer.id AS package_id,newer.version,a.response->'placements' AS placements,
                (a.response->>'generation')::bigint AS generation,
                ARRAY(SELECT jsonb_array_elements_text(a.response->'grants')) AS grants
           FROM activity a
           LEFT JOIN form_plugin_packages older ON older.id=(a.previous->>'package_id')::uuid
           LEFT JOIN form_plugin_packages newer ON newer.id=(a.response->>'package_id')::uuid
          ORDER BY a.created_at DESC,a.request_id
          LIMIT 100",
        vec![project_id.into()],
    ))
    .all(&state.db)
    .await?;
    Ok(ApiResponse::success(Catalog {
        items,
        bindings,
        binding_history,
        layout_supported: layouts_enabled(),
        extension_supported: extensions_enabled(),
    }))
}

pub async fn install(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    Path(project_id): Path<Uuid>,
    Json(request): Json<InstallRequest>,
) -> Result<impl IntoResponse, ApiError> {
    let (workspace_id, actor_id) = admin(&state, &claims, project_id).await?;
    let package = validate_install(request).await?;
    let tx = state.db.begin().await?;
    lock_admin(&tx, workspace_id, actor_id).await?;
    lock_project(&tx, project_id).await?;
    let previous = Package::find_by_statement(statement(
        "SELECT p.id,p.manifest,p.package_sha256,p.installed_by,p.installed_at,p.library_source_id,l.placements
           FROM form_plugin_packages p LEFT JOIN form_plugin_layouts l ON l.package_id=p.id
          WHERE p.project_id=$1 AND p.key=$2 AND p.version=$3",
        vec![
            project_id.into(),
            package.key.clone().into(),
            package.version.clone().into(),
        ],
    ))
    .one(&tx)
    .await?;
    if let Some(previous) = previous {
        if previous.library_source_id.is_some() || previous.package_sha256 != package.package_sha256 {
            return Err(ApiError::Conflict(
                "a different package already uses this key and version".into(),
            ));
        }
        let previous = previous.with_layout(&tx, project_id).await?;
        tx.commit().await?;
        return Ok(ApiResponse::success(previous));
    }
    let count = tx
        .query_one(statement(
            "SELECT count(*) AS count FROM form_plugin_packages WHERE project_id=$1",
            vec![project_id.into()],
        ))
        .await?
        .ok_or(ApiError::Internal)?
        .try_get::<i64>("", "count")?;
    if count >= 32 {
        return Err(bad("project package limit reached"));
    }
    let installed = Package::find_by_statement(statement(
        "INSERT INTO form_plugin_packages (project_id,key,version,manifest,package_sha256,wasm_bytes,html,installed_by)
         VALUES ($1,$2,$3,$4,$5,$6,$7,$8) RETURNING id,manifest,package_sha256,installed_by,installed_at,library_source_id,NULL::jsonb AS placements",
        vec![
            project_id.into(),
            package.key.into(),
            package.version.into(),
            package.manifest.into(),
            package.package_sha256.into(),
            package.wasm.into(),
            package.html.into(),
            actor_id.into(),
        ],
    ))
    .one(&tx)
    .await?
    .ok_or(ApiError::Internal)?;
    let installed = installed.with_layout(&tx, project_id).await?;
    tx.commit().await?;
    Ok(ApiResponse::success(installed))
}

/// Dispatches on `schema_version`: protocol 1/2 keep their checks and messages, protocol 3
/// follows the typed reasons of the extension contract.
async fn validate_install(request: InstallRequest) -> Result<ValidatedPackage, ApiError> {
    match parse_manifest(request.manifest)? {
        AnyManifest::V2(manifest) => validate_v2(manifest, request.wasm_base64, &request.web_base64).await,
        AnyManifest::V3(manifest) => validate_v3(manifest, request.wasm_base64, &request.web_base64).await,
    }
}

async fn validate_v2(
    manifest: Manifest,
    wasm_base64: Option<String>,
    web_base64: &str,
) -> Result<ValidatedPackage, ApiError> {
    manifest.validate()?;
    if manifest.ui.contributions.is_some() {
        require_layouts_flag()?;
    }
    let wasm_base64 = wasm_base64.ok_or_else(|| bad("invalid plugin asset path or size"))?;
    let wasm = decode_asset(&manifest.wasm, "plugin.wasm", &wasm_base64, WASM_BYTES)?;
    let html = String::from_utf8(decode_asset(&manifest.ui.entry, "panel.html", web_base64, WEB_BYTES)?)
        .map_err(|_| bad("plugin panel must be UTF-8 HTML"))?;
    if html.trim().is_empty() || html.contains('\0') {
        return Err(bad("plugin panel HTML is empty or invalid"));
    }
    match probe_wasm(&wasm).await {
        Ok(()) => {}
        Err(ProbeFailure::Busy) => return Err(busy()),
        Err(ProbeFailure::Invoke) => return Err(bad("WASM failed its ABI and capability installation check")),
        Err(ProbeFailure::Output(error)) => return Err(error),
    }
    let value = serde_json::to_value(&manifest).map_err(|_| ApiError::Internal)?;
    let package_sha256 = sha256(&serde_jcs::to_vec(&value).map_err(|_| ApiError::Internal)?);
    Ok(ValidatedPackage {
        key: manifest.key,
        version: manifest.version,
        manifest: value,
        package_sha256,
        wasm,
        html,
    })
}

async fn validate_v3(
    manifest: ManifestV3,
    wasm_base64: Option<String>,
    web_base64: &str,
) -> Result<ValidatedPackage, ApiError> {
    require_extensions()?;
    manifest.validate()?;
    let profile_mismatch = || {
        plugin_error(
            ApiErrorKind::InvalidUpdate,
            "profile_mismatch",
            "the package profile does not match the uploaded assets",
        )
    };
    let wasm = match (manifest.profile, wasm_base64, manifest.wasm.as_ref()) {
        (Profile::Ui, None, _) => Vec::new(),
        (Profile::Wasm, Some(encoded), Some(asset)) => {
            let wasm = decode_v3_asset(asset.size, &asset.sha256, &encoded, WASM_BYTES)?;
            match probe_wasm(&wasm).await {
                Ok(()) => wasm,
                Err(ProbeFailure::Busy) => return Err(busy()),
                Err(ProbeFailure::Invoke | ProbeFailure::Output(_)) => {
                    return Err(plugin_error(
                        ApiErrorKind::InvalidUpdate,
                        "wasm_probe_failed",
                        "WASM failed its ABI and capability installation check",
                    ));
                }
            }
        }
        _ => return Err(profile_mismatch()),
    };
    let html = String::from_utf8(decode_v3_asset(
        manifest.entry.size,
        &manifest.entry.sha256,
        web_base64,
        WEB_BYTES,
    )?)
    .map_err(|_| asset_invalid("plugin panel must be UTF-8 HTML"))?;
    if html.trim().is_empty() || html.contains('\0') {
        return Err(asset_invalid("plugin panel HTML is empty or invalid"));
    }
    let value = serde_json::to_value(&manifest).map_err(|_| ApiError::Internal)?;
    let package_sha256 = sha256(&serde_jcs::to_vec(&value).map_err(|_| ApiError::Internal)?);
    Ok(ValidatedPackage {
        key: manifest.key,
        version: manifest.version,
        manifest: value,
        package_sha256,
        wasm,
        html,
    })
}

fn asset_invalid(message: &str) -> ApiError {
    plugin_error(ApiErrorKind::InvalidUpdate, "asset_invalid", message)
}

/// Decodes a protocol 3 asset whose path and digest format the manifest validation already fixed.
fn decode_v3_asset(size: usize, digest: &str, encoded: &str, limit: usize) -> Result<Vec<u8>, ApiError> {
    if size == 0 || size > limit || encoded.len() > limit.div_ceil(3) * 4 {
        return Err(asset_invalid("invalid plugin asset path or size"));
    }
    let bytes = STANDARD
        .decode(encoded)
        .map_err(|_| asset_invalid("invalid plugin asset base64"))?;
    if bytes.len() != size || sha256(&bytes) != digest {
        return Err(plugin_error(
            ApiErrorKind::ChecksumMismatch,
            "asset_digest_mismatch",
            "plugin asset size or SHA-256 does not match the manifest",
        ));
    }
    Ok(bytes)
}

enum ProbeFailure {
    Busy,
    Invoke,
    Output(ApiError),
}

/// Runs both order amount capabilities once against a fixed input, as every WASM install does.
async fn probe_wasm(wasm: &[u8]) -> Result<(), ProbeFailure> {
    let _capacity = EXECUTIONS.try_acquire().map_err(|_| ProbeFailure::Busy)?;
    for capability in [PREVIEW, APPLY] {
        let probe = invoke_wasm_plugin(wasm.to_vec(), guest_input(capability, 2, 1250), runtime_policy())
            .await
            .map_err(|_| ProbeFailure::Invoke)?;
        parse_guest_amount(&probe.output).map_err(ProbeFailure::Output)?;
    }
    Ok(())
}

async fn lock_project(tx: &DatabaseTransaction, project_id: Uuid) -> Result<(), ApiError> {
    tx.query_one(statement(
        "SELECT id FROM projects WHERE id=$1 FOR UPDATE",
        vec![project_id.into()],
    ))
    .await?
    .ok_or_else(|| ApiError::not_found("project not found"))?;
    Ok(())
}

async fn lock_admin(tx: &DatabaseTransaction, workspace_id: Uuid, actor_id: Uuid) -> Result<(), ApiError> {
    let row = tx
        .query_one(statement(
            "SELECT role FROM workspace_members WHERE workspace_id=$1 AND user_id=$2 FOR SHARE",
            vec![workspace_id.into(), actor_id.into()],
        ))
        .await?;
    match row {
        Some(row) if role_is_form_admin(&row.try_get::<String>("", "role")?) => Ok(()),
        _ => Err(ApiError::forbidden("project plugin management permission changed")),
    }
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BindRequest {
    package_id: Option<Uuid>,
    expected_generation: i64,
    request_id: Uuid,
    grants: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    placements: Option<Vec<Placement>>,
}

pub async fn bind(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    Path((project_id, key)): Path<(Uuid, String)>,
    Json(mut request): Json<BindRequest>,
) -> Result<impl IntoResponse, ApiError> {
    let (workspace_id, actor_id) = admin(&state, &claims, project_id).await?;
    validate_key(&key)?;
    if request.request_id.is_nil() {
        return Err(bad("invalid binding grants or request ID"));
    }
    let tx = state.db.begin().await?;
    lock_admin(&tx, workspace_id, actor_id).await?;
    lock_project(&tx, project_id).await?;
    // Only a key with a real installation can acquire a binding, including disable requests.
    let selected = tx
        .query_one(statement(
            "SELECT manifest FROM form_plugin_packages
          WHERE project_id=$1 AND key=$2 AND ($3::uuid IS NULL OR (id=$3 AND library_revoked_at IS NULL)) LIMIT 1",
            vec![project_id.into(), key.clone().into(), request.package_id.into()],
        ))
        .await?
        .ok_or_else(|| ApiError::not_found("plugin package not installed for this project and key"))?;
    // Every binding writer holds the project lock, so the current binding cannot move from here on.
    let target_v3 = if request.package_id.is_some() {
        let manifest: Value = selected.try_get("", "manifest")?;
        is_v3(&manifest).then_some(manifest)
    } else {
        None
    };
    let v3 = if let Some(manifest) = &target_v3 {
        require_extensions()?;
        let manifest = stored_manifest_v3(manifest)?;
        validate_v3_grants(Some(&manifest), &mut request.grants)?;
        if let Some(placements) = &mut request.placements {
            normalize_v3_placements(&manifest, placements)?;
        }
        true
    } else if request.package_id.is_none() && current_binding_is_v3(&tx, project_id, &key).await? {
        validate_v3_grants(None, &mut request.grants)?;
        if request.placements.is_some() {
            return Err(plugin_error(
                ApiErrorKind::InvalidUpdate,
                "placements_invalid",
                "a disable request cannot carry placements",
            ));
        }
        false
    } else {
        validate_legacy_binding(&mut request, &selected)?;
        false
    };
    tx.execute(statement(
        "INSERT INTO form_plugin_bindings (project_id,key) VALUES ($1,$2) ON CONFLICT DO NOTHING",
        vec![project_id.into(), key.clone().into()],
    ))
    .await?;
    let current = lock_binding(&tx, project_id, &key).await?;
    let payload = serde_json::to_value(&request).map_err(|_| ApiError::Internal)?;
    if let Some(response) = binding_receipt(&tx, project_id, &key, request.request_id, actor_id, &payload).await? {
        tx.commit().await?;
        return Ok(ApiResponse::success(response));
    }
    if current.generation != request.expected_generation {
        return Err(stale_binding());
    }
    match (request.package_id, &request.placements) {
        (Some(_), Some(placements)) if v3 => {
            // Rows of contributions the target does not declare stay for a later rollback.
            for placement in placements {
                tx.execute(statement(
                    "INSERT INTO form_plugin_contribution_layouts
                        (project_id,key,contribution_id,enabled,position,updated_by)
                     VALUES ($1,$2,$3,$4,$5,$6)
                     ON CONFLICT (project_id,key,contribution_id) DO UPDATE SET enabled=EXCLUDED.enabled,
                        position=EXCLUDED.position,updated_by=EXCLUDED.updated_by,updated_at=clock_timestamp()",
                    vec![
                        project_id.into(),
                        key.clone().into(),
                        placement.id.clone().into(),
                        placement.enabled.into(),
                        i16::from(placement.position).into(),
                        actor_id.into(),
                    ],
                ))
                .await?;
            }
        }
        (Some(package_id), Some(placements)) => {
            let serialized = serde_json::to_value(placements).map_err(|_| ApiError::Internal)?;
            tx.execute(statement(
                "INSERT INTO form_plugin_layouts (package_id,placements) VALUES ($1,$2)
                ON CONFLICT (package_id) DO UPDATE SET placements=EXCLUDED.placements,updated_at=clock_timestamp()",
                vec![package_id.into(), serialized.into()],
            ))
            .await?;
        }
        _ => {}
    }
    tx.execute(statement(
        "UPDATE form_plugin_bindings SET package_id=$3, generation=generation+1, grants=$4
          WHERE project_id=$1 AND key=$2",
        vec![
            project_id.into(),
            key.clone().into(),
            request.package_id.into(),
            request.grants.into(),
        ],
    ))
    .await?;
    let binding = lock_binding(&tx, project_id, &key).await?;
    let response = serde_json::to_value(binding).map_err(|_| ApiError::Internal)?;
    insert_binding_receipt(&tx, project_id, &key, request.request_id, actor_id, payload, &response).await?;
    tx.commit().await?;
    Ok(ApiResponse::success(response))
}

/// Protocol 1/2 grant and layout rules with their original messages.
fn validate_legacy_binding(request: &mut BindRequest, selected: &sea_orm::QueryResult) -> Result<(), ApiError> {
    request.grants.sort_unstable();
    let valid_grants = match request.grants.as_slice() {
        [] => true,
        [preview] => preview == PREVIEW,
        [apply, preview] => apply == APPLY && preview == PREVIEW,
        _ => false,
    };
    if !valid_grants || (request.package_id.is_none() && !request.grants.is_empty()) {
        return Err(bad("invalid binding grants or request ID"));
    }
    let declarations = if request.package_id.is_some() {
        selected
            .try_get::<Value>("", "manifest")?
            .pointer("/ui/contributions")
            .cloned()
    } else {
        None
    };
    if declarations.is_some() {
        require_layouts_flag()?;
    }
    if let Some(placements) = &mut request.placements {
        let declared = declarations.ok_or_else(|| bad("this version has no configurable layout"))?;
        let declared: Vec<Contribution> = serde_json::from_value(declared).map_err(|_| ApiError::Internal)?;
        normalize_placements(&declared, placements)?;
    }
    Ok(())
}

async fn current_binding_is_v3(tx: &DatabaseTransaction, project_id: Uuid, key: &str) -> Result<bool, ApiError> {
    Ok(tx
        .query_one(statement(
            "SELECT 1 FROM form_plugin_bindings b JOIN form_plugin_packages p ON p.id=b.package_id
              WHERE b.project_id=$1 AND b.key=$2 AND p.manifest->>'schema_version'=$3",
            vec![project_id.into(), key.into(), SCHEMA_V3.into()],
        ))
        .await?
        .is_some())
}

/// Returns the stored response when this request ID already changed the binding with the same
/// request body; a different body under the same ID is a conflict.
async fn binding_receipt(
    tx: &DatabaseTransaction,
    project_id: Uuid,
    key: &str,
    request_id: Uuid,
    actor_id: Uuid,
    payload: &Value,
) -> Result<Option<Value>, ApiError> {
    let Some(receipt) = tx
        .query_one(statement(
            "SELECT actor_id,request,response FROM form_plugin_binding_changes
              WHERE project_id=$1 AND key=$2 AND request_id=$3",
            vec![project_id.into(), key.into(), request_id.into()],
        ))
        .await?
    else {
        return Ok(None);
    };
    if receipt.try_get::<Uuid>("", "actor_id")? != actor_id || &receipt.try_get::<Value>("", "request")? != payload {
        return Err(ApiError::Conflict("binding request ID has already been used".into()));
    }
    Ok(Some(receipt.try_get::<Value>("", "response")?))
}

async fn insert_binding_receipt(
    tx: &DatabaseTransaction,
    project_id: Uuid,
    key: &str,
    request_id: Uuid,
    actor_id: Uuid,
    payload: Value,
    response: &Value,
) -> Result<(), ApiError> {
    tx.execute(statement(
        "INSERT INTO form_plugin_binding_changes (project_id,key,request_id,actor_id,request,response)
         VALUES ($1,$2,$3,$4,$5,$6)",
        vec![
            project_id.into(),
            key.into(),
            request_id.into(),
            actor_id.into(),
            payload.into(),
            response.clone().into(),
        ],
    ))
    .await?;
    Ok(())
}

async fn lock_binding(tx: &DatabaseTransaction, project_id: Uuid, key: &str) -> Result<Binding, ApiError> {
    let binding = Binding::find_by_statement(statement(
        &format!(
            "SELECT {BINDING_FIELDS}
           FROM form_plugin_bindings b
           LEFT JOIN form_plugin_packages p ON p.id=b.package_id
           LEFT JOIN form_plugin_layouts l ON l.package_id=b.package_id
          WHERE b.project_id=$1 AND b.key=$2 FOR UPDATE OF b"
        ),
        vec![project_id.into(), key.into()],
    ))
    .one(tx)
    .await?
    .ok_or_else(stale_binding)?;
    let stored_v3 = if binding.manifest_v3.is_some() {
        stored_layouts(tx, project_id, Some(key)).await?
    } else {
        HashMap::new()
    };
    binding.with_layouts(&stored_v3)
}

const fn stale_binding() -> ApiError {
    ApiError::VersionConflict("form_plugin_binding_changed")
}

fn output_patch(amount_cents: u64) -> Value {
    // Form numeric fields use decimal strings at the API boundary.
    json!({"amount": format!("{}.{:02}", amount_cents / 100, amount_cents % 100)})
}

struct RecordContext {
    form: FormResponse,
    record: RecordResponse,
    actor_id: Uuid,
    role: String,
}

async fn record_context(state: &AppState, claims: &JwtClaims, record_id: Uuid) -> Result<RecordContext, ApiError> {
    enabled()?;
    let record = form::find_record(state, record_id).await?;
    let form = form::find_form(state, record.form_id).await?;
    if form.archived_at.is_some()
        || record.archived_at.is_some()
        || form.project_id != record.project_id
        || form.workspace_id != record.workspace_id
    {
        return Err(ApiError::not_found("active Form record not found"));
    }
    let (actor_id, role, _) = form::require_form_action(state, claims, None, &form, "form.view").await?;
    ensure_record_scope_allows_created_by(state, form.id, &role, actor_id, record.created_by).await?;
    Ok(RecordContext {
        form,
        record,
        actor_id,
        role,
    })
}

fn supports_form(form: &FormResponse) -> Result<bool, ApiError> {
    let fields = parse_fields(&form.schema).map_err(ApiError::BadRequest)?;
    Ok(["quantity", "unit_price", "amount"].into_iter().all(|key| {
        fields
            .iter()
            .any(|field| field.key == key && field.field_type == "number")
    }))
}

async fn read_allowed(state: &AppState, context: &RecordContext) -> Result<(), ApiError> {
    for field in ["quantity", "unit_price"] {
        ensure_field_read_policy_allows(state, context.form.id, &context.role, field).await?;
    }
    Ok(())
}

async fn write_allowed(state: &AppState, context: &RecordContext) -> Result<(), ApiError> {
    if !form_action_allowed(state, context.form.id, &context.role, "record.update").await? {
        return Err(ApiError::forbidden("form permission denied: record.update"));
    }
    ensure_field_write_policy_allows(state, context.form.id, &context.role, &output_patch(0)).await
}

#[derive(FromQueryResult, Serialize)]
struct Panel {
    package_id: Uuid,
    key: String,
    version: String,
    name: String,
    generation: i64,
    grants: Vec<String>,
    html: String,
    web_sha256: String,
    package_sha256: String,
    #[serde(skip)]
    declarations: Option<Value>,
    #[serde(skip)]
    placements: Option<Value>,
    #[sea_orm(skip)]
    #[serde(skip_serializing_if = "Option::is_none")]
    contributions: Option<Vec<EnabledContribution>>,
}

pub async fn discover(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    Path(record_id): Path<Uuid>,
) -> Result<impl IntoResponse, ApiError> {
    let context = record_context(&state, &claims, record_id).await?;
    if !supports_form(&context.form)? {
        return Ok(ApiResponse::success(Vec::<Panel>::new()));
    }
    match read_allowed(&state, &context).await {
        Ok(()) => {}
        Err(error) if error.kind() == ApiErrorKind::Forbidden => return Ok(ApiResponse::success(Vec::new())),
        Err(err) => return Err(err),
    }
    let can_apply = match write_allowed(&state, &context).await {
        Ok(()) => true,
        Err(error) if error.kind() == ApiErrorKind::Forbidden => false,
        Err(error) => return Err(error),
    };
    let panels = Panel::find_by_statement(statement(
        "SELECT p.id AS package_id,p.key,p.version,p.manifest->>'name' AS name,
                b.generation,b.grants,p.html,p.manifest->'ui'->'entry'->>'sha256' AS web_sha256,p.package_sha256,
                p.manifest->'ui'->'contributions' AS declarations,l.placements
           FROM form_plugin_bindings b JOIN form_plugin_packages p ON p.id=b.package_id
           LEFT JOIN form_plugin_layouts l ON l.package_id=p.id
          WHERE b.project_id=$1 AND p.library_revoked_at IS NULL AND p.manifest->'ui'->>'form_key'=$2 AND $3=ANY(b.grants)
            AND ($4::bool OR p.manifest->'ui'->'contributions' IS NULL)
          ORDER BY p.key",
        vec![
            context.form.project_id.into(),
            context.form.key.into(),
            PREVIEW.into(),
            layouts_enabled().into(),
        ],
    ))
    .all(&state.db)
    .await?;
    let mut visible = Vec::new();
    for mut panel in panels {
        if let Some(declared) = panel.declarations.take() {
            let placements = layouts_for(&declared, panel.placements.take())?;
            let declared = serde_json::from_value(declared).map_err(|_| ApiError::Internal)?;
            let enabled = enabled_contributions(declared, &placements);
            if enabled.is_empty() {
                continue;
            }
            panel.contributions = Some(enabled);
        }
        if !can_apply {
            panel.grants.retain(|capability| capability == PREVIEW);
        }
        visible.push(panel);
    }
    Ok(ApiResponse::success(visible))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InvokeRequest {
    generation: i64,
    request_id: Uuid,
    preview_request_id: Option<Uuid>,
    capability: String,
    input: Value,
}

#[derive(FromQueryResult)]
struct ActivePackage {
    key: String,
    version: String,
    wasm_bytes: Vec<u8>,
    wasm_sha256: String,
}

fn guest_input(capability: &str, quantity: u64, unit_price_cents: u64) -> Value {
    json!({"capability":capability,"quantity":quantity,"unit_price_cents":unit_price_cents})
}

const fn runtime_policy() -> PluginRuntimePolicy {
    PluginRuntimePolicy {
        timeout_ms: 250,
        fuel: 500_000,
        memory_bytes: 2 * 1024 * 1024,
    }
}

fn busy() -> ApiError {
    ApiError::typed(
        ApiErrorKind::LimitExceeded,
        "plugin execution capacity reached; retry shortly",
    )
}

#[derive(FromQueryResult, Serialize)]
struct Receipt {
    request_id: Uuid,
    actor_id: Uuid,
    record_id: Uuid,
    package_id: Uuid,
    key: String,
    version: String,
    generation: i64,
    capability: String,
    preview_request_id: Option<Uuid>,
    record_updated_at: Timestamp,
    schema_version: i32,
    status: String,
    amount_cents: Option<i64>,
    error_kind: Option<String>,
    duration_ms: Option<i64>,
    fuel_consumed: Option<i64>,
    created_at: Timestamp,
    contribution_id: Option<String>,
}

const RECEIPT_FIELDS: &str = "i.request_id,i.actor_id,i.record_id,i.package_id,p.key,p.version,
    i.generation,i.capability,i.preview_request_id,i.record_updated_at,i.schema_version,i.status,
    i.amount_cents,i.error_kind,i.duration_ms,i.fuel_consumed,i.created_at,i.contribution_id";

const RECEIPT_SOURCE: &str = "FROM form_plugin_invocations i JOIN form_plugin_packages p ON p.id=i.package_id";

pub async fn list_invocations(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    Path(project_id): Path<Uuid>,
) -> Result<impl IntoResponse, ApiError> {
    admin(&state, &claims, project_id).await?;
    let invocations = Receipt::find_by_statement(statement(
        &format!("SELECT {RECEIPT_FIELDS} {RECEIPT_SOURCE} WHERE i.project_id=$1 ORDER BY i.created_at DESC,i.request_id LIMIT 100"),
        vec![project_id.into()],
    )).all(&state.db).await?;
    Ok(ApiResponse::success(invocations))
}

pub(crate) struct Execution {
    workspace_id: Uuid,
    project_id: Uuid,
    key: String,
    package_id: Uuid,
    generation: i64,
    actor_id: Uuid,
    role: String,
    form_id: Uuid,
    record_id: Uuid,
    created_by: Option<Uuid>,
    record_updated_at: Timestamp,
    schema_version: i32,
    capability: String,
    request_id: Uuid,
    preview_request_id: Option<Uuid>,
    /// Set for protocol 3 invocations, which address one declared record contribution.
    contribution: Option<ContributionTarget>,
}

pub(crate) struct ContributionTarget {
    id: String,
    form_key: String,
}

impl Execution {
    async fn receipt<C: ConnectionTrait>(
        &self,
        db: &C,
        capability: &str,
        request_id: Uuid,
    ) -> Result<Option<Receipt>, ApiError> {
        Receipt::find_by_statement(statement(
            &format!(
                "SELECT {RECEIPT_FIELDS} {RECEIPT_SOURCE}
              WHERE i.project_id=$1 AND i.actor_id=$2 AND i.record_id=$3 AND i.package_id=$4 AND i.generation=$5
                AND i.capability=$6 AND i.request_id=$7"
            ),
            self.identity(capability, request_id),
        ))
        .one(db)
        .await
        .map_err(ApiError::from)
    }

    fn identity(&self, capability: &str, request_id: Uuid) -> Vec<sea_orm::Value> {
        vec![
            self.project_id.into(),
            self.actor_id.into(),
            self.record_id.into(),
            self.package_id.into(),
            self.generation.into(),
            capability.into(),
            request_id.into(),
        ]
    }

    async fn check_binding(&self, state: &AppState, tx: &DatabaseTransaction) -> Result<(), ApiError> {
        let binding = lock_binding(tx, self.project_id, &self.key).await?;
        if binding.package_id != Some(self.package_id) || binding.generation != self.generation {
            return Err(stale_binding());
        }
        if !binding.grants.contains(&self.capability) {
            return Err(ApiError::forbidden("plugin capability has not been granted"));
        }
        if let Some(target) = &self.contribution {
            require_extensions()?;
            if !binding.grants.iter().any(|grant| grant == UI_DISPLAY) {
                return Err(contribution_unavailable());
            }
            let manifest = stored_manifest_v3(binding.manifest_v3.as_ref().ok_or_else(contribution_unavailable)?)?;
            if !v3_record_target(&manifest, &target.id, &target.form_key) {
                return Err(contribution_unavailable());
            }
            let placements: Vec<Placement> = binding
                .placements
                .map(serde_json::from_value)
                .transpose()
                .map_err(|_| ApiError::Internal)?
                .unwrap_or_default();
            if !placements
                .iter()
                .any(|placement| placement.id == target.id && placement.enabled)
            {
                return Err(contribution_unavailable());
            }
        } else if let Some(placements) = binding.placements {
            require_layouts_flag()?;
            let placements: Vec<Placement> = serde_json::from_value(placements).map_err(|_| ApiError::Internal)?;
            if !placements.into_iter().any(|placement| placement.enabled) {
                return Err(ApiError::forbidden("plugin has no enabled contribution"));
            }
        }
        let member = tx
            .query_one(statement(
                "SELECT role FROM workspace_members WHERE workspace_id=$1 AND user_id=$2 FOR SHARE",
                vec![self.workspace_id.into(), self.actor_id.into()],
            ))
            .await?
            .ok_or_else(|| ApiError::forbidden("workspace permission changed"))?;
        if member.try_get::<String>("", "role")? != self.role {
            return Err(ApiError::forbidden("workspace permission changed"));
        }
        // Same lock used by Form permission changes. A revoked scope or field grant cannot
        // race the final check and commit, even when the relevant policy row was absent.
        tx.execute(statement(
            "SELECT pg_advisory_xact_lock(hashtextextended($1, 19019))",
            vec![self.form_id.to_string().into()],
        ))
        .await?;
        if !form_action_allowed(state, self.form_id, &self.role, "form.view").await? {
            return Err(ApiError::forbidden("form permission denied: form.view"));
        }
        ensure_record_scope_allows_created_by(state, self.form_id, &self.role, self.actor_id, self.created_by).await?;
        for field in ["quantity", "unit_price"] {
            ensure_field_read_policy_allows(state, self.form_id, &self.role, field).await?;
        }
        if self.capability == APPLY {
            if !form_action_allowed(state, self.form_id, &self.role, "record.update").await? {
                return Err(ApiError::forbidden("form permission denied: record.update"));
            }
            ensure_field_write_policy_allows(state, self.form_id, &self.role, &output_patch(0)).await?;
        }
        Ok(())
    }

    async fn replay<C: ConnectionTrait>(&self, db: &C) -> Result<Option<Receipt>, ApiError> {
        let receipt = self.receipt(db, &self.capability, self.request_id).await?;
        if receipt
            .as_ref()
            .is_some_and(|row| row.preview_request_id != self.preview_request_id)
        {
            return Err(ApiError::Conflict("request ID is bound to a different preview".into()));
        }
        if receipt
            .as_ref()
            .is_some_and(|row| row.contribution_id.as_deref() != self.contribution_id())
        {
            return Err(plugin_error(
                ApiErrorKind::InvalidUpdate,
                "contribution_mismatch",
                "request ID is bound to a different contribution",
            ));
        }
        Ok(receipt)
    }

    fn contribution_id(&self) -> Option<&str> {
        self.contribution.as_ref().map(|target| target.id.as_str())
    }

    async fn insert_receipt(
        &self,
        tx: &DatabaseTransaction,
        status: &str,
        amount_cents: Option<i64>,
        error_kind: Option<&str>,
        duration_ms: Option<i64>,
        fuel_consumed: Option<i64>,
    ) -> Result<(), ApiError> {
        let mut values = self.identity(&self.capability, self.request_id);
        values.extend([
            self.preview_request_id.into(),
            self.record_updated_at.into(),
            self.schema_version.into(),
            status.into(),
            amount_cents.into(),
            error_kind.into(),
            duration_ms.into(),
            fuel_consumed.into(),
            self.contribution_id().map(str::to_owned).into(),
        ]);
        tx.execute(statement(
            "INSERT INTO form_plugin_invocations
            (project_id,actor_id,record_id,package_id,generation,capability,request_id,preview_request_id,
             record_updated_at,schema_version,status,amount_cents,error_kind,duration_ms,fuel_consumed,
             contribution_id)
              VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13,$14,$15,$16)",
            values,
        ))
        .await?;
        Ok(())
    }

    async fn preview_amount(&self, state: &AppState) -> Result<Option<i64>, ApiError> {
        let Some(id) = self.preview_request_id else {
            return Ok(None);
        };
        let prior = self
            .receipt(&state.db, PREVIEW, id)
            .await?
            .ok_or_else(|| bad("a successful preview is required before applying"))?;
        if prior.record_updated_at != self.record_updated_at || prior.schema_version != self.schema_version {
            return Err(ApiError::VersionConflict("form_plugin_preview_stale"));
        }
        prior
            .amount_cents
            .map(Some)
            .ok_or_else(|| bad("a successful preview is required before applying"))
    }
}

pub(crate) struct ApplyCommit {
    execution: Execution,
    amount_cents: i64,
    runtime: PluginRuntimeOutput,
}

impl ApplyCommit {
    pub(crate) async fn begin(&self, state: &AppState, tx: &DatabaseTransaction) -> Result<bool, ApiError> {
        self.execution.check_binding(state, tx).await?;
        if let Some(row) = self.execution.replay(tx).await? {
            receipt_result(&row)?;
            return Ok(false);
        }
        Ok(true)
    }

    pub(crate) async fn finish(
        &self,
        tx: &DatabaseTransaction,
        previous_values: &Value,
        values: &Value,
    ) -> Result<(), ApiError> {
        let expected = output_patch(u64::try_from(self.amount_cents).map_err(|_| ApiError::Internal)?);
        if read_decimal(&expected, "amount")? != read_decimal(values, "amount")? {
            return Err(bad("Form validation or a calculation changed the confirmed amount"));
        }
        let previous_fields = previous_values.as_object().ok_or(ApiError::Internal)?;
        let next_fields = values.as_object().ok_or(ApiError::Internal)?;
        if previous_fields
            .iter()
            .any(|(key, value)| key != "amount" && next_fields.get(key) != Some(value))
            || next_fields
                .keys()
                .any(|key| key != "amount" && !previous_fields.contains_key(key))
        {
            return Err(bad("the order amount capability may only change the amount field"));
        }
        self.execution
            .insert_receipt(
                tx,
                "completed",
                Some(self.amount_cents),
                None,
                i64::try_from(self.runtime.duration_ms).ok(),
                self.runtime.fuel_consumed.and_then(|fuel| i64::try_from(fuel).ok()),
            )
            .await
    }
}

#[derive(Serialize)]
struct InvokeResult {
    amount_cents: i64,
    package_id: Uuid,
    version: String,
    generation: i64,
    request_id: Uuid,
}

fn receipt_result(receipt: &Receipt) -> Result<i64, ApiError> {
    receipt
        .amount_cents
        .ok_or_else(|| audited_error(receipt.error_kind.as_deref().unwrap_or("execution_failed")))
}

fn audited_error(kind: &str) -> ApiError {
    match kind {
        "timeout" => bad("plugin execution timed out"),
        "invalid_output" => bad("plugin result does not match the amount schema"),
        "invalid_record" => {
            bad("record requires a positive integer quantity and a unit price with at most two decimals")
        }
        "preview_mismatch" => ApiError::VersionConflict("form_plugin_preview_mismatch"),
        "form_rejected" => bad("Form rejected the confirmed amount"),
        _ => bad("plugin execution failed"),
    }
}

pub async fn invoke(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    Path((record_id, package_id)): Path<(Uuid, Uuid)>,
    Json(request): Json<InvokeRequest>,
) -> Result<impl IntoResponse, ApiError> {
    if request.input.as_object().is_none_or(|input| !input.is_empty())
        || request.request_id.is_nil()
        || !matches!(request.capability.as_str(), PREVIEW | APPLY)
        || (request.capability == APPLY) != request.preview_request_id.is_some()
    {
        return Err(bad(
            "invalid plugin capability, request ID, preview reference, or input",
        ));
    }
    let context = record_context(&state, &claims, record_id).await?;
    read_allowed(&state, &context).await?;
    if request.capability == APPLY {
        write_allowed(&state, &context).await?;
    }
    if !supports_form(&context.form)? {
        return Err(ApiError::not_found("plugin package does not support this Form"));
    }
    let active = ActivePackage::find_by_statement(statement(
        "SELECT p.key,p.version,p.wasm_bytes,p.manifest->'wasm'->>'sha256' AS wasm_sha256
           FROM form_plugin_packages p JOIN form_plugin_bindings b ON b.project_id=p.project_id AND b.key=p.key
          WHERE p.project_id=$1 AND p.id=$2 AND p.library_revoked_at IS NULL AND p.manifest->'ui'->>'form_key'=$3",
        vec![
            context.form.project_id.into(),
            package_id.into(),
            context.form.key.clone().into(),
        ],
    ))
    .one(&state.db)
    .await?
    .ok_or_else(|| ApiError::not_found("plugin package is not bound to this Form"))?;
    let execution = Execution {
        workspace_id: context.form.workspace_id,
        project_id: context.form.project_id,
        key: active.key.clone(),
        package_id,
        generation: request.generation,
        actor_id: context.actor_id,
        role: context.role.clone(),
        form_id: context.form.id,
        record_id,
        created_by: context.record.created_by,
        record_updated_at: context.record.updated_at,
        schema_version: context.form.schema_version,
        capability: request.capability,
        request_id: request.request_id,
        preview_request_id: request.preview_request_id,
        contribution: None,
    };
    Ok(ApiResponse::success(execute(&state, context, execution, active).await?))
}

/// Shared by the protocol 1/2 and protocol 3 invoke routes once the execution is identified.
async fn execute(
    state: &AppState,
    context: RecordContext,
    execution: Execution,
    active: ActivePackage,
) -> Result<InvokeResult, ApiError> {
    let package_id = execution.package_id;
    let generation = execution.generation;
    let request_id = execution.request_id;
    let record_id = execution.record_id;
    let version = active.version;
    let result = |amount_cents| InvokeResult {
        amount_cents,
        package_id,
        version: version.clone(),
        generation,
        request_id,
    };
    let _capacity = EXECUTIONS.try_acquire().map_err(|_| busy())?;
    let tx = state.db.begin().await?;
    execution.check_binding(state, &tx).await?;
    let replay = execution.replay(&tx).await?;
    tx.commit().await?;
    if let Some(receipt) = replay {
        return Ok(result(receipt_result(&receipt)?));
    }

    let expected_amount = execution.preview_amount(state).await?;
    let Ok((quantity, unit_price_cents)) = read_order_inputs(&context.record.values) else {
        return failure(state, &execution, "invalid_record", None, None)
            .await
            .map(result);
    };
    if sha256(&active.wasm_bytes) != active.wasm_sha256 {
        return Err(ApiError::Internal);
    }
    let input = guest_input(&execution.capability, quantity, unit_price_cents);
    let runtime = match invoke_wasm_plugin(active.wasm_bytes, input, runtime_policy()).await {
        Ok(runtime) => runtime,
        Err(err) => {
            let kind = if err.kind == PluginFailureKind::Timeout {
                "timeout"
            } else {
                "execution_failed"
            };
            return failure(
                state,
                &execution,
                kind,
                i64::try_from(err.duration_ms).ok(),
                err.fuel_consumed.and_then(|fuel| i64::try_from(fuel).ok()),
            )
            .await
            .map(result);
        }
    };
    let duration_ms = i64::try_from(runtime.duration_ms).ok();
    let fuel_consumed = runtime.fuel_consumed.and_then(|fuel| i64::try_from(fuel).ok());
    let Ok(amount) = parse_guest_amount(&runtime.output) else {
        return failure(state, &execution, "invalid_output", duration_ms, fuel_consumed)
            .await
            .map(result);
    };
    let amount_cents = i64::try_from(amount).map_err(|_| ApiError::Internal)?;
    if let Some(expected_amount) = expected_amount {
        if amount_cents != expected_amount {
            return failure(state, &execution, "preview_mismatch", duration_ms, fuel_consumed)
                .await
                .map(result);
        }
        let generated_title = form::render_title(&context.form.title_template, record_id, &context.record.values);
        let explicit_title = (context.record.title != generated_title).then(|| context.record.title.clone());
        let update = UpdateRecordRequest {
            values: Some(output_patch(amount)),
            title: explicit_title,
            source: Some(
                json!({"type":"form_plugin","actor_id":execution.actor_id,"package_id":package_id,
                "version":version,"generation":generation,"request_id":request_id}),
            ),
            idempotency_key: None,
            expected_updated_at: Some(context.record.updated_at),
            expected_schema_version: Some(context.form.schema_version),
        };
        let commit = ApplyCommit {
            execution,
            amount_cents,
            runtime,
        };
        match form::update_record_for_plugin(
            state,
            &context.form,
            context.record,
            commit.execution.actor_id,
            &commit.execution.role,
            update,
            &commit,
        )
        .await
        {
            Ok(_) => {}
            Err(ApiError::BadRequest(_)) => {
                // The Form transaction has rolled back. Record only the failure classification.
                return failure(state, &commit.execution, "form_rejected", duration_ms, fuel_consumed)
                    .await
                    .map(result);
            }
            Err(error) => return Err(error),
        }
    } else {
        let tx = state.db.begin().await?;
        execution.check_binding(state, &tx).await?;
        if let Some(receipt) = execution.replay(&tx).await? {
            let amount = receipt_result(&receipt)?;
            tx.commit().await?;
            return Ok(result(amount));
        }
        let current = tx
            .query_one(statement(
                "SELECT r.updated_at,f.schema_version FROM project_forms f
             JOIN form_records r ON r.form_id=f.id WHERE r.id=$1 AND r.archived_at IS NULL
               AND f.id=$2 AND f.archived_at IS NULL FOR SHARE OF f,r",
                vec![record_id.into(), execution.form_id.into()],
            ))
            .await?
            .ok_or_else(|| ApiError::not_found("active Form record not found"))?;
        if current.try_get::<Timestamp>("", "updated_at")? != execution.record_updated_at
            || current.try_get::<i32>("", "schema_version")? != execution.schema_version
        {
            return Err(ApiError::VersionConflict("form_plugin_preview_stale"));
        }
        execution
            .insert_receipt(&tx, "completed", Some(amount_cents), None, duration_ms, fuel_consumed)
            .await?;
        tx.commit().await?;
    }
    Ok(result(amount_cents))
}

/// A protocol 3 record contribution may drive the amount capabilities only when it targets the
/// WASM Form key and that key is the record's Form key.
pub(crate) fn v3_record_target(manifest: &ManifestV3, contribution_id: &str, form_key: &str) -> bool {
    let Some(wasm) = &manifest.wasm else {
        return false;
    };
    manifest.profile == Profile::Wasm
        && wasm.form_key == form_key
        && manifest.contribution(contribution_id).is_some_and(|contribution| {
            matches!(contribution.slot, Slot::FormRecordDetail | Slot::FormRecordActions)
                && contribution.form_key.as_deref() == Some(form_key)
        })
}

async fn failure(
    state: &AppState,
    execution: &Execution,
    kind: &str,
    duration_ms: Option<i64>,
    fuel_consumed: Option<i64>,
) -> Result<i64, ApiError> {
    let tx = state.db.begin().await?;
    execution.check_binding(state, &tx).await?;
    if let Some(receipt) = execution.replay(&tx).await? {
        let amount = receipt_result(&receipt)?;
        tx.commit().await?;
        return Ok(amount);
    }
    let status = if kind == "timeout" { "timeout" } else { "failed" };
    execution
        .insert_receipt(&tx, status, None, Some(kind), duration_ms, fuel_consumed)
        .await?;
    tx.commit().await?;
    Err(audited_error(kind))
}
