//! The `runner` agent: sessions of CLI agents in tmux, their messages and their guide.
//!
//! This module grows package by package. It starts with the tmux driver.

#[allow(dead_code, reason = "used by the session manager, which is a later package")]
pub mod tmux;

#[cfg(test)]
mod tmux_tests;
