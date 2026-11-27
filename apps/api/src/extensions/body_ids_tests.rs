//! Every id a plugin-reachable request body carries must be accounted for (contract C1 §2.3).
//!
//! A plugin call is confined to its primary target by the catalogue checker. An id in the body that
//! names another object (a second project, form or record) would be a way around the grant
//! selectors, so each id-like field of the request types of the `PLUGIN` operations is listed here
//! with the reason it needs no `extensions::ensure_plugin_secondary` check. A new id field fails the
//! test until it is classified on purpose. Today none names an object outside the primary target:
//! the handlers confine a sprint to the work item's own project, a milestone to the workspace, and
//! relation values of a record are stored as sent and read back through the read routes.
#![allow(clippy::expect_used, clippy::indexing_slicing)]

/// `(source file, request type, id-like fields, why they are safe)`.
const ACCOUNTED: &[(&str, &str, &[&str], &str)] = &[
    (
        include_str!("../routes/issue.rs"),
        "CreateIssueRequest",
        &["assignee_id", "sprint_id", "milestone_id"],
        "assignee is a person of the workspace; the sprint must belong to the target project \
         (ensure_sprint_in_project); a milestone belongs to the workspace, not to a project",
    ),
    (
        include_str!("../routes/issue.rs"),
        "UpdateIssueRequest",
        &["assignee_id", "sprint_id", "milestone_id"],
        "as for create; the sprint is checked against the work item's own project",
    ),
    (
        include_str!("../routes/comment.rs"),
        "CreateCommentRequest",
        &["mentions"],
        "people of the workspace; a mention only notifies",
    ),
    (
        include_str!("../routes/form.rs"),
        "CreateRecordRequest",
        &[],
        "values are opaque to the route; a relation value is stored as sent",
    ),
    (
        include_str!("../routes/form.rs"),
        "UpdateRecordRequest",
        &[],
        "as for create",
    ),
];

/// The id-like field names of `pub struct <name> { ... }` in `source`.
fn id_fields(source: &str, name: &str) -> Vec<String> {
    let start = source
        .find(&format!("pub struct {name} {{"))
        .unwrap_or_else(|| panic!("{name} is not in the source"));
    let body = &source[start..];
    let end = body.find("\n}").expect("the struct ends");
    body[..end]
        .lines()
        .filter_map(|line| {
            let field = line.trim().strip_prefix("pub ")?.split(':').next()?.trim();
            (field.ends_with("_id") || field.ends_with("_ids") || field == "mentions").then(|| field.to_owned())
        })
        .collect()
}

#[test]
fn every_id_of_a_plugin_reachable_body_is_accounted_for() {
    for (source, name, expected, why) in ACCOUNTED {
        assert!(!why.is_empty());
        assert_eq!(
            id_fields(source, name),
            expected.iter().map(|field| (*field).to_owned()).collect::<Vec<_>>(),
            "{name}: classify the new id field before a plugin can send it"
        );
    }
}
