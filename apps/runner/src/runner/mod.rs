//! The `runner` agent: a delivery for a bot connected to a project is checked, then run on the
//! project's channel. This module holds the checks, the allowed roots, the Sylvode client, the
//! headless channel, and the pieces of the tmux channel (driver, messages, guide installer) that
//! the session manager will use.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;

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
pub mod naming;
pub mod paths;
pub mod profile;
#[allow(dead_code, reason = "used by the session worker")]
pub mod queue;
pub mod recovery;
#[allow(dead_code, reason = "release, entries and owner are used by the sessions command")]
pub mod registry;
#[allow(dead_code, reason = "wired in by the application")]
pub mod session;
pub mod session_report;
pub mod setup;
pub mod sv_client;
pub mod timing;

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
mod config_tests;
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
mod naming_tests;
#[cfg(test)]
mod paths_tests;
#[cfg(test)]
mod profile_tests;
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
mod session_report_tests;
#[cfg(test)]
mod session_tests;
#[cfg(test)]
mod setup_tests;
#[cfg(test)]
mod sv_client_tests;
#[cfg(test)]
mod timing_tests;

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
        let http = build_http_client(&config.runner.http);
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
        // The configuration file holds every token and secret: no root may contain it, or a
        // project directory could be set over it.
        if let Some(file) = &config.config_path {
            let clash = |roots: &AllowedRoots| roots.contains(file);
            let message =
                "the configuration file must be outside every allowed root (it holds the bots' tokens)".to_string();
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

/// Whether command words turn the permission checks of the CLI off: `--dangerously-skip-permissions`
/// or `--permission-mode bypassPermissions` (also written `--permission-mode=bypassPermissions`).
pub fn words_bypass_permissions(words: &[String]) -> bool {
    words.iter().enumerate().any(|(at, word)| {
        word == "--dangerously-skip-permissions"
            || word == "--permission-mode=bypassPermissions"
            || (word == "--permission-mode" && words.get(at + 1).is_some_and(|next| next == "bypassPermissions"))
    })
}

/// The posture of every CLI the `runner` agents can start: one line each, and a warning for any
/// command that turns permission checks off (the default does) and for opencode (whose permissions
/// the runner does not control).
pub fn posture(config: &Config) -> Vec<PostureLine> {
    let runner = &config.runner;
    let claude = &runner.cli.claude;
    let launch_bypass = words_bypass_permissions(&claude.launch_command);
    let headless_bypass = words_bypass_permissions(&claude.headless_args);
    let mut lines = vec![PostureLine {
        warn: launch_bypass || headless_bypass,
        text: format!(
            "claude: session launch command {:?}, headless arguments {:?}; the CLI runs as this user with this user's \
             environment and login{}",
            claude.launch_command,
            claude.headless_args,
            if launch_bypass || headless_bypass {
                "; PERMISSION CHECKS ARE OFF"
            } else {
                ""
            }
        ),
    }];
    let codex = &runner.cli.codex;
    let codex_tmux_bypass = session_report::codex_bypasses(&codex.launch_command);
    let codex_headless_bypass = session_report::codex_bypasses(&codex.headless_args);
    lines.push(PostureLine {
        warn: codex.enabled && (codex_tmux_bypass || codex_headless_bypass),
        text: format!(
            "codex: {}, session launch command {:?}, headless arguments {:?}{}",
            if codex.enabled { "enabled" } else { "disabled" },
            codex.launch_command,
            codex.headless_args,
            if codex.enabled && (codex_tmux_bypass || codex_headless_bypass) {
                "; APPROVALS AND SANDBOX ARE OFF"
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

/// Writes the MCP files of every bot of the `runner` agents at start-up (design I2), so an operator
/// can start a session with them before the first task. A failure is logged; every delivery
/// writes them again.
pub fn write_bot_files_at_start(state: &AppState) {
    use sylvode_prereq::{Checker, InstallKind};
    let (Ok(files), config) = (&state.runner.state, &state.config) else {
        return;
    };
    let Some(mcp_server) = Checker::new(InstallKind::RunnerHeadless).locate(&config.runner.mcp.server_binary) else {
        tracing::warn!("the MCP server was not found: the bots' MCP files are written when a task arrives");
        return;
    };
    for target in crate::handshake::targets(config) {
        let bot = BotCreds {
            bot_id: target.bot_id.clone(),
            api_url: target.api_url,
            workspace_id: target.workspace_id,
            token: target.token,
        };
        if let Err(e) = files.write_bot_files(&bot.mcp_files(&config.runner), &mcp_server) {
            tracing::warn!("could not write the MCP files of bot {}: {e}", bot.bot_id);
        }
    }
}

/// The one HTTP client of the runner agents: no redirect is ever followed.
/// The pool settings come from `[runner.http]`; at their defaults the client is the library's own.
pub fn build_http_client(settings: &config::RunnerHttpConfig) -> Result<reqwest::Client, String> {
    let mut builder = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .pool_idle_timeout(std::time::Duration::from_secs(settings.pool_idle_timeout_secs));
    if settings.pool_max_idle_per_host > 0 {
        builder = builder.pool_max_idle_per_host(settings.pool_max_idle_per_host as usize);
    }
    if settings.tcp_keepalive_secs > 0 {
        builder = builder.tcp_keepalive(std::time::Duration::from_secs(settings.tcp_keepalive_secs));
    }
    if settings.connect_timeout_secs > 0 {
        builder = builder.connect_timeout(std::time::Duration::from_secs(settings.connect_timeout_secs));
    }
    builder
        .build()
        .map_err(|e| format!("the HTTP client for Sylvode could not be built: {e}"))
}

/// Installs the agent guide into the project directory (design 8.5) when the operator allows it
/// and the directory passed the allowed roots (a bot's session never uses a directory, and a
/// directory that failed the roots is not written to). A failure or conflict is logged; the task
/// runs either way, because the server instructions carry the procedure too.
async fn install_guide(config: &Config, files: &Arc<files::StateDir>, accepted: &checks::Accepted) {
    if !config.runner.install_agent_guide || !accepted.dir_checked {
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

/// Handles a delivery to a `runner` agent: the synchronous checks, then the channel. `received` is
/// when the delivery arrived; it only feeds the `delivery answered` timing log.
///
/// # Errors
///
/// HTTP 503 when Sylvode cannot answer in time, so the worker re-queues the task.
pub async fn accept(
    state: &Arc<AppState>,
    agent: &AgentConfig,
    payload: &Value,
    received: Instant,
) -> Result<Value, StatusCode> {
    let mut times = timing::DeliveryTimes::default();
    let result = decide(state, agent, payload, &mut times).await;
    times.total_ms = timing::millis(received.elapsed());
    timing::delivery_answered(payload, result.as_ref().ok(), &times);
    result
}

async fn decide(
    state: &Arc<AppState>,
    agent: &AgentConfig,
    payload: &Value,
    times: &mut timing::DeliveryTimes,
) -> Result<Value, StatusCode> {
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
    let Some(bot_creds) = BotCreds::for_target(bot, &target) else {
        return Ok(refused(
            agent,
            "runner_agent_disabled",
            "this runner has no workspace id for the task's bot",
        ));
    };
    let client = SvClient::new(http.clone(), target.api_url, target.token, target.surface);
    let Ok(registry) = &state.runner.registry else {
        return Ok(refused(
            agent,
            "runner_agent_disabled",
            "the runner's session registry cannot be used",
        ));
    };
    let check_started = Instant::now();
    let verdict = checks::check_delivery(
        Context {
            config,
            roots: state.runner.roots_for(agent),
            registry,
            state: state.runner.state.as_deref().ok(),
            protocol_mismatch: state.handshake.mismatch_text(),
            client,
            bot: bot_creds,
        },
        payload,
    )
    .await;
    times.check_ms = timing::millis(check_started.elapsed());
    match verdict {
        Verdict::Unavailable => Err(StatusCode::SERVICE_UNAVAILABLE),
        Verdict::Ignored(reason) => Ok(json!({"status": "ignored", "reason": reason, "agent": agent.id})),
        Verdict::Refused(refusal) => Ok(refused(agent, refusal.code, &refusal.reason)),
        Verdict::Run(accepted) if accepted.uses_tmux() => {
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
            let session = accepted.tmux.as_str().to_string();
            times.slot = accepted.slot.as_str().to_string();
            let enqueue_started = Instant::now();
            let queued = sessions.enqueue(session::TmuxTask::from_accepted(&accepted)).await;
            times.enqueue_ms = timing::millis(enqueue_started.elapsed());
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
                        "session": session,
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
            times.slot = accepted.slot.as_str().to_string();
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
