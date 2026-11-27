//! The shared engine, its epoch clock and the in-memory compile cache (contract C1 §5).

use std::sync::{Arc, LazyLock};
use std::time::Duration;

use hashlink::LruCache;
use parking_lot::Mutex;
use sha2::{Digest, Sha256};
use tokio::sync::{OnceCell, Semaphore};
use wasmtime::component::Component;
use wasmtime::{Config, Engine};

use super::error::HostError;
use super::limits::{COMPILE_CACHE_ENTRIES, COMPILE_CONCURRENCY, COMPILE_TIMEOUT};
use crate::catalogue::RefusalCode;

/// One tick of the epoch clock; a deadline is a number of ticks.
pub const EPOCH_TICK: Duration = Duration::from_millis(10);
/// The wasm stack of a guest.
const WASM_STACK: usize = 512 * 1024;

/// The ticks that cover `wall`, rounded up.
#[must_use]
pub fn deadline_ticks(wall: Duration) -> u64 {
    let tick = EPOCH_TICK.as_millis();
    u64::try_from(wall.as_millis().div_ceil(tick))
        .unwrap_or(u64::MAX)
        .max(1)
}

static ENGINE: LazyLock<Result<Engine, String>> = LazyLock::new(|| {
    let mut config = Config::new();
    config
        .wasm_component_model(true)
        .consume_fuel(true)
        .epoch_interruption(true)
        .max_wasm_stack(WASM_STACK)
        .wasm_threads(false)
        .wasm_gc(false)
        .wasm_function_references(false)
        .wasm_stack_switching(false)
        .wasm_component_model_async(false);
    let engine = Engine::new(&config).map_err(|error| error.to_string())?;
    // The clock runs on its own thread, not on the runtime: tests start and stop runtimes while the
    // engine lives for the whole process.
    let ticking = engine.clone();
    std::thread::Builder::new()
        .name("ext-epoch".to_string())
        .spawn(move || {
            loop {
                std::thread::sleep(EPOCH_TICK);
                ticking.increment_epoch();
            }
        })
        .map_err(|error| error.to_string())?;
    Ok(engine)
});

/// The process-wide engine.
///
/// # Errors
/// `plugin_failed` when the engine cannot be built.
pub fn engine() -> Result<&'static Engine, HostError> {
    ENGINE.as_ref().map_err(|error| {
        tracing::error!(%error, "the extension engine could not start");
        HostError::new(RefusalCode::PluginFailed)
    })
}

type Cell = Arc<OnceCell<Component>>;

/// Compiled components by the SHA-256 of their bytes; at most [`COMPILE_CACHE_ENTRIES`]. In memory
/// only: loading compiled artefacts is `unsafe` and the workspace denies `unsafe`, so a restart
/// recompiles.
pub struct CompileCache {
    entries: Mutex<LruCache<[u8; 32], Cell>>,
    compiling: Arc<Semaphore>,
}

impl CompileCache {
    #[must_use]
    pub fn new(entries: usize, concurrency: usize) -> Self {
        Self {
            entries: Mutex::new(LruCache::new(entries)),
            compiling: Arc::new(Semaphore::new(concurrency)),
        }
    }

    /// The compiled form of `bytes`. Concurrent calls for the same bytes compile once. A compile
    /// runs on the blocking pool, at most [`COMPILE_CONCURRENCY`] at a time, and is abandoned after
    /// [`COMPILE_TIMEOUT`] (the permit stays with the thread until it ends, so the bound holds).
    ///
    /// # Errors
    /// `plugin_invalid_component` for bytes that do not compile, or a wait or compile past the timeout.
    pub async fn get(&self, engine: &Engine, bytes: &[u8]) -> Result<Component, HostError> {
        let invalid = || HostError::new(RefusalCode::PluginInvalidComponent);
        let digest: [u8; 32] = Sha256::digest(bytes).into();
        let cell = {
            let mut entries = self.entries.lock();
            if let Some(cell) = entries.get(&digest) {
                Arc::clone(cell)
            } else {
                let cell = Cell::default();
                entries.insert(digest, Arc::clone(&cell));
                cell
            }
        };
        let compiled = cell
            .get_or_try_init(|| async {
                // The wait for a compile slot is bounded like the compile itself, and no instance or
                // memory slot is held while waiting (admission comes after the compile).
                let permit = tokio::time::timeout(COMPILE_TIMEOUT, Arc::clone(&self.compiling).acquire_owned())
                    .await
                    .map_err(|_| invalid())?
                    .map_err(|_| invalid())?;
                let engine = engine.clone();
                let bytes = bytes.to_vec();
                let task = tokio::task::spawn_blocking(move || {
                    let result = Component::new(&engine, &bytes);
                    drop(permit);
                    result
                });
                match tokio::time::timeout(COMPILE_TIMEOUT, task).await {
                    Ok(Ok(Ok(component))) => Ok(component),
                    _ => Err(invalid()),
                }
            })
            .await
            .cloned();
        if compiled.is_err() {
            // A failed compile is not cached; the next call tries again.
            self.entries.lock().remove(&digest);
        }
        compiled
    }

    #[cfg(test)]
    pub fn len(&self) -> usize {
        self.entries.lock().len()
    }
}

/// The compile cache of this process.
pub fn compile_cache() -> &'static CompileCache {
    static CACHE: LazyLock<CompileCache> =
        LazyLock::new(|| CompileCache::new(COMPILE_CACHE_ENTRIES, COMPILE_CONCURRENCY));
    &CACHE
}

#[cfg(test)]
#[allow(clippy::expect_used)]
mod tests {
    use std::time::Duration;

    use super::{CompileCache, deadline_ticks, engine};

    #[test]
    fn deadlines_round_up_to_ticks() {
        assert_eq!(deadline_ticks(Duration::from_millis(50)), 5);
        assert_eq!(deadline_ticks(Duration::from_millis(55)), 6);
        assert_eq!(deadline_ticks(Duration::from_secs(2)), 200);
        assert_eq!(deadline_ticks(Duration::ZERO), 1);
    }

    #[tokio::test]
    async fn bytes_that_are_not_a_component_are_refused_and_not_cached() {
        let cache = CompileCache::new(4, 2);
        let error = cache
            .get(engine().expect("engine"), b"not wasm")
            .await
            .expect_err("garbage");
        assert_eq!(error.code, crate::catalogue::RefusalCode::PluginInvalidComponent);
        assert_eq!(cache.len(), 0);
    }

    #[tokio::test]
    async fn a_component_compiles_once_and_is_served_from_the_cache() {
        let cache = CompileCache::new(4, 2);
        let engine = engine().expect("engine");
        let bytes = wat::parse_str("(component)").expect("wat");
        cache.get(engine, &bytes).await.expect("first");
        cache.get(engine, &bytes).await.expect("second");
        assert_eq!(cache.len(), 1);
    }
}
