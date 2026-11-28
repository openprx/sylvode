//! Which routes a plugin can reach (contract M10, section 3.1).
//!
//! The `PLUGIN` bit is computed, not listed: a route is plugin-reachable when it is a bot route, has
//! the risk `read`, `write` or `destructive`, has a scope the host can resolve, is not on one of the
//! closed lists below, is not a `purge`, and carries no `.no_plugin()` mark. `golden/plugin_ops.txt`
//! pins the result; any change of it needs an audit.

use super::types::{Entry, Exposure, PluginMark, Risk, Scope};

/// Areas none of whose ids a plugin can call.
///
/// Identity and instance administration, the plugin system itself (self-grant, supply chain, the
/// human-only approval boundary), the second plugin system and file serving.
pub const CLOSED_AREAS: [&str; 9] = [
    "account",
    "workspace",
    "system",
    "extensions",
    "packages",
    "market",
    "drafts",
    "legacy_plugins",
    "uploads",
];

/// `area.resource` pairs that are closed inside an otherwise open area.
///
/// Rules and sources route data to external channels or mint credentials; bots, webhooks,
/// notifications, transfer and uploads are outbound channels, secrets or bulk exfiltration; the task
/// protocol and runner belong to agent sessions.
pub const CLOSED_RESOURCES: [&str; 12] = [
    "alerts.rules",
    "alerts.sources",
    "alerts.inbound_events",
    "integrations.bots",
    "integrations.bot_logs",
    "integrations.webhooks",
    "integrations.transfer",
    "integrations.notifications",
    "integrations.uploads",
    "tasks.protocol",
    "tasks.runner",
    "tasks.change_hints",
];

/// Single operations closed inside an open resource.
///
/// Creating a task hands work to an agent session with its own rights. Of the id `tasks.tasks.read` only the issue and project lists are open; the
/// read of a task by id and the active-task read stay closed.
pub const CLOSED_OPS: [&str; 3] = ["tasks.tasks.create", "tasks.tasks.read", "tasks.tasks.read/active"];

/// The facet prefix of the task run protocol, closed as a group.
const CLOSED_OP_PREFIX: &str = "tasks.tasks.run/";

/// Whether the rule gives the entry the `PLUGIN` bit; ignores the bit it may already carry.
#[must_use]
pub fn reachable(entry: &Entry) -> bool {
    entry.exposure.contains(Exposure::BOT) && entry.plugin == PluginMark::Derived && allowed_by_rule(entry)
}

/// The rule without the mark: what a route must satisfy to carry `PLUGIN` at all.
#[must_use]
pub fn allowed_by_rule(entry: &Entry) -> bool {
    let Some((area, resource, verb)) = entry.id_parts() else {
        return false;
    };
    let pair = format!("{area}.{resource}");
    entry.exposure.contains(Exposure::BOT)
        && matches!(entry.risk, Risk::Read | Risk::Write | Risk::Destructive)
        && matches!(
            entry.scope,
            Scope::Project(_) | Scope::Global | Scope::Own | Scope::Filtered
        )
        && verb != "purge"
        && !CLOSED_AREAS.contains(&area)
        && !CLOSED_RESOURCES.contains(&pair.as_str())
        && !CLOSED_OPS.contains(&entry.op)
        && !entry.op.starts_with(CLOSED_OP_PREFIX)
}
