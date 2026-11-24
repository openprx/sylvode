//! Workspace milestones.
//!
//! One milestone spans the projects of a workspace, and a work item belongs to at most one of
//! them. Owners and admins manage milestones; members read them and set the milestone of an item
//! through the work item routes.
// Local SQL row types stay beside the queries whose column shapes they mirror.
#![allow(clippy::items_after_statements, clippy::similar_names)]

use std::collections::{BTreeMap, HashMap, HashSet};

use crate::middleware::bot_auth::{BotAuthContext, require_workspace_access};
use axum::{
    Extension, Json,
    extract::{Path, Query, State},
    response::IntoResponse,
};
use platform::{app::AppState, auth::JwtClaims, timestamp::Timestamp};
use sea_orm::{ConnectionTrait, DbBackend, FromQueryResult, Statement, TransactionTrait};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};
use time::Date;
use uuid::Uuid;

use crate::{
    entities::ISO_DATE_FORMAT,
    error::ApiError,
    request::nullable,
    response::{ApiResponse, PaginatedData},
    services::{
        issue_view::identifier,
        issues::{IssueRef, insert_activity},
        workflow_service::{finished_state_keys, resolve_effective_workflow_for_project},
    },
    webhook_trigger::{TriggerContext, WebhookEvent, trigger_webhooks},
};

/// The most items the detail answer lists; `truncated` says there were more.
const DETAIL_ITEM_LIMIT: usize = 500;
const NAME_MAX_CHARS: usize = 100;
/// The refusal for a milestone that cannot be assigned: missing, of another workspace or closed.
const NOT_ASSIGNABLE: &str = "milestone must be an open milestone of this workspace";

#[derive(Debug, Serialize)]
pub struct MilestoneResponse {
    pub id: Uuid,
    pub workspace_id: Uuid,
    pub name: String,
    pub description: String,
    pub target_date: Option<String>,
    pub status: String,
    pub created_at: String,
    pub updated_at: String,
    pub progress: MilestoneProgress,
}

/// The work items of a milestone and how many of them are in a finished state of their own
/// project's workflow.
#[derive(Debug, Default, Serialize)]
pub struct MilestoneProgress {
    pub total: i64,
    pub done: i64,
    pub by_project: Vec<ProjectProgress>,
}

#[derive(Debug, Serialize)]
pub struct ProjectProgress {
    pub project_id: Uuid,
    pub project_key: String,
    pub total: i64,
    pub done: i64,
}

#[derive(Debug, Serialize)]
pub struct MilestoneDetailResponse {
    #[serde(flatten)]
    pub milestone: MilestoneResponse,
    pub items_by_project: Vec<ProjectItems>,
    /// True when the milestone has more than 500 items and the list is cut.
    pub truncated: bool,
}

#[derive(Debug, Serialize)]
pub struct ProjectItems {
    pub project_id: Uuid,
    pub project_key: String,
    pub items: Vec<MilestoneItem>,
}

#[derive(Debug, Serialize)]
pub struct MilestoneItem {
    pub id: Uuid,
    pub identifier: String,
    pub title: String,
    pub state: String,
    pub done: bool,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CreateMilestoneRequest {
    pub name: String,
    pub description: Option<String>,
    pub target_date: Option<String>,
}

/// `PUT /milestones/{id}`: a key that is absent leaves the field alone. `target_date` also accepts
/// `null`, which clears it; `null` on any other field is refused.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
#[allow(
    clippy::option_option,
    reason = "absent, null and a value are three different requests"
)]
pub struct UpdateMilestoneRequest {
    #[serde(default, deserialize_with = "nullable")]
    pub name: Option<Option<String>>,
    #[serde(default, deserialize_with = "nullable")]
    pub description: Option<Option<String>>,
    #[serde(default, deserialize_with = "nullable")]
    pub target_date: Option<Option<String>>,
    #[serde(default, deserialize_with = "nullable")]
    pub status: Option<Option<String>>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ListMilestonesQuery {
    pub status: Option<String>,
    pub page: Option<i64>,
    pub per_page: Option<i64>,
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

fn build_auth_extensions(claims: JwtClaims, bot: Option<Extension<BotAuthContext>>) -> axum::http::Extensions {
    let mut extensions = axum::http::Extensions::new();
    extensions.insert(claims);
    if let Some(Extension(bot_ctx)) = bot {
        extensions.insert(bot_ctx);
    }
    extensions
}

fn validate_status(status: &str) -> Result<(), ApiError> {
    match status {
        "open" | "closed" => Ok(()),
        _ => Err(ApiError::BadRequest("status must be one of: open, closed".to_string())),
    }
}

/// The trimmed name, 1 to 100 characters.
fn clean_name(raw: &str) -> Result<String, ApiError> {
    let name = raw.trim();
    if name.is_empty() || name.chars().count() > NAME_MAX_CHARS {
        return Err(ApiError::BadRequest(format!(
            "name must be 1 to {NAME_MAX_CHARS} characters"
        )));
    }
    Ok(name.to_string())
}

fn parse_target_date(raw: &str) -> Result<Date, ApiError> {
    Date::parse(raw, ISO_DATE_FORMAT)
        .map_err(|_| ApiError::BadRequest("invalid target_date format (use YYYY-MM-DD)".to_string()))
}

fn require_manager(role: &str, action: &str) -> Result<(), ApiError> {
    if matches!(role, "owner" | "admin") {
        Ok(())
    } else {
        Err(ApiError::Forbidden(format!(
            "only owners and admins can {action} milestones"
        )))
    }
}

fn name_taken() -> ApiError {
    ApiError::conflict_coded(
        "milestone_name_taken",
        "milestone with this name already exists in workspace",
        None,
    )
}

/// Maps the unique violation of the name rule to the 409 it stands for; the check before each
/// write answers the common case, this catches two requests racing.
fn rule_violation(err: sea_orm::DbErr) -> ApiError {
    if err.to_string().contains("milestones_workspace_lower_name") {
        name_taken()
    } else {
        ApiError::from(err)
    }
}

#[derive(Debug, FromQueryResult)]
struct MilestoneRow {
    id: Uuid,
    workspace_id: Uuid,
    name: String,
    description: String,
    target_date: Option<Date>,
    status: String,
    created_at: Timestamp,
    updated_at: Timestamp,
}

const MILESTONE_COLUMNS: &str = "id, workspace_id, name, description, target_date, status, created_at, updated_at";

impl MilestoneRow {
    fn into_response(self, progress: MilestoneProgress) -> MilestoneResponse {
        MilestoneResponse {
            id: self.id,
            workspace_id: self.workspace_id,
            name: self.name,
            description: self.description,
            target_date: self.target_date.map(|d| d.to_string()),
            status: self.status,
            created_at: self.created_at.to_rfc3339(),
            updated_at: self.updated_at.to_rfc3339(),
            progress,
        }
    }

    /// The milestone as a webhook payload carries it: no progress, which is a count at read time.
    fn event_data(&self) -> Value {
        json!({
            "id": self.id.to_string(),
            "workspace_id": self.workspace_id.to_string(),
            "name": self.name,
            "description": self.description,
            "target_date": self.target_date.map(|d| d.to_string()),
            "status": self.status,
            "created_at": self.created_at.to_rfc3339(),
            "updated_at": self.updated_at.to_rfc3339(),
        })
    }
}

async fn load_milestone(
    db: &impl ConnectionTrait,
    milestone_id: Uuid,
    lock: bool,
) -> Result<Option<MilestoneRow>, ApiError> {
    let lock = if lock { " FOR UPDATE" } else { "" };
    Ok(MilestoneRow::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        format!("SELECT {MILESTONE_COLUMNS} FROM milestones WHERE id = $1{lock}"),
        vec![milestone_id.into()],
    ))
    .one(db)
    .await?)
}

/// Refuses a milestone that an item of `workspace_id` cannot be put into.
///
/// One that does not exist, belongs to another workspace, or is closed. All three answer the same,
/// so the route is no existence oracle over milestone ids. The row stays share-locked until the transaction ends, so
/// the milestone cannot be deleted or closed between this check and the write.
///
/// # Errors
/// `400` for a milestone that cannot be assigned; a database failure.
pub async fn ensure_open_milestone(
    db: &impl ConnectionTrait,
    milestone_id: Uuid,
    workspace_id: Uuid,
) -> Result<(), ApiError> {
    let found = db
        .query_one(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT 1 FROM milestones WHERE id = $1 AND workspace_id = $2 AND status = 'open' FOR SHARE",
            vec![milestone_id.into(), workspace_id.into()],
        ))
        .await?;
    if found.is_none() {
        return Err(ApiError::BadRequest(NOT_ASSIGNABLE.to_string()));
    }
    Ok(())
}

/// 409 when another milestone of the workspace has this name, ignoring case.
async fn ensure_name_free(
    db: &impl ConnectionTrait,
    workspace_id: Uuid,
    name: &str,
    except: Option<Uuid>,
) -> Result<(), ApiError> {
    let existing = db
        .query_one(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT id FROM milestones WHERE workspace_id = $1 AND lower(name) = lower($2) AND ($3::uuid IS NULL OR id <> $3)",
            vec![workspace_id.into(), name.to_string().into(), except.into()],
        ))
        .await?;
    if existing.is_some() {
        return Err(name_taken());
    }
    Ok(())
}

/// The finished state keys of each of `project_ids`' effective workflows.
async fn finished_states(
    state: &AppState,
    project_ids: impl Iterator<Item = Uuid>,
) -> Result<HashMap<Uuid, HashSet<String>>, ApiError> {
    let mut finished = HashMap::new();
    for project_id in project_ids {
        if let std::collections::hash_map::Entry::Vacant(slot) = finished.entry(project_id) {
            let workflow = resolve_effective_workflow_for_project(state, project_id).await?;
            slot.insert(finished_state_keys(&workflow).into_iter().collect());
        }
    }
    Ok(finished)
}

fn is_done(finished: &HashMap<Uuid, HashSet<String>>, project_id: Uuid, item_state: &str) -> bool {
    finished.get(&project_id).is_some_and(|keys| keys.contains(item_state))
}

/// Per project (ordered by key and id) the item count and the finished count.
type ProjectTotals = BTreeMap<(String, Uuid), (i64, i64)>;

/// The progress of each of `milestone_ids`; a milestone without items has the empty progress.
async fn progress_of(state: &AppState, milestone_ids: &[Uuid]) -> Result<HashMap<Uuid, MilestoneProgress>, ApiError> {
    #[derive(Debug, FromQueryResult)]
    struct Counts {
        milestone_id: Uuid,
        project_id: Uuid,
        project_key: String,
        state: String,
        n: i64,
    }
    if milestone_ids.is_empty() {
        return Ok(HashMap::new());
    }
    let counts = Counts::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        r"
            SELECT wi.milestone_id, wi.project_id, p.key AS project_key, wi.state, COUNT(*)::bigint AS n
            FROM work_items wi
            INNER JOIN projects p ON p.id = wi.project_id
            WHERE wi.milestone_id = ANY($1)
            GROUP BY wi.milestone_id, wi.project_id, p.key, wi.state
        ",
        vec![milestone_ids.to_vec().into()],
    ))
    .all(&state.db)
    .await?;
    let finished = finished_states(state, counts.iter().map(|c| c.project_id)).await?;

    // Per milestone, per project (ordered by key), the totals.
    let mut grouped: HashMap<Uuid, ProjectTotals> = HashMap::new();
    for row in counts {
        let entry = grouped
            .entry(row.milestone_id)
            .or_default()
            .entry((row.project_key, row.project_id))
            .or_default();
        entry.0 += row.n;
        if is_done(&finished, row.project_id, &row.state) {
            entry.1 += row.n;
        }
    }
    Ok(grouped
        .into_iter()
        .map(|(milestone_id, projects)| {
            let by_project: Vec<ProjectProgress> = projects
                .into_iter()
                .map(|((project_key, project_id), (total, done))| ProjectProgress {
                    project_id,
                    project_key,
                    total,
                    done,
                })
                .collect();
            let progress = MilestoneProgress {
                total: by_project.iter().map(|p| p.total).sum(),
                done: by_project.iter().map(|p| p.done).sum(),
                by_project,
            };
            (milestone_id, progress)
        })
        .collect())
}

async fn respond(state: &AppState, row: MilestoneRow) -> Result<MilestoneResponse, ApiError> {
    let progress = progress_of(state, &[row.id]).await?.remove(&row.id).unwrap_or_default();
    Ok(row.into_response(progress))
}

fn trigger_milestone_event(state: &AppState, event: WebhookEvent, workspace_id: Uuid, actor_id: Uuid, data: Value) {
    trigger_webhooks(
        state.clone(),
        TriggerContext {
            event,
            workspace_id,
            // A milestone belongs to the workspace, not to a project.
            project_id: Uuid::nil(),
            actor_id,
            issue_id: None,
            comment_id: None,
            label_id: None,
            sprint_id: None,
            changes: None,
            mentions: Vec::new(),
            extra_data: Some(data),
        },
    );
}

/// GET /`api/v1/workspaces/:workspace_id/milestones` - List milestones
pub async fn list_milestones(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    bot: Option<Extension<BotAuthContext>>,
    Path(workspace_id): Path<Uuid>,
    Query(query): Query<ListMilestonesQuery>,
) -> Result<impl IntoResponse, ApiError> {
    let extensions = build_auth_extensions(claims, bot);
    require_workspace_access(&state, &extensions, workspace_id).await?;
    if let Some(status) = &query.status {
        validate_status(status)?;
    }

    // The same paging rules as the work item list.
    let page = query.page.unwrap_or(1).max(1);
    let per_page = query.per_page.unwrap_or(15).clamp(1, 100);
    let offset = (page - 1).saturating_mul(per_page);

    let total: i64 = state
        .db
        .query_one(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT COUNT(*)::bigint AS count FROM milestones WHERE workspace_id = $1 AND ($2::text IS NULL OR status = $2)",
            vec![workspace_id.into(), query.status.clone().into()],
        ))
        .await?
        .ok_or(ApiError::Internal)?
        .try_get("", "count")?;

    let rows = MilestoneRow::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        format!(
            "SELECT {MILESTONE_COLUMNS} FROM milestones WHERE workspace_id = $1 AND ($2::text IS NULL OR status = $2) \
             ORDER BY target_date ASC NULLS LAST, lower(name) ASC, id ASC LIMIT $3 OFFSET $4"
        ),
        vec![workspace_id.into(), query.status.into(), per_page.into(), offset.into()],
    ))
    .all(&state.db)
    .await?;

    let ids: Vec<Uuid> = rows.iter().map(|row| row.id).collect();
    let mut progress = progress_of(&state, &ids).await?;
    let items: Vec<MilestoneResponse> = rows
        .into_iter()
        .map(|row| {
            let progress = progress.remove(&row.id).unwrap_or_default();
            row.into_response(progress)
        })
        .collect();
    let total_pages = if total == 0 {
        0
    } else {
        total / per_page + i64::from(total % per_page != 0)
    };
    Ok(ApiResponse::success(PaginatedData {
        items,
        total,
        page,
        per_page,
        total_pages,
    }))
}

/// POST /`api/v1/workspaces/:workspace_id/milestones` - Create a milestone
pub async fn create_milestone(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    bot: Option<Extension<BotAuthContext>>,
    Path(workspace_id): Path<Uuid>,
    Json(req): Json<CreateMilestoneRequest>,
) -> Result<impl IntoResponse, ApiError> {
    let extensions = build_auth_extensions(claims, bot);
    let (actor_id, role, _) = require_workspace_access(&state, &extensions, workspace_id).await?;
    require_manager(&role, "create")?;

    let name = clean_name(&req.name)?;
    let target_date = req.target_date.as_deref().map(parse_target_date).transpose()?;
    let description = req.description.unwrap_or_default();

    let milestone_id = Uuid::new_v4();
    let now = Timestamp::now();
    let tx = state.db.begin().await?;
    ensure_name_free(&tx, workspace_id, &name, None).await?;
    tx.execute(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "INSERT INTO milestones (id, workspace_id, name, description, target_date, status, created_by, created_at, updated_at) \
         VALUES ($1, $2, $3, $4, $5, 'open', $6, $7, $7)",
        vec![
            milestone_id.into(),
            workspace_id.into(),
            name.into(),
            description.into(),
            target_date.into(),
            actor_id.into(),
            now.into(),
        ],
    ))
    .await
    .map_err(rule_violation)?;
    let created = load_milestone(&tx, milestone_id, false)
        .await?
        .ok_or(ApiError::Internal)?;
    tx.commit().await.map_err(rule_violation)?;

    trigger_milestone_event(
        &state,
        WebhookEvent::MilestoneCreated,
        workspace_id,
        actor_id,
        json!({ "milestone": created.event_data() }),
    );
    Ok(ApiResponse::success(respond(&state, created).await?))
}

/// GET /api/v1/milestones/:id - One milestone with its work items grouped by project
pub async fn get_milestone(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    bot: Option<Extension<BotAuthContext>>,
    Path(milestone_id): Path<Uuid>,
) -> Result<impl IntoResponse, ApiError> {
    let extensions = build_auth_extensions(claims, bot);
    let row = load_milestone(&state.db, milestone_id, false)
        .await?
        .ok_or_else(|| ApiError::NotFound("milestone not found".to_string()))?;
    require_workspace_access(&state, &extensions, row.workspace_id).await?;

    #[derive(Debug, FromQueryResult)]
    struct ItemRow {
        id: Uuid,
        project_id: Uuid,
        project_key: String,
        sequence_number: i32,
        title: String,
        state: String,
    }
    let mut items = ItemRow::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        r"
            SELECT wi.id, wi.project_id, p.key AS project_key, wi.sequence_number, wi.title, wi.state
            FROM work_items wi
            INNER JOIN projects p ON p.id = wi.project_id
            WHERE wi.milestone_id = $1
            ORDER BY p.key, wi.sequence_number
            LIMIT $2
        ",
        // One more than the limit tells whether the list is cut.
        vec![
            milestone_id.into(),
            i64::try_from(DETAIL_ITEM_LIMIT + 1).unwrap_or(i64::MAX).into(),
        ],
    ))
    .all(&state.db)
    .await?;
    let truncated = items.len() > DETAIL_ITEM_LIMIT;
    items.truncate(DETAIL_ITEM_LIMIT);

    let finished = finished_states(&state, items.iter().map(|item| item.project_id)).await?;
    let mut items_by_project: Vec<ProjectItems> = Vec::new();
    for item in items {
        let entry = MilestoneItem {
            id: item.id,
            identifier: identifier(&item.project_key, item.sequence_number),
            title: item.title,
            done: is_done(&finished, item.project_id, &item.state),
            state: item.state,
        };
        match items_by_project.last_mut() {
            Some(group) if group.project_id == item.project_id => group.items.push(entry),
            _ => items_by_project.push(ProjectItems {
                project_id: item.project_id,
                project_key: item.project_key,
                items: vec![entry],
            }),
        }
    }

    Ok(ApiResponse::success(MilestoneDetailResponse {
        milestone: respond(&state, row).await?,
        items_by_project,
        truncated,
    }))
}

/// PUT /api/v1/milestones/:id - Edit or close a milestone
pub async fn update_milestone(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    bot: Option<Extension<BotAuthContext>>,
    Path(milestone_id): Path<Uuid>,
    Json(req): Json<UpdateMilestoneRequest>,
) -> Result<impl IntoResponse, ApiError> {
    let extensions = build_auth_extensions(claims, bot);
    let UpdateMilestoneRequest {
        name: requested_name,
        description: requested_description,
        target_date: requested_target,
        status: requested_status,
    } = req;
    let requested_name = not_null(requested_name, "name")?;
    let requested_description = not_null(requested_description, "description")?;
    let requested_status = not_null(requested_status, "status")?;

    // The row is read first only to find the workspace; the permission is checked before any
    // field is looked at, so a member learns nothing from the validation messages.
    let located = load_milestone(&state.db, milestone_id, false)
        .await?
        .ok_or_else(|| ApiError::NotFound("milestone not found".to_string()))?;
    let (actor_id, role, _) = require_workspace_access(&state, &extensions, located.workspace_id).await?;
    require_manager(&role, "edit")?;

    if let Some(status) = &requested_status {
        validate_status(status)?;
    }
    if requested_name.is_none()
        && requested_description.is_none()
        && requested_target.is_none()
        && requested_status.is_none()
    {
        return Err(ApiError::BadRequest("no fields to update".to_string()));
    }
    let requested_name = requested_name.as_deref().map(clean_name).transpose()?;
    let requested_target = requested_target
        .map(|target| target.map(|raw| parse_target_date(&raw)).transpose())
        .transpose()?;

    // The row stays locked until the commit, so two changes of one milestone run one after the other.
    let tx = state.db.begin().await?;
    let current = load_milestone(&tx, milestone_id, true)
        .await?
        .ok_or_else(|| ApiError::NotFound("milestone not found".to_string()))?;
    if let Some(name) = &requested_name {
        ensure_name_free(&tx, current.workspace_id, name, Some(milestone_id)).await?;
    }

    let mut changes = Map::<String, Value>::new();
    let mut updates = Vec::new();
    let mut values: Vec<sea_orm::Value> = Vec::new();
    let mut push = |column: &str, value: sea_orm::Value| {
        values.push(value);
        updates.push(format!("{column} = ${}", values.len()));
    };
    if let Some(name) = requested_name
        && name != current.name
    {
        changes.insert("name".to_string(), json!({"old": current.name, "new": name}));
        push("name", name.into());
    }
    if let Some(description) = requested_description
        && description != current.description
    {
        changes.insert(
            "description".to_string(),
            json!({"old": current.description, "new": description}),
        );
        push("description", description.into());
    }
    if let Some(target) = requested_target
        && target != current.target_date
    {
        changes.insert(
            "target_date".to_string(),
            json!({"old": current.target_date.map(|d| d.to_string()), "new": target.map(|d| d.to_string())}),
        );
        push("target_date", target.into());
    }
    if let Some(status) = requested_status
        && status != current.status
    {
        changes.insert("status".to_string(), json!({"old": current.status, "new": status}));
        push("status", status.into());
    }

    if changes.is_empty() {
        tx.commit().await?;
        return Ok(ApiResponse::success(respond(&state, current).await?));
    }
    push("updated_at", Timestamp::now().into());
    values.push(milestone_id.into());
    let query = format!(
        "UPDATE milestones SET {} WHERE id = ${}",
        updates.join(", "),
        values.len()
    );
    tx.execute(Statement::from_sql_and_values(DbBackend::Postgres, &query, values))
        .await
        .map_err(rule_violation)?;
    let updated = load_milestone(&tx, milestone_id, false)
        .await?
        .ok_or(ApiError::Internal)?;
    tx.commit().await.map_err(rule_violation)?;

    trigger_milestone_event(
        &state,
        WebhookEvent::MilestoneUpdated,
        updated.workspace_id,
        actor_id,
        json!({ "milestone": updated.event_data(), "changes": changes }),
    );
    Ok(ApiResponse::success(respond(&state, updated).await?))
}

/// DELETE /api/v1/milestones/:id - Delete a milestone; its items lose it, with a record each
pub async fn delete_milestone(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    bot: Option<Extension<BotAuthContext>>,
    Path(milestone_id): Path<Uuid>,
) -> Result<impl IntoResponse, ApiError> {
    let extensions = build_auth_extensions(claims, bot);
    let located = load_milestone(&state.db, milestone_id, false)
        .await?
        .ok_or_else(|| ApiError::NotFound("milestone not found".to_string()))?;
    let (actor_id, role, _) = require_workspace_access(&state, &extensions, located.workspace_id).await?;
    require_manager(&role, "delete")?;

    #[derive(Debug, FromQueryResult)]
    struct ClearedItem {
        id: Uuid,
        project_id: Uuid,
    }

    // The milestone row is locked first, so an assignment (which share-locks it) waits for this
    // transaction and then finds it gone.
    let tx = state.db.begin().await?;
    let Some(milestone) = load_milestone(&tx, milestone_id, true).await? else {
        return Err(ApiError::NotFound("milestone not found".to_string()));
    };
    let items = ClearedItem::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        r"
            SELECT wi.id, wi.project_id
            FROM work_items wi
            WHERE wi.milestone_id = $1
            ORDER BY wi.project_id, wi.sequence_number
            FOR UPDATE
        ",
        vec![milestone_id.into()],
    ))
    .all(&tx)
    .await?;
    let now = Timestamp::now();
    if !items.is_empty() {
        let ids: Vec<Uuid> = items.iter().map(|item| item.id).collect();
        tx.execute(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "UPDATE work_items SET milestone_id = NULL, updated_at = $1 WHERE id = ANY($2)",
            vec![now.into(), ids.into()],
        ))
        .await?;
    }
    for item in &items {
        let issue = IssueRef {
            id: item.id,
            project_id: item.project_id,
            workspace_id: milestone.workspace_id,
        };
        insert_activity(
            &tx,
            &issue,
            actor_id,
            "milestone_changed",
            &json!({"from": milestone_id.to_string(), "to": null}),
            now,
        )
        .await?;
    }
    tx.execute(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "DELETE FROM milestones WHERE id = $1",
        vec![milestone_id.into()],
    ))
    .await?;
    tx.commit().await?;

    for item in &items {
        trigger_webhooks(
            state.clone(),
            TriggerContext {
                event: WebhookEvent::IssueUpdated,
                workspace_id: milestone.workspace_id,
                project_id: item.project_id,
                actor_id,
                issue_id: Some(item.id),
                comment_id: None,
                label_id: None,
                sprint_id: None,
                changes: Some(json!({
                    "milestone_id": {"old": milestone_id.to_string(), "new": null}
                })),
                mentions: Vec::new(),
                extra_data: None,
            },
        );
    }
    trigger_milestone_event(
        &state,
        WebhookEvent::MilestoneDeleted,
        milestone.workspace_id,
        actor_id,
        json!({ "milestone": milestone.event_data(), "cleared_item_count": items.len() }),
    );

    Ok(ApiResponse::ok())
}

#[cfg(test)]
mod tests {
    use super::{clean_name, parse_target_date, validate_status};

    #[test]
    fn names_are_trimmed_and_bounded() {
        assert_eq!(clean_name("  Beta ").ok().as_deref(), Some("Beta"));
        assert!(clean_name("   ").is_err());
        assert!(clean_name(&"x".repeat(100)).is_ok());
        assert!(clean_name(&"x".repeat(101)).is_err());
        // The limit counts characters, not bytes.
        assert!(clean_name(&"里".repeat(100)).is_ok());
    }

    #[test]
    fn statuses_and_dates_are_checked() {
        assert!(validate_status("open").is_ok());
        assert!(validate_status("closed").is_ok());
        assert!(validate_status("done").is_err());
        assert!(parse_target_date("2030-02-01").is_ok());
        assert!(parse_target_date("2030-02-30").is_err());
        assert!(parse_target_date("01/02/2030").is_err());
    }
}
