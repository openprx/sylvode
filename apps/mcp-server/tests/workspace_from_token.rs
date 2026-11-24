//! One MCP server for several workspaces: on the networked transports a caller acts in the
//! workspace its own bot token belongs to, asked of `GET /api/v1/auth/me`, never in a configured
//! one. The API is an in-process stand-in that records what it saw.

use axum::{
    Json, Router,
    extract::{Request, State},
    http::HeaderMap,
};
use mcp_server::{
    client::{ClientConfig, OpenPrClient, TRANSPORT_LABEL_HTTP, TRANSPORT_LABEL_STDIO},
    protocol::{CallToolResult, ToolContent},
    server::McpServer,
};
use platform::config::{McpToolMode, Secret};
use serde_json::{Value, json};
use std::sync::Arc;
use tokio::sync::Mutex;

const CONFIGURED: &str = "11111111-1111-4111-8111-111111111111";
const ALICE_WORKSPACE: &str = "aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa";
const BOB_WORKSPACE: &str = "bbbbbbbb-bbbb-4bbb-8bbb-bbbbbbbbbbbb";
const ALICE: &str = "svb_alice_workspace_bot_token";
const BOB: &str = "svb_bob_workspace_bot_token";
const PERSON: &str = "person.jwt.token";
const PROJECT: &str = "22222222-2222-4222-8222-222222222222";
const SCOPED_REFUSAL: &str = "this bot may not do this in this project";

#[derive(Clone, Default)]
struct Probe {
    seen: Arc<Mutex<Vec<(String, String, String)>>>,
    refuse_project_writes: bool,
}

impl Probe {
    async fn requests(&self) -> Vec<(String, String, String)> {
        self.seen.lock().await.clone()
    }
    async fn count(&self, path: &str) -> usize {
        self.requests().await.iter().filter(|(_, seen, _)| seen == path).count()
    }
}

fn bearer(headers: &HeaderMap) -> String {
    headers
        .get("authorization")
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("Bearer "))
        .unwrap_or_default()
        .to_string()
}

async fn answer(State(probe): State<Probe>, request: Request) -> Json<Value> {
    let (method, path) = (request.method().to_string(), request.uri().path().to_string());
    let token = bearer(request.headers());
    probe
        .seen
        .lock()
        .await
        .push((method.clone(), path.clone(), token.clone()));
    if path == "/api/v1/auth/me" {
        let workspace = match token.as_str() {
            ALICE => Some(ALICE_WORKSPACE),
            BOB => Some(BOB_WORKSPACE),
            _ => None,
        };
        let bot = workspace.map(|workspace| {
            json!({"bot_id": PROJECT, "workspace_id": workspace, "permissions": ["read", "write"], "project_scope": "projects"})
        });
        return Json(json!({"code": 0, "message": "ok", "data": {"user": {}, "bot": bot}}));
    }
    if probe.refuse_project_writes && method != "GET" {
        return Json(json!({"code": 403, "message": SCOPED_REFUSAL, "data": null}));
    }
    // Carries the ids the owner lookups read; an empty object is a valid agent policy.
    let data = if path.ends_with("/agent-policy") {
        json!({})
    } else {
        json!({"id": PROJECT, "project_id": PROJECT})
    };
    Json(json!({"code": 0, "message": "ok", "data": data}))
}

async fn spawn_api(probe: Probe) -> Result<String, Box<dyn std::error::Error>> {
    let router = Router::new().fallback(answer).with_state(probe);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let addr = listener.local_addr()?;
    tokio::spawn(async move {
        let _ = axum::serve(listener, router).await;
    });
    Ok(format!("http://{addr}"))
}

/// The client of a networked transport: no identity of its own, and a configured workspace that
/// no caller may use.
fn networked_client(base_url: String) -> Result<OpenPrClient, String> {
    OpenPrClient::new(ClientConfig {
        base_url,
        credential: None,
        workspace_id: Some(CONFIGURED.to_string()),
        transport_label: TRANSPORT_LABEL_HTTP,
    })
}

fn text(result: &CallToolResult) -> String {
    result
        .content
        .iter()
        .filter_map(|content| match content {
            ToolContent::Text { text } => Some(text.as_str()),
            _ => None,
        })
        .collect()
}

type TestResult = Result<(), Box<dyn std::error::Error>>;

#[tokio::test]
async fn each_caller_acts_in_the_workspace_of_its_own_token() -> TestResult {
    let probe = Probe::default();
    let client = networked_client(spawn_api(probe.clone()).await?)?;
    for (token, workspace) in [(ALICE, ALICE_WORKSPACE), (BOB, BOB_WORKSPACE)] {
        let server = McpServer::new(client.acting_as(Secret::new(token)), McpToolMode::Full);
        let result = server.call_tool("projects.list", json!({})).await;
        assert_ne!(result.is_error, Some(true), "{}", text(&result));
        assert!(
            probe.requests().await.iter().any(|(method, path, seen)| method == "GET"
                && path == &format!("/api/v1/workspaces/{workspace}/projects")
                && seen == token),
            "{token} did not act in {workspace}"
        );
    }
    assert_eq!(
        probe.count(&format!("/api/v1/workspaces/{CONFIGURED}/projects")).await,
        0,
        "the configured workspace was used by a networked caller"
    );
    Ok(())
}

#[tokio::test]
async fn the_lookup_is_made_once_per_token_and_only_when_a_tool_needs_a_workspace() -> TestResult {
    let probe = Probe::default();
    let client = networked_client(spawn_api(probe.clone()).await?)?;
    let server = McpServer::new(client.acting_as(Secret::new(ALICE)), McpToolMode::Full);

    // A tool that names its own project needs no workspace: no lookup.
    let result = server
        .call_tool("work_items.get", json!({"work_item_id": PROJECT}))
        .await;
    assert_ne!(result.is_error, Some(true), "{}", text(&result));
    assert_eq!(probe.count("/api/v1/auth/me").await, 0);

    for _ in 0..3 {
        let result = server.call_tool("labels.list", json!({})).await;
        assert_ne!(result.is_error, Some(true), "{}", text(&result));
    }
    // A second request of the same caller is a new client made from the shared one.
    let again = McpServer::new(client.acting_as(Secret::new(ALICE)), McpToolMode::Full);
    let result = again.call_tool("labels.list", json!({})).await;
    assert_ne!(result.is_error, Some(true), "{}", text(&result));
    assert_eq!(
        probe.count("/api/v1/auth/me").await,
        1,
        "the answer was not cached per token"
    );
    assert_eq!(
        probe
            .count(&format!("/api/v1/workspaces/{ALICE_WORKSPACE}/labels"))
            .await,
        4
    );
    Ok(())
}

#[tokio::test]
async fn a_token_without_a_bot_is_refused_with_401_and_reaches_no_workspace_route() -> TestResult {
    let probe = Probe::default();
    let client = networked_client(spawn_api(probe.clone()).await?)?;
    let server = McpServer::new(client.acting_as(Secret::new(PERSON)), McpToolMode::Full);
    let result = server.call_tool("projects.list", json!({})).await;
    assert_eq!(result.is_error, Some(true));
    let message = text(&result);
    assert!(message.contains("401"), "{message}");
    assert!(!message.contains(PERSON), "the token is echoed: {message}");
    let requests = probe.requests().await;
    assert!(
        requests.iter().all(|(_, path, _)| path == "/api/v1/auth/me"),
        "{requests:?}"
    );
    // A failed lookup is not remembered: the next call asks again.
    let _ = server.call_tool("projects.list", json!({})).await;
    assert_eq!(probe.count("/api/v1/auth/me").await, 2);
    Ok(())
}

#[tokio::test]
async fn a_client_with_a_fixed_workspace_never_asks_the_api_for_one() -> TestResult {
    let probe = Probe::default();
    let client = OpenPrClient::new(ClientConfig {
        base_url: spawn_api(probe.clone()).await?,
        credential: Some(Secret::new(ALICE)),
        workspace_id: Some(CONFIGURED.to_string()),
        transport_label: TRANSPORT_LABEL_STDIO,
    })?;
    let result = McpServer::new(client, McpToolMode::Full)
        .call_tool("labels.list", json!({}))
        .await;
    assert_ne!(result.is_error, Some(true), "{}", text(&result));
    assert_eq!(probe.count("/api/v1/auth/me").await, 0);
    assert_eq!(probe.count(&format!("/api/v1/workspaces/{CONFIGURED}/labels")).await, 1);
    Ok(())
}

#[tokio::test]
async fn a_scoped_refusal_reaches_the_client_as_a_clear_tool_error() -> TestResult {
    let probe = Probe {
        refuse_project_writes: true,
        ..Probe::default()
    };
    let client = networked_client(spawn_api(probe.clone()).await?)?;
    let server = McpServer::new(client.acting_as(Secret::new(ALICE)), McpToolMode::Full);
    for (tool, arguments) in [
        ("work_items.create", json!({"project_id": PROJECT, "title": "t"})),
        ("work_items.update", json!({"work_item_id": PROJECT, "title": "t"})),
        ("milestones.create", json!({"name": "Beta"})),
    ] {
        let result = server.call_tool(tool, arguments).await;
        assert_eq!(result.is_error, Some(true), "{tool}");
        let message = text(&result);
        assert!(message.contains(SCOPED_REFUSAL), "{tool}: {message}");
        assert!(message.contains("403"), "{tool}: {message}");
        assert!(message.contains("will not help"), "{tool}: {message}");
        assert!(!message.contains(ALICE), "{tool}: the token is echoed: {message}");
    }
    Ok(())
}
