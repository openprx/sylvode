use crate::config::CliAgentConfig;
use serde::Serialize;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

/// Monotonically increasing JSON-RPC request ID counter.
static JSONRPC_ID: AtomicU64 = AtomicU64::new(1);

fn next_jsonrpc_id() -> u64 {
    JSONRPC_ID.fetch_add(1, Ordering::Relaxed)
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
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
    match callback_mode {
        "mcp" => send_mcp_callback(cfg, callback_url, payload, http_timeout_secs).await,
        "api" => send_api_callback(cfg, callback_url, payload, http_timeout_secs).await,
        other => Err(format!("unsupported callback mode: {other}")),
    }
}

/// Build an HTTP client with the configured timeout.
fn build_client(http_timeout_secs: u64) -> Result<reqwest::Client, String> {
    reqwest::Client::builder()
        .timeout(Duration::from_secs(http_timeout_secs))
        .build()
        .map_err(|e| format!("build callback client failed: {e}"))
}

/// Send a single JSON-RPC 2.0 request and check the response status.
async fn send_jsonrpc_request(
    client: &reqwest::Client,
    url: &str,
    token: Option<&str>,
    body: &serde_json::Value,
) -> Result<(), String> {
    let mut req = client.post(url).json(body);
    if let Some(t) = token
        && !t.is_empty()
    {
        req = req.bearer_auth(t);
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
        Err(format!("callback failed: {status} {body}"))
    }
}

/// Build a JSON-RPC 2.0 `tools/call` envelope.
fn jsonrpc_tools_call(name: &str, arguments: &serde_json::Value) -> serde_json::Value {
    serde_json::json!({
        "jsonrpc": "2.0",
        "id": next_jsonrpc_id(),
        "method": "tools/call",
        "params": {
            "name": name,
            "arguments": arguments,
        }
    })
}

/// Format the agent execution report comment body.
fn format_comment_body(payload: &CallbackPayload) -> String {
    let exit_code_str = payload
        .exit_code
        .map_or_else(|| "N/A".to_string(), |c| c.to_string());

    let mut body = format!(
        "**Agent Execution Report**\n\
         - Executor: {executor}\n\
         - Status: {status}\n\
         - Duration: {duration}ms\n\
         - Exit code: {exit_code}\n\
         \n\
         **Output:**\n\
         {stdout}",
        executor = payload.executor,
        status = payload.status,
        duration = payload.duration_ms,
        exit_code = exit_code_str,
        stdout = payload.stdout_tail,
    );

    if !payload.stderr_tail.is_empty() {
        body.push_str("\n\n**Stderr:**\n");
        body.push_str(&payload.stderr_tail);
    }

    body
}

/// Send callback in MCP mode using JSON-RPC 2.0 `tools/call` format.
///
/// - For "started" status: sends `work_items.update` to update issue state.
/// - For final status: sends `comments.create` with execution report, then
///   optionally `work_items.update` if a state transition is specified.
async fn send_mcp_callback(
    cfg: &CliAgentConfig,
    url: &str,
    payload: &CallbackPayload,
    http_timeout_secs: u64,
) -> Result<(), String> {
    let client = build_client(http_timeout_secs)?;
    let token = cfg.callback_token.as_deref();

    if payload.status == "started" {
        // Start callback: update issue state only (if state is provided)
        if let Some(state) = &payload.state {
            let args = serde_json::json!({
                    "work_item_id": payload.issue_id,
                    "state": state,
                });
            let body = jsonrpc_tools_call("work_items.update", &args);
            send_jsonrpc_request(&client, url, token, &body).await?;
        }
        return Ok(());
    }

    // Final callback (success / failed / timeout):
    // 1. Post execution report as a comment
    let comment_body = format_comment_body(payload);
    let comment_args = serde_json::json!({
        "work_item_id": payload.issue_id,
        "content": comment_body,
    });
    let comment_rpc = jsonrpc_tools_call("comments.create", &comment_args);
    send_jsonrpc_request(&client, url, token, &comment_rpc).await?;

    // 2. Update issue state if provided
    if let Some(state) = &payload.state {
        let state_args = serde_json::json!({
            "work_item_id": payload.issue_id,
            "state": state,
        });
        let state_rpc = jsonrpc_tools_call("work_items.update", &state_args);
        send_jsonrpc_request(&client, url, token, &state_rpc).await?;
    }

    Ok(())
}

/// Send callback in plain API mode (original format).
async fn send_api_callback(
    cfg: &CliAgentConfig,
    url: &str,
    payload: &CallbackPayload,
    http_timeout_secs: u64,
) -> Result<(), String> {
    let client = build_client(http_timeout_secs)?;
    let mut req = client.post(url).json(payload);

    if let Some(token) = &cfg.callback_token
        && !token.is_empty()
    {
        req = req.bearer_auth(token);
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
        Err(format!("callback failed: {status} {body}"))
    }
}

#[allow(clippy::too_many_arguments)]
pub const fn build_callback_payload(
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
