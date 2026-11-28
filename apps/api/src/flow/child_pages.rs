//! Prepared local intent, finalized only inside the accepted source Page content transaction.
use super::{collab::authz::PermissionLevel, scope};
use crate::error::ApiError;
use platform::timestamp::Timestamp;
use sea_orm::{
    ConnectionTrait, DatabaseConnection, DatabaseTransaction, DbBackend, FromQueryResult, Statement, TransactionTrait,
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use uuid::Uuid;
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Reservation {
    pub reservation_id: Uuid,
    pub child_page_id: Uuid,
    pub source_object_id: Uuid,
    pub title: String,
    pub expires_at: Timestamp,
    pub state: String,
}
#[derive(FromQueryResult)]
struct Row {
    id: Uuid,
    scope_id: Uuid,
    source_object_id: Uuid,
    actor_id: Uuid,
    child_page_id: Uuid,
    title: String,
    expires_at: Timestamp,
    state: String,
    expired: bool,
}
fn reason(code: &str) -> ApiError {
    ApiError::policy_rejected_with_details(
        "child Page preparation is no longer available",
        json!({"reason":code,"write_state":"not_applied"}),
    )
}
async fn editable(
    db: &DatabaseConnection,
    source: Uuid,
    actor: Uuid,
) -> Result<(scope::ObjectIdentity, scope::AuthorizationProof), ApiError> {
    let proof = scope::authorize(db, source, actor, PermissionLevel::Edit).await?;
    let identity = scope::identity(db, source).await?;
    if identity.object_type != "page" || !proof.resource_active {
        return Err(ApiError::policy_rejected(
            "child Page preparation requires an active editable Page",
        ));
    }
    Ok((identity, proof))
}
pub async fn reserve(
    db: &DatabaseConnection,
    source: Uuid,
    actor: Uuid,
    title: &str,
    key: &str,
) -> Result<Reservation, ApiError> {
    let (identity, proof) = editable(db, source, actor).await?;
    let title = title.trim();
    if title.is_empty() || title.chars().count() > 500 {
        return Err(ApiError::invalid_update("child title must contain 1..500 characters"));
    }
    let body = json!({"operation":"child_page_reserve","source_object_id":source,"title":title});
    let tx = db.begin().await?;
    if identity.workspace_id.is_none() {
        scope::require_personal_write(&tx).await?;
    }
    scope::fence(&tx, &proof).await?;
    if let Some(prior) = super::scope_commands::lock_and_replay(&tx, identity.scope_id, actor, key, &body).await? {
        let mut result: Reservation = serde_json::from_value(prior).map_err(|_| ApiError::internal())?;
        let row = reservation_row(&tx, result.child_page_id, false)
            .await?
            .ok_or_else(|| reason("child_reservation_unavailable"))?;
        check_pending(&row, source, actor)?;
        result.state = row.state;
        tx.commit().await?;
        return Ok(result);
    }
    let result = Reservation {
        reservation_id: Uuid::new_v4(),
        child_page_id: Uuid::new_v4(),
        source_object_id: source,
        title: title.into(),
        expires_at: Timestamp::now() + time::Duration::minutes(15),
        state: "pending".into(),
    };
    tx.execute(Statement::from_sql_and_values(DbBackend::Postgres,"INSERT INTO flow_child_page_reservations(id,scope_id,source_object_id,actor_id,child_page_id,title,expires_at) VALUES($1,$2,$3,$4,$5,$6,$7)",vec![result.reservation_id.into(),identity.scope_id.into(),source.into(),actor.into(),result.child_page_id.into(),result.title.clone().into(),result.expires_at.into()])).await?;
    let event = super::scope_event::insert_object_audit(
        &tx,
        &identity,
        actor,
        "flow.child_page.reserved",
        key,
        json!({"object_id":source}),
    )
    .await?;
    super::scope_commands::record(
        &tx,
        identity.scope_id,
        actor,
        key,
        body,
        source,
        event,
        serde_json::to_value(&result).map_err(|_| ApiError::internal())?,
    )
    .await?;
    tx.commit().await?;
    Ok(result)
}
fn check_pending(row: &Row, source: Uuid, actor: Uuid) -> Result<(), ApiError> {
    if row.source_object_id != source || row.actor_id != actor {
        return Err(reason("child_reservation_binding_mismatch"));
    }
    if row.state == "cancelled" {
        return Err(reason("child_reservation_cancelled"));
    }
    if row.state == "pending" && row.expired {
        return Err(reason("child_reservation_expired"));
    }
    Ok(())
}
async fn reservation_row<C: ConnectionTrait>(db: &C, child: Uuid, lock: bool) -> Result<Option<Row>, ApiError> {
    let suffix = if lock { " FOR UPDATE" } else { "" };
    Ok(Row::find_by_statement(Statement::from_sql_and_values(DbBackend::Postgres,format!("SELECT id,scope_id,source_object_id,actor_id,child_page_id,title,expires_at,state,expires_at<=now() AS expired FROM flow_child_page_reservations WHERE child_page_id=$1{suffix}"),vec![child.into()])).one(db).await?)
}
pub async fn cancel(
    db: &DatabaseConnection,
    source: Uuid,
    reservation: Uuid,
    actor: Uuid,
    key: &str,
) -> Result<Value, ApiError> {
    let (identity, proof) = editable(db, source, actor).await?;
    let tx = db.begin().await?;
    if identity.workspace_id.is_none() {
        scope::require_personal_write(&tx).await?;
    }
    scope::fence(&tx, &proof).await?;
    let body = json!({"operation":"child_page_cancel","reservation_id":reservation,"source_object_id":source});
    if let Some(prior) = super::scope_commands::lock_and_replay(&tx, identity.scope_id, actor, key, &body).await? {
        tx.commit().await?;
        return Ok(prior);
    }
    let changed=tx.execute(Statement::from_sql_and_values(DbBackend::Postgres,"UPDATE flow_child_page_reservations SET state='cancelled' WHERE id=$1 AND source_object_id=$2 AND scope_id=$3 AND actor_id=$4 AND state='pending'",vec![reservation.into(),source.into(),identity.scope_id.into(),actor.into()])).await?;
    if changed.rows_affected() != 1 {
        return Err(reason("child_reservation_not_pending"));
    }
    let result = json!({"reservation_id":reservation,"state":"cancelled"});
    let event = super::scope_event::insert_object_audit(
        &tx,
        &identity,
        actor,
        "flow.child_page.cancelled",
        key,
        json!({"object_id":source}),
    )
    .await?;
    super::scope_commands::record(&tx, identity.scope_id, actor, key, body, source, event, result.clone()).await?;
    tx.commit().await?;
    Ok(result)
}
#[derive(Debug, Clone)]
pub struct ChildReference {
    pub block_cid: String,
    pub child_page_id: Uuid,
}
pub fn typed_references(snapshot: &collab_core::SemanticSnapshot) -> Result<Vec<ChildReference>, ApiError> {
    snapshot
        .nodes
        .iter()
        .filter(|(_, node)| {
            !node.deleted
                && node.kind == collab_core::NodeKind::Block
                && node
                    .properties
                    .get("block_type")
                    .is_some_and(|kind| kind == "childPage")
        })
        .map(|(cid, node)| {
            Ok(ChildReference {
                block_cid: cid.to_string(),
                child_page_id: node
                    .properties
                    .get("child_page_id")
                    .and_then(|value| Uuid::parse_str(value).ok())
                    .ok_or_else(|| ApiError::invalid_update("childPage requires child_page_id"))?,
            })
        })
        .collect()
}
/// Called under the source authority fence and accepted document-head lock; no child exists
/// before this transaction, and any later validation failure rolls both aggregates back.
pub async fn reconcile_accepted(
    tx: &DatabaseTransaction,
    source: Uuid,
    actor: Option<Uuid>,
    seq: i64,
    references: &[ChildReference],
) -> Result<(), ApiError> {
    let identity = scope::identity(tx, source).await?;
    if identity.object_type != "page" {
        return Ok(());
    }
    for reference in references {
        let prior=tx.query_one(Statement::from_sql_and_values(DbBackend::Postgres,"SELECT child_page_id FROM flow_page_child_refs WHERE scope_id=$1 AND source_object_id=$2 AND block_cid=$3",vec![identity.scope_id.into(),source.into(),reference.block_cid.clone().into()])).await?;
        let unchanged =
            prior.and_then(|row| row.try_get::<Uuid>("", "child_page_id").ok()) == Some(reference.child_page_id);
        if unchanged {
            continue;
        }
        let actor =
            actor.ok_or_else(|| ApiError::unsupported_protocol("new child refs require a user-scoped credential"))?;
        let existing = tx
            .query_one(Statement::from_sql_and_values(
                DbBackend::Postgres,
                "SELECT object_type FROM flow_objects WHERE id=$1",
                vec![reference.child_page_id.into()],
            ))
            .await?;
        if let Some(existing) = existing {
            let target = scope::authorize(tx, reference.child_page_id, actor, PermissionLevel::View)
                .await
                .map_err(|_| reason("child_reference_unavailable"))?;
            if existing.try_get::<String>("", "object_type")? != "page" {
                return Err(reason("child_reference_not_page"));
            }
            if !target.resource_active {
                return Err(reason("child_reference_inactive"));
            }
            scope::fence_many(tx, &[target]).await?;
        } else {
            let reservation = reservation_row(tx, reference.child_page_id, true)
                .await?
                .ok_or_else(|| reason("child_reference_unavailable"))?;
            check_pending(&reservation, source, actor)?;
            if reservation.scope_id != identity.scope_id || reservation.state != "pending" {
                return Err(reason("child_reservation_unavailable"));
            }
            scope::insert_child_page(tx, &identity, actor, reservation.child_page_id, &reservation.title).await?;
            tx.execute(Statement::from_sql_and_values(DbBackend::Postgres,"UPDATE flow_child_page_reservations SET state='finalized',finalized_seq=$2,finalized_at=now() WHERE id=$1",vec![reservation.id.into(),seq.into()])).await?;
            let child = scope::identity(tx, reservation.child_page_id).await?;
            super::scope_event::insert_for_object(
                tx,
                &child,
                actor,
                "flow.object.created",
                "flow_object",
                child.id,
                &format!("child_finalize:{}", reservation.id),
                json!({"object_id":child.id,"object_type":"page","parent_object_id":source}),
                json!({}),
            )
            .await?;
        }
    }
    tx.execute(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "UPDATE flow_page_child_refs SET active=false WHERE scope_id=$1 AND source_object_id=$2 AND active",
        vec![identity.scope_id.into(), source.into()],
    ))
    .await?;
    for reference in references {
        tx.execute(Statement::from_sql_and_values(DbBackend::Postgres,"INSERT INTO flow_page_child_refs(scope_id,source_object_id,block_cid,child_page_id,active,first_seen_seq,last_seen_seq) VALUES($1,$2,$3,$4,true,$5,$5) ON CONFLICT(scope_id,source_object_id,block_cid) DO UPDATE SET child_page_id=EXCLUDED.child_page_id,active=true,last_seen_seq=EXCLUDED.last_seen_seq",vec![identity.scope_id.into(),source.into(),reference.block_cid.clone().into(),reference.child_page_id.into(),seq.into()])).await?;
    }
    Ok(())
}

pub async fn status(db: &DatabaseConnection, source: Uuid, id: Uuid, actor: Uuid) -> Result<Reservation, ApiError> {
    let proof = scope::authorize(db, source, actor, PermissionLevel::View).await?;
    let row=Row::find_by_statement(Statement::from_sql_and_values(DbBackend::Postgres,"SELECT id,scope_id,source_object_id,actor_id,child_page_id,title,expires_at,state,expires_at<=now() AS expired FROM flow_child_page_reservations WHERE id=$1 AND source_object_id=$2 AND scope_id=$3 AND actor_id=$4",vec![id.into(),source.into(),proof.scope_id.into(),actor.into()])).one(db).await?.ok_or_else(||reason("child_reservation_unavailable"))?;
    if !scope::proof_current(db, &proof).await? {
        return Err(super::policy::authorization_read_unstable());
    }
    Ok(Reservation {
        reservation_id: row.id,
        child_page_id: row.child_page_id,
        source_object_id: source,
        title: row.title,
        expires_at: row.expires_at,
        state: if row.state == "pending" && row.expired {
            "expired".into()
        } else {
            row.state
        },
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{routes::context::tenant_fixture::exec, scratch_or_skip};
    use collab_core::CollabEngine;
    #[test]
    fn reservation_binding_and_expiry_are_explicit_non_mutating_refusals() {
        let source = Uuid::new_v4();
        let actor = Uuid::new_v4();
        let mut row = Row {
            id: Uuid::new_v4(),
            scope_id: Uuid::new_v4(),
            source_object_id: source,
            actor_id: actor,
            child_page_id: Uuid::new_v4(),
            title: "Synthetic".into(),
            expires_at: Timestamp::now(),
            state: "pending".into(),
            expired: true,
        };
        let error = check_pending(&row, source, actor).unwrap_err();
        assert!(
            matches!(error,ApiError::Typed{details:Some(ref details),..} if details.get("reason").and_then(Value::as_str)==Some("child_reservation_expired"))
        );
        row.expired = false;
        assert!(check_pending(&row, source, Uuid::new_v4()).is_err());
        assert!(check_pending(&row, Uuid::new_v4(), actor).is_err());
        check_pending(&row, source, actor).unwrap();
    }
    #[tokio::test]
    #[allow(
        clippy::print_stderr,
        reason = "report the platform skip without a tracing subscriber"
    )]
    async fn child_reservation_actual_accept_rolls_back_and_retries_without_orphans() {
        use axum::{
            Extension, Router,
            body::{Body, to_bytes},
            http::Request,
            routing::{get, post},
        };
        use tower::ServiceExt;
        if !cfg!(target_os = "linux") {
            eprintln!("SKIPPED: actual isolated accepted-write path requires Linux");
            return;
        }
        let scratch = scratch_or_skip!(&format!("child_atomic_{}", Uuid::new_v4().simple()));
        let db = &scratch.db;
        // Compatibility rows are confined to this synthetic unit-test DB, not live readiness.
        db.execute_unprepared(
            "UPDATE flow_runtime_capabilities SET personal_provisioning=true,personal_writes=true WHERE singleton",
        )
        .await
        .unwrap();
        let owner = Uuid::new_v4();
        exec(db,"INSERT INTO users(id,email,password_hash,name,role,is_active) VALUES($1,$2,'!','Synthetic child owner','user',true)",vec![owner.into(),format!("{owner}@child-atomic.test").into()]).await;
        let home = scope::personal_home(db, owner).await.unwrap();
        let source = scope::create_personal(
            db,
            owner,
            scope::CreateInput {
                editor_model: 5,
                scope_id: home.0.id,
                object_type: "page".into(),
                parent_object_id: None,
                project_id: None,
                title: "Synthetic source".into(),
                idempotency_key: Uuid::new_v4().to_string(),
                message: None,
                initial_fields: vec![],
                initial_view: None,
            },
        )
        .await
        .unwrap();
        let object = Uuid::parse_str(
            source
                .pointer("/object/id")
                .expect("response contains id")
                .as_str()
                .unwrap(),
        )
        .unwrap();
        let document = Uuid::parse_str(
            source
                .pointer("/object/document_id")
                .expect("response contains document_id")
                .as_str()
                .unwrap(),
        )
        .unwrap();
        let state = platform::app::AppState {
            db: db.clone(),
            cfg: platform::config::AppConfig {
                app_name: "child-api-unit".into(),
                bind_addr: "127.0.0.1:0".into(),
                database_url: platform::config::Secret::new("postgres://unused/unused"),
                jwt_secret: platform::config::Secret::new("child-api-unit-secret"),
                jwt_access_ttl_seconds: 900,
                jwt_refresh_ttl_seconds: 3600,
                allow_insecure_cookies: false,
                collab_allowed_origins: vec![],
            },
            flow_permission_cache: platform::app::FlowPermissionCacheSlot::default(),
        };
        let claims = platform::auth::JwtClaims {
            sub: owner.to_string(),
            email: format!("{owner}@child-atomic.test"),
            token_type: platform::auth::TokenType::Access,
            iat: 0,
            exp: usize::MAX,
            sv: 0,
        };
        // Axum resolves these path placeholders when routing requests.
        #[allow(clippy::literal_string_with_formatting_args)]
        let app = Router::new()
            .route(
                "/objects/{object}/child-reservations",
                post(crate::routes::flow::post_flow_v2_child_reservation),
            )
            .route(
                "/objects/{object}/child-reservations/{reservation}",
                get(crate::routes::flow::get_flow_v2_child_reservation)
                    .delete(crate::routes::flow::delete_flow_v2_child_reservation),
            )
            .layer(Extension(claims))
            .with_state(state);
        let prepared = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(format!("/objects/{object}/child-reservations"))
                    .header("content-type", "application/json")
                    .header("Idempotency-Key", "reserve-intent")
                    .body(Body::from(json!({"title":"Synthetic pending child"}).to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();
        let prepared: Value =
            serde_json::from_slice(&to_bytes(prepared.into_body(), 1024 * 1024).await.unwrap()).unwrap();
        assert_eq!(prepared.get("code").and_then(Value::as_i64), Some(0));
        let reservation: Reservation =
            serde_json::from_value(prepared.get("data").expect("response contains data").clone()).unwrap();
        let pending = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri(format!(
                        "/objects/{object}/child-reservations/{}",
                        reservation.reservation_id
                    ))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let pending: Value =
            serde_json::from_slice(&to_bytes(pending.into_body(), 1024 * 1024).await.unwrap()).unwrap();
        assert_eq!(
            pending.pointer("/data/state").expect("response contains state"),
            "pending"
        );
        let cancelled = reserve(db, object, owner, "Synthetic cancelled child", "cancel-prepare")
            .await
            .unwrap();
        let cancelled_response = app
            .oneshot(
                Request::builder()
                    .method("DELETE")
                    .uri(format!(
                        "/objects/{object}/child-reservations/{}",
                        cancelled.reservation_id
                    ))
                    .header("Idempotency-Key", "cancel-action")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let cancelled_response: Value =
            serde_json::from_slice(&to_bytes(cancelled_response.into_body(), 1024 * 1024).await.unwrap()).unwrap();
        assert_eq!(
            cancelled_response
                .pointer("/data/state")
                .expect("response contains state"),
            "cancelled"
        );
        let replay = reserve(db, object, owner, "Synthetic pending child", "reserve-intent")
            .await
            .unwrap();
        assert_eq!(reservation.child_page_id, replay.child_page_id);
        let exists = db
            .query_one(Statement::from_sql_and_values(
                DbBackend::Postgres,
                "SELECT count(*) AS count FROM flow_objects WHERE id=$1",
                vec![reservation.child_page_id.into()],
            ))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            exists.try_get::<i64>("", "count").unwrap(),
            0,
            "prepare must not create a Page"
        );
        let mut engine = super::super::document::load_head_engine(db, document).await.unwrap();
        let before = engine.frontier();
        engine
            .insert_page_atom(
                "pending-child",
                None,
                1,
                "childPage",
                &json!({"child_page_id":reservation.child_page_id,"indent":0}),
            )
            .unwrap();
        let bytes = engine.export_from(&before).unwrap();
        let update = Uuid::new_v4();
        db.execute_unprepared("CREATE FUNCTION test_fail_child_ref() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN RAISE EXCEPTION USING ERRCODE='23514',MESSAGE='synthetic child finalization failure'; END $$; CREATE TRIGGER test_fail_child_ref BEFORE INSERT ON flow_page_child_refs FOR EACH ROW EXECUTE FUNCTION test_fail_child_ref()").await.unwrap();
        let first = super::super::scope_write::accept(
            db,
            object,
            bytes.clone(),
            super::super::scope_write::UpdateIntent {
                user: owner,
                update,
                client: "synthetic-child",
                key: Some("child-commit"),
                expected: Some(before.as_bytes()),
                command_body: Some(json!({"intent":"child"})),
            },
        )
        .await;
        assert!(first.is_err());
        let row=db.query_one(Statement::from_sql_and_values(DbBackend::Postgres,"SELECT (SELECT count(*) FROM flow_objects WHERE id=$1) AS objects,(SELECT count(*) FROM flow_objects WHERE parent_id=$2 AND id=$1) AS nav,(SELECT count(*) FROM collab_documents WHERE object_id=$1) AS docs,(SELECT count(*) FROM business_events WHERE aggregate_id=$1::text AND event_type='flow.object.created') AS events,(SELECT head_seq FROM collab_documents WHERE id=$3) AS head,(SELECT count(*) FROM collab_updates WHERE document_id=$3) AS updates,(SELECT count(*) FROM flow_scope_command_receipts WHERE idempotency_key='child-commit') AS receipts",vec![reservation.child_page_id.into(),object.into(),document.into()])).await.unwrap().unwrap();
        for key in ["objects", "nav", "docs", "events", "head", "updates", "receipts"] {
            assert_eq!(row.try_get::<i64>("", key).unwrap(), 0, "rollback must leave no {key}");
        }
        db.execute_unprepared(
            "DROP TRIGGER test_fail_child_ref ON flow_page_child_refs; DROP FUNCTION test_fail_child_ref()",
        )
        .await
        .unwrap();
        let second = super::super::scope_write::accept(
            db,
            object,
            bytes,
            super::super::scope_write::UpdateIntent {
                user: owner,
                update,
                client: "synthetic-child",
                key: Some("child-commit"),
                expected: Some(before.as_bytes()),
                command_body: Some(json!({"intent":"child"})),
            },
        )
        .await
        .unwrap();
        assert!(matches!(
            second,
            super::super::collab::write::AcceptOutcome::Accepted(_)
        ));
        let row=db.query_one(Statement::from_sql_and_values(DbBackend::Postgres,"SELECT (SELECT count(*) FROM flow_objects WHERE id=$1 AND parent_id=$2) AS nav,(SELECT head_seq FROM collab_documents WHERE id=$3) AS head,(SELECT state FROM flow_child_page_reservations WHERE id=$4) AS state",vec![reservation.child_page_id.into(),object.into(),document.into(),reservation.reservation_id.into()])).await.unwrap().unwrap();
        assert_eq!(row.try_get::<i64>("", "nav").unwrap(), 1);
        assert_eq!(row.try_get::<i64>("", "head").unwrap(), 1);
        assert_eq!(row.try_get::<String>("", "state").unwrap(), "finalized");
        scratch.drop_self().await;
    }
}
