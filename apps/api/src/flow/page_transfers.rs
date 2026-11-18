//! Candidate service for reviewed, atomic transfers between two editable Pages.
use super::{collab::authz::PermissionLevel, page_transfer_contract as wire, page_transfer_ir as ir, scope};
use crate::error::ApiError;
use base64::{Engine as _, engine::general_purpose::STANDARD as B64};
use collab_core::CollabEngine;
use sea_orm::{
    ConnectionTrait, DatabaseConnection, DatabaseTransaction, DbBackend, FromQueryResult, Statement, TransactionTrait,
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use uuid::Uuid;
#[derive(Clone, Debug, Serialize, Deserialize)]
struct Selected {
    cid: String,
    logical_id: Uuid,
    generation: i64,
    parent_cid: Option<String>,
    preorder: u32,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
struct SelectionMetadata {
    blocks: Vec<Selected>,
    source_parent: Option<String>,
    source_index: u32,
    target_parent: Option<String>,
    target_index: u32,
    #[serde(default)]
    reviewed_shares: Vec<super::page_transfer_prepare::ShareMemberMetadata>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
struct AssetMap {
    source: Uuid,
    target: Uuid,
    bytes: u64,
    sha256: String,
}
#[derive(Debug, Deserialize, Eq, FromQueryResult, PartialEq, Serialize)]
struct CopiedPlacement {
    physical_cid: String,
    logical_id: Uuid,
    generation: i64,
}

#[derive(FromQueryResult)]
struct OperationRow {
    id: Uuid,
    actor_id: Uuid,
    source_object_id: Uuid,
    target_object_id: Uuid,
    kind: String,
    state: String,
    prepare_body: Value,
    source_frontier: Vec<u8>,
    target_frontier: Vec<u8>,
    selection_digest: String,
    selection_metadata: Value,
    asset_mapping: Value,
    receipt: Option<Value>,
    copied_placements: Option<Value>,
    expires_at: platform::timestamp::Timestamp,
    failure_reason: Option<String>,
    commit_key: Option<String>,
    commit_body: Option<Value>,
    undo_key: Option<String>,
    undo_body: Option<Value>,
}
fn error(reason: &str) -> ApiError {
    ApiError::policy_rejected_with_details(
        "Page transfer unavailable",
        json!({"reason":reason,"write_state":"not_applied"}),
    )
}
// Result::map_err passes the engine error by value.
#[allow(clippy::needless_pass_by_value)]
fn map(error: collab_core::CollabError) -> ApiError {
    super::command::map_collab_error(&error)
}
fn parse_error(_: ir::Error) -> ApiError {
    error("transfer_unsupported_selected_content")
}
fn kind(value: &str) -> wire::Kind {
    if value == "move" {
        wire::Kind::Move
    } else {
        wire::Kind::Copy
    }
}
fn state(value: &str) -> Result<wire::State, ApiError> {
    serde_json::from_value(json!(value)).map_err(|_| ApiError::internal())
}
fn status(row: &OperationRow, details: bool) -> Result<Value, ApiError> {
    serde_json::to_value(wire::Status {
        transfer_id: row.id,
        kind: kind(&row.kind),
        state: state(&row.state)?,
        expires_at: row.expires_at.to_rfc3339(),
        receipt: if details {
            row.receipt
                .clone()
                .map(serde_json::from_value::<wire::Receipt>)
                .transpose()
                .map_err(|_| ApiError::internal())?
                .map(|mut receipt| {
                    if row.kind == "copy" && row.copied_placements.is_none() {
                        receipt.undo = wire::Undo::Unavailable;
                    }
                    receipt
                })
        } else {
            None
        },
        failure_reason: if details { row.failure_reason.clone() } else { None },
    })
    .map_err(|_| ApiError::internal())
}
async fn load<C: ConnectionTrait>(db: &C, id: Uuid, actor: Uuid) -> Result<OperationRow, ApiError> {
    OperationRow::find_by_statement(Statement::from_sql_and_values(DbBackend::Postgres,"SELECT id,actor_id,source_object_id,target_object_id,kind,state,prepare_body,source_frontier,target_frontier,selection_digest,selection_metadata,asset_mapping,receipt,copied_placements,expires_at,failure_reason,commit_key,commit_body,undo_key,undo_body FROM flow_page_block_transfers WHERE id=$1 AND actor_id=$2",vec![id.into(),actor.into()])).one(db).await?.ok_or_else(||error("transfer_unavailable"))
}
async fn proofs(
    db: &DatabaseConnection,
    actor: Uuid,
    source: Uuid,
    target: Uuid,
    kind: wire::Kind,
) -> Result<(scope::AuthorizationProof, scope::AuthorizationProof), ApiError> {
    if source == target {
        return Err(error("transfer_same_page_use_native_operation"));
    }
    let levels = if kind == wire::Kind::Move {
        (PermissionLevel::FullAccess, PermissionLevel::FullAccess)
    } else {
        (PermissionLevel::View, PermissionLevel::Edit)
    };
    let a = scope::authorize(db, source, actor, levels.0).await?;
    let b = scope::authorize(db, target, actor, levels.1).await?;
    if a.scope_id != b.scope_id {
        return Err(error("transfer_cross_scope_unsupported"));
    }
    if !a.resource_active || !b.resource_active {
        return Err(error("transfer_page_inactive"));
    }
    Ok((a, b))
}
// Inline child blocks are links, including links created by a finalized reservation.
async fn child_link_proofs(
    tx: &DatabaseTransaction,
    actor: Uuid,
    destination: Uuid,
    reverse_destination: Option<Uuid>,
    selected: &[ir::Block],
) -> Result<Vec<scope::AuthorizationProof>, ApiError> {
    let ids = selected
        .iter()
        .filter_map(|block| match block.content {
            ir::Content::ChildPage { child_page_id } => Some(child_page_id),
            _ => None,
        })
        .collect::<std::collections::BTreeSet<_>>();
    let mut proofs = Vec::with_capacity(ids.len());
    for id in ids {
        // A Move must be reversible to its original Page without creating a self-link.
        if id == destination || Some(id) == reverse_destination {
            return Err(error("transfer_child_reference_self"));
        }
        let target = scope::identity(tx, id)
            .await
            .map_err(|_| error("transfer_child_reference_unavailable"))?;
        let proof = scope::authorize(tx, id, actor, PermissionLevel::View)
            .await
            .map_err(|_| error("transfer_child_reference_unavailable"))?;
        if target.object_type != "page" || !proof.resource_active {
            return Err(error("transfer_child_reference_unavailable"));
        }
        proofs.push(proof);
    }
    Ok(proofs)
}

async fn lock_transfer_objects(
    tx: &DatabaseTransaction,
    source: Uuid,
    target: Uuid,
    proofs: &[scope::AuthorizationProof],
) -> Result<(), ApiError> {
    let objects = [source, target]
        .into_iter()
        .chain(proofs.iter().map(|proof| proof.object_id))
        .collect::<std::collections::BTreeSet<_>>();
    for object in objects {
        let row = tx
            .query_one(Statement::from_sql_and_values(
                DbBackend::Postgres,
                "SELECT lifecycle_status FROM flow_objects WHERE id=$1 FOR UPDATE",
                vec![object.into()],
            ))
            .await?;
        if row
            .map(|row| row.try_get::<String>("", "lifecycle_status"))
            .transpose()?
            .as_deref()
            != Some("active")
        {
            if object == source || object == target {
                return Err(error("transfer_page_inactive"));
            }
            return Err(error("transfer_child_reference_unavailable"));
        }
    }
    Ok(())
}

async fn affected_shares<C: ConnectionTrait>(
    db: &C,
    blocks: &[Selected],
) -> Result<Vec<super::page_transfer_prepare::ShareMemberMetadata>, ApiError> {
    let ids = blocks.iter().map(|block| block.logical_id).collect::<Vec<_>>();
    super::page_transfer_prepare::affected_shares(db, &ids)
        .await
        .map_err(|_| ApiError::internal())
}

fn selection_digest(
    blocks: &[ir::Block],
    shares: &[super::page_transfer_prepare::ShareMemberMetadata],
) -> Result<String, ApiError> {
    let bytes = if shares.is_empty() {
        serde_json::to_vec(blocks)
    } else {
        serde_json::to_vec(&(blocks, shares))
    }
    .map_err(|_| ApiError::internal())?;
    Ok(hex::encode(Sha256::digest(bytes)))
}

async fn lock_affected_shares(tx: &DatabaseTransaction, ids: Vec<Uuid>) -> Result<(), ApiError> {
    tx.query_all(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "SELECT s.id FROM flow_block_shares s WHERE EXISTS(SELECT 1 FROM flow_block_share_members m WHERE m.share_id=s.id AND m.logical_id=ANY($1::uuid[])) ORDER BY s.id FOR UPDATE",
        vec![ids.into()],
    ))
    .await?;
    Ok(())
}

async fn expire<C: ConnectionTrait>(db: &C, id: Uuid, actor: Uuid) -> Result<(), ApiError> {
    db.execute(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "UPDATE flow_page_block_transfers SET state='expired', failure_reason='transfer_expired', updated_at=now()
         WHERE id=$1 AND actor_id=$2 AND state='prepared' AND expires_at<=now()",
        vec![id.into(), actor.into()],
    ))
    .await?;
    Ok(())
}

pub async fn get_status(db: &DatabaseConnection, actor: Uuid, id: Uuid) -> Result<Value, ApiError> {
    expire(db, id, actor).await?;
    let row = load(db, id, actor).await?;
    let details = proofs(db, actor, row.source_object_id, row.target_object_id, kind(&row.kind))
        .await
        .is_ok();
    status(&row, details)
}

async fn ready<C: ConnectionTrait>(db: &C) -> Result<(), ApiError> {
    if scope::capabilities(db).await?.minimum_reader_version < 5 {
        return Err(error("transfer_readers_not_ready"));
    }
    scope::require_reader_ready(db).await
}
/// Transfers mutate already admitted editor documents under their ordinary write policy.
pub(super) async fn require_transfer_write<C: ConnectionTrait>(
    db: &C,
    identity: &scope::ObjectIdentity,
) -> Result<(), ApiError> {
    if let Some(workspace) = identity.workspace_id {
        super::collab::runtime::runtime().ensure_workspace_accepting(workspace)?;
        super::policy::require_flow_enabled_on(db, workspace).await?;
        scope::require_reader_ready(db).await
    } else {
        scope::require_personal_write(db).await
    }
}

fn check_key(key: &str) -> Result<(), ApiError> {
    if key.trim().is_empty() || key.len() > 128 {
        return Err(ApiError::invalid_update("Idempotency-Key must contain 1..128 bytes"));
    }
    Ok(())
}

async fn review_assets(
    tx: &sea_orm::DatabaseTransaction,
    source: &scope::ObjectIdentity,
    selected: &[ir::Block],
    head_seq: i64,
) -> Result<Vec<AssetMap>, ApiError> {
    let mut maps = BTreeMap::new();
    let mut total = 0_u64;
    for block in selected {
        let Some(asset) = block.asset() else { continue };
        let row = tx
            .query_one(Statement::from_sql_and_values(
                DbBackend::Postgres,
                "SELECT a.byte_size,a.content_sha256 FROM flow_page_assets a
             JOIN flow_page_asset_refs r ON (r.scope_id,r.object_id,r.asset_id)=(a.scope_id,a.object_id,a.id)
             JOIN collab_documents d ON d.object_id=a.object_id
             WHERE a.scope_id=$1 AND a.object_id=$2 AND a.id=$3 AND a.kind=$4 AND a.state='active'
               AND r.block_cid=$5 AND r.active AND r.last_seen_seq=$6 AND d.head_seq=$6",
                vec![
                    source.scope_id.into(),
                    source.id.into(),
                    asset.into(),
                    media_kind(block)?.into(),
                    block.source_cid.clone().into(),
                    head_seq.into(),
                ],
            ))
            .await?
            .ok_or_else(|| error("transfer_asset_unavailable"))?;
        if let std::collections::btree_map::Entry::Vacant(entry) = maps.entry(asset) {
            let bytes = u64::try_from(row.try_get::<i32>("", "byte_size")?).map_err(|_| ApiError::internal())?;
            total = total.checked_add(bytes).ok_or_else(|| error("transfer_asset_limit"))?;
            if total > 128 * 1024 * 1024 {
                return Err(error("transfer_asset_limit"));
            }
            entry.insert(AssetMap {
                source: asset,
                target: Uuid::new_v4(),
                bytes,
                sha256: row.try_get("", "content_sha256")?,
            });
        }
    }
    Ok(maps.into_values().collect())
}

fn media_kind(block: &ir::Block) -> Result<&'static str, ApiError> {
    match block.content {
        ir::Content::Image { .. } => Ok("image"),
        ir::Content::Attachment { .. } => Ok("attachment"),
        _ => Err(ApiError::internal()),
    }
}

/// The caller holds both accepted document locks and has fenced Page authority.
/// A failed commit cannot leave a staged destination blob.
async fn materialize_assets(
    tx: &sea_orm::DatabaseTransaction,
    source: &scope::ObjectIdentity,
    target: &scope::ObjectIdentity,
    actor: Uuid,
    selected: &[ir::Block],
    assets: &[AssetMap],
) -> Result<(), ApiError> {
    let mut blocks = BTreeMap::new();
    for block in selected {
        if let Some(asset) = block.asset() {
            blocks.entry(asset).or_insert(block);
        }
    }
    for asset in assets {
        let block = blocks.get(&asset.source).ok_or_else(ApiError::internal)?;
        let name = match &block.content {
            ir::Content::Attachment { name, .. } => {
                super::body_assets::validate_filename(name).unwrap_or_else(|_| "attachment".into())
            }
            _ => "image".into(),
        };
        let inserted = tx.execute(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "INSERT INTO flow_page_assets(id,scope_id,object_id,kind,filename,content_type,byte_size,content_sha256,bytes,created_by)
             SELECT $1,$2,$3,a.kind,$4,a.content_type,a.byte_size,a.content_sha256,a.bytes,$5
             FROM flow_page_assets a
             JOIN flow_page_asset_refs r ON (r.scope_id,r.object_id,r.asset_id)=(a.scope_id,a.object_id,a.id)
             JOIN collab_documents d ON d.object_id=a.object_id
             WHERE a.id=$6 AND a.object_id=$7 AND a.scope_id=$2 AND a.kind=$8 AND a.state='active'
               AND a.byte_size=$9 AND a.content_sha256=$10
               AND r.block_cid=$11 AND r.active AND r.last_seen_seq=d.head_seq
             FOR SHARE OF a",
            vec![asset.target.into(), target.scope_id.into(), target.id.into(), name.into(), actor.into(),
                 asset.source.into(), source.id.into(), media_kind(block)?.into(),
                 i32::try_from(asset.bytes).map_err(|_| ApiError::internal())?.into(),
                 asset.sha256.clone().into(), block.source_cid.clone().into()],
        )).await?;
        if inserted.rows_affected() != 1 {
            return Err(error("transfer_asset_unavailable"));
        }
    }
    Ok(())
}

/// Prepare reviews selected media metadata without creating target blobs or changing either Page.
pub async fn prepare(
    db: &DatabaseConnection,
    actor: Uuid,
    source: Uuid,
    key: &str,
    input: wire::PrepareInput,
) -> Result<Value, ApiError> {
    check_key(key)?;
    ready(db).await?;
    let body = serde_json::to_value(&input).map_err(|_| ApiError::internal())?;
    if let Some(prior)=db.query_one(Statement::from_sql_and_values(DbBackend::Postgres,"SELECT id,prepare_body FROM flow_page_block_transfers WHERE actor_id=$1 AND source_object_id=$2 AND prepare_key=$3",vec![actor.into(),source.into(),key.to_string().into()])).await? {
  if prior.try_get::<Value>("","prepare_body")?!=body{return Err(ApiError::idempotency_key_body_drift("transfer prepare identity changed"));}
  return get(db,actor,prior.try_get("","id")?).await;
 }
    let (source_proof, target_proof) = proofs(db, actor, source, input.target_object_id, input.kind).await?;
    let source_identity = scope::identity(db, source).await?;
    let target_identity = scope::identity(db, input.target_object_id).await?;
    if source_identity.object_type != "page" || target_identity.object_type != "page" {
        return Err(error("transfer_requires_pages"));
    }
    super::block_anchors::baseline_for_read(db, source).await?;
    super::block_anchors::baseline_for_read(db, input.target_object_id).await?;
    let pages = super::page_transfer_prepare::pages(db, source, input.target_object_id)
        .await
        .map_err(|_| error("transfer_registry_not_current"))?;
    let a = pages
        .iter()
        .find(|page| page.object_id == source)
        .ok_or_else(ApiError::internal)?;
    let b = pages
        .iter()
        .find(|page| page.object_id == input.target_object_id)
        .ok_or_else(ApiError::internal)?;
    let source_token = B64
        .decode(&input.expected_source_frontier)
        .map_err(|_| error("transfer_invalid_frontier"))?;
    let target_token = B64
        .decode(&input.expected_target_frontier)
        .map_err(|_| error("transfer_invalid_frontier"))?;
    if a.head_frontier != source_token {
        return Err(error("transfer_source_changed"));
    }
    if b.head_frontier != target_token {
        return Err(error("transfer_target_changed"));
    }
    let mut source_engine = super::document::load_head_engine(db, a.document_id).await?;
    let mut target_engine = super::document::load_head_engine(db, b.document_id).await?;
    if !source_engine.has_editor_doc() || !target_engine.has_editor_doc() {
        return Err(error("explicit_editor_initialization_required"));
    }
    source_engine.initialize_page_structure_model(5).map_err(map)?;
    target_engine.initialize_page_structure_model(5).map_err(map)?;
    let all = ir::read(&source_engine.semantic_snapshot().map_err(map)?).map_err(parse_error)?;
    let target_blocks = ir::read(&target_engine.semantic_snapshot().map_err(map)?).map_err(parse_error)?;
    let subtree = ir::select(&all, &input.root_block_cid, true).map_err(parse_error)?;
    if input.kind == wire::Kind::Move && matches!(input.selection, wire::Selection::Block) && subtree.len() > 1 {
        return Err(error("transfer_subtree_review_required"));
    }
    let selected = ir::select(
        &all,
        &input.root_block_cid,
        matches!(input.selection, wire::Selection::CurrentSubtree),
    )
    .map_err(parse_error)?;
    if selected.is_empty() || selected.len() > 256 {
        return Err(error("transfer_selection_limit"));
    }
    if target_blocks.is_empty() {
        return Err(error("transfer_target_needs_initial_block"));
    }
    if input
        .target_parent_block_cid
        .as_ref()
        .is_some_and(|parent| !target_blocks.iter().any(|block| &block.source_cid == parent))
    {
        return Err(error("transfer_target_position_changed"));
    }
    let target_siblings = target_blocks
        .iter()
        .filter(|block| block.parent_cid == input.target_parent_block_cid)
        .count();
    if input.target_index as usize > target_siblings {
        return Err(error("transfer_target_position_changed"));
    }
    let cids = selected
        .iter()
        .map(|block| block.source_cid.clone())
        .collect::<Vec<_>>();
    let placements = super::page_transfer_prepare::active_placements(db, source, &cids)
        .await
        .map_err(|_| error("transfer_identity_changed"))?;
    let selected_meta = selected
        .iter()
        .map(|block| {
            let p = placements
                .iter()
                .find(|p| p.physical_cid == block.source_cid)
                .ok_or_else(ApiError::internal)?;
            Ok(Selected {
                cid: block.source_cid.clone(),
                logical_id: p.logical_id,
                generation: p.generation,
                parent_cid: block.parent_cid.clone(),
                preorder: block.preorder,
            })
        })
        .collect::<Result<Vec<_>, ApiError>>()?;
    let root = all
        .iter()
        .find(|block| block.source_cid == input.root_block_cid)
        .ok_or_else(ApiError::internal)?;
    let source_index = all
        .iter()
        .filter(|block| block.parent_cid == root.parent_cid)
        .position(|block| block.source_cid == root.source_cid)
        .ok_or_else(ApiError::internal)?;
    let mut metadata = SelectionMetadata {
        blocks: selected_meta,
        source_parent: root.parent_cid.clone(),
        source_index: u32::try_from(source_index).map_err(|_| ApiError::internal())?,
        target_parent: input.target_parent_block_cid.clone(),
        target_index: input.target_index,
        reviewed_shares: Vec::new(),
    };
    let id = Uuid::new_v4();
    let tx = db.begin().await?;
    let reverse_destination = (input.kind == wire::Kind::Move).then_some(source);
    let mut proofs = child_link_proofs(&tx, actor, target_identity.id, reverse_destination, &selected).await?;
    proofs.push(source_proof);
    proofs.push(target_proof);
    scope::fence_many(&tx, &proofs).await?;
    require_transfer_write(&tx, &target_identity).await?;
    if input.kind == wire::Kind::Move {
        metadata.reviewed_shares = affected_shares(&tx, &metadata.blocks).await?;
    }
    let digest = selection_digest(&selected, &metadata.reviewed_shares)?;
    let assets = review_assets(&tx, &source_identity, &selected, a.head_seq).await?;
    let expires = platform::timestamp::Timestamp::now() + time::Duration::minutes(15);
    let inserted = tx.execute(Statement::from_sql_and_values(DbBackend::Postgres,"INSERT INTO flow_page_block_transfers(id,actor_id,source_object_id,target_object_id,kind,state,prepare_key,prepare_body,source_frontier,target_frontier,selection_digest,selection_metadata,asset_mapping,expires_at) VALUES($1,$2,$3,$4,$5,'prepared',$6,$7,$8,$9,$10,$11,$12,$13) ON CONFLICT (actor_id, source_object_id, prepare_key) DO NOTHING",vec![id.into(),actor.into(),source.into(),input.target_object_id.into(),if input.kind==wire::Kind::Move{"move"}else{"copy"}.into(),key.to_string().into(),body.into(),source_token.into(),target_token.into(),digest.into(),serde_json::to_value(metadata).map_err(|_|ApiError::internal())?.into(),serde_json::to_value(assets).map_err(|_|ApiError::internal())?.into(),expires.into()])).await?;
    if inserted.rows_affected() == 0 {
        tx.rollback().await?;
        let prior = db.query_one(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT id,prepare_body FROM flow_page_block_transfers WHERE actor_id=$1 AND source_object_id=$2 AND prepare_key=$3",
            vec![actor.into(), source.into(), key.to_string().into()],
        )).await?.ok_or_else(ApiError::internal)?;
        if prior.try_get::<Value>("", "prepare_body")?
            != serde_json::to_value(input).map_err(|_| ApiError::internal())?
        {
            return Err(ApiError::idempotency_key_body_drift(
                "transfer prepare identity changed",
            ));
        }
        return get(db, actor, prior.try_get("", "id")?).await;
    }
    tx.commit().await?;
    get(db, actor, id).await
}
pub async fn get(db: &DatabaseConnection, actor: Uuid, id: Uuid) -> Result<Value, ApiError> {
    expire(db, id, actor).await?;
    let row = load(db, id, actor).await?;
    if row.actor_id != actor {
        return Err(error("transfer_unavailable"));
    }
    let details = proofs(db, actor, row.source_object_id, row.target_object_id, kind(&row.kind))
        .await
        .is_ok();
    if row.state != "prepared" || !details {
        return status(&row, details);
    }
    let metadata: SelectionMetadata =
        serde_json::from_value(row.selection_metadata.clone()).map_err(|_| ApiError::internal())?;
    let assets: Vec<AssetMap> = serde_json::from_value(row.asset_mapping.clone()).map_err(|_| ApiError::internal())?;
    let count = metadata
        .reviewed_shares
        .iter()
        .map(|share| share.share_id)
        .collect::<std::collections::BTreeSet<_>>()
        .len();
    serde_json::to_value(wire::Prepared {
        transfer_id: id,
        kind: kind(&row.kind),
        state: wire::State::Prepared,
        expires_at: row.expires_at.to_rfc3339(),
        review: wire::Review {
            source_object_id: row.source_object_id,
            target_object_id: row.target_object_id,
            source_frontier: B64.encode(row.source_frontier),
            target_frontier: B64.encode(row.target_frontier),
            selection_digest: row.selection_digest,
            block_count: u32::try_from(metadata.blocks.len()).map_err(|_| ApiError::internal())?,
            asset_count: u32::try_from(assets.len()).map_err(|_| ApiError::internal())?,
            total_asset_bytes: assets.iter().map(|asset| asset.bytes).sum(),
            affected_share_count: u32::try_from(count).map_err(|_| ApiError::internal())?,
            native_undo_barrier_pages: if row.kind == "move" {
                vec![row.source_object_id, row.target_object_id]
            } else {
                vec![row.target_object_id]
            },
        },
        assets: wire::Assets {
            state: wire::AssetState::Ready,
            copied_count: u32::try_from(assets.len()).map_err(|_| ApiError::internal())?,
            total_bytes: assets.iter().map(|asset| asset.bytes).sum(),
        },
    })
    .map_err(|_| ApiError::internal())
}

const fn native_name(content: &ir::Content) -> &'static str {
    match content {
        ir::Content::Paragraph => "paragraph",
        ir::Content::Heading { .. } => "heading",
        ir::Content::Bullet => "bulletItem",
        ir::Content::Ordered { .. } => "orderedItem",
        ir::Content::Todo { .. } => "todoItem",
        ir::Content::Quote => "blockquote",
        ir::Content::Toggle { .. } => "toggle",
        ir::Content::Code => "codeBlock",
        ir::Content::Divider => "divider",
        ir::Content::Image { .. } => "image",
        ir::Content::Attachment { .. } => "attachment",
        ir::Content::ChildPage { .. } => "childPage",
    }
}
pub(super) fn insert_selected(
    engine: &mut collab_core::LoroCollabEngine,
    selected: &[ir::Block],
    parent: Option<&str>,
    index: u32,
    operation: Uuid,
) -> Result<Vec<(String, String)>, ApiError> {
    use collab_core::{NodeKind, Operation};
    let mut mapping = BTreeMap::<String, String>::new();
    let mut children = BTreeMap::<String, u32>::new();
    let mut output = Vec::new();
    let mut root_index = index;
    for (offset, block) in selected.iter().enumerate() {
        let alias = format!("transfer:{operation}:{offset}");
        let parent_cid = if let Some(parent) = &block.parent_cid {
            Some(
                mapping
                    .get(parent)
                    .ok_or_else(|| error("transfer_invalid_hierarchy"))?
                    .as_str(),
            )
        } else {
            parent
        };
        let at = {
            let next = block
                .parent_cid
                .as_ref()
                .map_or(&mut root_index, |parent| children.entry(parent.clone()).or_default());
            let at = *next;
            *next += 1;
            at
        };
        let cid = match &block.content {
            ir::Content::Image { asset_id, alt, caption } => engine
                .insert_page_atom(
                    &alias,
                    parent_cid,
                    at as usize,
                    "image",
                    &json!({"asset_id":asset_id,"alt":alt,"caption":caption,"indent":0}),
                )
                .map_err(map)?,
            ir::Content::Attachment {
                asset_id,
                name,
                mime,
                caption,
            } => engine
                .insert_page_atom(
                    &alias,
                    parent_cid,
                    at as usize,
                    "attachment",
                    &json!({"asset_id":asset_id,"name":name,"mime":mime,"caption":caption,"indent":0}),
                )
                .map_err(map)?,
            ir::Content::ChildPage { child_page_id } => engine
                .insert_page_atom(
                    &alias,
                    parent_cid,
                    at as usize,
                    "childPage",
                    &json!({"child_page_id":child_page_id,"indent":0}),
                )
                .map_err(map)?,
            _ => {
                engine
                    .apply_content_operation(&Operation::CreateNode {
                        id: alias.as_str().into(),
                        parent: parent_cid.map(Into::into),
                        index: at,
                        kind: NodeKind::Block,
                    })
                    .map_err(map)?;
                let cid = engine.resolve_content_node_id(&alias.as_str().into()).to_string();
                engine
                    .apply_content_operation(&Operation::SetProperty {
                        id: cid.as_str().into(),
                        key: "block_type".into(),
                        value: native_name(&block.content).into(),
                    })
                    .map_err(map)?;
                for (key, value) in match block.content {
                    ir::Content::Heading { level } => vec![("level", level.to_string())],
                    ir::Content::Ordered { order } => vec![("order", order.to_string())],
                    ir::Content::Todo { checked } => vec![("checked", checked.to_string())],
                    ir::Content::Toggle { collapsed } => vec![("collapsed", collapsed.to_string())],
                    _ => vec![],
                } {
                    engine
                        .apply_content_operation(&Operation::SetProperty {
                            id: cid.as_str().into(),
                            key: key.into(),
                            value,
                        })
                        .map_err(map)?;
                }
                if !block.text.is_empty() {
                    engine
                        .apply_content_operation(&Operation::InsertText {
                            id: cid.as_str().into(),
                            index: 0,
                            text: block.text.clone(),
                        })
                        .map_err(map)?;
                }
                if !block.spans.is_empty() {
                    engine
                        .apply_page_marks(
                            &cid,
                            &block
                                .spans
                                .iter()
                                .map(|span| collab_core::page_inline::MarkEdit {
                                    start: span.start,
                                    end: span.end,
                                    mark: span.mark,
                                    clear: false,
                                    href: span.href.clone(),
                                })
                                .collect::<Vec<_>>(),
                        )
                        .map_err(map)?;
                }
                cid
            }
        };
        mapping.insert(block.source_cid.clone(), cid.clone());
        output.push((block.source_cid.clone(), cid));
    }
    Ok(output)
}
async fn stage_document(
    tx: &sea_orm::DatabaseTransaction,
    identity: &scope::ObjectIdentity,
    actor: Uuid,
    document: Uuid,
    update: Uuid,
    bytes: &[u8],
    prepared: &super::collab::write::Prepared,
    operation: Uuid,
) -> Result<wire::AcceptedHead, ApiError> {
    let seq = prepared
        .observed
        .head_seq
        .checked_add(1)
        .ok_or_else(ApiError::internal)?;
    let key = format!("transfer:{operation}:{document}");
    let event =
        super::scope_event::insert_content_accepted(tx, identity, actor, document, seq, &key, "block_transfer").await?;
    tx.execute(Statement::from_sql_and_values(DbBackend::Postgres,"INSERT INTO collab_updates(document_id,seq,update_id,content_hash,idempotency_key,before_frontier,after_frontier,bytes,actor_id,origin_surface,origin_client_id,projection_seq,event_id) VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9,'web','block-transfer',$2,$10)",vec![document.into(),seq.into(),update.into(),prepared.content_hash_hex.clone().into(),key.into(),prepared.observed.head_frontier.clone().into(),prepared.after_frontier.clone().into(),bytes.to_vec().into(),actor.into(),event.into()])).await?;
    tx.execute(Statement::from_sql_and_values(DbBackend::Postgres,"UPDATE collab_documents SET head_seq=$2,head_frontier=$3,byte_count=byte_count+$4,update_count=update_count+1,updated_at=now() WHERE id=$1",vec![document.into(),seq.into(),prepared.after_frontier.clone().into(),i64::try_from(bytes.len()).map_err(|_|ApiError::internal())?.into()])).await?;
    tx.execute(Statement::from_sql_and_values(DbBackend::Postgres,"UPDATE flow_object_projections SET document_seq=$2,document_frontier=$3,title=$4,state=$5,plain_text=$6,updated_at=now() WHERE object_id=$1",vec![identity.id.into(),seq.into(),prepared.after_frontier.clone().into(),prepared.title.clone().into(),prepared.state_json.clone().into(),prepared.plain_text.clone().into()])).await?;
    super::body_assets::reconcile_accepted(tx, identity.id, seq, &prepared.asset_references).await?;
    super::child_pages::reconcile_accepted(tx, identity.id, Some(actor), seq, &prepared.child_references).await?;
    super::block_registry::reconcile_accepted(tx, identity.id, seq, &prepared.logical_blocks).await?;
    tx.execute(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "UPDATE flow_objects SET updated_at=now(),updated_by=$2 WHERE id=$1",
        vec![identity.id.into(), actor.into()],
    ))
    .await?;
    Ok(wire::AcceptedHead {
        object_id: identity.id,
        accepted_seq: seq,
        frontier: B64.encode(&prepared.after_frontier),
    })
}

/// Capture or validate the registry identities while holding the destination Page and
/// document locks. The order is stable even if another operation rearranged the Page.
async fn copied_placements(
    tx: &sea_orm::DatabaseTransaction,
    target: &scope::ObjectIdentity,
    mappings: &[wire::Mapping],
) -> Result<Vec<CopiedPlacement>, ApiError> {
    let cids = mappings
        .iter()
        .map(|mapping| &mapping.target_block_cid)
        .collect::<Vec<_>>();
    let rows = CopiedPlacement::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "SELECT physical_cid,logical_id,generation FROM flow_block_placements
         WHERE scope_id=$1 AND page_id=$2 AND state='active'
           AND physical_cid IN (SELECT jsonb_array_elements_text($3))
         ORDER BY logical_id FOR UPDATE",
        vec![target.scope_id.into(), target.id.into(), json!(cids).into()],
    ))
    .all(tx)
    .await?;
    if rows.len() != mappings.len() {
        return Err(error("transfer_identity_changed"));
    }
    Ok(rows)
}

/// Reconcile a lost or racing reply through the same actor-bound mutation identity.
pub async fn commit(
    db: &DatabaseConnection,
    actor: Uuid,
    id: Uuid,
    key: &str,
    input: wire::CommitInput,
) -> Result<Value, ApiError> {
    let failed = match commit_once(db, actor, id, key, &input).await {
        Ok(result) => return Ok(result),
        Err(failed) => failed,
    };
    let body = serde_json::to_value(input).map_err(|_| ApiError::internal())?;
    if let ApiError::Typed {
        details: Some(details), ..
    } = &failed
        && let Some(
            reason @ ("transfer_source_changed"
            | "transfer_target_changed"
            | "transfer_review_stale"
            | "transfer_asset_unavailable"),
        ) = details["reason"].as_str()
    {
        db.execute(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "UPDATE flow_page_block_transfers
             SET state='not_applied', commit_key=$3, commit_body=$4, failure_reason=$5, updated_at=now()
             WHERE id=$1 AND actor_id=$2 AND state='prepared'",
            vec![
                id.into(),
                actor.into(),
                key.to_owned().into(),
                body.clone().into(),
                reason.to_owned().into(),
            ],
        ))
        .await?;
    }
    if let Ok(row) = load(db, id, actor).await
        && matches!(row.state.as_str(), "accepted" | "undone" | "not_applied")
        && row.commit_key.as_deref() == Some(key)
        && row.commit_body.as_ref() == Some(&body)
    {
        return get_status(db, actor, id).await;
    }
    Err(failed)
}

/// The same operation row, both Page heads and their identity/ref updates commit atomically.
async fn commit_once(
    db: &DatabaseConnection,
    actor: Uuid,
    id: Uuid,
    key: &str,
    input: &wire::CommitInput,
) -> Result<Value, ApiError> {
    use super::collab::{
        runtime,
        write::{self, HydrateOutcome},
    };
    check_key(key)?;
    ready(db).await?;
    let row = load(db, id, actor).await?;
    let body = serde_json::to_value(input).map_err(|_| ApiError::internal())?;
    if matches!(row.state.as_str(), "expired" | "cancelled") {
        return get_status(db, actor, id).await;
    }
    if row.state != "prepared" {
        if row.commit_key.as_deref() != Some(key) || row.commit_body.as_ref() != Some(&body) {
            return Err(ApiError::idempotency_key_body_drift(
                "transfer commit confirmation changed",
            ));
        }
        return get(db, actor, id).await;
    }
    if row.expires_at <= platform::timestamp::Timestamp::now() {
        return get_status(db, actor, id).await;
    }
    if row.selection_digest != input.selection_digest
        || row.source_frontier
            != B64
                .decode(&input.expected_source_frontier)
                .map_err(|_| error("transfer_invalid_frontier"))?
        || row.target_frontier
            != B64
                .decode(&input.expected_target_frontier)
                .map_err(|_| error("transfer_invalid_frontier"))?
    {
        return Err(error("transfer_review_mismatch"));
    }
    let op_kind = kind(&row.kind);
    let (source_proof, target_proof) = proofs(db, actor, row.source_object_id, row.target_object_id, op_kind).await?;
    let source = scope::identity(db, row.source_object_id).await?;
    let target = scope::identity(db, row.target_object_id).await?;
    let pages = super::page_transfer_prepare::pages(db, source.id, target.id)
        .await
        .map_err(|_| error("transfer_registry_not_current"))?;
    let source_page = pages
        .iter()
        .find(|page| page.object_id == source.id)
        .ok_or_else(ApiError::internal)?;
    let target_page = pages
        .iter()
        .find(|page| page.object_id == target.id)
        .ok_or_else(ApiError::internal)?;
    let mut doc_ids = vec![source_page.document_id, target_page.document_id];
    doc_ids.sort();
    let mut guards = Vec::new();
    for document in &doc_ids {
        guards.push(
            runtime::runtime()
                .coordinator
                .acquire(*document)
                .await
                .map_err(|_| error("transfer_busy"))?,
        );
    }
    let mut source_engine = super::document::load_head_engine(db, source_page.document_id).await?;
    let mut target_engine = super::document::load_head_engine(db, target_page.document_id).await?;
    let source_base = source_engine.frontier();
    let target_base = target_engine.frontier();
    source_engine.initialize_page_structure_model(5).map_err(map)?;
    target_engine.initialize_page_structure_model(5).map_err(map)?;
    let metadata: SelectionMetadata =
        serde_json::from_value(row.selection_metadata.clone()).map_err(|_| ApiError::internal())?;
    let prepare_input: wire::PrepareInput =
        serde_json::from_value(row.prepare_body.clone()).map_err(|_| ApiError::internal())?;
    let all = ir::read(&source_engine.semantic_snapshot().map_err(map)?).map_err(parse_error)?;
    let selected = ir::select(
        &all,
        &prepare_input.root_block_cid,
        matches!(prepare_input.selection, wire::Selection::CurrentSubtree),
    )
    .map_err(parse_error)?;
    let digest = selection_digest(&selected, &metadata.reviewed_shares)?;
    if digest != row.selection_digest {
        return Err(error("transfer_source_changed"));
    }
    let assets: Vec<AssetMap> = serde_json::from_value(row.asset_mapping.clone()).map_err(|_| ApiError::internal())?;
    let asset_mapping = assets
        .iter()
        .map(|asset| (asset.source, asset.target))
        .collect::<BTreeMap<_, _>>();
    let mut copied = selected.clone();
    for block in &mut copied {
        block.remap_asset(&asset_mapping).map_err(parse_error)?;
    }
    let mappings = insert_selected(
        &mut target_engine,
        &copied,
        metadata.target_parent.as_deref(),
        metadata.target_index,
        id,
    )?;
    let mut placeholder = None;
    if op_kind == wire::Kind::Move {
        if all.len() == selected.len() {
            let alias = format!("transfer-placeholder:{id}");
            source_engine
                .apply_content_operation(&collab_core::Operation::CreateNode {
                    id: alias.as_str().into(),
                    parent: None,
                    index: 0,
                    kind: collab_core::NodeKind::Block,
                })
                .map_err(map)?;
            placeholder = Some(
                source_engine
                    .resolve_content_node_id(&alias.as_str().into())
                    .to_string(),
            );
        }
        source_engine
            .apply_content_operation(&collab_core::Operation::DeleteNode {
                id: prepare_input.root_block_cid.as_str().into(),
            })
            .map_err(map)?;
    }
    let target_bytes = target_engine.export_from(&target_base).map_err(map)?;
    let target_update = write::replay_stable_update_id(target_page.document_id, &format!("transfer:{id}:target"));
    let target_prepared = match write::hydrate_and_apply(
        db,
        &runtime::runtime().cache,
        target_page.document_id,
        target_update,
        &target_bytes,
        Some(&row.target_frontier),
    )
    .await?
    {
        HydrateOutcome::Prepared(prepared) => prepared,
        HydrateOutcome::Rejected(_) => return Err(error("transfer_target_changed")),
    };
    let source_change = if op_kind == wire::Kind::Move {
        let bytes = source_engine.export_from(&source_base).map_err(map)?;
        let update = write::replay_stable_update_id(source_page.document_id, &format!("transfer:{id}:source"));
        let prepared = match write::hydrate_and_apply(
            db,
            &runtime::runtime().cache,
            source_page.document_id,
            update,
            &bytes,
            Some(&row.source_frontier),
        )
        .await?
        {
            HydrateOutcome::Prepared(prepared) => prepared,
            HydrateOutcome::Rejected(_) => return Err(error("transfer_source_changed")),
        };
        Some((bytes, update, prepared))
    } else {
        None
    };
    let tx = db.begin().await?;
    tx.execute(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "SELECT id FROM flow_page_block_transfers WHERE id=$1 FOR UPDATE",
        vec![id.into()],
    ))
    .await?;
    expire(&tx, id, actor).await?;
    let locked = load(&tx, id, actor).await?;
    if locked.state != "prepared" {
        tx.commit().await?;
        if matches!(locked.state.as_str(), "accepted" | "undone" | "not_applied")
            && (locked.commit_key.as_deref() != Some(key) || locked.commit_body.as_ref() != Some(&body))
        {
            return Err(ApiError::idempotency_key_body_drift(
                "transfer commit confirmation changed",
            ));
        }
        return get_status(db, actor, id).await;
    }
    let reverse_destination = (op_kind == wire::Kind::Move).then_some(source.id);
    let mut proofs = child_link_proofs(&tx, actor, target.id, reverse_destination, &selected).await?;
    proofs.push(source_proof);
    proofs.push(target_proof);
    scope::fence_many(&tx, &proofs).await?;
    require_transfer_write(&tx, &target).await?;
    if op_kind == wire::Kind::Move {
        require_transfer_write(&tx, &source).await?;
    }
    lock_transfer_objects(&tx, source.id, target.id, &proofs).await?;
    for document in &doc_ids {
        let head = tx
            .query_one(Statement::from_sql_and_values(
                DbBackend::Postgres,
                "SELECT head_frontier FROM collab_documents WHERE id=$1 FOR UPDATE",
                vec![(*document).into()],
            ))
            .await?
            .ok_or_else(ApiError::internal)?;
        let expected = if *document == source_page.document_id {
            &row.source_frontier
        } else {
            &row.target_frontier
        };
        if head.try_get::<Vec<u8>>("", "head_frontier")? != *expected {
            return Err(error("transfer_review_stale"));
        }
    }
    if op_kind == wire::Kind::Move {
        lock_affected_shares(&tx, metadata.blocks.iter().map(|block| block.logical_id).collect()).await?;
        if affected_shares(&tx, &metadata.blocks).await? != metadata.reviewed_shares {
            return Err(error("transfer_review_stale"));
        }
    }
    materialize_assets(&tx, &source, &target, actor, &selected, &assets).await?;
    let share_count = if op_kind == wire::Kind::Move {
        move_registry(&tx, &source, &target, &metadata, &mappings, &target_prepared).await?
    } else {
        0
    };
    let source_head = if let Some((bytes, update, prepared)) = &source_change {
        stage_document(
            &tx,
            &source,
            actor,
            source_page.document_id,
            *update,
            bytes,
            prepared,
            id,
        )
        .await?
    } else {
        wire::AcceptedHead {
            object_id: source.id,
            accepted_seq: source_page.head_seq,
            frontier: B64.encode(&row.source_frontier),
        }
    };
    let target_head = stage_document(
        &tx,
        &target,
        actor,
        target_page.document_id,
        target_update,
        &target_bytes,
        &target_prepared,
        id,
    )
    .await?;
    let receipt = wire::Receipt {
        changed_page_ids: if op_kind == wire::Kind::Move {
            vec![source.id, target.id]
        } else {
            vec![target.id]
        },
        source: source_head.clone(),
        target: target_head.clone(),
        mappings: mappings
            .into_iter()
            .map(|(source_block_cid, target_block_cid)| wire::Mapping {
                source_block_cid,
                target_block_cid,
                identity: if op_kind == wire::Kind::Move {
                    wire::Identity::Preserved
                } else {
                    wire::Identity::New
                },
            })
            .collect(),
        source_placeholder_cid: placeholder,
        affected_share_count: share_count,
        undo: wire::Undo::Available {
            expected_source_frontier: source_head.frontier,
            expected_target_frontier: target_head.frontier,
        },
    };
    let copy_receipt = if op_kind == wire::Kind::Copy {
        let placements = copied_placements(&tx, &target, &receipt.mappings).await?;
        Some(serde_json::to_value(placements).map_err(|_| ApiError::internal())?)
    } else {
        None
    };
    tx.execute(Statement::from_sql_and_values(DbBackend::Postgres,"UPDATE flow_page_block_transfers SET state='accepted',receipt=$2,commit_key=$3,commit_body=$4,copied_placements=$5,updated_at=now() WHERE id=$1",vec![id.into(),serde_json::to_value(&receipt).map_err(|_|ApiError::internal())?.into(),key.to_string().into(),body.into(),copy_receipt.into()])).await?;
    tx.commit().await?;
    if let Some((bytes, update, prepared)) = &source_change
        && let Err(error) = publish_committed(
            db,
            &source,
            actor,
            source_page.document_id,
            *update,
            bytes,
            prepared,
            &receipt.source,
        )
        .await
    {
        tracing::warn!(%error,"committed transfer source delivery will backfill");
    }
    if let Err(error) = publish_committed(
        db,
        &target,
        actor,
        target_page.document_id,
        target_update,
        &target_bytes,
        &target_prepared,
        &receipt.target,
    )
    .await
    {
        tracing::warn!(%error,"committed transfer target delivery will backfill");
    }
    drop(guards);
    serde_json::to_value(wire::Accepted {
        transfer_id: id,
        kind: op_kind,
        state: wire::State::Accepted,
        receipt: Some(receipt),
    })
    .map_err(|_| ApiError::internal())
}

async fn move_registry(
    tx: &sea_orm::DatabaseTransaction,
    source: &scope::ObjectIdentity,
    target: &scope::ObjectIdentity,
    metadata: &SelectionMetadata,
    mappings: &[(String, String)],
    destination_prepared: &super::collab::write::Prepared,
) -> Result<u32, ApiError> {
    let logical_ids = metadata.blocks.iter().map(|block| block.logical_id).collect::<Vec<_>>();
    for selected in &metadata.blocks {
        let current=tx.query_one(Statement::from_sql_and_values(DbBackend::Postgres,"SELECT id,generation FROM flow_block_placements WHERE logical_id=$1 AND page_id=$2 AND physical_cid=$3 AND state='active' FOR UPDATE",vec![selected.logical_id.into(),source.id.into(),selected.cid.clone().into()])).await?.ok_or_else(||error("transfer_identity_changed"))?;
        if current.try_get::<i64>("", "generation")? != selected.generation {
            return Err(error("transfer_identity_changed"));
        }
        let target_cid = &mappings
            .iter()
            .find(|(cid, _)| cid == &selected.cid)
            .ok_or_else(ApiError::internal)?
            .1;
        let hash = destination_prepared
            .logical_blocks
            .iter()
            .find(|block| &block.physical_cid == target_cid)
            .ok_or_else(ApiError::internal)?
            .content_hash;
        tx.execute(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "UPDATE flow_block_placements SET state='moved',retired_at=now() WHERE id=$1",
            vec![current.try_get::<Uuid>("", "id")?.into()],
        ))
        .await?;
        tx.execute(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "INSERT INTO flow_block_alias_fences(scope_id,page_id,physical_cid,first_logical_id) VALUES($1,$2,$3,$4)",
            vec![
                target.scope_id.into(),
                target.id.into(),
                target_cid.clone().into(),
                selected.logical_id.into(),
            ],
        ))
        .await?;
        tx.execute(Statement::from_sql_and_values(DbBackend::Postgres,"INSERT INTO flow_block_placements(id,logical_id,scope_id,page_id,physical_cid,generation,state,content_hash) VALUES($1,$2,$3,$4,$5,$6,'active',$7)",vec![Uuid::new_v4().into(),selected.logical_id.into(),target.scope_id.into(),target.id.into(),target_cid.clone().into(),selected.generation.checked_add(1).ok_or_else(ApiError::internal)?.into(),hash.to_vec().into()])).await?;
    }
    let shares=tx.query_all(Statement::from_sql_and_values(DbBackend::Postgres,"SELECT s.id,s.root_logical_id,NOT EXISTS(SELECT 1 FROM flow_block_share_members outside WHERE outside.share_id=s.id AND NOT(outside.logical_id=ANY($1::uuid[]))) AS all_members FROM flow_block_shares s WHERE s.state<>'revoked' AND EXISTS(SELECT 1 FROM flow_block_share_members affected WHERE affected.share_id=s.id AND affected.logical_id=ANY($1::uuid[])) ORDER BY s.id FOR UPDATE",vec![logical_ids.clone().into()])).await?;
    let share_count = u32::try_from(shares.len()).map_err(|_| ApiError::internal())?;
    for share in shares {
        let share_id = share.try_get::<Uuid>("", "id")?;
        let root = share.try_get::<Uuid>("", "root_logical_id")?;
        if logical_ids.contains(&root) && share.try_get::<bool>("", "all_members")? {
            let source_cid = &metadata
                .blocks
                .iter()
                .find(|block| block.logical_id == root)
                .ok_or_else(ApiError::internal)?
                .cid;
            let new_root = &mappings
                .iter()
                .find(|(cid, _)| cid == source_cid)
                .ok_or_else(ApiError::internal)?
                .1;
            tx.execute(Statement::from_sql_and_values(
                DbBackend::Postgres,
                "DELETE FROM flow_block_share_asset_aliases WHERE share_id=$1",
                vec![share_id.into()],
            ))
            .await?;
            tx.execute(Statement::from_sql_and_values(DbBackend::Postgres,"UPDATE flow_block_shares SET source_scope_id=$2,source_page_id=$3,source_root_cid=$4,state=CASE WHEN state='target_removed' THEN state ELSE 'suspended' END,auth_revision=auth_revision+1 WHERE id=$1",vec![share_id.into(),target.scope_id.into(),target.id.into(),new_root.clone().into()])).await?;
        } else {
            tx.execute(Statement::from_sql_and_values(
                DbBackend::Postgres,
                "UPDATE flow_block_shares SET state=CASE WHEN state='target_removed' THEN state ELSE 'suspended' END,auth_revision=auth_revision+1 WHERE id=$1",
                vec![share_id.into()],
            ))
            .await?;
        }
    }

    Ok(share_count)
}

/// Explicit receipt-bound reversal. It never delegates to native single-Doc Undo history.
pub async fn undo(
    db: &DatabaseConnection,
    actor: Uuid,
    id: Uuid,
    key: &str,
    input: wire::UndoInput,
) -> Result<Value, ApiError> {
    let failed = match undo_once(db, actor, id, key, &input).await {
        Ok(result) => return Ok(result),
        Err(failed) => failed,
    };
    let body = serde_json::to_value(input).map_err(|_| ApiError::internal())?;
    if let Ok(row) = load(db, id, actor).await
        && row.state == "undone"
        && row.undo_key.as_deref() == Some(key)
        && row.undo_body.as_ref() == Some(&body)
    {
        return get_status(db, actor, id).await;
    }
    Err(failed)
}

async fn undo_once(
    db: &DatabaseConnection,
    actor: Uuid,
    id: Uuid,
    key: &str,
    input: &wire::UndoInput,
) -> Result<Value, ApiError> {
    use super::collab::{
        runtime,
        write::{self, HydrateOutcome},
    };
    check_key(key)?;
    ready(db).await?;
    let row = load(db, id, actor).await?;
    let body = serde_json::to_value(input).map_err(|_| ApiError::internal())?;
    if row.state == "undone" {
        if row.undo_key.as_deref() != Some(key) || row.undo_body.as_ref() != Some(&body) {
            return Err(ApiError::idempotency_key_body_drift(
                "transfer Undo confirmation changed",
            ));
        }
        return get(db, actor, id).await;
    }
    if row.state != "accepted" {
        return Err(error("transfer_not_accepted"));
    }
    let original: wire::Receipt = serde_json::from_value(row.receipt.clone().ok_or_else(ApiError::internal)?)
        .map_err(|_| ApiError::internal())?;
    if original.source.frontier != input.expected_source_frontier
        || original.target.frontier != input.expected_target_frontier
    {
        return Err(error("transfer_undo_review_mismatch"));
    }
    let expected_source = B64
        .decode(&input.expected_source_frontier)
        .map_err(|_| error("transfer_invalid_frontier"))?;
    let expected_target = B64
        .decode(&input.expected_target_frontier)
        .map_err(|_| error("transfer_invalid_frontier"))?;
    let op_kind = kind(&row.kind);
    let (source_proof, target_proof) = proofs(db, actor, row.source_object_id, row.target_object_id, op_kind).await?;
    let source = scope::identity(db, row.source_object_id).await?;
    let target = scope::identity(db, row.target_object_id).await?;
    let pages = super::page_transfer_prepare::pages(db, source.id, target.id)
        .await
        .map_err(|_| error("transfer_registry_not_current"))?;
    let a = pages
        .iter()
        .find(|page| page.object_id == source.id)
        .ok_or_else(ApiError::internal)?;
    let b = pages
        .iter()
        .find(|page| page.object_id == target.id)
        .ok_or_else(ApiError::internal)?;
    if a.head_frontier != expected_source || b.head_frontier != expected_target {
        return Err(error("transfer_undo_conflict"));
    }
    let mut doc_ids = vec![a.document_id, b.document_id];
    doc_ids.sort();
    let mut guards = Vec::new();
    for doc in &doc_ids {
        guards.push(
            runtime::runtime()
                .coordinator
                .acquire(*doc)
                .await
                .map_err(|_| error("transfer_busy"))?,
        );
    }
    let mut source_engine = super::document::load_head_engine(db, a.document_id).await?;
    let mut target_engine = super::document::load_head_engine(db, b.document_id).await?;
    let source_base = source_engine.frontier();
    let target_base = target_engine.frontier();
    let metadata: SelectionMetadata =
        serde_json::from_value(row.selection_metadata.clone()).map_err(|_| ApiError::internal())?;
    let root = original
        .mappings
        .first()
        .ok_or_else(ApiError::internal)?
        .target_block_cid
        .clone();
    let target_ir = ir::read(&target_engine.semantic_snapshot().map_err(map)?).map_err(parse_error)?;
    let mut selected = ir::select(&target_ir, &root, true).map_err(parse_error)?;
    let actual = selected
        .iter()
        .map(|block| block.source_cid.as_str())
        .collect::<std::collections::BTreeSet<_>>();
    let expected = original
        .mappings
        .iter()
        .map(|mapping| mapping.target_block_cid.as_str())
        .collect::<std::collections::BTreeSet<_>>();
    if actual != expected {
        return Err(error("transfer_undo_conflict"));
    }
    let operation = write::replay_stable_update_id(id, "explicit-transfer-undo");
    let mut reverse = Vec::new();
    if op_kind == wire::Kind::Move {
        source_engine.initialize_page_structure_model(5).map_err(map)?;
        let assets: Vec<AssetMap> =
            serde_json::from_value(row.asset_mapping.clone()).map_err(|_| ApiError::internal())?;
        let back = assets
            .iter()
            .map(|asset| (asset.target, asset.source))
            .collect::<BTreeMap<_, _>>();
        for block in &mut selected {
            block.remap_asset(&back).map_err(parse_error)?;
        }
        reverse = insert_selected(
            &mut source_engine,
            &selected,
            metadata.source_parent.as_deref(),
            metadata.source_index,
            operation,
        )?;
        if let Some(placeholder) = &original.source_placeholder_cid {
            source_engine
                .apply_content_operation(&collab_core::Operation::DeleteNode {
                    id: placeholder.as_str().into(),
                })
                .map_err(map)?;
        }
    }
    target_engine
        .apply_content_operation(&collab_core::Operation::DeleteNode {
            id: root.as_str().into(),
        })
        .map_err(map)?;
    let target_bytes = target_engine.export_from(&target_base).map_err(map)?;
    let target_update = write::replay_stable_update_id(b.document_id, &format!("undo:{id}:target"));
    let target_prepared = match write::hydrate_and_apply(
        db,
        &runtime::runtime().cache,
        b.document_id,
        target_update,
        &target_bytes,
        Some(&expected_target),
    )
    .await?
    {
        HydrateOutcome::Prepared(prepared) => prepared,
        HydrateOutcome::Rejected(_) => return Err(error("transfer_undo_conflict")),
    };
    let source_change = if op_kind == wire::Kind::Move {
        let bytes = source_engine.export_from(&source_base).map_err(map)?;
        let update = write::replay_stable_update_id(a.document_id, &format!("undo:{id}:source"));
        let prepared = match write::hydrate_and_apply(
            db,
            &runtime::runtime().cache,
            a.document_id,
            update,
            &bytes,
            Some(&expected_source),
        )
        .await?
        {
            HydrateOutcome::Prepared(prepared) => prepared,
            HydrateOutcome::Rejected(_) => return Err(error("transfer_undo_conflict")),
        };
        Some((bytes, update, prepared))
    } else {
        None
    };
    let tx = db.begin().await?;
    tx.execute(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "SELECT id FROM flow_page_block_transfers WHERE id=$1 FOR UPDATE",
        vec![id.into()],
    ))
    .await?;
    let current = load(&tx, id, actor).await?;
    if current.state == "undone" {
        tx.rollback().await?;
        if current.undo_key.as_deref() != Some(key) || current.undo_body.as_ref() != Some(&body) {
            return Err(ApiError::idempotency_key_body_drift(
                "transfer Undo confirmation changed",
            ));
        }
        return get(db, actor, id).await;
    }
    if current.state != "accepted" {
        return Err(error("transfer_undo_conflict"));
    }
    let mut proofs = vec![source_proof, target_proof];
    if op_kind == wire::Kind::Move {
        proofs.extend(child_link_proofs(&tx, actor, source.id, None, &selected).await?);
    }
    scope::fence_many(&tx, &proofs).await?;
    require_transfer_write(&tx, &target).await?;
    if op_kind == wire::Kind::Move {
        require_transfer_write(&tx, &source).await?;
    }
    lock_transfer_objects(&tx, source.id, target.id, &proofs).await?;
    for doc in &doc_ids {
        let head = tx
            .query_one(Statement::from_sql_and_values(
                DbBackend::Postgres,
                "SELECT head_frontier FROM collab_documents WHERE id=$1 FOR UPDATE",
                vec![(*doc).into()],
            ))
            .await?
            .ok_or_else(ApiError::internal)?;
        let expected = if *doc == a.document_id {
            &expected_source
        } else {
            &expected_target
        };
        if head.try_get::<Vec<u8>>("", "head_frontier")? != *expected {
            return Err(error("transfer_undo_conflict"));
        }
    }
    let target_placements = copied_placements(&tx, &target, &original.mappings).await?;
    let logical_ids = target_placements
        .iter()
        .map(|placement| placement.logical_id)
        .collect::<Vec<_>>();
    lock_affected_shares(&tx, logical_ids.clone()).await?;
    if op_kind == wire::Kind::Copy {
        let shares = super::page_transfer_prepare::affected_shares(&tx, &logical_ids)
            .await
            .map_err(|_| ApiError::internal())?;
        if !shares.is_empty() {
            return Err(error("transfer_copy_undo_requires_share_revocation"));
        }
        let expected_placements: Vec<CopiedPlacement> = serde_json::from_value(
            current
                .copied_placements
                .ok_or_else(|| error("transfer_undo_unavailable"))?,
        )
        .map_err(|_| ApiError::internal())?;
        if target_placements != expected_placements {
            return Err(error("transfer_identity_changed"));
        }
    }
    let affected = if let Some((_, _, prepared)) = &source_change {
        let reversed_meta = SelectionMetadata {
            blocks: metadata
                .blocks
                .iter()
                .map(|block| {
                    let forward = original
                        .mappings
                        .iter()
                        .find(|mapping| mapping.source_block_cid == block.cid)
                        .ok_or_else(ApiError::internal)?;
                    Ok(Selected {
                        cid: forward.target_block_cid.clone(),
                        logical_id: block.logical_id,
                        generation: block.generation.checked_add(1).ok_or_else(ApiError::internal)?,
                        parent_cid: None,
                        preorder: block.preorder,
                    })
                })
                .collect::<Result<Vec<_>, ApiError>>()?,
            source_parent: None,
            source_index: 0,
            target_parent: metadata.source_parent.clone(),
            target_index: metadata.source_index,
            reviewed_shares: Vec::new(),
        };
        move_registry(&tx, &target, &source, &reversed_meta, &reverse, prepared).await?
    } else {
        0
    };
    let source_head = if let Some((bytes, update, prepared)) = &source_change {
        stage_document(&tx, &source, actor, a.document_id, *update, bytes, prepared, operation).await?
    } else {
        wire::AcceptedHead {
            object_id: source.id,
            accepted_seq: a.head_seq,
            frontier: B64.encode(&expected_source),
        }
    };
    let target_head = stage_document(
        &tx,
        &target,
        actor,
        b.document_id,
        target_update,
        &target_bytes,
        &target_prepared,
        operation,
    )
    .await?;
    let mappings = if op_kind == wire::Kind::Move {
        reverse
            .into_iter()
            .map(|(target_block_cid, source_block_cid)| wire::Mapping {
                source_block_cid,
                target_block_cid,
                identity: wire::Identity::Preserved,
            })
            .collect()
    } else {
        original.mappings
    };
    let receipt = wire::Receipt {
        changed_page_ids: if op_kind == wire::Kind::Move {
            vec![source.id, target.id]
        } else {
            vec![target.id]
        },
        source: source_head,
        target: target_head,
        mappings,
        source_placeholder_cid: None,
        affected_share_count: affected,
        undo: wire::Undo::Unavailable,
    };
    tx.execute(Statement::from_sql_and_values(DbBackend::Postgres,"UPDATE flow_page_block_transfers SET state='undone',receipt=$2,undo_key=$3,undo_body=$4,updated_at=now() WHERE id=$1",vec![id.into(),serde_json::to_value(&receipt).map_err(|_|ApiError::internal())?.into(),key.to_string().into(),body.into()])).await?;
    tx.commit().await?;
    if let Some((bytes, update, prepared)) = &source_change
        && let Err(error) = publish_committed(
            db,
            &source,
            actor,
            a.document_id,
            *update,
            bytes,
            prepared,
            &receipt.source,
        )
        .await
    {
        tracing::warn!(%error,"committed transfer Undo source delivery will backfill");
    }
    if let Err(error) = publish_committed(
        db,
        &target,
        actor,
        b.document_id,
        target_update,
        &target_bytes,
        &target_prepared,
        &receipt.target,
    )
    .await
    {
        tracing::warn!(%error,"committed transfer Undo target delivery will backfill");
    }
    drop(guards);
    serde_json::to_value(wire::Accepted {
        transfer_id: id,
        kind: op_kind,
        state: wire::State::Undone,
        receipt: Some(receipt),
    })
    .map_err(|_| ApiError::internal())
}

pub async fn cancel(db: &DatabaseConnection, actor: Uuid, id: Uuid, key: &str) -> Result<Value, ApiError> {
    check_key(key)?;
    let tx = db.begin().await?;
    tx.execute(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "SELECT id FROM flow_page_block_transfers WHERE id=$1 AND actor_id=$2 FOR UPDATE",
        vec![id.into(), actor.into()],
    ))
    .await?;
    expire(&tx, id, actor).await?;
    let row = load(&tx, id, actor).await?;
    if row.state == "prepared" {
        tx.execute(Statement::from_sql_and_values(DbBackend::Postgres,"UPDATE flow_page_block_transfers SET state='cancelled',cancel_key=$2,cancel_body='{}'::jsonb,updated_at=now() WHERE id=$1",vec![id.into(),key.to_string().into()])).await?;
    }
    tx.commit().await?;
    get(db, actor, id).await
}

async fn publish_committed(
    db: &DatabaseConnection,
    identity: &scope::ObjectIdentity,
    actor: Uuid,
    document: Uuid,
    update: Uuid,
    bytes: &[u8],
    prepared: &super::collab::write::Prepared,
    head: &wire::AcceptedHead,
) -> Result<(), ApiError> {
    use super::collab::{runtime, write};
    if let Ok(candidate) = prepared.candidate.fork() {
        runtime::runtime().cache.put(
            document,
            candidate,
            prepared.observed.format_version.clone(),
            head.accepted_seq,
            prepared.after_frontier.clone(),
            prepared.decoded_bytes_hint,
        );
    }
    if let Some(workspace) = identity.workspace_id {
        let event = db
            .query_one(Statement::from_sql_and_values(
                DbBackend::Postgres,
                "SELECT event_id FROM collab_updates WHERE document_id=$1 AND seq=$2",
                vec![document.into(), head.accepted_seq.into()],
            ))
            .await?
            .ok_or_else(ApiError::internal)?
            .try_get::<Uuid>("", "event_id")?;
        let accepted = write::Accepted {
            update_id: update,
            head_seq: head.accepted_seq,
            head_frontier: prepared.after_frontier.clone(),
            projection_seq: head.accepted_seq,
            event_id: event,
            before_frontier: prepared.observed.head_frontier.clone(),
            should_advance_snapshot: false,
        };
        let request = write::UpdateRequest {
            document_id: document,
            update_id: update,
            bytes: bytes.to_vec(),
            idempotency_key: None,
            event_idempotency_key: None,
            origin_client_id: Some("block-transfer".into()),
            message: None,
            actor_id: actor,
            actor_is_bot: false,
            workspace_id: workspace,
            checked_epoch: scope::revision(db, identity.scope_id).await?,
            expected_frontier: None,
            origin: super::event_origin::CommandOrigin::first_request_from(super::event_origin::EventSurface::Rest),
        };
        super::collab::fanout::note_local_delivery(document, head.accepted_seq);
        if let Err(error) =
            super::collab::fanout::publish_document_update(db, workspace, document, head.accepted_seq).await
        {
            tracing::warn!(%error,%document,"committed transfer notice unavailable; receiver backfill remains authoritative");
        }
        write::broadcast_committed_update(&runtime::runtime().registry, None, &request, &accepted);
    }
    Ok(())
}
