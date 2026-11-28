//! The plugin host: runs one component for one invocation under fixed limits (contract C1 §4-§5).
//!
//! [`invoke`] is the single entry. It refuses before the guest runs (size, rate, admission, an
//! import the host does not provide, the wrong world), runs the guest under fuel, an epoch deadline,
//! a memory limit and a host-call budget, maps every failure to a typed [`HostError`], and returns
//! the redacted [`InvocationRecord`] with the result.
//!
//! No database transaction is ever open while a guest runs: the host holds no transaction, and every
//! host call is one ordinary request to the router, which opens and closes its own (AUD-L1.1-F1).
//!
//! The caller passes a principal whose installation view holds the requested ids and the grants;
//! each host call loads them again, so a revoke or a narrowed grant applies at the next call.

pub mod bindings;
pub mod engine;
pub mod error;
pub mod limits;
pub mod log;
mod run;
mod state;
#[cfg(test)]
mod tests;

/// A state without a database, for tests that must not reach one.
#[cfg(test)]
pub(crate) fn test_state() -> AppState {
    tests::state()
}

use std::collections::BTreeSet;

use platform::app::AppState;
use serde_json::Value;

pub use error::HostError;
pub use limits::World;
pub use log::InvocationRecord;
pub use run::{Source, verify_component};

use crate::catalogue::{Grant, Principal};

/// The capabilities the installation's active generation asks for and the subset granted. The
/// effective rights of a call are the user's, narrowed to the grants, narrowed to the requests.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct GrantSet {
    pub requested: BTreeSet<String>,
    pub grants: Vec<Grant>,
}

/// The result of one invocation and its log record.
#[derive(Debug)]
pub struct Invocation {
    /// The output of the export, or the typed refusal.
    pub result: Result<Value, HostError>,
    pub record: InvocationRecord,
}

/// Runs `component` once.
///
/// * `component` is bytes (compiled through the cache) or a component compiled earlier.
/// * `principal` must be a plugin or an installation principal; its installation's `requested` and
///   `grants` are what the guest may use, and every host call loads them again.
/// * `world` picks the export and the limits; a component built for the other world is refused
///   `plugin_world_mismatch`.
/// * `entry` is the action name, or the rule stage of a field rule (`^[a-z0-9_.-]{1,64}$`).
/// * `input` is JSON: the `input` of an action, or the `rule-input` record of a field rule
///   (`{stage, field_key, field_type, value, deps: [[key, value]], config}`).
///
/// A `field-rule` links nothing, so it cannot call anything. An `action` is linked only to the
/// interfaces the grant set can use.
pub async fn invoke(
    state: &AppState,
    principal: &Principal,
    component: Source<'_>,
    world: World,
    entry: &str,
    input: &str,
) -> Invocation {
    run::invoke(state, principal, component, world, entry, input).await
}
