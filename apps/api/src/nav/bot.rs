//! What a bot may not change in a project navigation (owner rule 2026-11-25, with the lead's
//! limits NAV-D1): external links and lock flags stay with people who administer the workspace.
//! Checked by diffing the new document against the stored one (or the defaults).

use std::collections::HashMap;

use super::doc::{EntryDoc, NavDoc, Target};
use crate::error::ApiError;

fn external(entry: &EntryDoc) -> Option<(&str, Option<&super::doc::Labels>)> {
    match &entry.target {
        Target::External { url } => Some((url.as_str(), entry.label.as_ref())),
        _ => None,
    }
}

/// # Errors
/// `external_requires_admin` for an added or changed external link, `lock_requires_admin` for any
/// change of a lock flag (adding a locked entry and removing a locked entry count as one).
pub fn check(stored: &NavDoc, new: &NavDoc) -> Result<(), ApiError> {
    let old: HashMap<&str, &EntryDoc> = stored.entries().map(|(_, entry)| (entry.id.as_str(), entry)).collect();
    for (_, entry) in new.entries() {
        let before = old.get(entry.id.as_str());
        if let Some(link) = external(entry)
            && before.and_then(|b| external(b)) != Some(link)
        {
            return Err(ApiError::Coded {
                status: 403,
                code: "external_requires_admin",
                message: "a bot cannot add or change an external link".to_string(),
                details: None,
            });
        }
        if before.is_some_and(|b| b.locked) != entry.locked {
            return Err(lock_refusal());
        }
    }
    let kept: std::collections::HashSet<&str> = new.entries().map(|(_, entry)| entry.id.as_str()).collect();
    if stored
        .entries()
        .any(|(_, entry)| entry.locked && !kept.contains(entry.id.as_str()))
    {
        return Err(lock_refusal());
    }
    Ok(())
}

fn lock_refusal() -> ApiError {
    ApiError::Coded {
        status: 403,
        code: "lock_requires_admin",
        message: "a bot cannot change which entries are locked".to_string(),
        details: None,
    }
}

#[cfg(test)]
#[allow(clippy::expect_used)]
mod tests {
    use super::*;
    use crate::nav::doc::{NavScope, parse};
    use serde_json::json;

    fn doc(entries: serde_json::Value) -> NavDoc {
        parse(
            &json!({"schema": "sv.nav.v1", "scope": "project", "sections": [{"id": "main", "entries": entries}]}),
            NavScope::Project,
        )
        .expect("valid")
    }

    fn code(result: Result<(), ApiError>) -> Option<&'static str> {
        match result {
            Err(ApiError::Coded { code, .. }) => Some(code),
            _ => None,
        }
    }

    fn link(id: &str, url: &str, locked: bool) -> serde_json::Value {
        json!({"id": id, "target": {"kind": "external", "url": url}, "label": {"en": "Handbook"}, "locked": locked})
    }

    fn page(id: &str, p: &str, locked: bool) -> serde_json::Value {
        json!({"id": id, "target": {"kind": "builtin", "page": p}, "locked": locked})
    }

    #[test]
    fn reordering_and_removing_are_allowed() {
        let stored = doc(json!([
            page("a", "project.board", false),
            link("h", "https://x.test/", false)
        ]));
        let new = doc(json!([
            link("h", "https://x.test/", false),
            page("a", "project.board", false)
        ]));
        assert_eq!(code(check(&stored, &new)), None);
        assert_eq!(
            code(check(&stored, &doc(json!([page("a", "project.board", false)])))),
            None
        );
    }

    #[test]
    fn an_added_or_changed_external_link_needs_an_administrator() {
        let stored = doc(json!([link("h", "https://x.test/", false)]));
        let added = doc(json!([
            link("h", "https://x.test/", false),
            link("n", "https://y.test/", false)
        ]));
        assert_eq!(code(check(&stored, &added)), Some("external_requires_admin"));
        let changed = doc(json!([link("h", "https://evil.test/", false)]));
        assert_eq!(code(check(&stored, &changed)), Some("external_requires_admin"));
        let converted = doc(json!([page("h", "project.board", false)]));
        assert_eq!(code(check(&stored, &converted)), None);
        let swapped = doc(json!([link("a", "https://x.test/", false)]));
        let stored_page = doc(json!([page("a", "project.board", false)]));
        assert_eq!(code(check(&stored_page, &swapped)), Some("external_requires_admin"));
    }

    #[test]
    fn a_lock_flag_never_changes_under_a_bot() {
        let stored = doc(json!([
            page("a", "project.board", true),
            page("b", "project.cycles", false)
        ]));
        let flipped = doc(json!([
            page("a", "project.board", false),
            page("b", "project.cycles", false)
        ]));
        assert_eq!(code(check(&stored, &flipped)), Some("lock_requires_admin"));
        let locked_new = doc(json!([
            page("a", "project.board", true),
            page("b", "project.cycles", true)
        ]));
        assert_eq!(code(check(&stored, &locked_new)), Some("lock_requires_admin"));
        let dropped = doc(json!([page("b", "project.cycles", false)]));
        assert_eq!(code(check(&stored, &dropped)), Some("lock_requires_admin"));
        let same = doc(json!([
            page("b", "project.cycles", false),
            page("a", "project.board", true)
        ]));
        assert_eq!(code(check(&stored, &same)), None);
    }
}
