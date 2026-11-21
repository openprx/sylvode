//! The shipped `mcp-server` binary configured for delegated (OAuth) access, driven over real TCP
//! against a stand-in API that also plays the token-context endpoint.
//!
//! `oauth_delegated_http.rs` proves the behaviour with the library pieces; this file proves that
//! `main.rs` wires them: the configuration keys reach the server, the metadata is routed outside
//! the token layer, and the transports that do not serve delegated access refuse it.

// Shared by every end-to-end file; this one uses only part of it.
#[allow(dead_code)]
mod support;

use axum::{Json, Router, extract::Request, http::StatusCode, response::IntoResponse};
use serde_json::{Value, json};
use std::error::Error;
use std::process::Stdio;
use std::sync::Arc;
use std::time::Duration;
use support::{ConfigFile, McpSettings, write_config_with_extra_mcp_keys};
use tokio::process::{Child, Command};
use tokio::sync::Mutex;

type TestResult = Result<(), Box<dyn Error>>;

const WORKSPACE: &str = "11111111-1111-4111-8111-111111111111";
const GRANT_WORKSPACE: &str = "77777777-7777-4777-8777-777777777777";
const FORWARD_SECRET: &str = "forward-secret-for-tests-0123456789abcdef";
const BOT_TOKEN: &str = "opr_caller_binary_e2e_token";

fn token(digit: char) -> String {
    format!("sva_{}", digit.to_string().repeat(64))
}

type Seen = Arc<Mutex<Vec<(String, Option<String>, Option<String>)>>>;

/// What the stand-in API needs from a request, taken synchronously so that the request itself is
/// not held across an await.
fn snapshot(request: &Request) -> (String, Option<String>, Option<String>) {
    let header = |name: &str| {
        request
            .headers()
            .get(name)
            .and_then(|value| value.to_str().ok())
            .map(str::to_string)
    };
    (
        request.uri().path().to_string(),
        header("authorization"),
        header("x-sylvode-forward-auth"),
    )
}

/// Answers the token-context endpoint by token (`a`: projects:read, `b`: revoked, `c`: API
/// failure) and every other path with an empty success envelope, recording what it saw.
async fn spawn_api() -> Result<(String, Seen), Box<dyn Error>> {
    let seen: Seen = Arc::new(Mutex::new(Vec::new()));
    let log = Arc::clone(&seen);
    let router = Router::new().fallback(move |request: Request| {
        let log = Arc::clone(&log);
        async move {
            let (path, authorization, forward_auth) = snapshot(&request);
            drop(request);
            log.lock()
                .await
                .push((path.clone(), authorization.clone(), forward_auth.clone()));
            if path == "/api/v1/oauth/internal/token-context" {
                let presented = authorization.unwrap_or_default();
                if forward_auth.as_deref() != Some(FORWARD_SECRET) {
                    return StatusCode::NOT_FOUND.into_response();
                }
                if presented == format!("Bearer {}", token('a')) {
                    return Json(json!({
                        "active": true, "client_id": "ramflux",
                        "sub": "33333333-3333-4333-8333-333333333333",
                        "grant_id": "44444444-4444-4444-8444-444444444444",
                        "workspace_id": GRANT_WORKSPACE, "scope": "projects:read",
                        "project_mode": "all", "exp": 4_102_444_800_i64
                    }))
                    .into_response();
                }
                if presented == format!("Bearer {}", token('c')) {
                    return StatusCode::INTERNAL_SERVER_ERROR.into_response();
                }
                return StatusCode::UNAUTHORIZED.into_response();
            }
            Json(json!({"code": 0, "message": "ok", "data": []})).into_response()
        }
    });
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let addr = listener.local_addr()?;
    tokio::spawn(async move {
        let _ = axum::serve(listener, router).await;
    });
    Ok((format!("http://{addr}"), seen))
}

async fn free_port() -> Result<u16, Box<dyn Error>> {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let port = listener.local_addr()?.port();
    drop(listener);
    Ok(port)
}

struct Server {
    child: Child,
    base: String,
    _config: ConfigFile,
}

impl Server {
    async fn spawn(api_url: &str, transport: &str) -> Result<Self, Box<dyn Error>> {
        let port = free_port().await?;
        let bind = format!("127.0.0.1:{port}");
        let config = write_config_with_extra_mcp_keys(
            &McpSettings {
                api_url,
                bot_token: None,
                workspace_id: WORKSPACE,
                transport: Some(transport),
                bind_addr: Some(&bind),
            },
            &format!(
                "oauth_issuer = \"https://sv.example.test\"\noauth_resource = \"http://{bind}/mcp/rpc\"\noauth_forward_secret = \"{FORWARD_SECRET}\"\n"
            ),
        )?;
        let child = Command::new(env!("CARGO_BIN_EXE_mcp-server"))
            .args(["serve", "--config"])
            .arg(config.path())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .kill_on_drop(true)
            .spawn()?;
        let server = Self {
            child,
            base: format!("http://{bind}"),
            _config: config,
        };
        let client = reqwest::Client::new();
        for _ in 0..100 {
            if client
                .get(format!("{}/health", server.base))
                .send()
                .await
                .is_ok_and(|response| response.status().is_success())
            {
                return Ok(server);
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        Err("MCP server never became ready".into())
    }

    async fn stop(mut self) -> TestResult {
        self.child.start_kill()?;
        tokio::time::timeout(Duration::from_secs(10), self.child.wait()).await??;
        Ok(())
    }
}

async fn rpc(
    server: &Server,
    token: Option<&str>,
    body: Value,
) -> Result<(StatusCode, Option<String>, Value), Box<dyn Error>> {
    let mut request = reqwest::Client::new()
        .post(format!("{}/mcp/rpc", server.base))
        .json(&body);
    if let Some(token) = token {
        request = request.header("Authorization", format!("Bearer {token}"));
    }
    let response = request.send().await?;
    let status = response.status();
    let challenge = response
        .headers()
        .get("www-authenticate")
        .and_then(|value| value.to_str().ok())
        .map(str::to_string);
    let text = response.text().await?;
    Ok((status, challenge, serde_json::from_str(&text).unwrap_or(Value::Null)))
}

fn tool_names(body: &Value) -> Vec<String> {
    body.pointer("/result/tools")
        .and_then(Value::as_array)
        .map(|tools| {
            tools
                .iter()
                .filter_map(|tool| tool.get("name").and_then(Value::as_str).map(str::to_string))
                .collect()
        })
        .unwrap_or_default()
}

#[tokio::test]
async fn the_http_transport_serves_metadata_challenges_and_delegated_calls() -> TestResult {
    let (api, seen) = spawn_api().await?;
    let server = Server::spawn(&api, "http").await?;
    let list = json!({"jsonrpc": "2.0", "id": 1, "method": "tools/list"});
    let client = reqwest::Client::new();

    // Metadata at both paths, outside the token layer.
    let resource = format!("{}/mcp/rpc", server.base);
    for path in [
        "/.well-known/oauth-protected-resource",
        "/.well-known/oauth-protected-resource/mcp/rpc",
    ] {
        let document: Value = client
            .get(format!("{}{path}", server.base))
            .send()
            .await?
            .json()
            .await?;
        assert_eq!(
            document.get("resource").and_then(Value::as_str),
            Some(resource.as_str()),
            "{path}"
        );
        assert_eq!(
            document.get("authorization_servers"),
            Some(&json!(["https://sv.example.test"]))
        );
    }

    // No token: 401 with the discovery challenge.
    let (status, challenge, _) = rpc(&server, None, list.clone()).await?;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    let challenge = challenge.ok_or("no challenge")?;
    assert!(
        challenge.contains(&format!(
            "resource_metadata=\"{}/.well-known/oauth-protected-resource/mcp/rpc\"",
            server.base
        )),
        "{challenge}"
    );
    assert!(
        challenge.contains("scope=\"projects:read work_items:read\""),
        "{challenge}"
    );

    // A valid delegated token sees the three project tools, and a call acts in the grant's workspace.
    let (status, _, body) = rpc(&server, Some(&token('a')), list.clone()).await?;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(tool_names(&body).len(), 3, "{body}");
    let (status, _, _) = rpc(
        &server,
        Some(&token('a')),
        json!({"jsonrpc": "2.0", "id": 2, "method": "tools/call", "params": {"name": "projects.list", "arguments": {}}}),
    )
    .await?;
    assert_eq!(status, StatusCode::OK);
    {
        let seen = seen.lock().await.clone();
        let call = seen
            .iter()
            .find(|(path, _, _)| path.ends_with("/projects"))
            .ok_or("projects.list did not reach the API")?;
        assert_eq!(call.0, format!("/api/v1/workspaces/{GRANT_WORKSPACE}/projects"));
        assert_eq!(call.2.as_deref(), Some(FORWARD_SECRET));
    }

    // Out of scope: 403 with the scope in the challenge, API untouched.
    let api_calls = || async {
        seen.lock()
            .await
            .iter()
            .filter(|(path, _, _)| !path.contains("/oauth/internal/"))
            .count()
    };
    let before = api_calls().await;
    let (status, challenge, _) = rpc(
        &server,
        Some(&token('a')),
        json!({"jsonrpc": "2.0", "id": 3, "method": "tools/call", "params": {"name": "work_items.get_by_identifier", "arguments": {"identifier": "X-1"}}}),
    )
    .await?;
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert!(challenge.ok_or("no challenge")?.contains("scope=\"work_items:read\""));
    assert_eq!(api_calls().await, before, "a refused call reached the API");

    // Revoked or unknown: 401. The API failing: 503.
    assert_eq!(
        rpc(&server, Some(&token('b')), list.clone()).await?.0,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        rpc(&server, Some(&token('c')), list.clone()).await?.0,
        StatusCode::SERVICE_UNAVAILABLE
    );

    // A bot token is forwarded as before: the whole registry, no forwarding credential.
    let (status, _, body) = rpc(&server, Some(BOT_TOKEN), list).await?;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(tool_names(&body).len(), 153);
    let (status, _, _) = rpc(
        &server,
        Some(BOT_TOKEN),
        json!({"jsonrpc": "2.0", "id": 4, "method": "tools/call", "params": {"name": "projects.list", "arguments": {}}}),
    )
    .await?;
    assert_eq!(status, StatusCode::OK);
    {
        let seen = seen.lock().await.clone();
        let call = seen
            .iter()
            .rev()
            .find(|(path, _, _)| path.ends_with("/projects"))
            .ok_or("the bot call did not reach the API")?;
        assert_eq!(call.0, format!("/api/v1/workspaces/{WORKSPACE}/projects"));
        assert_eq!(call.1.as_deref(), Some(format!("Bearer {BOT_TOKEN}").as_str()));
        assert_eq!(call.2, None);
    }
    server.stop().await
}

#[tokio::test]
async fn the_sse_transport_serves_no_delegated_access_and_no_metadata() -> TestResult {
    let (api, seen) = spawn_api().await?;
    let server = Server::spawn(&api, "sse").await?;
    let client = reqwest::Client::new();
    assert_eq!(
        client
            .get(format!("{}/.well-known/oauth-protected-resource", server.base))
            .send()
            .await?
            .status(),
        StatusCode::NOT_FOUND
    );
    let response = client
        .post(format!("{}/messages?session_id=none", server.base))
        .header("Authorization", format!("Bearer {}", token('a')))
        .json(&json!({"jsonrpc": "2.0", "id": 1, "method": "tools/list"}))
        .send()
        .await?;
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    assert!(seen.lock().await.is_empty(), "a delegated token was forwarded");
    server.stop().await
}
