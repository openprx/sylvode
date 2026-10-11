//! Flow <-> Forms bridge domain rules frozen by ADR-0019.
//!
//! This module deliberately does not call the Forms direct-access helpers: those helpers retain
//! Forms' historical "no policy row means allow" default, while BR-4 requires the bridge to treat
//! that same state as read-only and visibly unconfigured. The bridge computes one intersection
//! decision and every reference, embed and conversion path consumes it.

#![allow(clippy::items_after_statements)]

use std::collections::{BTreeMap, BTreeSet};

use axum::http::Extensions;
use chrono::{DateTime, Utc};
use platform::app::AppState;
use sea_orm::{ConnectionTrait, DbBackend, FromQueryResult, Statement, TransactionTrait};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use uuid::Uuid;

use super::collab::authz::PermissionLevel;
use super::collab::limits::CONVERSION_STARTED_STALE_AFTER_SECS;
use super::event_origin::{CommandOrigin, EventSurface};
use super::{collab::authz, policy, repository};
use crate::error::{ApiError, ApiErrorKind, ServerDrainingReason};
use crate::events::{BusinessEventInput, FlowDispatchSpec, insert_flow_event};
use crate::flow::IN_FLIGHT_RETRY_AFTER_MS;
use crate::forms::permissions::{permission_policy_field_allows, permission_policy_record_scope};

pub const BRIDGE_ACTIONS: [&str; 6] = [
    "form.view",
    "record.create",
    "record.update",
    "record.delete",
    "record.export",
    "form.design",
];

#[cfg(test)]
fn bridge_test_mutation(name: &str) -> bool {
    std::env::var("OPENPR_FLOW_TEST_BRIDGE_MUTATION").is_ok_and(|value| value == name)
}

#[cfg(not(test))]
const fn bridge_test_mutation(_name: &str) -> bool {
    false
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PolicyConfiguration {
    Explicit,
    Unconfigured,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BridgeAccess {
    ReadOnly,
    Controlled,
}

/// Public permission shape. It says what the caller can do, never why another action was denied.
///
/// In particular it contains no target-existence bit, policy JSON, denied field names, record
/// owner identity or record-scope expression (ADR-0019 BR-3).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BridgePermissionState {
    pub access: BridgeAccess,
    pub configuration: PolicyConfiguration,
    pub actions: Vec<String>,
    pub field_read_limited: bool,
    pub field_write_limited: bool,
    pub record_limited: bool,
}

const fn denied_permission_state(configuration: PolicyConfiguration) -> BridgePermissionState {
    BridgePermissionState {
        access: BridgeAccess::ReadOnly,
        configuration,
        actions: Vec::new(),
        field_read_limited: false,
        field_write_limited: false,
        record_limited: false,
    }
}

fn bridge_policy_rejected(message: impl Into<String>, state: &BridgePermissionState) -> ApiError {
    ApiError::policy_rejected_with_details(message, json!({ "permission_state": state }))
}

/// `policy_rejected.details.reason` values approved by `error-mapping-v1.md` (2026-10-10).
pub const PREVIEW_EXPIRED_REASON: &str = "preview_expired";
pub const TARGET_SCHEMA_CHANGED_REASON: &str = "target_schema_changed";
pub const PERMISSION_UNCONFIGURED_REASON: &str = "permission_unconfigured";

/// `policy_rejected` with an approved `details.reason` and, when the caller already holds a
/// decision for the target, its current `permission_state`.
fn policy_rejected_reason(
    message: impl Into<String>,
    reason: &'static str,
    state: Option<&BridgePermissionState>,
) -> ApiError {
    let details = state.map_or_else(
        || json!({ "reason": reason }),
        |state| json!({ "reason": reason, "permission_state": state }),
    );
    ApiError::policy_rejected_with_details(message, details)
}

/// The refusal for a target form whose decision does not include `record.create`. A form with no
/// policy row is read-only by ADR-0019 BR-4 and says so with `permission_unconfigured`, so the
/// caller can tell "ask an admin to configure the form" from "you are not allowed".
fn record_create_rejected(state: &BridgePermissionState) -> ApiError {
    const MESSAGE: &str = "target record creation is not permitted";
    match state.configuration {
        PolicyConfiguration::Unconfigured => {
            policy_rejected_reason(MESSAGE, PERMISSION_UNCONFIGURED_REASON, Some(state))
        }
        PolicyConfiguration::Explicit => bridge_policy_rejected(MESSAGE, state),
    }
}

fn permission_state_after_flow_change(snapshot: &Value, level: PermissionLevel) -> BridgePermissionState {
    let mut state = serde_json::from_value::<BridgePermissionState>(snapshot.clone())
        .unwrap_or_else(|_| denied_permission_state(PolicyConfiguration::Unconfigured));
    let ceiling = flow_action_ceiling(level);
    state
        .actions
        .retain(|action| ceiling.get(action.as_str()).copied().unwrap_or(false));
    state.access = if state.actions.iter().any(|action| action != "form.view") {
        BridgeAccess::Controlled
    } else {
        BridgeAccess::ReadOnly
    };
    state
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BridgePermissionDecision {
    pub state: BridgePermissionState,
    denied_read_fields: BTreeSet<String>,
    denied_write_fields: BTreeSet<String>,
    record_scope: String,
}

impl BridgePermissionDecision {
    #[must_use]
    pub fn allows(&self, action: &str) -> bool {
        self.state.actions.iter().any(|allowed| allowed == action)
    }

    #[must_use]
    pub fn field_allows(&self, field_key: &str, action: &str) -> bool {
        match action {
            "read" => !self.denied_read_fields.contains(field_key),
            "write" => !self.denied_write_fields.contains(field_key),
            _ => false,
        }
    }

    #[must_use]
    pub fn record_in_scope(&self, actor_is_owner: bool) -> bool {
        self.record_scope != "owned" || actor_is_owner
    }
}

fn flow_action_ceiling(level: PermissionLevel) -> BTreeMap<&'static str, bool> {
    let mut actions = BTreeMap::new();
    for action in BRIDGE_ACTIONS {
        let allowed = match action {
            "form.view" => level >= PermissionLevel::View,
            "record.create" | "record.update" => level >= PermissionLevel::Edit,
            "record.delete" | "record.export" => level >= PermissionLevel::FullAccess,
            // BR-2: schema authority and unknown actions never cross the bridge.
            _ => false,
        };
        actions.insert(action, allowed);
    }
    actions
}

/// Computes ADR-0019's Flow-grade x explicit-Forms-policy intersection.
///
/// `None` is the non-enumerating answer: callers must omit the reference/embed entirely. It is
/// returned for Flow `Denied`, guests, an explicit Forms denial of `form.view`, or an out-of-scope
/// record. A missing policy row is intentionally *not* passed to Forms' permissive direct-access
/// default; BR-4 narrows it to `form.view` and marks it `unconfigured`.
#[must_use]
pub fn bridge_permission(
    flow_level: PermissionLevel,
    forms_policy: Option<&Value>,
    record_owner_matches: Option<bool>,
) -> Option<BridgePermissionDecision> {
    if flow_level == PermissionLevel::Denied {
        return None;
    }

    let ceiling = flow_action_ceiling(flow_level);
    let (configuration, policy_actions, denied_read_fields, denied_write_fields, record_scope) = forms_policy
        .map_or_else(
            || {
                let inherit_forms_default_allow = bridge_test_mutation("br4_default_allow");
                let actions = BTreeMap::from([
                    ("form.view", true),
                    ("record.create", inherit_forms_default_allow),
                    ("record.update", inherit_forms_default_allow),
                    ("record.delete", inherit_forms_default_allow),
                    ("record.export", inherit_forms_default_allow),
                    ("form.design", false),
                ]);
                (
                    PolicyConfiguration::Unconfigured,
                    actions,
                    BTreeSet::new(),
                    BTreeSet::new(),
                    "all".to_string(),
                )
            },
            |policy| {
                let actions = BRIDGE_ACTIONS
                    .into_iter()
                    .map(|action| {
                        let allowed = action != "form.design"
                            && policy
                                .get("actions")
                                .and_then(Value::as_object)
                                .and_then(|actions| actions.get(action))
                                .and_then(Value::as_bool)
                                .unwrap_or(true);
                        (action, allowed)
                    })
                    .collect::<BTreeMap<_, _>>();
                let field_keys = policy
                    .get("fields")
                    .and_then(Value::as_object)
                    .into_iter()
                    .flat_map(|fields| fields.keys())
                    .cloned()
                    .collect::<Vec<_>>();
                let denied_read = field_keys
                    .iter()
                    .filter(|key| !permission_policy_field_allows(policy, key, "read"))
                    .cloned()
                    .collect();
                let denied_write = field_keys
                    .iter()
                    .filter(|key| !permission_policy_field_allows(policy, key, "write"))
                    .cloned()
                    .collect();
                (
                    PolicyConfiguration::Explicit,
                    actions,
                    denied_read,
                    denied_write,
                    permission_policy_record_scope(policy),
                )
            },
        );

    if record_scope == "owned" && record_owner_matches == Some(false) {
        return None;
    }

    let actions = BRIDGE_ACTIONS
        .into_iter()
        .filter(|action| ceiling.get(action).copied().unwrap_or(false))
        .filter(|action| policy_actions.get(action).copied().unwrap_or(false))
        .map(str::to_string)
        .collect::<Vec<_>>();
    if !actions.iter().any(|action| action == "form.view") {
        return None;
    }
    let controlled = actions.iter().any(|action| action != "form.view");
    Some(BridgePermissionDecision {
        state: BridgePermissionState {
            access: if controlled {
                BridgeAccess::Controlled
            } else {
                BridgeAccess::ReadOnly
            },
            configuration,
            actions,
            field_read_limited: !denied_read_fields.is_empty(),
            field_write_limited: !denied_write_fields.is_empty(),
            record_limited: record_scope == "owned",
        },
        denied_read_fields,
        denied_write_fields,
        record_scope,
    })
}

#[derive(Debug, Deserialize)]
pub struct CreateReferenceInput {
    pub target_type: String,
    pub target_id: Uuid,
    #[serde(default)]
    pub display: Value,
    pub idempotency_key: String,
}

#[derive(Debug, Serialize)]
pub struct ReferenceReceipt {
    pub reference_id: Uuid,
    pub source_object_id: Uuid,
    pub target_type: String,
    pub target_id: Uuid,
    pub lineage_id: Option<Uuid>,
    pub permission_state: BridgePermissionState,
}

#[derive(Debug, Serialize)]
pub struct RemoveReferenceReceipt {
    pub removed: bool,
    pub event_id: Uuid,
}

#[derive(Debug, Serialize)]
#[serde(tag = "visibility", rename_all = "snake_case")]
pub enum BridgeReferenceView {
    Available {
        reference_id: Uuid,
        source_object_id: Uuid,
        target_type: String,
        target_id: Uuid,
        title: String,
        status: String,
        summary: Value,
        display: Value,
        permission_state: Box<BridgePermissionState>,
    },
    Unavailable,
}

#[derive(Debug, Serialize)]
pub struct ReferenceList {
    pub items: Vec<BridgeReferenceView>,
}

#[derive(Debug, FromQueryResult)]
struct ReferenceRow {
    id: Uuid,
    workspace_id: Uuid,
    source_object_id: Uuid,
    target_type: String,
    target_id: Uuid,
    display: Value,
    removed_at: Option<DateTime<Utc>>,
}

struct BridgeActor {
    id: Uuid,
    role: String,
    is_bot: bool,
    flow_level: PermissionLevel,
}

struct TargetView {
    form_id: Uuid,
    title: String,
    status: String,
    summary: Value,
    created_by: Option<Uuid>,
}

async fn bridge_enabled<C: ConnectionTrait>(conn: &C, workspace_id: Uuid) -> Result<bool, ApiError> {
    #[derive(FromQueryResult)]
    struct Row {
        bridge_enabled: bool,
    }
    Ok(Row::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "SELECT bridge_enabled FROM flow_workspace_settings WHERE workspace_id = $1 AND flow_enabled = true",
        vec![workspace_id.into()],
    ))
    .one(conn)
    .await?
    .is_some_and(|row| row.bridge_enabled))
}

async fn source_actor(
    state: &AppState,
    extensions: &Extensions,
    source_object_id: Uuid,
    minimum: PermissionLevel,
) -> Result<(repository::ObjectViewRow, BridgeActor), ApiError> {
    let source = repository::fetch_object_view(&state.db, source_object_id)
        .await?
        .ok_or_else(|| ApiError::not_found("flow object not found".to_string()))?;
    let (actor_id, role, is_bot) = policy::resolve_flow_principal(state, extensions, source.workspace_id)
        .await
        .map_err(|_| ApiError::not_found("flow object not found".to_string()))?;
    let principal_kind = if is_bot { "bot" } else { "user" };
    let flow_level = authz::effective_permission(
        &state.db,
        source.workspace_id,
        source.id,
        principal_kind,
        actor_id,
        &role,
    )
    .await
    .map_err(|_| ApiError::not_found("flow object not found".to_string()))?;
    if flow_level < minimum {
        return Err(ApiError::not_found("flow object not found".to_string()));
    }
    Ok((
        source,
        BridgeActor {
            id: actor_id,
            role,
            is_bot,
            flow_level,
        },
    ))
}

async fn target_view<C: ConnectionTrait>(
    conn: &C,
    workspace_id: Uuid,
    target_type: &str,
    target_id: Uuid,
) -> Result<Option<TargetView>, ApiError> {
    #[derive(FromQueryResult)]
    struct Row {
        form_id: Uuid,
        title: String,
        summary: Value,
        created_by: Option<Uuid>,
    }
    let row = match target_type {
        "form" => {
            Row::find_by_statement(Statement::from_sql_and_values(
                DbBackend::Postgres,
                "SELECT id AS form_id, name AS title, \
             jsonb_build_object('description', description) AS summary, created_by \
             FROM project_forms WHERE id = $1 AND workspace_id = $2 AND archived_at IS NULL",
                vec![target_id.into(), workspace_id.into()],
            ))
            .one(conn)
            .await?
        }
        "form_record" => {
            Row::find_by_statement(Statement::from_sql_and_values(
                DbBackend::Postgres,
                "SELECT r.form_id, r.title, r.values AS summary, r.created_by \
             FROM form_records r INNER JOIN project_forms f ON f.id = r.form_id \
             WHERE r.id = $1 AND r.workspace_id = $2 AND r.archived_at IS NULL AND f.archived_at IS NULL",
                vec![target_id.into(), workspace_id.into()],
            ))
            .one(conn)
            .await?
        }
        _ => {
            return Err(ApiError::invalid_update(
                "target_type must be form or form_record".to_string(),
            ));
        }
    };
    Ok(row.map(|row| TargetView {
        form_id: row.form_id,
        title: row.title,
        status: "active".to_string(),
        summary: row.summary,
        created_by: row.created_by,
    }))
}

async fn target_permission<C: ConnectionTrait>(
    conn: &C,
    actor: &BridgeActor,
    target: &TargetView,
) -> Result<Option<BridgePermissionDecision>, ApiError> {
    if actor.role == "__flow_guest" {
        return Ok(None);
    }
    #[derive(FromQueryResult)]
    struct Row {
        policy: Value,
    }
    let policy = Row::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "SELECT policy FROM form_permissions \
         WHERE form_id = $1 AND subject_type = 'role' AND subject_id = $2",
        vec![target.form_id.into(), actor.role.clone().into()],
    ))
    .one(conn)
    .await?;
    Ok(bridge_permission(
        actor.flow_level,
        policy.as_ref().map(|row| &row.policy),
        Some(target.created_by == Some(actor.id)),
    ))
}

async fn lock_and_reauthorize_source(
    tx: &sea_orm::DatabaseTransaction,
    source: &repository::ObjectViewRow,
    actor: &mut BridgeActor,
    permission_snapshot: Option<&Value>,
) -> Result<(), ApiError> {
    #[derive(FromQueryResult)]
    struct SettingsRow {
        bridge_enabled: bool,
    }
    let settings = SettingsRow::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "SELECT bridge_enabled FROM flow_workspace_settings \
         WHERE workspace_id = $1 AND flow_enabled = true FOR SHARE",
        vec![source.workspace_id.into()],
    ))
    .one(tx)
    .await?
    .ok_or_else(|| ApiError::policy_rejected("forms bridge is disabled"))?;
    if !settings.bridge_enabled {
        return Err(ApiError::policy_rejected("forms bridge is disabled"));
    }

    if !actor.is_bot {
        #[derive(FromQueryResult)]
        struct RoleRow {
            role: String,
        }
        actor.role = RoleRow::find_by_statement(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT role FROM workspace_members WHERE workspace_id = $1 AND user_id = $2",
            vec![source.workspace_id.into(), actor.id.into()],
        ))
        .one(tx)
        .await?
        .map_or_else(|| "__flow_guest".to_string(), |row| row.role);
    }
    actor.flow_level = authz::effective_permission(
        tx,
        source.workspace_id,
        source.id,
        if actor.is_bot { "bot" } else { "user" },
        actor.id,
        &actor.role,
    )
    .await?;
    if actor.flow_level < PermissionLevel::Edit {
        if let Some(snapshot) = permission_snapshot {
            return Err(bridge_policy_rejected(
                "bridge permission changed before commit",
                &permission_state_after_flow_change(snapshot, actor.flow_level),
            ));
        }
        return Err(ApiError::policy_rejected("bridge permission changed before commit"));
    }
    Ok(())
}

async fn lock_and_reauthorize_reference(
    tx: &sea_orm::DatabaseTransaction,
    source: &repository::ObjectViewRow,
    actor: &mut BridgeActor,
    target_type: &str,
    target_id: Uuid,
) -> Result<(TargetView, BridgePermissionDecision), ApiError> {
    lock_and_reauthorize_source(tx, source, actor, None).await?;

    let form_id = match target_type {
        "form" => target_id,
        "form_record" => {
            #[derive(FromQueryResult)]
            struct FormRow {
                form_id: Uuid,
            }
            FormRow::find_by_statement(Statement::from_sql_and_values(
                DbBackend::Postgres,
                "SELECT form_id FROM form_records WHERE id = $1 AND workspace_id = $2",
                vec![target_id.into(), source.workspace_id.into()],
            ))
            .one(tx)
            .await?
            .ok_or_else(|| ApiError::not_found("bridge target not found".to_string()))?
            .form_id
        }
        _ => {
            return Err(ApiError::invalid_update(
                "target_type must be form or form_record".to_string(),
            ));
        }
    };
    tx.execute(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "SELECT pg_advisory_xact_lock(hashtextextended($1, 19019))",
        vec![form_id.to_string().into()],
    ))
    .await?;
    let target = target_view(tx, source.workspace_id, target_type, target_id)
        .await?
        .ok_or_else(|| ApiError::not_found("bridge target not found".to_string()))?;
    let permission = target_permission(tx, actor, &target)
        .await?
        .ok_or_else(|| ApiError::not_found("bridge target not found".to_string()))?;
    Ok((target, permission))
}

fn read_only_state(mut state: BridgePermissionState) -> BridgePermissionState {
    state.actions.retain(|action| action == "form.view");
    state.access = BridgeAccess::ReadOnly;
    state
}

pub async fn create_reference(
    state: &AppState,
    extensions: &Extensions,
    source_object_id: Uuid,
    input: CreateReferenceInput,
    origin: CommandOrigin,
) -> Result<ReferenceReceipt, ApiError> {
    if input.idempotency_key.trim().is_empty() {
        return Err(ApiError::invalid_update("idempotency_key is required".to_string()));
    }
    if !input.display.is_object() {
        return Err(ApiError::invalid_update("display must be an object".to_string()));
    }
    let (source, mut actor) = source_actor(state, extensions, source_object_id, PermissionLevel::Edit).await?;
    if !bridge_enabled(&state.db, source.workspace_id).await? {
        return Err(ApiError::policy_rejected("forms bridge is disabled"));
    }
    let tx = state.db.begin().await?;
    let (_target, permission) =
        lock_and_reauthorize_reference(&tx, &source, &mut actor, &input.target_type, input.target_id).await?;
    #[derive(FromQueryResult)]
    struct IdRow {
        id: Uuid,
    }
    let proposed_id = Uuid::new_v4();
    let inserted = IdRow::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "INSERT INTO flow_bridge_references \
         (id, workspace_id, source_object_id, target_type, target_id, display, idempotency_key, created_by, created_by_kind) \
         VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9) \
         ON CONFLICT (workspace_id, idempotency_key) DO NOTHING RETURNING id",
        vec![
            proposed_id.into(),
            source.workspace_id.into(),
            source.id.into(),
            input.target_type.clone().into(),
            input.target_id.into(),
            input.display.clone().into(),
            input.idempotency_key.clone().into(),
            actor.id.into(),
            (if actor.is_bot { "bot" } else { "user" }).into(),
        ],
    ))
    .one(&tx)
    .await?;
    let (reference_id, was_new) = if let Some(row) = inserted {
        (row.id, true)
    } else {
        let existing = ReferenceRow::find_by_statement(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT id, workspace_id, source_object_id, target_type, target_id, display, removed_at \
             FROM flow_bridge_references WHERE workspace_id = $1 AND idempotency_key = $2",
            vec![source.workspace_id.into(), input.idempotency_key.clone().into()],
        ))
        .one(&tx)
        .await?
        .ok_or_else(ApiError::internal)?;
        let same_source = existing.source_object_id == source.id;
        let same_target = existing.target_type == input.target_type && existing.target_id == input.target_id;
        let same_display = existing.display == input.display;
        if !same_source || !same_target || !same_display || existing.removed_at.is_some() {
            return Err(ApiError::policy_rejected(
                "idempotency key was used for another reference",
            ));
        }
        (existing.id, false)
    };

    if was_new {
        insert_flow_event(
            &tx,
            BusinessEventInput {
                workspace_id: source.workspace_id,
                project_id: source.project_id,
                event_type: "flow.reference.created".to_string(),
                aggregate_type: "flow_reference".to_string(),
                aggregate_id: reference_id.to_string(),
                actor_id: (!actor.is_bot).then_some(actor.id),
                source: origin.source_json(),
                payload: json!({
                    "reference_id": reference_id,
                    "source_object_id": source.id,
                    "target_type": input.target_type,
                    "target_id": input.target_id,
                    "lineage_id": Value::Null,
                }),
                metadata: json!({}),
                correlation_id: Some(origin.correlation_id),
                causation_id: origin.causation_id,
                idempotency_key: Some(format!("bridge-reference:{}", input.idempotency_key)),
            },
            Some(FlowDispatchSpec {
                max_attempts: crate::config::runtime().flow.dispatch_max_attempts,
                document_id: None,
                accepted_seq: None,
            }),
        )
        .await?;
    }
    tx.commit().await?;

    Ok(ReferenceReceipt {
        reference_id,
        source_object_id: source.id,
        target_type: input.target_type,
        target_id: input.target_id,
        lineage_id: None,
        permission_state: permission.state,
    })
}

pub async fn list_references(
    state: &AppState,
    extensions: &Extensions,
    source_object_id: Uuid,
) -> Result<ReferenceList, ApiError> {
    let (source, actor) = source_actor(state, extensions, source_object_id, PermissionLevel::View).await?;
    let enabled = bridge_enabled(&state.db, source.workspace_id).await?;
    let rows = ReferenceRow::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "SELECT id, workspace_id, source_object_id, target_type, target_id, display, removed_at \
         FROM flow_bridge_references WHERE workspace_id = $1 AND source_object_id = $2 AND removed_at IS NULL \
         ORDER BY created_at, id",
        vec![source.workspace_id.into(), source.id.into()],
    ))
    .all(&state.db)
    .await?;
    let mut items = Vec::with_capacity(rows.len());
    for row in rows {
        let Some(target) = target_view(&state.db, row.workspace_id, &row.target_type, row.target_id).await? else {
            items.push(BridgeReferenceView::Unavailable);
            continue;
        };
        let Some(permission) = target_permission(&state.db, &actor, &target).await? else {
            continue;
        };
        let summary = if row.target_type == "form_record" {
            permission
                .denied_read_fields
                .iter()
                .fold(target.summary, |mut values, key| {
                    if let Some(object) = values.as_object_mut() {
                        object.remove(key);
                    }
                    values
                })
        } else {
            target.summary
        };
        let permission_state = if enabled {
            permission.state
        } else {
            read_only_state(permission.state)
        };
        items.push(BridgeReferenceView::Available {
            reference_id: row.id,
            source_object_id: row.source_object_id,
            target_type: row.target_type,
            target_id: row.target_id,
            title: target.title,
            status: target.status,
            summary,
            display: row.display,
            permission_state: Box::new(permission_state),
        });
    }
    Ok(ReferenceList { items })
}

pub async fn remove_reference(
    state: &AppState,
    extensions: &Extensions,
    source_object_id: Uuid,
    reference_id: Uuid,
    idempotency_key: String,
    origin: CommandOrigin,
) -> Result<RemoveReferenceReceipt, ApiError> {
    if idempotency_key.trim().is_empty() {
        return Err(ApiError::invalid_update("Idempotency-Key is required".to_string()));
    }
    let (source, actor) = source_actor(state, extensions, source_object_id, PermissionLevel::Edit).await?;
    let tx = state.db.begin().await?;
    let reference = ReferenceRow::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "SELECT id, workspace_id, source_object_id, target_type, target_id, display, removed_at \
         FROM flow_bridge_references WHERE id = $1 AND workspace_id = $2 AND source_object_id = $3 FOR UPDATE",
        vec![reference_id.into(), source.workspace_id.into(), source.id.into()],
    ))
    .one(&tx)
    .await?
    .ok_or_else(|| ApiError::not_found("reference not found".to_string()))?;
    tx.execute(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "UPDATE flow_bridge_references SET removed_at = COALESCE(removed_at, now()) WHERE id = $1",
        vec![reference_id.into()],
    ))
    .await?;
    let outcome = insert_flow_event(
        &tx,
        BusinessEventInput {
            workspace_id: source.workspace_id,
            project_id: source.project_id,
            event_type: "flow.reference.removed".to_string(),
            aggregate_type: "flow_reference".to_string(),
            aggregate_id: reference_id.to_string(),
            actor_id: (!actor.is_bot).then_some(actor.id),
            source: origin.source_json(),
            payload: json!({
                "reference_id": reference_id,
                "source_object_id": source.id,
                "target_type": reference.target_type,
            }),
            metadata: json!({}),
            correlation_id: Some(origin.correlation_id),
            causation_id: origin.causation_id,
            idempotency_key: Some(format!("bridge-unreference:{idempotency_key}")),
        },
        Some(FlowDispatchSpec {
            max_attempts: crate::config::runtime().flow.dispatch_max_attempts,
            document_id: None,
            accepted_seq: None,
        }),
    )
    .await?;
    tx.commit().await?;
    Ok(RemoveReferenceReceipt {
        removed: true,
        event_id: outcome.event_id,
    })
}

#[derive(Debug, Deserialize)]
pub struct ConversionPreviewInput {
    pub source_object_id: Uuid,
    pub source_frontier: String,
    pub target_type: String,
    pub mapping: Value,
    pub idempotency_key: String,
}

#[derive(Debug, Serialize)]
pub struct ConversionPreviewResponse {
    pub preview_id: Uuid,
    pub expires_at: DateTime<Utc>,
    pub source_frontier: String,
    pub target_schema_version: i32,
    pub mapping: Value,
    pub warnings: Vec<String>,
    pub permission_decision: BridgePermissionState,
    pub estimated_objects: i32,
}

#[derive(Debug, Deserialize)]
pub struct ConversionCommitInput {
    pub preview_id: Uuid,
    pub source_frontier: String,
    pub target_schema_version: i32,
    pub idempotency_key: String,
    pub confirm: bool,
}

#[derive(Debug, Deserialize)]
pub struct ConversionRetryInput {
    pub idempotency_key: String,
    pub confirm: bool,
}

#[derive(Debug, Serialize, FromQueryResult, Clone)]
pub struct ConversionJobResponse {
    pub job_id: Uuid,
    pub status: String,
    pub source_object_id: Uuid,
    /// `form` (the conversion creates a form) or `form_record` (it creates a record in one).
    pub target_type: String,
    pub source_frontier: String,
    pub target_schema_version: i32,
    pub lineage_id: Option<Uuid>,
    pub created_target_ids: Vec<Uuid>,
    pub warnings: Value,
    pub error: Option<String>,
    /// Present only on a `failed` job the stale-`started` sweep failed: `"stale_started"`
    /// ([`STALE_STARTED_REASON`]). It tells a reaped job (`error = "server_rejected"`, retryable)
    /// apart from a permanent refusal carrying the same code.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error_reason: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct ConversionOwnerResponse {
    pub source_object_id: Uuid,
    pub project_id: Option<Uuid>,
}

#[derive(Debug, FromQueryResult, Clone)]
struct PreviewRow {
    id: Uuid,
    workspace_id: Uuid,
    actor_id: Uuid,
    actor_kind: String,
    source_object_id: Uuid,
    source_frontier: String,
    target_type: String,
    target_project_id: Uuid,
    target_form_id: Option<Uuid>,
    target_schema_version: i32,
    mapping: Value,
    permission_snapshot: Value,
    expires_at: DateTime<Utc>,
}

fn mapping_uuid(mapping: &Value, key: &str) -> Result<Uuid, ApiError> {
    mapping
        .get(key)
        .and_then(Value::as_str)
        .ok_or_else(|| ApiError::invalid_update(format!("mapping.{key} is required")))
        .and_then(|value| {
            Uuid::parse_str(value).map_err(|_| ApiError::invalid_update(format!("mapping.{key} is invalid")))
        })
}

fn current_frontier(source: &repository::ObjectViewRow) -> String {
    use base64::Engine as _;
    base64::engine::general_purpose::STANDARD.encode(&source.document_frontier)
}

async fn project_exists<C: ConnectionTrait>(conn: &C, workspace_id: Uuid, project_id: Uuid) -> Result<bool, ApiError> {
    #[derive(FromQueryResult)]
    struct Row {
        present: bool,
    }
    Ok(Row::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "SELECT EXISTS(SELECT 1 FROM projects WHERE id = $1 AND workspace_id = $2) AS present",
        vec![project_id.into(), workspace_id.into()],
    ))
    .one(conn)
    .await?
    .is_some_and(|row| row.present))
}

async fn form_schema<C: ConnectionTrait>(
    conn: &C,
    workspace_id: Uuid,
    form_id: Uuid,
) -> Result<Option<(Uuid, i32, Value)>, ApiError> {
    #[derive(FromQueryResult)]
    struct Row {
        project_id: Uuid,
        schema_version: i32,
        schema: Value,
    }
    Ok(Row::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "SELECT project_id, schema_version, schema FROM project_forms \
         WHERE id = $1 AND workspace_id = $2 AND archived_at IS NULL",
        vec![form_id.into(), workspace_id.into()],
    ))
    .one(conn)
    .await?
    .map(|row| (row.project_id, row.schema_version, row.schema)))
}

/// Warnings for a record conversion's `mapping.values` against the target form schema, as stable
/// `<code>:<field_key>` strings: `required_target_value_missing` (the commit will be refused by
/// Forms validation), `target_value_missing` (the field is left empty: Forms fields have no
/// default) and `unknown_target_field` (the value names no field of the schema and is dropped).
/// Server-assigned `autonumber` and row-based `child_table` fields are never the mapping's job.
fn conversion_warnings(schema: &Value, mapping: &Value) -> Vec<String> {
    let Ok(fields) = crate::forms::schema::parse_fields(schema) else {
        return Vec::new();
    };
    let values = mapping.get("values").and_then(Value::as_object);
    let mut warnings = Vec::new();
    for field in &fields {
        if matches!(field.field_type.as_str(), "autonumber" | "child_table")
            || !crate::forms::values::is_missing(values.and_then(|values| values.get(&field.key)))
        {
            continue;
        }
        let code = if field.required {
            "required_target_value_missing"
        } else {
            "target_value_missing"
        };
        warnings.push(format!("{code}:{}", field.key));
    }
    if let Some(values) = values {
        let mut unknown = values
            .keys()
            .filter(|key| !fields.iter().any(|field| &field.key == *key))
            .collect::<Vec<_>>();
        unknown.sort();
        warnings.extend(unknown.into_iter().map(|key| format!("unknown_target_field:{key}")));
    }
    warnings
}

pub async fn preview_conversion(
    state: &AppState,
    extensions: &Extensions,
    input: ConversionPreviewInput,
) -> Result<ConversionPreviewResponse, ApiError> {
    if input.idempotency_key.trim().is_empty() || !input.mapping.is_object() {
        return Err(ApiError::invalid_update(
            "idempotency_key and object mapping are required".to_string(),
        ));
    }
    let (source, actor) = source_actor(state, extensions, input.source_object_id, PermissionLevel::Edit).await?;
    if actor.role == "__flow_guest" {
        return Err(ApiError::not_found("bridge target not found".to_string()));
    }
    if !bridge_enabled(&state.db, source.workspace_id).await? {
        return Err(ApiError::policy_rejected("forms bridge conversion is not permitted"));
    }
    let frontier = current_frontier(&source);
    if input.source_frontier != frontier {
        return Err(ApiError::stale_frontier(
            "source frontier changed",
            Some(source.document_seq),
            Some(&frontier),
        ));
    }

    let (target_project_id, target_form_id, target_schema_version, permission, warnings) =
        match input.target_type.as_str() {
            "form" => {
                let project_id = mapping_uuid(&input.mapping, "target_project_id")?;
                if !matches!(actor.role.as_str(), "owner" | "admin")
                    || !project_exists(&state.db, source.workspace_id, project_id).await?
                {
                    return Err(ApiError::policy_rejected("target form creation is not permitted"));
                }
                (
                    project_id,
                    None,
                    0,
                    BridgePermissionState {
                        access: BridgeAccess::Controlled,
                        configuration: PolicyConfiguration::Explicit,
                        actions: vec!["form.view".to_string(), "record.create".to_string()],
                        field_read_limited: false,
                        field_write_limited: false,
                        record_limited: false,
                    },
                    Vec::new(),
                )
            }
            "form_record" => {
                let form_id = mapping_uuid(&input.mapping, "target_form_id")?;
                let target = target_view(&state.db, source.workspace_id, "form", form_id)
                    .await?
                    .ok_or_else(|| ApiError::not_found("bridge target not found".to_string()))?;
                let permission = target_permission(&state.db, &actor, &target).await?;
                let permission = match permission {
                    Some(decision) if decision.allows("record.create") => decision,
                    Some(decision) => return Err(record_create_rejected(&decision.state)),
                    None => {
                        return Err(ApiError::not_found("bridge target not found".to_string()));
                    }
                };
                let (project_id, schema_version, schema) = form_schema(&state.db, source.workspace_id, form_id)
                    .await?
                    .ok_or_else(|| ApiError::not_found("bridge target not found".to_string()))?;
                let warnings = conversion_warnings(&schema, &input.mapping);
                (project_id, Some(form_id), schema_version, permission.state, warnings)
            }
            _ => {
                return Err(ApiError::invalid_update(
                    "target_type must be form or form_record".to_string(),
                ));
            }
        };

    if let Some(existing) = PreviewRow::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "SELECT id, workspace_id, actor_id, actor_kind, source_object_id, source_frontier, target_type, \
         target_project_id, target_form_id, target_schema_version, mapping, permission_snapshot, expires_at \
         FROM flow_conversion_previews WHERE workspace_id = $1 AND idempotency_key = $2",
        vec![source.workspace_id.into(), input.idempotency_key.clone().into()],
    ))
    .one(&state.db)
    .await?
    {
        if existing.actor_id != actor.id
            || existing.source_object_id != source.id
            || existing.source_frontier != frontier
            || existing.target_type != input.target_type
            || existing.target_project_id != target_project_id
            || existing.target_form_id != target_form_id
            || existing.target_schema_version != target_schema_version
            || existing.mapping != input.mapping
        {
            return Err(ApiError::invalid_update(
                "idempotency_key was already used with different conversion preview input".to_string(),
            ));
        }
        let permission_decision =
            serde_json::from_value(existing.permission_snapshot).map_err(|_| ApiError::internal())?;
        return Ok(ConversionPreviewResponse {
            preview_id: existing.id,
            expires_at: existing.expires_at,
            source_frontier: existing.source_frontier,
            target_schema_version: existing.target_schema_version,
            mapping: existing.mapping,
            warnings,
            permission_decision,
            estimated_objects: 1,
        });
    }

    let preview_id = Uuid::new_v4();
    let expires_at = Utc::now() + chrono::Duration::minutes(15);
    let permission_snapshot = serde_json::to_value(&permission).map_err(|_| ApiError::internal())?;
    state
        .db
        .execute(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "INSERT INTO flow_conversion_previews \
             (id, workspace_id, actor_id, actor_kind, source_object_id, source_frontier, target_type, \
              target_project_id, target_form_id, target_schema_version, mapping, permission_snapshot, \
              idempotency_key, expires_at) VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13,$14)",
            vec![
                preview_id.into(),
                source.workspace_id.into(),
                actor.id.into(),
                (if actor.is_bot { "bot" } else { "user" }).into(),
                source.id.into(),
                frontier.clone().into(),
                input.target_type.into(),
                target_project_id.into(),
                target_form_id.into(),
                target_schema_version.into(),
                input.mapping.clone().into(),
                permission_snapshot.into(),
                input.idempotency_key.into(),
                expires_at.into(),
            ],
        ))
        .await?;
    Ok(ConversionPreviewResponse {
        preview_id,
        expires_at,
        source_frontier: frontier,
        target_schema_version,
        mapping: input.mapping,
        warnings,
        permission_decision: permission,
        estimated_objects: 1,
    })
}

async fn load_preview<C: ConnectionTrait>(conn: &C, preview_id: Uuid) -> Result<PreviewRow, ApiError> {
    PreviewRow::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "SELECT id, workspace_id, actor_id, actor_kind, source_object_id, source_frontier, target_type, \
         target_project_id, target_form_id, target_schema_version, mapping, permission_snapshot, expires_at \
         FROM flow_conversion_previews WHERE id = $1",
        vec![preview_id.into()],
    ))
    .one(conn)
    .await?
    .ok_or_else(|| ApiError::not_found("conversion preview not found".to_string()))
}

async fn load_job<C: ConnectionTrait>(conn: &C, job_id: Uuid) -> Result<ConversionJobResponse, ApiError> {
    ConversionJobResponse::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "SELECT id AS job_id, status, source_object_id, target_type, source_frontier, target_schema_version, \
         lineage_id, created_target_ids, warnings, error_code AS error, error_reason FROM flow_conversion_jobs WHERE id = $1",
        vec![job_id.into()],
    ))
    .one(conn)
    .await?
    .ok_or_else(|| ApiError::not_found("conversion job not found".to_string()))
}

#[cfg(test)]
fn conversion_fault(preview_id: Uuid, point: &str) -> bool {
    let selected = CONVERSION_FAULT_FOR_TEST.lock().get(&preview_id).copied().unwrap_or(0);
    let selected_point = match selected {
        1 => point == "before_target_create",
        2 => point == "after_target_create_before_lineage",
        3 => point == "after_commit_before_response",
        // The run fails and recording that failure fails too.
        4 => matches!(point, "before_target_create" | "record_failure"),
        _ => false,
    };
    selected_point || std::env::var("OPENPR_FLOW_TEST_CONVERSION_FAULT").is_ok_and(|value| value == point)
}

/// Fault points armed per preview, so tests running in parallel never see each other's faults.
#[cfg(test)]
static CONVERSION_FAULT_FOR_TEST: std::sync::LazyLock<parking_lot::Mutex<std::collections::HashMap<Uuid, u8>>> =
    std::sync::LazyLock::new(|| parking_lot::Mutex::new(std::collections::HashMap::new()));

/// Arms fault `point` for runs of `preview_id`; `0` disarms it.
#[cfg(test)]
pub(crate) fn set_conversion_fault_for_test(preview_id: Uuid, point: u8) {
    let mut faults = CONVERSION_FAULT_FOR_TEST.lock();
    if point == 0 {
        faults.remove(&preview_id);
    } else {
        faults.insert(preview_id, point);
    }
}

#[cfg(not(test))]
const fn conversion_fault(_preview_id: Uuid, _point: &str) -> bool {
    false
}

async fn conversion_event(
    tx: &sea_orm::DatabaseTransaction,
    preview: &PreviewRow,
    job_id: Uuid,
    event_type: &str,
    origin: &CommandOrigin,
    idempotency_key: &str,
    causation_id: Option<Uuid>,
    payload: Value,
) -> Result<Uuid, ApiError> {
    Ok(insert_flow_event(
        tx,
        BusinessEventInput {
            workspace_id: preview.workspace_id,
            project_id: Some(preview.target_project_id),
            event_type: event_type.to_string(),
            aggregate_type: "flow_conversion".to_string(),
            aggregate_id: job_id.to_string(),
            actor_id: (preview.actor_kind == "user").then_some(preview.actor_id),
            source: origin.source_json(),
            payload,
            metadata: json!({}),
            correlation_id: Some(origin.correlation_id),
            causation_id,
            idempotency_key: Some(idempotency_key.to_string()),
        },
        Some(FlowDispatchSpec {
            max_attempts: crate::config::runtime().flow.dispatch_max_attempts,
            document_id: None,
            accepted_seq: None,
        }),
    )
    .await?
    .event_id)
}

/// One claimed conversion run: the preview supplies the mapping and the target; the job supplies
/// the frontier and target schema version it froze (copied from the preview at commit, read back
/// from the job on retry).
struct ConversionRun {
    preview: PreviewRow,
    job_id: Uuid,
    idempotency_key: String,
    origin: CommandOrigin,
    source_frontier: String,
    target_schema_version: i32,
    started_event: Uuid,
}

/// Maps a failed run to the typed error answered to the caller and the stable
/// `error-mapping-v1.md` code stored as the job's `error`. The internal classification (driver
/// error, legacy variant, fault point) is logged here and never reaches the response.
fn conversion_failure(job_id: Uuid, error: ApiError) -> (ApiError, &'static str) {
    tracing::warn!(%job_id, error = ?error, "flow conversion job failed");
    let typed = if error.is_known_transient_database_failure() {
        ApiError::server_draining(
            ServerDrainingReason::Contention,
            IN_FLIGHT_RETRY_AFTER_MS,
            "conversion hit a transient database failure",
        )
    } else {
        let typed = error.into_typed();
        if typed.kind() == ApiErrorKind::Unclassified {
            ApiError::internal()
        } else {
            typed
        }
    };
    let code = match typed.kind() {
        // Not one of the 14 stable codes: a bounded-retry read that may succeed later.
        ApiErrorKind::AuthorizationChurn => {
            ApiErrorKind::ServerDraining(ServerDrainingReason::Contention).stable_code()
        }
        kind => kind.stable_code(),
    };
    (typed, code)
}

fn started_payload(preview: &PreviewRow, job_id: Uuid) -> Value {
    json!({"job_id": job_id, "source_object_id": preview.source_object_id, "target_type": preview.target_type})
}

/// Persists a new `started` job for `preview` together with its `flow.conversion.started` event.
/// `None` means another commit already owns this preview (or key) and nothing was written.
async fn claim_commit_job(
    state: &AppState,
    preview: &PreviewRow,
    job_id: Uuid,
    idempotency_key: &str,
    origin: &CommandOrigin,
) -> Result<Option<Uuid>, ApiError> {
    let tx = state.db.begin().await?;
    let claimed = tx
        .execute(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "INSERT INTO flow_conversion_jobs \
             (id, workspace_id, preview_id, actor_id, actor_kind, source_object_id, source_frontier, target_type, \
              target_schema_version, status, idempotency_key, started_at) \
             VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,'started',$10,now()) \
             ON CONFLICT DO NOTHING",
            vec![
                job_id.into(),
                preview.workspace_id.into(),
                preview.id.into(),
                preview.actor_id.into(),
                preview.actor_kind.clone().into(),
                preview.source_object_id.into(),
                preview.source_frontier.clone().into(),
                preview.target_type.clone().into(),
                preview.target_schema_version.into(),
                idempotency_key.into(),
            ],
        ))
        .await?;
    if claimed.rows_affected() == 0 {
        tx.rollback().await?;
        return Ok(None);
    }
    let started_event = conversion_event(
        &tx,
        preview,
        job_id,
        "flow.conversion.started",
        origin,
        &format!("conversion-started:{idempotency_key}"),
        None,
        started_payload(preview, job_id),
    )
    .await?;
    tx.commit().await?;
    Ok(Some(started_event))
}

/// Moves a `failed` job back to `started` under the retry key, with its `flow.conversion.started`
/// event. The claim begins a new run, so it resets `started_at`. `None` means the job is no longer `failed` (a concurrent retry won) and nothing changed.
async fn claim_retry_job(
    state: &AppState,
    preview: &PreviewRow,
    job_id: Uuid,
    idempotency_key: &str,
    origin: &CommandOrigin,
) -> Result<Option<Uuid>, ApiError> {
    let tx = state.db.begin().await?;
    let claimed = tx
        .execute(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "UPDATE flow_conversion_jobs SET status='started', error_code=NULL, error_reason=NULL, idempotency_key=$2, \
             started_at=now(), updated_at=now() \
             WHERE id=$1 AND status='failed'",
            vec![job_id.into(), idempotency_key.into()],
        ))
        .await?;
    if claimed.rows_affected() == 0 {
        tx.rollback().await?;
        return Ok(None);
    }
    let started_event = conversion_event(
        &tx,
        preview,
        job_id,
        "flow.conversion.started",
        origin,
        &format!("conversion-started:{idempotency_key}"),
        None,
        started_payload(preview, job_id),
    )
    .await?;
    tx.commit().await?;
    Ok(Some(started_event))
}

/// Marks a `started` job `failed` with its stable error code and emits `flow.conversion.failed`
/// (a business transition caused by the run's `started` event). `false` means the job was no
/// longer `started` (the failure came after the run committed) and nothing was written.
async fn fail_conversion_job(state: &AppState, run: &ConversionRun, error_code: &str) -> Result<bool, ApiError> {
    if conversion_fault(run.preview.id, "record_failure") {
        return Err(ApiError::internal());
    }
    let tx = state.db.begin().await?;
    let failed = tx
        .execute(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "UPDATE flow_conversion_jobs SET status='failed', error_code=$2, error_reason=NULL, updated_at=now() \
             WHERE id=$1 AND status='started'",
            vec![run.job_id.into(), error_code.into()],
        ))
        .await?;
    if failed.rows_affected() == 1 {
        conversion_event(
            &tx,
            &run.preview,
            run.job_id,
            "flow.conversion.failed",
            &run.origin,
            &format!("conversion-failed:{}", run.idempotency_key),
            Some(run.started_event),
            json!({"job_id": run.job_id, "source_object_id": run.preview.source_object_id, "error": error_code}),
        )
        .await?;
    }
    tx.commit().await?;
    Ok(failed.rows_affected() == 1)
}

/// `flow.conversion.failed` payload `reason` for a job reaped by [`reap_stale_conversion_jobs`].
pub const STALE_STARTED_REASON: &str = "stale_started";

/// Reaps conversion jobs left `started` for longer than `older_than`.
///
/// The age of a run is measured from `started_at`, which only the job's creation and a retry
/// claim set; `updated_at` moves with every write to the row and is not consulted, so touching a
/// `started` job does not postpone its reclaim.
///
/// Such a job's run crashed between its claim and its outcome, or its failure could not be
/// recorded. Each becomes `failed` with
/// `error = "server_rejected"` and one business `flow.conversion.failed` event carrying
/// `reason = "stale_started"`, so it can be retried.
///
/// Idempotent: a reaped job is no longer `started`, so a second sweep finds nothing. Rows another
/// transaction holds (a run in progress) are skipped, not waited for. Returns how many jobs it
/// reaped.
pub async fn reap_stale_conversion_jobs(state: &AppState, older_than: chrono::Duration) -> Result<u64, ApiError> {
    #[derive(FromQueryResult)]
    struct Stale {
        id: Uuid,
        preview_id: Uuid,
        idempotency_key: String,
    }
    let error_code = ApiErrorKind::ServerRejected.stable_code();
    let tx = state.db.begin().await?;
    let abandoned = Stale::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "SELECT id, preview_id, idempotency_key FROM flow_conversion_jobs \
         WHERE status='started' AND started_at < now() - make_interval(secs => $1) \
         ORDER BY started_at, id FOR UPDATE SKIP LOCKED",
        vec![older_than.num_seconds().into()],
    ))
    .all(&tx)
    .await?;
    let origin = CommandOrigin::first_request_from(EventSurface::System);
    for job in &abandoned {
        let preview = load_preview(&tx, job.preview_id).await?;
        tx.execute(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "UPDATE flow_conversion_jobs SET status='failed', error_code=$2, error_reason=$3, updated_at=now() \
             WHERE id=$1",
            vec![job.id.into(), error_code.into(), STALE_STARTED_REASON.into()],
        ))
        .await?;
        conversion_event(
            &tx,
            &preview,
            job.id,
            "flow.conversion.failed",
            &origin,
            &format!("conversion-reaped:{}:{}", job.id, job.idempotency_key),
            None,
            json!({
                "job_id": job.id,
                "source_object_id": preview.source_object_id,
                "error": error_code,
                "reason": STALE_STARTED_REASON,
            }),
        )
        .await?;
        tracing::warn!(job_id = %job.id, "reaped a flow conversion job left started");
    }
    tx.commit().await?;
    Ok(u64::try_from(abandoned.len()).unwrap_or(u64::MAX))
}

/// Runs [`reap_stale_conversion_jobs`] at once and then every `interval`.
///
/// Stops when `stop` turns `true` or its sender is dropped (`rest-api-v1.md` v0.7: the sweep runs
/// at API startup and periodically; a retry never reaps in place). A failed sweep is logged and
/// the loop goes on.
pub async fn run_conversion_reaper(
    state: AppState,
    interval: std::time::Duration,
    mut stop: tokio::sync::watch::Receiver<bool>,
) {
    let mut ticker = tokio::time::interval(interval);
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    loop {
        tokio::select! {
            _ = ticker.tick() => {
                match reap_stale_conversion_jobs(&state, conversion_started_stale_after()).await {
                    Ok(0) => {}
                    Ok(reaped) => tracing::warn!(reaped, "reaped flow conversion jobs left started"),
                    Err(err) => tracing::warn!(error = %err, "stale flow conversion job sweep could not run"),
                }
            }
            changed = stop.changed() => {
                if changed.is_err() || *stop.borrow() {
                    return;
                }
            }
        }
    }
}

/// The stale-`started` threshold as a duration ([`CONVERSION_STARTED_STALE_AFTER_SECS`]).
#[must_use]
pub const fn conversion_started_stale_after() -> chrono::Duration {
    chrono::Duration::seconds(CONVERSION_STARTED_STALE_AFTER_SECS)
}

/// Runs a claimed job. Any failure after the claim leaves the job `failed` with a stable `error`
/// (`rest-api-v1.md` v0.7, 2026-10-10) and still answers the caller with the typed error, whose
/// `details` add `job_id` and the job's actual `job_status` so the caller can open and retry it.
async fn execute_claimed(
    state: &AppState,
    source: repository::ObjectViewRow,
    actor: BridgeActor,
    run: ConversionRun,
) -> Result<ConversionJobResponse, ApiError> {
    match run_conversion(state, source, actor, &run).await {
        Ok(job) => Ok(job),
        Err(error) => {
            let (typed, code) = conversion_failure(run.job_id, error);
            // `details.job_status` is the status the job row really has once this answer is sent.
            let job_status = match fail_conversion_job(state, &run, code).await {
                Ok(true) => Some("failed".to_string()),
                // The job had already left `started` (the run committed before failing, or another
                // request moved it on): report the status it really has.
                Ok(false) => match load_job(&state.db, run.job_id).await {
                    Ok(job) => Some(job.status),
                    Err(load_error) => {
                        tracing::error!(
                            job_id = %run.job_id,
                            error = ?load_error,
                            "flow conversion job status could not be read after a failed run"
                        );
                        None
                    }
                },
                // Nothing was written: the job is still `started`, and the periodic sweep fails it
                // once it is stale.
                Err(record_error) => {
                    tracing::error!(
                        job_id = %run.job_id,
                        error = ?record_error,
                        "flow conversion failure could not be recorded; the job stays started"
                    );
                    Some("started".to_string())
                }
            };
            let typed = typed.with_detail("job_id", json!(run.job_id));
            Err(match job_status {
                Some(status) => typed.with_detail("job_status", json!(status)),
                None => typed,
            })
        }
    }
}

async fn run_conversion(
    state: &AppState,
    source: repository::ObjectViewRow,
    mut actor: BridgeActor,
    run: &ConversionRun,
) -> Result<ConversionJobResponse, ApiError> {
    let preview = &run.preview;
    let job_id = run.job_id;
    let idempotency_key = run.idempotency_key.as_str();
    let origin = &run.origin;
    let started_event = run.started_event;
    let tx = state.db.begin().await?;
    if !bridge_enabled(&tx, preview.workspace_id).await? {
        return Err(bridge_policy_rejected(
            "forms bridge conversion is not permitted",
            &denied_permission_state(PolicyConfiguration::Unconfigured),
        ));
    }
    lock_and_reauthorize_source(&tx, &source, &mut actor, Some(&preview.permission_snapshot)).await?;
    let source = repository::fetch_object_view(&tx, source.id)
        .await?
        .ok_or_else(|| ApiError::not_found("flow object not found".to_string()))?;
    let frontier = current_frontier(&source);
    if frontier != run.source_frontier {
        return Err(ApiError::stale_frontier(
            "source frontier changed",
            Some(source.document_seq),
            Some(&frontier),
        ));
    }

    let (schema, permission) = if let Some(form_id) = preview.target_form_id {
        tx.execute(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT pg_advisory_xact_lock(hashtextextended($1, 19019))",
            vec![form_id.to_string().into()],
        ))
        .await?;
        let target = target_view(&tx, preview.workspace_id, "form", form_id)
            .await?
            .ok_or_else(|| ApiError::policy_rejected("target form changed before commit"))?;
        let permission = target_permission(&tx, &actor, &target).await?;
        let permission = match permission {
            Some(decision) if decision.allows("record.create") => decision,
            Some(decision) => return Err(record_create_rejected(&decision.state)),
            None => {
                return Err(bridge_policy_rejected(
                    "target record creation is not permitted",
                    &denied_permission_state(PolicyConfiguration::Explicit),
                ));
            }
        };
        let (_, version, schema) = form_schema(&tx, preview.workspace_id, form_id)
            .await?
            .ok_or_else(|| ApiError::policy_rejected("target form changed before commit"))?;
        if version != run.target_schema_version {
            return Err(policy_rejected_reason(
                "target schema version changed before commit",
                TARGET_SCHEMA_CHANGED_REASON,
                Some(&permission.state),
            ));
        }
        (Some(schema), Some(permission))
    } else {
        if !matches!(actor.role.as_str(), "owner" | "admin")
            || !project_exists(&tx, preview.workspace_id, preview.target_project_id).await?
        {
            return Err(bridge_policy_rejected(
                "target form creation is not permitted",
                &denied_permission_state(PolicyConfiguration::Explicit),
            ));
        }
        (None, None)
    };

    let warnings = schema
        .as_ref()
        .map(|schema| conversion_warnings(schema, &preview.mapping))
        .unwrap_or_default();
    if conversion_fault(preview.id, "before_target_create") {
        return Err(ApiError::internal());
    }

    let mut native_record = None;
    let (target_id, target_version) = if let Some(form_id) = preview.target_form_id {
        let values = preview.mapping.get("values").cloned().unwrap_or_else(|| json!({}));
        if let Some(decision) = permission.as_ref()
            && values
                .as_object()
                .is_some_and(|object| object.keys().any(|key| !decision.field_allows(key, "write")))
        {
            return Err(ApiError::policy_rejected(
                "mapped values include a field that is not writable",
            ));
        }
        let title = preview
            .mapping
            .get("title")
            .and_then(Value::as_str)
            .map(str::to_string)
            .or_else(|| Some(source.projection_title.clone()));
        let form = crate::routes::form::find_form_with_conn(&tx, form_id).await?;
        let creation = crate::forms::native_create::create_record_in_transaction(
            state,
            &tx,
            &form,
            actor.id,
            &actor.role,
            actor.is_bot,
            crate::forms::native_create::NativeRecordCreateRequest {
                record_id: Some(Uuid::new_v4()),
                values,
                title,
                source: json!({
                    "type":"flow_conversion",
                    "source_object_id":source.id,
                    "correlation_id":origin.correlation_id,
                }),
                idempotency_key: Some(format!("forms-conversion-target:{idempotency_key}")),
            },
        )
        .await?;
        let target_id = creation.record_id;
        native_record = Some((form, creation));
        (target_id, run.target_schema_version)
    } else {
        let target_id = Uuid::new_v4();
        let key = preview
            .mapping
            .get("key")
            .and_then(Value::as_str)
            .unwrap_or("converted_flow");
        let name = preview
            .mapping
            .get("name")
            .and_then(Value::as_str)
            .unwrap_or(&source.projection_title);
        let form = crate::forms::native_create::create_form_in_transaction(
            &tx,
            crate::forms::native_create::NativeFormCreateRequest {
                form_id: Some(target_id),
                workspace_id: preview.workspace_id,
                project_id: preview.target_project_id,
                key: key.to_string(),
                name: name.to_string(),
                description: String::new(),
                icon: None,
                color: None,
                title_template: None,
                schema: preview
                    .mapping
                    .get("schema")
                    .cloned()
                    .unwrap_or_else(|| json!({"version":"openpr.form.schema.v1","fields":[]})),
                detail_layout: json!({}),
                created_by: (!actor.is_bot).then_some(actor.id),
                source: json!({
                    "type":"flow_conversion",
                    "source_object_id":source.id,
                    "correlation_id":origin.correlation_id,
                }),
                change_summary: "created from Flow conversion".to_string(),
            },
        )
        .await?;
        (target_id, form.schema_version)
    };
    if conversion_fault(preview.id, "after_target_create_before_lineage") {
        return Err(ApiError::internal());
    }

    let lineage_id = Uuid::new_v4();
    tx.execute(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "INSERT INTO flow_object_lineage \
         (id, workspace_id, source_object_id, source_frontier, target_type, target_id, target_version, relation, created_by) \
         VALUES ($1,$2,$3,$4,$5,$6,$7,'derived_from',$8)",
        vec![lineage_id.into(), preview.workspace_id.into(), preview.source_object_id.into(), run.source_frontier.clone().into(),
            preview.target_type.clone().into(), target_id.into(), target_version.into(), actor.id.into()],
    )).await?;
    tx.execute(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "UPDATE flow_conversion_jobs SET status='completed', lineage_id=$2, created_target_ids=ARRAY[$3]::uuid[], \
         warnings=$4, error_code=NULL, error_reason=NULL, updated_at=now() WHERE id=$1",
        vec![
            job_id.into(),
            lineage_id.into(),
            target_id.into(),
            json!(warnings).into(),
        ],
    ))
    .await?;
    conversion_event(
        &tx,
        preview,
        job_id,
        "flow.conversion.completed",
        origin,
        &format!("conversion-completed:{idempotency_key}"),
        Some(started_event),
        json!({"job_id":job_id,"source_object_id":preview.source_object_id,"target_type":preview.target_type,
            "target_id":target_id,"lineage_id":lineage_id}),
    )
    .await?;
    tx.commit().await?;
    if let Some((form, creation)) = native_record.as_ref() {
        crate::forms::native_create::finish_record_creation(state, form, creation).await?;
    }
    if conversion_fault(preview.id, "after_commit_before_response") {
        return Err(ApiError::internal());
    }
    load_job(&state.db, job_id).await
}

/// The caller of a commit or retry: it must still read the source, and be the preview's actor or
/// a workspace owner/admin. Anyone else learns nothing about the job.
async fn conversion_caller(
    state: &AppState,
    extensions: &Extensions,
    preview: &PreviewRow,
) -> Result<(repository::ObjectViewRow, BridgeActor), ApiError> {
    let (source, actor) = source_actor(state, extensions, preview.source_object_id, PermissionLevel::View).await?;
    if actor.id != preview.actor_id && !matches!(actor.role.as_str(), "owner" | "admin") {
        return Err(ApiError::not_found("conversion job not found".to_string()));
    }
    Ok((source, actor))
}

/// The commit preconditions that need no lock and no job: the source has not moved past the
/// preview and the target form's schema version is still the preview's. A form that is gone is
/// left to the run, which records the failure on the job.
async fn precheck_commit(
    state: &AppState,
    source: &repository::ObjectViewRow,
    preview: &PreviewRow,
) -> Result<(), ApiError> {
    let frontier = current_frontier(source);
    if frontier != preview.source_frontier {
        return Err(ApiError::stale_frontier(
            "source frontier changed",
            Some(source.document_seq),
            Some(&frontier),
        ));
    }
    if let Some(form_id) = preview.target_form_id
        && let Some((_, version, _)) = form_schema(&state.db, preview.workspace_id, form_id).await?
        && version != preview.target_schema_version
    {
        return Err(policy_rejected_reason(
            "target schema version changed before commit",
            TARGET_SCHEMA_CHANGED_REASON,
            None,
        ));
    }
    Ok(())
}

/// The job an earlier commit already created for this request, if any.
///
/// A commit is idempotent on its key *and* on its preview (`rest-api-v1.md` v0.7: a second commit
/// of the same `preview_id` under another key returns the same job). A key already spent on a
/// different preview is body drift.
async fn existing_commit_job<C: ConnectionTrait>(
    conn: &C,
    preview: &PreviewRow,
    idempotency_key: &str,
) -> Result<Option<Uuid>, ApiError> {
    #[derive(FromQueryResult)]
    struct Existing {
        id: Uuid,
        preview_id: Uuid,
    }
    if let Some(existing) = Existing::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "SELECT id, preview_id FROM flow_conversion_jobs WHERE workspace_id=$1 AND idempotency_key=$2",
        vec![preview.workspace_id.into(), idempotency_key.into()],
    ))
    .one(conn)
    .await?
    {
        if existing.preview_id != preview.id {
            return Err(ApiError::idempotency_key_body_drift(
                "idempotency_key was already used with a different conversion preview",
            ));
        }
        return Ok(Some(existing.id));
    }
    Ok(Existing::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "SELECT id, preview_id FROM flow_conversion_jobs WHERE preview_id=$1",
        vec![preview.id.into()],
    ))
    .one(conn)
    .await?
    .map(|existing| existing.id))
}

pub async fn commit_conversion(
    state: &AppState,
    extensions: &Extensions,
    input: ConversionCommitInput,
    origin: CommandOrigin,
) -> Result<ConversionJobResponse, ApiError> {
    if !input.confirm || input.idempotency_key.trim().is_empty() {
        return Err(ApiError::invalid_update(
            "confirm=true and idempotency_key are required".to_string(),
        ));
    }
    let preview = load_preview(&state.db, input.preview_id).await?;
    let (source, actor) = conversion_caller(state, extensions, &preview).await?;
    if input.source_frontier != preview.source_frontier {
        return Err(ApiError::stale_frontier(
            "source frontier differs from preview",
            None,
            Some(&preview.source_frontier),
        ));
    }
    if input.target_schema_version != preview.target_schema_version {
        return Err(policy_rejected_reason(
            "target schema version differs from preview",
            TARGET_SCHEMA_CHANGED_REASON,
            None,
        ));
    }
    if let Some(job_id) = existing_commit_job(&state.db, &preview, &input.idempotency_key).await? {
        return load_job(&state.db, job_id).await;
    }
    if preview.expires_at <= Utc::now() {
        return Err(policy_rejected_reason(
            "conversion preview expired",
            PREVIEW_EXPIRED_REASON,
            None,
        ));
    }
    precheck_commit(state, &source, &preview).await?;

    let job_id = Uuid::new_v4();
    let Some(started_event) = claim_commit_job(state, &preview, job_id, &input.idempotency_key, &origin).await? else {
        // A concurrent commit of the same preview (or key) won `UNIQUE (preview_id)` /
        // `UNIQUE (workspace_id, idempotency_key)`; answer with its job instead of a 500.
        return match existing_commit_job(&state.db, &preview, &input.idempotency_key).await? {
            Some(existing) => load_job(&state.db, existing).await,
            None => Err(ApiError::internal()),
        };
    };
    let run = ConversionRun {
        source_frontier: preview.source_frontier.clone(),
        target_schema_version: preview.target_schema_version,
        preview,
        job_id,
        idempotency_key: input.idempotency_key,
        origin,
        started_event,
    };
    execute_claimed(state, source, actor, run).await
}

pub async fn conversion_status(
    state: &AppState,
    extensions: &Extensions,
    job_id: Uuid,
) -> Result<ConversionJobResponse, ApiError> {
    #[derive(FromQueryResult)]
    struct Scope {
        source_object_id: Uuid,
        actor_id: Uuid,
    }
    let scope = Scope::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "SELECT source_object_id, actor_id FROM flow_conversion_jobs WHERE id=$1",
        vec![job_id.into()],
    ))
    .one(&state.db)
    .await?
    .ok_or_else(|| ApiError::not_found("conversion job not found".to_string()))?;
    let (_, actor) = source_actor(state, extensions, scope.source_object_id, PermissionLevel::View).await?;
    if actor.id != scope.actor_id && !matches!(actor.role.as_str(), "owner" | "admin") {
        return Err(ApiError::not_found("conversion job not found".to_string()));
    }
    load_job(&state.db, job_id).await
}

pub async fn conversion_owner(
    state: &AppState,
    extensions: &Extensions,
    conversion_id: Uuid,
) -> Result<ConversionOwnerResponse, ApiError> {
    #[derive(FromQueryResult)]
    struct Scope {
        source_object_id: Uuid,
        actor_id: Uuid,
    }
    let scope = Scope::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "SELECT source_object_id, actor_id FROM flow_conversion_previews WHERE id=$1 \
         UNION ALL SELECT source_object_id, actor_id FROM flow_conversion_jobs WHERE id=$1 LIMIT 1",
        vec![conversion_id.into()],
    ))
    .one(&state.db)
    .await?
    .ok_or_else(|| ApiError::not_found("conversion not found".to_string()))?;
    let (source, actor) = source_actor(state, extensions, scope.source_object_id, PermissionLevel::View).await?;
    if actor.id != scope.actor_id && !matches!(actor.role.as_str(), "owner" | "admin") {
        return Err(ApiError::not_found("conversion not found".to_string()));
    }
    Ok(ConversionOwnerResponse {
        source_object_id: source.id,
        project_id: source.project_id,
    })
}

pub async fn retry_conversion(
    state: &AppState,
    extensions: &Extensions,
    job_id: Uuid,
    input: ConversionRetryInput,
    origin: CommandOrigin,
) -> Result<ConversionJobResponse, ApiError> {
    if !input.confirm || input.idempotency_key.trim().is_empty() {
        return Err(ApiError::invalid_update(
            "confirm=true and idempotency_key are required".to_string(),
        ));
    }
    #[derive(FromQueryResult)]
    struct Job {
        preview_id: Uuid,
        status: String,
        idempotency_key: String,
        source_frontier: String,
        target_schema_version: i32,
    }
    let job = Job::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "SELECT preview_id, status, idempotency_key, source_frontier, target_schema_version \
         FROM flow_conversion_jobs WHERE id=$1",
        vec![job_id.into()],
    ))
    .one(&state.db)
    .await?
    .ok_or_else(|| ApiError::not_found("conversion job not found".to_string()))?;
    let preview = load_preview(&state.db, job.preview_id).await?;
    let (source, actor) = conversion_caller(state, extensions, &preview).await?;
    if job.status == "completed" || job.idempotency_key == input.idempotency_key {
        return load_job(&state.db, job_id).await;
    }
    // A `started` job is still running, or is waiting for the periodic sweep to fail it once it
    // goes stale (`run_conversion_reaper`); either way the retry is a concurrent request on a job
    // in flight (`error-mapping-v1.md`: `server_draining{reason:"contention"}`), and it never reaps
    // the job itself (`rest-api-v1.md` v0.7).
    if job.status == "started" {
        return Err(ApiError::server_draining(
            ServerDrainingReason::Contention,
            IN_FLIGHT_RETRY_AFTER_MS,
            "the conversion job is still started",
        ));
    }
    if job.status != "failed" {
        return Err(ApiError::policy_rejected("only failed conversion jobs can be retried"));
    }
    #[derive(FromQueryResult)]
    struct KeyOwner {
        id: Uuid,
    }
    if KeyOwner::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "SELECT id FROM flow_conversion_jobs WHERE workspace_id=$1 AND idempotency_key=$2",
        vec![
            preview.workspace_id.into(),
            input.idempotency_key.clone().into(),
            job_id.into(),
        ],
    ))
    .one(&state.db)
    .await?
    .is_some_and(|owner| owner.id != job_id)
    {
        return Err(ApiError::idempotency_key_body_drift(
            "idempotency_key was already used for another conversion job",
        ));
    }
    // No preview TTL check: the job holds the frontier and target schema version it froze, and
    // the run rechecks both, plus every permission, against them (`rest-api-v1.md` v0.7 retry).
    let Some(started_event) = claim_retry_job(state, &preview, job_id, &input.idempotency_key, &origin).await? else {
        // A concurrent retry already moved the job on.
        return load_job(&state.db, job_id).await;
    };
    let run = ConversionRun {
        preview,
        job_id,
        idempotency_key: input.idempotency_key,
        origin,
        source_frontier: job.source_frontier,
        target_schema_version: job.target_schema_version,
        started_event,
    };
    execute_claimed(state, source, actor, run).await
}

/// One Flow reference that points at a form record, as the record detail reports it.
///
/// When the caller cannot read the source object only `reference_id` and
/// `source_available: false` are serialized: no object id, title, display or timestamp, so an
/// unreadable source leaves no identifying trace (the Relations panel rule for hidden targets).
#[derive(Debug, Serialize)]
pub struct RecordLineageReference {
    pub reference_id: Uuid,
    pub source_available: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source_object_id: Option<Uuid>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source_title: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub display: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub created_at: Option<DateTime<Utc>>,
}

/// One conversion job that created a form record, as the record detail reports it.
///
/// Same visibility rule as [`RecordLineageReference`]: an unreadable source yields only `job_id`
/// and `source_available: false`.
#[derive(Debug, Serialize)]
pub struct RecordLineageConversion {
    pub job_id: Uuid,
    pub source_available: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source_object_id: Option<Uuid>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source_title: Option<String>,
    /// Whether the caller may open the job itself (`GET /flow/conversions/{job_id}`): the job's
    /// actor or a workspace owner/admin. Withheld when the source is unreadable.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub job_available: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status: Option<String>,
    /// Outer `None` withholds the key (unreadable source); `Some(None)` serializes `null`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub lineage_id: Option<Option<Uuid>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub created_at: Option<DateTime<Utc>>,
}

/// The `flow_lineage` field of the form record detail: live Flow references to the record and
/// the conversion jobs that created it. Both lists are empty when Flow or the bridge is off.
#[derive(Debug, Default, Serialize)]
pub struct RecordFlowLineage {
    pub references: Vec<RecordLineageReference>,
    pub conversions: Vec<RecordLineageConversion>,
}

/// Lists the Flow-side lineage of one form record for the record detail response.
///
/// The caller has already passed the Forms `form.view` gate for the record; this only decides,
/// per source object, whether the caller may read that object in Flow. Sources below `view`
/// are reported without identifiers. Flow disabled or the bridge disabled yields empty lists.
pub async fn record_flow_lineage(
    state: &AppState,
    extensions: &Extensions,
    workspace_id: Uuid,
    record_id: Uuid,
) -> Result<RecordFlowLineage, ApiError> {
    #[derive(FromQueryResult)]
    struct ReferenceLineageRow {
        id: Uuid,
        source_object_id: Uuid,
        display: Value,
        created_at: DateTime<Utc>,
    }
    #[derive(FromQueryResult)]
    struct ConversionLineageRow {
        id: Uuid,
        source_object_id: Uuid,
        actor_id: Uuid,
        status: String,
        lineage_id: Option<Uuid>,
        created_at: DateTime<Utc>,
    }
    #[derive(FromQueryResult)]
    struct TitleRow {
        id: Uuid,
        title: Option<String>,
    }

    if !bridge_enabled(&state.db, workspace_id).await? {
        return Ok(RecordFlowLineage::default());
    }
    let references = ReferenceLineageRow::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "SELECT id, source_object_id, display, created_at FROM flow_bridge_references \
         WHERE workspace_id = $1 AND target_type = 'form_record' AND target_id = $2 AND removed_at IS NULL \
         ORDER BY created_at, id",
        vec![workspace_id.into(), record_id.into()],
    ))
    .all(&state.db)
    .await?;
    let conversions = ConversionLineageRow::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "SELECT id, source_object_id, actor_id, status, lineage_id, created_at FROM flow_conversion_jobs \
         WHERE workspace_id = $1 AND target_type = 'form_record' AND $2 = ANY(created_target_ids) \
         ORDER BY created_at, id",
        vec![workspace_id.into(), record_id.into()],
    ))
    .all(&state.db)
    .await?;
    if references.is_empty() && conversions.is_empty() {
        return Ok(RecordFlowLineage::default());
    }

    let source_ids: Vec<Uuid> = references
        .iter()
        .map(|row| row.source_object_id)
        .chain(conversions.iter().map(|row| row.source_object_id))
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect();
    let mut caller: Option<(Uuid, bool)> = None;
    let readable: BTreeSet<Uuid> = match policy::resolve_flow_principal(state, extensions, workspace_id).await {
        Ok((actor_id, role, is_bot)) => {
            // Same rule as `conversion_status`: the job's actor, or a workspace owner/admin.
            caller = Some((actor_id, matches!(role.as_str(), "owner" | "admin")));
            authz::effective_permissions(
                &state.db,
                workspace_id,
                &source_ids,
                if is_bot { "bot" } else { "user" },
                actor_id,
                &role,
            )
            .await?
            .into_iter()
            .filter(|(_, level)| *level >= PermissionLevel::View)
            .map(|(id, _)| id)
            .collect()
        }
        Err(error)
            if matches!(
                error.kind(),
                ApiErrorKind::Unauthenticated | ApiErrorKind::Forbidden | ApiErrorKind::NotFound
            ) =>
        {
            BTreeSet::new()
        }
        Err(error) => return Err(error),
    };
    let readable_ids: Vec<Uuid> = readable.iter().copied().collect();
    let titles: BTreeMap<Uuid, Option<String>> = if readable_ids.is_empty() {
        BTreeMap::new()
    } else {
        TitleRow::find_by_statement(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT fo.id, p.title FROM flow_objects fo \
             LEFT JOIN flow_object_projections p ON p.object_id = fo.id \
             WHERE fo.workspace_id = $1 AND fo.id = ANY($2)",
            vec![workspace_id.into(), readable_ids.into()],
        ))
        .all(&state.db)
        .await?
        .into_iter()
        .map(|row| (row.id, row.title))
        .collect()
    };

    Ok(RecordFlowLineage {
        references: references
            .into_iter()
            .map(|row| {
                if readable.contains(&row.source_object_id) {
                    RecordLineageReference {
                        reference_id: row.id,
                        source_available: true,
                        source_object_id: Some(row.source_object_id),
                        source_title: titles.get(&row.source_object_id).cloned().flatten(),
                        display: Some(row.display),
                        created_at: Some(row.created_at),
                    }
                } else {
                    RecordLineageReference {
                        reference_id: row.id,
                        source_available: false,
                        source_object_id: None,
                        source_title: None,
                        display: None,
                        created_at: None,
                    }
                }
            })
            .collect(),
        conversions: conversions
            .into_iter()
            .map(|row| {
                if readable.contains(&row.source_object_id) {
                    RecordLineageConversion {
                        job_id: row.id,
                        source_available: true,
                        source_object_id: Some(row.source_object_id),
                        source_title: titles.get(&row.source_object_id).cloned().flatten(),
                        job_available: Some(
                            caller.is_some_and(|(caller_id, is_admin)| is_admin || caller_id == row.actor_id),
                        ),
                        status: Some(row.status),
                        lineage_id: Some(row.lineage_id),
                        created_at: Some(row.created_at),
                    }
                } else {
                    RecordLineageConversion {
                        job_id: row.id,
                        source_available: false,
                        source_object_id: None,
                        source_title: None,
                        job_available: None,
                        status: None,
                        lineage_id: None,
                        created_at: None,
                    }
                }
            })
            .collect(),
    })
}

#[cfg(test)]
mod tests {
    use super::{BridgeAccess, PolicyConfiguration, bridge_permission};
    use crate::flow::collab::authz::PermissionLevel;
    use serde_json::json;

    #[test]
    fn bridge_permission_is_the_intersection_in_both_directions() {
        let forms_denies_update = json!({"actions": {"form.view": true, "record.update": false}});
        let full = bridge_permission(PermissionLevel::FullAccess, Some(&forms_denies_update), None)
            .expect("view remains visible");
        assert!(!full.allows("record.update"), "Forms denial must beat Flow FullAccess");

        let forms_allows_delete = json!({"actions": {"form.view": true, "record.delete": true}});
        let view =
            bridge_permission(PermissionLevel::View, Some(&forms_allows_delete), None).expect("view remains visible");
        assert!(
            !view.allows("record.delete"),
            "Flow View must beat a Forms delete allowance"
        );
    }

    #[test]
    fn bridge_never_grants_form_design() {
        let policy = json!({"actions": {"form.view": true, "form.design": true}});
        let decision =
            bridge_permission(PermissionLevel::FullAccess, Some(&policy), None).expect("view remains visible");
        assert!(!decision.allows("form.design"));
    }

    #[test]
    fn unconfigured_forms_are_read_only_and_honestly_labelled() {
        let decision = bridge_permission(PermissionLevel::FullAccess, None, None)
            .expect("the BR-4 read-only floor remains visible");
        assert_eq!(decision.state.configuration, PolicyConfiguration::Unconfigured);
        assert_eq!(decision.state.access, BridgeAccess::ReadOnly);
        assert_eq!(decision.state.actions, ["form.view"]);
        assert!(!decision.allows("record.update"));
    }

    #[test]
    fn flow_denied_principals_get_no_reference_shape() {
        let permissive = json!({"actions": {"form.view": true, "record.update": true}});
        assert!(bridge_permission(PermissionLevel::Denied, Some(&permissive), None).is_none());
    }

    #[test]
    fn explicit_field_and_record_restrictions_survive_the_intersection() {
        let policy = json!({
            "actions": {"form.view": true, "record.update": true},
            "fields": {"private": {"read": false, "write": false}},
            "record_scope": "owned"
        });
        assert!(
            bridge_permission(PermissionLevel::Edit, Some(&policy), Some(false)).is_none(),
            "an out-of-scope record must not produce a placeholder"
        );
        let decision =
            bridge_permission(PermissionLevel::Edit, Some(&policy), Some(true)).expect("the owned record is visible");
        assert!(!decision.field_allows("private", "read"));
        assert!(!decision.field_allows("private", "write"));
        assert!(decision.state.field_read_limited);
        assert!(decision.state.field_write_limited);
        assert!(decision.state.record_limited);
    }
}
