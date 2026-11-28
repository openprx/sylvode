//! Importing a package from a public plugin index (contract M15, section 3, decisions O4 and O5).
//!
//! `POST /api/v1/workspaces/{w}/market/index/import` downloads the package of one index entry and
//! stores it as a private package of the workspace; the normal preview, request and approve review
//! follows unchanged. The route is for people with a console session: it sits behind the JWT
//! middleware, which no bot or delegated token passes.
//!
//! The index is not signed, so nothing in it is believed on its own word:
//!
//! 1. the version must be `listed` and its `sylvode` range must admit this build;
//! 2. the signing key must be pinned in the workspace. A key that is not pinned is pinned only by a
//!    workspace owner who names it in `trust_key_id` (an admin gets `publisher_trust_owner_only`),
//!    and only inside the transaction that stores a package that verifies with it;
//! 3. the download goes through [`super::download`] (https, at most 3 redirects, each hop checked,
//!    bounded by the declared size);
//! 4. the file must have the declared `size` and `sha256`;
//! 5. the signature is verified with the workspace's pinned key (`verify_blob`);
//! 6. the manifest must agree with the index entry (id, version, kind, publisher, key, package
//!    digest) and its own `sv.min` must admit this build (the range in the index can overstate a
//!    pre-release bound, so the manifest is checked here as well).
//!
//! Every refusal after the source is resolved writes an `import_refused` event; a stored package
//! writes `import`, a newly pinned key `trust_key`.

use std::collections::HashSet;
use std::sync::LazyLock;

use axum::{
    Extension, Json,
    extract::{Path, State},
    response::IntoResponse,
};
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use parking_lot::Mutex;
use platform::{app::AppState, auth::JwtClaims, config::IndexSource, timestamp::Timestamp};
use sea_orm::TransactionTrait;
use semver::Version;
use serde::Deserialize;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use sv_index::{KeyStatus, PluginDoc, PublisherKey, VersionEntry, VersionStatus};
use uuid::Uuid;

use super::{
    cache,
    download::{DownloadError, download},
    handlers::{coded, is_compatible, kind_name, plugin_not_found, plugin_publisher, running_version, visible_source},
    store,
};
use crate::{
    error::ApiError,
    packages::{
        access::Caller,
        handlers::PreparedUpload,
        store::{PackageSummary, find_owned},
    },
    response::ApiResponse,
};

/// The body of the import route.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ImportRequest {
    /// The configured source the plugin comes from.
    source: String,
    /// `<publisher>/<name>`.
    plugin_id: String,
    /// The exact version to import.
    version: String,
    /// The `key_id` of the signing key the workspace owner trusts, when it is not pinned yet.
    trust_key_id: Option<String>,
}

/// Workspaces with an import in flight in this process (one at a time per workspace). With several
/// API processes two imports of one workspace can overlap; the upload lock of the store step
/// serialises their writes, so the only cost is a second download.
static IN_FLIGHT: LazyLock<Mutex<HashSet<Uuid>>> = LazyLock::new(|| Mutex::new(HashSet::new()));

struct ImportSlot(Uuid);

impl ImportSlot {
    fn begin(workspace_id: Uuid) -> Result<Self, ApiError> {
        if IN_FLIGHT.lock().insert(workspace_id) {
            Ok(Self(workspace_id))
        } else {
            Err(coded(
                409,
                "index_import_in_progress",
                "another import of this workspace is still running",
            ))
        }
    }
}

impl Drop for ImportSlot {
    fn drop(&mut self) {
        IN_FLIGHT.lock().remove(&self.0);
    }
}

fn request_invalid(message: &str) -> ApiError {
    ApiError::bad_request_coded("import_request_invalid", message)
}

fn is_key_id(raw: &str) -> bool {
    raw.strip_prefix("ed25519:")
        .is_some_and(|hex| hex.len() == 32 && hex.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)))
}

fn check_request(body: &ImportRequest) -> Result<(), ApiError> {
    if sv_index::rules::plugin_id_publisher(&body.plugin_id).is_none() {
        return Err(request_invalid("plugin_id must be <publisher>/<name>"));
    }
    if sv_index::parse_exact_version(&body.version).is_err() {
        return Err(request_invalid("version must be an exact semver version"));
    }
    if body.trust_key_id.as_deref().is_some_and(|key| !is_key_id(key)) {
        return Err(request_invalid("trust_key_id must be a key id (ed25519:<32 hex>)"));
    }
    Ok(())
}

fn not_importable(reason: &'static str) -> ApiError {
    not_importable_with(reason, json!({}))
}

fn not_importable_with(reason: &'static str, extra: Value) -> ApiError {
    let mut details = json!({ "reason": reason });
    if let (Some(details), Value::Object(extra)) = (details.as_object_mut(), extra) {
        details.extend(extra);
    }
    ApiError::Coded {
        status: 409,
        code: "index_version_not_importable",
        message: "this version cannot be imported".to_string(),
        details: Some(details),
    }
}

fn mismatch(field: &'static str) -> ApiError {
    ApiError::Coded {
        status: 400,
        code: "index_mismatch",
        message: "the package does not match the index entry".to_string(),
        details: Some(json!({ "field": field })),
    }
}

fn key_revoked() -> ApiError {
    coded(
        409,
        "index_key_revoked",
        "the signing key of this version is revoked, in the index or in this workspace",
    )
}

fn download_failed(error: &DownloadError) -> ApiError {
    let mut details = json!({ "reason": error.reason() });
    if let (Some(status), Some(object)) = (error.status(), details.as_object_mut()) {
        object.insert("status".into(), json!(status));
    }
    ApiError::Coded {
        status: 502,
        code: "index_download_failed",
        message: "the package could not be downloaded".to_string(),
        details: Some(details),
    }
}

/// `{source, plugin_id, publisher, name, version, digest, url, sha256, imported_by, imported_at}`.
fn origin_json(source: &IndexSource, plugin: &PluginDoc, entry: &VersionEntry, user_id: Uuid) -> Value {
    let name = plugin.id.split_once('/').map_or("", |(_, name)| name);
    json!({
        "source": source.id,
        "plugin_id": plugin.id,
        "publisher": entry.publisher,
        "name": name,
        "version": entry.version,
        "digest": entry.digest,
        "url": entry.url,
        "sha256": entry.sha256,
        "imported_by": user_id,
        "imported_at": Timestamp::now(),
    })
}

/// The signing key of an entry as the index lists it: active, and its public key must hash to its
/// `key_id` (the index validator checks this too; the import does not rely on it).
fn index_key<'a>(
    publisher: Option<&'a sv_index::PublisherDoc>,
    entry: &VersionEntry,
) -> Result<(&'a PublisherKey, Vec<u8>), ApiError> {
    let key = publisher
        .and_then(|publisher| publisher.keys.iter().find(|key| key.key_id == entry.key_id))
        .ok_or_else(|| mismatch("key_id"))?;
    if key.status != KeyStatus::Active {
        return Err(key_revoked());
    }
    let public = URL_SAFE_NO_PAD
        .decode(key.public_key.as_bytes())
        .ok()
        .filter(|public| public.len() == 32 && sv_package::key_id(public) == key.key_id)
        .ok_or_else(|| mismatch("public_key"))?;
    Ok((key, public))
}

/// `POST /market/index/import`: downloads, verifies and stores one version of an indexed plugin.
///
/// Workspace owner or admin, console session only. Answers the package summary with its
/// `origin`; a version the workspace already holds answers the existing package.
pub async fn import_package(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    Path(workspace_id): Path<Uuid>,
    Json(body): Json<ImportRequest>,
) -> Result<impl IntoResponse, ApiError> {
    let caller = Caller::resolve(&state, &claims, None, workspace_id).await?;
    caller.require_admin()?;
    check_request(&body)?;
    let _slot = ImportSlot::begin(workspace_id)?;
    let source = visible_source(&state, workspace_id, &body.source).await?;
    match perform(&state, &caller, workspace_id, &source, &body).await {
        Ok(summary) => Ok(ApiResponse::success(summary)),
        Err(error) => {
            record_refusal(&state, &caller, workspace_id, &source, &body, &error).await;
            Err(error)
        }
    }
}

/// Writes the `import_refused` event of a coded refusal. A failed write is logged; the refusal is
/// what the caller gets either way.
async fn record_refusal(
    state: &AppState,
    caller: &Caller,
    workspace_id: Uuid,
    source: &IndexSource,
    body: &ImportRequest,
    error: &ApiError,
) {
    let ApiError::Coded { code, details, .. } = error else {
        return;
    };
    let reason = details
        .as_ref()
        .and_then(|details| details.get("reason").or_else(|| details.get("field")))
        .and_then(Value::as_str);
    let event = json!({
        "plugin_id": body.plugin_id,
        "version": body.version,
        "code": code,
        "reason": reason,
    });
    if let Err(write) = store::insert_event_by(
        &state.db,
        Some(workspace_id),
        &source.id,
        "import_refused",
        Some(caller.user_id),
        event,
    )
    .await
    {
        tracing::warn!(%workspace_id, error = %write, "a refused plugin import was not recorded");
    }
}

/// Whether the signing key has to be pinned by this import.
enum KeyState {
    Pinned,
    PinNow,
}

/// What the workspace holds for the entry's key, and what the caller may do about it.
async fn check_key(
    state: &AppState,
    caller: &Caller,
    workspace_id: Uuid,
    body: &ImportRequest,
    entry: &VersionEntry,
    publisher_name: Option<&sv_index::I18nText>,
) -> Result<KeyState, ApiError> {
    match store::workspace_key(&state.db, workspace_id, &entry.key_id).await? {
        Some(row) if row.publisher != entry.publisher => Err(coded(
            409,
            "index_key_conflict",
            "this workspace registered the key for another publisher",
        )),
        Some(row) if row.status != "active" => Err(key_revoked()),
        Some(_) => Ok(KeyState::Pinned),
        None => {
            let pinned = store::active_key_ids(&state.db, workspace_id, &entry.publisher).await?;
            let details = json!({
                "publisher": entry.publisher,
                "publisher_name": publisher_name,
                "key_id": entry.key_id,
                "fingerprint": entry.key_id,
                // A new key of a publisher the workspace already trusts is a key change.
                "key_change": !pinned.is_empty(),
                "pinned_key_ids": pinned,
            });
            match body.trust_key_id.as_deref() {
                None => Err(ApiError::Coded {
                    status: 409,
                    code: "publisher_untrusted",
                    message: "the signing key of this package is not trusted in this workspace".to_string(),
                    details: Some(details),
                }),
                Some(named) if named != entry.key_id => {
                    Err(request_invalid("trust_key_id is not the signing key of this version"))
                }
                Some(_) if !caller.is_owner() => Err(ApiError::Coded {
                    status: 403,
                    code: "publisher_trust_owner_only",
                    message: "only the workspace owner can trust a new publisher key".to_string(),
                    details: Some(details),
                }),
                Some(_) => Ok(KeyState::PinNow),
            }
        }
    }
}

/// The bytes with their lower-case hex SHA-256, hashed off the async threads.
async fn sha256_hex(bytes: Vec<u8>) -> Result<(Vec<u8>, String), ApiError> {
    tokio::task::spawn_blocking(move || {
        let digest = hex::encode(Sha256::digest(&bytes));
        (bytes, digest)
    })
    .await
    .map_err(|_| ApiError::internal())
}

async fn perform(
    state: &AppState,
    caller: &Caller,
    workspace_id: Uuid,
    source: &IndexSource,
    body: &ImportRequest,
) -> Result<PackageSummary, ApiError> {
    let doc = cache::load(&state.db, source).await?.ok_or_else(plugin_not_found)?;
    let plugin = doc
        .plugins
        .iter()
        .find(|plugin| plugin.id == body.plugin_id)
        .ok_or_else(plugin_not_found)?;
    let entry = plugin
        .versions
        .iter()
        .find(|version| version.version == body.version)
        .ok_or_else(|| not_importable("version_unknown"))?;
    match entry.status {
        VersionStatus::Listed => {}
        VersionStatus::Yanked => return Err(not_importable("yanked")),
        VersionStatus::Revoked => return Err(not_importable("revoked")),
    }
    if !is_compatible(entry) {
        return Err(not_importable_with("incompatible", json!({ "sylvode": entry.sylvode })));
    }
    if entry.publisher != plugin_publisher(&plugin.id) {
        return Err(mismatch("publisher"));
    }
    let publisher = doc.publishers.get(&entry.publisher);
    let (_, public_key) = index_key(publisher, entry)?;

    // A version the workspace already holds is answered as it is.
    if let Some(row) = find_owned(&state.db, Some(workspace_id), &entry.digest).await? {
        return Ok(PackageSummary::from(&row));
    }

    let key_state = check_key(
        state,
        caller,
        workspace_id,
        body,
        entry,
        publisher.map(|publisher| &publisher.name),
    )
    .await?;

    let hosts = &crate::config::runtime().packages.index_download_hosts;
    let bytes = download(&entry.url, entry.size, hosts)
        .await
        .map_err(|error| download_failed(&error))?;
    if bytes.len() as u64 != entry.size {
        return Err(ApiError::Coded {
            status: 400,
            code: "index_size_mismatch",
            message: "the downloaded file does not have the size the index declares".to_string(),
            details: Some(json!({ "declared": entry.size, "actual": bytes.len() })),
        });
    }
    let (bytes, sha256) = sha256_hex(bytes).await?;
    if !sha256.eq_ignore_ascii_case(&entry.sha256) {
        return Err(coded(
            400,
            "index_sha256_mismatch",
            "the downloaded file does not have the sha256 the index declares",
        ));
    }

    // Pin the key, verify with it and store, in one transaction: a package that does not verify
    // or does not match the index leaves no trusted key behind.
    let tx = state.db.begin().await?;
    let mut pinned_now = false;
    if matches!(key_state, KeyState::PinNow) {
        pinned_now = store::pin_key(
            &tx,
            workspace_id,
            &entry.publisher,
            &entry.key_id,
            &public_key,
            caller.user_id,
            &format!("index:{}", source.id),
        )
        .await?;
        if pinned_now {
            store::insert_event_by(
                &tx,
                Some(workspace_id),
                &source.id,
                "trust_key",
                Some(caller.user_id),
                json!({ "publisher": entry.publisher, "key_id": entry.key_id }),
            )
            .await?;
        } else {
            // The key was registered between the check and now: it must be this publisher's, active.
            match store::workspace_key(&tx, workspace_id, &entry.key_id).await? {
                Some(row) if row.publisher == entry.publisher && row.status == "active" => {}
                Some(row) if row.publisher == entry.publisher => return Err(key_revoked()),
                _ => {
                    return Err(coded(
                        409,
                        "index_key_conflict",
                        "this workspace registered the key for another publisher",
                    ));
                }
            }
        }
    }
    let prepared = PreparedUpload::verify(&tx, workspace_id, bytes).await?;
    let info = prepared.info();
    if info.package_key != plugin.id {
        return Err(mismatch("id"));
    }
    if info.version != entry.version {
        return Err(mismatch("version"));
    }
    if info.kind != kind_name(plugin.kind) {
        return Err(mismatch("kind"));
    }
    if info.publisher.get("id").and_then(Value::as_str) != Some(entry.publisher.as_str()) {
        return Err(mismatch("publisher"));
    }
    if info.publisher.get("key_id").and_then(Value::as_str) != Some(entry.key_id.as_str()) {
        return Err(mismatch("key_id"));
    }
    if info.digest != entry.digest {
        return Err(mismatch("digest"));
    }
    ensure_sv_min(&info.sv_min)?;
    let mut summary = prepared.store(&tx, workspace_id, caller.user_id).await?;
    let origin =
        store::set_package_origin(&tx, summary.id, &origin_json(source, plugin, entry, caller.user_id)).await?;
    store::insert_event_by(
        &tx,
        Some(workspace_id),
        &source.id,
        "import",
        Some(caller.user_id),
        json!({
            "plugin_id": plugin.id,
            "version": entry.version,
            "digest": entry.digest,
            "sha256": entry.sha256,
            "size": entry.size,
            "key_id": entry.key_id,
            "package_id": summary.id,
            "key_pinned": pinned_now,
        }),
    )
    .await?;
    tx.commit().await?;
    summary.origin = Some(origin);
    tracing::info!(%workspace_id, package_id = %summary.id, source = %source.id, "package imported from the plugin index");
    Ok(summary)
}

/// The manifest's own `sv.min` against the running version (without a pre-release tag, as the
/// index range check does): the range in the index is display data and can overstate a
/// pre-release bound, so the package itself decides.
fn ensure_sv_min(sv_min: &str) -> Result<(), ApiError> {
    let min = Version::parse(sv_min).map_err(|_| mismatch("sv_min"))?;
    match running_version() {
        Some(running) if running >= min => Ok(()),
        _ => Err(not_importable_with("sv_min", json!({ "sv_min": sv_min }))),
    }
}
