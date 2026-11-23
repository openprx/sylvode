//! Runner bot route classes of the `governance` router area (`router/governance.rs`).

use super::{RunnerRoute, r};
use crate::middleware::runner_bot_scope::{Class, Find};

#[rustfmt::skip]
pub static ROUTES: &[RunnerRoute] = &[
    r("GET", "/api/v1/governance/config", Class::NoProject),
    r("POST", "/api/v1/proposals", Class::Own),
    r("GET", "/api/v1/trust-scores/appeals", Class::Admin),
    r("POST", "/api/v1/trust-scores/appeals", Class::Admin),
    r("DELETE", "/api/v1/trust-scores/appeals/{id}", Class::Admin),
    r("GET", "/api/v1/trust-scores/appeals/{id}", Class::Admin),
    r("PATCH", "/api/v1/trust-scores/appeals/{id}", Class::Admin),
    r("DELETE", "/api/v1/vetoers", Class::Admin),
    r("GET", "/api/v1/vetoers", Class::Admin),
    r("POST", "/api/v1/vetoers", Class::Admin),
    r("GET", "/api/v1/decisions", Class::Refused),
    r("GET", "/api/v1/decisions/analytics", Class::Refused),
    r("GET", "/api/v1/impact-reviews", Class::Refused),
    r("GET", "/api/v1/proposal-templates", Class::Refused),
    r("POST", "/api/v1/proposal-templates", Class::Refused),
    r("GET", "/api/v1/proposals", Class::Refused),
    r("GET", "/api/v1/ai-learning/{review_id}/feedback", Class::Project(Find::Table("impact_reviews","review_id"))),
    r("GET", "/api/v1/decisions/{id}", Class::Project(Find::Decision("id"))),
    r("GET", "/api/v1/impact-reviews/{id}/participants", Class::Project(Find::Table("impact_reviews","id"))),
    r("POST", "/api/v1/impact-reviews/{id}/participants", Class::Project(Find::Table("impact_reviews","id"))),
    r("DELETE", "/api/v1/impact-reviews/{id}/participants/{user_id}", Class::Project(Find::Table("impact_reviews","id"))),
    r("PATCH", "/api/v1/impact-reviews/{id}/participants/{user_id}", Class::Project(Find::Table("impact_reviews","id"))),
    r("GET", "/api/v1/projects/{pid}/audit-reports", Class::Project(Find::Project("pid"))),
    r("POST", "/api/v1/projects/{pid}/audit-reports", Class::Project(Find::Project("pid"))),
    r("GET", "/api/v1/projects/{pid}/audit-reports/{id}", Class::Project(Find::Project("pid"))),
    r("GET", "/api/v1/projects/{project_id}/ai-participants/{bot_id}/alignment-stats", Class::Project(Find::Project("project_id"))),
    r("GET", "/api/v1/projects/{project_id}/ai-participants/{bot_id}/learning", Class::Project(Find::Project("project_id"))),
    r("DELETE", "/api/v1/proposal-comments/{id}", Class::Project(Find::ProposalComment("id"))),
    r("DELETE", "/api/v1/proposal-templates/{id}", Class::Project(Find::Table("proposal_templates","id"))),
    r("GET", "/api/v1/proposal-templates/{id}", Class::Project(Find::Table("proposal_templates","id"))),
    r("PUT", "/api/v1/proposal-templates/{id}", Class::Project(Find::Table("proposal_templates","id"))),
    r("DELETE", "/api/v1/proposals/{id}", Class::Project(Find::Proposal("id"))),
    r("GET", "/api/v1/proposals/{id}", Class::Project(Find::Proposal("id"))),
    r("PATCH", "/api/v1/proposals/{id}", Class::Project(Find::Proposal("id"))),
    r("POST", "/api/v1/proposals/{id}/archive", Class::Project(Find::Proposal("id"))),
    r("GET", "/api/v1/proposals/{id}/chain", Class::Project(Find::Proposal("id"))),
    r("GET", "/api/v1/proposals/{id}/comments", Class::Project(Find::Proposal("id"))),
    r("POST", "/api/v1/proposals/{id}/comments", Class::Project(Find::Proposal("id"))),
    r("DELETE", "/api/v1/proposals/{id}/comments/{comment_id}", Class::Project(Find::Proposal("id"))),
    r("GET", "/api/v1/proposals/{id}/decision", Class::Project(Find::Proposal("id"))),
    r("DELETE", "/api/v1/proposals/{id}/impact-review", Class::Project(Find::Proposal("id"))),
    r("GET", "/api/v1/proposals/{id}/impact-review", Class::Project(Find::Proposal("id"))),
    r("PATCH", "/api/v1/proposals/{id}/impact-review", Class::Project(Find::Proposal("id"))),
    r("POST", "/api/v1/proposals/{id}/impact-review", Class::Project(Find::Proposal("id"))),
    r("GET", "/api/v1/proposals/{id}/issues", Class::Project(Find::Proposal("id"))),
    r("POST", "/api/v1/proposals/{id}/issues", Class::Project(Find::Proposal("id"))),
    r("POST", "/api/v1/proposals/{id}/start-voting", Class::Project(Find::Proposal("id"))),
    r("POST", "/api/v1/proposals/{id}/submit", Class::Project(Find::Proposal("id"))),
    r("GET", "/api/v1/proposals/{id}/timeline", Class::Project(Find::Proposal("id"))),
    r("DELETE", "/api/v1/proposals/{id}/veto", Class::Project(Find::Proposal("id"))),
    r("GET", "/api/v1/proposals/{id}/veto", Class::Project(Find::Proposal("id"))),
    r("POST", "/api/v1/proposals/{id}/veto", Class::Project(Find::Proposal("id"))),
    r("POST", "/api/v1/proposals/{id}/veto/escalation", Class::Project(Find::Proposal("id"))),
    r("POST", "/api/v1/proposals/{id}/veto/escalation/vote", Class::Project(Find::Proposal("id"))),
    r("GET", "/api/v1/proposals/{id}/votes", Class::Project(Find::Proposal("id"))),
    r("POST", "/api/v1/proposals/{id}/votes", Class::Project(Find::Proposal("id"))),
    r("DELETE", "/api/v1/proposals/{id}/votes/mine", Class::Project(Find::Proposal("id"))),
    r("DELETE", "/api/v1/proposals/{proposal_id}/issues/{issue_id}", Class::Project(Find::Proposal("proposal_id"))),
];

#[cfg(test)]
mod tests {
    use super::ROUTES;
    use crate::middleware::bot_routes::class_counts;
    use std::collections::BTreeMap;

    /// The count per class is part of the review of a change to this area's table.
    #[test]
    fn the_classes_of_the_area() {
        assert_eq!(
            class_counts(ROUTES),
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
