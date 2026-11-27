//! Timing log events of the runner (info level). Each event is built from typed ids, numbers and
//! fixed words only: never a token, a secret, a URL, a request body or screen text can reach it.
//! Timings are whole milliseconds. The field sets are fixed by the parallel-delivery design (2.7).

use std::time::{Duration, Instant};

use serde_json::Value;

/// The longest id text logged (a task id is a UUID; anything longer is cut).
const ID_MAX: usize = 64;

/// A duration as whole milliseconds.
#[must_use]
pub fn millis(d: Duration) -> u64 {
    u64::try_from(d.as_millis()).unwrap_or(u64::MAX)
}

fn clip(text: &str) -> String {
    text.chars().take(ID_MAX).collect()
}

/// What `delivery answered` is built from.
#[derive(Debug, Default)]
pub struct DeliveryTimes {
    /// The slot that serves the task, when the checks got far enough to know it.
    pub slot: String,
    pub check_ms: u64,
    pub enqueue_ms: u64,
    pub total_ms: u64,
}

/// `delivery answered`: the runner's answer to one delivery. Only the task id, attempt and bot of
/// the payload and the status, reason code, channel and queue position of the answer are read; the
/// answer's free text and everything else in the payload are not.
pub fn delivery_answered(payload: &Value, answer: Option<&Value>, times: &DeliveryTimes) {
    let text = |value: &Value, key: &str| value.get(key).and_then(Value::as_str).map(clip).unwrap_or_default();
    let (status, reason_code, channel, position) = answer.map_or_else(
        || ("unavailable".to_string(), String::new(), String::new(), -1),
        |answer| {
            (
                text(answer, "status"),
                text(answer, "reason_code"),
                text(answer, "channel"),
                answer.get("queue_position").and_then(Value::as_i64).unwrap_or(-1),
            )
        },
    );
    let attempt = payload.get("attempts").and_then(Value::as_i64).unwrap_or(0);
    tracing::info!(
        task_id = %text(payload, "task_id"),
        attempt,
        bot = %text(payload, "ai_participant_id"),
        slot = %times.slot,
        channel = %channel,
        status = %status,
        reason_code = %reason_code,
        check_ms = times.check_ms,
        enqueue_ms = times.enqueue_ms,
        total_ms = times.total_ms,
        queue_position = position,
        "delivery answered"
    );
}

/// `task pushed`: a task was typed into its session.
pub fn task_pushed(task_id: &str, attempt: i64, slot: &str, queued: Duration, push: Duration, outcome: &str) {
    tracing::info!(
        task_id = %clip(task_id),
        attempt,
        slot = %slot,
        queued_ms = millis(queued),
        push_ms = millis(push),
        outcome = %outcome,
        "task pushed"
    );
}

/// `task left session`: the current task cleared; `end` is `terminal`, `superseded` or `gone`.
pub fn task_left_session(task_id: &str, attempt: i64, slot: &str, busy: Duration, end: &str) {
    tracing::info!(
        task_id = %clip(task_id),
        attempt,
        slot = %slot,
        busy_ms = millis(busy),
        end = %end,
        "task left session"
    );
}

/// `queue depth`: a session still holds queued tasks.
pub fn queue_depth(slot: &str, depth: usize, state: &str) {
    tracing::info!(slot = %slot, depth = depth as u64, state = %state, "queue depth");
}

/// Follows a session's current task and logs `task left session` when it clears or changes.
#[derive(Debug, Default)]
pub struct CurrentWatch {
    tracked: Option<(String, i64, Instant)>,
    hint: Option<&'static str>,
}

impl CurrentWatch {
    /// Says why the current task is about to clear when it is not because it ended (`superseded`
    /// or `gone`); used once, by the next change.
    pub const fn hint(&mut self, end: &'static str) {
        self.hint = Some(end);
    }

    /// `current` is the task now current (id, attempt, since when), if any.
    pub fn observe(&mut self, current: Option<(String, i64, Instant)>, now: Instant, slot: &str) {
        if self.tracked == current {
            return;
        }
        if let Some((id, attempt, since)) = self.tracked.take() {
            let end = self.hint.take().unwrap_or("terminal");
            task_left_session(&id, attempt, slot, now.saturating_duration_since(since), end);
        }
        self.hint = None;
        self.tracked = current;
    }
}

/// Logs `queue depth` at most once per `beat`, and only while the queue is not empty.
#[derive(Debug)]
pub struct DepthBeat {
    last: Instant,
}

impl DepthBeat {
    #[must_use]
    pub const fn new(now: Instant) -> Self {
        Self { last: now }
    }

    pub fn tick(&mut self, now: Instant, beat: Duration, slot: &str, depth: usize, state: &str) {
        if depth > 0 && now.saturating_duration_since(self.last) >= beat {
            self.last = now;
            queue_depth(slot, depth, state);
        }
    }
}
