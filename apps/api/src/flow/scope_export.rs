//! Accepted-only textual export jobs with real durable artifacts and current source authority.
use super::{
    collab::{authz::PermissionLevel, bootstrap::content_hash},
    scope,
};
use crate::error::ApiError;
use crate::flow::json_value::FlowJson;
use platform::timestamp::Timestamp;
use sea_orm::{ConnectionTrait, DatabaseConnection, DbBackend, FromQueryResult, Statement, TransactionTrait};
use serde_json::{Value, json};
use uuid::Uuid;
#[derive(FromQueryResult)]
pub struct ExportRow {
    pub id: Uuid,
    pub scope_id: Uuid,
    pub workspace_id: Option<Uuid>,
    pub scope_kind: String,
    pub object_id: Option<Uuid>,
    pub actor_id: Uuid,
    pub format: String,
    pub status: String,
    pub request_hash: String,
    pub package_sha256: Option<String>,
    pub size_bytes: Option<i64>,
    pub expires_at: Option<Timestamp>,
    pub artifact_id: Option<Uuid>,
    pub source_authz_revision: Option<i64>,
    pub policy_collection_id: Option<Uuid>,
    pub policy_schema_seq: Option<i64>,
    pub included_record_ids: Vec<Uuid>,
    pub audience_authorities: Value,
    pub artifact_resource_ids: Vec<Uuid>,
    pub artifact_schema_revisions: Value,
    pub warnings: Value,
}
const SELECT: &str = "SELECT id,scope_id,workspace_id,scope_kind,object_id,actor_id,format,status,request_hash,package_sha256,size_bytes,expires_at,artifact_id,source_authz_revision,policy_collection_id,policy_schema_seq,included_record_ids,audience_authorities,artifact_resource_ids,artifact_schema_revisions,warnings FROM flow_export_jobs";
pub fn receipt(row: &ExportRow) -> Value {
    json!({"job_id":row.id,"scope_id":row.scope_id,"object_id":row.object_id,"scope_kind":row.scope_kind,"status":row.status,"format":row.format,"checksum":row.package_sha256,"size":row.size_bytes,"expires_at":row.expires_at.map(platform::timestamp::Timestamp::to_rfc3339),"warnings":row.warnings,"download_url":format!("/api/v2/flow/exports/{}/artifact",row.id)})
}
async fn load<C: ConnectionTrait>(db: &C, job: Uuid) -> Result<ExportRow, ApiError> {
    ExportRow::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        format!("{SELECT} WHERE id=$1"),
        vec![job.into()],
    ))
    .one(db)
    .await?
    .ok_or_else(|| ApiError::not_found("export not found"))
}
/// The object an export job exports; `None` for a job of a whole scope.
pub async fn job_object<C: ConnectionTrait>(db: &C, job: Uuid) -> Result<Option<Uuid>, ApiError> {
    Ok(load(db, job).await?.object_id)
}
/// Whether `job` is a workspace export, judged by the workspace handlers and not by scoped access.
/// No principal is needed to tell; the handler that serves the job authorizes the caller.
pub async fn is_workspace_job<C: ConnectionTrait>(db: &C, job: Uuid) -> Result<bool, ApiError> {
    let row = load(db, job).await?;
    Ok(row.workspace_id.is_some() && !uses_scoped_access(&row))
}
pub async fn get<C: ConnectionTrait + TransactionTrait>(db: &C, job: Uuid, user: Uuid) -> Result<ExportRow, ApiError> {
    let row = load(db, job).await?;
    if row.workspace_id.is_some() && !uses_scoped_access(&row) {
        return Ok(row);
    }
    if row.actor_id != user {
        return Err(ApiError::not_found("export not found"));
    }
    let object = job_authorization_object(db, &row, user).await?;
    scope::authorize(db, object, user, PermissionLevel::View).await?;
    validate_job_policy(db, &row, user).await?;
    Ok(row)
}
pub async fn create(
    db: &DatabaseConnection,
    object: Uuid,
    user: Uuid,
    format: &str,
    at_seq: Option<i64>,
    include_history: bool,
    key: &str,
) -> Result<Value, ApiError> {
    create_inner(db, object, user, format, at_seq, include_history, key, None).await
}
pub async fn create_scope(
    db: &DatabaseConnection,
    scope_id: Uuid,
    user: Uuid,
    include_history: bool,
    key: &str,
    kinds: Option<Vec<String>>,
) -> Result<Value, ApiError> {
    let owner = scope::get(db, scope_id).await?;
    if owner.kind != "personal" || owner.owner_user_id != Some(user) {
        return Err(ApiError::not_found("personal export scope not found"));
    }
    require_scope_export(db).await?;
    let root = scope::roots(db, scope_id)
        .await?
        .into_iter()
        .find(|r| r.project_id.is_none())
        .ok_or_else(ApiError::internal)?
        .object_id;
    create_inner(
        db,
        root,
        user,
        "package",
        None,
        include_history,
        key,
        Some((scope_id, kinds)),
    )
    .await
}
async fn require_scope_export<C: ConnectionTrait>(db: &C) -> Result<(), ApiError> {
    if !scope::capabilities(db).await?.personal_scope_package_exports {
        return Err(ApiError::typed_with_details(
            crate::error::ApiErrorKind::FeatureDisabled,
            "personal scope package exports are not enabled",
            json!({"reason":"personal_scope_package_exports_disabled"}),
        ));
    }
    scope::require_reader_ready(db).await
}
async fn create_inner(
    db: &DatabaseConnection,
    object: Uuid,
    user: Uuid,
    format: &str,
    at_seq: Option<i64>,
    include_history: bool,
    key: &str,
    scope_selection: Option<(Uuid, Option<Vec<String>>)>,
) -> Result<Value, ApiError> {
    if !matches!(format, "markdown" | "json" | "csv" | "package") || (include_history && format != "package") {
        return Err(ApiError::unsupported_format(
            "personal export supports markdown,json,csv and policy-safe current-head page packages without history",
        ));
    }
    if key.trim().is_empty() || key.len() > 128 {
        return Err(ApiError::invalid_update(
            "export operation identity must contain1..128 bytes",
        ));
    }
    let identity = scope::identity(db, object).await?;
    if identity.workspace_id.is_some() && identity.object_type != "record" {
        return Err(ApiError::invalid_update(
            "legacy export requires legacy application adapter",
        ));
    }
    let proof = scope::authorize(db, object, user, PermissionLevel::View).await?;
    let body = if let Some((scope_id, kinds)) = scope_selection.as_ref() {
        let kinds = kinds.as_ref().map(|kinds| {
            let mut kinds = kinds.clone();
            kinds.sort();
            kinds.dedup();
            kinds
        });
        json!({"operation":"scope_export","scope_id":scope_id,"format":format,"include_history":include_history,"object_types":kinds})
    } else {
        json!({"operation":"object_export","object_id":object,"format":format,"at_seq":at_seq,"include_history":include_history})
    };
    let request_hash = content_hash(&serde_json::to_vec(&body).map_err(|_| ApiError::internal())?);
    let package_plan = if format == "package" {
        Some(if let Some((scope_id, kinds)) = scope_selection.as_ref() {
            super::scope_package::prepare_scope(db, *scope_id, user, kinds.as_deref()).await?
        } else {
            super::scope_package::prepare(db, object, user).await?
        })
    } else {
        None
    };
    let mut plan = prepare_export_plan(db, object, user, &identity.object_type, proof).await?;
    if let Some(package) = package_plan.as_ref() {
        plan.proofs = package.proofs.clone();
        let mut audiences = std::collections::BTreeMap::new();
        for proof in &plan.proofs {
            for (id, revision) in &proof.audience_revisions {
                if audiences.insert(*id, *revision).is_some_and(|old| old != *revision) {
                    return Err(super::policy::authorization_read_unstable());
                }
            }
        }
        plan.audiences=json!(audiences.into_iter().map(|(workspace_id,authz_revision)|json!({"workspace_id":workspace_id,"authz_revision":authz_revision})).collect::<Vec<_>>());
    }
    let tx = db.begin().await?;
    if scope_selection.is_some() {
        require_scope_export(&tx).await?;
    }
    scope::fence_many(&tx, &plan.proofs).await?;
    tx.execute(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "SELECT pg_advisory_xact_lock(hashtextextended($1,0))",
        vec![format!("export:{}:{user}:{key}", identity.scope_id).into()],
    ))
    .await?;
    if let Some(row) = ExportRow::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        format!("{SELECT} WHERE scope_id=$1 AND actor_kind='user' AND actor_id=$2 AND idempotency_key=$3"),
        vec![identity.scope_id.into(), user.into(), key.into()],
    ))
    .one(&tx)
    .await?
    {
        if row.request_hash != request_hash {
            if row.workspace_id.is_some()
                && !uses_scoped_access(&row)
                && row.request_hash
                    == super::export::legacy_object_request_hash(object, format, at_seq, include_history)?
            {
                let result = receipt(&row);
                tx.commit().await?;
                return Ok(result);
            }
            return Err(ApiError::idempotency_key_body_drift("export request changed"));
        }
        validate_job_policy(&tx, &row, user).await?;
        let result = receipt(&row);
        tx.commit().await?;
        return Ok(result);
    }
    let mut view = scope::object_view(&tx, object, user).await?;
    if matches!(identity.object_type.as_str(), "page" | "navigator") {
        let document = Uuid::parse_str(view.field("document_id").as_str().ok_or_else(ApiError::internal)?)
            .map_err(|_| ApiError::internal())?;
        let expected = view
            .field("frontier")
            .as_str()
            .ok_or_else(ApiError::internal)?
            .to_string();
        if let Some(engine) =
            super::projection::refresh_editor_state(db, document, &expected, view.field_mut("semantic_content")?)
                .await?
        {
            *view.field_mut("title")? = json!(engine.title().map_err(|e| super::command::map_collab_error(&e))?);
        }
    }
    let seq = view.field("document_seq").as_i64().ok_or_else(ApiError::internal)?;
    if at_seq.is_some_and(|at| at != seq) {
        return Err(ApiError::unsupported_protocol(
            "historical personal export is unavailable; use the accepted head",
        ));
    }
    let bytes = if let Some(package) = package_plan.as_ref() {
        super::scope_package::render(db, &tx, package, user, include_history).await?
    } else if matches!(identity.object_type.as_str(), "collection" | "record") {
        render_typed_export(&tx, object, user, &identity.object_type, format, &plan).await?
    } else {
        let semantic: collab_core::SemanticSnapshot =
            serde_json::from_value(view.field("semantic_content").clone()).map_err(|_| ApiError::internal())?;
        let plain_text = super::projection::plain_text(&semantic);

        super::export::render_object_text_export(
            object,
            view.field("object_type").as_str().ok_or_else(ApiError::internal)?,
            view.field("title").as_str().unwrap_or(""),
            view.field("semantic_content"),
            seq,
            view.field("frontier").as_str().ok_or_else(ApiError::internal)?,
            &plain_text,
            format,
        )?
    };
    let hash = content_hash(&bytes);
    let size = i64::try_from(bytes.len()).map_err(|_| ApiError::invalid_update("export artifact too large"))?;
    let artifact = Uuid::new_v4();
    let job = Uuid::new_v4();
    let expiry = Timestamp::now() + time::Duration::hours(24);
    tx.execute(Statement::from_sql_and_values(DbBackend::Postgres,"INSERT INTO flow_package_artifacts(id,workspace_id,scope_id,actor_kind,actor_id,purpose,package_sha256,size_bytes,package_bytes,expires_at) VALUES($1,$8,$2,'user',$3,'export',$4,$5,$6,$7)",vec![artifact.into(),identity.scope_id.into(),user.into(),hash.clone().into(),size.into(),bytes.into(),expiry.into(),identity.workspace_id.into()])).await?;
    tx.execute(Statement::from_sql_and_values(DbBackend::Postgres,"INSERT INTO flow_export_jobs(id,workspace_id,scope_id,object_id,scope_kind,format,at_seq,include_history,actor_kind,actor_id,idempotency_key,request_hash,status,artifact_id,package_sha256,size_bytes,expires_at) VALUES($1,$15,$2,$3,$14,$4,$5,$13,'user',$6,$7,$8,'completed',$9,$10,$11,$12)",vec![job.into(),identity.scope_id.into(),if scope_selection.is_some(){None}else{Some(object)}.into(),format.into(),at_seq.into(),user.into(),key.into(),request_hash.into(),artifact.into(),hash.into(),size.into(),expiry.into(),include_history.into(),if scope_selection.is_some(){"scope"}else{"object"}.into(),identity.workspace_id.into()])).await?;
    tx.execute(Statement::from_sql_and_values(DbBackend::Postgres,"UPDATE flow_export_jobs SET source_authz_revision=$2,policy_collection_id=$3,policy_schema_seq=$4,included_record_ids=$5,audience_authorities=$6 WHERE id=$1",vec![job.into(),plan.proofs.first().map(|p|p.revision).into(),plan.collection.into(),plan.schema_seq.into(),plan.records.clone().into(),plan.audiences.clone().into()])).await?;
    if package_plan.is_some() {
        tx.execute(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "UPDATE flow_export_jobs SET artifact_resource_ids=$2 WHERE id=$1",
            vec![
                job.into(),
                plan.proofs.iter().map(|p| p.object_id).collect::<Vec<_>>().into(),
            ],
        ))
        .await?;
    }
    if let Some(package) = package_plan.as_ref() {
        let mut warnings = package.warnings.clone();
        if include_history {
            warnings.push("non_content_history_events_omitted".into());
        }
        tx.execute(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "UPDATE flow_export_jobs SET warnings=$2 WHERE id=$1",
            vec![job.into(), json!(warnings).into()],
        ))
        .await?;
    }
    if let Some(fingerprint) = plan.record_fingerprint.as_ref() {
        tx.execute(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "UPDATE flow_export_jobs SET artifact_schema_revisions=$2 WHERE id=$1",
            vec![
                job.into(),
                json!([{"record_id":object,"record_context_version":1,"schema_fingerprint":fingerprint}]).into(),
            ],
        ))
        .await?;
    }
    let result = receipt(&get(&tx, job, user).await?);
    tx.commit().await?;
    Ok(result)
}
pub async fn download(db: &DatabaseConnection, job: Uuid, user: Uuid) -> Result<(Vec<u8>, String, String), ApiError> {
    let row = get(db, job, user).await?;
    let object = job_authorization_object(db, &row, user).await?;
    let proof = scope::authorize(db, object, user, PermissionLevel::View).await?;
    if row.expires_at.is_none_or(|t| t <= Timestamp::now()) {
        return Err(ApiError::not_found("export artifact expired"));
    }
    let artifact=db.query_one(Statement::from_sql_and_values(DbBackend::Postgres,"SELECT package_bytes,package_sha256 FROM flow_package_artifacts WHERE id=$1 AND scope_id=$2 AND actor_id=$3 AND expires_at>now()",vec![row.artifact_id.into(),row.scope_id.into(),user.into()])).await?.ok_or_else(||ApiError::not_found("export artifact not found"))?;
    let bytes: Vec<u8> = artifact.try_get("", "package_bytes")?;
    let hash: String = artifact.try_get("", "package_sha256")?;
    if hash != content_hash(&bytes) {
        return Err(ApiError::checksum_mismatch("export artifact checksum mismatch"));
    }
    validate_job_policy(db, &row, user).await?;
    if !scope::proof_current(db, &proof).await? {
        return Err(super::policy::authorization_read_unstable());
    }
    Ok((bytes, hash, row.format))
}

async fn render_typed_export(
    tx: &sea_orm::DatabaseTransaction,
    object: Uuid,
    user: Uuid,
    kind: &str,
    format: &str,
    plan: &ExportPlan,
) -> Result<Vec<u8>, ApiError> {
    use std::collections::BTreeSet;
    if kind == "record" {
        // No Record head is held before this call: shared helper acquires parent schema,
        // then Record head/projection, in the same outer transaction/savepoint.
        let context = super::scope_record_context::read_for_export(tx, object, user).await?;
        if Some(context.schema_fingerprint()) != plan.record_fingerprint {
            return Err(ApiError::policy_rejected(
                "record export schema changed during preparation",
            ));
        }
        return render_record_context(context, format);
    }
    let collection = if kind == "collection" {
        object
    } else {
        super::collections::owning_collection_id(tx, object)
            .await?
            .ok_or_else(|| ApiError::not_found("owning Collection not found"))?
    };
    scope::authorize(tx, collection, user, PermissionLevel::View).await?;
    // This is the same Collection-projection→ordered-document lock rank as typed commands and
    // rebuilds. It creates one stable accepted export without blocking an authorization revoke
    // after the transaction ends; download reauthorizes independently.
    tx.execute(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "SELECT collection_id FROM flow_collection_projections WHERE collection_id=$1 FOR SHARE",
        vec![collection.into()],
    ))
    .await?;
    let current = super::collections::collection_read_row(tx, collection).await?;
    if Some(current.schema_seq) != plan.schema_seq {
        return Err(ApiError::policy_rejected(
            "export field policy changed during preparation; retry",
        ));
    }

    let records = plan.records.clone();
    let documents=tx.query_all(Statement::from_sql_and_values(DbBackend::Postgres,"SELECT document_id FROM flow_record_projections WHERE record_id=ANY($1) AND collection_id=$2 ORDER BY document_id",vec![records.clone().into(),collection.into()])).await?.into_iter().map(|row|row.try_get::<Uuid>("","document_id")).collect::<Result<Vec<_>,_>>()?;
    if !documents.is_empty() {
        tx.query_all(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT id FROM collab_documents WHERE id=ANY($1) ORDER BY id FOR SHARE",
            vec![documents.into()],
        ))
        .await?;
    }
    let fields = super::collections::collection_fields(tx, collection, false)
        .await?
        .into_iter()
        .filter(|field| !super::collections::field_is_restricted(field))
        .collect::<Vec<_>>();
    let allowed = fields
        .iter()
        .map(|field| field.field_id.to_string())
        .collect::<BTreeSet<_>>();
    let mut items = Vec::new();
    for record in records {
        let view = match scope::object_view(tx, record, user).await {
            Ok(v) => v,
            Err(err) if err.kind() == crate::error::ApiErrorKind::NotFound => continue,
            Err(err) => return Err(err),
        };
        let row = tx
            .query_one(Statement::from_sql_and_values(
                DbBackend::Postgres,
                "SELECT properties FROM flow_record_projections WHERE record_id=$1 AND collection_id=$2",
                vec![record.into(), collection.into()],
            ))
            .await?
            .ok_or_else(ApiError::internal)?;
        let values: Value = row.try_get("", "properties")?;
        let values = values
            .as_object()
            .ok_or_else(ApiError::internal)?
            .iter()
            .filter(|(key, _)| allowed.contains(*key))
            .map(|(key, value)| (key.clone(), value.clone()))
            .collect::<serde_json::Map<_, _>>();
        items.push(json!({"record":view,"values_by_field_id":values}));
    }
    match format {
        "json" => serde_json::to_vec(&json!({"object_id":object,"object_type":kind,"fields":fields,"items":items}))
            .map_err(|_| ApiError::internal()),
        "csv" => {
            let mut rows = Vec::new();
            rows.push(
                std::iter::once("record_id".to_string())
                    .chain(fields.iter().map(|f| f.label.clone()))
                    .map(|cell| super::export::csv_cell(&cell))
                    .collect::<Vec<_>>()
                    .join(","),
            );
            for item in items {
                let mut cells = vec![item.field("record").field("id").as_str().unwrap_or("").to_string()];
                for field in &fields {
                    let value = &item.field("values_by_field_id")[field.field_id.to_string()];
                    cells.push(if value.is_null() {
                        String::new()
                    } else {
                        value.as_str().map_or_else(|| value.to_string(), str::to_string)
                    });
                }
                rows.push(
                    cells
                        .iter()
                        .map(|cell| super::export::csv_cell(cell))
                        .collect::<Vec<_>>()
                        .join(","),
                );
            }
            Ok((rows.join("\n") + "\n").into_bytes())
        }
        "markdown" => {
            let cell = |raw: &str| raw.replace('|', "\\|").replace('\n', "<br>");
            let mut output = format!(
                "| Record | {} |\n| --- | {} |\n",
                fields.iter().map(|f| cell(&f.label)).collect::<Vec<_>>().join(" | "),
                fields.iter().map(|_| "---").collect::<Vec<_>>().join(" | ")
            );
            for item in items {
                let id = item.field("record").field("id").as_str().unwrap_or("");
                let values = fields
                    .iter()
                    .map(|field| {
                        let value = &item.field("values_by_field_id")[field.field_id.to_string()];
                        cell(&if value.is_null() {
                            String::new()
                        } else {
                            value.as_str().map_or_else(|| value.to_string(), str::to_string)
                        })
                    })
                    .collect::<Vec<_>>();
                std::fmt::Write::write_fmt(&mut output, format_args!("| {id} | {} |\n", values.join(" | ")))
                    .map_err(|_| ApiError::internal())?;
            }
            Ok(output.into_bytes())
        }
        _ => Err(ApiError::unsupported_format(
            "typed text export supports json,markdown,csv",
        )),
    }
}

struct ExportPlan {
    proofs: Vec<scope::AuthorizationProof>,
    collection: Option<Uuid>,
    schema_seq: Option<i64>,
    records: Vec<Uuid>,
    audiences: Value,
    record_fingerprint: Option<String>,
}
async fn prepare_export_plan(
    db: &DatabaseConnection,
    object: Uuid,
    user: Uuid,
    kind: &str,
    proof: scope::AuthorizationProof,
) -> Result<ExportPlan, ApiError> {
    let mut proofs = vec![proof];
    let mut collection = None;
    let mut schema_seq = None;
    let mut records = Vec::new();
    let mut record_fingerprint = None;
    if kind == "record" {
        let context = super::scope_record_context::read_for_export(db, object, user).await?;
        let parent = context.parent_collection_id();
        collection = Some(parent);
        schema_seq = Some(super::collections::collection_read_row(db, parent).await?.schema_seq);
        record_fingerprint = Some(context.schema_fingerprint());
        records.push(object);
    }

    if kind == "collection" {
        let id = if kind == "collection" {
            object
        } else {
            super::collections::owning_collection_id(db, object)
                .await?
                .ok_or_else(|| ApiError::not_found("owning Collection not found"))?
        };
        collection = Some(id);
        schema_seq = Some(super::collections::collection_read_row(db, id).await?.schema_seq);
        proofs.push(scope::authorize(db, id, user, PermissionLevel::View).await?);
        let rows=db.query_all(Statement::from_sql_and_values(DbBackend::Postgres,"SELECT r.record_id FROM flow_record_projections r JOIN flow_objects o ON o.id=r.record_id WHERE r.collection_id=$1 AND o.lifecycle_status='active' AND ($2::uuid IS NULL OR r.record_id=$2) ORDER BY r.record_id LIMIT 10001",vec![id.into(),if kind=="record"{Some(object)}else{None}.into()])).await?;
        if rows.len() > 10_000 {
            return Err(ApiError::limit_exceeded(
                "typed export scan budget exhausted",
                "scan_budget",
                Some(json!(10_000)),
                Some(json!(rows.len())),
                None,
            ));
        }
        for row in rows {
            let record: Uuid = row.try_get("", "record_id")?;
            match scope::authorize(db, record, user, PermissionLevel::View).await {
                Ok(proof) => {
                    records.push(record);
                    proofs.push(proof);
                }
                Err(err) if err.kind() == crate::error::ApiErrorKind::NotFound => {}
                Err(err) => return Err(err),
            }
        }
    }
    let mut audiences = std::collections::BTreeMap::new();
    for proof in &proofs {
        for (id, revision) in &proof.audience_revisions {
            if let Some(previous) = audiences.insert(*id, *revision)
                && previous != *revision
            {
                return Err(ApiError::policy_rejected("export audience changed during preparation"));
            }
        }
    }
    let audiences = json!(
        audiences
            .into_iter()
            .map(|(workspace_id, authz_revision)| json!({"workspace_id":workspace_id,"authz_revision":authz_revision}))
            .collect::<Vec<_>>()
    );
    Ok(ExportPlan {
        proofs,
        collection,
        schema_seq,
        records,
        audiences,
        record_fingerprint,
    })
}
async fn validate_job_policy<C: ConnectionTrait + TransactionTrait>(
    db: &C,
    job: &ExportRow,
    user: Uuid,
) -> Result<(), ApiError> {
    let changed = || {
        ApiError::policy_rejected_with_details(
            "export authorization or field policy changed; create a new export request",
            json!({"reason":"export_authority_changed","new_request_required":true,"action":"regenerate_export"}),
        )
    };
    // Never stamp current authority onto old bytes. Old personal artifacts without an exact
    // creation proof remain retained, but require a fresh export request before downloading.
    if job.source_authz_revision.is_none() {
        return Err(changed());
    }
    if let Some(revision) = job.source_authz_revision
        && scope::revision(db, job.scope_id).await? != revision
    {
        return Err(changed());
    }
    if job.format == "package" && job.artifact_resource_ids.is_empty() {
        return Err(changed());
    }
    for object in &job.artifact_resource_ids {
        scope::authorize(db, *object, user, PermissionLevel::View)
            .await
            .map_err(|_| changed())?;
    }
    if let Some(fingerprint) = record_fingerprint(job) {
        let record = job.object_id.ok_or_else(changed)?;
        let context = super::scope_record_context::read_for_export(db, record, user)
            .await
            .map_err(|_| changed())?;
        if Some(context.parent_collection_id()) != job.policy_collection_id
            || context.schema_fingerprint() != fingerprint
        {
            return Err(changed());
        }
    } else if let Some(collection) = job.policy_collection_id {
        if Some(
            super::collections::collection_read_row(db, collection)
                .await?
                .schema_seq,
        ) != job.policy_schema_seq
        {
            return Err(changed());
        }
        scope::authorize(db, collection, user, PermissionLevel::View)
            .await
            .map_err(|_| changed())?;
    }
    if let Some(audiences) = job.audience_authorities.as_array() {
        for audience in audiences {
            let id = audience
                .get("workspace_id")
                .and_then(Value::as_str)
                .and_then(|id| Uuid::parse_str(id).ok())
                .ok_or_else(changed)?;
            let revision = audience
                .get("authz_revision")
                .and_then(Value::as_i64)
                .ok_or_else(changed)?;
            if super::collab::authz::read_epoch(db, id).await.map_err(|_| changed())? != revision {
                return Err(changed());
            }
        }
    }
    for record in &job.included_record_ids {
        scope::authorize(db, *record, user, PermissionLevel::View)
            .await
            .map_err(|_| changed())?;
    }
    Ok(())
}

async fn job_authorization_object<C: ConnectionTrait>(db: &C, row: &ExportRow, user: Uuid) -> Result<Uuid, ApiError> {
    if let Some(object) = row.object_id {
        return Ok(object);
    }
    let scope = scope::get(db, row.scope_id).await?;
    if row.scope_kind != "scope" || scope.kind != "personal" || scope.owner_user_id != Some(user) {
        return Err(ApiError::not_found("export job not found"));
    }
    scope::roots(db, row.scope_id)
        .await?
        .into_iter()
        .find(|r| r.project_id.is_none())
        .map(|r| r.object_id)
        .ok_or_else(ApiError::internal)
}

fn record_fingerprint(row: &ExportRow) -> Option<&str> {
    row.artifact_schema_revisions
        .as_array()?
        .iter()
        .find(|entry| {
            entry.get("record_context_version").and_then(Value::as_i64) == Some(1)
                && entry
                    .get("record_id")
                    .and_then(Value::as_str)
                    .and_then(|id| Uuid::parse_str(id).ok())
                    == row.object_id
        })?
        .get("schema_fingerprint")?
        .as_str()
}
pub fn uses_scoped_access(row: &ExportRow) -> bool {
    row.workspace_id.is_none() || record_fingerprint(row).is_some()
}
fn render_record_context(
    context: super::scope_record_context::AuthorizedRecordRead,
    format: &str,
) -> Result<Vec<u8>, ApiError> {
    let text = |value: &Value| {
        if value.is_null() {
            String::new()
        } else {
            value.as_str().map_or_else(|| value.to_string(), str::to_owned)
        }
    };
    match format {
        "json"=>serde_json::to_vec(&json!({"record":context.record,"fields":context.fields,"values_by_field_id":context.values_by_field_id,"field_secrecy_enabled":context.field_secrecy_enabled})).map_err(|_|ApiError::internal()),
        "csv"=>{let mut headers=vec!["record_id".to_string()];headers.extend(context.fields.iter().map(|field|field.label.clone()));let mut cells=vec![context.record.field("id").as_str().unwrap_or("").to_string()];cells.extend(context.fields.iter().map(|field|text(&context.values_by_field_id.get(&field.field_id.to_string()).cloned().unwrap_or(Value::Null))));let line=|cells:Vec<String>|cells.iter().map(|cell|format!("\"{}\"",cell.replace('\"',"\"\""))).collect::<Vec<_>>().join(",");Ok(format!("{}\n{}\n",line(headers),line(cells)).into_bytes())},
        "markdown"=>{let cell=|value:String|value.replace('|',"\\|").replace('\n'," ");let mut output=format!("# {}\n\n",cell(context.record.field("title").as_str().unwrap_or("").into()));for field in context.fields{std::fmt::Write::write_fmt(&mut output,format_args!("- **{}**: {}\n",cell(field.label),cell(text(&context.values_by_field_id.get(&field.field_id.to_string()).cloned().unwrap_or(Value::Null))))).map_err(|_|ApiError::internal())?;}Ok(output.into_bytes())},
        _=>Err(super::package::typed_raw_refusal(&["record".into()]))
    }
}
