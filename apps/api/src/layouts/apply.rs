//! The save path of an approved layout draft: the layout and its placement, in the caller's
//! transaction, with the approver as the author.

use sea_orm::DatabaseTransaction;
use serde_json::{Value, json};
use sv_layout::{Context, LayoutError, Options, SlotDescriptor, Validated};
use uuid::Uuid;

use super::{
    access::{Scope, ScopeKind},
    handlers::{invalid, validate_value},
    placements::{check_capacity, known_slot},
    store,
};
use crate::error::ApiError;

/// The slot a draft targets; it must take placements at the scope kind.
pub(crate) fn draft_slot(slot_id: &str, kind: ScopeKind) -> Result<SlotDescriptor, ApiError> {
    let slot = known_slot(slot_id)?;
    if !slot.scope_kinds.contains(&kind.as_str()) {
        return Err(ApiError::bad_request_coded(
            "slot_scope_not_allowed",
            "the slot does not accept placements at this scope kind",
        ));
    }
    Ok(slot)
}

/// Validates a spec for the slot it will sit in, as the console save path does.
pub(crate) fn validate_for_slot(spec: &Value, slot: &SlotDescriptor) -> Result<Validated, Vec<LayoutError>> {
    validate_value(
        spec,
        &Options {
            slot: Some(slot),
            context: Context::Console,
        },
    )
}

/// Stores the layout (`source` `draft`) and places it in the slot at the scope. A taken layout key
/// and a full slot refuse like the ordinary routes (`layout_key_taken`, `slot_occupied`,
/// `slot_full`); the caller's transaction then rolls back. Returns `{layout_id, placement_id}`.
pub(crate) async fn save_layout_draft(
    tx: &DatabaseTransaction,
    scope: &Scope,
    slot: &SlotDescriptor,
    spec: &Value,
    approver: Uuid,
) -> Result<Value, ApiError> {
    let validated = validate_for_slot(spec, slot).map_err(|errors| invalid("layout_invalid", &errors))?;
    let kind = scope.kind.as_str();
    store::lock_slot_scope(tx, slot.id, kind, scope.id).await?;
    let others = store::count_enabled(tx, slot.id, kind, scope.id, None).await?;
    check_capacity(slot, others)?;
    let layout = store::insert_layout(
        tx,
        scope.workspace_id,
        kind,
        scope.id,
        &validated.layout.key,
        spec,
        "draft",
        Some(approver),
    )
    .await?;
    let placement = store::insert_placement(
        tx,
        scope.workspace_id,
        slot.id,
        kind,
        scope.id,
        layout.id,
        0,
        None,
        Some(approver),
    )
    .await?;
    Ok(json!({ "layout_id": layout.id, "placement_id": placement.id }))
}
