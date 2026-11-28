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

use super::scoped_bot::Class;
use crate::catalogue::{Find, bot_routes};

/// How an id field is accounted for.
enum Account {
    /// Not an object of another project: the reason says why.
    Exempt(&'static str),
    /// Checked through `ensure_bot_level`: `(file relative to src/, function, proof)`. The function's
    /// body must call the helper (or a thin wrapper listed in `CHECK_CALLS`) and `proof` names the
    /// test that replays the attack through the real handler and expects the refusal.
    Checked(&'static str, &'static str, &'static str),
    /// A scope-native handler (the named function calls `user()`, which refuses bot credentials).
    ScopeNative(&'static str),
    /// Not closed by this change: the field names something the handler may not confine to the
    /// bot's project. Listed so it stays visible; the reason says what is known.
    Open(&'static str),
}
use Account::{Checked, Exempt, Open, ScopeNative};

/// Calls that hold a scoped bot to its project level: the helper and the thin wrappers around it.
const CHECK_CALLS: &[&str] = &[
    "ensure_bot_level(",
    "ensure_actor_level(",
    "ensure_relation_end(",
    "require_read(",
    "require_write(",
];

/// Why a `Value` field is listed as open: its content is stored or interpreted by code that has not
/// been read for object ids it might resolve (a form schema's relation targets, a layout's bindings).
/// Follow-up of audit item 2; each entry closes by reading that code, then becomes `Exempt` or `Checked`.
const UNREAD_VALUE: &str =
    "an untyped JSON value: the code that interprets it has not been read for ids it resolves (audit re-check item 2)";

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
    (
        "CreateReferenceInput",
        "target_id",
        Checked(
            "flow/bridge.rs",
            "create_reference",
            "a_reference_target_in_another_project_is_refused",
        ),
    ),
    // Flow commands (flow/command.rs, move_object.rs, relations.rs).
    (
        "MoveObjectPayload",
        "target_object_id",
        Checked(
            "flow/move_object.rs",
            "execute_on",
            "a_move_destination_needs_write_in_its_project",
        ),
    ),
    (
        "MoveObjectPayload",
        "after_id",
        Exempt("a sibling entry of the destination navigator, which is checked"),
    ),
    (
        "LinkPayload",
        "target_object_id",
        Checked(
            "flow/relations.rs",
            "ensure_relation_end",
            "a_relation_to_a_page_of_another_project_is_refused",
        ),
    ),
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
    (
        "BodyAssetCopy",
        "source_object_id",
        Checked(
            "routes/flow_v2.rs",
            "post_flow_v2_body_asset_copy",
            "an_asset_copy_source_in_another_project_is_refused",
        ),
    ),
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
        Checked(
            "routes/proposal.rs",
            "create_proposal",
            "a_proposal_template_of_an_unreadable_project_is_refused",
        ),
    ),
    (
        "CreateProposalRequest",
        "workspace_id",
        Exempt(
            "resolve_new_proposal_workspace accepts only a workspace the caller is a member of; a bot is a member of its own workspace only (audit re-check)",
        ),
    ),
    (
        "LinkIssueRequest",
        "issue_id",
        Checked(
            "routes/proposal.rs",
            "link_issue_to_proposal",
            "a_proposal_links_only_an_issue_of_a_readable_project",
        ),
    ),
    // Query parameters of GET routes (re-audit: the scan covers `Query<T>` too).
    (
        "ListDraftsQuery",
        "project_id",
        Exempt("list_drafts: resolve_project then require_read (which applies the bot's level to that project)"),
    ),
    (
        "ListDraftsQuery",
        "workspace_id",
        Exempt("list_drafts: member() refuses a bot outside its own workspace; rows are limited to readable_projects"),
    ),
    (
        "ListLayoutsQuery",
        "scope_id",
        Checked(
            "layouts/handlers.rs",
            "list_layouts",
            "layout_scopes_of_another_project_are_refused",
        ),
    ),
    (
        "ExportAttachmentPackageQuery",
        "view_id",
        Exempt("find_readable_view refuses a view of another form (routes/form.rs)"),
    ),
    (
        "ListAttachmentsQuery",
        "record_id",
        Exempt("a filter ANDed with form_id = the route's form: a record of another form matches nothing"),
    ),
    (
        "ExportRecordsQuery",
        "view_id",
        Exempt("find_readable_view refuses a view of another form (routes/form.rs)"),
    ),
    (
        "ListRecordsQuery",
        "view_id",
        Exempt("record_query_config_from_view refuses a view of another form (routes/form.rs)"),
    ),
    (
        "ByIdentifierQuery",
        "workspace_id",
        Exempt("a bot looks in its own workspace only; a different workspace_id answers not found"),
    ),
    (
        "ListIssuesQuery",
        "assignee_id",
        Exempt("a filter ANDed with project_id = the route's project"),
    ),
    (
        "ListIssuesQuery",
        "label_ids",
        Exempt("a filter ANDed with project_id = the route's project"),
    ),
    (
        "ListIssuesQuery",
        "milestone_id",
        Exempt("a filter ANDed with project_id = the route's project"),
    ),
    (
        "ListIssuesQuery",
        "sprint_id",
        Exempt("a filter ANDed with project_id = the route's project"),
    ),
    (
        "ListActivitiesQuery",
        "resource_id",
        Exempt("a filter on top of the route's project clause (activities of the project and its issues)"),
    ),
    (
        "ResolveQuery",
        "form_id",
        Exempt("bind_context refuses a form that is not in the route's project (layouts/placements.rs)"),
    ),
    (
        "ResolveQuery",
        "record_id",
        Exempt("bind_context refuses a record that is not in that form (layouts/placements.rs)"),
    ),
    (
        "BlockShareContextQuery",
        "project_id",
        ScopeNative("get_flow_v2_block_share"),
    ),
    (
        "BlockShareContextQuery",
        "workspace_id",
        ScopeNative("get_flow_v2_block_share"),
    ),
    (
        "GetCollectionRecordsQuery",
        "field_ids",
        Exempt("field ids of the collection of the route's own object"),
    ),
    (
        "GetCollectionRecordsQuery",
        "group",
        Exempt("a field id of the collection of the route's own object"),
    ),
    (
        "GetCollectionRecordsQuery",
        "view_id",
        Exempt("apply_saved_view reads the view by (collection_id, view_id) of the route's own object"),
    ),
    (
        "ScopeListQuery",
        "parent_id",
        Exempt(
            "the middleware requires and judges project_id; the listing ANDs it with parent_id, so only rows of the judged project return",
        ),
    ),
    // Fields typed `Value` that the ids scan cannot see into.
    (
        "ConversionPreviewInput",
        "mapping",
        Checked(
            "flow/bridge.rs",
            "ensure_conversion_target",
            "a_conversion_target_in_another_project_is_refused",
        ),
    ),
    (
        "RecordQueryPayload",
        "group",
        Exempt("a field id of the collection of the route's own object"),
    ),
    (
        "LinkPayload",
        "properties",
        Exempt("properties stored with the relation; the far end is the checked target_object_id"),
    ),
    (
        "NewComment",
        "mentions",
        Exempt("user ids, not project objects (audit F5)"),
    ),
    (
        "ScopeCommentPatch",
        "mentions",
        Exempt("user ids, not project objects (audit F5)"),
    ),
    (
        "CreateProposalCommentRequest",
        "mentions",
        Exempt("user ids, not project objects (audit F5)"),
    ),
    // Values the scan cannot see into (re-audit item 2); not yet read for embedded ids.
    ("UpdateChildRecordRequest", "source", Open(UNREAD_VALUE)),
    ("UpdateChildRecordRequest", "values", Open(UNREAD_VALUE)),
    ("UpdateRecordRequest", "source", Open(UNREAD_VALUE)),
    ("UpdateRecordRequest", "values", Open(UNREAD_VALUE)),
    ("UpdateViewRequest", "config", Open(UNREAD_VALUE)),
    ("UpdateFormRequest", "detail_layout", Open(UNREAD_VALUE)),
    ("UpdateFormRequest", "schema", Open(UNREAD_VALUE)),
    ("UpdateLayoutRequest", "spec", Open(UNREAD_VALUE)),
    ("UpdateProjectResourceRequest", "locator", Open(UNREAD_VALUE)),
    ("UpdateProjectResourceRequest", "permission_policy", Open(UNREAD_VALUE)),
    ("UpdateImpactReviewRequest", "data_sources", Open(UNREAD_VALUE)),
    ("UpdateImpactReviewRequest", "goal_achievements", Open(UNREAD_VALUE)),
    ("UpdateImpactReviewRequest", "metrics", Open(UNREAD_VALUE)),
    ("PageRequest", "layout_spec", Open(UNREAD_VALUE)),
    ("PageRequest", "nav", Open(UNREAD_VALUE)),
    (
        "UpdateSiteRequest",
        "title",
        Exempt("the site title text, never resolved as an object"),
    ),
    ("CreateDraftRequest", "spec", Open(UNREAD_VALUE)),
    (
        "CreateDraftRequest",
        "target",
        Exempt(
            "form_candidate and layout_candidate require the form or scope to lie in the request's own project (resolved by the middleware from project_id)",
        ),
    ),
    ("CreateChildRecordRequest", "metadata", Open(UNREAD_VALUE)),
    ("CreateChildRecordRequest", "source", Open(UNREAD_VALUE)),
    ("CreateChildRecordRequest", "values", Open(UNREAD_VALUE)),
    ("CreateRecordCommentRequest", "metadata", Open(UNREAD_VALUE)),
    ("CreateLinkRequest", "metadata", Open(UNREAD_VALUE)),
    ("ValidateRulesRequest", "default", Open(UNREAD_VALUE)),
    ("ValidateRulesRequest", "options", Open(UNREAD_VALUE)),
    ("ValidateRulesRequest", "rules", Open(UNREAD_VALUE)),
    ("ValidateRulesRequest", "samples", Open(UNREAD_VALUE)),
    ("ValidateSchemaRequest", "schema", Open(UNREAD_VALUE)),
    (
        "SaveImportMappingTemplateRequest",
        "id",
        Exempt("refused unless the template belongs to the route's form (routes/form.rs)"),
    ),
    ("RecalculatePreviewRequest", "values", Open(UNREAD_VALUE)),
    ("CreateRecordRequest", "source", Open(UNREAD_VALUE)),
    ("CreateRecordRequest", "values", Open(UNREAD_VALUE)),
    ("ValidateRecordRequest", "schema", Open(UNREAD_VALUE)),
    ("ValidateRecordRequest", "values", Open(UNREAD_VALUE)),
    ("CreateViewRequest", "config", Open(UNREAD_VALUE)),
    ("ValidateRequest", "spec", Open(UNREAD_VALUE)),
    ("CreateLayoutRequest", "spec", Open(UNREAD_VALUE)),
    ("InvokePluginRequest", "input", Open(UNREAD_VALUE)),
    ("CreateAiTaskRequest", "payload", Open(UNREAD_VALUE)),
    ("CreateCheckResultRequest", "result", Open(UNREAD_VALUE)),
    ("CreateFormRequest", "detail_layout", Open(UNREAD_VALUE)),
    ("CreateFormRequest", "schema", Open(UNREAD_VALUE)),
    ("InstallPluginRequest", "manifest", Open(UNREAD_VALUE)),
    ("CreateProjectResourceRequest", "locator", Open(UNREAD_VALUE)),
    ("CreateProjectResourceRequest", "permission_policy", Open(UNREAD_VALUE)),
    (
        "CreateSiteRequest",
        "title",
        Exempt("the site title text, never resolved as an object"),
    ),
    (
        "CreateReferenceInput",
        "display",
        Exempt("display text stored with the reference; the target is the checked target_id"),
    ),
    ("scope::CreateInput", "initial_fields", Open(UNREAD_VALUE)),
    ("scope::CreateInput", "initial_view", Open(UNREAD_VALUE)),
    ("UpdateProjectRequest", "type_settings", Open(UNREAD_VALUE)),
    ("PutConfig", "doc", Open(UNREAD_VALUE)),
    ("UpdateProposalTemplateRequest", "content", Open(UNREAD_VALUE)),
    // Layout and placement creation: the handler resolves the scope and requires write in its project.
    (
        "CreateLayoutRequest",
        "scope_id",
        Checked(
            "layouts/handlers.rs",
            "create_layout",
            "layout_scopes_of_another_project_are_refused",
        ),
    ),
    (
        "CreatePlacementRequest",
        "scope_id",
        Checked(
            "layouts/placements.rs",
            "create_placement",
            "layout_scopes_of_another_project_are_refused",
        ),
    ),
    (
        "CreatePlacementRequest",
        "layout_id",
        Exempt("create_placement accepts only a layout that lies inside the placement's scope, which is bot-checked"),
    ),
    (
        "CreatePlacementRequest",
        "slot_id",
        Exempt("a slot name of the fixed catalogue, not an object of any project"),
    ),
    // Layout validator (layouts/handlers.rs): the scope is resolved and read-checked by the handler.
    (
        "ValidateRequest",
        "scope_id",
        Checked(
            "layouts/handlers.rs",
            "validate_layout",
            "layout_scopes_of_another_project_are_refused",
        ),
    ),
    (
        "ValidateRecordRequest",
        "record_id",
        Exempt(
            "the handler refuses a record of another form (record.form_id != the route's form, 404); the form is the route's resolved target",
        ),
    ),
    (
        "ValidateRequest",
        "site_id",
        Exempt("the handler resolves the site's project and requires read access to it (not_found otherwise)"),
    ),
    // Site pages (site/admin.rs, site/drafts.rs): the route's site is the resolved target.
    (
        "PageRequest",
        "layout_id",
        Exempt(
            "check_page_layout requires an active user layout of the site's workspace at the site's scope or in the site's project or one of its forms (not_found otherwise); bot drafts are judged by the same check",
        ),
    ),
    (
        "ValidateRequest",
        "slot_id",
        Exempt("a slot name of the fixed catalogue, not an object of any project"),
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
        "export_portable_page",
        "routes/flow_portable_page.rs: user() refuses bot credentials",
    ),
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

/// The text of `fn name(` up to the end of its parameter list, and the file it is in.
fn signature_in<'a>(sources: &'a BTreeMap<PathBuf, String>, name: &str) -> Option<(&'a Path, &'a str)> {
    let needle = format!("fn {name}(");
    sources.iter().find_map(|(path, text)| {
        let at = text.find(&needle)?;
        let rest = &text[at..];
        let end = rest.find(") ->").or_else(|| rest.find("){"))?;
        Some((path.as_path(), &rest[..end]))
    })
}

fn signature<'a>(sources: &'a BTreeMap<PathBuf, String>, name: &str) -> Option<&'a str> {
    signature_in(sources, name).map(|(_, signature)| signature)
}

/// The last path segment of the type inside `wrapper` (`Json<`, `Query<`) in a parameter list.
fn wrapped_type(signature: &str, wrapper: &str) -> Option<String> {
    let at = signature.find(wrapper)?;
    let rest = &signature[at + wrapper.len()..];
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
    Some(rest[..end].trim().to_string())
}

/// `(module, name)` of a type path: `crate::flow::scope::CreateInput` is `(Some("scope"), "CreateInput")`.
fn split_path(path: &str) -> (Option<&str>, &str) {
    let mut parts = path.rsplit("::");
    let name = parts.next().unwrap_or(path);
    (parts.next(), name)
}

/// A field of a request struct as it is written in the source.
struct Field {
    /// The Rust name.
    name: String,
    /// Every name the wire knows it by: the Rust name, a `rename` and the `alias`es.
    wire: Vec<String>,
    /// The type as written.
    ty: String,
    flatten: bool,
}

impl Field {
    /// A field that can name an object: an id by name (Rust or wire), any `Uuid`, or an untyped
    /// `Value`, which can carry ids the scan cannot see (the conversion `mapping`).
    fn names_an_object(&self) -> bool {
        self.wire
            .iter()
            .any(|name| name.ends_with("_id") || name.ends_with("_ids"))
            || self.ty.contains("Uuid")
            || self.ty.contains("Value")
    }
}

/// The text between the braces of every `struct name {` in the sources, with its file.
fn struct_bodies<'a>(sources: &'a BTreeMap<PathBuf, String>, name: &str) -> Vec<(&'a Path, &'a str)> {
    let needle = format!("struct {name} {{");
    sources
        .iter()
        .filter_map(|(path, text)| {
            let at = text.find(&needle)?;
            let rest = &text[at + needle.len()..];
            Some((path.as_path(), &rest[..rest.find("\n}")?]))
        })
        .collect()
}

/// The quoted values after `key = ` in a serde attribute line.
fn serde_names(attribute: &str, key: &str) -> Vec<String> {
    attribute
        .match_indices(&format!("{key} = \""))
        .filter_map(|(at, matched)| {
            let rest = &attribute[at + matched.len()..];
            Some(rest[..rest.find('"')?].to_string())
        })
        .collect()
}

/// The fields written in the body of a struct.
fn parse_fields(body: &str) -> Vec<Field> {
    let mut fields = Vec::new();
    let mut attributes = String::new();
    for line in body.lines() {
        let line = line.trim();
        if line.starts_with("#[") {
            attributes.push_str(line);
            continue;
        }
        let line = line
            .trim_start_matches("pub(crate) ")
            .trim_start_matches("pub(super) ")
            .trim_start_matches("pub ");
        let Some((field, ty)) = line.split_once(':') else {
            continue;
        };
        if field.contains(' ') || field.starts_with("//") {
            attributes.clear();
            continue;
        }
        let mut wire = vec![field.to_string()];
        wire.extend(serde_names(&attributes, "rename"));
        wire.extend(serde_names(&attributes, "alias"));
        fields.push(Field {
            name: field.to_string(),
            wire,
            ty: ty.trim().trim_end_matches(',').to_string(),
            flatten: attributes.contains("flatten"),
        });
        attributes.clear();
    }
    fields
}

/// The fields of `struct name { ... }`, with `#[serde(flatten)]` fields replaced by the fields of
/// their own type. Panics when two files define a struct of that name: the scan would read the
/// wrong one (audit re-check, collisions); rename one of them.
fn fields_of(sources: &BTreeMap<PathBuf, String>, name: &str) -> Option<Vec<Field>> {
    fields_near(sources, name, None, None)
}

/// Like [`fields_of`], but when several files define the name, the definition in `home` (the file of
/// the handler that takes the type) is the one meant.
fn fields_near(
    sources: &BTreeMap<PathBuf, String>,
    name: &str,
    home: Option<&Path>,
    module: Option<&str>,
) -> Option<Vec<Field>> {
    let mut bodies = struct_bodies(sources, name);
    if bodies.len() > 1
        && let Some(module) = module
    {
        // The type is written with its module (`scope::CreateInput`): the file of that module.
        let in_module = |path: &Path| {
            let stem = path.file_stem().and_then(|stem| stem.to_str());
            let parent = path.parent().and_then(Path::file_name).and_then(|name| name.to_str());
            stem == Some(module) || (stem == Some("mod") && parent == Some(module))
        };
        let named: Vec<_> = bodies.iter().copied().filter(|(path, _)| in_module(path)).collect();
        if !named.is_empty() {
            bodies = named;
        }
    }
    if bodies.len() > 1
        && let Some(home) = home
    {
        let own: Vec<_> = bodies.iter().copied().filter(|(path, _)| *path == home).collect();
        if !own.is_empty() {
            bodies = own;
        }
    }
    if bodies.len() > 1
        && let Some(text) = home.and_then(|home| sources.get(home))
    {
        // An unqualified name the handler's file imports from a module (`use crate::response::PageQuery`).
        let imported: Vec<_> = bodies
            .iter()
            .copied()
            .filter(|(path, _)| {
                path.file_stem()
                    .and_then(|stem| stem.to_str())
                    .is_some_and(|stem| text.contains(&format!("{stem}::")))
            })
            .collect();
        if !imported.is_empty() {
            bodies = imported;
        }
    }
    let mut parsed: Vec<Vec<Field>> = bodies.iter().map(|(_, body)| parse_fields(body)).collect();
    // Several definitions left: harmless only when they read the same.
    let same = parsed.windows(2).all(|pair| {
        pair[0].len() == pair[1].len()
            && pair[0]
                .iter()
                .zip(&pair[1])
                .all(|(a, b)| a.name == b.name && a.ty == b.ty && a.wire == b.wire)
    });
    assert!(
        same,
        "struct {name} is defined in several files ({}) with different fields: the scan cannot tell them apart; rename one",
        bodies
            .iter()
            .map(|(path, _)| path.display().to_string())
            .collect::<Vec<_>>()
            .join(", ")
    );
    if parsed.is_empty() {
        return None;
    }
    let fields = parsed.swap_remove(0);
    // A flattened type is resolved from the file of the struct that flattens it.
    let home = home.or_else(|| bodies.first().map(|(path, _)| *path));
    let mut out = Vec::new();
    for field in fields {
        if field.flatten {
            let inner = field.ty.trim_start_matches("Option<").trim_end_matches('>');
            out.extend(fields_near(sources, inner, home, None).unwrap_or_default());
        } else {
            out.push(field);
        }
    }
    Some(out)
}

/// The names of the fields of `name` that can name an object.
fn id_fields(
    sources: &BTreeMap<PathBuf, String>,
    name: &str,
    home: Option<&Path>,
    module: Option<&str>,
) -> Option<Vec<String>> {
    Some(
        fields_near(sources, name, home, module)?
            .into_iter()
            .filter(Field::names_an_object)
            .map(|field| field.name)
            .collect(),
    )
}

/// The account of `struct_name.field`; `module` (`scope` for `scope::CreateInput`) picks the entry
/// when two modules define a struct of the same name and the table keys it as `module::Name`.
fn accounted(struct_name: &str, module: Option<&str>, field: &str) -> Option<&'static Account> {
    let qualified = module.map(|module| format!("{module}::{struct_name}"));
    ACCOUNTED
        .iter()
        .find(|(name, f, _)| *f == field && (*name == struct_name || qualified.as_deref() == Some(*name)))
        .map(|(_, _, account)| account)
}

/// The struct and module of a table key (`scope::CreateInput` is `scope`, `CreateInput`).
fn key_parts(key: &str) -> (Option<&str>, &str) {
    split_path(key)
}

/// Where a field is read from.
#[derive(Clone, Copy, PartialEq, Eq)]
enum From {
    Body,
    Query,
}

/// Whether the middleware resolves this field as the route's target (`Find::Body`, `Find::Conversion`
/// read the body, `Find::Query` reads one query parameter).
fn resolved_by_middleware(find: Find, from: From, field: &str) -> bool {
    match (find, from) {
        (Find::Body, From::Body) => matches!(field, "project_id" | "parent_object_id" | "scope_id"),
        (Find::Conversion, From::Body) => matches!(field, "source_object_id" | "preview_id"),
        (Find::Query(name), From::Query) => field == name,
        _ => false,
    }
}

/// The body of `fn name(` (to the closing brace in column 0, or the end of the file).
fn function_body<'a>(text: &'a str, name: &str) -> Option<&'a str> {
    let at = text.find(&format!("fn {name}("))?;
    let rest = &text[at..];
    Some(&rest[..rest.find("\n}\n").unwrap_or(rest.len())])
}

#[test]
fn every_id_of_a_bot_reachable_request_is_accounted_for() {
    let sources = sources();
    let handlers = handlers();
    let mut unaccounted = BTreeSet::new();
    for entry in bot_routes() {
        if !matches!(
            entry.class,
            Class::Project(_) | Class::Own | Class::NoProject | Class::Filtered
        ) {
            continue;
        }
        let key = (entry.method.to_string(), entry.pattern.to_string());
        let Some(handler) = handlers.get(&key) else {
            panic!("no handler found for {} {}", entry.method, entry.pattern);
        };
        let Some((home, signature)) = signature_in(&sources, handler) else {
            if !ALIASED_HANDLERS.iter().any(|(name, _)| name == handler) {
                unaccounted.insert(format!(
                    "{} {}: no fn {handler} found (a re-exported alias?): list it in ALIASED_HANDLERS",
                    entry.method, entry.pattern
                ));
            }
            continue;
        };
        for (wrapper, from) in [("Json<", From::Body), ("Query<", From::Query)] {
            let Some(type_path) = wrapped_type(signature, wrapper) else {
                continue;
            };
            let (module, type_name) = split_path(&type_path);
            let untyped = |unaccounted: &mut BTreeSet<String>, what: &str| {
                if !UNTYPED_BODIES.iter().any(|(name, _)| name == handler) {
                    unaccounted.insert(format!(
                        "{} {}: {what} {wrapper}{type_name}>; list handler {handler} in UNTYPED_BODIES with its reason",
                        entry.method, entry.pattern
                    ));
                }
            };
            if !type_name.chars().next().is_some_and(char::is_uppercase) || type_name == "Value" {
                untyped(&mut unaccounted, "untyped");
                continue;
            }
            let Some(fields) = id_fields(&sources, type_name, Some(home), module) else {
                untyped(&mut unaccounted, "no struct found for");
                continue;
            };
            for field in fields {
                let by_middleware = match entry.class {
                    Class::Project(find) => resolved_by_middleware(find, from, &field),
                    _ => false,
                };
                if !by_middleware && accounted(type_name, module, &field).is_none() {
                    unaccounted.insert(format!("{} {}: {type_name}.{field}", entry.method, entry.pattern));
                }
            }
        }
    }
    assert!(
        unaccounted.is_empty(),
        "request ids neither resolved by the middleware nor checked or exempted:\n{}",
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
            for field in id_fields(&sources, &name, Some(path), None).unwrap_or_default() {
                if field.contains("block_id") {
                    continue;
                }
                if accounted(&name, None, &field).is_none() {
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

/// How many `Value` fields are still open for reading; closing one lowers it, opening a new one
/// needs a conscious edit.
const KNOWN_OPEN: usize = 47;

/// Nothing stays open without a written reason: an open entry is an untyped `Value` field with
/// `UNREAD_VALUE`, and no other kind of field is open.
#[test]
fn the_open_entries_are_untyped_values_and_counted() {
    let sources = sources();
    let mut open = 0;
    for (name, field, account) in ACCOUNTED {
        if let Open(reason) = account {
            open += 1;
            assert_eq!(
                *reason, UNREAD_VALUE,
                "{name}.{field}: an open entry names the unread value"
            );
            let (module, bare) = key_parts(name);
            let fields = fields_near(&sources, bare, None, module).unwrap_or_else(|| panic!("no struct {name}"));
            assert!(
                fields.iter().any(|f| f.name == *field && f.ty.contains("Value")),
                "{name}.{field} is open but is not a Value field"
            );
        }
    }
    assert_eq!(open, KNOWN_OPEN);
}

/// Whether `name` is a test function in the sources.
fn is_test(sources: &BTreeMap<PathBuf, String>, name: &str) -> bool {
    sources.values().any(|text| {
        text.find(&format!("fn {name}(")).is_some_and(|at| {
            let before = &text[..at];
            let line_start = before.rfind("\n\n").unwrap_or(0);
            before[line_start..].contains("#[tokio::test]") || before[line_start..].contains("#[test]")
        })
    })
}

/// A `Checked` entry is true, field by field: the named function calls the helper (or a wrapper of
/// it), and the named test replays the attack through the real handler and names the field. Every
/// entry names a real field.
#[test]
fn the_accounting_is_true() {
    let sources = sources();
    let src = Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/src"));
    for (name, field, account) in ACCOUNTED {
        let (module, bare) = key_parts(name);
        let fields = fields_near(&sources, bare, None, module).unwrap_or_else(|| panic!("no struct {name}"));
        assert!(fields.iter().any(|f| f.name == *field), "{name} has no field {field}");
        match account {
            Checked(file, function, proof) => {
                let text = sources.get(&src.join(file)).unwrap_or_else(|| panic!("no file {file}"));
                let body = function_body(text, function).unwrap_or_else(|| panic!("no fn {function} in {file}"));
                assert!(
                    CHECK_CALLS.iter().any(|call| body.contains(call)),
                    "{name}.{field} is marked checked, but {file}::{function} calls none of {CHECK_CALLS:?}"
                );
                assert!(is_test(&sources, proof), "{name}.{field}: no test {proof}");
                let proof_body = sources
                    .values()
                    .find_map(|text| function_body(text, proof))
                    .unwrap_or_else(|| panic!("no test {proof}"));
                assert!(
                    proof_body.contains(field),
                    "{name}.{field}: the proof test {proof} never names the field"
                );
            }
            Exempt(reason) | Open(reason) => assert!(!reason.is_empty(), "{name}.{field} needs a reason"),
            ScopeNative(handler) => {
                let text = sources
                    .values()
                    .find_map(|text| function_body(text, handler))
                    .unwrap_or_else(|| panic!("no fn {handler}"));
                assert!(
                    text.contains("user(&claims") || text.contains("user_or_absent(&claims"),
                    "{name}.{field} is marked scope-native, but {handler} does not call user()"
                );
            }
        }
    }
    // The layout helpers the handlers lean on hold the bot to its project level themselves.
    let access = sources.get(&src.join("layouts/access.rs")).expect("layouts/access.rs");
    for function in ["require_read", "require_write"] {
        assert!(
            function_body(access, function).is_some_and(|body| body.contains("ensure_bot_level(")),
            "{function} does not call ensure_bot_level"
        );
    }
    for (handler, reason) in UNTYPED_BODIES {
        assert!(signature(&sources, handler).is_some(), "no fn {handler}");
        assert!(!reason.is_empty());
    }
}

/// Two request types of one name would be read as one (the scan finds a struct by name): a type a
/// bot-reachable handler takes may share its name with another struct only when the handler's own
/// file defines it, and every struct of `ACCOUNTED` (keyed by name) must be defined once.
#[test]
fn request_type_names_are_unique() {
    let sources = sources();
    let handlers = handlers();
    for entry in bot_routes() {
        let Some((home, signature)) = handlers
            .get(&(entry.method.to_string(), entry.pattern.to_string()))
            .and_then(|handler| signature_in(&sources, handler))
        else {
            continue;
        };
        for wrapper in ["Json<", "Query<"] {
            if let Some(type_path) = wrapped_type(signature, wrapper) {
                let (module, type_name) = split_path(&type_path);
                // Panics when the name is defined twice and neither the module nor the handler's
                // file picks one.
                let _ = fields_near(&sources, type_name, Some(home), module);
            }
        }
    }
    for (name, _, _) in ACCOUNTED {
        let (module, bare) = key_parts(name);
        let _ = fields_near(&sources, bare, None, module);
    }
}

/// The scan itself: an id in a query string, a renamed id, a flattened struct and an id in a `Value`
/// are all seen.
#[test]
fn the_scan_sees_query_ids_renames_flattened_fields_and_values() {
    let mut sources = BTreeMap::new();
    sources.insert(
        PathBuf::from("a.rs"),
        "struct Inner {\n    owner_id: Uuid,\n}\nstruct Probe {\n    #[serde(rename = \"projectId\", alias = \"scope_id\")]\n    project: String,\n    #[serde(flatten)]\n    inner: Inner,\n    mapping: serde_json::Value,\n    after: Option<Uuid>,\n    title: String,\n}\n"
            .to_string(),
    );
    let seen = id_fields(&sources, "Probe", None, None).expect("probe");
    assert_eq!(seen, ["project", "owner_id", "mapping", "after"]);
    sources.insert(PathBuf::from("b.rs"), "struct Probe {\n    x: u8,\n}\n".to_string());
    assert!(std::panic::catch_unwind(|| fields_of(&sources, "Probe")).is_err());
}
