//! The burst queue of alert tasks.
//!
//! A task that a cap refuses (`max_tasks_per_hour` or `max_active_tasks` of the rule, or the
//! workspace limit) is not dropped. The ingest transaction marks the alert `waiting` and stamps
//! `task_queued_at`; this module, called by the worker's follow-up tick, starts waiting tasks as
//! capacity frees up and suppresses the ones that waited for an hour.
//!
//! * **Persistence.** The queue is the set of `inbound_alerts` rows with `task_state = 'waiting'`.
//!   Nothing is held in memory, so a restart of the API or the worker loses nothing, and two
//!   workers can run the tick together (`FOR UPDATE SKIP LOCKED` on the alert, a row lock on the
//!   rule so the cap checks of two starts of one rule are serialised).
//! * **Order.** Oldest first, fair across rules: the candidates are ranked by their position inside
//!   their own rule's line (`ROW_NUMBER() OVER (PARTITION BY rule ...)`) and then by age, so one
//!   rule with a long line cannot starve another rule's single waiting alert.
//! * **Re-check at start.** A task is created through the same `start_task` the ingest uses, which
//!   calls the central dispatch guard (`create_ai_task`) again. Before that, the rule, its bot, the
//!   source and the work item are read again: a rule that was disabled, a bot that left, or a work
//!   item that was closed settles the entry instead of starting a task nobody wants. So does a
//!   runner bot the project has not enabled (`bot_unavailable`, with the suppression notice). Any
//!   other refusal by the guard leaves the entry waiting (it may be temporary) until the hour is
//!   over.
//! * **Expiry.** After [`QUEUE_TTL`] the entry becomes `suppressed` and the rule's recipients
//!   get the existing `alert_suppressed` notice (at most one per rule per hour). The clock is a
//!   parameter, so tests move time without waiting.

use std::collections::HashSet;

use platform::timestamp::Timestamp;
use sea_orm::{ConnectionTrait, DatabaseConnection, DbBackend, FromQueryResult, Statement, TransactionTrait};
use uuid::Uuid;

use super::{
    rules::{RULE_COLUMNS, RuleRow},
    severity::Severity,
    tasks::{AlertScope, Cap, TaskStart, cap_state, note_suppression, start_task, valid_bot},
};
use crate::{error::ApiError, services::ai_task_service::runner_bot_block};

/// How long a task waits for capacity before it is suppressed, as a duration and in the seconds
/// the SQL takes.
pub const QUEUE_TTL: time::Duration = time::Duration::hours(1);
const QUEUE_TTL_SECS: f64 = 3_600.0;
/// Entries examined per step of one tick; a longer line drains over the next ticks.
const BATCH: i64 = 100;

/// What one drain did.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct QueueReport {
    /// Tasks created from the queue.
    pub started: usize,
    /// Entries suppressed after waiting the full hour.
    pub expired: usize,
    /// Entries settled without a task because the rule, the bot or the work item no longer allows
    /// one.
    pub dropped: usize,
}

#[derive(FromQueryResult)]
struct Candidate {
    id: Uuid,
    rule_id: Option<Uuid>,
    workspace_id: Uuid,
}

#[derive(FromQueryResult)]
struct Expired {
    id: Uuid,
}

/// A waiting alert with everything a start needs, read under the alert's row lock.
#[derive(FromQueryResult)]
struct Waiting {
    id: Uuid,
    rule_id: Option<Uuid>,
    source_id: Uuid,
    project_id: Uuid,
    workspace_id: Uuid,
    work_item_id: Uuid,
    task_state: String,
    expired: bool,
    max_severity_rank: i16,
    occurrences: i32,
    task_seq: i32,
    event_ref: Option<Uuid>,
    event_kind: Option<String>,
    wi_title: String,
    wi_assignee: Option<Uuid>,
    wi_terminal: bool,
    source_name: String,
    source_enabled: bool,
    acting_user_id: Option<Uuid>,
    acting_role: Option<String>,
}

impl Waiting {
    fn scope(&self) -> AlertScope<'_> {
        AlertScope {
            workspace_id: self.workspace_id,
            project_id: self.project_id,
            source_id: self.source_id,
            source_name: &self.source_name,
            acting_user_id: self.acting_user_id,
        }
    }

    /// Whether the person the source acts for may still stand behind it (D7), as the ingest checks.
    fn source_is_live(&self) -> bool {
        self.source_enabled
            && self.acting_user_id.is_some()
            && matches!(self.acting_role.as_deref(), Some("owner" | "admin"))
    }
}

enum Step {
    Started,
    /// Not started and not settled; the entry waits on.
    Held {
        blocked_rule: Option<Uuid>,
        blocked_workspace: Option<Uuid>,
    },
    Dropped,
    /// Gone or settled by another worker between the candidate list and the lock.
    Skipped,
}

/// One drain at `now`: expire the entries that waited an hour, then start waiting tasks in fair
/// order while capacity lasts.
pub async fn drain(db: &DatabaseConnection, workspace_limit: u32, now: Timestamp) -> Result<QueueReport, ApiError> {
    let mut report = QueueReport::default();

    let expired = Expired::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "SELECT id FROM inbound_alerts \
          WHERE task_state = 'waiting' AND task_queued_at + make_interval(secs => $1) <= $2 \
          ORDER BY task_queued_at, id LIMIT $3",
        vec![QUEUE_TTL_SECS.into(), now.into(), BATCH.into()],
    ))
    .all(db)
    .await?;
    for entry in expired {
        if expire_one(db, entry.id, now).await? {
            report.expired += 1;
        }
    }

    // Rank inside the rule's own line first, then by age: the first of every rule goes before the
    // second of any rule.
    let candidates = Candidate::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "SELECT q.id, q.rule_id, q.workspace_id FROM ( \
           SELECT a.id, a.rule_id, a.task_queued_at, p.workspace_id, \
                  ROW_NUMBER() OVER (PARTITION BY COALESCE(a.rule_id, a.id) ORDER BY a.task_queued_at, a.id) AS place \
             FROM inbound_alerts a JOIN projects p ON p.id = a.project_id \
            WHERE a.task_state = 'waiting' AND a.task_queued_at + make_interval(secs => $1) > $2) q \
          ORDER BY q.place, q.task_queued_at, q.id LIMIT $3",
        vec![QUEUE_TTL_SECS.into(), now.into(), BATCH.into()],
    ))
    .all(db)
    .await?;

    // A rule or a workspace that refused once in this tick refuses again: usage only grows while
    // the tick runs, so the rest of its line is not examined.
    let mut blocked_rules: HashSet<Uuid> = HashSet::new();
    let mut blocked_workspaces: HashSet<Uuid> = HashSet::new();
    for candidate in candidates {
        if candidate.rule_id.is_some_and(|rule| blocked_rules.contains(&rule))
            || blocked_workspaces.contains(&candidate.workspace_id)
        {
            continue;
        }
        match try_start(db, candidate.id, workspace_limit, now).await? {
            Step::Started => report.started += 1,
            Step::Dropped => report.dropped += 1,
            Step::Held {
                blocked_rule,
                blocked_workspace,
            } => {
                blocked_rules.extend(blocked_rule);
                blocked_workspaces.extend(blocked_workspace);
            }
            Step::Skipped => {}
        }
    }
    Ok(report)
}

async fn lock_waiting<C: ConnectionTrait>(tx: &C, id: Uuid, now: Timestamp) -> Result<Option<Waiting>, ApiError> {
    Ok(Waiting::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "SELECT a.id, a.rule_id, a.source_id, a.project_id, p.workspace_id, a.work_item_id, a.task_state, \
                COALESCE(a.task_queued_at + make_interval(secs => $2) <= $3, false) AS expired, \
                a.max_severity_rank, a.occurrences, a.task_seq, \
                COALESCE(a.queued_event_id, \
                         (SELECT e.id FROM inbound_events e WHERE e.alert_id = a.id ORDER BY e.received_at LIMIT 1)) \
                    AS event_ref, \
                COALESCE((SELECT e.kind FROM inbound_events e WHERE e.id = a.queued_event_id), \
                         (SELECT e.kind FROM inbound_events e WHERE e.alert_id = a.id ORDER BY e.received_at LIMIT 1)) \
                    AS event_kind, \
                w.title AS wi_title, w.assignee_id AS wi_assignee, \
                EXISTS (SELECT 1 FROM workflow_states st \
                         WHERE st.workflow_id = COALESCE(p.workflow_id, ws.workflow_id, \
                               (SELECT id FROM workflows WHERE is_system_default ORDER BY created_at LIMIT 1)) \
                           AND st.key = w.state AND st.is_terminal) AS wi_terminal, \
                s.name AS source_name, s.enabled AS source_enabled, s.acting_user_id, \
                (SELECT m.role FROM workspace_members m JOIN users u ON u.id = m.user_id AND u.is_active \
                  WHERE m.workspace_id = p.workspace_id AND m.user_id = s.acting_user_id) AS acting_role \
           FROM inbound_alerts a \
           JOIN projects p ON p.id = a.project_id \
           JOIN workspaces ws ON ws.id = p.workspace_id \
           JOIN work_items w ON w.id = a.work_item_id \
           JOIN inbound_sources s ON s.id = a.source_id \
          WHERE a.id = $1 FOR UPDATE OF a SKIP LOCKED",
        vec![id.into(), QUEUE_TTL_SECS.into(), now.into()],
    ))
    .one(tx)
    .await?)
}

async fn load_rule<C: ConnectionTrait>(tx: &C, rule_id: Uuid, lock: bool) -> Result<Option<RuleRow>, ApiError> {
    Ok(RuleRow::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        format!(
            "SELECT {RULE_COLUMNS} FROM alert_rules WHERE id = $1{}",
            if lock { " FOR UPDATE" } else { "" }
        ),
        vec![rule_id.into()],
    ))
    .one(tx)
    .await?)
}

/// Marks a waiting alert's task as suppressed, records why and, when the rule still exists, tells
/// its recipients (at most once per rule per hour).
async fn suppress<C: ConnectionTrait>(
    tx: &C,
    waiting: &Waiting,
    rule: Option<&RuleRow>,
    cause: &str,
    notice: bool,
) -> Result<(), ApiError> {
    tx.execute(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "UPDATE inbound_alerts SET task_state = 'suppressed', queued_event_id = NULL, updated_at = now() \
          WHERE id = $1 AND task_state = 'waiting'",
        vec![waiting.id.into()],
    ))
    .await?;
    if let Some(rule) = rule {
        if notice {
            note_suppression(tx, rule, waiting.scope(), cause, Some(waiting.work_item_id)).await?;
        } else {
            super::audit::record(
                tx,
                waiting.workspace_id,
                waiting.project_id,
                "alert_rule.suppressed",
                "alert_rule",
                rule.id,
                waiting.acting_user_id,
                super::audit::inbound_source_value(waiting.source_id),
                serde_json::json!({ "cap": cause, "work_item_id": waiting.work_item_id }),
            )
            .await?;
        }
    }
    Ok(())
}

/// Suppresses one entry that waited the full hour. `false` when it is no longer waiting or expired
/// or another worker holds it.
async fn expire_one(db: &DatabaseConnection, id: Uuid, now: Timestamp) -> Result<bool, ApiError> {
    let tx = db.begin().await?;
    let Some(waiting) = lock_waiting(&tx, id, now).await? else {
        return Ok(false);
    };
    if waiting.task_state != "waiting" || !waiting.expired {
        return Ok(false);
    }
    let rule = match waiting.rule_id {
        Some(rule_id) => load_rule(&tx, rule_id, false).await?,
        None => None,
    };
    suppress(&tx, &waiting, rule.as_ref(), "queue_timeout", true).await?;
    tx.commit().await?;
    Ok(true)
}

/// Starts the waiting task of one alert when everything still allows it.
async fn try_start(db: &DatabaseConnection, id: Uuid, workspace_limit: u32, now: Timestamp) -> Result<Step, ApiError> {
    let tx = db.begin().await?;
    let Some(waiting) = lock_waiting(&tx, id, now).await? else {
        return Ok(Step::Skipped);
    };
    if waiting.task_state != "waiting" || waiting.expired {
        return Ok(Step::Skipped);
    }
    // The rule is locked like the ingest locks it, so two starts of one rule see each other's task.
    let rule = match waiting.rule_id {
        Some(rule_id) => load_rule(&tx, rule_id, true).await?,
        None => None,
    };
    if waiting.wi_terminal {
        suppress(&tx, &waiting, rule.as_ref(), "work_item_closed", false).await?;
        tx.commit().await?;
        return Ok(Step::Dropped);
    }
    let Some(rule) = rule.filter(|rule| rule.enabled && rule.wants_task()) else {
        let found = match waiting.rule_id {
            Some(rule_id) => load_rule(&tx, rule_id, false).await?,
            None => None,
        };
        suppress(&tx, &waiting, found.as_ref(), "rule_changed", false).await?;
        tx.commit().await?;
        return Ok(Step::Dropped);
    };
    let Some(bot) = valid_bot(&tx, rule.bot_user_id, waiting.workspace_id).await? else {
        suppress(&tx, &waiting, Some(&rule), "bot_unavailable", true).await?;
        tx.commit().await?;
        return Ok(Step::Dropped);
    };
    // A runner bot the project does not (or no longer) has enabled is a settled outcome like a bot
    // that left: waiting an hour would not change it. The guard in `create_ai_task` checks the same.
    if runner_bot_block(&tx, waiting.project_id, bot).await?.is_some() {
        suppress(&tx, &waiting, Some(&rule), "bot_unavailable", true).await?;
        tx.commit().await?;
        return Ok(Step::Dropped);
    }
    if !waiting.source_is_live() {
        // Paused or suspended meanwhile: the entry waits, and the hour decides.
        return Ok(Step::Held {
            blocked_rule: None,
            blocked_workspace: None,
        });
    }
    let caps = cap_state(&tx, rule.id, waiting.workspace_id).await?;
    if let Some(cap) = caps.refusal(&rule, workspace_limit) {
        return Ok(Step::Held {
            blocked_rule: matches!(cap, Cap::Task | Cap::Active).then_some(rule.id),
            blocked_workspace: matches!(cap, Cap::Workspace).then_some(waiting.workspace_id),
        });
    }
    let (Some(event_id), Some(kind)) = (waiting.event_ref, waiting.event_kind.as_deref()) else {
        tracing::warn!(alert_id = %waiting.id, "queued alert has no event to start a task for");
        return Ok(Step::Held {
            blocked_rule: None,
            blocked_workspace: None,
        });
    };
    let severity = Severity::from_rank(waiting.max_severity_rank).unwrap_or(Severity::High);
    let started = start_task(
        &tx,
        &TaskStart {
            scope: waiting.scope(),
            rule: &rule,
            bot,
            alert_id: waiting.id,
            work_item_id: waiting.work_item_id,
            title: &waiting.wi_title,
            current_assignee: waiting.wi_assignee,
            occurrences: waiting.occurrences,
            task_seq: waiting.task_seq,
            severity,
            kind,
            event_id,
        },
    )
    .await;
    match started {
        Ok(Some(_)) => {
            tx.commit().await?;
            Ok(Step::Started)
        }
        Ok(None) => Ok(Step::Held {
            blocked_rule: None,
            blocked_workspace: None,
        }),
        Err(error) => {
            // The dispatch guard (or the store) refused. The transaction is dropped and rolls back;
            // the entry stays queued until the hour is over.
            tracing::warn!(alert_id = %waiting.id, error = %error, "queued alert task was refused at start");
            Ok(Step::Held {
                blocked_rule: None,
                blocked_workspace: None,
            })
        }
    }
}
