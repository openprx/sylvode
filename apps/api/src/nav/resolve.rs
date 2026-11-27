//! Resolution: the stored document (or the defaults), the viewer's rights and the person's
//! preferences become the navigation one viewer sees. Pure over loaded data; both menu routes
//! use [`resolve`].

use std::collections::{BTreeSet, HashSet};

use serde::Serialize;
use uuid::Uuid;

use super::{
    doc::{EntryDoc, Labels, NavDoc, NavScope, Section, Target, builtin_applies},
    prefs::Prefs,
    refs::{Place, Refs, Where, locate},
};

pub const RESOLVED_SCHEMA: &str = "sv.nav.resolved.v1";
/// The section the pinned entries are moved into.
pub const PINNED_SECTION: &str = "_pinned";
/// Pins shown at most, the administrator's and the person's together.
pub const MAX_PINNED: usize = 8;

/// Who looks at the navigation.
#[derive(Debug, Clone, Copy)]
pub struct Viewer {
    pub is_bot: bool,
    /// Workspace owner or admin (a person; a bot is never an administrator here).
    pub is_admin: bool,
}

/// What the loaded data says about one viewer.
pub struct Inputs<'a> {
    pub at: Where,
    pub project_type: Option<&'a str>,
    pub viewer: Viewer,
    pub refs: &'a Refs,
    /// Object ids the viewer may read (only consulted for the kinds that need a permission check).
    pub visible: &'a HashSet<Uuid>,
    pub prefs: Option<&'a Prefs>,
    pub include_user_hidden: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ResolvedEntry {
    pub id: String,
    /// The section the entry is stored in, also when it is shown in `_pinned`.
    pub section_id: String,
    pub kind: &'static str,
    pub target: Target,
    pub label: Option<Labels>,
    pub name: Option<String>,
    pub icon: Option<String>,
    pub href: Option<String>,
    pub pinned: bool,
    pub locked: bool,
    pub user_hidden: bool,
    /// Index in its stored section after the person's order is applied (also when pinned).
    pub position: usize,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ResolvedSection {
    pub id: String,
    pub label: Option<Labels>,
    pub collapsed: bool,
    pub entries: Vec<ResolvedEntry>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Resolved {
    pub schema: &'static str,
    pub scope: &'static str,
    pub scope_id: Uuid,
    pub config_version: i32,
    pub customised: bool,
    pub sections: Vec<ResolvedSection>,
}

struct Item {
    entry: ResolvedEntry,
}

fn needs_permission_check(target: &Target) -> bool {
    target.object_id().is_some()
}

/// Steps 2 to 4 for one entry: `None` when it is not shown to this viewer at all.
fn admit(section: &Section, entry: &EntryDoc, input: &Inputs<'_>) -> Option<ResolvedEntry> {
    if entry.hidden {
        return None;
    }
    let (name, href) = match &entry.target {
        Target::Builtin { page } => {
            if !builtin_applies(page, input.project_type) {
                return None;
            }
            // Project settings are for people who administer the workspace; a bot never sees them.
            if page == "project.settings" && (!input.viewer.is_admin || input.viewer.is_bot) {
                return None;
            }
            (None, None)
        }
        Target::PluginApp { .. } => (None, None),
        Target::External { url } => (None, Some(url.clone())),
        target => match locate(target, &input.at, input.refs)? {
            Place::Found { name, href } => {
                let object = target.object_id()?;
                if needs_permission_check(target) && !input.visible.contains(&object) {
                    return None;
                }
                (Some(name), Some(href))
            }
            Place::Missing | Place::OutOfScope => return None,
        },
    };
    Some(ResolvedEntry {
        id: entry.id.clone(),
        section_id: section.id.clone(),
        kind: entry.target.kind(),
        target: entry.target.clone(),
        label: entry.label.clone(),
        name,
        icon: entry.icon.clone(),
        href,
        pinned: entry.pinned,
        locked: entry.locked,
        user_hidden: false,
        position: 0,
    })
}

/// Reorders one section: locked entries keep their administrator index, the others follow the
/// person's order, unlisted ones after the listed ones in administrator order.
fn reorder(items: Vec<Item>, order: Option<&Vec<String>>) -> Vec<Item> {
    let Some(order) = order else {
        return items;
    };
    let total = items.len();
    let mut slots: Vec<Option<Item>> = Vec::with_capacity(total);
    let mut movable = Vec::new();
    for item in items {
        if item.entry.locked {
            slots.push(Some(item));
        } else {
            slots.push(None);
            movable.push(item);
        }
    }
    let rank = |id: &str| order.iter().position(|listed| listed == id);
    let (mut listed, unlisted): (Vec<Item>, Vec<Item>) =
        movable.into_iter().partition(|item| rank(&item.entry.id).is_some());
    listed.sort_by_key(|item| rank(&item.entry.id));
    let mut movable = listed.into_iter().chain(unlisted);
    for slot in &mut slots {
        if slot.is_none() {
            *slot = movable.next();
        }
    }
    slots.into_iter().flatten().collect()
}

/// Resolves the navigation of one scope for one viewer.
#[must_use]
pub fn resolve(doc: &NavDoc, config_version: i32, customised: bool, input: &Inputs<'_>) -> Resolved {
    let prefs = if input.viewer.is_bot { None } else { input.prefs };
    let hidden: BTreeSet<&str> = prefs
        .map(|p| p.hidden.iter().map(String::as_str).collect())
        .unwrap_or_default();

    let mut sections: Vec<(ResolvedSection, Vec<Item>)> = Vec::new();
    for section in &doc.sections {
        let mut shown = Vec::new();
        for entry in &section.entries {
            let Some(mut resolved) = admit(section, entry, input) else {
                continue;
            };
            if !resolved.locked && hidden.contains(resolved.id.as_str()) {
                if !input.include_user_hidden {
                    continue;
                }
                resolved.user_hidden = true;
            }
            shown.push(Item { entry: resolved });
        }
        let collapsed = prefs.map_or(section.collapsed, |p| {
            p.collapsed.contains(&section.id) || (section.collapsed && !p.expanded.contains(&section.id))
        });
        sections.push((
            ResolvedSection {
                id: section.id.clone(),
                label: section.label.clone(),
                collapsed,
                entries: Vec::new(),
            },
            reorder(shown, prefs.and_then(|p| p.order.get(&section.id)))
                .into_iter()
                .enumerate()
                .map(|(position, mut item)| {
                    item.entry.position = position;
                    item
                })
                .collect(),
        ));
    }

    // Pins: the administrator's first (minus what the person unpinned, locked ones stay), then the
    // person's own in the order they pinned them.
    let unpinned: BTreeSet<&str> = prefs
        .map(|p| p.unpinned.iter().map(String::as_str).collect())
        .unwrap_or_default();
    let mut pin_order: Vec<String> = Vec::new();
    for (_, items) in &sections {
        for item in items {
            let entry = &item.entry;
            if entry.pinned && (entry.locked || !unpinned.contains(entry.id.as_str())) && !entry.user_hidden {
                pin_order.push(entry.id.clone());
            }
        }
    }
    if let Some(prefs) = prefs {
        for id in &prefs.pinned {
            let known = sections
                .iter()
                .flat_map(|(_, items)| items)
                .any(|item| item.entry.id == *id && !item.entry.user_hidden);
            if known && !pin_order.contains(id) {
                pin_order.push(id.clone());
            }
        }
    }
    pin_order.truncate(MAX_PINNED);

    let mut pinned_entries: Vec<ResolvedEntry> = Vec::new();
    let mut out: Vec<ResolvedSection> = Vec::new();
    for (mut head, items) in sections {
        for item in items {
            let mut entry = item.entry;
            if pin_order.contains(&entry.id) {
                entry.pinned = true;
                pinned_entries.push(entry);
            } else {
                entry.pinned = false;
                head.entries.push(entry);
            }
        }
        out.push(head);
    }
    pinned_entries.sort_by_key(|entry| pin_order.iter().position(|id| *id == entry.id));
    if !pinned_entries.is_empty() {
        out.insert(
            0,
            ResolvedSection {
                id: PINNED_SECTION.to_string(),
                label: None,
                collapsed: false,
                entries: pinned_entries,
            },
        );
    }
    out.retain(|section| !section.entries.is_empty());

    Resolved {
        schema: RESOLVED_SCHEMA,
        scope: input.at.scope.as_str(),
        scope_id: match input.at.scope {
            NavScope::Workspace => input.at.workspace_id,
            NavScope::Project => input.at.project_id.unwrap_or(input.at.workspace_id),
        },
        config_version,
        customised,
        sections: out,
    }
}

/// Entry ids of a document whose target no longer exists or left the scope.
#[must_use]
pub fn broken(doc: &NavDoc, at: &Where, refs: &Refs) -> Vec<String> {
    doc.entries()
        .filter(|(_, entry)| {
            matches!(
                locate(&entry.target, at, refs),
                Some(Place::Missing | Place::OutOfScope)
            )
        })
        .map(|(_, entry)| entry.id.clone())
        .collect()
}

#[cfg(test)]
#[allow(clippy::expect_used)]
mod tests {
    use super::*;
    use crate::nav::doc::{self, parse};
    use serde_json::json;

    fn project_at() -> Where {
        Where {
            scope: NavScope::Project,
            workspace_id: Uuid::from_u128(1),
            project_id: Some(Uuid::from_u128(2)),
        }
    }

    fn entry(id: &str, page: &str, extra: serde_json::Value) -> serde_json::Value {
        let mut e = json!({"id": id, "target": {"kind": "builtin", "page": page}});
        e.as_object_mut()
            .expect("object")
            .extend(extra.as_object().expect("object").clone());
        e
    }

    fn doc_of(entries: Vec<serde_json::Value>) -> NavDoc {
        parse(
            &json!({"schema": "sv.nav.v1", "scope": "project", "sections": [{"id": "main", "entries": entries}]}),
            NavScope::Project,
        )
        .expect("valid document")
    }

    fn run(
        doc: &NavDoc,
        viewer: Viewer,
        prefs: Option<&Prefs>,
        project_type: Option<&str>,
    ) -> Vec<(String, Vec<String>)> {
        let refs = Refs::default();
        let visible = HashSet::new();
        let resolved = resolve(
            doc,
            1,
            true,
            &Inputs {
                at: project_at(),
                project_type,
                viewer,
                refs: &refs,
                visible: &visible,
                prefs,
                include_user_hidden: false,
            },
        );
        resolved
            .sections
            .into_iter()
            .map(|s| (s.id, s.entries.into_iter().map(|e| e.id).collect()))
            .collect()
    }

    const MEMBER: Viewer = Viewer {
        is_bot: false,
        is_admin: false,
    };
    const ADMIN: Viewer = Viewer {
        is_bot: false,
        is_admin: true,
    };

    fn prefs(value: serde_json::Value) -> Prefs {
        crate::nav::prefs::parse(&value).expect("valid prefs")
    }

    #[test]
    fn the_defaults_resolve_to_the_tree_of_each_project_type() {
        let code = run(&doc::defaults(NavScope::Project, None), ADMIN, None, None);
        assert_eq!(
            code,
            [(
                "main".to_string(),
                ["overview", "issues", "board", "cycles", "forms", "apps", "settings"]
                    .map(String::from)
                    .to_vec()
            )]
        );
        let forms = run(
            &doc::defaults(NavScope::Project, Some("custom_form")),
            MEMBER,
            None,
            Some("custom_form"),
        );
        assert_eq!(
            forms,
            [("main".to_string(), ["forms", "apps"].map(String::from).to_vec())]
        );
    }

    #[test]
    fn settings_are_for_workspace_admins_only_and_never_for_bots() {
        let d = doc::defaults(NavScope::Project, None);
        let ids = |v| run(&d, v, None, None)[0].1.clone();
        assert!(ids(ADMIN).contains(&"settings".to_string()));
        assert!(!ids(MEMBER).contains(&"settings".to_string()));
        assert!(
            !ids(Viewer {
                is_bot: true,
                is_admin: true
            })
            .contains(&"settings".to_string())
        );
    }

    #[test]
    fn hidden_entries_and_work_item_builtins_of_a_custom_form_project_are_dropped() {
        let d = doc_of(vec![
            entry("issues", "project.issues", json!({})),
            entry("board", "project.board", json!({"hidden": true})),
            entry("forms", "project.forms", json!({})),
        ]);
        assert_eq!(run(&d, MEMBER, None, None)[0].1, ["issues", "forms"]);
        assert_eq!(run(&d, MEMBER, None, Some("custom_form"))[0].1, ["forms"]);
    }

    #[test]
    fn a_person_hides_reorders_and_pins_but_a_locked_entry_stays_put() {
        let d = doc_of(vec![
            entry("overview", "project.overview", json!({"locked": true})),
            entry("issues", "project.issues", json!({})),
            entry("board", "project.board", json!({})),
            entry("cycles", "project.cycles", json!({})),
        ]);
        let p = prefs(json!({
            "schema": "sv.nav.prefs.v1",
            "hidden": ["overview", "issues"],
            "order": {"main": ["cycles", "overview", "board"]},
            "pinned": ["board", "gone"]
        }));
        let out = run(&d, MEMBER, Some(&p), None);
        assert_eq!(
            out,
            [
                ("_pinned".to_string(), vec!["board".to_string()]),
                ("main".to_string(), vec!["overview".to_string(), "cycles".to_string()])
            ]
        );
    }

    #[test]
    fn admin_pins_are_unpinned_by_a_person_unless_locked_and_the_pinned_section_is_capped() {
        let d = doc_of(vec![
            entry("issues", "project.issues", json!({"pinned": true})),
            entry("board", "project.board", json!({"pinned": true, "locked": true})),
            entry("cycles", "project.cycles", json!({})),
        ]);
        let p = prefs(json!({"schema": "sv.nav.prefs.v1", "unpinned": ["issues", "board"], "pinned": ["cycles"]}));
        let out = run(&d, MEMBER, Some(&p), None);
        assert_eq!(
            out[0],
            ("_pinned".to_string(), vec!["board".to_string(), "cycles".to_string()])
        );
        assert_eq!(out[1], ("main".to_string(), vec!["issues".to_string()]));

        let many: Vec<_> = [
            "project.overview",
            "project.issues",
            "project.board",
            "project.cycles",
            "project.forms",
            "project.apps",
            "project.settings",
        ]
        .iter()
        .enumerate()
        .map(|(i, page)| entry(&format!("e{i}"), page, json!({"pinned": true})))
        .collect();
        let d = doc_of(many);
        assert_eq!(run(&d, ADMIN, None, None)[0].1.len(), 7);
    }

    #[test]
    fn a_bot_gets_the_document_without_preferences() {
        let d = doc_of(vec![
            entry("issues", "project.issues", json!({})),
            entry("board", "project.board", json!({})),
        ]);
        let p = prefs(json!({"schema": "sv.nav.prefs.v1", "hidden": ["issues"]}));
        let out = run(
            &d,
            Viewer {
                is_bot: true,
                is_admin: false,
            },
            Some(&p),
            None,
        );
        assert_eq!(out[0].1, ["issues", "board"]);
    }

    #[test]
    fn user_hidden_entries_show_only_when_asked_and_collapse_is_merged() {
        let d = doc_of(vec![entry("issues", "project.issues", json!({}))]);
        let p = prefs(json!({"schema": "sv.nav.prefs.v1", "hidden": ["issues"], "collapsed": ["main"]}));
        let refs = Refs::default();
        let visible = HashSet::new();
        let resolved = resolve(
            &d,
            1,
            true,
            &Inputs {
                at: project_at(),
                project_type: None,
                viewer: MEMBER,
                refs: &refs,
                visible: &visible,
                prefs: Some(&p),
                include_user_hidden: true,
            },
        );
        assert!(resolved.sections[0].collapsed);
        assert!(resolved.sections[0].entries[0].user_hidden);
    }

    #[test]
    fn a_person_keeps_a_default_collapsed_section_open() {
        let d = parse(
            &json!({"schema": "sv.nav.v1", "scope": "project", "sections": [
                {"id": "main", "collapsed": true, "entries": [entry("issues", "project.issues", json!({}))]}]}),
            NavScope::Project,
        )
        .expect("valid");
        let collapsed = |p: Option<&Prefs>| {
            let refs = Refs::default();
            let visible = HashSet::new();
            resolve(
                &d,
                1,
                true,
                &Inputs {
                    at: project_at(),
                    project_type: None,
                    viewer: MEMBER,
                    refs: &refs,
                    visible: &visible,
                    prefs: p,
                    include_user_hidden: false,
                },
            )
            .sections[0]
                .collapsed
        };
        assert!(collapsed(None));
        assert!(!collapsed(Some(&prefs(
            json!({"schema": "sv.nav.prefs.v1", "expanded": ["main"]})
        ))));
        assert!(collapsed(Some(&prefs(
            json!({"schema": "sv.nav.prefs.v1", "expanded": ["main"], "collapsed": ["main"]})
        ))));
    }
}
