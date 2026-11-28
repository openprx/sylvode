//! Reading an index repository checkout: the file rules, every document, the images, and the
//! assembly of `index.json` from them.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};

use super::CliError;
use super::report::Finding;
use crate::limits;
use crate::{
    HostPolicy, I18nText, ImageRole, IndexDoc, IndexMetadata, Mode, Options, PluginDoc, PublisherDoc, ValidationError,
    check_image, parse_plugin, parse_publisher,
};

const INDEX_FILE: &str = "index.json";
/// Directory depth and entry count the repository walk accepts, so a hostile tree stays bounded.
const MAX_DEPTH: usize = 12;
const MAX_ENTRIES: usize = 50_000;

/// A plugin document with where it came from.
pub struct PluginFile {
    /// `plugins/<publisher>/<name>/plugin.json`.
    pub file: String,
    pub doc: PluginDoc,
}

/// The documents of a checkout.
#[derive(Default)]
pub struct Loaded {
    /// `(publishers/<id>.json, document)`.
    pub publishers: Vec<(String, PublisherDoc)>,
    pub plugins: Vec<PluginFile>,
    pub findings: Vec<Finding>,
    /// A document failed to read or parse; cross-document steps are skipped.
    pub broken: bool,
}

/// The owner-curated part of `index.json`.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct Meta {
    pub title: I18nText,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<I18nText>,
    pub metadata: IndexMetadata,
}

/// The committed `index.json`, as far as it can be read.
pub struct Committed {
    pub bytes: Vec<u8>,
    /// Lenient decode (no validation); `None` when it is not even the right shape.
    pub doc: Option<IndexDoc>,
}

fn io(what: &str, path: &Path, e: &std::io::Error) -> CliError {
    CliError(format!("{what} {}: {e}", path.display()))
}

/// Reads a file of at most `max` bytes.
pub fn read_bounded(path: &Path, max: u64) -> Result<Vec<u8>, CliError> {
    let file = fs::File::open(path).map_err(|e| io("cannot open", path, &e))?;
    let mut out = Vec::new();
    file.take(max.saturating_add(1))
        .read_to_end(&mut out)
        .map_err(|e| io("cannot read", path, &e))?;
    if out.len() as u64 > max {
        return Err(CliError(format!("{} is larger than {max} bytes", path.display())));
    }
    Ok(out)
}

/// The committed `index.json` of a checkout, `None` when absent.
pub fn read_committed(root: &Path) -> Result<Option<Committed>, CliError> {
    let path = root.join(INDEX_FILE);
    if !path.exists() {
        return Ok(None);
    }
    let bytes = read_bounded(&path, limits::MAX_INDEX_BYTES)?;
    let doc = serde_json::from_slice(&bytes).ok();
    Ok(Some(Committed { bytes, doc }))
}

#[derive(PartialEq, Eq)]
enum Kind {
    Dir,
    File,
    Other,
}

/// Sorted `(name, kind)` of a directory; symbolic links are `Other` (never followed).
fn entries(dir: &Path) -> Result<Vec<(String, Kind)>, CliError> {
    let mut out = Vec::new();
    let read = match fs::read_dir(dir) {
        Ok(r) => r,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(out),
        Err(e) => return Err(io("cannot read directory", dir, &e)),
    };
    for entry in read {
        let entry = entry.map_err(|e| io("cannot read directory", dir, &e))?;
        let kind = match entry.file_type() {
            Ok(t) if t.is_dir() => Kind::Dir,
            Ok(t) if t.is_file() => Kind::File,
            _ => Kind::Other,
        };
        out.push((entry.file_name().to_string_lossy().into_owned(), kind));
    }
    out.sort_by(|a, b| a.0.cmp(&b.0));
    Ok(out)
}

/// Repository-wide file rules: no package files, nothing over 1 MiB, no symbolic links.
fn scan_tree(root: &Path, findings: &mut Vec<Finding>) -> Result<(), CliError> {
    let mut stack = vec![(PathBuf::new(), 0usize)];
    let mut seen = 0usize;
    while let Some((rel, depth)) = stack.pop() {
        for (name, kind) in entries(&root.join(&rel))? {
            if rel.as_os_str().is_empty() && name == ".git" {
                continue;
            }
            seen += 1;
            if seen > MAX_ENTRIES {
                findings.push(Finding::new(".", "$", "repo_limits", "repository has too many entries"));
                return Ok(());
            }
            let child = rel.join(&name);
            let shown = child.to_string_lossy().replace('\\', "/");
            match kind {
                Kind::Other => findings.push(Finding::new(
                    &shown,
                    "$",
                    "file_refused",
                    "not a regular file or directory (symbolic links are refused)",
                )),
                Kind::Dir if depth + 1 > MAX_DEPTH => {
                    findings.push(Finding::new(&shown, "$", "repo_limits", "directory nested too deeply"));
                }
                Kind::Dir => stack.push((child, depth + 1)),
                Kind::File => {
                    let ext = Path::new(&name)
                        .extension()
                        .and_then(|e| e.to_str())
                        .unwrap_or_default();
                    if ext.eq_ignore_ascii_case("svpkg") || ext.eq_ignore_ascii_case("zip") {
                        findings.push(Finding::new(
                            &shown,
                            "$",
                            "package_file",
                            "package files are never committed; host the package at an https URL",
                        ));
                    }
                    let len = fs::metadata(root.join(&child)).map_or(0, |m| m.len());
                    if len > limits::MAX_REPO_FILE_BYTES {
                        findings.push(Finding::new(
                            &shown,
                            "$",
                            "file_too_large",
                            format!("{len} bytes, at most {} allowed", limits::MAX_REPO_FILE_BYTES),
                        ));
                    }
                }
            }
        }
    }
    Ok(())
}

fn push_issues(findings: &mut Vec<Finding>, file: &str, err: &ValidationError) {
    for issue in &err.issues {
        findings.push(Finding::new(file, &issue.path, "invalid", issue.message.clone()));
    }
}

/// Loads and validates every document and image of a checkout.
pub fn load(root: &Path, declared: Option<&IndexMetadata>, hosts: &dyn HostPolicy) -> Result<Loaded, CliError> {
    let mut loaded = Loaded::default();
    scan_tree(root, &mut loaded.findings)?;
    let opts = Options {
        mode: Mode::Strict,
        hosts,
    };
    load_publishers(root, &opts, &mut loaded)?;
    for (publisher, kind) in entries(&root.join("plugins"))? {
        if kind != Kind::Dir {
            loaded.findings.push(Finding::new(
                &format!("plugins/{publisher}"),
                "$",
                "unexpected_file",
                "only `<publisher>/<name>/` directories belong under plugins/",
            ));
            continue;
        }
        for (name, kind) in entries(&root.join("plugins").join(&publisher))? {
            let dir = format!("plugins/{publisher}/{name}");
            if kind != Kind::Dir {
                loaded.findings.push(Finding::new(
                    &dir,
                    "$",
                    "unexpected_file",
                    "only `<name>/` directories belong under a publisher directory",
                ));
                continue;
            }
            load_plugin(root, &dir, &format!("{publisher}/{name}"), declared, &opts, &mut loaded)?;
        }
    }
    Ok(loaded)
}

fn load_publishers(root: &Path, opts: &Options<'_>, loaded: &mut Loaded) -> Result<(), CliError> {
    for (name, kind) in entries(&root.join("publishers"))? {
        let file = format!("publishers/{name}");
        let Some(stem) = name.strip_suffix(".json").filter(|_| kind == Kind::File) else {
            loaded.findings.push(Finding::new(
                &file,
                "$",
                "unexpected_file",
                "only `<id>.json` files belong under publishers/",
            ));
            continue;
        };
        let bytes = match read_bounded(&root.join(&file), limits::MAX_REPO_FILE_BYTES) {
            Ok(b) => b,
            Err(e) => {
                loaded.broken = true;
                loaded.findings.push(Finding::new(&file, "$", "unreadable", e.0));
                continue;
            }
        };
        match parse_publisher(&bytes, opts) {
            Ok(doc) => {
                if doc.id != stem {
                    loaded.findings.push(Finding::new(
                        &file,
                        "$.id",
                        "id_mismatch",
                        format!("must equal the file name `{stem}`"),
                    ));
                }
                loaded.publishers.push((file, doc));
            }
            Err(e) => {
                loaded.broken = true;
                push_issues(&mut loaded.findings, &file, &e);
            }
        }
    }
    Ok(())
}

fn load_plugin(
    root: &Path,
    dir: &str,
    expected_id: &str,
    declared: Option<&IndexMetadata>,
    opts: &Options<'_>,
    loaded: &mut Loaded,
) -> Result<(), CliError> {
    let file = format!("{dir}/plugin.json");
    let bytes = match read_bounded(&root.join(&file), limits::MAX_REPO_FILE_BYTES) {
        Ok(b) => b,
        Err(e) => {
            loaded.broken = true;
            loaded.findings.push(Finding::new(&file, "$", "unreadable", e.0));
            return Ok(());
        }
    };
    let doc = match parse_plugin(&bytes, declared, opts) {
        Ok(d) => d,
        Err(e) => {
            loaded.broken = true;
            push_issues(&mut loaded.findings, &file, &e);
            return Ok(());
        }
    };
    if doc.id != expected_id {
        loaded.broken = true;
        loaded.findings.push(Finding::new(
            &file,
            "$.id",
            "id_mismatch",
            format!("must equal the directory `{expected_id}`"),
        ));
    }
    check_plugin_files(root, dir, &file, &doc, &mut loaded.findings)?;
    loaded.plugins.push(PluginFile { file, doc });
    Ok(())
}

/// Images exist and obey the rules; no other file sits in the plugin directory.
fn check_plugin_files(
    root: &Path,
    dir: &str,
    file: &str,
    doc: &PluginDoc,
    findings: &mut Vec<Finding>,
) -> Result<(), CliError> {
    let mut referenced: BTreeSet<&str> = BTreeSet::from(["plugin.json"]);
    let mut images: Vec<(String, &str, ImageRole)> = vec![("$.icon".to_owned(), doc.icon.as_str(), ImageRole::Icon)];
    for (i, s) in doc.screenshots.iter().enumerate() {
        images.push((
            format!("$.screenshots[{i}].path"),
            s.path.as_str(),
            ImageRole::Screenshot,
        ));
    }
    for (json_path, rel, role) in &images {
        referenced.insert(rel);
        let max = match role {
            ImageRole::Icon => limits::MAX_ICON_BYTES,
            ImageRole::Screenshot => limits::MAX_SCREENSHOT_BYTES,
        };
        match read_bounded(&root.join(dir).join(rel), max) {
            Err(_) if !root.join(dir).join(rel).is_file() => {
                findings.push(Finding::new(file, json_path, "image", "file not found"));
            }
            Err(e) => findings.push(Finding::new(file, json_path, "image", e.0)),
            Ok(bytes) => {
                if let Err(e) = check_image(*role, Some(rel), &bytes) {
                    findings.push(Finding::new(file, json_path, "image", e.to_string()));
                }
            }
        }
    }
    let mut present: Vec<String> = Vec::new();
    for (name, kind) in entries(&root.join(dir))? {
        match kind {
            Kind::File => present.push(name),
            Kind::Dir if name == "screenshots" => {
                for (inner, k) in entries(&root.join(dir).join("screenshots"))? {
                    if k == Kind::File {
                        present.push(format!("screenshots/{inner}"));
                    } else {
                        findings.push(Finding::new(
                            &format!("{dir}/screenshots/{inner}"),
                            "$",
                            "unexpected_file",
                            "only image files belong in screenshots/",
                        ));
                    }
                }
            }
            Kind::Dir | Kind::Other => findings.push(Finding::new(
                &format!("{dir}/{name}"),
                "$",
                "unexpected_file",
                "only plugin.json, the icon and screenshots/ belong in a plugin directory",
            )),
        }
    }
    for p in present {
        if !referenced.contains(p.as_str()) {
            findings.push(Finding::new(
                &format!("{dir}/{p}"),
                "$",
                "unreferenced_file",
                "not referenced by plugin.json",
            ));
        }
    }
    Ok(())
}

/// The plugin order of `index.json`: the committed order, then new plugins by id.
pub fn plugin_order(loaded: &Loaded, committed: Option<&IndexDoc>) -> Vec<usize> {
    let by_id: BTreeMap<&str, usize> = loaded
        .plugins
        .iter()
        .enumerate()
        .map(|(i, p)| (p.doc.id.as_str(), i))
        .collect();
    let mut order = Vec::new();
    let mut taken = BTreeSet::new();
    if let Some(doc) = committed {
        for p in &doc.plugins {
            if let Some(&i) = by_id.get(p.id.as_str())
                && taken.insert(i)
            {
                order.push(i);
            }
        }
    }
    order.extend(by_id.values().copied().filter(|i| !taken.contains(i)));
    order
}

/// Builds the index document from the loaded documents.
pub fn assemble(loaded: &Loaded, meta: &Meta, order: &[usize], generated_at: u64) -> IndexDoc {
    IndexDoc {
        schema: crate::INDEX_SCHEMA.to_owned(),
        generated_at,
        title: meta.title.clone(),
        description: meta.description.clone(),
        metadata: meta.metadata.clone(),
        publishers: loaded
            .publishers
            .iter()
            .map(|(_, p)| (p.id.clone(), p.clone()))
            .collect(),
        plugins: order
            .iter()
            .filter_map(|i| loaded.plugins.get(*i))
            .map(|p| p.doc.clone())
            .collect(),
    }
}

/// The exact bytes of `index.json`: two-space JSON and a final newline.
pub fn render_index(doc: &IndexDoc) -> Result<Vec<u8>, CliError> {
    let mut bytes = serde_json::to_vec_pretty(doc)?;
    bytes.push(b'\n');
    Ok(bytes)
}
