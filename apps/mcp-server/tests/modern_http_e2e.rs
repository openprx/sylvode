#![allow(clippy::indexing_slicing)]
//! The modern era over HTTP on the shipped binary: header validation, statuses, `Origin`, the
//! Server-Sent Events response of `subscriptions/listen`, the stream caps and the graceful end.

mod support;

use axum::{Json, Router, routing::get};
use reqwest::StatusCode;
use serde_json::{Value, json};
use std::error::Error;
use std::process::Stdio;
use std::time::Duration;
use support::{McpSettings, write_config_with_extra_mcp_keys};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::process::{Child, Command};

type TestResult = Result<(), Box<dyn Error>>;

const TOKEN: &str = "opr_modern_http_bot_token";
const OTHER_TOKEN: &str = "opr_modern_http_other_token";
const META: &str = "io.modelcontextprotocol/protocolVersion";

async fn spawn_api() -> Result<String, Box<dyn Error>> {
    let router = Router::new().fallback(get(|| async {
        Json(json!({"code": 0, "message": "ok", "data": {"items": [], "total": 0}}))
    }));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let addr = listener.local_addr()?;
    tokio::spawn(async move {
        let _ = axum::serve(listener, router).await;
    });
    Ok(format!("http://{addr}"))
}

struct Server {
    child: Child,
    base: String,
    http: reqwest::Client,
    _config: support::ConfigFile,
}

impl Server {
    async fn start(extra_keys: &str) -> Result<Self, Box<dyn Error>> {
        Self::start_with(extra_keys, false).await
    }

    /// As [`Self::start`]; `hold` keeps listen streams open (the test-only hold, which the test
    /// build of the binary contains and a release build does not).
    async fn start_with(extra_keys: &str, hold: bool) -> Result<Self, Box<dyn Error>> {
        let api_url = spawn_api().await?;
        let bind = {
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
            listener.local_addr()?.to_string()
        };
        let config = write_config_with_extra_mcp_keys(
            &McpSettings {
                api_url: &api_url,
                bot_token: None,
                workspace_id: None,
                transport: Some("http"),
                bind_addr: Some(&bind),
            },
            &format!("{}{extra_keys}", support::FULL_TOOLS),
        )?;
        let child = Command::new(env!("CARGO_BIN_EXE_mcp-server"))
            .args(["serve", "--config"])
            .arg(config.path())
            .env("SYLVODE_MCP_TEST_HOLD_LISTEN", if hold { "1" } else { "0" })
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .kill_on_drop(true)
            .spawn()?;
        let server = Self {
            child,
            base: format!("http://{bind}"),
            http: reqwest::Client::new(),
            _config: config,
        };
        for _ in 0..200 {
            if server
                .http
                .get(format!("{}/health", server.base))
                .send()
                .await
                .is_ok_and(|r| r.status().is_success())
            {
                return Ok(server);
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        Err("server never became ready".into())
    }

    fn url(&self) -> String {
        format!("{}/mcp/rpc", self.base)
    }

    /// Posts `body` with the `Mcp-*` headers of a well-formed modern request, plus `extra`.
    async fn modern(
        &self,
        token: &str,
        body: &Value,
        extra: &[(&str, &str)],
    ) -> Result<reqwest::Response, Box<dyn Error>> {
        let method = body["method"].as_str().unwrap_or_default();
        let mut request = self
            .http
            .post(self.url())
            .bearer_auth(token)
            .header("MCP-Protocol-Version", "2026-07-28")
            .header("Mcp-Method", method)
            .json(body);
        for (name, value) in extra {
            request = request.header(*name, *value);
        }
        Ok(request.send().await?)
    }

    fn pid(&self) -> Result<u32, Box<dyn Error>> {
        self.child.id().ok_or_else(|| "server already exited".into())
    }
}

fn modern_body(id: i64, method: &str, params: Value) -> Value {
    let mut params = params;
    if let Some(fields) = params.as_object_mut() {
        fields.insert("_meta".to_string(), json!({ META: "2026-07-28" }));
    }
    json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params})
}

/// Reads one SSE event (`data:` payload) from a streaming response, skipping comments.
async fn next_event(response: &mut reqwest::Response, buffer: &mut String) -> Result<Option<Value>, Box<dyn Error>> {
    loop {
        if let Some(end) = buffer.find("\n\n") {
            let block: String = buffer.drain(..end + 2).collect();
            let data: Vec<&str> = block.lines().filter_map(|line| line.strip_prefix("data:")).collect();
            if data.is_empty() {
                continue;
            }
            return Ok(Some(serde_json::from_str(data.join("\n").trim())?));
        }
        match tokio::time::timeout(Duration::from_secs(20), response.chunk()).await?? {
            Some(chunk) => buffer.push_str(&String::from_utf8_lossy(&chunk)),
            None => return Ok(None),
        }
    }
}

#[tokio::test]
async fn a_name_header_that_differs_from_the_body_is_refused_with_400() -> TestResult {
    let server = Server::start("").await?;
    let body = modern_body(1, "tools/call", json!({"name": "projects.list", "arguments": {}}));
    let response = server.modern(TOKEN, &body, &[("Mcp-Name", "work_items.list")]).await?;
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    let answer: Value = response.json().await?;
    assert_eq!(answer["error"]["code"], -32020, "{answer}");
    assert_eq!(answer["error"]["data"]["header"], "Mcp-Name");
    assert_eq!(answer["id"], 1);

    // The same call with the right header is served.
    let ok = server.modern(TOKEN, &body, &[("Mcp-Name", "projects.list")]).await?;
    assert_eq!(ok.status(), StatusCode::OK);

    // Missing protocol-version and method headers are refused too.
    let bare = server
        .http
        .post(server.url())
        .bearer_auth(TOKEN)
        .json(&modern_body(2, "tools/list", json!({})))
        .send()
        .await?;
    assert_eq!(bare.status(), StatusCode::BAD_REQUEST);
    let answer: Value = bare.json().await?;
    assert_eq!(answer["error"]["data"]["header"], "MCP-Protocol-Version");
    Ok(())
}

#[tokio::test]
async fn header_validation_runs_after_authentication() -> TestResult {
    let server = Server::start("").await?;
    let response = server
        .http
        .post(server.url())
        .json(&modern_body(1, "tools/list", json!({})))
        .send()
        .await?;
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    Ok(())
}

#[tokio::test]
async fn an_unknown_modern_method_is_404_and_an_unknown_version_is_400() -> TestResult {
    let server = Server::start("").await?;
    let response = server
        .modern(TOKEN, &modern_body(1, "tools/nope", json!({})), &[])
        .await?;
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
    assert_eq!(response.json::<Value>().await?["error"]["code"], -32601);

    let body = json!({"jsonrpc": "2.0", "id": 2, "method": "tools/list", "params": {"_meta": {META: "1900-01-01"}}});
    let response = server
        .http
        .post(server.url())
        .bearer_auth(TOKEN)
        .header("MCP-Protocol-Version", "1900-01-01")
        .header("Mcp-Method", "tools/list")
        .json(&body)
        .send()
        .await?;
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    let answer: Value = response.json().await?;
    assert_eq!(answer["error"]["code"], -32022);
    assert_eq!(
        answer["error"]["data"]["supported"],
        json!(["2026-07-28", "2025-11-25"])
    );
    assert_eq!(answer["error"]["data"]["requested"], "1900-01-01");
    Ok(())
}

#[tokio::test]
async fn legacy_requests_need_no_mcp_headers_and_the_version_header_alone_stays_legacy() -> TestResult {
    let server = Server::start("").await?;
    let init = json!({"jsonrpc": "2.0", "id": 1, "method": "initialize",
                      "params": {"protocolVersion": "2025-11-25", "capabilities": {}, "clientInfo": {"name": "c", "version": "1"}}});
    let response = server
        .http
        .post(server.url())
        .bearer_auth(TOKEN)
        .json(&init)
        .send()
        .await?;
    assert_eq!(response.status(), StatusCode::OK);
    assert!(response.headers().get("mcp-session-id").is_none());
    assert_eq!(
        response.json::<Value>().await?["result"]["protocolVersion"],
        "2025-11-25"
    );

    let list = json!({"jsonrpc": "2.0", "id": 2, "method": "tools/list"});
    let response = server
        .http
        .post(server.url())
        .bearer_auth(TOKEN)
        .header("MCP-Protocol-Version", "2025-06-18")
        .json(&list)
        .send()
        .await?;
    assert_eq!(response.status(), StatusCode::OK);
    assert!(response.headers().get("mcp-session-id").is_none());
    let answer: Value = response.json().await?;
    assert!(answer["result"]["tools"].is_array());
    assert!(answer["result"].get("resultType").is_none(), "{answer}");

    // An unknown legacy method keeps its 200 answer.
    let unknown = json!({"jsonrpc": "2.0", "id": 3, "method": "nope"});
    let response = server
        .http
        .post(server.url())
        .bearer_auth(TOKEN)
        .json(&unknown)
        .send()
        .await?;
    assert_eq!(response.status(), StatusCode::OK);
    Ok(())
}

#[tokio::test]
async fn a_modern_response_carries_no_session_id_and_get_is_405() -> TestResult {
    let server = Server::start("").await?;
    let response = server
        .modern(TOKEN, &modern_body(1, "tools/list", json!({})), &[])
        .await?;
    assert_eq!(response.status(), StatusCode::OK);
    assert!(response.headers().get("mcp-session-id").is_none());
    let get = server.http.get(server.url()).bearer_auth(TOKEN).send().await?;
    assert_eq!(get.status(), StatusCode::METHOD_NOT_ALLOWED);
    Ok(())
}

#[tokio::test]
async fn a_modern_notification_is_accepted_with_202() -> TestResult {
    let server = Server::start("").await?;
    let body = json!({"jsonrpc": "2.0", "method": "notifications/cancelled",
                      "params": {"_meta": {META: "2026-07-28"}, "requestId": 1}});
    let response = server.modern(TOKEN, &body, &[]).await?;
    assert_eq!(response.status(), StatusCode::ACCEPTED);
    Ok(())
}

#[tokio::test]
async fn origin_absent_is_served_present_must_be_configured() -> TestResult {
    let server = Server::start("allowed_origins = [\"https://ok.example\"]\n").await?;
    let body = modern_body(1, "tools/list", json!({}));
    let none = server.modern(TOKEN, &body, &[]).await?;
    assert_eq!(none.status(), StatusCode::OK);
    let allowed = server.modern(TOKEN, &body, &[("Origin", "https://ok.example")]).await?;
    assert_eq!(allowed.status(), StatusCode::OK);
    let refused = server
        .modern(TOKEN, &body, &[("Origin", "https://evil.example")])
        .await?;
    assert_eq!(refused.status(), StatusCode::FORBIDDEN);
    assert_eq!(refused.json::<Value>().await?, json!({"error": "origin not allowed"}));

    // The check comes before authentication and covers the legacy routes too.
    let anonymous = server
        .http
        .post(server.url())
        .header("Origin", "https://evil.example")
        .json(&body)
        .send()
        .await?;
    assert_eq!(anonymous.status(), StatusCode::FORBIDDEN);
    let sse = server
        .http
        .get(format!("{}/sse", server.base))
        .bearer_auth(TOKEN)
        .header("Origin", "https://evil.example")
        .send()
        .await?;
    assert_eq!(sse.status(), StatusCode::FORBIDDEN);
    let health = server
        .http
        .get(format!("{}/health", server.base))
        .header("Origin", "https://evil.example")
        .send()
        .await?;
    assert_eq!(health.status(), StatusCode::OK);
    Ok(())
}

fn listen_body(id: i64) -> Value {
    modern_body(
        id,
        "subscriptions/listen",
        json!({"notifications": {"toolsListChanged": true, "resourceSubscriptions": ["sylvode://tasks/0F"]}}),
    )
}

#[tokio::test]
async fn listen_answers_an_event_stream_with_the_acknowledgment_first_then_keepalives() -> TestResult {
    let server = Server::start_with("", true).await?;
    let mut response = server.modern(TOKEN, &listen_body(7), &[]).await?;
    assert_eq!(response.status(), StatusCode::OK);
    let content_type = response.headers().get("content-type").and_then(|v| v.to_str().ok());
    assert_eq!(content_type, Some("text/event-stream"));
    assert_eq!(
        response
            .headers()
            .get("x-accel-buffering")
            .and_then(|v| v.to_str().ok()),
        Some("no")
    );
    assert!(response.headers().get("mcp-session-id").is_none());
    let mut buffer = String::new();
    let ack = next_event(&mut response, &mut buffer).await?.ok_or("stream ended")?;
    assert_eq!(ack["method"], "notifications/subscriptions/acknowledged", "{ack}");
    assert_eq!(ack["params"]["_meta"]["io.modelcontextprotocol/subscriptionId"], 7);
    assert_eq!(ack["params"]["notifications"], json!({"resourceSubscriptions": []}));

    // A keep-alive comment arrives within 16 s while nothing else happens.
    let started = std::time::Instant::now();
    let mut seen = buffer.contains(':');
    while !seen && started.elapsed() < Duration::from_secs(16) {
        let chunk = tokio::time::timeout(Duration::from_secs(16), response.chunk()).await??;
        let text = chunk
            .map(|bytes| String::from_utf8_lossy(&bytes).to_string())
            .unwrap_or_default();
        seen = text.lines().any(|line| line.starts_with(':'));
    }
    assert!(seen, "no keep-alive comment within 16 s");
    Ok(())
}

#[tokio::test]
async fn listen_with_the_switch_off_is_method_not_found() -> TestResult {
    let server = Server::start("listen = false\n").await?;
    let response = server.modern(TOKEN, &listen_body(1), &[]).await?;
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
    assert_eq!(response.json::<Value>().await?["error"]["code"], -32601);
    Ok(())
}

#[tokio::test]
async fn a_legacy_listen_request_is_method_not_found() -> TestResult {
    let server = Server::start("").await?;
    let body = json!({"jsonrpc": "2.0", "id": 1, "method": "subscriptions/listen", "params": {}});
    let response = server
        .http
        .post(server.url())
        .bearer_auth(TOKEN)
        .json(&body)
        .send()
        .await?;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response.json::<Value>().await?["error"]["code"], -32601);
    Ok(())
}

#[tokio::test]
async fn the_stream_cap_refuses_with_429_and_retry_after_and_frees_on_close() -> TestResult {
    let server = Server::start_with("max_listen_streams = 2\n", true).await?;
    let mut first = server.modern(TOKEN, &listen_body(1), &[]).await?;
    let mut second = server.modern(OTHER_TOKEN, &listen_body(1), &[]).await?;
    let (mut b1, mut b2) = (String::new(), String::new());
    assert!(next_event(&mut first, &mut b1).await?.is_some());
    assert!(next_event(&mut second, &mut b2).await?.is_some());

    let refused = server.modern(TOKEN, &listen_body(2), &[]).await?;
    assert_eq!(refused.status(), StatusCode::TOO_MANY_REQUESTS);
    assert_eq!(
        refused.headers().get("retry-after").and_then(|v| v.to_str().ok()),
        Some("30")
    );
    let answer: Value = refused.json().await?;
    assert_eq!(answer["error"]["code"], -32000);
    assert_eq!(answer["error"]["data"]["limit"], 2);

    // Closing one stream frees its place.
    drop(first);
    let mut reopened = None;
    for _ in 0..50 {
        let attempt = server.modern(TOKEN, &listen_body(3), &[]).await?;
        if attempt.status() == StatusCode::OK {
            reopened = Some(attempt);
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert!(reopened.is_some(), "the closed stream's place was never freed");
    Ok(())
}

#[tokio::test]
async fn sigterm_ends_an_open_stream_with_the_final_result_and_exits_zero() -> TestResult {
    let mut server = Server::start_with("", true).await?;
    let mut stream = server.modern(TOKEN, &listen_body(9), &[]).await?;
    let mut buffer = String::new();
    assert!(next_event(&mut stream, &mut buffer).await?.is_some());

    let status = Command::new("kill")
        .arg("-TERM")
        .arg(server.pid()?.to_string())
        .status()
        .await?;
    assert!(status.success());

    let result = next_event(&mut stream, &mut buffer).await?.ok_or("no final result")?;
    assert_eq!(result["id"], 9, "{result}");
    assert_eq!(result["result"]["resultType"], "complete");
    assert_eq!(result["result"]["_meta"]["io.modelcontextprotocol/subscriptionId"], 9);
    assert!(
        next_event(&mut stream, &mut buffer).await?.is_none(),
        "the stream must end"
    );

    let exit = tokio::time::timeout(Duration::from_secs(10), server.child.wait()).await??;
    assert_eq!(exit.code(), Some(0));
    Ok(())
}

/// Without the hold a listen request is the empty acknowledgment followed by the final result, and
/// the connection ends: a caller whose token nobody checked holds no stream and no cap slot.
#[tokio::test]
async fn a_made_up_bearer_gets_the_acknowledgment_and_the_final_result_and_the_connection_ends() -> TestResult {
    let server = Server::start("").await?;
    let mut response = server.modern("totally-not-a-token", &listen_body(7), &[]).await?;
    assert_eq!(response.status(), StatusCode::OK);
    let mut buffer = String::new();
    let ack = next_event(&mut response, &mut buffer).await?.ok_or("stream ended")?;
    assert_eq!(ack["method"], "notifications/subscriptions/acknowledged", "{ack}");
    assert_eq!(ack["params"]["notifications"], json!({"resourceSubscriptions": []}));
    let end = next_event(&mut response, &mut buffer).await?.ok_or("no final result")?;
    assert_eq!(end["id"], 7, "{end}");
    assert_eq!(end["result"]["resultType"], "complete");
    assert!(
        next_event(&mut response, &mut buffer).await?.is_none(),
        "the stream must end"
    );
    Ok(())
}

#[tokio::test]
async fn made_up_tokens_never_fill_the_stream_caps() -> TestResult {
    let server = Server::start("max_listen_streams = 2\n").await?;
    for index in 0..25 {
        let token = format!("made-up-token-{index}");
        let mut response = server.modern(&token, &listen_body(index), &[]).await?;
        assert_eq!(response.status(), StatusCode::OK, "token {index}");
        let mut buffer = String::new();
        assert!(next_event(&mut response, &mut buffer).await?.is_some());
        assert!(next_event(&mut response, &mut buffer).await?.is_some());
        assert!(next_event(&mut response, &mut buffer).await?.is_none());
    }
    Ok(())
}

#[tokio::test]
async fn a_listen_request_with_a_bad_envelope_is_refused_with_400() -> TestResult {
    let server = Server::start("").await?;
    for body in [
        json!({"jsonrpc": "1.0", "id": 1, "method": "subscriptions/listen",
               "params": {"_meta": {META: "2026-07-28"}}}),
        json!({"jsonrpc": "2.0", "id": {"a": 1}, "method": "subscriptions/listen",
               "params": {"_meta": {META: "2026-07-28"}}}),
    ] {
        let response = server.modern(TOKEN, &body, &[]).await?;
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        assert_eq!(response.json::<Value>().await?["error"]["code"], -32600);
    }
    Ok(())
}

#[tokio::test]
async fn two_origin_headers_are_refused_and_a_silent_connection_is_closed() -> TestResult {
    let server = Server::start("allowed_origins = [\"https://ok.example\"]\n").await?;
    let body = modern_body(1, "tools/list", json!({}));
    let two = server
        .modern(
            TOKEN,
            &body,
            &[("Origin", "https://ok.example"), ("Origin", "https://evil.example")],
        )
        .await?;
    assert_eq!(two.status(), StatusCode::FORBIDDEN);

    // A connection that sends part of a request head and then nothing is closed after the
    // header-read timeout (10 s).
    let address = server.base.trim_start_matches("http://").to_string();
    let mut socket = tokio::net::TcpStream::connect(&address).await?;
    socket.write_all(b"POST /mcp/rpc HTTP/1.1\r\nHost: x\r\n").await?;
    let mut rest = Vec::new();
    let closed = tokio::time::timeout(Duration::from_secs(20), socket.read_to_end(&mut rest)).await;
    assert!(closed.is_ok(), "the silent connection stayed open");
    Ok(())
}

/// A backend failure names only what was being read: no internal address, no identifier.
#[tokio::test]
async fn a_backend_failure_does_not_leak_the_api_address_or_the_identifier() -> TestResult {
    // The server's API address is a closed port, so every read fails in the transport.
    let api = "http://127.0.0.1:1";
    let bind = {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
        listener.local_addr()?.to_string()
    };
    let config = write_config_with_extra_mcp_keys(
        &McpSettings {
            api_url: api,
            bot_token: None,
            workspace_id: None,
            transport: Some("http"),
            bind_addr: Some(&bind),
        },
        "",
    )?;
    let mut child = Command::new(env!("CARGO_BIN_EXE_mcp-server"))
        .args(["serve", "--config"])
        .arg(config.path())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .spawn()?;
    let http = reqwest::Client::new();
    let mut ready = false;
    for _ in 0..200 {
        if http
            .get(format!("http://{bind}/health"))
            .send()
            .await
            .is_ok_and(|r| r.status().is_success())
        {
            ready = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert!(ready);
    let body = modern_body(1, "resources/read", json!({"uri": "sylvode://issues/PRX-4242"}));
    let response = http
        .post(format!("http://{bind}/mcp/rpc"))
        .bearer_auth(TOKEN)
        .header("MCP-Protocol-Version", "2026-07-28")
        .header("Mcp-Method", "resources/read")
        .header("Mcp-Name", "sylvode://issues/PRX-4242")
        .json(&body)
        .send()
        .await?;
    let text = response.text().await?;
    let answer: Value = serde_json::from_str(&text)?;
    assert_eq!(answer["error"]["code"], -32603, "{text}");
    assert!(!text.contains("127.0.0.1"), "{text}");
    assert!(!text.contains("PRX-4242"), "{text}");
    child.start_kill()?;
    tokio::time::timeout(Duration::from_secs(10), child.wait()).await??;
    Ok(())
}
