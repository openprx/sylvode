//! Private comments never move between Pages. A CID is never used to rebind a known anchor.
use super::block_registry::{PreparedBlock, REGISTRY_ROWS_PER_STATEMENT, RegistrationAllowance};
use super::collab::bootstrap;
use super::collab::write::LOCKED_PHASE_STAGING_BUDGET;
use crate::error::ApiError;
use sea_orm::{ConnectionTrait, DatabaseConnection, DatabaseTransaction, DbBackend, Statement};
use std::time::Instant;
use uuid::Uuid;

pub(crate) struct Prepared {
    document: Uuid,
    head_seq: i64,
    frontier: Vec<u8>,
    blocks: Vec<PreparedBlock>,
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
    if !engine
        .has_editor_doc()
        .map_err(|error| super::command::map_collab_error(&error))?
    {
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
    super::block_registry::reconcile_accepted(
        tx,
        object,
        prepared.head_seq,
        &prepared.blocks,
        RegistrationAllowance::within(Instant::now(), LOCKED_PHASE_STAGING_BUDGET),
    )
    .await
}

/// Upper bound on the transactions one `complete_registration` call may run.
const COMPLETION_STEPS_MAX: usize = 64;
/// How often the Page head may move under one `complete_registration` call before it gives up.
const COMPLETION_HEAD_MOVES_MAX: usize = 3;

/// The head, frontier and prepared blocks of one Page, as one consistent read.
async fn load_snapshot(db: &DatabaseConnection, page: Uuid) -> Result<Prepared, ApiError> {
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
    Ok(Prepared {
        document,
        head_seq: boot.head_seq,
        frontier: boot.head_frontier,
        blocks,
    })
}

/// Result of one bounded registration transaction.
#[derive(Debug, PartialEq, Eq)]
enum RegistrationStep {
    /// Nothing is pending any more.
    Complete,
    /// The transaction committed and blocks are still pending.
    Progress,
    /// The Page head moved since the snapshot was read; nothing was written.
    HeadMoved,
}

fn contention(message: &'static str) -> ApiError {
    ApiError::server_draining(crate::error::ServerDrainingReason::Contention, 200, message)
}

/// One transaction of the completion loop: the lock sequence of an accepted write, the production
/// statement caps, and between `rows_min` (guaranteeing progress) and `rows_max` newly registered
/// rows while the registration stays inside the staging budget.
async fn registration_step(
    db: &DatabaseConnection,
    page: Uuid,
    snapshot: &Prepared,
    rows_min: usize,
    rows_max: usize,
) -> Result<RegistrationStep, ApiError> {
    use sea_orm::TransactionTrait;
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
    super::collab::write::set_locked_phase_statement_budgets(&tx, 1).await?;
    let head = tx
        .query_one(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT head_seq,head_frontier FROM collab_documents WHERE id=$1 FOR UPDATE",
            vec![snapshot.document.into()],
        ))
        .await?
        .ok_or_else(ApiError::internal)?;
    if head.try_get::<i64>("", "head_seq")? != snapshot.head_seq
        || !super::frontier::same(&head.try_get::<Vec<u8>>("", "head_frontier")?, &snapshot.frontier)?
    {
        tx.rollback().await?;
        return Ok(RegistrationStep::HeadMoved);
    }
    let started = Instant::now();
    let allowance = RegistrationAllowance::bounded(started, LOCKED_PHASE_STAGING_BUDGET, rows_max, rows_min);
    let staged = tokio::time::timeout(
        LOCKED_PHASE_STAGING_BUDGET,
        super::block_registry::reconcile(
            &tx,
            identity.scope_id,
            page,
            snapshot.head_seq,
            &snapshot.blocks,
            allowance,
        ),
    )
    .await;
    match staged {
        Ok(Ok(outcome)) => {
            tx.commit().await?;
            Ok(if outcome.pending == 0 {
                RegistrationStep::Complete
            } else {
                RegistrationStep::Progress
            })
        }
        Ok(Err(error)) => {
            let _ = tx.rollback().await;
            Err(error)
        }
        Err(_) => {
            let _ = tx.rollback().await;
            Err(contention("comment baseline busy; retry"))
        }
    }
}

/// Register every block of a Page, in short capped
/// transactions, before an identity consumer (comment list, anchored comment, block share,
/// transfer) reads the registry. A private metadata baseline does not alter the Loro document,
/// native structure or history. Returns a retryable `server_draining{contention}` if the Page
/// keeps changing or needs more than `COMPLETION_STEPS_MAX` steps.
pub(crate) async fn complete_registration(db: &DatabaseConnection, page: Uuid) -> Result<(), ApiError> {
    if super::scope::capabilities(db).await?.minimum_reader_version < 5
        || super::scope::identity(db, page).await?.object_type != "page"
    {
        return Ok(());
    }
    if db
        .query_one(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT page_id FROM flow_block_checkpoints WHERE page_id=$1 AND pending_registrations=0",
            vec![page.into()],
        ))
        .await?
        .is_some()
    {
        return Ok(());
    }
    let mut snapshot: Option<Prepared> = None;
    let mut head_moves = 0;
    for _ in 0..COMPLETION_STEPS_MAX {
        let current = match snapshot.take() {
            Some(current) => current,
            None => load_snapshot(db, page).await?,
        };
        match registration_step(db, page, &current, REGISTRY_ROWS_PER_STATEMENT, usize::MAX).await? {
            RegistrationStep::Complete => return Ok(()),
            RegistrationStep::Progress => snapshot = Some(current),
            RegistrationStep::HeadMoved => {
                head_moves += 1;
                if head_moves >= COMPLETION_HEAD_MOVES_MAX {
                    break;
                }
            }
        }
    }
    Err(contention("comment baseline busy; retry"))
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
        // Replies inherit the exact private original identity, including an unresolved state.
        let changed=db.execute(Statement::from_sql_and_values(DbBackend::Postgres,"INSERT INTO flow_block_comment_anchors(comment_id,source_page_id,original_block_id,logical_id,origin_placement_id,origin_generation,state) SELECT $1,source_page_id,original_block_id,logical_id,origin_placement_id,origin_generation,state FROM flow_block_comment_anchors WHERE comment_id=$2 AND source_page_id=$3",vec![comment.into(),parent.into(),page.into()])).await?;
        if changed.rows_affected() == 0 {
            db.execute(Statement::from_sql_and_values(DbBackend::Postgres,"INSERT INTO flow_block_comment_anchors(comment_id,source_page_id,original_block_id,state) VALUES($1,$2,$3,'orphaned')",vec![comment.into(),page.into(),block.to_string().into()])).await?;
        }
        return Ok(());
    }
    let row=db.query_one(Statement::from_sql_and_values(DbBackend::Postgres,"SELECT p.id,p.logical_id,p.generation FROM flow_block_placements p JOIN flow_block_checkpoints c ON c.page_id=p.page_id AND c.scope_id=p.scope_id JOIN collab_documents d ON d.object_id=p.page_id AND d.head_seq=c.accepted_head WHERE p.page_id=$1 AND p.physical_cid=$2 AND p.state='active' FOR SHARE OF p,c,d",vec![page.into(),block.to_string().into()])).await?;
    let Some(row) = row else {
        // A block that is still pending registration has no identity yet; that is retryable.
        let pending = db
            .query_one(Statement::from_sql_and_values(
                DbBackend::Postgres,
                "SELECT pending_registrations FROM flow_block_checkpoints WHERE page_id=$1",
                vec![page.into()],
            ))
            .await?
            .map(|row| row.try_get::<i32>("", "pending_registrations"))
            .transpose()?;
        if pending.is_none_or(|pending| pending > 0) {
            return Err(contention("block anchor registration pending; retry"));
        }
        return Err(ApiError::invalid_update("block anchor unavailable"));
    };
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
                id: "second-block".into(),
                parent: None,
                index: 1,
                kind: collab_core::NodeKind::Block,
            })
            .unwrap();
        let second_cid = source
            .resolve_content_node_id(&"second-block".into())
            .unwrap()
            .to_string();
        let original_snapshot = source.export_snapshot().unwrap();
        let replayed = LoroCollabEngine::load(&original_snapshot).unwrap();
        let replayed_wire = replayed.frontier();
        let original_frontier = reverse_two_peer_entries(replayed_wire.as_bytes());
        assert_ne!(original_frontier, replayed_wire.as_bytes());
        assert!(replayed.matches_frontier(&original_frontier));
        exec(db,"UPDATE collab_documents SET snapshot=$2,snapshot_checksum=encode(digest($2,'sha256'),'hex'),byte_count=octet_length($2),snapshot_frontier=$3,head_frontier=$3 WHERE id=$1",vec![documents[0].into(),original_snapshot.clone().into(),original_frontier.clone().into()]).await;
        db.execute_unprepared("UPDATE flow_runtime_capabilities SET minimum_reader_version=5 WHERE singleton")
            .await
            .unwrap();
        complete_registration(db, pages[0]).await.unwrap();
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
        // are compared as version vectors, so rows in either peer order keep matching.
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
        db.execute_unprepared(include_str!("../../../../migrations/0095_flow_logical_blocks.sql"))
            .await
            .unwrap();
        let placement=db.query_one(Statement::from_sql_and_values(DbBackend::Postgres,"SELECT id,logical_id FROM flow_block_placements WHERE page_id=$1 AND physical_cid=$2 AND state='active'",vec![pages[0].into(),cid.clone().into()])).await.unwrap().unwrap();
        let origin = placement.try_get::<Uuid>("", "id").unwrap();
        let logical = placement.try_get::<Uuid>("", "logical_id").unwrap();
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
        let second = super::super::comments::create_comment_on(
            &tx,
            None,
            Some(scopes[0]),
            pages[0],
            &super::super::comments::CommentActor {
                user_id: owners[0],
                level: super::super::collab::authz::PermissionLevel::FullAccess,
            },
            super::super::comments::NewComment {
                body: "Second block note".into(),
                block_id: Some(second_cid.clone()),
                parent_id: None,
                mentions: vec![],
            },
        )
        .await
        .unwrap();
        assert_eq!(second.anchor_status.as_deref(), Some("bound"));
        tx.commit().await.unwrap();
        let blocks = super::super::block_registry::prepare(&source.semantic_snapshot().unwrap()).unwrap();
        let tx = db.begin().await.unwrap();
        super::super::block_registry::reconcile(&tx, scopes[0], pages[0], 0, &blocks, RegistrationAllowance::Complete)
            .await
            .unwrap();
        tx.commit().await.unwrap();
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
        let destination_cid = destination
            .resolve_content_node_id(&"moved-carrier".into())
            .unwrap()
            .to_string();
        assert_ne!(destination_cid, cid);
        exec(db,"UPDATE collab_documents SET snapshot=$2,snapshot_checksum=encode(digest($2,'sha256'),'hex'),byte_count=octet_length($2),snapshot_frontier=$3,head_frontier=$3 WHERE id=$1",vec![documents[1].into(),destination.export_snapshot().unwrap().into(),destination.frontier().as_bytes().to_vec().into()]).await;
        exec(db,"INSERT INTO flow_block_placements(id,logical_id,scope_id,page_id,physical_cid,generation,state,content_hash) VALUES($1,$2,$3,$4,$5,2,'active',decode(repeat('00',32),'hex'))",vec![Uuid::new_v4().into(),logical.into(),scopes[1].into(),pages[1].into(),destination_cid.into()]).await;
        let destination_blocks =
            super::super::block_registry::prepare(&destination.semantic_snapshot().unwrap()).unwrap();
        let tx = db.begin().await.unwrap();
        super::super::block_registry::reconcile(
            &tx,
            scopes[1],
            pages[1],
            0,
            &destination_blocks,
            RegistrationAllowance::Complete,
        )
        .await
        .unwrap();
        super::super::block_registry::reconcile(&tx, scopes[0], pages[0], 0, &blocks, RegistrationAllowance::Complete)
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
        let view = after.items.iter().find(|view| view.id == known.id).unwrap();
        assert!(view.block_id.is_none());
        assert_eq!(view.anchor_status.as_deref(), Some("orphaned"));
        assert_eq!(
            after
                .items
                .iter()
                .find(|view| view.id == second.id)
                .unwrap()
                .block_id
                .as_deref(),
            Some(second_cid.as_str())
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

    async fn pending_registrations(db: &sea_orm::DatabaseConnection, page: Uuid) -> i32 {
        db.query_one(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT pending_registrations FROM flow_block_checkpoints WHERE page_id=$1",
            vec![page.into()],
        ))
        .await
        .unwrap()
        .unwrap()
        .try_get("", "pending_registrations")
        .unwrap()
    }

    async fn active_hashes(db: &sea_orm::DatabaseConnection, page: Uuid) -> std::collections::BTreeMap<String, String> {
        db.query_all(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT physical_cid,encode(content_hash,'hex') AS hash FROM flow_block_placements WHERE page_id=$1 AND state='active'",
            vec![page.into()],
        ))
        .await
        .unwrap()
        .iter()
        .map(|row| {
            (
                row.try_get::<String>("", "physical_cid").unwrap(),
                row.try_get::<String>("", "hash").unwrap(),
            )
        })
        .collect()
    }

    /// The completion loop reloads its snapshot when the Page head moves between steps and still
    /// ends with exactly the registry of the final snapshot.
    #[tokio::test]
    async fn completion_loop_survives_interleaved_writes() {
        use super::super::block_registry::fixtures::{blocks, page};
        let scratch = scratch_or_skip!(&format!("block_anchor_loop_{}", Uuid::new_v4().simple()));
        let db = &scratch.db;
        super::super::scope_runtime::fixture::register_running_readers(db).await;
        let (_, _, page_id) = page(db).await;
        let mut snapshot = load_snapshot(db, page_id).await.unwrap();
        snapshot.blocks = blocks(1_200, 1);
        // Step 1 registers 500 blocks and leaves 700 pending.
        assert_eq!(
            registration_step(db, page_id, &snapshot, 0, REGISTRY_ROWS_PER_STATEMENT)
                .await
                .unwrap(),
            RegistrationStep::Progress
        );
        assert_eq!(pending_registrations(db, page_id).await, 700);
        // A write moves the head: the stale snapshot writes nothing.
        exec(
            db,
            "UPDATE collab_documents SET head_seq=head_seq+1 WHERE object_id=$1",
            vec![page_id.into()],
        )
        .await;
        assert_eq!(
            registration_step(db, page_id, &snapshot, 0, REGISTRY_ROWS_PER_STATEMENT)
                .await
                .unwrap(),
            RegistrationStep::HeadMoved
        );
        assert_eq!(pending_registrations(db, page_id).await, 700);
        // The reloaded snapshot of the new head also changed content and dropped a block.
        // (A real reload would replay the document; the bumped head has no update behind it, so the
        // new snapshot is assembled from the same document at the new head.)
        let mut snapshot = Prepared {
            document: snapshot.document,
            head_seq: snapshot.head_seq + 1,
            frontier: snapshot.frontier.clone(),
            blocks: Vec::new(),
        };
        let mut final_blocks = blocks(1_200, 1);
        final_blocks[3] = super::super::block_registry::fixtures::block(3, 2);
        let removed = final_blocks.remove(7).physical_cid;
        snapshot.blocks = final_blocks.clone();
        let mut steps = 0;
        loop {
            steps += 1;
            match registration_step(db, page_id, &snapshot, 0, REGISTRY_ROWS_PER_STATEMENT)
                .await
                .unwrap()
            {
                RegistrationStep::Complete => break,
                RegistrationStep::Progress => assert!(steps < 10, "the loop must converge"),
                RegistrationStep::HeadMoved => panic!("the head did not move again"),
            }
        }
        assert_eq!(
            steps, 2,
            "700 pending blocks need two groups of {REGISTRY_ROWS_PER_STATEMENT}"
        );
        assert_eq!(pending_registrations(db, page_id).await, 0);
        let active = active_hashes(db, page_id).await;
        let expected = final_blocks
            .iter()
            .map(|block| {
                (
                    block.physical_cid.clone(),
                    block.content_hash.iter().fold(String::new(), |mut hex, byte| {
                        use std::fmt::Write as _;
                        let _ = write!(hex, "{byte:02x}");
                        hex
                    }),
                )
            })
            .collect::<std::collections::BTreeMap<_, _>>();
        assert_eq!(active, expected);
        assert!(!active.contains_key(&removed));
        scratch.drop_self().await;
    }

    /// `complete_registration` recognises a page that is registered but still has
    /// pending blocks, registers the real document and clears the counter.
    #[tokio::test]
    async fn complete_registration_finishes_a_partly_registered_page() {
        use super::super::block_registry::fixtures::page;
        let scratch = scratch_or_skip!(&format!("block_anchor_complete_{}", Uuid::new_v4().simple()));
        let db = &scratch.db;
        super::super::scope_runtime::fixture::register_running_readers(db).await;
        let (_, _, page_id) = page(db).await;
        assert_eq!(pending_registrations(db, page_id).await, 0);
        complete_registration(db, page_id).await.unwrap();
        exec(
            db,
            "UPDATE flow_block_checkpoints SET pending_registrations=3 WHERE page_id=$1",
            vec![page_id.into()],
        )
        .await;
        complete_registration(db, page_id).await.unwrap();
        assert_eq!(pending_registrations(db, page_id).await, 0);
        scratch.drop_self().await;
    }
}
