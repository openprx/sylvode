//! Catalogue entries of the `extensions` router area (`router/extensions.rs`).

use crate::catalogue::types::dsl::*;

#[rustfmt::skip]
pub static ENTRIES: &[Entry] = &[
    // Temporary development route of the plugin host; removed when installations land.
    console("extensions.dev.invoke", Post, "/api/v1/admin/extensions/dev-invoke", InstanceAdmin, Admin),
];
