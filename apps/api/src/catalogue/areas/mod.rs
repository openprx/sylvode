//! One table of catalogue entries per router file; see `catalogue/mod.rs`.

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
mod system;
mod uploads;

use super::types::Entry;

/// Every area's table. A route belongs to the file named like its router file (`system` holds the
/// routes of `router/mod.rs`).
pub static TABLES: &[&[Entry]] = &[
    alerts::ENTRIES,
    auth_workspace::ENTRIES,
    catalogue::ENTRIES,
    drafts::ENTRIES,
    extensions::ENTRIES,
    flow::ENTRIES,
    forms::ENTRIES,
    governance::ENTRIES,
    integrations::ENTRIES,
    issues::ENTRIES,
    layouts::ENTRIES,
    packages::ENTRIES,
    planning::ENTRIES,
    projects::ENTRIES,
    site::ENTRIES,
    system::ENTRIES,
    uploads::ENTRIES,
];
