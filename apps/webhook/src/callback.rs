use crate::config::CliAgentConfig;
use serde::Serialize;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

/// Identifies this dispatcher in the Sylvode API's bot operation ledger.
const MCP_SURFACE_HEADER: &str = "X-Sylvode-MCP-Surface";
/// Legacy name of [`MCP_SURFACE_HEADER`]. Sent alongside it with the identical value for the
/// whole Sylvode 1.x line, so this service records the same surface against an API that only
/// knows the legacy header and against one that knows both (ADR-0020 D3).
const LEGACY_MCP_SURFACE_HEADER: &str = "X-OpenPR-MCP-Surface";
/// The surface this dispatcher reports for a plain API callback.
const CLI_SURFACE: &str = "cli";

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

    let resp = req.send().await.map_err(|e| format!("callback request failed: {e}"))?;

    let status = resp.status();
    let body = resp.text().await.unwrap_or_default();
    if !status.is_success() {
        return Err(format!("callback failed: {status} {body}"));
    }
    interpret_jsonrpc_body(&body)
}

/// Reads the JSON-RPC body a `tools/call` answered with.
///
/// An MCP server reports both protocol errors (`error`) and tool errors (`result.isError`)
/// inside a 200 response, so the transport status says nothing about whether the write landed.
fn interpret_jsonrpc_body(body: &str) -> Result<(), String> {
    let Some(value) = parse_jsonrpc_response(body) else {
        // A transport that answered 200 with something other than a JSON-RPC envelope is not
        // evidence of failure, and treating it as one would fail every non-MCP receiver.
        return Ok(());
    };
    if let Some(error) = value.get("error") {
        return Err(format!("callback rejected: {error}"));
    }
    let result = value.get("result");
    if result
        .and_then(|result| result.get("isError"))
        .and_then(serde_json::Value::as_bool)
        == Some(true)
    {
        let detail = result
            .and_then(|result| result.get("content"))
            .map_or_else(|| "no detail".to_string(), ToString::to_string);
        return Err(format!("callback tool reported an error: {detail}"));
    }
    Ok(())
}

/// Accepts both a plain JSON body and the `data:` frames of an SSE transport.
fn parse_jsonrpc_response(body: &str) -> Option<serde_json::Value> {
    if let Ok(value) = serde_json::from_str::<serde_json::Value>(body) {
        return Some(value);
    }
    body.lines()
        .filter_map(|line| line.strip_prefix("data:"))
        .filter_map(|frame| serde_json::from_str::<serde_json::Value>(frame.trim()).ok())
        .next_back()
}

/// Reads the `{code, message, data}` envelope the Sylvode REST API answers with.
///
/// A receiver that is not the Sylvode API answers something else, which is left alone.
fn interpret_api_body(body: &str) -> Result<(), String> {
    let Ok(value) = serde_json::from_str::<serde_json::Value>(body) else {
        return Ok(());
    };
    match value.get("code").and_then(serde_json::Value::as_i64) {
        Some(0) | None => Ok(()),
        Some(code) => {
            let message = value
                .get("message")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("no message");
            Err(format!("callback rejected with code {code}: {message}"))
        }
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

fn format_comment_body(payload: &CallbackPayload) -> String {
    let exit_code_str = payload.exit_code.map_or_else(|| "N/A".to_string(), |c| c.to_string());

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
    let req = api_callback_request(&client, url, payload, cfg.callback_token.as_deref());
    let resp = req.send().await.map_err(|e| format!("callback request failed: {e}"))?;

    let status = resp.status();
    let body = resp.text().await.unwrap_or_default();
    if !status.is_success() {
        return Err(format!("callback failed: {status} {body}"));
    }
    interpret_api_body(&body)
}

/// Builds the plain API callback request.
///
/// The Sylvode API's bot operation ledger groups a request by the surface header. Without it every
/// write from here is recorded as an anonymous REST call. The MCP path sets its own headers.
fn api_callback_request(
    client: &reqwest::Client,
    url: &str,
    payload: &CallbackPayload,
    token: Option<&str>,
) -> reqwest::RequestBuilder {
    let mut req = client
        .post(url)
        .header(MCP_SURFACE_HEADER, CLI_SURFACE)
        .header(LEGACY_MCP_SURFACE_HEADER, CLI_SURFACE)
        .json(payload);
    if let Some(token) = token
        && !token.is_empty()
    {
        req = req.bearer_auth(token);
    }
    req
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
        "failed" | "timeout" | "cancelled" => cfg.update_state_on_fail.clone(),
        _ => None,
    }
}

#[cfg(test)]
// `serde_json::Value` indexing returns `Null` for a missing key; it does not panic.
#[allow(clippy::indexing_slicing)]
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

    fn sample_payload() -> CallbackPayload {
        build_callback_payload(
            "issue-1".into(),
            "run-1".into(),
            "codex".into(),
            "success".into(),
            "done".into(),
            Some(0),
            1,
            String::new(),
            String::new(),
            None,
        )
    }

    #[test]
    fn api_callback_sends_canonical_and_legacy_surface_headers_with_one_value() {
        let client = build_client(5).unwrap();
        let request = api_callback_request(&client, "http://127.0.0.1:9/callback", &sample_payload(), Some("opr_x"))
            .build()
            .unwrap();
        let headers = request.headers();

        let canonical: Vec<_> = headers.get_all("x-sylvode-mcp-surface").iter().collect();
        let legacy: Vec<_> = headers.get_all("x-openpr-mcp-surface").iter().collect();
        assert_eq!(canonical, vec!["cli"], "canonical surface header");
        assert_eq!(
            legacy,
            vec!["cli"],
            "legacy surface header for an API that predates the rename"
        );
        assert_eq!(
            headers.get("authorization").and_then(|v| v.to_str().ok()),
            Some("Bearer opr_x")
        );
    }

    #[test]
    fn api_callback_omits_authorization_for_an_empty_token() {
        let client = build_client(5).unwrap();
        let request = api_callback_request(&client, "http://127.0.0.1:9/callback", &sample_payload(), Some(""))
            .build()
            .unwrap();
        assert!(request.headers().get("authorization").is_none());
        assert!(request.headers().get("x-sylvode-mcp-surface").is_some());
    }

    fn mcp_config(url: &str, token: &str) -> CliAgentConfig {
        toml::from_str(&format!(
            "executor = 'codex'\ncallback = 'mcp'\ncallback_url = '{url}'\ncallback_token = '{token}'\n"
        ))
        .expect("cli config")
    }

    fn final_payload(state: Option<&str>) -> CallbackPayload {
        build_callback_payload(
            "issue-9".into(),
            "run-9".into(),
            "codex".into(),
            "failed".into(),
            "cli execution failed".into(),
            Some(2),
            5,
            "out".into(),
            "err".into(),
            state.map(Into::into),
        )
    }

    /// An MCP callback is a JSON-RPC `tools/call` sent with the bot token as a bearer token: a
    /// comment with the report, then the state update.
    #[tokio::test]
    async fn mcp_callback_sends_the_bearer_token_with_each_tool_call() {
        let mut server = crate::test_support::Recorder::start().await;
        let cfg = mcp_config(&server.url, "bot-token");
        send_callback(&cfg, &final_payload(Some("todo")), 5)
            .await
            .expect("callback succeeds");

        let wait = std::time::Duration::from_secs(5);
        let comment = server.next(wait).await.expect("comment request");
        assert_eq!(
            comment.headers.get("authorization").and_then(|v| v.to_str().ok()),
            Some("Bearer bot-token")
        );
        assert_eq!(comment.body["method"], "tools/call");
        assert_eq!(comment.body["params"]["name"], "comments.create");
        assert_eq!(comment.body["params"]["arguments"]["work_item_id"], "issue-9");

        let update = server.next(wait).await.expect("state request");
        assert_eq!(
            update.headers.get("authorization").and_then(|v| v.to_str().ok()),
            Some("Bearer bot-token")
        );
        assert_eq!(update.body["params"]["name"], "work_items.update");
        assert_eq!(update.body["params"]["arguments"]["state"], "todo");
        assert!(server.next(std::time::Duration::from_millis(200)).await.is_none());
    }

    #[test]
    fn jsonrpc_tool_error_inside_a_200_is_a_failure() {
        let body = r#"{"jsonrpc":"2.0","id":1,"result":{"isError":true,"content":[{"type":"text","text":"work item not found"}]}}"#;

        let error = interpret_jsonrpc_body(body).expect_err("a tool error must not read as success");
        assert!(error.contains("work item not found"), "{error}");
    }

    #[test]
    fn jsonrpc_protocol_error_inside_a_200_is_a_failure() {
        let body = r#"{"jsonrpc":"2.0","id":1,"error":{"code":-32602,"message":"unknown tool"}}"#;

        let error = interpret_jsonrpc_body(body).expect_err("a protocol error must not read as success");
        assert!(error.contains("unknown tool"), "{error}");
    }

    #[test]
    fn jsonrpc_success_and_non_envelope_bodies_pass() {
        assert!(interpret_jsonrpc_body(r#"{"jsonrpc":"2.0","id":1,"result":{"content":[]}}"#).is_ok());
        assert!(interpret_jsonrpc_body("OK").is_ok());
    }

    #[test]
    fn jsonrpc_result_is_read_from_an_sse_frame() {
        let body = "event: message\ndata: {\"jsonrpc\":\"2.0\",\"id\":1,\"result\":{\"isError\":true,\"content\":\"denied\"}}\n\n";

        let error = interpret_jsonrpc_body(body).expect_err("an SSE-framed tool error must be read");
        assert!(error.contains("denied"), "{error}");
    }

    #[test]
    fn rest_business_code_inside_a_200_is_a_failure() {
        let error = interpret_api_body(r#"{"code":403,"message":"forbidden"}"#)
            .expect_err("a non-zero business code must not read as success");
        assert!(error.contains("forbidden"), "{error}");

        assert!(interpret_api_body(r#"{"code":0,"message":"success"}"#).is_ok());
        assert!(interpret_api_body("not json").is_ok());
    }
}
