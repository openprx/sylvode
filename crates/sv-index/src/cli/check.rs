//! `sv-index check`: documents, images, freshness of `index.json`, the append-only rules against a
//! base, and the package behind each new or changed version.

use std::collections::BTreeSet;
use std::net::{IpAddr, SocketAddr};
use std::path::Path;
use std::process::Command;

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use serde_json::{Value, json};
use sv_package::Limits;

use super::entry::{contents_of, sha256_hex};
use super::fetch::Fetcher;
use super::repo::{self, Loaded, Meta, PluginFile};
use super::report::{Download, Finding, Report};
use super::{CheckArgs, CliError, DownloadMode};
use crate::{
    Changes, ExactHosts, HostPolicy, IndexDoc, IndexMetadata, KeyStatus, Options, PublisherDoc, VersionEntry,
    VersionStatus, compare, limits, lowest_admitted, parse_compat, parse_index, validate_index,
};

pub fn run(args: &CheckArgs) -> Result<bool, CliError> {
    let root = args.repo.as_path();
    let has_base = args.base.is_some() || args.base_ref.is_some() || args.base_empty;
    let mode = args.download.unwrap_or(if has_base {
        DownloadMode::Changed
    } else {
        DownloadMode::None
    });
    if mode == DownloadMode::Changed && !has_base {
        return Err(CliError(
            "--download changed needs a base (--base, --base-ref or --base-empty)".to_owned(),
        ));
    }
    let base = load_base(args)?;
    let fetcher = if mode == DownloadMode::None {
        None
    } else {
        Some(make_fetcher(args)?)
    };
    let hosts = ExactHosts(args.download_hosts.clone());

    let committed = repo::read_committed(root)?;
    let declared: Option<&IndexMetadata> = committed.as_ref().and_then(|c| c.doc.as_ref()).map(|d| &d.metadata);
    let loaded = repo::load(root, declared, &hosts)?;
    let mut report = Report {
        publishers: loaded.publishers.len(),
        plugins: loaded.plugins.len(),
        versions: loaded.plugins.iter().map(|p| p.doc.versions.len()).sum(),
        findings: loaded.findings.clone(),
        ..Report::default()
    };

    let mut committed_doc: Option<&IndexDoc> = None;
    match &committed {
        None => report.findings.push(Finding::new(
            "index.json",
            "$",
            "stale_index",
            "index.json is missing; run `sv-index build`",
        )),
        Some(c) if c.doc.is_none() => {
            let err = parse_index(&c.bytes, &Options::strict()).err();
            for issue in err.iter().flat_map(|e| &e.issues) {
                report.findings.push(Finding::new(
                    "index.json",
                    &issue.path,
                    "invalid",
                    issue.message.clone(),
                ));
            }
        }
        Some(c) => committed_doc = c.doc.as_ref(),
    }

    match (loaded.broken, committed_doc, committed.as_ref()) {
        (false, Some(prev), Some(c)) => {
            let meta = Meta {
                title: prev.title.clone(),
                description: prev.description.clone(),
                metadata: prev.metadata.clone(),
            };
            let order = repo::plugin_order(&loaded, Some(prev));
            let head = repo::assemble(&loaded, &meta, &order, prev.generated_at);
            cross_findings(&head, &loaded, &order, &hosts, &mut report.findings);
            if repo::render_index(&head)? != c.bytes {
                report.findings.push(Finding::new(
                    "index.json",
                    "$",
                    "stale_index",
                    "index.json differs from what `sv-index build` produces; run it and commit the result",
                ));
            }
            let changes = base.as_ref().map(|base_doc| {
                let cmp = compare(base_doc, &head);
                for issue in &cmp.issues {
                    let (file, path) = relocate(&issue.path, &loaded, &order);
                    report
                        .findings
                        .push(Finding::new(&file, &path, "append_only", issue.message.clone()));
                }
                report.changes = Some(changes_json(&cmp.changes));
                cmp.changes
            });
            if let Some(fetcher) = &fetcher {
                verify_packages(&loaded, mode, changes.as_ref(), fetcher, &mut report);
            }
        }
        _ => report.notes.push(
            "cross-document, freshness, base and download checks skipped: fix the findings above first".to_owned(),
        ),
    }

    if args.json {
        println!("{}", serde_json::to_string_pretty(&report.to_json())?);
    } else {
        print!("{}", report.render("check"));
    }
    Ok(report.ok())
}

/// Cross-document rules of the assembled index, reported against the file they concern.
pub fn cross_findings(
    doc: &IndexDoc,
    loaded: &Loaded,
    order: &[usize],
    hosts: &dyn HostPolicy,
    out: &mut Vec<Finding>,
) {
    for issue in validate_index(doc, hosts).finish() {
        let (file, path) = relocate(&issue.path, loaded, order);
        out.push(Finding::new(&file, &path, "invalid", issue.message));
    }
}

/// Maps a path of the assembled index (`$.plugins[3].versions[0].sha256`) to its source file and the
/// path inside that file.
fn relocate(path: &str, loaded: &Loaded, order: &[usize]) -> (String, String) {
    if let Some(rest) = path.strip_prefix("$.plugins[")
        && let Some((n, tail)) = rest.split_once(']')
        && let Ok(i) = n.parse::<usize>()
        && let Some(p) = order.get(i).and_then(|k| loaded.plugins.get(*k))
    {
        return (p.file.clone(), format!("${tail}"));
    }
    if let Some(rest) = path.strip_prefix("$.publishers.") {
        let end = rest.find(['.', '[']).unwrap_or(rest.len());
        let (id, tail) = rest.split_at(end);
        if let Some((file, _)) = loaded.publishers.iter().find(|(_, p)| p.id == id) {
            return (file.clone(), format!("${tail}"));
        }
    }
    ("index.json".to_owned(), path.to_owned())
}

fn changes_json(c: &Changes) -> Value {
    let pairs = |v: &[(String, String)], a: &str, b: &str| -> Value {
        v.iter().map(|(x, y)| json!({ a: x, b: y })).collect::<Vec<_>>().into()
    };
    json!({
        "new_publishers": c.new_publishers,
        "new_keys": pairs(&c.new_keys, "publisher", "key_id"),
        "revoked_keys": pairs(&c.revoked_keys, "publisher", "key_id"),
        "new_plugins": c.new_plugins,
        "new_versions": pairs(&c.new_versions, "plugin", "version"),
        "status_changes": pairs(&c.status_changes, "plugin", "version"),
        "url_changes": pairs(&c.url_changes, "plugin", "version"),
    })
}

fn load_base(args: &CheckArgs) -> Result<Option<IndexDoc>, CliError> {
    let hint = "pass --base-empty when this is the first commit";
    let bytes = if let Some(dir) = &args.base {
        let path = dir.join("index.json");
        if !path.is_file() {
            return Err(CliError(format!("base {} has no index.json; {hint}", dir.display())));
        }
        repo::read_bounded(&path, limits::MAX_INDEX_BYTES)?
    } else if let Some(git_ref) = &args.base_ref {
        git_base(&args.repo, git_ref, hint)?
    } else if args.base_empty {
        return Ok(Some(IndexDoc {
            schema: crate::INDEX_SCHEMA.to_owned(),
            generated_at: 0,
            title: std::collections::BTreeMap::new(),
            description: None,
            metadata: IndexMetadata {
                categories: std::collections::BTreeMap::new(),
                labels: std::collections::BTreeMap::new(),
            },
            publishers: std::collections::BTreeMap::new(),
            plugins: Vec::new(),
        }));
    } else {
        return Ok(None);
    };
    parse_index(&bytes, &Options::lenient())
        .map(Some)
        .map_err(|e| CliError(format!("the base index.json is invalid:\n{e}")))
}

fn git_base(repo: &Path, git_ref: &str, hint: &str) -> Result<Vec<u8>, CliError> {
    if git_ref.is_empty() || git_ref.starts_with('-') {
        return Err(CliError("--base-ref is not a valid ref".to_owned()));
    }
    let git = |args: &[&str]| {
        Command::new("git")
            .arg("-C")
            .arg(repo)
            .args(args)
            .output()
            .map_err(|e| CliError(format!("cannot run git: {e}")))
    };
    let listing = git(&["ls-tree", "--name-only", git_ref, "--", "index.json"])?;
    if !listing.status.success() {
        return Err(CliError(format!(
            "git cannot read ref `{git_ref}` in {}",
            repo.display()
        )));
    }
    if listing.stdout.is_empty() {
        return Err(CliError(format!("`{git_ref}` has no index.json; {hint}")));
    }
    let shown = git(&["show", &format!("{git_ref}:index.json")])?;
    if !shown.status.success() {
        return Err(CliError(format!("git cannot show index.json of `{git_ref}`")));
    }
    if shown.stdout.len() as u64 > limits::MAX_INDEX_BYTES {
        return Err(CliError("the base index.json is too large".to_owned()));
    }
    Ok(shown.stdout)
}

fn make_fetcher(args: &CheckArgs) -> Result<Fetcher, CliError> {
    let mut pins = Vec::new();
    for spec in &args.resolve {
        let (host, ip) = spec
            .split_once('=')
            .ok_or_else(|| CliError(format!("--resolve `{spec}`: expected HOST=IP")))?;
        let addr: SocketAddr = ip
            .parse()
            .or_else(|_| ip.parse::<IpAddr>().map(|ip| SocketAddr::new(ip, 443)))
            .map_err(|_| {
                CliError(format!(
                    "--resolve `{spec}`: expected an IP address, optionally with :port"
                ))
            })?;
        pins.push((host.to_ascii_lowercase(), addr));
    }
    let mut roots = Vec::new();
    for file in &args.ca_file {
        let pem = repo::read_bounded(file, limits::MAX_REPO_FILE_BYTES)?;
        let certs = reqwest::Certificate::from_pem_bundle(&pem)
            .map_err(|e| CliError(format!("--ca-file {}: {e}", file.display())))?;
        roots.extend(certs);
    }
    Ok(Fetcher::new(args.download_hosts.clone(), pins, roots))
}

/// Downloads and verifies the selected versions.
fn verify_packages(
    loaded: &Loaded,
    mode: DownloadMode,
    changes: Option<&Changes>,
    fetcher: &Fetcher,
    report: &mut Report,
) {
    let selected: BTreeSet<(&str, &str)> = changes
        .map(|c| {
            c.new_versions
                .iter()
                .chain(&c.url_changes)
                .map(|(p, v)| (p.as_str(), v.as_str()))
                .collect()
        })
        .unwrap_or_default();
    for plugin in &loaded.plugins {
        for (j, v) in plugin.doc.versions.iter().enumerate() {
            if mode == DownloadMode::Changed && !selected.contains(&(plugin.doc.id.as_str(), v.version.as_str())) {
                continue;
            }
            let mut download = Download {
                plugin: plugin.doc.id.clone(),
                version: v.version.clone(),
                url: v.url.clone(),
                outcome: "ok",
                note: None,
            };
            if v.status != VersionStatus::Listed {
                download.outcome = "skipped";
                download.note = Some("version is not listed".to_owned());
            } else if let Some(publisher) = loaded.publishers.iter().map(|(_, p)| p).find(|p| p.id == v.publisher) {
                let before = report.findings.len();
                check_version(fetcher, plugin, j, v, publisher, &mut report.findings);
                if report.findings.len() > before {
                    download.outcome = "failed";
                    download.note = report.findings.get(before).map(|f| format!("{} {}", f.path, f.message));
                }
            } else {
                download.outcome = "skipped";
                download.note = Some("publisher not found".to_owned());
            }
            report.downloads.push(download);
        }
    }
}

fn check_version(
    fetcher: &Fetcher,
    plugin: &PluginFile,
    j: usize,
    v: &VersionEntry,
    publisher: &PublisherDoc,
    findings: &mut Vec<Finding>,
) {
    let file = plugin.file.as_str();
    let at = |member: &str| format!("$.versions[{j}].{member}");
    let mut add = |member: &str, code: &'static str, message: String| {
        findings.push(Finding::new(file, &at(member), code, message));
    };

    let Some(key) = publisher.keys.iter().find(|k| k.key_id == v.key_id) else {
        add(
            "key_id",
            "key_unknown",
            "key is not one of the publisher's keys".to_owned(),
        );
        return;
    };
    if key.status == KeyStatus::Revoked {
        add(
            "key_id",
            "key_revoked",
            "a listed version is signed by a revoked key; yank or revoke the version".to_owned(),
        );
        return;
    }
    let bytes = match fetcher.fetch(&v.url, v.size) {
        Ok(b) => b,
        Err(e) => {
            let member = if matches!(e, super::fetch::FetchError::TooLarge) {
                "size"
            } else {
                "url"
            };
            add(member, "download_failed", e.message());
            return;
        }
    };
    if bytes.len() as u64 != v.size {
        add(
            "size",
            "size_mismatch",
            format!("the download is {} bytes, the index says {}", bytes.len(), v.size),
        );
    }
    let sha = sha256_hex(&bytes);
    if sha != v.sha256 {
        add(
            "sha256",
            "sha256_mismatch",
            format!("the download hashes to {sha}, the index says {}", v.sha256),
        );
    }
    let pkg = match sv_package::unpack(&bytes, &Limits::default()) {
        Ok(p) => p,
        Err(e) => {
            add("url", "package_invalid", format!("the package is not valid: {e}"));
            return;
        }
    };
    compare_manifest(&pkg, plugin, v, &mut add);
    match URL_SAFE_NO_PAD.decode(&key.public_key) {
        Ok(public) => {
            if let Err(e) = pkg.verify_with_key(&public) {
                add(
                    "key_id",
                    "signature_invalid",
                    format!("the package signature does not verify: {e}"),
                );
            }
        }
        Err(_) => add("key_id", "key_invalid", "public_key is not base64url".to_owned()),
    }
}

/// The manifest must say what the index says.
fn compare_manifest(
    pkg: &sv_package::UnverifiedPackage,
    plugin: &PluginFile,
    v: &VersionEntry,
    add: &mut impl FnMut(&str, &'static str, String),
) {
    let m = &pkg.manifest;
    let mut differ = |member: &str, index: String, manifest: String| {
        add(
            member,
            "manifest_mismatch",
            format!("the index says {index}, the package says {manifest}"),
        );
    };
    if m.id != plugin.doc.id {
        differ(
            "version",
            format!("plugin `{}`", plugin.doc.id),
            format!("id `{}`", m.id),
        );
    }
    if m.version != v.version {
        differ("version", v.version.clone(), m.version.clone());
    }
    if m.kind != plugin.doc.kind {
        differ(
            "version",
            format!("kind {:?}", plugin.doc.kind),
            format!("kind {:?}", m.kind),
        );
    }
    if m.publisher.id != v.publisher {
        differ("publisher", v.publisher.clone(), m.publisher.id.clone());
    }
    if m.publisher.key_id != v.key_id {
        differ("key_id", v.key_id.clone(), m.publisher.key_id.clone());
    }
    if pkg.digest != v.digest {
        differ("digest", v.digest.clone(), pkg.digest.clone());
    }
    match (parse_compat(&v.sylvode), semver::Version::parse(&m.sv.min)) {
        (Ok(req), Ok(min)) => match lowest_admitted(&req) {
            Some(lowest) if lowest >= min => {}
            Some(lowest) => add(
                "sylvode",
                "compat_mismatch",
                format!("the range admits Sylvode {lowest}, below the package's sv.min {min}"),
            ),
            None => add("sylvode", "compat_mismatch", "the range admits no version".to_owned()),
        },
        (Err(_), _) => {} // reported by the document rules
        (_, Err(_)) => add(
            "sylvode",
            "compat_mismatch",
            "the package's sv.min is not a version".to_owned(),
        ),
    }
    let listed = serde_json::to_value(&v.contents).unwrap_or(Value::Null);
    let actual = serde_json::to_value(contents_of(m)).unwrap_or(Value::Null);
    if let (Value::Object(l), Value::Object(a)) = (&listed, &actual) {
        for (name, want) in a {
            if l.get(name) != Some(want) {
                add(
                    &format!("contents.{name}"),
                    "contents_mismatch",
                    format!(
                        "the index lists {}, the package has {want}",
                        l.get(name).unwrap_or(&Value::Null)
                    ),
                );
            }
        }
    }
}
