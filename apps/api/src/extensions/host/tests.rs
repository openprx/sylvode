//! The host against the committed sample and bad components (`examples/plugins/dist`), without a
//! database: everything a guest can do before it reaches a handler.
#![allow(clippy::expect_used, clippy::panic, clippy::indexing_slicing)]

use std::collections::BTreeSet;

use platform::{
    app::AppState,
    config::{AppConfig, Secret},
};
use sea_orm::DatabaseConnection;
use serde_json::json;
use uuid::Uuid;

use super::run::invoke_with;
use super::{GrantSet, Invocation, World};
use crate::catalogue::{Grant, InstallationView, Invoker, Principal, RefusalCode};
use crate::entities::workspace_member::WorkspaceRole;

const ACTION: &[u8] = include_bytes!("../../../../../examples/plugins/dist/service-desk-action.wasm");
const RULE: &[u8] = include_bytes!("../../../../../examples/plugins/dist/service-desk-rule.wasm");
const ENDLESS: &[u8] = include_bytes!("../../../../../examples/plugins/dist/bad-endless-loop.wasm");
const MEMORY: &[u8] = include_bytes!("../../../../../examples/plugins/dist/bad-memory-growth.wasm");
const CALLS: &[u8] = include_bytes!("../../../../../examples/plugins/dist/bad-host-calls.wasm");

const WORKSPACE: Uuid = Uuid::from_u128(0x10);
const USER: Uuid = Uuid::from_u128(0x20);
const RECORD: &str = "0b1d2f3e-4a5b-4c6d-8e7f-9a0b1c2d3e4f";

pub(super) fn state() -> AppState {
    AppState {
        cfg: AppConfig {
            app_name: "host-test".to_string(),
            bind_addr: "127.0.0.1:0".to_string(),
            database_url: Secret::new("postgres://unused/unused"),
            jwt_secret: Secret::new("a-test-signing-secret-of-some-length"),
            jwt_access_ttl_seconds: 900,
            jwt_refresh_ttl_seconds: 3600,
            allow_insecure_cookies: false,
            collab_allowed_origins: Vec::new(),
        },
        db: DatabaseConnection::Disconnected,
        flow_permission_cache: platform::app::FlowPermissionCacheSlot::default(),
    }
}

/// Each run is a user of its own: the rate limit counts per user and is process-wide.
fn principal(installation: Uuid) -> Principal {
    principal_of(installation, Uuid::new_v4())
}

fn principal_of(installation: Uuid, user: Uuid) -> Principal {
    Principal::Plugin {
        invoker: Invoker::User {
            user_id: user,
            workspace_id: WORKSPACE,
            role: WorkspaceRole::Member,
        },
        installation: InstallationView {
            installation_id: installation,
            generation: 1,
            workspace_id: WORKSPACE,
            project_id: None,
            requested: BTreeSet::new(),
            grants: Vec::new(),
        },
    }
}

fn grants(ids: &[&str]) -> GrantSet {
    GrantSet {
        requested: ids.iter().map(|id| (*id).to_string()).collect(),
        grants: ids
            .iter()
            .map(|id| Grant {
                capability: (*id).to_string(),
                project: None,
                form: None,
                plugin_ops_digest: String::new(),
            })
            .collect(),
    }
}

async fn run(component: &[u8], world: World, entry: &str, input: &str, grants: &GrantSet) -> Invocation {
    run_as(Uuid::new_v4(), component, world, entry, input, grants).await
}

async fn run_as(
    installation: Uuid,
    component: &[u8],
    world: World,
    entry: &str,
    input: &str,
    grants: &GrantSet,
) -> Invocation {
    invoke_with(
        &state(),
        &principal(installation),
        component,
        world,
        entry,
        input,
        grants,
        Some(0),
    )
    .await
}

fn code(invocation: &Invocation) -> (RefusalCode, Option<String>) {
    let error = invocation.result.as_ref().expect_err("the invocation is refused");
    (error.code, error.guest_code.clone())
}

fn rule_input(stage: &str, value: &serde_json::Value) -> String {
    json!({"stage": stage, "field_key": "ref", "field_type": "text", "value": value, "deps": [], "config": null})
        .to_string()
}

#[tokio::test]
async fn the_rule_normalizes_validates_and_keeps() {
    let none = grants(&[]);
    let normalized = run(
        RULE,
        World::FieldRule,
        "normalize",
        &rule_input("normalize", &json!(" sd-123456 ")),
        &none,
    )
    .await;
    assert_eq!(normalized.result.expect("ok"), json!({"replace": "SD-123456"}));
    let kept = run(
        RULE,
        World::FieldRule,
        "normalize",
        &rule_input("normalize", &json!("SD-123456")),
        &none,
    )
    .await;
    assert_eq!(kept.result.expect("ok"), json!({"keep": true}));
    let valid = run(
        RULE,
        World::FieldRule,
        "validate",
        &rule_input("validate", &json!("SD-123456")),
        &none,
    )
    .await;
    assert_eq!(valid.result.expect("ok"), json!({"keep": true}));
    assert_eq!(valid.record.outcome, "ok");
    assert_eq!(valid.record.principal_kind, "none");
    assert_eq!(valid.record.host_calls, 0);
    assert!(valid.record.fuel_used.is_some_and(|fuel| fuel > 0 && fuel < 2_000_000));
    let invalid = run(
        RULE,
        World::FieldRule,
        "validate",
        &rule_input("validate", &json!("SD-12")),
        &none,
    )
    .await;
    assert_eq!(
        code(&invalid),
        (RefusalCode::PluginGuestError, Some("invalid_reference".to_string()))
    );
    assert_eq!(invalid.record.outcome, "guest_error");
    assert_eq!(invalid.record.error_code.as_deref(), Some("invalid_reference"));
}

#[tokio::test]
async fn a_record_of_the_log_holds_digests_and_sizes_only() {
    let input = rule_input("validate", &json!("SD-123456"));
    let done = run(RULE, World::FieldRule, "validate", &input, &grants(&[])).await;
    let row = serde_json::to_string(&done.record).expect("json");
    assert!(!row.contains("SD-123456"), "{row}");
    assert!(done.record.input_digest.starts_with("hmac-sha256:"));
    assert_eq!(done.record.input_size, input.len());
    assert!(
        done.record
            .output_digest
            .as_deref()
            .is_some_and(|d| d.starts_with("hmac-sha256:"))
    );
}

#[tokio::test]
async fn a_replace_from_validate_and_bad_input_are_a_failure_of_the_guest() {
    let none = grants(&[]);
    let bad = run(RULE, World::FieldRule, "validate", "{\"stage\":\"validate\"}", &none).await;
    assert_eq!(code(&bad).0, RefusalCode::PluginFailed);
    let stage = run(
        RULE,
        World::FieldRule,
        "validate",
        &rule_input("explode", &json!("x")),
        &none,
    )
    .await;
    assert_eq!(code(&stage).0, RefusalCode::PluginFailed);
}

#[tokio::test]
async fn an_endless_loop_runs_out_of_fuel() {
    let done = run(ENDLESS, World::Action, "go", "{}", &grants(&[])).await;
    assert_eq!(code(&done).0, RefusalCode::PluginFuelExhausted);
    assert_eq!(done.record.outcome, "limit");
    assert_eq!(done.record.fuel_used, Some(World::Action.limits().fuel));
}

#[tokio::test]
async fn memory_growth_is_stopped_at_the_limit() {
    let done = run(MEMORY, World::Action, "go", "{}", &grants(&[])).await;
    assert_eq!(code(&done).0, RefusalCode::PluginMemoryLimit);
    assert_eq!(done.record.outcome, "limit");
}

#[tokio::test]
async fn the_201st_host_call_ends_the_invocation() {
    let done = run(CALLS, World::Action, "go", "{}", &grants(&["forms.records.read"])).await;
    assert_eq!(code(&done).0, RefusalCode::PluginHostCallLimit);
    assert_eq!(done.record.outcome, "limit");
    assert_eq!(done.record.host_calls, 201);
    // The first 200 were answered `capability_not_exposed`, each one a denial.
    assert_eq!(done.record.host_denials, 200);
    assert_eq!(done.record.last_denial.as_deref(), Some("capability_not_exposed"));
}

#[tokio::test]
async fn a_capability_outside_the_grant_is_refused_to_the_guest() {
    let input = json!({"record_id": RECORD}).to_string();
    // Read is granted; the sample plugin's update is not requested.
    let done = run(
        ACTION,
        World::Action,
        "close_request",
        &input,
        &grants(&["forms.records.read"]),
    )
    .await;
    assert_eq!(
        code(&done),
        (
            RefusalCode::PluginGuestError,
            Some("capability_not_requested".to_string())
        )
    );
    assert_eq!(done.record.host_calls, 1);
    assert_eq!(done.record.host_denials, 1);
    assert_eq!(done.record.last_denial.as_deref(), Some("capability_not_requested"));
    // Requested but not granted.
    let mut asked = grants(&["forms.records.read"]);
    asked.requested.insert("forms.records.update".to_string());
    let done = run(ACTION, World::Action, "close_request", &input, &asked).await;
    assert_eq!(
        code(&done),
        (
            RefusalCode::PluginGuestError,
            Some("capability_not_granted".to_string())
        )
    );
    assert_eq!(done.record.last_denial.as_deref(), Some("capability_not_granted"));
}

#[tokio::test]
async fn a_granted_call_reaches_the_resolution_of_its_target() {
    // With no database the resolution fails; the guest sees `unavailable`, not a handler message.
    let input = json!({"record_id": RECORD}).to_string();
    let done = run(
        ACTION,
        World::Action,
        "close_request",
        &input,
        &grants(&["forms.records.update"]),
    )
    .await;
    assert_eq!(
        code(&done),
        (RefusalCode::PluginGuestError, Some("unavailable".to_string()))
    );
    assert_eq!(done.record.host_denials, 0);
}

#[tokio::test]
async fn a_bad_record_id_is_refused_before_any_lookup() {
    let input = json!({"record_id": "../other"}).to_string();
    let done = run(
        ACTION,
        World::Action,
        "close_request",
        &input,
        &grants(&["forms.records.update"]),
    )
    .await;
    assert_eq!(
        code(&done),
        (RefusalCode::PluginGuestError, Some("bad_request".to_string()))
    );
}

#[tokio::test]
async fn an_interface_without_a_usable_grant_is_not_linked() {
    // The sample action imports `records`; with no grant at all the host provides nothing.
    let input = json!({"record_id": RECORD}).to_string();
    let none = run(ACTION, World::Action, "close_request", &input, &grants(&[])).await;
    assert_eq!(code(&none).0, RefusalCode::PluginUnknownImport);
    assert_eq!(none.record.outcome, "refused");
    // A grant that the manifest did not request does not count.
    let mut unrequested = grants(&["forms.records.update"]);
    unrequested.requested.clear();
    let unrequested = run(ACTION, World::Action, "close_request", &input, &unrequested).await;
    assert_eq!(code(&unrequested).0, RefusalCode::PluginUnknownImport);
}

#[tokio::test]
async fn a_component_with_an_unknown_import_or_version_is_refused() {
    let wasi = wat::parse_str(
        r#"(component (import "wasi:cli/environment@0.2.0" (instance)) (core module $m (func (export "run"))))"#,
    )
    .expect("wat");
    let done = run(&wasi, World::Action, "go", "{}", &grants(&[])).await;
    // No `run` export either: the world is checked first.
    assert_eq!(code(&done).0, RefusalCode::PluginWorldMismatch);

    let with_run = |import: &str| {
        wat::parse_str(format!(
            r#"(component
                (import "{import}" (instance))
                (core module $m (func (export "run") (param i32 i32 i32 i32) (result i32) i32.const 0)
                  (memory (export "memory") 1)
                  (func (export "cabi_realloc") (param i32 i32 i32 i32) (result i32) i32.const 0))
                (core instance $i (instantiate $m))
                (func (export "run") (param "action" string) (param "input" string) (result (result string (error string)))
                  (canon lift (core func $i "run") (memory (core memory $i "memory")) (realloc (core func $i "cabi_realloc")))))"#
        ))
        .expect("wat")
    };
    let unknown = run(
        &with_run("wasi:cli/environment@0.2.0"),
        World::Action,
        "go",
        "{}",
        &grants(&[]),
    )
    .await;
    assert_eq!(code(&unknown).0, RefusalCode::PluginUnknownImport);
    let version = run(
        &with_run("sv:plugin/records@3.0.0"),
        World::Action,
        "go",
        "{}",
        &grants(&[]),
    )
    .await;
    assert_eq!(code(&version).0, RefusalCode::PluginWitVersion);
    let other = run(
        &with_run("sv:plugin/secrets@4.0.0"),
        World::Action,
        "go",
        "{}",
        &grants(&[]),
    )
    .await;
    assert_eq!(code(&other).0, RefusalCode::PluginUnknownImport);
}

#[tokio::test]
async fn the_wrong_world_and_garbage_are_refused() {
    let rule_as_action = run(RULE, World::Action, "go", "{}", &grants(&[])).await;
    assert_eq!(code(&rule_as_action).0, RefusalCode::PluginWorldMismatch);
    let action_as_rule = run(ACTION, World::FieldRule, "go", "{}", &grants(&[])).await;
    assert_eq!(code(&action_as_rule).0, RefusalCode::PluginWorldMismatch);
    let garbage = run(b"not a component", World::Action, "go", "{}", &grants(&[])).await;
    assert_eq!(code(&garbage).0, RefusalCode::PluginInvalidComponent);
}

#[tokio::test]
async fn sizes_are_checked_before_the_guest_runs() {
    let big_input = "x".repeat(World::FieldRule.limits().input + 1);
    let done = run(RULE, World::FieldRule, "validate", &big_input, &grants(&[])).await;
    assert_eq!(code(&done), (RefusalCode::PluginInputTooLarge, None));
    let big_component = vec![0_u8; World::FieldRule.limits().component + 1];
    let done = run(&big_component, World::FieldRule, "validate", "{}", &grants(&[])).await;
    assert_eq!(code(&done), (RefusalCode::PluginComponentTooLarge, None));
    let done = run(RULE, World::FieldRule, "Not A Name", "{}", &grants(&[])).await;
    assert_eq!(code(&done), (RefusalCode::PluginActionUnknown, None));
}

#[tokio::test]
async fn a_user_principal_cannot_run_a_plugin() {
    let user = Principal::User {
        user_id: USER,
        workspace_id: WORKSPACE,
        role: WorkspaceRole::Owner,
    };
    let done = invoke_with(
        &state(),
        &user,
        RULE,
        World::FieldRule,
        "validate",
        "{}",
        &grants(&[]),
        Some(0),
    )
    .await;
    assert_eq!(code(&done).0, RefusalCode::CapabilityNotExposed);
}

#[tokio::test]
async fn the_per_installation_ceiling_refuses_instead_of_queueing() {
    let installation = Uuid::new_v4();
    let held: Vec<_> = (0..World::Action.limits().per_installation)
        .map(|_| {
            super::limits::gates()
                .admit(World::Action, installation)
                .expect("within the ceiling")
        })
        .collect();
    let done = run_as(installation, ENDLESS, World::Action, "go", "{}", &grants(&[])).await;
    assert_eq!(code(&done), (RefusalCode::PluginConcurrencyLimit, None));
    assert_eq!(done.record.outcome, "refused");
    drop(held);
}

#[tokio::test]
async fn the_rate_per_user_is_enforced_for_actions() {
    let user = Uuid::new_v4();
    let mut codes = Vec::new();
    for _ in 0..=super::limits::USER_RATE {
        let done = invoke_with(
            &state(),
            &principal_of(Uuid::new_v4(), user),
            b"not a component",
            World::Action,
            "go",
            "{}",
            &grants(&[]),
            Some(0),
        )
        .await;
        codes.push(code(&done).0);
    }
    let refused = codes
        .iter()
        .filter(|code| **code == RefusalCode::PluginConcurrencyLimit)
        .count();
    assert_eq!(refused, 1, "{codes:?}");
    assert_eq!(codes.last(), Some(&RefusalCode::PluginConcurrencyLimit));
}

/// Two memories of 40 MiB each are 80 MiB in one instance, over the 64 MiB limit of an action: the
/// limit is on the sum of the instance's memories, not on each one.
#[tokio::test]
async fn several_memories_share_one_memory_limit() {
    let component = |second: u32| {
        wat::parse_str(format!(
            r#"(component
                (core module $m
                  (memory (export "memory") 640)
                  (memory {second})
                  (func (export "run") (param i32 i32 i32 i32) (result i32) i32.const 0)
                  (func (export "cabi_realloc") (param i32 i32 i32 i32) (result i32) i32.const 0))
                (core instance $i (instantiate $m))
                (type $err0 (record (field "code" string)))
                (export $err "action-error" (type $err0))
                (type $answer (result string (error $err)))
                (func $run (param "action" string) (param "input" string) (result $answer)
                  (canon lift (core func $i "run") (memory (core memory $i "memory")) (realloc (core func $i "cabi_realloc"))))
                (export "run" (func $run)))"#
        ))
        .expect("wat")
    };
    let refused = run(&component(640), World::Action, "go", "{}", &grants(&[])).await;
    assert_eq!(code(&refused).0, RefusalCode::PluginMemoryLimit);
    assert_eq!(refused.record.outcome, "limit");
    // 40 MiB and 16 MiB fit: the same shape under the limit is not refused for its memory.
    let fits = run(&component(256), World::Action, "go", "{}", &grants(&[])).await;
    assert_ne!(
        fits.result.as_ref().err().map(|error| error.code),
        Some(RefusalCode::PluginMemoryLimit)
    );
}
