use crate::client::{
    ApiErrorClass, ListRecordsQuery, ListWorkItemsQuery, OpenPrClient, classify_api_error, is_backend_failure,
};
use crate::era::{self, CachePolicy, CacheScope, Era};
use crate::guides;
use crate::listen::{SubKey, Subscribable, Verdict};
use crate::oauth::{DelegatedGrant, ToolRefusal, gate_tool_call, is_static_guide_resource};
use crate::protocol::{CallToolParams, CallToolResult, JsonRpcError, JsonRpcRequest, JsonRpcResponse, ListToolsResult};
use crate::tools;
use platform::config::McpToolMode;
use serde_json::{Value, json};
use std::collections::{BTreeSet, HashMap};
use std::sync::Arc;
use tokio::sync::Mutex;
use uuid::Uuid;

/// How a single `tools/call` is bound to a project agent policy.
///
/// The scope is taken from the *registered tool schema*, never from the arguments of
/// the call, because the arguments are attacker controlled: a caller that could turn
/// `work_items.delete` into a "declared project" tool simply by adding a `project_id`
/// would pick the policy that authorizes it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PolicyScope {
    /// The request itself is addressed by `project_id` (`/api/v1/projects/{id}/...`),
    /// so the declared value *is* the call target and is authoritative — once it has
    /// been validated as a canonical UUID.
    DeclaredProject { required: bool },
    /// The request is addressed by a resource id. The owning project is read back from
    /// the API; a `project_id` in the arguments is never the policy subject and any
    /// disagreement with the real owner refuses the call.
    OwnedBy(OwnerLookup),
    /// Nothing the call touches belongs to a project: workspace wide surfaces
    /// (workspace labels, workspace search, members, uploads) and global catalogues
    /// (project types). No project agent policy can govern these;
    /// see `report`/`README` notes and `TOOL_POLICY_SCOPES` for the full list.
    WorkspaceWide,
    /// `WorkspaceWide`, plus the call is a Flow administrative operation
    /// (`mcp-surface-v1.md`: "WorkspaceWide(admin)") — a workspace rollout flag change or a
    /// workspace-wide import or export step. Like `WorkspaceWide`, no *project* agent policy can
    /// govern these, so `resolve_policy_project_id` answers `Ok(None)` for both the same
    /// way. The variant exists as its own name, not a bool on `WorkspaceWide`, only so a
    /// tool's admin intent is visible at the `TOOL_POLICY_SCOPES` call site and in the
    /// coverage tests below.
    ///
    /// What this scope does *not* itself do: `mcp-surface-v1.md` specifies that
    /// `WorkspaceWide(admin)` "要求当前 bot 同时具有 Flow admin capability、exact tool policy
    /// 与 route workspace admin policy". `apps/api` does carry a real bot-capability model for
    /// this — `middleware::bot_auth::BotPermission::{Read,Write,Admin}`, stored per bot on
    /// `workspace_bots.permissions` — and every Flow admin REST endpoint checked here
    /// (`PUT .../features/flow`, `.../collab/verify` at `write`, the import
    /// and export admin routes) already enforces it: a bot without the right permission gets a real `403`, not
    /// a fiction, as confirmed against a live `apps/api` instance while building this change.
    /// What is missing is purely a *pre-check*: the MCP server has no endpoint that lets it
    /// introspect a caller's own bot permissions before dispatch, and no existing tool in this
    /// registry pre-filters `tools/list` by bot capability either (the only filter is
    /// `is_tool_enabled_by_policy`, which is keyed by *project* agent policy and does not apply
    /// to workspace-scoped tools at all) — so `WorkspaceWideAdmin`'s enforcement at this layer
    /// is the same defense in depth `WorkspaceWide` already does (refusing an ownership-bearing
    /// argument a workspace-wide tool cannot legitimately receive), and the real admit/refuse
    /// decision is, consistently with every other admin-gated surface in this codebase, made
    /// once by the REST endpoint the call actually reaches.
    WorkspaceWideAdmin,
    /// A read-only integrity probe whose schema selects exactly one workspace or Flow object.
    WorkspaceOrFlowObject,
}

/// The API read that maps a resource id to its owning project.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum OwnerLookup {
    /// `record_id` / `form_id` / `attachment_id`.
    FormData,
    /// `work_item_id` -> `GET /api/v1/issues/{id}`.
    WorkItem,
    /// `identifier` -> `GET /api/v1/issues/by-identifier/{identifier}`.
    WorkItemIdentifier,
    /// `plugin_id` -> `GET /api/v1/plugins/{id}`.
    Plugin,
    /// `check_result_id` -> `GET /api/v1/check-results/{id}`.
    CheckResult,
    /// `sprint_id` -> `GET /api/v1/sprints/{id}`.
    Sprint,
    /// `comment_id` -> `GET /api/v1/comments/{id}`.
    Comment,
    /// `object_id` -> `GET /api/v2/flow/objects/{id}`.
    ///
    /// Unlike every other lookup here, the owner this returns is *optional*: a Flow object
    /// may belong to no project at all (`v0.4-flow-alpha.md`'s `flow_objects.project_id` is
    /// nullable), which `resolve_policy_project_id` handles as its own match arm rather than
    /// through the generic `resolve_owning_project` path every mandatory-owner lookup shares
    /// (`mcp-surface-v1.md` describes this as `project_id=None` falling back to
    /// `WorkspaceWide`). `mcp-surface-v1.md` also
    /// names `source_object_id` and `collection_id` as owner-bearing keys this lookup covers;
    /// only `object_id` is wired here because it is the only key any v0.4 tool declares
    /// (`objects.get`, `objects.history`) — `source_object_id` (move/link) and `collection_id`
    /// (Collections) belong to v0.5/v0.6 tools this change does not register, and adding
    /// unreachable match arms for them now would be exactly the untested surface the seven
    /// iron rules ban. Extend [`OwnerLookup::argument`] and
    /// [`McpServer::resolve_flow_object_owner`] when those tools land.
    FlowObject,
    /// `preview_id` / `job_id` -> the conversion's source Flow object owner endpoint.
    FlowConversion,
    /// `task_id` (with `attempt`) -> `GET /api/v1/ai/tasks/{id}?attempt=N`, whose view names the
    /// project. The endpoint answers only the task's own bot, so a task of another bot never
    /// resolves.
    AiTask,
}

impl OwnerLookup {
    /// The argument this lookup is keyed by, for error messages.
    const fn argument(self) -> &'static str {
        match self {
            Self::FormData => "form_id, record_id or attachment_id",
            Self::WorkItem => "work_item_id",
            Self::WorkItemIdentifier => "identifier",
            Self::Plugin => "plugin_id",
            Self::CheckResult => "check_result_id",
            Self::Sprint => "sprint_id",
            Self::Comment => "comment_id",
            Self::FlowObject => "object_id, source_object_id or collection_id",
            Self::FlowConversion => "preview_id or job_id",
            Self::AiTask => "task_id",
        }
    }
}

/// Ownership bearing arguments in resolution order. A tool without `project_id` is
/// governed through the first of these its schema declares.
///
/// `proposal_id`, `label_id` and `resource_id` are deliberately absent: proposals and
/// labels carry no project column (`migrations/0012_governance_phase1.sql`,
/// `migrations/0003_labels.sql`), and `resource_id` only ever appears next to a
/// `project_id` that already scopes the request path.
const OWNERSHIP_ARGUMENTS: [(&str, OwnerLookup); 15] = [
    ("record_id", OwnerLookup::FormData),
    ("form_id", OwnerLookup::FormData),
    ("attachment_id", OwnerLookup::FormData),
    ("work_item_id", OwnerLookup::WorkItem),
    ("identifier", OwnerLookup::WorkItemIdentifier),
    ("plugin_id", OwnerLookup::Plugin),
    ("check_result_id", OwnerLookup::CheckResult),
    ("sprint_id", OwnerLookup::Sprint),
    ("comment_id", OwnerLookup::Comment),
    ("object_id", OwnerLookup::FlowObject),
    ("source_object_id", OwnerLookup::FlowObject),
    ("collection_id", OwnerLookup::FlowObject),
    ("preview_id", OwnerLookup::FlowConversion),
    ("job_id", OwnerLookup::FlowConversion),
    ("task_id", OwnerLookup::AiTask),
];

/// The policy scope of every registered tool.
///
/// `server::tests::tool_policy_scopes_match_the_registered_tool_schemas` re-derives
/// this table from the live tool registry (`tools::get_all_tool_definitions`), so a new
/// or renamed tool cannot silently land outside the policy gate — including tools whose
/// name shares no prefix with the family they belong to, such as `events.tail`.
const TOOL_POLICY_SCOPES: &[(&str, PolicyScope)] = &[
    ("projects.list", PolicyScope::WorkspaceWide),
    ("projects.get", PolicyScope::DeclaredProject { required: true }),
    ("projects.create", PolicyScope::WorkspaceWide),
    ("projects.update", PolicyScope::DeclaredProject { required: true }),
    ("projects.delete", PolicyScope::DeclaredProject { required: true }),
    ("project_types.list", PolicyScope::WorkspaceWide),
    ("project_types.get", PolicyScope::WorkspaceWide),
    // PARKED(project-resources): restore with the six registrations and the resource template; see RUNNER-WORK-PACKAGES-STAGES-3-6.md P3.2
    // (
    //     "project_resources.list",
    //     PolicyScope::DeclaredProject { required: true },
    // ),
    // (
    //     "project_resources.create",
    //     PolicyScope::DeclaredProject { required: true },
    // ),
    // (
    //     "project_resources.update",
    //     PolicyScope::DeclaredProject { required: true },
    // ),
    // (
    //     "project_resources.delete",
    //     PolicyScope::DeclaredProject { required: true },
    // ),
    ("forms.list", PolicyScope::DeclaredProject { required: true }),
    ("forms.get", PolicyScope::OwnedBy(OwnerLookup::FormData)),
    ("forms.create", PolicyScope::DeclaredProject { required: true }),
    ("forms.update_schema", PolicyScope::OwnedBy(OwnerLookup::FormData)),
    ("forms.duplicate", PolicyScope::OwnedBy(OwnerLookup::FormData)),
    ("forms.schema_summary", PolicyScope::OwnedBy(OwnerLookup::FormData)),
    ("forms.field_usage", PolicyScope::OwnedBy(OwnerLookup::FormData)),
    ("forms.field_dependencies", PolicyScope::OwnedBy(OwnerLookup::FormData)),
    ("form_schema_versions.list", PolicyScope::OwnedBy(OwnerLookup::FormData)),
    ("form_schema_versions.get", PolicyScope::OwnedBy(OwnerLookup::FormData)),
    ("form_permissions.get", PolicyScope::OwnedBy(OwnerLookup::FormData)),
    ("form_permissions.update", PolicyScope::OwnedBy(OwnerLookup::FormData)),
    ("form_views.list", PolicyScope::OwnedBy(OwnerLookup::FormData)),
    ("form_attachments.list", PolicyScope::OwnedBy(OwnerLookup::FormData)),
    ("form_attachments.create", PolicyScope::OwnedBy(OwnerLookup::FormData)),
    ("form_attachments.archive", PolicyScope::OwnedBy(OwnerLookup::FormData)),
    ("form_attachments.restore", PolicyScope::OwnedBy(OwnerLookup::FormData)),
    ("form_records.list", PolicyScope::OwnedBy(OwnerLookup::FormData)),
    ("form_records.export", PolicyScope::OwnedBy(OwnerLookup::FormData)),
    (
        "form_records.import_preview",
        PolicyScope::OwnedBy(OwnerLookup::FormData),
    ),
    (
        "form_records.import_commit",
        PolicyScope::OwnedBy(OwnerLookup::FormData),
    ),
    ("form_records.get", PolicyScope::OwnedBy(OwnerLookup::FormData)),
    ("form_records.create", PolicyScope::OwnedBy(OwnerLookup::FormData)),
    ("form_records.update", PolicyScope::OwnedBy(OwnerLookup::FormData)),
    ("form_records.link", PolicyScope::OwnedBy(OwnerLookup::FormData)),
    (
        "form_records.relation_targets",
        PolicyScope::OwnedBy(OwnerLookup::FormData),
    ),
    ("form_records.children", PolicyScope::OwnedBy(OwnerLookup::FormData)),
    ("form_records.child_create", PolicyScope::OwnedBy(OwnerLookup::FormData)),
    ("form_records.child_update", PolicyScope::OwnedBy(OwnerLookup::FormData)),
    (
        "form_records.child_archive",
        PolicyScope::OwnedBy(OwnerLookup::FormData),
    ),
    (
        "form_records.child_restore",
        PolicyScope::OwnedBy(OwnerLookup::FormData),
    ),
    ("form_records.aggregate", PolicyScope::OwnedBy(OwnerLookup::FormData)),
    ("events.tail", PolicyScope::OwnedBy(OwnerLookup::FormData)),
    ("forms.validate_schema", PolicyScope::DeclaredProject { required: true }),
    ("forms.validate_rules", PolicyScope::DeclaredProject { required: true }),
    (
        "forms.create_schema_draft",
        PolicyScope::DeclaredProject { required: true },
    ),
    ("layouts.validate", PolicyScope::DeclaredProject { required: true }),
    ("layouts.create_draft", PolicyScope::DeclaredProject { required: true }),
    ("plugins.list", PolicyScope::DeclaredProject { required: true }),
    ("plugins.get", PolicyScope::OwnedBy(OwnerLookup::Plugin)),
    ("plugins.install", PolicyScope::DeclaredProject { required: true }),
    ("plugins.invoke", PolicyScope::OwnedBy(OwnerLookup::Plugin)),
    ("plugin_invocations.list", PolicyScope::OwnedBy(OwnerLookup::Plugin)),
    ("context.get_project", PolicyScope::DeclaredProject { required: true }),
    (
        "context.get_governance",
        PolicyScope::DeclaredProject { required: true },
    ),
    (
        "context.get_agent_policy",
        PolicyScope::DeclaredProject { required: true },
    ),
    ("work_items.list", PolicyScope::DeclaredProject { required: true }),
    ("work_items.get", PolicyScope::OwnedBy(OwnerLookup::WorkItem)),
    (
        "work_items.get_by_identifier",
        PolicyScope::OwnedBy(OwnerLookup::WorkItemIdentifier),
    ),
    ("work_items.create", PolicyScope::DeclaredProject { required: true }),
    ("work_items.update", PolicyScope::OwnedBy(OwnerLookup::WorkItem)),
    ("work_items.add_label", PolicyScope::OwnedBy(OwnerLookup::WorkItem)),
    ("work_items.remove_label", PolicyScope::OwnedBy(OwnerLookup::WorkItem)),
    ("work_items.list_labels", PolicyScope::OwnedBy(OwnerLookup::WorkItem)),
    ("work_items.delete", PolicyScope::OwnedBy(OwnerLookup::WorkItem)),
    ("work_items.search", PolicyScope::WorkspaceWide),
    ("work_items.add_labels", PolicyScope::OwnedBy(OwnerLookup::WorkItem)),
    ("work_items.link_page", PolicyScope::OwnedBy(OwnerLookup::WorkItem)),
    ("work_items.unlink_page", PolicyScope::OwnedBy(OwnerLookup::WorkItem)),
    ("work_items.list_pages", PolicyScope::OwnedBy(OwnerLookup::WorkItem)),
    ("objects.list_work_items", PolicyScope::OwnedBy(OwnerLookup::FlowObject)),
    ("comments.list", PolicyScope::OwnedBy(OwnerLookup::WorkItem)),
    ("comments.create", PolicyScope::OwnedBy(OwnerLookup::WorkItem)),
    ("comments.delete", PolicyScope::OwnedBy(OwnerLookup::Comment)),
    ("files.upload", PolicyScope::WorkspaceWide),
    ("proposals.list", PolicyScope::DeclaredProject { required: true }),
    ("proposals.get", PolicyScope::WorkspaceWide),
    ("proposals.create", PolicyScope::DeclaredProject { required: true }),
    (
        "proposals.create_from_result",
        PolicyScope::OwnedBy(OwnerLookup::CheckResult),
    ),
    ("check_results.create", PolicyScope::DeclaredProject { required: true }),
    ("release.readiness.get", PolicyScope::DeclaredProject { required: true }),
    ("members.list", PolicyScope::WorkspaceWide),
    ("bot_operation_logs.list", PolicyScope::WorkspaceWide),
    ("sprints.create", PolicyScope::DeclaredProject { required: true }),
    ("sprints.list", PolicyScope::DeclaredProject { required: true }),
    ("sprints.update", PolicyScope::OwnedBy(OwnerLookup::Sprint)),
    ("sprints.delete", PolicyScope::OwnedBy(OwnerLookup::Sprint)),
    // The workspace is the token's own; who may change a milestone is decided by the API route
    // (workspace owner or admin), which also refuses a token limited to some projects.
    ("milestones.list", PolicyScope::WorkspaceWide),
    ("milestones.get", PolicyScope::WorkspaceWide),
    ("milestones.create", PolicyScope::WorkspaceWide),
    ("milestones.update", PolicyScope::WorkspaceWide),
    ("milestones.delete", PolicyScope::WorkspaceWide),
    ("labels.create", PolicyScope::WorkspaceWide),
    ("labels.list", PolicyScope::WorkspaceWide),
    (
        "labels.list_by_project",
        PolicyScope::DeclaredProject { required: true },
    ),
    ("labels.update", PolicyScope::WorkspaceWide),
    ("labels.delete", PolicyScope::WorkspaceWide),
    ("search.all", PolicyScope::WorkspaceWide),
    // PARKED(project-resources): restore with the six registrations and the resource template; see RUNNER-WORK-PACKAGES-STAGES-3-6.md P3.2
    // ("code.resources.list", PolicyScope::DeclaredProject { required: true }),
    // ("code.directory.get", PolicyScope::DeclaredProject { required: true }),
    ("code.task_context.get", PolicyScope::DeclaredProject { required: true }),
    (
        "code.change_proposal.create",
        PolicyScope::DeclaredProject { required: true },
    ),
    (
        "documents.extract_summary",
        PolicyScope::DeclaredProject { required: true },
    ),
    ("documents.review_risk", PolicyScope::DeclaredProject { required: true }),
    ("approval.request", PolicyScope::DeclaredProject { required: true }),
    ("inspection.report", PolicyScope::DeclaredProject { required: true }),
    (
        "corrective_action.propose",
        PolicyScope::DeclaredProject { required: true },
    ),
    // Flow v0.4 (`mcp-surface-v1.md`).
    ("flow.feature_get", PolicyScope::WorkspaceWide),
    ("flow.feature_set", PolicyScope::WorkspaceWideAdmin),
    ("objects.get", PolicyScope::OwnedBy(OwnerLookup::FlowObject)),
    ("objects.query", PolicyScope::DeclaredProject { required: false }),
    ("objects.history", PolicyScope::OwnedBy(OwnerLookup::FlowObject)),
    ("objects.create", PolicyScope::DeclaredProject { required: false }),
    ("objects.patch", PolicyScope::OwnedBy(OwnerLookup::FlowObject)),
    ("objects.move", PolicyScope::OwnedBy(OwnerLookup::FlowObject)),
    ("objects.link", PolicyScope::OwnedBy(OwnerLookup::FlowObject)),
    ("objects.unlink", PolicyScope::OwnedBy(OwnerLookup::FlowObject)),
    ("objects.diff", PolicyScope::OwnedBy(OwnerLookup::FlowObject)),
    ("objects.grants_get", PolicyScope::OwnedBy(OwnerLookup::FlowObject)),
    ("objects.grants_set", PolicyScope::OwnedBy(OwnerLookup::FlowObject)),
    ("objects.inheritance_set", PolicyScope::OwnedBy(OwnerLookup::FlowObject)),
    ("objects.relations", PolicyScope::OwnedBy(OwnerLookup::FlowObject)),
    ("objects.search", PolicyScope::DeclaredProject { required: false }),
    ("collections.describe", PolicyScope::OwnedBy(OwnerLookup::FlowObject)),
    ("collections.query", PolicyScope::OwnedBy(OwnerLookup::FlowObject)),
    ("records.create", PolicyScope::OwnedBy(OwnerLookup::FlowObject)),
    ("objects.reference", PolicyScope::OwnedBy(OwnerLookup::FlowObject)),
    ("objects.unreference", PolicyScope::OwnedBy(OwnerLookup::FlowObject)),
    ("objects.convert_preview", PolicyScope::OwnedBy(OwnerLookup::FlowObject)),
    (
        "objects.convert_commit",
        PolicyScope::OwnedBy(OwnerLookup::FlowConversion),
    ),
    (
        "objects.convert_status",
        PolicyScope::OwnedBy(OwnerLookup::FlowConversion),
    ),
    (
        "objects.convert_retry",
        PolicyScope::OwnedBy(OwnerLookup::FlowConversion),
    ),
    ("objects.export", PolicyScope::OwnedBy(OwnerLookup::FlowObject)),
    ("objects.export_workspace", PolicyScope::WorkspaceWideAdmin),
    ("objects.import_artifact", PolicyScope::WorkspaceWideAdmin),
    ("objects.import_preview", PolicyScope::WorkspaceWideAdmin),
    ("objects.import_commit", PolicyScope::WorkspaceWideAdmin),
    ("objects.import_status", PolicyScope::WorkspaceWideAdmin),
    ("collab.status", PolicyScope::WorkspaceWideAdmin),
    ("objects.integrity", PolicyScope::WorkspaceOrFlowObject),
    ("collab.compact", PolicyScope::OwnedBy(OwnerLookup::FlowObject)),
    ("deliveries.replay", PolicyScope::WorkspaceWideAdmin),
    (
        "collab.rebuild_projection",
        PolicyScope::OwnedBy(OwnerLookup::FlowObject),
    ),
    ("collab.repair_quarantine", PolicyScope::WorkspaceOrFlowObject),
    (
        "collab.projection_lag",
        PolicyScope::DeclaredProject { required: false },
    ),
    ("flow.comments.list", PolicyScope::OwnedBy(OwnerLookup::FlowObject)),
    ("flow.comments.create", PolicyScope::OwnedBy(OwnerLookup::FlowObject)),
    ("flow.comments.update", PolicyScope::OwnedBy(OwnerLookup::FlowObject)),
    ("flow.comments.delete", PolicyScope::OwnedBy(OwnerLookup::FlowObject)),
    ("flow.comments.resolve", PolicyScope::OwnedBy(OwnerLookup::FlowObject)),
    ("flow.shares.list", PolicyScope::OwnedBy(OwnerLookup::FlowObject)),
    ("flow.shares.create", PolicyScope::OwnedBy(OwnerLookup::FlowObject)),
    ("flow.shares.revoke", PolicyScope::OwnedBy(OwnerLookup::FlowObject)),
    ("flow.appearance.set", PolicyScope::OwnedBy(OwnerLookup::FlowObject)),
    ("flow.appearance.upload", PolicyScope::OwnedBy(OwnerLookup::FlowObject)),
    ("objects.restore", PolicyScope::OwnedBy(OwnerLookup::FlowObject)),
    ("flow.trash.list", PolicyScope::DeclaredProject { required: false }),
    ("objects.purge", PolicyScope::OwnedBy(OwnerLookup::FlowObject)),
    ("flow.mentions.list", PolicyScope::OwnedBy(OwnerLookup::FlowObject)),
    (tools::tasks::TASK_GET, PolicyScope::OwnedBy(OwnerLookup::AiTask)),
    (tools::tasks::TASK_ACK, PolicyScope::OwnedBy(OwnerLookup::AiTask)),
    (tools::tasks::TASK_PROGRESS, PolicyScope::OwnedBy(OwnerLookup::AiTask)),
    (tools::tasks::TASK_COMPLETE, PolicyScope::OwnedBy(OwnerLookup::AiTask)),
    (tools::tasks::TASK_FAIL, PolicyScope::OwnedBy(OwnerLookup::AiTask)),
];

/// Scope of one registered tool. Unknown names fail closed as mandatory
/// declared-project calls before dispatch rejects them.
fn tool_policy_scope(tool_name: &str) -> PolicyScope {
    TOOL_POLICY_SCOPES
        .iter()
        .find(|(name, _)| *name == tool_name)
        .map_or(PolicyScope::DeclaredProject { required: true }, |(_, scope)| *scope)
}

/// The "exact tool policy" leg of `WorkspaceWide(admin)` (`mcp-surface-v1.md:17`: a caller
/// must hold "Flow admin capability、exact tool policy 与 route workspace admin policy";
/// "不得从配置默认 workspace、tool 名或 caller payload 推导 admin scope").
///
/// Before this change, `PolicyScope::WorkspaceWideAdmin` was enforced nowhere at runtime:
/// `McpServer::resolve_policy_project_id` answers `Ok(None)` for it exactly like plain
/// `WorkspaceWide` (no project to check a policy against), so
/// `McpServer::enforce_project_tool_policy` always trivially succeeded for the five admin
/// tools and the entire admission decision fell through to whatever the target REST route
/// itself checked. This closes that: the call is only admitted here if (a) its own registered
/// [`PolicyScope`] — never a heuristic on the tool's name or a `project_id`/`workspace_id`
/// argument — is exactly [`PolicyScope::WorkspaceWideAdmin`], the "exact tool policy" itself,
/// and (b) the caller supplied an explicit, canonical `workspace_id` of their own, so the
/// admin scope is always the workspace the caller named, never this process's
/// `mcp.workspace_id` default. Every other tool is untouched: this only fires for tools this
/// file itself has classified as workspace-admin, and it fails closed on anything else.
///
/// A free function, not a method: it needs nothing from `McpServer` itself, only the tool's
/// own registered scope and its own arguments.
fn enforce_workspace_admin_tool_policy(tool_name: &str, args: &Value) -> Result<(), String> {
    if tool_policy_scope(tool_name) != PolicyScope::WorkspaceWideAdmin {
        return Ok(());
    }
    match uuid_argument(args, "workspace_id")? {
        Some(_) => Ok(()),
        None => Err(format!(
            "Tool '{tool_name}' requires an explicit, canonical workspace_id; admin scope is never derived from \
             this process's configured default workspace"
        )),
    }
}

/// The object a form scoped call operates on. The governing project is always
/// resolved from this object through the API, never from the call arguments.
#[derive(Debug, Clone, PartialEq, Eq)]
enum FormScopeTarget {
    Form(String),
    Record(String),
    Attachment(String),
}

impl FormScopeTarget {
    fn cache_key(&self) -> String {
        match self {
            Self::Form(id) => format!("form:{id}"),
            Self::Record(id) => format!("record:{id}"),
            Self::Attachment(id) => format!("attachment:{id}"),
        }
    }

    fn describe(&self) -> String {
        match self {
            Self::Form(id) => format!("form {id}"),
            Self::Record(id) => format!("record {id}"),
            Self::Attachment(id) => format!("attachment {id}"),
        }
    }
}

/// Picks the identifier that decides which project owns the data a form scoped call
/// touches, plus the `form_id` the caller claims it belongs to.
///
/// `record_id` wins over `form_id`: the record is the narrower object and its API
/// payload reports the real `form_id`, so a call that names both can be checked instead
/// of trusting the wider handle. Resolving through the *wider* id was the fragile part
/// of the previous version — a future tool taking both would have been governed by the
/// one the caller can pick freely.
/// Refuses a call that carries an ownership bearing argument the tool's schema does not declare.
///
/// The owner of a call is resolved from its arguments, and some resolutions pick the first of
/// several ids they find (`record_id`, then `form_id`, then `attachment_id`). An undeclared id a
/// caller adds could therefore redirect which project's agent policy is evaluated: a tool
/// addressed by `attachment_id` called with a `form_id` of a project the caller may read would be
/// governed by that project's policy and then act on the attachment. A tool only ever takes the
/// ids it declares. Tools of the Flow-only surface are not in the registry and are checked by
/// their own preparation.
fn reject_undeclared_ownership_arguments(tool_name: &str, args: &Value) -> Result<(), String> {
    let Some(definition) = tools::entry::registry_definition(tool_name) else {
        return Ok(());
    };
    let declared = definition.input_schema.get("properties").and_then(Value::as_object);
    for (argument, _) in OWNERSHIP_ARGUMENTS {
        let present = args.get(argument).is_some_and(|value| !value.is_null());
        if present && !declared.is_some_and(|properties| properties.contains_key(argument)) {
            tracing::warn!(
                tool_name = %tool_name,
                argument = %argument,
                "Refusing call: an ownership bearing argument the tool does not declare"
            );
            return Err(format!(
                "Tool '{tool_name}' does not take '{argument}'; refusing because an undeclared id could \
                 change which project policy applies"
            ));
        }
    }
    Ok(())
}

fn form_scope_target(args: &Value) -> Result<Option<(FormScopeTarget, Option<String>)>, String> {
    let declared_form_id = uuid_argument(args, "form_id")?;
    if let Some(record_id) = uuid_argument(args, "record_id")? {
        return Ok(Some((FormScopeTarget::Record(record_id), declared_form_id)));
    }
    if let Some(form_id) = declared_form_id {
        return Ok(Some((FormScopeTarget::Form(form_id), None)));
    }
    Ok(uuid_argument(args, "attachment_id")?.map(|id| (FormScopeTarget::Attachment(id), None)))
}

/// A resource's owning project, plus the owning form when the API reports one.
#[derive(Debug, Clone, PartialEq, Eq)]
struct ResolvedOwner {
    project_id: String,
    form_id: Option<String>,
}

/// Upper bound for the resource to project mapping cache.
const PROJECT_ID_CACHE_LIMIT: usize = 1_024;

/// The header a connection can name its tool mode with, next to the `?tools=` query parameter.
pub const TOOL_MODE_HEADER: &str = "x-sylvode-tool-mode";

/// The tool mode one connection asked for, or the server's own when it asked for none.
///
/// A connection asks with `?tools=` and `X-Sylvode-Tool-Mode`, each of which may be repeated.
/// `raw_query` is the request's raw query string. A value other than `search` or `full`, and any
/// two values that disagree (two parameters, two header lines, or a parameter and a header), are
/// refused rather than guessed at; an identical repeat is harmless.
pub fn requested_tool_mode(
    raw_query: Option<&str>,
    headers: &axum::http::HeaderMap,
    default: McpToolMode,
) -> Result<McpToolMode, String> {
    let parse =
        |source: &str, raw: &str| McpToolMode::parse(raw).ok_or_else(|| format!("{source} must be search or full"));
    let mut asked = Vec::new();
    // `Url` decodes the pairs; the host is a placeholder.
    let pairs = raw_query
        .and_then(|query| reqwest::Url::parse(&format!("http://mode.invalid/?{query}")).ok())
        .map(|url| {
            url.query_pairs()
                .filter(|(key, _)| key == "tools")
                .map(|(_, value)| value.into_owned())
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    for value in &pairs {
        asked.push(parse("the tools query parameter", value)?);
    }
    for line in headers.get_all(TOOL_MODE_HEADER) {
        let text = line
            .to_str()
            .map_err(|_| "X-Sylvode-Tool-Mode is not text".to_string())?;
        asked.push(parse("X-Sylvode-Tool-Mode", text)?);
    }
    let mut modes = asked.iter().copied();
    match modes.next() {
        None => Ok(default),
        Some(first) if modes.all(|mode| mode == first) => Ok(first),
        Some(_) => Err("the requested tool modes disagree".to_string()),
    }
}

/// What a transport decided about the tool listing of the servers it builds: the mode and whether
/// `sylvode_write` may be used.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ToolSurface {
    pub mode: McpToolMode,
    pub allow_write_call: bool,
    /// `[mcp] listen`: the server declares `tools.listChanged` and serves `subscriptions/listen`.
    pub listen: bool,
}

impl ToolSurface {
    /// The surface of `[mcp]`: its `tool_mode` and `allow_write_call`.
    pub const fn new(mode: McpToolMode, allow_write_call: bool) -> Self {
        Self {
            mode,
            allow_write_call,
            listen: false,
        }
    }

    /// The same switches with `[mcp] listen` set.
    #[must_use]
    pub const fn with_listen(self, listen: bool) -> Self {
        Self { listen, ..self }
    }

    /// The same switches with another mode (a connection that asked for one).
    #[must_use]
    pub const fn with_mode(self, mode: McpToolMode) -> Self {
        Self { mode, ..self }
    }
}

pub struct McpServer {
    client: OpenPrClient,
    /// Maps `form:<id>` / `record:<id>` / `work_item:<id>` / ... to the owning project.
    /// Only ownership is cached, never a policy: a record cannot move between projects,
    /// but an agent policy can be edited at any time and is therefore re-read on every
    /// call. A cache hit costs zero extra requests, a miss costs exactly one lookup
    /// (two only for an attachment whose payload reports just its form).
    ///
    /// The cache is per-`McpServer`, and an `McpServer` is built per request on the
    /// networked transports — it never spans two callers. That matters beyond tidiness:
    /// ownership is resolved by *calling the API as the caller*, so an entry another
    /// caller's credential paid for would let this one skip that call and learn that a form
    /// id exists, and which project owns it, without ever proving it may look. Sharing the
    /// map across callers would turn a performance cache into an existence oracle, so the
    /// two lifetimes must not be decoupled. On stdio there is one caller for the life of the
    /// process, so the same map is simply that caller's own.
    project_id_cache: Arc<Mutex<HashMap<String, ResolvedOwner>>>,
    /// What a delegated (OAuth) caller was granted; `None` for every other caller.
    ///
    /// When present it narrows the server on every door: `tools/list` shows only the tools the
    /// scopes allow, `tools/call` refuses anything else before the policy chain runs, and
    /// `resources/*` serves only the static guides. It is fixed when the server is built for one
    /// request and cannot be changed by anything the caller sends.
    delegated: Option<DelegatedGrant>,
    /// Which tools this server lists and accepts by name. Meaningful for a plain (non-delegated,
    /// non-Flow-only) server only: a delegated server always serves the scoped registry and a
    /// Flow-only server its own definitions.
    tool_mode: McpToolMode,
    /// Whether `sylvode_write` may be used. `false` makes it answer that writes are disabled.
    write_call_allowed: bool,
    /// `[mcp] listen`: `tools.listChanged` is declared and `tools/list` may be cached for the
    /// longer time a push makes safe.
    listen: bool,
    /// The id of the bot this server's credential belongs to, asked of the API once and kept: a
    /// server serves one credential for its whole life.
    bot_id: tokio::sync::OnceCell<Uuid>,
}

impl McpServer {
    /// A server for one plain caller. The mode is a required argument so that no construction
    /// site can end up with a listing nobody chose.
    pub fn new(client: OpenPrClient, tool_mode: McpToolMode) -> Self {
        Self {
            client,
            project_id_cache: Arc::new(Mutex::new(HashMap::new())),
            delegated: None,
            tool_mode,
            write_call_allowed: true,
            listen: false,
            bot_id: tokio::sync::OnceCell::new(),
        }
    }

    /// A server for one plain caller on a transport that decided its [`ToolSurface`].
    pub fn for_surface(client: OpenPrClient, surface: ToolSurface) -> Self {
        Self::new(client, surface.mode)
            .with_write_call_allowed(surface.allow_write_call)
            .with_listen(surface.listen)
    }

    /// Sets whether the server declares push (`[mcp] listen`).
    #[must_use]
    pub const fn with_listen(mut self, listen: bool) -> Self {
        self.listen = listen;
        self
    }

    /// Sets whether `sylvode_write` may be used (`[mcp] allow_write_call`).
    #[must_use]
    pub const fn with_write_call_allowed(mut self, allowed: bool) -> Self {
        self.write_call_allowed = allowed;
        self
    }

    /// A server for one delegated caller. `client` must already act for that caller
    /// ([`OpenPrClient::acting_as_delegated`]); `grant` is what its token context allows.
    pub fn new_delegated(client: OpenPrClient, grant: DelegatedGrant) -> Self {
        Self {
            client,
            project_id_cache: Arc::new(Mutex::new(HashMap::new())),
            delegated: Some(grant),
            // Ignored: a delegated server lists the scoped registry whatever a transport asked.
            tool_mode: McpToolMode::Full,
            write_call_allowed: true,
            listen: false,
            bot_id: tokio::sync::OnceCell::new(),
        }
    }

    /// Why a `tools/call` with these `params` must be refused for a delegated caller, or `None`
    /// when it may proceed (or when the caller is not delegated).
    ///
    /// The transport asks this before dispatch so that it can answer with the HTTP status and
    /// challenge the refusal calls for. `call_tool` asks the same question again, so no path into
    /// a tool skips it.
    pub fn delegated_refusal(&self, params: Option<&Value>) -> Option<ToolRefusal> {
        let grant = self.delegated.as_ref()?;
        let name = params.and_then(|params| params.get("name")).and_then(Value::as_str)?;
        let arguments = params.and_then(|params| params.get("arguments"));
        gate_tool_call(grant, name, arguments).err()
    }

    /// Handles one request, placing it in its era first (`era::classify`). Used by the stdio
    /// transport and by tests; the HTTP transport classifies itself and calls
    /// [`Self::handle_request_in`].
    pub async fn handle_request(&self, req: JsonRpcRequest) -> Option<JsonRpcResponse> {
        match era::classify(&req) {
            Ok(era) => self.handle_request_in(&era, req).await,
            Err(error) => {
                tracing::info!(method = %req.method, era = "unclassified", "Refusing an MCP request: {error:?}");
                req.id
                    .map(|id| JsonRpcResponse::error(Some(id), error.into_rpc_error()))
            }
        }
    }

    /// Handles one request of the given era. A notification never gets an answer.
    pub async fn handle_request_in(&self, era: &Era, req: JsonRpcRequest) -> Option<JsonRpcResponse> {
        tracing::info!(
            method = %req.method,
            era = era.label(),
            client = %client_label(era, &req),
            id_kind = id_kind(req.id.as_ref()),
            "Handling MCP request"
        );

        if req.jsonrpc != "2.0" {
            return Some(JsonRpcResponse::error(
                req.id,
                JsonRpcError::invalid_request("Invalid jsonrpc version"),
            ));
        }

        // Notifications (`notifications/initialized`, `notifications/cancelled`, ...) carry no
        // answer; none of them has an effect on a per-request server.
        req.id.as_ref()?;

        let static_guide_read = req.method == "resources/read"
            && req
                .params
                .as_ref()
                .and_then(|params| params.get("uri"))
                .and_then(Value::as_str)
                .is_some_and(is_static_guide_resource);
        let method = req.method.clone();
        let response = self.dispatch(era, req).await;
        Some(if era.is_modern() {
            // A plain caller of a server that pushes may cache the tool list for longer: it is told
            // when the list changes. Everyone else polls at the short interval.
            let tools_ttl_ms = if self.listen && self.applicable_mode().is_some() {
                era::TTL_TOOLS_LISTEN_MS
            } else {
                era::TTL_TOOLS_NO_LISTEN_MS
            };
            decorate_modern_response(&method, static_guide_read, tools_ttl_ms, response)
        } else {
            response
        })
    }

    async fn dispatch(&self, era: &Era, req: JsonRpcRequest) -> JsonRpcResponse {
        let modern = era.is_modern();
        if self.client.is_flow_only() && req.method.starts_with("resources/") {
            return match req.method.as_str() {
                "resources/list" => JsonRpcResponse::success(req.id, json!({"resources":[]})),
                "resources/templates/list" => JsonRpcResponse::success(req.id, json!({"resourceTemplates":[]})),
                _ => JsonRpcResponse::error(
                    req.id,
                    JsonRpcError::method_not_found(
                        "Flow-only mode exposes registered Flow tools, not workspace resource dispatch",
                    ),
                ),
            };
        }

        match req.method.as_str() {
            "server/discover" => self.handle_discover(req.id),
            // The modern era has no handshake and no ping.
            "initialize" | "ping" if modern => JsonRpcResponse::error(
                req.id,
                JsonRpcError::method_not_found(format!("Unknown method: {}", req.method)),
            ),
            "initialize" => self.handle_initialize(req.id, req.params.as_ref()),
            "ping" => JsonRpcResponse::success(req.id, json!({})),
            "tools/list" => self.handle_list_tools(req.id, req.params).await,
            "tools/call" => self.handle_call_tool(req.id, req.params).await,
            "resources/list" => self.handle_resources_list(req.id),
            "resources/templates/list" => self.handle_resources_templates_list(req.id),
            "resources/read" => self.handle_resources_read(req.id, req.params, era).await,
            _ => JsonRpcResponse::error(
                req.id,
                JsonRpcError::method_not_found(format!("Unknown method: {}", req.method)),
            ),
        }
    }

    async fn handle_list_tools(&self, id: Option<Value>, params: Option<Value>) -> JsonRpcResponse {
        let search_listing = self.applicable_mode() == Some(McpToolMode::Search);
        let mut static_tools = tools::catalog::annotate(if self.client.is_flow_only() {
            tools::flow_scope::definitions()
        } else if search_listing {
            tools::entry::search_mode_listing()
        } else {
            tools::get_all_tool_definitions()
        });
        if let Some(grant) = &self.delegated {
            static_tools.retain(|tool| crate::oauth_scopes::allowed(grant.scopes, &tool.name));
        }
        let requested_project_id = match extract_tools_list_project_id(params.as_ref()) {
            Ok(project_id) => project_id,
            Err(error) => return JsonRpcResponse::error(id, JsonRpcError::invalid_params(error)),
        };
        let tools = match requested_project_id {
            Some(project_id) => match self.client.get_project_agent_policy(&project_id).await {
                Ok(policy) => match tools::capabilities::extract_enabled_tool_names(&policy) {
                    Some(enabled_tool_names) => {
                        // The policy names registry tools, which the search listing does not contain.
                        let known = if search_listing {
                            tools::get_all_tool_definitions()
                        } else {
                            static_tools.clone()
                        };
                        let missing = tools::capabilities::unknown_tool_names(&known, &enabled_tool_names);
                        if !missing.is_empty() {
                            tracing::warn!(
                                project_id = %project_id,
                                missing_tools = ?missing,
                                "Project agent policy references tools not registered by this MCP server"
                            );
                        }
                        if search_listing {
                            // Entry tools are not policy subjects; the task tools are always enabled.
                            static_tools.retain(|tool| {
                                tools::entry::is_entry_tool(&tool.name) || enabled_tool_names.contains(&tool.name)
                            });
                            static_tools
                        } else {
                            tools::capabilities::filter_tool_definitions(static_tools, &enabled_tool_names)
                        }
                    }
                    None => static_tools,
                },
                Err(error) => {
                    return JsonRpcResponse::error(
                        id,
                        JsonRpcError::internal_error(format!(
                            "Failed to load project agent policy for tools/list: {error}"
                        )),
                    );
                }
            },
            None => static_tools,
        };
        let result = ListToolsResult { tools };

        match serde_json::to_value(&result) {
            Ok(value) => JsonRpcResponse::success(id, value),
            Err(e) => JsonRpcResponse::error(
                id,
                JsonRpcError::internal_error(format!("Failed to serialize tools: {e}")),
            ),
        }
    }

    async fn handle_call_tool(&self, id: Option<Value>, params: Option<Value>) -> JsonRpcResponse {
        let Some(params) = params else {
            return JsonRpcResponse::error(id, JsonRpcError::invalid_params("Missing params"));
        };

        let call_params: CallToolParams = match serde_json::from_value(params) {
            Ok(p) => p,
            Err(e) => {
                return JsonRpcResponse::error(id, JsonRpcError::invalid_params(format!("Invalid params: {e}")));
            }
        };

        let args = call_params
            .arguments
            .unwrap_or_else(|| Value::Object(serde_json::Map::default()));

        let result = self.call_tool(&call_params.name, args).await;

        match serde_json::to_value(&result) {
            Ok(value) => JsonRpcResponse::success(id, value),
            Err(e) => JsonRpcResponse::error(
                id,
                JsonRpcError::internal_error(format!("Failed to serialize result: {e}")),
            ),
        }
    }

    /// The mode that decides which names this server accepts, or `None` when mode does not apply
    /// (a delegated server, which has its scope table, and a Flow-only server).
    fn applicable_mode(&self) -> Option<McpToolMode> {
        (self.delegated.is_none() && !self.client.is_flow_only()).then_some(self.tool_mode)
    }

    /// Runs one `tools/call`: the delegated gate, then the mode (a name `tools/list` does not offer
    /// is refused), then the entry tools or, for a registry tool, the policy chain.
    pub async fn call_tool(&self, name: &str, args: Value) -> CallToolResult {
        if let Some(grant) = &self.delegated
            && let Err(refusal) = gate_tool_call(grant, name, Some(&args))
        {
            return CallToolResult::error(refusal.message());
        }
        let mode = self.applicable_mode();
        if let Some(mode) = mode
            && let Some(refusal) = tools::entry::unlisted_refusal(mode, name)
        {
            return CallToolResult::error(refusal);
        }
        if mode == Some(McpToolMode::Search) && tools::entry::is_entry_tool(name) {
            return self.call_entry_tool(name, args).await;
        }
        self.call_registry_tool(name, args).await
    }

    /// The four search mode entry tools. They are handled before the scoped client and the policy
    /// chain exist: `sylvode_overview` and `sylvode_search` read the catalogue (and, for a named
    /// project, its agent policy), and the wrappers hand their target to
    /// [`Self::call_registry_tool`], which applies every check of a direct call under the
    /// target's own name, so the operation log shows `work_items.update`, not `sylvode_write`.
    async fn call_entry_tool(&self, name: &str, args: Value) -> CallToolResult {
        match name {
            tools::entry::OVERVIEW | tools::entry::SEARCH => self.call_catalogue_tool(name, &args).await,
            _ => {
                if name == tools::entry::WRITE && !self.write_call_allowed {
                    return CallToolResult::error(tools::entry::writes_disabled_message());
                }
                match tools::entry::resolve_wrapper_call(name, &args) {
                    Ok((tool, arguments)) => self.call_registry_tool(&tool, arguments).await,
                    Err(error) => CallToolResult::error(error),
                }
            }
        }
    }

    async fn call_catalogue_tool(&self, name: &str, args: &Value) -> CallToolResult {
        let project = match tools::entry::project_argument(args) {
            Ok(project) => project,
            Err(error) => return CallToolResult::error(error),
        };
        let enabled = match &project {
            Some(project_id) => match self.enabled_tools_of(name, project_id).await {
                Ok(enabled) => enabled,
                Err(error) => return CallToolResult::error(error),
            },
            None => None,
        };
        let answer = if name == tools::entry::OVERVIEW {
            Ok(tools::entry::overview(project.as_deref(), enabled.as_ref()))
        } else {
            tools::entry::search(args, enabled.as_ref())
        };
        match answer {
            Ok(value) => CallToolResult::success(value.to_string()),
            Err(error) => CallToolResult::error(error),
        }
    }

    /// The tools a project's agent policy offers, or `None` when the policy names none and so
    /// restricts nothing. One API call, as `tools/list` with a project makes.
    async fn enabled_tools_of(&self, operation: &str, project_id: &str) -> Result<Option<BTreeSet<String>>, String> {
        let policy = self
            .client
            .recording_operation(operation)
            .get_project_agent_policy(project_id)
            .await
            .map_err(|error| format!("Failed to load project agent policy for {operation}: {error}"))?;
        if !policy.get("data").is_some_and(Value::is_object) {
            return Err(format!(
                "Refusing {operation}: the agent-policy response for project {project_id} carries no policy object"
            ));
        }
        Ok(tools::capabilities::extract_enabled_tool_names(&policy))
    }

    /// Whether `[mcp] allow_write_call = false` refuses `name`: a tool the catalogue marks as
    /// writing. The five task tools are the one exception, since a runner bot needs the task
    /// lifecycle to work. A Flow-only server also lists tools the catalogue does not know
    /// (`objects.command`, ...); those cannot be classified, so they count as writes.
    fn is_write_class(&self, name: &str) -> bool {
        if tools::tasks::is_task_tool(name) {
            return false;
        }
        tools::catalog::meta(name).map_or_else(
            || self.client.is_flow_only(),
            |entry| entry.access == tools::catalog::Access::Write,
        )
    }

    /// A registry tool through the delegated gate, the scoped client and the policy chain: the
    /// one door every direct call and every wrapper call goes through.
    async fn call_registry_tool(&self, name: &str, args: Value) -> CallToolResult {
        if let Some(grant) = &self.delegated
            && let Err(refusal) = gate_tool_call(grant, name, Some(&args))
        {
            return CallToolResult::error(refusal.message());
        }
        if !self.write_call_allowed && self.is_write_class(name) {
            return CallToolResult::error(tools::entry::writes_disabled_message());
        }
        // A tool whose schema is closed takes exactly the arguments it declares, by every door:
        // the wrappers always validated them, and a direct call must not forward more to the API.
        if !self.client.is_flow_only()
            && let Some(definition) = tools::entry::registry_definition(name)
            && definition.input_schema.get("additionalProperties") == Some(&json!(false))
            && let Err(error) = tools::schema_check::validate(&definition.input_schema, &args)
        {
            return CallToolResult::error(format!("Invalid arguments for '{name}': {error}"));
        }
        let scoped = Self {
            client: self.client.recording_operation(name),
            project_id_cache: if self.client.is_flow_only() {
                Arc::new(Mutex::new(HashMap::new()))
            } else {
                Arc::clone(&self.project_id_cache)
            },
            delegated: self.delegated.clone(),
            tool_mode: self.tool_mode,
            write_call_allowed: self.write_call_allowed,
            listen: self.listen,
            bot_id: self.bot_id.clone(),
        };
        if scoped.client.is_flow_only() {
            let request = match tools::flow_scope::prepare(&scoped.client, name, &args).await {
                Ok(value) => value,
                Err(error) => return error,
            };
            let mut projects = request
                .policy_projects
                .iter()
                .cloned()
                .collect::<std::collections::BTreeSet<_>>();
            for target in &request.form_policy_args {
                match scoped.resolve_form_data_owner(name, target).await {
                    Ok(project) => {
                        projects.insert(project);
                    }
                    Err(error) => return CallToolResult::error(error),
                }
            }
            for project in &projects {
                for policy_tool in &request.policy_tools {
                    if let Err(error) = scoped.enforce_resolved_project_policy(policy_tool, Some(project)).await {
                        return CallToolResult::error(error);
                    }
                }
            }
            return tools::flow_scope::execute(&scoped.client, request).await;
        }
        if let Err(error) = enforce_workspace_admin_tool_policy(name, &args) {
            return CallToolResult::error(error);
        }
        if let Err(error) = scoped.enforce_flow_admin_capability(name, &args).await {
            return CallToolResult::error(error);
        }
        match scoped.enforce_project_tool_policy(name, &args).await {
            Ok(()) => scoped.execute_tool(name, args).await,
            Err(error) => CallToolResult::error(error),
        }
    }

    /// The "Flow admin capability" leg of `WorkspaceWide(admin)` (`mcp-surface-v1.md:17`).
    ///
    /// `apps/api` carries no capability distinct from generic workspace admin for Flow
    /// specifically — there is no `flow_admin` column or bit anywhere in `migrations/*.sql`
    /// today, only `workspace_bots.permissions`'s `read`/`write`/`admin`
    /// (`migrations/0022_bot_tokens.sql:10`) — so this leg and the "route workspace admin
    /// policy" leg the target endpoint itself enforces
    /// (`apps/api/src/flow/policy.rs`'s `require_flow_workspace_admin_access`) are, in
    /// today's data model, backed by the exact same permission bit. Rather than fabricate a
    /// second check that would silently pass on the strength of the first one's own success,
    /// this performs one genuine, separate round trip — with the caller's own forwarded
    /// credential, via `OpenPrClient::list_workspace_bots` — against real, already-admin-gated
    /// REST surface before the write is ever attempted. A caller that cannot pass this
    /// genuinely does not hold admin capability in this workspace; the write's own check
    /// downstream remains the authority of record for the "route workspace admin policy" leg.
    ///
    /// Applied only to `flow.feature_set`.
    async fn enforce_flow_admin_capability(&self, tool_name: &str, args: &Value) -> Result<(), String> {
        if tool_name != "flow.feature_set" {
            return Ok(());
        }
        // `enforce_workspace_admin_tool_policy` already refuses a missing/malformed
        // `workspace_id` for every `WorkspaceWideAdmin` tool; this re-derives it rather than
        // assume call order, so this function stays correct even if it is ever called on its
        // own.
        let Some(workspace_id) = uuid_argument(args, "workspace_id")? else {
            return Err(format!(
                "Tool '{tool_name}' requires an explicit, canonical workspace_id"
            ));
        };
        self.client
            .list_workspace_bots(&workspace_id)
            .await
            .map(|_| ())
            .map_err(|error| {
                format!(
                    "Refusing '{tool_name}': the calling bot/user does not hold Flow admin capability for workspace \
                 {workspace_id} ({error})"
                )
            })
    }

    /// Dispatches a tool *after* it has been authorized. Private on purpose: `call_tool`
    /// is the only entry point, so no transport (stdio, HTTP, CLI) can reach a tool
    /// without passing the project policy gate and the audit trail.
    // The exhaustive static dispatcher intentionally keeps every registered name in one match so
    // registry/scope tests can prove two-way coverage. Each arm is awaited immediately.
    #[allow(clippy::large_stack_frames)]
    async fn execute_tool(&self, name: &str, args: Value) -> CallToolResult {
        match name {
            // Projects
            "projects.list" => tools::projects::list_projects(&self.client, args).await,
            "projects.get" => tools::projects::get_project(&self.client, args).await,
            "projects.create" => tools::projects::create_project(&self.client, args).await,
            "projects.update" => tools::projects::update_project(&self.client, args).await,
            "projects.delete" => tools::projects::handle_delete_project(&self.client, args).await,
            "project_types.list" => tools::project_types::list_project_types(&self.client, args).await,
            "project_types.get" => tools::project_types::get_project_type(&self.client, args).await,
            // PARKED(project-resources): restore with the six registrations and the resource template; see RUNNER-WORK-PACKAGES-STAGES-3-6.md P3.2
            // "project_resources.list" => tools::project_types::list_project_resources(&self.client, args).await,
            // "project_resources.create" => tools::project_types::create_project_resource(&self.client, args).await,
            // "project_resources.update" => tools::project_types::update_project_resource(&self.client, args).await,
            // "project_resources.delete" => tools::project_types::delete_project_resource(&self.client, args).await,
            "forms.list" => tools::forms::list_forms(&self.client, args).await,
            "forms.get" => tools::forms::get_form(&self.client, args).await,
            "forms.create" => tools::forms::create_form(&self.client, args).await,
            "forms.update_schema" => tools::forms::update_form_schema(&self.client, args).await,
            "forms.duplicate" => tools::forms::duplicate_form(&self.client, args).await,
            "forms.schema_summary" => tools::forms::get_form_schema_summary(&self.client, args).await,
            "forms.field_usage" => tools::forms::get_form_field_usage(&self.client, args).await,
            "forms.field_dependencies" => tools::forms::get_form_field_dependencies(&self.client, args).await,
            "form_schema_versions.list" => tools::forms::list_form_schema_versions(&self.client, args).await,
            "form_schema_versions.get" => tools::forms::get_form_schema_version(&self.client, args).await,
            "form_permissions.get" => tools::forms::get_form_permissions(&self.client, args).await,
            "form_permissions.update" => tools::forms::update_form_permissions(&self.client, args).await,
            "form_views.list" => tools::forms::list_form_views(&self.client, args).await,
            "form_attachments.list" => tools::forms::list_form_attachments(&self.client, args).await,
            "form_attachments.create" => tools::forms::create_form_attachment(&self.client, args).await,
            "form_attachments.archive" => tools::forms::archive_form_attachment(&self.client, args).await,
            "form_attachments.restore" => tools::forms::restore_form_attachment(&self.client, args).await,
            "form_records.list" => tools::forms::list_form_records(&self.client, args).await,
            "form_records.export" => tools::forms::export_form_records(&self.client, args).await,
            "form_records.import_preview" => tools::forms::preview_import_form_records(&self.client, args).await,
            "form_records.import_commit" => tools::forms::import_form_records(&self.client, args).await,
            "form_records.get" => tools::forms::get_form_record(&self.client, args).await,
            "form_records.create" => tools::forms::create_form_record(&self.client, args).await,
            "form_records.update" => tools::forms::update_form_record(&self.client, args).await,
            "form_records.link" => tools::forms::link_form_record(&self.client, args).await,
            "form_records.relation_targets" => tools::forms::list_form_relation_targets(&self.client, args).await,
            "form_records.children" => tools::forms::list_form_record_children(&self.client, args).await,
            "form_records.child_create" => tools::forms::create_child_form_record(&self.client, args).await,
            "form_records.child_update" => tools::forms::update_child_form_record(&self.client, args).await,
            "form_records.child_archive" => tools::forms::archive_child_form_record(&self.client, args).await,
            "form_records.child_restore" => tools::forms::restore_child_form_record(&self.client, args).await,
            "form_records.aggregate" => tools::forms::aggregate_form_records(&self.client, args).await,
            "events.tail" => tools::forms::events_tail(&self.client, args).await,
            "forms.validate_schema" => tools::forms::validate_form_schema(&self.client, args).await,
            "forms.validate_rules" => tools::forms::validate_field_rules(&self.client, args).await,
            "forms.create_schema_draft" => tools::forms::create_schema_draft(&self.client, args).await,
            "layouts.validate" => tools::layouts::validate_layout(&self.client, args).await,
            "layouts.create_draft" => tools::layouts::create_layout_draft(&self.client, args).await,
            "plugins.list" => tools::plugins::list_plugins(&self.client, args).await,
            "plugins.get" => tools::plugins::get_plugin(&self.client, args).await,
            "plugins.install" => tools::plugins::install_plugin(&self.client, args).await,
            "plugins.invoke" => tools::plugins::invoke_plugin(&self.client, args).await,
            "plugin_invocations.list" => tools::plugins::list_plugin_invocations(&self.client, args).await,
            "context.get_project" => tools::context::get_project_context(&self.client, args).await,
            "context.get_governance" => tools::context::get_governance_context(&self.client, args).await,
            "context.get_agent_policy" => tools::context::get_agent_policy(&self.client, args).await,

            // Work Items
            "work_items.list" => tools::work_items::list_work_items(&self.client, args).await,
            "work_items.get" => tools::work_items::get_work_item(&self.client, args).await,
            "work_items.get_by_identifier" => tools::work_items::get_work_item_by_identifier(&self.client, args).await,
            "work_items.create" => tools::work_items::create_work_item(&self.client, args).await,
            "work_items.update" => tools::work_items::update_work_item(&self.client, args).await,
            "work_items.add_label" => tools::work_items::add_label_to_work_item(&self.client, args).await,
            "work_items.remove_label" => tools::work_items::remove_label_from_work_item(&self.client, args).await,
            "work_items.list_labels" => tools::work_items::list_work_item_labels(&self.client, args).await,
            "work_items.delete" => tools::work_items::handle_delete_work_item(&self.client, args).await,
            "work_items.search" => tools::work_items::search_work_items(&self.client, args).await,
            "work_items.add_labels" => tools::work_items::add_labels_to_work_item(&self.client, args).await,
            "work_items.link_page" => tools::page_links::link_page(&self.client, args).await,
            "work_items.unlink_page" => tools::page_links::unlink_page(&self.client, args).await,
            "work_items.list_pages" => tools::page_links::list_pages(&self.client, args).await,
            "objects.list_work_items" => tools::page_links::list_work_items_of_page(&self.client, args).await,

            // Comments
            "comments.list" => tools::comments::list_comments(&self.client, args).await,
            "comments.create" => tools::comments::create_comment(&self.client, args).await,
            "comments.delete" => tools::comments::handle_delete_comment(&self.client, args).await,
            "files.upload" => tools::files::upload_file(&self.client, args).await,

            // Proposals
            "proposals.list" => tools::proposals::list_proposals(&self.client, args).await,
            "proposals.get" => tools::proposals::get_proposal(&self.client, args).await,
            "proposals.create" => tools::proposals::create_proposal(&self.client, args).await,
            "proposals.create_from_result" => tools::proposals::create_proposal_from_result(&self.client, args).await,
            "check_results.create" => tools::proposals::create_check_result(&self.client, args).await,
            "release.readiness.get" => tools::release::get_release_readiness(&self.client, args).await,

            // Members
            "members.list" => tools::members::list_members(&self.client, args).await,
            "bot_operation_logs.list" => tools::operation_logs::list_bot_operation_logs(&self.client, args).await,

            // Sprints
            "sprints.create" => tools::sprints::create_sprint(&self.client, args).await,
            "sprints.list" => tools::sprints::list_sprints(&self.client, args).await,
            "sprints.update" => tools::sprints::update_sprint(&self.client, args).await,
            "sprints.delete" => tools::sprints::handle_delete_sprint(&self.client, args).await,

            // Milestones
            "milestones.list" => tools::milestones::list_milestones(&self.client, args).await,
            "milestones.get" => tools::milestones::get_milestone(&self.client, args).await,
            "milestones.create" => tools::milestones::create_milestone(&self.client, args).await,
            "milestones.update" => tools::milestones::update_milestone(&self.client, args).await,
            "milestones.delete" => tools::milestones::handle_delete_milestone(&self.client, args).await,

            // Labels
            "labels.create" => tools::labels::create_label(&self.client, args).await,
            "labels.list" => tools::labels::list_labels(&self.client, args).await,
            "labels.list_by_project" => tools::labels::list_project_labels(&self.client, args).await,
            "labels.update" => tools::labels::update_label(&self.client, args).await,
            "labels.delete" => tools::labels::handle_delete_label(&self.client, args).await,

            // Search
            "search.all" => tools::search::search_all(&self.client, args).await,
            // PARKED(project-resources): restore with the six registrations and the resource template; see RUNNER-WORK-PACKAGES-STAGES-3-6.md P3.2
            // "code.resources.list" => tools::scenario_tools::code_resources_list(&self.client, args).await,
            // "code.directory.get" => tools::scenario_tools::code_directory_get(&self.client, args).await,
            "code.task_context.get" => tools::scenario_tools::code_task_context_get(&self.client, args).await,
            "code.change_proposal.create" => {
                tools::scenario_tools::code_change_proposal_create(&self.client, args).await
            }
            "documents.extract_summary" => tools::scenario_tools::documents_extract_summary(&self.client, args).await,
            "documents.review_risk" => tools::scenario_tools::documents_review_risk(&self.client, args).await,
            "approval.request" => tools::scenario_tools::approval_request(&self.client, args).await,
            "inspection.report" => tools::scenario_tools::inspection_report(&self.client, args).await,
            "corrective_action.propose" => tools::scenario_tools::corrective_action_propose(&self.client, args).await,

            // Flow v0.4
            "flow.feature_get" => tools::flow_features::get_flow_feature(&self.client, args).await,
            "flow.feature_set" => tools::flow_features::set_flow_feature(&self.client, args).await,
            "objects.get" => tools::objects::get_flow_object(&self.client, args).await,
            "objects.query" => tools::objects::query_flow_objects(&self.client, args).await,
            "objects.history" => tools::objects::get_flow_object_history(&self.client, args).await,
            "objects.create" => tools::objects::create_flow_object(&self.client, args).await,
            "objects.patch" => tools::objects::patch_flow_object(&self.client, args).await,
            "objects.move" => tools::objects::move_flow_object(&self.client, args).await,
            "objects.link" => tools::objects::link_flow_objects(&self.client, args).await,
            "objects.unlink" => tools::objects::unlink_flow_objects(&self.client, args).await,
            "objects.diff" => tools::objects::diff_flow_object(&self.client, args).await,
            "objects.grants_get" => tools::objects::get_flow_object_grants(&self.client, args).await,
            "objects.grants_set" => tools::objects::set_flow_object_grants(&self.client, args).await,
            "objects.inheritance_set" => tools::objects::set_flow_object_inheritance(&self.client, args).await,
            "objects.relations" => tools::objects::list_flow_object_relations(&self.client, args).await,
            "objects.search" => tools::objects::search_flow_objects(&self.client, args).await,
            "collab.projection_lag" => tools::objects::get_flow_projection_lag(&self.client, args).await,
            "collections.describe" => tools::objects::describe_collection(&self.client, args).await,
            "collections.query" => tools::objects::query_collection(&self.client, args).await,
            "records.create" => tools::objects::create_collection_record(&self.client, args).await,
            "objects.reference" => tools::objects::reference_flow_object(&self.client, args).await,
            "objects.unreference" => tools::objects::unreference_flow_object(&self.client, args).await,
            "objects.convert_preview" => tools::objects::convert_preview(&self.client, args).await,
            "objects.convert_commit" => tools::objects::convert_commit(&self.client, args).await,
            "objects.convert_status" => tools::objects::convert_status(&self.client, args).await,
            "objects.convert_retry" => tools::objects::convert_retry(&self.client, args).await,
            "objects.export" => tools::objects::export_flow_object(&self.client, args).await,
            "objects.export_workspace" => tools::objects::export_flow_workspace(&self.client, args).await,
            "objects.import_artifact" => tools::objects::import_flow_artifact(&self.client, args).await,
            "objects.import_preview" => tools::objects::import_flow_preview(&self.client, args).await,
            "objects.import_commit" => tools::objects::import_flow_commit(&self.client, args).await,
            "objects.import_status" => tools::objects::import_flow_status(&self.client, args).await,
            "collab.status" => tools::objects::flow_collab_status(&self.client, args).await,
            "objects.integrity" => tools::objects::flow_integrity(&self.client, args).await,
            "collab.compact" => tools::objects::compact_flow_document(&self.client, args).await,
            "deliveries.replay" => tools::objects::replay_flow_deliveries(&self.client, args).await,
            "collab.rebuild_projection" => tools::objects::rebuild_flow_projection(&self.client, args).await,
            "collab.repair_quarantine" => tools::objects::repair_quarantine(&self.client, args).await,
            tools::tasks::TASK_GET => tools::tasks::get(&self.client, args).await,
            tools::tasks::TASK_ACK => tools::tasks::ack(&self.client, args).await,
            tools::tasks::TASK_PROGRESS => tools::tasks::progress(&self.client, args).await,
            tools::tasks::TASK_COMPLETE => tools::tasks::complete(&self.client, args).await,
            tools::tasks::TASK_FAIL => tools::tasks::fail(&self.client, args).await,
            "flow.comments.list" => tools::flow_social::list_comments(&self.client, args).await,
            "flow.comments.create" => tools::flow_social::create_comment(&self.client, args).await,
            "flow.comments.update" => tools::flow_social::update_comment(&self.client, args).await,
            "flow.comments.delete" => tools::flow_social::delete_comment(&self.client, args).await,
            "flow.comments.resolve" => tools::flow_social::resolve_comment(&self.client, args).await,
            "flow.shares.list" => tools::flow_social::list_share_links(&self.client, args).await,
            "flow.shares.create" => tools::flow_social::create_share_link(&self.client, args).await,
            "flow.shares.revoke" => tools::flow_social::revoke_share_link(&self.client, args).await,
            "flow.appearance.set" => tools::flow_social::set_appearance(&self.client, args).await,
            "flow.appearance.upload" => tools::flow_social::upload_appearance(&self.client, args).await,
            "objects.restore" => tools::flow_social::restore_version(&self.client, args).await,
            "flow.trash.list" => tools::flow_social::list_trash(&self.client, args).await,
            "objects.purge" => tools::flow_social::purge_object(&self.client, args).await,
            "flow.mentions.list" => tools::flow_social::list_mention_candidates(&self.client, args).await,

            _ => CallToolResult::error(format!("Unknown tool: {name}")),
        }
    }

    /// Test-only door for `tools::catalog`'s HTTP method witness: runs the tool body without the
    /// policy chain so that the witness sees exactly the requests the tool itself issues.
    #[cfg(test)]
    pub(crate) async fn execute_tool_for_catalog_witness(&self, name: &str, args: Value) -> CallToolResult {
        self.execute_tool(name, args).await
    }

    /// Resolves the project whose agent policy governs this call.
    ///
    /// The scope comes from [`tool_policy_scope`], i.e. from the registered schema, not
    /// from the arguments. A tool addressed by a resource id carries no trustworthy
    /// `project_id`, so the owning project is always read back from the API; a
    /// `project_id` smuggled into the arguments is never the policy subject and any
    /// disagreement with the real owner refuses the call, because a caller naming a
    /// foreign project is itself a bypass attempt. When the owner cannot be resolved the
    /// call is refused as well (fail closed).
    async fn resolve_policy_project_id(&self, tool_name: &str, args: &Value) -> Result<Option<String>, String> {
        let scope = tool_policy_scope(tool_name);
        // Workspace wide tools have their own, stricter guard below.
        if !matches!(scope, PolicyScope::WorkspaceWide | PolicyScope::WorkspaceWideAdmin) {
            reject_undeclared_ownership_arguments(tool_name, args)?;
        }
        match scope {
            PolicyScope::WorkspaceWide | PolicyScope::WorkspaceWideAdmin => {
                // Nothing this tool touches belongs to a project, so there is no policy
                // to evaluate. Refuse anyway if the call somehow carries an ownership
                // bearing id: that means the classification and the schema disagree, and
                // the safe reading of a disagreement is "not authorized".
                self.reject_ownership_bearing_arguments(tool_name, args)?;
                Ok(None)
            }
            PolicyScope::WorkspaceOrFlowObject => {
                match (uuid_argument(args, "workspace_id")?, uuid_argument(args, "object_id")?) {
                    (Some(_), None) => Ok(None),
                    (None, Some(_)) => {
                        let owner = self.resolve_flow_object_owner(tool_name, args).await?;
                        match &owner {
                            Some(project_id) => self.reject_foreign_project_claims(tool_name, args, project_id)?,
                            None => self.reject_any_project_claim(tool_name, args)?,
                        }
                        Ok(owner)
                    }
                    _ => Err(format!(
                        "Tool '{tool_name}' requires exactly one of workspace_id or object_id"
                    )),
                }
            }
            PolicyScope::DeclaredProject { required } => match declared_project_id(args)? {
                Some(project_id) => Ok(Some(project_id)),
                None if required => {
                    tracing::warn!(
                        tool_name = %tool_name,
                        "Refusing project scoped call: no project_id to evaluate the project agent policy against"
                    );
                    Err(format!(
                        "Tool '{tool_name}' requires a project_id, so the project agent policy cannot be evaluated \
                         without one"
                    ))
                }
                None => Ok(None),
            },
            // A Flow object may belong to no project at all, unlike every other id addressed
            // resource this file governs — see `OwnerLookup::FlowObject`'s doc comment for why
            // this cannot share the generic `OwnedBy(lookup)` arm below, whose owner is always
            // mandatory.
            PolicyScope::OwnedBy(OwnerLookup::FlowObject) => {
                let owner = self.resolve_flow_object_owner(tool_name, args).await?;
                match &owner {
                    Some(project_id) => self.reject_foreign_project_claims(tool_name, args, project_id)?,
                    None => self.reject_any_project_claim(tool_name, args)?,
                }
                Ok(owner)
            }
            PolicyScope::OwnedBy(OwnerLookup::FlowConversion) => {
                let owner = self.resolve_flow_conversion_owner(tool_name, args).await?;
                match &owner {
                    Some(project_id) => self.reject_foreign_project_claims(tool_name, args, project_id)?,
                    None => self.reject_any_project_claim(tool_name, args)?,
                }
                Ok(owner)
            }
            PolicyScope::OwnedBy(lookup) => {
                let owner = self.resolve_owning_project(tool_name, lookup, args).await?;
                self.reject_foreign_project_claims(tool_name, args, &owner)?;
                Ok(Some(owner))
            }
        }
    }

    /// Refuses a call whose schema is workspace scoped (no project agent policy applies) but
    /// which was somehow called with a project-owned resource id anyway. Shared by
    /// `WorkspaceWide` and `WorkspaceWideAdmin`, which differ only in what they address, never
    /// in this guard.
    #[allow(clippy::unused_self)]
    fn reject_ownership_bearing_arguments(&self, tool_name: &str, args: &Value) -> Result<(), String> {
        if let Some((argument, _)) = OWNERSHIP_ARGUMENTS
            .iter()
            .find(|(argument, _)| args.get(*argument).is_some_and(|value| !value.is_null()))
        {
            tracing::warn!(
                tool_name = %tool_name,
                argument = %argument,
                "Refusing workspace scoped call that carries a project owned identifier"
            );
            return Err(format!(
                "Tool '{tool_name}' is registered as workspace scoped but was called with '{argument}'; \
                 refusing because the owning project would go unchecked"
            ));
        }
        Ok(())
    }

    /// Reads the owning project of an id addressed call back from the API.
    async fn resolve_owning_project(
        &self,
        tool_name: &str,
        lookup: OwnerLookup,
        args: &Value,
    ) -> Result<String, String> {
        match lookup {
            OwnerLookup::FlowObject => {
                // `resolve_policy_project_id` never reaches here: `FlowObject`'s owner is
                // optional (a Flow object may belong to no project) and is resolved by its
                // own match arm through `resolve_flow_object_owner`, not through this
                // mandatory-owner path. This arm exists only so the match stays exhaustive
                // over every `OwnerLookup` variant instead of silently falling into the `_`
                // arm below and calling the wrong endpoint if a future refactor ever did
                // route it here.
                Err(format!(
                    "Tool '{tool_name}' is refused: FlowObject ownership must be resolved through \
                     resolve_flow_object_owner, not resolve_owning_project"
                ))
            }
            OwnerLookup::FlowConversion => Err(format!(
                "Tool '{tool_name}' is refused: FlowConversion ownership must be resolved through its source lookup"
            )),
            OwnerLookup::AiTask => self.resolve_ai_task_owner(tool_name, args).await,
            OwnerLookup::FormData => self.resolve_form_data_owner(tool_name, args).await,
            OwnerLookup::WorkItemIdentifier => {
                let identifier = identifier_argument(args)?.ok_or_else(|| {
                    format!("Tool '{tool_name}' carries no identifier, so the owning project cannot be resolved")
                })?;
                let cache_key = format!("identifier:{}", identifier.to_ascii_lowercase());
                let description = format!("work item {identifier}");
                if let Some(owner) = self.cached_project_id(&cache_key).await {
                    return Ok(owner.project_id);
                }
                let response = self
                    .client
                    .get_work_item_by_identifier(&identifier)
                    .await
                    .map_err(|error| format!("Failed to resolve owning project for {description}: {error}"))?;
                self.store_owner(cache_key, &description, &response)
                    .await
                    .map(|owner| owner.project_id)
            }
            _ => {
                let id = required_owner_argument(tool_name, lookup, args)?;
                let (prefix, kind) = match lookup {
                    OwnerLookup::WorkItem => ("work_item", "work item"),
                    OwnerLookup::Plugin => ("plugin", "plugin"),
                    OwnerLookup::Sprint => ("sprint", "sprint"),
                    OwnerLookup::Comment => ("comment", "comment"),
                    _ => ("check_result", "check result"),
                };
                let cache_key = format!("{prefix}:{id}");
                let description = format!("{kind} {id}");
                if let Some(owner) = self.cached_project_id(&cache_key).await {
                    return Ok(owner.project_id);
                }
                let response = match lookup {
                    OwnerLookup::WorkItem => self.client.get_work_item(&id).await,
                    OwnerLookup::Plugin => self.client.get_plugin(&id).await,
                    OwnerLookup::Sprint => self.client.get_sprint(&id).await,
                    OwnerLookup::Comment => self.client.get_comment(&id).await,
                    _ => self.client.get_check_result(&id).await,
                }
                .map_err(|error| format!("Failed to resolve owning project for {description}: {error}"))?;
                self.store_owner(cache_key, &description, &response)
                    .await
                    .map(|owner| owner.project_id)
            }
        }
    }

    /// Reads a Flow object's owning project back from the API, or `None` when the object
    /// belongs to no project at all — the one owner-bearing resource in this file for which
    /// that is a legitimate, expected answer rather than a resolution failure. Not cached: a
    /// v0.4 read is one call, and `project_id_cache`'s `ResolvedOwner` is typed for a
    /// mandatory `project_id`, which a projectless Flow object does not have.
    async fn resolve_flow_object_owner(&self, tool_name: &str, args: &Value) -> Result<Option<String>, String> {
        let object_id = required_owner_argument(tool_name, OwnerLookup::FlowObject, args)?;
        let response = self
            .client
            .get_flow_object(&object_id, "")
            .await
            .map_err(|error| format!("Failed to resolve owning project for flow object {object_id}: {error}"))?;
        let data = response
            .get("data")
            .ok_or_else(|| format!("API response for flow object {object_id} carries no data"))?;
        Ok(data.get("project_id").and_then(Value::as_str).map(str::to_string))
    }

    /// Reads a task's project from the task view. The API serves the view only to the task's own
    /// bot, so a task of another bot (or any caller that is not a bot) fails here with the API's
    /// refusal and never reaches the policy check or the tool.
    async fn resolve_ai_task_owner(&self, tool_name: &str, args: &Value) -> Result<String, String> {
        let (task_id, attempt) = tools::tasks::task_target(args)
            .map_err(|error| format!("Tool '{tool_name}' cannot resolve the task's project: {error}"))?;
        let cache_key = format!("ai_task:{task_id}");
        if let Some(owner) = self.cached_project_id(&cache_key).await {
            return Ok(owner.project_id);
        }
        let response = self
            .client
            .get_ai_task(&task_id, attempt)
            .await
            .map_err(|error| format!("Failed to resolve owning project for task {task_id}: {error}"))?;
        let project_id = response
            .pointer("/data/project/id")
            .and_then(Value::as_str)
            .map(str::to_string)
            .ok_or_else(|| format!("API response for task {task_id} carries no project id"))?;
        let owner = ResolvedOwner {
            project_id,
            form_id: None,
        };
        self.cache_project_id(cache_key, &owner).await;
        Ok(owner.project_id)
    }

    async fn resolve_flow_conversion_owner(&self, tool_name: &str, args: &Value) -> Result<Option<String>, String> {
        let conversion_id = required_owner_argument(tool_name, OwnerLookup::FlowConversion, args)?;
        let path = format!("/api/v2/flow/conversions/{conversion_id}/owner");
        let response: Value = self
            .client
            .get(&path)
            .await
            .map_err(|error| format!("Failed to resolve conversion source owner {conversion_id}: {error}"))?;
        let data = response
            .get("data")
            .ok_or_else(|| format!("API response for conversion {conversion_id} carries no data"))?;
        Ok(data.get("project_id").and_then(Value::as_str).map(str::to_string))
    }

    /// Resolves the owner of a form, record or attachment. When the caller names both a
    /// record and a form, the form claim is checked against the record's real form.
    async fn resolve_form_data_owner(&self, tool_name: &str, args: &Value) -> Result<String, String> {
        let Some((target, declared_form_id)) = form_scope_target(args)? else {
            tracing::warn!(
                tool_name = %tool_name,
                "Refusing form scoped call: no form_id, record_id or attachment_id to resolve the owning project"
            );
            return Err(format!(
                "Tool '{tool_name}' carries no form_id, record_id or attachment_id, so the owning project cannot be \
                 resolved and the project agent policy cannot be evaluated"
            ));
        };

        let owner = self.lookup_project_id(&target).await?;
        if let Some(declared_form_id) = declared_form_id {
            match owner.form_id.as_deref() {
                Some(actual) if actual.eq_ignore_ascii_case(&declared_form_id) => {}
                Some(actual) => {
                    tracing::warn!(
                        tool_name = %tool_name,
                        declared_form_id = %declared_form_id,
                        owning_form_id = %actual,
                        target = %target.describe(),
                        "Refusing call: caller supplied a form_id that does not own the record"
                    );
                    return Err(format!(
                        "Tool '{tool_name}' was called with form_id '{declared_form_id}', but {} belongs to form \
                         '{actual}'; refusing to authorize a call that names a foreign form",
                        target.describe()
                    ));
                }
                None => {
                    return Err(format!(
                        "Tool '{tool_name}' was called with form_id '{declared_form_id}', but the API response for {} \
                         reports no form_id to check it against",
                        target.describe()
                    ));
                }
            }
        }
        Ok(owner.project_id)
    }

    async fn lookup_project_id(&self, target: &FormScopeTarget) -> Result<ResolvedOwner, String> {
        let cache_key = target.cache_key();
        if let Some(owner) = self.cached_project_id(&cache_key).await {
            return Ok(owner);
        }

        let response = match target {
            FormScopeTarget::Form(id) => self.client.get_form(id).await,
            FormScopeTarget::Record(id) => self.client.get_form_record(id).await,
            FormScopeTarget::Attachment(id) => self.client.get_form_attachment(id).await,
        }
        .map_err(|error| format!("Failed to resolve owning project for {}: {error}", target.describe()))?;

        let data = response.get("data");
        let form_id = data
            .and_then(|data| data.get("form_id"))
            .and_then(Value::as_str)
            .map(str::to_string);
        let project_id = if let Some(project_id) = data.and_then(|data| data.get("project_id")).and_then(Value::as_str)
        {
            project_id.to_string()
        } else {
            // Payloads that only identify the owning form (attachments) are resolved
            // one level further instead of being treated as unresolvable.
            let form_id = form_id.as_deref().ok_or_else(|| {
                format!(
                    "API response for {} carries neither project_id nor form_id",
                    target.describe()
                )
            })?;
            self.fetch_form_project_id(form_id).await?
        };

        let owner = ResolvedOwner { project_id, form_id };
        self.cache_project_id(cache_key, &owner).await;
        Ok(owner)
    }

    async fn fetch_form_project_id(&self, form_id: &str) -> Result<String, String> {
        let target = FormScopeTarget::Form(form_id.to_string());
        let cache_key = target.cache_key();
        let description = target.describe();
        if let Some(owner) = self.cached_project_id(&cache_key).await {
            return Ok(owner.project_id);
        }
        let response = self
            .client
            .get_form(form_id)
            .await
            .map_err(|error| format!("Failed to resolve owning project for {description}: {error}"))?;
        self.store_owner(cache_key, &description, &response)
            .await
            .map(|owner| owner.project_id)
    }

    /// Reads the ownership fields out of an API payload and caches them. An answer that
    /// carries no `project_id` is an error, never a silent "no policy applies".
    async fn store_owner(
        &self,
        cache_key: String,
        description: &str,
        response: &Value,
    ) -> Result<ResolvedOwner, String> {
        let data = response.get("data");
        let project_id = data
            .and_then(|data| data.get("project_id"))
            .and_then(Value::as_str)
            .map(str::to_string)
            .ok_or_else(|| format!("API response for {description} carries no project_id"))?;
        let owner = ResolvedOwner {
            project_id,
            form_id: data
                .and_then(|data| data.get("form_id"))
                .and_then(Value::as_str)
                .map(str::to_string),
        };

        self.cache_project_id(cache_key, &owner).await;
        Ok(owner)
    }

    /// Refuses a call that names any project other than the resolved owner, wherever the
    /// claim is hidden (`project_id`, `projectId`, `payload.project_id`, ...).
    #[allow(clippy::unused_self)]
    fn reject_foreign_project_claims(&self, tool_name: &str, args: &Value, owner: &str) -> Result<(), String> {
        for claimed in claimed_project_ids(args) {
            if !claimed.eq_ignore_ascii_case(owner) {
                let claimed = sanitize_for_error(&claimed);
                tracing::warn!(
                    tool_name = %tool_name,
                    declared_project_id = %claimed,
                    owning_project_id = %owner,
                    "Refusing call: caller supplied a project_id that does not own the target"
                );
                return Err(format!(
                    "Tool '{tool_name}' was called with project_id '{claimed}', but the target belongs to project \
                     '{owner}'; refusing to evaluate the project agent policy against a foreign project"
                ));
            }
        }
        Ok(())
    }

    /// For an id addressed call whose target turned out to belong to no project (a Flow
    /// object with `project_id = null`): any project claim in the payload is foreign by
    /// construction, since there is no real owner to match it against, so it is refused
    /// rather than silently ignored (`mcp-surface-v1.md`: "如 payload 任意位置出现 project
    /// claim，必须拒绝而非忽略").
    #[allow(clippy::unused_self)]
    fn reject_any_project_claim(&self, tool_name: &str, args: &Value) -> Result<(), String> {
        if let Some(claimed) = claimed_project_ids(args).into_iter().next() {
            let claimed = sanitize_for_error(&claimed);
            tracing::warn!(
                tool_name = %tool_name,
                declared_project_id = %claimed,
                "Refusing call: caller supplied a project_id for a target that belongs to no project"
            );
            return Err(format!(
                "Tool '{tool_name}' was called with project_id '{claimed}', but the target belongs to no project; \
                 refusing to evaluate a project agent policy that cannot exist"
            ));
        }
        Ok(())
    }

    async fn cached_project_id(&self, cache_key: &str) -> Option<ResolvedOwner> {
        let cache = self.project_id_cache.lock().await;
        cache.get(cache_key).cloned()
    }

    async fn cache_project_id(&self, cache_key: String, owner: &ResolvedOwner) {
        let mut cache = self.project_id_cache.lock().await;
        if cache.len() >= PROJECT_ID_CACHE_LIMIT {
            cache.clear();
        }
        cache.insert(cache_key, owner.clone());
    }

    async fn enforce_project_tool_policy(&self, tool_name: &str, args: &Value) -> Result<(), String> {
        let Some(project_id) = self.resolve_policy_project_id(tool_name, args).await? else {
            return Ok(());
        };

        self.enforce_resolved_project_policy(tool_name, Some(&project_id)).await
    }

    async fn enforce_resolved_project_policy(&self, tool_name: &str, project_id: Option<&str>) -> Result<(), String> {
        let Some(project_id) = project_id else { return Ok(()) };
        let policy = self
            .client
            .get_project_agent_policy(project_id)
            .await
            .map_err(|error| format!("Failed to load project agent policy for tools/call: {error}"))?;
        // The policy subject is a validated UUID and the client rejects anything that is
        // not a `{code, message, data}` envelope, so a response that reached this point
        // really is an agent policy. Requiring the `data` object as well keeps the gate
        // fail closed if either guarantee is ever weakened.
        if !policy.get("data").is_some_and(Value::is_object) {
            return Err(format!(
                "Refusing '{tool_name}': the agent-policy response for project {project_id} carries no policy object"
            ));
        }
        if is_tool_enabled_by_policy(&policy, tool_name) {
            Ok(())
        } else {
            Err(format!(
                "Tool '{tool_name}' is disabled by project agent policy for project {project_id}"
            ))
        }
    }

    /// Whether `agent_id` (a canonical UUID) is the bot of this server's credential. The bot id
    /// is asked of the API once; a failed ask is not remembered.
    async fn is_own_bot(&self, agent_id: &str) -> Result<bool, ResourceError> {
        let own = self
            .bot_id
            .get_or_try_init(|| self.client.bot_identity())
            .await
            .map_err(|error| ResourceError::from_backend("Failed to identify the calling bot", &error))?;
        Ok(own.to_string() == agent_id)
    }

    /// Decides one subscription of a listen request as the caller: the read policy of the URI must
    /// pass and the object must be one the caller may read now. A refusal, an unknown object and
    /// another bot's object all come out as [`Verdict::Denied`], so a caller learns nothing it
    /// could not learn by reading. Only a failure of the API itself is [`Verdict::Unavailable`].
    pub async fn subscription_verdict(&self, uri: &str, subscribable: &Subscribable) -> Verdict {
        if self.delegated.is_some() {
            return Verdict::Denied;
        }
        if let Err(error) = self.enforce_resource_read_policy(uri).await {
            return verdict_of_error(&error);
        }
        match subscribable {
            Subscribable::Task(task_id) => match self.client.get_ai_task_current(&task_id.to_string()).await {
                Ok(_) => Verdict::Granted(SubKey::Task(*task_id)),
                Err(error) => verdict_of_error(&ResourceError::from_backend(
                    "Failed to check a task subscription",
                    &error,
                )),
            },
            Subscribable::Issue(identifier) => match self.client.get_work_item_by_identifier(identifier).await {
                Ok(item) => item
                    .get("data")
                    .unwrap_or(&item)
                    .get("id")
                    .and_then(Value::as_str)
                    .and_then(|raw| Uuid::parse_str(raw).ok())
                    .map_or(Verdict::Denied, |id| Verdict::Granted(SubKey::Issue(id))),
                Err(error) => verdict_of_error(&ResourceError::from_backend(
                    "Failed to check an issue subscription",
                    &error,
                )),
            },
            Subscribable::Inbox(agent_id) => match self.is_own_bot(&agent_id.to_string()).await {
                Ok(true) => Verdict::Granted(SubKey::Inbox),
                Ok(false) => Verdict::Denied,
                Err(error) => verdict_of_error(&error),
            },
        }
    }

    /// Applies the project agent policy to one `resources/read`.
    ///
    /// The resource is mapped to the tool whose policy already governs the same payload
    /// and handed to `enforce_project_tool_policy`. Ownership resolution, foreign project
    /// rejection and the ownership cache are therefore the tool path's, not a second
    /// implementation that could drift away from it: `sylvode://forms/{id}/records` is
    /// authorized by exactly the decision `form_records.list` would have made.
    async fn enforce_resource_read_policy(&self, uri: &str) -> Result<(), ResourceError> {
        match resource_policy_subject(uri).map_err(ResourceError::Invalid)? {
            ResourceSubject::Ungoverned => Ok(()),
            ResourceSubject::Tool { tool, args } => self
                .enforce_project_tool_policy(tool, &args)
                .await
                .map_err(|error| ResourceError::from_policy(&error)),
        }
    }

    /// The capabilities both handshakes announce, before the per-caller additions.
    ///
    /// `subscribe` is declared by the modern handshake only, and only while `[mcp] listen` is on:
    /// the legacy transports offer no push.
    fn base_capabilities(&self, tools: &Value, subscribe: bool) -> Value {
        if self.client.is_flow_only() {
            json!({ "tools": tools })
        } else {
            json!({
                "tools": tools,
                "resources": { "subscribe": subscribe, "listChanged": false }
            })
        }
    }

    /// `server/discover` (modern era): static, answers without any API call.
    ///
    /// The task protocol and the instructions are for a plain caller only: a delegated server has
    /// no task tools and a Flow-only server none of this surface.
    fn handle_discover(&self, id: Option<Value>) -> JsonRpcResponse {
        // Flow-only declares `tools` alone; every other server declares what `[mcp] listen` allows.
        let tools = if self.client.is_flow_only() {
            json!({})
        } else {
            json!({ "listChanged": self.listen })
        };
        let mut capabilities = self.base_capabilities(&tools, self.listen);
        let mut result = json!({ "supportedVersions": era::MODERN_VERSIONS });
        if self.applicable_mode().is_some() {
            if let Some(fields) = capabilities.as_object_mut() {
                fields.insert(
                    "extensions".to_string(),
                    json!({ era::TASK_PROTOCOL_EXTENSION: { "version": tools::entry::TASK_PROTOCOL_VERSION } }),
                );
            }
            if let Some(fields) = result.as_object_mut() {
                fields.insert("instructions".to_string(), json!(guides::SERVER_INSTRUCTIONS));
            }
        }
        if let Some(fields) = result.as_object_mut() {
            fields.insert("capabilities".to_string(), capabilities);
        }
        era::decorate_modern(
            &mut result,
            CachePolicy::Ttl {
                ms: era::TTL_DISCOVER_MS,
                scope: CacheScope::Private,
            },
        );
        JsonRpcResponse::success(id, result)
    }

    /// Legacy `initialize`: echoes the requested protocol version when it is a known one.
    fn handle_initialize(&self, id: Option<Value>, params: Option<&Value>) -> JsonRpcResponse {
        let requested = params
            .and_then(|params| params.get("protocolVersion"))
            .and_then(Value::as_str);
        let mut result = json!({
            "protocolVersion": era::negotiate_legacy(requested),
            "capabilities": self.base_capabilities(&json!({ "listChanged": false }), false),
            "serverInfo": era::server_info()
        });

        // The task protocol and the instructions are for a plain caller only.
        if self.applicable_mode().is_some()
            && let Some(fields) = result.as_object_mut()
        {
            if let Some(capabilities) = fields.get_mut("capabilities").and_then(Value::as_object_mut) {
                capabilities.insert(
                    "experimental".to_string(),
                    json!({ "sylvode/task-protocol": { "version": tools::entry::TASK_PROTOCOL_VERSION } }),
                );
            }
            fields.insert("instructions".to_string(), json!(guides::SERVER_INSTRUCTIONS));
        }
        JsonRpcResponse::success(id, result)
    }

    /// The static resource catalogue.
    ///
    /// Deliberately not policy filtered, unlike `tools/list`. Every entry is a fixed
    /// description of a *shape*: the four names, descriptions and MIME types below are
    /// identical for every caller, carry no project identifier and no project owned data,
    /// and the request itself names no project to filter against. Withholding them would
    /// hide nothing — `resources/read` is where the data lives, and that is gated.
    #[allow(clippy::unused_self)]
    fn handle_resources_list(&self, id: Option<Value>) -> JsonRpcResponse {
        let mut resources = vec![
            json!({
                "uri": "sylvode://skills/sylvode-mcp",
                "name": "Sylvode MCP Skill",
                "description": "How to work on Sylvode tasks and data: the task procedure, finding and calling tools, conventions and recipes.",
                "mimeType": "text/markdown"
            }),
            json!({
                "uri": "sylvode://skills/sylvode-mcp/tools",
                "name": "Sylvode Tool Reference",
                "description": "Every Sylvode tool with its category, whether it reads or writes, and a one line summary.",
                "mimeType": "text/markdown"
            }),
            json!({
                "uri": "sylvode://guides/workflows",
                "name": "Common Workflow Patterns",
                "description": "Recipes: fix an assigned issue, answer a mention, vote on a proposal, find and change data.",
                "mimeType": "text/markdown"
            }),
        ];
        if self.delegated.is_some() {
            // Only the static guides.
            resources.retain(|resource| {
                resource
                    .get("uri")
                    .and_then(Value::as_str)
                    .is_some_and(is_static_guide_resource)
            });
        }

        JsonRpcResponse::success(id, json!({ "resources": resources }))
    }

    /// The URI templates a client can expand.
    ///
    /// Not policy filtered, for the same reason as `handle_resources_list`: a template is
    /// a `{project_id}` placeholder, never a project. Expanding it lands in
    /// `handle_resources_read`, which resolves the real owner and refuses when the policy
    /// does.
    #[allow(clippy::unused_self)]
    fn handle_resources_templates_list(&self, id: Option<Value>) -> JsonRpcResponse {
        if self.delegated.is_some() {
            // Every template addresses data, and delegated callers reach data through the
            // scope-checked tools only.
            return JsonRpcResponse::success(id, json!({ "resourceTemplates": [] }));
        }
        let templates = vec![
            json!({
                "uriTemplate": "sylvode://projects/{project_id}/issues",
                "name": "Project Issues",
                "description": "List issues for a specific project",
                "mimeType": "application/json"
            }),
            json!({
                "uriTemplate": "sylvode://projects/{project_id}/forms",
                "name": "Project Forms",
                "description": "List universal forms for a specific project",
                "mimeType": "application/json"
            }),
            json!({
                "uriTemplate": "sylvode://forms/{form_id}",
                "name": "Form Schema",
                "description": "Read one universal form schema and display metadata",
                "mimeType": "application/json"
            }),
            json!({
                "uriTemplate": "sylvode://forms/{form_id}/records",
                "name": "Form Records",
                "description": "List records for one universal form",
                "mimeType": "application/json"
            }),
            json!({
                "uriTemplate": "sylvode://forms/{form_id}/events",
                "name": "Form Events",
                "description": "Read recent business events for one universal form",
                "mimeType": "application/json"
            }),
            json!({
                "uriTemplate": "sylvode://form-records/{record_id}",
                "name": "Form Record",
                "description": "Read one universal form record",
                "mimeType": "application/json"
            }),
            json!({
                "uriTemplate": "sylvode://form-records/{record_id}/events",
                "name": "Form Record Events",
                "description": "Read recent business events for one universal form record",
                "mimeType": "application/json"
            }),
            json!({
                "uriTemplate": "sylvode://projects/{project_id}/context",
                "name": "Project Context",
                "description": "Read project type, resources, governance, workflow, and policy in one context payload",
                "mimeType": "application/json"
            }),
            json!({
                "uriTemplate": "sylvode://projects/{project_id}/governance",
                "name": "Project Governance",
                "description": "Read governance, workflow, and recent decision context for a project",
                "mimeType": "application/json"
            }),
            json!({
                "uriTemplate": "sylvode://projects/{project_id}/agent-policy",
                "name": "Project Agent Policy",
                "description": "Read effective MCP/AI action policy for a project",
                "mimeType": "application/json"
            }),
            json!({
                "uriTemplate": "sylvode://projects/{project_id}/release-readiness",
                "name": "Project Release Readiness",
                "description": "Read acceptance gates, blockers, and release readiness evidence for a project",
                "mimeType": "application/json"
            }),
            json!({
                "uriTemplate": "sylvode://projects/{project_id}/type",
                "name": "Project Type",
                "description": "Read project type, settings, and scenario metadata for a project",
                "mimeType": "application/json"
            }),
            // PARKED(project-resources): restore with the six registrations and the resource template; see RUNNER-WORK-PACKAGES-STAGES-3-6.md P3.2
            // json!({
            //     "uriTemplate": "sylvode://projects/{project_id}/resources",
            //     "name": "Project Resources",
            //     "description": "Read project resources of kind directory or custom",
            //     "mimeType": "application/json"
            // }),
            json!({
                "uriTemplate": "sylvode://projects/{project_id}/recent-decisions",
                "name": "Project Recent Decisions",
                "description": "Read recent decision context available to project agents",
                "mimeType": "application/json"
            }),
            json!({
                "uriTemplate": "sylvode://projects/{project_id}/sprints",
                "name": "Project Sprints",
                "description": "List sprints for a specific project",
                "mimeType": "application/json"
            }),
            json!({
                "uriTemplate": "sylvode://issues/{identifier}",
                "name": "Issue by Identifier",
                "description": "Get issue details by human-readable identifier (e.g. PRX-42)",
                "mimeType": "application/json"
            }),
            json!({
                "uriTemplate": "sylvode://tasks/{task_id}",
                "name": "Agent Task",
                "description": "Read one task of this bot (current attempt): status, plan, progress and result",
                "mimeType": "application/json"
            }),
            json!({
                "uriTemplate": "sylvode://agents/{agent_id}/inbox",
                "name": "Agent Inbox",
                "description": "The open tasks of this bot; agent_id is the bot's own id",
                "mimeType": "application/json"
            }),
            json!({
                "uriTemplate": "sylvode://objects/{object_id}",
                "name": "Flow Object",
                "description": "Read one Flow object's semantic JSON projection",
                "mimeType": "application/json"
            }),
            json!({
                "uriTemplate": "sylvode://objects/{object_id}/history?limit={limit}",
                "name": "Flow Object History",
                "description": "Read a bounded history page for one Flow object",
                "mimeType": "application/json"
            }),
            json!({
                "uriTemplate": "sylvode://objects/{object_id}/schema",
                "name": "Flow Object Schema",
                "description": "Read the visible schema for one Flow object",
                "mimeType": "application/json"
            }),
            json!({
                "uriTemplate": "sylvode://collections/{collection_id}/records?cursor={cursor}&limit={limit}",
                "name": "Flow Collection Records",
                "description": "Read one policy-filtered page of Collection records",
                "mimeType": "application/json"
            }),
            json!({
                "uriTemplate": "sylvode://workspaces/{workspace_id}/navigator?project_id={project_id}",
                "name": "Flow Navigator",
                "description": "Read a policy-filtered workspace or project navigator",
                "mimeType": "application/json"
            }),
        ];

        JsonRpcResponse::success(id, json!({ "resourceTemplates": templates }))
    }

    /// `resources/read`: the answer is the contents of [`Self::read_resource`], or the JSON-RPC
    /// error its failure maps to in the caller's era.
    async fn handle_resources_read(&self, id: Option<Value>, params: Option<Value>, era: &Era) -> JsonRpcResponse {
        let requested_uri = match params
            .as_ref()
            .and_then(|value| value.get("uri"))
            .and_then(Value::as_str)
        {
            Some(uri) if !uri.is_empty() => uri.to_string(),
            _ => {
                return JsonRpcResponse::error(id, JsonRpcError::invalid_params("Missing required field: uri"));
            }
        };
        match self.read_resource(&requested_uri).await {
            Ok(read) => JsonRpcResponse::success(
                id,
                json!({ "contents": [{ "uri": requested_uri, "mimeType": read.mime_type, "text": read.text }] }),
            ),
            Err(error) => JsonRpcResponse::error(id, error.into_rpc_error(&requested_uri, era.is_modern())),
        }
    }

    /// Reads one resource: the delegated gate, the project agent policy, then the arm of the URI.
    ///
    /// `uri` is the text the client sent; it is never normalised.
    async fn read_resource(&self, uri: &str) -> Result<ResourceRead, ResourceError> {
        if self.delegated.is_some() && !is_static_guide_resource(uri) {
            tracing::warn!(
                uri = %sanitize_for_error(uri),
                "Refusing resources/read: the resource is not available to connected applications"
            );
            return Err(ResourceError::Refused(REFUSED_NOT_AVAILABLE));
        }

        // `resources/read` serves the same project owned payloads `tools/call` does, so it
        // is gated by the same project agent policy. Without this the resource door is a
        // complete bypass of every capability an administrator disabled on the tool door.
        if let Err(error) = self.enforce_resource_read_policy(uri).await {
            tracing::warn!(
                uri = %sanitize_for_error(uri),
                "Refusing resources/read: the project agent policy does not authorize it"
            );
            return Err(error);
        }

        match uri {
            "sylvode://skills/sylvode-mcp" => return Ok(ResourceRead::markdown(guides::SKILL_MD)),
            "sylvode://skills/sylvode-mcp/tools" => return Ok(ResourceRead::markdown(guides::TOOLS_MD)),
            "sylvode://guides/workflows" => return Ok(ResourceRead::markdown(guides::RECIPES_MD)),
            _ => {}
        }

        if let Some((object_id, query)) = parse_flow_object_resource_uri(uri, "/history") {
            return ResourceRead::from_api(
                self.client.get_flow_object_history(object_id, query).await,
                "Failed to read Flow object history resource",
            );
        }

        if let Some((object_id, query)) = parse_flow_object_resource_uri(uri, "/schema") {
            return ResourceRead::from_api(
                self.client
                    .get::<Value>(&format!("/api/v2/flow/objects/{object_id}/schema{query}"))
                    .await,
                "Failed to read Flow object schema resource",
            );
        }

        if let Some((object_id, query)) = parse_flow_object_resource_uri(uri, "") {
            return ResourceRead::from_api(
                self.client.get_flow_object(object_id, query).await,
                "Failed to read Flow object resource",
            );
        }

        if let Some((collection_id, query)) = parse_collection_records_resource_uri(uri) {
            return ResourceRead::from_api(
                self.client
                    .get::<Value>(&format!("/api/v2/flow/collections/{collection_id}/records{query}"))
                    .await,
                "Failed to read Flow Collection records resource",
            );
        }

        if let Some((workspace_id, query)) = parse_navigator_resource_uri(uri) {
            let navigator = match self.client.flow_scope_id(workspace_id).await {
                Ok(scope) => {
                    self.client
                        .get::<Value>(&format!("/api/v2/flow/scopes/{scope}/navigator{query}"))
                        .await
                }
                Err(error) => Err(error),
            };
            return ResourceRead::from_api(navigator, "Failed to read Flow navigator resource");
        }

        if let Some(project_id) = parse_project_resource_uri(uri, "/issues") {
            return ResourceRead::from_api(
                self.client
                    .list_work_items(
                        project_id,
                        &ListWorkItemsQuery {
                            page: Some(1),
                            per_page: Some(50),
                            ..ListWorkItemsQuery::default()
                        },
                    )
                    .await,
                "Failed to read project issues resource",
            );
        }

        if let Some(project_id) = parse_project_resource_uri(uri, "/forms") {
            return ResourceRead::from_api(
                self.client.list_forms(project_id, None, None).await,
                "Failed to read project forms resource",
            );
        }

        if let Some(form_id) = parse_form_resource_uri(uri, "") {
            return ResourceRead::from_api(self.client.get_form(form_id).await, "Failed to read form resource");
        }

        if let Some(form_id) = parse_form_resource_uri(uri, "/records") {
            return ResourceRead::from_api(
                self.client
                    .list_form_records(form_id, &ListRecordsQuery::default())
                    .await,
                "Failed to read form records resource",
            );
        }

        if let Some(form_id) = parse_form_resource_uri(uri, "/events") {
            return ResourceRead::from_api(
                self.client.list_form_events(form_id, None, None, None).await,
                "Failed to read form events resource",
            );
        }

        if let Some(record_id) = parse_form_record_resource_uri(uri, "") {
            return ResourceRead::from_api(
                self.client.get_form_record(record_id).await,
                "Failed to read form record resource",
            );
        }

        if let Some(record_id) = parse_form_record_resource_uri(uri, "/events") {
            return ResourceRead::from_api(
                self.client.list_form_record_events(record_id, None, None, None).await,
                "Failed to read form record events resource",
            );
        }

        if let Some(project_id) = parse_project_resource_uri(uri, "/context") {
            return ResourceRead::from_api(
                self.client.get_project_context(project_id).await,
                "Failed to read project context",
            );
        }

        if let Some(project_id) = parse_project_resource_uri(uri, "/governance") {
            return ResourceRead::from_api(
                self.client.get_project_governance_context(project_id).await,
                "Failed to read project governance",
            );
        }

        if let Some(project_id) = parse_project_resource_uri(uri, "/agent-policy") {
            return ResourceRead::from_api(
                self.client.get_project_agent_policy(project_id).await,
                "Failed to read project agent policy",
            );
        }

        if let Some(project_id) = parse_project_resource_uri(uri, "/release-readiness") {
            return ResourceRead::from_api(
                self.client.get_project_release_readiness(project_id).await,
                "Failed to read project release readiness",
            );
        }

        if let Some(project_id) = parse_project_resource_uri(uri, "/type") {
            return ResourceRead::from_api(
                self.client.get_project(project_id).await,
                "Failed to read project type resource",
            );
        }

        // PARKED(project-resources): restore with the six registrations and the resource template; see RUNNER-WORK-PACKAGES-STAGES-3-6.md P3.2
        // if let Some(project_id) = parse_project_resource_uri(uri, "/resources") {
        //     return ResourceRead::from_api(
        //         self.client.list_project_resources(project_id).await,
        //         "Failed to read project resources",
        //     );
        // }

        if let Some(project_id) = parse_project_resource_uri(uri, "/recent-decisions") {
            return ResourceRead::from_api(
                self.client
                    .get_project_governance_context(project_id)
                    .await
                    .map(|governance| {
                        governance
                            .get("data")
                            .and_then(|data| data.get("recent_decisions"))
                            .cloned()
                            .unwrap_or_else(|| json!([]))
                    }),
                "Failed to read project recent decisions",
            );
        }

        if let Some(project_id) = parse_project_resource_uri(uri, "/sprints") {
            return ResourceRead::from_api(
                self.client.list_sprints(project_id).await,
                "Failed to read project sprints resource",
            );
        }

        if let Some(identifier) = parse_issue_identifier_uri(uri) {
            return ResourceRead::from_api(
                self.client.get_work_item_by_identifier(&identifier).await,
                "Failed to read issue resource",
            );
        }

        // Only the task's own bot may read it, and the API answers another bot's task as not
        // found; the id was validated by the policy step above.
        if let Some(task_id) = parse_task_resource_uri(uri).and_then(canonical_uuid) {
            return ResourceRead::from_api(
                self.client.get_ai_task_current(&task_id).await,
                "Failed to read task resource",
            );
        }

        // The inbox of the caller's own bot only: any other agent id is not found, without a
        // call that could tell whether that bot exists.
        if let Some(agent_id) = parse_agent_inbox_resource_uri(uri).and_then(canonical_uuid) {
            if !self.is_own_bot(&agent_id).await? {
                return Err(ResourceError::NotFound);
            }
            return ResourceRead::from_api(
                self.client.list_active_ai_tasks().await,
                "Failed to read agent inbox resource",
            );
        }

        Err(ResourceError::NotFound)
    }
}

/// How a failed check counts as a verdict: only a backend failure leaves the question open.
const fn verdict_of_error(error: &ResourceError) -> Verdict {
    match error {
        ResourceError::Backend(_) => Verdict::Unavailable,
        ResourceError::NotFound | ResourceError::Refused(_) | ResourceError::Invalid(_) => Verdict::Denied,
    }
}

/// `data.reason` of a refusal for a delegated caller that asks for anything but a static guide.
const REFUSED_NOT_AVAILABLE: &str = "not_available_to_connected_applications";

/// The text of one resource, as `resources/read` answers it. A read always has exactly one
/// content block, so a successful answer never carries an empty `contents` array.
#[derive(Debug, Clone, PartialEq, Eq)]
struct ResourceRead {
    mime_type: &'static str,
    text: String,
}

impl ResourceRead {
    fn markdown(text: &str) -> Self {
        Self {
            mime_type: "text/markdown",
            text: text.to_string(),
        }
    }

    /// The JSON payload of an API read, or the failure mapped to the error of a resource read.
    /// `context` names the read in the message of a backend failure.
    fn from_api(result: Result<impl std::fmt::Display, String>, context: &str) -> Result<Self, ResourceError> {
        result
            .map(|payload| Self {
                mime_type: "application/json",
                text: payload.to_string(),
            })
            .map_err(|error| ResourceError::from_backend(context, &error))
    }
}

/// Why one `resources/read` failed, before the era decides how to say it.
#[derive(Debug, Clone, PartialEq, Eq)]
enum ResourceError {
    /// No such resource: a URI shape this server does not serve, or an object the API does not
    /// know (or will not show).
    NotFound,
    /// The caller may not read it; the reason is `data.reason` of the answer.
    Refused(&'static str),
    /// An id inside the URI is not valid; the text names the field.
    Invalid(String),
    /// The backend failed; the text is the message of a `-32603` answer.
    Backend(String),
}

impl ResourceError {
    /// A failure of an API read: a missing object is not found, a refusal is a refusal, anything
    /// else is a backend failure. The answer names only what was being read; the detail (an
    /// internal address, the requested identifier) goes to the log, never to the caller.
    fn from_backend(context: &str, error: &str) -> Self {
        match classify_api_error(error) {
            ApiErrorClass::NotFound => Self::NotFound,
            ApiErrorClass::Forbidden => Self::Refused("forbidden"),
            ApiErrorClass::Unauthorized | ApiErrorClass::Other => {
                tracing::warn!(context, detail = %sanitize_for_error(error), "A resource read failed in the backend");
                Self::Backend(context.to_string())
            }
        }
    }

    /// A failure of the policy check. A lookup the API failed is classified like a read; text
    /// that is no API failure is the policy's own refusal.
    fn from_policy(error: &str) -> Self {
        match classify_api_error(error) {
            ApiErrorClass::NotFound => Self::NotFound,
            ApiErrorClass::Forbidden => Self::Refused("forbidden"),
            ApiErrorClass::Unauthorized | ApiErrorClass::Other if is_backend_failure(error) => {
                tracing::warn!(detail = %sanitize_for_error(error), "The resource access check failed in the backend");
                Self::Backend("Failed to check the access policy of the resource".to_string())
            }
            ApiErrorClass::Unauthorized | ApiErrorClass::Other => Self::Refused("policy"),
        }
    }

    /// The JSON-RPC error of this failure for the requested `uri`. Messages never carry the URI
    /// text; `data.uri` does, unchanged.
    fn into_rpc_error(self, uri: &str, modern: bool) -> JsonRpcError {
        match self {
            Self::NotFound if modern => JsonRpcError::resource_not_found(uri),
            Self::NotFound => JsonRpcError::resource_not_found_legacy(uri),
            Self::Refused(reason) => JsonRpcError::resource_refused(uri, reason),
            Self::Invalid(message) => JsonRpcError::invalid_params(message),
            Self::Backend(message) => JsonRpcError::internal_error(message),
        }
    }
}

/// The kind of a JSON-RPC id for the request log; the id itself is never logged.
pub const fn id_kind(id: Option<&Value>) -> &'static str {
    match id {
        None => "none",
        Some(Value::String(_)) => "string",
        Some(Value::Number(_)) => "number",
        Some(_) => "other",
    }
}

/// The client name for the request log: from the modern `_meta` or the legacy `initialize`
/// parameters, sanitised and truncated.
fn client_label(era: &Era, req: &JsonRpcRequest) -> String {
    let name = match era {
        Era::Modern { client_info, .. } => client_info.as_ref().and_then(|info| info.get("name")),
        Era::Legacy if req.method == "initialize" => req
            .params
            .as_ref()
            .and_then(|params| params.get("clientInfo"))
            .and_then(|info| info.get("name")),
        Era::Legacy => None,
    };
    name.and_then(Value::as_str)
        .map_or_else(String::new, |name| sanitize_for_error(name).chars().take(64).collect())
}

/// Adds the modern fields to a successful result: `resultType`, `serverInfo`, the cache policy of
/// the method and, for `tools/call`, `structuredContent` when the first text block is JSON.
fn decorate_modern_response(
    method: &str,
    static_guide_read: bool,
    tools_ttl_ms: u64,
    mut response: JsonRpcResponse,
) -> JsonRpcResponse {
    let Some(result) = response.result.as_mut() else {
        return response;
    };
    let cache = match method {
        "server/discover" => return response,
        "tools/list" => CachePolicy::Ttl {
            ms: tools_ttl_ms,
            scope: CacheScope::Private,
        },
        "resources/list" | "resources/templates/list" => CachePolicy::Ttl {
            ms: era::TTL_CATALOGUE_MS,
            scope: CacheScope::Public,
        },
        "resources/read" if static_guide_read => CachePolicy::Ttl {
            ms: era::TTL_CATALOGUE_MS,
            scope: CacheScope::Public,
        },
        "resources/read" => CachePolicy::Ttl {
            ms: era::TTL_BOARD_MS,
            scope: CacheScope::Private,
        },
        _ => CachePolicy::None,
    };
    if method == "tools/call" {
        set_structured_content(result);
    }
    era::decorate_modern(result, cache);
    response
}

/// `structuredContent` = the first text block, when that text is a JSON object or array.
fn set_structured_content(result: &mut Value) {
    let structured = result
        .get("content")
        .and_then(Value::as_array)
        .and_then(|blocks| blocks.first())
        .filter(|block| block.get("type").and_then(Value::as_str) == Some("text"))
        .and_then(|block| block.get("text"))
        .and_then(Value::as_str)
        .and_then(|text| serde_json::from_str::<Value>(text).ok())
        .filter(|value| value.is_object() || value.is_array());
    if let (Some(structured), Some(fields)) = (structured, result.as_object_mut()) {
        fields.insert("structuredContent".to_string(), structured);
    }
}

/// `sylvode://projects/{project_id}/<suffix>` -> the tool whose project agent policy
/// governs the very same payload.
///
/// `/recent-decisions` maps to `context.get_governance` because that is the call it is
/// served from: it reads the governance context and returns one field of it, so it can
/// carry no weaker authorization than the whole document does.
const PROJECT_RESOURCE_POLICY_TOOLS: [(&str, &str); 9] = [
    ("/issues", "work_items.list"),
    ("/forms", "forms.list"),
    ("/context", "context.get_project"),
    ("/governance", "context.get_governance"),
    ("/agent-policy", "context.get_agent_policy"),
    ("/release-readiness", "release.readiness.get"),
    ("/type", "projects.get"),
    // PARKED(project-resources): restore with the six registrations and the resource template; see RUNNER-WORK-PACKAGES-STAGES-3-6.md P3.2
    // ("/resources", "project_resources.list"),
    ("/recent-decisions", "context.get_governance"),
    ("/sprints", "sprints.list"),
];

/// `sylvode://forms/{form_id}<suffix>` -> the governing tool. The empty suffix is last
/// because it matches the bare form URI only (`parse_form_resource_uri` refuses a
/// remainder containing `/`), and reading it last keeps that intent obvious.
const FORM_RESOURCE_POLICY_TOOLS: [(&str, &str); 3] = [
    ("/records", "form_records.list"),
    ("/events", "events.tail"),
    ("", "forms.get"),
];

/// `sylvode://form-records/{record_id}<suffix>` -> the governing tool.
const FORM_RECORD_RESOURCE_POLICY_TOOLS: [(&str, &str); 2] = [("/events", "events.tail"), ("", "form_records.get")];

/// The tool that governs `sylvode://issues/{identifier}`.
const ISSUE_IDENTIFIER_POLICY_TOOL: &str = "work_items.get_by_identifier";

/// What decides whether one `resources/read` is allowed.
#[derive(Debug, Clone, PartialEq, Eq)]
enum ResourceSubject {
    /// Served from process memory, or from a workspace wide catalogue that no project
    /// agent policy can govern — the equivalent tools are `PolicyScope::WorkspaceWide`,
    /// so there is no policy to consult and nothing project owned to leak.
    Ungoverned,
    /// Governed exactly like `tool` invoked with `args`.
    Tool { tool: &'static str, args: Value },
}

/// Classifies a `resources/read` URI into the policy subject that governs it.
///
/// The match order mirrors `handle_resources_read` one for one, and
/// `every_readable_resource_uri_is_governed_like_its_tool` pins the whole pairing, so a
/// resource cannot be served through a door this function does not recognise.
///
/// An id lifted out of a URI is only accepted once it parses as a canonical UUID. That is
/// the same guard the tool path applies to `project_id`, and for the same reason: the
/// value is interpolated into the API URL that both the policy lookup and the read
/// itself address, so a value that could reshape that URL must never reach either.
fn resource_policy_subject(uri: &str) -> Result<ResourceSubject, String> {
    // The guides are compile time constants.
    if matches!(
        uri,
        "sylvode://skills/sylvode-mcp" | "sylvode://skills/sylvode-mcp/tools" | "sylvode://guides/workflows"
    ) {
        return Ok(ResourceSubject::Ungoverned);
    }

    if let Some((object_id, _)) = parse_flow_object_resource_uri(uri, "/history") {
        return Ok(ResourceSubject::Tool {
            tool: "objects.history",
            args: json!({"object_id":canonical_resource_id(object_id,"object_id")?}),
        });
    }
    if let Some((object_id, _)) = parse_flow_object_resource_uri(uri, "/schema") {
        return Ok(ResourceSubject::Tool {
            tool: "objects.get",
            args: json!({"object_id":canonical_resource_id(object_id,"object_id")?}),
        });
    }
    if let Some((object_id, _)) = parse_flow_object_resource_uri(uri, "") {
        return Ok(ResourceSubject::Tool {
            tool: "objects.get",
            args: json!({"object_id":canonical_resource_id(object_id,"object_id")?}),
        });
    }
    if let Some((collection_id, _)) = parse_collection_records_resource_uri(uri) {
        return Ok(ResourceSubject::Tool {
            tool: "collections.query",
            args: json!({"collection_id":canonical_resource_id(collection_id,"collection_id")?}),
        });
    }
    if let Some((workspace_id, query)) = parse_navigator_resource_uri(uri) {
        let mut args = json!({"workspace_id":canonical_resource_id(workspace_id,"workspace_id")?});
        if let Some(project_id) = resource_query_parameter(query, "project_id") {
            args.as_object_mut()
                .ok_or("navigator policy arguments were not an object")?
                .insert(
                    "project_id".to_string(),
                    json!(canonical_resource_id(project_id, "project_id")?),
                );
        }
        return Ok(ResourceSubject::Tool {
            tool: "objects.query",
            args,
        });
    }

    for (suffix, tool) in PROJECT_RESOURCE_POLICY_TOOLS {
        if let Some(project_id) = parse_project_resource_uri(uri, suffix) {
            return Ok(ResourceSubject::Tool {
                tool,
                args: json!({ "project_id": canonical_resource_id(project_id, "project_id")? }),
            });
        }
    }

    for (suffix, tool) in FORM_RESOURCE_POLICY_TOOLS {
        if let Some(form_id) = parse_form_resource_uri(uri, suffix) {
            return Ok(ResourceSubject::Tool {
                tool,
                args: json!({ "form_id": canonical_resource_id(form_id, "form_id")? }),
            });
        }
    }

    for (suffix, tool) in FORM_RECORD_RESOURCE_POLICY_TOOLS {
        if let Some(record_id) = parse_form_record_resource_uri(uri, suffix) {
            return Ok(ResourceSubject::Tool {
                tool,
                args: json!({ "record_id": canonical_resource_id(record_id, "record_id")? }),
            });
        }
    }

    // A task and an inbox are the calling bot's own state: the API answers another bot's task as
    // not found and `is_own_bot` refuses another agent id, so no project policy applies. The ids
    // are still validated before they reach a URL.
    if let Some(task_id) = parse_task_resource_uri(uri) {
        canonical_resource_id(task_id, "task_id")?;
        return Ok(ResourceSubject::Ungoverned);
    }
    if let Some(agent_id) = parse_agent_inbox_resource_uri(uri) {
        canonical_resource_id(agent_id, "agent_id")?;
        return Ok(ResourceSubject::Ungoverned);
    }

    if let Some(identifier) = parse_issue_identifier_uri(uri) {
        // Validated with the allowlist the tool path uses, and the exact string that
        // passed is the one the policy is evaluated against — an identifier that only
        // becomes acceptable after trimming would authorize a different read than the
        // dispatch below performs.
        let args = json!({ "identifier": identifier.as_str() });
        if identifier_argument(&args)?.as_deref() != Some(identifier.as_str()) {
            return Err(format!(
                "Invalid params: issue identifier '{}' is not a work item identifier",
                sanitize_for_error(&identifier)
            ));
        }
        return Ok(ResourceSubject::Tool {
            tool: ISSUE_IDENTIFIER_POLICY_TOOL,
            args,
        });
    }

    // Nothing matched, so `handle_resources_read` reaches no backend for this URI and
    // answers "Unknown resource URI". There is no data access to authorize.
    Ok(ResourceSubject::Ungoverned)
}

/// A project owned id lifted out of a resource URI, or an error that refuses the read.
fn canonical_resource_id(raw: &str, field: &str) -> Result<String, String> {
    canonical_uuid(raw).ok_or_else(|| {
        format!(
            "Invalid params: {field} '{}' is not a canonical UUID",
            sanitize_for_error(raw)
        )
    })
}

fn split_resource_query(uri: &str) -> (&str, &str) {
    match uri.split_once('?') {
        Some((path, _query)) => (path, &uri[path.len()..]),
        None => (uri, ""),
    }
}

fn resource_query_parameter<'a>(query: &'a str, key: &str) -> Option<&'a str> {
    query
        .strip_prefix('?')?
        .split('&')
        .filter_map(|pair| pair.split_once('='))
        .find_map(|(name, value)| (name == key && !value.is_empty()).then_some(value))
}

fn parse_flow_object_resource_uri<'a>(uri: &'a str, suffix: &str) -> Option<(&'a str, &'a str)> {
    let (path, query) = split_resource_query(uri);
    let object_id = path.strip_prefix("sylvode://objects/")?.strip_suffix(suffix)?;
    if object_id.is_empty() || object_id.contains('/') {
        return None;
    }
    Some((object_id, query))
}

fn parse_collection_records_resource_uri(uri: &str) -> Option<(&str, &str)> {
    let (path, query) = split_resource_query(uri);
    let collection_id = path.strip_prefix("sylvode://collections/")?.strip_suffix("/records")?;
    if collection_id.is_empty() || collection_id.contains('/') {
        return None;
    }
    Some((collection_id, query))
}

fn parse_navigator_resource_uri(uri: &str) -> Option<(&str, &str)> {
    let (path, query) = split_resource_query(uri);
    let workspace_id = path.strip_prefix("sylvode://workspaces/")?.strip_suffix("/navigator")?;
    if workspace_id.is_empty() || workspace_id.contains('/') {
        return None;
    }
    Some((workspace_id, query))
}

fn parse_project_resource_uri<'a>(uri: &'a str, suffix: &str) -> Option<&'a str> {
    let project_id = uri.strip_prefix("sylvode://projects/")?.strip_suffix(suffix)?;
    if project_id.is_empty() || project_id.contains('/') {
        return None;
    }
    Some(project_id)
}

fn parse_form_resource_uri<'a>(uri: &'a str, suffix: &str) -> Option<&'a str> {
    let form_id = uri.strip_prefix("sylvode://forms/")?.strip_suffix(suffix)?;
    if form_id.is_empty() || form_id.contains('/') {
        return None;
    }
    Some(form_id)
}

fn parse_form_record_resource_uri<'a>(uri: &'a str, suffix: &str) -> Option<&'a str> {
    let record_id = uri.strip_prefix("sylvode://form-records/")?.strip_suffix(suffix)?;
    if record_id.is_empty() || record_id.contains('/') {
        return None;
    }
    Some(record_id)
}

pub(crate) fn parse_issue_identifier_uri(uri: &str) -> Option<String> {
    let identifier = uri.strip_prefix("sylvode://issues/")?;
    if identifier.is_empty() || identifier.contains('/') {
        return None;
    }

    Some(identifier.to_string())
}

/// `sylvode://tasks/{task_id}` -> the id text as written.
pub(crate) fn parse_task_resource_uri(uri: &str) -> Option<&str> {
    let task_id = uri.strip_prefix("sylvode://tasks/")?;
    if task_id.is_empty() || task_id.contains('/') || task_id.contains('?') {
        return None;
    }
    Some(task_id)
}

/// `sylvode://agents/{agent_id}/inbox` -> the id text as written.
pub(crate) fn parse_agent_inbox_resource_uri(uri: &str) -> Option<&str> {
    let agent_id = uri.strip_prefix("sylvode://agents/")?.strip_suffix("/inbox")?;
    if agent_id.is_empty() || agent_id.contains('/') || agent_id.contains('?') {
        return None;
    }
    Some(agent_id)
}

/// The project whose policy filters `tools/list`. Validated as a UUID for the same
/// reason the `tools/call` gate is: the value is interpolated into the policy URL.
fn extract_tools_list_project_id(params: Option<&Value>) -> Result<Option<String>, String> {
    let Some(params) = params else {
        return Ok(None);
    };
    let Some(raw) = params
        .get("project_id")
        .or_else(|| params.get("projectId"))
        .or_else(|| params.get("context").and_then(|context| context.get("project_id")))
        .or_else(|| params.get("context").and_then(|context| context.get("projectId")))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
    else {
        return Ok(None);
    };
    canonical_uuid(raw).map(Some).ok_or_else(|| {
        format!(
            "Invalid params: project_id '{}' is not a canonical UUID",
            sanitize_for_error(raw)
        )
    })
}

fn string_argument(args: &Value, key: &str) -> Option<String> {
    args.get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
}

/// Parses the canonical UUID text the API uses for every primary key
/// (`8-4-4-4-12` hex with hyphens; the API itself binds these as `Path<Uuid>`).
///
/// Everything else is rejected: path traversal (`../`), percent escapes, query or
/// fragment smuggling (`?`, `#`), embedded slashes, blank strings, over-long input and
/// non-hex characters. This is what stops a caller supplied id from reshaping the URL
/// it is interpolated into — `"<uuid>/work-items?pad="` used to turn the policy lookup
/// `GET /api/v1/projects/{id}/agent-policy` into a completely different, always
/// permissive request. The result is lower-cased so that two spellings of the same id
/// compare equal.
pub(crate) fn canonical_uuid(value: &str) -> Option<String> {
    let value = value.trim();
    if value.len() != 36 {
        return None;
    }
    for (index, byte) in value.bytes().enumerate() {
        let accepted = match index {
            8 | 13 | 18 | 23 => byte == b'-',
            _ => byte.is_ascii_hexdigit(),
        };
        if !accepted {
            return None;
        }
    }
    Some(value.to_ascii_lowercase())
}

/// Trims attacker controlled text before it is echoed into an error message, so a
/// hostile id cannot inject newlines or flood the transcript.
fn sanitize_for_error(value: &str) -> String {
    let cleaned: String = value
        .chars()
        .filter(|character| !character.is_control())
        .take(64)
        .collect();
    if value.chars().filter(|c| !c.is_control()).count() > 64 {
        format!("{cleaned}...")
    } else {
        cleaned
    }
}

/// Reads a UUID argument. A present-but-invalid value is an error, never a silently
/// missing one: treating it as absent would drop the call back to "no policy applies".
fn uuid_argument(args: &Value, key: &str) -> Result<Option<String>, String> {
    let Some(raw) = args.get(key) else {
        return Ok(None);
    };
    if raw.is_null() {
        return Ok(None);
    }
    let text = raw
        .as_str()
        .ok_or_else(|| format!("Invalid input: {key} must be a string carrying a UUID"))?;
    canonical_uuid(text).map(Some).ok_or_else(|| {
        format!(
            "Invalid input: {key} '{}' is not a canonical UUID",
            sanitize_for_error(text)
        )
    })
}

/// Human readable work item identifiers (`PRX-42`) are not UUIDs, so they get their own
/// conservative character allowlist before they are used to address the API.
pub(crate) fn identifier_argument(args: &Value) -> Result<Option<String>, String> {
    let Some(identifier) = string_argument(args, "identifier") else {
        return Ok(None);
    };
    let accepted = identifier.len() <= 64
        && identifier
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'));
    if accepted {
        Ok(Some(identifier))
    } else {
        Err(format!(
            "Invalid input: identifier '{}' is not a work item identifier",
            sanitize_for_error(&identifier)
        ))
    }
}

fn required_owner_argument(tool_name: &str, lookup: OwnerLookup, args: &Value) -> Result<String, String> {
    if lookup == OwnerLookup::FlowObject {
        for key in ["object_id", "source_object_id", "collection_id"] {
            if let Some(value) = uuid_argument(args, key)? {
                return Ok(value);
            }
        }
        return Err(format!(
            "Tool '{tool_name}' carries no {}, so the owning project cannot be resolved and the project agent policy cannot be evaluated",
            lookup.argument()
        ));
    }
    let key = lookup.argument();
    uuid_argument(args, key)?.ok_or_else(|| {
        format!(
            "Tool '{tool_name}' carries no {key}, so the owning project cannot be resolved and the project agent \
             policy cannot be evaluated"
        )
    })
}

/// The `project_id` a declared-project call is authorized against. Only the top level
/// argument counts, because that is the one the tool implementations actually send to
/// the API; a nested `payload.project_id` is caller payload, not a routing decision.
fn declared_project_id(args: &Value) -> Result<Option<String>, String> {
    if let Some(project_id) = uuid_argument(args, "project_id")? {
        return Ok(Some(project_id));
    }
    uuid_argument(args, "projectId")
}

/// Every place a caller can claim a project, used to refuse id addressed calls that
/// name someone other than the resolved owner.
fn claimed_project_ids(args: &Value) -> Vec<String> {
    ["project_id", "projectId"]
        .iter()
        .filter_map(|key| args.get(*key))
        .chain(
            args.get("payload")
                .into_iter()
                .flat_map(|payload| ["project_id", "projectId"].map(|key| payload.get(key)))
                .flatten(),
        )
        .filter_map(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
        .collect()
}

/// Whether the project agent policy lists this tool.
///
/// A policy that carries no `data.mcp.tool_registry.enabled_tools` array admits every
/// tool. That is deliberate, and it is *not* the "unconfigured projects are ungoverned"
/// hole it looks like at first read: `enabled_tools` is not a stored opt-in setting that
/// an administrator may forget to fill in. The API derives it on every request from the
/// project type's capabilities (`apps/api/src/routes/context.rs` `build_agent_policy` ->
/// `build_mcp_tool_registry`), which always emits the array — at minimum the unconditional
/// `core` group — so a live API answer always takes the `contains` branch. A project whose
/// type does not enable a capability really does get its corresponding tools refused.
///
/// The fallback therefore only fires when the field is missing altogether: an API older or
/// newer than this binary, or a policy payload reshaped in transit. Failing closed there
/// would take every tool down on a schema change rather than on a policy decision, so the
/// choice is intentional. It is also not the only line of defence: the response must be a
/// `{code: 0, ...}` envelope with a `data` object (`enforce_project_tool_policy`), the
/// project must be a canonical UUID, and id addressed tools still resolve their real owner.
fn is_tool_enabled_by_policy(policy_response: &Value, tool_name: &str) -> bool {
    tools::capabilities::extract_enabled_tool_names(policy_response).is_none_or(|enabled| enabled.contains(tool_name))
}

#[cfg(test)]
fn summarize_tool_result(result: &CallToolResult) -> Option<String> {
    let text = result.content.iter().find_map(|item| match item {
        crate::protocol::ToolContent::Text { text } => Some(text.as_str()),
        _ => None,
    })?;
    if text.chars().count() > 2_000 {
        Some(text.chars().take(2_000).collect())
    } else {
        Some(text.to_string())
    }
}

#[cfg(test)]
fn redact_tool_arguments(args: &Value) -> Value {
    match args {
        Value::Object(map) => Value::Object(
            map.iter()
                .map(|(key, value)| {
                    let lower = key.to_ascii_lowercase();
                    let redacted = if lower.contains("token")
                        || lower.contains("secret")
                        || lower.contains("password")
                        || lower.contains("authorization")
                    {
                        Value::String("[REDACTED]".to_string())
                    } else {
                        redact_tool_arguments(value)
                    };
                    (key.clone(), redacted)
                })
                .collect(),
        ),
        Value::Array(items) => Value::Array(items.iter().map(redact_tool_arguments).collect()),
        _ => args.clone(),
    }
}

#[cfg(test)]
mod tests;
