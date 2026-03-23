use crate::{callback, config::AgentConfig, config::Config};
use serde_json::Value;
use std::process::Stdio;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

const DEFAULT_SUBPROCESS_TIMEOUT_SECS: u64 = 60;

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

    let report = execute_cli_task(config, agent, payload, None).await;
    format!(
        "cli {} run_id={} issue_id={}",
        report.status, report.run_id, report.issue_id
    )
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

    let start_state = cfg.update_state_on_start.clone();
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

    let run = run_cli_executor(
        &cfg.executor,
        cfg.workdir.as_deref(),
        &prompt,
        cfg.timeout_secs,
        cfg.max_output_chars,
    )
    .await;

    let final_state = callback::state_for_status(cfg, &run.status);
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

async fn run_cli_executor(
    executor: &str,
    workdir: Option<&str>,
    prompt: &str,
    timeout_secs: u64,
    max_output_chars: usize,
) -> CliRunResult {
    let (program, args) = match build_executor_command(executor, prompt) {
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
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    if let Some(dir) = workdir {
        cmd.current_dir(dir);
    }

    let started = Instant::now();
    let child = match cmd.spawn() {
        Ok(c) => c,
        Err(e) => {
            return CliRunResult {
                status: "failed".into(),
                exit_code: None,
                duration_ms: started.elapsed().as_millis(),
                stdout_tail: String::new(),
                stderr_tail: format!("spawn failed: {e}"),
            };
        }
    };

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

fn build_executor_command(executor: &str, prompt: &str) -> Result<(&'static str, Vec<String>), String> {
    match executor {
        "codex" => Ok(("codex", vec!["exec".into(), "--full-auto".into(), prompt.into()])),
        "claude-code" => Ok((
            "claude",
            vec![
                "--print".into(),
                "--permission-mode".into(),
                "bypassPermissions".into(),
                prompt.into(),
            ],
        )),
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

    let title = payload
        .get("data")
        .and_then(|d| d.get("issue"))
        .and_then(|i| i.get("title"))
        .and_then(Value::as_str)
        .unwrap_or("untitled");
    let reason = payload
        .get("bot_context")
        .and_then(|bc| bc.get("trigger_reason"))
        .and_then(Value::as_str)
        .unwrap_or("unknown");

    base.replace("{issue_id}", issue_id)
        .replace("{title}", title)
        .replace("{reason}", reason)
}

pub fn extract_issue_id(payload: &Value) -> Option<String> {
    let issue_id = payload
        .get("data")
        .and_then(|d| d.get("issue"))
        .and_then(|i| i.get("id"))?;

    issue_id
        .as_str()
        .map(ToString::to_string)
        .or_else(|| issue_id.as_i64().map(|n| n.to_string()))
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

    let event = payload.get("event").and_then(Value::as_str).unwrap_or("unknown");
    let title = payload
        .get("data")
        .and_then(|d| d.get("issue"))
        .and_then(|i| i.get("title"))
        .and_then(Value::as_str)
        .unwrap_or("untitled");
    let issue_key = payload
        .get("data")
        .and_then(|d| d.get("issue"))
        .and_then(|i| i.get("key"))
        .and_then(Value::as_str)
        .unwrap_or("");
    let reason = payload
        .get("bot_context")
        .and_then(|bc| bc.get("trigger_reason"))
        .and_then(Value::as_str)
        .unwrap_or("unknown");
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
    let state = payload
        .get("data")
        .and_then(|d| d.get("issue"))
        .and_then(|i| i.get("state"))
        .and_then(Value::as_str)
        .unwrap_or("");
    let priority = payload
        .get("data")
        .and_then(|d| d.get("issue"))
        .and_then(|i| i.get("priority"))
        .and_then(Value::as_str)
        .unwrap_or("");
    let issue_id = extract_issue_id(payload).unwrap_or_default();

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
        .replace("{url}", &format!("issue/{issue_id}"))
}

#[cfg(test)]
mod tests {
    use super::{build_executor_command, dispatch, extract_issue_id, outbound_signature_header_value};
    use crate::config::Config;
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
        let (_, args) = build_executor_command("codex", "fix it").expect("codex should be allowed");
        assert_eq!(args, vec!["exec", "--full-auto", "fix it"]);

        assert!(build_executor_command("bash", "rm -rf /").is_err());
    }

    #[test]
    fn extracts_issue_id_from_payload() {
        let payload = json!({"data": {"issue": {"id": "42"}}});
        assert_eq!(extract_issue_id(&payload).as_deref(), Some("42"));
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
