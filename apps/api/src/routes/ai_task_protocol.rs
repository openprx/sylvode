//! The task protocol: the endpoints a bot, or the runner acting for it, uses to read, acknowledge
//! and report on its tasks.
//!
//! Every `/api/v1/ai/tasks/...` endpoint here requires the task's own bot. A person (a system
//! administrator included) and a connected application are refused: none of these routes is on
//! the delegated allow-list, and [`ensure_task_bot`] refuses a request without a bot context.

use axum::{
    Extension, Json,
    body::Bytes,
    extract::{Path, Query, State},
    http::StatusCode,
    response::{IntoResponse, Response},
};
use platform::{app::AppState, auth::JwtClaims};
use sea_orm::TransactionTrait;
use serde::Deserialize;
use serde_json::{Value, json};
use uuid::Uuid;

use crate::{
    error::ApiError,
    middleware::bot_auth::BotAuthContext,
    oauth::rate::{RateBucket, RateError, RateKey, enforce},
    response::ApiResponse,
    services::{
        ai_task_reply::{
            Artifact, CompletionInput, Outcome, ReplyReport, RoutedReply, route_completion, route_failure,
            send_reply_triggers,
        },
        ai_task_service::{
            AiTaskRow, AttemptEnd, LeaseLimits, ProgressKind, TASK_PROTOCOL, acknowledge_task, cas_end_attempt,
            complete_active_attempt, insert_ai_task_business_event, insert_ai_task_event, is_active_status,
            list_active_tasks_of_bot, load_task_with_protocol, not_active_for_attempt, notify_ai_task_completed,
            notify_ai_task_failed, report_progress,
        },
        ai_task_view::build_task_view,
        bot_connection_status::{self, MAX_REPORT_BYTES, RUNNER_REPORT_INTERVAL_SECS},
    },
};

/// The most tasks `GET /ai/tasks/active` answers with.
const ACTIVE_TASK_LIMIT: i64 = 100;
const MAX_PLAN_CHARS: usize = 4000;
const MAX_NOTE_CHARS: usize = 1000;
const MAX_QUEUE_POSITION: i32 = 10_000;

/// The `?attempt=N` every task endpoint requires.
#[derive(Debug, Default, Deserialize)]
pub struct TaskAttemptQuery {
    pub attempt: Option<i32>,
}

impl TaskAttemptQuery {
    fn required(&self) -> Result<i32, ApiError> {
        self.attempt
            .ok_or_else(|| ApiError::BadRequest("attempt is required".to_string()))
    }
}

#[derive(Debug, Deserialize)]
pub struct AckRequest {
    pub plan: String,
}

#[derive(Debug, Deserialize)]
pub struct ProgressRequest {
    #[serde(default)]
    pub note: String,
    #[serde(default)]
    pub percent: Option<i32>,
    #[serde(default)]
    pub kind: Option<String>,
    #[serde(default)]
    pub queue_position: Option<i32>,
}

/// Refuses everything but the task's own bot.
///
/// That is a person, a system administrator, another bot, and a request without a bot context (a
/// connected application never has one). The refusal is the 404 of a task that does not exist, so
/// a foreign task id cannot be told from an unknown one.
pub fn ensure_task_bot(task: &AiTaskRow, bot: Option<&BotAuthContext>, claims: &JwtClaims) -> Result<(), ApiError> {
    let own =
        bot.is_some_and(|bot| bot.bot_id == task.ai_participant_id) && claims.sub == task.ai_participant_id.to_string();
    if own {
        Ok(())
    } else {
        // The same answer as for a task that does not exist: whether a task id exists is not
        // something a bot that does not own it may learn.
        Err(ApiError::NotFound("ai task not found".to_string()))
    }
}

fn lease_limits() -> LeaseLimits {
    LeaseLimits::from_config(&crate::config::runtime().ai_tasks)
}

/// `GET /api/v1/ai/protocol`: the protocol version, the features and the lease settings.
#[allow(clippy::unused_async, reason = "axum handlers are async functions")]
pub async fn get_protocol() -> impl IntoResponse {
    let config = crate::config::runtime().ai_tasks;
    ApiResponse::success(json!({
        "task_protocol": TASK_PROTOCOL,
        "api_version": env!("CARGO_PKG_VERSION"),
        "features": ["ack", "lease", "reply_routing", "runner_phase", "runner_report"],
        "runner_report_interval_secs": RUNNER_REPORT_INTERVAL_SECS,
        "lease": {
            "processing_timeout_secs": config.processing_timeout_secs,
            "lease_extension_secs": config.lease_extension_secs,
            "max_processing_secs": config.max_processing_secs,
        },
    }))
}

/// `PUT /api/v1/ai/runner/report`: the runner's self-report.
///
/// Only the bot of a project bot connection may report: a person is refused, and so is a bot that
/// no connection made. The body is reduced to component names, states and versions before it is
/// stored (see [`bot_connection_status::sanitize_report`]); a body over 16 KiB is refused with 413.
pub async fn put_runner_report(
    State(state): State<AppState>,
    bot: Option<Extension<BotAuthContext>>,
    body: Bytes,
) -> Result<Response, ApiError> {
    let Some(Extension(bot)) = bot else {
        return Err(ApiError::Forbidden("only a runner bot can report".to_string()));
    };
    // Each call is a row update: a runner reports every ten minutes, so a dozen a minute is far
    // more than it needs.
    match enforce(&state.db, RateBucket::RunnerReport, &RateKey::from_id(bot.bot_id)).await {
        Ok(()) => {}
        Err(RateError::Limited(limited)) => {
            let mut response = (
                StatusCode::TOO_MANY_REQUESTS,
                ApiResponse::<()>::error(429, "too many reports; wait before reporting again"),
            )
                .into_response();
            if let Ok(value) = limited.retry_after_secs.to_string().parse() {
                response.headers_mut().insert(axum::http::header::RETRY_AFTER, value);
            }
            return Ok(response);
        }
        Err(RateError::Store(error)) => {
            tracing::warn!(error = %error, "runner report rate counter unavailable");
            return Err(ApiError::Internal);
        }
    }
    if body.len() > MAX_REPORT_BYTES {
        return Ok((
            StatusCode::PAYLOAD_TOO_LARGE,
            ApiResponse::<()>::error(413, "the report is larger than 16 KiB"),
        )
            .into_response());
    }
    let report: Value = serde_json::from_slice(&body)
        .map_err(|_| ApiError::BadRequest("the report must be a JSON object".to_string()))?;
    if !report.is_object() {
        return Err(ApiError::BadRequest("the report must be a JSON object".to_string()));
    }
    bot_connection_status::store_report(&state.db, bot.bot_id, &bot_connection_status::sanitize_report(&report))
        .await?;
    Ok(ApiResponse::success(json!({ "accepted": true, "interval_secs": RUNNER_REPORT_INTERVAL_SECS })).into_response())
}

/// The report route's body limit of [`MAX_REPORT_BYTES`], applied before the body is read, so a
/// large body is cut off instead of buffered up to the global limit.
pub const fn runner_report_body_limit() -> axum::extract::DefaultBodyLimit {
    axum::extract::DefaultBodyLimit::max(MAX_REPORT_BYTES)
}

/// `GET /api/v1/ai/tasks/active`: the calling bot's own active tasks, oldest first.
pub async fn list_active_tasks(
    State(state): State<AppState>,
    bot: Option<Extension<BotAuthContext>>,
) -> Result<impl IntoResponse, ApiError> {
    let Some(Extension(bot)) = bot else {
        return Err(ApiError::Forbidden(
            "active tasks can only be listed by a bot".to_string(),
        ));
    };
    let tasks = list_active_tasks_of_bot(&state.db, bot.bot_id, ACTIVE_TASK_LIMIT).await?;
    let mut views = Vec::with_capacity(tasks.len());
    for entry in &tasks {
        views.push(build_task_view(&state.db, &entry.task, &entry.protocol).await?);
    }
    Ok(ApiResponse::success(views))
}

/// `GET /api/v1/ai/tasks/{task_id}?attempt=N`: the task view. A task that ended at attempt `N`
/// is answered with `"active": false` so an agent can tell it is done; any other attempt or
/// state is a 400.
pub async fn get_task(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    bot: Option<Extension<BotAuthContext>>,
    Path(task_id): Path<Uuid>,
    Query(query): Query<TaskAttemptQuery>,
) -> Result<impl IntoResponse, ApiError> {
    let entry = load_task_with_protocol(&state.db, task_id).await?;
    ensure_task_bot(&entry.task, bot.as_ref().map(|bot| &bot.0), &claims)?;
    let attempt = query.required()?;
    let task = &entry.task;
    let ended_here = matches!(task.status.as_str(), "completed" | "failed") && task.attempts == attempt;
    let active_here = is_active_status(&task.status) && task.attempts == attempt;
    if !active_here && !ended_here {
        return Err(ApiError::BadRequest(format!(
            "task is not active for attempt {attempt} (current attempt {})",
            task.attempts
        )));
    }
    Ok(ApiResponse::success(
        build_task_view(&state.db, task, &entry.protocol).await?,
    ))
}

/// `POST /api/v1/ai/tasks/{task_id}/ack?attempt=N`: the agent has the task and says how it will
/// proceed.
pub async fn ack_task(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    bot: Option<Extension<BotAuthContext>>,
    Path(task_id): Path<Uuid>,
    Query(query): Query<TaskAttemptQuery>,
    Json(request): Json<AckRequest>,
) -> Result<impl IntoResponse, ApiError> {
    let entry = load_task_with_protocol(&state.db, task_id).await?;
    ensure_task_bot(&entry.task, bot.as_ref().map(|bot| &bot.0), &claims)?;
    let attempt = query.required()?;
    let plan = request.plan.trim();
    if plan.is_empty() || plan.chars().count() > MAX_PLAN_CHARS {
        return Err(ApiError::BadRequest(format!(
            "plan must be 1 to {MAX_PLAN_CHARS} characters"
        )));
    }
    let outcome = acknowledge_task(
        &state.db,
        task_id,
        attempt,
        plan,
        entry.task.ai_participant_id,
        lease_limits(),
    )
    .await?;
    tracing::debug!(task_id = %task_id, attempt, ?outcome, "task acknowledged");
    let entry = load_task_with_protocol(&state.db, task_id).await?;
    Ok(ApiResponse::success(json!({
        "task": build_task_view(&state.db, &entry.task, &entry.protocol).await?,
        "proceed": true,
    })))
}

/// `POST /api/v1/ai/tasks/{task_id}/progress?attempt=N`: a progress report from the agent or the
/// runner. It extends the lease.
pub async fn report_task_progress(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    bot: Option<Extension<BotAuthContext>>,
    Path(task_id): Path<Uuid>,
    Query(query): Query<TaskAttemptQuery>,
    Json(request): Json<ProgressRequest>,
) -> Result<impl IntoResponse, ApiError> {
    let entry = load_task_with_protocol(&state.db, task_id).await?;
    ensure_task_bot(&entry.task, bot.as_ref().map(|bot| &bot.0), &claims)?;
    let attempt = query.required()?;
    if request.note.chars().count() > MAX_NOTE_CHARS {
        return Err(ApiError::BadRequest(format!(
            "note must be at most {MAX_NOTE_CHARS} characters"
        )));
    }
    if request.percent.is_some_and(|percent| !(0..=100).contains(&percent)) {
        return Err(ApiError::BadRequest("percent must be between 0 and 100".to_string()));
    }
    if request
        .queue_position
        .is_some_and(|position| !(0..=MAX_QUEUE_POSITION).contains(&position))
    {
        return Err(ApiError::BadRequest(format!(
            "queue_position must be between 0 and {MAX_QUEUE_POSITION}"
        )));
    }
    let kind = match request.kind.as_deref() {
        None => ProgressKind::Agent,
        Some(kind) => ProgressKind::parse(kind).ok_or_else(|| {
            ApiError::BadRequest(
                "kind must be agent, runner.queued, runner.pushed, runner.alive, runner.reminded or runner.blocked"
                    .to_string(),
            )
        })?,
    };
    let payload: Value = json!({
        "note": request.note,
        "percent": request.percent,
        "queue_position": request.queue_position,
        "kind": kind.event_type(),
        "attempt": attempt,
    });
    let outcome = report_progress(&state.db, task_id, attempt, kind, payload, lease_limits()).await?;
    Ok(ApiResponse::success(json!({
        "lease_expires_at": outcome.lease_expires_at.to_rfc3339(),
        "coalesced": outcome.coalesced,
    })))
}

const MAX_SUMMARY_CHARS: usize = 20_000;
const MAX_ARTIFACTS: usize = 20;
const MAX_ARTIFACT_REF_CHARS: usize = 500;
const MAX_ARTIFACT_LABEL_CHARS: usize = 200;
const MAX_VOTE_REASON_CHARS: usize = 4000;
const MAX_FAIL_REASON_CHARS: usize = 2000;
const ARTIFACT_KINDS: [&str; 5] = ["commit", "branch", "pull_request", "file", "url"];

#[derive(Debug, Deserialize)]
pub struct ArtifactRequest {
    pub kind: String,
    #[serde(rename = "ref")]
    pub reference: String,
    #[serde(default)]
    pub label: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct CompleteRequest {
    pub outcome: String,
    pub summary: String,
    #[serde(default)]
    pub artifacts: Vec<ArtifactRequest>,
    #[serde(default)]
    pub vote: Option<String>,
    #[serde(default)]
    pub vote_reason: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct FailRequest {
    pub reason: String,
    #[serde(default)]
    pub retryable: bool,
    #[serde(default)]
    pub source: Option<String>,
}

fn bad(message: impl Into<String>) -> ApiError {
    ApiError::BadRequest(message.into())
}

impl CompleteRequest {
    fn validate(self) -> Result<CompletionInput, ApiError> {
        let outcome =
            Outcome::parse(&self.outcome).ok_or_else(|| bad("outcome must be done, needs_review or no_change"))?;
        let summary = self.summary.trim();
        if summary.is_empty() || summary.chars().count() > MAX_SUMMARY_CHARS {
            return Err(bad(format!("summary must be 1 to {MAX_SUMMARY_CHARS} characters")));
        }
        if self.artifacts.len() > MAX_ARTIFACTS {
            return Err(bad(format!("artifacts must have at most {MAX_ARTIFACTS} items")));
        }
        let mut artifacts = Vec::with_capacity(self.artifacts.len());
        for artifact in self.artifacts {
            if !ARTIFACT_KINDS.contains(&artifact.kind.as_str()) {
                return Err(bad("artifact kind must be commit, branch, pull_request, file or url"));
            }
            if artifact.reference.is_empty() || artifact.reference.chars().count() > MAX_ARTIFACT_REF_CHARS {
                return Err(bad(format!(
                    "artifact ref must be 1 to {MAX_ARTIFACT_REF_CHARS} characters"
                )));
            }
            if artifact
                .label
                .as_deref()
                .is_some_and(|label| label.chars().count() > MAX_ARTIFACT_LABEL_CHARS)
            {
                return Err(bad(format!(
                    "artifact label must be at most {MAX_ARTIFACT_LABEL_CHARS} characters"
                )));
            }
            artifacts.push(Artifact {
                kind: artifact.kind,
                reference: artifact.reference,
                label: artifact.label,
            });
        }
        if let Some(vote) = self.vote.as_deref()
            && !matches!(vote, "yes" | "no" | "abstain")
        {
            return Err(bad("vote must be yes, no or abstain"));
        }
        if self
            .vote_reason
            .as_deref()
            .is_some_and(|reason| reason.chars().count() > MAX_VOTE_REASON_CHARS)
        {
            return Err(bad(format!(
                "vote_reason must be at most {MAX_VOTE_REASON_CHARS} characters"
            )));
        }
        Ok(CompletionInput {
            outcome,
            summary: summary.to_string(),
            artifacts,
            vote: self.vote,
            vote_reason: self.vote_reason,
        })
    }
}

/// `POST /api/v1/ai/tasks/{task_id}/complete?attempt=N`: ends the task as completed and writes the
/// reply where the task came from, in the same transaction.
pub async fn complete_task_with_reply(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    bot: Option<Extension<BotAuthContext>>,
    Path(task_id): Path<Uuid>,
    Query(query): Query<TaskAttemptQuery>,
    Json(request): Json<CompleteRequest>,
) -> Result<impl IntoResponse, ApiError> {
    let entry = load_task_with_protocol(&state.db, task_id).await?;
    ensure_task_bot(&entry.task, bot.as_ref().map(|bot| &bot.0), &claims)?;
    let attempt = query.required()?;
    let input = request.validate()?;
    let bot_id = entry.task.ai_participant_id;

    let result = json!({
        "outcome": input.outcome.as_str(),
        "summary": input.summary,
        "artifacts": input.artifacts,
        "vote": input.vote,
        "vote_reason": input.vote_reason,
    });
    let txn = state.db.begin().await?;
    let Some(ended) = complete_active_attempt(&txn, task_id, attempt, &result).await? else {
        let current = load_task_with_protocol(&txn, task_id).await?.task;
        // The attempt already completed: saying the same thing again is the harmless "not active",
        // saying something else is a conflict with the stored result, which the first call decided.
        if current.status == "completed" && current.attempts == attempt && current.result.as_ref() != Some(&result) {
            return Err(ApiError::Conflict(
                "the task was already completed with a different result".to_string(),
            ));
        }
        return Err(not_active_for_attempt(attempt, current.attempts));
    };
    let (task, previous_status) = (ended.task, ended.previous_status);
    let routed = route_completion(&state, &txn, &task, &input).await?;

    insert_ai_task_event(
        &txn,
        task_id,
        "completed",
        json!({ "result": result, "ack_skipped": previous_status == "processing" }),
    )
    .await?;
    insert_ai_task_event(&txn, task_id, "reply_routed", json!(routed.report)).await?;
    insert_ai_task_business_event(
        &txn,
        &task,
        "ai_task.completed",
        Some(bot_id),
        json!({ "type": "ai_task_protocol", "actor_id": bot_id }),
        json!({ "result": task.result, "reply": routed.report }),
        Some(previous_status.as_str()),
    )
    .await?;
    txn.commit().await?;

    finish_response(&state, task, routed, None).await
}

/// `POST /api/v1/ai/tasks/{task_id}/fail?attempt=N`: ends the attempt as failed, or re-queues the
/// task when `retryable` and attempts remain, and reports a final failure where the task came from.
pub async fn fail_task_with_reply(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    bot: Option<Extension<BotAuthContext>>,
    Path(task_id): Path<Uuid>,
    Query(query): Query<TaskAttemptQuery>,
    Json(request): Json<FailRequest>,
) -> Result<impl IntoResponse, ApiError> {
    let entry = load_task_with_protocol(&state.db, task_id).await?;
    ensure_task_bot(&entry.task, bot.as_ref().map(|bot| &bot.0), &claims)?;
    let attempt = query.required()?;
    let reason = request.reason.trim();
    if reason.is_empty() || reason.chars().count() > MAX_FAIL_REASON_CHARS {
        return Err(bad(format!("reason must be 1 to {MAX_FAIL_REASON_CHARS} characters")));
    }
    let source = match request.source.as_deref() {
        None | Some("agent") => "agent",
        Some("runner") => "runner",
        Some(_) => return Err(bad("source must be agent or runner")),
    };
    let end = if request.retryable {
        AttemptEnd::RetryWhileAttemptsRemain
    } else {
        AttemptEnd::Terminal
    };
    let bot_id = entry.task.ai_participant_id;

    let txn = state.db.begin().await?;
    let Some(ended) = cas_end_attempt(&txn, task_id, attempt, end, reason).await? else {
        let current = load_task_with_protocol(&txn, task_id).await?.task;
        // As for a completion: the same reason again is "not active", another reason is a conflict.
        if current.status == "failed" && current.attempts == attempt && current.error_message.as_deref() != Some(reason)
        {
            return Err(ApiError::Conflict(
                "the task already failed with a different reason".to_string(),
            ));
        }
        return Err(not_active_for_attempt(attempt, current.attempts));
    };
    let (task, previous_status) = (ended.task, ended.previous_status);
    let will_retry = task.status == "pending";
    let event_payload = json!({
        "error_message": reason,
        "source": source,
        "attempts": task.attempts,
        "max_attempts": task.max_attempts,
        "next_retry_at": task.next_retry_at.map(platform::timestamp::Timestamp::to_rfc3339),
    });
    insert_ai_task_event(
        &txn,
        task_id,
        if will_retry { "retried" } else { "failed" },
        event_payload.clone(),
    )
    .await?;
    insert_ai_task_business_event(
        &txn,
        &task,
        if will_retry {
            "ai_task.retried"
        } else {
            "ai_task.failed"
        },
        Some(bot_id),
        json!({ "type": "ai_task_protocol", "actor_id": bot_id }),
        event_payload,
        Some(previous_status.as_str()),
    )
    .await?;
    let routed = if will_retry {
        RoutedReply::default()
    } else {
        let routed = route_failure(&txn, &task, "Could not complete", reason).await?;
        insert_ai_task_event(&txn, task_id, "reply_routed", json!(routed.report)).await?;
        routed
    };
    txn.commit().await?;

    finish_response(&state, task, routed, Some(will_retry)).await
}

/// After the commit: the webhooks, then the answer with the task view. `will_retry` is `Some` for
/// a fail.
async fn finish_response(
    state: &AppState,
    task: AiTaskRow,
    routed: RoutedReply,
    will_retry: Option<bool>,
) -> Result<Json<ApiResponse<Value>>, ApiError> {
    let bot_id = task.ai_participant_id;
    match task.status.as_str() {
        "completed" => notify_ai_task_completed(state, &task, bot_id).await?,
        "failed" => notify_ai_task_failed(state, &task, bot_id).await?,
        _ => {}
    }
    send_reply_triggers(state, routed.triggers);
    let entry = load_task_with_protocol(&state.db, task.id).await?;
    let view = build_task_view(&state.db, &entry.task, &entry.protocol).await?;
    let report: ReplyReport = routed.report;
    Ok(ApiResponse::success(will_retry.map_or_else(
        || json!({ "task": view, "reply": report }),
        |will_retry| json!({ "task": view, "will_retry": will_retry, "reply": report }),
    )))
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::panic, clippy::too_many_lines, clippy::indexing_slicing)]
mod tests {
    use super::{
        AckRequest, CompleteRequest, FailRequest, ProgressRequest, TaskAttemptQuery, ack_task,
        complete_task_with_reply, ensure_task_bot, fail_task_with_reply, get_protocol, get_task, list_active_tasks,
        report_task_progress,
    };
    use crate::error::ApiError;
    use crate::middleware::bot_auth::BotAuthContext;
    use crate::routes::context::tenant_fixture::{
        Tenant, bot_context_for, claims_for, count, exec, seed_bot_user, seed_member, seed_tenant, state_for,
    };
    use crate::scratch_or_skip;
    use crate::services::ai_task_fixture::seed_task;
    use axum::{
        Extension, Json,
        extract::{Path, Query, State},
        response::IntoResponse,
    };
    use platform::auth::JwtClaims;
    use sea_orm::DatabaseConnection;
    use serde_json::{Value, json};
    use uuid::Uuid;

    async fn json_of(answer: Result<impl IntoResponse, ApiError>) -> Result<Value, ApiError> {
        let response = answer?.into_response();
        let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("the body reads");
        Ok(serde_json::from_slice(&bytes).expect("the body is JSON"))
    }

    fn data(value: &Value) -> &Value {
        &value["data"]
    }

    fn bad_request(answer: &Result<Value, ApiError>) -> &str {
        match answer {
            Err(ApiError::BadRequest(message)) => message,
            other => panic!("expected a 400, got {other:?}"),
        }
    }

    /// The one refusal for a task the caller does not own: the same as for a task that does not exist.
    fn not_found(answer: &Result<Value, ApiError>) -> bool {
        matches!(answer, Err(ApiError::NotFound(message)) if message == "ai task not found")
    }

    /// Who calls: the task's bot, another bot of the workspace, a member, a system administrator.
    #[derive(Clone, Copy, Debug)]
    enum Caller {
        TaskBot,
        OtherBot,
        Member,
        SystemAdmin,
    }

    struct World {
        tenant: Tenant,
        bot: Uuid,
        other_bot: Uuid,
        member: Uuid,
        admin: Uuid,
    }

    impl World {
        async fn new(db: &DatabaseConnection, tag: &str) -> Self {
            let tenant = seed_tenant(db, tag).await;
            let bot = seed_bot_user(db, &tenant).await;
            let other_bot = seed_bot_user(db, &tenant).await;
            let member = seed_member(db, &tenant, "owner").await;
            let admin = seed_member(db, &tenant, "member").await;
            exec(db, "UPDATE users SET role = 'admin' WHERE id = $1", vec![admin.into()]).await;
            Self {
                tenant,
                bot,
                other_bot,
                member,
                admin,
            }
        }

        fn identity(&self, caller: Caller) -> (JwtClaims, Option<BotAuthContext>) {
            let ws = self.tenant.workspace_id;
            match caller {
                Caller::TaskBot => (
                    claims_for(self.bot),
                    Some(bot_context_for(self.bot, ws, &["read", "write"])),
                ),
                Caller::OtherBot => (
                    claims_for(self.other_bot),
                    Some(bot_context_for(self.other_bot, ws, &["read", "write"])),
                ),
                Caller::Member => (claims_for(self.member), None),
                Caller::SystemAdmin => (claims_for(self.admin), None),
            }
        }
    }

    fn attempt(n: Option<i32>) -> Query<TaskAttemptQuery> {
        Query(TaskAttemptQuery { attempt: n })
    }

    async fn get(
        db: &DatabaseConnection,
        w: &World,
        caller: Caller,
        task: Uuid,
        n: Option<i32>,
    ) -> Result<Value, ApiError> {
        let (claims, bot) = w.identity(caller);
        json_of(
            get_task(
                State(state_for(db.clone())),
                Extension(claims),
                bot.map(Extension),
                Path(task),
                attempt(n),
            )
            .await,
        )
        .await
    }

    async fn ack(
        db: &DatabaseConnection,
        w: &World,
        caller: Caller,
        task: Uuid,
        n: Option<i32>,
        plan: &str,
    ) -> Result<Value, ApiError> {
        let (claims, bot) = w.identity(caller);
        json_of(
            ack_task(
                State(state_for(db.clone())),
                Extension(claims),
                bot.map(Extension),
                Path(task),
                attempt(n),
                Json(AckRequest { plan: plan.to_string() }),
            )
            .await,
        )
        .await
    }

    async fn progress(
        db: &DatabaseConnection,
        w: &World,
        caller: Caller,
        task: Uuid,
        n: Option<i32>,
        body: Value,
    ) -> Result<Value, ApiError> {
        let (claims, bot) = w.identity(caller);
        let request: ProgressRequest = serde_json::from_value(body).expect("a progress body");
        json_of(
            report_task_progress(
                State(state_for(db.clone())),
                Extension(claims),
                bot.map(Extension),
                Path(task),
                attempt(n),
                Json(request),
            )
            .await,
        )
        .await
    }

    async fn complete(
        db: &DatabaseConnection,
        w: &World,
        caller: Caller,
        task: Uuid,
        n: Option<i32>,
    ) -> Result<Value, ApiError> {
        complete_with(db, w, caller, task, n, "s").await
    }

    async fn complete_with(
        db: &DatabaseConnection,
        w: &World,
        caller: Caller,
        task: Uuid,
        n: Option<i32>,
        summary: &str,
    ) -> Result<Value, ApiError> {
        let (claims, bot) = w.identity(caller);
        json_of(
            complete_task_with_reply(
                State(state_for(db.clone())),
                Extension(claims),
                bot.map(Extension),
                Path(task),
                attempt(n),
                Json(CompleteRequest {
                    outcome: "done".to_string(),
                    summary: summary.to_string(),
                    artifacts: Vec::new(),
                    vote: None,
                    vote_reason: None,
                }),
            )
            .await,
        )
        .await
    }

    async fn fail(
        db: &DatabaseConnection,
        w: &World,
        caller: Caller,
        task: Uuid,
        n: Option<i32>,
    ) -> Result<Value, ApiError> {
        fail_with(db, w, caller, task, n, "r").await
    }

    async fn fail_with(
        db: &DatabaseConnection,
        w: &World,
        caller: Caller,
        task: Uuid,
        n: Option<i32>,
        reason: &str,
    ) -> Result<Value, ApiError> {
        let (claims, bot) = w.identity(caller);
        json_of(
            fail_task_with_reply(
                State(state_for(db.clone())),
                Extension(claims),
                bot.map(Extension),
                Path(task),
                attempt(n),
                Json(FailRequest {
                    reason: reason.to_string(),
                    retryable: false,
                    source: None,
                }),
            )
            .await,
        )
        .await
    }

    async fn active(db: &DatabaseConnection, w: &World, caller: Caller) -> Result<Value, ApiError> {
        let (_, bot) = w.identity(caller);
        json_of(list_active_tasks(State(state_for(db.clone())), bot.map(Extension)).await).await
    }

    #[tokio::test]
    async fn only_the_tasks_own_bot_may_operate_it() {
        let scratch = scratch_or_skip!("aitp_authorisation");
        let db = &scratch.db;
        let w = World::new(db, "au").await;
        let task = seed_task(db, &w.tenant, w.bot, "processing", 1, 0).await;

        // A task of another bot is indistinguishable from a task that does not exist, on every
        // endpoint and for every caller that is not the task's bot: one answer, no oracle.
        let unknown = Uuid::new_v4();
        for caller in [Caller::OtherBot, Caller::Member, Caller::SystemAdmin] {
            for id in [task, unknown] {
                assert!(
                    not_found(&get(db, &w, caller, id, Some(1)).await),
                    "get {caller:?} {id}"
                );
                assert!(
                    not_found(&ack(db, &w, caller, id, Some(1), "plan").await),
                    "ack {caller:?} {id}"
                );
                assert!(
                    not_found(&progress(db, &w, caller, id, Some(1), json!({"note": "x"})).await),
                    "progress {caller:?} {id}"
                );
                assert!(
                    not_found(&complete(db, &w, caller, id, Some(1)).await),
                    "complete {caller:?} {id}"
                );
                assert!(
                    not_found(&fail(db, &w, caller, id, Some(1)).await),
                    "fail {caller:?} {id}"
                );
            }
        }
        for caller in [Caller::Member, Caller::SystemAdmin] {
            assert!(active(db, &w, caller).await.is_err(), "active list {caller:?}");
        }
        // Nothing changed.
        let task_row = get(db, &w, Caller::TaskBot, task, Some(1))
            .await
            .expect("the bot reads");
        assert_eq!(data(&task_row)["status"], "processing");

        assert_eq!(
            data(&get(db, &w, Caller::TaskBot, task, Some(1)).await.expect("get"))["task_id"],
            task.to_string()
        );
        assert!(ack(db, &w, Caller::TaskBot, task, Some(1), "plan").await.is_ok());
        assert!(
            progress(db, &w, Caller::TaskBot, task, Some(1), json!({"note": "x"}))
                .await
                .is_ok()
        );
        assert!(active(db, &w, Caller::TaskBot).await.is_ok());

        // The protocol answer is open to a bot and to a person.
        let answer = json_of(Ok::<_, ApiError>(get_protocol().await))
            .await
            .expect("protocol");
        assert_eq!(data(&answer)["task_protocol"], 1);
        assert_eq!(
            data(&answer)["features"],
            json!(["ack", "lease", "reply_routing", "runner_phase", "runner_report"])
        );
        assert_eq!(data(&answer)["runner_report_interval_secs"], 600);
        assert_eq!(data(&answer)["api_version"], env!("CARGO_PKG_VERSION"));
        assert_eq!(data(&answer)["lease"]["lease_extension_secs"], 1800);
        assert_eq!(data(&answer)["lease"]["max_processing_secs"], 86400);
        assert_eq!(data(&answer)["lease"]["processing_timeout_secs"], 3600);
        scratch.drop_self().await;
    }

    /// A connected application has no bot context: the guard refuses it even if its token
    /// somehow reached a handler.
    #[tokio::test]
    async fn a_delegated_request_is_refused_by_the_guard() {
        use crate::middleware::bot_auth::EventSurface;
        use crate::oauth::delegated::{DelegatedAuthContext, with_context};
        use crate::oauth::scopes::{ProjectScope, ScopeSet, TokenResource};

        let scratch = scratch_or_skip!("aitp_delegated");
        let db = &scratch.db;
        let w = World::new(db, "dl").await;
        let task = seed_task(db, &w.tenant, w.bot, "processing", 1, 0).await;
        let entry = crate::services::ai_task_service::load_task_with_protocol(db, task)
            .await
            .expect("task loads");
        let delegated = DelegatedAuthContext {
            grant_id: Uuid::new_v4(),
            token_id: Uuid::new_v4(),
            client_id: std::sync::Arc::from("ramflux"),
            user_id: w.member,
            workspace_id: w.tenant.workspace_id,
            scopes: ScopeSet::all(),
            projects: ProjectScope::All,
            resource: TokenResource::Api,
            surface: EventSurface::Rest,
            tool_name: None,
            request_id: Uuid::new_v4(),
        };
        let refused = with_context(delegated, async {
            ensure_task_bot(&entry.task, None, &claims_for(w.bot))
        })
        .await;
        assert!(matches!(refused, Err(ApiError::NotFound(_))));
        assert!(
            ensure_task_bot(
                &entry.task,
                Some(&bot_context_for(w.bot, w.tenant.workspace_id, &["read"])),
                &claims_for(w.bot)
            )
            .is_ok()
        );
        scratch.drop_self().await;
    }

    /// A repeat of an end the task already has is the harmless "not active" answer when it says the
    /// same thing, and a conflict when it says something else: the stored result is the first one.
    #[tokio::test]
    async fn a_repeated_end_with_other_content_is_a_conflict() {
        let scratch = scratch_or_skip!("aitp_repeat_conflict");
        let db = &scratch.db;
        let w = World::new(db, "rc").await;
        let completed = seed_task(db, &w.tenant, w.bot, "processing", 1, 0).await;
        complete_with(db, &w, Caller::TaskBot, completed, Some(1), "first summary")
            .await
            .expect("the first completion");
        let same = complete_with(db, &w, Caller::TaskBot, completed, Some(1), "first summary").await;
        assert!(
            matches!(&same, Err(ApiError::BadRequest(message)) if message.contains("not active")),
            "{same:?}"
        );
        let other = complete_with(db, &w, Caller::TaskBot, completed, Some(1), "another summary").await;
        assert!(
            matches!(&other, Err(ApiError::Conflict(message)) if message.contains("different result")),
            "{other:?}"
        );
        // A repeat for another attempt is just not active.
        let stale = complete_with(db, &w, Caller::TaskBot, completed, Some(2), "another summary").await;
        assert!(matches!(&stale, Err(ApiError::BadRequest(_))), "{stale:?}");

        let failed = seed_task(db, &w.tenant, w.bot, "processing", 1, 0).await;
        fail_with(db, &w, Caller::TaskBot, failed, Some(1), "first reason")
            .await
            .expect("the first failure");
        let same = fail_with(db, &w, Caller::TaskBot, failed, Some(1), "first reason").await;
        assert!(
            matches!(&same, Err(ApiError::BadRequest(message)) if message.contains("not active")),
            "{same:?}"
        );
        let other = fail_with(db, &w, Caller::TaskBot, failed, Some(1), "another reason").await;
        assert!(
            matches!(&other, Err(ApiError::Conflict(message)) if message.contains("different reason")),
            "{other:?}"
        );
        // Completing a failed task (or failing a completed one) stays a plain "not active".
        let crossed = complete_with(db, &w, Caller::TaskBot, failed, Some(1), "s").await;
        assert!(matches!(&crossed, Err(ApiError::BadRequest(_))), "{crossed:?}");
        scratch.drop_self().await;
    }

    #[tokio::test]
    async fn the_attempt_is_required() {
        let scratch = scratch_or_skip!("aitp_attempt_required");
        let db = &scratch.db;
        let w = World::new(db, "ar").await;
        let task = seed_task(db, &w.tenant, w.bot, "processing", 1, 0).await;
        for answer in [
            get(db, &w, Caller::TaskBot, task, None).await,
            ack(db, &w, Caller::TaskBot, task, None, "plan").await,
            progress(db, &w, Caller::TaskBot, task, None, json!({})).await,
        ] {
            assert_eq!(bad_request(&answer), "attempt is required");
        }
        scratch.drop_self().await;
    }

    #[tokio::test]
    async fn acknowledging_moves_the_task_once_and_keeps_the_first_plan() {
        let scratch = scratch_or_skip!("aitp_ack");
        let db = &scratch.db;
        let w = World::new(db, "ak").await;
        let task = seed_task(db, &w.tenant, w.bot, "processing", 2, 0).await;

        let first = ack(db, &w, Caller::TaskBot, task, Some(2), "  read the code, then fix it  ")
            .await
            .expect("ack");
        assert_eq!(data(&first)["proceed"], true);
        assert_eq!(data(&first)["task"]["status"], "acknowledged");
        assert_eq!(data(&first)["task"]["plan"], "read the code, then fix it");
        assert!(data(&first)["task"]["acknowledged_at"].is_string());
        assert!(data(&first)["task"]["lease_expires_at"].is_string());

        // The same attempt again: 200, the stored plan is not overwritten, nothing more is written.
        let again = ack(db, &w, Caller::TaskBot, task, Some(2), "a different plan")
            .await
            .expect("repeat ack");
        assert_eq!(data(&again)["task"]["plan"], "read the code, then fix it");
        assert_eq!(
            count(
                db,
                "SELECT COUNT(*)::bigint AS count FROM ai_task_events WHERE task_id = $1 AND event_type = 'acknowledged'",
                vec![task.into()],
            )
            .await,
            1
        );

        // Another attempt names both numbers.
        let other = ack(db, &w, Caller::TaskBot, task, Some(1), "plan").await;
        let message = bad_request(&other);
        assert!(
            message.contains("attempt 1") && message.contains("current attempt 2"),
            "{message}"
        );

        // A task that ended is not acknowledged.
        let done = seed_task(db, &w.tenant, w.bot, "completed", 1, 0).await;
        assert!(ack(db, &w, Caller::TaskBot, done, Some(1), "plan").await.is_err());
        let pending = seed_task(db, &w.tenant, w.bot, "pending", 1, 0).await;
        assert!(ack(db, &w, Caller::TaskBot, pending, Some(1), "plan").await.is_err());

        // The plan is 1 to 4000 characters.
        let fresh = seed_task(db, &w.tenant, w.bot, "processing", 1, 0).await;
        for plan in ["", "   ", &"x".repeat(4001)] {
            assert!(bad_request(&ack(db, &w, Caller::TaskBot, fresh, Some(1), plan).await).contains("plan"));
        }
        assert!(
            ack(db, &w, Caller::TaskBot, fresh, Some(1), &"x".repeat(4000))
                .await
                .is_ok()
        );
        scratch.drop_self().await;
    }

    #[tokio::test]
    async fn an_ack_writes_one_outbox_row_and_progress_writes_none() {
        let scratch = scratch_or_skip!("aitp_outbox");
        let db = &scratch.db;
        let w = World::new(db, "ob").await;
        let task = seed_task(db, &w.tenant, w.bot, "processing", 1, 0).await;
        let rows = |db: DatabaseConnection| async move {
            count(
                &db,
                "SELECT COUNT(*)::bigint AS count FROM oauth_event_outbox WHERE event_type = 'task.status_changed'",
                vec![],
            )
            .await
        };
        assert_eq!(rows(db.clone()).await, 0);
        ack(db, &w, Caller::TaskBot, task, Some(1), "plan").await.expect("ack");
        assert_eq!(rows(db.clone()).await, 1);
        for kind in ["agent", "runner.alive", "runner.blocked"] {
            progress(db, &w, Caller::TaskBot, task, Some(1), json!({"kind": kind}))
                .await
                .expect("progress");
        }
        assert_eq!(
            rows(db.clone()).await,
            1,
            "progress and heartbeats never reach the outbox"
        );
        assert_eq!(
            count(
                db,
                "SELECT COUNT(*)::bigint AS count FROM business_events WHERE event_type = 'ai_task.acknowledged' AND aggregate_id = $1",
                vec![task.to_string().into()],
            )
            .await,
            1
        );
        scratch.drop_self().await;
    }

    async fn lease_secs_from_now(db: &DatabaseConnection, task: Uuid) -> f64 {
        use sea_orm::FromQueryResult;
        #[derive(FromQueryResult)]
        struct Row {
            secs: f64,
        }
        Row::find_by_statement(sea_orm::Statement::from_sql_and_values(
            sea_orm::DbBackend::Postgres,
            "SELECT EXTRACT(EPOCH FROM (lease_expires_at - now()))::float8 AS secs FROM ai_tasks WHERE id = $1",
            vec![task.into()],
        ))
        .one(db)
        .await
        .expect("query runs")
        .expect("task exists")
        .secs
    }

    async fn phase_of(db: &DatabaseConnection, task: Uuid) -> Option<String> {
        use sea_orm::FromQueryResult;
        #[derive(FromQueryResult)]
        struct Row {
            runner_phase: Option<String>,
        }
        Row::find_by_statement(sea_orm::Statement::from_sql_and_values(
            sea_orm::DbBackend::Postgres,
            "SELECT runner_phase FROM ai_tasks WHERE id = $1",
            vec![task.into()],
        ))
        .one(db)
        .await
        .expect("query runs")
        .expect("task exists")
        .runner_phase
    }

    #[tokio::test]
    async fn progress_extends_the_lease_up_to_the_cap_and_coalesces_events() {
        let scratch = scratch_or_skip!("aitp_progress_lease");
        let db = &scratch.db;
        let w = World::new(db, "pl").await;

        // Started 86,000 s ago: the 86,400 s cap leaves about 400 s, less than one extension.
        let old = seed_task(db, &w.tenant, w.bot, "processing", 1, 86_000).await;
        let capped = progress(db, &w, Caller::TaskBot, old, Some(1), json!({"note": "still here"}))
            .await
            .expect("progress");
        assert_eq!(data(&capped)["coalesced"], false);
        let secs = lease_secs_from_now(db, old).await;
        assert!(
            (380.0..=401.0).contains(&secs),
            "the lease is capped at started_at + 86400, got {secs}"
        );

        // A recent task gets a full extension.
        let fresh = seed_task(db, &w.tenant, w.bot, "processing", 1, 0).await;
        let first = progress(
            db,
            &w,
            Caller::TaskBot,
            fresh,
            Some(1),
            json!({"note": "one", "percent": 10}),
        )
        .await
        .expect("progress");
        assert_eq!(data(&first)["coalesced"], false);
        let secs = lease_secs_from_now(db, fresh).await;
        assert!((1790.0..=1801.0).contains(&secs), "lease is now + 1800, got {secs}");
        exec(
            db,
            "UPDATE ai_tasks SET lease_expires_at = now() + interval '1 minute' WHERE id = $1",
            vec![fresh.into()],
        )
        .await;

        // A second report within 30 s writes no event but still extends the lease.
        let second = progress(db, &w, Caller::TaskBot, fresh, Some(1), json!({"note": "two"}))
            .await
            .expect("progress");
        assert_eq!(data(&second)["coalesced"], true);
        assert!(lease_secs_from_now(db, fresh).await > 1700.0);
        assert_eq!(
            count(
                db,
                "SELECT COUNT(*)::bigint AS count FROM ai_task_events WHERE task_id = $1 AND event_type = 'progress'",
                vec![fresh.into()],
            )
            .await,
            1
        );
        scratch.drop_self().await;
    }

    #[tokio::test]
    async fn runner_kinds_set_the_phase_and_the_others_leave_it() {
        let scratch = scratch_or_skip!("aitp_progress_phase");
        let db = &scratch.db;
        let w = World::new(db, "ph").await;
        let task = seed_task(db, &w.tenant, w.bot, "processing", 1, 0).await;
        for (kind, expected) in [
            ("runner.queued", Some("queued")),
            ("runner.alive", Some("queued")),
            ("runner.pushed", Some("pushed")),
            ("runner.reminded", Some("pushed")),
            ("agent", Some("pushed")),
            ("runner.blocked", Some("blocked")),
        ] {
            progress(
                db,
                &w,
                Caller::TaskBot,
                task,
                Some(1),
                json!({"kind": kind, "note": kind, "queue_position": 2}),
            )
            .await
            .unwrap_or_else(|err| panic!("{kind}: {err:?}"));
            assert_eq!(phase_of(db, task).await.as_deref(), expected, "after {kind}");
        }
        assert_eq!(
            count(
                db,
                "SELECT COUNT(DISTINCT event_type)::bigint AS count FROM ai_task_events WHERE task_id = $1",
                vec![task.into()],
            )
            .await,
            6,
            "every kind has its own event type"
        );
        scratch.drop_self().await;
    }

    #[tokio::test]
    async fn progress_bounds_and_inactive_tasks_are_refused() {
        let scratch = scratch_or_skip!("aitp_progress_bounds");
        let db = &scratch.db;
        let w = World::new(db, "pb").await;
        let task = seed_task(db, &w.tenant, w.bot, "acknowledged", 1, 0).await;
        for body in [
            json!({"note": "x".repeat(1001)}),
            json!({"percent": -1}),
            json!({"percent": 101}),
            json!({"queue_position": -1}),
            json!({"queue_position": 10_001}),
            json!({"kind": "runner.dancing"}),
        ] {
            assert!(
                progress(db, &w, Caller::TaskBot, task, Some(1), body.clone())
                    .await
                    .is_err(),
                "{body} must be refused"
            );
        }
        let edge = json!({"note": "x".repeat(1000), "percent": 100, "queue_position": 0});
        assert!(progress(db, &w, Caller::TaskBot, task, Some(1), edge).await.is_ok());

        let wrong = progress(db, &w, Caller::TaskBot, task, Some(2), json!({})).await;
        let message = bad_request(&wrong);
        assert!(
            message.contains("attempt 2") && message.contains("current attempt 1"),
            "{message}"
        );
        let done = seed_task(db, &w.tenant, w.bot, "completed", 1, 0).await;
        assert!(
            progress(db, &w, Caller::TaskBot, done, Some(1), json!({}))
                .await
                .is_err()
        );
        scratch.drop_self().await;
    }

    #[tokio::test]
    async fn the_view_serves_active_and_ended_attempts_only() {
        let scratch = scratch_or_skip!("aitp_view_states");
        let db = &scratch.db;
        let w = World::new(db, "vs").await;
        let active_task = seed_task(db, &w.tenant, w.bot, "processing", 2, 0).await;
        let completed = seed_task(db, &w.tenant, w.bot, "completed", 2, 0).await;
        let pending = seed_task(db, &w.tenant, w.bot, "pending", 2, 0).await;

        let view = get(db, &w, Caller::TaskBot, active_task, Some(2))
            .await
            .expect("active view");
        assert_eq!(data(&view)["active"], true);
        assert_eq!(data(&view)["attempt"], 2);
        let ended = get(db, &w, Caller::TaskBot, completed, Some(2))
            .await
            .expect("ended view");
        assert_eq!(data(&ended)["active"], false);
        assert_eq!(data(&ended)["status"], "completed");

        let other_attempt = get(db, &w, Caller::TaskBot, active_task, Some(1)).await;
        let message = bad_request(&other_attempt);
        assert!(
            message.contains("attempt 1") && message.contains("current attempt 2"),
            "{message}"
        );
        assert!(get(db, &w, Caller::TaskBot, completed, Some(1)).await.is_err());
        assert!(get(db, &w, Caller::TaskBot, pending, Some(2)).await.is_err());
        assert!(matches!(
            get(db, &w, Caller::TaskBot, Uuid::new_v4(), Some(1)).await,
            Err(ApiError::NotFound(_))
        ));
        scratch.drop_self().await;
    }

    #[tokio::test]
    async fn the_active_list_is_the_bots_own_oldest_first_and_bounded() {
        let scratch = scratch_or_skip!("aitp_active_list");
        let db = &scratch.db;
        let w = World::new(db, "al").await;
        let newer = seed_task(db, &w.tenant, w.bot, "processing", 1, 100).await;
        let older = seed_task(db, &w.tenant, w.bot, "acknowledged", 1, 5_000).await;
        seed_task(db, &w.tenant, w.bot, "completed", 1, 9_000).await;
        seed_task(db, &w.tenant, w.bot, "pending", 0, 9_000).await;
        seed_task(db, &w.tenant, w.other_bot, "processing", 1, 9_000).await;

        let listed = active(db, &w, Caller::TaskBot).await.expect("active list");
        let ids: Vec<&str> = data(&listed)
            .as_array()
            .expect("a list")
            .iter()
            .filter_map(|view| view["task_id"].as_str())
            .collect();
        assert_eq!(ids, vec![older.to_string(), newer.to_string()]);

        exec(
            db,
            "INSERT INTO ai_tasks (id, project_id, ai_participant_id, task_type, status, attempts, max_attempts, started_at) \
             SELECT gen_random_uuid(), $1, $2, 'issue_assigned', 'processing', 1, 3, now() \
             FROM generate_series(1, 120)",
            vec![w.tenant.project_id.into(), w.bot.into()],
        )
        .await;
        let capped = active(db, &w, Caller::TaskBot).await.expect("active list");
        assert_eq!(data(&capped).as_array().expect("a list").len(), 100);
        scratch.drop_self().await;
    }
}
