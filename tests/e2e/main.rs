//! End-to-end tests: each one runs the real `sylvode-webhook` (or `openpr-webhook`) executable
//! against a generated configuration and observes it from outside, through HTTP, the tunnel
//! WebSocket, the requests it makes to a stub Sylvode API, the processes it starts and its output.

// Test helpers outside `#[test]` functions may fail loudly; this is test code only.
#![cfg(unix)]
#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic, clippy::indexing_slicing)]

mod cli;
mod startup;
mod support;
mod webhook;
