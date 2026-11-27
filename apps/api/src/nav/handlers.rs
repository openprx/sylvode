//! Handlers of the navigation routes: the two resolved menus, the two configuration documents and
//! the per-person preferences.
//!
//! Navigation is presentation, never a grant: every target keeps its own permission check, and the
//! menus only hide what the viewer cannot read. Configuration reads are limited to people who may
//! write it because a raw document names objects the viewer may not see.

use std::collections::HashSet;

use axum::{
    Extension, Json,
    extract::{Path, Query, State},
    response::IntoResponse,
};
use platform::{app::AppState, auth::JwtClaims, timestamp::Timestamp};
use sea_orm::{DbBackend, FromQueryResult, Statement};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use uuid::Uuid;

use super::{
    bot,
    doc::{self, NavDoc, NavScope},
    prefs::{self, Prefs},
    refs::{self, Place, Refs, Where, locate},
    resolve::{self, Inputs, Viewer},
    store::{self, Writer},
};
use crate::{
    error::ApiError,
    flow::policy::{authorize_flow_objects, begin_flow_read},
    forms::permissions::form_action_allowed,
    layouts::access::{
        Principal, Scope, ScopeKind, member, require_read, require_write, resolve_scope, resolve_stored_scope,
    },
    middleware::{
        bot_auth::{BotAuthContext, require_workspace_access_from_auth},
        scoped_bot::{ProjectFilter, readable_projects},
    },
    response::ApiResponse,
};

fn is_admin_role(role: &str) -> bool {
    matches!(role.trim().to_ascii_lowercase().as_str(), "owner" | "admin")
}

/// Where a request's scope sits, from the scope the layout access rules resolved.
fn place_of(nav: NavScope, scope: &Scope) -> Where {
    Where {
        scope: nav,
        workspace_id: scope.workspace_id,
        project_id: match nav {
            NavScope::Workspace => None,
            NavScope::Project => scope.project_id,
        },
    }
}

async fn workspace_scope(state: &AppState, id: Uuid) -> Result<(Scope, Where), ApiError> {
    let scope = resolve_scope(&state.db, ScopeKind::Workspace, id).await?;
    let at = place_of(NavScope::Workspace, &scope);
    Ok((scope, at))
}

async fn project_scope(state: &AppState, id: Uuid) -> Result<(Scope, Where), ApiError> {
    let scope = resolve_scope(&state.db, ScopeKind::Project, id).await?;
    let at = place_of(NavScope::Project, &scope);
    Ok((scope, at))
}

/// The objects of `doc` the viewer may read, for the kinds that carry their own permission.
async fn visible_ids(
    state: &AppState,
    who: &Principal,
    role: &str,
    at: &Where,
    doc: &NavDoc,
    refs: &Refs,
) -> Result<HashSet<Uuid>, ApiError> {
    let filter = match who.bot_id() {
        Some(bot_id) => readable_projects(&state.db, bot_id).await?,
        None => ProjectFilter::All,
    };
    let mut visible = HashSet::new();
    let mut flows = Vec::new();
    for (_, entry) in doc.entries() {
        let target = &entry.target;
        let Some(Place::Found { .. }) = locate(target, at, refs) else {
            continue;
        };
        match target {
            doc::Target::Project { project_id } => {
                if filter.allows(*project_id) {
                    visible.insert(*project_id);
                }
            }
            doc::Target::Form { form_id } => {
                let Some(form) = refs.forms.get(form_id) else {
                    continue;
                };
                if filter.allows(form.project_id) && form_action_allowed(state, *form_id, role, "form.view").await? {
                    visible.insert(*form_id);
                }
            }
            doc::Target::FlowPage { object_id } => flows.push(*object_id),
            doc::Target::LayoutPage { layout_id } => {
                let Some(layout) = refs.layouts.get(layout_id) else {
                    continue;
                };
                let Ok(scope) = resolve_stored_scope(&state.db, &layout.scope_kind, layout.scope_id).await else {
                    continue;
                };
                if require_read(state, who, &scope).await.is_ok() {
                    visible.insert(*layout_id);
                }
            }
            doc::Target::Builtin { .. } | doc::Target::PluginApp { .. } | doc::Target::External { .. } => {}
        }
    }
    if !flows.is_empty() {
        // A workspace without Flow, or a permission epoch that moved under us, shows no Flow pages.
        if let Ok(context) = begin_flow_read(state, &who.extensions(), at.workspace_id).await
            && let Ok(Some(allowed)) = authorize_flow_objects(
                state,
                &context,
                &flows,
                crate::flow::collab::authz::PermissionLevel::View,
            )
            .await
        {
            visible.extend(flows.iter().zip(allowed).filter(|(_, ok)| *ok).map(|(id, _)| *id));
        }
    }
    Ok(visible)
}

#[derive(Debug, Deserialize)]
pub struct MenuQuery {
    #[serde(default)]
    include_user_hidden: Option<String>,
}

impl MenuQuery {
    fn include(&self) -> bool {
        matches!(self.include_user_hidden.as_deref(), Some("1" | "true"))
    }
}

/// The stored document of a scope, or the defaults when nobody configured it.
async fn effective_doc(
    state: &AppState,
    at: &Where,
    project_type: Option<&str>,
) -> Result<(NavDoc, i32, bool), ApiError> {
    match store::load_config(&state.db, at.workspace_id, at.project_id).await? {
        Some(row) => {
            let doc = serde_json::from_value(row.doc).map_err(|_| ApiError::internal())?;
            Ok((doc, row.version, true))
        }
        None => Ok((doc::defaults(at.scope, project_type), 0, false)),
    }
}

async fn menu(
    state: &AppState,
    who: &Principal,
    at: Where,
    project_type: Option<String>,
    include_user_hidden: bool,
) -> Result<resolve::Resolved, ApiError> {
    let role = member(state, who, at.workspace_id).await?;
    let (doc, version, customised) = effective_doc(state, &at, project_type.as_deref()).await?;
    let refs = refs::load(&state.db, &doc).await?;
    let visible = visible_ids(state, who, &role, &at, &doc, &refs).await?;
    let user = who.user_id()?;
    let stored_prefs = match user {
        Some(user_id) => store::load_prefs(&state.db, user_id, at.workspace_id, at.project_id)
            .await?
            .and_then(|row| prefs::parse(&row.prefs).ok()),
        None => None,
    };
    Ok(resolve::resolve(
        &doc,
        version,
        customised,
        &Inputs {
            at,
            project_type: project_type.as_deref(),
            viewer: Viewer {
                is_bot: user.is_none(),
                is_admin: user.is_some() && is_admin_role(&role),
            },
            refs: &refs,
            visible: &visible,
            prefs: stored_prefs.as_ref(),
            include_user_hidden,
        },
    ))
}

/// `GET /api/v1/workspaces/{workspace_id}/nav`
pub async fn get_workspace_menu(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    bot: Option<Extension<BotAuthContext>>,
    Path(workspace_id): Path<Uuid>,
    Query(query): Query<MenuQuery>,
) -> Result<impl IntoResponse, ApiError> {
    let who = Principal::new(claims, bot);
    let (scope, at) = workspace_scope(&state, workspace_id).await?;
    require_read(&state, &who, &scope).await?;
    Ok(ApiResponse::success(
        menu(&state, &who, at, None, query.include()).await?,
    ))
}

/// `GET /api/v1/projects/{project_id}/nav`
pub async fn get_project_menu(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    bot: Option<Extension<BotAuthContext>>,
    Path(project_id): Path<Uuid>,
    Query(query): Query<MenuQuery>,
) -> Result<impl IntoResponse, ApiError> {
    let who = Principal::new(claims, bot);
    let (scope, at) = project_scope(&state, project_id).await?;
    require_read(&state, &who, &scope).await?;
    let project_type = store::project_type(&state.db, project_id).await?;
    Ok(ApiResponse::success(
        menu(&state, &who, at, project_type, query.include()).await?,
    ))
}

#[derive(Debug, Serialize)]
struct ConfigBody {
    doc: NavDoc,
    version: i32,
    is_default: bool,
    updated_at: Option<Timestamp>,
    broken: Vec<String>,
}

async fn config_body(state: &AppState, at: &Where, project_type: Option<&str>) -> Result<ConfigBody, ApiError> {
    match store::load_config(&state.db, at.workspace_id, at.project_id).await? {
        Some(row) => {
            let doc: NavDoc = serde_json::from_value(row.doc).map_err(|_| ApiError::internal())?;
            let refs = refs::load(&state.db, &doc).await?;
            let broken = resolve::broken(&doc, at, &refs);
            Ok(ConfigBody {
                doc,
                version: row.version,
                is_default: false,
                updated_at: Some(row.updated_at),
                broken,
            })
        }
        None => Ok(ConfigBody {
            doc: doc::defaults(at.scope, project_type),
            version: 0,
            is_default: true,
            updated_at: None,
            broken: Vec::new(),
        }),
    }
}

/// Read access to a configuration: people who may write it, and bots that can read the scope.
async fn require_config_read(state: &AppState, who: &Principal, scope: &Scope) -> Result<(), ApiError> {
    require_read(state, who, scope).await?;
    if who.user_id()?.is_some() {
        let role = member(state, who, scope.workspace_id).await?;
        if !is_admin_role(&role) {
            return Err(ApiError::forbidden(
                "only workspace owners and admins can read the navigation configuration",
            ));
        }
    }
    Ok(())
}

/// Write access: people who administer the workspace; a bot only on a project navigation, with the
/// same rights as for the project's other content (its own limits are `bot::check`).
async fn require_config_write(
    state: &AppState,
    who: &Principal,
    scope: &Scope,
    nav: NavScope,
) -> Result<Writer, ApiError> {
    let role = member(state, who, scope.workspace_id).await?;
    if let Some(user) = who.user_id()? {
        if !is_admin_role(&role) {
            return Err(ApiError::forbidden(
                "only workspace owners and admins can change the navigation",
            ));
        }
        return Ok(Writer {
            user: Some(user),
            bot: None,
        });
    }
    if nav == NavScope::Workspace {
        return Err(ApiError::forbidden("a bot cannot change the workspace navigation"));
    }
    require_write(state, who, scope).await?;
    Ok(Writer {
        user: None,
        bot: who.bot_id(),
    })
}

/// `GET /api/v1/workspaces/{workspace_id}/nav/config`
pub async fn get_workspace_config(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    bot: Option<Extension<BotAuthContext>>,
    Path(workspace_id): Path<Uuid>,
) -> Result<impl IntoResponse, ApiError> {
    let who = Principal::new(claims, bot);
    let (scope, at) = workspace_scope(&state, workspace_id).await?;
    require_config_read(&state, &who, &scope).await?;
    Ok(ApiResponse::success(config_body(&state, &at, None).await?))
}

/// `GET /api/v1/projects/{project_id}/nav/config`
pub async fn get_project_config(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    bot: Option<Extension<BotAuthContext>>,
    Path(project_id): Path<Uuid>,
) -> Result<impl IntoResponse, ApiError> {
    let who = Principal::new(claims, bot);
    let (scope, at) = project_scope(&state, project_id).await?;
    require_config_read(&state, &who, &scope).await?;
    let project_type = store::project_type(&state.db, project_id).await?;
    Ok(ApiResponse::success(
        config_body(&state, &at, project_type.as_deref()).await?,
    ))
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PutConfig {
    doc: Value,
    expected_version: i32,
}

async fn put_config(
    state: &AppState,
    who: &Principal,
    scope: &Scope,
    at: Where,
    project_type: Option<String>,
    req: PutConfig,
) -> Result<ConfigBody, ApiError> {
    let writer = require_config_write(state, who, scope, at.scope).await?;
    let doc = doc::parse(&req.doc, at.scope)?;
    let refs = refs::load(&state.db, &doc).await?;
    refs::validate_targets(&doc, &at, &refs)?;
    if writer.bot.is_some() {
        let (stored, _, _) = effective_doc(state, &at, project_type.as_deref()).await?;
        bot::check(&stored, &doc)?;
    }
    let value = serde_json::to_value(&doc).map_err(|_| ApiError::internal())?;
    let saved = if req.expected_version == 0 {
        store::insert_config(&state.db, at.workspace_id, at.project_id, &value, writer).await?
    } else {
        store::update_config(
            &state.db,
            at.workspace_id,
            at.project_id,
            &value,
            req.expected_version,
            writer,
        )
        .await?
    };
    let row = saved.ok_or_else(|| {
        ApiError::conflict_coded(
            "version_conflict",
            "the navigation changed since it was read; reload it and apply the change again",
            None,
        )
    })?;
    let broken = resolve::broken(&doc, &at, &refs);
    Ok(ConfigBody {
        doc,
        version: row.version,
        is_default: false,
        updated_at: Some(row.updated_at),
        broken,
    })
}

/// `PUT /api/v1/workspaces/{workspace_id}/nav/config`
pub async fn put_workspace_config(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    bot: Option<Extension<BotAuthContext>>,
    Path(workspace_id): Path<Uuid>,
    Json(req): Json<PutConfig>,
) -> Result<impl IntoResponse, ApiError> {
    let who = Principal::new(claims, bot);
    let (scope, at) = workspace_scope(&state, workspace_id).await?;
    Ok(ApiResponse::success(
        put_config(&state, &who, &scope, at, None, req).await?,
    ))
}

/// `PUT /api/v1/projects/{project_id}/nav/config`
pub async fn put_project_config(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    bot: Option<Extension<BotAuthContext>>,
    Path(project_id): Path<Uuid>,
    Json(req): Json<PutConfig>,
) -> Result<impl IntoResponse, ApiError> {
    let who = Principal::new(claims, bot);
    let (scope, at) = project_scope(&state, project_id).await?;
    let project_type = store::project_type(&state.db, project_id).await?;
    Ok(ApiResponse::success(
        put_config(&state, &who, &scope, at, project_type, req).await?,
    ))
}

/// `DELETE /api/v1/workspaces/{workspace_id}/nav/config`: back to the defaults.
pub async fn delete_workspace_config(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    bot: Option<Extension<BotAuthContext>>,
    Path(workspace_id): Path<Uuid>,
) -> Result<impl IntoResponse, ApiError> {
    let who = Principal::new(claims, bot);
    let (scope, at) = workspace_scope(&state, workspace_id).await?;
    require_config_write(&state, &who, &scope, NavScope::Workspace).await?;
    store::delete_config(&state.db, at.workspace_id, None).await?;
    Ok(ApiResponse::success(config_body(&state, &at, None).await?))
}

/// `DELETE /api/v1/projects/{project_id}/nav/config`: back to the defaults.
pub async fn delete_project_config(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    bot: Option<Extension<BotAuthContext>>,
    Path(project_id): Path<Uuid>,
) -> Result<impl IntoResponse, ApiError> {
    let who = Principal::new(claims, bot);
    let (scope, at) = project_scope(&state, project_id).await?;
    let writer = require_config_write(&state, &who, &scope, NavScope::Project).await?;
    if writer.bot.is_some() {
        // Resetting would drop every lock and link the document holds, so it is a person's call.
        let (stored, _, customised) = effective_doc(&state, &at, None).await?;
        if customised {
            bot::check(&stored, &doc::defaults(NavScope::Project, None))?;
        }
    }
    store::delete_config(&state.db, at.workspace_id, at.project_id).await?;
    let project_type = store::project_type(&state.db, project_id).await?;
    Ok(ApiResponse::success(
        config_body(&state, &at, project_type.as_deref()).await?,
    ))
}

#[derive(Debug, Deserialize)]
pub struct PrefsQuery {
    workspace_id: Uuid,
    #[serde(default)]
    project_id: Option<Uuid>,
}

/// The signed-in person and the scope their preferences belong to.
async fn prefs_scope(
    state: &AppState,
    claims: &JwtClaims,
    query: &PrefsQuery,
) -> Result<(Uuid, Uuid, Option<Uuid>), ApiError> {
    let user_id = Uuid::parse_str(&claims.sub).map_err(|_| ApiError::Unauthorized("invalid user id".to_string()))?;
    require_workspace_access_from_auth(state, claims, None, query.workspace_id).await?;
    if let Some(project_id) = query.project_id {
        #[derive(FromQueryResult)]
        struct One {
            #[allow(dead_code, reason = "only the presence of the row matters")]
            id: Uuid,
        }
        One::find_by_statement(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT id FROM projects WHERE id = $1 AND workspace_id = $2",
            vec![project_id.into(), query.workspace_id.into()],
        ))
        .one(&state.db)
        .await?
        .ok_or_else(|| ApiError::not_found("project not found"))?;
    }
    Ok((user_id, query.workspace_id, query.project_id))
}

fn prefs_body(prefs: Value, updated_at: Option<Timestamp>) -> Value {
    json!({ "prefs": prefs, "updated_at": updated_at })
}

/// `GET /api/v1/me/nav-prefs?workspace_id=&project_id=`
pub async fn get_prefs(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    Query(query): Query<PrefsQuery>,
) -> Result<impl IntoResponse, ApiError> {
    let (user, workspace, project) = prefs_scope(&state, &claims, &query).await?;
    let body = match store::load_prefs(&state.db, user, workspace, project).await? {
        Some(row) => prefs_body(row.prefs, Some(row.updated_at)),
        None => prefs_body(
            serde_json::to_value(Prefs::empty()).map_err(|_| ApiError::internal())?,
            None,
        ),
    };
    Ok(ApiResponse::success(body))
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PutPrefs {
    prefs: Value,
}

/// `PUT /api/v1/me/nav-prefs?workspace_id=&project_id=`
pub async fn put_prefs(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    Query(query): Query<PrefsQuery>,
    Json(req): Json<PutPrefs>,
) -> Result<impl IntoResponse, ApiError> {
    let (user, workspace, project) = prefs_scope(&state, &claims, &query).await?;
    let parsed = prefs::parse(&req.prefs)?;
    let value = serde_json::to_value(&parsed).map_err(|_| ApiError::internal())?;
    let row = store::save_prefs(&state.db, user, workspace, project, &value).await?;
    Ok(ApiResponse::success(prefs_body(row.prefs, Some(row.updated_at))))
}

/// `DELETE /api/v1/me/nav-prefs?workspace_id=&project_id=`
pub async fn delete_prefs(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    Query(query): Query<PrefsQuery>,
) -> Result<impl IntoResponse, ApiError> {
    let (user, workspace, project) = prefs_scope(&state, &claims, &query).await?;
    store::delete_prefs(&state.db, user, workspace, project).await?;
    Ok(ApiResponse::success(prefs_body(
        serde_json::to_value(Prefs::empty()).map_err(|_| ApiError::internal())?,
        None,
    )))
}
