#![allow(clippy::indexing_slicing)]
//! Both protocol eras on the shipped binary: `server/discover` and modern requests next to the
//! legacy `initialize`, over stdio and over HTTP, against a stand-in API.

mod support;

use axum::{Json, Router, routing::get};
use serde_json::{Value, json};
use std::error::Error;
use std::process::Stdio;
use std::time::Duration;
use support::{McpSettings, write_config_with_extra_mcp_keys};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, Command};

type TestResult = Result<(), Box<dyn Error>>;

const TOKEN: &str = "opr_lifecycle_bot_token";
const META: &str = "io.modelcontextprotocol/protocolVersion";

fn modern(id: i64, method: &str, params: Value) -> Value {
    let mut params = params;
    if let Some(fields) = params.as_object_mut() {
        fields.insert(
            "_meta".to_string(),
            json!({ META: "2026-07-28", "io.modelcontextprotocol/clientInfo": {"name": "e2e", "version": "1"},
                    "io.modelcontextprotocol/clientCapabilities": {} }),
        );
    }
    json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params})
}

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

fn spawn_server(api_url: &str, http_bind: Option<&str>) -> Result<(Child, support::ConfigFile), Box<dyn Error>> {
    let config = write_config_with_extra_mcp_keys(
        &McpSettings {
            api_url,
            bot_token: if http_bind.is_some() { None } else { Some(TOKEN) },
            workspace_id: if http_bind.is_some() {
                None
            } else {
                Some("11111111-1111-4111-8111-111111111111")
            },
            transport: Some(if http_bind.is_some() { "http" } else { "stdio" }),
            bind_addr: http_bind,
        },
        support::FULL_TOOLS,
    )?;
    let child = Command::new(env!("CARGO_BIN_EXE_mcp-server"))
        .args(["serve", "--config"])
        .arg(config.path())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .kill_on_drop(true)
        .spawn()?;
    Ok((child, config))
}

async fn stop(mut child: Child) -> TestResult {
    child.start_kill()?;
    tokio::time::timeout(Duration::from_secs(10), child.wait()).await??;
    Ok(())
}

/// Checks the assertions every era-aware server must meet; `ask` sends one request and returns
/// the answer.
async fn exercise<F, Fut>(expect_structured: bool, mut ask: F) -> TestResult
where
    F: FnMut(Value) -> Fut,
    Fut: std::future::Future<Output = Result<Value, Box<dyn Error>>>,
{
    // Discover: no `_meta` needed, the probe of the modern era.
    let discover = ask(json!({"jsonrpc": "2.0", "id": 1, "method": "server/discover"})).await?;
    let result = &discover["result"];
    assert_eq!(result["resultType"], "complete", "{discover}");
    assert_eq!(result["supportedVersions"], json!(["2026-07-28"]));
    assert!(result["ttlMs"].is_u64() && result["cacheScope"].is_string() && result["capabilities"].is_object());
    assert!(result["capabilities"]["extensions"]["io.sylvode/task-protocol"].is_object());

    // Modern tools/list.
    let list = ask(modern(2, "tools/list", json!({}))).await?;
    assert_eq!(list["result"]["resultType"], "complete", "{list}");
    assert!(
        list["result"]["tools"]
            .as_array()
            .is_some_and(|tools| !tools.is_empty())
    );

    // Modern tools/call with structuredContent.
    let call = ask(modern(
        3,
        "tools/call",
        json!({"name": "projects.list", "arguments": {}}),
    ))
    .await?;
    let text = call["result"]["content"][0]["text"].as_str().ok_or("no text")?;
    if expect_structured {
        assert_eq!(
            call["result"]["structuredContent"],
            serde_json::from_str::<Value>(text)?,
            "{call}"
        );
    } else {
        // The HTTP stand-in has no workspace bot, so the tool answers a plain-text error.
        assert!(call["result"].get("structuredContent").is_none(), "{call}");
        assert_eq!(call["result"]["resultType"], "complete");
    }

    // Modern ping and initialize do not exist.
    assert_eq!(ask(modern(4, "ping", json!({}))).await?["error"]["code"], -32601);

    // Unsupported modern version.
    let bad = ask(json!({"jsonrpc": "2.0", "id": 5, "method": "tools/list",
                         "params": {"_meta": {META: "1900-01-01"}}}))
    .await?;
    assert_eq!(bad["error"]["code"], -32022, "{bad}");
    assert_eq!(bad["error"]["data"]["requested"], "1900-01-01");

    // The legacy era on the same process.
    let init = ask(json!({"jsonrpc": "2.0", "id": 6, "method": "initialize",
                          "params": {"protocolVersion": "2025-06-18", "capabilities": {},
                                     "clientInfo": {"name": "legacy", "version": "1"}}}))
    .await?;
    assert_eq!(init["result"]["protocolVersion"], "2025-06-18", "{init}");
    assert!(init["result"]["capabilities"].get("resourceTemplates").is_none());
    let legacy_list = ask(json!({"jsonrpc": "2.0", "id": 7, "method": "tools/list"})).await?;
    assert!(legacy_list["result"]["tools"].is_array());
    assert!(legacy_list["result"].get("resultType").is_none(), "{legacy_list}");
    Ok(())
}

#[tokio::test]
async fn stdio_serves_both_eras_on_one_process() -> TestResult {
    let api_url = spawn_api().await?;
    let (mut child, _config) = spawn_server(&api_url, None)?;
    let stdin = child.stdin.take().ok_or("no stdin")?;
    let lines = BufReader::new(child.stdout.take().ok_or("no stdout")?).lines();
    let pipe = std::sync::Arc::new(tokio::sync::Mutex::new((stdin, lines)));
    exercise(true, |request| {
        let line = format!("{request}\n");
        let pipe = std::sync::Arc::clone(&pipe);
        async move {
            let mut guard = pipe.lock().await;
            let (stdin, lines) = &mut *guard;
            stdin.write_all(line.as_bytes()).await?;
            stdin.flush().await?;
            let answer = tokio::time::timeout(Duration::from_secs(20), lines.next_line())
                .await??
                .ok_or("stdout closed")?;
            drop(guard);
            Ok(serde_json::from_str(&answer)?)
        }
    })
    .await?;
    drop(pipe);
    stop(child).await
}

#[tokio::test]
async fn http_serves_both_eras_on_one_endpoint_without_a_session_id() -> TestResult {
    let api_url = spawn_api().await?;
    let bind = {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
        listener.local_addr()?.to_string()
    };
    let (child, _config) = spawn_server(&api_url, Some(&bind))?;
    let http = reqwest::Client::new();
    let base = format!("http://{bind}");
    let mut ready = false;
    for _ in 0..100 {
        if http
            .get(format!("{base}/health"))
            .send()
            .await
            .is_ok_and(|r| r.status().is_success())
        {
            ready = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert!(ready, "server never became ready");
    exercise(false, |request| {
        let http = http.clone();
        let url = format!("{base}/mcp/rpc");
        async move {
            let mut post = http.post(url).bearer_auth(TOKEN).json(&request);
            // A modern request carries the `Mcp-*` headers that repeat its body.
            if request["params"]["_meta"][META] == "2026-07-28" {
                post = post
                    .header("MCP-Protocol-Version", "2026-07-28")
                    .header("Mcp-Method", request["method"].as_str().unwrap_or_default());
                if request["method"] == "tools/call" {
                    post = post.header("Mcp-Name", request["params"]["name"].as_str().unwrap_or_default());
                }
            }
            let response = post.send().await?;
            assert!(response.headers().get("mcp-session-id").is_none());
            let status = response.status();
            let body = response.text().await?;
            serde_json::from_str(&body).map_err(|error| format!("{status} {body}: {error}").into())
        }
    })
    .await?;
    stop(child).await
}
