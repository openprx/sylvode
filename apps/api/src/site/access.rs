//! Who may administer a site from the console (contract C3 section 6.6, LEAD AUD-C3-M11, C3-6,
//! C3-6b): readers are members of the site's workspace (a scoped bot with `read` in the project);
//! site creation and retitling are open to members and to bots with `write`; page writes to
//! workspace owners and admins, the site's `site_editor` users and bots with `write`; publishing,
//! settings, bindings, roles and deletion to human workspace owners and admins only.
//!
//! Every id is resolved inside the caller's reach; an outsider gets `not_found`.

use axum::Extension;
use platform::{app::AppState, auth::JwtClaims};
use sea_orm::{FromQueryResult, Statement};
use uuid::Uuid;

use super::store::{self, LayoutRow, SiteRow};
use crate::error::ApiError;
use crate::layouts::access::require_form_view;
use crate::middleware::bot_auth::{
    BotAuthContext, BotPermission, ensure_bot_permission, require_workspace_access_from_auth,
};
use crate::middleware::scoped_bot::{Level, Secondary, ensure_bot_level};

/// The authenticated console caller: a person, or a bot token with its person-shaped claims.
pub struct Who {
    claims: JwtClaims,
    bot: Option<BotAuthContext>,
}

impl Who {
    pub(crate) fn new(claims: JwtClaims, bot: Option<Extension<BotAuthContext>>) -> Self {
        Self {
            claims,
            bot: bot.map(|Extension(bot)| bot),
        }
    }

    pub(crate) const fn is_bot(&self) -> bool {
        self.bot.is_some()
    }

    /// The bot behind the request; `None` for a person.
    pub(crate) fn bot_id(&self) -> Option<Uuid> {
        self.bot.as_ref().map(|bot| bot.bot_id)
    }

    /// The person behind the request; `None` for a bot.
    pub(crate) fn user_id(&self) -> Result<Option<Uuid>, ApiError> {
        if self.bot.is_some() {
            return Ok(None);
        }
        Uuid::parse_str(&self.claims.sub)
            .map(Some)
            .map_err(|_| ApiError::Unauthorized("invalid user id".to_string()))
    }
}

fn site_not_found() -> ApiError {
    ApiError::not_found("site not found")
}

fn as_not_found(error: ApiError) -> ApiError {
    match error {
        ApiError::Forbidden(_) | ApiError::NotFound(_) => site_not_found(),
        other => other,
    }
}

/// The caller's workspace role; outsiders (and bots of another workspace) get `not_found`.
async fn member(state: &AppState, who: &Who, workspace_id: Uuid) -> Result<String, ApiError> {
    if who.bot.as_ref().is_some_and(|bot| bot.workspace_id != workspace_id) {
        return Err(site_not_found());
    }
    let (_, role, _) = require_workspace_access_from_auth(state, &who.claims, who.bot.as_ref(), workspace_id)
        .await
        .map_err(as_not_found)?;
    Ok(role)
}

fn is_admin(role: &str) -> bool {
    matches!(role.trim().to_ascii_lowercase().as_str(), "owner" | "admin")
}

async fn bot_level(state: &AppState, who: &Who, project_id: Uuid, level: Level) -> Result<(), ApiError> {
    let Some(bot) = who.bot.as_ref() else {
        return Ok(());
    };
    if level == Level::Write {
        ensure_bot_permission(bot, BotPermission::Write)?;
    }
    ensure_bot_level(&state.db, Some(bot.bot_id), Secondary::Project(project_id), level)
        .await
        .map_err(|error| {
            if level == Level::Read {
                as_not_found(error)
            } else {
                error
            }
        })
}

/// A project's workspace; an unknown project is `not_found`.
pub async fn project_workspace(state: &AppState, project_id: Uuid) -> Result<Uuid, ApiError> {
    #[derive(FromQueryResult)]
    struct Row {
        workspace_id: Uuid,
    }
    Row::find_by_statement(Statement::from_sql_and_values(
        sea_orm::DbBackend::Postgres,
        "SELECT workspace_id FROM projects WHERE id = $1",
        vec![project_id.into()],
    ))
    .one(&state.db)
    .await?
    .map(|row| row.workspace_id)
    .ok_or_else(|| ApiError::not_found("project not found"))
}

/// Read access to a project's sites.
pub async fn require_project_read(state: &AppState, who: &Who, project_id: Uuid) -> Result<Uuid, ApiError> {
    let workspace_id = project_workspace(state, project_id).await?;
    member(state, who, workspace_id).await?;
    bot_level(state, who, project_id, Level::Read).await?;
    Ok(workspace_id)
}

/// Creating a site in a project: members and bots with `write`.
pub async fn require_project_write(state: &AppState, who: &Who, project_id: Uuid) -> Result<Uuid, ApiError> {
    let workspace_id = project_workspace(state, project_id).await?;
    member(state, who, workspace_id).await?;
    bot_level(state, who, project_id, Level::Write).await?;
    Ok(workspace_id)
}

/// Loads a site the caller may read.
pub async fn readable_site(state: &AppState, who: &Who, site_id: Uuid) -> Result<SiteRow, ApiError> {
    let site = store::load_site(&state.db, site_id).await?.ok_or_else(site_not_found)?;
    member(state, who, site.workspace_id).await?;
    bot_level(state, who, site.project_id, Level::Read).await?;
    Ok(site)
}

/// Loads a site whose pages and title the caller may write: owners, admins and the site's
/// `site_editor` users; bots with `write` in the project (the caller applies the draft rule).
pub async fn page_writable_site(state: &AppState, who: &Who, site_id: Uuid) -> Result<SiteRow, ApiError> {
    let site = readable_site(state, who, site_id).await?;
    if who.is_bot() {
        bot_level(state, who, site.project_id, Level::Write).await?;
        return Ok(site);
    }
    let role = member(state, who, site.workspace_id).await?;
    if is_admin(&role) {
        return Ok(site);
    }
    let editor = match who.user_id()? {
        Some(user_id) => store::load_role(&state.db, site.id, user_id)
            .await?
            .is_some_and(|row| row.role == "site_editor"),
        None => false,
    };
    if editor {
        Ok(site)
    } else {
        Err(ApiError::forbidden(
            "only workspace owners, admins and site editors change site pages",
        ))
    }
}

/// Loads a site for a human-only administration route: workspace owners and admins.
pub async fn admin_site(state: &AppState, who: &Who, site_id: Uuid) -> Result<SiteRow, ApiError> {
    if who.is_bot() {
        return Err(ApiError::forbidden("this route is for people only"));
    }
    let site = store::load_site(&state.db, site_id).await?.ok_or_else(site_not_found)?;
    let role = member(state, who, site.workspace_id).await?;
    if is_admin(&role) {
        Ok(site)
    } else {
        Err(ApiError::forbidden("only workspace owners and admins administer sites"))
    }
}

/// A page may show a form-scope layout only to a caller who holds `form.view` on that form (person
/// or bot, by the workspace role; bots are bound by their project level first). A caller without it
/// gets the same `not_found` as for a missing layout. Layouts of other scopes pass.
pub async fn require_layout_viewable(
    state: &AppState,
    who: &Who,
    site: &SiteRow,
    layout: &LayoutRow,
) -> Result<(), ApiError> {
    if layout.scope_kind != "form" {
        return Ok(());
    }
    let denied = || ApiError::not_found("layout not found");
    bot_level(state, who, site.project_id, Level::Read)
        .await
        .map_err(|_| denied())?;
    let role = member(state, who, site.workspace_id).await.map_err(|_| denied())?;
    require_form_view(state, &role, layout.scope_id)
        .await
        .map_err(|_| denied())
}

/// The draft-path form of [`require_layout_viewable`]: judges the layout a draft names by id; an
/// id that does not resolve is left to the draft's own review report.
pub async fn require_layout_id_viewable(
    state: &AppState,
    who: &Who,
    site: &SiteRow,
    layout_id: Uuid,
) -> Result<(), ApiError> {
    match store::load_layout(&state.db, layout_id).await? {
        Some(layout) if layout.workspace_id == site.workspace_id => {
            require_layout_viewable(state, who, site, &layout).await
        }
        _ => Ok(()),
    }
}
