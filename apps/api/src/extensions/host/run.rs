//! One invocation from admission to the log record.

use std::sync::Arc;
use std::time::{Duration, Instant};

use platform::app::AppState;
use sea_orm::{ConnectionTrait, DbBackend, Statement};
use serde::Deserialize;
use serde_json::Value;
use uuid::Uuid;
use wasmtime::component::{Component, HasSelf, Linker};
use wasmtime::{Store, Trap};

use super::bindings::{action, rule};
use super::engine::{compile_cache, deadline_ticks, engine};
use super::error::{HostError, valid_guest_code};
use super::limits::{World, gates};
use super::log::{InvocationRecord, digest, digest_key, emit, store};
use super::state::{HostState, InvocationInfo};
use super::{GrantSet, Invocation};
use crate::catalogue::{Grant, InstallationView, Invoker, Principal, RefusalCode};
use crate::extensions::dispatch::{CallEnv, Live};

/// The package of the interfaces a plugin may import.
const IMPORT_PREFIX: &str = "sv:plugin/";
const VERSION: &str = "4.0.0";
/// The capabilities the `records` interface calls.
const RECORD_CAPABILITIES: [&str; 3] = ["forms.records.read", "forms.records.create", "forms.records.update"];

/// What the log record needs from a run, filled even when the run fails.
#[derive(Default)]
struct Telemetry {
    fuel_used: Option<u64>,
    host_calls: u32,
    log_lines: u32,
    denials: u32,
    last_denial: Option<String>,
    output: Option<String>,
}

struct Who {
    installation_id: Uuid,
    generation: i32,
    workspace_id: Uuid,
    actor: Uuid,
    /// The approving administrator when the call is non-interactive (an installation principal).
    approver: Option<Uuid>,
}

const fn who(principal: &Principal) -> Result<(Who, &InstallationView), HostError> {
    let (installation, actor, approver) = match principal {
        Principal::Plugin {
            invoker: Invoker::User { user_id, .. },
            installation,
        } => (installation, *user_id, None),
        Principal::Installation {
            installation,
            approver_user_id,
        } => (installation, *approver_user_id, Some(*approver_user_id)),
        _ => return Err(HostError::new(RefusalCode::CapabilityNotExposed)),
    };
    Ok((
        Who {
            installation_id: installation.installation_id,
            generation: installation.generation,
            workspace_id: installation.workspace_id,
            actor,
            approver,
        },
        installation,
    ))
}

/// The component an invocation runs: bytes that go through the compile cache, or a component the
/// caller compiled earlier (so the run does not depend on the cache still holding it).
#[derive(Clone, Copy)]
pub enum Source<'a> {
    Bytes(&'a [u8]),
    Compiled {
        component: &'a Component,
        /// The size of the component's bytes, checked against the world's limit like the bytes are.
        size: usize,
    },
}

impl Source<'_> {
    const fn size(&self) -> usize {
        match self {
            Self::Bytes(bytes) => bytes.len(),
            Self::Compiled { size, .. } => *size,
        }
    }
}

/// Runs `component` for an installation principal. Its requested ids and grants are the ones in the
/// principal's installation view; every host call loads them again (see `dispatch::fresh_principal`).
pub async fn invoke(
    state: &AppState,
    principal: &Principal,
    component: Source<'_>,
    world: World,
    entry: &str,
    input: &str,
) -> Invocation {
    let grants = match principal {
        Principal::Plugin { installation, .. } | Principal::Installation { installation, .. } => GrantSet {
            requested: installation.requested.clone(),
            grants: installation.grants.clone(),
        },
        _ => GrantSet::default(),
    };
    invoke_source(state, principal, component, world, entry, input, &grants, None).await
}

/// [`invoke`] with the invoking user's `session_version` given instead of read from the database,
/// for tests of the host without a database: the calls of such a run do not load the installation
/// again and nothing is written to the invocation log. `None` is the production path.
#[cfg(test)]
#[allow(clippy::too_many_arguments)]
pub(super) async fn invoke_with(
    state: &AppState,
    principal: &Principal,
    component: &[u8],
    world: World,
    entry: &str,
    input: &str,
    grants: &GrantSet,
    session: Option<i64>,
) -> Invocation {
    invoke_source(
        state,
        principal,
        Source::Bytes(component),
        world,
        entry,
        input,
        grants,
        session,
    )
    .await
}

#[allow(clippy::too_many_arguments)]
async fn invoke_source(
    state: &AppState,
    principal: &Principal,
    component: Source<'_>,
    world: World,
    entry: &str,
    input: &str,
    grants: &GrantSet,
    session: Option<i64>,
) -> Invocation {
    let started = Instant::now();
    let key = digest_key(state.cfg.jwt_secret.expose());
    let mut telemetry = Telemetry::default();
    let identity = who(principal);
    let result = match &identity {
        Ok((who, installation)) => {
            execute(
                state,
                who,
                installation,
                component,
                world,
                entry,
                input,
                grants,
                session,
                &mut telemetry,
            )
            .await
        }
        Err(error) => Err(error.clone()),
    };
    let (installation_id, generation, workspace_id, actor) = identity.as_ref().map_or_else(
        |_| (Uuid::nil(), 0, Uuid::nil(), None),
        |(who, _)| (who.installation_id, who.generation, who.workspace_id, Some(who.actor)),
    );
    let (outcome, error_code) = match &result {
        Ok(_) => ("ok", None),
        Err(error) => (
            error.outcome(),
            Some(
                error
                    .guest_code
                    .clone()
                    .unwrap_or_else(|| error.code.as_str().to_string()),
            ),
        ),
    };
    let record = InvocationRecord {
        id: Uuid::new_v4(),
        workspace_id,
        installation_id,
        generation,
        world: world.name(),
        entry: entry.to_string(),
        principal_kind: if world == World::FieldRule { "none" } else { "user" },
        principal_id: if world == World::FieldRule { None } else { actor },
        input_digest: digest(key.as_ref(), input.as_bytes()),
        input_size: input.len(),
        output_digest: telemetry
            .output
            .as_ref()
            .map(|output| digest(key.as_ref(), output.as_bytes())),
        output_size: telemetry.output.as_ref().map(String::len),
        outcome,
        error_code,
        duration_ms: u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX),
        fuel_used: telemetry.fuel_used,
        host_calls: telemetry.host_calls,
        log_lines: telemetry.log_lines,
        host_denials: telemetry.denials,
        last_denial: telemetry.last_denial,
    };
    emit(&record);
    if session.is_none() {
        if world == World::FieldRule {
            // A rule runs while its writer's transaction is open: the row is written by a task of its
            // own, so the write never waits for a second connection.
            let (state, record) = (state.clone(), record.clone());
            tokio::spawn(async move { store(&state, &record).await });
        } else {
            store(state, &record).await;
        }
    }
    Invocation { result, record }
}

fn valid_entry(entry: &str) -> bool {
    (1..=64).contains(&entry.len())
        && entry
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || matches!(byte, b'_' | b'.' | b'-'))
}

/// Which interfaces of the `action` world a grant set can use. `ctx`, `log` and `clock` are always
/// linked; `records` only when a grant names a record capability; `catalogue` when there is any grant.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Plan {
    records: bool,
    catalogue: bool,
}

fn plan(grants: &GrantSet) -> Plan {
    let usable = |grant: &&Grant| grants.requested.contains(&grant.capability);
    Plan {
        records: grants
            .grants
            .iter()
            .filter(usable)
            .any(|grant| RECORD_CAPABILITIES.contains(&grant.capability.as_str())),
        catalogue: grants.grants.iter().any(|grant| usable(&grant)),
    }
}

/// The imports a component of `world` may have, as full names.
fn allowed_imports(world: World, plan: Plan) -> Vec<String> {
    let name = |interface: &str| format!("{IMPORT_PREFIX}{interface}@{VERSION}");
    match world {
        // `types` and `rule-types` only declare types; they have no functions to provide.
        World::FieldRule => vec![name("types"), name("rule-types")],
        World::Action => {
            // `types` only declares types.
            let mut names = vec![name("types"), name("ctx"), name("log"), name("clock")];
            if plan.records {
                names.push(name("records"));
            }
            if plan.catalogue {
                names.push(name("catalogue"));
            }
            names
        }
    }
}

/// Refuses a component that does not export the world's function, or imports anything the host will
/// not provide. Runs before instantiation so each case has its own typed refusal.
fn check_shape(
    engine: &wasmtime::Engine,
    component: &wasmtime::component::Component,
    world: World,
    plan: Plan,
) -> Result<(), HostError> {
    let kind = component.component_type();
    let export = match world {
        World::FieldRule => "apply",
        World::Action => "run",
    };
    if !kind.exports(engine).any(|(name, _)| name == export) {
        return Err(HostError::new(RefusalCode::PluginWorldMismatch));
    }
    let allowed = allowed_imports(world, plan);
    for (name, _) in kind.imports(engine) {
        if allowed.iter().any(|known| known == name) {
            continue;
        }
        let other_version = name.starts_with(IMPORT_PREFIX) && !name.ends_with(&format!("@{VERSION}"));
        return Err(HostError::new(if other_version {
            RefusalCode::PluginWitVersion
        } else {
            RefusalCode::PluginUnknownImport
        }));
    }
    Ok(())
}

/// Checks a component of a package at install time (contract C1 §6).
///
/// The checks are the size limit, that it compiles, exports the world's function and imports only
/// interfaces of this host's package version that the world may have.
///
/// Every interface of the world counts as allowed here; which of them an invocation links depends
/// on the grants.
///
/// # Errors
/// `plugin_component_too_large`, `plugin_invalid_component`, `plugin_world_mismatch`,
/// `plugin_wit_version` or `plugin_unknown_import`.
pub async fn verify_component(bytes: &[u8], world: World) -> Result<(), HostError> {
    if bytes.len() > world.limits().component {
        return Err(HostError::new(RefusalCode::PluginComponentTooLarge));
    }
    let engine = engine()?;
    let component = compile_cache().get(engine, bytes).await?;
    check_shape(
        engine,
        &component,
        world,
        Plan {
            records: true,
            catalogue: true,
        },
    )
}

async fn session_version(state: &AppState, actor: Uuid) -> Result<i64, HostError> {
    let failed = || HostError::new(RefusalCode::PluginFailed);
    let row = state
        .db
        .query_one(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT is_active, session_version FROM users WHERE id = $1",
            vec![actor.into()],
        ))
        .await
        .map_err(|_| failed())?
        .ok_or_else(failed)?;
    let active: bool = row.try_get("", "is_active").map_err(|_| failed())?;
    let version: i64 = row.try_get("", "session_version").map_err(|_| failed())?;
    if active { Ok(version) } else { Err(failed()) }
}

/// Maps a wasmtime error: a host decision first, then the traps the limits cause.
fn map_error(error: &wasmtime::Error) -> HostError {
    if let Some(host) = error.downcast_ref::<HostError>() {
        return host.clone();
    }
    match error.downcast_ref::<Trap>() {
        Some(Trap::OutOfFuel) => HostError::new(RefusalCode::PluginFuelExhausted),
        Some(Trap::Interrupt) => HostError::new(RefusalCode::PluginDeadlineExceeded),
        _ => HostError::new(RefusalCode::PluginFailed),
    }
}

#[allow(clippy::too_many_arguments)]
async fn execute(
    state: &AppState,
    who: &Who,
    installation: &InstallationView,
    source: Source<'_>,
    world: World,
    entry: &str,
    input: &str,
    grants: &GrantSet,
    session: Option<i64>,
    telemetry: &mut Telemetry,
) -> Result<Value, HostError> {
    let limits = world.limits();
    if !valid_entry(entry) {
        return Err(HostError::new(RefusalCode::PluginActionUnknown));
    }
    if source.size() > limits.component {
        return Err(HostError::new(RefusalCode::PluginComponentTooLarge));
    }
    if input.len() > limits.input {
        return Err(HostError::new(RefusalCode::PluginInputTooLarge));
    }
    if world == World::Action {
        gates().rate(who.actor, Instant::now())?;
    }
    // Compile (or fetch) before taking any slot: a compile in progress must not hold an instance
    // or a memory reservation.
    let engine = engine()?;
    let component = match source {
        Source::Bytes(bytes) => compile_cache().get(engine, bytes).await?,
        Source::Compiled { component, .. } => component.clone(),
    };
    let _admission = gates().admit(world, who.installation_id)?;
    let plan = plan(grants);
    check_shape(engine, &component, world, plan)?;

    let deadline = Instant::now() + limits.wall;
    let env = if world == World::Action {
        let view = InstallationView {
            requested: grants.requested.clone(),
            grants: grants.grants.clone(),
            ..installation.clone()
        };
        let invoker = Invoker::User {
            user_id: who.actor,
            workspace_id: who.workspace_id,
            role: crate::entities::workspace_member::WorkspaceRole::Member,
        };
        let live = (session.is_none()).then_some(Live { approver: who.approver });
        let principal = match who.approver {
            Some(approver_user_id) => Principal::Installation {
                installation: view,
                approver_user_id,
            },
            None => Principal::Plugin {
                invoker,
                installation: view,
            },
        };
        Some(Arc::new(CallEnv {
            state: state.clone(),
            principal: Arc::new(principal),
            live,
            actor: who.actor,
            session_version: match session {
                Some(version) => version,
                None => session_version(state, who.actor).await?,
            },
            installation_id: who.installation_id,
            generation: who.generation,
            request_id: Uuid::new_v4(),
        }))
    } else {
        None
    };
    let info = InvocationInfo {
        actor: who.actor.to_string(),
        workspace: who.workspace_id.to_string(),
        project: installation.project_id.map(|project| project.to_string()),
        installation: who.installation_id.to_string(),
        generation: u32::try_from(who.generation).unwrap_or(0),
        action: entry.to_string(),
        request_id: env.as_ref().map_or_else(Uuid::nil, |env| env.request_id).to_string(),
    };
    let mut store = Store::new(engine, HostState::new(&limits, env, info, deadline));
    store.limiter(HostState::limiter);
    store
        .set_fuel(limits.fuel)
        .map_err(|_| HostError::new(RefusalCode::PluginFailed))?;
    store.set_epoch_deadline(deadline_ticks(limits.wall));
    store.epoch_deadline_trap();

    // A little over the epoch deadline: the epoch stops a running guest, this stops a host call that
    // does not come back.
    let outer = limits.wall + Duration::from_millis(250);
    let ran = tokio::time::timeout(outer, run_world(&mut store, &component, world, plan, entry, input)).await;
    telemetry.fuel_used = store.get_fuel().ok().map(|left| limits.fuel.saturating_sub(left));
    let data = store.data();
    telemetry.host_calls = data.host_calls;
    telemetry.log_lines = data.log_lines;
    telemetry.denials = data.denials;
    telemetry.last_denial.clone_from(&data.last_denial);
    let text = ran.map_err(|_| HostError::new(RefusalCode::PluginDeadlineExceeded))??;
    if text.len() > limits.output {
        return Err(HostError::new(RefusalCode::PluginOutputTooLarge));
    }
    let value: Value = serde_json::from_str(&text).map_err(|_| HostError::new(RefusalCode::PluginFailed))?;
    telemetry.output = Some(text);
    Ok(value)
}

/// Instantiates and calls the world's export; returns the output as JSON text.
async fn run_world(
    store: &mut Store<HostState>,
    component: &wasmtime::component::Component,
    world: World,
    plan: Plan,
    entry: &str,
    input: &str,
) -> Result<String, HostError> {
    let engine = store.engine().clone();
    match world {
        World::Action => {
            let mut linker = Linker::<HostState>::new(&engine);
            let wired = (|| -> wasmtime::Result<()> {
                action::sv::plugin::ctx::add_to_linker::<HostState, HasSelf<HostState>>(&mut linker, |state| state)?;
                action::sv::plugin::log::add_to_linker::<HostState, HasSelf<HostState>>(&mut linker, |state| state)?;
                action::sv::plugin::clock::add_to_linker::<HostState, HasSelf<HostState>>(&mut linker, |state| state)?;
                if plan.records {
                    action::sv::plugin::records::add_to_linker::<HostState, HasSelf<HostState>>(
                        &mut linker,
                        |state| state,
                    )?;
                }
                if plan.catalogue {
                    action::sv::plugin::catalogue::add_to_linker::<HostState, HasSelf<HostState>>(
                        &mut linker,
                        |state| state,
                    )?;
                }
                Ok(())
            })();
            wired.map_err(|_| HostError::new(RefusalCode::PluginFailed))?;
            let instance = action::Action::instantiate_async(&mut *store, component, &linker)
                .await
                .map_err(|error| map_error(&error))?;
            let answer = instance
                .call_run(&mut *store, entry, &input.to_string())
                .await
                .map_err(|error| map_error(&error))?;
            match answer {
                Ok(text) => Ok(text),
                Err(error) if valid_guest_code(&error.code) => Err(HostError::guest(error.code)),
                Err(_) => Err(HostError::new(RefusalCode::PluginFailed)),
            }
        }
        World::FieldRule => {
            let linker = Linker::<HostState>::new(&engine);
            let instance = rule::FieldRule::instantiate_async(&mut *store, component, &linker)
                .await
                .map_err(|error| map_error(&error))?;
            let parsed = parse_rule_input(input)?;
            let stage_validates = matches!(parsed.stage, rule::sv::plugin::rule_types::Stage::Validate);
            let answer = instance
                .call_apply(&mut *store, &parsed)
                .await
                .map_err(|error| map_error(&error))?;
            match answer {
                Ok(rule::sv::plugin::rule_types::RuleOutcome::Keep) => Ok("{\"keep\":true}".to_string()),
                Ok(rule::sv::plugin::rule_types::RuleOutcome::Replace(value)) => {
                    // Only normalize and transform may replace a value.
                    if stage_validates || serde_json::from_str::<Value>(&value).is_err() {
                        Err(HostError::new(RefusalCode::PluginFailed))
                    } else {
                        Ok(format!("{{\"replace\":{value}}}"))
                    }
                }
                Err(violation) if valid_guest_code(&violation.code) => Err(HostError::guest(violation.code)),
                Err(_) => Err(HostError::new(RefusalCode::PluginFailed)),
            }
        }
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RuleInputJson {
    stage: String,
    field_key: String,
    field_type: String,
    value: Value,
    #[serde(default)]
    deps: Vec<(String, Value)>,
    #[serde(default)]
    config: Value,
}

fn parse_rule_input(input: &str) -> Result<rule::sv::plugin::rule_types::RuleInput, HostError> {
    use rule::sv::plugin::rule_types::{RuleInput, Stage};
    let bad = || HostError::new(RefusalCode::PluginFailed);
    let parsed: RuleInputJson = serde_json::from_str(input).map_err(|_| bad())?;
    let stage = match parsed.stage.as_str() {
        "normalize" => Stage::Normalize,
        "validate" => Stage::Validate,
        "transform" => Stage::Transform,
        _ => return Err(bad()),
    };
    Ok(RuleInput {
        stage,
        field_key: parsed.field_key,
        field_type: parsed.field_type,
        value: parsed.value.to_string(),
        deps: parsed
            .deps
            .into_iter()
            .map(|(key, value)| (key, value.to_string()))
            .collect(),
        config: parsed.config.to_string(),
    })
}

#[cfg(test)]
#[allow(clippy::expect_used)]
mod epoch_tests {
    use std::time::{Duration, Instant};

    use wasmtime::Store;

    use super::run_world;
    use super::{Plan, World};
    use crate::catalogue::RefusalCode;
    use crate::extensions::host::engine::{compile_cache, deadline_ticks, engine};
    use crate::extensions::host::state::{HostState, InvocationInfo};

    const ENDLESS: &[u8] = include_bytes!("../../../../../examples/plugins/dist/bad-endless-loop.wasm");

    /// The epoch deadline is its own limit: a guest that still has fuel is stopped by the clock, with
    /// `plugin_deadline_exceeded` (the fuel limit is tested with the same fixture and the real budget).
    #[tokio::test]
    async fn the_epoch_deadline_stops_a_guest_that_still_has_fuel() {
        let engine = engine().expect("engine");
        let component = compile_cache().get(engine, ENDLESS).await.expect("component");
        let limits = World::Action.limits();
        let wall = Duration::from_millis(40);
        let info = InvocationInfo {
            actor: String::new(),
            workspace: String::new(),
            project: None,
            installation: String::new(),
            generation: 1,
            action: "spin".to_string(),
            request_id: String::new(),
        };
        let mut store = Store::new(engine, HostState::new(&limits, None, info, Instant::now() + wall));
        store.limiter(HostState::limiter);
        // More fuel than a 40 ms run can burn, so only the epoch can stop it.
        store.set_fuel(u64::MAX / 2).expect("fuel");
        store.set_epoch_deadline(deadline_ticks(wall));
        store.epoch_deadline_trap();
        let plan = Plan {
            records: false,
            catalogue: false,
        };
        let started = Instant::now();
        let error = run_world(&mut store, &component, World::Action, plan, "spin", "{}")
            .await
            .expect_err("the guest never returns");
        assert_eq!(error.code, RefusalCode::PluginDeadlineExceeded);
        assert!(
            started.elapsed() < Duration::from_secs(2),
            "stopped by the clock, not by luck"
        );
    }
}
