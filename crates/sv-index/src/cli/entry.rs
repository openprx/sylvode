//! `sv-index entry`: the version entry of a local package, and the `contents` projection shared with
//! `check`.

use sv_package::{Limits, Manifest};

use super::{CliError, EntryArgs, now_secs, repo};
use crate::{AnyHost, ContentLayout, ContentPage, Contents, VersionEntry, VersionStatus, download_url_problem, limits};

/// The display projection of a manifest (the same facts as the `analysis` of
/// `GET .../packages/{digest}`). `forms` lists the binds that create a form from a package file;
/// `custom_code` is the manifest's computed flag (any action or rule).
pub fn contents_of(m: &Manifest) -> Contents {
    Contents {
        requests: m.requests.iter().map(|r| r.id.clone()).collect(),
        custom_code: m.computed_custom_code(),
        actions: m.actions.iter().map(|a| a.name.clone()).collect(),
        rules: m.rules.iter().map(|r| r.name.clone()).collect(),
        binds: m.binds.keys().cloned().collect(),
        layouts: m
            .layouts
            .iter()
            .map(|l| ContentLayout {
                key: l.key.clone(),
                slots: l.slots.clone(),
            })
            .collect(),
        pages: m
            .pages
            .iter()
            .map(|p| ContentPage {
                route: p.route.clone(),
                audience: p.audience.clone(),
            })
            .collect(),
        forms: m
            .binds
            .iter()
            .filter(|(_, b)| b.create_from.is_some())
            .map(|(name, _)| name.clone())
            .collect(),
        project_block: m.project.is_some(),
    }
}

/// Lower-case hex SHA-256 of `bytes`.
pub fn sha256_hex(bytes: &[u8]) -> String {
    let digest = sv_package::sha256_digest(bytes);
    digest.strip_prefix("sha256:").unwrap_or(&digest).to_owned()
}

pub fn run(args: &EntryArgs) -> Result<bool, CliError> {
    if let Some(why) = download_url_problem(&args.url, &AnyHost) {
        return Err(CliError(format!("--url: {why}")));
    }
    let bytes = repo::read_bounded(&args.package, limits::MAX_PACKAGE_BYTES)?;
    let pkg = sv_package::unpack(&bytes, &Limits::default()).map_err(|e| CliError(e.to_string()))?;
    let m = &pkg.manifest;
    let entry = VersionEntry {
        version: m.version.clone(),
        published_at: args.published_at.unwrap_or_else(now_secs),
        status: VersionStatus::Listed,
        status_reason: None,
        url: args.url.clone(),
        size: bytes.len() as u64,
        sha256: sha256_hex(&bytes),
        digest: pkg.digest.clone(),
        publisher: m.publisher.id.clone(),
        key_id: m.publisher.key_id.clone(),
        sylvode: args.sylvode.clone().unwrap_or_else(|| format!(">={}", m.sv.min)),
        contents: contents_of(m),
        changelog: None,
    };
    println!("{}", serde_json::to_string_pretty(&entry)?);
    Ok(true)
}
