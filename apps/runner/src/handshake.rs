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
pub fn report_body(rows: &[Component]) -> Value {
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
    json!({
        "generated_by": {"version": env!("CARGO_PKG_VERSION"), "task_protocol": TASK_PROTOCOL},
        "components": components,
    })
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
        match client.put_report(&report_body(&rows), CALL_TIMEOUT).await {
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

/// The start-up task: a round now and one every interval until shutdown.
pub async fn run_loop(
    config: Arc<Config>,
    http: reqwest::Client,
    handshake: Handshake,
    background: crate::background::Background,
) {
    loop {
        let wait = run_once(&config, &http, &handshake).await;
        tokio::select! {
            () = tokio::time::sleep(wait) => {}
            () = background.cancelled() => return,
        }
    }
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
        let body = report_body(&[tmux, warn, fail]);
        assert_eq!(
            body["components"],
            json!([
                {"component": "tmux", "status": "ok", "version": "3.7b"},
                {"component": "cli.claude", "status": "warn"},
                {"component": "mcp_server", "status": "fail"},
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
