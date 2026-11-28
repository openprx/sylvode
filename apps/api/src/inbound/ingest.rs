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
//!    (`DispatchOrigin::InboundRule`) unless the alert already has one. A task refused by a cap is
//!    not dropped: the alert is marked `waiting` and the worker starts the task when capacity frees
//!    up, or suppresses it after an hour (see `queue`).
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
    severity::Severity,
    tasks::{
        ACTIVE_TASK_STATUSES, AlertScope, Cap, HOUR, TaskStart, cap_state, note_suppression, scalar, start_task,
        valid_bot,
    },
};
use crate::{
    error::ApiError,
    services::{
        ai_task_service::{bot_cannot_read_project, runner_bot_block},
        issues::write_activity,
        work_item_service::{NewWorkItem, insert_work_item},
        workflow_service::{EffectiveWorkflow, resolve_effective_workflow_for_project},
    },
    webhook_trigger::{TriggerContext, WebhookEvent, trigger_webhooks_after_ai_tasks},
};

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

/// An open alert with the facts the decisions need.
#[derive(FromQueryResult)]
pub(super) struct AlertRow {
    pub(super) id: Uuid,
    pub(super) work_item_id: Uuid,
    pub(super) occurrences: i32,
    pub(super) max_severity_rank: i16,
    pub(super) task_seq: i32,
    pub(super) fresh: bool,
    pub(super) task_active: bool,
    /// The alert's `task_state`: `none`, `created`, `waiting` or `suppressed`.
    pub(super) task_state: String,
    pub(super) rearm_active: bool,
    pub(super) wi_state: String,
    pub(super) wi_title: String,
    pub(super) wi_assignee: Option<Uuid>,
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
    task_state: String,
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
    let inserted = db
        .execute(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "INSERT INTO inbound_events (id, source_id, workspace_id, project_id, event_id, body_sha256, kind, \
         severity_rank, dedup_key, occurred_at, title, summary, subject, payload, signed, client_ip, outcome) \
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15, $16::inet, 'no_rule') \
         ON CONFLICT (source_id, event_id) DO NOTHING",
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
        .await?;
    Ok(inserted.rows_affected() == 1)
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

    let scope = scope_of(source);
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

    let reusable = match find_open_alert(
        tx,
        source.id,
        &envelope.dedup_key,
        rule.dedup_window_secs,
        rule.rearm_after_secs,
        true,
    )
    .await?
    {
        Some(alert) if alert.fresh && !is_terminal(workflow, &alert.wi_state) => Some(alert),
        Some(alert) => {
            close_alert(tx, source, &alert, "window_or_terminal_state").await?;
            None
        }
        None => None,
    };

    let alert = if let Some(row) = reusable {
        attach(tx, envelope, &rule, scope, row).await?
    } else {
        if opened_this_hour(tx, rule.id).await? >= i64::from(request.config.max_alerts_per_rule_per_hour) {
            note_suppression(tx, &rule, scope, "alert_cap", None).await?;
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
        note_suppression(tx, &rule, scope, "bot_unavailable", Some(alert.work_item_id)).await?;
    }

    if let Some(bot) = bot {
        match decide_task(tx, request, envelope, &rule, &alert, bot, event_id).await? {
            TaskDecision::Created(task_id) => decision.task_id = Some(task_id),
            TaskDecision::Skipped => {}
            TaskDecision::Queued(cap) => {
                decision.outcome = "task_queued";
                audit::record(
                    tx,
                    source.workspace_id,
                    source.project_id,
                    "alert_rule.queued",
                    "alert_rule",
                    rule.id,
                    source.acting_user_id,
                    audit::inbound_source_value(source.id),
                    json!({ "cap": cap.as_str(), "work_item_id": alert.work_item_id, "alert_id": alert.id }),
                )
                .await?;
            }
            TaskDecision::BotUnavailable => {
                // The runner bot has no enabled connection in this project: like a bot that left,
                // the alert opens and no task starts, and no later task will either.
                decision.outcome = "bot_unavailable";
                note_suppression(tx, &rule, scope, "bot_unavailable", Some(alert.work_item_id)).await?;
            }
        }
    }
    Ok(decision)
}

/// The source's identity for tasks, audit events and notices.
fn scope_of(source: &SourceRow) -> AlertScope<'_> {
    AlertScope {
        workspace_id: source.workspace_id,
        project_id: source.project_id,
        source_id: source.id,
        source_name: &source.name,
        acting_user_id: source.acting_user_id,
    }
}

pub(super) fn is_terminal(workflow: &EffectiveWorkflow, state: &str) -> bool {
    workflow
        .states
        .iter()
        .any(|candidate| candidate.key == state && candidate.is_terminal)
}

pub(super) async fn lock_rule<C: ConnectionTrait>(tx: &C, rule_id: Uuid) -> Result<Option<RuleRow>, ApiError> {
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

/// Alerts the rule opened in the last hour, from the alert rows themselves so an alert whose task
/// is queued or suppressed counts like any other.
pub(super) async fn opened_this_hour<C: ConnectionTrait>(tx: &C, rule_id: Uuid) -> Result<i64, ApiError> {
    scalar(
        tx,
        &format!("SELECT COUNT(*) AS n FROM inbound_alerts WHERE rule_id = $1 AND created_at > now() - {HOUR}"),
        vec![rule_id.into()],
    )
    .await
}

/// The open alert of `(source_id, dedup_key)`. `lock` takes the row lock the ingest transaction
/// needs; the rule test reads without it.
pub(super) async fn find_open_alert<C: ConnectionTrait>(
    tx: &C,
    source_id: Uuid,
    dedup_key: &str,
    window_secs: i32,
    rearm_secs: i32,
    lock: bool,
) -> Result<Option<AlertRow>, ApiError> {
    Ok(AlertRow::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        format!(
            "SELECT a.id, a.work_item_id, a.occurrences, a.max_severity_rank, a.task_seq, \
                    (a.last_seen_at + make_interval(secs => $3) > now()) AS fresh, \
                    COALESCE(t.status IN {ACTIVE_TASK_STATUSES}, false) AS task_active, \
                    a.task_state, \
                    COALESCE(t.status NOT IN {ACTIVE_TASK_STATUSES} \
                             AND COALESCE(a.task_ended_at, t.completed_at, t.updated_at) \
                                 + make_interval(secs => $4) > now(), false) AS rearm_active, \
                    w.state AS wi_state, w.title AS wi_title, w.assignee_id AS wi_assignee \
               FROM inbound_alerts a \
               JOIN work_items w ON w.id = a.work_item_id \
               LEFT JOIN ai_tasks t ON t.id = a.task_id \
              WHERE a.source_id = $1 AND a.dedup_key = $2 AND a.open{}",
            if lock { " FOR UPDATE OF a" } else { "" }
        ),
        vec![
            source_id.into(),
            dedup_key.to_owned().into(),
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

/// Step 5: the event joins the open alert. When it raises the alert's highest severity to `high`
/// or above, the rule's recipients are told (once per rise: a second event of the same severity
/// raises nothing).
async fn attach<C: ConnectionTrait>(
    tx: &C,
    envelope: &Envelope,
    rule: &RuleRow,
    scope: AlertScope<'_>,
    row: AlertRow,
) -> Result<AlertContext, ApiError> {
    let rank = row.max_severity_rank.max(envelope.severity.rank());
    tx.execute(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "UPDATE inbound_alerts SET occurrences = occurrences + 1, last_seen_at = now(), \
                max_severity_rank = $2, updated_at = now() WHERE id = $1",
        vec![row.id.into(), rank.into()],
    ))
    .await?;
    if rank > row.max_severity_rank && envelope.severity >= Severity::High {
        notify::send(
            tx,
            AlertNotification::Escalated,
            &Subject {
                workspace_id: scope.workspace_id,
                project_id: scope.project_id,
                work_item_id: Some(row.work_item_id),
            },
            &rule.notify_user_ids,
            &notify::escalated_content(envelope.severity, &row.wi_title),
            &notify::payload(
                Some(row.id),
                Some(envelope.severity),
                Some(&envelope.kind),
                scope.source_name,
                None,
            ),
        )
        .await?;
        audit::record(
            tx,
            scope.workspace_id,
            scope.project_id,
            "alert.escalated",
            "work_item",
            row.work_item_id,
            None,
            audit::inbound_source_value(scope.source_id),
            json!({
                "alert_id": row.id,
                "from": Severity::from_rank(row.max_severity_rank).map(Severity::as_str),
                "to": envelope.severity.as_str(),
            }),
        )
        .await?;
    }
    Ok(AlertContext {
        id: row.id,
        work_item_id: row.work_item_id,
        title: row.wi_title,
        assignee: row.wi_assignee,
        occurrences: row.occurrences + 1,
        task_seq: row.task_seq,
        task_active: row.task_active,
        task_state: row.task_state,
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
    write_activity(tx, work_item_id, acting, "alert_opened", &detail, now).await?;

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
        task_state: "none".to_owned(),
        rearm_active: false,
        opened_now: true,
    })
}

enum TaskDecision {
    Created(Uuid),
    Skipped,
    /// A cap refused the task now; the alert waits for capacity.
    Queued(Cap),
    /// The runner bot is not connected to the project or its connection is disabled.
    BotUnavailable,
}

/// Step 7: whether this event starts a bot task, queues it behind a cap, or does nothing.
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
    if alert.task_active || alert.rearm_active || alert.task_state == "waiting" {
        return Ok(TaskDecision::Skipped);
    }
    // A runner bot the rule's project has not enabled gets no task, now or later: the alert is
    // still opened and the event records why (the rule warns about this when it is saved). The
    // central guard in `create_ai_task` refuses it too; this settles the outcome before a cap
    // can park the alert in the queue.
    if runner_bot_block(tx, source.project_id, bot).await?.is_some()
        || bot_cannot_read_project(tx, source.project_id, bot).await?
    {
        return Ok(TaskDecision::BotUnavailable);
    }
    let caps = cap_state(tx, rule.id, source.workspace_id).await?;
    let refused = caps.refusal(rule, request.config.max_active_alert_tasks_per_workspace);
    // Alerts already waiting for this rule were queued first and are served first: a newcomer does
    // not jump ahead of them in the moment between a slot freeing and the worker's next tick.
    let queued = match refused {
        Some(cap) => Some(cap),
        None if waiting_for_rule(tx, rule.id).await? => Some(Cap::Active),
        None => None,
    };
    if let Some(cap) = queued {
        enqueue(tx, alert.id, event_id).await?;
        return Ok(TaskDecision::Queued(cap));
    }
    let started = start_task(
        tx,
        &TaskStart {
            scope: scope_of(source),
            rule,
            bot,
            alert_id: alert.id,
            work_item_id: alert.work_item_id,
            title: &alert.title,
            current_assignee: alert.assignee,
            occurrences: alert.occurrences,
            task_seq: alert.task_seq,
            severity: envelope.severity,
            kind: &envelope.kind,
            event_id,
        },
    )
    .await?;
    Ok(started.map_or(TaskDecision::Skipped, TaskDecision::Created))
}

pub(super) async fn waiting_for_rule<C: ConnectionTrait>(tx: &C, rule_id: Uuid) -> Result<bool, ApiError> {
    Ok(scalar(
        tx,
        "SELECT COUNT(*) AS n FROM inbound_alerts WHERE rule_id = $1 AND task_state = 'waiting'",
        vec![rule_id.into()],
    )
    .await?
        > 0)
}

/// Marks the alert as waiting for a task slot. The queue lives in this row, so a restart of any
/// process loses nothing.
async fn enqueue<C: ConnectionTrait>(tx: &C, alert_id: Uuid, event_id: Uuid) -> Result<(), ApiError> {
    tx.execute(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "UPDATE inbound_alerts SET task_state = 'waiting', task_queued_at = now(), queued_event_id = $2, \
                updated_at = now() WHERE id = $1",
        vec![alert_id.into(), event_id.into()],
    ))
    .await?;
    Ok(())
}
