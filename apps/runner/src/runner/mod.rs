//! The `runner` agent: a delivery for a bot connected to a project is checked, then run on the
//! project's channel. This module holds the checks, the allowed roots, the Sylvode client, the
//! headless channel, and the pieces of the tmux channel (driver, messages, guide installer) that
//! the session manager will use.

use std::path::PathBuf;
use std::sync::Arc;

use axum::http::StatusCode;
use serde_json::{Value, json};

use self::checks::{BotCreds, Context, Verdict};
use self::paths::AllowedRoots;
use self::registry::SessionRegistry;
use self::sv_client::SvClient;
use crate::AppState;
use crate::config::{AgentConfig, AiTaskCallback, Config};

pub mod checks;
pub mod config;
#[allow(dead_code, reason = "used by the session worker")]
pub mod detect;
#[allow(dead_code, reason = "used by the session manager, which is the next package")]
pub mod files;
pub mod headless;
#[allow(dead_code, reason = "used by the session worker")]
pub mod hooks;
#[allow(dead_code, reason = "used by the session manager, which is the next package")]
pub mod launch;
#[allow(dead_code, reason = "used by the session worker")]
pub mod machine;
pub mod paths;
#[allow(dead_code, reason = "used by the session worker")]
pub mod queue;
#[allow(dead_code, reason = "release, entries and owner are used by the sessions command")]
pub mod registry;
pub mod sv_client;

#[allow(dead_code, reason = "used by the session manager, which is a later package")]
pub mod guide;
#[allow(dead_code, reason = "used by the session manager, which is a later package")]
pub mod message;
#[allow(dead_code, reason = "used by the message builder and the session manager")]
pub mod sanitize;
#[allow(dead_code, reason = "used by the session manager, which is a later package")]
pub mod tmux;

#[cfg(test)]
mod checks_tests;
#[cfg(test)]
mod files_tests;
#[cfg(test)]
mod guide_tests;
#[cfg(test)]
mod headless_tests;
#[cfg(test)]
mod launch_tests;
#[cfg(test)]
mod machine_tests;
#[cfg(test)]
mod message_tests;
#[cfg(test)]
mod paths_tests;
#[cfg(test)]
mod registry_tests;
#[cfg(test)]
mod sanitize_tests;
#[cfg(test)]
mod sv_client_tests;

/// What the `runner` agents need while the service runs, built once at start-up.
pub struct RunnerRuntime {
    /// The state directory (private, locked), or why it cannot be used.
    pub state: Result<Arc<files::StateDir>, String>,
    /// The validated roots, or why they are not valid (which refuses the `runner` agents only).
    pub roots: Result<AllowedRoots, String>,
    /// The session-name registry, or why it cannot be used (which refuses `runner` agents).
    pub registry: Result<SessionRegistry, String>,
    /// Shared by every client: no redirects, so a bearer token never follows one. There is no
    /// fallback client: when this one cannot be built, `runner` agents do not start.
    pub http: Result<reqwest::Client, String>,
}

/// The state directory: `[runner] state_dir`, else the platform default below the home directory.
pub fn state_dir(config: &Config, home: Option<&std::path::Path>) -> Option<PathBuf> {
    let expand = |text: &str| -> Option<PathBuf> {
        text.strip_prefix("~/")
            .map_or_else(|| Some(PathBuf::from(text)), |rest| home.map(|home| home.join(rest)))
    };
    let default = || {
        home.map(|home| {
            if cfg!(target_os = "macos") {
                home.join("Library/Application Support/sylvode-runner")
            } else {
                home.join(".local/state/sylvode-runner")
            }
        })
    };
    config.runner.state_dir.as_deref().map_or_else(default, expand)
}

impl RunnerRuntime {
    /// Validates the roots and opens the registry. Warnings are logged here.
    pub fn new(config: &Config) -> Self {
        let home = paths::service_user_home();
        let roots = match AllowedRoots::from_entries(&config.runner.allowed_roots, home.as_deref()) {
            Ok(report) => {
                for warning in &report.warnings {
                    tracing::warn!("{warning}");
                }
                Ok(report.roots)
            }
            Err(e) => {
                if config.agents.iter().any(|a| a.agent_type == "runner") {
                    tracing::error!("{e}; `runner` agents will refuse every task");
                }
                Err(e)
            }
        };
        let http = build_http_client();
        let state_path = state_dir(config, home.as_deref());
        let state = state_path.as_deref().map_or_else(
            || Err("no state directory: set [runner] state_dir (no home directory is known)".to_string()),
            |path| files::StateDir::open(path).map(Arc::new),
        );
        if let Err(e) = &state
            && config.agents.iter().any(|a| a.agent_type == "runner")
        {
            tracing::error!("the runner state directory cannot be used: {e}; `runner` agents will refuse every task");
        }
        let registry = SessionRegistry::open(state_path.as_deref());
        if let Err(e) = &registry
            && config.agents.iter().any(|a| a.agent_type == "runner")
        {
            tracing::error!("{e}; `runner` agents will refuse every task");
        }
        Self {
            state,
            roots,
            registry,
            http,
        }
    }
}

/// The one HTTP client of the runner agents: no redirect is ever followed.
pub fn build_http_client() -> Result<reqwest::Client, String> {
    reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .map_err(|e| format!("the HTTP client for Sylvode could not be built: {e}"))
}

fn refused(agent: &AgentConfig, code: &str, reason: &str) -> Value {
    json!({"status": "refused", "reason_code": code, "reason": reason, "agent": agent.id})
}

/// Handles a delivery to a `runner` agent: the synchronous checks, then the channel.
///
/// # Errors
///
/// HTTP 503 when Sylvode cannot answer in time, so the worker re-queues the task.
pub async fn accept(state: &Arc<AppState>, agent: &AgentConfig, payload: &Value) -> Result<Value, StatusCode> {
    let config = &state.config;
    let bot = payload
        .get("ai_participant_id")
        .and_then(Value::as_str)
        .unwrap_or_default();
    // The delivery's bot must be one of the agent's bots (a route by agent type alone could send
    // it another's).
    if !agent.runner_bot_ids().iter().any(|id| crate::config::same_bot(id, bot)) {
        return Ok(refused(
            agent,
            "runner_agent_disabled",
            "this runner agent does not serve the task's bot",
        ));
    }
    let Ok(http) = &state.runner.http else {
        return Ok(refused(
            agent,
            "runner_agent_disabled",
            "the runner's HTTP client is not available",
        ));
    };
    let AiTaskCallback::Report(target) = config.ai_task_callback_for(Some(agent), bot) else {
        return Ok(refused(
            agent,
            "runner_agent_disabled",
            "this runner has no usable token for the task's bot, or safe mode is on",
        ));
    };
    let client = SvClient::new(http.clone(), target.api_url, target.token, target.surface);
    let bot_creds = BotCreds {
        bot_id: bot.to_string(),
        api_url: target.api_url.to_string(),
        token: target.token.to_string(),
    };
    let Ok(registry) = &state.runner.registry else {
        return Ok(refused(
            agent,
            "runner_agent_disabled",
            "the runner's session registry cannot be used",
        ));
    };
    let verdict = checks::check_delivery(
        Context {
            config,
            roots: &state.runner.roots,
            registry,
            protocol_mismatch: state.handshake.mismatch_text(),
            client,
            bot: bot_creds,
        },
        payload,
    )
    .await;
    match verdict {
        Verdict::Unavailable => Err(StatusCode::SERVICE_UNAVAILABLE),
        Verdict::Ignored(reason) => Ok(json!({"status": "ignored", "reason": reason, "agent": agent.id})),
        Verdict::Refused(refusal) => Ok(refused(agent, refusal.code, &refusal.reason)),
        Verdict::Run(accepted) if accepted.channel == checks::Channel::Tmux => {
            let _ = accepted;
            Ok(refused(
                agent,
                "tmux_unavailable",
                "the tmux channel arrives with the session manager",
            ))
        }
        Verdict::Run(accepted) => {
            let (Ok(files), Ok(roots)) = (&state.runner.state, &state.runner.roots) else {
                return Ok(refused(
                    agent,
                    "runner_agent_disabled",
                    "the runner state directory is not usable",
                ));
            };
            let prepared = match headless::prepare(config, files, &accepted, &accepted.mcp_server) {
                Ok(prepared) => prepared,
                Err(refusal) => return Ok(refused(agent, refusal.code, &refusal.reason)),
            };
            let key = accepted.project_key.clone();
            headless::start(config, &state.background, roots.clone(), prepared, *accepted);
            Ok(json!({
                "status": "dispatched",
                "channel": "headless",
                "agent": agent.id,
                "session": key,
                "result": "headless run started",
            }))
        }
    }
}
