//! The route classes of a scoped bot (a runner bot or a project-scoped bot), one file per router area.
//!
//! One entry per method and path of `router/<area>.rs` behind the bot authentication. Kept by hand:
//! the test `every_router_route_is_classified` fails when a route is added to a router file
//! without an entry in the matching area file here, or an entry names a route that is gone.

use super::scoped_bot::{Class, RunnerRoute};

mod alerts;
mod auth_workspace;
mod catalogue;
mod drafts;
mod extensions;
mod flow;
mod forms;
mod governance;
mod integrations;
mod issues;
mod layouts;
mod packages;
mod planning;
mod projects;
mod site;
mod uploads;

/// Every area's table; a route belongs to the file named like its router area.
pub static RUNNER_BOT_ROUTES: &[&[RunnerRoute]] = &[
    alerts::ROUTES,
    auth_workspace::ROUTES,
    catalogue::ROUTES,
    drafts::ROUTES,
    extensions::ROUTES,
    flow::ROUTES,
    forms::ROUTES,
    governance::ROUTES,
    integrations::ROUTES,
    issues::ROUTES,
    layouts::ROUTES,
    packages::ROUTES,
    planning::ROUTES,
    projects::ROUTES,
    site::ROUTES,
    uploads::ROUTES,
];

const fn r(method: &'static str, pattern: &'static str, class: Class) -> RunnerRoute {
    RunnerRoute {
        method,
        pattern,
        class,
        comment: false,
    }
}

/// A route that comments (creates, edits, deletes or resolves a comment): a scoped bot needs the
/// `comment` level in the project, not `write`.
const fn rc(method: &'static str, pattern: &'static str, class: Class) -> RunnerRoute {
    RunnerRoute {
        method,
        pattern,
        class,
        comment: true,
    }
}

/// The number of routes per class name of one area's table.
#[cfg(test)]
fn class_counts(routes: &[RunnerRoute]) -> std::collections::BTreeMap<&'static str, usize> {
    let mut counts = std::collections::BTreeMap::new();
    for entry in routes {
        *counts.entry(entry.class.name()).or_default() += 1;
    }
    counts
}
