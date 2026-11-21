//! The headless channel: the CLI runs once, in the canonical working directory, and ends when its
//! process ends.
//!
//! A run gets only what the runner gives it (design 6.1, 6.6):
//! - the argument vector of `launch.rs`: the runner's own per-bot MCP file with strict MCP config,
//!   an explicit allow list, and for claude `--setting-sources user` so committed project settings
//!   (allow rules, hooks, MCP servers) are not loaded;
//! - the allow-listed environment of the tmux channel (`tmux::allowed_environment`) plus the
//!   variables the operator names in `[runner.cli.*] env_passthrough`, nothing else;
//! - a directory that is checked again after any wait for a run slot, immediately before the
//!   process starts. The process is started in the canonical path; handing it a verified
//!   descriptor would need an `unsafe` `fchdir` between fork and exec, which this crate forbids,
//!   so the path is re-canonicalised and compared instead. A swap after that check, by a process of
//!   the runner's own user, remains possible (design 6.2).
//!
//! The prompt is the task message of design section 4.9. When the process ends and Sylvode still
//! shows the task active, the runner fails it (`source: "runner"`): not retryable when the agent
//! ended on its own, retryable when the runner cut the run short (shutdown, time limit, no free
//! slot).

use std::path::{Path, PathBuf};
use std::time::Duration;

use super::checks::{Accepted, Refusal};
use super::config::RunnerConfig;
use super::files::{BotFiles, StateDir};
use super::launch;
use super::paths::AllowedRoots;
use crate::config::Config;
use crate::executor_permissions::ExecutorSettings;
use crate::process::{self, End};

/// How long one headless run may take.
pub const RUN_TIMEOUT_SECS: u64 = 3600;

/// How long a task waits for a free run slot before it is failed (retryably).
pub const SLOT_WAIT: Duration = Duration::from_mins(30);

/// The text the task is failed with when the agent ended without ending it.
pub const UNFINISHED_REASON: &str = "agent ended without completing or failing the task";

/// The text the task is failed with when the runner cut the run short.
pub const CUT_SHORT_REASON: &str = "the runner stopped the run before the agent ended the task";

/// The settings of `cli` as the permission validation takes them. The launch code validates its
/// permissions through the same settings, so every channel refuses the same combinations.
pub fn settings_for(runner: &RunnerConfig, cli: &str) -> ExecutorSettings {
    match cli {
        "claude" => {
            let claude = &runner.cli.claude;
            let mut cfg = ExecutorSettings::new("claude-code");
            cfg.permission_mode = Some(claude.permission_mode.clone());
            cfg.dangerously_bypass_permissions = claude.dangerously_bypass_permissions;
            cfg
        }
        "codex" => {
            let codex = &runner.cli.codex;
            let mut cfg = ExecutorSettings::new("codex");
            cfg.sandbox = Some(codex.sandbox.clone());
            cfg.dangerously_bypass_permissions = codex.dangerously_bypass_permissions;
            cfg
        }
        _ => ExecutorSettings::new("opencode"),
    }
}

/// What a run needs, decided before the delivery is answered.
#[derive(Debug)]
pub struct Prepared {
    pub program: PathBuf,
    pub args: Vec<String>,
    /// The operator's pass-through variable names for this CLI.
    pub passthrough: Vec<String>,
    /// Variables the runner sets itself (`CLAUDE_CONFIG_DIR`).
    pub set_env: Vec<(String, String)>,
}

fn refusal(code: &'static str, reason: impl Into<String>) -> Refusal {
    Refusal {
        code,
        reason: reason.into(),
    }
}

/// Whether the project directory holds configuration of the CLI that the runner cannot switch off
/// (codex, opencode) and that could widen what the run may do.
fn has_project_cli_files(cli: &str, dir: &Path) -> bool {
    let names: &[&str] = match cli {
        "codex" => &[".codex", "AGENTS.md"],
        "opencode" => &["opencode.json", "opencode.jsonc", ".opencode"],
        _ => &[],
    };
    // AGENTS.md is guidance, not a permission source; only the configuration directories count.
    names
        .iter()
        .filter(|name| **name != "AGENTS.md")
        .any(|name| std::fs::symlink_metadata(dir.join(name)).is_ok())
}

/// Writes the per-bot files and builds the argument vector.
///
/// # Errors
///
/// The refusal for the delivery.
pub fn prepare(config: &Config, state: &StateDir, accepted: &Accepted, mcp_server: &Path) -> Result<Prepared, Refusal> {
    let runner = &config.runner;
    let bot = &accepted.bot;
    if matches!(accepted.cli.as_str(), "codex" | "opencode")
        && has_project_cli_files(&accepted.cli, &accepted.dir)
        && !runner.trust_project_files.contains(&accepted.project_key)
    {
        return Err(refusal(
            "workspace_untrusted",
            format!(
                "the project directory has {} configuration that the runner cannot switch off; the operator must \
                 add the project to runner.trust_project_files",
                accepted.cli
            ),
        ));
    }
    let allow_write = accepted.cli == "claude" && runner.cli.claude.allow_write_call;
    state
        .write_bot_files(
            &BotFiles {
                bot_id: &bot.bot_id,
                api_url: &bot.api_url,
                token: &bot.token,
                tool_mode: &runner.mcp.tool_mode,
                allow_write_call: allow_write,
            },
            mcp_server,
        )
        .map_err(|e| {
            tracing::error!("could not write the MCP files of bot {}: {e}", bot.bot_id);
            refusal(
                "runner_agent_disabled",
                "the runner could not prepare the MCP configuration",
            )
        })?;
    let toml = state.mcp_toml(&bot.bot_id);
    let claude_json = state.claude_mcp_json(&bot.bot_id);
    let prompt = &accepted.message;
    let (args, passthrough) = match accepted.cli.as_str() {
        "claude" => (
            launch::claude_headless_argv(
                &accepted.program,
                &runner.cli.claude,
                &accepted.dir,
                &claude_json,
                &super::launch::Protected {
                    state_dir: state.root(),
                    config_file: config.config_path.as_deref(),
                },
                prompt,
            ),
            runner.cli.claude.env_passthrough.clone(),
        ),
        "codex" => (
            launch::codex_headless_argv(
                &accepted.program,
                &runner.cli.codex,
                &accepted.dir,
                mcp_server,
                &toml,
                prompt,
            ),
            runner.cli.codex.env_passthrough.clone(),
        ),
        _ => (
            launch::opencode_headless_argv(&accepted.program, prompt),
            runner.cli.opencode.env_passthrough.clone(),
        ),
    };
    let args = args.map_err(|e| match e {
        launch::LaunchError::Unsafe(why) => refusal("path_outside_allowlist", why),
        launch::LaunchError::Permissions(why) => {
            tracing::error!("headless permissions refused: {why}");
            refusal(
                "runner_agent_disabled",
                "the runner's permission settings for this CLI are not valid",
            )
        }
    })?;
    let mut set_env = Vec::new();
    if accepted.cli == "claude" {
        let dir = state.ensure_claude_config_dir(&accepted.bot.bot_id).map_err(|e| {
            tracing::error!("could not prepare the claude configuration directory: {e}");
            refusal(
                "runner_agent_disabled",
                "the runner could not prepare the CLI's configuration directory",
            )
        })?;
        set_env.push(("CLAUDE_CONFIG_DIR".to_string(), dir.to_string_lossy().into_owned()));
    }
    Ok(Prepared {
        program: accepted.program.clone(),
        args,
        passthrough,
        set_env,
    })
}

/// How a run ended.
#[derive(Debug, PartialEq, Eq)]
pub struct RunEnd {
    /// `success`, `failed`, `timeout` or `cancelled`.
    pub status: &'static str,
    pub exit_code: Option<i32>,
}

/// Starts the CLI in `dir` with the allow-listed environment and waits for it.
pub async fn execute(
    prepared: &Prepared,
    dir: &Path,
    timeout: Duration,
    cancel: impl std::future::Future<Output = ()>,
) -> RunEnd {
    let Some((_, args)) = prepared.args.split_first() else {
        return RunEnd {
            status: "failed",
            exit_code: None,
        };
    };
    let mut command = tokio::process::Command::new(&prepared.program);
    command.args(args).current_dir(dir).env_clear();
    for (name, value) in super::tmux::allowed_environment(&prepared.passthrough, false) {
        command.env(name, value);
    }
    for (name, value) in &prepared.set_env {
        command.env(name, value);
    }
    match process::run(command, timeout, process::TERMINATION_GRACE, cancel).await {
        Ok(finished) => match finished.end {
            End::Exited(status) if status.success() => RunEnd {
                status: "success",
                exit_code: status.code(),
            },
            End::Exited(status) => RunEnd {
                status: "failed",
                exit_code: status.code(),
            },
            End::TimedOut => RunEnd {
                status: "timeout",
                exit_code: None,
            },
            End::Cancelled => RunEnd {
                status: "cancelled",
                exit_code: None,
            },
            End::WaitFailed(_) => RunEnd {
                status: "failed",
                exit_code: None,
            },
        },
        Err(e) => {
            tracing::error!("could not start the CLI: {e}");
            RunEnd {
                status: "failed",
                exit_code: None,
            }
        }
    }
}

/// Starts the run in the background: a slot of `cli_max_concurrency` (waited for at most
/// [`SLOT_WAIT`]), a second check of the directory, a progress report, the CLI, heartbeats while it
/// runs, and the failure of a task the agent left open.
pub fn start(
    config: &Config,
    background: &crate::background::Background,
    roots: AllowedRoots,
    prepared: Prepared,
    accepted: Accepted,
) {
    let timeout = Duration::from_secs(config.runner.delivery_check_timeout_secs);
    let heartbeat = Duration::from_secs(config.runner.heartbeat_secs.max(1));
    let task_background = background.clone();
    background.spawn(async move {
        let fail = |reason: &'static str, retryable: bool| {
            let client = accepted.client.clone();
            let (id, attempt) = (accepted.task_id.clone(), accepted.attempt);
            async move {
                if let Err(e) = client.fail(&id, attempt, reason, retryable, timeout).await {
                    tracing::warn!("could not fail the task: {e}");
                }
            }
        };
        let permit = match tokio::time::timeout(SLOT_WAIT, task_background.cli_permit()).await {
            Ok(Some(permit)) => permit,
            Ok(None) => return,
            Err(_) => {
                fail("no run slot became free in time", true).await;
                return;
            }
        };
        // The directory is checked again now: the wait for a slot can be long, and the path is
        // resolved by name when the process starts.
        let unchanged = accepted
            .dir
            .to_str()
            .and_then(|text| roots.check_directory(text).ok())
            .is_some_and(|again| again == accepted.dir);
        if !unchanged {
            tracing::warn!("the working directory of a task changed before its run started");
            drop(permit);
            fail(
                "the project working directory changed after the task was accepted",
                false,
            )
            .await;
            return;
        }
        let _ = accepted
            .client
            .progress(
                &accepted.task_id,
                accepted.attempt,
                "runner.pushed",
                "started a headless run",
                None,
                timeout,
            )
            .await;
        let run = execute(
            &prepared,
            &accepted.dir,
            Duration::from_secs(RUN_TIMEOUT_SECS),
            task_background.cancelled(),
        );
        tokio::pin!(run);
        let mut ticker = tokio::time::interval_at(tokio::time::Instant::now() + heartbeat, heartbeat);
        let result = loop {
            tokio::select! {
                result = &mut run => break result,
                _ = ticker.tick() => {
                    let _ = accepted.client.progress(
                        &accepted.task_id, accepted.attempt, "runner.alive", "the CLI is still running", None, timeout,
                    ).await;
                }
            }
        };
        drop(permit);
        tracing::info!(status = result.status, exit_code = ?result.exit_code, "headless run ended");
        // When the view cannot be read, ending an already ended task is refused harmlessly.
        let still_active = accepted
            .client
            .task_view(&accepted.task_id, accepted.attempt, timeout)
            .await
            .map_or(true, |view| {
                view.get("active").and_then(serde_json::Value::as_bool) != Some(false)
            });
        if still_active {
            // A run the runner cut short (shutdown, time limit) did not give the agent its say:
            // that may be tried again. An agent that ended on its own is not retried.
            if matches!(result.status, "cancelled" | "timeout") {
                fail(CUT_SHORT_REASON, true).await;
            } else {
                fail(UNFINISHED_REASON, false).await;
            }
        }
    });
}
