//! Scope-native package assembly shares the strict ZIP64/checksum codec with v1.
//! Raw typed snapshots require a separate field-policy-safe format and are refused here.
use super::{
    collab::authz::PermissionLevel,
    package::{
        ENGINE_CRATE_VERSION, ENGINE_NAME, ENGINE_WIRE_FORMAT_VERSION, ExportPackageManifest, ExportPolicy,
        PackageCounts, PackageEngine, PackageHistory, PackageMemberInput, PackageProducer, PackageSource,
        SCOPE_PACKAGE_SCHEMA, build_package,
    },
    scope,
};
use crate::error::ApiError;
use crate::flow::json_value::FlowJson;
use base64::{Engine, engine::general_purpose::STANDARD};
use collab_core::CollabEngine;
use sea_orm::{ConnectionTrait, DatabaseConnection, DatabaseTransaction, DbBackend, Statement};
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};
use uuid::Uuid;
pub struct Plan {
    pub objects: Vec<Uuid>,
    pub proofs: Vec<scope::AuthorizationProof>,
    pub scope_id: Uuid,
    pub roots: Vec<Uuid>,
    pub selection: &'static str,
    pub warnings: Vec<String>,
}
pub async fn prepare(db: &DatabaseConnection, root: Uuid, user: Uuid) -> Result<Plan, ApiError> {
    let source = scope::identity(db, root).await?;
    db.query_one(Statement::from_sql_and_values(DbBackend::Postgres,"SELECT id FROM flow_objects WHERE id=$1 AND NOT flow_is_system_navigator_root(object_type,parent_id,governance_metadata)",vec![root.into()])).await?.ok_or_else(||ApiError::not_found("package object not found"))?;
    prepare_selected(db, source.scope_id, user, vec![root], vec![root], "object", Vec::new()).await
}
pub async fn prepare_scope(
    db: &DatabaseConnection,
    scope_id: Uuid,
    user: Uuid,
    kinds: Option<&[String]>,
) -> Result<Plan, ApiError> {
    let scope = scope::get(db, scope_id).await?;
    if scope.kind != "personal" || scope.owner_user_id != Some(user) {
        return Err(ApiError::not_found("personal export scope not found"));
    }
    if kinds.is_some_and(|k| k.is_empty() || k.iter().any(|v| !matches!(v.as_str(), "page" | "navigator"))) {
        return Err(ApiError::invalid_update_with_details(
            "object_types must select Page/Navigator kinds",
            json!({"reason":"invalid_package_object_types"}),
        ));
    }
    let rows=db.query_all(Statement::from_sql_and_values(DbBackend::Postgres,"SELECT id,parent_id,object_type FROM flow_objects WHERE scope_id=$1 AND NOT flow_is_system_navigator_root(object_type,parent_id,governance_metadata) ORDER BY id LIMIT 10001",vec![scope_id.into()])).await?;
    if rows.len() > 10000 {
        return Err(ApiError::limit_exceeded(
            "scope export scan budget exhausted",
            "scan_budget",
            Some(json!(10000)),
            Some(json!(rows.len())),
            None,
        ));
    }
    let mut objects = Vec::new();
    let mut parents = BTreeMap::new();
    let mut omitted = BTreeSet::new();
    for row in rows {
        let kind = row.try_get::<String>("", "object_type")?;
        if kinds.is_some_and(|k| !k.contains(&kind)) {
            omitted.insert(kind);
            continue;
        }
        let id = row.try_get::<Uuid>("", "id")?;
        parents.insert(id, row.try_get::<Option<Uuid>>("", "parent_id")?);
        objects.push(id);
    }
    if objects.is_empty() {
        return Err(ApiError::not_found("scope export selection is empty"));
    }
    let selected = objects.iter().copied().collect::<BTreeSet<_>>();
    let roots = objects
        .iter()
        .filter(|id| parents.get(id).and_then(|p| *p).is_none_or(|p| !selected.contains(&p)))
        .copied()
        .collect();
    let mut warnings = Vec::new();
    if let Some(kinds) = kinds {
        let mut kinds = kinds.to_vec();
        kinds.sort();
        kinds.dedup();
        warnings.push(format!("object_type_filter_applied:{}", kinds.join(",")));
        if !omitted.is_empty() {
            warnings.push(format!(
                "object_types_omitted:{}",
                omitted.into_iter().collect::<Vec<_>>().join(",")
            ));
        }
    }
    prepare_selected(db, scope_id, user, objects, roots, "scope", warnings).await
}
async fn prepare_selected(
    db: &DatabaseConnection,
    scope_id: Uuid,
    user: Uuid,
    objects: Vec<Uuid>,
    roots: Vec<Uuid>,
    selection: &'static str,
    warnings: Vec<String>,
) -> Result<Plan, ApiError> {
    let mut proofs = Vec::new();
    let mut unsupported = Vec::new();
    for object in &objects {
        let identity = scope::identity(db, *object).await?;
        if matches!(identity.object_type.as_str(), "collection" | "record") {
            unsupported.push(identity.object_type);
        }
        proofs.push(scope::authorize(db, *object, user, PermissionLevel::View).await?);
    }
    if !unsupported.is_empty() {
        return Err(super::package::typed_raw_refusal(&unsupported));
    }
    let endpoints=db.query_all(Statement::from_sql_and_values(DbBackend::Postgres,"SELECT source_object_id,target_object_id FROM flow_relations WHERE scope_id=$1 AND (source_object_id=ANY($2) OR target_object_id=ANY($2)) ORDER BY id LIMIT 10001",vec![scope_id.into(),objects.clone().into()])).await?;
    if endpoints.len() > 10000 {
        return Err(ApiError::limit_exceeded(
            "package relation scan budget exhausted",
            "scan_budget",
            Some(json!(10000)),
            Some(json!(endpoints.len())),
            None,
        ));
    }
    let mut known = objects.iter().copied().collect::<BTreeSet<_>>();
    for row in endpoints {
        for column in ["source_object_id", "target_object_id"] {
            let id = row.try_get::<Uuid>("", column)?;
            if known.insert(id) {
                proofs.push(scope::authorize(db, id, user, PermissionLevel::View).await?);
            }
        }
    }
    Ok(Plan {
        objects,
        proofs,
        scope_id,
        roots,
        selection,
        warnings,
    })
}
pub async fn render(
    db: &DatabaseConnection,
    tx: &DatabaseTransaction,
    plan: &Plan,
    user: Uuid,
    include_history: bool,
) -> Result<Vec<u8>, ApiError> {
    // All document heads are locked in one order before loading snapshots through the common loader.
    tx.query_all(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "SELECT id FROM collab_documents WHERE object_id=ANY($1) ORDER BY id FOR SHARE",
        vec![plan.objects.clone().into()],
    ))
    .await?;
    let selected = plan.objects.iter().copied().collect::<BTreeSet<_>>();
    let mut members = Vec::new();
    let mut through = BTreeMap::new();
    let mut bytes_seen = 0usize;
    let mut update_count = 0u64;
    let mut warnings = plan.warnings.clone();
    for object in &plan.objects {
        let view = scope::object_view(tx, *object, user).await?;
        let doc = Uuid::parse_str(view.field("document_id").as_str().ok_or_else(ApiError::internal)?)
            .map_err(|_| ApiError::internal())?;
        let boot = super::collab::bootstrap::load(db, doc).await?;
        super::collab::bootstrap::check_bootstrap_decoded_bytes(
            u64::try_from(
                boot.snapshot
                    .len()
                    .saturating_add(boot.tail_updates.iter().map(|u| u.bytes.len()).sum()),
            )
            .unwrap_or(u64::MAX),
        )?;
        let mut engine =
            collab_core::LoroCollabEngine::load(&boot.snapshot).map_err(|e| super::command::map_collab_error(&e))?;
        for update in &boot.tail_updates {
            engine
                .import_update(&update.bytes)
                .map_err(|e| super::command::map_collab_error(&e))?;
        }
        super::package::require_supported_snapshot(&engine)?;
        let snapshot = if include_history {
            boot.snapshot.clone()
        } else {
            engine
                .export_snapshot()
                .map_err(|e| super::command::map_collab_error(&e))?
        };
        bytes_seen = bytes_seen.saturating_add(snapshot.len());
        if bytes_seen as u64 > super::collab::limits::IMPORT_ARCHIVE_BYTES_MAX {
            return Err(ApiError::limit_exceeded(
                "package output budget exhausted",
                "import_archive_bytes",
                Some(json!(super::collab::limits::IMPORT_ARCHIVE_BYTES_MAX)),
                Some(json!(bytes_seen)),
                None,
            ));
        }
        let seq = view.field("document_seq").as_i64().ok_or_else(ApiError::internal)?;
        if STANDARD.encode(engine.frontier().as_bytes()) != view.field("frontier").as_str().unwrap_or("") {
            return Err(ApiError::resync_required(
                "package document head changed during assembly",
                None,
            ));
        }
        let parent = view
            .field("parent_id")
            .as_str()
            .and_then(|v| Uuid::parse_str(v).ok())
            .filter(|id| selected.contains(id));
        let object_json = json!({"source_object_id":object,"source_scope_id":plan.scope_id,"object_type":view.field("object_type"),"lifecycle_status":view.field("lifecycle_status"),"project_id":null,"parent_object_id":parent,"governance_metadata":view.field("governance_metadata"),"appearance":view.field("appearance"),"source_document_id":doc,"engine":ENGINE_NAME,"format_version":"loro-1","accepted_seq":seq,"accepted_frontier":view.field("frontier"),"semantic_hash":engine.semantic_snapshot().map_err(|e|super::command::map_collab_error(&e))?.semantic_hash(),"snapshot_sha256":super::collab::bootstrap::content_hash(&snapshot),"projection_seq":view.field("projection_seq"),"created_at":view.field("created_at"),"updated_at":view.field("updated_at"),"archived_at":view.field("archived_at")});
        members.push(PackageMemberInput {
            path: format!("objects/{object}/object.json"),
            kind: "object".into(),
            bytes: serde_jcs::to_vec(&object_json).map_err(|_| ApiError::internal())?,
        });
        members.push(PackageMemberInput {
            path: format!("documents/{doc}/snapshot.bin"),
            kind: "snapshot".into(),
            bytes: snapshot,
        });
        if include_history {
            for update in boot.tail_updates {
                update_count += 1;
                members.push(PackageMemberInput {
                    path: format!("documents/{doc}/updates/{}.bin", update.seq),
                    kind: "update".into(),
                    bytes: update.bytes,
                });
            }
        }
        through.insert(doc.to_string(), seq);
    }
    let rows=tx.query_all(Statement::from_sql_and_values(DbBackend::Postgres,"SELECT id,relation_type,source_object_id,target_object_id,position_key,properties FROM flow_relations WHERE scope_id=$1 AND (source_object_id=ANY($2) OR target_object_id=ANY($2)) ORDER BY id LIMIT 10001",vec![plan.scope_id.into(),plan.objects.clone().into()])).await?;
    if rows.len() > 10000 {
        return Err(ApiError::invalid_update(
            "package relation set changed during preparation",
        ));
    }
    let mut relations = Vec::new();
    for row in &rows {
        let source = row.try_get::<Uuid>("", "source_object_id")?;
        let target = row.try_get::<Uuid>("", "target_object_id")?;
        for endpoint in [source, target] {
            scope::authorize(tx, endpoint, user, PermissionLevel::View).await?;
            if !plan.proofs.iter().any(|p| p.object_id == endpoint) {
                return Err(ApiError::policy_rejected(
                    "package reference set changed during preparation",
                ));
            }
        }
        let value = json!({"source_relation_id":row.try_get::<Uuid>("","id")?,"relation_type":row.try_get::<String>("","relation_type")?,"source":{"kind":if selected.contains(&source){"internal"}else{"external"},"source_object_id":source},"target":{"kind":if selected.contains(&target){"internal"}else{"external"},"source_object_id":target},"position_key":row.try_get::<String>("","position_key")?,"properties":row.try_get::<Value>("","properties")?});
        relations.extend(serde_jcs::to_vec(&value).map_err(|_| ApiError::internal())?);
        relations.push(b'\n');
    }
    members.push(PackageMemberInput {
        path: "relations/relations.jsonl".into(),
        kind: "relation".into(),
        bytes: relations,
    });
    members.push(PackageMemberInput {
        path: "lineage/lineage.jsonl".into(),
        kind: "lineage".into(),
        bytes: Vec::new(),
    });
    let mut event_count = 0u64;
    if include_history {
        // Only accepted content events belong to this v2 content-history stream. Private
        // grants/uploads/Forms metadata is not promoted into a recipient's history implicitly.
        warnings.push("non_content_history_events_omitted".into());
        let events=tx.query_all(Statement::from_sql_and_values(DbBackend::Postgres,"SELECT DISTINCT e.id,e.event_type,e.aggregate_type,e.aggregate_id,e.actor_id,e.source,e.payload,e.metadata,e.created_at FROM business_events e JOIN collab_updates u ON u.event_id=e.id JOIN collab_documents d ON d.id=u.document_id WHERE e.source_flow_scope_id=$1 AND d.object_id=ANY($2) ORDER BY e.created_at,e.id",vec![plan.scope_id.into(),plan.objects.clone().into()])).await?;
        let mut lines = Vec::new();
        for event in events {
            let kind = event.try_get::<String>("", "event_type")?;
            if super::event_policy::flow_event_payload_policy(&kind).is_none() {
                continue;
            }
            let source = event.try_get::<Value>("", "source")?;
            let value = json!({"source_event_id":event.try_get::<Uuid>("","id")?,"event_type":kind,"aggregate_type":event.try_get::<String>("","aggregate_type")?,"aggregate_id":event.try_get::<String>("","aggregate_id")?,"actor":event.try_get::<Option<Uuid>>("","actor_id")?,"origin":source.get("surface"),"payload":super::event_policy::redact_flow_event_payload_for_delivery(&kind,&event.try_get::<Value>("","payload")?),"metadata":super::event_policy::redact_flow_event_metadata_for_delivery(&event.try_get::<Value>("","metadata")?),"created_at":event.try_get::<chrono::DateTime<chrono::Utc>>("","created_at")?.to_rfc3339()});
            lines.extend(serde_jcs::to_vec(&value).map_err(|_| ApiError::internal())?);
            lines.push(b'\n');
            event_count += 1;
        }
        members.push(PackageMemberInput {
            path: "history/events.jsonl".into(),
            kind: "event".into(),
            bytes: lines,
        });
    }
    let now = chrono::Utc::now();
    let manifest = ExportPackageManifest {
        schema: SCOPE_PACKAGE_SCHEMA.into(),
        package_id: Uuid::new_v4().to_string(),
        created_at: now.to_rfc3339(),
        producer: PackageProducer {
            product: "sylvode".into(),
            version: env!("CARGO_PKG_VERSION").into(),
            source_head: env!("OPENPR_EMBEDDED_GIT_COMMIT").into(),
        },
        source: PackageSource {
            workspace_id: None,
            scope_id: Some(plan.scope_id.to_string()),
            scope: plan.selection.into(),
            root_object_ids: plan.roots.iter().map(ToString::to_string).collect(),
        },
        flow_schema_version: 2,
        engine: PackageEngine {
            name: ENGINE_NAME.into(),
            crate_version: ENGINE_CRATE_VERSION.into(),
            wire_format_version: ENGINE_WIRE_FORMAT_VERSION,
            semantic_model: Some(super::package::SEMANTIC_MODEL_V3.into()),
        },
        history: PackageHistory {
            included: include_history,
            snapshot_history: Some("retained".into()),
            through_seq_by_document: through,
        },
        counts: PackageCounts {
            objects: plan.objects.len() as u64,
            documents: plan.objects.len() as u64,
            relations: rows.len() as u64,
            updates: update_count,
            events: event_count,
            ..PackageCounts::default()
        },
        members: Vec::new(),
        export_policy: ExportPolicy {
            complete: true,
            permission_snapshot_at: now.to_rfc3339(),
        },
        projects: Vec::new(),
        warnings,
    };
    let bytes = build_package(manifest, members)?.bytes;
    if bytes.len() as u64 > super::collab::limits::IMPORT_ARCHIVE_BYTES_MAX {
        return Err(ApiError::limit_exceeded(
            "package archive byte budget exhausted",
            "import_archive_bytes",
            Some(json!(super::collab::limits::IMPORT_ARCHIVE_BYTES_MAX)),
            Some(json!(bytes.len())),
            None,
        ));
    }
    Ok(bytes)
}
