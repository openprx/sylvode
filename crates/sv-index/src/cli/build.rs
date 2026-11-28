//! `sv-index build`: write `index.json` from the documents of a checkout.

use std::fs;

use serde_json::json;

use super::repo::{self, Meta};
use super::report::Report;
use super::{BuildArgs, CliError, check, now_secs};
use crate::{AnyHost, limits};

pub fn run(args: &BuildArgs) -> Result<bool, CliError> {
    let root = &args.repo;
    let committed = repo::read_committed(root)?;
    let meta: Meta = match (&args.meta, committed.as_ref().and_then(|c| c.doc.as_ref())) {
        (Some(path), _) => serde_json::from_slice(&repo::read_bounded(path, limits::MAX_REPO_FILE_BYTES)?)
            .map_err(|e| CliError(format!("--meta {}: {e}", path.display())))?,
        (None, Some(doc)) => Meta {
            title: doc.title.clone(),
            description: doc.description.clone(),
            metadata: doc.metadata.clone(),
        },
        (None, None) => {
            return Err(CliError(
                "no readable index.json to take the title and metadata from; pass --meta <file>".to_owned(),
            ));
        }
    };

    let loaded = repo::load(root, Some(&meta.metadata), &AnyHost)?;
    let mut report = Report {
        publishers: loaded.publishers.len(),
        plugins: loaded.plugins.len(),
        versions: loaded.plugins.iter().map(|p| p.doc.versions.len()).sum(),
        findings: loaded.findings.clone(),
        ..Report::default()
    };
    let previous = committed.as_ref().and_then(|c| c.doc.as_ref());
    let order = repo::plugin_order(&loaded, previous);
    let previous_ts = previous.map_or(0, |d| d.generated_at);
    if !loaded.broken {
        let doc = repo::assemble(&loaded, &meta, &order, previous_ts);
        check::cross_findings(&doc, &loaded, &order, &AnyHost, &mut report.findings);
    }
    if !report.ok() {
        finish(args, &report, false, false)?;
        return Ok(false);
    }

    let same = repo::assemble(&loaded, &meta, &order, previous_ts);
    let same_bytes = repo::render_index(&same)?;
    if committed.as_ref().is_some_and(|c| c.bytes == same_bytes) {
        finish(args, &report, false, true)?;
        return Ok(true);
    }
    let fresh = repo::assemble(&loaded, &meta, &order, args.generated_at.unwrap_or_else(now_secs));
    let bytes = repo::render_index(&fresh)?;
    if bytes.len() as u64 > limits::MAX_INDEX_BYTES {
        return Err(CliError(format!(
            "index.json would be {} bytes, at most {} allowed",
            bytes.len(),
            limits::MAX_INDEX_BYTES
        )));
    }
    let tmp = root.join("index.json.tmp");
    fs::write(&tmp, &bytes).map_err(|e| CliError(format!("cannot write {}: {e}", tmp.display())))?;
    fs::rename(&tmp, root.join("index.json")).map_err(|e| CliError(format!("cannot replace index.json: {e}")))?;
    finish(args, &report, true, true)?;
    Ok(true)
}

fn finish(args: &BuildArgs, report: &Report, written: bool, ok: bool) -> Result<(), CliError> {
    if args.json {
        let mut v = report.to_json();
        if let Some(o) = v.as_object_mut() {
            o.insert("written".to_owned(), json!(written));
            o.insert("ok".to_owned(), json!(ok));
        }
        println!("{}", serde_json::to_string_pretty(&v)?);
        return Ok(());
    }
    if !ok {
        print!("{}", report.render("build"));
        println!("sv-index build: refused, index.json not written");
    } else if written {
        println!(
            "sv-index build: wrote index.json ({} publishers, {} plugins, {} versions)",
            report.publishers, report.plugins, report.versions
        );
    } else {
        println!("sv-index build: index.json is up to date");
    }
    Ok(())
}
