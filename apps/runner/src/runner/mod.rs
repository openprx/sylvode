//! The `runner` agent: a delivery for a bot connected to a project is checked, then run on the
//! project's channel. This module holds the checks, the allowed roots, the Sylvode client, the
//! headless channel, and the pieces of the tmux channel (driver, messages, guide installer) that
//! the session manager will use.

use std::path::PathBuf;
use std::sync::Arc;

use axum::http::StatusCode;
use serde_json::{Value, json};

use self::checks::{Context, SessionRegistry, Verdict};
use self::paths::AllowedRoots;
use self::sv_client::SvClient;
use crate::AppState;
use crate::config::{AgentConfig, AiTaskCallback, Config};

pub mod checks;
pub mod config;
#[allow(dead_code, reason = "used by the session manager, which is the next package")]
pub mod files;
pub mod headless;
#[allow(dead_code, reason = "used by the session manager, which is the next package")]
pub mod launch;
pub mod paths;
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
mod launch_tests;
#[cfg(test)]
mod message_tests;
#[cfg(test)]
mod paths_tests;
#[cfg(test)]
mod sanitize_tests;
#[cfg(test)]
mod sv_client_tests;

/// What the `runner` agents need while the service runs, built once at start-up.
pub struct RunnerRuntime {
    /// The validated roots, or why they are not valid (which refuses the `runner` agents only).
    pub roots: Result<AllowedRoots, String>,
    pub registry: SessionRegistry,
    /// Shared by every client: no redirects, so a bearer token never follows one.
    pub http: reqwest::Client,
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
        let home = std::env::var_os("HOME").map(PathBuf::from);
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
        let http = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .unwrap_or_default();
        Self {
            roots,
            registry: SessionRegistry::open(state_dir(config, home.as_deref()).as_deref()),
            http,
        }
    }
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
    let AiTaskCallback::Report(target) = config.ai_task_callback_for(Some(agent), bot) else {
        return Ok(refused(
            agent,
            "runner_agent_disabled",
            "this runner has no usable token for the task's bot, or safe mode is on",
        ));
    };
    let client = SvClient::new(state.runner.http.clone(), target.api_url, target.token, target.surface);
    let verdict = checks::check_delivery(
        Context {
            config,
            roots: &state.runner.roots,
            registry: &state.runner.registry,
            protocol_mismatch: state.handshake.mismatch_text(),
            client,
        },
        payload,
    )
    .await;
    match verdict {
        Verdict::Unavailable => Err(StatusCode::SERVICE_UNAVAILABLE),
        Verdict::Ignored(reason) => Ok(json!({"status": "ignored", "reason": reason, "agent": agent.id})),
        Verdict::Refused(refusal) => Ok(refused(agent, refusal.code, &refusal.reason)),
        Verdict::Run(accepted) => {
            let key = accepted.project_key.clone();
            headless::start(config, &state.background, *accepted);
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
