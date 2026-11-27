//! Scope-native transport. Legacy resources delegate business commands to the existing service.
use crate::flow::json_value::FlowJson;
use crate::middleware::bot_auth::BotAuthContext;
use crate::{
    error::ApiError,
    flow::{
        collab::{authz::PermissionLevel, write::AcceptOutcome},
        scope, scope_write,
    },
    response::ApiResponse,
};
use axum::{
    Extension, Json,
    extract::{Path, Query, State},
    response::{IntoResponse, Response},
};
use base64::{Engine, engine::general_purpose::STANDARD as BASE64};
use collab_core::CollabEngine;
use platform::{app::AppState, auth::JwtClaims};
use sea_orm::{ConnectionTrait, DbBackend, FromQueryResult, Statement, TransactionTrait};
use serde::Deserialize;
use serde_json::{Value, json};
use uuid::Uuid;

/// The human principal of a request. A workspace bot is refused: the caller is on a surface that only
/// a person reaches (a personal scope, a collaboration ticket, a bootstrap).
pub(super) fn user(claims: &JwtClaims, bot: Option<&Extension<BotAuthContext>>) -> Result<Uuid, ApiError> {
    if bot.is_some() {
        return Err(ApiError::unsupported_protocol(
            "this Flow operation does not accept workspace bot credentials",
        ));
    }
    Uuid::parse_str(&claims.sub).map_err(|_| ApiError::unauthenticated("invalid user identity"))
}

/// What a request names, to find the workspace a bot is judged against.
#[derive(Clone, Copy)]
pub(super) enum Subject {
    Object(Uuid),
    Scope(Uuid),
}

/// The principal of a request on workspace-bound content.
///
/// A person is the user of the token. A workspace bot is accepted only on a target bound to a
/// workspace: the bot's workspace and `flow_enabled` are checked with the same policy the workspace
/// handlers use, and the actor is the bot's own user id, so events and audit name the bot. A personal
/// scope is never reachable with a bot credential, and a target of another workspace is refused as
/// absent when it names an object.
pub(super) async fn principal(
    state: &AppState,
    claims: &JwtClaims,
    bot: Option<&Extension<BotAuthContext>>,
    subject: Subject,
) -> Result<Uuid, ApiError> {
    let Some(Extension(context)) = bot else {
        return user(claims, None);
    };
    let workspace = match subject {
        Subject::Object(object) => scope::identity(&state.db, object).await?.workspace_id,
        Subject::Scope(scope_id) => scope::legacy_workspace(&state.db, scope_id).await?,
    };
    let Some(workspace) = workspace else {
        return Err(ApiError::unsupported_protocol(
            "workspace bot credentials do not reach personal Flow scopes",
        ));
    };
    let mut extensions = axum::http::Extensions::new();
    extensions.insert(claims.clone());
    extensions.insert(context.clone());
    let access = crate::flow::policy::require_flow_workspace_access(state, &extensions, workspace).await;
    match (subject, access) {
        (Subject::Object(_), Err(err)) => Err(crate::flow::policy::collapse_object_denial(err)),
        (_, Err(err)) => Err(err),
        (_, Ok((actor, _, _))) => Ok(actor),
    }
}
pub async fn get_flow_v2_capabilities(State(state): State<AppState>) -> Result<impl IntoResponse, ApiError> {
    Ok(ApiResponse::success(scope::capabilities(&state.db).await?))
}
pub async fn get_flow_v2_home(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    bot: Option<Extension<BotAuthContext>>,
) -> Result<impl IntoResponse, ApiError> {
    let user = user(&claims, bot.as_ref())?;
    let (scope, roots) = scope::personal_home(&state.db, user).await?;
    Ok(ApiResponse::success(
        json!({"scope":scope,"roots":roots,"capabilities":scope::capabilities(&state.db).await?}),
    ))
}
pub async fn get_flow_v2_workspace_scope(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    bot: Option<Extension<BotAuthContext>>,
    Path(workspace): Path<Uuid>,
) -> Result<impl IntoResponse, ApiError> {
    let extensions = super::build_auth_extensions(claims, bot);
    crate::flow::policy::require_flow_workspace_access(&state, &extensions, workspace).await?;
    let scope = scope::legacy_scope(&state.db, workspace).await?;
    let roots = scope::roots(&state.db, scope.id).await?;
    Ok(ApiResponse::success(json!({"scope":scope,"roots":roots})))
}
pub async fn get_flow_v2_object(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    bot: Option<Extension<BotAuthContext>>,
    Path(object): Path<Uuid>,
    Query(params): Query<super::GetFlowObjectQuery>,
) -> Result<impl IntoResponse, ApiError> {
    let user = principal(&state, &claims, bot.as_ref(), Subject::Object(object)).await?;
    let mut view = if let Some(seq) = params.at_seq {
        crate::flow::scope_history::object_at(&state, object, user, seq).await?
    } else {
        scope::object_view(&state.db, object, user).await?
    };
    if view.field("object_type") == "record" {
        if params.at_seq.is_none() {
            view = crate::flow::scope_record_context::read_for_export(&state.db, object, user)
                .await?
                .record;
        } else {
            crate::flow::scope_record_context::sanitize_record_view(&state.db, object, user, &mut view).await?;
        }
    } else if params.at_seq.is_none() {
        crate::flow::scope_editor::decorate(&state.db, object, user, &mut view).await?;
    }
    if let Some(render) = params.render
        && render == "markdown"
    {
        let semantic: collab_core::SemanticSnapshot =
            serde_json::from_value(view.field("semantic_content").clone()).map_err(|_| ApiError::internal())?;
        *view.field_mut("rendered_markdown")? = json!(crate::flow::projection::render_markdown_for_object(
            view.field("title").as_str().unwrap_or(""),
            &semantic,
            object
        ));
    }
    Ok(ApiResponse::success(view))
}
pub async fn post_flow_v2_object(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    bot: Option<Extension<BotAuthContext>>,
    Json(input): Json<scope::CreateInput>,
) -> Result<Response, ApiError> {
    if let Some(workspace) = scope::legacy_workspace(&state.db, input.scope_id).await? {
        let extensions = super::build_auth_extensions(claims, bot);
        let (actor, _, is_bot) =
            crate::flow::policy::require_flow_workspace_access(&state, &extensions, workspace).await?;
        let legacy = crate::flow::command::CreateObjectInput {
            workspace_id: workspace,
            actor_id: actor,
            actor_is_bot: is_bot,
            object_type: input.object_type,
            project_id: input.project_id,
            parent_object_id: input.parent_object_id,
            title: input.title,
            idempotency_key: input.idempotency_key,
            message: input.message,
            origin: super::request_origin(&extensions),
        };
        let accepted = if input.initial_fields.is_empty() && input.initial_view.is_none() {
            crate::flow::command::create_object_with_editor_model(&state, legacy, input.editor_model).await?
        } else {
            crate::flow::command::create_object_with_collection_schema(
                &state,
                legacy,
                json!({"initial_fields":input.initial_fields,"initial_view":input.initial_view}),
            )
            .await?
        };
        let mut value = serde_json::to_value(accepted).map_err(|_| ApiError::internal())?;
        *value.field_mut("object")? = scope::object_view(
            &state.db,
            value
                .field("object")
                .field("id")
                .as_str()
                .and_then(|s| Uuid::parse_str(s).ok())
                .ok_or_else(ApiError::internal)?,
            actor,
        )
        .await?;
        return Ok(ApiResponse::success(value).into_response());
    }
    let actor = user(&claims, bot.as_ref())?;
    Ok(ApiResponse::success(scope::create_personal(&state.db, actor, input).await?).into_response())
}
#[derive(Default, Deserialize)]
pub struct ScopeListQuery {
    pub parent_id: Option<Uuid>,
    pub project_id: Option<Uuid>,
    #[serde(default)]
    pub unprojected: bool,
    pub limit: Option<u64>,
    pub cursor: Option<String>,
    pub q: Option<String>,
    #[serde(default)]
    pub include_archived: bool,
    pub object_type: Option<String>,
    pub depth: Option<u64>,
}
#[derive(FromQueryResult)]
struct ObjectId {
    id: Uuid,
}
async fn accessible_scope(state: &AppState, scope_id: Uuid, user: Uuid) -> Result<scope::FlowScope, ApiError> {
    let scope = scope::get(&state.db, scope_id).await?;
    if scope.kind == "personal" && scope.owner_user_id != Some(user) {
        return Err(ApiError::not_found("flow scope not found"));
    }
    if let Some(workspace) = scope::legacy_workspace(&state.db, scope_id).await? {
        #[derive(FromQueryResult)]
        struct Member {
            present: bool,
        }
        let member = Member::find_by_statement(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT EXISTS(SELECT 1 FROM workspace_members WHERE workspace_id=$1 AND user_id=$2) AS present",
            vec![workspace.into(), user.into()],
        ))
        .one(&state.db)
        .await?
        .is_some_and(|m| m.present);
        if !member {
            return Err(ApiError::not_found("flow scope not found"));
        }
        crate::flow::policy::require_flow_enabled(state, workspace).await?;
    }
    Ok(scope)
}
pub async fn get_flow_v2_objects(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    bot: Option<Extension<BotAuthContext>>,
    Path(scope_id): Path<Uuid>,
    Query(params): Query<ScopeListQuery>,
) -> Result<impl IntoResponse, ApiError> {
    if params.depth.is_some() {
        return Err(ApiError::invalid_update("depth applies only to navigator queries"));
    }
    let actor = principal(&state, &claims, bot.as_ref(), Subject::Scope(scope_id)).await?;
    if params.unprojected && params.project_id.is_some() {
        return Err(ApiError::invalid_update(
            "project_id and unprojected are mutually exclusive",
        ));
    }
    if let Some(parent) = params.parent_id {
        let proof = scope::authorize(&state.db, parent, actor, PermissionLevel::View).await?;
        if proof.scope_id != scope_id {
            return Err(ApiError::not_found("flow parent not found"));
        }
    } else {
        accessible_scope(&state, scope_id, actor).await?;
    }

    let limit = params.limit.unwrap_or(50).clamp(1, 100);
    let cursor = params
        .cursor
        .as_deref()
        .map(Uuid::parse_str)
        .transpose()
        .map_err(|_| ApiError::invalid_update("invalid object cursor"))?;
    let rows=ObjectId::find_by_statement(Statement::from_sql_and_values(DbBackend::Postgres,"SELECT o.id FROM flow_objects o JOIN flow_object_projections p ON p.object_id=o.id WHERE o.scope_id=$1 AND NOT flow_is_system_navigator_root(o.object_type,o.parent_id,o.governance_metadata) AND ($2::uuid IS NULL OR o.parent_id=$2) AND ($3::uuid IS NULL OR o.id>$3) AND ($4 OR o.lifecycle_status='active') AND ($5::text IS NULL OR o.object_type=$5) AND ($6::uuid IS NULL OR o.project_id=$6) AND ($7::text IS NULL OR p.title ILIKE '%'||$7||'%') AND (NOT $9::boolean OR o.project_id IS NULL) ORDER BY o.id LIMIT $8",vec![scope_id.into(),params.parent_id.into(),cursor.into(),params.include_archived.into(),params.object_type.into(),params.project_id.into(),params.q.into(),10001_i64.into(),params.unprojected.into()])).all(&state.db).await?;
    let mut items = Vec::new();
    let mut next = None;
    let scanned = rows.len();
    let mut last_scanned = None;
    for row in rows {
        if items.len() >= usize::try_from(limit).unwrap_or(100) {
            next = last_scanned.map(|id: Uuid| id.to_string());
            break;
        }
        last_scanned = Some(row.id);
        match scope::object_view(&state.db, row.id, actor).await {
            Ok(view) => items.push(view),
            Err(err) if err.kind() == crate::error::ApiErrorKind::NotFound => {}
            Err(err) => return Err(err),
        }
    }
    if next.is_none() && scanned >= 10001 {
        next = last_scanned.map(|id| id.to_string());
    }
    Ok(ApiResponse::success(json!({"items":items,"next_cursor":next})))
}
pub async fn get_flow_v2_navigator(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    bot: Option<Extension<BotAuthContext>>,
    Path(scope_id): Path<Uuid>,
    Query(params): Query<ScopeListQuery>,
) -> Result<impl IntoResponse, ApiError> {
    let depth = params.depth.unwrap_or(crate::flow::query::MAX_NAVIGATOR_DEPTH);
    if !(1..=crate::flow::query::MAX_NAVIGATOR_DEPTH).contains(&depth) {
        return Err(ApiError::invalid_update("navigator depth must be1..20"));
    }
    if params.parent_id.is_some()
        || params.unprojected
        || params.limit.is_some()
        || params.cursor.is_some()
        || params.q.is_some()
        || params.object_type.is_some()
    {
        return Err(ApiError::invalid_update(
            "navigator supports only project_id, depth and include_archived",
        ));
    }
    let actor = principal(&state, &claims, bot.as_ref(), Subject::Scope(scope_id)).await?;
    accessible_scope(&state, scope_id, actor).await?;
    let roots = scope::roots(&state.db, scope_id).await?;
    let root = roots
        .iter()
        .find(|r| r.project_id == params.project_id)
        .ok_or_else(|| ApiError::not_found("navigator root not found"))?
        .object_id;
    let root_proof = scope::authorize(&state.db, root, actor, PermissionLevel::View).await?;
    let root_view = scope::object_view(&state.db, root, actor).await?;
    let positions = crate::flow::scope_move::navigator_positions(
        &state.db,
        root,
        root_view
            .field("document_seq")
            .as_i64()
            .ok_or_else(ApiError::internal)?,
    )
    .await?;
    let mut nodes = Vec::new();
    let mut external_proofs = Vec::new();
    // Assemble only policy-visible nodes. Source parents are canonical inside an owned scope.
    let rows=ObjectId::find_by_statement(Statement::from_sql_and_values(DbBackend::Postgres,"WITH RECURSIVE tree AS(SELECT id,parent_id,lifecycle_status,created_at,ARRAY[id]::uuid[] AS path,1::bigint AS depth FROM flow_objects WHERE scope_id=$1 AND parent_id=$2 UNION ALL SELECT c.id,c.parent_id,c.lifecycle_status,c.created_at,t.path||c.id,t.depth+1 FROM tree t JOIN flow_objects c ON c.parent_id=t.id AND c.scope_id=$1 WHERE t.depth<$3 AND NOT c.id=ANY(t.path)) SELECT id FROM tree WHERE $4 OR lifecycle_status='active' ORDER BY created_at,id LIMIT 1001",vec![scope_id.into(),root.into(),i64::try_from(depth).unwrap_or(20).into(),params.include_archived.into()])).all(&state.db).await?;
    if rows.len() > 1000 {
        return Err(ApiError::limit_exceeded(
            "scan_budget",
            "navigator scan budget exceeded",
            None,
            None,
            Some(1000),
        ));
    }
    for (fallback_index, row) in rows.into_iter().enumerate() {
        match scope::object_view(&state.db,row.id,actor).await{Ok(view)=>nodes.push(json!({"object_id":view.field("id"),"parent_id":view.field("parent_id"),"position":crate::flow::scope_move::position_for(&positions,row.id,fallback_index)?,"title":view.field("title"),"type":view.field("object_type"),"appearance":view.field("appearance")})),Err(err)if err.kind()==crate::error::ApiErrorKind::NotFound=>{},Err(err)=>return Err(err)}
    }
    if let Some(destination) = scope::legacy_workspace(&state.db, scope_id).await? {
        let mounts=state.db.query_all(Statement::from_sql_and_values(DbBackend::Postgres,"SELECT m.id,m.source_object_id,o.scope_id FROM flow_mounts m JOIN flow_objects o ON o.id=m.source_object_id WHERE m.destination_workspace_id=$1 AND m.destination_project_id IS NOT DISTINCT FROM $2 ORDER BY m.created_at,m.id LIMIT 1000",vec![destination.into(),params.project_id.into()])).await?;
        for mount in mounts {
            let source: Uuid = mount.try_get("", "source_object_id")?;
            let mount_id: Uuid = mount.try_get("", "id")?;
            let source_proof = match scope::authorize(&state.db, source, actor, PermissionLevel::View).await {
                Ok(proof) => proof,
                Err(err) if err.kind() == crate::error::ApiErrorKind::NotFound => continue,
                Err(err) => return Err(err),
            };
            external_proofs.push(source_proof);
            match scope::object_view(&state.db, source, actor).await {
                Ok(view) => {
                    let can_remove =
                        crate::flow::scope_mounts::can_remove(&state.db, destination, source, actor).await?;
                    nodes.push(json!({"object_id":source,"parent_id":root,"position":format!("mount:{mount_id}"),"title":view.field("title"),"type":view.field("object_type"),"appearance":view.field("appearance"),"scope_id":view.field("scope_id"),"mount_id":mount_id,"ownership":"external","mount_status":if view.field("lifecycle_status")=="active"{"active"}else{"source_archived"},"can_remove":can_remove}));
                }
                Err(err) if err.kind() == crate::error::ApiErrorKind::NotFound => {}
                Err(err) => return Err(err),
            }
        }
    }
    for proof in external_proofs {
        if !scope::proof_current(&state.db, &proof).await? {
            return Err(crate::flow::policy::authorization_read_unstable());
        }
    }
    if !scope::proof_current(&state.db, &root_proof).await? {
        return Err(crate::flow::policy::authorization_read_unstable());
    }
    Ok(ApiResponse::success(
        json!({"scope_id":scope_id,"root_object_id":root,"nodes":nodes,"document_seq":root_view.field("document_seq"),"frontier":root_view.field("frontier")}),
    ))
}
#[derive(Deserialize)]
pub struct EditorModelQuery {
    #[serde(default = "default_editor_model")]
    pub editor_model: u32,
}
const fn default_editor_model() -> u32 {
    1
}
pub async fn get_flow_v2_bootstrap(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    bot: Option<Extension<BotAuthContext>>,
    Path(object): Path<Uuid>,
    Query(model): Query<EditorModelQuery>,
) -> Result<impl IntoResponse, ApiError> {
    Ok(ApiResponse::success(
        scope_write::bootstrap_model(&state.db, object, user(&claims, bot.as_ref())?, model.editor_model).await?,
    ))
}
pub async fn post_flow_v2_command(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    bot: Option<Extension<BotAuthContext>>,
    Path(object): Path<Uuid>,
    Json(mut req): Json<super::ExecuteFlowCommandRequest>,
) -> Result<impl IntoResponse, ApiError> {
    let actor = principal(&state, &claims, bot.as_ref(), Subject::Object(object)).await?;
    let identity = scope::identity(&state.db, object).await?;
    req.command.payload = crate::flow::body_assets::normalize_command_payload(
        &state.db,
        object,
        actor,
        &req.command.command_type,
        &req.command.payload,
    )
    .await?;
    if identity.object_type == "record"
        && matches!(req.command.command_type.as_str(), "record_patch" | "record_archive")
    {
        return Ok(ApiResponse::success(
            crate::flow::scope_record_context::execute(
                &state,
                object,
                actor,
                &req.command.command_type,
                req.command.payload,
                &req.idempotency_key,
                req.expected_frontier.as_deref(),
                req.message.as_deref(),
            )
            .await?,
        ));
    }
    if req.command.command_type == "initialize_editor_doc" {
        let expected = req
            .expected_frontier
            .as_deref()
            .ok_or_else(|| ApiError::invalid_update("expected_frontier is required for editor initialization"))?;
        return Ok(ApiResponse::success(
            crate::flow::scope_editor::initialize(
                &state.db,
                object,
                actor,
                req.command.payload,
                expected,
                &req.idempotency_key,
            )
            .await?,
        ));
    }
    if let Some(workspace) = identity.workspace_id {
        let extensions = super::build_auth_extensions(claims, bot);
        let (_, role, is_bot) = crate::flow::policy::resolve_flow_principal(&state, &extensions, workspace).await?;
        let result = crate::flow::command::execute_command(
            &state,
            crate::flow::command::ExecuteCommandInput {
                object_id: object,
                actor_id: actor,
                principal_kind: if is_bot { "bot" } else { "user" }.to_string(),
                role,
                command_type: req.command.command_type,
                payload: req.command.payload,
                expected_frontier: req.expected_frontier,
                idempotency_key: req.idempotency_key,
                message: req.message,
                origin_client_id: format!("rest:{actor}"),
                origin: super::request_origin(&extensions),
            },
        )
        .await?;
        let mut result = serde_json::to_value(result).map_err(|_| ApiError::internal())?;
        *result.field_mut("object")? = scope::object_view(&state.db, object, actor).await?;
        return Ok(ApiResponse::success(result));
    }
    if req.command.command_type == "move_object" {
        return Ok(ApiResponse::success(
            crate::flow::scope_move::execute(
                &state,
                object,
                actor,
                req.command.payload,
                &req.idempotency_key,
                req.expected_frontier.as_deref(),
            )
            .await?,
        ));
    }
    if matches!(req.command.command_type.as_str(), "link" | "unlink") {
        return Ok(ApiResponse::success(
            crate::flow::scope_relations::execute(
                &state,
                object,
                actor,
                &req.command.command_type,
                req.command.payload,
                &req.idempotency_key,
                req.expected_frontier.as_deref(),
            )
            .await?,
        ));
    }
    if let Some(kind) = crate::flow::collections::CollectionCommandType::parse(&req.command.command_type) {
        return Ok(ApiResponse::success(
            crate::flow::scope_collections::execute(
                &state,
                object,
                actor,
                kind,
                req.command.payload,
                &req.idempotency_key,
                req.expected_frontier.as_deref(),
                req.message.as_deref(),
            )
            .await?,
        ));
    }
    let command_body = json!({"object_id":object,"command":{"type":req.command.command_type,"payload":req.command.payload},"expected_frontier":req.expected_frontier,"message":req.message});
    scope::authorize(&state.db, object, actor, PermissionLevel::Edit).await?;
    if let Some(replay) = crate::flow::scope_write::command_replay(
        &state.db,
        identity.scope_id,
        actor,
        &req.idempotency_key,
        &command_body,
    )
    .await?
    {
        return Ok(ApiResponse::success(replay));
    }
    if matches!(req.command.command_type.as_str(), "archive" | "restore") {
        let required = crate::flow::command::scope_lifecycle_permission(&identity.object_type, identity.parent_id);
        let proof = scope::authorize(&state.db, object, actor, required).await?;
        let tx = state.db.begin().await?;
        scope::require_personal_write(&tx).await?;
        scope::lock_change(&tx, &proof).await?;
        if let Some(result) = crate::flow::scope_commands::lock_and_replay(
            &tx,
            identity.scope_id,
            actor,
            &req.idempotency_key,
            &command_body,
        )
        .await?
        {
            tx.commit().await?;
            return Ok(ApiResponse::success(result));
        }
        scope::authorize(&tx, object, actor, required).await?;
        let archived = req.command.command_type == "archive";
        let current = scope::identity(&tx, object).await?;
        if (current.lifecycle_status == "archived") == archived {
            return Err(ApiError::invalid_update(
                "object is already in the requested lifecycle state",
            ));
        }
        crate::flow::repository::set_objects_lifecycle(
            &tx,
            &[object],
            if archived { "archived" } else { "active" },
            if archived {
                Some(platform::timestamp::Timestamp::now())
            } else {
                None
            },
            Some(actor),
        )
        .await?;
        scope::advance_revision(&tx, identity.scope_id).await?;
        let event = crate::flow::scope_event::insert(
            &tx,
            identity.scope_id,
            actor,
            if archived {
                "flow.object.archived"
            } else {
                "flow.object.restored"
            },
            "flow_object",
            object,
            &req.idempotency_key,
            json!({"object_id":object,"status":if archived{"archived"}else{"active"}}),
            json!({"idempotency_body":command_body}),
            None,
        )
        .await?;
        let view = scope::object_view(&tx, object, actor).await?;
        let result = scope::accepted(view, event);
        crate::flow::scope_commands::record(
            &tx,
            identity.scope_id,
            actor,
            &req.idempotency_key,
            command_body,
            object,
            event,
            result.clone(),
        )
        .await?;
        tx.commit().await?;
        return Ok(ApiResponse::success(result));
    }
    let view = scope::object_view(&state.db, object, actor).await?;
    let document = Uuid::parse_str(view.field("document_id").as_str().ok_or_else(ApiError::internal)?)
        .map_err(|_| ApiError::internal())?;
    let mut engine = crate::flow::document::load_head_engine(&state.db, document).await?;
    let before = engine.frontier();
    if identity.object_type == "page" && req.command.command_type != "set_title" {
        engine
            .initialize_page_editor()
            .map_err(|e| crate::flow::command::map_collab_error(&e))?;
    }
    crate::flow::command::apply_scope_content_command(&mut engine, &req.command.command_type, &req.command.payload)?;
    let bytes = engine
        .export_from(&before)
        .map_err(|e| crate::flow::command::map_collab_error(&e))?;
    let update = crate::flow::collab::write::replay_stable_update_id(document, &req.idempotency_key);
    let expected = req
        .expected_frontier
        .as_deref()
        .map(|s| {
            BASE64
                .decode(s)
                .map_err(|_| ApiError::invalid_update("invalid expected frontier"))
        })
        .transpose()?;
    let result = scope_write::accept(
        &state.db,
        object,
        bytes,
        scope_write::UpdateIntent {
            user: actor,
            update,
            client: &format!("rest:{actor}"),
            key: Some(&req.idempotency_key),
            expected: expected.as_deref(),
            command_body: Some(command_body),
        },
    )
    .await?;
    match result {
        AcceptOutcome::Accepted(accepted) => Ok(ApiResponse::success(scope::accepted(
            scope::object_view(&state.db, object, actor).await?,
            accepted.event_id,
        ))),
        AcceptOutcome::Rejected(rejected) => Err(crate::flow::scope_write::map_rejection(&rejected)),
    }
}

#[derive(Deserialize)]
pub struct ScopeTicketRequest {
    #[serde(default = "default_editor_model")]
    editor_model: u32,
    object_id: Uuid,
    document_id: Uuid,
    client_id: Uuid,
    origin: String,
    #[serde(default)]
    authorization_notifications: bool,
}
pub async fn post_flow_v2_ticket(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    bot: Option<Extension<BotAuthContext>>,
    Json(req): Json<ScopeTicketRequest>,
) -> Result<impl IntoResponse, ApiError> {
    scope::authorize(
        &state.db,
        req.object_id,
        user(&claims, bot.as_ref())?,
        crate::flow::collab::authz::PermissionLevel::View,
    )
    .await?;
    crate::flow::scope_editor::require_model(&state.db, req.object_id, req.editor_model).await?;
    Ok(ApiResponse::success(
        crate::flow::scope_collab::issue_with_options(
            &state.db,
            user(&claims, bot.as_ref())?,
            req.object_id,
            req.document_id,
            req.client_id,
            &req.origin,
            &state.cfg.collab_allowed_origins,
            req.authorization_notifications,
        )
        .await?,
    ))
}
/// The upgrade query: the one-time ticket and the UUID the ticket was issued for.
#[derive(Deserialize)]
pub struct ScopeWsQuery {
    ticket: String,
    client_id: Uuid,
}
pub async fn get_flow_v2_ws(
    ws: axum::extract::ws::WebSocketUpgrade,
    State(state): State<AppState>,
    params: Result<Query<ScopeWsQuery>, axum::extract::rejection::QueryRejection>,
    headers: axum::http::HeaderMap,
) -> Response {
    // A client id that is not a UUID cannot belong to any ticket: refuse it with the frame every
    // other handshake failure uses.
    let Ok(Query(params)) = params else {
        return ApiError::unauthenticated("invalid ticket").into_response();
    };
    let origin = headers
        .get(axum::http::header::ORIGIN)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    match crate::flow::scope_collab::consume(&state.db, &params.ticket, params.client_id, origin).await {
        Ok(context) => ws
            .max_message_size(crate::flow::collab::limits::WEBSOCKET_FRAME_BYTES_MAX)
            .max_frame_size(crate::flow::collab::limits::WEBSOCKET_FRAME_BYTES_MAX)
            .on_upgrade(move |socket| crate::flow::scope_collab::run(socket, state, context))
            .into_response(),
        Err(err) => err.into_response(),
    }
}

pub async fn post_flow_v2_purge(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    bot: Option<Extension<BotAuthContext>>,
    Path(object): Path<Uuid>,
    headers: axum::http::HeaderMap,
) -> Result<Response, ApiError> {
    if let Ok(identity) = scope::identity(&state.db, object).await
        && identity.workspace_id.is_some()
    {
        return Ok(
            super::post_flow_purge(State(state), Extension(claims), bot, Path(object))
                .await?
                .into_response(),
        );
    }
    let actor = user(&claims, bot.as_ref())?;
    let default_key = format!("purge:{object}");
    let key = optional_intent(&headers)?.unwrap_or(&default_key);
    Ok(ApiResponse::success(crate::flow::scope_purge::purge(&state, object, actor, key).await?).into_response())
}

pub async fn post_flow_v2_restore_version(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    bot: Option<Extension<BotAuthContext>>,
    Path(object): Path<Uuid>,
    Json(body): Json<super::flow_page::RestoreVersionBody>,
) -> Result<Response, ApiError> {
    if scope::identity(&state.db, object).await?.workspace_id.is_some() {
        return Ok(
            super::post_flow_restore_version(State(state), Extension(claims), bot, Path(object), Json(body))
                .await?
                .into_response(),
        );
    }
    let actor = user(&claims, bot.as_ref())?;
    Ok(ApiResponse::success(
        crate::flow::scope_history::restore_page(&state, object, actor, body.seq, &body.idempotency_key, body.message)
            .await?,
    )
    .into_response())
}

pub async fn get_flow_v2_diff(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    bot: Option<Extension<BotAuthContext>>,
    Path(object): Path<Uuid>,
    Query(params): Query<super::FlowObjectDiffQuery>,
) -> Result<impl IntoResponse, ApiError> {
    let actor = principal(&state, &claims, bot.as_ref(), Subject::Object(object)).await?;
    scope::authorize(&state.db, object, actor, PermissionLevel::View).await?;
    let from = params
        .from_seq
        .ok_or_else(|| ApiError::invalid_update("from_seq is required"))?;
    let to = params
        .to_seq
        .ok_or_else(|| ApiError::invalid_update("to_seq is required"))?;
    let render = crate::flow::query::Render::parse(params.render.as_deref())?;
    Ok(ApiResponse::success(
        crate::flow::scope_history::diff(&state, object, actor, from, to, render).await?,
    ))
}

pub async fn get_flow_v2_history(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    bot: Option<Extension<BotAuthContext>>,
    Path(object): Path<Uuid>,
    Query(query): Query<super::FlowObjectHistoryQuery>,
) -> Result<impl IntoResponse, ApiError> {
    let actor = principal(&state, &claims, bot.as_ref(), Subject::Object(object)).await?;
    let proof = scope::authorize(&state.db, object, actor, PermissionLevel::View).await?;
    let rows = state.db.query_all(Statement::from_sql_and_values(DbBackend::Postgres,
        "SELECT seq,actor,origin,message,semantic_summary,created_at FROM (
           SELECT u.seq,u.actor_id AS actor,u.origin_surface AS origin,e.metadata->>'message' AS message,
                  CASE WHEN e.event_type='flow.version.imported' THEN e.payload
                       ELSE COALESCE(e.metadata->'semantic_summary','{}'::jsonb) END AS semantic_summary,u.created_at
             FROM collab_updates u JOIN collab_documents d ON d.id=u.document_id
             JOIN business_events e ON e.id=u.event_id WHERE d.object_id=$1
           UNION ALL
           SELECT 0,e.actor_id,'system',NULL,
                  jsonb_build_object('imported',true,'source_seq',e.metadata->'imported_base_source_seq',
                      'source_recorded_at',e.metadata->'imported_base_source_recorded_at',
                      'package_sha256',e.metadata->'package_sha256'),e.created_at
             FROM collab_documents d JOIN business_events e
               ON e.aggregate_id=d.object_id::text AND e.idempotency_key='portable_graph_create:'||d.object_id::text
             WHERE d.object_id=$1 AND e.event_type='flow.object.created' AND e.metadata->>'imported_base_source_seq' IS NOT NULL
               AND (d.snapshot_seq=0 OR EXISTS(SELECT 1 FROM collab_updates first WHERE first.document_id=d.id AND first.seq=1))
         ) versions WHERE ($2::bigint IS NULL OR seq<$2) ORDER BY seq DESC LIMIT $3",
        vec![object.into(),query.before_seq.into(),i64::try_from(query.limit.unwrap_or(50).clamp(1,100)).unwrap_or(100).into()])
    ).await?;
    let mut items = Vec::new();
    for row in rows {
        items.push(json!({"seq":row.try_get::<i64>("","seq")?,"actor":row.try_get::<Option<Uuid>>("","actor")?,"origin":row.try_get::<String>("","origin")?,"message":row.try_get::<Option<String>>("","message")?,"semantic_summary":row.try_get::<Value>("","semantic_summary")?,"created_at":row.try_get::<platform::timestamp::Timestamp>("","created_at")?.to_rfc3339()}));
    }
    if !scope::proof_current(&state.db, &proof).await? {
        return Err(crate::flow::policy::authorization_read_unstable());
    }
    let next = items.last().and_then(|v| v.field("seq").as_i64());
    Ok(ApiResponse::success(json!({"items":items,"next_before_seq":next})))
}
pub async fn get_flow_v2_comments(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    bot: Option<Extension<BotAuthContext>>,
    Path(object): Path<Uuid>,
) -> Result<impl IntoResponse, ApiError> {
    let proof = scope::authorize(
        &state.db,
        object,
        principal(&state, &claims, bot.as_ref(), Subject::Object(object)).await?,
        PermissionLevel::View,
    )
    .await?;
    let comments = crate::flow::comments::list_comments(&state.db, object).await?;
    if !scope::proof_current(&state.db, &proof).await? {
        return Err(crate::flow::policy::authorization_read_unstable());
    }
    Ok(ApiResponse::success(comments))
}
pub async fn get_flow_v2_grants(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    bot: Option<Extension<BotAuthContext>>,
    Path(object): Path<Uuid>,
) -> Result<impl IntoResponse, ApiError> {
    let actor = principal(&state, &claims, bot.as_ref(), Subject::Object(object)).await?;
    let proof = scope::authorize(&state.db, object, actor, PermissionLevel::View).await?;
    let rows=state.db.query_all(Statement::from_sql_and_values(DbBackend::Postgres,"SELECT principal_kind,principal_id,level,granted_by,created_at FROM flow_object_grants WHERE object_id=$1 AND scope_id=$2 ORDER BY principal_kind,principal_id",vec![object.into(),proof.scope_id.into()])).await?;
    let mut grants = Vec::new();
    for row in rows {
        if proof.level < PermissionLevel::FullAccess {
            break;
        }
        grants.push(json!({"principal_kind":row.try_get::<String>("","principal_kind")?,"principal_id":row.try_get::<Uuid>("","principal_id")?,"level":row.try_get::<String>("","level")?,"granted_by":row.try_get::<Option<Uuid>>("","granted_by")?,"created_at":row.try_get::<platform::timestamp::Timestamp>("","created_at")?.to_rfc3339()}));
    }
    if !scope::proof_current(&state.db, &proof).await? {
        return Err(crate::flow::policy::authorization_read_unstable());
    }
    let identity = scope::identity(&state.db, object).await?;
    let inherit = state
        .db
        .query_one(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT inherit_from_parent FROM flow_objects WHERE id=$1",
            vec![object.into()],
        ))
        .await?
        .and_then(|r| r.try_get::<bool>("", "inherit_from_parent").ok())
        .unwrap_or(false);
    Ok(ApiResponse::success(
        json!({"object_id":object,"scope_id":identity.scope_id,"inherit_from_parent":inherit,"items":grants,"inherited":[],"effective_level":proof.level.as_wire(),"caller_level":proof.level.as_wire(),"authz_epoch":proof.revision,"authz_revision":proof.revision}),
    ))
}
pub async fn put_flow_v2_grants(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    bot: Option<Extension<BotAuthContext>>,
    Path(object): Path<Uuid>,
    Json(input): Json<super::SetGrantsRequest>,
) -> Result<Response, ApiError> {
    if scope::identity(&state.db, object).await?.workspace_id.is_some() {
        return Ok(
            super::put_flow_object_grants(State(state), Extension(claims), bot, Path(object), Json(input))
                .await?
                .into_response(),
        );
    }
    let actor = user(&claims, bot.as_ref())?;
    Ok(ApiResponse::success(
        crate::flow::scope_acl::mutate(
            &state.db,
            object,
            actor,
            crate::flow::scope_acl::Mutation {
                grants: Some(input.grants.into_iter().map(Into::into).collect()),
                inheritance: None,
                dry_run: input.dry_run,
                confirm_self_lockout: input.confirm_self_lockout,
                idempotency_key: input.idempotency_key,
            },
        )
        .await?,
    )
    .into_response())
}
pub async fn put_flow_v2_inheritance(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    bot: Option<Extension<BotAuthContext>>,
    Path(object): Path<Uuid>,
    Json(input): Json<super::SetInheritanceRequest>,
) -> Result<Response, ApiError> {
    if scope::identity(&state.db, object).await?.workspace_id.is_some() {
        return Ok(
            super::put_flow_object_inheritance(State(state), Extension(claims), bot, Path(object), Json(input))
                .await?
                .into_response(),
        );
    }
    let actor = user(&claims, bot.as_ref())?;
    Ok(ApiResponse::success(
        crate::flow::scope_acl::mutate(
            &state.db,
            object,
            actor,
            crate::flow::scope_acl::Mutation {
                grants: input
                    .initial_grants
                    .map(|grants| grants.into_iter().map(Into::into).collect()),
                inheritance: Some(input.inherit_from_parent),
                dry_run: input.dry_run,
                confirm_self_lockout: input.confirm_self_lockout,
                idempotency_key: input.idempotency_key,
            },
        )
        .await?,
    )
    .into_response())
}

pub async fn get_flow_v2_search(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    bot: Option<Extension<BotAuthContext>>,
    Path(scope_id): Path<Uuid>,
    Query(query): Query<super::FlowSearchQuery>,
) -> Result<Response, ApiError> {
    let actor = principal(&state, &claims, bot.as_ref(), Subject::Scope(scope_id)).await?;
    let owner_scope = accessible_scope(&state, scope_id, actor).await?;
    let selectors =
        usize::from(query.project_id.is_some()) + usize::from(query.unprojected) + usize::from(query.all_visible);
    if selectors > 1 || owner_scope.kind == "legacy_managed" && selectors == 0 {
        return Err(ApiError::invalid_update(
            "select exactly one of project_id, unprojected=true, or all_visible=true for legacy search",
        ));
    }
    if owner_scope.kind == "personal" && query.project_id.is_some() {
        return Err(ApiError::invalid_update(
            "a personal scope has no legacy project partition",
        ));
    }
    if query
        .freshness
        .as_deref()
        .is_some_and(|v| !matches!(v, "allow_stale" | "require_current"))
    {
        return Err(ApiError::invalid_update(
            "freshness must be allow_stale or require_current",
        ));
    }
    if query
        .object_type
        .as_deref()
        .is_some_and(|v| !matches!(v, "page" | "navigator"))
    {
        return Err(ApiError::invalid_update("search object_type must be page or navigator"));
    }
    if query.q.is_empty() || query.q.chars().count() > 256 {
        return Err(ApiError::invalid_update("q must contain1..256 characters"));
    }
    if let Some(workspace) = scope::legacy_workspace(&state.db, scope_id).await? {
        return Ok(super::get_flow_search(
            State(state),
            Extension(claims),
            bot,
            Path(workspace),
            super::FlowQuery(query),
        )
        .await?
        .into_response());
    }
    let cursor = query
        .cursor
        .as_deref()
        .map(Uuid::parse_str)
        .transpose()
        .map_err(|_| ApiError::invalid_update("invalid search cursor"))?;
    let limit = query.limit.unwrap_or(50).clamp(1, 100) as usize;
    let rows=ObjectId::find_by_statement(Statement::from_sql_and_values(DbBackend::Postgres,"SELECT o.id FROM flow_objects o JOIN flow_object_projections p ON p.object_id=o.id WHERE o.scope_id=$1 AND o.lifecycle_status='active' AND NOT flow_is_system_navigator_root(o.object_type,o.parent_id,o.governance_metadata) AND ($2::uuid IS NULL OR o.id>$2) AND ($3::uuid IS NULL OR o.project_id=$3) AND ($4::text IS NULL OR o.object_type=$4) AND (NOT $6::boolean OR o.project_id IS NULL) AND (p.title ILIKE '%'||$5||'%' OR p.plain_text ILIKE '%'||$5||'%') ORDER BY o.id LIMIT 10001",vec![scope_id.into(),cursor.into(),query.project_id.into(),query.object_type.into(),query.q.clone().into(),query.unprojected.into()])).all(&state.db).await?;
    let mut items = Vec::new();
    let mut next = None;
    let scanned = rows.len();
    let mut last = None;
    let mut head = 0;
    let mut indexed = 0;
    for row in rows {
        if items.len() >= limit {
            next = last.map(|id: Uuid| id.to_string());
            break;
        }
        last = Some(row.id);
        let view = match scope::object_view(&state.db, row.id, actor).await {
            Ok(v) => v,
            Err(err) if err.kind() == crate::error::ApiErrorKind::NotFound => continue,
            Err(err) => return Err(err),
        };
        let text = state
            .db
            .query_one(Statement::from_sql_and_values(
                DbBackend::Postgres,
                "SELECT plain_text FROM flow_object_projections WHERE object_id=$1",
                vec![row.id.into()],
            ))
            .await?
            .and_then(|r| r.try_get::<String>("", "plain_text").ok())
            .unwrap_or_default();
        let lag = view.field("projection_lag").as_i64().unwrap_or(0);
        if lag > 0 && query.freshness.as_deref() == Some("require_current") {
            return Err(ApiError::server_draining(
                crate::error::ServerDrainingReason::Contention,
                200,
                "search projection is behind accepted content",
            ));
        }
        let title = view.field("title").as_str().unwrap_or("");
        let mut fields = Vec::new();
        if title.to_lowercase().contains(&query.q.to_lowercase()) {
            fields.push("title");
        }
        if text.to_lowercase().contains(&query.q.to_lowercase()) {
            fields.push("plain_text");
        }
        head = head.max(view.field("document_seq").as_i64().unwrap_or(0));
        indexed = indexed.max(view.field("projection_seq").as_i64().unwrap_or(0));
        items.push(json!({"object":{"id":row.id,"object_type":view.field("object_type"),"title":title,"lifecycle_status":view.field("lifecycle_status"),"project_id":view.field("project_id")},"matched_fields":fields,"snippets":{"title":escape_html(title),"plain_text":escape_html(&text.chars().take(300).collect::<String>())},"indexed_seq":view.field("projection_seq"),"head_seq":view.field("document_seq"),"projection_lag":view.field("projection_lag"),"stale":lag>0}));
    }
    if next.is_none() && scanned >= 10001 {
        next = last.map(|id| id.to_string());
    }
    Ok(ApiResponse::success(
        json!({"items":items,"next_cursor":next,"index_frontier":{"indexed_seq":indexed,"head_seq":head,"lag":head-indexed,"stale":false}}),
    )
    .into_response())
}
fn escape_html(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#39;")
}
pub async fn get_flow_v2_projection_lag(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    bot: Option<Extension<BotAuthContext>>,
    Path(scope_id): Path<Uuid>,
    Query(params): Query<crate::flow::scope_navigation_query::QueryParams>,
) -> Result<Response, ApiError> {
    let actor = principal(&state, &claims, bot.as_ref(), Subject::Scope(scope_id)).await?;
    if let Some(workspace) = scope::legacy_workspace(&state.db, scope_id).await? {
        return Ok(super::get_flow_projection_lag(
            State(state),
            Extension(claims),
            bot,
            Path(workspace),
            super::FlowQuery(super::ProjectionLagQuery {
                project_id: params.project_id,
                cursor: params.cursor,
                limit: params.limit,
            }),
        )
        .await?
        .into_response());
    }
    accessible_scope(&state, scope_id, actor).await?;
    Ok(ApiResponse::success(
        crate::flow::scope_navigation_query::read(&state.db, scope_id, actor, "projection_lag", params).await?,
    )
    .into_response())
}
pub async fn get_flow_v2_trash(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    bot: Option<Extension<BotAuthContext>>,
    Path(scope_id): Path<Uuid>,
    Query(params): Query<crate::flow::scope_navigation_query::QueryParams>,
) -> Result<Response, ApiError> {
    let actor = principal(&state, &claims, bot.as_ref(), Subject::Scope(scope_id)).await?;
    if let Some(workspace) = scope::legacy_workspace(&state.db, scope_id).await? {
        return Ok(super::get_flow_trash(
            State(state),
            Extension(claims),
            bot,
            Path(workspace),
            Query(super::flow_page::TrashQuery {
                project_id: params.project_id,
                cursor: params.cursor,
                limit: params.limit,
            }),
        )
        .await?
        .into_response());
    }
    accessible_scope(&state, scope_id, actor).await?;
    Ok(ApiResponse::success(
        crate::flow::scope_navigation_query::read(&state.db, scope_id, actor, "trash", params).await?,
    )
    .into_response())
}
#[derive(Deserialize)]
pub struct PrincipalSearch {
    q: String,
}
pub async fn get_flow_v2_share_principals(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    bot: Option<Extension<BotAuthContext>>,
    Path(scope_id): Path<Uuid>,
    Query(query): Query<PrincipalSearch>,
) -> Result<impl IntoResponse, ApiError> {
    let actor = user(&claims, bot.as_ref())?;
    let scope = accessible_scope(&state, scope_id, actor).await?;
    if scope.owner_user_id != Some(actor) {
        return Err(ApiError::forbidden("scope management permission is required"));
    }
    if query.q.trim().chars().count() < 3 || query.q.len() > 256 {
        return Err(ApiError::invalid_update("principal search requires3..256 characters"));
    }
    let rows=state.db.query_all(Statement::from_sql_and_values(DbBackend::Postgres,"SELECT id,COALESCE(NULLIF(name,''),email) AS label FROM users WHERE email ILIKE $1 OR name ILIKE $1 ORDER BY id LIMIT 20",vec![format!("%{}%",query.q.trim()).into()])).await?;
    let mut items = Vec::new();
    for row in rows {
        items.push(json!({"principal_kind":"user","principal_id":row.try_get::<Uuid>("","id")?,"label":row.try_get::<String>("","label")?}));
    }
    Ok(ApiResponse::success(json!({"items":items})))
}

pub async fn get_flow_v2_mounts(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    bot: Option<Extension<BotAuthContext>>,
    Path(object): Path<Uuid>,
) -> Result<impl IntoResponse, ApiError> {
    Ok(ApiResponse::success(
        json!({"items":crate::flow::scope_mounts::list(&state.db,user(&claims,bot.as_ref())?,object).await?}),
    ))
}
pub async fn post_flow_v2_mount(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    bot: Option<Extension<BotAuthContext>>,
    Path(object): Path<Uuid>,
    Json(input): Json<crate::flow::scope_mounts::MountInput>,
) -> Result<impl IntoResponse, ApiError> {
    Ok(ApiResponse::success(
        crate::flow::scope_mounts::create(&state.db, user(&claims, bot.as_ref())?, object, input).await?,
    ))
}
pub async fn patch_flow_v2_mount(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    bot: Option<Extension<BotAuthContext>>,
    Path((object, mount)): Path<(Uuid, Uuid)>,
    Json(input): Json<crate::flow::scope_mounts::ChangeMountInput>,
) -> Result<impl IntoResponse, ApiError> {
    Ok(ApiResponse::success(
        crate::flow::scope_mounts::change(&state.db, user(&claims, bot.as_ref())?, object, mount, input).await?,
    ))
}
pub async fn delete_flow_v2_mount(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    bot: Option<Extension<BotAuthContext>>,
    Path((object, mount)): Path<(Uuid, Uuid)>,
) -> Result<impl IntoResponse, ApiError> {
    crate::flow::scope_mounts::remove(&state.db, user(&claims, bot.as_ref())?, object, mount).await?;
    Ok(ApiResponse::success(json!({"removed":true})))
}

pub async fn post_flow_v2_export(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    bot: Option<Extension<BotAuthContext>>,
    Path(object): Path<Uuid>,
    Json(input): Json<super::PackageExportRequest>,
) -> Result<Response, ApiError> {
    if input.object_types.is_some() {
        return Err(ApiError::invalid_update_with_details(
            "object export does not accept object_types",
            json!({"reason":"invalid_package_object_types"}),
        ));
    }
    let identity = scope::identity(&state.db, object).await?;
    if identity.workspace_id.is_some() && identity.object_type != "record" {
        return Ok(
            super::post_flow_object_export(State(state), Extension(claims), bot, Path(object), Json(input))
                .await?
                .into_response(),
        );
    }
    let actor = principal(&state, &claims, bot.as_ref(), Subject::Object(object)).await?;
    Ok(ApiResponse::success(
        crate::flow::scope_export::create(
            &state.db,
            object,
            actor,
            &input.format,
            input.at_seq,
            input.include_history,
            &input.idempotency_key,
        )
        .await?,
    )
    .into_response())
}
pub async fn get_flow_v2_export(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    bot: Option<Extension<BotAuthContext>>,
    Path(job): Path<Uuid>,
) -> Result<Response, ApiError> {
    if crate::flow::scope_export::is_workspace_job(&state.db, job).await? {
        return Ok(super::get_flow_export(State(state), Extension(claims), bot, Path(job))
            .await?
            .into_response());
    }
    let actor = user(&claims, bot.as_ref())?;
    let row = crate::flow::scope_export::get(&state.db, job, actor).await?;
    Ok(ApiResponse::success(crate::flow::scope_export::receipt(&row)).into_response())
}
pub async fn get_flow_v2_export_artifact(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    bot: Option<Extension<BotAuthContext>>,
    Path(job): Path<Uuid>,
) -> Result<Response, ApiError> {
    if crate::flow::scope_export::is_workspace_job(&state.db, job).await? {
        return Ok(
            super::get_flow_export_artifact(State(state), Extension(claims), bot, Path(job))
                .await?
                .into_response(),
        );
    }
    let actor = user(&claims, bot.as_ref())?;
    crate::flow::scope_export::get(&state.db, job, actor).await?;
    let (bytes, hash, format) = crate::flow::scope_export::download(&state.db, job, actor).await?;
    let mut headers = axum::http::HeaderMap::new();

    headers.insert(
        axum::http::header::CACHE_CONTROL,
        axum::http::HeaderValue::from_static("private, no-store"),
    );
    headers.insert(
        axum::http::header::VARY,
        axum::http::HeaderValue::from_static("Authorization, Cookie"),
    );
    headers.insert(
        axum::http::header::CONTENT_DISPOSITION,
        axum::http::HeaderValue::from_static("attachment"),
    );
    headers.insert(
        axum::http::header::CONTENT_TYPE,
        axum::http::HeaderValue::from_static(match format.as_str() {
            "markdown" => "text/markdown; charset=utf-8",
            "csv" => "text/csv; charset=utf-8",
            _ => "application/json; charset=utf-8",
        }),
    );
    headers.insert(
        "x-flow-package-sha256",
        axum::http::HeaderValue::from_str(&hash).map_err(|_| ApiError::internal())?,
    );
    Ok((headers, bytes).into_response())
}
pub async fn post_flow_v2_reference(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    bot: Option<Extension<BotAuthContext>>,
    Path(object): Path<Uuid>,
    Json(input): Json<crate::flow::bridge::CreateReferenceInput>,
) -> Result<Response, ApiError> {
    if scope::identity(&state.db, object).await?.workspace_id.is_some() {
        return Ok(
            super::post_flow_object_reference(State(state), Extension(claims), bot, Path(object), Json(input))
                .await?
                .into_response(),
        );
    }
    Ok(ApiResponse::success(
        crate::flow::bridge::create_scope_reference(&state, user(&claims, bot.as_ref())?, object, input).await?,
    )
    .into_response())
}
pub async fn delete_flow_v2_reference(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    bot: Option<Extension<BotAuthContext>>,
    Path((object, reference)): Path<(Uuid, Uuid)>,
    headers: axum::http::HeaderMap,
) -> Result<Response, ApiError> {
    if scope::identity(&state.db, object).await?.workspace_id.is_some() {
        return Ok(super::delete_flow_object_reference(
            State(state),
            Extension(claims),
            bot,
            Path((object, reference)),
            headers,
        )
        .await?
        .into_response());
    }
    let key = optional_intent(&headers)?.ok_or_else(|| ApiError::invalid_update("Idempotency-Key is required"))?;
    Ok(ApiResponse::success(
        crate::flow::bridge::remove_scope_reference(&state, user(&claims, bot.as_ref())?, object, reference, key)
            .await?,
    )
    .into_response())
}

pub async fn get_flow_v2_references(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    bot: Option<Extension<BotAuthContext>>,
    Path(object): Path<Uuid>,
) -> Result<impl IntoResponse, ApiError> {
    Ok(ApiResponse::success(
        crate::flow::bridge::list_scope_references(
            &state,
            principal(&state, &claims, bot.as_ref(), Subject::Object(object)).await?,
            object,
        )
        .await?,
    ))
}

pub async fn get_flow_v2_collection(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    bot: Option<Extension<BotAuthContext>>,
    Path(collection): Path<Uuid>,
    Query(query): Query<super::DescribeCollectionQuery>,
) -> Result<impl IntoResponse, ApiError> {
    Ok(ApiResponse::success(
        crate::flow::scope_collections::describe(
            &state,
            collection,
            principal(&state, &claims, bot.as_ref(), Subject::Object(collection)).await?,
            query.at_seq,
        )
        .await?,
    ))
}
pub async fn get_flow_v2_schema(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    bot: Option<Extension<BotAuthContext>>,
    Path(object): Path<Uuid>,
    Query(query): Query<super::DescribeCollectionQuery>,
) -> Result<impl IntoResponse, ApiError> {
    let actor = principal(&state, &claims, bot.as_ref(), Subject::Object(object)).await?;
    scope::authorize(&state.db, object, actor, PermissionLevel::View).await?;
    let collection = crate::flow::collections::owning_collection_id(&state.db, object)
        .await?
        .unwrap_or(object);
    Ok(ApiResponse::success(
        crate::flow::scope_collections::describe(&state, collection, actor, query.at_seq).await?,
    ))
}
pub async fn get_flow_v2_records(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    bot: Option<Extension<BotAuthContext>>,
    Path(collection): Path<Uuid>,
    Query(params): Query<super::GetCollectionRecordsQuery>,
) -> Result<impl IntoResponse, ApiError> {
    let actor = principal(&state, &claims, bot.as_ref(), Subject::Object(collection)).await?;
    scope::authorize(&state.db, collection, actor, PermissionLevel::View).await?;
    let request = crate::flow::collections::RecordQueryPayload {
        filter: super::parse_json_query(params.filter, "filter")?,
        sort: super::parse_json_query(params.sort, "sort")?,
        group: params.group,
        cursor: params.cursor,
        limit: params.limit.unwrap_or(50),
        field_ids: super::parse_field_ids(params.field_ids)?,
    };
    let request = crate::flow::collections::apply_saved_view(&state.db, collection, params.view_id, &request).await?;
    Ok(ApiResponse::success(
        crate::flow::scope_collections::query(&state, collection, user(&claims, bot.as_ref())?, &request).await?,
    ))
}
pub async fn post_flow_v2_record_query(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    bot: Option<Extension<BotAuthContext>>,
    Path(collection): Path<Uuid>,
    Json(request): Json<crate::flow::collections::RecordQueryPayload>,
) -> Result<impl IntoResponse, ApiError> {
    Ok(ApiResponse::success(
        crate::flow::scope_collections::query(
            &state,
            collection,
            principal(&state, &claims, bot.as_ref(), Subject::Object(collection)).await?,
            &request,
        )
        .await?,
    ))
}
pub async fn post_flow_v2_record(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    bot: Option<Extension<BotAuthContext>>,
    Path(collection): Path<Uuid>,
    Json(request): Json<super::CreateCollectionRecordRequest>,
) -> Result<Response, ApiError> {
    Ok(post_flow_v2_command(
        State(state),
        Extension(claims),
        bot,
        Path(collection),
        Json(super::ExecuteFlowCommandRequest {
            command: super::FlowCommandEnvelope {
                command_type: "record_create".into(),
                payload: json!({"properties":request.values_by_field_id,"body":request.body}),
            },
            expected_frontier: None,
            idempotency_key: request.idempotency_key,
            message: request.message,
        }),
    )
    .await?
    .into_response())
}

/// Bounded transport-only compatibility: the exact v1 router retains its domain services and
/// authority checks. Scope resolution changes context, never ownership or event replay identity.
#[derive(Clone)]
pub struct LegacyFlowRouter(pub axum::Router);
pub async fn flow_v2_legacy_adapter(
    State(state): State<AppState>,
    Extension(legacy): Extension<LegacyFlowRouter>,
    Extension(claims): Extension<JwtClaims>,
    bot: Option<Extension<BotAuthContext>>,
    request: axum::extract::Request,
) -> Result<Response, ApiError> {
    use tower::ServiceExt;
    let actor = user(&claims, bot.as_ref())?;
    let original = request.uri().clone();
    let path = original.path();
    let suffix = path
        .strip_prefix("/api/v2/flow/")
        .ok_or_else(|| ApiError::not_found("Flow route not found"))?;
    let parts = suffix.split('/').collect::<Vec<_>>();
    let target = if parts.first() == Some(&"scopes") {
        let scope_id = parts
            .get(1)
            .and_then(|s| Uuid::parse_str(s).ok())
            .ok_or_else(|| ApiError::invalid_update("invalid Flow scope"))?;
        let Some(workspace) = scope::legacy_workspace(&state.db, scope_id).await? else {
            accessible_scope(&state, scope_id, actor).await?;
            return Err(ApiError::typed_with_details(
                crate::error::ApiErrorKind::FeatureDisabled,
                "this scope-native surface is not yet available",
                json!({"reason":"scope_surface_not_ready"}),
            ));
        };
        format!(
            "/api/v1/workspaces/{workspace}/flow/{}",
            parts.iter().skip(2).copied().collect::<Vec<_>>().join("/")
        )
    } else {
        if matches!(parts.first(), Some(&"objects" | &"collections")) {
            let object = parts
                .get(1)
                .and_then(|s| Uuid::parse_str(s).ok())
                .ok_or_else(|| ApiError::invalid_update("invalid Flow object"))?;
            let identity = scope::identity(&state.db, object).await?;
            if identity.workspace_id.is_none() {
                scope::authorize(&state.db, object, actor, PermissionLevel::View).await?;
                return Err(ApiError::typed_with_details(
                    crate::error::ApiErrorKind::FeatureDisabled,
                    "this scope-native surface is not yet available",
                    json!({"reason":"scope_surface_not_ready"}),
                ));
            }
        }
        format!("/api/v1/flow/{suffix}")
    };
    let target = if let Some(query) = original.query() {
        format!("{target}?{query}")
    } else {
        target
    };
    let (mut request_parts, mut body) = request.into_parts();
    // These v2 destination fields are semantic, not ignorable JSON extras. The legacy bridge
    // supports its genuine source workspace only; explicitly map the matching project and reject
    // cross-workspace intent until the scope-native conversion application service handles it.
    if parts.as_slice() == ["conversions", "preview"] && request_parts.method == axum::http::Method::POST {
        let bytes = axum::body::to_bytes(body, 200 * 1024 * 1024)
            .await
            .map_err(|_| ApiError::invalid_update("conversion request exceeds the HTTP body limit"))?;
        let mut input: Value =
            serde_json::from_slice(&bytes).map_err(|_| ApiError::invalid_update("invalid conversion JSON"))?;
        let source = input
            .get("source_object_id")
            .and_then(Value::as_str)
            .and_then(|id| Uuid::parse_str(id).ok())
            .ok_or_else(|| ApiError::invalid_update("source_object_id required"))?;
        let identity = scope::identity(&state.db, source).await?;
        scope::authorize(&state.db, source, actor, PermissionLevel::View).await?;
        let workspace = identity.workspace_id.ok_or_else(|| {
            ApiError::typed_with_details(
                crate::error::ApiErrorKind::FeatureDisabled,
                "personal conversion is not yet available",
                json!({"reason":"scope_surface_not_ready"}),
            )
        })?;
        if input
            .get("destination_workspace_id")
            .and_then(Value::as_str)
            .is_some_and(|id| id != workspace.to_string())
        {
            return Err(ApiError::unsupported_protocol(
                "cross-workspace conversion requires the scope-native conversion service",
            ));
        }
        if let Some(project) = input
            .get("destination_project_id")
            .and_then(Value::as_str)
            .and_then(|id| Uuid::parse_str(id).ok())
        {
            if input.get("target_type").and_then(Value::as_str) == Some("form") {
                if input
                    .pointer("/mapping/target_project_id")
                    .and_then(Value::as_str)
                    .is_some_and(|id| id != project.to_string())
                {
                    return Err(ApiError::invalid_update(
                        "destination project and mapping target project disagree",
                    ));
                }
                input.field_mut("mapping")?["target_project_id"] = json!(project);
            } else if let Some(form) = input
                .pointer("/mapping/target_form_id")
                .and_then(Value::as_str)
                .and_then(|id| Uuid::parse_str(id).ok())
            {
                let matches = state
                    .db
                    .query_one(Statement::from_sql_and_values(
                        DbBackend::Postgres,
                        "SELECT project_id=$3 AS matches FROM project_forms WHERE id=$1 AND workspace_id=$2",
                        vec![form.into(), workspace.into(), project.into()],
                    ))
                    .await?
                    .and_then(|row| row.try_get::<bool>("", "matches").ok())
                    .unwrap_or(false);
                if !matches {
                    return Err(ApiError::not_found("conversion target not found"));
                }
            }
        }
        if let Some(input) = input.as_object_mut() {
            input.remove("destination_workspace_id");
            input.remove("destination_project_id");
        }
        let bytes = serde_json::to_vec(&input).map_err(|_| ApiError::internal())?;
        request_parts.headers.remove(axum::http::header::CONTENT_LENGTH);
        body = axum::body::Body::from(bytes);
    }

    // Outer wildcard captures are private Axum extensions. Carrying them into another router
    // appends a second Path parameter and corrupts extraction. Reestablish trusted context only;
    // the inner exact v1 authentication middleware still validates the original credential.
    let peer = request_parts
        .extensions
        .get::<axum::extract::ConnectInfo<std::net::SocketAddr>>()
        .copied();
    request_parts.extensions = axum::http::Extensions::new();
    request_parts.extensions.insert(claims);
    if let Some(peer) = peer {
        request_parts.extensions.insert(peer);
    }
    request_parts.uri = target
        .parse()
        .map_err(|_| ApiError::invalid_update("invalid legacy Flow route"))?;
    let response = legacy
        .0
        .oneshot(axum::http::Request::from_parts(request_parts, body))
        .await
        .map_err(|_| ApiError::internal())?;
    if !response
        .headers()
        .get(axum::http::header::CONTENT_TYPE)
        .and_then(|h| h.to_str().ok())
        .is_some_and(|h| h.starts_with("application/json"))
    {
        return Ok(response);
    }
    if matches!(legacy_response_shape(&parts), LegacyDtoShape::None) {
        return Ok(response);
    }
    let (mut response_parts, body) = response.into_parts();
    let bytes = axum::body::to_bytes(body, 16 * 1024 * 1024)
        .await
        .map_err(|_| ApiError::internal())?;
    let Ok(mut value) = serde_json::from_slice::<Value>(&bytes) else {
        return Ok(axum::http::Response::from_parts(
            response_parts,
            axum::body::Body::from(bytes),
        ));
    };
    let shape = legacy_response_shape(&parts);
    let positions = match shape {
        LegacyDtoShape::AcceptedObject => vec!["/data/object".to_string()],
        LegacyDtoShape::RecordQuery => value
            .pointer("/data/items")
            .and_then(Value::as_array)
            .map(|items| (0..items.len()).map(|i| format!("/data/items/{i}/record")).collect())
            .unwrap_or_default(),
        LegacyDtoShape::None | LegacyDtoShape::ScopedMetadata => Vec::new(),
    };
    for pointer in positions {
        let id = value
            .pointer(&pointer)
            .and_then(|v| v.get("id"))
            .and_then(Value::as_str)
            .and_then(|id| Uuid::parse_str(id).ok());
        if let Some(id) = id
            && let Ok(view) = scope::object_view(&state.db, id, actor).await
            && let Some(slot) = value.pointer_mut(&pointer)
        {
            *slot = view;
        }
    }
    if matches!(shape, LegacyDtoShape::ScopedMetadata)
        && let Some(data) = value.get_mut("data").and_then(Value::as_object_mut)
        && let Some(scope) = parts.get(1).and_then(|id| Uuid::parse_str(id).ok())
    {
        data.insert("target_scope_id".into(), json!(scope));
    }
    let bytes = serde_json::to_vec(&value).map_err(|_| ApiError::internal())?;
    response_parts.headers.remove(axum::http::header::CONTENT_LENGTH);
    Ok(axum::http::Response::from_parts(
        response_parts,
        axum::body::Body::from(bytes),
    ))
}

#[derive(Clone, Copy)]
enum LegacyDtoShape {
    None,
    AcceptedObject,
    RecordQuery,
    ScopedMetadata,
}
fn legacy_response_shape(parts: &[&str]) -> LegacyDtoShape {
    match parts {
        ["objects", _, "history", "restore"] | ["objects", _, "commands" | "schema"] | ["collections", _] => {
            LegacyDtoShape::AcceptedObject
        }
        ["collections", _, "records" | "query"] => LegacyDtoShape::RecordQuery,
        ["scopes", _, "imports", _] => LegacyDtoShape::ScopedMetadata,
        _ => LegacyDtoShape::None,
    }
}

#[cfg(test)]
mod transport_adapter_tests {
    use super::*;
    use tower::ServiceExt;
    #[tokio::test]
    async fn inner_router_receives_only_its_own_path_capture() {
        async fn inner(Path(job): Path<Uuid>) -> Json<Value> {
            Json(json!({"job_id":job}))
        }
        let id = Uuid::new_v4();
        let claims = JwtClaims {
            sub: Uuid::new_v4().to_string(),
            email: "adapter@example.test".into(),
            token_type: platform::auth::TokenType::Access,
            iat: 0,
            exp: 0,
            sv: 0,
        };
        let state = AppState {
            db: sea_orm::DatabaseConnection::Disconnected,
            cfg: platform::config::AppConfig {
                app_name: "adapter".into(),
                bind_addr: "127.0.0.1:0".into(),
                database_url: platform::config::Secret::new("postgres://unused/unused"),
                jwt_secret: platform::config::Secret::new("adapter-test-secret"),
                jwt_access_ttl_seconds: 900,
                jwt_refresh_ttl_seconds: 3600,
                allow_insecure_cookies: false,
                collab_allowed_origins: Vec::new(),
            },
            flow_permission_cache: platform::app::FlowPermissionCacheSlot::default(),
        };
        let legacy = axum::Router::new().route("/api/v1/flow/exports/{job_id}/artifact", axum::routing::get(inner));
        let router = axum::Router::new()
            .route(
                "/api/v2/flow/{*legacy_path}",
                axum::routing::get(flow_v2_legacy_adapter),
            )
            .layer(Extension(claims))
            .layer(Extension(LegacyFlowRouter(legacy)))
            .with_state(state);
        let response = router
            .oneshot(
                axum::http::Request::builder()
                    .uri(format!("/api/v2/flow/exports/{id}/artifact"))
                    .body(axum::body::Body::empty())
                    .expect("request"),
            )
            .await
            .expect("response");
        assert_eq!(response.status(), axum::http::StatusCode::OK);
        let bytes = axum::body::to_bytes(response.into_body(), 1024).await.expect("body");
        let body: Value = serde_json::from_slice(&bytes).expect("JSON");
        assert_eq!(body.field("job_id").as_str(), Some(id.to_string().as_str()));
    }
    #[test]
    fn opaque_semantic_json_is_not_a_dto_normalization_target() {
        assert!(matches!(
            legacy_response_shape(&["objects", "id", "comments"]),
            LegacyDtoShape::None
        ));
        assert!(matches!(
            legacy_response_shape(&["objects", "id", "relations"]),
            LegacyDtoShape::None
        ));
    }
}

fn optional_intent(headers: &axum::http::HeaderMap) -> Result<Option<&str>, ApiError> {
    headers
        .get("Idempotency-Key")
        .map(|value| {
            value
                .to_str()
                .map_err(|_| ApiError::invalid_update("Idempotency-Key must be text"))
        })
        .transpose()
}
pub async fn post_flow_v2_comment(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    bot: Option<Extension<BotAuthContext>>,
    Path(object): Path<Uuid>,
    headers: axum::http::HeaderMap,
    Json(input): Json<crate::flow::comments::NewComment>,
) -> Result<impl IntoResponse, ApiError> {
    Ok(ApiResponse::success(
        crate::flow::scope_social::create_comment(
            &state.db,
            object,
            principal(&state, &claims, bot.as_ref(), Subject::Object(object)).await?,
            input,
            optional_intent(&headers)?,
        )
        .await?,
    ))
}
#[derive(Deserialize)]
pub struct ScopeCommentPatch {
    body: String,
    mentions: Option<Vec<Uuid>>,
}
#[derive(Deserialize)]
pub struct ScopeCommentResolve {
    resolved: bool,
}
pub async fn patch_flow_v2_comment(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    bot: Option<Extension<BotAuthContext>>,
    Path((object, comment)): Path<(Uuid, Uuid)>,
    headers: axum::http::HeaderMap,
    Json(input): Json<ScopeCommentPatch>,
) -> Result<impl IntoResponse, ApiError> {
    Ok(ApiResponse::success(
        crate::flow::scope_social::update_comment(
            &state.db,
            object,
            comment,
            principal(&state, &claims, bot.as_ref(), Subject::Object(object)).await?,
            &input.body,
            input.mentions,
            optional_intent(&headers)?,
        )
        .await?,
    ))
}
pub async fn delete_flow_v2_comment(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    bot: Option<Extension<BotAuthContext>>,
    Path((object, comment)): Path<(Uuid, Uuid)>,
    headers: axum::http::HeaderMap,
) -> Result<impl IntoResponse, ApiError> {
    crate::flow::scope_social::delete_comment(
        &state.db,
        object,
        comment,
        principal(&state, &claims, bot.as_ref(), Subject::Object(object)).await?,
        optional_intent(&headers)?,
    )
    .await?;
    Ok(ApiResponse::success(json!({"deleted":true})))
}
pub async fn resolve_flow_v2_comment(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    bot: Option<Extension<BotAuthContext>>,
    Path((object, comment)): Path<(Uuid, Uuid)>,
    headers: axum::http::HeaderMap,
    Json(input): Json<ScopeCommentResolve>,
) -> Result<impl IntoResponse, ApiError> {
    Ok(ApiResponse::success(
        crate::flow::scope_social::resolve_comment(
            &state.db,
            object,
            comment,
            principal(&state, &claims, bot.as_ref(), Subject::Object(object)).await?,
            input.resolved,
            optional_intent(&headers)?,
        )
        .await?,
    ))
}

#[derive(Deserialize)]
pub struct ScopeAssetUpload {
    data_base64: String,
}
pub async fn put_flow_v2_appearance(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    bot: Option<Extension<BotAuthContext>>,
    Path(object): Path<Uuid>,
    headers: axum::http::HeaderMap,
    Json(input): Json<crate::flow::appearance::PageAppearance>,
) -> Result<impl IntoResponse, ApiError> {
    Ok(ApiResponse::success(
        crate::flow::scope_assets::set(
            &state.db,
            object,
            principal(&state, &claims, bot.as_ref(), Subject::Object(object)).await?,
            input,
            optional_intent(&headers)?,
        )
        .await?,
    ))
}
pub async fn post_flow_v2_asset(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    bot: Option<Extension<BotAuthContext>>,
    Path(object): Path<Uuid>,
    headers: axum::http::HeaderMap,
    Json(input): Json<ScopeAssetUpload>,
) -> Result<impl IntoResponse, ApiError> {
    Ok(ApiResponse::success(
        crate::flow::scope_assets::upload(
            &state.db,
            object,
            principal(&state, &claims, bot.as_ref(), Subject::Object(object)).await?,
            &input.data_base64,
            optional_intent(&headers)?,
        )
        .await?,
    ))
}
pub async fn get_flow_v2_asset(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    bot: Option<Extension<BotAuthContext>>,
    Path((object, asset)): Path<(Uuid, Uuid)>,
) -> Result<impl IntoResponse, ApiError> {
    let asset = crate::flow::scope_assets::read(
        &state.db,
        object,
        asset,
        principal(&state, &claims, bot.as_ref(), Subject::Object(object)).await?,
    )
    .await?;
    let mut headers = axum::http::HeaderMap::new();
    headers.insert(
        axum::http::header::CONTENT_TYPE,
        axum::http::HeaderValue::from_static(asset.content_type),
    );
    headers.insert(
        axum::http::header::CACHE_CONTROL,
        axum::http::HeaderValue::from_static("private, no-store"),
    );
    headers.insert(
        axum::http::header::X_CONTENT_TYPE_OPTIONS,
        axum::http::HeaderValue::from_static("nosniff"),
    );
    Ok((headers, asset.bytes))
}

#[derive(Deserialize)]
pub struct ScopeShareCreate {
    expires_at: Option<platform::timestamp::Timestamp>,
}
pub async fn get_flow_v2_share_links(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    bot: Option<Extension<BotAuthContext>>,
    Path(object): Path<Uuid>,
) -> Result<impl IntoResponse, ApiError> {
    Ok(ApiResponse::success(
        crate::flow::scope_shares::list(
            &state.db,
            object,
            principal(&state, &claims, bot.as_ref(), Subject::Object(object)).await?,
        )
        .await?,
    ))
}
pub async fn post_flow_v2_share_link(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    bot: Option<Extension<BotAuthContext>>,
    Path(object): Path<Uuid>,
    Json(input): Json<ScopeShareCreate>,
) -> Result<impl IntoResponse, ApiError> {
    Ok(ApiResponse::success(
        crate::flow::scope_shares::create(
            &state.db,
            object,
            principal(&state, &claims, bot.as_ref(), Subject::Object(object)).await?,
            input.expires_at,
        )
        .await?,
    ))
}
pub async fn revoke_flow_v2_share_link(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    bot: Option<Extension<BotAuthContext>>,
    Path((object, link)): Path<(Uuid, Uuid)>,
) -> Result<impl IntoResponse, ApiError> {
    Ok(ApiResponse::success(
        crate::flow::scope_shares::revoke(
            &state.db,
            object,
            link,
            principal(&state, &claims, bot.as_ref(), Subject::Object(object)).await?,
        )
        .await?,
    ))
}
pub async fn get_flow_v2_public_share(
    State(state): State<AppState>,
    Path(token): Path<String>,
    super::ShareClientIp(ip): super::ShareClientIp,
) -> Result<impl IntoResponse, ApiError> {
    crate::flow::shares::enforce_public_share_rate_limit(&state.db, ip).await?;
    Ok(ApiResponse::success(
        crate::flow::scope_shares::read_public(&state.db, &token).await?,
    ))
}
pub async fn get_flow_v2_public_asset(
    State(state): State<AppState>,
    Path((token, asset)): Path<(String, Uuid)>,
    super::ShareClientIp(ip): super::ShareClientIp,
) -> Result<impl IntoResponse, ApiError> {
    crate::flow::shares::enforce_public_share_rate_limit(&state.db, ip).await?;
    let asset = crate::flow::scope_shares::read_public_asset(&state.db, &token, asset).await?;
    let mut headers = axum::http::HeaderMap::new();
    headers.insert(
        axum::http::header::CONTENT_TYPE,
        axum::http::HeaderValue::from_static(asset.content_type),
    );
    headers.insert(
        axum::http::header::CACHE_CONTROL,
        axum::http::HeaderValue::from_static("private, no-store"),
    );
    headers.insert(
        axum::http::header::X_CONTENT_TYPE_OPTIONS,
        axum::http::HeaderValue::from_static("nosniff"),
    );
    Ok((headers, asset.bytes))
}
#[derive(Default, Deserialize)]
pub struct ScopeMentionQuery {
    #[serde(default)]
    q: String,
}
pub async fn get_flow_v2_mentions(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    bot: Option<Extension<BotAuthContext>>,
    Path(object): Path<Uuid>,
    Query(input): Query<ScopeMentionQuery>,
) -> Result<impl IntoResponse, ApiError> {
    Ok(ApiResponse::success(
        crate::flow::scope_social::mention_candidates(
            &state.db,
            object,
            principal(&state, &claims, bot.as_ref(), Subject::Object(object)).await?,
            &input.q,
        )
        .await?,
    ))
}

/// The work items linked to a page that the caller may read, and how many others are linked.
pub async fn get_flow_v2_work_items(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    bot: Option<Extension<BotAuthContext>>,
    Path(object): Path<Uuid>,
) -> Result<impl IntoResponse, ApiError> {
    use crate::services::issue_view::{PageCaller, PageRef, viewable_pages, work_items_of_page};
    let identity = scope::identity(&state.db, object).await?;
    let extensions = super::build_auth_extensions(claims, bot);
    let access = match identity.workspace_id {
        Some(workspace) => crate::flow::policy::resolve_flow_principal(&state, &extensions, workspace)
            .await
            .map_err(|_| ApiError::not_found("flow resource not found"))?,
        None => match extensions.get::<JwtClaims>().map(|claims| Uuid::parse_str(&claims.sub)) {
            Some(Ok(user)) if extensions.get::<BotAuthContext>().is_none() => (user, String::new(), false),
            _ => return Err(ApiError::not_found("flow resource not found")),
        },
    };
    let caller = PageCaller::new(access, identity.workspace_id.unwrap_or_else(Uuid::nil));
    let page = PageRef {
        id: object,
        workspace_id: identity.workspace_id,
    };
    if !viewable_pages(&state.db, &caller, &[page]).await?.contains(&object) {
        return Err(ApiError::not_found("flow resource not found"));
    }
    Ok(ApiResponse::success(
        work_items_of_page(&state.db, &caller, object).await?,
    ))
}

pub async fn get_flow_v2_relations(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    bot: Option<Extension<BotAuthContext>>,
    Path(object): Path<Uuid>,
    Query(input): Query<super::FlowRelationsQuery>,
) -> Result<impl IntoResponse, ApiError> {
    Ok(ApiResponse::success(
        crate::flow::scope_relations::list(
            &state,
            object,
            principal(&state, &claims, bot.as_ref(), Subject::Object(object)).await?,
            crate::flow::relations::ListRelationsParams {
                direction: crate::flow::relations::RelationDirection::parse(input.direction.as_deref())?,
                relation_type: input.relation_type,
                cursor: input.cursor,
                limit: input.limit,
            },
        )
        .await?,
    ))
}

/// Import artifacts keep streaming/body limits from the outer Flow router. Ownership is checked
/// before parsing a personal upload; legacy requests reuse their actual workspace handler.
pub async fn post_flow_v2_import_artifact(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    bot: Option<Extension<BotAuthContext>>,
    Path(scope_id): Path<Uuid>,
    request: axum::extract::Request,
) -> Result<Response, ApiError> {
    use axum::extract::{FromRequest, Multipart};
    if let Some(workspace) = scope::legacy_workspace(&state.db, scope_id).await? {
        return Ok(
            super::post_flow_import_artifact(State(state), Extension(claims), bot, Path(workspace), request)
                .await?
                .into_response(),
        );
    }
    let actor = user(&claims, bot.as_ref())?;
    crate::flow::scope_import::require_destination(&state.db, scope_id, actor).await?;
    let multipart_key = optional_intent(request.headers())?.unwrap_or_default().to_string();
    let multipart = request
        .headers()
        .get(axum::http::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| v.starts_with("multipart/form-data"));
    let (bytes, expected, key) = if multipart {
        let mut input = Multipart::from_request(request, &state)
            .await
            .map_err(|_| ApiError::invalid_update("invalid package multipart upload"))?;
        let mut stager = crate::flow::import::BoundedImportStager::new(Vec::new(), std::io::sink());
        let mut seen = false;
        while let Some(mut field) = input
            .next_field()
            .await
            .map_err(|_| ApiError::invalid_update("package multipart stream failed"))?
        {
            if seen || field.name() != Some("package") {
                return Err(ApiError::invalid_update("upload requires exactly one package field"));
            }
            seen = true;
            while let Some(chunk) = field
                .chunk()
                .await
                .map_err(|_| ApiError::invalid_update("package multipart stream failed"))?
            {
                stager.write_archive_chunk(&chunk)?;
            }
        }
        if !seen {
            return Err(ApiError::invalid_update("package field is missing"));
        }
        stager.finish_archive()?;
        let (bytes, _) = stager.into_stages()?;
        (bytes, None, multipart_key)
    } else {
        let Json(input) = Json::<super::PackageArtifactRequest>::from_request(request, &state)
            .await
            .map_err(|_| ApiError::invalid_update("invalid import artifact JSON"))?;
        let bytes = match input.source.kind.as_str() {
            "inline_base64" => {
                if input.source.object_key.is_some() || input.source.size.is_some() {
                    return Err(ApiError::invalid_update(
                        "inline_base64 cannot carry staged object fields",
                    ));
                }
                let encoded = input
                    .source
                    .package_base64
                    .as_deref()
                    .ok_or_else(|| ApiError::invalid_update("package_base64 is required"))?;
                let mut stager = crate::flow::import::BoundedImportStager::new(Vec::new(), std::io::sink());
                stager.stage_inline_base64_archive(encoded.as_bytes())?;
                stager.finish_archive()?;
                stager.into_stages()?.0
            }
            "staged_object" => {
                if input.source.package_base64.is_some() {
                    return Err(ApiError::invalid_update("staged_object cannot carry package_base64"));
                }
                let key = input
                    .source
                    .object_key
                    .as_deref()
                    .ok_or_else(|| ApiError::invalid_update("object_key is required"))?;
                if !key.starts_with(&format!("flow-package-staging/scopes/{scope_id}/")) {
                    return Err(ApiError::forbidden("staged object is not bound to the target scope"));
                }
                let size = input
                    .source
                    .size
                    .ok_or_else(|| ApiError::invalid_update("staged size is required"))?;
                if size > crate::flow::collab::limits::IMPORT_ARCHIVE_BYTES_MAX {
                    return Err(ApiError::limit_exceeded(
                        "staged package exceeds byte budget",
                        "import_archive_bytes",
                        Some(json!(crate::flow::collab::limits::IMPORT_ARCHIVE_BYTES_MAX)),
                        Some(json!(size)),
                        None,
                    ));
                }
                if input.source.package_sha256.is_none() {
                    return Err(ApiError::invalid_update("staged checksum is required"));
                }
                let bytes = crate::services::object_storage::ObjectStorage::from_runtime_config()?
                    .get_bounded(key, crate::flow::collab::limits::IMPORT_ARCHIVE_BYTES_MAX)
                    .await?;
                if bytes.len() as u64 != size {
                    return Err(ApiError::checksum_mismatch("staged package size disagrees"));
                }
                bytes
            }
            _ => {
                return Err(ApiError::invalid_update(
                    "artifact source must be inline_base64 or staged_object",
                ));
            }
        };
        (bytes, input.source.package_sha256, input.idempotency_key)
    };
    Ok(ApiResponse::success(
        crate::flow::scope_import::upload(&state.db, scope_id, actor, bytes, expected.as_deref(), &key).await?,
    )
    .into_response())
}
pub async fn post_flow_v2_import_preview(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    bot: Option<Extension<BotAuthContext>>,
    Path(scope_id): Path<Uuid>,
    Json(input): Json<super::PackageImportPreviewRequest>,
) -> Result<Response, ApiError> {
    if let Some(workspace) = scope::legacy_workspace(&state.db, scope_id).await? {
        return Ok(
            super::post_flow_import_preview(State(state), Extension(claims), bot, Path(workspace), Json(input))
                .await?
                .into_response(),
        );
    }
    let input = crate::flow::scope_import::PreviewInput {
        artifact_id: input.artifact_id,
        project_mapping: input.project_mapping,
        external_reference_policy: input.external_reference_policy,
        conflict_policy: input.conflict_policy,
        include_history: input.include_history,
        idempotency_key: input.idempotency_key,
    };
    Ok(ApiResponse::success(
        crate::flow::scope_import::preview(&state.db, scope_id, user(&claims, bot.as_ref())?, input).await?,
    )
    .into_response())
}
pub async fn post_flow_v2_import_commit(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    bot: Option<Extension<BotAuthContext>>,
    Path((scope_id, preview)): Path<(Uuid, Uuid)>,
    Json(input): Json<super::PackageImportCommitRequest>,
) -> Result<Response, ApiError> {
    if let Some(workspace) = scope::legacy_workspace(&state.db, scope_id).await? {
        return Ok(super::post_flow_import_commit(
            State(state),
            Extension(claims),
            bot,
            Path((workspace, preview)),
            Json(input),
        )
        .await?
        .into_response());
    }
    let input = crate::flow::scope_import::CommitInput {
        package_sha256: input.package_sha256,
        mapping_hash: input.mapping_hash,
        conflict_policy: input.conflict_policy,
        confirm: input.confirm,
        idempotency_key: input.idempotency_key,
    };
    Ok(ApiResponse::success(
        crate::flow::scope_import::commit(&state.db, scope_id, user(&claims, bot.as_ref())?, preview, input).await?,
    )
    .into_response())
}
pub async fn get_flow_v2_import(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    bot: Option<Extension<BotAuthContext>>,
    Path((scope_id, job)): Path<(Uuid, Uuid)>,
) -> Result<Response, ApiError> {
    if let Some(workspace) = scope::legacy_workspace(&state.db, scope_id).await? {
        let (principal, _) = super::package_workspace_principal(&state, claims, bot, workspace).await?;
        let report = crate::flow::package_import::get_import_report(&state.db, workspace, job, &principal).await?;
        let mut report = serde_json::to_value(report).map_err(|_| ApiError::internal())?;
        *report.field_mut("target_scope_id")? = json!(scope_id);
        return Ok(ApiResponse::success(report).into_response());
    }
    Ok(ApiResponse::success(
        crate::flow::scope_import::get(&state.db, scope_id, user(&claims, bot.as_ref())?, job).await?,
    )
    .into_response())
}

pub async fn post_flow_v2_scope_export(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    bot: Option<Extension<BotAuthContext>>,
    Path(scope_id): Path<Uuid>,
    Json(input): Json<super::PackageExportRequest>,
) -> Result<Response, ApiError> {
    if let Some(workspace) = scope::legacy_workspace(&state.db, scope_id).await? {
        return Ok(super::post_flow_workspace_export(
            State(state),
            Extension(claims),
            bot,
            Path(workspace),
            Json(input),
        )
        .await?
        .into_response());
    }
    if input.format != "package" || input.project_id.is_some() || input.at_seq.is_some() {
        return Err(ApiError::invalid_update(
            "personal scope export requires package at the accepted head, with no project binding",
        ));
    }
    Ok(ApiResponse::success(
        crate::flow::scope_export::create_scope(
            &state.db,
            scope_id,
            user(&claims, bot.as_ref())?,
            input.include_history,
            &input.idempotency_key,
            input.object_types,
        )
        .await?,
    )
    .into_response())
}

async fn scope_conversion_identity(state: &AppState, id: Uuid, job: bool) -> Result<(Uuid, bool), ApiError> {
    let sql = if job {
        "SELECT p.source_object_id,p.workspace_id FROM flow_conversion_jobs j JOIN flow_conversion_previews p ON p.id=j.preview_id WHERE j.id=$1"
    } else {
        "SELECT source_object_id,workspace_id FROM flow_conversion_previews WHERE id=$1"
    };
    let row = state
        .db
        .query_one(Statement::from_sql_and_values(
            DbBackend::Postgres,
            sql,
            vec![id.into()],
        ))
        .await?
        .ok_or_else(|| ApiError::not_found("conversion not found"))?;
    let object = row.try_get("", "source_object_id")?;
    let identity = scope::identity(&state.db, object).await?;
    Ok((object, identity.workspace_id != Some(row.try_get("", "workspace_id")?)))
}
pub async fn post_flow_v2_conversion_preview(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    bot: Option<Extension<BotAuthContext>>,
    Json(mut input): Json<Value>,
) -> Result<Response, ApiError> {
    let object = input
        .field("source_object_id")
        .as_str()
        .and_then(|s| Uuid::parse_str(s).ok())
        .ok_or_else(|| ApiError::invalid_update("source_object_id required"))?;
    let identity = scope::identity(&state.db, object).await?;
    let destination = input
        .get("destination_workspace_id")
        .and_then(Value::as_str)
        .map(Uuid::parse_str)
        .transpose()
        .map_err(|_| ApiError::invalid_update("invalid destination workspace"))?
        .or(identity.workspace_id)
        .ok_or_else(|| ApiError::invalid_update("explicit destination_workspace_id required"))?;
    if identity.workspace_id != Some(destination) {
        let actor = user(&claims, bot.as_ref())?;
        *input.field_mut("destination_workspace_id")? = json!(destination);
        let request =
            serde_json::from_value(input).map_err(|_| ApiError::invalid_update("invalid scoped conversion preview"))?;
        return Ok(
            ApiResponse::success(crate::flow::bridge::preview_scope_conversion(&state, actor, request).await?)
                .into_response(),
        );
    }
    if let Some(project) = input.get("destination_project_id").cloned().filter(|v| !v.is_null()) {
        if input.field("target_type") == "form" {
            if input
                .pointer("/mapping/target_project_id")
                .is_some_and(|v| v != &project)
            {
                return Err(ApiError::invalid_update("destination project and mapping disagree"));
            }
            input.field_mut("mapping")?["target_project_id"] = project;
        } else {
            let form = input
                .pointer("/mapping/target_form_id")
                .and_then(Value::as_str)
                .and_then(|s| Uuid::parse_str(s).ok())
                .ok_or_else(|| ApiError::invalid_update("target_form_id required"))?;
            let project = project
                .as_str()
                .and_then(|s| Uuid::parse_str(s).ok())
                .ok_or_else(|| ApiError::invalid_update("invalid destination project"))?;
            if state
                .db
                .query_one(Statement::from_sql_and_values(
                    DbBackend::Postgres,
                    "SELECT id FROM project_forms WHERE id=$1 AND workspace_id=$2 AND project_id=$3",
                    vec![form.into(), destination.into(), project.into()],
                ))
                .await?
                .is_none()
            {
                return Err(ApiError::not_found("conversion target not found"));
            }
        }
    }
    // The destination fields were folded into the mapping above; the request type takes none of them.
    if let Some(fields) = input.as_object_mut() {
        fields.remove("destination_project_id");
        fields.remove("destination_workspace_id");
    }
    let request = serde_json::from_value(input).map_err(|_| ApiError::invalid_update("invalid conversion preview"))?;
    let extensions = super::build_auth_extensions(claims, bot);
    Ok(
        ApiResponse::success(crate::flow::bridge::preview_conversion(&state, &extensions, request).await?)
            .into_response(),
    )
}
pub async fn post_flow_v2_conversion(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    bot: Option<Extension<BotAuthContext>>,
    Json(input): Json<crate::flow::bridge::ConversionCommitInput>,
) -> Result<Response, ApiError> {
    let (_, scoped) = scope_conversion_identity(&state, input.preview_id, false).await?;
    let actor = if scoped {
        Some(user(&claims, bot.as_ref())?)
    } else {
        None
    };
    let extensions = super::build_auth_extensions(claims, bot);
    let origin = super::request_origin(&extensions);
    let result = if let Some(actor) = actor {
        crate::flow::bridge::commit_scope_conversion(&state, actor, input, origin).await?
    } else {
        crate::flow::bridge::commit_conversion(&state, &extensions, input, origin).await?
    };
    Ok(ApiResponse::success(result).into_response())
}
pub async fn get_flow_v2_conversion(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    bot: Option<Extension<BotAuthContext>>,
    Path(job): Path<Uuid>,
) -> Result<Response, ApiError> {
    let (_, scoped) = scope_conversion_identity(&state, job, true).await?;
    let actor = if scoped {
        Some(user(&claims, bot.as_ref())?)
    } else {
        None
    };
    let extensions = super::build_auth_extensions(claims, bot);
    let result = if let Some(actor) = actor {
        crate::flow::bridge::scope_conversion_status(&state, actor, job).await?
    } else {
        crate::flow::bridge::conversion_status(&state, &extensions, job).await?
    };
    Ok(ApiResponse::success(result).into_response())
}
pub async fn post_flow_v2_conversion_retry(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    bot: Option<Extension<BotAuthContext>>,
    Path(job): Path<Uuid>,
    Json(input): Json<crate::flow::bridge::ConversionRetryInput>,
) -> Result<Response, ApiError> {
    let (_, scoped) = scope_conversion_identity(&state, job, true).await?;
    let actor = if scoped {
        Some(user(&claims, bot.as_ref())?)
    } else {
        None
    };
    let extensions = super::build_auth_extensions(claims, bot);
    let origin = super::request_origin(&extensions);
    let result = if let Some(actor) = actor {
        crate::flow::bridge::retry_scope_conversion(&state, actor, job, input, origin).await?
    } else {
        crate::flow::bridge::retry_conversion(&state, &extensions, job, input, origin).await?
    };
    Ok(ApiResponse::success(result).into_response())
}
pub async fn get_flow_v2_conversion_owner(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    bot: Option<Extension<BotAuthContext>>,
    Path(id): Path<Uuid>,
) -> Result<Response, ApiError> {
    let row=state.db.query_one(Statement::from_sql_and_values(DbBackend::Postgres,"SELECT source_object_id,workspace_id,actor_id FROM flow_conversion_previews WHERE id=$1 UNION ALL SELECT source_object_id,workspace_id,actor_id FROM flow_conversion_jobs WHERE id=$1 LIMIT 1",vec![id.into()])).await?.ok_or_else(||ApiError::not_found("conversion not found"))?;
    let object = row.try_get("", "source_object_id")?;
    let identity = scope::identity(&state.db, object).await?;
    if identity.workspace_id != Some(row.try_get("", "workspace_id")?) {
        let actor = user(&claims, bot.as_ref())?;
        scope::authorize(&state.db, object, actor, PermissionLevel::View).await?;
        if actor != row.try_get::<Uuid>("", "actor_id")? {
            return Err(ApiError::not_found("conversion not found"));
        }
        return Ok(
            ApiResponse::success(json!({"source_object_id":object,"project_id":identity.project_id})).into_response(),
        );
    }
    let extensions = super::build_auth_extensions(claims, bot);
    Ok(ApiResponse::success(crate::flow::bridge::conversion_owner(&state, &extensions, id).await?).into_response())
}

pub async fn post_flow_v2_operation(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    bot: Option<Extension<BotAuthContext>>,
    Path(object): Path<Uuid>,
    Json(input): Json<crate::flow::scope_operations::EnqueueInput>,
) -> Result<Response, ApiError> {
    Ok(ApiResponse::success(
        crate::flow::scope_operations::enqueue(&state.db, object, user(&claims, bot.as_ref())?, input).await?,
    )
    .into_response())
}
pub async fn get_flow_v2_operation(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    bot: Option<Extension<BotAuthContext>>,
    Path((scope_id, id)): Path<(Uuid, Uuid)>,
) -> Result<Response, ApiError> {
    Ok(ApiResponse::success(
        crate::flow::scope_operations::status(&state.db, scope_id, user(&claims, bot.as_ref())?, id).await?,
    )
    .into_response())
}
pub async fn get_flow_v2_scope_health(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    bot: Option<Extension<BotAuthContext>>,
    Path(scope_id): Path<Uuid>,
) -> Result<Response, ApiError> {
    Ok(ApiResponse::success(
        crate::flow::scope_operations::health(&state.db, scope_id, user(&claims, bot.as_ref())?).await?,
    )
    .into_response())
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DiagnosticsQuery {
    pub include_sizes: Option<bool>,
}
pub async fn get_flow_v2_collab_diagnostics(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    bot: Option<Extension<BotAuthContext>>,
    Path(object): Path<Uuid>,
    Query(params): Query<DiagnosticsQuery>,
) -> Result<Response, ApiError> {
    if params.include_sizes == Some(false) {
        return Err(ApiError::invalid_update(
            "diagnostic DTO includes sizes; include_sizes must be true or omitted",
        ));
    }
    let actor = principal(&state, &claims, bot.as_ref(), Subject::Object(object)).await?;
    let proof = scope::authorize(&state.db, object, actor, PermissionLevel::View).await?;
    let row=state.db.query_one(Statement::from_sql_and_values(DbBackend::Postgres,
        "SELECT d.id,d.engine,d.format_version,d.snapshot_seq,d.head_seq,d.head_frontier,d.update_count,d.byte_count,p.document_seq AS projection_seq,(SELECT count(*) FROM flow_integrity_records i WHERE i.scope_id=o.scope_id AND i.subject_kind='collab_document' AND i.subject_id=d.id::text AND i.status='open') AS open_issues FROM flow_objects o JOIN collab_documents d ON d.object_id=o.id JOIN flow_object_projections p ON p.object_id=o.id WHERE o.id=$1",vec![object.into()])).await?.ok_or_else(||ApiError::not_found("Flow document not found"))?;
    let result = json!({"scope_id":proof.scope_id,"object_id":object,"document_id":row.try_get::<Uuid>("","id")?,"engine":row.try_get::<String>("","engine")?,"format_version":row.try_get::<String>("","format_version")?,"snapshot_seq":row.try_get::<i64>("","snapshot_seq")?,"head_seq":row.try_get::<i64>("","head_seq")?,"frontier":BASE64.encode(row.try_get::<Vec<u8>>("","head_frontier")?),"update_count":row.try_get::<i64>("","update_count")?,"byte_size":row.try_get::<i64>("","byte_count")?,"last_compacted_at":null,"projection_seq":row.try_get::<i64>("","projection_seq")?,"integrity_state":if row.try_get::<i64>("","open_issues")?>0{"open_issues"}else{"ok"}});
    if !scope::proof_current(&state.db, &proof).await? {
        return Err(crate::flow::policy::authorization_read_unstable());
    }
    Ok(ApiResponse::success(result).into_response())
}

fn body_asset_intent(headers: &axum::http::HeaderMap) -> Result<String, ApiError> {
    let key = optional_intent(headers)?.ok_or_else(|| ApiError::invalid_update("Idempotency-Key is required"))?;
    if key.trim().is_empty() || key.len() > 128 {
        return Err(ApiError::invalid_update("Idempotency-Key must contain 1..128 bytes"));
    }
    Ok(key.into())
}
pub async fn get_flow_v2_body_asset_limits(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    bot: Option<Extension<BotAuthContext>>,
    Path(object): Path<Uuid>,
) -> Result<impl IntoResponse, ApiError> {
    Ok(ApiResponse::success(
        crate::flow::body_assets::limits_for(&state.db, object, user(&claims, bot.as_ref())?).await?,
    ))
}
pub async fn post_flow_v2_body_asset(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    bot: Option<Extension<BotAuthContext>>,
    Path(object): Path<Uuid>,
    request: axum::extract::Request,
) -> Result<impl IntoResponse, ApiError> {
    use axum::extract::FromRequest;
    let actor = user(&claims, bot.as_ref())?;
    let key = body_asset_intent(request.headers())?;
    let limits = crate::flow::body_assets::admit_upload(&state.db, object, actor).await?;
    let multipart = axum::extract::Multipart::from_request(request, &state)
        .await
        .map_err(|_| ApiError::invalid_update("body assets require valid multipart/form-data"))?;
    let upload = crate::flow::body_assets::decode_multipart(multipart, limits.max_bytes).await?;
    Ok(ApiResponse::success(
        crate::flow::body_assets::upload(&state.db, object, actor, upload, &key).await?,
    ))
}
pub async fn post_flow_v2_body_asset_base64(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    bot: Option<Extension<BotAuthContext>>,
    Path(object): Path<Uuid>,
    request: axum::extract::Request,
) -> Result<impl IntoResponse, ApiError> {
    use axum::extract::FromRequest;
    let actor = user(&claims, bot.as_ref())?;
    let key = body_asset_intent(request.headers())?;
    let limits = crate::flow::body_assets::admit_upload(&state.db, object, actor).await?;
    let Json(input) = Json::<crate::flow::body_assets::Base64Upload>::from_request(request, &state)
        .await
        .map_err(|_| ApiError::invalid_update("invalid body asset JSON"))?;
    let upload = crate::flow::body_assets::decode_base64(&input, limits.max_bytes)?;
    Ok(ApiResponse::success(
        crate::flow::body_assets::upload(&state.db, object, actor, upload, &key).await?,
    ))
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BodyAssetCopy {
    source_object_id: Uuid,
    asset_id: Uuid,
}
pub async fn post_flow_v2_body_asset_copy(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    bot: Option<Extension<BotAuthContext>>,
    Path(object): Path<Uuid>,
    headers: axum::http::HeaderMap,
    Json(input): Json<BodyAssetCopy>,
) -> Result<impl IntoResponse, ApiError> {
    // The source page is a second object of this request: a scoped bot must be able to read it.
    crate::middleware::scoped_bot::ensure_bot_level(
        &state.db,
        bot.as_ref().map(|bot| bot.0.bot_id),
        crate::middleware::scoped_bot::Secondary::FlowObject(input.source_object_id),
        crate::middleware::scoped_bot::Level::Read,
    )
    .await?;
    Ok(ApiResponse::success(
        crate::flow::body_assets::copy_into(
            &state.db,
            object,
            input.source_object_id,
            input.asset_id,
            user(&claims, bot.as_ref())?,
            &body_asset_intent(&headers)?,
        )
        .await?,
    ))
}
pub async fn get_flow_v2_body_asset_metadata(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    bot: Option<Extension<BotAuthContext>>,
    Path((object, asset)): Path<(Uuid, Uuid)>,
) -> Result<impl IntoResponse, ApiError> {
    Ok(ApiResponse::success(
        crate::flow::body_assets::metadata(&state.db, object, asset, user(&claims, bot.as_ref())?).await?,
    ))
}
fn body_asset_response(asset: crate::flow::body_assets::AssetBytes) -> Result<Response, ApiError> {
    use axum::http::{HeaderMap, HeaderValue, header};
    let mut headers = HeaderMap::new();
    for (key, value) in [
        (header::CONTENT_TYPE, asset.metadata.content_type),
        (header::CONTENT_DISPOSITION, asset.disposition),
        (header::CONTENT_LENGTH, asset.bytes.len().to_string()),
        (header::CACHE_CONTROL, "private, no-store".into()),
        (header::X_CONTENT_TYPE_OPTIONS, "nosniff".into()),
    ] {
        headers.insert(key, HeaderValue::from_str(&value).map_err(|_| ApiError::internal())?);
    }
    headers.insert(
        "x-flow-asset-sha256",
        HeaderValue::from_str(&asset.metadata.sha256).map_err(|_| ApiError::internal())?,
    );
    headers.insert(
        header::ACCESS_CONTROL_EXPOSE_HEADERS,
        HeaderValue::from_static("Content-Disposition, Content-Type, Content-Length, X-Flow-Asset-SHA256"),
    );
    Ok((headers, asset.bytes).into_response())
}
pub async fn get_flow_v2_body_asset(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    bot: Option<Extension<BotAuthContext>>,
    Path((object, asset)): Path<(Uuid, Uuid)>,
) -> Result<Response, ApiError> {
    body_asset_response(crate::flow::body_assets::read(&state.db, object, asset, user(&claims, bot.as_ref())?).await?)
}
pub async fn delete_flow_v2_body_asset(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    bot: Option<Extension<BotAuthContext>>,
    Path((object, asset)): Path<(Uuid, Uuid)>,
    headers: axum::http::HeaderMap,
) -> Result<impl IntoResponse, ApiError> {
    Ok(ApiResponse::success(
        crate::flow::body_assets::discard(
            &state.db,
            object,
            asset,
            user(&claims, bot.as_ref())?,
            &body_asset_intent(&headers)?,
        )
        .await?,
    ))
}
pub async fn get_flow_v2_public_body_asset(
    State(state): State<AppState>,
    Path((token, asset)): Path<(String, Uuid)>,
    super::ShareClientIp(ip): super::ShareClientIp,
) -> Result<Response, ApiError> {
    crate::flow::shares::enforce_public_share_rate_limit(&state.db, ip).await?;
    body_asset_response(crate::flow::scope_shares::read_public_body_asset(&state.db, &token, asset).await?)
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ChildReservationInput {
    title: String,
}
pub async fn post_flow_v2_child_reservation(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    bot: Option<Extension<BotAuthContext>>,
    Path(object): Path<Uuid>,
    headers: axum::http::HeaderMap,
    Json(input): Json<ChildReservationInput>,
) -> Result<impl IntoResponse, ApiError> {
    Ok(ApiResponse::success(
        crate::flow::child_pages::reserve(
            &state.db,
            object,
            user(&claims, bot.as_ref())?,
            &input.title,
            &body_asset_intent(&headers)?,
        )
        .await?,
    ))
}
pub async fn get_flow_v2_child_reservation(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    bot: Option<Extension<BotAuthContext>>,
    Path((object, reservation)): Path<(Uuid, Uuid)>,
) -> Result<impl IntoResponse, ApiError> {
    Ok(ApiResponse::success(
        crate::flow::child_pages::status(&state.db, object, reservation, user(&claims, bot.as_ref())?).await?,
    ))
}
pub async fn delete_flow_v2_child_reservation(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    bot: Option<Extension<BotAuthContext>>,
    Path((object, reservation)): Path<(Uuid, Uuid)>,
    headers: axum::http::HeaderMap,
) -> Result<impl IntoResponse, ApiError> {
    Ok(ApiResponse::success(
        crate::flow::child_pages::cancel(
            &state.db,
            object,
            reservation,
            user(&claims, bot.as_ref())?,
            &body_asset_intent(&headers)?,
        )
        .await?,
    ))
}

pub async fn post_flow_v2_block_share(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    bot: Option<Extension<BotAuthContext>>,
    Path(source): Path<Uuid>,
    headers: axum::http::HeaderMap,
    Json(input): Json<crate::flow::block_shares::CreateInput>,
) -> Result<impl IntoResponse, ApiError> {
    Ok(ApiResponse::success(
        crate::flow::block_shares::create(
            &state.db,
            user(&claims, bot.as_ref())?,
            source,
            &body_asset_intent(&headers)?,
            input,
        )
        .await?,
    ))
}
#[derive(Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BlockShareContextQuery {
    pub workspace_id: Option<Uuid>,
    pub project_id: Option<Uuid>,
}
pub async fn get_flow_v2_block_share(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    bot: Option<Extension<BotAuthContext>>,
    Path(share): Path<Uuid>,
    Query(context): Query<BlockShareContextQuery>,
) -> Result<impl IntoResponse, ApiError> {
    Ok(ApiResponse::success(
        crate::flow::block_shares::read_in_context(
            &state.db,
            user(&claims, bot.as_ref())?,
            share,
            context.workspace_id,
            context.project_id,
        )
        .await?,
    ))
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BlockShareDiscoveryQuery {
    pub workspace_id: Uuid,
    pub project_id: Option<Uuid>,
    pub cursor: Option<Uuid>,
}
pub async fn get_flow_v2_block_shares(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    bot: Option<Extension<BotAuthContext>>,
    Query(context): Query<BlockShareDiscoveryQuery>,
) -> Result<impl IntoResponse, ApiError> {
    Ok(ApiResponse::success(
        crate::flow::block_shares::discover_page(
            &state.db,
            user(&claims, bot.as_ref())?,
            context.workspace_id,
            context.project_id,
            context.cursor,
        )
        .await?,
    ))
}
pub async fn patch_flow_v2_block_share(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    bot: Option<Extension<BotAuthContext>>,
    Path(share): Path<Uuid>,
    headers: axum::http::HeaderMap,
    Json(input): Json<collab_core::block_projection::SharedPatch>,
) -> Result<impl IntoResponse, ApiError> {
    Ok(ApiResponse::success(
        crate::flow::block_shares::patch(
            &state.db,
            user(&claims, bot.as_ref())?,
            share,
            &body_asset_intent(&headers)?,
            input,
        )
        .await?,
    ))
}
pub async fn get_flow_v2_block_share_asset(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    bot: Option<Extension<BotAuthContext>>,
    Path((share, alias)): Path<(Uuid, Uuid)>,
) -> Result<Response, ApiError> {
    use axum::http::{HeaderMap, HeaderValue, header};
    let asset = crate::flow::block_shares::read_asset(&state.db, user(&claims, bot.as_ref())?, share, alias).await?;
    let mut headers = HeaderMap::new();
    for (key, value) in [
        (header::CONTENT_TYPE, asset.content_type),
        (header::CONTENT_DISPOSITION, asset.disposition),
        (header::CONTENT_LENGTH, asset.bytes.len().to_string()),
        (header::CACHE_CONTROL, "private, no-store".into()),
        (header::X_CONTENT_TYPE_OPTIONS, "nosniff".into()),
        (
            axum::http::HeaderName::from_static("x-flow-share-id"),
            asset.share_id.to_string(),
        ),
        (
            axum::http::HeaderName::from_static("x-flow-asset-alias"),
            asset.asset_alias.to_string(),
        ),
        (axum::http::HeaderName::from_static("x-flow-asset-sha256"), asset.sha256),
    ] {
        headers.insert(key, HeaderValue::from_str(&value).map_err(|_| ApiError::internal())?);
    }
    headers.insert(header::ACCESS_CONTROL_EXPOSE_HEADERS,HeaderValue::from_static("Content-Disposition, Content-Type, Content-Length, X-Flow-Asset-SHA256, X-Flow-Share-ID, X-Flow-Asset-Alias"));
    Ok((headers, asset.bytes).into_response())
}

pub async fn get_flow_v2_source_block_shares(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    bot: Option<Extension<BotAuthContext>>,
    Path(source): Path<Uuid>,
) -> Result<impl IntoResponse, ApiError> {
    Ok(ApiResponse::success(
        crate::flow::block_shares::list_source(&state.db, user(&claims, bot.as_ref())?, source).await?,
    ))
}
pub async fn post_flow_v2_suspend_block_share(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    bot: Option<Extension<BotAuthContext>>,
    Path((source, share)): Path<(Uuid, Uuid)>,
    headers: axum::http::HeaderMap,
) -> Result<impl IntoResponse, ApiError> {
    Ok(ApiResponse::success(
        crate::flow::block_shares::suspend(
            &state.db,
            user(&claims, bot.as_ref())?,
            source,
            share,
            &body_asset_intent(&headers)?,
        )
        .await?,
    ))
}
pub async fn post_flow_v2_revoke_block_share(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    bot: Option<Extension<BotAuthContext>>,
    Path((source, share)): Path<(Uuid, Uuid)>,
    headers: axum::http::HeaderMap,
) -> Result<impl IntoResponse, ApiError> {
    Ok(ApiResponse::success(
        crate::flow::block_shares::revoke(
            &state.db,
            user(&claims, bot.as_ref())?,
            source,
            share,
            &body_asset_intent(&headers)?,
        )
        .await?,
    ))
}
pub async fn post_flow_v2_republish_block_share(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    bot: Option<Extension<BotAuthContext>>,
    Path((source, share)): Path<(Uuid, Uuid)>,
    headers: axum::http::HeaderMap,
    Json(input): Json<crate::flow::block_shares::CreateInput>,
) -> Result<impl IntoResponse, ApiError> {
    Ok(ApiResponse::success(
        crate::flow::block_shares::republish(
            &state.db,
            user(&claims, bot.as_ref())?,
            source,
            share,
            &body_asset_intent(&headers)?,
            input,
        )
        .await?,
    ))
}
pub async fn delete_flow_v2_block_share_placement(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    bot: Option<Extension<BotAuthContext>>,
    Path((share, placement)): Path<(Uuid, Uuid)>,
    headers: axum::http::HeaderMap,
) -> Result<impl IntoResponse, ApiError> {
    Ok(ApiResponse::success(
        crate::flow::block_shares::remove_placement(
            &state.db,
            user(&claims, bot.as_ref())?,
            share,
            placement,
            &body_asset_intent(&headers)?,
        )
        .await?,
    ))
}

pub async fn get_flow_v2_block_share_placements(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    bot: Option<Extension<BotAuthContext>>,
    Query(query): Query<BlockShareDiscoveryQuery>,
) -> Result<impl IntoResponse, ApiError> {
    Ok(ApiResponse::success(
        crate::flow::block_shares::list_target_placements(
            &state.db,
            user(&claims, bot.as_ref())?,
            query.workspace_id,
            query.project_id,
            query.cursor,
        )
        .await?,
    ))
}
