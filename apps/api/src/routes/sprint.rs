// Domain-specific local names remain explicit for audit readability.
// Local SQL row types stay beside the queries whose column shapes they mirror.
#![allow(clippy::items_after_statements, clippy::similar_names)]

use crate::middleware::bot_auth::{BotAuthContext, require_workspace_access};
use axum::{
    Extension, Json,
    extract::{Path, State},
    response::IntoResponse,
};
use platform::{app::AppState, auth::JwtClaims, timestamp::Timestamp};
use sea_orm::{ConnectionTrait, DbBackend, FromQueryResult, Statement, TransactionTrait};
use serde::{Deserialize, Serialize};
use time::Date;
use uuid::Uuid;

use crate::{
    entities::ISO_DATE_FORMAT,
    error::ApiError,
    request::nullable,
    response::{ApiResponse, PaginatedData},
    services::{
        sprint_moves::{MoveScope, emit_moves, lock_all_items, lock_unfinished_items, move_items},
        workflow_service::{finished_state_keys, resolve_effective_workflow_for_project},
    },
    webhook_trigger::{TriggerContext, WebhookEvent, trigger_webhooks},
};

#[derive(Debug, Serialize)]
pub struct SprintResponse {
    pub id: Uuid,
    pub project_id: Uuid,
    pub name: String,
    pub description: String,
    pub start_date: Option<String>,
    pub end_date: Option<String>,
    pub status: String,
    pub created_at: String,
    pub updated_at: String,
    /// Only on the answer that completed or cancelled the sprint.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rollover: Option<RolloverSummary>,
}

/// What completing or cancelling a sprint did with its unfinished work items.
#[derive(Debug, Serialize)]
pub struct RolloverSummary {
    pub moved_count: i64,
    /// The planned sprint the items went to; `null` is the backlog.
    pub target_sprint_id: Option<Uuid>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CreateSprintRequest {
    pub name: String,
    pub description: Option<String>,
    pub start_date: Option<String>,
    pub end_date: Option<String>,
    pub status: Option<String>,
}

/// `PUT /sprints/{id}`: a key that is absent leaves the field alone. `start_date` and `end_date`
/// also accept `null`, which clears them; `null` on any other field is refused.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
#[allow(
    clippy::option_option,
    reason = "absent, null and a value are three different requests"
)]
pub struct UpdateSprintRequest {
    #[serde(default, deserialize_with = "nullable")]
    pub name: Option<Option<String>>,
    #[serde(default, deserialize_with = "nullable")]
    pub description: Option<Option<String>>,
    #[serde(default, deserialize_with = "nullable")]
    pub start_date: Option<Option<String>>,
    #[serde(default, deserialize_with = "nullable")]
    pub end_date: Option<Option<String>>,
    #[serde(default, deserialize_with = "nullable")]
    pub status: Option<Option<String>>,
    /// With `completed` or `cancelled`: the planned sprint that takes the unfinished items instead
    /// of the backlog. `null` means the backlog.
    #[serde(default, deserialize_with = "nullable")]
    pub rollover_sprint_id: Option<Option<Uuid>>,
}

/// The value of a field that cannot be cleared: `null` is a request error, not "no change".
#[allow(
    clippy::option_option,
    reason = "absent, null and a value are three different requests"
)]
fn not_null<T>(value: Option<Option<T>>, field: &str) -> Result<Option<T>, ApiError> {
    match value {
        Some(None) => Err(ApiError::BadRequest(format!("{field} cannot be null"))),
        Some(Some(value)) => Ok(Some(value)),
        None => Ok(None),
    }
}

/// Validate sprint status
fn validate_status(status: &str) -> Result<(), ApiError> {
    match status {
        "planned" | "active" | "completed" | "cancelled" => Ok(()),
        _ => Err(ApiError::BadRequest(
            "status must be one of: planned, active, completed, cancelled".to_string(),
        )),
    }
}

/// The lifecycle: `planned -> active -> completed`, and `planned` or `active -> cancelled`.
/// Completed and cancelled are final, and `active -> planned` is refused: telling which items were
/// finished during the sprint would need its start time and each item's state history.
fn transition_allowed(from: &str, to: &str) -> bool {
    matches!(
        (from, to),
        ("planned", "active" | "cancelled") | ("active", "completed" | "cancelled")
    )
}

fn parse_date(raw: &str, field: &str) -> Result<Date, ApiError> {
    Date::parse(raw, ISO_DATE_FORMAT)
        .map_err(|_| ApiError::BadRequest(format!("invalid {field} format (use YYYY-MM-DD)")))
}

/// An end date before the start date is refused; either side missing is fine.
fn check_date_order(start: Option<Date>, end: Option<Date>) -> Result<(), ApiError> {
    if let (Some(start), Some(end)) = (start, end)
        && end < start
    {
        return Err(ApiError::BadRequest(
            "end_date must not be before start_date".to_string(),
        ));
    }
    Ok(())
}

/// Maps the unique violations of the sprint rules to the 409 they stand for; the check before
/// each write answers the common case with the names, these catch two requests racing.
fn rule_violation(err: sea_orm::DbErr) -> ApiError {
    let text = err.to_string();
    if text.contains("sprints_one_active_per_project") {
        ApiError::Conflict("project already has an active sprint".to_string())
    } else if text.contains("sprints_project_lower_name_key") {
        ApiError::Conflict("sprint with this name already exists in project".to_string())
    } else {
        ApiError::from(err)
    }
}

fn build_auth_extensions(claims: JwtClaims, bot: Option<Extension<BotAuthContext>>) -> axum::http::Extensions {
    let mut extensions = axum::http::Extensions::new();
    extensions.insert(claims);
    if let Some(Extension(bot_ctx)) = bot {
        extensions.insert(bot_ctx);
    }
    extensions
}

#[derive(Debug, FromQueryResult)]
struct SprintRow {
    id: Uuid,
    project_id: Uuid,
    name: String,
    description: String,
    start_date: Option<Date>,
    end_date: Option<Date>,
    status: String,
    created_at: Timestamp,
    updated_at: Timestamp,
}

const SPRINT_COLUMNS: &str = "id, project_id, name, description, start_date, end_date, status, created_at, updated_at";

impl From<SprintRow> for SprintResponse {
    fn from(row: SprintRow) -> Self {
        Self {
            id: row.id,
            project_id: row.project_id,
            name: row.name,
            description: row.description,
            start_date: row.start_date.map(|d| d.to_string()),
            end_date: row.end_date.map(|d| d.to_string()),
            status: row.status,
            created_at: row.created_at.to_rfc3339(),
            updated_at: row.updated_at.to_rfc3339(),
            rollover: None,
        }
    }
}

async fn load_sprint(db: &impl ConnectionTrait, sprint_id: Uuid) -> Result<Option<SprintRow>, ApiError> {
    Ok(SprintRow::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        format!("SELECT {SPRINT_COLUMNS} FROM sprints WHERE id = $1"),
        vec![sprint_id.into()],
    ))
    .one(db)
    .await?)
}

/// 409 when the project already has an active sprint other than `except`, naming it.
async fn ensure_no_other_active_sprint(
    db: &impl ConnectionTrait,
    project_id: Uuid,
    except: Option<Uuid>,
) -> Result<(), ApiError> {
    #[derive(Debug, FromQueryResult)]
    struct Active {
        id: Uuid,
        name: String,
    }
    let active = Active::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "SELECT id, name FROM sprints WHERE project_id = $1 AND status = 'active' AND ($2::uuid IS NULL OR id <> $2) FOR UPDATE",
        vec![project_id.into(), except.into()],
    ))
    .one(db)
    .await?;
    match active {
        Some(active) => Err(ApiError::Conflict(format!(
            "project already has an active sprint: {} ({})",
            active.name, active.id
        ))),
        None => Ok(()),
    }
}

/// 409 when another sprint of the project has this name, ignoring case.
async fn ensure_name_free(
    db: &impl ConnectionTrait,
    project_id: Uuid,
    name: &str,
    except: Option<Uuid>,
) -> Result<(), ApiError> {
    let existing = db
        .query_one(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT id FROM sprints WHERE project_id = $1 AND lower(name) = lower($2) AND ($3::uuid IS NULL OR id <> $3)",
            vec![project_id.into(), name.to_string().into(), except.into()],
        ))
        .await?;
    if existing.is_some() {
        return Err(ApiError::Conflict(
            "sprint with this name already exists in project".to_string(),
        ));
    }
    Ok(())
}

/// POST /`api/v1/projects/:project_id/sprints` - Create sprint
pub async fn create_sprint(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    bot: Option<Extension<BotAuthContext>>,
    Path(project_id): Path<Uuid>,
    Json(req): Json<CreateSprintRequest>,
) -> Result<impl IntoResponse, ApiError> {
    let _user_id = Uuid::parse_str(&claims.sub).map_err(|_| ApiError::Unauthorized("invalid user id".to_string()))?;
    let extensions = build_auth_extensions(claims, bot);

    if req.name.trim().is_empty() {
        return Err(ApiError::BadRequest("name is required".to_string()));
    }

    #[derive(Debug, FromQueryResult)]
    struct ProjectWorkspace {
        workspace_id: Uuid,
    }

    let project = ProjectWorkspace::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "SELECT workspace_id FROM projects WHERE id = $1",
        vec![project_id.into()],
    ))
    .one(&state.db)
    .await?
    .ok_or_else(|| ApiError::NotFound("project not found".to_string()))?;

    let (_, role, _) = require_workspace_access(&state, &extensions, project.workspace_id).await?;

    let sprint_status = req.status.unwrap_or_else(|| "planned".to_string());
    if !matches!(sprint_status.as_str(), "planned" | "active") {
        return Err(ApiError::BadRequest(
            "a new sprint can only be planned or active".to_string(),
        ));
    }
    // Starting is a status change, so creating an active sprint needs the same role.
    if sprint_status == "active" && !matches!(role.as_str(), "owner" | "admin") {
        return Err(ApiError::Forbidden(
            "only owners and admins can change a sprint's status".to_string(),
        ));
    }

    let start_date = req
        .start_date
        .as_deref()
        .map(|raw| parse_date(raw, "start_date"))
        .transpose()?;
    let end_date = req
        .end_date
        .as_deref()
        .map(|raw| parse_date(raw, "end_date"))
        .transpose()?;
    check_date_order(start_date, end_date)?;

    let sprint_id = Uuid::new_v4();
    let now = Timestamp::now();
    let description = req.description.unwrap_or_default();

    let tx = state.db.begin().await?;
    ensure_name_free(&tx, project_id, &req.name, None).await?;
    if sprint_status == "active" {
        ensure_no_other_active_sprint(&tx, project_id, None).await?;
    }
    tx.execute(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "INSERT INTO sprints (id, project_id, name, description, start_date, end_date, status, created_at, updated_at) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9)",
        vec![
            sprint_id.into(),
            project_id.into(),
            req.name.clone().into(),
            description.clone().into(),
            start_date.into(),
            end_date.into(),
            sprint_status.clone().into(),
            now.into(),
            now.into(),
        ],
    ))
    .await
    .map_err(rule_violation)?;
    tx.commit().await.map_err(rule_violation)?;

    Ok(ApiResponse::success(SprintResponse {
        id: sprint_id,
        project_id,
        name: req.name,
        description,
        start_date: start_date.map(|d| d.to_string()),
        end_date: end_date.map(|d| d.to_string()),
        status: sprint_status,
        created_at: now.to_rfc3339(),
        updated_at: now.to_rfc3339(),
        rollover: None,
    }))
}

/// GET /`api/v1/projects/:project_id/sprints` - List sprints
pub async fn list_sprints(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    bot: Option<Extension<BotAuthContext>>,
    Path(project_id): Path<Uuid>,
) -> Result<impl IntoResponse, ApiError> {
    let _user_id = Uuid::parse_str(&claims.sub).map_err(|_| ApiError::Unauthorized("invalid user id".to_string()))?;
    let extensions = build_auth_extensions(claims, bot);

    #[derive(Debug, FromQueryResult)]
    struct ProjectWorkspace {
        workspace_id: Uuid,
    }

    let project = ProjectWorkspace::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "SELECT workspace_id FROM projects WHERE id = $1",
        vec![project_id.into()],
    ))
    .one(&state.db)
    .await?
    .ok_or_else(|| ApiError::NotFound("project not found".to_string()))?;

    require_workspace_access(&state, &extensions, project.workspace_id).await?;

    let sprints = SprintRow::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        format!("SELECT {SPRINT_COLUMNS} FROM sprints WHERE project_id = $1 ORDER BY start_date DESC NULLS LAST, created_at DESC"),
        vec![project_id.into()],
    ))
    .all(&state.db)
    .await?;

    let response: Vec<SprintResponse> = sprints.into_iter().map(SprintResponse::from).collect();

    Ok(ApiResponse::success(PaginatedData::from_items(response)))
}

/// GET /api/v1/sprints/:id - Get one sprint
pub async fn get_sprint(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    bot: Option<Extension<BotAuthContext>>,
    Path(sprint_id): Path<Uuid>,
) -> Result<impl IntoResponse, ApiError> {
    let extensions = build_auth_extensions(claims, bot);

    #[derive(Debug, FromQueryResult)]
    struct SprintWorkspace {
        workspace_id: Uuid,
    }

    let owner = SprintWorkspace::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "SELECT p.workspace_id FROM sprints s INNER JOIN projects p ON s.project_id = p.id WHERE s.id = $1",
        vec![sprint_id.into()],
    ))
    .one(&state.db)
    .await?
    .ok_or_else(|| ApiError::NotFound("sprint not found".to_string()))?;

    require_workspace_access(&state, &extensions, owner.workspace_id).await?;

    let sprint = load_sprint(&state.db, sprint_id)
        .await?
        .ok_or_else(|| ApiError::NotFound("sprint not found".to_string()))?;
    Ok(ApiResponse::success(SprintResponse::from(sprint)))
}

/// PUT /api/v1/sprints/:id - Update sprint
pub async fn update_sprint(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    bot: Option<Extension<BotAuthContext>>,
    Path(sprint_id): Path<Uuid>,
    Json(req): Json<UpdateSprintRequest>,
) -> Result<impl IntoResponse, ApiError> {
    let user_id = Uuid::parse_str(&claims.sub).map_err(|_| ApiError::Unauthorized("invalid user id".to_string()))?;
    let extensions = build_auth_extensions(claims, bot);

    let UpdateSprintRequest {
        name: requested_name,
        description: requested_description,
        start_date: requested_start,
        end_date: requested_end,
        status: requested_status,
        rollover_sprint_id: requested_rollover,
    } = req;
    let requested_name = not_null(requested_name, "name")?;
    let requested_description = not_null(requested_description, "description")?;
    let requested_status = not_null(requested_status, "status")?;
    if let Some(status) = &requested_status {
        validate_status(status)?;
    }
    if requested_name.is_none()
        && requested_description.is_none()
        && requested_start.is_none()
        && requested_end.is_none()
        && requested_status.is_none()
        && requested_rollover.is_none()
    {
        return Err(ApiError::BadRequest("no fields to update".to_string()));
    }
    if requested_name.as_ref().is_some_and(|name| name.trim().is_empty()) {
        return Err(ApiError::BadRequest("name cannot be empty".to_string()));
    }
    let requested_start = requested_start
        .map(|start| start.map(|raw| parse_date(&raw, "start_date")).transpose())
        .transpose()?;
    let requested_end = requested_end
        .map(|end| end.map(|raw| parse_date(&raw, "end_date")).transpose())
        .transpose()?;

    // The row stays locked until the commit, so two changes of one sprint run one after the other.
    let tx = state.db.begin().await?;

    #[derive(Debug, FromQueryResult)]
    struct Current {
        project_id: Uuid,
        workspace_id: Uuid,
        status: String,
        start_date: Option<Date>,
        end_date: Option<Date>,
    }

    let current = Current::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        r"
            SELECT s.project_id, p.workspace_id, s.status, s.start_date, s.end_date
            FROM sprints s
            INNER JOIN projects p ON s.project_id = p.id
            WHERE s.id = $1
            FOR UPDATE OF s
        ",
        vec![sprint_id.into()],
    ))
    .one(&tx)
    .await?
    .ok_or_else(|| ApiError::NotFound("sprint not found".to_string()))?;

    let (_, role, _) = require_workspace_access(&state, &extensions, current.workspace_id).await?;

    // A rollover target only makes sense together with the status change that ends the sprint.
    let ends_sprint = |status: &Option<String>| matches!(status.as_deref(), Some("completed" | "cancelled"));
    let rollover_target = requested_rollover.flatten();
    if rollover_target.is_some()
        && !(ends_sprint(&requested_status) && requested_status.as_deref() != Some(&current.status))
    {
        return Err(ApiError::BadRequest(
            "rollover_sprint_id is only valid when completing or cancelling a sprint".to_string(),
        ));
    }

    // A status equal to the current one is a no-op: no permission check, no event.
    let status_change = requested_status.filter(|status| *status != current.status);
    if let Some(to) = &status_change {
        if !matches!(role.as_str(), "owner" | "admin") {
            return Err(ApiError::Forbidden(
                "only owners and admins can change a sprint's status".to_string(),
            ));
        }
        if !transition_allowed(&current.status, to) {
            return Err(ApiError::BadRequest(format!(
                "sprint status cannot change from {} to {to}",
                current.status
            )));
        }
        if to == "active" {
            ensure_no_other_active_sprint(&tx, current.project_id, Some(sprint_id)).await?;
        }
    }

    // Ending the sprint: the unfinished items go to the backlog or to the chosen planned sprint,
    // in this same transaction, so a failure leaves the sprint and its items as they were.
    let scope = MoveScope {
        workspace_id: current.workspace_id,
        project_id: current.project_id,
        actor_id: user_id,
    };
    let moved = if status_change
        .as_deref()
        .is_some_and(|to| matches!(to, "completed" | "cancelled"))
    {
        if let Some(target) = rollover_target {
            tx.query_one(Statement::from_sql_and_values(
                DbBackend::Postgres,
                "SELECT id FROM sprints WHERE id = $1 AND project_id = $2 AND status = 'planned' AND id <> $3 FOR UPDATE",
                vec![target.into(), current.project_id.into(), sprint_id.into()],
            ))
            .await?
            .ok_or_else(|| {
                ApiError::BadRequest("rollover sprint must be another planned sprint of the same project".to_string())
            })?;
        }
        let workflow = resolve_effective_workflow_for_project(&state, current.project_id).await?;
        let items = lock_unfinished_items(&tx, sprint_id, &finished_state_keys(&workflow)).await?;
        move_items(&tx, scope, &items, rollover_target, Timestamp::now()).await?;
        Some((items, rollover_target))
    } else {
        None
    };

    check_date_order(
        requested_start.unwrap_or(current.start_date),
        requested_end.unwrap_or(current.end_date),
    )?;

    if let Some(name) = &requested_name {
        ensure_name_free(&tx, current.project_id, name, Some(sprint_id)).await?;
    }

    let mut updates = Vec::new();
    let mut values: Vec<sea_orm::Value> = Vec::new();
    let mut push = |column: &str, value: sea_orm::Value| {
        values.push(value);
        updates.push(format!("{column} = ${}", values.len()));
    };
    if let Some(name) = requested_name {
        push("name", name.into());
    }
    if let Some(description) = requested_description {
        push("description", description.into());
    }
    if let Some(start) = requested_start {
        push("start_date", start.into());
    }
    if let Some(end) = requested_end {
        push("end_date", end.into());
    }
    if let Some(status) = &status_change {
        push("status", status.clone().into());
    }
    push("updated_at", Timestamp::now().into());
    values.push(sprint_id.into());
    let query = format!("UPDATE sprints SET {} WHERE id = ${}", updates.join(", "), values.len());

    tx.execute(Statement::from_sql_and_values(DbBackend::Postgres, &query, values))
        .await
        .map_err(rule_violation)?;

    let updated = load_sprint(&tx, sprint_id).await?.ok_or(ApiError::Internal)?;
    tx.commit().await.map_err(rule_violation)?;

    let rollover = moved.as_ref().map(|(items, target)| RolloverSummary {
        moved_count: i64::try_from(items.len()).unwrap_or(i64::MAX),
        target_sprint_id: *target,
    });

    if let Some(new_status) = status_change {
        let event = match new_status.as_str() {
            "active" => Some(WebhookEvent::SprintStarted),
            "completed" => Some(WebhookEvent::SprintCompleted),
            _ => None,
        };
        if let Some(event) = event {
            trigger_webhooks(
                state.clone(),
                TriggerContext {
                    event,
                    workspace_id: current.workspace_id,
                    project_id: current.project_id,
                    actor_id: user_id,
                    issue_id: None,
                    comment_id: None,
                    label_id: None,
                    sprint_id: Some(sprint_id),
                    changes: Some(serde_json::json!({
                        "status": {"old": current.status, "new": new_status},
                        "rollover": rollover.as_ref().map(|summary| serde_json::json!({
                            "moved_count": summary.moved_count,
                            "target_sprint_id": summary.target_sprint_id,
                        })),
                    })),
                    mentions: Vec::new(),
                    extra_data: None,
                },
            );
        }
    }
    if let Some((items, target)) = &moved {
        emit_moves(&state, scope, items, *target);
    }

    let mut response = SprintResponse::from(updated);
    response.rollover = rollover;
    Ok(ApiResponse::success(response))
}

/// DELETE /api/v1/sprints/:id - Delete sprint
pub async fn delete_sprint(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    bot: Option<Extension<BotAuthContext>>,
    Path(sprint_id): Path<Uuid>,
) -> Result<impl IntoResponse, ApiError> {
    let user_id = Uuid::parse_str(&claims.sub).map_err(|_| ApiError::Unauthorized("invalid user id".to_string()))?;
    let extensions = build_auth_extensions(claims, bot);

    #[derive(Debug, FromQueryResult)]
    struct SprintWorkspace {
        workspace_id: Uuid,
        project_id: Uuid,
    }

    let sprint = SprintWorkspace::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        r"
            SELECT p.workspace_id, s.project_id
            FROM sprints s
            INNER JOIN projects p ON s.project_id = p.id
            WHERE s.id = $1
        ",
        vec![sprint_id.into()],
    ))
    .one(&state.db)
    .await?
    .ok_or_else(|| ApiError::NotFound("sprint not found".to_string()))?;

    let (_, role, _) = require_workspace_access(&state, &extensions, sprint.workspace_id).await?;
    if !matches!(role.as_str(), "owner" | "admin") {
        return Err(ApiError::Forbidden(
            "only owners and admins can delete sprints".to_string(),
        ));
    }

    // The items go to the backlog with a record of it, instead of silently losing their sprint.
    let scope = MoveScope {
        workspace_id: sprint.workspace_id,
        project_id: sprint.project_id,
        actor_id: user_id,
    };
    let tx = state.db.begin().await?;
    let items = lock_all_items(&tx, sprint_id).await?;
    move_items(&tx, scope, &items, None, Timestamp::now()).await?;
    tx.execute(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "DELETE FROM sprints WHERE id = $1",
        vec![sprint_id.into()],
    ))
    .await?;
    tx.commit().await?;

    emit_moves(&state, scope, &items, None);

    Ok(ApiResponse::ok())
}
