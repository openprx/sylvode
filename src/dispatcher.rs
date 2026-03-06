use crate::config::AgentConfig;
use serde_json::Value;

pub async fn dispatch(agent: &AgentConfig, payload: &Value) -> String {
    match agent.agent_type.as_str() {
        "openclaw" => dispatch_openclaw(agent, payload).await,
        "openprx" => dispatch_openprx(agent, payload).await,
        "webhook" => dispatch_webhook(agent, payload).await,
        "custom" => dispatch_custom(agent, payload).await,
        other => format!("unknown agent type: {}", other),
    }
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

    // Method 1: Direct Signal API call (preferred — no CLI dependency)
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

    // Method 2: CLI fallback
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

async fn dispatch_webhook(agent: &AgentConfig, payload: &Value) -> String {
    let cfg = match &agent.webhook {
        Some(c) => c,
        None => return "missing webhook config".into(),
    };

    let client = reqwest::Client::new();
    match client.post(&cfg.url).json(payload).send().await {
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
    let issue_id = payload
        .get("data")
        .and_then(|d| d.get("issue"))
        .and_then(|i| i.get("id"))
        .and_then(|v| v.as_str())
        .unwrap_or("");

    tmpl.replace("{event}", event)
        .replace("{title}", title)
        .replace("{key}", issue_key)
        .replace("{reason}", reason)
        .replace("{actor}", actor)
        .replace("{workspace}", workspace)
        .replace("{project}", project)
        .replace("{state}", state)
        .replace("{priority}", priority)
        .replace("{issue_id}", issue_id)
        .replace("{url}", &format!("issue/{}", issue_id))
}
