//! Independent Linux production-service transaction exercise; no mocked permissions/apply.
use api::flow::{document, page_transfer_contract as wire, page_transfers, scope, scope_runtime, scope_write};
use base64::{Engine as _, engine::general_purpose::STANDARD as B64};
use collab_core::{CollabEngine, NodeKind, Operation};
use sea_orm::{ConnectionTrait, DbBackend, Statement};
use uuid::Uuid;
#[path = "fixtures/page_service_postgres.rs"]
mod fixture;
#[tokio::test]
#[allow(
    clippy::print_stderr,
    reason = "report the platform skip without a tracing subscriber"
)]
async fn actual_same_scope_move_commits_both_heads_or_neither_and_replays() {
    if !cfg!(target_os = "linux") {
        eprintln!("NOT EXECUTED: real accepted apply requires Linux");
        return;
    }
    let scratch = fixture::scratch(&format!("transfer_{}", Uuid::new_v4().simple()))
        .await
        .unwrap();
    let db = &scratch.db;
    for component in ["api", "worker", "maintenance"] {
        scope_runtime::renew(db, Uuid::new_v4(), component).await.unwrap();
    }
    db.execute_unprepared("UPDATE flow_runtime_capabilities SET minimum_reader_version=5,personal_provisioning=true,personal_writes=true WHERE singleton").await.unwrap();
    let owner = Uuid::new_v4();
    fixture::exec(
        db,
        "INSERT INTO users(id,email,password_hash,name,role,is_active) VALUES($1,$2,'!','Transfer owner','user',true)",
        vec![owner.into(), format!("{owner}@transfer.test").into()],
    )
    .await;
    let home = scope::personal_home(db, owner).await.unwrap();
    let mut pages = Vec::new();
    for title in ["SOURCE_PRIVATE_TITLE", "Target title", "LINK_PRIVATE_TITLE"] {
        let result = scope::create_personal(
            db,
            owner,
            scope::CreateInput {
                scope_id: home.0.id,
                object_type: "page".into(),
                editor_model: 5,
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
                result
                    .pointer("/object/id")
                    .expect("response has /object/id")
                    .as_str()
                    .unwrap(),
            )
            .unwrap(),
            Uuid::parse_str(
                result
                    .pointer("/object/document_id")
                    .expect("response has /object/document_id")
                    .as_str()
                    .unwrap(),
            )
            .unwrap(),
        ));
    }
    let (source, source_doc) = *pages.first().expect("fixture has item 1");
    let (target, target_doc) = *pages.get(1).expect("fixture has item 2");
    let mut engine = document::load_head_engine(db, source_doc).await.unwrap();
    let base = engine.frontier();
    let root = engine.semantic_snapshot().unwrap().nodes.keys().next().unwrap().clone();
    engine
        .apply_content_operation(&Operation::InsertText {
            id: root.clone(),
            index: 0,
            text: "Selected 中文😀".into(),
        })
        .unwrap();
    engine
        .apply_page_marks(
            root.as_ref(),
            &[collab_core::page_inline::MarkEdit {
                start: 9,
                end: 19,
                mark: collab_core::page_inline::InlineMark::Bold,
                clear: false,
                href: None,
            }],
        )
        .unwrap();
    engine
        .apply_content_operation(&Operation::CreateNode {
            id: "child".into(),
            parent: Some(root.clone()),
            index: 0,
            kind: NodeKind::Block,
        })
        .unwrap();
    engine
        .apply_content_operation(&Operation::InsertText {
            id: "child".into(),
            index: 0,
            text: "Selected child".into(),
        })
        .unwrap();
    engine
        .apply_content_operation(&Operation::CreateNode {
            id: "private-sibling".into(),
            parent: None,
            index: 1,
            kind: NodeKind::Block,
        })
        .unwrap();
    engine
        .apply_content_operation(&Operation::InsertText {
            id: "private-sibling".into(),
            index: 0,
            text: "PRIVATE_SIBLING".into(),
        })
        .unwrap();
    let attachment = Uuid::new_v4();
    let attachment_bytes = b"transfer attachment fixture".to_vec();
    fixture::exec(
        db,
        "INSERT INTO flow_page_assets(id,scope_id,object_id,kind,filename,content_type,byte_size,content_sha256,bytes,created_by)
         VALUES($1,$2,$3,'attachment','fixture.bin','application/octet-stream',$4,$5,$6,$7)",
        vec![
            attachment.into(),
            home.0.id.into(),
            source.into(),
            i32::try_from(attachment_bytes.len()).unwrap().into(),
            api::flow::collab::bootstrap::content_hash(&attachment_bytes).into(),
            attachment_bytes.clone().into(),
            owner.into(),
        ],
    )
    .await;
    for (name, index) in [("fixture.bin", 1), ("second-reference.bin", 2)] {
        engine
            .insert_page_atom(
                &format!("media-{index}"),
                Some(root.as_ref()),
                index,
                "attachment",
                &serde_json::json!({
                    "asset_id": attachment,
                    "name": name,
                    "mime": "application/octet-stream",
                    "caption": "Selected attachment",
                    "indent": 0,
                }),
            )
            .unwrap();
    }
    let (linked, linked_doc) = *pages.get(2).expect("fixture has item 3");
    let link_parent = scope::identity(db, linked).await.unwrap().parent_id;
    let created = api::flow::child_pages::reserve(db, source, owner, "CHILD_PRIVATE_TITLE", "transfer-child-fixture")
        .await
        .unwrap()
        .child_page_id;
    for (index, page) in [(3, linked), (4, created)] {
        engine
            .insert_page_atom(
                &format!("child-link-{index}"),
                Some(root.as_ref()),
                index,
                "childPage",
                &serde_json::json!({"child_page_id": page, "indent": 0}),
            )
            .unwrap();
    }
    let active_child_refs = async |object: Uuid| {
        db.query_all(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT child_page_id FROM flow_page_child_refs WHERE source_object_id=$1 AND active",
            vec![object.into()],
        ))
        .await
        .unwrap()
        .iter()
        .map(|row| row.try_get::<Uuid>("", "child_page_id").unwrap())
        .collect::<std::collections::BTreeSet<_>>()
    };
    let expected_child_refs = std::collections::BTreeSet::from([linked, created]);
    let target_assets = async || {
        db.query_all(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT id,bytes,state FROM flow_page_assets WHERE object_id=$1 ORDER BY id",
            vec![target.into()],
        ))
        .await
        .unwrap()
    };
    let source_asset_state = async || {
        db.query_one(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT state FROM flow_page_assets WHERE id=$1",
            vec![attachment.into()],
        ))
        .await
        .unwrap()
        .unwrap()
        .try_get::<String>("", "state")
        .unwrap()
    };
    let result = scope_write::accept(
        db,
        source,
        engine.export_from(&base).unwrap(),
        scope_write::UpdateIntent {
            user: owner,
            update: Uuid::new_v4(),
            client: "transfer-fixture",
            key: None,
            expected: Some(base.as_bytes()),
            command_body: None,
        },
    )
    .await
    .unwrap();
    assert!(matches!(result, api::flow::collab::write::AcceptOutcome::Accepted(_)));
    let old_uuid = db
        .query_one(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT logical_id FROM flow_block_placements WHERE page_id=$1 AND physical_cid=$2 AND state='active'",
            vec![source.into(), root.to_string().into()],
        ))
        .await
        .unwrap()
        .unwrap()
        .try_get::<Uuid>("", "logical_id")
        .unwrap();
    let source_frontier = db
        .query_one(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT head_frontier FROM collab_documents WHERE id=$1",
            vec![source_doc.into()],
        ))
        .await
        .unwrap()
        .unwrap()
        .try_get::<Vec<u8>>("", "head_frontier")
        .unwrap();
    let target_frontier = db
        .query_one(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT head_frontier FROM collab_documents WHERE id=$1",
            vec![target_doc.into()],
        ))
        .await
        .unwrap()
        .unwrap()
        .try_get::<Vec<u8>>("", "head_frontier")
        .unwrap();
    let key = Uuid::new_v4().to_string();
    let prepared = page_transfers::prepare(
        db,
        owner,
        source,
        &key,
        wire::PrepareInput {
            kind: wire::Kind::Move,
            target_object_id: target,
            root_block_cid: root.to_string(),
            selection: wire::Selection::CurrentSubtree,
            target_parent_block_cid: None,
            target_index: 0,
            expected_source_frontier: B64.encode(&source_frontier),
            expected_target_frontier: B64.encode(&target_frontier),
        },
    )
    .await
    .unwrap();
    let id = Uuid::parse_str(
        prepared
            .pointer("/transfer_id")
            .expect("response has /transfer_id")
            .as_str()
            .unwrap(),
    )
    .unwrap();
    assert_eq!(
        *prepared
            .pointer("/review/block_count")
            .expect("response has /review/block_count"),
        6
    );
    assert!(!prepared.to_string().contains("LINK_PRIVATE_TITLE"));
    assert!(!prepared.to_string().contains("CHILD_PRIVATE_TITLE"));
    for child in &expected_child_refs {
        assert!(!prepared.to_string().contains(&child.to_string()));
    }
    assert_eq!(active_child_refs(source).await, expected_child_refs);
    assert!(active_child_refs(target).await.is_empty());
    let linked_frontier = db
        .query_one(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT head_frontier FROM collab_documents WHERE id=$1",
            vec![linked_doc.into()],
        ))
        .await
        .unwrap()
        .unwrap()
        .try_get::<Vec<u8>>("", "head_frontier")
        .unwrap();
    let self_link = page_transfers::prepare(
        db,
        owner,
        source,
        "child-self-test",
        wire::PrepareInput {
            kind: wire::Kind::Copy,
            target_object_id: linked,
            root_block_cid: root.to_string(),
            selection: wire::Selection::CurrentSubtree,
            target_parent_block_cid: None,
            target_index: 0,
            expected_source_frontier: B64.encode(&source_frontier),
            expected_target_frontier: B64.encode(&linked_frontier),
        },
    )
    .await
    .unwrap_err();
    assert!(format!("{self_link:?}").contains("transfer_child_reference_self"));
    assert_eq!(
        *prepared
            .pointer("/review/asset_count")
            .expect("response has /review/asset_count"),
        1
    );
    assert_eq!(
        *prepared
            .pointer("/assets/total_bytes")
            .expect("response has /assets/total_bytes"),
        attachment_bytes.len()
    );
    assert!(target_assets().await.is_empty());
    assert_eq!(source_asset_state().await, "active");
    let input = wire::CommitInput {
        expected_source_frontier: B64.encode(&source_frontier),
        expected_target_frontier: B64.encode(&target_frontier),
        selection_digest: prepared
            .pointer("/review/selection_digest")
            .expect("response has /review/selection_digest")
            .as_str()
            .unwrap()
            .into(),
    };
    let collaborator = Uuid::new_v4();
    fixture::exec(
        db,
        "INSERT INTO users(id,email,password_hash,name,role,is_active) VALUES($1,$2,'!','Transfer collaborator','user',true)",
        vec![collaborator.into(), format!("{collaborator}@transfer.test").into()],
    ).await;
    let grant = async |object, level: &str| {
        fixture::exec(
            db,
            "INSERT INTO flow_object_grants(scope_id,object_id,principal_kind,principal_id,level,granted_by)
             VALUES($1,$2,'user',$3,$4,$5)",
            vec![
                home.0.id.into(),
                object,
                collaborator.into(),
                level.to_owned().into(),
                owner.into(),
            ],
        )
        .await;
    };
    grant(source.into(), "view").await;
    grant(target.into(), "edit").await;
    grant(created.into(), "view").await;
    let collaborator_input = wire::PrepareInput {
        kind: wire::Kind::Copy,
        target_object_id: target,
        root_block_cid: root.to_string(),
        selection: wire::Selection::CurrentSubtree,
        target_parent_block_cid: None,
        target_index: 0,
        expected_source_frontier: B64.encode(&source_frontier),
        expected_target_frontier: B64.encode(&target_frontier),
    };
    let denied = page_transfers::prepare(db, collaborator, source, "copy-link-denied", collaborator_input.clone())
        .await
        .unwrap_err();
    assert!(format!("{denied:?}").contains("transfer_child_reference_unavailable"));
    grant(linked.into(), "view").await;
    let allowed = page_transfers::prepare(db, collaborator, source, "copy-link-allowed", collaborator_input)
        .await
        .unwrap();
    let allowed_id = Uuid::parse_str(
        allowed
            .pointer("/transfer_id")
            .expect("response has /transfer_id")
            .as_str()
            .unwrap(),
    )
    .unwrap();
    fixture::exec(
        db,
        "DELETE FROM flow_object_grants WHERE object_id=$1 AND principal_id=$2",
        vec![linked.into(), collaborator.into()],
    )
    .await;
    let revoked = page_transfers::commit(
        db,
        collaborator,
        allowed_id,
        "copy-link-revoked",
        wire::CommitInput {
            expected_source_frontier: B64.encode(&source_frontier),
            expected_target_frontier: B64.encode(&target_frontier),
            selection_digest: allowed
                .pointer("/review/selection_digest")
                .expect("response has /review/selection_digest")
                .as_str()
                .unwrap()
                .to_owned(),
        },
    )
    .await
    .unwrap_err();
    assert!(format!("{revoked:?}").contains("transfer_child_reference_unavailable"));
    assert!(active_child_refs(target).await.is_empty());
    assert!(target_assets().await.is_empty());
    let commit_key = Uuid::new_v4().to_string();
    let set_link_lifecycle = async |state: &str| {
        fixture::exec(
            db,
            "UPDATE flow_objects SET lifecycle_status=$2,archived_at=CASE WHEN $2='archived' THEN now() END WHERE id=$1",
            vec![linked.into(), state.to_owned().into()],
        )
        .await;
    };
    set_link_lifecycle("archived").await;
    let unavailable = page_transfers::commit(db, owner, id, &commit_key, input.clone())
        .await
        .unwrap_err();
    assert!(format!("{unavailable:?}").contains("transfer_child_reference_unavailable"));
    assert_eq!(active_child_refs(source).await, expected_child_refs);
    assert!(active_child_refs(target).await.is_empty());
    set_link_lifecycle("active").await;
    db.execute_unprepared("CREATE FUNCTION test_transfer_rollback() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN IF NEW.state='accepted' THEN RAISE EXCEPTION 'synthetic transfer rollback' USING ERRCODE='23514'; END IF; RETURN NEW; END $$; CREATE TRIGGER test_transfer_rollback BEFORE UPDATE ON flow_page_block_transfers FOR EACH ROW EXECUTE FUNCTION test_transfer_rollback();").await.unwrap();
    assert!(
        page_transfers::commit(db, owner, id, &commit_key, input.clone())
            .await
            .is_err()
    );
    assert!(target_assets().await.is_empty());
    assert_eq!(source_asset_state().await, "active");
    for (doc, expected) in [(source_doc, &source_frontier), (target_doc, &target_frontier)] {
        assert_eq!(
            db.query_one(Statement::from_sql_and_values(
                DbBackend::Postgres,
                "SELECT head_frontier FROM collab_documents WHERE id=$1",
                vec![doc.into()]
            ))
            .await
            .unwrap()
            .unwrap()
            .try_get::<Vec<u8>>("", "head_frontier")
            .unwrap(),
            *expected
        );
    }
    assert_eq!(
        db.query_one(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT page_id FROM flow_block_placements WHERE logical_id=$1 AND state='active'",
            vec![old_uuid.into()]
        ))
        .await
        .unwrap()
        .unwrap()
        .try_get::<Uuid>("", "page_id")
        .unwrap(),
        source
    );
    db.execute_unprepared(
        "DROP TRIGGER test_transfer_rollback ON flow_page_block_transfers;DROP FUNCTION test_transfer_rollback();",
    )
    .await
    .unwrap();
    let accepted = page_transfers::commit(db, owner, id, &commit_key, input.clone())
        .await
        .unwrap();
    assert_eq!(*accepted.pointer("/state").expect("response has /state"), "accepted");
    assert!(active_child_refs(source).await.is_empty());
    assert_eq!(active_child_refs(target).await, expected_child_refs);
    let moved_assets = target_assets().await;
    assert_eq!(moved_assets.len(), 1);
    assert_ne!(
        moved_assets
            .first()
            .expect("fixture has item 1")
            .try_get::<Uuid>("", "id")
            .unwrap(),
        attachment
    );
    assert_eq!(
        moved_assets
            .first()
            .expect("fixture has item 1")
            .try_get::<Vec<u8>>("", "bytes")
            .unwrap(),
        attachment_bytes
    );
    assert_eq!(
        moved_assets
            .first()
            .expect("fixture has item 1")
            .try_get::<String>("", "state")
            .unwrap(),
        "active"
    );
    assert_eq!(source_asset_state().await, "retained");
    let moved = accepted
        .pointer("/receipt/mappings/0/target_block_cid")
        .expect("response has /receipt/mappings/0/target_block_cid")
        .as_str()
        .unwrap();
    assert_ne!(moved, root.as_ref());
    let src = document::load_head_engine(db, source_doc)
        .await
        .unwrap()
        .semantic_snapshot()
        .unwrap();
    let dst = document::load_head_engine(db, target_doc)
        .await
        .unwrap()
        .semantic_snapshot()
        .unwrap();
    assert!(src.nodes.values().any(|node| node.text == "PRIVATE_SIBLING"));
    assert!(!src.nodes.values().any(|node| node.text == "Selected 中文😀"));
    assert!(dst.nodes.values().any(|node| node.text == "Selected 中文😀"));
    assert!(!dst.nodes.values().any(|node| node.text == "PRIVATE_SIBLING"));
    assert_eq!(
        document::load_head_engine(db, target_doc)
            .await
            .unwrap()
            .title()
            .unwrap(),
        "Target title"
    );
    assert_eq!(
        db.query_one(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT logical_id FROM flow_block_placements WHERE page_id=$1 AND physical_cid=$2 AND state='active'",
            vec![target.into(), moved.to_string().into()]
        ))
        .await
        .unwrap()
        .unwrap()
        .try_get::<Uuid>("", "logical_id")
        .unwrap(),
        old_uuid
    );
    let replay = page_transfers::commit(db, owner, id, &commit_key, input).await.unwrap();
    assert_eq!(*replay.pointer("/state").expect("response has /state"), "accepted");
    let undo_key = Uuid::new_v4().to_string();
    let undo_input = wire::UndoInput {
        expected_source_frontier: accepted
            .pointer("/receipt/source/frontier")
            .expect("response has /receipt/source/frontier")
            .as_str()
            .unwrap()
            .into(),
        expected_target_frontier: accepted
            .pointer("/receipt/target/frontier")
            .expect("response has /receipt/target/frontier")
            .as_str()
            .unwrap()
            .into(),
    };
    set_link_lifecycle("archived").await;
    let unavailable = page_transfers::undo(db, owner, id, &undo_key, undo_input.clone())
        .await
        .unwrap_err();
    assert!(format!("{unavailable:?}").contains("transfer_child_reference_unavailable"));
    assert!(active_child_refs(source).await.is_empty());
    assert_eq!(active_child_refs(target).await, expected_child_refs);
    set_link_lifecycle("active").await;
    let undone = page_transfers::undo(db, owner, id, &undo_key, undo_input.clone())
        .await
        .unwrap();
    assert_eq!(*undone.pointer("/state").expect("response has /state"), "undone");
    assert_eq!(active_child_refs(source).await, expected_child_refs);
    assert!(active_child_refs(target).await.is_empty());
    assert_eq!(source_asset_state().await, "active");
    assert_eq!(
        target_assets()
            .await
            .first()
            .expect("fixture has item 1")
            .try_get::<String>("", "state")
            .unwrap(),
        "retained"
    );
    assert_eq!(
        *undone.pointer("/receipt/undo").expect("response has /receipt/undo"),
        serde_json::json!({"state":"unavailable"})
    );
    assert_eq!(
        *undone
            .pointer("/receipt/source/object_id")
            .expect("response has /receipt/source/object_id"),
        source.to_string()
    );
    assert_eq!(
        *undone
            .pointer("/receipt/target/object_id")
            .expect("response has /receipt/target/object_id"),
        target.to_string()
    );
    let restored = document::load_head_engine(db, source_doc)
        .await
        .unwrap()
        .semantic_snapshot()
        .unwrap();
    let cleaned = document::load_head_engine(db, target_doc)
        .await
        .unwrap()
        .semantic_snapshot()
        .unwrap();
    assert!(restored.nodes.values().any(|node| node.text == "Selected 中文😀"));
    assert!(restored.nodes.values().any(|node| node.text == "Selected child"));
    assert!(restored.nodes.values().any(|node| node.text == "PRIVATE_SIBLING"));
    assert!(!cleaned.nodes.values().any(|node| node.text == "Selected 中文😀"));
    assert_eq!(
        db.query_one(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT page_id FROM flow_block_placements WHERE logical_id=$1 AND state='active'",
            vec![old_uuid.into()]
        ))
        .await
        .unwrap()
        .unwrap()
        .try_get::<Uuid>("", "page_id")
        .unwrap(),
        source
    );
    let retry = page_transfers::undo(db, owner, id, &undo_key, undo_input)
        .await
        .unwrap();
    assert_eq!(*retry.pointer("/state").expect("response has /state"), "undone");
    // Copy is independent: both destination CIDs and logical IDs are new, source stays intact.
    let fresh_source = db
        .query_one(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT head_frontier FROM collab_documents WHERE id=$1",
            vec![source_doc.into()],
        ))
        .await
        .unwrap()
        .unwrap()
        .try_get::<Vec<u8>>("", "head_frontier")
        .unwrap();
    let fresh_target = db
        .query_one(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT head_frontier FROM collab_documents WHERE id=$1",
            vec![target_doc.into()],
        ))
        .await
        .unwrap()
        .unwrap()
        .try_get::<Vec<u8>>("", "head_frontier")
        .unwrap();
    let restored_root = undone
        .pointer("/receipt/mappings/0/source_block_cid")
        .expect("response has /receipt/mappings/0/source_block_cid")
        .as_str()
        .unwrap();
    let copy_prepared = page_transfers::prepare(
        db,
        owner,
        source,
        &Uuid::new_v4().to_string(),
        wire::PrepareInput {
            kind: wire::Kind::Copy,
            target_object_id: target,
            root_block_cid: restored_root.into(),
            selection: wire::Selection::CurrentSubtree,
            target_parent_block_cid: None,
            target_index: 0,
            expected_source_frontier: B64.encode(&fresh_source),
            expected_target_frontier: B64.encode(&fresh_target),
        },
    )
    .await
    .unwrap();
    let copy_id = Uuid::parse_str(
        copy_prepared
            .pointer("/transfer_id")
            .expect("response has /transfer_id")
            .as_str()
            .unwrap(),
    )
    .unwrap();
    let copied = page_transfers::commit(
        db,
        owner,
        copy_id,
        &Uuid::new_v4().to_string(),
        wire::CommitInput {
            expected_source_frontier: B64.encode(&fresh_source),
            expected_target_frontier: B64.encode(&fresh_target),
            selection_digest: copy_prepared
                .pointer("/review/selection_digest")
                .expect("response has /review/selection_digest")
                .as_str()
                .unwrap()
                .into(),
        },
    )
    .await
    .unwrap();
    assert_eq!(active_child_refs(source).await, expected_child_refs);
    assert_eq!(active_child_refs(target).await, expected_child_refs);
    assert_eq!(
        *copied
            .pointer("/receipt/changed_page_ids")
            .expect("response has /receipt/changed_page_ids"),
        serde_json::json!([target])
    );
    let copied_assets = target_assets().await;
    assert_eq!(copied_assets.len(), 2);
    let active_copy = copied_assets
        .iter()
        .find(|copy| copy.try_get::<String>("", "state").unwrap() == "active")
        .unwrap();
    assert_eq!(active_copy.try_get::<Vec<u8>>("", "bytes").unwrap(), attachment_bytes);
    assert_eq!(source_asset_state().await, "active");
    let copied_block_cid = copied
        .pointer("/receipt/mappings/0/target_block_cid")
        .expect("response has /receipt/mappings/0/target_block_cid")
        .as_str()
        .unwrap();
    let copy_uuid = db
        .query_one(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT logical_id FROM flow_block_placements WHERE page_id=$1 AND physical_cid=$2 AND state='active'",
            vec![target.into(), copied_block_cid.to_string().into()],
        ))
        .await
        .unwrap()
        .unwrap()
        .try_get::<Uuid>("", "logical_id")
        .unwrap();
    assert_ne!(copy_uuid, old_uuid);
    assert_eq!(
        db.query_one(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT head_frontier FROM collab_documents WHERE id=$1",
            vec![source_doc.into()]
        ))
        .await
        .unwrap()
        .unwrap()
        .try_get::<Vec<u8>>("", "head_frontier")
        .unwrap(),
        fresh_source
    );
    let undo_copy_input = wire::UndoInput {
        expected_source_frontier: copied
            .pointer("/receipt/source/frontier")
            .expect("response has /receipt/source/frontier")
            .as_str()
            .unwrap()
            .into(),
        expected_target_frontier: copied
            .pointer("/receipt/target/frontier")
            .expect("response has /receipt/target/frontier")
            .as_str()
            .unwrap()
            .into(),
    };
    let undo_copy_key = Uuid::new_v4().to_string();
    let replacement = Uuid::new_v4();
    fixture::exec(
        db,
        "INSERT INTO flow_logical_blocks(id,state) VALUES($1,'active')",
        vec![replacement.into()],
    )
    .await;
    let head_pair = async || {
        db.query_all(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT head_frontier FROM collab_documents WHERE id IN ($1,$2) ORDER BY id",
            vec![source_doc.into(), target_doc.into()],
        ))
        .await
        .unwrap()
        .iter()
        .map(|row| row.try_get::<Vec<u8>>("", "head_frontier").unwrap())
        .collect::<Vec<_>>()
    };
    let copy_heads = head_pair().await;
    let copy_status = page_transfers::get(db, owner, copy_id).await.unwrap();
    let placement_sql = "UPDATE flow_block_placements SET logical_id=$3,generation=$4
        WHERE page_id=$1 AND physical_cid=$2 AND state='active'";
    let generation = db
        .query_one(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT generation FROM flow_block_placements WHERE logical_id=$1 AND state='active'",
            vec![copy_uuid.into()],
        ))
        .await
        .unwrap()
        .unwrap()
        .try_get::<i64>("", "generation")
        .unwrap();
    // Simulate registry-only drift without editing either reviewed Doc head or its CID set.
    for (logical_id, placement_generation) in [(copy_uuid, generation + 1), (replacement, generation)] {
        fixture::exec(
            db,
            placement_sql,
            vec![
                target.into(),
                copied_block_cid.to_string().into(),
                logical_id.into(),
                placement_generation.into(),
            ],
        )
        .await;
        let rejected = page_transfers::undo(db, owner, copy_id, &undo_copy_key, undo_copy_input.clone())
            .await
            .unwrap_err();
        assert!(format!("{rejected:?}").contains("transfer_identity_changed"));
        assert_eq!(head_pair().await, copy_heads);
        assert_eq!(page_transfers::get(db, owner, copy_id).await.unwrap(), copy_status);
    }
    fixture::exec(
        db,
        placement_sql,
        vec![
            target.into(),
            copied_block_cid.to_string().into(),
            copy_uuid.into(),
            generation.into(),
        ],
    )
    .await;
    let proof = db
        .query_one(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT copied_placements FROM flow_page_block_transfers WHERE id=$1",
            vec![copy_id.into()],
        ))
        .await
        .unwrap()
        .unwrap()
        .try_get::<serde_json::Value>("", "copied_placements")
        .unwrap();
    assert_eq!(
        proof.as_array().unwrap().len(),
        copied
            .pointer("/receipt/mappings")
            .expect("response has /receipt/mappings")
            .as_array()
            .unwrap()
            .len()
    );
    assert!(!serde_json::to_string(&copied).unwrap().contains(&copy_uuid.to_string()));
    fixture::exec(
        db,
        "UPDATE flow_page_block_transfers SET copied_placements=NULL WHERE id=$1",
        vec![copy_id.into()],
    )
    .await;
    let legacy = page_transfers::get(db, owner, copy_id).await.unwrap();
    assert_eq!(
        *legacy.pointer("/receipt/undo").expect("response has /receipt/undo"),
        serde_json::json!({"state":"unavailable"})
    );
    let legacy_rejected = page_transfers::undo(db, owner, copy_id, &undo_copy_key, undo_copy_input.clone())
        .await
        .unwrap_err();
    assert!(format!("{legacy_rejected:?}").contains("transfer_undo_unavailable"));
    assert_eq!(head_pair().await, copy_heads);
    fixture::exec(
        db,
        "UPDATE flow_page_block_transfers SET copied_placements=$2 WHERE id=$1",
        vec![copy_id.into(), proof.into()],
    )
    .await;
    let copy_undo = page_transfers::undo(db, owner, copy_id, &undo_copy_key, undo_copy_input)
        .await
        .unwrap();
    assert_eq!(*copy_undo.pointer("/state").expect("response has /state"), "undone");
    assert_eq!(active_child_refs(source).await, expected_child_refs);
    assert!(active_child_refs(target).await.is_empty());
    let preserved = db
        .query_one(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT (SELECT parent_id FROM flow_objects WHERE id=$1) AS child_parent,
                (SELECT parent_id FROM flow_objects WHERE id=$2) AS link_parent,
                (SELECT count(*) FROM flow_child_page_reservations WHERE state='finalized') AS finalized,
                (SELECT count(*) FROM flow_objects WHERE parent_id=$3) AS children",
            vec![created.into(), linked.into(), source.into()],
        ))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(preserved.try_get::<Uuid>("", "child_parent").unwrap(), source);
    assert_eq!(
        preserved.try_get::<Option<Uuid>>("", "link_parent").unwrap(),
        link_parent
    );
    assert_eq!(preserved.try_get::<i64>("", "finalized").unwrap(), 1);
    assert_eq!(preserved.try_get::<i64>("", "children").unwrap(), 1);
    assert_eq!(source_asset_state().await, "active");
    assert!(
        target_assets()
            .await
            .iter()
            .all(|copy| copy.try_get::<String>("", "state").unwrap() == "retained")
    );
    assert_eq!(
        *copy_undo
            .pointer("/receipt/changed_page_ids")
            .expect("response has /receipt/changed_page_ids"),
        serde_json::json!([target])
    );
    let replacement_owner = Uuid::new_v4();
    fixture::exec(db,"INSERT INTO users(id,email,password_hash,name,role,is_active) VALUES($1,$2,'!','Replacement owner','user',true)",vec![replacement_owner.into(),format!("{replacement_owner}@transfer.test").into()]).await;
    fixture::exec(
        db,
        "UPDATE flow_scopes SET owner_user_id=$2 WHERE id=$1",
        vec![home.0.id.into(), replacement_owner.into()],
    )
    .await;
    let ack_only = page_transfers::get(db, owner, id).await.unwrap();
    assert_eq!(*ack_only.pointer("/state").expect("response has /state"), "undone");
    assert!(ack_only.pointer("/receipt").expect("response has /receipt").is_null());
    assert!(!serde_json::to_string(&ack_only).unwrap().contains(root.as_ref()));
    if let Ok(directory) = std::env::var("SV_TRANSFER_FIXTURE_DIRECTORY") {
        let directory = std::path::Path::new(&directory);
        std::fs::create_dir_all(directory).unwrap();
        for (name, value) in [
            ("prepare", &prepared),
            ("accepted", &accepted),
            ("undone", &undone),
            ("copy-accepted", &copied),
            ("copy-undone", &copy_undo),
            ("metadata-only", &ack_only),
        ] {
            std::fs::write(
                directory.join(format!("transfer-{name}.json")),
                serde_json::to_string_pretty(value).unwrap(),
            )
            .unwrap();
        }
    }
    scratch.drop_self().await;
}

#[tokio::test]
async fn workspace_transfer_keeps_admission_and_write_guards() {
    use api::flow::{collab::authz::PermissionLevel, repository as repo};
    use collab_core::LoroCollabEngine;
    use sea_orm::TransactionTrait;
    use serde_json::json;

    if !cfg!(target_os = "linux") {
        return;
    }
    let scratch = fixture::scratch(&format!("transfer_guards_{}", Uuid::new_v4().simple()))
        .await
        .unwrap();
    let db = &scratch.db;
    // Reader 5 is the default since 0106; start below it so the transfer reader gate is exercised.
    db.execute_unprepared("UPDATE flow_runtime_capabilities SET minimum_reader_version=4 WHERE singleton")
        .await
        .unwrap();
    let workspace = Uuid::new_v4();
    let owner = Uuid::new_v4();
    let reader = Uuid::new_v4();
    for actor in [owner, reader] {
        fixture::exec(
            db,
            "INSERT INTO users(id,email,password_hash,name,role,is_active) VALUES($1,$2,'!','Workspace fixture','user',true)",
            vec![actor.into(), format!("{actor}@transfer.test").into()],
        )
        .await;
    }
    fixture::exec(
        db,
        "INSERT INTO workspaces(id,slug,name,created_by) VALUES($1,$2,'Workspace fixture',$3)",
        vec![workspace.into(), workspace.to_string().into(), owner.into()],
    )
    .await;
    fixture::exec(
        db,
        "INSERT INTO workspace_members(workspace_id,user_id,role) VALUES($1,$2,'owner')",
        vec![workspace.into(), owner.into()],
    )
    .await;
    fixture::exec(
        db,
        "INSERT INTO flow_workspace_settings(workspace_id,flow_enabled) VALUES($1,true)",
        vec![workspace.into()],
    )
    .await;
    let workspace_scope = scope::legacy_scope(db, workspace).await.unwrap();
    let parent = scope::roots(db, workspace_scope.id)
        .await
        .unwrap()
        .first()
        .expect("fixture has item 1")
        .object_id;
    let mut pages = Vec::new();
    for admitted in [true, true, false] {
        let object = Uuid::new_v4();
        let document = Uuid::new_v4();
        let mut engine = LoroCollabEngine::new_empty(1);
        engine.set_title("Workspace fixture Page").unwrap();
        let cid = if admitted {
            engine.initialize_page_editor().unwrap();
            engine.initialize_page_structure_model(5).unwrap();
            engine
                .semantic_snapshot()
                .unwrap()
                .nodes
                .keys()
                .next()
                .map(ToString::to_string)
        } else {
            None
        };
        let frontier = engine.frontier().as_bytes().to_vec();
        let tx = db.begin().await.unwrap();
        repo::insert_flow_object(
            &tx,
            &repo::NewFlowObject {
                id: object,
                workspace_id: workspace,
                project_id: None,
                object_type: "page".into(),
                parent_id: Some(parent),
                created_by: Some(owner),
                governance_metadata: json!({}),
            },
        )
        .await
        .unwrap();
        repo::insert_collab_document(
            &tx,
            &repo::NewCollabDocument {
                id: document,
                object_id: object,
                format_version: "loro-1".into(),
                snapshot: engine.export_snapshot().unwrap(),
                frontier: frontier.clone(),
            },
        )
        .await
        .unwrap();
        repo::insert_projection(
            &tx,
            &repo::NewProjection {
                object_id: object,
                document_seq: 0,
                document_frontier: frontier.clone(),
                title: "Workspace fixture Page".into(),
                state: json!(engine.semantic_snapshot().unwrap()),
                plain_text: String::new(),
            },
        )
        .await
        .unwrap();
        tx.commit().await.unwrap();
        if admitted {
            fixture::exec(
                db,
                "INSERT INTO flow_object_grants(scope_id,workspace_id,object_id,principal_kind,principal_id,level,granted_by)
                 VALUES($1,$2,$3,'user',$4,'view',$5)",
                vec![workspace_scope.id.into(), workspace.into(), object.into(), reader.into(), owner.into()],
            )
            .await;
        }
        pages.push((object, document, B64.encode(frontier), cid));
    }
    let source = pages.first().expect("fixture has item 1");
    let target = pages.get(1).expect("fixture has item 2");
    let legacy = pages.get(2).expect("fixture has item 3");
    let prepare = |actor, destination: &(Uuid, Uuid, String, Option<String>)| {
        let input = wire::PrepareInput {
            kind: wire::Kind::Copy,
            target_object_id: destination.0,
            root_block_cid: source.3.clone().unwrap(),
            selection: wire::Selection::Block,
            target_parent_block_cid: None,
            target_index: 0,
            expected_source_frontier: source.2.clone(),
            expected_target_frontier: destination.2.clone(),
        };
        async move { page_transfers::prepare(db, actor, source.0, &Uuid::new_v4().to_string(), input).await }
    };
    let err = prepare(owner, target).await.unwrap_err();
    assert!(format!("{err:?}").contains("transfer_readers_not_ready"));
    db.execute_unprepared(
        "UPDATE flow_runtime_capabilities SET minimum_reader_version=5,scope_native_legacy=false WHERE singleton",
    )
    .await
    .unwrap();
    assert!(
        prepare(owner, target).await.is_err(),
        "reader leases are independently required"
    );
    for component in ["api", "worker", "maintenance"] {
        scope_runtime::renew(db, Uuid::new_v4(), component).await.unwrap();
    }
    assert_eq!(
        scope::authorize(db, target.0, reader, PermissionLevel::View)
            .await
            .unwrap()
            .level,
        PermissionLevel::View
    );
    assert!(prepare(reader, target).await.is_err(), "Read cannot write the target");
    let err = prepare(owner, legacy).await.unwrap_err();
    assert!(format!("{err:?}").contains("explicit_editor_initialization_required"));
    assert!(
        !document::load_head_engine(db, legacy.1)
            .await
            .unwrap()
            .has_editor_doc()
            .unwrap()
    );

    let prepared = prepare(owner, target).await.unwrap();
    let id = Uuid::parse_str(
        prepared
            .pointer("/transfer_id")
            .expect("response has /transfer_id")
            .as_str()
            .unwrap(),
    )
    .unwrap();
    let input = wire::CommitInput {
        expected_source_frontier: source.2.clone(),
        expected_target_frontier: target.2.clone(),
        selection_digest: prepared
            .pointer("/review/selection_digest")
            .expect("response has /review/selection_digest")
            .as_str()
            .unwrap()
            .into(),
    };
    fixture::exec(
        db,
        "UPDATE flow_workspace_settings SET flow_enabled=false WHERE workspace_id=$1",
        vec![workspace.into()],
    )
    .await;
    let err = prepare(owner, target).await.unwrap_err();
    assert!(format!("{err:?}").contains("flow is not enabled for this workspace"));
    assert!(
        page_transfers::commit(db, owner, id, "workspace-commit", input.clone())
            .await
            .is_err()
    );
    fixture::exec(
        db,
        "UPDATE flow_workspace_settings SET flow_enabled=true WHERE workspace_id=$1",
        vec![workspace.into()],
    )
    .await;
    for (_, document, frontier, _) in &pages {
        assert_eq!(
            B64.encode(
                document::load_head_engine(db, *document)
                    .await
                    .unwrap()
                    .frontier()
                    .as_bytes()
            ),
            *frontier
        );
    }
    let accepted = page_transfers::commit(db, owner, id, "workspace-commit", input)
        .await
        .unwrap();
    assert_eq!(*accepted.pointer("/state").expect("response has /state"), "accepted");
    assert!(!scope::capabilities(db).await.unwrap().scope_native_legacy);
    scratch.drop_self().await;
}
