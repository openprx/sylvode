//! Bot tasks of alerts: the safety caps, the start of a task and the notice of a suppression.
//!
//! Both the ingest transaction and the worker's queue drain (`queue`) go through these functions,
//! so a task started an hour later passes exactly the checks a task started at once passes,
//! including the central agent-dispatch guard inside `create_ai_task`.

use sea_orm::{ConnectionTrait, DbBackend, FromQueryResult, Statement};
use serde_json::json;
use uuid::Uuid;

use super::{
    audit,
    notify::{self, AlertNotification, Subject},
    rules::RuleRow,
    severity::Severity,
};
use crate::{
    error::ApiError,
    services::{
        ai_task_service::{CreateAiTaskInput, DispatchOrigin, create_ai_task},
        issues::write_activity,
    },
};

/// Statuses in which a task still occupies a slot. One list for every cap and every check, so the
/// runner protocol's `acknowledged` joins it without another change.
pub const ACTIVE_TASK_STATUSES: &str = "('pending','processing','acknowledged')";
pub const HOUR: &str = "interval '1 hour'";

#[derive(FromQueryResult)]
struct CountRow {
    n: i64,
}

#[derive(FromQueryResult)]
struct IdRow {
    id: Uuid,
}

/// One COUNT statement.
pub async fn scalar<C: ConnectionTrait>(db: &C, sql: &str, values: Vec<sea_orm::Value>) -> Result<i64, ApiError> {
    Ok(
        CountRow::find_by_statement(Statement::from_sql_and_values(DbBackend::Postgres, sql, values))
            .one(db)
            .await?
            .map_or(0, |row| row.n),
    )
}

/// The cap that refuses another task.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Cap {
    /// `max_tasks_per_hour` of the rule.
    Task,
    /// `max_active_tasks` of the rule.
    Active,
    /// `max_active_alert_tasks_per_workspace`.
    Workspace,
}

impl Cap {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Task => "task_cap",
            Self::Active => "active_cap",
            Self::Workspace => "workspace_cap",
        }
    }
}

/// What the caps are measured against right now.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize)]
pub struct CapState {
    pub tasks_last_hour: i64,
    pub active_for_rule: i64,
    pub active_in_workspace: i64,
}

impl CapState {
    /// The first cap that is reached, in the order task, active, workspace.
    pub fn refusal(&self, rule: &RuleRow, workspace_limit: u32) -> Option<Cap> {
        if self.tasks_last_hour >= i64::from(rule.max_tasks_per_hour) {
            Some(Cap::Task)
        } else if self.active_for_rule >= i64::from(rule.max_active_tasks) {
            Some(Cap::Active)
        } else if self.active_in_workspace >= i64::from(workspace_limit) {
            Some(Cap::Workspace)
        } else {
            None
        }
    }
}

/// Measures the caps of `rule_id` and of the workspace.
pub async fn cap_state<C: ConnectionTrait>(db: &C, rule_id: Uuid, workspace_id: Uuid) -> Result<CapState, ApiError> {
    let tasks_last_hour = scalar(
        db,
        &format!(
            "SELECT COUNT(*) AS n FROM inbound_events \
              WHERE rule_id = $1 AND task_created_at IS NOT NULL AND task_created_at > now() - {HOUR}"
        ),
        vec![rule_id.into()],
    )
    .await?;
    let active_for_rule = scalar(
        db,
        &format!(
            "SELECT COUNT(*) AS n FROM inbound_alerts a JOIN ai_tasks t ON t.id = a.task_id \
              WHERE a.rule_id = $1 AND t.status IN {ACTIVE_TASK_STATUSES}"
        ),
        vec![rule_id.into()],
    )
    .await?;
    let active_in_workspace = scalar(
        db,
        &format!(
            "SELECT COUNT(*) AS n FROM inbound_alerts a \
               JOIN ai_tasks t ON t.id = a.task_id \
               JOIN projects p ON p.id = a.project_id \
              WHERE p.workspace_id = $1 AND t.status IN {ACTIVE_TASK_STATUSES}"
        ),
        vec![workspace_id.into()],
    )
    .await?;
    Ok(CapState {
        tasks_last_hour,
        active_for_rule,
        active_in_workspace,
    })
}

/// The bot of a rule when it is still a bot account that is a member of the workspace.
pub async fn valid_bot<C: ConnectionTrait>(
    db: &C,
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
    .one(db)
    .await?;
    Ok(found.map(|row| row.id))
}

/// Where an alert lives: what a task, an audit event or a notice needs to know about its source.
#[derive(Clone, Copy)]
pub struct AlertScope<'a> {
    pub workspace_id: Uuid,
    pub project_id: Uuid,
    pub source_id: Uuid,
    pub source_name: &'a str,
    pub acting_user_id: Option<Uuid>,
}

/// The facts of one alert task to start.
pub struct TaskStart<'a> {
    pub scope: AlertScope<'a>,
    pub rule: &'a RuleRow,
    pub bot: Uuid,
    pub alert_id: Uuid,
    pub work_item_id: Uuid,
    pub title: &'a str,
    pub current_assignee: Option<Uuid>,
    pub occurrences: i32,
    pub task_seq: i32,
    pub severity: Severity,
    pub kind: &'a str,
    /// The event that asked for the task: the event of this request, or the queued one.
    pub event_id: Uuid,
}

/// Creates the alert's task through the central dispatch guard and records it on the alert, the
/// event and the work item. `None` when a task with the same idempotency key exists already.
pub async fn start_task<C: ConnectionTrait>(tx: &C, start: &TaskStart<'_>) -> Result<Option<Uuid>, ApiError> {
    let scope = start.scope;
    let seq = start.task_seq + 1;
    let task = create_ai_task(
        tx,
        CreateAiTaskInput {
            origin: DispatchOrigin::InboundRule {
                rule_id: start.rule.id,
                inbound_event_id: start.event_id,
                source_id: scope.source_id,
            },
            project_id: scope.project_id,
            ai_participant_id: start.bot,
            actor_id: scope.acting_user_id,
            task_type: "issue_assigned".to_owned(),
            reference_type: Some("work_item".to_owned()),
            reference_id: Some(start.work_item_id),
            priority: start.severity.task_priority(),
            payload: json!({
                "issue_id": start.work_item_id.to_string(),
                "project_id": scope.project_id.to_string(),
                "assignee_id": start.bot.to_string(),
                "trigger": "alert_rule",
                "title": start.title,
                "alert": {
                    "alert_id": start.alert_id.to_string(),
                    "rule_id": start.rule.id.to_string(),
                    "source_id": scope.source_id.to_string(),
                    "inbound_event_id": start.event_id.to_string(),
                    "severity": start.severity.as_str(),
                    "kind": start.kind,
                    "occurrences": start.occurrences,
                },
            }),
            idempotency_key: Some(format!("alert:{}:{seq}", start.alert_id)),
            max_attempts: start.rule.task_max_attempts,
        },
    )
    .await?;
    let Some(task) = task else {
        return Ok(None);
    };
    tx.execute(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "UPDATE inbound_alerts SET task_id = $2, task_seq = $3, task_state = 'created', task_ended_at = NULL, \
                task_queued_at = NULL, queued_event_id = NULL, updated_at = now() WHERE id = $1",
        vec![start.alert_id.into(), task.id.into(), seq.into()],
    ))
    .await?;
    tx.execute(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "UPDATE inbound_events SET task_id = $2, task_created_at = now() WHERE id = $1",
        vec![start.event_id.into(), task.id.into()],
    ))
    .await?;
    if start.current_assignee != Some(start.bot) {
        // Set directly: `queue_assignment_ai_task` would create a second task for the same work.
        tx.execute(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "UPDATE work_items SET assignee_id = $2, updated_at = now() WHERE id = $1",
            vec![start.work_item_id.into(), start.bot.into()],
        ))
        .await?;
        write_activity(
            tx,
            start.work_item_id,
            start.bot,
            "assigned",
            &json!({ "from": start.current_assignee.map(|id| id.to_string()), "to": start.bot.to_string() }),
            platform::timestamp::Timestamp::now(),
        )
        .await?;
    }
    audit::record(
        tx,
        scope.workspace_id,
        scope.project_id,
        "alert_rule.fired",
        "alert_rule",
        start.rule.id,
        scope.acting_user_id,
        audit::inbound_source_value(scope.source_id),
        json!({ "inbound_event_id": start.event_id, "alert_id": start.alert_id, "task_id": task.id }),
    )
    .await?;
    Ok(Some(task.id))
}

/// Records a suppression and tells the rule's recipients, at most once per rule per hour.
/// `cause` is a cap name, `bot_unavailable`, or a reason an alert's queued task was dropped.
pub async fn note_suppression<C: ConnectionTrait>(
    tx: &C,
    rule: &RuleRow,
    scope: AlertScope<'_>,
    cause: &str,
    work_item_id: Option<Uuid>,
) -> Result<(), ApiError> {
    if cause != "bot_unavailable" {
        audit::record(
            tx,
            scope.workspace_id,
            scope.project_id,
            "alert_rule.suppressed",
            "alert_rule",
            rule.id,
            scope.acting_user_id,
            audit::inbound_source_value(scope.source_id),
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
            workspace_id: scope.workspace_id,
            project_id: scope.project_id,
            work_item_id,
        },
        &rule.notify_user_ids,
        &format!("alert_suppressed:{}", rule.name),
        &json!({ "rule_id": rule.id, "cause": cause, "source_name": scope.source_name }),
    )
    .await?;
    Ok(())
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use crate::inbound::rules::ACTION_WORK_ITEM_AND_TASK;

    fn rule(max_tasks: i32, max_active: i32) -> RuleRow {
        RuleRow {
            id: Uuid::new_v4(),
            workspace_id: Uuid::new_v4(),
            project_id: Uuid::new_v4(),
            name: "r".into(),
            position: 0,
            enabled: true,
            source_id: None,
            kind_patterns: Vec::new(),
            min_severity: "high".into(),
            action: ACTION_WORK_ITEM_AND_TASK.into(),
            bot_user_id: None,
            instructions: String::new(),
            priority_map: json!({}),
            label_ids: Vec::new(),
            notify_user_ids: Vec::new(),
            max_tasks_per_hour: max_tasks,
            max_active_tasks: max_active,
            dedup_window_secs: 3600,
            rearm_after_secs: 900,
            task_max_attempts: 3,
            version: 1,
        }
    }

    #[test]
    fn the_first_reached_cap_refuses_in_a_fixed_order() {
        let r = rule(6, 2);
        let free = CapState {
            tasks_last_hour: 5,
            active_for_rule: 1,
            active_in_workspace: 4,
        };
        assert_eq!(free.refusal(&r, 5), None);
        assert_eq!(
            CapState {
                tasks_last_hour: 6,
                ..free
            }
            .refusal(&r, 5),
            Some(Cap::Task)
        );
        assert_eq!(
            CapState {
                active_for_rule: 2,
                ..free
            }
            .refusal(&r, 5),
            Some(Cap::Active)
        );
        assert_eq!(
            CapState {
                active_in_workspace: 5,
                ..free
            }
            .refusal(&r, 5),
            Some(Cap::Workspace)
        );
        assert_eq!(
            CapState {
                tasks_last_hour: 6,
                active_for_rule: 2,
                active_in_workspace: 5
            }
            .refusal(&r, 5)
            .map(Cap::as_str),
            Some("task_cap")
        );
    }
}
