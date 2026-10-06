//! Refuses a signed `/webhook` request that is replayed.
//!
//! A signature proves who wrote a body, not when it was sent, so a captured request could be sent
//! again to dispatch the same agent again. Two checks run on signed requests only, using fields
//! inside the signed body:
//!
//! - **Freshness.** A webhook delivery carries `timestamp` (RFC 3339). A delivery whose timestamp
//!   is further than `security.max_delivery_age_secs` from now, either way, is refused.
//! - **Uniqueness.** A webhook delivery carries a unique `id`; a worker AI task carries `task_id`
//!   and `attempts` (a retry of the task has a new attempt number). A key seen before, within the
//!   remembered window, is refused.
//!
//! A body without these fields (not sent by Sylvode) passes both checks.

use serde_json::Value;
use sha2::{Digest, Sha256};
use std::collections::{HashMap, VecDeque};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

/// The most delivery keys remembered; the oldest are forgotten first.
const CAPACITY: usize = 10_000;
/// How long a key is remembered at least, even with a short freshness window or none.
const MIN_REMEMBER: Duration = Duration::from_secs(600);

/// Why a request was refused.
#[derive(Debug, PartialEq, Eq)]
pub enum Refusal {
    /// The delivery's `timestamp` is `skew_secs` away from now, more than allowed.
    Stale { skew_secs: u64 },
    /// The delivery key was seen before. `key` names it for the log: its kind and the start of
    /// its digest.
    Duplicate { key: String },
}

#[derive(Debug)]
pub struct ReplayGuard {
    max_age: Option<Duration>,
    remember: Duration,
    seen: HashMap<Key, Instant>,
    order: VecDeque<(Instant, Key)>,
}

impl ReplayGuard {
    /// `max_age_secs = 0` turns the freshness check off; uniqueness is always checked.
    pub fn new(max_age_secs: u64) -> Self {
        let max_age = (max_age_secs > 0).then(|| Duration::from_secs(max_age_secs));
        Self {
            max_age,
            remember: max_age.map_or(MIN_REMEMBER, |age| (age * 2).max(MIN_REMEMBER)),
            seen: HashMap::new(),
            order: VecDeque::new(),
        }
    }

    /// Checks `payload` and, when it passes, remembers its key.
    pub fn check(&mut self, payload: &Value) -> Result<(), Refusal> {
        self.check_at(payload, Instant::now(), unix_now())
    }

    fn check_at(&mut self, payload: &Value, now: Instant, unix_now: i64) -> Result<(), Refusal> {
        if let Some(max_age) = self.max_age
            && let Some(sent) = payload.get("timestamp").and_then(Value::as_str).and_then(parse_rfc3339)
        {
            let skew_secs = unix_now.abs_diff(sent);
            if skew_secs > max_age.as_secs() {
                return Err(Refusal::Stale { skew_secs });
            }
        }
        self.forget_expired(now);
        let Some((kind, key)) = delivery_key(payload) else {
            return Ok(());
        };
        if self.seen.contains_key(&key) {
            let short = hex::encode(key.get(..8).unwrap_or_default());
            return Err(Refusal::Duplicate {
                key: format!("{kind}:{short}"),
            });
        }
        if self.order.len() >= CAPACITY
            && let Some((_, oldest)) = self.order.pop_front()
        {
            self.seen.remove(&oldest);
        }
        self.seen.insert(key, now);
        self.order.push_back((now, key));
        Ok(())
    }

    fn forget_expired(&mut self, now: Instant) {
        while let Some((at, _)) = self.order.front() {
            if now.saturating_duration_since(*at) < self.remember {
                break;
            }
            if let Some((_, key)) = self.order.pop_front() {
                self.seen.remove(&key);
            }
        }
    }
}

/// A remembered delivery key: the SHA-256 digest of its fields, so every key takes 32 bytes
/// however long the fields are.
type Key = [u8; 32];

/// The key that identifies one delivery, with its kind: `id` of a webhook delivery, or `task_id`
/// and `attempts` of a worker AI task. Each field is hashed with its length in front, so no two
/// different field values give the same input.
fn delivery_key(payload: &Value) -> Option<(&'static str, Key)> {
    let digest = |kind: &str, fields: &[&[u8]]| -> Key {
        let mut hasher = Sha256::new();
        hasher.update(kind.as_bytes());
        for field in fields {
            hasher.update((field.len() as u64).to_be_bytes());
            hasher.update(field);
        }
        hasher.finalize().into()
    };
    if let Some(task_id) = payload.get("task_id").and_then(Value::as_str) {
        let attempts = payload.get("attempts").map_or_else(String::new, ToString::to_string);
        return Some(("task", digest("task", &[task_id.as_bytes(), attempts.as_bytes()])));
    }
    payload
        .get("id")
        .and_then(Value::as_str)
        .filter(|id| !id.is_empty())
        .map(|id| ("delivery", digest("delivery", &[id.as_bytes()])))
}

fn unix_now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .ok()
        .and_then(|d| i64::try_from(d.as_secs()).ok())
        .unwrap_or_default()
}

/// Parses `YYYY-MM-DDTHH:MM:SS[.fraction](Z|±HH:MM)` into Unix seconds.
fn parse_rfc3339(text: &str) -> Option<i64> {
    let (date, rest) = text.split_once(['T', 't', ' '])?;
    let mut date_parts = date.splitn(3, '-');
    let year: i64 = date_parts.next()?.parse().ok()?;
    let month: i64 = date_parts.next()?.parse().ok()?;
    let day: i64 = date_parts.next()?.parse().ok()?;

    let (clock, offset_secs) = if let Some(clock) = rest.strip_suffix(['Z', 'z']) {
        (clock, 0)
    } else {
        let split = rest.rfind(['+', '-'])?;
        let (clock, offset) = rest.split_at(split);
        let sign = if offset.starts_with('-') { -1 } else { 1 };
        let (hours, minutes) = offset.get(1..)?.split_once(':')?;
        let hours: i64 = hours.parse().ok()?;
        let minutes: i64 = minutes.parse().ok()?;
        (clock, sign * (hours * 3600 + minutes * 60))
    };
    let clock = clock.split_once('.').map_or(clock, |(whole, _)| whole);
    let mut clock_parts = clock.splitn(3, ':');
    let hour: i64 = clock_parts.next()?.parse().ok()?;
    let minute: i64 = clock_parts.next()?.parse().ok()?;
    let second: i64 = clock_parts.next()?.parse().ok()?;
    let valid = (1..=12).contains(&month)
        && (1..=31).contains(&day)
        && (0..24).contains(&hour)
        && (0..60).contains(&minute)
        && (0..=60).contains(&second);
    if !valid {
        return None;
    }
    Some(days_from_civil(year, month, day) * 86_400 + hour * 3600 + minute * 60 + second - offset_secs)
}

/// Days since 1970-01-01 of a proleptic Gregorian date (Howard Hinnant's algorithm).
const fn days_from_civil(year: i64, month: i64, day: i64) -> i64 {
    let y = if month <= 2 { year - 1 } else { year };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let mp = (month + 9) % 12;
    let doy = (153 * mp + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn parses_the_timestamps_sylvode_writes() {
        assert_eq!(parse_rfc3339("1970-01-01T00:00:00Z"), Some(0));
        assert_eq!(parse_rfc3339("2026-10-05T12:00:00+00:00"), Some(1_791_201_600));
        assert_eq!(
            parse_rfc3339("2026-10-05T12:00:00.123456789+00:00"),
            Some(1_791_201_600)
        );
        assert_eq!(parse_rfc3339("2026-10-05T14:00:00+02:00"), Some(1_791_201_600));
        assert_eq!(parse_rfc3339("2026-10-05T07:30:00-04:30"), Some(1_791_201_600));
        assert_eq!(parse_rfc3339("2024-02-29T00:00:00Z"), Some(1_709_164_800));
        for bad in [
            "",
            "yesterday",
            "2026-13-01T00:00:00Z",
            "2026-10-05",
            "2026-10-05T25:00:00Z",
        ] {
            assert_eq!(parse_rfc3339(bad), None, "{bad:?}");
        }
    }

    fn delivery(id: &str, timestamp: &str) -> Value {
        json!({"id": id, "event": "issue.assigned", "timestamp": timestamp})
    }

    const NOW: i64 = 1_791_201_600; // 2026-10-05T12:00:00Z

    #[test]
    fn a_fresh_delivery_passes_once_and_its_replay_is_refused() {
        let mut guard = ReplayGuard::new(300);
        let now = Instant::now();
        let body = delivery("d-1", "2026-10-05T12:00:00+00:00");
        assert_eq!(guard.check_at(&body, now, NOW), Ok(()));
        assert_eq!(
            guard.check_at(&body, now, NOW + 1),
            Err(Refusal::Duplicate {
                key: format!("delivery:{}", hex::encode(&delivery_key(&body).expect("key").1[..8]))
            })
        );
        assert_eq!(
            guard.check_at(&delivery("d-2", "2026-10-05T12:00:00+00:00"), now, NOW),
            Ok(())
        );
    }

    #[test]
    fn a_delivery_outside_the_window_either_way_is_refused() {
        let mut guard = ReplayGuard::new(300);
        let now = Instant::now();
        assert_eq!(
            guard.check_at(&delivery("old", "2026-10-05T11:54:59+00:00"), now, NOW),
            Err(Refusal::Stale { skew_secs: 301 })
        );
        assert_eq!(
            guard.check_at(&delivery("future", "2026-10-05T12:05:01+00:00"), now, NOW),
            Err(Refusal::Stale { skew_secs: 301 })
        );
        assert_eq!(
            guard.check_at(&delivery("edge", "2026-10-05T11:55:00+00:00"), now, NOW),
            Ok(())
        );
    }

    #[test]
    fn a_zero_window_turns_only_freshness_off() {
        let mut guard = ReplayGuard::new(0);
        let now = Instant::now();
        let ancient = delivery("a", "2001-01-01T00:00:00Z");
        assert_eq!(guard.check_at(&ancient, now, NOW), Ok(()));
        assert!(matches!(
            guard.check_at(&ancient, now, NOW),
            Err(Refusal::Duplicate { .. })
        ));
    }

    #[test]
    fn worker_tasks_are_keyed_by_task_and_attempt() {
        let mut guard = ReplayGuard::new(300);
        let now = Instant::now();
        let first = json!({"task_id": "t-1", "attempts": 1});
        assert_eq!(guard.check_at(&first, now, NOW), Ok(()));
        assert!(matches!(
            guard.check_at(&first, now, NOW),
            Err(Refusal::Duplicate { .. })
        ));
        assert_eq!(
            guard.check_at(&json!({"task_id": "t-1", "attempts": 2}), now, NOW),
            Ok(())
        );
    }

    #[test]
    fn keys_are_forgotten_after_the_window_and_beyond_capacity() {
        let mut guard = ReplayGuard::new(300);
        let start = Instant::now();
        let body = json!({"id": "d-1"});
        assert_eq!(guard.check_at(&body, start, NOW), Ok(()));
        assert_eq!(guard.check_at(&body, start + Duration::from_secs(601), NOW), Ok(()));

        for n in 0..=CAPACITY {
            assert_eq!(guard.check_at(&json!({"id": format!("n-{n}")}), start, NOW), Ok(()));
        }
        assert!(guard.seen.len() <= CAPACITY);
    }

    /// A key is remembered for twice the freshness window (at least ten minutes): a delivery that
    /// is still fresh enough to pass the timestamp check is always still remembered.
    #[test]
    fn keys_are_remembered_for_twice_a_long_freshness_window() {
        let mut guard = ReplayGuard::new(1000);
        let start = Instant::now();
        let body = json!({"task_id": "t-1", "attempts": 1});
        assert_eq!(guard.check_at(&body, start, NOW), Ok(()));
        assert!(matches!(
            guard.check_at(&body, start + Duration::from_secs(1999), NOW),
            Err(Refusal::Duplicate { .. })
        ));
        assert_eq!(guard.check_at(&body, start + Duration::from_secs(2000), NOW), Ok(()));

        let mut short = ReplayGuard::new(10);
        assert_eq!(short.check_at(&body, start, NOW), Ok(()));
        assert!(matches!(
            short.check_at(&body, start + Duration::from_secs(599), NOW),
            Err(Refusal::Duplicate { .. })
        ));
        assert_eq!(short.check_at(&body, start + Duration::from_secs(600), NOW), Ok(()));
    }

    /// The memory never holds more than `CAPACITY` keys, and the oldest goes first.
    #[test]
    fn the_memory_is_bounded_and_forgets_the_oldest_key_first() {
        let mut guard = ReplayGuard::new(300);
        let now = Instant::now();
        for n in 0..CAPACITY {
            assert_eq!(guard.check_at(&json!({"id": format!("n-{n}")}), now, NOW), Ok(()));
        }
        assert_eq!(guard.seen.len(), CAPACITY);
        assert_eq!(guard.check_at(&json!({"id": "one-more"}), now, NOW), Ok(()));
        assert_eq!(guard.seen.len(), CAPACITY);
        assert_eq!(guard.order.len(), CAPACITY);
        assert_eq!(
            guard.check_at(&json!({"id": "n-0"}), now, NOW),
            Ok(()),
            "the oldest was forgotten"
        );
        assert!(matches!(
            guard.check_at(&json!({"id": "n-2"}), now, NOW),
            Err(Refusal::Duplicate { .. })
        ));
    }

    /// A remembered key takes the same small space however long the identifying fields are: the
    /// fields come from an authenticated sender, but a body may be 2 MB, and 10,000 such keys would
    /// otherwise hold gigabytes.
    #[test]
    fn a_remembered_key_has_a_fixed_size() {
        let mut guard = ReplayGuard::new(300);
        let now = Instant::now();
        let long = "x".repeat(100_000);
        for body in [
            json!({"id": long}),
            json!({"task_id": long, "attempts": 1}),
            json!({"id": format!("{long}y")}),
        ] {
            assert_eq!(guard.check_at(&body, now, NOW), Ok(()));
            assert!(matches!(
                guard.check_at(&body, now, NOW),
                Err(Refusal::Duplicate { .. })
            ));
        }
        let stored: usize = guard.seen.keys().map(|key| key.len()).sum();
        assert!(stored <= 3 * 32, "{stored} bytes of keys stored");
    }

    #[test]
    fn a_body_without_identifying_fields_passes() {
        let mut guard = ReplayGuard::new(300);
        let body = json!({"event": "issue.created"});
        assert_eq!(guard.check(&body), Ok(()));
        assert_eq!(guard.check(&body), Ok(()));
    }
}
