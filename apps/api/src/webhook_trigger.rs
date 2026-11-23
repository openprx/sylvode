// This data shape mirrors an established wire or persistence schema and must remain stable.
// Local SQL row types stay beside the queries whose column shapes they mirror.
#![allow(clippy::items_after_statements, clippy::struct_field_names)]

use std::time::Instant;

use hmac::{Hmac, Mac};
use platform::app::AppState;
use platform::timestamp::Timestamp;
use reqwest::header::{CONTENT_TYPE, HeaderMap, HeaderValue, USER_AGENT};
use sea_orm::{ConnectionTrait, DbBackend, FromQueryResult, Statement};
use serde::Serialize;
use serde_json::{Value, json};
use sha2::Sha256;
use uuid::Uuid;

use crate::outbound::{delivery_client, resolve_outbound_target, truncate_string};
use crate::services::bot_identity::is_bot_entity_type;
use crate::services::issue_view::identifier;

/// Hard cap on the response body pulled from a webhook receiver.
///
/// Bounded so a receiver cannot stream an unbounded body into the delivery record.
const WEBHOOK_RESPONSE_BYTE_LIMIT: usize = 64 * 1024;
/// Characters of receiver diagnostics persisted on `webhook_deliveries`.
const WEBHOOK_DIAGNOSTIC_CHARS: usize = 2_000;

#[derive(Debug, Clone, Copy)]
pub enum WebhookEvent {
    IssueCreated,
    IssueUpdated,
    IssueAssigned,
    IssueStateChanged,
    IssueDeleted,
    CommentCreated,
    CommentUpdated,
    CommentDeleted,
    LabelAdded,
    LabelRemoved,
    SprintStarted,
    SprintCompleted,
    ProposalCreated,
    ProposalUpdated,
    ProposalDeleted,
    ProposalSubmitted,
    ProposalVotingStarted,
    ProposalArchived,
    ProposalVoteCast,
    ProjectCreated,
    ProjectUpdated,
    ProjectDeleted,
    MemberAdded,
    MemberRemoved,
    VetoExercised,
    VetoWithdrawn,
    EscalationStarted,
    AppealCreated,
    GovernanceConfigUpdated,
    AiTaskCompleted,
    AiTaskFailed,
}

impl WebhookEvent {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::IssueCreated => "issue.created",
            Self::IssueUpdated => "issue.updated",
            Self::IssueAssigned => "issue.assigned",
            Self::IssueStateChanged => "issue.state_changed",
            Self::IssueDeleted => "issue.deleted",
            Self::CommentCreated => "comment.created",
            Self::CommentUpdated => "comment.updated",
            Self::CommentDeleted => "comment.deleted",
            Self::LabelAdded => "label.added",
            Self::LabelRemoved => "label.removed",
            Self::SprintStarted => "sprint.started",
            Self::SprintCompleted => "sprint.completed",
            Self::ProposalCreated => "proposal.created",
            Self::ProposalUpdated => "proposal.updated",
            Self::ProposalDeleted => "proposal.deleted",
            Self::ProposalSubmitted => "proposal.submitted",
            Self::ProposalVotingStarted => "proposal.voting_started",
            Self::ProposalArchived => "proposal.archived",
            Self::ProposalVoteCast => "proposal.vote_cast",
            Self::ProjectCreated => "project.created",
            Self::ProjectUpdated => "project.updated",
            Self::ProjectDeleted => "project.deleted",
            Self::MemberAdded => "member.added",
            Self::MemberRemoved => "member.removed",
            Self::VetoExercised => "veto.exercised",
            Self::VetoWithdrawn => "veto.withdrawn",
            Self::EscalationStarted => "escalation.started",
            Self::AppealCreated => "appeal.created",
            Self::GovernanceConfigUpdated => "governance_config.updated",
            Self::AiTaskCompleted => "ai.task_completed",
            Self::AiTaskFailed => "ai.task_failed",
        }
    }
}

#[derive(Debug, Clone)]
pub struct TriggerContext {
    pub event: WebhookEvent,
    pub workspace_id: Uuid,
    pub project_id: Uuid,
    pub actor_id: Uuid,
    pub issue_id: Option<Uuid>,
    pub comment_id: Option<Uuid>,
    pub label_id: Option<Uuid>,
    pub sprint_id: Option<Uuid>,
    pub changes: Option<Value>,
    pub mentions: Vec<Uuid>,
    pub extra_data: Option<Value>,
}

#[derive(Debug, Serialize)]
pub struct WebhookPayload {
    pub id: String,
    pub event: String,
    pub timestamp: String,
    pub workspace: PayloadWorkspace,
    pub project: PayloadProject,
    pub actor: PayloadActor,
    pub data: Value,
    pub bot_context: Option<BotContext>,
}

#[derive(Debug, Serialize)]
pub struct PayloadWorkspace {
    pub id: String,
    pub name: String,
}

#[derive(Debug, Serialize)]
pub struct PayloadProject {
    pub id: String,
    pub name: String,
    pub key: String,
}

#[derive(Debug, Serialize)]
pub struct PayloadActor {
    pub id: String,
    pub name: String,
    pub email: String,
    pub entity_type: String,
}

#[derive(Debug, Serialize)]
pub struct BotContext {
    pub is_bot_task: bool,
    pub bot_id: String,
    pub bot_name: String,
    pub bot_agent_type: String,
    pub trigger_reason: String,
    pub webhook_id: String,
}

#[derive(Debug, FromQueryResult)]
struct ActiveWebhookRow {
    id: Uuid,
    url: String,
    secret: String,
    bot_user_id: Option<Uuid>,
}

#[derive(Debug, FromQueryResult)]
struct WorkspaceRow {
    id: Uuid,
    name: String,
}

#[derive(Debug, FromQueryResult)]
struct ProjectRow {
    id: Uuid,
    name: String,
    key: String,
}

#[derive(Debug, FromQueryResult)]
struct ActorRow {
    id: Uuid,
    name: String,
    email: String,
    entity_type: String,
}

#[derive(Debug)]
struct BotIdentity {
    bot_id: Uuid,
    bot_name: String,
    bot_agent_type: String,
}

#[derive(Debug)]
struct DeliveryRecord {
    webhook_id: Uuid,
    event: String,
    payload: Value,
    request_headers: Value,
    response_status: Option<i32>,
    response_body: Option<String>,
    error: Option<String>,
    duration_ms: Option<i64>,
    delivered_at: Timestamp,
}

/// Delivers `ctx` to every active webhook of the workspace that subscribes to its event.
pub fn trigger_webhooks(state: AppState, ctx: TriggerContext) {
    trigger_webhooks_after_ai_tasks(state, ctx, Vec::new());
}

/// [`trigger_webhooks`] for an action that has already queued a worker AI task for each bot in
/// `ai_task_bots`.
///
/// One action gives a bot at most one bot task (see [`bot_task_suppression`]). The AI task is
/// that bot's task for this action, so the deliveries of `ctx` reach the subscription of such a
/// bot as plain notifications, without `bot_context`. Pass the same list to every
/// `trigger_webhooks_after_ai_tasks` call the action makes (an assignment that also changes the
/// state emits `issue.assigned` and `issue.state_changed`).
pub fn trigger_webhooks_after_ai_tasks(state: AppState, ctx: TriggerContext, ai_task_bots: Vec<Uuid>) {
    tokio::spawn(async move {
        if let Err(err) = trigger_webhooks_inner(state, ctx, ai_task_bots).await {
            tracing::warn!(error = %err, "webhook trigger failed");
        }
    });
}

async fn trigger_webhooks_inner(
    state: AppState,
    ctx: TriggerContext,
    ai_task_bots: Vec<Uuid>,
) -> Result<(), sea_orm::DbErr> {
    let webhooks = ActiveWebhookRow::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        r"
            SELECT id, url, secret, bot_user_id
            FROM webhooks
            WHERE workspace_id = $1
              AND active = true
              AND events ? $2
        ",
        vec![ctx.workspace_id.into(), ctx.event.as_str().into()],
    ))
    .all(&state.db)
    .await?;

    if webhooks.is_empty() {
        return Ok(());
    }

    for webhook in webhooks {
        let delivery_id = Uuid::new_v4();
        let payload = match build_payload(&state, &ctx, delivery_id, &webhook, &ai_task_bots).await {
            Ok(v) => v,
            Err(err) => {
                tracing::warn!(webhook_id = %webhook.id, event = ctx.event.as_str(), error = %err, "build payload failed");
                let fallback_payload = json!({
                    "id": delivery_id.to_string(),
                    "event": ctx.event.as_str(),
                    "timestamp": Timestamp::now().to_rfc3339(),
                });
                let _ = record_delivery(
                    &state,
                    DeliveryRecord {
                        webhook_id: webhook.id,
                        event: ctx.event.as_str().to_string(),
                        payload: fallback_payload,
                        request_headers: json!({}),
                        response_status: None,
                        response_body: None,
                        error: Some(format!("build payload failed: {err}")),
                        duration_ms: None,
                        delivered_at: Timestamp::now(),
                    },
                )
                .await;
                continue;
            }
        };

        let _ = deliver_webhook(&state, &webhook, payload).await;
    }

    Ok(())
}

async fn build_payload(
    state: &AppState,
    ctx: &TriggerContext,
    delivery_id: Uuid,
    webhook: &ActiveWebhookRow,
    ai_task_bots: &[Uuid],
) -> Result<WebhookPayload, sea_orm::DbErr> {
    let workspace = WorkspaceRow::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "SELECT id, name FROM workspaces WHERE id = $1",
        vec![ctx.workspace_id.into()],
    ))
    .one(&state.db)
    .await?
    .ok_or_else(|| sea_orm::DbErr::Custom("workspace not found".to_string()))?;

    let project = if ctx.project_id.is_nil() {
        ProjectRow {
            id: Uuid::nil(),
            name: "workspace".to_string(),
            key: "WORKSPACE".to_string(),
        }
    } else {
        ProjectRow::find_by_statement(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT id, name, key FROM projects WHERE id = $1",
            vec![ctx.project_id.into()],
        ))
        .one(&state.db)
        .await?
        .ok_or_else(|| sea_orm::DbErr::Custom("project not found".to_string()))?
    };

    let actor = ActorRow::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "SELECT id, name, email, entity_type FROM users WHERE id = $1",
        vec![ctx.actor_id.into()],
    ))
    .one(&state.db)
    .await?
    .ok_or_else(|| sea_orm::DbErr::Custom("actor not found".to_string()))?;

    let built_data = build_event_data(state, ctx, &project.key).await?;

    let mut bot_context = None;
    if let Some(candidate) = bot_task_candidate(ctx.event, &built_data.assignee_ids, &ctx.mentions, webhook.bot_user_id)
    {
        if let Some(suppression) = bot_task_suppression(candidate.bot_id, ctx.actor_id, ai_task_bots) {
            tracing::debug!(
                webhook_id = %webhook.id,
                event = ctx.event.as_str(),
                bot_id = %candidate.bot_id,
                reason = suppression.as_str(),
                "delivered without bot_context"
            );
        } else if let Some(identity) = load_bot_identity(&state.db, candidate.bot_id).await {
            bot_context = Some(BotContext {
                is_bot_task: true,
                bot_id: identity.bot_id.to_string(),
                bot_name: identity.bot_name,
                bot_agent_type: identity.bot_agent_type,
                trigger_reason: candidate.trigger_reason.to_string(),
                webhook_id: webhook.id.to_string(),
            });
        }
    }

    Ok(WebhookPayload {
        id: delivery_id.to_string(),
        event: ctx.event.as_str().to_string(),
        timestamp: Timestamp::now().to_rfc3339(),
        workspace: PayloadWorkspace {
            id: workspace.id.to_string(),
            name: workspace.name,
        },
        project: PayloadProject {
            id: project.id.to_string(),
            name: project.name,
            key: project.key,
        },
        actor: PayloadActor {
            id: actor.id.to_string(),
            name: actor.name,
            email: actor.email,
            entity_type: actor.entity_type,
        },
        data: built_data.data,
        bot_context,
    })
}

struct BuiltEventData {
    data: Value,
    assignee_ids: Vec<Uuid>,
}

async fn build_event_data(
    state: &AppState,
    ctx: &TriggerContext,
    project_key: &str,
) -> Result<BuiltEventData, sea_orm::DbErr> {
    match ctx.event {
        WebhookEvent::IssueCreated
        | WebhookEvent::IssueUpdated
        | WebhookEvent::IssueAssigned
        | WebhookEvent::IssueStateChanged => {
            let (issue, assignee_ids) = if let Some(extra) = ctx.extra_data.clone() {
                let ids = extract_assignee_ids(&extra);
                (extra, ids)
            } else {
                let issue_id = ctx
                    .issue_id
                    .ok_or_else(|| sea_orm::DbErr::Custom("missing issue_id".to_string()))?;
                issue_snapshot(&state.db, issue_id)
                    .await?
                    .ok_or_else(|| sea_orm::DbErr::Custom("issue not found".to_string()))?
            };

            let data = if matches!(ctx.event, WebhookEvent::IssueUpdated) {
                json!({
                    "issue": issue,
                    "changes": ctx.changes.clone().unwrap_or_else(|| json!({})),
                })
            } else if matches!(ctx.event, WebhookEvent::IssueAssigned) {
                json!({
                    "issue": issue,
                    "changes": ctx.changes.clone().unwrap_or_else(|| json!({
                        "assignee_ids": {"old": [], "new": []}
                    })),
                })
            } else if matches!(ctx.event, WebhookEvent::IssueStateChanged) {
                json!({
                    "issue": issue,
                    "changes": ctx.changes.clone().unwrap_or_else(|| json!({
                        "state": {"old": null, "new": null}
                    })),
                })
            } else {
                json!({ "issue": issue })
            };

            Ok(BuiltEventData { data, assignee_ids })
        }
        WebhookEvent::IssueDeleted => {
            let issue = ctx
                .extra_data
                .clone()
                .unwrap_or_else(|| json!({ "id": ctx.issue_id.map(|v| v.to_string()) }));
            let assignee_ids = extract_assignee_ids(&issue);
            Ok(BuiltEventData {
                data: json!({ "issue": issue }),
                assignee_ids,
            })
        }
        WebhookEvent::CommentCreated | WebhookEvent::CommentUpdated | WebhookEvent::CommentDeleted => {
            let (comment, issue_summary) = if let Some(extra) = ctx.extra_data.clone() {
                let issue = extra
                    .get("issue")
                    .cloned()
                    .unwrap_or_else(|| json!({ "id": ctx.issue_id.map(|v| v.to_string()) }));
                let comment = extra
                    .get("comment")
                    .cloned()
                    .unwrap_or_else(|| json!({ "id": ctx.comment_id.map(|v| v.to_string()) }));
                (comment, issue)
            } else {
                let comment_id = ctx
                    .comment_id
                    .ok_or_else(|| sea_orm::DbErr::Custom("missing comment_id".to_string()))?;
                fetch_comment_payload(state, comment_id, project_key).await?
            };

            let data = if matches!(ctx.event, WebhookEvent::CommentCreated) {
                json!({
                    "comment": comment,
                    "issue": issue_summary,
                    "mentions": ctx
                        .mentions
                        .iter()
                        .map(Uuid::to_string)
                        .collect::<Vec<_>>(),
                })
            } else {
                json!({
                    "comment": comment,
                    "issue": issue_summary,
                })
            };

            Ok(BuiltEventData {
                data,
                assignee_ids: Vec::new(),
            })
        }
        WebhookEvent::LabelAdded | WebhookEvent::LabelRemoved => {
            // A deleted label has no row left to read: its sender passes the data it captured.
            if let Some(extra) = ctx.extra_data.clone() {
                return Ok(BuiltEventData {
                    data: extra,
                    assignee_ids: Vec::new(),
                });
            }
            let issue_id = ctx
                .issue_id
                .ok_or_else(|| sea_orm::DbErr::Custom("missing issue_id".to_string()))?;
            let label_id = ctx
                .label_id
                .ok_or_else(|| sea_orm::DbErr::Custom("missing label_id".to_string()))?;

            #[derive(Debug, FromQueryResult)]
            struct IssueBrief {
                id: Uuid,
                sequence_number: i32,
                title: String,
            }

            #[derive(Debug, FromQueryResult)]
            struct LabelBrief {
                id: Uuid,
                name: String,
                color: String,
            }

            let issue = IssueBrief::find_by_statement(Statement::from_sql_and_values(
                DbBackend::Postgres,
                "SELECT id, sequence_number, title FROM work_items WHERE id = $1",
                vec![issue_id.into()],
            ))
            .one(&state.db)
            .await?
            .ok_or_else(|| sea_orm::DbErr::Custom("issue not found".to_string()))?;

            let label = LabelBrief::find_by_statement(Statement::from_sql_and_values(
                DbBackend::Postgres,
                "SELECT id, name, color FROM labels WHERE id = $1",
                vec![label_id.into()],
            ))
            .one(&state.db)
            .await?
            .ok_or_else(|| sea_orm::DbErr::Custom("label not found".to_string()))?;

            Ok(BuiltEventData {
                data: json!({
                    "issue": {
                        "id": issue.id.to_string(),
                        "key": identifier(project_key, issue.sequence_number),
                        "title": issue.title,
                    },
                    "label": {
                        "id": label.id.to_string(),
                        "name": label.name,
                        "color": label.color,
                    }
                }),
                assignee_ids: Vec::new(),
            })
        }
        WebhookEvent::SprintStarted | WebhookEvent::SprintCompleted => {
            let sprint_id = ctx
                .sprint_id
                .ok_or_else(|| sea_orm::DbErr::Custom("missing sprint_id".to_string()))?;

            #[derive(Debug, FromQueryResult)]
            struct SprintRow {
                id: Uuid,
                name: String,
                description: String,
                status: String,
                start_date: Option<time::Date>,
                end_date: Option<time::Date>,
                created_at: Timestamp,
                updated_at: Timestamp,
            }

            let sprint = SprintRow::find_by_statement(Statement::from_sql_and_values(
                DbBackend::Postgres,
                "SELECT id, name, description, status, start_date, end_date, created_at, updated_at FROM sprints WHERE id = $1",
                vec![sprint_id.into()],
            ))
            .one(&state.db)
            .await?
            .ok_or_else(|| sea_orm::DbErr::Custom("sprint not found".to_string()))?;

            let mut data = json!({
                "sprint": {
                    "id": sprint.id.to_string(),
                    "name": sprint.name,
                    "description": sprint.description,
                    "status": sprint.status,
                    "start_date": sprint.start_date.map(|v| v.to_string()),
                    "end_date": sprint.end_date.map(|v| v.to_string()),
                    "created_at": sprint.created_at.to_rfc3339(),
                    "updated_at": sprint.updated_at.to_rfc3339(),
                }
            });
            // What completing the sprint did with its unfinished items.
            if let Some(rollover) = ctx
                .changes
                .as_ref()
                .and_then(|changes| changes.get("rollover"))
                .filter(|rollover| !rollover.is_null())
                && let Some(object) = data.as_object_mut()
            {
                object.insert("rollover".to_string(), rollover.clone());
            }

            Ok(BuiltEventData {
                data,
                assignee_ids: Vec::new(),
            })
        }
        WebhookEvent::ProposalCreated
        | WebhookEvent::ProposalUpdated
        | WebhookEvent::ProposalDeleted
        | WebhookEvent::ProposalSubmitted
        | WebhookEvent::ProposalVotingStarted
        | WebhookEvent::ProposalArchived
        | WebhookEvent::ProposalVoteCast
        | WebhookEvent::ProjectCreated
        | WebhookEvent::ProjectUpdated
        | WebhookEvent::ProjectDeleted
        | WebhookEvent::MemberAdded
        | WebhookEvent::MemberRemoved
        | WebhookEvent::VetoExercised
        | WebhookEvent::VetoWithdrawn
        | WebhookEvent::EscalationStarted
        | WebhookEvent::AppealCreated
        | WebhookEvent::GovernanceConfigUpdated
        | WebhookEvent::AiTaskCompleted
        | WebhookEvent::AiTaskFailed => Ok(BuiltEventData {
            data: ctx.extra_data.clone().unwrap_or_else(|| json!({})),
            assignee_ids: Vec::new(),
        }),
    }
}

/// The JSON of an issue as every issue event carries it, and the issue's assignee ids.
///
/// `key` is the issue's identifier (`WEB-12`). Read before a delete, it is the object as it was,
/// labels included; `None` when the issue does not exist.
pub(crate) async fn issue_snapshot(
    db: &impl ConnectionTrait,
    issue_id: Uuid,
) -> Result<Option<(Value, Vec<Uuid>)>, sea_orm::DbErr> {
    #[derive(Debug, FromQueryResult)]
    struct IssueRow {
        id: Uuid,
        project_key: String,
        sequence_number: i32,
        title: String,
        description: String,
        state: String,
        priority: String,
        assignee_id: Option<Uuid>,
        sprint_id: Option<Uuid>,
        due_at: Option<Timestamp>,
        created_at: Timestamp,
        updated_at: Timestamp,
    }

    #[derive(Debug, FromQueryResult)]
    struct LabelIdRow {
        label_id: Uuid,
    }

    let Some(issue) = IssueRow::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        r"
            SELECT wi.id, p.key AS project_key, wi.sequence_number, wi.title, wi.description, wi.state,
                   wi.priority, wi.assignee_id, wi.sprint_id, wi.due_at, wi.created_at, wi.updated_at
            FROM work_items wi
            INNER JOIN projects p ON p.id = wi.project_id
            WHERE wi.id = $1
        ",
        vec![issue_id.into()],
    ))
    .one(db)
    .await?
    else {
        return Ok(None);
    };

    let label_rows = LabelIdRow::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "SELECT label_id FROM work_item_labels WHERE work_item_id = $1 ORDER BY label_id",
        vec![issue_id.into()],
    ))
    .all(db)
    .await?;

    let label_ids = label_rows
        .into_iter()
        .map(|row| row.label_id.to_string())
        .collect::<Vec<_>>();

    let assignee_ids = issue.assignee_id.map(|v| vec![v]).unwrap_or_default();

    let assignee_id_values = assignee_ids.iter().map(Uuid::to_string).collect::<Vec<_>>();

    Ok(Some((
        json!({
            "id": issue.id.to_string(),
            "key": identifier(&issue.project_key, issue.sequence_number),
            "title": issue.title,
            "description": issue.description,
            "state": issue.state,
            "priority": issue.priority,
            "assignee_ids": assignee_id_values,
            "label_ids": label_ids,
            "sprint_id": issue.sprint_id.map(|v| v.to_string()),
            "due_at": issue.due_at.map(Timestamp::to_rfc3339),
            "created_at": issue.created_at.to_rfc3339(),
            "updated_at": issue.updated_at.to_rfc3339(),
        }),
        assignee_ids,
    )))
}

async fn fetch_comment_payload(
    state: &AppState,
    comment_id: Uuid,
    project_key: &str,
) -> Result<(Value, Value), sea_orm::DbErr> {
    #[derive(Debug, FromQueryResult)]
    struct CommentRow {
        id: Uuid,
        issue_id: Uuid,
        content: String,
        author_id: Option<Uuid>,
        author_name: Option<String>,
        created_at: Timestamp,
        updated_at: Timestamp,
    }

    #[derive(Debug, FromQueryResult)]
    struct IssueRow {
        id: Uuid,
        sequence_number: i32,
        title: String,
    }

    let comment = CommentRow::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        r"
            SELECT c.id,
                   c.work_item_id AS issue_id,
                   c.body AS content,
                   c.author_id,
                   u.name AS author_name,
                   c.created_at,
                   c.updated_at
            FROM comments c
            LEFT JOIN users u ON c.author_id = u.id
            WHERE c.id = $1
        ",
        vec![comment_id.into()],
    ))
    .one(&state.db)
    .await?
    .ok_or_else(|| sea_orm::DbErr::Custom("comment not found".to_string()))?;

    let issue = IssueRow::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "SELECT id, sequence_number, title FROM work_items WHERE id = $1",
        vec![comment.issue_id.into()],
    ))
    .one(&state.db)
    .await?
    .ok_or_else(|| sea_orm::DbErr::Custom("issue not found".to_string()))?;

    Ok((
        json!({
            "id": comment.id.to_string(),
            "issue_id": comment.issue_id.to_string(),
            "content": comment.content,
            "author_id": comment.author_id.map(|v| v.to_string()),
            "author_name": comment.author_name.unwrap_or_default(),
            "created_at": comment.created_at.to_rfc3339(),
            "updated_at": comment.updated_at.to_rfc3339(),
        }),
        json!({
            "id": issue.id.to_string(),
            "key": identifier(project_key, issue.sequence_number),
            "title": issue.title,
        }),
    ))
}

/// The bot a delivery to a webhook would hand a bot task to, before [`bot_task_suppression`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct BotTaskCandidate {
    bot_id: Uuid,
    trigger_reason: &'static str,
}

/// The webhook's bot is a candidate when it is an assignee of the issue the event is about, or,
/// for `comment.created`, mentioned in the comment. Whether that user really is a bot is checked
/// afterwards by [`load_bot_identity`].
fn bot_task_candidate(
    event: WebhookEvent,
    assignee_ids: &[Uuid],
    mentions: &[Uuid],
    webhook_bot_user_id: Option<Uuid>,
) -> Option<BotTaskCandidate> {
    let bot_id = webhook_bot_user_id?;
    if assignee_ids.contains(&bot_id) {
        return Some(BotTaskCandidate {
            bot_id,
            trigger_reason: default_trigger_reason(event),
        });
    }
    if matches!(event, WebhookEvent::CommentCreated) && mentions.contains(&bot_id) {
        return Some(BotTaskCandidate {
            bot_id,
            trigger_reason: "mentioned",
        });
    }
    None
}

/// Why a delivery that would otherwise be a bot task for a bot goes out without `bot_context`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BotTaskSuppression {
    /// The bot caused the event itself (it changed the state, edited, commented, completed its
    /// own task). A bot is never handed a task by its own action, which would make an agent
    /// trigger itself in a loop.
    OwnAction,
    /// The action already queued a worker AI task for this bot (an assignment or a mention),
    /// which is the bot's one task for it.
    AiTaskQueued,
}

impl BotTaskSuppression {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::OwnAction => "own_action",
            Self::AiTaskQueued => "ai_task_queued",
        }
    }
}

/// The rule that gives one action at most one bot task per bot, and none to the bot that acted.
///
/// The delivery itself is still sent; only its `bot_context` is left out, so the subscription
/// keeps receiving the event as a plain notification and a receiver answers it `not_bot_task`.
pub fn bot_task_suppression(bot_id: Uuid, actor_id: Uuid, ai_task_bots: &[Uuid]) -> Option<BotTaskSuppression> {
    if bot_id == actor_id {
        Some(BotTaskSuppression::OwnAction)
    } else if ai_task_bots.contains(&bot_id) {
        Some(BotTaskSuppression::AiTaskQueued)
    } else {
        None
    }
}

/// Name and agent type of `bot_id`, or `None` when that user is not a bot.
async fn load_bot_identity(db: &impl ConnectionTrait, bot_id: Uuid) -> Option<BotIdentity> {
    let row = match db
        .query_one(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT entity_type, agent_type, name FROM users WHERE id = $1",
            vec![bot_id.into()],
        ))
        .await
    {
        Ok(row) => row?,
        Err(err) => {
            tracing::warn!(bot_id = %bot_id, error = %err, "bot identity lookup failed, delivered without bot_context");
            return None;
        }
    };

    let entity_type: Option<String> = row.try_get("", "entity_type").ok();
    if !entity_type.as_deref().is_some_and(is_bot_entity_type) {
        return None;
    }

    let bot_name: String = row.try_get::<String>("", "name").unwrap_or_else(|_| "Bot".to_string());
    let bot_agent_type = row
        .try_get::<Option<String>>("", "agent_type")
        .ok()
        .flatten()
        .unwrap_or_else(|| "custom".to_string());

    Some(BotIdentity {
        bot_id,
        bot_name,
        bot_agent_type,
    })
}

/// `User-Agent` of every outbound webhook delivery (ADR-0020 D3): the webhook triggers here, the
/// event dispatcher and the worker's AI-task dispatch.
pub const WEBHOOK_USER_AGENT: &str = "Sylvode-Webhook/1.0";

/// The headers a delivery is sent with, and the copy of them stored with the delivery record.
///
/// Built together from the same values so the stored `request_headers` cannot drift from what
/// actually went on the wire.
fn delivery_request_headers(signature: &str, event: &str, delivery_id: &str) -> (HeaderMap, Value) {
    let signature = format!("sha256={signature}");
    let mut headers = HeaderMap::new();
    headers.insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));
    headers.insert(USER_AGENT, HeaderValue::from_static(WEBHOOK_USER_AGENT));
    headers.insert(
        WEBHOOK_SIGNATURE_HEADER,
        HeaderValue::from_str(&signature).unwrap_or(HeaderValue::from_static("sha256=")),
    );
    headers.insert(
        "X-Webhook-Event",
        HeaderValue::from_str(event).unwrap_or(HeaderValue::from_static("unknown")),
    );
    headers.insert(
        "X-Webhook-Delivery",
        HeaderValue::from_str(delivery_id).unwrap_or(HeaderValue::from_static("unknown")),
    );

    let recorded = json!({
        "Content-Type": "application/json",
        "User-Agent": WEBHOOK_USER_AGENT,
        "X-Webhook-Signature": signature,
        "X-Webhook-Event": event,
        "X-Webhook-Delivery": delivery_id,
    });
    (headers, recorded)
}

async fn deliver_webhook(
    state: &AppState,
    webhook: &ActiveWebhookRow,
    payload: WebhookPayload,
) -> Result<(), sea_orm::DbErr> {
    let body = serde_json::to_string(&payload)
        .map_err(|e| sea_orm::DbErr::Custom(format!("serialize payload failed: {e}")))?;

    let signature = sign_payload(&webhook.secret, &body)
        .map_err(|e| sea_orm::DbErr::Custom(format!("sign payload failed: {e}")))?;

    let (headers, request_headers) = delivery_request_headers(&signature, &payload.event, &payload.id);

    let payload_json = serde_json::to_value(&payload)
        .map_err(|e| sea_orm::DbErr::Custom(format!("serialize payload json failed: {e}")))?;

    // The stored URL is re-validated on every delivery, not only when the webhook was saved: the
    // row may predate the checks, may have been edited by another path, and DNS for a host that
    // passed yesterday can point at an internal address today.
    let target = match resolve_outbound_target(&webhook.url).await {
        Ok(target) => target,
        Err(err) => {
            tracing::warn!(webhook_id = %webhook.id, error = %err, "webhook target refused, delivery not attempted");
            let record = DeliveryRecord {
                webhook_id: webhook.id,
                event: payload.event.clone(),
                payload: payload_json,
                request_headers,
                response_status: None,
                response_body: None,
                error: Some(truncate_string(
                    format!("outbound target refused: {err}"),
                    WEBHOOK_DIAGNOSTIC_CHARS,
                )),
                duration_ms: Some(0),
                delivered_at: Timestamp::now(),
            };
            if let Err(record_err) = record_delivery(state, record).await {
                tracing::warn!(webhook_id = %webhook.id, error = %record_err, "webhook delivery record not stored");
            }
            return Ok(());
        }
    };

    // The checked addresses are the ones connected to: the name is not resolved a second time.
    // Redirects are disabled so a target that passed the check cannot bounce the signed request
    // into the internal network afterwards.
    let client = delivery_client(
        target.pin.as_ref(),
        std::time::Duration::from_secs(10),
        std::time::Duration::from_secs(5),
    )
    .map_err(|e| sea_orm::DbErr::Custom(format!("build reqwest client failed: {e}")))?;

    let started = Instant::now();
    let sent = client.post(target.url).headers(headers).body(body).send().await;

    let (response_status, response_body, error) = match sent {
        Ok(resp) => {
            let status = i32::from(resp.status().as_u16());
            let text = truncate_string(
                read_capped_body(resp, WEBHOOK_RESPONSE_BYTE_LIMIT).await,
                WEBHOOK_DIAGNOSTIC_CHARS,
            );
            (Some(status), Some(text), None)
        }
        Err(err) => (
            None,
            None,
            Some(truncate_string(err.to_string(), WEBHOOK_DIAGNOSTIC_CHARS)),
        ),
    };

    let duration_ms = i64::try_from(started.elapsed().as_millis()).unwrap_or(i64::MAX);

    let record = DeliveryRecord {
        webhook_id: webhook.id,
        event: payload.event.clone(),
        payload: payload_json,
        request_headers,
        response_status,
        response_body,
        error,
        duration_ms: Some(duration_ms),
        delivered_at: Timestamp::now(),
    };
    if let Err(record_err) = record_delivery(state, record).await {
        tracing::warn!(webhook_id = %webhook.id, error = %record_err, "webhook delivery record not stored");
    }

    if let Err(err) = state
        .db
        .execute(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "UPDATE webhooks SET last_triggered_at = $1 WHERE id = $2",
            vec![Timestamp::now().into(), webhook.id.into()],
        ))
        .await
    {
        tracing::warn!(webhook_id = %webhook.id, error = %err, "webhook last_triggered_at not updated");
    }

    Ok(())
}

/// Reads at most `limit` bytes of the response.
///
/// `Response::text` pulls the whole body into memory, so a hostile or broken receiver could answer
/// a webhook delivery with an unbounded stream and exhaust the API process, and whatever it sent
/// was then stored verbatim in `webhook_deliveries`.
async fn read_capped_body(mut response: reqwest::Response, limit: usize) -> String {
    let mut collected: Vec<u8> = Vec::new();
    loop {
        match response.chunk().await {
            Ok(Some(chunk)) => {
                let remaining = limit.saturating_sub(collected.len());
                if remaining == 0 {
                    break;
                }
                collected.extend(chunk.iter().take(remaining).copied());
            }
            Ok(None) => break,
            Err(err) => {
                tracing::warn!(error = %err, "webhook response body read failed");
                break;
            }
        }
    }
    String::from_utf8_lossy(&collected).into_owned()
}

async fn record_delivery(state: &AppState, rec: DeliveryRecord) -> Result<(), sea_orm::DbErr> {
    state
        .db
        .execute(Statement::from_sql_and_values(
            DbBackend::Postgres,
            r"
                INSERT INTO webhook_deliveries (
                    id, webhook_id, event_type, payload, request_headers, response_status,
                    response_body, error, duration_ms, delivered_at, created_at, retry_count
                )
                VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, 0)
            ",
            vec![
                Uuid::new_v4().into(),
                rec.webhook_id.into(),
                rec.event.into(),
                rec.payload.into(),
                rec.request_headers.into(),
                rec.response_status.into(),
                rec.response_body.into(),
                rec.error.into(),
                rec.duration_ms.into(),
                rec.delivered_at.into(),
                Timestamp::now().into(),
            ],
        ))
        .await?;

    Ok(())
}

/// Header carrying the outbound HMAC, shared with the worker so both paths agree on the name.
pub const WEBHOOK_SIGNATURE_HEADER: &str = "X-Webhook-Signature";

/// Signs an outbound webhook body the way `X-Webhook-Signature: sha256=<hex>` is verified.
///
/// Shared with the worker's AI-task dispatch so both outbound paths present the same signature
/// instead of one of them delivering unauthenticated.
pub fn sign_payload(secret: &str, raw_body: &str) -> Result<String, String> {
    let mut mac = Hmac::<Sha256>::new_from_slice(secret.as_bytes()).map_err(|e| format!("invalid hmac secret: {e}"))?;
    mac.update(raw_body.as_bytes());
    let bytes = mac.finalize().into_bytes();
    Ok(hex::encode(bytes))
}

fn extract_assignee_ids(issue_payload: &Value) -> Vec<Uuid> {
    issue_payload
        .get("assignee_ids")
        .and_then(Value::as_array)
        .map(|arr| {
            arr.iter()
                .filter_map(Value::as_str)
                .filter_map(|raw| Uuid::parse_str(raw).ok())
                .collect::<Vec<_>>()
        })
        .unwrap_or_default()
}

const fn default_trigger_reason(event: WebhookEvent) -> &'static str {
    match event {
        WebhookEvent::IssueCreated => "created",
        WebhookEvent::IssueStateChanged => "status_changed",
        WebhookEvent::CommentCreated => "mentioned",
        WebhookEvent::AiTaskCompleted => "completed",
        WebhookEvent::AiTaskFailed => "failed",
        _ => "assigned",
    }
}

#[cfg(test)]
mod delivery_tests {
    use super::{
        WEBHOOK_DIAGNOSTIC_CHARS, WEBHOOK_RESPONSE_BYTE_LIMIT, delivery_client, delivery_request_headers,
        read_capped_body,
    };
    use crate::outbound::validate_outbound_url;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    fn build_delivery_client() -> Result<reqwest::Client, reqwest::Error> {
        delivery_client(
            None,
            std::time::Duration::from_secs(10),
            std::time::Duration::from_secs(5),
        )
    }

    /// Serves one request with a fixed raw HTTP response and returns the URL to hit.
    async fn serve_once(response: Vec<u8>) -> String {
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("a loopback port must be bindable");
        let addr = listener.local_addr().expect("the bound port must be readable");
        tokio::spawn(async move {
            if let Ok((mut socket, _)) = listener.accept().await {
                let mut discard = [0_u8; 2048];
                let _read = socket.read(&mut discard).await;
                let _written = socket.write_all(&response).await;
                let _flushed = socket.flush().await;
            }
        });
        format!("http://{addr}/hook")
    }

    /// A receiver that answers a delivery with an unbounded body must not be able to pull the API
    /// process down with it, and whatever it did send must not land verbatim in `webhook_deliveries`.
    #[tokio::test]
    async fn a_response_body_is_capped_before_it_is_stored() {
        let oversized = "A".repeat(WEBHOOK_RESPONSE_BYTE_LIMIT * 2);
        let raw = format!(
            "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nContent-Type: text/plain\r\n\r\n{oversized}",
            oversized.len()
        );
        let url = serve_once(raw.into_bytes()).await;

        let client = build_delivery_client().expect("client must build");
        let response = client.post(&url).send().await.expect("request must complete");
        let body = read_capped_body(response, WEBHOOK_RESPONSE_BYTE_LIMIT).await;

        assert_eq!(
            body.len(),
            WEBHOOK_RESPONSE_BYTE_LIMIT,
            "the body must stop at the cap instead of following the receiver"
        );
        const {
            assert!(
                WEBHOOK_DIAGNOSTIC_CHARS < WEBHOOK_RESPONSE_BYTE_LIMIT,
                "what is stored must be smaller than what is read"
            );
        }
    }

    /// The outbound checks are done once, against one host. Following a redirect would move the
    /// signed delivery to a host that was never checked, which is the whole SSRF bypass.
    #[tokio::test]
    async fn deliveries_do_not_follow_redirects() {
        let raw =
            b"HTTP/1.1 302 Found\r\nLocation: http://169.254.169.254/latest/meta-data/\r\nContent-Length: 0\r\n\r\n";
        let url = serve_once(raw.to_vec()).await;

        let client = build_delivery_client().expect("client must build");
        let response = client.post(&url).send().await.expect("request must complete");

        assert_eq!(
            response.status().as_u16(),
            302,
            "the redirect must be reported, never followed"
        );
        assert_eq!(
            response.url().as_str(),
            url,
            "the request must not have moved to the redirect target"
        );
    }

    /// ADR-0020 D3: the `User-Agent` a delivery puts on the wire is the Sylvode token with the
    /// legacy token in its comment, and the `request_headers` stored with the delivery record
    /// carry exactly that value.
    #[tokio::test]
    async fn the_delivery_user_agent_on_the_wire_matches_the_recorded_one() {
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("a loopback port must be bindable");
        let addr = listener.local_addr().expect("the bound port must be readable");
        let received = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.expect("the delivery connects");
            let mut raw = Vec::new();
            let mut buffer = [0_u8; 4096];
            while !raw.windows(4).any(|window| window == b"\r\n\r\n") {
                let read = socket.read(&mut buffer).await.expect("the request is readable");
                if read == 0 {
                    break;
                }
                raw.extend_from_slice(buffer.get(..read).unwrap_or_default());
            }
            let _written = socket
                .write_all(b"HTTP/1.1 204 No Content\r\nContent-Length: 0\r\n\r\n")
                .await;
            String::from_utf8_lossy(&raw).into_owned()
        });

        let (headers, recorded) = delivery_request_headers("abc123", "issue.created", "delivery-1");
        let client = build_delivery_client().expect("client must build");
        client
            .post(format!("http://{addr}/hook"))
            .headers(headers)
            .body("{}")
            .send()
            .await
            .expect("request must complete");
        let raw = received.await.expect("the receiver finishes");

        let sent: Vec<&str> = raw
            .lines()
            .filter_map(|line| line.split_once(':'))
            .filter(|(name, _)| name.eq_ignore_ascii_case("user-agent"))
            .map(|(_, value)| value.trim())
            .collect();
        assert_eq!(sent, ["Sylvode-Webhook/1.0"], "exactly one User-Agent: {raw}");
        assert_eq!(
            recorded.get("User-Agent").and_then(serde_json::Value::as_str),
            sent.first().copied()
        );
        assert_eq!(
            recorded.get("X-Webhook-Signature").and_then(serde_json::Value::as_str),
            Some("sha256=abc123")
        );
    }

    /// Delivery re-validates the stored URL, so a webhook row that predates the checks, or that was
    /// edited elsewhere, cannot reach an internal address.
    #[tokio::test]
    async fn internal_targets_are_refused_before_the_request_is_built() {
        for url in [
            "http://127.0.0.1/hook",
            "http://169.254.169.254/latest/meta-data/",
            "http://user:pass@example.com/hook",
            "file:///etc/passwd",
        ] {
            assert!(
                validate_outbound_url(url).await.is_err(),
                "{url} must not be deliverable"
            );
        }
    }
}

/// The rule that decides whether a delivery is a bot task: who is a candidate, and when a
/// candidate's task is left out because the action already gave it one or because it acted
/// itself.
#[cfg(test)]
mod bot_task_rule_tests {
    use super::{BotTaskCandidate, BotTaskSuppression, WebhookEvent, bot_task_candidate, bot_task_suppression};
    use uuid::Uuid;

    /// Number of deliveries of one action that reach `bot`'s webhook as bot tasks.
    fn webhook_bot_tasks(
        bot: Uuid,
        actor: Uuid,
        deliveries: &[(WebhookEvent, &[Uuid], &[Uuid])],
        ai_task_bots: &[Uuid],
    ) -> usize {
        deliveries
            .iter()
            .filter_map(|(event, assignees, mentions)| bot_task_candidate(*event, assignees, mentions, Some(bot)))
            .filter(|candidate| bot_task_suppression(candidate.bot_id, actor, ai_task_bots).is_none())
            .count()
    }

    #[test]
    fn the_webhook_bot_is_a_candidate_as_assignee_or_mentioned_in_a_comment() {
        let bot = Uuid::new_v4();
        let other = Uuid::new_v4();
        assert_eq!(
            bot_task_candidate(WebhookEvent::IssueStateChanged, &[bot], &[], Some(bot)),
            Some(BotTaskCandidate {
                bot_id: bot,
                trigger_reason: "status_changed"
            })
        );
        assert_eq!(
            bot_task_candidate(WebhookEvent::CommentCreated, &[], &[bot], Some(bot)),
            Some(BotTaskCandidate {
                bot_id: bot,
                trigger_reason: "mentioned"
            })
        );
        assert_eq!(
            bot_task_candidate(WebhookEvent::IssueUpdated, &[other], &[], Some(bot)),
            None
        );
        assert_eq!(
            bot_task_candidate(WebhookEvent::CommentUpdated, &[], &[bot], Some(bot)),
            None
        );
        assert_eq!(bot_task_candidate(WebhookEvent::IssueCreated, &[bot], &[], None), None);
        // AI task events carry no assignees, so they stay plain notifications.
        assert_eq!(
            bot_task_candidate(WebhookEvent::AiTaskCompleted, &[], &[], Some(bot)),
            None
        );
    }

    #[test]
    fn a_bot_gets_no_task_from_its_own_action_and_none_twice() {
        let bot = Uuid::new_v4();
        let human = Uuid::new_v4();
        assert_eq!(bot_task_suppression(bot, bot, &[]), Some(BotTaskSuppression::OwnAction));
        assert_eq!(
            bot_task_suppression(bot, bot, &[bot]),
            Some(BotTaskSuppression::OwnAction)
        );
        assert_eq!(
            bot_task_suppression(bot, human, &[bot]),
            Some(BotTaskSuppression::AiTaskQueued)
        );
        assert_eq!(bot_task_suppression(bot, human, &[]), None);
        assert_eq!(bot_task_suppression(bot, human, &[Uuid::new_v4()]), None);
    }

    /// Creating an issue assigned to a bot emits `issue.created` and `issue.assigned` and queues
    /// one AI task; neither delivery may be a second (or third) task.
    #[test]
    fn create_with_a_bot_assignee_is_one_task() {
        let bot = Uuid::new_v4();
        let human = Uuid::new_v4();
        let deliveries: [(WebhookEvent, &[Uuid], &[Uuid]); 2] = [
            (WebhookEvent::IssueCreated, &[bot], &[]),
            (WebhookEvent::IssueAssigned, &[bot], &[]),
        ];
        let ai_tasks = [bot];
        assert_eq!(
            ai_tasks.len() + webhook_bot_tasks(bot, human, &deliveries, &ai_tasks),
            1
        );
    }

    /// Assigning later, also together with a state change and an edit, is one task.
    #[test]
    fn assigning_an_existing_issue_to_a_bot_is_one_task() {
        let bot = Uuid::new_v4();
        let human = Uuid::new_v4();
        let deliveries: [(WebhookEvent, &[Uuid], &[Uuid]); 3] = [
            (WebhookEvent::IssueAssigned, &[bot], &[]),
            (WebhookEvent::IssueStateChanged, &[bot], &[]),
            (WebhookEvent::IssueUpdated, &[bot], &[]),
        ];
        let ai_tasks = [bot];
        assert_eq!(
            ai_tasks.len() + webhook_bot_tasks(bot, human, &deliveries, &ai_tasks),
            1
        );
    }

    #[test]
    fn a_mention_is_one_task() {
        let bot = Uuid::new_v4();
        let human = Uuid::new_v4();
        let deliveries: [(WebhookEvent, &[Uuid], &[Uuid]); 1] = [(WebhookEvent::CommentCreated, &[], &[bot])];
        let ai_tasks = [bot];
        assert_eq!(
            ai_tasks.len() + webhook_bot_tasks(bot, human, &deliveries, &ai_tasks),
            1
        );
    }

    #[test]
    fn state_changes_reach_the_assigned_bot_unless_it_made_them() {
        let bot_a = Uuid::new_v4();
        let bot_b = Uuid::new_v4();
        let human = Uuid::new_v4();
        let deliveries: [(WebhookEvent, &[Uuid], &[Uuid]); 1] = [(WebhookEvent::IssueStateChanged, &[bot_a], &[])];
        assert_eq!(webhook_bot_tasks(bot_a, bot_a, &deliveries, &[]), 0, "own change");
        assert_eq!(
            webhook_bot_tasks(bot_a, human, &deliveries, &[]),
            1,
            "a person's change"
        );
        assert_eq!(
            webhook_bot_tasks(bot_a, bot_b, &deliveries, &[]),
            1,
            "another bot's change"
        );
    }
}
