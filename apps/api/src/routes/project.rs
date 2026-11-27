// Public and framework-facing signatures remain stable during this behavior-neutral cleanup.
// Local SQL row types stay beside the queries whose column shapes they mirror.
#![allow(
    clippy::items_after_statements,
    clippy::needless_pass_by_value,
    clippy::similar_names
)]

use axum::{
    Extension, Json,
    extract::{Path, Query, State},
    http::HeaderMap,
    response::IntoResponse,
};
use platform::{app::AppState, auth::JwtClaims};
use sea_orm::{ConnectionTrait, DbBackend, FromQueryResult, Statement, TransactionTrait};
use serde::{Deserialize, Serialize};
use serde_json::{Value as JsonValue, json};
use std::collections::HashMap;
use uuid::Uuid;

use crate::{
    error::ApiError,
    events::{BusinessEventInput, insert_business_event},
    extensions::install::{access, approve, template},
    middleware::bot_auth::{BotAuthContext, require_workspace_access, require_workspace_access_from_auth},
    response::{ApiResponse, PaginatedData},
    routes::decision_domain,
    webhook_trigger::{TriggerContext, WebhookEvent, trigger_webhooks},
};

#[derive(Debug, Serialize)]
pub struct ProjectResponse {
    pub id: Uuid,
    pub workspace_id: Uuid,
    pub key: String,
    pub name: String,
    pub description: String,
    pub type_key: String,
    pub type_settings: JsonValue,
    pub created_at: String,
    pub updated_at: String,
    pub issue_counts: Option<IssueCounts>,
}

#[derive(Debug, Serialize, Clone, Default)]
pub struct IssueCounts {
    pub by_state: HashMap<String, i64>,
    pub total: i64,
}

#[derive(Debug, Deserialize)]
pub struct CreateProjectRequest {
    pub key: String,
    pub name: String,
    pub description: Option<String>,
    pub type_key: Option<String>,
    pub type_settings: Option<JsonValue>,
    /// Package key of a built-in template package (for example `sv/contract-review`) installed into the
    /// new project. Needs a workspace owner or admin.
    pub template_package: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct UpdateProjectRequest {
    pub name: Option<String>,
    pub key: Option<String>,
    pub description: Option<String>,
    pub type_key: Option<String>,
    pub type_settings: Option<JsonValue>,
    pub expected_updated_at: Option<platform::timestamp::Timestamp>,
}

async fn ensure_project_type_exists(state: &AppState, type_key: &str) -> Result<(), ApiError> {
    let exists = state
        .db
        .query_one(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT 1 FROM project_types WHERE key = $1",
            vec![type_key.to_string().into()],
        ))
        .await?
        .is_some();

    if exists {
        Ok(())
    } else {
        Err(ApiError::BadRequest("project type does not exist".to_string()))
    }
}

/// POST /`api/v1/workspaces/:workspace_id/projects` - Create a new project
pub async fn create_project(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    Path(workspace_id): Path<Uuid>,
    headers: HeaderMap,
    Json(req): Json<CreateProjectRequest>,
) -> Result<impl IntoResponse, ApiError> {
    let user_id = Uuid::parse_str(&claims.sub).map_err(|_| ApiError::Unauthorized("invalid user id".to_string()))?;

    if req.key.trim().is_empty() || req.name.trim().is_empty() {
        return Err(ApiError::BadRequest("key and name are required".to_string()));
    }

    // Validate key format (uppercase letters only)
    if !req.key.chars().all(|c| c.is_ascii_uppercase() || c.is_ascii_digit()) {
        return Err(ApiError::BadRequest(
            "key must contain only uppercase letters and digits".to_string(),
        ));
    }

    // Check workspace membership
    let _role = get_workspace_role(&state, workspace_id, user_id).await?;

    // Check if key already exists in workspace
    let existing = state
        .db
        .query_one(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT id FROM projects WHERE workspace_id = $1 AND key = $2",
            vec![workspace_id.into(), req.key.clone().into()],
        ))
        .await?;

    if existing.is_some() {
        return Err(ApiError::Conflict(
            "project key already exists in this workspace".to_string(),
        ));
    }

    let project_id = Uuid::new_v4();
    let now = platform::timestamp::Timestamp::now();
    let description = req.description.unwrap_or_default();
    // A template package goes through the installation flow, which only a person who owns or
    // administers the workspace uses; everything is refused before anything is written.
    let chosen = match req.template_package.as_deref() {
        Some(key) => {
            access::admin_user(&state, &claims, workspace_id).await?;
            Some(template::prepare(&state, workspace_id, key.trim()).await?)
        }
        None => None,
    };
    // The caller's type wins over the package's; a template without one makes a form project.
    let type_key = req.type_key.clone().unwrap_or_else(|| {
        chosen
            .as_ref()
            .map_or("code_project", |prepared| prepared.type_key().unwrap_or("custom_form"))
            .to_string()
    });
    ensure_project_type_exists(&state, &type_key).await?;
    let mut type_settings = req.type_settings.unwrap_or_else(|| json!({}));
    let Some(settings) = type_settings.as_object_mut() else {
        return Err(ApiError::BadRequest("type_settings must be a JSON object".to_string()));
    };
    let template_package = chosen.as_ref().map(|prepared| {
        let (key, version) = prepared.package();
        json!({ "key": key, "version": version })
    });
    if let Some(package) = &template_package {
        settings.insert("template_package".to_string(), package.clone());
    }

    let tx = state.db.begin().await?;
    tx.execute(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "INSERT INTO projects (id, workspace_id, key, name, description, type_key, type_settings, workflow_id, created_by, created_at, updated_at) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11)",
        vec![
            project_id.into(),
            workspace_id.into(),
            req.key.clone().into(),
            req.name.clone().into(),
            description.clone().into(),
            type_key.clone().into(),
            type_settings.clone().into(),
            Option::<Uuid>::None.into(),
            user_id.into(),
            now.into(),
            now.into(),
        ],
    ))
    .await?;
    insert_business_event(
        &tx,
        BusinessEventInput {
            workspace_id,
            project_id: Some(project_id),
            event_type: "project.created".to_string(),
            aggregate_type: "project".to_string(),
            aggregate_id: project_id.to_string(),
            actor_id: Some(user_id),
            source: json!({ "type": "user", "actor_id": user_id }),
            payload: json!({
                "project_id": project_id,
                "workspace_id": workspace_id,
                "key": req.key,
                "name": req.name,
                "description": description,
                "type_key": type_key,
                "type_settings": type_settings,
                "template_package": template_package
            }),
            metadata: json!({
                "template_package": template_package,
                "template_initialized": template_package.is_some()
            }),
            correlation_id: None,
            causation_id: None,
            idempotency_key: None,
        },
    )
    .await?;
    if let Some(prepared) = &chosen {
        // Workflow, labels, installation, forms and pages share the project's transaction: a failure
        // anywhere leaves no project behind.
        let locale = preferred_locale(&headers);
        let outcome = async {
            template::apply_project_block(&tx, prepared, workspace_id, project_id, &req.name, &req.key, &locale)
                .await?;
            template::install_prepared_in(&state, &tx, user_id, workspace_id, project_id, prepared).await
        }
        .await;
        approve::refresh_rules(&state).await;
        outcome?;
    }
    tx.commit().await?;

    decision_domain::initialize_default_domains_for_project(&state, project_id).await?;

    trigger_webhooks(
        state.clone(),
        TriggerContext {
            event: WebhookEvent::ProjectCreated,
            workspace_id,
            project_id,
            actor_id: user_id,
            issue_id: None,
            comment_id: None,
            label_id: None,
            sprint_id: None,
            changes: None,
            mentions: Vec::new(),
            extra_data: Some(serde_json::json!({
                "project": {
                    "id": project_id,
                    "workspace_id": workspace_id,
                    "key": req.key,
                    "name": req.name,
                    "description": description,
                    "type_key": type_key.clone(),
                    "type_settings": type_settings.clone(),
                    "template_package": template_package,
                    "created_at": now.to_rfc3339(),
                    "updated_at": now.to_rfc3339(),
                }
            })),
        },
    );

    Ok(ApiResponse::success(ProjectResponse {
        id: project_id,
        workspace_id,
        key: req.key,
        name: req.name,
        description,
        type_key,
        type_settings,
        created_at: now.to_rfc3339(),
        updated_at: now.to_rfc3339(),
        issue_counts: None,
    }))
}

/// A template package to install into an existing project.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InstallTemplateRequest {
    /// Package key of a built-in template package, for example `sv/contract-review`.
    pub package_key: String,
}

/// POST /api/v1/projects/:project_id/template-install
///
/// Installs a built-in template package (forms, layouts, site pages; no code) into the project and
/// approves it at once. Owners and admins of the workspace only; a package with code or capabilities
/// goes through the installation review instead.
pub async fn install_project_template(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    Path(project_id): Path<Uuid>,
    Json(req): Json<InstallTemplateRequest>,
) -> Result<impl IntoResponse, ApiError> {
    #[derive(Debug, FromQueryResult)]
    struct ProjectScope {
        workspace_id: Uuid,
    }
    let project = ProjectScope::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "SELECT workspace_id FROM projects WHERE id = $1",
        vec![project_id.into()],
    ))
    .one(&state.db)
    .await?
    .ok_or_else(|| ApiError::NotFound("project not found".to_string()))?;
    let admin = access::admin_user(&state, &claims, project.workspace_id).await?;
    let installed =
        template::install_builtin(&state, admin, project.workspace_id, project_id, req.package_key.trim()).await?;
    Ok(ApiResponse::success(json!({
        "installation_id": installed.installation_id,
        "package_key": installed.package_key,
        "version": installed.version,
        "forms": installed
            .forms
            .iter()
            .map(|(bind, form_id)| json!({ "bind": bind, "form_id": form_id }))
            .collect::<Vec<_>>(),
    })))
}

#[derive(Debug, Deserialize)]
pub struct TemplatePreviewQuery {
    pub package_key: String,
    pub type_key: Option<String>,
}

/// GET /api/v1/workspaces/:workspace_id/project-templates/preview
///
/// What creating a project from a built-in template package would apply (type, workflow states,
/// labels, forms, pages). Writes nothing; a private or unknown package is `not_found`.
pub async fn preview_project_template(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    bot: Option<Extension<BotAuthContext>>,
    Path(workspace_id): Path<Uuid>,
    headers: HeaderMap,
    Query(query): Query<TemplatePreviewQuery>,
) -> Result<impl IntoResponse, ApiError> {
    let bot = bot.map(|Extension(context)| context);
    let (_, role, is_bot) = require_workspace_access_from_auth(&state, &claims, bot.as_ref(), workspace_id)
        .await
        .map_err(|_| ApiError::not_found("workspace not found"))?;
    let type_key = query.type_key.as_deref().map(str::trim).filter(|key| !key.is_empty());
    if let Some(key) = type_key {
        let grammar = key.len() <= 48
            && key.starts_with(|c: char| c.is_ascii_lowercase())
            && key
                .chars()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_');
        if !grammar {
            return Err(ApiError::BadRequest("type_key is not a project type key".to_string()));
        }
    }
    let auto_approve = !is_bot && matches!(role.as_str(), "owner" | "admin");
    let preview = template::preview_create(
        &state,
        workspace_id,
        query.package_key.trim(),
        type_key,
        auto_approve,
        &preferred_locale(&headers),
    )
    .await?;
    Ok(ApiResponse::success(preview))
}

/// The primary language of the first `Accept-Language` entry, lower-case, or `en`.
fn preferred_locale(headers: &HeaderMap) -> String {
    headers
        .get(axum::http::header::ACCEPT_LANGUAGE)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.split([',', ';', '-']).next())
        .map(|tag| tag.trim().to_ascii_lowercase())
        .filter(|tag| (2..=3).contains(&tag.len()) && tag.bytes().all(|b| b.is_ascii_lowercase()))
        .unwrap_or_else(|| "en".to_string())
}

/// `page` / `per_page` for [`list_projects`]. Both optional: a request carrying neither keeps the
/// historical "every project in one page" response, so callers that never paginated see no change.
#[derive(Debug, Default, Deserialize)]
pub struct ListProjectsQuery {
    pub page: Option<i64>,
    pub per_page: Option<i64>,
}

const LIST_PROJECTS_DEFAULT_PER_PAGE: i64 = 20;
const LIST_PROJECTS_MAX_PER_PAGE: i64 = 100;

/// The `(page, per_page)` window a [`ListProjectsQuery`] asks for, or `None` for the unpaginated
/// listing. Out-of-range values are clamped the same way `routes::my` clamps its lists.
fn list_projects_window(query: &ListProjectsQuery) -> Option<(i64, i64)> {
    if query.page.is_none() && query.per_page.is_none() {
        return None;
    }
    let page = query.page.unwrap_or(1).max(1);
    let per_page = query
        .per_page
        .unwrap_or(LIST_PROJECTS_DEFAULT_PER_PAGE)
        .clamp(1, LIST_PROJECTS_MAX_PER_PAGE);
    Some((page, per_page))
}

/// GET /`api/v1/workspaces/:workspace_id/projects` - List projects in workspace
pub async fn list_projects(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    bot: Option<Extension<BotAuthContext>>,
    Path(workspace_id): Path<Uuid>,
    Query(query): Query<ListProjectsQuery>,
) -> Result<impl IntoResponse, ApiError> {
    let mut extensions = axum::http::Extensions::new();
    extensions.insert(claims);
    if let Some(Extension(bot_ctx)) = bot {
        extensions.insert(bot_ctx);
    }

    // Check workspace membership
    require_workspace_access(&state, &extensions, workspace_id).await?;

    #[derive(Debug, sea_orm::FromQueryResult)]
    struct ProjectRow {
        id: Uuid,
        workspace_id: Uuid,
        key: String,
        name: String,
        description: String,
        type_key: Option<String>,
        type_settings: JsonValue,
        created_at: platform::timestamp::Timestamp,
        updated_at: platform::timestamp::Timestamp,
    }

    // A connected application sees the projects of its grant only: a grant limited to some
    // projects adds the id filter to every statement that reads or counts the list.
    let oauth_projects: Option<Vec<Uuid>> =
        crate::oauth::delegated::current().and_then(|context| match &context.projects {
            crate::oauth::scopes::ProjectScope::All => None,
            crate::oauth::scopes::ProjectScope::Selected(ids) => Some(ids.to_vec()),
        });
    // A scoped bot sees the projects it can read (a runner bot: where it has an enabled connection).
    let bot_projects = match extensions.get::<BotAuthContext>() {
        Some(bot_ctx) => crate::middleware::scoped_bot::readable_projects(&state.db, bot_ctx.bot_id)
            .await?
            .ids(),
        None => None,
    };
    let granted_projects: Option<Vec<Uuid>> = match (oauth_projects, bot_projects) {
        (Some(oauth), Some(bot)) => Some(oauth.into_iter().filter(|id| bot.contains(id)).collect()),
        (Some(ids), None) | (None, Some(ids)) => Some(ids),
        (None, None) => None,
    };
    let project_filter = if granted_projects.is_some() {
        " AND id = ANY($2)"
    } else {
        ""
    };
    let filter_values = |mut values: Vec<sea_orm::Value>| {
        if let Some(ids) = &granted_projects {
            values.insert(1, ids.clone().into());
        }
        values
    };
    let (limit_param, offset_param) = if granted_projects.is_some() {
        ("$3", "$4")
    } else {
        ("$2", "$3")
    };

    let window = list_projects_window(&query);
    let projects = match window {
        None => {
            ProjectRow::find_by_statement(Statement::from_sql_and_values(
                DbBackend::Postgres,
                format!("SELECT id, workspace_id, key, name, description, type_key, type_settings, created_at, updated_at FROM projects WHERE workspace_id = $1{project_filter} ORDER BY created_at DESC"),
                filter_values(vec![workspace_id.into()]),
            ))
            .all(&state.db)
            .await?
        }
        Some((page, per_page)) => {
            ProjectRow::find_by_statement(Statement::from_sql_and_values(
                DbBackend::Postgres,
                format!("SELECT id, workspace_id, key, name, description, type_key, type_settings, created_at, updated_at FROM projects WHERE workspace_id = $1{project_filter} ORDER BY created_at DESC, id DESC LIMIT {limit_param} OFFSET {offset_param}"),
                filter_values(vec![
                    workspace_id.into(),
                    per_page.into(),
                    page.saturating_sub(1).saturating_mul(per_page).into(),
                ]),
            ))
            .all(&state.db)
            .await?
        }
    };

    #[derive(Debug, FromQueryResult)]
    struct IssueCountRow {
        project_id: Uuid,
        state: String,
        count: i64,
    }

    let project_ids: Vec<Uuid> = projects.iter().map(|project| project.id).collect();
    let mut issue_counts_by_project: HashMap<Uuid, IssueCounts> = HashMap::new();

    if !project_ids.is_empty() {
        let issue_counts = IssueCountRow::find_by_statement(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT project_id, state, COUNT(*) as count FROM work_items WHERE project_id = ANY($1) GROUP BY project_id, state",
            vec![project_ids.into()],
        ))
        .all(&state.db)
        .await?;

        for row in issue_counts {
            let entry = issue_counts_by_project.entry(row.project_id).or_default();
            *entry.by_state.entry(row.state).or_insert(0) += row.count;
            entry.total += row.count;
        }
    }

    let response: Vec<ProjectResponse> = projects
        .into_iter()
        .map(|p| ProjectResponse {
            id: p.id,
            workspace_id: p.workspace_id,
            key: p.key,
            name: p.name,
            description: p.description,
            type_key: p.type_key.unwrap_or_else(|| "code_project".to_string()),
            type_settings: p.type_settings,
            created_at: p.created_at.to_rfc3339(),
            updated_at: p.updated_at.to_rfc3339(),
            issue_counts: Some(issue_counts_by_project.get(&p.id).cloned().unwrap_or_default()),
        })
        .collect();

    let Some((page, per_page)) = window else {
        return Ok(ApiResponse::success(PaginatedData::from_items(response)));
    };
    #[derive(Debug, FromQueryResult)]
    struct CountRow {
        total: i64,
    }
    let total = CountRow::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        format!("SELECT COUNT(*)::bigint AS total FROM projects WHERE workspace_id = $1{project_filter}"),
        filter_values(vec![workspace_id.into()]),
    ))
    .one(&state.db)
    .await?
    .map_or(0, |row| row.total);
    Ok(ApiResponse::success(PaginatedData {
        items: response,
        total,
        page,
        per_page,
        total_pages: if total == 0 {
            1
        } else {
            total / per_page + i64::from(total % per_page != 0)
        },
    }))
}

/// GET /api/v1/projects/:id - Get project details
pub async fn get_project(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    bot: Option<Extension<BotAuthContext>>,
    Path(project_id): Path<Uuid>,
) -> Result<impl IntoResponse, ApiError> {
    let _user_id = Uuid::parse_str(&claims.sub).map_err(|_| ApiError::Unauthorized("invalid user id".to_string()))?;
    let mut extensions = axum::http::Extensions::new();
    extensions.insert(claims);
    if let Some(Extension(bot_ctx)) = bot {
        extensions.insert(bot_ctx);
    }

    #[derive(Debug, sea_orm::FromQueryResult)]
    struct ProjectRow {
        id: Uuid,
        workspace_id: Uuid,
        key: String,
        name: String,
        description: String,
        type_key: Option<String>,
        type_settings: JsonValue,
        created_at: platform::timestamp::Timestamp,
        updated_at: platform::timestamp::Timestamp,
    }

    let project = ProjectRow::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        r"
            SELECT p.id, p.workspace_id, p.key, p.name, p.description, p.type_key, p.type_settings, p.created_at, p.updated_at
            FROM projects p
            WHERE p.id = $1
        ",
        vec![project_id.into()],
    ))
    .one(&state.db)
    .await?
    .ok_or_else(|| ApiError::NotFound("project not found".to_string()))?;

    require_workspace_access(&state, &extensions, project.workspace_id).await?;

    Ok(ApiResponse::success(ProjectResponse {
        id: project.id,
        workspace_id: project.workspace_id,
        key: project.key,
        name: project.name,
        description: project.description,
        type_key: project.type_key.unwrap_or_else(|| "code_project".to_string()),
        type_settings: project.type_settings,
        created_at: project.created_at.to_rfc3339(),
        updated_at: project.updated_at.to_rfc3339(),
        issue_counts: None,
    }))
}

/// PUT /api/v1/projects/:id - Update project
pub async fn update_project(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    Path(project_id): Path<Uuid>,
    Json(req): Json<UpdateProjectRequest>,
) -> Result<impl IntoResponse, ApiError> {
    let user_id = Uuid::parse_str(&claims.sub).map_err(|_| ApiError::Unauthorized("invalid user id".to_string()))?;

    // Get project's workspace and check permission
    #[derive(Debug, sea_orm::FromQueryResult)]
    struct WorkspaceIdRow {
        workspace_id: Uuid,
    }

    let project = WorkspaceIdRow::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "SELECT workspace_id FROM projects WHERE id = $1",
        vec![project_id.into()],
    ))
    .one(&state.db)
    .await?
    .ok_or_else(|| ApiError::NotFound("project not found".to_string()))?;

    let role = get_workspace_role(&state, project.workspace_id, user_id).await?;
    if role == "member" {
        return Err(ApiError::Forbidden(
            "only owners and admins can update projects".to_string(),
        ));
    }

    // Validate key if provided
    if let Some(ref key) = req.key {
        if !key.chars().all(|c| c.is_ascii_uppercase() || c.is_ascii_digit()) {
            return Err(ApiError::BadRequest(
                "key must contain only uppercase letters and digits".to_string(),
            ));
        }

        // Check key uniqueness in workspace
        let existing = state
            .db
            .query_one(Statement::from_sql_and_values(
                DbBackend::Postgres,
                "SELECT id FROM projects WHERE workspace_id = $1 AND key = $2 AND id != $3",
                vec![project.workspace_id.into(), key.clone().into(), project_id.into()],
            ))
            .await?;

        if existing.is_some() {
            return Err(ApiError::Conflict("key already exists in this workspace".to_string()));
        }
    }

    // Build update query dynamically
    let mut updates = Vec::new();
    let mut values: Vec<sea_orm::Value> = Vec::new();
    let mut param_idx = 1;

    if let Some(name) = req.name {
        updates.push(format!("name = ${param_idx}"));
        values.push(name.into());
        param_idx += 1;
    }

    if let Some(key) = req.key {
        updates.push(format!("key = ${param_idx}"));
        values.push(key.into());
        param_idx += 1;
    }

    if let Some(description) = req.description {
        updates.push(format!("description = ${param_idx}"));
        values.push(description.into());
        param_idx += 1;
    }

    if let Some(type_key) = req.type_key {
        ensure_project_type_exists(&state, &type_key).await?;
        updates.push(format!("type_key = ${param_idx}"));
        values.push(type_key.into());
        param_idx += 1;
    }

    if let Some(type_settings) = req.type_settings {
        updates.push(format!("type_settings = ${param_idx}"));
        values.push(type_settings.into());
        param_idx += 1;
    }

    if updates.is_empty() {
        return Err(ApiError::BadRequest("no fields to update".to_string()));
    }

    updates.push("updated_at = GREATEST(clock_timestamp(), updated_at + interval '1 microsecond')".into());

    values.push(project_id.into());

    let mut query = format!("UPDATE projects SET {} WHERE id = ${}", updates.join(", "), param_idx);
    if let Some(expected) = req.expected_updated_at {
        param_idx += 1;
        std::fmt::Write::write_fmt(&mut query, format_args!(" AND updated_at=${param_idx}"))
            .map_err(|_| ApiError::Internal)?;
        values.push(expected.into());
    }
    let tx = state.db.begin().await?;
    let written = tx
        .execute(Statement::from_sql_and_values(DbBackend::Postgres, &query, values))
        .await?;
    if written.rows_affected() != 1 {
        return Err(ApiError::VersionConflict("project_version_conflict"));
    }

    // Fetch updated project
    #[derive(Debug, sea_orm::FromQueryResult)]
    struct ProjectRow {
        id: Uuid,
        workspace_id: Uuid,
        key: String,
        name: String,
        description: String,
        type_key: Option<String>,
        type_settings: JsonValue,
        created_at: platform::timestamp::Timestamp,
        updated_at: platform::timestamp::Timestamp,
    }

    let updated = ProjectRow::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "SELECT id, workspace_id, key, name, description, type_key, type_settings, created_at, updated_at FROM projects WHERE id = $1",
        vec![project_id.into()],
    ))
    .one(&tx)
    .await?
    .ok_or_else(|| ApiError::Internal)?;
    insert_business_event(
        &tx,
        BusinessEventInput {
            workspace_id: updated.workspace_id,
            project_id: Some(updated.id),
            event_type: "project.updated".to_string(),
            aggregate_type: "project".to_string(),
            aggregate_id: updated.id.to_string(),
            actor_id: Some(user_id),
            source: json!({ "type": "user", "actor_id": user_id }),
            payload: json!({
                "project_id": updated.id,
                "workspace_id": updated.workspace_id,
                "key": updated.key,
                "name": updated.name,
                "description": updated.description,
                "type_key": updated.type_key.clone(),
                "type_settings": updated.type_settings.clone(),
                "updated_at": updated.updated_at.to_rfc3339()
            }),
            metadata: json!({
                "project_id": updated.id,
                "project_key": updated.key
            }),
            correlation_id: None,
            causation_id: None,
            idempotency_key: None,
        },
    )
    .await?;
    tx.commit().await?;

    trigger_webhooks(
        state.clone(),
        TriggerContext {
            event: WebhookEvent::ProjectUpdated,
            workspace_id: updated.workspace_id,
            project_id: updated.id,
            actor_id: user_id,
            issue_id: None,
            comment_id: None,
            label_id: None,
            sprint_id: None,
            changes: None,
            mentions: Vec::new(),
            extra_data: Some(serde_json::json!({
                "project": {
                    "id": updated.id,
                    "workspace_id": updated.workspace_id,
                    "key": updated.key,
                    "name": updated.name,
                    "description": updated.description,
                    "type_key": updated.type_key.clone(),
                    "type_settings": updated.type_settings.clone(),
                    "created_at": updated.created_at.to_rfc3339(),
                    "updated_at": updated.updated_at.to_rfc3339(),
                }
            })),
        },
    );

    Ok(ApiResponse::success(ProjectResponse {
        id: updated.id,
        workspace_id: updated.workspace_id,
        key: updated.key,
        name: updated.name,
        description: updated.description,
        type_key: updated.type_key.unwrap_or_else(|| "code_project".to_string()),
        type_settings: updated.type_settings,
        created_at: updated.created_at.to_rfc3339(),
        updated_at: updated.updated_at.to_rfc3339(),
        issue_counts: None,
    }))
}

/// Removes the project's blocking content, then the project itself, inside the caller's transaction.
async fn delete_project_row(
    tx: &sea_orm::DatabaseTransaction,
    project_id: Uuid,
) -> Result<Option<(Uuid, i64)>, ApiError> {
    let released = crate::services::teardown::delete_project_content(tx, project_id).await?;
    tx.execute(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "DELETE FROM projects WHERE id = $1",
        vec![project_id.into()],
    ))
    .await?;
    Ok(released)
}

/// DELETE /api/v1/projects/:id - Delete project
pub async fn delete_project(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    Path(project_id): Path<Uuid>,
) -> Result<impl IntoResponse, ApiError> {
    let user_id = Uuid::parse_str(&claims.sub).map_err(|_| ApiError::Unauthorized("invalid user id".to_string()))?;

    // Get project's workspace and check permission
    #[derive(Debug, sea_orm::FromQueryResult)]
    struct ProjectDeleteRow {
        workspace_id: Uuid,
        key: String,
        name: String,
        description: String,
        type_key: Option<String>,
        type_settings: JsonValue,
    }

    let project = ProjectDeleteRow::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "SELECT workspace_id, key, name, description, type_key, type_settings FROM projects WHERE id = $1",
        vec![project_id.into()],
    ))
    .one(&state.db)
    .await?
    .ok_or_else(|| ApiError::NotFound("project not found".to_string()))?;

    let role = get_workspace_role(&state, project.workspace_id, user_id).await?;
    if role != "owner" && role != "admin" {
        return Err(ApiError::Forbidden(
            "only owners and admins can delete projects".to_string(),
        ));
    }

    let tx = state.db.begin().await?;
    crate::flow::scope_mounts::fence_destination_delete(&tx, project.workspace_id, Some(project_id)).await?;
    let current = tx
        .query_one(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT role FROM workspace_members WHERE workspace_id=$1 AND user_id=$2",
            vec![project.workspace_id.into(), user_id.into()],
        ))
        .await?;
    if !current.is_some_and(|row| {
        row.try_get::<String>("", "role")
            .is_ok_and(|role| matches!(role.as_str(), "owner" | "admin"))
    }) {
        return Err(ApiError::Forbidden(
            "only owners and admins can delete projects".to_string(),
        ));
    }

    insert_business_event(
        &tx,
        BusinessEventInput {
            workspace_id: project.workspace_id,
            project_id: None,
            event_type: "project.deleted".to_string(),
            aggregate_type: "project".to_string(),
            aggregate_id: project_id.to_string(),
            actor_id: Some(user_id),
            source: json!({ "type": "user", "actor_id": user_id }),
            payload: json!({
                "project_id": project_id,
                "workspace_id": project.workspace_id,
                "key": project.key.clone(),
                "name": project.name.clone(),
                "description": project.description.clone(),
                "type_key": project.type_key.clone(),
                "type_settings": project.type_settings.clone(),
                "status": "deleted"
            }),
            metadata: json!({
                "project_id": project_id,
                "project_key": project.key
            }),
            correlation_id: None,
            causation_id: None,
            idempotency_key: None,
        },
    )
    .await?;
    let released = delete_project_row(&tx, project_id)
        .await
        .map_err(|err| crate::services::teardown::refuse_blocked(err, "project"))?;
    tx.commit().await?;
    // A connected bot's seat left with its last project: drop its cached permissions and sessions.
    if let Some((workspace_id, epoch)) = released {
        crate::services::bot_connections::settle_epoch(&state, workspace_id, Some(epoch)).await;
    }

    trigger_webhooks(
        state.clone(),
        TriggerContext {
            event: WebhookEvent::ProjectDeleted,
            workspace_id: project.workspace_id,
            project_id,
            actor_id: user_id,
            issue_id: None,
            comment_id: None,
            label_id: None,
            sprint_id: None,
            changes: None,
            mentions: Vec::new(),
            extra_data: Some(serde_json::json!({
                "project": {
                    "id": project_id,
                    "workspace_id": project.workspace_id,
                    "status": "deleted"
                }
            })),
        },
    );

    Ok(ApiResponse::ok())
}

/// Helper: Get user's role in workspace
async fn get_workspace_role(state: &AppState, workspace_id: Uuid, user_id: Uuid) -> Result<String, ApiError> {
    #[derive(Debug, sea_orm::FromQueryResult)]
    struct RoleRow {
        role: String,
    }

    let row = RoleRow::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "SELECT role FROM workspace_members WHERE workspace_id = $1 AND user_id = $2",
        vec![workspace_id.into(), user_id.into()],
    ))
    .one(&state.db)
    .await?
    .ok_or_else(|| ApiError::NotFound("workspace not found or access denied".to_string()))?;

    Ok(row.role)
}

#[cfg(test)]
mod tests {
    use axum::http::{HeaderMap, HeaderValue, header::ACCEPT_LANGUAGE};

    use super::preferred_locale;

    fn locale_of(value: Option<&str>) -> String {
        let mut headers = HeaderMap::new();
        if let Some(value) = value {
            headers.insert(ACCEPT_LANGUAGE, HeaderValue::from_str(value).expect("header"));
        }
        preferred_locale(&headers)
    }

    #[test]
    fn the_template_locale_is_the_primary_language_or_english() {
        assert_eq!(locale_of(None), "en");
        assert_eq!(locale_of(Some("zh-CN,zh;q=0.9,en;q=0.8")), "zh");
        assert_eq!(locale_of(Some("EN-us")), "en");
        assert_eq!(locale_of(Some("*")), "en");
        assert_eq!(locale_of(Some("")), "en");
        assert_eq!(locale_of(Some("x1")), "en");
    }
}
