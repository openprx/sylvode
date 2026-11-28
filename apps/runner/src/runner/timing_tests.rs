#![allow(clippy::indexing_slicing)]

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use parking_lot::Mutex;
use serde_json::json;
use tracing::field::{Field, Visit};
use tracing::{Event, Subscriber};
use tracing_subscriber::layer::{Context, SubscriberExt};
use tracing_subscriber::{Layer, Registry};

use super::machine::State;
use super::session::{PushTry, pushed_word, state_word};
use super::timing::{self, CurrentWatch, DeliveryTimes, DepthBeat};

const TOKEN: &str = "svt_MADE_UP_TOKEN_0123456789";
const URL: &str = "https://sylvode.example.invalid/api/v1/x?token=MADE_UP_QUERY";

type Captured = Arc<Mutex<Vec<(String, BTreeMap<String, String>)>>>;

struct Fields(BTreeMap<String, String>);

impl Visit for Fields {
    fn record_debug(&mut self, field: &Field, value: &dyn std::fmt::Debug) {
        self.0.insert(field.name().to_string(), format!("{value:?}"));
    }
}

struct Capture(Captured);

impl<S: Subscriber> Layer<S> for Capture {
    fn on_event(&self, event: &Event<'_>, _ctx: Context<'_, S>) {
        let mut fields = Fields(BTreeMap::new());
        event.record(&mut fields);
        let message = fields.0.remove("message").unwrap_or_default();
        assert_eq!(*event.metadata().level(), tracing::Level::INFO);
        self.0.lock().push((message, fields.0));
    }
}

/// Runs `f` under a capturing subscriber and returns the single event it logged.
fn capture(f: impl FnOnce()) -> (String, BTreeMap<String, String>) {
    let captured: Captured = Arc::default();
    let subscriber = Registry::default().with(Capture(Arc::clone(&captured)));
    tracing::subscriber::with_default(subscriber, f);
    let events = captured.lock().clone();
    assert_eq!(events.len(), 1, "exactly one event");
    events.into_iter().next().expect("one event")
}

fn names(fields: &BTreeMap<String, String>) -> Vec<&str> {
    fields.keys().map(String::as_str).collect()
}

fn assert_clean(fields: &BTreeMap<String, String>) {
    for value in fields.values() {
        assert!(!value.contains(TOKEN) && !value.contains("MADE_UP") && !value.contains("https://"));
    }
}

#[test]
fn delivery_answered_has_the_designed_fields_and_no_secret() {
    let payload = json!({
        "task_id": "11111111-1111-4111-8111-111111111111",
        "attempts": 2,
        "ai_participant_id": "00000000-0000-4000-8000-000000000001",
        "token": TOKEN,
        "api_url": URL,
        "message": format!("use {TOKEN} at {URL}"),
    });
    let answer = json!({
        "status": "dispatched",
        "channel": "tmux",
        "queue_position": 3,
        "reason": format!("{TOKEN} {URL}"),
        "session": "s",
    });
    let times = DeliveryTimes {
        slot: "bot-x-claude".into(),
        check_ms: 120,
        enqueue_ms: 4,
        total_ms: 130,
    };
    let (message, fields) = capture(|| timing::delivery_answered(&payload, Some(&answer), &times));
    assert_eq!(message, "delivery answered");
    assert_eq!(
        names(&fields),
        [
            "attempt",
            "bot",
            "channel",
            "check_ms",
            "enqueue_ms",
            "queue_position",
            "reason_code",
            "slot",
            "status",
            "task_id",
            "total_ms"
        ]
    );
    assert_eq!(fields["status"], "dispatched");
    assert_eq!(fields["channel"], "tmux");
    assert_eq!(fields["queue_position"], "3");
    assert_eq!(fields["check_ms"], "120");
    assert_eq!(fields["total_ms"], "130");
    assert_eq!(fields["attempt"], "2");
    assert_clean(&fields);
}

#[test]
fn delivery_answered_reports_a_refusal_code_and_an_unavailable_check() {
    let payload = json!({"task_id": "t", "attempts": 1, "ai_participant_id": "b"});
    let refused = json!({"status": "refused", "reason_code": "runner_agent_disabled", "reason": TOKEN});
    let (_, fields) = capture(|| timing::delivery_answered(&payload, Some(&refused), &DeliveryTimes::default()));
    assert_eq!(fields["status"], "refused");
    assert_eq!(fields["reason_code"], "runner_agent_disabled");
    assert_eq!(fields["queue_position"], "-1");
    assert_clean(&fields);

    let (_, fields) = capture(|| timing::delivery_answered(&payload, None, &DeliveryTimes::default()));
    assert_eq!(fields["status"], "unavailable");
}

#[test]
fn task_pushed_has_the_designed_fields() {
    let (message, fields) = capture(|| {
        timing::task_pushed(
            "11111111-1111-4111-8111-111111111111",
            1,
            "bot-x-claude",
            2,
            Duration::from_millis(1500),
            Duration::from_micros(2_600),
            "pushed",
        );
    });
    assert_eq!(message, "task pushed");
    assert_eq!(
        names(&fields),
        [
            "attempt",
            "member",
            "outcome",
            "push_ms",
            "queued_ms",
            "slot",
            "task_id"
        ]
    );
    assert_eq!(fields["member"], "2");
    assert_eq!(fields["queued_ms"], "1500");
    assert_eq!(fields["push_ms"], "2");
    assert_clean(&fields);
}

#[test]
fn task_left_session_has_the_designed_fields() {
    let (message, fields) = capture(|| {
        timing::task_left_session("t1", 3, "bot-x-claude", 0, Duration::from_secs(61), "superseded");
    });
    assert_eq!(message, "task left session");
    assert_eq!(
        names(&fields),
        ["attempt", "busy_ms", "end", "member", "slot", "task_id"]
    );
    assert_eq!(fields["busy_ms"], "61000");
    assert_eq!(fields["end"], "superseded");
}

#[test]
fn queue_depth_has_the_designed_fields() {
    let (message, fields) = capture(|| timing::queue_depth("bot-x-claude", 3, 4, "busy"));
    assert_eq!(message, "queue depth");
    assert_eq!(names(&fields), ["depth", "member", "slot", "state"]);
    assert_eq!(fields["depth"], "4");
}

#[test]
fn ids_are_logged_only_as_canonical_uuids_else_invalid() {
    let payload = json!({
        "task_id": "11111111-1111-4111-8111-111111111111\nforged line",
        "attempts": 1,
        "ai_participant_id": "not-a-uuid",
    });
    let (_, fields) = capture(|| timing::delivery_answered(&payload, None, &DeliveryTimes::default()));
    assert_eq!(fields["task_id"], "invalid");
    assert_eq!(fields["bot"], "invalid");

    let payload = json!({
        "task_id": "11111111111141118111111111111111",
        "attempts": 1,
        "ai_participant_id": "00000000-0000-4000-8000-00000000000A",
    });
    let (_, fields) = capture(|| timing::delivery_answered(&payload, None, &DeliveryTimes::default()));
    assert_eq!(fields["task_id"], "11111111-1111-4111-8111-111111111111");
    assert_eq!(fields["bot"], "00000000-0000-4000-8000-00000000000a");

    let (_, fields) = capture(|| timing::task_left_session(&"x".repeat(500), 1, "s", 0, Duration::ZERO, "terminal"));
    assert_eq!(fields["task_id"], "invalid");
}

fn capture_all(f: impl FnOnce()) -> Vec<(String, BTreeMap<String, String>)> {
    let captured: Captured = Arc::default();
    let subscriber = Registry::default().with(Capture(Arc::clone(&captured)));
    tracing::subscriber::with_default(subscriber, f);
    captured.lock().clone()
}

#[test]
fn the_current_task_clearing_logs_one_event_with_its_end() {
    let t0 = Instant::now();
    let t1 = t0 + Duration::from_secs(5);
    let events = capture_all(|| {
        let mut watch = CurrentWatch::default();
        watch.observe(
            Some(("11111111-1111-4111-8111-111111111111".into(), 1, t0)),
            t0,
            "slot",
            0,
        );
        watch.observe(
            Some(("11111111-1111-4111-8111-111111111111".into(), 1, t0)),
            t1,
            "slot",
            0,
        );
        watch.hint("gone");
        watch.observe(None, t1, "slot", 0);
        watch.observe(None, t1, "slot", 0);
        // The hint does not outlive the change it described.
        watch.observe(Some(("t2".into(), 1, t1)), t1, "slot", 0);
        watch.observe(None, t1 + Duration::from_secs(1), "slot", 0);
    });
    assert_eq!(events.len(), 2);
    assert_eq!(events[0].1["end"], "gone");
    assert_eq!(events[0].1["busy_ms"], "5000");
    assert_eq!(events[0].1["task_id"], "11111111-1111-4111-8111-111111111111");
    assert_eq!(events[1].1["end"], "terminal");
    assert_eq!(events[1].1["busy_ms"], "1000");
}

#[test]
fn queue_depth_is_logged_once_per_beat_and_only_when_not_empty() {
    let t0 = Instant::now();
    let beat = Duration::from_secs(10);
    let events = capture_all(|| {
        let mut depth = DepthBeat::new(t0);
        depth.tick(t0 + Duration::from_secs(11), beat, "slot", 0, 0, "idle");
        depth.tick(t0 + Duration::from_secs(5), beat, "slot", 0, 2, "busy");
        depth.tick(t0 + Duration::from_secs(11), beat, "slot", 0, 2, "busy");
        depth.tick(t0 + Duration::from_secs(12), beat, "slot", 0, 2, "busy");
        depth.tick(t0 + Duration::from_secs(22), beat, "slot", 0, 1, "idle");
    });
    assert_eq!(events.len(), 2);
    assert_eq!(events[0].1["depth"], "2");
    assert_eq!(events[1].1["state"], "idle");
}

#[test]
fn only_typed_pushes_get_a_pushed_word() {
    assert_eq!(pushed_word(&PushTry::Pushed), Some("pushed"));
    assert_eq!(pushed_word(&PushTry::Unknown), Some("unknown"));
    assert_eq!(pushed_word(&PushTry::PastedNotSubmitted), Some("pasted_not_submitted"));
    assert_eq!(pushed_word(&PushTry::NotReady), None);
    assert_eq!(pushed_word(&PushTry::Held), None);
    assert_eq!(state_word(&State::Idle), "idle");
}

#[test]
fn member_selected_has_the_designed_fields() {
    let (message, fields) = capture(|| {
        timing::member_selected(
            "11111111-1111-4111-8111-111111111111",
            "00000000-0000-4000-8000-000000000001",
            2,
            "idle",
        );
    });
    assert_eq!(message, "member selected");
    assert_eq!(names(&fields), ["bot", "member", "reason", "task_id"]);
    assert_eq!(fields["member"], "2");
    assert_eq!(fields["reason"], "idle");
    assert_clean(&fields);
}
