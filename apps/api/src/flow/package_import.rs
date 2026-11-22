//! Isolated upload, frozen preview, and atomic promotion for Flow package v1 imports.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::io::{Cursor, Read};

use collab_core::{CollabEngine, LoroCollabEngine};
use platform::timestamp::Timestamp;
use sea_orm::{ConnectionTrait, DatabaseConnection, DbBackend, FromQueryResult, Statement, TransactionTrait};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use time::Duration;
use uuid::Uuid;

use crate::error::{ApiError, ServerDrainingReason};
use crate::events::{BusinessEventInput, FlowDispatchSpec, insert_flow_event};
use crate::flow::IN_FLIGHT_RETRY_AFTER_MS;

use super::package::{ExportPackageManifest, PackageEngine, VerifiedPackage, verify_package};
use super::{import::BoundedImportStager, projection, repository};

const IMPORT_ARTIFACT_TTL_MINUTES: i64 = 30;

#[cfg(test)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PromotionFaultPoint {
    JobStarted,
    ObjectPromoted,
    RelationsPromoted,
    EventInserted,
    ReportFinished,
    PreviewLinked,
}

#[cfg(test)]
static PROMOTION_FAULT: parking_lot::Mutex<Option<(Uuid, PromotionFaultPoint)>> = parking_lot::Mutex::new(None);

#[cfg(test)]
fn fail_promotion_at(preview_id: Uuid, point: PromotionFaultPoint) -> Result<(), ApiError> {
    if PROMOTION_FAULT.lock().as_ref() == Some(&(preview_id, point)) {
        return Err(ApiError::Internal);
    }
    Ok(())
}

#[derive(Debug, Clone)]
pub struct ImportPrincipal {
    pub id: Uuid,
    pub kind: String,
    pub role: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExternalReferencePolicy {
    Reject,
    Detach,
}

impl ExternalReferencePolicy {
    const fn as_str(self) -> &'static str {
        match self {
            Self::Reject => "reject",
            Self::Detach => "detach",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConflictPolicy {
    RejectExisting,
    ReuseImportLineage,
}

impl ConflictPolicy {
    const fn as_str(self) -> &'static str {
        match self {
            Self::RejectExisting => "reject_existing",
            Self::ReuseImportLineage => "reuse_import_lineage",
        }
    }
}

#[derive(Debug, Clone)]
pub struct PreviewImportRequest {
    pub workspace_id: Uuid,
    pub artifact_id: Uuid,
    pub project_map: BTreeMap<Uuid, Option<Uuid>>,
    pub external_reference_policy: ExternalReferencePolicy,
    pub conflict_policy: ConflictPolicy,
    pub include_history: bool,
    pub idempotency_key: String,
    pub principal: ImportPrincipal,
}

#[derive(Debug, Clone)]
pub struct CommitImportRequest {
    pub workspace_id: Uuid,
    pub preview_id: Uuid,
    pub package_sha256: String,
    pub mapping_hash: String,
    pub conflict_policy: ConflictPolicy,
    pub confirm: bool,
    pub idempotency_key: String,
    pub principal: ImportPrincipal,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct FrozenImportMapping {
    pub workspace_map: BTreeMap<Uuid, Uuid>,
    pub object_map: BTreeMap<Uuid, Uuid>,
    pub document_map: BTreeMap<Uuid, Uuid>,
    pub relation_map: BTreeMap<Uuid, Uuid>,
    pub project_map: BTreeMap<Uuid, Option<Uuid>>,
    pub external_reference_policy: ExternalReferencePolicy,
    pub conflict_policy: ConflictPolicy,
}

/// One source project referenced by the package's objects. `title` comes from the package's
/// optional `projects` manifest field and is `null` for packages exported without it.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ManifestProjectSummary {
    pub source_project_id: Uuid,
    pub title: Option<String>,
}

/// `rest-api-v1.md` v0.8 `imports/preview` `manifest_summary`:
/// `{projects:[{source_project_id,title}],object_count,document_count,package_schema,engine}`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ManifestSummary {
    pub projects: Vec<ManifestProjectSummary>,
    pub object_count: u64,
    pub document_count: u64,
    pub package_schema: String,
    pub engine: PackageEngine,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<super::package::PackageSource>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub history: Option<Value>,
}

/// `invalid_update.details.reason` when a commit's frozen `project_map` leaves a package source
/// project unmapped (`error-mapping-v1.md` approved reason table).
pub const PROJECT_UNMAPPED_REASON: &str = "project_unmapped";

/// The `project_map` literal that places a source project's objects outside any project.
///
/// `export-package-v1.md`: `source_project_id -> target_project_id | unprojected`. JSON `null`
/// means the same.
pub const UNPROJECTED_TARGET: &str = "unprojected";

/// One structured preview conflict. `conflicts` keeps listing the bare subject ids for existing
/// consumers; `conflict_details` carries the kind and the facts a caller needs to resolve it.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ImportConflict {
    /// The target workspace already holds an import of this package object
    /// (`(package_sha256, source_object_id)` lineage); `existing_target_id` is that object.
    LineageExists {
        source_object_id: Uuid,
        existing_target_id: Uuid,
    },
    /// A package source project with no entry in `project_map`; commit refuses it with
    /// `invalid_update{reason:"project_unmapped"}` until it is mapped to a project or `unprojected`.
    ProjectUnmapped {
        source_project_id: Uuid,
        title: Option<String>,
    },
}

/// Accepts `project_mapping` values as a target project id, `null`, or the contract literal
/// `"unprojected"`; the last two both freeze to "no project".
pub fn deserialize_project_mapping<'de, D>(deserializer: D) -> Result<BTreeMap<Uuid, Option<Uuid>>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let raw = BTreeMap::<Uuid, Option<String>>::deserialize(deserializer)?;
    raw.into_iter()
        .map(|(source, target)| match target.as_deref() {
            None | Some(UNPROJECTED_TARGET) => Ok((source, None)),
            Some(value) => Uuid::parse_str(value)
                .map(|target| (source, Some(target)))
                .map_err(|_| {
                    serde::de::Error::custom("project_mapping values must be a project id or \"unprojected\"")
                }),
        })
        .collect()
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ImportPreviewReceipt {
    pub preview_id: Uuid,
    pub package_id: Uuid,
    pub package_sha256: String,
    pub mapping_hash: String,
    pub manifest_summary: ManifestSummary,
    pub mapping: FrozenImportMapping,
    pub conflicts: Vec<String>,
    pub conflict_details: Vec<ImportConflict>,
    pub warnings: Vec<String>,
    pub estimated_changes: BTreeMap<String, u64>,
    pub expires_at: Timestamp,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ImportReport {
    pub import_id: Uuid,
    pub package_id: Uuid,
    pub package_sha256: String,
    pub source_workspace_id: Uuid,
    pub target_workspace_id: Uuid,
    pub status: String,
    pub mapping_hash: String,
    pub conflict_policy: String,
    pub counts: BTreeMap<String, u64>,
    pub object_mapping: BTreeMap<Uuid, Uuid>,
    pub document_mapping: BTreeMap<Uuid, Uuid>,
    pub detached_references: Vec<Uuid>,
    pub warnings: Vec<String>,
    pub started_at: Timestamp,
    pub finished_at: Timestamp,
    pub actor: Uuid,
    pub audit_event_id: Uuid,
}

#[derive(Debug, Deserialize)]
pub(crate) struct PackageObject {
    pub(crate) source_object_id: Uuid,
    #[serde(default)]
    pub(crate) source_workspace_id: Option<Uuid>,
    #[serde(default)]
    pub(crate) source_scope_id: Option<Uuid>,
    pub(crate) object_type: String,
    pub(crate) lifecycle_status: String,
    pub(crate) project_id: Option<Uuid>,
    pub(crate) parent_object_id: Option<Uuid>,
    pub(crate) governance_metadata: Value,
    pub(crate) source_document_id: Uuid,
    pub(crate) engine: String,
    pub(crate) format_version: String,
    pub(crate) accepted_seq: i64,
    pub(crate) accepted_frontier: String,
    pub(crate) semantic_hash: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub(crate) struct RelationEndpoint {
    pub(crate) kind: String,
    pub(crate) source_object_id: Uuid,
}

#[derive(Debug, Serialize, Deserialize)]
pub(crate) struct PackageRelation {
    pub(crate) source_relation_id: Uuid,
    pub(crate) relation_type: String,
    pub(crate) source: RelationEndpoint,
    pub(crate) target: RelationEndpoint,
    pub(crate) position_key: String,
    pub(crate) properties: Value,
}

pub(crate) struct PackagePlan {
    pub(crate) verified: VerifiedPackage,
    pub(crate) objects: Vec<PackageObject>,
    pub(crate) relations: Vec<PackageRelation>,
}

#[derive(Debug, FromQueryResult)]
struct ArtifactRow {
    actor_kind: String,
    actor_id: Uuid,
    package_sha256: String,
    package_bytes: Vec<u8>,
    expires_at: Timestamp,
}

#[derive(Debug, FromQueryResult)]
struct PreviewRow {
    artifact_id: Uuid,
    actor_kind: String,
    actor_id: Uuid,
    package_sha256: String,
    mapping_hash: String,
    mapping: Value,
    request: Value,
    expires_at: Timestamp,
    committed_import_id: Option<Uuid>,
}

#[derive(Debug, FromQueryResult)]
struct ExistingPreviewRow {
    id: Uuid,
    package_sha256: String,
    mapping_hash: String,
    mapping: Value,
    conflicts: Value,
    warnings: Value,
    estimated_changes: Value,
    request_hash: String,
    expires_at: Timestamp,
}

#[derive(Debug, FromQueryResult)]
struct ExistingArtifactRow {
    id: Uuid,
    package_sha256: String,
    request_hash: Option<String>,
    expires_at: Timestamp,
}

pub async fn upload_package_artifact(
    db: &DatabaseConnection,
    workspace_id: Uuid,
    principal: &ImportPrincipal,
    bytes: Vec<u8>,
    expected_package_sha256: Option<&str>,
    idempotency_key: &str,
) -> Result<(Uuid, String, Timestamp), ApiError> {
    enforce_admin(principal)?;
    validate_key(idempotency_key)?;
    let verified = verify_package(Cursor::new(&bytes), expected_package_sha256)?;
    let request_hash = verified.package_sha256.clone();
    if let Some(existing) = ExistingArtifactRow::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "SELECT id,package_sha256,request_hash,expires_at FROM flow_package_artifacts \
         WHERE workspace_id=$1 AND actor_kind=$2 AND actor_id=$3 AND purpose='import' AND idempotency_key=$4",
        vec![
            workspace_id.into(),
            principal.kind.clone().into(),
            principal.id.into(),
            idempotency_key.into(),
        ],
    ))
    .one(db)
    .await?
    {
        if existing.request_hash.as_deref() != Some(request_hash.as_str()) {
            return Err(ApiError::idempotency_key_body_drift(
                "artifact idempotency key body drift",
            ));
        }
        return Ok((existing.id, existing.package_sha256, existing.expires_at));
    }
    let id = Uuid::new_v4();
    let expires_at = Timestamp::now() + Duration::minutes(IMPORT_ARTIFACT_TTL_MINUTES);
    let size = i64::try_from(bytes.len()).map_err(|_| ApiError::internal())?;
    db.execute(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "INSERT INTO flow_package_artifacts \
         (id,workspace_id,actor_kind,actor_id,purpose,package_sha256,idempotency_key,request_hash,size_bytes,package_bytes,expires_at) \
         VALUES ($1,$2,$3,$4,'import',$5,$6,$7,$8,$9,$10)",
        vec![
            id.into(),
            workspace_id.into(),
            principal.kind.clone().into(),
            principal.id.into(),
            verified.package_sha256.clone().into(),
            idempotency_key.into(),
            request_hash.into(),
            size.into(),
            bytes.into(),
            expires_at.into(),
        ],
    ))
    .await?;
    Ok((id, verified.package_sha256, expires_at))
}

pub async fn upload_inline_base64_artifact(
    db: &DatabaseConnection,
    workspace_id: Uuid,
    principal: &ImportPrincipal,
    encoded: &[u8],
    expected_package_sha256: Option<&str>,
    idempotency_key: &str,
) -> Result<(Uuid, String, Timestamp), ApiError> {
    let mut stager = BoundedImportStager::new(Vec::new(), std::io::sink());
    stager.stage_inline_base64_archive(Cursor::new(encoded))?;
    stager.finish_archive()?;
    let (bytes, _) = stager.into_stages()?;
    upload_package_artifact(
        db,
        workspace_id,
        principal,
        bytes,
        expected_package_sha256,
        idempotency_key,
    )
    .await
}

pub async fn get_import_report(
    db: &DatabaseConnection,
    workspace_id: Uuid,
    import_id: Uuid,
    principal: &ImportPrincipal,
) -> Result<ImportReport, ApiError> {
    enforce_admin(principal)?;
    let job = repository::fetch_import_job(db, workspace_id, import_id)
        .await?
        .ok_or_else(|| ApiError::not_found("import not found".to_string()))?;
    job.report
        .ok_or_else(|| {
            ApiError::server_draining(
                ServerDrainingReason::Contention,
                IN_FLIGHT_RETRY_AFTER_MS,
                "import has not completed",
            )
        })
        .and_then(|value| serde_json::from_value(value).map_err(|_| ApiError::internal()))
}

pub async fn preview_package_import(
    db: &DatabaseConnection,
    request: &PreviewImportRequest,
) -> Result<ImportPreviewReceipt, ApiError> {
    validate_key(&request.idempotency_key)?;
    enforce_admin(&request.principal)?;
    let artifact = load_artifact(db, request.workspace_id, request.artifact_id, &request.principal).await?;
    let plan = parse_plan(&artifact.package_bytes, &artifact.package_sha256)?;
    if request.include_history != plan.verified.manifest.history.included {
        return Err(ApiError::invalid_update(
            "include_history must match the uploaded package manifest".to_string(),
        ));
    }
    validate_project_targets(db, request.workspace_id, &request.project_map).await?;
    let request_json = json!({
        "artifact_id": request.artifact_id,
        "project_map": request.project_map,
        "external_reference_policy": request.external_reference_policy,
        "conflict_policy": request.conflict_policy,
        "include_history": request.include_history,
    });
    let request_hash = sha256_json(&request_json)?;
    if let Some(existing) = existing_preview(db, request, &request_hash).await? {
        return preview_receipt(existing, &plan);
    }
    let prior_targets = lineage_targets(db, request.workspace_id, &plan).await?;
    let lineage_conflicts: Vec<String> = prior_targets
        .keys()
        .filter(|(kind, _)| kind == "object")
        .map(|(_, source_id)| *source_id)
        .collect::<BTreeSet<_>>()
        .into_iter()
        .map(|source_id| source_id.to_string())
        .collect();
    let mut conflicts = lineage_conflicts;
    conflicts.extend(
        unmapped_projects(&request.project_map, &plan)
            .into_iter()
            .map(|id| id.to_string()),
    );
    let mapping = freeze_mapping(request, &plan, &prior_targets)?;
    let mapping_json = serde_json::to_value(&mapping).map_err(|_| ApiError::internal())?;
    let mapping_hash = sha256_json(&mapping_json)?;
    let preview_id = Uuid::new_v4();
    let expires_at = Timestamp::now() + Duration::minutes(IMPORT_ARTIFACT_TTL_MINUTES);
    let package_id = parse_manifest_uuid(&plan.verified.manifest)?;
    let estimated_changes = estimated_changes(&plan);
    let summary = manifest_summary(&plan);
    let summary_json = serde_json::to_value(&summary).map_err(|_| ApiError::internal())?;
    let details = conflict_details(&conflicts, &mapping, &summary)?;
    let tx = db.begin().await?;
    lock_authorization_epoch(&tx, request.workspace_id).await?;
    tx.execute(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "INSERT INTO flow_import_previews \
         (id,workspace_id,artifact_id,actor_kind,actor_id,package_sha256,mapping_hash,mapping,request,manifest_summary,conflicts,warnings,estimated_changes,idempotency_key,request_hash,expires_at) \
         VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,'[\"inner_reference_validation_before_promotion\"]'::jsonb,$12,$13,$14,$15)",
        vec![
            preview_id.into(), request.workspace_id.into(), request.artifact_id.into(),
            request.principal.kind.clone().into(), request.principal.id.into(), artifact.package_sha256.clone().into(),
            mapping_hash.clone().into(), mapping_json.into(), request_json.clone().into(),
            summary_json.into(), json!(conflicts).into(), json!(estimated_changes).into(),
            request.idempotency_key.clone().into(), request_hash.into(), expires_at.into(),
        ],
    )).await?;
    insert_flow_event(
        &tx,
        import_event(
            request.workspace_id,
            preview_id,
            &request.principal,
            "flow.import.previewed",
            None,
        ),
        None,
    )
    .await?;
    tx.commit().await?;
    Ok(ImportPreviewReceipt {
        preview_id,
        package_id,
        package_sha256: artifact.package_sha256,
        mapping_hash,
        manifest_summary: summary,
        mapping,
        conflicts,
        conflict_details: details,
        warnings: vec!["inner_reference_validation_before_promotion".into()],
        estimated_changes,
        expires_at,
    })
}

pub async fn commit_package_import(
    db: &DatabaseConnection,
    request: &CommitImportRequest,
) -> Result<ImportReport, ApiError> {
    if !request.confirm {
        return Err(ApiError::invalid_update("confirm=true is required".to_string()));
    }
    validate_key(&request.idempotency_key)?;
    enforce_admin(&request.principal)?;
    if let Some(existing) = repository::find_import_job_by_idempotency_key(
        db,
        request.workspace_id,
        "flow_package",
        &request.idempotency_key,
    )
    .await?
    {
        let expected = commit_request_hash(request)?;
        if existing.request_body_hash != expected {
            return Err(ApiError::idempotency_key_body_drift(
                "import idempotency key body drift",
            ));
        }
        return existing
            .report
            .ok_or_else(|| {
                ApiError::server_draining(
                    ServerDrainingReason::Contention,
                    IN_FLIGHT_RETRY_AFTER_MS,
                    "import job has not completed",
                )
            })
            .and_then(|value| serde_json::from_value(value).map_err(|_| ApiError::internal()));
    }

    if super::rollback::control(db).await?.import_promotion_paused {
        return Err(ApiError::server_draining(
            crate::error::ServerDrainingReason::Drain,
            5_000,
            "package import promotion is paused for application rollback",
        ));
    }

    // Everything that decodes, replays or projects the uploaded documents happens here, in the
    // isolated worker and outside of any transaction: the transaction below only stores what the
    // worker validated (see `import_replay`).
    let prepared = prepare_documents(db, request).await?;

    let tx = db.begin().await?;
    lock_authorization_epoch(&tx, request.workspace_id).await?;
    let preview = load_preview(&tx, request).await?;
    if let Some(import_id) = preview.committed_import_id {
        let job = repository::fetch_import_job(&tx, request.workspace_id, import_id)
            .await?
            .ok_or_else(ApiError::internal)?;
        tx.rollback().await?;
        return job
            .report
            .ok_or_else(|| {
                ApiError::server_draining(
                    ServerDrainingReason::Contention,
                    IN_FLIGHT_RETRY_AFTER_MS,
                    "import job has not completed",
                )
            })
            .and_then(|value| serde_json::from_value(value).map_err(|_| ApiError::internal()));
    }
    let mapping: FrozenImportMapping = serde_json::from_value(preview.mapping.clone())
        .map_err(|_| ApiError::checksum_mismatch("frozen import mapping cannot be decoded"))?;
    let (plan, validated) = prepared.ok_or_else(ApiError::internal)?;
    // The artifact is addressed by its digest: the package the documents were validated from is
    // the package this transaction promotes, or the import is refused.
    confirm_artifact_tx(
        &tx,
        request.workspace_id,
        preview.artifact_id,
        &request.principal,
        &request.package_sha256,
    )
    .await?;
    validate_frozen_mapping(&mapping, &plan, request.workspace_id, request.conflict_policy)?;
    // Re-read lineage inside the promotion transaction: preview only reports conflicts, and an
    // import committed after the preview must not be silently skipped or duplicated either.
    if request.conflict_policy == ConflictPolicy::RejectExisting
        && lineage_targets(&tx, request.workspace_id, &plan)
            .await?
            .keys()
            .any(|(kind, _)| kind == "object")
    {
        return Err(ApiError::policy_rejected(
            "conflict_policy reject_existing refuses package objects that already have import lineage",
        ));
    }
    validate_projects(&tx, request.workspace_id, &mapping.project_map, &plan).await?;

    let import_id = Uuid::new_v4();
    let started_at = Timestamp::now();
    let request_json = json!({
        "preview_id": request.preview_id,
        "package_sha256": request.package_sha256,
        "mapping_hash": request.mapping_hash,
        "conflict_policy": request.conflict_policy,
        "confirm": true,
    });
    repository::insert_import_job(
        &tx,
        &repository::NewImportJob {
            id: import_id,
            workspace_id: request.workspace_id,
            kind: "flow_package",
            source_workspace_id: Some(parse_source_workspace(&plan.verified.manifest)?),
            mapping_hash: &request.mapping_hash,
            package_sha256: Some(&request.package_sha256),
            artifact_id: Some(preview.artifact_id),
            conflict_policy: Some(request.conflict_policy.as_str()),
            external_reference_policy: Some(mapping.external_reference_policy.as_str()),
            request: request_json,
            idempotency_key: &request.idempotency_key,
            request_body_hash: &commit_request_hash(request)?,
            actor_user_id: actor_user_id(&request.principal),
        },
    )
    .await?;
    repository::start_import_job(&tx, import_id).await?;
    #[cfg(test)]
    fail_promotion_at(request.preview_id, PromotionFaultPoint::JobStarted)?;

    let (created, reused) = promote_objects(&tx, &validated, &plan, &mapping, request, import_id).await?;
    let detached = promote_relations(&tx, &plan, &mapping, request, import_id).await?;
    #[cfg(test)]
    fail_promotion_at(request.preview_id, PromotionFaultPoint::RelationsPromoted)?;
    let event = insert_flow_event(
        &tx,
        import_event(
            request.workspace_id,
            import_id,
            &request.principal,
            "flow.import.completed",
            Some(json!({
                "import_id": import_id, "package_id": plan.verified.manifest.package_id,
                "package_sha256": request.package_sha256, "created": created, "reused": reused,
            })),
        ),
        Some(FlowDispatchSpec {
            max_attempts: crate::config::runtime().flow.dispatch_max_attempts,
            document_id: None,
            accepted_seq: None,
        }),
    )
    .await?;
    #[cfg(test)]
    fail_promotion_at(request.preview_id, PromotionFaultPoint::EventInserted)?;
    let finished_at = Timestamp::now();
    let report = ImportReport {
        import_id,
        package_id: parse_manifest_uuid(&plan.verified.manifest)?,
        package_sha256: request.package_sha256.clone(),
        source_workspace_id: parse_source_workspace(&plan.verified.manifest)?,
        target_workspace_id: request.workspace_id,
        status: "completed".to_string(),
        mapping_hash: request.mapping_hash.clone(),
        conflict_policy: request.conflict_policy.as_str().to_string(),
        counts: BTreeMap::from([
            (
                "planned".to_string(),
                u64::try_from(plan.objects.len()).unwrap_or(u64::MAX),
            ),
            ("created".to_string(), created),
            ("reused".to_string(), reused),
            (
                "detached".to_string(),
                u64::try_from(detached.len()).unwrap_or(u64::MAX),
            ),
            ("failed".to_string(), 0),
        ]),
        object_mapping: mapping.object_map.clone(),
        document_mapping: mapping.document_map.clone(),
        detached_references: detached,
        warnings: Vec::new(),
        started_at,
        finished_at,
        actor: request.principal.id,
        audit_event_id: event.event_id,
    };
    let report_json = serde_json::to_value(&report).map_err(|_| ApiError::internal())?;
    repository::finish_import_job(
        &tx,
        import_id,
        "completed",
        Some(report_json),
        None,
        Some(event.event_id),
    )
    .await?;
    #[cfg(test)]
    fail_promotion_at(request.preview_id, PromotionFaultPoint::ReportFinished)?;
    tx.execute(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "UPDATE flow_import_previews SET committed_import_id=$2 WHERE id=$1 AND committed_import_id IS NULL",
        vec![request.preview_id.into(), import_id.into()],
    ))
    .await?;
    #[cfg(test)]
    fail_promotion_at(request.preview_id, PromotionFaultPoint::PreviewLinked)?;
    tx.commit().await?;
    Ok(report)
}

/// The documents of the package of a commit, validated in the isolated worker, with its plan.
/// `None` for a preview that an earlier commit already completed: the transaction answers it with
/// the stored report and validates nothing again.
async fn prepare_documents(
    db: &DatabaseConnection,
    request: &CommitImportRequest,
) -> Result<Option<(PackagePlan, Vec<super::import_replay::ValidatedDocument>)>, ApiError> {
    let preview = load_preview(db, request).await?;
    if preview.committed_import_id.is_some() {
        return Ok(None);
    }
    let artifact = load_artifact(db, request.workspace_id, preview.artifact_id, &request.principal).await?;
    let plan = parse_plan(&artifact.package_bytes, &request.package_sha256)?;
    let validated = super::import_replay::validate_documents(&artifact.package_bytes, &plan).await?;
    Ok(Some((plan, validated)))
}

/// Re-reads, inside the promotion transaction, the artifact the documents were validated from and
/// refuses the import unless it is still the caller's, unexpired and of the committed digest. The
/// package bytes are not read again.
async fn confirm_artifact_tx<C: ConnectionTrait>(
    conn: &C,
    workspace_id: Uuid,
    artifact_id: Uuid,
    principal: &ImportPrincipal,
    package_sha256: &str,
) -> Result<(), ApiError> {
    let row = conn
        .query_one(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT actor_kind,actor_id,package_sha256,expires_at \
             FROM flow_package_artifacts WHERE id=$1 AND workspace_id=$2 AND purpose='import' FOR SHARE",
            vec![artifact_id.into(), workspace_id.into()],
        ))
        .await?
        .ok_or_else(|| ApiError::not_found("import artifact not found".to_string()))?;
    if row.try_get::<String>("", "actor_kind")? != principal.kind
        || row.try_get::<Uuid>("", "actor_id")? != principal.id
        || row.try_get::<Timestamp>("", "expires_at")? <= Timestamp::now()
    {
        return Err(ApiError::not_found("import artifact not found".to_string()));
    }
    if row.try_get::<String>("", "package_sha256")? != package_sha256 {
        return Err(ApiError::checksum_mismatch("import artifact changed"));
    }
    Ok(())
}

fn enforce_admin(principal: &ImportPrincipal) -> Result<(), ApiError> {
    if !matches!(principal.kind.as_str(), "user" | "bot") || !matches!(principal.role.as_str(), "owner" | "admin") {
        return Err(ApiError::forbidden(
            "package import requires workspace admin".to_string(),
        ));
    }
    Ok(())
}

fn actor_user_id(principal: &ImportPrincipal) -> Option<Uuid> {
    (principal.kind == "user").then_some(principal.id)
}

fn validate_key(key: &str) -> Result<(), ApiError> {
    if key.trim().is_empty() || key.len() > 255 {
        return Err(ApiError::invalid_update(
            "idempotency_key must contain 1-255 characters".to_string(),
        ));
    }
    Ok(())
}

async fn lock_authorization_epoch<C: ConnectionTrait>(conn: &C, workspace_id: Uuid) -> Result<(), ApiError> {
    let row = conn
        .query_one(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT authz_epoch FROM flow_workspace_settings WHERE workspace_id=$1 AND flow_enabled FOR SHARE",
            vec![workspace_id.into()],
        ))
        .await?;
    if row.is_none() {
        return Err(ApiError::feature_disabled("Flow is disabled for this workspace"));
    }
    Ok(())
}

async fn load_artifact(
    db: &DatabaseConnection,
    workspace_id: Uuid,
    artifact_id: Uuid,
    principal: &ImportPrincipal,
) -> Result<ArtifactRow, ApiError> {
    load_artifact_tx(db, workspace_id, artifact_id, principal).await
}

async fn load_artifact_tx<C: ConnectionTrait>(
    conn: &C,
    workspace_id: Uuid,
    artifact_id: Uuid,
    principal: &ImportPrincipal,
) -> Result<ArtifactRow, ApiError> {
    let row = ArtifactRow::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "SELECT actor_kind,actor_id,package_sha256,package_bytes,expires_at \
         FROM flow_package_artifacts WHERE id=$1 AND workspace_id=$2 AND purpose='import'",
        vec![artifact_id.into(), workspace_id.into()],
    ))
    .one(conn)
    .await?
    .ok_or_else(|| ApiError::not_found("import artifact not found".to_string()))?;
    if row.actor_kind != principal.kind || row.actor_id != principal.id || row.expires_at <= Timestamp::now() {
        return Err(ApiError::not_found("import artifact not found".to_string()));
    }
    Ok(row)
}

async fn load_preview<C: ConnectionTrait>(conn: &C, request: &CommitImportRequest) -> Result<PreviewRow, ApiError> {
    let row = PreviewRow::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "SELECT artifact_id,actor_kind,actor_id,package_sha256,mapping_hash,mapping,request,expires_at,committed_import_id \
         FROM flow_import_previews WHERE id=$1 AND workspace_id=$2 FOR UPDATE",
        vec![request.preview_id.into(), request.workspace_id.into()],
    )).one(conn).await?.ok_or_else(|| ApiError::not_found("import preview not found".to_string()))?;
    if row.actor_kind != request.principal.kind
        || row.actor_id != request.principal.id
        || row.expires_at <= Timestamp::now()
    {
        return Err(ApiError::not_found("import preview not found".to_string()));
    }
    if row.package_sha256 != request.package_sha256 || row.mapping_hash != request.mapping_hash {
        return Err(ApiError::checksum_mismatch("commit does not match the frozen preview"));
    }
    if row.request.get("conflict_policy") != Some(&json!(request.conflict_policy)) {
        return Err(ApiError::invalid_update("commit conflict policy differs from preview"));
    }
    Ok(row)
}

async fn existing_preview(
    db: &DatabaseConnection,
    request: &PreviewImportRequest,
    request_hash: &str,
) -> Result<Option<ExistingPreviewRow>, ApiError> {
    let row = ExistingPreviewRow::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "SELECT id,package_sha256,mapping_hash,mapping,conflicts,warnings,estimated_changes,request_hash,expires_at \
         FROM flow_import_previews WHERE workspace_id=$1 AND actor_kind=$2 AND actor_id=$3 AND idempotency_key=$4",
        vec![
            request.workspace_id.into(),
            request.principal.kind.clone().into(),
            request.principal.id.into(),
            request.idempotency_key.clone().into(),
        ],
    ))
    .one(db)
    .await?;
    if row
        .as_ref()
        .is_some_and(|existing| existing.request_hash != request_hash)
    {
        return Err(ApiError::idempotency_key_body_drift(
            "preview idempotency key body drift",
        ));
    }
    Ok(row)
}

fn preview_receipt(row: ExistingPreviewRow, plan: &PackagePlan) -> Result<ImportPreviewReceipt, ApiError> {
    let summary = manifest_summary(plan);
    let conflicts: Vec<String> = serde_json::from_value(row.conflicts).map_err(|_| ApiError::internal())?;
    let mapping: FrozenImportMapping = serde_json::from_value(row.mapping).map_err(|_| ApiError::internal())?;
    let details = conflict_details(&conflicts, &mapping, &summary)?;
    Ok(ImportPreviewReceipt {
        preview_id: row.id,
        package_id: parse_manifest_uuid(&plan.verified.manifest)?,
        package_sha256: row.package_sha256,
        mapping_hash: row.mapping_hash,
        manifest_summary: summary,
        mapping,
        conflicts,
        conflict_details: details,
        warnings: serde_json::from_value(row.warnings).map_err(|_| ApiError::internal())?,
        estimated_changes: serde_json::from_value(row.estimated_changes).map_err(|_| ApiError::internal())?,
        expires_at: row.expires_at,
    })
}

pub(crate) fn parse_plan(bytes: &[u8], expected_hash: &str) -> Result<PackagePlan, ApiError> {
    let verified = verify_package(Cursor::new(bytes), Some(expected_hash))?;
    if verified.manifest.schema == super::package::PORTABLE_PAGE_SCHEMA {
        return Err(ApiError::unsupported_format(
            "portable Page packages require a Page destination and the portable import command",
        ));
    }
    let mut objects = Vec::new();
    for member in &verified.manifest.members {
        if member.kind == "object" {
            let object: PackageObject = serde_json::from_slice(&member_bytes(bytes, &member.path)?)
                .map_err(|_| ApiError::unsupported_format("object metadata is invalid"))?;
            let expected_path = format!("objects/{}/object.json", object.source_object_id);
            if member.path != expected_path
                || object.source_workspace_id.map(|id| id.to_string()) != verified.manifest.source.workspace_id
            {
                return Err(ApiError::unsupported_format(
                    "object metadata identity disagrees with its manifest path",
                ));
            }
            if verified.manifest.schema == super::package::SCOPE_PACKAGE_SCHEMA
                && object.source_scope_id.map(|id| id.to_string()) != verified.manifest.source.scope_id
            {
                return Err(ApiError::unsupported_format(
                    "object scope provenance disagrees with its manifest",
                ));
            }
            if matches!(object.object_type.as_str(), "collection" | "record") {
                return Err(super::package::typed_raw_refusal(&[object.object_type]));
            }
            objects.push(object);
        }
    }
    let relations: Vec<PackageRelation> = parse_jsonl(&member_bytes(bytes, "relations/relations.jsonl")?)?;
    if objects.len() != usize::try_from(verified.manifest.counts.objects).unwrap_or(usize::MAX)
        || relations.len() != usize::try_from(verified.manifest.counts.relations).unwrap_or(usize::MAX)
    {
        return Err(ApiError::checksum_mismatch(
            "package semantic counts disagree with member content",
        ));
    }
    validate_plan_graph(&objects, &relations)?;
    Ok(PackagePlan {
        verified,
        objects,
        relations,
    })
}

fn parse_jsonl<T: for<'de> Deserialize<'de>>(bytes: &[u8]) -> Result<Vec<T>, ApiError> {
    if !bytes.is_empty() && !bytes.ends_with(b"\n") {
        return Err(ApiError::unsupported_format("JSONL member must end with LF"));
    }
    bytes
        .split(|byte| *byte == b'\n')
        .filter(|line| !line.is_empty())
        .map(|line| {
            let value: Value =
                serde_json::from_slice(line).map_err(|_| ApiError::unsupported_format("JSONL member is invalid"))?;
            if serde_jcs::to_vec(&value).map_err(|_| ApiError::unsupported_format("JSONL cannot be canonicalized"))?
                != line
            {
                return Err(ApiError::unsupported_format("JSONL line is not canonical JSON"));
            }
            serde_json::from_value(value).map_err(|_| ApiError::unsupported_format("JSONL row shape is invalid"))
        })
        .collect()
}

pub(crate) fn member_bytes(bytes: &[u8], path: &str) -> Result<Vec<u8>, ApiError> {
    let mut archive =
        zip::ZipArchive::new(Cursor::new(bytes)).map_err(|_| ApiError::unsupported_format("invalid ZIP archive"))?;
    let mut file = archive
        .by_name(path)
        .map_err(|_| ApiError::unsupported_format("required package member is missing"))?;
    let mut body = Vec::with_capacity(usize::try_from(file.size()).unwrap_or(0));
    file.read_to_end(&mut body)
        .map_err(|_| ApiError::unsupported_format("package member cannot be decoded"))?;
    Ok(body)
}

fn validate_plan_graph(objects: &[PackageObject], relations: &[PackageRelation]) -> Result<(), ApiError> {
    let object_ids: HashSet<_> = objects.iter().map(|object| object.source_object_id).collect();
    let document_ids: HashSet<_> = objects.iter().map(|object| object.source_document_id).collect();
    if object_ids.len() != objects.len() || document_ids.len() != objects.len() {
        return Err(ApiError::unsupported_format(
            "package object or document ids are duplicated",
        ));
    }
    for object in objects {
        if object
            .parent_object_id
            .is_some_and(|parent| !object_ids.contains(&parent))
        {
            return Err(ApiError::unsupported_format(
                "object parent is not present in the complete package",
            ));
        }
    }
    let mut relation_ids = HashSet::new();
    for relation in relations {
        if !relation_ids.insert(relation.source_relation_id)
            || !matches!(relation.source.kind.as_str(), "internal" | "external")
            || !matches!(relation.target.kind.as_str(), "internal" | "external")
            || (relation.source.kind == "internal" && !object_ids.contains(&relation.source.source_object_id))
            || (relation.target.kind == "internal" && !object_ids.contains(&relation.target.source_object_id))
        {
            return Err(ApiError::unsupported_format(
                "relation mapping is incomplete or inconsistent",
            ));
        }
    }
    Ok(())
}

fn freeze_mapping(
    request: &PreviewImportRequest,
    plan: &PackagePlan,
    prior_targets: &HashMap<(String, Uuid), Uuid>,
) -> Result<FrozenImportMapping, ApiError> {
    let source_workspace = parse_source_workspace(&plan.verified.manifest)?;
    Ok(FrozenImportMapping {
        workspace_map: BTreeMap::from([(source_workspace, request.workspace_id)]),
        object_map: plan
            .objects
            .iter()
            .map(|row| {
                let key = ("object".to_string(), row.source_object_id);
                (
                    row.source_object_id,
                    prior_targets.get(&key).copied().unwrap_or_else(Uuid::new_v4),
                )
            })
            .collect(),
        document_map: plan
            .objects
            .iter()
            .map(|row| {
                let key = ("document".to_string(), row.source_document_id);
                (
                    row.source_document_id,
                    prior_targets.get(&key).copied().unwrap_or_else(Uuid::new_v4),
                )
            })
            .collect(),
        relation_map: plan
            .relations
            .iter()
            .map(|row| {
                let key = ("relation".to_string(), row.source_relation_id);
                (
                    row.source_relation_id,
                    prior_targets.get(&key).copied().unwrap_or_else(Uuid::new_v4),
                )
            })
            .collect(),
        project_map: request.project_map.clone(),
        external_reference_policy: request.external_reference_policy,
        conflict_policy: request.conflict_policy,
    })
}

fn validate_frozen_mapping(
    mapping: &FrozenImportMapping,
    plan: &PackagePlan,
    workspace_id: Uuid,
    policy: ConflictPolicy,
) -> Result<(), ApiError> {
    let source_workspace = parse_source_workspace(&plan.verified.manifest)?;
    if mapping.workspace_map != BTreeMap::from([(source_workspace, workspace_id)])
        || mapping.conflict_policy != policy
        || mapping.object_map.len() != plan.objects.len()
        || mapping.document_map.len() != plan.objects.len()
        || mapping.relation_map.len() != plan.relations.len()
        || plan.objects.iter().any(|row| {
            !mapping.object_map.contains_key(&row.source_object_id)
                || !mapping.document_map.contains_key(&row.source_document_id)
        })
        || plan
            .relations
            .iter()
            .any(|row| !mapping.relation_map.contains_key(&row.source_relation_id))
    {
        return Err(ApiError::checksum_mismatch(
            "frozen import mapping is incomplete or changed",
        ));
    }
    let targets: HashSet<_> = mapping
        .object_map
        .values()
        .chain(mapping.document_map.values())
        .chain(mapping.relation_map.values())
        .collect();
    if targets.len() != mapping.object_map.len() + mapping.document_map.len() + mapping.relation_map.len() {
        return Err(ApiError::checksum_mismatch("frozen target ids are not globally unique"));
    }
    Ok(())
}

/// Package source projects that `project_map` does not cover, in id order. A preview lists them as
/// `project_unmapped` conflicts; a commit refuses them.
fn unmapped_projects(project_map: &BTreeMap<Uuid, Option<Uuid>>, plan: &PackagePlan) -> Vec<Uuid> {
    source_projects(plan)
        .into_iter()
        .filter(|id| !project_map.contains_key(id))
        .collect()
}

/// Rebuilds the structured conflicts from the stored subject ids, so a fresh preview and its
/// idempotent replay answer the same `conflict_details`.
/// A lineage conflict's existing target is the frozen `object_map` entry, which preview pins to the
/// prior lineage target.
fn conflict_details(
    conflicts: &[String],
    mapping: &FrozenImportMapping,
    summary: &ManifestSummary,
) -> Result<Vec<ImportConflict>, ApiError> {
    conflicts
        .iter()
        .map(|subject| {
            let id = Uuid::parse_str(subject).map_err(|_| ApiError::internal())?;
            if let Some(existing_target_id) = mapping.object_map.get(&id) {
                return Ok(ImportConflict::LineageExists {
                    source_object_id: id,
                    existing_target_id: *existing_target_id,
                });
            }
            summary
                .projects
                .iter()
                .find(|project| project.source_project_id == id)
                .map(|project| ImportConflict::ProjectUnmapped {
                    source_project_id: id,
                    title: project.title.clone(),
                })
                .ok_or_else(ApiError::internal)
        })
        .collect()
}

async fn validate_projects<C: ConnectionTrait>(
    conn: &C,
    workspace_id: Uuid,
    project_map: &BTreeMap<Uuid, Option<Uuid>>,
    plan: &PackagePlan,
) -> Result<(), ApiError> {
    if !unmapped_projects(project_map, plan).is_empty() {
        return Err(ApiError::invalid_update_with_details(
            "project_map must cover every source project",
            json!({ "reason": PROJECT_UNMAPPED_REASON }),
        ));
    }
    validate_project_targets(conn, workspace_id, project_map).await
}

async fn validate_project_targets<C: ConnectionTrait>(
    conn: &C,
    workspace_id: Uuid,
    project_map: &BTreeMap<Uuid, Option<Uuid>>,
) -> Result<(), ApiError> {
    for target in project_map.values().flatten() {
        let found = conn
            .query_one(Statement::from_sql_and_values(
                DbBackend::Postgres,
                "SELECT id FROM projects WHERE id=$1 AND workspace_id=$2",
                vec![(*target).into(), workspace_id.into()],
            ))
            .await?;
        if found.is_none() {
            return Err(ApiError::invalid_update(
                "project_map target does not belong to the target workspace".to_string(),
            ));
        }
    }
    Ok(())
}

async fn lineage_targets<C: ConnectionTrait>(
    conn: &C,
    workspace_id: Uuid,
    plan: &PackagePlan,
) -> Result<HashMap<(String, Uuid), Uuid>, ApiError> {
    #[derive(FromQueryResult)]
    struct Row {
        source_id: Uuid,
        target_kind: String,
        target_id: Uuid,
    }
    let source_ids: Vec<_> = plan
        .objects
        .iter()
        .flat_map(|row| [row.source_object_id, row.source_document_id])
        .chain(plan.relations.iter().map(|row| row.source_relation_id))
        .collect();
    let rows = Row::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "SELECT source_id,target_kind,target_id FROM flow_import_lineage WHERE target_workspace_id=$1 AND package_sha256=$2 AND source_kind='flow_package' AND source_id=ANY($3)",
        vec![workspace_id.into(), plan.verified.package_sha256.clone().into(), source_ids.into()],
    )).all(conn).await?;
    Ok(rows
        .into_iter()
        .map(|row| ((row.target_kind, row.source_id), row.target_id))
        .collect())
}

async fn promote_objects(
    tx: &sea_orm::DatabaseTransaction,
    validated: &[super::import_replay::ValidatedDocument],
    plan: &PackagePlan,
    mapping: &FrozenImportMapping,
    request: &CommitImportRequest,
    import_id: Uuid,
) -> Result<(u64, u64), ApiError> {
    let mut pending: HashMap<Uuid, &PackageObject> =
        plan.objects.iter().map(|row| (row.source_object_id, row)).collect();
    let mut inserted = HashSet::new();
    let mut created = 0u64;
    let mut reused = 0u64;
    while !pending.is_empty() {
        let ready: Vec<_> = pending
            .iter()
            .filter(|(_, row)| row.parent_object_id.is_none_or(|parent| inserted.contains(&parent)))
            .map(|(id, _)| *id)
            .collect();
        if ready.is_empty() {
            return Err(ApiError::unsupported_format("object parent graph contains a cycle"));
        }
        for source_id in ready {
            let object = pending.remove(&source_id).ok_or_else(ApiError::internal)?;
            let target_object = mapping
                .object_map
                .get(&source_id)
                .copied()
                .ok_or_else(ApiError::internal)?;
            let target_document = mapping
                .document_map
                .get(&object.source_document_id)
                .copied()
                .ok_or_else(ApiError::internal)?;
            // The head the worker validated, its projection and its title: what is stored comes
            // from here, never from the uploaded bytes.
            let valid = validated
                .iter()
                .find(|document| document.source == source_id)
                .ok_or_else(ApiError::internal)?;
            let accepted_frontier = valid.frontier.clone();
            let snapshot = valid.snapshot.clone();
            let semantic = &valid.semantic;
            if mapping.conflict_policy == ConflictPolicy::ReuseImportLineage {
                #[derive(FromQueryResult)]
                struct ExistingHead {
                    head_seq: i64,
                    head_frontier: Vec<u8>,
                    snapshot: Vec<u8>,
                }
                if let Some(existing) = ExistingHead::find_by_statement(Statement::from_sql_and_values(
                    DbBackend::Postgres,
                    "SELECT cd.head_seq,cd.head_frontier,cd.snapshot FROM flow_objects fo \
                     JOIN collab_documents cd ON cd.object_id=fo.id \
                     WHERE fo.workspace_id=$1 AND fo.id=$2 AND cd.id=$3",
                    vec![
                        request.workspace_id.into(),
                        target_object.into(),
                        target_document.into(),
                    ],
                ))
                .one(tx)
                .await?
                {
                    let existing_engine = LoroCollabEngine::load(&existing.snapshot).map_err(|_| {
                        ApiError::checksum_mismatch("reused import lineage points at an invalid document")
                    })?;
                    if existing.head_seq != object.accepted_seq
                        || !super::frontier::same(&accepted_frontier, &existing.head_frontier)?
                        || package_semantic_hash(&existing_engine, &plan.verified.manifest)? != object.semantic_hash
                    {
                        return Err(ApiError::stale_frontier(
                            "reused import lineage target has drifted",
                            Some(existing.head_seq),
                            None,
                        ));
                    }
                    inserted.insert(source_id);
                    reused = reused.saturating_add(1);
                    continue;
                }
            }
            let project_id = object
                .project_id
                .and_then(|id| mapping.project_map.get(&id).copied().flatten());
            let parent_id = if let Some(source_parent) = object.parent_object_id {
                Some(
                    mapping
                        .object_map
                        .get(&source_parent)
                        .copied()
                        .ok_or_else(ApiError::internal)?,
                )
            } else {
                Some(repository::ensure_navigator_root(tx, request.workspace_id, project_id).await?)
            };
            let lifecycle = object.lifecycle_status.as_str();
            if !matches!(lifecycle, "active" | "archived") || object.engine != "loro" {
                return Err(ApiError::unsupported_format(
                    "object lifecycle or engine is unsupported",
                ));
            }
            let archived_at = (lifecycle == "archived").then_some(Timestamp::now());
            let governance = sanitize_governance(&object.governance_metadata)?;
            tx.execute(Statement::from_sql_and_values(
                DbBackend::Postgres,
                "INSERT INTO flow_objects \
                 (id,workspace_id,project_id,object_type,parent_id,governance_metadata,lifecycle_status,created_by,updated_by,archived_at) \
                 VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$8,$9)",
                vec![target_object.into(),request.workspace_id.into(),project_id.into(),object.object_type.clone().into(),parent_id.into(),
                    governance.into(),lifecycle.into(),actor_user_id(&request.principal).into(),archived_at.into()],
            )).await?;
            let byte_count = i64::try_from(snapshot.len()).map_err(|_| ApiError::internal())?;
            tx.execute(Statement::from_sql_and_values(
                DbBackend::Postgres,
                "INSERT INTO collab_documents \
                 (id,object_id,engine,format_version,snapshot,snapshot_frontier,snapshot_seq,head_frontier,head_seq,byte_count,update_count) \
                 VALUES ($1,$2,'loro',$3,$4,$5,$6,$5,$6,$7,0)",
                vec![target_document.into(),target_object.into(),object.format_version.clone().into(),snapshot.into(),
                    accepted_frontier.clone().into(),object.accepted_seq.into(),byte_count.into()],
            )).await?;
            repository::insert_projection(
                tx,
                &repository::NewProjection {
                    object_id: target_object,
                    document_seq: object.accepted_seq,
                    document_frontier: accepted_frontier,
                    title: valid.title.clone(),
                    state: projection::state_json(semantic).map_err(|_| ApiError::internal())?,
                    plain_text: projection::plain_text(semantic),
                },
            )
            .await?;
            if object.object_type == "page" {
                super::block_registry::reconcile_accepted(
                    tx,
                    target_object,
                    object.accepted_seq,
                    &super::block_registry::prepare(semantic)?,
                    super::block_registry::RegistrationAllowance::Complete,
                )
                .await?;
                super::body_assets::reconcile_accepted(
                    tx,
                    target_object,
                    object.accepted_seq,
                    &super::body_assets::typed_references(semantic)?,
                )
                .await?;
            }
            insert_package_lineage(
                tx,
                import_id,
                request.workspace_id,
                &plan.verified.package_sha256,
                "object",
                source_id,
                &object.semantic_hash,
                target_object,
            )
            .await?;
            insert_package_lineage(
                tx,
                import_id,
                request.workspace_id,
                &plan.verified.package_sha256,
                "document",
                object.source_document_id,
                &object.semantic_hash,
                target_document,
            )
            .await?;
            inserted.insert(source_id);
            created = created.saturating_add(1);
            #[cfg(test)]
            fail_promotion_at(request.preview_id, PromotionFaultPoint::ObjectPromoted)?;
        }
    }
    Ok((created, reused))
}

async fn promote_relations<C: ConnectionTrait>(
    tx: &C,
    plan: &PackagePlan,
    mapping: &FrozenImportMapping,
    request: &CommitImportRequest,
    import_id: Uuid,
) -> Result<Vec<Uuid>, ApiError> {
    let mut detached = Vec::new();
    for relation in &plan.relations {
        let source = mapping.object_map.get(&relation.source.source_object_id).copied();
        let target = mapping.object_map.get(&relation.target.source_object_id).copied();
        let (Some(source), Some(target)) = (source, target) else {
            if mapping.external_reference_policy == ExternalReferencePolicy::Reject {
                return Err(ApiError::invalid_update(
                    "external relation reference rejected by import policy".to_string(),
                ));
            }
            detached.push(relation.source_relation_id);
            continue;
        };
        let relation_id = mapping
            .relation_map
            .get(&relation.source_relation_id)
            .copied()
            .ok_or_else(ApiError::internal)?;
        if mapping.conflict_policy == ConflictPolicy::ReuseImportLineage {
            let existing = tx
                .query_one(Statement::from_sql_and_values(
                    DbBackend::Postgres,
                    "SELECT id FROM flow_relations WHERE id=$1 AND workspace_id=$2 \
                     AND source_object_id=$3 AND target_object_id=$4 AND relation_type=$5",
                    vec![
                        relation_id.into(),
                        request.workspace_id.into(),
                        source.into(),
                        target.into(),
                        relation.relation_type.clone().into(),
                    ],
                ))
                .await?;
            if existing.is_some() {
                continue;
            }
        }
        tx.execute(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "INSERT INTO flow_relations \
             (id,workspace_id,relation_type,source_object_id,target_object_id,position_key,properties,created_by) \
             VALUES ($1,$2,$3,$4,$5,$6,$7,$8)",
            vec![
                relation_id.into(),
                request.workspace_id.into(),
                relation.relation_type.clone().into(),
                source.into(),
                target.into(),
                relation.position_key.clone().into(),
                relation.properties.clone().into(),
                actor_user_id(&request.principal).into(),
            ],
        ))
        .await?;
        insert_package_lineage(
            tx,
            import_id,
            request.workspace_id,
            &plan.verified.package_sha256,
            "relation",
            relation.source_relation_id,
            &sha256_json(&serde_json::to_value(relation).map_err(|_| ApiError::internal())?)?,
            relation_id,
        )
        .await?;
    }
    Ok(detached)
}

async fn insert_package_lineage<C: ConnectionTrait>(
    tx: &C,
    import_id: Uuid,
    workspace_id: Uuid,
    package_sha256: &str,
    target_kind: &str,
    source_id: Uuid,
    source_hash: &str,
    target_id: Uuid,
) -> Result<(), ApiError> {
    tx.execute(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "INSERT INTO flow_import_lineage \
         (import_id,source_kind,source_id,target_workspace_id,source_content_hash,result,package_sha256,target_kind,target_id) \
         VALUES ($1,'flow_package',$2,$3,$4,'created',$5,$6,$7)",
        vec![import_id.into(),source_id.into(),workspace_id.into(),source_hash.into(),package_sha256.into(),
            target_kind.into(),target_id.into()],
    )).await?;
    Ok(())
}

pub(crate) fn sanitize_governance(value: &Value) -> Result<Value, ApiError> {
    let mut object = value
        .as_object()
        .cloned()
        .ok_or_else(|| ApiError::unsupported_format("governance metadata must be an object"))?;
    for key in [
        "system_role",
        "workspace_members",
        "grants",
        "tokens",
        "tickets",
        "presence",
    ] {
        object.remove(key);
    }
    Ok(Value::Object(object))
}

fn parse_manifest_uuid(manifest: &ExportPackageManifest) -> Result<Uuid, ApiError> {
    Uuid::parse_str(&manifest.package_id).map_err(|_| ApiError::unsupported_format("package_id is not a UUID"))
}

fn parse_source_workspace(manifest: &ExportPackageManifest) -> Result<Uuid, ApiError> {
    Uuid::parse_str(
        manifest
            .source
            .workspace_id
            .as_deref()
            .ok_or_else(|| ApiError::unsupported_format("scope-native package requires scope-aware promotion"))?,
    )
    .map_err(|_| ApiError::unsupported_format("source workspace id is not a UUID"))
}

pub(crate) fn estimated_changes(plan: &PackagePlan) -> BTreeMap<String, u64> {
    BTreeMap::from([
        (
            "objects".to_string(),
            u64::try_from(plan.objects.len()).unwrap_or(u64::MAX),
        ),
        (
            "documents".to_string(),
            u64::try_from(plan.objects.len()).unwrap_or(u64::MAX),
        ),
        (
            "relations".to_string(),
            u64::try_from(plan.relations.len()).unwrap_or(u64::MAX),
        ),
    ])
}

pub(crate) fn manifest_summary(plan: &PackagePlan) -> ManifestSummary {
    let manifest = &plan.verified.manifest;
    let titles: HashMap<&str, &str> = manifest
        .projects
        .iter()
        .map(|project| (project.source_project_id.as_str(), project.title.as_str()))
        .collect();
    let projects = source_projects(plan)
        .into_iter()
        .map(|source_project_id| ManifestProjectSummary {
            source_project_id,
            title: titles
                .get(source_project_id.to_string().as_str())
                .map(|title| (*title).to_string()),
        })
        .collect();
    ManifestSummary {
        projects,
        object_count: manifest.counts.objects,
        document_count: manifest.counts.documents,
        package_schema: manifest.schema.clone(),
        engine: manifest.engine.clone(),
        source: Some(manifest.source.clone()),
        history: Some(json!({"included":manifest.history.included,"snapshot_history":"retained"})),
    }
}

/// Every source project the package's objects reference, in id order. Derived from the verified
/// object metadata, not from the optional manifest list, so it is complete for any v1 package.
fn source_projects(plan: &PackagePlan) -> BTreeSet<Uuid> {
    plan.objects.iter().filter_map(|row| row.project_id).collect()
}

pub(crate) fn sha256_json(value: &Value) -> Result<String, ApiError> {
    let bytes = serde_jcs::to_vec(value).map_err(|_| ApiError::internal())?;
    Ok(hex::encode(Sha256::digest(bytes)))
}

fn commit_request_hash(request: &CommitImportRequest) -> Result<String, ApiError> {
    sha256_json(&json!({
        "preview_id":request.preview_id,"package_sha256":request.package_sha256,"mapping_hash":request.mapping_hash,
        "conflict_policy":request.conflict_policy,"confirm":request.confirm,
    }))
}

fn import_event(
    workspace_id: Uuid,
    aggregate_id: Uuid,
    principal: &ImportPrincipal,
    event_type: &str,
    payload: Option<Value>,
) -> BusinessEventInput {
    BusinessEventInput {
        workspace_id,
        project_id: None,
        event_type: event_type.to_string(),
        aggregate_type: "flow_import".to_string(),
        aggregate_id: aggregate_id.to_string(),
        actor_id: actor_user_id(principal),
        source: json!({"surface":"system","actor_kind":principal.kind}),
        payload: payload.unwrap_or_else(|| json!({"preview_id":aggregate_id})),
        metadata: json!({}),
        correlation_id: None,
        causation_id: None,
        idempotency_key: None,
    }
}

pub(crate) fn package_semantic_hash(
    engine: &LoroCollabEngine,
    manifest: &ExportPackageManifest,
) -> Result<String, ApiError> {
    super::package::semantic_model(manifest)?.semantic_hash(engine)
}

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::items_after_statements,
    clippy::print_stderr,
    clippy::struct_field_names
)]
mod tests {
    use super::*;
    use crate::flow::command::{
        CreateObjectInput, ExecuteCommandInput, SetFlowFeatureInput, create_object, execute_command, set_flow_feature,
    };
    use crate::flow::event_origin::{CommandOrigin, EventSurface};
    use crate::flow::export::{CreateExportRequest, ExportPrincipal, ExportScope, create_package_export};
    use crate::flow::package::{PackageMemberInput, build_package};
    use crate::routes::context::tenant_fixture::{exec, scratch, seed_tenant};
    use base64::Engine as _;
    use platform::{
        app::AppState,
        config::{AppConfig, Secret},
    };

    /// In-process replay of an uploaded document, for the unit tests of the fingerprints and of the
    /// host's shadow-root check on fixtures. Production never decodes uploaded bytes here: the
    /// isolated worker does (`import_replay`).
    fn reconstruct_head(
        package: &[u8],
        manifest: &ExportPackageManifest,
        object: &PackageObject,
    ) -> Result<Vec<u8>, ApiError> {
        let snapshot_path = format!("documents/{}/snapshot.bin", object.source_document_id);
        let snapshot = member_bytes(package, &snapshot_path)?;
        let mut engine = LoroCollabEngine::load(&snapshot)
            .map_err(|_| ApiError::checksum_mismatch("package snapshot cannot be decoded"))?;
        let prefix = format!("documents/{}/updates/", object.source_document_id);
        let mut updates: Vec<_> = manifest
            .members
            .iter()
            .filter(|member| member.path.starts_with(&prefix))
            .map(|member| {
                let seq = member
                    .path
                    .strip_prefix(&prefix)
                    .and_then(|tail| tail.strip_suffix(".bin"))
                    .and_then(|value| value.parse::<i64>().ok())
                    .ok_or_else(|| ApiError::unsupported_format("update sequence path is invalid"))?;
                Ok((seq, member.path.as_str()))
            })
            .collect::<Result<_, ApiError>>()?;
        updates.sort_unstable_by_key(|(seq, _)| *seq);
        for (_, path) in updates {
            engine
                .import_update(&member_bytes(package, path)?)
                .map_err(|_| ApiError::checksum_mismatch("package update cannot be applied"))?;
        }
        super::super::package::require_no_shadow_roots(&engine)?;
        engine
            .export_snapshot()
            .map_err(|_| ApiError::checksum_mismatch("package head cannot be compacted"))
    }

    #[test]
    fn owned_v1_browser_package_uses_exact_legacy_fingerprint_and_v2_does_not() {
        use super::super::package::*;
        const OLD_HASH: &str = "1c60df0b968bf3771931d029a3fe501f1e0b51f760012ea458f8bf46a1fc6026";
        let path = std::path::PathBuf::from(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/fixtures/flow-v2/legacy-browser-package-v1"
        ));
        if std::env::var_os("FLOW_WRITE_LEGACY_BROWSER_FIXTURE").is_some() {
            let snapshot = std::fs::read(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/../../frontend/tests/fixtures/editor-doc-v1/browser.snapshot"
            ))
            .unwrap();
            let engine = LoroCollabEngine::load(&snapshot).unwrap();
            assert_eq!(
                engine.legacy_package_semantic_snapshot_v1().unwrap().semantic_hash(),
                OLD_HASH
            );
            let object = Uuid::from_u128(0x1111_1111_1111_4111_8111_1111_1111_1111);
            let document = Uuid::from_u128(0x2222_2222_2222_4222_8222_2222_2222_2222);
            let workspace = Uuid::from_u128(0x3333_3333_3333_4333_8333_3333_3333_3333);
            let metadata = json!({"source_object_id":object,"source_workspace_id":workspace,"object_type":"page","lifecycle_status":"active","project_id":null,"parent_object_id":null,"governance_metadata":{},"source_document_id":document,"engine":"loro","format_version":"loro-1","accepted_seq":0,"accepted_frontier":base64::engine::general_purpose::STANDARD.encode(engine.frontier().as_bytes()),"semantic_hash":OLD_HASH});
            let manifest = ExportPackageManifest {
                schema: PACKAGE_SCHEMA.into(),
                package_id: "44444444-4444-4444-8444-444444444444".into(),
                created_at: "2026-11-15T00:00:00Z".into(),
                producer: PackageProducer {
                    product: "sylvode".into(),
                    version: "0.2.536".into(),
                    source_head: "d523d7c724b4122f7b83cb30e6614a36f51a1c65".into(),
                },
                source: PackageSource {
                    workspace_id: Some(workspace.to_string()),
                    scope_id: None,
                    scope: "object".into(),
                    root_object_ids: vec![object.to_string()],
                },
                flow_schema_version: 1,
                engine: PackageEngine {
                    name: ENGINE_NAME.into(),
                    crate_version: ENGINE_CRATE_VERSION.into(),
                    wire_format_version: ENGINE_WIRE_FORMAT_VERSION,
                    semantic_model: None,
                },
                history: PackageHistory {
                    included: false,
                    snapshot_history: None,
                    through_seq_by_document: BTreeMap::from([(document.to_string(), 0)]),
                },
                counts: PackageCounts {
                    objects: 1,
                    documents: 1,
                    ..PackageCounts::default()
                },
                members: Vec::new(),
                export_policy: ExportPolicy {
                    complete: true,
                    permission_snapshot_at: "2026-11-15T00:00:00Z".into(),
                },
                projects: Vec::new(),
                warnings: Vec::new(),
            };
            let built = build_package(
                manifest,
                vec![
                    PackageMemberInput {
                        path: format!("objects/{object}/object.json"),
                        kind: "object".into(),
                        bytes: serde_jcs::to_vec(&metadata).unwrap(),
                    },
                    PackageMemberInput {
                        path: format!("documents/{document}/snapshot.bin"),
                        kind: "snapshot".into(),
                        bytes: snapshot.clone(),
                    },
                    PackageMemberInput {
                        path: "relations/relations.jsonl".into(),
                        kind: "relation".into(),
                        bytes: Vec::new(),
                    },
                    PackageMemberInput {
                        path: "lineage/lineage.jsonl".into(),
                        kind: "lineage".into(),
                        bytes: Vec::new(),
                    },
                ],
            )
            .unwrap();
            std::fs::create_dir_all(&path).unwrap();
            std::fs::write(path.join("package.zip"), &built.bytes).unwrap();
            std::fs::write(path.join("contract.json"),serde_json::to_vec_pretty(&json!({"fixture_kind":"synthetic_pre_change_v1_format","reader_source":"d523d7c724b4122f7b83cb30e6614a36f51a1c65:crates/collab-core/src/engine.rs semantic_snapshot (tree-only)","package_sha256":built.package_sha256,"snapshot_sha256":super::super::collab::bootstrap::content_hash(&snapshot),"legacy_semantic_hash":OLD_HASH,"canonical_semantic_hash":engine.semantic_snapshot().unwrap().semantic_hash()})).unwrap()).unwrap();
        }
        let bytes = std::fs::read(path.join("package.zip")).unwrap();
        let contract: Value = serde_json::from_slice(&std::fs::read(path.join("contract.json")).unwrap()).unwrap();
        let plan = parse_plan(&bytes, contract["package_sha256"].as_str().unwrap()).unwrap();
        let snapshot = reconstruct_head(&bytes, &plan.verified.manifest, &plan.objects[0]).unwrap();
        let engine = LoroCollabEngine::load(&snapshot).unwrap();
        assert_eq!(
            package_semantic_hash(&engine, &plan.verified.manifest).unwrap(),
            OLD_HASH
        );
        assert_eq!(plan.objects[0].semantic_hash, OLD_HASH);
        assert_ne!(engine.semantic_snapshot().unwrap().semantic_hash(), OLD_HASH);
        let mut v2 = plan.verified.manifest.clone();
        v2.schema = SCOPE_PACKAGE_SCHEMA.into();
        v2.flow_schema_version = 2;
        v2.source.scope_id = Some("55555555-5555-4555-8555-555555555555".into());
        assert_eq!(
            package_semantic_hash(&engine, &v2).unwrap(),
            contract["canonical_semantic_hash"].as_str().unwrap()
        );
        assert_ne!(
            package_semantic_hash(&engine, &v2).unwrap(),
            plan.objects[0].semantic_hash
        );
    }

    /// The stored legacy browser package, rebuilt with `snapshot` as its document snapshot and,
    /// when given, `tail` as the single update after it.
    fn rebuilt_package(snapshot: &[u8], tail: Option<&[u8]>) -> (Vec<u8>, String) {
        rebuilt_package_with_frontier(snapshot, tail, None)
    }

    /// [`rebuilt_package`] whose object names `frontier` as its accepted frontier, for a snapshot
    /// that is not the one of the stored fixture.
    fn rebuilt_package_with_frontier(
        snapshot: &[u8],
        tail: Option<&[u8]>,
        frontier: Option<&[u8]>,
    ) -> (Vec<u8>, String) {
        let path = std::path::PathBuf::from(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/fixtures/flow-v2/legacy-browser-package-v1"
        ));
        let bytes = std::fs::read(path.join("package.zip")).unwrap();
        let contract: Value = serde_json::from_slice(&std::fs::read(path.join("contract.json")).unwrap()).unwrap();
        let plan = parse_plan(&bytes, contract["package_sha256"].as_str().unwrap()).unwrap();
        let document = plan.objects[0].source_document_id;
        let mut members = Vec::new();
        for member in &plan.verified.manifest.members {
            let mut body = if member.kind == "snapshot" {
                snapshot.to_vec()
            } else {
                member_bytes(&bytes, &member.path).unwrap()
            };
            if member.kind == "object" && (tail.is_some() || frontier.is_some()) {
                let mut object: Value = serde_json::from_slice(&body).unwrap();
                if tail.is_some() {
                    object["accepted_seq"] = json!(1);
                }
                if let Some(frontier) = frontier {
                    object["accepted_frontier"] = json!(base64::engine::general_purpose::STANDARD.encode(frontier));
                }
                body = serde_jcs::to_vec(&object).unwrap();
            }
            members.push(PackageMemberInput {
                path: member.path.clone(),
                kind: member.kind.clone(),
                bytes: body,
            });
        }
        if let Some(tail) = tail {
            members.push(PackageMemberInput {
                path: format!("documents/{document}/updates/1.bin"),
                kind: "update".into(),
                bytes: tail.to_vec(),
            });
            members.push(PackageMemberInput {
                path: "history/events.jsonl".into(),
                kind: "event".into(),
                bytes: Vec::new(),
            });
        }
        let mut manifest = plan.verified.manifest;
        if tail.is_some() {
            manifest.history.included = true;
            manifest.counts.updates = 1;
            manifest.history.through_seq_by_document.insert(document.to_string(), 1);
        }
        let built = build_package(manifest, members).unwrap();
        (built.bytes, built.package_sha256)
    }

    fn reconstruct(package: (Vec<u8>, String)) -> Result<Vec<u8>, ApiError> {
        let (bytes, hash) = package;
        let plan = parse_plan(&bytes, &hash)?;
        reconstruct_head(&bytes, &plan.verified.manifest, &plan.objects[0])
    }

    fn assert_shadow_refusal(outcome: Result<Vec<u8>, ApiError>, what: &str) {
        let error = outcome.expect_err(what);
        assert_eq!(error.kind(), crate::error::ApiErrorKind::InvalidUpdate, "{what}");
        let rendered = format!("{error:?}");
        assert!(rendered.contains("reserved_root_container_type"), "{what}: {rendered}");
    }

    /// An uploaded document that carries a shadow root is refused with a typed error naming the
    /// reason, whether the root sits in the uploaded snapshot or arrives in its uploaded tail. The
    /// same package without the shadow is accepted as before.
    #[test]
    fn a_package_document_with_a_shadow_root_is_refused() {
        let clean = std::fs::read(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../frontend/tests/fixtures/editor-doc-v1/browser.snapshot"
        ))
        .unwrap();
        // Unmodified: accepted, with and without an update tail.
        reconstruct(rebuilt_package(&clean, None)).unwrap();
        let benign = {
            let doc = loro::LoroDoc::new();
            doc.import(&clean).unwrap();
            doc.set_peer_id(9_100_000).unwrap();
            let from = doc.oplog_vv();
            doc.get_map("scratch_root").insert("k", 1).unwrap();
            doc.commit();
            doc.export(loro::ExportMode::updates(&from)).unwrap()
        };
        reconstruct(rebuilt_package(&clean, Some(&benign))).unwrap();

        for (name, kind) in [
            ("flow_editor_structure_meta", "text"),
            ("doc", "list"),
            ("order", "text"),
        ] {
            let shadow_update = |base: &[u8]| {
                let doc = loro::LoroDoc::new();
                doc.import(base).unwrap();
                doc.set_peer_id(9_100_001).unwrap();
                let from = doc.oplog_vv();
                match kind {
                    "text" => doc.get_text(name).insert(0, "x").unwrap(),
                    _ => doc.get_list(name).insert(0, 1).unwrap(),
                }
                doc.commit();
                doc.export(loro::ExportMode::updates(&from)).unwrap()
            };
            // In the uploaded snapshot (state and history).
            let doc = loro::LoroDoc::new();
            doc.import(&clean).unwrap();
            doc.import(&shadow_update(&clean)).unwrap();
            let with_shadow = doc.export(loro::ExportMode::Snapshot).unwrap();
            assert_shadow_refusal(
                reconstruct(rebuilt_package(&with_shadow, None)),
                &format!("snapshot with a shadow {name}"),
            );
            // In the uploaded update tail only.
            assert_shadow_refusal(
                reconstruct(rebuilt_package(&clean, Some(&shadow_update(&clean)))),
                &format!("tail with a shadow {name}"),
            );
        }
    }

    #[test]
    fn navigator_packages_select_explicit_frozen_or_current_semantic_models() {
        let snapshot =
            include_bytes!("../../../../frontend/tests/fixtures/flow-navigator-order/browser-reedit.snapshot").to_vec();
        let engine = LoroCollabEngine::load(&snapshot).unwrap();
        let encoded_before = engine.export_snapshot().unwrap();
        let frontier_before = engine.frontier();
        let state_before = engine.root_state_json();
        assert!(super::super::package::require_supported_snapshot(&engine).is_err());
        let old_hash = engine.legacy_package_semantic_snapshot_v2().unwrap().semantic_hash();
        let current_hash = engine.semantic_snapshot().unwrap().semantic_hash();
        assert_ne!(old_hash, current_hash);
        let prior = std::fs::read(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/fixtures/flow-v2/legacy-browser-package-v1/package.zip"
        ))
        .unwrap();
        let verified = super::super::package::verify_package(std::io::Cursor::new(prior), None).unwrap();
        let mut manifest = verified.manifest;
        manifest.schema = super::super::package::SCOPE_PACKAGE_SCHEMA.into();
        manifest.flow_schema_version = 2;
        manifest.source.workspace_id = None;
        manifest.source.scope_id = Some("eeeeeeee-eeee-4eee-8eee-eeeeeeeeeeee".into());
        manifest.history.snapshot_history = Some("retained".into());
        manifest.engine.semantic_model = None;
        assert_eq!(package_semantic_hash(&engine, &manifest).unwrap(), old_hash);
        manifest.engine.semantic_model = Some(super::super::package::SEMANTIC_MODEL_V2.into());
        assert_eq!(package_semantic_hash(&engine, &manifest).unwrap(), old_hash);
        manifest.engine.semantic_model = Some(super::super::package::SEMANTIC_MODEL_V3.into());
        assert_eq!(package_semantic_hash(&engine, &manifest).unwrap(), current_hash);
        manifest.engine.semantic_model = Some("unknown_future_model".into());
        assert!(package_semantic_hash(&engine, &manifest).is_err());
        // Fingerprint readers may not materialize an editor structure root in a legacy snapshot.
        assert_eq!(engine.root_state_json(), state_before);
        assert_eq!(engine.frontier(), frontier_before);
        assert_eq!(engine.export_snapshot().unwrap(), encoded_before);
    }

    fn state_for(db: DatabaseConnection) -> AppState {
        AppState {
            cfg: AppConfig {
                app_name: "flow-package-import-test".to_string(),
                bind_addr: "127.0.0.1:0".to_string(),
                database_url: Secret::new("postgres://unused/unused"),
                jwt_secret: Secret::new("flow-package-import-test-secret"),
                jwt_access_ttl_seconds: 900,
                jwt_refresh_ttl_seconds: 3600,
                default_author_id: None,
                allow_insecure_cookies: false,
                collab_allowed_origins: Vec::new(),
            },
            db,
            flow_permission_cache: platform::app::FlowPermissionCacheSlot::default(),
        }
    }

    async fn enabled_workspace(state: &AppState, label: &str) -> (Uuid, Uuid) {
        let tenant = seed_tenant(&state.db, label).await;
        exec(
            &state.db,
            "UPDATE workspace_members SET role='owner' WHERE workspace_id=$1 AND user_id=$2",
            vec![tenant.workspace_id.into(), tenant.member_id.into()],
        )
        .await;
        set_flow_feature(
            state,
            SetFlowFeatureInput {
                workspace_id: tenant.workspace_id,
                actor_id: tenant.member_id,
                actor_is_bot: false,
                enabled: Some(true),
                default_member_level: Some("edit".to_string()),
                idempotency_key: format!("enable-{label}"),
                origin: CommandOrigin::first_request_from(EventSurface::Rest),
            },
        )
        .await
        .unwrap();
        (tenant.workspace_id, tenant.member_id)
    }

    async fn exported_fixture(state: &AppState, source_workspace: Uuid, source_owner: Uuid, label: &str) -> Vec<u8> {
        #[derive(FromQueryResult)]
        struct Root {
            id: Uuid,
        }
        let root = Root::find_by_statement(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT id FROM flow_objects WHERE workspace_id=$1 AND object_type='navigator' AND parent_id IS NULL",
            vec![source_workspace.into()],
        ))
        .one(&state.db)
        .await
        .unwrap()
        .unwrap();
        let page = create_object(
            state,
            CreateObjectInput {
                workspace_id: source_workspace,
                actor_id: source_owner,
                actor_is_bot: false,
                object_type: "page".to_string(),
                project_id: None,
                parent_object_id: Some(root.id),
                title: "portable state".to_string(),
                idempotency_key: format!("create-{label}"),
                message: None,
                origin: CommandOrigin::first_request_from(EventSurface::Rest),
            },
        )
        .await
        .unwrap()
        .object
        .id;
        execute_command(
            state,
            ExecuteCommandInput {
                object_id: page,
                actor_id: source_owner,
                principal_kind: "user".to_string(),
                role: "owner".to_string(),
                command_type: "set_title".to_string(),
                payload: json!({"title":"portable accepted head"}),
                expected_frontier: None,
                idempotency_key: format!("update-{label}"),
                message: None,
                origin_client_id: "package-import-test".to_string(),
                origin: CommandOrigin::first_request_from(EventSurface::Rest),
            },
        )
        .await
        .unwrap();
        let receipt = create_package_export(
            &state.db,
            &CreateExportRequest {
                object_types: None,
                scope: ExportScope::Workspace {
                    workspace_id: source_workspace,
                    project_id: None,
                },
                format: "package".to_string(),
                at_seq: None,
                include_history: true,
                idempotency_key: format!("export-{label}"),
                source_head: "0123456789abcdef0123456789abcdef01234567".to_string(),
                principal: ExportPrincipal {
                    id: source_owner,
                    kind: "user".to_string(),
                    role: "owner".to_string(),
                    workspace_export_capability: false,
                },
            },
        )
        .await
        .unwrap();
        assert_eq!(
            receipt.status, "completed",
            "source package export failed: {:?}",
            receipt.error
        );
        let (bytes, checksum, format) = crate::flow::export::download_package_export(
            &state.db,
            receipt.job_id,
            &ExportPrincipal {
                id: source_owner,
                kind: "user".to_string(),
                role: "owner".to_string(),
                workspace_export_capability: false,
            },
        )
        .await
        .expect("completed fixture artifact can be downloaded");
        assert_eq!(checksum, receipt.checksum);
        assert_eq!(format, "package");
        bytes
    }

    fn principal(id: Uuid) -> ImportPrincipal {
        ImportPrincipal {
            id,
            kind: "user".to_string(),
            role: "owner".to_string(),
        }
    }

    async fn canonical_count(db: &DatabaseConnection, workspace_id: Uuid) -> i64 {
        db.query_one(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT COUNT(*)::bigint AS count FROM flow_objects WHERE workspace_id=$1",
            vec![workspace_id.into()],
        ))
        .await
        .unwrap()
        .unwrap()
        .try_get("", "count")
        .unwrap()
    }

    async fn prepare_preview(
        db: &DatabaseConnection,
        workspace_id: Uuid,
        owner_id: Uuid,
        package: Vec<u8>,
        label: &str,
    ) -> (ImportPreviewReceipt, CommitImportRequest) {
        let principal = principal(owner_id);
        let replay_package = package.clone();
        let artifact_key = format!("artifact-{label}");
        let (artifact_id, package_sha256, _) =
            upload_package_artifact(db, workspace_id, &principal, package, None, &artifact_key)
                .await
                .unwrap();
        let replay = upload_package_artifact(
            db,
            workspace_id,
            &principal,
            replay_package,
            Some(&package_sha256),
            &artifact_key,
        )
        .await
        .unwrap();
        assert_eq!(replay.0, artifact_id);
        assert_eq!(replay.1, package_sha256);
        let preview = preview_package_import(
            db,
            &PreviewImportRequest {
                workspace_id,
                artifact_id,
                project_map: BTreeMap::new(),
                external_reference_policy: ExternalReferencePolicy::Detach,
                conflict_policy: ConflictPolicy::RejectExisting,
                include_history: true,
                idempotency_key: format!("preview-{label}"),
                principal: principal.clone(),
            },
        )
        .await
        .unwrap();
        let commit = CommitImportRequest {
            workspace_id,
            preview_id: preview.preview_id,
            package_sha256,
            mapping_hash: preview.mapping_hash.clone(),
            conflict_policy: ConflictPolicy::RejectExisting,
            confirm: true,
            idempotency_key: format!("commit-{label}"),
            principal,
        };
        (preview, commit)
    }

    #[tokio::test]
    async fn flow_package_import_preview_writes_no_canonical_state_and_commit_remaps_exact_document_heads() {
        if !cfg!(target_os = "linux") {
            eprintln!("NOT EXECUTED: the import validates in the isolated worker, which requires Linux");
            return;
        }
        let Some(scratch) = scratch("flow_package_import_roundtrip").await else {
            eprintln!("SKIPPED (no database): set OPENPR_TEST_DATABASE_URL to run this test");
            return;
        };
        // A scratch database models a running api and worker.
        crate::flow::scope_runtime::fixture::register_running_readers(&scratch.db).await;
        let state = state_for(scratch.db.clone());
        let (source_workspace, source_owner) = enabled_workspace(&state, "package_source").await;
        let (target_workspace, target_owner) = enabled_workspace(&state, "package_target").await;
        let package = exported_fixture(&state, source_workspace, source_owner, "roundtrip").await;
        let reuse_package = package.clone();
        let before = canonical_count(&state.db, target_workspace).await;
        let (preview, commit) = prepare_preview(&state.db, target_workspace, target_owner, package, "roundtrip").await;
        assert_eq!(canonical_count(&state.db, target_workspace).await, before);
        let mut cross_actor = commit.clone();
        cross_actor.principal = principal(Uuid::new_v4());
        assert!(
            matches!(
                commit_package_import(&state.db, &cross_actor).await,
                Err(ref e) if e.kind() == crate::error::ApiErrorKind::NotFound
            ),
            "a preview is actor-bound even when the other principal claims an admin role"
        );
        let mut cross_workspace = commit.clone();
        cross_workspace.workspace_id = source_workspace;
        assert!(
            matches!(
                commit_package_import(&state.db, &cross_workspace).await,
                Err(ref e) if e.kind() == crate::error::ApiErrorKind::NotFound
            ),
            "a preview id cannot be replayed in another workspace"
        );
        assert_ne!(
            preview.mapping.object_map.keys().collect::<Vec<_>>(),
            preview.mapping.object_map.values().collect::<Vec<_>>()
        );
        exec(
            &state.db,
            "UPDATE flow_v08_rollback_control SET import_promotion_paused=true,reason='v0.7 rollback test',changed_at=now() WHERE singleton=true",
            vec![],
        )
        .await;
        let paused = commit_package_import(&state.db, &commit)
            .await
            .expect_err("rollback pause must reject a valid frozen promotion before any canonical write");
        assert_eq!(
            paused.kind(),
            crate::error::ApiErrorKind::ServerDraining(crate::error::ServerDrainingReason::Drain)
        );
        assert_eq!(canonical_count(&state.db, target_workspace).await, before);
        exec(
            &state.db,
            "UPDATE flow_v08_rollback_control SET import_promotion_paused=false,reason=NULL,changed_at=now() WHERE singleton=true",
            vec![],
        )
        .await;
        let report = commit_package_import(&state.db, &commit).await.unwrap();
        assert_eq!(report.counts["created"], 1);
        assert_eq!(canonical_count(&state.db, target_workspace).await, before + 1);
        let dispatch = state
            .db
            .query_one(Statement::from_sql_and_values(
                DbBackend::Postgres,
                "SELECT \
                   COUNT(*) FILTER (WHERE be.event_type='flow.import.completed')::bigint AS completed, \
                   COUNT(*) FILTER (WHERE be.event_type='flow.import.previewed')::bigint AS previewed \
                 FROM event_dispatch ed JOIN business_events be ON be.id=ed.event_id WHERE be.workspace_id=$1",
                vec![target_workspace.into()],
            ))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(dispatch.try_get::<i64>("", "completed").unwrap(), 1);
        assert_eq!(dispatch.try_get::<i64>("", "previewed").unwrap(), 0);
        let replay = commit_package_import(&state.db, &commit).await.unwrap();
        assert_eq!(replay.import_id, report.import_id);
        assert_eq!(replay.audit_event_id, report.audit_event_id);

        let target_principal = principal(target_owner);
        let (reuse_artifact, reuse_sha, _) = upload_package_artifact(
            &state.db,
            target_workspace,
            &target_principal,
            reuse_package,
            None,
            "artifact-reuse",
        )
        .await
        .unwrap();
        let reuse_preview = preview_package_import(
            &state.db,
            &PreviewImportRequest {
                workspace_id: target_workspace,
                artifact_id: reuse_artifact,
                project_map: BTreeMap::new(),
                external_reference_policy: ExternalReferencePolicy::Detach,
                conflict_policy: ConflictPolicy::ReuseImportLineage,
                include_history: true,
                idempotency_key: "preview-reuse".to_string(),
                principal: target_principal.clone(),
            },
        )
        .await
        .unwrap();
        assert_eq!(reuse_preview.mapping.object_map, report.object_mapping);
        let reuse_report = commit_package_import(
            &state.db,
            &CommitImportRequest {
                workspace_id: target_workspace,
                preview_id: reuse_preview.preview_id,
                package_sha256: reuse_sha,
                mapping_hash: reuse_preview.mapping_hash,
                conflict_policy: ConflictPolicy::ReuseImportLineage,
                confirm: true,
                idempotency_key: "commit-reuse".to_string(),
                principal: target_principal,
            },
        )
        .await
        .unwrap();
        assert_eq!(reuse_report.counts["created"], 0);
        assert_eq!(reuse_report.counts["reused"], 1);
        assert_eq!(canonical_count(&state.db, target_workspace).await, before + 1);

        #[derive(FromQueryResult)]
        struct Head {
            document_id: Uuid,
            head_seq: i64,
            head_frontier: Vec<u8>,
            snapshot: Vec<u8>,
        }
        for (source_object, target_object) in &report.object_mapping {
            let source = Head::find_by_statement(Statement::from_sql_and_values(
                DbBackend::Postgres,
                "SELECT cd.id AS document_id,cd.head_seq,cd.head_frontier,cd.snapshot FROM collab_documents cd JOIN flow_objects fo ON fo.id=cd.object_id WHERE fo.id=$1",
                vec![(*source_object).into()],
            )).one(&state.db).await.unwrap().unwrap();
            let target = Head::find_by_statement(Statement::from_sql_and_values(
                DbBackend::Postgres,
                "SELECT cd.id AS document_id,cd.head_seq,cd.head_frontier,cd.snapshot FROM collab_documents cd JOIN flow_objects fo ON fo.id=cd.object_id WHERE fo.id=$1",
                vec![(*target_object).into()],
            )).one(&state.db).await.unwrap().unwrap();
            assert_eq!(target.head_seq, source.head_seq);
            assert_eq!(target.head_frontier, source.head_frontier);
            #[derive(FromQueryResult)]
            struct Update {
                bytes: Vec<u8>,
            }
            let updates = Update::find_by_statement(Statement::from_sql_and_values(
                DbBackend::Postgres,
                "SELECT bytes FROM collab_updates WHERE document_id=$1 ORDER BY seq",
                vec![source.document_id.into()],
            ))
            .all(&state.db)
            .await
            .unwrap();
            let mut source_engine = LoroCollabEngine::load(&source.snapshot).unwrap();
            for update in updates {
                source_engine.import_update(&update.bytes).unwrap();
            }
            let source_hash = source_engine.semantic_snapshot().unwrap().semantic_hash();
            let target_hash = LoroCollabEngine::load(&target.snapshot)
                .unwrap()
                .semantic_snapshot()
                .unwrap()
                .semantic_hash();
            assert_eq!(target_hash, source_hash);
        }
        let mut drift = commit.clone();
        drift.mapping_hash = "0".repeat(64);
        let drifted = commit_package_import(&state.db, &drift)
            .await
            .expect_err("a drifted commit body under a used key must be refused");
        assert_eq!(drifted.kind(), crate::error::ApiErrorKind::InvalidUpdate, "{drifted:?}");
        assert!(
            matches!(&drifted, ApiError::Typed { details: Some(details), .. }
                if details["reason"] == crate::error::IDEMPOTENCY_KEY_BODY_DRIFT_REASON),
            "{drifted:?}"
        );
        assert!(matches!(
            upload_package_artifact(
                &state.db,
                target_workspace,
                &ImportPrincipal {
                    role: "member".to_string(),
                    ..principal(target_owner)
                },
                Vec::new(),
                None,
                "artifact-denied",
            )
            .await,
            Err(ref e) if e.kind() == crate::error::ApiErrorKind::Forbidden
        ));
        scratch.drop_self().await;
    }

    #[tokio::test]
    async fn flow_package_import_promotion_fault_rolls_back_every_canonical_row_and_completion_event() {
        if !cfg!(target_os = "linux") {
            eprintln!("NOT EXECUTED: the import validates in the isolated worker, which requires Linux");
            return;
        }
        let Some(scratch) = scratch("flow_package_import_rollback").await else {
            eprintln!("SKIPPED (no database): set OPENPR_TEST_DATABASE_URL to run this test");
            return;
        };
        // A scratch database models a running api and worker.
        crate::flow::scope_runtime::fixture::register_running_readers(&scratch.db).await;
        let state = state_for(scratch.db.clone());
        let (source_workspace, source_owner) = enabled_workspace(&state, "rollback_source").await;
        let (target_workspace, target_owner) = enabled_workspace(&state, "rollback_target").await;
        let package = exported_fixture(&state, source_workspace, source_owner, "rollback").await;
        let before = canonical_count(&state.db, target_workspace).await;
        let (_, commit) = prepare_preview(&state.db, target_workspace, target_owner, package, "rollback").await;
        for point in [
            PromotionFaultPoint::JobStarted,
            PromotionFaultPoint::ObjectPromoted,
            PromotionFaultPoint::RelationsPromoted,
            PromotionFaultPoint::EventInserted,
            PromotionFaultPoint::ReportFinished,
            PromotionFaultPoint::PreviewLinked,
        ] {
            *PROMOTION_FAULT.lock() = Some((commit.preview_id, point));
            let result = commit_package_import(&state.db, &commit).await;
            *PROMOTION_FAULT.lock() = None;
            assert!(matches!(result, Err(ApiError::Internal)), "fault point {point:?}");
            assert_eq!(
                canonical_count(&state.db, target_workspace).await,
                before,
                "fault point {point:?}"
            );
            let rows = state.db.query_one(Statement::from_sql_and_values(
                DbBackend::Postgres,
                "SELECT \
                   (SELECT COUNT(*) FROM flow_import_jobs WHERE workspace_id=$1 AND kind='flow_package')::bigint AS jobs, \
                   (SELECT COUNT(*) FROM flow_import_lineage WHERE target_workspace_id=$1 AND source_kind='flow_package')::bigint AS lineage, \
                   (SELECT COUNT(*) FROM business_events WHERE workspace_id=$1 AND event_type='flow.import.completed')::bigint AS completed",
                vec![target_workspace.into()],
            )).await.unwrap().unwrap();
            assert_eq!(rows.try_get::<i64>("", "jobs").unwrap(), 0, "fault point {point:?}");
            assert_eq!(rows.try_get::<i64>("", "lineage").unwrap(), 0, "fault point {point:?}");
            assert_eq!(
                rows.try_get::<i64>("", "completed").unwrap(),
                0,
                "fault point {point:?}"
            );
        }
        scratch.drop_self().await;
    }

    /// A page whose one block is one character over `text_block_chars_max` in the snapshot of its
    /// upload, followed by a small update: a document the product cannot write (a write above the
    /// limit is refused) and an upload can carry, because nothing but the import window bounds the
    /// content of a snapshot. Returns the snapshot, the tail and the frontier of the head.
    fn oversized_page_upload() -> (Vec<u8>, Vec<u8>, Vec<u8>) {
        let mut engine = LoroCollabEngine::new_empty(1);
        engine.initialize_page_editor().unwrap();
        let id = collab_core::NodeId::from("b00000");
        engine
            .apply_content_operation(&collab_core::Operation::CreateNode {
                id: id.clone(),
                parent: None,
                index: 0,
                kind: collab_core::NodeKind::Block,
            })
            .unwrap();
        engine
            .apply_content_operation(&collab_core::Operation::InsertText {
                id,
                index: 0,
                text: "x".repeat(collab_core::limits::TEXT_BLOCK_CHARS_MAX + 1),
            })
            .unwrap();
        let snapshot = engine.export_snapshot().unwrap();
        let before = engine.frontier();
        engine.set_title("after the snapshot").unwrap();
        (
            snapshot,
            engine.export_from(&before).unwrap(),
            engine.frontier().as_bytes().to_vec(),
        )
    }

    async fn counted(db: &DatabaseConnection, table: &str) -> i64 {
        db.query_one(Statement::from_string(
            DbBackend::Postgres,
            format!("SELECT count(*)::bigint AS n FROM {table}"),
        ))
        .await
        .unwrap()
        .unwrap()
        .try_get("", "n")
        .unwrap()
    }

    /// A-2: an upload whose head is above a documented content limit is refused as `limit_exceeded`
    /// naming the limit, every time, and nothing is stored: no object, no document, no job.
    #[tokio::test]
    async fn flow_package_import_refuses_a_snapshot_over_a_content_limit_and_stores_nothing() {
        if !cfg!(target_os = "linux") {
            eprintln!("NOT EXECUTED: the import validates in the isolated worker, which requires Linux");
            return;
        }
        let Some(scratch) = scratch("flow_package_import_limit").await else {
            eprintln!("SKIPPED (no database): set OPENPR_TEST_DATABASE_URL to run this test");
            return;
        };
        crate::flow::scope_runtime::fixture::register_running_readers(&scratch.db).await;
        let state = state_for(scratch.db.clone());
        let (workspace, owner) = enabled_workspace(&state, "package_limit").await;
        let (snapshot, tail, frontier) = oversized_page_upload();
        let package = rebuilt_package_with_frontier(&snapshot, Some(&tail), Some(&frontier)).0;
        let before = canonical_count(&state.db, workspace).await;
        let (_, commit) = prepare_preview(&state.db, workspace, owner, package, "limit").await;
        for attempt in 0..2 {
            let error = commit_package_import(&state.db, &commit)
                .await
                .expect_err("an upload over a content limit must be refused");
            assert_eq!(
                error.kind(),
                crate::error::ApiErrorKind::LimitExceeded,
                "attempt {attempt}: {error:?}"
            );
            let ApiError::Typed { details, .. } = &error else {
                panic!("a typed error");
            };
            let details = details.as_ref().unwrap();
            assert_eq!(details["limit_kind"], "text_block_chars");
            assert_eq!(details["limit"], collab_core::limits::TEXT_BLOCK_CHARS_MAX);
        }
        assert_eq!(canonical_count(&state.db, workspace).await, before);
        for table in ["flow_import_jobs", "flow_import_lineage"] {
            assert_eq!(counted(&state.db, table).await, 0, "{table}");
        }
        scratch.drop_self().await;
    }

    /// The same package, valid, off Linux: the isolated worker does not run there, so the import is
    /// refused as a platform defect and never decodes the upload in the API process.
    #[tokio::test]
    async fn flow_package_import_is_refused_where_the_worker_does_not_run() {
        if cfg!(target_os = "linux") {
            eprintln!("NOT EXECUTED: this refusal is the behaviour of a build without the worker");
            return;
        }
        let Some(scratch) = scratch("flow_package_import_platform").await else {
            eprintln!("SKIPPED (no database): set OPENPR_TEST_DATABASE_URL to run this test");
            return;
        };
        crate::flow::scope_runtime::fixture::register_running_readers(&scratch.db).await;
        let state = state_for(scratch.db.clone());
        let (workspace, owner) = enabled_workspace(&state, "package_platform").await;
        let clean = std::fs::read(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../frontend/tests/fixtures/editor-doc-v1/browser.snapshot"
        ))
        .unwrap();
        let tail = {
            let doc = loro::LoroDoc::new();
            doc.import(&clean).unwrap();
            doc.set_peer_id(9_100_000).unwrap();
            let from = doc.oplog_vv();
            doc.get_map("scratch_root").insert("k", 1).unwrap();
            doc.commit();
            doc.export(loro::ExportMode::updates(&from)).unwrap()
        };
        let package = rebuilt_package(&clean, Some(&tail)).0;
        let before = canonical_count(&state.db, workspace).await;
        let (_, commit) = prepare_preview(&state.db, workspace, owner, package, "platform").await;
        let error = commit_package_import(&state.db, &commit).await.expect_err("refused");
        assert_eq!(error.kind(), crate::error::ApiErrorKind::ServerRejected, "{error:?}");
        assert!(
            format!("{error:?}").contains("isolated_apply_unsupported_platform"),
            "{error:?}"
        );
        assert_eq!(canonical_count(&state.db, workspace).await, before);
        assert_eq!(counted(&state.db, "flow_import_jobs").await, 0);
        scratch.drop_self().await;
    }
}
