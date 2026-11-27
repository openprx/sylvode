// Public and framework-facing signatures remain stable during this behavior-neutral cleanup.
#![allow(clippy::needless_pass_by_value)]

use std::sync::Arc;

use platform::app::AppState;
use sea_orm::{ConnectionTrait, DbBackend, FromQueryResult, Statement, TransactionTrait};
use serde_json::{Value, json};
use uuid::Uuid;

use crate::{
    catalogue::{Refusal, RefusalCode},
    error::ApiError,
    events::{BusinessEventInput, insert_business_event},
    forms::{permissions::ensure_field_write_policy_allows, schema::parse_fields},
    plugins::{
        limits,
        manifest::{PluginHook, PluginRuntimePolicy, parse_manifest},
        redact,
        runtime::{PluginInvocationStatus, PluginRuntimeError, PluginRuntimeOutput, invoke_wasm_plugin},
    },
};

#[derive(Debug, FromQueryResult)]
struct PluginHookRow {
    id: Uuid,
    workspace_id: Uuid,
    project_id: Uuid,
    key: String,
    manifest: Value,
    wasm_bytes: Option<Vec<u8>>,
}

pub async fn run_field_validator_hooks(
    state: &AppState,
    workspace_id: Uuid,
    project_id: Uuid,
    form_id: Uuid,
    form_key: &str,
    schema: &Value,
    values: &Value,
) -> Result<(), ApiError> {
    let Some(values_object) = values.as_object() else {
        return Ok(());
    };
    let fields = parse_fields(schema).map_err(ApiError::BadRequest)?;
    let plugins = load_active_plugins(state, workspace_id, project_id).await?;

    for plugin in plugins.into_iter().map(Arc::new) {
        let manifest = parse_manifest(&plugin.manifest).map_err(ApiError::BadRequest)?;
        let matching_hooks = manifest
            .capabilities
            .hooks
            .iter()
            .filter(|hook| hook.kind == "field_validator")
            .filter(|hook| hook_matches_form(hook, form_key))
            .collect::<Vec<_>>();
        if matching_hooks.is_empty() {
            continue;
        }
        let Some(wasm_bytes) = plugin.wasm_bytes.clone() else {
            continue;
        };

        for field in &fields {
            let Some(field_value) = values_object.get(&field.key) else {
                continue;
            };
            for _hook in matching_hooks
                .iter()
                .copied()
                .filter(|hook| hook_matches_field(hook, &field.key))
            {
                let input = json!({
                    "hook_kind": "field_validator",
                    "plugin_key": plugin.key,
                    "payload": {
                        "workspace_id": workspace_id,
                        "project_id": project_id,
                        "form_id": form_id,
                        "form_key": form_key,
                        "field_key": field.key,
                        "field_type": field.field_type,
                        "value": field_value,
                        "values": values,
                    }
                });
                let run = run_and_record_hook(
                    state,
                    &plugin,
                    "field_validator",
                    wasm_bytes.clone(),
                    input,
                    manifest.capabilities.runtime.clone(),
                    validator_rejection_message,
                )
                .await?;
                // The guest's own text never reaches the caller.
                match run {
                    Ok((_, None)) => {}
                    Ok((_, Some(_))) | Err(_) => {
                        return Err(Refusal::new(RefusalCode::LegacyPluginValidationFailed, None).into());
                    }
                }
            }
        }
    }

    Ok(())
}

/// Runs the `formula` hooks of the project over `values`. A patch is applied only when the acting
/// `role` may write each key under the form's field write policy and, when the hook declares a
/// `field_key`, the patch touches that key alone; otherwise the write is refused
/// `legacy_plugin_patch_denied`. Every write path passes its requester's role.
pub async fn run_formula_hooks(
    state: &AppState,
    workspace_id: Uuid,
    project_id: Uuid,
    form_id: Uuid,
    form_key: &str,
    role: &str,
    values: Value,
) -> Result<Value, ApiError> {
    let mut values = values;
    if !values.is_object() {
        return Ok(values);
    }
    let plugins = load_active_plugins(state, workspace_id, project_id).await?;

    for plugin in plugins.into_iter().map(Arc::new) {
        let manifest = parse_manifest(&plugin.manifest).map_err(ApiError::BadRequest)?;
        let matching_hooks = manifest
            .capabilities
            .hooks
            .iter()
            .filter(|hook| hook.kind == "formula")
            .filter(|hook| hook_matches_form(hook, form_key))
            .collect::<Vec<_>>();
        if matching_hooks.is_empty() {
            continue;
        }
        let Some(wasm_bytes) = plugin.wasm_bytes.clone() else {
            continue;
        };

        for hook in matching_hooks {
            let input = json!({
                "hook_kind": "formula",
                "plugin_key": plugin.key,
                "payload": {
                    "workspace_id": workspace_id,
                    "project_id": project_id,
                    "form_id": form_id,
                    "form_key": form_key,
                    "field_key": hook.field_key,
                    "values": values,
                }
            });
            let run = run_and_record_hook(
                state,
                &plugin,
                "formula",
                wasm_bytes.clone(),
                input,
                manifest.capabilities.runtime.clone(),
                no_rejection,
            )
            .await?;
            match run {
                Ok((output, _)) => {
                    let patch = formula_patch_from_output(&output.output, hook.field_key.as_deref())?;
                    ensure_patch_allowed(state, form_id, role, hook.field_key.as_deref(), &patch).await?;
                    merge_patch(&mut values, patch)?;
                }
                Err(_) => {
                    return Err(ApiError::BadRequest("formula plugin failed".to_string()));
                }
            }
        }
    }

    Ok(values)
}

pub async fn run_event_handler_hooks(
    state: &AppState,
    workspace_id: Uuid,
    project_id: Uuid,
    form_id: Uuid,
    form_key: &str,
    record_id: Option<Uuid>,
    event_type: &str,
    payload: Value,
) -> Result<(), ApiError> {
    // Same contract as `invoke_plugin`: the lookup, the guest, and the invocation row run on a
    // task scheduled before this future awaits, so cancelling the request that triggered the
    // hook cannot skip the row.
    let state = state.clone();
    let form_key = form_key.to_string();
    let event_type = event_type.to_string();
    tokio::spawn(async move {
        run_event_handler_hooks_detached(
            state,
            workspace_id,
            project_id,
            form_id,
            form_key,
            record_id,
            event_type,
            payload,
        )
        .await
    })
    .await
    .map_err(|err| {
        tracing::error!(error = %err, "plugin event-handler task failed");
        ApiError::Internal
    })?
}

async fn run_event_handler_hooks_detached(
    state: AppState,
    workspace_id: Uuid,
    project_id: Uuid,
    form_id: Uuid,
    form_key: String,
    record_id: Option<Uuid>,
    event_type: String,
    payload: Value,
) -> Result<(), ApiError> {
    let plugins = load_active_plugins(&state, workspace_id, project_id).await?;

    for plugin in plugins.into_iter().map(Arc::new) {
        let manifest = parse_manifest(&plugin.manifest).map_err(ApiError::BadRequest)?;
        let matching_hooks = manifest
            .capabilities
            .hooks
            .iter()
            .filter(|hook| hook.kind == "event_handler")
            .filter(|hook| hook_matches_form(hook, &form_key))
            .filter(|hook| hook.event_type.as_deref().is_none_or(|value| value == event_type))
            .collect::<Vec<_>>();
        if matching_hooks.is_empty() {
            continue;
        }
        let Some(wasm_bytes) = plugin.wasm_bytes.clone() else {
            continue;
        };

        for _hook in matching_hooks {
            let input = json!({
                "hook_kind": "event_handler",
                "plugin_key": plugin.key,
                "payload": {
                    "workspace_id": workspace_id,
                    "project_id": project_id,
                    "form_id": form_id,
                    "form_key": form_key,
                    "record_id": record_id,
                    "event_type": event_type,
                    "event_payload": payload,
                }
            });
            // An event handler's outcome is recorded and does not affect the caller.
            let _recorded: HookRun = run_and_record_hook(
                &state,
                &plugin,
                "event_handler",
                wasm_bytes.clone(),
                input,
                manifest.capabilities.runtime.clone(),
                no_rejection,
            )
            .await?;
        }
    }

    Ok(())
}

/// A hook run as its record describes it: the guest's output and, for a validator, its rejection.
type HookRun = Result<(PluginRuntimeOutput, Option<String>), PluginRuntimeError>;

/// Runs one hook and records it in `plugin_invocations` (with its `plugin.invoked` event).
///
/// Both happen on a task of their own, so the record is written even when the request whose write
/// triggered the hook is cancelled; the guest is then bounded by its own deadline rather than by
/// the request. `rejection` reads a validator's refusal from the output, which records the run as
/// `failed` with that message.
async fn run_and_record_hook(
    state: &AppState,
    plugin: &Arc<PluginHookRow>,
    hook_kind: &'static str,
    wasm_bytes: Vec<u8>,
    input: Value,
    runtime: PluginRuntimePolicy,
    rejection: fn(&Value) -> Option<String>,
) -> Result<HookRun, ApiError> {
    let state = state.clone();
    let plugin = Arc::clone(plugin);
    let permit = limits::try_acquire(plugin.id)?;
    tokio::spawn(async move {
        let _permit = permit;
        match invoke_wasm_plugin(wasm_bytes, input.clone(), limits::clamp(runtime)).await {
            Ok(output) => {
                let refused = rejection(&output.output);
                insert_hook_invocation(&state, &plugin, hook_kind, input, output.clone(), refused.clone()).await?;
                Ok(Ok((output, refused)))
            }
            Err(error) => {
                insert_failed_hook_invocation(&state, &plugin, hook_kind, input, &error).await?;
                Ok(Err(error))
            }
        }
    })
    .await
    .map_err(|err| {
        tracing::error!(hook_kind, error = %err, "plugin hook task failed");
        ApiError::Internal
    })?
}

/// For hooks whose output never refuses anything.
const fn no_rejection(_: &Value) -> Option<String> {
    None
}

fn hook_matches_form(hook: &PluginHook, form_key: &str) -> bool {
    hook.form_key.as_deref().is_none_or(|value| value == form_key)
}

fn hook_matches_field(hook: &PluginHook, field_key: &str) -> bool {
    hook.field_key.as_deref().is_none_or(|value| value == field_key)
}

fn validator_rejection_message(output: &Value) -> Option<String> {
    if output.get("ok").and_then(Value::as_bool) != Some(false) {
        return None;
    }
    Some(
        output
            .get("error")
            .or_else(|| output.get("message"))
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .unwrap_or("validator returned ok=false")
            .to_string(),
    )
}

fn patch_denied() -> ApiError {
    Refusal::new(RefusalCode::LegacyPluginPatchDenied, None).into()
}

/// A patch is an object, and when the hook declares a `field_key` it holds that key alone.
fn patch_within_hook_field(hook_field_key: Option<&str>, patch: &Value) -> bool {
    patch
        .as_object()
        .is_some_and(|keys| hook_field_key.is_none_or(|own| keys.keys().all(|key| key == own)))
}

/// A formula patch may touch only the hook's own field when it declares one, and only keys the
/// caller may write under the form's field write policy; checked before the merge.
async fn ensure_patch_allowed(
    state: &AppState,
    form_id: Uuid,
    role: &str,
    hook_field_key: Option<&str>,
    patch: &Value,
) -> Result<(), ApiError> {
    if !patch_within_hook_field(hook_field_key, patch) {
        return Err(patch_denied());
    }
    ensure_field_write_policy_allows(state, form_id, role, patch)
        .await
        .map_err(|error| match error {
            ApiError::Forbidden(_) => patch_denied(),
            other => other,
        })
}

fn formula_patch_from_output(output: &Value, field_key: Option<&str>) -> Result<Value, ApiError> {
    if let Some(patch) = output.get("patch") {
        if patch.is_object() {
            return Ok(patch.clone());
        }
        return Err(ApiError::BadRequest(
            "formula plugin patch must be an object".to_string(),
        ));
    }
    if let Some(field_key) = field_key
        && let Some(value) = output.get("value")
    {
        return Ok(json!({ field_key: value }));
    }
    Ok(json!({}))
}

fn merge_patch(values: &mut Value, patch: Value) -> Result<(), ApiError> {
    let Some(values_object) = values.as_object_mut() else {
        return Err(ApiError::BadRequest(
            "formula target values must be an object".to_string(),
        ));
    };
    let Some(patch_object) = patch.as_object() else {
        return Err(ApiError::BadRequest("formula patch must be an object".to_string()));
    };
    for (key, value) in patch_object {
        values_object.insert(key.clone(), value.clone());
    }
    Ok(())
}

async fn load_active_plugins(
    state: &AppState,
    workspace_id: Uuid,
    project_id: Uuid,
) -> Result<Vec<PluginHookRow>, ApiError> {
    Ok(PluginHookRow::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        r"SELECT id, workspace_id, project_id, key, manifest, wasm_bytes
             FROM plugins
            WHERE workspace_id = $1
              AND project_id = $2
              AND status = 'active'
              AND wasm_bytes IS NOT NULL
            ORDER BY key ASC",
        vec![workspace_id.into(), project_id.into()],
    ))
    .all(&state.db)
    .await?)
}

async fn insert_hook_invocation(
    state: &AppState,
    plugin: &PluginHookRow,
    hook_kind: &str,
    input: Value,
    output: PluginRuntimeOutput,
    refusal: Option<String>,
) -> Result<(), ApiError> {
    insert_invocation(
        state,
        plugin,
        InvocationInsert {
            hook_kind,
            // A validator that ran and returned `ok: false` rejected the value: it failed.
            status: if refusal.is_some() {
                PluginInvocationStatus::Failed
            } else {
                PluginInvocationStatus::Completed
            },
            input,
            output: output.output,
            error_code: refusal.map(|_| redact::ERROR_VALIDATION_FAILED),
            duration_ms: i64::try_from(output.duration_ms).unwrap_or(i64::MAX),
            fuel_consumed: output.fuel_consumed.and_then(|value| i64::try_from(value).ok()),
        },
    )
    .await
}

async fn insert_failed_hook_invocation(
    state: &AppState,
    plugin: &PluginHookRow,
    hook_kind: &str,
    input: Value,
    error: &PluginRuntimeError,
) -> Result<(), ApiError> {
    insert_invocation(
        state,
        plugin,
        InvocationInsert {
            hook_kind,
            status: error.kind.into(),
            input,
            output: json!({}),
            error_code: Some(match error.kind {
                crate::plugins::runtime::PluginFailureKind::Timeout => redact::ERROR_TIMEOUT,
                crate::plugins::runtime::PluginFailureKind::Failed => redact::ERROR_FAILED,
            }),
            duration_ms: i64::try_from(error.duration_ms).unwrap_or(i64::MAX),
            fuel_consumed: error.fuel_consumed.and_then(|value| i64::try_from(value).ok()),
        },
    )
    .await
}

struct InvocationInsert<'a> {
    hook_kind: &'a str,
    status: PluginInvocationStatus,
    input: Value,
    output: Value,
    error_code: Option<&'static str>,
    duration_ms: i64,
    fuel_consumed: Option<i64>,
}

async fn insert_invocation(
    state: &AppState,
    plugin: &PluginHookRow,
    invocation: InvocationInsert<'_>,
) -> Result<(), ApiError> {
    let invocation_id = Uuid::new_v4();
    let tx = state.db.begin().await?;
    tx.execute(Statement::from_sql_and_values(
        DbBackend::Postgres,
        r"INSERT INTO plugin_invocations (
                    id, workspace_id, project_id, plugin_id, plugin_key, hook_kind, status,
                    input, output, error_message, duration_ms, fuel_consumed
                )
                VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12)",
        vec![
            invocation_id.into(),
            plugin.workspace_id.into(),
            plugin.project_id.into(),
            plugin.id.into(),
            plugin.key.clone().into(),
            invocation.hook_kind.to_string().into(),
            invocation.status.as_str().into(),
            redact::digest_object(&invocation.input).into(),
            redact::digest_object(&invocation.output).into(),
            invocation.error_code.into(),
            invocation.duration_ms.into(),
            invocation.fuel_consumed.into(),
        ],
    ))
    .await?;
    insert_business_event(
        &tx,
        BusinessEventInput {
            workspace_id: plugin.workspace_id,
            project_id: Some(plugin.project_id),
            event_type: "plugin.invoked".to_string(),
            aggregate_type: "plugin".to_string(),
            aggregate_id: plugin.id.to_string(),
            actor_id: None,
            source: json!({
                "type": "system",
                "origin": "wasm_hook",
                "hook_kind": invocation.hook_kind
            }),
            payload: json!({
                "plugin_id": plugin.id,
                "plugin_key": plugin.key,
                "invocation_id": invocation_id,
                "hook_kind": invocation.hook_kind,
                "status": invocation.status.as_str(),
                "duration_ms": invocation.duration_ms,
                "fuel_consumed": invocation.fuel_consumed
            }),
            metadata: json!({
                "plugin_id": plugin.id,
                "plugin_key": plugin.key,
                "invocation_id": invocation_id,
                "hook_kind": invocation.hook_kind,
                "automatic_hook": true
            }),
            correlation_id: None,
            causation_id: None,
            idempotency_key: None,
        },
    )
    .await?;
    tx.commit().await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{
        formula_patch_from_output, hook_matches_field, hook_matches_form, merge_patch, patch_within_hook_field,
        validator_rejection_message,
    };
    use crate::plugins::manifest::PluginHook;
    use serde_json::json;

    #[test]
    fn hook_matching_respects_optional_form_and_field_scope() {
        let scoped = PluginHook {
            kind: "field_validator".to_string(),
            form_key: Some("order".to_string()),
            field_key: Some("amount".to_string()),
            event_type: None,
        };
        assert!(hook_matches_form(&scoped, "order"));
        assert!(!hook_matches_form(&scoped, "sku"));
        assert!(hook_matches_field(&scoped, "amount"));
        assert!(!hook_matches_field(&scoped, "quantity"));

        let global = PluginHook {
            kind: "field_validator".to_string(),
            form_key: None,
            field_key: None,
            event_type: None,
        };
        assert!(hook_matches_form(&global, "order"));
        assert!(hook_matches_field(&global, "amount"));
    }

    #[test]
    fn validator_output_can_reject_values() {
        assert_eq!(
            validator_rejection_message(&json!({"ok": false, "error": "amount too high"})).as_deref(),
            Some("amount too high")
        );
        assert!(validator_rejection_message(&json!({"ok": true})).is_none());
        assert!(validator_rejection_message(&json!({})).is_none());
    }

    /// A formula hook that declares its field may write that field only; one that declares none
    /// is left to the write policy.
    #[test]
    fn a_patch_stays_within_the_hooks_own_field() {
        assert!(patch_within_hook_field(Some("total"), &json!({"total": "1"})));
        assert!(patch_within_hook_field(Some("total"), &json!({})));
        assert!(!patch_within_hook_field(
            Some("total"),
            &json!({"total": "1", "status": "paid"})
        ));
        assert!(!patch_within_hook_field(Some("total"), &json!({"status": "paid"})));
        assert!(patch_within_hook_field(None, &json!({"status": "paid"})));
        assert!(!patch_within_hook_field(None, &json!([1])));
    }

    #[test]
    fn formula_output_can_patch_values() {
        let patch = formula_patch_from_output(&json!({"patch": {"total": "0.30"}}), None).expect("patch");
        assert_eq!(patch.get("total").and_then(serde_json::Value::as_str), Some("0.30"));

        let patch = formula_patch_from_output(&json!({"value": "0.30"}), Some("total")).expect("field patch");
        assert_eq!(patch.get("total").and_then(serde_json::Value::as_str), Some("0.30"));

        let mut values = json!({"subtotal": "0.10"});
        merge_patch(&mut values, patch).expect("merge");
        assert_eq!(values.get("total").and_then(serde_json::Value::as_str), Some("0.30"));
    }
}
