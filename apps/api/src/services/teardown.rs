//! Removal of a workspace's or a project's content, in an order every foreign key accepts.
//!
//! Deleting a `workspaces` or `projects` row relies on `ON DELETE CASCADE` for most of what hangs
//! off it, but a cascade is **not** ordered. Three kinds of constraint make the order matter, and
//! each one failed in practice (a workspace holding a Flow page with an accepted collaboration
//! update answered `database error`, because `collab_updates.event_id` still referenced the
//! `business_events` row the cascade reached first):
//!
//! * a plain (`NO ACTION`) reference to a row that the same cascade also deletes, for example
//!   `collab_updates.event_id`, `event_dispatch.event_id`, `flow_import_jobs.audit_event_id`,
//!   `flow_scope_command_receipts.event_id`, `form_plugin_bindings` to its package;
//! * a `RESTRICT` reference between rows that all belong to the doomed content, for example the
//!   conversion jobs, previews and lineage rows that point at a source object;
//! * a plain reference to the workspace or project itself that has no cascade at all:
//!   `activities` and `notifications`.
//!
//! The statements below delete the referencing rows first, so the final `DELETE` finds nothing in
//! its way. No constraint is weakened: a `RESTRICT` that points at the doomed content **from
//! outside it** (another workspace's conversion lineage keeps its source object on purpose, see
//! `flow::scope_purge::retention_leaf`) still stops the delete, and [`refuse_blocked`] turns that
//! stop into a typed refusal instead of a database error.
use sea_orm::{ConnectionTrait, DatabaseTransaction, DbBackend, FromQueryResult, Statement};
use serde_json::json;
use uuid::Uuid;

use crate::error::ApiError;

/// `details.reason` of the refusal returned when content outside the deleted workspace or project
/// still references what would be deleted.
pub const DELETE_BLOCKED_REASON: &str = "retained_by_external_reference";

async fn run(tx: &DatabaseTransaction, sql: &str, values: Vec<sea_orm::Value>) -> Result<(), ApiError> {
    tx.execute(Statement::from_sql_and_values(DbBackend::Postgres, sql, values))
        .await?;
    Ok(())
}

#[derive(FromQueryResult)]
struct ScopeId {
    scope_id: Uuid,
}

/// Deletes everything that blocks the removal of `workspace_id`, then the workspace, then its
/// legacy Flow scope. The caller owns the transaction and commits it.
pub async fn delete_workspace_content(tx: &DatabaseTransaction, workspace_id: Uuid) -> Result<(), ApiError> {
    let scopes: Vec<Uuid> = ScopeId::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "SELECT scope_id FROM flow_workspace_scope_bindings WHERE workspace_id = $1",
        vec![workspace_id.into()],
    ))
    .all(tx)
    .await?
    .into_iter()
    .map(|row| row.scope_id)
    .collect();
    let ws = || vec![workspace_id.into()];

    // Connections first: `bot_runners.webhook_id` is a plain reference to a webhook that the
    // workspace cascade also deletes, so whether the cascade succeeds would depend on the order the
    // database fires its triggers in. The connection rows cascade from the runner rows.
    run(tx, "DELETE FROM bot_runners WHERE workspace_id = $1", ws()).await?;

    // Conversion provenance: jobs point at previews and lineage, all three at source objects.
    run(tx, "DELETE FROM flow_conversion_jobs WHERE workspace_id = $1", ws()).await?;
    run(tx, "DELETE FROM flow_conversion_previews WHERE workspace_id = $1", ws()).await?;
    run(tx, "DELETE FROM flow_object_lineage WHERE workspace_id = $1", ws()).await?;
    // Page assets are referenced by share aliases without a cascade.
    run(
        tx,
        "DELETE FROM flow_block_share_asset_aliases WHERE source_page_id IN \
         (SELECT id FROM flow_objects WHERE workspace_id = $1)",
        ws(),
    )
    .await?;
    // Flow content: objects take their documents, accepted updates, assets, shares, registry
    // placements and comments with them. This must precede the events the updates reference.
    run(tx, "DELETE FROM flow_objects WHERE workspace_id = $1", ws()).await?;
    delete_event_dependents(tx, "SELECT id FROM business_events WHERE workspace_id = $1", ws()).await?;
    run(tx, "DELETE FROM flow_import_previews WHERE workspace_id = $1", ws()).await?;
    run(tx, "DELETE FROM flow_import_jobs WHERE workspace_id = $1", ws()).await?;
    run(
        tx,
        "DELETE FROM flow_scope_command_receipts WHERE scope_id = ANY($1)",
        vec![scopes.clone().into()],
    )
    .await?;
    run(
        tx,
        "DELETE FROM form_plugin_bindings WHERE project_id IN (SELECT id FROM projects WHERE workspace_id = $1)",
        ws(),
    )
    .await?;
    run(tx, "DELETE FROM activities WHERE workspace_id = $1", ws()).await?;
    run(tx, "DELETE FROM notifications WHERE workspace_id = $1", ws()).await?;
    run(tx, "DELETE FROM workspaces WHERE id = $1", ws()).await?;
    // The legacy scope was bound to the workspace by a RESTRICT reference, so it can only go once
    // the binding has cascaded away. Left behind it would be an unreachable scope with its own
    // settings, revisions and receipts.
    run(
        tx,
        "DELETE FROM flow_scopes WHERE id = ANY($1) AND kind = 'legacy_managed'",
        vec![scopes.into()],
    )
    .await
}

/// Deletes everything that blocks the removal of `project_id`; the caller then deletes the project.
///
/// Returns the workspace and authorization epoch when a connected bot's seat left with the project,
/// for the caller to settle (`bot_connections::settle_epoch`) once its transaction commits.
pub async fn delete_project_content(
    tx: &DatabaseTransaction,
    project_id: Uuid,
) -> Result<Option<(Uuid, i64)>, ApiError> {
    // A connected bot's token, webhook and seat go with its last project, so a deleted project never
    // leaves a live credential or an orphaned managed webhook behind.
    let released = crate::services::bot_connections::release_for_project(tx, project_id).await?;
    let pr = || vec![project_id.into()];
    let in_project = "(SELECT id FROM flow_objects WHERE project_id = $1)";
    run(
        tx,
        &format!(
            "DELETE FROM flow_conversion_jobs WHERE source_object_id IN {in_project} \
             OR preview_id IN (SELECT id FROM flow_conversion_previews \
                               WHERE source_object_id IN {in_project} OR target_project_id = $1) \
             OR lineage_id IN (SELECT id FROM flow_object_lineage WHERE source_object_id IN {in_project})"
        ),
        pr(),
    )
    .await?;
    run(
        tx,
        &format!(
            "DELETE FROM flow_conversion_previews WHERE source_object_id IN {in_project} OR target_project_id = $1"
        ),
        pr(),
    )
    .await?;
    run(
        tx,
        &format!("DELETE FROM flow_object_lineage WHERE source_object_id IN {in_project}"),
        pr(),
    )
    .await?;
    run(
        tx,
        &format!("DELETE FROM flow_block_share_asset_aliases WHERE source_page_id IN {in_project}"),
        pr(),
    )
    .await?;
    run(tx, "DELETE FROM flow_objects WHERE project_id = $1", pr()).await?;
    delete_event_dependents(tx, "SELECT id FROM business_events WHERE project_id = $1", pr()).await?;
    run(
        tx,
        "DELETE FROM flow_import_previews WHERE committed_import_id IN \
         (SELECT j.id FROM flow_import_jobs j JOIN business_events e ON e.id = j.audit_event_id WHERE e.project_id = $1)",
        pr(),
    )
    .await?;
    run(
        tx,
        "DELETE FROM flow_import_jobs WHERE audit_event_id IN (SELECT id FROM business_events WHERE project_id = $1)",
        pr(),
    )
    .await?;
    run(
        tx,
        "DELETE FROM flow_scope_command_receipts WHERE event_id IN (SELECT id FROM business_events WHERE project_id = $1)",
        pr(),
    )
    .await?;
    run(tx, "DELETE FROM form_plugin_bindings WHERE project_id = $1", pr()).await?;
    run(tx, "DELETE FROM activities WHERE project_id = $1", pr()).await?;
    Ok(released)
}

/// Delivery bookkeeping that references `business_events` without a cascade. `events` is a
/// subquery selecting the ids of the events about to be deleted.
async fn delete_event_dependents(
    tx: &DatabaseTransaction,
    events: &str,
    values: Vec<sea_orm::Value>,
) -> Result<(), ApiError> {
    run(
        tx,
        &format!("DELETE FROM event_delivery_sources WHERE source_event_id IN ({events})"),
        values.clone(),
    )
    .await?;
    run(
        tx,
        &format!("DELETE FROM event_deliveries WHERE event_id IN ({events})"),
        values.clone(),
    )
    .await?;
    run(
        tx,
        &format!("DELETE FROM event_dispatch WHERE event_id IN ({events})"),
        values,
    )
    .await
}

/// Turns the database's refusal to delete into a typed one.
///
/// A foreign key violation (`23503`) or a `RESTRICT` violation (`23001`) at this point means
/// content outside the deleted workspace or project still references it, on purpose: retained
/// conversion lineage, for one. That is a stable, caller-visible condition, not a server fault, so
/// it answers `invalid_update` with a reason the caller can branch on, instead of the legacy
/// `database error`. Anything else stays what it was.
#[must_use]
pub fn refuse_blocked(err: ApiError, what: &str) -> ApiError {
    match err.database_sqlstate().as_deref() {
        Some("23503" | "23001") => {
            tracing::warn!(error = %err, what, "delete refused: retained by an external reference");
            ApiError::invalid_update_with_details(
                format!(
                    "this {what} cannot be deleted while content outside it still references it; \
                     remove or archive that content first"
                ),
                json!({ "reason": DELETE_BLOCKED_REASON }),
            )
        }
        _ => err,
    }
}
