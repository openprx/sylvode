//! What an approval writes besides the installation: the package's layouts and their placements.
//!
//! Rows carry `installation_id` and `generation` (migration 0135/0138) and are shown only while the
//! generation is the installation's active one. Forms a package creates are ordinary project forms
//! and carry no tag: they survive a disable or a revoke.

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
/// each in the slots it names.
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
) -> Result<(), ApiError> {
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
    }
    Ok(())
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

/// Deletes every layout and placement the installation created (all generations); a revoke.
///
/// # Errors
/// A database failure.
pub async fn delete_all(tx: &impl ConnectionTrait, installation_id: Uuid) -> Result<(), ApiError> {
    for table in ["layout_placements", "layouts"] {
        tx.execute(Statement::from_sql_and_values(
            DbBackend::Postgres,
            format!("DELETE FROM {table} WHERE installation_id = $1"),
            vec![installation_id.into()],
        ))
        .await?;
    }
    Ok(())
}
