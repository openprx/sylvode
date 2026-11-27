//! SQL access to `layouts`, `layout_placements` and `layout_user_prefs`.
//!
//! Rows that belong to a package installation (`installation_id` set) are returned by the reads here
//! only while the installation is `active` and the row's `generation` is its active one (contract C3
//! section 3); a disabled or revoked installation, or a superseded generation, shows nothing. Only
//! the install flow changes such rows.

use platform::timestamp::Timestamp;
use sea_orm::{ConnectionTrait, DbBackend, FromQueryResult, Statement, Value as SqlValue};
use serde::Serialize;
use serde_json::Value;
use uuid::Uuid;

use crate::error::ApiError;

/// A layout row that is shown: the user's own, or a package's while its generation is the active one
/// of an active installation. Needs the layouts table aliased `l`.
pub(super) const VISIBLE_LAYOUT: &str = "(l.installation_id IS NULL OR EXISTS (SELECT 1 FROM installations i \
     WHERE i.id = l.installation_id AND i.status = 'active' AND i.active_generation = l.generation))";

pub(super) const LAYOUT_COLUMNS: &str = "id, workspace_id, scope_kind, scope_id, layout_key, version, spec, source, \
     installation_id, generation, status, updated_at";

#[derive(Debug, Clone, FromQueryResult)]
pub(super) struct LayoutRow {
    pub(super) id: Uuid,
    pub(super) workspace_id: Uuid,
    pub(super) scope_kind: String,
    pub(super) scope_id: Uuid,
    pub(super) layout_key: String,
    pub(super) version: i32,
    pub(super) spec: Value,
    pub(super) source: String,
    pub(super) installation_id: Option<Uuid>,
    pub(super) generation: Option<i32>,
    pub(super) status: String,
    pub(super) updated_at: Timestamp,
}

/// `LayoutSummary` of contract C3.
#[derive(Debug, Serialize)]
pub struct LayoutSummary {
    pub id: Uuid,
    pub scope_kind: String,
    pub scope_id: Uuid,
    pub layout_key: String,
    pub version: i32,
    pub source: String,
    pub status: String,
    pub installation_id: Option<Uuid>,
    pub generation: Option<i32>,
    pub updated_at: Timestamp,
}

impl From<&LayoutRow> for LayoutSummary {
    fn from(row: &LayoutRow) -> Self {
        Self {
            id: row.id,
            scope_kind: row.scope_kind.clone(),
            scope_id: row.scope_id,
            layout_key: row.layout_key.clone(),
            version: row.version,
            source: row.source.clone(),
            status: row.status.clone(),
            installation_id: row.installation_id,
            generation: row.generation,
            updated_at: row.updated_at,
        }
    }
}

pub(super) async fn load_layout(db: &impl ConnectionTrait, id: Uuid) -> Result<Option<LayoutRow>, ApiError> {
    Ok(LayoutRow::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        format!("SELECT {LAYOUT_COLUMNS} FROM layouts WHERE id = $1"),
        vec![id.into()],
    ))
    .one(db)
    .await?)
}

/// One page of the active and archived layouts stored at a scope, newest change first.
pub(super) async fn list_layouts(
    db: &impl ConnectionTrait,
    scope_kind: &str,
    scope_id: Uuid,
    limit: i64,
    offset: i64,
) -> Result<(Vec<LayoutRow>, i64), ApiError> {
    #[derive(FromQueryResult)]
    struct Count {
        total: i64,
    }
    let total = Count::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        format!(
            "SELECT COUNT(*)::bigint AS total FROM layouts l WHERE l.scope_kind = $1 AND l.scope_id = $2 AND {VISIBLE_LAYOUT}"
        ),
        vec![scope_kind.to_owned().into(), scope_id.into()],
    ))
    .one(db)
    .await?
    .map_or(0, |row| row.total);
    let rows = LayoutRow::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        format!(
            "SELECT {LAYOUT_COLUMNS} FROM layouts l WHERE l.scope_kind = $1 AND l.scope_id = $2 AND {VISIBLE_LAYOUT} \
             ORDER BY l.updated_at DESC, l.id LIMIT $3 OFFSET $4"
        ),
        vec![
            scope_kind.to_owned().into(),
            scope_id.into(),
            limit.into(),
            offset.into(),
        ],
    ))
    .all(db)
    .await?;
    Ok((rows, total))
}

/// Inserts a layout written by a person (`source` `user`) or applied from an approved draft
/// (`draft`); a taken key maps to `layout_key_taken`.
#[allow(clippy::too_many_arguments, reason = "one column per argument of the INSERT")]
pub(super) async fn insert_layout(
    db: &impl ConnectionTrait,
    workspace_id: Uuid,
    scope_kind: &str,
    scope_id: Uuid,
    key: &str,
    spec: &Value,
    source: &str,
    created_by: Option<Uuid>,
) -> Result<LayoutRow, ApiError> {
    LayoutRow::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        format!(
            "INSERT INTO layouts (workspace_id, scope_kind, scope_id, layout_key, spec, source, created_by) \
             VALUES ($1, $2, $3, $4, $5, $6, $7) RETURNING {LAYOUT_COLUMNS}"
        ),
        vec![
            workspace_id.into(),
            scope_kind.to_owned().into(),
            scope_id.into(),
            key.to_owned().into(),
            spec.clone().into(),
            source.to_owned().into(),
            created_by.into(),
        ],
    ))
    .one(db)
    .await
    .map_err(key_violation)?
    .ok_or_else(ApiError::internal)
}

/// Replaces the spec of a layout whose version is still `expected_version`; `None` when the
/// version moved on.
pub(super) async fn update_layout(
    db: &impl ConnectionTrait,
    id: Uuid,
    expected_version: i32,
    key: &str,
    spec: &Value,
) -> Result<Option<LayoutRow>, ApiError> {
    LayoutRow::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        format!(
            "UPDATE layouts SET layout_key = $3, spec = $4, version = version + 1, updated_at = now() \
             WHERE id = $1 AND version = $2 AND status = 'active' RETURNING {LAYOUT_COLUMNS}"
        ),
        vec![
            id.into(),
            expected_version.into(),
            key.to_owned().into(),
            spec.clone().into(),
        ],
    ))
    .one(db)
    .await
    .map_err(key_violation)
}

pub(super) async fn archive_layout(db: &impl ConnectionTrait, id: Uuid) -> Result<(), ApiError> {
    db.execute(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "UPDATE layouts SET status = 'archived', version = version + 1, updated_at = now() WHERE id = $1",
        vec![id.into()],
    ))
    .await?;
    Ok(())
}

fn key_violation(err: sea_orm::DbErr) -> ApiError {
    if err.to_string().contains("layouts_key") {
        ApiError::conflict_coded(
            "layout_key_taken",
            "a layout with this key already exists in the scope (an archived one still holds its key)",
            None,
        )
    } else {
        ApiError::from(err)
    }
}

/// The slots of the enabled placements that use a layout, for re-validation after a spec change.
pub(super) async fn slots_using_layout(db: &impl ConnectionTrait, layout_id: Uuid) -> Result<Vec<String>, ApiError> {
    #[derive(FromQueryResult)]
    struct Slot {
        slot_id: String,
    }
    Ok(Slot::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "SELECT DISTINCT slot_id FROM layout_placements WHERE layout_id = $1 AND enabled ORDER BY slot_id",
        vec![layout_id.into()],
    ))
    .all(db)
    .await?
    .into_iter()
    .map(|row| row.slot_id)
    .collect())
}

#[derive(Debug, Clone, FromQueryResult)]
pub(super) struct PlacementRow {
    pub(super) id: Uuid,
    pub(super) slot_id: String,
    pub(super) scope_kind: String,
    pub(super) scope_id: Uuid,
    pub(super) layout_id: Uuid,
    pub(super) position: i32,
    pub(super) enabled: bool,
    pub(super) condition: Option<String>,
    pub(super) installation_id: Option<Uuid>,
}

const PLACEMENT_COLUMNS: &str =
    "id, slot_id, scope_kind, scope_id, layout_id, position, enabled, condition, installation_id";

/// `Placement` of contract C3.
#[derive(Debug, Serialize)]
pub struct Placement {
    pub id: Uuid,
    pub slot_id: String,
    pub scope_kind: String,
    pub scope_id: Uuid,
    pub layout_id: Uuid,
    pub position: i32,
    pub enabled: bool,
    pub condition: Option<String>,
    pub installation_id: Option<Uuid>,
}

impl From<PlacementRow> for Placement {
    fn from(row: PlacementRow) -> Self {
        Self {
            id: row.id,
            slot_id: row.slot_id,
            scope_kind: row.scope_kind,
            scope_id: row.scope_id,
            layout_id: row.layout_id,
            position: row.position,
            enabled: row.enabled,
            condition: row.condition,
            installation_id: row.installation_id,
        }
    }
}

pub(super) async fn load_placement(db: &impl ConnectionTrait, id: Uuid) -> Result<Option<PlacementRow>, ApiError> {
    Ok(PlacementRow::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        format!("SELECT {PLACEMENT_COLUMNS} FROM layout_placements WHERE id = $1"),
        vec![id.into()],
    ))
    .one(db)
    .await?)
}

/// Serialises writers of one slot at one scope for the rest of the transaction.
pub(super) async fn lock_slot_scope(
    db: &impl ConnectionTrait,
    slot_id: &str,
    scope_kind: &str,
    scope_id: Uuid,
) -> Result<(), ApiError> {
    db.execute(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "SELECT pg_advisory_xact_lock(hashtextextended($1, 0))",
        vec![format!("layout_slot:{slot_id}:{scope_kind}:{scope_id}").into()],
    ))
    .await?;
    Ok(())
}

/// The enabled placements of a slot at a scope, not counting `except`.
pub(super) async fn count_enabled(
    db: &impl ConnectionTrait,
    slot_id: &str,
    scope_kind: &str,
    scope_id: Uuid,
    except: Option<Uuid>,
) -> Result<i64, ApiError> {
    #[derive(FromQueryResult)]
    struct Count {
        total: i64,
    }
    Ok(Count::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "SELECT COUNT(*)::bigint AS total FROM layout_placements \
         WHERE slot_id = $1 AND scope_kind = $2 AND scope_id = $3 AND enabled \
           AND ($4::uuid IS NULL OR id <> $4)",
        vec![
            slot_id.to_owned().into(),
            scope_kind.to_owned().into(),
            scope_id.into(),
            except.into(),
        ],
    ))
    .one(db)
    .await?
    .map_or(0, |row| row.total))
}

#[allow(clippy::too_many_arguments, reason = "one column per argument of the INSERT")]
pub(super) async fn insert_placement(
    db: &impl ConnectionTrait,
    workspace_id: Uuid,
    slot_id: &str,
    scope_kind: &str,
    scope_id: Uuid,
    layout_id: Uuid,
    position: i32,
    condition: Option<&str>,
    created_by: Option<Uuid>,
) -> Result<PlacementRow, ApiError> {
    PlacementRow::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        format!(
            "INSERT INTO layout_placements \
               (workspace_id, slot_id, scope_kind, scope_id, layout_id, position, condition, created_by) \
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8) RETURNING {PLACEMENT_COLUMNS}"
        ),
        vec![
            workspace_id.into(),
            slot_id.to_owned().into(),
            scope_kind.to_owned().into(),
            scope_id.into(),
            layout_id.into(),
            position.into(),
            condition.map(str::to_owned).into(),
            created_by.into(),
        ],
    ))
    .one(db)
    .await?
    .ok_or_else(ApiError::internal)
}

/// New values of a placement; `None` leaves a column alone, `condition: Some(None)` clears it.
#[allow(
    clippy::option_option,
    reason = "unchanged, cleared and set are three different updates"
)]
pub(super) struct PlacementChange {
    pub(super) position: Option<i32>,
    pub(super) enabled: Option<bool>,
    pub(super) condition: Option<Option<String>>,
}

pub(super) async fn update_placement(
    db: &impl ConnectionTrait,
    id: Uuid,
    change: &PlacementChange,
) -> Result<PlacementRow, ApiError> {
    let (set_condition, condition) = change
        .condition
        .as_ref()
        .map_or((false, None), |value| (true, value.clone()));
    PlacementRow::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        format!(
            "UPDATE layout_placements SET position = COALESCE($2, position), enabled = COALESCE($3, enabled), \
               condition = CASE WHEN $4 THEN $5 ELSE condition END \
             WHERE id = $1 RETURNING {PLACEMENT_COLUMNS}"
        ),
        vec![
            id.into(),
            change.position.into(),
            change.enabled.into(),
            set_condition.into(),
            condition.into(),
        ],
    ))
    .one(db)
    .await?
    .ok_or_else(|| ApiError::not_found("placement not found"))
}

pub(super) async fn delete_placement(db: &impl ConnectionTrait, id: Uuid) -> Result<(), ApiError> {
    db.execute(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "DELETE FROM layout_placements WHERE id = $1",
        vec![id.into()],
    ))
    .await?;
    Ok(())
}

/// One resolved entry of a slot, before the viewer's preferences are applied to the answer.
#[derive(Debug, FromQueryResult)]
pub(super) struct ResolvedRow {
    pub(super) placement_id: Uuid,
    pub(super) layout_id: Uuid,
    pub(super) layout_key: String,
    pub(super) spec: Value,
    pub(super) position: i32,
    pub(super) condition: Option<String>,
    pub(super) collapsed: bool,
    pub(super) installation_id: Option<Uuid>,
}

/// The enabled placements of `slot_id` that apply to `(workspace, project, form)`, outer scopes
/// first, then by the viewer's own position (else the placement's), then by creation; hidden ones
/// removed. Only rows of active layouts; a package's placement and layout show only while their
/// generation is the active one of an active installation (see the module comment).
pub(super) async fn resolve_slot(
    db: &impl ConnectionTrait,
    slot_id: &str,
    workspace_id: Uuid,
    project_id: Uuid,
    form_id: Option<Uuid>,
    viewer: Option<Uuid>,
) -> Result<Vec<ResolvedRow>, ApiError> {
    let form: SqlValue = form_id.into();
    Ok(ResolvedRow::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "SELECT p.id AS placement_id, p.layout_id, l.layout_key, l.spec, \
                COALESCE(u.position, p.position) AS position, p.condition, \
                COALESCE(u.collapsed, false) AS collapsed, p.installation_id \
         FROM layout_placements p \
         JOIN layouts l ON l.id = p.layout_id AND l.workspace_id = p.workspace_id AND l.status = 'active' \
         LEFT JOIN layout_user_prefs u ON u.placement_id = p.id AND u.user_id = $5 \
         LEFT JOIN installations pi ON pi.id = p.installation_id \
         LEFT JOIN installations li ON li.id = l.installation_id \
         WHERE p.slot_id = $1 AND p.workspace_id = $2 AND p.enabled \
           AND (p.installation_id IS NULL OR (pi.status = 'active' AND pi.active_generation = p.generation)) \
           AND (l.installation_id IS NULL OR (li.status = 'active' AND li.active_generation = l.generation)) \
           AND NOT COALESCE(u.hidden, false) \
           AND ((p.scope_kind = 'workspace' AND p.scope_id = $2) \
             OR (p.scope_kind = 'project' AND p.scope_id = $3) \
             OR (p.scope_kind = 'form' AND p.scope_id = $4)) \
         ORDER BY CASE p.scope_kind WHEN 'workspace' THEN 0 WHEN 'project' THEN 1 ELSE 2 END, \
                  COALESCE(u.position, p.position), p.created_at, p.id",
        vec![
            slot_id.to_owned().into(),
            workspace_id.into(),
            project_id.into(),
            form,
            viewer.into(),
        ],
    ))
    .all(db)
    .await?)
}

/// `LayoutUserPref` answer of the preference route.
#[derive(Debug, Serialize, FromQueryResult)]
pub struct Prefs {
    pub placement_id: Uuid,
    pub hidden: bool,
    pub collapsed: bool,
    pub position: Option<i32>,
}

/// New preference values; `position: Some(None)` clears the viewer's own order.
#[allow(
    clippy::option_option,
    reason = "unchanged, cleared and set are three different updates"
)]
pub(super) struct PrefsChange {
    pub(super) hidden: Option<bool>,
    pub(super) collapsed: Option<bool>,
    pub(super) position: Option<Option<i32>>,
}

pub(super) async fn upsert_prefs(
    db: &impl ConnectionTrait,
    user_id: Uuid,
    placement_id: Uuid,
    change: &PrefsChange,
) -> Result<Prefs, ApiError> {
    let (set_position, position) = change.position.map_or((false, None), |value| (true, value));
    Prefs::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "INSERT INTO layout_user_prefs (user_id, placement_id, hidden, collapsed, position) \
         VALUES ($1, $2, COALESCE($3, false), COALESCE($4, false), $6) \
         ON CONFLICT (user_id, placement_id) DO UPDATE SET \
           hidden = COALESCE($3, layout_user_prefs.hidden), \
           collapsed = COALESCE($4, layout_user_prefs.collapsed), \
           position = CASE WHEN $5 THEN $6 ELSE layout_user_prefs.position END \
         RETURNING placement_id, hidden, collapsed, position",
        vec![
            user_id.into(),
            placement_id.into(),
            change.hidden.into(),
            change.collapsed.into(),
            set_position.into(),
            position.into(),
        ],
    ))
    .one(db)
    .await?
    .ok_or_else(ApiError::internal)
}

/// The package keys of the installations that placed the entries, by installation id, for the
/// `installations` map of a slot answer.
pub(super) async fn installation_keys(
    db: &impl ConnectionTrait,
    ids: &[Uuid],
) -> Result<Vec<(String, Uuid)>, ApiError> {
    #[derive(FromQueryResult)]
    struct Key {
        id: Uuid,
        package_key: String,
    }
    if ids.is_empty() {
        return Ok(Vec::new());
    }
    Ok(Key::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "SELECT id, package_key FROM installations WHERE id = ANY($1) AND status = 'active'",
        vec![ids.to_vec().into()],
    ))
    .all(db)
    .await?
    .into_iter()
    .map(|row| (row.package_key, row.id))
    .collect())
}
