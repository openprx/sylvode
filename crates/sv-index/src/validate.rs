//! Semantic validation of typed documents. Every function reports all findings with JSON paths.

use std::collections::{BTreeMap, BTreeSet};

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;

use crate::error::{Issues, element, member};
use crate::limits;
use crate::model::{
    Contents, I18nText, INDEX_SCHEMA, IndexDoc, IndexMetadata, KeyStatus, PLUGIN_SCHEMA, PUBLISHER_SCHEMA, PluginDoc,
    PublisherDoc, PublisherKey, VersionEntry, VersionStatus,
};
use crate::rules::{self, HostPolicy, Text, check_i18n, check_text, check_unique};

fn check_schema(issues: &mut Issues, path: &str, got: &str, want: &str) {
    if got != want {
        issues.push(&member(path, "schema"), format!("must be `{want}`"));
    }
}

fn check_time(issues: &mut Issues, path: &str, t: u64) {
    if t >= limits::MAX_TIMESTAMP {
        issues.push(path, "timestamp must be Unix seconds below 10^11");
    }
}

fn check_web(issues: &mut Issues, path: &str, url: &str) {
    if let Some(why) = rules::web_url_problem(url) {
        issues.push(path, why);
    }
}

fn check_opt_i18n(issues: &mut Issues, path: &str, map: Option<&I18nText>, max: usize, kind: Text) {
    if let Some(m) = map {
        check_i18n(issues, path, m, max, kind);
    }
}

/// Validates a publisher document.
pub fn validate_publisher_into(issues: &mut Issues, path: &str, doc: &PublisherDoc) {
    check_schema(issues, path, &doc.schema, PUBLISHER_SCHEMA);
    if let Some(why) = rules::publisher_id_problem(&doc.id) {
        issues.push(&member(path, "id"), why);
    }
    check_i18n(
        issues,
        &member(path, "name"),
        &doc.name,
        limits::MAX_TITLE_CHARS,
        Text::Line,
    );
    if let Some(h) = &doc.homepage {
        check_web(issues, &member(path, "homepage"), h);
    }
    let mp = member(path, "maintainers");
    if !(1..=limits::MAX_MAINTAINERS).contains(&doc.maintainers.len()) {
        issues.push(&mp, format!("1..{} maintainers required", limits::MAX_MAINTAINERS));
    }
    for (i, m) in doc.maintainers.iter().enumerate() {
        if !rules::github_login_ok(m) {
            issues.push(&element(&mp, i), "not a GitHub login");
        }
    }
    check_unique(issues, &mp, doc.maintainers.iter(), "maintainer");
    let kp = member(path, "keys");
    if !(1..=limits::MAX_KEYS).contains(&doc.keys.len()) {
        issues.push(&kp, format!("1..{} keys required", limits::MAX_KEYS));
    }
    let mut ids = BTreeSet::new();
    let mut publics = BTreeSet::new();
    for (i, key) in doc.keys.iter().enumerate() {
        let p = element(&kp, i);
        validate_key(issues, &p, key);
        if !ids.insert(key.key_id.as_str()) {
            issues.push(&member(&p, "key_id"), "duplicate key_id");
        }
        if !publics.insert(key.public_key.as_str()) {
            issues.push(&member(&p, "public_key"), "duplicate public_key");
        }
    }
}

fn validate_key(issues: &mut Issues, path: &str, key: &PublisherKey) {
    let kid = member(path, "key_id");
    if !rules::key_id_ok(&key.key_id) {
        issues.push(&kid, "must be `ed25519:` followed by 32 lower-case hex digits");
    }
    match URL_SAFE_NO_PAD.decode(&key.public_key) {
        Ok(raw) if raw.len() == 32 => {
            if rules::key_id_ok(&key.key_id) && sv_package::key_id(&raw) != key.key_id {
                issues.push(&kid, "does not match the fingerprint of public_key");
            }
        }
        _ => issues.push(&member(path, "public_key"), "must be unpadded base64url of 32 bytes"),
    }
    check_time(issues, &member(path, "added_at"), key.added_at);
    let (ra, rr) = (member(path, "revoked_at"), member(path, "reason"));
    match key.status {
        KeyStatus::Active => {
            if key.revoked_at.is_some() {
                issues.push(&ra, "must be absent for an active key");
            }
            if key.reason.is_some() {
                issues.push(&rr, "must be absent for an active key");
            }
        }
        KeyStatus::Revoked => {
            match key.revoked_at {
                None => issues.push(&ra, "required for a revoked key"),
                Some(t) => {
                    check_time(issues, &ra, t);
                    if t < key.added_at {
                        issues.push(&ra, "must not be earlier than added_at");
                    }
                }
            }
            match &key.reason {
                None => issues.push(&rr, "required for a revoked key"),
                Some(r) => check_text(issues, &rr, r, limits::MAX_REASON_CHARS),
            }
        }
    }
}

/// Validates a publisher document on its own.
pub fn validate_publisher(doc: &PublisherDoc) -> Issues {
    let mut issues = Issues::new();
    validate_publisher_into(&mut issues, "$", doc);
    issues
}

fn check_listed_items(issues: &mut Issues, path: &str, items: &[String]) {
    if items.len() > limits::MAX_CONTENT_ITEMS {
        issues.push(path, format!("at most {} entries", limits::MAX_CONTENT_ITEMS));
    }
    for (i, s) in items.iter().enumerate() {
        check_text(issues, &element(path, i), s, limits::MAX_CONTENT_CHARS);
    }
}

fn validate_contents(issues: &mut Issues, path: &str, c: &Contents) {
    for (name, list) in [
        ("requests", &c.requests),
        ("actions", &c.actions),
        ("rules", &c.rules),
        ("binds", &c.binds),
        ("forms", &c.forms),
    ] {
        check_listed_items(issues, &member(path, name), list);
    }
    let lp = member(path, "layouts");
    if c.layouts.len() > limits::MAX_CONTENT_ITEMS {
        issues.push(&lp, format!("at most {} entries", limits::MAX_CONTENT_ITEMS));
    }
    for (i, l) in c.layouts.iter().enumerate() {
        let p = element(&lp, i);
        check_text(issues, &member(&p, "key"), &l.key, limits::MAX_CONTENT_CHARS);
        check_listed_items(issues, &member(&p, "slots"), &l.slots);
    }
    let pp = member(path, "pages");
    if c.pages.len() > limits::MAX_CONTENT_ITEMS {
        issues.push(&pp, format!("at most {} entries", limits::MAX_CONTENT_ITEMS));
    }
    for (i, page) in c.pages.iter().enumerate() {
        let p = element(&pp, i);
        if !page.route.starts_with('/') || page.route.chars().count() > 200 || page.route.chars().any(char::is_control)
        {
            issues.push(
                &member(&p, "route"),
                "must start with `/`, at most 200 characters, no control characters",
            );
        }
        if !matches!(page.audience.as_str(), "visitor" | "operator") {
            issues.push(&member(&p, "audience"), "must be `visitor` or `operator`");
        }
    }
}

fn validate_version(issues: &mut Issues, path: &str, plugin_publisher: &str, v: &VersionEntry, hosts: &dyn HostPolicy) {
    if let Err(why) = rules::parse_exact_version(&v.version) {
        issues.push(&member(path, "version"), why);
    }
    check_time(issues, &member(path, "published_at"), v.published_at);
    let sp = member(path, "status_reason");
    match (&v.status_reason, v.status) {
        (None, VersionStatus::Yanked | VersionStatus::Revoked) => {
            issues.push(&sp, "required unless the version is listed");
        }
        (Some(r), _) => check_text(issues, &sp, r, limits::MAX_REASON_CHARS),
        (None, VersionStatus::Listed) => {}
    }
    if let Some(why) = rules::download_url_problem(&v.url, hosts) {
        issues.push(&member(path, "url"), why);
    }
    if v.size == 0 || v.size > limits::MAX_PACKAGE_BYTES {
        issues.push(
            &member(path, "size"),
            format!("must be 1..={} bytes", limits::MAX_PACKAGE_BYTES),
        );
    }
    if !rules::sha256_ok(&v.sha256) {
        issues.push(&member(path, "sha256"), "must be 64 lower-case hex digits");
    }
    if !rules::digest_ok(&v.digest) {
        issues.push(
            &member(path, "digest"),
            "must be `sha256:` followed by 64 lower-case hex digits",
        );
    }
    if v.publisher != plugin_publisher {
        issues.push(
            &member(path, "publisher"),
            "must equal the publisher prefix of the plugin id",
        );
    }
    if !rules::key_id_ok(&v.key_id) {
        issues.push(
            &member(path, "key_id"),
            "must be `ed25519:` followed by 32 lower-case hex digits",
        );
    }
    if let Err(why) = rules::parse_compat(&v.sylvode) {
        issues.push(&member(path, "sylvode"), why);
    }
    validate_contents(issues, &member(path, "contents"), &v.contents);
    check_opt_i18n(
        issues,
        &member(path, "changelog"),
        v.changelog.as_ref(),
        limits::MAX_CHANGELOG_CHARS,
        Text::Block,
    );
}

fn validate_metadata(issues: &mut Issues, path: &str, doc: &PluginDoc, declared: Option<&IndexMetadata>) {
    let m = &doc.metadata;
    let cp = member(path, "category");
    if !rules::meta_id_ok(&m.category) {
        issues.push(&cp, "must match ^[a-z][a-z0-9-]{0,31}$");
    } else if declared.is_some_and(|d| !d.categories.contains_key(&m.category)) {
        issues.push(
            &cp,
            format!("category `{}` is not declared in the index metadata", m.category),
        );
    }
    let lp = member(path, "labels");
    if m.labels.len() > limits::MAX_LABELS {
        issues.push(&lp, format!("at most {} labels", limits::MAX_LABELS));
    }
    for (i, l) in m.labels.iter().enumerate() {
        let p = element(&lp, i);
        if !rules::meta_id_ok(l) {
            issues.push(&p, "must match ^[a-z][a-z0-9-]{0,31}$");
        } else if declared.is_some_and(|d| !d.labels.contains_key(l)) {
            issues.push(&p, format!("label `{l}` is not declared in the index metadata"));
        }
    }
    check_unique(issues, &lp, m.labels.iter(), "label");
    let kp = member(path, "keywords");
    if m.keywords.len() > limits::MAX_KEYWORDS {
        issues.push(&kp, format!("at most {} keywords", limits::MAX_KEYWORDS));
    }
    for (i, k) in m.keywords.iter().enumerate() {
        check_text(issues, &element(&kp, i), k, limits::MAX_KEYWORD_CHARS);
    }
    check_unique(issues, &kp, m.keywords.iter(), "keyword");
}

/// Validates a plugin document. `declared` is the index metadata the category and labels are checked
/// against (skipped when `None`); `hosts` decides which download hosts are acceptable.
pub fn validate_plugin_into(
    issues: &mut Issues,
    path: &str,
    doc: &PluginDoc,
    declared: Option<&IndexMetadata>,
    hosts: &dyn HostPolicy,
) {
    check_schema(issues, path, &doc.schema, PLUGIN_SCHEMA);
    let publisher = rules::plugin_id_publisher(&doc.id);
    if publisher.is_none() {
        issues.push(
            &member(path, "id"),
            "must be `<publisher>/<name>`, lower-case letters, digits and hyphens",
        );
    }
    check_i18n(
        issues,
        &member(path, "title"),
        &doc.title,
        limits::MAX_TITLE_CHARS,
        Text::Line,
    );
    check_i18n(
        issues,
        &member(path, "summary"),
        &doc.summary,
        limits::MAX_SUMMARY_CHARS,
        Text::Line,
    );
    check_i18n(
        issues,
        &member(path, "description"),
        &doc.description,
        limits::MAX_DESCRIPTION_CHARS,
        Text::Block,
    );
    if let Some(why) = rules::icon_path_problem(&doc.icon) {
        issues.push(&member(path, "icon"), why);
    }
    let sp = member(path, "screenshots");
    if doc.screenshots.len() > limits::MAX_SCREENSHOTS {
        issues.push(&sp, format!("at most {} screenshots", limits::MAX_SCREENSHOTS));
    }
    for (i, s) in doc.screenshots.iter().enumerate() {
        let p = element(&sp, i);
        if let Some(why) = rules::screenshot_path_problem(&s.path) {
            issues.push(&member(&p, "path"), why);
        }
        check_i18n(
            issues,
            &member(&p, "caption"),
            &s.caption,
            limits::MAX_CAPTION_CHARS,
            Text::Line,
        );
    }
    check_unique(issues, &sp, doc.screenshots.iter().map(|s| &s.path), "screenshot path");
    for (name, url) in [("homepage", &doc.homepage), ("repository", &doc.repository)] {
        if let Some(u) = url {
            check_web(issues, &member(path, name), u);
        }
    }
    check_text(
        issues,
        &member(path, "license"),
        &doc.license,
        limits::MAX_LICENSE_CHARS,
    );
    validate_metadata(issues, &member(path, "metadata"), doc, declared);

    let vp = member(path, "versions");
    if !(1..=limits::MAX_VERSIONS).contains(&doc.versions.len()) {
        issues.push(&vp, format!("1..{} versions required", limits::MAX_VERSIONS));
    }
    let mut seen = BTreeSet::new();
    for (i, v) in doc.versions.iter().enumerate() {
        let p = element(&vp, i);
        validate_version(issues, &p, publisher.unwrap_or_default(), v, hosts);
        if !seen.insert(v.version.as_str()) {
            issues.push(&member(&p, "version"), format!("duplicate version `{}`", v.version));
        }
    }
    let lp = member(&member(path, "channels"), "latest");
    match doc.versions.iter().find(|v| v.version == doc.channels.latest) {
        Some(v) if v.status == VersionStatus::Listed => {}
        Some(_) => issues.push(&lp, "must name a listed version"),
        None => issues.push(&lp, "names a version that does not exist"),
    }
}

/// Validates a plugin document on its own.
pub fn validate_plugin(doc: &PluginDoc, declared: Option<&IndexMetadata>, hosts: &dyn HostPolicy) -> Issues {
    let mut issues = Issues::new();
    validate_plugin_into(&mut issues, "$", doc, declared, hosts);
    issues
}

/// Validates the whole index: every document, plus the cross-document rules (publisher map keys,
/// unique plugin ids, a key in one publisher only, version keys known to their publisher).
pub fn validate_index(doc: &IndexDoc, hosts: &dyn HostPolicy) -> Issues {
    let mut issues = Issues::new();
    check_schema(&mut issues, "$", &doc.schema, INDEX_SCHEMA);
    check_time(&mut issues, "$.generated_at", doc.generated_at);
    check_i18n(&mut issues, "$.title", &doc.title, limits::MAX_TITLE_CHARS, Text::Line);
    check_opt_i18n(
        &mut issues,
        "$.description",
        doc.description.as_ref(),
        limits::MAX_INDEX_DESCRIPTION_CHARS,
        Text::Line,
    );
    for (group, map) in [
        ("categories", &doc.metadata.categories),
        ("labels", &doc.metadata.labels),
    ] {
        let gp = member("$.metadata", group);
        for (id, entry) in map {
            let p = member(&gp, id);
            if !rules::meta_id_ok(id) {
                issues.push(&p, "id must match ^[a-z][a-z0-9-]{0,31}$");
            }
            check_i18n(
                &mut issues,
                &member(&p, "title"),
                &entry.title,
                limits::MAX_TITLE_CHARS,
                Text::Line,
            );
        }
    }

    let mut key_owner: BTreeMap<&str, &str> = BTreeMap::new();
    for (id, publisher) in &doc.publishers {
        let p = member("$.publishers", id);
        validate_publisher_into(&mut issues, &p, publisher);
        if *id != publisher.id {
            issues.push(&member(&p, "id"), "must equal the map key");
        }
        for (i, k) in publisher.keys.iter().enumerate() {
            if let Some(other) = key_owner.insert(k.key_id.as_str(), id.as_str())
                && other != id
            {
                issues.push(
                    &member(&element(&member(&p, "keys"), i), "key_id"),
                    format!("key already listed under publisher `{other}`"),
                );
            }
        }
    }

    let mut ids = BTreeSet::new();
    for (i, plugin) in doc.plugins.iter().enumerate() {
        let p = element("$.plugins", i);
        validate_plugin_into(&mut issues, &p, plugin, Some(&doc.metadata), hosts);
        if !ids.insert(plugin.id.as_str()) {
            issues.push(&member(&p, "id"), format!("duplicate plugin id `{}`", plugin.id));
        }
        let Some(prefix) = rules::plugin_id_publisher(&plugin.id) else {
            continue;
        };
        let Some(publisher) = doc.publishers.get(prefix) else {
            issues.push(&member(&p, "id"), format!("publisher `{prefix}` is not listed"));
            continue;
        };
        for (j, v) in plugin.versions.iter().enumerate() {
            if !publisher.keys.iter().any(|k| k.key_id == v.key_id) {
                issues.push(
                    &member(&element(&member(&p, "versions"), j), "key_id"),
                    format!("key is not one of publisher `{prefix}`'s keys"),
                );
            }
        }
    }
    issues
}
