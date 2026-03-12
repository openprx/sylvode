use crate::config::CliAgentConfig;
use serde::Serialize;
use std::time::Duration;

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct CallbackPayload {
    pub issue_id: String,
    pub run_id: String,
    pub executor: String,
    pub status: String,
    pub summary: String,
    pub exit_code: Option<i32>,
    pub duration_ms: u128,
    pub stdout_tail: String,
    pub stderr_tail: String,
    pub state: Option<String>,
}

pub async fn send_callback(
    cfg: &CliAgentConfig,
    payload: &CallbackPayload,
    http_timeout_secs: u64,
) -> Result<(), String> {
    let callback_url = match &cfg.callback_url {
        Some(u) if !u.is_empty() => u,
        _ => return Ok(()),
    };

    let callback_mode = cfg.callback.as_deref().unwrap_or("mcp");
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(http_timeout_secs))
        .build()
        .map_err(|e| format!("build callback client failed: {e}"))?;

    let mut req = match callback_mode {
        "mcp" => client.post(callback_url).json(&serde_json::json!({
            "method": "issue.comment",
            "params": payload,
        })),
        "api" => client.post(callback_url).json(payload),
        other => return Err(format!("unsupported callback mode: {other}")),
    };

    if let Some(token) = &cfg.callback_token {
        if !token.is_empty() {
            req = req.bearer_auth(token);
        }
    }

    let resp = req
        .send()
        .await
        .map_err(|e| format!("callback request failed: {e}"))?;
    if resp.status().is_success() {
        Ok(())
    } else {
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        Err(format!("callback failed: {} {}", status, body))
    }
}

#[allow(clippy::too_many_arguments)]
pub fn build_callback_payload(
    issue_id: String,
    run_id: String,
    executor: String,
    status: String,
    summary: String,
    exit_code: Option<i32>,
    duration_ms: u128,
    stdout_tail: String,
    stderr_tail: String,
    state: Option<String>,
) -> CallbackPayload {
    CallbackPayload {
        issue_id,
        run_id,
        executor,
        status,
        summary,
        exit_code,
        duration_ms,
        stdout_tail,
        stderr_tail,
        state,
    }
}

pub fn state_for_status(cfg: &CliAgentConfig, status: &str) -> Option<String> {
    match status {
        "success" => cfg.update_state_on_success.clone(),
        "failed" | "timeout" => cfg.update_state_on_fail.clone(),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builds_callback_payload_with_issue_id() {
        let payload = build_callback_payload(
            "123".into(),
            "run-1".into(),
            "codex".into(),
            "success".into(),
            "ok".into(),
            Some(0),
            100,
            "out".into(),
            "err".into(),
            Some("done".into()),
        );

        assert_eq!(payload.issue_id, "123");
        assert_eq!(payload.run_id, "run-1");
        assert_eq!(payload.status, "success");
        assert_eq!(payload.state.as_deref(), Some("done"));
    }
}
