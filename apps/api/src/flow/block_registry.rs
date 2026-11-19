//! Private Page identity registry. No recipient DTO or authorization is derived from these rows.
//! Caller must hold the accepted Page-head lock and use its existing `DatabaseTransaction`.
use crate::error::ApiError;
use collab_core::block_identity::{Alias, AliasState, reconcile_alias};
use sea_orm::{ConnectionTrait, DatabaseTransaction, DbBackend, FromQueryResult, Statement};
use std::collections::{BTreeSet, HashMap};
use uuid::Uuid;

#[derive(Clone, Debug)]
pub struct PreparedBlock {
    pub physical_cid: String,
    pub parent_physical_cid: Option<String>,
    pub order_key: String,
    pub asset_id: Option<Uuid>,
    pub legacy_alias: Option<String>,
    /// Digest of normalized typed content only; never Page metadata or private comments.
    pub content_hash: [u8; 32],
}
/// Prepare hashes outside the accepted-head lock from validated active semantic content.
/// Unknown property bags, private discussions, source parent/title/sequence are excluded.
pub fn prepare(snapshot: &collab_core::SemanticSnapshot) -> Result<Vec<PreparedBlock>, ApiError> {
    use sha2::{Digest, Sha256};
    let mut result = Vec::new();
    let canonical = snapshot.nodes.values().any(|node| {
        node.properties
            .get("content_source")
            .is_some_and(|source| source == "editor_doc")
    });
    for (cid, node) in &snapshot.nodes {
        if node.deleted
            || node.kind != collab_core::NodeKind::Block
            || (canonical
                && node
                    .properties
                    .get("content_source")
                    .is_none_or(|source| source != "editor_doc"))
        {
            continue;
        }
        let kind = node.properties.get("block_type").map_or("paragraph", String::as_str);
        let slots: &[&str] = match kind {
            "heading" => &["level"],
            "orderedItem" => &["order"],
            "todoItem" => &["checked"],
            "toggle" => &["collapsed"],
            "image" => &["asset_id", "alt", "caption"],
            "attachment" => &["asset_id", "name", "mime", "caption"],
            // Legacy opaque blocks receive identity, but cannot be published by the typed projector.
            _ => &[],
        };
        let mut content = serde_json::Map::new();
        content.insert("kind".into(), serde_json::json!(kind));
        content.insert(
            "text".into(),
            serde_json::json!(if kind == "childPage" { "" } else { &node.text }),
        );
        for key in slots {
            if let Some(value) = node.properties.get(*key) {
                content.insert((*key).into(), serde_json::json!(value));
            }
        }
        let spans: Vec<collab_core::page_inline::InlineSpan> = node
            .properties
            .get("inline_spans")
            .map(|raw| serde_json::from_str(raw))
            .transpose()
            .map_err(|_| ApiError::invalid_update("invalid typed inline spans"))?
            .unwrap_or_default();
        for span in &spans {
            if span.start >= span.end
                || span.end > node.text.len()
                || !node.text.is_char_boundary(span.start)
                || !node.text.is_char_boundary(span.end)
                || (span.mark == collab_core::page_inline::InlineMark::Link
                    && !span.href.as_deref().is_some_and(collab_core::page_inline::safe_href))
                || (span.mark != collab_core::page_inline::InlineMark::Link && span.href.is_some())
            {
                return Err(ApiError::invalid_update("invalid shared inline range"));
            }
        }
        content.insert(
            "spans".into(),
            serde_json::to_value(spans).map_err(|_| ApiError::internal())?,
        );
        let encoded = serde_json::to_vec(&content).map_err(|_| ApiError::internal())?;
        result.push(PreparedBlock {
            physical_cid: cid.to_string(),
            parent_physical_cid: node.parent.as_ref().map(ToString::to_string),
            legacy_alias: node
                .properties
                .get("logical_id")
                .filter(|alias| !alias.is_empty() && alias.chars().count() <= 128 && alias.len() <= 256)
                .cloned(),
            order_key: node.order_key.clone(),
            asset_id: if matches!(kind, "image" | "attachment") {
                Some(
                    node.properties
                        .get("asset_id")
                        .and_then(|id| Uuid::parse_str(id).ok())
                        .ok_or_else(|| ApiError::invalid_update("invalid typed asset reference"))?,
                )
            } else {
                None
            },
            content_hash: Sha256::digest(encoded).into(),
        });
    }
    Ok(result)
}

/// Digest source-independent selected content and relationships. A single block's private
/// parent/order never changes this value; relative selected member ordering does.
pub fn visible_graph_hash(blocks: &[PreparedBlock], members: &[(Uuid, String)]) -> Result<[u8; 32], ApiError> {
    use sha2::{Digest, Sha256};
    let by_cid = blocks
        .iter()
        .map(|block| (block.physical_cid.as_str(), block))
        .collect::<std::collections::BTreeMap<_, _>>();
    let member_by_cid = members
        .iter()
        .map(|(id, cid)| (cid.as_str(), *id))
        .collect::<std::collections::BTreeMap<_, _>>();
    let mut selected = Vec::new();
    for (id, cid) in members {
        let block = by_cid
            .get(cid.as_str())
            .ok_or_else(|| ApiError::invalid_update("missing selected logical placement"))?;
        selected.push((*id, *block));
    }
    selected
        .sort_by(|(left_id, left), (right_id, right)| left.order_key.cmp(&right.order_key).then(left_id.cmp(right_id)));
    let mut normalized = Vec::new();
    for (id, block) in selected {
        let parent = block
            .parent_physical_cid
            .as_deref()
            .and_then(|cid| member_by_cid.get(cid))
            .copied();
        normalized.push((id, parent, block.content_hash));
    }
    let bytes = serde_json::to_vec(&normalized).map_err(|_| ApiError::internal())?;
    Ok(Sha256::digest(bytes).into())
}

/// Test-only statement counter: proves the statement count of `reconcile` does not depend on page size.
#[cfg(test)]
pub(crate) mod statement_probe {
    use std::cell::Cell;
    use std::future::Future;

    tokio::task_local! {
        static COUNTER: Cell<usize>;
    }

    /// Record one database round trip for the surrounding `count` scope, if any.
    pub fn note() {
        let _ = COUNTER.try_with(|counter| counter.set(counter.get() + 1));
    }

    /// Run `future` and return its output with the number of statements noted while it ran.
    pub async fn count<F: Future>(future: F) -> (F::Output, usize) {
        COUNTER
            .scope(Cell::new(0), async {
                let output = future.await;
                (output, COUNTER.with(Cell::get))
            })
            .await
    }
}

/// Latest placement of one physical CID, as locked by the bulk read of `reconcile`.
#[derive(FromQueryResult)]
struct LatestPlacement {
    id: Uuid,
    physical_cid: String,
    logical_id: Uuid,
    generation: i64,
    state: String,
    content_hash: Vec<u8>,
}

/// Column-wise bulk write plan built by the decision pass; no database access happens while filling it.
#[derive(Default)]
struct WritePlan {
    fresh_ids: Vec<Uuid>,
    fence_cids: Vec<String>,
    fence_logical: Vec<Uuid>,
    fence_retired: Vec<bool>,
    hash_ids: Vec<Uuid>,
    hash_values: Vec<Vec<u8>>,
    activate_ids: Vec<Uuid>,
    new_ids: Vec<Uuid>,
    new_logical: Vec<Uuid>,
    new_cids: Vec<String>,
    new_generations: Vec<i64>,
    new_hashes: Vec<Vec<u8>>,
}

/// Reconcile one accepted head. No independent commit, connection or raw client identity input.
///
/// Multi-Page operations must lock all Page heads in UUID order before calling this function.
/// Affected shares are suspended in this transaction; returned IDs are private audit metadata.
///
/// The statement count is independent of the number of blocks: one bulk locking read, a pure
/// decision pass, at most five bulk writes, and rows whose content did not change are not rewritten.
pub async fn reconcile(
    tx: &DatabaseTransaction,
    scope_id: Uuid,
    page_id: Uuid,
    accepted_head: i64,
    blocks: &[PreparedBlock],
) -> Result<Vec<Uuid>, ApiError> {
    let mut seen = BTreeSet::new();
    if accepted_head < 0
        || blocks
            .iter()
            .any(|b| b.physical_cid.is_empty() || b.physical_cid.len() > 256 || !seen.insert(b.physical_cid.as_str()))
    {
        return Err(ApiError::invalid_update("invalid logical block reconciliation input"));
    }
    // The checkpoint lock additionally serializes initial registration for existing Pages.
    #[cfg(test)]
    statement_probe::note();
    tx.execute(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "INSERT INTO flow_block_checkpoints(scope_id,page_id,accepted_head) VALUES($1,$2,0) ON CONFLICT DO NOTHING",
        vec![scope_id.into(), page_id.into()],
    ))
    .await?;
    #[cfg(test)]
    statement_probe::note();
    let checkpoint = tx
        .query_one(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT accepted_head FROM flow_block_checkpoints WHERE scope_id=$1 AND page_id=$2 FOR UPDATE",
            vec![scope_id.into(), page_id.into()],
        ))
        .await?
        .ok_or_else(|| ApiError::invalid_update("missing block checkpoint"))?;
    if checkpoint.try_get::<i64>("", "accepted_head")? > accepted_head {
        return Err(ApiError::invalid_update("stale block reconciliation head"));
    }
    let cids = blocks.iter().map(|b| b.physical_cid.clone()).collect::<Vec<_>>();
    // Locks exactly the rows the former per-block query locked: the latest placement of every CID.
    #[cfg(test)]
    statement_probe::note();
    let latest = LatestPlacement::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "SELECT p.id,p.physical_cid,p.logical_id,p.generation,p.state,p.content_hash FROM flow_block_placements p WHERE p.id IN (SELECT DISTINCT ON (h.physical_cid) h.id FROM flow_block_placements h WHERE h.scope_id=$1 AND h.page_id=$2 AND h.physical_cid=ANY($3::text[]) ORDER BY h.physical_cid,h.alias_order DESC) ORDER BY p.id FOR UPDATE OF p",
        vec![scope_id.into(), page_id.into(), cids.clone().into()],
    ))
    .all(tx)
    .await?;
    let mut latest_by_cid = HashMap::with_capacity(latest.len());
    for row in latest {
        if latest_by_cid.insert(row.physical_cid.clone(), row).is_some() {
            return Err(ApiError::internal());
        }
    }
    let mut suspend = Vec::new();
    let mut plan = WritePlan::default();
    for block in blocks {
        let old = latest_by_cid.get(&block.physical_cid);
        let alias = old.map(|old| Alias {
            logical_id: old.logical_id,
            generation: old.generation.unsigned_abs(),
            state: match old.state.as_str() {
                "active" => AliasState::Active,
                "deleted" => AliasState::Deleted,
                _ => AliasState::Moved,
            },
        });
        let decision = reconcile_alias(alias, Uuid::new_v4())
            .map_err(|_| ApiError::invalid_update("logical placement generation conflict"))?;
        let generation = i64::try_from(decision.generation)
            .map_err(|_| ApiError::invalid_update("logical placement generation exhausted"))?;
        if decision.fresh_identity {
            plan.fresh_ids.push(decision.logical_id);
        }
        plan.fence_cids.push(block.physical_cid.clone());
        plan.fence_logical.push(decision.logical_id);
        plan.fence_retired
            .push(alias.is_some_and(|alias| alias.state != AliasState::Active));
        if let Some(old) = old.filter(|row| row.state == "active") {
            if old.content_hash != block.content_hash {
                plan.hash_ids.push(old.id);
                plan.hash_values.push(block.content_hash.to_vec());
            }
        } else {
            plan.activate_ids.push(decision.logical_id);
            plan.new_ids.push(Uuid::new_v4());
            plan.new_logical.push(decision.logical_id);
            plan.new_cids.push(block.physical_cid.clone());
            plan.new_generations.push(generation);
            plan.new_hashes.push(block.content_hash.to_vec());
        }
        if decision.suspend_shares {
            suspend.push(decision.logical_id);
        }
    }
    if !plan.fresh_ids.is_empty() {
        #[cfg(test)]
        statement_probe::note();
        tx.execute(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "INSERT INTO flow_logical_blocks(id,state) SELECT v.id,'active' FROM unnest($1::uuid[]) WITH ORDINALITY AS v(id,ord) ORDER BY v.ord",
            vec![plan.fresh_ids.into()],
        ))
        .await?;
    }
    if !plan.fence_cids.is_empty() {
        // Data-identical to the former per-block upsert, but an unchanged fence is not rewritten.
        #[cfg(test)]
        statement_probe::note();
        tx.execute(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "INSERT INTO flow_block_alias_fences(scope_id,page_id,physical_cid,first_logical_id,ever_retired) SELECT $1,$2,v.cid,v.logical_id,v.retired FROM unnest($3::text[],$4::uuid[],$5::bool[]) WITH ORDINALITY AS v(cid,logical_id,retired,ord) ORDER BY v.ord ON CONFLICT(scope_id,page_id,physical_cid) DO UPDATE SET ever_retired=true WHERE NOT flow_block_alias_fences.ever_retired AND (EXCLUDED.ever_retired OR flow_block_alias_fences.first_logical_id<>EXCLUDED.first_logical_id)",
            vec![
                scope_id.into(),
                page_id.into(),
                plan.fence_cids.into(),
                plan.fence_logical.into(),
                plan.fence_retired.into(),
            ],
        ))
        .await?;
    }
    if !plan.hash_ids.is_empty() {
        #[cfg(test)]
        statement_probe::note();
        tx.execute(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "UPDATE flow_block_placements p SET content_hash=v.hash FROM unnest($1::uuid[],$2::bytea[]) AS v(id,hash) WHERE p.id=v.id AND p.content_hash IS DISTINCT FROM v.hash",
            vec![plan.hash_ids.into(), plan.hash_values.into()],
        ))
        .await?;
    }
    if !plan.activate_ids.is_empty() {
        #[cfg(test)]
        statement_probe::note();
        tx.execute(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "UPDATE flow_logical_blocks SET state='active' WHERE id=ANY($1::uuid[]) AND state<>'active'",
            vec![plan.activate_ids.into()],
        ))
        .await?;
        #[cfg(test)]
        statement_probe::note();
        tx.execute(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "INSERT INTO flow_block_placements(id,logical_id,scope_id,page_id,physical_cid,generation,state,content_hash) SELECT v.id,v.logical_id,$1,$2,v.cid,v.generation,'active',v.hash FROM unnest($3::uuid[],$4::uuid[],$5::text[],$6::int8[],$7::bytea[]) WITH ORDINALITY AS v(id,logical_id,cid,generation,hash,ord) ORDER BY v.ord",
            vec![
                scope_id.into(),
                page_id.into(),
                plan.new_ids.into(),
                plan.new_logical.into(),
                plan.new_cids.into(),
                plan.new_generations.into(),
                plan.new_hashes.into(),
            ],
        ))
        .await?;
    }
    #[cfg(test)]
    statement_probe::note();
    let removed = tx.query_all(Statement::from_sql_and_values(DbBackend::Postgres,
        "UPDATE flow_block_placements SET state='deleted',retired_at=now() WHERE scope_id=$1 AND page_id=$2 AND state='active' AND NOT(physical_cid=ANY($3::text[])) RETURNING logical_id",
        vec![scope_id.into(),page_id.into(),cids.into()])).await?;
    let mut removed_ids = Vec::with_capacity(removed.len());
    for row in removed {
        removed_ids.push(row.try_get::<Uuid>("", "logical_id")?);
    }
    if !removed_ids.is_empty() {
        #[cfg(test)]
        statement_probe::note();
        tx.execute(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "UPDATE flow_logical_blocks SET state='deleted' WHERE id=ANY($1::uuid[])",
            vec![removed_ids.clone().into()],
        ))
        .await?;
        suspend.extend(removed_ids);
    }
    #[cfg(test)]
    statement_probe::note();
    tx.execute(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "UPDATE flow_block_checkpoints SET accepted_head=$3 WHERE scope_id=$1 AND page_id=$2",
        vec![scope_id.into(), page_id.into(), accepted_head.into()],
    ))
    .await?;
    suspend.sort_unstable();
    suspend.dedup();
    if !suspend.is_empty() {
        #[cfg(test)]
        statement_probe::note();
        tx.execute(Statement::from_sql_and_values(DbBackend::Postgres,
            "UPDATE flow_block_shares SET state='suspended',auth_revision=auth_revision+1 WHERE state='active' AND id IN (SELECT share_id FROM flow_block_share_members WHERE logical_id=ANY($1::uuid[]))",
            vec![suspend.clone().into()])).await?;
    }
    // Project only fixed member relationships and relative ordering, never absolute source position.
    let by_cid = blocks
        .iter()
        .map(|block| (block.physical_cid.as_str(), block))
        .collect::<HashMap<_, _>>();
    #[cfg(test)]
    statement_probe::note();
    let shares=tx.query_all(Statement::from_sql_and_values(DbBackend::Postgres,
        "SELECT id,projection_hash FROM flow_block_shares WHERE source_scope_id=$1 AND source_page_id=$2 AND state<>'revoked' ORDER BY id FOR UPDATE",
        vec![scope_id.into(),page_id.into()])).await?;
    for share in shares {
        let share_id = share.try_get::<Uuid>("", "id")?;
        #[cfg(test)]
        statement_probe::note();
        let rows=tx.query_all(Statement::from_sql_and_values(DbBackend::Postgres,
            "SELECT m.member_id,p.physical_cid FROM flow_block_share_members m JOIN flow_block_placements p ON p.logical_id=m.logical_id AND p.state='active' WHERE m.share_id=$1 AND p.scope_id=$2 AND p.page_id=$3",
            vec![share_id.into(),scope_id.into(),page_id.into()])).await?;
        let mut members = Vec::new();
        for row in rows {
            members.push((
                row.try_get::<Uuid>("", "member_id")?,
                row.try_get::<String>("", "physical_cid")?,
            ));
        }
        for (member, cid) in &members {
            if let Some(asset) = by_cid.get(cid.as_str()).and_then(|block| block.asset_id) {
                #[cfg(test)]
                statement_probe::note();
                tx.execute(Statement::from_sql_and_values(DbBackend::Postgres,"INSERT INTO flow_block_share_asset_aliases(share_id,member_id,alias_id,source_scope_id,source_page_id,asset_id) VALUES($1,$2,$3,$4,$5,$6) ON CONFLICT(share_id,member_id,asset_id) DO NOTHING",vec![share_id.into(),(*member).into(),Uuid::new_v4().into(),scope_id.into(),page_id.into(),asset.into()])).await?;
            }
        }
        let hash = visible_graph_hash(blocks, &members)?;
        if share.try_get::<Vec<u8>>("", "projection_hash")? != hash {
            #[cfg(test)]
            statement_probe::note();
            tx.execute(Statement::from_sql_and_values(
                DbBackend::Postgres,
                "UPDATE flow_block_shares SET projection_hash=$2,content_revision=content_revision+1 WHERE id=$1",
                vec![share_id.into(), hash.to_vec().into()],
            ))
            .await?;
        }
    }
    super::block_anchors::sync_legacy(tx, page_id, blocks).await?;
    Ok(suspend)
}

/// Accepted-write hook, inactive until the complete registry-capable process barrier is raised.
/// The process barrier is separate from client editor model5.
pub async fn reconcile_accepted(
    tx: &DatabaseTransaction,
    object: Uuid,
    seq: i64,
    blocks: &[PreparedBlock],
) -> Result<(), ApiError> {
    if super::scope::capabilities(tx).await?.minimum_reader_version < 5 {
        return Ok(());
    }
    let identity = super::scope::identity(tx, object).await?;
    if identity.object_type != "page" {
        return Ok(());
    }
    reconcile(tx, identity.scope_id, object, seq, blocks).await?;
    Ok(())
}

/// Only the server-generated legacy editor initializer may carry an existing legacy identity
/// to its new canonical Map. Raw client aliases never enter this transition.
pub(crate) async fn preserve_editor_aliases(
    tx: &DatabaseTransaction,
    page: Uuid,
    blocks: &[PreparedBlock],
) -> Result<(), ApiError> {
    if super::scope::capabilities(tx).await?.minimum_reader_version < 5 {
        return Ok(());
    }
    let scope = super::scope::identity(tx, page).await?.scope_id;
    for block in blocks {
        let Some(alias) = &block.legacy_alias else {
            continue;
        };
        if alias == &block.physical_cid {
            continue;
        }
        let old=tx.query_one(Statement::from_sql_and_values(DbBackend::Postgres,"SELECT id,logical_id,generation FROM flow_block_placements WHERE scope_id=$1 AND page_id=$2 AND physical_cid=$3 AND state='active' FOR UPDATE",vec![scope.into(),page.into(),alias.clone().into()])).await?;
        let Some(old) = old else {
            continue;
        };
        if tx
            .query_one(Statement::from_sql_and_values(
                DbBackend::Postgres,
                "SELECT physical_cid FROM flow_block_alias_fences WHERE scope_id=$1 AND page_id=$2 AND physical_cid=$3",
                vec![scope.into(), page.into(), block.physical_cid.clone().into()],
            ))
            .await?
            .is_some()
        {
            return Err(ApiError::policy_rejected("editor carrier identity already exists"));
        }
        let logical = old.try_get::<Uuid>("", "logical_id")?;
        let generation = old
            .try_get::<i64>("", "generation")?
            .checked_add(1)
            .ok_or_else(ApiError::internal)?;
        tx.execute(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "UPDATE flow_block_placements SET state='moved',retired_at=now() WHERE id=$1",
            vec![old.try_get::<Uuid>("", "id")?.into()],
        ))
        .await?;
        tx.execute(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "INSERT INTO flow_block_alias_fences(scope_id,page_id,physical_cid,first_logical_id) VALUES($1,$2,$3,$4)",
            vec![
                scope.into(),
                page.into(),
                block.physical_cid.clone().into(),
                logical.into(),
            ],
        ))
        .await?;
        tx.execute(Statement::from_sql_and_values(DbBackend::Postgres,"INSERT INTO flow_block_placements(id,logical_id,scope_id,page_id,physical_cid,generation,state,content_hash) VALUES($1,$2,$3,$4,$5,$6,'active',$7)",vec![Uuid::new_v4().into(),logical.into(),scope.into(),page.into(),block.physical_cid.clone().into(),generation.into(),block.content_hash.to_vec().into()])).await?;
    }
    Ok(())
}

/// Page fixtures shared by the registry and anchor tests.
#[cfg(test)]
pub(crate) mod fixtures {
    use super::PreparedBlock;
    use crate::routes::context::tenant_fixture::exec;
    use sea_orm::{DatabaseConnection, TransactionTrait};
    use uuid::Uuid;

    /// Register the three reader leases through the production registration function, so Page
    /// writes are not refused with `scope_runtime_not_ready` at reader floor 5. Local to these
    /// tests: replace with the shared scratch-database helper when the harness branch is merged.
    pub async fn register_reader_leases(db: &DatabaseConnection) {
        for component in ["api", "worker", "maintenance"] {
            crate::flow::scope_runtime::renew(db, Uuid::new_v4(), component)
                .await
                .unwrap();
        }
    }

    /// A personal scope with one navigator and one Page. Returns `(actor, scope, page)`.
    pub async fn page(db: &DatabaseConnection) -> (Uuid, Uuid, Uuid) {
        let actor = Uuid::new_v4();
        let scope = Uuid::new_v4();
        let page = Uuid::new_v4();
        exec(db,"INSERT INTO users(id,email,password_hash,name,role,is_active) VALUES($1,$2,'!','Registry fixture','user',true)",vec![actor.into(),format!("{actor}@registry.test").into()]).await;
        exec(
            db,
            "INSERT INTO flow_scopes(id,kind,owner_user_id) VALUES($1,'personal',$2)",
            vec![scope.into(), actor.into()],
        )
        .await;
        exec(
            db,
            "INSERT INTO flow_scope_settings(scope_id) VALUES($1)",
            vec![scope.into()],
        )
        .await;
        let tx = db.begin().await.unwrap();
        let navigator = Uuid::new_v4();
        crate::flow::scope::insert_object_with_identity(
            &tx,
            scope,
            actor,
            "navigator",
            None,
            "",
            None,
            navigator,
            Uuid::new_v4(),
        )
        .await
        .unwrap();
        crate::flow::scope::insert_object_with_identity(
            &tx,
            scope,
            actor,
            "page",
            Some(navigator),
            "Registry fixture",
            None,
            page,
            Uuid::new_v4(),
        )
        .await
        .unwrap();
        tx.commit().await.unwrap();
        (actor, scope, page)
    }

    /// Block `index` at content `version`; the hash is unique per pair.
    pub fn block(index: usize, version: u8) -> PreparedBlock {
        let mut content_hash = [version; 32];
        content_hash[..8].copy_from_slice(&u64::try_from(index).unwrap().to_be_bytes());
        PreparedBlock {
            physical_cid: format!("cid:{index}:Map"),
            parent_physical_cid: None,
            order_key: format!("{index:08}"),
            asset_id: None,
            legacy_alias: None,
            content_hash,
        }
    }

    pub fn blocks(count: usize, version: u8) -> Vec<PreparedBlock> {
        (0..count).map(|index| block(index, version)).collect()
    }
}

#[cfg(test)]
#[allow(clippy::indexing_slicing, clippy::print_stderr)]
mod tests {
    use super::*;
    use crate::{routes::context::tenant_fixture::exec, scratch_or_skip};
    use sea_orm::TransactionTrait;

    async fn logical(tx: &DatabaseTransaction, page: Uuid) -> (Uuid, i64) {
        let row = tx
            .query_one(Statement::from_sql_and_values(
                DbBackend::Postgres,
                "SELECT logical_id,generation FROM flow_block_placements WHERE page_id=$1 AND state='active'",
                vec![page.into()],
            ))
            .await
            .unwrap()
            .unwrap();
        (
            row.try_get("", "logical_id").unwrap(),
            row.try_get("", "generation").unwrap(),
        )
    }

    #[test]
    fn actual_semantic_derived_indent_and_private_move_do_not_change_single_projection() {
        let mut engine = collab_core::LoroCollabEngine::new_empty(99220);
        engine.initialize_page_editor().unwrap();
        engine.initialize_page_structure().unwrap();
        let root = engine.semantic_snapshot().unwrap().nodes.keys().next().unwrap().clone();
        for (key, value) in [("block_type", "orderedItem"), ("order", "7")] {
            engine
                .apply_content_operation(&collab_core::Operation::SetProperty {
                    id: root.clone(),
                    key: key.into(),
                    value: value.into(),
                })
                .unwrap();
        }
        engine
            .apply_content_operation(&collab_core::Operation::CreateNode {
                id: "private-parent".into(),
                parent: None,
                index: 1,
                kind: collab_core::NodeKind::Block,
            })
            .unwrap();
        let before = engine.semantic_snapshot().unwrap();
        let before_blocks = prepare(&before).unwrap();
        let selected = vec![(Uuid::from_u128(1), root.to_string())];
        let digest = visible_graph_hash(&before_blocks, &selected).unwrap();
        engine
            .apply_content_operation(&collab_core::Operation::MoveNode {
                id: root.clone(),
                new_parent: Some("private-parent".into()),
                index: 0,
            })
            .unwrap();
        let after = engine.semantic_snapshot().unwrap();
        assert_ne!(
            before.nodes.get(&root).unwrap().properties.get("indent"),
            after.nodes.get(&root).unwrap().properties.get("indent")
        );
        assert_eq!(after.nodes.get(&root).unwrap().properties.get("order").unwrap(), "7");
        let after_blocks = prepare(&after).unwrap();
        assert_eq!(digest, visible_graph_hash(&after_blocks, &selected).unwrap());
        assert_eq!(
            before_blocks
                .iter()
                .find(|block| block.physical_cid == root.as_ref())
                .unwrap()
                .content_hash,
            after_blocks
                .iter()
                .find(|block| block.physical_cid == root.as_ref())
                .unwrap()
                .content_hash
        );
    }

    #[test]
    fn projection_revision_ignores_private_position_but_tracks_selected_graph() {
        let a = Uuid::from_u128(1);
        let b = Uuid::from_u128(2);
        let mut blocks = vec![
            PreparedBlock {
                physical_cid: "a".into(),
                parent_physical_cid: Some("hidden".into()),
                order_key: "10".into(),
                asset_id: None,
                legacy_alias: None,
                content_hash: [1; 32],
            },
            PreparedBlock {
                physical_cid: "b".into(),
                parent_physical_cid: None,
                order_key: "20".into(),
                asset_id: None,
                legacy_alias: None,
                content_hash: [2; 32],
            },
        ];
        let single = vec![(a, "a".into())];
        let initial = visible_graph_hash(&blocks, &single).unwrap();
        blocks
            .get_mut(0)
            .expect("first fixture block exists")
            .parent_physical_cid = Some("another private parent".into());
        blocks.get_mut(0).expect("first fixture block exists").order_key = "11".into();
        assert_eq!(initial, visible_graph_hash(&blocks, &single).unwrap());
        let both = vec![(a, "a".into()), (b, "b".into())];
        let original = visible_graph_hash(&blocks, &both).unwrap();
        blocks
            .get_mut(1)
            .expect("second fixture block exists")
            .parent_physical_cid = Some("a".into());
        assert_ne!(original, visible_graph_hash(&blocks, &both).unwrap());
        blocks
            .get_mut(1)
            .expect("second fixture block exists")
            .parent_physical_cid = None;
        blocks.get_mut(1).expect("second fixture block exists").order_key = "00".into();
        assert_ne!(original, visible_graph_hash(&blocks, &both).unwrap());
    }

    #[test]
    fn prepare_ignores_private_metadata_and_rejects_unsafe_inline() {
        let mut snapshot = collab_core::SemanticSnapshot::default();
        let id: collab_core::NodeId = "cid:fixture:Map".into();
        snapshot.nodes.insert(
            id.clone(),
            collab_core::SemanticNode {
                parent: None,
                order_key: "0".into(),
                kind: collab_core::NodeKind::Block,
                text: "中文😀".into(),
                properties: std::collections::BTreeMap::new(),
                deleted: false,
            },
        );
        let original = prepare(&snapshot).unwrap();
        let node = snapshot.nodes.get_mut(&id).unwrap();
        node.parent = Some("private-parent".into());
        node.order_key = "unrelated reorder".into();
        node.properties.insert("private_comments".into(), "secret".into());
        node.properties.insert("page_title".into(), "secret title".into());
        assert_eq!(
            prepare(&snapshot)
                .unwrap()
                .first()
                .expect("prepared block exists")
                .content_hash,
            original.first().expect("original block exists").content_hash
        );
        snapshot.nodes.get_mut(&id).unwrap().text.push('!');
        assert_ne!(
            prepare(&snapshot)
                .unwrap()
                .first()
                .expect("prepared block exists")
                .content_hash,
            original.first().expect("original block exists").content_hash
        );
        snapshot.nodes.get_mut(&id).unwrap().properties.insert(
            "inline_spans".into(),
            r#"[{"start":0,"end":3,"mark":"link","href":"javascript:bad"}]"#.into(),
        );
        assert!(prepare(&snapshot).is_err());
    }

    #[tokio::test]
    async fn registry_reconcile_restore_moved_alias_and_rollback() {
        let scratch = scratch_or_skip!(&format!("block_registry_{}", Uuid::new_v4().simple()));
        let db = &scratch.db;
        register_reader_leases(db).await;
        let actor = Uuid::new_v4();
        let scope = Uuid::new_v4();
        let page = Uuid::new_v4();
        exec(db,"INSERT INTO users(id,email,password_hash,name,role,is_active) VALUES($1,$2,'!','Registry fixture','user',true)",vec![actor.into(),format!("{actor}@registry.test").into()]).await;
        exec(
            db,
            "INSERT INTO flow_scopes(id,kind,owner_user_id) VALUES($1,'personal',$2)",
            vec![scope.into(), actor.into()],
        )
        .await;
        exec(
            db,
            "INSERT INTO flow_scope_settings(scope_id) VALUES($1)",
            vec![scope.into()],
        )
        .await;
        let tx = db.begin().await.unwrap();
        let navigator = Uuid::new_v4();
        super::super::scope::insert_object_with_identity(
            &tx,
            scope,
            actor,
            "navigator",
            None,
            "",
            None,
            navigator,
            Uuid::new_v4(),
        )
        .await
        .unwrap();
        super::super::scope::insert_object_with_identity(
            &tx,
            scope,
            actor,
            "page",
            Some(navigator),
            "Registry fixture",
            None,
            page,
            Uuid::new_v4(),
        )
        .await
        .unwrap();
        let blocks = [PreparedBlock {
            physical_cid: "cid:fixture:Map".into(),
            parent_physical_cid: None,
            order_key: "0".into(),
            asset_id: None,
            legacy_alias: None,
            content_hash: [7; 32],
        }];
        reconcile(&tx, scope, page, 0, &blocks).await.unwrap();
        let original = logical(&tx, page).await;
        let share = Uuid::new_v4();
        let member = Uuid::new_v4();
        tx.execute(Statement::from_sql_and_values(DbBackend::Postgres,"INSERT INTO flow_block_shares(id,source_scope_id,source_page_id,created_by,publisher_user_id,state,source_root_cid,selection,root_logical_id) VALUES($1,$2,$3,$4,$4,'active','cid:fixture:Map','block',$5)",vec![share.into(),scope.into(),page.into(),actor.into(),original.0.into()])).await.unwrap();
        tx.execute(Statement::from_sql_and_values(DbBackend::Postgres,"INSERT INTO flow_block_share_members(share_id,member_id,logical_id,member_order,content_hash) VALUES($1,$2,$3,0,$4)",vec![share.into(),member.into(),original.0.into(),vec![7_u8;32].into()])).await.unwrap();
        let initial_hash = visible_graph_hash(&blocks, &[(member, blocks[0].physical_cid.clone())]).unwrap();
        tx.execute(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "UPDATE flow_block_shares SET projection_hash=$2 WHERE id=$1",
            vec![share.into(), initial_hash.to_vec().into()],
        ))
        .await
        .unwrap();
        reconcile(&tx, scope, page, 2, &blocks).await.unwrap();
        let revision = tx
            .query_one(Statement::from_sql_and_values(
                DbBackend::Postgres,
                "SELECT content_revision FROM flow_block_shares WHERE id=$1",
                vec![share.into()],
            ))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(revision.try_get::<i64>("", "content_revision").unwrap(), 1);
        let edited = [PreparedBlock {
            physical_cid: blocks[0].physical_cid.clone(),
            parent_physical_cid: None,
            order_key: "0".into(),
            asset_id: None,
            legacy_alias: None,
            content_hash: [8; 32],
        }];
        reconcile(&tx, scope, page, 3, &edited).await.unwrap();
        let revision = tx
            .query_one(Statement::from_sql_and_values(
                DbBackend::Postgres,
                "SELECT content_revision FROM flow_block_shares WHERE id=$1",
                vec![share.into()],
            ))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(revision.try_get::<i64>("", "content_revision").unwrap(), 2);
        reconcile(&tx, scope, page, 4, &[]).await.unwrap();
        let suspended = reconcile(&tx, scope, page, 5, &blocks).await.unwrap();
        let status = tx
            .query_one(Statement::from_sql_and_values(
                DbBackend::Postgres,
                "SELECT state FROM flow_block_shares WHERE id=$1",
                vec![share.into()],
            ))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(status.try_get::<String>("", "state").unwrap(), "suspended");
        let restored = logical(&tx, page).await;
        assert_eq!(original.0, restored.0);
        assert_eq!(restored.1, original.1 + 1);
        assert_eq!(suspended, vec![original.0]);
        tx.execute(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "UPDATE flow_block_placements SET state='moved',retired_at=now() WHERE page_id=$1 AND state='active'",
            vec![page.into()],
        ))
        .await
        .unwrap();
        reconcile(&tx, scope, page, 6, &blocks).await.unwrap();
        let after_move_restore = logical(&tx, page).await;
        assert_ne!(after_move_restore.0, original.0);
        assert!(reconcile(&tx, scope, page, 5, &blocks).await.is_err());
        tx.rollback().await.unwrap();
        let row = db
            .query_one(Statement::from_string(
                DbBackend::Postgres,
                "SELECT count(*) AS n FROM flow_logical_blocks",
            ))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(row.try_get::<i64>("", "n").unwrap(), 0);
        scratch.drop_self().await;
    }

    use super::fixtures::{block, blocks, page, register_reader_leases};
    use sea_orm::DatabaseConnection;
    use std::collections::HashMap;
    use std::time::{Duration, Instant};

    /// Verbatim copy of the per-block implementation that `reconcile` replaced. Only the one-time
    /// legacy seal at its end is the current `sync_legacy`; that function has its own reference.
    #[derive(FromQueryResult)]
    struct ReferencePlacement {
        id: Uuid,
        logical_id: Uuid,
        generation: i64,
        state: String,
    }
    async fn reconcile_reference(
        tx: &DatabaseTransaction,
        scope_id: Uuid,
        page_id: Uuid,
        accepted_head: i64,
        blocks: &[PreparedBlock],
    ) -> Result<Vec<Uuid>, ApiError> {
        let mut seen = BTreeSet::new();
        if accepted_head < 0
            || blocks.iter().any(|b| {
                b.physical_cid.is_empty() || b.physical_cid.len() > 256 || !seen.insert(b.physical_cid.as_str())
            })
        {
            return Err(ApiError::invalid_update("invalid logical block reconciliation input"));
        }
        // The checkpoint lock additionally serializes initial registration for existing Pages.
        tx.execute(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "INSERT INTO flow_block_checkpoints(scope_id,page_id,accepted_head) VALUES($1,$2,0) ON CONFLICT DO NOTHING",
            vec![scope_id.into(), page_id.into()],
        ))
        .await?;
        let checkpoint = tx
            .query_one(Statement::from_sql_and_values(
                DbBackend::Postgres,
                "SELECT accepted_head FROM flow_block_checkpoints WHERE scope_id=$1 AND page_id=$2 FOR UPDATE",
                vec![scope_id.into(), page_id.into()],
            ))
            .await?
            .ok_or_else(|| ApiError::invalid_update("missing block checkpoint"))?;
        if checkpoint.try_get::<i64>("", "accepted_head")? > accepted_head {
            return Err(ApiError::invalid_update("stale block reconciliation head"));
        }
        let mut suspend = Vec::new();
        for block in blocks {
            let old = ReferencePlacement::find_by_statement(Statement::from_sql_and_values(DbBackend::Postgres,
                "SELECT id,logical_id,generation,state FROM flow_block_placements WHERE scope_id=$1 AND page_id=$2 AND physical_cid=$3 ORDER BY alias_order DESC LIMIT 1 FOR UPDATE",
                vec![scope_id.into(),page_id.into(),block.physical_cid.clone().into()])).one(tx).await?;
            let alias = old.as_ref().map(|old| Alias {
                logical_id: old.logical_id,
                generation: old.generation.unsigned_abs(),
                state: match old.state.as_str() {
                    "active" => AliasState::Active,
                    "deleted" => AliasState::Deleted,
                    _ => AliasState::Moved,
                },
            });
            let decision = reconcile_alias(alias, Uuid::new_v4())
                .map_err(|_| ApiError::invalid_update("logical placement generation conflict"))?;
            let generation = i64::try_from(decision.generation)
                .map_err(|_| ApiError::invalid_update("logical placement generation exhausted"))?;
            if decision.fresh_identity {
                tx.execute(Statement::from_sql_and_values(
                    DbBackend::Postgres,
                    "INSERT INTO flow_logical_blocks(id,state) VALUES($1,'active')",
                    vec![decision.logical_id.into()],
                ))
                .await?;
            }
            tx.execute(Statement::from_sql_and_values(DbBackend::Postgres,"INSERT INTO flow_block_alias_fences(scope_id,page_id,physical_cid,first_logical_id,ever_retired) VALUES($1,$2,$3,$4,$5) ON CONFLICT(scope_id,page_id,physical_cid) DO UPDATE SET ever_retired=flow_block_alias_fences.ever_retired OR EXCLUDED.ever_retired OR flow_block_alias_fences.first_logical_id<>EXCLUDED.first_logical_id",vec![scope_id.into(),page_id.into(),block.physical_cid.clone().into(),decision.logical_id.into(),alias.is_some_and(|alias|alias.state!=AliasState::Active).into()])).await?;
            if let Some(old) = old.filter(|row| row.state == "active") {
                tx.execute(Statement::from_sql_and_values(
                    DbBackend::Postgres,
                    "UPDATE flow_block_placements SET content_hash=$2 WHERE id=$1",
                    vec![old.id.into(), block.content_hash.to_vec().into()],
                ))
                .await?;
            } else {
                tx.execute(Statement::from_sql_and_values(
                    DbBackend::Postgres,
                    "UPDATE flow_logical_blocks SET state='active' WHERE id=$1",
                    vec![decision.logical_id.into()],
                ))
                .await?;
                tx.execute(Statement::from_sql_and_values(DbBackend::Postgres,
                    "INSERT INTO flow_block_placements(id,logical_id,scope_id,page_id,physical_cid,generation,state,content_hash) VALUES($1,$2,$3,$4,$5,$6,'active',$7)",
                    vec![Uuid::new_v4().into(),decision.logical_id.into(),scope_id.into(),page_id.into(),block.physical_cid.clone().into(),generation.into(),block.content_hash.to_vec().into()])).await?;
            }
            if decision.suspend_shares {
                suspend.push(decision.logical_id);
            }
        }
        let cids = blocks.iter().map(|b| b.physical_cid.clone()).collect::<Vec<_>>();
        let removed = tx.query_all(Statement::from_sql_and_values(DbBackend::Postgres,
            "UPDATE flow_block_placements SET state='deleted',retired_at=now() WHERE scope_id=$1 AND page_id=$2 AND state='active' AND NOT(physical_cid=ANY($3::text[])) RETURNING logical_id",
            vec![scope_id.into(),page_id.into(),cids.into()])).await?;
        for row in removed {
            let id = row.try_get::<Uuid>("", "logical_id")?;
            tx.execute(Statement::from_sql_and_values(
                DbBackend::Postgres,
                "UPDATE flow_logical_blocks SET state='deleted' WHERE id=$1",
                vec![id.into()],
            ))
            .await?;
            suspend.push(id);
        }
        tx.execute(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "UPDATE flow_block_checkpoints SET accepted_head=$3 WHERE scope_id=$1 AND page_id=$2",
            vec![scope_id.into(), page_id.into(), accepted_head.into()],
        ))
        .await?;
        suspend.sort_unstable();
        suspend.dedup();
        if !suspend.is_empty() {
            tx.execute(Statement::from_sql_and_values(DbBackend::Postgres,
                "UPDATE flow_block_shares SET state='suspended',auth_revision=auth_revision+1 WHERE state='active' AND id IN (SELECT share_id FROM flow_block_share_members WHERE logical_id=ANY($1::uuid[]))",
                vec![suspend.clone().into()])).await?;
        }
        // Project only fixed member relationships and relative ordering, never absolute source position.
        let shares=tx.query_all(Statement::from_sql_and_values(DbBackend::Postgres,
            "SELECT id,projection_hash FROM flow_block_shares WHERE source_scope_id=$1 AND source_page_id=$2 AND state<>'revoked' ORDER BY id FOR UPDATE",
            vec![scope_id.into(),page_id.into()])).await?;
        for share in shares {
            let share_id = share.try_get::<Uuid>("", "id")?;
            let rows=tx.query_all(Statement::from_sql_and_values(DbBackend::Postgres,
                "SELECT m.member_id,p.physical_cid FROM flow_block_share_members m JOIN flow_block_placements p ON p.logical_id=m.logical_id AND p.state='active' WHERE m.share_id=$1 AND p.scope_id=$2 AND p.page_id=$3",
                vec![share_id.into(),scope_id.into(),page_id.into()])).await?;
            let mut members = Vec::new();
            for row in rows {
                members.push((
                    row.try_get::<Uuid>("", "member_id")?,
                    row.try_get::<String>("", "physical_cid")?,
                ));
            }
            for (member, cid) in &members {
                if let Some(asset) = blocks
                    .iter()
                    .find(|block| block.physical_cid == *cid)
                    .and_then(|block| block.asset_id)
                {
                    tx.execute(Statement::from_sql_and_values(DbBackend::Postgres,"INSERT INTO flow_block_share_asset_aliases(share_id,member_id,alias_id,source_scope_id,source_page_id,asset_id) VALUES($1,$2,$3,$4,$5,$6) ON CONFLICT(share_id,member_id,asset_id) DO NOTHING",vec![share_id.into(),(*member).into(),Uuid::new_v4().into(),scope_id.into(),page_id.into(),asset.into()])).await?;
                }
            }
            let hash = visible_graph_hash(blocks, &members)?;
            if share.try_get::<Vec<u8>>("", "projection_hash")? != hash {
                tx.execute(Statement::from_sql_and_values(
                    DbBackend::Postgres,
                    "UPDATE flow_block_shares SET projection_hash=$2,content_revision=content_revision+1 WHERE id=$1",
                    vec![share_id.into(), hash.to_vec().into()],
                ))
                .await?;
            }
        }
        crate::flow::block_anchors::sync_legacy(tx, page_id, blocks).await?;
        Ok(suspend)
    }

    /// Masks the random Loro peer id of a CID, e.g. `cid:2@123:Map` becomes `cid:2@peer:Map`. Creating
    /// a Page registers its initial editor block under such a CID.
    fn mask_peer(cid: &str) -> String {
        match cid.split_once('@') {
            Some((head, tail)) => format!("{head}@peer{}", tail.find(':').map_or("", |at| &tail[at..])),
            None => cid.to_string(),
        }
    }

    /// Normalised view of everything a reconciliation writes for one Page. Random UUIDs are replaced
    /// by the index of their first appearance, so two Pages that went through the same history compare equal.
    async fn dump(tx: &DatabaseTransaction, page: Uuid) -> (Vec<String>, HashMap<Uuid, String>) {
        let placements = tx
            .query_all(Statement::from_sql_and_values(
                DbBackend::Postgres,
                "SELECT physical_cid,logical_id,generation,state,encode(content_hash,'hex') AS hash,(retired_at IS NOT NULL) AS retired FROM flow_block_placements WHERE page_id=$1 ORDER BY physical_cid,alias_order",
                vec![page.into()],
            ))
            .await
            .unwrap();
        let mut index = HashMap::<Uuid, usize>::new();
        let mut cid_of = HashMap::<Uuid, String>::new();
        let mut lines = Vec::new();
        for row in &placements {
            let logical: Uuid = row.try_get("", "logical_id").unwrap();
            let next = index.len();
            let id = *index.entry(logical).or_insert(next);
            let cid = mask_peer(&row.try_get::<String>("", "physical_cid").unwrap());
            cid_of.insert(logical, cid.clone());
            lines.push(format!(
                "placement {cid} logical#{id} gen={} {} {} retired={}",
                row.try_get::<i64>("", "generation").unwrap(),
                row.try_get::<String>("", "state").unwrap(),
                row.try_get::<String>("", "hash").unwrap(),
                row.try_get::<bool>("", "retired").unwrap(),
            ));
        }
        for row in tx
            .query_all(Statement::from_sql_and_values(
                DbBackend::Postgres,
                "SELECT physical_cid,first_logical_id,ever_retired FROM flow_block_alias_fences WHERE page_id=$1 ORDER BY physical_cid",
                vec![page.into()],
            ))
            .await
            .unwrap()
        {
            let first: Uuid = row.try_get("", "first_logical_id").unwrap();
            lines.push(format!(
                "fence {} first=logical#{:?} retired={}",
                mask_peer(&row.try_get::<String>("", "physical_cid").unwrap()),
                index.get(&first),
                row.try_get::<bool>("", "ever_retired").unwrap(),
            ));
        }
        let mut ordered = index.iter().collect::<Vec<_>>();
        ordered.sort_by_key(|(_, id)| **id);
        for (logical, id) in ordered {
            let state: String = tx
                .query_one(Statement::from_sql_and_values(
                    DbBackend::Postgres,
                    "SELECT state FROM flow_logical_blocks WHERE id=$1",
                    vec![(*logical).into()],
                ))
                .await
                .unwrap()
                .unwrap()
                .try_get("", "state")
                .unwrap();
            lines.push(format!("logical#{id} {state}"));
        }
        let checkpoint = tx
            .query_one(Statement::from_sql_and_values(
                DbBackend::Postgres,
                "SELECT accepted_head,legacy_anchors_sealed FROM flow_block_checkpoints WHERE page_id=$1",
                vec![page.into()],
            ))
            .await
            .unwrap()
            .unwrap();
        lines.push(format!(
            "checkpoint head={} sealed={}",
            checkpoint.try_get::<i64>("", "accepted_head").unwrap(),
            checkpoint.try_get::<bool>("", "legacy_anchors_sealed").unwrap(),
        ));
        for row in tx
            .query_all(Statement::from_sql_and_values(
                DbBackend::Postgres,
                "SELECT state,auth_revision,content_revision,encode(projection_hash,'hex') AS hash FROM flow_block_shares WHERE source_page_id=$1 ORDER BY source_root_cid",
                vec![page.into()],
            ))
            .await
            .unwrap()
        {
            lines.push(format!(
                "share {} auth={} content={} {}",
                row.try_get::<String>("", "state").unwrap(),
                row.try_get::<i64>("", "auth_revision").unwrap(),
                row.try_get::<i64>("", "content_revision").unwrap(),
                row.try_get::<String>("", "hash").unwrap(),
            ));
        }
        for row in tx
            .query_all(Statement::from_sql_and_values(
                DbBackend::Postgres,
                "SELECT original_block_id,state,origin_generation,logical_id FROM flow_block_comment_anchors WHERE source_page_id=$1 ORDER BY original_block_id",
                vec![page.into()],
            ))
            .await
            .unwrap()
        {
            let logical: Option<Uuid> = row.try_get("", "logical_id").unwrap();
            lines.push(format!(
                "anchor {} {} gen={:?} logical#{:?}",
                row.try_get::<String>("", "original_block_id").unwrap(),
                row.try_get::<String>("", "state").unwrap(),
                row.try_get::<Option<i64>>("", "origin_generation").unwrap(),
                logical.and_then(|id| index.get(&id)),
            ));
        }
        (lines, cid_of)
    }

    /// Suspended logical identities of one reconciliation, as sorted physical CIDs.
    fn suspended_cids(suspend: &[Uuid], cid_of: &HashMap<Uuid, String>) -> Vec<String> {
        let mut cids = suspend
            .iter()
            .map(|id| cid_of.get(id).cloned().unwrap_or_else(|| "<unknown>".into()))
            .collect::<Vec<_>>();
        cids.sort();
        cids
    }

    /// Run the new and the reference implementation on two Pages and require identical results.
    async fn apply_both(
        tx: &DatabaseTransaction,
        (scope_a, page_a): (Uuid, Uuid),
        (scope_b, page_b): (Uuid, Uuid),
        head: i64,
        blocks: &[PreparedBlock],
    ) {
        let new = reconcile(tx, scope_a, page_a, head, blocks).await.unwrap();
        let old = reconcile_reference(tx, scope_b, page_b, head, blocks).await.unwrap();
        let (new_lines, new_cids) = dump(tx, page_a).await;
        let (old_lines, old_cids) = dump(tx, page_b).await;
        assert_eq!(new_lines, old_lines, "registry rows diverge at head {head}");
        assert_eq!(
            suspended_cids(&new, &new_cids),
            suspended_cids(&old, &old_cids),
            "suspended identities diverge at head {head}"
        );
    }

    async fn add_share(
        tx: &DatabaseTransaction,
        (actor, scope, page): (Uuid, Uuid, Uuid),
        blocks: &[PreparedBlock],
        cid: &str,
    ) {
        let logical: Uuid = tx
            .query_one(Statement::from_sql_and_values(
                DbBackend::Postgres,
                "SELECT logical_id FROM flow_block_placements WHERE page_id=$1 AND physical_cid=$2 AND state='active'",
                vec![page.into(), cid.into()],
            ))
            .await
            .unwrap()
            .unwrap()
            .try_get("", "logical_id")
            .unwrap();
        let share = Uuid::new_v4();
        let member = Uuid::from_u128(77);
        tx.execute(Statement::from_sql_and_values(DbBackend::Postgres,"INSERT INTO flow_block_shares(id,source_scope_id,source_page_id,created_by,publisher_user_id,state,source_root_cid,selection,root_logical_id) VALUES($1,$2,$3,$4,$4,'active',$5,'block',$6)",vec![share.into(),scope.into(),page.into(),actor.into(),cid.into(),logical.into()])).await.unwrap();
        tx.execute(Statement::from_sql_and_values(DbBackend::Postgres,"INSERT INTO flow_block_share_members(share_id,member_id,logical_id,member_order,content_hash) VALUES($1,$2,$3,0,$4)",vec![share.into(),member.into(),logical.into(),vec![7_u8;32].into()])).await.unwrap();
        let hash = visible_graph_hash(blocks, &[(member, cid.to_string())]).unwrap();
        tx.execute(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "UPDATE flow_block_shares SET projection_hash=$2 WHERE id=$1",
            vec![share.into(), hash.to_vec().into()],
        ))
        .await
        .unwrap();
    }

    async fn scratch_with_leases(label: &str) -> Option<crate::routes::context::tenant_fixture::Scratch> {
        let scratch = crate::routes::context::tenant_fixture::scratch(label).await?;
        register_reader_leases(&scratch.db).await;
        Some(scratch)
    }

    #[tokio::test]
    async fn bulk_reconcile_matches_reference_on_mixed_history() {
        let Some(scratch) = scratch_with_leases(&format!("block_registry_eq_{}", Uuid::new_v4().simple())).await else {
            eprintln!("SKIPPED (no database)");
            return;
        };
        let db = &scratch.db;
        let (actor_a, scope_a, page_a) = page(db).await;
        let (actor_b, scope_b, page_b) = page(db).await;
        let (a, b) = ((scope_a, page_a), (scope_b, page_b));
        let tx = db.begin().await.unwrap();
        // 1. Three fresh blocks.
        let step1 = vec![block(0, 1), block(1, 1), block(2, 1)];
        apply_both(&tx, a, b, 0, &step1).await;
        // 2. Change one block, add two, remove one.
        let step2 = vec![block(0, 1), block(1, 2), block(3, 1), block(4, 1)];
        apply_both(&tx, a, b, 1, &step2).await;
        // 3. Restore the removed CID: deleted alias, generation + 1, suspension.
        let step3 = vec![block(0, 1), block(1, 2), block(2, 1), block(3, 1), block(4, 1)];
        apply_both(&tx, a, b, 2, &step3).await;
        // 4. A moved placement reconciled again takes a fresh identity.
        for page in [page_a, page_b] {
            tx.execute(Statement::from_sql_and_values(
                DbBackend::Postgres,
                "UPDATE flow_block_placements SET state='moved',retired_at=now() WHERE page_id=$1 AND physical_cid=$2 AND state='active'",
                vec![page.into(), block(0, 1).physical_cid.into()],
            ))
            .await
            .unwrap();
        }
        apply_both(&tx, a, b, 3, &step3).await;
        // 5. Shares with members, then a content change and a member removal.
        for (actor, scope, page) in [(actor_a, scope_a, page_a), (actor_b, scope_b, page_b)] {
            add_share(&tx, (actor, scope, page), &step3, &block(1, 2).physical_cid).await;
            add_share(&tx, (actor, scope, page), &step3, &block(3, 1).physical_cid).await;
        }
        apply_both(&tx, a, b, 4, &step3).await;
        let step5 = vec![block(0, 1), block(1, 3), block(2, 1), block(3, 1), block(4, 1)];
        apply_both(&tx, a, b, 5, &step5).await;
        let step6 = vec![block(0, 1), block(1, 3), block(2, 1), block(4, 1)];
        apply_both(&tx, a, b, 6, &step6).await;
        let (lines, _) = dump(&tx, page_a).await;
        assert!(
            lines.iter().any(|line| line.starts_with("share suspended")),
            "the member removal must suspend its share: {lines:?}"
        );
        // 7. Everything removed, then everything restored.
        apply_both(&tx, a, b, 7, &[]).await;
        apply_both(&tx, a, b, 8, &step3).await;
        tx.rollback().await.unwrap();
        scratch.drop_self().await;
    }

    async fn counted(tx: &DatabaseTransaction, scope: Uuid, page: Uuid, head: i64, blocks: &[PreparedBlock]) -> usize {
        let (result, count) = statement_probe::count(reconcile(tx, scope, page, head, blocks)).await;
        result.unwrap();
        count
    }

    #[tokio::test]
    async fn reconcile_statement_count_is_independent_of_block_count() {
        let Some(scratch) = scratch_with_leases(&format!("block_registry_count_{}", Uuid::new_v4().simple())).await
        else {
            eprintln!("SKIPPED (no database)");
            return;
        };
        let db = &scratch.db;
        let mut first = Vec::new();
        let mut steady = Vec::new();
        for size in [50, 2_000] {
            let (_, scope, page) = page(db).await;
            let tx = db.begin().await.unwrap();
            let initial = blocks(size, 1);
            first.push(counted(&tx, scope, page, 0, &initial).await);
            let mut changed = initial.clone();
            changed[0] = block(0, 2);
            steady.push(counted(&tx, scope, page, 1, &changed).await);
            tx.rollback().await.unwrap();
        }
        eprintln!("reconcile statements: first registration {first:?}, steady state {steady:?}");
        assert_eq!(first[0], first[1], "first registration must not grow with the page");
        assert_eq!(steady[0], steady[1], "steady state must not grow with the page");
        assert!(
            first.iter().chain(&steady).all(|count| *count <= 20),
            "{first:?} {steady:?}"
        );
        scratch.drop_self().await;
    }

    async fn row_versions(tx: &DatabaseTransaction, page: Uuid) -> Vec<String> {
        tx.query_all(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT 'p:'||id::text||':'||xmin::text AS v FROM flow_block_placements WHERE page_id=$1 UNION ALL SELECT 'f:'||physical_cid||':'||xmin::text FROM flow_block_alias_fences WHERE page_id=$1 ORDER BY 1",
            vec![page.into()],
        ))
        .await
        .unwrap()
        .iter()
        .map(|row| row.try_get::<String>("", "v").unwrap())
        .collect()
    }

    #[tokio::test]
    async fn unchanged_reconcile_rewrites_no_rows() {
        let Some(scratch) = scratch_with_leases(&format!("block_registry_xmin_{}", Uuid::new_v4().simple())).await
        else {
            eprintln!("SKIPPED (no database)");
            return;
        };
        let db = &scratch.db;
        let (_, scope, page) = page(db).await;
        let initial = blocks(500, 1);
        let tx = db.begin().await.unwrap();
        reconcile(&tx, scope, page, 0, &initial).await.unwrap();
        tx.commit().await.unwrap();
        // A separate transaction gets a new xid, so any rewritten row would show a new xmin.
        let tx = db.begin().await.unwrap();
        let before = row_versions(&tx, page).await;
        // 500 placements and 500 fences, plus the Page's own initial block (removed at the first head).
        assert_eq!(before.len(), 1_002);
        reconcile(&tx, scope, page, 1, &initial).await.unwrap();
        assert_eq!(
            before,
            row_versions(&tx, page).await,
            "identical content must rewrite no rows"
        );
        let mut changed = initial.clone();
        changed[7] = block(7, 2);
        reconcile(&tx, scope, page, 2, &changed).await.unwrap();
        let after = row_versions(&tx, page).await;
        let rewritten = before.iter().zip(&after).filter(|(old, new)| old != new).count();
        assert_eq!(rewritten, 1, "a single changed block must rewrite exactly one row");
        tx.rollback().await.unwrap();
        scratch.drop_self().await;
    }

    /// Mirrors the production per-statement caps (100 ms statement, 80 ms lock) of the locked phase.
    async fn begin_capped(db: &DatabaseConnection) -> DatabaseTransaction {
        let tx = db.begin().await.unwrap();
        crate::flow::collab::write::set_locked_phase_statement_budgets(&tx, 1)
            .await
            .unwrap();
        tx
    }

    /// The locked-phase staging budget of `collab::write` (4 x the document lock hold limit).
    const STAGING_BUDGET: Duration = Duration::from_millis(400);

    #[tokio::test]
    async fn reconcile_10k_blocks_within_statement_and_staging_budgets() {
        let Some(scratch) = scratch_with_leases(&format!("block_registry_10k_{}", Uuid::new_v4().simple())).await
        else {
            eprintln!("SKIPPED (no database)");
            return;
        };
        let db = &scratch.db;
        let (_, scope, page_id) = page(db).await;
        let initial = blocks(10_000, 1);
        let tx = begin_capped(db).await;
        let started = Instant::now();
        reconcile(&tx, scope, page_id, 0, &initial).await.unwrap();
        let first = started.elapsed();
        tx.commit().await.unwrap();
        let mut changed = initial.clone();
        changed[5_000] = block(5_000, 2);
        let tx = begin_capped(db).await;
        let started = Instant::now();
        reconcile(&tx, scope, page_id, 1, &changed).await.unwrap();
        let steady = started.elapsed();
        tx.commit().await.unwrap();
        // Regression evidence: the removed per-block implementation on a 1,550-block page.
        let (_, reference_scope, reference_page) = page(db).await;
        let reference_blocks = blocks(1_550, 1);
        let tx = db.begin().await.unwrap();
        reconcile_reference(&tx, reference_scope, reference_page, 0, &reference_blocks)
            .await
            .unwrap();
        tx.commit().await.unwrap();
        let mut reference_changed = reference_blocks.clone();
        reference_changed[10] = block(10, 2);
        let tx = db.begin().await.unwrap();
        let started = Instant::now();
        reconcile_reference(&tx, reference_scope, reference_page, 1, &reference_changed)
            .await
            .unwrap();
        let reference = started.elapsed();
        tx.commit().await.unwrap();
        eprintln!(
            "reconcile timings: bulk first registration (10000) {first:?}, bulk steady state (10000) {steady:?}, reference steady state (1550) {reference:?}"
        );
        assert!(first < STAGING_BUDGET, "first registration took {first:?}");
        assert!(steady < Duration::from_millis(300), "steady state took {steady:?}");
        if std::env::var("OPENPR_FLOW_QUIET_PG_QUALIFIED").is_ok_and(|value| value == "1") {
            assert!(steady < Duration::from_millis(100), "steady state took {steady:?}");
        }
        scratch.drop_self().await;
    }

    /// A large Page written before the registry was active is sealed by its first write afterwards:
    /// it must fit the locked-phase budgets and register exactly what the old loop registered,
    /// including the one-time adoption of legacy comment identifiers.
    #[tokio::test]
    async fn large_legacy_page_first_write_registers_like_the_old_loop() {
        let Some(scratch) = scratch_with_leases(&format!("block_registry_upgrade_{}", Uuid::new_v4().simple())).await
        else {
            eprintln!("SKIPPED (no database)");
            return;
        };
        let db = &scratch.db;
        db.execute_unprepared("UPDATE flow_runtime_capabilities SET minimum_reader_version=4 WHERE singleton")
            .await
            .unwrap();
        let (actor_a, scope_a, page_a) = page(db).await;
        let (actor_b, scope_b, page_b) = page(db).await;
        let mut legacy = blocks(5_000, 1);
        for (index, block) in legacy.iter_mut().enumerate() {
            block.legacy_alias = Some(format!("legacy-{index}"));
        }
        for (actor, scope, page) in [(actor_a, scope_a, page_a), (actor_b, scope_b, page_b)] {
            for index in (0..5_000).step_by(25) {
                let target = if index % 50 == 0 {
                    format!("legacy-{index}")
                } else {
                    format!("cid:{index}:Map")
                };
                exec(db,"INSERT INTO flow_comments(id,scope_id,object_id,block_id,author_id,body,created_at) VALUES($1,$2,$3,$4,$5,'Old private note',now()-interval '1 day')",vec![Uuid::new_v4().into(),scope.into(),page.into(),target.into(),actor.into()]).await;
            }
        }
        db.execute_unprepared("UPDATE flow_runtime_capabilities SET minimum_reader_version=5 WHERE singleton")
            .await
            .unwrap();
        let tx = begin_capped(db).await;
        let first_write = tokio::time::timeout(STAGING_BUDGET, reconcile_accepted(&tx, page_a, 0, &legacy)).await;
        first_write
            .expect("first write after the floor is raised must fit the staging budget")
            .unwrap();
        tx.commit().await.unwrap();
        let tx = db.begin().await.unwrap();
        reconcile_reference(&tx, scope_b, page_b, 0, &legacy).await.unwrap();
        let (new_lines, _) = dump(&tx, page_a).await;
        let (old_lines, _) = dump(&tx, page_b).await;
        assert!(
            new_lines.len() > 15_000,
            "the page must be registered: {}",
            new_lines.len()
        );
        assert!(
            new_lines
                .iter()
                .filter(|line| line.starts_with("anchor ") && line.contains(" known "))
                .count()
                >= 100,
            "legacy comments must be adopted"
        );
        assert_eq!(new_lines, old_lines);
        tx.rollback().await.unwrap();
        scratch.drop_self().await;
    }
}
