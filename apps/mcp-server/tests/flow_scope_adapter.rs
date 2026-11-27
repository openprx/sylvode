//! Developer contract mocks only: no user runtime, database, or live authorization tests.
use axum::{
    Json, Router,
    body::Bytes,
    extract::State,
    http::{HeaderMap, Method, Uri},
    routing::any,
};
use mcp_server::{
    client::{ClientConfig, OpenPrClient, TRANSPORT_LABEL_HTTP},
    flow_runtime::{FlowRuntime, access_token},
    protocol::{JsonRpcRequest, ToolContent},
    server::McpServer,
};
use platform::config::McpToolMode;
use platform::config::{McpTransport, Secret};
use serde_json::{Value, json};
use std::sync::Arc;
use tokio::sync::Mutex;
const SA: &str = "11111111-1111-4111-8111-111111111111";
const SB: &str = "22222222-2222-4222-8222-222222222222";
const OBJECT: &str = "33333333-3333-4333-8333-333333333333";
const WORKSPACE: &str = "44444444-4444-4444-8444-444444444444";
const PROJECT: &str = "55555555-5555-4555-8555-555555555555";
#[derive(Default)]
struct Mock {
    calls: Vec<(String, String, String, Value)>,
    queries: Vec<(String, Option<String>)>,
    project: bool,
    deny_write: bool,
    resource_type: Option<String>,
    resources: std::collections::HashMap<String, Value>,
    deny_only_project: Option<String>,
    relations: Vec<Value>,
    fields: Vec<Value>,
    enabled_tools: Option<Vec<String>>,
    forms: std::collections::HashMap<String, Value>,
    form_records: std::collections::HashMap<String, Value>,
    intents: Vec<(String, Option<String>)>,
    endpoint_errors: std::collections::HashMap<String, Value>,
}
async fn handler(
    State(state): State<Arc<Mutex<Mock>>>,
    headers: HeaderMap,
    method: Method,
    uri: Uri,
    bytes: Bytes,
) -> Json<Value> {
    let token = headers
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default()
        .to_owned();
    let body = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
    let path = uri.path().to_owned();
    let mut state = state.lock().await;
    state
        .calls
        .push((method.to_string(), path.clone(), token.clone(), body.clone()));
    state.queries.push((path.clone(), uri.query().map(str::to_owned)));
    state.intents.push((
        path.clone(),
        headers
            .get("Idempotency-Key")
            .and_then(|key| key.to_str().ok())
            .map(str::to_owned),
    ));
    if let Some(error) = state.endpoint_errors.get(&uri.to_string()) {
        return Json(error.clone());
    }
    let scope = if token.contains("user-b") { SB } else { SA };
    if state.deny_write && matches!(method, Method::POST | Method::PATCH | Method::PUT | Method::DELETE) {
        return Json(
            json!({"code":403,"message":"denied","data":null,"error_code":"forbidden","details":{"action":"edit"}}),
        );
    }
    let resource_id = path.rsplit('/').next().unwrap_or_default();
    let custom = state
        .resources
        .get(resource_id)
        .filter(|_| path == format!("/api/v2/flow/objects/{resource_id}"));
    let data = if path.ends_with("/agent-policy") {
        state.enabled_tools.as_ref().map_or_else(
            || {
                if state
                    .deny_only_project
                    .as_ref()
                    .is_some_and(|project| !path.contains(project))
                {
                    json!({})
                } else {
                    json!({"mcp":{"tool_registry":{"enabled_tools":[]}}})
                }
            },
            |enabled| json!({"mcp":{"tool_registry":{"enabled_tools":enabled}}}),
        )
    } else if path.starts_with("/api/v1/forms/") {
        state.forms.get(resource_id).cloned().unwrap_or_else(|| json!({}))
    } else if path.starts_with("/api/v1/form-records/") {
        state
            .form_records
            .get(resource_id)
            .cloned()
            .unwrap_or_else(|| json!({}))
    } else if let Some(resource) = custom {
        resource.clone()
    } else if path == format!("/api/v2/flow/records/{OBJECT}/context") {
        json!({"record":state.resources.get(OBJECT),"fields":state.fields,"values_by_field_id":{},"field_secrecy_enabled":false,"schema_revision":"opaque-record-schema"})
    } else if path.ends_with("/relations") {
        json!({"items":state.relations,"next_cursor":null})
    } else if path == format!("/api/v2/flow/collections/{OBJECT}") {
        json!({"fields":state.fields})
    } else if path == "/api/v2/flow/home" {
        json!({"scope":{"id":scope,"kind":"personal"},"roots":[],"capabilities":{}})
    } else if path == format!("/api/v2/flow/workspaces/{WORKSPACE}/scope") {
        json!({"scope":{"id":SA,"kind":"legacy_managed"},"roots":[]})
    } else if path == "/api/v2/flow/objects" && method == Method::POST {
        json!({"object":{"id":OBJECT,"scope_id":body.get("scope_id"),"project_id":null,"object_type":body.get("object_type")}})
    } else if path.ends_with(&format!("/objects/{OBJECT}")) {
        if state.project {
            json!({"id":OBJECT,"scope_id":SA,"workspace_id":WORKSPACE,"project_id":if state.project{json!(PROJECT)}else{Value::Null},"object_type":state.resource_type.as_deref().unwrap_or("page")})
        } else {
            json!({"id":OBJECT,"scope_id":scope,"project_id":null,"object_type":state.resource_type.as_deref().unwrap_or("page")})
        }
    } else if path == format!("/api/v2/flow/objects/{SB}") {
        if state.project {
            json!({"id":SB,"scope_id":SA,"workspace_id":WORKSPACE,"project_id":if state.project{json!(PROJECT)}else{Value::Null},"object_type":"page"})
        } else {
            json!({"id":SB,"scope_id":scope,"project_id":null,"object_type":"page"})
        }
    } else {
        json!({"items":[]})
    };
    drop(state);
    Json(json!({"code":0,"message":"ok","data":data}))
}
async fn setup() -> Result<
    (
        OpenPrClient,
        Arc<Mutex<Mock>>,
        tokio::task::JoinHandle<std::io::Result<()>>,
    ),
    Box<dyn std::error::Error + Send + Sync>,
> {
    let state = Arc::new(Mutex::new(Mock::default()));
    let app = Router::new().route("/{*path}", any(handler)).with_state(state.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let url = format!("http://{}", listener.local_addr()?);
    let task = tokio::spawn(async move { axum::serve(listener, app).await });
    let client = OpenPrClient::new(ClientConfig {
        base_url: url,
        credential: None,
        workspace_id: Some(String::new()),
        transport_label: TRANSPORT_LABEL_HTTP,
    })?
    .flow_only();
    Ok((client, state, task))
}
fn text(result: &mcp_server::protocol::CallToolResult) -> String {
    result
        .content
        .iter()
        .filter_map(|c| match c {
            ToolContent::Text { text } => Some(text.as_str()),
            _ => None,
        })
        .collect()
}
#[tokio::test]
async fn static_registry_and_unrelated_dispatch_have_no_home_side_effect() {
    let (client, state, task) = setup().await.expect("mock fixture");
    let server = McpServer::new(client.acting_as(Secret::new("fixture-user-a")), McpToolMode::Full);
    let response = server
        .handle_request(JsonRpcRequest {
            jsonrpc: "2.0".into(),
            id: Some(json!(1)),
            method: "tools/list".into(),
            params: None,
        })
        .await
        .expect("response");
    let tools = response
        .result
        .expect("result")
        .get("tools")
        .and_then(Value::as_array)
        .expect("tools")
        .clone();
    assert_eq!(tools.len(), 36);
    assert!(
        tools
            .iter()
            .all(|v| mcp_server::tools::flow_scope::supports(v.get("name").and_then(Value::as_str).expect("name")))
    );
    let result = server.call_tool("work_items.delete", json!({"personal":true})).await;
    assert_eq!(result.is_error, Some(true));
    assert!(state.lock().await.calls.is_empty());
    task.abort();
}
#[tokio::test]
async fn acting_as_users_resolve_personal_scope_per_call_and_never_use_workspace_defaults() {
    let (client, state, task) = setup().await.expect("mock fixture");
    for token in ["fixture-user-a", "fixture-user-b"] {
        let server = McpServer::new(client.acting_as(Secret::new(token)), McpToolMode::Full);
        let result = server.call_tool("objects.query", json!({"personal":true})).await;
        assert_ne!(result.is_error, Some(true), "{}", text(&result));
    }
    let calls = &state.lock().await.calls;
    assert_eq!(calls.iter().filter(|v| v.1 == "/api/v2/flow/home").count(), 2);
    assert!(
        calls
            .iter()
            .any(|v| v.1 == format!("/api/v2/flow/scopes/{SA}/objects") && v.2 == "Bearer fixture-user-a")
    );
    assert!(
        calls
            .iter()
            .any(|v| v.1 == format!("/api/v2/flow/scopes/{SB}/objects") && v.2 == "Bearer fixture-user-b")
    );
    task.abort();
}
#[tokio::test]
async fn create_sends_real_scope_and_typed_command_keeps_backend_business_denial() {
    let (client, state, task) = setup().await.expect("mock fixture");
    let server = McpServer::new(client.acting_as(Secret::new("fixture-user-a")), McpToolMode::Full);
    let result = server
        .call_tool(
            "objects.create",
            json!({"personal":true,"type":"page","title":"Own source","idempotency_key":"create-1"}),
        )
        .await;
    assert_ne!(result.is_error, Some(true), "{}", text(&result));
    {
        let state = state.lock().await;
        let body = state
            .calls
            .iter()
            .find(|v| v.0 == "POST")
            .expect("create call")
            .3
            .clone();
        drop(state);
        assert_eq!(body.get("scope_id"), Some(&json!(SA)));
        assert!(body.get("workspace_id").is_none());
    }
    state.lock().await.deny_write = true;
    let result=server.call_tool("objects.command",json!({"object_id":OBJECT,"command":{"type":"set_title","payload":{"title":"Next"}},"idempotency_key":"command-1"})).await;
    assert_eq!(result.is_error, Some(true));
    let value: Value = serde_json::from_str(&text(&result)).expect("typed error");
    assert_eq!(value.pointer("/error/code").expect("code"), "forbidden");
    task.abort();
}
#[tokio::test]
async fn ambiguous_selectors_fail_before_home_lookup() {
    let (client, state, task) = setup().await.expect("mock fixture");
    for args in [
        json!({"personal":true,"workspace_id":WORKSPACE}),
        json!({}),
        json!({"object_id":OBJECT}),
    ] {
        let result = McpServer::new(client.acting_as(Secret::new("fixture-user-a")), McpToolMode::Full)
            .call_tool("objects.query", args)
            .await;
        assert_eq!(result.is_error, Some(true));
    }
    assert!(state.lock().await.calls.is_empty());
    task.abort();
}
#[tokio::test]
async fn legacy_project_policy_still_denies_actual_owner_even_without_a_claim() {
    let (client, state, task) = setup().await.expect("mock fixture");
    state.lock().await.project = true;
    let result = McpServer::new(client.acting_as(Secret::new("fixture-user-a")), McpToolMode::Full)
        .call_tool("objects.get", json!({"object_id":OBJECT}))
        .await;
    assert_eq!(result.is_error, Some(true));
    assert!(text(&result).contains("disabled by project agent policy"));
    assert_eq!(state.lock().await.calls.len(), 2);
    task.abort();
}
#[test]
fn explicit_flow_runtime_needs_no_workspace_and_never_reinterprets_bot_secret() {
    let secret = access_token("fixture-current-user-token").expect("token");
    let runtime = FlowRuntime::load(
        None,
        Some("http://127.0.0.1:1"),
        Some(&secret),
        None,
        Some(McpTransport::Stdio),
        None,
    )
    .expect("flow settings");
    assert!(runtime.workspace_id.is_none());
    assert!(!format!("{runtime:?}").contains("fixture-current-user-token"));
    assert!(access_token("opr_fixture_only").is_err());
    assert!(FlowRuntime::load(None, None, Some(&secret), None, Some(McpTransport::Http), None).is_err());
}

#[tokio::test]
async fn parented_creation_uses_actual_parent_project_policy_not_an_omitted_claim() {
    let (client, state, task) = setup().await.expect("mock fixture");
    state.lock().await.project = true;
    let result=McpServer::new(client.acting_as(Secret::new("fixture-user-a")), McpToolMode::Full).call_tool("objects.create",json!({"workspace_id":WORKSPACE,"parent_id":OBJECT,"type":"page","title":"Child","idempotency_key":"child-1"})).await;
    assert_eq!(result.is_error, Some(true));
    assert!(text(&result).contains("disabled by project agent policy"));
    let calls = &state.lock().await.calls;
    assert!(calls.iter().any(|call| call.1.ends_with("/agent-policy")));
    assert!(!calls.iter().any(|call| call.0 == "POST"));
    task.abort();
}

#[tokio::test]
async fn parent_scope_resolution_never_probes_a_shared_sources_private_navigator() {
    let (client, state, task) = setup().await.expect("mock fixture");
    let result = McpServer::new(client.acting_as(Secret::new("fixture-user-a")), McpToolMode::Full)
        .call_tool(
            "objects.create",
            json!({"scope_id":SA,"parent_id":OBJECT,"type":"page","title":"Child","idempotency_key":"child-1"}),
        )
        .await;
    assert_ne!(result.is_error, Some(true), "{}", text(&result));
    let calls = &state.lock().await.calls;
    assert!(
        !calls
            .iter()
            .any(|call| call.1.ends_with("/home") || call.1.ends_with("/navigator"))
    );
    assert_eq!(calls.len(), 2);
    task.abort();
}

#[tokio::test]
async fn typed_resource_commands_never_offer_a_raw_or_generic_block_bypass() {
    let (client, state, task) = setup().await.expect("mock fixture");
    state.lock().await.resource_type = Some("collection".into());
    let server = McpServer::new(client.acting_as(Secret::new("fixture-user-a")), McpToolMode::Full);
    let result=server.call_tool("objects.command",json!({"object_id":OBJECT,"command":{"type":"semantic_patch","payload":{"operations":[]}},"idempotency_key":"typed-1"})).await;
    assert_eq!(result.is_error, Some(true));
    assert!(text(&result).contains("typed server commands"));
    let result=server.call_tool("records.create",json!({"collection_id":OBJECT,"values_by_field_id":{"aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa":"typed"},"idempotency_key":"record-1"})).await;
    assert_ne!(result.is_error, Some(true), "{}", text(&result));
    let calls = &state.lock().await.calls;
    let posts: Vec<_> = calls.iter().filter(|call| call.0 == "POST").collect();
    assert_eq!(posts.len(), 1);
    assert_eq!(
        posts.first().expect("POST").1,
        format!("/api/v2/flow/collections/{OBJECT}/records")
    );
    assert!(posts.first().expect("POST").3.get("workspace_id").is_none());
    task.abort();
}

#[test]
fn cli_flow_entry_keeps_credentials_redacted_and_schema_listing_explicit() {
    use clap::{CommandFactory, Parser};
    use mcp_server::cli_app::{
        command::Cli,
        entry::{Route, route},
    };
    let secret = "fixture-current-user-secret";
    let cli = Cli::try_parse_from([
        "sylvode",
        "--access-token",
        secret,
        "flow",
        "call",
        "--name",
        "objects.query",
        "--args-json",
        "{\"personal\":true}",
    ])
    .expect("Flow CLI");
    assert!(!format!("{cli:?}").contains(secret));
    assert!(!Cli::command().render_long_help().to_string().contains(secret));
    let args: Vec<std::ffi::OsString> = ["sylvode", "--access-token-file", "/private/example", "flow", "tools"]
        .into_iter()
        .map(Into::into)
        .collect();
    assert_eq!(route(&args), Route::Flow);
    let token = access_token("invalid whitespace credential").expect_err("invalid token");
    assert!(!token.contains("invalid whitespace credential"));
}

#[tokio::test]
async fn a_workspace_bot_uses_the_v2_surface_and_keeps_backend_write_denial() {
    let (client, state, task) = setup().await.expect("mock fixture");
    let server = McpServer::new(client.acting_as(Secret::new("opr_fixture_only")), McpToolMode::Full);
    let result = server
        .call_tool("objects.query", json!({"workspace_id":WORKSPACE,"unprojected":true}))
        .await;
    assert_ne!(result.is_error, Some(true), "{}", text(&result));
    state.lock().await.deny_write = true;
    let result = server
        .call_tool(
            "objects.create",
            json!({"workspace_id":WORKSPACE,"type":"page","title":"Legacy","idempotency_key":"legacy-create"}),
        )
        .await;
    assert_eq!(result.is_error, Some(true));
    let calls = &state.lock().await.calls;
    assert!(
        calls
            .iter()
            .all(|call| call.1.starts_with("/api/v2/flow/") && call.2 == "Bearer opr_fixture_only")
    );
    assert!(
        calls
            .iter()
            .any(|call| call.1 == format!("/api/v2/flow/scopes/{SA}/objects"))
    );
    assert!(calls.iter().any(|call| call.1 == "/api/v2/flow/objects"));
    assert!(!calls.iter().any(|call| call.1.ends_with("/home")));
    task.abort();
}

#[tokio::test]
async fn canonical_object_claims_cannot_choose_an_unrelated_workspace_or_scope() {
    let (client, state, task) = setup().await.expect("mock fixture");
    let result = McpServer::new(client.acting_as(Secret::new("fixture-user-a")), McpToolMode::Full)
        .call_tool("objects.get", json!({"object_id":OBJECT,"scope_id":SB}))
        .await;
    assert_eq!(result.is_error, Some(true));
    assert!(text(&result).contains("canonical Flow resource"));
    assert_eq!(state.lock().await.calls.len(), 1);
    task.abort();
}

#[test]
fn profile_schemas_do_not_claim_unimplemented_package_or_historical_reads() {
    let definitions = mcp_server::tools::flow_scope::definitions();
    let export = definitions
        .iter()
        .find(|tool| tool.name == "objects.export")
        .expect("export");
    assert_eq!(
        export
            .input_schema
            .pointer("/properties/format/enum")
            .expect("format enum"),
        &json!(["json", "markdown", "csv"])
    );
    assert!(export.input_schema.pointer("/properties/include_history").is_none());
    let get = definitions.iter().find(|tool| tool.name == "objects.get").expect("get");
    assert!(get.input_schema.pointer("/properties/at_seq").is_some());
    assert!(get.input_schema.pointer("/properties/personal").is_none());
    let query = definitions
        .iter()
        .find(|tool| tool.name == "objects.query")
        .expect("query");
    assert_eq!(
        query
            .input_schema
            .get("oneOf")
            .and_then(Value::as_array)
            .expect("selectors")
            .len(),
        3
    );
}

#[tokio::test]
async fn existing_sylvode_binary_flow_call_reuses_the_mocked_mcp_path_without_workspace_config() {
    let state = Arc::new(Mutex::new(Mock::default()));
    // Ephemeral mock only, never the protected manual-view API or database.
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.expect("CLI mock");
    let address = format!("http://{}", listener.local_addr().expect("address"));
    let app = Router::new().route("/{*path}", any(handler)).with_state(state.clone());
    let cli_task = tokio::spawn(async move {
        axum::serve(listener, app).await.expect("CLI mock server");
    });
    let output = tokio::process::Command::new(env!("CARGO_BIN_EXE_sylvode"))
        .args([
            "--api-url",
            &address,
            "--access-token",
            "fixture-user-a",
            "flow",
            "call",
            "--name",
            "objects.query",
            "--args-json",
            "{\"personal\":true}",
        ])
        .output()
        .await
        .expect("CLI process");
    assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
    let result: Value = serde_json::from_slice(&output.stdout).expect("stable JSON output");
    assert!(result.get("data").is_some());
    assert!(!String::from_utf8_lossy(&output.stdout).contains("fixture-user-a"));
    assert_eq!(state.lock().await.calls.len(), 2);
    cli_task.abort();
}

#[test]
fn private_token_file_and_flow_config_do_not_weaken_legacy_workspace_validation() {
    use std::io::Write as _;
    let directory = std::env::temp_dir().join(format!("sv-flow-mcp-contract-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir(&directory).expect("private fixture directory");
    let token_file = directory.join("access-token");
    let mut file = std::fs::File::create(&token_file).expect("fixture token file");
    file.write_all(b"fixture-file-token\n").expect("fixture bytes");
    drop(file);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&token_file, std::fs::Permissions::from_mode(0o600)).expect("private permissions");
    }
    let runtime = FlowRuntime::load(
        None,
        Some("http://127.0.0.1:1"),
        None,
        Some(&token_file),
        Some(McpTransport::Stdio),
        None,
    )
    .expect("explicit user file");
    assert!(runtime.workspace_id.is_none());
    assert!(!format!("{runtime:?}").contains("fixture-file-token"));
    let config = directory.join("flow.toml");
    std::fs::write(
        &config,
        "[mcp]\napi_url = 'http://127.0.0.1:1'\ntransport = 'http'\nbot_token = 'opr_fixture_only'\n",
    )
    .expect("fixture config");
    let loaded = platform::config::OpenPrConfig::load(Some(&config)).expect("structural config");
    assert!(
        loaded
            .mcp_runtime()
            .expect("a networked runtime needs no configured workspace")
            .workspace_id
            .is_none(),
        "a networked caller's workspace is its token's, not the file's"
    );
    let stdio_config = directory.join("stdio.toml");
    std::fs::write(
        &stdio_config,
        "[mcp]\napi_url = 'http://127.0.0.1:1'\ntransport = 'stdio'\nbot_token = 'opr_fixture_only'\n",
    )
    .expect("fixture config");
    let stdio = platform::config::OpenPrConfig::load(Some(&stdio_config)).expect("structural config");
    assert!(
        stdio.mcp_runtime().is_err(),
        "the stdio runtime still requires a real workspace"
    );
    let flow = FlowRuntime::load(Some(&config), None, None, None, None, None).expect("network Flow-only runtime");
    assert!(flow.workspace_id.is_none());
    assert!(
        flow.credential.is_none(),
        "configured bot is not network caller fallback"
    );
    assert_eq!(flow.transport, McpTransport::Http);
    std::fs::remove_dir_all(directory).expect("remove only this synthetic file fixture");
}

#[tokio::test]
async fn human_legacy_search_uses_canonical_scope_after_explicit_partition_validation() {
    let (client, state, task) = setup().await.expect("mock fixture");
    let server = McpServer::new(client.acting_as(Secret::new("fixture-user-a")), McpToolMode::Full);
    let denied = server
        .call_tool("objects.search", json!({"workspace_id":WORKSPACE,"q":"needle"}))
        .await;
    assert_eq!(denied.is_error, Some(true));
    let result = server
        .call_tool(
            "objects.search",
            json!({"workspace_id":WORKSPACE,"unprojected":true,"q":"needle"}),
        )
        .await;
    assert_ne!(result.is_error, Some(true), "{}", text(&result));
    let calls = &state.lock().await.calls;
    assert!(calls.iter().any(|v| v.1 == format!("/api/v2/flow/scopes/{SA}/search")));
    assert!(!calls.iter().any(|v| v.1.starts_with("/api/v1/")));
    task.abort();
}

#[tokio::test]
async fn explicit_editor_initializer_requires_captured_frontier_and_no_client_content() {
    let (client, state, task) = setup().await.expect("mock fixture");
    let server = McpServer::new(client.acting_as(Secret::new("fixture-user-a")), McpToolMode::Full);
    for args in [
        json!({"object_id":OBJECT,"command":{"type":"initialize_editor_doc","payload":{}},"idempotency_key":"init"}),
        json!({"object_id":OBJECT,"command":{"type":"initialize_editor_doc","payload":{"blocks":[]}},"expected_frontier":"fenced","idempotency_key":"init"}),
    ] {
        assert_eq!(server.call_tool("objects.command", args).await.is_error, Some(true));
    }
    assert!(!state.lock().await.calls.iter().any(|call| call.0 == "POST"));
    let result=server.call_tool("objects.command",json!({"object_id":OBJECT,"command":{"type":"initialize_editor_doc","payload":{}},"expected_frontier":"fenced","idempotency_key":"init"})).await;
    assert_ne!(result.is_error, Some(true), "{}", text(&result));
    {
        let calls = &state.lock().await.calls;
        let sent = calls
            .iter()
            .find(|call| call.0 == "POST")
            .expect("explicit initialization request");
        assert_eq!(sent.1, format!("/api/v2/flow/objects/{OBJECT}/commands"));
        assert_eq!(
            sent.3,
            json!({"command":{"type":"initialize_editor_doc","payload":{}},"expected_frontier":"fenced","idempotency_key":"init"})
        );
    }
    state.lock().await.resource_type = Some("collection".into());
    let refused=server.call_tool("objects.command",json!({"object_id":OBJECT,"command":{"type":"initialize_editor_doc","payload":{}},"expected_frontier":"fenced","idempotency_key":"typed-init"})).await;
    assert_eq!(refused.is_error, Some(true));
    assert_eq!(
        state.lock().await.calls.iter().filter(|call| call.0 == "POST").count(),
        1
    );
    task.abort();
}

#[tokio::test]
async fn metadata_relation_commands_preserve_payload_without_fabricating_document_frontier() {
    let (client, state, task) = setup().await.expect("mock fixture");
    let server = McpServer::new(client.acting_as(Secret::new("fixture-user-a")), McpToolMode::Full);
    let payload = json!({"target_object_id":SB,"relation_type":"reference","properties":{"label":"related"}});
    let refused=server.call_tool("objects.command",json!({"object_id":OBJECT,"command":{"type":"link","payload":payload},"expected_frontier":"incorrect-head","idempotency_key":"link"})).await;
    assert_eq!(refused.is_error, Some(true));
    let result = server
        .call_tool(
            "objects.command",
            json!({"object_id":OBJECT,"command":{"type":"link","payload":payload},"idempotency_key":"link"}),
        )
        .await;
    assert_ne!(result.is_error, Some(true), "{}", text(&result));
    let calls = &state.lock().await.calls;
    let body = &calls
        .iter()
        .find(|call| call.0 == "POST")
        .expect("explicit relation call")
        .3;
    assert_eq!(
        body.get("command").and_then(|command| command.get("payload")),
        Some(&payload)
    );
    assert!(body.get("expected_frontier").is_none());
    task.abort();
}

const OTHER_PROJECT: &str = "66666666-6666-4666-8666-666666666666";
fn legacy_resource(id: &str, project: &str, kind: &str) -> Value {
    json!({"id":id,"scope_id":SA,"workspace_id":WORKSPACE,"project_id":project,"object_type":kind})
}
#[tokio::test]
async fn every_link_move_and_record_target_project_is_checked_before_mutation() {
    for credential in ["fixture-user-a", "opr_fixture_token"] {
        for kind in ["link", "move_object", "record_patch", "record_archive"] {
            let (client, state, task) = setup().await.expect("mock fixture");
            {
                let mut mock = state.lock().await;
                mock.resources.insert(
                    OBJECT.into(),
                    legacy_resource(
                        OBJECT,
                        PROJECT,
                        if kind.starts_with("record_") {
                            "collection"
                        } else {
                            "page"
                        },
                    ),
                );
                mock.resources.insert(
                    SB.into(),
                    legacy_resource(
                        SB,
                        OTHER_PROJECT,
                        if kind.starts_with("record_") { "record" } else { "page" },
                    ),
                );
                mock.deny_only_project = Some(OTHER_PROJECT.into());
            }
            let payload = if kind.starts_with("record_") {
                json!({"record_id":SB,"body":"changed"})
            } else {
                json!({"target_object_id":SB,"relation_type":"reference"})
            };
            let server = McpServer::new(client.acting_as(Secret::new(credential)), McpToolMode::Full);
            let result=server.call_tool("objects.command",json!({"object_id":OBJECT,"command":{"type":kind,"payload":payload},"idempotency_key":"target-gate"})).await;
            assert_eq!(result.is_error, Some(true), "{kind}: {}", text(&result));
            assert!(
                text(&result).contains("disabled by project agent policy"),
                "{}",
                text(&result)
            );
            let calls = &state.lock().await.calls;
            assert!(
                calls
                    .iter()
                    .any(|call| call.1.contains(OTHER_PROJECT) && call.1.ends_with("/agent-policy"))
            );
            assert!(
                !calls.iter().any(|call| call.0 == "POST"),
                "No {kind} mutation after target denial"
            );
            task.abort();
        }
    }
}
#[tokio::test]
async fn unlink_uses_actual_visible_relation_target_and_missing_target_fails_closed() {
    for available in [true, false] {
        let (client, state, task) = setup().await.expect("mock fixture");
        {
            let mut mock = state.lock().await;
            mock.resources
                .insert(OBJECT.into(), legacy_resource(OBJECT, PROJECT, "page"));
            mock.resources
                .insert(SB.into(), legacy_resource(SB, OTHER_PROJECT, "page"));
            mock.deny_only_project = Some(OTHER_PROJECT.into());
            if available {
                mock.relations.push(
                    json!({"visibility":"visible","relation_id":SB,"direction":"outgoing","other_object":{"id":SB}}),
                );
            }
            drop(mock);
        }
        let server = McpServer::new(client.acting_as(Secret::new("fixture-user-a")), McpToolMode::Full);
        let result=server.call_tool("objects.command",json!({"object_id":OBJECT,"command":{"type":"unlink","payload":{"relation_id":SB}},"idempotency_key":"unlink-gate"})).await;
        assert_eq!(result.is_error, Some(true));
        assert!(
            text(&result).contains(if available {
                "disabled by project agent policy"
            } else {
                "could not be resolved"
            }),
            "{}",
            text(&result)
        );
        assert!(!state.lock().await.calls.iter().any(|call| call.0 == "POST"));
        task.abort();
    }
}
#[tokio::test]
async fn typed_relation_values_resolve_only_declared_relation_fields_and_actual_target_project() {
    let (client, state, task) = setup().await.expect("mock fixture");
    {
        let mut mock = state.lock().await;
        mock.resources
            .insert(OBJECT.into(), legacy_resource(OBJECT, PROJECT, "collection"));
        mock.resources
            .insert(SB.into(), legacy_resource(SB, OTHER_PROJECT, "page"));
        mock.fields.push(json!({"field_id":SA,"type":"relation"}));
        mock.deny_only_project = Some(OTHER_PROJECT.into());
    }
    let server = McpServer::new(client.acting_as(Secret::new("fixture-user-a")), McpToolMode::Full);
    let result = server
        .call_tool(
            "records.create",
            json!({"collection_id":OBJECT,"values_by_field_id":{SA:[SB]},"idempotency_key":"relation-field"}),
        )
        .await;
    assert_eq!(result.is_error, Some(true));
    assert!(
        text(&result).contains("disabled by project agent policy"),
        "{}",
        text(&result)
    );
    assert!(!state.lock().await.calls.iter().any(|call| call.0 == "POST"));
    task.abort();
}

#[tokio::test]
async fn permitted_multi_project_targets_dispatch_and_plain_text_ids_are_not_ownership_claims() {
    let (client, state, task) = setup().await.expect("mock fixture");
    {
        let mut mock = state.lock().await;
        mock.resources
            .insert(OBJECT.into(), legacy_resource(OBJECT, PROJECT, "collection"));
        mock.resources
            .insert(SB.into(), legacy_resource(SB, OTHER_PROJECT, "page"));
        mock.deny_only_project = Some(SA.into()); // neither real project is denied
    }
    let server = McpServer::new(client.acting_as(Secret::new("fixture-user-a")), McpToolMode::Full);
    let result=server.call_tool("objects.command",json!({"object_id":OBJECT,"command":{"type":"link","payload":{"target_object_id":SB,"relation_type":"reference"}},"idempotency_key":"both-allowed"})).await;
    assert_ne!(result.is_error, Some(true), "{}", text(&result));
    {
        let mut mock = state.lock().await;
        mock.calls.clear();
        mock.deny_only_project = Some(OTHER_PROJECT.into());
        mock.fields.push(json!({"field_id":SA,"type":"text"}));
        drop(mock);
    }
    let result = server
        .call_tool(
            "records.create",
            json!({"collection_id":OBJECT,"values_by_field_id":{SA:SB},"idempotency_key":"text-id"}),
        )
        .await;
    assert_ne!(result.is_error, Some(true), "{}", text(&result));
    let calls = &state.lock().await.calls;
    assert!(calls.iter().any(|call| call.0 == "POST"));
    assert!(!calls.iter().any(|call| call.1.ends_with(&format!("/objects/{SB}"))));
    task.abort();
}

#[tokio::test]
async fn missing_target_project_metadata_is_not_inferred_as_unprojected() {
    let (client, state, task) = setup().await.expect("mock fixture");
    {
        let mut mock = state.lock().await;
        mock.resources
            .insert(OBJECT.into(), legacy_resource(OBJECT, PROJECT, "page"));
        mock.resources.insert(
            SB.into(),
            json!({"id":SB,"scope_id":SA,"workspace_id":WORKSPACE,"object_type":"page"}),
        );
        mock.deny_only_project = Some(OTHER_PROJECT.into());
    }
    let server = McpServer::new(client.acting_as(Secret::new("fixture-user-a")), McpToolMode::Full);
    let result=server.call_tool("objects.command",json!({"object_id":OBJECT,"command":{"type":"link","payload":{"target_object_id":SB,"relation_type":"reference"}},"idempotency_key":"missing-owner"})).await;
    assert_eq!(result.is_error, Some(true));
    assert!(
        text(&result).contains("project ownership metadata"),
        "{}",
        text(&result)
    );
    assert!(!state.lock().await.calls.iter().any(|call| call.0 == "POST"));
    task.abort();
}

#[tokio::test]
async fn generic_command_alias_cannot_bypass_denied_existing_patch_or_link_tool_names() {
    for (kind, baseline, payload) in [
        (
            "semantic_patch",
            "objects.patch",
            json!({"operations":[{"op":"create_node","id":"agent-block","parent":null,"index":0,"kind":"block"}]}),
        ),
        (
            "link",
            "objects.link",
            json!({"target_object_id":SB,"relation_type":"reference"}),
        ),
    ] {
        let (client, state, task) = setup().await.expect("mock fixture");
        {
            let mut mock = state.lock().await;
            mock.resources
                .insert(OBJECT.into(), legacy_resource(OBJECT, PROJECT, "page"));
            mock.resources.insert(SB.into(), legacy_resource(SB, PROJECT, "page"));
            mock.enabled_tools = Some(vec!["objects.command".into()]);
        }
        let server = McpServer::new(client.acting_as(Secret::new("fixture-user-a")), McpToolMode::Full);
        let result=server.call_tool("objects.command",json!({"object_id":OBJECT,"command":{"type":kind,"payload":payload},"idempotency_key":"baseline-policy"})).await;
        assert_eq!(result.is_error, Some(true));
        assert!(text(&result).contains(baseline), "{}", text(&result));
        assert!(!state.lock().await.calls.iter().any(|call| call.0 == "POST"));
        task.abort();
    }
}

#[tokio::test]
async fn existing_social_and_access_names_use_scope_native_verbs_without_default_workspace() {
    let (client, state, task) = setup().await.expect("mock fixture");
    let server = McpServer::new(client.acting_as(Secret::new("fixture-user-a")), McpToolMode::Full);
    for (name, args, verb, suffix) in [
        ("objects.grants_get", json!({"object_id":OBJECT}), "GET", "grants"),
        (
            "objects.grants_set",
            json!({"object_id":OBJECT,"grants":[],"dry_run":true,"idempotency_key":"grants"}),
            "PUT",
            "grants",
        ),
        (
            "objects.inheritance_set",
            json!({"object_id":OBJECT,"inherit_from_parent":false,"dry_run":true,"idempotency_key":"inherit"}),
            "PUT",
            "inheritance",
        ),
        ("objects.relations", json!({"object_id":OBJECT}), "GET", "relations"),
        ("flow.comments.list", json!({"object_id":OBJECT}), "GET", "comments"),
        (
            "flow.comments.create",
            json!({"object_id":OBJECT,"body":"comment","idempotency_key":"comment-create"}),
            "POST",
            "comments",
        ),
        (
            "flow.comments.update",
            json!({"object_id":OBJECT,"flow_comment_id":SB,"body":"edit","idempotency_key":"comment-edit"}),
            "PATCH",
            &format!("comments/{SB}"),
        ),
        (
            "flow.comments.delete",
            json!({"object_id":OBJECT,"flow_comment_id":SB,"idempotency_key":"comment-delete"}),
            "DELETE",
            &format!("comments/{SB}"),
        ),
        (
            "flow.comments.resolve",
            json!({"object_id":OBJECT,"flow_comment_id":SB,"resolved":true}),
            "POST",
            &format!("comments/{SB}/resolve"),
        ),
        ("flow.shares.list", json!({"object_id":OBJECT}), "GET", "share-links"),
        ("flow.shares.create", json!({"object_id":OBJECT}), "POST", "share-links"),
        (
            "flow.shares.revoke",
            json!({"object_id":OBJECT,"link_id":SB}),
            "POST",
            &format!("share-links/{SB}/revoke"),
        ),
        (
            "flow.appearance.set",
            json!({"object_id":OBJECT,"icon":{"kind":"emoji","emoji":"leaf"},"idempotency_key":"appearance"}),
            "PUT",
            "appearance",
        ),
        (
            "flow.appearance.upload",
            json!({"object_id":OBJECT,"data_base64":"synthetic-base64"}),
            "POST",
            "appearance/assets",
        ),
        (
            "flow.mentions.list",
            json!({"object_id":OBJECT,"q":"candidate"}),
            "GET",
            "mention-candidates",
        ),
        ("objects.purge", json!({"object_id":OBJECT}), "POST", "purge"),
    ] {
        let prepared =
            mcp_server::tools::flow_scope::prepare(&client.acting_as(Secret::new("fixture-user-a")), name, &args)
                .await
                .expect("prepared adapter");
        assert_eq!(
            prepared.path.split('?').next(),
            Some(format!("/api/v2/flow/objects/{OBJECT}/{suffix}").as_str())
        );
        let result = server.call_tool(name, args).await;
        assert_ne!(result.is_error, Some(true), "{name}: {}", text(&result));
        let dto: Value = serde_json::from_str(&text(&result)).expect("baseline success DTO");
        assert!(
            dto.get("code").is_none(),
            "Named tool keeps baseline data DTO, not a REST envelope"
        );
        let calls = &state.lock().await.calls;
        let last = calls.last().expect("adapter request");
        assert_eq!(last.0, verb);
        assert_eq!(last.1, format!("/api/v2/flow/objects/{OBJECT}/{suffix}"));
        assert!(!last.3.to_string().contains("workspace_id"));
    }
    let mock = state.lock().await;
    assert!(
        mock.intents
            .iter()
            .any(|(path, key)| path.ends_with("/comments") && key.as_deref() == Some("comment-create"))
    );
    assert!(
        mock.intents
            .iter()
            .any(|(path, key)| path.ends_with("/appearance") && key.as_deref() == Some("appearance"))
    );
    assert!(!mock.calls.iter().any(|call| call.1.ends_with("/home")));
    drop(mock);
    task.abort();
}
#[tokio::test]
async fn named_patch_and_link_keep_existing_policy_names_without_requiring_generic_alias_permission() {
    for (name, args) in [
        (
            "objects.patch",
            json!({"object_id":OBJECT,"operations":[{"op":"create_node","id":"block","parent":null,"index":0,"kind":"block"}],"idempotency_key":"patch"}),
        ),
        (
            "objects.link",
            json!({"source_object_id":OBJECT,"target_object_id":SB,"relation_type":"reference","idempotency_key":"link"}),
        ),
    ] {
        let (client, state, task) = setup().await.expect("mock fixture");
        {
            let mut mock = state.lock().await;
            mock.resources
                .insert(OBJECT.into(), legacy_resource(OBJECT, PROJECT, "page"));
            mock.resources.insert(SB.into(), legacy_resource(SB, PROJECT, "page"));
            mock.enabled_tools = Some(vec![name.to_owned()]);
        }
        let server = McpServer::new(client.acting_as(Secret::new("fixture-user-a")), McpToolMode::Full);
        let result = server.call_tool(name, args).await;
        assert_ne!(result.is_error, Some(true), "{}", text(&result));
        assert!(state.lock().await.calls.iter().any(|call| call.0 == "POST"));
        task.abort();
    }
}
#[tokio::test]
async fn forms_reference_checks_actual_destination_project_fresh_and_cleanup_does_not_require_destination() {
    let (client, state, task) = setup().await.expect("mock fixture");
    {
        let mut mock = state.lock().await;
        mock.forms.insert(SB.into(), json!({"id":SB,"project_id":PROJECT}));
        mock.deny_only_project = Some(OTHER_PROJECT.into());
    }
    let server = McpServer::new(client.acting_as(Secret::new("fixture-user-a")), McpToolMode::Full);
    let args = json!({"object_id":OBJECT,"target_type":"form","target_id":SB,"idempotency_key":"reference"});
    let first = server.call_tool("objects.reference", args.clone()).await;
    assert_ne!(first.is_error, Some(true), "{}", text(&first));
    state
        .lock()
        .await
        .forms
        .insert(SB.into(), json!({"id":SB,"project_id":OTHER_PROJECT}));
    let second = server.call_tool("objects.reference", args).await;
    assert_eq!(second.is_error, Some(true));
    assert!(text(&second).contains("disabled by project agent policy"));
    {
        let mock = state.lock().await;
        assert_eq!(
            mock.calls
                .iter()
                .filter(|call| call.1 == format!("/api/v1/forms/{SB}"))
                .count(),
            2
        );
        assert_eq!(mock.calls.iter().filter(|call| call.0 == "POST").count(), 1);
        drop(mock);
    }
    let removed = server
        .call_tool(
            "objects.unreference",
            json!({"object_id":OBJECT,"reference_id":SB,"idempotency_key":"cleanup"}),
        )
        .await;
    assert_ne!(removed.is_error, Some(true), "{}", text(&removed));
    let mock = state.lock().await;
    assert_eq!(mock.calls.last().expect("cleanup").0, "DELETE");
    assert_eq!(
        mock.calls
            .iter()
            .filter(|call| call.1 == format!("/api/v1/forms/{SB}"))
            .count(),
        2
    );
    assert!(
        mock.intents
            .iter()
            .any(|(path, key)| path.ends_with(&format!("/references/{SB}")) && key.as_deref() == Some("cleanup"))
    );
    drop(mock);
    task.abort();
}
#[tokio::test]
async fn form_record_reference_uses_real_parent_form_project_and_scope_lists_are_explicit() {
    let (client, state, task) = setup().await.expect("mock fixture");
    {
        let mut mock = state.lock().await;
        mock.form_records.insert(SB.into(), json!({"id":SB,"form_id":SA}));
        mock.forms
            .insert(SA.into(), json!({"id":SA,"project_id":OTHER_PROJECT}));
        mock.deny_only_project = Some(OTHER_PROJECT.into());
    }
    let server = McpServer::new(client.acting_as(Secret::new("fixture-user-a")), McpToolMode::Full);
    let result = server
        .call_tool(
            "objects.reference",
            json!({"object_id":OBJECT,"target_type":"form_record","target_id":SB,"idempotency_key":"record-reference"}),
        )
        .await;
    assert_eq!(result.is_error, Some(true));
    assert!(!state.lock().await.calls.iter().any(|call| call.0 == "POST"));
    assert_ne!(
        server
            .call_tool("flow.trash.list", json!({"personal":true}))
            .await
            .is_error,
        Some(true)
    );
    assert_eq!(
        state.lock().await.calls.last().expect("scope trash").1,
        format!("/api/v2/flow/scopes/{SA}/trash")
    );
    assert_eq!(
        server.call_tool("flow.trash.list", json!({})).await.is_error,
        Some(true)
    );
    task.abort();
}

#[test]
fn actual_baseline_flow_registry_matrix_is_explicit_and_never_invents_pending_tools() {
    let baseline = mcp_server::tools::get_all_tool_definitions();
    let flow = baseline
        .iter()
        .filter(|tool| {
            ["flow.", "objects.", "collections.", "records.", "collab."]
                .iter()
                .any(|prefix| tool.name.starts_with(prefix))
        })
        .collect::<Vec<_>>();
    let profile = mcp_server::tools::flow_scope::definitions();
    let profile_names = profile
        .iter()
        .map(|tool| tool.name.as_str())
        .collect::<std::collections::BTreeSet<_>>();
    let initial = mcp_server::tools::flow_scope::TOOLS
        .iter()
        .copied()
        .collect::<std::collections::BTreeSet<_>>();
    assert_eq!(initial.len(), 11);
    assert_eq!(profile_names.len(), 36);
    assert_eq!(
        flow.iter().filter(|tool| initial.contains(tool.name.as_str())).count(),
        9
    );
    for pending in [
        "objects.restore",
        "objects.export_workspace",
        "objects.import_artifact",
        "objects.import_preview",
        "objects.import_commit",
        "objects.import_status",
        "objects.convert_preview",
        "collab.compact",
    ] {
        assert!(!profile_names.contains(pending));
    }
    if std::env::var("WRITE_FLOW_TOOL_MATRIX").as_deref() == Ok("1") {
        let rows=flow.iter().map(|tool|json!({"tool":tool.name,"initial11":initial.contains(tool.name.as_str()),"scope_profile":profile_names.contains(tool.name.as_str()),"baseline_schema":tool.input_schema,"scope_schema":profile.iter().find(|candidate|candidate.name==tool.name).map(|candidate|&candidate.input_schema),"status":if profile_names.contains(tool.name.as_str()){"source_adapted_developer_mocks_not_runtime_acceptance"}else{"unchanged_legacy_only_or_pending_personal_contract"}})).collect::<Vec<_>>();
        let path = std::env::var("FLOW_TOOL_MATRIX_OUTPUT")
            .expect("set FLOW_TOOL_MATRIX_OUTPUT when explicitly generating the tool matrix");
        std::fs::write(path,serde_json::to_string_pretty(&json!({"date":"2026-11-15","evidence":"static actual registry + Sol developer mocks, no runtime acceptance","baseline_total":baseline.len(),"baseline_flow":flow.len(),"initial_profile":11,"initial_baseline_overlap":9,"current_profile":profile.len(),"new_context_or_alias_names":["flow.home","objects.command"],"rows":rows})).expect("serialize static matrix")).expect("write owned matrix");
    }
}

#[tokio::test]
async fn existing_read_options_are_forwarded_or_explicitly_rejected_without_silent_fallback() {
    let (client, state, task) = setup().await.expect("mock fixture");
    let caller = client.acting_as(Secret::new("fixture-user-a"));
    let get = mcp_server::tools::flow_scope::prepare(
        &caller,
        "objects.get",
        &json!({"object_id":OBJECT,"at_seq":3,"render":"markdown"}),
    )
    .await
    .expect("mapped GET");
    assert!(get.path.contains("at_seq=3"));
    assert!(get.path.contains("render=markdown"));
    state.lock().await.resource_type = Some("collection".into());
    let describe = mcp_server::tools::flow_scope::prepare(
        &caller,
        "collections.describe",
        &json!({"collection_id":OBJECT,"at_seq":3}),
    )
    .await
    .expect("mapped current-only schema option");
    assert!(describe.path.ends_with("?at_seq=3"));
    state.lock().await.endpoint_errors.insert(
        format!("/api/v2/flow/collections/{OBJECT}?at_seq=3"),
        json!({"code":400,"error_code":"invalid_update","message":"not current","data":null}),
    );
    let server = McpServer::new(caller, McpToolMode::Full);
    let rejected = server
        .call_tool("collections.describe", json!({"collection_id":OBJECT,"at_seq":3}))
        .await;
    assert_eq!(rejected.is_error, Some(true));
    assert!(text(&rejected).contains("invalid_update"));
    state.lock().await.calls.clear();
    for (name, args) in [
        ("objects.get", json!({"object_id":OBJECT,"at_snapshot":"unsupported"})),
        (
            "collections.describe",
            json!({"collection_id":OBJECT,"historical_schema":true}),
        ),
        (
            "objects.create",
            json!({"personal":true,"type":"collection","title":"invalid","after_block_id":"block-without-page","idempotency_key":"bad"}),
        ),
    ] {
        assert_eq!(server.call_tool(name, args).await.is_error, Some(true));
    }
    assert!(state.lock().await.calls.is_empty());
    task.abort();
}
#[tokio::test]
async fn atomic_embed_derives_actual_page_context_without_home_and_never_treats_block_anchor_as_object() {
    let (client, state, task) = setup().await.expect("mock fixture");
    state.lock().await.resources.insert(
        OBJECT.into(),
        json!({"id":OBJECT,"scope_id":SB,"workspace_id":null,"project_id":null,"object_type":"page"}),
    );
    let server = McpServer::new(client.acting_as(Secret::new("fixture-user-a")), McpToolMode::Full);
    let result = server
        .call_tool(
            "objects.create",
            json!({"type":"collection","title":"embedded","embed_page_id":OBJECT,"idempotency_key":"embed"}),
        )
        .await;
    assert_ne!(result.is_error, Some(true), "{}", text(&result));
    {
        let mock = state.lock().await;
        assert!(
            !mock
                .calls
                .iter()
                .any(|call| call.1.ends_with("/home") || call.1.ends_with("/navigator"))
        );
        let post = mock
            .calls
            .iter()
            .find(|call| call.0 == "POST")
            .expect("typed embed command");
        assert_eq!(post.1, format!("/api/v2/flow/objects/{OBJECT}/commands"));
        assert_eq!(post.3.pointer("/command/type"), Some(&json!("create_collection_embed")));
        assert!(!post.3.to_string().contains("workspace_id"));
        drop(mock);
    }
    state.lock().await.endpoint_errors.insert(format!("/api/v2/flow/objects/{OBJECT}/commands"),json!({"code":400,"error_code":"unsupported_protocol","details":{"reason":"editor_relative_embed_position_unsupported"},"message":"unsupported position","data":null}));
    let anchor = "cid:4@71001:Map";
    let refused=server.call_tool("objects.create",json!({"type":"collection","title":"embedded","embed_page_id":OBJECT,"after_block_id":anchor,"idempotency_key":"anchored-embed"})).await;
    assert_eq!(refused.is_error, Some(true));
    assert!(text(&refused).contains("editor_relative_embed_position_unsupported"));
    assert!(!state.lock().await.calls.iter().any(|call| call.1.contains(anchor)));
    task.abort();
}

#[tokio::test]
async fn embedded_creation_checks_actual_page_project_and_both_existing_mutation_policies() {
    let (client, state, task) = setup().await.expect("mock fixture");
    {
        let mut mock = state.lock().await;
        mock.resources
            .insert(OBJECT.into(), legacy_resource(OBJECT, OTHER_PROJECT, "page"));
        mock.enabled_tools = Some(vec!["objects.create".into()]);
    }
    let server = McpServer::new(client.acting_as(Secret::new("fixture-user-a")), McpToolMode::Full);
    let args = json!({"type":"collection","title":"embedded","embed_page_id":OBJECT,"idempotency_key":"embed-policy"});
    let result = server.call_tool("objects.create", args).await;
    assert_eq!(result.is_error, Some(true));
    assert!(text(&result).contains("objects.patch"));
    let calls = &state.lock().await.calls;
    assert!(
        calls
            .iter()
            .any(|call| call.1.contains(OTHER_PROJECT) && call.1.ends_with("/agent-policy"))
    );
    assert!(!calls.iter().any(|call| call.0 == "POST" || call.1.ends_with("/home")));
    task.abort();
}

#[tokio::test]
async fn personal_move_and_navigation_queries_use_canonical_handlers_without_legacy_fallback() {
    let (client, state, task) = setup().await.expect("mock fixture");
    let server = McpServer::new(client.acting_as(Secret::new("fixture-user-a")), McpToolMode::Full);
    let moved = server
        .call_tool(
            "objects.move",
            json!({"object_id":OBJECT,"target_object_id":SB,"idempotency_key":"personal-move"}),
        )
        .await;
    assert_ne!(moved.is_error, Some(true), "{}", text(&moved));
    assert!(
        state
            .lock()
            .await
            .calls
            .iter()
            .any(|call| call.0 == "POST" && call.1 == format!("/api/v2/flow/objects/{OBJECT}/commands"))
    );
    state.lock().await.calls.clear();
    for name in ["flow.trash.list", "collab.projection_lag"] {
        let response = server
            .call_tool(name, json!({"personal":true,"limit":1,"cursor":"scope-cursor"}))
            .await;
        assert_ne!(response.is_error, Some(true), "{}", text(&response));
        let query = state
            .lock()
            .await
            .queries
            .last()
            .and_then(|(_, query)| query.clone())
            .unwrap();
        assert!(query.contains("limit=1"));
        assert!(query.contains("cursor=scope-cursor"));
        assert!(!query.contains("workspace_id"));
    }
    assert!(
        state
            .lock()
            .await
            .calls
            .iter()
            .any(|call| call.1.starts_with(&format!("/api/v2/flow/scopes/{SA}/")))
    );
    let workspace = server
        .call_tool("collab.projection_lag", json!({"workspace_id":WORKSPACE,"limit":1}))
        .await;
    assert_ne!(workspace.is_error, Some(true), "{}", text(&workspace));
    assert_eq!(
        state
            .lock()
            .await
            .calls
            .last()
            .expect("a workspace projection lag call")
            .1,
        format!("/api/v2/flow/scopes/{SA}/projection-lag")
    );
    task.abort();
}

#[tokio::test]
async fn record_only_schema_and_patch_use_bounded_record_contract_without_parent_queries() {
    let (client, state, task) = setup().await.expect("mock fixture");
    {
        let mut mock = state.lock().await;
        mock.resources.insert(OBJECT.into(),json!({"id":OBJECT,"scope_id":SA,"workspace_id":WORKSPACE,"project_id":PROJECT,"object_type":"record","record_context_supported":true}));
        mock.fields.push(json!({"field_id":SB,"type":"text","label":"visible"}));
        mock.deny_only_project = Some(OTHER_PROJECT.into());
    }
    let server = McpServer::new(client.acting_as(Secret::new("fixture-user-a")), McpToolMode::Full);
    let schema = server
        .call_tool("collections.describe", json!({"record_id":OBJECT}))
        .await;
    assert_ne!(schema.is_error, Some(true), "{}", text(&schema));
    assert!(text(&schema).contains("opaque-record-schema"));
    let patched=server.call_tool("objects.command",json!({"object_id":OBJECT,"command":{"type":"record_patch","payload":{"properties":{SB:"edited"},"schema_revision":"opaque-record-schema"}},"expected_frontier":"record-frontier","idempotency_key":"record-only-patch"})).await;
    assert_ne!(patched.is_error, Some(true), "{}", text(&patched));
    let calls = &state.lock().await.calls;
    assert!(!calls.iter().any(|call| call.1.contains("/collections/")));
    let post = calls
        .iter()
        .find(|call| call.0 == "POST")
        .expect("record-target typed command");
    assert_eq!(post.1, format!("/api/v2/flow/objects/{OBJECT}/commands"));
    assert!(post.3.pointer("/command/payload/record_id").is_none());
    task.abort();
}
#[tokio::test]
async fn record_only_context_readiness_and_opaque_revision_are_not_silently_bypassed() {
    let (client, state, task) = setup().await.expect("mock fixture");
    state.lock().await.resources.insert(
        OBJECT.into(),
        json!({"id":OBJECT,"scope_id":SA,"project_id":null,"object_type":"record","record_context_supported":false}),
    );
    let server = McpServer::new(client.acting_as(Secret::new("fixture-user-a")), McpToolMode::Full);
    assert_eq!(
        server
            .call_tool("collections.describe", json!({"record_id":OBJECT}))
            .await
            .is_error,
        Some(true)
    );
    assert!(
        !state
            .lock()
            .await
            .calls
            .iter()
            .any(|call| call.1.contains("/context") || call.1.contains("/collections/"))
    );
    state.lock().await.resources.insert(
        OBJECT.into(),
        json!({"id":OBJECT,"scope_id":SA,"project_id":null,"object_type":"record","record_context_supported":true}),
    );
    let rejected=server.call_tool("objects.command",json!({"object_id":OBJECT,"command":{"type":"record_patch","payload":{"properties":{}}},"expected_frontier":"record-frontier","idempotency_key":"missing-schema"})).await;
    assert_eq!(rejected.is_error, Some(true));
    assert!(!state.lock().await.calls.iter().any(|call| call.0 == "POST"));
    task.abort();
}
