// Domain-specific local names remain explicit for audit readability.
// Local SQL row types stay beside the queries whose column shapes they mirror.
#![allow(clippy::items_after_statements, clippy::similar_names)]

use crate::middleware::bot_auth::{BotAuthContext, require_workspace_access};
use axum::{
    Extension, Json,
    extract::{Path, Query, State},
    response::IntoResponse,
};
use platform::{app::AppState, auth::JwtClaims};
use sea_orm::{ConnectionTrait, DbBackend, FromQueryResult, Statement, TransactionTrait};
use serde::Deserialize;
use serde_json::{Map, Value, json};
use uuid::Uuid;

use crate::{
    error::ApiError,
    request::nullable,
    response::{ApiResponse, PaginatedData},
    routes::milestone::ensure_open_milestone,
    services::{
        ai_task_service::{CreateAiTaskInput, DispatchOrigin, DispatchOutcome, dispatch_ai_task, runner_bot_block},
        bot_identity::is_bot_user,
        issue_view::{
            IssueResponse, PageCaller, attach_page_links, identifier, issue_labels, load_issue, page_link_counts,
        },
        issues::{
            IssueRef, NotifyAssignment, NotifyChange, insert_activity, notify_assignment, notify_priority_change,
            notify_state_change, record_label_change,
        },
        work_item_service::{NewWorkItem, insert_work_item},
        workflow_service::{allowed_state_values, default_project_state, resolve_effective_workflow_for_project},
    },
    webhook_trigger::{
        TriggerContext, WebhookEvent, issue_snapshot, trigger_webhooks, trigger_webhooks_after_ai_tasks,
    },
};

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CreateIssueRequest {
    pub title: String,
    pub description: Option<String>,
    pub state: Option<String>,
    pub priority: Option<String>,
    pub assignee_id: Option<Uuid>,
    pub due_at: Option<String>,
    pub sprint_id: Option<Uuid>,
    pub milestone_id: Option<Uuid>,
}

/// `PUT /issues/{id}`: a key that is absent leaves the field alone. `assignee_id`, `sprint_id`,
/// `milestone_id` and `due_at` also accept `null`, which clears them; `null` on any other field is refused.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
#[allow(
    clippy::option_option,
    reason = "absent, null and a value are three different requests"
)]
pub struct UpdateIssueRequest {
    #[serde(default, deserialize_with = "nullable")]
    pub title: Option<Option<String>>,
    #[serde(default, deserialize_with = "nullable")]
    pub description: Option<Option<String>>,
    #[serde(default, deserialize_with = "nullable")]
    pub state: Option<Option<String>>,
    #[serde(default, deserialize_with = "nullable")]
    pub priority: Option<Option<String>>,
    #[serde(default, deserialize_with = "nullable")]
    pub assignee_id: Option<Option<Uuid>>,
    #[serde(default, deserialize_with = "nullable")]
    pub due_at: Option<Option<String>>,
    #[serde(default, deserialize_with = "nullable")]
    pub sprint_id: Option<Option<Uuid>>,
    #[serde(default, deserialize_with = "nullable")]
    pub milestone_id: Option<Option<Uuid>>,
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

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ListIssuesQuery {
    pub page: Option<i64>,
    pub per_page: Option<i64>,
    pub state: Option<String>,
    pub assignee_id: Option<Uuid>,
    pub priority: Option<String>,
    /// A sprint id, or `none` for the items that are in no sprint.
    pub sprint_id: Option<String>,
    /// A milestone id, or `none` for the items that are in no milestone.
    pub milestone_id: Option<String>,
    pub search: Option<String>,
    pub label_ids: Option<String>,
    pub sort_by: Option<String>,
    pub sort_order: Option<String>,
}

/// Validate state field against project's effective workflow
async fn validate_project_state(state: &AppState, project_id: Uuid, state_key: &str) -> Result<(), ApiError> {
    let workflow = resolve_effective_workflow_for_project(state, project_id).await?;
    if workflow.states.iter().any(|s| s.key == state_key) {
        return Ok(());
    }

    Err(ApiError::BadRequest(format!(
        "state must be one of: {}",
        allowed_state_values(&workflow)
    )))
}

/// Validate priority field
fn validate_priority(priority: &str) -> Result<(), ApiError> {
    match priority {
        "low" | "medium" | "high" | "urgent" => Ok(()),
        _ => Err(ApiError::BadRequest(
            "priority must be one of: low, medium, high, urgent".to_string(),
        )),
    }
}

pub(crate) fn build_auth_extensions(
    claims: JwtClaims,
    bot: Option<Extension<BotAuthContext>>,
) -> axum::http::Extensions {
    let mut extensions = axum::http::Extensions::new();
    extensions.insert(claims);
    if let Some(Extension(bot_ctx)) = bot {
        extensions.insert(bot_ctx);
    }
    extensions
}

/// POST /`api/v1/projects/:project_id/issues` - Create a new issue
/// Refuses a `sprint_id` that does not belong to the work item's own project.
///
/// `work_items.sprint_id` is a single-column foreign key to `sprints(id)`, and neither create nor
/// update used to look at it, so an issue could be filed into another tenant's sprint: a dangling
/// cross-tenant reference, an existence oracle on `sprints` ids (foreign-key violation vs success)
/// and a row that the other tenant can silently mutate through `ON DELETE SET NULL`. The sibling
/// field `assignee_id` has always been checked this way, against `workspace_members`.
///
/// Sprints belong to a project, so the containment checked here is the project, which is strictly
/// inside the workspace. A sprint in another project and a sprint that does not exist produce the
/// same `BadRequest`, so this does not become an oracle of its own.
/// One assignment of an issue, as queued by [`queue_assignment_ai_task`].
pub(crate) struct AssignmentTaskInput {
    pub(crate) project_id: Uuid,
    pub(crate) issue_id: Uuid,
    pub(crate) assignee_id: Uuid,
    pub(crate) actor_id: Uuid,
    pub(crate) trigger: &'static str,
    pub(crate) idempotency_key: String,
}

/// What an assignment produced: the bots that received their task for it as a worker AI task (to
/// pass to `trigger_webhooks_after_ai_tasks`).
#[derive(Debug, Default)]
pub(crate) struct AssignmentOutcome {
    pub(crate) ai_task_bots: Vec<Uuid>,
}

/// Whether assigning `assignee_id` queues an `issue_assigned` AI task.
///
/// Assigning a bot gives it exactly one bot task, and that task is the worker AI task: it is
/// durable, retried, and completed or failed through the AI-task callbacks. A bot that assigns an
/// issue to itself gets no task, since an action never hands its own actor a task. A person
/// assignee gets none either.
const fn assignment_queues_ai_task(actor_id: Uuid, assignee_id: Uuid, assignee_is_bot: bool) -> bool {
    assignee_is_bot && actor_id.as_u128() != assignee_id.as_u128()
}

pub(crate) async fn queue_assignment_ai_task(
    db: &impl ConnectionTrait,
    input: AssignmentTaskInput,
) -> Result<AssignmentOutcome, ApiError> {
    let assignee_is_bot = is_bot_user(db, input.assignee_id).await?;
    if !assignment_queues_ai_task(input.actor_id, input.assignee_id, assignee_is_bot) {
        return Ok(AssignmentOutcome {
            ai_task_bots: Vec::new(),
        });
    }
    // Assigning a runner bot that this project has not enabled is allowed (the issue is its
    // assignee) but starts nothing: the bot is not given a task it may not run.
    if let Some(block) = runner_bot_block(db, input.project_id, input.assignee_id).await? {
        tracing::info!(
            issue_id = %input.issue_id,
            bot_id = %input.assignee_id,
            reason = block.code(),
            "assignment queued no task"
        );
        return Ok(AssignmentOutcome {
            ai_task_bots: Vec::new(),
        });
    }
    let queued = dispatch_ai_task(
        db,
        CreateAiTaskInput {
            origin: DispatchOrigin::Request,
            project_id: input.project_id,
            ai_participant_id: input.assignee_id,
            actor_id: Some(input.actor_id),
            task_type: "issue_assigned".to_string(),
            reference_type: Some("work_item".to_string()),
            reference_id: Some(input.issue_id),
            priority: 10,
            payload: json!({
                "issue_id": input.issue_id.to_string(),
                "project_id": input.project_id.to_string(),
                "assignee_id": input.assignee_id.to_string(),
                "trigger": input.trigger,
            }),
            idempotency_key: Some(input.idempotency_key),
            max_attempts: 3,
        },
    )
    .await?;
    match queued {
        DispatchOutcome::Created(_) => {}
        // The same key was queued before: the bot already holds its task for this assignment.
        DispatchOutcome::Duplicate => {
            tracing::debug!(issue_id = %input.issue_id, bot_id = %input.assignee_id, "assignment task already queued");
        }
        // The assignment stands and the bot stays in the list below: it is also the bots whose
        // webhook delivery must not carry `bot_context`, which would start the bot.
        DispatchOutcome::ChainRefused(refusal) => {
            tracing::info!(
                issue_id = %input.issue_id,
                bot_id = %input.assignee_id,
                reason = refusal.code(),
                "assignment queued no task"
            );
        }
    }
    Ok(AssignmentOutcome {
        ai_task_bots: vec![input.assignee_id],
    })
}

async fn ensure_sprint_in_project(
    db: &impl ConnectionTrait,
    sprint_id: Uuid,
    project_id: Uuid,
) -> Result<(), ApiError> {
    let exists = db
        .query_one(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT 1 FROM sprints WHERE id = $1 AND project_id = $2",
            vec![sprint_id.into(), project_id.into()],
        ))
        .await?;

    if exists.is_none() {
        return Err(ApiError::BadRequest("sprint must belong to this project".to_string()));
    }

    Ok(())
}

pub async fn create_issue(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    bot: Option<Extension<BotAuthContext>>,
    Path(project_id): Path<Uuid>,
    Json(req): Json<CreateIssueRequest>,
) -> Result<impl IntoResponse, ApiError> {
    let user_id = Uuid::parse_str(&claims.sub).map_err(|_| ApiError::Unauthorized("invalid user id".to_string()))?;
    let extensions = build_auth_extensions(claims, bot);

    if req.title.trim().is_empty() {
        return Err(ApiError::BadRequest("title is required".to_string()));
    }

    let issue_priority = req.priority.unwrap_or_else(|| "medium".to_string());
    validate_priority(&issue_priority)?;

    // Check project exists and require workspace access (user or bot).
    #[derive(Debug, FromQueryResult)]
    struct ProjectWorkspace {
        workspace_id: Uuid,
    }

    let project = ProjectWorkspace::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        r"
            SELECT p.workspace_id
            FROM projects p
            WHERE p.id = $1
        ",
        vec![project_id.into()],
    ))
    .one(&state.db)
    .await?
    .ok_or_else(|| ApiError::NotFound("project not found".to_string()))?;

    let access = require_workspace_access(&state, &extensions, project.workspace_id).await?;

    // A connected application may not assign a work item to an agent (nothing is written yet).
    if let Some(assignee_id) = req.assignee_id {
        crate::oauth::delegated::refuse_agent_dispatch(&state.db, &[assignee_id]).await?;
    }

    // Validate state by effective workflow (project > workspace > system)
    let issue_state = if let Some(requested) = req.state {
        validate_project_state(&state, project_id, &requested).await?;
        requested
    } else {
        default_project_state(&state, project_id).await?
    };

    // If assignee specified, verify they're a workspace member
    if let Some(assignee_id) = req.assignee_id {
        let member_exists = state
            .db
            .query_one(Statement::from_sql_and_values(
                DbBackend::Postgres,
                "SELECT 1 FROM workspace_members WHERE workspace_id = $1 AND user_id = $2",
                vec![project.workspace_id.into(), assignee_id.into()],
            ))
            .await?;

        if member_exists.is_none() {
            return Err(ApiError::BadRequest("assignee must be a workspace member".to_string()));
        }
    }

    // If a sprint is specified, verify it belongs to this project (and therefore this workspace)
    if let Some(sprint_id) = req.sprint_id {
        ensure_sprint_in_project(&state.db, sprint_id, project_id).await?;
    }

    let issue_id = Uuid::new_v4();
    let now = platform::timestamp::Timestamp::now();
    let description = req.description.unwrap_or_default();

    // Parse due_at if provided
    let due_at: Option<platform::timestamp::Timestamp> = if let Some(due_str) = req.due_at {
        Some(
            platform::timestamp::Timestamp::parse_rfc3339(&due_str)
                .map_err(|_| ApiError::BadRequest("invalid due_at format".to_string()))?,
        )
    } else {
        None
    };

    let tx = state.db.begin().await?;
    if let Some(milestone_id) = req.milestone_id {
        ensure_open_milestone(&tx, milestone_id, project.workspace_id).await?;
    }
    insert_work_item(
        &tx,
        &NewWorkItem {
            id: issue_id,
            project_id,
            title: &req.title,
            description: &description,
            state: &issue_state,
            priority: &issue_priority,
            assignee_id: req.assignee_id,
            due_at,
            created_by: user_id,
            sprint_id: req.sprint_id,
            now,
        },
    )
    .await?;
    if let Some(milestone_id) = req.milestone_id {
        tx.execute(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "UPDATE work_items SET milestone_id = $1 WHERE id = $2",
            vec![milestone_id.into(), issue_id.into()],
        ))
        .await?;
    }
    if let Some(assignee_id) = req.assignee_id {
        notify_assignment(
            &tx,
            NotifyAssignment {
                workspace_id: project.workspace_id,
                project_id,
                issue_id,
                title: &req.title,
                assignee_id,
                actor_id: user_id,
            },
        )
        .await?;
    }
    tx.commit().await?;

    // The assignment is queued as an AI task before any delivery is triggered, so every delivery
    // of this request knows that the bot already has its one task for it.
    let assignment = match req.assignee_id {
        Some(assignee_id) => {
            queue_assignment_ai_task(
                &state.db,
                AssignmentTaskInput {
                    project_id,
                    issue_id,
                    assignee_id,
                    actor_id: user_id,
                    trigger: "issue.create",
                    idempotency_key: format!("issue_assigned:{issue_id}:{assignee_id}"),
                },
            )
            .await?
        }
        None => AssignmentOutcome::default(),
    };

    trigger_webhooks_after_ai_tasks(
        state.clone(),
        TriggerContext {
            event: WebhookEvent::IssueCreated,
            workspace_id: project.workspace_id,
            project_id,
            actor_id: user_id,
            issue_id: Some(issue_id),
            comment_id: None,
            label_id: None,
            sprint_id: None,
            changes: None,
            mentions: Vec::new(),
            extra_data: None,
        },
        assignment.ai_task_bots.clone(),
    );

    if req.assignee_id.is_some() {
        let created_assignee_ids: Vec<String> = req.assignee_id.iter().map(Uuid::to_string).collect();
        trigger_webhooks_after_ai_tasks(
            state.clone(),
            TriggerContext {
                event: WebhookEvent::IssueAssigned,
                workspace_id: project.workspace_id,
                project_id,
                actor_id: user_id,
                issue_id: Some(issue_id),
                comment_id: None,
                label_id: None,
                sprint_id: None,
                changes: Some(json!({
                    "assignee_ids": {
                        "old": [],
                        "new": created_assignee_ids,
                        "added": created_assignee_ids,
                        "removed": []
                    }
                })),
                mentions: Vec::new(),
                extra_data: None,
            },
            assignment.ai_task_bots,
        );
    }

    let mut created = load_issue(&state.db, issue_id).await?.ok_or(ApiError::Internal)?;
    attach_page_links(
        &state.db,
        &PageCaller::new(access, project.workspace_id),
        &mut created.issue,
    )
    .await?;
    Ok(ApiResponse::success(created.issue))
}

/// GET /`api/v1/projects/:project_id/issues` - List issues in project
pub async fn list_issues(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    bot: Option<Extension<BotAuthContext>>,
    Path(project_id): Path<Uuid>,
    Query(query): Query<ListIssuesQuery>,
) -> Result<impl IntoResponse, ApiError> {
    let extensions = build_auth_extensions(claims, bot);

    #[derive(Debug, FromQueryResult)]
    struct ProjectWorkspace {
        workspace_id: Uuid,
        key: String,
    }

    let project = ProjectWorkspace::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "SELECT workspace_id, key FROM projects WHERE id = $1",
        vec![project_id.into()],
    ))
    .one(&state.db)
    .await?
    .ok_or_else(|| ApiError::NotFound("project not found".to_string()))?;

    require_workspace_access(&state, &extensions, project.workspace_id).await?;

    let page = query.page.unwrap_or(1).max(1);
    let per_page = query.per_page.unwrap_or(15).clamp(1, 100);
    let offset = (page - 1) * per_page;

    // Build query with filters
    let mut where_clauses = vec!["project_id = $1".to_string()];
    let mut values: Vec<sea_orm::Value> = vec![project_id.into()];
    let mut param_idx = 2;

    if let Some(state_filter) = query.state {
        validate_project_state(&state, project_id, &state_filter).await?;
        where_clauses.push(format!("state = ${param_idx}"));
        values.push(state_filter.into());
        param_idx += 1;
    }

    if let Some(assignee) = query.assignee_id {
        where_clauses.push(format!("assignee_id = ${param_idx}"));
        values.push(assignee.into());
        param_idx += 1;
    }

    if let Some(priority) = query.priority {
        validate_priority(&priority)?;
        where_clauses.push(format!("priority = ${param_idx}"));
        values.push(priority.into());
        param_idx += 1;
    }

    if let Some(sprint) = query.sprint_id {
        if sprint == "none" {
            where_clauses.push("sprint_id IS NULL".to_string());
        } else {
            let sprint_id = Uuid::parse_str(&sprint)
                .map_err(|_| ApiError::BadRequest("sprint_id must be a UUID or none".to_string()))?;
            where_clauses.push(format!("sprint_id = ${param_idx}"));
            values.push(sprint_id.into());
            param_idx += 1;
        }
    }

    if let Some(milestone) = query.milestone_id {
        if milestone == "none" {
            where_clauses.push("milestone_id IS NULL".to_string());
        } else {
            let milestone_id = Uuid::parse_str(&milestone)
                .map_err(|_| ApiError::BadRequest("milestone_id must be a UUID or none".to_string()))?;
            where_clauses.push(format!("milestone_id = ${param_idx}"));
            values.push(milestone_id.into());
            param_idx += 1;
        }
    }

    if let Some(search) = query.search {
        let search_text = search.trim();
        if !search_text.is_empty() {
            where_clauses.push(format!(
                "(title ILIKE ${param_idx} OR description ILIKE ${param_idx} OR CAST(id AS TEXT) ILIKE ${param_idx})"
            ));
            values.push(format!("%{search_text}%").into());
            param_idx += 1;
        }
    }

    if let Some(label_ids_raw) = query.label_ids {
        let mut label_ids: Vec<Uuid> = Vec::new();
        for raw in label_ids_raw
            .split(',')
            .map(str::trim)
            .filter(|value| !value.is_empty())
        {
            let label_id =
                Uuid::parse_str(raw).map_err(|_| ApiError::BadRequest(format!("invalid label id: {raw}")))?;
            if !label_ids.contains(&label_id) {
                label_ids.push(label_id);
            }
        }

        if !label_ids.is_empty() {
            let start_idx = param_idx;
            let placeholders = (start_idx..start_idx + label_ids.len())
                .map(|idx| format!("${idx}"))
                .collect::<Vec<_>>()
                .join(", ");
            where_clauses.push(format!(
                "id IN (
                    SELECT wil.work_item_id
                    FROM work_item_labels wil
                    WHERE wil.label_id IN ({})
                    GROUP BY wil.work_item_id
                    HAVING COUNT(DISTINCT wil.label_id) = {}
                )",
                placeholders,
                label_ids.len()
            ));
            for label_id in label_ids {
                values.push(label_id.into());
                param_idx += 1;
            }
        }
    }

    let sort_by = match query.sort_by.as_deref() {
        Some("created_at") => "created_at",
        Some("priority") => "priority",
        Some("title") => "title",
        _ => "updated_at",
    };
    let sort_order = match query.sort_order.as_deref() {
        Some("asc") => "ASC",
        _ => "DESC",
    };
    let order_by = if sort_by == "priority" {
        format!(
            "CASE priority WHEN 'low' THEN 1 WHEN 'medium' THEN 2 WHEN 'high' THEN 3 WHEN 'urgent' THEN 4 ELSE 5 END {sort_order}"
        )
    } else {
        format!("{sort_by} {sort_order}")
    };
    let where_sql = where_clauses.join(" AND ");

    // Total count
    let count_result = state
        .db
        .query_one(Statement::from_sql_and_values(
            DbBackend::Postgres,
            format!("SELECT COUNT(*) AS count FROM work_items WHERE {where_sql}"),
            values.clone(),
        ))
        .await?
        .ok_or_else(|| ApiError::Internal)?;

    let total: i64 = count_result.try_get("", "count")?;

    let sql = format!(
        "SELECT id, project_id, sequence_number, sprint_id, milestone_id, title, description, state, priority, assignee_id, due_at, created_by, created_at, updated_at, proposal_id, governance_exempt, governance_exempt_reason FROM work_items WHERE {} ORDER BY {} LIMIT ${} OFFSET ${}",
        where_sql,
        order_by,
        param_idx,
        param_idx + 1
    );
    values.push(per_page.into());
    values.push(offset.into());

    #[derive(Debug, FromQueryResult)]
    struct IssueRow {
        id: Uuid,
        project_id: Uuid,
        sequence_number: i32,
        sprint_id: Option<Uuid>,
        milestone_id: Option<Uuid>,
        title: String,
        description: String,
        state: String,
        priority: String,
        assignee_id: Option<Uuid>,
        due_at: Option<platform::timestamp::Timestamp>,
        created_by: Option<Uuid>,
        created_at: platform::timestamp::Timestamp,
        updated_at: platform::timestamp::Timestamp,
        proposal_id: Option<String>,
        governance_exempt: bool,
        governance_exempt_reason: Option<String>,
    }

    let issues = IssueRow::find_by_statement(Statement::from_sql_and_values(DbBackend::Postgres, &sql, values))
        .all(&state.db)
        .await?;

    let issue_ids: Vec<Uuid> = issues.iter().map(|issue| issue.id).collect();
    let mut label_map = issue_labels(&state.db, &issue_ids).await?;
    let link_counts = page_link_counts(&state.db, &issue_ids).await?;

    let response: Vec<IssueResponse> = issues
        .into_iter()
        .map(|i| IssueResponse {
            id: i.id,
            identifier: identifier(&project.key, i.sequence_number),
            project_id: i.project_id,
            sprint_id: i.sprint_id,
            milestone_id: i.milestone_id,
            title: i.title,
            description: i.description,
            state: i.state,
            priority: i.priority,
            assignee_id: i.assignee_id,
            due_at: i.due_at.map(platform::timestamp::Timestamp::to_rfc3339),
            created_by: i.created_by,
            created_at: i.created_at.to_rfc3339(),
            updated_at: i.updated_at.to_rfc3339(),
            proposal_id: i.proposal_id,
            governance_exempt: i.governance_exempt,
            governance_exempt_reason: i.governance_exempt_reason,
            labels: label_map.remove(&i.id).unwrap_or_default(),
            page_links: None,
            page_link_count: Some(link_counts.get(&i.id).copied().unwrap_or(0)),
        })
        .collect();

    let total_pages = if total == 0 {
        0
    } else {
        total / per_page + i64::from(total % per_page != 0)
    };

    Ok(ApiResponse::success(PaginatedData {
        items: response,
        total,
        page,
        per_page,
        total_pages,
    }))
}

/// GET /api/v1/issues/:id - Get issue details
pub async fn get_issue(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    bot: Option<Extension<BotAuthContext>>,
    Path(issue_id): Path<Uuid>,
) -> Result<impl IntoResponse, ApiError> {
    let extensions = build_auth_extensions(claims, bot);

    let loaded = load_issue(&state.db, issue_id)
        .await?
        .ok_or_else(|| ApiError::NotFound("issue not found".to_string()))?;

    let access = require_workspace_access(&state, &extensions, loaded.workspace_id).await?;

    let mut issue = loaded.issue;
    attach_page_links(&state.db, &PageCaller::new(access, loaded.workspace_id), &mut issue).await?;
    Ok(ApiResponse::success(issue))
}

/// PUT /api/v1/issues/:id - Update issue
pub async fn update_issue(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    bot: Option<Extension<BotAuthContext>>,
    Path(issue_id): Path<Uuid>,
    Json(req): Json<UpdateIssueRequest>,
) -> Result<impl IntoResponse, ApiError> {
    let user_id = Uuid::parse_str(&claims.sub).map_err(|_| ApiError::Unauthorized("invalid user id".to_string()))?;
    let extensions = build_auth_extensions(claims, bot);

    let UpdateIssueRequest {
        title: requested_title,
        description: requested_description,
        state: requested_state,
        priority: requested_priority,
        assignee_id: requested_assignee,
        due_at: requested_due_at,
        sprint_id: requested_sprint,
        milestone_id: requested_milestone,
    } = req;
    let requested_title = not_null(requested_title, "title")?;
    let requested_description = not_null(requested_description, "description")?;
    let requested_state = not_null(requested_state, "state")?;
    let requested_priority = not_null(requested_priority, "priority")?;

    // Get current issue values and verify access.
    #[derive(Debug, FromQueryResult)]
    struct CurrentIssue {
        project_id: Uuid,
        workspace_id: Uuid,
        title: String,
        description: String,
        state: String,
        priority: String,
        assignee_id: Option<Uuid>,
        sprint_id: Option<Uuid>,
        milestone_id: Option<Uuid>,
        due_at: Option<platform::timestamp::Timestamp>,
    }

    let current_issue = CurrentIssue::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        r"
            SELECT wi.project_id, p.workspace_id, wi.title, wi.description, wi.state, wi.priority, wi.assignee_id, wi.sprint_id, wi.milestone_id, wi.due_at
            FROM work_items wi
            INNER JOIN projects p ON wi.project_id = p.id
            WHERE wi.id = $1
        ",
        vec![issue_id.into()],
    ))
    .one(&state.db)
    .await?
    .ok_or_else(|| ApiError::NotFound("issue not found".to_string()))?;

    let access = require_workspace_access(&state, &extensions, current_issue.workspace_id).await?;

    // A connected application may not assign a work item to an agent (nothing is written yet).
    if let Some(Some(assignee_id)) = requested_assignee {
        crate::oauth::delegated::refuse_agent_dispatch(&state.db, &[assignee_id]).await?;
    }

    // Validate state and priority if provided
    if let Some(ref state_val) = requested_state {
        validate_project_state(&state, current_issue.project_id, state_val).await?;
    }

    if let Some(ref priority) = requested_priority {
        validate_priority(priority)?;
    }

    // Verify assignee is workspace member if provided
    if let Some(Some(assignee_id)) = requested_assignee {
        let member_exists = state
            .db
            .query_one(Statement::from_sql_and_values(
                DbBackend::Postgres,
                "SELECT 1 FROM workspace_members WHERE workspace_id = $1 AND user_id = $2",
                vec![current_issue.workspace_id.into(), assignee_id.into()],
            ))
            .await?;

        if member_exists.is_none() {
            return Err(ApiError::BadRequest("assignee must be a workspace member".to_string()));
        }
    }

    // Verify the sprint belongs to this issue's project (and therefore this workspace)
    if let Some(Some(sprint_id)) = requested_sprint {
        ensure_sprint_in_project(&state.db, sprint_id, current_issue.project_id).await?;
    }

    let requested_due_at = requested_due_at
        .map(|due| {
            due.map(|due_str| {
                platform::timestamp::Timestamp::parse_rfc3339(&due_str)
                    .map_err(|_| ApiError::BadRequest("invalid due_at format".to_string()))
            })
            .transpose()
        })
        .transpose()?;

    // Build update query
    let mut updates = Vec::new();
    let mut values: Vec<sea_orm::Value> = Vec::new();
    let mut param_idx = 1;

    if let Some(title) = requested_title.clone() {
        if title.trim().is_empty() {
            return Err(ApiError::BadRequest("title cannot be empty".to_string()));
        }
        updates.push(format!("title = ${param_idx}"));
        values.push(title.into());
        param_idx += 1;
    }

    if let Some(description) = requested_description.clone() {
        updates.push(format!("description = ${param_idx}"));
        values.push(description.into());
        param_idx += 1;
    }

    if let Some(state_val) = requested_state.clone() {
        updates.push(format!("state = ${param_idx}"));
        values.push(state_val.into());
        param_idx += 1;
    }

    if let Some(priority) = requested_priority.clone() {
        updates.push(format!("priority = ${param_idx}"));
        values.push(priority.into());
        param_idx += 1;
    }

    if let Some(assignee) = requested_assignee {
        updates.push(format!("assignee_id = ${param_idx}"));
        values.push(assignee.into());
        param_idx += 1;
    }

    if let Some(due_at) = requested_due_at {
        updates.push(format!("due_at = ${param_idx}"));
        values.push(due_at.into());
        param_idx += 1;
    }

    if let Some(sprint_id) = requested_sprint {
        updates.push(format!("sprint_id = ${param_idx}"));
        values.push(sprint_id.into());
        param_idx += 1;
    }

    if let Some(milestone_id) = requested_milestone {
        updates.push(format!("milestone_id = ${param_idx}"));
        values.push(milestone_id.into());
        param_idx += 1;
    }

    if updates.is_empty() {
        return Err(ApiError::BadRequest("no fields to update".to_string()));
    }

    let now = platform::timestamp::Timestamp::now();
    updates.push(format!("updated_at = ${param_idx}"));
    values.push(now.into());
    param_idx += 1;

    values.push(issue_id.into());

    let query = format!("UPDATE work_items SET {} WHERE id = ${}", updates.join(", "), param_idx);

    let tx = state.db.begin().await?;

    // Keeping the milestone the item already has is not an assignment: a closed milestone does
    // not block the other edits of its items.
    if let Some(Some(milestone_id)) = requested_milestone
        && Some(milestone_id) != current_issue.milestone_id
    {
        ensure_open_milestone(&tx, milestone_id, current_issue.workspace_id).await?;
    }

    tx.execute(Statement::from_sql_and_values(DbBackend::Postgres, &query, values))
        .await?;

    // Read the row back inside the transaction, so the answer is exactly what was written.
    let mut updated = load_issue(&tx, issue_id).await?.ok_or(ApiError::Internal)?.issue;

    let assignee_changed = requested_assignee.is_some_and(|new_assignee| new_assignee != current_issue.assignee_id);
    let state_changed = requested_state
        .as_ref()
        .is_some_and(|new_state| *new_state != current_issue.state);

    struct ActivityChange {
        action: &'static str,
        detail: serde_json::Value,
    }

    let mut updated_fields = Map::<String, Value>::new();
    if let Some(new_title) = requested_title.as_ref()
        && new_title != &current_issue.title
    {
        updated_fields.insert(
            "title".to_string(),
            json!({
                "old": current_issue.title.clone(),
                "new": updated.title.clone(),
            }),
        );
    }
    if let Some(new_description) = requested_description.as_ref()
        && new_description != &current_issue.description
    {
        updated_fields.insert(
            "description".to_string(),
            json!({
                "old": current_issue.description.clone(),
                "new": updated.description.clone(),
            }),
        );
    }
    if let Some(new_priority) = requested_priority.as_ref()
        && new_priority != &current_issue.priority
    {
        updated_fields.insert(
            "priority".to_string(),
            json!({
                "old": current_issue.priority.clone(),
                "new": updated.priority.clone(),
            }),
        );
    }

    if let Some(new_sprint_id) = requested_sprint
        && new_sprint_id != current_issue.sprint_id
    {
        updated_fields.insert(
            "sprint_id".to_string(),
            json!({
                "old": current_issue.sprint_id.map(|id| id.to_string()),
                "new": new_sprint_id.map(|id| id.to_string()),
            }),
        );
    }
    if let Some(new_milestone_id) = requested_milestone
        && new_milestone_id != current_issue.milestone_id
    {
        updated_fields.insert(
            "milestone_id".to_string(),
            json!({
                "old": current_issue.milestone_id.map(|id| id.to_string()),
                "new": new_milestone_id.map(|id| id.to_string()),
            }),
        );
    }
    if let Some(new_due_at) = requested_due_at
        && new_due_at != current_issue.due_at
    {
        updated_fields.insert(
            "due_at".to_string(),
            json!({
                "old": current_issue.due_at.map(platform::timestamp::Timestamp::to_rfc3339),
                "new": new_due_at.map(platform::timestamp::Timestamp::to_rfc3339),
            }),
        );
    }

    let mut changes: Vec<ActivityChange> = Vec::new();

    if let Some(new_title) = requested_title.as_ref()
        && new_title != &current_issue.title
    {
        changes.push(ActivityChange {
            action: "title_changed",
            detail: serde_json::json!({ "from": current_issue.title.clone(), "to": updated.title.clone() }),
        });
    }

    // The text of a description can be long: the entry says it changed, the item holds the text.
    if let Some(new_description) = requested_description.as_ref()
        && new_description != &current_issue.description
    {
        changes.push(ActivityChange {
            action: "description_changed",
            detail: serde_json::json!({}),
        });
    }

    if let Some(new_due_at) = requested_due_at
        && new_due_at != current_issue.due_at
    {
        changes.push(ActivityChange {
            action: "due_date_changed",
            detail: serde_json::json!({
                "from": current_issue.due_at.map(platform::timestamp::Timestamp::to_rfc3339),
                "to": new_due_at.map(platform::timestamp::Timestamp::to_rfc3339)
            }),
        });
    }

    if let Some(new_assignee) = requested_assignee
        && new_assignee != current_issue.assignee_id
    {
        changes.push(ActivityChange {
            action: "assigned",
            detail: serde_json::json!({
                "from": current_issue.assignee_id.map(|id| id.to_string()),
                "to": new_assignee.map(|id| id.to_string())
            }),
        });
    }

    if let Some(new_state) = requested_state
        && new_state != current_issue.state
    {
        changes.push(ActivityChange {
            action: "status_changed",
            detail: serde_json::json!({
                "from": current_issue.state.clone(),
                "to": new_state
            }),
        });
    }

    if let Some(new_priority) = requested_priority
        && new_priority != current_issue.priority
    {
        changes.push(ActivityChange {
            action: "priority_changed",
            detail: serde_json::json!({
                "from": current_issue.priority.clone(),
                "to": new_priority
            }),
        });
    }

    if let Some(new_sprint_id) = requested_sprint
        && new_sprint_id != current_issue.sprint_id
    {
        changes.push(ActivityChange {
            action: "sprint_changed",
            detail: serde_json::json!({
                "from": current_issue.sprint_id.map(|id| id.to_string()),
                "to": new_sprint_id.map(|id| id.to_string())
            }),
        });
    }

    if let Some(new_milestone_id) = requested_milestone
        && new_milestone_id != current_issue.milestone_id
    {
        changes.push(ActivityChange {
            action: "milestone_changed",
            detail: serde_json::json!({
                "from": current_issue.milestone_id.map(|id| id.to_string()),
                "to": new_milestone_id.map(|id| id.to_string())
            }),
        });
    }

    let issue_ref = IssueRef {
        id: issue_id,
        project_id: current_issue.project_id,
        workspace_id: current_issue.workspace_id,
    };
    for change in &changes {
        insert_activity(&tx, &issue_ref, user_id, change.action, &change.detail, now).await?;
    }

    if let Some(Some(new_assignee)) = requested_assignee
        && assignee_changed
    {
        notify_assignment(
            &tx,
            NotifyAssignment {
                workspace_id: current_issue.workspace_id,
                project_id: current_issue.project_id,
                issue_id,
                title: &updated.title,
                assignee_id: new_assignee,
                actor_id: user_id,
            },
        )
        .await?;
    }

    // The assignee hears about a change of state or priority made by someone else. A new assignee
    // was just told of the assignment, which already shows the item as it is now.
    if let Some(assignee_id) = updated.assignee_id.filter(|_| !assignee_changed) {
        let base = NotifyChange {
            workspace_id: current_issue.workspace_id,
            project_id: current_issue.project_id,
            issue_id,
            title: updated.title.as_str(),
            assignee_id,
            actor_id: user_id,
            old: "",
            new: "",
        };
        if updated.state != current_issue.state {
            let change = NotifyChange {
                old: &current_issue.state,
                new: &updated.state,
                ..base
            };
            notify_state_change(&tx, change).await?;
        }
        if updated.priority != current_issue.priority {
            let change = NotifyChange {
                old: &current_issue.priority,
                new: &updated.priority,
                ..base
            };
            notify_priority_change(&tx, change).await?;
        }
    }

    tx.commit().await?;

    let to_single_or_empty = |value: Option<Uuid>| value.map(|id| vec![id.to_string()]).unwrap_or_default();

    // A change of assignee to a bot is queued as that bot's AI task before any delivery of this
    // request is triggered; the state change or edit made in the same request is part of the
    // same action and gives the bot no second task.
    let assignment = match updated.assignee_id.filter(|_| assignee_changed) {
        Some(new_assignee) => {
            queue_assignment_ai_task(
                &state.db,
                AssignmentTaskInput {
                    project_id: current_issue.project_id,
                    issue_id,
                    assignee_id: new_assignee,
                    actor_id: user_id,
                    trigger: "issue.update_assignee",
                    // Every assignment is its own task: reassigning A -> B -> A gives A a new
                    // task the second time, which a key of issue and assignee alone would
                    // silently drop as a duplicate of the first.
                    idempotency_key: format!("issue_assigned:{issue_id}:{new_assignee}:{}", Uuid::new_v4()),
                },
            )
            .await?
        }
        None => AssignmentOutcome::default(),
    };

    if assignee_changed {
        trigger_webhooks_after_ai_tasks(
            state.clone(),
            TriggerContext {
                event: WebhookEvent::IssueAssigned,
                workspace_id: current_issue.workspace_id,
                project_id: current_issue.project_id,
                actor_id: user_id,
                issue_id: Some(issue_id),
                comment_id: None,
                label_id: None,
                sprint_id: None,
                changes: Some(json!({
                    "assignee_ids": {
                        "old": to_single_or_empty(current_issue.assignee_id),
                        "new": to_single_or_empty(updated.assignee_id),
                        "added": to_single_or_empty(updated.assignee_id.filter(|id| Some(*id) != current_issue.assignee_id)),
                        "removed": to_single_or_empty(current_issue.assignee_id.filter(|id| Some(*id) != updated.assignee_id)),
                    }
                })),
                mentions: Vec::new(),
                extra_data: None,
            },
            assignment.ai_task_bots.clone(),
        );
    }

    if state_changed {
        trigger_webhooks_after_ai_tasks(
            state.clone(),
            TriggerContext {
                event: WebhookEvent::IssueStateChanged,
                workspace_id: current_issue.workspace_id,
                project_id: current_issue.project_id,
                actor_id: user_id,
                issue_id: Some(issue_id),
                comment_id: None,
                label_id: None,
                sprint_id: None,
                changes: Some(json!({
                    "state": {
                        "old": current_issue.state.clone(),
                        "new": updated.state.clone(),
                    }
                })),
                mentions: Vec::new(),
                extra_data: None,
            },
            assignment.ai_task_bots.clone(),
        );
    }

    if !updated_fields.is_empty() {
        trigger_webhooks_after_ai_tasks(
            state.clone(),
            TriggerContext {
                event: WebhookEvent::IssueUpdated,
                workspace_id: current_issue.workspace_id,
                project_id: current_issue.project_id,
                actor_id: user_id,
                issue_id: Some(issue_id),
                comment_id: None,
                label_id: None,
                sprint_id: None,
                changes: Some(json!(updated_fields)),
                mentions: Vec::new(),
                extra_data: None,
            },
            assignment.ai_task_bots,
        );
    }

    attach_page_links(
        &state.db,
        &PageCaller::new(access, current_issue.workspace_id),
        &mut updated,
    )
    .await?;
    Ok(ApiResponse::success(updated))
}

/// DELETE /api/v1/issues/:id - Delete issue
pub async fn delete_issue(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    bot: Option<Extension<BotAuthContext>>,
    Path(issue_id): Path<Uuid>,
) -> Result<impl IntoResponse, ApiError> {
    let user_id = Uuid::parse_str(&claims.sub).map_err(|_| ApiError::Unauthorized("invalid user id".to_string()))?;
    let extensions = build_auth_extensions(claims, bot);

    // Get issue context and check permission
    #[derive(Debug, FromQueryResult)]
    struct IssueWorkspace {
        project_id: Uuid,
        workspace_id: Uuid,
    }

    let issue_ws = IssueWorkspace::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        r"
            SELECT wi.project_id, p.workspace_id
            FROM work_items wi
            INNER JOIN projects p ON wi.project_id = p.id
            WHERE wi.id = $1
        ",
        vec![issue_id.into()],
    ))
    .one(&state.db)
    .await?
    .ok_or_else(|| ApiError::NotFound("issue not found".to_string()))?;

    let (_, role, _) = require_workspace_access(&state, &extensions, issue_ws.workspace_id).await?;
    if role != "owner" && role != "admin" {
        return Err(ApiError::Forbidden(
            "only owners and admins can delete issues".to_string(),
        ));
    }

    // The event carries the issue as it was, labels included, so it is read before the delete.
    let (deleted_issue_payload, _) = issue_snapshot(&state.db, issue_id)
        .await?
        .ok_or_else(|| ApiError::NotFound("issue not found".to_string()))?;

    state
        .db
        .execute(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "DELETE FROM work_items WHERE id = $1",
            vec![issue_id.into()],
        ))
        .await?;

    trigger_webhooks(
        state.clone(),
        TriggerContext {
            event: WebhookEvent::IssueDeleted,
            workspace_id: issue_ws.workspace_id,
            project_id: issue_ws.project_id,
            actor_id: user_id,
            issue_id: Some(issue_id),
            comment_id: None,
            label_id: None,
            sprint_id: None,
            changes: None,
            mentions: Vec::new(),
            extra_data: Some(deleted_issue_payload),
        },
    );

    Ok(ApiResponse::ok())
}

/// Query of `GET /api/v1/issues/by-identifier/:identifier`.
#[derive(Debug, Deserialize)]
pub struct ByIdentifierQuery {
    /// Narrows the lookup to one workspace; needed when a user's workspaces share a project key.
    pub workspace_id: Option<Uuid>,
}

/// GET /api/v1/issues/by-identifier/:identifier
/// Get a work item by its short identifier, e.g. "PRX-42".
///
/// A bot or a connected application looks in its own workspace. A user looks in the workspaces
/// they are a member of: one match answers, none is 404, and several answer 409 so the caller
/// passes `workspace_id`.
pub async fn get_issue_by_identifier(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    bot: Option<Extension<BotAuthContext>>,
    Path(identifier): Path<String>,
    Query(query): Query<ByIdentifierQuery>,
) -> Result<impl IntoResponse, ApiError> {
    // Parse "PREFIX-N"
    let (prefix, seq) = identifier
        .rsplit_once('-')
        .ok_or_else(|| ApiError::BadRequest("identifier must be in format PREFIX-N, e.g. PRX-42".to_string()))?;

    let seq_num: i64 = seq
        .parse()
        .map_err(|_| ApiError::BadRequest(format!("sequence number '{seq}' is not a valid integer")))?;

    let not_found = || ApiError::NotFound(format!("work item '{identifier}' not found"));

    #[derive(Debug, FromQueryResult)]
    struct Located {
        id: Uuid,
        workspace_id: Uuid,
    }

    let fixed_workspace = match (crate::oauth::delegated::current(), bot.as_ref()) {
        (Some(delegated), _) => Some(delegated.workspace_id),
        (None, Some(Extension(bot_ctx))) => Some(bot_ctx.workspace_id),
        (None, None) => None,
    };
    let located = if let Some(workspace_id) = fixed_workspace {
        if query.workspace_id.is_some_and(|requested| requested != workspace_id) {
            return Err(not_found());
        }
        Located::find_by_statement(Statement::from_sql_and_values(
            DbBackend::Postgres,
            r"
                SELECT wi.id, p.workspace_id
                FROM work_items wi
                INNER JOIN projects p ON wi.project_id = p.id
                WHERE p.workspace_id = $1
                  AND UPPER(p.key) = UPPER($2)
                  AND wi.sequence_number = $3
            ",
            vec![workspace_id.into(), prefix.into(), seq_num.into()],
        ))
        .one(&state.db)
        .await?
        .ok_or_else(not_found)?
    } else {
        let user_id =
            Uuid::parse_str(&claims.sub).map_err(|_| ApiError::Unauthorized("invalid user id".to_string()))?;
        let mut matches = Located::find_by_statement(Statement::from_sql_and_values(
            DbBackend::Postgres,
            r"
                SELECT wi.id, p.workspace_id
                FROM work_items wi
                INNER JOIN projects p ON wi.project_id = p.id
                INNER JOIN workspace_members wm ON wm.workspace_id = p.workspace_id AND wm.user_id = $1
                WHERE UPPER(p.key) = UPPER($2)
                  AND wi.sequence_number = $3
                  AND ($4::uuid IS NULL OR p.workspace_id = $4)
                LIMIT 2
            ",
            vec![user_id.into(), prefix.into(), seq_num.into(), query.workspace_id.into()],
        ))
        .all(&state.db)
        .await?;
        if matches.len() > 1 {
            return Err(ApiError::Conflict(format!(
                "identifier '{identifier}' matches work items in several of your workspaces; pass workspace_id"
            )));
        }
        matches.pop().ok_or_else(not_found)?
    };

    let extensions = build_auth_extensions(claims, bot);
    let access = require_workspace_access(&state, &extensions, located.workspace_id).await?;

    let mut issue = load_issue(&state.db, located.id).await?.ok_or_else(not_found)?.issue;
    attach_page_links(&state.db, &PageCaller::new(access, located.workspace_id), &mut issue).await?;

    Ok(ApiResponse::success(issue))
}

/// POST /`api/v1/issues/:issue_id/labels/batch`
/// Add multiple labels to a work item in one request.
#[derive(Debug, serde::Deserialize)]
pub struct AddLabelsRequest {
    pub label_ids: Vec<Uuid>,
}

pub async fn add_labels_to_issue(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    bot: Option<Extension<BotAuthContext>>,
    Path(issue_id): Path<Uuid>,
    Json(payload): Json<AddLabelsRequest>,
) -> Result<impl IntoResponse, ApiError> {
    if payload.label_ids.is_empty() {
        return Err(ApiError::BadRequest("label_ids must not be empty".to_string()));
    }
    let user_id = Uuid::parse_str(&claims.sub).map_err(|_| ApiError::Unauthorized("invalid user id".to_string()))?;
    let extensions = build_auth_extensions(claims, bot);

    #[derive(Debug, FromQueryResult)]
    struct IssueWorkspace {
        workspace_id: Uuid,
        project_id: Uuid,
    }

    let owner = IssueWorkspace::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "SELECT p.workspace_id, wi.project_id FROM work_items wi INNER JOIN projects p ON wi.project_id = p.id WHERE wi.id = $1",
        vec![issue_id.into()],
    ))
    .one(&state.db)
    .await?
    .ok_or_else(|| ApiError::NotFound("work item not found or access denied".to_string()))?;
    require_workspace_access(&state, &extensions, owner.workspace_id).await?;

    let added = attach_labels_to_issue(&state.db, issue_id, owner.workspace_id, user_id, &payload.label_ids).await?;

    // One `label.added` for each label that was really attached; one that was already there is not
    // a change.
    for label_id in added {
        trigger_webhooks(
            state.clone(),
            TriggerContext {
                event: WebhookEvent::LabelAdded,
                workspace_id: owner.workspace_id,
                project_id: owner.project_id,
                actor_id: user_id,
                issue_id: Some(issue_id),
                comment_id: None,
                label_id: Some(label_id),
                sprint_id: None,
                changes: None,
                mentions: Vec::new(),
                extra_data: None,
            },
        );
    }

    Ok(ApiResponse::ok())
}

/// Attaches labels to a work item, refusing any label outside the caller's workspace.
///
/// The single-label sibling, `label::add_label_to_issue`, already checks
/// `SELECT 1 FROM labels WHERE id = $1 AND workspace_id = $2`; the batch path checked only the work
/// item and inserted every `label_id` unchecked, so a bot token could pull another tenant's label
/// onto its own issue and then read that label's name, colour and description back out through
/// `GET /issues/{id}/labels`. This applies the same predicate to every id in the batch.
///
/// Every label is validated before any row is written, and the writes then run inside one
/// transaction, so a rejected batch leaves `work_item_labels` untouched.
async fn attach_labels_to_issue(
    db: &sea_orm::DatabaseConnection,
    issue_id: Uuid,
    workspace_id: Uuid,
    actor_id: Uuid,
    label_ids: &[Uuid],
) -> Result<Vec<Uuid>, ApiError> {
    // Verify issue belongs to the caller's workspace
    let exists = db
        .query_one(Statement::from_sql_and_values(
            DbBackend::Postgres,
            r"
                SELECT 1 AS exists
                FROM work_items wi
                INNER JOIN projects p ON wi.project_id = p.id
                WHERE wi.id = $1 AND p.workspace_id = $2
            ",
            vec![issue_id.into(), workspace_id.into()],
        ))
        .await?;
    if exists.is_none() {
        return Err(ApiError::NotFound("work item not found or access denied".to_string()));
    }

    // Verify every label belongs to the same workspace, before anything is written.
    for label_id in label_ids {
        let label_exists = db
            .query_one(Statement::from_sql_and_values(
                DbBackend::Postgres,
                "SELECT 1 FROM labels WHERE id = $1 AND workspace_id = $2",
                vec![(*label_id).into(), workspace_id.into()],
            ))
            .await?;
        if label_exists.is_none() {
            return Err(ApiError::NotFound("label not found in workspace".to_string()));
        }
    }

    // Insert all label associations; the ones already attached are left alone and not reported.
    let tx = db.begin().await?;
    let mut added = Vec::new();
    for label_id in label_ids {
        let inserted = tx
            .query_one(Statement::from_sql_and_values(
                DbBackend::Postgres,
                "INSERT INTO work_item_labels (work_item_id, label_id) VALUES ($1, $2) \
                 ON CONFLICT DO NOTHING RETURNING label_id",
                vec![issue_id.into(), (*label_id).into()],
            ))
            .await?;
        if inserted.is_some() && !added.contains(label_id) {
            record_label_change(&tx, issue_id, actor_id, *label_id, true).await?;
            added.push(*label_id);
        }
    }
    tx.commit().await?;

    Ok(added)
}

#[cfg(test)]
mod tenant_scoping_tests {
    use super::{attach_labels_to_issue, ensure_sprint_in_project};
    use crate::error::ApiError;
    use crate::routes::context::tenant_fixture::{count, exec, seed_label, seed_tenant, seed_work_item};
    use crate::scratch_or_skip;
    use uuid::Uuid;

    /// V4. The batch label route checked that the work item belonged to the bot's workspace and
    /// then inserted every `label_id` unchecked, so a bot token could pull another tenant's label
    /// onto its own issue and read that label's name, colour and description back through
    /// `GET /issues/{id}/labels`. The single-label sibling in `label.rs` always had the predicate.
    ///
    /// The batch here mixes a legitimate label with a foreign one: a rejected batch must write
    /// neither, so the check has to happen before any insert rather than per row.
    #[tokio::test]
    async fn a_label_from_another_workspace_cannot_be_batch_attached() {
        let scratch = scratch_or_skip!("v4_batch_labels");
        let a = seed_tenant(&scratch.db, "a").await;
        let b = seed_tenant(&scratch.db, "b").await;

        let my_issue = seed_work_item(&scratch.db, &a, "my issue").await;
        let my_label = seed_label(&scratch.db, &a, "mine").await;
        let their_label = seed_label(&scratch.db, &b, "their secret label").await;

        let result = attach_labels_to_issue(
            &scratch.db,
            my_issue,
            a.workspace_id,
            a.member_id,
            &[my_label, their_label],
        )
        .await;
        match &result {
            Err(ApiError::NotFound(_)) => {}
            other => panic!("a foreign label in the batch should answer NotFound, got {other:?}"),
        }

        assert_eq!(
            count(
                &scratch.db,
                "SELECT COUNT(*)::bigint AS count FROM work_item_labels",
                vec![],
            )
            .await,
            0,
            "a rejected batch must write no association at all, not even the legitimate label"
        );

        scratch.drop_self().await;
    }

    /// A batch of the caller's own labels still applies.
    #[tokio::test]
    async fn labels_from_the_same_workspace_still_batch_attach() {
        let scratch = scratch_or_skip!("v4_batch_labels_ok");
        let a = seed_tenant(&scratch.db, "a").await;

        let my_issue = seed_work_item(&scratch.db, &a, "my issue").await;
        let first = seed_label(&scratch.db, &a, "first").await;
        let second = seed_label(&scratch.db, &a, "second").await;

        attach_labels_to_issue(&scratch.db, my_issue, a.workspace_id, a.member_id, &[first, second])
            .await
            .expect("labels of the caller's own workspace attach");

        assert_eq!(
            count(
                &scratch.db,
                "SELECT COUNT(*)::bigint AS count FROM work_item_labels WHERE work_item_id = $1",
                vec![my_issue.into()],
            )
            .await,
            2
        );

        scratch.drop_self().await;
    }

    /// V5. Neither `create_issue` nor `update_issue` looked at `sprint_id`, while the sibling
    /// field `assignee_id` has always been checked against `workspace_members`. An unchecked
    /// `sprint_id` leaves a cross-tenant dangling reference that the other tenant can rewrite
    /// through `ON DELETE SET NULL`, and turns the route into an existence oracle over `sprints`.
    #[tokio::test]
    async fn a_sprint_from_another_workspace_is_refused() {
        let scratch = scratch_or_skip!("v5_sprint_scope");
        let a = seed_tenant(&scratch.db, "a").await;
        let b = seed_tenant(&scratch.db, "b").await;

        let their_sprint = Uuid::new_v4();
        exec(
            &scratch.db,
            "INSERT INTO sprints (id, project_id, name) VALUES ($1, $2, 'their sprint')",
            vec![their_sprint.into(), b.project_id.into()],
        )
        .await;

        let result = ensure_sprint_in_project(&scratch.db, their_sprint, a.project_id).await;
        match &result {
            Err(ApiError::BadRequest(_)) => {}
            other => panic!("another tenant's sprint should be refused, got {other:?}"),
        }

        scratch.drop_self().await;
    }

    /// A sprint that does not exist answers exactly what a sprint in another tenant answers, so
    /// the check does not become an oracle of its own.
    #[tokio::test]
    async fn a_missing_sprint_is_indistinguishable_from_a_foreign_one() {
        let scratch = scratch_or_skip!("v5_sprint_no_oracle");
        let a = seed_tenant(&scratch.db, "a").await;
        let b = seed_tenant(&scratch.db, "b").await;

        let their_sprint = Uuid::new_v4();
        exec(
            &scratch.db,
            "INSERT INTO sprints (id, project_id, name) VALUES ($1, $2, 'their sprint')",
            vec![their_sprint.into(), b.project_id.into()],
        )
        .await;

        let foreign = ensure_sprint_in_project(&scratch.db, their_sprint, a.project_id).await;
        let missing = ensure_sprint_in_project(&scratch.db, Uuid::new_v4(), a.project_id).await;

        assert_eq!(
            format!("{foreign:?}"),
            format!("{missing:?}"),
            "a foreign sprint and a missing one must answer identically"
        );

        scratch.drop_self().await;
    }

    /// A sprint of the issue's own project still passes.
    #[tokio::test]
    async fn a_sprint_in_the_same_project_is_accepted() {
        let scratch = scratch_or_skip!("v5_sprint_ok");
        let a = seed_tenant(&scratch.db, "a").await;

        let my_sprint = Uuid::new_v4();
        exec(
            &scratch.db,
            "INSERT INTO sprints (id, project_id, name) VALUES ($1, $2, 'my sprint')",
            vec![my_sprint.into(), a.project_id.into()],
        )
        .await;

        ensure_sprint_in_project(&scratch.db, my_sprint, a.project_id)
            .await
            .expect("a sprint of the issue's own project is accepted");

        scratch.drop_self().await;
    }
}

#[cfg(test)]
mod assignment_rule_tests {
    use super::assignment_queues_ai_task;
    use uuid::Uuid;

    #[test]
    fn only_a_bot_assigned_by_someone_else_gets_an_assignment_task() {
        let actor = Uuid::new_v4();
        let assignee = Uuid::new_v4();
        assert!(assignment_queues_ai_task(actor, assignee, true));
        assert!(!assignment_queues_ai_task(actor, assignee, false), "a person");
        assert!(
            !assignment_queues_ai_task(assignee, assignee, true),
            "a bot assigning itself"
        );
    }
}

/// Bot tasks produced by the issue routes against a migrated scratch database: worker AI tasks
/// queued, and webhook deliveries that carry `bot_context`. Each scenario counts both, so a
/// duplicate on either channel fails it.
///
/// The webhooks point at a loopback address, which outbound validation refuses; the refused
/// delivery is still recorded with its full payload, which is what these tests read.
#[cfg(test)]
#[allow(clippy::expect_used, clippy::panic)]
mod bot_task_dispatch_tests {
    use super::{CreateIssueRequest, UpdateIssueRequest, create_issue, update_issue};
    use crate::routes::context::tenant_fixture::{Tenant, count, exec, seed_bot_user, seed_tenant, seed_work_item};
    use crate::scratch_or_skip;
    use axum::extract::{Extension, Json, Path, State};
    use platform::auth::{JwtClaims, TokenType};
    use sea_orm::DatabaseConnection;
    use std::time::Duration;
    use uuid::Uuid;

    const EVENTS: &str = r#"["issue.created","issue.assigned","issue.updated","issue.state_changed"]"#;

    fn state_for(db: DatabaseConnection) -> platform::app::AppState {
        use platform::config::{AppConfig, Secret};
        platform::app::AppState {
            cfg: AppConfig {
                app_name: "bot-task-dispatch-test".to_string(),
                bind_addr: "127.0.0.1:0".to_string(),
                database_url: Secret::new("postgres://unused/unused"),
                jwt_secret: Secret::new("bot-task-dispatch-test-secret"),
                jwt_access_ttl_seconds: 900,
                jwt_refresh_ttl_seconds: 3600,
                allow_insecure_cookies: false,
                collab_allowed_origins: Vec::new(),
            },
            db,
            flow_permission_cache: platform::app::FlowPermissionCacheSlot::default(),
        }
    }

    fn claims(user: Uuid) -> JwtClaims {
        JwtClaims {
            sub: user.to_string(),
            email: format!("{user}@tenant.test"),
            token_type: TokenType::Access,
            iat: 0,
            exp: 0,
            sv: 0,
        }
    }

    struct Fixture {
        tenant: Tenant,
        bot_a: Uuid,
        bot_b: Uuid,
        human: Uuid,
    }

    /// A tenant with two bots, each with its own webhook, and a second person.
    async fn fixture(db: &DatabaseConnection, tag: &str) -> Fixture {
        let tenant = seed_tenant(db, tag).await;
        let bot_a = seed_bot_user(db, &tenant).await;
        let bot_b = seed_bot_user(db, &tenant).await;
        let human = Uuid::new_v4();
        exec(
            db,
            "INSERT INTO users (id, email, password_hash, name, role, is_active) VALUES ($1, $2, '!', 'h', 'user', true)",
            vec![human.into(), format!("{human}@tenant.test").into()],
        )
        .await;
        exec(
            db,
            "INSERT INTO workspace_members (workspace_id, user_id, role) VALUES ($1, $2, 'member')",
            vec![tenant.workspace_id.into(), human.into()],
        )
        .await;
        for bot in [bot_a, bot_b] {
            exec(
                db,
                "INSERT INTO webhooks (id, workspace_id, name, url, secret, events, active, bot_user_id, created_by) \
                 VALUES ($1, $2, 'receiver', 'http://127.0.0.1:9/webhook', 'test-secret', $3::jsonb, true, $4, $5)",
                vec![
                    Uuid::new_v4().into(),
                    tenant.workspace_id.into(),
                    EVENTS.into(),
                    bot.into(),
                    tenant.member_id.into(),
                ],
            )
            .await;
        }
        Fixture {
            tenant,
            bot_a,
            bot_b,
            human,
        }
    }

    async fn assign_directly(db: &DatabaseConnection, issue: Uuid, assignee: Uuid) {
        exec(
            db,
            "UPDATE work_items SET assignee_id = $2 WHERE id = $1",
            vec![issue.into(), assignee.into()],
        )
        .await;
    }

    /// Waits until the workspace has recorded `expected` deliveries (they are sent from spawned
    /// tasks), then gives stragglers a moment so an extra delivery would be counted too.
    async fn settle_deliveries(db: &DatabaseConnection, workspace: Uuid, expected: i64) {
        let sql = "SELECT COUNT(*)::bigint AS count FROM webhook_deliveries d \
                   JOIN webhooks w ON w.id = d.webhook_id WHERE w.workspace_id = $1";
        for _ in 0..200 {
            if count(db, sql, vec![workspace.into()]).await >= expected {
                break;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        tokio::time::sleep(Duration::from_millis(300)).await;
        assert_eq!(
            count(db, sql, vec![workspace.into()]).await,
            expected,
            "deliveries recorded"
        );
    }

    async fn webhook_bot_tasks(db: &DatabaseConnection, bot: Uuid) -> i64 {
        count(
            db,
            "SELECT COUNT(*)::bigint AS count FROM webhook_deliveries d JOIN webhooks w ON w.id = d.webhook_id \
             WHERE w.bot_user_id = $1 AND d.payload->'bot_context' IS NOT NULL \
               AND d.payload->'bot_context' <> 'null'::jsonb",
            vec![bot.into()],
        )
        .await
    }

    async fn ai_tasks(db: &DatabaseConnection, bot: Uuid) -> i64 {
        count(
            db,
            "SELECT COUNT(*)::bigint AS count FROM ai_tasks WHERE ai_participant_id = $1",
            vec![bot.into()],
        )
        .await
    }

    async fn bot_tasks(db: &DatabaseConnection, bot: Uuid) -> i64 {
        ai_tasks(db, bot).await + webhook_bot_tasks(db, bot).await
    }

    fn update(assignee: Option<Uuid>, state: Option<&str>) -> UpdateIssueRequest {
        UpdateIssueRequest {
            title: None,
            description: None,
            state: state.map(|value| Some(value.to_string())),
            priority: None,
            assignee_id: assignee.map(Some),
            due_at: None,
            sprint_id: None,
            milestone_id: None,
        }
    }

    async fn put(state: &platform::app::AppState, actor: Uuid, issue: Uuid, req: UpdateIssueRequest) {
        update_issue(
            State(state.clone()),
            Extension(claims(actor)),
            None,
            Path(issue),
            Json(req),
        )
        .await
        .unwrap_or_else(|err| panic!("update_issue failed: {err:?}"));
    }

    /// Before the fix this was three runs: `issue.created` and `issue.assigned` with
    /// `bot_context`, plus the AI task.
    #[tokio::test]
    async fn create_with_a_bot_assignee_is_exactly_one_bot_task() {
        let scratch = scratch_or_skip!("bot_task_create");
        let db = &scratch.db;
        let f = fixture(db, "bc").await;
        let state = state_for(db.clone());

        create_issue(
            State(state),
            Extension(claims(f.tenant.member_id)),
            None,
            Path(f.tenant.project_id),
            Json(CreateIssueRequest {
                title: "for the bot".to_string(),
                description: None,
                state: None,
                priority: None,
                assignee_id: Some(f.bot_a),
                due_at: None,
                sprint_id: None,
                milestone_id: None,
            }),
        )
        .await
        .unwrap_or_else(|err| panic!("create_issue failed: {err:?}"));

        // issue.created and issue.assigned, to each of the two webhooks.
        settle_deliveries(db, f.tenant.workspace_id, 4).await;
        assert_eq!(ai_tasks(db, f.bot_a).await, 1);
        assert_eq!(bot_tasks(db, f.bot_a).await, 1, "one assignment, one bot task");
        assert_eq!(bot_tasks(db, f.bot_b).await, 0);
        scratch.drop_self().await;
    }

    /// Before the fix this was two runs: `issue.assigned` with `bot_context` plus the AI task.
    /// An assignment made together with a state change is still one task.
    #[tokio::test]
    async fn assigning_later_and_reassigning_are_one_bot_task_each() {
        let scratch = scratch_or_skip!("bot_task_assign");
        let db = &scratch.db;
        let f = fixture(db, "ba").await;
        let state = state_for(db.clone());
        let issue = seed_work_item(db, &f.tenant, "later").await;

        put(
            &state,
            f.tenant.member_id,
            issue,
            update(Some(f.bot_a), Some("in_progress")),
        )
        .await;
        // issue.assigned and issue.state_changed, to each webhook.
        settle_deliveries(db, f.tenant.workspace_id, 4).await;
        assert_eq!(bot_tasks(db, f.bot_a).await, 1, "assign later");

        put(&state, f.tenant.member_id, issue, update(Some(f.bot_b), None)).await;
        settle_deliveries(db, f.tenant.workspace_id, 6).await;
        assert_eq!(bot_tasks(db, f.bot_b).await, 1, "reassigned bot");
        assert_eq!(bot_tasks(db, f.bot_a).await, 1, "the previous bot gets nothing new");

        put(&state, f.tenant.member_id, issue, update(Some(f.bot_a), None)).await;
        settle_deliveries(db, f.tenant.workspace_id, 8).await;
        assert_eq!(
            bot_tasks(db, f.bot_a).await,
            2,
            "assigned back: a new task, not a dropped duplicate"
        );
        scratch.drop_self().await;
    }

    #[tokio::test]
    async fn assigning_a_person_is_no_bot_task() {
        let scratch = scratch_or_skip!("bot_task_human");
        let db = &scratch.db;
        let f = fixture(db, "bh").await;
        let state = state_for(db.clone());
        let issue = seed_work_item(db, &f.tenant, "human").await;
        assign_directly(db, issue, f.bot_a).await;

        put(&state, f.tenant.member_id, issue, update(Some(f.human), None)).await;
        settle_deliveries(db, f.tenant.workspace_id, 2).await;
        assert_eq!(bot_tasks(db, f.bot_a).await, 0);
        assert_eq!(bot_tasks(db, f.bot_b).await, 0);
        assert_eq!(
            count(db, "SELECT COUNT(*)::bigint AS count FROM ai_tasks", vec![]).await,
            0
        );
        scratch.drop_self().await;
    }

    /// The bot's own change to done used to come back to it as a `status_changed` bot task.
    #[tokio::test]
    async fn a_bot_changing_its_own_issue_gets_no_task() {
        let scratch = scratch_or_skip!("bot_task_self");
        let db = &scratch.db;
        let f = fixture(db, "bs").await;
        let state = state_for(db.clone());
        let issue = seed_work_item(db, &f.tenant, "mine").await;
        assign_directly(db, issue, f.bot_a).await;

        put(&state, f.bot_a, issue, update(None, Some("done"))).await;
        settle_deliveries(db, f.tenant.workspace_id, 2).await;
        assert_eq!(bot_tasks(db, f.bot_a).await, 0, "own state change");

        // A bot assigning an issue to itself is its own action too.
        let claimed = seed_work_item(db, &f.tenant, "claimed").await;
        put(&state, f.bot_a, claimed, update(Some(f.bot_a), None)).await;
        settle_deliveries(db, f.tenant.workspace_id, 4).await;
        assert_eq!(bot_tasks(db, f.bot_a).await, 0, "self assignment");
        scratch.drop_self().await;
    }

    #[tokio::test]
    async fn others_changing_a_bot_issue_still_reach_the_bot() {
        let scratch = scratch_or_skip!("bot_task_others");
        let db = &scratch.db;
        let f = fixture(db, "bo").await;
        let state = state_for(db.clone());
        let issue = seed_work_item(db, &f.tenant, "theirs").await;
        assign_directly(db, issue, f.bot_a).await;

        put(&state, f.human, issue, update(None, Some("in_progress"))).await;
        settle_deliveries(db, f.tenant.workspace_id, 2).await;
        assert_eq!(webhook_bot_tasks(db, f.bot_a).await, 1, "a person's state change");

        put(&state, f.bot_b, issue, update(None, Some("done"))).await;
        settle_deliveries(db, f.tenant.workspace_id, 4).await;
        assert_eq!(webhook_bot_tasks(db, f.bot_a).await, 2, "another bot's state change");
        assert_eq!(bot_tasks(db, f.bot_b).await, 0, "the acting bot is not the assignee");
        assert_eq!(ai_tasks(db, f.bot_a).await, 0);
        scratch.drop_self().await;
    }
}
