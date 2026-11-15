//! Scope-native transport adapters for actual existing registry names. Domain validation and
//! resource authorization stay in the same API services; ownership/policy reuse Flow-only core.
use super::flow_scope::{self, Request};
use crate::{
    client::OpenPrClient,
    protocol::{CallToolResult, ToolDefinition},
};
use reqwest::Method;
use serde_json::{Value, json};

pub const READY: &[&str] = &[
    "objects.patch",
    "objects.move",
    "objects.link",
    "objects.unlink",
    "objects.diff",
    "objects.grants_get",
    "objects.grants_set",
    "objects.inheritance_set",
    "objects.relations",
    "objects.reference",
    "objects.unreference",
    "collab.projection_lag",
    "flow.comments.list",
    "flow.comments.create",
    "flow.comments.update",
    "flow.comments.delete",
    "flow.comments.resolve",
    "flow.shares.list",
    "flow.shares.create",
    "flow.shares.revoke",
    "flow.appearance.set",
    "flow.appearance.upload",
    "flow.trash.list",
    "flow.mentions.list",
    "objects.purge",
];
pub fn supports(name: &str) -> bool {
    READY.contains(&name)
}
fn set(value: &mut Value, key: &str, item: Value) {
    if let Some(map) = value.as_object_mut() {
        map.insert(key.to_owned(), item);
    }
}
pub fn definitions() -> Vec<ToolDefinition> {
    super::get_all_tool_definitions().into_iter().filter(|tool|supports(&tool.name)).map(|mut tool|{
  let selector=matches!(tool.name.as_str(),"flow.trash.list"|"collab.projection_lag");
  if let Some(properties)=tool.input_schema.get_mut("properties"){
   for key in ["scope_id","workspace_id","project_id"]{if key=="project_id"&&tool.name=="flow.trash.list"{continue;}set(properties,key,json!({"type":"string","format":"uuid"}));}
   if selector {set(properties,"personal",json!({"const":true}));}
   if matches!(tool.name.as_str(),"flow.comments.create"|"flow.comments.update"|"flow.comments.delete"|"flow.comments.resolve"|"flow.appearance.set"|"flow.appearance.upload"|"objects.purge"){
    set(properties,"idempotency_key",json!({"type":"string","minLength":1,"maxLength":128,"description":"Optional stable retry intent key"}));
   }
  }
  if selector {
   if let Some(required)=tool.input_schema.get_mut("required").and_then(Value::as_array_mut){required.retain(|key|key.as_str()!=Some("workspace_id"));}
   set(&mut tool.input_schema,"oneOf",json!([
    {"required":["personal"],"not":{"anyOf":[{"required":["scope_id"]},{"required":["workspace_id"]},{"required":["project_id"]}]}},
    {"required":["scope_id"],"not":{"anyOf":[{"required":["personal"]},{"required":["workspace_id"]}]}},
    {"required":["workspace_id"],"not":{"anyOf":[{"required":["personal"]},{"required":["scope_id"]}]}}
   ]));
  }
  if tool.name=="flow.comments.create"{tool.description=tool.description.replace("only workspace members are notified.","only eligible resource participants are notified.");}
  if tool.name=="flow.mentions.list"{tool.description="List bounded eligible people for mentions on this object. Resource visibility and recipient policy apply; this is not a global directory.".into();}
  if tool.name=="objects.move"{tool.description="Move an eligible Page/Collection within its canonical scope, preserving typed Record association. Source management and target edit are required; cross-scope ownership changes are refused. Genuine legacy workspace uses its original move service.".into();}
  if tool.name=="collab.projection_lag"{tool.description="Read actual policy-filtered projection lag. Personal scope accepts cursor and limit1..100, has no project filter, and enforces the published scan budget; genuine legacy workspace retains its original query contract.".into();}
  if tool.name=="objects.diff"{tool.description="Read a bounded semantic diff at retained sequences under current authorization and field secrecy. No raw update bytes are returned; stable editor block anchors may be container IDs.".into();}
  if tool.name=="flow.trash.list"{tool.description="List archived objects the caller can view. Personal scope supports cursor and limit1..100 with explicit scan-budget refusal; genuine legacy workspace retains its original query contract.".into();}
  tool.description=format!("{} Scope-native Flow profile: canonical object/scope authority; real-workspace OPR compatibility only. Personal recipients/grants follow current resource eligibility, not a global directory. Backend gates remain authoritative.",tool.description);
  if tool.name=="objects.reference"{tool.description.push_str(" The actual destination Form/record project policy is checked independently of the source scope.");}
  if tool.name=="objects.unreference"{tool.description.push_str(" Source-Edit link cleanup does not read or mutate the destination Form.");}
  tool
 }).collect()
}
fn context(args: &Value, source_key: &str) -> Value {
    let mut result = json!({});
    for key in ["scope_id", "workspace_id", "project_id"] {
        if let Some(value) = args.get(key) {
            set(&mut result, key, value.clone());
        }
    }
    if let Some(id) = args.get(source_key) {
        set(&mut result, "object_id", id.clone());
    }
    result
}
fn copied(args: &Value, keys: &[&str]) -> Value {
    let mut result = json!({});
    for key in keys {
        if let Some(value) = args.get(*key) {
            set(&mut result, key, value.clone());
        }
    }
    result
}
fn required_uuid(args: &Value, key: &str) -> Result<String, CallToolResult> {
    flow_scope::uuid(args, key)?.ok_or_else(|| CallToolResult::error(format!("{key} is required")))
}
fn key(args: &Value) -> Result<Option<String>, CallToolResult> {
    args.get("idempotency_key")
        .map(|value| {
            value
                .as_str()
                .filter(|key| !key.is_empty() && key.len() <= 128)
                .map(str::to_owned)
                .ok_or_else(|| CallToolResult::error("idempotency_key must be 1..128 characters"))
        })
        .transpose()
}
fn query(path: &str, args: &Value, keys: &[&str]) -> Result<String, CallToolResult> {
    let mut url =
        reqwest::Url::parse("http://flow.invalid").map_err(|_| CallToolResult::error("Cannot build Flow query"))?;
    {
        let mut pairs = url.query_pairs_mut();
        for key in keys {
            if let Some(value) = args.get(*key) {
                pairs.append_pair(key, &value.as_str().map_or_else(|| value.to_string(), str::to_owned));
            }
        }
    }
    Ok(format!(
        "{path}{}",
        url.query()
            .filter(|value| !value.is_empty())
            .map(|value| format!("?{value}"))
            .unwrap_or_default()
    ))
}
fn defaults(mut body: Value, keys: &[(&str, Value)]) -> Value {
    for (key, value) in keys {
        if body.get(*key).is_none() {
            set(&mut body, key, value.clone());
        }
    }
    body
}

pub async fn prepare(client: &OpenPrClient, name: &str, args: &Value) -> Result<Request, CallToolResult> {
    flow_scope::validate_arguments(name, args)?;
    if matches!(name, "flow.trash.list" | "collab.projection_lag") {
        let selected = copied(args, &["personal", "scope_id", "workspace_id", "project_id"]);
        if name == "flow.trash.list" && selected.get("project_id").is_some() {
            return Err(CallToolResult::error(
                "Trash is a scope list and does not accept project_id",
            ));
        }
        // Query's scope selector resolves identity lazily on this call, without tools/list side effects.
        let mut request = flow_scope::prepare_core(client, "objects.query", &selected).await?;
        let prefix = if let Some(workspace) = request.legacy_workspace_id.as_deref() {
            format!("/api/v1/workspaces/{workspace}/flow")
        } else {
            if args.get("project_id").is_some() {
                return Err(CallToolResult::error("Personal navigation has no project partition"));
            }
            request
                .path
                .split('?')
                .next()
                .and_then(|path| path.strip_suffix("/objects"))
                .ok_or_else(|| CallToolResult::error("Cannot resolve canonical scope path"))?
                .to_owned()
        };
        request.path = query(
            &format!(
                "{prefix}/{}",
                if name == "flow.trash.list" {
                    "trash"
                } else {
                    "projection-lag"
                }
            ),
            args,
            &["project_id", "cursor", "limit"],
        )?;
        request.policy_tools = vec![name.to_owned()];
        return Ok(request);
    }
    let source_key = if matches!(name, "objects.link" | "objects.unlink") {
        "source_object_id"
    } else {
        "object_id"
    };
    required_uuid(args, source_key)?;
    let mut selected = context(args, source_key);
    if matches!(
        name,
        "objects.patch" | "objects.move" | "objects.link" | "objects.unlink"
    ) {
        let (kind, payload) = match name {
            "objects.patch" => ("semantic_patch", copied(args, &["operations"])),
            "objects.move" => (
                "move_object",
                defaults(
                    copied(
                        args,
                        &[
                            "target_object_id",
                            "after_id",
                            "expected_target_frontier",
                            "confirm_self_lockout",
                        ],
                    ),
                    &[("confirm_self_lockout", json!(false))],
                ),
            ),
            "objects.link" => (
                "link",
                defaults(
                    copied(args, &["target_object_id", "relation_type", "properties"]),
                    &[("properties", json!({}))],
                ),
            ),
            _ => ("unlink", copied(args, &["relation_id"])),
        };
        set(&mut selected, "command", json!({"type":kind,"payload":payload}));
        for field in ["idempotency_key", "message", "expected_frontier"] {
            if let Some(value) = args.get(field) {
                set(&mut selected, field, value.clone());
            }
        }
        let mut request = flow_scope::prepare_core(client, "objects.command", &selected).await?;
        request.policy_tools.retain(|tool| tool != "objects.command");
        if !request.policy_tools.iter().any(|tool| tool == name) {
            request.policy_tools.push(name.to_owned());
        }
        return Ok(request);
    }
    let mut request = flow_scope::prepare_core(client, "objects.get", &selected).await?;
    request.policy_tools = vec![name.to_owned()];
    let object_path = request.path.clone();
    let mut method = Method::GET;
    let mut body = None;
    let mut intent = None;
    request.path = match name {
        "objects.diff" => query(&format!("{object_path}/diff"), args, &["from_seq", "to_seq", "render"])?,
        "objects.relations" => query(
            &format!("{object_path}/relations"),
            args,
            &["direction", "relation_type", "cursor", "limit"],
        )?,
        "objects.grants_get" => format!("{object_path}/grants"),
        "objects.grants_set" | "objects.inheritance_set" => {
            method = Method::PUT;
            let keys = if name == "objects.grants_set" {
                &[
                    "grants",
                    "confirm_self_lockout",
                    "dry_run",
                    "idempotency_key",
                    "message",
                ][..]
            } else {
                &[
                    "inherit_from_parent",
                    "initial_grants",
                    "confirm_self_lockout",
                    "dry_run",
                    "idempotency_key",
                    "message",
                ][..]
            };
            body = Some(defaults(
                copied(args, keys),
                &[("confirm_self_lockout", json!(false)), ("dry_run", json!(false))],
            ));
            format!(
                "{object_path}/{}",
                if name == "objects.grants_set" {
                    "grants"
                } else {
                    "inheritance"
                }
            )
        }
        "objects.reference" => {
            method = Method::POST;
            let target = required_uuid(args, "target_id")?;
            let kind = args
                .get("target_type")
                .and_then(Value::as_str)
                .ok_or_else(|| CallToolResult::error("target_type is required"))?;
            request.form_policy_args.push(match kind {
                "form" => json!({"form_id":target}),
                "form_record" => json!({"record_id":target}),
                _ => return Err(CallToolResult::error("target_type must be form or form_record")),
            });
            body = Some(defaults(
                copied(args, &["target_type", "target_id", "display", "idempotency_key"]),
                &[("display", json!({}))],
            ));
            format!("{object_path}/references")
        }
        "objects.unreference" => {
            method = Method::DELETE;
            intent = key(args)?;
            // Removing the link mutates only source metadata; retain baseline cleanup when target is unavailable.
            format!("{object_path}/references/{}", required_uuid(args, "reference_id")?)
        }
        "flow.comments.list" => format!("{object_path}/comments"),
        "flow.comments.create" => {
            method = Method::POST;
            intent = key(args)?;
            if args.get("parent_id").is_some() {
                required_uuid(args, "parent_id")?;
            }
            body = Some(copied(args, &["body", "block_id", "parent_id", "mentions"]));
            format!("{object_path}/comments")
        }
        "flow.comments.update" => {
            method = Method::PATCH;
            intent = key(args)?;
            body = Some(copied(args, &["body", "mentions"]));
            format!("{object_path}/comments/{}", required_uuid(args, "flow_comment_id")?)
        }
        "flow.comments.delete" => {
            method = Method::DELETE;
            intent = key(args)?;
            format!("{object_path}/comments/{}", required_uuid(args, "flow_comment_id")?)
        }
        "flow.comments.resolve" => {
            method = Method::POST;
            intent = key(args)?;
            body = Some(copied(args, &["resolved"]));
            format!(
                "{object_path}/comments/{}/resolve",
                required_uuid(args, "flow_comment_id")?
            )
        }
        "flow.shares.list" => format!("{object_path}/share-links"),
        "flow.shares.create" => {
            method = Method::POST;
            body = Some(copied(args, &["expires_at"]));
            format!("{object_path}/share-links")
        }
        "flow.shares.revoke" => {
            method = Method::POST;
            body = Some(json!({}));
            format!("{object_path}/share-links/{}/revoke", required_uuid(args, "link_id")?)
        }
        "flow.appearance.set" => {
            method = Method::PUT;
            intent = key(args)?;
            body = Some(defaults(
                copied(args, &["icon", "cover"]),
                &[("icon", Value::Null), ("cover", Value::Null)],
            ));
            format!("{object_path}/appearance")
        }
        "flow.appearance.upload" => {
            method = Method::POST;
            intent = key(args)?;
            body = Some(copied(args, &["data_base64"]));
            format!("{object_path}/appearance/assets")
        }
        "flow.mentions.list" => query(&format!("{object_path}/mention-candidates"), args, &["q"])?,
        "objects.purge" => {
            method = Method::POST;
            intent = key(args)?;
            body = Some(json!({}));
            format!("{object_path}/purge")
        }
        _ => return Err(CallToolResult::error("Unsupported existing Flow adapter")),
    };
    request.method = method;
    request.body = body;
    request.idempotency_key = intent;
    Ok(request)
}

/// Existing atomic create/embed variant: the actual Page is authoritative, not a home/root.
pub async fn prepare_embed(client: &OpenPrClient, args: &Value) -> Result<Request, CallToolResult> {
    flow_scope::validate_arguments("objects.create", args)?;
    if args.get("type").and_then(Value::as_str) != Some("collection")
        || args.get("parent_id").is_some()
        || args.get("personal").is_some()
    {
        return Err(CallToolResult::error(
            "embed_page_id requires type=collection and cannot be combined with parent_id or personal/home selection",
        ));
    }
    let page = required_uuid(args, "embed_page_id")?;
    let mut selected = context(args, "embed_page_id");
    set(&mut selected, "object_id", json!(page));
    let payload = copied(args, &["title", "initial_fields", "initial_view", "after_block_id"]);
    set(
        &mut selected,
        "command",
        json!({"type":"create_collection_embed","payload":payload}),
    );
    for field in ["idempotency_key", "message"] {
        if let Some(value) = args.get(field) {
            set(&mut selected, field, value.clone());
        }
    }
    let mut request = flow_scope::prepare_core(client, "objects.command", &selected).await?;
    request.policy_tools.retain(|tool| tool != "objects.command");
    Ok(request)
}

/// The existing schema tool's Record alternative never queries its parent Collection.
pub async fn prepare_record_context(client: &OpenPrClient, args: &Value) -> Result<Request, CallToolResult> {
    flow_scope::validate_arguments("collections.describe", args)?;
    if client.legacy_bot_token_shape() || args.get("collection_id").is_some() || args.get("at_seq").is_some() {
        return Err(CallToolResult::error(
            "Record-only context requires current human identity and record_id alone; no parent/history schema option",
        ));
    }
    let record = required_uuid(args, "record_id")?;
    let mut selected = copied(args, &["scope_id", "workspace_id", "project_id"]);
    set(&mut selected, "object_id", json!(record));
    let mut request = flow_scope::prepare_core(client, "objects.get", &selected).await?;
    if request.object_type.as_deref() != Some("record") || !request.record_context_supported {
        return Err(CallToolResult::error(
            "This server does not advertise bounded Record-only context support",
        ));
    }
    request.path = format!("/api/v2/flow/records/{record}/context");
    request.policy_tools = vec!["collections.describe".to_owned()];
    Ok(request)
}
