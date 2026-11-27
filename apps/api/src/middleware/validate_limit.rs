//! A per-principal limit for the validators (`POST` routes that only read): 60 calls a minute.
//!
//! The counter lives in the process, so with several replicas the effective limit is per replica.
//! Entries older than the window are dropped, and the table is bounded.

use std::collections::{HashMap, VecDeque};
use std::sync::LazyLock;
use std::time::{Duration, Instant};

use parking_lot::Mutex;

use crate::error::ApiError;

/// Calls per principal in one window.
pub const CALLS_PER_MINUTE: usize = 60;
const WINDOW: Duration = Duration::from_secs(60);
/// Principals kept before the table is swept of idle ones.
const SWEEP_AT: usize = 10_000;

static CALLS: LazyLock<Mutex<HashMap<String, VecDeque<Instant>>>> = LazyLock::new(|| Mutex::new(HashMap::new()));

fn allow(calls: &mut HashMap<String, VecDeque<Instant>>, principal: &str, now: Instant) -> bool {
    if calls.len() >= SWEEP_AT {
        calls.retain(|_, times| times.back().is_some_and(|last| now.duration_since(*last) < WINDOW));
    }
    let times = calls.entry(principal.to_owned()).or_default();
    while times.front().is_some_and(|first| now.duration_since(*first) >= WINDOW) {
        times.pop_front();
    }
    if times.len() >= CALLS_PER_MINUTE {
        return false;
    }
    times.push_back(now);
    true
}

/// Counts one validator call of `principal` (the token's subject: a user or a bot id).
///
/// # Errors
/// `429 rate_limited` when the principal already made 60 calls in the last minute.
pub fn check(principal: &str) -> Result<(), ApiError> {
    if allow(&mut CALLS.lock(), principal, Instant::now()) {
        return Ok(());
    }
    Err(ApiError::Coded {
        status: 429,
        code: "rate_limited",
        message: "too many validation calls in the last minute".to_owned(),
        details: None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_sixty_first_call_in_a_minute_is_refused_and_each_principal_has_its_own_budget() {
        let mut calls = HashMap::new();
        let start = Instant::now();
        for _ in 0..CALLS_PER_MINUTE {
            assert!(allow(&mut calls, "a", start));
        }
        assert!(!allow(&mut calls, "a", start));
        assert!(allow(&mut calls, "b", start));
    }

    #[test]
    fn the_budget_returns_when_the_window_has_passed() {
        let mut calls = HashMap::new();
        let start = Instant::now();
        for _ in 0..CALLS_PER_MINUTE {
            assert!(allow(&mut calls, "a", start));
        }
        assert!(allow(&mut calls, "a", start + WINDOW));
    }
}
