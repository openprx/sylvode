//! Drafts: specs written by agents (form schemas, layouts, site pages) that wait for a person to
//! review them.
//!
//! A draft changes nothing until a workspace owner or administrator approves it in the console;
//! approval runs the ordinary save of its kind with the approver's rights. Bots and connected
//! applications can create and read drafts and can never approve or reject them.

mod apply;
pub mod handlers;
mod prepare;
mod store;
