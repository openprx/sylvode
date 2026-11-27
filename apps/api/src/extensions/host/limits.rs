//! The fixed limits of an invocation and the process-wide gates (contract C1 §5, MF-5).

use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, LazyLock};
use std::time::{Duration, Instant};

use parking_lot::Mutex;
use tokio::sync::{OwnedSemaphorePermit, Semaphore};
use uuid::Uuid;

use super::error::HostError;
use crate::catalogue::RefusalCode;

const KIB: usize = 1024;
const MIB: usize = 1024 * KIB;

/// The two worlds the host links.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum World {
    /// A pure rule: no imports at all.
    FieldRule,
    /// An action: records, catalogue, log, clock and context.
    Action,
}

impl World {
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::FieldRule => "field-rule",
            Self::Action => "action",
        }
    }

    #[must_use]
    pub const fn limits(self) -> Limits {
        match self {
            Self::FieldRule => Limits {
                wall: Duration::from_millis(50),
                fuel: 2_000_000,
                memory: 8 * MIB,
                host_calls: 0,
                input: 64 * KIB,
                output: 64 * KIB,
                component: 2 * MIB,
                per_installation: 8,
            },
            Self::Action => Limits {
                wall: Duration::from_secs(2),
                fuel: 50_000_000,
                memory: 64 * MIB,
                host_calls: 200,
                input: 256 * KIB,
                output: MIB,
                component: 8 * MIB,
                per_installation: 4,
            },
        }
    }
}

/// Limits of one world; fixed in the MVP (no manifest or admin override).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Limits {
    pub wall: Duration,
    pub fuel: u64,
    /// Linear memory of the instance, in bytes.
    pub memory: usize,
    /// Dispatching host calls per invocation; call N+1 ends the invocation.
    pub host_calls: u32,
    pub input: usize,
    pub output: usize,
    pub component: usize,
    pub per_installation: usize,
}

/// A request or response of a host call.
pub const CALL_BODY_MAX: usize = MIB;
/// Counted guest log lines; more are dropped.
pub const LOG_LINES_MAX: u32 = 50;
/// Instances running at once, all installations.
pub const PROCESS_INSTANCES: usize = 32;
/// Linear memory reserved by running instances, all installations, in MiB.
pub const PROCESS_MEMORY_MIB: usize = 1024;
/// Compilations at once and the time one may take.
pub const COMPILE_CONCURRENCY: usize = 2;
pub const COMPILE_TIMEOUT: Duration = Duration::from_secs(10);
/// Invocations per user in the window (the invoke route, contract §5).
pub const USER_RATE: usize = 30;
pub const USER_RATE_WINDOW: Duration = Duration::from_secs(10);
/// Components kept compiled.
pub const COMPILE_CACHE_ENTRIES: usize = 64;

/// What a running invocation holds; released on drop.
#[derive(Debug)]
pub struct Admission {
    _instance: OwnedSemaphorePermit,
    _memory: OwnedSemaphorePermit,
    _installation: OwnedSemaphorePermit,
}

/// The semaphores and counters that bound the host as a whole.
pub struct Gates {
    instances: Arc<Semaphore>,
    memory_mib: Arc<Semaphore>,
    installations: Mutex<HashMap<(Uuid, World), Arc<Semaphore>>>,
    users: Mutex<HashMap<Uuid, VecDeque<Instant>>>,
}

impl Gates {
    #[must_use]
    pub fn new(instances: usize, memory_mib: usize) -> Self {
        Self {
            instances: Arc::new(Semaphore::new(instances)),
            memory_mib: Arc::new(Semaphore::new(memory_mib)),
            installations: Mutex::new(HashMap::new()),
            users: Mutex::new(HashMap::new()),
        }
    }

    /// Takes an instance slot, the world's memory budget and an installation slot, or refuses
    /// `plugin_concurrency_limit`. Nothing queues: a full gate refuses at once.
    ///
    /// # Errors
    /// `plugin_concurrency_limit`.
    pub fn admit(&self, world: World, installation: Uuid) -> Result<Admission, HostError> {
        let busy = || HostError::new(RefusalCode::PluginConcurrencyLimit);
        let limits = world.limits();
        let per_installation = {
            let mut map = self.installations.lock();
            map.retain(|_, gate| Arc::strong_count(gate) > 1 || gate.available_permits() < limits.per_installation);
            Arc::clone(
                map.entry((installation, world))
                    .or_insert_with(|| Arc::new(Semaphore::new(limits.per_installation))),
            )
        };
        let installation_permit = per_installation.try_acquire_owned().map_err(|_| busy())?;
        let instance = Arc::clone(&self.instances).try_acquire_owned().map_err(|_| busy())?;
        let mib = u32::try_from(limits.memory / MIB).map_err(|_| busy())?;
        let memory = Arc::clone(&self.memory_mib)
            .try_acquire_many_owned(mib)
            .map_err(|_| busy())?;
        Ok(Admission {
            _instance: instance,
            _memory: memory,
            _installation: installation_permit,
        })
    }

    /// Counts one invocation of `user` in the window, or refuses `plugin_concurrency_limit`.
    ///
    /// # Errors
    /// `plugin_concurrency_limit` (HTTP 429 at the route).
    pub fn rate(&self, user: Uuid, now: Instant) -> Result<(), HostError> {
        let mut users = self.users.lock();
        users.retain(|_, hits| {
            hits.back()
                .is_some_and(|last| now.duration_since(*last) < USER_RATE_WINDOW)
        });
        let hits = users.entry(user).or_default();
        while hits
            .front()
            .is_some_and(|first| now.duration_since(*first) >= USER_RATE_WINDOW)
        {
            hits.pop_front();
        }
        if hits.len() >= USER_RATE {
            return Err(HostError::new(RefusalCode::PluginConcurrencyLimit));
        }
        hits.push_back(now);
        Ok(())
    }
}

/// The gates of this process.
pub fn gates() -> &'static Gates {
    static GATES: LazyLock<Gates> = LazyLock::new(|| Gates::new(PROCESS_INSTANCES, PROCESS_MEMORY_MIB));
    &GATES
}

#[cfg(test)]
#[allow(clippy::expect_used)]
mod tests {
    use std::time::{Duration, Instant};

    use uuid::Uuid;

    use super::{Gates, USER_RATE, World};
    use crate::catalogue::RefusalCode;

    #[test]
    fn the_instance_gate_refuses_instead_of_queueing() {
        let gates = Gates::new(2, 1024);
        let first = gates.admit(World::Action, Uuid::from_u128(1)).expect("first");
        let _second = gates.admit(World::Action, Uuid::from_u128(2)).expect("second");
        let refused = gates.admit(World::Action, Uuid::from_u128(3)).expect_err("third");
        assert_eq!(refused.code, RefusalCode::PluginConcurrencyLimit);
        drop(first);
        gates
            .admit(World::Action, Uuid::from_u128(3))
            .expect("a slot is free again");
    }

    #[test]
    fn the_memory_budget_is_reserved_per_world() {
        // 100 MiB hold one action (64) and four field rules (8 each) but not two actions.
        let gates = Gates::new(32, 100);
        let _action = gates.admit(World::Action, Uuid::from_u128(1)).expect("action");
        let refused = gates
            .admit(World::Action, Uuid::from_u128(2))
            .expect_err("second action");
        assert_eq!(refused.code, RefusalCode::PluginConcurrencyLimit);
        let rules: Vec<_> = (10..14)
            .map(|n| gates.admit(World::FieldRule, Uuid::from_u128(n)).expect("rule"))
            .collect();
        assert_eq!(rules.len(), 4);
        assert!(gates.admit(World::FieldRule, Uuid::from_u128(20)).is_err());
    }

    #[test]
    fn one_installation_has_its_own_ceiling() {
        let gates = Gates::new(32, 1024);
        let installation = Uuid::from_u128(7);
        let held: Vec<_> = (0..4)
            .map(|_| gates.admit(World::Action, installation).expect("within the ceiling"))
            .collect();
        assert!(gates.admit(World::Action, installation).is_err());
        assert!(gates.admit(World::Action, Uuid::from_u128(8)).is_ok());
        assert_eq!(held.len(), 4);
    }

    #[test]
    fn the_user_rate_is_a_sliding_window() {
        let gates = Gates::new(32, 1024);
        let user = Uuid::from_u128(5);
        let start = Instant::now();
        for _ in 0..USER_RATE {
            gates.rate(user, start).expect("within the rate");
        }
        assert!(gates.rate(user, start).is_err());
        assert!(gates.rate(Uuid::from_u128(6), start).is_ok());
        gates
            .rate(user, start + Duration::from_secs(11))
            .expect("the window moved on");
    }
}
