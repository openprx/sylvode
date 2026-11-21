//! What a person asking "is my runner ready?" is shown: the status of a bot connection.
//!
//! Computed on read, from three sources and no new polling channel:
//!
//! 1. the deliveries the worker made for the bot's tasks (`ai_task_events`), any project, because
//!    the runner is shared;
//! 2. the bot token's last use (`workspace_bots.last_used_at`) against when its credentials were
//!    issued (`bot_runners.credentials_issued_at`);
//! 3. the runner's own report (`PUT /api/v1/ai/runner/report`), sanitised before it is stored: the
//!    API keeps component names, states and versions, never a path, a hint or a detail.

use platform::timestamp::Timestamp;
use sea_orm::{ConnectionTrait, DbBackend, FromQueryResult, Statement};
use serde::Serialize;
use serde_json::{Value, json};
use uuid::Uuid;

use crate::{error::ApiError, services::ai_task_service::TASK_PROTOCOL};

/// How often a runner reports, in seconds. Told to the runner by `GET /api/v1/ai/protocol`.
pub const RUNNER_REPORT_INTERVAL_SECS: i64 = 600;
/// A report is fresh for two intervals and a minute.
const REPORT_FRESH_SECS: i64 = 2 * RUNNER_REPORT_INTERVAL_SECS + 60;
/// The largest report body the API accepts.
pub const MAX_REPORT_BYTES: usize = 16 * 1024;
/// Deliveries older than this are no longer evidence of anything.
const DELIVERY_WINDOW_DAYS: i64 = 30;
/// How much of a delivery's error text the status shows.
const DETAIL_CHARS: usize = 200;
const MAX_VERSION_CHARS: usize = 64;

// ---------------------------------------------------------------------------------------------
// The report
// ---------------------------------------------------------------------------------------------

/// Whether `name` is a component the API keeps: `tmux`, `cli.claude`, `cli.codex`, `cli.opencode`,
/// `mcp_server` or `api_protocol`.
fn is_known_component(name: &str) -> bool {
    matches!(
        name,
        "tmux" | "cli.claude" | "cli.codex" | "cli.opencode" | "mcp_server" | "api_protocol"
    )
}

fn short_version(value: Option<&Value>) -> Option<String> {
    value
        .and_then(Value::as_str)
        .filter(|version| !version.is_empty() && version.chars().count() <= MAX_VERSION_CHARS)
        .map(str::to_string)
}

/// Reduces a runner's report to what the API keeps.
///
/// That is `generated_by.version`, `generated_by.task_protocol`, and per known component its
/// `component`, `status` (`ok`, `warn` or `fail`) and `version` (at most 64 characters).
/// Everything else, including every unknown key at any depth, is dropped, so no host path reaches
/// the database.
#[must_use]
pub fn sanitize_report(body: &Value) -> Value {
    let generated_by = body.get("generated_by");
    let mut kept_by = serde_json::Map::new();
    if let Some(version) = short_version(generated_by.and_then(|by| by.get("version"))) {
        kept_by.insert("version".to_string(), json!(version));
    }
    if let Some(protocol) = generated_by
        .and_then(|by| by.get("task_protocol"))
        .and_then(Value::as_u64)
        .filter(|protocol| *protocol < 1_000_000)
    {
        kept_by.insert("task_protocol".to_string(), json!(protocol));
    }
    let components: Vec<Value> = body
        .get("components")
        .and_then(Value::as_array)
        .map(|rows| {
            rows.iter()
                .filter_map(|row| {
                    let name = row
                        .get("component")
                        .and_then(Value::as_str)
                        .filter(|name| is_known_component(name))?;
                    let status = row
                        .get("status")
                        .and_then(Value::as_str)
                        .filter(|status| matches!(*status, "ok" | "warn" | "fail"))?;
                    let mut kept = serde_json::Map::new();
                    kept.insert("component".to_string(), json!(name));
                    kept.insert("status".to_string(), json!(status));
                    if let Some(version) = short_version(row.get("version")) {
                        kept.insert("version".to_string(), json!(version));
                    }
                    Some(Value::Object(kept))
                })
                .collect()
        })
        .unwrap_or_default();
    json!({ "generated_by": kept_by, "components": components })
}

/// Stores a runner's sanitised report. 404 when `bot_id` is not a runner bot of a connection.
pub async fn store_report(db: &impl ConnectionTrait, bot_id: Uuid, report: &Value) -> Result<(), ApiError> {
    let stored = db
        .execute(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "UPDATE bot_runners SET report = $2, reported_at = now(), updated_at = now() WHERE bot_id = $1",
            vec![bot_id.into(), report.clone().into()],
        ))
        .await?;
    if stored.rows_affected() == 0 {
        return Err(ApiError::NotFound("this bot has no runner connection".to_string()));
    }
    Ok(())
}

// ---------------------------------------------------------------------------------------------
// The status
// ---------------------------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct RunnerStatus {
    /// `waiting`, `online`, `not_reporting` or `unreachable`.
    pub state: &'static str,
    pub reported_at: Option<Timestamp>,
    pub credentials_used_at: Option<Timestamp>,
    pub version: Option<String>,
    pub task_protocol: Option<u64>,
    /// `protocol_mismatch` when the runner speaks another task protocol than this API.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub warning: Option<&'static str>,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct ComponentStatus {
    /// `ready`, `warning`, `missing`, `disabled` or `unknown`.
    pub state: &'static str,
    /// `report`, `delivery` or `none`.
    pub source: &'static str,
    pub version: Option<String>,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct DeliveryStatus {
    /// `delivered`, `refused`, `failed` or `none`.
    pub state: &'static str,
    pub at: Option<Timestamp>,
    pub project_key: Option<String>,
    pub task_id: Option<Uuid>,
    pub reason_code: Option<String>,
    pub detail: Option<String>,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct ConnectionStatus {
    pub runner: RunnerStatus,
    pub cli: ComponentStatus,
    /// `None` for the headless channel, which does not use tmux.
    pub tmux: Option<ComponentStatus>,
    pub last_delivery: DeliveryStatus,
    pub computed_at: Timestamp,
}

#[derive(Debug, FromQueryResult)]
struct SourceRow {
    credentials_issued_at: Timestamp,
    report: Option<Value>,
    reported_at: Option<Timestamp>,
    last_used_at: Option<Timestamp>,
}

#[derive(Debug, FromQueryResult)]
struct DeliveryRow {
    event_type: String,
    payload: Value,
    created_at: Timestamp,
    task_id: Uuid,
    project_key: String,
}

/// The inputs of the derivation, separate from the database so each row of the table is a test.
pub struct StatusInputs {
    pub now: Timestamp,
    pub credentials_issued_at: Timestamp,
    pub last_used_at: Option<Timestamp>,
    pub report: Option<Value>,
    pub reported_at: Option<Timestamp>,
    pub latest_delivery: Option<LatestDelivery>,
    pub cli: String,
    pub channel: String,
}

/// The latest delivery event of the bot's tasks.
pub struct LatestDelivery {
    pub kind: DeliveryKind,
    pub at: Timestamp,
    pub project_key: String,
    pub task_id: Uuid,
    pub reason_code: Option<String>,
    pub detail: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeliveryKind {
    Delivered,
    /// The runner refused (`runner_declined`).
    Refused,
    /// The delivery did not reach the runner (`delivery_failed`).
    Failed,
}

fn first_chars(text: &str) -> String {
    text.chars().take(DETAIL_CHARS).collect()
}

fn component_in<'a>(report: &'a Value, name: &str) -> Option<&'a Value> {
    report
        .get("components")?
        .as_array()?
        .iter()
        .find(|row| row.get("component").and_then(Value::as_str) == Some(name))
}

/// The state of one component (the CLI or tmux) from the report, else the latest delivery.
fn component_status(
    inputs: &StatusInputs,
    report_is_fresh: bool,
    component: &str,
    refusal_state: &[(&'static str, &'static str)],
) -> ComponentStatus {
    if report_is_fresh
        && let Some(row) = inputs
            .report
            .as_ref()
            .and_then(|report| component_in(report, component))
    {
        let state = match row.get("status").and_then(Value::as_str) {
            Some("ok") => Some("ready"),
            Some("warn") => Some("warning"),
            Some("fail") => Some("missing"),
            _ => None,
        };
        if let Some(state) = state {
            return ComponentStatus {
                state,
                source: "report",
                version: short_version(row.get("version")),
            };
        }
    }
    if let Some(delivery) = &inputs.latest_delivery {
        if delivery.kind == DeliveryKind::Refused
            && let Some(code) = delivery.reason_code.as_deref()
            && let Some((_, state)) = refusal_state.iter().find(|(refused, _)| *refused == code)
        {
            return ComponentStatus {
                state,
                source: "delivery",
                version: None,
            };
        }
        if delivery.kind == DeliveryKind::Delivered {
            return ComponentStatus {
                state: "ready",
                source: "delivery",
                version: None,
            };
        }
    }
    ComponentStatus {
        state: "unknown",
        source: "none",
        version: None,
    }
}

/// Derives the status of a connection (rules of CONNECT-A-BOT-RULES.md 5.2).
#[must_use]
pub fn derive(inputs: &StatusInputs) -> ConnectionStatus {
    let seconds_since = |at: Timestamp| (inputs.now.unix_timestamp() - at.unix_timestamp()).max(0);
    let report_is_fresh = inputs
        .reported_at
        .is_some_and(|at| seconds_since(at) < REPORT_FRESH_SECS);
    let credentials_used = inputs
        .last_used_at
        .is_some_and(|used| used >= inputs.credentials_issued_at);

    // A delivery that did not reach the runner, after the runner's last sign of life.
    let unreachable = inputs.latest_delivery.as_ref().is_some_and(|delivery| {
        delivery.kind == DeliveryKind::Failed && inputs.reported_at.is_none_or(|reported_at| delivery.at > reported_at)
    });

    let state = if !credentials_used {
        "waiting"
    } else if unreachable {
        "unreachable"
    } else if report_is_fresh {
        "online"
    } else {
        // A report exists but is stale, or the token was used but never reported.
        "not_reporting"
    };

    let report_generated_by = inputs.report.as_ref().and_then(|report| report.get("generated_by"));
    let task_protocol = report_generated_by
        .and_then(|by| by.get("task_protocol"))
        .and_then(Value::as_u64);
    let runner = RunnerStatus {
        state,
        reported_at: inputs.reported_at,
        credentials_used_at: inputs.last_used_at,
        version: short_version(report_generated_by.and_then(|by| by.get("version"))),
        task_protocol,
        warning: task_protocol
            .filter(|protocol| *protocol != u64::from(TASK_PROTOCOL))
            .map(|_| "protocol_mismatch"),
    };

    let cli_component = format!("cli.{}", inputs.cli);
    let cli = component_status(
        inputs,
        report_is_fresh,
        &cli_component,
        &[("cli_unavailable", "missing"), ("cli_disabled", "disabled")],
    );
    let tmux = (inputs.channel == "tmux")
        .then(|| component_status(inputs, report_is_fresh, "tmux", &[("tmux_unavailable", "missing")]));

    let last_delivery = inputs.latest_delivery.as_ref().map_or(
        DeliveryStatus {
            state: "none",
            at: None,
            project_key: None,
            task_id: None,
            reason_code: None,
            detail: None,
        },
        |delivery| DeliveryStatus {
            state: match delivery.kind {
                DeliveryKind::Delivered => "delivered",
                DeliveryKind::Refused => "refused",
                DeliveryKind::Failed => "failed",
            },
            at: Some(delivery.at),
            project_key: Some(delivery.project_key.clone()),
            task_id: Some(delivery.task_id),
            reason_code: delivery.reason_code.clone(),
            detail: delivery.detail.clone(),
        },
    );

    ConnectionStatus {
        runner,
        cli,
        tmux,
        last_delivery,
        computed_at: inputs.now,
    }
}

/// The status of the connection of `bot_id`, for a project whose settings name `cli` and
/// `channel`.
pub async fn compute(
    db: &impl ConnectionTrait,
    bot_id: Uuid,
    cli: &str,
    channel: &str,
) -> Result<ConnectionStatus, ApiError> {
    let source = SourceRow::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "SELECT r.credentials_issued_at, r.report, r.reported_at, b.last_used_at \
         FROM bot_runners r JOIN workspace_bots b ON b.id = r.bot_id WHERE r.bot_id = $1",
        vec![bot_id.into()],
    ))
    .one(db)
    .await?
    .ok_or_else(|| ApiError::NotFound("this bot has no runner connection".to_string()))?;

    let delivery = DeliveryRow::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "SELECT e.event_type, e.payload, e.created_at, t.id AS task_id, p.key AS project_key \
         FROM ai_task_events e \
         JOIN ai_tasks t ON t.id = e.task_id \
         JOIN projects p ON p.id = t.project_id \
         WHERE t.ai_participant_id = $1 \
           AND e.created_at > now() - make_interval(days => $2::int) \
           AND (e.event_type = 'delivered' \
                OR (e.event_type IN ('failed', 'retried') \
                    AND e.payload->>'reason' IN ('runner_declined', 'delivery_failed'))) \
         ORDER BY e.created_at DESC \
         LIMIT 1",
        vec![bot_id.into(), i32::try_from(DELIVERY_WINDOW_DAYS).unwrap_or(30).into()],
    ))
    .one(db)
    .await?;

    let latest_delivery = delivery.map(|row| {
        let reason = row.payload.get("reason").and_then(Value::as_str);
        let kind = match (row.event_type.as_str(), reason) {
            ("delivered", _) => DeliveryKind::Delivered,
            (_, Some("runner_declined")) => DeliveryKind::Refused,
            _ => DeliveryKind::Failed,
        };
        LatestDelivery {
            kind,
            at: row.created_at,
            project_key: row.project_key,
            task_id: row.task_id,
            reason_code: row
                .payload
                .get("reason_code")
                .and_then(Value::as_str)
                .filter(|code| crate::services::ai_task_service::is_reason_code(code))
                .map(str::to_string),
            detail: row
                .payload
                .get("error_message")
                .and_then(Value::as_str)
                .map(first_chars),
        }
    });

    Ok(derive(&StatusInputs {
        now: Timestamp::now(),
        credentials_issued_at: source.credentials_issued_at,
        last_used_at: source.last_used_at,
        report: source.report,
        reported_at: source.reported_at,
        latest_delivery,
        cli: cli.to_string(),
        channel: channel.to_string(),
    }))
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::panic, clippy::indexing_slicing)]
mod tests {
    use super::{
        DeliveryKind, LatestDelivery, MAX_REPORT_BYTES, RUNNER_REPORT_INTERVAL_SECS, StatusInputs, derive,
        sanitize_report,
    };
    use platform::timestamp::Timestamp;
    use serde_json::{Value, json};
    use uuid::Uuid;

    fn at(rfc3339: &str) -> Timestamp {
        Timestamp::parse_rfc3339(rfc3339).expect("a timestamp")
    }

    fn now() -> Timestamp {
        at("2026-11-21T12:00:00Z")
    }

    fn report(cli_status: &str, tmux_status: &str) -> Value {
        json!({
            "generated_by": {"version": "0.5.1", "task_protocol": 1},
            "components": [
                {"component": "tmux", "status": tmux_status, "version": "3.4"},
                {"component": "cli.claude", "status": cli_status, "version": "2.1.0"},
            ]
        })
    }

    fn inputs() -> StatusInputs {
        StatusInputs {
            now: now(),
            credentials_issued_at: at("2026-11-21T10:00:00Z"),
            last_used_at: Some(at("2026-11-21T11:00:00Z")),
            report: Some(report("ok", "ok")),
            reported_at: Some(at("2026-11-21T11:55:00Z")),
            latest_delivery: None,
            cli: "claude".to_string(),
            channel: "tmux".to_string(),
        }
    }

    fn delivery(kind: DeliveryKind, when: &str, code: Option<&str>) -> LatestDelivery {
        LatestDelivery {
            kind,
            at: at(when),
            project_key: "SVWL".to_string(),
            task_id: Uuid::nil(),
            reason_code: code.map(str::to_string),
            detail: Some("detail".to_string()),
        }
    }

    #[test]
    fn the_runner_row_follows_the_table() {
        // waiting: the new credentials have not been used.
        let mut waiting = inputs();
        waiting.last_used_at = Some(at("2026-11-21T09:00:00Z"));
        assert_eq!(derive(&waiting).runner.state, "waiting");
        waiting.last_used_at = None;
        assert_eq!(derive(&waiting).runner.state, "waiting", "never used");
        // online: a fresh report.
        let online = derive(&inputs());
        assert_eq!(online.runner.state, "online");
        assert_eq!(online.runner.version.as_deref(), Some("0.5.1"));
        assert_eq!(online.runner.task_protocol, Some(1));
        assert_eq!(online.runner.warning, None);
        // not_reporting: a stale report, or token used with no report.
        let mut stale = inputs();
        stale.reported_at = Some(at("2026-11-21T11:30:00Z"));
        assert_eq!(derive(&stale).runner.state, "not_reporting");
        let mut none = inputs();
        none.report = None;
        none.reported_at = None;
        assert_eq!(derive(&none).runner.state, "not_reporting");
        // The freshness edge is two intervals plus a minute.
        let fresh_secs = 2 * RUNNER_REPORT_INTERVAL_SECS + 60;
        let mut edge = inputs();
        edge.reported_at =
            Some(Timestamp::from_unix_timestamp(now().unix_timestamp() - fresh_secs + 1, 0).expect("time"));
        assert_eq!(derive(&edge).runner.state, "online");
        edge.reported_at = Some(Timestamp::from_unix_timestamp(now().unix_timestamp() - fresh_secs, 0).expect("time"));
        assert_eq!(derive(&edge).runner.state, "not_reporting");
        // unreachable: the latest delivery failed after the last report.
        let mut unreachable = inputs();
        unreachable.latest_delivery = Some(delivery(DeliveryKind::Failed, "2026-11-21T11:58:00Z", None));
        assert_eq!(derive(&unreachable).runner.state, "unreachable");
        // A failure older than the report is not newer than it.
        unreachable.latest_delivery = Some(delivery(DeliveryKind::Failed, "2026-11-21T11:50:00Z", None));
        assert_eq!(derive(&unreachable).runner.state, "online");
        // A protocol the API does not speak is a warning.
        let mut mismatch = inputs();
        mismatch.report = Some(json!({"generated_by": {"task_protocol": 2}, "components": []}));
        assert_eq!(derive(&mismatch).runner.warning, Some("protocol_mismatch"));
    }

    #[test]
    fn the_cli_and_tmux_rows_prefer_a_fresh_report_then_the_latest_delivery() {
        // Fresh report states.
        for (status, state) in [("ok", "ready"), ("warn", "warning"), ("fail", "missing")] {
            let mut input = inputs();
            input.report = Some(report(status, status));
            let derived = derive(&input);
            assert_eq!((derived.cli.state, derived.cli.source), (state, "report"));
            assert_eq!(derived.cli.version.as_deref(), Some("2.1.0"));
            let tmux = derived.tmux.expect("the tmux channel shows tmux");
            assert_eq!((tmux.state, tmux.source), (state, "report"));
        }
        // A stale report says nothing: the latest delivery decides.
        let mut stale = inputs();
        stale.reported_at = Some(at("2026-11-21T08:00:00Z"));
        stale.latest_delivery = Some(delivery(
            DeliveryKind::Refused,
            "2026-11-21T11:00:00Z",
            Some("cli_unavailable"),
        ));
        assert_eq!(derive(&stale).cli.state, "missing");
        assert_eq!(derive(&stale).cli.source, "delivery");
        stale.latest_delivery = Some(delivery(
            DeliveryKind::Refused,
            "2026-11-21T11:00:00Z",
            Some("cli_disabled"),
        ));
        assert_eq!(derive(&stale).cli.state, "disabled");
        stale.latest_delivery = Some(delivery(
            DeliveryKind::Refused,
            "2026-11-21T11:00:00Z",
            Some("tmux_unavailable"),
        ));
        assert_eq!(derive(&stale).tmux.expect("tmux").state, "missing");
        assert_eq!(
            derive(&stale).cli.state,
            "unknown",
            "another component's refusal says nothing of the CLI"
        );
        stale.latest_delivery = Some(delivery(DeliveryKind::Delivered, "2026-11-21T11:00:00Z", None));
        assert_eq!(derive(&stale).cli.state, "ready");
        assert_eq!(derive(&stale).tmux.expect("tmux").state, "ready");
        // Nothing at all.
        let mut nothing = inputs();
        nothing.report = None;
        nothing.reported_at = None;
        let unknown = derive(&nothing);
        assert_eq!((unknown.cli.state, unknown.cli.source), ("unknown", "none"));
        // A component the report does not list falls through.
        let mut absent = inputs();
        absent.cli = "codex".to_string();
        assert_eq!(derive(&absent).cli.state, "unknown");
        // Headless hides tmux.
        let mut headless = inputs();
        headless.channel = "headless".to_string();
        assert!(derive(&headless).tmux.is_none());
    }

    #[test]
    fn the_last_delivery_row_reports_the_latest_event() {
        let none = derive(&inputs()).last_delivery;
        assert_eq!(none.state, "none");
        for (kind, state) in [
            (DeliveryKind::Delivered, "delivered"),
            (DeliveryKind::Refused, "refused"),
            (DeliveryKind::Failed, "failed"),
        ] {
            let mut input = inputs();
            input.latest_delivery = Some(delivery(kind, "2026-11-21T11:00:00Z", Some("path_outside_allowlist")));
            let last = derive(&input).last_delivery;
            assert_eq!(last.state, state);
            assert_eq!(last.project_key.as_deref(), Some("SVWL"));
            assert_eq!(last.reason_code.as_deref(), Some("path_outside_allowlist"));
            assert_eq!(last.detail.as_deref(), Some("detail"));
        }
    }

    #[test]
    fn a_report_keeps_only_known_components_states_and_short_versions() {
        let hostile = json!({
            "schema": "sylvode.doctor.v1",
            "generated_by": {"binary": "sylvode-runner", "version": "0.5.1", "task_protocol": 1, "path": "/home/dev/bin"},
            "components": [
                {"component": "tmux", "status": "ok", "version": "3.4", "detail": "/usr/bin/tmux", "hint": "install it"},
                {"component": "cli.claude", "status": "warn", "version": "v".repeat(65), "path": "/opt/claude"},
                {"component": "cli.vim", "status": "ok"},
                {"component": "mcp_server", "status": "great"},
                {"component": "api_protocol", "status": "fail"},
                "not an object",
            ],
            "projects": [{"working_directory": "/home/dev/work"}],
            "summary": {"ok": 1}
        });
        let kept = sanitize_report(&hostile);
        assert_eq!(
            kept,
            json!({
                "generated_by": {"version": "0.5.1", "task_protocol": 1},
                "components": [
                    {"component": "tmux", "status": "ok", "version": "3.4"},
                    {"component": "cli.claude", "status": "warn"},
                    {"component": "api_protocol", "status": "fail"},
                ]
            })
        );
        assert!(!kept.to_string().contains('/'), "no path survives");
        // Garbage in, an empty report out.
        assert_eq!(
            sanitize_report(&json!("text")),
            json!({"generated_by": {}, "components": []})
        );
        assert_eq!(MAX_REPORT_BYTES, 16 * 1024);
    }
}
