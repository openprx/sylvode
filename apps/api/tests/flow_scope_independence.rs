//! Real `PostgreSQL` scope and migration gates. This target deliberately requires an explicitly
//! disposable superuser maintenance URL; absence is a failure, never a passing skip.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
use api::flow::{collab::authz::PermissionLevel, scope, scope_collab};
use collab_core::CollabEngine as _;
// Required fixture fields fail explicitly; absence/null expectations use get() separately.
trait FixtureField {
    fn field(&self, key: &str) -> &serde_json::Value;
}
impl FixtureField for serde_json::Value {
    fn field(&self, key: &str) -> &serde_json::Value {
        self.get(key).expect("required fixture field")
    }
}
use sea_orm::{ConnectionTrait, Database, DatabaseConnection, DbBackend, Statement, TransactionTrait};
use uuid::Uuid;

struct Scratch {
    db: DatabaseConnection,
    admin: DatabaseConnection,
    name: String,
}
impl Scratch {
    async fn new() -> Self {
        let url = std::env::var("OPENPR_TEST_DATABASE_URL")
            .expect("set disposable OPENPR_TEST_DATABASE_URL; no implicit database use");
        let admin = Database::connect(&url).await.unwrap();
        let name = format!("sv_flow_backend_gate_{}", Uuid::new_v4().simple());
        admin
            .execute_unprepared(&format!("CREATE DATABASE \"{name}\""))
            .await
            .unwrap();
        let (prefix, _) = url.rsplit_once('/').unwrap();
        let db = Database::connect(format!("{prefix}/{name}")).await.unwrap();
        let mut files = std::fs::read_dir(concat!(env!("CARGO_MANIFEST_DIR"), "/../../migrations"))
            .unwrap()
            .map(|r| r.unwrap().path())
            .filter(|p| p.extension().is_some_and(|s| s == "sql"))
            .collect::<Vec<_>>();
        files.sort();
        for file in files {
            db.execute_unprepared(&std::fs::read_to_string(&file).unwrap())
                .await
                .unwrap_or_else(|e| panic!("{}: {e}", file.display()));
        }
        // These are labeled unit-fixture reader leases, not deployment readiness evidence.
        for component in ["api", "worker", "maintenance"] {
            api::flow::scope_runtime::renew(&db, Uuid::new_v4(), component)
                .await
                .unwrap();
        }
        db.execute_unprepared(
            "UPDATE flow_runtime_capabilities SET personal_provisioning=true,personal_writes=true WHERE singleton",
        )
        .await
        .unwrap();
        Self { db, admin, name }
    }
    async fn close(self) {
        drop(self.db);
        self.admin
            .execute_unprepared(&format!("DROP DATABASE \"{}\" WITH(FORCE)", self.name))
            .await
            .unwrap();
    }
}
async fn user(db: &DatabaseConnection) -> Uuid {
    let id = Uuid::new_v4();
    db.execute(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "INSERT INTO users(id,email,password_hash,name,role,is_active) VALUES($1,$2,'','Scope test','user',true)",
        vec![id.into(), format!("{id}@example.test").into()],
    ))
    .await
    .unwrap();
    id
}
async fn count(db: &DatabaseConnection, table: &str) -> i64 {
    db.query_one(Statement::from_string(
        DbBackend::Postgres,
        format!("SELECT count(*) AS n FROM {table}"),
    ))
    .await
    .unwrap()
    .unwrap()
    .try_get("", "n")
    .unwrap()
}
fn create(scope_id: Uuid, parent: Option<Uuid>, key: &str) -> scope::CreateInput {
    scope::CreateInput {
        scope_id,
        object_type: "page".to_string(),
        editor_model: 1,
        parent_object_id: parent,
        project_id: None,
        title: "Independent page".to_string(),
        idempotency_key: key.to_string(),
        message: None,
        initial_fields: vec![],
        initial_view: None,
    }
}
#[tokio::test]
async fn no_workspace_concurrent_home_creation_replay_and_owner_disposition() {
    let scratch = Scratch::new().await;
    let db = &scratch.db;
    let owner = user(db).await;
    let (a, b) = tokio::join!(scope::personal_home(db, owner), scope::personal_home(db, owner));
    let (a, ar) = a.unwrap();
    let (b, br) = b.unwrap();
    assert_eq!(a.id, b.id);
    assert_eq!(
        ar.first().expect("fixture element").object_id,
        br.first().expect("fixture element").object_id
    );
    assert_ne!(a.id, ar.first().expect("fixture element").object_id);
    assert_eq!(count(db, "workspaces").await, 0);
    assert_eq!(count(db, "workspace_members").await, 0);
    let accepted = scope::create_personal(db, owner, create(a.id, None, "create-one"))
        .await
        .unwrap();
    let replay = scope::create_personal(db, owner, create(a.id, None, "create-one"))
        .await
        .unwrap();
    assert_eq!(accepted.field("event_id"), replay.field("event_id"));
    assert_eq!(accepted.field("object").field("id"), replay.field("object").field("id"));
    assert!(accepted.field("object").get("workspace_id").is_none());
    let object = Uuid::parse_str(accepted.field("object").field("id").as_str().unwrap()).unwrap();
    let boot = api::flow::scope_write::bootstrap(db, object, owner).await.unwrap();
    assert_eq!(*boot.field("head_seq"), 0);
    assert_eq!(*boot.field("limits").field("version"), "2");
    assert!(boot.field("limits").get("connections_per_workspace_max").is_none());
    assert!(
        db.execute(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "DELETE FROM users WHERE id=$1",
            vec![owner.into()]
        ))
        .await
        .is_err()
    );
    db.execute_unprepared(
        "UPDATE flow_runtime_capabilities SET external_mounts=false,personal_writes=false,personal_provisioning=false WHERE singleton",
    )
    .await
    .unwrap();
    assert_eq!(scope::personal_home(db, owner).await.unwrap().0.id, a.id);
    assert!(scope::object_view(db, object, owner).await.is_ok());
    scratch.close().await;
}
#[tokio::test]
async fn direct_guest_ticket_matches_read_acl_and_revocation_fences_commit() {
    let scratch = Scratch::new().await;
    let db = &scratch.db;
    let owner = user(db).await;
    let guest = user(db).await;
    let outsider = user(db).await;
    let (scope, _) = scope::personal_home(db, owner).await.unwrap();
    let accepted = scope::create_personal(db, owner, create(scope.id, None, "guest-page"))
        .await
        .unwrap();
    let object = Uuid::parse_str(accepted.field("object").field("id").as_str().unwrap()).unwrap();
    let document = Uuid::parse_str(accepted.field("object").field("document_id").as_str().unwrap()).unwrap();
    db.execute(Statement::from_sql_and_values(DbBackend::Postgres,"INSERT INTO flow_object_grants(scope_id,workspace_id,object_id,principal_kind,principal_id,level,granted_by) VALUES($1,NULL,$2,'user',$3,'edit',$4)",vec![scope.id.into(),object.into(),guest.into(),owner.into()])).await.unwrap();
    let proof = scope::authorize(db, object, guest, PermissionLevel::Edit)
        .await
        .unwrap();
    assert!(scope::object_view(db, object, outsider).await.is_err());
    let guest_view = scope::object_view(db, object, guest).await.unwrap();
    assert!(guest_view.get("parent_id").is_none());
    let origin = "http://127.0.0.1:18080";
    let ticket = scope_collab::issue(
        db,
        guest,
        object,
        document,
        "guest-client",
        origin,
        &[origin.to_string()],
    )
    .await
    .unwrap();
    assert!(ticket.field("websocket_path").as_str().unwrap().contains("?ticket="));
    assert_eq!(count(db, "workspace_members").await, 0);
    let tx = db.begin().await.unwrap();
    tx.execute(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "DELETE FROM flow_object_grants WHERE object_id=$1 AND principal_id=$2",
        vec![object.into(), guest.into()],
    ))
    .await
    .unwrap();
    scope::advance_revision(&tx, scope.id).await.unwrap();
    tx.commit().await.unwrap();
    let tx = db.begin().await.unwrap();
    assert!(scope::fence(&tx, &proof).await.is_err());
    tx.rollback().await.unwrap();
    assert!(
        scope_collab::consume(db, ticket.field("ticket").as_str().unwrap(), "guest-client", origin)
            .await
            .is_err()
    );
    assert!(scope::object_view(db, object, guest).await.is_err());
    scratch.close().await;
}
#[tokio::test]
async fn scoped_event_failure_rolls_back_content_and_dispatch() {
    let scratch = Scratch::new().await;
    let db = &scratch.db;
    let owner = user(db).await;
    let (scope, _) = scope::personal_home(db, owner).await.unwrap();
    let objects = count(db, "flow_objects").await;
    db.execute_unprepared("CREATE FUNCTION reject_scope_dispatch() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN IF NEW.source_flow_scope_id IS NOT NULL THEN RAISE EXCEPTION 'injected dispatch failure'; END IF;RETURN NEW;END $$;CREATE TRIGGER reject_scope_dispatch BEFORE INSERT ON event_dispatch FOR EACH ROW EXECUTE FUNCTION reject_scope_dispatch()").await.unwrap();
    assert!(
        scope::create_personal(db, owner, create(scope.id, None, "fault-one"))
            .await
            .is_err()
    );
    assert_eq!(count(db, "flow_objects").await, objects);
    assert_eq!(count(db, "business_events").await, 0);
    assert_eq!(count(db, "event_dispatch").await, 0);
    db.execute_unprepared("DROP TRIGGER reject_scope_dispatch ON event_dispatch")
        .await
        .unwrap();
    assert!(
        scope::create_personal(db, owner, create(scope.id, None, "fault-one"))
            .await
            .is_ok()
    );
    assert_eq!(count(db, "business_events").await, 1);
    assert_eq!(count(db, "event_dispatch").await, 1);
    scratch.close().await;
}

fn test_state(db: DatabaseConnection) -> platform::app::AppState {
    platform::app::AppState {
        db,
        cfg: platform::config::AppConfig {
            app_name: "scope-gate".into(),
            bind_addr: "127.0.0.1:0".into(),
            database_url: platform::config::Secret::new("postgres://unused/unused"),
            jwt_secret: platform::config::Secret::new("scope-gate-test-secret"),
            jwt_access_ttl_seconds: 900,
            jwt_refresh_ttl_seconds: 3600,
            allow_insecure_cookies: false,
            collab_allowed_origins: vec![],
        },
        flow_permission_cache: platform::app::FlowPermissionCacheSlot::default(),
    }
}
fn test_claims(user: Uuid) -> axum::Extension<platform::auth::JwtClaims> {
    axum::Extension(platform::auth::JwtClaims {
        sub: user.to_string(),
        email: format!("{user}@example.test"),
        token_type: platform::auth::TokenType::Access,
        iat: 0,
        exp: 0,
        sv: 0,
    })
}
async fn command(
    state: &platform::app::AppState,
    user: Uuid,
    object: Uuid,
    kind: &str,
    key: &str,
) -> Result<serde_json::Value, api::error::ApiError> {
    use axum::response::IntoResponse;

    let response = api::routes::flow::post_flow_v2_command(
        axum::extract::State(state.clone()),
        test_claims(user),
        None,
        axum::extract::Path(object),
        axum::Json(api::routes::flow::ExecuteFlowCommandRequest {
            command: api::routes::flow::FlowCommandEnvelope {
                command_type: kind.to_string(),
                payload: serde_json::json!({}),
            },
            expected_frontier: None,
            idempotency_key: key.to_string(),
            message: None,
        }),
    )
    .await?
    .into_response();
    Ok(serde_json::from_slice(&axum::body::to_bytes(response.into_body(), 1024 * 1024).await.unwrap()).unwrap())
}
#[tokio::test]
async fn lifecycle_request_identity_prevents_archive_restore_alias_and_keeps_epoch_stable() {
    let scratch = Scratch::new().await;
    let owner = user(&scratch.db).await;
    let (scope, _) = scope::personal_home(&scratch.db, owner).await.unwrap();
    let created = scope::create_personal(&scratch.db, owner, create(scope.id, None, "lifecycle-create"))
        .await
        .unwrap();
    let object = Uuid::parse_str(created.field("object").field("id").as_str().unwrap()).unwrap();
    let state = test_state(scratch.db.clone());
    let archived = command(&state, owner, object, "archive", "lifecycle-same")
        .await
        .unwrap();
    let revision = scope::revision(&scratch.db, scope.id).await.unwrap();
    let replay = command(&state, owner, object, "archive", "lifecycle-same")
        .await
        .unwrap();
    assert_eq!(archived, replay);
    let error = command(&state, owner, object, "restore", "lifecycle-same")
        .await
        .expect_err("changed operation must conflict");
    assert_eq!(error.kind(), api::error::ApiErrorKind::InvalidUpdate);
    assert_eq!(
        scope::identity(&scratch.db, object).await.unwrap().lifecycle_status,
        "archived"
    );
    assert_eq!(scope::revision(&scratch.db, scope.id).await.unwrap(), revision);
    assert_eq!(count(&scratch.db, "flow_scope_command_receipts").await, 2);
    command(&state, owner, object, "restore", "lifecycle-restore")
        .await
        .unwrap();
    assert_eq!(
        scope::identity(&scratch.db, object).await.unwrap().lifecycle_status,
        "active"
    );
    drop(state);
    scratch.close().await;
}
#[tokio::test]
async fn real_text_export_stores_verified_artifact_replays_and_rechecks_guest_access() {
    let scratch = Scratch::new().await;
    let owner = user(&scratch.db).await;
    let guest = user(&scratch.db).await;
    let (scope, _) = scope::personal_home(&scratch.db, owner).await.unwrap();
    let created = scope::create_personal(&scratch.db, owner, create(scope.id, None, "export-create"))
        .await
        .unwrap();
    let object = Uuid::parse_str(created.field("object").field("id").as_str().unwrap()).unwrap();
    let receipt = api::flow::scope_export::create(&scratch.db, object, owner, "markdown", None, false, "export-one")
        .await
        .unwrap();
    let replay = api::flow::scope_export::create(&scratch.db, object, owner, "markdown", None, false, "export-one")
        .await
        .unwrap();
    assert_eq!(receipt, replay);
    let job = Uuid::parse_str(receipt.field("job_id").as_str().unwrap()).unwrap();
    let (bytes, hash, format) = api::flow::scope_export::download(&scratch.db, job, owner)
        .await
        .unwrap();
    assert!(String::from_utf8(bytes.clone()).unwrap().contains("Independent page"));
    assert_eq!(hash, api::flow::collab::bootstrap::content_hash(&bytes));
    assert_eq!(format, "markdown");
    assert_eq!(count(&scratch.db, "flow_export_jobs").await, 1);
    assert_eq!(count(&scratch.db, "flow_package_artifacts").await, 1);
    assert!(
        api::flow::scope_export::download(&scratch.db, job, guest)
            .await
            .is_err()
    );
    assert!(
        api::flow::scope_export::create(&scratch.db, object, owner, "json", None, false, "export-one")
            .await
            .is_err()
    );
    scratch.close().await;
}

#[tokio::test]
async fn legacy_restricted_parent_creation_is_fenced_and_leaves_no_residue() {
    let scratch = Scratch::new().await;
    let db = &scratch.db;
    let owner = user(db).await;
    let member = user(db).await;
    let workspace = Uuid::new_v4();
    db.execute(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "INSERT INTO workspaces(id,slug,name,created_by) VALUES($1,$2,'Legacy scope',$3)",
        vec![workspace.into(), workspace.to_string().into(), owner.into()],
    ))
    .await
    .unwrap();
    db.execute(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "INSERT INTO workspace_members(workspace_id,user_id,role) VALUES($1,$2,'owner'),($1,$3,'member')",
        vec![workspace.into(), owner.into(), member.into()],
    ))
    .await
    .unwrap();
    db.execute(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "INSERT INTO flow_workspace_settings(workspace_id,flow_enabled) VALUES($1,true) ON CONFLICT (workspace_id) DO UPDATE SET flow_enabled = EXCLUDED.flow_enabled",
        vec![workspace.into()],
    ))
    .await
    .unwrap();
    let scope = scope::legacy_scope(db, workspace).await.unwrap();
    let root = scope::roots(db, scope.id)
        .await
        .unwrap()
        .first()
        .expect("fixture element")
        .object_id;
    let parent = Uuid::new_v4();
    db.execute(Statement::from_sql_and_values(DbBackend::Postgres,"INSERT INTO flow_objects(id,workspace_id,object_type,parent_id,inherit_from_parent) VALUES($1,$2,'page',$3,false)",vec![parent.into(),workspace.into(),root.into()])).await.unwrap();

    let engine = collab_core::LoroCollabEngine::new_empty(1);
    let doc = Uuid::new_v4();
    api::flow::repository::insert_collab_document(
        db,
        &api::flow::repository::NewCollabDocument {
            id: doc,
            object_id: parent,
            format_version: "loro-1".into(),
            snapshot: engine.export_snapshot().unwrap(),
            frontier: engine.frontier().as_bytes().to_vec(),
        },
    )
    .await
    .unwrap();
    api::flow::repository::insert_projection(
        db,
        &api::flow::repository::NewProjection {
            object_id: parent,
            document_seq: 0,
            document_frontier: engine.frontier().as_bytes().to_vec(),
            title: "Restricted parent".into(),
            state: serde_json::json!({"nodes":{}}),
            plain_text: String::new(),
        },
    )
    .await
    .unwrap();
    let before = (
        count(db, "flow_objects").await,
        count(db, "collab_documents").await,
        count(db, "business_events").await,
        count(db, "event_dispatch").await,
    );
    let state = test_state(db.clone());
    let result = api::flow::command::create_object(
        &state,
        api::flow::command::CreateObjectInput {
            workspace_id: workspace,
            actor_id: member,
            actor_is_bot: false,
            object_type: "page".into(),
            project_id: None,
            parent_object_id: Some(parent),
            title: "Must not exist".into(),
            idempotency_key: "restricted-create".into(),
            message: None,
            origin: api::flow::event_origin::CommandOrigin::first_request_from(
                api::flow::event_origin::EventSurface::Rest,
            ),
        },
    )
    .await;
    assert!(matches!(result,Err(err)if err.kind()==api::error::ApiErrorKind::PolicyRejected));
    assert_eq!(
        before,
        (
            count(db, "flow_objects").await,
            count(db, "collab_documents").await,
            count(db, "business_events").await,
            count(db, "event_dispatch").await
        )
    );
    drop(state);
    scratch.close().await;
}
async fn wait_for_database_lock(db: &DatabaseConnection, pid: i32) {
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(3);
    loop {
        let waiting = db
            .query_one(Statement::from_sql_and_values(
                DbBackend::Postgres,
                "SELECT EXISTS(SELECT 1 FROM pg_locks WHERE pid=$1 AND NOT granted) AS waiting",
                vec![pid.into()],
            ))
            .await
            .unwrap()
            .unwrap()
            .try_get::<bool>("", "waiting")
            .unwrap();
        if waiting {
            return;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "expected actual PostgreSQL lock wait"
        );
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
}
#[tokio::test]
async fn authorization_write_fence_serializes_both_revocation_orders_without_upgrade() {
    let scratch = Scratch::new().await;
    let db = &scratch.db;
    let owner = user(db).await;
    let (scope, _) = scope::personal_home(db, owner).await.unwrap();
    let created = scope::create_personal(db, owner, create(scope.id, None, "fence-page"))
        .await
        .unwrap();
    let object = Uuid::parse_str(created.field("object").field("id").as_str().unwrap()).unwrap();
    let proof = scope::authorize(db, object, owner, PermissionLevel::Edit)
        .await
        .unwrap();
    let writer = db.begin().await.unwrap();
    scope::fence(&writer, &proof).await.unwrap();
    let other = db.clone();
    let other_proof = proof.clone();
    let (started, ready) = tokio::sync::oneshot::channel();
    let mut revoke = tokio::spawn(async move {
        let tx = other.begin().await.unwrap();
        let pid = tx
            .query_one(Statement::from_string(
                DbBackend::Postgres,
                "SELECT pg_backend_pid() AS pid".to_string(),
            ))
            .await
            .unwrap()
            .unwrap()
            .try_get::<i32>("", "pid")
            .unwrap();
        started.send(pid).unwrap();
        scope::lock_change(&tx, &other_proof).await.unwrap();
        scope::advance_revision(&tx, other_proof.scope_id).await.unwrap();
        tx.commit().await.unwrap();
    });
    let pid = ready.await.unwrap();
    wait_for_database_lock(db, pid).await;
    assert!(!revoke.is_finished());
    writer.commit().await.unwrap();
    (&mut revoke).await.unwrap();
    let old = proof;
    let new = scope::authorize(db, object, owner, PermissionLevel::Edit)
        .await
        .unwrap();
    let revoker = db.begin().await.unwrap();
    scope::lock_change(&revoker, &new).await.unwrap();
    scope::advance_revision(&revoker, scope.id).await.unwrap();
    let other = db.clone();
    let (started, ready) = tokio::sync::oneshot::channel();
    let waiter = tokio::spawn(async move {
        let tx = other.begin().await.unwrap();
        let pid = tx
            .query_one(Statement::from_string(
                DbBackend::Postgres,
                "SELECT pg_backend_pid() AS pid".to_string(),
            ))
            .await
            .unwrap()
            .unwrap()
            .try_get::<i32>("", "pid")
            .unwrap();
        started.send(pid).unwrap();
        let result = scope::fence(&tx, &old).await;
        tx.rollback().await.unwrap();
        result
    });
    let pid = ready.await.unwrap();
    wait_for_database_lock(db, pid).await;
    revoker.commit().await.unwrap();
    assert!(waiter.await.unwrap().is_err());
    scratch.close().await;
}

#[tokio::test]
async fn personal_typed_collection_records_share_validators_and_hide_restricted_values() {
    use serde_json::json;
    let scratch = Scratch::new().await;
    let db = &scratch.db;
    let owner = user(db).await;
    let outsider = user(db).await;
    let (scope, _) = scope::personal_home(db, owner).await.unwrap();
    let text = Uuid::new_v4();
    let number = Uuid::new_v4();
    let secret = Uuid::new_v4();
    let mut input = create(scope.id, None, "typed-collection");
    input.object_type = "collection".into();
    input.initial_fields = vec![
        json!({"field_id":text,"label":"Name","field_type":"text","config":{},"index":0}),
        json!({"field_id":number,"label":"Count","field_type":"number","config":{},"index":1}),
        json!({"field_id":secret,"label":"Secret","field_type":"text","config":{"restricted":true},"index":2}),
    ];
    let created = scope::create_personal(db, owner, input).await.unwrap();
    let collection = Uuid::parse_str(created.field("object").field("id").as_str().unwrap()).unwrap();
    let state = test_state(db.clone());
    let description = api::flow::scope_collections::describe(&state, collection, owner, None)
        .await
        .unwrap();
    assert_eq!(description.field("fields").as_array().unwrap().len(), 2);
    assert_eq!(*description.field("client_crdt_enabled"), false);
    assert_eq!(*description.field("field_secrecy_enabled"), true);
    assert!(
        !description.to_string().contains("Secret"),
        "restricted field metadata leaked in complete description"
    );
    assert!(
        !scope::object_view(db, collection, owner)
            .await
            .unwrap()
            .to_string()
            .contains("Secret")
    );
    let payload = json!({"properties":{text.to_string():"visible name",number.to_string():42,secret.to_string():"must remain hidden"},"body":"record body"});
    let accepted = api::flow::scope_collections::execute(
        &state,
        collection,
        owner,
        api::flow::collections::CollectionCommandType::RecordCreate,
        payload.clone(),
        "typed-record",
        None,
        None,
    )
    .await
    .unwrap();
    let replay = api::flow::scope_collections::execute(
        &state,
        collection,
        owner,
        api::flow::collections::CollectionCommandType::RecordCreate,
        payload,
        "typed-record",
        None,
        None,
    )
    .await
    .unwrap();
    assert_eq!(accepted, replay);
    let record = Uuid::parse_str(accepted.field("object").field("id").as_str().unwrap()).unwrap();
    assert_eq!(*accepted.field("object").field("semantic_content"), json!({}));
    assert!(!accepted.to_string().contains("must remain hidden"));
    let request: api::flow::collections::RecordQueryPayload = serde_json::from_value(json!({"limit":50})).unwrap();
    let result = api::flow::scope_collections::query(&state, collection, owner, &request)
        .await
        .unwrap();
    assert_eq!(result.field("items").as_array().unwrap().len(), 1);
    assert_eq!(
        result
            .field("items")
            .as_array()
            .expect("fixture array")
            .first()
            .expect("fixture element")
            .field("values_by_field_id")
            .field(&text.to_string()),
        "visible name"
    );
    assert!(
        result
            .field("items")
            .as_array()
            .expect("fixture array")
            .first()
            .expect("fixture element")
            .field("values_by_field_id")
            .get(secret.to_string())
            .is_none()
    );
    assert!(!result.to_string().contains("must remain hidden"));
    assert!(!result.to_string().contains("Secret"));
    assert!(scope::object_view(db, record, outsider).await.is_err());
    assert!(api::flow::scope_write::bootstrap(db, record, owner).await.is_err());
    let invalid = json!({"properties":{number.to_string():"not a number"}});
    let before = count(db, "flow_objects").await;
    assert!(
        api::flow::scope_collections::execute(
            &state,
            collection,
            owner,
            api::flow::collections::CollectionCommandType::RecordCreate,
            invalid,
            "typed-invalid",
            None,
            None
        )
        .await
        .is_err()
    );
    assert_eq!(before, count(db, "flow_objects").await);
    assert_eq!(count(db, "flow_record_projections").await, 1);
    assert_eq!(count(db, "flow_record_value_projections").await, 3);
    assert_eq!(count(db, "workspaces").await, 0);
    drop(state);
    scratch.close().await;
}

#[tokio::test]
async fn typed_exports_invalidate_old_bytes_on_record_or_field_policy_change_and_stay_immutable() {
    use axum::response::IntoResponse;
    use serde_json::json;
    let scratch = Scratch::new().await;
    let db = &scratch.db;
    let owner = user(db).await;
    let guest = user(db).await;
    let (scope, _) = scope::personal_home(db, owner).await.unwrap();
    let field = Uuid::new_v4();
    let mut input = create(scope.id, None, "export-policy-collection");
    input.object_type = "collection".into();
    input.initial_fields =
        vec![json!({"field_id":field,"label":"Visible salary","field_type":"text","config":{},"index":0})];
    let created = scope::create_personal(db, owner, input).await.unwrap();
    let collection = Uuid::parse_str(created.field("object").field("id").as_str().unwrap()).unwrap();
    let state = test_state(db.clone());
    let record = api::flow::scope_collections::execute(
        &state,
        collection,
        owner,
        api::flow::collections::CollectionCommandType::RecordCreate,
        json!({"properties":{field.to_string():"exported secret after restriction"}}),
        "export-policy-record",
        None,
        None,
    )
    .await
    .unwrap();
    let record = Uuid::parse_str(record.field("object").field("id").as_str().unwrap()).unwrap();
    db.execute(Statement::from_sql_and_values(DbBackend::Postgres,"INSERT INTO flow_object_grants(scope_id,workspace_id,object_id,principal_kind,principal_id,level,granted_by) VALUES($1,NULL,$2,'user',$3,'view',$4)",vec![scope.id.into(),collection.into(),guest.into(),owner.into()])).await.unwrap();
    for format in ["json", "markdown", "csv"] {
        let receipt =
            api::flow::scope_export::create(db, collection, guest, format, None, false, &format!("guest-{format}"))
                .await
                .unwrap();
        let job = Uuid::parse_str(receipt.field("job_id").as_str().unwrap()).unwrap();
        let (bytes, hash, _) = api::flow::scope_export::download(db, job, guest).await.unwrap();
        assert_eq!(hash, api::flow::collab::bootstrap::content_hash(&bytes));
        assert!(
            String::from_utf8(bytes)
                .unwrap()
                .contains("exported secret after restriction")
        );
    }
    let job = db
        .query_one(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT id FROM flow_export_jobs WHERE scope_id=$1 AND actor_id=$2 AND idempotency_key='guest-json'",
            vec![scope.id.into(), guest.into()],
        ))
        .await
        .unwrap()
        .unwrap()
        .try_get::<Uuid>("", "id")
        .unwrap();
    let response = api::routes::flow::get_flow_v2_export_artifact(
        axum::extract::State(state.clone()),
        test_claims(guest),
        None,
        axum::extract::Path(job),
    )
    .await
    .unwrap()
    .into_response();
    assert_eq!(
        response.headers()[axum::http::header::CACHE_CONTROL],
        "private, no-store"
    );
    let tx = db.begin().await.unwrap();
    db.execute(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "UPDATE flow_objects SET inherit_from_parent=false WHERE id=$1",
        vec![record.into()],
    ))
    .await
    .unwrap();
    scope::advance_revision(&tx, scope.id).await.unwrap();
    tx.commit().await.unwrap();
    assert!(scope::object_view(db, record, guest).await.is_err());
    assert!(api::flow::scope_export::download(db, job, guest).await.is_err());
    assert!(
        api::flow::scope_export::create(db, collection, guest, "json", None, false, "guest-json")
            .await
            .is_err()
    );
    let fresh = api::flow::scope_export::create(db, collection, guest, "json", None, false, "guest-fresh")
        .await
        .unwrap();
    let fresh = Uuid::parse_str(fresh.field("job_id").as_str().unwrap()).unwrap();
    let (bytes, _, _) = api::flow::scope_export::download(db, fresh, guest).await.unwrap();
    assert!(
        !String::from_utf8(bytes)
            .unwrap()
            .contains("exported secret after restriction")
    );
    let owner_job = api::flow::scope_export::create(
        db,
        collection,
        owner,
        "json",
        None,
        false,
        "owner-before-field-restrict",
    )
    .await
    .unwrap();
    let owner_job = Uuid::parse_str(owner_job.field("job_id").as_str().unwrap()).unwrap();
    // Explicit policy fixture: the live validator exercises actual field_update separately.
    db.execute(Statement::from_sql_and_values(DbBackend::Postgres,"UPDATE flow_field_projections SET config='{\"restricted\":true}'::jsonb WHERE collection_id=$1 AND field_id=$2",vec![collection.into(),field.into()])).await.unwrap();
    db.execute(Statement::from_sql_and_values(DbBackend::Postgres,"UPDATE flow_collection_projections SET field_secrecy_enabled=true,client_crdt_enabled=false,schema_seq=schema_seq+1 WHERE collection_id=$1",vec![collection.into()])).await.unwrap();
    assert!(api::flow::scope_export::download(db, owner_job, owner).await.is_err());
    assert!(
        api::flow::scope_export::create(
            db,
            collection,
            owner,
            "json",
            None,
            false,
            "owner-before-field-restrict"
        )
        .await
        .is_err()
    );
    let fresh =
        api::flow::scope_export::create(db, collection, owner, "json", None, false, "owner-after-field-restrict")
            .await
            .unwrap();
    let fresh = Uuid::parse_str(fresh.field("job_id").as_str().unwrap()).unwrap();
    let (bytes, _, _) = api::flow::scope_export::download(db, fresh, owner).await.unwrap();
    let text = String::from_utf8(bytes).unwrap();
    assert!(!text.contains("Visible salary"));
    assert!(!text.contains("exported secret after restriction"));
    drop(state);
    scratch.close().await;
}

#[tokio::test]
async fn artifact_without_trustworthy_creation_authority_is_retained_but_not_downloadable() {
    let scratch = Scratch::new().await;
    let db = &scratch.db;
    let owner = user(db).await;
    let (scope, _) = scope::personal_home(db, owner).await.unwrap();
    let object = scope::create_personal(db, owner, create(scope.id, None, "old-artifact-source"))
        .await
        .unwrap();
    let object = Uuid::parse_str(object.field("object").field("id").as_str().unwrap()).unwrap();
    let receipt = api::flow::scope_export::create(db, object, owner, "markdown", None, false, "old-artifact-key")
        .await
        .unwrap();
    let job = Uuid::parse_str(receipt.field("job_id").as_str().unwrap()).unwrap();
    let (_, hash, _) = api::flow::scope_export::download(db, job, owner).await.unwrap();
    // Models an expanded pre0083 row: no current proof is ever stamped onto the old bytes.
    db.execute(Statement::from_sql_and_values(DbBackend::Postgres,"UPDATE flow_export_jobs SET source_authz_revision=NULL,policy_collection_id=NULL,policy_schema_seq=NULL,included_record_ids='{}'::uuid[],audience_authorities='[]'::jsonb WHERE id=$1",vec![job.into()])).await.unwrap();
    assert!(api::flow::scope_export::download(db, job, owner).await.is_err());
    assert!(
        api::flow::scope_export::create(db, object, owner, "markdown", None, false, "old-artifact-key")
            .await
            .is_err()
    );
    let row=db.query_one(Statement::from_sql_and_values(DbBackend::Postgres,"SELECT j.id,a.package_sha256 FROM flow_export_jobs j JOIN flow_package_artifacts a ON a.id=j.artifact_id WHERE j.id=$1",vec![job.into()])).await.unwrap().unwrap();
    assert_eq!(row.try_get::<Uuid>("", "id").unwrap(), job);
    assert_eq!(row.try_get::<String>("", "package_sha256").unwrap(), hash);
    scratch.close().await;
}

#[tokio::test]
async fn legacy_artifacts_capture_policy_and_only_old_page_text_has_a_safe_exception() {
    use api::flow::{
        export::{CreateExportRequest, ExportPrincipal, ExportScope},
        repository as repo,
    };
    use collab_core::CollabEngine;
    use serde_json::json;
    let scratch = Scratch::new().await;
    let db = &scratch.db;
    let owner = user(db).await;
    let workspace = Uuid::new_v4();
    db.execute(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "INSERT INTO workspaces(id,slug,name,created_by) VALUES($1,$2,'Legacy export',$3)",
        vec![workspace.into(), workspace.to_string().into(), owner.into()],
    ))
    .await
    .unwrap();
    db.execute(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "INSERT INTO workspace_members(workspace_id,user_id,role) VALUES($1,$2,'owner')",
        vec![workspace.into(), owner.into()],
    ))
    .await
    .unwrap();
    db.execute(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "INSERT INTO flow_workspace_settings(workspace_id,flow_enabled) VALUES($1,true) ON CONFLICT (workspace_id) DO UPDATE SET flow_enabled = EXCLUDED.flow_enabled",
        vec![workspace.into()],
    ))
    .await
    .unwrap();
    let scope = scope::legacy_scope(db, workspace).await.unwrap();
    let root = scope::roots(db, scope.id)
        .await
        .unwrap()
        .first()
        .expect("fixture element")
        .object_id;
    let field = Uuid::new_v4();
    let mut ids = Vec::new();
    for kind in ["page", "collection"] {
        let object = Uuid::new_v4();
        let document = Uuid::new_v4();
        let mut engine = collab_core::LoroCollabEngine::new_empty(1);
        engine.set_title(&format!("Legacy {kind}")).unwrap();
        if kind == "collection" {
            api::flow::collections::apply_initial_collection_schema(&mut engine,&json!({"initial_fields":[{"field_id":field,"label":"Legacy salary","field_type":"text","config":{},"index":0}]})).unwrap();
        }
        let tx = db.begin().await.unwrap();
        repo::insert_flow_object(
            &tx,
            &repo::NewFlowObject {
                id: object,
                workspace_id: workspace,
                project_id: None,
                object_type: kind.into(),
                parent_id: Some(root),
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
                frontier: engine.frontier().as_bytes().to_vec(),
            },
        )
        .await
        .unwrap();
        repo::insert_projection(
            &tx,
            &repo::NewProjection {
                object_id: object,
                document_seq: 0,
                document_frontier: engine.frontier().as_bytes().to_vec(),
                title: format!("Legacy {kind}"),
                state: serde_json::to_value(engine.semantic_snapshot().unwrap()).unwrap(),
                plain_text: String::new(),
            },
        )
        .await
        .unwrap();
        if kind == "collection" {
            api::flow::collections::insert_collection_projection(&tx, object, document, workspace, None, json!({}))
                .await
                .unwrap();
            api::flow::collections::sync_new_collection_projection(&tx, object, &engine)
                .await
                .unwrap();
        }
        tx.commit().await.unwrap();
        ids.push(object);
    }
    let principal = ExportPrincipal {
        id: owner,
        kind: "user".into(),
        role: "owner".into(),
        workspace_export_capability: false,
    };
    let request = |object, format: &str, key: &str| CreateExportRequest {
        object_types: None,
        scope: ExportScope::Object(object),
        format: format.into(),
        at_seq: None,
        include_history: false,
        idempotency_key: key.into(),
        source_head: "d523d7c724b4122f7b83cb30e6614a36f51a1c65".into(),
        principal: principal.clone(),
    };
    let page = api::flow::export::create_package_export(
        db,
        &request(
            ids.first().copied().expect("fixture element"),
            "markdown",
            "legacy-page-text",
        ),
    )
    .await
    .unwrap();
    assert!(
        api::flow::export::download_package_export(db, page.job_id, &principal)
            .await
            .is_ok()
    );
    db.execute(Statement::from_sql_and_values(DbBackend::Postgres,"UPDATE flow_export_jobs SET source_authz_revision=NULL,artifact_resource_ids='{}'::uuid[],artifact_schema_revisions='[]'::jsonb WHERE id=$1",vec![page.job_id.into()])).await.unwrap();
    assert!(
        api::flow::export::download_package_export(db, page.job_id, &principal)
            .await
            .is_ok(),
        "single old legacy Page text is the explicit policy-complete exception"
    );
    let typed = api::flow::export::create_package_export(
        db,
        &request(
            ids.get(1).copied().expect("fixture element"),
            "json",
            "legacy-typed-before",
        ),
    )
    .await
    .unwrap();
    let (bytes, _, _) = api::flow::export::download_package_export(db, typed.job_id, &principal)
        .await
        .unwrap();
    assert!(String::from_utf8(bytes).unwrap().contains("Legacy salary"));
    db.execute(Statement::from_sql_and_values(DbBackend::Postgres,"UPDATE flow_field_projections SET config='{\"restricted\":true}'::jsonb WHERE collection_id=$1 AND field_id=$2",vec![ids.get(1).copied().expect("fixture element").into(),field.into()])).await.unwrap();
    db.execute(Statement::from_sql_and_values(DbBackend::Postgres,"UPDATE flow_collection_projections SET schema_seq=schema_seq+1,field_secrecy_enabled=true,client_crdt_enabled=false WHERE collection_id=$1",vec![ids.get(1).copied().expect("fixture element").into()])).await.unwrap();
    assert!(
        api::flow::export::download_package_export(db, typed.job_id, &principal)
            .await
            .is_err()
    );
    let fresh = api::flow::export::create_package_export(
        db,
        &request(
            ids.get(1).copied().expect("fixture element"),
            "json",
            "legacy-typed-after",
        ),
    )
    .await
    .unwrap();
    let (bytes, _, _) = api::flow::export::download_package_export(db, fresh.job_id, &principal)
        .await
        .unwrap();
    assert!(!String::from_utf8(bytes).unwrap().contains("Legacy salary"));
    db.execute(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "UPDATE flow_export_jobs SET source_authz_revision=NULL WHERE id=$1",
        vec![fresh.job_id.into()],
    ))
    .await
    .unwrap();
    assert!(
        api::flow::export::download_package_export(db, fresh.job_id, &principal)
            .await
            .is_err(),
        "old typed bytes cannot be blessed by current authority"
    );
    let package = api::flow::export::create_package_export(
        db,
        &request(
            ids.first().copied().expect("fixture element"),
            "package",
            "legacy-page-package",
        ),
    )
    .await
    .unwrap();
    db.execute(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "UPDATE flow_export_jobs SET source_authz_revision=NULL WHERE id=$1",
        vec![package.job_id.into()],
    ))
    .await
    .unwrap();
    assert!(
        api::flow::export::download_package_export(db, package.job_id, &principal)
            .await
            .is_err(),
        "old package can contain external dependencies; no blanket Page exception"
    );
    scratch.close().await;
}

#[cfg(target_os = "linux")]
#[tokio::test]
async fn personal_collection_embed_is_one_atomic_transaction_and_retry_identity() {
    use serde_json::json;
    let scratch = Scratch::new().await;
    let db = &scratch.db;
    let owner = user(db).await;
    let (scope, _) = scope::personal_home(db, owner).await.unwrap();
    let page = scope::create_personal(db, owner, create(scope.id, None, "embed-page"))
        .await
        .unwrap();
    let page = Uuid::parse_str(page.field("object").field("id").as_str().unwrap()).unwrap();
    let document = Uuid::parse_str(
        scope::object_view(db, page, owner)
            .await
            .unwrap()
            .field("document_id")
            .as_str()
            .unwrap(),
    )
    .unwrap();
    let state = test_state(db.clone());
    let tables = [
        "flow_objects",
        "collab_documents",
        "flow_object_projections",
        "flow_collection_projections",
        "flow_field_projections",
        "flow_relations",
        "collab_updates",
        "business_events",
        "event_dispatch",
        "flow_scope_command_receipts",
    ];
    let mut before = Vec::new();
    for table in tables {
        before.push(count(db, table).await);
    }
    db.execute_unprepared("CREATE FUNCTION fail_personal_embed_relation() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN IF NEW.workspace_id IS NULL AND NEW.relation_type='embeds' THEN RAISE EXCEPTION 'injected embed relation failure';END IF;RETURN NEW;END $$;CREATE TRIGGER fail_personal_embed_relation BEFORE INSERT ON flow_relations FOR EACH ROW EXECUTE FUNCTION fail_personal_embed_relation()").await.unwrap();
    let field = Uuid::new_v4();
    let payload = json!({"title":"Atomic embedded Collection","initial_fields":[{"field_id":field,"label":"Name","field_type":"text","config":{},"index":0}],"display_settings":{}});
    assert!(
        api::flow::scope_collections::execute(
            &state,
            page,
            owner,
            api::flow::collections::CollectionCommandType::CreateCollectionEmbed,
            payload.clone(),
            "atomic-embed",
            None,
            None
        )
        .await
        .is_err()
    );
    let mut after = Vec::new();
    for table in tables {
        after.push(count(db, table).await);
    }
    assert_eq!(
        before, after,
        "block/object/document/projection/relation/event/dispatch/receipt must roll back together"
    );
    assert_eq!(
        api::flow::collab::bootstrap::load(db, document).await.unwrap().head_seq,
        0
    );
    db.execute_unprepared("DROP TRIGGER fail_personal_embed_relation ON flow_relations")
        .await
        .unwrap();
    let result = api::flow::scope_collections::execute(
        &state,
        page,
        owner,
        api::flow::collections::CollectionCommandType::CreateCollectionEmbed,
        payload.clone(),
        "atomic-embed",
        None,
        None,
    )
    .await
    .unwrap();
    let replay = api::flow::scope_collections::execute(
        &state,
        page,
        owner,
        api::flow::collections::CollectionCommandType::CreateCollectionEmbed,
        payload,
        "atomic-embed",
        None,
        None,
    )
    .await
    .unwrap();
    assert_eq!(result, replay);
    let collection = Uuid::parse_str(result.field("command_result").field("collection_id").as_str().unwrap()).unwrap();
    assert_eq!(scope::identity(db, collection).await.unwrap().scope_id, scope.id);
    assert_eq!(scope::identity(db, collection).await.unwrap().workspace_id, None);
    assert_eq!(
        api::flow::collab::bootstrap::load(db, document).await.unwrap().head_seq,
        1
    );
    assert_eq!(count(db, "flow_relations").await, 1);
    assert_eq!(count(db, "flow_collection_projections").await, 1);
    assert_eq!(count(db, "collab_updates").await, 1);
    assert_eq!(count(db, "workspaces").await, 0);
    assert_eq!(count(db, "workspace_members").await, 0);
    drop(state);
    scratch.close().await;
}
#[tokio::test]
async fn authorization_notifications_are_explicitly_bound_to_the_one_use_ticket() {
    let scratch = Scratch::new().await;
    let db = &scratch.db;
    let owner = user(db).await;
    let (scope, _) = scope::personal_home(db, owner).await.unwrap();
    let page = scope::create_personal(db, owner, create(scope.id, None, "notification-page"))
        .await
        .unwrap();
    let page = Uuid::parse_str(page.field("object").field("id").as_str().unwrap()).unwrap();
    let document = Uuid::parse_str(
        scope::object_view(db, page, owner)
            .await
            .unwrap()
            .field("document_id")
            .as_str()
            .unwrap(),
    )
    .unwrap();
    let origin = "http://127.0.0.1:18080";
    let old = scope_collab::issue(db, owner, page, document, "old-client", origin, &[origin.into()])
        .await
        .unwrap();
    assert_eq!(*old.field("authorization_notifications"), false);
    let old = scope_collab::consume(db, old.field("ticket").as_str().unwrap(), "old-client", origin)
        .await
        .unwrap();
    assert!(!old.authorization_notifications);
    let new = scope_collab::issue_with_options(db, owner, page, document, "new-client", origin, &[origin.into()], true)
        .await
        .unwrap();
    assert_eq!(*new.field("authorization_notifications"), true);
    let context = scope_collab::consume(db, new.field("ticket").as_str().unwrap(), "new-client", origin)
        .await
        .unwrap();
    assert!(context.authorization_notifications);
    assert_eq!(context.scope_id, scope.id);
    assert_eq!(context.object_id, page);
    assert_eq!(context.document_id, document);
    assert!(
        scope_collab::consume(db, new.field("ticket").as_str().unwrap(), "new-client", origin)
            .await
            .is_err()
    );
    scratch.close().await;
}

#[tokio::test]
async fn personal_comment_threads_mentions_and_receipts_need_no_workspace() {
    use serde_json::json;
    let scratch = Scratch::new().await;
    let db = &scratch.db;
    let owner = user(db).await;
    let commenter = user(db).await;
    let viewer = user(db).await;
    let outsider = user(db).await;
    let (scope, _) = scope::personal_home(db, owner).await.unwrap();
    let created = scope::create_personal(db, owner, create(scope.id, None, "social-page"))
        .await
        .unwrap();
    let object = Uuid::parse_str(created.field("object").field("id").as_str().unwrap()).unwrap();
    for (principal, level) in [(commenter, "comment"), (viewer, "view")] {
        db.execute(Statement::from_sql_and_values(DbBackend::Postgres,"INSERT INTO flow_object_grants(scope_id,workspace_id,object_id,principal_kind,principal_id,level,granted_by) VALUES($1,NULL,$2,'user',$3,$4,$5)",vec![scope.id.into(),object.into(),principal.into(),level.into(),owner.into()])).await.unwrap();
    }
    let input = api::flow::comments::NewComment {
        body: " note ".into(),
        block_id: None,
        parent_id: None,
        mentions: vec![viewer, outsider],
    };
    let comment = api::flow::scope_social::create_comment(db, object, commenter, input.clone(), Some("comment-intent"))
        .await
        .unwrap();
    let replay = api::flow::scope_social::create_comment(db, object, commenter, input, Some("comment-intent"))
        .await
        .unwrap();
    assert_eq!(comment.id, replay.id);
    assert_eq!(comment.body, "note");
    assert_eq!(count(db, "flow_comments").await, 1);
    assert_eq!(count(db, "notifications").await, 1);
    let notification = db
        .query_one(Statement::from_string(
            DbBackend::Postgres,
            "SELECT user_id,workspace_id,link FROM notifications".to_string(),
        ))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(notification.try_get::<Uuid>("", "user_id").unwrap(), viewer);
    assert_eq!(notification.try_get::<Option<Uuid>>("", "workspace_id").unwrap(), None);
    assert_eq!(
        notification.try_get::<String>("", "link").unwrap(),
        format!("/flow/{object}")
    );
    assert!(
        api::flow::scope_social::create_comment(
            db,
            object,
            viewer,
            api::flow::comments::NewComment {
                body: "denied".into(),
                block_id: None,
                parent_id: None,
                mentions: vec![]
            },
            None
        )
        .await
        .is_err()
    );
    let edited = api::flow::scope_social::update_comment(
        db,
        object,
        comment.id,
        commenter,
        "updated",
        None,
        Some("comment-edit"),
    )
    .await
    .unwrap();
    assert_eq!(edited.body, "updated");
    assert_eq!(count(db, "notifications").await, 1);
    assert!(
        api::flow::scope_social::update_comment(db, object, comment.id, owner, "not the author", None, None)
            .await
            .is_err()
    );
    let thread_reply = api::flow::scope_social::create_comment(
        db,
        object,
        owner,
        api::flow::comments::NewComment {
            body: "reply".into(),
            block_id: None,
            parent_id: Some(comment.id),
            mentions: vec![],
        },
        None,
    )
    .await
    .unwrap();
    assert!(
        api::flow::scope_social::create_comment(
            db,
            object,
            owner,
            api::flow::comments::NewComment {
                body: "nested reply".into(),
                block_id: None,
                parent_id: Some(thread_reply.id),
                mentions: vec![]
            },
            None
        )
        .await
        .is_err()
    );
    let resolved =
        api::flow::scope_social::resolve_comment(db, object, comment.id, commenter, true, Some("comment-resolve"))
            .await
            .unwrap();
    assert!(resolved.resolved_at.is_some());
    api::flow::scope_social::delete_comment(db, object, comment.id, owner, Some("comment-delete"))
        .await
        .unwrap();
    assert!(
        api::flow::comments::list_comments(db, object)
            .await
            .unwrap()
            .items
            .is_empty()
    );
    assert_eq!(count(db, "workspaces").await, 0);
    assert_eq!(count(db, "workspace_members").await, 0);
    let events=db.query_all(Statement::from_sql_and_values(DbBackend::Postgres,"SELECT workspace_id,source_flow_scope_id,payload,metadata FROM business_events WHERE event_type LIKE 'flow.comment.%'",vec![])).await.unwrap();
    assert!(!events.is_empty());
    for event in events {
        assert_eq!(event.try_get::<Option<Uuid>>("", "workspace_id").unwrap(), None);
        assert_eq!(
            event.try_get::<Option<Uuid>>("", "source_flow_scope_id").unwrap(),
            Some(scope.id)
        );
        assert!(
            !event
                .try_get::<serde_json::Value>("", "payload")
                .unwrap()
                .to_string()
                .contains("updated")
        );
        assert_eq!(
            event.try_get::<serde_json::Value>("", "metadata").unwrap().get("body"),
            None
        );
    }
    let _ = json!({});
    scratch.close().await;
}

#[tokio::test]
async fn personal_assets_publish_only_object_owned_references_and_public_tokens_store_hashes_only() {
    use api::flow::appearance::{PageAppearance, PageIcon};
    let scratch = Scratch::new().await;
    let db = &scratch.db;
    let owner = user(db).await;
    let viewer = user(db).await;
    let (scope, _) = scope::personal_home(db, owner).await.unwrap();
    let a = scope::create_personal(db, owner, create(scope.id, None, "assets-a"))
        .await
        .unwrap();
    let b = scope::create_personal(db, owner, create(scope.id, None, "assets-b"))
        .await
        .unwrap();
    let a = Uuid::parse_str(a.field("object").field("id").as_str().unwrap()).unwrap();
    let b = Uuid::parse_str(b.field("object").field("id").as_str().unwrap()).unwrap();
    db.execute(Statement::from_sql_and_values(DbBackend::Postgres,"INSERT INTO flow_object_grants(scope_id,workspace_id,object_id,principal_kind,principal_id,level,granted_by) VALUES($1,NULL,$2,'user',$3,'view',$4)",vec![scope.id.into(),a.into(),viewer.into(),owner.into()])).await.unwrap();
    let image = "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR4nGNgYPj/HwADAgH/5ncLrgAAAABJRU5ErkJggg==";
    let uploaded = api::flow::scope_assets::upload(db, a, owner, image, Some("asset-one"))
        .await
        .unwrap();
    let replay = api::flow::scope_assets::upload(db, a, owner, image, Some("asset-one"))
        .await
        .unwrap();
    assert_eq!(uploaded.asset_id, replay.asset_id);
    assert_eq!(count(db, "flow_appearance_assets").await, 1);
    assert!(
        api::flow::scope_assets::read(db, a, uploaded.asset_id, owner)
            .await
            .is_ok()
    );
    assert!(
        api::flow::scope_assets::read(db, a, uploaded.asset_id, viewer)
            .await
            .is_err()
    );
    let appearance = PageAppearance {
        icon: Some(PageIcon::Image {
            asset_id: uploaded.asset_id,
        }),
        cover: None,
    };
    assert!(
        api::flow::scope_assets::set(db, b, owner, appearance.clone(), Some("wrong-asset"))
            .await
            .is_err()
    );
    api::flow::scope_assets::set(db, a, owner, appearance, Some("appearance-one"))
        .await
        .unwrap();
    let bytes = api::flow::scope_assets::read(db, a, uploaded.asset_id, viewer)
        .await
        .unwrap();
    assert_eq!(bytes.content_type, "image/png");
    let public = api::flow::scope_shares::create(db, a, owner, None).await.unwrap();
    let listed = api::flow::scope_shares::list(db, a, owner).await.unwrap();
    assert!(!serde_json::to_string(&listed).unwrap().contains(&public.token));
    assert!(api::flow::scope_shares::list(db, a, viewer).await.is_err());
    let page = api::flow::scope_shares::read_public(db, &public.token).await.unwrap();
    assert_eq!(page.object_id, a);
    let asset = api::flow::scope_shares::read_public_asset(db, &public.token, uploaded.asset_id)
        .await
        .unwrap();
    assert_eq!(asset.bytes, bytes.bytes);
    let other = api::flow::scope_assets::upload(db, b, owner, image, Some("asset-other"))
        .await
        .unwrap();
    assert!(
        api::flow::scope_shares::read_public_asset(db, &public.token, other.asset_id)
            .await
            .is_err()
    );
    let audit=db.query_one(Statement::from_string(DbBackend::Postgres,"SELECT count(*) AS n FROM event_dispatch d JOIN business_events e ON e.id=d.event_id WHERE e.event_type='flow.appearance.asset_uploaded'".to_string())).await.unwrap().unwrap().try_get::<i64>("","n").unwrap();
    assert_eq!(audit, 0, "unpublished uploads are audit-only");
    let stored=db.query_all(Statement::from_string(DbBackend::Postgres,"SELECT result::text AS data FROM flow_scope_command_receipts UNION ALL SELECT (payload||metadata)::text FROM business_events".to_string())).await.unwrap();
    for row in stored {
        assert!(
            !row.try_get::<String>("", "data").unwrap().contains(&public.token),
            "raw public bearer token must not enter receipts/events"
        );
    }
    db.execute_unprepared(
        "UPDATE flow_runtime_capabilities SET external_mounts=false,personal_writes=false,personal_provisioning=false WHERE singleton",
    )
    .await
    .unwrap();
    assert!(api::flow::scope_shares::read_public(db, &public.token).await.is_ok());
    api::flow::scope_shares::revoke(db, a, public.link.id, owner)
        .await
        .unwrap();
    assert!(api::flow::scope_shares::read_public(db, &public.token).await.is_err());
    assert!(
        api::flow::scope_shares::read_public_asset(db, &public.token, uploaded.asset_id)
            .await
            .is_err()
    );
    assert_eq!(count(db, "workspaces").await, 0);
    assert_eq!(count(db, "workspace_members").await, 0);
    scratch.close().await;
}

#[tokio::test]
async fn personal_inheritance_is_atomic_dry_run_and_request_bound() {
    use api::flow::scope_acl::{self, Mutation};
    let scratch = Scratch::new().await;
    let db = &scratch.db;
    let owner = user(db).await;
    let guest = user(db).await;
    let (scope, _) = scope::personal_home(db, owner).await.unwrap();
    let parent = scope::create_personal(db, owner, create(scope.id, None, "acl-parent"))
        .await
        .unwrap();
    let parent = Uuid::parse_str(parent.field("object").field("id").as_str().unwrap()).unwrap();
    let child = scope::create_personal(db, owner, create(scope.id, Some(parent), "acl-child"))
        .await
        .unwrap();
    let child = Uuid::parse_str(child.field("object").field("id").as_str().unwrap()).unwrap();
    scope_acl::mutate(
        db,
        parent,
        owner,
        Mutation {
            grants: Some(vec![api::flow::grants::GrantRequest {
                principal_kind: "user".into(),
                principal_id: guest,
                level: "view".into(),
            }]),
            inheritance: None,
            dry_run: false,
            confirm_self_lockout: false,
            idempotency_key: "acl-parent-grant".into(),
        },
    )
    .await
    .unwrap();
    assert!(scope::object_view(db, child, guest).await.is_ok());
    let mutation = |inherit, dry, key: &str| Mutation {
        grants: None,
        inheritance: Some(inherit),
        dry_run: dry,
        confirm_self_lockout: false,
        idempotency_key: key.into(),
    };
    let revision = scope::revision(db, scope.id).await.unwrap();
    let preview = scope_acl::mutate(db, child, owner, mutation(false, true, "acl-preview"))
        .await
        .unwrap();
    assert_eq!(*preview.field("applied"), false);
    assert_eq!(scope::revision(db, scope.id).await.unwrap(), revision);
    assert!(scope::object_view(db, child, guest).await.is_ok());
    let applied = scope_acl::mutate(db, child, owner, mutation(false, false, "acl-inherit"))
        .await
        .unwrap();
    let replay = scope_acl::mutate(db, child, owner, mutation(false, false, "acl-inherit"))
        .await
        .unwrap();
    assert_eq!(applied, replay);
    assert!(scope::object_view(db, child, guest).await.is_err());
    assert!(
        scope_acl::mutate(db, child, owner, mutation(true, false, "acl-inherit"))
            .await
            .is_err()
    );
    scope_acl::mutate(db, child, owner, mutation(true, false, "acl-restore"))
        .await
        .unwrap();
    assert!(scope::object_view(db, child, guest).await.is_ok());
    db.execute_unprepared(
        "UPDATE flow_runtime_capabilities SET external_mounts=false,personal_writes=false,personal_provisioning=false WHERE singleton",
    )
    .await
    .unwrap();
    scope_acl::mutate(db, child, owner, mutation(false, false, "acl-contained-revoke"))
        .await
        .unwrap();
    assert!(scope::object_view(db, child, guest).await.is_err());
    assert_eq!(count(db, "workspaces").await, 0);
    scratch.close().await;
}
#[tokio::test]
async fn personal_relations_replay_and_hide_unreadable_endpoint_metadata() {
    use api::flow::relations::{ListRelationsParams, RelationDirection};
    use serde_json::json;
    let scratch = Scratch::new().await;
    let db = &scratch.db;
    let owner = user(db).await;
    let guest = user(db).await;
    let (scope, _) = scope::personal_home(db, owner).await.unwrap();
    let a = scope::create_personal(db, owner, create(scope.id, None, "rel-a"))
        .await
        .unwrap();
    let a = Uuid::parse_str(a.field("object").field("id").as_str().unwrap()).unwrap();
    let mut input = create(scope.id, None, "rel-b");
    input.title = "Private sibling title".into();
    let b = scope::create_personal(db, owner, input).await.unwrap();
    let b = Uuid::parse_str(b.field("object").field("id").as_str().unwrap()).unwrap();
    let state = test_state(db.clone());
    let payload = json!({"target_object_id":b,"relation_type":"references","properties":{"label":"do not expose when unavailable"}});
    let first = api::flow::scope_relations::execute(&state, a, owner, "link", payload.clone(), "rel-link", None)
        .await
        .unwrap();
    let replay = api::flow::scope_relations::execute(&state, a, owner, "link", payload, "rel-link", None)
        .await
        .unwrap();
    assert_eq!(first, replay);
    assert_eq!(count(db, "flow_relations").await, 1);
    assert_eq!(count(db, "collab_updates").await, 0);
    db.execute(Statement::from_sql_and_values(DbBackend::Postgres,"INSERT INTO flow_object_grants(scope_id,workspace_id,object_id,principal_kind,principal_id,level,granted_by) VALUES($1,NULL,$2,'user',$3,'view',$4)",vec![scope.id.into(),a.into(),guest.into(),owner.into()])).await.unwrap();
    let listed = api::flow::scope_relations::list(
        &state,
        a,
        guest,
        ListRelationsParams {
            direction: RelationDirection::Both,
            relation_type: None,
            cursor: None,
            limit: Some(50),
        },
    )
    .await
    .unwrap();
    assert_eq!(*listed.field("items"), json!([{"visibility":"unavailable"}]));
    assert!(!listed.to_string().contains(&b.to_string()));
    assert!(!listed.to_string().contains("Private sibling title"));
    assert!(!listed.to_string().contains("do not expose"));
    let relation = first
        .field("command_result")
        .field("relation")
        .field("relation_id")
        .as_str()
        .unwrap();
    api::flow::scope_relations::execute(
        &state,
        a,
        owner,
        "unlink",
        json!({"relation_id":relation}),
        "rel-unlink",
        None,
    )
    .await
    .unwrap();
    assert_eq!(count(db, "flow_relations").await, 0);
    assert_eq!(count(db, "collab_updates").await, 0);
    drop(state);
    scratch.close().await;
}
#[tokio::test]
async fn legacy_v2_search_distinguishes_project_unprojected_and_all_visible() {
    use api::flow::repository as repo;
    use axum::response::IntoResponse;
    use collab_core::CollabEngine;
    use serde_json::json;
    let scratch = Scratch::new().await;
    let db = &scratch.db;
    let owner = user(db).await;
    let workspace = Uuid::new_v4();
    let project = Uuid::new_v4();
    db.execute(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "INSERT INTO workspaces(id,slug,name,created_by) VALUES($1,$2,'Search selector fixture',$3)",
        vec![workspace.into(), workspace.to_string().into(), owner.into()],
    ))
    .await
    .unwrap();
    db.execute(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "INSERT INTO workspace_members(workspace_id,user_id,role) VALUES($1,$2,'owner')",
        vec![workspace.into(), owner.into()],
    ))
    .await
    .unwrap();
    db.execute(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "INSERT INTO flow_workspace_settings(workspace_id,flow_enabled) VALUES($1,true) ON CONFLICT (workspace_id) DO UPDATE SET flow_enabled = EXCLUDED.flow_enabled",
        vec![workspace.into()],
    ))
    .await
    .unwrap();
    db.execute(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "INSERT INTO projects(id,workspace_id,key,name) VALUES($1,$2,'SEL','Selector project')",
        vec![project.into(), workspace.into()],
    ))
    .await
    .unwrap();
    let scope = scope::legacy_scope(db, workspace).await.unwrap();
    let mut ids = Vec::new();
    for (partition, title) in [(None, "needle unprojected"), (Some(project), "needle project")] {
        let root = repo::ensure_navigator_root(db, workspace, partition).await.unwrap();
        let object = Uuid::new_v4();
        let document = Uuid::new_v4();
        let mut engine = collab_core::LoroCollabEngine::new_empty(1);
        engine.set_title(title).unwrap();
        repo::insert_flow_object(
            db,
            &repo::NewFlowObject {
                id: object,
                workspace_id: workspace,
                project_id: partition,
                object_type: "page".into(),
                parent_id: Some(root),
                created_by: Some(owner),
                governance_metadata: json!({}),
            },
        )
        .await
        .unwrap();
        repo::insert_collab_document(
            db,
            &repo::NewCollabDocument {
                id: document,
                object_id: object,
                format_version: "loro-1".into(),
                snapshot: engine.export_snapshot().unwrap(),
                frontier: engine.frontier().as_bytes().to_vec(),
            },
        )
        .await
        .unwrap();
        repo::insert_projection(
            db,
            &repo::NewProjection {
                object_id: object,
                document_seq: 0,
                document_frontier: engine.frontier().as_bytes().to_vec(),
                title: title.into(),
                state: json!({"nodes":{}}),
                plain_text: "needle".into(),
            },
        )
        .await
        .unwrap();
        ids.push(object);
    }
    let state = test_state(db.clone());
    let unprojected = *ids.first().expect("unprojected fixture");
    let projected = *ids.get(1).expect("project fixture");
    let search = async |query| {
        api::routes::flow::get_flow_v2_search(
            axum::extract::State(state.clone()),
            test_claims(owner),
            None,
            axum::extract::Path(scope.id),
            axum::extract::Query(serde_json::from_value(query).unwrap()),
        )
        .await
    };

    // A missing project index row makes the project and all-visible frontiers stale. The
    // unprojected selector must stay current because its one visible page has been rebuilt.
    let rebuilt = api::flow::maintenance::rebuild_search(db, unprojected, Some(0), true)
        .await
        .unwrap();
    assert!(rebuilt.changed && rebuilt.executed);
    assert!(
        search(json!({"q":"needle","unprojected":true,"object_type":"page","freshness":"require_current"}))
            .await
            .is_ok()
    );
    for selector in [json!({"project_id":project}), json!({"all_visible":true})] {
        let mut query = json!({"q":"needle","object_type":"page","freshness":"require_current"});
        query
            .as_object_mut()
            .unwrap()
            .extend(selector.as_object().unwrap().clone());
        let error = search(query)
            .await
            .expect_err("the selected scope includes an unindexed Page");
        assert_eq!(error.kind(), api::error::ApiErrorKind::StaleFrontier);
    }
    let rebuilt = api::flow::maintenance::rebuild_search(db, projected, Some(0), true)
        .await
        .unwrap();
    assert!(rebuilt.changed && rebuilt.executed);

    for (selector, expected) in [
        (json!({"unprojected":true}), vec![unprojected]),
        (json!({"project_id":project}), vec![projected]),
        (json!({"all_visible":true}), ids.clone()),
    ] {
        let mut query = json!({"q":"needle","object_type":"page","freshness":"require_current"});
        for (key, value) in selector.as_object().unwrap() {
            query
                .as_object_mut()
                .expect("fixture object")
                .insert(key.clone(), value.clone());
        }
        let response = search(query).await.unwrap().into_response();
        let bytes = axum::body::to_bytes(response.into_body(), 1024 * 1024).await.unwrap();
        let body: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        let mut found = body
            .field("data")
            .field("items")
            .as_array()
            .unwrap()
            .iter()
            .map(|row| Uuid::parse_str(row.field("object").field("id").as_str().unwrap()).unwrap())
            .collect::<Vec<_>>();
        found.sort_unstable();
        let mut expected = expected;
        expected.sort_unstable();
        assert_eq!(found, expected);
    }
    let query = serde_json::from_value(json!({"q":"needle","project_id":project,"unprojected":true})).unwrap();
    assert!(
        api::routes::flow::get_flow_v2_search(
            axum::extract::State(state.clone()),
            test_claims(owner),
            None,
            axum::extract::Path(scope.id),
            axum::extract::Query(query)
        )
        .await
        .is_err()
    );
    drop(state);
    scratch.close().await;
}

#[tokio::test]
async fn historical_reads_refuse_unretained_sequences_instead_of_substituting_head() {
    let scratch = Scratch::new().await;
    let owner = user(&scratch.db).await;
    let (scope, _) = scope::personal_home(&scratch.db, owner).await.unwrap();
    let created = scope::create_personal(&scratch.db, owner, create(scope.id, None, "history-retention"))
        .await
        .unwrap();
    let object = Uuid::parse_str(created.field("object").field("id").as_str().unwrap()).unwrap();
    let document = Uuid::parse_str(created.field("object").field("document_id").as_str().unwrap()).unwrap();
    // Simulate a retained checkpoint whose earlier sequence map has expired. No replay worker
    // is invoked for these refusals, so the test also runs on the native non-Linux host.
    scratch
        .db
        .execute(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "UPDATE collab_documents SET head_seq=4,snapshot_seq=4 WHERE id=$1",
            vec![document.into()],
        ))
        .await
        .unwrap();
    let state = test_state(scratch.db.clone());
    let err = api::flow::scope_history::reconstruct(&state, object, owner, 2, 2)
        .await
        .unwrap_err();
    assert!(format!("{err:?}").contains("history_not_retained"));
    assert!(
        api::flow::scope_history::reconstruct(&state, object, owner, 0, 0)
            .await
            .is_err()
    );
    assert!(
        api::flow::scope_history::reconstruct(&state, object, owner, -1, 2)
            .await
            .is_err()
    );
    assert!(
        api::flow::scope_history::reconstruct(&state, object, owner, 2, 5)
            .await
            .is_err()
    );
    assert_eq!(count(&scratch.db, "collab_updates").await, 0);
    drop(state);
    scratch.close().await;
}

#[tokio::test]
async fn page_version_restore_noop_reserves_operation_identity_without_content_dispatch() {
    let scratch = Scratch::new().await;
    let owner = user(&scratch.db).await;
    let (scope, _) = scope::personal_home(&scratch.db, owner).await.unwrap();
    let created = scope::create_personal(&scratch.db, owner, create(scope.id, None, "history-noop-page"))
        .await
        .unwrap();
    let object = Uuid::parse_str(created.field("object").field("id").as_str().unwrap()).unwrap();
    let state = test_state(scratch.db.clone());
    let dispatch = count(&scratch.db, "event_dispatch").await;
    let first = api::flow::scope_history::restore_page(&state, object, owner, 0, "restore-noop", None)
        .await
        .unwrap();
    let retry = api::flow::scope_history::restore_page(&state, object, owner, 0, "restore-noop", None)
        .await
        .unwrap();
    assert_eq!(first, retry);
    assert_eq!(*first.field("changed"), false);
    assert_eq!(*first.field("head_seq"), 0);
    assert_eq!(count(&scratch.db, "event_dispatch").await, dispatch);
    assert_eq!(count(&scratch.db, "collab_updates").await, 0);
    let err = api::flow::scope_history::restore_page(
        &state,
        object,
        owner,
        0,
        "restore-noop",
        Some("different request".into()),
    )
    .await
    .unwrap_err();
    assert!(format!("{err:?}").contains("idempotency_key_body_drift"));
    assert_eq!(count(&scratch.db, "event_dispatch").await, dispatch);
    drop(state);
    scratch.close().await;
}

#[tokio::test]
async fn personal_purge_validates_subtree_and_retains_only_deletion_receipt() {
    let scratch = Scratch::new().await;
    let owner = user(&scratch.db).await;
    let (scope, roots) = scope::personal_home(&scratch.db, owner).await.unwrap();
    let state = test_state(scratch.db.clone());
    assert!(
        api::flow::scope_purge::purge(
            &state,
            roots.first().expect("fixture element").object_id,
            owner,
            "purge-root"
        )
        .await
        .is_err()
    );
    let parent = scope::create_personal(&scratch.db, owner, create(scope.id, None, "purge-parent"))
        .await
        .unwrap();
    let parent = Uuid::parse_str(parent.field("object").field("id").as_str().unwrap()).unwrap();
    let child = scope::create_personal(&scratch.db, owner, create(scope.id, Some(parent), "purge-child"))
        .await
        .unwrap();
    let child = Uuid::parse_str(child.field("object").field("id").as_str().unwrap()).unwrap();
    scratch
        .db
        .execute(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "UPDATE flow_objects SET lifecycle_status='archived',archived_at=now() WHERE id=$1",
            vec![parent.into()],
        ))
        .await
        .unwrap();
    let before = count(&scratch.db, "flow_objects").await;
    assert!(
        api::flow::scope_purge::purge(&state, parent, owner, "purge-tree")
            .await
            .is_err()
    );
    assert_eq!(count(&scratch.db, "flow_objects").await, before);
    assert_eq!(count(&scratch.db, "flow_purge_receipts").await, 0);
    scratch
        .db
        .execute(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "UPDATE flow_objects SET lifecycle_status='archived',archived_at=now() WHERE id=$1",
            vec![child.into()],
        ))
        .await
        .unwrap();
    let first = api::flow::scope_purge::purge(&state, parent, owner, "purge-tree")
        .await
        .unwrap();
    assert_eq!(*first.field("deleted_count"), 2);
    assert_eq!(
        api::flow::scope_purge::purge(&state, parent, owner, "purge-tree")
            .await
            .unwrap(),
        first
    );
    assert_eq!(count(&scratch.db, "flow_objects").await, before - 2);
    assert_eq!(count(&scratch.db, "flow_purge_receipts").await, 1);
    assert_eq!(count(&scratch.db, "flow_scope_command_receipts").await, 0);
    assert!(scope::identity(&scratch.db, child).await.is_err());
    let body = serde_json::json!({"operation":"something_else"});
    assert!(
        api::flow::scope_commands::replay(&scratch.db, scope.id, owner, "purge-tree", &body)
            .await
            .is_err()
    );
    assert_eq!(count(&scratch.db, "workspaces").await, 0);
    drop(state);
    scratch.close().await;
}

#[tokio::test]
async fn destination_admin_removes_only_placement_and_can_replay_without_source_access() {
    let scratch = Scratch::new().await;
    let db = &scratch.db;
    let source_owner = user(db).await;
    let destination_owner = user(db).await;
    let (personal, _) = scope::personal_home(db, source_owner).await.unwrap();
    let created = scope::create_personal(db, source_owner, create(personal.id, None, "mounted-page"))
        .await
        .unwrap();
    let object = Uuid::parse_str(created.field("object").field("id").as_str().unwrap()).unwrap();
    let workspace = Uuid::new_v4();
    let mount = Uuid::new_v4();
    db.execute(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "INSERT INTO workspaces(id,slug,name,created_by) VALUES($1,$2,'Destination',$3)",
        vec![workspace.into(), workspace.to_string().into(), destination_owner.into()],
    ))
    .await
    .unwrap();
    db.execute(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "INSERT INTO workspace_members(workspace_id,user_id,role) VALUES($1,$2,'owner')",
        vec![workspace.into(), destination_owner.into()],
    ))
    .await
    .unwrap();
    db.execute(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "INSERT INTO flow_workspace_settings(workspace_id,flow_enabled) VALUES($1,true) ON CONFLICT (workspace_id) DO UPDATE SET flow_enabled = EXCLUDED.flow_enabled",
        vec![workspace.into()],
    ))
    .await
    .unwrap();
    // Fixture-only preexisting placement: the external publishing rollout gate stays disabled.
    db.execute(Statement::from_sql_and_values(DbBackend::Postgres,"INSERT INTO flow_mounts(id,source_object_id,destination_workspace_id,grant_level,created_by) VALUES($1,$2,$3,'view',$4)",vec![mount.into(),object.into(),workspace.into(),source_owner.into()])).await.unwrap();
    assert!(
        api::flow::scope_mounts::can_remove(db, workspace, object, destination_owner)
            .await
            .unwrap()
    );
    let objects = count(db, "flow_objects").await;
    api::flow::scope_mounts::remove(db, destination_owner, object, mount)
        .await
        .unwrap();
    assert_eq!(count(db, "flow_mounts").await, 0);
    assert_eq!(count(db, "flow_objects").await, objects);
    api::flow::scope_mounts::remove(db, destination_owner, object, mount)
        .await
        .unwrap();
    assert_eq!(count(db, "flow_objects").await, objects);
    assert_eq!(scope::identity(db, object).await.unwrap().scope_id, personal.id);
    scratch.close().await;
}

#[tokio::test]
async fn personal_forms_reference_uses_real_target_workspace_and_scope_receipts() {
    use api::flow::bridge::{self, CreateReferenceInput};
    use serde_json::json;
    let scratch = Scratch::new().await;
    let db = &scratch.db;
    let owner = user(db).await;
    let (personal, _) = scope::personal_home(db, owner).await.unwrap();
    let page = scope::create_personal(db, owner, create(personal.id, None, "forms-source"))
        .await
        .unwrap();
    let object = Uuid::parse_str(page.field("object").field("id").as_str().unwrap()).unwrap();
    let workspace = Uuid::new_v4();
    let project = Uuid::new_v4();
    let form = Uuid::new_v4();
    db.execute(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "INSERT INTO workspaces(id,slug,name,created_by) VALUES($1,$2,'Forms destination',$3)",
        vec![workspace.into(), workspace.to_string().into(), owner.into()],
    ))
    .await
    .unwrap();
    db.execute(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "INSERT INTO workspace_members(workspace_id,user_id,role) VALUES($1,$2,'owner')",
        vec![workspace.into(), owner.into()],
    ))
    .await
    .unwrap();
    db.execute(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "INSERT INTO flow_workspace_settings(workspace_id,flow_enabled,bridge_enabled) VALUES($1,true,true) ON CONFLICT (workspace_id) DO UPDATE SET flow_enabled = EXCLUDED.flow_enabled, bridge_enabled = EXCLUDED.bridge_enabled",
        vec![workspace.into()],
    ))
    .await
    .unwrap();
    db.execute(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "INSERT INTO projects(id,workspace_id,key,name,created_by) VALUES($1,$2,'FREF','Forms destination',$3)",
        vec![project.into(), workspace.into(), owner.into()],
    ))
    .await
    .unwrap();
    db.execute(Statement::from_sql_and_values(DbBackend::Postgres,"INSERT INTO project_forms(id,workspace_id,project_id,key,name,created_by) VALUES($1,$2,$3,'source_ref','Reference target',$4)",vec![form.into(),workspace.into(),project.into(),owner.into()])).await.unwrap();
    let state = test_state(db.clone());
    let input = || CreateReferenceInput {
        target_type: "form".into(),
        target_id: form,
        display: json!({"mode":"card"}),
        idempotency_key: "forms-reference".into(),
    };
    let first = bridge::create_scope_reference(&state, owner, object, input())
        .await
        .unwrap();
    assert_eq!(
        bridge::create_scope_reference(&state, owner, object, input())
            .await
            .unwrap(),
        first
    );
    let reference = Uuid::parse_str(first.field("reference_id").as_str().unwrap()).unwrap();
    let row = db
        .query_one(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT workspace_id,scope_id FROM flow_bridge_references WHERE id=$1",
            vec![reference.into()],
        ))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(row.try_get::<Uuid>("", "workspace_id").unwrap(), workspace);
    assert_eq!(row.try_get::<Uuid>("", "scope_id").unwrap(), personal.id);
    assert!(scope::identity(db, object).await.unwrap().workspace_id.is_none());
    let visible = bridge::list_scope_references(&state, owner, object).await.unwrap();
    assert_eq!(
        serde_json::to_value(visible)
            .unwrap()
            .field("items")
            .as_array()
            .unwrap()
            .len(),
        1
    );
    // The record lineage must resolve the canonical personal source, not the destination
    // workspace. A destination admin without a source grant receives no source identifiers.
    let record = Uuid::new_v4();
    db.execute(Statement::from_sql_and_values(DbBackend::Postgres,"INSERT INTO form_records(id,workspace_id,project_id,form_id,title,created_by) VALUES($1,$2,$3,$4,'Record target',$5)",vec![record.into(),workspace.into(),project.into(),form.into(),owner.into()])).await.unwrap();
    bridge::create_scope_reference(
        &state,
        owner,
        object,
        CreateReferenceInput {
            target_type: "form_record".into(),
            target_id: record,
            display: json!({}),
            idempotency_key: "record-reference".into(),
        },
    )
    .await
    .unwrap();
    let mut auth = axum::http::Extensions::new();
    auth.insert(test_claims(owner).0);
    let lineage = serde_json::to_value(
        bridge::record_flow_lineage(&state, &auth, workspace, record)
            .await
            .unwrap(),
    )
    .unwrap();
    assert_eq!(
        lineage
            .field("references")
            .as_array()
            .expect("fixture array")
            .first()
            .expect("fixture element")
            .field("source_available"),
        true
    );
    assert_eq!(
        *lineage
            .field("references")
            .as_array()
            .expect("fixture array")
            .first()
            .expect("fixture element")
            .field("source_scope_id"),
        json!(personal.id)
    );
    assert!(
        lineage
            .field("references")
            .as_array()
            .expect("fixture array")
            .first()
            .expect("fixture element")
            .get("source_workspace_id")
            .is_none()
    );
    let other = user(db).await;
    db.execute(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "INSERT INTO workspace_members(workspace_id,user_id,role) VALUES($1,$2,'admin')",
        vec![workspace.into(), other.into()],
    ))
    .await
    .unwrap();
    let mut auth = axum::http::Extensions::new();
    auth.insert(test_claims(other).0);
    let denied = serde_json::to_value(
        bridge::record_flow_lineage(&state, &auth, workspace, record)
            .await
            .unwrap(),
    )
    .unwrap();
    assert_eq!(
        denied
            .field("references")
            .as_array()
            .expect("fixture array")
            .first()
            .expect("fixture element")
            .field("source_available"),
        false
    );
    assert!(!denied.to_string().contains(&object.to_string()));
    assert!(!denied.to_string().contains(&personal.id.to_string()));
    let removed = bridge::remove_scope_reference(&state, owner, object, reference, "remove-forms-reference")
        .await
        .unwrap();
    assert_eq!(
        bridge::remove_scope_reference(&state, owner, object, reference, "remove-forms-reference")
            .await
            .unwrap(),
        removed
    );
    assert_eq!(count(db, "workspaces").await, 1);
    assert_eq!(count(db, "workspace_members").await, 2);
    drop(state);
    scratch.close().await;
}

#[tokio::test]
async fn personal_page_package_has_scope_provenance_real_snapshots_and_stable_job() {
    use serde_json::json;
    let scratch = Scratch::new().await;
    let db = &scratch.db;
    let owner = user(db).await;
    let (personal, _) = scope::personal_home(db, owner).await.unwrap();
    let page = scope::create_personal(db, owner, create(personal.id, None, "package-page"))
        .await
        .unwrap();
    let object = Uuid::parse_str(page.field("object").field("id").as_str().unwrap()).unwrap();
    let child = scope::create_personal(db, owner, create(personal.id, Some(object), "package-child"))
        .await
        .unwrap();
    let child = Uuid::parse_str(child.field("object").field("id").as_str().unwrap()).unwrap();
    let first = api::flow::scope_export::create(db, object, owner, "package", None, false, "scope-package")
        .await
        .unwrap();
    assert_eq!(
        api::flow::scope_export::create(db, object, owner, "package", None, false, "scope-package")
            .await
            .unwrap(),
        first
    );
    let job = Uuid::parse_str(first.field("job_id").as_str().unwrap()).unwrap();
    let (bytes, hash, format) = api::flow::scope_export::download(db, job, owner).await.unwrap();
    assert_eq!(format, "package");
    let verified = api::flow::package::verify_package(std::io::Cursor::new(bytes.clone()), Some(&hash)).unwrap();
    assert_eq!(verified.manifest.schema, api::flow::package::SCOPE_PACKAGE_SCHEMA);
    assert_eq!(
        api::flow::package::semantic_model(&verified.manifest).unwrap(),
        api::flow::package::PackageSemanticModel::V4,
    );
    assert_eq!(verified.manifest.source.scope_id, Some(personal.id.to_string()));
    assert_eq!(verified.manifest.source.workspace_id, None);
    assert_eq!(verified.manifest.counts.objects, 1);
    assert!(!verified.manifest.history.included);
    let mut zip = zip::ZipArchive::new(std::io::Cursor::new(bytes)).unwrap();
    assert!(zip.by_name(&format!("objects/{child}/object.json")).is_err());
    {
        let id = object;
        let metadata: serde_json::Value =
            serde_json::from_reader(zip.by_name(&format!("objects/{id}/object.json")).unwrap()).unwrap();
        assert_eq!(*metadata.field("source_scope_id"), json!(personal.id));
        assert!(metadata.get("source_workspace_id").is_none());
        let doc = metadata.field("source_document_id").as_str().unwrap();
        let mut data = Vec::new();
        std::io::Read::read_to_end(
            &mut zip.by_name(&format!("documents/{doc}/snapshot.bin")).unwrap(),
            &mut data,
        )
        .unwrap();
        let engine = <collab_core::LoroCollabEngine as collab_core::CollabEngine>::load(&data).unwrap();
        assert_eq!(
            engine.package_semantic_snapshot_v4().unwrap().semantic_hash(),
            metadata.field("semantic_hash").as_str().unwrap()
        );
    }
    assert_eq!(count(db, "workspaces").await, 0);
    assert_eq!(count(db, "workspace_members").await, 0);
    scratch.close().await;
}

#[tokio::test]
async fn editor_admission_is_explicit_and_initialized_noop_replays_without_head_change() {
    use collab_core::CollabEngine;
    use serde_json::json;
    let scratch = Scratch::new().await;
    let db = &scratch.db;
    let owner = user(db).await;
    let (personal, _) = scope::personal_home(db, owner).await.unwrap();
    let created = scope::create_personal(db, owner, create(personal.id, None, "editor-init-ready"))
        .await
        .unwrap();
    let object = Uuid::parse_str(created.field("object").field("id").as_str().unwrap()).unwrap();
    let expected = created.field("object").field("frontier").as_str().unwrap();
    let first = api::flow::scope_editor::initialize(db, object, owner, json!({}), expected, "editor-noop")
        .await
        .unwrap();
    assert_eq!(
        api::flow::scope_editor::initialize(db, object, owner, json!({}), expected, "editor-noop")
            .await
            .unwrap(),
        first
    );
    assert_eq!(count(db, "collab_updates").await, 0);
    assert!(
        api::flow::scope_editor::initialize(
            db,
            object,
            owner,
            json!({"blocks":[]}),
            expected,
            "editor-client-blocks"
        )
        .await
        .is_err()
    );
    let mut engine = collab_core::LoroCollabEngine::new_empty(21);
    engine.set_title("Legacy page").unwrap();
    engine
        .apply_operation(&collab_core::Operation::CreateNode {
            id: "old-block".into(),
            parent: None,
            index: 0,
            kind: collab_core::NodeKind::Block,
        })
        .unwrap();
    engine
        .apply_operation(&collab_core::Operation::InsertText {
            id: "old-block".into(),
            index: 0,
            text: "Readable legacy content".into(),
        })
        .unwrap();
    let bytes = engine.export_snapshot().unwrap();
    let frontier = engine.frontier().as_bytes().to_vec();
    let doc = Uuid::parse_str(created.field("object").field("document_id").as_str().unwrap()).unwrap();
    db.execute(Statement::from_sql_and_values(DbBackend::Postgres,"UPDATE collab_documents SET snapshot=$2,snapshot_checksum=$3,snapshot_frontier=$4,head_frontier=$4 WHERE id=$1",vec![doc.into(),bytes.clone().into(),api::flow::collab::bootstrap::content_hash(&bytes).into(),frontier.clone().into()])).await.unwrap();
    let mut view = scope::object_view(db, object, owner).await.unwrap();
    api::flow::scope_editor::decorate(db, object, owner, &mut view)
        .await
        .unwrap();
    assert_eq!(*view.field("editor_state"), "migration_required");
    assert_eq!(*view.field("editor_initialization_supported"), true);
    let err = api::flow::scope_write::bootstrap(db, object, owner).await.unwrap_err();
    assert!(format!("{err:?}").contains("editor_migration_required"));
    db.execute_unprepared(
        "UPDATE flow_runtime_capabilities SET external_mounts=false,personal_writes=false WHERE singleton",
    )
    .await
    .unwrap();
    assert!(
        api::flow::scope_editor::initialize(
            db,
            object,
            owner,
            json!({}),
            &base64::Engine::encode(&base64::engine::general_purpose::STANDARD, &frontier),
            "editor-disabled"
        )
        .await
        .is_err()
    );
    assert_eq!(count(db, "collab_updates").await, 0);
    assert_eq!(
        api::flow::document::load_head_engine(db, doc)
            .await
            .unwrap()
            .semantic_snapshot()
            .unwrap()
            .nodes
            .get("old-block")
            .unwrap()
            .text,
        "Readable legacy content"
    );
    scratch.close().await;
}

#[tokio::test]
async fn stale_ordinal_projection_refreshes_targeted_get_and_export_without_content_writes() {
    use axum::response::IntoResponse;
    use collab_core::CollabEngine;
    use serde_json::json;
    let scratch = Scratch::new().await;
    let db = &scratch.db;
    let owner = user(db).await;
    let (personal, _) = scope::personal_home(db, owner).await.unwrap();
    let created = scope::create_personal(db, owner, create(personal.id, None, "old-projection"))
        .await
        .unwrap();
    let object = Uuid::parse_str(created.field("object").field("id").as_str().unwrap()).unwrap();
    let doc = Uuid::parse_str(created.field("object").field("document_id").as_str().unwrap()).unwrap();
    let bytes = std::fs::read(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../frontend/tests/fixtures/editor-doc-v1/browser.snapshot"
    ))
    .unwrap();
    let engine = collab_core::LoroCollabEngine::load(&bytes).unwrap();
    let frontier = engine.frontier().as_bytes().to_vec();
    db.execute(Statement::from_sql_and_values(DbBackend::Postgres,"UPDATE collab_documents SET snapshot=$2,snapshot_checksum=$3,snapshot_frontier=$4,head_frontier=$4 WHERE id=$1",vec![doc.into(),bytes.clone().into(),api::flow::collab::bootstrap::content_hash(&bytes).into(),frontier.clone().into()])).await.unwrap();
    let old = json!({"nodes":{"__editor_prose__:0":{"parent":null,"order_key":"00000000","kind":"block","text":"Browser 😀 prose","properties":{"content_source":"editor_doc"},"deleted":false}}});
    db.execute(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "UPDATE flow_object_projections SET state=$2,document_frontier=$3 WHERE object_id=$1",
        vec![object.into(), old.clone().into(), frontier.into()],
    ))
    .await
    .unwrap();
    let state = test_state(db.clone());
    let response = api::routes::flow::get_flow_v2_object(
        axum::extract::State(state.clone()),
        test_claims(owner),
        None,
        axum::extract::Path(object),
        axum::extract::Query(serde_json::from_value(json!({})).unwrap()),
    )
    .await
    .unwrap()
    .into_response();
    let body = String::from_utf8(
        axum::body::to_bytes(response.into_body(), 1024 * 1024)
            .await
            .unwrap()
            .to_vec(),
    )
    .unwrap();
    assert!(body.contains("cid:"));
    assert!(!body.contains("__editor_prose__:"));
    let export = api::flow::scope_export::create(db, object, owner, "json", None, false, "fresh-projection-export")
        .await
        .unwrap();
    let job = Uuid::parse_str(export.field("job_id").as_str().unwrap()).unwrap();
    let (artifact, _, _) = api::flow::scope_export::download(db, job, owner).await.unwrap();
    let artifact = String::from_utf8(artifact).unwrap();
    assert!(artifact.contains("cid:"));
    assert!(!artifact.contains("__editor_prose__:"));
    let stored = db
        .query_one(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT state FROM flow_object_projections WHERE object_id=$1",
            vec![object.into()],
        ))
        .await
        .unwrap()
        .unwrap()
        .try_get::<serde_json::Value>("", "state")
        .unwrap();
    assert_eq!(stored, old);
    assert_eq!(count(db, "collab_updates").await, 0);
    let retained = db
        .query_one(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT snapshot FROM collab_documents WHERE id=$1",
            vec![doc.into()],
        ))
        .await
        .unwrap()
        .unwrap()
        .try_get::<Vec<u8>>("", "snapshot")
        .unwrap();
    assert_eq!(retained, bytes);
    drop(state);
    scratch.close().await;
}

#[tokio::test]
async fn new_legacy_page_is_editor_ready_while_existing_conversion_cutover_is_off() {
    let scratch = Scratch::new().await;
    let db = &scratch.db;
    let owner = user(db).await;
    let workspace = Uuid::new_v4();
    db.execute(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "INSERT INTO workspaces(id,slug,name,created_by) VALUES($1,$2,'Legacy editor ready',$3)",
        vec![workspace.into(), workspace.to_string().into(), owner.into()],
    ))
    .await
    .unwrap();
    db.execute(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "INSERT INTO workspace_members(workspace_id,user_id,role) VALUES($1,$2,'owner')",
        vec![workspace.into(), owner.into()],
    ))
    .await
    .unwrap();
    db.execute(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "INSERT INTO flow_workspace_settings(workspace_id,flow_enabled) VALUES($1,true) ON CONFLICT (workspace_id) DO UPDATE SET flow_enabled = EXCLUDED.flow_enabled",
        vec![workspace.into()],
    ))
    .await
    .unwrap();
    scope::legacy_scope(db, workspace).await.unwrap();
    assert!(
        scope::capabilities(db).await.unwrap().scope_native_legacy,
        "legacy editor initialization is on by default"
    );
    db.execute_unprepared("UPDATE flow_runtime_capabilities SET scope_native_legacy=false WHERE singleton")
        .await
        .unwrap();
    let state = test_state(db.clone());
    let created = api::flow::command::create_object(
        &state,
        api::flow::command::CreateObjectInput {
            workspace_id: workspace,
            actor_id: owner,
            actor_is_bot: false,
            object_type: "page".into(),
            project_id: None,
            parent_object_id: None,
            title: "New legacy page".into(),
            idempotency_key: "new-legacy-editor".into(),
            message: None,
            origin: api::flow::event_origin::CommandOrigin::first_request_from(
                api::flow::event_origin::EventSurface::Rest,
            ),
        },
    )
    .await
    .unwrap();
    api::flow::scope_editor::require_admitted(db, created.object.id)
        .await
        .unwrap();
    let bootstrap = api::flow::scope_write::bootstrap(db, created.object.id, owner)
        .await
        .unwrap();
    assert_eq!(*bootstrap.field("head_seq"), 0);
    let mut view = scope::object_view(db, created.object.id, owner).await.unwrap();
    api::flow::scope_editor::decorate(db, created.object.id, owner, &mut view)
        .await
        .unwrap();
    assert_eq!(*view.field("editor_state"), "ready");
    assert_eq!(*view.field("editor_initialization_supported"), false);
    assert_eq!(count(db, "flow_scopes").await, 1);
    assert_eq!(count(db, "workspaces").await, 1);
    drop(state);
    scratch.close().await;
}

/// The default state: `scope_native_legacy` is on, so a Page an agent created in a workspace with an
/// older editor model can be upgraded to the rich editor, and the switch still closes the path.
#[tokio::test]
async fn workspace_page_upgrades_to_the_rich_editor_with_scope_native_legacy_on() {
    use collab_core::CollabEngine;
    use serde_json::json;
    let scratch = Scratch::new().await;
    let db = &scratch.db;
    let owner = user(db).await;
    let workspace = Uuid::new_v4();
    db.execute(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "INSERT INTO workspaces(id,slug,name,created_by) VALUES($1,$2,'Legacy upgrade',$3)",
        vec![workspace.into(), workspace.to_string().into(), owner.into()],
    ))
    .await
    .unwrap();
    db.execute(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "INSERT INTO workspace_members(workspace_id,user_id,role) VALUES($1,$2,'owner')",
        vec![workspace.into(), owner.into()],
    ))
    .await
    .unwrap();
    assert!(
        workspace_flow_enabled(db, workspace).await,
        "the workspace trigger left Flow on"
    );
    scope::legacy_scope(db, workspace).await.unwrap();
    assert!(scope::capabilities(db).await.unwrap().scope_native_legacy);
    let state = test_state(db.clone());
    let created = api::flow::command::create_object(
        &state,
        api::flow::command::CreateObjectInput {
            workspace_id: workspace,
            actor_id: owner,
            actor_is_bot: false,
            object_type: "page".into(),
            project_id: None,
            parent_object_id: None,
            title: "Agent page".into(),
            idempotency_key: "agent-page".into(),
            message: None,
            origin: api::flow::event_origin::CommandOrigin::first_request_from(
                api::flow::event_origin::EventSurface::Rest,
            ),
        },
    )
    .await
    .unwrap();
    let object = created.object.id;
    let mut engine = collab_core::LoroCollabEngine::new_empty(21);
    engine.set_title("Agent page").unwrap();
    engine
        .apply_operation(&collab_core::Operation::CreateNode {
            id: "old-block".into(),
            parent: None,
            index: 0,
            kind: collab_core::NodeKind::Block,
        })
        .unwrap();
    let bytes = engine.export_snapshot().unwrap();
    let frontier = engine.frontier().as_bytes().to_vec();
    let doc: Uuid = db
        .query_one(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT id FROM collab_documents WHERE object_id=$1",
            vec![object.into()],
        ))
        .await
        .unwrap()
        .unwrap()
        .try_get("", "id")
        .unwrap();
    db.execute(Statement::from_sql_and_values(DbBackend::Postgres,"UPDATE collab_documents SET snapshot=$2,snapshot_checksum=$3,snapshot_frontier=$4,head_frontier=$4 WHERE id=$1",vec![doc.into(),bytes.clone().into(),api::flow::collab::bootstrap::content_hash(&bytes).into(),frontier.clone().into()])).await.unwrap();
    let mut view = scope::object_view(db, object, owner).await.unwrap();
    api::flow::scope_editor::decorate(db, object, owner, &mut view)
        .await
        .unwrap();
    assert_eq!(*view.field("editor_state"), "migration_required");
    assert_eq!(*view.field("editor_initialization_supported"), true);
    let expected = base64::Engine::encode(&base64::engine::general_purpose::STANDARD, &frontier);

    db.execute_unprepared("UPDATE flow_runtime_capabilities SET scope_native_legacy=false WHERE singleton")
        .await
        .unwrap();
    let refused = api::flow::scope_editor::initialize(db, object, owner, json!({}), &expected, "agent-upgrade-off")
        .await
        .unwrap_err();
    assert!(format!("{refused:?}").contains("scope_native_legacy_disabled"));
    db.execute_unprepared("UPDATE flow_runtime_capabilities SET scope_native_legacy=true WHERE singleton")
        .await
        .unwrap();
    api::flow::scope_editor::initialize(db, object, owner, json!({}), &expected, "agent-upgrade")
        .await
        .unwrap();
    let mut upgraded = scope::object_view(db, object, owner).await.unwrap();
    api::flow::scope_editor::decorate(db, object, owner, &mut upgraded)
        .await
        .unwrap();
    assert_eq!(*upgraded.field("editor_state"), "ready");
    drop(state);
    scratch.close().await;
}

async fn workspace_flow_enabled(db: &DatabaseConnection, workspace: Uuid) -> bool {
    db.query_one(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "SELECT flow_enabled FROM flow_workspace_settings WHERE workspace_id=$1",
        vec![workspace.into()],
    ))
    .await
    .unwrap()
    .is_some_and(|row| row.try_get::<bool>("", "flow_enabled").unwrap())
}

#[tokio::test]
#[allow(
    clippy::print_stderr,
    reason = "report the platform skip without a tracing subscriber"
)]
async fn pre_change_v1_browser_package_promotes_with_old_fingerprint_and_current_projection() {
    use api::flow::package_import::*;
    use serde_json::json;
    if !cfg!(target_os = "linux") {
        eprintln!("NOT EXECUTED: the import validates in the isolated worker, which requires Linux");
        return;
    }
    let scratch = Scratch::new().await;
    let db = &scratch.db;
    let owner = user(db).await;
    let workspace = Uuid::new_v4();
    db.execute(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "INSERT INTO workspaces(id,slug,name,created_by) VALUES($1,$2,'Package destination',$3)",
        vec![workspace.into(), workspace.to_string().into(), owner.into()],
    ))
    .await
    .unwrap();
    db.execute(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "INSERT INTO workspace_members(workspace_id,user_id,role) VALUES($1,$2,'owner')",
        vec![workspace.into(), owner.into()],
    ))
    .await
    .unwrap();
    db.execute(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "INSERT INTO flow_workspace_settings(workspace_id,flow_enabled) VALUES($1,true) ON CONFLICT (workspace_id) DO UPDATE SET flow_enabled = EXCLUDED.flow_enabled",
        vec![workspace.into()],
    ))
    .await
    .unwrap();
    scope::legacy_scope(db, workspace).await.unwrap();
    let path = std::path::PathBuf::from(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/flow-v2/browser-package-v4"
    ));
    let bytes = std::fs::read(path.join("package.zip")).unwrap();
    let contract: serde_json::Value =
        serde_json::from_slice(&std::fs::read(path.join("contract.json")).unwrap()).unwrap();
    let principal = ImportPrincipal {
        id: owner,
        kind: "user".into(),
        role: "owner".into(),
    };
    let (artifact, hash, _) = upload_package_artifact(
        db,
        workspace,
        &principal,
        bytes,
        Some(contract.field("package_sha256").as_str().unwrap()),
        "old-browser-upload",
    )
    .await
    .unwrap();
    let preview = preview_package_import(
        db,
        &PreviewImportRequest {
            workspace_id: workspace,
            artifact_id: artifact,
            project_map: std::collections::BTreeMap::new(),
            external_reference_policy: ExternalReferencePolicy::Reject,
            conflict_policy: ConflictPolicy::RejectExisting,
            include_history: false,
            idempotency_key: "old-browser-preview".into(),
            principal: principal.clone(),
        },
    )
    .await
    .unwrap();
    let request = CommitImportRequest {
        workspace_id: workspace,
        preview_id: preview.preview_id,
        package_sha256: hash,
        mapping_hash: preview.mapping_hash,
        conflict_policy: ConflictPolicy::RejectExisting,
        confirm: true,
        idempotency_key: "old-browser-commit".into(),
        principal,
    };
    let report = commit_package_import(db, &request).await.unwrap();
    assert_eq!(
        commit_package_import(db, &request).await.unwrap().import_id,
        report.import_id
    );
    let target = *report.object_mapping.values().next().unwrap();
    let view = scope::object_view(db, target, owner).await.unwrap();
    assert!(view.field("semantic_content").to_string().contains("Browser 😀 prose"));
    assert!(
        view.field("semantic_content")
            .field("nodes")
            .as_object()
            .unwrap()
            .keys()
            .any(|id| id.starts_with("cid:"))
    );
    assert_eq!(
        *view.field("semantic_content").field("__flow_projection_model"),
        json!(api::flow::projection::PROJECTION_MODEL_VERSION)
    );
    assert_eq!(count(db, "workspaces").await, 1);
    scratch.close().await;
}

#[tokio::test]
async fn legacy_raw_typed_packages_refuse_and_explicit_scope_kind_selection_records_warnings() {
    use api::flow::{command, export};
    use serde_json::json;
    let scratch = Scratch::new().await;
    let db = &scratch.db;
    let owner = user(db).await;
    let workspace = Uuid::new_v4();
    db.execute(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "INSERT INTO workspaces(id,slug,name,created_by) VALUES($1,$2,'Package kinds',$3)",
        vec![workspace.into(), workspace.to_string().into(), owner.into()],
    ))
    .await
    .unwrap();
    db.execute(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "INSERT INTO workspace_members(workspace_id,user_id,role) VALUES($1,$2,'owner')",
        vec![workspace.into(), owner.into()],
    ))
    .await
    .unwrap();
    db.execute(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "INSERT INTO flow_workspace_settings(workspace_id,flow_enabled) VALUES($1,true) ON CONFLICT (workspace_id) DO UPDATE SET flow_enabled = EXCLUDED.flow_enabled",
        vec![workspace.into()],
    ))
    .await
    .unwrap();
    scope::legacy_scope(db, workspace).await.unwrap();
    let state = test_state(db.clone());
    let create_input = |kind: &str| command::CreateObjectInput {
        workspace_id: workspace,
        actor_id: owner,
        actor_is_bot: false,
        object_type: kind.into(),
        project_id: None,
        parent_object_id: None,
        title: format!("{kind} export"),
        idempotency_key: format!("create-{kind}"),
        message: None,
        origin: api::flow::event_origin::CommandOrigin::first_request_from(api::flow::event_origin::EventSurface::Rest),
    };
    let page = command::create_object(&state, create_input("page"))
        .await
        .unwrap()
        .object
        .id;
    let collection=command::create_object_with_collection_schema(&state,create_input("collection"),json!({"initial_fields":[{"field_id":Uuid::new_v4(),"label":"Public field","field_type":"text","config":{},"index":0}]})).await.unwrap().object.id;
    let principal = export::ExportPrincipal {
        id: owner,
        kind: "user".into(),
        role: "owner".into(),
        workspace_export_capability: false,
    };
    let request = |scope, types, key: &str| export::CreateExportRequest {
        scope,
        object_types: types,
        format: "package".into(),
        at_seq: None,
        include_history: false,
        idempotency_key: key.into(),
        source_head: "d523d7c724b4122f7b83cb30e6614a36f51a1c65".into(),
        principal: principal.clone(),
    };
    for target in [
        export::ExportScope::Object(collection),
        export::ExportScope::Workspace {
            workspace_id: workspace,
            project_id: None,
        },
    ] {
        let err = export::create_package_export(db, &request(target, None, "typed-raw-refused"))
            .await
            .unwrap_err();
        assert!(format!("{err:?}").contains("typed_package_roundtrip_unsupported"));
    }
    assert_eq!(count(db, "flow_export_jobs").await, 0);
    let target = export::ExportScope::Workspace {
        workspace_id: workspace,
        project_id: None,
    };
    let first = export::create_package_export(
        db,
        &request(target, Some(vec!["page".into(), "navigator".into()]), "selected-kinds"),
    )
    .await
    .unwrap();
    assert_eq!(
        first.warnings,
        vec![
            "object_type_filter_applied:navigator,page",
            "object_types_omitted:collection"
        ]
    );
    let retry = export::create_package_export(
        db,
        &request(
            target,
            Some(vec!["navigator".into(), "page".into(), "page".into()]),
            "selected-kinds",
        ),
    )
    .await
    .unwrap();
    assert_eq!(retry.job_id, first.job_id);
    assert_eq!(retry.warnings, first.warnings);
    assert_eq!(
        export::get_package_export(db, first.job_id, &principal)
            .await
            .unwrap()
            .warnings,
        first.warnings
    );
    let (bytes, hash, _) = export::download_package_export(db, first.job_id, &principal)
        .await
        .unwrap();
    let verified = api::flow::package::verify_package(std::io::Cursor::new(bytes), Some(&hash)).unwrap();
    assert_eq!(verified.manifest.counts.objects, 1);
    assert_eq!(verified.manifest.source.root_object_ids, vec![page.to_string()]);
    assert_eq!(verified.manifest.warnings, first.warnings);
    let view = scope::object_view(db, collection, owner).await.unwrap();
    assert!(
        !view
            .field("supported_export_formats")
            .as_array()
            .unwrap()
            .iter()
            .any(|v| v == "package")
    );
    assert!(
        export::create_package_export(db, &request(target, Some(vec!["page".into()]), "selected-kinds"))
            .await
            .is_err()
    );
    drop(state);
    scratch.close().await;
}

#[tokio::test]
async fn personal_import_transport_stages_real_archive_and_obeys_workflow_and_platform_admission() {
    use axum::{
        Json,
        extract::{Path, State},
        response::IntoResponse,
    };
    use base64::Engine;
    use serde_json::json;

    let scratch = Scratch::new().await;
    let db = &scratch.db;
    let owner = user(db).await;
    let (scope, _) = scope::personal_home(db, owner).await.unwrap();
    let state = test_state(db.clone());
    db.execute_unprepared("UPDATE flow_runtime_capabilities SET personal_package_imports=false WHERE singleton")
        .await
        .unwrap();
    let request = axum::http::Request::builder()
        .header("content-type", "application/json")
        .body(axum::body::Body::from(
            "invalid JSON must not be parsed while workflow is disabled",
        ))
        .unwrap();
    let err = api::routes::flow::post_flow_v2_import_artifact(
        State(state.clone()),
        test_claims(owner),
        None,
        Path(scope.id),
        request,
    )
    .await
    .unwrap_err();
    assert!(format!("{err:?}").contains("personal_package_imports_disabled"));
    db.execute_unprepared("UPDATE flow_runtime_capabilities SET personal_package_imports=true WHERE singleton")
        .await
        .unwrap();
    let bytes = std::fs::read(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/flow-v2/browser-package-v4/package.zip"
    ))
    .unwrap();
    let body = json!({"source":{"kind":"inline_base64","package_base64":base64::engine::general_purpose::STANDARD.encode(bytes)},"idempotency_key":"transport-upload"});
    let request = axum::http::Request::builder()
        .header("content-type", "application/json")
        .body(axum::body::Body::from(serde_json::to_vec(&body).unwrap()))
        .unwrap();
    let response = api::routes::flow::post_flow_v2_import_artifact(
        State(state.clone()),
        test_claims(owner),
        None,
        Path(scope.id),
        request,
    )
    .await
    .unwrap()
    .into_response();
    let artifact: serde_json::Value =
        serde_json::from_slice(&axum::body::to_bytes(response.into_body(), 1024 * 1024).await.unwrap()).unwrap();
    let artifact_id = Uuid::parse_str(artifact.field("data").field("artifact_id").as_str().unwrap()).unwrap();
    let preview=api::routes::flow::post_flow_v2_import_preview(State(state.clone()),test_claims(owner),None,Path(scope.id),Json(serde_json::from_value(json!({"artifact_id":artifact_id,"project_mapping":{},"external_reference_policy":"reject","conflict_policy":"reject_existing","include_history":false,"idempotency_key":"transport-preview"})).unwrap())).await.unwrap().into_response();
    let preview: serde_json::Value =
        serde_json::from_slice(&axum::body::to_bytes(preview.into_body(), 1024 * 1024).await.unwrap()).unwrap();
    let preview = &preview.field("data");
    assert_eq!(*preview.field("mapping").field("workspace_map"), json!({}));
    assert!(
        preview
            .field("manifest_summary")
            .field("source")
            .get("scope_id")
            .is_none()
    );
    let preview_id = Uuid::parse_str(preview.field("preview_id").as_str().unwrap()).unwrap();
    let outcome=api::routes::flow::post_flow_v2_import_commit(State(state.clone()),test_claims(owner),None,Path((scope.id,preview_id)),Json(serde_json::from_value(json!({"package_sha256":preview.field("package_sha256"),"mapping_hash":preview.field("mapping_hash"),"conflict_policy":"reject_existing","confirm":true,"idempotency_key":"transport-commit"})).unwrap())).await;
    #[cfg(not(target_os = "linux"))]
    {
        let err = outcome.unwrap_err();
        assert!(format!("{err:?}").contains("unsupported_platform"));
        assert_eq!(count(db, "flow_import_jobs").await, 0);
    }
    #[cfg(target_os = "linux")]
    {
        let response = outcome.unwrap().into_response();
        let receipt: serde_json::Value =
            serde_json::from_slice(&axum::body::to_bytes(response.into_body(), 1024 * 1024).await.unwrap()).unwrap();
        assert_eq!(*receipt.field("data").field("status"), "completed");
    }
    assert_eq!(count(db, "workspaces").await, 0);
    assert_eq!(count(db, "workspace_members").await, 0);
    drop(state);
    scratch.close().await;
}

#[tokio::test]
async fn personal_scope_export_selects_supported_kinds_excludes_root_and_preserves_history_members() {
    use collab_core::CollabEngine;
    use serde_json::json;
    let scratch = Scratch::new().await;
    let db = &scratch.db;
    let owner = user(db).await;
    let (scope, roots) = scope::personal_home(db, owner).await.unwrap();
    let created = scope::create_personal(db, owner, create(scope.id, None, "scope-package-page"))
        .await
        .unwrap();
    let object = Uuid::parse_str(created.field("object").field("id").as_str().unwrap()).unwrap();
    let document = Uuid::parse_str(created.field("object").field("document_id").as_str().unwrap()).unwrap();
    db.execute_unprepared("UPDATE flow_runtime_capabilities SET personal_scope_package_exports=false WHERE singleton")
        .await
        .unwrap();
    assert!(
        api::flow::scope_export::create_scope(
            db,
            scope.id,
            owner,
            false,
            "scope-export",
            Some(vec!["page".into(), "navigator".into()])
        )
        .await
        .is_err()
    );
    db.execute_unprepared("UPDATE flow_runtime_capabilities SET personal_scope_package_exports=true WHERE singleton")
        .await
        .unwrap();
    // Coherent accepted-history fixture: real CRDT bytes/frontiers and event/dispatch transaction; no worker pass is claimed.
    let mut engine = api::flow::document::load_head_engine(db, document).await.unwrap();
    let before = engine.frontier().as_bytes().to_vec();
    engine.set_title("History final title").unwrap();
    let after = engine.frontier().as_bytes().to_vec();
    let update = engine
        .export_from(&collab_core::Frontier::from_bytes(before.clone()))
        .unwrap();
    let event = Uuid::new_v4();
    let update_id = Uuid::new_v4();
    let tx = db.begin().await.unwrap();
    tx.execute(Statement::from_sql_and_values(DbBackend::Postgres,"INSERT INTO business_events(id,workspace_id,source_scope_kind,source_flow_scope_id,event_type,aggregate_type,aggregate_id,actor_id,source,payload,metadata) VALUES($1,NULL,'flow',$2,'flow.content.accepted','flow_document',$3,$4,'{}',$5,'{}')",vec![event.into(),scope.id.into(),document.to_string().into(),owner.into(),json!({"object_id":object,"document_id":document,"accepted_seq":1}).into()])).await.unwrap();
    tx.execute(Statement::from_sql_and_values(DbBackend::Postgres,"INSERT INTO collab_updates(document_id,seq,update_id,content_hash,before_frontier,after_frontier,bytes,actor_id,origin_surface,origin_client_id,projection_seq,event_id) VALUES($1,1,$2,$3,$4,$5,$6,$7,'rest','history-fixture',1,$8)",vec![document.into(),update_id.into(),api::flow::collab::bootstrap::content_hash(&update).into(),before.into(),after.clone().into(),update.clone().into(),owner.into(),event.into()])).await.unwrap();
    tx.execute(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "UPDATE collab_documents SET head_seq=1,head_frontier=$2,update_count=1,byte_count=byte_count+$3 WHERE id=$1",
        vec![
            document.into(),
            after.clone().into(),
            i64::try_from(update.len()).unwrap().into(),
        ],
    ))
    .await
    .unwrap();
    tx.execute(Statement::from_sql_and_values(DbBackend::Postgres,"UPDATE flow_object_projections SET document_seq=1,document_frontier=$2,title='History final title',state=$3 WHERE object_id=$1",vec![object.into(),after.into(),api::flow::projection::state_json(&engine.semantic_snapshot().unwrap()).unwrap().into()])).await.unwrap();
    tx.execute(Statement::from_sql_and_values(DbBackend::Postgres,"INSERT INTO event_dispatch(id,event_id,workspace_id,source_flow_scope_id,event_type,document_id,accepted_seq,max_attempts) VALUES($1,$2,NULL,$3,'flow.content.accepted',$4,1,3)",vec![Uuid::new_v4().into(),event.into(),scope.id.into(),document.into()])).await.unwrap();
    tx.commit().await.unwrap();
    let mut collection = create(scope.id, None, "scope-package-typed");
    collection.object_type = "collection".into();
    scope::create_personal(db, owner, collection).await.unwrap();
    let err = api::flow::scope_export::create_scope(db, scope.id, owner, true, "scope-all-refused", None)
        .await
        .unwrap_err();
    assert!(format!("{err:?}").contains("typed_package_roundtrip_unsupported"));
    let first = api::flow::scope_export::create_scope(
        db,
        scope.id,
        owner,
        true,
        "scope-export",
        Some(vec!["page".into(), "navigator".into()]),
    )
    .await
    .unwrap();
    assert!(first.get("object_id").is_none_or(serde_json::Value::is_null));
    assert_eq!(*first.field("scope_kind"), "scope");
    let job = Uuid::parse_str(first.field("job_id").as_str().unwrap()).unwrap();
    let (bytes, hash, _) = api::flow::scope_export::download(db, job, owner).await.unwrap();
    let verified = api::flow::package::verify_package(std::io::Cursor::new(bytes.clone()), Some(&hash)).unwrap();
    assert_eq!(verified.manifest.source.scope, "scope");
    assert_eq!(verified.manifest.counts.objects, 1);
    assert_eq!(verified.manifest.counts.updates, 1);
    assert!(verified.manifest.history.included);
    assert_eq!(verified.manifest.history.snapshot_history.as_deref(), Some("retained"));
    assert!(
        !verified
            .manifest
            .source
            .root_object_ids
            .contains(&roots.first().expect("fixture element").object_id.to_string())
    );
    let mut archive = zip::ZipArchive::new(std::io::Cursor::new(bytes)).unwrap();
    let mut snapshot = Vec::new();
    std::io::Read::read_to_end(
        &mut archive.by_name(&format!("documents/{document}/snapshot.bin")).unwrap(),
        &mut snapshot,
    )
    .unwrap();
    let mut tail = Vec::new();
    std::io::Read::read_to_end(
        &mut archive.by_name(&format!("documents/{document}/updates/1.bin")).unwrap(),
        &mut tail,
    )
    .unwrap();
    let mut restored = collab_core::LoroCollabEngine::load(&snapshot).unwrap();
    assert_ne!(restored.title().unwrap(), "History final title");
    restored.import_update(&tail).unwrap();
    assert_eq!(restored.frontier(), engine.frontier());
    assert_eq!(restored.title().unwrap(), "History final title");
    assert!(
        first
            .field("warnings")
            .as_array()
            .unwrap()
            .iter()
            .any(|v| v == "object_types_omitted:collection")
    );
    assert!(
        first
            .field("warnings")
            .as_array()
            .unwrap()
            .iter()
            .any(|v| v == "non_content_history_events_omitted")
    );
    assert_eq!(
        api::flow::scope_export::create_scope(
            db,
            scope.id,
            owner,
            true,
            "scope-export",
            Some(vec!["navigator".into(), "page".into()])
        )
        .await
        .unwrap()
        .field("job_id"),
        first.field("job_id")
    );
    db.execute_unprepared("UPDATE flow_runtime_capabilities SET personal_scope_package_exports=false WHERE singleton")
        .await
        .unwrap();
    assert_eq!(api::flow::scope_export::get(db, job, owner).await.unwrap().id, job);
    assert_eq!(count(db, "workspaces").await, 0);
    assert_eq!(count(db, "workspace_members").await, 0);
    scratch.close().await;
}

#[tokio::test]
async fn personal_forms_conversion_commits_native_target_with_scoped_job_and_replay() {
    use api::flow::{
        bridge,
        event_origin::{CommandOrigin, EventSurface},
    };
    use axum::{
        Json,
        extract::{Path, State},
        response::IntoResponse,
    };
    use serde_json::json;
    let scratch = Scratch::new().await;
    let db = &scratch.db;
    let owner = user(db).await;
    let (personal, _) = scope::personal_home(db, owner).await.unwrap();
    let created = scope::create_personal(db, owner, create(personal.id, None, "conversion-source"))
        .await
        .unwrap();
    let source = Uuid::parse_str(created.field("object").field("id").as_str().unwrap()).unwrap();
    let frontier = scope::object_view(db, source, owner)
        .await
        .unwrap()
        .field("frontier")
        .as_str()
        .unwrap()
        .to_owned();
    let workspace = Uuid::new_v4();
    let project = Uuid::new_v4();
    db.execute(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "INSERT INTO workspaces(id,slug,name,created_by) VALUES($1,$2,'Conversion destination',$3)",
        vec![workspace.into(), workspace.to_string().into(), owner.into()],
    ))
    .await
    .unwrap();
    db.execute(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "INSERT INTO workspace_members(workspace_id,user_id,role) VALUES($1,$2,'owner')",
        vec![workspace.into(), owner.into()],
    ))
    .await
    .unwrap();
    db.execute(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "INSERT INTO flow_workspace_settings(workspace_id,flow_enabled,bridge_enabled) VALUES($1,true,true) ON CONFLICT (workspace_id) DO UPDATE SET flow_enabled = EXCLUDED.flow_enabled, bridge_enabled = EXCLUDED.bridge_enabled",
        vec![workspace.into()],
    ))
    .await
    .unwrap();
    db.execute(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "INSERT INTO projects(id,workspace_id,key,name,created_by) VALUES($1,$2,'CONV','Conversion destination',$3)",
        vec![project.into(), workspace.into(), owner.into()],
    ))
    .await
    .unwrap();
    let state = test_state(db.clone());
    let preview_input = || bridge::ScopeConversionPreviewInput {
        source_object_id: source,
        source_frontier: frontier.clone(),
        target_type: "form".into(),
        mapping: json!({"target_project_id":project,"key":"converted_scope","name":"Scoped form","schema":{"version":"openpr.form.schema.v1","fields":[{"field_id":"portable_subject","key":"subject","label":"Subject","type":"text","required":true}]}}),
        idempotency_key: "conversion-preview".into(),
        destination_workspace_id: workspace,
        destination_project_id: Some(project),
    };
    // Default-on since 0106; switch it off to prove the refusal.
    db.execute_unprepared("UPDATE flow_runtime_capabilities SET personal_forms_conversion=false WHERE singleton")
        .await
        .unwrap();
    assert!(
        bridge::preview_scope_conversion(&state, owner, preview_input())
            .await
            .is_err()
    );
    db.execute_unprepared("UPDATE flow_runtime_capabilities SET personal_forms_conversion=true WHERE singleton")
        .await
        .unwrap();
    let mut invalid = preview_input();
    invalid.idempotency_key = "invalid-field-map".into();
    *invalid
        .mapping
        .pointer_mut("/schema/fields")
        .expect("fixture has schema fields") = json!([{"key":"broken","type":"missing-field-type"}]);
    assert!(bridge::preview_scope_conversion(&state, owner, invalid).await.is_err());
    let preview = bridge::preview_scope_conversion(&state, owner, preview_input())
        .await
        .unwrap();
    assert_eq!(
        bridge::preview_scope_conversion(&state, owner, preview_input())
            .await
            .unwrap()
            .preview_id,
        preview.preview_id
    );
    let commit_input = || bridge::ConversionCommitInput {
        preview_id: preview.preview_id,
        source_frontier: frontier.clone(),
        target_schema_version: 0,
        idempotency_key: "conversion-commit".into(),
        confirm: true,
    };
    let job = bridge::commit_scope_conversion(
        &state,
        owner,
        commit_input(),
        CommandOrigin::first_request_from(EventSurface::Rest),
    )
    .await
    .unwrap();
    assert_eq!(job.status, "completed");
    assert_eq!(job.created_target_ids.len(), 1);
    assert_eq!(
        bridge::commit_scope_conversion(
            &state,
            owner,
            commit_input(),
            CommandOrigin::first_request_from(EventSurface::Rest)
        )
        .await
        .unwrap()
        .job_id,
        job.job_id
    );
    assert_eq!(
        bridge::scope_conversion_status(&state, owner, job.job_id)
            .await
            .unwrap()
            .status,
        "completed"
    );
    let target = db
        .query_one(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT workspace_id,project_id,name FROM project_forms WHERE id=$1",
            vec![job.created_target_ids.first().copied().expect("fixture element").into()],
        ))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(target.try_get::<Uuid>("", "workspace_id").unwrap(), workspace);
    assert_eq!(target.try_get::<Uuid>("", "project_id").unwrap(), project);
    let form_id = *job.created_target_ids.first().unwrap();
    let submission = api::routes::form::create_form_record(
        State(state.clone()),
        test_claims(owner),
        None,
        Path(form_id),
        Json(
            serde_json::from_value(json!({"values":{"subject":"Submitted from the converted Form"},
          "idempotency_key":"converted-runtime-submit"}))
            .unwrap(),
        ),
    )
    .await
    .unwrap()
    .into_response();
    assert!(submission.status().is_success());
    let row = db
        .query_one(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT values FROM form_records WHERE form_id=$1",
            vec![form_id.into()],
        ))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        *row.try_get::<serde_json::Value>("", "values")
            .unwrap()
            .pointer("/subject")
            .expect("response has /subject"),
        "Submitted from the converted Form"
    );
    let ownership = db
        .query_one(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT scope_id,workspace_id,request_key FROM flow_conversion_jobs WHERE id=$1",
            vec![job.job_id.into()],
        ))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(ownership.try_get::<Uuid>("", "scope_id").unwrap(), personal.id);
    assert_eq!(
        ownership.try_get::<String>("", "request_key").unwrap(),
        "conversion-commit"
    );
    assert_eq!(count(db, "workspaces").await, 1);
    assert_eq!(count(db, "workspace_members").await, 1);
    let sources=db.query_one(Statement::from_sql_and_values(DbBackend::Postgres,"SELECT count(*) AS n FROM business_events WHERE event_type LIKE 'flow.conversion.%' AND source_flow_scope_id=$1 AND workspace_id IS NULL",vec![personal.id.into()])).await.unwrap().unwrap().try_get::<i64>("","n").unwrap();
    assert_eq!(sources, 3);

    // A native Forms INSERT failure must leave no target/lineage; retry uses the same durable job.
    let mut failed_input = preview_input();
    failed_input.idempotency_key = "conversion-failure-preview".into();
    *failed_input.mapping.get_mut("key").expect("required fixture field") = json!("converted_retry");
    let failed_preview = bridge::preview_scope_conversion(&state, owner, failed_input)
        .await
        .unwrap();
    db.execute_unprepared("CREATE FUNCTION reject_scope_conversion_target() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN IF NEW.key='converted_retry' THEN RAISE EXCEPTION 'synthetic target failure'; END IF; RETURN NEW; END $$; CREATE TRIGGER reject_scope_conversion_target BEFORE INSERT ON project_forms FOR EACH ROW EXECUTE FUNCTION reject_scope_conversion_target()").await.unwrap();
    let failed_commit = bridge::ConversionCommitInput {
        preview_id: failed_preview.preview_id,
        source_frontier: frontier.clone(),
        target_schema_version: 0,
        idempotency_key: "conversion-failed-commit".into(),
        confirm: true,
    };
    assert!(
        bridge::commit_scope_conversion(
            &state,
            owner,
            failed_commit,
            CommandOrigin::first_request_from(EventSurface::Rest)
        )
        .await
        .is_err()
    );
    let failed = db
        .query_one(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT id,status FROM flow_conversion_jobs WHERE preview_id=$1",
            vec![failed_preview.preview_id.into()],
        ))
        .await
        .unwrap()
        .unwrap();
    let failed_id = failed.try_get::<Uuid>("", "id").unwrap();
    assert_eq!(failed.try_get::<String>("", "status").unwrap(), "failed");
    assert_eq!(count(db, "project_forms").await, 1);
    assert_eq!(count(db, "flow_object_lineage").await, 1);
    db.execute_unprepared(
        "DROP TRIGGER reject_scope_conversion_target ON project_forms; DROP FUNCTION reject_scope_conversion_target()",
    )
    .await
    .unwrap();
    let retry = || bridge::ConversionRetryInput {
        confirm: true,
        idempotency_key: "conversion-retry".into(),
    };
    let retried = bridge::retry_scope_conversion(
        &state,
        owner,
        failed_id,
        retry(),
        CommandOrigin::first_request_from(EventSurface::Rest),
    )
    .await
    .unwrap();
    assert_eq!(retried.status, "completed");
    assert_eq!(retried.job_id, failed_id);
    assert_eq!(
        bridge::retry_scope_conversion(
            &state,
            owner,
            failed_id,
            retry(),
            CommandOrigin::first_request_from(EventSurface::Rest)
        )
        .await
        .unwrap()
        .created_target_ids,
        retried.created_target_ids
    );
    assert_eq!(count(db, "project_forms").await, 2);
    assert_eq!(count(db, "flow_object_lineage").await, 2);
    assert!(
        bridge::retry_scope_conversion(
            &state,
            owner,
            failed_id,
            bridge::ConversionRetryInput {
                confirm: true,
                idempotency_key: "conversion-commit".into()
            },
            CommandOrigin::first_request_from(EventSurface::Rest)
        )
        .await
        .is_err()
    );

    // The v2 transport preserves an explicit real destination.
    let route_preview=api::routes::flow::post_flow_v2_conversion_preview(State(state.clone()),test_claims(owner),None,Json(json!({"source_object_id":source,"source_frontier":frontier,"target_type":"form","mapping":{"key":"converted_route","name":"Route form","schema":{"version":"openpr.form.schema.v1","fields":[]}},"destination_workspace_id":workspace,"destination_project_id":project,"idempotency_key":"route-preview"}))).await.unwrap();
    let route_preview: serde_json::Value = serde_json::from_slice(
        &axum::body::to_bytes(route_preview.into_body(), 1024 * 1024)
            .await
            .unwrap(),
    )
    .unwrap();
    assert_eq!(*route_preview.field("code"), 0, "{route_preview}");
    let route_preview_id = Uuid::parse_str(route_preview.field("data").field("preview_id").as_str().unwrap()).unwrap();
    let route_job = api::routes::flow::post_flow_v2_conversion(
        State(state.clone()),
        test_claims(owner),
        None,
        Json(bridge::ConversionCommitInput {
            preview_id: route_preview_id,
            source_frontier: frontier.clone(),
            target_schema_version: 0,
            idempotency_key: "route-commit".into(),
            confirm: true,
        }),
    )
    .await
    .unwrap();
    let route_job: serde_json::Value =
        serde_json::from_slice(&axum::body::to_bytes(route_job.into_body(), 1024 * 1024).await.unwrap()).unwrap();
    assert_eq!(*route_job.field("data").field("status"), "completed", "{route_job}");
    let route_job_id = Uuid::parse_str(route_job.field("data").field("job_id").as_str().unwrap()).unwrap();
    let status =
        api::routes::flow::get_flow_v2_conversion(State(state.clone()), test_claims(owner), None, Path(route_job_id))
            .await
            .unwrap()
            .into_response();
    assert_eq!(status.status(), axum::http::StatusCode::OK);
    // Simulate a process crash after a durable claim, without running another target creation.
    let mut abandoned_input = preview_input();
    abandoned_input.idempotency_key = "abandoned-preview".into();
    let abandoned = bridge::preview_scope_conversion(&state, owner, abandoned_input)
        .await
        .unwrap();
    let abandoned_id = Uuid::new_v4();
    db.execute(Statement::from_sql_and_values(DbBackend::Postgres,"INSERT INTO flow_conversion_jobs(id,workspace_id,preview_id,actor_id,actor_kind,source_object_id,source_frontier,target_type,target_schema_version,status,idempotency_key,request_key,started_at) VALUES($1,$2,$3,$4,'user',$5,$6,'form',0,'started',$7,'abandoned-claim',now()-interval '2 hours')",vec![abandoned_id.into(),workspace.into(),abandoned.preview_id.into(),owner.into(),source.into(),frontier.into(),format!("scope:{}:abandoned-claim",personal.id).into()])).await.unwrap();
    assert_eq!(
        bridge::reap_stale_conversion_jobs(&state, time::Duration::hours(1))
            .await
            .unwrap(),
        1
    );
    assert_eq!(
        bridge::reap_stale_conversion_jobs(&state, time::Duration::hours(1))
            .await
            .unwrap(),
        0
    );
    let reaped = bridge::scope_conversion_status(&state, owner, abandoned_id)
        .await
        .unwrap();
    assert_eq!(reaped.status, "failed");
    assert_eq!(reaped.error_reason.as_deref(), Some(bridge::STALE_STARTED_REASON));
    assert_eq!(count(db, "project_forms").await, 3);
    scratch.close().await;
}

/// `cross_workspace_forms_conversion` (default on) gates converting a Page of one workspace into the
/// Forms of a workspace: the switch refuses with its typed reason, and with it on the preview,
/// commit, replay and status all work for a workspace source.
#[tokio::test]
async fn workspace_page_converts_into_another_workspace_forms_when_the_switch_is_on() {
    use api::flow::{
        bridge,
        event_origin::{CommandOrigin, EventSurface},
    };
    use serde_json::json;
    let scratch = Scratch::new().await;
    let db = &scratch.db;
    let owner = user(db).await;
    let mut workspaces = Vec::new();
    for name in ["Conversion source workspace", "Conversion target workspace"] {
        let workspace = Uuid::new_v4();
        db.execute(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "INSERT INTO workspaces(id,slug,name,created_by) VALUES($1,$2,$3,$4)",
            vec![
                workspace.into(),
                workspace.to_string().into(),
                name.into(),
                owner.into(),
            ],
        ))
        .await
        .unwrap();
        db.execute(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "INSERT INTO workspace_members(workspace_id,user_id,role) VALUES($1,$2,'owner')",
            vec![workspace.into(), owner.into()],
        ))
        .await
        .unwrap();
        workspaces.push(workspace);
    }
    let [source_workspace, destination] = workspaces[..] else {
        panic!("two workspaces were created")
    };
    assert!(workspace_flow_enabled(db, source_workspace).await);
    assert!(workspace_flow_enabled(db, destination).await);
    let project = Uuid::new_v4();
    db.execute(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "INSERT INTO projects(id,workspace_id,key,name,created_by) VALUES($1,$2,'XCONV','Cross conversion',$3)",
        vec![project.into(), destination.into(), owner.into()],
    ))
    .await
    .unwrap();
    scope::legacy_scope(db, source_workspace).await.unwrap();
    let state = test_state(db.clone());
    let created = api::flow::command::create_object(
        &state,
        api::flow::command::CreateObjectInput {
            workspace_id: source_workspace,
            actor_id: owner,
            actor_is_bot: false,
            object_type: "page".into(),
            project_id: None,
            parent_object_id: None,
            title: "Workspace conversion source".into(),
            idempotency_key: "workspace-conversion-source".into(),
            message: None,
            origin: CommandOrigin::first_request_from(EventSurface::Rest),
        },
    )
    .await
    .unwrap();
    let source = created.object.id;
    let frontier = scope::object_view(db, source, owner)
        .await
        .unwrap()
        .field("frontier")
        .as_str()
        .unwrap()
        .to_owned();
    let preview_input = |key: &str| bridge::ScopeConversionPreviewInput {
        source_object_id: source,
        source_frontier: frontier.clone(),
        target_type: "form".into(),
        mapping: json!({"target_project_id":project,"key":"converted_workspace","name":"Workspace form","schema":{"version":"openpr.form.schema.v1","fields":[{"field_id":"portable_subject","key":"subject","label":"Subject","type":"text","required":true}]}}),
        idempotency_key: key.into(),
        destination_workspace_id: destination,
        destination_project_id: Some(project),
    };
    assert!(
        scope::capabilities(db).await.unwrap().cross_workspace_forms_conversion,
        "workspace Forms conversion is on by default"
    );
    db.execute_unprepared(
        "UPDATE flow_runtime_capabilities SET cross_workspace_forms_conversion=false WHERE singleton",
    )
    .await
    .unwrap();
    let refused = bridge::preview_scope_conversion(&state, owner, preview_input("workspace-conversion-off"))
        .await
        .unwrap_err();
    assert!(format!("{refused:?}").contains("scope_forms_conversion_disabled"));
    db.execute_unprepared("UPDATE flow_runtime_capabilities SET cross_workspace_forms_conversion=true WHERE singleton")
        .await
        .unwrap();
    let preview = bridge::preview_scope_conversion(&state, owner, preview_input("workspace-conversion-preview"))
        .await
        .unwrap();
    let commit_input = || bridge::ConversionCommitInput {
        preview_id: preview.preview_id,
        source_frontier: frontier.clone(),
        target_schema_version: 0,
        idempotency_key: "workspace-conversion-commit".into(),
        confirm: true,
    };
    let job = bridge::commit_scope_conversion(
        &state,
        owner,
        commit_input(),
        CommandOrigin::first_request_from(EventSurface::Rest),
    )
    .await
    .unwrap();
    assert_eq!(job.status, "completed");
    assert_eq!(job.created_target_ids.len(), 1);
    assert_eq!(
        bridge::commit_scope_conversion(
            &state,
            owner,
            commit_input(),
            CommandOrigin::first_request_from(EventSurface::Rest)
        )
        .await
        .unwrap()
        .job_id,
        job.job_id
    );
    assert_eq!(
        bridge::scope_conversion_status(&state, owner, job.job_id)
            .await
            .unwrap()
            .status,
        "completed"
    );
    let target = db
        .query_one(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT workspace_id,project_id FROM project_forms WHERE id=$1",
            vec![job.created_target_ids.first().copied().expect("one target").into()],
        ))
        .await
        .unwrap()
        .expect("the converted form exists");
    assert_eq!(target.try_get::<Uuid>("", "workspace_id").unwrap(), destination);
    assert_eq!(target.try_get::<Uuid>("", "project_id").unwrap(), project);
    drop(state);
    scratch.close().await;
}

#[tokio::test]
async fn retention_skips_nonleaf_and_destination_deletion_preserves_personal_source() {
    use axum::{
        extract::{Path, State},
        response::IntoResponse,
    };
    use serde_json::json;
    let scratch = Scratch::new().await;
    let db = &scratch.db;
    let owner = user(db).await;
    let (personal, _) = scope::personal_home(db, owner).await.unwrap();
    let parent = scope::create_personal(db, owner, create(personal.id, None, "retention-parent"))
        .await
        .unwrap();
    let parent = Uuid::parse_str(parent.field("object").field("id").as_str().unwrap()).unwrap();
    let child = scope::create_personal(db, owner, create(personal.id, Some(parent), "active-child"))
        .await
        .unwrap();
    let child = Uuid::parse_str(child.field("object").field("id").as_str().unwrap()).unwrap();
    db.execute(Statement::from_sql_and_values(DbBackend::Postgres,"UPDATE flow_objects SET lifecycle_status='archived',archived_at=now()-interval '40 days',permanent_cleanup_after=now()-interval '1 day' WHERE id=$1",vec![parent.into()])).await.unwrap();
    assert!(
        !api::flow::scope_purge::retention_leaf(db, parent, platform::timestamp::Timestamp::now())
            .await
            .unwrap()
    );
    assert!(scope::object_view(db, child, owner).await.is_ok());
    let leaf = scope::create_personal(db, owner, create(personal.id, None, "expired-leaf"))
        .await
        .unwrap();
    let leaf = Uuid::parse_str(leaf.field("object").field("id").as_str().unwrap()).unwrap();
    db.execute(Statement::from_sql_and_values(DbBackend::Postgres,"UPDATE flow_objects SET lifecycle_status='archived',archived_at=now()-interval '40 days',permanent_cleanup_after=now()-interval '1 day' WHERE id=$1",vec![leaf.into()])).await.unwrap();
    assert!(
        api::flow::scope_purge::retention_leaf(db, leaf, platform::timestamp::Timestamp::now())
            .await
            .unwrap()
    );
    assert!(
        !api::flow::scope_purge::retention_leaf(db, leaf, platform::timestamp::Timestamp::now())
            .await
            .unwrap()
    );
    let workspace = Uuid::new_v4();
    let project = Uuid::new_v4();
    db.execute(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "INSERT INTO workspaces(id,slug,name,created_by) VALUES($1,$2,'Disposable destination',$3)",
        vec![workspace.into(), workspace.to_string().into(), owner.into()],
    ))
    .await
    .unwrap();
    db.execute(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "INSERT INTO workspace_members(workspace_id,user_id,role) VALUES($1,$2,'owner')",
        vec![workspace.into(), owner.into()],
    ))
    .await
    .unwrap();
    db.execute(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "INSERT INTO flow_workspace_settings(workspace_id,flow_enabled) VALUES($1,true) ON CONFLICT (workspace_id) DO UPDATE SET flow_enabled = EXCLUDED.flow_enabled",
        vec![workspace.into()],
    ))
    .await
    .unwrap();
    db.execute(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "INSERT INTO projects(id,workspace_id,key,name,created_by) VALUES($1,$2,'DEST','Destination',$3)",
        vec![project.into(), workspace.into(), owner.into()],
    ))
    .await
    .unwrap();
    db.execute(Statement::from_sql_and_values(DbBackend::Postgres,"INSERT INTO flow_mounts(source_object_id,destination_workspace_id,destination_project_id,grant_level,created_by) VALUES($1,$2,$3,'view',$4)",vec![child.into(),workspace.into(),project.into(),owner.into()])).await.unwrap();
    let before = db
        .query_one(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT authz_revision FROM flow_scope_settings WHERE scope_id=$1",
            vec![personal.id.into()],
        ))
        .await
        .unwrap()
        .unwrap()
        .try_get::<i64>("", "authz_revision")
        .unwrap();
    let state = test_state(db.clone());
    let deleted = api::routes::workspace::delete_workspace(State(state), test_claims(owner), Path(workspace))
        .await
        .unwrap()
        .into_response();
    assert_eq!(deleted.status(), axum::http::StatusCode::OK);
    assert_eq!(count(db, "flow_mounts").await, 0);
    assert_eq!(count(db, "workspaces").await, 0);
    let source = scope::object_view(db, child, owner).await.unwrap();
    assert_eq!(*source.field("scope_id"), json!(personal.id));
    let after = db
        .query_one(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT authz_revision FROM flow_scope_settings WHERE scope_id=$1",
            vec![personal.id.into()],
        ))
        .await
        .unwrap()
        .unwrap()
        .try_get::<i64>("", "authz_revision")
        .unwrap();
    assert_eq!(after, before + 1);
    scratch.close().await;
}

#[tokio::test]
async fn scoped_maintenance_jobs_keep_real_ownership_and_use_shared_compaction_and_quarantine() {
    use api::flow::scope_operations as ops;
    use serde_json::json;
    let scratch = Scratch::new().await;
    let db = &scratch.db;
    let owner = user(db).await;
    let (personal, _) = scope::personal_home(db, owner).await.unwrap();
    let created = scope::create_personal(db, owner, create(personal.id, None, "ops-source"))
        .await
        .unwrap();
    let object = Uuid::parse_str(created.field("object").field("id").as_str().unwrap()).unwrap();
    let input = |operation: &str, key: &str, dry_run: bool| ops::EnqueueInput {
        operation: operation.into(),
        expected_head_seq: 0,
        dry_run,
        idempotency_key: key.into(),
        confirm_quarantine: !dry_run,
    };
    let queued = ops::enqueue(db, object, owner, input("compact", "compact-plan", true))
        .await
        .unwrap();
    let operation = Uuid::parse_str(queued.field("operation_id").as_str().unwrap()).unwrap();
    assert_eq!(
        ops::enqueue(db, object, owner, input("compact", "compact-plan", true))
            .await
            .unwrap(),
        queued
    );
    assert_eq!(ops::run_compaction_jobs(db, 2).await.unwrap(), 1);
    assert_eq!(
        ops::status(db, personal.id, owner, operation)
            .await
            .unwrap()
            .field("status"),
        "completed"
    );
    let compact = ops::enqueue(db, object, owner, input("compact", "compact-execute", false))
        .await
        .unwrap();
    assert_eq!(ops::run_compaction_jobs(db, 2).await.unwrap(), 1);
    let compact_id = Uuid::parse_str(compact.field("operation_id").as_str().unwrap()).unwrap();
    assert_eq!(
        ops::status(db, personal.id, owner, compact_id)
            .await
            .unwrap()
            .field("result")
            .field("head_seq"),
        0
    );
    let document = Uuid::parse_str(queued.field("document_id").as_str().unwrap()).unwrap();
    let finding = Uuid::new_v4();
    db.execute(Statement::from_sql_and_values(DbBackend::Postgres,"INSERT INTO flow_integrity_records(id,workspace_id,kind,subject_kind,subject_id,detected_by) VALUES($1,NULL,'synthetic_finding','collab_document',$2,'developer-fixture')",vec![finding.into(),document.to_string().into()])).await.unwrap();
    let preview = ops::enqueue(db, object, owner, input("repair_quarantine", "quarantine-plan", true))
        .await
        .unwrap();
    assert_eq!(ops::run_quarantine_jobs(db, 2).await.unwrap(), 1);
    let preview_id = Uuid::parse_str(preview.field("operation_id").as_str().unwrap()).unwrap();
    assert_eq!(
        ops::status(db, personal.id, owner, preview_id)
            .await
            .unwrap()
            .field("result")
            .field("integrity_record_count"),
        1
    );
    assert_eq!(
        ops::health(db, personal.id, owner)
            .await
            .unwrap()
            .field("open_integrity_records"),
        1
    );
    let mut unconfirmed = input("repair_quarantine", "quarantine-unconfirmed", false);
    unconfirmed.confirm_quarantine = false;
    assert!(ops::enqueue(db, object, owner, unconfirmed).await.is_err());
    let repaired = ops::enqueue(
        db,
        object,
        owner,
        input("repair_quarantine", "quarantine-execute", false),
    )
    .await
    .unwrap();
    assert_eq!(ops::run_quarantine_jobs(db, 2).await.unwrap(), 1);
    let repair_id = Uuid::parse_str(repaired.field("operation_id").as_str().unwrap()).unwrap();
    assert_eq!(
        ops::status(db, personal.id, owner, repair_id)
            .await
            .unwrap()
            .field("status"),
        "completed"
    );
    assert_eq!(
        ops::health(db, personal.id, owner)
            .await
            .unwrap()
            .field("open_integrity_records"),
        0
    );
    assert_eq!(ops::run_quarantine_jobs(db, 2).await.unwrap(), 0);
    assert!(
        ops::enqueue(db, object, owner, input("compact", "quarantine-execute", false))
            .await
            .is_err()
    );
    let row = db
        .query_one(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT workspace_id,owner_flow_scope_id,scope_id FROM flow_operation_runs WHERE id=$1",
            vec![operation.into()],
        ))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(row.try_get::<Option<Uuid>>("", "workspace_id").unwrap(), None);
    assert_eq!(row.try_get::<Uuid>("", "owner_flow_scope_id").unwrap(), personal.id);
    assert_eq!(row.try_get::<Uuid>("", "scope_id").unwrap(), document);
    let old = api::flow::operations::document_scope(db, document).await.unwrap_err();
    assert_eq!(old.kind(), api::error::ApiErrorKind::UnsupportedProtocol);
    let health = ops::health(db, personal.id, owner).await.unwrap();
    assert_eq!(*health.field("queue_depth"), 0);
    assert_eq!(*health.field("scope_id"), json!(personal.id));
    assert_eq!(count(db, "workspaces").await, 0);
    assert_eq!(count(db, "workspace_members").await, 0);
    // Replay the latest operation ownership migration against populated scoped rows.
    db.execute_unprepared(include_str!(
        "../../../migrations/0091_flow_operation_scope_ownership.sql"
    ))
    .await
    .unwrap();
    assert_eq!(
        ops::status(db, personal.id, owner, operation)
            .await
            .unwrap()
            .field("status"),
        "completed"
    );
    let diagnostic = api::routes::flow::get_flow_v2_collab_diagnostics(
        axum::extract::State(test_state(db.clone())),
        test_claims(owner),
        None,
        axum::extract::Path(object),
        axum::extract::Query(serde_json::from_value(json!({"include_sizes":true})).unwrap()),
    )
    .await
    .unwrap();
    let diagnostic: serde_json::Value =
        serde_json::from_slice(&axum::body::to_bytes(diagnostic.into_body(), 1024 * 1024).await.unwrap()).unwrap();
    assert_eq!(*diagnostic.field("data").field("scope_id"), json!(personal.id));
    assert_eq!(*diagnostic.field("data").field("document_id"), json!(document));
    assert_eq!(
        diagnostic.field("data").field("format_version").as_str(),
        Some("loro-1")
    );
    assert!(
        api::routes::flow::get_flow_v2_collab_diagnostics(
            axum::extract::State(test_state(db.clone())),
            test_claims(owner),
            None,
            axum::extract::Path(object),
            axum::extract::Query(serde_json::from_value(json!({"include_sizes":false})).unwrap())
        )
        .await
        .is_err()
    );
    scratch.close().await;
}

async fn read_navigator_fixture(
    state: platform::app::AppState,
    owner: Uuid,
    scope: Uuid,
    params: serde_json::Value,
) -> serde_json::Value {
    use axum::response::IntoResponse;
    let response = api::routes::flow::get_flow_v2_navigator(
        axum::extract::State(state),
        test_claims(owner),
        None,
        axum::extract::Path(scope),
        axum::extract::Query(serde_json::from_value(params).unwrap()),
    )
    .await
    .unwrap()
    .into_response();
    serde_json::from_slice(&axum::body::to_bytes(response.into_body(), 1024 * 1024).await.unwrap()).unwrap()
}

#[tokio::test]
async fn navigator_honors_depth_and_archived_options_without_silent_truncation() {
    use axum::extract::{Path, Query, State};
    use serde_json::json;
    let scratch = Scratch::new().await;
    let db = &scratch.db;
    let owner = user(db).await;
    let (personal, _) = scope::personal_home(db, owner).await.unwrap();
    let parent = scope::create_personal(db, owner, create(personal.id, None, "nav-parent"))
        .await
        .unwrap();
    let parent = Uuid::parse_str(parent.field("object").field("id").as_str().unwrap()).unwrap();
    let child = scope::create_personal(db, owner, create(personal.id, Some(parent), "nav-child"))
        .await
        .unwrap();
    let child = Uuid::parse_str(child.field("object").field("id").as_str().unwrap()).unwrap();
    let state = test_state(db.clone());

    let shallow = read_navigator_fixture(state.clone(), owner, personal.id, json!({"depth":1})).await;
    assert_eq!(
        shallow.field("data").field("nodes").as_array().unwrap().len(),
        1,
        "{shallow}"
    );
    let deep = read_navigator_fixture(state.clone(), owner, personal.id, json!({"depth":2})).await;
    assert_eq!(deep.field("data").field("nodes").as_array().unwrap().len(), 2, "{deep}");
    db.execute(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "UPDATE flow_objects SET lifecycle_status='archived',archived_at=now() WHERE id=$1",
        vec![child.into()],
    ))
    .await
    .unwrap();
    assert_eq!(
        read_navigator_fixture(state.clone(), owner, personal.id, json!({"depth":2}))
            .await
            .field("data")
            .field("nodes")
            .as_array()
            .unwrap()
            .len(),
        1
    );
    assert_eq!(
        read_navigator_fixture(
            state.clone(),
            owner,
            personal.id,
            json!({"depth":2,"include_archived":true})
        )
        .await
        .field("data")
        .field("nodes")
        .as_array()
        .unwrap()
        .len(),
        2
    );
    for params in [
        json!({"depth":0}),
        json!({"depth":21}),
        json!({"cursor":"ignored-before"}),
    ] {
        assert!(
            api::routes::flow::get_flow_v2_navigator(
                State(state.clone()),
                test_claims(owner),
                None,
                Path(personal.id),
                Query(serde_json::from_value(params).unwrap())
            )
            .await
            .is_err()
        );
    }
    scratch.close().await;
}

#[tokio::test]
async fn stale_model_two_navigator_refreshes_from_order_map_without_mutation() {
    use axum::{
        extract::{Path, Query, State},
        response::IntoResponse,
    };
    use collab_core::CollabEngine;
    use serde_json::{Value, json};
    let scratch = Scratch::new().await;
    let db = &scratch.db;
    let owner = user(db).await;
    let (personal, _) = scope::personal_home(db, owner).await.unwrap();
    let mut request = create(personal.id, None, "old-map-projection");
    request.object_type = "navigator".into();
    let created = scope::create_personal(db, owner, request).await.unwrap();
    let object = created
        .field("object")
        .field("id")
        .as_str()
        .unwrap()
        .parse::<Uuid>()
        .unwrap();
    let doc = scope::object_view(db, object, owner)
        .await
        .unwrap()
        .field("document_id")
        .as_str()
        .unwrap()
        .parse::<Uuid>()
        .unwrap();
    let bytes =
        include_bytes!("../../../frontend/tests/fixtures/flow-navigator-order/browser-reedit.snapshot").to_vec();
    let engine = collab_core::LoroCollabEngine::load(&bytes).unwrap();
    let current = engine.semantic_snapshot().unwrap();
    let node = "33333333-3333-4333-8333-333333333333";
    // A stored projection from before the navigator order map was applied: the node keeps its
    // tree position instead of the order key of the map.
    let mut old = serde_json::to_value(&current).unwrap();
    *old.pointer_mut(&format!("/nodes/{node}/order_key"))
        .expect("the node has an order key") = json!("00000002");
    old.as_object_mut()
        .expect("fixture object")
        .insert("__flow_projection_model".into(), json!(2));
    assert_eq!(current.nodes.get(node).unwrap().order_key, "9");
    assert_ne!(old.field("nodes").field(node).field("order_key").as_str(), Some("9"));
    let frontier = engine.frontier().as_bytes().to_vec();
    db.execute(Statement::from_sql_and_values(DbBackend::Postgres,"UPDATE collab_documents SET snapshot=$2,snapshot_checksum=$3,snapshot_frontier=$4,head_frontier=$4 WHERE id=$1",vec![doc.into(),bytes.clone().into(),api::flow::collab::bootstrap::content_hash(&bytes).into(),frontier.clone().into()])).await.unwrap();
    db.execute(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "UPDATE flow_object_projections SET state=$2,document_frontier=$3 WHERE object_id=$1",
        vec![object.into(), old.clone().into(), frontier.into()],
    ))
    .await
    .unwrap();
    let response = api::routes::flow::get_flow_v2_object(
        State(test_state(db.clone())),
        test_claims(owner),
        None,
        Path(object),
        Query(serde_json::from_value(json!({})).unwrap()),
    )
    .await
    .unwrap()
    .into_response();
    let response: Value =
        serde_json::from_slice(&axum::body::to_bytes(response.into_body(), 1024 * 1024).await.unwrap()).unwrap();
    assert_eq!(
        response
            .field("data")
            .field("semantic_content")
            .field("nodes")
            .field(node)
            .field("order_key")
            .as_str(),
        Some("9")
    );
    assert_eq!(
        *response
            .field("data")
            .field("semantic_content")
            .field("__flow_projection_model"),
        json!(api::flow::projection::PROJECTION_MODEL_VERSION)
    );
    assert_eq!(*response.field("data").field("scope_id"), json!(personal.id));
    assert!(
        !response
            .field("data")
            .field("supported_export_formats")
            .as_array()
            .expect("fixture formats")
            .iter()
            .any(|format| format == "package")
    );
    let rejected = api::flow::scope_export::create(db, object, owner, "package", None, false, "known-inner-reference")
        .await
        .expect_err("reference remapping is not implemented");
    assert_eq!(rejected.kind(), api::error::ApiErrorKind::UnsupportedFormat);
    assert_eq!(count(db, "flow_export_jobs").await, 0);
    assert_eq!(count(db, "flow_import_jobs").await, 0);
    let retained=db.query_one(Statement::from_sql_and_values(DbBackend::Postgres,"SELECT d.snapshot,p.state FROM collab_documents d JOIN flow_object_projections p ON p.object_id=d.object_id WHERE d.id=$1",vec![doc.into()])).await.unwrap().unwrap();
    assert_eq!(retained.try_get::<Vec<u8>>("", "snapshot").unwrap(), bytes);
    assert_eq!(retained.try_get::<Value>("", "state").unwrap(), old);
    assert_eq!(count(db, "collab_updates").await, 0);
    assert_eq!(count(db, "workspaces").await, 0);
    assert_eq!(count(db, "workspace_members").await, 0);
    scratch.close().await;
}

#[tokio::test]
async fn real_mount_permission_changes_and_revocation_fence_existing_readers() {
    use api::flow::scope_mounts::{self, ChangeMountInput, MountInput};
    let scratch = Scratch::new().await;
    let db = &scratch.db;
    let source_owner = user(db).await;
    let member = user(db).await;
    let target_admin = user(db).await;
    let outsider = user(db).await;
    let (personal, _) = scope::personal_home(db, source_owner).await.unwrap();
    let created = scope::create_personal(db, source_owner, create(personal.id, None, "mount-change-page"))
        .await
        .unwrap();
    let object = Uuid::parse_str(created.field("object").field("id").as_str().unwrap()).unwrap();
    let workspace = Uuid::new_v4();
    db.execute(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "INSERT INTO workspaces(id,slug,name,created_by) VALUES($1,$2,'Mount target',$3)",
        vec![workspace.into(), workspace.to_string().into(), source_owner.into()],
    ))
    .await
    .unwrap();
    for (actor, role) in [(source_owner, "owner"), (member, "member"), (target_admin, "admin")] {
        db.execute(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "INSERT INTO workspace_members(workspace_id,user_id,role) VALUES($1,$2,$3)",
            vec![workspace.into(), actor.into(), role.into()],
        ))
        .await
        .unwrap();
    }
    db.execute_unprepared("UPDATE flow_runtime_capabilities SET external_mounts=true")
        .await
        .unwrap();
    let mount = scope_mounts::create(
        db,
        source_owner,
        object,
        MountInput {
            destination_workspace_id: workspace,
            destination_project_id: None,
            level: "edit".into(),
            idempotency_key: "mount-new".into(),
        },
    )
    .await
    .unwrap();
    let edit = scope::authorize(db, object, member, PermissionLevel::Edit)
        .await
        .unwrap();
    assert!(scope::object_view(db, object, member).await.is_ok());
    assert!(scope::object_view(db, object, outsider).await.is_err());
    let change = |key: &str| ChangeMountInput {
        expected_level: "edit".into(),
        level: "view".into(),
        idempotency_key: key.into(),
    };
    assert!(
        scope_mounts::change(db, target_admin, object, mount.id, change("forbidden"))
            .await
            .is_err()
    );
    let changed = scope_mounts::change(db, source_owner, object, mount.id, change("mount-change"))
        .await
        .unwrap();
    assert_eq!(changed.level, "view");
    assert_eq!(
        scope_mounts::change(db, source_owner, object, mount.id, change("mount-change"))
            .await
            .unwrap()
            .id,
        mount.id
    );
    assert!(
        scope_mounts::change(db, source_owner, object, mount.id, change("stale-level"))
            .await
            .is_err()
    );
    assert!(!scope::proof_current(db, &edit).await.unwrap());
    assert!(
        scope::authorize(db, object, member, PermissionLevel::Edit)
            .await
            .is_err()
    );
    let read = scope::authorize(db, object, member, PermissionLevel::View)
        .await
        .unwrap();
    api::routes::member::remove_member(
        axum::extract::State(test_state(db.clone())),
        test_claims(target_admin),
        axum::extract::Path((workspace, member)),
    )
    .await
    .unwrap();
    assert!(!scope::proof_current(db, &read).await.unwrap());
    assert!(scope::object_view(db, object, member).await.is_err());
    scope_mounts::remove(db, target_admin, object, mount.id).await.unwrap();
    scope_mounts::remove(db, target_admin, object, mount.id).await.unwrap();
    assert!(scope::object_view(db, object, target_admin).await.is_err());
    assert!(scope::object_view(db, object, source_owner).await.is_ok());
    scratch.close().await;
}
