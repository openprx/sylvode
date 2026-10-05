use axum::{
    Extension, Json,
    extract::{Path, Query, State},
    response::IntoResponse,
};
use base64::{Engine as _, engine::general_purpose::STANDARD};
use platform::{app::AppState, auth::JwtClaims};
use sea_orm::{ConnectionTrait, DbBackend, FromQueryResult, Statement, TransactionTrait};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use uuid::Uuid;

use crate::{
    error::ApiError,
    events::{BusinessEventInput, insert_business_event},
    middleware::bot_auth::{BotAuthContext, require_workspace_access_from_auth},
    plugins::{
        manifest::parse_manifest,
        runtime::{
            PluginInvocationStatus, PluginRuntimeError, PluginRuntimeOutput, invoke_wasm_plugin, validate_wasm_module,
        },
    },
    response::{ApiResponse, PaginatedData},
};

#[derive(Debug, Deserialize)]
pub struct ListPluginsQuery {
    pub page: Option<i64>,
    pub per_page: Option<i64>,
}

#[derive(Debug, Deserialize)]
pub struct InstallPluginRequest {
    pub manifest: Value,
    pub wasm_base64: Option<String>,
    pub status: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct UpdatePluginRequest {
    pub status: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct InvokePluginRequest {
    pub hook_kind: String,
    #[serde(default)]
    pub input: Value,
}

#[derive(Debug, Serialize, FromQueryResult, Clone)]
pub struct PluginResponse {
    pub id: Uuid,
    pub workspace_id: Uuid,
    pub project_id: Uuid,
    pub key: String,
    pub name: String,
    pub version: String,
    pub description: String,
    pub manifest: Value,
    pub wasm_sha256: Option<String>,
    pub status: String,
    pub installed_by: Option<Uuid>,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub updated_at: chrono::DateTime<chrono::Utc>,
}

#[derive(Debug, FromQueryResult)]
struct PluginRuntimeRow {
    id: Uuid,
    workspace_id: Uuid,
    project_id: Uuid,
    key: String,
    manifest: Value,
    wasm_bytes: Option<Vec<u8>>,
    status: String,
}

#[derive(Debug, Serialize, FromQueryResult)]
pub struct PluginInvocationResponse {
    pub id: Uuid,
    pub workspace_id: Uuid,
    pub project_id: Option<Uuid>,
    pub plugin_id: Option<Uuid>,
    pub plugin_key: String,
    pub hook_kind: String,
    pub status: String,
    pub input: Value,
    pub output: Value,
    pub error_message: Option<String>,
    pub duration_ms: i64,
    pub fuel_consumed: Option<i64>,
    pub created_at: chrono::DateTime<chrono::Utc>,
}

#[derive(Debug, FromQueryResult)]
struct CountRow {
    count: i64,
}

#[derive(Debug, FromQueryResult)]
struct ProjectWorkspace {
    workspace_id: Uuid,
}

pub async fn list_project_plugins(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    bot: Option<Extension<BotAuthContext>>,
    Path(project_id): Path<Uuid>,
    Query(query): Query<ListPluginsQuery>,
) -> Result<impl IntoResponse, ApiError> {
    let workspace_id = ensure_project_access(&state, &claims, bot.as_ref().map(|b| &b.0), project_id).await?;
    let page = query.page.unwrap_or(1).max(1);
    let per_page = query.per_page.unwrap_or(50).clamp(1, 200);
    let offset = (page - 1) * per_page;

    let total = count_query(
        &state,
        "SELECT COUNT(*)::bigint AS count FROM plugins WHERE workspace_id = $1 AND project_id = $2",
        vec![workspace_id.into(), project_id.into()],
    )
    .await?;
    let items = PluginResponse::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        r"SELECT id, workspace_id, project_id, key, name, version, description, manifest,
                  wasm_sha256, status, installed_by, created_at, updated_at
             FROM plugins
            WHERE workspace_id = $1 AND project_id = $2
            ORDER BY key ASC, version DESC
            LIMIT $3 OFFSET $4",
        vec![workspace_id.into(), project_id.into(), per_page.into(), offset.into()],
    ))
    .all(&state.db)
    .await?;

    Ok(ApiResponse::success(PaginatedData {
        items,
        total,
        page,
        per_page,
        total_pages: total_pages(total, per_page),
    }))
}

pub async fn install_project_plugin(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    bot: Option<Extension<BotAuthContext>>,
    Path(project_id): Path<Uuid>,
    Json(req): Json<InstallPluginRequest>,
) -> Result<impl IntoResponse, ApiError> {
    let (workspace_id, actor_id, is_bot) =
        ensure_project_actor(&state, &claims, bot.as_ref().map(|b| &b.0), project_id).await?;
    let manifest = parse_manifest(&req.manifest).map_err(ApiError::BadRequest)?;
    let manifest_value = serde_json::to_value(&manifest).map_err(|_| ApiError::Internal)?;
    let (wasm_bytes, wasm_sha256) = decode_and_validate_wasm(req.wasm_base64.as_deref())?;
    let status = normalize_plugin_status(req.status.as_deref())?;

    let tx = state.db.begin().await?;
    let row = PluginResponse::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        r"INSERT INTO plugins (
                workspace_id, project_id, key, name, version, description, manifest,
                wasm_bytes, wasm_sha256, status, installed_by
            )
            VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11)
            RETURNING id, workspace_id, project_id, key, name, version, description, manifest,
                      wasm_sha256, status, installed_by, created_at, updated_at",
        vec![
            workspace_id.into(),
            project_id.into(),
            manifest.key.clone().into(),
            manifest.name.clone().into(),
            manifest.version.clone().into(),
            manifest.description.clone().into(),
            manifest_value.into(),
            wasm_bytes.into(),
            wasm_sha256.into(),
            status.into(),
            if is_bot { None::<Uuid> } else { Some(actor_id) }.into(),
        ],
    ))
    .one(&tx)
    .await
    .map_err(map_plugin_insert_error)?
    .ok_or(ApiError::Internal)?;
    insert_plugin_event(
        &tx,
        &row,
        "plugin.installed",
        if is_bot { None } else { Some(actor_id) },
        json!({ "type": if is_bot { "bot" } else { "user" }, "actor_id": actor_id }),
        json!({
            "plugin_id": row.id,
            "plugin_key": row.key,
            "version": row.version,
            "status": row.status,
            "wasm_sha256": row.wasm_sha256
        }),
    )
    .await?;
    tx.commit().await?;

    Ok(ApiResponse::success(row))
}

pub async fn get_plugin(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    bot: Option<Extension<BotAuthContext>>,
    Path(plugin_id): Path<Uuid>,
) -> Result<impl IntoResponse, ApiError> {
    let row = get_plugin_response(&state, plugin_id).await?;
    require_workspace_access_from_auth(&state, &claims, bot.as_ref().map(|b| &b.0), row.workspace_id).await?;
    Ok(ApiResponse::success(row))
}

pub async fn update_plugin(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    bot: Option<Extension<BotAuthContext>>,
    Path(plugin_id): Path<Uuid>,
    Json(req): Json<UpdatePluginRequest>,
) -> Result<impl IntoResponse, ApiError> {
    let existing = get_plugin_response(&state, plugin_id).await?;
    let (actor_id, _, is_bot) =
        require_workspace_access_from_auth(&state, &claims, bot.as_ref().map(|b| &b.0), existing.workspace_id).await?;
    let status = normalize_plugin_status(req.status.as_deref())?;

    let tx = state.db.begin().await?;
    let row = PluginResponse::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        r"UPDATE plugins
              SET status = $2, updated_at = now()
            WHERE id = $1
            RETURNING id, workspace_id, project_id, key, name, version, description, manifest,
                      wasm_sha256, status, installed_by, created_at, updated_at",
        vec![plugin_id.into(), status.into()],
    ))
    .one(&tx)
    .await?
    .ok_or_else(|| ApiError::NotFound("plugin not found".to_string()))?;
    insert_plugin_event(
        &tx,
        &row,
        "plugin.updated",
        if is_bot { None } else { Some(actor_id) },
        json!({ "type": if is_bot { "bot" } else { "user" }, "actor_id": actor_id }),
        json!({
            "plugin_id": row.id,
            "plugin_key": row.key,
            "previous_status": existing.status,
            "status": row.status
        }),
    )
    .await?;
    tx.commit().await?;

    Ok(ApiResponse::success(row))
}

pub async fn invoke_plugin(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    bot: Option<Extension<BotAuthContext>>,
    Path(plugin_id): Path<Uuid>,
    Json(req): Json<InvokePluginRequest>,
) -> Result<impl IntoResponse, ApiError> {
    let row = get_plugin_runtime_row(&state, plugin_id).await?;
    let (actor_id, _, is_bot) =
        require_workspace_access_from_auth(&state, &claims, bot.as_ref().map(|b| &b.0), row.workspace_id).await?;
    if row.status != "active" {
        return Err(ApiError::BadRequest("plugin is not active".to_string()));
    }
    let manifest = parse_manifest(&row.manifest).map_err(ApiError::BadRequest)?;
    let hook_kind = normalize_hook_kind(&req.hook_kind)?;
    if !manifest.capabilities.hooks.iter().any(|hook| hook.kind == hook_kind)
        && !manifest.capabilities.tools.iter().any(|tool| tool.name == hook_kind)
    {
        return Err(ApiError::BadRequest(
            "plugin manifest does not declare this hook/tool".to_string(),
        ));
    }
    let Some(wasm_bytes) = row.wasm_bytes.clone() else {
        let tx = state.db.begin().await?;
        // Nothing ran, so nothing was spent: zero time, no fuel figure.
        let invocation = insert_invocation(
            &tx,
            &row,
            NewInvocation {
                hook_kind: &hook_kind,
                status: PluginInvocationStatus::Failed,
                input: req.input,
                output: json!({}),
                error_message: Some("plugin has no wasm module".to_string()),
                duration_ms: 0,
                fuel_consumed: None,
            },
        )
        .await?;
        insert_plugin_invoked_event(
            &tx,
            &row,
            &invocation,
            if is_bot { None } else { Some(actor_id) },
            json!({ "type": if is_bot { "bot" } else { "user" }, "actor_id": actor_id }),
        )
        .await?;
        tx.commit().await?;
        return Ok(ApiResponse::success(invocation));
    };

    let input = json!({
        "hook_kind": hook_kind,
        "plugin_key": row.key,
        "payload": req.input,
    });
    let result = invoke_wasm_plugin(wasm_bytes, input.clone(), manifest.capabilities.runtime).await;
    let tx = state.db.begin().await?;
    let invocation = match result {
        Ok(output) => insert_success_invocation(&tx, &row, &hook_kind, input, output).await?,
        Err(err) => insert_failed_invocation(&tx, &row, &hook_kind, input, err).await?,
    };
    insert_plugin_invoked_event(
        &tx,
        &row,
        &invocation,
        if is_bot { None } else { Some(actor_id) },
        json!({ "type": if is_bot { "bot" } else { "user" }, "actor_id": actor_id }),
    )
    .await?;
    tx.commit().await?;

    Ok(ApiResponse::success(invocation))
}

pub async fn list_plugin_invocations(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    bot: Option<Extension<BotAuthContext>>,
    Path(plugin_id): Path<Uuid>,
    Query(query): Query<ListPluginsQuery>,
) -> Result<impl IntoResponse, ApiError> {
    let plugin = get_plugin_response(&state, plugin_id).await?;
    require_workspace_access_from_auth(&state, &claims, bot.as_ref().map(|b| &b.0), plugin.workspace_id).await?;
    let page = query.page.unwrap_or(1).max(1);
    let per_page = query.per_page.unwrap_or(50).clamp(1, 200);
    let offset = (page - 1) * per_page;
    let total = count_query(
        &state,
        "SELECT COUNT(*)::bigint AS count FROM plugin_invocations WHERE plugin_id = $1",
        vec![plugin_id.into()],
    )
    .await?;
    let items = PluginInvocationResponse::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        r"SELECT id, workspace_id, project_id, plugin_id, plugin_key, hook_kind, status,
                  input, output, error_message, duration_ms, fuel_consumed, created_at
             FROM plugin_invocations
            WHERE plugin_id = $1
            ORDER BY created_at DESC
            LIMIT $2 OFFSET $3",
        vec![plugin_id.into(), per_page.into(), offset.into()],
    ))
    .all(&state.db)
    .await?;

    Ok(ApiResponse::success(PaginatedData {
        items,
        total,
        page,
        per_page,
        total_pages: total_pages(total, per_page),
    }))
}

async fn ensure_project_access(
    state: &AppState,
    claims: &JwtClaims,
    bot: Option<&BotAuthContext>,
    project_id: Uuid,
) -> Result<Uuid, ApiError> {
    Ok(ensure_project_actor(state, claims, bot, project_id).await?.0)
}

async fn ensure_project_actor(
    state: &AppState,
    claims: &JwtClaims,
    bot: Option<&BotAuthContext>,
    project_id: Uuid,
) -> Result<(Uuid, Uuid, bool), ApiError> {
    let row = ProjectWorkspace::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "SELECT workspace_id FROM projects WHERE id = $1",
        vec![project_id.into()],
    ))
    .one(&state.db)
    .await?
    .ok_or_else(|| ApiError::NotFound("project not found".to_string()))?;
    let (actor_id, _, is_bot) = require_workspace_access_from_auth(state, claims, bot, row.workspace_id).await?;
    Ok((row.workspace_id, actor_id, is_bot))
}

async fn get_plugin_response(state: &AppState, plugin_id: Uuid) -> Result<PluginResponse, ApiError> {
    PluginResponse::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        r"SELECT id, workspace_id, project_id, key, name, version, description, manifest,
                  wasm_sha256, status, installed_by, created_at, updated_at
             FROM plugins
            WHERE id = $1",
        vec![plugin_id.into()],
    ))
    .one(&state.db)
    .await?
    .ok_or_else(|| ApiError::NotFound("plugin not found".to_string()))
}

async fn get_plugin_runtime_row(state: &AppState, plugin_id: Uuid) -> Result<PluginRuntimeRow, ApiError> {
    PluginRuntimeRow::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        r"SELECT id, workspace_id, project_id, key, manifest, wasm_bytes, status
             FROM plugins
            WHERE id = $1",
        vec![plugin_id.into()],
    ))
    .one(&state.db)
    .await?
    .ok_or_else(|| ApiError::NotFound("plugin not found".to_string()))
}

async fn insert_success_invocation<C>(
    db: &C,
    plugin: &PluginRuntimeRow,
    hook_kind: &str,
    input: Value,
    output: PluginRuntimeOutput,
) -> Result<PluginInvocationResponse, ApiError>
where
    C: ConnectionTrait,
{
    insert_invocation(
        db,
        plugin,
        NewInvocation {
            hook_kind,
            status: PluginInvocationStatus::Completed,
            input,
            output: output.output,
            error_message: None,
            duration_ms: i64::try_from(output.duration_ms).unwrap_or(i64::MAX),
            fuel_consumed: output.fuel_consumed.and_then(|value| i64::try_from(value).ok()),
        },
    )
    .await
}

/// Records a run that produced no output: `timeout` for an expired deadline, `failed` for
/// anything else, with the elapsed time and whatever fuel the store knew it had burnt.
async fn insert_failed_invocation<C>(
    db: &C,
    plugin: &PluginRuntimeRow,
    hook_kind: &str,
    input: Value,
    error: PluginRuntimeError,
) -> Result<PluginInvocationResponse, ApiError>
where
    C: ConnectionTrait,
{
    insert_invocation(
        db,
        plugin,
        NewInvocation {
            hook_kind,
            status: error.kind.into(),
            input,
            output: json!({}),
            error_message: Some(error.message),
            duration_ms: i64::try_from(error.duration_ms).unwrap_or(i64::MAX),
            fuel_consumed: error.fuel_consumed.and_then(|value| i64::try_from(value).ok()),
        },
    )
    .await
}

/// One `plugin_invocations` row, as written.
struct NewInvocation<'a> {
    hook_kind: &'a str,
    status: PluginInvocationStatus,
    input: Value,
    output: Value,
    error_message: Option<String>,
    duration_ms: i64,
    fuel_consumed: Option<i64>,
}

async fn insert_invocation<C>(
    db: &C,
    plugin: &PluginRuntimeRow,
    invocation: NewInvocation<'_>,
) -> Result<PluginInvocationResponse, ApiError>
where
    C: ConnectionTrait,
{
    PluginInvocationResponse::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        r"INSERT INTO plugin_invocations (
                workspace_id, project_id, plugin_id, plugin_key, hook_kind, status,
                input, output, error_message, duration_ms, fuel_consumed
            )
            VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11)
            RETURNING id, workspace_id, project_id, plugin_id, plugin_key, hook_kind, status,
                      input, output, error_message, duration_ms, fuel_consumed, created_at",
        vec![
            plugin.workspace_id.into(),
            plugin.project_id.into(),
            plugin.id.into(),
            plugin.key.clone().into(),
            invocation.hook_kind.to_string().into(),
            invocation.status.as_str().into(),
            invocation.input.into(),
            invocation.output.into(),
            invocation.error_message.into(),
            invocation.duration_ms.into(),
            invocation.fuel_consumed.into(),
        ],
    ))
    .one(db)
    .await?
    .ok_or(ApiError::Internal)
}

async fn insert_plugin_invoked_event<C>(
    db: &C,
    plugin: &PluginRuntimeRow,
    invocation: &PluginInvocationResponse,
    actor_id: Option<Uuid>,
    source: Value,
) -> Result<(), ApiError>
where
    C: ConnectionTrait,
{
    insert_business_event(
        db,
        BusinessEventInput {
            workspace_id: plugin.workspace_id,
            project_id: Some(plugin.project_id),
            event_type: "plugin.invoked".to_string(),
            aggregate_type: "plugin".to_string(),
            aggregate_id: plugin.id.to_string(),
            actor_id,
            source,
            payload: json!({
                "plugin_id": plugin.id,
                "plugin_key": plugin.key,
                "invocation_id": invocation.id,
                "hook_kind": invocation.hook_kind,
                "status": invocation.status,
                "duration_ms": invocation.duration_ms,
                "fuel_consumed": invocation.fuel_consumed
            }),
            metadata: json!({
                "plugin_id": plugin.id,
                "plugin_key": plugin.key,
                "invocation_id": invocation.id,
                "hook_kind": invocation.hook_kind
            }),
            correlation_id: None,
            causation_id: None,
            idempotency_key: None,
        },
    )
    .await?;
    Ok(())
}

async fn insert_plugin_event<C>(
    db: &C,
    plugin: &PluginResponse,
    event_type: &str,
    actor_id: Option<Uuid>,
    source: Value,
    payload: Value,
) -> Result<(), ApiError>
where
    C: ConnectionTrait,
{
    insert_business_event(
        db,
        BusinessEventInput {
            workspace_id: plugin.workspace_id,
            project_id: Some(plugin.project_id),
            event_type: event_type.to_string(),
            aggregate_type: "plugin".to_string(),
            aggregate_id: plugin.id.to_string(),
            actor_id,
            source,
            payload,
            metadata: json!({
                "plugin_id": plugin.id,
                "plugin_key": plugin.key,
                "version": plugin.version
            }),
            correlation_id: None,
            causation_id: None,
            idempotency_key: None,
        },
    )
    .await?;
    Ok(())
}

async fn count_query(state: &AppState, sql: &str, values: Vec<sea_orm::Value>) -> Result<i64, ApiError> {
    let row = CountRow::find_by_statement(Statement::from_sql_and_values(DbBackend::Postgres, sql, values))
        .one(&state.db)
        .await?;
    Ok(row.map_or(0, |item| item.count))
}

const fn total_pages(total: i64, per_page: i64) -> i64 {
    if total == 0 {
        0
    } else {
        (total + per_page - 1) / per_page
    }
}

fn normalize_plugin_status(value: Option<&str>) -> Result<String, ApiError> {
    match value.unwrap_or("active").trim() {
        "active" | "disabled" | "failed" => Ok(value.unwrap_or("active").trim().to_string()),
        other => Err(ApiError::BadRequest(format!("unsupported plugin status: {other}"))),
    }
}

fn normalize_hook_kind(value: &str) -> Result<String, ApiError> {
    let value = value.trim();
    if value.is_empty() {
        return Err(ApiError::BadRequest("hook_kind is required".to_string()));
    }
    Ok(value.to_string())
}

fn decode_and_validate_wasm(value: Option<&str>) -> Result<(Option<Vec<u8>>, Option<String>), ApiError> {
    let Some(value) = value.map(str::trim).filter(|value| !value.is_empty()) else {
        return Ok((None, None));
    };
    let bytes = STANDARD
        .decode(value.as_bytes())
        .map_err(|err| ApiError::BadRequest(format!("wasm_base64 is invalid: {err}")))?;
    validate_wasm_module(&bytes).map_err(ApiError::BadRequest)?;
    let hash = Sha256::digest(&bytes);
    Ok((Some(bytes), Some(format!("{hash:x}"))))
}

fn map_plugin_insert_error(err: sea_orm::DbErr) -> ApiError {
    let text = err.to_string();
    if text.contains("plugins_project_key_version_unique") {
        ApiError::Conflict("plugin key/version already installed for this project".to_string())
    } else {
        ApiError::Database(err)
    }
}

#[cfg(test)]
mod tests {
    use super::{decode_and_validate_wasm, normalize_plugin_status};

    #[test]
    fn normalizes_plugin_status() {
        assert_eq!(normalize_plugin_status(None).expect("status"), "active");
        assert_eq!(normalize_plugin_status(Some("disabled")).expect("status"), "disabled");
        assert!(normalize_plugin_status(Some("deleted")).is_err());
    }

    #[test]
    fn rejects_invalid_wasm_base64() {
        assert!(decode_and_validate_wasm(Some("not wasm")).is_err());
    }
}

/// What `plugin_invocations` records for each way a plugin run can end, through the real invoke
/// route and the real automatic hook path, against a migrated scratch database.
#[cfg(test)]
mod invocation_record_database_tests {
    use super::{InvokePluginRequest, invoke_plugin};
    use crate::plugins::hooks::run_event_handler_hooks;
    use crate::routes::context::tenant_fixture::{Tenant, exec, seed_tenant};
    use crate::scratch_or_skip;
    use axum::extract::{Extension, Json, Path, State};
    use axum::response::IntoResponse;
    use platform::{
        app::AppState,
        auth::{JwtClaims, TokenType},
        config::{AppConfig, Secret},
    };
    use sea_orm::{DatabaseConnection, DbBackend, FromQueryResult, Statement};
    use serde_json::{Value, json};
    use uuid::Uuid;

    fn state_for(db: DatabaseConnection) -> AppState {
        AppState {
            cfg: AppConfig {
                app_name: "plugin-invocation-test".to_string(),
                bind_addr: "127.0.0.1:0".to_string(),
                database_url: Secret::new("postgres://unused/unused"),
                jwt_secret: Secret::new("plugin-invocation-test-secret"),
                jwt_access_ttl_seconds: 900,
                jwt_refresh_ttl_seconds: 3600,
                default_author_id: None,
                allow_insecure_cookies: false,
                collab_allowed_origins: Vec::new(),
            },
            db,
            flow_permission_cache: platform::app::FlowPermissionCacheSlot::default(),
        }
    }

    fn claims_for(user_id: Uuid) -> JwtClaims {
        JwtClaims {
            sub: user_id.to_string(),
            email: format!("{user_id}@tenant.test"),
            token_type: TokenType::Access,
            iat: 0,
            exp: 0,
        }
    }

    /// A guest whose `openpr_invoke` loops forever.
    fn spin_forever_wasm() -> Vec<u8> {
        wat::parse_str(
            r#"
            (module
              (memory (export "memory") 1)
              (func (export "sylvode_alloc") (param i32) (result i32) i32.const 0)
              (func (export "sylvode_invoke") (param i32) (param i32) (result i64)
                (loop $again
                  br $again)
                i64.const 0))
            "#,
        )
        .expect("wat should compile")
    }

    /// A guest that counts down from `iterations`, then traps on `unreachable`.
    fn count_down_then_trap_wasm(iterations: u64) -> Vec<u8> {
        wat::parse_str(format!(
            r#"
            (module
              (memory (export "memory") 1)
              (func (export "sylvode_alloc") (param i32) (result i32) i32.const 0)
              (func (export "sylvode_invoke") (param i32) (param i32) (result i64)
                (local $n i64)
                i64.const {iterations}
                local.set $n
                (loop $again
                  local.get $n
                  i64.const 1
                  i64.sub
                  local.tee $n
                  i64.const 0
                  i64.gt_s
                  br_if $again)
                unreachable))
            "#
        ))
        .expect("wat should compile")
    }

    /// Installs an active plugin with one `event_handler` hook, straight into the table, so
    /// that even bytes the install route would refuse can be stored.
    async fn install(
        db: &DatabaseConnection,
        tenant: &Tenant,
        key: &str,
        wasm: Vec<u8>,
        timeout_ms: u64,
        fuel: u64,
    ) -> Uuid {
        let plugin_id = Uuid::new_v4();
        let manifest = json!({
            "key": key,
            "name": key,
            "version": "1.0.0",
            "capabilities": {
                "hooks": [{"kind": "event_handler", "event_type": "record.created"}],
                "runtime": {"timeout_ms": timeout_ms, "fuel": fuel, "memory_bytes": 1_048_576}
            }
        });
        exec(
            db,
            "INSERT INTO plugins (id, workspace_id, project_id, key, name, version, manifest, wasm_bytes, status) \
             VALUES ($1, $2, $3, $4, $4, '1.0.0', $5, $6, 'active')",
            vec![
                plugin_id.into(),
                tenant.workspace_id.into(),
                tenant.project_id.into(),
                key.into(),
                manifest.into(),
                wasm.into(),
            ],
        )
        .await;
        plugin_id
    }

    #[derive(Debug, FromQueryResult)]
    struct Recorded {
        status: String,
        error_message: Option<String>,
        duration_ms: i64,
        fuel_consumed: Option<i64>,
        event_status: Option<String>,
    }

    /// The single invocation row of `plugin_id`, with the status its `plugin.invoked` event carries.
    async fn recorded(db: &DatabaseConnection, plugin_id: Uuid) -> Recorded {
        let rows = Recorded::find_by_statement(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT i.status, i.error_message, i.duration_ms, i.fuel_consumed, \
                    (SELECT e.payload->>'status' FROM business_events e \
                      WHERE e.event_type = 'plugin.invoked' AND e.aggregate_id = $1::text) AS event_status \
               FROM plugin_invocations i WHERE i.plugin_id = $1",
            vec![plugin_id.into()],
        ))
        .all(db)
        .await
        .expect("invocations load");
        assert_eq!(
            rows.len(),
            1,
            "expected exactly one invocation of {plugin_id}: {rows:?}"
        );
        rows.into_iter().next().expect("one row")
    }

    async fn invoke(state: &AppState, tenant: &Tenant, plugin_id: Uuid) -> Value {
        let response = invoke_plugin(
            State(state.clone()),
            Extension(claims_for(tenant.member_id)),
            None,
            Path(plugin_id),
            Json(InvokePluginRequest {
                hook_kind: "event_handler".to_string(),
                input: json!({}),
            }),
        )
        .await
        .expect("the invoke route records the outcome")
        .into_response();
        let bytes = axum::body::to_bytes(response.into_body(), 1024 * 1024)
            .await
            .expect("the response body is readable");
        serde_json::from_slice(&bytes).expect("the response body is JSON")
    }

    /// A deadline expiry is stored as `timeout`, not `failed`, with the elapsed wall time and the
    /// fuel the guest had burnt when it was interrupted; the message keeps its exact text.
    #[tokio::test]
    async fn a_deadline_expiry_is_recorded_as_timeout_with_its_real_cost() {
        let scratch = scratch_or_skip!("plugin_timeout_record");
        let state = state_for(scratch.db.clone());
        let tenant = seed_tenant(&scratch.db, "pt").await;
        let plugin_id = install(&scratch.db, &tenant, "spinner", spin_forever_wasm(), 100, 1_000_000_000).await;

        let body = invoke(&state, &tenant, plugin_id).await;
        let row = recorded(&scratch.db, plugin_id).await;

        assert_eq!(row.status, "timeout", "{row:?}");
        assert_eq!(row.error_message.as_deref(), Some("wasm execution timeout after 100ms"));
        assert!(row.duration_ms >= 100, "{row:?}");
        assert!(row.fuel_consumed.is_some_and(|fuel| fuel > 0), "{row:?}");
        assert_eq!(row.event_status.as_deref(), Some("timeout"), "{row:?}");
        assert_eq!(body.pointer("/data/status"), Some(&json!("timeout")), "{body}");
        assert_eq!(
            body.pointer("/data/duration_ms"),
            Some(&json!(row.duration_ms)),
            "{body}"
        );
        assert_eq!(
            body.pointer("/data/fuel_consumed"),
            Some(&json!(row.fuel_consumed)),
            "{body}"
        );
        scratch.drop_self().await;
    }

    /// A guest trap after real work stays `failed`, but keeps its elapsed time and its fuel;
    /// fuel exhaustion records the whole budget as consumed; a module that never compiled has no
    /// fuel to report.
    #[tokio::test]
    async fn failures_record_elapsed_time_and_known_fuel() {
        let scratch = scratch_or_skip!("plugin_failure_record");
        let state = state_for(scratch.db.clone());
        let tenant = seed_tenant(&scratch.db, "pf").await;

        let trapping = install(
            &scratch.db,
            &tenant,
            "trapper",
            count_down_then_trap_wasm(50_000_000),
            30_000,
            1_000_000_000,
        )
        .await;
        invoke(&state, &tenant, trapping).await;
        let row = recorded(&scratch.db, trapping).await;
        assert_eq!(row.status, "failed", "{row:?}");
        assert!(
            row.error_message
                .as_deref()
                .is_some_and(|message| message.starts_with("plugin invocation trapped")),
            "{row:?}"
        );
        assert!(row.duration_ms > 0, "{row:?}");
        assert!(row.fuel_consumed.is_some_and(|fuel| fuel >= 50_000_000), "{row:?}");
        assert_eq!(row.event_status.as_deref(), Some("failed"), "{row:?}");

        let exhausted = install(&scratch.db, &tenant, "exhausted", spin_forever_wasm(), 30_000, 10).await;
        invoke(&state, &tenant, exhausted).await;
        let row = recorded(&scratch.db, exhausted).await;
        assert_eq!(row.status, "failed", "{row:?}");
        assert!(
            row.error_message
                .as_deref()
                .is_some_and(|message| message.contains("fuel")),
            "{row:?}"
        );
        assert_eq!(row.fuel_consumed, Some(10), "{row:?}");

        let broken = install(
            &scratch.db,
            &tenant,
            "broken",
            b"not a wasm module".to_vec(),
            30_000,
            10,
        )
        .await;
        invoke(&state, &tenant, broken).await;
        let row = recorded(&scratch.db, broken).await;
        assert_eq!(row.status, "failed", "{row:?}");
        assert!(
            row.error_message
                .as_deref()
                .is_some_and(|message| message.starts_with("invalid wasm module")),
            "{row:?}"
        );
        assert_eq!(row.fuel_consumed, None, "{row:?}");
        scratch.drop_self().await;
    }

    /// The automatic hook path records the same truth as the invoke route.
    #[tokio::test]
    async fn an_automatic_hook_timeout_is_recorded_as_timeout() {
        let scratch = scratch_or_skip!("plugin_hook_timeout_record");
        let state = state_for(scratch.db.clone());
        let tenant = seed_tenant(&scratch.db, "ph").await;
        let plugin_id = install(
            &scratch.db,
            &tenant,
            "hookspin",
            spin_forever_wasm(),
            100,
            1_000_000_000,
        )
        .await;

        run_event_handler_hooks(
            &state,
            tenant.workspace_id,
            tenant.project_id,
            Uuid::new_v4(),
            "orders",
            None,
            "record.created",
            json!({}),
        )
        .await
        .expect("an event handler failure does not fail the caller");
        let row = recorded(&scratch.db, plugin_id).await;

        assert_eq!(row.status, "timeout", "{row:?}");
        assert_eq!(row.error_message.as_deref(), Some("wasm execution timeout after 100ms"));
        assert!(row.duration_ms >= 100, "{row:?}");
        assert!(row.fuel_consumed.is_some_and(|fuel| fuel > 0), "{row:?}");
        assert_eq!(row.event_status.as_deref(), Some("timeout"), "{row:?}");
        scratch.drop_self().await;
    }
}
