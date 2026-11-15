//! Explicit, idempotent editor-format admission. GET only inspects private candidates.
use super::{
    collab::{authz::PermissionLevel, write::AcceptOutcome},
    scope,
};
use crate::error::{ApiError, ApiErrorKind};
use base64::{Engine, engine::general_purpose::STANDARD};
use collab_core::CollabEngine;
use sea_orm::{ConnectionTrait, DatabaseConnection, DbBackend, Statement, TransactionTrait};
use serde_json::{Value, json};
use uuid::Uuid;
pub async fn require_write<C: ConnectionTrait>(db: &C, identity: &scope::ObjectIdentity) -> Result<(), ApiError> {
    if identity.workspace_id.is_none() {
        return scope::require_personal_write(db).await;
    }
    if !scope::capabilities(db).await?.scope_native_legacy {
        return Err(ApiError::typed_with_details(
            ApiErrorKind::FeatureDisabled,
            "legacy editor initialization is not enabled",
            json!({"reason":"scope_native_legacy_disabled"}),
        ));
    }
    scope::require_reader_ready(db).await
}
async fn document<C: ConnectionTrait>(db: &C, object: Uuid) -> Result<(Uuid, i64, Vec<u8>), ApiError> {
    let row = db
        .query_one(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT id,head_seq,head_frontier FROM collab_documents WHERE object_id=$1",
            vec![object.into()],
        ))
        .await?
        .ok_or_else(|| ApiError::not_found("Flow document not found"))?;
    Ok((
        row.try_get("", "id")?,
        row.try_get("", "head_seq")?,
        row.try_get("", "head_frontier")?,
    ))
}
fn admission_error(seq: i64, frontier: &[u8], unsupported: bool) -> ApiError {
    ApiError::typed_with_details(
        ApiErrorKind::UnsupportedProtocol,
        "editor initialization is required before collaboration",
        json!({"reason":if unsupported{"editor_migration_unsupported"}else{"editor_migration_required"},"expected_seq":seq,"expected_frontier":STANDARD.encode(frontier)}),
    )
}
pub async fn require_admitted(db: &DatabaseConnection, object: Uuid) -> Result<(), ApiError> {
    if scope::identity(db, object).await?.object_type != "page" {
        return Ok(());
    }
    let (doc, seq, frontier) = document(db, object).await?;
    let mut engine = super::document::load_head_engine(db, doc).await?;
    if engine.has_editor_doc() {
        return Ok(());
    }
    let unsupported = engine.initialize_page_editor().is_err();
    Err(admission_error(seq, &frontier, unsupported))
}
pub async fn decorate(db: &DatabaseConnection, object: Uuid, user: Uuid, view: &mut Value) -> Result<(), ApiError> {
    let page = view.get("object_type").and_then(Value::as_str) == Some("page");
    if !page && view.get("object_type").and_then(Value::as_str) != Some("navigator") {
        return Ok(());
    }
    let proof = scope::authorize(db, object, user, PermissionLevel::View).await?;
    let identity = scope::identity(db, object).await?;
    let (doc, _, _) = document(db, object).await?;
    let expected = view["frontier"].as_str().ok_or_else(ApiError::internal)?.to_string();
    let mut engine = if let Some(engine) =
        super::projection::refresh_editor_state(db, doc, &expected, &mut view["semantic_content"]).await?
    {
        view["title"] = json!(engine.title().map_err(|e| super::command::map_collab_error(&e))?);
        engine
    } else {
        super::document::load_head_engine(db, doc).await?
    };

    if collab_core::isolation::wire::package_reference_remap_required(&engine)
        .map_err(|e| super::command::map_collab_error(&e))?
    {
        if let Some(formats) = view.get_mut("supported_export_formats").and_then(Value::as_array_mut) {
            formats.retain(|format| format.as_str() != Some("package"));
        }
        if let Some(fields) = view.as_object_mut() {
            fields.insert("history_export_supported".into(), json!(false));
            fields.insert("package_snapshot_history_retained".into(), json!(false));
            fields.insert(
                "export_unavailable_reason".into(),
                json!("package_inner_reference_remap_unsupported"),
            );
        }
    }
    if !page {
        if !scope::proof_current(db, &proof).await? {
            return Err(super::policy::authorization_read_unstable());
        }
        return Ok(());
    }
    view["editor_state"] = json!(if engine.has_editor_doc() {
        "ready"
    } else if engine.initialize_page_editor().is_ok() {
        "migration_required"
    } else {
        "unsupported"
    });
    let supported =
        proof.level >= PermissionLevel::Edit && proof.resource_active && view["editor_state"] != "unsupported";
    view["editor_initialization_supported"] = json!(if supported {
        match require_write(db, &identity).await {
            Ok(()) => true,
            Err(e) if e.kind() == ApiErrorKind::FeatureDisabled => false,
            Err(e) => return Err(e),
        }
    } else {
        false
    });
    if !scope::proof_current(db, &proof).await? {
        return Err(super::policy::authorization_read_unstable());
    }
    Ok(())
}
pub async fn initialize(
    db: &DatabaseConnection,
    object: Uuid,
    user: Uuid,
    payload: Value,
    expected: &str,
    key: &str,
) -> Result<Value, ApiError> {
    let identity = scope::identity(db, object).await?;
    let proof = scope::authorize(db, object, user, PermissionLevel::Edit).await?;
    if identity.object_type != "page" || !proof.resource_active {
        return Err(ApiError::invalid_update(
            "only an active Page can initialize editor prose",
        ));
    }
    if !payload.as_object().is_some_and(serde_json::Map::is_empty) {
        return Err(ApiError::invalid_update(
            "initialize_editor_doc accepts only an empty payload",
        ));
    }
    let expected_bytes = STANDARD
        .decode(expected)
        .map_err(|_| ApiError::invalid_update("invalid expected_frontier"))?;
    let body =
        json!({"operation":"initialize_editor_doc","object_id":object,"expected_frontier":expected,"payload":{}});
    if let Some(result) = super::scope_commands::replay(db, identity.scope_id, user, key, &body).await? {
        return Ok(result);
    }
    require_write(db, &identity).await?;
    if let Some(workspace) = identity.workspace_id
        && super::repository::find_idempotent_event(db, workspace, key)
            .await?
            .is_some()
    {
        return Err(ApiError::idempotency_key_body_drift(
            "legacy event identity is already used",
        ));
    }
    let (doc, seq, frontier) = document(db, object).await?;
    if expected_bytes != frontier {
        return Err(ApiError::stale_frontier(
            "editor initialization requires the current head",
            Some(seq),
            Some(&STANDARD.encode(&frontier)),
        ));
    }
    let mut engine = super::document::load_head_engine(db, doc).await?;
    let base = engine.frontier();
    if base.as_bytes() != expected_bytes {
        return Err(ApiError::stale_frontier(
            "document changed before editor initialization",
            None,
            None,
        ));
    }
    let changed = engine
        .initialize_page_editor()
        .map_err(|_| admission_error(seq, &frontier, true))?;
    if !changed {
        let tx = db.begin().await?;
        require_write(&tx, &identity).await?;
        scope::fence(&tx, &proof).await?;
        if let Some(result) = super::scope_commands::lock_and_replay(&tx, identity.scope_id, user, key, &body).await? {
            tx.commit().await?;
            return Ok(result);
        }
        let current = tx
            .query_one(Statement::from_sql_and_values(
                DbBackend::Postgres,
                "SELECT head_frontier FROM collab_documents WHERE id=$1 FOR SHARE",
                vec![doc.into()],
            ))
            .await?
            .ok_or_else(ApiError::internal)?
            .try_get::<Vec<u8>>("", "head_frontier")?;
        if current != expected_bytes {
            return Err(ApiError::stale_frontier(
                "document changed before editor initialization",
                None,
                None,
            ));
        }
        let event = super::scope_event::insert_object_audit(
            &tx,
            &identity,
            user,
            "flow.editor.initialized_noop",
            key,
            json!({"object_id":object}),
        )
        .await?;
        let result = scope::accepted(scope::object_view(&tx, object, user).await?, event);
        super::scope_commands::record(&tx, identity.scope_id, user, key, body, object, event, result.clone()).await?;
        tx.commit().await?;
        return Ok(result);
    }
    let bytes = engine
        .export_from(&base)
        .map_err(|e| super::command::map_collab_error(&e))?;
    let update = super::collab::write::replay_stable_update_id(doc, key);
    match super::scope_write::accept_editor_init(db, object, user, update, bytes, key, &expected_bytes, body.clone())
        .await?
    {
        AcceptOutcome::Accepted(_) => super::scope_commands::replay(db, identity.scope_id, user, key, &body)
            .await?
            .ok_or_else(ApiError::internal),
        AcceptOutcome::Rejected(rejected) => Err(super::scope_write::map_rejection(&rejected)),
    }
}

pub async fn require_document_admitted(db: &DatabaseConnection, document: Uuid) -> Result<(), ApiError> {
    let row = db
        .query_one(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT object_id FROM collab_documents WHERE id=$1",
            vec![document.into()],
        ))
        .await?
        .ok_or_else(|| ApiError::not_found("document not found"))?;
    require_admitted(db, row.try_get("", "object_id")?).await
}
