//! Every id a bot-reachable request body carries must be accounted for.
//!
//! The middleware resolves one target per route. An `*_id` / `*_ids` field of a request body that is
//! neither that target nor checked by the handler against the bot's project level is a way to reach a
//! second project (audit F2). This test walks every unsafe route a scoped bot can reach, finds the
//! handler's `Json<T>` type in the source, and lists the id fields of `T`; each must be the
//! middleware's target for that route, exempt with a reason, or checked (the named source file calls
//! `ensure_bot_level`). Command payloads (`*Payload` structs of the Flow command registry) are
//! scanned the same way. A new field fails here until it is classified on purpose.
//!
//! Limits: fields of nested types and of an untyped `Json<Value>` body are not seen; an untyped body
//! must be listed in `UNTYPED_BODIES` with the reason it is covered.
#![cfg(test)]
#![allow(clippy::expect_used, clippy::panic, clippy::indexing_slicing)]

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use super::scoped_bot::{Class, Find, all_routes};

/// How an id field is accounted for.
enum Account {
    /// Not an object of another project: the reason says why.
    Exempt(&'static str),
    /// Checked through `ensure_bot_level`; the file (relative to `src/`) must call it.
    Checked(&'static str),
    /// A scope-native handler (the named function calls `user()`, which refuses bot credentials).
    ScopeNative(&'static str),
    /// Not closed by this change: the field names something the handler may not confine to the
    /// bot's project. Listed so it stays visible; the reason says what is known.
    Open(&'static str),
}
use Account::{Checked, Exempt, Open, ScopeNative};

/// `(struct name, field)`: the id fields of request bodies and command payloads that name something
/// beyond the route's resolved target.
const ACCOUNTED: &[(&str, &str, Account)] = &[
    // Flow conversions and references (flow/bridge.rs).
    (
        "ConversionPreviewInput",
        "source_object_id",
        Exempt("the route's resolved target (Find::Conversion); source_actor also checks it"),
    ),
    (
        "ConversionCommitInput",
        "preview_id",
        Exempt("the route's resolved target (Find::Conversion): the preview's source object"),
    ),
    ("CreateReferenceInput", "target_id", Checked("flow/bridge.rs")),
    // Flow commands (flow/command.rs, move_object.rs, relations.rs).
    ("MoveObjectPayload", "target_object_id", Checked("flow/move_object.rs")),
    (
        "MoveObjectPayload",
        "after_id",
        Exempt("a sibling entry of the destination navigator, which is checked"),
    ),
    ("LinkPayload", "target_object_id", Checked("flow/relations.rs")),
    (
        "UnlinkPayload",
        "relation_id",
        Exempt("must belong to the command's own object; its far end is checked"),
    ),
    (
        "BodyAssetPayload",
        "asset_id",
        Exempt("an asset of the object's own scope; the bytes are copied only by body-assets/copy, which is checked"),
    ),
    (
        "ChildLinkPayload",
        "child_page_id",
        Exempt(
            "a pointer stored in the page; reading the child goes through the object routes, which the middleware judges",
        ),
    ),
    ("BodyAssetCopy", "source_object_id", Checked("routes/flow_v2.rs")),
    (
        "BodyAssetCopy",
        "asset_id",
        Exempt("an asset of the source object, which is checked"),
    ),
    (
        "LinkPageRequest",
        "object_id",
        Exempt("viewable_pages applies the bot's project level to the page"),
    ),
    // Flow collections and comments: ids inside the route's own object.
    (
        "RecordQueryPayload",
        "field_ids",
        Exempt("field ids of the collection of the route's own object"),
    ),
    (
        "CreateCollectionRecordRequest",
        "values_by_field_id",
        Exempt("field ids of the collection of the route's own object"),
    ),
    (
        "PackageExportRequest",
        "project_id",
        Exempt("the object export refuses a project_id (routes/flow.rs); the scope export is scope-native"),
    ),
    // Scope-native routes: the handler refuses bot credentials.
    (
        "NewComment",
        "block_id",
        Exempt("a block of the route's own page: normalized and registered against that object only"),
    ),
    (
        "NewComment",
        "parent_id",
        Exempt("flow/comments.rs resolves the parent comment within the same object"),
    ),
    (
        "MountInput",
        "destination_project_id",
        ScopeNative("post_flow_v2_mount"),
    ),
    (
        "MountInput",
        "destination_workspace_id",
        ScopeNative("post_flow_v2_mount"),
    ),
    // Forms: the forms layer confines these to the route's own form or project.
    (
        "CreateChildRecordRequest",
        "child_form_id",
        Exempt("resolve_child_form requires the parent record's workspace and project"),
    ),
    (
        "CreateLinkRequest",
        "target_id",
        Exempt("ensure_link_target_scope requires the same project, then form.view on the target form"),
    ),
    (
        "CreateAttachmentRequest",
        "record_id",
        Exempt("refused unless the record belongs to the route's form"),
    ),
    (
        "CreateAttachmentPackageJobRequest",
        "view_id",
        Exempt("find_readable_view refuses a view of another form"),
    ),
    (
        "CreateExportJobRequest",
        "view_id",
        Exempt("find_readable_view refuses a view of another form"),
    ),
    (
        "ImportRecordsFileRequest",
        "mapping_template_id",
        Exempt("refused unless the template belongs to the route's form (routes/form.rs)"),
    ),
    (
        "CreateImportJobRequest",
        "mapping_template_id",
        Exempt("refused unless the template belongs to the route's form (routes/form.rs)"),
    ),
    // Issues, proposals, tasks.
    (
        "CreateIssueRequest",
        "assignee_id",
        Exempt("a user id, not a project object; assignees must be members"),
    ),
    ("CreateIssueRequest", "sprint_id", Exempt("ensure_sprint_in_project")),
    (
        "CreateIssueRequest",
        "milestone_id",
        Exempt("milestones are workspace-wide; ensure_open_milestone checks the workspace"),
    ),
    (
        "UpdateIssueRequest",
        "assignee_id",
        Exempt("a user id, not a project object; assignees must be members"),
    ),
    (
        "UpdateIssueRequest",
        "sprint_id",
        Exempt("ensure_sprint_in_project against the issue's project"),
    ),
    (
        "UpdateIssueRequest",
        "milestone_id",
        Exempt("milestones are workspace-wide; ensure_open_milestone checks the workspace"),
    ),
    (
        "UpdateSprintRequest",
        "rollover_sprint_id",
        Exempt("refused unless it is another planned sprint of the same project"),
    ),
    (
        "SetProjectWorkflowRequest",
        "workflow_id",
        Exempt("workflows are workspace-level definitions; the handler requires an owner or admin"),
    ),
    (
        "AddLabelsRequest",
        "label_ids",
        Exempt("labels are workspace-wide; attach_labels_to_issue checks the workspace"),
    ),
    (
        "CreateAiTaskRequest",
        "ai_participant_id",
        Exempt("ensure_project_ai_participant"),
    ),
    (
        "CreateAiTaskRequest",
        "reference_id",
        Exempt("ensure_reference_in_project"),
    ),
    (
        "CreateImpactReviewRequest",
        "reviewer_id",
        Exempt("a user id, not a project object"),
    ),
    (
        "UpsertParticipantRequest",
        "user_id",
        Exempt("a user id, not a project object"),
    ),
    (
        "CreateProposalRequest",
        "template_id",
        Open(
            "a template belongs to a project and its text is copied into the proposal; the handler checks project membership of the account (is_project_member), not the bot's project level; the route is class Own",
        ),
    ),
    (
        "CreateProposalRequest",
        "workspace_id",
        Open(
            "the workspace of the proposal; proposals are not project objects, handled by the proposal actor context, not verified here",
        ),
    ),
    (
        "LinkIssueRequest",
        "issue_id",
        Open("audit F7: a bot that authored an approved proposal can link an issue of any project of the workspace"),
    ),
    (
        "PackageImportPreviewRequest",
        "artifact_id",
        ScopeNative("post_flow_v2_import_preview"),
    ),
];

/// Source files whose `*Payload` structs carry no id of another object, with the reason.
const PAYLOAD_FILES_EXEMPT: &[(&str, &str)] = &[
    (
        "/src/flow/collections.rs",
        "field, view and record ids of the collection the command's own object holds",
    ),
    (
        "/src/flow/scope_move.rs",
        "the scope-native move: its handler refuses bot credentials (user())",
    ),
    ("/src/flow/search.rs", "a pagination cursor, not a request body"),
];

/// Handlers whose body is an untyped `Json<Value>` and the reason each is covered.
const UNTYPED_BODIES: &[(&str, &str)] = &[(
    "post_flow_v2_conversion_preview",
    "refuses bot credentials before it reads the body (user()); the v1 twin has a typed body",
)];

/// Handlers registered under an alias (`use ... as`), whose function the scan cannot find by name.
const ALIASED_HANDLERS: &[(&str, &str)] = &[
    (
        "prepare_page_transfer",
        "routes/flow_transfer.rs: actor() calls flow_v2::user(), which refuses bot credentials",
    ),
    (
        "export_portable_graph",
        "routes/flow_portable_page.rs: user() refuses bot credentials",
    ),
    (
        "preview_portable_graph",
        "routes/flow_portable_page.rs: user() refuses bot credentials",
    ),
    (
        "import_portable_graph",
        "routes/flow_portable_page.rs: user() refuses bot credentials",
    ),
    (
        "preview_portable_page",
        "routes/flow_portable_page.rs: user() refuses bot credentials",
    ),
    (
        "import_portable_page",
        "routes/flow_portable_page.rs: user() refuses bot credentials",
    ),
];

fn sources() -> BTreeMap<PathBuf, String> {
    fn walk(dir: &Path, out: &mut BTreeMap<PathBuf, String>) {
        for entry in std::fs::read_dir(dir)
            .expect("a source directory is readable")
            .flatten()
        {
            let path = entry.path();
            if path.is_dir() {
                walk(&path, out);
            } else if path.extension().is_some_and(|ext| ext == "rs") {
                let text = std::fs::read_to_string(&path).expect("a source file is readable");
                out.insert(path, text);
            }
        }
    }
    let mut out = BTreeMap::new();
    walk(Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/src")), &mut out);
    // Request types that live in the workspace's own crates (not the vendored ones).
    let crates = Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/../../crates"));
    for entry in std::fs::read_dir(crates)
        .expect("the crates directory is readable")
        .flatten()
    {
        let src = entry.path().join("src");
        if entry.file_name() != "vendor" && src.is_dir() {
            walk(&src, &mut out);
        }
    }
    out
}

/// The `(method, pattern)` -> handler name map of the router files.
fn handlers() -> BTreeMap<(String, String), String> {
    let source = crate::router_source::text();
    let bytes = source.as_bytes();
    let mut found = BTreeMap::new();
    let mut from = 0;
    while let Some(at) = source[from..].find(".route(") {
        let open = from + at + ".route(".len();
        from = open;
        let Some(literal) = source[open..].trim_start().strip_prefix('"') else {
            continue;
        };
        let Some(end) = literal.find('"') else {
            continue;
        };
        let pattern = &literal[..end];
        let mut depth = 1_i32;
        let mut index = open;
        while depth > 0 && index < bytes.len() {
            match bytes[index] {
                b'(' => depth += 1,
                b')' => depth -= 1,
                _ => {}
            }
            index += 1;
        }
        let body = &source[open..index];
        if !body.contains("bot_or_user_auth_middleware") {
            continue;
        }
        for method in ["get", "post", "put", "patch", "delete"] {
            let needle = format!("{method}(");
            let mut search = 0;
            while let Some(position) = body[search..].find(&needle) {
                let absolute = search + position;
                let before = body[..absolute].chars().next_back();
                search = absolute + needle.len();
                if before.is_some_and(|c| c.is_ascii_alphanumeric() || c == '_') {
                    continue;
                }
                let rest = &body[absolute + needle.len()..];
                let name: String = rest
                    .chars()
                    .take_while(|c| c.is_ascii_alphanumeric() || *c == '_' || *c == ':')
                    .collect();
                let name = name.rsplit("::").next().unwrap_or_default().to_string();
                found.insert((method.to_ascii_uppercase(), pattern.to_string()), name);
            }
        }
    }
    found
}

/// The text of `fn name(` up to the end of its parameter list.
fn signature<'a>(sources: &'a BTreeMap<PathBuf, String>, name: &str) -> Option<&'a str> {
    let needle = format!("fn {name}(");
    sources.values().find_map(|text| {
        let at = text.find(&needle)?;
        let rest = &text[at..];
        let end = rest.find(") ->").or_else(|| rest.find("){"))?;
        Some(&rest[..end])
    })
}

/// The last path segment of the type inside `Json<...>` in a parameter list.
fn json_type(signature: &str) -> Option<String> {
    let at = signature.find("Json<")?;
    let rest = &signature[at + "Json<".len()..];
    let mut depth = 1;
    let mut end = 0;
    for (index, ch) in rest.char_indices() {
        match ch {
            '<' => depth += 1,
            '>' => {
                depth -= 1;
                if depth == 0 {
                    end = index;
                    break;
                }
            }
            _ => {}
        }
    }
    let inner = rest[..end].trim();
    Some(inner.rsplit("::").next().unwrap_or(inner).to_string())
}

/// The id fields (`*_id`, `*_ids`) of `struct name { ... }`.
fn id_fields(sources: &BTreeMap<PathBuf, String>, name: &str) -> Option<Vec<String>> {
    let needle = format!("struct {name} {{");
    let text = sources.values().find_map(|text| {
        let at = text.find(&needle)?;
        let rest = &text[at + needle.len()..];
        Some(&rest[..rest.find("\n}")?])
    })?;
    Some(
        text.lines()
            .filter_map(|line| {
                let line = line
                    .trim()
                    .trim_start_matches("pub(crate) ")
                    .trim_start_matches("pub(super) ")
                    .trim_start_matches("pub ");
                let (field, _) = line.split_once(':')?;
                (field.ends_with("_id") || field.ends_with("_ids")).then(|| field.to_string())
            })
            .collect(),
    )
}

fn accounted(struct_name: &str, field: &str) -> Option<&'static Account> {
    ACCOUNTED
        .iter()
        .find(|(name, f, _)| *name == struct_name && *f == field)
        .map(|(_, _, account)| account)
}

/// The id field the middleware resolves for a route (`Find::Body`, `Find::Conversion`).
fn resolved_by_middleware(find: Find, field: &str) -> bool {
    match find {
        Find::Body => matches!(field, "project_id" | "parent_object_id" | "scope_id"),
        Find::Conversion => matches!(field, "source_object_id" | "preview_id"),
        _ => false,
    }
}

#[test]
fn every_id_of_a_bot_reachable_body_is_accounted_for() {
    let sources = sources();
    let handlers = handlers();
    let mut unaccounted = BTreeSet::new();
    for entry in all_routes() {
        if matches!(entry.method, "GET" | "DELETE")
            || !matches!(entry.class, Class::Project(_) | Class::Own | Class::NoProject)
        {
            continue;
        }
        let key = (entry.method.to_string(), entry.pattern.to_string());
        let Some(handler) = handlers.get(&key) else {
            panic!("no handler found for {} {}", entry.method, entry.pattern);
        };
        let Some(signature) = signature(&sources, handler) else {
            if !ALIASED_HANDLERS.iter().any(|(name, _)| name == handler) {
                unaccounted.insert(format!(
                    "{} {}: no fn {handler} found (a re-exported alias?): list it in ALIASED_HANDLERS",
                    entry.method, entry.pattern
                ));
            }
            continue;
        };
        let Some(type_name) = json_type(signature) else {
            continue;
        };
        if matches!(type_name.as_str(), "Value") {
            assert!(
                UNTYPED_BODIES.iter().any(|(name, _)| name == handler),
                "{} {}: handler {handler} takes an untyped JSON body; list it in UNTYPED_BODIES with its reason",
                entry.method,
                entry.pattern
            );
            continue;
        }
        let fields = id_fields(&sources, &type_name)
            .unwrap_or_else(|| panic!("{} {}: no struct {type_name} found", entry.method, entry.pattern));
        for field in fields {
            let by_middleware = match entry.class {
                Class::Project(find) => resolved_by_middleware(find, &field),
                _ => false,
            };
            if !by_middleware && accounted(&type_name, &field).is_none() {
                unaccounted.insert(format!("{} {}: {type_name}.{field}", entry.method, entry.pattern));
            }
        }
    }
    assert!(
        unaccounted.is_empty(),
        "request body ids neither resolved by the middleware nor checked or exempted:\n{}",
        unaccounted.into_iter().collect::<Vec<_>>().join("\n")
    );
}

/// Flow command payloads: the `*Payload` structs of the command registry.
#[test]
fn every_id_of_a_flow_command_payload_is_accounted_for() {
    let sources = sources();
    let mut unaccounted = BTreeSet::new();
    for (path, text) in &sources {
        let relative = path.to_string_lossy();
        if !relative.contains("/src/flow/") || PAYLOAD_FILES_EXEMPT.iter().any(|(file, _)| relative.ends_with(file)) {
            continue;
        }
        for (at, _) in text.match_indices("Payload {") {
            let head = &text[..at];
            let name_start = head.rfind("struct ").map(|index| index + "struct ".len());
            let Some(name_start) = name_start else {
                continue;
            };
            let name = format!("{}Payload", &head[name_start..]);
            if name.contains(' ') || name.contains('\n') || name.contains('<') {
                continue;
            }
            for field in id_fields(&sources, &name).unwrap_or_default() {
                if field.contains("block_id") {
                    continue;
                }
                if accounted(&name, &field).is_none() {
                    unaccounted.insert(format!("{relative}: {name}.{field}"));
                }
            }
        }
    }
    assert!(
        unaccounted.is_empty(),
        "command payload ids neither checked nor exempted:\n{}",
        unaccounted.into_iter().collect::<Vec<_>>().join("\n")
    );
}

/// The entries this change leaves open are exactly the known ones: closing one removes it here,
/// opening a new one needs a conscious edit.
#[test]
fn the_open_entries_are_the_known_ones() {
    let open: Vec<String> = ACCOUNTED
        .iter()
        .filter(|(_, _, account)| matches!(account, Open(_)))
        .map(|(name, field, _)| format!("{name}.{field}"))
        .collect();
    assert_eq!(
        open,
        [
            "CreateProposalRequest.template_id",
            "CreateProposalRequest.workspace_id",
            "LinkIssueRequest.issue_id"
        ]
    );
}

/// A `Checked` entry is true: the file it names calls the helper; every entry names a real field.
#[test]
fn the_accounting_is_true() {
    let sources = sources();
    let src = Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/src"));
    for (name, field, account) in ACCOUNTED {
        let fields = id_fields(&sources, name).unwrap_or_else(|| panic!("no struct {name}"));
        assert!(fields.iter().any(|f| f == field), "{name} has no field {field}");
        match account {
            Checked(file) => {
                let text = sources.get(&src.join(file)).unwrap_or_else(|| panic!("no file {file}"));
                assert!(
                    text.contains("ensure_bot_level"),
                    "{name}.{field} is marked checked, but {file} never calls ensure_bot_level"
                );
            }
            Exempt(reason) | Open(reason) => assert!(!reason.is_empty(), "{name}.{field} needs a reason"),
            ScopeNative(handler) => {
                let text = sources
                    .values()
                    .find_map(|text| {
                        let at = text.find(&format!("fn {handler}("))?;
                        let rest = &text[at..];
                        Some(&rest[..rest.find("\n}\n").unwrap_or(rest.len())])
                    })
                    .unwrap_or_else(|| panic!("no fn {handler}"));
                assert!(
                    text.contains("user(&claims") || text.contains("user_or_absent(&claims"),
                    "{name}.{field} is marked scope-native, but {handler} does not call user()"
                );
            }
        }
    }
    for (handler, reason) in UNTYPED_BODIES {
        assert!(signature(&sources, handler).is_some(), "no fn {handler}");
        assert!(!reason.is_empty());
    }
}
