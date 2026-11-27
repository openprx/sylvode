//! Reading a stored package for the install flow (contract C3 §5.3).
//!
//! The install routes never trust the stored manifest column: [`open`] reads the blob again and
//! repeats the whole verification (structure, signature against an active key, file digests,
//! layouts), so what a review shows and what an approval creates come from the verified bytes.

use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, LazyLock};

use parking_lot::Mutex;
use sea_orm::{ConnectionTrait, DbBackend, FromQueryResult, Statement};
use sv_package::VerifiedPackage;
use uuid::Uuid;

use super::{
    store,
    verify::{KeyScope, verify_blob},
};
use crate::error::ApiError;

/// A stored package row with what the install flow needs to decide before it opens the blob.
#[derive(Debug, Clone, FromQueryResult)]
pub struct StoredPackage {
    pub id: Uuid,
    pub digest: String,
    pub blob_digest: String,
    pub package_key: String,
    pub version: String,
    pub kind: String,
    pub tier: String,
    pub custom_code: bool,
    pub installable: bool,
    /// Status of the signing key: `active` or `revoked`.
    pub key_status: String,
    pub publisher_key_id: Uuid,
}

/// A package whose verification was repeated just now.
pub struct Opened {
    pub package: VerifiedPackage,
    /// Computed by the server: the manifest has actions or rules, or a layout has a `custom-frame`.
    pub custom_code: bool,
}

const COLUMNS: &str = "p.id, p.digest, p.blob_digest, p.package_key, p.version, p.kind, p.tier, p.custom_code, \
                       p.installable, k.status AS key_status, p.publisher_key_id";

/// The package of this digest that `workspace_id` may see: built-in, or private to the workspace.
///
/// # Errors
/// A database failure.
pub async fn find(
    db: &impl ConnectionTrait,
    workspace_id: Uuid,
    digest: &str,
) -> Result<Option<StoredPackage>, ApiError> {
    Ok(StoredPackage::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        format!(
            "SELECT {COLUMNS} FROM packages p JOIN publisher_keys k ON k.id = p.publisher_key_id \
             WHERE p.digest = $1 AND (p.tier = 'builtin' OR p.workspace_id = $2) \
             ORDER BY (p.tier = 'builtin') DESC LIMIT 1"
        ),
        vec![digest.into(), workspace_id.into()],
    ))
    .one(db)
    .await?)
}

/// A package row by its id, if the workspace may see it.
///
/// # Errors
/// A database failure.
pub async fn find_by_row(
    db: &impl ConnectionTrait,
    workspace_id: Uuid,
    row_id: Uuid,
) -> Result<Option<StoredPackage>, ApiError> {
    Ok(StoredPackage::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        format!(
            "SELECT {COLUMNS} FROM packages p JOIN publisher_keys k ON k.id = p.publisher_key_id \
             WHERE p.id = $1 AND (p.tier = 'builtin' OR p.workspace_id = $2)"
        ),
        vec![row_id.into(), workspace_id.into()],
    ))
    .one(db)
    .await?)
}

/// Reads the blob and verifies it again.
///
/// # Errors
/// `package_invalid` when the blob no longer verifies (including a revoked key), or a database
/// failure.
pub async fn open(
    db: &sea_orm::DatabaseConnection,
    workspace_id: Uuid,
    stored: &StoredPackage,
) -> Result<Opened, ApiError> {
    let bytes = store::blob_bytes(db, &stored.blob_digest)
        .await?
        .ok_or_else(ApiError::internal)?;
    let scope = if stored.tier == "builtin" {
        KeyScope::Builtin
    } else {
        KeyScope::Workspace(workspace_id)
    };
    let verified = verify_blob(db, bytes, scope).await?;
    Ok(Opened {
        package: verified.package,
        custom_code: verified.custom_code,
    })
}

/// Component bytes of packages in use, by package digest and path. A package is immutable under its
/// digest, so an entry never goes stale; a revoked key disables the installations of its packages
/// instead (contract C3-7).
struct Components {
    by_key: HashMap<(String, String), Arc<Vec<u8>>>,
    order: VecDeque<(String, String)>,
    bytes: usize,
}

const COMPONENT_CACHE_BYTES: usize = 64 * 1024 * 1024;

static COMPONENTS: LazyLock<Mutex<Components>> = LazyLock::new(|| {
    Mutex::new(Components {
        by_key: HashMap::new(),
        order: VecDeque::new(),
        bytes: 0,
    })
});

/// The bytes of one component file of a stored package. Read through [`open`] on a miss, so a
/// package that does not verify is never run.
///
/// # Errors
/// As [`open`], and `package_invalid` when the manifest's component is not in the package.
pub async fn component_bytes(
    db: &sea_orm::DatabaseConnection,
    workspace_id: Uuid,
    stored: &StoredPackage,
    path: &str,
) -> Result<Arc<Vec<u8>>, ApiError> {
    let key = (stored.digest.clone(), path.to_string());
    if let Some(hit) = COMPONENTS.lock().by_key.get(&key) {
        return Ok(Arc::clone(hit));
    }
    let opened = open(db, workspace_id, stored).await?;
    let bytes = Arc::new(
        opened
            .package
            .files
            .get(path)
            .cloned()
            .ok_or_else(|| ApiError::bad_request_coded("package_invalid", "the component is not in the package"))?,
    );
    let mut cache = COMPONENTS.lock();
    if !cache.by_key.contains_key(&key) {
        cache.bytes = cache.bytes.saturating_add(bytes.len());
        cache.by_key.insert(key.clone(), Arc::clone(&bytes));
        cache.order.push_back(key);
        while cache.bytes > COMPONENT_CACHE_BYTES && cache.order.len() > 1 {
            let Some(old) = cache.order.pop_front() else {
                break;
            };
            if let Some(gone) = cache.by_key.remove(&old) {
                cache.bytes = cache.bytes.saturating_sub(gone.len());
            }
        }
    }
    drop(cache);
    Ok(bytes)
}
