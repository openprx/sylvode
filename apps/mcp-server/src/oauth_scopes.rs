//! Which MCP tools a delegated caller (an OAuth access token, as opposed to an `opr_` bot token or a
//! user token) may see and call, and under which scope.
//!
//! Default deny: a tool is available to a delegated caller only when it has an entry in
//! [`SCOPED_TOOLS`]. Every other tool, and any name that is in no table at all, is refused and is
//! absent from `tools/list`. [`WITHHELD_TOOLS`] lists the registered tools that were decided
//! against, so that adding a tool to the registry without deciding it fails
//! `every_registered_tool_has_a_decision`.
//!
//! First release: delegated callers get read tools only. The write tools that the contract maps to
//! `work_items:write` and `forms:write` are withheld whatever scopes the token carries, because a
//! delegated caller must never change data through this surface yet. The write scopes keep their
//! hierarchy (a `:write` scope implies its `:read` scope) so that a token carrying one still reads.

use api::oauth::scopes::{Scope, ScopeSet};

/// What a delegated caller may do with one tool.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ToolDecision {
    /// Callable by a token whose scopes allow this scope (hierarchy included).
    Requires(Scope),
    /// Not callable by any delegated caller, whatever its scopes claim.
    NotAvailable,
}

/// Tools a delegated caller may use, each with the scope it requires.
pub const SCOPED_TOOLS: &[(&str, Scope)] = &[
    ("projects.list", Scope::ProjectsRead),
    ("projects.get", Scope::ProjectsRead),
    ("context.get_project", Scope::ProjectsRead),
    ("work_items.list", Scope::WorkItemsRead),
    ("work_items.get", Scope::WorkItemsRead),
    ("work_items.get_by_identifier", Scope::WorkItemsRead),
    ("work_items.search", Scope::WorkItemsRead),
    ("work_items.list_labels", Scope::WorkItemsRead),
    ("comments.list", Scope::WorkItemsRead),
    ("labels.list_by_project", Scope::WorkItemsRead),
    ("forms.list", Scope::FormsRead),
    ("forms.get", Scope::FormsRead),
    ("forms.schema_summary", Scope::FormsRead),
    ("form_views.list", Scope::FormsRead),
    ("form_records.list", Scope::FormsRead),
    ("form_records.get", Scope::FormsRead),
    ("form_records.children", Scope::FormsRead),
];

/// Registered tools that no delegated caller may use.
///
/// The first ten change data and are mapped to a write scope in the contract; they are withheld in
/// the first release. The rest are administration, deletion, agent dispatch, import and export,
/// plugins, Flow and anything else outside the first release's read surface. The last two names
/// belong to the Flow-only registry, which delegated callers cannot reach at all.
pub const WITHHELD_TOOLS: &[&str] = &[
    "work_items.create",
    "work_items.update",
    "work_items.add_label",
    "work_items.add_labels",
    "work_items.remove_label",
    "comments.create",
    "form_records.create",
    "form_records.update",
    "form_records.child_create",
    "form_records.child_update",
    "projects.create",
    "projects.update",
    "projects.delete",
    "project_types.list",
    "project_types.get",
    "scenario_templates.list",
    "scenario_templates.get",
    "scenario_templates.install",
    "project_resources.list",
    "project_resources.create",
    "project_resources.update",
    "project_resources.delete",
    "forms.create",
    "forms.create_from_template",
    "forms.update_schema",
    "forms.duplicate",
    "forms.field_usage",
    "forms.field_dependencies",
    "form_schema_versions.list",
    "form_schema_versions.get",
    "form_permissions.get",
    "form_permissions.update",
    "form_attachments.list",
    "form_attachments.create",
    "form_attachments.archive",
    "form_attachments.restore",
    "form_records.export",
    "form_records.import_preview",
    "form_records.import_commit",
    "form_records.link",
    "form_records.relation_targets",
    "form_records.child_archive",
    "form_records.child_restore",
    "form_records.aggregate",
    "events.tail",
    "plugins.list",
    "plugins.get",
    "plugins.install",
    "plugins.invoke",
    "plugin_invocations.list",
    "context.get_governance",
    "context.get_agent_policy",
    "work_items.delete",
    "comments.delete",
    "files.upload",
    "proposals.list",
    "proposals.get",
    "proposals.create",
    "proposals.create_from_result",
    "check_results.create",
    "release.readiness.get",
    "members.list",
    "bot_operation_logs.list",
    "sprints.create",
    "sprints.list",
    "sprints.update",
    "sprints.delete",
    "labels.create",
    "labels.list",
    "labels.update",
    "labels.delete",
    "search.all",
    "code.resources.list",
    "code.directory.get",
    "code.task_context.get",
    "code.change_proposal.create",
    "documents.extract_summary",
    "documents.review_risk",
    "approval.request",
    "inspection.report",
    "corrective_action.propose",
    "flow.feature_get",
    "flow.feature_set",
    "objects.get",
    "objects.query",
    "objects.history",
    "objects.create",
    "objects.patch",
    "objects.move",
    "objects.link",
    "objects.unlink",
    "objects.diff",
    "objects.grants_get",
    "objects.grants_set",
    "objects.inheritance_set",
    "objects.relations",
    "objects.search",
    "collections.describe",
    "collections.query",
    "records.create",
    "objects.reference",
    "objects.unreference",
    "objects.convert_preview",
    "objects.convert_commit",
    "objects.convert_status",
    "objects.convert_retry",
    "objects.export",
    "objects.export_workspace",
    "objects.import_artifact",
    "objects.import_preview",
    "objects.import_commit",
    "objects.import_status",
    "collab.status",
    "objects.integrity",
    "collab.compact",
    "deliveries.replay",
    "collab.rebuild_projection",
    "collab.repair_quarantine",
    "collab.projection_lag",
    "legacy_pages.inventory",
    "legacy_pages.import_preview",
    "legacy_pages.import_commit",
    "legacy_pages.import_status",
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
    "objects.restore",
    "flow.trash.list",
    "objects.purge",
    "flow.mentions.list",
    "flow.home",
    "objects.command",
];

/// The decision for `tool`. A name with no entry is `NotAvailable`.
pub fn decision(tool: &str) -> ToolDecision {
    SCOPED_TOOLS
        .iter()
        .find(|(name, _)| *name == tool)
        .map_or(ToolDecision::NotAvailable, |(_, scope)| ToolDecision::Requires(*scope))
}

/// Whether `scopes` lets a delegated caller see and call `tool`.
pub fn allowed(scopes: ScopeSet, tool: &str) -> bool {
    match decision(tool) {
        ToolDecision::Requires(required) => scopes.allows(required),
        ToolDecision::NotAvailable => false,
    }
}

/// Whether `tool` has an explicit decision, either way.
pub fn is_decided(tool: &str) -> bool {
    SCOPED_TOOLS.iter().any(|(name, _)| *name == tool) || WITHHELD_TOOLS.contains(&tool)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::oauth::{DelegatedGrant, gate_tool_call};
    use std::collections::BTreeSet;
    use uuid::Uuid;

    fn registered() -> BTreeSet<String> {
        crate::tools::get_all_tool_definitions()
            .into_iter()
            .chain(crate::tools::flow_scope::definitions())
            .map(|tool| tool.name)
            .collect()
    }

    /// Adding a tool to either registry without deciding it for delegated callers fails here.
    #[test]
    fn every_registered_tool_has_a_decision() {
        let undecided: Vec<String> = registered().into_iter().filter(|name| !is_decided(name)).collect();
        assert!(
            undecided.is_empty(),
            "tools with no delegated-access decision (add each to SCOPED_TOOLS or WITHHELD_TOOLS): {undecided:?}"
        );
    }

    /// A renamed or removed tool leaves no stale entry behind.
    #[test]
    fn every_table_entry_names_a_registered_tool() {
        let registered = registered();
        let stale: Vec<&str> = SCOPED_TOOLS
            .iter()
            .map(|(name, _)| *name)
            .chain(WITHHELD_TOOLS.iter().copied())
            .filter(|name| !registered.contains(*name))
            .collect();
        assert!(
            stale.is_empty(),
            "table entries that are not registered tools: {stale:?}"
        );
    }

    #[test]
    fn no_tool_is_decided_twice() {
        let mut seen = BTreeSet::new();
        for name in SCOPED_TOOLS
            .iter()
            .map(|(name, _)| *name)
            .chain(WITHHELD_TOOLS.iter().copied())
        {
            assert!(seen.insert(name), "{name} has two decisions");
        }
    }

    #[test]
    fn the_first_release_exposes_exactly_these_totals() {
        assert_eq!(SCOPED_TOOLS.len(), 17);
        assert_eq!(WITHHELD_TOOLS.len(), 139);
        // The main registry (154 tools) is fully covered; the two extra names are Flow-only.
        assert_eq!(crate::tools::get_all_tool_definitions().len(), 154);
        assert_eq!(SCOPED_TOOLS.len() + WITHHELD_TOOLS.len(), 154 + 2);
    }

    #[test]
    fn only_read_scopes_unlock_tools() {
        for (name, scope) in SCOPED_TOOLS {
            assert!(
                matches!(scope, Scope::ProjectsRead | Scope::WorkItemsRead | Scope::FormsRead),
                "{name} requires {scope:?}"
            );
        }
        let writes = [
            "work_items.create",
            "work_items.update",
            "work_items.add_label",
            "work_items.add_labels",
            "work_items.remove_label",
            "comments.create",
            "form_records.create",
            "form_records.update",
            "form_records.child_create",
            "form_records.child_update",
        ];
        for name in writes {
            assert_eq!(decision(name), ToolDecision::NotAvailable, "{name}");
        }
    }

    #[test]
    fn dispatching_deleting_and_administering_tools_are_withheld() {
        for name in [
            "work_items.delete",
            "projects.delete",
            "comments.delete",
            "plugins.invoke",
            "plugins.install",
            "files.upload",
            "proposals.create",
            "check_results.create",
            "approval.request",
            "form_records.import_commit",
            "form_records.export",
            "objects.purge",
            "deliveries.replay",
            "bot_operation_logs.list",
            "members.list",
        ] {
            assert_eq!(decision(name), ToolDecision::NotAvailable, "{name}");
        }
    }

    fn scope_sets() -> Vec<ScopeSet> {
        let mut sets = vec![ScopeSet::EMPTY, ScopeSet::all()];
        for scope in Scope::ALL {
            sets.push(ScopeSet::EMPTY.with(scope));
        }
        sets.push(ScopeSet::from_scopes(&[Scope::ProjectsRead, Scope::WorkItemsRead]));
        sets.push(ScopeSet::from_scopes(&[Scope::FormsWrite, Scope::WorkItemsWrite]));
        sets
    }

    /// `tools/list` and `tools/call` agree for every registered tool under every kind of grant:
    /// a tool that is not listed is refused, and a listed tool passes the gate.
    #[test]
    fn the_list_filter_and_the_call_gate_agree() {
        let workspace = Uuid::from_u128(1);
        for scopes in scope_sets() {
            let grant = DelegatedGrant {
                scopes,
                workspace_id: workspace,
            };
            for name in registered().iter().map(String::as_str).chain(["unknown.tool", ""]) {
                assert_eq!(
                    allowed(scopes, name),
                    gate_tool_call(&grant, name, None).is_ok(),
                    "{name} under {}",
                    scopes.to_scope_string()
                );
            }
        }
    }

    #[test]
    fn the_filter_per_scope_set_is_the_contract_table_for_reads() {
        let names = |scopes: ScopeSet| -> Vec<&'static str> {
            SCOPED_TOOLS
                .iter()
                .filter(|(_, required)| scopes.allows(*required))
                .map(|(name, _)| *name)
                .collect()
        };
        assert!(names(ScopeSet::EMPTY).is_empty());
        assert_eq!(
            names(ScopeSet::from_scopes(&[Scope::ProjectsRead])),
            ["projects.list", "projects.get", "context.get_project"]
        );
        assert_eq!(names(ScopeSet::all()).len(), 17);
        // Write scopes imply their read scope.
        assert_eq!(names(ScopeSet::from_scopes(&[Scope::FormsWrite])).len(), 7);
        assert_eq!(names(ScopeSet::from_scopes(&[Scope::WorkItemsWrite])).len(), 7);
        // Scopes that belong to the API only unlock nothing here.
        assert!(names(ScopeSet::from_scopes(&[Scope::NotificationsRead, Scope::TasksRead])).is_empty());
    }
}
