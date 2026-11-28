//! The protocol handshake and the periodic report (design section 4.11).
//!
//! At start-up, and then every report interval, the runner asks the API for its task protocol with
//! each `runner` agent's bot token (`GET /api/v1/ai/protocol`, with the bot's surface header),
//! compares `task_protocol` with [`TASK_PROTOCOL`], and sends the sanitised report
//! (`PUT /api/v1/ai/runner/report`). `/health` shows the result. A mismatch refuses every `runner`
//! agent at delivery with `reason_code = "protocol_mismatch"`; other agents keep working.
//!
//! The report holds component names, statuses and versions only: the API keeps nothing else, and
//! this side never builds more (no path, hint or detail). Tokens are used for requests and never
//! logged; a refused token is logged as `bot <id>: token refused by Sylvode (revoked, rotated or
//! wrong surface)`.

use std::collections::HashSet;
use std::sync::Arc;
use std::time::Duration;

use parking_lot::Mutex;
use serde_json::{Value, json};
use sylvode_prereq::{Component, ComponentKind, Finding, InstallKind, Status, severity};

use crate::config::{AiTaskCallback, Config};
use crate::runner::sv_client::{SvClient, SvError, TASK_PROTOCOL, token_refused};

/// The shortest and the longest report interval taken from the API, whatever it advertises: a
/// misconfigured or hostile server cannot make every runner report every second.
pub const MIN_ADVERTISED_INTERVAL_SECS: u64 = 60;
pub const MAX_ADVERTISED_INTERVAL_SECS: u64 = 3600;

/// The interval used when the API does not advertise one.
pub const DEFAULT_INTERVAL_SECS: u64 = 600;

/// How long one handshake or report call may take.
const CALL_TIMEOUT: Duration = Duration::from_secs(10);

/// What the handshake found.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProtocolState {
    /// There is no `runner` agent, so nothing was checked.
    Unchecked,
    /// Not checked yet.
    Pending,
    Ok,
    /// The API speaks another protocol.
    Mismatch,
    /// No answer, or the token was refused: delivery checks will say more.
    Unreachable,
}

impl ProtocolState {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Unchecked => "unchecked",
            Self::Pending => "pending",
            Self::Ok => "ok",
            Self::Mismatch => "mismatch",
            Self::Unreachable => "unreachable",
        }
    }
}

#[derive(Clone, Debug)]
struct Status_ {
    state: ProtocolState,
    api: Option<u64>,
}

#[derive(Default)]
struct Memory {
    /// Bots whose report Sylvode answered 404 for: not retried until restart.
    no_runner_row: HashSet<String>,
    /// Bots whose refused token was already logged (cleared when it works again).
    refused_logged: HashSet<String>,
}

/// The shared result of the handshake.
#[derive(Clone)]
pub struct Handshake {
    status: Arc<Mutex<Status_>>,
    memory: Arc<Mutex<Memory>>,
    prereq: Arc<crate::doctor::Prereq>,
}

impl Handshake {
    pub fn new(has_runner_agents: bool) -> Self {
        Self {
            status: Arc::new(Mutex::new(Status_ {
                state: if has_runner_agents {
                    ProtocolState::Pending
                } else {
                    ProtocolState::Unchecked
                },
                api: None,
            })),
            memory: Arc::new(Mutex::new(Memory::default())),
            prereq: Arc::new(crate::doctor::Prereq::default()),
        }
    }

    /// The `protocol` object of `/health`.
    pub fn health(&self) -> Value {
        let status = self.status.lock();
        json!({"runner": TASK_PROTOCOL, "api": status.api, "state": status.state.as_str()})
    }

    /// The text a delivery is refused with while the protocols differ.
    pub fn mismatch_text(&self) -> Option<String> {
        let status = self.status.lock();
        (status.state == ProtocolState::Mismatch).then(|| {
            format!(
                "task protocol mismatch (runner {TASK_PROTOCOL}, api {}); upgrade the older component",
                status.api.map_or_else(|| "unknown".to_string(), |api| api.to_string())
            )
        })
    }
}

/// One bot the handshake speaks for. `Debug` hides the token.
#[derive(Clone)]
pub struct Target {
    pub bot_id: String,
    pub api_url: String,
    pub token: String,
    pub surface: String,
    /// The bot's workspace; the config check guarantees a runner bot has one.
    pub workspace_id: String,
}

impl std::fmt::Debug for Target {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Target")
            .field("bot_id", &self.bot_id)
            .field("api_url", &self.api_url)
            .field("token", &"<redacted>")
            .field("surface", &self.surface)
            .field("workspace_id", &self.workspace_id)
            .finish()
    }
}

/// The bots of the `runner` agents with their credentials from `[ai_task_callback]`.
pub fn targets(config: &Config) -> Vec<Target> {
    let mut out: Vec<Target> = Vec::new();
    for agent in config.agents.iter().filter(|a| a.agent_type == "runner") {
        for id in agent.runner_bot_ids() {
            if out.iter().any(|t| t.bot_id.eq_ignore_ascii_case(&id)) {
                continue;
            }
            if let AiTaskCallback::Report(t) = config.ai_task_callback_for(&id) {
                let Some(workspace_id) = t.workspace_id else { continue };
                out.push(Target {
                    bot_id: id,
                    api_url: t.api_url.to_string(),
                    token: t.token.to_string(),
                    surface: t.surface.to_string(),
                    workspace_id: workspace_id.to_string(),
                });
            }
        }
    }
    out
}

impl Handshake {
    /// The rows of the doctor's snapshot that Sylvode keeps (`tmux`, `cli.*`, `mcp_server`,
    /// `runner`), as of at most a minute ago: probing runs programs, so it happens off the async
    /// threads and at most once a minute.
    pub async fn prerequisite_rows(&self, config: &Config) -> Vec<Component> {
        let (prereq, config) = (Arc::clone(&self.prereq), config.clone());
        let rows = tokio::task::spawn_blocking(move || prereq.rows(&config))
            .await
            .unwrap_or_default();
        rows.into_iter()
            .filter(|row| {
                row.component == "tmux"
                    || row.component == "mcp_server"
                    || row.component == "runner"
                    || row.component.starts_with("cli.")
            })
            .collect()
    }

    /// The current prerequisite rows (cached for a minute).
    pub async fn rows(&self, config: &Config) -> Vec<Component> {
        let (prereq, config) = (Arc::clone(&self.prereq), config.clone());
        tokio::task::spawn_blocking(move || prereq.rows(&config))
            .await
            .unwrap_or_default()
    }
}

/// The report body: the subset Sylvode keeps. Information counts as ok; paths, hints and details
/// never leave the process.
pub fn report_body(
    rows: &[Component],
    sessions: &[crate::runner::session_report::Assessment],
    postures: &[(&'static str, crate::runner::session_report::Posture)],
) -> Value {
    let components: Vec<Value> = rows
        .iter()
        .map(|row| {
            let status = match row.status {
                Status::Ok | Status::Info => "ok",
                Status::Warn => "warn",
                Status::Fail => "fail",
            };
            let mut kept = serde_json::Map::new();
            kept.insert("component".to_string(), json!(row.component));
            kept.insert("status".to_string(), json!(status));
            if let Some(version) = &row.version {
                kept.insert("version".to_string(), json!(version));
            }
            Value::Object(kept)
        })
        .collect();
    let mut components = components;
    components.extend(postures.iter().map(|(cli, posture)| posture_row(cli, *posture)));
    components.extend(sessions.iter().map(session_row));
    json!({
        "generated_by": {"version": env!("CARGO_PKG_VERSION"), "task_protocol": TASK_PROTOCOL},
        "components": components,
    })
}

/// A `posture` row, one per CLI: whether the launch settings of each channel run with permission
/// checks off.
fn posture_row(cli: &str, posture: crate::runner::session_report::Posture) -> Value {
    json!({
        "component": "posture",
        "cli": cli,
        "status": if posture.tmux || posture.headless { "warn" } else { "ok" },
        "bypass": {"tmux": posture.tmux, "headless": posture.headless},
    })
}

/// A `tmux_session` row: `scope` (`bot` for a bot's own session, `project` for a project's), the
/// project key for a project row only, the CLI type and fields of fixed vocabularies (including
/// where the tmux name came from, never the name); no session name, path or pane id.
fn session_row(session: &crate::runner::session_report::Assessment) -> Value {
    let status = match session.status {
        Status::Ok | Status::Info => "ok",
        Status::Warn => "warn",
        Status::Fail => "fail",
    };
    let mut row = serde_json::Map::new();
    row.insert("component".to_string(), json!("tmux_session"));
    if let Some(key) = session.name.key() {
        row.insert("scope".to_string(), json!("project"));
        row.insert("project_key".to_string(), json!(key));
    } else {
        row.insert("scope".to_string(), json!("bot"));
        // Which member of the bot's pool the row is (0 for the primary).
        row.insert("member".to_string(), json!(session.name.member()));
    }
    row.insert("cli".to_string(), json!(session.name.cli()));
    row.insert("status".to_string(), json!(status));
    row.insert("state".to_string(), json!(session.state.word()));
    row.insert("session_source".to_string(), json!(session.source.as_str()));
    if let Some(created_by) = session.created_by {
        row.insert("created_by".to_string(), json!(created_by.word()));
    }
    if let Some(hooks) = session.hooks {
        row.insert("hooks".to_string(), json!(if hooks { "seen" } else { "none" }));
    }
    if let Some(permissions) = session.permissions {
        row.insert("permissions".to_string(), json!(permissions.word()));
    }
    if let Some(mcp) = session.mcp {
        row.insert("mcp".to_string(), json!(mcp.word()));
    }
    Value::Object(row)
}

fn api_row(api_protocol: u64, api_version: Option<&str>) -> Component {
    let finding = if api_protocol == u64::from(TASK_PROTOCOL) {
        Finding::Healthy
    } else {
        Finding::ProtocolMismatch
    };
    let status = severity(InstallKind::RunnerTmux, ComponentKind::ApiProtocol, finding);
    let mut row = Component::new("api_protocol", true, status, true);
    row.version = api_version.map(str::to_string);
    row
}

/// One round: the protocol call for every bot, then the report for every bot that answered.
/// Returns the interval until the next round.
pub async fn run_once(config: &Config, http: &reqwest::Client, handshake: &Handshake) -> Duration {
    let mut interval = config.runner.report_interval_secs.unwrap_or(DEFAULT_INTERVAL_SECS);
    let mut mismatch: Option<u64> = None;
    let mut answered = false;
    // The sessions are looked at once a round (read-only calls that run programs).
    let sessions = {
        let config = config.clone();
        tokio::task::spawn_blocking(move || crate::runner::session_report::assess_recorded(&config))
            .await
            .unwrap_or_default()
    };
    let postures = crate::runner::session_report::postures_of(&config.runner);
    for target in targets(config) {
        let client = SvClient::new(http.clone(), &target.api_url, &target.token, &target.surface);
        let data = match client.protocol(CALL_TIMEOUT).await {
            Ok(data) => data,
            Err(e) => {
                note_failure(handshake, &target, &e);
                continue;
            }
        };
        handshake.memory.lock().refused_logged.remove(&target.bot_id);
        answered = true;
        let api = data.get("task_protocol").and_then(Value::as_u64).unwrap_or(0);
        if config.runner.report_interval_secs.is_none()
            && let Some(secs) = data.get("runner_report_interval_secs").and_then(Value::as_u64)
        {
            interval = secs.clamp(MIN_ADVERTISED_INTERVAL_SECS, MAX_ADVERTISED_INTERVAL_SECS);
        }
        if api != u64::from(TASK_PROTOCOL) {
            mismatch = Some(api);
        }
        let version = data.get("api_version").and_then(Value::as_str);
        let mut rows = vec![api_row(api, version)];
        rows.extend(handshake.prerequisite_rows(config).await);
        if handshake.memory.lock().no_runner_row.contains(&target.bot_id) {
            continue;
        }
        // Only the sessions this bot serves.
        let own: Vec<_> = sessions
            .iter()
            .filter(|session| session.bot_id.eq_ignore_ascii_case(&target.bot_id))
            .cloned()
            .collect();
        match client
            .put_report(&report_body(&rows, &own, &postures), CALL_TIMEOUT)
            .await
        {
            Ok(()) => {}
            Err(SvError::Status(404)) => {
                tracing::warn!(
                    "bot {}: Sylvode has no runner connection for this bot; its report is not sent again until restart",
                    target.bot_id
                );
                handshake.memory.lock().no_runner_row.insert(target.bot_id.clone());
            }
            Err(e) => note_failure(handshake, &target, &e),
        }
    }
    record(handshake, mismatch, answered);
    Duration::from_secs(interval)
}

/// Stores the result of a round.
fn record(handshake: &Handshake, mismatch: Option<u64>, answered: bool) {
    let mut status = handshake.status.lock();
    let previous = status.state;
    match (mismatch, answered) {
        (Some(api), _) => {
            status.state = ProtocolState::Mismatch;
            status.api = Some(api);
            if previous != ProtocolState::Mismatch {
                tracing::error!(
                    "task protocol mismatch: runner {TASK_PROTOCOL}, api {api}; upgrade the older component"
                );
            }
        }
        (None, true) => {
            status.state = ProtocolState::Ok;
            status.api = Some(u64::from(TASK_PROTOCOL));
        }
        (None, false) => {
            if status.state == ProtocolState::Pending || status.state == ProtocolState::Ok {
                status.state = ProtocolState::Unreachable;
            }
        }
    }
}

fn note_failure(handshake: &Handshake, target: &Target, error: &SvError) {
    if token_refused(error) {
        if handshake.memory.lock().refused_logged.insert(target.bot_id.clone()) {
            tracing::error!(
                "bot {}: token refused by Sylvode (revoked, rotated or wrong surface)",
                target.bot_id
            );
        }
    } else {
        tracing::warn!("bot {}: {error}", target.bot_id);
    }
}

/// Asks for a report before the next periodic one: a session was created, recovered or first used,
/// and Sylvode should not show the old state for up to a whole interval.
///
/// Asking is free and cheap to repeat. The loop decides when a round really runs ([`Pacing`]), so
/// any number of requests cost at most one round per [`MIN_ROUND_GAP`].
#[derive(Clone, Default)]
pub struct ReportTrigger(Arc<tokio::sync::Notify>);

impl ReportTrigger {
    pub fn request(&self) {
        self.0.notify_one();
    }
}

/// How an extra round is paced. A request waits `debounce` (so several that arrive together are one
/// round), and a round never starts less than `min_gap` after the start of the previous one,
/// periodic or not: at most six extra reports a minute, under the API's limit of twelve a minute
/// for one bot, and the periodic report is at most one a minute.
#[derive(Clone, Copy, Debug)]
pub struct Pacing {
    pub debounce: Duration,
    pub min_gap: Duration,
}

/// The pacing the runner runs with.
pub const MIN_ROUND_GAP: Duration = Duration::from_secs(10);
pub const PACING: Pacing = Pacing {
    debounce: Duration::from_secs(2),
    min_gap: MIN_ROUND_GAP,
};

/// Runs `round` now, then again after the interval it returned, or earlier when `trigger` asks and
/// the pacing allows, until shutdown.
pub async fn drive<F, Fut>(
    trigger: &ReportTrigger,
    background: &crate::background::Background,
    pacing: Pacing,
    mut round: F,
) where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = Duration>,
{
    loop {
        let started = tokio::time::Instant::now();
        let wait = round().await;
        tokio::select! {
            () = tokio::time::sleep(wait) => {}
            () = background.cancelled() => return,
            () = trigger.0.notified() => {
                let paced = async {
                    tokio::time::sleep(pacing.debounce).await;
                    if let Some(remaining) = pacing.min_gap.checked_sub(started.elapsed()) {
                        tokio::time::sleep(remaining).await;
                    }
                };
                tokio::select! {
                    () = paced => {}
                    () = background.cancelled() => return,
                }
                // Requests that arrived while waiting are answered by the round about to run.
                let _ = tokio::time::timeout(Duration::ZERO, trigger.0.notified()).await;
            }
        }
    }
}

/// The start-up task: a round now and one every interval, or sooner when asked, until shutdown.
pub async fn run_loop(
    config: Arc<Config>,
    http: reqwest::Client,
    handshake: Handshake,
    background: crate::background::Background,
    trigger: ReportTrigger,
) {
    drive(&trigger, &background, PACING, || run_once(&config, &http, &handshake)).await;
}

#[cfg(test)]
#[allow(clippy::indexing_slicing)]
mod tests {
    use super::*;
    use sylvode_prereq::Component;

    #[test]
    fn the_report_holds_names_statuses_and_versions_only() {
        let mut tmux = Component::new("tmux", true, Status::Info, true)
            .with_hint("install tmux >= 3.2: brew install tmux")
            .with_detail("socket", json!("sylvode"));
        tmux.version = Some("3.7b".to_string());
        tmux.path = Some("/opt/homebrew/bin/tmux".to_string());
        let warn = Component::new("cli.claude", true, Status::Warn, false);
        let fail = Component::new("mcp_server", false, Status::Fail, true).with_hint("secret hint");
        let body = report_body(
            &[tmux, warn, fail],
            &[],
            &[(
                "claude",
                crate::runner::session_report::Posture {
                    tmux: false,
                    headless: false,
                },
            )],
        );
        assert_eq!(
            body["components"],
            json!([
                {"component": "tmux", "status": "ok", "version": "3.7b"},
                {"component": "cli.claude", "status": "warn"},
                {"component": "mcp_server", "status": "fail"},
                {"component": "posture", "cli": "claude", "status": "ok", "bypass": {"tmux": false, "headless": false}},
            ])
        );
        assert_eq!(
            body["generated_by"],
            json!({"version": env!("CARGO_PKG_VERSION"), "task_protocol": 1})
        );
        let text = body.to_string();
        for leaked in ["/opt", "hint", "detail", "path", "socket", "install"] {
            assert!(!text.contains(leaked), "{leaked}: {text}");
        }
        // Small enough for the API's 16 KiB limit with room to spare.
        assert!(text.len() < 1024);
    }

    /// The rows the API's sanitiser keeps (`bot_connection_status.rs`): `posture` with a boolean per
    /// channel, and `tmux_session` keyed by project key and CLI with five fixed-vocabulary fields.
    #[test]
    fn the_posture_and_session_rows_have_exactly_the_shape_the_api_keeps() {
        use crate::runner::files::CreatedBy;
        use crate::runner::session_report::{Assessment, Mcp, Permissions, Posture, SessionState};
        use crate::runner::tmux::{NameSource, Slot, TmuxName};
        let name = Slot::project("QLA", "claude").expect("name");
        let running = Assessment {
            tmux: TmuxName::new("lead").expect("tmux name"),
            source: NameSource::Local,
            name,
            bot_id: "b".to_string(),
            state: SessionState::Running,
            created_by: Some(CreatedBy::Operator),
            hooks_seen: None,
            hooks: Some(true),
            permissions: Some(Permissions::Bypass),
            mcp: Some(Mcp::Stale),
            pane_id: Some("%3".to_string()),
            foreground: Some("claude".to_string()),
            status: Status::Warn,
        };
        let absent = Assessment {
            tmux: TmuxName::derived(&running.name).expect("derived"),
            source: NameSource::Derived,
            state: SessionState::Absent,
            created_by: None,
            hooks: None,
            permissions: None,
            mcp: None,
            pane_id: None,
            foreground: None,
            status: Status::Ok,
            ..running.clone()
        };
        let bot_row = Assessment {
            name: Slot::bot("8c1f0a52-3d4e-4b6a-9c7d-1e2f3a4b5c6d", "claude").expect("bot slot"),
            tmux: TmuxName::new("sv").expect("tmux name"),
            source: NameSource::Local,
            state: SessionState::Absent,
            created_by: None,
            hooks: None,
            permissions: None,
            mcp: None,
            pane_id: None,
            foreground: None,
            status: Status::Ok,
            ..running.clone()
        };
        let body = report_body(
            &[],
            &[running, absent, bot_row],
            &[
                (
                    "claude",
                    Posture {
                        tmux: true,
                        headless: false,
                    },
                ),
                (
                    "codex",
                    Posture {
                        tmux: false,
                        headless: false,
                    },
                ),
            ],
        );
        assert_eq!(
            body["components"],
            json!([
                {"component": "posture", "cli": "claude", "status": "warn", "bypass": {"tmux": true, "headless": false}},
                {"component": "posture", "cli": "codex", "status": "ok", "bypass": {"tmux": false, "headless": false}},
                {"component": "tmux_session", "scope": "project", "project_key": "QLA", "cli": "claude",
                 "status": "warn", "state": "running", "session_source": "local", "created_by": "operator",
                 "hooks": "seen", "permissions": "bypass", "mcp": "stale"},
                {"component": "tmux_session", "scope": "project", "project_key": "QLA", "cli": "claude",
                 "status": "ok", "state": "absent", "session_source": "derived"},
                {"component": "tmux_session", "scope": "bot", "member": 0, "cli": "claude", "status": "ok",
                 "state": "absent", "session_source": "local"},
            ])
        );
        // No session name, pane id or foreground command leaves the process.
        let text = body.to_string();
        for leaked in [
            "QLA-claude",
            "lead",
            "%3",
            "pane",
            "foreground",
            "\"b\"",
            "bot-",
            "8c1f0a52",
            "\"sv\"",
        ] {
            assert!(!text.contains(leaked), "{leaked}: {text}");
        }
    }

    /// Counts the rounds of a loop that is asked often, to show the rate rule: a burst of requests is
    /// one round after the debounce, and rounds are never closer than the minimum gap.
    #[tokio::test]
    async fn a_trigger_runs_a_round_within_the_debounce_and_never_faster_than_the_gap() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        let pacing = Pacing {
            debounce: Duration::from_millis(30),
            min_gap: Duration::from_millis(300),
        };
        let background = crate::background::Background::new(1);
        let trigger = ReportTrigger::default();
        let starts: Arc<Mutex<Vec<tokio::time::Instant>>> = Arc::new(Mutex::new(Vec::new()));
        let rounds = Arc::new(AtomicUsize::new(0));
        let task = {
            let (trigger, background, starts, rounds) = (
                trigger.clone(),
                background.clone(),
                Arc::clone(&starts),
                Arc::clone(&rounds),
            );
            tokio::spawn(async move {
                drive(&trigger, &background, pacing, || {
                    starts.lock().push(tokio::time::Instant::now());
                    rounds.fetch_add(1, Ordering::SeqCst);
                    // A periodic interval far longer than the test.
                    std::future::ready(Duration::from_hours(1))
                })
                .await;
            })
        };
        // The first round runs at once.
        tokio::time::sleep(Duration::from_millis(20)).await;
        assert_eq!(rounds.load(Ordering::SeqCst), 1);
        // A request right after it still waits for the minimum gap since that round started.
        trigger.request();
        tokio::time::sleep(Duration::from_millis(150)).await;
        assert_eq!(rounds.load(Ordering::SeqCst), 1, "not before the gap");
        tokio::time::sleep(Duration::from_millis(250)).await;
        assert_eq!(rounds.load(Ordering::SeqCst), 2, "one round for the request");
        // A burst of fifty requests is one round, after the gap.
        for _ in 0..50 {
            trigger.request();
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
        assert_eq!(rounds.load(Ordering::SeqCst), 3, "a burst is one round");
        // Nothing more runs without a request.
        tokio::time::sleep(Duration::from_millis(400)).await;
        assert_eq!(rounds.load(Ordering::SeqCst), 3);
        let times = starts.lock().clone();
        for pair in times.windows(2) {
            assert!(
                pair[1].duration_since(pair[0]) >= pacing.min_gap,
                "rounds are at least the gap apart"
            );
        }
        background.begin_shutdown();
        let _ = tokio::time::timeout(Duration::from_secs(2), task).await;
    }

    /// A request is honoured within the debounce when the previous round is old enough, and a
    /// shutdown ends the loop even while it waits.
    #[tokio::test]
    async fn a_request_after_a_quiet_period_runs_after_the_debounce_and_shutdown_ends_the_loop() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        let pacing = Pacing {
            debounce: Duration::from_millis(40),
            min_gap: Duration::from_millis(100),
        };
        let background = crate::background::Background::new(1);
        let trigger = ReportTrigger::default();
        let rounds = Arc::new(AtomicUsize::new(0));
        let task = {
            let (trigger, background, rounds) = (trigger.clone(), background.clone(), Arc::clone(&rounds));
            tokio::spawn(async move {
                drive(&trigger, &background, pacing, || {
                    rounds.fetch_add(1, Ordering::SeqCst);
                    std::future::ready(Duration::from_hours(1))
                })
                .await;
            })
        };
        tokio::time::sleep(Duration::from_millis(250)).await;
        assert_eq!(rounds.load(Ordering::SeqCst), 1);
        trigger.request();
        tokio::time::sleep(Duration::from_millis(20)).await;
        assert_eq!(rounds.load(Ordering::SeqCst), 1, "waits for the debounce");
        tokio::time::sleep(Duration::from_millis(60)).await;
        assert_eq!(rounds.load(Ordering::SeqCst), 2, "runs after it");
        trigger.request();
        background.begin_shutdown();
        assert!(
            tokio::time::timeout(Duration::from_secs(2), task).await.is_ok(),
            "the loop ends on shutdown"
        );
    }

    #[test]
    fn a_mismatching_api_is_a_failing_row_and_a_matching_one_is_ok() {
        assert_eq!(api_row(1, Some("0.5.0")).status, Status::Ok);
        assert_eq!(api_row(2, Some("0.6.0")).status, Status::Fail);
        assert_eq!(api_row(0, None).status, Status::Fail);
    }

    const BOT: &str = "00000000-0000-4000-8000-000000000001";

    fn config(agents: &str) -> Config {
        Config::from_toml(&format!(
            "[server]\nlisten = \"127.0.0.1:1\"\n[security]\nallow_unsigned = true\n\
             [ai_task_callback]\nenabled = true\napi_url = \"http://127.0.0.1:2\"\n\
             [[ai_task_callback.bots]]\nbot_id = \"{BOT}\"\nworkspace_id = \"11111111-1111-4111-8111-111111111111\"\ntoken = \"opr_hshshshshshshsh\"\nsurface = \"mcp_stdio\"\n\
             [runner]\nallowed_roots = [\"/x\"]\n{agents}"
        ))
        .expect("config")
    }

    #[test]
    fn the_handshake_speaks_for_the_bots_of_runner_agents_only() {
        let cfg = config(&format!(
            "[[agents]]\nid = \"{BOT}\"\nname = \"r\"\nagent_type = \"runner\"\n[agents.route]\nbot_ids = [\"{BOT}\"]\n\
             [[agents]]\nid = \"c\"\nname = \"c\"\nagent_type = \"custom\"\n[agents.custom]\ncommand = \"echo\"\n"
        ));
        let found = targets(&cfg);
        assert_eq!(found.len(), 1);
        assert_eq!(
            (found[0].bot_id.as_str(), found[0].surface.as_str()),
            (BOT, "mcp_stdio")
        );
        assert_eq!(found[0].api_url, "http://127.0.0.1:2");
        assert!(!format!("{:?}", found[0]).contains("opr_"));
        let none = config(
            "[[agents]]\nid = \"c\"\nname = \"c\"\nagent_type = \"custom\"\n[agents.custom]\ncommand = \"echo\"\n",
        );
        assert!(targets(&none).is_empty());
    }

    #[test]
    fn health_shows_the_state_and_the_mismatch_text_names_both_protocols() {
        let handshake = Handshake::new(true);
        assert_eq!(
            handshake.health(),
            json!({"runner": 1, "api": null, "state": "pending"})
        );
        assert_eq!(handshake.mismatch_text(), None);
        {
            let mut status = handshake.status.lock();
            status.state = ProtocolState::Mismatch;
            status.api = Some(2);
        }
        assert_eq!(
            handshake.mismatch_text().as_deref(),
            Some("task protocol mismatch (runner 1, api 2); upgrade the older component")
        );
        assert_eq!(Handshake::new(false).health()["state"], "unchecked");
    }

    /// Whatever the server advertises, the report interval stays between a minute and an hour; an
    /// operator's own setting is taken as written.
    #[tokio::test]
    async fn the_advertised_report_interval_is_clamped() {
        use axum::{Router, routing::get};
        for (advertised, want) in [(1_u64, 60_u64), (59, 60), (600, 600), (99_999, 3600), (0, 60)] {
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.expect("bind");
            let addr = listener.local_addr().expect("addr");
            let app = Router::new().route(
                "/api/v1/ai/protocol",
                get(move || async move {
                    json!({"code": 0, "data": {"task_protocol": 1, "api_version": "0.5.0", "runner_report_interval_secs": advertised}})
                        .to_string()
                }),
            );
            tokio::spawn(async move {
                let _ = axum::serve(listener, app).await;
            });
            let cfg = config(&format!(
                "[[agents]]\nid = \"{BOT}\"\nname = \"r\"\nagent_type = \"runner\"\n[agents.route]\nbot_ids = [\"{BOT}\"]\n"
            ));
            let mut cfg = cfg;
            cfg.ai_task_callback.api_url = Some(format!("http://{addr}"));
            let wait = run_once(&cfg, &reqwest::Client::new(), &Handshake::new(true)).await;
            assert_eq!(wait, Duration::from_secs(want), "advertised {advertised}");
            cfg.runner.report_interval_secs = Some(2);
            let wait = run_once(&cfg, &reqwest::Client::new(), &Handshake::new(true)).await;
            assert_eq!(wait, Duration::from_secs(2), "an operator's setting is kept");
        }
    }
}
