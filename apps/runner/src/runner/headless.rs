//! The headless channel: the CLI runs once, in the canonical working directory, and ends when its
//! process ends.
//!
//! A run gets (design section 4):
//! - the argument vector of `launch.rs`: the operator's `headless_args`, the runner's own per-bot
//!   MCP file and, for claude, the allow list of the Sylvode tools;
//! - the runner's own environment minus the agent variables (`tmux::agent_environment`): the
//!   operator's variables and login, no forced configuration directory;
//! - a directory that is checked again after any wait for a run slot, immediately before the
//!   process starts. The process is started in the canonical path; handing it a verified
//!   descriptor would need an `unsafe` `fchdir` between fork and exec, which this crate forbids,
//!   so the path is re-canonicalised and compared instead. A swap after that check, by a process of
//!   the runner's own user, remains possible.
//!
//! The prompt is the task message of design section 4.9. When the process ends and Sylvode still
//! shows the task active, the runner fails it (`source: "runner"`): not retryable when the agent
//! ended on its own, retryable when the runner cut the run short (shutdown, time limit, no free
//! slot).

use std::path::{Path, PathBuf};
use std::time::Duration;

use super::checks::{Accepted, Refusal};
use super::files::StateDir;
use super::launch;
use super::paths::AllowedRoots;
use crate::config::Config;
use crate::process::{self, End};

/// How long one headless run may take.
pub const RUN_TIMEOUT_SECS: u64 = 3600;

/// How long a task waits for a free run slot before it is failed (retryably).
pub const SLOT_WAIT: Duration = Duration::from_mins(30);

/// The text the task is failed with when the agent ended without ending it.
pub const UNFINISHED_REASON: &str = "agent ended without completing or failing the task";

/// The text the task is failed with when the runner cut the run short.
pub const CUT_SHORT_REASON: &str = "the runner stopped the run before the agent ended the task";

/// What a run needs, decided before the delivery is answered.
#[derive(Debug)]
pub struct Prepared {
    pub program: PathBuf,
    pub args: Vec<String>,
}

fn refusal(code: &'static str, reason: impl Into<String>) -> Refusal {
    Refusal {
        code,
        reason: reason.into(),
    }
}

/// Writes the per-bot files and builds the argument vector.
///
/// # Errors
///
/// The refusal for the delivery.
pub fn prepare(config: &Config, state: &StateDir, accepted: &Accepted, mcp_server: &Path) -> Result<Prepared, Refusal> {
    let runner = &config.runner;
    let bot = &accepted.bot;
    state.write_bot_files(&bot.mcp_files(runner), mcp_server).map_err(|e| {
        tracing::error!("could not write the MCP files of bot {}: {e}", bot.bot_id);
        refusal(
            "runner_agent_disabled",
            "the runner could not prepare the MCP configuration",
        )
    })?;
    let toml = state.mcp_toml(&bot.bot_id);
    let claude_json = state.claude_mcp_json(&bot.bot_id);
    let prompt = &accepted.message;
    let args = match accepted.cli.as_str() {
        "claude" => launch::claude_headless_argv(
            &accepted.program,
            &runner.cli.claude,
            &claude_json,
            runner.mcp.allow_write_call,
            prompt,
        ),
        "codex" => launch::codex_headless_argv(&accepted.program, &runner.cli.codex, mcp_server, &toml, prompt),
        _ => launch::opencode_headless_argv(&accepted.program, prompt),
    };
    let args = args.map_err(|e| {
        tracing::error!("headless launch refused: {e}");
        refusal(
            "runner_agent_disabled",
            "the runner could not build the command line for this CLI",
        )
    })?;
    Ok(Prepared {
        program: accepted.program.clone(),
        args,
    })
}

/// How a run ended.
#[derive(Debug, PartialEq, Eq)]
pub struct RunEnd {
    /// `success`, `failed`, `timeout` or `cancelled`.
    pub status: &'static str,
    pub exit_code: Option<i32>,
}

/// Starts the CLI in `dir` with the operator's environment (minus the agent variables) and waits
/// for it.
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
    for (name, value) in super::tmux::agent_environment() {
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
