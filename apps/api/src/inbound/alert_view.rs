//! `GET /api/v1/issues/{id}/alert`: the read-only facts of an alert work item.
//!
//! Any member of the work item's workspace may read them (a bot token with read permission
//! included, like `GET /api/v1/issues/{id}`). The route is not on the delegated allow-list, so a
//! connected application's token gets 403 from the authentication layer. Nothing a source or a
//! rule holds in secret is shown: no token, signing secret, instructions or event payload.

use axum::{
    Extension,
    extract::{Path, State},
    response::IntoResponse,
};
use platform::{app::AppState, auth::JwtClaims, timestamp::Timestamp};
use sea_orm::{DbBackend, FromQueryResult, Statement};
use serde::Serialize;
use uuid::Uuid;

use super::{queue::QUEUE_TTL, severity::Severity};
use crate::{
    error::ApiError,
    middleware::bot_auth::{BotAuthContext, require_workspace_access_from_auth},
    response::ApiResponse,
};

#[derive(FromQueryResult)]
struct Row {
    workspace_id: Uuid,
    alert_id: Option<Uuid>,
    source_id: Option<Uuid>,
    source_name: Option<String>,
    kind: Option<String>,
    max_severity_rank: Option<i16>,
    occurrences: Option<i32>,
    first_seen_at: Option<Timestamp>,
    last_seen_at: Option<Timestamp>,
    open: Option<bool>,
    rule_id: Option<Uuid>,
    rule_name: Option<String>,
    task_id: Option<Uuid>,
    task_status: Option<String>,
    task_state: Option<String>,
    task_queued_at: Option<Timestamp>,
}

#[derive(Serialize)]
struct SourceView {
    id: Uuid,
    name: String,
}

#[derive(Serialize)]
struct RuleView {
    id: Uuid,
    name: String,
}

#[derive(Serialize)]
struct TaskView {
    id: Uuid,
    /// The status of the task itself: `pending`, `processing`, `completed`, `failed` and so on.
    status: String,
}

#[derive(Serialize)]
struct AlertView {
    issue_id: Uuid,
    alert_id: Uuid,
    source: Option<SourceView>,
    kind: Option<String>,
    /// The highest severity any occurrence reached.
    severity: Option<&'static str>,
    occurrences: i32,
    first_seen_at: Timestamp,
    last_seen_at: Timestamp,
    /// Whether new occurrences still join this work item.
    open: bool,
    rule: Option<RuleView>,
    /// The alert's latest bot task, when one was started.
    task: Option<TaskView>,
    /// `none`, `created`, `waiting` (queued behind a safety limit) or `suppressed`.
    task_state: String,
    /// When a waiting task was queued, and when it will be suppressed if it has not started.
    queued_at: Option<Timestamp>,
    queue_expires_at: Option<Timestamp>,
}

pub async fn get_issue_alert(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    bot: Option<Extension<BotAuthContext>>,
    Path(issue_id): Path<Uuid>,
) -> Result<impl IntoResponse, ApiError> {
    let row = Row::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "SELECT p.workspace_id, a.id AS alert_id, a.source_id, s.name AS source_name, \
                (SELECT e.kind FROM inbound_events e WHERE e.alert_id = a.id ORDER BY e.received_at LIMIT 1) AS kind, \
                a.max_severity_rank, a.occurrences, a.first_seen_at, a.last_seen_at, a.open, \
                a.rule_id, r.name AS rule_name, a.task_id, t.status AS task_status, a.task_state, a.task_queued_at \
           FROM work_items w \
           JOIN projects p ON p.id = w.project_id \
           LEFT JOIN inbound_alerts a ON a.work_item_id = w.id \
           LEFT JOIN inbound_sources s ON s.id = a.source_id \
           LEFT JOIN alert_rules r ON r.id = a.rule_id \
           LEFT JOIN ai_tasks t ON t.id = a.task_id \
          WHERE w.id = $1",
        vec![issue_id.into()],
    ))
    .one(&state.db)
    .await?
    .ok_or_else(|| ApiError::NotFound("issue not found".to_owned()))?;

    // Access is decided before the answer says whether the work item is an alert at all.
    require_workspace_access_from_auth(&state, &claims, bot.as_ref().map(|b| &b.0), row.workspace_id).await?;

    let (Some(alert_id), Some(first_seen_at), Some(last_seen_at)) = (row.alert_id, row.first_seen_at, row.last_seen_at)
    else {
        return Err(ApiError::NotFound("issue is not an alert".to_owned()));
    };
    let task_state = row.task_state.unwrap_or_else(|| "none".to_owned());
    let queued_at = if task_state == "waiting" {
        row.task_queued_at
    } else {
        None
    };
    Ok(ApiResponse::success(AlertView {
        issue_id,
        alert_id,
        source: row
            .source_id
            .zip(row.source_name)
            .map(|(id, name)| SourceView { id, name }),
        kind: row.kind,
        severity: row
            .max_severity_rank
            .and_then(Severity::from_rank)
            .map(Severity::as_str),
        occurrences: row.occurrences.unwrap_or(1),
        first_seen_at,
        last_seen_at,
        open: row.open.unwrap_or(false),
        rule: row.rule_id.zip(row.rule_name).map(|(id, name)| RuleView { id, name }),
        task: row
            .task_id
            .zip(row.task_status)
            .map(|(id, status)| TaskView { id, status }),
        task_state,
        queued_at,
        queue_expires_at: queued_at.map(|at| at + QUEUE_TTL),
    }))
}
