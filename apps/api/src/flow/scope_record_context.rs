//! Bounded Record authority and visible schema. No Collection read/edit grant is manufactured.
use super::json_value::FlowJson;
use super::{collab::authz::PermissionLevel, collections, scope};
use crate::error::{ApiError, ApiErrorKind};
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use hmac::{Hmac, Mac};
use platform::app::AppState;
use sea_orm::{
    ConnectionTrait, DatabaseConnection, DatabaseTransaction, DbBackend, FromQueryResult, Statement, TransactionTrait,
};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};
use sha2::{Digest, Sha256};
use std::collections::BTreeSet;
use uuid::Uuid;

/// Remains false until route/write hooks and owned developer checks are collected.
pub const RECORD_CONTEXT_SUPPORTED: bool = true;
#[derive(Debug, Serialize)]
pub struct RecordFieldView {
    pub field_id: Uuid,
    pub label: String,
    #[serde(rename = "type")]
    pub field_type: String,
    pub config: Value,
    pub position: i64,
    pub archived: bool,
}
#[derive(Serialize)]
pub struct RecordContext {
    pub record: Value,
    pub fields: Vec<RecordFieldView>,
    pub values_by_field_id: Map<String, Value>,
    pub field_secrecy_enabled: bool,
    pub schema_revision: String,
}
/// Server-only authorized read, deliberately not Serialize (parent dependencies stay internal).
pub struct AuthorizedRecordRead {
    pub record: Value,
    pub fields: Vec<RecordFieldView>,
    pub values_by_field_id: Map<String, Value>,
    pub field_secrecy_enabled: bool,
    guard: RecordSchemaGuard,
}
impl AuthorizedRecordRead {
    pub const fn parent_collection_id(&self) -> Uuid {
        self.guard.collection
    }
    pub const fn document_id(&self) -> Uuid {
        self.guard.document
    }
    pub fn schema_fingerprint(&self) -> String {
        hex::encode(self.guard.fingerprint)
    }
}

#[derive(Clone)]
pub(crate) struct RecordSchemaGuard {
    record: Uuid,
    collection: Uuid,
    scope: Uuid,
    document: Uuid,
    principal: Uuid,
    fingerprint: [u8; 32],
    allowed_fields: BTreeSet<Uuid>,
    body_allowed: bool,
    require_active_parent: bool,
}
pub(crate) struct RecordAuthority {
    pub(crate) proof: scope::AuthorizationProof,
    pub(crate) guard: RecordSchemaGuard,
    pub(crate) canonical_body: Value,
}
impl RecordAuthority {
    pub(crate) const fn record(&self) -> Uuid {
        self.guard.record
    }
    pub(crate) const fn collection(&self) -> Uuid {
        self.guard.collection
    }
    pub(crate) fn schema_guard(&self) -> RecordSchemaGuard {
        self.guard.clone()
    }
}
#[derive(FromQueryResult)]
struct Binding {
    collection_id: Uuid,
    document_id: Uuid,
    properties: Value,
    document_seq: i64,
    head_seq: i64,
}
async fn binding<C: ConnectionTrait>(db: &C, record: Uuid, lock: bool) -> Result<Binding, ApiError> {
    let sql = format!(
        "SELECT r.collection_id,r.document_id,r.properties,r.document_seq,d.head_seq FROM flow_record_projections r JOIN collab_documents d ON d.id=r.document_id AND d.object_id=r.record_id WHERE r.record_id=$1{}",
        if lock { " FOR SHARE OF r" } else { "" }
    );
    Binding::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        sql,
        vec![record.into()],
    ))
    .one(db)
    .await?
    .ok_or_else(|| ApiError::not_found("Record not found"))
}
async fn checked_binding<C: ConnectionTrait>(
    db: &C,
    record: Uuid,
    lock: bool,
) -> Result<(scope::ObjectIdentity, Binding), ApiError> {
    let identity = scope::identity(db, record).await?;
    if identity.object_type != "record" {
        return Err(ApiError::not_found("Record not found"));
    }
    let row = binding(db, record, lock).await?;
    let parent = scope::identity(db, row.collection_id).await?;
    if identity.parent_id != Some(row.collection_id)
        || parent.scope_id != identity.scope_id
        || parent.object_type != "collection"
    {
        return Err(ApiError::not_found("Record not found"));
    }
    Ok((identity, row))
}
async fn visible_schema<C: ConnectionTrait>(
    db: &C,
    collection: Uuid,
) -> Result<(Vec<RecordFieldView>, bool), ApiError> {
    let projection = collections::collection_read_row(db, collection).await?;
    let fields = collections::collection_fields(db, collection, true).await?;
    let secrecy = projection.field_secrecy_enabled || fields.iter().any(collections::field_is_restricted);
    let mut visible = Vec::new();
    for field in fields
        .into_iter()
        .filter(|f| !f.archived && !collections::field_is_restricted(f))
    {
        visible.push(RecordFieldView {
            field_id: field.field_id,
            label: field.label,
            field_type: field.field_type,
            config: field.config,
            position: i64::try_from(visible.len()).map_err(|_| ApiError::internal())?,
            archived: false,
        });
    }
    Ok((visible, secrecy))
}
fn fingerprint(fields: &[RecordFieldView], secret: bool) -> Result<[u8; 32], ApiError> {
    let bytes = serde_json::to_vec(&json!({"fields":fields,"field_secrecy_enabled":secret}))
        .map_err(|_| ApiError::internal())?;
    Ok(Sha256::digest(bytes).into())
}
fn token_bytes(guard: &RecordSchemaGuard) -> Result<Vec<u8>, ApiError> {
    serde_json::to_vec(&json!({"version":1,"record":guard.record,"scope":guard.scope,"document":guard.document,"principal":guard.principal,"visible_schema_hash":hex::encode(guard.fingerprint)})).map_err(|_|ApiError::internal())
}
fn mint(secret: &str, guard: &RecordSchemaGuard) -> Result<String, ApiError> {
    let mut mac = Hmac::<Sha256>::new_from_slice(secret.as_bytes()).map_err(|_| ApiError::internal())?;
    mac.update(&token_bytes(guard)?);
    Ok(format!("rctx1.{}", URL_SAFE_NO_PAD.encode(mac.finalize().into_bytes())))
}
pub(crate) fn schema_changed() -> ApiError {
    ApiError::typed_with_details(
        ApiErrorKind::StaleFrontier,
        "Record fields changed; refresh and review the Record context",
        json!({"reason":"record_schema_changed","action":"refresh_record_context","write_state":"not_accepted","recoverable":true}),
    )
}
fn verify(secret: &str, provided: &str, guard: &RecordSchemaGuard) -> Result<(), ApiError> {
    let bytes = provided
        .strip_prefix("rctx1.")
        .and_then(|value| URL_SAFE_NO_PAD.decode(value).ok())
        .ok_or_else(schema_changed)?;
    let mut mac = Hmac::<Sha256>::new_from_slice(secret.as_bytes()).map_err(|_| ApiError::internal())?;
    mac.update(&token_bytes(guard)?);
    mac.verify_slice(&bytes).map_err(|_| schema_changed())
}
fn guard(
    identity: &scope::ObjectIdentity,
    row: &Binding,
    user: Uuid,
    fields: &[RecordFieldView],
    secrecy: bool,
) -> Result<RecordSchemaGuard, ApiError> {
    Ok(RecordSchemaGuard {
        record: identity.id,
        collection: row.collection_id,
        scope: identity.scope_id,
        document: row.document_id,
        principal: user,
        fingerprint: fingerprint(fields, secrecy)?,
        allowed_fields: fields.iter().map(|f| f.field_id).collect(),
        body_allowed: !secrecy,
        require_active_parent: true,
    })
}
pub fn decorate_record_view(view: &mut Value) {
    if view.get("object_type").and_then(Value::as_str) == Some("record")
        && let Some(object) = view.as_object_mut()
    {
        object.remove("parent_id");
        object.insert("record_context_supported".into(), json!(RECORD_CONTEXT_SUPPORTED));
    }
}
fn bounded_semantic(view: &mut Value, fields: &[RecordFieldView], secrecy: bool) {
    if secrecy {
        if let Some(object) = view.as_object_mut() {
            object.insert("semantic_content".into(), collections::redacted_semantic_content());
        }
        return;
    }
    let allowed = fields
        .iter()
        .map(|field| field.field_id.to_string())
        .collect::<BTreeSet<_>>();
    // Known typed Record semantic nodes only. Field values/body strings remain opaque and unchanged.
    if let Some(nodes) = view
        .pointer_mut("/semantic_content/nodes")
        .and_then(Value::as_object_mut)
    {
        nodes.retain(|id, _| id == "record:body" || allowed.contains(id));
    }
}
async fn bounded_result(state: &AppState, record: Uuid, user: Uuid, result: Value) -> Result<Value, ApiError> {
    let current = read(state, record, user).await?;
    let mut result = sanitize_result(result);
    if let Some(object) = result.get_mut("object") {
        bounded_semantic(object, &current.fields, current.field_secrecy_enabled);
        if let Some(actions) = current.record.get("permitted_actions") {
            *object.field_mut("permitted_actions")? = actions.clone();
        }
        if let Some(target) = object.as_object_mut() {
            for key in [
                "record_content_writable",
                "record_archive_allowed",
                "record_readonly_reason",
            ] {
                if let Some(value) = current.record.get(key) {
                    target.insert(key.to_owned(), value.clone());
                } else {
                    target.remove(key);
                }
            }
        }
    }
    Ok(result)
}

pub(crate) fn sanitize_result(mut result: Value) -> Value {
    if let Some(object) = result.get_mut("object") {
        decorate_record_view(object);
    }
    // Known response positions only; opaque user values are never recursively rewritten.
    if let Some(command) = result.get_mut("command_result").and_then(Value::as_object_mut) {
        for key in ["collection_id", "schema_seq", "record_count"] {
            command.remove(key);
        }
    }
    result
}
pub async fn read_for_export<C: ConnectionTrait + TransactionTrait>(
    db: &C,
    record: Uuid,
    user: Uuid,
) -> Result<AuthorizedRecordRead, ApiError> {
    let proof = scope::authorize(db, record, user, PermissionLevel::View).await?;
    let (_, initial) = checked_binding(db, record, false).await?;
    let tx = db.begin().await?;
    scope::fence(&tx, &proof).await?;
    tx.execute(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "SELECT collection_id FROM flow_collection_projections WHERE collection_id=$1 FOR SHARE",
        vec![initial.collection_id.into()],
    ))
    .await?;
    // Shared writer order is schema projection -> document head -> record projection.
    tx.query_one(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "SELECT id FROM collab_documents WHERE id=$1 AND object_id=$2 FOR SHARE",
        vec![initial.document_id.into(), record.into()],
    ))
    .await?
    .ok_or_else(|| ApiError::not_found("Record document not found"))?;
    let (identity, row) = checked_binding(&tx, record, true).await?;
    if row.collection_id != initial.collection_id {
        return Err(schema_changed());
    }
    if row.document_seq != row.head_seq {
        return Err(ApiError::resync_required("Record projection is not current", None));
    }
    let (fields, secrecy) = visible_schema(&tx, row.collection_id).await?;
    let guard = guard(&identity, &row, user, &fields, secrecy)?;
    let properties = row.properties.as_object().ok_or_else(ApiError::internal)?;
    let allowed = guard.allowed_fields.clone();
    let values = properties
        .iter()
        .filter(|(id, _)| Uuid::parse_str(id).ok().is_some_and(|id| allowed.contains(&id)))
        .map(|(id, value)| (id.clone(), value.clone()))
        .collect();
    let mut view = scope::object_view(&tx, record, user).await?;
    decorate_record_view(&mut view);
    bounded_semantic(&mut view, &fields, secrecy);
    let parent_active = scope::identity(&tx, row.collection_id).await?.lifecycle_status == "active";
    let archive_allowed = proof.level >= PermissionLevel::Edit && proof.resource_active;
    *view.field_mut("record_archive_allowed")? = json!(archive_allowed);
    *view.field_mut("record_content_writable")? = json!(archive_allowed && parent_active);
    if !archive_allowed || !parent_active {
        *view.field_mut("record_readonly_reason")? = json!(if identity.lifecycle_status != "active" {
            "record_inactive"
        } else if proof.level < PermissionLevel::Edit {
            "insufficient_access"
        } else if !proof.resource_active {
            "scope_unavailable"
        } else {
            "container_inactive"
        });
    }
    let result = AuthorizedRecordRead {
        record: view,
        fields,
        values_by_field_id: values,
        field_secrecy_enabled: secrecy,
        guard,
    };
    tx.commit().await?;
    Ok(result)
}
pub async fn read(state: &AppState, record: Uuid, user: Uuid) -> Result<RecordContext, ApiError> {
    let result = read_for_export(&state.db, record, user).await?;
    let revision = mint(state.cfg.jwt_secret.expose(), &result.guard)?;
    Ok(RecordContext {
        record: result.record,
        fields: result.fields,
        values_by_field_id: result.values_by_field_id,
        field_secrecy_enabled: result.field_secrecy_enabled,
        schema_revision: revision,
    })
}
/// Masks only known Record DTO/typed semantic positions, preserving historical values/head.
pub async fn sanitize_record_view(
    db: &DatabaseConnection,
    record: Uuid,
    user: Uuid,
    view: &mut Value,
) -> Result<(), ApiError> {
    if view.get("id").and_then(Value::as_str) != Some(record.to_string().as_str())
        || view.get("object_type").and_then(Value::as_str) != Some("record")
    {
        return Err(ApiError::policy_rejected("Record response identity mismatch"));
    }
    let current = read_for_export(db, record, user).await?;
    decorate_record_view(view);
    bounded_semantic(view, &current.fields, current.field_secrecy_enabled);
    if let Some(actions) = current.record.get("permitted_actions") {
        *view.field_mut("permitted_actions")? = actions.clone();
    }
    if let Some(target) = view.as_object_mut() {
        for key in [
            "record_content_writable",
            "record_archive_allowed",
            "record_readonly_reason",
        ] {
            if let Some(value) = current.record.get(key) {
                target.insert(key.to_owned(), value.clone());
            } else {
                target.remove(key);
            }
        }
    }

    Ok(())
}

impl RecordSchemaGuard {
    pub(crate) fn validate_values(&self, payload: &Value) -> Result<(), ApiError> {
        if payload.get("body").is_some_and(|value| !value.is_null()) && !self.body_allowed {
            return Err(ApiError::policy_rejected(
                "Record body is not available under field secrecy",
            ));
        }
        if let Some(properties) = payload.get("properties").and_then(Value::as_object) {
            for id in properties.keys() {
                if !Uuid::parse_str(id)
                    .ok()
                    .is_some_and(|id| self.allowed_fields.contains(&id))
                {
                    return Err(ApiError::policy_rejected("Record field is not available for editing"));
                }
            }
        }
        Ok(())
    }
}
/// Called after the shared parent projection lock, before field validation/persistence.
pub(crate) async fn validate_guard(
    tx: &DatabaseTransaction,
    collection: Uuid,
    record: Uuid,
    user: Uuid,
    guard: &RecordSchemaGuard,
    payload: &Value,
) -> Result<(), ApiError> {
    let (identity, row) = checked_binding(tx, record, false).await?;
    if guard.record != record
        || guard.collection != collection
        || guard.principal != user
        || identity.scope_id != guard.scope
        || row.document_id != guard.document
    {
        return Err(schema_changed());
    }
    if guard.require_active_parent && scope::identity(tx, collection).await?.lifecycle_status != "active" {
        return Err(ApiError::policy_rejected(
            "Record container is unavailable for content changes",
        ));
    }
    let (fields, secrecy) = visible_schema(tx, collection).await?;
    if fingerprint(&fields, secrecy)? != guard.fingerprint {
        return Err(schema_changed());
    }
    guard.validate_values(payload)
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RecordInput {
    record_id: Option<Uuid>,
    #[serde(default)]
    properties: Map<String, Value>,
    body: Option<String>,
    schema_revision: String,
}
pub async fn execute(
    state: &AppState,
    record: Uuid,
    user: Uuid,
    kind: &str,
    payload: Value,
    key: &str,
    expected: Option<&str>,
    message: Option<&str>,
) -> Result<Value, ApiError> {
    let kind = match kind {
        "record_patch" => collections::CollectionCommandType::RecordPatch,
        "record_archive" => collections::CollectionCommandType::RecordArchive,
        _ => {
            return Err(ApiError::unsupported_protocol(
                "Record target supports only typed patch/archive",
            ));
        }
    };
    let proof = scope::authorize(&state.db, record, user, PermissionLevel::Edit).await?;
    let input: RecordInput =
        serde_json::from_value(payload).map_err(|_| ApiError::invalid_update("invalid Record command payload"))?;
    if input.record_id.is_some_and(|id| id != record) {
        return Err(ApiError::invalid_update("record_id must equal the target Record"));
    }
    if input.schema_revision.is_empty() || input.schema_revision.len() > 256 {
        return Err(ApiError::invalid_update("schema_revision is required"));
    }
    if kind == collections::CollectionCommandType::RecordArchive
        && (!input.properties.is_empty() || input.body.is_some())
    {
        return Err(ApiError::invalid_update(
            "record_archive does not accept changed properties/body",
        ));
    }
    let expected = expected
        .ok_or_else(|| ApiError::invalid_update("expected_frontier is required for a Record-target command"))?;
    let normalized = json!({"record_id":record,"properties":input.properties,"body":input.body,"schema_revision":input.schema_revision});
    let canonical = json!({"operation":"record_target_command","record_id":record,"command":kind.wire_name(),"payload":normalized,"expected_frontier":expected,"message":message});
    if let Some(prior) = super::scope_commands::replay(&state.db, proof.scope_id, user, key, &canonical).await? {
        return bounded_result(state, record, user, prior).await;
    }
    let (identity, row) = checked_binding(&state.db, record, false).await?;
    if !proof.resource_active {
        return Err(ApiError::policy_rejected("Archived Record is read-only"));
    }
    let (fields, secrecy) = visible_schema(&state.db, row.collection_id).await?;
    let mut guard = guard(&identity, &row, user, &fields, secrecy)?;
    guard.require_active_parent = kind == collections::CollectionCommandType::RecordPatch;
    verify(state.cfg.jwt_secret.expose(), &input.schema_revision, &guard)?;
    guard.validate_values(&normalized)?;
    let authority = RecordAuthority {
        proof,
        guard,
        canonical_body: canonical,
    };
    let result = super::scope_collections::execute_record_authorized(
        state,
        user,
        kind,
        normalized,
        key,
        Some(expected),
        message,
        authority,
    )
    .await?;
    bounded_result(state, record, user, result).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::routes::context::tenant_fixture::{self, exec};
    use sea_orm::{DatabaseConnection, TransactionTrait};
    fn state(db: DatabaseConnection) -> AppState {
        AppState {
            db,
            cfg: platform::config::AppConfig {
                app_name: "record-context-unit".into(),
                bind_addr: "127.0.0.1:0".into(),
                database_url: platform::config::Secret::new("postgres://unused/unused"),
                jwt_secret: platform::config::Secret::new("record-context-unit-secret"),
                jwt_access_ttl_seconds: 900,
                jwt_refresh_ttl_seconds: 3600,
                allow_insecure_cookies: false,
                collab_allowed_origins: vec![],
            },
            flow_permission_cache: platform::app::FlowPermissionCacheSlot::default(),
        }
    }
    struct Fixture {
        scratch: tenant_fixture::Scratch,
        state: AppState,
        owner: Uuid,
        guest: Uuid,
        collection: Uuid,
        record: Uuid,
        sibling: Uuid,
        visible: Uuid,
        hidden: Uuid,
    }
    async fn fixture(secret: bool) -> Fixture {
        let scratch = tenant_fixture::scratch(&format!("record_ctx_{}", Uuid::new_v4().simple()))
            .await
            .expect("explicit disposable OPENPR_TEST_DATABASE_URL required");
        let db = &scratch.db;
        for component in ["api", "worker", "maintenance"] {
            super::super::scope_runtime::renew(db, Uuid::new_v4(), component)
                .await
                .unwrap();
        }
        db.execute_unprepared(
            "UPDATE flow_runtime_capabilities SET personal_provisioning=true,personal_writes=true WHERE singleton",
        )
        .await
        .unwrap();
        let owner = Uuid::new_v4();
        let guest = Uuid::new_v4();
        for id in [owner, guest] {
            exec(db,"INSERT INTO users(id,email,password_hash,name,role,is_active) VALUES($1,$2,'!','Record unit fixture','user',true)",vec![id.into(),format!("{id}@record-unit.test").into()]).await;
        }
        let home = scope::personal_home(db, owner).await.unwrap();
        let scope_id = home.0.id;
        let visible = Uuid::new_v4();
        let hidden = Uuid::new_v4();
        let view = Uuid::new_v4();
        let created=scope::create_personal(db,owner,scope::CreateInput{editor_model:1,scope_id,object_type:"collection".into(),parent_object_id:None,project_id:None,title:"PRIVATE PARENT TITLE SENTINEL".into(),idempotency_key:Uuid::new_v4().to_string(),message:None,initial_fields:vec![json!({"field_id":hidden,"label":"HIDDEN FIELD LABEL SENTINEL","field_type":"text","index":0,"config":{"restricted":secret}}),json!({"field_id":visible,"label":"Visible field","field_type":"text","index":1,"config":{}})],initial_view:Some(json!({"view_id":view,"name":"PRIVATE VIEW SENTINEL","view_type":"table","config":{}}))}).await.unwrap();
        let collection = Uuid::parse_str(created.pointer("/object/id").unwrap().as_str().unwrap()).unwrap();
        let state = state(db.clone());
        let mut ids = Vec::new();
        for index in 0..2 {
            let row=super::super::scope_collections::execute(&state,collection,owner,collections::CollectionCommandType::RecordCreate,json!({"properties":{visible.to_string():format!("visible value {index}"),hidden.to_string():"HIDDEN VALUE SENTINEL"},"body":"body text"}),&Uuid::new_v4().to_string(),None,None).await.unwrap();
            ids.push(Uuid::parse_str(row.pointer("/object/id").unwrap().as_str().unwrap()).unwrap());
        }
        let record = *ids.first().unwrap();
        let sibling = *ids.get(1).unwrap();
        exec(db,"INSERT INTO flow_object_grants(object_id,scope_id,principal_kind,principal_id,level,granted_by) VALUES($1,$2,'user',$3,'edit',$4)",vec![record.into(),scope_id.into(),guest.into(),owner.into()]).await;
        Fixture {
            scratch,
            state,
            owner,
            guest,
            collection,
            record,
            sibling,
            visible,
            hidden,
        }
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
    #[tokio::test]
    async fn bounded_context_without_parent_access_redacts_entire_response_and_binds_token() {
        let f = fixture(true).await;
        assert!(
            scope::authorize(&f.state.db, f.collection, f.guest, PermissionLevel::View)
                .await
                .is_err()
        );
        assert!(
            scope::authorize(&f.state.db, f.collection, f.guest, PermissionLevel::Edit)
                .await
                .is_err()
        );
        let context = read(&f.state, f.record, f.guest).await.unwrap();
        let value = serde_json::to_value(&context).unwrap();
        let all = value.to_string();
        for private in [
            f.collection.to_string(),
            f.sibling.to_string(),
            f.hidden.to_string(),
            "PRIVATE PARENT TITLE SENTINEL".into(),
            "PRIVATE VIEW SENTINEL".into(),
            "HIDDEN FIELD LABEL SENTINEL".into(),
            "HIDDEN VALUE SENTINEL".into(),
        ] {
            assert!(
                !all.contains(&private),
                "private parent/field metadata escaped the bounded DTO"
            );
        }
        assert!(value.get("schema_seq").is_none());
        assert!(value.get("collection_id").is_none());
        assert!(value.pointer("/record/parent_id").is_none());
        assert!(value.pointer("/fields/0/document_seq").is_none());
        assert_eq!(value.pointer("/fields/0/position"), Some(&json!(0)));
        assert_eq!(context.values_by_field_id.len(), 1);
        let bytes = URL_SAFE_NO_PAD
            .decode(context.schema_revision.strip_prefix("rctx1.").unwrap())
            .unwrap();
        assert_eq!(bytes.len(), 32);
        assert!(serde_json::from_slice::<Value>(&bytes).is_err());
        let (identity, row) = checked_binding(&f.state.db, f.record, false).await.unwrap();
        let guard = guard(&identity, &row, f.guest, &context.fields, true).unwrap();
        verify(f.state.cfg.jwt_secret.expose(), &context.schema_revision, &guard).unwrap();
        let other = RecordSchemaGuard {
            principal: f.owner,
            ..guard
        };
        assert!(verify(f.state.cfg.jwt_secret.expose(), &context.schema_revision, &other).is_err());
        assert_eq!(count(&f.state.db, "workspaces").await, 0);
        assert_eq!(count(&f.state.db, "workspace_members").await, 0);
        drop(f.state);
        f.scratch.drop_self().await;
    }
    #[tokio::test]
    async fn stale_schema_and_hidden_fields_refuse_before_any_record_update() {
        let f = fixture(true).await;
        let context = read(&f.state, f.record, f.guest).await.unwrap();
        let frontier = context.record.field("frontier").as_str().unwrap();
        let before = count(&f.state.db, "collab_updates").await;
        let (identity, row) = checked_binding(&f.state.db, f.record, false).await.unwrap();
        let captured = guard(&identity, &row, f.guest, &context.fields, true).unwrap();
        let hidden = execute(
            &f.state,
            f.record,
            f.guest,
            "record_patch",
            json!({"properties":{f.hidden.to_string():"not writable"},"schema_revision":context.schema_revision}),
            "hidden-refusal",
            Some(frontier),
            None,
        )
        .await
        .err()
        .unwrap();
        assert_eq!(hidden.kind(), ApiErrorKind::PolicyRejected);
        exec(
            &f.state.db,
            "UPDATE flow_field_projections SET label='Visible field changed' WHERE collection_id=$1 AND field_id=$2",
            vec![f.collection.into(), f.visible.into()],
        )
        .await;
        let changed = execute(
            &f.state,
            f.record,
            f.guest,
            "record_patch",
            json!({"properties":{f.visible.to_string():"draft"},"schema_revision":context.schema_revision}),
            "schema-refusal",
            Some(frontier),
            None,
        )
        .await
        .err()
        .unwrap();
        assert_eq!(changed.kind(), ApiErrorKind::StaleFrontier);
        let tx = f.state.db.begin().await.unwrap();
        tx.execute(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT collection_id FROM flow_collection_projections WHERE collection_id=$1 FOR SHARE",
            vec![f.collection.into()],
        ))
        .await
        .unwrap();
        assert_eq!(
            validate_guard(
                &tx,
                f.collection,
                f.record,
                f.guest,
                &captured,
                &json!({"properties":{}})
            )
            .await
            .err()
            .unwrap()
            .kind(),
            ApiErrorKind::StaleFrontier
        );
        tx.rollback().await.unwrap();
        assert_eq!(count(&f.state.db, "collab_updates").await, before);
        drop(f.state);
        f.scratch.drop_self().await;
    }
    #[tokio::test]
    async fn record_edit_only_archive_keeps_head_and_immutable_receipt_without_parent_disclosure() {
        let f = fixture(false).await;
        let context = read(&f.state, f.record, f.guest).await.unwrap();
        let frontier = context.record.field("frontier").as_str().unwrap();
        let payload = json!({"schema_revision":context.schema_revision});
        let updates = count(&f.state.db, "collab_updates").await;
        let events = count(&f.state.db, "business_events").await;
        let result = execute(
            &f.state,
            f.record,
            f.guest,
            "record_archive",
            payload.clone(),
            "archive-unit",
            Some(frontier),
            None,
        )
        .await
        .unwrap();
        assert_eq!(result.pointer("/object/lifecycle_status"), Some(&json!("archived")));
        assert!(!result.to_string().contains(&f.collection.to_string()));
        assert_eq!(count(&f.state.db, "collab_updates").await, updates);
        assert_eq!(count(&f.state.db, "business_events").await, events + 1);
        let again = execute(
            &f.state,
            f.record,
            f.guest,
            "record_archive",
            payload.clone(),
            "archive-unit",
            Some(frontier),
            None,
        )
        .await
        .unwrap();
        assert_eq!(again.field("event_id"), result.field("event_id"));
        assert_eq!(count(&f.state.db, "business_events").await, events + 1);
        let drift = execute(
            &f.state,
            f.record,
            f.guest,
            "record_archive",
            payload,
            "archive-unit",
            Some("different"),
            None,
        )
        .await
        .err()
        .unwrap();
        assert_eq!(drift.kind(), ApiErrorKind::InvalidUpdate);
        assert!(
            matches!(drift,ApiError::Typed{details:Some(ref details),..} if details.get("reason").and_then(Value::as_str)==Some("idempotency_key_body_drift"))
        );
        assert!(
            scope::authorize(&f.state.db, f.collection, f.guest, PermissionLevel::View)
                .await
                .is_err()
        );
        drop(f.state);
        f.scratch.drop_self().await;
    }
    #[tokio::test]
    async fn inactive_container_blocks_content_but_keeps_authorized_record_cleanup_readable() {
        let f = fixture(false).await;
        exec(
            &f.state.db,
            "UPDATE flow_objects SET lifecycle_status='archived',archived_at=now() WHERE id=$1",
            vec![f.collection.into()],
        )
        .await;
        let context = read(&f.state, f.record, f.guest).await.unwrap();
        assert_eq!(context.record.get("lifecycle_status"), Some(&json!("active")));
        assert_eq!(context.record.get("record_content_writable"), Some(&json!(false)));
        assert_eq!(context.record.get("record_archive_allowed"), Some(&json!(true)));
        assert_eq!(
            context.record.get("record_readonly_reason"),
            Some(&json!("container_inactive"))
        );
        let frontier = context.record.get("frontier").unwrap().as_str().unwrap();
        let before = count(&f.state.db, "collab_updates").await;
        let patch = execute(
            &f.state,
            f.record,
            f.guest,
            "record_patch",
            json!({"schema_revision":context.schema_revision,"properties":{f.visible.to_string():"blocked"}}),
            "inactive-patch",
            Some(frontier),
            None,
        )
        .await
        .err()
        .unwrap();
        assert!(matches!(
            patch.kind(),
            ApiErrorKind::InvalidUpdate | ApiErrorKind::PolicyRejected
        ));
        assert_eq!(count(&f.state.db, "collab_updates").await, before);
        let archived = execute(
            &f.state,
            f.record,
            f.guest,
            "record_archive",
            json!({"schema_revision":context.schema_revision}),
            "inactive-cleanup",
            Some(frontier),
            None,
        )
        .await
        .unwrap();
        assert_eq!(archived.pointer("/object/lifecycle_status"), Some(&json!("archived")));
        assert_eq!(count(&f.state.db, "collab_updates").await, before);
        drop(f.state);
        f.scratch.drop_self().await;
    }
    #[tokio::test]
    async fn record_only_rendered_exports_preserve_public_values_without_parent_or_hidden_data() {
        let f = fixture(true).await;
        assert!(
            scope::authorize(&f.state.db, f.collection, f.guest, PermissionLevel::View)
                .await
                .is_err()
        );
        for format in ["json", "markdown", "csv"] {
            let receipt = super::super::scope_export::create(
                &f.state.db,
                f.record,
                f.guest,
                format,
                None,
                false,
                &Uuid::new_v4().to_string(),
            )
            .await
            .unwrap();
            let job = Uuid::parse_str(receipt.get("job_id").unwrap().as_str().unwrap()).unwrap();
            let got = super::super::scope_export::get(&f.state.db, job, f.guest)
                .await
                .unwrap();
            let get_dto = super::super::scope_export::receipt(&got);
            let (bytes, _, wire_format) = super::super::scope_export::download(&f.state.db, job, f.guest)
                .await
                .unwrap();
            assert_eq!(wire_format, format);
            let artifact = String::from_utf8(bytes).unwrap();
            assert!(
                artifact.contains("visible value 0"),
                "public value missing from rendered artifact"
            );
            for output in [artifact, receipt.to_string(), get_dto.to_string()] {
                for private in [
                    f.collection.to_string(),
                    f.sibling.to_string(),
                    f.hidden.to_string(),
                    "PRIVATE PARENT TITLE SENTINEL".into(),
                    "PRIVATE VIEW SENTINEL".into(),
                    "HIDDEN FIELD LABEL SENTINEL".into(),
                    "HIDDEN VALUE SENTINEL".into(),
                ] {
                    assert!(
                        !output.contains(&private),
                        "private parent/field metadata escaped export artifact or job DTO"
                    );
                }
                for private_key in ["policy_schema_seq", "policy_collection_id", "schema_seq"] {
                    assert!(
                        !output.contains(private_key),
                        "private schema dependency escaped wire DTO"
                    );
                }
            }
        }
        drop(f.state);
        f.scratch.drop_self().await;
    }
}
