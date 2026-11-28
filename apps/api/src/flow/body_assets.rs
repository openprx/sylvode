//! Phase2 candidate: Page-owned private body assets, independently from appearance uploads.
//! These contracts are not wired to HTTP or enabled against the current reader3 runtime.
use crate::error::ApiError;
use base64::{Engine, engine::general_purpose::STANDARD};
use serde::{Deserialize, Serialize};
use serde_json::json;
use uuid::Uuid;

pub const IMAGE_CONTENT_TYPES: &[&str] = &["image/png", "image/jpeg", "image/webp", "image/gif"];
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AssetKind {
    Image,
    Attachment,
}
impl AssetKind {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Image => "image",
            Self::Attachment => "attachment",
        }
    }
}
#[derive(Debug, Clone, Serialize)]
pub struct AssetLimits {
    pub max_bytes: u64,
    pub image_content_types: &'static [&'static str],
    pub upload_transport: &'static str,
    pub editor_model: u32,
}
impl AssetLimits {
    pub fn configured() -> Self {
        Self {
            max_bytes: crate::config::runtime().flow.body_asset_max_bytes,
            image_content_types: IMAGE_CONTENT_TYPES,
            upload_transport: "multipart",
            editor_model: 5,
        }
    }
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Base64Upload {
    pub kind: AssetKind,
    pub filename: String,
    pub data_base64: String,
    pub content_type: Option<String>,
}
#[derive(Debug, Serialize, Deserialize)]
pub struct AssetCreated {
    pub asset_id: Uuid,
    pub object_id: Uuid,
    pub kind: AssetKind,
    pub filename: String,
    pub content_type: String,
    pub byte_size: usize,
    pub sha256: String,
    pub state: String,
    pub read_path: String,
}
pub struct ValidatedUpload {
    pub kind: AssetKind,
    pub filename: String,
    pub content_type: &'static str,
    pub bytes: Vec<u8>,
    pub sha256: String,
}
fn size_error(limit: u64, observed: u64) -> ApiError {
    ApiError::limit_exceeded(
        "Page asset exceeds the configured byte limit",
        "page_asset_bytes",
        Some(json!(limit)),
        Some(json!(observed)),
        None,
    )
}
pub fn validate_filename(value: &str) -> Result<String, ApiError> {
    let value = value.trim();
    if value.is_empty() || value.len() > 255 || value.chars().any(|ch| ch.is_control() || matches!(ch, '/' | '\\')) {
        return Err(ApiError::invalid_update(
            "asset filename must be 1..255 UTF-8 bytes with no path or control characters",
        ));
    }
    Ok(value.into())
}
pub fn validate_bytes(
    kind: AssetKind,
    filename: &str,
    reported_content_type: Option<&str>,
    bytes: Vec<u8>,
    limit: u64,
) -> Result<ValidatedUpload, ApiError> {
    let observed = u64::try_from(bytes.len()).map_err(|_| ApiError::invalid_update("invalid asset size"))?;
    if observed > limit {
        return Err(size_error(limit, observed));
    }
    if bytes.is_empty() {
        return Err(ApiError::invalid_update("asset file is empty"));
    }
    let filename = validate_filename(filename)?;
    if reported_content_type
        .is_some_and(|value| value.len() > 128 || !value.is_ascii() || value.chars().any(char::is_control))
    {
        return Err(ApiError::invalid_update("invalid reported content type"));
    }
    let content_type = match kind {
        AssetKind::Image => super::appearance::sniff_image(&bytes)
            .ok_or_else(|| ApiError::invalid_update("image bytes must have PNG, JPEG, WebP or GIF magic"))?,
        AssetKind::Attachment => "application/octet-stream",
    };
    let sha256 = super::collab::bootstrap::content_hash(&bytes);
    Ok(ValidatedUpload {
        kind,
        filename,
        content_type,
        bytes,
        sha256,
    })
}
pub fn decode_base64(input: &Base64Upload, limit: u64) -> Result<ValidatedUpload, ApiError> {
    let max_encoded = limit.saturating_add(2) / 3 * 4;
    let observed =
        u64::try_from(input.data_base64.len()).map_err(|_| ApiError::invalid_update("invalid encoded asset size"))?;
    if observed > max_encoded {
        return Err(size_error(limit, observed.saturating_mul(3) / 4));
    }
    let bytes = STANDARD
        .decode(&input.data_base64)
        .map_err(|_| ApiError::invalid_update("invalid asset base64"))?;
    validate_bytes(input.kind, &input.filename, input.content_type.as_deref(), bytes, limit)
}
/// Download disposition always comes from the stored filename, never document attrs.
pub fn attachment_disposition(filename: &str) -> Result<String, ApiError> {
    let filename = validate_filename(filename)?;
    let mut encoded = String::new();
    for byte in filename.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.') {
            encoded.push(char::from(byte));
        } else {
            use std::fmt::Write;
            let _ = write!(&mut encoded, "%{byte:02X}");
        }
    }
    Ok(format!(
        "attachment; filename=\"attachment\"; filename*=UTF-8''{encoded}"
    ))
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn bytes_and_headers_are_authoritative_not_client_mime_or_filename_labels() {
        let image = validate_bytes(
            AssetKind::Image,
            "photo.png",
            Some("text/html"),
            vec![0x89, b'P', b'N', b'G', 13, 10, 26, 10],
            1024,
        )
        .unwrap();
        assert_eq!(image.content_type, "image/png");
        let file = validate_bytes(
            AssetKind::Attachment,
            "中文.html",
            Some("text/html"),
            b"<html>literal bytes</html>".to_vec(),
            1024,
        )
        .unwrap();
        assert_eq!(file.content_type, "application/octet-stream");
        let disposition = attachment_disposition(&file.filename).unwrap();
        assert!(disposition.starts_with("attachment;"));
        assert!(disposition.contains("filename*=UTF-8''%"));
        assert!(
            validate_bytes(
                AssetKind::Image,
                "wrong.png",
                Some("image/png"),
                b"<svg/>".to_vec(),
                1024
            )
            .is_err()
        );
    }
    #[test]
    fn bounded_decode_refuses_size_and_malformed_metadata_before_storage() {
        let input = Base64Upload {
            kind: AssetKind::Attachment,
            filename: "test.bin".into(),
            data_base64: "A".repeat(100),
            content_type: None,
        };
        assert!(decode_base64(&input, 1).is_err());
        for filename in ["../escape", "bad\\name", "bad\nheader", ""] {
            assert!(validate_filename(filename).is_err());
        }
        assert!(validate_bytes(AssetKind::Attachment, "a.bin", None, vec![], 10).is_err());
    }
}

/// Read multipart incrementally; never collect the whole generic HTTP body first.
pub async fn decode_multipart(
    mut multipart: axum::extract::Multipart,
    limit: u64,
) -> Result<ValidatedUpload, ApiError> {
    let mut kind = None;
    let mut file = None;
    while let Some(mut field) = multipart
        .next_field()
        .await
        .map_err(|_| ApiError::invalid_update("malformed body asset multipart"))?
    {
        let name = field.name().unwrap_or("").to_owned();
        match name.as_str() {
            "kind" if kind.is_none() => {
                let mut value = Vec::new();
                while let Some(chunk) = field
                    .chunk()
                    .await
                    .map_err(|_| ApiError::invalid_update("malformed upload kind"))?
                {
                    if value.len().saturating_add(chunk.len()) > 16 {
                        return Err(ApiError::invalid_update("invalid upload kind"));
                    }
                    value.extend_from_slice(&chunk);
                }
                kind = Some(match value.as_slice() {
                    b"image" => AssetKind::Image,
                    b"attachment" => AssetKind::Attachment,
                    _ => return Err(ApiError::invalid_update("kind must be image or attachment")),
                });
            }
            "file" if file.is_none() => {
                let filename = field
                    .file_name()
                    .ok_or_else(|| ApiError::invalid_update("file filename is required"))?
                    .to_owned();
                let reported = field.content_type().map(str::to_owned);
                let mut bytes = Vec::new();
                while let Some(chunk) = field
                    .chunk()
                    .await
                    .map_err(|_| ApiError::invalid_update("incomplete asset upload"))?
                {
                    let observed = u64::try_from(bytes.len().saturating_add(chunk.len()))
                        .map_err(|_| ApiError::invalid_update("invalid asset size"))?;
                    if observed > limit {
                        return Err(size_error(limit, observed));
                    }
                    bytes
                        .try_reserve(chunk.len())
                        .map_err(|_| ApiError::invalid_update("asset buffer allocation failed"))?;
                    bytes.extend_from_slice(&chunk);
                }
                file = Some((filename, reported, bytes));
            }
            _ => {
                return Err(ApiError::invalid_update(
                    "multipart accepts exactly one kind and one file",
                ));
            }
        }
    }
    let kind = kind.ok_or_else(|| ApiError::invalid_update("upload kind is required"))?;
    let (filename, reported, bytes) = file.ok_or_else(|| ApiError::invalid_update("upload file is required"))?;
    validate_bytes(kind, &filename, reported.as_deref(), bytes, limit)
}

use super::{collab::authz::PermissionLevel, scope};
use sea_orm::{ConnectionTrait, DatabaseConnection, DbBackend, FromQueryResult, Statement, TransactionTrait};
#[derive(FromQueryResult)]
struct AssetRow {
    id: Uuid,
    object_id: Uuid,
    kind: String,
    filename: String,
    content_type: String,
    byte_size: i32,
    content_sha256: String,
    state: String,
    created_by: Option<Uuid>,
    referenced: bool,
    head_seq: i64,
}
pub struct AssetBytes {
    pub metadata: AssetCreated,
    pub bytes: Vec<u8>,
    pub disposition: String,
}
async fn page<C: ConnectionTrait>(
    db: &C,
    object: Uuid,
    user: Uuid,
    minimum: PermissionLevel,
) -> Result<(scope::ObjectIdentity, scope::AuthorizationProof), ApiError> {
    let proof = scope::authorize(db, object, user, minimum).await?;
    let identity = scope::identity(db, object).await?;
    if identity.object_type != "page" || (minimum >= PermissionLevel::Edit && !proof.resource_active) {
        return Err(ApiError::policy_rejected("body assets require an active editable Page"));
    }
    Ok((identity, proof))
}
fn result(row: AssetRow) -> Result<AssetCreated, ApiError> {
    let kind = match row.kind.as_str() {
        "image" => AssetKind::Image,
        "attachment" => AssetKind::Attachment,
        _ => return Err(ApiError::internal()),
    };
    Ok(AssetCreated {
        asset_id: row.id,
        object_id: row.object_id,
        kind,
        filename: row.filename,
        content_type: row.content_type,
        byte_size: usize::try_from(row.byte_size).map_err(|_| ApiError::internal())?,
        sha256: row.content_sha256,
        state: row.state,
        read_path: format!("/api/v2/flow/objects/{}/body-assets/{}", row.object_id, row.id),
    })
}
/// Store blob and idempotent metadata receipt in one private database transaction.
/// Request bodies and events contain hashes/metadata, never binary/base64 payloads.
pub async fn upload(
    db: &DatabaseConnection,
    object: Uuid,
    user: Uuid,
    upload: ValidatedUpload,
    key: &str,
) -> Result<AssetCreated, ApiError> {
    let configured = AssetLimits::configured().max_bytes;
    if u64::try_from(upload.bytes.len()).is_ok_and(|bytes| bytes > configured) {
        return Err(size_error(
            configured,
            u64::try_from(upload.bytes.len()).unwrap_or(u64::MAX),
        ));
    }
    let (identity, proof) = page(db, object, user, PermissionLevel::Edit).await?;
    let body = json!({"operation":"page_asset_upload","object_id":object,"kind":upload.kind,"filename":upload.filename,"content_type":upload.content_type,"byte_size":upload.bytes.len(),"sha256":upload.sha256});
    let tx = db.begin().await?;
    if identity.workspace_id.is_none() {
        scope::require_personal_write(&tx).await?;
    }
    scope::fence(&tx, &proof).await?;
    if let Some(prior) = super::scope_commands::lock_and_replay(&tx, identity.scope_id, user, key, &body).await? {
        tx.commit().await?;
        return serde_json::from_value(prior).map_err(|_| ApiError::internal());
    }
    let id = Uuid::new_v4();
    let byte_size = i32::try_from(upload.bytes.len()).map_err(|_| ApiError::invalid_update("invalid asset size"))?;
    tx.execute(Statement::from_sql_and_values(DbBackend::Postgres,"INSERT INTO flow_page_assets(id,scope_id,object_id,kind,filename,content_type,byte_size,content_sha256,bytes,created_by) VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9,$10)",vec![id.into(),identity.scope_id.into(),object.into(),upload.kind.as_str().into(),upload.filename.clone().into(),upload.content_type.into(),byte_size.into(),upload.sha256.clone().into(),upload.bytes.into(),user.into()])).await?;
    let result = AssetCreated {
        asset_id: id,
        object_id: object,
        kind: upload.kind,
        filename: upload.filename,
        content_type: upload.content_type.into(),
        byte_size: usize::try_from(byte_size).map_err(|_| ApiError::internal())?,
        sha256: upload.sha256,
        state: "staged".into(),
        read_path: format!("/api/v2/flow/objects/{object}/body-assets/{id}"),
    };
    let event = super::scope_event::insert_object_audit(
        &tx,
        &identity,
        user,
        "flow.page_asset.uploaded",
        key,
        json!({"object_id":object,"asset_id":id,"content_type":result.content_type,"byte_size":byte_size}),
    )
    .await?;
    super::scope_commands::record(
        &tx,
        identity.scope_id,
        user,
        key,
        body,
        object,
        event,
        serde_json::to_value(&result).map_err(|_| ApiError::internal())?,
    )
    .await?;
    tx.commit().await?;
    Ok(result)
}
async fn authorized_row(
    db: &DatabaseConnection,
    object: Uuid,
    asset: Uuid,
    user: Uuid,
) -> Result<(AssetRow, scope::AuthorizationProof), ApiError> {
    let (_, proof) = page(db, object, user, PermissionLevel::View).await?;
    let row=AssetRow::find_by_statement(Statement::from_sql_and_values(DbBackend::Postgres,"SELECT a.id,a.object_id,a.kind,a.filename,a.content_type,a.byte_size,a.content_sha256,a.state,a.created_by,EXISTS(SELECT 1 FROM flow_page_asset_refs r WHERE r.scope_id=a.scope_id AND r.object_id=a.object_id AND r.asset_id=a.id AND r.active AND r.last_seen_seq=d.head_seq) AS referenced,d.head_seq FROM flow_page_assets a JOIN collab_documents d ON d.object_id=a.object_id WHERE a.id=$1 AND a.object_id=$2 AND a.scope_id=$3",vec![asset.into(),object.into(),proof.scope_id.into()])).one(db).await?.ok_or_else(||ApiError::not_found("Page body asset not found"))?;
    if row.state == "discarded"
        || (!row.referenced && proof.level < PermissionLevel::Edit && row.created_by != Some(user))
    {
        return Err(ApiError::not_found("Page body asset not found"));
    }
    Ok((row, proof))
}
async fn read_stable(
    db: &DatabaseConnection,
    object: Uuid,
    asset: Uuid,
    user: Uuid,
    row: &AssetRow,
    proof: &scope::AuthorizationProof,
) -> Result<(), ApiError> {
    if !scope::proof_current(db, proof).await? {
        return Err(super::policy::authorization_read_unstable());
    }
    let current = authorized_row(db, object, asset, user).await?;
    if current.0.head_seq != row.head_seq || current.0.state != row.state || current.0.referenced != row.referenced {
        return Err(super::policy::authorization_read_unstable());
    }
    Ok(())
}
pub async fn metadata(
    db: &DatabaseConnection,
    object: Uuid,
    asset: Uuid,
    user: Uuid,
) -> Result<AssetCreated, ApiError> {
    let (row, proof) = authorized_row(db, object, asset, user).await?;
    read_stable(db, object, asset, user, &row, &proof).await?;
    result(row)
}
pub async fn read(db: &DatabaseConnection, object: Uuid, asset: Uuid, user: Uuid) -> Result<AssetBytes, ApiError> {
    let (row, proof) = authorized_row(db, object, asset, user).await?;
    let blob = db
        .query_one(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT bytes FROM flow_page_assets WHERE id=$1 AND object_id=$2 AND scope_id=$3",
            vec![asset.into(), object.into(), proof.scope_id.into()],
        ))
        .await?
        .ok_or_else(|| ApiError::not_found("Page body asset not found"))?;
    let bytes: Vec<u8> = blob.try_get("", "bytes")?;
    if super::collab::bootstrap::content_hash(&bytes) != row.content_sha256 {
        return Err(ApiError::checksum_mismatch("Page body asset checksum mismatch"));
    }
    read_stable(db, object, asset, user, &row, &proof).await?;
    let metadata = result(row)?;
    let disposition = if metadata.kind == AssetKind::Attachment {
        attachment_disposition(&metadata.filename)?
    } else {
        "inline".into()
    };
    Ok(AssetBytes {
        metadata,
        bytes,
        disposition,
    })
}
/// Discard never-referenced uploads only. Historical assets are retained for restore.
pub async fn discard(
    db: &DatabaseConnection,
    object: Uuid,
    asset: Uuid,
    user: Uuid,
    key: &str,
) -> Result<serde_json::Value, ApiError> {
    let (identity, proof) = page(db, object, user, PermissionLevel::Edit).await?;
    let body = json!({"operation":"page_asset_discard","object_id":object,"asset_id":asset});
    let tx = db.begin().await?;
    if identity.workspace_id.is_none() {
        scope::require_personal_write(&tx).await?;
    }
    scope::fence(&tx, &proof).await?;
    if let Some(prior) = super::scope_commands::lock_and_replay(&tx, identity.scope_id, user, key, &body).await? {
        tx.commit().await?;
        return Ok(prior);
    }
    let changed=tx.execute(Statement::from_sql_and_values(DbBackend::Postgres,"UPDATE flow_page_assets a SET state='discarded',updated_at=now() WHERE a.id=$1 AND a.object_id=$2 AND a.scope_id=$3 AND a.first_referenced_seq IS NULL AND NOT EXISTS(SELECT 1 FROM flow_page_asset_refs r WHERE r.asset_id=a.id)",vec![asset.into(),object.into(),identity.scope_id.into()])).await?;
    if changed.rows_affected() != 1 {
        return Err(ApiError::policy_rejected(
            "only a never-referenced staged upload can be discarded",
        ));
    }
    let result = json!({"asset_id":asset,"state":"discarded"});
    let event = super::scope_event::insert_object_audit(
        &tx,
        &identity,
        user,
        "flow.page_asset.discarded",
        key,
        json!({"object_id":object,"asset_id":asset}),
    )
    .await?;
    super::scope_commands::record(&tx, identity.scope_id, user, key, body, object, event, result.clone()).await?;
    tx.commit().await?;
    Ok(result)
}
/// Bounded maintenance primitive. No scheduler is connected until rollout policy is accepted.
/// Only expired, never-referenced staged blobs qualify; any historical ref prevents deletion.
pub async fn reap_unused(
    db: &DatabaseConnection,
    older_than: platform::timestamp::Timestamp,
    batch: u32,
) -> Result<u64, ApiError> {
    reap_rows(db, older_than, batch).await
}
async fn reap_rows(
    db: &DatabaseConnection,
    older_than: platform::timestamp::Timestamp,
    batch: u32,
) -> Result<u64, ApiError> {
    if !(1..=32).contains(&batch) {
        return Err(ApiError::invalid_update("asset reap batch must be 1..32"));
    }
    let outcome=db.execute(Statement::from_sql_and_values(DbBackend::Postgres,"DELETE FROM flow_page_assets WHERE id IN (SELECT a.id FROM flow_page_assets a WHERE a.state IN ('staged','discarded') AND a.first_referenced_seq IS NULL AND a.created_at<$1 AND NOT EXISTS(SELECT 1 FROM flow_page_asset_refs r WHERE r.asset_id=a.id) ORDER BY a.created_at,a.id LIMIT $2 FOR UPDATE SKIP LOCKED)",vec![older_than.into(),i64::from(batch).into()])).await?;
    Ok(outcome.rows_affected())
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AssetReference {
    pub block_cid: String,
    pub asset_id: Uuid,
    pub kind: AssetKind,
}
/// Decode only explicitly typed media slots; plain UUID-looking text is never scanned.
pub fn typed_references(snapshot: &collab_core::SemanticSnapshot) -> Result<Vec<AssetReference>, ApiError> {
    let mut references = Vec::new();
    for (id, node) in &snapshot.nodes {
        if node.deleted || node.kind != collab_core::NodeKind::Block {
            continue;
        }
        let kind = match node.properties.get("block_type").map(String::as_str) {
            Some("image") => AssetKind::Image,
            Some("attachment") => AssetKind::Attachment,
            _ => continue,
        };
        let asset_id = node
            .properties
            .get("asset_id")
            .and_then(|value| Uuid::parse_str(value).ok())
            .ok_or_else(|| ApiError::invalid_update("media block requires a typed asset_id"))?;
        references.push(AssetReference {
            block_cid: id.to_string(),
            asset_id,
            kind,
        });
    }
    check_reference_shapes(&references)?;
    Ok(references)
}
fn check_reference_shapes(references: &[AssetReference]) -> Result<(), ApiError> {
    if references.len() > collab_core::limits::DOCUMENT_BLOCK_COUNT_MAX {
        return Err(ApiError::invalid_update("too many Page asset references"));
    }
    let mut blocks = std::collections::BTreeSet::new();
    for reference in references {
        if reference.block_cid.is_empty()
            || reference.block_cid.len() > 256
            || !blocks.insert(reference.block_cid.as_str())
        {
            return Err(ApiError::invalid_update(
                "body asset references require unique bounded block CIDs",
            ));
        }
    }
    Ok(())
}
#[derive(FromQueryResult)]
struct LockedAsset {
    id: Uuid,
    kind: String,
    state: String,
}
/// The caller owns the accepted document-head lock and Page authority fence. Validation,
/// current reference replacement and retention metadata must share its content/event transaction.
pub async fn lock_and_sync_references(
    tx: &sea_orm::DatabaseTransaction,
    scope_id: Uuid,
    object: Uuid,
    seq: i64,
    references: &[AssetReference],
) -> Result<(), ApiError> {
    sync_reference_rows(tx, scope_id, object, seq, references).await
}
async fn sync_reference_rows(
    tx: &sea_orm::DatabaseTransaction,
    scope_id: Uuid,
    object: Uuid,
    seq: i64,
    references: &[AssetReference],
) -> Result<(), ApiError> {
    check_reference_shapes(references)?;
    if seq < 0 {
        return Err(ApiError::invalid_update("invalid accepted Page asset sequence"));
    }
    let ids: Vec<Uuid> = references
        .iter()
        .map(|reference| reference.asset_id)
        .collect::<std::collections::BTreeSet<_>>()
        .into_iter()
        .collect();
    if !ids.is_empty() {
        let assets=LockedAsset::find_by_statement(Statement::from_sql_and_values(DbBackend::Postgres,"SELECT id,kind,state FROM flow_page_assets WHERE scope_id=$1 AND object_id=$2 AND id=ANY($3) ORDER BY id FOR UPDATE",vec![scope_id.into(),object.into(),ids.clone().into()])).all(tx).await?;
        if assets.len() != ids.len() {
            return Err(ApiError::invalid_update_with_details(
                "asset does not belong to this Page; copy it into the destination first",
                json!({"reason":"page_asset_owner_mismatch"}),
            ));
        }
        let lookup: std::collections::BTreeMap<_, _> = assets.iter().map(|asset| (asset.id, asset)).collect();
        for reference in references {
            let asset = lookup.get(&reference.asset_id).ok_or_else(ApiError::internal)?;
            if asset.kind != reference.kind.as_str() || asset.state == "discarded" {
                return Err(ApiError::invalid_update(
                    "body asset kind/state does not match the Page node",
                ));
            }
        }
    }
    tx.execute(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "UPDATE flow_page_asset_refs SET active=false WHERE scope_id=$1 AND object_id=$2 AND active",
        vec![scope_id.into(), object.into()],
    ))
    .await?;
    if !references.is_empty() {
        tx.execute(Statement::from_sql_and_values(DbBackend::Postgres,"INSERT INTO flow_page_asset_refs(scope_id,object_id,block_cid,asset_id,active,first_seen_seq,last_seen_seq) SELECT $1,$2,r.block_cid,r.asset_id,true,$5,$5 FROM unnest($3::text[],$4::uuid[]) AS r(block_cid,asset_id) ON CONFLICT(scope_id,object_id,block_cid,asset_id) DO UPDATE SET active=true,last_seen_seq=EXCLUDED.last_seen_seq",vec![scope_id.into(),object.into(),references.iter().map(|reference|reference.block_cid.clone()).collect::<Vec<_>>().into(),references.iter().map(|reference|reference.asset_id).collect::<Vec<_>>().into(),seq.into()])).await?;
    }
    tx.execute(Statement::from_sql_and_values(DbBackend::Postgres,"UPDATE flow_page_assets a SET state=CASE WHEN EXISTS(SELECT 1 FROM flow_page_asset_refs r WHERE r.scope_id=a.scope_id AND r.object_id=a.object_id AND r.asset_id=a.id AND r.active) THEN 'active' WHEN a.first_referenced_seq IS NOT NULL THEN 'retained' ELSE a.state END,first_referenced_seq=CASE WHEN a.id=ANY($3) THEN coalesce(a.first_referenced_seq,$4) ELSE a.first_referenced_seq END,last_referenced_seq=CASE WHEN a.id=ANY($3) THEN $4 ELSE a.last_referenced_seq END,updated_at=now() WHERE a.scope_id=$1 AND a.object_id=$2 AND (a.id=ANY($3) OR a.state='active')",vec![scope_id.into(),object.into(),ids.into(),seq.into()])).await?;
    Ok(())
}

/// Cross-Page copying creates an independently owned blob. No source asset authorization
/// is delegated through the target's bytes endpoint; same-Page duplicate nodes may reuse a blob.
pub async fn copy_into(
    db: &DatabaseConnection,
    target: Uuid,
    source: Uuid,
    asset: Uuid,
    user: Uuid,
    key: &str,
) -> Result<AssetCreated, ApiError> {
    if target == source {
        return Err(ApiError::invalid_update(
            "same-Page duplicate nodes should reuse their owned asset",
        ));
    }
    let (target_identity, target_proof) = page(db, target, user, PermissionLevel::Edit).await?;
    let body =
        json!({"operation":"page_asset_copy","target_object_id":target,"source_object_id":source,"asset_id":asset});
    if let Some(prior) = super::scope_commands::replay(db, target_identity.scope_id, user, key, &body).await? {
        return serde_json::from_value(prior).map_err(|_| ApiError::internal());
    }
    let source_bytes = read(db, source, asset, user).await?;
    let (source_row, source_proof) = authorized_row(db, source, asset, user).await?;
    let upload = validate_bytes(
        source_bytes.metadata.kind,
        &source_bytes.metadata.filename,
        Some(&source_bytes.metadata.content_type),
        source_bytes.bytes,
        AssetLimits::configured().max_bytes,
    )?;
    let tx = db.begin().await?;
    if target_identity.workspace_id.is_none() {
        scope::require_personal_write(&tx).await?;
    }
    scope::fence_many(&tx, &[source_proof.clone(), target_proof]).await?;
    if let Some(prior) = super::scope_commands::lock_and_replay(&tx, target_identity.scope_id, user, key, &body).await?
    {
        tx.commit().await?;
        return serde_json::from_value(prior).map_err(|_| ApiError::internal());
    }
    // A source body-ref removal cannot race this final copy decision: content writers take
    // this same document row FOR UPDATE. Blob bytes remain outside the locked-phase CPU work.
    let current = tx
        .query_one(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT head_seq FROM collab_documents WHERE object_id=$1 FOR SHARE",
            vec![source.into()],
        ))
        .await?
        .ok_or_else(|| ApiError::not_found("source Page not found"))?;
    if current.try_get::<i64>("", "head_seq")? != source_row.head_seq {
        return Err(super::policy::authorization_read_unstable());
    }
    let source_lock=tx.query_one(Statement::from_sql_and_values(DbBackend::Postgres,"SELECT state,created_by,EXISTS(SELECT 1 FROM flow_page_asset_refs r WHERE r.asset_id=a.id AND r.active) AS referenced FROM flow_page_assets a WHERE id=$1 AND object_id=$2 AND scope_id=$3 FOR SHARE",vec![asset.into(),source.into(),source_proof.scope_id.into()])).await?.ok_or_else(||ApiError::not_found("source Page asset not found"))?;
    let visible = source_lock.try_get::<bool>("", "referenced")?
        || source_proof.level >= PermissionLevel::Edit
        || source_lock.try_get::<Option<Uuid>>("", "created_by")? == Some(user);
    if !visible || source_lock.try_get::<String>("", "state")? == "discarded" {
        return Err(ApiError::not_found("source Page asset not found"));
    }
    let id = Uuid::new_v4();
    let byte_size = i32::try_from(upload.bytes.len()).map_err(|_| ApiError::internal())?;
    tx.execute(Statement::from_sql_and_values(DbBackend::Postgres,"INSERT INTO flow_page_assets(id,scope_id,object_id,kind,filename,content_type,byte_size,content_sha256,bytes,created_by) VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9,$10)",vec![id.into(),target_identity.scope_id.into(),target.into(),upload.kind.as_str().into(),upload.filename.clone().into(),upload.content_type.into(),byte_size.into(),upload.sha256.clone().into(),upload.bytes.into(),user.into()])).await?;
    let result = AssetCreated {
        asset_id: id,
        object_id: target,
        kind: upload.kind,
        filename: upload.filename,
        content_type: upload.content_type.into(),
        byte_size: usize::try_from(byte_size).map_err(|_| ApiError::internal())?,
        sha256: upload.sha256,
        state: "staged".into(),
        read_path: format!("/api/v2/flow/objects/{target}/body-assets/{id}"),
    };
    let event = super::scope_event::insert_object_audit(
        &tx,
        &target_identity,
        user,
        "flow.page_asset.copied",
        key,
        json!({"object_id":target,"asset_id":id}),
    )
    .await?;
    super::scope_commands::record(
        &tx,
        target_identity.scope_id,
        user,
        key,
        body,
        target,
        event,
        serde_json::to_value(&result).map_err(|_| ApiError::internal())?,
    )
    .await?;
    tx.commit().await?;
    Ok(result)
}

#[cfg(test)]
mod sql_tests {
    use super::*;
    use crate::{routes::context::tenant_fixture::exec, scratch_or_skip};
    #[tokio::test]
    async fn body_asset_sql_ownership_reference_rollback_and_history_retention() {
        let scratch = scratch_or_skip!(&format!("page_asset_{}", Uuid::new_v4().simple()));
        let db = &scratch.db;
        let owner = Uuid::new_v4();
        let scope_id = Uuid::new_v4();
        let a = Uuid::new_v4();
        let b = Uuid::new_v4();
        exec(db,"INSERT INTO users(id,email,password_hash,name,role,is_active) VALUES($1,$2,'!','Synthetic asset owner','user',true)",vec![owner.into(),format!("{owner}@page-asset.test").into()]).await;
        exec(
            db,
            "INSERT INTO flow_scopes(id,kind,owner_user_id) VALUES($1,'personal',$2)",
            vec![scope_id.into(), owner.into()],
        )
        .await;
        exec(
            db,
            "INSERT INTO flow_scope_settings(scope_id) VALUES($1)",
            vec![scope_id.into()],
        )
        .await;
        let setup = db.begin().await.unwrap();
        let navigator = Uuid::new_v4();
        super::super::scope::insert_object_with_identity(
            &setup,
            scope_id,
            owner,
            "navigator",
            None,
            "",
            None,
            navigator,
            Uuid::new_v4(),
        )
        .await
        .unwrap();
        for page_id in [a, b] {
            super::super::scope::insert_object_with_identity(
                &setup,
                scope_id,
                owner,
                "page",
                Some(navigator),
                "Synthetic asset Page",
                None,
                page_id,
                Uuid::new_v4(),
            )
            .await
            .unwrap();
        }
        setup.commit().await.unwrap();
        let aa = Uuid::new_v4();
        let bb = Uuid::new_v4();
        let bytes = vec![0x89, b'P', b'N', b'G', 13, 10, 26, 10];
        let hash = super::super::collab::bootstrap::content_hash(&bytes);
        for (asset, page_id) in [(aa, a), (bb, b)] {
            exec(db,"INSERT INTO flow_page_assets(id,scope_id,object_id,kind,filename,content_type,byte_size,content_sha256,bytes,created_by) VALUES($1,$2,$3,'image','synthetic.png','image/png',8,$4,$5,$6)",vec![asset.into(),scope_id.into(),page_id.into(),hash.clone().into(),bytes.clone().into(),owner.into()]).await;
        }
        let direct_foreign = db.begin().await.unwrap();
        let denied=direct_foreign.execute(Statement::from_sql_and_values(DbBackend::Postgres,"INSERT INTO flow_page_asset_refs(scope_id,object_id,block_cid,asset_id,active,first_seen_seq,last_seen_seq) VALUES($1,$2,'cid:foreign-db:Map',$3,true,1,1)",vec![scope_id.into(),a.into(),bb.into()])).await;
        assert!(
            denied
                .as_ref()
                .is_err_and(|error| error.to_string().contains("foreign key")),
            "composite FK must reject a foreign-Page asset"
        );
        direct_foreign.rollback().await.unwrap();
        let invalid = db.begin().await.unwrap();
        assert!(
            sync_reference_rows(
                &invalid,
                scope_id,
                a,
                1,
                &[AssetReference {
                    block_cid: "cid:synthetic-block:Map".into(),
                    asset_id: bb,
                    kind: AssetKind::Image
                }]
            )
            .await
            .is_err()
        );
        invalid.rollback().await.unwrap();
        let active = db.begin().await.unwrap();
        sync_reference_rows(
            &active,
            scope_id,
            a,
            1,
            &[AssetReference {
                block_cid: "cid:synthetic-block:Map".into(),
                asset_id: aa,
                kind: AssetKind::Image,
            }],
        )
        .await
        .unwrap();
        active.commit().await.unwrap();
        let rolled_back = db.begin().await.unwrap();
        sync_reference_rows(&rolled_back, scope_id, a, 2, &[]).await.unwrap();
        rolled_back.rollback().await.unwrap();
        let state=db.query_one(Statement::from_sql_and_values(DbBackend::Postgres,"SELECT state,first_referenced_seq,(SELECT count(*) FROM flow_page_asset_refs WHERE asset_id=$1 AND active) AS active FROM flow_page_assets WHERE id=$1",vec![aa.into()])).await.unwrap().unwrap();
        assert_eq!(state.try_get::<String>("", "state").unwrap(), "active");
        assert_eq!(state.try_get::<i64>("", "active").unwrap(), 1);
        let retained = db.begin().await.unwrap();
        sync_reference_rows(&retained, scope_id, a, 2, &[]).await.unwrap();
        retained.commit().await.unwrap();
        let state = db
            .query_one(Statement::from_sql_and_values(
                DbBackend::Postgres,
                "SELECT state,first_referenced_seq FROM flow_page_assets WHERE id=$1",
                vec![aa.into()],
            ))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(state.try_get::<String>("", "state").unwrap(), "retained");
        assert_eq!(state.try_get::<i64>("", "first_referenced_seq").unwrap(), 1);
        assert_eq!(
            reap_rows(
                db,
                platform::timestamp::Timestamp::now() + time::Duration::minutes(1),
                32
            )
            .await
            .unwrap(),
            1
        );
        let remaining = db
            .query_one(Statement::from_sql_and_values(
                DbBackend::Postgres,
                "SELECT count(*) AS retained FROM flow_page_assets WHERE id=$1",
                vec![aa.into()],
            ))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(remaining.try_get::<i64>("", "retained").unwrap(), 1);
        scratch.drop_self().await;
    }
}

/// Shared hook for accepted Page writes. A non-Page object is a no-op;
/// typed refs never bypass ownership checks.
pub async fn reconcile_accepted(
    tx: &sea_orm::DatabaseTransaction,
    object: Uuid,
    seq: i64,
    references: &[AssetReference],
) -> Result<(), ApiError> {
    let identity = scope::identity(tx, object).await?;
    if identity.object_type != "page" {
        if !references.is_empty() {
            return Err(ApiError::invalid_update("body assets require a Page"));
        }
        return Ok(());
    }
    lock_and_sync_references(tx, identity.scope_id, object, seq, references).await
}

pub async fn admit_upload(db: &DatabaseConnection, object: Uuid, user: Uuid) -> Result<AssetLimits, ApiError> {
    let (identity, _) = page(db, object, user, PermissionLevel::Edit).await?;
    if identity.workspace_id.is_none() {
        scope::require_personal_write(db).await?;
    }
    Ok(AssetLimits::configured())
}
pub async fn limits_for(db: &DatabaseConnection, object: Uuid, user: Uuid) -> Result<AssetLimits, ApiError> {
    page(db, object, user, PermissionLevel::View).await?;
    Ok(AssetLimits::configured())
}
/// Only the public-link service calls this after authenticating a grant bound to this Page.
/// Staged and historical-only assets are never eligible for anonymous reads.
pub(super) async fn read_public_current(
    db: &DatabaseConnection,
    scope_id: Uuid,
    object: Uuid,
    asset: Uuid,
) -> Result<(AssetBytes, i64), ApiError> {
    let row = public_current_row(db, scope_id, object, asset).await?;
    let blob = db
        .query_one(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT bytes FROM flow_page_assets WHERE id=$1 AND object_id=$2 AND scope_id=$3",
            vec![asset.into(), object.into(), scope_id.into()],
        ))
        .await?
        .ok_or_else(super::shares::missing)?;
    let bytes: Vec<u8> = blob.try_get("", "bytes")?;
    if super::collab::bootstrap::content_hash(&bytes) != row.content_sha256 {
        return Err(ApiError::checksum_mismatch("Page body asset checksum mismatch"));
    }
    let head = row.head_seq;
    let metadata = result(row)?;
    let disposition = if metadata.kind == AssetKind::Attachment {
        attachment_disposition(&metadata.filename)?
    } else {
        "inline".into()
    };
    Ok((
        AssetBytes {
            metadata,
            bytes,
            disposition,
        },
        head,
    ))
}
async fn public_current_row(
    db: &DatabaseConnection,
    scope_id: Uuid,
    object: Uuid,
    asset: Uuid,
) -> Result<AssetRow, ApiError> {
    let row=AssetRow::find_by_statement(Statement::from_sql_and_values(DbBackend::Postgres,"SELECT a.id,a.object_id,a.kind,a.filename,a.content_type,a.byte_size,a.content_sha256,a.state,a.created_by,true AS referenced,d.head_seq FROM flow_page_assets a JOIN collab_documents d ON d.object_id=a.object_id WHERE a.id=$1 AND a.scope_id=$2 AND a.object_id=$3 AND a.state='active' AND EXISTS(SELECT 1 FROM flow_page_asset_refs r WHERE r.scope_id=a.scope_id AND r.object_id=a.object_id AND r.asset_id=a.id AND r.active AND r.last_seen_seq=d.head_seq)",vec![asset.into(),scope_id.into(),object.into()])).one(db).await?.ok_or_else(super::shares::missing)?;
    Ok(row)
}
pub(super) async fn confirm_public_current(
    db: &DatabaseConnection,
    scope_id: Uuid,
    object: Uuid,
    asset: Uuid,
    head: i64,
) -> Result<(), ApiError> {
    let current = public_current_row(db, scope_id, object, asset).await?;
    if current.head_seq != head {
        return Err(super::shares::missing());
    }
    Ok(())
}

#[cfg(test)]
mod api_tests {
    use super::*;
    use crate::{routes::context::tenant_fixture::exec, scratch_or_skip};
    use axum::{
        Extension, Router,
        body::{Body, to_bytes},
        http::{Request, StatusCode},
        routing::{get, post},
    };
    use collab_core::CollabEngine;
    use serde_json::Value;
    use tower::ServiceExt;
    fn state(db: DatabaseConnection) -> platform::app::AppState {
        platform::app::AppState {
            db,
            cfg: platform::config::AppConfig {
                app_name: "asset-api-unit".into(),
                bind_addr: "127.0.0.1:0".into(),
                database_url: platform::config::Secret::new("postgres://unused/unused"),
                jwt_secret: platform::config::Secret::new("asset-api-synthetic-secret"),
                jwt_access_ttl_seconds: 900,
                jwt_refresh_ttl_seconds: 3600,
                allow_insecure_cookies: false,
                collab_allowed_origins: vec![],
            },
            flow_permission_cache: platform::app::FlowPermissionCacheSlot::default(),
        }
    }
    async fn body_json(response: axum::response::Response) -> Value {
        serde_json::from_slice(&to_bytes(response.into_body(), 1024 * 1024).await.unwrap()).unwrap()
    }
    #[tokio::test]
    #[allow(
        clippy::print_stderr,
        reason = "report the platform skip without a tracing subscriber"
    )]
    async fn phase2_asset_http_actual_upload_owned_ref_copy_public_and_history() {
        if !cfg!(target_os = "linux") {
            eprintln!("SKIPPED: actual accepted-write path requires Linux");
            return;
        }
        let scratch = scratch_or_skip!(&format!("asset_api_{}", Uuid::new_v4().simple()));
        let db = &scratch.db;
        db.execute_unprepared(
            "UPDATE flow_runtime_capabilities SET personal_provisioning=true,personal_writes=true WHERE singleton",
        )
        .await
        .unwrap();
        let owner = Uuid::new_v4();
        exec(db,"INSERT INTO users(id,email,password_hash,name,role,is_active) VALUES($1,$2,'!','Synthetic asset API owner','user',true)",vec![owner.into(),format!("{owner}@asset-api.test").into()]).await;
        let home = scope::personal_home(db, owner).await.unwrap();
        let mut pages = Vec::new();
        for title in ["Synthetic source", "Synthetic target"] {
            let created = scope::create_personal(
                db,
                owner,
                scope::CreateInput {
                    editor_model: 5,
                    scope_id: home.0.id,
                    object_type: "page".into(),
                    parent_object_id: None,
                    project_id: None,
                    title: title.into(),
                    idempotency_key: Uuid::new_v4().to_string(),
                    message: None,
                    initial_fields: vec![],
                    initial_view: None,
                },
            )
            .await
            .unwrap();
            pages.push((
                Uuid::parse_str(
                    created
                        .pointer("/object/id")
                        .expect("response contains id")
                        .as_str()
                        .unwrap(),
                )
                .unwrap(),
                Uuid::parse_str(
                    created
                        .pointer("/object/document_id")
                        .expect("response contains document_id")
                        .as_str()
                        .unwrap(),
                )
                .unwrap(),
            ));
        }
        let (source, document) = *pages.first().unwrap();
        let (target, target_document) = *pages.get(1).unwrap();
        let state = state(db.clone());
        let claims = platform::auth::JwtClaims {
            sub: owner.to_string(),
            email: format!("{owner}@asset-api.test"),
            token_type: platform::auth::TokenType::Access,
            iat: 0,
            exp: usize::MAX,
            sv: 0,
        };
        // Axum resolves these path placeholders when routing requests.
        #[allow(clippy::literal_string_with_formatting_args)]
        let app = Router::new()
            .route(
                "/objects/{object}/commands",
                post(crate::routes::flow::post_flow_v2_command),
            )
            .route(
                "/objects/{object}/body-assets",
                post(crate::routes::flow::post_flow_v2_body_asset),
            )
            .route(
                "/objects/{object}/body-assets/copy",
                post(crate::routes::flow::post_flow_v2_body_asset_copy),
            )
            .route(
                "/objects/{object}/body-assets/{asset}",
                get(crate::routes::flow::get_flow_v2_body_asset),
            )
            .route(
                "/public/{token}/{asset}",
                get(crate::routes::flow::get_flow_v2_public_body_asset),
            )
            .layer(Extension(claims))
            .with_state(state.clone());
        let bytes = vec![0x89, b'P', b'N', b'G', 13, 10, 26, 10];
        let mut multipart=b"--asset-test\r\nContent-Disposition: form-data; name=\"kind\"\r\n\r\nimage\r\n--asset-test\r\nContent-Disposition: form-data; name=\"file\"; filename=\"proof.png\"\r\nContent-Type: text/html\r\n\r\n".to_vec();
        multipart.extend_from_slice(&bytes);
        multipart.extend_from_slice(b"\r\n--asset-test--\r\n");
        let upload = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(format!("/objects/{source}/body-assets"))
                    .header("content-type", "multipart/form-data; boundary=asset-test")
                    .header("Idempotency-Key", "asset-upload")
                    .body(Body::from(multipart))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(upload.status(), StatusCode::OK);
        let upload = body_json(upload).await;
        assert_eq!(upload.get("code").and_then(Value::as_i64), Some(0));
        assert_eq!(
            upload
                .pointer("/data/content_type")
                .expect("response contains content_type"),
            "image/png"
        );
        let asset = Uuid::parse_str(
            upload
                .pointer("/data/asset_id")
                .expect("response contains asset_id")
                .as_str()
                .unwrap(),
        )
        .unwrap();
        let preview = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri(format!("/objects/{source}/body-assets/{asset}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(preview.status(), StatusCode::OK);
        assert_eq!(preview.headers().get("content-type").unwrap(), "image/png");
        assert_eq!(
            preview.headers().get("x-flow-asset-sha256").unwrap(),
            super::super::collab::bootstrap::content_hash(&bytes).as_str()
        );
        assert_eq!(
            to_bytes(preview.into_body(), 1024).await.unwrap().as_ref(),
            bytes.as_slice()
        );
        let json_bytes = b"{\"hello\":\"world\"}";
        let mut attachment_body="--asset-file\r\nContent-Disposition: form-data; name=\"kind\"\r\n\r\nattachment\r\n--asset-file\r\nContent-Disposition: form-data; name=\"file\"; filename=\"说明.json\"\r\nContent-Type: application/json\r\n\r\n".as_bytes().to_vec();
        attachment_body.extend_from_slice(json_bytes);
        attachment_body.extend_from_slice(b"\r\n--asset-file--\r\n");
        let attachment = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(format!("/objects/{source}/body-assets"))
                    .header("content-type", "multipart/form-data; boundary=asset-file")
                    .header("Idempotency-Key", "json-upload")
                    .body(Body::from(attachment_body))
                    .unwrap(),
            )
            .await
            .unwrap();
        let attachment = body_json(attachment).await;
        assert_eq!(attachment.get("code").and_then(Value::as_i64), Some(0));
        let attachment_id = Uuid::parse_str(
            attachment
                .pointer("/data/asset_id")
                .expect("response contains asset_id")
                .as_str()
                .unwrap(),
        )
        .unwrap();
        let download = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri(format!("/objects/{source}/body-assets/{attachment_id}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(
            download.headers().get("content-type").unwrap(),
            "application/octet-stream"
        );
        assert!(
            download
                .headers()
                .get("content-disposition")
                .unwrap()
                .to_str()
                .unwrap()
                .starts_with("attachment;")
        );
        if let Ok(path) = std::env::var("PAGE_ASSET_HEADER_FIXTURE") {
            let headers = download
                .headers()
                .iter()
                .filter_map(|(key, value)| {
                    value
                        .to_str()
                        .ok()
                        .map(|value| (key.as_str().to_owned(), Value::String(value.into())))
                })
                .collect::<serde_json::Map<_, _>>();
            std::fs::write(
                path,
                serde_json::to_vec_pretty(&json!({"synthetic":true,"kind":"attachment","headers":headers})).unwrap(),
            )
            .unwrap();
        }
        assert_eq!(to_bytes(download.into_body(), 1024).await.unwrap().as_ref(), json_bytes);
        let link = super::super::scope_shares::create(db, source, owner, None)
            .await
            .unwrap();
        let public_uri = format!("/public/{}/{asset}", link.token);
        let staged_public = app
            .clone()
            .oneshot(Request::builder().uri(&public_uri).body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert!(staged_public.headers().get("x-flow-asset-sha256").is_none());
        assert_eq!(
            body_json(staged_public)
                .await
                .get("code")
                .expect("response contains code"),
            404
        );
        let inserted=app.clone().oneshot(Request::builder().method("POST").uri(format!("/objects/{source}/commands")).header("content-type","application/json").body(Body::from(json!({"command":{"type":"insert_body_asset","payload":{"block_id":"api-image","index":1,"asset_id":asset,"kind":"image","alt":"synthetic","caption":""}},"idempotency_key":"asset-sdk"}).to_string())).unwrap()).await.unwrap();
        let inserted = body_json(inserted).await;
        assert_eq!(inserted.get("code").and_then(Value::as_i64), Some(0));
        let engine = super::super::document::load_head_engine(db, document).await.unwrap();
        let cid = engine
            .semantic_snapshot()
            .unwrap()
            .nodes
            .iter()
            .find(|(_, node)| node.properties.get("block_type").is_some_and(|kind| kind == "image"))
            .unwrap()
            .0
            .to_string();
        let public = app
            .clone()
            .oneshot(Request::builder().uri(&public_uri).body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(public.status(), StatusCode::OK);
        assert_eq!(public.headers().get("x-content-type-options").unwrap(), "nosniff");
        assert_eq!(
            to_bytes(public.into_body(), 1024).await.unwrap().as_ref(),
            bytes.as_slice()
        );
        let mut foreign = super::super::document::load_head_engine(db, target_document)
            .await
            .unwrap();
        let before = foreign.frontier();
        foreign
            .insert_page_atom(
                "foreign",
                None,
                1,
                "image",
                &json!({"asset_id":asset,"alt":"","caption":"","indent":0}),
            )
            .unwrap();
        assert!(
            super::super::scope_write::accept(
                db,
                target,
                foreign.export_from(&before).unwrap(),
                super::super::scope_write::UpdateIntent {
                    user: owner,
                    update: Uuid::new_v4(),
                    client: "asset-api",
                    key: None,
                    expected: None,
                    command_body: None
                }
            )
            .await
            .is_err()
        );
        let copied = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(format!("/objects/{target}/body-assets/copy"))
                    .header("content-type", "application/json")
                    .header("Idempotency-Key", "copy-intent")
                    .body(Body::from(
                        json!({"source_object_id":source,"asset_id":asset}).to_string(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(copied.status(), StatusCode::OK);
        let copied = body_json(copied).await;
        let copy_id = Uuid::parse_str(
            copied
                .pointer("/data/asset_id")
                .expect("response contains asset_id")
                .as_str()
                .unwrap(),
        )
        .unwrap();
        assert_ne!(copy_id, asset);
        assert_eq!(
            copied.pointer("/data/object_id").expect("response contains object_id"),
            &json!(target)
        );
        let mut removed = super::super::document::load_head_engine(db, document).await.unwrap();
        let before = removed.frontier();
        removed
            .apply_content_operation(&collab_core::Operation::DeleteNode { id: cid.into() })
            .unwrap();
        super::super::scope_write::accept(
            db,
            source,
            removed.export_from(&before).unwrap(),
            super::super::scope_write::UpdateIntent {
                user: owner,
                update: Uuid::new_v4(),
                client: "asset-api",
                key: None,
                expected: None,
                command_body: None,
            },
        )
        .await
        .unwrap();
        let unlinked_public = app
            .clone()
            .oneshot(Request::builder().uri(&public_uri).body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert!(unlinked_public.headers().get("x-flow-asset-sha256").is_none());
        assert_eq!(
            body_json(unlinked_public)
                .await
                .get("code")
                .expect("response contains code"),
            404
        );
        assert_eq!(metadata(db, source, asset, owner).await.unwrap().state, "retained");
        super::super::scope_history::restore_page(&state, source, owner, 1, "asset-restore", None)
            .await
            .unwrap();
        let restored = app
            .oneshot(Request::builder().uri(public_uri).body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(restored.status(), StatusCode::OK);
        scratch.drop_self().await;
    }
}

/// Enrich SDK media labels from immutable private storage metadata; headers never read node attrs.
pub async fn normalize_command_payload(
    db: &DatabaseConnection,
    object: Uuid,
    actor: Uuid,
    command: &str,
    payload: &serde_json::Value,
) -> Result<serde_json::Value, ApiError> {
    if !matches!(command, "insert_body_asset" | "replace_body_asset") {
        return Ok(payload.clone());
    }
    let id = payload
        .get("asset_id")
        .and_then(serde_json::Value::as_str)
        .and_then(|value| Uuid::parse_str(value).ok())
        .ok_or_else(|| ApiError::invalid_update("asset_id is required"))?;
    let asset = metadata(db, object, id, actor).await?;
    if payload.get("kind").and_then(serde_json::Value::as_str) != Some(asset.kind.as_str()) {
        return Err(ApiError::invalid_update("asset kind does not match storage"));
    }
    let mut payload = payload.clone();
    let map = payload
        .as_object_mut()
        .ok_or_else(|| ApiError::invalid_update("asset payload must be an object"))?;
    map.insert("name".into(), serde_json::Value::String(asset.filename));
    map.insert("mime".into(), serde_json::Value::String(asset.content_type));
    Ok(payload)
}
