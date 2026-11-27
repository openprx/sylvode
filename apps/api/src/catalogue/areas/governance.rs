//! Catalogue entries of the `governance` router area (`router/governance.rs`).

use crate::catalogue::types::dsl::{
    Admin, Delete, Destructive, Entry, Find, Get, Global, InstanceAdmin, Own, Patch, Post, Project, Put, Read,
    Workspace, WorkspaceAdmin, Write, bot, console,
};

#[rustfmt::skip]
pub static ENTRIES: &[Entry] = &[
    bot("governance.ai_participants.read/alignment_stats", Get, "/api/v1/projects/{project_id}/ai-participants/{bot_id}/alignment-stats", Project(Find::Project("project_id")), Read),
    bot("governance.ai_participants.read/feedback", Get, "/api/v1/ai-learning/{review_id}/feedback", Project(Find::Table("impact_reviews","review_id")), Read),
    bot("governance.ai_participants.read/learning", Get, "/api/v1/projects/{project_id}/ai-participants/{bot_id}/learning", Project(Find::Project("project_id")), Read),
    bot("governance.appeals.create", Post, "/api/v1/trust-scores/appeals", WorkspaceAdmin, Admin),
    bot("governance.appeals.delete", Delete, "/api/v1/trust-scores/appeals/{id}", WorkspaceAdmin, Admin),
    bot("governance.appeals.read", Get, "/api/v1/trust-scores/appeals/{id}", WorkspaceAdmin, Admin),
    bot("governance.appeals.read/list", Get, "/api/v1/trust-scores/appeals", WorkspaceAdmin, Admin),
    bot("governance.appeals.update", Patch, "/api/v1/trust-scores/appeals/{id}", WorkspaceAdmin, Admin),
    console("governance.audit_logs.read", Get, "/api/v1/governance/audit-logs", InstanceAdmin, Admin),
    bot("governance.audit_reports.create", Post, "/api/v1/projects/{pid}/audit-reports", Project(Find::Project("pid")), Write),
    bot("governance.audit_reports.read", Get, "/api/v1/projects/{pid}/audit-reports/{id}", Project(Find::Project("pid")), Read),
    bot("governance.audit_reports.read/list", Get, "/api/v1/projects/{pid}/audit-reports", Project(Find::Project("pid")), Read),
    bot("governance.config.read", Get, "/api/v1/governance/config", Global, Read),
    console("governance.config.update", Put, "/api/v1/governance/config", InstanceAdmin, Admin),
    bot("governance.decisions.read", Get, "/api/v1/decisions/{id}", Project(Find::Decision("id")), Read),
    bot("governance.decisions.read/analytics", Get, "/api/v1/decisions/analytics", Workspace, Read),
    bot("governance.decisions.read/list", Get, "/api/v1/decisions", Workspace, Read),
    bot("governance.decisions.read/proposal", Get, "/api/v1/proposals/{id}/decision", Project(Find::Proposal("id")), Read),
    bot("governance.impact_reviews.create/proposal", Post, "/api/v1/proposals/{id}/impact-review", Project(Find::Proposal("id")), Write),
    bot("governance.impact_reviews.delete/proposal", Delete, "/api/v1/proposals/{id}/impact-review", Project(Find::Proposal("id")), Destructive),
    bot("governance.impact_reviews.read/list", Get, "/api/v1/impact-reviews", Workspace, Read),
    bot("governance.impact_reviews.read/participants", Get, "/api/v1/impact-reviews/{id}/participants", Project(Find::Table("impact_reviews","id")), Read),
    bot("governance.impact_reviews.read/proposal", Get, "/api/v1/proposals/{id}/impact-review", Project(Find::Proposal("id")), Read),
    bot("governance.impact_reviews.update/participant", Patch, "/api/v1/impact-reviews/{id}/participants/{user_id}", Project(Find::Table("impact_reviews","id")), Write),
    bot("governance.impact_reviews.update/participant_add", Post, "/api/v1/impact-reviews/{id}/participants", Project(Find::Table("impact_reviews","id")), Write),
    bot("governance.impact_reviews.update/participant_remove", Delete, "/api/v1/impact-reviews/{id}/participants/{user_id}", Project(Find::Table("impact_reviews","id")), Write),
    bot("governance.impact_reviews.update/proposal", Patch, "/api/v1/proposals/{id}/impact-review", Project(Find::Proposal("id")), Write),
    bot("governance.proposal_comments.create", Post, "/api/v1/proposals/{id}/comments", Project(Find::Proposal("id")), Write).comment(),
    bot("governance.proposal_comments.delete", Delete, "/api/v1/proposal-comments/{id}", Project(Find::ProposalComment("id")), Destructive).comment(),
    bot("governance.proposal_comments.delete/by_proposal", Delete, "/api/v1/proposals/{id}/comments/{comment_id}", Project(Find::Proposal("id")), Destructive).comment(),
    bot("governance.proposal_comments.read", Get, "/api/v1/proposals/{id}/comments", Project(Find::Proposal("id")), Read),
    bot("governance.proposal_templates.create", Post, "/api/v1/proposal-templates", Workspace, Write),
    bot("governance.proposal_templates.delete", Delete, "/api/v1/proposal-templates/{id}", Project(Find::Table("proposal_templates","id")), Destructive),
    bot("governance.proposal_templates.read", Get, "/api/v1/proposal-templates/{id}", Project(Find::Table("proposal_templates","id")), Read),
    bot("governance.proposal_templates.read/list", Get, "/api/v1/proposal-templates", Workspace, Read),
    bot("governance.proposal_templates.update", Put, "/api/v1/proposal-templates/{id}", Project(Find::Table("proposal_templates","id")), Write),
    bot("governance.proposals.archive", Post, "/api/v1/proposals/{id}/archive", Project(Find::Proposal("id")), Write),
    bot("governance.proposals.create", Post, "/api/v1/proposals", Own, Write),
    bot("governance.proposals.delete", Delete, "/api/v1/proposals/{id}", Project(Find::Proposal("id")), Destructive),
    bot("governance.proposals.read", Get, "/api/v1/proposals/{id}", Project(Find::Proposal("id")), Read),
    bot("governance.proposals.read/chain", Get, "/api/v1/proposals/{id}/chain", Project(Find::Proposal("id")), Read),
    bot("governance.proposals.read/issues", Get, "/api/v1/proposals/{id}/issues", Project(Find::Proposal("id")), Read),
    bot("governance.proposals.read/list", Get, "/api/v1/proposals", Workspace, Read),
    bot("governance.proposals.read/timeline", Get, "/api/v1/proposals/{id}/timeline", Project(Find::Proposal("id")), Read),
    bot("governance.proposals.start_voting", Post, "/api/v1/proposals/{id}/start-voting", Project(Find::Proposal("id")), Write),
    bot("governance.proposals.submit", Post, "/api/v1/proposals/{id}/submit", Project(Find::Proposal("id")), Write),
    bot("governance.proposals.update", Patch, "/api/v1/proposals/{id}", Project(Find::Proposal("id")), Write),
    bot("governance.proposals.update/issue_add", Post, "/api/v1/proposals/{id}/issues", Project(Find::Proposal("id")), Write),
    bot("governance.proposals.update/issue_remove", Delete, "/api/v1/proposals/{proposal_id}/issues/{issue_id}", Project(Find::Proposal("proposal_id")), Write),
    bot("governance.vetoers.create", Post, "/api/v1/vetoers", WorkspaceAdmin, Admin),
    bot("governance.vetoers.delete", Delete, "/api/v1/vetoers", WorkspaceAdmin, Admin),
    bot("governance.vetoers.read", Get, "/api/v1/vetoers", WorkspaceAdmin, Admin),
    bot("governance.vetoes.create", Post, "/api/v1/proposals/{id}/veto", Project(Find::Proposal("id")), Write),
    bot("governance.vetoes.delete", Delete, "/api/v1/proposals/{id}/veto", Project(Find::Proposal("id")), Destructive),
    bot("governance.vetoes.escalate", Post, "/api/v1/proposals/{id}/veto/escalation", Project(Find::Proposal("id")), Write),
    bot("governance.vetoes.escalate/vote", Post, "/api/v1/proposals/{id}/veto/escalation/vote", Project(Find::Proposal("id")), Write),
    bot("governance.vetoes.read", Get, "/api/v1/proposals/{id}/veto", Project(Find::Proposal("id")), Read),
    bot("governance.votes.create", Post, "/api/v1/proposals/{id}/votes", Project(Find::Proposal("id")), Write),
    bot("governance.votes.delete", Delete, "/api/v1/proposals/{id}/votes/mine", Project(Find::Proposal("id")), Destructive),
    bot("governance.votes.read", Get, "/api/v1/proposals/{id}/votes", Project(Find::Proposal("id")), Read),
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
                ("admin", 8),
                ("no project", 1),
                ("own resource", 1),
                ("project-resolved", 42),
                ("refused", 6)
            ]),
            "the classes of the governance routes changed"
        );
    }
}
