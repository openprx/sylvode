//! The search mode entry tools: `sylvode_overview`, `sylvode_search`, `sylvode_read` and
//! `sylvode_write`.
//!
//! In search mode `tools/list` offers these four and the five task tools. Every other tool is
//! found with `sylvode_search` (ranking over the tool catalogue) and called through
//! `sylvode_read` or `sylvode_write`. This module holds the definitions, the ranking, the
//! overview and the checks on a wrapper call; the server dispatches and applies the policy chain.
//!
//! The wrappers cannot reach more than a direct call could: the target must be a catalogue tool
//! of the right access class (so a parked tool is simply unknown), its arguments are validated
//! against its own schema, and the call then runs through the same checks as a direct call.

use std::collections::BTreeSet;
use std::sync::OnceLock;

use platform::config::McpToolMode;
use serde_json::{Value, json};
use uuid::Uuid;

use super::catalog::{self, Access, CATALOG, CATEGORIES};
use super::schema_check;
use super::tasks;
use crate::protocol::ToolDefinition;

pub const OVERVIEW: &str = "sylvode_overview";
pub const SEARCH: &str = "sylvode_search";
pub const READ: &str = "sylvode_read";
pub const WRITE: &str = "sylvode_write";

/// The four entry tool names, in listing order.
pub const ENTRY_TOOL_NAMES: [&str; 4] = [OVERVIEW, SEARCH, READ, WRITE];

/// The version of the task protocol the server speaks (`capabilities.experimental`).
pub const TASK_PROTOCOL_VERSION: u64 = 1;

/// Upper bound of the serialized `sylvode_search` answer; results beyond it come without schema.
pub const SEARCH_BUDGET_BYTES: usize = 24 * 1024;

const DEFAULT_LIMIT: u64 = 5;
const MAX_LIMIT: u64 = 10;
const WRITES_DISABLED: &str = "writes are disabled for this bot by the runner configuration";

pub fn is_entry_tool(name: &str) -> bool {
    ENTRY_TOOL_NAMES.contains(&name)
}

/// Whether `tools/list` of `mode` offers `name`: the entry tools and the task tools in search
/// mode, every catalogue tool (the task tools among them) in full mode.
pub fn listed_in(mode: McpToolMode, name: &str) -> bool {
    match mode {
        McpToolMode::Search => is_entry_tool(name) || tasks::is_task_tool(name),
        McpToolMode::Full => catalog::meta(name).is_some(),
    }
}

/// The refusal for a call to a name `tools/list` of `mode` does not offer, or `None` when the
/// name is offered. A catalogue tool in search mode is pointed to the wrappers.
pub fn unlisted_refusal(mode: McpToolMode, name: &str) -> Option<String> {
    if listed_in(mode, name) {
        return None;
    }
    let shown = printable(name);
    Some(match mode {
        McpToolMode::Search if catalog::meta(name).is_some() => {
            format!("'{shown}' is not listed in search mode; call it through sylvode_read or sylvode_write")
        }
        McpToolMode::Search => format!("unknown tool '{shown}'; use sylvode_search"),
        McpToolMode::Full if is_entry_tool(name) => format!("'{shown}' is not listed in full mode"),
        McpToolMode::Full => format!("unknown tool '{shown}'"),
    })
}

/// The name a tool is attributed to in the operation log.
///
/// The API records only dotted lower-case names (`work_items.update`). The task tools and the
/// search entry tools are named for agents with underscores only, so they are attributed to dotted
/// names: `tasks.acknowledge` for `sylvode_task_ack`, `entry.search` for `sylvode_search`. Every
/// other tool is attributed to its own name. `sylvode_read` and `sylvode_write` never appear: a
/// wrapper call is attributed to its target.
pub fn attribution_name(name: &str) -> &str {
    match name {
        tasks::TASK_GET => "tasks.get",
        tasks::TASK_ACK => "tasks.acknowledge",
        tasks::TASK_PROGRESS => "tasks.progress",
        tasks::TASK_COMPLETE => "tasks.complete",
        tasks::TASK_FAIL => "tasks.fail",
        OVERVIEW => "entry.overview",
        SEARCH => "entry.search",
        other => other,
    }
}

/// A caller supplied name, cut and stripped of control characters for an error text.
fn printable(text: &str) -> String {
    text.chars().filter(|c| !c.is_control()).take(80).collect::<String>()
}

fn project_id_property() -> Value {
    json!({ "type": "string", "format": "uuid", "description": "Filter by this project's agent policy." })
}

fn wrapper_schema() -> Value {
    json!({
        "type": "object",
        "additionalProperties": false,
        "required": ["tool", "arguments"],
        "properties": {
            "tool": { "type": "string", "pattern": r"^[a-z_]+(\.[a-z_]+)+$" },
            "arguments": { "type": "object" }
        }
    })
}

/// The four entry tool definitions (D4.6).
pub fn definitions() -> Vec<ToolDefinition> {
    vec![
        ToolDefinition {
            name: OVERVIEW.to_string(),
            description: "Start here. Lists Sylvode tool categories with counts, how to find and call tools, and \
                          the task procedure. Read-only."
                .to_string(),
            input_schema: json!({
                "type": "object",
                "additionalProperties": false,
                "properties": { "project_id": project_id_property() }
            }),
            annotations: Some(json!({ "readOnlyHint": true })),
        },
        ToolDefinition {
            name: SEARCH.to_string(),
            description: "Find Sylvode tools by keyword, category, intent or exact name. Returns each match with \
                          its full input schema and whether to call it with sylvode_read or sylvode_write. \
                          Read-only."
                .to_string(),
            input_schema: json!({
                "type": "object",
                "additionalProperties": false,
                "properties": {
                    "query": {
                        "type": "string", "maxLength": 200,
                        "description": "Words or an exact tool name, e.g. 'close issue' or 'work_items.update'."
                    },
                    "category": { "type": "string", "description": "A category from sylvode_overview." },
                    "access": { "type": "string", "enum": ["read", "write", "any"], "default": "any" },
                    "project_id": { "type": "string", "format": "uuid" },
                    "limit": { "type": "integer", "minimum": 1, "maximum": MAX_LIMIT, "default": DEFAULT_LIMIT }
                },
                "anyOf": [{ "required": ["query"] }, { "required": ["category"] }]
            }),
            annotations: Some(json!({ "readOnlyHint": true })),
        },
        ToolDefinition {
            name: READ.to_string(),
            description: "Call one read-only Sylvode tool by name. Refuses tools that change data (use \
                          sylvode_write)."
                .to_string(),
            input_schema: wrapper_schema(),
            annotations: Some(json!({ "readOnlyHint": true })),
        },
        ToolDefinition {
            name: WRITE.to_string(),
            description: "Call one Sylvode tool that creates, changes or deletes data. Refuses read-only tools \
                          (use sylvode_read)."
                .to_string(),
            input_schema: wrapper_schema(),
            annotations: Some(json!({ "readOnlyHint": false, "destructiveHint": true })),
        },
    ]
}

/// The nine definitions `tools/list` offers in search mode: the entry tools, then the task tools.
pub fn search_mode_listing() -> Vec<ToolDefinition> {
    let mut listing = definitions();
    listing.extend(tasks::tool_definitions());
    listing
}

/// The registry definitions, built once.
fn registry() -> &'static [ToolDefinition] {
    static REGISTRY: OnceLock<Vec<ToolDefinition>> = OnceLock::new();
    REGISTRY.get_or_init(super::get_all_tool_definitions)
}

/// The definition of a registry tool.
pub fn registry_definition(name: &str) -> Option<&'static ToolDefinition> {
    registry().iter().find(|tool| tool.name == name)
}

// ---- wrappers ----

/// The checks of one `sylvode_read` or `sylvode_write` call before it runs.
///
/// They are the shape of the wrapper's own arguments, a catalogue tool of the right access class,
/// and the target's arguments against the target's schema. Returns the target and its arguments.
///
/// Whether writes are allowed at all is the caller's decision and is made before this runs.
pub fn resolve_wrapper_call(wrapper: &str, args: &Value) -> Result<(String, Value), String> {
    let wanted = if wrapper == READ { Access::Read } else { Access::Write };
    let tool = args
        .get("tool")
        .and_then(Value::as_str)
        .ok_or_else(|| format!("{wrapper} needs a 'tool' name; use sylvode_search to find one"))?;
    if is_entry_tool(tool) || tasks::is_task_tool(tool) {
        return Err(format!(
            "'{}' is called directly, not through {wrapper}",
            printable(tool)
        ));
    }
    schema_check::validate(&wrapper_schema(), args)
        .map_err(|error| format!("Invalid arguments for {wrapper}: {error}"))?;
    let entry = catalog::meta(tool).ok_or_else(|| format!("unknown tool '{}'; use sylvode_search", printable(tool)))?;
    if entry.access != wanted {
        return Err(match wanted {
            Access::Read => format!("'{tool}' changes data; call it with sylvode_write"),
            Access::Write => format!("'{tool}' only reads data; call it with sylvode_read"),
        });
    }
    let definition =
        registry_definition(tool).ok_or_else(|| format!("unknown tool '{}'; use sylvode_search", printable(tool)))?;
    let arguments = args.get("arguments").cloned().unwrap_or(Value::Null);
    schema_check::validate(&definition.input_schema, &arguments)
        .map_err(|error| format!("Invalid arguments for '{tool}': {error}"))?;
    Ok((tool.to_string(), arguments))
}

/// The answer of a `sylvode_write` call while writes are switched off.
pub const fn writes_disabled_message() -> &'static str {
    WRITES_DISABLED
}

// ---- search ----

fn tokens(text: &str) -> Vec<String> {
    let mut words = Vec::new();
    for word in text.to_lowercase().split(|c: char| !c.is_alphanumeric()) {
        if !word.is_empty() && !words.iter().any(|seen| seen == word) {
            words.push(word.to_string());
        }
    }
    words
}

fn normalized(text: &str) -> String {
    tokens(text).join(" ")
}

/// How well a catalogue entry matches the query: 5 for the exact name, 3 per query word in the
/// name, 2 per query word among the keywords, 1 per word in the summary, 1 when a word names the
/// category, and 2 when the whole query is one of the keywords.
fn score(entry: &catalog::ToolMeta, exact: bool, words: &[String], phrase: &str) -> u32 {
    let name_words = tokens(entry.name);
    let summary_words = tokens(entry.summary);
    let keyword_words = entry
        .keywords
        .iter()
        .flat_map(|keyword| tokens(keyword))
        .collect::<Vec<_>>();
    let category_words = tokens(entry.category);
    let mut total = if exact { 5 } else { 0 };
    for word in words {
        if name_words.contains(word) {
            total += 3;
        }
        if keyword_words.contains(word) {
            total += 2;
        }
        if summary_words.contains(word) {
            total += 1;
        }
    }
    if words.iter().any(|word| category_words.contains(word)) {
        total += 1;
    }
    if !phrase.is_empty() && entry.keywords.iter().any(|keyword| normalized(keyword) == phrase) {
        total += 2;
    }
    total
}

/// The arguments of one `sylvode_search` call, read after schema validation.
struct SearchRequest {
    query: String,
    category: Option<String>,
    access: Option<Access>,
    limit: usize,
}

fn read_search_request(args: &Value) -> Result<SearchRequest, String> {
    let query = args
        .get("query")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .trim()
        .to_string();
    let category = args.get("category").and_then(Value::as_str).map(str::to_string);
    if let Some(category) = &category
        && !CATEGORIES.iter().any(|known| known.name == category)
    {
        return Err(format!(
            "unknown category '{}'; sylvode_overview lists the categories",
            printable(category)
        ));
    }
    let access = match args.get("access").and_then(Value::as_str) {
        Some("read") => Some(Access::Read),
        Some("write") => Some(Access::Write),
        _ => None,
    };
    let limit = args
        .get("limit")
        .and_then(Value::as_u64)
        .unwrap_or(DEFAULT_LIMIT)
        .clamp(1, MAX_LIMIT);
    if query.is_empty() && category.is_none() {
        return Err("sylvode_search needs a 'query' or a 'category'".to_string());
    }
    Ok(SearchRequest {
        query,
        category,
        access,
        limit: usize::try_from(limit).unwrap_or(5),
    })
}

/// The tool call to give for a catalogue tool: the wrapper of its access class, or the task tool
/// itself (task tools are called directly).
fn call_with(entry: &catalog::ToolMeta) -> &'static str {
    if tasks::is_task_tool(entry.name) {
        return entry.name;
    }
    match entry.access {
        Access::Read => READ,
        Access::Write => WRITE,
    }
}

/// Runs `sylvode_search`. `enabled` is the set of tools the project agent policy offers, or `None`
/// when no project was named (or the policy lists none, which means unrestricted).
pub fn search(args: &Value, enabled: Option<&BTreeSet<String>>) -> Result<Value, String> {
    schema_check::validate(&definitions().remove(1).input_schema, args)
        .map_err(|error| format!("Invalid arguments for {SEARCH}: {error}"))?;
    let request = read_search_request(args)?;
    let phrase = normalized(&request.query);
    let words = tokens(&request.query);
    let lowered = request.query.to_lowercase();

    let mut matches = CATALOG
        .iter()
        .enumerate()
        .filter(|(_, entry)| request.access.is_none_or(|access| entry.access == access))
        .filter(|(_, entry)| {
            request
                .category
                .as_deref()
                .is_none_or(|category| entry.category == category)
        })
        .filter(|(_, entry)| enabled.is_none_or(|names| names.contains(entry.name)))
        .filter_map(|(order, entry)| {
            let exact = !lowered.is_empty() && entry.name == lowered;
            let points = score(entry, exact, &words, &phrase);
            // A category-only search lists the whole category; a query needs at least one hit.
            (points > 0 || request.query.is_empty()).then_some((exact, points, order, entry))
        })
        .collect::<Vec<_>>();
    matches.sort_by(|left, right| {
        right
            .0
            .cmp(&left.0)
            .then(right.1.cmp(&left.1))
            .then(left.2.cmp(&right.2))
    });
    let total_matches = matches.len();
    matches.truncate(request.limit);

    let mut results = Vec::with_capacity(matches.len());
    let mut size = 0usize;
    let mut omitted = false;
    for (index, (_, _, _, entry)) in matches.iter().enumerate() {
        let mut item = json!({
            "name": entry.name,
            "category": entry.category,
            "access": entry.access.as_str(),
            "call_with": call_with(entry),
            "summary": entry.summary,
        });
        let schema = registry_definition(entry.name).map(|definition| definition.input_schema.clone());
        let with_schema = schema.as_ref().map(|schema| {
            let mut full = item.clone();
            if let Some(fields) = full.as_object_mut() {
                fields.insert("inputSchema".to_string(), schema.clone());
                fields.insert("schema_omitted".to_string(), json!(false));
            }
            full
        });
        let fits = with_schema
            .as_ref()
            .is_some_and(|full| index == 0 || size + full.to_string().len() <= SEARCH_BUDGET_BYTES);
        item = match with_schema {
            Some(full) if fits => full,
            _ => {
                omitted = true;
                if let Some(fields) = item.as_object_mut() {
                    fields.insert("schema_omitted".to_string(), json!(true));
                }
                item
            }
        };
        size += item.to_string().len();
        results.push(item);
    }

    let mut hint = match matches.first() {
        Some((_, _, _, entry)) if tasks::is_task_tool(entry.name) => {
            format!("Call {} directly with its arguments", entry.name)
        }
        Some((_, _, _, entry)) => format!(
            "Call with {} {{\"tool\":\"{}\",\"arguments\":{{...}}}}",
            call_with(entry),
            entry.name
        ),
        None => "No tool matched; try other words, a category from sylvode_overview, or an exact tool name".to_string(),
    };
    if omitted {
        hint.push_str(". Some results came without their schema: search the exact name to get its schema");
    }
    Ok(json!({
        "mode": "search",
        "total_matches": total_matches,
        "returned": results.len(),
        "results": results,
        "hint": hint,
    }))
}

// ---- overview ----

/// Runs `sylvode_overview`. `project` is the project the policy filter was applied for.
pub fn overview(project: Option<&str>, enabled: Option<&BTreeSet<String>>) -> Value {
    let categories = CATEGORIES
        .iter()
        .filter_map(|category| {
            let entries = CATALOG
                .iter()
                .filter(|entry| entry.category == category.name)
                .filter(|entry| enabled.is_none_or(|names| names.contains(entry.name)))
                .collect::<Vec<_>>();
            if entries.is_empty() {
                return None;
            }
            let reads = entries.iter().filter(|entry| entry.access == Access::Read).count();
            Some(json!({
                "name": category.name,
                "title": category.title,
                "count": entries.len(),
                "read": reads,
                "write": entries.len() - reads,
                "summary": category.summary,
            }))
        })
        .collect::<Vec<_>>();
    json!({
        "server": "sylvode-mcp-server",
        "version": env!("CARGO_PKG_VERSION"),
        "task_protocol": TASK_PROTOCOL_VERSION,
        "mode": "search",
        "categories": categories,
        "how_to": [
            "sylvode_search {query} to find a tool and its schema",
            "sylvode_read for access=read",
            "sylvode_write for access=write",
        ],
        "task_procedure": [
            "sylvode_task_get",
            "sylvode_task_ack",
            "work + sylvode_task_progress",
            "sylvode_task_complete | sylvode_task_fail",
        ],
        "policy_filtered_for_project": project,
    })
}

/// The `project_id` of an entry tool call: absent, or a canonical UUID.
pub fn project_argument(args: &Value) -> Result<Option<String>, String> {
    match args.get("project_id") {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(text)) => Uuid::parse_str(text)
            .map(|id| Some(id.to_string()))
            .map_err(|_| "Invalid arguments: project_id must be a UUID".to_string()),
        Some(_) => Err("Invalid arguments: project_id must be a UUID".to_string()),
    }
}

#[cfg(test)]
#[allow(clippy::indexing_slicing, clippy::needless_pass_by_value)]
mod tests {
    use super::*;
    use crate::tools::get_all_tool_definitions;
    use crate::tools::test_support::minimal_arguments;

    const PARKED: [&str; 6] = [
        "project_resources.list",
        "project_resources.create",
        "project_resources.update",
        "project_resources.delete",
        "code.resources.list",
        "code.directory.get",
    ];

    fn names_of(result: &Value) -> Vec<String> {
        result
            .get("results")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(|item| item.get("name").and_then(Value::as_str).map(str::to_string))
            .collect()
    }

    fn run(args: Value) -> Value {
        search(&args, None).expect("a valid search")
    }

    // ---- the listing ----

    #[test]
    fn the_search_listing_is_exactly_the_four_entry_tools_and_the_five_task_tools() {
        let listing = search_mode_listing();
        let names = listing.iter().map(|tool| tool.name.as_str()).collect::<Vec<_>>();
        assert_eq!(
            names,
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
        for name in &names {
            assert!(
                !name.contains('.') && name.chars().all(|c| c.is_ascii_lowercase() || c == '_'),
                "{name} must use only [a-z_]"
            );
        }
        let hints = |name: &str| {
            listing
                .iter()
                .find(|tool| tool.name == name)
                .and_then(|tool| tool.annotations.clone())
        };
        assert_eq!(hints(OVERVIEW), Some(json!({ "readOnlyHint": true })));
        assert_eq!(hints(READ), Some(json!({ "readOnlyHint": true })));
        assert_eq!(
            hints(WRITE),
            Some(json!({ "readOnlyHint": false, "destructiveHint": true }))
        );
    }

    #[test]
    fn the_entry_tools_are_in_no_registry_and_no_catalogue() {
        for name in ENTRY_TOOL_NAMES {
            assert!(registry_definition(name).is_none(), "{name}");
            assert!(catalog::meta(name).is_none(), "{name}");
        }
    }

    /// The schema checker enforces every keyword the registry, the task tools and the entry tools
    /// use, so a schema can never be half enforced by a wrapper without this test failing first.
    #[test]
    fn every_schema_of_the_surface_is_fully_enforceable() {
        let mut problems = Vec::new();
        for tool in get_all_tool_definitions()
            .into_iter()
            .chain(definitions())
            .chain(tasks::tool_definitions())
        {
            for problem in schema_check::unsupported(&tool.input_schema) {
                problems.push(format!("{}: {problem}", tool.name));
            }
        }
        assert!(problems.is_empty(), "{problems:#?}");
    }

    // ---- mode ----

    #[test]
    fn each_mode_lists_exactly_what_its_listing_contains() {
        let search_names = search_mode_listing()
            .into_iter()
            .map(|tool| tool.name)
            .collect::<BTreeSet<_>>();
        let registry_names = get_all_tool_definitions()
            .into_iter()
            .map(|tool| tool.name)
            .collect::<BTreeSet<_>>();
        let everything = registry_names
            .iter()
            .cloned()
            .chain(ENTRY_TOOL_NAMES.map(str::to_string))
            .chain(PARKED.map(str::to_string))
            .chain(["no.such_tool".to_string(), String::new()])
            .collect::<BTreeSet<_>>();
        for name in &everything {
            assert_eq!(
                listed_in(McpToolMode::Search, name),
                search_names.contains(name),
                "search: {name}"
            );
            assert_eq!(
                listed_in(McpToolMode::Full, name),
                registry_names.contains(name),
                "full: {name}"
            );
            assert_eq!(
                unlisted_refusal(McpToolMode::Search, name).is_none(),
                search_names.contains(name)
            );
            assert_eq!(
                unlisted_refusal(McpToolMode::Full, name).is_none(),
                registry_names.contains(name)
            );
        }
        assert_eq!(search_names.len(), 9);
        assert_eq!(registry_names.len(), 154);
    }

    #[test]
    fn the_refusals_name_the_way_out_and_never_echo_control_characters() {
        assert_eq!(
            unlisted_refusal(McpToolMode::Search, "projects.list").as_deref(),
            Some("'projects.list' is not listed in search mode; call it through sylvode_read or sylvode_write")
        );
        for parked in PARKED {
            assert_eq!(
                unlisted_refusal(McpToolMode::Search, parked),
                Some(format!("unknown tool '{parked}'; use sylvode_search")),
                "{parked} is parked, so it is unknown"
            );
        }
        assert_eq!(
            unlisted_refusal(McpToolMode::Full, "sylvode_search").as_deref(),
            Some("'sylvode_search' is not listed in full mode")
        );
        let hostile =
            unlisted_refusal(McpToolMode::Search, &format!("a\u{1b}[31m\n{}", "x".repeat(500))).unwrap_or_default();
        assert!(
            hostile.chars().all(|c| !c.is_control()) && hostile.len() < 200,
            "{hostile}"
        );
    }

    // ---- ranking ----

    #[test]
    fn the_exact_name_comes_first_with_its_schema_for_every_catalogue_tool() {
        for entry in CATALOG {
            let result = run(json!({ "query": entry.name, "limit": 3 }));
            let first = result["results"].get(0).unwrap_or(&Value::Null);
            assert_eq!(first["name"], json!(entry.name), "{}", entry.name);
            assert_eq!(first["schema_omitted"], json!(false), "{}", entry.name);
            assert_eq!(
                first.get("inputSchema"),
                registry_definition(entry.name).map(|tool| &tool.input_schema),
                "{}",
                entry.name
            );
            assert_eq!(first["access"], json!(entry.access.as_str()));
            let expected_call = if tasks::is_task_tool(entry.name) {
                entry.name
            } else if entry.access == Access::Read {
                READ
            } else {
                WRITE
            };
            assert_eq!(first["call_with"], json!(expected_call), "{}", entry.name);
            // The upper case spelling finds it too.
            let shouted = run(json!({ "query": entry.name.to_uppercase(), "limit": 1 }));
            assert_eq!(names_of(&shouted), [entry.name], "{}", entry.name);
        }
    }

    #[test]
    fn a_keyword_synonym_finds_the_tool() {
        let top = |query: &str| names_of(&run(json!({ "query": query, "limit": 3 })));
        assert!(
            top("close issue").contains(&"work_items.update".to_string()),
            "{:?}",
            top("close issue")
        );
        assert_eq!(
            top("close issue").first().map(String::as_str),
            Some("work_items.update")
        );
        assert!(top("new sprint").contains(&"sprints.create".to_string()));
        assert!(top("recycle bin").contains(&"flow.trash.list".to_string()));
        assert!(top("who can access a form").contains(&"form_permissions.get".to_string()));
        assert!(top("heartbeat").contains(&"sylvode_task_progress".to_string()));
    }

    #[test]
    fn every_keyword_of_every_tool_finds_that_tool_in_the_top_ten() {
        for entry in CATALOG {
            for keyword in entry.keywords {
                let found = names_of(&run(json!({ "query": keyword, "limit": 10 })));
                assert!(
                    found.contains(&entry.name.to_string()),
                    "{keyword:?} does not find {}: {found:?}",
                    entry.name
                );
            }
        }
    }

    #[test]
    fn the_category_and_access_filters_apply_before_ranking() {
        for category in CATEGORIES {
            let listed = run(json!({ "category": category.name, "limit": 10 }));
            let expected = CATALOG
                .iter()
                .filter(|entry| entry.category == category.name)
                .take(10)
                .map(|entry| entry.name.to_string())
                .collect::<Vec<_>>();
            assert_eq!(names_of(&listed), expected, "{}", category.name);
            for access in [Access::Read, Access::Write] {
                let filtered = run(json!({ "category": category.name, "access": access.as_str(), "limit": 10 }));
                for name in names_of(&filtered) {
                    let entry = catalog::meta(&name).expect("catalogue tool");
                    assert_eq!((entry.category, entry.access), (category.name, access), "{name}");
                }
            }
        }
        let writes = run(json!({ "query": "work items", "access": "write", "category": "work_items", "limit": 10 }));
        assert!(
            names_of(&writes)
                .iter()
                .all(|name| catalog::meta(name).is_some_and(|e| e.access == Access::Write))
        );
        assert!(names_of(&writes).contains(&"work_items.update".to_string()));
        assert!(search(&json!({ "category": "no_such" }), None).is_err_and(|error| error.contains("unknown category")),);
    }

    #[test]
    fn the_limit_the_default_and_the_counts_are_honoured() {
        let default = run(json!({ "query": "work items" }));
        assert_eq!(default["returned"], json!(5));
        assert!(default["total_matches"].as_u64().unwrap_or(0) > 5);
        let one = run(json!({ "query": "work items", "limit": 1 }));
        assert_eq!(one["returned"], json!(1));
        assert_eq!(one["total_matches"], default["total_matches"]);
        for bad in [
            json!({ "query": "x", "limit": 0 }),
            json!({ "query": "x", "limit": 11 }),
            json!({ "query": "x", "limit": "3" }),
        ] {
            assert!(search(&bad, None).is_err(), "{bad}");
        }
        assert!(search(&json!({}), None).is_err(), "a query or a category is required");
        assert!(search(&json!({ "query": "x".repeat(201) }), None).is_err());
        assert!(search(&json!({ "query": "x", "access": "both" }), None).is_err());
        assert!(search(&json!({ "query": "x", "project_id": "nope" }), None).is_err());
        assert!(search(&json!({ "query": "x", "extra": 1 }), None).is_err());
        let none = run(json!({ "query": "zzzzqqqq" }));
        assert_eq!(none["total_matches"], json!(0));
        assert!(
            none["hint"]
                .as_str()
                .is_some_and(|hint| hint.contains("No tool matched"))
        );
    }

    /// Search can only ever name a catalogue tool: a parked tool is in no catalogue, so it cannot be
    /// found by name, keyword or category, and nothing the catalogue lacks can appear.
    #[test]
    fn search_never_returns_a_name_outside_the_catalogue() {
        let known = CATALOG.iter().map(|entry| entry.name).collect::<BTreeSet<_>>();
        let mut queries = PARKED.iter().map(ToString::to_string).collect::<Vec<_>>();
        queries.extend([
            "project resources".to_string(),
            "directory resource".to_string(),
            "code resources".to_string(),
        ]);
        queries.extend(CATALOG.iter().map(|entry| entry.name.to_string()));
        queries.extend(
            CATALOG
                .iter()
                .flat_map(|entry| entry.keywords.iter().map(ToString::to_string)),
        );
        queries.extend(CATEGORIES.iter().map(|category| category.name.to_string()));
        for query in &queries {
            for name in names_of(&run(json!({ "query": query, "limit": 10 }))) {
                assert!(known.contains(name.as_str()), "{query:?} returned {name}");
                assert!(!PARKED.contains(&name.as_str()), "{query:?} returned parked {name}");
            }
        }
    }

    #[test]
    fn the_policy_filter_removes_a_tool_from_search_and_overview() {
        for entry in CATALOG {
            let enabled = CATALOG
                .iter()
                .map(|other| other.name.to_string())
                .filter(|name| name != entry.name)
                .collect::<BTreeSet<_>>();
            let found = search(&json!({ "query": entry.name, "limit": 10 }), Some(&enabled)).expect("search");
            assert!(
                !names_of(&found).contains(&entry.name.to_string()),
                "{} survived the filter",
                entry.name
            );
            let category = search(&json!({ "category": entry.category, "limit": 10 }), Some(&enabled)).expect("search");
            assert!(!names_of(&category).contains(&entry.name.to_string()), "{}", entry.name);
        }
        let only_one = BTreeSet::from(["work_items.get".to_string()]);
        let overview = overview(Some("11111111-1111-4111-8111-111111111111"), Some(&only_one));
        let categories = overview["categories"].as_array().cloned().unwrap_or_default();
        assert_eq!(categories.len(), 1);
        assert_eq!(categories[0]["name"], json!("work_items"));
        assert_eq!(
            (categories[0]["count"].clone(), categories[0]["read"].clone()),
            (json!(1), json!(1))
        );
        assert_eq!(
            overview["policy_filtered_for_project"],
            json!("11111111-1111-4111-8111-111111111111")
        );
    }

    #[test]
    fn results_past_the_budget_come_without_a_schema_and_the_first_never_does() {
        let result = run(json!({ "category": "flow", "limit": 10 }));
        let results = result["results"].as_array().cloned().unwrap_or_default();
        assert_eq!(results.len(), 10);
        let mut with_schema = 0;
        for (index, item) in results.iter().enumerate() {
            let omitted = item["schema_omitted"] == json!(true);
            assert_eq!(omitted, item.get("inputSchema").is_none(), "{item}");
            if index == 0 {
                assert!(!omitted);
            }
            if !omitted {
                with_schema += 1;
            }
        }
        let with_schemas_size = results
            .iter()
            .filter(|item| item.get("inputSchema").is_some())
            .map(|item| item.to_string().len())
            .sum::<usize>();
        assert!(
            with_schemas_size <= SEARCH_BUDGET_BYTES || with_schema == 1,
            "{with_schemas_size} bytes of schemas in {with_schema} results"
        );
        if results.iter().any(|item| item["schema_omitted"] == json!(true)) {
            assert!(
                result["hint"]
                    .as_str()
                    .is_some_and(|hint| hint.contains("search the exact name"))
            );
        }
    }

    #[test]
    fn the_overview_counts_the_whole_catalogue_by_category() {
        let overview = overview(None, None);
        let categories = overview["categories"].as_array().cloned().unwrap_or_default();
        assert_eq!(categories.len(), 20);
        let total = categories.iter().map(|c| c["count"].as_u64().unwrap_or(0)).sum::<u64>();
        assert_eq!(total, 154);
        for category in &categories {
            let (count, read, write) = (
                category["count"].as_u64().unwrap_or(0),
                category["read"].as_u64().unwrap_or(0),
                category["write"].as_u64().unwrap_or(0),
            );
            assert_eq!(count, read + write, "{category}");
        }
        assert_eq!(overview["version"], json!(env!("CARGO_PKG_VERSION")));
        assert_eq!(overview["task_protocol"], json!(1));
        assert_eq!(overview["mode"], json!("search"));
        assert_eq!(overview["policy_filtered_for_project"], Value::Null);
    }

    // ---- the wrappers ----

    fn wrap(wrapper: &str, tool: &str, arguments: &Value) -> Result<(String, Value), String> {
        resolve_wrapper_call(wrapper, &json!({ "tool": tool, "arguments": arguments }))
    }

    /// Over the whole catalogue: a tool is reachable through the wrapper of its own access class
    /// with the arguments its schema allows, and through no other door.
    #[test]
    fn every_catalogue_tool_is_reachable_through_its_own_wrapper_only() {
        for definition in get_all_tool_definitions() {
            let entry = catalog::meta(&definition.name).expect("catalogue entry");
            let arguments = minimal_arguments(&definition);
            if tasks::is_task_tool(&definition.name) {
                for wrapper in [READ, WRITE] {
                    let refused = wrap(wrapper, &definition.name, &arguments).expect_err("task tools are direct");
                    assert!(refused.contains("is called directly"), "{refused}");
                }
                continue;
            }
            let (own, other) = if entry.access == Access::Read {
                (READ, WRITE)
            } else {
                (WRITE, READ)
            };
            let reached = wrap(own, &definition.name, &arguments);
            assert_eq!(
                reached,
                Ok((definition.name.clone(), arguments.clone())),
                "{}",
                definition.name
            );
            let refused = wrap(other, &definition.name, &arguments).expect_err("the other wrapper refuses");
            let expected = if own == READ {
                format!("'{}' only reads data; call it with sylvode_read", definition.name)
            } else {
                format!("'{}' changes data; call it with sylvode_write", definition.name)
            };
            assert_eq!(refused, expected);
        }
    }

    #[test]
    fn parked_entry_task_and_unknown_names_are_not_wrapper_targets() {
        for wrapper in [READ, WRITE] {
            for parked in PARKED {
                let refused = wrap(
                    wrapper,
                    parked,
                    &json!({ "project_id": "11111111-1111-4111-8111-111111111111" }),
                )
                .expect_err("parked");
                assert_eq!(refused, format!("unknown tool '{parked}'; use sylvode_search"));
            }
            for direct in ENTRY_TOOL_NAMES.iter().copied().chain(tasks::TASK_TOOL_NAMES) {
                let refused = wrap(wrapper, direct, &json!({})).expect_err("direct only");
                assert_eq!(refused, format!("'{direct}' is called directly, not through {wrapper}"));
            }
            for unknown in [
                "no.such_tool",
                "a.b.c",
                "projects.LIST",
                "projects.list ",
                "projects..list",
                "",
                "../x",
            ] {
                assert!(wrap(wrapper, unknown, &json!({})).is_err(), "{unknown:?}");
            }
        }
    }

    #[test]
    fn the_wrapper_arguments_are_validated_before_anything_else() {
        for wrapper in [READ, WRITE] {
            for bad in [
                json!({}),
                json!({ "tool": "projects.list" }),
                json!({ "arguments": {} }),
                json!({ "tool": 7, "arguments": {} }),
                json!({ "tool": "projects.list", "arguments": [] }),
                json!({ "tool": "projects.list", "arguments": "x" }),
                json!({ "tool": "projects.list", "arguments": {}, "extra": true }),
                json!([]),
                Value::Null,
            ] {
                assert!(resolve_wrapper_call(wrapper, &bad).is_err(), "{wrapper} {bad}");
            }
        }
    }

    /// Arguments are validated against the target's own schema: every tool with a closed schema
    /// refuses an undeclared argument and every required argument is enforced.
    #[test]
    fn the_target_schema_is_enforced_for_every_tool() {
        let mut closed = 0;
        for definition in get_all_tool_definitions() {
            if tasks::is_task_tool(&definition.name) {
                continue;
            }
            let entry = catalog::meta(&definition.name).expect("catalogue entry");
            let wrapper = if entry.access == Access::Read { READ } else { WRITE };
            let arguments = minimal_arguments(&definition);
            if definition.input_schema.get("additionalProperties") == Some(&json!(false)) {
                closed += 1;
                let mut extended = arguments.clone();
                if let Some(object) = extended.as_object_mut() {
                    object.insert("zz_undeclared".to_string(), json!(1));
                }
                assert!(
                    wrap(wrapper, &definition.name, &extended).is_err_and(|e| e.contains("zz_undeclared")),
                    "{} accepted an undeclared argument",
                    definition.name
                );
            }
            for required in definition
                .input_schema
                .get("required")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(Value::as_str)
            {
                let mut missing = arguments.clone();
                if let Some(object) = missing.as_object_mut() {
                    object.remove(required);
                }
                assert!(
                    wrap(wrapper, &definition.name, &missing).is_err(),
                    "{} accepted a call without {required}",
                    definition.name
                );
            }
            // A wrong type for any declared property is refused.
            for (property, schema) in definition
                .input_schema
                .get("properties")
                .and_then(Value::as_object)
                .into_iter()
                .flatten()
            {
                let wrong = match schema.get("type").and_then(Value::as_str) {
                    Some("string") => json!(12345),
                    Some("integer" | "number") => json!("not a number"),
                    Some("boolean") => json!("not a boolean"),
                    Some("array") => json!("not an array"),
                    Some("object") => json!("not an object"),
                    _ => continue,
                };
                if schema.get("const").is_some() || schema.get("enum").is_some() && schema.get("type").is_none() {
                    continue;
                }
                let mut changed = arguments.clone();
                if let Some(object) = changed.as_object_mut() {
                    object.insert(property.clone(), wrong);
                }
                assert!(
                    wrap(wrapper, &definition.name, &changed).is_err(),
                    "{}.{property} accepted a value of the wrong type",
                    definition.name
                );
            }
        }
        assert!(closed >= 40, "only {closed} tools have a closed schema");
    }

    #[test]
    fn a_project_id_is_a_canonical_uuid_or_absent() {
        assert_eq!(project_argument(&json!({})), Ok(None));
        assert_eq!(project_argument(&json!({ "project_id": null })), Ok(None));
        assert_eq!(
            project_argument(&json!({ "project_id": "AAAAAAAA-AAAA-4AAA-8AAA-AAAAAAAAAAAA" })),
            Ok(Some("aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa".to_string()))
        );
        for bad in [
            json!("x"),
            json!("../1"),
            json!(1),
            json!("11111111-1111-4111-8111-111111111111/x"),
        ] {
            assert!(project_argument(&json!({ "project_id": bad })).is_err(), "{bad}");
        }
    }
}
