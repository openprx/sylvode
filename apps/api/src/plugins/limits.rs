//! Containment of the old plugin runtime: tighter limits than the manifest may ask for, and a
//! concurrency cap per plugin (audit SF-7). The old system is removed with the new host.

use std::collections::HashMap;
use std::sync::{Arc, LazyLock};

use tokio::sync::{OwnedSemaphorePermit, Semaphore};
use uuid::Uuid;

use super::manifest::PluginRuntimePolicy;
use crate::error::ApiError;

/// The longest run of an old-system plugin.
pub const MAX_TIMEOUT_MS: u64 = 2_000;
/// The most fuel of an old-system run.
pub const MAX_FUEL: u64 = 50_000_000;
/// The most linear memory of an old-system run.
pub const MAX_MEMORY_BYTES: usize = 64 * 1024 * 1024;
/// Concurrent runs of one old-system plugin.
pub const MAX_CONCURRENT_PER_PLUGIN: usize = 4;

/// The manifest's limits, capped at the containment limits.
#[must_use]
pub fn clamp(policy: PluginRuntimePolicy) -> PluginRuntimePolicy {
    PluginRuntimePolicy {
        timeout_ms: policy.timeout_ms.min(MAX_TIMEOUT_MS),
        fuel: policy.fuel.min(MAX_FUEL),
        memory_bytes: policy.memory_bytes.min(MAX_MEMORY_BYTES),
    }
}

static GATES: LazyLock<parking_lot::Mutex<HashMap<Uuid, Arc<Semaphore>>>> =
    LazyLock::new(|| parking_lot::Mutex::new(HashMap::new()));

/// A permit to run `plugin_id`, held for the whole run.
///
/// # Errors
/// `plugin_concurrency_limit` (429) while [`MAX_CONCURRENT_PER_PLUGIN`] runs are active.
pub fn try_acquire(plugin_id: Uuid) -> Result<OwnedSemaphorePermit, ApiError> {
    let gate = {
        let mut gates = GATES.lock();
        // A gate nobody holds is dropped, so the map does not grow with the plugins ever run.
        gates.retain(|_, gate| Arc::strong_count(gate) > 1);
        Arc::clone(
            gates
                .entry(plugin_id)
                .or_insert_with(|| Arc::new(Semaphore::new(MAX_CONCURRENT_PER_PLUGIN))),
        )
    };
    gate.try_acquire_owned().map_err(|_| ApiError::Coded {
        status: 429,
        code: "plugin_concurrency_limit",
        message: "too many concurrent runs of this plugin".to_string(),
        details: None,
    })
}

#[cfg(test)]
mod tests {
    use super::{MAX_CONCURRENT_PER_PLUGIN, MAX_FUEL, MAX_MEMORY_BYTES, MAX_TIMEOUT_MS, clamp, try_acquire};
    use crate::plugins::manifest::PluginRuntimePolicy;
    use uuid::Uuid;

    #[test]
    fn limits_are_capped_and_smaller_ones_kept() {
        let clamped = clamp(PluginRuntimePolicy {
            timeout_ms: 30_000,
            fuel: 1_000_000_000,
            memory_bytes: 128 * 1024 * 1024,
        });
        assert_eq!(
            (clamped.timeout_ms, clamped.fuel, clamped.memory_bytes),
            (MAX_TIMEOUT_MS, MAX_FUEL, MAX_MEMORY_BYTES)
        );
        let small = clamp(PluginRuntimePolicy {
            timeout_ms: 500,
            fuel: 1_000,
            memory_bytes: 65_536,
        });
        assert_eq!((small.timeout_ms, small.fuel, small.memory_bytes), (500, 1_000, 65_536));
    }

    /// The fifth concurrent run of one plugin is refused; another plugin is not affected; a finished run frees its slot.
    #[test]
    fn concurrency_is_capped_per_plugin() {
        let plugin = Uuid::new_v4();
        let held: Vec<_> = (0..MAX_CONCURRENT_PER_PLUGIN)
            .filter_map(|_| try_acquire(plugin).ok())
            .collect();
        assert_eq!(held.len(), MAX_CONCURRENT_PER_PLUGIN);
        assert!(try_acquire(plugin).is_err());
        assert!(try_acquire(Uuid::new_v4()).is_ok());
        drop(held);
        assert!(try_acquire(plugin).is_ok());
    }
}
