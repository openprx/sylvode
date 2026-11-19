//! Private plugin releases and explicit project admission.
use super::{
    ActivePackage, EXECUTIONS, InstallRequest, Package, admin, audited_error, bad, busy, enabled, guest_input,
    lock_admin, lock_binding, lock_project, require_extensions, runtime_policy, sha256, statement, validate_install,
};
use crate::{
    error::{ApiError, ApiErrorKind},
    forms::permissions::role_is_form_admin,
    plugins::{
        package::{PREVIEW, parse_guest_amount},
        package_v3::{SCHEMA_V3, plugin_error},
        runtime::{PluginFailureKind, invoke_wasm_plugin},
    },
    response::ApiResponse,
};
use axum::{
    Extension, Json,
    extract::{Path, State},
    response::IntoResponse,
};
use platform::timestamp::Timestamp;
use platform::{app::AppState, auth::JwtClaims};
use sea_orm::{ConnectionTrait, DatabaseTransaction, FromQueryResult, TransactionTrait};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use uuid::Uuid;

fn user(claims: &JwtClaims) -> Result<Uuid, ApiError> {
    Uuid::parse_str(&claims.sub).map_err(|_| ApiError::unauthenticated("invalid user identity"))
}

#[derive(FromQueryResult, Serialize)]
struct PersonalPackage {
    id: Uuid,
    manifest: Value,
    package_sha256: String,
    installed_by: Uuid,
    installed_at: Timestamp,
    archived_at: Option<Timestamp>,
    project_links: Value,
}

#[derive(FromQueryResult, Serialize)]
struct ProjectChoice {
    id: Uuid,
    name: String,
    key: String,
    workspace_id: Uuid,
    workspace_name: String,
    can_manage: bool,
}

pub async fn list(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
) -> Result<impl IntoResponse, ApiError> {
    enabled()?;
    let actor = user(&claims)?;
    let items = PersonalPackage::find_by_statement(statement(
        "SELECT s.id,s.manifest,s.package_sha256,s.owner_id AS installed_by,s.installed_at,s.archived_at,
                COALESCE((
                    SELECT jsonb_agg(jsonb_build_object('project_id',r.project_id,'package_id',
                         CASE WHEN r.status='attached' AND member.user_id IS NOT NULL THEN p.id END,'status',r.status,
                         'active',COALESCE(member.user_id IS NOT NULL AND b.package_id=p.id AND cardinality(b.grants)>0,false)) ORDER BY r.project_id)
                      FROM form_plugin_library_requests r
                      JOIN projects project ON project.id=r.project_id
                      LEFT JOIN workspace_members member ON member.workspace_id=project.workspace_id AND member.user_id=s.owner_id
                      LEFT JOIN form_plugin_packages p ON p.project_id=r.project_id AND p.library_source_id=s.id
                      LEFT JOIN form_plugin_bindings b ON b.project_id=r.project_id AND b.key=s.key
                     WHERE r.source_id=s.id
                ),'[]'::jsonb) AS project_links
           FROM form_plugin_library s WHERE s.owner_id=$1
          ORDER BY s.archived_at NULLS FIRST,s.key,s.installed_at DESC,s.id",
        vec![actor.into()],
    ))
    .all(&state.db)
    .await?;
    let projects = ProjectChoice::find_by_statement(statement(
        "SELECT p.id,p.name,p.key,w.id AS workspace_id,w.name AS workspace_name,lower(btrim(m.role)) IN ('owner','admin') AS can_manage
           FROM workspace_members m JOIN workspaces w ON w.id=m.workspace_id
           JOIN projects p ON p.workspace_id=w.id
          WHERE m.user_id=$1 ORDER BY w.name,p.name,p.id",
        vec![actor.into()],
    ))
    .all(&state.db)
    .await?;
    Ok(ApiResponse::success(json!({"items":items,"projects":projects})))
}

pub async fn save(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    Json(request): Json<InstallRequest>,
) -> Result<impl IntoResponse, ApiError> {
    enabled()?;
    let actor = user(&claims)?;
    let package = validate_install(request).await?;
    let tx = state.db.begin().await?;
    tx.query_one(statement(
        "SELECT id FROM users WHERE id=$1 FOR UPDATE",
        vec![actor.into()],
    ))
    .await?
    .ok_or_else(|| ApiError::unauthenticated("invalid user identity"))?;
    let previous = PersonalPackage::find_by_statement(statement(
        "SELECT id,manifest,package_sha256,owner_id AS installed_by,installed_at,archived_at,'[]'::jsonb AS project_links
           FROM form_plugin_library WHERE owner_id=$1 AND key=$2 AND version=$3",
        vec![actor.into(), package.key.clone().into(), package.version.clone().into()],
    ))
    .one(&tx)
    .await?;
    if let Some(previous) = previous {
        if previous.package_sha256 != package.package_sha256 || previous.archived_at.is_some() {
            return Err(ApiError::Conflict(
                "this library key and version is already reserved; publish a new version".into(),
            ));
        }
        tx.commit().await?;
        return Ok(ApiResponse::success(previous));
    }
    let count = tx
        .query_one(statement(
            "SELECT count(*) AS count FROM form_plugin_library WHERE owner_id=$1",
            vec![actor.into()],
        ))
        .await?
        .ok_or(ApiError::Internal)?
        .try_get::<i64>("", "count")?;
    if count >= 128 {
        return Err(bad("personal plugin version limit reached"));
    }
    let package = PersonalPackage::find_by_statement(statement(
        "INSERT INTO form_plugin_library (owner_id,key,version,manifest,package_sha256,wasm_bytes,html)
         VALUES ($1,$2,$3,$4,$5,$6,$7)
         RETURNING id,manifest,package_sha256,owner_id AS installed_by,installed_at,archived_at,'[]'::jsonb AS project_links",
        vec![
            actor.into(),
            package.key.into(),
            package.version.into(),
            package.manifest.into(),
            package.package_sha256.into(),
            package.wasm.into(),
            package.html.into(),
        ],
    )).one(&tx).await?.ok_or(ApiError::Internal)?;
    tx.commit().await?;
    Ok(ApiResponse::success(package))
}

fn package_missing() -> ApiError {
    ApiError::not_found("personal plugin version not found")
}

pub async fn archive(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    Path(source): Path<Uuid>,
) -> Result<impl IntoResponse, ApiError> {
    enabled()?;
    let actor = user(&claims)?;
    let tx = state.db.begin().await?;
    tx.query_one(statement(
        "UPDATE form_plugin_library SET archived_at=COALESCE(archived_at,clock_timestamp())
          WHERE id=$1 AND owner_id=$2 RETURNING id",
        vec![source.into(), actor.into()],
    ))
    .await?
    .ok_or_else(package_missing)?;
    tx.execute(statement(
        "UPDATE form_plugin_library_requests SET status='revoked',decided_by=$2,decided_at=clock_timestamp()
          WHERE source_id=$1 AND status='pending'",
        vec![source.into(), actor.into()],
    ))
    .await?;
    tx.commit().await?;
    Ok(ApiResponse::success(json!({"archived":true})))
}

pub async fn panel(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    Path(source): Path<Uuid>,
) -> Result<impl IntoResponse, ApiError> {
    enabled()?;
    let actor = user(&claims)?;
    let row = state
        .db
        .query_one(statement(
            "SELECT html,CASE WHEN manifest->>'schema_version'=$3 THEN manifest->'entry'->>'sha256'
                    ELSE manifest->'ui'->'entry'->>'sha256' END AS web_sha256 FROM form_plugin_library
          WHERE id=$1 AND owner_id=$2 AND archived_at IS NULL",
            vec![source.into(), actor.into(), SCHEMA_V3.into()],
        ))
        .await?
        .ok_or_else(package_missing)?;
    Ok(ApiResponse::success(json!({
        "html":row.try_get::<String>("","html")?,
        "web_sha256":row.try_get::<String>("","web_sha256")?,
    })))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PreviewRequest {
    quantity: u64,
    unit_price_cents: u64,
}

pub async fn preview(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    Path(source): Path<Uuid>,
    Json(request): Json<PreviewRequest>,
) -> Result<impl IntoResponse, ApiError> {
    enabled()?;
    let actor = user(&claims)?;
    if !(1..=1_000_000).contains(&request.quantity) || request.unit_price_cents > 1_000_000_000 {
        return Err(bad(
            "personal preview input is outside the supported quantity or price range",
        ));
    }
    let profile = state
        .db
        .query_one(statement(
            "SELECT manifest->>'schema_version'=$3 AND manifest->>'profile'='ui' AS ui_only
               FROM form_plugin_library WHERE id=$1 AND owner_id=$2 AND archived_at IS NULL",
            vec![source.into(), actor.into(), SCHEMA_V3.into()],
        ))
        .await?
        .ok_or_else(package_missing)?;
    if profile.try_get::<bool>("", "ui_only")? {
        return Err(plugin_error(
            ApiErrorKind::InvalidUpdate,
            "profile_mismatch",
            "a ui profile release has no WASM to preview",
        ));
    }
    let package = ActivePackage::find_by_statement(statement(
        "SELECT key,version,wasm_bytes,manifest->'wasm'->>'sha256' AS wasm_sha256
           FROM form_plugin_library WHERE id=$1 AND owner_id=$2 AND archived_at IS NULL",
        vec![source.into(), actor.into()],
    ))
    .one(&state.db)
    .await?
    .ok_or_else(package_missing)?;
    if sha256(&package.wasm_bytes) != package.wasm_sha256 {
        return Err(ApiError::Internal);
    }
    let _capacity = EXECUTIONS.try_acquire().map_err(|_| busy())?;
    let runtime = invoke_wasm_plugin(
        package.wasm_bytes,
        guest_input(PREVIEW, request.quantity, request.unit_price_cents),
        runtime_policy(),
    )
    .await
    .map_err(|error| {
        let kind = if error.kind == PluginFailureKind::Timeout {
            "timeout"
        } else {
            "execution_failed"
        };
        audited_error(kind)
    })?;
    Ok(ApiResponse::success(
        json!({"amount_cents":parse_guest_amount(&runtime.output)?}),
    ))
}

async fn member_role(tx: &DatabaseTransaction, project: Uuid, actor: Uuid) -> Result<String, ApiError> {
    tx.query_one(statement(
        "SELECT m.role FROM projects p JOIN workspace_members m ON m.workspace_id=p.workspace_id
          WHERE p.id=$1 AND m.user_id=$2 FOR SHARE OF m",
        vec![project.into(), actor.into()],
    ))
    .await?
    .ok_or_else(|| ApiError::not_found("project not found"))?
    .try_get("", "role")
    .map_err(Into::into)
}

async fn set_status(
    tx: &DatabaseTransaction,
    project: Uuid,
    source: Uuid,
    status: &str,
    actor: Uuid,
) -> Result<(), ApiError> {
    tx.execute(statement(
        "INSERT INTO form_plugin_library_requests (project_id,source_id,status,decided_by,decided_at)
         VALUES ($1,$2,$3,CASE WHEN $3='pending' THEN NULL ELSE $4::uuid END,
                 CASE WHEN $3='pending' THEN NULL ELSE clock_timestamp() END)
         ON CONFLICT (project_id,source_id) DO UPDATE SET status=EXCLUDED.status,
                submitted_at=CASE WHEN $3='pending' AND form_plugin_library_requests.status<>'pending'
                  THEN EXCLUDED.submitted_at ELSE form_plugin_library_requests.submitted_at END,
                decided_by=EXCLUDED.decided_by,decided_at=EXCLUDED.decided_at",
        vec![project.into(), source.into(), status.into(), actor.into()],
    ))
    .await?;
    Ok(())
}

async fn install_projection(
    tx: &DatabaseTransaction,
    project: Uuid,
    source: Uuid,
    actor: Uuid,
) -> Result<Package, ApiError> {
    if let Some(existing) = tx
        .query_one(statement(
            "SELECT p.library_source_id FROM form_plugin_packages p JOIN form_plugin_library s USING (key,version)
          WHERE p.project_id=$1 AND s.id=$2",
            vec![project.into(), source.into()],
        ))
        .await?
    {
        if existing.try_get::<Option<Uuid>>("", "library_source_id")? != Some(source) {
            return Err(ApiError::Conflict(
                "this project already reserves the plugin key and version".into(),
            ));
        }
    } else {
        let count = tx
            .query_one(statement(
                "SELECT count(*) AS count FROM form_plugin_packages WHERE project_id=$1",
                vec![project.into()],
            ))
            .await?
            .ok_or(ApiError::Internal)?
            .try_get::<i64>("", "count")?;
        if count >= 32 {
            return Err(bad("project package limit reached"));
        }
    }
    let item = Package::find_by_statement(statement(
        "INSERT INTO form_plugin_packages (project_id,key,version,manifest,package_sha256,wasm_bytes,html,installed_by,library_source_id)
         SELECT $1,s.key,s.version,s.manifest,s.package_sha256,s.wasm_bytes,s.html,$3,s.id
           FROM form_plugin_library s WHERE s.id=$2 AND s.archived_at IS NULL
         ON CONFLICT (project_id,key,version) DO UPDATE SET library_revoked_at=NULL
          WHERE form_plugin_packages.library_source_id=EXCLUDED.library_source_id
         RETURNING id,manifest,package_sha256,installed_by,installed_at,library_source_id,
                   (SELECT placements FROM form_plugin_layouts WHERE package_id=form_plugin_packages.id) AS placements",
        vec![project.into(),source.into(),actor.into()],
    )).one(tx).await?.ok_or_else(package_missing)?;
    set_status(tx, project, source, "attached", actor).await?;
    item.with_layout(tx, project).await
}

pub async fn attach(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    Path((project, source)): Path<(Uuid, Uuid)>,
) -> Result<impl IntoResponse, ApiError> {
    enabled()?;
    let actor = user(&claims)?;
    let tx = state.db.begin().await?;
    let role = member_role(&tx, project, actor).await?;
    lock_project(&tx, project).await?;
    let release = tx
        .query_one(statement(
            "SELECT manifest->>'schema_version'=$3 AS v3 FROM form_plugin_library
              WHERE id=$1 AND owner_id=$2 AND archived_at IS NULL FOR SHARE",
            vec![source.into(), actor.into(), SCHEMA_V3.into()],
        ))
        .await?
        .ok_or_else(package_missing)?;
    if release.try_get::<bool>("", "v3")? {
        require_extensions()?;
    }
    let response = if role_is_form_admin(&role) {
        let package = install_projection(&tx, project, source, actor).await?;
        json!({"status":"attached","package":package})
    } else if tx
        .query_one(statement(
            "SELECT 1 FROM form_plugin_library_requests WHERE project_id=$1 AND source_id=$2 AND status='attached'",
            vec![project.into(), source.into()],
        ))
        .await?
        .is_some()
    {
        json!({"status":"attached"})
    } else {
        set_status(&tx, project, source, "pending", actor).await?;
        json!({"status":"pending"})
    };
    tx.commit().await?;
    Ok(ApiResponse::success(response))
}

#[derive(FromQueryResult, Serialize)]
struct PendingRequest {
    source_id: Uuid,
    owner_id: Uuid,
    manifest: Value,
    package_sha256: String,
    submitted_at: Timestamp,
}

pub async fn requests(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    Path(project): Path<Uuid>,
) -> Result<impl IntoResponse, ApiError> {
    admin(&state, &claims, project).await?;
    let items = PendingRequest::find_by_statement(statement(
        "SELECT r.source_id,s.owner_id,s.manifest,s.package_sha256,r.submitted_at
           FROM form_plugin_library_requests r JOIN form_plugin_library s ON s.id=r.source_id
          WHERE r.project_id=$1 AND r.status='pending' AND s.archived_at IS NULL
          ORDER BY r.submitted_at,r.source_id",
        vec![project.into()],
    ))
    .all(&state.db)
    .await?;
    Ok(ApiResponse::success(json!({"items":items})))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Decision {
    approve: bool,
}

pub async fn decide(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    Path((project, source)): Path<(Uuid, Uuid)>,
    Json(decision): Json<Decision>,
) -> Result<impl IntoResponse, ApiError> {
    let (workspace, actor) = admin(&state, &claims, project).await?;
    let tx = state.db.begin().await?;
    lock_admin(&tx, workspace, actor).await?;
    lock_project(&tx, project).await?;
    let row = tx
        .query_one(statement(
            "SELECT s.owner_id,s.manifest->>'schema_version'=$3 AS v3
               FROM form_plugin_library_requests r JOIN form_plugin_library s ON s.id=r.source_id
          WHERE r.project_id=$1 AND r.source_id=$2 AND r.status='pending' AND s.archived_at IS NULL FOR SHARE OF s",
            vec![project.into(), source.into(), SCHEMA_V3.into()],
        ))
        .await?
        .ok_or_else(|| ApiError::not_found("pending plugin request not found"))?;
    if row.try_get::<bool>("", "v3")? {
        require_extensions()?;
    }
    let response = if decision.approve {
        // Membership at submission alone cannot authorize a new project install after removal.
        let owner = row.try_get::<Uuid>("", "owner_id")?;
        if tx
            .query_one(statement(
                "SELECT 1 FROM workspace_members WHERE workspace_id=$1 AND user_id=$2 FOR SHARE",
                vec![workspace.into(), owner.into()],
            ))
            .await?
            .is_none()
        {
            return Err(ApiError::Conflict(
                "submitting owner is no longer a project member".into(),
            ));
        }
        let package = install_projection(&tx, project, source, actor).await?;
        json!({"status":"attached","package":package})
    } else {
        set_status(&tx, project, source, "rejected", actor).await?;
        json!({"status":"rejected"})
    };
    tx.commit().await?;
    Ok(ApiResponse::success(response))
}

pub async fn revoke(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    Path((project, source)): Path<(Uuid, Uuid)>,
) -> Result<impl IntoResponse, ApiError> {
    enabled()?;
    let actor = user(&claims)?;
    let tx = state.db.begin().await?;
    let owns_link = tx
        .query_one(statement(
            "SELECT 1 FROM form_plugin_library_requests r JOIN form_plugin_library s ON s.id=r.source_id
          WHERE r.project_id=$1 AND r.source_id=$2 AND s.owner_id=$3",
            vec![project.into(), source.into(), actor.into()],
        ))
        .await?
        .is_some();
    if !owns_link && !role_is_form_admin(&member_role(&tx, project, actor).await?) {
        return Err(ApiError::forbidden(
            "only the release owner or project administrators can revoke a plugin link",
        ));
    }
    lock_project(&tx, project).await?;
    tx.query_one(statement(
        "UPDATE form_plugin_library_requests SET status='revoked',decided_by=$3,decided_at=clock_timestamp()
          WHERE project_id=$1 AND source_id=$2 RETURNING source_id",
        vec![project.into(), source.into(), actor.into()],
    ))
    .await?
    .ok_or_else(|| ApiError::not_found("plugin link not found"))?;
    if let Some(row) = tx
        .query_one(statement(
            "UPDATE form_plugin_packages SET library_revoked_at=clock_timestamp()
          WHERE project_id=$1 AND library_source_id=$2 AND library_revoked_at IS NULL RETURNING id,key",
            vec![project.into(), source.into()],
        ))
        .await?
    {
        let key = row.try_get::<String>("", "key")?;
        let projected = row.try_get::<Uuid>("", "id")?;
        let disabled = tx
            .query_one(statement(
                "UPDATE form_plugin_bindings SET package_id=NULL,grants='{}',generation=generation+1
              WHERE project_id=$1 AND key=$2 AND package_id=$3 RETURNING generation",
                vec![project.into(), key.clone().into(), projected.into()],
            ))
            .await?;
        if disabled.is_some() {
            let binding = lock_binding(&tx, project, &key).await?;
            let response = serde_json::to_value(binding).map_err(|_| ApiError::Internal)?;
            tx.execute(statement(
                "INSERT INTO form_plugin_binding_changes (project_id,key,request_id,actor_id,request,response)
                 VALUES ($1,$2,$3,$4,$5,$6)",
                vec![
                    project.into(),
                    key.into(),
                    Uuid::new_v4().into(),
                    actor.into(),
                    json!({"reason":"library_link_revoked","source_id":source}).into(),
                    response.into(),
                ],
            ))
            .await?;
        }
    }
    tx.commit().await?;
    Ok(ApiResponse::success(json!({"revoked":true})))
}
