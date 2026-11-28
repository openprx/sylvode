//! Console administration of sites (contract C3 section 6.6): sites, pages, bindings and roles.
//!
//! Bot writes on a published site (pages, the title) are stored as `site_page` drafts and answered
//! `{draft_id}` (LEAD C3-6); a workspace owner or admin approves them. Human users, site editors
//! included, write directly (C3-6b).

use axum::{
    Extension, Json,
    extract::{Path, Query, State},
    response::{IntoResponse, Response},
};
use platform::{app::AppState, auth::JwtClaims};
use sea_orm::{ConnectionTrait, TransactionTrait};
use serde::Deserialize;
use serde_json::{Value, json};
use sv_layout::Validated;
use uuid::Uuid;

use super::access::{
    Who, admin_site, page_writable_site, readable_site, require_layout_viewable, require_project_read,
    require_project_write,
};
use super::drafts::{PageTarget, submit};
use super::layouts::{binding_keys, validate_site_layout};
use super::projection::Projection;
use super::store::{self, NewSite, PageRow, PageValues, SiteBody, SiteRow};
use crate::catalogue::SiteRole;
use crate::error::ApiError;
use crate::middleware::bot_auth::BotAuthContext;
use crate::response::{ApiResponse, PageQuery, PaginatedData};

const MAX_TEXT_CHARS: usize = 200;
const MAX_LOCALES: usize = 8;
const MAX_FRAME_ANCESTORS: usize = 8;

fn invalid(code: &'static str, message: &str) -> ApiError {
    ApiError::bad_request_coded(code, message)
}

fn is_slug(slug: &str) -> bool {
    let bytes = slug.as_bytes();
    (2..=40).contains(&bytes.len())
        && bytes
            .first()
            .is_some_and(|b| b.is_ascii_lowercase() || b.is_ascii_digit())
        && bytes
            .iter()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || *b == b'-')
}

/// A text value: a string, or an i18n map with `en` (at most eight locales).
pub(super) fn check_text(value: &Value, code: &'static str) -> Result<(), ApiError> {
    let fits = |text: &str| !text.trim().is_empty() && text.chars().count() <= MAX_TEXT_CHARS;
    let ok = match value {
        Value::String(text) => fits(text),
        Value::Object(map) => {
            map.len() <= MAX_LOCALES
                && map.get("en").and_then(Value::as_str).is_some_and(fits)
                && map.iter().all(|(locale, text)| {
                    locale.len() <= 16
                        && locale.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
                        && text.as_str().is_some_and(fits)
                })
        }
        _ => false,
    };
    if ok {
        Ok(())
    } else {
        Err(invalid(
            code,
            "a text is a string or a map of locales with `en`, each at most 200 characters",
        ))
    }
}

fn is_route(route: &str) -> bool {
    if route == "/" {
        return true;
    }
    let Some(rest) = route.strip_prefix('/') else {
        return false;
    };
    rest.split('/').enumerate().all(|(index, part)| {
        part.strip_prefix(':').map_or_else(
            || {
                !part.is_empty()
                    && part
                        .bytes()
                        .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
            },
            |name| index > 0 && !name.is_empty() && name.bytes().all(|b| b.is_ascii_lowercase() || b == b'_'),
        )
    })
}

/// The console shape of sites: each row with its link and the installation behind it.
async fn site_bodies(state: &AppState, rows: &[SiteRow]) -> Result<Vec<SiteBody>, ApiError> {
    let ids: Vec<Uuid> = rows.iter().map(|row| row.id).collect();
    let mut installations = store::installations_of_sites(&state.db, &ids).await?;
    let config = crate::config::runtime();
    Ok(rows
        .iter()
        .map(|row| {
            let link = super::link::site_link(
                row.allowed_origin(),
                &row.slug,
                &config.site,
                config.public_url.as_deref(),
            );
            SiteBody::new(row, link, installations.remove(&row.id))
        })
        .collect())
}

async fn site_body(state: &AppState, row: &SiteRow) -> Result<SiteBody, ApiError> {
    site_bodies(state, std::slice::from_ref(row))
        .await?
        .pop()
        .ok_or_else(ApiError::internal)
}

// ---------------------------------------------------------------------------------------------
// Sites

#[derive(Debug, Deserialize)]
pub struct ListQuery {
    #[serde(flatten)]
    page: PageQuery,
}

/// `GET /api/v1/projects/{project_id}/sites`
pub async fn list_sites(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    bot: Option<Extension<BotAuthContext>>,
    Path(project_id): Path<Uuid>,
    Query(query): Query<ListQuery>,
) -> Result<impl IntoResponse, ApiError> {
    let who = Who::new(claims, bot);
    require_project_read(&state, &who, project_id).await?;
    let paging = query.page.resolve(50, 200);
    let (rows, total) = store::list_sites(&state.db, project_id, paging.per_page, paging.offset()).await?;
    let items = site_bodies(&state, &rows).await?;
    Ok(ApiResponse::success(PaginatedData::page(items, total, paging)))
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CreateSiteRequest {
    slug: String,
    title: Value,
}

/// `POST /api/v1/projects/{project_id}/sites`: a new site starts as `draft`.
pub async fn create_site(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    bot: Option<Extension<BotAuthContext>>,
    Path(project_id): Path<Uuid>,
    Json(req): Json<CreateSiteRequest>,
) -> Result<impl IntoResponse, ApiError> {
    let who = Who::new(claims, bot);
    let workspace_id = require_project_write(&state, &who, project_id).await?;
    if !is_slug(&req.slug) {
        return Err(invalid(
            "slug_invalid",
            "a slug is 2-40 lowercase letters, digits or hyphens",
        ));
    }
    check_text(&req.title, "title_invalid")?;
    let row = store::insert_site(
        &state.db,
        NewSite {
            workspace_id,
            project_id,
            slug: &req.slug,
            title: &req.title,
            created_by: who.user_id()?,
        },
    )
    .await?
    .ok_or_else(|| ApiError::conflict_coded("slug_taken", "this slug is taken", None))?;
    Ok(ApiResponse::success(site_body(&state, &row).await?))
}

/// `GET /api/v1/sites/{id}`
pub async fn get_site(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    bot: Option<Extension<BotAuthContext>>,
    Path(id): Path<Uuid>,
) -> Result<impl IntoResponse, ApiError> {
    let who = Who::new(claims, bot);
    let site = readable_site(&state, &who, id).await?;
    Ok(ApiResponse::success(site_body(&state, &site).await?))
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UpdateSiteRequest {
    #[serde(default)]
    title: Option<Value>,
    #[serde(default)]
    slug: Option<String>,
}

/// `PATCH /api/v1/sites/{id}`: the title, and the slug while the site is a draft.
///
/// The slug is for owners and admins who are people (it is the site's address); the title keeps the
/// rule of the pages. A published or disabled site refuses a new slug with `site_not_draft`.
pub async fn update_site(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    bot: Option<Extension<BotAuthContext>>,
    Path(id): Path<Uuid>,
    Json(req): Json<UpdateSiteRequest>,
) -> Result<Response, ApiError> {
    let who = Who::new(claims, bot);
    // Rights before shapes: an unauthorised caller gets 403 whatever it sent.
    let site = page_writable_site(&state, &who, id).await?;
    if req.slug.is_some() {
        admin_site(&state, &who, id).await?;
    }
    if req.title.is_none() && req.slug.is_none() {
        return Err(invalid("update_empty", "send a title, a slug or both"));
    }
    if let Some(title) = &req.title {
        check_text(title, "title_invalid")?;
    }
    if let Some(slug) = &req.slug
        && !is_slug(slug)
    {
        return Err(invalid(
            "slug_invalid",
            "a slug is 2-40 lowercase letters, digits or hyphens",
        ));
    }
    if req.slug.is_some() && site.status != "draft" {
        return Err(ApiError::conflict_coded(
            "site_not_draft",
            "the slug of a site changes only while it is a draft",
            None,
        ));
    }
    let tx = state.db.begin().await?;
    let titled = if let Some(title) = &req.title {
        if drafted(&tx, &who, &site).await? {
            drop(tx);
            let target = PageTarget::title(site.id);
            return submit(&state, &who, &site, &target, json!({ "title": title })).await;
        }
        store::update_title(&tx, site.id, title)
            .await?
            .ok_or_else(|| ApiError::not_found("site not found"))?
    } else {
        site.clone()
    };
    let row = if let Some(slug) = &req.slug {
        store::update_slug(&tx, site.id, slug).await?.ok_or_else(|| {
            ApiError::conflict_coded(
                "site_not_draft",
                "the slug of a site changes only while it is a draft",
                None,
            )
        })?
    } else {
        titled
    };
    tx.commit().await?;
    Ok(ApiResponse::success(site_body(&state, &row).await?).into_response())
}

async fn set_status(state: &AppState, who: &Who, id: Uuid, status: &str) -> Result<SiteBody, ApiError> {
    let site = admin_site(state, who, id).await?;
    let row = store::set_status(&state.db, site.id, status)
        .await?
        .ok_or_else(|| ApiError::not_found("site not found"))?;
    site_body(state, &row).await
}

/// `POST /api/v1/sites/{id}/publish`
pub async fn publish_site(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    Path(id): Path<Uuid>,
) -> Result<impl IntoResponse, ApiError> {
    let who = Who::new(claims, None);
    Ok(ApiResponse::success(set_status(&state, &who, id, "published").await?))
}

/// `POST /api/v1/sites/{id}/unpublish`: back to `draft`.
pub async fn unpublish_site(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    Path(id): Path<Uuid>,
) -> Result<impl IntoResponse, ApiError> {
    let who = Who::new(claims, None);
    Ok(ApiResponse::success(set_status(&state, &who, id, "draft").await?))
}

/// `POST /api/v1/sites/{id}/disable`: the site API answers `404` for it.
pub async fn disable_site(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    Path(id): Path<Uuid>,
) -> Result<impl IntoResponse, ApiError> {
    let who = Who::new(claims, None);
    Ok(ApiResponse::success(set_status(&state, &who, id, "disabled").await?))
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SettingsRequest {
    #[serde(default)]
    allowed_origin: Option<String>,
    #[serde(default)]
    frame_ancestors: Option<Vec<String>>,
    #[serde(default)]
    theme: Option<String>,
}

/// An exact `https://host[:port]` origin (S13): no path, query, user info, `*` or `null`.
fn is_https_origin(origin: &str) -> bool {
    let Ok(url) = url::Url::parse(origin) else {
        return false;
    };
    url.scheme() == "https"
        && url
            .host_str()
            .is_some_and(|host| !host.is_empty() && !host.contains('*'))
        && url.username().is_empty()
        && url.password().is_none()
        && url.query().is_none()
        && url.fragment().is_none()
        && url.origin().ascii_serialization() == origin
}

/// `PUT /api/v1/sites/{id}/settings`: replaces the settings.
pub async fn put_settings(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    Path(id): Path<Uuid>,
    Json(req): Json<SettingsRequest>,
) -> Result<impl IntoResponse, ApiError> {
    let who = Who::new(claims, None);
    let site = admin_site(&state, &who, id).await?;
    let mut settings = serde_json::Map::new();
    if let Some(origin) = req.allowed_origin {
        if !is_https_origin(&origin) {
            return Err(invalid(
                "allowed_origin_invalid",
                "allowed_origin is one exact https origin",
            ));
        }
        settings.insert("allowed_origin".to_string(), json!(origin));
    }
    if let Some(ancestors) = req.frame_ancestors {
        if ancestors.len() > MAX_FRAME_ANCESTORS || !ancestors.iter().all(|origin| is_https_origin(origin)) {
            return Err(invalid(
                "frame_ancestors_invalid",
                "frame_ancestors holds at most 8 exact https origins",
            ));
        }
        settings.insert("frame_ancestors".to_string(), json!(ancestors));
    }
    if let Some(theme) = req.theme {
        if theme.is_empty() || theme.len() > 32 || !theme.bytes().all(|b| b.is_ascii_lowercase() || b == b'-') {
            return Err(invalid("theme_invalid", "a theme is a short lowercase name"));
        }
        settings.insert("theme".to_string(), json!(theme));
    }
    let row = store::update_settings(&state.db, site.id, &Value::Object(settings))
        .await?
        .ok_or_else(|| ApiError::not_found("site not found"))?;
    Ok(ApiResponse::success(site_body(&state, &row).await?))
}

/// `DELETE /api/v1/sites/{id}`
pub async fn delete_site(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    Path(id): Path<Uuid>,
) -> Result<impl IntoResponse, ApiError> {
    let who = Who::new(claims, None);
    let site = admin_site(&state, &who, id).await?;
    if site.is_published() {
        return Err(ApiError::conflict_coded(
            "site_published",
            "unpublish the site before deleting it",
            None,
        ));
    }
    let tx = state.db.begin().await?;
    if !store::delete_site(&tx, site.id).await? {
        return Err(ApiError::conflict_coded(
            "site_published",
            "unpublish the site before deleting it",
            None,
        ));
    }
    tx.commit().await?;
    Ok(ApiResponse::success(json!({ "id": site.id })))
}

// ---------------------------------------------------------------------------------------------
// Pages

/// `GET /api/v1/sites/{id}/pages`
pub async fn list_pages(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    bot: Option<Extension<BotAuthContext>>,
    Path(id): Path<Uuid>,
) -> Result<impl IntoResponse, ApiError> {
    let who = Who::new(claims, bot);
    let site = readable_site(&state, &who, id).await?;
    let pages = store::list_pages(&state.db, site.id).await?;
    Ok(ApiResponse::success(json!({ "items": pages })))
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PageRequest {
    #[serde(default)]
    route: Option<String>,
    #[serde(default)]
    layout_id: Option<Uuid>,
    #[serde(default)]
    audience: Option<String>,
    #[serde(default)]
    nav: Option<Value>,
    #[serde(default)]
    position: Option<i32>,
    #[serde(default)]
    layout_spec: Option<Value>,
}

pub(super) fn check_page_values(route: &str, audience: &str, nav: Option<&Value>) -> Result<(), ApiError> {
    if !is_route(route) || route.len() > 200 {
        return Err(invalid(
            "route_invalid",
            "a route is `/` or lowercase segments, with `:name` parameters",
        ));
    }
    if !matches!(audience, "visitor" | "operator") {
        return Err(invalid("audience_invalid", "audience is visitor or operator"));
    }
    if let Some(nav) = nav.filter(|nav| !nav.is_null()) {
        check_text(nav, "nav_invalid")?;
    }
    Ok(())
}

/// Loads a layout a page may show: an active user layout at this site's scope or in the site's
/// project (or one of its forms). Anything else is `not_found`.
async fn load_page_layout(
    db: &impl ConnectionTrait,
    site: &SiteRow,
    layout_id: Uuid,
) -> Result<store::LayoutRow, ApiError> {
    let not_found = || ApiError::not_found("layout not found");
    let layout = store::load_layout(db, layout_id).await?.ok_or_else(not_found)?;
    if layout.workspace_id != site.workspace_id || layout.status != "active" || layout.installation_id.is_some() {
        return Err(not_found());
    }
    let in_scope = match layout.scope_kind.as_str() {
        "site" => layout.scope_id == site.id,
        "project" => layout.scope_id == site.project_id,
        "form" => store::form_in_project(db, layout.scope_id, site.project_id).await?,
        _ => false,
    };
    if !in_scope {
        return Err(not_found());
    }
    Ok(layout)
}

/// Checks a layout a page may show: in reach (see `load_page_layout`) and valid in site context.
/// Returns the layout row and the verdict. This is the caller-less judgement of a draft review.
pub(super) async fn check_page_layout(
    db: &impl ConnectionTrait,
    site: &SiteRow,
    layout_id: Uuid,
) -> Result<(store::LayoutRow, Validated), ApiError> {
    let layout = load_page_layout(db, site, layout_id).await?;
    let validated = validate_site_layout(&layout.spec, binding_keys(db, site.id).await?)?;
    Ok((layout, validated))
}

/// `check_page_layout` for a caller who saves the page now: a form-scope layout also needs
/// `form.view` of the caller, judged before the spec is validated so a refusal leaks nothing.
async fn check_page_layout_as(
    state: &AppState,
    who: &Who,
    site: &SiteRow,
    layout_id: Uuid,
) -> Result<store::LayoutRow, ApiError> {
    let layout = load_page_layout(&state.db, site, layout_id).await?;
    require_layout_viewable(state, who, site, &layout).await?;
    validate_site_layout(&layout.spec, binding_keys(&state.db, site.id).await?)?;
    Ok(layout)
}

/// The submit-time check of a layout a draft names: missing, out of the site's reach, or a form
/// layout the caller may not view all answer the same `not_found`, before any draft exists. The
/// spec is left to the draft review, which also reports a layout deleted after submit.
async fn require_draft_layout(state: &AppState, who: &Who, site: &SiteRow, layout_id: Uuid) -> Result<(), ApiError> {
    let layout = load_page_layout(&state.db, site, layout_id).await?;
    require_layout_viewable(state, who, site, &layout).await
}

/// Stores a layout spec at the site's scope (a new layout, or the page's own site layout).
pub(super) async fn save_site_layout(
    tx: &sea_orm::DatabaseTransaction,
    site: &SiteRow,
    created_by: Option<Uuid>,
    spec: &Value,
    existing: Option<Uuid>,
) -> Result<Uuid, ApiError> {
    let validated = validate_site_layout(spec, binding_keys(tx, site.id).await?)?;
    let key = validated.layout.key.clone();
    let taken = || ApiError::conflict_coded("layout_key_taken", "a layout of this site already has this key", None);
    if let Some(id) = existing {
        if store::update_site_layout(tx, id, &key, spec, None).await? {
            return Ok(id);
        }
        return Err(taken());
    }
    store::insert_site_layout(tx, site.workspace_id, site.id, &key, spec, created_by)
        .await?
        .ok_or_else(taken)
}

/// `POST /api/v1/sites/{id}/pages`: `layout_id` or `layout_spec` (stored at the site's scope).
pub async fn create_page(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    bot: Option<Extension<BotAuthContext>>,
    Path(id): Path<Uuid>,
    Json(req): Json<PageRequest>,
) -> Result<Response, ApiError> {
    let who = Who::new(claims, bot);
    let site = page_writable_site(&state, &who, id).await?;
    let route = req.route.ok_or_else(|| invalid("route_invalid", "route is required"))?;
    let audience = req.audience.unwrap_or_else(|| "visitor".to_string());
    check_page_values(&route, &audience, req.nav.as_ref())?;
    if req.layout_id.is_some() == req.layout_spec.is_some() {
        return Err(invalid(
            "layout_required",
            "give exactly one of layout_id and layout_spec",
        ));
    }
    let tx = state.db.begin().await?;
    if drafted(&tx, &who, &site).await? {
        drop(tx);
        if let Some(layout_id) = req.layout_id {
            require_draft_layout(&state, &who, &site, layout_id).await?;
        }
        let target = PageTarget::create(site.id);
        let spec = page_spec(
            &route,
            &audience,
            req.nav.as_ref(),
            req.position,
            req.layout_id,
            req.layout_spec,
        );
        return submit(&state, &who, &site, &target, spec).await;
    }
    let layout_id = match (req.layout_id, req.layout_spec.as_ref()) {
        (Some(layout_id), _) => check_page_layout_as(&state, &who, &site, layout_id).await?.id,
        (None, Some(spec)) => save_site_layout(&tx, &site, who.user_id()?, spec, None).await?,
        (None, None) => return Err(ApiError::internal()),
    };
    let page = store::insert_page(
        &tx,
        site.id,
        PageValues {
            route: &route,
            layout_id,
            audience: &audience,
            nav: req.nav.as_ref().filter(|nav| !nav.is_null()),
            position: req.position.unwrap_or(0),
        },
    )
    .await?
    .ok_or_else(|| ApiError::conflict_coded("route_taken", "this site already has a page at this route", None))?;
    tx.commit().await?;
    Ok(ApiResponse::success(page).into_response())
}

/// The `spec` of a `site_page` draft for a page: its values, and the layout it shows (an
/// existing layout or a spec stored at the site's scope on approval).
pub(super) fn page_spec(
    route: &str,
    audience: &str,
    nav: Option<&Value>,
    position: Option<i32>,
    layout_id: Option<Uuid>,
    layout_spec: Option<Value>,
) -> Value {
    let mut spec = serde_json::Map::new();
    spec.insert("route".to_string(), json!(route));
    spec.insert("audience".to_string(), json!(audience));
    if let Some(nav) = nav.filter(|nav| !nav.is_null()) {
        spec.insert("nav".to_string(), nav.clone());
    }
    spec.insert("position".to_string(), json!(position.unwrap_or(0)));
    if let Some(layout_id) = layout_id {
        spec.insert("layout_id".to_string(), json!(layout_id));
    }
    if let Some(layout_spec) = layout_spec {
        spec.insert("layout_spec".to_string(), layout_spec);
    }
    Value::Object(spec)
}

/// A page of a package installation is read-only: only an install, upgrade or revoke changes it.
pub(super) fn refuse_package_page(page: &PageRow) -> Result<(), ApiError> {
    if page.installation_id.is_some() {
        return Err(ApiError::conflict_coded(
            "package_page_readonly",
            "the page belongs to a package installation; change the package instead",
            None,
        ));
    }
    Ok(())
}

/// The page and its site, for a page route (the page id carries no project of its own).
async fn page_and_site(state: &AppState, who: &Who, page_id: Uuid) -> Result<(PageRow, SiteRow), ApiError> {
    let page = store::load_page(&state.db, page_id)
        .await?
        .ok_or_else(|| ApiError::not_found("page not found"))?;
    let site = page_writable_site(state, who, page.site_id)
        .await
        .map_err(|error| match error {
            ApiError::Forbidden(message) => ApiError::Forbidden(message),
            _ => ApiError::not_found("page not found"),
        })?;
    refuse_package_page(&page)?;
    Ok((page, site))
}

/// Whether a bot's write to this site becomes a `site_page` draft (LEAD C3-6). The site's status
/// is re-read under a share lock in the write transaction, so a publish cannot slip between this
/// decision and a bot's direct write.
async fn drafted(tx: &sea_orm::DatabaseTransaction, who: &Who, site: &SiteRow) -> Result<bool, ApiError> {
    let status = store::site_status_for_share(tx, site.id).await?;
    Ok(who.is_bot() && status == "published")
}

/// `PATCH /api/v1/site-pages/{page_id}`: absent members stay; `layout_spec` replaces the page's own
/// site layout (or stores a new one).
pub async fn update_page(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    bot: Option<Extension<BotAuthContext>>,
    Path(page_id): Path<Uuid>,
    Json(req): Json<PageRequest>,
) -> Result<Response, ApiError> {
    let who = Who::new(claims, bot);
    let (page, site) = page_and_site(&state, &who, page_id).await?;
    let route = req.route.unwrap_or_else(|| page.route.clone());
    let audience = req.audience.unwrap_or_else(|| page.audience.clone());
    let nav = match req.nav {
        Some(Value::Null) => None,
        Some(nav) => Some(nav),
        None => page.nav.clone(),
    };
    check_page_values(&route, &audience, nav.as_ref())?;
    let tx = state.db.begin().await?;
    if drafted(&tx, &who, &site).await? {
        drop(tx);
        if req.layout_id.is_some() && req.layout_spec.is_some() {
            return Err(invalid(
                "layout_required",
                "give at most one of layout_id and layout_spec",
            ));
        }
        if let Some(layout_id) = req.layout_id {
            require_draft_layout(&state, &who, &site, layout_id).await?;
        }
        // Without a layout change the draft names the page's current layout, so the reviewer sees it.
        let layout_id = req
            .layout_id
            .or_else(|| req.layout_spec.is_none().then_some(page.layout_id));
        let layout_version = store::load_layout(&state.db, page.layout_id)
            .await?
            .map_or(0, |layout| layout.version);
        let target = PageTarget::update(site.id, &page, layout_version);
        let position = Some(req.position.unwrap_or(page.position));
        let spec = page_spec(&route, &audience, nav.as_ref(), position, layout_id, req.layout_spec);
        return submit(&state, &who, &site, &target, spec).await;
    }
    let current = store::load_layout(&tx, page.layout_id).await?;
    let own_site_layout = current
        .as_ref()
        .filter(|layout| layout.scope_kind == "site" && layout.scope_id == site.id)
        .map(|layout| layout.id);
    let layout_id = match (req.layout_id, req.layout_spec.as_ref()) {
        (Some(_), Some(_)) => {
            return Err(invalid(
                "layout_required",
                "give at most one of layout_id and layout_spec",
            ));
        }
        (Some(layout_id), None) => check_page_layout_as(&state, &who, &site, layout_id).await?.id,
        (None, Some(spec)) => save_site_layout(&tx, &site, who.user_id()?, spec, own_site_layout).await?,
        (None, None) => page.layout_id,
    };
    let updated = store::update_page(
        &tx,
        page.id,
        PageValues {
            route: &route,
            layout_id,
            audience: &audience,
            nav: nav.as_ref(),
            position: req.position.unwrap_or(page.position),
        },
    )
    .await?
    .ok_or_else(|| ApiError::conflict_coded("route_taken", "this site already has a page at this route", None))?;
    if let Some(old) = own_site_layout.filter(|old| *old != layout_id) {
        store::delete_unused_site_layout(&tx, old).await?;
    }
    tx.commit().await?;
    Ok(ApiResponse::success(updated).into_response())
}

/// `DELETE /api/v1/site-pages/{page_id}`: the page's own site layout goes with it.
pub async fn delete_page(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    bot: Option<Extension<BotAuthContext>>,
    Path(page_id): Path<Uuid>,
) -> Result<Response, ApiError> {
    let who = Who::new(claims, bot);
    let (page, site) = page_and_site(&state, &who, page_id).await?;
    let tx = state.db.begin().await?;
    if drafted(&tx, &who, &site).await? {
        drop(tx);
        let target = PageTarget::delete(site.id, &page);
        return submit(&state, &who, &site, &target, json!({})).await;
    }
    store::delete_page(&tx, page.id).await?;
    store::delete_unused_site_layout(&tx, page.layout_id).await?;
    tx.commit().await?;
    Ok(ApiResponse::success(json!({ "id": page.id })).into_response())
}

// ---------------------------------------------------------------------------------------------
// Bindings

fn binding_body(row: &store::BindingRow) -> Value {
    json!({
        "site_id": row.site_id,
        "binding_key": row.binding_key,
        "form_id": row.form_id,
        "projection": row.projection,
    })
}

/// `GET /api/v1/sites/{id}/bindings`
pub async fn list_bindings(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    bot: Option<Extension<BotAuthContext>>,
    Path(id): Path<Uuid>,
) -> Result<impl IntoResponse, ApiError> {
    let who = Who::new(claims, bot);
    let site = readable_site(&state, &who, id).await?;
    let rows = store::list_bindings(&state.db, site.id).await?;
    Ok(ApiResponse::success(
        json!({ "items": rows.iter().map(binding_body).collect::<Vec<_>>() }),
    ))
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BindingRequest {
    form_id: Uuid,
    projection: Value,
}

fn is_binding_key(key: &str) -> bool {
    let bytes = key.as_bytes();
    (1..=40).contains(&bytes.len())
        && bytes.first().is_some_and(u8::is_ascii_lowercase)
        && bytes
            .iter()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || *b == b'_')
}

/// `PUT /api/v1/sites/{id}/bindings/{key}`: the form must belong to the site's project; the
/// projection is checked against its schema.
pub async fn put_binding(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    Path((id, key)): Path<(Uuid, String)>,
    Json(req): Json<BindingRequest>,
) -> Result<impl IntoResponse, ApiError> {
    let who = Who::new(claims, None);
    let site = admin_site(&state, &who, id).await?;
    if !is_binding_key(&key) {
        return Err(invalid(
            "binding_key_invalid",
            "a binding key is 1-40 lowercase letters, digits or `_`",
        ));
    }
    let form = crate::routes::form::find_form(&state, req.form_id)
        .await
        .ok()
        .filter(|form| form.project_id == site.project_id && form.archived_at.is_none())
        .ok_or_else(|| ApiError::not_found("form not found"))?;
    let projection = Projection::parse(&req.projection, &form.schema)?;
    let stored = serde_json::to_value(&projection).map_err(|_| ApiError::internal())?;
    let row = store::upsert_binding(&state.db, site.id, &key, form.id, &stored)
        .await?
        .ok_or_else(ApiError::internal)?;
    Ok(ApiResponse::success(binding_body(&row)))
}

/// `DELETE /api/v1/sites/{id}/bindings/{key}`
pub async fn delete_binding(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    Path((id, key)): Path<(Uuid, String)>,
) -> Result<impl IntoResponse, ApiError> {
    let who = Who::new(claims, None);
    let site = admin_site(&state, &who, id).await?;
    if !store::delete_binding(&state.db, site.id, &key).await? {
        return Err(ApiError::not_found("binding not found"));
    }
    Ok(ApiResponse::success(json!({ "site_id": site.id, "binding_key": key })))
}

// ---------------------------------------------------------------------------------------------
// Roles

/// `GET /api/v1/sites/{id}/roles`
pub async fn list_roles(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    bot: Option<Extension<BotAuthContext>>,
    Path(id): Path<Uuid>,
) -> Result<impl IntoResponse, ApiError> {
    let who = Who::new(claims, bot);
    let site = readable_site(&state, &who, id).await?;
    let rows = store::list_roles(&state.db, site.id).await?;
    Ok(ApiResponse::success(json!({ "items": rows })))
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RoleRequest {
    role: String,
}

/// `PUT /api/v1/sites/{id}/roles/{user_id}`: only for active members of the site's workspace.
pub async fn put_role(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    Path((id, user_id)): Path<(Uuid, Uuid)>,
    Json(req): Json<RoleRequest>,
) -> Result<impl IntoResponse, ApiError> {
    let who = Who::new(claims, None);
    let site = admin_site(&state, &who, id).await?;
    let role =
        SiteRole::parse(&req.role).ok_or_else(|| invalid("role_invalid", "role is site_operator or site_editor"))?;
    if store::workspace_role(&state.db, site.workspace_id, user_id)
        .await?
        .is_none()
    {
        return Err(invalid(
            "user_not_member",
            "the user is not an active member of the site's workspace",
        ));
    }
    let row = store::upsert_role(&state.db, site.id, user_id, role.as_db())
        .await?
        .ok_or_else(ApiError::internal)?;
    Ok(ApiResponse::success(row))
}

/// `DELETE /api/v1/sites/{id}/roles/{user_id}`: the user's sessions of the site end with it.
pub async fn delete_role(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    Path((id, user_id)): Path<(Uuid, Uuid)>,
) -> Result<impl IntoResponse, ApiError> {
    let who = Who::new(claims, None);
    let site = admin_site(&state, &who, id).await?;
    let tx = state.db.begin().await?;
    let removed = store::delete_role(&tx, site.id, user_id).await?;
    tx.commit().await?;
    if !removed {
        return Err(ApiError::not_found("role not found"));
    }
    Ok(ApiResponse::success(json!({ "site_id": site.id, "user_id": user_id })))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slugs_routes_and_keys() {
        assert!(is_slug("help-desk") && is_slug("a1") && !is_slug("a") && !is_slug("-x") && !is_slug("Help"));
        assert!(is_route("/") && is_route("/requests/new") && is_route("/requests/:id"));
        assert!(!is_route("/:id") && !is_route("requests") && !is_route("/a//b") && !is_route("/A"));
        assert!(is_binding_key("requests") && !is_binding_key("1x") && !is_binding_key("a-b"));
    }

    #[test]
    fn texts_need_english() {
        assert!(check_text(&json!("Help"), "x").is_ok());
        assert!(check_text(&json!({"en": "Help", "zh": "帮助"}), "x").is_ok());
        assert!(check_text(&json!({"zh": "帮助"}), "x").is_err());
        assert!(check_text(&json!(""), "x").is_err());
        assert!(check_text(&json!(3), "x").is_err());
    }

    #[test]
    fn origins_are_exact_https() {
        assert!(is_https_origin("https://help.example.com"));
        assert!(is_https_origin("https://help.example.com:8443"));
        assert!(!is_https_origin("http://help.example.com"));
        assert!(!is_https_origin("https://help.example.com/"));
        assert!(!is_https_origin("https://*.example.com"));
        assert!(!is_https_origin("null"));
    }
}
