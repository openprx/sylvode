//! Processing of one authenticated, signature-checked inbound event.
//!
//! Everything after the envelope check runs in one database transaction:
//!
//! 1. the event is stored once per source and `event_id` (a repeat is a duplicate, a repeat with a
//!    different body is a conflict);
//! 2. the first matching rule is found and its row locked, which serialises the cap checks of
//!    concurrent events of the same rule;
//! 3. the event is attached to the open alert of its `dedup_key`, or opens a new alert work item;
//! 4. when the rule hands alerts to a bot, one task is created through the central dispatch guard
//!    (`DispatchOrigin::InboundRule`) unless an alert-level or rule-level limit says no.
//!
//! Outbound webhooks run after the commit. The answer never tells the source which rule matched or
//! whether a task was created.

use axum::{
    Extension,
    extract::State,
    http::StatusCode,
    response::{IntoResponse, Response},
};
use platform::{app::AppState, timestamp::Timestamp};
use sea_orm::{ConnectionTrait, DbBackend, FromQueryResult, Statement, TransactionTrait};
use serde_json::{Value, json};
use uuid::Uuid;

use super::{
    audit,
    auth::{SourceRow, VerifiedRequest},
    description::{self, AlertFacts},
    envelope::{Envelope, EnvelopeError, parse_envelope},
    notify::{self, AlertNotification, Subject},
    response::{InboundError, answer},
    rules::{RuleRow, first_matching_rule, rule_matches},
};
use crate::{
    error::ApiError,
    services::{
        ai_task_service::{CreateAiTaskInput, DispatchOrigin, create_ai_task, runner_bot_block},
        work_item_service::{NewWorkItem, insert_work_item},
        workflow_service::{EffectiveWorkflow, resolve_effective_workflow_for_project},
    },
    webhook_trigger::{TriggerContext, WebhookEvent, trigger_webhooks_after_ai_tasks},
};

/// Statuses in which a task still occupies a slot. One list for every cap and every check, so the
/// runner protocol's `acknowledged` joins it without another change.
pub const ACTIVE_TASK_STATUSES: &str = "('pending','processing','acknowledged')";
const HOUR: &str = "interval '1 hour'";

/// What happened to a stored event.
struct Decision {
    outcome: &'static str,
    rule_id: Option<Uuid>,
    alert_id: Option<Uuid>,
    task_id: Option<Uuid>,
    /// Set when a work item was opened: what to deliver to webhook subscribers after the commit.
    opened: Option<Opened>,
}

impl Decision {
    const fn none(outcome: &'static str) -> Self {
        Self {
            outcome,
            rule_id: None,
            alert_id: None,
            task_id: None,
            opened: None,
        }
    }
}

struct Opened {
    work_item_id: Uuid,
    /// The bot the work item is assigned to, so its subscription is not also handed the same work
    /// as a plain `issue.created`.
    assigned_bot: Option<Uuid>,
}

enum Processed {
    Stored { event_id: Uuid, opened: Option<Opened> },
    Duplicate(Uuid),
    Conflict,
}

#[derive(FromQueryResult)]
struct ExistingEvent {
    id: Uuid,
    body_sha256: String,
}

#[derive(FromQueryResult)]
struct CountRow {
    n: i64,
}

#[derive(FromQueryResult)]
struct IdRow {
    id: Uuid,
}

/// An open alert with the facts the decisions need.
#[derive(FromQueryResult)]
struct AlertRow {
    id: Uuid,
    work_item_id: Uuid,
    occurrences: i32,
    max_severity_rank: i16,
    task_seq: i32,
    fresh: bool,
    task_active: bool,
    rearm_active: bool,
    wi_state: String,
    wi_title: String,
    wi_assignee: Option<Uuid>,
}

/// The facts of the alert an event was attached to or opened.
struct AlertContext {
    id: Uuid,
    work_item_id: Uuid,
    title: String,
    assignee: Option<Uuid>,
    occurrences: i32,
    task_seq: i32,
    task_active: bool,
    rearm_active: bool,
    opened_now: bool,
}

/// The handler of `POST /api/v1/inbound/events`, behind [`super::auth::inbound_source_auth`].
pub async fn post_event(State(state): State<AppState>, Extension(request): Extension<VerifiedRequest>) -> Response {
    let source = &request.source;
    let envelope = match parse_envelope(&request.body, Timestamp::now()) {
        Ok(envelope) => envelope,
        Err(error) => {
            let (reason, refusal) = match error {
                EnvelopeError::Invalid { path, reason } => {
                    ("invalid_envelope", InboundError::invalid_envelope(&path, &reason))
                }
                EnvelopeError::OccurredAtOutOfRange => (
                    "occurred_at_out_of_range",
                    InboundError::new(StatusCode::UNPROCESSABLE_ENTITY, "occurred_at_out_of_range"),
                ),
            };
            if let Err(error) = audit::record_rejection(
                &state.db,
                source.workspace_id,
                source.project_id,
                source.id,
                reason,
                request.client,
            )
            .await
            {
                tracing::warn!(error = %error, "inbound rejection audit failed");
            }
            return refusal.into_response();
        }
    };

    match process(&state, &request, &envelope).await {
        Ok(Processed::Stored { event_id, opened }) => {
            if let Some(opened) = opened {
                trigger_after_open(&state, source, &opened);
            }
            answer(StatusCode::ACCEPTED, "accepted", event_id)
        }
        Ok(Processed::Duplicate(event_id)) => answer(StatusCode::OK, "duplicate", event_id),
        Ok(Processed::Conflict) => {
            if let Err(error) = audit::record_rejection(
                &state.db,
                source.workspace_id,
                source.project_id,
                source.id,
                "event_id_conflict",
                request.client,
            )
            .await
            {
                tracing::warn!(error = %error, "inbound rejection audit failed");
            }
            InboundError::event_id_conflict().into_response()
        }
        Err(error) => {
            tracing::error!(error = %error, source_id = %source.id, "inbound event processing failed");
            InboundError::store_unavailable().into_response()
        }
    }
}

/// Hands the new work item to webhook subscribers, as `create_issue` does for a work item created
/// with an assignee.
fn trigger_after_open(state: &AppState, source: &SourceRow, opened: &Opened) {
    let Some(actor_id) = source.acting_user_id else {
        return;
    };
    trigger_webhooks_after_ai_tasks(
        state.clone(),
        TriggerContext {
            event: WebhookEvent::IssueCreated,
            workspace_id: source.workspace_id,
            project_id: source.project_id,
            actor_id,
            issue_id: Some(opened.work_item_id),
            comment_id: None,
            label_id: None,
            sprint_id: None,
            changes: None,
            mentions: Vec::new(),
            extra_data: None,
        },
        opened.assigned_bot.into_iter().collect(),
    );
}

async fn process(state: &AppState, request: &VerifiedRequest, envelope: &Envelope) -> Result<Processed, ApiError> {
    let source = &request.source;
    // Read before the transaction opens, so the transaction never waits for a second connection.
    let workflow = resolve_effective_workflow_for_project(state, source.project_id).await?;

    let tx = state.db.begin().await?;
    let event_id = Uuid::new_v4();
    if !insert_event(&tx, event_id, request, envelope).await? {
        let existing = ExistingEvent::find_by_statement(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT id, body_sha256 FROM inbound_events WHERE source_id = $1 AND event_id = $2",
            vec![source.id.into(), envelope.event_id.clone().into()],
        ))
        .one(&tx)
        .await?
        .ok_or_else(|| ApiError::NotFound("inbound event".to_owned()))?;
        return Ok(if existing.body_sha256.trim() == request.body_sha256 {
            Processed::Duplicate(existing.id)
        } else {
            Processed::Conflict
        });
    }

    let decision = decide(&tx, request, envelope, &workflow, event_id).await?;

    tx.execute(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "UPDATE inbound_events SET outcome = $2, rule_id = $3, alert_id = $4, task_id = $5 WHERE id = $1",
        vec![
            event_id.into(),
            decision.outcome.into(),
            decision.rule_id.into(),
            decision.alert_id.into(),
            decision.task_id.into(),
        ],
    ))
    .await?;
    audit::record(
        &tx,
        source.workspace_id,
        source.project_id,
        "inbound_event.accepted",
        "inbound_event",
        event_id,
        source.acting_user_id,
        audit::inbound_source_value(source.id),
        json!({
            "source_id": source.id,
            "event_id": envelope.event_id,
            "kind": envelope.kind,
            "severity": envelope.severity.as_str(),
            "outcome": decision.outcome,
            "rule_id": decision.rule_id,
            "alert_id": decision.alert_id,
            "task_id": decision.task_id,
        }),
    )
    .await?;
    tx.commit().await?;
    Ok(Processed::Stored {
        event_id,
        opened: decision.opened,
    })
}

/// Stores the event with the provisional outcome `no_rule`. `false` when the source already has an
/// event with this `event_id`.
async fn insert_event<C: ConnectionTrait>(
    db: &C,
    id: Uuid,
    request: &VerifiedRequest,
    envelope: &Envelope,
) -> Result<bool, ApiError> {
    let source = &request.source;
    let inserted = IdRow::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "INSERT INTO inbound_events (id, source_id, workspace_id, project_id, event_id, body_sha256, kind, \
         severity_rank, dedup_key, occurred_at, title, summary, subject, payload, signed, client_ip, outcome) \
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15, $16::inet, 'no_rule') \
         ON CONFLICT (source_id, event_id) DO NOTHING RETURNING id",
        vec![
            id.into(),
            source.id.into(),
            source.workspace_id.into(),
            source.project_id.into(),
            envelope.event_id.clone().into(),
            request.body_sha256.clone().into(),
            envelope.kind.clone().into(),
            envelope.severity.rank().into(),
            envelope.dedup_key.clone().into(),
            envelope.occurred_at.into(),
            envelope.title.clone().into(),
            envelope.summary.clone().into(),
            Value::Object(envelope.subject.clone()).into(),
            envelope.payload.clone().into(),
            request.signed.into(),
            request.client.to_string().into(),
        ],
    ))
    .one(db)
    .await?;
    Ok(inserted.is_some())
}

async fn scalar<C: ConnectionTrait>(db: &C, sql: &str, values: Vec<sea_orm::Value>) -> Result<i64, ApiError> {
    Ok(
        CountRow::find_by_statement(Statement::from_sql_and_values(DbBackend::Postgres, sql, values))
            .one(db)
            .await?
            .map_or(0, |row| row.n),
    )
}

/// Steps 2 to 7 of the processing: rule, alert, task.
async fn decide<C: ConnectionTrait>(
    tx: &C,
    request: &VerifiedRequest,
    envelope: &Envelope,
    workflow: &EffectiveWorkflow,
    event_id: Uuid,
) -> Result<Decision, ApiError> {
    let source = &request.source;
    let Some(candidate) =
        first_matching_rule(tx, source.project_id, source.id, &envelope.kind, envelope.severity).await?
    else {
        return Ok(Decision::none("no_rule"));
    };
    // The row lock serialises the cap checks of concurrent events of this rule. The rule is read
    // again under it, so an edit between the two reads is honoured.
    let Some(rule) = lock_rule(tx, candidate.id).await? else {
        return Ok(Decision::none("no_rule"));
    };
    if !rule_matches(&rule, source.id, &envelope.kind, envelope.severity) {
        return Ok(Decision::none("no_rule"));
    }

    let bot = if rule.wants_task() {
        valid_bot(tx, rule.bot_user_id, source.workspace_id).await?
    } else {
        None
    };
    let bot_unavailable = rule.wants_task() && bot.is_none();

    let mut decision = Decision {
        rule_id: Some(rule.id),
        ..Decision::none(if bot_unavailable {
            "bot_unavailable"
        } else {
            "alert_attached"
        })
    };

    let reusable = match find_open_alert(tx, source, envelope, rule.dedup_window_secs, rule.rearm_after_secs).await? {
        Some(alert) if alert.fresh && !is_terminal(workflow, &alert.wi_state) => Some(alert),
        Some(alert) => {
            close_alert(tx, source, &alert, "window_or_terminal_state").await?;
            None
        }
        None => None,
    };

    let alert = if let Some(row) = reusable {
        attach(tx, envelope, row).await?
    } else {
        if opened_this_hour(tx, rule.id).await? >= i64::from(request.config.max_alerts_per_rule_per_hour) {
            note_suppression(tx, &rule, source, "alert_cap", None).await?;
            decision.outcome = "suppressed_alert_cap";
            return Ok(decision);
        }
        open_alert(tx, request, envelope, workflow, &rule, bot, event_id).await?
    };
    decision.alert_id = Some(alert.id);
    if alert.opened_now {
        decision.outcome = if bot_unavailable {
            "bot_unavailable"
        } else {
            "alert_opened"
        };
        decision.opened = Some(Opened {
            work_item_id: alert.work_item_id,
            assigned_bot: bot,
        });
    }
    if bot_unavailable {
        note_suppression(tx, &rule, source, "bot_unavailable", Some(alert.work_item_id)).await?;
    }

    if let Some(bot) = bot {
        match decide_task(tx, request, envelope, &rule, &alert, bot, event_id).await? {
            TaskDecision::Created(task_id) => decision.task_id = Some(task_id),
            TaskDecision::Skipped => {}
            TaskDecision::Suppressed(cap) => {
                decision.outcome = match cap {
                    "task_cap" => "suppressed_task_cap",
                    "active_cap" => "suppressed_active_cap",
                    "workspace_cap" => "suppressed_workspace_cap",
                    // The runner bot has no enabled connection in this project: like a bot that
                    // is unavailable, the alert opens and no task starts.
                    _ => "bot_unavailable",
                };
                note_suppression(tx, &rule, source, cap, Some(alert.work_item_id)).await?;
            }
        }
    }
    Ok(decision)
}

fn is_terminal(workflow: &EffectiveWorkflow, state: &str) -> bool {
    workflow
        .states
        .iter()
        .any(|candidate| candidate.key == state && candidate.is_terminal)
}

async fn lock_rule<C: ConnectionTrait>(tx: &C, rule_id: Uuid) -> Result<Option<RuleRow>, ApiError> {
    Ok(RuleRow::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        format!(
            "SELECT {} FROM alert_rules WHERE id = $1 AND enabled FOR UPDATE",
            super::rules::RULE_COLUMNS
        ),
        vec![rule_id.into()],
    ))
    .one(tx)
    .await?)
}

/// The bot of a rule when it is still a bot account that is a member of the workspace.
async fn valid_bot<C: ConnectionTrait>(
    tx: &C,
    bot_user_id: Option<Uuid>,
    workspace_id: Uuid,
) -> Result<Option<Uuid>, ApiError> {
    let Some(bot) = bot_user_id else {
        return Ok(None);
    };
    let found = IdRow::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "SELECT u.id FROM users u JOIN workspace_members m ON m.user_id = u.id AND m.workspace_id = $2 \
          WHERE u.id = $1 AND u.entity_type = 'bot' AND u.is_active",
        vec![bot.into(), workspace_id.into()],
    ))
    .one(tx)
    .await?;
    Ok(found.map(|row| row.id))
}

async fn opened_this_hour<C: ConnectionTrait>(tx: &C, rule_id: Uuid) -> Result<i64, ApiError> {
    scalar(
        tx,
        &format!(
            "SELECT COUNT(*) AS n FROM inbound_events \
              WHERE rule_id = $1 AND outcome = 'alert_opened' AND received_at > now() - {HOUR}"
        ),
        vec![rule_id.into()],
    )
    .await
}

async fn find_open_alert<C: ConnectionTrait>(
    tx: &C,
    source: &SourceRow,
    envelope: &Envelope,
    window_secs: i32,
    rearm_secs: i32,
) -> Result<Option<AlertRow>, ApiError> {
    Ok(AlertRow::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        format!(
            "SELECT a.id, a.work_item_id, a.occurrences, a.max_severity_rank, a.task_seq, \
                    (a.last_seen_at + make_interval(secs => $3) > now()) AS fresh, \
                    COALESCE(t.status IN {ACTIVE_TASK_STATUSES}, false) AS task_active, \
                    COALESCE(t.status NOT IN {ACTIVE_TASK_STATUSES} \
                             AND COALESCE(a.task_ended_at, t.completed_at, t.updated_at) \
                                 + make_interval(secs => $4) > now(), false) AS rearm_active, \
                    w.state AS wi_state, w.title AS wi_title, w.assignee_id AS wi_assignee \
               FROM inbound_alerts a \
               JOIN work_items w ON w.id = a.work_item_id \
               LEFT JOIN ai_tasks t ON t.id = a.task_id \
              WHERE a.source_id = $1 AND a.dedup_key = $2 AND a.open \
                FOR UPDATE OF a"
        ),
        vec![
            source.id.into(),
            envelope.dedup_key.clone().into(),
            f64::from(window_secs).into(),
            f64::from(rearm_secs).into(),
        ],
    ))
    .one(tx)
    .await?)
}

async fn close_alert<C: ConnectionTrait>(
    tx: &C,
    source: &SourceRow,
    alert: &AlertRow,
    reason: &str,
) -> Result<(), ApiError> {
    tx.execute(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "UPDATE inbound_alerts SET open = false, updated_at = now() WHERE id = $1",
        vec![alert.id.into()],
    ))
    .await?;
    audit::record(
        tx,
        source.workspace_id,
        source.project_id,
        "alert.closed",
        "work_item",
        alert.work_item_id,
        None,
        audit::inbound_source_value(source.id),
        json!({ "alert_id": alert.id, "occurrences": alert.occurrences, "reason": reason }),
    )
    .await
}

/// Step 5: the event joins the open alert.
async fn attach<C: ConnectionTrait>(tx: &C, envelope: &Envelope, row: AlertRow) -> Result<AlertContext, ApiError> {
    let rank = row.max_severity_rank.max(envelope.severity.rank());
    tx.execute(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "UPDATE inbound_alerts SET occurrences = occurrences + 1, last_seen_at = now(), \
                max_severity_rank = $2, updated_at = now() WHERE id = $1",
        vec![row.id.into(), rank.into()],
    ))
    .await?;
    Ok(AlertContext {
        id: row.id,
        work_item_id: row.work_item_id,
        title: row.wi_title,
        assignee: row.wi_assignee,
        occurrences: row.occurrences + 1,
        task_seq: row.task_seq,
        task_active: row.task_active,
        rearm_active: row.rearm_active,
        opened_now: false,
    })
}

#[derive(FromQueryResult)]
struct IdentifierRow {
    identifier: String,
}

/// Step 6: a new alert work item and its alert row, with the activity, the audit event and the
/// `alert_opened` notifications.
async fn open_alert<C: ConnectionTrait>(
    tx: &C,
    request: &VerifiedRequest,
    envelope: &Envelope,
    workflow: &EffectiveWorkflow,
    rule: &RuleRow,
    bot: Option<Uuid>,
    event_id: Uuid,
) -> Result<AlertContext, ApiError> {
    let source = &request.source;
    let acting = source
        .acting_user_id
        .ok_or_else(|| ApiError::Forbidden("source has no acting user".to_owned()))?;
    let work_item_id = Uuid::new_v4();
    let alert_id = Uuid::new_v4();
    let now = Timestamp::now();
    let title = description::build_title(envelope.severity, &source.name, &envelope.kind, &envelope.subject);
    let state = workflow
        .states
        .iter()
        .find(|candidate| candidate.is_initial)
        .or_else(|| workflow.states.first())
        .map_or("todo", |initial| initial.key.as_str());

    insert_work_item(
        tx,
        &NewWorkItem {
            id: work_item_id,
            project_id: source.project_id,
            title: &title,
            description: "",
            state,
            priority: &rule.priority_for(envelope.severity),
            assignee_id: bot,
            due_at: None,
            created_by: acting,
            sprint_id: None,
            now,
        },
    )
    .await?;

    // The identifier exists only once the row does, and the instructions may name it.
    let identifier = IdentifierRow::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "SELECT p.key || '-' || w.sequence_number::text AS identifier \
           FROM work_items w JOIN projects p ON p.id = w.project_id WHERE w.id = $1",
        vec![work_item_id.into()],
    ))
    .one(tx)
    .await?
    .map_or_else(String::new, |row| row.identifier);
    let seen = now.to_rfc3339();
    let text = description::build_description(
        &AlertFacts {
            identifier: &identifier,
            severity: envelope.severity,
            kind: &envelope.kind,
            source_name: &source.name,
            rule_name: &rule.name,
            occurrences: 1,
            first_seen: &seen,
            last_seen: &seen,
        },
        &rule.instructions,
        &envelope.title,
        &envelope.summary,
        &envelope.subject,
        &envelope.payload,
    );
    tx.execute(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "UPDATE work_items SET description = $2 WHERE id = $1",
        vec![work_item_id.into(), text.into()],
    ))
    .await?;

    if !rule.label_ids.is_empty() {
        tx.execute(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "INSERT INTO work_item_labels (work_item_id, label_id, created_at) \
             SELECT $1, l.id, now() FROM labels l WHERE l.id = ANY($2) AND l.workspace_id = $3 \
             ON CONFLICT DO NOTHING",
            vec![
                work_item_id.into(),
                rule.label_ids.clone().into(),
                source.workspace_id.into(),
            ],
        ))
        .await?;
    }

    let detail = json!({ "source": source.name, "source_id": source.id, "rule": rule.name, "rule_id": rule.id });
    tx.execute(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "INSERT INTO activities (id, workspace_id, project_id, issue_id, user_id, action, detail, created_at, \
         resource_type, resource_id, event_type, actor_id, payload) \
         VALUES ($1, $2, $3, $4, $5, 'alert_opened', $6::jsonb, $7, 'issue', $4, 'alert_opened', $5, $6::jsonb)",
        vec![
            Uuid::new_v4().into(),
            source.workspace_id.into(),
            source.project_id.into(),
            work_item_id.into(),
            acting.into(),
            detail.into(),
            now.into(),
        ],
    ))
    .await?;

    tx.execute(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "INSERT INTO inbound_alerts (id, source_id, project_id, rule_id, dedup_key, work_item_id, \
         max_severity_rank, occurrences, first_seen_at, last_seen_at) \
         VALUES ($1, $2, $3, $4, $5, $6, $7, 1, now(), now())",
        vec![
            alert_id.into(),
            source.id.into(),
            source.project_id.into(),
            rule.id.into(),
            envelope.dedup_key.clone().into(),
            work_item_id.into(),
            envelope.severity.rank().into(),
        ],
    ))
    .await?;

    notify::send(
        tx,
        AlertNotification::Opened,
        &Subject {
            workspace_id: source.workspace_id,
            project_id: source.project_id,
            work_item_id: Some(work_item_id),
        },
        &rule.notify_user_ids,
        &notify::opened_content(envelope.severity, &title),
        &notify::payload(
            Some(alert_id),
            Some(envelope.severity),
            Some(&envelope.kind),
            &source.name,
            None,
        ),
    )
    .await?;
    audit::record(
        tx,
        source.workspace_id,
        source.project_id,
        "alert.opened",
        "work_item",
        work_item_id,
        Some(acting),
        audit::inbound_source_value(source.id),
        json!({
            "alert_id": alert_id,
            "inbound_event_id": event_id,
            "rule_id": rule.id,
            "severity": envelope.severity.as_str(),
            "occurrences": 1,
        }),
    )
    .await?;

    Ok(AlertContext {
        id: alert_id,
        work_item_id,
        title,
        assignee: bot,
        occurrences: 1,
        task_seq: 0,
        task_active: false,
        rearm_active: false,
        opened_now: true,
    })
}

enum TaskDecision {
    Created(Uuid),
    Skipped,
    /// The named cap refused the task.
    Suppressed(&'static str),
}

/// Step 7: whether this event starts a bot task, and starts it.
async fn decide_task<C: ConnectionTrait>(
    tx: &C,
    request: &VerifiedRequest,
    envelope: &Envelope,
    rule: &RuleRow,
    alert: &AlertContext,
    bot: Uuid,
    event_id: Uuid,
) -> Result<TaskDecision, ApiError> {
    let source = &request.source;
    if alert.task_active || alert.rearm_active {
        return Ok(TaskDecision::Skipped);
    }
    // A runner bot the rule's project has not enabled gets no task: the alert is still opened and
    // the event records why (the rule warns about this when it is saved).
    if let Some(block) = runner_bot_block(tx, source.project_id, bot).await? {
        return Ok(TaskDecision::Suppressed(match block {
            crate::services::ai_task_service::RunnerBotBlock::NotConnected => "bot_not_connected_to_project",
            crate::services::ai_task_service::RunnerBotBlock::Disabled => "connection_disabled",
        }));
    }
    let rule_id = rule.id;
    let tasks_last_hour = scalar(
        tx,
        &format!(
            "SELECT COUNT(*) AS n FROM inbound_events \
              WHERE rule_id = $1 AND task_id IS NOT NULL AND received_at > now() - {HOUR}"
        ),
        vec![rule_id.into()],
    )
    .await?;
    if tasks_last_hour >= i64::from(rule.max_tasks_per_hour) {
        return Ok(TaskDecision::Suppressed("task_cap"));
    }
    let active_for_rule = scalar(
        tx,
        &format!(
            "SELECT COUNT(*) AS n FROM inbound_alerts a JOIN ai_tasks t ON t.id = a.task_id \
              WHERE a.rule_id = $1 AND t.status IN {ACTIVE_TASK_STATUSES}"
        ),
        vec![rule_id.into()],
    )
    .await?;
    if active_for_rule >= i64::from(rule.max_active_tasks) {
        return Ok(TaskDecision::Suppressed("active_cap"));
    }
    let active_in_workspace = scalar(
        tx,
        &format!(
            "SELECT COUNT(*) AS n FROM inbound_alerts a \
               JOIN ai_tasks t ON t.id = a.task_id \
               JOIN projects p ON p.id = a.project_id \
              WHERE p.workspace_id = $1 AND t.status IN {ACTIVE_TASK_STATUSES}"
        ),
        vec![source.workspace_id.into()],
    )
    .await?;
    if active_in_workspace >= i64::from(request.config.max_active_alert_tasks_per_workspace) {
        return Ok(TaskDecision::Suppressed("workspace_cap"));
    }

    let seq = alert.task_seq + 1;
    let task = create_ai_task(
        tx,
        CreateAiTaskInput {
            origin: DispatchOrigin::InboundRule {
                rule_id,
                inbound_event_id: event_id,
                source_id: source.id,
            },
            project_id: source.project_id,
            ai_participant_id: bot,
            actor_id: source.acting_user_id,
            task_type: "issue_assigned".to_owned(),
            reference_type: Some("work_item".to_owned()),
            reference_id: Some(alert.work_item_id),
            priority: envelope.severity.task_priority(),
            payload: json!({
                "issue_id": alert.work_item_id.to_string(),
                "project_id": source.project_id.to_string(),
                "assignee_id": bot.to_string(),
                "trigger": "alert_rule",
                "title": alert.title,
                "alert": {
                    "alert_id": alert.id.to_string(),
                    "rule_id": rule_id.to_string(),
                    "source_id": source.id.to_string(),
                    "inbound_event_id": event_id.to_string(),
                    "severity": envelope.severity.as_str(),
                    "kind": envelope.kind,
                    "occurrences": alert.occurrences,
                },
            }),
            idempotency_key: Some(format!("alert:{}:{seq}", alert.id)),
            max_attempts: rule.task_max_attempts,
        },
    )
    .await?;
    let Some(task) = task else {
        return Ok(TaskDecision::Skipped);
    };
    tx.execute(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "UPDATE inbound_alerts SET task_id = $2, task_seq = $3, task_state = 'created', task_ended_at = NULL, \
                updated_at = now() WHERE id = $1",
        vec![alert.id.into(), task.id.into(), seq.into()],
    ))
    .await?;
    if alert.assignee != Some(bot) {
        // Set directly: `queue_assignment_ai_task` would create a second task for the same work.
        tx.execute(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "UPDATE work_items SET assignee_id = $2, updated_at = now() WHERE id = $1",
            vec![alert.work_item_id.into(), bot.into()],
        ))
        .await?;
    }
    audit::record(
        tx,
        source.workspace_id,
        source.project_id,
        "alert_rule.fired",
        "alert_rule",
        rule_id,
        source.acting_user_id,
        audit::inbound_source_value(source.id),
        json!({ "inbound_event_id": event_id, "alert_id": alert.id, "task_id": task.id }),
    )
    .await?;
    Ok(TaskDecision::Created(task.id))
}

/// Records a suppression and tells the rule's recipients, at most once per rule per hour.
async fn note_suppression<C: ConnectionTrait>(
    tx: &C,
    rule: &RuleRow,
    source: &SourceRow,
    cause: &str,
    work_item_id: Option<Uuid>,
) -> Result<(), ApiError> {
    if cause != "bot_unavailable" {
        audit::record(
            tx,
            source.workspace_id,
            source.project_id,
            "alert_rule.suppressed",
            "alert_rule",
            rule.id,
            source.acting_user_id,
            audit::inbound_source_value(source.id),
            json!({ "cap": cause, "work_item_id": work_item_id }),
        )
        .await?;
    }
    let claimed = IdRow::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        format!(
            "UPDATE alert_rules SET last_suppression_notice_at = now() \
              WHERE id = $1 AND (last_suppression_notice_at IS NULL OR last_suppression_notice_at < now() - {HOUR}) \
              RETURNING id"
        ),
        vec![rule.id.into()],
    ))
    .one(tx)
    .await?;
    if claimed.is_none() {
        return Ok(());
    }
    notify::send(
        tx,
        AlertNotification::Suppressed,
        &Subject {
            workspace_id: source.workspace_id,
            project_id: source.project_id,
            work_item_id,
        },
        &rule.notify_user_ids,
        &format!("alert_suppressed:{}", rule.name),
        &json!({ "rule_id": rule.id, "cause": cause, "source_name": source.name }),
    )
    .await?;
    Ok(())
}
