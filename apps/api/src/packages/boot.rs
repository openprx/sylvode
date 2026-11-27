//! Boot-time sync of the built-in publisher and the built-in packages (contract C3 section 4.2).
//!
//! [`sync_builtin`] makes the set of active `sv` keys equal to the pinned or configured key,
//! revokes every other `sv` key, then verifies and stores every `*.svpkg` of the configured
//! directory as tier `builtin` and re-verifies the stored built-in packages from their blobs. Any
//! failure is returned, and the caller stops the start.

use std::path::Path;

use anyhow::{Context, anyhow};
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use platform::config::PackagesConfig;
use sea_orm::{DatabaseConnection, TransactionTrait};

use super::{
    keys,
    store::{self, NewPackage},
    verify::{KeyScope, verify_blob},
};
use crate::error::ApiError;

/// The release public key of publisher `sv`, pinned when the binary is built with
/// `SV_RELEASE_PACKAGE_KEY` set (base64url, no padding). A binary with a pinned key ignores
/// `[packages] builtin_public_key`.
const PINNED_RELEASE_KEY: Option<&str> = option_env!("SV_RELEASE_PACKAGE_KEY");

fn api_err(e: ApiError) -> anyhow::Error {
    anyhow!("{e:?}")
}

/// The effective built-in public key: the pinned one when the build has it, else the configuration's.
fn effective_key(cfg: &PackagesConfig) -> anyhow::Result<Option<(String, Vec<u8>)>> {
    let text = PINNED_RELEASE_KEY.map(str::trim).or(cfg.builtin_public_key.as_deref());
    let Some(text) = text else { return Ok(None) };
    let public = URL_SAFE_NO_PAD
        .decode(text)
        .ok()
        .filter(|k| k.len() == 32)
        .ok_or_else(|| anyhow!("the built-in package key is not a 32-byte base64url public key"))?;
    Ok(Some((sv_package::key_id(&public), public)))
}

/// Runs the whole sync.
///
/// # Errors
/// The key is malformed, a built-in package file is unreadable or does not verify, or the
/// database fails.
pub async fn sync_builtin(db: &DatabaseConnection, cfg: &PackagesConfig) -> anyhow::Result<()> {
    let key = effective_key(cfg)?;
    keys::sync_builtin_key(db, key.as_ref().map(|(id, public)| (id.as_str(), public.as_slice())))
        .await
        .map_err(api_err)?;
    if let Some(dir) = &cfg.builtin_dir {
        store_directory(db, dir).await?;
    }
    Ok(())
}

async fn store_directory(db: &DatabaseConnection, dir: &Path) -> anyhow::Result<()> {
    let mut files = Vec::new();
    let mut entries = tokio::fs::read_dir(dir)
        .await
        .with_context(|| format!("built-in package directory {} is unreadable", dir.display()))?;
    while let Some(entry) = entries.next_entry().await? {
        let path = entry.path();
        if path.extension().is_some_and(|e| e == "svpkg") && entry.file_type().await?.is_file() {
            files.push(path);
        }
    }
    files.sort();
    for path in files {
        let bytes = tokio::fs::read(&path)
            .await
            .with_context(|| format!("built-in package {} is unreadable", path.display()))?;
        let verified = verify_blob(db, bytes.clone(), KeyScope::Builtin)
            .await
            .map_err(|e| anyhow!("built-in package {} refused: {e:?}", path.display()))?;
        let package = &verified.package;
        let tx = db.begin().await?;
        if store::find_owned(&tx, None, &package.digest)
            .await
            .map_err(api_err)?
            .is_some()
        {
            continue;
        }
        if store::version_taken(&tx, None, &package.manifest.id, &package.manifest.version)
            .await
            .map_err(api_err)?
        {
            return Err(anyhow!(
                "built-in package {} {} is already stored with different content; bump its version",
                package.manifest.id,
                package.manifest.version
            ));
        }
        store::put_blob(&tx, &package.blob_digest, bytes)
            .await
            .map_err(api_err)?;
        let kind = serde_json::to_value(package.manifest.kind)?;
        let new = NewPackage {
            workspace_id: None,
            digest: &package.digest,
            blob_digest: &package.blob_digest,
            package_key: &package.manifest.id,
            version: &package.manifest.version,
            kind: kind.as_str().ok_or_else(|| anyhow!("package kind is not a string"))?,
            publisher_key_id: verified.key_row,
            manifest: &package.manifest_value,
            custom_code: verified.custom_code,
            tier: "builtin",
            created_by: None,
        };
        store::insert_package(&tx, &new).await.map_err(api_err)?;
        tx.commit().await?;
        tracing::info!(package = %package.manifest.id, version = %package.manifest.version, "built-in package stored");
    }
    Ok(())
}
