//! Portable Page promotion with real `PostgreSQL` and the production isolated Loro apply worker.
use api::flow::collab::bootstrap;
use api::flow::{
    document, portable_graph_package as graph, portable_page_package as package, scope, scope_runtime, scope_write,
};
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
async fn portable_page_copies_assets_and_links_atomically() {
    if !cfg!(target_os = "linux") {
        eprintln!("NOT EXECUTED: real accepted apply requires Linux");
        return;
    }
    let scratch = fixture::scratch(&format!("portable_{}", Uuid::new_v4().simple()))
        .await
        .unwrap();
    let db = &scratch.db;
    for component in ["api", "worker", "maintenance"] {
        scope_runtime::renew(db, Uuid::new_v4(), component).await.unwrap();
    }
    db.execute_unprepared("UPDATE flow_runtime_capabilities SET minimum_reader_version=5,personal_provisioning=true,personal_writes=true,portable_page_packages_enabled=true WHERE singleton").await.unwrap();
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
    let (target, _) = *pages.get(1).expect("fixture has item 2");
    let mut engine = document::load_head_engine(db, source_doc).await.unwrap();
    let base = engine.frontier();
    let head_token = bootstrap::load(db, source_doc).await.unwrap().head_frontier;
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
    let (linked, _) = *pages.get(2).expect("fixture has item 3");
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
    let image_bytes = b"\x89PNG\r\n\x1a\nportable-image-test";
    let image = api::flow::body_assets::upload(
        db,
        source,
        owner,
        api::flow::body_assets::validate_bytes(
            api::flow::body_assets::AssetKind::Image,
            "fixture.png",
            Some("image/png"),
            image_bytes.to_vec(),
            1024,
        )
        .unwrap(),
        "upload-image",
    )
    .await
    .unwrap()
    .asset_id;
    engine
        .insert_page_atom(
            "media-image",
            None,
            2,
            "image",
            &serde_json::json!({"asset_id":image,"alt":"Image alt","caption":"Image caption","indent":0}),
        )
        .unwrap();
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
    let result = scope_write::accept(
        db,
        source,
        engine.export_from(&base).unwrap(),
        scope_write::UpdateIntent {
            user: owner,
            update: Uuid::new_v4(),
            client: "transfer-fixture",
            key: None,
            expected: Some(&head_token),
            command_body: None,
        },
    )
    .await
    .unwrap();
    assert!(matches!(result, api::flow::collab::write::AcceptOutcome::Accepted(_)));
    let count = async || {
        let mut totals = Vec::new();
        for table in [
            "flow_objects",
            "collab_documents",
            "flow_page_assets",
            "flow_page_asset_refs",
            "flow_page_child_refs",
            "flow_scope_command_receipts",
            "business_events",
        ] {
            let row = db
                .query_one(Statement::from_string(
                    DbBackend::Postgres,
                    format!("SELECT count(*) AS count FROM {table}"),
                ))
                .await
                .unwrap()
                .unwrap();
            totals.push(row.try_get::<i64>("", "count").unwrap());
        }
        totals
    };
    let previous = count().await;
    let bytes = package::export(db, source, owner).await.unwrap();
    let mut zip = zip::ZipArchive::new(std::io::Cursor::new(&bytes)).unwrap();
    assert!(zip.by_name("history/events.jsonl").is_err());
    assert!(zip.by_name(&format!("assets/{image}.bin")).is_ok());
    let review = package::preview(db, target, owner, &bytes, false).await.unwrap();
    assert_eq!(
        *review.pointer("/title").expect("response has /title"),
        "SOURCE_PRIVATE_TITLE"
    );
    assert_eq!(*review.pointer("/asset_count").expect("response has /asset_count"), 2);
    assert_eq!(
        *review
            .pointer("/linked_page_count")
            .expect("response has /linked_page_count"),
        2
    );
    let browser_package = std::fs::read(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/flow-v2/browser-package-v4/package.zip"
    ))
    .unwrap();
    assert_eq!(
        api::flow::package::verify_package(std::io::Cursor::new(&browser_package), None)
            .unwrap()
            .manifest
            .schema,
        api::flow::package::PACKAGE_SCHEMA
    );
    assert!(
        package::preview(db, target, owner, &browser_package, false)
            .await
            .is_err()
    );

    assert_eq!(
        *review.pointer("/blocks/0/text").expect("response has /blocks/0/text"),
        "Selected 中文😀"
    );
    assert_eq!(package::preview(db, source, owner, &bytes, true).await.unwrap(), review);
    assert!(package::preview(db, target, owner, &bytes, true).await.is_err());
    assert!(
        package::preview(db, target, Uuid::new_v4(), &bytes, false)
            .await
            .is_err()
    );
    assert_eq!(count().await, previous);
    fixture::exec(
        db,
        "UPDATE flow_scope_settings SET writes_enabled=false WHERE scope_id=$1",
        vec![home.0.id.into()],
    )
    .await;
    assert!(package::export(db, source, owner).await.is_ok());
    assert!(package::preview(db, source, owner, &bytes, true).await.is_ok());
    assert!(package::preview(db, target, owner, &bytes, false).await.is_err());
    fixture::exec(
        db,
        "UPDATE flow_scope_settings SET writes_enabled=true WHERE scope_id=$1",
        vec![home.0.id.into()],
    )
    .await;
    assert_eq!(count().await, previous);
    let mut corrupted = bytes.clone();
    let index = corrupted.len() / 2;
    *corrupted.get_mut(index).expect("corruption offset is in the fixture") ^= 1;
    let hash = review
        .pointer("/package_sha256")
        .expect("response has /package_sha256")
        .as_str()
        .unwrap();
    assert!(package::preview(db, target, owner, &corrupted, false).await.is_err());
    assert!(
        package::import(db, target, owner, &corrupted, hash, "changed")
            .await
            .is_err()
    );
    assert_eq!(count().await, previous);

    // Inject a database failure after canonical Page, snapshot, projection, assets and refs
    // have been written. The ordinary endpoint transaction must roll them all back.
    db.execute_unprepared(
        "CREATE FUNCTION reject_package_probe_event() RETURNS trigger LANGUAGE plpgsql AS $$
         BEGIN RAISE EXCEPTION 'package promotion probe'; END; $$;
         CREATE TRIGGER reject_package_probe_event BEFORE INSERT ON business_events
         FOR EACH ROW WHEN (NEW.idempotency_key = 'rollback-package')
         EXECUTE FUNCTION reject_package_probe_event()",
    )
    .await
    .unwrap();
    assert!(
        package::import(db, target, owner, &bytes, hash, "rollback-package")
            .await
            .is_err()
    );
    assert_eq!(count().await, previous);
    db.execute_unprepared("DROP TRIGGER reject_package_probe_event ON business_events")
        .await
        .unwrap();

    let (a, b, c) = tokio::join!(
        package::import(db, target, owner, &bytes, hash, "same-package"),
        package::import(db, target, owner, &bytes, hash, "same-package"),
        package::import(db, target, owner, &bytes, hash, "same-package"),
    );
    let receipt = a.unwrap();
    assert_eq!(b.unwrap(), receipt);
    assert_eq!(c.unwrap(), receipt);
    let imported = Uuid::parse_str(
        receipt
            .pointer("/object_id")
            .expect("response has /object_id")
            .as_str()
            .unwrap(),
    )
    .unwrap();
    let imported_doc = Uuid::parse_str(
        receipt
            .pointer("/document_id")
            .expect("response has /document_id")
            .as_str()
            .unwrap(),
    )
    .unwrap();
    assert_ne!((imported, imported_doc), (source, source_doc));
    assert_eq!(scope::identity(db, imported).await.unwrap().parent_id, Some(target));
    assert_eq!(active_child_refs(imported).await, expected_child_refs);
    assert_eq!(scope::identity(db, linked).await.unwrap().parent_id, link_parent);
    assert_eq!(scope::identity(db, created).await.unwrap().parent_id, Some(source));

    let assets = db
        .query_all(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT id,kind,bytes,state FROM flow_page_assets WHERE object_id=$1 ORDER BY kind",
            vec![imported.into()],
        ))
        .await
        .unwrap();
    assert_eq!(assets.len(), 2);
    for (asset, (old_id, expected_bytes)) in assets
        .iter()
        .zip([(attachment, attachment_bytes), (image, image_bytes.to_vec())])
    {
        let id = asset.try_get::<Uuid>("", "id").unwrap();
        assert_ne!(id, old_id);
        assert_eq!(asset.try_get::<Vec<u8>>("", "bytes").unwrap(), expected_bytes);
        assert_eq!(asset.try_get::<String>("", "state").unwrap(), "active");
        assert_eq!(
            api::flow::body_assets::read(db, imported, id, owner)
                .await
                .unwrap()
                .bytes,
            expected_bytes
        );
        assert!(api::flow::body_assets::read(db, source, id, owner).await.is_err());
    }
    let new = document::load_head_engine(db, imported_doc).await.unwrap();
    let snapshot = new.semantic_snapshot().unwrap();
    let blocks = api::flow::page_transfer_ir::read(&snapshot).unwrap();
    assert_eq!(blocks.len(), 8);
    assert_eq!(blocks.first().expect("fixture has item 1").text, "Selected 中文😀");
    assert_eq!(blocks.first().expect("fixture has item 1").spans.len(), 1);
    assert_eq!(
        blocks.get(1).expect("fixture has item 2").parent_cid.as_deref(),
        Some(blocks.first().expect("fixture has item 1").source_cid.as_str())
    );
    assert_eq!(blocks.get(6).expect("fixture has item 7").text, "PRIVATE_SIBLING");
    let rows = count().await;
    assert_eq!(
        *rows.first().expect("fixture has item 1") - *previous.first().expect("fixture has item 1"),
        1
    );
    assert_eq!(
        *rows.get(2).expect("fixture has item 3") - *previous.get(2).expect("fixture has item 3"),
        2
    );
    assert_eq!(
        *rows.get(3).expect("fixture has item 4") - *previous.get(3).expect("fixture has item 4"),
        3
    );
    assert_eq!(
        *rows.get(4).expect("fixture has item 5") - *previous.get(4).expect("fixture has item 5"),
        2
    );
    assert_eq!(
        *rows.get(5).expect("fixture has item 6") - *previous.get(5).expect("fixture has item 6"),
        1
    );
    assert_eq!(
        *rows.get(6).expect("fixture has item 7") - *previous.get(6).expect("fixture has item 7"),
        1
    );
    db.execute_unprepared("UPDATE flow_runtime_capabilities SET portable_graph_packages_enabled=true")
        .await
        .unwrap();
    let select = |page_ids| {
        serde_json::from_value(serde_json::json!({
            "page_ids":page_ids,"form_ids":[],"include_history":true,
        }))
        .unwrap()
    };
    assert!(graph::export(db, owner, source, select(vec![source])).await.is_err());
    for offset in 0..40 {
        let mut typing = document::load_head_engine(db, source_doc).await.unwrap();
        let base = typing.frontier();
        let head_token = bootstrap::load(db, source_doc).await.unwrap().head_frontier;
        typing
            .apply_content_operation(&Operation::InsertText {
                id: root.clone(),
                index: offset,
                text: "t".into(),
            })
            .unwrap();
        let accepted = scope_write::accept(
            db,
            source,
            typing.export_from(&base).unwrap(),
            scope_write::UpdateIntent {
                user: owner,
                update: Uuid::new_v4(),
                client: "timeline-typing",
                key: None,
                expected: Some(&head_token),
                command_body: None,
            },
        )
        .await
        .unwrap();
        assert!(matches!(accepted, api::flow::collab::write::AcceptOutcome::Accepted(_)));
    }
    let mut engine = document::load_head_engine(db, source_doc).await.unwrap();
    let base = engine.frontier();
    let head_token = bootstrap::load(db, source_doc).await.unwrap().head_frontier;
    let image_cid = engine.resolve_content_node_id(&"media-image".into()).unwrap();
    engine
        .apply_content_operation(&Operation::DeleteNode { id: image_cid })
        .unwrap();
    engine.set_title("Head after removing image").unwrap();
    assert!(matches!(
        scope_write::accept(
            db,
            source,
            engine.export_from(&base).unwrap(),
            scope_write::UpdateIntent {
                user: owner,
                update: Uuid::new_v4(),
                client: "test",
                key: None,
                expected: Some(&head_token),
                command_body: None
            },
        )
        .await
        .unwrap(),
        api::flow::collab::write::AcceptOutcome::Accepted(_)
    ));

    let graph_zip = graph::export(db, owner, source, select(vec![source, linked, created]))
        .await
        .unwrap();
    let review = graph::preview(db, owner, target, &graph_zip, false).await.unwrap();
    assert_eq!(
        review
            .pointer("/pages")
            .expect("response has /pages")
            .as_array()
            .unwrap()
            .len(),
        3
    );
    assert_eq!(
        *review
            .pointer("/include_history")
            .expect("response has /include_history"),
        true
    );
    let parent_review = review
        .pointer("/pages")
        .expect("response has /pages")
        .as_array()
        .unwrap()
        .iter()
        .find(|entry| entry["source_object_id"] == source.to_string())
        .unwrap();
    assert!(
        parent_review
            .pointer("/excerpts/0")
            .expect("response has /excerpts/0")
            .as_str()
            .unwrap()
            .contains("Selected 中文😀")
    );
    assert_eq!(
        review
            .pointer("/references")
            .expect("response has /references")
            .as_array()
            .unwrap()
            .len(),
        2
    );
    let digest = review
        .pointer("/package_sha256")
        .expect("response has /package_sha256")
        .as_str()
        .unwrap();
    let (left, right) = tokio::join!(
        graph::import(db, owner, target, &graph_zip, digest, "same-graph"),
        graph::import(db, owner, target, &graph_zip, digest, "same-graph"),
    );
    let receipt = left.unwrap();
    assert_eq!(right.unwrap(), receipt);
    let mapped = |source: Uuid| {
        let row = receipt
            .pointer("/pages")
            .expect("response has /pages")
            .as_array()
            .unwrap()
            .iter()
            .find(|row| row["source_object_id"] == source.to_string())
            .unwrap();
        (Uuid::parse_str(row["object_id"].as_str().unwrap()).unwrap(), row)
    };
    let (new_source, source_row) = mapped(source);
    let (new_linked, _) = mapped(linked);
    let (new_child, _) = mapped(created);
    assert_ne!(new_source, source);
    assert_ne!(new_linked, linked);
    assert_ne!(new_child, created);
    assert_eq!(
        scope::identity(db, new_child).await.unwrap().parent_id,
        Some(new_source)
    );
    assert_eq!(
        active_child_refs(new_source).await,
        std::collections::BTreeSet::from([new_child, new_linked])
    );
    let state = platform::app::AppState {
        db: db.clone(),
        cfg: platform::config::AppConfig {
            app_name: "portable-graph-gate".into(),
            bind_addr: "127.0.0.1:0".into(),
            database_url: platform::config::Secret::new("postgres://fixture/fixture"),
            jwt_secret: platform::config::Secret::new("portable-graph-gate"),
            jwt_access_ttl_seconds: 900,
            jwt_refresh_ttl_seconds: 3600,
            allow_insecure_cookies: false,
            collab_allowed_origins: vec![],
        },
        flow_permission_cache: platform::app::FlowPermissionCacheSlot::default(),
    };
    let versions = source_row["versions"].as_array().unwrap();
    assert_eq!(versions.len(), 43);
    let image_version = versions
        .iter()
        .rev()
        .nth(1)
        .and_then(|version| version.get("local_seq"))
        .expect("penultimate version has a local sequence")
        .as_i64()
        .unwrap();
    let asset = db
        .query_one(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT id,state,bytes FROM flow_page_assets WHERE object_id=$1 AND kind='image'",
            vec![new_source.into()],
        ))
        .await
        .unwrap()
        .unwrap();
    let image_copy = asset.try_get::<Uuid>("", "id").unwrap();
    assert_ne!(image_copy, image);
    assert_eq!(asset.try_get::<String>("", "state").unwrap(), "retained");
    assert_eq!(asset.try_get::<Vec<u8>>("", "bytes").unwrap(), image_bytes);
    let historical = api::flow::scope_history::object_at(&state, new_source, owner, image_version)
        .await
        .unwrap();
    assert_eq!(
        *historical.pointer("/title").expect("response has /title"),
        "SOURCE_PRIVATE_TITLE"
    );
    let initial = api::flow::scope_history::object_at(&state, new_source, owner, 1)
        .await
        .unwrap();
    let initial_nodes = initial
        .pointer("/semantic_content/nodes")
        .expect("response has /semantic_content/nodes")
        .as_object()
        .unwrap();
    let historical_nodes = historical
        .pointer("/semantic_content/nodes")
        .expect("response has /semantic_content/nodes")
        .as_object()
        .unwrap();
    let initial_cid = initial_nodes
        .iter()
        .find(|(_, node)| node["text"] == "Selected 中文😀")
        .unwrap()
        .0;
    assert!(
        historical_nodes
            .get(initial_cid)
            .and_then(|node| node.get("text"))
            .expect("historical node has text")
            .as_str()
            .unwrap()
            .starts_with(&"t".repeat(40))
    );

    assert!(
        api::flow::scope_history::object_at(&state, new_source, Uuid::new_v4(), image_version)
            .await
            .is_err()
    );
    api::flow::scope_history::restore_page(&state, new_source, owner, image_version, "graph-restore", None)
        .await
        .unwrap();
    assert_eq!(
        api::flow::body_assets::read(db, new_source, image_copy, owner)
            .await
            .unwrap()
            .bytes,
        image_bytes
    );
    assert_eq!(
        active_child_refs(new_source).await,
        std::collections::BTreeSet::from([new_child, new_linked])
    );
    drop(state);
    scratch.drop_self().await;
}
