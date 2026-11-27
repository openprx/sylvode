//! Which MCP tools a delegated caller (an OAuth access token, as opposed to an `opr_` bot token or a
//! user token) may see and call, and under which scope.
//!
//! Default deny: a tool is available to a delegated caller only when it has an entry in
//! [`SCOPED_TOOLS`]. Every other tool, and any name that is in no table at all, is refused and is
//! absent from `tools/list`. [`WITHHELD_TOOLS`] lists the registered tools that were decided
//! against, so that adding a tool to the registry without deciding it fails
//! `every_registered_tool_has_a_decision`.
//!
//! A delegated caller reads work items, projects, comments and the linked pages of a work item,
//! and reads and writes the records of forms. Only tools whose API routes are on the delegated
//! allow-list (`apps/api/src/oauth/delegated.rs`, `DELEGATED_ROUTES`) are opened, each under the scope
//! of its route. The tools that map to `work_items:write` stay withheld whatever scopes the token
//! carries: no delegated route changes a work item through this surface yet. The write scopes keep
//! their hierarchy (a `:write` scope implies its `:read` scope) so that a token carrying one still reads.

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
    ("work_items.list_labels", Scope::WorkItemsRead),
    ("comments.list", Scope::WorkItemsRead),
    ("work_items.list_pages", Scope::WorkItemsRead),
    // The six form routes that are delegated: list a project's forms, read a form, list, read,
    // create and update records. Each route authorizes as a member and applies the form's own field
    // policies. Every other form tool (design, permissions, attachments, views, children, import,
    // export, links, aggregates, events) has no delegated route and stays withheld.
    ("forms.list", Scope::FormsRead),
    ("forms.get", Scope::FormsRead),
    ("form_records.list", Scope::FormsRead),
    ("form_records.get", Scope::FormsRead),
    ("form_records.create", Scope::FormsWrite),
    ("form_records.update", Scope::FormsWrite),
];

/// Registered tools that no delegated caller may use.
///
/// The tools that change work items, labels or comments are mapped to a write scope in the contract
/// and are withheld. The rest are administration, deletion, agent dispatch, import and export,
/// plugins, Flow and anything else whose API route is not delegated. The last two names belong to
/// the Flow-only registry, which delegated callers cannot reach at all.
pub const WITHHELD_TOOLS: &[&str] = &[
    // Read tools whose API routes are not on the delegated allow-list, so they could only fail
    // closed: search is not scoped to a grant's projects, the project label list is bot-only, and
    // the remaining form reads (schema summary, views, children) have no delegated route.
    "work_items.search",
    "labels.list_by_project",
    "forms.schema_summary",
    "form_views.list",
    "form_records.children",
    "work_items.create",
    "work_items.update",
    "work_items.add_label",
    "work_items.add_labels",
    "work_items.link_page",
    "work_items.unlink_page",
    "work_items.remove_label",
    "comments.create",
    "form_records.child_create",
    "form_records.child_update",
    "projects.create",
    "projects.update",
    "projects.delete",
    "project_types.list",
    "project_types.get",
    "forms.create",
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
    "forms.validate_schema",
    "forms.validate_rules",
    "forms.create_schema_draft",
    "layouts.validate",
    "layouts.create_draft",
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
    "milestones.list",
    "milestones.get",
    "milestones.create",
    "milestones.update",
    "milestones.delete",
    "labels.create",
    "labels.list",
    "labels.update",
    "labels.delete",
    "search.all",
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
    "objects.list_work_items",
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
    // The task tools act for a bot, and the API refuses anything but the task's own bot.
    "sylvode_task_get",
    "sylvode_task_ack",
    "sylvode_task_progress",
    "sylvode_task_complete",
    "sylvode_task_fail",
    // The search mode entry tools. A delegated server ignores the tool mode, and the wrappers would
    // only re-expose the delegated tools under another name.
    "sylvode_overview",
    "sylvode_search",
    "sylvode_read",
    "sylvode_write",
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
            .chain(crate::tools::entry::definitions())
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
        assert_eq!(SCOPED_TOOLS.len(), 15);
        assert_eq!(WITHHELD_TOOLS.len(), 150);
        // The main registry (159 tools) and the four search mode entry tools are fully covered; the two
        // extra names are Flow-only.
        assert_eq!(crate::tools::get_all_tool_definitions().len(), 159);
        assert_eq!(SCOPED_TOOLS.len() + WITHHELD_TOOLS.len(), 159 + 4 + 2);
    }

    /// The five task tools are closed to every connected application, whatever its scopes, and
    /// the refusal is the one every other withheld tool gets.
    #[test]
    fn the_task_tools_are_closed_to_delegated_callers() {
        let workspace = Uuid::from_u128(1);
        let reference = DelegatedGrant {
            scopes: ScopeSet::all(),
            workspace_id: workspace,
            projects: None,
        };
        let expected = gate_tool_call(&reference, "work_items.delete", None).err();
        assert_eq!(expected, Some(crate::oauth::ToolRefusal::NotAvailable));
        for name in crate::tools::tasks::TASK_TOOL_NAMES {
            assert_eq!(decision(name), ToolDecision::NotAvailable, "{name}");
            assert!(WITHHELD_TOOLS.contains(&name), "{name}");
            for scopes in scope_sets() {
                assert!(!allowed(scopes, name), "{name} under {}", scopes.to_scope_string());
                let grant = DelegatedGrant {
                    scopes,
                    workspace_id: workspace,
                    projects: None,
                };
                assert_eq!(
                    gate_tool_call(&grant, name, None).err(),
                    expected,
                    "{name} under {}",
                    scopes.to_scope_string()
                );
            }
        }
    }

    /// The delegated tool set is exactly the fifteen tools of the table, under every grant: nothing
    /// else in the registry is listed, task tools included.
    #[test]
    fn the_delegated_listing_is_exactly_the_scoped_tools() {
        let listed = |scopes: ScopeSet| -> BTreeSet<String> {
            registered().into_iter().filter(|name| allowed(scopes, name)).collect()
        };
        let scoped = SCOPED_TOOLS
            .iter()
            .map(|(name, _)| (*name).to_string())
            .collect::<BTreeSet<_>>();
        assert_eq!(scoped.len(), 15);
        assert_eq!(listed(ScopeSet::all()), scoped);
        for scopes in scope_sets() {
            assert!(listed(scopes).is_subset(&scoped), "{}", scopes.to_scope_string());
        }
    }

    /// Only the two form record writes are opened under a write scope; every other write tool,
    /// the work item and page link writes included, stays withheld.
    #[test]
    fn only_the_form_record_writes_unlock_under_a_write_scope() {
        for (name, scope) in SCOPED_TOOLS {
            let expected = match *name {
                "form_records.create" | "form_records.update" => Scope::FormsWrite,
                "forms.list" | "forms.get" | "form_records.list" | "form_records.get" => Scope::FormsRead,
                "projects.list" | "projects.get" | "context.get_project" => Scope::ProjectsRead,
                _ => Scope::WorkItemsRead,
            };
            assert_eq!(*scope, expected, "{name}");
        }
        let writes = [
            "work_items.create",
            "work_items.update",
            "work_items.add_label",
            "work_items.add_labels",
            "work_items.remove_label",
            "work_items.link_page",
            "work_items.unlink_page",
            "comments.create",
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
                projects: None,
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
    fn the_filter_per_scope_set_is_the_contract_table() {
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
        assert_eq!(names(ScopeSet::all()).len(), 15);
        // Write scopes imply their read scope.
        assert_eq!(
            names(ScopeSet::from_scopes(&[Scope::FormsRead])),
            ["forms.list", "forms.get", "form_records.list", "form_records.get"]
        );
        assert_eq!(
            names(ScopeSet::from_scopes(&[Scope::FormsWrite])),
            [
                "forms.list",
                "forms.get",
                "form_records.list",
                "form_records.get",
                "form_records.create",
                "form_records.update"
            ]
        );
        assert_eq!(names(ScopeSet::from_scopes(&[Scope::WorkItemsWrite])).len(), 6);
        assert_eq!(names(ScopeSet::from_scopes(&[Scope::WorkItemsRead])).len(), 6);
        // Scopes that belong to the API only unlock nothing here.
        assert!(names(ScopeSet::from_scopes(&[Scope::NotificationsRead, Scope::TasksRead])).is_empty());
    }
}
