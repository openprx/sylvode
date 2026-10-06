use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio::task::{JoinError, JoinHandle};
use wasmtime::{Config, Engine, Instance, Module, Store, StoreLimits, StoreLimitsBuilder, UpdateDeadline};

use super::manifest::PluginRuntimePolicy;

const ABI_VERSION: i32 = 1;
const MAX_OUTPUT_BYTES: usize = 1024 * 1024;
/// The most elements a plugin's table may hold, its initial size included.
///
/// A table element costs the host one pointer (8 bytes), so this caps table memory at 512 KiB
/// per invocation. Plugins use their table for indirect calls only; a compiled Rust plugin has
/// a few hundred entries.
pub const MAX_TABLE_ELEMENTS: usize = 65_536;
/// The largest plugin module accepted at install and at invocation, in bytes.
///
/// Compilation cannot be interrupted, so the module size is what bounds the time a blocking
/// thread spends compiling. A plugin implements the three-export core ABI with no imports (no
/// WASI, no libc I/O); an optimised Rust plugin that parses and writes JSON with `serde_json` is
/// a few hundred KiB, so 4 MiB leaves about ten times that headroom while staying far below the
/// 200 MiB request body limit.
pub const MAX_MODULE_BYTES: usize = 4 * 1024 * 1024;
/// How often the caller re-advances the engine epoch after the deadline while it waits for the
/// guest thread to return. One advance normally suffices; the repeats close the window where a
/// store was created, or a guest observed the epoch, concurrently with the first advance.
const INTERRUPT_RETRY_INTERVAL: Duration = Duration::from_millis(10);

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PluginRuntimeOutput {
    pub output: Value,
    pub duration_ms: u64,
    pub fuel_consumed: Option<u64>,
}

/// How a plugin run that produced no output ended, as `plugin_invocations.status` records it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PluginFailureKind {
    /// The wall-clock deadline (`runtime.timeout_ms`) expired first.
    Timeout,
    /// Anything else: a module that is too large, does not compile or does not instantiate, a
    /// missing export, a guest trap (fuel exhaustion included), or output the host cannot read.
    Failed,
}

/// A plugin run that produced no output, with everything the store knew when it stopped.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PluginRuntimeError {
    pub kind: PluginFailureKind,
    /// The user-visible reason. A timeout reads exactly `wasm execution timeout after {N}ms`.
    pub message: String,
    /// Wall-clock time from the start of the invocation until it ended.
    pub duration_ms: u64,
    /// Fuel the guest burnt before it stopped; `None` when no store with fuel ever existed
    /// (the module did not compile, or the deadline passed before it had) or the run could not be
    /// observed.
    pub fuel_consumed: Option<u64>,
}

impl std::fmt::Display for PluginRuntimeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for PluginRuntimeError {}

/// `plugin_invocations.status`, whose CHECK constraint admits exactly these three values.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PluginInvocationStatus {
    Completed,
    Failed,
    Timeout,
}

impl PluginInvocationStatus {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Completed => "completed",
            Self::Failed => "failed",
            Self::Timeout => "timeout",
        }
    }
}

impl From<PluginFailureKind> for PluginInvocationStatus {
    fn from(kind: PluginFailureKind) -> Self {
        match kind {
            PluginFailureKind::Timeout => Self::Timeout,
            PluginFailureKind::Failed => Self::Failed,
        }
    }
}

#[derive(Debug)]
struct StoreState {
    limits: StoreLimits,
}

/// What the invocation and its blocking task share.
///
/// `expired` is set by the caller once the deadline has passed (or the invocation was dropped);
/// `guest_started` is set by the task once the guest's store exists and guest code may run. Both
/// use sequentially consistent operations, so of "the caller sets `expired` then reads
/// `guest_started`" and "the task sets `guest_started` then reads `expired`" at least one side
/// sees the other's write: either the caller knows the guest started and interrupts it, or the
/// task sees the deadline and never runs guest code.
#[derive(Debug, Default)]
struct InvocationFlags {
    expired: AtomicBool,
    guest_started: AtomicBool,
}

/// Runs a plugin with its wall-clock deadline enforced from the start of the request.
///
/// The deadline covers waiting for a blocking thread, compiling the module, instantiating it and
/// running the guest. Compilation cannot be interrupted; when the deadline passes before the
/// guest has started, the invocation returns the timeout at once and the blocking task stops
/// before running any guest code as soon as compilation (bounded by [`MAX_MODULE_BYTES`])
/// finishes. A guest that is running is interrupted through the engine epoch.
///
/// Every invocation builds its own engine, so the epoch advances issued here only ever reach this
/// invocation's store. The store deadline is still expressed relative to the current epoch and the
/// epoch callback only interrupts once this invocation's own `expired` flag is set, so the
/// mechanism stays isolated even if an engine is later shared between invocations.
pub async fn invoke_wasm_plugin(
    wasm_bytes: Vec<u8>,
    input: Value,
    policy: PluginRuntimePolicy,
) -> Result<PluginRuntimeOutput, PluginRuntimeError> {
    let engine = build_engine().map_err(|message| PluginRuntimeError {
        kind: PluginFailureKind::Failed,
        message,
        duration_ms: 0,
        fuel_consumed: None,
    })?;
    invoke_on_engine(&engine, wasm_bytes, input, policy).await
}

/// Checks that `wasm_bytes` is a module the plugin runtime accepts.
///
/// The module must be within [`MAX_MODULE_BYTES`] and valid for the runtime's feature set. This
/// compiles the module, so run it off the async
/// executor (see [`validate_wasm_module_blocking`]) when the bytes come from a request.
pub fn validate_wasm_module(wasm_bytes: &[u8]) -> Result<(), String> {
    ensure_module_size(wasm_bytes.len())?;
    let engine = build_engine()?;
    Module::new(&engine, wasm_bytes).map_err(|err| format!("invalid wasm module: {err}"))?;
    Ok(())
}

/// [`validate_wasm_module`] on a blocking thread, so compiling an uploaded module never stalls
/// an async worker thread.
pub async fn validate_wasm_module_blocking(wasm_bytes: Arc<[u8]>) -> Result<(), String> {
    ensure_module_size(wasm_bytes.len())?;
    tokio::task::spawn_blocking(move || validate_wasm_module(&wasm_bytes))
        .await
        .map_err(|err| format!("wasm validation task failed: {err}"))?
}

/// Refuses a module larger than [`MAX_MODULE_BYTES`].
pub fn ensure_module_size(len: usize) -> Result<(), String> {
    if len > MAX_MODULE_BYTES {
        Err(format!(
            "wasm module is {len} bytes; plugin modules are limited to {MAX_MODULE_BYTES} bytes"
        ))
    } else {
        Ok(())
    }
}

async fn invoke_on_engine(
    engine: &Engine,
    wasm_bytes: Vec<u8>,
    input: Value,
    policy: PluginRuntimePolicy,
) -> Result<PluginRuntimeOutput, PluginRuntimeError> {
    let started = Instant::now();
    let timeout = Duration::from_millis(policy.timeout_ms);
    let timeout_ms = policy.timeout_ms;
    ensure_module_size(wasm_bytes.len()).map_err(|message| PluginRuntimeError {
        kind: PluginFailureKind::Failed,
        message,
        duration_ms: millis_u64(started.elapsed()),
        fuel_consumed: None,
    })?;
    let flags = Arc::new(InvocationFlags::default());
    let task_engine = engine.clone();
    let task_flags = Arc::clone(&flags);
    let mut task = tokio::task::spawn_blocking(move || {
        invoke_wasm_plugin_sync(&task_engine, &wasm_bytes, &input, &policy, &task_flags)
    });
    // If this future is dropped before the guest returns (request cancelled, runtime shutting
    // down), the guard interrupts the guest so its blocking thread is released.
    let guard = InterruptOnDrop {
        engine: engine.clone(),
        flags: Arc::clone(&flags),
        armed: true,
    };

    if let Ok(joined) = tokio::time::timeout(timeout, &mut task).await {
        guard.disarm();
        return match joined {
            Ok(result) => result,
            Err(err) => Err(PluginRuntimeError {
                kind: PluginFailureKind::Failed,
                message: format!("wasm task failed: {err}"),
                duration_ms: millis_u64(started.elapsed()),
                fuel_consumed: None,
            }),
        };
    }

    flags.expired.store(true, Ordering::SeqCst);
    guard.disarm();
    let fuel_consumed = if flags.guest_started.load(Ordering::SeqCst) {
        // The guest's own report, discarded below, still says how much fuel it burnt.
        match interrupt_until_finished(engine, &mut task).await {
            Ok(Ok(output)) => {
                tracing::debug!(timeout_ms, "wasm plugin finished after its deadline; result discarded");
                output.fuel_consumed
            }
            Ok(Err(err)) => {
                tracing::debug!(timeout_ms, error = %err, "wasm plugin stopped after its deadline");
                err.fuel_consumed
            }
            Err(err) => {
                tracing::warn!(timeout_ms, error = %err, "wasm task failed after its deadline");
                None
            }
        }
    } else {
        // Still queued for a blocking thread or still compiling. No guest code has run and none
        // will: the task reads `expired` before it starts the guest and returns on its own.
        tracing::debug!(timeout_ms, "wasm plugin deadline passed before the guest started");
        None
    };
    Err(PluginRuntimeError {
        kind: PluginFailureKind::Timeout,
        message: timeout_error(timeout_ms),
        duration_ms: millis_u64(started.elapsed()),
        fuel_consumed,
    })
}

/// Advances the engine epoch until the blocking task returns, so a running guest whose
/// `expired` flag is set reaches its epoch deadline and is interrupted.
///
/// A single advance is not enough when the guest's store was created, or the guest last observed
/// the epoch, concurrently with that advance: its deadline is then one tick past the advanced
/// epoch. Advancing again every [`INTERRUPT_RETRY_INTERVAL`] reaches it.
async fn interrupt_until_finished<T>(engine: &Engine, task: &mut JoinHandle<T>) -> Result<T, JoinError> {
    loop {
        engine.increment_epoch();
        if let Ok(joined) = tokio::time::timeout(INTERRUPT_RETRY_INTERVAL, &mut *task).await {
            return joined;
        }
    }
}

struct InterruptOnDrop {
    engine: Engine,
    flags: Arc<InvocationFlags>,
    armed: bool,
}

impl InterruptOnDrop {
    fn disarm(mut self) {
        self.armed = false;
    }
}

impl Drop for InterruptOnDrop {
    fn drop(&mut self) {
        if self.armed {
            self.flags.expired.store(true, Ordering::SeqCst);
            self.engine.increment_epoch();
        }
    }
}

fn timeout_error(timeout_ms: u64) -> String {
    format!("wasm execution timeout after {timeout_ms}ms")
}

/// Compiles, instantiates and runs the plugin on the current (blocking) thread.
///
/// Every error from here is `Failed`: the timeout is decided by the caller alone, which discards
/// whatever this returns once the deadline has passed (only the fuel figure is kept).
fn invoke_wasm_plugin_sync(
    engine: &Engine,
    wasm_bytes: &[u8],
    input: &Value,
    policy: &PluginRuntimePolicy,
    flags: &Arc<InvocationFlags>,
) -> Result<PluginRuntimeOutput, PluginRuntimeError> {
    let started = Instant::now();
    // Before a store with fuel exists there is no cost to report.
    let without_store = |message: String| PluginRuntimeError {
        kind: PluginFailureKind::Failed,
        message,
        duration_ms: millis_u64(started.elapsed()),
        fuel_consumed: None,
    };
    if flags.expired.load(Ordering::SeqCst) {
        return Err(without_store(DEADLINE_PASSED.to_string()));
    }
    let module = Module::new(engine, wasm_bytes).map_err(|err| without_store(format!("invalid wasm module: {err}")))?;
    let mut store = new_plugin_store(engine, policy, flags).map_err(without_store)?;
    flags.guest_started.store(true, Ordering::SeqCst);

    let outcome = run_guest(&mut store, &module, input, flags);
    // Known from here on whatever the outcome: fuel exhaustion leaves zero remaining, a trap or
    // an interrupt leaves whatever the guest had not burnt yet.
    let fuel_consumed = store
        .get_fuel()
        .ok()
        .map(|remaining| policy.fuel.saturating_sub(remaining));
    let duration_ms = millis_u64(started.elapsed());
    match outcome {
        Ok(output) => Ok(PluginRuntimeOutput {
            output,
            duration_ms,
            fuel_consumed,
        }),
        Err(message) => Err(PluginRuntimeError {
            kind: PluginFailureKind::Failed,
            message,
            duration_ms,
            fuel_consumed,
        }),
    }
}

/// Why the blocking task stopped on its own; never shown, because the caller reports the timeout.
const DEADLINE_PASSED: &str = "the invocation deadline passed";

/// A store with the plugin's limits, fuel budget and epoch deadline.
///
/// The deadline is one tick past the epoch current at store creation. Reaching it only runs the
/// callback: it interrupts the guest when this invocation's own deadline has passed and otherwise
/// re-arms one tick past the then-current epoch, so an epoch advance made on behalf of anything
/// else can never stop this guest early.
fn new_plugin_store(
    engine: &Engine,
    policy: &PluginRuntimePolicy,
    flags: &Arc<InvocationFlags>,
) -> Result<Store<StoreState>, String> {
    let mut store = Store::new(
        engine,
        StoreState {
            limits: plugin_store_limits(policy),
        },
    );
    store.limiter(|state| &mut state.limits);
    store
        .set_fuel(policy.fuel)
        .map_err(|err| format!("failed to set wasm fuel: {err}"))?;
    store.set_epoch_deadline(1);
    let callback_flags = Arc::clone(flags);
    store.epoch_deadline_callback(move |_| {
        if callback_flags.expired.load(Ordering::SeqCst) {
            Ok(UpdateDeadline::Interrupt)
        } else {
            Ok(UpdateDeadline::Continue(1))
        }
    });
    Ok(store)
}

/// Instantiates the module in `store`, runs the ABI and reads the guest's JSON output.
fn run_guest(
    store: &mut Store<StoreState>,
    module: &Module,
    input: &Value,
    flags: &InvocationFlags,
) -> Result<Value, String> {
    let ensure_not_expired = || {
        if flags.expired.load(Ordering::SeqCst) {
            Err(DEADLINE_PASSED.to_string())
        } else {
            Ok(())
        }
    };

    ensure_not_expired()?;
    let instance =
        Instance::new(&mut *store, module, &[]).map_err(|err| format!("failed to instantiate wasm: {err:#}"))?;

    if let Ok(version_fn) = instance.get_typed_func::<(), i32>(&mut *store, "openpr_plugin_abi_version") {
        ensure_not_expired()?;
        let version = version_fn
            .call(&mut *store, ())
            .map_err(|err| format!("failed to read plugin abi version: {err:#}"))?;
        if version != ABI_VERSION {
            return Err(format!("unsupported plugin abi version: {version}"));
        }
    }

    let alloc = instance
        .get_typed_func::<i32, i32>(&mut *store, "openpr_alloc")
        .map_err(|_| "plugin must export openpr_alloc(len: i32) -> i32".to_string())?;
    let invoke = instance
        .get_typed_func::<(i32, i32), i64>(&mut *store, "openpr_invoke")
        .map_err(|_| "plugin must export openpr_invoke(ptr: i32, len: i32) -> i64".to_string())?;
    let memory = instance
        .get_memory(&mut *store, "memory")
        .ok_or_else(|| "plugin must export memory".to_string())?;

    let input_bytes = serde_json::to_vec(input).map_err(|err| format!("failed to encode plugin input: {err}"))?;
    let input_len = i32::try_from(input_bytes.len()).map_err(|_| "plugin input is too large".to_string())?;
    ensure_not_expired()?;
    let input_ptr = alloc
        .call(&mut *store, input_len)
        .map_err(|err| format!("plugin allocation failed: {err:#}"))?;
    let input_offset = usize::try_from(input_ptr).map_err(|_| "plugin returned negative input pointer".to_string())?;
    memory
        .write(&mut *store, input_offset, &input_bytes)
        .map_err(|err| format!("failed to write plugin input: {err}"))?;

    ensure_not_expired()?;
    let output_handle = invoke
        .call(&mut *store, (input_ptr, input_len))
        .map_err(|err| format!("plugin invocation trapped: {err:#}"))?;
    let (output_ptr, output_len) = unpack_ptr_len(output_handle)?;
    if output_len > MAX_OUTPUT_BYTES {
        return Err("plugin output is too large".to_string());
    }
    let mut output_bytes = vec![0_u8; output_len];
    memory
        .read(&*store, output_ptr, &mut output_bytes)
        .map_err(|err| format!("failed to read plugin output: {err}"))?;
    serde_json::from_slice(&output_bytes).map_err(|err| format!("plugin output is not JSON: {err}"))
}

/// The resource limits of a plugin store. Every growable resource a store can allocate is pinned
/// here rather than left at wasmtime's default, which for table elements is unlimited:
///
/// - one instance (the plugin itself; a plugin has no imports to instantiate anything else),
/// - one linear memory of at most `runtime.memory_bytes`,
/// - one table of at most [`MAX_TABLE_ELEMENTS`] elements, initial size included,
/// - a refused growth returns `-1` to the guest instead of trapping, as the core spec defines.
///
/// A GC heap, when a module uses one, is bounded by the same `memory_size` limit.
fn plugin_store_limits(policy: &PluginRuntimePolicy) -> StoreLimits {
    StoreLimitsBuilder::new()
        .memory_size(policy.memory_bytes)
        .table_elements(MAX_TABLE_ELEMENTS)
        .instances(1)
        .memories(1)
        .tables(1)
        .trap_on_grow_failure(false)
        .build()
}

fn build_engine() -> Result<Engine, String> {
    let mut config = Config::new();
    config.consume_fuel(true);
    // Wall-clock deadlines interrupt running guest code through epoch checks inserted at
    // function entries and loop back-edges; fuel stays the deterministic CPU budget.
    config.epoch_interruption(true);
    // Wasmtime 49 turned the wide-arithmetic proposal on by default. Keep the guest-visible
    // feature set identical to what plugins were validated against before the upgrade.
    config.wasm_wide_arithmetic(false);
    // Compile each module on the invocation's own blocking thread. Parallel compilation shares
    // one process-wide thread pool, so one tenant's large module would delay every other
    // tenant's compilation, and with it their deadlines.
    config.parallel_compilation(false);
    Engine::new(&config).map_err(|err| format!("failed to build wasm engine: {err}"))
}

fn unpack_ptr_len(handle: i64) -> Result<(usize, usize), String> {
    let raw = u64::try_from(handle).map_err(|_| "plugin returned negative output handle".to_string())?;
    let ptr = usize::try_from(raw >> 32).map_err(|_| "plugin output pointer is too large".to_string())?;
    let len = usize::try_from(raw & 0xffff_ffff).map_err(|_| "plugin output length is too large".to_string())?;
    Ok((ptr, len))
}

fn millis_u64(duration: Duration) -> u64 {
    u64::try_from(duration.as_millis()).unwrap_or(u64::MAX)
}

#[cfg(test)]
mod tests {
    use super::{
        InvocationFlags, MAX_MODULE_BYTES, MAX_TABLE_ELEMENTS, PluginFailureKind, PluginInvocationStatus,
        PluginRuntimePolicy, build_engine, interrupt_until_finished, invoke_on_engine, invoke_wasm_plugin,
        new_plugin_store, plugin_store_limits, validate_wasm_module,
    };
    use serde_json::json;
    use std::fmt::Write as _;
    use std::sync::{Arc, atomic::Ordering};
    use std::time::{Duration, Instant};

    fn echo_ok_wasm() -> Vec<u8> {
        wat::parse_str(
            r#"
            (module
              (memory (export "memory") 1)
              (global $heap (mut i32) (i32.const 4096))
              (data (i32.const 1024) "{\"ok\":true,\"total\":\"0.30\"}")
              (func (export "openpr_plugin_abi_version") (result i32)
                i32.const 1)
              (func (export "openpr_alloc") (param $len i32) (result i32)
                (local $ptr i32)
                global.get $heap
                local.set $ptr
                global.get $heap
                local.get $len
                i32.add
                global.set $heap
                local.get $ptr)
              (func (export "openpr_invoke") (param $ptr i32) (param $len i32) (result i64)
                i64.const 1024
                i64.const 32
                i64.shl
                i64.const 26
                i64.or))
            "#,
        )
        .expect("wat should compile")
    }

    #[tokio::test]
    async fn invokes_core_wasm_abi_and_reads_json_output() {
        let wasm = echo_ok_wasm();
        validate_wasm_module(&wasm).expect("module should validate");
        let output = invoke_wasm_plugin(
            wasm,
            json!({"amount": "0.30"}),
            PluginRuntimePolicy {
                timeout_ms: 500,
                fuel: 100_000,
                memory_bytes: 1024 * 1024,
            },
        )
        .await
        .expect("wasm should run");

        assert_eq!(output.output.get("ok").and_then(serde_json::Value::as_bool), Some(true));
        assert_eq!(
            output.output.get("total").and_then(serde_json::Value::as_str),
            Some("0.30")
        );
        assert!(output.fuel_consumed.unwrap_or(0) > 0);
    }

    #[tokio::test]
    async fn rejects_wasm_without_required_abi_exports() {
        let wasm = wat::parse_str("(module)").expect("wat should compile");
        let err = invoke_wasm_plugin(wasm, json!({}), PluginRuntimePolicy::default())
            .await
            .expect_err("missing abi should fail");

        assert!(err.message.contains("openpr_alloc"), "{err}");
        assert_eq!(err.kind, PluginFailureKind::Failed);
        // The store existed, so its (zero) fuel use is known.
        assert_eq!(err.fuel_consumed, Some(0), "{err:?}");
    }

    /// Bytes that never compile leave no store behind, so no fuel figure can be reported.
    #[tokio::test]
    async fn an_uncompilable_module_reports_no_fuel() {
        let err = invoke_wasm_plugin(b"not wasm".to_vec(), json!({}), PluginRuntimePolicy::default())
            .await
            .expect_err("garbage must not run");

        assert!(err.message.starts_with("invalid wasm module"), "{err}");
        assert_eq!(err.kind, PluginFailureKind::Failed);
        assert_eq!(err.fuel_consumed, None);
    }

    #[test]
    fn invocation_statuses_match_the_table_constraint() {
        assert_eq!(PluginInvocationStatus::Completed.as_str(), "completed");
        assert_eq!(
            PluginInvocationStatus::from(PluginFailureKind::Failed).as_str(),
            "failed"
        );
        assert_eq!(
            PluginInvocationStatus::from(PluginFailureKind::Timeout).as_str(),
            "timeout"
        );
        let migration = include_str!("../../../../migrations/0033_plugins_wasm.sql");
        assert!(
            migration.contains("CHECK (status IN ('completed', 'failed', 'timeout'))"),
            "plugin_invocations.status must admit every value the runtime records"
        );
    }

    #[tokio::test]
    async fn rejects_wasm_imports_so_plugins_have_no_host_io() {
        let wasm = wat::parse_str(
            r#"
            (module
              (import "wasi_snapshot_preview1" "fd_write" (func $fd_write))
              (memory (export "memory") 1)
              (func (export "openpr_alloc") (param i32) (result i32) i32.const 0)
              (func (export "openpr_invoke") (param i32) (param i32) (result i64)
                call $fd_write
                i64.const 0))
            "#,
        )
        .expect("wat should compile");
        let err = invoke_wasm_plugin(wasm, json!({}), PluginRuntimePolicy::default())
            .await
            .expect_err("imports should fail");

        assert!(err.message.contains("failed to instantiate wasm"), "{err}");
    }

    fn memory_grow_probe_wasm(initial_pages: u32) -> Vec<u8> {
        wat::parse_str(format!(
            r#"
            (module
              (memory (export "memory") {initial_pages})
              (data (i32.const 1024) "{{\"grow\":\"denied\"}}")
              (data (i32.const 2048) "{{\"grow\":\"allowed\"}}")
              (func (export "openpr_alloc") (param i32) (result i32) i32.const 4096)
              (func (export "openpr_invoke") (param i32) (param i32) (result i64)
                i32.const 4
                memory.grow
                i32.const -1
                i32.eq
                if (result i64)
                  i64.const 1024
                  i64.const 32
                  i64.shl
                  i64.const 17
                  i64.or
                else
                  i64.const 2048
                  i64.const 32
                  i64.shl
                  i64.const 18
                  i64.or
                end))
            "#
        ))
        .expect("wat should compile")
    }

    #[tokio::test]
    async fn memory_limit_denies_growth_beyond_policy() {
        let policy = PluginRuntimePolicy {
            timeout_ms: 500,
            fuel: 100_000,
            memory_bytes: 2 * 65_536,
        };
        let output = invoke_wasm_plugin(memory_grow_probe_wasm(1), json!({}), policy)
            .await
            .expect("probe should run");

        assert_eq!(
            output.output.get("grow").and_then(serde_json::Value::as_str),
            Some("denied")
        );
    }

    #[tokio::test]
    async fn memory_limit_allows_growth_within_policy() {
        let policy = PluginRuntimePolicy {
            timeout_ms: 500,
            fuel: 100_000,
            memory_bytes: 8 * 65_536,
        };
        let output = invoke_wasm_plugin(memory_grow_probe_wasm(1), json!({}), policy)
            .await
            .expect("probe should run");

        assert_eq!(
            output.output.get("grow").and_then(serde_json::Value::as_str),
            Some("allowed")
        );
    }

    #[tokio::test]
    async fn memory_limit_rejects_initial_memory_beyond_policy() {
        let policy = PluginRuntimePolicy {
            timeout_ms: 500,
            fuel: 100_000,
            memory_bytes: 2 * 65_536,
        };
        let err = invoke_wasm_plugin(memory_grow_probe_wasm(4), json!({}), policy)
            .await
            .expect_err("oversized initial memory should fail");

        assert!(err.message.contains("failed to instantiate wasm"), "{err}");
    }

    /// A module whose `openpr_invoke` grows its only table by `grow_by` elements and reports
    /// whether the growth was refused.
    fn table_grow_probe_wasm(initial_elements: u32, grow_by: u32) -> Vec<u8> {
        wat::parse_str(format!(
            r#"
            (module
              (memory (export "memory") 1)
              (table $t {initial_elements} funcref)
              (data (i32.const 1024) "{{\"grow\":\"denied\"}}")
              (data (i32.const 2048) "{{\"grow\":\"allowed\"}}")
              (func (export "openpr_alloc") (param i32) (result i32) i32.const 4096)
              (func (export "openpr_invoke") (param i32) (param i32) (result i64)
                ref.null func
                i32.const {grow_by}
                table.grow $t
                i32.const -1
                i32.eq
                if (result i64)
                  i64.const 1024
                  i64.const 32
                  i64.shl
                  i64.const 17
                  i64.or
                else
                  i64.const 2048
                  i64.const 32
                  i64.shl
                  i64.const 18
                  i64.or
                end))
            "#
        ))
        .expect("wat should compile")
    }

    /// Fuel is charged per table element, so the budget has to allow the growth to be attempted
    /// for the limiter, not fuel, to be what refuses it.
    const TABLE_PROBE_POLICY: PluginRuntimePolicy = PluginRuntimePolicy {
        timeout_ms: 30_000,
        fuel: 100_000_000,
        memory_bytes: 64 * 1024,
    };

    /// A guest must not be able to allocate host memory through table growth: before the table
    /// element limit, `table.grow` of this size succeeded and allocated about 8 bytes per element
    /// outside `runtime.memory_bytes` (the audit measured 2.3 GB of RSS for 300 M elements).
    #[tokio::test]
    async fn table_growth_beyond_the_element_limit_is_refused() {
        let grow_by = u32::try_from(MAX_TABLE_ELEMENTS * 16).expect("probe size fits in i32");
        let output = invoke_wasm_plugin(table_grow_probe_wasm(1, grow_by), json!({}), TABLE_PROBE_POLICY)
            .await
            .expect("probe should run");

        assert_eq!(
            output.output.get("grow").and_then(serde_json::Value::as_str),
            Some("denied")
        );
    }

    #[tokio::test]
    async fn table_growth_within_the_element_limit_is_allowed() {
        let output = invoke_wasm_plugin(table_grow_probe_wasm(1, 100), json!({}), TABLE_PROBE_POLICY)
            .await
            .expect("probe should run");

        assert_eq!(
            output.output.get("grow").and_then(serde_json::Value::as_str),
            Some("allowed")
        );
    }

    #[tokio::test]
    async fn an_initial_table_beyond_the_element_limit_is_rejected() {
        let initial = u32::try_from(MAX_TABLE_ELEMENTS + 1).expect("probe size fits in u32");
        let err = invoke_wasm_plugin(table_grow_probe_wasm(initial, 0), json!({}), TABLE_PROBE_POLICY)
            .await
            .expect_err("an oversized initial table must not instantiate");

        assert!(err.message.contains("failed to instantiate wasm"), "{err}");
    }

    /// The echo module plus one extra item (a second memory, a second table).
    fn echo_with_extra_wasm(extra: &str) -> Vec<u8> {
        wat::parse_str(format!(
            r#"
            (module
              (memory (export "memory") 1)
              {extra}
              (data (i32.const 1024) "{{\"ok\":true}}")
              (func (export "openpr_alloc") (param i32) (result i32) i32.const 4096)
              (func (export "openpr_invoke") (param i32) (param i32) (result i64)
                i64.const 1024
                i64.const 32
                i64.shl
                i64.const 11
                i64.or))
            "#
        ))
        .expect("wat should compile")
    }

    /// The engine accepts multi-memory modules, so the store limit is what keeps a plugin to one
    /// linear memory (and so to one `runtime.memory_bytes` allowance).
    #[tokio::test]
    async fn a_second_linear_memory_is_refused() {
        let err = invoke_wasm_plugin(
            echo_with_extra_wasm("(memory $second 1)"),
            json!({}),
            PluginRuntimePolicy::default(),
        )
        .await
        .expect_err("a second memory must not instantiate");

        assert!(err.message.contains("failed to instantiate wasm"), "{err}");
    }

    #[tokio::test]
    async fn a_second_table_is_refused() {
        let err = invoke_wasm_plugin(
            echo_with_extra_wasm("(table $first 1 funcref) (table $second 1 funcref)"),
            json!({}),
            PluginRuntimePolicy::default(),
        )
        .await
        .expect_err("a second table must not instantiate");

        assert!(err.message.contains("failed to instantiate wasm"), "{err}");
    }

    #[tokio::test]
    async fn one_table_and_one_memory_are_allowed() {
        let output = invoke_wasm_plugin(
            echo_with_extra_wasm("(table $only 1 funcref)"),
            json!({}),
            PluginRuntimePolicy::default(),
        )
        .await
        .expect("one memory and one table are within the limits");

        assert_eq!(output.output, json!({"ok": true}));
    }

    /// A store built from the plugin limits holds exactly one instance: a second instantiation in
    /// the same store is refused. The module has no memory or table, so only the instance count
    /// can refuse it.
    #[test]
    fn a_plugin_store_admits_a_single_instance() {
        let engine = build_engine().expect("engine should build");
        let module = wasmtime::Module::new(&engine, "(module (func (export \"f\")))").expect("module should compile");
        let mut store = wasmtime::Store::new(
            &engine,
            super::StoreState {
                limits: plugin_store_limits(&PluginRuntimePolicy::default()),
            },
        );
        store.limiter(|state| &mut state.limits);
        store.set_epoch_deadline(u64::MAX);
        wasmtime::Instance::new(&mut store, &module, &[]).expect("the first instance is within the limit");
        let second = wasmtime::Instance::new(&mut store, &module, &[]);

        assert!(second.is_err(), "a second instance must be refused");
    }

    /// A valid plugin of `functions` functions with long straight-line bodies: within the module
    /// size limit, but slow to compile.
    fn slow_to_compile_wasm(functions: usize) -> Vec<u8> {
        let mut body = String::new();
        for step in 0..400 {
            writeln!(
                body,
                "local.get 0 i32.const {step} i32.mul i32.const 3 i32.xor local.set 0"
            )
            .expect("writing to a String cannot fail");
        }
        let mut text = String::from(
            r#"(module
              (memory (export "memory") 1)
              (data (i32.const 1024) "{\"ok\":true}")
              (func (export "openpr_alloc") (param i32) (result i32) i32.const 4096)
              (func (export "openpr_invoke") (param i32) (param i32) (result i64)
                i64.const 1024
                i64.const 32
                i64.shl
                i64.const 11
                i64.or)
            "#,
        );
        for index in 0..functions {
            writeln!(
                text,
                "(func (export \"f{index}\") (param i32) (result i32)\n{body}local.get 0)"
            )
            .expect("writing to a String cannot fail");
        }
        text.push(')');
        wat::parse_str(text).expect("wat should compile")
    }

    /// The request-level deadline covers compilation: a module that takes far longer to compile
    /// than its `timeout_ms` returns the timeout at the deadline, not when compilation finishes.
    /// Before, the caller waited for the blocking task, so the wait was the compile time.
    #[test]
    fn the_deadline_covers_compilation() {
        let wasm = slow_to_compile_wasm(20);
        let compile_started = Instant::now();
        validate_wasm_module(&wasm).expect("the module is valid and within the size limit");
        let compile = compile_started.elapsed();
        assert!(
            compile >= Duration::from_millis(200),
            "the probe module must take well over the 20 ms deadline to compile (took {compile:?})"
        );

        let (result, waited) = run_on_bounded_runtime(1, async move {
            let started = Instant::now();
            let result = invoke_wasm_plugin(
                wasm,
                json!({}),
                PluginRuntimePolicy {
                    timeout_ms: 20,
                    fuel: 100_000,
                    memory_bytes: 1024 * 1024,
                },
            )
            .await;
            (result, started.elapsed())
        });

        let err = result.expect_err("compilation cannot finish within the deadline");
        assert_eq!(err.kind, PluginFailureKind::Timeout, "{err:?}");
        assert_eq!(err.message, "wasm execution timeout after 20ms");
        assert_eq!(err.fuel_consumed, None, "no guest ran: {err:?}");
        assert!(
            waited < compile / 2,
            "the timeout must arrive at the deadline, not after compilation \
             (waited {waited:?}, compile takes {compile:?})"
        );
    }

    /// With every blocking thread busy, a queued invocation still returns at its own deadline, and
    /// its guest never runs once a thread frees up: the single thread is available right after
    /// the busy guest is interrupted.
    #[test]
    fn a_queued_invocation_times_out_at_its_own_deadline_and_never_runs() {
        let bound = Duration::from_secs(10);
        let (busy, queued, released) = run_on_bounded_runtime(1, async move {
            let busy = tokio::spawn(invoke_wasm_plugin(
                spin_forever_wasm(),
                json!({}),
                PluginRuntimePolicy {
                    timeout_ms: 1_500,
                    fuel: u64::MAX,
                    memory_bytes: 1024 * 1024,
                },
            ));
            tokio::time::sleep(Duration::from_millis(100)).await;
            let started = Instant::now();
            let queued = invoke_wasm_plugin(
                spin_forever_wasm(),
                json!({}),
                PluginRuntimePolicy {
                    timeout_ms: 100,
                    fuel: u64::MAX,
                    memory_bytes: 1024 * 1024,
                },
            )
            .await;
            let queued = (queued, started.elapsed());
            let busy = tokio::time::timeout(bound, busy).await;
            let released = tokio::time::timeout(bound, tokio::task::spawn_blocking(|| 7_u8)).await;
            (busy, queued, released)
        });

        let (queued_result, queued_waited) = queued;
        let err = queued_result.expect_err("the queued guest cannot run before its deadline");
        assert_eq!(err.kind, PluginFailureKind::Timeout, "{err:?}");
        assert_eq!(err.fuel_consumed, None, "no guest ran: {err:?}");
        assert!(
            queued_waited < Duration::from_secs(1),
            "the queued invocation must not wait for the busy guest ({queued_waited:?})"
        );
        let busy = busy
            .expect("the busy invocation returns within the bound")
            .expect("the busy invocation task completes");
        assert_eq!(
            busy.expect_err("spins until interrupted").kind,
            PluginFailureKind::Timeout
        );
        let job = released.expect("the queued guest must not occupy the thread after its deadline");
        assert_eq!(job.expect("blocking job should complete"), 7);
    }

    /// The post-deadline loop keeps advancing the epoch until the task returns. A store created
    /// after the first advance has its deadline one tick past that advance; a single advance would
    /// leave its guest spinning forever.
    #[test]
    fn interrupting_reaches_a_store_created_after_the_first_advance() {
        let engine = build_engine().expect("engine should build");
        let module = wasmtime::Module::new(&engine, spin_forever_wasm()).expect("module should compile");
        let flags = Arc::new(InvocationFlags::default());
        flags.expired.store(true, Ordering::SeqCst);
        let task_engine = engine.clone();
        let outcome = run_on_bounded_runtime(2, async move {
            let mut task = tokio::task::spawn_blocking(move || {
                // Long enough for the first epoch advance to have happened.
                std::thread::sleep(Duration::from_millis(200));
                let policy = PluginRuntimePolicy {
                    fuel: u64::MAX,
                    ..PluginRuntimePolicy::default()
                };
                let mut store = new_plugin_store(&task_engine, &policy, &flags).expect("store");
                let instance = wasmtime::Instance::new(&mut store, &module, &[]).expect("instance");
                let invoke = instance
                    .get_typed_func::<(i32, i32), i64>(&mut store, "openpr_invoke")
                    .expect("export");
                invoke.call(&mut store, (0, 0))
            });
            tokio::time::timeout(Duration::from_secs(10), interrupt_until_finished(&engine, &mut task)).await
        });

        let call = outcome
            .expect("the guest must be interrupted within the bound")
            .expect("the task completes");
        let err = call.expect_err("the spinning guest only stops when interrupted");
        assert!(
            matches!(err.downcast_ref::<wasmtime::Trap>(), Some(wasmtime::Trap::Interrupt)),
            "{err:#}"
        );
    }

    #[tokio::test]
    async fn a_module_over_the_size_limit_is_refused_before_compiling() {
        let oversized = vec![0_u8; MAX_MODULE_BYTES + 1];
        let err = validate_wasm_module(&oversized).expect_err("an oversized module must be refused");
        assert!(err.contains("plugin modules are limited to 4194304 bytes"), "{err}");

        let err = invoke_wasm_plugin(oversized, json!({}), PluginRuntimePolicy::default())
            .await
            .expect_err("an oversized module must not run");
        assert_eq!(err.kind, PluginFailureKind::Failed);
        assert!(err.message.contains("plugin modules are limited"), "{err}");
        assert_eq!(err.fuel_consumed, None);
    }

    #[test]
    fn rejects_wide_arithmetic_proposal_modules() {
        let wasm = wat::parse_str(
            r"
            (module
              (func (param i64 i64 i64 i64) (result i64 i64)
                local.get 0
                local.get 1
                local.get 2
                local.get 3
                i64.add128))
            ",
        )
        .expect("wat should compile");
        let err = validate_wasm_module(&wasm).expect_err("wide-arithmetic must stay disabled");

        assert!(err.contains("invalid wasm module"));
    }

    fn spin_forever_wasm() -> Vec<u8> {
        wat::parse_str(
            r#"
            (module
              (memory (export "memory") 1)
              (func (export "openpr_alloc") (param i32) (result i32) i32.const 0)
              (func (export "openpr_invoke") (param i32) (param i32) (result i64)
                (loop $again
                  br $again)
                i64.const 0))
            "#,
        )
        .expect("wat should compile")
    }

    /// Counts down from `iterations` in a guest loop, then returns `{"done":true}`.
    fn count_down_wasm(iterations: u64) -> Vec<u8> {
        wat::parse_str(format!(
            r#"
            (module
              (memory (export "memory") 1)
              (data (i32.const 1024) "{{\"done\":true}}")
              (func (export "openpr_alloc") (param i32) (result i32) i32.const 4096)
              (func (export "openpr_invoke") (param i32) (param i32) (result i64)
                (local $n i64)
                i64.const {iterations}
                local.set $n
                (loop $again
                  local.get $n
                  i64.const 1
                  i64.sub
                  local.tee $n
                  i64.const 0
                  i64.gt_s
                  br_if $again)
                i64.const 1024
                i64.const 32
                i64.shl
                i64.const 13
                i64.or))
            "#
        ))
        .expect("wat should compile")
    }

    #[tokio::test]
    async fn fuel_limit_traps_runaway_plugins() {
        // The deadline is far beyond the time 10 units of fuel can take, so the only way this
        // guest stops is fuel exhaustion, and the error must say so rather than claim a timeout.
        let err = invoke_wasm_plugin(
            spin_forever_wasm(),
            json!({}),
            PluginRuntimePolicy {
                timeout_ms: 30_000,
                fuel: 10,
                memory_bytes: 1024 * 1024,
            },
        )
        .await
        .expect_err("fuel should trap");

        assert!(err.message.starts_with("plugin invocation trapped"), "{err}");
        assert!(err.message.contains("fuel"), "{err}");
        assert!(!err.message.contains("timeout"), "{err}");
        assert_eq!(err.kind, PluginFailureKind::Failed);
        assert_eq!(
            err.fuel_consumed,
            Some(10),
            "fuel exhaustion burns the whole budget: {err:?}"
        );
    }

    #[tokio::test]
    async fn guest_trap_is_reported_as_trap_not_timeout() {
        let wasm = wat::parse_str(
            r#"
            (module
              (memory (export "memory") 1)
              (func (export "openpr_alloc") (param i32) (result i32) i32.const 0)
              (func (export "openpr_invoke") (param i32) (param i32) (result i64)
                unreachable))
            "#,
        )
        .expect("wat should compile");
        let err = invoke_wasm_plugin(
            wasm,
            json!({}),
            PluginRuntimePolicy {
                timeout_ms: 30_000,
                fuel: 100_000,
                memory_bytes: 1024 * 1024,
            },
        )
        .await
        .expect_err("unreachable should trap");

        assert!(err.message.starts_with("plugin invocation trapped"), "{err}");
        assert!(!err.message.contains("timeout"), "{err}");
        assert!(!err.message.contains("fuel"), "{err}");
        assert_eq!(err.kind, PluginFailureKind::Failed);
        assert!(
            err.fuel_consumed.is_some(),
            "a trap leaves the store's fuel readable: {err:?}"
        );
    }

    /// Runs `future` on a dedicated runtime that is shut down with a timeout, so a guest that
    /// keeps spinning on a blocking thread after a failed assertion cannot hang test teardown
    /// (`#[tokio::test]` waits for blocking threads without a bound).
    fn run_on_bounded_runtime<F: std::future::Future>(max_blocking_threads: usize, future: F) -> F::Output {
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .max_blocking_threads(max_blocking_threads)
            .enable_all()
            .build()
            .expect("runtime should build");
        let output = runtime.block_on(future);
        runtime.shutdown_timeout(Duration::from_secs(1));
        output
    }

    /// A guest that never yields, with fuel that cannot run out in practice, must be stopped by
    /// the wall-clock deadline and must give its blocking thread back.
    ///
    /// The runtime has exactly one blocking thread. After the timeout error, a trivial blocking
    /// job has to get that thread; if the guest were still spinning on it the job would queue
    /// forever. Each wait has its own 10 s bound (50x the 200 ms deadline) so a regression fails
    /// by assertion instead of hanging, and the runtime is shut down with a timeout so a spinning
    /// guest cannot block test teardown either.
    #[test]
    fn deadline_interrupts_runaway_guest_and_releases_its_thread() {
        let bound = Duration::from_secs(10);
        let (invocation, released) = run_on_bounded_runtime(1, async {
            let invocation = tokio::time::timeout(
                bound,
                invoke_wasm_plugin(
                    spin_forever_wasm(),
                    json!({}),
                    PluginRuntimePolicy {
                        timeout_ms: 200,
                        fuel: u64::MAX,
                        memory_bytes: 1024 * 1024,
                    },
                ),
            )
            .await;
            let released = tokio::time::timeout(bound, tokio::task::spawn_blocking(|| 7_u8)).await;
            (invocation, released)
        });

        let result = invocation.expect("invocation must return within the bound once the deadline passes");
        let err = result.expect_err("runaway guest must not succeed");
        assert_eq!(err.message, "wasm execution timeout after 200ms");
        assert_eq!(err.kind, PluginFailureKind::Timeout);
        assert_eq!(PluginInvocationStatus::from(err.kind).as_str(), "timeout");
        assert!(err.duration_ms >= 200, "{err:?}");
        assert!(err.fuel_consumed.is_some_and(|fuel| fuel > 0), "{err:?}");
        let job = released.expect("the single blocking thread must be free after the timeout");
        assert_eq!(job.expect("blocking job should complete"), 7);
    }

    /// Dropping the invocation future before its deadline (a cancelled request, a runtime shutting
    /// down) must still interrupt the guest and free the single blocking thread. Bounds as in the
    /// deadline test.
    #[test]
    fn cancelled_invocation_interrupts_guest_and_releases_its_thread() {
        let bound = Duration::from_secs(10);
        let (cancelled, released) = run_on_bounded_runtime(1, async {
            let cancelled = tokio::time::timeout(
                Duration::from_millis(200),
                invoke_wasm_plugin(
                    spin_forever_wasm(),
                    json!({}),
                    PluginRuntimePolicy {
                        timeout_ms: 30_000,
                        fuel: u64::MAX,
                        memory_bytes: 1024 * 1024,
                    },
                ),
            )
            .await;
            let released = tokio::time::timeout(bound, tokio::task::spawn_blocking(|| 7_u8)).await;
            (cancelled, released)
        });

        assert!(cancelled.is_err(), "the caller must have dropped the invocation first");
        let job = released.expect("the single blocking thread must be free after the cancellation");
        assert_eq!(job.expect("blocking job should complete"), 7);
    }

    /// Two invocations share one engine, so the epoch advance issued at the short deadline
    /// reaches the long invocation's store too. The long one must keep running and finish.
    ///
    /// The long guest's loop takes about 1.5 s on an idle machine (measured) and the short
    /// deadline is 100 ms, so the short timeout fires while the long guest is still running; the
    /// test asserts that overlap instead of assuming it. The long deadline is the manifest
    /// maximum (30 s), leaving ample headroom on a loaded machine.
    #[test]
    fn epoch_advance_for_one_deadline_does_not_stop_another_invocation() {
        let engine = build_engine().expect("engine should build");
        let started = Instant::now();
        let short = async {
            let result = invoke_on_engine(
                &engine,
                spin_forever_wasm(),
                json!({}),
                PluginRuntimePolicy {
                    timeout_ms: 100,
                    fuel: u64::MAX,
                    memory_bytes: 1024 * 1024,
                },
            )
            .await;
            (result, started.elapsed())
        };
        let long = async {
            let result = invoke_on_engine(
                &engine,
                count_down_wasm(LONG_GUEST_ITERATIONS),
                json!({}),
                PluginRuntimePolicy {
                    timeout_ms: 30_000,
                    fuel: u64::MAX,
                    memory_bytes: 1024 * 1024,
                },
            )
            .await;
            (result, started.elapsed())
        };
        let ((short_result, short_done), (long_result, long_done)) = run_on_bounded_runtime(2, async {
            tokio::time::timeout(Duration::from_mins(1), async { tokio::join!(short, long) }).await
        })
        .expect("both invocations must finish within the bound");

        let short_err = short_result.expect_err("short invocation must time out");
        assert_eq!(short_err.message, "wasm execution timeout after 100ms");
        assert_eq!(short_err.kind, PluginFailureKind::Timeout);
        let output = long_result.expect("long invocation must not be stopped by the other deadline");
        assert_eq!(output.output, json!({"done": true}));
        assert!(
            long_done > short_done,
            "long guest must still have been running when the short deadline fired \
             (short {short_done:?}, long {long_done:?})"
        );
    }

    const LONG_GUEST_ITERATIONS: u64 = 2_000_000_000;
}
