//! Links between work items and Flow pages: `work_items.link_page`, `work_items.unlink_page`,
//! `work_items.list_pages` and, from the page side, `objects.list_work_items`.
//!
//! Who may do what is the API's decision. Linking needs edit rights on the work item and at least
//! view rights on the page, unlinking needs edit rights on the work item, and an item holds at most
//! 50 links. A refusal (page not found, only a Flow page can be linked, the limit of 50) reaches the
//! caller as the tool's error.
//!
//! A link to a page the caller may not open is `restricted`: the API sends `title: null` and the
//! tools say so in a `display` text instead of leaving the caller to guess. A title is never made up.

use super::objects::{StructuredApiError, get_structured, respond_data};
use crate::client::{OpenPrClient, encode_query_component};
use crate::protocol::{CallToolResult, ToolDefinition};
use serde::Deserialize;
use serde_json::{Value, json};

const UUID_PATTERN: &str = "^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$";

/// What a restricted link reads as. There is no title to show: the caller has no access to the page.
const RESTRICTED_DISPLAY: &str = "Restricted page (you have no access to this page, so its title is not shown)";
const RESTRICTED_NOTE: &str = "Links with access \"restricted\" point at pages you cannot open: they have no title, \
and it is not shown on purpose. Do not guess it.";
const MAX_LINKS: u32 = 50;

fn id_property(description: &str) -> Value {
    json!({ "type": "string", "description": description, "pattern": UUID_PATTERN })
}

fn pretty(value: &Value) -> CallToolResult {
    CallToolResult::success(serde_json::to_string_pretty(value).unwrap_or_default())
}

/// The text that stands for a link's page: its title, or a plain statement that there is none.
fn display_of(link: &Value) -> String {
    let archived = link.get("lifecycle").and_then(Value::as_str) == Some("archived");
    let mut text = match (
        link.get("access").and_then(Value::as_str),
        link.get("title").and_then(Value::as_str),
    ) {
        (Some("visible"), Some(title)) => title.to_string(),
        _ => RESTRICTED_DISPLAY.to_string(),
    };
    if archived {
        text.push_str(" [in trash]");
    }
    text
}

/// Adds `display` to every link of `links` and returns how many of them are restricted. Anything
/// that is not an array of objects is left as it is.
fn annotate_links(links: &mut Value) -> usize {
    let Some(rows) = links.as_array_mut() else {
        return 0;
    };
    let mut restricted = 0;
    for link in rows {
        let display = display_of(link);
        let is_restricted = link.get("access").and_then(Value::as_str) != Some("visible");
        if let Some(object) = link.as_object_mut() {
            object.insert("display".to_string(), json!(display));
            restricted += usize::from(is_restricted);
        }
    }
    restricted
}

/// Annotates the `page_links` of a single work item answer (`{code, message, data}`) in place.
pub fn annotate_work_item_answer(answer: &mut Value) {
    let Some(data) = answer.get_mut("data").and_then(Value::as_object_mut) else {
        return;
    };
    let Some(links) = data.get_mut("page_links") else {
        return;
    };
    if annotate_links(links) > 0 {
        data.insert("page_links_note".to_string(), json!(RESTRICTED_NOTE));
    }
}

// ---- work_items.link_page ----

pub fn link_page_tool() -> ToolDefinition {
    ToolDefinition {
        annotations: None,
        name: "work_items.link_page".to_string(),
        description: format!(
            "Link a Flow page to a work item. Needs edit rights on the work item and view rights on the page; \
only a Flow page of the work item's workspace (or one mounted into it) can be linked, and a work item holds at \
most {MAX_LINKS} links. Linking a pair that is already linked changes nothing. A page you cannot view answers \
\"page not found\"."
        ),
        input_schema: json!({
            "type": "object",
            "properties": {
                "work_item_id": id_property("UUID of the work item"),
                "object_id": id_property("UUID of the Flow page")
            },
            "required": ["work_item_id", "object_id"],
            "additionalProperties": false
        }),
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct LinkPageInput {
    work_item_id: String,
    object_id: String,
}

pub async fn link_page(client: &OpenPrClient, args: Value) -> CallToolResult {
    let input: LinkPageInput = match serde_json::from_value(args) {
        Ok(i) => i,
        Err(e) => return CallToolResult::error(format!("Invalid input: {e}")),
    };
    match client.link_issue_page(&input.work_item_id, &input.object_id).await {
        Ok(mut answer) => {
            if let Some(link) = answer.get_mut("data") {
                let mut one = Value::Array(vec![link.take()]);
                annotate_links(&mut one);
                if let Some(annotated) = one.as_array_mut().and_then(Vec::pop) {
                    *link = annotated;
                }
            }
            pretty(&answer)
        }
        Err(e) => CallToolResult::error(e),
    }
}

// ---- work_items.unlink_page ----

pub fn unlink_page_tool() -> ToolDefinition {
    ToolDefinition {
        annotations: None,
        name: "work_items.unlink_page".to_string(),
        description: "Remove the link between a work item and a Flow page. Needs edit rights on the work item \
only. Unlinking a page that is not linked changes nothing. The page itself is not touched."
            .to_string(),
        input_schema: json!({
            "type": "object",
            "properties": {
                "work_item_id": id_property("UUID of the work item"),
                "object_id": id_property("UUID of the Flow page")
            },
            "required": ["work_item_id", "object_id"],
            "additionalProperties": false
        }),
    }
}

pub async fn unlink_page(client: &OpenPrClient, args: Value) -> CallToolResult {
    let input: LinkPageInput = match serde_json::from_value(args) {
        Ok(i) => i,
        Err(e) => return CallToolResult::error(format!("Invalid input: {e}")),
    };
    match client.unlink_issue_page(&input.work_item_id, &input.object_id).await {
        Ok(_) => CallToolResult::success("The page is not linked to the work item"),
        Err(e) => CallToolResult::error(e),
    }
}

// ---- work_items.list_pages ----

pub fn list_pages_tool() -> ToolDefinition {
    ToolDefinition {
        annotations: None,
        name: "work_items.list_pages".to_string(),
        description: format!(
            "List the Flow pages linked to a work item (at most {MAX_LINKS}). Each link has object_id, title, \
access (visible or restricted), lifecycle (active or archived, archived = in the trash), linked_at and a plain \
`display` text. A restricted link has title null: you may not open that page, and its title is not shown."
        ),
        input_schema: json!({
            "type": "object",
            "properties": { "work_item_id": id_property("UUID of the work item") },
            "required": ["work_item_id"],
            "additionalProperties": false
        }),
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct WorkItemInput {
    work_item_id: String,
}

pub async fn list_pages(client: &OpenPrClient, args: Value) -> CallToolResult {
    let input: WorkItemInput = match serde_json::from_value(args) {
        Ok(i) => i,
        Err(e) => return CallToolResult::error(format!("Invalid input: {e}")),
    };
    let mut links = match client.list_issue_pages(&input.work_item_id).await {
        Ok(mut answer) => answer.get_mut("data").map_or(Value::Null, Value::take),
        Err(e) => return CallToolResult::error(e),
    };
    let restricted = annotate_links(&mut links);
    let total = links.as_array().map_or(0, Vec::len);
    let mut out = json!({ "page_links": links, "count": total, "restricted_count": restricted });
    if restricted > 0
        && let Some(object) = out.as_object_mut()
    {
        object.insert("note".to_string(), json!(RESTRICTED_NOTE));
    }
    pretty(&out)
}

// ---- objects.list_work_items ----

pub fn list_work_items_of_page_tool() -> ToolDefinition {
    ToolDefinition {
        annotations: None,
        name: "objects.list_work_items".to_string(),
        description: "List the work items linked to a Flow page: id, identifier, title, state and project_id of \
every item you may read. Items you may not read are not listed, only counted in hidden_count; truncated says \
the list is cut short."
            .to_string(),
        input_schema: json!({
            "type": "object",
            "properties": { "object_id": id_property("UUID of the Flow page") },
            "required": ["object_id"],
            "additionalProperties": false
        }),
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ObjectInput {
    object_id: String,
}

/// Adds a plain `note` to the answer when some linked items are not listed.
fn annotate_page_work_items(data: &mut Value) {
    let hidden = data.get("hidden_count").and_then(Value::as_u64).unwrap_or(0);
    let truncated = data.get("truncated").and_then(Value::as_bool).unwrap_or(false);
    let mut parts = Vec::new();
    if hidden > 0 {
        parts.push(format!(
            "{hidden} more work item(s) are linked to this page but you may not read them, so they are not listed."
        ));
    }
    if truncated {
        parts.push("The list is truncated: not every linked work item is shown.".to_string());
    }
    if !parts.is_empty()
        && let Some(object) = data.as_object_mut()
    {
        object.insert("note".to_string(), json!(parts.join(" ")));
    }
}

pub async fn list_work_items_of_page(client: &OpenPrClient, args: Value) -> CallToolResult {
    let input: ObjectInput = match serde_json::from_value(args) {
        Ok(i) => i,
        Err(e) => return CallToolResult::error(format!("Invalid input: {e}")),
    };
    let path = format!(
        "/api/v2/flow/objects/{}/work-items",
        encode_query_component(&input.object_id)
    );
    let answer: Result<Value, StructuredApiError> = get_structured(client, &path).await.map(|mut envelope| {
        if let Some(data) = envelope.get_mut("data") {
            annotate_page_work_items(data);
        }
        envelope
    });
    respond_data(answer)
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::unwrap_used, clippy::indexing_slicing)]
mod tests {
    use super::*;
    use crate::client::test_api;
    use crate::tools::test_support::UUID;
    use axum::{Json, Router, http::StatusCode, routing::get};

    const OTHER: &str = "22222222-2222-4222-8222-222222222222";

    fn links() -> Value {
        json!([
            { "object_id": UUID, "title": "Runbook", "access": "visible", "lifecycle": "active", "linked_at": "t" },
            { "object_id": OTHER, "title": null, "access": "restricted", "lifecycle": "archived", "linked_at": "t" }
        ])
    }

    fn envelope(data: &Value) -> Value {
        json!({ "code": 0, "message": "success", "data": data })
    }

    async fn client_for(router: Router) -> OpenPrClient {
        let url = test_api::spawn(router).await.expect("stand-in api");
        test_api::client(url).expect("client")
    }

    fn text(result: &CallToolResult) -> String {
        serde_json::to_string(&result.content).expect("content")
    }

    fn body_json(result: &CallToolResult) -> Value {
        let content = serde_json::to_value(&result.content).expect("content");
        let raw = content[0]["text"].as_str().expect("text").to_string();
        serde_json::from_str(&raw).expect("json output")
    }

    #[test]
    fn a_restricted_link_says_so_and_never_gets_a_title() {
        let mut rows = links();
        assert_eq!(annotate_links(&mut rows), 1);
        assert_eq!(rows[0]["display"], "Runbook");
        let restricted = rows[1]["display"].as_str().unwrap();
        assert!(restricted.starts_with("Restricted page"), "{restricted}");
        assert!(restricted.ends_with("[in trash]"), "{restricted}");
        assert_eq!(rows[1]["title"], Value::Null);
    }

    #[test]
    fn a_visible_link_without_a_title_is_not_given_one() {
        let mut rows = json!([{ "object_id": UUID, "title": null, "access": "visible", "lifecycle": "active" }]);
        annotate_links(&mut rows);
        assert!(rows[0]["display"].as_str().unwrap().starts_with("Restricted page"));
    }

    #[test]
    fn work_item_answers_keep_page_links_and_flag_restricted_ones() {
        let mut answer = envelope(&json!({ "id": UUID, "page_links": links() }));
        annotate_work_item_answer(&mut answer);
        assert_eq!(answer["data"]["page_links"].as_array().unwrap().len(), 2);
        assert_eq!(answer["data"]["page_links"][0]["object_id"], UUID);
        assert!(answer["data"]["page_links_note"].is_string());

        let mut plain = envelope(&json!({ "id": UUID, "page_links": [links()[0].clone()] }));
        annotate_work_item_answer(&mut plain);
        assert!(plain["data"].get("page_links_note").is_none());

        let mut none = envelope(&json!({ "id": UUID }));
        annotate_work_item_answer(&mut none);
        assert!(none["data"].get("page_links").is_none());
    }

    #[tokio::test]
    async fn work_items_get_passes_page_links_through() {
        let router = Router::new().route(
            "/api/v1/issues/{id}",
            get(|| async { Json(envelope(&json!({ "id": UUID, "page_links": links() }))) }),
        );
        let client = client_for(router).await;
        let result = crate::tools::work_items::get_work_item(&client, json!({ "work_item_id": UUID })).await;
        assert_ne!(result.is_error, Some(true), "{result:?}");
        let out = body_json(&result);
        assert_eq!(out["data"]["page_links"][1]["access"], "restricted");
        assert_eq!(out["data"]["page_links"][1]["title"], Value::Null);
        assert!(out["data"]["page_links"][1]["display"].is_string());
        assert!(out["data"]["page_links_note"].is_string());
    }

    #[tokio::test]
    async fn list_pages_reports_counts_and_the_restricted_note() {
        let router = Router::new().route("/api/v1/issues/{id}/pages", get(|| async { Json(envelope(&links())) }));
        let client = client_for(router).await;
        let result = list_pages(&client, json!({ "work_item_id": UUID })).await;
        let out = body_json(&result);
        assert_eq!(out["count"], 2);
        assert_eq!(out["restricted_count"], 1);
        assert!(out["note"].is_string());
        assert_eq!(out["page_links"][1]["title"], Value::Null);
    }

    #[tokio::test]
    async fn link_and_unlink_send_the_right_requests() {
        let api = crate::tools::test_support::Api::spawn(crate::tools::test_support::Policy::Permissive)
            .await
            .expect("stand-in api");
        let client = api.client().expect("client");
        let linked = link_page(&client, json!({ "work_item_id": UUID, "object_id": OTHER })).await;
        assert_ne!(linked.is_error, Some(true), "{linked:?}");
        let unlinked = unlink_page(&client, json!({ "work_item_id": UUID, "object_id": OTHER })).await;
        assert_ne!(unlinked.is_error, Some(true), "{unlinked:?}");
        let seen = api.seen();
        let post = seen.iter().find(|s| s.method == "POST").expect("a link was sent");
        assert_eq!(post.path, format!("/api/v1/issues/{UUID}/pages"));
        assert_eq!(post.body, json!({ "object_id": OTHER }));
        let delete = seen.iter().find(|s| s.method == "DELETE").expect("an unlink was sent");
        assert_eq!(delete.path, format!("/api/v1/issues/{UUID}/pages/{OTHER}"));
    }

    #[tokio::test]
    async fn api_refusals_reach_the_caller_as_tool_errors() {
        for (code, message) in [
            (404, "page not found"),
            (400, "only a Flow page can be linked"),
            (400, "a work item can link at most 50 pages"),
        ] {
            let router = Router::new().route(
                "/api/v1/issues/{id}/pages",
                axum::routing::post(
                    move || async move { Json(json!({ "code": code, "message": message, "data": null })) },
                ),
            );
            let client = client_for(router).await;
            let result = link_page(&client, json!({ "work_item_id": UUID, "object_id": OTHER })).await;
            assert_eq!(result.is_error, Some(true), "{message}");
            let shown = text(&result);
            assert!(shown.contains(message), "{shown}");
            assert!(shown.contains(&code.to_string()), "{shown}");
        }
    }

    #[tokio::test]
    async fn an_http_error_status_is_a_tool_error_too() {
        let router = Router::new().route(
            "/api/v1/issues/{id}/pages",
            get(|| async { (StatusCode::NOT_FOUND, "no") }),
        );
        let client = client_for(router).await;
        let result = list_pages(&client, json!({ "work_item_id": UUID })).await;
        assert_eq!(result.is_error, Some(true));
    }

    #[tokio::test]
    async fn the_page_side_listing_counts_what_it_hides() {
        let router = Router::new().route(
            "/api/v2/flow/objects/{id}/work-items",
            get(|| async {
                Json(envelope(&json!({
                    "items": [{ "id": UUID, "identifier": "PRX-1", "title": "t", "state": "todo", "project_id": OTHER }],
                    "hidden_count": 3,
                    "truncated": true
                })))
            }),
        );
        let client = client_for(router).await;
        let result = list_work_items_of_page(&client, json!({ "object_id": UUID })).await;
        assert_ne!(result.is_error, Some(true), "{result:?}");
        let out = body_json(&result);
        assert_eq!(out["items"][0]["identifier"], "PRX-1");
        let note = out["note"].as_str().unwrap();
        assert!(note.contains("3 more"), "{note}");
        assert!(note.contains("truncated"), "{note}");
    }

    #[tokio::test]
    async fn the_page_side_listing_reports_a_missing_page() {
        let router = Router::new().route(
            "/api/v2/flow/objects/{id}/work-items",
            get(|| async { Json(json!({ "code": 404, "message": "flow resource not found", "data": null })) }),
        );
        let client = client_for(router).await;
        let result = list_work_items_of_page(&client, json!({ "object_id": UUID })).await;
        assert_eq!(result.is_error, Some(true));
        assert!(text(&result).contains("flow resource not found"));
    }

    #[test]
    fn schemas_refuse_unknown_fields_and_require_ids() {
        for tool in [
            link_page_tool(),
            unlink_page_tool(),
            list_pages_tool(),
            list_work_items_of_page_tool(),
        ] {
            assert_eq!(tool.input_schema["additionalProperties"], false, "{}", tool.name);
            assert!(tool.input_schema["required"].as_array().is_some_and(|r| !r.is_empty()));
        }
    }
}
