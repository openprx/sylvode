//! The rule test: what would happen to an event, without doing any of it.
//!
//! The same readers the ingest uses decide the answer (rule matching, the open alert, the caps,
//! the queue), but nothing is written: no event, alert, work item, task, notification or audit
//! row, no rate counter and no lock. Concurrent traffic can change the answer between the test and
//! a real event, so it is a forecast, not a promise.

use sea_orm::{ConnectionTrait, DbBackend, FromQueryResult, Statement};
use serde::Serialize;
use serde_json::{Map, Value};
use uuid::Uuid;

use super::{
    description::build_title,
    ingest::{find_open_alert, is_terminal, opened_this_hour, waiting_for_rule},
    notify::current_human_members,
    rules::{ACTION_WORK_ITEM_AND_TASK, RULE_COLUMNS, RuleRow, mismatch_reason},
    severity::Severity,
    tasks::{CapState, cap_state, valid_bot},
};
use crate::{error::ApiError, services::workflow_service::EffectiveWorkflow};

/// What the test needs to know about an event.
pub struct EventFacts {
    pub kind: String,
    pub severity: Severity,
    pub dedup_key: String,
    pub subject: Map<String, Value>,
}

/// The source the sample event is posted as.
pub struct SourceFacts<'a> {
    pub id: Uuid,
    pub workspace_id: Uuid,
    pub project_id: Uuid,
    pub name: &'a str,
}

/// The limits the verdict is measured against.
#[derive(Clone, Copy)]
pub struct Limits {
    pub workspace_active_tasks: u32,
    pub alerts_per_rule_per_hour: u32,
}

#[derive(Serialize)]
pub struct RuleVerdict {
    pub id: Uuid,
    pub name: String,
    pub position: i32,
    pub matched: bool,
    /// `matched`, or why the rule does not handle the event.
    pub reason: &'static str,
}

#[derive(Serialize)]
pub struct CapView {
    #[serde(flatten)]
    pub state: CapState,
    pub max_tasks_per_hour: i32,
    pub max_active_tasks: i32,
    pub max_active_tasks_per_workspace: u32,
    pub alerts_opened_last_hour: i64,
    pub max_alerts_per_rule_per_hour: u32,
}

#[derive(Serialize)]
pub struct DryRunReport {
    pub matched: bool,
    pub rule: Option<RuleVerdict>,
    pub rules_evaluated: Vec<RuleVerdict>,
    /// `no_rule`, `would_open`, `would_attach` or `would_refuse_alert_cap`.
    pub alert: &'static str,
    /// `none`, `bot_unavailable`, `would_start`, `would_queue`, `already_active`, `already_waiting`
    /// or `rearm_delay`.
    pub task: &'static str,
    /// The cap that would queue the task.
    pub queue_cause: Option<&'static str>,
    /// An attach that raises the alert's highest severity to `high` or above notifies the rule's
    /// recipients.
    pub would_escalate: bool,
    pub caps: Option<CapView>,
    /// Current human members of the workspace among the rule's recipients.
    pub notify_recipients: usize,
    /// The title a new alert work item would get.
    pub title: String,
}

pub async fn evaluate<C: ConnectionTrait>(
    db: &C,
    workflow: &EffectiveWorkflow,
    source: &SourceFacts<'_>,
    event: &EventFacts,
    limits: Limits,
) -> Result<DryRunReport, ApiError> {
    let rules = RuleRow::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        format!("SELECT {RULE_COLUMNS} FROM alert_rules WHERE project_id = $1 ORDER BY position, id"),
        vec![source.project_id.into()],
    ))
    .all(db)
    .await?;
    let verdicts: Vec<RuleVerdict> = rules
        .iter()
        .map(|rule| {
            let reason = mismatch_reason(rule, source.id, &event.kind, event.severity);
            RuleVerdict {
                id: rule.id,
                name: rule.name.clone(),
                position: rule.position,
                matched: reason.is_none(),
                reason: reason.unwrap_or("matched"),
            }
        })
        .collect();
    let title = build_title(event.severity, source.name, &event.kind, &event.subject);
    let Some(rule) = rules
        .iter()
        .zip(&verdicts)
        .find_map(|(rule, verdict)| verdict.matched.then_some(rule))
    else {
        return Ok(DryRunReport {
            matched: false,
            rule: None,
            rules_evaluated: verdicts,
            alert: "no_rule",
            task: "none",
            queue_cause: None,
            would_escalate: false,
            caps: None,
            notify_recipients: 0,
            title,
        });
    };
    let alert_state = alert_forecast(db, workflow, source, event, rule).await?;

    let bot = if rule.action == ACTION_WORK_ITEM_AND_TASK {
        valid_bot(db, rule.bot_user_id, source.workspace_id).await?
    } else {
        None
    };
    let alerts_opened = opened_this_hour(db, rule.id).await?;
    let caps = cap_state(db, rule.id, source.workspace_id).await?;
    let refusal = caps.refusal(rule, limits.workspace_active_tasks);

    let mut alert = alert_state.verdict;
    if alert == "would_open" && alerts_opened >= i64::from(limits.alerts_per_rule_per_hour) {
        alert = "would_refuse_alert_cap";
    }
    let mut queue_cause = None;
    let task = if alert == "would_refuse_alert_cap" || rule.action != ACTION_WORK_ITEM_AND_TASK {
        "none"
    } else if bot.is_none() {
        "bot_unavailable"
    } else if let Some(gate) = alert_state.task_gate {
        gate
    } else if let Some(cap) = refusal {
        queue_cause = Some(cap.as_str());
        "would_queue"
    } else if waiting_for_rule(db, rule.id).await? {
        queue_cause = Some("queue_ahead");
        "would_queue"
    } else {
        "would_start"
    };
    let recipients = current_human_members(db, source.workspace_id, &rule.notify_user_ids).await?;
    Ok(DryRunReport {
        matched: true,
        rule: Some(RuleVerdict {
            id: rule.id,
            name: rule.name.clone(),
            position: rule.position,
            matched: true,
            reason: "matched",
        }),
        rules_evaluated: verdicts,
        alert,
        task,
        queue_cause,
        would_escalate: alert_state.escalates,
        caps: Some(CapView {
            state: caps,
            max_tasks_per_hour: rule.max_tasks_per_hour,
            max_active_tasks: rule.max_active_tasks,
            max_active_tasks_per_workspace: limits.workspace_active_tasks,
            alerts_opened_last_hour: alerts_opened,
            max_alerts_per_rule_per_hour: limits.alerts_per_rule_per_hour,
        }),
        notify_recipients: recipients.len(),
        title,
    })
}

struct AlertForecast {
    verdict: &'static str,
    /// Why an alert that exists starts no task: `already_active`, `already_waiting` or
    /// `rearm_delay`.
    task_gate: Option<&'static str>,
    escalates: bool,
}

/// Whether the event would open or join an alert, read the way the ingest reads it but unlocked.
async fn alert_forecast<C: ConnectionTrait>(
    db: &C,
    workflow: &EffectiveWorkflow,
    source: &SourceFacts<'_>,
    event: &EventFacts,
    rule: &RuleRow,
) -> Result<AlertForecast, ApiError> {
    let open = find_open_alert(
        db,
        source.id,
        &event.dedup_key,
        rule.dedup_window_secs,
        rule.rearm_after_secs,
        false,
    )
    .await?;
    Ok(match open {
        Some(alert) if alert.fresh && !is_terminal(workflow, &alert.wi_state) => AlertForecast {
            verdict: "would_attach",
            task_gate: if alert.task_active {
                Some("already_active")
            } else if alert.task_state == "waiting" {
                Some("already_waiting")
            } else if alert.rearm_active {
                Some("rearm_delay")
            } else {
                None
            },
            escalates: event.severity >= Severity::High && event.severity.rank() > alert.max_severity_rank,
        },
        _ => AlertForecast {
            verdict: "would_open",
            task_gate: None,
            escalates: false,
        },
    })
}

/// The facts of a stored event of `project_id`, for a test by event id. `None` when no such event
/// exists in the project.
pub async fn stored_event<C: ConnectionTrait>(
    db: &C,
    project_id: Uuid,
    source_id: Uuid,
    event_id: Uuid,
) -> Result<Option<EventFacts>, ApiError> {
    #[derive(FromQueryResult)]
    struct Row {
        kind: String,
        severity_rank: i16,
        dedup_key: String,
        subject: Value,
    }
    let row = Row::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "SELECT kind, severity_rank, dedup_key, subject FROM inbound_events \
          WHERE id = $1 AND project_id = $2 AND source_id = $3",
        vec![event_id.into(), project_id.into(), source_id.into()],
    ))
    .one(db)
    .await?;
    Ok(row.and_then(|row| {
        Some(EventFacts {
            kind: row.kind,
            severity: Severity::from_rank(row.severity_rank)?,
            dedup_key: row.dedup_key,
            subject: match row.subject {
                Value::Object(map) => map,
                _ => Map::new(),
            },
        })
    }))
}
