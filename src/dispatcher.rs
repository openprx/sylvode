use crate::background::Background;
use crate::process::{self, End};
use crate::{callback, config::AgentConfig, config::CliAgentConfig, config::Config};
use serde_json::Value;
use std::borrow::Cow;
use std::future::Future;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

const DEFAULT_SUBPROCESS_TIMEOUT_SECS: u64 = 60;

#[allow(clippy::doc_markdown, clippy::literal_string_with_formatting_args)]
const DEFAULT_MCP_INSTRUCTIONS: &str = r#"## MCP Integration

You have Sylvode MCP tools available. Use them to get full issue context before working:

1. Call `work_items.get` with work_item_id="{issue_id}" to read full issue details (title, description, state, priority, assignee)
2. Call `comments.list` with work_item_id="{issue_id}" to read all comments and discussion
3. Call `work_items.list_labels` with work_item_id="{issue_id}" to read labels

After completing your work:

4. Call `comments.create` with work_item_id="{issue_id}" to post a summary of what you did
5. Call `work_items.update` with work_item_id="{issue_id}" and state="done" if the fix is complete"#;

#[allow(clippy::doc_markdown, clippy::literal_string_with_formatting_args)]
const DEFAULT_FORM_MCP_INSTRUCTIONS: &str = r#"## MCP Integration

You have Sylvode MCP tools available. This webhook is linked to a universal-form event:

1. Call `forms.get` with form_id="{form_id}" when a form_id is available, or call `forms.list` with project_id="{project_id}" and match form_key="{form_key}".
2. Call `form_records.get` with record_id="{record_id}" when a record_id is available.
3. Call `events.tail` with form_id="{form_id}" or record_id="{record_id}" to inspect the recent business-event stream.
4. Call `comments.create` on the linked work item, when there is one, to report what you did."#;

/// Message sent by the `openclaw`, `openprx` and `custom` agents without a `message_template`.
#[allow(clippy::literal_string_with_formatting_args)]
const DEFAULT_MESSAGE_TEMPLATE: &str = "[{project}] {event}: {key} {title}\n{actor} | Trigger: {reason}";

/// Prompt given to a `cli` agent without a `prompt_template`.
#[allow(clippy::literal_string_with_formatting_args)]
const DEFAULT_PROMPT_TEMPLATE: &str = "Fix issue {issue_id}: {title}\nContext: {reason}";

pub async fn dispatch(config: &Config, background: &Background, agent: &AgentConfig, payload: &Value) -> String {
    match agent.agent_type.as_str() {
        "openclaw" => dispatch_openclaw(config, background, agent, payload).await,
        "openprx" => dispatch_openprx(config, background, agent, payload).await,
        "webhook" => dispatch_webhook(config, agent, payload).await,
        "custom" => dispatch_custom(config, background, agent, payload).await,
        "cli" => dispatch_cli(config, background, agent, payload).await,
        other => format!("unknown agent type: {other}"),
    }
}

async fn dispatch_cli(config: &Config, background: &Background, agent: &AgentConfig, payload: &Value) -> String {
    if !config.cli_enabled() {
        return "cli disabled by feature flag or safe mode".into();
    }

    let Some(cfg) = &agent.cli else {
        return "missing cli config".into();
    };

    let issue_id = extract_issue_id(payload).unwrap_or_else(|| "unknown".to_string());
    let run_id = format!(
        "run-{}",
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |d| d.as_millis())
    );
    let prompt = build_cli_prompt(agent, payload, &issue_id);
    let executor = cfg.executor.clone();

    // 1. Send start callback synchronously (wait for it)
    let start_state = if cfg.skip_callback_state {
        None
    } else {
        cfg.update_state_on_start.clone()
    };
    if config.callback_enabled() && start_state.is_some() {
        let start_payload = callback::build_callback_payload(
            issue_id.clone(),
            run_id.clone(),
            executor.clone(),
            "started".into(),
            "task started".into(),
            None,
            0,
            String::new(),
            String::new(),
            start_state,
        );
        if let Err(e) = callback::send_callback(cfg, &start_payload, config.runtime.http_timeout_secs).await {
            tracing::warn!("start callback failed: {e}");
        }
    }
    // 2. Clone what the background task needs
    let bg_cfg = cfg.clone();
    let bg_issue_id = issue_id.clone();
    let bg_run_id = run_id.clone();
    let bg_executor = executor.clone();
    let callback_enabled = config.callback_enabled();
    let http_timeout_secs = config.runtime.http_timeout_secs;

    let task_background = background.clone();

    // 3. Run the CLI process in the background — do NOT await. It waits for one of the
    // `cli_max_concurrency` slots first. On shutdown the run is cancelled, whether it is waiting
    // or running, and still sends its terminal callback below.
    background.spawn(async move {
        let run = match task_background.cli_permit().await {
            Some(permit) => {
                tracing::info!(executor = %bg_executor, "CLI agent spawned");
                let run = run_cli_executor(&bg_cfg, &prompt, task_background.cancelled()).await;
                drop(permit);
                run
            }
            None => CliRunResult::cancelled_before_start(),
        };

        if run.status == "success" {
            tracing::info!(
                executor = %bg_executor,
                status = %run.status,
                exit_code = ?run.exit_code,
                duration_ms = %run.duration_ms,
                "CLI agent completed"
            );
        } else {
            tracing::error!(
                executor = %bg_executor,
                status = %run.status,
                exit_code = ?run.exit_code,
                duration_ms = %run.duration_ms,
                stderr = %run.stderr_tail,
                stdout = %run.stdout_tail,
                "CLI agent failed"
            );
        }

        // Send final callback only on failure/timeout when skip_callback_state is false
        // (on success the agent handles state updates via MCP itself)
        if callback_enabled && !bg_cfg.skip_callback_state && run.status != "success" {
            let final_state = callback::state_for_status(&bg_cfg, &run.status);
            let summary = format!("cli execution {}", run.status);
            let callback_payload = callback::build_callback_payload(
                bg_issue_id,
                bg_run_id,
                bg_executor,
                run.status,
                summary,
                run.exit_code,
                run.duration_ms,
                run.stdout_tail,
                run.stderr_tail,
                final_state,
            );

            if let Err(e) = callback::send_callback(&bg_cfg, &callback_payload, http_timeout_secs).await {
                tracing::warn!("final callback failed: {e}");
            }
        }
    });

    // 4. Return immediately
    format!("cli agent spawned in background run_id={run_id} issue_id={issue_id} executor={executor}")
}

#[derive(Debug, Clone)]
pub struct CliExecutionReport {
    pub run_id: String,
    pub issue_id: String,
    pub status: String,
    pub summary: String,
}

pub async fn execute_cli_task(
    config: &Config,
    agent: &AgentConfig,
    payload: &Value,
    run_id_override: Option<String>,
    cancelled: impl Future<Output = ()>,
) -> CliExecutionReport {
    let Some(cfg) = &agent.cli else {
        return CliExecutionReport {
            run_id: run_id_override.unwrap_or_else(|| "run-invalid".to_string()),
            issue_id: "unknown".to_string(),
            status: "failed".to_string(),
            summary: "missing cli config".to_string(),
        };
    };

    if !config.cli_enabled() {
        return CliExecutionReport {
            run_id: run_id_override.unwrap_or_else(|| "run-disabled".to_string()),
            issue_id: extract_issue_id(payload).unwrap_or_else(|| "unknown".to_string()),
            status: "failed".to_string(),
            summary: "cli disabled by feature flag or safe mode".to_string(),
        };
    }

    let issue_id = extract_issue_id(payload).unwrap_or_else(|| "unknown".to_string());
    let run_id = run_id_override.unwrap_or_else(|| {
        format!(
            "run-{}",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map_or(0, |d| d.as_millis())
        )
    });
    let prompt = build_cli_prompt(agent, payload, &issue_id);

    let start_state = if cfg.skip_callback_state {
        None
    } else {
        cfg.update_state_on_start.clone()
    };
    if config.callback_enabled() && start_state.is_some() {
        let start_payload = callback::build_callback_payload(
            issue_id.clone(),
            run_id.clone(),
            cfg.executor.clone(),
            "started".into(),
            "task started".into(),
            None,
            0,
            String::new(),
            String::new(),
            start_state,
        );
        if let Err(e) = callback::send_callback(cfg, &start_payload, config.runtime.http_timeout_secs).await {
            tracing::warn!("start callback failed: {e}");
        }
    }
    let run = run_cli_executor(cfg, &prompt, cancelled).await;

    tracing::info!(
        executor = %cfg.executor,
        status = %run.status,
        exit_code = ?run.exit_code,
        duration_ms = %run.duration_ms,
        "CLI execution completed"
    );

    let final_state = if cfg.skip_callback_state {
        None
    } else {
        callback::state_for_status(cfg, &run.status)
    };
    let summary = if run.status == "success" {
        "cli execution completed".to_string()
    } else {
        format!("cli execution {}", run.status)
    };

    if config.callback_enabled() {
        let callback_payload = callback::build_callback_payload(
            issue_id.clone(),
            run_id.clone(),
            cfg.executor.clone(),
            run.status.clone(),
            summary.clone(),
            run.exit_code,
            run.duration_ms,
            run.stdout_tail.clone(),
            run.stderr_tail.clone(),
            final_state,
        );

        if let Err(e) = callback::send_callback(cfg, &callback_payload, config.runtime.http_timeout_secs).await {
            tracing::warn!("final callback failed: {e}");
        }
    }

    CliExecutionReport {
        run_id,
        issue_id,
        status: run.status,
        summary,
    }
}

#[derive(Debug)]
struct CliRunResult {
    status: String,
    exit_code: Option<i32>,
    duration_ms: u128,
    stdout_tail: String,
    stderr_tail: String,
}

impl CliRunResult {
    /// A run that shutdown cancelled while it was still waiting for a slot.
    fn cancelled_before_start() -> Self {
        Self {
            status: "cancelled".into(),
            exit_code: None,
            duration_ms: 0,
            stdout_tail: String::new(),
            stderr_tail: "cancelled by shutdown before it started".into(),
        }
    }
}

async fn run_cli_executor(cfg: &CliAgentConfig, prompt: &str, cancelled: impl Future<Output = ()>) -> CliRunResult {
    let (program, args) = match build_executor_command(&cfg.executor, prompt, cfg.mcp_config_path.as_deref()) {
        Ok(v) => v,
        Err(err) => {
            return CliRunResult {
                status: "failed".into(),
                exit_code: None,
                duration_ms: 0,
                stdout_tail: String::new(),
                stderr_tail: err,
            };
        }
    };

    let mut cmd = tokio::process::Command::new(program);
    cmd.args(args)
        .env_remove("CLAUDECODE")
        .env_remove("CLAUDE_CODE_ENTRYPOINT");
    if let Some(dir) = &cfg.workdir {
        cmd.current_dir(dir);
    }
    for (key, value) in &cfg.env_vars {
        cmd.env(key, value);
    }

    let started = Instant::now();
    let timeout_secs = cfg.timeout_secs;
    let max_output_chars = cfg.max_output_chars;
    let finished = match process::run(
        cmd,
        Duration::from_secs(timeout_secs),
        process::TERMINATION_GRACE,
        cancelled,
    )
    .await
    {
        Ok(finished) => finished,
        Err(e) => {
            tracing::error!(executor = %cfg.executor, error = %e, "Failed to spawn CLI process");
            return CliRunResult {
                status: "failed".into(),
                exit_code: None,
                duration_ms: started.elapsed().as_millis(),
                stdout_tail: String::new(),
                stderr_tail: format!("spawn failed: {e}"),
            };
        }
    };

    let stdout_tail = tail_chars(&String::from_utf8_lossy(&finished.stdout), max_output_chars);
    let stderr_tail = tail_chars(&String::from_utf8_lossy(&finished.stderr), max_output_chars);
    let (status, exit_code, stderr_tail) = match finished.end {
        End::Exited(exit) => (
            if exit.success() { "success" } else { "failed" },
            exit.code(),
            stderr_tail,
        ),
        End::WaitFailed(e) => ("failed", None, format!("wait failed: {e}")),
        End::TimedOut => ("timeout", None, format!("{stderr_tail}\ntimeout after {timeout_secs}s")),
        End::Cancelled => ("cancelled", None, format!("{stderr_tail}\ncancelled by shutdown")),
    };
    CliRunResult {
        status: status.into(),
        exit_code,
        duration_ms: started.elapsed().as_millis(),
        stdout_tail,
        stderr_tail: stderr_tail.trim_start_matches('\n').to_string(),
    }
}

fn build_executor_command(
    executor: &str,
    prompt: &str,
    mcp_config_path: Option<&str>,
) -> Result<(&'static str, Vec<String>), String> {
    // The prompt is a positional argument and contains payload text (an issue title, say), so a
    // leading `-` would make it an option of the executor, such as codex's `--config=...`.
    if prompt.starts_with('-') {
        return Err(
            "the prompt starts with `-` and would be read as an option of the executor; \
                    the run was not started (start prompt_template with fixed text)"
                .to_string(),
        );
    }
    match executor {
        "codex" => Ok((
            "codex",
            vec![
                "exec".into(),
                "--dangerously-bypass-approvals-and-sandbox".into(),
                "--skip-git-repo-check".into(),
                prompt.into(),
            ],
        )),
        "claude-code" => {
            let mut args = vec![
                "-p".into(),
                prompt.into(),
                "--permission-mode".into(),
                "bypassPermissions".into(),
            ];
            if let Some(mcp_path) = mcp_config_path {
                args.push("--mcp-config".into());
                args.push(mcp_path.into());
            }
            Ok(("claude", args))
        }
        "opencode" => Ok(("opencode", vec!["run".into(), prompt.into()])),
        _ => Err(format!("executor not allowed: {executor}")),
    }
}

fn build_cli_prompt(agent: &AgentConfig, payload: &Value, issue_id: &str) -> String {
    let base = agent
        .cli
        .as_ref()
        .and_then(|c| c.prompt_template.as_deref())
        .unwrap_or(DEFAULT_PROMPT_TEMPLATE);

    let title = extract_issue_title(payload).unwrap_or("untitled");
    let reason = extract_trigger_reason(payload).unwrap_or("unknown");
    let event = extract_event(payload).unwrap_or("unknown");
    let project_id = extract_project_id(payload).unwrap_or("<project_id>");
    let form_id = extract_form_id(payload).unwrap_or("<form_id>");
    let form_key = extract_form_key(payload).unwrap_or("<form_key>");
    let record_id = extract_record_id(payload).unwrap_or("<record_id>");

    let user_prompt = render_template(
        base,
        &[
            ("issue_id", issue_id),
            ("title", title),
            ("reason", reason),
            ("event", event),
            ("project_id", project_id),
            ("form_id", form_id),
            ("form_key", form_key),
            ("record_id", record_id),
        ],
    );

    // Only append MCP instructions when explicitly configured or when
    // mcp_config_path / env_vars indicate MCP integration is active.
    let cli = agent.cli.as_ref();
    let has_mcp_config =
        cli.is_some_and(|c| c.mcp_instructions.is_some() || c.mcp_config_path.is_some() || !c.env_vars.is_empty());

    if !has_mcp_config {
        return user_prompt;
    }

    let mcp_instructions = cli.and_then(|c| c.mcp_instructions.as_deref()).unwrap_or_else(|| {
        if is_form_event(payload) {
            DEFAULT_FORM_MCP_INSTRUCTIONS
        } else {
            DEFAULT_MCP_INSTRUCTIONS
        }
    });

    let instructions = render_template(
        mcp_instructions,
        &[
            ("issue_id", issue_id),
            ("project_id", project_id),
            ("form_id", form_id),
            ("form_key", form_key),
            ("record_id", record_id),
            ("event", event),
        ],
    );

    let mut prompt = format!("{user_prompt}\n\n{instructions}");
    if let Some(project_context_instructions) = build_project_context_instructions(payload) {
        prompt.push_str("\n\n");
        prompt.push_str(&project_context_instructions);
    }
    if let Some(form_context_instructions) = build_form_context_instructions(payload) {
        prompt.push_str("\n\n");
        prompt.push_str(&form_context_instructions);
    }
    prompt
}

fn string_field<'a>(value: &'a Value, key: &str) -> Option<&'a str> {
    value.get(key).and_then(Value::as_str).filter(|value| !value.is_empty())
}

fn nested_string_field<'a>(value: &'a Value, path: &[&str]) -> Option<&'a str> {
    let mut cursor = value;
    for key in path {
        cursor = cursor.get(*key)?;
    }
    cursor.as_str().filter(|value| !value.is_empty())
}

/// Whether `payload` is an AI task envelope from the Sylvode worker.
fn is_worker_task(payload: &Value) -> bool {
    string_field(payload, "task_id").is_some() && string_field(payload, "ai_participant_id").is_some()
}

/// The issue a delivery is about, which is also the issue every callback for it writes to.
///
/// For a worker AI task this is only the task's `reference_id` (when `reference_type` is
/// `work_item`), which the worker takes from the task record. The task's inner `payload` is
/// free-form data supplied by whoever created the task, so an `issue_id` inside it is ignored:
/// otherwise it could point the callback, sent with the bot's token, at another issue.
pub fn extract_issue_id(payload: &Value) -> Option<String> {
    if is_worker_task(payload) {
        return (payload.get("reference_type").and_then(Value::as_str) == Some("work_item"))
            .then(|| {
                payload
                    .get("reference_id")
                    .and_then(Value::as_str)
                    .map(ToString::to_string)
            })
            .flatten();
    }
    let direct_issue_id = payload
        .get("data")
        .and_then(|d| d.get("issue"))
        .and_then(|i| i.get("id"))
        .or_else(|| payload.get("payload").and_then(|p| p.get("issue_id")))
        .or_else(|| payload.get("issue_id"));

    if let Some(issue_id) = direct_issue_id {
        return issue_id
            .as_str()
            .map(ToString::to_string)
            .or_else(|| issue_id.as_i64().map(|n| n.to_string()));
    }

    let reference_type = payload.get("reference_type").and_then(Value::as_str);
    if reference_type == Some("work_item") {
        return payload
            .get("reference_id")
            .and_then(Value::as_str)
            .map(ToString::to_string);
    }

    None
}

fn extract_issue_title(payload: &Value) -> Option<&str> {
    payload
        .get("data")
        .and_then(|d| d.get("issue"))
        .and_then(|i| i.get("title"))
        .and_then(Value::as_str)
        .or_else(|| {
            payload
                .get("payload")
                .and_then(|p| p.get("issue_title"))
                .and_then(Value::as_str)
        })
        .or_else(|| payload.get("issue_title").and_then(Value::as_str))
        .or_else(|| payload.get("title").and_then(Value::as_str))
}

fn extract_event(payload: &Value) -> Option<&str> {
    string_field(payload, "event")
        .or_else(|| string_field(payload, "event_type"))
        .or_else(|| nested_string_field(payload, &["payload", "event"]))
        .or_else(|| nested_string_field(payload, &["payload", "event_type"]))
        .or_else(|| nested_string_field(payload, &["payload", "envelope", "event_type"]))
        .or_else(|| string_field(payload, "task_type"))
}

fn extract_trigger_reason(payload: &Value) -> Option<&str> {
    payload
        .get("bot_context")
        .and_then(|bc| bc.get("trigger_reason"))
        .and_then(Value::as_str)
        .or_else(|| payload.get("trigger_kind").and_then(Value::as_str))
        .or_else(|| {
            payload
                .get("payload")
                .and_then(|p| p.get("trigger"))
                .and_then(Value::as_str)
        })
        .or_else(|| payload.get("task_type").and_then(Value::as_str))
}

fn extract_project_id(payload: &Value) -> Option<&str> {
    payload
        .get("project_id")
        .and_then(Value::as_str)
        .or_else(|| {
            payload
                .get("payload")
                .and_then(|p| p.get("project_id"))
                .and_then(Value::as_str)
        })
        .or_else(|| nested_string_field(payload, &["payload", "envelope", "project_id"]))
        .or_else(|| nested_string_field(payload, &["payload", "envelope", "metadata", "project_id"]))
        .or_else(|| nested_string_field(payload, &["payload", "envelope", "payload", "project_id"]))
        .or_else(|| payload.get("project").and_then(|p| p.get("id")).and_then(Value::as_str))
        .filter(|value| !value.is_empty())
}

fn extract_form_id(payload: &Value) -> Option<&str> {
    string_field(payload, "form_id")
        .or_else(|| nested_string_field(payload, &["payload", "form_id"]))
        .or_else(|| nested_string_field(payload, &["metadata", "form_id"]))
        .or_else(|| nested_string_field(payload, &["payload", "metadata", "form_id"]))
        .or_else(|| nested_string_field(payload, &["payload", "envelope", "metadata", "form_id"]))
        .or_else(|| nested_string_field(payload, &["payload", "envelope", "payload", "form_id"]))
}

fn extract_form_key(payload: &Value) -> Option<&str> {
    string_field(payload, "form_key")
        .or_else(|| nested_string_field(payload, &["payload", "form_key"]))
        .or_else(|| nested_string_field(payload, &["metadata", "form_key"]))
        .or_else(|| nested_string_field(payload, &["payload", "metadata", "form_key"]))
        .or_else(|| nested_string_field(payload, &["payload", "envelope", "metadata", "form_key"]))
        .or_else(|| nested_string_field(payload, &["payload", "envelope", "payload", "form_key"]))
        .or_else(|| {
            nested_string_field(payload, &["payload", "envelope", "aggregate", "type"])
                .and_then(|value| value.strip_prefix("form."))
                .filter(|value| !value.is_empty())
        })
}

fn extract_record_id(payload: &Value) -> Option<&str> {
    string_field(payload, "record_id")
        .or_else(|| nested_string_field(payload, &["payload", "record_id"]))
        .or_else(|| nested_string_field(payload, &["metadata", "record_id"]))
        .or_else(|| nested_string_field(payload, &["payload", "metadata", "record_id"]))
        .or_else(|| nested_string_field(payload, &["payload", "envelope", "metadata", "record_id"]))
        .or_else(|| nested_string_field(payload, &["payload", "envelope", "payload", "record_id"]))
        .or_else(|| nested_string_field(payload, &["payload", "envelope", "aggregate", "id"]))
}

fn is_form_event(payload: &Value) -> bool {
    extract_form_key(payload).is_some()
        || extract_form_id(payload).is_some()
        || extract_event(payload).is_some_and(|event| event.starts_with("form.") || event.starts_with("print_job."))
}

fn build_project_context_instructions(payload: &Value) -> Option<String> {
    let project_id = extract_project_id(payload)?;
    Some(format!(
        "## Project MCP Context\n\nBefore changing state or posting final output, call `context.get_project` with project_id=\"{project_id}\" to read the project type, resources, governance context, and effective agent policy. When your MCP client supports project-scoped discovery, call `tools/list` with project_id=\"{project_id}\" and only use tools enabled by that project policy."
    ))
}

fn build_form_context_instructions(payload: &Value) -> Option<String> {
    if !is_form_event(payload) {
        return None;
    }

    let project_id = extract_project_id(payload).unwrap_or("<project_id>");
    let form_id = extract_form_id(payload).unwrap_or("<form_id>");
    let form_key = extract_form_key(payload).unwrap_or("<form_key>");
    let record_id = extract_record_id(payload).unwrap_or("<record_id>");
    let event = extract_event(payload).unwrap_or("<event>");

    Some(format!(
        "## Universal Form Context\n\nThis is Sylvode universal-form event `{event}` for project_id=\"{project_id}\", form_id=\"{form_id}\", form_key=\"{form_key}\", record_id=\"{record_id}\". Treat custom fields as schema-defined business data: inspect the form schema before assuming field meaning, preserve decimal strings for amount fields, and write any operational result through MCP instead of mutating local webhook state."
    ))
}

fn issue_field<'a>(payload: &'a Value, key: &str) -> Option<&'a str> {
    let payload_key = format!("issue_{key}");
    payload
        .get("data")
        .and_then(|d| d.get("issue"))
        .and_then(|i| i.get(key))
        .and_then(Value::as_str)
        .or_else(|| {
            payload
                .get("payload")
                .and_then(|p| p.get(&payload_key))
                .and_then(Value::as_str)
        })
        .or_else(|| payload.get(&payload_key).and_then(Value::as_str))
}

fn tail_chars(input: &str, max_chars: usize) -> String {
    let char_count = input.chars().count();
    if char_count <= max_chars {
        return input.to_string();
    }
    input.chars().skip(char_count - max_chars).collect()
}

// --- Security: all subprocess dispatchers use direct args, no sh -c ---

/// The result of a command run by an `openclaw`, `openprx` or `custom` agent.
struct CommandOutput {
    success: bool,
    stdout: String,
    stderr: String,
}

async fn run_subprocess_with_timeout(
    program: &str,
    args: &[&str],
    timeout_secs: u64,
    cancelled: impl Future<Output = ()>,
) -> Result<CommandOutput, String> {
    let mut command = tokio::process::Command::new(program);
    command.args(args);
    let finished = process::run(
        command,
        Duration::from_secs(timeout_secs),
        process::TERMINATION_GRACE,
        cancelled,
    )
    .await
    .map_err(|e| format!("spawn {program} failed: {e}"))?;
    match finished.end {
        End::Exited(status) => Ok(CommandOutput {
            success: status.success(),
            stdout: String::from_utf8_lossy(&finished.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&finished.stderr).into_owned(),
        }),
        End::TimedOut => Err(format!("subprocess {program} timed out after {timeout_secs}s")),
        End::Cancelled => Err(format!("subprocess {program} cancelled by shutdown")),
        End::WaitFailed(e) => Err(format!("subprocess {program} wait failed: {e}")),
    }
}

/// Arguments of an `openclaw` or `openprx` command: the configured fixed arguments, then
/// `--channel <channel> --target <target> --message <message>`.
fn channel_command_args<'a>(fixed: &'a [String], channel: &'a str, target: &'a str, message: &'a str) -> Vec<&'a str> {
    fixed
        .iter()
        .map(String::as_str)
        .chain(["--channel", channel, "--target", target, "--message", message])
        .collect()
}

async fn dispatch_openclaw(config: &Config, background: &Background, agent: &AgentConfig, payload: &Value) -> String {
    let Some(cfg) = &agent.openclaw else {
        return "missing openclaw config".into();
    };

    let message = format_message(agent, payload);
    let timeout = config.runtime.http_timeout_secs.max(DEFAULT_SUBPROCESS_TIMEOUT_SECS);

    let args = channel_command_args(&cfg.args, &cfg.channel, &cfg.target, &message);

    match run_subprocess_with_timeout(&cfg.command, &args, timeout, background.cancelled()).await {
        Ok(o) if o.success => {
            let out = o.stdout.trim();
            tracing::info!("openclaw ok: {out}");
            format!("ok: {out}")
        }
        Ok(o) => {
            let err = o.stderr.trim();
            tracing::error!("openclaw failed: {err}");
            format!("error: {err}")
        }
        Err(e) => {
            tracing::error!("openclaw exec failed: {e}");
            format!("exec_error: {e}")
        }
    }
}

async fn dispatch_openprx(config: &Config, background: &Background, agent: &AgentConfig, payload: &Value) -> String {
    let Some(cfg) = &agent.openprx else {
        return "missing openprx config".into();
    };

    let message = format_message(agent, payload);

    if let Some(signal_api) = &cfg.signal_api {
        let account = cfg.account.as_deref().unwrap_or("");
        let url = format!("{}/api/v1/send/{account}", signal_api.trim_end_matches('/'));

        let body = serde_json::json!({
            "recipients": [&cfg.target],
            "message": message
        });

        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(config.runtime.http_timeout_secs))
            .build();
        let Ok(client) = client else {
            return "error: failed to build http client".into();
        };

        return match client.post(&url).json(&body).send().await {
            Ok(resp) if resp.status().is_success() => {
                tracing::info!("openprx signal ok");
                "ok".into()
            }
            Ok(resp) => {
                let status = resp.status();
                let text = resp.text().await.unwrap_or_default();
                tracing::error!("openprx signal {status}: {text}");
                format!("error: {status} {text}")
            }
            Err(e) => {
                tracing::error!("openprx signal request failed: {e}");
                format!("error: {e}")
            }
        };
    }

    if let Some(command) = &cfg.command {
        let timeout = config.runtime.http_timeout_secs.max(DEFAULT_SUBPROCESS_TIMEOUT_SECS);

        let args = channel_command_args(&cfg.args, &cfg.channel, &cfg.target, &message);

        return match run_subprocess_with_timeout(command, &args, timeout, background.cancelled()).await {
            Ok(o) if o.success => {
                tracing::info!("openprx cli ok");
                "ok".into()
            }
            Ok(o) => {
                let err = o.stderr.trim();
                tracing::error!("openprx cli failed: {err}");
                format!("error: {err}")
            }
            Err(e) => format!("exec_error: {e}"),
        };
    }

    "openprx config needs either signal_api or command".into()
}

fn outbound_signature_header_value(payload: &Value, secret: Option<&str>) -> Option<String> {
    secret.and_then(|secret| {
        serde_json::to_vec(payload)
            .ok()
            .and_then(|bytes| crate::signature::sign_payload(&bytes, secret).ok())
            .map(|sig| format!("sha256={sig}"))
    })
}

async fn dispatch_webhook(config: &Config, agent: &AgentConfig, payload: &Value) -> String {
    let Some(cfg) = &agent.webhook else {
        return "missing webhook config".into();
    };

    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(config.runtime.http_timeout_secs))
        .build();
    let Ok(client) = client else {
        return "webhook_error: failed to build http client".into();
    };

    let mut request = client.post(&cfg.url).json(payload);
    if let Some(signature_value) = outbound_signature_header_value(payload, cfg.secret.as_deref()) {
        request = request.header(crate::signature::OUTBOUND_SIGNATURE_HEADER, signature_value);
    }

    match request.send().await {
        Ok(resp) => format!("webhook: {}", resp.status()),
        Err(e) => format!("webhook_error: {e}"),
    }
}

async fn dispatch_custom(config: &Config, background: &Background, agent: &AgentConfig, payload: &Value) -> String {
    let Some(cfg) = &agent.custom else {
        return "missing custom config".into();
    };

    let message = format_message(agent, payload);
    let timeout = config.runtime.http_timeout_secs.max(DEFAULT_SUBPROCESS_TIMEOUT_SECS);

    let args = match build_custom_args(cfg.args.as_deref().unwrap_or_default(), &message) {
        Ok(args) => args,
        Err(e) => {
            tracing::warn!(agent = %agent.id, "custom agent not run: {e}");
            return format!("custom_error: {e}");
        }
    };
    let args: Vec<&str> = args.iter().map(AsRef::as_ref).collect();

    match run_subprocess_with_timeout(&cfg.command, &args, timeout, background.cancelled()).await {
        Ok(o) => o.stdout,
        Err(e) => format!("custom_error: {e}"),
    }
}

const MESSAGE_PLACEHOLDER: &str = "{message}";

/// Builds the argument list of a `custom` agent.
///
/// Each configured argument containing `{message}` has it replaced by the rendered message; the
/// result stays exactly one argument (there is no shell and nothing is split). Without the
/// placeholder in any argument, the message is appended as the last argument.
///
/// An argument that starts with `-` only because of the inserted message is refused, unless an
/// earlier configured argument is `--`: payload text must never turn into an option of the
/// command. An argument the operator wrote with a leading `-`, such as `--text={message}`, is
/// left alone.
fn build_custom_args<'a>(configured: &'a [String], message: &'a str) -> Result<Vec<Cow<'a, str>>, String> {
    let has_placeholder = configured.iter().any(|arg| arg.contains(MESSAGE_PLACEHOLDER));
    let mut args = Vec::with_capacity(configured.len() + 1);
    let mut options_ended = false;
    for arg in configured {
        if arg.contains(MESSAGE_PLACEHOLDER) {
            let rendered = render_template(arg, &[("message", message)]);
            refuse_injected_option(arg, &rendered, options_ended)?;
            args.push(Cow::Owned(rendered));
        } else {
            args.push(Cow::Borrowed(arg.as_str()));
        }
        options_ended |= arg == "--";
    }
    if !has_placeholder {
        refuse_injected_option("", message, options_ended)?;
        args.push(Cow::Borrowed(message));
    }
    Ok(args)
}

fn refuse_injected_option(configured: &str, rendered: &str, options_ended: bool) -> Result<(), String> {
    if !options_ended && rendered.starts_with('-') && !configured.starts_with('-') {
        return Err("the message starts with `-` and would be read as an option; \
                    put `--` before it in args if the command accepts that"
            .to_string());
    }
    Ok(())
}

fn format_message(agent: &AgentConfig, payload: &Value) -> String {
    let tmpl = agent.message_template.as_deref().unwrap_or(DEFAULT_MESSAGE_TEMPLATE);

    let event = extract_event(payload).unwrap_or("unknown");
    let title = extract_issue_title(payload).unwrap_or("untitled");
    let issue_key = issue_field(payload, "key").unwrap_or("");
    let reason = extract_trigger_reason(payload).unwrap_or("unknown");
    let actor = payload
        .get("actor")
        .and_then(|a| a.get("name"))
        .and_then(Value::as_str)
        .unwrap_or("unknown");
    let workspace = payload
        .get("workspace")
        .and_then(|w| w.get("name"))
        .and_then(Value::as_str)
        .unwrap_or("");
    let project = payload
        .get("project")
        .and_then(|p| p.get("name"))
        .and_then(Value::as_str)
        .unwrap_or("");
    let state = issue_field(payload, "state").unwrap_or("");
    let priority = issue_field(payload, "priority").unwrap_or("");
    let issue_id = extract_issue_id(payload).unwrap_or_default();
    let project_id = extract_project_id(payload).unwrap_or("");
    let form_id = extract_form_id(payload).unwrap_or("");
    let form_key = extract_form_key(payload).unwrap_or("");
    let record_id = extract_record_id(payload).unwrap_or("");

    let url = format!("issue/{issue_id}");

    render_template(
        tmpl,
        &[
            ("event", event),
            ("title", title),
            ("key", issue_key),
            ("reason", reason),
            ("actor", actor),
            ("workspace", workspace),
            ("project", project),
            ("state", state),
            ("priority", priority),
            ("issue_id", &issue_id),
            ("project_id", project_id),
            ("form_id", form_id),
            ("form_key", form_key),
            ("record_id", record_id),
            ("url", &url),
        ],
    )
}

/// Renders `template` in a single pass. Each `{name}` that has an entry in `values` is replaced by
/// that value; any other text, including an unknown `{name}`, is copied unchanged. An inserted
/// value is never scanned again, so a placeholder inside payload text stays literal.
fn render_template(template: &str, values: &[(&str, &str)]) -> String {
    let mut out = String::with_capacity(template.len());
    let mut rest = template;
    while let Some((before, after_open)) = rest.split_once('{') {
        out.push_str(before);
        let substituted = after_open.split_once('}').and_then(|(name, after_close)| {
            values
                .iter()
                .find(|(key, _)| *key == name)
                .map(|(_, value)| (*value, after_close))
        });
        if let Some((value, after_close)) = substituted {
            out.push_str(value);
            rest = after_close;
        } else {
            out.push('{');
            rest = after_open;
        }
    }
    out.push_str(rest);
    out
}

#[cfg(test)]
#[allow(clippy::literal_string_with_formatting_args)]
mod tests {
    use super::{
        build_cli_prompt, build_executor_command, dispatch, extract_issue_id, format_message,
        outbound_signature_header_value,
    };
    use crate::background::Background;
    use crate::{callback, config::Config};
    use serde_json::json;

    fn base_config() -> Config {
        toml::from_str(
            r#"
[server]
listen = "127.0.0.1:9090"

[security]
allow_unsigned = true
webhook_secrets = []
"#,
        )
        .expect("parse base config")
    }

    #[test]
    fn builds_outbound_signature_header_when_secret_exists() {
        let payload = json!({"event":"issue.created","value":1});
        let header = outbound_signature_header_value(&payload, Some("top-secret"));

        assert!(header.as_deref().unwrap_or_default().starts_with("sha256="));

        let expected_sig =
            crate::signature::sign_payload(serde_json::to_vec(&payload).unwrap().as_slice(), "top-secret")
                .expect("sign should succeed");
        assert_eq!(header, Some(format!("sha256={expected_sig}")));
    }

    #[test]
    fn cli_executor_whitelist_builds_expected_command() {
        let (_, args) = build_executor_command("codex", "fix it", None).expect("codex should be allowed");
        assert_eq!(
            args,
            vec![
                "exec",
                "--dangerously-bypass-approvals-and-sandbox",
                "--skip-git-repo-check",
                "fix it"
            ]
        );

        assert!(build_executor_command("bash", "rm -rf /", None).is_err());
    }

    /// A prompt is always a positional argument; one that starts with `-` would be read as an
    /// option of the executor (`--config=...` for codex), so it is refused for every executor.
    #[test]
    fn a_prompt_that_starts_with_a_dash_is_refused_for_every_executor() {
        for executor in ["codex", "claude-code", "opencode"] {
            for prompt in ["--config=model_provider=evil --oss", "-x", "-"] {
                let err = build_executor_command(executor, prompt, None).expect_err("refused");
                assert!(err.contains("starts with `-`"), "{executor} {prompt:?}: {err}");
            }
            assert!(build_executor_command(executor, " -x", None).is_ok(), "{executor}");
        }
    }

    #[test]
    fn claude_code_executor_includes_mcp_config_when_set() {
        let (prog, args) =
            build_executor_command("claude-code", "fix it", Some("/path/to/mcp.json")).expect("claude-code allowed");
        assert_eq!(prog, "claude");
        // prompt must come immediately after -p, not as a trailing positional
        assert_eq!(args.first().map(String::as_str), Some("-p"));
        assert_eq!(args.get(1).map(String::as_str), Some("fix it"));
        assert!(args.contains(&"--mcp-config".to_string()));
        assert!(args.contains(&"/path/to/mcp.json".to_string()));
        // prompt must not be the last element (that would make claude treat it as a project dir)
        assert_ne!(args.last().map(String::as_str), Some("fix it"));
    }

    #[test]
    fn claude_code_executor_omits_mcp_config_when_none() {
        let (_, args) = build_executor_command("claude-code", "fix it", None).expect("claude-code allowed");
        assert!(!args.contains(&"--mcp-config".to_string()));
        // prompt must come immediately after -p flag
        assert_eq!(args.first().map(String::as_str), Some("-p"));
        assert_eq!(args.get(1).map(String::as_str), Some("fix it"));
    }

    #[test]
    fn build_cli_prompt_appends_default_mcp_instructions_when_env_vars_set() {
        let agent: crate::config::AgentConfig = toml::from_str(
            r#"
id = "a1"
name = "CLI"
agent_type = "cli"
[cli]
executor = "codex"
prompt_template = "Fix issue {issue_id}: {title}"
[cli.env_vars]
OPENPR_API_URL = "http://localhost:3000"
"#,
        )
        .expect("parse agent");

        let payload =
            json!({"data":{"issue":{"id":"42","title":"Login bug"}},"bot_context":{"trigger_reason":"assigned"}});
        let prompt = build_cli_prompt(&agent, &payload, "42");

        assert!(prompt.starts_with("Fix issue 42: Login bug"));
        assert!(prompt.contains("work_items.get"));
        assert!(prompt.contains("comments.list"));
        assert!(prompt.contains("comments.create"));
    }

    #[test]
    fn build_cli_prompt_omits_mcp_instructions_when_no_mcp_config() {
        let agent: crate::config::AgentConfig = toml::from_str(
            r#"
id = "a1"
name = "CLI"
agent_type = "cli"
[cli]
executor = "codex"
prompt_template = "Fix issue {issue_id}: {title}"
"#,
        )
        .expect("parse agent");

        let payload =
            json!({"data":{"issue":{"id":"42","title":"Login bug"}},"bot_context":{"trigger_reason":"assigned"}});
        let prompt = build_cli_prompt(&agent, &payload, "42");

        assert!(prompt.starts_with("Fix issue 42: Login bug"));
        assert!(
            !prompt.contains("work_items.get"),
            "should not contain MCP instructions"
        );
    }

    #[test]
    fn build_cli_prompt_uses_custom_mcp_instructions() {
        let agent: crate::config::AgentConfig = toml::from_str(
            r#"
id = "a1"
name = "CLI"
agent_type = "cli"
[cli]
executor = "codex"
prompt_template = "Fix {issue_id}"
mcp_instructions = "Custom: read issue {issue_id} first"
"#,
        )
        .expect("parse agent");

        let payload = json!({"data":{"issue":{"id":"99"}}});
        let prompt = build_cli_prompt(&agent, &payload, "99");

        assert!(prompt.contains("Custom: read issue 99 first"));
        assert!(!prompt.contains("work_items.get"));
    }

    #[test]
    fn skip_callback_state_returns_none() {
        let cfg: crate::config::CliAgentConfig = toml::from_str(
            r#"
executor = "codex"
skip_callback_state = true
update_state_on_success = "done"
"#,
        )
        .expect("parse cli config");

        assert!(cfg.skip_callback_state);
        // When skip_callback_state is true, state should be None regardless of status
        let state = if cfg.skip_callback_state {
            None
        } else {
            callback::state_for_status(&cfg, "success")
        };
        assert!(state.is_none());
    }

    #[test]
    fn extracts_issue_id_from_payload() {
        let payload = json!({"data": {"issue": {"id": "42"}}});
        assert_eq!(extract_issue_id(&payload).as_deref(), Some("42"));
    }

    #[test]
    fn extracts_issue_id_from_worker_ai_task_envelope() {
        let payload = json!({
            "task_id": "task-1",
            "ai_participant_id": "bot-1",
            "task_type": "issue_assigned",
            "reference_type": "work_item",
            "reference_id": "issue-99",
            "payload": {
                "project_id": "project-1",
                "trigger": "issue.create"
            }
        });

        assert_eq!(extract_issue_id(&payload).as_deref(), Some("issue-99"));
    }

    /// The issue of a worker AI task is its `reference_id`; an `issue_id` inside the free-form
    /// `payload`, or a non-work-item reference, yields nothing else.
    #[test]
    fn a_worker_task_issue_id_comes_only_from_its_reference() {
        let task = json!({
            "task_id": "task-1",
            "ai_participant_id": "bot-1",
            "reference_type": "work_item",
            "reference_id": "REAL-ISSUE",
            "payload": {"issue_id": "OTHER-ISSUE"}
        });
        assert_eq!(extract_issue_id(&task).as_deref(), Some("REAL-ISSUE"));

        let proposal = json!({
            "task_id": "task-2",
            "ai_participant_id": "bot-1",
            "reference_type": "proposal",
            "reference_id": "PROPOSAL-1",
            "payload": {"issue_id": "OTHER-ISSUE"}
        });
        assert_eq!(extract_issue_id(&proposal), None);
    }

    /// Outside a worker task (a tunnel task body, say) the order is `data.issue.id`, then
    /// `payload.issue_id`, then a top-level `issue_id`.
    #[test]
    fn issue_id_precedence_outside_worker_tasks() {
        let all = json!({"data": {"issue": {"id": "A"}}, "payload": {"issue_id": "B"}, "issue_id": "C"});
        assert_eq!(extract_issue_id(&all).as_deref(), Some("A"));
        let inner = json!({"payload": {"issue_id": "B"}, "issue_id": "C"});
        assert_eq!(extract_issue_id(&inner).as_deref(), Some("B"));
        let top = json!({"issue_id": 7});
        assert_eq!(extract_issue_id(&top).as_deref(), Some("7"));
    }

    #[test]
    fn build_cli_prompt_uses_worker_ai_task_envelope_context() {
        let agent: crate::config::AgentConfig = toml::from_str(
            r#"
id = "bot-1"
name = "CLI"
agent_type = "cli"
[cli]
executor = "codex"
prompt_template = "Fix issue {issue_id}: {title}\nContext: {reason}"
[cli.env_vars]
OPENPR_API_URL = "http://localhost:3000"
"#,
        )
        .expect("parse agent");

        let payload = json!({
            "task_id": "task-1",
            "ai_participant_id": "bot-1",
            "task_type": "issue_assigned",
            "reference_type": "work_item",
            "reference_id": "issue-99",
            "payload": {
                "project_id": "project-1",
                "issue_title": "Worker envelope issue",
                "trigger": "issue.create"
            }
        });
        let issue_id = extract_issue_id(&payload).expect("issue id");
        let prompt = build_cli_prompt(&agent, &payload, &issue_id);

        assert!(prompt.starts_with("Fix issue issue-99: Worker envelope issue"));
        assert!(prompt.contains("Context: issue.create"));
        assert!(prompt.contains("work_items.get"));
        assert!(prompt.contains("Project MCP Context"));
        assert!(prompt.contains("context.get_project"));
        assert!(prompt.contains("tools/list"));
        assert!(prompt.contains("project_id=\"project-1\""));
    }

    #[test]
    fn build_cli_prompt_replaces_project_id_placeholder_in_custom_mcp_instructions() {
        let agent: crate::config::AgentConfig = toml::from_str(
            r#"
id = "bot-1"
name = "CLI"
agent_type = "cli"
[cli]
executor = "codex"
prompt_template = "Fix issue {issue_id}: {title}"
mcp_instructions = "Read project {project_id}, then issue {issue_id}."
"#,
        )
        .expect("parse agent");

        let payload = json!({
            "project_id": "project-1",
            "data": {
                "issue": {
                    "id": "issue-1",
                    "title": "Project scoped issue"
                }
            }
        });
        let issue_id = extract_issue_id(&payload).expect("issue id");
        let prompt = build_cli_prompt(&agent, &payload, &issue_id);

        assert!(prompt.contains("Read project project-1, then issue issue-1."));
        assert!(prompt.contains("context.get_project"));
    }

    #[test]
    fn build_cli_prompt_never_instructs_the_retired_invocation_tools() {
        let agent: crate::config::AgentConfig = toml::from_str(
            r#"
id = "bot-1"
name = "CLI"
agent_type = "cli"
[cli]
executor = "codex"
prompt_template = "Fix issue {issue_id}: {title}"
mcp_instructions = "Read issue {issue_id} before writing."
"#,
        )
        .expect("parse agent");

        let payload = json!({
            "task_id": "task-1",
            "project_id": "project-1",
            "ai_participant_id": "bot-1",
            "task_type": "issue_assigned",
            "reference_type": "work_item",
            "reference_id": "issue-99",
            "invocation_id": "invocation-1",
            "payload": {
                "issue_title": "Ledger issue"
            }
        });
        let issue_id = extract_issue_id(&payload).expect("issue id");
        let prompt = build_cli_prompt(&agent, &payload, &issue_id);

        assert!(prompt.contains("Read issue issue-99 before writing."));
        assert!(prompt.contains("project_id=\"project-1\""));
        // Sylvode retired agent invocations. An old deployment may still send the id, but the
        // tools it named are gone, so an agent must never be told to call them.
        assert!(!prompt.contains("Invocation Ledger"));
        assert!(!prompt.contains("invocations."));
    }

    #[test]
    fn build_cli_prompt_uses_form_context_for_a_form_event() {
        let agent: crate::config::AgentConfig = toml::from_str(
            r#"
id = "print-agent"
name = "Print Agent"
agent_type = "cli"
[cli]
executor = "codex"
prompt_template = "Handle {event} {form_key}/{record_id}"
[cli.env_vars]
OPENPR_API_URL = "http://localhost:3000"
"#,
        )
        .expect("parse agent");

        let payload = json!({
            "event": "form.record.created",
            "payload": {
                "envelope": {
                    "version": "openpr.event.v1",
                    "event_type": "form.record.created",
                    "project_id": "project-1",
                    "metadata": {
                        "form_id": "form-1",
                        "form_key": "order",
                        "record_id": "record-1"
                    }
                }
            }
        });
        let prompt = build_cli_prompt(&agent, &payload, "unknown");

        assert!(prompt.starts_with("Handle form.record.created order/record-1"));
        assert!(prompt.contains("forms.get"));
        assert!(prompt.contains("form_records.get"));
        assert!(prompt.contains("events.tail"));
        assert!(prompt.contains("Universal Form Context"));
        assert!(prompt.contains("project_id=\"project-1\""));
        assert!(prompt.contains("form_id=\"form-1\""));
        assert!(prompt.contains("form_key=\"order\""));
        assert!(prompt.contains("record_id=\"record-1\""));
    }

    #[test]
    fn format_message_replaces_form_placeholders() {
        let agent: crate::config::AgentConfig = toml::from_str(
            r#"
id = "printer"
name = "Printer"
agent_type = "webhook"
message_template = "{event} {project_id} {form_key} {record_id}"
"#,
        )
        .expect("parse agent");
        let payload = json!({
            "payload": {
                "envelope": {
                    "version": "openpr.event.v1",
                    "event_type": "print_job.created",
                    "project_id": "project-1",
                    "metadata": {
                        "form_key": "print_job",
                        "record_id": "record-1"
                    }
                }
            }
        });

        assert_eq!(
            format_message(&agent, &payload),
            "print_job.created project-1 print_job record-1"
        );
    }

    #[test]
    fn build_cli_prompt_stays_free_of_invocation_instructions() {
        let agent: crate::config::AgentConfig = toml::from_str(
            r#"
id = "bot-1"
name = "CLI"
agent_type = "cli"
[cli]
executor = "codex"
prompt_template = "Fix issue {issue_id}: {title}"
mcp_instructions = "Read issue {issue_id} before writing."
"#,
        )
        .expect("parse agent");

        let payload = json!({
            "data": {
                "issue": {
                    "id": "issue-1",
                    "title": "No invocation"
                }
            }
        });
        let issue_id = extract_issue_id(&payload).expect("issue id");
        let prompt = build_cli_prompt(&agent, &payload, &issue_id);

        assert!(!prompt.contains("Invocation Ledger"));
        assert!(!prompt.contains("invocations.complete"));
    }

    /// A delivery shaped like the one the Sylvode API sends for `issue.assigned` to a bot
    /// (`apps/api/src/webhook_trigger.rs`, `build_payload` and `fetch_issue_payload`).
    fn issue_assigned_delivery() -> serde_json::Value {
        json!({
            "id": "6f1d1f43-0b8e-4c8a-9a51-6d0e2f0c1a11",
            "event": "issue.assigned",
            "timestamp": "2026-10-05T12:00:00+00:00",
            "workspace": {"id": "2b0c6a59-3d5e-4a43-8f43-6a1c7f2b9e01", "name": "Acme"},
            "project": {"id": "8d9e2c1b-7f6a-4e5d-9c3b-2a1f0e9d8c7b", "name": "Billing", "key": "BIL"},
            "actor": {"id": "a1", "name": "Alice", "email": "alice@example.com", "entity_type": "human"},
            "data": {
                "issue": {
                    "id": "4c3b2a19-0f8e-4d7c-b6a5-948372615e0d",
                    "key": "BIL-4C3B2A19",
                    "title": "Invoice totals are rounded twice",
                    "description": "",
                    "state": "todo",
                    "priority": "high",
                    "assignee_ids": ["b0b0b0b0-0000-4000-8000-000000000001"],
                    "label_ids": [],
                    "sprint_id": null,
                    "created_at": "2026-10-05T11:00:00+00:00",
                    "updated_at": "2026-10-05T12:00:00+00:00"
                },
                "changes": {"assignee_ids": {"old": [], "new": ["b0b0b0b0-0000-4000-8000-000000000001"]}}
            },
            "bot_context": {
                "is_bot_task": true,
                "bot_id": "b0b0b0b0-0000-4000-8000-000000000001",
                "bot_name": "Fixer",
                "bot_agent_type": "cli",
                "trigger_reason": "assigned",
                "webhook_id": "0e0e0e0e-0000-4000-8000-000000000002"
            }
        })
    }

    #[test]
    fn format_message_fills_the_default_template() {
        let agent: crate::config::AgentConfig =
            toml::from_str("id = 'o1'\nname = 'OpenClaw'\nagent_type = 'openclaw'").expect("parse agent");

        assert_eq!(
            format_message(&agent, &issue_assigned_delivery()),
            "[Billing] issue.assigned: BIL-4C3B2A19 Invoice totals are rounded twice\nAlice | Trigger: assigned"
        );
    }

    #[test]
    fn build_cli_prompt_fills_the_default_template() {
        let agent: crate::config::AgentConfig =
            toml::from_str("id = 'c1'\nname = 'CLI'\nagent_type = 'cli'\n[cli]\nexecutor = 'codex'")
                .expect("parse agent");
        let payload = issue_assigned_delivery();
        let issue_id = extract_issue_id(&payload).expect("issue id");

        assert_eq!(
            build_cli_prompt(&agent, &payload, &issue_id),
            "Fix issue 4c3b2a19-0f8e-4d7c-b6a5-948372615e0d: Invoice totals are rounded twice\nContext: assigned"
        );
    }

    /// A value taken from the payload is inserted as it is: a placeholder inside it is not
    /// expanded again, so a title cannot pull other fields into the message.
    #[test]
    fn templates_do_not_expand_placeholders_inside_payload_values() {
        let agent: crate::config::AgentConfig = toml::from_str(
            "id = 'o1'\nname = 'OpenClaw'\nagent_type = 'openclaw'\nmessage_template = '{title} by {actor}'",
        )
        .expect("parse agent");
        let mut payload = issue_assigned_delivery();
        *payload.pointer_mut("/data/issue/title").expect("fixture has a title") = json!("see {actor} and {issue_id}");

        assert_eq!(format_message(&agent, &payload), "see {actor} and {issue_id} by Alice");
    }

    fn custom_agent(args: &str, message_template: &str) -> crate::config::AgentConfig {
        toml::from_str(&format!(
            "id = 'logger'\nname = 'Logger'\nagent_type = 'custom'\nmessage_template = '{message_template}'\n\
             [custom]\ncommand = 'printf'\nargs = {args}\n"
        ))
        .expect("parse agent")
    }

    fn titled(title: &str) -> serde_json::Value {
        let mut payload = issue_assigned_delivery();
        *payload.pointer_mut("/data/issue/title").expect("fixture has a title") = json!(title);
        payload
    }

    /// `{message}` in `args` is replaced by the rendered message, and the message is then not
    /// appended a second time.
    #[cfg(unix)]
    #[tokio::test]
    async fn custom_args_substitute_the_message_placeholder() {
        let agent = custom_agent(r#"["[%s]", "{message}"]"#, "{key} {title}");
        let out = dispatch(&base_config(), &Background::new(1), &agent, &issue_assigned_delivery()).await;
        assert_eq!(out, "[BIL-4C3B2A19 Invoice totals are rounded twice]");
    }

    /// Substituted text stays one argument, whatever it contains, and is never split or
    /// interpreted by a shell.
    #[cfg(unix)]
    #[tokio::test]
    async fn custom_args_keep_a_substituted_message_as_one_argument() {
        let agent = custom_agent(r#"["[%s]", "--label={message}"]"#, "{title}");
        let out = dispatch(
            &base_config(),
            &Background::new(1),
            &agent,
            &titled("a b \"c\" $(id); d"),
        )
        .await;
        assert_eq!(out, "[--label=a b \"c\" $(id); d]");
    }

    /// Without the placeholder the message is appended as the last argument, as before.
    #[cfg(unix)]
    #[tokio::test]
    async fn custom_args_without_placeholder_append_the_message() {
        let agent = custom_agent(r#"["[%s]"]"#, "{key}");
        let out = dispatch(&base_config(), &Background::new(1), &agent, &issue_assigned_delivery()).await;
        assert_eq!(out, "[BIL-4C3B2A19]");
    }

    /// Payload text cannot become an option: an argument that starts with `-` only because of
    /// the substituted message is refused unless an earlier argument is `--`.
    #[cfg(unix)]
    #[tokio::test]
    async fn custom_args_refuse_a_message_that_would_start_an_option() {
        let config = base_config();
        let payload = titled("-v x");

        let substituted = custom_agent(r#"["[%s]", "{message}"]"#, "{title}");
        let out = dispatch(&config, &Background::new(1), &substituted, &payload).await;
        assert!(out.starts_with("custom_error:"), "{out}");
        assert!(out.contains("starts with `-`"), "{out}");

        let appended = custom_agent(r#"["[%s]"]"#, "{title}");
        let out = dispatch(&config, &Background::new(1), &appended, &payload).await;
        assert!(out.starts_with("custom_error:"), "{out}");

        let separated = custom_agent(r#"["[%s]", "--", "{message}"]"#, "{title}");
        assert_eq!(
            dispatch(&config, &Background::new(1), &separated, &payload).await,
            "[--][-v x]"
        );
    }

    /// `args` of an `openclaw` agent go between the command and `--channel`, so
    /// `openclaw message send` is written as a program plus two arguments.
    #[cfg(unix)]
    #[tokio::test]
    async fn openclaw_args_precede_the_channel_options() {
        let agent: crate::config::AgentConfig = toml::from_str(
            "id = 'david'\nname = 'David'\nagent_type = 'openclaw'\nmessage_template = '{key}'\n\
             [openclaw]\ncommand = 'printf'\nargs = ['[%s]']\nchannel = 'signal'\ntarget = 'uuid:u1'\n",
        )
        .expect("parse agent");
        let out = dispatch(&base_config(), &Background::new(1), &agent, &issue_assigned_delivery()).await;
        assert_eq!(
            out,
            "ok: [--channel][signal][--target][uuid:u1][--message][BIL-4C3B2A19]"
        );
    }

    /// The same for an `openprx` agent that sends through a command.
    #[cfg(unix)]
    #[tokio::test]
    async fn openprx_args_precede_the_channel_options() {
        let agent: crate::config::AgentConfig = toml::from_str(
            "id = 'vano'\nname = 'Vano'\nagent_type = 'openprx'\nmessage_template = '{key}'\n\
             [openprx]\ncommand = 'sh'\nargs = ['-c', 'test \"$*\" = \"--channel signal --target t --message BIL-4C3B2A19\"', 'sh']\n\
             target = 't'\n",
        )
        .expect("parse agent");
        let out = dispatch(&base_config(), &Background::new(1), &agent, &issue_assigned_delivery()).await;
        assert_eq!(out, "ok");
    }

    #[tokio::test]
    async fn cli_path_is_blocked_when_feature_disabled() {
        let config = base_config();
        let agent: crate::config::AgentConfig = toml::from_str(
            r#"
id = "a1"
name = "CLI"
agent_type = "cli"
[cli]
executor = "codex"
"#,
        )
        .expect("parse agent");

        let result = dispatch(&config, &Background::new(1), &agent, &json!({})).await;
        assert!(result.contains("disabled"));
    }

    #[tokio::test]
    async fn legacy_four_dispatch_paths_are_not_blocked_by_new_flags() {
        let config = base_config();
        let payload = json!({"event":"issue.created"});

        let openclaw: crate::config::AgentConfig =
            toml::from_str("id='o1'\nname='OpenClaw'\nagent_type='openclaw'").unwrap();
        let openprx: crate::config::AgentConfig =
            toml::from_str("id='o2'\nname='OpenPRX'\nagent_type='openprx'").unwrap();
        let webhook: crate::config::AgentConfig =
            toml::from_str("id='o3'\nname='Webhook'\nagent_type='webhook'").unwrap();
        let custom: crate::config::AgentConfig = toml::from_str("id='o4'\nname='Custom'\nagent_type='custom'").unwrap();

        assert!(
            dispatch(&config, &Background::new(1), &openclaw, &payload)
                .await
                .contains("missing openclaw config")
        );
        assert!(
            dispatch(&config, &Background::new(1), &openprx, &payload)
                .await
                .contains("missing openprx config")
        );
        assert!(
            dispatch(&config, &Background::new(1), &webhook, &payload)
                .await
                .contains("missing webhook config")
        );
        assert!(
            dispatch(&config, &Background::new(1), &custom, &payload)
                .await
                .contains("missing custom config")
        );
    }
}
