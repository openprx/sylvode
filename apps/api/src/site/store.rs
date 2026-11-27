//! SQL access to `sites`, `site_pages`, `site_bindings`, `site_roles`, `site_visitors` and
//! `site_sessions` (migration 0137).
//!
//! Rows that belong to a package installation (`installation_id` set) are not returned: resolving
//! them needs the join with the installation's active generation, which arrives with migration
//! 0138. Until then they stay hidden, like the package rows of the layout store.

use platform::timestamp::Timestamp;
use sea_orm::{ConnectionTrait, DbBackend, FromQueryResult, Statement};
use serde::Serialize;
use serde_json::Value;
use uuid::Uuid;

use crate::error::ApiError;

const SITE_COLUMNS: &str = "id, workspace_id, project_id, slug, title, settings, status, created_at";

/// A `sites` row.
#[derive(Debug, Clone, FromQueryResult)]
pub struct SiteRow {
    pub(crate) id: Uuid,
    pub(crate) workspace_id: Uuid,
    pub(crate) project_id: Uuid,
    pub(crate) slug: String,
    pub(crate) title: Value,
    pub(crate) settings: Value,
    pub(crate) status: String,
    pub(crate) created_at: Timestamp,
}

/// `Site` of contract C3 section 6.6.
#[derive(Debug, Serialize)]
pub struct SiteBody {
    id: Uuid,
    project_id: Uuid,
    slug: String,
    title: Value,
    settings: Value,
    status: String,
    created_at: Timestamp,
}

impl From<&SiteRow> for SiteBody {
    fn from(row: &SiteRow) -> Self {
        Self {
            id: row.id,
            project_id: row.project_id,
            slug: row.slug.clone(),
            title: row.title.clone(),
            settings: row.settings.clone(),
            status: row.status.clone(),
            created_at: row.created_at,
        }
    }
}

impl SiteRow {
    pub(crate) fn is_published(&self) -> bool {
        self.status == "published"
    }

    pub(crate) fn is_disabled(&self) -> bool {
        self.status == "disabled"
    }

    /// The exact origin of a separate-origin site (`settings.allowed_origin`), if set.
    pub(crate) fn allowed_origin(&self) -> Option<&str> {
        self.settings
            .get("allowed_origin")
            .and_then(Value::as_str)
            .filter(|origin| !origin.is_empty())
    }
}

fn statement(sql: &str, values: Vec<sea_orm::Value>) -> Statement {
    Statement::from_sql_and_values(DbBackend::Postgres, sql, values)
}

pub(crate) async fn load_site(db: &impl ConnectionTrait, id: Uuid) -> Result<Option<SiteRow>, ApiError> {
    Ok(SiteRow::find_by_statement(statement(
        &format!("SELECT {SITE_COLUMNS} FROM sites WHERE id = $1"),
        vec![id.into()],
    ))
    .one(db)
    .await?)
}

pub(crate) async fn load_site_by_slug(db: &impl ConnectionTrait, slug: &str) -> Result<Option<SiteRow>, ApiError> {
    Ok(SiteRow::find_by_statement(statement(
        &format!("SELECT {SITE_COLUMNS} FROM sites WHERE slug = $1"),
        vec![slug.into()],
    ))
    .one(db)
    .await?)
}

pub(crate) async fn list_sites(
    db: &impl ConnectionTrait,
    project_id: Uuid,
    limit: i64,
    offset: i64,
) -> Result<(Vec<SiteRow>, i64), ApiError> {
    let rows = SiteRow::find_by_statement(statement(
        &format!("SELECT {SITE_COLUMNS} FROM sites WHERE project_id = $1 ORDER BY created_at, id LIMIT $2 OFFSET $3"),
        vec![project_id.into(), limit.into(), offset.into()],
    ))
    .all(db)
    .await?;
    let total = count(
        db,
        "SELECT count(*)::bigint AS n FROM sites WHERE project_id = $1",
        project_id,
    )
    .await?;
    Ok((rows, total))
}

async fn count(db: &impl ConnectionTrait, sql: &str, id: Uuid) -> Result<i64, ApiError> {
    #[derive(FromQueryResult)]
    struct Count {
        n: i64,
    }
    Ok(Count::find_by_statement(statement(sql, vec![id.into()]))
        .one(db)
        .await?
        .map_or(0, |row| row.n))
}

pub(crate) struct NewSite<'a> {
    pub(crate) workspace_id: Uuid,
    pub(crate) project_id: Uuid,
    pub(crate) slug: &'a str,
    pub(crate) title: &'a Value,
    pub(crate) created_by: Option<Uuid>,
}

/// Inserts a draft site; `None` when the slug is taken.
pub(crate) async fn insert_site(db: &impl ConnectionTrait, site: NewSite<'_>) -> Result<Option<SiteRow>, ApiError> {
    Ok(SiteRow::find_by_statement(statement(
        &format!(
            "INSERT INTO sites (workspace_id, project_id, slug, title, created_by) VALUES ($1, $2, $3, $4, $5) \
             ON CONFLICT (slug) DO NOTHING RETURNING {SITE_COLUMNS}"
        ),
        vec![
            site.workspace_id.into(),
            site.project_id.into(),
            site.slug.into(),
            site.title.clone().into(),
            site.created_by.into(),
        ],
    ))
    .one(db)
    .await?)
}

pub(crate) async fn update_title(
    db: &impl ConnectionTrait,
    id: Uuid,
    title: &Value,
) -> Result<Option<SiteRow>, ApiError> {
    Ok(SiteRow::find_by_statement(statement(
        &format!("UPDATE sites SET title = $2 WHERE id = $1 RETURNING {SITE_COLUMNS}"),
        vec![id.into(), title.clone().into()],
    ))
    .one(db)
    .await?)
}

pub(crate) async fn set_status(db: &impl ConnectionTrait, id: Uuid, status: &str) -> Result<Option<SiteRow>, ApiError> {
    Ok(SiteRow::find_by_statement(statement(
        &format!("UPDATE sites SET status = $2 WHERE id = $1 RETURNING {SITE_COLUMNS}"),
        vec![id.into(), status.into()],
    ))
    .one(db)
    .await?)
}

pub(crate) async fn update_settings(
    db: &impl ConnectionTrait,
    id: Uuid,
    settings: &Value,
) -> Result<Option<SiteRow>, ApiError> {
    Ok(SiteRow::find_by_statement(statement(
        &format!("UPDATE sites SET settings = $2 WHERE id = $1 RETURNING {SITE_COLUMNS}"),
        vec![id.into(), settings.clone().into()],
    ))
    .one(db)
    .await?)
}

/// Deletes a site (pages, bindings, roles, visitors and sessions cascade) and then the layouts
/// stored at its scope, which nothing else can reference.
pub(crate) async fn delete_site(db: &impl ConnectionTrait, id: Uuid) -> Result<(), ApiError> {
    db.execute(statement("DELETE FROM sites WHERE id = $1", vec![id.into()]))
        .await?;
    db.execute(statement(
        "DELETE FROM layouts WHERE scope_kind = 'site' AND scope_id = $1",
        vec![id.into()],
    ))
    .await?;
    Ok(())
}

const PAGE_COLUMNS: &str = "id, site_id, route, layout_id, audience, nav, position, installation_id";

#[derive(Debug, Clone, FromQueryResult, Serialize)]
pub(crate) struct PageRow {
    pub(crate) id: Uuid,
    pub(crate) site_id: Uuid,
    pub(crate) route: String,
    pub(crate) layout_id: Uuid,
    pub(crate) audience: String,
    pub(crate) nav: Option<Value>,
    pub(crate) position: i32,
    pub(crate) installation_id: Option<Uuid>,
}

pub(crate) async fn list_pages(db: &impl ConnectionTrait, site_id: Uuid) -> Result<Vec<PageRow>, ApiError> {
    Ok(PageRow::find_by_statement(statement(
        &format!(
            "SELECT {PAGE_COLUMNS} FROM site_pages WHERE site_id = $1 AND installation_id IS NULL \
             ORDER BY position, route"
        ),
        vec![site_id.into()],
    ))
    .all(db)
    .await?)
}

pub(crate) async fn load_page(db: &impl ConnectionTrait, id: Uuid) -> Result<Option<PageRow>, ApiError> {
    Ok(PageRow::find_by_statement(statement(
        &format!("SELECT {PAGE_COLUMNS} FROM site_pages WHERE id = $1 AND installation_id IS NULL"),
        vec![id.into()],
    ))
    .one(db)
    .await?)
}

pub(crate) struct PageValues<'a> {
    pub(crate) route: &'a str,
    pub(crate) layout_id: Uuid,
    pub(crate) audience: &'a str,
    pub(crate) nav: Option<&'a Value>,
    pub(crate) position: i32,
}

/// Inserts a page; `None` when the route is taken.
pub(crate) async fn insert_page(
    db: &impl ConnectionTrait,
    site_id: Uuid,
    page: PageValues<'_>,
) -> Result<Option<PageRow>, ApiError> {
    Ok(PageRow::find_by_statement(statement(
        &format!(
            "INSERT INTO site_pages (site_id, route, layout_id, audience, nav, position) \
             VALUES ($1, $2, $3, $4, $5, $6) ON CONFLICT DO NOTHING RETURNING {PAGE_COLUMNS}"
        ),
        vec![
            site_id.into(),
            page.route.into(),
            page.layout_id.into(),
            page.audience.into(),
            page.nav.cloned().into(),
            page.position.into(),
        ],
    ))
    .one(db)
    .await?)
}

/// Replaces a page's values; `None` when the new route is taken by another page.
pub(crate) async fn update_page(
    db: &impl ConnectionTrait,
    id: Uuid,
    page: PageValues<'_>,
) -> Result<Option<PageRow>, ApiError> {
    let taken = db
        .query_one(statement(
            "SELECT 1 AS one FROM site_pages p JOIN site_pages me ON me.id = $1 \
             WHERE p.site_id = me.site_id AND p.route = $2 AND p.id <> $1 AND p.generation IS NULL",
            vec![id.into(), page.route.into()],
        ))
        .await?
        .is_some();
    if taken {
        return Ok(None);
    }
    Ok(PageRow::find_by_statement(statement(
        &format!(
            "UPDATE site_pages SET route = $2, layout_id = $3, audience = $4, nav = $5, position = $6 \
             WHERE id = $1 RETURNING {PAGE_COLUMNS}"
        ),
        vec![
            id.into(),
            page.route.into(),
            page.layout_id.into(),
            page.audience.into(),
            page.nav.cloned().into(),
            page.position.into(),
        ],
    ))
    .one(db)
    .await?)
}

pub(crate) async fn delete_page(db: &impl ConnectionTrait, id: Uuid) -> Result<(), ApiError> {
    db.execute(statement("DELETE FROM site_pages WHERE id = $1", vec![id.into()]))
        .await?;
    Ok(())
}

/// Whether a published site has a page showing the layout `layout_id` (index `site_pages_layout`).
pub(crate) async fn layout_on_published_site(db: &impl ConnectionTrait, layout_id: Uuid) -> Result<bool, ApiError> {
    Ok(db
        .query_one(statement(
            "SELECT 1 AS one FROM site_pages p JOIN sites s ON s.id = p.site_id \
             WHERE p.layout_id = $1 AND s.status = 'published' LIMIT 1",
            vec![layout_id.into()],
        ))
        .await?
        .is_some())
}

/// The published-site pages that show a layout, with their site, in a stable order.
pub(crate) async fn published_pages_with_layout(
    db: &impl ConnectionTrait,
    layout_id: Uuid,
) -> Result<Vec<PageRow>, ApiError> {
    Ok(PageRow::find_by_statement(statement(
        "SELECT p.id, p.site_id, p.route, p.layout_id, p.audience, p.nav, p.position, p.installation_id \
         FROM site_pages p JOIN sites s ON s.id = p.site_id \
         WHERE p.layout_id = $1 AND s.status = 'published' AND p.installation_id IS NULL \
         ORDER BY p.site_id, p.position, p.route",
        vec![layout_id.into()],
    ))
    .all(db)
    .await?)
}

/// The sites (any status) with a page showing a layout.
pub(crate) async fn sites_showing_layout(db: &impl ConnectionTrait, layout_id: Uuid) -> Result<Vec<Uuid>, ApiError> {
    #[derive(FromQueryResult)]
    struct Id {
        site_id: Uuid,
    }
    Ok(Id::find_by_statement(statement(
        "SELECT DISTINCT site_id FROM site_pages WHERE layout_id = $1 ORDER BY site_id",
        vec![layout_id.into()],
    ))
    .all(db)
    .await?
    .into_iter()
    .map(|row| row.site_id)
    .collect())
}

/// Whether another page of the site already has this route (`except` is the page being edited).
pub(crate) async fn route_taken(
    db: &impl ConnectionTrait,
    site_id: Uuid,
    route: &str,
    except: Option<Uuid>,
) -> Result<bool, ApiError> {
    Ok(db
        .query_one(statement(
            "SELECT 1 AS one FROM site_pages \
             WHERE site_id = $1 AND route = $2 AND generation IS NULL AND ($3::uuid IS NULL OR id <> $3)",
            vec![site_id.into(), route.into(), except.into()],
        ))
        .await?
        .is_some())
}

/// Whether a form belongs to a project.
pub(crate) async fn form_in_project(
    db: &impl ConnectionTrait,
    form_id: Uuid,
    project_id: Uuid,
) -> Result<bool, ApiError> {
    Ok(db
        .query_one(statement(
            "SELECT 1 AS one FROM project_forms WHERE id = $1 AND project_id = $2",
            vec![form_id.into(), project_id.into()],
        ))
        .await?
        .is_some())
}

/// The console slots a layout is placed in.
pub(crate) async fn layout_slots(db: &impl ConnectionTrait, layout_id: Uuid) -> Result<Vec<String>, ApiError> {
    #[derive(FromQueryResult)]
    struct Slot {
        slot_id: String,
    }
    Ok(Slot::find_by_statement(statement(
        "SELECT DISTINCT slot_id FROM layout_placements WHERE layout_id = $1 ORDER BY slot_id",
        vec![layout_id.into()],
    ))
    .all(db)
    .await?
    .into_iter()
    .map(|row| row.slot_id)
    .collect())
}

/// A binding with the schema and title template of its form, as the exposure lists read it.
#[derive(Debug, Clone, FromQueryResult)]
pub(crate) struct ExposedBinding {
    pub(crate) site_id: Uuid,
    pub(crate) binding_key: String,
    pub(crate) projection: Value,
    pub(crate) schema: Value,
    pub(crate) title_template: String,
}

const EXPOSED_BINDING_SQL: &str = "SELECT b.site_id, b.binding_key, b.projection, f.schema, f.title_template \
     FROM site_bindings b JOIN sites s ON s.id = b.site_id JOIN project_forms f ON f.id = b.form_id \
     WHERE f.project_id = s.project_id AND f.archived_at IS NULL";

/// The bindings of a site whose form is still in the site's project and not archived.
pub(crate) async fn exposed_bindings_of_site(
    db: &impl ConnectionTrait,
    site_id: Uuid,
) -> Result<Vec<ExposedBinding>, ApiError> {
    Ok(ExposedBinding::find_by_statement(statement(
        &format!("{EXPOSED_BINDING_SQL} AND b.site_id = $1 ORDER BY b.binding_key"),
        vec![site_id.into()],
    ))
    .all(db)
    .await?)
}

/// The bindings of a form; `published_only` keeps those of published sites.
pub(crate) async fn bindings_of_form(
    db: &impl ConnectionTrait,
    form_id: Uuid,
    published_only: bool,
) -> Result<Vec<ExposedBinding>, ApiError> {
    Ok(ExposedBinding::find_by_statement(statement(
        &format!(
            "{EXPOSED_BINDING_SQL} AND b.form_id = $1 AND (NOT $2 OR s.status = 'published') \
             ORDER BY b.site_id, b.binding_key"
        ),
        vec![form_id.into(), published_only.into()],
    ))
    .all(db)
    .await?)
}

/// A site-scope layout row: the layout a site page shows.
#[derive(Debug, Clone, FromQueryResult)]
pub(crate) struct LayoutRow {
    pub(crate) id: Uuid,
    pub(crate) workspace_id: Uuid,
    pub(crate) scope_kind: String,
    pub(crate) scope_id: Uuid,
    pub(crate) layout_key: String,
    pub(crate) version: i32,
    pub(crate) spec: Value,
    pub(crate) status: String,
    pub(crate) installation_id: Option<Uuid>,
}

pub(crate) async fn load_layout(db: &impl ConnectionTrait, id: Uuid) -> Result<Option<LayoutRow>, ApiError> {
    Ok(LayoutRow::find_by_statement(statement(
        "SELECT id, workspace_id, scope_kind, scope_id, layout_key, version, spec, status, installation_id \
         FROM layouts WHERE id = $1",
        vec![id.into()],
    ))
    .one(db)
    .await?)
}

/// Inserts a site-scope layout; `None` when the key is taken at that site.
pub(crate) async fn insert_site_layout(
    db: &impl ConnectionTrait,
    workspace_id: Uuid,
    site_id: Uuid,
    key: &str,
    spec: &Value,
    created_by: Option<Uuid>,
) -> Result<Option<Uuid>, ApiError> {
    #[derive(FromQueryResult)]
    struct Id {
        id: Uuid,
    }
    Ok(Id::find_by_statement(statement(
        "INSERT INTO layouts (workspace_id, scope_kind, scope_id, layout_key, spec, source, created_by) \
         VALUES ($1, 'site', $2, $3, $4, 'user', $5) ON CONFLICT DO NOTHING RETURNING id",
        vec![
            workspace_id.into(),
            site_id.into(),
            key.into(),
            spec.clone().into(),
            created_by.into(),
        ],
    ))
    .one(db)
    .await?
    .map(|row| row.id))
}

/// Replaces the spec of a layout (a site-scope layout, or the layout an approved draft edits in
/// place); `false` when the key is taken by another layout of its scope.
pub(crate) async fn update_site_layout(
    db: &impl ConnectionTrait,
    id: Uuid,
    key: &str,
    spec: &Value,
) -> Result<bool, ApiError> {
    let taken = db
        .query_one(statement(
            "SELECT 1 AS one FROM layouts l JOIN layouts me ON me.id = $1 \
             WHERE l.scope_kind = me.scope_kind AND l.scope_id = me.scope_id AND l.layout_key = $2 \
               AND l.id <> $1 AND l.generation IS NULL",
            vec![id.into(), key.into()],
        ))
        .await?
        .is_some();
    if taken {
        return Ok(false);
    }
    db.execute(statement(
        "UPDATE layouts SET spec = $2, layout_key = $3, version = version + 1, status = 'active', updated_at = now() \
         WHERE id = $1",
        vec![id.into(), spec.clone().into(), key.into()],
    ))
    .await?;
    Ok(true)
}

/// Deletes a site-scope layout once no page of its site shows it any more.
pub(crate) async fn delete_unused_site_layout(db: &impl ConnectionTrait, id: Uuid) -> Result<(), ApiError> {
    db.execute(statement(
        "DELETE FROM layouts l WHERE l.id = $1 AND l.scope_kind = 'site' \
         AND NOT EXISTS (SELECT 1 FROM site_pages p WHERE p.layout_id = l.id)",
        vec![id.into()],
    ))
    .await?;
    Ok(())
}

#[derive(Debug, Clone, FromQueryResult)]
pub(crate) struct BindingRow {
    pub(crate) site_id: Uuid,
    pub(crate) binding_key: String,
    pub(crate) form_id: Uuid,
    pub(crate) projection: Value,
}

pub(crate) async fn list_bindings(db: &impl ConnectionTrait, site_id: Uuid) -> Result<Vec<BindingRow>, ApiError> {
    Ok(BindingRow::find_by_statement(statement(
        "SELECT site_id, binding_key, form_id, projection FROM site_bindings WHERE site_id = $1 ORDER BY binding_key",
        vec![site_id.into()],
    ))
    .all(db)
    .await?)
}

pub(crate) async fn load_binding(
    db: &impl ConnectionTrait,
    site_id: Uuid,
    key: &str,
) -> Result<Option<BindingRow>, ApiError> {
    Ok(BindingRow::find_by_statement(statement(
        "SELECT site_id, binding_key, form_id, projection FROM site_bindings WHERE site_id = $1 AND binding_key = $2",
        vec![site_id.into(), key.into()],
    ))
    .one(db)
    .await?)
}

pub(crate) async fn upsert_binding(
    db: &impl ConnectionTrait,
    site_id: Uuid,
    key: &str,
    form_id: Uuid,
    projection: &Value,
) -> Result<Option<BindingRow>, ApiError> {
    Ok(BindingRow::find_by_statement(statement(
        "INSERT INTO site_bindings (site_id, binding_key, form_id, projection) VALUES ($1, $2, $3, $4) \
         ON CONFLICT (site_id, binding_key) DO UPDATE SET form_id = EXCLUDED.form_id, projection = EXCLUDED.projection \
         RETURNING site_id, binding_key, form_id, projection",
        vec![site_id.into(), key.into(), form_id.into(), projection.clone().into()],
    ))
    .one(db)
    .await?)
}

pub(crate) async fn delete_binding(db: &impl ConnectionTrait, site_id: Uuid, key: &str) -> Result<bool, ApiError> {
    let done = db
        .execute(statement(
            "DELETE FROM site_bindings WHERE site_id = $1 AND binding_key = $2",
            vec![site_id.into(), key.into()],
        ))
        .await?;
    Ok(done.rows_affected() > 0)
}

#[derive(Debug, Clone, FromQueryResult, Serialize)]
pub(crate) struct RoleRow {
    pub(crate) site_id: Uuid,
    pub(crate) user_id: Uuid,
    pub(crate) role: String,
}

pub(crate) async fn list_roles(db: &impl ConnectionTrait, site_id: Uuid) -> Result<Vec<RoleRow>, ApiError> {
    Ok(RoleRow::find_by_statement(statement(
        "SELECT site_id, user_id, role FROM site_roles WHERE site_id = $1 ORDER BY user_id",
        vec![site_id.into()],
    ))
    .all(db)
    .await?)
}

pub(crate) async fn load_role(
    db: &impl ConnectionTrait,
    site_id: Uuid,
    user_id: Uuid,
) -> Result<Option<RoleRow>, ApiError> {
    Ok(RoleRow::find_by_statement(statement(
        "SELECT site_id, user_id, role FROM site_roles WHERE site_id = $1 AND user_id = $2",
        vec![site_id.into(), user_id.into()],
    ))
    .one(db)
    .await?)
}

pub(crate) async fn upsert_role(
    db: &impl ConnectionTrait,
    site_id: Uuid,
    user_id: Uuid,
    role: &str,
) -> Result<Option<RoleRow>, ApiError> {
    Ok(RoleRow::find_by_statement(statement(
        "INSERT INTO site_roles (site_id, user_id, role) VALUES ($1, $2, $3) \
         ON CONFLICT (site_id, user_id) DO UPDATE SET role = EXCLUDED.role RETURNING site_id, user_id, role",
        vec![site_id.into(), user_id.into(), role.into()],
    ))
    .one(db)
    .await?)
}

/// Deletes a role and, in the same transaction (the caller's), that user's sessions of the site.
pub(crate) async fn delete_role(db: &impl ConnectionTrait, site_id: Uuid, user_id: Uuid) -> Result<bool, ApiError> {
    let done = db
        .execute(statement(
            "DELETE FROM site_roles WHERE site_id = $1 AND user_id = $2",
            vec![site_id.into(), user_id.into()],
        ))
        .await?;
    db.execute(statement(
        "DELETE FROM site_sessions WHERE site_id = $1 AND user_id = $2",
        vec![site_id.into(), user_id.into()],
    ))
    .await?;
    Ok(done.rows_affected() > 0)
}

/// Whether the user is an active member of the workspace; the workspace role when so.
pub(crate) async fn workspace_role(
    db: &impl ConnectionTrait,
    workspace_id: Uuid,
    user_id: Uuid,
) -> Result<Option<String>, ApiError> {
    #[derive(FromQueryResult)]
    struct Role {
        role: String,
    }
    Ok(Role::find_by_statement(statement(
        "SELECT m.role FROM workspace_members m JOIN users u ON u.id = m.user_id \
         WHERE m.workspace_id = $1 AND m.user_id = $2 AND u.is_active AND u.entity_type <> 'bot'",
        vec![workspace_id.into(), user_id.into()],
    ))
    .one(db)
    .await?
    .map(|row| row.role))
}

/// The visitor id of a visitor token at a site.
pub(crate) async fn find_visitor(
    db: &impl ConnectionTrait,
    site_id: Uuid,
    token_hash: &[u8],
) -> Result<Option<Uuid>, ApiError> {
    #[derive(FromQueryResult)]
    struct Id {
        id: Uuid,
    }
    Ok(Id::find_by_statement(statement(
        "UPDATE site_visitors SET last_seen = now() WHERE site_id = $1 AND token_hash = $2 RETURNING id",
        vec![site_id.into(), token_hash.to_vec().into()],
    ))
    .one(db)
    .await?
    .map(|row| row.id))
}

pub(crate) async fn insert_visitor(
    db: &impl ConnectionTrait,
    site_id: Uuid,
    token_hash: &[u8],
) -> Result<Uuid, ApiError> {
    #[derive(FromQueryResult)]
    struct Id {
        id: Uuid,
    }
    Id::find_by_statement(statement(
        "INSERT INTO site_visitors (site_id, token_hash) VALUES ($1, $2) RETURNING id",
        vec![site_id.into(), token_hash.to_vec().into()],
    ))
    .one(db)
    .await?
    .map(|row| row.id)
    .ok_or(ApiError::Internal)
}

/// An operator session with the facts its per-request re-check needs.
#[derive(Debug, Clone, FromQueryResult)]
pub(crate) struct SessionRow {
    pub(crate) user_id: Uuid,
    pub(crate) display_name: String,
    pub(crate) role: String,
    pub(crate) workspace_role: String,
}

/// The live operator session of a token at a site, re-checked on every request (contract C3
/// section 6.3): unexpired and used within the idle window, the user active with an unchanged
/// session version, still a member of the site's workspace and still holding a site role. A
/// session that fails any check is deleted. A live session's `last_seen` moves forward.
pub(crate) async fn live_session(
    db: &impl ConnectionTrait,
    site_id: Uuid,
    workspace_id: Uuid,
    token_hash: &[u8],
    idle_secs: i64,
) -> Result<Option<SessionRow>, ApiError> {
    let row = SessionRow::find_by_statement(statement(
        "SELECT s.user_id, u.name AS display_name, r.role, m.role AS workspace_role \
         FROM site_sessions s \
         JOIN users u ON u.id = s.user_id \
         JOIN site_roles r ON r.site_id = s.site_id AND r.user_id = s.user_id \
         JOIN workspace_members m ON m.workspace_id = $2 AND m.user_id = s.user_id \
         WHERE s.token_hash = $3 AND s.site_id = $1 \
           AND s.expires_at > now() AND s.last_seen > now() - make_interval(secs => $4) \
           AND u.is_active AND u.session_version = s.session_version",
        vec![
            site_id.into(),
            workspace_id.into(),
            token_hash.to_vec().into(),
            f64::from(i32::try_from(idle_secs).unwrap_or(i32::MAX)).into(),
        ],
    ))
    .one(db)
    .await?;
    if row.is_some() {
        db.execute(statement(
            "UPDATE site_sessions SET last_seen = now() WHERE token_hash = $1",
            vec![token_hash.to_vec().into()],
        ))
        .await?;
    } else {
        delete_session(db, token_hash).await?;
    }
    Ok(row)
}

pub(crate) async fn insert_session(
    db: &impl ConnectionTrait,
    token_hash: &[u8],
    site_id: Uuid,
    user_id: Uuid,
    session_version: i64,
    absolute_secs: i64,
) -> Result<(), ApiError> {
    db.execute(statement(
        "INSERT INTO site_sessions (token_hash, site_id, user_id, session_version, expires_at) \
         VALUES ($1, $2, $3, $4, now() + make_interval(secs => $5))",
        vec![
            token_hash.to_vec().into(),
            site_id.into(),
            user_id.into(),
            session_version.into(),
            f64::from(i32::try_from(absolute_secs).unwrap_or(i32::MAX)).into(),
        ],
    ))
    .await?;
    Ok(())
}

pub(crate) async fn delete_session(db: &impl ConnectionTrait, token_hash: &[u8]) -> Result<(), ApiError> {
    db.execute(statement(
        "DELETE FROM site_sessions WHERE token_hash = $1",
        vec![token_hash.to_vec().into()],
    ))
    .await?;
    Ok(())
}

/// Records a visitor submitted to one form of a site within the last hour.
pub(crate) async fn visitor_submissions_last_hour(
    db: &impl ConnectionTrait,
    form_id: Uuid,
    site_id: Uuid,
    visitor_id: Uuid,
) -> Result<i64, ApiError> {
    #[derive(FromQueryResult)]
    struct Count {
        n: i64,
    }
    Ok(Count::find_by_statement(statement(
        "SELECT count(*)::bigint AS n FROM form_records \
         WHERE form_id = $1 AND source->>'kind' = 'site' AND source->>'site_id' = $2 \
           AND source->>'site_visitor_id' = $3 AND created_at > now() - interval '1 hour'",
        vec![
            form_id.into(),
            site_id.to_string().into(),
            visitor_id.to_string().into(),
        ],
    ))
    .one(db)
    .await?
    .map_or(0, |row| row.n))
}

/// Removes a departed member's site roles and operator sessions at the sites of that workspace,
/// in the caller's transaction (contract C3 section 6.3, re-audit F2).
pub(crate) async fn remove_member(
    db: &impl ConnectionTrait,
    workspace_id: Uuid,
    user_id: Uuid,
) -> Result<(), ApiError> {
    for table in ["site_roles", "site_sessions"] {
        db.execute(statement(
            &format!(
                "DELETE FROM {table} WHERE user_id = $2 AND site_id IN (SELECT id FROM sites WHERE workspace_id = $1)"
            ),
            vec![workspace_id.into(), user_id.into()],
        ))
        .await?;
    }
    Ok(())
}
