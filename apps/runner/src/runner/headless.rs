//! The headless channel: the CLI runs once, in the canonical working directory, with the
//! permissions of `[runner.cli.*]`, and ends when its process ends.
//!
//! The prompt is the first line of the task message followed by the procedure text of the default
//! instructions. When the process ends and Sylvode still shows the task active, the runner fails
//! it (`source: "runner"`, not retryable): the agent ended without completing or failing it.

use std::time::Duration;

use super::checks::Accepted;
use super::config::RunnerConfig;
use crate::config::{CliAgentConfig, Config};
use crate::dispatcher::run_cli_executor;

/// How long one headless run may take.
pub const RUN_TIMEOUT_SECS: u64 = 3600;

/// The text the task is failed with when the agent ended without ending it.
pub const UNFINISHED_REASON: &str = "agent ended without completing or failing the task";

/// The settings of the run, built from `[runner.cli.<cli>]`, `[runner.mcp]` and the canonical
/// directory.
pub fn cli_settings(config: &Config, accepted: &Accepted) -> CliAgentConfig {
    settings_for(&config.runner, &accepted.cli, &accepted.dir.to_string_lossy())
}

/// The settings of `cli` in `dir` as the headless executor takes them. The interactive launch
/// validates its permissions through the same settings, so both channels refuse the same
/// combinations.
pub fn settings_for(runner: &RunnerConfig, cli: &str, dir: &str) -> CliAgentConfig {
    let mut cfg = match cli {
        "claude" => {
            let claude = &runner.cli.claude;
            let mut cfg = CliAgentConfig::headless("claude-code", dir, RUN_TIMEOUT_SECS);
            cfg.permission_mode = Some(claude.permission_mode.clone());
            if !claude.allowed_tools.is_empty() || claude.allow_write_call {
                let mut tools = claude.allowed_tools.clone();
                if claude.allow_write_call {
                    tools.push("mcp__sylvode__sylvode_write".to_string());
                }
                cfg.allowed_tools = Some(tools);
            }
            if !claude.disallowed_tools.is_empty() {
                cfg.disallowed_tools = Some(claude.disallowed_tools.clone());
            }
            cfg.dangerously_bypass_permissions = claude.dangerously_bypass_permissions;
            cfg
        }
        "codex" => {
            let codex = &runner.cli.codex;
            let mut cfg = CliAgentConfig::headless("codex", dir, RUN_TIMEOUT_SECS);
            cfg.sandbox = Some(codex.sandbox.clone());
            cfg.dangerously_bypass_permissions = codex.dangerously_bypass_permissions;
            cfg
        }
        _ => CliAgentConfig::headless("opencode", dir, RUN_TIMEOUT_SECS),
    };
    cfg.mcp_config_path.clone_from(&runner.mcp.mcp_config_path);
    cfg
}

/// The prompt: the task message of design section 4.9, as the tmux channel types it.
pub fn prompt(accepted: &Accepted) -> String {
    accepted.message.clone()
}

/// Starts the run in the background: a slot of `cli_max_concurrency`, a progress report, the CLI,
/// heartbeats while it runs, and the failure of a task the agent left open.
pub fn start(config: &Config, background: &crate::background::Background, accepted: Accepted) {
    let settings = cli_settings(config, &accepted);
    let prompt = prompt(&accepted);
    let timeout = Duration::from_secs(config.runner.delivery_check_timeout_secs);
    let heartbeat = Duration::from_secs(config.runner.heartbeat_secs.max(1));
    let task_background = background.clone();
    background.spawn(async move {
        let Some(permit) = task_background.cli_permit().await else {
            return;
        };
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
        let program = accepted.program.to_string_lossy().into_owned();
        let run = run_cli_executor(&settings, &prompt, Some(&program), task_background.cancelled());
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
        tracing::info!(status = %result.status, exit_code = ?result.exit_code, "headless run ended");
        // When the view cannot be read, ending an already ended task is refused harmlessly.
        let still_active = accepted
            .client
            .task_view(&accepted.task_id, accepted.attempt, timeout)
            .await
            .map_or(true, |view| {
                view.get("active").and_then(serde_json::Value::as_bool) != Some(false)
            });
        if still_active
            && let Err(e) = accepted
                .client
                .fail(&accepted.task_id, accepted.attempt, UNFINISHED_REASON, false, timeout)
                .await
        {
            tracing::warn!("could not fail the task after the run: {e}");
        }
    });
}
