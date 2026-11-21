//! The configuration sections the API reads outside of `AppState`.
//!
//! [`AppState`](platform::app::AppState) carries the database handle and the handful of values
//! every request handler needs. Three sections do not fit there: object storage, the outbound
//! allowlist are read from code paths that are several calls away
//! from an `AppState`, and from free functions the worker also links against.
//!
//! They used to be read from the process environment at the point of use, which is precisely what
//! made them reachable from anywhere. This module keeps that reach without keeping the
//! environment: `main` installs the sections once, before the listener is bound, and
//! [`runtime`] hands out a shared borrow of them afterwards.
//!
//! "`main`" means every binary that links this library, not just the API server. The worker links
//! it too and runs the form import, export, attachment packaging and retention jobs, which build
//! their object storage from [`runtime`]; it installs its own configuration file at startup for
//! the same reason.
//!
use std::sync::{Arc, OnceLock};

use platform::config::{
    AiTasksConfig, DEFAULT_STORAGE_DIR, FlowConfig, InboundConfig, IpCidr, OAuthConfig, OpenPrConfig, OutboundConfig,
    StorageBackend, StorageConfig,
};

use crate::security::default_trusted_proxies;

/// The sections of the configuration file reached from outside `AppState`.
#[derive(Clone, Debug)]
pub struct RuntimeConfig {
    pub storage: StorageConfig,
    pub outbound: OutboundConfig,
    pub flow: FlowConfig,
    /// Lease and processing limits of AI tasks, shared with the worker's sweep.
    pub ai_tasks: AiTasksConfig,
    /// Peers allowed to vouch for a client address through `X-Forwarded-For` / `X-Real-IP`.
    pub trusted_proxies: Vec<IpCidr>,
    /// The authorization server settings, `None` when `[oauth] issuer` is not configured.
    pub oauth: Option<Arc<OAuthConfig>>,
    /// The alert chain's inbound endpoint settings (`[inbound]`, off by default).
    pub inbound: InboundConfig,
}

impl RuntimeConfig {
    /// Projects a validated configuration file onto the sections this module publishes.
    pub fn from_config(config: &OpenPrConfig) -> Self {
        Self {
            storage: config.storage.clone(),
            outbound: config.outbound.clone(),
            flow: config.flow.clone(),
            ai_tasks: config.ai_tasks,
            trusted_proxies: config
                .server
                .trusted_proxies
                .clone()
                .unwrap_or_else(default_trusted_proxies),
            oauth: config.oauth.clone().map(Arc::new),
            inbound: config.inbound,
        }
    }

    /// The settings a process that never called [`install`] runs with.
    ///
    /// Only unit tests reach this: every binary linking this library installs the file before it
    /// does any work, and a failure to do so aborts startup. The values are the safe end of each
    /// setting — local storage under [`DEFAULT_STORAGE_DIR`], an empty outbound allowlist with the
    /// private address checks on.
    ///
    /// Being the safe end is not the same as being correct: a binary that reaches these settings
    /// silently disagrees with the file the rest of the deployment reads. [`runtime`] seals the
    /// `OnceLock` on the first read, so the first such read is permanent and a later [`install`]
    /// reports an error rather than repairing it.
    fn fallback() -> Self {
        Self {
            storage: StorageConfig {
                backend: StorageBackend::Local,
                dir: DEFAULT_STORAGE_DIR.into(),
                s3: None,
            },
            outbound: OutboundConfig::default(),
            flow: FlowConfig::default(),
            ai_tasks: AiTasksConfig::default(),
            trusted_proxies: default_trusted_proxies(),
            oauth: None,
            inbound: InboundConfig::default(),
        }
    }
}

static RUNTIME: OnceLock<RuntimeConfig> = OnceLock::new();

/// Publishes the configuration file for the rest of the process.
///
/// Called once by `main`, before anything can serve a request. A second call is an error rather
/// than a silent no-op: two different configurations in one process would mean one half of the
/// service runs on settings the operator cannot see.
pub fn install(config: &OpenPrConfig) -> Result<(), String> {
    RUNTIME
        .set(RuntimeConfig::from_config(config))
        .map_err(|_| "configuration has already been installed for this process".to_string())
}

/// The installed configuration, or the fallback described on [`RuntimeConfig::fallback`].
pub fn runtime() -> &'static RuntimeConfig {
    RUNTIME.get_or_init(RuntimeConfig::fallback)
}

/// Whether the collab write window may decide incrementally (`[flow] incremental_window`, off
/// unless the file turns it on). Unit tests turn it on for their own thread with
/// [`test_support::enable_incremental_window`], so a test that exercises the incremental path says so.
pub fn incremental_window() -> bool {
    #[cfg(test)]
    if let Some(forced) = test_support::OVERRIDE.with(std::cell::Cell::get) {
        return forced;
    }
    runtime().flow.incremental_window
}

/// Per-thread overrides of settings for unit tests (every `#[tokio::test]` runs on one thread).
#[cfg(test)]
pub mod test_support {
    use std::cell::Cell;

    thread_local! {
        pub(super) static OVERRIDE: Cell<Option<bool>> = const { Cell::new(None) };
    }

    /// Holds `[flow] incremental_window = true` on this thread until it is dropped.
    pub struct IncrementalWindowOn;

    pub fn enable_incremental_window() -> IncrementalWindowOn {
        OVERRIDE.with(|forced| forced.set(Some(true)));
        IncrementalWindowOn
    }

    impl Drop for IncrementalWindowOn {
        fn drop(&mut self) {
            OVERRIDE.with(|forced| forced.set(None));
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::{RuntimeConfig, runtime};
    use platform::config::StorageBackend;

    #[test]
    fn the_fallback_is_the_safe_end_of_every_setting() {
        let fallback = RuntimeConfig::fallback();
        assert_eq!(fallback.storage.backend, StorageBackend::Local);
        assert!(fallback.storage.s3.is_none());
        assert!(!fallback.outbound.allow_private);
        assert!(fallback.outbound.allowlist_csv().is_empty());
    }

    #[test]
    fn runtime_is_readable_without_an_installed_file() {
        // The unit test process never calls `install`, so this exercises the fallback path the
        // rest of the test suite depends on.
        assert_eq!(runtime().storage.backend, StorageBackend::Local);
    }
}
