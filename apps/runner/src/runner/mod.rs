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
pub mod cli;
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
pub mod recovery;
#[allow(dead_code, reason = "release, entries and owner are used by the sessions command")]
pub mod registry;
#[allow(dead_code, reason = "wired in by the application")]
pub mod session;
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
mod cli_tests;
#[cfg(test)]
mod detect_tests;
#[cfg(test)]
mod files_tests;
#[cfg(test)]
mod guide_tests;
#[cfg(test)]
mod headless_tests;
#[cfg(test)]
mod hooks_tests;
#[cfg(test)]
mod launch_tests;
#[cfg(test)]
mod machine_tests;
#[cfg(test)]
mod message_tests;
#[cfg(test)]
mod paths_tests;
#[cfg(test)]
mod queue_tests;
#[cfg(test)]
mod recovery_tests;
#[cfg(test)]
mod registry_tests;
#[cfg(test)]
mod runtime_tests;
#[cfg(test)]
mod sanitize_tests;
#[cfg(test)]
mod session_tests;
#[cfg(test)]
mod sv_client_tests;

/// What the `runner` agents need while the service runs, built once at start-up.
pub struct RunnerRuntime {
    /// The state directory (private, locked), or why it cannot be used.
    pub state: Result<Arc<files::StateDir>, String>,
    /// The validated roots, or why they are not valid (which refuses the `runner` agents only).
    pub roots: Result<AllowedRoots, String>,
    /// Roots of the `runner` agents that have their own `allowed_roots`, by agent id.
    pub agent_roots: std::collections::HashMap<String, Result<AllowedRoots, String>>,
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
    /// The roots that apply to `agent`: its own `allowed_roots`, else `[runner] allowed_roots`.
    pub fn roots_for(&self, agent: &crate::config::AgentConfig) -> &Result<AllowedRoots, String> {
        self.agent_roots.get(&agent.id).unwrap_or(&self.roots)
    }

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
        // The state directory holds every bot's token: it must not be reachable as a project
        // directory, so it is never inside, equal to, or above a root.
        let mut roots = roots;
        let mut agent_roots: std::collections::HashMap<String, Result<AllowedRoots, String>> =
            std::collections::HashMap::new();
        for agent in config
            .agents
            .iter()
            .filter(|a| a.agent_type == "runner" && !a.allowed_roots.is_empty())
        {
            let own = AllowedRoots::from_entries(&agent.allowed_roots, home.as_deref()).map(|report| {
                for warning in &report.warnings {
                    tracing::warn!("{warning}");
                }
                report.roots
            });
            agent_roots.insert(agent.id.clone(), own);
        }
        if let Some(state_path) = &state_path {
            let canonical = state_path.canonicalize().unwrap_or_else(|_| state_path.clone());
            let clash = |roots: &AllowedRoots| roots.overlaps(&canonical) || roots.overlaps(state_path);
            let message = "runner.state_dir must be outside every allowed root (it holds the bots' tokens)".to_string();
            if roots.as_ref().is_ok_and(&clash) {
                tracing::error!("{message}; `runner` agents will refuse every task");
                roots = Err(message.clone());
            }
            for own in agent_roots.values_mut() {
                if own.as_ref().is_ok_and(&clash) {
                    *own = Err(message.clone());
                }
            }
        }
        // Two agents' roots must not overlap: one agent's tasks must not reach another's tree.
        let ids: Vec<String> = agent_roots.keys().cloned().collect();
        for (n, a) in ids.iter().enumerate() {
            for b in ids.iter().skip(n + 1) {
                let overlap = matches!(
                    (agent_roots.get(a), agent_roots.get(b)),
                    (Some(Ok(x)), Some(Ok(y))) if x.overlaps_roots(y)
                );
                if overlap {
                    let message = "the allowed_roots of two runner agents overlap".to_string();
                    tracing::error!("{message}; both agents will refuse every task");
                    agent_roots.insert(a.clone(), Err(message.clone()));
                    agent_roots.insert(b.clone(), Err(message));
                }
            }
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
            agent_roots,
            registry,
            http,
        }
    }
}

/// One line of the start-up log about what a CLI may do, and whether it is a warning.
#[derive(Debug, PartialEq, Eq)]
pub struct PostureLine {
    pub warn: bool,
    pub text: String,
}

/// The permission posture of every CLI the `runner` agents can start: one line each, and a warning
/// for opencode (whose permissions the runner does not control) and for any setting that turns
/// permission checks off.
pub fn posture(config: &Config) -> Vec<PostureLine> {
    use crate::executor_permissions::ExecutorPermissions;
    let runner = &config.runner;
    let mut lines = Vec::new();
    let claude = &runner.cli.claude;
    let extras = claude.allowed_tools.len();
    let write = if claude.allow_write_call {
        " + sylvode_write"
    } else {
        ""
    };
    let settings = headless::settings_for(runner, "claude");
    let bypass = ExecutorPermissions::from_config(&settings).is_ok_and(|p| p.bypasses_permissions());
    lines.push(PostureLine {
        warn: bypass,
        text: format!(
            "claude: permission_mode = {}, allowed tools = the {} search-mode Sylvode tools{write} + Read/Glob/Grep in the \
             project directory + {extras} configured, disallowed = {}, strict MCP config from the runner's own file, project \
             settings not loaded{}",
            claude.permission_mode,
            launch::SEARCH_MODE_TOOLS.len(),
            claude.disallowed_tools.len(),
            if bypass { "; PERMISSION CHECKS ARE OFF" } else { "" }
        ),
    });
    let codex = &runner.cli.codex;
    let settings = headless::settings_for(runner, "codex");
    let codex_open = ExecutorPermissions::from_config(&settings).is_ok_and(|p| p.bypasses_permissions());
    lines.push(PostureLine {
        warn: codex_open,
        text: format!(
            "codex: {}, sandbox = {}, approvals never{}",
            if codex.enabled { "enabled" } else { "disabled" },
            codex.sandbox,
            if codex_open && codex.enabled {
                "; THE SANDBOX IS OFF"
            } else {
                ""
            }
        ),
    });
    if runner.cli.opencode.enabled {
        lines.push(PostureLine {
            warn: true,
            text: "opencode is enabled: opencode permissions are not controlled by sylvode-runner".to_string(),
        });
    } else {
        lines.push(PostureLine {
            warn: false,
            text: "opencode: disabled".to_string(),
        });
    }
    lines
}

/// The one HTTP client of the runner agents: no redirect is ever followed.
pub fn build_http_client() -> Result<reqwest::Client, String> {
    reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .map_err(|e| format!("the HTTP client for Sylvode could not be built: {e}"))
}

/// Installs the agent guide into the project directory (design 8.5) when the operator allows it.
/// A failure or conflict is logged; the task runs either way, because the server instructions carry
/// the procedure too.
async fn install_guide(config: &Config, files: &Arc<files::StateDir>, accepted: &checks::Accepted) {
    if !config.runner.install_agent_guide {
        return;
    }
    let dir = accepted.dir.clone();
    let cli = accepted.cli.clone();
    let key = accepted.project_key.clone();
    let exclude = config.runner.git_exclude_managed;
    let backups = files.root().join("sessions").join(&key).join("backups");
    let label = format!(
        "t{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_secs())
    );
    let report =
        tokio::task::spawn_blocking(move || guide::install_for_cli(&dir, &cli, exclude, &backups, &label)).await;
    match report {
        Ok(report) => {
            match &report.outcome {
                Ok(guide::Outcome::Conflict | guide::Outcome::NotManaged) => {
                    tracing::warn!(project = %key, "agent_guide: conflict; the guide was not installed");
                }
                Ok(outcome) => tracing::debug!(project = %key, ?outcome, "agent guide"),
                Err(e) => tracing::warn!(project = %key, "the agent guide could not be installed: {e}"),
            }
            if let Some(warning) = report.exclude.as_ref().and_then(guide::ExcludeOutcome::doctor_warning) {
                tracing::warn!(project = %key, "{warning}");
            }
        }
        Err(e) => tracing::warn!("the guide installer stopped: {e}"),
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
    let AiTaskCallback::Report(target) = config.ai_task_callback_for(bot) else {
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
            roots: state.runner.roots_for(agent),
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
            let Some(sessions) = &state.sessions else {
                return Ok(refused(
                    agent,
                    "runner_agent_disabled",
                    "the tmux channel is not available on this runner",
                ));
            };
            if let Ok(files) = &state.runner.state {
                install_guide(config, files, &accepted).await;
            }
            let key = accepted.project_key.clone();
            if let (Ok(files), Ok(session_key)) = (&state.runner.state, tmux::SessionKey::new(&key))
                && let Err(e) = files.set_headless(&session_key, false)
            {
                tracing::warn!("could not clear the headless mark of session {key}: {e}");
            }
            let queued = sessions.enqueue(session::TmuxTask::from_accepted(&accepted)).await;
            Ok(queued.map_or_else(
                || {
                    refused(
                        agent,
                        "runner_agent_disabled",
                        "the session of this project could not take the task",
                    )
                },
                |position| {
                    json!({
                        "status": "dispatched",
                        "channel": "tmux",
                        "agent": agent.id,
                        "session": key,
                        "queue_position": position,
                    })
                },
            ))
        }
        Verdict::Run(accepted) => {
            let (Ok(files), Ok(roots)) = (&state.runner.state, state.runner.roots_for(agent)) else {
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
            install_guide(config, files, &accepted).await;
            let key = accepted.project_key.clone();
            // A session this runner made for the project stays idle from now on: mark it so that
            // `sessions --prune` may remove it.
            if let Ok(session_key) = tmux::SessionKey::new(&key)
                && let Err(e) = files.set_headless(&session_key, true)
            {
                tracing::warn!("could not mark session {key} as headless: {e}");
            }
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
