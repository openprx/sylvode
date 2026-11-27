//! DIY navigation: the stored `sv.nav.v1` documents of a workspace and of its projects, the
//! per-person preferences on top, and the resolution both menus are served from.
//!
//! The document rules and the resolution are pure (`doc`, `prefs`, `resolve`, `bot`); `refs` and
//! `store` read the database; `handlers` holds the routes and the access rules.

mod bot;
pub mod doc;
pub mod handlers;
pub mod prefs;
mod refs;
pub mod resolve;
mod store;
