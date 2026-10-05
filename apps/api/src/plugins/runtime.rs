use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use serde_json::Value;
use wasmtime::{Config, Engine, Instance, Module, Store, StoreLimits, StoreLimitsBuilder, Trap, UpdateDeadline};

use super::manifest::PluginRuntimePolicy;

const ABI_VERSION: i32 = 1;
const MAX_OUTPUT_BYTES: usize = 1024 * 1024;
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

#[derive(Debug)]
struct StoreState {
    limits: StoreLimits,
}

/// Runs a plugin with its wall-clock deadline enforced by epoch interruption.
///
/// Every invocation builds its own engine, so the epoch advances issued here only ever reach this
/// invocation's store. The store deadline is still expressed relative to the current epoch and the
/// epoch callback only interrupts once this invocation's own `expired` flag is set, so the
/// mechanism stays isolated even if an engine is later shared between invocations.
pub async fn invoke_wasm_plugin(
    wasm_bytes: Vec<u8>,
    input: Value,
    policy: PluginRuntimePolicy,
) -> Result<PluginRuntimeOutput, String> {
    let engine = build_engine()?;
    invoke_on_engine(&engine, wasm_bytes, input, policy).await
}

pub fn validate_wasm_module(wasm_bytes: &[u8]) -> Result<(), String> {
    let engine = build_engine()?;
    Module::new(&engine, wasm_bytes).map_err(|err| format!("invalid wasm module: {err}"))?;
    Ok(())
}

async fn invoke_on_engine(
    engine: &Engine,
    wasm_bytes: Vec<u8>,
    input: Value,
    policy: PluginRuntimePolicy,
) -> Result<PluginRuntimeOutput, String> {
    let timeout = Duration::from_millis(policy.timeout_ms);
    let timeout_ms = policy.timeout_ms;
    let expired = Arc::new(AtomicBool::new(false));
    let task_engine = engine.clone();
    let task_expired = Arc::clone(&expired);
    let mut task = tokio::task::spawn_blocking(move || {
        invoke_wasm_plugin_sync(&task_engine, &wasm_bytes, &input, &policy, &task_expired)
    });
    // If this future is dropped before the guest returns (request cancelled, runtime shutting
    // down), the guard interrupts the guest so its blocking thread is released.
    let guard = InterruptOnDrop {
        engine: engine.clone(),
        expired: Arc::clone(&expired),
        armed: true,
    };

    if let Ok(joined) = tokio::time::timeout(timeout, &mut task).await {
        guard.disarm();
        return match joined {
            Ok(result) => result,
            Err(err) => Err(format!("wasm task failed: {err}")),
        };
    }

    expired.store(true, Ordering::SeqCst);
    let late = loop {
        engine.increment_epoch();
        if let Ok(joined) = tokio::time::timeout(INTERRUPT_RETRY_INTERVAL, &mut task).await {
            break joined;
        }
    };
    guard.disarm();
    match late {
        Ok(Ok(_)) => tracing::debug!(timeout_ms, "wasm plugin finished after its deadline; result discarded"),
        Ok(Err(err)) => tracing::debug!(timeout_ms, error = %err, "wasm plugin stopped after its deadline"),
        Err(err) => tracing::warn!(timeout_ms, error = %err, "wasm task failed after its deadline"),
    }
    Err(timeout_error(timeout_ms))
}

struct InterruptOnDrop {
    engine: Engine,
    expired: Arc<AtomicBool>,
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
            self.expired.store(true, Ordering::SeqCst);
            self.engine.increment_epoch();
        }
    }
}

fn timeout_error(timeout_ms: u64) -> String {
    format!("wasm execution timeout after {timeout_ms}ms")
}

fn invoke_wasm_plugin_sync(
    engine: &Engine,
    wasm_bytes: &[u8],
    input: &Value,
    policy: &PluginRuntimePolicy,
    expired: &Arc<AtomicBool>,
) -> Result<PluginRuntimeOutput, String> {
    let started = Instant::now();
    let module = Module::new(engine, wasm_bytes).map_err(|err| format!("invalid wasm module: {err}"))?;
    let limits = StoreLimitsBuilder::new()
        .memory_size(policy.memory_bytes)
        .instances(1)
        .memories(1)
        .tables(1)
        .build();
    let mut store = Store::new(engine, StoreState { limits });
    store.limiter(|state| &mut state.limits);
    store
        .set_fuel(policy.fuel)
        .map_err(|err| format!("failed to set wasm fuel: {err}"))?;
    // The deadline is one tick past the epoch current at store creation. Reaching it only runs
    // the callback: it interrupts the guest when this invocation's own deadline has passed and
    // otherwise re-arms one tick past the then-current epoch, so an epoch advance made on behalf
    // of anything else can never stop this guest early.
    store.set_epoch_deadline(1);
    let callback_expired = Arc::clone(expired);
    store.epoch_deadline_callback(move |_| {
        if callback_expired.load(Ordering::SeqCst) {
            Ok(UpdateDeadline::Interrupt)
        } else {
            Ok(UpdateDeadline::Continue(1))
        }
    });
    let guest_error = |context: &str, err: wasmtime::Error| {
        if matches!(err.downcast_ref::<Trap>(), Some(Trap::Interrupt)) {
            timeout_error(policy.timeout_ms)
        } else {
            format!("{context}: {err:#}")
        }
    };
    let ensure_not_expired = || {
        if expired.load(Ordering::SeqCst) {
            Err(timeout_error(policy.timeout_ms))
        } else {
            Ok(())
        }
    };

    ensure_not_expired()?;
    let instance =
        Instance::new(&mut store, &module, &[]).map_err(|err| guest_error("failed to instantiate wasm", err))?;

    if let Ok(version_fn) = instance.get_typed_func::<(), i32>(&mut store, "openpr_plugin_abi_version") {
        ensure_not_expired()?;
        let version = version_fn
            .call(&mut store, ())
            .map_err(|err| guest_error("failed to read plugin abi version", err))?;
        if version != ABI_VERSION {
            return Err(format!("unsupported plugin abi version: {version}"));
        }
    }

    let alloc = instance
        .get_typed_func::<i32, i32>(&mut store, "openpr_alloc")
        .map_err(|_| "plugin must export openpr_alloc(len: i32) -> i32".to_string())?;
    let invoke = instance
        .get_typed_func::<(i32, i32), i64>(&mut store, "openpr_invoke")
        .map_err(|_| "plugin must export openpr_invoke(ptr: i32, len: i32) -> i64".to_string())?;
    let memory = instance
        .get_memory(&mut store, "memory")
        .ok_or_else(|| "plugin must export memory".to_string())?;

    let input_bytes = serde_json::to_vec(input).map_err(|err| format!("failed to encode plugin input: {err}"))?;
    let input_len = i32::try_from(input_bytes.len()).map_err(|_| "plugin input is too large".to_string())?;
    ensure_not_expired()?;
    let input_ptr = alloc
        .call(&mut store, input_len)
        .map_err(|err| guest_error("plugin allocation failed", err))?;
    let input_offset = usize::try_from(input_ptr).map_err(|_| "plugin returned negative input pointer".to_string())?;
    memory
        .write(&mut store, input_offset, &input_bytes)
        .map_err(|err| format!("failed to write plugin input: {err}"))?;

    ensure_not_expired()?;
    let output_handle = invoke
        .call(&mut store, (input_ptr, input_len))
        .map_err(|err| guest_error("plugin invocation trapped", err))?;
    let (output_ptr, output_len) = unpack_ptr_len(output_handle)?;
    if output_len > MAX_OUTPUT_BYTES {
        return Err("plugin output is too large".to_string());
    }
    let mut output_bytes = vec![0_u8; output_len];
    memory
        .read(&store, output_ptr, &mut output_bytes)
        .map_err(|err| format!("failed to read plugin output: {err}"))?;
    let output = serde_json::from_slice(&output_bytes).map_err(|err| format!("plugin output is not JSON: {err}"))?;
    let fuel_remaining = store.get_fuel().ok();

    Ok(PluginRuntimeOutput {
        output,
        duration_ms: millis_u64(started.elapsed()),
        fuel_consumed: fuel_remaining.map(|remaining| policy.fuel.saturating_sub(remaining)),
    })
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
    use super::{PluginRuntimePolicy, build_engine, invoke_on_engine, invoke_wasm_plugin, validate_wasm_module};
    use serde_json::json;
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

        assert!(err.contains("openpr_alloc"));
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

        assert!(err.contains("failed to instantiate wasm"));
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

        assert!(err.contains("failed to instantiate wasm"));
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

        assert!(err.starts_with("plugin invocation trapped"), "{err}");
        assert!(err.contains("fuel"), "{err}");
        assert!(!err.contains("timeout"), "{err}");
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

        assert!(err.starts_with("plugin invocation trapped"), "{err}");
        assert!(!err.contains("timeout"), "{err}");
        assert!(!err.contains("fuel"), "{err}");
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
        assert_eq!(
            result.expect_err("runaway guest must not succeed"),
            "wasm execution timeout after 200ms"
        );
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

        assert_eq!(
            short_result.expect_err("short invocation must time out"),
            "wasm execution timeout after 100ms"
        );
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
