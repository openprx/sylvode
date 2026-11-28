//! Append-only and one-way-status rules against a base snapshot (pure functions on two parsed
//! indexes; reading the base from git is the CLI's job).

use std::collections::BTreeMap;

use crate::error::{Issue, Issues, element, member};
use crate::model::{IndexDoc, KeyStatus, PluginDoc, PublisherDoc, VersionEntry};

/// What a head index adds or changes relative to its base. Used for the maintainer check and for
/// choosing which packages to download.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Changes {
    /// Publisher ids new in head.
    pub new_publishers: Vec<String>,
    /// `(publisher, key_id)` of keys added to a publisher that already existed in base.
    pub new_keys: Vec<(String, String)>,
    /// `(publisher, key_id)` of keys that went from active to revoked.
    pub revoked_keys: Vec<(String, String)>,
    /// Plugin ids new in head.
    pub new_plugins: Vec<String>,
    /// `(plugin id, version)` of versions new in head.
    pub new_versions: Vec<(String, String)>,
    /// `(plugin id, version)` whose `status` changed.
    pub status_changes: Vec<(String, String)>,
    /// `(plugin id, version)` whose `url` changed (the download is re-checked against the same bytes).
    pub url_changes: Vec<(String, String)>,
}

/// Outcome of a comparison.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Comparison {
    /// Rule violations; empty means `head` is an allowed successor of `base`.
    pub issues: Vec<Issue>,
    /// The changes found (complete even when there are violations).
    pub changes: Changes,
}

/// Compares `head` with `base`.
///
/// Allowed: new publishers, plugins and versions (versions only appended after the existing ones);
/// new keys on a publisher; a key going `active` to `revoked`; a version going `listed` to `yanked`
/// or `revoked`, or `yanked` to `revoked`; changes of `status_reason` and `url`; any edit of display
/// text, images and index metadata. Refused: removal of a publisher, key, plugin or version; any
/// other change of a key; any other change of a version; a status moving backwards.
#[must_use]
pub fn compare(base: &IndexDoc, head: &IndexDoc) -> Comparison {
    let mut issues = Issues::new();
    let mut changes = Changes::default();

    for (id, b) in &base.publishers {
        let p = member("$.publishers", id);
        match head.publishers.get(id) {
            None => issues.push(&p, "publisher removed; publishers are never deleted"),
            Some(h) => compare_publisher(&mut issues, &mut changes, &p, b, h),
        }
    }
    for id in head.publishers.keys().filter(|id| !base.publishers.contains_key(*id)) {
        changes.new_publishers.push(id.clone());
    }

    let head_plugins: BTreeMap<&str, (usize, &PluginDoc)> = head
        .plugins
        .iter()
        .enumerate()
        .map(|(i, p)| (p.id.as_str(), (i, p)))
        .collect();
    for b in &base.plugins {
        match head_plugins.get(b.id.as_str()) {
            None => issues.push(
                "$.plugins",
                format!("plugin `{}` removed; plugins are never deleted", b.id),
            ),
            Some((i, h)) => compare_plugin(&mut issues, &mut changes, &element("$.plugins", *i), b, h),
        }
    }
    let base_ids: Vec<&str> = base.plugins.iter().map(|p| p.id.as_str()).collect();
    for p in head.plugins.iter().filter(|p| !base_ids.contains(&p.id.as_str())) {
        changes.new_plugins.push(p.id.clone());
        changes
            .new_versions
            .extend(p.versions.iter().map(|v| (p.id.clone(), v.version.clone())));
    }

    Comparison {
        issues: issues.finish(),
        changes,
    }
}

fn compare_publisher(issues: &mut Issues, changes: &mut Changes, path: &str, base: &PublisherDoc, head: &PublisherDoc) {
    let kp = member(path, "keys");
    for b in &base.keys {
        let Some((i, h)) = head.keys.iter().enumerate().find(|(_, k)| k.key_id == b.key_id) else {
            issues.push(&kp, format!("key `{}` removed; keys are never deleted", b.key_id));
            continue;
        };
        let p = element(&kp, i);
        if h.public_key != b.public_key || h.added_at != b.added_at {
            issues.push(&p, "key is immutable: public_key and added_at must not change");
        }
        match (b.status, h.status) {
            (KeyStatus::Active, KeyStatus::Revoked) => changes.revoked_keys.push((head.id.clone(), b.key_id.clone())),
            (KeyStatus::Revoked, KeyStatus::Active) => {
                issues.push(&member(&p, "status"), "a revoked key cannot become active");
            }
            (KeyStatus::Revoked, KeyStatus::Revoked) => {
                if h.revoked_at != b.revoked_at || h.reason != b.reason {
                    issues.push(&p, "revocation is final: revoked_at and reason must not change");
                }
            }
            (KeyStatus::Active, KeyStatus::Active) => {}
        }
    }
    for k in head
        .keys
        .iter()
        .filter(|k| !base.keys.iter().any(|b| b.key_id == k.key_id))
    {
        changes.new_keys.push((head.id.clone(), k.key_id.clone()));
    }
}

fn compare_plugin(issues: &mut Issues, changes: &mut Changes, path: &str, base: &PluginDoc, head: &PluginDoc) {
    let vp = member(path, "versions");
    // Append-only: the base versions are a prefix of the head versions, in the same order.
    for (i, b) in base.versions.iter().enumerate() {
        let p = element(&vp, i);
        match head.versions.get(i) {
            Some(h) if h.version == b.version => compare_version(issues, changes, &p, &head.id, b, h),
            Some(_) => issues.push(
                &member(&p, "version"),
                "versions are append-only: order and content of existing versions must not change",
            ),
            None => issues.push(
                &p,
                format!("version `{}` removed; versions are never deleted", b.version),
            ),
        }
    }
    for v in head.versions.iter().skip(base.versions.len()) {
        changes.new_versions.push((head.id.clone(), v.version.clone()));
    }
}

fn compare_version(
    issues: &mut Issues,
    changes: &mut Changes,
    path: &str,
    plugin: &str,
    base: &VersionEntry,
    head: &VersionEntry,
) {
    let key = || (plugin.to_owned(), base.version.clone());
    // Everything except status, status_reason and url is immutable.
    let normalised = |v: &VersionEntry| VersionEntry {
        status: base.status,
        status_reason: None,
        url: String::new(),
        ..v.clone()
    };
    if normalised(base) != normalised(head) {
        issues.push(
            path,
            format!(
                "version `{}` is immutable except status, status_reason and url",
                base.version
            ),
        );
    }
    if head.status != base.status {
        if head.status.rank() < base.status.rank() {
            issues.push(
                &member(path, "status"),
                "status is one-way: listed, then yanked, then revoked",
            );
        } else {
            changes.status_changes.push(key());
        }
    }
    if head.url != base.url {
        changes.url_changes.push(key());
    }
}
