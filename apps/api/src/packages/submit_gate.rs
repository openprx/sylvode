//! What a bot's package submissions may cost before anything is read (audit M11 F2).
//!
//! A submission buffers up to 32 MiB, copies it and verifies it while holding one of the two
//! instance-wide verification slots. So one submission per bot runs at a time, and every attempt
//! counts against a per-minute window whether it succeeds or not; both are checked before the body
//! is read. The state lives in this process: with several API processes each has its own window,
//! and the database caps (pending uploads, drafts per minute) still hold for all of them.

use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::LazyLock;
use std::time::{Duration, Instant};

use parking_lot::Mutex;
use uuid::Uuid;

use crate::error::ApiError;

/// Attempts (verified or not) one bot may start per minute.
const ATTEMPTS_PER_MINUTE: usize = 10;
const WINDOW: Duration = Duration::from_mins(1);

static IN_FLIGHT: LazyLock<Mutex<HashSet<Uuid>>> = LazyLock::new(|| Mutex::new(HashSet::new()));
static ATTEMPTS: LazyLock<Mutex<HashMap<Uuid, VecDeque<Instant>>>> = LazyLock::new(|| Mutex::new(HashMap::new()));

/// Holds the bot's single in-flight submission; released when dropped.
pub(super) struct Submission {
    bot: Uuid,
}

impl Drop for Submission {
    fn drop(&mut self) {
        IN_FLIGHT.lock().remove(&self.bot);
    }
}

fn busy(message: &'static str) -> ApiError {
    ApiError::Coded {
        status: 429,
        code: "rate_limited",
        message: message.to_owned(),
        details: None,
    }
}

/// Starts a submission of `bot`: refuses a second one in flight and an attempt over the window.
pub(super) fn begin(bot: Uuid) -> Result<Submission, ApiError> {
    begin_at(bot, Instant::now())
}

fn begin_at(bot: Uuid, now: Instant) -> Result<Submission, ApiError> {
    let mut attempts = ATTEMPTS.lock();
    attempts.retain(|_, times| times.back().is_some_and(|last| now.duration_since(*last) < WINDOW));
    let times = attempts.entry(bot).or_default();
    while times.front().is_some_and(|first| now.duration_since(*first) >= WINDOW) {
        times.pop_front();
    }
    if times.len() >= ATTEMPTS_PER_MINUTE {
        return Err(busy("too many package uploads in the last minute"));
    }
    if !IN_FLIGHT.lock().insert(bot) {
        return Err(busy("another package upload of this bot is still running"));
    }
    times.push_back(now);
    drop(attempts);
    Ok(Submission { bot })
}
