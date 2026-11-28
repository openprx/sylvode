//! Canonical Flow ownership and authorization namespaces. A scope is never a navigator root.
//!
//! Workspace authority delegates to the original workspace fence; personal authority uses its own
//! row. No code in this module manufactures a workspace or a workspace membership.
#[derive(FromQueryResult)]
struct ActiveScopeUser {
    id: Uuid,
}
#[derive(FromQueryResult)]
struct ScopePermissionGrant {
    level: String,
    workspace_id: Option<Uuid>,
    authz_epoch: Option<i64>,
}
#[derive(FromQueryResult)]
struct PriorScopeCreate {
    id: Uuid,
    aggregate_id: String,
    metadata: Value,
}

use crate::flow::json_value::FlowJson;
use collab_core::{CollabEngine, LoroCollabEngine};
use platform::timestamp::Timestamp;
use sea_orm::{
    ConnectionTrait, DatabaseConnection, DatabaseTransaction, DbBackend, FromQueryResult, Statement, TransactionTrait,
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use uuid::Uuid;

use super::{
    collab::authz::{self, PermissionLevel},
    projection, repository,
};
use crate::error::ApiError;

#[derive(Debug, Clone, Serialize, FromQueryResult)]
pub struct FlowScope {
    pub id: Uuid,
    pub kind: String,
    pub owner_user_id: Option<Uuid>,
    pub lifecycle_status: String,
}
#[derive(Debug, Clone, Serialize, FromQueryResult)]
pub struct ScopeRoot {
    pub object_id: Uuid,
    pub project_id: Option<Uuid>,
}
// Operator switches are independent published booleans.
#[expect(clippy::struct_excessive_bools, reason = "independent operator capability switches")]
#[derive(Debug, Clone, Serialize, FromQueryResult)]
pub struct Capabilities {
    pub block_transfers_enabled: bool,
    pub portable_page_packages_enabled: bool,
    pub portable_graph_packages_enabled: bool,
    pub workspace_editor_initialization: bool,
    pub personal_provisioning: bool,
    pub personal_writes: bool,
    pub external_mounts: bool,
    pub personal_package_imports: bool,
    pub personal_scope_package_exports: bool,
    pub personal_forms_conversion: bool,
    pub cross_workspace_forms_conversion: bool,
}
pub async fn capabilities<C: ConnectionTrait>(db: &C) -> Result<Capabilities, ApiError> {
    Capabilities::find_by_statement(Statement::from_string(
        DbBackend::Postgres,
        "SELECT block_transfers_enabled,portable_page_packages_enabled,portable_graph_packages_enabled,workspace_editor_initialization,personal_provisioning,personal_writes,external_mounts,personal_package_imports,personal_scope_package_exports,personal_forms_conversion,cross_workspace_forms_conversion FROM flow_runtime_capabilities WHERE singleton".to_string(),
    ))
    .one(db)
    .await?
    .ok_or_else(ApiError::internal)
}
pub async fn get<C: ConnectionTrait>(db: &C, id: Uuid) -> Result<FlowScope, ApiError> {
    FlowScope::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "SELECT id,kind,owner_user_id,lifecycle_status FROM flow_scopes WHERE id=$1",
        vec![id.into()],
    ))
    .one(db)
    .await?
    .ok_or_else(not_found)
}
pub async fn bound_workspace<C: ConnectionTrait>(db: &C, id: Uuid) -> Result<Option<Uuid>, ApiError> {
    #[derive(FromQueryResult)]
    struct Row {
        workspace_id: Uuid,
    }
    Ok(Row::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "SELECT workspace_id FROM flow_workspace_scope_bindings WHERE scope_id=$1",
        vec![id.into()],
    ))
    .one(db)
    .await?
    .map(|r| r.workspace_id))
}
pub async fn workspace_scope<C: ConnectionTrait>(db: &C, workspace_id: Uuid) -> Result<FlowScope, ApiError> {
    #[derive(FromQueryResult)]
    struct Row {
        scope_id: Uuid,
    }
    let row = Row::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "SELECT scope_id FROM flow_workspace_scope_bindings WHERE workspace_id=$1",
        vec![workspace_id.into()],
    ))
    .one(db)
    .await?
    .ok_or_else(not_found)?;
    get(db, row.scope_id).await
}
pub async fn roots<C: ConnectionTrait>(db: &C, scope_id: Uuid) -> Result<Vec<ScopeRoot>, ApiError> {
    Ok(ScopeRoot::find_by_statement(Statement::from_sql_and_values(DbBackend::Postgres,
 "SELECT id AS object_id,project_id FROM flow_objects WHERE scope_id=$1 AND object_type='navigator' AND parent_id IS NULL ORDER BY project_id NULLS FIRST,id",vec![scope_id.into()])).all(db).await?)
}
fn not_found() -> ApiError {
    ApiError::not_found("flow resource not found")
}
fn disabled(reason: &str) -> ApiError {
    ApiError::typed_with_details(
        crate::error::ApiErrorKind::FeatureDisabled,
        "Flow scope rollout unavailable",
        json!({"reason":reason}),
    )
}
/// Refuses a personal-scope write while the operator switch for personal writes is off. Existing
/// reads never depend on creation flags, so containment cannot orphan already accepted content.
pub async fn require_personal_write<C: ConnectionTrait>(db: &C) -> Result<(), ApiError> {
    let caps = capabilities(db).await?;
    if !caps.personal_writes {
        return Err(disabled("personal_creation_disabled"));
    }
    Ok(())
}
/// Concurrent provisioning serializes on the existing user row, then atomically publishes the
/// personal scope, settings, root, document, and projection. Owner FK uses RESTRICT, never CASCADE.
pub async fn personal_home(db: &DatabaseConnection, user: Uuid) -> Result<(FlowScope, Vec<ScopeRoot>), ApiError> {
    let tx = db.begin().await?;

    let user_row = ActiveScopeUser::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "SELECT id FROM users WHERE id=$1 FOR UPDATE",
        vec![user.into()],
    ))
    .one(&tx)
    .await?
    .ok_or_else(not_found)?;
    let _ = user_row.id;
    let existing = FlowScope::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "SELECT id,kind,owner_user_id,lifecycle_status FROM flow_scopes WHERE kind='personal' AND owner_user_id=$1",
        vec![user.into()],
    ))
    .one(&tx)
    .await?;
    let scope = if let Some(scope) = existing {
        scope
    } else {
        if !capabilities(&tx).await?.personal_provisioning {
            return Err(disabled("personal_creation_disabled"));
        }
        require_personal_write(&tx).await?;
        FlowScope::find_by_statement(Statement::from_sql_and_values(DbBackend::Postgres,
   "INSERT INTO flow_scopes(kind,owner_user_id) VALUES('personal',$1) RETURNING id,kind,owner_user_id,lifecycle_status",vec![user.into()])).one(&tx).await?.ok_or_else(ApiError::internal)?
    };
    tx.execute(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "INSERT INTO flow_scope_settings(scope_id) VALUES($1) ON CONFLICT DO NOTHING",
        vec![scope.id.into()],
    ))
    .await?;
    if roots(&tx, scope.id).await?.is_empty() {
        require_personal_write(&tx).await?;
        insert_object(&tx, scope.id, user, "navigator", None, "", None).await?;
    }
    let home_roots = roots(&tx, scope.id).await?;
    tx.commit().await?;
    Ok((scope, home_roots))
}
#[derive(Debug, Clone, FromQueryResult)]
pub struct ObjectIdentity {
    pub id: Uuid,
    pub scope_id: Uuid,
    pub workspace_id: Option<Uuid>,
    pub project_id: Option<Uuid>,
    pub parent_id: Option<Uuid>,
    pub object_type: String,
    pub lifecycle_status: String,
}
pub async fn identity<C: ConnectionTrait>(db: &C, object: Uuid) -> Result<ObjectIdentity, ApiError> {
    ObjectIdentity::find_by_statement(Statement::from_sql_and_values(DbBackend::Postgres,
 "SELECT id,scope_id,workspace_id,project_id,parent_id,object_type,lifecycle_status FROM flow_objects WHERE id=$1",vec![object.into()])).one(db).await?.ok_or_else(not_found)
}
#[derive(Debug, Clone)]
pub struct AuthorizationProof {
    pub scope_id: Uuid,
    pub object_id: Uuid,
    pub principal_id: Uuid,
    pub revision: i64,
    pub level: PermissionLevel,
    pub audience_revisions: Vec<(Uuid, i64)>,
    pub resource_active: bool,
}
#[derive(FromQueryResult)]
struct Revision {
    authz_revision: i64,
}
pub async fn revision<C: ConnectionTrait>(db: &C, scope_id: Uuid) -> Result<i64, ApiError> {
    if let Some(workspace) = bound_workspace(db, scope_id).await? {
        return authz::read_epoch(db, workspace).await;
    }
    Revision::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "SELECT authz_revision FROM flow_scope_settings WHERE scope_id=$1",
        vec![scope_id.into()],
    ))
    .one(db)
    .await?
    .map(|r| r.authz_revision)
    .ok_or_else(not_found)
}
/// A personal mounted audience is never a workspace rescue authority. Resolve only grants on the
/// requested object's in-scope ACL chain; never expose ancestors or siblings as navigation data.
pub async fn authorize<C: ConnectionTrait>(
    db: &C,
    object: Uuid,
    user: Uuid,
    minimum: PermissionLevel,
) -> Result<AuthorizationProof, ApiError> {
    let object_row = identity(db, object).await?;
    let scope = get(db, object_row.scope_id).await?;
    let checked = revision(db, scope.id).await?;
    if let Some(workspace) = object_row.workspace_id {
        #[derive(FromQueryResult)]
        struct Principal {
            role: Option<String>,
            is_bot: bool,
        }
        let principal = Principal::find_by_statement(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT (SELECT role FROM workspace_members WHERE workspace_id=$1 AND user_id=$2) AS role, \
             EXISTS(SELECT 1 FROM users WHERE id=$2 AND entity_type=$3) AS is_bot",
            vec![
                workspace.into(),
                user.into(),
                crate::services::bot_identity::BOT_ENTITY_TYPE.into(),
            ],
        ))
        .one(db)
        .await?;
        let guest = || "__flow_guest".to_string();
        let (role, kind) = match principal {
            // A workspace bot is evaluated as a bot, with the role its token carries (never the one of
            // its membership row): the human-only admin fallback does not apply to it, its grants are
            // `bot` grants, and a project-scoped bot is never an administrator.
            Some(Principal { is_bot: true, .. }) => (
                crate::middleware::bot_auth::bot_token_role(db, user, workspace)
                    .await?
                    .unwrap_or_else(guest),
                "bot",
            ),
            Some(Principal { role, .. }) => (role.unwrap_or_else(guest), "user"),
            None => (guest(), "user"),
        };
        super::policy::require_flow_enabled_on(db, workspace).await?;
        let level = authz::effective_permission(db, workspace, object, kind, user, &role).await?;
        if level < minimum {
            return Err(not_found());
        }
        return Ok(AuthorizationProof {
            scope_id: scope.id,
            object_id: object,
            principal_id: user,
            revision: checked,
            level,
            audience_revisions: Vec::new(),
            resource_active: object_row.lifecycle_status == "active" && scope.lifecycle_status == "active",
        });
    }
    if scope.owner_user_id == Some(user) {
        return Ok(AuthorizationProof {
            scope_id: scope.id,
            object_id: object,
            principal_id: user,
            revision: checked,
            level: PermissionLevel::FullAccess,
            audience_revisions: Vec::new(),
            resource_active: object_row.lifecycle_status == "active" && scope.lifecycle_status == "active",
        });
    }

    let chain = authz::scope_inheritance_chain(db, scope.id, object).await?;
    let end = chain.boundary_index.map_or(chain.ids.len(), |index| index + 1);
    let ids = chain.ids.get(..end).ok_or_else(ApiError::internal)?.to_vec();
    let rows=ScopePermissionGrant::find_by_statement(Statement::from_sql_and_values(DbBackend::Postgres,
      "SELECT g.level,NULL::uuid AS workspace_id,NULL::bigint AS authz_epoch FROM flow_object_grants g WHERE g.object_id=ANY($1) AND g.scope_id=$2 AND g.principal_kind='user' AND g.principal_id=$3 UNION ALL SELECT m.grant_level,m.destination_workspace_id,s.authz_epoch FROM flow_mounts m JOIN workspace_members wm ON wm.workspace_id=m.destination_workspace_id AND wm.user_id=$3 JOIN flow_workspace_settings s ON s.workspace_id=wm.workspace_id JOIN users recipient ON recipient.id=wm.user_id AND recipient.is_active AND recipient.entity_type='human' JOIN flow_objects mounted ON mounted.id=m.source_object_id AND mounted.lifecycle_status='active' WHERE m.source_object_id=ANY($1) AND $4",vec![ids.into(),scope.id.into(),user.into(),(object_row.lifecycle_status == "active" && scope.lifecycle_status == "active").into()])).all(db).await?;
    let mut level = PermissionLevel::Denied;
    let mut audiences = Vec::new();
    for row in rows {
        let granted = PermissionLevel::parse_grant_level(&row.level).ok_or_else(ApiError::internal)?;
        level = level.max(granted);
        if granted >= minimum
            && let (Some(w), Some(e)) = (row.workspace_id, row.authz_epoch)
        {
            audiences.push((w, e));
        }
    }
    if level < minimum {
        return Err(not_found());
    }
    audiences.sort_unstable();
    audiences.dedup();
    if revision(db, scope.id).await? != checked {
        return Err(super::policy::authorization_read_unstable());
    }
    Ok(AuthorizationProof {
        scope_id: scope.id,
        object_id: object,
        principal_id: user,
        revision: checked,
        level,
        audience_revisions: audiences,
        resource_active: object_row.lifecycle_status == "active" && scope.lifecycle_status == "active",
    })
}
/// Stable lock order: audience workspace fences sorted by UUID, then personal scope fence,
/// then object rows/document heads.
///
/// Workspace writes still lock their original workspace row. Membership writers synchronously advance those audience rows in their existing transaction.
pub async fn fence(tx: &DatabaseTransaction, proof: &AuthorizationProof) -> Result<(), ApiError> {
    if let Some(workspace) = bound_workspace(tx, proof.scope_id).await? {
        return authz::fence_epoch_for_share(tx, workspace, proof.revision).await;
    }
    for (workspace, epoch) in &proof.audience_revisions {
        authz::fence_epoch_for_share(tx, *workspace, *epoch).await?;
    }
    let current = Revision::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "SELECT authz_revision FROM flow_scope_settings WHERE scope_id=$1 AND writes_enabled FOR SHARE",
        vec![proof.scope_id.into()],
    ))
    .one(tx)
    .await?
    .ok_or_else(not_found)?;
    if current.authz_revision != proof.revision {
        return Err(ApiError::policy_rejected("scope authorization changed"));
    }
    // Re-evaluate membership/ACL under all held fences. This catches a dependency removed between
    // discovery and lock acquisition without allowing a private ancestor traversal.
    authorize(tx, proof.object_id, proof.principal_id, proof.level).await?;
    Ok(())
}
/// Authority-changing operations acquire their intended exclusive lock immediately, never
/// upgrade a shared lock. Audience dependencies precede the source scope in UUID order.
pub async fn lock_change(tx: &DatabaseTransaction, proof: &AuthorizationProof) -> Result<(), ApiError> {
    for (workspace, epoch) in &proof.audience_revisions {
        authz::fence_epoch_for_share(tx, *workspace, *epoch).await?;
    }
    if let Some(workspace) = bound_workspace(tx, proof.scope_id).await? {
        let current = authz::lock_epoch_for_update(tx, workspace).await?;
        if current != proof.revision {
            return Err(ApiError::policy_rejected("authorization changed"));
        }
    } else {
        let current = Revision::find_by_statement(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT authz_revision FROM flow_scope_settings WHERE scope_id=$1 FOR UPDATE",
            vec![proof.scope_id.into()],
        ))
        .one(tx)
        .await?
        .ok_or_else(not_found)?;
        if current.authz_revision != proof.revision {
            return Err(ApiError::policy_rejected("authorization changed"));
        }
    }
    Ok(())
}
pub async fn proof_current<C: ConnectionTrait>(db: &C, proof: &AuthorizationProof) -> Result<bool, ApiError> {
    if revision(db, proof.scope_id).await? != proof.revision {
        return Ok(false);
    }
    for (workspace, epoch) in &proof.audience_revisions {
        if authz::read_epoch(db, *workspace).await? != *epoch {
            return Ok(false);
        }
    }
    Ok(true)
}
pub async fn advance_revision<C: ConnectionTrait>(tx: &C, scope_id: Uuid) -> Result<i64, ApiError> {
    if let Some(workspace) = bound_workspace(tx, scope_id).await? {
        return authz::advance_epoch(tx, workspace).await;
    }
    Revision::find_by_statement(Statement::from_sql_and_values(DbBackend::Postgres,r"
 WITH advanced AS(UPDATE flow_scope_settings SET authz_revision=authz_revision+1,updated_at=now() WHERE scope_id=$1 RETURNING scope_id,authz_revision), logged AS(INSERT INTO flow_scope_revocations(scope_id,authz_revision) SELECT scope_id,authz_revision FROM advanced RETURNING authz_revision)
 SELECT authz_revision,pg_notify('openpr_flow_scope_authz',json_build_object('scope_id',$1,'authz_revision',authz_revision)::text) FROM logged",vec![scope_id.into()])).one(tx).await?.map(|r|r.authz_revision).ok_or_else(not_found)
}
#[derive(FromQueryResult)]
struct PersonalView {
    id: Uuid,
    scope_id: Uuid,
    workspace_id: Option<Uuid>,
    project_id: Option<Uuid>,
    parent_id: Option<Uuid>,
    object_type: String,
    lifecycle_status: String,
    governance_metadata: Value,
    appearance: Value,
    created_at: Timestamp,
    updated_at: Timestamp,
    archived_at: Option<Timestamp>,
    document_id: Uuid,
    document_seq: i64,
    frontier: Vec<u8>,
    title: String,
    semantic_content: Value,
    projection_seq: i64,
}
pub async fn object_view<C: ConnectionTrait>(db: &C, object: Uuid, user: Uuid) -> Result<Value, ApiError> {
    let proof = authorize(db, object, user, PermissionLevel::View).await?;
    let row=PersonalView::find_by_statement(Statement::from_sql_and_values(DbBackend::Postgres,r"
 SELECT o.id,o.scope_id,o.workspace_id,o.project_id,o.parent_id,o.object_type,o.lifecycle_status,o.governance_metadata,o.appearance,o.created_at,o.updated_at,o.archived_at,d.id AS document_id,d.head_seq AS document_seq,d.head_frontier AS frontier,p.title,p.state AS semantic_content,p.document_seq AS projection_seq
 FROM flow_objects o JOIN collab_documents d ON d.object_id=o.id JOIN flow_object_projections p ON p.object_id=o.id WHERE o.id=$1",vec![object.into()])).one(db).await?.ok_or_else(not_found)?;
    let actions = super::collab::frame::permitted_actions(proof.level, proof.resource_active);
    let mut value = json!({"id":row.id,"scope_id":row.scope_id,"project_id":row.project_id,"parent_id":row.parent_id,"object_type":row.object_type,"lifecycle_status":row.lifecycle_status,"governance_metadata":row.governance_metadata,"appearance":row.appearance,"title":row.title,"semantic_content":row.semantic_content,"document_id":row.document_id,"document_seq":row.document_seq,"frontier":projection::encode_frontier(&collab_core::Frontier::from_bytes(row.frontier)),"projection_seq":row.projection_seq,"projection_lag":row.document_seq-row.projection_seq,"created_at":row.created_at.to_rfc3339(),"updated_at":row.updated_at.to_rfc3339(),"archived_at":row.archived_at.map(platform::timestamp::Timestamp::to_rfc3339),"permitted_actions":actions});
    let secrecy = super::collections::has_collection_secrecy(db, object).await?
        || !super::collections::secret_record_ids(db, &[object]).await?.is_empty();
    let mut formats = vec!["json", "markdown", "csv"];
    if row.object_type == "record" {
        *value.field_mut("record_context_supported")? = json!(super::scope_record_context::RECORD_CONTEXT_SUPPORTED);
        let parent = super::collections::owning_collection_id(db, object).await?;
        let broad_parent = if let Some(parent) = parent {
            match authorize(db, parent, user, PermissionLevel::View).await {
                Ok(_) => true,
                Err(e)
                    if matches!(
                        e.kind(),
                        crate::error::ApiErrorKind::NotFound | crate::error::ApiErrorKind::Forbidden
                    ) =>
                {
                    false
                }
                Err(e) => return Err(e),
            }
        } else {
            false
        };
        if !broad_parent && !super::scope_record_context::RECORD_CONTEXT_SUPPORTED {
            formats.clear();
            *value.field_mut("export_unavailable_reason")? = json!("record_context_not_ready");
        }
    }
    if !secrecy
        && !matches!(row.object_type.as_str(), "collection" | "record")
        && (row.object_type == "page" || (row.object_type == "navigator" && row.parent_id.is_some()))
    {
        formats.push("package");
    }
    *value.field_mut("package_snapshot_history_retained")? = json!(formats.contains(&"package"));
    *value.field_mut("supported_export_formats")? = json!(formats);
    *value.field_mut("history_export_supported")? = json!(formats.contains(&"package"));
    let forms_ready = if let Some(workspace) = row.workspace_id {
        proof.level >= PermissionLevel::Edit
            && proof.resource_active
            && super::bridge::bridge_enabled(db, workspace).await?
            && db
                .query_one(Statement::from_sql_and_values(
                    DbBackend::Postgres,
                    "SELECT user_id FROM workspace_members WHERE workspace_id=$1 AND user_id=$2",
                    vec![workspace.into(), user.into()],
                ))
                .await?
                .is_some()
    } else {
        false
    };
    let caps = capabilities(db).await?;
    let scoped_forms_ready = proof.level >= PermissionLevel::Edit
        && proof.resource_active
        && if row.workspace_id.is_none() {
            caps.personal_forms_conversion && require_personal_write(db).await.is_ok()
        } else {
            caps.cross_workspace_forms_conversion
        };
    *value.field_mut("supported_forms_conversion")? = json!(forms_ready || scoped_forms_ready);
    *value.field_mut("forms_conversion_destination_mode")? = json!(if scoped_forms_ready {
        "explicit_workspace"
    } else if forms_ready {
        "same_workspace"
    } else {
        "unavailable"
    });
    if let Some(workspace) = row.workspace_id {
        *value.field_mut("workspace_id")? = json!(workspace);
    }
    if row.object_type == "collection" {
        super::collections::redact_collection_content(db, object, value.field_mut("semantic_content")?).await?;
    }
    if row.object_type == "record" && !super::collections::secret_record_ids(db, &[object]).await?.is_empty() {
        *value.field_mut("semantic_content")? = super::collections::redacted_semantic_content();
    }
    // An explicitly shared object does not authorize any ancestor identity or metadata.
    if get(db, proof.scope_id).await?.kind == "personal" && get(db, proof.scope_id).await?.owner_user_id != Some(user) {
        value.as_object_mut().map(|v| v.remove("parent_id"));
    }
    if !proof_current(db, &proof).await? {
        return Err(super::policy::authorization_read_unstable());
    }
    Ok(value)
}
async fn insert_object(
    tx: &DatabaseTransaction,
    scope_id: Uuid,
    user: Uuid,
    kind: &str,
    parent: Option<Uuid>,
    title: &str,
    schema: Option<&Value>,
) -> Result<Uuid, ApiError> {
    insert_object_with_identity(
        tx,
        scope_id,
        user,
        kind,
        parent,
        title,
        schema,
        Uuid::new_v4(),
        Uuid::new_v4(),
    )
    .await
}
#[allow(clippy::too_many_arguments)]
pub(crate) async fn insert_object_with_identity(
    tx: &DatabaseTransaction,
    scope_id: Uuid,
    user: Uuid,
    kind: &str,
    parent: Option<Uuid>,
    title: &str,
    schema: Option<&Value>,
    object: Uuid,
    document: Uuid,
) -> Result<Uuid, ApiError> {
    insert_object_with_identity_model(tx, scope_id, user, kind, parent, title, schema, object, document, 1).await
}
#[allow(clippy::too_many_arguments)]
async fn insert_object_with_identity_model(
    tx: &DatabaseTransaction,
    scope_id: Uuid,
    user: Uuid,
    kind: &str,
    parent: Option<Uuid>,
    title: &str,
    schema: Option<&Value>,
    object: Uuid,
    document: Uuid,
    editor_model: u32,
) -> Result<Uuid, ApiError> {
    let mut engine = LoroCollabEngine::new_empty(rand::random());
    engine
        .set_title(title)
        .map_err(|e| super::command::map_collab_error(&e))?;
    if kind == "page" {
        engine
            .initialize_page_editor()
            .map_err(|e| super::command::map_collab_error(&e))?;
        if editor_model >= 4 {
            engine
                .initialize_page_structure_model(editor_model)
                .map_err(|e| super::command::map_collab_error(&e))?;
        }
    }
    if let Some(schema) = schema {
        super::collections::apply_initial_collection_schema(&mut engine, schema)?;
    }
    let snapshot = engine
        .export_snapshot()
        .map_err(|e| super::command::map_collab_error(&e))?;
    let frontier = engine.frontier().as_bytes().to_vec();
    let semantic = engine
        .semantic_snapshot()
        .map_err(|e| super::command::map_collab_error(&e))?;
    tx.execute(Statement::from_sql_and_values(DbBackend::Postgres,
 "INSERT INTO flow_objects(id,scope_id,workspace_id,object_type,parent_id,created_by,governance_metadata) VALUES($1,$2,NULL,$3,$4,$5,$6)",
 vec![object.into(),scope_id.into(),kind.into(),parent.into(),user.into(),if kind=="navigator" && parent.is_none(){json!({"system_role":repository::NAVIGATOR_ROOT_SYSTEM_ROLE})}else{json!({})}.into()])).await?;
    repository::insert_collab_document(
        tx,
        &repository::NewCollabDocument {
            id: document,
            object_id: object,
            format_version: "loro-1".to_string(),
            snapshot,
            frontier: frontier.clone(),
        },
    )
    .await?;
    repository::insert_projection(
        tx,
        &repository::NewProjection {
            object_id: object,
            document_seq: 0,
            document_frontier: frontier,
            title: title.to_string(),
            state: projection::state_json(&semantic).map_err(|_| ApiError::internal())?,
            plain_text: projection::plain_text(&semantic),
        },
    )
    .await?;
    if kind == "page" {
        super::block_registry::reconcile_accepted(
            tx,
            object,
            0,
            &super::block_registry::prepare(&semantic)?,
            super::block_registry::RegistrationAllowance::Complete,
        )
        .await?;
        super::body_assets::reconcile_accepted(tx, object, 0, &super::body_assets::typed_references(&semantic)?)
            .await?;
    }
    if kind == "collection" {
        super::collections::insert_scope_collection_projection(tx, object, document).await?;
        super::collections::sync_new_collection_projection(tx, object, &engine).await?;
    }
    Ok(object)
}
#[derive(Debug, Deserialize)]
pub struct CreateInput {
    #[serde(default = "default_editor_model")]
    pub editor_model: u32,
    pub scope_id: Uuid,
    pub object_type: String,
    pub parent_object_id: Option<Uuid>,
    pub project_id: Option<Uuid>,
    pub title: String,
    pub idempotency_key: String,
    pub message: Option<String>,
    #[serde(default)]
    pub initial_fields: Vec<Value>,
    pub initial_view: Option<Value>,
}
const fn default_editor_model() -> u32 {
    1
}
pub async fn create_personal(db: &DatabaseConnection, user: Uuid, input: CreateInput) -> Result<Value, ApiError> {
    if !matches!(input.editor_model, 1 | 4 | 5) || (input.editor_model >= 4 && input.object_type != "page") {
        return Err(ApiError::invalid_update(
            "editor_model must be 1 or 4; model4 requires Page",
        ));
    }
    if !matches!(input.object_type.as_str(), "page" | "navigator" | "collection") || input.project_id.is_some() {
        return Err(ApiError::invalid_update(
            "personal creation requires page/navigator/collection with no workspace project",
        ));
    }
    if input.idempotency_key.trim().is_empty() || input.idempotency_key.len() > 128 || input.title.chars().count() > 500
    {
        return Err(ApiError::invalid_update("invalid creation title or idempotency key"));
    }
    let parent = if let Some(parent) = input.parent_object_id {
        parent
    } else {
        roots(db, input.scope_id)
            .await?
            .first()
            .map(|r| r.object_id)
            .ok_or_else(not_found)?
    };
    let proof = authorize(db, parent, user, PermissionLevel::Edit).await?;
    if proof.scope_id != input.scope_id || bound_workspace(db, proof.scope_id).await?.is_some() {
        return Err(not_found());
    }
    let mut body = json!({"operation":"create","object_type":input.object_type,"parent_object_id":parent,"title":input.title.trim(),"message":input.message,"initial_fields":input.initial_fields,"initial_view":input.initial_view,"editor_model":input.editor_model});
    if input.editor_model == 1
        && let Some(body) = body.as_object_mut()
    {
        body.remove("editor_model");
    }
    let tx = db.begin().await?;
    require_personal_write(&tx).await?;
    fence(&tx, &proof).await?;
    if let Some(result) =
        super::scope_commands::lock_and_replay(&tx, input.scope_id, user, &input.idempotency_key, &body).await?
    {
        tx.commit().await?;
        return Ok(result);
    }
    // Serialize scope command identities before replay lookup, including concurrent creates.
    tx.execute(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "SELECT pg_advisory_xact_lock(hashtextextended($1,0))",
        vec![format!("{}:{}", input.scope_id, input.idempotency_key).into()],
    ))
    .await?;

    if let Some(prior)=PriorScopeCreate::find_by_statement(Statement::from_sql_and_values(DbBackend::Postgres,"SELECT id,aggregate_id,metadata FROM business_events WHERE source_scope_kind='flow' AND source_flow_scope_id=$1 AND idempotency_key=$2",vec![input.scope_id.into(),input.idempotency_key.clone().into()])).one(&tx).await?{
  if prior.metadata.get("idempotency_body")!=Some(&body){return Err(ApiError::idempotency_key_body_drift("creation request changed"));}
  let object=Uuid::parse_str(&prior.aggregate_id).map_err(|_|ApiError::internal())?;
  let view=object_view(&tx,object,user).await?;tx.commit().await?;return Ok(accepted(view,prior.id));
 }
    authz::ensure_scope_parent_can_adopt_child(&tx, input.scope_id, parent).await?;
    let parent_row = identity(&tx, parent).await?;
    if parent_row.lifecycle_status != "active" {
        return Err(ApiError::policy_rejected("parent is archived"));
    }
    let schema = (!input.initial_fields.is_empty() || input.initial_view.is_some())
        .then(|| json!({"initial_fields":input.initial_fields,"initial_view":input.initial_view}));
    if schema.is_some() && input.object_type != "collection" {
        return Err(ApiError::invalid_update("collection schema requires collection"));
    }
    let object = insert_object_with_identity_model(
        &tx,
        input.scope_id,
        user,
        &input.object_type,
        Some(parent),
        input.title.trim(),
        schema.as_ref(),
        Uuid::new_v4(),
        Uuid::new_v4(),
        input.editor_model,
    )
    .await?;
    let event = super::scope_event::insert(
        &tx,
        input.scope_id,
        user,
        "flow.object.created",
        "flow_object",
        object,
        &input.idempotency_key,
        json!({"object_id":object}),
        json!({"idempotency_body":body}),
        None,
    )
    .await?;
    let view = object_view(&tx, object, user).await?;
    let result = accepted(view, event);
    super::scope_commands::record(
        &tx,
        input.scope_id,
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
pub fn accepted(object: Value, event_id: Uuid) -> Value {
    let mut result = json!({"accepted_seq":object.field("document_seq"),"head_frontier":object.field("frontier"),"projection_seq":object.field("projection_seq"),"semantic_diff":{},"affected_object_ids":[object.field("id")],"event_id":event_id});
    if let Value::Object(fields) = &mut result {
        fields.insert("object".into(), object);
    }
    result
}

/// Fences an export's complete resource impact set: all sorted audience authorities first, then
/// sorted personal scope authorities, then reauthorization. No scope→new-audience lock inversion.
pub async fn fence_many(tx: &DatabaseTransaction, proofs: &[AuthorizationProof]) -> Result<(), ApiError> {
    fence_many_with_workspaces(tx, proofs, &[]).await
}
pub async fn fence_many_with_workspaces(
    tx: &DatabaseTransaction,
    proofs: &[AuthorizationProof],
    extra: &[(Uuid, i64)],
) -> Result<(), ApiError> {
    let mut audiences = std::collections::BTreeMap::new();
    for (id, revision) in extra {
        if audiences.insert(*id, *revision).is_some_and(|old| old != *revision) {
            return Err(ApiError::policy_rejected(
                "destination authority changed during preparation",
            ));
        }
    }
    let mut scopes = std::collections::BTreeMap::new();
    for proof in proofs {
        if let Some(previous) = scopes.insert(proof.scope_id, proof.revision)
            && previous != proof.revision
        {
            return Err(ApiError::policy_rejected(
                "export authorization changed during preparation",
            ));
        }
        for (id, revision) in &proof.audience_revisions {
            if let Some(previous) = audiences.insert(*id, *revision)
                && previous != *revision
            {
                return Err(ApiError::policy_rejected("export audience changed during preparation"));
            }
        }
    }
    let mut workspace_scopes = std::collections::BTreeSet::new();
    for (scope, revision) in &scopes {
        if let Some(workspace) = bound_workspace(tx, *scope).await? {
            workspace_scopes.insert(*scope);
            if let Some(previous) = audiences.insert(workspace, *revision)
                && previous != *revision
            {
                return Err(ApiError::policy_rejected(
                    "workspace authority changed during preparation",
                ));
            }
        }
    }
    for (audience, revision) in audiences {
        authz::fence_epoch_for_share(tx, audience, revision).await?;
    }
    for (scope, revision) in scopes {
        if workspace_scopes.contains(&scope) {
            continue;
        }
        let current = Revision::find_by_statement(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT authz_revision FROM flow_scope_settings WHERE scope_id=$1 FOR SHARE",
            vec![scope.into()],
        ))
        .one(tx)
        .await?
        .ok_or_else(not_found)?;
        if current.authz_revision != revision {
            return Err(ApiError::policy_rejected("export authorization changed"));
        }
    }
    for proof in proofs {
        authorize(tx, proof.object_id, proof.principal_id, proof.level).await?;
    }
    Ok(())
}

/// A reservation owns no Page until the source accepted-content transaction calls this.
pub(crate) async fn insert_child_page(
    tx: &DatabaseTransaction,
    parent: &ObjectIdentity,
    actor: Uuid,
    object: Uuid,
    title: &str,
) -> Result<(), ApiError> {
    authz::ensure_scope_parent_can_adopt_child(tx, parent.scope_id, parent.id).await?;
    if parent.workspace_id.is_none() {
        insert_object_with_identity_model(
            tx,
            parent.scope_id,
            actor,
            "page",
            Some(parent.id),
            title,
            None,
            object,
            Uuid::new_v4(),
            5,
        )
        .await?;
        return Ok(());
    }
    let mut engine = LoroCollabEngine::new_empty(rand::random());
    engine
        .set_title(title)
        .map_err(|error| super::command::map_collab_error(&error))?;
    engine
        .initialize_page_editor()
        .map_err(|error| super::command::map_collab_error(&error))?;
    engine
        .initialize_page_structure_model(5)
        .map_err(|error| super::command::map_collab_error(&error))?;
    let document = Uuid::new_v4();
    let snapshot = engine
        .export_snapshot()
        .map_err(|error| super::command::map_collab_error(&error))?;
    let frontier = engine.frontier().as_bytes().to_vec();
    let semantic = engine
        .semantic_snapshot()
        .map_err(|error| super::command::map_collab_error(&error))?;
    tx.execute(Statement::from_sql_and_values(DbBackend::Postgres,"INSERT INTO flow_objects(id,scope_id,workspace_id,project_id,object_type,parent_id,created_by,governance_metadata) VALUES($1,$2,$3,$4,'page',$5,$6,'{}'::jsonb)",vec![object.into(),parent.scope_id.into(),parent.workspace_id.into(),parent.project_id.into(),parent.id.into(),actor.into()])).await?;
    repository::insert_collab_document(
        tx,
        &repository::NewCollabDocument {
            id: document,
            object_id: object,
            format_version: "loro-1".into(),
            snapshot,
            frontier: frontier.clone(),
        },
    )
    .await?;
    repository::insert_projection(
        tx,
        &repository::NewProjection {
            object_id: object,
            document_seq: 0,
            document_frontier: frontier,
            title: title.into(),
            state: projection::state_json(&semantic).map_err(|_| ApiError::internal())?,
            plain_text: String::new(),
        },
    )
    .await?;
    super::block_registry::reconcile_accepted(
        tx,
        object,
        0,
        &super::block_registry::prepare(&semantic)?,
        super::block_registry::RegistrationAllowance::Complete,
    )
    .await?;
    Ok(())
}
