//! Scope-owned package staging/preview/promotion, sharing the verified package model and worker.
//!
//! Personal destinations have no workspace or project. Only tested Page/Navigator snapshots
//! are admitted here. Included update members replay to a validated head, matching v1; source
//! application update/event rows are not recreated as a new target timeline.
use super::{collab::authz::PermissionLevel, package_import as shared, scope};
use crate::error::ApiError;
use crate::flow::json_value::FlowJson;
use base64::{Engine, engine::general_purpose::STANDARD};
use platform::timestamp::Timestamp;
use sea_orm::{ConnectionTrait, DatabaseConnection, DatabaseTransaction, DbBackend, Statement, TransactionTrait};
use serde::Deserialize;
use serde_json::{Value, json};
use std::{
    collections::{BTreeMap, BTreeSet},
    io::Cursor,
};
use uuid::Uuid;
#[derive(Deserialize)]
pub struct PreviewInput {
    pub artifact_id: Uuid,
    #[serde(default)]
    pub project_mapping: BTreeMap<Uuid, Option<Uuid>>,
    pub external_reference_policy: shared::ExternalReferencePolicy,
    pub conflict_policy: shared::ConflictPolicy,
    pub include_history: bool,
    pub idempotency_key: String,
}
#[derive(Deserialize)]
pub struct CommitInput {
    pub package_sha256: String,
    pub mapping_hash: String,
    pub conflict_policy: shared::ConflictPolicy,
    pub confirm: bool,
    pub idempotency_key: String,
}
async fn owner<C: ConnectionTrait>(db: &C, scope_id: Uuid, user: Uuid) -> Result<scope::AuthorizationProof, ApiError> {
    let scope = scope::get(db, scope_id).await?;
    if scope.kind != "personal" || scope.owner_user_id != Some(user) || scope.lifecycle_status != "active" {
        return Err(ApiError::not_found("personal import destination not found"));
    }
    let root = scope::roots(db, scope_id)
        .await?
        .into_iter()
        .find(|r| r.project_id.is_none())
        .ok_or_else(ApiError::internal)?
        .object_id;
    scope::authorize(db, root, user, PermissionLevel::FullAccess).await
}
pub async fn require_destination(db: &DatabaseConnection, scope_id: Uuid, user: Uuid) -> Result<(), ApiError> {
    owner(db, scope_id, user).await?;
    require_enabled(db).await
}
async fn require_enabled<C: ConnectionTrait>(db: &C) -> Result<(), ApiError> {
    scope::require_personal_write(db).await?;
    if !scope::capabilities(db).await?.personal_package_imports {
        return Err(ApiError::typed_with_details(
            crate::error::ApiErrorKind::FeatureDisabled,
            "personal package imports are not enabled",
            json!({"reason":"personal_package_imports_disabled"}),
        ));
    }
    Ok(())
}
pub async fn upload(
    db: &DatabaseConnection,
    scope_id: Uuid,
    user: Uuid,
    bytes: Vec<u8>,
    expected: Option<&str>,
    key: &str,
) -> Result<Value, ApiError> {
    let proof = owner(db, scope_id, user).await?;
    require_enabled(db).await?;
    let verified = super::package::verify_package(Cursor::new(&bytes), expected)?;
    let body = json!({"operation":"import_upload","package_sha256":verified.package_sha256});
    let tx = db.begin().await?;
    require_enabled(&tx).await?;
    scope::fence(&tx, &proof).await?;
    if let Some(result) = super::scope_commands::lock_and_replay(&tx, scope_id, user, key, &body).await? {
        tx.commit().await?;
        return Ok(result);
    }
    let artifact = Uuid::new_v4();
    let expiry = Timestamp::now() + time::Duration::minutes(30);
    let size = i64::try_from(bytes.len()).map_err(|_| ApiError::internal())?;
    tx.execute(Statement::from_sql_and_values(DbBackend::Postgres,"INSERT INTO flow_package_artifacts(id,scope_id,workspace_id,actor_kind,actor_id,purpose,package_sha256,idempotency_key,request_hash,size_bytes,package_bytes,expires_at) VALUES($1,$2,NULL,'user',$3,'import',$4,$5,$4,$6,$7,$8)",vec![artifact.into(),scope_id.into(),user.into(),verified.package_sha256.clone().into(),key.into(),size.into(),bytes.into(),expiry.into()])).await?;
    let event = super::scope_event::insert_object_audit(
        &tx,
        &scope::identity(&tx, proof.object_id).await?,
        user,
        "flow.import.uploaded",
        key,
        json!({"object_id":proof.object_id,"artifact_id":artifact,"package_sha256":verified.package_sha256}),
    )
    .await?;
    let result = json!({"artifact_id":artifact,"package_sha256":verified.package_sha256,"size":size,"expires_at":expiry.to_rfc3339()});
    super::scope_commands::record(&tx, scope_id, user, key, body, proof.object_id, event, result.clone()).await?;
    tx.commit().await?;
    Ok(result)
}
async fn artifact<C: ConnectionTrait>(
    db: &C,
    scope_id: Uuid,
    user: Uuid,
    id: Uuid,
) -> Result<(Vec<u8>, String), ApiError> {
    let row=db.query_one(Statement::from_sql_and_values(DbBackend::Postgres,"SELECT package_bytes,package_sha256 FROM flow_package_artifacts WHERE id=$1 AND scope_id=$2 AND workspace_id IS NULL AND actor_kind='user' AND actor_id=$3 AND purpose='import' AND expires_at>now()",vec![id.into(),scope_id.into(),user.into()])).await?.ok_or_else(||ApiError::not_found("import artifact unavailable or expired"))?;
    Ok((row.try_get("", "package_bytes")?, row.try_get("", "package_sha256")?))
}
fn known_plan(plan: &shared::PackagePlan) -> Result<(), ApiError> {
    for object in &plan.objects {
        if !matches!(object.object_type.as_str(), "page" | "navigator")
            || !matches!(object.lifecycle_status.as_str(), "active" | "archived")
            || object.engine != "loro"
            || object.format_version != "loro-1"
        {
            return Err(ApiError::unsupported_format(
                "scope import requires supported Page/Navigator documents",
            ));
        }
    }
    Ok(())
}
async fn lineage<C: ConnectionTrait>(
    db: &C,
    scope_id: Uuid,
    hash: &str,
) -> Result<BTreeMap<(String, Uuid), Uuid>, ApiError> {
    let rows=db.query_all(Statement::from_sql_and_values(DbBackend::Postgres,"SELECT target_kind,source_id,target_id FROM flow_import_lineage WHERE target_scope_id=$1 AND package_sha256=$2 AND source_kind='flow_package' ORDER BY target_kind,source_id",vec![scope_id.into(),hash.into()])).await?;
    rows.into_iter()
        .map(|r| {
            Ok((
                (r.try_get("", "target_kind")?, r.try_get("", "source_id")?),
                r.try_get("", "target_id")?,
            ))
        })
        .collect()
}
pub async fn preview(
    db: &DatabaseConnection,
    scope_id: Uuid,
    user: Uuid,
    input: PreviewInput,
) -> Result<Value, ApiError> {
    let proof = owner(db, scope_id, user).await?;
    require_enabled(db).await?;
    if input.project_mapping.values().any(Option::is_some) {
        return Err(ApiError::invalid_update(
            "personal imports map source projects only to unprojected",
        ));
    }
    let (bytes, hash) = artifact(db, scope_id, user, input.artifact_id).await?;
    let plan = shared::parse_plan(&bytes, &hash)?;
    known_plan(&plan)?;
    if input.include_history != plan.verified.manifest.history.included {
        return Err(ApiError::invalid_update(
            "include_history must match the verified manifest",
        ));
    }
    let body = json!({"operation":"import_preview","artifact_id":input.artifact_id,"project_mapping":input.project_mapping,"external_reference_policy":input.external_reference_policy,"conflict_policy":input.conflict_policy,"include_history":input.include_history});
    let tx = db.begin().await?;
    require_enabled(&tx).await?;
    scope::fence(&tx, &proof).await?;
    if let Some(result) =
        super::scope_commands::lock_and_replay(&tx, scope_id, user, &input.idempotency_key, &body).await?
    {
        tx.commit().await?;
        return Ok(result);
    }
    let prior = lineage(&tx, scope_id, &hash).await?;
    let reuse = input.conflict_policy == shared::ConflictPolicy::ReuseImportLineage;
    let mut objects = BTreeMap::new();
    let mut documents = BTreeMap::new();
    let mut relations = BTreeMap::new();
    let mut conflicts = Vec::new();
    let projects = plan
        .objects
        .iter()
        .filter_map(|o| o.project_id)
        .collect::<BTreeSet<_>>();
    for project in projects {
        if !input.project_mapping.contains_key(&project) {
            conflicts.push(format!("project_unmapped:{project}"));
        }
    }
    for object in &plan.objects {
        if !reuse && prior.contains_key(&("object".into(), object.source_object_id)) {
            conflicts.push(object.source_object_id.to_string());
        }
        objects.insert(
            object.source_object_id,
            if reuse {
                prior.get(&("object".into(), object.source_object_id)).copied()
            } else {
                None
            }
            .unwrap_or_else(Uuid::new_v4),
        );
        documents.insert(
            object.source_document_id,
            if reuse {
                prior.get(&("document".into(), object.source_document_id)).copied()
            } else {
                None
            }
            .unwrap_or_else(Uuid::new_v4),
        );
    }
    for relation in &plan.relations {
        if (relation.source.kind == "external" || relation.target.kind == "external")
            && input.external_reference_policy == shared::ExternalReferencePolicy::Reject
        {
            conflicts.push(format!("external_reference:{}", relation.source_relation_id));
        }
        relations.insert(
            relation.source_relation_id,
            if reuse {
                prior.get(&("relation".into(), relation.source_relation_id)).copied()
            } else {
                None
            }
            .unwrap_or_else(Uuid::new_v4),
        );
    }
    let mut scope_map = BTreeMap::new();
    if let Some(source) = plan.verified.manifest.source.scope_id.as_ref() {
        scope_map.insert(source.clone(), scope_id);
    }
    let mapping = json!({"target_scope_id":scope_id,"scope_map":scope_map,"workspace_map":{},"object_map":objects,"document_map":documents,"relation_map":relations,"project_map":input.project_mapping,"external_reference_policy":input.external_reference_policy,"conflict_policy":input.conflict_policy});
    let mapping_hash = shared::sha256_json(&mapping)?;
    let preview = Uuid::new_v4();
    let expiry = Timestamp::now() + time::Duration::minutes(30);
    let summary = shared::manifest_summary(&plan);
    let estimated = shared::estimated_changes(&plan);
    let result = json!({"preview_id":preview,"package_id":plan.verified.manifest.package_id,"package_sha256":hash,"mapping_hash":mapping_hash,"manifest_summary":summary,"mapping":mapping,"conflicts":conflicts,"conflict_details":[],"warnings":["crdt_snapshot_history_retained","inner_reference_validation_before_promotion"],"estimated_changes":estimated,"expires_at":expiry.to_rfc3339()});
    tx.execute(Statement::from_sql_and_values(DbBackend::Postgres,"INSERT INTO flow_import_previews(id,scope_id,workspace_id,artifact_id,actor_kind,actor_id,package_sha256,mapping_hash,mapping,request,manifest_summary,conflicts,warnings,estimated_changes,idempotency_key,request_hash,expires_at) VALUES($1,$2,NULL,$3,'user',$4,$5,$6,$7,$8,$9,$10,$11,$12,$13,$14,$15)",vec![preview.into(),scope_id.into(),input.artifact_id.into(),user.into(),hash.into(),mapping_hash.into(),mapping.into(),body.clone().into(),serde_json::to_value(summary).map_err(|_|ApiError::internal())?.into(),json!(conflicts).into(),json!(["crdt_snapshot_history_retained","inner_reference_validation_before_promotion"]).into(),json!(estimated).into(),input.idempotency_key.clone().into(),shared::sha256_json(&body)?.into(),expiry.into()])).await?;
    let event = super::scope_event::insert_object_audit(
        &tx,
        &scope::identity(&tx, proof.object_id).await?,
        user,
        "flow.import.previewed",
        &input.idempotency_key,
        json!({"object_id":proof.object_id,"preview_id":preview}),
    )
    .await?;
    super::scope_commands::record(
        &tx,
        scope_id,
        user,
        &input.idempotency_key,
        body,
        proof.object_id,
        event,
        result.clone(),
    )
    .await?;
    tx.commit().await?;
    Ok(result)
}
#[cfg(test)]
tokio::task_local! {
    // Explicit developer unit-test isolation boundary only. No production environment/request
    // flag can populate this proof map; production always executes the isolated worker below.
    static TEST_VERIFIED_REPLAY: BTreeMap<Uuid,collab_core::isolation::wire::ReplayDiffResult>;
}
async fn replay_snapshot(
    snapshot: Vec<u8>,
    request: collab_core::isolation::wire::ReplayDiffRequest,
    object: Uuid,
) -> Result<collab_core::isolation::wire::ReplayDiffResult, ApiError> {
    #[cfg(test)]
    if let Ok(Some(result)) = TEST_VERIFIED_REPLAY.try_with(|map| map.get(&object).cloned()) {
        return Ok(result);
    }
    let (result, observation) =
        tokio::task::spawn_blocking(move || collab_core::isolation::isolated_diff_observed(&snapshot, &request))
            .await
            .map_err(|_| ApiError::internal())?;
    result.map_err(|e| {
        super::query::isolated_diff_error(e, &observation, object, || {
            ApiError::checksum_mismatch("package snapshot cannot be reconstructed")
        })
    })
}
/// The replayed head of an uploaded package document (its snapshot with its whole update tail)
/// must not carry a root container with a reserved name and the wrong type. The head is what the
/// worker exported after replaying the package, so its history holds both the uploaded snapshot's
/// and the uploaded tail's operations. The check reads all of it, outside the isolated window.
async fn require_head_without_shadow_roots(
    validated: &collab_core::isolation::wire::ReplayDiffResult,
) -> Result<(), ApiError> {
    let head = STANDARD
        .decode(
            validated
                .validated_head_snapshot_base64
                .as_deref()
                .ok_or_else(|| ApiError::unsupported_protocol("isolated worker lacks package replay support"))?,
        )
        .map_err(|_| ApiError::internal())?;
    tokio::task::spawn_blocking(move || {
        let engine = <collab_core::LoroCollabEngine as collab_core::CollabEngine>::load(&head)
            .map_err(|_| ApiError::checksum_mismatch("package head cannot be decoded"))?;
        super::package::require_no_shadow_roots(&engine)
    })
    .await
    .map_err(|_| ApiError::internal())?
}
struct Validated {
    source: Uuid,
    snapshot: Vec<u8>,
    frontier: Vec<u8>,
    semantic: collab_core::SemanticSnapshot,
    title: String,
}
async fn validate_snapshots(bytes: &[u8], plan: &shared::PackagePlan) -> Result<Vec<Validated>, ApiError> {
    let mut result = Vec::new();
    for object in &plan.objects {
        let snapshot = shared::member_bytes(bytes, &format!("documents/{}/snapshot.bin", object.source_document_id))?;
        let frontier = STANDARD
            .decode(&object.accepted_frontier)
            .map_err(|_| ApiError::unsupported_format("invalid accepted frontier"))?;
        let prefix = format!("documents/{}/updates/", object.source_document_id);
        let mut updates = plan
            .verified
            .manifest
            .members
            .iter()
            .filter(|m| m.kind == "update" && m.path.starts_with(&prefix))
            .map(|m| {
                let seq = m
                    .path
                    .strip_prefix(&prefix)
                    .and_then(|s| s.strip_suffix(".bin"))
                    .and_then(|s| s.parse::<i64>().ok())
                    .ok_or_else(|| ApiError::unsupported_format("invalid package update sequence"))?;
                Ok((seq, m.path.clone()))
            })
            .collect::<Result<Vec<_>, ApiError>>()?;
        updates.sort_by_key(|v| v.0);
        let first = object
            .accepted_seq
            .saturating_sub(i64::try_from(updates.len()).unwrap_or(i64::MAX))
            .saturating_add(1);
        let mut encoded = Vec::new();
        for (index, (seq, path)) in updates.into_iter().enumerate() {
            if seq < 1 || seq != first.saturating_add(i64::try_from(index).unwrap_or(i64::MAX)) {
                return Err(ApiError::checksum_mismatch("package history tail is discontinuous"));
            }
            encoded.push(STANDARD.encode(shared::member_bytes(bytes, &path)?));
        }
        let request = collab_core::isolation::wire::ReplayDiffRequest {
            snapshot_frontier: frontier.clone(),
            updates: Vec::new(),
            from_frontier: frontier.clone(),
            to_frontier: frontier.clone(),
            package_updates_base64: Some(encoded),
        };
        let validated = replay_snapshot(snapshot, request, object.source_object_id).await?;
        require_head_without_shadow_roots(&validated).await?;
        let hash = match super::package::semantic_model(&plan.verified.manifest)? {
            super::package::PackageSemanticModel::V1 => {
                validated.legacy_package_semantic_hash_v1.clone().ok_or_else(|| {
                    ApiError::unsupported_protocol("isolated worker lacks the v1 package fingerprint reader")
                })?
            }
            super::package::PackageSemanticModel::V2 => {
                validated.legacy_package_semantic_hash_v2.clone().ok_or_else(|| {
                    ApiError::unsupported_protocol("isolated worker lacks the v2 package fingerprint reader")
                })?
            }
            super::package::PackageSemanticModel::V3 => {
                validated.legacy_package_semantic_hash_v3.clone().ok_or_else(|| {
                    ApiError::unsupported_protocol("isolated worker lacks the v3 package fingerprint reader")
                })?
            }
            super::package::PackageSemanticModel::V4 => {
                validated.legacy_package_semantic_hash_v4.clone().ok_or_else(|| {
                    ApiError::unsupported_protocol("isolated worker lacks the v4 compatible package reader")
                })?
            }
            super::package::PackageSemanticModel::V5 => validated.to_snapshot.semantic_hash(),
        };
        if hash != object.semantic_hash {
            return Err(ApiError::checksum_mismatch(
                "package semantic fingerprint disagrees with its versioned reader",
            ));
        }
        match validated.package_reference_remap_required {
            Some(false) => {}
            Some(true) => return Err(super::package::inner_reference_refusal()),
            None => {
                return Err(ApiError::unsupported_protocol(
                    "isolated worker lacks package reference validation",
                ));
            }
        }
        let snapshot = STANDARD
            .decode(
                validated
                    .validated_head_snapshot_base64
                    .as_deref()
                    .ok_or_else(|| ApiError::unsupported_protocol("isolated worker lacks package replay support"))?,
            )
            .map_err(|_| ApiError::internal())?;
        super::collab::bootstrap::check_bootstrap_decoded_bytes(u64::try_from(snapshot.len()).unwrap_or(u64::MAX))?;
        result.push(Validated {
            source: object.source_object_id,
            snapshot,
            frontier,
            semantic: validated.to_snapshot,
            title: validated.to_title,
        });
    }
    Ok(result)
}
fn map_ids(mapping: &Value, name: &str) -> Result<BTreeMap<Uuid, Uuid>, ApiError> {
    serde_json::from_value(mapping.field(name).clone())
        .map_err(|_| ApiError::checksum_mismatch("frozen import mapping cannot be decoded"))
}
struct ImportLineage<'a> {
    tx: &'a DatabaseTransaction,
    scope_id: Uuid,
    job: Uuid,
    hash: &'a str,
    source_scope: Option<Uuid>,
    source_workspace: Option<Uuid>,
}
impl ImportLineage<'_> {
    async fn put(
        &self,
        kind: &str,
        source: Uuid,
        source_hash: &str,
        target: Uuid,
        reused: bool,
    ) -> Result<(), ApiError> {
        if reused {
            return Ok(());
        }
        self.tx.execute(Statement::from_sql_and_values(DbBackend::Postgres,"INSERT INTO flow_import_lineage(import_id,source_kind,source_id,source_scope_id,source_workspace_id,target_scope_id,target_workspace_id,source_content_hash,result,package_sha256,target_kind,target_id) VALUES($1,'flow_package',$2,$3,$4,$5,NULL,$6,'created',$7,$8,$9)",vec![self.job.into(),source.into(),self.source_scope.into(),self.source_workspace.into(),self.scope_id.into(),source_hash.into(),self.hash.into(),kind.into(),target.into()])).await?;
        Ok(())
    }
}
pub async fn get(db: &DatabaseConnection, scope_id: Uuid, user: Uuid, job: Uuid) -> Result<Value, ApiError> {
    let proof = owner(db, scope_id, user).await?;
    let row=db.query_one(Statement::from_sql_and_values(DbBackend::Postgres,"SELECT report FROM flow_import_jobs WHERE id=$1 AND scope_id=$2 AND workspace_id IS NULL AND actor_user_id=$3",vec![job.into(),scope_id.into(),user.into()])).await?.ok_or_else(||ApiError::not_found("import job not found"))?;
    if !scope::proof_current(db, &proof).await? {
        return Err(super::policy::authorization_read_unstable());
    }
    row.try_get::<Option<Value>>("", "report")?.ok_or_else(|| {
        ApiError::server_draining(
            crate::error::ServerDrainingReason::Contention,
            1000,
            "import is still in progress",
        )
    })
}
pub async fn commit(
    db: &DatabaseConnection,
    scope_id: Uuid,
    user: Uuid,
    preview: Uuid,
    input: CommitInput,
) -> Result<Value, ApiError> {
    let proof = owner(db, scope_id, user).await?;
    if !input.confirm {
        return Err(ApiError::invalid_update("confirm=true is required"));
    }
    let body = json!({"operation":"import_commit","preview_id":preview,"package_sha256":input.package_sha256,"mapping_hash":input.mapping_hash,"conflict_policy":input.conflict_policy,"confirm":true});
    if let Some(result) = super::scope_commands::replay(db, scope_id, user, &input.idempotency_key, &body).await? {
        return Ok(result);
    }
    require_enabled(db).await?;
    if super::rollback::control(db).await?.import_promotion_paused {
        return Err(ApiError::server_draining(
            crate::error::ServerDrainingReason::Drain,
            5000,
            "package import promotion is paused",
        ));
    }
    let row=db.query_one(Statement::from_sql_and_values(DbBackend::Postgres,"SELECT artifact_id,package_sha256,mapping_hash,mapping,conflicts,expires_at,committed_import_id FROM flow_import_previews WHERE id=$1 AND scope_id=$2 AND workspace_id IS NULL AND actor_kind='user' AND actor_id=$3",vec![preview.into(),scope_id.into(),user.into()])).await?.ok_or_else(||ApiError::not_found("import preview not found"))?;
    let mapping: Value = row.try_get("", "mapping")?;
    if row.try_get::<String>("", "package_sha256")? != input.package_sha256
        || row.try_get::<String>("", "mapping_hash")? != input.mapping_hash
        || shared::sha256_json(&mapping)? != input.mapping_hash
        || *mapping.field("target_scope_id") != json!(scope_id)
        || *mapping.field("conflict_policy") != json!(input.conflict_policy)
    {
        return Err(ApiError::checksum_mismatch("commit does not match the frozen preview"));
    }
    let previous = row.try_get::<Option<Uuid>>("", "committed_import_id")?;
    if let Some(job) = previous {
        let report = get(db, scope_id, user, job).await?;
        let receipt = json!({"job_id":job,"import_id":job,"status":report.field("status")});
        let event = report
            .field("audit_event_id")
            .as_str()
            .and_then(|v| Uuid::parse_str(v).ok())
            .ok_or_else(ApiError::internal)?;
        let tx = db.begin().await?;
        require_enabled(&tx).await?;
        scope::fence(&tx, &proof).await?;
        if let Some(result) =
            super::scope_commands::lock_and_replay(&tx, scope_id, user, &input.idempotency_key, &body).await?
        {
            tx.commit().await?;
            return Ok(result);
        }
        super::scope_commands::record(
            &tx,
            scope_id,
            user,
            &input.idempotency_key,
            body,
            proof.object_id,
            event,
            receipt.clone(),
        )
        .await?;
        tx.commit().await?;
        return Ok(receipt);
    }
    if row.try_get::<Timestamp>("", "expires_at")? <= Timestamp::now() {
        return Err(ApiError::policy_rejected_with_details(
            "import preview expired",
            json!({"reason":"preview_expired"}),
        ));
    }
    let conflicts = row.try_get::<Value>("", "conflicts")?;
    if conflicts.as_array().is_none_or(|v| !v.is_empty()) {
        return Err(ApiError::policy_rejected(
            "resolve import preview conflicts before promotion",
        ));
    }
    let artifact_id = row.try_get::<Uuid>("", "artifact_id")?;
    let (bytes, hash) = artifact(db, scope_id, user, artifact_id).await?;
    if hash != input.package_sha256 {
        return Err(ApiError::checksum_mismatch("import artifact changed"));
    }
    let plan = shared::parse_plan(&bytes, &hash)?;
    known_plan(&plan)?;
    let validated = validate_snapshots(&bytes, &plan).await?;
    let objects = map_ids(&mapping, "object_map")?;
    let documents = map_ids(&mapping, "document_map")?;
    let relations = map_ids(&mapping, "relation_map")?;
    if objects.len() != plan.objects.len()
        || documents.len() != plan.objects.len()
        || objects.values().collect::<BTreeSet<_>>().len() != objects.len()
        || documents.values().collect::<BTreeSet<_>>().len() != documents.len()
    {
        return Err(ApiError::checksum_mismatch("frozen import mapping cardinality changed"));
    }
    let tx = db.begin().await?;
    require_enabled(&tx).await?;
    scope::fence(&tx, &proof).await?;
    if let Some(result) =
        super::scope_commands::lock_and_replay(&tx, scope_id, user, &input.idempotency_key, &body).await?
    {
        tx.commit().await?;
        return Ok(result);
    }
    let frozen = tx
        .query_one(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT committed_import_id,expires_at FROM flow_import_previews WHERE id=$1 AND scope_id=$2 FOR UPDATE",
            vec![preview.into(), scope_id.into()],
        ))
        .await?
        .ok_or_else(ApiError::internal)?;
    if frozen.try_get::<Option<Uuid>>("", "committed_import_id")?.is_some() {
        return Err(ApiError::server_draining(
            crate::error::ServerDrainingReason::Contention,
            1000,
            "another import intent completed this preview; reload its report",
        ));
    }
    if frozen.try_get::<Timestamp>("", "expires_at")? <= Timestamp::now() {
        return Err(ApiError::policy_rejected_with_details(
            "import preview expired",
            json!({"reason":"preview_expired"}),
        ));
    }
    let prior = lineage(&tx, scope_id, &hash).await?;
    let reuse = input.conflict_policy == shared::ConflictPolicy::ReuseImportLineage;
    if !reuse && prior.keys().any(|(kind, _)| kind == "object") {
        return Err(ApiError::policy_rejected("package objects already have import lineage"));
    }
    let job = Uuid::new_v4();
    let now = Timestamp::now();
    let source_scope = plan
        .verified
        .manifest
        .source
        .scope_id
        .as_deref()
        .map(Uuid::parse_str)
        .transpose()
        .map_err(|_| ApiError::unsupported_format("invalid source scope provenance"))?;
    let source_workspace = plan
        .verified
        .manifest
        .source
        .workspace_id
        .as_deref()
        .map(Uuid::parse_str)
        .transpose()
        .map_err(|_| ApiError::unsupported_format("invalid source workspace provenance"))?;
    tx.execute(Statement::from_sql_and_values(DbBackend::Postgres,"INSERT INTO flow_import_jobs(id,scope_id,workspace_id,kind,source_scope_id,source_workspace_id,mapping_hash,package_sha256,artifact_id,conflict_policy,external_reference_policy,request,status,idempotency_key,request_body_hash,actor_user_id,started_at) VALUES($1,$2,NULL,'flow_package',$3,$4,$5,$6,$7,$8,$9,$10,'running',$11,$12,$13,$14)",vec![job.into(),scope_id.into(),source_scope.into(),source_workspace.into(),input.mapping_hash.clone().into(),hash.clone().into(),artifact_id.into(),input.conflict_policy_value().into(),mapping.field("external_reference_policy").as_str().ok_or_else(ApiError::internal)?.into(),body.clone().into(),input.idempotency_key.clone().into(),shared::sha256_json(&body)?.into(),user.into(),now.into()])).await?;
    let mut pending = plan.objects.iter().collect::<Vec<_>>();
    let mut inserted = BTreeSet::new();
    let mut created = 0usize;
    let mut reused = 0usize;
    while !pending.is_empty() {
        let index = pending
            .iter()
            .position(|o| o.parent_object_id.is_none_or(|parent| inserted.contains(&parent)))
            .ok_or_else(|| ApiError::unsupported_format("import parent graph contains a cycle"))?;
        let object = pending.remove(index);
        let target = *objects.get(&object.source_object_id).ok_or_else(ApiError::internal)?;
        let document = *documents
            .get(&object.source_document_id)
            .ok_or_else(ApiError::internal)?;
        let valid = validated
            .iter()
            .find(|v| v.source == object.source_object_id)
            .ok_or_else(ApiError::internal)?;
        let existing = prior.get(&("object".into(), object.source_object_id));
        if reuse && existing.is_some() {
            if existing != Some(&target)
                || prior.get(&("document".into(), object.source_document_id)) != Some(&document)
            {
                return Err(ApiError::checksum_mismatch("frozen lineage mapping changed"));
            }
            let head=tx.query_one(Statement::from_sql_and_values(DbBackend::Postgres,"SELECT d.head_seq,d.head_frontier FROM collab_documents d JOIN flow_objects o ON o.id=d.object_id WHERE d.id=$1 AND o.id=$2 AND o.scope_id=$3 FOR SHARE OF d",vec![document.into(),target.into(),scope_id.into()])).await?.ok_or_else(||ApiError::stale_frontier("import lineage target was removed",None,None))?;
            if head.try_get::<i64>("", "head_seq")? != object.accepted_seq
                || !super::frontier::same(&head.try_get::<Vec<u8>>("", "head_frontier")?, &valid.frontier)?
            {
                return Err(ApiError::stale_frontier("import lineage target changed", None, None));
            }
            reused += 1;
            inserted.insert(object.source_object_id);
            continue;
        }
        let parent = object
            .parent_object_id
            .map(|id| objects.get(&id).copied().ok_or_else(ApiError::internal))
            .transpose()?
            .unwrap_or(proof.object_id);
        let governance = shared::sanitize_governance(&object.governance_metadata)?;
        tx.execute(Statement::from_sql_and_values(DbBackend::Postgres,"INSERT INTO flow_objects(id,scope_id,workspace_id,project_id,object_type,parent_id,governance_metadata,lifecycle_status,created_by,updated_by,archived_at) VALUES($1,$2,NULL,NULL,$3,$4,$5,$6,$7,$7,$8)",vec![target.into(),scope_id.into(),object.object_type.clone().into(),parent.into(),governance.into(),object.lifecycle_status.clone().into(),user.into(),if object.lifecycle_status=="archived"{Some(now)}else{None}.into()])).await?;
        tx.execute(Statement::from_sql_and_values(DbBackend::Postgres,"INSERT INTO collab_documents(id,object_id,engine,format_version,snapshot,snapshot_frontier,snapshot_seq,head_frontier,head_seq,byte_count,update_count) VALUES($1,$2,'loro',$3,$4,$5,$6,$5,$6,$7,0)",vec![document.into(),target.into(),object.format_version.clone().into(),valid.snapshot.clone().into(),valid.frontier.clone().into(),object.accepted_seq.into(),i64::try_from(valid.snapshot.len()).map_err(|_|ApiError::internal())?.into()])).await?;
        super::repository::insert_projection(
            &tx,
            &super::repository::NewProjection {
                object_id: target,
                document_seq: object.accepted_seq,
                document_frontier: valid.frontier.clone(),
                title: valid.title.clone(),
                state: super::projection::state_json(&valid.semantic).map_err(|_| ApiError::internal())?,
                plain_text: super::projection::plain_text(&valid.semantic),
            },
        )
        .await?;
        if object.object_type == "page" {
            super::block_registry::reconcile_accepted(
                &tx,
                target,
                object.accepted_seq,
                &super::block_registry::prepare(&valid.semantic)?,
                super::block_registry::RegistrationAllowance::Complete,
            )
            .await?;
            super::body_assets::reconcile_accepted(
                &tx,
                target,
                object.accepted_seq,
                &super::body_assets::typed_references(&valid.semantic)?,
            )
            .await?;
        }
        ImportLineage {
            tx: &tx,
            scope_id,
            job,
            hash: &hash,
            source_scope,
            source_workspace,
        }
        .put("object", object.source_object_id, &object.semantic_hash, target, false)
        .await?;
        ImportLineage {
            tx: &tx,
            scope_id,
            job,
            hash: &hash,
            source_scope,
            source_workspace,
        }
        .put(
            "document",
            object.source_document_id,
            &object.semantic_hash,
            document,
            false,
        )
        .await?;
        created += 1;
        inserted.insert(object.source_object_id);
    }
    let mut detached = Vec::new();
    for relation in &plan.relations {
        let (Some(source), Some(target)) = (
            objects.get(&relation.source.source_object_id),
            objects.get(&relation.target.source_object_id),
        ) else {
            if mapping.field("external_reference_policy") == "reject" {
                return Err(ApiError::policy_rejected("external reference cannot be promoted"));
            }
            detached.push(relation.source_relation_id);
            continue;
        };
        let id = *relations
            .get(&relation.source_relation_id)
            .ok_or_else(ApiError::internal)?;
        if reuse && prior.contains_key(&("relation".into(), relation.source_relation_id)) {
            continue;
        }
        tx.execute(Statement::from_sql_and_values(DbBackend::Postgres,"INSERT INTO flow_relations(id,scope_id,workspace_id,relation_type,source_object_id,target_object_id,position_key,properties,created_by) VALUES($1,$2,NULL,$3,$4,$5,$6,$7,$8)",vec![id.into(),scope_id.into(),relation.relation_type.clone().into(),(*source).into(),(*target).into(),relation.position_key.clone().into(),relation.properties.clone().into(),user.into()])).await?;
        ImportLineage {
            tx: &tx,
            scope_id,
            job,
            hash: &hash,
            source_scope,
            source_workspace,
        }
        .put(
            "relation",
            relation.source_relation_id,
            &shared::sha256_json(&serde_json::to_value(relation).map_err(|_| ApiError::internal())?)?,
            id,
            false,
        )
        .await?;
    }
    let event = super::scope_event::insert_for_object(
        &tx,
        &scope::identity(&tx, proof.object_id).await?,
        user,
        "flow.import.completed",
        "flow_import",
        job,
        &input.idempotency_key,
        json!({"object_id":proof.object_id,"import_id":job,"created":created,"reused":reused}),
        json!({}),
    )
    .await?;
    let report = json!({"import_id":job,"package_id":plan.verified.manifest.package_id,"package_sha256":hash,"source_scope_id":source_scope,"source_workspace_id":source_workspace,"target_scope_id":scope_id,"target_workspace_id":null,"status":"completed","mapping_hash":input.mapping_hash,"conflict_policy":input.conflict_policy,"counts":{"planned":plan.objects.len(),"created":created,"reused":reused,"relations":plan.relations.len()-detached.len()},"object_mapping":objects,"document_mapping":documents,"detached_references":detached,"warnings":["crdt_snapshot_history_retained","inner_reference_validation_before_promotion"],"started_at":now.to_rfc3339(),"finished_at":Timestamp::now().to_rfc3339(),"actor":user,"audit_event_id":event});
    tx.execute(Statement::from_sql_and_values(DbBackend::Postgres,"UPDATE flow_import_jobs SET status='completed',report=$2,audit_event_id=$3,finished_at=now(),updated_at=now() WHERE id=$1",vec![job.into(),report.into(),event.into()])).await?;
    tx.execute(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "UPDATE flow_import_previews SET committed_import_id=$2 WHERE id=$1",
        vec![preview.into(), job.into()],
    ))
    .await?;
    let receipt = json!({"job_id":job,"import_id":job,"status":"completed"});
    super::scope_commands::record(
        &tx,
        scope_id,
        user,
        &input.idempotency_key,
        body,
        proof.object_id,
        event,
        receipt.clone(),
    )
    .await?;
    if let Err(error) = tx.commit().await {
        tracing::warn!(%error,%job,"scope import commit outcome unknown");
        return Err(ApiError::server_draining(
            crate::error::ServerDrainingReason::Contention,
            1000,
            "retry the same import identity to resolve its outcome",
        ));
    }
    Ok(receipt)
}
impl CommitInput {
    const fn conflict_policy_value(&self) -> &'static str {
        match self.conflict_policy {
            shared::ConflictPolicy::RejectExisting => "reject_existing",
            shared::ConflictPolicy::ReuseImportLineage => "reuse_import_lineage",
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;
    use crate::routes::context::tenant_fixture::{exec, scratch};
    use collab_core::CollabEngine;
    async fn counts(db: &DatabaseConnection) -> BTreeMap<String, i64> {
        let mut values = BTreeMap::new();
        for table in [
            "flow_objects",
            "collab_documents",
            "flow_object_projections",
            "flow_import_jobs",
            "flow_import_lineage",
            "business_events",
            "event_dispatch",
            "flow_scope_command_receipts",
        ] {
            let row = db
                .query_one(Statement::from_string(
                    DbBackend::Postgres,
                    format!("SELECT count(*)::bigint AS n FROM {table}"),
                ))
                .await
                .unwrap()
                .unwrap();
            values.insert(table.into(), row.try_get("", "n").unwrap());
        }
        values
    }
    fn replayed(head: &[u8]) -> collab_core::isolation::wire::ReplayDiffResult {
        let engine = collab_core::LoroCollabEngine::load(head).unwrap();
        collab_core::isolation::wire::ReplayDiffResult {
            semantic_diff: collab_core::SemanticDiff::default(),
            to_snapshot: engine.semantic_snapshot().unwrap(),
            legacy_package_semantic_hash_v4: None,
            legacy_package_semantic_hash_v3: None,
            from_title: String::new(),
            to_title: String::new(),
            package_reference_remap_required: Some(false),
            legacy_package_semantic_hash_v2: None,
            legacy_package_semantic_hash_v1: None,
            validated_head_snapshot_base64: Some(STANDARD.encode(head)),
        }
    }
    /// The replayed head of an uploaded document is refused when it carries a reserved root of the
    /// wrong type, and accepted when it does not.
    #[tokio::test]
    async fn a_replayed_package_head_with_a_shadow_root_is_refused() {
        let clean = include_bytes!("../../../../frontend/tests/fixtures/editor-doc-v1/browser.snapshot").to_vec();
        require_head_without_shadow_roots(&replayed(&clean)).await.unwrap();
        let doc = loro::LoroDoc::new();
        doc.import(&clean).unwrap();
        doc.set_peer_id(9_200_001).unwrap();
        doc.get_text("flow_editor_structure_meta").insert(0, "x").unwrap();
        doc.commit();
        let shadowed = doc.export(loro::ExportMode::Snapshot).unwrap();
        let error = require_head_without_shadow_roots(&replayed(&shadowed))
            .await
            .expect_err("a shadow root must be refused");
        assert_eq!(error.kind(), crate::error::ApiErrorKind::InvalidUpdate);
        assert!(
            format!("{error:?}").contains("reserved_root_container_type"),
            "{error:?}"
        );
        // A head the worker did not export has nothing to check.
        let mut missing = replayed(&clean);
        missing.validated_head_snapshot_base64 = None;
        assert!(require_head_without_shadow_roots(&missing).await.is_err());
    }
    #[tokio::test]
    async fn mocked_verified_replay_scope_import_uses_real_pg_and_rolls_back_every_promotion_row() {
        // This fixture mocks ONLY the verified isolated-replay result. Production has no bypass;
        // codec, frozen mapping, scope/write gates, SQL constraints/transaction and receipts are real.
        let label = format!("scope_import_{}", Uuid::new_v4().simple());
        let scratch = scratch(&label)
            .await
            .expect("explicit disposable OPENPR_TEST_DATABASE_URL is required");
        let db = &scratch.db;

        let user = Uuid::new_v4();
        exec(db,"INSERT INTO users(id,email,password_hash,name,role,is_active) VALUES($1,$2,'','Scope import unit','user',true)",vec![user.into(),format!("{user}@example.test").into()]).await;
        for component in ["api", "worker", "maintenance"] {
            super::super::scope_runtime::renew(db, Uuid::new_v4(), component)
                .await
                .unwrap();
        }
        exec(
            db,
            "UPDATE flow_runtime_capabilities SET personal_provisioning=true,personal_writes=true WHERE singleton",
            vec![],
        )
        .await;
        let (scope, _) = scope::personal_home(db, user).await.unwrap();
        let scope_id = scope.id;
        let path = std::path::PathBuf::from(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/fixtures/flow-v2/legacy-browser-package-v1"
        ));
        let bytes = std::fs::read(path.join("package.zip")).unwrap();
        assert!(
            upload(db, scope_id, user, bytes.clone(), None, "import-upload")
                .await
                .is_err()
        );
        exec(
            db,
            "UPDATE flow_runtime_capabilities SET personal_package_imports=true WHERE singleton",
            vec![],
        )
        .await;
        let artifact = upload(db, scope_id, user, bytes.clone(), None, "import-upload")
            .await
            .unwrap();
        let artifact_id = Uuid::parse_str(artifact.field("artifact_id").as_str().unwrap()).unwrap();
        assert_eq!(
            upload(db, scope_id, user, bytes.clone(), None, "import-upload")
                .await
                .unwrap(),
            artifact
        );
        let preview = preview(
            db,
            scope_id,
            user,
            PreviewInput {
                artifact_id,
                project_mapping: BTreeMap::new(),
                external_reference_policy: shared::ExternalReferencePolicy::Reject,
                conflict_policy: shared::ConflictPolicy::RejectExisting,
                include_history: false,
                idempotency_key: "import-preview".into(),
            },
        )
        .await
        .unwrap();
        assert_eq!(*preview.field("mapping").field("workspace_map"), json!({}));
        assert_eq!(*preview.field("mapping").field("target_scope_id"), json!(scope_id));
        let preview_id = Uuid::parse_str(preview.field("preview_id").as_str().unwrap()).unwrap();
        let input = |key: &str| CommitInput {
            package_sha256: preview.field("package_sha256").as_str().unwrap().into(),
            mapping_hash: preview.field("mapping_hash").as_str().unwrap().into(),
            conflict_policy: shared::ConflictPolicy::RejectExisting,
            confirm: true,
            idempotency_key: key.into(),
        };
        let plan = shared::parse_plan(&bytes, preview.field("package_sha256").as_str().unwrap()).unwrap();
        let object = plan.objects.first().expect("fixture object");
        let snapshot =
            shared::member_bytes(&bytes, &format!("documents/{}/snapshot.bin", object.source_document_id)).unwrap();
        let engine = collab_core::LoroCollabEngine::load(&snapshot).unwrap();
        let proof = collab_core::isolation::wire::ReplayDiffResult {
            semantic_diff: collab_core::SemanticDiff::default(),
            to_snapshot: engine.semantic_snapshot().unwrap(),
            legacy_package_semantic_hash_v4: Some(
                engine.legacy_package_semantic_snapshot_v4().unwrap().semantic_hash(),
            ),
            legacy_package_semantic_hash_v3: Some(
                engine.legacy_package_semantic_snapshot_v3().unwrap().semantic_hash(),
            ),
            from_title: engine.title().unwrap(),
            to_title: engine.title().unwrap(),
            package_reference_remap_required: Some(
                collab_core::isolation::wire::package_reference_remap_required(&engine).unwrap(),
            ),
            legacy_package_semantic_hash_v2: Some(
                engine.legacy_package_semantic_snapshot_v2().unwrap().semantic_hash(),
            ),
            legacy_package_semantic_hash_v1: Some(
                engine.legacy_package_semantic_snapshot_v1().unwrap().semantic_hash(),
            ),
            validated_head_snapshot_base64: Some(STANDARD.encode(engine.export_snapshot().unwrap())),
        };
        let mock = BTreeMap::from([(object.source_object_id, proof)]);
        // Mock ONLY the isolated worker's policy verdict; the rejection must precede every
        // canonical/job/lineage write, including when an old worker lacks this policy capability.
        for verdict in [Some(true), None] {
            let mut blocked = mock.clone();
            for proof in blocked.values_mut() {
                proof.package_reference_remap_required = verdict;
            }
            let before = counts(db).await;
            let rejected = TEST_VERIFIED_REPLAY
                .scope(
                    blocked,
                    commit(db, scope_id, user, preview_id, input("unsupported-inner-reference")),
                )
                .await;
            assert!(rejected.is_err());
            assert_eq!(counts(db).await, before);
        }
        TEST_VERIFIED_REPLAY.scope(mock,async{
   let before=counts(db).await;
   db.execute_unprepared("CREATE FUNCTION scope_import_fault() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN IF NEW.target_kind='document' THEN RAISE EXCEPTION 'unit promotion fault'; END IF; RETURN NEW; END $$; CREATE TRIGGER scope_import_fault BEFORE INSERT ON flow_import_lineage FOR EACH ROW EXECUTE FUNCTION scope_import_fault()").await.unwrap();
   assert!(commit(db,scope_id,user,preview_id,input("import-commit")).await.is_err());assert_eq!(counts(db).await,before);
   db.execute_unprepared("DROP TRIGGER scope_import_fault ON flow_import_lineage; DROP FUNCTION scope_import_fault()").await.unwrap();
   let first=commit(db,scope_id,user,preview_id,input("import-commit")).await.unwrap();let after=counts(db).await;
   assert_eq!(commit(db,scope_id,user,preview_id,input("import-commit")).await.unwrap(),first);assert_eq!(counts(db).await,after);
   let other=commit(db,scope_id,user,preview_id,input("second-completed-intent")).await.unwrap();assert_eq!(other,first);
   assert!(super::super::scope_commands::replay(db,scope_id,user,"second-completed-intent",&json!({"operation":"different"})).await.is_err());
   let job=Uuid::parse_str(first.field("job_id").as_str().unwrap()).unwrap();let report=get(db,scope_id,user,job).await.unwrap();
   assert_eq!(*report.field("target_scope_id"),json!(scope_id));assert!(report.field("target_workspace_id").is_null());assert!(report.field("source_scope_id").is_null());assert_eq!(report.field("counts").field("created"),1);
   let imported=report.field("object_mapping").as_object().unwrap().values().next().unwrap().as_str().unwrap();let view=scope::object_view(db,Uuid::parse_str(imported).unwrap(),user).await.unwrap();
   assert_eq!(*view.field("scope_id"),json!(scope_id));assert!(view.get("workspace_id").is_none());assert!(view.field("semantic_content").to_string().contains("Browser 😀 prose"));
   for table in ["workspaces","workspace_members"]{let row=db.query_one(Statement::from_string(DbBackend::Postgres,format!("SELECT count(*)::bigint AS n FROM {table}"))).await.unwrap().unwrap();assert_eq!(row.try_get::<i64>("","n").unwrap(),0);}
  }).await;
        scratch.drop_self().await;
    }
}
