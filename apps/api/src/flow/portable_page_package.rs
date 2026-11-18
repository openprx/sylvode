//! Current-head packages contain typed Page content and current private body assets.
//! Promotion creates new object, document, block, and media identities in one transaction.

use std::{
    collections::{BTreeMap, BTreeSet},
    io::{Cursor, Read},
};

use collab_core::{CollabEngine, LoroCollabEngine, Operation, SemanticSnapshot};
use platform::timestamp::Timestamp;
use sea_orm::{ConnectionTrait, DatabaseConnection, DbBackend, Statement, TransactionTrait};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use uuid::Uuid;

use crate::error::{ApiError, ApiErrorKind};

use super::{
    body_assets::{self, AssetKind, ValidatedUpload},
    collab::{authz, authz::PermissionLevel, bootstrap::content_hash},
    package::{
        self, ENGINE_CRATE_VERSION, ENGINE_NAME, ENGINE_WIRE_FORMAT_VERSION, ExportPackageManifest, ExportPolicy,
        PORTABLE_PAGE_ASSET_COUNT, PORTABLE_PAGE_BYTES, PORTABLE_PAGE_CONTENT_BYTES, PORTABLE_PAGE_PATH,
        PORTABLE_PAGE_SCHEMA, PackageCounts, PackageEngine, PackageHistory, PackageMemberInput, PackageProducer,
        PackageSource, SEMANTIC_MODEL_V5,
    },
    page_transfer_ir::{self, Block, Content},
    repository::{self, NewCollabDocument, NewProjection},
    scope::{self, AuthorizationProof, ObjectIdentity},
};

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Asset {
    id: Uuid,
    kind: AssetKind,
    filename: String,
    content_type: String,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Page {
    title: String,
    blocks: Vec<Block>,
    assets: Vec<Asset>,
}

struct Candidate {
    hash: String,
    source_scope: Uuid,
    source_object: Uuid,
    title: String,
    blocks: Vec<Block>,
    uploads: BTreeMap<Uuid, ValidatedUpload>,
}

fn unsupported(reason: &'static str, message: &'static str) -> ApiError {
    ApiError::typed_with_details(ApiErrorKind::UnsupportedFormat, message, json!({"reason":reason}))
}

fn invalid_content() -> ApiError {
    unsupported(
        "portable_page_invalid_content",
        "Page content has an unsupported or invalid shape",
    )
}

fn linked_page_unavailable() -> ApiError {
    ApiError::policy_rejected_with_details(
        "A linked Page is unavailable in this scope",
        json!({"reason":"portable_page_link_unavailable"}),
    )
}

fn size_limit() -> ApiError {
    ApiError::limit_exceeded(
        "Portable Page package exceeds the archive limit",
        "portable_page_package_bytes",
        Some(json!(PORTABLE_PAGE_BYTES)),
        None,
        None,
    )
}

// Result::map_err passes the engine error by value.
#[allow(clippy::needless_pass_by_value)]
fn map(error: collab_core::CollabError) -> ApiError {
    super::command::map_collab_error(&error)
}

async fn require_enabled<C: ConnectionTrait>(db: &C) -> Result<(), ApiError> {
    if !scope::capabilities(db).await?.portable_page_packages_supported {
        return Err(ApiError::typed_with_details(
            ApiErrorKind::FeatureDisabled,
            "Portable Page packages are disabled",
            json!({"reason":"portable_page_packages_disabled"}),
        ));
    }
    scope::require_reader_ready(db).await
}

pub(super) async fn page<C: ConnectionTrait>(
    db: &C,
    id: Uuid,
    actor: Uuid,
    level: PermissionLevel,
) -> Result<(ObjectIdentity, AuthorizationProof), ApiError> {
    let mut proof = scope::authorize(db, id, actor, level).await?;
    let identity = scope::identity(db, id).await?;
    if identity.object_type != "page" || !proof.resource_active {
        return Err(ApiError::policy_rejected_with_details(
            "Portable packages require an active Page",
            json!({"reason":"portable_page_required"}),
        ));
    }
    // Fence the operation's required authority, including a View on a read-only scope.
    proof.level = level;
    Ok((identity, proof))
}

pub(super) fn asset_path(id: Uuid) -> String {
    format!("assets/{id}.bin")
}

pub(super) const fn asset_kind(block: &Block) -> Option<AssetKind> {
    match block.content {
        Content::Image { .. } => Some(AssetKind::Image),
        Content::Attachment { .. } => Some(AssetKind::Attachment),
        _ => None,
    }
}

fn linked_ids(blocks: &[Block]) -> BTreeSet<Uuid> {
    blocks
        .iter()
        .filter_map(|block| match block.content {
            Content::ChildPage { child_page_id } => Some(child_page_id),
            _ => None,
        })
        .collect()
}

async fn link_proofs<C: ConnectionTrait>(
    db: &C,
    scope_id: Uuid,
    actor: Uuid,
    blocks: &[Block],
) -> Result<Vec<AuthorizationProof>, ApiError> {
    let mut proofs = Vec::new();
    for id in linked_ids(blocks) {
        let (link, proof) = page(db, id, actor, PermissionLevel::View)
            .await
            .map_err(|_| linked_page_unavailable())?;
        if link.scope_id != scope_id {
            return Err(linked_page_unavailable());
        }
        proofs.push(proof);
    }
    Ok(proofs)
}

async fn fence(tx: &sea_orm::DatabaseTransaction, proofs: &[AuthorizationProof]) -> Result<(), ApiError> {
    scope::fence_many(tx, proofs).await?;
    for proof in proofs {
        let (identity, _) = page(tx, proof.object_id, proof.principal_id, proof.level).await?;
        if identity.scope_id != proof.scope_id {
            return Err(super::policy::authorization_read_unstable());
        }
        if proof.level >= PermissionLevel::Edit {
            scope::fence(tx, proof).await?;
        }
    }
    Ok(())
}

pub(super) fn canonicalize(blocks: &mut [Block]) -> Result<(), ApiError> {
    let mut identifiers = BTreeMap::new();
    for (index, block) in blocks.iter_mut().enumerate() {
        let id = format!("b{index}");
        let prior = std::mem::replace(&mut block.source_cid, id);
        if identifiers.insert(prior, block.source_cid.clone()).is_some() {
            return Err(invalid_content());
        }
        if let Some(parent) = &mut block.parent_cid {
            *parent = identifiers.get(parent).ok_or_else(invalid_content)?.clone();
        }
    }
    Ok(())
}

pub(super) fn read_blocks(snapshot: &SemanticSnapshot) -> Result<Vec<Block>, ApiError> {
    let blocks = page_transfer_ir::read(snapshot).map_err(|_| invalid_content())?;
    if snapshot.nodes.values().filter(|node| !node.deleted).count() != blocks.len() {
        return Err(unsupported(
            "portable_page_unrepresented_reference",
            "This Page contains a block or embedded object with no portable mapping",
        ));
    }
    Ok(blocks)
}

pub(super) fn validate_graph(title: &str, blocks: &[Block]) -> Result<(), ApiError> {
    if title.chars().count() > 500 || blocks.is_empty() || blocks.len() > collab_core::limits::DOCUMENT_BLOCK_COUNT_MAX
    {
        return Err(invalid_content());
    }
    let mut parents = Vec::new();
    for (index, block) in blocks.iter().enumerate() {
        let atom = matches!(
            block.content,
            Content::Divider | Content::Image { .. } | Content::Attachment { .. } | Content::ChildPage { .. }
        );
        if block.source_cid != format!("b{index}")
            || block.preorder as usize != index
            || block.text.chars().count() > collab_core::limits::TEXT_BLOCK_CHARS_MAX
            || (atom && (!block.text.is_empty() || !block.spans.is_empty()))
        {
            return Err(invalid_content());
        }
        if let Some(parent) = &block.parent_cid {
            let depth = parents.iter().position(|id| id == parent).ok_or_else(invalid_content)?;
            if depth >= 7 {
                return Err(invalid_content());
            }
            parents.truncate(depth + 1);
        } else {
            parents.clear();
        }
        parents.push(if atom { String::new() } else { block.source_cid.clone() });
    }
    Ok(())
}

pub async fn export(db: &DatabaseConnection, object: Uuid, actor: Uuid) -> Result<Vec<u8>, ApiError> {
    require_enabled(db).await?;
    let (source, source_proof) = page(db, object, actor, PermissionLevel::View).await?;
    let head = db
        .query_one(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT id,head_seq,head_frontier FROM collab_documents WHERE object_id=$1",
            vec![object.into()],
        ))
        .await?
        .ok_or_else(ApiError::internal)?;
    let document: Uuid = head.try_get("", "id")?;
    let seq: i64 = head.try_get("", "head_seq")?;
    let frontier: Vec<u8> = head.try_get("", "head_frontier")?;
    let boot = super::collab::bootstrap::load(db, document).await?;
    if boot.head_seq != seq || boot.head_frontier != frontier {
        return Err(super::policy::authorization_read_unstable());
    }
    let mut engine = super::document::engine_from_boot(&boot)?;
    if !engine.has_editor_doc() {
        engine.initialize_page_editor().map_err(|_| invalid_content())?;
    }
    let title = engine.title().map_err(map)?;
    let blocks = read_blocks(&engine.semantic_snapshot().map_err(map)?)?;
    let mut portable_blocks = blocks.clone();
    canonicalize(&mut portable_blocks)?;
    validate_graph(&title, &portable_blocks)?;
    reconstruct(&title, &portable_blocks, Uuid::new_v4())?;
    let mut proofs = link_proofs(db, source.scope_id, actor, &blocks).await?;
    proofs.push(source_proof);

    let tx = db.begin().await?;
    require_enabled(&tx).await?;
    fence(&tx, &proofs).await?;
    let current = tx
        .query_one(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT head_seq,head_frontier FROM collab_documents WHERE id=$1 AND object_id=$2 FOR SHARE",
            vec![document.into(), object.into()],
        ))
        .await?
        .ok_or_else(ApiError::internal)?;
    if current.try_get::<i64>("", "head_seq")? != seq || current.try_get::<Vec<u8>>("", "head_frontier")? != frontier {
        return Err(super::policy::authorization_read_unstable());
    }

    if tx
        .query_one(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT 1 FROM flow_bridge_references WHERE source_object_id=$1 AND removed_at IS NULL LIMIT 1",
            vec![object.into()],
        ))
        .await?
        .is_some()
    {
        return Err(unsupported(
            "portable_page_unrepresented_reference",
            "This Page has an external Reference or Embed that cannot be included in a portable package",
        ));
    }
    let mut assets = BTreeMap::new();
    let mut members = Vec::new();
    let mut asset_total = 0usize;
    for block in &blocks {
        if let Content::ChildPage { child_page_id } = block.content {
            let link = tx.query_one(Statement::from_sql_and_values(DbBackend::Postgres,
                "SELECT 1 FROM flow_page_child_refs WHERE scope_id=$1 AND source_object_id=$2 AND block_cid=$3 AND child_page_id=$4 AND active AND last_seen_seq=$5",
                vec![source.scope_id.into(), object.into(), block.source_cid.clone().into(), child_page_id.into(), seq.into()]
            )).await?;
            if link.is_none() {
                return Err(linked_page_unavailable());
            }
        }
        let (Some(id), Some(kind)) = (block.asset(), asset_kind(block)) else {
            continue;
        };
        let row = tx
            .query_one(Statement::from_sql_and_values(
                DbBackend::Postgres,
                "SELECT a.filename,a.content_type,a.byte_size,a.content_sha256 FROM flow_page_assets a
             JOIN flow_page_asset_refs r ON (r.scope_id,r.object_id,r.asset_id)=(a.scope_id,a.object_id,a.id)
             WHERE a.scope_id=$1 AND a.object_id=$2 AND a.id=$3 AND a.kind=$4 AND a.state='active'
               AND r.block_cid=$5 AND r.active AND r.last_seen_seq=$6 FOR SHARE OF a",
                vec![
                    source.scope_id.into(),
                    object.into(),
                    id.into(),
                    kind.as_str().into(),
                    block.source_cid.clone().into(),
                    seq.into(),
                ],
            ))
            .await?
            .ok_or_else(|| unsupported("portable_page_asset_unavailable", "A current Page asset is unavailable"))?;
        if let std::collections::btree_map::Entry::Vacant(entry) = assets.entry(id) {
            let size = usize::try_from(row.try_get::<i32>("", "byte_size")?).map_err(|_| ApiError::internal())?;
            asset_total = asset_total.saturating_add(size);
            if asset_total > PORTABLE_PAGE_BYTES || members.len() == PORTABLE_PAGE_ASSET_COUNT {
                return Err(size_limit());
            }
            let data = tx
                .query_one(Statement::from_sql_and_values(
                    DbBackend::Postgres,
                    "SELECT bytes FROM flow_page_assets WHERE id=$1",
                    vec![id.into()],
                ))
                .await?
                .ok_or_else(ApiError::internal)?
                .try_get::<Vec<u8>>("", "bytes")?;
            let filename: String = row.try_get("", "filename")?;
            let content_type: String = row.try_get("", "content_type")?;
            let upload = body_assets::validate_bytes(
                kind,
                &filename,
                Some(&content_type),
                data,
                body_assets::AssetLimits::configured().max_bytes,
            )?;
            if upload.bytes.len() != size
                || upload.sha256 != row.try_get::<String>("", "content_sha256")?
                || upload.content_type != content_type
            {
                return Err(ApiError::checksum_mismatch(
                    "Stored Page asset disagrees with its metadata",
                ));
            }
            entry.insert(Asset {
                id,
                kind,
                filename,
                content_type,
            });
            members.push(PackageMemberInput {
                path: asset_path(id),
                kind: "page_asset".into(),
                bytes: upload.bytes,
            });
        }
    }
    let content = Page {
        title,
        blocks: portable_blocks,
        assets: assets.into_values().collect(),
    };
    let content = serde_jcs::to_vec(&content).map_err(|_| ApiError::internal())?;
    if content.len() as u64 > PORTABLE_PAGE_CONTENT_BYTES
        || asset_total.saturating_add(content.len()) > PORTABLE_PAGE_BYTES
    {
        return Err(size_limit());
    }
    members.push(PackageMemberInput {
        path: PORTABLE_PAGE_PATH.into(),
        kind: "page_content".into(),
        bytes: content,
    });
    let now = Timestamp::now().to_rfc3339();
    let manifest = ExportPackageManifest {
        schema: PORTABLE_PAGE_SCHEMA.into(),
        package_id: Uuid::new_v4().to_string(),
        created_at: now.clone(),
        producer: PackageProducer {
            product: "sylvode".into(),
            version: env!("CARGO_PKG_VERSION").into(),
            source_head: env!("OPENPR_EMBEDDED_GIT_COMMIT").into(),
        },
        source: PackageSource {
            workspace_id: source.workspace_id.map(|id| id.to_string()),
            scope_id: Some(source.scope_id.to_string()),
            scope: "object".into(),
            root_object_ids: vec![object.to_string()],
        },
        flow_schema_version: 2,
        engine: PackageEngine {
            name: ENGINE_NAME.into(),
            crate_version: ENGINE_CRATE_VERSION.into(),
            wire_format_version: ENGINE_WIRE_FORMAT_VERSION,
            semantic_model: Some(SEMANTIC_MODEL_V5.into()),
        },
        history: PackageHistory {
            included: false,
            snapshot_history: Some("excluded".into()),
            through_seq_by_document: BTreeMap::new(),
        },
        counts: PackageCounts {
            objects: 1,
            documents: 1,
            ..PackageCounts::default()
        },
        members: Vec::new(),
        export_policy: ExportPolicy {
            complete: true,
            permission_snapshot_at: now,
        },
        projects: Vec::new(),
        warnings: vec!["current_page_content_only".into(), "linked_pages_are_references".into()],
    };
    let bytes = package::build_package(manifest, members)?.bytes;
    if bytes.len() > PORTABLE_PAGE_BYTES {
        return Err(size_limit());
    }
    tx.commit().await?;
    Ok(bytes)
}

fn decode(bytes: &[u8], expected: Option<&str>) -> Result<Candidate, ApiError> {
    let verified = package::verify_portable_page(bytes, expected)?;
    let hash = verified.package_sha256;
    let manifest = verified.manifest;
    let source_scope = manifest
        .source
        .scope_id
        .as_deref()
        .and_then(|id| Uuid::parse_str(id).ok())
        .ok_or_else(invalid_content)?;
    let source_object = manifest
        .source
        .root_object_ids
        .first()
        .and_then(|id| Uuid::parse_str(id).ok())
        .ok_or_else(invalid_content)?;
    let mut archive = zip::ZipArchive::new(Cursor::new(bytes)).map_err(|_| invalid_content())?;
    let page: Page = {
        let member = archive.by_name(PORTABLE_PAGE_PATH).map_err(|_| invalid_content())?;
        serde_json::from_reader(member).map_err(|_| invalid_content())?
    };
    validate_graph(&page.title, &page.blocks)?;
    if page.assets.len() + 1 != manifest.members.len() {
        return Err(invalid_content());
    }
    let mut uploads = BTreeMap::new();
    let expected_assets = page
        .blocks
        .iter()
        .filter_map(|block| Some((block.asset()?, asset_kind(block)?)))
        .collect::<Vec<_>>();
    let ids = expected_assets.iter().map(|(id, _)| *id).collect::<BTreeSet<_>>();
    let maximum = body_assets::AssetLimits::configured().max_bytes;
    for asset in page.assets {
        if !ids.contains(&asset.id) {
            return Err(invalid_content());
        }
        let mut member = archive.by_name(&asset_path(asset.id)).map_err(|_| invalid_content())?;
        let size = member.size();
        if size > maximum {
            return Err(size_limit());
        }
        let mut data = Vec::new();
        member.read_to_end(&mut data).map_err(|_| invalid_content())?;
        let upload =
            body_assets::validate_bytes(asset.kind, &asset.filename, Some(&asset.content_type), data, maximum)?;
        if upload.content_type != asset.content_type
            || upload.filename != asset.filename
            || uploads.insert(asset.id, upload).is_some()
        {
            return Err(invalid_content());
        }
    }
    if uploads.len() != ids.len()
        || expected_assets
            .iter()
            .any(|(id, kind)| uploads.get(id).is_none_or(|asset| asset.kind != *kind))
    {
        return Err(invalid_content());
    }
    Ok(Candidate {
        hash,
        source_scope,
        source_object,
        title: page.title,
        blocks: page.blocks,
        uploads,
    })
}

async fn destination(
    db: &DatabaseConnection,
    object: Uuid,
    actor: Uuid,
    candidate: &Candidate,
) -> Result<(ObjectIdentity, Vec<AuthorizationProof>), ApiError> {
    require_enabled(db).await?;
    let (parent, parent_proof) = page(db, object, actor, PermissionLevel::Edit).await?;
    if parent.scope_id != candidate.source_scope {
        return Err(ApiError::policy_rejected_with_details(
            "Portable Page import requires the original scope",
            json!({"reason":"portable_page_scope_mismatch"}),
        ));
    }
    super::page_transfers::require_transfer_write(db, &parent).await?;
    let mut proofs = link_proofs(db, parent.scope_id, actor, &candidate.blocks).await?;
    proofs.push(parent_proof);
    Ok((parent, proofs))
}

pub(super) fn same_portable_blocks(before: &[Block], after: &[Block]) -> bool {
    fn spans(block: &Block) -> Vec<collab_core::page_inline::InlineSpan> {
        let mut spans = block.spans.clone();
        spans.sort_by(|left, right| {
            (left.mark, &left.href, left.start, left.end).cmp(&(right.mark, &right.href, right.start, right.end))
        });
        let mut merged: Vec<collab_core::page_inline::InlineSpan> = Vec::new();
        for span in spans {
            if let Some(last) = merged
                .last_mut()
                .filter(|last| last.mark == span.mark && last.href == span.href)
                && last.end == span.start
            {
                last.end = span.end;
            } else {
                merged.push(span);
            }
        }
        merged
    }
    before.len() == after.len()
        && before.iter().zip(after).all(|(left, right)| {
            left.source_cid == right.source_cid
                && left.parent_cid == right.parent_cid
                && left.preorder == right.preorder
                && left.content == right.content
                && left.text == right.text
                && spans(left) == spans(right)
        })
}

pub(super) fn reconstruct(
    title: &str,
    blocks: &[Block],
    operation: Uuid,
) -> Result<(LoroCollabEngine, SemanticSnapshot), ApiError> {
    let mut engine = LoroCollabEngine::new_empty(rand::random());
    engine.set_title(title).map_err(map)?;
    engine.initialize_page_editor().map_err(map)?;
    engine.initialize_page_structure_model(5).map_err(map)?;
    let original = read_blocks(&engine.semantic_snapshot().map_err(map)?)?;
    let placeholder = original.first().ok_or_else(ApiError::internal)?;
    super::page_transfers::insert_selected(&mut engine, blocks, None, 0, operation)?;
    engine
        .apply_content_operation(&Operation::DeleteNode {
            id: placeholder.source_cid.as_str().into(),
        })
        .map_err(map)?;
    let semantic = engine.semantic_snapshot().map_err(map)?;
    let mut reconstructed = read_blocks(&semantic)?;
    canonicalize(&mut reconstructed)?;
    if !same_portable_blocks(&reconstructed, blocks) || engine.title().map_err(map)? != title {
        return Err(unsupported(
            "portable_page_roundtrip_mismatch",
            "Page content cannot be represented losslessly",
        ));
    }
    Ok((engine, semantic))
}

pub async fn preview(
    db: &DatabaseConnection,
    object: Uuid,
    actor: Uuid,
    bytes: &[u8],
    export_review: bool,
) -> Result<Value, ApiError> {
    require_enabled(db).await?;
    let (context, proof) = page(
        db,
        object,
        actor,
        if export_review {
            PermissionLevel::View
        } else {
            PermissionLevel::Edit
        },
    )
    .await?;
    let candidate = decode(bytes, None)?;
    let (parent, proofs) = if export_review {
        if context.scope_id != candidate.source_scope || object != candidate.source_object {
            return Err(ApiError::policy_rejected_with_details(
                "Export review requires its source Page",
                json!({"reason":"portable_page_source_mismatch"}),
            ));
        }
        let mut proofs = link_proofs(db, context.scope_id, actor, &candidate.blocks).await?;
        proofs.push(proof);
        (context, proofs)
    } else {
        destination(db, object, actor, &candidate).await?
    };
    reconstruct(&candidate.title, &candidate.blocks, Uuid::new_v4())?;
    let tx = db.begin().await?;
    require_enabled(&tx).await?;
    fence(&tx, &proofs).await?;
    if !export_review {
        super::page_transfers::require_transfer_write(&tx, &parent).await?;
        authz::ensure_scope_parent_can_adopt_child(&tx, parent.scope_id, object).await?;
    }
    tx.commit().await?;
    let assets = candidate
        .uploads
        .values()
        .map(|asset| json!({"filename":asset.filename,"size":asset.bytes.len()}))
        .collect::<Vec<_>>();
    let blocks = candidate
        .blocks
        .iter()
        .take(30)
        .map(|block| {
            let kind = match &block.content {
                Content::Paragraph => "paragraph",
                Content::Heading { .. } => "heading",
                Content::Bullet => "bullet",
                Content::Ordered { .. } => "ordered",
                Content::Todo { .. } => "todo",
                Content::Quote => "quote",
                Content::Toggle { .. } => "toggle",
                Content::Code => "code",
                Content::Divider => "divider",
                Content::Image { .. } => "image",
                Content::Attachment { .. } => "attachment",
                Content::ChildPage { .. } => "child_page",
            };
            let text = match &block.content {
                Content::Image { alt, .. } => alt,
                Content::Attachment { name, .. } => name,
                _ => &block.text,
            }
            .chars()
            .take(160)
            .collect::<String>();
            json!({"kind":kind,"text":text})
        })
        .collect::<Vec<_>>();
    Ok(
        json!({"title":candidate.title,"block_count":candidate.blocks.len(),"asset_count":assets.len(),
        "asset_bytes":candidate.uploads.values().map(|asset| asset.bytes.len()).sum::<usize>(),
        "linked_page_count":linked_ids(&candidate.blocks).len(),"blocks":blocks,"assets":assets,
        "package_sha256":candidate.hash}),
    )
}

pub async fn import(
    db: &DatabaseConnection,
    parent_id: Uuid,
    actor: Uuid,
    bytes: &[u8],
    expected: &str,
    key: &str,
) -> Result<Value, ApiError> {
    require_enabled(db).await?;
    let (preflight, _) = page(db, parent_id, actor, PermissionLevel::Edit).await?;
    if key.trim().is_empty() || key.len() > 128 {
        return Err(ApiError::invalid_update("operation key must contain 1..128 bytes"));
    }
    if content_hash(bytes) != expected {
        return Err(ApiError::checksum_mismatch(
            "Portable Page package changed since preview",
        ));
    }
    let body = json!({"operation":"portable_page_import","parent_object_id":parent_id,"package_sha256":expected});
    if let Some(prior) = super::scope_commands::replay(db, preflight.scope_id, actor, key, &body).await? {
        return Ok(prior);
    }
    let candidate = decode(bytes, Some(expected))?;
    let (parent, proofs) = destination(db, parent_id, actor, &candidate).await?;
    let object = Uuid::new_v4();
    let document = Uuid::new_v4();
    let mapping = candidate
        .uploads
        .keys()
        .map(|source| (*source, Uuid::new_v4()))
        .collect::<BTreeMap<_, _>>();
    let mut blocks = candidate.blocks;
    for block in &mut blocks {
        block.remap_asset(&mapping).map_err(|_| invalid_content())?;
    }
    let (engine, semantic) = reconstruct(&candidate.title, &blocks, object)?;
    let snapshot = engine.export_snapshot().map_err(map)?;
    super::collab::bootstrap::check_bootstrap_decoded_bytes(snapshot.len() as u64)?;
    let frontier = engine.frontier().as_bytes().to_vec();
    let refs = body_assets::typed_references(&semantic)?;
    let child_refs = super::child_pages::typed_references(&semantic)?;
    let registry = super::block_registry::prepare(&semantic)?;
    let tx = db.begin().await?;
    require_enabled(&tx).await?;
    super::page_transfers::require_transfer_write(&tx, &parent).await?;
    fence(&tx, &proofs).await?;
    if let Some(prior) = super::scope_commands::lock_and_replay(&tx, parent.scope_id, actor, key, &body).await? {
        tx.commit().await?;
        return Ok(prior);
    }
    authz::ensure_scope_parent_can_adopt_child(&tx, parent.scope_id, parent.id).await?;
    tx.execute(Statement::from_sql_and_values(DbBackend::Postgres,
        "INSERT INTO flow_objects(id,scope_id,workspace_id,project_id,object_type,parent_id,created_by,governance_metadata) VALUES($1,$2,$3,$4,'page',$5,$6,'{}'::jsonb)",
        vec![object.into(),parent.scope_id.into(),parent.workspace_id.into(),parent.project_id.into(),parent.id.into(),actor.into()])).await?;
    repository::insert_collab_document(
        &tx,
        &NewCollabDocument {
            id: document,
            object_id: object,
            format_version: "loro-1".into(),
            snapshot,
            frontier: frontier.clone(),
        },
    )
    .await?;
    repository::insert_projection(
        &tx,
        &NewProjection {
            object_id: object,
            document_seq: 0,
            document_frontier: frontier,
            title: candidate.title,
            state: super::projection::state_json(&semantic).map_err(|_| ApiError::internal())?,
            plain_text: super::projection::plain_text(&semantic),
        },
    )
    .await?;
    for (source, upload) in candidate.uploads {
        let id = *mapping.get(&source).ok_or_else(ApiError::internal)?;
        let size = i32::try_from(upload.bytes.len()).map_err(|_| size_limit())?;
        tx.execute(Statement::from_sql_and_values(DbBackend::Postgres,
            "INSERT INTO flow_page_assets(id,scope_id,object_id,kind,filename,content_type,byte_size,content_sha256,bytes,created_by) VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9,$10)",
            vec![id.into(),parent.scope_id.into(),object.into(),upload.kind.as_str().into(),upload.filename.into(),upload.content_type.into(),size.into(),upload.sha256.into(),upload.bytes.into(),actor.into()]
        )).await?;
    }
    super::block_registry::reconcile_accepted(&tx, object, 0, &registry).await?;
    body_assets::reconcile_accepted(&tx, object, 0, &refs).await?;
    super::child_pages::reconcile_accepted(&tx, object, Some(actor), 0, &child_refs).await?;
    let identity = scope::identity(&tx, object).await?;
    let result = json!({"object_id":object,"document_id":document,"asset_count":mapping.len()});
    let event = super::scope_event::insert_for_object(
        &tx,
        &identity,
        actor,
        "flow.object.created",
        "flow_object",
        object,
        key,
        json!({"object_id":object,"object_type":"page","parent_object_id":parent.id}),
        json!({}),
    )
    .await?;
    super::scope_commands::record(&tx, parent.scope_id, actor, key, body, object, event, result.clone()).await?;
    tx.commit().await?;
    Ok(result)
}
