//! Catalogue entries of the `system` router area (`router/mod.rs (the `/health` and `/ready` routes)`).

use crate::catalogue::types::dsl::*;

#[rustfmt::skip]
pub static ENTRIES: &[Entry] = &[
    console("system.health.read", Get, "/health", Public, Read),
    console("system.health.read/ready", Get, "/ready", Public, Read),
];
