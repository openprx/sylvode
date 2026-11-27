//! Reply routing: what the server writes where a task came from when the task ends.
//!
//! The routing runs in the caller's transaction, next to the compare-and-set that ends the task,
//! so the end and its reply commit together or not at all. It returns the webhooks to send after
//! the commit.
//!
//! Agent text is untrusted, and a reply must never start another task. Routing therefore never
//! calls `create_ai_task`, inserts comments without extracting mentions (no notification and no
//! mention task, whatever the text says), and a state change never assigns. Retryable failures
//! re-queue the same task with an `UPDATE`.

use platform::app::AppState;
use sea_orm::{ConnectionTrait, DbBackend, FromQueryResult, Statement, TransactionTrait};
use serde::Serialize;
use serde_json::Value;
use uuid::Uuid;

use crate::{
    error::ApiError,
    routes::proposal::find_proposal,
    services::{
        ai_task_service::AiTaskRow,
        ai_task_view::SourceKind,
        comments::insert_comment,
        execution_settings::{self, ExecutionSettings},
        issues::{IssueRef, record_label_change, set_state},
        proposals::{VoteActor, cast_vote},
        workflow_service::resolve_effective_workflow_for_project,
    },
    webhook_trigger::{TriggerContext, WebhookEvent, trigger_webhooks_after_ai_tasks},
};

/// How an agent says a task ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    Done,
    NeedsReview,
    NoChange,
}

impl Outcome {
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "done" => Some(Self::Done),
            "needs_review" => Some(Self::NeedsReview),
            "no_change" => Some(Self::NoChange),
            _ => None,
        }
    }

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Done => "done",
            Self::NeedsReview => "needs_review",
            Self::NoChange => "no_change",
        }
    }
}

/// A thing the agent produced, listed under its summary.
#[derive(Debug, Clone, Serialize)]
pub struct Artifact {
    pub kind: String,
    #[serde(rename = "ref")]
    pub reference: String,
    pub label: Option<String>,
}

/// A validated completion.
#[derive(Debug, Clone)]
pub struct CompletionInput {
    pub outcome: Outcome,
    pub summary: String,
    pub artifacts: Vec<Artifact>,
    pub vote: Option<String>,
    pub vote_reason: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct StateChanged {
    pub from: String,
    pub to: String,
}

/// What the server wrote in reply (frozen shape).
#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize)]
pub struct ReplyReport {
    pub comment_id: Option<Uuid>,
    pub state_changed: Option<StateChanged>,
    pub label_added: Option<Uuid>,
    pub vote: Option<String>,
    pub warnings: Vec<String>,
}

/// A report and the webhooks to send once the transaction has committed.
#[derive(Debug, Default)]
pub struct RoutedReply {
    pub report: ReplyReport,
    pub triggers: Vec<TriggerContext>,
}

/// Sends the webhooks of a committed reply. The task's bot is the actor, so its own webhook gets
/// no bot context for its own action.
pub fn send_reply_triggers(state: &AppState, triggers: Vec<TriggerContext>) {
    for trigger in triggers {
        trigger_webhooks_after_ai_tasks(state.clone(), trigger, Vec::new());
    }
}

fn artifact_line(artifact: &Artifact) -> String {
    artifact.label.as_deref().filter(|label| !label.is_empty()).map_or_else(
        || format!("- {}: {}", artifact.kind, artifact.reference),
        |label| format!("- {label} ({}): {}", artifact.kind, artifact.reference),
    )
}

/// The summary followed by the artifacts, as the comment text.
pub fn compose_body(summary: &str, artifacts: &[Artifact]) -> String {
    let mut body = summary.trim().to_string();
    if !artifacts.is_empty() {
        let lines: Vec<String> = artifacts.iter().map(artifact_line).collect();
        body.push_str("\n\n**Artifacts**\n");
        body.push_str(&lines.join("\n"));
    }
    body
}

/// The issue a comment task is about (`payload.issue_id`) or an assignment's reference.
fn issue_id_for(task: &AiTaskRow, kind: SourceKind) -> Option<Uuid> {
    let from_payload = || {
        task.payload
            .get("issue_id")
            .and_then(Value::as_str)
            .and_then(|id| Uuid::parse_str(id).ok())
    };
    match kind {
        SourceKind::IssueAssignment => task.reference_id,
        SourceKind::CommentMention | SourceKind::ReviewRequest => from_payload(),
        SourceKind::ProposalVote | SourceKind::Manual => None,
    }
}

async fn find_issue<C: ConnectionTrait>(
    db: &C,
    issue_id: Option<Uuid>,
    project_id: Uuid,
) -> Result<Option<IssueRef>, ApiError> {
    let Some(issue_id) = issue_id else {
        return Ok(None);
    };
    Ok(IssueRef::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "SELECT wi.id, wi.project_id, p.workspace_id FROM work_items wi \
         INNER JOIN projects p ON p.id = wi.project_id WHERE wi.id = $1 AND wi.project_id = $2",
        vec![issue_id.into(), project_id.into()],
    ))
    .one(db)
    .await?)
}

/// The first line of a reply to a mention.
fn mention_prefix(task: &AiTaskRow) -> String {
    let comment_id = task
        .payload
        .get("comment_id")
        .and_then(Value::as_str)
        .map(str::to_string)
        .or_else(|| task.reference_id.map(|id| id.to_string()))
        .unwrap_or_default();
    format!("Re: comment {comment_id}\n\n")
}

const fn comment_trigger(issue: &IssueRef, bot_id: Uuid, comment_id: Uuid) -> TriggerContext {
    TriggerContext {
        event: WebhookEvent::CommentCreated,
        workspace_id: issue.workspace_id,
        project_id: issue.project_id,
        actor_id: bot_id,
        issue_id: Some(issue.id),
        comment_id: Some(comment_id),
        label_id: None,
        sprint_id: None,
        changes: None,
        mentions: Vec::new(),
        extra_data: None,
    }
}

/// The state an `auto` or `state:<key>` setting names for an issue now in `current`, or why none.
async fn target_state(
    state: &AppState,
    project_id: Uuid,
    settings: Option<&ExecutionSettings>,
    current: &str,
    warnings: &mut Vec<String>,
) -> Result<Option<String>, ApiError> {
    let setting = settings.map_or("auto", |settings| settings.on_complete_state.as_str());
    if setting == "none" {
        return Ok(None);
    }
    let workflow = resolve_effective_workflow_for_project(state, project_id).await?;
    if setting == "auto" {
        if workflow
            .states
            .iter()
            .any(|candidate| candidate.key == current && candidate.is_terminal)
        {
            return Ok(None);
        }
        let done = workflow
            .states
            .iter()
            .filter(|candidate| candidate.category == "done")
            .min_by_key(|candidate| candidate.position);
        if done.is_none() {
            warnings.push("no done state in workflow".to_string());
        }
        return Ok(done.map(|candidate| candidate.key.clone()));
    }
    let key = setting.strip_prefix("state:").unwrap_or(setting);
    if workflow.states.iter().any(|candidate| candidate.key == key) {
        Ok(Some(key.to_string()))
    } else {
        warnings.push(format!("state {key} not in workflow"));
        Ok(None)
    }
}

/// Writes the reply to a completed task in `txn`.
///
/// An assigned issue gets the summary as a comment and, for `done`, the state its project's
/// settings name. A mention or review request gets a reply comment on the issue. Another kind of
/// task writes nothing. A routing error leaves the caller to roll the transaction back.
pub async fn route_completion<C: ConnectionTrait>(
    state: &AppState,
    txn: &C,
    task: &AiTaskRow,
    input: &CompletionInput,
) -> Result<RoutedReply, ApiError> {
    let kind = SourceKind::of(task);
    let bot_id = task.ai_participant_id;
    let mut routed = RoutedReply::default();
    match kind {
        SourceKind::Manual => {}
        SourceKind::ProposalVote => route_vote(state, txn, task, input, &mut routed).await?,
        SourceKind::IssueAssignment | SourceKind::CommentMention | SourceKind::ReviewRequest => {
            let issue = find_issue(txn, issue_id_for(task, kind), task.project_id)
                .await?
                .ok_or_else(|| ApiError::Conflict("the issue this task refers to no longer exists".to_string()))?;
            let prefix = if kind == SourceKind::IssueAssignment {
                String::new()
            } else {
                mention_prefix(task)
            };
            let body = format!("{prefix}{}", compose_body(&input.summary, &input.artifacts));
            let comment = insert_comment(txn, issue.id, bot_id, &body, &[]).await?;
            routed.report.comment_id = Some(comment.id);
            routed.triggers.push(comment_trigger(&issue, bot_id, comment.id));

            if kind == SourceKind::IssueAssignment && input.outcome == Outcome::Done {
                let settings = execution_settings::load(txn, task.project_id, task.ai_participant_id).await?;
                let current = current_state(txn, issue.id).await?;
                let target = target_state(
                    state,
                    task.project_id,
                    settings.as_ref(),
                    &current,
                    &mut routed.report.warnings,
                )
                .await?;
                if let Some(target) = target
                    && let Some(change) = set_state(txn, &issue, bot_id, &target).await?
                {
                    routed.report.state_changed = Some(StateChanged {
                        from: change.from,
                        to: change.to,
                    });
                    routed.triggers.push(change.trigger);
                }
            }
        }
    }
    Ok(routed)
}

/// Casts the bot's vote for a proposal vote task.
///
/// The vote, its reason and the choice are checked exactly as the vote endpoint checks them; a
/// refusal (a reason that is too short, a closed voting window) is a 400 and the task stays
/// active, so the agent can call again. A vote the bot already cast completes the task with the
/// warning `vote already recorded`.
async fn route_vote<C: ConnectionTrait>(
    state: &AppState,
    txn: &C,
    task: &AiTaskRow,
    input: &CompletionInput,
    routed: &mut RoutedReply,
) -> Result<(), ApiError> {
    if input.outcome == Outcome::NoChange {
        return Err(ApiError::BadRequest(
            "a vote task cannot end with no_change".to_string(),
        ));
    }
    let Some(choice) = input.vote.as_deref() else {
        return Err(ApiError::BadRequest(
            "vote is required for a proposal_vote task".to_string(),
        ));
    };
    let proposal_id = task
        .payload
        .get("proposal_id")
        .and_then(Value::as_str)
        .map(str::to_string)
        .or_else(|| task.reference_id.map(|id| id.to_string()))
        .ok_or_else(|| ApiError::Conflict("the proposal this task refers to is not named".to_string()))?;
    let proposal = find_proposal(state, &proposal_id)
        .await
        .map_err(|_| ApiError::Conflict("the proposal this task refers to no longer exists".to_string()))?;
    let same_workspace = txn
        .query_one(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT 1 FROM projects WHERE id = $1 AND workspace_id IS NOT DISTINCT FROM $2",
            vec![task.project_id.into(), proposal.workspace_id.into()],
        ))
        .await?
        .is_some();
    if !same_workspace {
        return Err(ApiError::Conflict(
            "the proposal this task refers to no longer exists".to_string(),
        ));
    }
    let actor = VoteActor {
        user_id: task.ai_participant_id,
        author_type: "ai",
    };
    match cast_vote(state, txn, &actor, &proposal, choice, input.vote_reason.as_deref()).await {
        Ok(cast) => {
            routed.report.vote = Some(cast.vote.choice);
            routed.triggers.extend(cast.trigger);
        }
        Err(ApiError::Conflict(_)) => routed.report.warnings.push("vote already recorded".to_string()),
        Err(other) => return Err(other),
    }
    Ok(())
}

async fn current_state<C: ConnectionTrait>(db: &C, issue_id: Uuid) -> Result<String, ApiError> {
    #[derive(FromQueryResult)]
    struct Current {
        state: String,
    }
    Ok(Current::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "SELECT state FROM work_items WHERE id = $1",
        vec![issue_id.into()],
    ))
    .one(db)
    .await?
    .ok_or_else(|| ApiError::NotFound("issue not found".to_string()))?
    .state)
}

/// Writes the reply to a task that ended as failed (not one that will be retried) in `txn`: a
/// comment `{lead}: {reason}` and, for an assigned issue, the project's failure label.
///
/// A missing issue is a warning, not an error: a task must be able to end when its issue is gone.
pub async fn route_failure<C: ConnectionTrait>(
    txn: &C,
    task: &AiTaskRow,
    lead: &str,
    reason: &str,
) -> Result<RoutedReply, ApiError> {
    let kind = SourceKind::of(task);
    let bot_id = task.ai_participant_id;
    let mut routed = RoutedReply::default();
    if !matches!(
        kind,
        SourceKind::IssueAssignment | SourceKind::CommentMention | SourceKind::ReviewRequest
    ) {
        return Ok(routed);
    }
    let Some(issue) = find_issue(txn, issue_id_for(task, kind), task.project_id).await? else {
        routed.report.warnings.push("issue not found".to_string());
        return Ok(routed);
    };
    let prefix = if kind == SourceKind::IssueAssignment {
        String::new()
    } else {
        mention_prefix(task)
    };
    let comment = insert_comment(
        txn,
        issue.id,
        bot_id,
        &format!("{prefix}{lead}: {}", reason.trim()),
        &[],
    )
    .await?;
    routed.report.comment_id = Some(comment.id);
    routed.triggers.push(comment_trigger(&issue, bot_id, comment.id));

    if kind == SourceKind::IssueAssignment
        && let Some(label_id) = execution_settings::load(txn, task.project_id, task.ai_participant_id)
            .await?
            .and_then(|settings| settings.on_fail_label_id)
    {
        let in_workspace = txn
            .query_one(Statement::from_sql_and_values(
                DbBackend::Postgres,
                "SELECT 1 FROM labels WHERE id = $1 AND workspace_id = $2",
                vec![label_id.into(), issue.workspace_id.into()],
            ))
            .await?
            .is_some();
        if in_workspace {
            let attached = txn
                .execute(Statement::from_sql_and_values(
                    DbBackend::Postgres,
                    "INSERT INTO work_item_labels (work_item_id, label_id) VALUES ($1, $2) \
                     ON CONFLICT (work_item_id, label_id) DO NOTHING",
                    vec![issue.id.into(), label_id.into()],
                ))
                .await?;
            if attached.rows_affected() > 0 {
                record_label_change(txn, issue.id, bot_id, label_id, true).await?;
            }
            routed.report.label_added = Some(label_id);
        } else {
            routed.report.warnings.push("label not found".to_string());
        }
    }
    Ok(routed)
}

/// The reply to a task the runner refused at delivery.
///
/// For a task whose connection exists, enabled or not, it is a comment
/// `Could not start: {reason}` where the task came from, written in its own transaction after the
/// worker failed the task. A project whose connection is disabled still wants to know why its
/// task did not start; a task without a connection was never configured and gets no comment.
pub async fn announce_start_refusal(state: &AppState, task: &AiTaskRow, reason: &str) -> Result<(), ApiError> {
    if execution_settings::load(&state.db, task.project_id, task.ai_participant_id)
        .await?
        .is_none()
    {
        return Ok(());
    }
    let txn = state.db.begin().await?;
    let routed = route_failure(&txn, task, "Could not start", reason).await?;
    txn.commit().await?;
    send_reply_triggers(state, routed.triggers);
    Ok(())
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::panic, clippy::too_many_lines, clippy::indexing_slicing)]
mod tests {
    use super::{Artifact, announce_start_refusal, compose_body};
    use crate::error::ApiError;
    use crate::routes::ai_task_protocol::TaskAttemptQuery;
    use crate::routes::ai_task_protocol::{
        CompleteRequest, FailRequest, complete_task_with_reply, fail_task_with_reply,
    };
    use crate::routes::context::tenant_fixture::{
        Tenant, bot_context_for, claims_for, count, exec, seed_bot_runner, seed_bot_user, seed_label, seed_member,
        seed_tenant, seed_work_item, state_for,
    };
    use crate::scratch_or_skip;
    use crate::services::ai_task_fixture::{seed_task, seed_task_for};
    use crate::services::ai_task_service::load_task_with_protocol;
    use axum::{
        Extension, Json,
        extract::{Path, Query, State},
        response::IntoResponse,
    };
    use sea_orm::{DatabaseConnection, FromQueryResult};
    use serde_json::{Value, json};
    use uuid::Uuid;

    #[test]
    fn the_comment_lists_the_artifacts_under_the_summary() {
        let artifacts = vec![
            Artifact {
                kind: "commit".into(),
                reference: "abc123".into(),
                label: None,
            },
            Artifact {
                kind: "pull_request".into(),
                reference: "https://example.test/pr/1".into(),
                label: Some("Fix".into()),
            },
        ];
        assert_eq!(
            compose_body("  Done it.  ", &artifacts),
            "Done it.\n\n**Artifacts**\n- commit: abc123\n- Fix (pull_request): https://example.test/pr/1"
        );
        assert_eq!(compose_body("Only text", &[]), "Only text");
    }

    struct Env {
        db: DatabaseConnection,
        tenant: Tenant,
        bot: Uuid,
        issue: Uuid,
    }

    impl Env {
        async fn new(db: &DatabaseConnection, tag: &str) -> Self {
            let tenant = seed_tenant(db, tag).await;
            let bot = seed_bot_user(db, &tenant).await;
            let issue = seed_work_item(db, &tenant, "Fix the build").await;
            Self {
                db: db.clone(),
                tenant,
                bot,
                issue,
            }
        }

        async fn assignment(&self, status: &str) -> Uuid {
            seed_task_for(
                &self.db,
                &self.tenant,
                self.bot,
                status,
                1,
                0,
                ("issue_assigned", Some("work_item"), Some(self.issue)),
                json!({"issue_id": self.issue.to_string()}),
            )
            .await
        }

        async fn complete(&self, task: Uuid, attempt: i32, body: Value) -> Result<Value, ApiError> {
            let request: CompleteRequest = serde_json::from_value(body).expect("a complete body");
            let answer = complete_task_with_reply(
                State(state_for(self.db.clone())),
                Extension(claims_for(self.bot)),
                Some(Extension(bot_context_for(
                    self.bot,
                    self.tenant.workspace_id,
                    &["read", "write"],
                ))),
                Path(task),
                Query(TaskAttemptQuery { attempt: Some(attempt) }),
                Json(request),
            )
            .await?
            .into_response();
            let bytes = axum::body::to_bytes(answer.into_body(), usize::MAX)
                .await
                .expect("body");
            Ok(serde_json::from_slice(&bytes).expect("json"))
        }

        async fn fail(&self, task: Uuid, attempt: i32, body: Value) -> Result<Value, ApiError> {
            let request: FailRequest = serde_json::from_value(body).expect("a fail body");
            let answer = fail_task_with_reply(
                State(state_for(self.db.clone())),
                Extension(claims_for(self.bot)),
                Some(Extension(bot_context_for(
                    self.bot,
                    self.tenant.workspace_id,
                    &["read", "write"],
                ))),
                Path(task),
                Query(TaskAttemptQuery { attempt: Some(attempt) }),
                Json(request),
            )
            .await?
            .into_response();
            let bytes = axum::body::to_bytes(answer.into_body(), usize::MAX)
                .await
                .expect("body");
            Ok(serde_json::from_slice(&bytes).expect("json"))
        }

        async fn comments(&self) -> Vec<(Uuid, String)> {
            #[derive(FromQueryResult)]
            struct Row {
                author_id: Uuid,
                body: String,
            }
            Row::find_by_statement(sea_orm::Statement::from_sql_and_values(
                sea_orm::DbBackend::Postgres,
                "SELECT author_id, body FROM comments WHERE work_item_id = $1 ORDER BY created_at, id",
                vec![self.issue.into()],
            ))
            .all(&self.db)
            .await
            .expect("comments load")
            .into_iter()
            .map(|row| (row.author_id, row.body))
            .collect()
        }

        async fn state(&self) -> String {
            #[derive(FromQueryResult)]
            struct Row {
                state: String,
            }
            Row::find_by_statement(sea_orm::Statement::from_sql_and_values(
                sea_orm::DbBackend::Postgres,
                "SELECT state FROM work_items WHERE id = $1",
                vec![self.issue.into()],
            ))
            .one(&self.db)
            .await
            .expect("state loads")
            .expect("issue exists")
            .state
        }

        async fn settings(&self, on_complete_state: &str, label: Option<Uuid>) {
            seed_bot_runner(&self.db, &self.tenant, self.bot).await;
            exec(
                &self.db,
                "INSERT INTO project_bot_connections \
                 (project_id, bot_id, working_directory, enabled, on_complete_state, on_fail_label_id) \
                 VALUES ($1, $2, '/work/a', true, $3, $4) \
                 ON CONFLICT (project_id, bot_id) DO UPDATE SET on_complete_state = $3, on_fail_label_id = $4",
                vec![
                    self.tenant.project_id.into(),
                    self.bot.into(),
                    on_complete_state.to_string().into(),
                    label.into(),
                ],
            )
            .await;
        }

        async fn status(&self, task: Uuid) -> String {
            load_task_with_protocol(&self.db, task).await.expect("task").task.status
        }
    }

    fn data(value: &Value) -> &Value {
        &value["data"]
    }

    #[tokio::test]
    async fn a_done_assignment_comments_and_moves_the_issue_to_the_done_state() {
        let scratch = scratch_or_skip!("reply_done");
        let env = Env::new(&scratch.db, "dn").await;
        let task = env.assignment("processing").await;
        let before = count(
            &env.db,
            "SELECT COUNT(*)::bigint AS count FROM oauth_event_outbox",
            vec![],
        )
        .await;

        let answer = env
            .complete(
                task,
                1,
                json!({"outcome": "done", "summary": "Fixed it.", "artifacts": [{"kind": "commit", "ref": "abc123"}]}),
            )
            .await
            .expect("complete");
        assert_eq!(data(&answer)["task"]["status"], "completed");
        assert_eq!(data(&answer)["task"]["active"], false);
        assert_eq!(
            data(&answer)["reply"]["state_changed"],
            json!({"from": "todo", "to": "done"})
        );
        assert_eq!(data(&answer)["reply"]["warnings"], json!([]));
        let comments = env.comments().await;
        assert_eq!(comments.len(), 1);
        assert_eq!(comments[0].0, env.bot, "the bot is the author");
        assert_eq!(comments[0].1, "Fixed it.\n\n**Artifacts**\n- commit: abc123");
        assert_eq!(data(&answer)["reply"]["comment_id"].as_str().map(str::len), Some(36));
        assert_eq!(env.state().await, "done");

        // Completing from `processing` is recorded; one outbox row for the one status change.
        assert_eq!(
            count(
                &env.db,
                "SELECT COUNT(*)::bigint AS count FROM ai_task_events WHERE task_id = $1 AND event_type = 'completed' \
                 AND payload->>'ack_skipped' = 'true'",
                vec![task.into()],
            )
            .await,
            1
        );
        assert_eq!(
            count(&env.db, "SELECT COUNT(*)::bigint AS count FROM ai_task_events WHERE task_id = $1 AND event_type = 'reply_routed'", vec![task.into()]).await,
            1
        );
        assert_eq!(
            count(
                &env.db,
                "SELECT COUNT(*)::bigint AS count FROM oauth_event_outbox",
                vec![]
            )
            .await,
            before + 1
        );
        assert_eq!(
            count(
                &env.db,
                "SELECT COUNT(*)::bigint AS count FROM activities WHERE issue_id = $1 AND action = 'status_changed' AND user_id = $2",
                vec![env.issue.into(), env.bot.into()],
            )
            .await,
            1
        );
        scratch.drop_self().await;
    }

    #[tokio::test]
    async fn the_state_follows_the_settings_and_the_issue() {
        let scratch = scratch_or_skip!("reply_states");
        let env = Env::new(&scratch.db, "st").await;
        let body = json!({"outcome": "done", "summary": "ok"});

        // An issue already in a terminal state is not moved by `auto`.
        exec(
            &env.db,
            "UPDATE work_items SET state = 'done' WHERE id = $1",
            vec![env.issue.into()],
        )
        .await;
        let task = env.assignment("acknowledged").await;
        let answer = env.complete(task, 1, body.clone()).await.expect("complete");
        assert_eq!(data(&answer)["reply"]["state_changed"], Value::Null);
        assert_eq!(env.state().await, "done");
        exec(
            &env.db,
            "UPDATE work_items SET state = 'todo' WHERE id = $1",
            vec![env.issue.into()],
        )
        .await;

        env.settings("none", None).await;
        let task = env.assignment("processing").await;
        env.complete(task, 1, body.clone()).await.expect("complete");
        assert_eq!(env.state().await, "todo", "none leaves the state");

        env.settings("state:in_progress", None).await;
        let task = env.assignment("processing").await;
        let answer = env.complete(task, 1, body.clone()).await.expect("complete");
        assert_eq!(env.state().await, "in_progress");
        assert_eq!(
            data(&answer)["reply"]["state_changed"],
            json!({"from": "todo", "to": "in_progress"})
        );
        exec(
            &env.db,
            "UPDATE work_items SET state = 'todo' WHERE id = $1",
            vec![env.issue.into()],
        )
        .await;

        env.settings("state:nope", None).await;
        let task = env.assignment("processing").await;
        let answer = env.complete(task, 1, body).await.expect("complete");
        assert_eq!(env.state().await, "todo", "an unknown key leaves the state");
        assert_eq!(
            data(&answer)["reply"]["warnings"],
            json!(["state nope not in workflow"])
        );
        scratch.drop_self().await;
    }

    #[tokio::test]
    async fn auto_picks_the_done_state_with_the_lowest_position() {
        let scratch = scratch_or_skip!("reply_lowest_done");
        let env = Env::new(&scratch.db, "ld").await;
        let workflow = Uuid::new_v4();
        exec(
            &env.db,
            "INSERT INTO workflows (id, workspace_id, name) VALUES ($1, $2, 'two done states')",
            vec![workflow.into(), env.tenant.workspace_id.into()],
        )
        .await;
        exec(
            &env.db,
            "INSERT INTO workflow_states (workflow_id, key, display_name, category, position, is_initial, is_terminal) VALUES \
             ($1, 'todo', 'Todo', 'active', 1, true, false), \
             ($1, 'shipped', 'Shipped', 'done', 5, false, true), \
             ($1, 'released', 'Released', 'done', 3, false, true)",
            vec![workflow.into()],
        )
        .await;
        exec(
            &env.db,
            "UPDATE projects SET workflow_id = $2 WHERE id = $1",
            vec![env.tenant.project_id.into(), workflow.into()],
        )
        .await;
        let task = env.assignment("processing").await;
        env.complete(task, 1, json!({"outcome": "done", "summary": "ok"}))
            .await
            .expect("complete");
        assert_eq!(env.state().await, "released");
        scratch.drop_self().await;
    }

    #[tokio::test]
    async fn needs_review_and_no_change_comment_without_moving_the_issue() {
        let scratch = scratch_or_skip!("reply_other_outcomes");
        let env = Env::new(&scratch.db, "nr").await;
        for outcome in ["needs_review", "no_change"] {
            let task = env.assignment("processing").await;
            let answer = env
                .complete(
                    task,
                    1,
                    json!({"outcome": outcome, "summary": format!("result {outcome}")}),
                )
                .await
                .expect("complete");
            assert_eq!(data(&answer)["reply"]["state_changed"], Value::Null);
        }
        assert_eq!(env.state().await, "todo");
        assert_eq!(env.comments().await.len(), 2);
        scratch.drop_self().await;
    }

    #[tokio::test]
    async fn a_mention_gets_a_reply_comment_and_no_state_change() {
        let scratch = scratch_or_skip!("reply_mention");
        let env = Env::new(&scratch.db, "mn").await;
        for task_type in ["comment_requested", "review_requested"] {
            let comment = Uuid::new_v4();
            let task = seed_task_for(
                &env.db,
                &env.tenant,
                env.bot,
                "processing",
                1,
                0,
                (task_type, Some("comment"), Some(comment)),
                json!({"issue_id": env.issue.to_string(), "comment_id": comment.to_string()}),
            )
            .await;
            env.complete(task, 1, json!({"outcome": "done", "summary": "Here is the answer."}))
                .await
                .expect("complete");
            let comments = env.comments().await;
            let last = comments.last().expect("a comment");
            assert_eq!(last.1, format!("Re: comment {comment}\n\nHere is the answer."));
        }
        assert_eq!(env.state().await, "todo");
        scratch.drop_self().await;
    }

    #[tokio::test]
    async fn a_manual_task_stores_its_result_and_writes_nothing() {
        let scratch = scratch_or_skip!("reply_manual");
        let env = Env::new(&scratch.db, "mu").await;
        let task = seed_task(&env.db, &env.tenant, env.bot, "processing", 1, 0).await;
        let answer = env
            .complete(task, 1, json!({"outcome": "done", "summary": "ok"}))
            .await
            .expect("complete");
        assert_eq!(data(&answer)["reply"]["comment_id"], Value::Null);
        assert!(env.comments().await.is_empty());
        let stored = load_task_with_protocol(&env.db, task)
            .await
            .expect("task")
            .task
            .result
            .expect("result");
        assert_eq!(stored["summary"], "ok");
        assert_eq!(stored["outcome"], "done");
        scratch.drop_self().await;
    }

    #[tokio::test]
    async fn a_final_failure_comments_the_reason_and_labels_the_issue() {
        let scratch = scratch_or_skip!("reply_fail");
        let env = Env::new(&scratch.db, "fl").await;
        let label = seed_label(&env.db, &env.tenant, "agent-failed").await;
        env.settings("auto", Some(label)).await;

        let task = env.assignment("acknowledged").await;
        let answer = env
            .fail(
                task,
                1,
                json!({"reason": "the tests do not compile", "retryable": false}),
            )
            .await
            .expect("fail");
        assert_eq!(data(&answer)["will_retry"], false);
        assert_eq!(data(&answer)["task"]["status"], "failed");
        assert_eq!(data(&answer)["reply"]["label_added"], label.to_string());
        let comments = env.comments().await;
        assert_eq!(comments[0].1, "Could not complete: the tests do not compile");
        assert_eq!(comments[0].0, env.bot);
        assert_eq!(
            count(
                &env.db,
                "SELECT COUNT(*)::bigint AS count FROM work_item_labels WHERE work_item_id = $1 AND label_id = $2",
                vec![env.issue.into(), label.into()],
            )
            .await,
            1
        );
        assert_eq!(env.state().await, "todo");

        // A label of another workspace is not added.
        let other = seed_tenant(&env.db, "fx").await;
        let foreign = seed_label(&env.db, &other, "foreign").await;
        env.settings("auto", Some(foreign)).await;
        let task = env.assignment("processing").await;
        let answer = env.fail(task, 1, json!({"reason": "again"})).await.expect("fail");
        assert_eq!(data(&answer)["reply"]["label_added"], Value::Null);
        assert_eq!(data(&answer)["reply"]["warnings"], json!(["label not found"]));
        scratch.drop_self().await;
    }

    #[tokio::test]
    async fn a_retryable_failure_with_attempts_left_writes_nothing_to_the_issue() {
        let scratch = scratch_or_skip!("reply_retry");
        let env = Env::new(&scratch.db, "rt").await;
        let task = env.assignment("processing").await;
        let answer = env
            .fail(
                task,
                1,
                json!({"reason": "service down", "retryable": true, "source": "runner"}),
            )
            .await
            .expect("fail");
        assert_eq!(data(&answer)["will_retry"], true);
        assert_eq!(data(&answer)["task"]["status"], "pending");
        assert_eq!(data(&answer)["reply"]["comment_id"], Value::Null);
        assert!(env.comments().await.is_empty());
        assert_eq!(env.status(task).await, "pending");
        assert_eq!(
            count(&env.db, "SELECT COUNT(*)::bigint AS count FROM ai_task_events WHERE task_id = $1 AND event_type = 'reply_routed'", vec![task.into()]).await,
            0
        );

        // On the last attempt the same call fails the task for good.
        let last = seed_task_for(
            &env.db,
            &env.tenant,
            env.bot,
            "processing",
            3,
            0,
            ("issue_assigned", Some("work_item"), Some(env.issue)),
            json!({}),
        )
        .await;
        let answer = env
            .fail(last, 3, json!({"reason": "still down", "retryable": true}))
            .await
            .expect("fail");
        assert_eq!(data(&answer)["will_retry"], false);
        assert_eq!(env.comments().await.len(), 1);
        scratch.drop_self().await;
    }

    #[tokio::test]
    async fn a_task_ends_once() {
        let scratch = scratch_or_skip!("reply_ordering");
        let env = Env::new(&scratch.db, "od").await;
        let body = json!({"outcome": "done", "summary": "ok"});

        let failed = env.assignment("processing").await;
        env.fail(failed, 1, json!({"reason": "x"})).await.expect("fail");
        assert!(matches!(
            env.complete(failed, 1, body.clone()).await,
            Err(ApiError::BadRequest(_))
        ));

        let completed = env.assignment("processing").await;
        env.complete(completed, 1, body.clone()).await.expect("complete");
        assert!(matches!(
            env.complete(completed, 1, body).await,
            Err(ApiError::BadRequest(_))
        ));
        assert!(matches!(
            env.fail(completed, 1, json!({"reason": "x"})).await,
            Err(ApiError::BadRequest(_))
        ));
        let wrong_attempt = env.assignment("processing").await;
        let refused = env
            .complete(wrong_attempt, 2, json!({"outcome": "done", "summary": "ok"}))
            .await;
        assert!(matches!(&refused, Err(ApiError::BadRequest(message)) if message.contains("current attempt 1")));
        scratch.drop_self().await;
    }

    #[tokio::test]
    async fn a_routing_error_rolls_the_completion_back() {
        let scratch = scratch_or_skip!("reply_atomic");
        let env = Env::new(&scratch.db, "at").await;
        let task = env.assignment("acknowledged").await;
        exec(&env.db, "DELETE FROM work_items WHERE id = $1", vec![env.issue.into()]).await;
        let refused = env.complete(task, 1, json!({"outcome": "done", "summary": "ok"})).await;
        assert!(matches!(refused, Err(ApiError::Conflict(_))));
        assert_eq!(
            env.status(task).await,
            "acknowledged",
            "the status change was rolled back"
        );
        assert_eq!(
            count(&env.db, "SELECT COUNT(*)::bigint AS count FROM ai_task_events WHERE task_id = $1 AND event_type IN ('completed', 'reply_routed')", vec![task.into()]).await,
            0
        );
        // The task can still end: a failure with a missing issue only warns.
        let failed = env.fail(task, 1, json!({"reason": "issue gone"})).await.expect("fail");
        assert_eq!(data(&failed)["reply"]["warnings"], json!(["issue not found"]));
        assert_eq!(env.status(task).await, "failed");
        scratch.drop_self().await;
    }

    /// Agent text never starts anything: no mention task, no notification, whatever it names.
    #[tokio::test]
    async fn a_reply_that_names_people_and_bots_creates_no_task_and_no_notification() {
        let scratch = scratch_or_skip!("reply_guard");
        let env = Env::new(&scratch.db, "gd").await;
        let other_bot = seed_bot_user(&env.db, &env.tenant).await;
        let person = seed_member(&env.db, &env.tenant, "member").await;
        exec(
            &env.db,
            "UPDATE users SET name = 'otherbot' WHERE id = $1",
            vec![other_bot.into()],
        )
        .await;
        let tasks = "SELECT COUNT(*)::bigint AS count FROM ai_tasks";
        let notifications = "SELECT COUNT(*)::bigint AS count FROM notifications";

        let task = env.assignment("processing").await;
        let tasks_before = count(&env.db, tasks, vec![]).await;
        let notifications_before = count(&env.db, notifications, vec![]).await;
        let text = format!(
            "Thanks @otherbot and @{person}; also @{} please review {other_bot}",
            env.bot
        );
        env.complete(task, 1, json!({"outcome": "done", "summary": text}))
            .await
            .expect("complete");
        let second = env.assignment("processing").await;
        env.fail(second, 1, json!({"reason": text})).await.expect("fail");

        assert_eq!(
            count(&env.db, tasks, vec![]).await,
            tasks_before + 1,
            "only the fixture task of the second call was added"
        );
        assert_eq!(count(&env.db, notifications, vec![]).await, notifications_before);
        assert_eq!(
            count(
                &env.db,
                "SELECT COUNT(*)::bigint AS count FROM ai_tasks WHERE status = 'pending'",
                vec![]
            )
            .await,
            0,
            "nothing was queued"
        );
        scratch.drop_self().await;
    }

    #[tokio::test]
    async fn a_start_refusal_is_reported_on_the_issue_for_configured_projects() {
        let scratch = scratch_or_skip!("reply_start_refusal");
        let env = Env::new(&scratch.db, "sr").await;
        let state = state_for(env.db.clone());
        let task_id = env.assignment("failed").await;
        let task = load_task_with_protocol(&env.db, task_id).await.expect("task").task;

        announce_start_refusal(&state, &task, "directory is outside the allowed roots")
            .await
            .expect("no settings");
        assert!(
            env.comments().await.is_empty(),
            "a project without settings gets no comment"
        );

        env.settings("auto", None).await;
        exec(
            &env.db,
            "UPDATE project_bot_connections SET enabled = false WHERE project_id = $1",
            vec![env.tenant.project_id.into()],
        )
        .await;
        announce_start_refusal(
            &state,
            &task,
            "the runner refused the task (path_outside_allowlist): the directory is outside the allowed roots",
        )
        .await
        .expect("disabled");
        let comments = env.comments().await;
        assert_eq!(
            comments.len(),
            1,
            "a configured project gets the comment even when disabled"
        );
        assert_eq!(comments[0].0, env.bot);
        assert_eq!(
            comments[0].1,
            "Could not start: the runner refused the task (path_outside_allowlist): \
             the directory is outside the allowed roots"
        );

        exec(
            &env.db,
            "UPDATE project_bot_connections SET enabled = true WHERE project_id = $1",
            vec![env.tenant.project_id.into()],
        )
        .await;
        announce_start_refusal(&state, &task, "the runner has no agent for bot Build bot")
            .await
            .expect("enabled");
        assert_eq!(env.comments().await.len(), 2);
        scratch.drop_self().await;
    }

    #[tokio::test]
    async fn a_malformed_completion_is_refused() {
        let scratch = scratch_or_skip!("reply_validation");
        let env = Env::new(&scratch.db, "vd").await;
        let task = env.assignment("processing").await;
        let artifacts: Vec<Value> = (0..21)
            .map(|n| json!({"kind": "file", "ref": format!("f{n}")}))
            .collect();
        for body in [
            json!({"outcome": "maybe", "summary": "x"}),
            json!({"outcome": "done", "summary": ""}),
            json!({"outcome": "done", "summary": "   "}),
            json!({"outcome": "done", "summary": "x".repeat(20_001)}),
            json!({"outcome": "done", "summary": "x", "artifacts": artifacts}),
            json!({"outcome": "done", "summary": "x", "artifacts": [{"kind": "svn", "ref": "r"}]}),
            json!({"outcome": "done", "summary": "x", "artifacts": [{"kind": "file", "ref": ""}]}),
            json!({"outcome": "done", "summary": "x", "artifacts": [{"kind": "file", "ref": "y".repeat(501)}]}),
            json!({"outcome": "done", "summary": "x", "artifacts": [{"kind": "file", "ref": "r", "label": "l".repeat(201)}]}),
            json!({"outcome": "done", "summary": "x", "vote": "maybe"}),
            json!({"outcome": "done", "summary": "x", "vote_reason": "r".repeat(4001)}),
        ] {
            assert!(
                matches!(env.complete(task, 1, body.clone()).await, Err(ApiError::BadRequest(_))),
                "{body} must be refused"
            );
        }
        for body in [
            json!({"reason": ""}),
            json!({"reason": "r".repeat(2001)}),
            json!({"reason": "x", "source": "model"}),
        ] {
            assert!(
                matches!(env.fail(task, 1, body.clone()).await, Err(ApiError::BadRequest(_))),
                "{body}"
            );
        }
        assert_eq!(env.status(task).await, "processing");
        assert!(env.comments().await.is_empty());
        scratch.drop_self().await;
    }

    const LONG_REASON: &str = "The change is small, well tested and matches the documented design of the module.";

    impl Env {
        /// The bot as a voting participant of the project, and a proposal in its voting window,
        /// linked to the project through the issue. Returns the proposal's text id.
        async fn voting_proposal(&self) -> String {
            let proposal = format!("PROP-{}", Uuid::new_v4());
            exec(
                &self.db,
                "INSERT INTO ai_participants (id, project_id, name, registered_by) \
                 VALUES ($1, $2, 'voter', $3) ON CONFLICT (project_id, id) DO NOTHING",
                vec![
                    self.bot.to_string().into(),
                    self.tenant.project_id.into(),
                    self.tenant.member_id.into(),
                ],
            )
            .await;
            exec(
                &self.db,
                "INSERT INTO proposals (id, title, proposal_type, status, author_id, author_type, content, workspace_id, \
                                        voting_started_at) \
                 VALUES ($1, 'Adopt the plan', 'feature', 'voting', $2, 'human', 'body', $3, now())",
                vec![
                    proposal.clone().into(),
                    self.tenant.member_id.to_string().into(),
                    self.tenant.workspace_id.into(),
                ],
            )
            .await;
            exec(
                &self.db,
                "INSERT INTO proposal_issue_links (proposal_id, issue_id) VALUES ($1, $2)",
                vec![proposal.clone().into(), self.issue.into()],
            )
            .await;
            proposal
        }

        async fn vote_task(&self, proposal: &str) -> Uuid {
            seed_task_for(
                &self.db,
                &self.tenant,
                self.bot,
                "processing",
                1,
                0,
                ("vote_requested", Some("proposal"), None),
                json!({"proposal_id": proposal}),
            )
            .await
        }

        async fn votes(&self, proposal: &str) -> i64 {
            count(
                &self.db,
                "SELECT COUNT(*)::bigint AS count FROM votes WHERE proposal_id = $1 AND voter_id = $2",
                vec![proposal.to_string().into(), self.bot.to_string().into()],
            )
            .await
        }
    }

    #[tokio::test]
    async fn completing_a_vote_task_casts_the_bots_vote() {
        let scratch = scratch_or_skip!("reply_vote");
        let env = Env::new(&scratch.db, "vo").await;
        let proposal = env.voting_proposal().await;
        let task = env.vote_task(&proposal).await;
        assert!(
            load_task_with_protocol(&env.db, task)
                .await
                .expect("task")
                .task
                .reference_id
                .is_none(),
            "a PROP-<uuid> id cannot be a reference; the payload carries it"
        );

        let answer = env
            .complete(
                task,
                1,
                json!({"outcome": "done", "summary": "Voted.", "vote": "yes", "vote_reason": LONG_REASON}),
            )
            .await
            .expect("complete");
        assert_eq!(data(&answer)["reply"]["vote"], "yes");
        assert_eq!(data(&answer)["reply"]["comment_id"], Value::Null);
        assert_eq!(data(&answer)["reply"]["warnings"], json!([]));
        assert_eq!(data(&answer)["task"]["status"], "completed");
        assert_eq!(env.votes(&proposal).await, 1);
        assert!(env.comments().await.is_empty(), "a vote task writes no comment");
        assert_eq!(
            count(
                &env.db,
                "SELECT COUNT(*)::bigint AS count FROM votes WHERE proposal_id = $1 AND choice = 'yes' AND voter_type = 'ai' \
                 AND reason = $2",
                vec![proposal.clone().into(), LONG_REASON.to_string().into()],
            )
            .await,
            1
        );
        scratch.drop_self().await;
    }

    #[tokio::test]
    async fn the_vote_service_returns_the_webhook_to_send_after_commit() {
        use crate::routes::proposal::find_proposal;
        use crate::services::proposals::{VoteActor, cast_vote};
        use sea_orm::TransactionTrait;
        let scratch = scratch_or_skip!("reply_vote_trigger");
        let env = Env::new(&scratch.db, "vg").await;
        let proposal_id = env.voting_proposal().await;
        let state = state_for(env.db.clone());
        let proposal = find_proposal(&state, &proposal_id).await.expect("proposal");
        let txn = env.db.begin().await.expect("transaction");
        let cast = cast_vote(
            &state,
            &txn,
            &VoteActor {
                user_id: env.bot,
                author_type: "ai",
            },
            &proposal,
            " YES ",
            Some(LONG_REASON),
        )
        .await
        .expect("the vote is cast");
        txn.commit().await.expect("commit");
        assert_eq!(cast.vote.choice, "yes");
        assert_eq!(cast.tally.yes, 1);
        let trigger = cast.trigger.expect("a project resolves, so there is a webhook to send");
        assert_eq!(trigger.actor_id, env.bot);
        assert_eq!(trigger.project_id, env.tenant.project_id);
        scratch.drop_self().await;
    }

    #[tokio::test]
    async fn a_refused_vote_leaves_the_task_active_so_the_agent_can_call_again() {
        let scratch = scratch_or_skip!("reply_vote_refused");
        let env = Env::new(&scratch.db, "vr").await;
        let proposal = env.voting_proposal().await;

        // A reason that is too short.
        let task = env.vote_task(&proposal).await;
        let short = env
            .complete(
                task,
                1,
                json!({"outcome": "done", "summary": "x", "vote": "no", "vote_reason": "too short"}),
            )
            .await;
        assert!(
            matches!(&short, Err(ApiError::BadRequest(message)) if message == "reason is required for AI vote and must be at least 50 characters"),
            "{short:?}"
        );
        assert_eq!(env.status(task).await, "processing");
        assert_eq!(env.votes(&proposal).await, 0);
        // The same task then ends with a proper reason.
        env.complete(
            task,
            1,
            json!({"outcome": "done", "summary": "x", "vote": "no", "vote_reason": LONG_REASON}),
        )
        .await
        .expect("the second call succeeds");
        assert_eq!(env.status(task).await, "completed");

        // A closed voting window.
        let late = env.voting_proposal().await;
        exec(
            &env.db,
            "UPDATE proposals SET voting_started_at = now() - interval '10 days' WHERE id = $1",
            vec![late.clone().into()],
        )
        .await;
        let late_task = env.vote_task(&late).await;
        let closed = env
            .complete(
                late_task,
                1,
                json!({"outcome": "done", "summary": "x", "vote": "yes", "vote_reason": LONG_REASON}),
            )
            .await;
        assert!(
            matches!(&closed, Err(ApiError::BadRequest(message)) if message.contains("voting period has ended")),
            "{closed:?}"
        );
        assert_eq!(env.status(late_task).await, "processing");

        // A proposal that is not in voting.
        let draft = env.voting_proposal().await;
        exec(
            &env.db,
            "UPDATE proposals SET status = 'draft' WHERE id = $1",
            vec![draft.clone().into()],
        )
        .await;
        let draft_task = env.vote_task(&draft).await;
        let refused = env
            .complete(
                draft_task,
                1,
                json!({"outcome": "done", "summary": "x", "vote": "yes", "vote_reason": LONG_REASON}),
            )
            .await;
        assert!(
            matches!(&refused, Err(ApiError::BadRequest(message)) if message == "proposal is not in voting status")
        );
        assert_eq!(env.status(draft_task).await, "processing");
        scratch.drop_self().await;
    }

    #[tokio::test]
    async fn an_existing_vote_completes_the_task_with_a_warning() {
        let scratch = scratch_or_skip!("reply_vote_existing");
        let env = Env::new(&scratch.db, "ve").await;
        let proposal = env.voting_proposal().await;
        exec(
            &env.db,
            "INSERT INTO votes (proposal_id, voter_id, voter_type, choice, weight, reason, voted_at) \
             VALUES ($1, $2, 'ai', 'abstain', 1.0, 'earlier', now())",
            vec![proposal.clone().into(), env.bot.to_string().into()],
        )
        .await;
        let task = env.vote_task(&proposal).await;
        let answer = env
            .complete(
                task,
                1,
                json!({"outcome": "done", "summary": "x", "vote": "yes", "vote_reason": LONG_REASON}),
            )
            .await
            .expect("complete");
        assert_eq!(data(&answer)["task"]["status"], "completed");
        assert_eq!(data(&answer)["reply"]["vote"], Value::Null);
        assert_eq!(data(&answer)["reply"]["warnings"], json!(["vote already recorded"]));
        assert_eq!(env.votes(&proposal).await, 1, "the earlier vote stands");
        scratch.drop_self().await;
    }

    #[tokio::test]
    async fn a_vote_task_needs_a_vote_and_cannot_end_with_no_change() {
        let scratch = scratch_or_skip!("reply_vote_missing");
        let env = Env::new(&scratch.db, "vm").await;
        let proposal = env.voting_proposal().await;
        let task = env.vote_task(&proposal).await;

        let missing = env.complete(task, 1, json!({"outcome": "done", "summary": "x"})).await;
        assert!(
            matches!(&missing, Err(ApiError::BadRequest(message)) if message == "vote is required for a proposal_vote task"),
            "{missing:?}"
        );
        let no_change = env
            .complete(
                task,
                1,
                json!({"outcome": "no_change", "summary": "x", "vote": "yes", "vote_reason": LONG_REASON}),
            )
            .await;
        assert!(matches!(&no_change, Err(ApiError::BadRequest(message)) if message.contains("no_change")));
        assert_eq!(env.status(task).await, "processing");
        assert_eq!(env.votes(&proposal).await, 0);

        // A failed vote task writes nothing and casts no vote.
        let failed = env
            .fail(task, 1, json!({"reason": "cannot decide"}))
            .await
            .expect("fail");
        assert_eq!(data(&failed)["reply"]["comment_id"], Value::Null);
        assert_eq!(env.votes(&proposal).await, 0);
        scratch.drop_self().await;
    }

    #[tokio::test]
    async fn a_proposal_of_another_workspace_cannot_be_voted_on_through_a_task() {
        let scratch = scratch_or_skip!("reply_vote_foreign");
        let env = Env::new(&scratch.db, "vf").await;
        let other = Env::new(&scratch.db, "vx").await;
        let foreign = other.voting_proposal().await;
        let task = env.vote_task(&foreign).await;
        let refused = env
            .complete(
                task,
                1,
                json!({"outcome": "done", "summary": "x", "vote": "yes", "vote_reason": LONG_REASON}),
            )
            .await;
        assert!(matches!(refused, Err(ApiError::Conflict(_))));
        assert_eq!(env.status(task).await, "processing");
        assert_eq!(other.votes(&foreign).await, 0);
        scratch.drop_self().await;
    }
}
