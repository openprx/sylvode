//! Bounded Flow-only tools. Scope/home resolution is local to one authenticated call.
use super::objects::{get_structured, request_structured, respond, respond_data};
use crate::client::OpenPrClient;
use crate::protocol::{CallToolResult, ToolDefinition};
use serde_json::{Value, json};

pub const TOOLS: &[&str] = &[
    "flow.home",
    "objects.query",
    "objects.create",
    "objects.get",
    "objects.history",
    "objects.command",
    "objects.search",
    "collections.describe",
    "collections.query",
    "records.create",
    "objects.export",
];
const COMMANDS: &[&str] = &[
    "initialize_editor_doc",
    "insert_body_asset",
    "replace_body_asset",
    "link_child_page",
    "prepare_child_page",
    "upload_body_asset",
    "link",
    "unlink",
    "set_title",
    "insert_block",
    "update_block",
    "delete_block",
    "move_block",
    "semantic_patch",
    "archive",
    "restore",
    "move_object",
    "field_create",
    "field_update",
    "field_archive",
    "field_reorder",
    "view_create",
    "view_update",
    "view_reorder",
    "record_patch",
    "record_archive",
    "create_collection_embed",
];

// Only internal request/schema object builders. Never normalize response or artifact content.
fn set_field(object: &mut Value, key: &str, value: Value) {
    if let Some(fields) = object.as_object_mut() {
        fields.insert(key.to_owned(), value);
    }
}

pub fn supports(name: &str) -> bool {
    TOOLS.contains(&name) || super::flow_scope_existing::supports(name)
}
pub fn definitions() -> Vec<ToolDefinition> {
    let mut result=TOOLS.iter().map(|name| {
        let selector=matches!(*name,"objects.query"|"objects.create"|"objects.search");
        let mut properties=json!({"scope_id":{"type":"string","format":"uuid"},"workspace_id":{"type":"string","format":"uuid"},"project_id":{"type":"string","format":"uuid"}});
        let mut required=Vec::new();
        if selector {set_field(&mut properties, "personal", json!({"const":true}));}
        match *name {
            "flow.home"=>properties=json!({}),
            "objects.get"|"objects.history"|"objects.command"|"objects.export"=>{set_field(&mut properties, "object_id", json!({"type":"string","format":"uuid"}));required.push("object_id");},
            "objects.create"=>{set_field(&mut properties, "type", json!({"enum":["page","navigator","collection"]}));set_field(&mut properties, "title", json!({"type":"string"}));set_field(&mut properties, "parent_id", json!({"type":"string","format":"uuid"}));set_field(&mut properties, "initial_fields", json!({"type":"array","items":{"type":"object"}}));set_field(&mut properties, "initial_view", json!({"type":"object"}));required.extend(["type","title","idempotency_key"]);},
            "collections.describe"|"collections.query"|"records.create"=>{set_field(&mut properties, "collection_id", json!({"type":"string","format":"uuid"}));required.push("collection_id");},
            _=>{}
        }
        if matches!(*name,"objects.query"|"objects.search"|"objects.history"|"collections.query") {
            set_field(&mut properties, "limit", json!({"type":"integer","minimum":1,"maximum":100}));
        }
        if matches!(*name,"objects.query"|"objects.search"|"collections.query") {set_field(&mut properties, "cursor", json!({"type":"string"}));}
        if matches!(*name,"objects.query"|"objects.search") {
            set_field(&mut properties, "q", json!({"type":"string"}));set_field(&mut properties, "unprojected", json!({"type":"boolean"}));
        }
        if *name=="objects.query"{set_field(&mut properties, "type", json!({"type":"string"}));set_field(&mut properties, "include_archived", json!({"type":"boolean"}));}
        if *name=="objects.search"{set_field(&mut properties, "freshness", json!({"enum":["allow_stale","require_current"]}));required.push("q");}
        if *name=="objects.get" {set_field(&mut properties,"at_seq",json!({"type":"integer","minimum":0}));set_field(&mut properties,"render",json!({"enum":["semantic_json","markdown"]}));}
        if *name=="collections.describe" {set_field(&mut properties,"record_id",json!({"type":"string","format":"uuid","description":"Bounded Record-only visible field context, no parent Collection access"}));required.retain(|key|*key!="collection_id");set_field(&mut properties,"at_seq",json!({"type":"integer","minimum":0,"description":"Must match current schema_seq; historical typed schema is not supported"}));}
        if *name=="objects.create" {set_field(&mut properties,"embed_page_id",json!({"type":"string","format":"uuid","description":"Atomic Collection embed; this Page determines actual scope/project, never own home"}));set_field(&mut properties,"after_block_id",json!({"type":"string","description":"Existing tree block anchor only; editor prose CID/alias placement is unsupported. Omit to append."}));}
        if *name=="objects.history"{set_field(&mut properties, "before_seq", json!({"type":"integer","minimum":0}));}
        if *name=="objects.export"{set_field(&mut properties, "format", json!({"enum":["json","markdown","csv"]}));required.extend(["format","idempotency_key"]);}
        if *name=="records.create"{set_field(&mut properties, "values_by_field_id", json!({"type":"object"}));set_field(&mut properties, "body", json!({"type":"string"}));required.extend(["values_by_field_id","idempotency_key"]);}
        if *name=="collections.query"{for key in ["filter","sort"]{set_field(&mut properties, key, json!({"type":["object","null"]}));}set_field(&mut properties, "group", json!({"type":["string","null"]}));set_field(&mut properties, "field_ids", json!({"type":"array","items":{"type":"string","format":"uuid"}}));}
        if *name=="objects.command" {set_field(&mut properties, "command", json!({"type":"object","required":["type"],"properties":{"type":{"enum":COMMANDS},"payload":{"type":"object"}},"additionalProperties":false}));set_field(&mut properties, "expected_frontier", json!({"type":"string"}));required.extend(["command","idempotency_key"]);}
        if matches!(*name,"objects.create"|"objects.command"|"records.create"|"objects.export"){set_field(&mut properties, "idempotency_key", json!({"type":"string","minLength":1,"maxLength":128}));set_field(&mut properties, "message", json!({"type":"string"}));}
        let mut schema=json!({"type":"object","properties":properties,"required":required,"additionalProperties":false});
        if selector {set_field(&mut schema, "oneOf", json!([
            {"required":["personal"],"not":{"anyOf":[{"required":["scope_id"]},{"required":["workspace_id"]}]}},
            {"required":["scope_id"],"not":{"anyOf":[{"required":["personal"]},{"required":["workspace_id"]}]}},
            {"required":["workspace_id"],"not":{"anyOf":[{"required":["personal"]},{"required":["scope_id"]}]}}
        ]));}
        if *name=="collections.describe"{set_field(&mut schema,"oneOf",json!([{"required":["collection_id"],"not":{"required":["record_id"]}},{"required":["record_id"],"not":{"required":["collection_id"]}}]));}
        if *name=="objects.create" {
            set_field(&mut schema,"oneOf",json!([
                {"required":["personal"],"not":{"anyOf":[{"required":["scope_id"]},{"required":["workspace_id"]},{"required":["embed_page_id"]}]}},
                {"required":["scope_id"],"not":{"anyOf":[{"required":["personal"]},{"required":["workspace_id"]},{"required":["embed_page_id"]}]}},
                {"required":["workspace_id"],"not":{"anyOf":[{"required":["personal"]},{"required":["scope_id"]},{"required":["embed_page_id"]}]}},
                {"required":["embed_page_id"],"properties":{"type":{"const":"collection"}},"not":{"anyOf":[{"required":["personal"]},{"required":["parent_id"]}]}}
            ]));
        }
        ToolDefinition{annotations:None,name:(*name).into(),description:format!("Bounded Flow-only {name}: current semantic resources and typed commands. Scopes use v2 authority; a workspace bot reaches workspace content only. Package/history restoration and raw collaborative payloads are not advertised. Home is resolved only on an actual call."),input_schema:schema}
    }).collect::<Vec<_>>();
    result.extend(super::flow_scope_existing::definitions());
    result
}

pub(crate) fn uuid(args: &Value, key: &str) -> Result<Option<String>, CallToolResult> {
    let Some(value) = args.get(key) else { return Ok(None) };
    let value = value
        .as_str()
        .ok_or_else(|| CallToolResult::error(format!("{key} must be a canonical UUID")))?;
    let parsed =
        uuid::Uuid::parse_str(value).map_err(|_| CallToolResult::error(format!("{key} must be a canonical UUID")))?;
    if parsed.is_nil() || parsed.to_string() != value {
        return Err(CallToolResult::error(format!("{key} must be a canonical nonzero UUID")));
    }
    Ok(Some(parsed.to_string()))
}
pub(crate) async fn data(client: &OpenPrClient, path: &str) -> Result<Value, CallToolResult> {
    let value = get_structured(client, path)
        .await
        .map_err(|error| respond(Err(error)))?;
    value
        .get("data")
        .filter(|value| value.is_object())
        .cloned()
        .ok_or_else(|| CallToolResult::error("Flow API returned no resource data"))
}
fn resource_project(resource: &Value) -> Result<Option<String>, CallToolResult> {
    match resource.get("project_id") {
        Some(Value::Null) => Ok(None),
        Some(Value::String(_)) => uuid(resource, "project_id"),
        _ => Err(CallToolResult::error(
            "Actual Flow resource has no valid project ownership metadata; no mutation sent",
        )),
    }
}

fn reject_claim(args: &Value, key: &str, actual: Option<&str>) -> Result<(), CallToolResult> {
    if let Some(claim) = uuid(args, key)?
        && Some(claim.as_str()) != actual
    {
        return Err(CallToolResult::error(format!(
            "{key} does not match the canonical Flow resource"
        )));
    }
    Ok(())
}
#[derive(Debug)]
pub struct Request {
    pub path: String,
    pub body: Option<Value>,
    pub policy_projects: Vec<String>,
    pub policy_tools: Vec<String>,
    pub method: reqwest::Method,
    pub idempotency_key: Option<String>,
    pub form_policy_args: Vec<Value>,
    pub workspace_id: Option<String>,
    pub object_type: Option<String>,
    pub record_context_supported: bool,
}

/// No cache is stored in `OpenPrClient` or `McpServer`, including across `acting_as` identities.
pub async fn prepare(client: &OpenPrClient, name: &str, args: &Value) -> Result<Request, CallToolResult> {
    if name == "collections.describe" && args.get("record_id").is_some() {
        return super::flow_scope_existing::prepare_record_context(client, args).await;
    }
    if name == "objects.create" && args.get("embed_page_id").is_some() {
        return super::flow_scope_existing::prepare_embed(client, args).await;
    }
    if super::flow_scope_existing::supports(name) {
        return super::flow_scope_existing::prepare(client, name, args).await;
    }
    prepare_core(client, name, args).await
}

pub(crate) fn validate_arguments(name: &str, args: &Value) -> Result<(), CallToolResult> {
    if !args.is_object() {
        return Err(CallToolResult::error("Flow arguments must be an object"));
    }
    if !supports(name) {
        return Err(CallToolResult::error("Tool is not registered in Flow-only mode"));
    }
    let definition = definitions()
        .into_iter()
        .find(|definition| definition.name == name)
        .ok_or_else(|| CallToolResult::error("Unregistered Flow tool"))?;
    let properties = definition
        .input_schema
        .get("properties")
        .and_then(Value::as_object)
        .ok_or_else(|| CallToolResult::error("Invalid Flow tool schema"))?;
    if args
        .as_object()
        .is_some_and(|values| values.keys().any(|key| !properties.contains_key(key)))
    {
        return Err(CallToolResult::error("Unexpected argument for this bounded Flow tool"));
    }
    if definition
        .input_schema
        .get("required")
        .and_then(Value::as_array)
        .is_some_and(|keys| {
            keys.iter()
                .filter_map(Value::as_str)
                .any(|key| args.get(key).is_none_or(Value::is_null))
        })
    {
        return Err(CallToolResult::error("Missing required Flow tool argument"));
    }
    Ok(())
}

pub(crate) async fn prepare_core(client: &OpenPrClient, name: &str, args: &Value) -> Result<Request, CallToolResult> {
    validate_arguments(name, args)?;
    if name == "objects.create" && args.get("after_block_id").is_some() {
        return Err(CallToolResult::error(
            "after_block_id requires an explicit embed_page_id",
        ));
    }
    if args.get("at_seq").is_some_and(|value| value.as_u64().is_none()) {
        return Err(CallToolResult::error("at_seq must be a nonnegative integer"));
    }
    if args
        .get("render")
        .is_some_and(|value| !matches!(value.as_str(), Some("semantic_json" | "markdown")))
    {
        return Err(CallToolResult::error("render must be semantic_json or markdown"));
    }
    let base = "/api/v2/flow";
    if name == "flow.home" {
        if args.as_object().is_some_and(|value| !value.is_empty()) {
            return Err(CallToolResult::error(
                "flow.home accepts no workspace/project or identity override",
            ));
        }
        return Ok(Request {
            path: format!("{base}/home"),
            body: None,
            policy_projects: Vec::new(),
            policy_tools: vec![name.to_owned()],
            method: reqwest::Method::GET,
            idempotency_key: None,
            form_policy_args: Vec::new(),
            workspace_id: None,
            object_type: None,
            record_context_supported: false,
        });
    }
    let resource_key = if matches!(name, "collections.describe" | "collections.query" | "records.create") {
        "collection_id"
    } else {
        "object_id"
    };
    let object_id = uuid(args, resource_key)?;
    let selector_call = matches!(name, "objects.query" | "objects.create" | "objects.search");
    if selector_call && object_id.is_some() {
        return Err(CallToolResult::error(
            "List/create/search require an explicit selector, not object_id",
        ));
    }
    if !selector_call && object_id.is_none() {
        return Err(CallToolResult::error("object_id is required"));
    }
    if args
        .get("limit")
        .is_some_and(|value| value.as_u64().is_none_or(|limit| !(1..=100).contains(&limit)))
    {
        return Err(CallToolResult::error("limit must be between 1 and 100"));
    }
    if name == "objects.search" && args.get("q").and_then(Value::as_str).is_none() {
        return Err(CallToolResult::error("q is required"));
    }

    let mut resource_type = None;
    let mut record_context_supported = false;
    let parent_id = if name == "objects.create" {
        uuid(args, "parent_id")?
    } else {
        None
    };
    let mut parent_resource = None;
    let (scope, workspace, mut project) = if let Some(object) = object_id.as_deref() {
        let resource = data(client, &format!("{base}/objects/{object}")).await?;
        let workspace = resource.get("workspace_id").and_then(Value::as_str).map(str::to_owned);
        let scope = resource.get("scope_id").and_then(Value::as_str).map(str::to_owned);
        if scope.is_none() {
            return Err(CallToolResult::error(
                "Canonical Flow scope_id is missing from object response",
            ));
        }
        let project = resource_project(&resource)?;
        if workspace.is_none() && project.is_some() {
            return Err(CallToolResult::error(
                "Personal Flow cannot inherit a project policy from destination context",
            ));
        }
        reject_claim(args, "scope_id", scope.as_deref())?;
        reject_claim(args, "workspace_id", workspace.as_deref())?;
        reject_claim(args, "project_id", project.as_deref())?;
        if args.get("personal").is_some() {
            return Err(CallToolResult::error(
                "Object-target calls resolve scope from the object; do not supply personal",
            ));
        }
        resource_type = resource.get("object_type").and_then(Value::as_str).map(str::to_owned);
        record_context_supported = resource.get("record_context_supported").and_then(Value::as_bool) == Some(true);
        if resource_key == "collection_id" && resource_type.as_deref() != Some("collection") {
            return Err(CallToolResult::error(
                "collection_id must identify a Collection; its mutations stay typed",
            ));
        }
        (scope, workspace, project)
    } else {
        let scope = uuid(args, "scope_id")?;
        let workspace = uuid(args, "workspace_id")?;
        let personal = args.get("personal").is_some_and(|value| value.as_bool() == Some(true));
        if args.get("personal").is_some() && !personal {
            return Err(CallToolResult::error("personal must be true when supplied"));
        }
        if usize::from(scope.is_some()) + usize::from(workspace.is_some()) + usize::from(personal) != 1 {
            return Err(CallToolResult::error(
                "Choose exactly one of personal:true, scope_id, or real workspace_id",
            ));
        }
        {
            let info = if personal {
                data(client, "/api/v2/flow/home").await?
            } else if let Some(workspace) = workspace.as_deref() {
                data(client, &format!("/api/v2/flow/workspaces/{workspace}/scope")).await?
            } else {
                let scope = scope.as_deref().unwrap_or_default();
                if let Some(parent) = parent_id.as_deref() {
                    let parent = data(client, &format!("{base}/objects/{parent}")).await?;
                    if parent.get("scope_id").and_then(Value::as_str) != Some(scope) {
                        return Err(CallToolResult::error(
                            "Parent does not belong to the requested canonical scope",
                        ));
                    }
                    let info = json!({"scope":{"id":scope,"kind":if parent.get("workspace_id").and_then(Value::as_str).is_some(){"legacy_managed"}else{"personal"}},"workspace_id":parent.get("workspace_id")});
                    parent_resource = Some(parent);
                    info
                } else {
                    let nav = data(client, &format!("/api/v2/flow/scopes/{scope}/navigator")).await?;
                    let root = nav
                        .get("root_object_id")
                        .and_then(Value::as_str)
                        .ok_or_else(|| CallToolResult::error("Flow navigator response has no canonical root"))?;
                    let root = data(client, &format!("/api/v2/flow/objects/{root}")).await?;
                    if root.get("scope_id").and_then(Value::as_str) != Some(scope) {
                        return Err(CallToolResult::error("Flow root scope does not match requested scope"));
                    }
                    json!({"scope":{"id":scope,"kind":if root.get("workspace_id").and_then(Value::as_str).is_some(){"legacy_managed"}else{"personal"}},"workspace_id":root.get("workspace_id")})
                }
            };
            let id = info
                .pointer("/scope/id")
                .and_then(Value::as_str)
                .ok_or_else(|| CallToolResult::error("Flow scope response has no canonical scope id"))?
                .to_owned();
            let personal_kind = info.pointer("/scope/kind").and_then(Value::as_str) == Some("personal");
            if personal && !personal_kind {
                return Err(CallToolResult::error(
                    "Home response must identify a real personal Flow scope",
                ));
            }
            if workspace.is_some() && personal_kind {
                return Err(CallToolResult::error(
                    "Workspace resolver returned a personal source instead of a legacy binding",
                ));
            }
            let project = uuid(args, "project_id")?;
            if personal_kind && project.is_some() {
                return Err(CallToolResult::error(
                    "Personal Flow does not accept a destination project_id",
                ));
            }
            let workspace = if personal_kind {
                None
            } else {
                workspace.or_else(|| info.get("workspace_id").and_then(Value::as_str).map(str::to_owned))
            };
            (Some(id), workspace, project)
        }
    };
    if let Some(parent_id) = parent_id.as_deref() {
        let parent = match parent_resource {
            Some(value) => value,
            None => data(client, &format!("{base}/objects/{parent_id}")).await?,
        };
        if parent.get("scope_id").and_then(Value::as_str) != scope.as_deref() {
            return Err(CallToolResult::error(
                "Parent does not belong to the selected canonical scope",
            ));
        }
        let actual = resource_project(&parent)?;
        if project.is_some() && project != actual {
            return Err(CallToolResult::error("project_id does not match the parent resource"));
        }
        project = actual;
    }
    if project.is_some() && args.get("unprojected").and_then(Value::as_bool) == Some(true) {
        return Err(CallToolResult::error(
            "project_id and unprojected are mutually exclusive",
        ));
    }
    if name == "objects.search"
        && workspace.is_some()
        && project.is_none()
        && args.get("unprojected").and_then(Value::as_bool) != Some(true)
    {
        return Err(CallToolResult::error(
            "Legacy search requires project_id or unprojected:true; project policy cannot be bypassed by an all-project search",
        ));
    }
    let mut query =
        reqwest::Url::parse("http://flow.invalid").map_err(|_| CallToolResult::error("Cannot construct Flow query"))?;
    {
        let mut q = query.query_pairs_mut();
        for key in [
            "q",
            "cursor",
            "freshness",
            "limit",
            "before_seq",
            "at_seq",
            "render",
            "unprojected",
            "all_visible",
            "include_archived",
        ] {
            if let Some(value) = args.get(key) {
                q.append_pair(key, &value.as_str().map_or_else(|| value.to_string(), str::to_owned));
            }
        }
        if let Some(project) = project.as_deref() {
            q.append_pair("project_id", project);
        }
        if let Some(kind) = args
            .get("type")
            .and_then(Value::as_str)
            .filter(|_| name == "objects.query")
        {
            q.append_pair("object_type", kind);
        }
    }
    let suffix = query
        .query()
        .filter(|value| !value.is_empty())
        .map(|value| format!("?{value}"))
        .unwrap_or_default();
    let object = object_id.as_deref();
    let (path, body) = match name {
        "objects.get" => (
            format!(
                "{base}/objects/{}{suffix}",
                object.ok_or_else(|| CallToolResult::error("object_id is required"))?
            ),
            None,
        ),
        "objects.history" => (
            format!(
                "{base}/objects/{}/history{suffix}",
                object.ok_or_else(|| CallToolResult::error("object_id is required"))?
            ),
            None,
        ),
        "objects.query" | "objects.search" => {
            let tail = if name == "objects.query" { "objects" } else { "search" };
            let path = format!("{base}/scopes/{}/{tail}{suffix}", scope.as_deref().unwrap_or_default());
            (path, None)
        }
        "objects.create" => {
            let key = args
                .get("idempotency_key")
                .and_then(Value::as_str)
                .filter(|value| !value.is_empty())
                .ok_or_else(|| CallToolResult::error("idempotency_key is required"))?;
            let kind = args
                .get("type")
                .and_then(Value::as_str)
                .filter(|value| matches!(*value, "page" | "navigator" | "collection"))
                .ok_or_else(|| {
                    CallToolResult::error("type must be page, navigator, or collection; Record creation is typed")
                })?;
            let title = args
                .get("title")
                .and_then(Value::as_str)
                .ok_or_else(|| CallToolResult::error("title is required"))?;
            let mut body = json!({"object_type":kind,"title":title,"idempotency_key":key});
            for key in ["initial_fields", "initial_view", "message"] {
                if let Some(value) = args.get(key) {
                    set_field(&mut body, key, value.clone());
                }
            }
            if let Some(parent) = uuid(args, "parent_id")? {
                set_field(&mut body, "parent_object_id", json!(parent));
            }
            if let Some(project) = project.as_deref() {
                set_field(&mut body, "project_id", json!(project));
            }
            set_field(&mut body, "scope_id", json!(scope));
            (format!("{base}/objects"), Some(body))
        }
        "collections.describe" => (
            format!("{base}/collections/{}{suffix}", object.unwrap_or_default()),
            None,
        ),
        "collections.query" | "records.create" => {
            let mut body = args.clone();
            for key in ["collection_id", "scope_id", "workspace_id", "project_id", "personal"] {
                body.as_object_mut().map(|value| value.remove(key));
            }
            let tail = if name == "collections.query" {
                "query"
            } else {
                "records"
            };
            (
                format!("{base}/collections/{}/{tail}", object.unwrap_or_default()),
                Some(body),
            )
        }
        "objects.export" => {
            let format = args
                .get("format")
                .and_then(Value::as_str)
                .filter(|value| matches!(*value, "json" | "markdown" | "csv"))
                .ok_or_else(|| {
                    CallToolResult::error(
                        "Bounded current-head export supports json, markdown, csv; package/history are not advertised",
                    )
                })?;
            let body = json!({"format":format,"include_history":false,"idempotency_key":args.get("idempotency_key")});
            (
                format!("{base}/objects/{}/exports", object.unwrap_or_default()),
                Some(body),
            )
        }
        "objects.command" => {
            let command = args
                .get("command")
                .filter(|value| value.is_object())
                .ok_or_else(|| CallToolResult::error("command is required"))?;
            let kind = command
                .get("type")
                .and_then(Value::as_str)
                .ok_or_else(|| CallToolResult::error("command.type is required"))?;
            if !COMMANDS.contains(&kind) {
                return Err(CallToolResult::error("Raw collaborative payloads are not MCP commands"));
            }
            if matches!(kind, "link" | "unlink") && args.get("expected_frontier").is_some() {
                return Err(CallToolResult::error(
                    "Metadata-only relations reject expected_frontier; they do not advance the document head",
                ));
            }
            if resource_type.as_deref() == Some("record") && matches!(kind, "record_patch" | "record_archive") {
                if !record_context_supported {
                    return Err(CallToolResult::error(
                        "Record-only typed commands require the current human Record-context service",
                    ));
                }
                let payload = command
                    .get("payload")
                    .ok_or_else(|| CallToolResult::error("Record payload is required"))?;
                if let Some(record) = uuid(payload, "record_id")?
                    && Some(record.as_str()) != object
                {
                    return Err(CallToolResult::error("record_id must equal the actual Record target"));
                }
                let schema_ok = payload
                    .get("schema_revision")
                    .and_then(Value::as_str)
                    .is_some_and(|token| !token.is_empty());
                let frontier_ok = args
                    .get("expected_frontier")
                    .and_then(Value::as_str)
                    .is_some_and(|value| !value.is_empty());
                if !schema_ok || !frontier_ok {
                    return Err(CallToolResult::error(
                        "Record-only commands require captured schema_revision and Record expected_frontier",
                    ));
                }
            }
            if matches!(
                kind,
                "insert_body_asset"
                    | "replace_body_asset"
                    | "link_child_page"
                    | "prepare_child_page"
                    | "upload_body_asset"
            ) && resource_type.as_deref() != Some("page")
            {
                return Err(CallToolResult::error(
                    "body asset/child commands require an actual Page",
                ));
            }
            if kind == "create_collection_embed" && resource_type.as_deref() != Some("page") {
                return Err(CallToolResult::error(
                    "Atomic Collection embedding requires an actual Page source",
                ));
            }
            let valid_initialization = resource_type.as_deref() == Some("page")
                && command
                    .get("payload")
                    .and_then(Value::as_object)
                    .is_some_and(|payload| {
                        payload.is_empty()
                            || (payload.len() == 1
                                && payload
                                    .get("model")
                                    .and_then(Value::as_str)
                                    .is_some_and(|model| matches!(model, "rich_page_v4" | "rich_page_v5")))
                    })
                && args
                    .get("expected_frontier")
                    .and_then(Value::as_str)
                    .is_some_and(|value| !value.is_empty());
            if kind == "initialize_editor_doc" && !valid_initialization {
                return Err(CallToolResult::error(
                    "Editor initialization requires a Page, empty payload or supported model selector and captured expected_frontier; client content is not accepted",
                ));
            }
            if matches!(resource_type.as_deref(), Some("collection" | "record"))
                && matches!(
                    kind,
                    "semantic_patch" | "insert_block" | "update_block" | "delete_block" | "move_block"
                )
            {
                return Err(CallToolResult::error(
                    "Collection/Record operations require typed server commands",
                ));
            }
            let key = args
                .get("idempotency_key")
                .and_then(Value::as_str)
                .filter(|value| !value.is_empty())
                .ok_or_else(|| CallToolResult::error("idempotency_key is required"))?;
            let mut body = json!({"command":command,"idempotency_key":key});
            for key in ["expected_frontier", "message"] {
                if let Some(value) = args.get(key) {
                    set_field(&mut body, key, value.clone());
                }
            }
            (
                format!(
                    "{base}/objects/{}/commands",
                    object.ok_or_else(|| CallToolResult::error("object_id is required"))?
                ),
                Some(body),
            )
        }
        _ => return Err(CallToolResult::error("Tool is not registered in Flow-only mode")),
    };
    let mut policy_projects = std::collections::BTreeSet::new();
    if let Some(project) = project {
        policy_projects.insert(project);
    }
    for project in affected_project_ids(
        client,
        name,
        args,
        AffectedContext {
            base,
            scope: scope.as_deref(),
            workspace: workspace.as_deref(),
            source: object,
            record_target: resource_type.as_deref() == Some("record"),
        },
    )
    .await?
    {
        policy_projects.insert(project);
    }
    let mut path = path;
    let mut body = body;
    let idempotency_key = if name == "objects.command"
        && args
            .pointer("/command/type")
            .and_then(Value::as_str)
            .is_some_and(|kind| matches!(kind, "prepare_child_page" | "upload_body_asset"))
    {
        if resource_type.as_deref() != Some("page") {
            return Err(CallToolResult::error(
                "Page preparation/uploads require a user JWT and actual Page source",
            ));
        }
        let kind = args
            .pointer("/command/type")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let source = object.ok_or_else(|| CallToolResult::error("object_id is required"))?;
        path = format!(
            "/api/v2/flow/objects/{source}/{}",
            if kind == "prepare_child_page" {
                "child-reservations"
            } else {
                "body-assets/base64"
            }
        );
        body = Some(
            args.pointer("/command/payload")
                .cloned()
                .ok_or_else(|| CallToolResult::error("command.payload is required"))?,
        );
        args.get("idempotency_key").and_then(Value::as_str).map(str::to_owned)
    } else {
        None
    };
    let method = if body.is_some() {
        reqwest::Method::POST
    } else {
        reqwest::Method::GET
    };
    Ok(Request {
        path,
        body,
        policy_projects: policy_projects.into_iter().collect(),
        policy_tools: operation_policy_tools(name, args),
        method,
        idempotency_key,
        form_policy_args: Vec::new(),
        workspace_id: workspace,
        object_type: resource_type,
        record_context_supported,
    })
}

fn operation_policy_tools(name: &str, args: &Value) -> Vec<String> {
    let mut tools = vec![name.to_owned()];
    if name == "objects.command" {
        let baseline = match args.pointer("/command/type").and_then(Value::as_str) {
            Some("link") => "objects.link",
            Some("unlink") => "objects.unlink",
            Some("move_object") => "objects.move",
            _ => "objects.patch",
        };
        tools.push(baseline.to_owned());
        if args.pointer("/command/type").and_then(Value::as_str) == Some("create_collection_embed") {
            tools.push("objects.create".to_owned());
        }
    }
    tools
}

/// Only declared domain ownership fields are inspected. User content is never recursively
/// scanned for IDs/project claims, and no caller hint selects a policy subject.
#[derive(Clone, Copy)]
struct AffectedContext<'a> {
    base: &'a str,
    scope: Option<&'a str>,
    workspace: Option<&'a str>,
    source: Option<&'a str>,
    record_target: bool,
}
async fn affected_project_ids(
    client: &OpenPrClient,
    name: &str,
    args: &Value,
    context: AffectedContext<'_>,
) -> Result<Vec<String>, CallToolResult> {
    let AffectedContext {
        base,
        scope,
        workspace,
        source,
        record_target,
    } = context;
    let mut targets = std::collections::BTreeSet::new();
    let command = args.pointer("/command/type").and_then(Value::as_str);
    let payload = args.pointer("/command/payload");
    if name == "objects.command" {
        match command {
            Some("link" | "move_object") => {
                let payload = payload.ok_or_else(|| CallToolResult::error("command.payload is required"))?;
                targets.insert(
                    uuid(payload, "target_object_id")?
                        .ok_or_else(|| CallToolResult::error("target_object_id is required"))?,
                );
                if command == Some("move_object")
                    && let Some(after) = uuid(payload, "after_id")?
                {
                    targets.insert(after);
                }
            }
            Some("unlink") => {
                let payload = payload.ok_or_else(|| CallToolResult::error("command.payload is required"))?;
                let relation =
                    uuid(payload, "relation_id")?.ok_or_else(|| CallToolResult::error("relation_id is required"))?;
                targets.insert(outgoing_relation_target(client, base, source.unwrap_or_default(), &relation).await?);
            }
            Some("record_patch" | "record_archive") => {
                let payload = payload.ok_or_else(|| CallToolResult::error("command.payload is required"))?;
                if record_target {
                    if let Some(record) = uuid(payload, "record_id")?
                        && Some(record.as_str()) != source
                    {
                        return Err(CallToolResult::error("record_id must equal the actual Record target"));
                    }
                } else {
                    targets.insert(
                        uuid(payload, "record_id")?.ok_or_else(|| CallToolResult::error("record_id is required"))?,
                    );
                }
            }
            _ => {}
        }
    }
    // Relation values are object references only when their declared collection field says so.
    // For personal scopes no project policy exists; backend scope/resource authorization remains.
    let values = if name == "records.create" {
        args.get("values_by_field_id")
    } else if name == "objects.command" && command == Some("record_patch") {
        payload.and_then(|p| p.get("properties"))
    } else {
        None
    };
    if workspace.is_some()
        && let Some(values) = values.and_then(Value::as_object).filter(|v| !v.is_empty())
    {
        let schema_path = if record_target {
            format!("{base}/records/{}/context", source.unwrap_or_default())
        } else {
            format!("{base}/collections/{}", source.unwrap_or_default())
        };
        let schema = data(client, &schema_path).await?;
        let fields = schema
            .get("fields")
            .and_then(Value::as_array)
            .ok_or_else(|| CallToolResult::error("Cannot resolve actual collection field types for project policy"))?;
        for (field, value) in values {
            let definition = fields
                .iter()
                .find(|candidate| candidate.get("field_id").and_then(Value::as_str) == Some(field));
            let definition =
                definition.ok_or_else(|| CallToolResult::error("Cannot resolve record field ownership/type"))?;
            let field_type = definition
                .get("type")
                .and_then(Value::as_str)
                .ok_or_else(|| CallToolResult::error("Cannot resolve actual record field type for project policy"))?;
            if !matches!(
                field_type,
                "text" | "number" | "boolean" | "date" | "select" | "multi_select" | "relation"
            ) {
                return Err(CallToolResult::error(
                    "Unsupported record field type for bounded project-policy resolution",
                ));
            }
            if field_type == "relation" {
                for target in value
                    .as_array()
                    .ok_or_else(|| CallToolResult::error("Relation field value must be an array"))?
                {
                    let wrapped = json!({"target":target});
                    targets.insert(
                        uuid(&wrapped, "target")?
                            .ok_or_else(|| CallToolResult::error("Relation target must be an object UUID"))?,
                    );
                }
            }
        }
    }
    if targets.len() > 1000 {
        return Err(CallToolResult::error(
            "Bounded Flow policy resolution supports at most 1000 distinct affected targets",
        ));
    }
    let mut projects = Vec::new();
    for target in targets {
        let resource = data(client, &format!("{base}/objects/{target}")).await?;
        if resource.get("id").and_then(Value::as_str) != Some(target.as_str()) {
            return Err(CallToolResult::error("Target object response identity mismatch"));
        }
        if resource.get("workspace_id").and_then(Value::as_str) != workspace
            || (scope.is_some() && resource.get("scope_id").and_then(Value::as_str) != scope)
        {
            return Err(CallToolResult::error(
                "Affected target does not belong to the actual source scope/workspace",
            ));
        }
        if let Some(project) = resource_project(&resource)? {
            if workspace.is_none() {
                return Err(CallToolResult::error(
                    "Personal target cannot supply a legacy project context",
                ));
            }
            projects.push(project);
        }
    }
    Ok(projects)
}
async fn outgoing_relation_target(
    client: &OpenPrClient,
    base: &str,
    source: &str,
    relation: &str,
) -> Result<String, CallToolResult> {
    let mut cursor = None;
    let mut seen = std::collections::BTreeSet::new();
    // No unrestricted directory or ownership guess: use the source's authorized outgoing rows.
    for _ in 0..100 {
        let mut query = reqwest::Url::parse("http://flow.invalid")
            .map_err(|_| CallToolResult::error("Cannot construct relation query"))?;
        {
            let mut pairs = query.query_pairs_mut();
            pairs.append_pair("direction", "outgoing").append_pair("limit", "100");
            if let Some(cursor) = cursor.as_deref() {
                pairs.append_pair("cursor", cursor);
            }
        }
        let page = data(
            client,
            &format!(
                "{base}/objects/{source}/relations?{}",
                query.query().unwrap_or_default()
            ),
        )
        .await?;
        let items = page
            .get("items")
            .and_then(Value::as_array)
            .ok_or_else(|| CallToolResult::error("Cannot resolve relation target for project policy"))?;
        for row in items {
            if row.get("relation_id").and_then(Value::as_str) == Some(relation) {
                if row.get("visibility").and_then(Value::as_str) != Some("visible")
                    || row.get("direction").and_then(Value::as_str) != Some("outgoing")
                {
                    return Err(CallToolResult::error(
                        "Relation target is unavailable for policy resolution",
                    ));
                }
                let other = row
                    .get("other_object")
                    .ok_or_else(|| CallToolResult::error("Relation target identity missing"))?;
                return uuid(other, "id")?.ok_or_else(|| CallToolResult::error("Relation target identity missing"));
            }
        }
        let Some(next) = page
            .get("next_cursor")
            .and_then(Value::as_str)
            .filter(|value| !value.is_empty())
        else {
            return Err(CallToolResult::error(
                "Relation target could not be resolved; no mutation sent",
            ));
        };
        if !seen.insert(next.to_owned()) {
            return Err(CallToolResult::error("Relation policy lookup cursor repeated"));
        }
        cursor = Some(next.to_owned());
    }
    Err(CallToolResult::error(
        "Relation policy lookup exceeded its bounded page limit; no mutation sent",
    ))
}

pub async fn execute(client: &OpenPrClient, request: Request) -> CallToolResult {
    respond_data(
        request_structured(
            client,
            &request.path,
            request.method,
            request.body.as_ref(),
            request.idempotency_key.as_deref(),
        )
        .await,
    )
}

#[cfg(test)]
mod page_v5_schema_tests {
    use super::*;
    #[test]
    fn page_v5_typed_commands_are_in_existing_tool_schema_without_remote_create_alias() {
        let definition = definitions()
            .into_iter()
            .find(|definition| definition.name == "objects.command")
            .unwrap();
        let kinds = definition
            .input_schema
            .pointer("/properties/command/properties/type/enum")
            .unwrap()
            .as_array()
            .unwrap();
        for kind in [
            "insert_body_asset",
            "replace_body_asset",
            "link_child_page",
            "prepare_child_page",
            "upload_body_asset",
        ] {
            assert!(kinds.iter().any(|value| value == kind));
        }
        assert!(!kinds.iter().any(|value| value == "create_child_page"));
        validate_arguments("objects.command",&json!({"object_id":"11111111-1111-4111-8111-111111111111","command":{"type":"link_child_page","payload":{"block_id":"child","index":1,"child_page_id":"33333333-3333-4333-8333-333333333333"}},"idempotency_key":"typed-child"})).unwrap();
    }
}
