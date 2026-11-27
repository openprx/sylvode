//! Catalogue entries of the `projects` router area (`router/projects.rs`).

use crate::catalogue::types::dsl::*;

#[rustfmt::skip]
pub static ENTRIES: &[Entry] = &[
    bot("governance.decision_domains.read", Get, "/api/v1/decision-domains", Workspace, Read),
    bot("governance.trust_scores.read", Get, "/api/v1/trust-scores/{user_id}", Workspace, Read),
    bot("governance.trust_scores.read/domain", Get, "/api/v1/trust-scores/{user_id}/{domain}", Workspace, Read),
    bot("governance.trust_scores.read/history", Get, "/api/v1/trust-scores/{user_id}/history", Workspace, Read),
    bot("governance.trust_scores.read/list", Get, "/api/v1/trust-scores", Workspace, Read),
    bot("governance.trust_scores.read/user", Get, "/api/v1/users/{id}/trust", Workspace, Read),
    console("legacy_plugins.contributions.invoke", Post, "/api/v1/form-records/{record_id}/plugin-contributions/{key}/{contribution_id}/invoke", Project(Find::Table("form_records", "record_id")), Write).experimental(),
    console("legacy_plugins.contributions.invoke/data", Post, "/api/v1/projects/{project_id}/plugin-contributions/{key}/{contribution_id}/data", Project(Find::Project("project_id")), Write).experimental(),
    console("legacy_plugins.contributions.read/entry", Get, "/api/v1/projects/{project_id}/plugin-contributions/{key}/{contribution_id}/entry", Project(Find::Project("project_id")), Read).experimental(),
    console("legacy_plugins.contributions.read/list", Get, "/api/v1/projects/{project_id}/plugin-contributions", Project(Find::Project("project_id")), Read).experimental(),
    console("legacy_plugins.contributions.read/record", Get, "/api/v1/form-records/{record_id}/plugin-contributions", Project(Find::Table("form_records", "record_id")), Read).experimental(),
    bot("legacy_plugins.invocations.read", Get, "/api/v1/plugins/{plugin_id}/invocations", Project(Find::Table("plugins","plugin_id")), Read).experimental(),
    console("legacy_plugins.invocations.read/packages", Get, "/api/v1/projects/{project_id}/plugin-packages/invocations", Project(Find::Project("project_id")), Read).experimental(),
    console("legacy_plugins.packages.delete/library", Delete, "/api/v1/projects/{project_id}/plugin-packages/library/{package_id}", Project(Find::Project("project_id")), Destructive).experimental(),
    console("legacy_plugins.packages.delete/mine", Delete, "/api/v1/me/form-plugin-packages/{package_id}", Own, Destructive).experimental(),
    console("legacy_plugins.packages.install", Post, "/api/v1/projects/{project_id}/plugin-packages", Project(Find::Project("project_id")), Write).experimental(),
    console("legacy_plugins.packages.install/library", Post, "/api/v1/projects/{project_id}/plugin-packages/library/{package_id}", Project(Find::Project("project_id")), Write).experimental(),
    console("legacy_plugins.packages.install/library_request", Post, "/api/v1/projects/{project_id}/plugin-packages/library-requests/{package_id}", Project(Find::Project("project_id")), Write).experimental(),
    console("legacy_plugins.packages.install/mine", Post, "/api/v1/me/form-plugin-packages", Own, Write).experimental(),
    console("legacy_plugins.packages.read/library_requests", Get, "/api/v1/projects/{project_id}/plugin-packages/library-requests", Project(Find::Project("project_id")), Read).experimental(),
    console("legacy_plugins.packages.read/list", Get, "/api/v1/projects/{project_id}/plugin-packages", Project(Find::Project("project_id")), Read).experimental(),
    console("legacy_plugins.packages.read/mine", Get, "/api/v1/me/form-plugin-packages", Own, Read).experimental(),
    console("legacy_plugins.packages.read/mine_panel", Get, "/api/v1/me/form-plugin-packages/{package_id}/panel", Own, Read).experimental(),
    console("legacy_plugins.packages.read/mine_preview", Post, "/api/v1/me/form-plugin-packages/{package_id}/preview", Own, Read).experimental(),
    console("legacy_plugins.packages.update/binding", Post, "/api/v1/projects/{project_id}/plugin-packages/{key}/binding", Project(Find::Project("project_id")), Write).experimental(),
    console("legacy_plugins.packages.update/configuration_reset", Post, "/api/v1/projects/{project_id}/plugin-packages/{key}/configuration/reset", Project(Find::Project("project_id")), Write).experimental(),
    bot("legacy_plugins.plugins.install", Post, "/api/v1/projects/{project_id}/plugins", Project(Find::Project("project_id")), Write).experimental(),
    bot("legacy_plugins.plugins.invoke", Post, "/api/v1/plugins/{plugin_id}/invoke", Project(Find::Table("plugins","plugin_id")), Write).experimental(),
    bot("legacy_plugins.plugins.read", Get, "/api/v1/plugins/{plugin_id}", Project(Find::Table("plugins","plugin_id")), Read).experimental(),
    bot("legacy_plugins.plugins.read/list", Get, "/api/v1/projects/{project_id}/plugins", Project(Find::Project("project_id")), Read).experimental(),
    bot("legacy_plugins.plugins.update", Patch, "/api/v1/plugins/{plugin_id}", Project(Find::Table("plugins","plugin_id")), Write).experimental(),
    console("legacy_plugins.preferences.update", Put, "/api/v1/projects/{project_id}/plugin-preferences/{key}/{contribution_id}", Project(Find::Project("project_id")), Write).experimental(),
    console("legacy_plugins.preferences.update/reset", Delete, "/api/v1/projects/{project_id}/plugin-preferences/{key}/{contribution_id}", Project(Find::Project("project_id")), Write).experimental(),
    console("legacy_plugins.preferences.update/reset_all", Delete, "/api/v1/projects/{project_id}/plugin-preferences", Project(Find::Project("project_id")), Write).experimental(),
    bot("projects.bot_connections.create", Post, "/api/v1/projects/{project_id}/bot-connections", WorkspaceAdmin, Admin),
    bot("projects.bot_connections.delete", Delete, "/api/v1/projects/{project_id}/bot-connections/{bot_id}", WorkspaceAdmin, Admin),
    bot("projects.bot_connections.read", Get, "/api/v1/projects/{project_id}/bot-connections/{bot_id}", WorkspaceAdmin, Admin),
    bot("projects.bot_connections.read/list", Get, "/api/v1/projects/{project_id}/bot-connections", WorkspaceAdmin, Admin),
    bot("projects.bot_connections.rotate", Post, "/api/v1/projects/{project_id}/bot-connections/{bot_id}/rotate", WorkspaceAdmin, Secret),
    bot("projects.bot_connections.update", Patch, "/api/v1/projects/{project_id}/bot-connections/{bot_id}", WorkspaceAdmin, Admin),
    bot("projects.bot_connections.update/disable", Post, "/api/v1/projects/{project_id}/bot-connections/{bot_id}/disable", WorkspaceAdmin, Admin),
    bot("projects.bot_connections.update/enable", Post, "/api/v1/projects/{project_id}/bot-connections/{bot_id}/enable", WorkspaceAdmin, Admin),
    bot("projects.check_results.create", Post, "/api/v1/projects/{project_id}/check-results", Project(Find::Project("project_id")), Write),
    bot("projects.check_results.create/proposal", Post, "/api/v1/check-results/{check_result_id}/proposal", Project(Find::Table("check_results","check_result_id")), Write),
    bot("projects.check_results.read", Get, "/api/v1/check-results/{check_result_id}", Project(Find::Table("check_results","check_result_id")), Read),
    bot("projects.check_results.read/list", Get, "/api/v1/projects/{project_id}/check-results", Project(Find::Project("project_id")), Read),
    bot("projects.domains.create", Post, "/api/v1/projects/{project_id}/domains", Project(Find::Project("project_id")), Write),
    bot("projects.domains.delete", Delete, "/api/v1/projects/{project_id}/domains/{id}", Project(Find::Project("project_id")), Destructive),
    bot("projects.domains.read/list", Get, "/api/v1/projects/{project_id}/domains", Project(Find::Project("project_id")), Read),
    bot("projects.domains.read/members", Get, "/api/v1/projects/{project_id}/domains/{id}/members", Project(Find::Project("project_id")), Read),
    bot("projects.domains.update", Patch, "/api/v1/projects/{project_id}/domains/{id}", Project(Find::Project("project_id")), Write),
    bot("projects.project_types.create", Post, "/api/v1/workspaces/{workspace_id}/project-types", WorkspaceAdmin, Admin),
    bot("projects.project_types.read", Get, "/api/v1/project-types/{key}", Global, Admin),
    bot("projects.project_types.read/list", Get, "/api/v1/project-types", Global, Admin),
    bot("projects.project_types.read/workspace_list", Get, "/api/v1/workspaces/{workspace_id}/project-types", WorkspaceAdmin, Admin),
    bot("projects.project_types.update", Patch, "/api/v1/project-types/{key}", WorkspaceAdmin, Admin),
    bot("projects.projects.create", Post, "/api/v1/workspaces/{workspace_id}/projects", WorkspaceAdmin, Admin),
    bot("projects.projects.delete", Delete, "/api/v1/projects/{id}", WorkspaceAdmin, Admin),
    bot("projects.projects.read", Get, "/api/v1/projects/{id}", Project(Find::Project("id")), Read).oauth().plugin(),
    bot("projects.projects.read/agent_policy", Get, "/api/v1/projects/{project_id}/agent-policy", Project(Find::Project("project_id")), Read).oauth(),
    bot("projects.projects.read/context", Get, "/api/v1/projects/{project_id}/context", Project(Find::Project("project_id")), Read).oauth(),
    bot("projects.projects.read/governance_context", Get, "/api/v1/projects/{project_id}/governance-context", Project(Find::Project("project_id")), Read),
    bot("projects.projects.read/release_readiness", Get, "/api/v1/projects/{project_id}/release-readiness", Project(Find::Project("project_id")), Read),
    bot("projects.projects.read/workspace_list", Get, "/api/v1/workspaces/{workspace_id}/projects", Filtered, Read).oauth(),
    bot("projects.projects.update", Put, "/api/v1/projects/{id}", Project(Find::Project("id")), Write),
    bot("projects.resources.create", Post, "/api/v1/projects/{project_id}/resources", Project(Find::Project("project_id")), Write),
    bot("projects.resources.delete", Delete, "/api/v1/projects/{project_id}/resources/{resource_id}", Project(Find::Project("project_id")), Destructive),
    bot("projects.resources.read/list", Get, "/api/v1/projects/{project_id}/resources", Project(Find::Project("project_id")), Read),
    bot("projects.resources.update", Patch, "/api/v1/projects/{project_id}/resources/{resource_id}", Project(Find::Project("project_id")), Write),
    bot("projects.scenario_templates.install", Post, "/api/v1/projects/{project_id}/scenario-templates/{template_key}/install", Project(Find::Project("project_id")), Write),
    bot("projects.scenario_templates.read", Get, "/api/v1/scenario-templates/{key}", Global, Read),
    bot("projects.scenario_templates.read/list", Get, "/api/v1/scenario-templates", Global, Read),
    bot("tasks.change_hints.read", Get, "/api/v1/bot/change-hints", Own, Read),
    bot("tasks.protocol.read", Get, "/api/v1/ai/protocol", Global, Read),
    bot("tasks.runner.report", Put, "/api/v1/ai/runner/report", Global, Write),
    bot("tasks.tasks.create", Post, "/api/v1/projects/{project_id}/ai/tasks", Project(Find::Project("project_id")), Write),
    bot("tasks.tasks.read", Get, "/api/v1/ai/tasks/{task_id}", Global, Read),
    bot("tasks.tasks.read/active", Get, "/api/v1/ai/tasks/active", Global, Read),
    bot("tasks.tasks.read/issue", Get, "/api/v1/issues/{issue_id}/ai-tasks", Project(Find::Issue("issue_id")), Read),
    bot("tasks.tasks.read/list", Get, "/api/v1/projects/{project_id}/ai/tasks", Project(Find::Project("project_id")), Read).oauth(),
    bot("tasks.tasks.run/ack", Post, "/api/v1/ai/tasks/{task_id}/ack", Global, Write),
    bot("tasks.tasks.run/complete", Post, "/api/v1/ai/tasks/{task_id}/complete", Global, Write),
    bot("tasks.tasks.run/fail", Post, "/api/v1/ai/tasks/{task_id}/fail", Global, Write),
    bot("tasks.tasks.run/progress", Post, "/api/v1/ai/tasks/{task_id}/progress", Global, Write),
];

#[cfg(test)]
mod tests {
    use super::ENTRIES;
    use crate::catalogue::bot_class_counts;
    use std::collections::BTreeMap;

    /// The count per class is part of the review of a change to this area's table.
    #[test]
    fn the_classes_of_the_area() {
        assert_eq!(
            bot_class_counts(ENTRIES),
            BTreeMap::from([
                ("admin", 13),
                ("no project", 12),
                ("own resource", 1),
                ("filtered", 1),
                ("project-resolved", 29),
                ("refused", 6)
            ]),
            "the classes of the projects routes changed"
        );
    }
}
