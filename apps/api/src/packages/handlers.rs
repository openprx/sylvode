//! Handlers of the package routes: upload, list, get, and publisher key registration.

use axum::{
    Extension, Json,
    extract::{Multipart, Path, Query, State},
    response::IntoResponse,
};
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use platform::{app::AppState, auth::JwtClaims};
use sea_orm::TransactionTrait;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sv_package::{Kind, Limits, Manifest};
use uuid::Uuid;

use super::{
    access::Caller,
    keys::{self, PublisherKey},
    store::{self, NewPackage, PackageSummary},
    verify::{KeyScope, Refusal, verify_blob},
};
use crate::{
    error::ApiError,
    middleware::bot_auth::BotAuthContext,
    response::{ApiResponse, PageQuery, PaginatedData},
};

/// The package kinds a list may filter by.
const KINDS: [&str; 4] = ["form-template", "page-template", "app", "plugin"];

fn package_not_found() -> ApiError {
    ApiError::not_found("package not found")
}

fn is_digest(s: &str) -> bool {
    s.strip_prefix("sha256:")
        .is_some_and(|h| h.len() == 64 && h.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)))
}

const fn kind_name(kind: Kind) -> &'static str {
    match kind {
        Kind::FormTemplate => "form-template",
        Kind::PageTemplate => "page-template",
        Kind::App => "app",
        Kind::Plugin => "plugin",
    }
}

/// Reads the `file` part of the upload, never holding more than the zip limit.
async fn read_file_part(mut multipart: Multipart) -> Result<Vec<u8>, ApiError> {
    let limit = Limits::default().max_zip_bytes;
    let mut bytes: Option<Vec<u8>> = None;
    while let Some(mut field) = multipart
        .next_field()
        .await
        .map_err(|_| ApiError::bad_request_coded("multipart_invalid", "the upload is not valid multipart data"))?
    {
        if field.name() != Some("file") || bytes.is_some() {
            return Err(ApiError::bad_request_coded(
                "multipart_invalid",
                "send exactly one multipart part named `file`",
            ));
        }
        let mut data = Vec::new();
        while let Some(chunk) = field
            .chunk()
            .await
            .map_err(|_| ApiError::bad_request_coded("multipart_invalid", "the upload could not be read"))?
        {
            if (data.len() as u64).saturating_add(chunk.len() as u64) > limit {
                return Err(Refusal::too_large().into());
            }
            data.extend_from_slice(&chunk);
        }
        bytes = Some(data);
    }
    bytes
        .filter(|b| !b.is_empty())
        .ok_or_else(|| ApiError::bad_request_coded("multipart_invalid", "the part `file` is missing or empty"))
}

/// Most private packages one workspace may store.
const MAX_PACKAGES_PER_WORKSPACE: i64 = 200;
/// Most blob bytes one workspace may store across its private packages (256 MiB).
const MAX_PACKAGE_BYTES_PER_WORKSPACE: i64 = 256 * 1024 * 1024;

/// Refuses an upload that would take the workspace over its package count (`409`) or its stored
/// bytes (`413`). The workspace's uploads are serialised first, so two uploads cannot both pass.
async fn enforce_quota(
    db: &impl sea_orm::ConnectionTrait,
    workspace_id: Uuid,
    blob_digest: &str,
    size: usize,
) -> Result<(), ApiError> {
    store::lock_workspace_uploads(db, workspace_id).await?;
    let usage = store::workspace_usage(db, workspace_id).await?;
    if usage.packages >= MAX_PACKAGES_PER_WORKSPACE {
        return Err(ApiError::conflict_coded(
            "package_quota_exceeded",
            "this workspace stores the most packages it may; delete an unused package with the package delete route of the workspace",
            Some(json!({ "limit": MAX_PACKAGES_PER_WORKSPACE })),
        ));
    }
    let added = if store::workspace_has_blob(db, workspace_id, blob_digest).await? {
        0
    } else {
        i64::try_from(size).unwrap_or(i64::MAX)
    };
    if usage.bytes.saturating_add(added) > MAX_PACKAGE_BYTES_PER_WORKSPACE {
        return Err(ApiError::Coded {
            status: 413,
            code: "package_storage_exceeded",
            message: "this upload would take the workspace over its package storage limit".to_string(),
            details: Some(json!({ "limit_bytes": MAX_PACKAGE_BYTES_PER_WORKSPACE })),
        });
    }
    Ok(())
}

/// `POST /api/v1/workspaces/{w}/packages` (multipart `file`): verifies with the workspace's
/// registered publisher keys and stores the blob by digest.
pub async fn upload_package(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    Path(workspace_id): Path<Uuid>,
    multipart: Multipart,
) -> Result<impl IntoResponse, ApiError> {
    let caller = Caller::resolve(&state, &claims, None, workspace_id).await?;
    caller.require_admin()?;
    let bytes = read_file_part(multipart).await?;
    let verified = verify_blob(&state.db, bytes.clone(), KeyScope::Workspace(workspace_id)).await?;
    let package = &verified.package;

    let tx = state.db.begin().await?;
    if !keys::lock_active(&tx, verified.key_row).await? {
        return Err(Refusal::key_revoked().into());
    }
    let row = if let Some(existing) = store::find_owned(&tx, Some(workspace_id), &package.digest).await? {
        // The same package was uploaded before: the answer is the existing row.
        existing
    } else {
        if store::version_taken(&tx, Some(workspace_id), &package.manifest.id, &package.manifest.version).await? {
            return Err(ApiError::conflict_coded(
                "package_version_exists",
                "this workspace already has a different package with this id and version",
                None,
            ));
        }
        enforce_quota(&tx, workspace_id, &package.blob_digest, bytes.len()).await?;
        store::put_blob(&tx, &package.blob_digest, bytes).await?;
        let files = verified.files_json();
        let new = NewPackage {
            workspace_id: Some(workspace_id),
            digest: &package.digest,
            blob_digest: &package.blob_digest,
            package_key: &package.manifest.id,
            version: &package.manifest.version,
            kind: kind_name(package.manifest.kind),
            publisher_key_id: verified.key_row,
            manifest: &package.manifest_value,
            files: &files,
            custom_code: verified.custom_code,
            tier: "private",
            created_by: Some(caller.user_id),
        };
        let inserted = store::insert_package(&tx, &new).await.map_err(|e| {
            if e.database_sqlstate().as_deref() == Some("23505") {
                ApiError::conflict_coded(
                    "package_version_exists",
                    "this workspace already has a different package with this id and version",
                    None,
                )
            } else {
                e
            }
        })?;
        match inserted {
            Some(row) => row,
            None => store::find_owned(&tx, Some(workspace_id), &package.digest)
                .await?
                .ok_or_else(ApiError::internal)?,
        }
    };
    tx.commit().await?;
    tracing::info!(%workspace_id, package_id = %row.id, "package stored");
    Ok(ApiResponse::success(PackageSummary::from(&row)))
}

fn package_in_use() -> ApiError {
    ApiError::conflict_coded(
        "package_in_use",
        "an active or pending installation uses this package; revoke the installation first",
        None,
    )
}

/// A foreign key violation (`23503`) during the delete means a live generation appeared after the check.
fn map_in_use(error: ApiError) -> ApiError {
    if error.database_sqlstate().as_deref() == Some("23503") {
        package_in_use()
    } else {
        error
    }
}

/// `DELETE /api/v1/workspaces/{w}/packages/{digest}`: removes a stored private package.
///
/// Refused while a live installation generation (pending or approved, of an installation that is
/// not revoked) references the package, and for a built-in package. The package's non-live
/// generations go with it; the blob nothing else uses is cleaned after the commit. Serialised with
/// uploads by the workspace's upload lock.
pub async fn delete_package(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    Path((workspace_id, digest)): Path<(Uuid, String)>,
) -> Result<impl IntoResponse, ApiError> {
    let caller = Caller::resolve(&state, &claims, None, workspace_id).await?;
    caller.require_admin()?;
    if !is_digest(&digest) {
        return Err(package_not_found());
    }
    let tx = state.db.begin().await?;
    store::lock_workspace_uploads(&tx, workspace_id).await?;
    let Some(row) = store::find_owned(&tx, Some(workspace_id), &digest).await? else {
        return Err(if store::find_owned(&tx, None, &digest).await?.is_some() {
            ApiError::conflict_coded("package_builtin", "a built-in package cannot be deleted", None)
        } else {
            package_not_found()
        });
    };
    if store::has_live_generation(&tx, row.id).await? {
        return Err(package_in_use());
    }
    store::delete_history_generations(&tx, row.id)
        .await
        .map_err(map_in_use)?;
    store::delete_package(&tx, row.id).await.map_err(map_in_use)?;
    // The deferred `active_generation` key is checked here: a revoke that slipped in is a conflict.
    tx.commit().await.map_err(|err| map_in_use(err.into()))?;
    tracing::info!(%workspace_id, package_id = %row.id, "package deleted");
    remove_orphan_blob(&state, workspace_id, &row.blob_digest).await;
    Ok(ApiResponse::ok())
}

/// Drops the blob of a deleted package when nothing else uses it, in a transaction of its own after
/// the package delete committed. A failure (a foreign-key violation means a concurrent upload of
/// the same blob) is logged and never fails the request.
async fn remove_orphan_blob(state: &AppState, workspace_id: Uuid, blob_digest: &str) {
    let result = async {
        let tx = state.db.begin().await?;
        store::delete_unreferenced_blobs(&tx, std::slice::from_ref(&blob_digest.to_owned())).await?;
        tx.commit().await?;
        Ok::<(), ApiError>(())
    }
    .await;
    if let Err(err) = result {
        tracing::warn!(%workspace_id, error = %err, "package blob of a deleted package was kept");
    }
}

#[derive(Debug, Deserialize)]
pub struct ListQuery {
    kind: Option<String>,
    q: Option<String>,
    // Not `#[serde(flatten)]`: a flattened struct reads every query value as a string, so
    // `per_page=200` would be refused as an invalid integer.
    page: Option<i64>,
    per_page: Option<i64>,
}

/// `GET /api/v1/workspaces/{w}/packages?kind=&q=&page=`: built-in packages and the workspace's own.
pub async fn list_packages(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    bot: Option<Extension<BotAuthContext>>,
    Path(workspace_id): Path<Uuid>,
    Query(query): Query<ListQuery>,
) -> Result<impl IntoResponse, ApiError> {
    Caller::resolve(&state, &claims, bot, workspace_id).await?;
    if let Some(kind) = query.kind.as_deref()
        && !KINDS.contains(&kind)
    {
        return Err(ApiError::bad_request_coded(
            "kind_invalid",
            "kind must be one of: form-template, page-template, app, plugin",
        ));
    }
    let q = query.q.as_deref().map(str::trim).filter(|q| !q.is_empty());
    if q.is_some_and(|q| q.chars().count() > 100) {
        return Err(ApiError::bad_request_coded(
            "q_invalid",
            "q is limited to 100 characters",
        ));
    }
    let paging = PageQuery {
        page: query.page,
        per_page: query.per_page,
    }
    .resolve(50, 200);
    let (rows, total) = store::list_visible(
        &state.db,
        workspace_id,
        query.kind.as_deref(),
        q,
        paging.per_page,
        paging.offset(),
    )
    .await?;
    let items: Vec<PackageSummary> = rows.iter().map(PackageSummary::from).collect();
    Ok(ApiResponse::success(PaginatedData::page(items, total, paging)))
}

#[derive(Debug, Serialize)]
pub struct PackageDetail {
    #[serde(flatten)]
    summary: PackageSummary,
    manifest: Value,
    files: Vec<Value>,
    analysis: Value,
}

/// What the review needs from the manifest: requests with their bind hints, actions, rules,
/// layouts, pages and the forms the package binds or creates. `risk` is filled from the
/// capability catalogue once it exists.
fn analysis(manifest: &Manifest) -> Value {
    let form_hint = |f: &Option<String>| f.as_deref().and_then(|f| f.strip_prefix("$bind.")).map(str::to_owned);
    json!({
        "requests": manifest.requests.iter()
            .map(|r| json!({"id": r.id, "risk": null, "form_hint": form_hint(&r.form)}))
            .collect::<Vec<_>>(),
        "actions": manifest.actions.iter().map(|a| json!({"name": a.name, "label": a.label})).collect::<Vec<_>>(),
        "rules": manifest.rules.iter()
            .map(|r| json!({"name": r.name, "stages": r.stages, "field_types": r.field_types}))
            .collect::<Vec<_>>(),
        "layouts": manifest.layouts.iter().map(|l| json!({"key": l.key, "slots": l.slots})).collect::<Vec<_>>(),
        "pages": manifest.pages.iter()
            .map(|p| json!({"route": p.route, "layout": p.layout, "audience": p.audience}))
            .collect::<Vec<_>>(),
        "forms": manifest.binds.iter()
            .map(|(name, b)| json!({"bind": name, "create": b.create_from.is_some(), "requires_fields": b.requires_fields}))
            .collect::<Vec<_>>(),
    })
}

/// `GET /api/v1/workspaces/{w}/packages/{digest}`: the stored package in detail.
///
/// The summary, the manifest, the file list and the analysis, all from the stored row: a read
/// never opens the blob (the manifest column is display-only; install re-derives everything from
/// the blob).
pub async fn get_package(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    bot: Option<Extension<BotAuthContext>>,
    Path((workspace_id, digest)): Path<(Uuid, String)>,
) -> Result<impl IntoResponse, ApiError> {
    Caller::resolve(&state, &claims, bot, workspace_id).await?;
    if !is_digest(&digest) {
        return Err(package_not_found());
    }
    let row = store::find_visible(&state.db, workspace_id, &digest)
        .await?
        .ok_or_else(package_not_found)?;
    let manifest: Manifest = serde_json::from_value(row.manifest.clone()).map_err(|_| {
        tracing::error!(package_id = %row.id, "stored manifest no longer parses");
        ApiError::internal()
    })?;
    let files = row.files.as_array().cloned().unwrap_or_default();
    Ok(ApiResponse::success(PackageDetail {
        summary: PackageSummary::from(&row),
        manifest: row.manifest,
        files,
        analysis: analysis(&manifest),
    }))
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RegisterKeyRequest {
    publisher: String,
    /// The 32-byte Ed25519 public key, base64url without padding.
    public_key: String,
}

fn publisher_ok(p: &str) -> bool {
    let b = p.as_bytes();
    (1..=39).contains(&b.len())
        && b.first().is_some_and(|c| c.is_ascii_lowercase() || c.is_ascii_digit())
        && b.iter()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || *c == b'-')
}

/// `GET /api/v1/workspaces/{w}/publisher-keys`
pub async fn list_publisher_keys(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    Path(workspace_id): Path<Uuid>,
) -> Result<impl IntoResponse, ApiError> {
    let caller = Caller::resolve(&state, &claims, None, workspace_id).await?;
    caller.require_admin()?;
    let rows = keys::list_workspace_keys(&state.db, workspace_id).await?;
    let items: Vec<PublisherKey> = rows.iter().map(PublisherKey::from).collect();
    Ok(ApiResponse::success(PaginatedData::from_items(items)))
}

/// `POST /api/v1/workspaces/{w}/publisher-keys`: registers a public key for a publisher id of
/// this workspace. Workspace owner only; `sv` is reserved.
pub async fn register_publisher_key(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    Path(workspace_id): Path<Uuid>,
    Json(body): Json<RegisterKeyRequest>,
) -> Result<impl IntoResponse, ApiError> {
    let caller = Caller::resolve(&state, &claims, None, workspace_id).await?;
    caller.require_owner()?;
    if body.publisher == "sv" {
        return Err(ApiError::bad_request_coded(
            "publisher_reserved",
            "publisher `sv` is reserved",
        ));
    }
    if !publisher_ok(&body.publisher) {
        return Err(ApiError::bad_request_coded(
            "publisher_invalid",
            "publisher must match [a-z0-9][a-z0-9-]{0,38}",
        ));
    }
    let public = URL_SAFE_NO_PAD
        .decode(body.public_key.trim())
        .ok()
        .filter(|k| k.len() == 32)
        .ok_or_else(|| {
            ApiError::bad_request_coded(
                "public_key_invalid",
                "public_key must be 32 bytes in base64url without padding",
            )
        })?;
    let key_id = sv_package::key_id(&public);
    let row = keys::insert_workspace_key(
        &state.db,
        workspace_id,
        &body.publisher,
        &key_id,
        &public,
        caller.user_id,
    )
    .await?
    .ok_or_else(|| ApiError::conflict_coded("publisher_key_exists", "this key is already registered", None))?;
    tracing::info!(%workspace_id, key_id = %row.key_id, "publisher key registered");
    Ok(ApiResponse::success(PublisherKey::from(&row)))
}

/// `POST /api/v1/workspaces/{w}/publisher-keys/{key_id}/revoke`: revokes the key; its packages
/// stop being installable. Workspace owner only; revoking twice answers the revoked key.
pub async fn revoke_publisher_key(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    Path((workspace_id, key_row_id)): Path<(Uuid, Uuid)>,
) -> Result<impl IntoResponse, ApiError> {
    let caller = Caller::resolve(&state, &claims, None, workspace_id).await?;
    caller.require_owner()?;
    let tx = state.db.begin().await?;
    let row = keys::find_workspace_key(&tx, workspace_id, key_row_id)
        .await?
        .ok_or_else(|| ApiError::not_found("publisher key not found"))?;
    let revoked = keys::revoke_key(&tx, &row).await?;
    // Installations that run a package signed by this key stop (contract C3-7).
    let stopped = crate::extensions::install::store::disable_for_revoked_key(&tx, row.id).await?;
    tx.commit().await?;
    if !stopped.is_empty() {
        tracing::info!(%workspace_id, count = stopped.len(), "installations disabled: their package key was revoked");
        if let Err(error) = crate::extensions::rules::refresh(&state.db).await {
            tracing::warn!(%error, "the plugin rule registry was not refreshed");
        }
    }
    tracing::info!(%workspace_id, key_id = %revoked.key_id, "publisher key revoked");
    Ok(ApiResponse::success(PublisherKey::from(&revoked)))
}

#[cfg(test)]
mod tests {
    use super::{is_digest, publisher_ok};

    #[test]
    fn digests_and_publishers_are_checked_strictly() {
        assert!(is_digest(&format!("sha256:{}", "a".repeat(64))));
        assert!(!is_digest(&format!("sha256:{}", "A".repeat(64))));
        assert!(!is_digest("sha256:abc"));
        assert!(!is_digest(&"a".repeat(64)));
        assert!(publisher_ok("acme") && publisher_ok("a-b-1"));
        assert!(!publisher_ok("") && !publisher_ok("-a") && !publisher_ok("A") && !publisher_ok(&"a".repeat(40)));
    }
}
