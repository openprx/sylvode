//! Private Page identity registry. No recipient DTO or authorization is derived from these rows.
//! Caller must hold the accepted Page-head lock and use its existing `DatabaseTransaction`.
use crate::error::ApiError;
use collab_core::block_identity::{Alias, AliasState, reconcile_alias};
use sea_orm::{ConnectionTrait, DatabaseTransaction, DbBackend, FromQueryResult, Statement};
use std::collections::BTreeSet;
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

#[derive(FromQueryResult)]
struct Placement {
    id: Uuid,
    logical_id: Uuid,
    generation: i64,
    state: String,
}
/// Reconcile one accepted head. No independent commit, connection or raw client identity input.
///
/// Multi-Page operations must lock all Page heads in UUID order before calling this function.
/// Affected shares are suspended in this transaction; returned IDs are private audit metadata.
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
        let old = Placement::find_by_statement(Statement::from_sql_and_values(DbBackend::Postgres,
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

#[cfg(test)]
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
}
