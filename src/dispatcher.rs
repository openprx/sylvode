use crate::{callback, config::AgentConfig};
use serde_json::Value;
use std::process::Stdio;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

pub async fn dispatch(agent: &AgentConfig, payload: &Value) -> String {
    match agent.agent_type.as_str() {
        "openclaw" => dispatch_openclaw(agent, payload).await,
        "openprx" => dispatch_openprx(agent, payload).await,
        "webhook" => dispatch_webhook(agent, payload).await,
        "custom" => dispatch_custom(agent, payload).await,
        "cli" => dispatch_cli(agent, payload).await,
        other => format!("unknown agent type: {}", other),
    }
}

async fn dispatch_cli(agent: &AgentConfig, payload: &Value) -> String {
    let cfg = match &agent.cli {
        Some(c) => c,
        None => return "missing cli config".into(),
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

    let start_state = cfg.update_state_on_start.clone();
    if start_state.is_some() {
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
        if let Err(e) = callback::send_callback(cfg, &start_payload).await {
            tracing::warn!("start callback failed: {}", e);
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

    let callback_payload = callback::build_callback_payload(
        issue_id,
        run_id,
        cfg.executor.clone(),
        run.status.clone(),
        summary,
        run.exit_code,
        run.duration_ms,
        run.stdout_tail.clone(),
        run.stderr_tail.clone(),
        final_state,
    );

    if let Err(e) = callback::send_callback(cfg, &callback_payload).await {
        tracing::warn!("final callback failed: {}", e);
    }

    format!(
        "cli {} exit={:?} duration_ms={}",
        run.status, run.exit_code, run.duration_ms
    )
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
            }
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
                stderr_tail: format!("spawn failed: {}", e),
            }
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
            stderr_tail: format!("wait failed: {}", e),
        },
        Err(_) => CliRunResult {
            status: "timeout".into(),
            exit_code: None,
            duration_ms: started.elapsed().as_millis(),
            stdout_tail: String::new(),
            stderr_tail: format!("timeout after {}s", timeout_secs),
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
        _ => Err(format!("executor not allowed: {}", executor)),
    }
}

fn build_cli_prompt(agent: &AgentConfig, payload: &Value, issue_id: &str) -> String {
    let base = agent
        .cli
        .as_ref()
        .and_then(|c| c.prompt_template.as_deref())
        .unwrap_or("Fix issue {issue_id}: {title}\nContext: {reason}");

    let title = payload
        .get("data")
        .and_then(|d| d.get("issue"))
        .and_then(|i| i.get("title"))
        .and_then(|v| v.as_str())
        .unwrap_or("untitled");
    let reason = payload
        .get("bot_context")
        .and_then(|bc| bc.get("trigger_reason"))
        .and_then(|v| v.as_str())
        .unwrap_or("unknown");

    base.replace("{issue_id}", issue_id)
        .replace("{title}", title)
        .replace("{reason}", reason)
}

fn extract_issue_id(payload: &Value) -> Option<String> {
    let issue_id = payload
        .get("data")
        .and_then(|d| d.get("issue"))
        .and_then(|i| i.get("id"))?;

    if let Some(id) = issue_id.as_str() {
        Some(id.to_string())
    } else {
        issue_id.as_i64().map(|n| n.to_string())
    }
}

fn tail_chars(input: &str, max_chars: usize) -> String {
    if input.chars().count() <= max_chars {
        return input.to_string();
    }
    input
        .chars()
        .rev()
        .take(max_chars)
        .collect::<String>()
        .chars()
        .rev()
        .collect()
}

async fn dispatch_openclaw(agent: &AgentConfig, payload: &Value) -> String {
    let cfg = match &agent.openclaw {
        Some(c) => c,
        None => return "missing openclaw config".into(),
    };

    let message = format_message(agent, payload);

    let output = tokio::process::Command::new("sh")
        .arg("-c")
        .arg(format!(
            "{} --channel {} --target \"{}\" --message \"{}\"",
            cfg.command,
            cfg.channel,
            cfg.target,
            message.replace('\\', "\\\\").replace('"', "\\\"")
        ))
        .output()
        .await;

    match output {
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
            tracing::error!("exec failed: {}", e);
            format!("exec_error: {}", e)
        }
    }
}

async fn dispatch_openprx(agent: &AgentConfig, payload: &Value) -> String {
    let cfg = match &agent.openprx {
        Some(c) => c,
        None => return "missing openprx config".into(),
    };

    let message = format_message(agent, payload);

    if let Some(signal_api) = &cfg.signal_api {
        let account = cfg.account.as_deref().unwrap_or("");
        let url = format!("{}/api/v1/send/{}", signal_api.trim_end_matches('/'), account);

        let body = serde_json::json!({
            "recipients": [&cfg.target],
            "message": message
        });

        let client = reqwest::Client::new();
        return match client.post(&url).json(&body).send().await {
            Ok(resp) if resp.status().is_success() => {
                tracing::info!("openprx signal ok");
                "ok".into()
            }
            Ok(resp) => {
                let status = resp.status();
                let text = resp.text().await.unwrap_or_default();
                tracing::error!("openprx signal {}: {}", status, text);
                format!("error: {} {}", status, text)
            }
            Err(e) => {
                tracing::error!("openprx signal request failed: {}", e);
                format!("error: {}", e)
            }
        };
    }

    if let Some(command) = &cfg.command {
        let output = tokio::process::Command::new("sh")
            .arg("-c")
            .arg(format!(
                "{} --channel {} --target \"{}\" --message \"{}\"",
                command,
                cfg.channel,
                cfg.target,
                message.replace('\\', "\\\\").replace('"', "\\\"")
            ))
            .output()
            .await;

        return match output {
            Ok(o) if o.status.success() => {
                tracing::info!("openprx cli ok");
                "ok".into()
            }
            Ok(o) => {
                let err = String::from_utf8_lossy(&o.stderr);
                tracing::error!("openprx cli failed: {}", err.trim());
                format!("error: {}", err.trim())
            }
            Err(e) => format!("exec_error: {}", e),
        };
    }

    "openprx config needs either signal_api or command".into()
}

fn outbound_signature_header_value(payload: &Value, secret: Option<&str>) -> Option<String> {
    secret.and_then(|secret| {
        serde_json::to_vec(payload)
            .ok()
            .map(|bytes| format!("sha256={}", crate::signature::sign_payload(&bytes, secret)))
    })
}

async fn dispatch_webhook(agent: &AgentConfig, payload: &Value) -> String {
    let cfg = match &agent.webhook {
        Some(c) => c,
        None => return "missing webhook config".into(),
    };

    let mut request = reqwest::Client::new().post(&cfg.url).json(payload);
    if let Some(signature_value) = outbound_signature_header_value(payload, cfg.secret.as_deref()) {
        request = request.header(crate::signature::OUTBOUND_SIGNATURE_HEADER, signature_value);
    }

    match request.send().await {
        Ok(resp) => format!("webhook: {}", resp.status()),
        Err(e) => format!("webhook_error: {}", e),
    }
}

async fn dispatch_custom(agent: &AgentConfig, payload: &Value) -> String {
    let cfg = match &agent.custom {
        Some(c) => c,
        None => return "missing custom config".into(),
    };

    let message = format_message(agent, payload);
    let full_cmd = format!("{} \"{}\"", cfg.command, message.replace('"', "\\\""));

    match tokio::process::Command::new("sh")
        .arg("-c")
        .arg(&full_cmd)
        .output()
        .await
    {
        Ok(o) => String::from_utf8_lossy(&o.stdout).to_string(),
        Err(e) => format!("custom_error: {}", e),
    }
}

fn format_message(agent: &AgentConfig, payload: &Value) -> String {
    let tmpl = agent
        .message_template
        .as_deref()
        .unwrap_or("🔔 [{project}] {event}: {key} {title}\n👤 {actor} | Trigger: {reason}");

    let event = payload
        .get("event")
        .and_then(|v| v.as_str())
        .unwrap_or("unknown");
    let title = payload
        .get("data")
        .and_then(|d| d.get("issue"))
        .and_then(|i| i.get("title"))
        .and_then(|v| v.as_str())
        .unwrap_or("untitled");
    let issue_key = payload
        .get("data")
        .and_then(|d| d.get("issue"))
        .and_then(|i| i.get("key"))
        .and_then(|v| v.as_str())
        .unwrap_or("");
    let reason = payload
        .get("bot_context")
        .and_then(|bc| bc.get("trigger_reason"))
        .and_then(|v| v.as_str())
        .unwrap_or("unknown");
    let actor = payload
        .get("actor")
        .and_then(|a| a.get("name"))
        .and_then(|v| v.as_str())
        .unwrap_or("unknown");
    let workspace = payload
        .get("workspace")
        .and_then(|w| w.get("name"))
        .and_then(|v| v.as_str())
        .unwrap_or("");
    let project = payload
        .get("project")
        .and_then(|p| p.get("name"))
        .and_then(|v| v.as_str())
        .unwrap_or("");
    let state = payload
        .get("data")
        .and_then(|d| d.get("issue"))
        .and_then(|i| i.get("state"))
        .and_then(|v| v.as_str())
        .unwrap_or("");
    let priority = payload
        .get("data")
        .and_then(|d| d.get("issue"))
        .and_then(|i| i.get("priority"))
        .and_then(|v| v.as_str())
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
        .replace("{url}", &format!("issue/{}", issue_id))
}

#[cfg(test)]
mod tests {
    use super::{build_executor_command, extract_issue_id, outbound_signature_header_value};
    use serde_json::json;

    #[test]
    fn builds_outbound_signature_header_when_secret_exists() {
        let payload = json!({"event":"issue.created","value":1});
        let header = outbound_signature_header_value(&payload, Some("top-secret"));

        assert!(header.as_deref().unwrap_or_default().starts_with("sha256="));

        let expected_sig = crate::signature::sign_payload(
            serde_json::to_vec(&payload).unwrap().as_slice(),
            "top-secret",
        );
        assert_eq!(header, Some(format!("sha256={}", expected_sig)));
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
}
