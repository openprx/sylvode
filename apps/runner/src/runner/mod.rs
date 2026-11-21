//! The `runner` agent: sessions of CLI agents in tmux, their messages and their guide.
//!
//! This module grows package by package. It holds the tmux driver, the message builder with its
//! title sanitiser, and the agent guide installer; the session manager that uses them follows.

#[allow(dead_code, reason = "used by the session manager, which is a later package")]
pub mod guide;
#[allow(dead_code, reason = "used by the session manager, which is a later package")]
pub mod message;
#[allow(dead_code, reason = "used by the message builder and the session manager")]
pub mod sanitize;
#[allow(dead_code, reason = "used by the session manager, which is a later package")]
pub mod tmux;

#[cfg(test)]
mod guide_tests;
#[cfg(test)]
mod message_tests;
#[cfg(test)]
mod sanitize_tests;
#[cfg(test)]
mod tmux_tests;
