//! Who may read or change what: scope resolution and the permission rules of the layout routes.
//!
//! Every id is resolved inside the caller's reach and refused `not_found` otherwise (contract C3
//! section 0.1), so a caller learns nothing about scopes of another workspace.

use axum::Extension;
use platform::{app::AppState, auth::JwtClaims};
use sea_orm::{ConnectionTrait, DbBackend, FromQueryResult, Statement};
use uuid::Uuid;

use crate::{
    error::ApiError,
    forms::permissions::form_action_allowed,
    middleware::{
        bot_auth::{BotAuthContext, BotPermission, ensure_bot_permission, require_workspace_access_from_auth},
        scoped_bot::{Level, ProjectFilter, Secondary, ensure_bot_level, readable_projects},
    },
};

/// The authenticated caller of a request: a person, or a bot token with its person-shaped claims.
pub struct Principal {
    claims: JwtClaims,
    bot: Option<BotAuthContext>,
}

impl Principal {
    pub(crate) fn new(claims: JwtClaims, bot: Option<Extension<BotAuthContext>>) -> Self {
        Self {
            claims,
            bot: bot.map(|Extension(ctx)| ctx),
        }
    }

    /// The person behind the request; `None` for a bot, whose id is not a user id.
    pub(crate) fn user_id(&self) -> Result<Option<Uuid>, ApiError> {
        if self.bot.is_some() {
            return Ok(None);
        }
        Uuid::parse_str(&self.claims.sub)
            .map(Some)
            .map_err(|_| ApiError::Unauthorized("invalid user id".to_string()))
    }

    pub(crate) fn bot_id(&self) -> Option<Uuid> {
        self.bot.as_ref().map(|bot| bot.bot_id)
    }

    /// The request extensions the shared authorisation helpers read (claims and bot context).
    pub(crate) fn extensions(&self) -> axum::http::Extensions {
        let mut extensions = axum::http::Extensions::new();
        extensions.insert(self.claims.clone());
        if let Some(bot) = &self.bot {
            extensions.insert(bot.clone());
        }
        extensions
    }
}

/// The three scope kinds a console caller may store layouts and placements in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScopeKind {
    Workspace,
    Project,
    Form,
}

impl ScopeKind {
    pub(crate) fn parse(raw: &str) -> Result<Self, ApiError> {
        match raw {
            "workspace" => Ok(Self::Workspace),
            "project" => Ok(Self::Project),
            "form" => Ok(Self::Form),
            "site" => Err(ApiError::bad_request_coded(
                "scope_kind_unsupported",
                "site layouts are saved with their site pages",
            )),
            _ => Err(ApiError::bad_request_coded(
                "scope_kind_invalid",
                "scope_kind must be one of: workspace, project, form",
            )),
        }
    }

    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::Workspace => "workspace",
            Self::Project => "project",
            Self::Form => "form",
        }
    }
}

/// A scope resolved to the workspace and project that own it.
#[derive(Debug, Clone, Copy)]
pub struct Scope {
    pub(crate) kind: ScopeKind,
    pub(crate) id: Uuid,
    pub(crate) workspace_id: Uuid,
    /// The project of a project or form scope.
    pub(crate) project_id: Option<Uuid>,
}

impl Scope {
    /// Whether a row stored at `outer` may be used at this scope: a workspace layout anywhere in
    /// its workspace, a project layout in that project or its forms, a form layout in that form.
    pub(super) fn lies_inside(&self, outer_kind: &str, outer_id: Uuid, outer_workspace: Uuid) -> bool {
        if self.workspace_id != outer_workspace {
            return false;
        }
        match outer_kind {
            "workspace" => outer_id == self.workspace_id,
            "project" => self.project_id == Some(outer_id),
            "form" => self.kind == ScopeKind::Form && self.id == outer_id,
            _ => false,
        }
    }
}

fn scope_not_found() -> ApiError {
    ApiError::not_found("scope not found")
}

/// Resolves a scope by kind and id; a scope that does not exist is `not_found`.
pub async fn resolve_scope(db: &impl ConnectionTrait, kind: ScopeKind, id: Uuid) -> Result<Scope, ApiError> {
    #[derive(FromQueryResult)]
    struct Owner {
        workspace_id: Uuid,
        project_id: Option<Uuid>,
    }
    let sql = match kind {
        ScopeKind::Workspace => "SELECT id AS workspace_id, NULL::uuid AS project_id FROM workspaces WHERE id = $1",
        ScopeKind::Project => "SELECT workspace_id, id AS project_id FROM projects WHERE id = $1",
        ScopeKind::Form => "SELECT workspace_id, project_id FROM project_forms WHERE id = $1",
    };
    let owner = Owner::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        sql,
        vec![id.into()],
    ))
    .one(db)
    .await?
    .ok_or_else(scope_not_found)?;
    Ok(Scope {
        kind,
        id,
        workspace_id: owner.workspace_id,
        project_id: owner.project_id,
    })
}

/// Resolves the scope stored on a row (`scope_kind` text and id).
pub async fn resolve_stored_scope(db: &impl ConnectionTrait, kind: &str, id: Uuid) -> Result<Scope, ApiError> {
    let kind = match kind {
        "workspace" => ScopeKind::Workspace,
        "project" => ScopeKind::Project,
        "form" => ScopeKind::Form,
        _ => return Err(scope_not_found()),
    };
    resolve_scope(db, kind, id).await
}

/// Resolves a project; used by the slot resolution, whose context names the project.
pub async fn resolve_project(db: &impl ConnectionTrait, project_id: Uuid) -> Result<Scope, ApiError> {
    resolve_scope(db, ScopeKind::Project, project_id).await
}

/// The caller's membership in the scope's workspace. A person outside the workspace and a bot of
/// another workspace both get `not_found`.
pub async fn member(state: &AppState, who: &Principal, workspace_id: Uuid) -> Result<String, ApiError> {
    if who.bot.as_ref().is_some_and(|bot| bot.workspace_id != workspace_id) {
        return Err(scope_not_found());
    }
    let (_, role, _) = require_workspace_access_from_auth(state, &who.claims, who.bot.as_ref(), workspace_id).await?;
    Ok(role)
}

fn reads_as_not_found(error: ApiError) -> ApiError {
    match error {
        ApiError::Forbidden(_) => scope_not_found(),
        other => other,
    }
}

/// Read access to a scope: a member of its workspace; a bot limited to projects only in the
/// scope's project, and never at workspace scope. A form scope is as private as the form: the
/// caller's role must hold `form.view` on it, else the scope reads as `not_found`.
pub async fn require_read(state: &AppState, who: &Principal, scope: &Scope) -> Result<(), ApiError> {
    let role = member(state, who, scope.workspace_id).await?;
    if let Some(bot_id) = who.bot_id() {
        match scope.project_id {
            Some(project_id) => ensure_bot_level(&state.db, Some(bot_id), Secondary::Project(project_id), Level::Read)
                .await
                .map_err(reads_as_not_found)?,
            None => match readable_projects(&state.db, bot_id).await? {
                ProjectFilter::All => {}
                ProjectFilter::Only(_) => return Err(scope_not_found()),
            },
        }
    }
    if scope.kind == ScopeKind::Form {
        require_form_view(state, &role, scope.id).await?;
    }
    Ok(())
}

/// Fails closed with `not_found` unless `role` holds `form.view` on the form.
pub async fn require_form_view(state: &AppState, role: &str, form_id: Uuid) -> Result<(), ApiError> {
    if form_action_allowed(state, form_id, role, "form.view").await? {
        Ok(())
    } else {
        Err(scope_not_found())
    }
}

/// The caller's workspace role, for the checks that need it beyond `require_read`.
pub async fn caller_role(state: &AppState, who: &Principal, workspace_id: Uuid) -> Result<String, ApiError> {
    member(state, who, workspace_id).await
}

/// Write access to a scope (contract C3 section 3): workspace scope for owners and admins;
/// project scope for members; form scope for users with `form.design` on the form; a bot needs
/// the `write` permission and `write` in the scope's project, and never writes workspace scope.
pub async fn require_write(state: &AppState, who: &Principal, scope: &Scope) -> Result<(), ApiError> {
    let role = member(state, who, scope.workspace_id).await?;
    if let Some(bot_id) = who.bot_id() {
        let Some(project_id) = scope.project_id else {
            return Err(ApiError::forbidden("a bot cannot change workspace layouts"));
        };
        if let Some(bot) = who.bot.as_ref() {
            ensure_bot_permission(bot, BotPermission::Write)?;
        }
        return ensure_bot_level(&state.db, Some(bot_id), Secondary::Project(project_id), Level::Write)
            .await
            .map_err(reads_as_not_found);
    }
    match scope.kind {
        ScopeKind::Workspace => {
            if matches!(role.trim().to_ascii_lowercase().as_str(), "owner" | "admin") {
                Ok(())
            } else {
                Err(ApiError::forbidden(
                    "only workspace owners and admins can change workspace layouts",
                ))
            }
        }
        ScopeKind::Project => Ok(()),
        ScopeKind::Form => {
            if form_action_allowed(state, scope.id, &role, "form.design").await? {
                Ok(())
            } else {
                Err(ApiError::forbidden("form permission denied: form.design"))
            }
        }
    }
}

/// Refuses a bot: the route is for people only.
pub(super) fn require_person(who: &Principal) -> Result<Uuid, ApiError> {
    who.user_id()?
        .ok_or_else(|| ApiError::forbidden("this route is for people only"))
}

/// Membership only, for routes that name no scope (the catalogues and the validator).
pub(super) fn authenticated(who: &Principal) -> Result<(), ApiError> {
    who.user_id().map(|_| ())
}
