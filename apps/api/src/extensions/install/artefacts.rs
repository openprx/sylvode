//! What an approval writes besides the installation: the package's layouts, their placements and
//! its site pages.
//!
//! Rows carry `installation_id` and `generation` (migration 0135/0137/0138) and are shown only while
//! the generation is the installation's active one. Forms a package creates are ordinary project
//! forms and carry no tag: they survive a disable or a revoke.

use std::collections::BTreeMap;

use sea_orm::{ConnectionTrait, DbBackend, FromQueryResult, Statement};
use serde_json::Value;
use sv_package::Manifest;
use uuid::Uuid;

use crate::error::ApiError;

/// The scope the placements of an installation use: the project, or the workspace.
#[derive(Clone, Copy, Debug)]
pub struct PlacementScope {
    pub workspace_id: Uuid,
    pub project_id: Option<Uuid>,
}

impl PlacementScope {
    const fn kind(self) -> &'static str {
        if self.project_id.is_some() {
            "project"
        } else {
            "workspace"
        }
    }

    fn id(self) -> Uuid {
        self.project_id.unwrap_or(self.workspace_id)
    }
}

#[derive(FromQueryResult)]
struct Id {
    id: Uuid,
}

#[derive(FromQueryResult)]
struct Count {
    total: i64,
}

/// Creates the layouts of `manifest` for generation `generation` of the installation and places
/// each in the slots it names. Returns the id of each created layout by layout key.
///
/// # Errors
/// `layout_key_taken` when a layout key is taken in the scope, `slot_occupied` when a slot is full,
/// `package_invalid` when a layout file is not JSON, or a database failure.
pub async fn create_layouts(
    tx: &impl ConnectionTrait,
    manifest: &Manifest,
    files: &std::collections::BTreeMap<String, Vec<u8>>,
    scope: PlacementScope,
    installation_id: Uuid,
    generation: i32,
    created_by: Uuid,
) -> Result<BTreeMap<String, Uuid>, ApiError> {
    let mut created = BTreeMap::new();
    for entry in &manifest.layouts {
        let spec: Value = files
            .get(&entry.path)
            .and_then(|bytes| serde_json::from_slice(bytes).ok())
            .ok_or_else(|| ApiError::bad_request_coded("package_invalid", "a layout file is not valid JSON"))?;
        let layout = Id::find_by_statement(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "INSERT INTO layouts (workspace_id, scope_kind, scope_id, layout_key, spec, source, installation_id, \
                                  generation, created_by) \
             VALUES ($1, $2, $3, $4, $5, 'package', $6, $7, $8) RETURNING id",
            vec![
                scope.workspace_id.into(),
                scope.kind().to_owned().into(),
                scope.id().into(),
                entry.key.clone().into(),
                spec.into(),
                installation_id.into(),
                generation.into(),
                created_by.into(),
            ],
        ))
        .one(tx)
        .await
        .map_err(|error| {
            if error.to_string().contains("layouts_key") {
                ApiError::conflict_coded(
                    "layout_key_taken",
                    "a layout with this key already exists in the scope",
                    None,
                )
            } else {
                ApiError::from(error)
            }
        })?
        .ok_or_else(ApiError::internal)?;
        for slot_id in &entry.slots {
            place(tx, scope, slot_id, layout.id, installation_id, generation, created_by).await?;
        }
        created.insert(entry.key.clone(), layout.id);
    }
    Ok(created)
}

/// Creates the site pages of `manifest` for generation `generation` in the project's site, which is
/// created as a `draft` when the project has none. A page serves nothing until the site is published
/// and a binding gives its form visitor access (contract C3 §6): the package never opens a form to
/// the public by itself.
///
/// # Errors
/// `pages_need_project` for a workspace scope, `page_route_taken` when the site already has a page
/// with a route of the package at this generation, or a database failure.
pub async fn create_pages(
    tx: &impl ConnectionTrait,
    manifest: &Manifest,
    layouts: &BTreeMap<String, Uuid>,
    scope: PlacementScope,
    installation_id: Uuid,
    generation: i32,
    created_by: Uuid,
) -> Result<(), ApiError> {
    if manifest.pages.is_empty() {
        return Ok(());
    }
    let project_id = scope
        .project_id
        .ok_or_else(|| ApiError::bad_request_coded("pages_need_project", "site pages are installed into a project"))?;
    let site_id = site_of_project(tx, scope.workspace_id, project_id, created_by).await?;
    for (position, page) in manifest.pages.iter().enumerate() {
        let layout_id = layouts.get(&page.layout).ok_or_else(ApiError::internal)?;
        let nav = page.nav.as_ref().and_then(|nav| serde_json::to_value(nav).ok());
        let position = i32::try_from(position).map_err(|_| ApiError::internal())?;
        tx.execute(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "INSERT INTO site_pages (site_id, route, layout_id, audience, nav, position, installation_id, generation) \
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8)",
            vec![
                site_id.into(),
                page.route.clone().into(),
                (*layout_id).into(),
                page.audience.clone().into(),
                nav.into(),
                position.into(),
                installation_id.into(),
                generation.into(),
            ],
        ))
        .await
        .map_err(|error| {
            if error.to_string().contains("site_pages_route") {
                ApiError::conflict_coded(
                    "page_route_taken",
                    "the site already has a page at a route of this package",
                    None,
                )
            } else {
                ApiError::from(error)
            }
        })?;
    }
    Ok(())
}

/// The project's oldest site, or a new `draft` one named after the project.
async fn site_of_project(
    tx: &impl ConnectionTrait,
    workspace_id: Uuid,
    project_id: Uuid,
    created_by: Uuid,
) -> Result<Uuid, ApiError> {
    let existing = Id::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "SELECT id FROM sites WHERE project_id = $1 ORDER BY created_at, id LIMIT 1",
        vec![project_id.into()],
    ))
    .one(tx)
    .await?;
    if let Some(site) = existing {
        return Ok(site.id);
    }
    // Slugs are global and the project key is not: a random suffix keeps the insert from colliding.
    let slug = format!("site-{}", &Uuid::new_v4().simple().to_string()[..12]);
    Id::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "INSERT INTO sites (workspace_id, project_id, slug, title, created_by) \
         SELECT $1, p.id, $3, jsonb_build_object('en', left(p.name, 80)), $4 FROM projects p WHERE p.id = $2 \
         RETURNING id",
        vec![workspace_id.into(), project_id.into(), slug.into(), created_by.into()],
    ))
    .one(tx)
    .await?
    .map(|row| row.id)
    .ok_or_else(|| ApiError::not_found("project not found"))
}

async fn place(
    tx: &impl ConnectionTrait,
    scope: PlacementScope,
    slot_id: &str,
    layout_id: Uuid,
    installation_id: Uuid,
    generation: i32,
    created_by: Uuid,
) -> Result<(), ApiError> {
    let slot = sv_layout::slot(slot_id)
        .filter(|slot| slot.scope_kinds.contains(&scope.kind()))
        .ok_or_else(|| {
            ApiError::bad_request_coded("slot_unknown", "the slot does not take placements at this scope")
        })?;
    // The same lock key the layout routes use, so a user placing at the same slot waits.
    tx.execute(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "SELECT pg_advisory_xact_lock(hashtextextended($1, 0))",
        vec![format!("layout_slot:{slot_id}:{}:{}", scope.kind(), scope.id()).into()],
    ))
    .await?;
    // Enabled placements that are shown: the user's own and those of an active generation.
    let taken = Count::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "SELECT COUNT(*)::bigint AS total FROM layout_placements p \
         LEFT JOIN installations i ON i.id = p.installation_id \
         WHERE p.slot_id = $1 AND p.scope_kind = $2 AND p.scope_id = $3 AND p.enabled \
           AND (p.installation_id IS NULL OR (i.status = 'active' AND p.generation = i.active_generation))",
        vec![
            slot_id.to_owned().into(),
            scope.kind().to_owned().into(),
            scope.id().into(),
        ],
    ))
    .one(tx)
    .await?
    .map_or(0, |row| row.total);
    if taken >= i64::from(slot.max_entries) {
        return Err(ApiError::conflict_coded(
            "slot_occupied",
            "the slot has no room for another placement at this scope",
            None,
        ));
    }
    tx.execute(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "INSERT INTO layout_placements (workspace_id, slot_id, scope_kind, scope_id, layout_id, position, \
                                        installation_id, generation, created_by) \
         VALUES ($1, $2, $3, $4, $5, 0, $6, $7, $8)",
        vec![
            scope.workspace_id.into(),
            slot_id.to_owned().into(),
            scope.kind().to_owned().into(),
            scope.id().into(),
            layout_id.into(),
            installation_id.into(),
            generation.into(),
            created_by.into(),
        ],
    ))
    .await?;
    Ok(())
}

/// Deletes every page, layout and placement the installation created (all generations); a revoke.
///
/// # Errors
/// A database failure.
pub async fn delete_all(tx: &impl ConnectionTrait, installation_id: Uuid) -> Result<(), ApiError> {
    for table in ["site_pages", "layout_placements", "layouts"] {
        tx.execute(Statement::from_sql_and_values(
            DbBackend::Postgres,
            format!("DELETE FROM {table} WHERE installation_id = $1"),
            vec![installation_id.into()],
        ))
        .await?;
    }
    Ok(())
}
