//! Unit tests of the MCP server: policy scopes, resources, tool listing and dispatch.

use super::{
    FORM_RECORD_RESOURCE_POLICY_TOOLS, FORM_RESOURCE_POLICY_TOOLS, ISSUE_IDENTIFIER_POLICY_TOOL, OWNERSHIP_ARGUMENTS,
    PROJECT_RESOURCE_POLICY_TOOLS, PolicyScope, ResourceSubject, TOOL_POLICY_SCOPES, canonical_uuid,
    claimed_project_ids, declared_project_id, extract_tools_list_project_id, is_tool_enabled_by_policy,
    redact_tool_arguments, resource_policy_subject, summarize_tool_result, tool_policy_scope,
};
use crate::guides;
use crate::protocol::{CallToolResult, JsonRpcRequest, ToolContent};
use axum::{
    Json, Router,
    routing::{delete, get, post},
};
use platform::config::McpToolMode;
use serde_json::{Value, json};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

type TestResult = Result<(), Box<dyn std::error::Error>>;

// Canonical UUIDs: every id the policy gate touches now has to be one.
const PROJECT: &str = "11111111-1111-4111-8111-111111111111";
const FOREIGN_PROJECT: &str = "99999999-9999-4999-8999-999999999999";
const FORM: &str = "22222222-2222-4222-8222-222222222222";
const OTHER_FORM: &str = "2b2b2b2b-2b2b-4b2b-8b2b-2b2b2b2b2b2b";
const RECORD: &str = "33333333-3333-4333-8333-333333333333";
const ATTACHMENT: &str = "44444444-4444-4444-8444-444444444444";
const WORK_ITEM: &str = "55555555-5555-4555-8555-555555555555";
const LABEL: &str = "66666666-6666-4666-8666-666666666666";
const SPRINT: &str = "77777777-7777-4777-8777-777777777777";
const COMMENT: &str = "88888888-8888-4888-8888-888888888888";
const RESOURCE: &str = "aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa";
const PLUGIN: &str = "bbbbbbbb-bbbb-4bbb-8bbb-bbbbbbbbbbbb";

/// ADR-0009 freezes the pre-Flow `search.all` contract: it stays workspace-wide and its
/// result selector exposes only project/issue/comment. Flow search is a distinct surface.
#[test]
fn legacy_search_all_remains_workspace_wide_and_non_flow() {
    assert_eq!(tool_policy_scope("search.all"), PolicyScope::WorkspaceWide);
    let tool = crate::tools::search::search_all_tool();
    assert_eq!(tool.name, "search.all");
    assert_eq!(
        tool.input_schema.pointer("/properties/type/enum"),
        Some(&json!(["issue", "project", "comment"]))
    );
    assert!(!tool.input_schema.to_string().contains("flow"));
}

fn server(base_url: String) -> Result<super::McpServer, String> {
    Ok(super::McpServer::new(
        crate::client::test_api::client(base_url)?,
        McpToolMode::Full,
    ))
}

fn request(id: i64, method: &str, params: Option<Value>) -> JsonRpcRequest {
    JsonRpcRequest {
        jsonrpc: "2.0".to_string(),
        id: Some(json!(id)),
        method: method.to_string(),
        params,
    }
}

fn expand_resource_template(template: &str) -> String {
    template
        .replace("{project_id}", PROJECT)
        .replace("{workspace_id}", PROJECT)
        .replace("{form_id}", FORM)
        .replace("{record_id}", RECORD)
        .replace("{object_id}", RESOURCE)
        .replace("{collection_id}", RESOURCE)
        .replace("{identifier}", "PRX-42")
        .replace("{key}", "software-delivery")
        .replace("{cursor}", "cursor-1")
        .replace("{limit}", "10")
}

#[tokio::test]
async fn initialize_uses_sylvode_display_identity() -> TestResult {
    let response = server("http://127.0.0.1:1".to_string())?
        .handle_request(request(1, "initialize", Some(json!({}))))
        .await
        .and_then(|response| response.result)
        .ok_or("initialize did not return a result")?;
    assert_eq!(response.pointer("/serverInfo/name"), Some(&json!("sylvode-mcp-server")));
    Ok(())
}

/// The v0.9 migration gate is deliberately enumerated from both sides: the expected table
/// names every frozen resource, while the observed table comes from the live registry methods.
/// Equality of the sets and counts prevents a representative sample from masquerading as
/// all-resources coverage. Every observed identity is then read through both schemes.
#[tokio::test]
async fn every_registered_resource_is_served_by_its_sylvode_identity_only() -> TestResult {
    let router = Router::new().fallback(get(|| async {
        Json(json!({
            "code":0,
            "data":{
                "project_id":PROJECT,
                "form_id":FORM,
                "recent_decisions":[],
                "mcp":{"tool_registry":{}}
            }
        }))
    }));
    let server = server(crate::client::test_api::spawn(router).await?)?;
    let resources = server
        .handle_request(request(1, "resources/list", None))
        .await
        .and_then(|response| response.result)
        .and_then(|result| result.get("resources").and_then(Value::as_array).cloned())
        .ok_or("resources/list did not return an array")?;
    let templates = server
        .handle_request(request(2, "resources/templates/list", None))
        .await
        .and_then(|response| response.result)
        .and_then(|result| result.get("resourceTemplates").and_then(Value::as_array).cloned())
        .ok_or("resources/templates/list did not return an array")?;

    let expected = [
        "sylvode://skills/sylvode-mcp",
        "sylvode://skills/sylvode-mcp/tools",
        "sylvode://guides/workflows",
        "sylvode://scenario-templates",
        "sylvode://projects/{project_id}/issues",
        "sylvode://projects/{project_id}/forms",
        "sylvode://forms/{form_id}",
        "sylvode://forms/{form_id}/records",
        "sylvode://forms/{form_id}/events",
        "sylvode://form-records/{record_id}",
        "sylvode://form-records/{record_id}/events",
        "sylvode://scenario-templates/{key}",
        "sylvode://projects/{project_id}/context",
        "sylvode://projects/{project_id}/governance",
        "sylvode://projects/{project_id}/agent-policy",
        "sylvode://projects/{project_id}/release-readiness",
        "sylvode://projects/{project_id}/type",
        "sylvode://projects/{project_id}/recent-decisions",
        "sylvode://projects/{project_id}/sprints",
        "sylvode://issues/{identifier}",
        "sylvode://objects/{object_id}",
        "sylvode://objects/{object_id}/history?limit={limit}",
        "sylvode://objects/{object_id}/schema",
        "sylvode://collections/{collection_id}/records?cursor={cursor}&limit={limit}",
        "sylvode://workspaces/{workspace_id}/navigator?project_id={project_id}",
    ];
    let observed = resources
        .iter()
        .filter_map(|row| row.get("uri").and_then(Value::as_str))
        .chain(
            templates
                .iter()
                .filter_map(|row| row.get("uriTemplate").and_then(Value::as_str)),
        )
        .collect::<Vec<_>>();
    assert_eq!(resources.len(), 4, "static resource registry cardinality drifted");
    assert_eq!(templates.len(), 21, "template registry cardinality drifted");
    assert_eq!(
        observed.len(),
        expected.len(),
        "enumeration did not cover the live registry"
    );
    assert_eq!(observed, expected, "resource registry identities or order drifted");

    // Every resource answers by its own `sylvode://` identity and carries no migration metadata.
    for (index, registered) in observed.into_iter().enumerate() {
        let index = i64::try_from(index)?;
        let canonical = expand_resource_template(registered);
        let canonical_response = server
            .handle_request(request(100 + index, "resources/read", Some(json!({"uri":canonical}))))
            .await
            .ok_or("canonical resources/read returned no response")?;
        assert!(
            canonical_response.error.is_none(),
            "canonical read failed: {canonical_response:?}"
        );
        let result = canonical_response.result.ok_or("resource read had no result")?;
        assert!(
            result.pointer("/contents/0/_meta").is_none(),
            "{registered} carries migration metadata"
        );
    }
    Ok(())
}

fn policy_route(enabled_tools: Value) -> axum::routing::MethodRouter {
    get(move || {
        let enabled_tools = enabled_tools.clone();
        async move {
            Json(json!({
                "code": 0,
                "data": { "mcp": { "tool_registry": { "enabled_tools": enabled_tools } } }
            }))
        }
    })
}

fn message(result: &CallToolResult) -> String {
    summarize_tool_result(result).unwrap_or_default()
}

#[tokio::test]
async fn every_v08_registered_tool_has_a_real_dispatch_arm() -> TestResult {
    let server = server("http://127.0.0.1:1".to_string())?;
    let id = "11111111-1111-4111-8111-111111111111";
    let calls = [
        (
            "objects.export",
            json!({"object_id":id,"format":"package","idempotency_key":"k"}),
        ),
        (
            "objects.export_workspace",
            json!({"workspace_id":id,"idempotency_key":"k"}),
        ),
        (
            "objects.import_artifact",
            json!({"workspace_id":id,"package_base64":"eA==","idempotency_key":"k"}),
        ),
        (
            "objects.import_preview",
            json!({"workspace_id":id,"artifact_id":id,"external_reference_policy":"drop","conflict_policy":"new_ids","idempotency_key":"k"}),
        ),
        (
            "objects.import_commit",
            json!({"workspace_id":id,"import_id":id,"package_sha256":"h","mapping_hash":"m","conflict_policy":"new_ids","confirm":true,"idempotency_key":"k"}),
        ),
        ("objects.import_status", json!({"workspace_id":id,"import_id":id})),
        ("collab.status", json!({"workspace_id":id})),
        ("objects.integrity", json!({"workspace_id":id,"deep":false})),
        (
            "collab.compact",
            json!({"object_id":id,"document_id":id,"dry_run":true,"expected_head_seq":0,"idempotency_key":"k"}),
        ),
        (
            "deliveries.replay",
            json!({"workspace_id":id,"mode":"rebuild","from":"2026-01-01T00:00:00Z","to":"2026-01-02T00:00:00Z","dry_run":true,"confirm":true,"idempotency_key":"k"}),
        ),
        (
            "collab.rebuild_projection",
            json!({"object_id":id,"dry_run":true,"expected_head_seq":0,"idempotency_key":"k"}),
        ),
        (
            "collab.repair_quarantine",
            json!({"workspace_id":id,"dry_run":true,"idempotency_key":"k"}),
        ),
    ];
    for (name, args) in calls {
        let result = server.execute_tool(name, args).await;
        assert!(
            !message(&result).contains("Unknown tool"),
            "{name} is registered but undispatchable"
        );
    }
    Ok(())
}

#[test]
fn tools_list_project_id_accepts_top_level_and_context_params() -> TestResult {
    assert_eq!(
        extract_tools_list_project_id(Some(&json!({ "project_id": PROJECT })))?.as_deref(),
        Some(PROJECT)
    );
    assert_eq!(
        extract_tools_list_project_id(Some(&json!({ "context": { "projectId": PROJECT } })))?.as_deref(),
        Some(PROJECT)
    );
    assert_eq!(
        extract_tools_list_project_id(Some(&json!({ "project_id": " " })))?,
        None
    );
    assert!(extract_tools_list_project_id(Some(&json!({ "project_id": "p1" }))).is_err());
    Ok(())
}

#[test]
fn a_connection_chooses_its_tool_mode_by_query_or_header_and_a_conflict_is_refused() {
    use axum::http::{HeaderMap, HeaderValue};
    let headers = |value: Option<&str>| {
        let mut map = HeaderMap::new();
        if let Some(value) = value {
            map.insert(
                super::TOOL_MODE_HEADER,
                HeaderValue::from_str(value).expect("header value"),
            );
        }
        map
    };
    // The raw query string of the request, as the transports pass it on.
    let choose = |query: Option<&str>, header: Option<&str>, default| {
        let raw = query.map(|value| format!("tools={value}"));
        super::requested_tool_mode(raw.as_deref(), &headers(header), default)
    };
    for default in [McpToolMode::Search, McpToolMode::Full] {
        assert_eq!(
            choose(None, None, default),
            Ok(default),
            "no choice keeps the server's own"
        );
        assert_eq!(choose(Some("full"), None, default), Ok(McpToolMode::Full));
        assert_eq!(choose(Some("search"), None, default), Ok(McpToolMode::Search));
        assert_eq!(choose(None, Some("full"), default), Ok(McpToolMode::Full));
        assert_eq!(choose(None, Some(" Search "), default), Ok(McpToolMode::Search));
        assert_eq!(choose(Some("full"), Some("FULL"), default), Ok(McpToolMode::Full));
        for (query, header) in [(Some("full"), Some("search")), (Some("search"), Some("full"))] {
            assert!(choose(query, header, default).is_err_and(|error| error.contains("disagree")));
        }
        for bad in ["", "all", "fulll", "search,full"] {
            assert!(choose(Some(bad), None, default).is_err(), "{bad:?}");
            assert!(choose(None, Some(bad), default).is_err(), "{bad:?}");
        }
    }
    // Repeated parameters and header lines: a conflict is refused, an identical repeat is not, and
    // the other parameters of the query are ignored.
    for default in [McpToolMode::Search, McpToolMode::Full] {
        let raw = |query: &str| super::requested_tool_mode(Some(query), &HeaderMap::new(), default);
        assert_eq!(raw("tools=full&tools=full"), Ok(McpToolMode::Full));
        assert_eq!(raw("a=1&tools=search&b=2&tools=search"), Ok(McpToolMode::Search));
        assert_eq!(raw("a=1&b=2"), Ok(default));
        assert_eq!(raw(""), Ok(default));
        assert_eq!(
            raw("tools=%66ull"),
            Ok(McpToolMode::Full),
            "percent encoding is decoded"
        );
        for conflict in [
            "tools=search&tools=full",
            "tools=full&tools=search&tools=full",
            "tools=full&tools=",
        ] {
            assert!(
                raw(conflict).is_err_and(|error| error.contains("disagree") || error.contains("search or full")),
                "{conflict}"
            );
        }
        let mut lines = HeaderMap::new();
        lines.append(super::TOOL_MODE_HEADER, HeaderValue::from_static("search"));
        lines.append(super::TOOL_MODE_HEADER, HeaderValue::from_static("full"));
        assert!(super::requested_tool_mode(None, &lines, default).is_err_and(|error| error.contains("disagree")));
        let mut same = HeaderMap::new();
        same.append(super::TOOL_MODE_HEADER, HeaderValue::from_static("full"));
        same.append(super::TOOL_MODE_HEADER, HeaderValue::from_static("FULL"));
        assert_eq!(super::requested_tool_mode(None, &same, default), Ok(McpToolMode::Full));
        let mut mixed = HeaderMap::new();
        mixed.append(super::TOOL_MODE_HEADER, HeaderValue::from_static("full"));
        mixed.append(super::TOOL_MODE_HEADER, HeaderValue::from_static("everything"));
        assert!(super::requested_tool_mode(None, &mixed, default).is_err());
    }
    let mut binary = HeaderMap::new();
    binary.insert(
        super::TOOL_MODE_HEADER,
        HeaderValue::from_bytes(&[0xff, 0xfe]).expect("opaque bytes"),
    );
    assert!(super::requested_tool_mode(None, &binary, McpToolMode::Search).is_err());
}

/// The five static resources are the embedded files, byte for byte, and the old skill URI is gone.
#[tokio::test]
async fn the_static_resources_are_the_embedded_generated_files() -> TestResult {
    let server = server("http://127.0.0.1:1".to_string())?;
    for (uri, expected) in [
        ("sylvode://skills/sylvode-mcp", guides::SKILL_MD),
        ("sylvode://skills/sylvode-mcp/tools", guides::TOOLS_MD),
        ("sylvode://guides/workflows", guides::RECIPES_MD),
    ] {
        let response = server
            .handle_resources_read(Some(json!(1)), Some(json!({ "uri": uri })))
            .await;
        let value = serde_json::to_value(&response)?;
        assert_eq!(
            value.pointer("/result/contents/0/text").and_then(Value::as_str),
            Some(expected),
            "{uri}"
        );
        assert_eq!(
            value.pointer("/result/contents/0/mimeType"),
            Some(&json!("text/markdown"))
        );
    }
    for gone in ["sylvode://skills/sylvode-mcp/missing"] {
        let response = server
            .handle_resources_read(Some(json!(1)), Some(json!({ "uri": gone })))
            .await;
        let body = serde_json::to_string(&response)?;
        assert!(body.contains("Unknown resource URI"), "{gone}: {body}");
    }
    Ok(())
}

/// The listing names exactly the readable static guides, and the skill carries its generated
/// overview of the live catalogue.
#[tokio::test]
async fn the_resource_list_and_the_embedded_skill_match_the_catalogue() -> TestResult {
    let server = server("http://127.0.0.1:1".to_string())?;
    let response = server.handle_request(request(1, "resources/list", None)).await;
    let value = serde_json::to_value(&response)?;
    let uris = value
        .pointer("/result/resources")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|resource| resource.get("uri").and_then(Value::as_str))
        .collect::<Vec<_>>();
    assert_eq!(
        uris,
        [
            "sylvode://skills/sylvode-mcp",
            "sylvode://skills/sylvode-mcp/tools",
            "sylvode://guides/workflows",
            "sylvode://scenario-templates",
        ]
    );
    let count = crate::tools::get_all_tool_definitions().len();
    assert!(
        guides::SKILL_MD.contains(&format!("Sylvode has {count} tools in")),
        "the embedded skill is out of date: run sylvode-skillgen --write"
    );
    assert!(guides::TOOLS_MD.contains(&format!("{count} tools in")));
    for name in ["projects.list", "work_items.update", "sylvode_task_get"] {
        assert!(guides::TOOLS_MD.contains(&format!("`{name}`")), "{name}");
    }
    Ok(())
}

/// A server without the flow-only surface gives instructions; flow-only gives none and no resources.
#[tokio::test]
async fn initialize_serves_the_generated_instructions_except_flow_only() -> TestResult {
    let plain = server("http://127.0.0.1:1".to_string())?;
    let response = serde_json::to_value(plain.handle_request(request(1, "initialize", None)).await)?;
    assert_eq!(
        response.pointer("/result/instructions").and_then(Value::as_str),
        Some(guides::SERVER_INSTRUCTIONS)
    );
    let flow_only = super::McpServer::new(
        crate::client::test_api::client("http://127.0.0.1:1".to_string())?.flow_only(),
        McpToolMode::Full,
    );
    let response = serde_json::to_value(flow_only.handle_request(request(1, "initialize", None)).await)?;
    assert!(response.pointer("/result/instructions").is_none());
    let resources = serde_json::to_value(flow_only.handle_request(request(2, "resources/list", None)).await)?;
    assert_eq!(resources.pointer("/result/resources"), Some(&json!([])));
    Ok(())
}

#[test]
fn declared_project_id_only_trusts_the_top_level_argument() -> TestResult {
    assert_eq!(
        declared_project_id(&json!({ "project_id": PROJECT }))?.as_deref(),
        Some(PROJECT)
    );
    assert_eq!(
        declared_project_id(&json!({ "projectId": PROJECT }))?.as_deref(),
        Some(PROJECT)
    );
    assert_eq!(declared_project_id(&json!({ "work_item_id": WORK_ITEM }))?, None);
    // A nested payload is caller data, never a routing decision.
    assert_eq!(
        declared_project_id(&json!({ "payload": { "project_id": PROJECT } }))?,
        None
    );
    Ok(())
}

/// Every shape the audit called out has to be refused, not silently normalised into
/// "no project declared", because that would drop the call back to "no policy".
#[test]
fn declared_project_id_rejects_every_smuggling_shape() {
    for hostile in [
        json!({ "project_id": format!("{PROJECT}/work-items?pad=") }),
        json!({ "project_id": format!("{PROJECT}?") }),
        json!({ "project_id": format!("{PROJECT}#") }),
        json!({ "project_id": format!("../../{PROJECT}") }),
        json!({ "project_id": format!("{PROJECT}%2F..%2Fx") }),
        json!({ "project_id": format!("{PROJECT}\r\nX-Injected: 1") }),
        json!({ "project_id": "" }),
        json!({ "project_id": "   " }),
        json!({ "project_id": "not-a-uuid" }),
        json!({ "project_id": "1111111111114111811111111111111" }),
        json!({ "project_id": format!("{PROJECT}{PROJECT}") }),
        json!({ "project_id": 42 }),
        json!({ "project_id": ["11111111-1111-4111-8111-111111111111"] }),
        json!({ "project_id": "gggggggg-1111-4111-8111-111111111111" }),
    ] {
        assert!(
            declared_project_id(&hostile).is_err(),
            "{hostile} was accepted as a project id"
        );
    }
}

#[test]
fn canonical_uuid_normalises_case_and_surrounding_space() {
    assert_eq!(
        canonical_uuid("  11111111-1111-4111-8111-11111111111A  ").as_deref(),
        Some("11111111-1111-4111-8111-11111111111a")
    );
    assert_eq!(canonical_uuid("11111111_1111_4111_8111_111111111111"), None);
}

#[test]
fn claimed_project_ids_sees_every_hiding_place() {
    let claims = claimed_project_ids(&json!({
        "project_id": "a",
        "projectId": "b",
        "payload": { "project_id": "c", "projectId": "d" }
    }));
    assert_eq!(claims, vec!["a", "b", "c", "d"]);
    assert!(claimed_project_ids(&json!({ "record_id": RECORD })).is_empty());
}

#[test]
fn project_tool_policy_denies_disabled_tool_names() {
    let policy = json!({
        "code": 0,
        "data": {
            "mcp": {
                "tool_registry": {
                    "enabled_tools": ["context.get_project", "work_items.list"]
                }
            }
        }
    });

    assert!(is_tool_enabled_by_policy(&policy, "context.get_project"));
    assert!(!is_tool_enabled_by_policy(&policy, "sprints.create"));
}

#[test]
fn tool_call_audit_redacts_sensitive_arguments() {
    let redacted = redact_tool_arguments(&json!({
        "content": "ok",
        "nested": { "bot_token": "secret" },
        "items": [{ "password": "pw" }]
    }));

    assert_eq!(redacted.get("content").and_then(Value::as_str), Some("ok"));
    assert_eq!(
        redacted
            .get("nested")
            .and_then(|nested| nested.get("bot_token"))
            .and_then(Value::as_str),
        Some("[REDACTED]")
    );
    assert_eq!(
        redacted
            .get("items")
            .and_then(Value::as_array)
            .and_then(|items| items.first())
            .and_then(|item| item.get("password"))
            .and_then(Value::as_str),
        Some("[REDACTED]")
    );
}

/// Tools whose schema carries no `project_id` or ownership argument but which are
/// nonetheless a Flow *administrative* operation (`WorkspaceWideAdmin`, not plain
/// `WorkspaceWide`). Nothing in a schema shape distinguishes "workspace wide" from
/// "workspace wide and admin only" — both address a workspace, never a project or a
/// project-owned resource — so unlike every other branch of the derivation below this
/// one is necessarily a name list, exactly like the `destructive` list further down.
const WORKSPACE_ADMIN_TOOLS: [&str; 12] = [
    "flow.feature_set",
    "legacy_pages.inventory",
    "legacy_pages.import_preview",
    "legacy_pages.import_commit",
    "legacy_pages.import_status",
    "objects.export_workspace",
    "objects.import_artifact",
    "objects.import_preview",
    "objects.import_commit",
    "objects.import_status",
    "collab.status",
    "deliveries.replay",
];

/// The coverage gate. It re-derives the scope of every tool from the *live* registry
/// (`tools::get_all_tool_definitions`), so it sees exactly the tools that are served
/// and cannot be fooled by naming: a name based filter never saw `events.tail`, which
/// is how that tool stayed unguarded while its guard test was green.
#[test]
fn tool_policy_scopes_match_the_registered_tool_schemas() {
    let tools = crate::tools::get_all_tool_definitions();
    assert_eq!(
        tools.len(),
        TOOL_POLICY_SCOPES.len(),
        "every registered tool needs an explicit policy scope"
    );

    for tool in &tools {
        let properties = tool
            .input_schema
            .get("properties")
            .and_then(Value::as_object)
            .cloned()
            .unwrap_or_default();

        let expected = if matches!(tool.name.as_str(), "objects.integrity" | "collab.repair_quarantine") {
            PolicyScope::WorkspaceOrFlowObject
        } else if WORKSPACE_ADMIN_TOOLS.contains(&tool.name.as_str()) {
            PolicyScope::WorkspaceWideAdmin
        } else if properties.contains_key("project_id") {
            // The required bit is the schema's actual contract. A required project id must
            // always enter project policy; an optional one may resolve to no project and then
            // use the tool's documented workspace/projectless semantics. There is no separate
            // schema-marker gate: `objects.create` and `collab.projection_lag` legitimately
            // omit `unprojected`, while `objects.query`/`objects.search` expose it because they
            // need an explicit query-filter choice.
            let required = tool
                .input_schema
                .get("required")
                .and_then(Value::as_array)
                .is_some_and(|required| required.iter().any(|value| value.as_str() == Some("project_id")));
            PolicyScope::DeclaredProject { required }
        } else if let Some((_, lookup)) = OWNERSHIP_ARGUMENTS
            .iter()
            .find(|(argument, _)| properties.contains_key(*argument))
        {
            PolicyScope::OwnedBy(*lookup)
        } else {
            PolicyScope::WorkspaceWide
        };

        assert_eq!(
            tool_policy_scope(&tool.name),
            expected,
            "{} is classified against its own schema",
            tool.name
        );
    }

    let registered = tools
        .iter()
        .map(|tool| tool.name.as_str())
        .collect::<std::collections::HashSet<_>>();
    for (name, _) in TOOL_POLICY_SCOPES {
        assert!(
            registered.contains(name),
            "TOOL_POLICY_SCOPES lists an unregistered tool: {name}"
        );
    }

    // Tools nobody can address without naming a project must never be workspace wide.
    for destructive in [
        "projects.delete",
        "work_items.delete",
        "comments.delete",
        "sprints.delete",
        "form_records.update",
        "plugins.invoke",
        "proposals.create_from_result",
        "events.tail",
    ] {
        assert_ne!(
            tool_policy_scope(destructive),
            PolicyScope::WorkspaceWide,
            "{destructive} must not run outside the project agent policy"
        );
    }

    // Unknown names fail closed before dispatch rejects them.
    assert_eq!(
        tool_policy_scope("acme.deploy"),
        PolicyScope::DeclaredProject { required: true }
    );
}

/// `PolicyScope::DeclaredProject { required: false }` is the declared no-project branch:
/// `resolve_policy_project_id` answers `Ok(None)` for it and `enforce_project_tool_policy`
/// then returns `Ok(())` without reading any policy, while the tool itself still runs
/// against a workspace-addressed endpoint. The v0.5 contract deliberately uses this for
/// several Flow tools: omission means workspace/projectless scope, while an actual project id
/// is checked normally. `unprojected` is a query filter on the tools that publish it, not a
/// universal authorization marker.
///
/// The invariant is checked against the *live registry*, not against a name list, so a
/// tool added later with a mismatched required bit fails here instead of silently changing
/// policy semantics. Two things matter: a `required: true` tool's schema must require
/// `project_id`; a `required: false` tool must publish `project_id` but must not require it.
#[test]
fn declared_project_id_is_mandatory_for_every_project_scoped_tool() {
    let mut optional_scope_without_project_id = Vec::new();
    let mut optional_schema = Vec::new();

    for tool in &crate::tools::get_all_tool_definitions() {
        let properties = tool
            .input_schema
            .get("properties")
            .and_then(Value::as_object)
            .cloned()
            .unwrap_or_default();
        let declares_project_id = properties.contains_key("project_id");
        // The frozen v0.8 MCP contract explicitly gives this workspace-admin export an
        // optional project subset. The route still authorizes the workspace admin and then
        // reauthorizes every selected object; this is not a DeclaredProject policy target.
        if tool.name == "objects.export_workspace" {
            assert_eq!(tool_policy_scope(&tool.name), PolicyScope::WorkspaceWideAdmin);
            assert!(
                declares_project_id,
                "the contracted optional project subset disappeared"
            );
            continue;
        }
        if tool_policy_scope(&tool.name) == (PolicyScope::DeclaredProject { required: false }) {
            if !declares_project_id {
                optional_scope_without_project_id.push(tool.name.clone());
            }
            continue;
        }

        if !declares_project_id {
            continue;
        }
        let schema_requires_project_id = tool
            .input_schema
            .get("required")
            .and_then(Value::as_array)
            .is_some_and(|required| required.iter().any(|value| value.as_str() == Some("project_id")));
        if !schema_requires_project_id {
            optional_schema.push(tool.name.clone());
        }
    }

    assert!(
        optional_scope_without_project_id.is_empty(),
        "these tools declare optional project policy without publishing project_id in their schema: {optional_scope_without_project_id:?}"
    );
    assert!(
        optional_schema.is_empty(),
        "these tools accept a project_id their schema does not require, so callers are invited to omit it: {optional_schema:?}"
    );
}

/// The other half of the same class: a tool must not be `WorkspaceWide` while its
/// schema offers a `project_id`, because `WorkspaceWide` skips the policy outright and
/// `OWNERSHIP_ARGUMENTS` does not list `project_id`, so nothing would refuse the call.
#[test]
fn workspace_wide_tools_do_not_accept_a_project_id() {
    let offenders = crate::tools::get_all_tool_definitions()
        .iter()
        .filter(|tool| tool_policy_scope(&tool.name) == PolicyScope::WorkspaceWide)
        .filter(|tool| {
            tool.input_schema
                .get("properties")
                .and_then(Value::as_object)
                .is_some_and(|properties| properties.contains_key("project_id"))
        })
        .map(|tool| tool.name.clone())
        .collect::<Vec<_>>();

    assert!(
        offenders.is_empty(),
        "workspace scoped tools that take a project_id are ungoverned project tools: {offenders:?}"
    );
}

#[test]
fn form_scope_target_prefers_the_record_over_the_form() -> TestResult {
    assert_eq!(
        super::form_scope_target(&json!({ "form_id": FORM, "record_id": RECORD }))?,
        Some((
            super::FormScopeTarget::Record(RECORD.to_string()),
            Some(FORM.to_string())
        ))
    );
    assert_eq!(
        super::form_scope_target(&json!({ "form_id": FORM }))?,
        Some((super::FormScopeTarget::Form(FORM.to_string()), None))
    );
    assert_eq!(
        super::form_scope_target(&json!({ "attachment_id": ATTACHMENT }))?,
        Some((super::FormScopeTarget::Attachment(ATTACHMENT.to_string()), None))
    );
    assert_eq!(super::form_scope_target(&json!({ "project_id": PROJECT }))?, None);
    assert!(super::form_scope_target(&json!({ "form_id": "../x" })).is_err());
    Ok(())
}

#[test]
fn string_argument_ignores_blank_and_non_string_values() {
    assert_eq!(
        super::string_argument(&json!({ "form_id": " f1 " }), "form_id").as_deref(),
        Some("f1")
    );
    assert_eq!(super::string_argument(&json!({ "form_id": "  " }), "form_id"), None);
    assert_eq!(super::string_argument(&json!({ "form_id": 7 }), "form_id"), None);
}

#[test]
fn error_messages_do_not_echo_unbounded_caller_input() {
    let hostile = format!("{}\n\nINJECTED", "x".repeat(200));
    let sanitized = super::sanitize_for_error(&hostile);
    assert!(!sanitized.contains('\n'));
    assert!(sanitized.len() <= 67, "{sanitized}");
}

#[tokio::test]
async fn form_scoped_calls_are_denied_by_project_policy() -> TestResult {
    let form_lookups = Arc::new(AtomicUsize::new(0));
    let counter = Arc::clone(&form_lookups);
    let router = Router::new()
        .route(
            &format!("/api/v1/forms/{FORM}"),
            get(move || {
                let counter = Arc::clone(&counter);
                async move {
                    counter.fetch_add(1, Ordering::SeqCst);
                    Json(json!({ "code": 0, "data": { "id": FORM, "project_id": PROJECT } }))
                }
            }),
        )
        .route(
            &format!("/api/v1/form-records/{RECORD}"),
            get(|| async { Json(json!({ "code": 0, "data": { "id": RECORD, "project_id": PROJECT } })) }),
        )
        .route(
            &format!("/api/v1/projects/{PROJECT}/agent-policy"),
            policy_route(json!(["forms.get"])),
        );
    let server = server(crate::client::test_api::spawn(router).await?)?;

    let by_form = server.call_tool("form_records.list", json!({ "form_id": FORM })).await;
    let by_record = server
        .call_tool("form_records.update", json!({ "record_id": RECORD, "values": {} }))
        .await;

    assert_eq!(by_form.is_error, Some(true));
    assert_eq!(by_record.is_error, Some(true));
    assert!(message(&by_form).contains("disabled by project agent policy"));

    // Second call for the same form must be served from the cache.
    let repeat = server.call_tool("form_records.list", json!({ "form_id": FORM })).await;
    assert_eq!(repeat.is_error, Some(true));
    assert_eq!(form_lookups.load(Ordering::SeqCst), 1);
    Ok(())
}

#[tokio::test]
async fn form_scoped_calls_pass_when_policy_enables_the_tool() -> TestResult {
    let router = Router::new()
        .route(
            &format!("/api/v1/forms/{FORM}"),
            get(|| async { Json(json!({ "code": 0, "data": { "id": FORM, "project_id": PROJECT } })) }),
        )
        .route(
            &format!("/api/v1/forms/{FORM}/records"),
            get(|| async {
                Json(json!({
                    "code": 0,
                    "data": { "items": [], "total": 0, "page": 1, "per_page": 50, "total_pages": 0 }
                }))
            }),
        )
        .route(
            &format!("/api/v1/projects/{PROJECT}/agent-policy"),
            policy_route(json!(["form_records.list"])),
        );
    let server = server(crate::client::test_api::spawn(router).await?)?;

    let result = server.call_tool("form_records.list", json!({ "form_id": FORM })).await;

    assert_eq!(result.is_error, None, "{}", message(&result));
    Ok(())
}

#[tokio::test]
async fn form_scoped_call_rejects_a_smuggled_foreign_project_id() -> TestResult {
    let updates = Arc::new(AtomicUsize::new(0));
    let foreign_policy_reads = Arc::new(AtomicUsize::new(0));
    let update_counter = Arc::clone(&updates);
    let foreign_counter = Arc::clone(&foreign_policy_reads);

    let router = Router::new()
        .route(
            &format!("/api/v1/form-records/{RECORD}"),
            get(|| async { Json(json!({ "code": 0, "data": { "id": RECORD, "project_id": PROJECT } })) }).patch(
                move || {
                    let counter = Arc::clone(&update_counter);
                    async move {
                        counter.fetch_add(1, Ordering::SeqCst);
                        Json(json!({ "code": 0, "data": { "id": RECORD } }))
                    }
                },
            ),
        )
        .route(
            &format!("/api/v1/projects/{PROJECT}/agent-policy"),
            policy_route(json!(["forms.get"])),
        )
        .route(
            &format!("/api/v1/projects/{FOREIGN_PROJECT}/agent-policy"),
            get(move || {
                let counter = Arc::clone(&foreign_counter);
                async move {
                    counter.fetch_add(1, Ordering::SeqCst);
                    Json(json!({
                        "code": 0,
                        "data": { "mcp": { "tool_registry": { "enabled_tools": ["form_records.update"] } } }
                    }))
                }
            }),
        );
    let server = server(crate::client::test_api::spawn(router).await?)?;

    let smuggled = server
        .call_tool(
            "form_records.update",
            json!({ "record_id": RECORD, "values": {}, "project_id": FOREIGN_PROJECT }),
        )
        .await;

    assert_eq!(smuggled.is_error, Some(true));
    let text = message(&smuggled);
    assert!(text.contains(&format!("belongs to project '{PROJECT}'")), "{text}");
    assert!(text.contains("foreign project"), "{text}");

    // The same trick through the nested payload object must fail as well.
    let nested = server
        .call_tool(
            "form_records.update",
            json!({ "record_id": RECORD, "values": {}, "payload": { "project_id": FOREIGN_PROJECT } }),
        )
        .await;
    assert_eq!(nested.is_error, Some(true));

    // Without the smuggled id the owning project decides, and it says no.
    let honest = server
        .call_tool("form_records.update", json!({ "record_id": RECORD, "values": {} }))
        .await;
    assert_eq!(honest.is_error, Some(true));
    assert!(message(&honest).contains("disabled by project agent policy"));

    assert_eq!(updates.load(Ordering::SeqCst), 0, "the record must never be written");
    assert_eq!(
        foreign_policy_reads.load(Ordering::SeqCst),
        0,
        "the policy of a project that does not own the record must never be consulted"
    );
    Ok(())
}

#[tokio::test]
async fn form_scoped_call_accepts_a_project_id_that_matches_the_owner() -> TestResult {
    let updates = Arc::new(AtomicUsize::new(0));
    let update_counter = Arc::clone(&updates);
    let router = Router::new()
        .route(
            &format!("/api/v1/form-records/{RECORD}"),
            get(|| async {
                Json(json!({ "code": 0, "data": { "id": RECORD, "form_id": FORM, "project_id": PROJECT } }))
            })
            .patch(move || {
                let counter = Arc::clone(&update_counter);
                async move {
                    counter.fetch_add(1, Ordering::SeqCst);
                    Json(json!({ "code": 0, "data": { "id": RECORD } }))
                }
            }),
        )
        .route(
            &format!("/api/v1/projects/{PROJECT}/agent-policy"),
            policy_route(json!(["form_records.update"])),
        );
    let server = server(crate::client::test_api::spawn(router).await?)?;

    let result = server
        .call_tool(
            "form_records.update",
            json!({ "record_id": RECORD, "values": {}, "project_id": PROJECT }),
        )
        .await;

    assert_eq!(result.is_error, None, "{}", message(&result));
    assert_eq!(updates.load(Ordering::SeqCst), 1);
    Ok(())
}

/// A call that names both a record and a form is authorized through the record, and
/// the wider `form_id` claim is checked against the record's real owner instead of
/// being trusted.
#[tokio::test]
async fn a_record_call_refuses_a_form_id_that_does_not_own_it() -> TestResult {
    let listings = Arc::new(AtomicUsize::new(0));
    let counter = Arc::clone(&listings);
    let router = Router::new()
        .route(
            &format!("/api/v1/form-records/{RECORD}"),
            get(|| async {
                Json(json!({ "code": 0, "data": { "id": RECORD, "form_id": FORM, "project_id": PROJECT } }))
            }),
        )
        .route(
            &format!("/api/v1/forms/{OTHER_FORM}/attachments"),
            get(move || {
                let counter = Arc::clone(&counter);
                async move {
                    counter.fetch_add(1, Ordering::SeqCst);
                    Json(json!({ "code": 0, "data": { "items": [] } }))
                }
            }),
        )
        .route(
            &format!("/api/v1/projects/{PROJECT}/agent-policy"),
            policy_route(json!(["form_attachments.list"])),
        );
    let server = server(crate::client::test_api::spawn(router).await?)?;

    let mismatched = server
        .call_tool(
            "form_attachments.list",
            json!({ "form_id": OTHER_FORM, "record_id": RECORD }),
        )
        .await;

    assert_eq!(mismatched.is_error, Some(true));
    assert!(
        message(&mismatched).contains("foreign form"),
        "{}",
        message(&mismatched)
    );
    assert_eq!(listings.load(Ordering::SeqCst), 0);
    Ok(())
}

#[tokio::test]
async fn events_tail_is_governed_by_the_owning_project_policy() -> TestResult {
    let reads = Arc::new(AtomicUsize::new(0));
    let read_counter = Arc::clone(&reads);
    let router = Router::new()
        .route(
            &format!("/api/v1/forms/{FORM}"),
            get(|| async { Json(json!({ "code": 0, "data": { "id": FORM, "project_id": PROJECT } })) }),
        )
        .route(
            &format!("/api/v1/forms/{FORM}/events"),
            get(move || {
                let counter = Arc::clone(&read_counter);
                async move {
                    counter.fetch_add(1, Ordering::SeqCst);
                    Json(json!({ "code": 0, "data": { "items": [] } }))
                }
            }),
        )
        .route(
            &format!("/api/v1/projects/{PROJECT}/agent-policy"),
            policy_route(json!(["forms.get"])),
        );
    let server = server(crate::client::test_api::spawn(router).await?)?;

    let denied = server.call_tool("events.tail", json!({ "form_id": FORM })).await;
    assert_eq!(denied.is_error, Some(true));
    assert!(message(&denied).contains("disabled by project agent policy"));

    let smuggled = server
        .call_tool("events.tail", json!({ "form_id": FORM, "project_id": FOREIGN_PROJECT }))
        .await;
    assert_eq!(smuggled.is_error, Some(true));

    let unscoped = server.call_tool("events.tail", json!({ "per_page": 50 })).await;
    assert_eq!(unscoped.is_error, Some(true));
    assert!(message(&unscoped).contains("carries no form_id, record_id or attachment_id"));

    assert_eq!(reads.load(Ordering::SeqCst), 0, "events must never be read");
    Ok(())
}

#[tokio::test]
async fn attachment_tools_fail_closed_while_the_api_has_no_lookup() -> TestResult {
    let mutations = Arc::new(AtomicUsize::new(0));
    let archive_counter = Arc::clone(&mutations);
    let restore_counter = Arc::clone(&mutations);
    // Only the mutating routes exist, exactly like the current API surface.
    let router = Router::new()
        .route(
            &format!("/api/v1/form-attachments/{ATTACHMENT}"),
            delete(move || {
                let counter = Arc::clone(&archive_counter);
                async move {
                    counter.fetch_add(1, Ordering::SeqCst);
                    Json(json!({ "code": 0, "message": "ok" }))
                }
            }),
        )
        .route(
            &format!("/api/v1/form-attachments/{ATTACHMENT}/restore"),
            post(move || {
                let counter = Arc::clone(&restore_counter);
                async move {
                    counter.fetch_add(1, Ordering::SeqCst);
                    Json(json!({ "code": 0, "data": { "id": ATTACHMENT } }))
                }
            }),
        );
    let server = server(crate::client::test_api::spawn(router).await?)?;

    for tool in ["form_attachments.archive", "form_attachments.restore"] {
        let result = server.call_tool(tool, json!({ "attachment_id": ATTACHMENT })).await;
        assert_eq!(result.is_error, Some(true), "{tool} must not be allowed through");
        let text = message(&result);
        assert!(text.contains("GET /api/v1/form-attachments/{attachment_id}"), "{text}");
    }
    assert_eq!(
        mutations.load(Ordering::SeqCst),
        0,
        "attachments must not be mutated while the policy cannot be evaluated"
    );
    Ok(())
}

#[tokio::test]
async fn attachment_tools_are_governed_once_the_api_exposes_the_lookup() -> TestResult {
    fn router(enabled_tools: Value, archives: &Arc<AtomicUsize>) -> Router {
        let counter = Arc::clone(archives);
        Router::new()
            .route(
                &format!("/api/v1/form-attachments/{ATTACHMENT}"),
                // The lookup only knows the owning form; the project is resolved from it.
                get(|| async { Json(json!({ "code": 0, "data": { "id": ATTACHMENT, "form_id": FORM } })) }).delete(
                    move || {
                        let counter = Arc::clone(&counter);
                        async move {
                            counter.fetch_add(1, Ordering::SeqCst);
                            Json(json!({ "code": 0, "message": "ok" }))
                        }
                    },
                ),
            )
            .route(
                &format!("/api/v1/forms/{FORM}"),
                get(|| async { Json(json!({ "code": 0, "data": { "id": FORM, "project_id": PROJECT } })) }),
            )
            .route(
                &format!("/api/v1/projects/{PROJECT}/agent-policy"),
                policy_route(enabled_tools),
            )
    }

    let denied_archives = Arc::new(AtomicUsize::new(0));
    let denied_server = server(crate::client::test_api::spawn(router(json!(["forms.get"]), &denied_archives)).await?)?;
    let denied = denied_server
        .call_tool("form_attachments.archive", json!({ "attachment_id": ATTACHMENT }))
        .await;
    assert_eq!(denied.is_error, Some(true));
    assert!(message(&denied).contains("disabled by project agent policy"));
    assert_eq!(denied_archives.load(Ordering::SeqCst), 0);

    let allowed_archives = Arc::new(AtomicUsize::new(0));
    let allowed_server =
        server(crate::client::test_api::spawn(router(json!(["form_attachments.archive"]), &allowed_archives)).await?)?;
    let allowed = allowed_server
        .call_tool("form_attachments.archive", json!({ "attachment_id": ATTACHMENT }))
        .await;
    assert_eq!(allowed.is_error, None, "{}", message(&allowed));
    assert_eq!(allowed_archives.load(Ordering::SeqCst), 1);

    // A foreign project_id stays refused even when the lookup works.
    let smuggled = allowed_server
        .call_tool(
            "form_attachments.archive",
            json!({ "attachment_id": ATTACHMENT, "project_id": FOREIGN_PROJECT }),
        )
        .await;
    assert_eq!(smuggled.is_error, Some(true));
    assert_eq!(allowed_archives.load(Ordering::SeqCst), 1);
    Ok(())
}

/// The round-2 gap: everything outside the form family ran with no project policy at
/// all. Work items, comments, plugins and check results are now resolved back to their
/// owning project exactly like form data.
#[tokio::test]
async fn id_addressed_non_form_tools_are_governed_by_the_owning_project() -> TestResult {
    let mutations = Arc::new(AtomicUsize::new(0));
    let delete_counter = Arc::clone(&mutations);
    let invoke_counter = Arc::clone(&mutations);
    let issue_lookups = Arc::new(AtomicUsize::new(0));
    let issue_counter = Arc::clone(&issue_lookups);

    let router = Router::new()
        .route(
            &format!("/api/v1/issues/{WORK_ITEM}"),
            get(move || {
                let counter = Arc::clone(&issue_counter);
                async move {
                    counter.fetch_add(1, Ordering::SeqCst);
                    Json(json!({ "code": 0, "data": { "id": WORK_ITEM, "project_id": PROJECT } }))
                }
            })
            .delete(move || {
                let counter = Arc::clone(&delete_counter);
                async move {
                    counter.fetch_add(1, Ordering::SeqCst);
                    Json(json!({ "code": 0, "message": "ok" }))
                }
            }),
        )
        .route(
            &format!("/api/v1/plugins/{PLUGIN}"),
            get(|| async { Json(json!({ "code": 0, "data": { "id": PLUGIN, "project_id": PROJECT } })) }),
        )
        .route(
            &format!("/api/v1/plugins/{PLUGIN}/invoke"),
            post(move || {
                let counter = Arc::clone(&invoke_counter);
                async move {
                    counter.fetch_add(1, Ordering::SeqCst);
                    Json(json!({ "code": 0, "data": {} }))
                }
            }),
        )
        .route(
            &format!("/api/v1/projects/{PROJECT}/agent-policy"),
            policy_route(json!(["work_items.list"])),
        );
    let server = server(crate::client::test_api::spawn(router).await?)?;

    for (tool, args) in [
        ("work_items.delete", json!({ "work_item_id": WORK_ITEM })),
        (
            "work_items.update",
            json!({ "work_item_id": WORK_ITEM, "state": "done" }),
        ),
        ("comments.list", json!({ "work_item_id": WORK_ITEM })),
        (
            "plugins.invoke",
            json!({ "plugin_id": PLUGIN, "hook_kind": "on_create" }),
        ),
    ] {
        let result = server.call_tool(tool, args).await;
        assert_eq!(result.is_error, Some(true), "{tool} escaped the policy gate");
        assert!(
            message(&result).contains("disabled by project agent policy"),
            "{tool}: {}",
            message(&result)
        );
    }
    assert_eq!(mutations.load(Ordering::SeqCst), 0, "no backend mutation may happen");
    // Four calls, two distinct owners, and the owner of each is read exactly once.
    assert_eq!(issue_lookups.load(Ordering::SeqCst), 1, "owner lookups must be cached");
    Ok(())
}

/// The blocker from the round-2 audit, end to end: a caller aims a destructive tool
/// at a project it does not own by smuggling the id, and the smuggled project's
/// policy must never be consulted nor the destructive endpoint reached.
#[tokio::test]
async fn a_smuggled_project_id_cannot_reach_a_destructive_tool() -> TestResult {
    let deletes = Arc::new(AtomicUsize::new(0));
    let policy_reads = Arc::new(AtomicUsize::new(0));
    let delete_counter = Arc::clone(&deletes);
    let policy_counter = Arc::clone(&policy_reads);

    let router = Router::new()
        .route(
            &format!("/api/v1/projects/{PROJECT}"),
            delete(move || {
                let counter = Arc::clone(&delete_counter);
                async move {
                    counter.fetch_add(1, Ordering::SeqCst);
                    Json(json!({ "code": 0, "message": "deleted" }))
                }
            }),
        )
        .route(
            &format!("/api/v1/projects/{PROJECT}/work-items"),
            get(|| async { Json(json!({ "code": 0, "data": { "items": [] } })) }),
        )
        .route(
            &format!("/api/v1/projects/{PROJECT}/agent-policy"),
            get(move || {
                let counter = Arc::clone(&policy_counter);
                async move {
                    counter.fetch_add(1, Ordering::SeqCst);
                    Json(json!({
                        "code": 0,
                        "data": { "mcp": { "tool_registry": { "enabled_tools": ["projects.list"] } } }
                    }))
                }
            }),
        );
    let server = server(crate::client::test_api::spawn(router).await?)?;

    // The exact shapes the audit used to walk the policy lookup off `/agent-policy`.
    for smuggled in [
        format!("{PROJECT}?"),
        format!("{PROJECT}/work-items?pad="),
        format!("{PROJECT}#"),
        format!("{PROJECT}/"),
    ] {
        let result = server
            .call_tool("projects.delete", json!({ "project_id": smuggled }))
            .await;
        assert_eq!(result.is_error, Some(true), "'{smuggled}' was accepted");
        assert!(
            message(&result).contains("is not a canonical UUID"),
            "'{smuggled}': {}",
            message(&result)
        );
    }
    assert_eq!(deletes.load(Ordering::SeqCst), 0, "the project must never be deleted");
    assert_eq!(
        policy_reads.load(Ordering::SeqCst),
        0,
        "a malformed project id must not even reach the policy endpoint"
    );

    // The honest call is still evaluated, and the policy still says no.
    let honest = server
        .call_tool("projects.delete", json!({ "project_id": PROJECT }))
        .await;
    assert_eq!(honest.is_error, Some(true));
    assert!(message(&honest).contains("disabled by project agent policy"));
    assert_eq!(deletes.load(Ordering::SeqCst), 0);
    assert_eq!(policy_reads.load(Ordering::SeqCst), 1);
    Ok(())
}

/// Tools whose owner the API cannot report must refuse rather than run unguarded.
#[tokio::test]
async fn tools_without_an_owner_lookup_fail_closed() -> TestResult {
    let mutations = Arc::new(AtomicUsize::new(0));
    let sprint_counter = Arc::clone(&mutations);
    let comment_counter = Arc::clone(&mutations);
    let router = Router::new()
        .route(
            &format!("/api/v1/sprints/{SPRINT}"),
            delete(move || {
                let counter = Arc::clone(&sprint_counter);
                async move {
                    counter.fetch_add(1, Ordering::SeqCst);
                    Json(json!({ "code": 0, "message": "ok" }))
                }
            }),
        )
        .route(
            &format!("/api/v1/comments/{COMMENT}"),
            delete(move || {
                let counter = Arc::clone(&comment_counter);
                async move {
                    counter.fetch_add(1, Ordering::SeqCst);
                    Json(json!({ "code": 0, "message": "ok" }))
                }
            }),
        );
    let server = server(crate::client::test_api::spawn(router).await?)?;

    for (tool, args, endpoint) in [
        (
            "sprints.delete",
            json!({ "sprint_id": SPRINT }),
            "GET /api/v1/sprints/{sprint_id}",
        ),
        (
            "sprints.update",
            json!({ "sprint_id": SPRINT, "name": "renamed" }),
            "GET /api/v1/sprints/{sprint_id}",
        ),
        (
            "comments.delete",
            json!({ "comment_id": COMMENT }),
            "GET /api/v1/comments/{comment_id}",
        ),
    ] {
        let result = server.call_tool(tool, args).await;
        assert_eq!(result.is_error, Some(true), "{tool} ran unguarded");
        assert!(message(&result).contains(endpoint), "{tool}: {}", message(&result));
    }
    assert_eq!(mutations.load(Ordering::SeqCst), 0);
    Ok(())
}

/// Workspace scoped tools have no owning project by design, but a call that smuggles
/// a project owned id into one is a classification mismatch and is refused.
#[tokio::test]
async fn workspace_scoped_tools_reject_project_owned_identifiers() -> TestResult {
    let router = Router::new().route(
        "/api/v1/workspaces/workspace-1/labels",
        get(|| async { Json(json!({ "code": 0, "data": { "items": [] } })) }),
    );
    let server = server(crate::client::test_api::spawn(router).await?)?;

    let clean = server.call_tool("labels.list", json!({})).await;
    assert_eq!(clean.is_error, None, "{}", message(&clean));

    let smuggled = server.call_tool("labels.list", json!({ "record_id": RECORD })).await;
    assert_eq!(smuggled.is_error, Some(true));
    assert!(
        message(&smuggled).contains("registered as workspace scoped"),
        "{}",
        message(&smuggled)
    );
    Ok(())
}

/// Every `Ok(_) => success("... deleted")` call site must still report backend
/// failures, because the API answers HTTP 200 and carries the real status in the
/// response envelope.
#[tokio::test]
async fn mutation_tools_report_envelope_errors_instead_of_fake_success() -> TestResult {
    async fn denied() -> Json<Value> {
        Json(json!({ "code": 403, "message": "policy denied", "data": null }))
    }

    let router = Router::new()
        .route(&format!("/api/v1/projects/{PROJECT}"), delete(denied))
        .route(&format!("/api/v1/labels/{LABEL}"), delete(denied))
        .route(
            &format!("/api/v1/issues/{WORK_ITEM}"),
            get(|| async { Json(json!({ "code": 0, "data": { "id": WORK_ITEM, "project_id": PROJECT } })) })
                .delete(denied),
        )
        .route(
            &format!("/api/v1/issues/{WORK_ITEM}/labels/{LABEL}"),
            post(denied).delete(denied),
        )
        .route(&format!("/api/v1/issues/{WORK_ITEM}/labels/batch"), post(denied))
        .route(
            &format!("/api/v1/projects/{PROJECT}/agent-policy"),
            get(|| async { Json(json!({ "code": 0, "data": {} })) }),
        );
    let server = server(crate::client::test_api::spawn(router).await?)?;

    let calls = [
        ("projects.delete", json!({ "project_id": PROJECT })),
        ("labels.delete", json!({ "label_id": LABEL })),
        ("work_items.delete", json!({ "work_item_id": WORK_ITEM })),
        (
            "work_items.add_label",
            json!({ "work_item_id": WORK_ITEM, "label_id": LABEL }),
        ),
        (
            "work_items.remove_label",
            json!({ "work_item_id": WORK_ITEM, "label_id": LABEL }),
        ),
        (
            "work_items.add_labels",
            json!({ "work_item_id": WORK_ITEM, "label_ids": [LABEL] }),
        ),
    ];

    for (tool, args) in calls {
        let result = server.call_tool(tool, args).await;
        let text = message(&result);
        assert_eq!(result.is_error, Some(true), "{tool} reported success: {text}");
        assert!(text.contains("API error 403"), "{tool}: {text}");
        assert!(text.contains("policy denied"), "{tool}: {text}");
    }
    Ok(())
}

/// An agent-policy answer that is not the `{code, message, data}` envelope must not
/// authorize anything: that fail-open default is what made a smuggled policy URL
/// useful in the first place.
#[tokio::test]
async fn a_non_envelope_policy_answer_never_authorizes() -> TestResult {
    let deletes = Arc::new(AtomicUsize::new(0));
    let counter = Arc::clone(&deletes);
    let router = Router::new()
        .route(
            &format!("/api/v1/projects/{PROJECT}"),
            delete(move || {
                let counter = Arc::clone(&counter);
                async move {
                    counter.fetch_add(1, Ordering::SeqCst);
                    Json(json!({ "code": 0, "message": "deleted" }))
                }
            }),
        )
        .route(
            &format!("/api/v1/projects/{PROJECT}/agent-policy"),
            // A 200 with a project object rather than a policy envelope.
            get(|| async { Json(json!({ "id": PROJECT, "name": "victim" })) }),
        );
    let server = server(crate::client::test_api::spawn(router).await?)?;

    let result = server
        .call_tool("projects.delete", json!({ "project_id": PROJECT }))
        .await;

    assert_eq!(result.is_error, Some(true));
    assert!(message(&result).contains("Malformed response"), "{}", message(&result));
    assert_eq!(deletes.load(Ordering::SeqCst), 0);
    Ok(())
}

#[test]
fn tool_call_audit_summary_uses_text_content() {
    let result = CallToolResult {
        content: vec![ToolContent::text("done")],
        is_error: None,
    };

    assert_eq!(summarize_tool_result(&result).as_deref(), Some("done"));
}

/// The whole `resources/read` surface, one row per readable URI, pinned against the
/// tool whose policy governs it.
///
/// This is the table the resource door is audited from. A new resource that reaches
/// the API without a row here is the exact bug this test exists to catch: it would
/// be served by `handle_resources_read` while `resource_policy_subject` classified it
/// `Ungoverned`, which is a second unauthenticated door onto project data.
#[test]
fn every_readable_resource_uri_is_governed_like_its_tool() -> TestResult {
    let governed = [
        // Project addressed reads.
        (format!("sylvode://projects/{PROJECT}/issues"), "work_items.list"),
        (format!("sylvode://projects/{PROJECT}/forms"), "forms.list"),
        (format!("sylvode://projects/{PROJECT}/context"), "context.get_project"),
        (
            format!("sylvode://projects/{PROJECT}/governance"),
            "context.get_governance",
        ),
        (
            format!("sylvode://projects/{PROJECT}/agent-policy"),
            "context.get_agent_policy",
        ),
        (
            format!("sylvode://projects/{PROJECT}/release-readiness"),
            "release.readiness.get",
        ),
        (format!("sylvode://projects/{PROJECT}/type"), "projects.get"),
        (
            format!("sylvode://projects/{PROJECT}/recent-decisions"),
            "context.get_governance",
        ),
        (format!("sylvode://projects/{PROJECT}/sprints"), "sprints.list"),
    ];
    for (uri, expected_tool) in governed {
        assert_eq!(
            resource_policy_subject(&uri)?,
            ResourceSubject::Tool {
                tool: expected_tool,
                args: json!({ "project_id": PROJECT }),
            },
            "{uri} is not governed by {expected_tool}"
        );
    }

    // Form addressed reads resolve their owner from the form, exactly like the tools.
    for (uri, expected_tool) in [
        (format!("sylvode://forms/{FORM}"), "forms.get"),
        (format!("sylvode://forms/{FORM}/records"), "form_records.list"),
        (format!("sylvode://forms/{FORM}/events"), "events.tail"),
    ] {
        assert_eq!(
            resource_policy_subject(&uri)?,
            ResourceSubject::Tool {
                tool: expected_tool,
                args: json!({ "form_id": FORM }),
            },
            "{uri} is not governed by {expected_tool}"
        );
    }

    for (uri, expected_tool) in [
        (format!("sylvode://form-records/{RECORD}"), "form_records.get"),
        (format!("sylvode://form-records/{RECORD}/events"), "events.tail"),
    ] {
        assert_eq!(
            resource_policy_subject(&uri)?,
            ResourceSubject::Tool {
                tool: expected_tool,
                args: json!({ "record_id": RECORD }),
            },
            "{uri} is not governed by {expected_tool}"
        );
    }

    assert_eq!(
        resource_policy_subject("sylvode://issues/PRX-42")?,
        ResourceSubject::Tool {
            tool: ISSUE_IDENTIFIER_POLICY_TOOL,
            args: json!({ "identifier": "PRX-42" }),
        }
    );

    // The resources no project agent policy can govern, and why: the guides are compile time
    // constants, the scenario catalogue is global and its tools are `WorkspaceWide`.
    for uri in [
        "sylvode://skills/sylvode-mcp",
        "sylvode://skills/sylvode-mcp/tools",
        "sylvode://guides/workflows",
        "sylvode://scenario-templates",
        "sylvode://scenario-templates/software-delivery",
    ] {
        assert_eq!(
            resource_policy_subject(uri)?,
            ResourceSubject::Ungoverned,
            "{uri} should carry no project policy"
        );
    }

    Ok(())
}

/// Every tool named in the resource tables has to be a tool the policy gate knows, or
/// the resource door would be authorized against a name no policy can ever list.
#[test]
fn every_resource_policy_tool_is_registered_in_the_policy_table() {
    let registered: Vec<&str> = TOOL_POLICY_SCOPES.iter().map(|(name, _)| *name).collect();
    let referenced = PROJECT_RESOURCE_POLICY_TOOLS
        .iter()
        .chain(FORM_RESOURCE_POLICY_TOOLS.iter())
        .chain(FORM_RECORD_RESOURCE_POLICY_TOOLS.iter())
        .map(|(_, tool)| *tool)
        .chain(std::iter::once(ISSUE_IDENTIFIER_POLICY_TOOL));
    for tool in referenced {
        assert!(
            registered.contains(&tool),
            "resource tables map to '{tool}', which is not in TOOL_POLICY_SCOPES"
        );
    }
}

/// A project scoped resource whose id is not a canonical UUID is refused outright.
/// Before the gate existed the raw text went straight into the API path, which is the
/// URL reshaping `canonical_uuid` was introduced to stop on the tool side.
#[test]
fn resource_uris_carrying_an_unparsable_id_are_refused() {
    for uri in [
        "sylvode://projects/not-a-uuid/sprints",
        &format!("sylvode://projects/{PROJECT}%2Fpad/agent-policy"),
        &format!("sylvode://projects/{PROJECT}?pad=/sprints"),
        "sylvode://forms/not-a-uuid/records",
        "sylvode://form-records/not-a-uuid/events",
        "sylvode://issues/PRX 42",
    ] {
        assert!(
            resource_policy_subject(uri).is_err(),
            "{uri} was classified instead of refused"
        );
    }
}

/// The project resources template is parked: no URI of that shape is listed or read, and
/// the read never reaches the API (nothing listens on the port below).
#[tokio::test]
async fn the_parked_project_resources_template_is_unknown() -> TestResult {
    let server = server("http://127.0.0.1:1".to_string())?;
    let uri = format!("sylvode://projects/{PROJECT}/resources");
    assert!(
        PROJECT_RESOURCE_POLICY_TOOLS
            .iter()
            .all(|(suffix, _)| *suffix != "/resources"),
        "the policy table still maps /resources"
    );
    assert_eq!(resource_policy_subject(&uri)?, ResourceSubject::Ungoverned);
    let response = server
        .handle_resources_read(Some(json!(1)), Some(json!({ "uri": uri })))
        .await;
    let body = serde_json::to_string(&response)?;
    assert!(body.contains("Unknown resource URI"), "{uri} -> {body}");
    Ok(())
}

/// A traversal shaped URI is stopped one step earlier: the id would span a `/`, so no
/// parser claims it, `handle_resources_read` matches nothing and answers "Unknown
/// resource URI" without calling the API. Classifying it `Ungoverned` is therefore
/// correct rather than a hole — this test is what keeps the two facts tied together,
/// because "no policy applies" is only safe while "no backend is reached" holds.
#[tokio::test]
async fn traversal_shaped_resource_uris_reach_no_backend() -> TestResult {
    // Any API call would fail outright: nothing listens on this port.
    let server = server("http://127.0.0.1:1".to_string())?;
    for uri in [
        "sylvode://projects/../../admin/resources",
        "sylvode://forms/../../workspaces/records",
        "sylvode://form-records/a/b/events",
    ] {
        assert_eq!(
            resource_policy_subject(uri)?,
            ResourceSubject::Ungoverned,
            "{uri} was classified as a governed read"
        );
        let response = server
            .handle_resources_read(Some(json!(1)), Some(json!({ "uri": uri })))
            .await;
        let body = serde_json::to_string(&response)?;
        assert!(body.contains("Unknown resource URI"), "{uri} -> {body}");
    }
    Ok(())
}

/// The guides are process memory, so they stay readable without any API round trip.
/// A gate that needed the API to serve a compile time constant would be a new failure
/// mode, not a control.
#[tokio::test]
async fn static_guides_are_served_without_consulting_any_policy() -> TestResult {
    // Base URL points at a port nothing listens on: any API call would fail the read.
    let server = server("http://127.0.0.1:1".to_string())?;
    let response = server
        .handle_resources_read(Some(json!(1)), Some(json!({ "uri": "sylvode://guides/workflows" })))
        .await;
    let body = serde_json::to_string(&response)?;
    assert!(body.contains("Common workflows"), "{body}");
    Ok(())
}
