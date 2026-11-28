//! Scope-owned maintenance queue. Target document identity never substitutes for Flow ownership.
use super::{collab::authz::PermissionLevel, scope};
use crate::error::ApiError;
use sea_orm::{ConnectionTrait, DatabaseConnection, DatabaseTransaction, DbBackend, Statement, TransactionTrait};
use serde::Deserialize;
use serde_json::{Value, json};
use uuid::Uuid;
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EnqueueInput {
    pub operation: String,
    pub expected_head_seq: i64,
    pub dry_run: bool,
    pub idempotency_key: String,
    #[serde(default)]
    pub confirm_quarantine: bool,
}
pub async fn enqueue(
    db: &DatabaseConnection,
    object: Uuid,
    user: Uuid,
    input: EnqueueInput,
) -> Result<Value, ApiError> {
    if !matches!(
        input.operation.as_str(),
        "rebuild_projection" | "rebuild_search" | "verify_document" | "compact" | "repair_quarantine"
    ) {
        return Err(ApiError::invalid_update("unsupported scoped maintenance operation"));
    }
    if input.operation == "repair_quarantine" && !input.dry_run && !input.confirm_quarantine {
        return Err(ApiError::policy_rejected(
            "quarantine execution requires confirm_quarantine=true",
        ));
    }
    let identity = scope::identity(db, object).await?;
    let proof = scope::authorize(db, object, user, PermissionLevel::FullAccess).await?;
    if identity.workspace_id.is_none() {
        scope::require_personal_write(db).await?;
    }
    let body = json!({"operation":"maintenance_enqueue","object_id":object,"maintenance_operation":input.operation,"expected_head_seq":input.expected_head_seq,"dry_run":input.dry_run,"confirm_quarantine":input.confirm_quarantine});
    let tx = db.begin().await?;
    scope::fence(&tx, &proof).await?;
    if let Some(result) =
        super::scope_commands::lock_and_replay(&tx, identity.scope_id, user, &input.idempotency_key, &body).await?
    {
        tx.commit().await?;
        return Ok(result);
    }
    let head = tx
        .query_one(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT id,head_seq FROM collab_documents WHERE object_id=$1 FOR SHARE",
            vec![object.into()],
        ))
        .await?
        .ok_or_else(|| ApiError::not_found("Flow document not found"))?;
    let document: Uuid = head.try_get("", "id")?;
    let seq: i64 = head.try_get("", "head_seq")?;
    if seq != input.expected_head_seq {
        return Err(ApiError::stale_frontier("maintenance head changed", Some(seq), None));
    }
    let id = Uuid::new_v4();
    tx.execute(Statement::from_sql_and_values(DbBackend::Postgres,
        "INSERT INTO flow_operation_runs(id,owner_flow_scope_id,workspace_id,operation,scope_kind,scope_id,dry_run,expected_head_seq,status,actor_id,principal_kind,principal_id,idempotency_key,request_hash) VALUES($1,$2,$3,$4,'document',$5,$6,$7,'planned',$8,'user',$8,$9,$10)",
        vec![id.into(),identity.scope_id.into(),identity.workspace_id.into(),input.operation.clone().into(),document.into(),input.dry_run.into(),seq.into(),user.into(),input.idempotency_key.clone().into(),super::collab::bootstrap::content_hash(&serde_json::to_vec(&body).map_err(|_|ApiError::internal())?).into()])).await?;
    let event = super::scope_event::insert_object_audit(
        &tx,
        &identity,
        user,
        "flow.operation.queued",
        &format!("operation-queued:{}", input.idempotency_key),
        json!({"object_id":object,"operation_id":id,"operation":input.operation}),
    )
    .await?;
    let result = json!({"operation_id":id,"operation":input.operation,"status":"planned","scope_id":identity.scope_id,"object_id":object,"document_id":document,"expected_head_seq":seq,"dry_run":input.dry_run,"audit_event_id":event});
    super::scope_commands::record(
        &tx,
        identity.scope_id,
        user,
        &input.idempotency_key,
        body,
        object,
        event,
        result.clone(),
    )
    .await?;
    tx.commit().await?;
    Ok(result)
}
pub async fn status(db: &DatabaseConnection, scope_id: Uuid, user: Uuid, id: Uuid) -> Result<Value, ApiError> {
    let row=db.query_one(Statement::from_sql_and_values(DbBackend::Postgres,
        "SELECT r.operation,r.status,r.scope_id AS document_id,r.dry_run,r.expected_head_seq,r.result_redacted,r.created_at,r.finished_at,o.id AS object_id FROM flow_operation_runs r JOIN collab_documents d ON d.id=r.scope_id JOIN flow_objects o ON o.id=d.object_id WHERE r.id=$1 AND r.owner_flow_scope_id=$2",vec![id.into(),scope_id.into()])).await?.ok_or_else(||ApiError::not_found("Flow operation not found"))?;
    let object: Uuid = row.try_get("", "object_id")?;
    let proof = scope::authorize(db, object, user, PermissionLevel::FullAccess).await?;
    let result = json!({"operation_id":id,"scope_id":scope_id,"object_id":object,"document_id":row.try_get::<Uuid>("","document_id")?,"operation":row.try_get::<String>("","operation")?,"status":row.try_get::<String>("","status")?,"dry_run":row.try_get::<bool>("","dry_run")?,"expected_head_seq":row.try_get::<Option<i64>>("","expected_head_seq")?,"result":row.try_get::<Value>("","result_redacted")?});
    if !scope::proof_current(db, &proof).await? {
        return Err(super::policy::authorization_read_unstable());
    }
    Ok(result)
}
/// Held across a worker's shared domain operation and its completion. No user impersonation:
/// this revalidates the original human maintenance intent and prevents revocation crossing commit.
pub async fn job_authority(db: &DatabaseConnection, id: Uuid) -> Result<Option<DatabaseTransaction>, ApiError> {
    let row=db.query_one(Statement::from_sql_and_values(DbBackend::Postgres,
        "SELECT r.principal_kind,r.principal_id,o.id AS object_id FROM flow_operation_runs r JOIN collab_documents d ON d.id=r.scope_id JOIN flow_objects o ON o.id=d.object_id AND o.scope_id=r.owner_flow_scope_id WHERE r.id=$1",vec![id.into()])).await?.ok_or_else(||ApiError::not_found("Flow maintenance target not found"))?;
    // Historical system/bot jobs keep their existing operator contract; scope-native enqueues are human-only.
    if row.try_get::<Option<String>>("", "principal_kind")?.as_deref() != Some("user") {
        return Ok(None);
    }
    let actor = row
        .try_get::<Option<Uuid>>("", "principal_id")?
        .ok_or_else(ApiError::internal)?;
    let proof = scope::authorize(db, row.try_get("", "object_id")?, actor, PermissionLevel::FullAccess).await?;
    let tx = db.begin().await?;
    scope::fence(&tx, &proof).await?;
    Ok(Some(tx))
}
pub async fn health(db: &DatabaseConnection, scope_id: Uuid, user: Uuid) -> Result<Value, ApiError> {
    let owned = scope::get(db, scope_id).await?;
    if owned.owner_user_id != Some(user) {
        let workspace = scope::bound_workspace(db, scope_id)
            .await?
            .ok_or_else(|| ApiError::not_found("Flow scope not found"))?;
        if !super::scope_mounts::destination_can_manage(db, workspace, user).await? {
            return Err(ApiError::not_found("Flow scope not found"));
        }
    }
    let roots = scope::roots(db, scope_id).await?;
    let root = roots
        .first()
        .ok_or_else(|| ApiError::not_found("Flow scope not found"))?
        .object_id;
    let proof = scope::authorize(db, root, user, PermissionLevel::FullAccess).await?;
    let row=db.query_one(Statement::from_sql_and_values(DbBackend::Postgres,
        "SELECT (SELECT count(*) FROM flow_objects WHERE scope_id=$1) AS objects,(SELECT count(*) FROM flow_operation_runs WHERE owner_flow_scope_id=$1 AND status IN('planned','running')) AS queue_depth,(SELECT count(*) FROM flow_integrity_records WHERE scope_id=$1 AND status='open') AS open_integrity_records,COALESCE((SELECT sum(octet_length(d.snapshot)) FROM collab_documents d JOIN flow_objects o ON o.id=d.object_id WHERE o.scope_id=$1),0)::bigint AS snapshot_bytes",vec![scope_id.into()])).await?.ok_or_else(ApiError::internal)?;
    let result = json!({"scope_id":scope_id,"objects":row.try_get::<i64>("","objects")?,"queue_depth":row.try_get::<i64>("","queue_depth")?,"open_integrity_records":row.try_get::<i64>("","open_integrity_records")?,"snapshot_bytes":row.try_get::<i64>("","snapshot_bytes")?,"supported_operations":["compact","rebuild_projection","rebuild_search","verify_document","repair_quarantine"]});
    if !scope::proof_current(db, &proof).await? {
        return Err(super::policy::authorization_read_unstable());
    }
    Ok(result)
}

/// Queue consumer uses the existing snapshot/ack-aware compactor; force-resync is never implicit.
pub async fn run_compaction_jobs(db: &DatabaseConnection, batch: usize) -> Result<u64, ApiError> {
    if super::rollback::control(db).await?.compaction_paused {
        return Ok(0);
    }
    let mut completed = 0;
    for _ in 0..batch.clamp(1, 100) {
        let Some(row)=db.query_one(Statement::from_string(DbBackend::Postgres,
            "UPDATE flow_operation_runs SET status='running' WHERE id=(SELECT id FROM flow_operation_runs WHERE operation='compact' AND status='planned' ORDER BY created_at,id FOR UPDATE SKIP LOCKED LIMIT 1) RETURNING id,scope_id,dry_run,expected_head_seq".to_owned())).await? else {break;};
        let id: Uuid = row.try_get("", "id")?;
        let authority = job_authority(db, id).await;
        let (_guard, outcome) = match authority {
            Err(error) => (None, Err(error)),
            Ok(guard) => {
                let outcome=async {
                    let document=row.try_get("","scope_id")?;
                    let expected:i64=row.try_get::<Option<i64>>("","expected_head_seq")?.ok_or_else(||ApiError::invalid_update("maintenance requires an expected head"))?;
                    if row.try_get::<bool>("","dry_run")? {
                        let boot=super::collab::bootstrap::load(db,document).await?;
                        if boot.head_seq!=expected{return Err(ApiError::stale_frontier("maintenance head changed",Some(boot.head_seq),None));}
                        Ok(json!({"head_seq":boot.head_seq,"snapshot_seq":boot.snapshot_seq,"would_compact":boot.snapshot_seq<boot.head_seq}))
                    }else{
                        let compact=super::collab::compaction::compact(db,document,expected,false).await?;
                        Ok(json!({"document_id":compact.document_id,"head_seq":compact.head_seq,"previous_snapshot_seq":compact.previous_snapshot_seq,"snapshot_checksum":compact.snapshot_checksum,"deleted_updates":compact.deleted_updates,"lagging_clients":compact.lagging_clients,"forced_resync_clients":compact.forced_resync_clients}))
                    }
                }.await;
                (guard, outcome)
            }
        };
        let (status, result) = match outcome {
            Ok(result) => {
                completed += 1;
                ("completed", result)
            }
            Err(error) => {
                tracing::warn!(%id,%error,"scope compaction job failed");
                ("failed", json!({"reason":"compaction_failed"}))
            }
        };
        db.execute(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "UPDATE flow_operation_runs SET status=$2,result_redacted=$3,finished_at=now() WHERE id=$1",
            vec![id.into(), status.into(), result.into()],
        ))
        .await?;
    }
    Ok(completed)
}

/// Quarantine means acknowledging the selected findings, never rewriting a damaged document.
pub async fn run_quarantine_jobs(db: &DatabaseConnection, batch: usize) -> Result<u64, ApiError> {
    let mut completed = 0;
    for _ in 0..batch.clamp(1, 100) {
        let Some(row)=db.query_one(Statement::from_string(DbBackend::Postgres,
            "UPDATE flow_operation_runs SET status='running' WHERE id=(SELECT id FROM flow_operation_runs WHERE operation='repair_quarantine' AND status='planned' ORDER BY created_at,id FOR UPDATE SKIP LOCKED LIMIT 1) RETURNING id,owner_flow_scope_id,scope_id,dry_run,principal_id,expected_head_seq".to_owned())).await? else{break;};
        let id: Uuid = row.try_get("", "id")?;
        let authority = job_authority(db, id).await;
        let (_guard, outcome) = match authority {
            Err(error) => (None, Err(error)),
            Ok(guard) => {
                let outcome=async {
                    let document:Uuid=row.try_get("","scope_id")?;
                    let owner:Uuid=row.try_get("","owner_flow_scope_id")?;
                    let user:Uuid=row.try_get::<Option<Uuid>>("","principal_id")?.ok_or_else(ApiError::internal)?;
                    let tx=db.begin().await?;
                    let head=tx.query_one(Statement::from_sql_and_values(DbBackend::Postgres,
                        "SELECT object_id,head_seq FROM collab_documents WHERE id=$1 FOR SHARE",vec![document.into()])).await?.ok_or_else(||ApiError::not_found("Flow document not found"))?;
                    if Some(head.try_get::<i64>("","head_seq")?)!=row.try_get::<Option<i64>>("","expected_head_seq")?{return Err(ApiError::stale_frontier("maintenance head changed",Some(head.try_get("","head_seq")?),None));}
                    let object:Uuid=head.try_get("","object_id")?;
                    let findings=tx.query_all(Statement::from_sql_and_values(DbBackend::Postgres,
                        "SELECT id FROM flow_integrity_records WHERE scope_id=$1 AND status='open' AND ((subject_kind='collab_document' AND subject_id=$2) OR(subject_kind='flow_object' AND subject_id=$3)) ORDER BY id FOR UPDATE",vec![owner.into(),document.to_string().into(),object.to_string().into()])).await?;
                    let ids=findings.into_iter().map(|finding|finding.try_get::<Uuid>("","id")).collect::<Result<Vec<_>,_>>()?;
                    let dry_run:bool=row.try_get("","dry_run")?;
                    if !dry_run {
                        tx.execute(Statement::from_sql_and_values(DbBackend::Postgres,
                            "UPDATE flow_integrity_records SET status='ignored',resolved_at=now() WHERE id=ANY($1)",vec![ids.clone().into()])).await?;
                    }
                    let result=json!({"object_id":object,"integrity_record_ids":ids,"integrity_record_count":ids.len(),"dry_run":dry_run});
                    super::scope_event::insert_object_audit(&tx,&scope::identity(&tx,object).await?,user,
                        if dry_run{"flow.operation.previewed"}else{"flow.repair.completed"},&format!("scope-operation:{id}"),json!({"operation_id":id,"object_id":object,"integrity_record_count":ids.len(),"dry_run":dry_run})).await?;
                    tx.execute(Statement::from_sql_and_values(DbBackend::Postgres,
                        "UPDATE flow_operation_runs SET status='completed',result_redacted=$2,finished_at=now() WHERE id=$1",vec![id.into(),result.into()])).await?;
                    tx.commit().await?;Ok(())
                }.await;
                (guard, outcome)
            }
        };
        match outcome {
            Ok(()) => completed += 1,
            Err(error) => {
                tracing::warn!(%id,%error,"scope quarantine job failed");
                db.execute(Statement::from_sql_and_values(DbBackend::Postgres,"UPDATE flow_operation_runs SET status='failed',result_redacted=jsonb_build_object('reason','quarantine_failed'),finished_at=now() WHERE id=$1",vec![id.into()])).await?;
            }
        }
    }
    Ok(completed)
}
