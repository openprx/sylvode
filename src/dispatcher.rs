use crate::{callback, config::AgentConfig, config::CliAgentConfig, config::Config};
use serde_json::Value;
use std::process::Stdio;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

const DEFAULT_SUBPROCESS_TIMEOUT_SECS: u64 = 60;

#[allow(clippy::doc_markdown, clippy::literal_string_with_formatting_args)]
const DEFAULT_MCP_INSTRUCTIONS: &str = r#"## MCP Integration

You have OpenPR MCP tools available. Use them to get full issue context before working:

1. Call `work_items.get` with work_item_id="{issue_id}" to read full issue details (title, description, state, priority, assignee)
2. Call `comments.list` with work_item_id="{issue_id}" to read all comments and discussion
3. Call `work_items.list_labels` with work_item_id="{issue_id}" to read labels

After completing your work:

4. Call `comments.create` with work_item_id="{issue_id}" to post a summary of what you did
5. Call `work_items.update` with work_item_id="{issue_id}" and state="done" if the fix is complete
6. If an invocation_id is provided below, call `invocations.complete` with that invocation_id and a result object that summarizes the outcome. If the task cannot be completed, call `invocations.fail` with the invocation_id, error_message, and any useful result details."#;

#[allow(clippy::doc_markdown, clippy::literal_string_with_formatting_args)]
const DEFAULT_FORM_MCP_INSTRUCTIONS: &str = r#"## MCP Integration

You have OpenPR MCP tools available. This webhook is linked to a universal-form event:

1. Call `forms.get` with form_id="{form_id}" when a form_id is available, or call `forms.list` with project_id="{project_id}" and match form_key="{form_key}".
2. Call `form_records.get` with record_id="{record_id}" when a record_id is available.
3. Call `events.tail` with form_id="{form_id}" or record_id="{record_id}" to inspect the recent business-event stream.
4. If an invocation_id is provided below, call `invocations.complete` with that invocation_id and a result object containing status, summary, form_key, and record_id. If the task cannot be completed, call `invocations.fail` with the invocation_id, error_message, and useful result details."#;

pub async fn dispatch(config: &Config, agent: &AgentConfig, payload: &Value) -> String {
    match agent.agent_type.as_str() {
        "openclaw" => dispatch_openclaw(config, agent, payload).await,
        "openprx" => dispatch_openprx(config, agent, payload).await,
        "webhook" => dispatch_webhook(config, agent, payload).await,
        "custom" => dispatch_custom(config, agent, payload).await,
        "cli" => dispatch_cli(config, agent, payload).await,
        other => format!("unknown agent type: {other}"),
    }
}

async fn dispatch_cli(config: &Config, agent: &AgentConfig, payload: &Value) -> String {
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
            .map(|d| d.as_millis())
            .unwrap_or(0)
    );
    let prompt = build_cli_prompt(agent, payload, &issue_id);
    let executor = cfg.executor.clone();
    let invocation_id = extract_invocation_id(payload).map(ToString::to_string);

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
    if config.callback_enabled()
        && let Some(invocation_id) = invocation_id.as_deref()
    {
        let result = serde_json::json!({
            "run_id": &run_id,
            "issue_id": &issue_id,
            "executor": &executor,
            "status": "started",
            "summary": "cli task started",
        });
        if let Err(e) = callback::send_invocation_status(
            cfg,
            invocation_id,
            "started",
            result,
            None,
            config.runtime.http_timeout_secs,
        )
        .await
        {
            tracing::warn!("invocation start callback failed: {e}");
        }
    }

    // 2. Clone what the background task needs
    let bg_cfg = cfg.clone();
    let bg_issue_id = issue_id.clone();
    let bg_run_id = run_id.clone();
    let bg_executor = executor.clone();
    let bg_invocation_id = invocation_id.clone();
    let callback_enabled = config.callback_enabled();
    let http_timeout_secs = config.runtime.http_timeout_secs;

    // 3. Spawn CLI process in background — do NOT await
    tokio::spawn(async move {
        tracing::info!(executor = %bg_executor, "CLI agent spawned");

        let run = run_cli_executor(&bg_cfg, &prompt).await;

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

        if callback_enabled && let Some(invocation_id) = bg_invocation_id.as_deref() {
            let summary = if run.status == "success" {
                "cli execution completed".to_string()
            } else {
                format!("cli execution {}", run.status)
            };
            let result = serde_json::json!({
                "run_id": &bg_run_id,
                "issue_id": &bg_issue_id,
                "executor": &bg_executor,
                "status": &run.status,
                "summary": &summary,
                "exit_code": run.exit_code,
                "duration_ms": run.duration_ms,
                "stdout_tail": &run.stdout_tail,
                "stderr_tail": &run.stderr_tail,
            });
            let error_message = if run.status == "success" { None } else { Some(summary) };
            if let Err(e) = callback::send_invocation_status(
                &bg_cfg,
                invocation_id,
                &run.status,
                result,
                error_message,
                http_timeout_secs,
            )
            .await
            {
                tracing::warn!("invocation final callback failed: {e}");
            }
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
                .map(|d| d.as_millis())
                .unwrap_or(0)
        )
    });
    let prompt = build_cli_prompt(agent, payload, &issue_id);
    let invocation_id = extract_invocation_id(payload).map(ToString::to_string);

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
    if config.callback_enabled()
        && let Some(invocation_id) = invocation_id.as_deref()
    {
        let result = serde_json::json!({
            "run_id": &run_id,
            "issue_id": &issue_id,
            "executor": &cfg.executor,
            "status": "started",
            "summary": "cli task started",
        });
        if let Err(e) = callback::send_invocation_status(
            cfg,
            invocation_id,
            "started",
            result,
            None,
            config.runtime.http_timeout_secs,
        )
        .await
        {
            tracing::warn!("invocation start callback failed: {e}");
        }
    }

    let run = run_cli_executor(cfg, &prompt).await;

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
        if let Some(invocation_id) = invocation_id.as_deref() {
            let result = serde_json::json!({
                "run_id": &run_id,
                "issue_id": &issue_id,
                "executor": &cfg.executor,
                "status": &run.status,
                "summary": &summary,
                "exit_code": run.exit_code,
                "duration_ms": run.duration_ms,
                "stdout_tail": &run.stdout_tail,
                "stderr_tail": &run.stderr_tail,
            });
            let error_message = if run.status == "success" {
                None
            } else {
                Some(summary.clone())
            };
            if let Err(e) = callback::send_invocation_status(
                cfg,
                invocation_id,
                &run.status,
                result,
                error_message,
                config.runtime.http_timeout_secs,
            )
            .await
            {
                tracing::warn!("invocation final callback failed: {e}");
            }
        }

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

async fn run_cli_executor(cfg: &CliAgentConfig, prompt: &str) -> CliRunResult {
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
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .env_remove("CLAUDECODE")
        .env_remove("CLAUDE_CODE_ENTRYPOINT");
    if let Some(dir) = &cfg.workdir {
        cmd.current_dir(dir);
    }
    for (key, value) in &cfg.env_vars {
        cmd.env(key, value);
    }

    let started = Instant::now();
    let child = match cmd.spawn() {
        Ok(c) => c,
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

    let timeout_secs = cfg.timeout_secs;
    let max_output_chars = cfg.max_output_chars;
    let output_result = tokio::time::timeout(Duration::from_secs(timeout_secs), child.wait_with_output()).await;

    match output_result {
        Ok(Ok(output)) => {
            let stdout = String::from_utf8_lossy(&output.stdout).to_string();
            let stderr = String::from_utf8_lossy(&output.stderr).to_string();
            CliRunResult {
                status: if output.status.success() {
                    "success".into()
                } else {
                    "failed".into()
                },
                exit_code: output.status.code(),
                duration_ms: started.elapsed().as_millis(),
                stdout_tail: tail_chars(&stdout, max_output_chars),
                stderr_tail: tail_chars(&stderr, max_output_chars),
            }
        }
        Ok(Err(e)) => CliRunResult {
            status: "failed".into(),
            exit_code: None,
            duration_ms: started.elapsed().as_millis(),
            stdout_tail: String::new(),
            stderr_tail: format!("wait failed: {e}"),
        },
        Err(_) => CliRunResult {
            status: "timeout".into(),
            exit_code: None,
            duration_ms: started.elapsed().as_millis(),
            stdout_tail: String::new(),
            stderr_tail: format!("timeout after {timeout_secs}s"),
        },
    }
}

fn build_executor_command(
    executor: &str,
    prompt: &str,
    mcp_config_path: Option<&str>,
) -> Result<(&'static str, Vec<String>), String> {
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

#[allow(clippy::doc_markdown, clippy::literal_string_with_formatting_args)]
fn build_cli_prompt(agent: &AgentConfig, payload: &Value, issue_id: &str) -> String {
    let base = agent
        .cli
        .as_ref()
        .and_then(|c| c.prompt_template.as_deref())
        // Template placeholders — not format args
        .unwrap_or("Fix issue ISSUE_ID: TITLE\nContext: REASON");

    let title = extract_issue_title(payload).unwrap_or("untitled");
    let reason = extract_trigger_reason(payload).unwrap_or("unknown");
    let event = extract_event(payload).unwrap_or("unknown");
    let project_id = extract_project_id(payload).unwrap_or("<project_id>");
    let form_id = extract_form_id(payload).unwrap_or("<form_id>");
    let form_key = extract_form_key(payload).unwrap_or("<form_key>");
    let record_id = extract_record_id(payload).unwrap_or("<record_id>");
    let connector_kind = extract_connector_kind(payload).unwrap_or("<connector_kind>");

    let user_prompt = base
        .replace("{issue_id}", issue_id)
        .replace("{title}", title)
        .replace("{reason}", reason)
        .replace("{event}", event)
        .replace("{project_id}", project_id)
        .replace("{form_id}", form_id)
        .replace("{form_key}", form_key)
        .replace("{record_id}", record_id)
        .replace("{connector_kind}", connector_kind);

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

    let instructions = mcp_instructions
        .replace("{issue_id}", issue_id)
        .replace("{project_id}", project_id)
        .replace("{form_id}", form_id)
        .replace("{form_key}", form_key)
        .replace("{record_id}", record_id)
        .replace("{event}", event)
        .replace("{connector_kind}", connector_kind);

    let mut prompt = format!("{user_prompt}\n\n{instructions}");
    if let Some(project_context_instructions) = build_project_context_instructions(payload) {
        prompt.push_str("\n\n");
        prompt.push_str(&project_context_instructions);
    }
    if let Some(ledger_instructions) = build_invocation_ledger_instructions(payload) {
        prompt.push_str("\n\n");
        prompt.push_str(&ledger_instructions);
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

pub fn extract_issue_id(payload: &Value) -> Option<String> {
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

fn extract_invocation_id(payload: &Value) -> Option<&str> {
    payload
        .get("invocation_id")
        .and_then(Value::as_str)
        .or_else(|| {
            payload
                .get("payload")
                .and_then(|p| p.get("invocation_id"))
                .and_then(Value::as_str)
        })
        .filter(|value| !value.is_empty())
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

fn extract_connector_kind(payload: &Value) -> Option<&str> {
    string_field(payload, "connector_kind").or_else(|| nested_string_field(payload, &["payload", "connector_kind"]))
}

fn is_form_event(payload: &Value) -> bool {
    extract_form_key(payload).is_some()
        || extract_form_id(payload).is_some()
        || extract_event(payload).is_some_and(|event| event.starts_with("form.") || event.starts_with("print_job."))
}

fn build_project_context_instructions(payload: &Value) -> Option<String> {
    let project_id = extract_project_id(payload)?;
    Some(format!(
        "## Project MCP Context\n\nBefore changing state or posting final output, call `context.get_project` with project_id=\"{project_id}\" to read the project type, resources, connectors, governance context, and effective agent policy. When your MCP client supports project-scoped discovery, call `tools/list` with project_id=\"{project_id}\" and only use tools enabled by that project policy."
    ))
}

fn build_invocation_ledger_instructions(payload: &Value) -> Option<String> {
    let invocation_id = extract_invocation_id(payload)?;
    let mut instructions = format!(
        "## Invocation Ledger\n\nThis task is linked to OpenPR invocation `{invocation_id}`. Call `invocations.report_progress` when you start meaningful work. Before finishing, call `invocations.complete` with invocation_id=\"{invocation_id}\" and a result object containing status, summary, and changed_files if applicable. If you cannot complete the task, call `invocations.fail` with invocation_id=\"{invocation_id}\" and an error_message."
    );

    if let Some(project_id) = extract_project_id(payload) {
        instructions.push_str("\nUse project_id=\"");
        instructions.push_str(project_id);
        instructions.push_str("\" when listing project context or invocation records.");
    }

    Some(instructions)
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
    let connector_kind = extract_connector_kind(payload).unwrap_or("<connector_kind>");

    Some(format!(
        "## Universal Form Context\n\nThis is OpenPR universal-form event `{event}` for project_id=\"{project_id}\", form_id=\"{form_id}\", form_key=\"{form_key}\", record_id=\"{record_id}\", connector_kind=\"{connector_kind}\". Treat custom fields as schema-defined business data: inspect the form schema before assuming field meaning, preserve decimal strings for amount fields, and write any operational result through MCP or the invocation ledger instead of mutating local webhook state."
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

async fn run_subprocess_with_timeout(
    program: &str,
    args: &[&str],
    timeout_secs: u64,
) -> Result<std::process::Output, String> {
    let child = tokio::process::Command::new(program)
        .args(args)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .map_err(|e| format!("spawn {program} failed: {e}"))?;

    tokio::time::timeout(Duration::from_secs(timeout_secs), child.wait_with_output())
        .await
        .map_err(|_| format!("subprocess {program} timed out after {timeout_secs}s"))?
        .map_err(|e| format!("subprocess {program} wait failed: {e}"))
}

async fn dispatch_openclaw(config: &Config, agent: &AgentConfig, payload: &Value) -> String {
    let Some(cfg) = &agent.openclaw else {
        return "missing openclaw config".into();
    };

    let message = format_message(agent, payload);
    let timeout = config.runtime.http_timeout_secs.max(DEFAULT_SUBPROCESS_TIMEOUT_SECS);

    match run_subprocess_with_timeout(
        &cfg.command,
        &[
            "--channel",
            &cfg.channel,
            "--target",
            &cfg.target,
            "--message",
            &message,
        ],
        timeout,
    )
    .await
    {
        Ok(o) if o.status.success() => {
            let out = String::from_utf8_lossy(&o.stdout);
            tracing::info!("openclaw ok: {}", out.trim());
            format!("ok: {}", out.trim())
        }
        Ok(o) => {
            let err = String::from_utf8_lossy(&o.stderr);
            tracing::error!("openclaw failed: {}", err.trim());
            format!("error: {}", err.trim())
        }
        Err(e) => {
            tracing::error!("openclaw exec failed: {e}");
            format!("exec_error: {e}")
        }
    }
}

async fn dispatch_openprx(config: &Config, agent: &AgentConfig, payload: &Value) -> String {
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

        return match run_subprocess_with_timeout(
            command,
            &[
                "--channel",
                &cfg.channel,
                "--target",
                &cfg.target,
                "--message",
                &message,
            ],
            timeout,
        )
        .await
        {
            Ok(o) if o.status.success() => {
                tracing::info!("openprx cli ok");
                "ok".into()
            }
            Ok(o) => {
                let err = String::from_utf8_lossy(&o.stderr);
                tracing::error!("openprx cli failed: {}", err.trim());
                format!("error: {}", err.trim())
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

async fn dispatch_custom(config: &Config, agent: &AgentConfig, payload: &Value) -> String {
    let Some(cfg) = &agent.custom else {
        return "missing custom config".into();
    };

    let message = format_message(agent, payload);
    let timeout = config.runtime.http_timeout_secs.max(DEFAULT_SUBPROCESS_TIMEOUT_SECS);

    // Build args: use configured args or default to passing message as single arg
    let configured_args: Vec<String> = cfg.args.clone().unwrap_or_default();
    let mut final_args: Vec<&str> = configured_args.iter().map(String::as_str).collect();
    final_args.push(&message);

    match run_subprocess_with_timeout(&cfg.command, &final_args, timeout).await {
        Ok(o) => String::from_utf8_lossy(&o.stdout).to_string(),
        Err(e) => format!("custom_error: {e}"),
    }
}

#[allow(clippy::doc_markdown, clippy::literal_string_with_formatting_args)]
fn format_message(agent: &AgentConfig, payload: &Value) -> String {
    let tmpl = agent.message_template.as_deref().unwrap_or(
        // Template placeholders — not format args
        "[TPL_PROJECT] TPL_EVENT: TPL_KEY TPL_TITLE\nTPL_ACTOR | Trigger: TPL_REASON",
    );

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
    let connector_kind = extract_connector_kind(payload).unwrap_or("");

    tmpl.replace("{event}", event)
        .replace("{title}", title)
        .replace("{key}", issue_key)
        .replace("{reason}", reason)
        .replace("{actor}", actor)
        .replace("{workspace}", workspace)
        .replace("{project}", project)
        .replace("{state}", state)
        .replace("{priority}", priority)
        .replace("{issue_id}", &issue_id)
        .replace("{project_id}", project_id)
        .replace("{form_id}", form_id)
        .replace("{form_key}", form_key)
        .replace("{record_id}", record_id)
        .replace("{connector_kind}", connector_kind)
        .replace("{url}", &format!("issue/{issue_id}"))
}

#[cfg(test)]
#[allow(clippy::literal_string_with_formatting_args)]
mod tests {
    use super::{
        build_cli_prompt, build_executor_command, dispatch, extract_issue_id, format_message,
        outbound_signature_header_value,
    };
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
    fn build_cli_prompt_includes_invocation_ledger_instructions_when_available() {
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
        assert!(prompt.contains("OpenPR invocation `invocation-1`"));
        assert!(prompt.contains("invocations.report_progress"));
        assert!(prompt.contains("invocations.complete"));
        assert!(prompt.contains("invocations.fail"));
        assert!(prompt.contains("project_id=\"project-1\""));
    }

    #[test]
    fn build_cli_prompt_uses_form_context_for_connector_envelope() {
        let agent: crate::config::AgentConfig = toml::from_str(
            r#"
id = "print-agent"
name = "Print Agent"
agent_type = "cli"
[cli]
executor = "codex"
prompt_template = "Handle {event} {form_key}/{record_id} via {connector_kind}"
[cli.env_vars]
OPENPR_API_URL = "http://localhost:3000"
"#,
        )
        .expect("parse agent");

        let payload = json!({
            "event": "form.record.created",
            "invocation_id": "invocation-1",
            "connector_kind": "print",
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

        assert!(prompt.starts_with("Handle form.record.created order/record-1 via print"));
        assert!(prompt.contains("forms.get"));
        assert!(prompt.contains("form_records.get"));
        assert!(prompt.contains("events.tail"));
        assert!(prompt.contains("Universal Form Context"));
        assert!(prompt.contains("project_id=\"project-1\""));
        assert!(prompt.contains("form_id=\"form-1\""));
        assert!(prompt.contains("form_key=\"order\""));
        assert!(prompt.contains("record_id=\"record-1\""));
        assert!(prompt.contains("connector_kind=\"print\""));
        assert!(prompt.contains("OpenPR invocation `invocation-1`"));
    }

    #[test]
    fn format_message_replaces_form_placeholders() {
        let agent: crate::config::AgentConfig = toml::from_str(
            r#"
id = "printer"
name = "Printer"
agent_type = "webhook"
message_template = "{event} {project_id} {form_key} {record_id} {connector_kind}"
"#,
        )
        .expect("parse agent");
        let payload = json!({
            "connector_kind": "print",
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
            "print_job.created project-1 print_job record-1 print"
        );
    }

    #[test]
    fn build_cli_prompt_omits_invocation_ledger_instructions_without_invocation_id() {
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

        let result = dispatch(&config, &agent, &json!({})).await;
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
            dispatch(&config, &openclaw, &payload)
                .await
                .contains("missing openclaw config")
        );
        assert!(
            dispatch(&config, &openprx, &payload)
                .await
                .contains("missing openprx config")
        );
        assert!(
            dispatch(&config, &webhook, &payload)
                .await
                .contains("missing webhook config")
        );
        assert!(
            dispatch(&config, &custom, &payload)
                .await
                .contains("missing custom config")
        );
    }
}
