//! Same production-service/router assertions as the unit fixture, compiled independently to
//! keep the Linux compiler peak below 4 GiB. No authorization/isolation fallback exists here.
use api::flow::block_shares::*;
use sea_orm::{ConnectionTrait, DbBackend, Statement};
use uuid::Uuid;
#[path = "fixtures/page_service_postgres.rs"]
mod fixture;
#[cfg(test)]
mod sql_tests {
    use super::*;
    use crate::fixture::{self as tenant_fixture, exec};
    use base64::Engine as _;
    use collab_core::{CollabEngine, NodeKind, Operation};
    use sea_orm::TransactionTrait;
    #[tokio::test]
    #[allow(
        clippy::print_stderr,
        reason = "report the database skip without a tracing subscriber"
    )]
    async fn source_create_receipt_and_recipient_projection_are_independent_of_private_page() {
        let Some(scratch) = tenant_fixture::scratch(&format!("block_share_{}", Uuid::new_v4().simple())).await else {
            eprintln!("SKIPPED: disposable database required");
            return;
        };
        let db = &scratch.db;
        for component in ["api", "worker", "maintenance"] {
            api::flow::scope_runtime::renew(db, Uuid::new_v4(), component)
                .await
                .unwrap();
        }
        db.execute_unprepared("UPDATE flow_runtime_capabilities SET minimum_reader_version=5,personal_provisioning=true,personal_writes=true WHERE singleton").await.unwrap();
        let owner = Uuid::new_v4();
        let recipient = Uuid::new_v4();
        for actor in [owner, recipient] {
            exec(db,"INSERT INTO users(id,email,password_hash,name,role,is_active) VALUES($1,$2,'!','Share fixture','user',true)",vec![actor.into(),format!("{actor}@block-share.test").into()]).await;
        }
        let workspace = Uuid::new_v4();
        let project = Uuid::new_v4();
        exec(
            db,
            "INSERT INTO workspaces(id,slug,name,created_by) VALUES($1,$2,'Share destination',$3)",
            vec![workspace.into(), format!("share-{workspace}").into(), owner.into()],
        )
        .await;
        exec(
            db,
            "INSERT INTO workspace_members(workspace_id,user_id,role) VALUES($1,$2,'owner'),($1,$3,'member')",
            vec![workspace.into(), owner.into(), recipient.into()],
        )
        .await;
        exec(
            db,
            "INSERT INTO projects(id,workspace_id,key,name,created_by) VALUES($1,$2,'SHARE','Share project',$3)",
            vec![project.into(), workspace.into(), owner.into()],
        )
        .await;
        let home = api::flow::scope::personal_home(db, owner).await.unwrap();
        let created = api::flow::scope::create_personal(
            db,
            owner,
            api::flow::scope::CreateInput {
                editor_model: 5,
                scope_id: home.0.id,
                object_type: "page".into(),
                parent_object_id: None,
                project_id: None,
                title: "PRIVATE_PAGE_TITLE_SENTINEL".into(),
                idempotency_key: Uuid::new_v4().to_string(),
                message: None,
                initial_fields: vec![],
                initial_view: None,
            },
        )
        .await
        .unwrap();
        let page = Uuid::parse_str(
            created
                .pointer("/object/id")
                .expect("response has /object/id")
                .as_str()
                .unwrap(),
        )
        .unwrap();
        let document = Uuid::parse_str(
            created
                .pointer("/object/document_id")
                .expect("response has /object/document_id")
                .as_str()
                .unwrap(),
        )
        .unwrap();
        let mut engine = api::flow::document::load_head_engine(db, document).await.unwrap();
        let root = engine.semantic_snapshot().unwrap().nodes.keys().next().unwrap().clone();
        engine
            .apply_content_operation(&Operation::InsertText {
                id: root.clone(),
                index: 0,
                text: "Selected 中文😀".into(),
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
                text: "PRIVATE_SIBLING_SENTINEL".into(),
            })
            .unwrap();
        let attachment_bytes = br#"{"payload":"allowed"}"#.to_vec();
        let image_bytes = vec![0x89, b'P', b'N', b'G', 13, 10, 26, 10];
        let attachment = api::flow::body_assets::upload(
            db,
            page,
            owner,
            api::flow::body_assets::validate_bytes(
                api::flow::body_assets::AssetKind::Attachment,
                "PRIVATE_STORAGE_SENTINEL.json",
                Some("application/json"),
                attachment_bytes.clone(),
                50 * 1024 * 1024,
            )
            .unwrap(),
            &Uuid::new_v4().to_string(),
        )
        .await
        .unwrap();
        let image = api::flow::body_assets::upload(
            db,
            page,
            owner,
            api::flow::body_assets::validate_bytes(
                api::flow::body_assets::AssetKind::Image,
                "PRIVATE_IMAGE_SENTINEL.png",
                None,
                image_bytes.clone(),
                50 * 1024 * 1024,
            )
            .unwrap(),
            &Uuid::new_v4().to_string(),
        )
        .await
        .unwrap();
        let attachment_cid=engine.insert_page_atom("attachment-fixture",None,2,"attachment",&serde_json::json!({"asset_id":attachment.asset_id,"name":"Allowed public.json","mime":"application/json","caption":"Visible caption","indent":0})).unwrap();
        let image_cid=engine.insert_page_atom("image-fixture",None,3,"image",&serde_json::json!({"asset_id":image.asset_id,"alt":"Allowed image","caption":"Visible image","indent":0})).unwrap();
        // Fixture setup only: install a complete initial snapshot, not an accepted-write substitute.
        exec(
            db,
            "UPDATE collab_documents SET snapshot=$2,snapshot_checksum=encode(digest($2,'sha256'),'hex'),byte_count=octet_length($2),snapshot_frontier=$3,head_frontier=$3 WHERE id=$1",
            vec![
                document.into(),
                engine.export_snapshot().unwrap().into(),
                engine.frontier().as_bytes().to_vec().into(),
            ],
        )
        .await;
        let refs = api::flow::body_assets::typed_references(&engine.semantic_snapshot().unwrap()).unwrap();
        let asset_tx = db.begin().await.unwrap();
        api::flow::body_assets::lock_and_sync_references(&asset_tx, home.0.id, page, 0, &refs)
            .await
            .unwrap();
        asset_tx.commit().await.unwrap();
        let private_comment = api::flow::scope_social::create_comment(
            db,
            page,
            owner,
            api::flow::comments::NewComment {
                body: "PRIVATE_COMMENT_SENTINEL".into(),
                block_id: Some(root.to_string()),
                parent_id: None,
                mentions: vec![],
            },
            Some(&Uuid::new_v4().to_string()),
        )
        .await
        .unwrap();
        assert_eq!(private_comment.anchor_status.as_deref(), Some("bound"));
        let key = Uuid::new_v4().to_string();
        let input = CreateInput {
            expected_frontier: base64::engine::general_purpose::STANDARD.encode(engine.frontier().as_bytes()),
            root_block_cid: root.to_string(),
            selection: Selection::Block,
            audiences: vec![AudienceInput {
                workspace_id: workspace,
                project_id: Some(project),
                audience: AudienceKind::Users,
                recipient_user_ids: vec![recipient],
                permission: SharePermission::Edit,
            }],
        };
        let view = create(db, owner, page, &key, input.clone()).await.unwrap();
        let replay = create(db, owner, page, &key, input.clone()).await.unwrap();
        assert_eq!(view.share_id, replay.share_id);
        let recipient_view = read(db, recipient, view.share_id).await.unwrap();
        assert_eq!(recipient_view.effective_permission, SharePermission::Edit);
        assert_eq!(recipient_view.projection.members.len(), 1);
        let encoded = serde_json::to_string(&recipient_view).unwrap();
        assert!(encoded.contains("Selected 中文😀"));
        for secret in [
            page.to_string(),
            document.to_string(),
            root.to_string(),
            "PRIVATE_PAGE_TITLE_SENTINEL".into(),
            "PRIVATE_SIBLING_SENTINEL".into(),
            "PRIVATE_COMMENT_SENTINEL".into(),
        ] {
            assert!(!encoded.contains(&secret));
        }
        assert!(
            api::flow::scope::authorize(db, page, recipient, api::flow::collab::authz::PermissionLevel::View)
                .await
                .is_err()
        );
        std::fs::write(
            "/tmp/sv-shared-projection.actual-db.fixture.json",
            serde_json::to_string_pretty(&recipient_view).unwrap(),
        )
        .unwrap();
        #[cfg(target_os = "linux")]
        verify_linux_patch(
            db,
            owner,
            recipient,
            page,
            document,
            view.share_id,
            root.to_string(),
            key.clone(),
            input.clone(),
        )
        .await;
        verify_management(
            db,
            owner,
            recipient,
            page,
            document,
            view.share_id,
            workspace,
            project,
            input,
        )
        .await;
        verify_media_alias_http(
            db,
            owner,
            recipient,
            page,
            document,
            workspace,
            attachment_cid,
            image_cid,
            attachment_bytes,
            image_bytes,
        )
        .await;
        scratch.drop_self().await;
    }

    #[allow(clippy::too_many_arguments, reason = "keep test actors and fixture IDs explicit")]
    async fn verify_management(
        db: &sea_orm::DatabaseConnection,
        owner: Uuid,
        recipient: Uuid,
        page: Uuid,
        document: Uuid,
        share: Uuid,
        workspace: Uuid,
        project: Uuid,
        mut input: CreateInput,
    ) {
        let body_before = api::flow::document::load_head_engine(db, document)
            .await
            .unwrap()
            .semantic_snapshot()
            .unwrap()
            .semantic_hash();
        let listed = list_source(db, owner, page).await.unwrap();
        std::fs::write(
            "/tmp/sv-source-block-shares.fixture.json",
            serde_json::to_string_pretty(&listed).unwrap(),
        )
        .unwrap();
        let target_metadata = list_target_placements(db, owner, workspace, Some(project), None)
            .await
            .unwrap();
        assert_eq!(target_metadata.len(), 1);
        assert!(!target_metadata.first().unwrap().can_read);
        assert!(read(db, owner, share).await.is_err());
        assert!(discover(db, owner, workspace, Some(project)).await.unwrap().is_empty());
        let metadata_json = serde_json::to_string(&target_metadata).unwrap();
        assert!(!metadata_json.contains(&page.to_string()));
        assert!(!metadata_json.contains(&document.to_string()));
        assert!(!metadata_json.contains("PRIVATE_"));
        assert!(!metadata_json.contains("root_block_cid"));
        assert!(!metadata_json.contains("projection"));
        assert_eq!(listed.len(), 1);
        assert_eq!(listed.first().unwrap().root_block_cid, input.root_block_cid);
        assert_eq!(
            suspend(db, owner, page, share, &Uuid::new_v4().to_string())
                .await
                .unwrap()
                .state,
            "suspended"
        );
        assert!(read(db, recipient, share).await.is_err());
        input.expected_frontier = base64::engine::general_purpose::STANDARD.encode(
            api::flow::document::load_head_engine(db, document)
                .await
                .unwrap()
                .frontier()
                .as_bytes(),
        );
        republish(db, owner, page, share, &Uuid::new_v4().to_string(), input.clone())
            .await
            .unwrap();
        assert!(read(db, recipient, share).await.is_ok());
        // No GET between archive and restore: the durable trigger must prevent resurrection.
        exec(
            db,
            "UPDATE flow_objects SET lifecycle_status='archived',archived_at=now() WHERE id=$1",
            vec![page.into()],
        )
        .await;
        exec(
            db,
            "UPDATE flow_objects SET lifecycle_status='active',archived_at=NULL WHERE id=$1",
            vec![page.into()],
        )
        .await;
        let state = db
            .query_one(Statement::from_sql_and_values(
                DbBackend::Postgres,
                "SELECT state FROM flow_block_shares WHERE id=$1",
                vec![share.into()],
            ))
            .await
            .unwrap()
            .unwrap()
            .try_get::<String>("", "state")
            .unwrap();
        assert_eq!(state, "suspended");
        assert!(read(db, recipient, share).await.is_err());
        republish(db, owner, page, share, &Uuid::new_v4().to_string(), input.clone())
            .await
            .unwrap();
        let scope = api::flow::scope::identity(db, page).await.unwrap().scope_id;
        exec(
            db,
            "UPDATE flow_scopes SET lifecycle_status='archived' WHERE id=$1",
            vec![scope.into()],
        )
        .await;
        exec(
            db,
            "UPDATE flow_scopes SET lifecycle_status='active' WHERE id=$1",
            vec![scope.into()],
        )
        .await;
        assert!(read(db, recipient, share).await.is_err());
        republish(db, owner, page, share, &Uuid::new_v4().to_string(), input.clone())
            .await
            .unwrap();
        let second = create(db, owner, page, &Uuid::new_v4().to_string(), input.clone())
            .await
            .unwrap();
        let placement = read(db, recipient, share).await.unwrap().display_context.placement_id;
        remove_placement(db, owner, share, placement, &Uuid::new_v4().to_string())
            .await
            .unwrap();
        assert!(read(db, recipient, share).await.is_err());
        assert!(read(db, recipient, second.share_id).await.is_ok());
        // Losing destination access cannot prevent a source controller stopping delegation.
        exec(
            db,
            "DELETE FROM workspace_members WHERE workspace_id=$1 AND user_id=$2",
            vec![workspace.into(), owner.into()],
        )
        .await;
        assert_eq!(
            suspend(db, owner, page, second.share_id, &Uuid::new_v4().to_string())
                .await
                .unwrap()
                .state,
            "suspended"
        );
        exec(
            db,
            "INSERT INTO workspace_members(workspace_id,user_id,role) VALUES($1,$2,'owner')",
            vec![workspace.into(), owner.into()],
        )
        .await;
        republish(
            db,
            owner,
            page,
            second.share_id,
            &Uuid::new_v4().to_string(),
            input.clone(),
        )
        .await
        .unwrap();
        exec(db, "DELETE FROM projects WHERE id=$1", vec![project.into()]).await;
        let state = db
            .query_one(Statement::from_sql_and_values(
                DbBackend::Postgres,
                "SELECT state FROM flow_block_shares WHERE id=$1",
                vec![second.share_id.into()],
            ))
            .await
            .unwrap()
            .unwrap()
            .try_get::<String>("", "state")
            .unwrap();
        assert_eq!(state, "target_removed");
        assert!(read(db, recipient, second.share_id).await.is_err());
        assert_eq!(
            api::flow::document::load_head_engine(db, document)
                .await
                .unwrap()
                .semantic_snapshot()
                .unwrap()
                .semantic_hash(),
            body_before
        );
        let target = Uuid::new_v4();
        exec(
            db,
            "INSERT INTO workspaces(id,slug,name,created_by) VALUES($1,$2,'Second target',$3)",
            vec![target.into(), format!("share-{target}").into(), owner.into()],
        )
        .await;
        exec(
            db,
            "INSERT INTO workspace_members(workspace_id,user_id,role) VALUES($1,$2,'owner'),($1,$3,'member')",
            vec![target.into(), owner.into(), recipient.into()],
        )
        .await;
        input.audiences = vec![AudienceInput {
            workspace_id: target,
            project_id: None,
            audience: AudienceKind::Workspace,
            recipient_user_ids: vec![],
            permission: SharePermission::View,
        }];
        let third = create(db, owner, page, &Uuid::new_v4().to_string(), input)
            .await
            .unwrap();
        assert!(read(db, recipient, third.share_id).await.is_ok());
        exec(db, "DELETE FROM workspaces WHERE id=$1", vec![target.into()]).await;
        assert!(read(db, recipient, third.share_id).await.is_err());
        assert_eq!(
            api::flow::document::load_head_engine(db, document)
                .await
                .unwrap()
                .semantic_snapshot()
                .unwrap()
                .semantic_hash(),
            body_before
        );
    }

    #[allow(clippy::too_many_arguments, reason = "keep test actors and fixture IDs explicit")]
    async fn verify_media_alias_http(
        db: &sea_orm::DatabaseConnection,
        owner: Uuid,
        recipient: Uuid,
        page: Uuid,
        document: Uuid,
        workspace: Uuid,
        attachment_cid: String,
        image_cid: String,
        attachment_bytes: Vec<u8>,
        image_bytes: Vec<u8>,
    ) {
        use axum::{
            Extension, Router,
            body::{Body, to_bytes},
            http::Request,
            routing::get,
        };
        use collab_core::block_projection::SharedContent;
        use tower::ServiceExt;
        let state = platform::app::AppState {
            db: db.clone(),
            cfg: platform::config::AppConfig {
                app_name: "share-http-unit".into(),
                bind_addr: "127.0.0.1:0".into(),
                database_url: platform::config::Secret::new("postgres://unused/unused"),
                jwt_secret: platform::config::Secret::new("share-synthetic-test-only"),
                jwt_access_ttl_seconds: 900,
                jwt_refresh_ttl_seconds: 3600,
                default_author_id: None,
                allow_insecure_cookies: false,
                collab_allowed_origins: vec![],
            },
            flow_permission_cache: platform::app::FlowPermissionCacheSlot::default(),
        };
        let claims = platform::auth::JwtClaims {
            sub: recipient.to_string(),
            email: format!("{recipient}@block-share.test"),
            token_type: platform::auth::TokenType::Access,
            iat: 0,
            exp: usize::MAX,
            sv: 0,
        };
        #[allow(clippy::literal_string_with_formatting_args, reason = "Axum path placeholders")]
        let app = Router::new()
            .route(
                "/shares/{share}/assets/{alias}",
                get(api::routes::flow::get_flow_v2_block_share_asset),
            )
            .layer(Extension(claims))
            .with_state(state);
        let frontier = base64::engine::general_purpose::STANDARD.encode(
            api::flow::document::load_head_engine(db, document)
                .await
                .unwrap()
                .frontier()
                .as_bytes(),
        );
        for (cid, expected, image) in [
            (attachment_cid, attachment_bytes, false),
            (image_cid, image_bytes, true),
        ] {
            let input = CreateInput {
                expected_frontier: frontier.clone(),
                root_block_cid: cid,
                selection: Selection::Block,
                audiences: vec![AudienceInput {
                    workspace_id: workspace,
                    project_id: None,
                    audience: AudienceKind::Users,
                    recipient_user_ids: vec![recipient],
                    permission: SharePermission::View,
                }],
            };
            let share = create(db, owner, page, &Uuid::new_v4().to_string(), input)
                .await
                .unwrap();
            let projected = read(db, recipient, share.share_id).await.unwrap();
            let (SharedContent::Attachment { asset_alias: alias, .. }
            | SharedContent::Image { asset_alias: alias, .. }) = projected.projection.members.first().unwrap().content
            else {
                panic!("expected media");
            };
            let response = app
                .clone()
                .oneshot(
                    Request::builder()
                        .uri(format!("/shares/{}/assets/{alias}", share.share_id))
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap();
            assert_eq!(response.status(), axum::http::StatusCode::OK);
            assert_eq!(
                response.headers().get("x-flow-share-id").unwrap(),
                share.share_id.to_string().as_str()
            );
            assert_eq!(
                response.headers().get("x-flow-asset-alias").unwrap(),
                alias.to_string().as_str()
            );
            assert_eq!(response.headers().get("x-content-type-options").unwrap(), "nosniff");
            assert_eq!(response.headers().get("cache-control").unwrap(), "private, no-store");
            let disposition = response.headers().get("content-disposition").unwrap().to_str().unwrap();
            assert!(!disposition.contains("PRIVATE_"));
            if image {
                assert!(disposition.starts_with("inline;"));
                assert!(disposition.contains("shared-image.png"));
                assert_eq!(response.headers().get("content-type").unwrap(), "image/png");
            } else {
                assert!(disposition.starts_with("attachment;"));
                assert!(disposition.contains("Allowed%20public.json"));
                assert_eq!(
                    response.headers().get("content-type").unwrap(),
                    "application/octet-stream"
                );
            }
            assert!(
                response
                    .headers()
                    .iter()
                    .all(|(_, value)| !value.to_str().unwrap_or_default().contains(&page.to_string()))
            );
            assert_eq!(
                response.headers().get("x-flow-asset-sha256").unwrap(),
                api::flow::collab::bootstrap::content_hash(&expected).as_str()
            );
            assert_eq!(
                to_bytes(response.into_body(), 1024).await.unwrap().as_ref(),
                expected.as_slice()
            );
        }
    }

    #[cfg(target_os = "linux")]
    async fn verify_linux_patch(
        db: &sea_orm::DatabaseConnection,
        owner: Uuid,
        recipient: Uuid,
        page: Uuid,
        document: Uuid,
        share: Uuid,
        root: String,
        create_key: String,
        create_input: CreateInput,
    ) {
        use collab_core::block_projection::{SharedCommand, SharedPatch};
        let before = read(db, recipient, share).await.unwrap();
        let member = before.projection.members.first().unwrap().member_id;
        let key = Uuid::new_v4().to_string();
        let request = SharedPatch {
            expected_revision: before.projection.revision,
            commands: vec![SharedCommand::ReplaceText {
                member_id: member,
                text: "Recipient edited 中文😀".into(),
                spans: vec![],
            }],
        };
        let saved = patch(db, recipient, share, &key, request.clone()).await.unwrap();
        assert_eq!(saved.projection.revision, before.projection.revision + 1);
        assert_eq!(saved.receipt.accepted_revision, saved.projection.revision as i64);
        let engine = api::flow::document::load_head_engine(db, document).await.unwrap();
        let semantic = engine.semantic_snapshot().unwrap();
        assert_eq!(
            semantic.nodes.get(root.as_str()).unwrap().text,
            "Recipient edited 中文😀"
        );
        assert!(
            semantic
                .nodes
                .values()
                .any(|node| node.text == "PRIVATE_SIBLING_SENTINEL")
        );
        let audit = db
            .query_one(Statement::from_sql_and_values(
                DbBackend::Postgres,
                "SELECT actor_id FROM collab_updates WHERE document_id=$1 ORDER BY seq DESC LIMIT 1",
                vec![document.into()],
            ))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(audit.try_get::<Uuid>("", "actor_id").unwrap(), recipient);
        let head = db
            .query_one(Statement::from_sql_and_values(
                DbBackend::Postgres,
                "SELECT head_seq FROM collab_documents WHERE id=$1",
                vec![document.into()],
            ))
            .await
            .unwrap()
            .unwrap()
            .try_get::<i64>("", "head_seq")
            .unwrap();
        let stale = patch(db, recipient, share, &Uuid::new_v4().to_string(), request.clone())
            .await
            .unwrap_err();
        assert_eq!(stale.kind(), api::error::ApiErrorKind::StaleFrontier);
        let unchanged = db
            .query_one(Statement::from_sql_and_values(
                DbBackend::Postgres,
                "SELECT head_seq FROM collab_documents WHERE id=$1",
                vec![document.into()],
            ))
            .await
            .unwrap()
            .unwrap()
            .try_get::<i64>("", "head_seq")
            .unwrap();
        assert_eq!(head, unchanged);
        let replay = patch(db, recipient, share, &key, request.clone()).await.unwrap();
        assert!(replay.receipt.replayed);
        assert_eq!(replay.receipt.accepted_revision, saved.receipt.accepted_revision);
        // A normal private sibling edit advances only the source head, never the share revision.
        let mut engine = api::flow::document::load_head_engine(db, document).await.unwrap();
        let base = engine.frontier();
        let semantic = engine.semantic_snapshot().unwrap();
        let (private, node) = semantic
            .nodes
            .iter()
            .find(|(_, node)| node.text == "PRIVATE_SIBLING_SENTINEL")
            .unwrap();
        engine
            .apply_content_operation(&Operation::InsertText {
                id: private.clone(),
                index: node.text.len() as u32,
                text: " private activity".into(),
            })
            .unwrap();
        let bytes = engine.export_from(&base).unwrap();
        let accepted = api::flow::scope_write::accept(
            db,
            page,
            bytes,
            api::flow::scope_write::UpdateIntent {
                user: owner,
                update: Uuid::new_v4(),
                client: "private-fixture",
                key: None,
                expected: Some(base.as_bytes()),
                command_body: None,
            },
        )
        .await
        .unwrap();
        assert!(matches!(accepted, api::flow::collab::write::AcceptOutcome::Accepted(_)));
        let confirmed = create(db, owner, page, &create_key, create_input.clone())
            .await
            .unwrap();
        assert_eq!(confirmed.share_id, share);
        let republish_key = Uuid::new_v4().to_string();
        suspend(db, owner, page, share, &Uuid::new_v4().to_string())
            .await
            .unwrap();
        let mut reviewed = create_input.clone();
        reviewed.expected_frontier = base64::engine::general_purpose::STANDARD.encode(
            api::flow::document::load_head_engine(db, document)
                .await
                .unwrap()
                .frontier()
                .as_bytes(),
        );
        republish(db, owner, page, share, &republish_key, reviewed.clone())
            .await
            .unwrap();
        let mut private_engine = api::flow::document::load_head_engine(db, document).await.unwrap();
        let private_base = private_engine.frontier();
        let private_semantic = private_engine.semantic_snapshot().unwrap();
        let (private, node) = private_semantic
            .nodes
            .iter()
            .find(|(_, node)| node.text.starts_with("PRIVATE_SIBLING_SENTINEL"))
            .unwrap();
        private_engine
            .apply_content_operation(&Operation::InsertText {
                id: private.clone(),
                index: node.text.len() as u32,
                text: " confirm replay".into(),
            })
            .unwrap();
        let accepted = api::flow::scope_write::accept(
            db,
            page,
            private_engine.export_from(&private_base).unwrap(),
            api::flow::scope_write::UpdateIntent {
                user: owner,
                update: Uuid::new_v4(),
                client: "private-replay-fixture",
                key: None,
                expected: Some(private_base.as_bytes()),
                command_body: None,
            },
        )
        .await
        .unwrap();
        assert!(matches!(accepted, api::flow::collab::write::AcceptOutcome::Accepted(_)));
        let confirmed = republish(db, owner, page, share, &republish_key, reviewed)
            .await
            .unwrap();
        assert_eq!(confirmed.share_id, share);
        let after_private = read(db, recipient, share).await.unwrap();
        assert_eq!(
            serde_json::to_value(&after_private.projection).unwrap(),
            serde_json::to_value(&saved.projection).unwrap()
        );
        let replay = patch(db, recipient, share, &key, request).await.unwrap();
        assert!(replay.receipt.replayed);
        assert_eq!(replay.receipt.accepted_revision, saved.receipt.accepted_revision);
        assert_eq!(replay.projection.revision, saved.projection.revision);
        // This is an isolated new-module fixture, not a retry of held legacy grant scenarios.
        exec(
            db,
            "UPDATE flow_block_share_audiences SET permission='view' WHERE share_id=$1",
            vec![share.into()],
        )
        .await;
        exec(
            db,
            "UPDATE flow_block_shares SET auth_revision=auth_revision+1 WHERE id=$1",
            vec![share.into()],
        )
        .await;
        let forbidden = SharedPatch {
            expected_revision: after_private.projection.revision,
            commands: vec![SharedCommand::ReplaceText {
                member_id: member,
                text: "must not apply".into(),
                spans: vec![],
            }],
        };
        assert!(
            patch(db, recipient, share, &Uuid::new_v4().to_string(), forbidden)
                .await
                .is_err()
        );
        assert_eq!(
            api::flow::document::load_head_engine(db, document)
                .await
                .unwrap()
                .semantic_snapshot()
                .unwrap()
                .nodes
                .get(root.as_str())
                .unwrap()
                .text,
            "Recipient edited 中文😀"
        );
        assert!(
            api::flow::scope::authorize(
                db,
                page,
                recipient,
                api::flow::collab::authz::PermissionLevel::FullAccess
            )
            .await
            .is_err()
        );
    }
}
