//! The search mode behaviour of [`crate::server::McpServer`], checked over the whole catalogue
//! against the recording API stand-in: what each mode lists and accepts, that neither wrapper
//! reaches further than a direct call, and that the write switch and the delegated gate hold.

#![allow(clippy::indexing_slicing, clippy::needless_pass_by_value)]

use super::catalog::{self, Access, CATALOG};
use super::entry::{self, ENTRY_TOOL_NAMES};
use super::tasks::{self, TASK_TOOL_NAMES};
use super::test_support::{Api, Policy, Seen, UUID, minimal_arguments};
use crate::oauth::DelegatedGrant;
use crate::protocol::{CallToolResult, JsonRpcRequest, ToolContent, ToolDefinition};
use crate::server::McpServer;
use platform::config::McpToolMode;
use serde_json::{Value, json};
use std::collections::BTreeSet;

type TestResult = Result<(), Box<dyn std::error::Error>>;

fn text(result: &CallToolResult) -> String {
    result
        .content
        .iter()
        .filter_map(|item| match item {
            ToolContent::Text { text } => Some(text.as_str()),
            _ => None,
        })
        .collect()
}

fn server(api: &Api, mode: McpToolMode) -> Result<McpServer, String> {
    Ok(McpServer::new(api.client()?, mode))
}

fn registry() -> Vec<ToolDefinition> {
    crate::tools::get_all_tool_definitions()
}

fn non_task_tools() -> Vec<ToolDefinition> {
    registry()
        .into_iter()
        .filter(|tool| !tasks::is_task_tool(&tool.name))
        .collect()
}

fn wrapper_for(name: &str) -> &'static str {
    match catalog::meta(name).map(|entry| entry.access) {
        Some(Access::Read) => entry::READ,
        _ => entry::WRITE,
    }
}

async fn rpc(server: &McpServer, method: &str, params: Option<Value>) -> Value {
    let request = JsonRpcRequest {
        jsonrpc: "2.0".to_string(),
        id: Some(json!(1)),
        method: method.to_string(),
        params,
    };
    let response = server.handle_request(request).await;
    response
        .and_then(|response| serde_json::to_value(response).ok())
        .unwrap_or(Value::Null)
}

fn listed_names(response: &Value) -> Vec<String> {
    response
        .pointer("/result/tools")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|tool| tool.get("name").and_then(Value::as_str).map(str::to_string))
        .collect()
}

// ---- the listing of each mode ----

#[tokio::test]
async fn tools_list_offers_nine_tools_in_search_mode_and_the_registry_in_full_mode() -> TestResult {
    let api = Api::spawn(Policy::Permissive).await?;
    let search = listed_names(&rpc(&server(&api, McpToolMode::Search)?, "tools/list", None).await);
    assert_eq!(
        search.iter().map(String::as_str).collect::<Vec<_>>(),
        [
            "sylvode_overview",
            "sylvode_search",
            "sylvode_read",
            "sylvode_write",
            "sylvode_task_get",
            "sylvode_task_ack",
            "sylvode_task_progress",
            "sylvode_task_complete",
            "sylvode_task_fail"
        ]
    );
    let full = listed_names(&rpc(&server(&api, McpToolMode::Full)?, "tools/list", None).await);
    assert_eq!(full.len(), 153);
    assert_eq!(
        full.iter().cloned().collect::<BTreeSet<_>>(),
        registry().into_iter().map(|tool| tool.name).collect::<BTreeSet<_>>()
    );
    for name in ENTRY_TOOL_NAMES {
        assert!(!full.contains(&name.to_string()), "{name} is a search mode tool");
    }
    assert!(api.seen().is_empty(), "listing without a project asks the API nothing");
    Ok(())
}

#[tokio::test]
async fn a_project_policy_filters_the_task_tools_but_never_the_entry_tools() -> TestResult {
    let api = Api::spawn(Policy::DenyAll).await?;
    let params = Some(json!({ "project_id": UUID }));
    let search = listed_names(&rpc(&server(&api, McpToolMode::Search)?, "tools/list", params.clone()).await);
    assert_eq!(
        search.iter().map(String::as_str).collect::<Vec<_>>(),
        ENTRY_TOOL_NAMES,
        "the policy enables nothing, so only the entry tools remain"
    );
    assert_eq!(api.seen().len(), 1, "one policy read");
    let full = listed_names(&rpc(&server(&api, McpToolMode::Full)?, "tools/list", params).await);
    assert!(full.is_empty(), "{full:?}");

    let permissive = Api::spawn(Policy::Permissive).await?;
    let all = listed_names(
        &rpc(
            &server(&permissive, McpToolMode::Search)?,
            "tools/list",
            Some(json!({ "project_id": UUID })),
        )
        .await,
    );
    assert_eq!(all.len(), 9);
    Ok(())
}

#[tokio::test]
async fn the_listed_definitions_carry_the_read_only_hint_in_both_modes() -> TestResult {
    let api = Api::spawn(Policy::Permissive).await?;
    for mode in [McpToolMode::Search, McpToolMode::Full] {
        let response = rpc(&server(&api, mode)?, "tools/list", None).await;
        for tool in response
            .pointer("/result/tools")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            assert!(
                tool.pointer("/annotations/readOnlyHint").is_some_and(Value::is_boolean),
                "{mode:?}: {tool}"
            );
        }
    }
    Ok(())
}

#[tokio::test]
async fn initialize_carries_the_instructions_and_the_task_capability_for_a_plain_caller_only() -> TestResult {
    let api = Api::spawn(Policy::Permissive).await?;
    for mode in [McpToolMode::Search, McpToolMode::Full] {
        let response = rpc(&server(&api, mode)?, "initialize", Some(json!({}))).await;
        assert_eq!(
            response.pointer("/result/capabilities/experimental/sylvode~1task-protocol/version"),
            Some(&json!(1)),
            "{mode:?}"
        );
        assert_eq!(
            response.pointer("/result/instructions").and_then(Value::as_str),
            Some(crate::guides::SERVER_INSTRUCTIONS)
        );
        assert!(response.pointer("/result/capabilities/tools").is_some());
    }
    let search = rpc(&server(&api, McpToolMode::Search)?, "initialize", None).await;
    assert!(
        search
            .pointer("/result/instructions")
            .and_then(Value::as_str)
            .is_some_and(|text| text.contains("sylvode_search"))
    );

    let delegated = McpServer::new_delegated(
        api.client()?,
        DelegatedGrant {
            scopes: api::oauth::scopes::ScopeSet::all(),
            workspace_id: uuid::Uuid::from_u128(1),
            projects: None,
        },
    );
    let response = rpc(&delegated, "initialize", Some(json!({}))).await;
    assert!(response.pointer("/result/instructions").is_none(), "{response}");
    assert!(
        response.pointer("/result/capabilities/experimental").is_none(),
        "{response}"
    );

    let flow_only = McpServer::new(api.client()?.flow_only(), McpToolMode::Search);
    let response = rpc(&flow_only, "initialize", Some(json!({}))).await;
    assert!(response.pointer("/result/instructions").is_none(), "{response}");
    assert!(
        response.pointer("/result/capabilities/experimental").is_none(),
        "{response}"
    );
    Ok(())
}

// ---- a direct call of a name the mode does not list ----

#[tokio::test]
async fn search_mode_refuses_the_direct_call_of_every_registry_tool_before_any_request() -> TestResult {
    let api = Api::spawn(Policy::Permissive).await?;
    let server = server(&api, McpToolMode::Search)?;
    for tool in non_task_tools() {
        let result = server.call_tool(&tool.name, minimal_arguments(&tool)).await;
        assert_eq!(result.is_error, Some(true), "{}", tool.name);
        assert_eq!(
            text(&result),
            format!(
                "Error: '{}' is not listed in search mode; call it through sylvode_read or sylvode_write",
                tool.name
            ),
            "{}",
            tool.name
        );
    }
    assert!(
        api.seen().is_empty(),
        "a refused direct call reached the API: {:?}",
        api.seen()
    );
    for parked in [
        "project_resources.list",
        "project_resources.create",
        "project_resources.update",
        "project_resources.delete",
        "code.resources.list",
        "code.directory.get",
        "no.such_tool",
    ] {
        let result = server.call_tool(parked, json!({ "project_id": UUID })).await;
        assert_eq!(
            text(&result),
            format!("Error: unknown tool '{parked}'; use sylvode_search")
        );
    }
    assert!(api.seen().is_empty());
    Ok(())
}

#[tokio::test]
async fn the_task_tools_and_the_entry_tools_are_callable_directly_in_search_mode() -> TestResult {
    let api = Api::spawn(Policy::Permissive).await?;
    let server = server(&api, McpToolMode::Search)?;
    for tool in tasks::tool_definitions().into_iter().chain(entry::definitions()) {
        let args = if entry::is_entry_tool(&tool.name) {
            match tool.name.as_str() {
                entry::SEARCH => json!({ "query": "work items" }),
                entry::READ | entry::WRITE => json!({ "tool": "projects.list", "arguments": {} }),
                _ => json!({}),
            }
        } else {
            minimal_arguments(&tool)
        };
        let result = server.call_tool(&tool.name, args).await;
        assert!(
            !text(&result).contains("is not listed in"),
            "{}: {}",
            tool.name,
            text(&result)
        );
    }
    Ok(())
}

#[tokio::test]
async fn full_mode_accepts_every_registry_name_and_refuses_the_entry_tools() -> TestResult {
    let api = Api::spawn(Policy::Permissive).await?;
    let server = server(&api, McpToolMode::Full)?;
    for name in ENTRY_TOOL_NAMES {
        let result = server.call_tool(name, json!({})).await;
        assert_eq!(text(&result), format!("Error: '{name}' is not listed in full mode"));
    }
    assert!(api.seen().is_empty());
    for tool in registry() {
        let result = server.call_tool(&tool.name, minimal_arguments(&tool)).await;
        assert!(
            !text(&result).contains("is not listed in"),
            "{}: {}",
            tool.name,
            text(&result)
        );
    }
    assert!(!api.seen().is_empty());
    Ok(())
}

// ---- the wrappers ----

#[tokio::test]
async fn each_wrapper_refuses_the_other_access_class_for_the_whole_catalogue_before_any_request() -> TestResult {
    let api = Api::spawn(Policy::Permissive).await?;
    let server = server(&api, McpToolMode::Search)?;
    for tool in non_task_tools() {
        let (wrong, hint) = match wrapper_for(&tool.name) {
            entry::READ => (
                entry::WRITE,
                format!("'{}' only reads data; call it with sylvode_read", tool.name),
            ),
            _ => (
                entry::READ,
                format!("'{}' changes data; call it with sylvode_write", tool.name),
            ),
        };
        let result = server
            .call_tool(
                wrong,
                json!({ "tool": tool.name, "arguments": minimal_arguments(&tool) }),
            )
            .await;
        assert_eq!(result.is_error, Some(true), "{}", tool.name);
        assert_eq!(text(&result), format!("Error: {hint}"));
    }
    assert!(api.seen().is_empty(), "{:?}", api.seen());
    Ok(())
}

/// The strongest statement of "a wrapper cannot reach more than a direct call": for every tool of
/// the catalogue the wrapper call and the direct call make the same API requests and give the same
/// answer, with a policy that restricts nothing and with a policy that enables nothing. The
/// operation log label of every request is the inner tool name.
#[tokio::test]
async fn a_wrapper_call_is_the_direct_call_for_every_tool_under_every_policy() -> TestResult {
    for policy in [Policy::Permissive, Policy::DenyAll] {
        for tool in non_task_tools() {
            let arguments = minimal_arguments(&tool);
            let direct_api = Api::spawn(policy).await?;
            let wrapped_api = Api::spawn(policy).await?;
            let direct = server(&direct_api, McpToolMode::Full)?
                .call_tool(&tool.name, arguments.clone())
                .await;
            let wrapped = server(&wrapped_api, McpToolMode::Search)?
                .call_tool(
                    wrapper_for(&tool.name),
                    json!({ "tool": tool.name, "arguments": arguments }),
                )
                .await;
            assert_eq!(text(&wrapped), text(&direct), "{policy:?} {}", tool.name);
            assert_eq!(wrapped.is_error, direct.is_error, "{policy:?} {}", tool.name);
            let shape = |seen: Vec<Seen>| {
                seen.into_iter()
                    .map(|seen| (seen.method, seen.path, seen.query, seen.body, seen.tool))
                    .collect::<Vec<_>>()
            };
            let wrapped_requests = wrapped_api.seen();
            for request in &wrapped_requests {
                assert_eq!(
                    request.tool.as_deref(),
                    Some(tool.name.as_str()),
                    "{policy:?} {}",
                    tool.name
                );
            }
            assert_eq!(
                shape(wrapped_requests),
                shape(direct_api.seen()),
                "{policy:?} {}",
                tool.name
            );
        }
    }
    Ok(())
}

#[tokio::test]
async fn a_project_policy_that_disables_a_tool_stops_it_through_the_wrapper_too() -> TestResult {
    let api = Api::spawn(Policy::DenyAll).await?;
    let server = server(&api, McpToolMode::Search)?;
    let result = server
        .call_tool(
            entry::WRITE,
            json!({ "tool": "work_items.update", "arguments": { "work_item_id": UUID, "title": "x" } }),
        )
        .await;
    assert_eq!(result.is_error, Some(true));
    assert!(
        text(&result).contains("disabled by project agent policy"),
        "{}",
        text(&result)
    );
    assert!(
        api.seen().iter().all(|request| request.method == "GET"),
        "{:?}",
        api.seen()
    );
    Ok(())
}

#[tokio::test]
async fn ownership_bearing_arguments_inside_a_wrapper_call_reach_the_owner_lookup() -> TestResult {
    let api = Api::spawn(Policy::Permissive).await?;
    let server = server(&api, McpToolMode::Search)?;
    let result = server
        .call_tool(
            entry::READ,
            json!({ "tool": "work_items.get", "arguments": { "work_item_id": UUID } }),
        )
        .await;
    assert_eq!(result.is_error, None, "{}", text(&result));
    let paths = api.seen().into_iter().map(|seen| seen.path).collect::<Vec<_>>();
    assert!(
        paths.iter().any(|path| path == &format!("/api/v1/issues/{UUID}")),
        "{paths:?}"
    );
    assert!(
        paths.iter().any(|path| path.ends_with("/agent-policy")),
        "the owner's policy is read: {paths:?}"
    );
    Ok(())
}

// ---- allow_write_call ----

#[tokio::test]
async fn with_writes_disabled_sylvode_write_answers_for_the_whole_catalogue_and_reads_still_work() -> TestResult {
    let api = Api::spawn(Policy::Permissive).await?;
    let server = server(&api, McpToolMode::Search)?.with_write_call_allowed(false);
    for tool in non_task_tools() {
        let result = server
            .call_tool(
                entry::WRITE,
                json!({ "tool": tool.name, "arguments": minimal_arguments(&tool) }),
            )
            .await;
        assert_eq!(result.is_error, Some(true), "{}", tool.name);
        assert_eq!(
            text(&result),
            "Error: writes are disabled for this bot by the runner configuration"
        );
    }
    assert!(
        api.seen().is_empty(),
        "a disabled write reached the API: {:?}",
        api.seen()
    );

    let read = server
        .call_tool(entry::READ, json!({ "tool": "projects.list", "arguments": {} }))
        .await;
    assert_eq!(read.is_error, None, "{}", text(&read));
    assert!(!api.seen().is_empty());
    let task = server
        .call_tool(
            tasks::TASK_PROGRESS,
            json!({ "task_id": UUID, "attempt": 1, "note": "n" }),
        )
        .await;
    assert!(!text(&task).contains("disabled"), "{}", text(&task));
    // The write tool is still listed: the switch is an answer, not a listing.
    assert!(listed_names(&rpc(&server, "tools/list", None).await).contains(&entry::WRITE.to_string()));
    Ok(())
}

#[tokio::test]
async fn with_writes_allowed_sylvode_write_runs_a_write_tool() -> TestResult {
    let api = Api::spawn(Policy::Permissive).await?;
    let server = server(&api, McpToolMode::Search)?;
    let result = server
        .call_tool(
            entry::WRITE,
            json!({ "tool": "labels.create", "arguments": { "name": "bug", "color": "#ff0000" } }),
        )
        .await;
    assert_eq!(result.is_error, None, "{}", text(&result));
    assert!(
        api.seen()
            .iter()
            .any(|seen| seen.method == "POST" && seen.tool.as_deref() == Some("labels.create"))
    );
    Ok(())
}

// ---- search through the server ----

#[tokio::test]
async fn sylvode_search_with_a_project_reads_its_policy_once_and_filters() -> TestResult {
    let api = Api::spawn(Policy::DenyAll).await?;
    let server = server(&api, McpToolMode::Search)?;
    let result = server
        .call_tool(
            entry::SEARCH,
            json!({ "query": "work_items.update", "project_id": UUID }),
        )
        .await;
    assert_eq!(result.is_error, None, "{}", text(&result));
    let answer: Value = serde_json::from_str(&text(&result))?;
    assert_eq!(answer["returned"], json!(0), "{answer}");
    let seen = api.seen();
    assert_eq!(seen.len(), 1, "{seen:?}");
    assert_eq!(seen[0].method, "GET");
    assert!(seen[0].path.ends_with("/agent-policy"));

    let open = Api::spawn(Policy::Permissive).await?;
    let result = self::server(&open, McpToolMode::Search)?
        .call_tool(entry::SEARCH, json!({ "query": "close issue", "project_id": UUID }))
        .await;
    let answer: Value = serde_json::from_str(&text(&result))?;
    assert_eq!(answer["results"][0]["name"], json!("work_items.update"));
    assert_eq!(open.seen().len(), 1);

    let overview = self::server(&open, McpToolMode::Search)?
        .call_tool(entry::OVERVIEW, json!({ "project_id": UUID }))
        .await;
    let overview: Value = serde_json::from_str(&text(&overview))?;
    assert_eq!(overview["policy_filtered_for_project"], json!(UUID));
    assert_eq!(overview["categories"].as_array().map(Vec::len), Some(19));
    Ok(())
}

#[tokio::test]
async fn sylvode_search_and_overview_make_no_request_without_a_project() -> TestResult {
    let api = Api::spawn(Policy::Permissive).await?;
    let server = server(&api, McpToolMode::Search)?;
    let search = server
        .call_tool(entry::SEARCH, json!({ "category": "flow", "limit": 10 }))
        .await;
    let overview = server.call_tool(entry::OVERVIEW, json!({})).await;
    assert_eq!((search.is_error, overview.is_error), (None, None));
    assert!(api.seen().is_empty());
    for bad in [
        json!({}),
        json!({ "query": "x", "project_id": "not-a-uuid" }),
        json!({ "query": 5 }),
    ] {
        assert_eq!(server.call_tool(entry::SEARCH, bad).await.is_error, Some(true));
    }
    assert_eq!(
        server
            .call_tool(entry::OVERVIEW, json!({ "project_id": "../x" }))
            .await
            .is_error,
        Some(true)
    );
    assert!(api.seen().is_empty());
    Ok(())
}

// ---- delegated callers and Flow-only ----

fn delegated(api: &Api) -> Result<McpServer, String> {
    Ok(McpServer::new_delegated(
        api.client()?,
        DelegatedGrant {
            scopes: api::oauth::scopes::ScopeSet::all(),
            workspace_id: uuid::Uuid::from_u128(1),
            projects: None,
        },
    ))
}

#[tokio::test]
async fn a_delegated_server_ignores_the_mode_and_refuses_every_entry_and_task_tool() -> TestResult {
    let api = Api::spawn(Policy::Permissive).await?;
    let server = delegated(&api)?;
    let listed = listed_names(&rpc(&server, "tools/list", None).await);
    let scoped = crate::oauth_scopes::SCOPED_TOOLS
        .iter()
        .map(|(name, _)| (*name).to_string())
        .collect::<BTreeSet<_>>();
    assert_eq!(listed.iter().cloned().collect::<BTreeSet<_>>(), scoped);
    assert_eq!(listed.len(), 8);
    for name in ENTRY_TOOL_NAMES.iter().copied().chain(TASK_TOOL_NAMES) {
        let result = server
            .call_tool(
                name,
                json!({ "query": "x", "tool": "projects.list", "arguments": {}, "task_id": UUID, "attempt": 1 }),
            )
            .await;
        assert_eq!(
            text(&result),
            "Error: tool not available to connected applications",
            "{name}"
        );
    }
    // A registry tool of its own scope table is called by name, as before.
    let result = server.call_tool("projects.list", json!({})).await;
    assert!(!text(&result).contains("is not listed in"), "{}", text(&result));
    // Through no wrapper: even the eight tools it may call directly.
    assert!(
        text(
            &server
                .call_tool(entry::READ, json!({ "tool": "projects.list", "arguments": {} }))
                .await
        )
        .contains("not available to connected applications")
    );
    // Everything else of the catalogue is refused by the gate whichever way it is named.
    for tool in non_task_tools().into_iter().filter(|tool| !scoped.contains(&tool.name)) {
        let result = server.call_tool(&tool.name, minimal_arguments(&tool)).await;
        assert_eq!(
            text(&result),
            "Error: tool not available to connected applications",
            "{}",
            tool.name
        );
    }
    Ok(())
}

#[tokio::test]
async fn a_flow_only_server_keeps_its_own_listing_whatever_the_mode() -> TestResult {
    let api = Api::spawn(Policy::Permissive).await?;
    let expected = crate::tools::flow_scope::definitions()
        .into_iter()
        .map(|tool| tool.name)
        .collect::<BTreeSet<_>>();
    for mode in [McpToolMode::Search, McpToolMode::Full] {
        let server = McpServer::new(api.client()?.flow_only(), mode);
        let listed = listed_names(&rpc(&server, "tools/list", None).await);
        assert_eq!(listed.iter().cloned().collect::<BTreeSet<_>>(), expected, "{mode:?}");
        for name in ENTRY_TOOL_NAMES {
            assert!(!listed.contains(&name.to_string()));
            let result = server.call_tool(name, json!({ "query": "x" })).await;
            assert_eq!(result.is_error, Some(true), "{name}");
        }
    }
    assert!(api.seen().is_empty());
    Ok(())
}

#[test]
fn the_catalogue_and_the_listings_agree_on_the_task_tools() {
    for name in TASK_TOOL_NAMES {
        assert_eq!(catalog::meta(name).map(|entry| entry.category), Some("tasks"));
    }
    assert_eq!(CATALOG.iter().filter(|entry| entry.category == "tasks").count(), 5);
}
