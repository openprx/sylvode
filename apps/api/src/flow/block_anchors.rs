//! Private comments never move between Pages. A CID is never used to rebind a known anchor.
use super::collab::bootstrap;
use crate::error::ApiError;
use sea_orm::{ConnectionTrait, DatabaseConnection, DatabaseTransaction, DbBackend, Statement};
use uuid::Uuid;

pub(crate) struct Prepared {
    document: Uuid,
    head_seq: i64,
    frontier: Vec<u8>,
    blocks: Vec<super::block_registry::PreparedBlock>,
}
pub(crate) async fn prepare(
    db: &DatabaseConnection,
    object: Uuid,
    anchored: bool,
    reply: bool,
) -> Result<Option<Prepared>, ApiError> {
    if !anchored && !reply {
        return Ok(None);
    }
    if super::scope::capabilities(db).await?.minimum_reader_version < 5
        || super::scope::identity(db, object).await?.object_type != "page"
    {
        return Ok(None);
    }
    let document = db
        .query_one(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT id FROM collab_documents WHERE object_id=$1",
            vec![object.into()],
        ))
        .await?
        .ok_or_else(ApiError::internal)?
        .try_get::<Uuid>("", "id")?;
    let boot = bootstrap::load(db, document).await?;
    let engine = super::document::engine_from_boot(&boot)?;
    if !engine.matches_frontier(&boot.head_frontier) {
        return Err(ApiError::resync_required("comment anchor head did not replay", None));
    }
    if !engine.has_editor_doc() {
        if anchored && !reply {
            return Err(ApiError::invalid_update(
                "initialize Page editor before creating a block anchor",
            ));
        }
        return Ok(None);
    }
    Ok(Some(Prepared {
        document,
        head_seq: boot.head_seq,
        frontier: boot.head_frontier,
        blocks: super::block_registry::prepare(
            &engine
                .semantic_snapshot()
                .map_err(|error| super::command::map_collab_error(&error))?,
        )?,
    }))
}
pub(crate) async fn ensure(
    tx: &DatabaseTransaction,
    object: Uuid,
    prepared: Option<&Prepared>,
) -> Result<(), ApiError> {
    let Some(prepared) = prepared else {
        return Ok(());
    };
    let head = tx
        .query_one(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT head_seq,head_frontier FROM collab_documents WHERE id=$1 FOR UPDATE",
            vec![prepared.document.into()],
        ))
        .await?
        .ok_or_else(ApiError::internal)?;
    if head.try_get::<i64>("", "head_seq")? != prepared.head_seq
        || !super::frontier::same(&head.try_get::<Vec<u8>>("", "head_frontier")?, &prepared.frontier)?
    {
        return Err(ApiError::server_draining(
            crate::error::ServerDrainingReason::Contention,
            200,
            "comment anchor busy; retry",
        ));
    }
    super::block_registry::reconcile_accepted(tx, object, prepared.head_seq, &prepared.blocks).await
}

/// Adopt the current legacy Page+identifier meaning exactly once under its accepted-head lock.
/// This defines the first UUID baseline; it never claims prior UUID history or runs after seal.
pub(crate) async fn sync_legacy(
    tx: &DatabaseTransaction,
    page: Uuid,
    blocks: &[super::block_registry::PreparedBlock],
) -> Result<(), ApiError> {
    #[cfg(test)]
    super::block_registry::statement_probe::note();
    let checkpoint = tx
        .query_one(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT accepted_head,legacy_anchors_sealed FROM flow_block_checkpoints WHERE page_id=$1 FOR UPDATE",
            vec![page.into()],
        ))
        .await?
        .ok_or_else(ApiError::internal)?;
    if !checkpoint.try_get::<bool>("", "legacy_anchors_sealed")? {
        #[cfg(test)]
        super::block_registry::statement_probe::note();
        let head = tx
            .query_one(Statement::from_sql_and_values(
                DbBackend::Postgres,
                "SELECT head_seq FROM collab_documents WHERE object_id=$1 FOR UPDATE",
                vec![page.into()],
            ))
            .await?
            .ok_or_else(ApiError::internal)?;
        if head.try_get::<i64>("", "head_seq")? != checkpoint.try_get::<i64>("", "accepted_head")? {
            return Err(ApiError::policy_rejected("legacy anchor baseline head changed"));
        }
        let mut aliases = std::collections::BTreeMap::<String, Vec<&super::block_registry::PreparedBlock>>::new();
        for block in blocks {
            aliases.entry(block.physical_cid.clone()).or_default().push(block);
            if let Some(alias) = &block.legacy_alias
                && alias != &block.physical_cid
            {
                aliases.entry(alias.clone()).or_default().push(block);
            }
        }
        let mut alias_names = Vec::new();
        let mut alias_cids = Vec::new();
        for (alias, candidates) in aliases {
            if let [block] = candidates.as_slice() {
                alias_names.push(alias);
                alias_cids.push(block.physical_cid.clone());
            }
        }
        if !alias_names.is_empty() {
            #[cfg(test)]
            super::block_registry::statement_probe::note();
            tx.execute(Statement::from_sql_and_values(DbBackend::Postgres,r"INSERT INTO flow_block_comment_anchors(comment_id,source_page_id,original_block_id,logical_id,origin_placement_id,origin_generation,state)
SELECT DISTINCT ON (c.id) c.id,c.object_id,c.block_id,p.logical_id,p.id,p.generation,'known'
FROM unnest($2::text[],$3::text[]) AS a(alias,cid)
JOIN flow_comments c ON c.object_id=$1 AND c.block_id=a.alias
JOIN flow_block_placements p ON p.page_id=c.object_id AND p.physical_cid=a.cid AND p.state='active'
JOIN flow_block_alias_fences f ON f.page_id=p.page_id AND f.scope_id=p.scope_id AND f.physical_cid=p.physical_cid AND f.first_logical_id=p.logical_id AND NOT f.ever_retired
WHERE NOT EXISTS(SELECT 1 FROM flow_block_alias_fences retired WHERE retired.page_id=$1 AND retired.physical_cid=a.alias AND retired.ever_retired)
ORDER BY c.id,p.alias_order DESC
ON CONFLICT(comment_id) DO UPDATE SET logical_id=EXCLUDED.logical_id,origin_placement_id=EXCLUDED.origin_placement_id,origin_generation=EXCLUDED.origin_generation,state='known' WHERE flow_block_comment_anchors.logical_id IS NULL",vec![page.into(),alias_names.into(),alias_cids.into()])).await?;
        }
        #[cfg(test)]
        super::block_registry::statement_probe::note();
        tx.execute(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "UPDATE flow_block_checkpoints SET legacy_anchors_sealed=true WHERE page_id=$1",
            vec![page.into()],
        ))
        .await?;
    }
    // After sealing, even a currently matching CID cannot nominate a new identity for old rows.
    #[cfg(test)]
    super::block_registry::statement_probe::note();
    tx.execute(Statement::from_sql_and_values(DbBackend::Postgres,"INSERT INTO flow_block_comment_anchors(comment_id,source_page_id,original_block_id,state) SELECT id,object_id,block_id,'orphaned' FROM flow_comments WHERE object_id=$1 AND block_id IS NOT NULL ON CONFLICT(comment_id) DO NOTHING",vec![page.into()])).await?;
    Ok(())
}

/// A private metadata baseline does not alter the Loro document, native structure or history.
pub(crate) async fn baseline_for_read(db: &DatabaseConnection, page: Uuid) -> Result<(), ApiError> {
    use sea_orm::TransactionTrait;
    if super::scope::capabilities(db).await?.minimum_reader_version < 5
        || super::scope::identity(db, page).await?.object_type != "page"
    {
        return Ok(());
    }
    if db
        .query_one(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT page_id FROM flow_block_checkpoints WHERE page_id=$1 AND legacy_anchors_sealed",
            vec![page.into()],
        ))
        .await?
        .is_some()
    {
        return Ok(());
    }
    for _ in 0..3 {
        let document = db
            .query_one(Statement::from_sql_and_values(
                DbBackend::Postgres,
                "SELECT id FROM collab_documents WHERE object_id=$1",
                vec![page.into()],
            ))
            .await?
            .ok_or_else(ApiError::internal)?
            .try_get::<Uuid>("", "id")?;
        let boot = bootstrap::load(db, document).await?;
        let engine = super::document::engine_from_boot(&boot)?;
        if !engine.matches_frontier(&boot.head_frontier) {
            return Err(ApiError::resync_required("comment baseline head did not replay", None));
        }
        let blocks = super::block_registry::prepare(
            &engine
                .semantic_snapshot()
                .map_err(|error| super::command::map_collab_error(&error))?,
        )?;
        let identity = super::scope::identity(db, page).await?;
        let tx = db.begin().await?;
        if let Some(workspace) = identity.workspace_id {
            tx.execute(Statement::from_sql_and_values(
                DbBackend::Postgres,
                "SELECT id FROM workspaces WHERE id=$1 FOR KEY SHARE",
                vec![workspace.into()],
            ))
            .await?;
        }
        tx.execute(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT id FROM flow_scopes WHERE id=$1 FOR KEY SHARE",
            vec![identity.scope_id.into()],
        ))
        .await?;
        tx.execute(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT id FROM flow_objects WHERE id=$1 FOR KEY SHARE",
            vec![page.into()],
        ))
        .await?;
        let head = tx
            .query_one(Statement::from_sql_and_values(
                DbBackend::Postgres,
                "SELECT head_seq,head_frontier FROM collab_documents WHERE id=$1 FOR UPDATE",
                vec![document.into()],
            ))
            .await?
            .ok_or_else(ApiError::internal)?;
        if head.try_get::<i64>("", "head_seq")? != boot.head_seq
            || !super::frontier::same(&head.try_get::<Vec<u8>>("", "head_frontier")?, &boot.head_frontier)?
        {
            tx.rollback().await?;
            continue;
        }
        let scope = super::scope::identity(&tx, page).await?.scope_id;
        super::block_registry::reconcile(&tx, scope, page, boot.head_seq, &blocks).await?;
        tx.commit().await?;
        return Ok(());
    }
    Err(ApiError::server_draining(
        crate::error::ServerDrainingReason::Contention,
        200,
        "comment baseline busy; retry",
    ))
}

pub(crate) async fn bind_new<C: ConnectionTrait>(
    db: &C,
    comment: Uuid,
    page: Uuid,
    block: Option<&str>,
    parent: Option<Uuid>,
) -> Result<(), ApiError> {
    if super::scope::capabilities(db).await?.minimum_reader_version < 5
        || super::scope::identity(db, page).await?.object_type != "page"
    {
        return Ok(());
    }
    let Some(block) = block else {
        return Ok(());
    };
    if let Some(parent) = parent {
        // Replies inherit the exact private original identity, including unresolved legacy state.
        let changed=db.execute(Statement::from_sql_and_values(DbBackend::Postgres,"INSERT INTO flow_block_comment_anchors(comment_id,source_page_id,original_block_id,logical_id,origin_placement_id,origin_generation,state) SELECT $1,source_page_id,original_block_id,logical_id,origin_placement_id,origin_generation,state FROM flow_block_comment_anchors WHERE comment_id=$2 AND source_page_id=$3",vec![comment.into(),parent.into(),page.into()])).await?;
        if changed.rows_affected() == 0 {
            db.execute(Statement::from_sql_and_values(DbBackend::Postgres,"INSERT INTO flow_block_comment_anchors(comment_id,source_page_id,original_block_id,state) VALUES($1,$2,$3,'orphaned')",vec![comment.into(),page.into(),block.to_string().into()])).await?;
        }
        return Ok(());
    }
    let row=db.query_one(Statement::from_sql_and_values(DbBackend::Postgres,"SELECT p.id,p.logical_id,p.generation FROM flow_block_placements p JOIN flow_block_checkpoints c ON c.page_id=p.page_id AND c.scope_id=p.scope_id JOIN collab_documents d ON d.object_id=p.page_id AND d.head_seq=c.accepted_head WHERE p.page_id=$1 AND p.physical_cid=$2 AND p.state='active' FOR SHARE OF p,c,d",vec![page.into(),block.to_string().into()])).await?.ok_or_else(||ApiError::invalid_update("block anchor unavailable"))?;
    db.execute(Statement::from_sql_and_values(DbBackend::Postgres,"INSERT INTO flow_block_comment_anchors(comment_id,source_page_id,original_block_id,logical_id,origin_placement_id,origin_generation,state) VALUES($1,$2,$3,$4,$5,$6,'known')",vec![comment.into(),page.into(),block.to_string().into(),row.try_get::<Uuid>("","logical_id")?.into(),row.try_get::<Uuid>("","id")?.into(),row.try_get::<i64>("","generation")?.into()])).await?;
    Ok(())
}

pub(crate) async fn decorate<C: ConnectionTrait>(
    db: &C,
    views: &mut [super::comments::FlowCommentView],
) -> Result<(), ApiError> {
    if views.is_empty() || super::scope::capabilities(db).await?.minimum_reader_version < 5 {
        return Ok(());
    }
    let Some(first) = views.first() else {
        return Ok(());
    };
    if super::scope::identity(db, first.object_id).await?.object_type != "page" {
        return Ok(());
    }
    for view in views {
        if view.block_id.is_none() {
            continue;
        }
        let row=db.query_one(Statement::from_sql_and_values(DbBackend::Postgres,"SELECT p.physical_cid FROM flow_block_comment_anchors a LEFT JOIN flow_block_placements p ON p.logical_id=a.logical_id AND p.page_id=a.source_page_id AND p.state='active' WHERE a.comment_id=$1 AND a.source_page_id=$2 AND a.state='known'",vec![view.id.into(),view.object_id.into()])).await?;
        let current = row
            .map(|row| row.try_get::<Option<String>>("", "physical_cid"))
            .transpose()?
            .flatten();
        // Missing origin FK never falls back to same-text CID; only verified UUID resolves.
        view.anchor_status = Some(if current.is_some() { "bound" } else { "orphaned" }.into());
        view.block_id = current;
    }
    Ok(())
}

#[cfg(test)]
#[allow(clippy::indexing_slicing, clippy::print_stderr)]
mod tests {
    use super::*;
    use crate::{routes::context::tenant_fixture::exec, scratch_or_skip};
    use collab_core::{CollabEngine, LoroCollabEngine};
    use sea_orm::TransactionTrait;

    fn reverse_two_peer_entries(wire: &[u8]) -> Vec<u8> {
        // A Loro frontier encodes the peer-map length and then peer/counter varint pairs.
        let (&count, payload) = wire.split_first().unwrap();
        assert_eq!(count, 2, "the fixture must have two editing peers");
        let fields = payload.split_inclusive(|byte| byte & 0x80 == 0).collect::<Vec<_>>();
        let [first_peer, first_counter, second_peer, second_counter] = fields.as_slice() else {
            panic!("expected two complete peer/counter pairs");
        };
        [&[count], *second_peer, *second_counter, *first_peer, *first_counter].concat()
    }
    #[tokio::test]
    async fn private_anchors_never_follow_reused_cids_or_cascade_moved_identity() {
        let scratch = scratch_or_skip!(&format!("block_anchor_{}", Uuid::new_v4().simple()));
        let db = &scratch.db;
        for component in ["api", "worker", "maintenance"] {
            super::super::scope_runtime::renew(db, Uuid::new_v4(), component)
                .await
                .unwrap();
        }
        db.execute_unprepared("UPDATE flow_runtime_capabilities SET minimum_reader_version=4 WHERE singleton")
            .await
            .unwrap();
        let owners = [Uuid::new_v4(), Uuid::new_v4()];
        let scopes = [Uuid::new_v4(), Uuid::new_v4()];
        let pages = [Uuid::new_v4(), Uuid::new_v4()];
        let documents = [Uuid::new_v4(), Uuid::new_v4()];
        for (((owner, scope), page), document) in owners.into_iter().zip(scopes).zip(pages).zip(documents) {
            exec(db,"INSERT INTO users(id,email,password_hash,name,role,is_active) VALUES($1,$2,'!','Anchor fixture','user',true)",vec![owner.into(),format!("{owner}@anchor.test").into()]).await;
            exec(
                db,
                "INSERT INTO flow_scopes(id,kind,owner_user_id) VALUES($1,'personal',$2)",
                vec![scope.into(), owner.into()],
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
            super::super::scope::insert_object_with_identity(
                &tx,
                scope,
                owner,
                "page",
                Some(navigator),
                "Anchor Page",
                None,
                page,
                document,
            )
            .await
            .unwrap();
            tx.commit().await.unwrap();
        }
        let mut source = super::super::document::load_head_engine(db, documents[0])
            .await
            .unwrap();
        let cid = source
            .semantic_snapshot()
            .unwrap()
            .nodes
            .keys()
            .next()
            .unwrap()
            .to_string();
        source
            .apply_content_operation(&collab_core::Operation::CreateNode {
                id: "legacy-readable-alias".into(),
                parent: None,
                index: 1,
                kind: collab_core::NodeKind::Block,
            })
            .unwrap();
        let aliased = source
            .resolve_content_node_id(&"legacy-readable-alias".into())
            .to_string();
        let original_snapshot = source.export_snapshot().unwrap();
        let replayed = LoroCollabEngine::load(&original_snapshot).unwrap();
        let replayed_wire = replayed.frontier();
        let original_frontier = reverse_two_peer_entries(replayed_wire.as_bytes());
        assert_ne!(original_frontier, replayed_wire.as_bytes());
        assert!(replayed.matches_frontier(&original_frontier));
        exec(db,"UPDATE collab_documents SET snapshot=$2,snapshot_checksum=encode(digest($2,'sha256'),'hex'),byte_count=octet_length($2),snapshot_frontier=$3,head_frontier=$3 WHERE id=$1",vec![documents[0].into(),original_snapshot.clone().into(),original_frontier.clone().into()]).await;
        let legacy = Uuid::new_v4();
        let legacy_alias = Uuid::new_v4();
        let unresolved = Uuid::new_v4();
        for (id, block) in [
            (legacy, cid.as_str()),
            (legacy_alias, "legacy-readable-alias"),
            (unresolved, "unresolved-old-id"),
        ] {
            exec(db,"INSERT INTO flow_comments(id,scope_id,object_id,block_id,author_id,body,created_at) VALUES($1,$2,$3,$4,$5,'Old private note',now()-interval '1 day')",vec![id.into(),scopes[0].into(),pages[0].into(),block.into(),owners[0].into()]).await;
        }
        db.execute_unprepared("UPDATE flow_runtime_capabilities SET minimum_reader_version=5 WHERE singleton")
            .await
            .unwrap();
        let before = super::super::comments::list_comments(db, pages[0]).await.unwrap();
        assert_eq!(
            before
                .items
                .iter()
                .find(|view| view.id == legacy)
                .unwrap()
                .block_id
                .as_deref(),
            Some(cid.as_str())
        );
        assert_eq!(
            before
                .items
                .iter()
                .find(|view| view.id == legacy_alias)
                .unwrap()
                .block_id
                .as_deref(),
            Some(aliased.as_str())
        );
        assert_eq!(
            before
                .items
                .iter()
                .find(|view| view.id == unresolved)
                .unwrap()
                .anchor_status
                .as_deref(),
            Some("orphaned")
        );
        let unchanged = db
            .query_one(Statement::from_sql_and_values(
                DbBackend::Postgres,
                "SELECT snapshot,head_frontier,head_seq FROM collab_documents WHERE id=$1",
                vec![documents[0].into()],
            ))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(unchanged.try_get::<Vec<u8>>("", "snapshot").unwrap(), original_snapshot);
        assert_eq!(
            unchanged.try_get::<Vec<u8>>("", "head_frontier").unwrap(),
            original_frontier
        );
        assert_eq!(unchanged.try_get::<i64>("", "head_seq").unwrap(), 0);
        let prepared = prepare(db, pages[0], true, false).await.unwrap().unwrap();
        assert_eq!(prepared.frontier, original_frontier);
        let tx = db.begin().await.unwrap();
        ensure(&tx, pages[0], Some(&prepared)).await.unwrap();
        exec(
            &tx,
            "UPDATE collab_documents SET head_seq=head_seq+1 WHERE id=$1",
            vec![documents[0].into()],
        )
        .await;
        let contention = crate::error::ApiErrorKind::ServerDraining(crate::error::ServerDrainingReason::Contention);
        assert_eq!(
            ensure(&tx, pages[0], Some(&prepared)).await.unwrap_err().kind(),
            contention
        );
        tx.rollback().await.unwrap();

        // The same version re-encoded in canonical peer order is not a head change: frontiers
        // are compared as version vectors, so legacy-order rows keep matching.
        let tx = db.begin().await.unwrap();
        exec(
            &tx,
            "UPDATE collab_documents SET head_frontier=$2 WHERE id=$1",
            vec![documents[0].into(), replayed_wire.as_bytes().to_vec().into()],
        )
        .await;
        ensure(&tx, pages[0], Some(&prepared)).await.unwrap();
        tx.rollback().await.unwrap();

        // A different version at the same head_seq is still contention.
        let tx = db.begin().await.unwrap();
        exec(
            &tx,
            "UPDATE collab_documents SET head_frontier=$2 WHERE id=$1",
            vec![documents[0].into(), vec![0_u8].into()],
        )
        .await;
        assert_eq!(
            ensure(&tx, pages[0], Some(&prepared)).await.unwrap_err().kind(),
            contention
        );
        tx.rollback().await.unwrap();
        let sealed = db
            .query_one(Statement::from_sql_and_values(
                DbBackend::Postgres,
                "SELECT legacy_anchors_sealed FROM flow_block_checkpoints WHERE page_id=$1",
                vec![pages[0].into()],
            ))
            .await
            .unwrap()
            .unwrap();
        assert!(sealed.try_get::<bool>("", "legacy_anchors_sealed").unwrap());
        let baseline_identity = db
            .query_one(Statement::from_sql_and_values(
                DbBackend::Postgres,
                "SELECT logical_id FROM flow_block_comment_anchors WHERE comment_id=$1",
                vec![legacy.into()],
            ))
            .await
            .unwrap()
            .unwrap()
            .try_get::<Uuid>("", "logical_id")
            .unwrap();
        db.execute_unprepared(include_str!("../../../../migrations/0095_flow_logical_blocks.sql"))
            .await
            .unwrap();
        let placement=db.query_one(Statement::from_sql_and_values(DbBackend::Postgres,"SELECT id,logical_id FROM flow_block_placements WHERE page_id=$1 AND physical_cid=$2 AND state='active'",vec![pages[0].into(),cid.clone().into()])).await.unwrap().unwrap();
        let origin = placement.try_get::<Uuid>("", "id").unwrap();
        let logical = placement.try_get::<Uuid>("", "logical_id").unwrap();
        assert_eq!(logical, baseline_identity);
        let tx = db.begin().await.unwrap();
        let known = super::super::comments::create_comment_on(
            &tx,
            None,
            Some(scopes[0]),
            pages[0],
            &super::super::comments::CommentActor {
                user_id: owners[0],
                level: super::super::collab::authz::PermissionLevel::FullAccess,
            },
            super::super::comments::NewComment {
                body: "New private anchor".into(),
                block_id: Some(cid.clone()),
                parent_id: None,
                mentions: vec![],
            },
        )
        .await
        .unwrap();
        assert_eq!(known.anchor_status.as_deref(), Some("bound"));
        tx.commit().await.unwrap();
        // A later legacy-shaped row cannot obtain a second baseline, even for an unchanged CID.
        let late = Uuid::new_v4();
        exec(db,"INSERT INTO flow_comments(id,scope_id,object_id,block_id,author_id,body) VALUES($1,$2,$3,$4,$5,'Unproven late legacy note')",vec![late.into(),scopes[0].into(),pages[0].into(),cid.clone().into(),owners[0].into()]).await;
        let blocks = super::super::block_registry::prepare(&source.semantic_snapshot().unwrap()).unwrap();
        let tx = db.begin().await.unwrap();
        super::super::block_registry::reconcile(&tx, scopes[0], pages[0], 0, &blocks)
            .await
            .unwrap();
        tx.commit().await.unwrap();
        assert_eq!(
            super::super::comments::list_comments(db, pages[0])
                .await
                .unwrap()
                .items
                .iter()
                .find(|view| view.id == late)
                .unwrap()
                .anchor_status
                .as_deref(),
            Some("orphaned")
        );
        // Trusted-move state-layer fixture: distinct destination native carrier, no copied comments.
        exec(
            db,
            "UPDATE flow_block_placements SET state='moved',retired_at=now() WHERE id=$1",
            vec![origin.into()],
        )
        .await;
        let mut destination = super::super::document::load_head_engine(db, documents[1])
            .await
            .unwrap();
        destination
            .apply_content_operation(&collab_core::Operation::CreateNode {
                id: "moved-carrier".into(),
                parent: None,
                index: 1,
                kind: collab_core::NodeKind::Block,
            })
            .unwrap();
        let destination_cid = destination.resolve_content_node_id(&"moved-carrier".into()).to_string();
        assert_ne!(destination_cid, cid);
        exec(db,"UPDATE collab_documents SET snapshot=$2,snapshot_checksum=encode(digest($2,'sha256'),'hex'),byte_count=octet_length($2),snapshot_frontier=$3,head_frontier=$3 WHERE id=$1",vec![documents[1].into(),destination.export_snapshot().unwrap().into(),destination.frontier().as_bytes().to_vec().into()]).await;
        exec(db,"INSERT INTO flow_block_placements(id,logical_id,scope_id,page_id,physical_cid,generation,state,content_hash) VALUES($1,$2,$3,$4,$5,2,'active',decode(repeat('00',32),'hex'))",vec![Uuid::new_v4().into(),logical.into(),scopes[1].into(),pages[1].into(),destination_cid.into()]).await;
        let destination_blocks =
            super::super::block_registry::prepare(&destination.semantic_snapshot().unwrap()).unwrap();
        let tx = db.begin().await.unwrap();
        super::super::block_registry::reconcile(&tx, scopes[1], pages[1], 0, &destination_blocks)
            .await
            .unwrap();
        super::super::block_registry::reconcile(&tx, scopes[0], pages[0], 0, &blocks)
            .await
            .unwrap();
        tx.commit().await.unwrap();
        let replacement = db
            .query_one(Statement::from_sql_and_values(
                DbBackend::Postgres,
                "SELECT logical_id FROM flow_block_placements WHERE page_id=$1 AND physical_cid=$2 AND state='active'",
                vec![pages[0].into(), cid.clone().into()],
            ))
            .await
            .unwrap()
            .unwrap()
            .try_get::<Uuid>("", "logical_id")
            .unwrap();
        assert_ne!(replacement, logical);
        exec(db, "DELETE FROM flow_block_placements WHERE id=$1", vec![origin.into()]).await;
        let after = super::super::comments::list_comments(db, pages[0]).await.unwrap();
        for id in [legacy, known.id, late] {
            let view = after.items.iter().find(|view| view.id == id).unwrap();
            assert!(view.block_id.is_none());
            assert_eq!(view.anchor_status.as_deref(), Some("orphaned"));
        }
        assert_eq!(
            after
                .items
                .iter()
                .find(|view| view.id == legacy_alias)
                .unwrap()
                .block_id
                .as_deref(),
            Some(aliased.as_str())
        );
        let anchor = db
            .query_one(Statement::from_sql_and_values(
                DbBackend::Postgres,
                "SELECT logical_id,origin_placement_id FROM flow_block_comment_anchors WHERE comment_id=$1",
                vec![known.id.into()],
            ))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(anchor.try_get::<Uuid>("", "logical_id").unwrap(), logical);
        assert_eq!(anchor.try_get::<Option<Uuid>>("", "origin_placement_id").unwrap(), None);
        assert!(
            super::super::comments::list_comments(db, pages[1])
                .await
                .unwrap()
                .items
                .is_empty()
        );
        exec(db, "DELETE FROM flow_scopes WHERE id=$1", vec![scopes[0].into()]).await;
        let surviving=db.query_one(Statement::from_sql_and_values(DbBackend::Postgres,"SELECT b.state,p.page_id FROM flow_logical_blocks b JOIN flow_block_placements p ON p.logical_id=b.id AND p.state='active' WHERE b.id=$1",vec![logical.into()])).await.unwrap().unwrap();
        assert_eq!(surviving.try_get::<String>("", "state").unwrap(), "active");
        assert_eq!(surviving.try_get::<Uuid>("", "page_id").unwrap(), pages[1]);
        scratch.drop_self().await;
    }

    /// Verbatim copy of the per-alias seal that `sync_legacy` replaced.
    async fn sync_legacy_reference(
        tx: &DatabaseTransaction,
        page: Uuid,
        blocks: &[crate::flow::block_registry::PreparedBlock],
    ) -> Result<(), ApiError> {
        let checkpoint = tx
            .query_one(Statement::from_sql_and_values(
                DbBackend::Postgres,
                "SELECT accepted_head,legacy_anchors_sealed FROM flow_block_checkpoints WHERE page_id=$1 FOR UPDATE",
                vec![page.into()],
            ))
            .await?
            .ok_or_else(ApiError::internal)?;
        if !checkpoint.try_get::<bool>("", "legacy_anchors_sealed")? {
            let head = tx
                .query_one(Statement::from_sql_and_values(
                    DbBackend::Postgres,
                    "SELECT head_seq FROM collab_documents WHERE object_id=$1 FOR UPDATE",
                    vec![page.into()],
                ))
                .await?
                .ok_or_else(ApiError::internal)?;
            if head.try_get::<i64>("", "head_seq")? != checkpoint.try_get::<i64>("", "accepted_head")? {
                return Err(ApiError::policy_rejected("legacy anchor baseline head changed"));
            }
            let mut aliases =
                std::collections::BTreeMap::<String, Vec<&crate::flow::block_registry::PreparedBlock>>::new();
            for block in blocks {
                aliases.entry(block.physical_cid.clone()).or_default().push(block);
                if let Some(alias) = &block.legacy_alias
                    && alias != &block.physical_cid
                {
                    aliases.entry(alias.clone()).or_default().push(block);
                }
            }
            for (alias, candidates) in aliases {
                if candidates.len() != 1 {
                    continue;
                }
                let Some(block) = candidates.first() else {
                    continue;
                };
                tx.execute(Statement::from_sql_and_values(DbBackend::Postgres,r"INSERT INTO flow_block_comment_anchors(comment_id,source_page_id,original_block_id,logical_id,origin_placement_id,origin_generation,state)
    SELECT c.id,c.object_id,c.block_id,p.logical_id,p.id,p.generation,'known'
    FROM flow_comments c JOIN flow_block_placements p ON p.page_id=c.object_id AND p.physical_cid=$3 AND p.state='active'
    JOIN flow_block_alias_fences f ON f.page_id=p.page_id AND f.scope_id=p.scope_id AND f.physical_cid=p.physical_cid AND f.first_logical_id=p.logical_id AND NOT f.ever_retired
    WHERE c.object_id=$1 AND c.block_id=$2 AND NOT EXISTS(SELECT 1 FROM flow_block_alias_fences retired WHERE retired.page_id=$1 AND retired.physical_cid=$2 AND retired.ever_retired)
    ON CONFLICT(comment_id) DO UPDATE SET logical_id=EXCLUDED.logical_id,origin_placement_id=EXCLUDED.origin_placement_id,origin_generation=EXCLUDED.origin_generation,state='known' WHERE flow_block_comment_anchors.logical_id IS NULL",vec![page.into(),alias.into(),block.physical_cid.clone().into()])).await?;
            }
            tx.execute(Statement::from_sql_and_values(
                DbBackend::Postgres,
                "UPDATE flow_block_checkpoints SET legacy_anchors_sealed=true WHERE page_id=$1",
                vec![page.into()],
            ))
            .await?;
        }
        // After sealing, even a currently matching CID cannot nominate a new identity for old rows.
        tx.execute(Statement::from_sql_and_values(DbBackend::Postgres,"INSERT INTO flow_block_comment_anchors(comment_id,source_page_id,original_block_id,state) SELECT id,object_id,block_id,'orphaned' FROM flow_comments WHERE object_id=$1 AND block_id IS NOT NULL ON CONFLICT(comment_id) DO NOTHING",vec![page.into()])).await?;
        Ok(())
    }

    async fn unsealed_legacy_page(
        db: &sea_orm::DatabaseConnection,
    ) -> (Uuid, Vec<super::super::block_registry::PreparedBlock>) {
        use super::super::block_registry::fixtures::{block, page};
        let (actor, scope, page) = page(db).await;
        let mut blocks = (0..6).map(|index| block(index, 1)).collect::<Vec<_>>();
        // old1 and old4 identify one block each; old2 names two blocks; c5 claims the CID of c0, so
        // c0 becomes ambiguous.
        blocks[1].legacy_alias = Some("old1".into());
        blocks[2].legacy_alias = Some("old2".into());
        blocks[3].legacy_alias = Some("old2".into());
        blocks[4].legacy_alias = Some("old4".into());
        blocks[5].legacy_alias = Some(blocks[0].physical_cid.clone());
        exec(
            db,
            "INSERT INTO flow_block_checkpoints(scope_id,page_id,accepted_head) VALUES($1,$2,0)",
            vec![scope.into(), page.into()],
        )
        .await;
        for block in &blocks {
            let logical = Uuid::new_v4();
            exec(
                db,
                "INSERT INTO flow_logical_blocks(id,state) VALUES($1,'active')",
                vec![logical.into()],
            )
            .await;
            exec(db,"INSERT INTO flow_block_placements(id,logical_id,scope_id,page_id,physical_cid,generation,state,content_hash) VALUES($1,$2,$3,$4,$5,1,'active',$6)",vec![Uuid::new_v4().into(),logical.into(),scope.into(),page.into(),block.physical_cid.clone().into(),block.content_hash.to_vec().into()]).await;
            exec(db,"INSERT INTO flow_block_alias_fences(scope_id,page_id,physical_cid,first_logical_id,ever_retired) VALUES($1,$2,$3,$4,false)",vec![scope.into(),page.into(),block.physical_cid.clone().into(),logical.into()]).await;
            if block.legacy_alias.as_deref() == Some("old4") {
                // A retired readable alias must never be adopted.
                exec(db,"INSERT INTO flow_block_alias_fences(scope_id,page_id,physical_cid,first_logical_id,ever_retired) VALUES($1,$2,'old4',$3,true)",vec![scope.into(),page.into(),logical.into()]).await;
            }
        }
        for target in ["cid:0:Map", "old1", "old2", "old4", "cid:5:Map", "cid:2:Map", "unknown"] {
            exec(db,"INSERT INTO flow_comments(id,scope_id,object_id,block_id,author_id,body,created_at) VALUES($1,$2,$3,$4,$5,'Old private note',now()-interval '1 day')",vec![Uuid::new_v4().into(),scope.into(),page.into(),target.into(),actor.into()]).await;
        }
        (page, blocks)
    }

    async fn anchor_rows(db: &sea_orm::DatabaseConnection, page: Uuid) -> Vec<String> {
        db.query_all(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT a.original_block_id||' '||a.state||' gen='||coalesce(a.origin_generation::text,'-')||' cid='||coalesce(p.physical_cid,'-')||' sealed='||c.legacy_anchors_sealed::text AS line FROM flow_block_comment_anchors a LEFT JOIN flow_block_placements p ON p.id=a.origin_placement_id JOIN flow_block_checkpoints c ON c.page_id=a.source_page_id WHERE a.source_page_id=$1 ORDER BY a.original_block_id",
            vec![page.into()],
        ))
        .await
        .unwrap()
        .iter()
        .map(|row| row.try_get::<String>("", "line").unwrap())
        .collect()
    }

    #[tokio::test]
    async fn bulk_legacy_seal_matches_per_alias_reference() {
        let scratch = scratch_or_skip!(&format!("block_anchor_seal_{}", Uuid::new_v4().simple()));
        let db = &scratch.db;
        super::super::block_registry::fixtures::register_reader_leases(db).await;
        // Below floor 5 creating a Page registers nothing, so both Pages start unsealed.
        db.execute_unprepared("UPDATE flow_runtime_capabilities SET minimum_reader_version=4 WHERE singleton")
            .await
            .unwrap();
        let (page_new, blocks_new) = unsealed_legacy_page(db).await;
        let (page_old, blocks_old) = unsealed_legacy_page(db).await;
        db.execute_unprepared("UPDATE flow_runtime_capabilities SET minimum_reader_version=5 WHERE singleton")
            .await
            .unwrap();
        let tx = db.begin().await.unwrap();
        sync_legacy(&tx, page_new, &blocks_new).await.unwrap();
        tx.commit().await.unwrap();
        let tx = db.begin().await.unwrap();
        sync_legacy_reference(&tx, page_old, &blocks_old).await.unwrap();
        tx.commit().await.unwrap();
        let new_rows = anchor_rows(db, page_new).await;
        let old_rows = anchor_rows(db, page_old).await;
        assert_eq!(new_rows, old_rows);
        let adopted = |needle: &str| {
            new_rows
                .iter()
                .any(|row| row.starts_with(needle) && row.contains(" known "))
        };
        assert!(adopted("cid:5:Map "), "{new_rows:?}");
        assert!(adopted("old1 "), "{new_rows:?}");
        assert!(adopted("cid:2:Map "), "{new_rows:?}");
        for refused in ["cid:0:Map ", "old2 ", "old4 ", "unknown "] {
            assert!(
                new_rows
                    .iter()
                    .any(|row| row.starts_with(refused) && row.contains(" orphaned ")),
                "{refused} must stay orphaned: {new_rows:?}"
            );
        }
        scratch.drop_self().await;
    }
}
