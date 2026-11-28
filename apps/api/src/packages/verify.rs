//! Verification of package bytes: structure, signature against the registry, layout content.
//!
//! Content validation at upload covers layouts only. The C2 form validation of `create_from`
//! files and the C1 section 6 checks of components (WIT package, worlds, imports) run at
//! install time from the stored blob (L2.P2m); nothing installs or runs package content before.

use std::collections::BTreeSet;

use sea_orm::{ConnectionTrait, DbBackend, FromQueryResult, Statement};
use serde_json::{Value, json};
use sv_layout::{Context, Options};
use sv_package::{ErrorCode, KeyLookup, Limits, VerifiedPackage, has_custom_frame};
use tokio::sync::Semaphore;
use uuid::Uuid;

use crate::error::ApiError;

/// Concurrent verifications in the process: each holds up to the zip (32 MiB) plus its inflated
/// content (64 MiB) in memory.
static VERIFY_SLOTS: Semaphore = Semaphore::const_new(2);

/// Whose keys verify a package.
#[derive(Debug, Clone, Copy)]
pub(super) enum KeyScope {
    /// An upload: the uploading workspace's own keys.
    Workspace(Uuid),
    /// A built-in package: the configured `sv` keys.
    Builtin,
}

/// A package whose signature verified.
pub(super) struct Verified {
    pub(super) package: VerifiedPackage,
    /// `publisher_keys.id` of the key that signed it.
    pub(super) key_row: Uuid,
    /// Computed by the server, never read from the manifest.
    pub(super) custom_code: bool,
}

/// Why a package was refused: `package_invalid` with `details.reason`.
#[derive(Debug)]
pub(super) struct Refusal {
    reason: &'static str,
    /// For `signature_invalid`: `key_unknown` or `key_revoked`.
    cause: Option<&'static str>,
    message: String,
    errors: Option<Value>,
}

impl Refusal {
    fn new(reason: &'static str, message: impl Into<String>) -> Self {
        Self {
            reason,
            cause: None,
            message: message.into(),
            errors: None,
        }
    }

    pub(super) fn key_revoked() -> Self {
        let mut refusal = Self::new("signature_invalid", "the signing key was revoked");
        refusal.cause = Some("key_revoked");
        refusal
    }

    pub(super) fn too_large() -> Self {
        Self::new("too_large", "the upload is larger than the package limit")
    }
}

impl Verified {
    /// `[{path, size}]` of the content files, stored with the row so reads never open the blob.
    pub(super) fn files_json(&self) -> Value {
        Value::Array(
            self.package
                .files
                .iter()
                .map(|(path, data)| json!({"path": path, "size": data.len()}))
                .collect(),
        )
    }
}

impl From<sv_package::PackageError> for Refusal {
    fn from(e: sv_package::PackageError) -> Self {
        let (reason, cause) = match e.code {
            ErrorCode::TooLarge => ("too_large", None),
            ErrorCode::TooManyEntries => ("too_many_entries", None),
            ErrorCode::PathInvalid => ("path_invalid", None),
            ErrorCode::EntryType => ("entry_type", None),
            ErrorCode::FileUnlisted => ("file_unlisted", None),
            ErrorCode::FileMissing => ("file_missing", None),
            ErrorCode::DigestMismatch => ("digest_mismatch", None),
            ErrorCode::SignatureInvalid => ("signature_invalid", None),
            ErrorCode::KeyUnknown => ("signature_invalid", Some("key_unknown")),
            ErrorCode::KeyRevoked => ("signature_invalid", Some("key_revoked")),
            ErrorCode::KeyInvalid => ("signature_invalid", Some("key_invalid")),
            ErrorCode::ManifestInvalid => ("manifest_invalid", None),
            ErrorCode::ZipInvalid => ("zip_invalid", None),
        };
        Self {
            reason,
            cause,
            message: e.message,
            errors: None,
        }
    }
}

impl From<Refusal> for ApiError {
    fn from(r: Refusal) -> Self {
        let mut details = serde_json::Map::new();
        details.insert("reason".into(), json!(r.reason));
        details.insert("message".into(), json!(r.message));
        if let Some(cause) = r.cause {
            details.insert("cause".into(), json!(cause));
        }
        if let Some(errors) = r.errors {
            details.insert("errors".into(), errors);
        }
        let details = Value::Object(details);
        Self::Coded {
            status: 400,
            code: "package_invalid",
            message: "the package is not acceptable".to_string(),
            details: Some(details),
        }
    }
}

#[derive(FromQueryResult)]
struct KeyRow {
    id: Uuid,
    publisher: String,
    public_key: Vec<u8>,
    status: String,
}

async fn find_key(db: &impl ConnectionTrait, key_id: &str, scope: KeyScope) -> Result<Option<KeyRow>, ApiError> {
    let (sql, values) = match scope {
        KeyScope::Workspace(w) => (
            "SELECT id, publisher, public_key, status FROM publisher_keys WHERE key_id = $1 AND workspace_id = $2",
            vec![key_id.into(), w.into()],
        ),
        KeyScope::Builtin => (
            "SELECT id, publisher, public_key, status FROM publisher_keys WHERE key_id = $1 AND workspace_id IS NULL",
            vec![key_id.into()],
        ),
    };
    Ok(
        KeyRow::find_by_statement(Statement::from_sql_and_values(DbBackend::Postgres, sql, values))
            .one(db)
            .await?,
    )
}

/// Validates every layout file of the manifest with `sv-layout`; a layout used by a page and
/// placed in no slot is a site layout, every other one is a console layout.
fn check_layouts(package: &VerifiedPackage) -> Result<bool, Refusal> {
    let bindings: BTreeSet<String> = package.manifest.binds.keys().cloned().collect();
    let mut custom = false;
    for entry in &package.manifest.layouts {
        let site = entry.slots.is_empty() && package.manifest.pages.iter().any(|p| p.layout == entry.key);
        let opts = Options {
            slot: None,
            context: if site {
                Context::Site {
                    bindings: bindings.clone(),
                }
            } else {
                Context::Console
            },
        };
        let Some(bytes) = package.files.get(&entry.path) else {
            return Err(Refusal::new(
                "file_missing",
                format!("`{}` is listed but absent", entry.path),
            ));
        };
        if let Err(errors) = sv_layout::validate_bytes(bytes, &opts) {
            let mut refusal = Refusal::new("layout_invalid", format!("layout `{}` does not validate", entry.key));
            refusal.errors = serde_json::to_value(&errors).ok();
            return Err(refusal);
        }
        custom |= serde_json::from_slice::<Value>(bytes).is_ok_and(|v| has_custom_frame(&v));
    }
    Ok(custom)
}

/// Unpacks `bytes`, verifies the signature with the registry's key for `scope`, validates the
/// layouts and computes `custom_code`. Nothing is written to disk.
pub(super) async fn verify_blob(
    db: &impl ConnectionTrait,
    bytes: Vec<u8>,
    scope: KeyScope,
) -> Result<Verified, ApiError> {
    let _slot = VERIFY_SLOTS.acquire().await.map_err(|_| ApiError::internal())?;
    let unverified = tokio::task::spawn_blocking(move || sv_package::unpack(&bytes, &Limits::default()))
        .await
        .map_err(|_| ApiError::internal())?
        .map_err(Refusal::from)?;

    let manifest_publisher = unverified.manifest.publisher.id.clone();
    match scope {
        KeyScope::Workspace(_) if manifest_publisher == "sv" => {
            return Err(Refusal::new("publisher_reserved", "publisher `sv` is reserved").into());
        }
        KeyScope::Builtin if manifest_publisher != "sv" => {
            return Err(Refusal::new("publisher_reserved", "built-in packages are published by `sv`").into());
        }
        _ => {}
    }

    let key_id = unverified.manifest.publisher.key_id.clone();
    let row = find_key(db, &key_id, scope).await?;
    let lookup = match &row {
        None => KeyLookup::Unknown,
        Some(r) if r.publisher != manifest_publisher => KeyLookup::Unknown,
        Some(r) if r.status != "active" => KeyLookup::Revoked,
        Some(r) => KeyLookup::Active(r.public_key.clone()),
    };
    let package = unverified.verify(|_| lookup).map_err(Refusal::from)?;
    let key_row = row.map(|r| r.id).ok_or_else(ApiError::internal)?;
    let custom_frame = check_layouts(&package)?;
    check_project_type(db, &package).await?;
    let custom_code = package.manifest.computed_custom_code() || custom_frame;
    Ok(Verified {
        package,
        key_row,
        custom_code,
    })
}

/// The project type a package's `project` block names must exist (checked at upload and at boot).
async fn check_project_type(db: &impl ConnectionTrait, package: &VerifiedPackage) -> Result<(), ApiError> {
    let Some(type_key) = package
        .manifest
        .project
        .as_ref()
        .and_then(|project| project.type_key.as_ref())
    else {
        return Ok(());
    };
    let known = db
        .query_one(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT 1 FROM project_types WHERE key = $1",
            vec![type_key.clone().into()],
        ))
        .await?
        .is_some();
    if known {
        Ok(())
    } else {
        Err(Refusal::new("manifest_invalid", "project.type_key names no project type").into())
    }
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::panic, clippy::indexing_slicing)]
mod tests {
    use serde_json::json;
    use sv_package::{ErrorCode, PackageError};

    use super::{Refusal, has_custom_frame};
    use crate::error::ApiError;

    #[test]
    fn custom_frames_are_found_at_any_depth() {
        assert!(has_custom_frame(
            &json!({"root": {"children": [{"type": "custom-frame"}]}})
        ));
        assert!(!has_custom_frame(
            &json!({"root": {"type": "stack", "children": [{"type": "text"}]}})
        ));
    }

    #[test]
    fn key_problems_surface_as_signature_invalid_with_a_cause() {
        let refusal = Refusal::from(PackageError {
            code: ErrorCode::KeyRevoked,
            message: "revoked".into(),
        });
        match ApiError::from(refusal) {
            ApiError::Coded { code, details, .. } => {
                assert_eq!(code, "package_invalid");
                let d = details.expect("details");
                assert_eq!(d["reason"], "signature_invalid");
                assert_eq!(d["cause"], "key_revoked");
            }
            other => panic!("unexpected {other:?}"),
        }
    }

    #[test]
    fn the_sample_layout_of_the_qa_steps_validates() {
        let spec = br#"{"schema":"sv.layout.v1","key":"sample.summary","title":{"en":"Summary"},"params":{"record_id":{"type":"string","required":true}},"data":{"req":{"source":"record","form":"$bind.requests","record":"$params.record_id","fields":["title"]}},"root":{"type":"stack","props":{"gap":"md"},"children":[{"type":"stat","props":{"label":{"en":"Title"},"value":"$data.req.values.title"}}]}}"#;
        let result = sv_layout::validate_bytes(spec, &sv_layout::Options::default());
        assert!(result.is_ok(), "{result:?}");
    }
}
