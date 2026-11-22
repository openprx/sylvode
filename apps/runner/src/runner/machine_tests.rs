#![allow(clippy::indexing_slicing, clippy::too_many_lines)]

use std::time::{Duration, Instant};

use super::detect::Dialog;
use super::hooks::HookEvent;
use super::machine::{
    Action, EnsureOutcome, Machine, PushOutcome, Reminder, State, Task, Timers, TurnWatch, ViewFacts,
};

#[derive(Clone, Debug, PartialEq, Eq)]
struct T(&'static str, i64);

impl Task for T {
    fn id(&self) -> &str {
        self.0
    }

    fn attempt(&self) -> i64 {
        self.1
    }
}

fn timers() -> Timers {
    Timers {
        heartbeat: Duration::from_mins(10),
        ack_reminder: Duration::from_mins(3),
        report_unacked: Duration::from_mins(15),
        completion_grace: Duration::from_mins(1),
        blocked_fail: Duration::from_mins(5),
        ready_timeout: Duration::from_secs(90),
        push_confirm: Duration::from_secs(10),
        poll: Duration::from_secs(10),
        liveness: Duration::from_secs(10),
        push_retry: Duration::from_secs(1),
        screen_hold: Duration::from_mins(10),
    }
}

/// A machine and a made-up clock.
struct H {
    m: Machine<T>,
    now: Instant,
}

const ACTIVE: ViewFacts = ViewFacts {
    active: true,
    acknowledged: false,
};
const ACKED: ViewFacts = ViewFacts {
    active: true,
    acknowledged: true,
};
const DONE: ViewFacts = ViewFacts {
    active: false,
    acknowledged: true,
};

impl H {
    fn new() -> Self {
        let now = Instant::now();
        Self {
            m: Machine::new(timers(), "SVWL", "tmux -L sylvode attach -t =SVWL", now),
            now,
        }
    }

    fn advance(&mut self, secs: u64) {
        self.now += Duration::from_secs(secs);
    }

    fn tick(&mut self, secs: u64) -> Vec<Action<T>> {
        self.advance(secs);
        self.m.on_tick(self.now)
    }

    fn enqueue(&mut self, task: T) -> Vec<Action<T>> {
        self.m.enqueue(task, self.now)
    }

    fn hook(&mut self, event: HookEvent) -> Vec<Action<T>> {
        self.m.on_hook(event, self.now)
    }

    fn view(&mut self, facts: ViewFacts) -> Vec<Action<T>> {
        self.m.on_view(facts, self.now)
    }

    fn push(&mut self, outcome: PushOutcome) -> Vec<Action<T>> {
        self.m.on_push(outcome, self.now)
    }

    /// Idle with a running CLI, nothing queued.
    fn idle() -> Self {
        let mut h = Self::new();
        h.m.adopt_running(h.now);
        h
    }

    /// One task pushed and the turn started: Busy.
    fn busy(task: T) -> Self {
        let mut h = Self::idle();
        let actions = h.enqueue(task.clone());
        assert!(actions.contains(&Action::TryPush(task)), "{actions:?}");
        h.push(PushOutcome::Pushed);
        h.hook(HookEvent::UserPromptSubmit);
        assert_eq!(h.m.state, State::Busy);
        h
    }
}

fn kinds(actions: &[Action<T>]) -> Vec<&'static str> {
    actions
        .iter()
        .map(|a| match a {
            Action::EnsureSession => "ensure",
            Action::TryPush(_) => "try_push",
            Action::EnterRetry => "enter",
            Action::SendReminder(Reminder::NotAcknowledged) => "remind_unacked",
            Action::SendReminder(Reminder::StillOpen) => "remind_open",
            Action::Progress { kind, .. } => kind,
            Action::Fail { retryable: true, .. } => "fail_retryable",
            Action::Fail { retryable: false, .. } => "fail",
            Action::Poll(_) => "poll",
            Action::CheckLiveness => "liveness",
            Action::CheckDialog => "dialog",
        })
        .collect()
}

fn fails(actions: &[Action<T>]) -> Vec<(&'static str, String, bool)> {
    actions
        .iter()
        .filter_map(|a| match a {
            Action::Fail {
                task,
                reason,
                retryable,
            } => Some((task.0, reason.clone(), *retryable)),
            _ => None,
        })
        .collect()
}

#[test]
fn a_task_goes_through_the_whole_cycle() {
    let mut h = H::new();
    assert_eq!(h.m.state, State::NoSession);
    // Arrival: queued, and the session is ensured.
    assert_eq!(kinds(&h.enqueue(T("a", 1))), ["runner.queued", "ensure"]);
    // The session was created: the CLI starts.
    assert!(h.m.on_ensure(EnsureOutcome::Started, h.now).is_empty());
    assert!(matches!(h.m.state, State::Starting { .. }));
    // Ready: push the head of the queue.
    assert_eq!(h.m.on_ready(h.now), vec![Action::TryPush(T("a", 1))]);
    assert_eq!(h.m.state, State::Idle);
    // Pushed.
    let actions = h.push(PushOutcome::Pushed);
    assert_eq!(kinds(&actions), ["runner.pushed"]);
    assert!(matches!(h.m.state, State::Pushing { .. }));
    assert!(h.m.queue.is_empty());
    // The turn starts.
    assert!(h.hook(HookEvent::UserPromptSubmit).is_empty());
    assert_eq!(h.m.state, State::Busy);
    // The turn ends: Sylvode is asked.
    assert_eq!(h.hook(HookEvent::Stop), vec![Action::Poll(T("a", 1))]);
    // Acknowledged and then completed.
    assert!(h.view(ACKED).is_empty());
    assert!(h.view(DONE).is_empty());
    assert_eq!(h.m.state, State::Idle);
    assert!(h.m.current_task().is_none());
}

#[test]
fn tasks_are_pushed_in_arrival_order_one_at_a_time() {
    let mut h = H::idle();
    assert_eq!(kinds(&h.enqueue(T("a", 1))), ["runner.queued", "try_push"]);
    // The second and third wait, with their positions.
    let actions = h.enqueue(T("b", 1));
    assert_eq!(actions.len(), 1);
    assert!(
        matches!(
            &actions[0],
            Action::Progress {
                queue_position: Some(2),
                ..
            }
        ),
        "{actions:?}"
    );
    h.enqueue(T("c", 1));
    assert_eq!(h.m.queue.len(), 3);
    // A push is in flight: no second TryPush.
    assert!(!kinds(&h.tick(2)).contains(&"try_push"));
    h.push(PushOutcome::Pushed);
    h.hook(HookEvent::UserPromptSubmit);
    h.hook(HookEvent::Stop);
    // The task is not terminal yet: b waits.
    assert!(!kinds(&h.view(ACKED)).contains(&"try_push"));
    assert_eq!(h.m.queue.len(), 2);
    // Terminal and idle: b is next, never c.
    assert_eq!(h.view(DONE), vec![Action::TryPush(T("b", 1))]);
}

#[test]
fn a_push_that_is_not_possible_yet_is_tried_again_a_second_later() {
    let mut h = H::idle();
    h.enqueue(T("a", 1));
    assert!(h.push(PushOutcome::NotReady).is_empty());
    assert!(h.tick(0).is_empty(), "not in the same instant");
    assert_eq!(h.tick(1), vec![Action::TryPush(T("a", 1))]);
    // Three plain failures in a row fail the task (retryable) and move on.
    let mut h = H::idle();
    h.enqueue(T("a", 1));
    h.enqueue(T("b", 1));
    assert!(h.push(PushOutcome::Failed).is_empty());
    h.tick(1);
    assert!(h.push(PushOutcome::Failed).is_empty());
    h.tick(1);
    let actions = h.push(PushOutcome::Failed);
    let failed = fails(&actions);
    assert_eq!(failed.len(), 1);
    assert_eq!((failed[0].0, failed[0].2), ("a", true));
    assert!(failed[0].1.contains("SVWL"), "{failed:?}");
}

#[test]
fn duplicates_and_new_attempts() {
    let mut h = H::idle();
    h.enqueue(T("a", 1));
    h.push(PushOutcome::NotReady);
    // The same task and attempt: ignored.
    assert!(h.enqueue(T("a", 1)).is_empty());
    assert_eq!(h.m.queue.len(), 1);
    // A new attempt replaces the old entry in place.
    h.enqueue(T("b", 1));
    let actions = h.enqueue(T("a", 2));
    assert!(
        matches!(&actions[0], Action::Progress { queue_position: Some(1), task, .. } if task.1 == 2),
        "{actions:?}"
    );
    assert_eq!(h.m.queue.len(), 2);
    assert_eq!(h.m.queue.front().map(|e| e.attempt), Some(2));

    // A new attempt of the task being supervised ends the old attempt.
    let mut h = H::busy(T("x", 1));
    h.hook(HookEvent::Stop);
    assert!(h.m.current_task().is_some());
    h.enqueue(T("x", 2));
    assert!(h.m.current_task().is_none());
    assert_eq!(h.m.queue.len(), 1);
}

#[test]
fn the_turn_must_start_after_a_push_with_one_enter_retry_then_blocked() {
    let mut h = H::idle();
    h.enqueue(T("a", 1));
    h.push(PushOutcome::Pushed);
    assert!(!kinds(&h.tick(9)).contains(&"enter"));
    assert_eq!(kinds(&h.tick(1)), ["enter", "liveness"]);
    // The second window is another ten seconds; then the session is blocked.
    assert!(!kinds(&h.tick(9)).contains(&"runner.blocked"));
    let actions = h.tick(1);
    assert!(kinds(&actions).contains(&"runner.blocked"), "{actions:?}");
    assert!(matches!(h.m.state, State::Blocked { .. }));
    // Seeing the turn start later resumes it.
    h.hook(HookEvent::UserPromptSubmit);
    assert_eq!(h.m.state, State::Busy);
}

#[test]
fn an_unknown_delivery_presses_enter_and_never_pastes_again() {
    for outcome in [PushOutcome::Unknown, PushOutcome::PastedNotSubmitted] {
        let mut h = H::idle();
        h.enqueue(T("a", 1));
        let actions = h.push(outcome);
        assert_eq!(kinds(&actions), ["runner.pushed", "enter"]);
        // The task is current, not queued: nothing can paste it again.
        assert!(h.m.queue.is_empty());
        assert!(!kinds(&h.tick(1)).contains(&"try_push"));
    }
}

#[test]
fn a_task_the_agent_never_acknowledges_gets_one_reminder_then_fails_retryably() {
    let mut h = H::busy(T("a", 1));
    // A busy CLI is never nagged, however long.
    assert!(!kinds(&h.tick(200)).iter().any(|k| k.starts_with("remind")));
    h.hook(HookEvent::Stop);
    assert!(
        h.view(ACTIVE)
            .iter()
            .any(|a| matches!(a, Action::SendReminder(Reminder::NotAcknowledged)))
    );
    // The worker could not send it (input not empty): asked again, not counted.
    assert!(h.m.on_reminder(Reminder::NotAcknowledged, false, h.now).is_empty());
    assert!(
        h.view(ACTIVE)
            .iter()
            .any(|a| matches!(a, Action::SendReminder(Reminder::NotAcknowledged)))
    );
    // Sent: recorded once.
    let recorded = h.m.on_reminder(Reminder::NotAcknowledged, true, h.now);
    assert_eq!(kinds(&recorded), ["runner.reminded"]);
    assert!(
        !kinds(&h.view(ACTIVE)).iter().any(|k| k.starts_with("remind")),
        "once only"
    );
    // The reminder starts a turn, which ends with the task still not acknowledged.
    h.hook(HookEvent::UserPromptSubmit);
    h.hook(HookEvent::Stop);
    let failed = fails(&h.view(ACTIVE));
    assert_eq!(
        failed,
        vec![("a", "agent did not acknowledge the task".to_string(), true)]
    );
    assert_eq!(h.m.state, State::Idle);
}

#[test]
fn an_unacknowledged_task_fails_after_the_report_time_even_without_a_turn_end() {
    let mut h = H::busy(T("a", 1));
    h.tick(899);
    let actions = h.tick(1);
    let failed = fails(&actions);
    assert_eq!(failed.len(), 1, "{actions:?}");
    assert!(failed[0].2, "retryable");
    // An acknowledged task is not failed for that.
    let mut h = H::busy(T("a", 1));
    h.view(ACKED);
    assert!(fails(&h.tick(1000)).is_empty());
}

#[test]
fn the_reminder_for_an_unacknowledged_task_also_comes_after_the_ack_time_when_idle() {
    let mut h = H::busy(T("a", 1));
    h.hook(HookEvent::Stop);
    // Idle but no turn counted yet? A Stop is a turn end: the reminder is due at once.
    assert!(
        h.view(ACTIVE)
            .iter()
            .any(|a| matches!(a, Action::SendReminder(Reminder::NotAcknowledged)))
    );
}

#[test]
fn a_task_left_open_after_an_acknowledged_turn_gets_the_grace_one_reminder_and_then_fails() {
    let mut h = H::busy(T("a", 1));
    h.view(ACKED);
    h.hook(HookEvent::Stop);
    // Right after the turn end: the model may be calling complete now.
    assert!(!kinds(&h.view(ACKED)).iter().any(|k| k.starts_with("remind")));
    h.advance(59);
    assert!(!kinds(&h.view(ACKED)).iter().any(|k| k.starts_with("remind")));
    // After the grace time, with a view taken after it: the reminder.
    h.advance(1);
    assert!(kinds(&h.view(ACKED)).contains(&"remind_open"));
    assert!(
        h.m.on_reminder(Reminder::StillOpen, true, h.now)
            .iter()
            .any(|a| matches!(
                a,
                Action::Progress {
                    kind: "runner.reminded",
                    ..
                }
            ))
    );
    assert!(!kinds(&h.view(ACKED)).contains(&"remind_open"), "once only");
    // The next turn ends with the task still open: failed, not retryable, naming the session.
    h.hook(HookEvent::UserPromptSubmit);
    h.hook(HookEvent::Stop);
    h.advance(60);
    let failed = fails(&h.view(ACKED));
    assert_eq!(failed.len(), 1);
    assert!(!failed[0].2, "not retryable");
    assert!(
        failed[0]
            .1
            .starts_with("agent ended its turn without completing or failing the task"),
        "{failed:?}"
    );
    assert!(failed[0].1.contains("tmux -L sylvode attach -t =SVWL"), "{failed:?}");
    assert_eq!(h.m.state, State::Idle);
}

#[test]
fn completing_inside_the_grace_time_is_not_forgetting() {
    let mut h = H::busy(T("a", 1));
    h.view(ACKED);
    h.hook(HookEvent::Stop);
    h.advance(30);
    assert!(h.view(DONE).is_empty());
    assert!(fails(&h.tick(120)).is_empty());
    assert_eq!(h.m.state, State::Idle);
}

#[test]
fn a_persons_own_turn_is_not_forgetting_and_does_not_block_heartbeats() {
    let mut h = H::idle();
    // The person starts a turn: no push is pending.
    h.hook(HookEvent::UserPromptSubmit);
    assert_eq!(h.m.state, State::BusyUser);
    // A task arrives and waits; it is not pushed and not failed, and it keeps reporting.
    let actions = h.enqueue(T("a", 1));
    assert_eq!(kinds(&actions), ["runner.queued"]);
    assert!(!kinds(&h.tick(5)).contains(&"try_push"));
    let beat = h.tick(600);
    assert!(kinds(&beat).contains(&"runner.queued"), "{beat:?}");
    assert!(fails(&beat).is_empty());
    // The person's turn ends: the task goes in.
    assert!(kinds(&h.hook(HookEvent::Stop)).contains(&"try_push"));
    assert_eq!(h.m.state, State::Idle);
}

#[test]
fn heartbeats_follow_the_design_and_stop_when_blocked_or_dead() {
    let mut h = H::busy(T("a", 1));
    h.view(ACKED);
    h.enqueue(T("q", 1));
    h.advance(599);
    let before = h.m.on_tick(h.now);
    assert!(!kinds(&before).contains(&"runner.alive"));
    let beat = h.tick(1);
    assert!(kinds(&beat).contains(&"runner.alive"), "{beat:?}");
    assert!(kinds(&beat).contains(&"runner.queued"), "{beat:?}");
    let position = beat.iter().find_map(|a| match a {
        Action::Progress {
            kind: "runner.queued",
            queue_position,
            ..
        } => *queue_position,
        _ => None,
    });
    assert_eq!(position, Some(1));
    // Blocked: the current task stops reporting alive.
    h.hook(HookEvent::Notification);
    assert!(matches!(h.m.state, State::Blocked { .. }));
    let later = h.tick(700);
    assert!(!kinds(&later).contains(&"runner.alive"), "{later:?}");
}

#[test]
fn a_blocked_session_fails_its_task_after_five_minutes_and_keeps_the_session() {
    let mut h = H::busy(T("a", 1));
    let actions = h.hook(HookEvent::Notification);
    assert_eq!(kinds(&actions), ["runner.blocked"]);
    // Another notification does not report twice.
    assert!(!kinds(&h.hook(HookEvent::Notification)).contains(&"runner.blocked"));
    assert!(fails(&h.tick(299)).is_empty());
    let failed = fails(&h.tick(1));
    assert_eq!(failed.len(), 1);
    assert!(!failed[0].2);
    assert_eq!(
        failed[0].1,
        "the CLI is waiting for a permission decision or dialog; attach to tmux session SVWL"
    );
    // The session is kept (Blocked, not Dead), the task is over, and it is not failed twice.
    assert!(matches!(h.m.state, State::Blocked { .. }));
    assert!(fails(&h.tick(1000)).is_empty());
    // The next task waits until the session is idle again.
    assert!(!kinds(&h.enqueue(T("b", 1))).contains(&"try_push"));
    assert!(kinds(&h.hook(HookEvent::Stop)).contains(&"try_push"));
}

#[test]
fn a_dialog_on_a_running_task_blocks_it() {
    let mut h = H::busy(T("a", 1));
    let actions = h.m.on_dialog(Dialog::Chooser, h.now);
    assert_eq!(kinds(&actions), ["runner.blocked"]);
    assert!(matches!(h.m.state, State::Blocked { .. }));
    // The person answers: the turn goes on.
    h.hook(HookEvent::UserPromptSubmit);
    assert_eq!(h.m.state, State::Busy);
}

#[test]
fn a_session_that_is_gone_fails_the_running_task_retryably_and_recreates_for_the_queue() {
    let mut h = H::busy(T("a", 1));
    h.enqueue(T("b", 1));
    let actions = h.m.on_session_gone(h.now);
    let failed = fails(&actions);
    assert_eq!(
        failed,
        vec![(
            "a",
            "tmux session SVWL ended while the task was running".to_string(),
            true
        )]
    );
    assert_eq!(h.m.state, State::Dead);
    // The queued task stays; the session is recreated for it.
    assert_eq!(h.m.queue.len(), 1);
    assert!(kinds(&actions).contains(&"ensure"));
    // Created again: the queued task is pushed after readiness.
    h.m.on_ensure(EnsureOutcome::Started, h.now);
    assert_eq!(h.m.on_ready(h.now), vec![Action::TryPush(T("b", 1))]);
    // Nothing running when the session goes: nothing to fail.
    let mut idle = H::idle();
    assert!(fails(&idle.m.on_session_gone(idle.now)).is_empty());
    assert_eq!(idle.m.state, State::Dead);
}

#[test]
fn a_push_that_finds_the_session_gone_is_a_dead_session() {
    let mut h = H::idle();
    h.enqueue(T("a", 1));
    let actions = h.push(PushOutcome::SessionGone);
    assert_eq!(h.m.state, State::Dead);
    assert!(kinds(&actions).contains(&"ensure"));
    assert_eq!(h.m.queue.len(), 1);
}

/// `SessionStart` also fires after compaction, `/clear` and resume: it neither fails the running
/// task nor lets the next one be pushed into the busy CLI.
#[test]
fn a_session_start_event_while_a_task_runs_neither_fails_nor_pushes() {
    let mut h = H::busy(T("a", 1));
    h.enqueue(T("b", 1));
    for state in ["busy", "mid-turn"] {
        if state == "mid-turn" {
            h.hook(HookEvent::UserPromptSubmit);
        }
        let actions = h.hook(HookEvent::SessionStart);
        assert!(fails(&actions).is_empty(), "{state}: {actions:?}");
        assert!(!kinds(&actions).contains(&"try_push"), "{state}: {actions:?}");
        assert_eq!(h.m.state, State::Busy, "{state}");
        assert_eq!(h.m.current_task(), Some(&T("a", 1)), "{state}");
    }
    // Idle and person-driven states ignore it too.
    let mut idle = H::idle();
    idle.hook(HookEvent::SessionStart);
    assert_eq!(idle.m.state, State::Idle);
}

/// An idle-prompt notification (not registered any more, but the machine must not rely on it)
/// leaves an idle session idle, and a blocked task that Sylvode ended frees the session.
#[test]
fn a_notification_with_nothing_running_does_not_block_and_a_blocked_task_that_ended_frees_the_session() {
    let mut idle = H::idle();
    assert!(idle.hook(HookEvent::Notification).is_empty());
    assert_eq!(idle.m.state, State::Idle);
    let mut h = H::busy(T("a", 1));
    h.hook(HookEvent::Notification);
    assert!(matches!(h.m.state, State::Blocked { .. }));
    // Blocked keeps polling the task.
    assert!(kinds(&h.tick(11)).contains(&"poll"));
    h.enqueue(T("b", 1));
    let actions = h.view(ViewFacts {
        active: false,
        acknowledged: true,
    });
    assert!(
        !kinds(&actions).contains(&"try_push"),
        "the CLI is still on its dialog: {actions:?}"
    );
    assert!(h.m.current_task().is_none());
    assert!(!matches!(h.m.state, State::Blocked { .. }), "{:?}", h.m.state);
    // The turn ending (the dialog answered) lets the next task in.
    assert!(kinds(&h.hook(HookEvent::Stop)).contains(&"try_push"));
}

#[test]
fn start_up_waits_for_readiness_checks_dialogs_and_times_out_retryably() {
    let mut h = H::new();
    h.enqueue(T("a", 1));
    h.m.on_ensure(EnsureOutcome::Started, h.now);
    assert!(kinds(&h.tick(1)).contains(&"dialog"));
    assert!(fails(&h.tick(88)).is_empty());
    let failed = fails(&h.tick(2));
    assert_eq!(
        failed,
        vec![("a", "cli did not become ready within 90 s".to_string(), true)]
    );
    assert!(matches!(h.m.state, State::Starting { .. }));
    // A later task starts the wait afresh and is pushed once the CLI is ready.
    h.enqueue(T("b", 1));
    assert_eq!(h.m.on_ready(h.now), vec![Action::TryPush(T("b", 1))]);
}

/// A first-start dialog (trust, bypass confirmation, chooser) fails every waiting task as
/// retryable with the attach hint: the operator answers it once in the session and the next
/// attempt goes in.
#[test]
fn a_first_start_dialog_fails_every_waiting_task_as_retryable() {
    for dialog in [Dialog::Trust, Dialog::BypassConfirm, Dialog::Chooser] {
        let mut h = H::new();
        h.enqueue(T("a", 1));
        h.enqueue(T("b", 1));
        h.m.on_ensure(EnsureOutcome::Started, h.now);
        let actions = h.m.on_dialog(dialog, h.now);
        let failed = fails(&actions);
        assert_eq!(failed.len(), 2, "{dialog:?}");
        assert!(
            failed
                .iter()
                .all(|f| f.2 && f.1.contains("shows a dialog; attach with tmux attach -t SVWL")),
            "{failed:?}"
        );
        assert!(h.m.queue.is_empty());
        // A task arriving while the dialog is up fails at once, retryably.
        let later = fails(&h.enqueue(T("c", 1)));
        assert_eq!(later.len(), 1);
        assert!(later[0].2);
        // After the operator answered, the CLI becomes ready and tasks queue again.
        h.m.on_ready(h.now);
        assert!(kinds(&h.enqueue(T("d", 1))).contains(&"try_push"));
    }
}

/// Each dialog names its own code and failure text; only a usage limit is not retried.
#[test]
fn dialogs_have_their_codes_texts_and_retryability() {
    assert_eq!(Dialog::NotLoggedIn.code(), "cli_not_logged_in");
    assert_eq!(Dialog::UsageLimit.code(), "cli_usage_limit");
    for dialog in [Dialog::Trust, Dialog::BypassConfirm, Dialog::Chooser] {
        assert_eq!(dialog.code(), "cli_dialog");
        assert!(dialog.retryable());
    }
    assert!(Dialog::NotLoggedIn.retryable() && !Dialog::UsageLimit.retryable());
    assert_eq!(
        Dialog::NotLoggedIn.reason("QLA-claude"),
        "the CLI in tmux session QLA-claude is not logged in; attach and log in"
    );
    assert_eq!(
        Dialog::BypassConfirm.reason("QLA-claude"),
        "the CLI in tmux session QLA-claude shows a dialog; attach with tmux attach -t QLA-claude and answer it"
    );
    assert_eq!(
        Dialog::UsageLimit.reason("QLA-claude"),
        "the CLI reports that its usage limit is reached"
    );
}

#[test]
fn a_declined_session_fails_the_head_task_and_tries_the_next_a_second_later() {
    let mut h = H::new();
    h.enqueue(T("a", 1));
    h.enqueue(T("b", 1));
    let actions = h.m.on_ensure(
        EnsureOutcome::Declined {
            reason: "window 0 runs vim".to_string(),
            retryable: false,
        },
        h.now,
    );
    assert_eq!(fails(&actions), vec![("a", "window 0 runs vim".to_string(), false)]);
    assert!(!kinds(&actions).contains(&"ensure"), "not in the same instant");
    assert_eq!(kinds(&h.tick(1)), ["ensure"]);
}

#[test]
fn an_adopted_running_cli_is_idle_at_once() {
    let mut h = H::new();
    h.enqueue(T("a", 1));
    let actions = h.m.on_ensure(EnsureOutcome::Running, h.now);
    assert_eq!(actions, vec![Action::TryPush(T("a", 1))]);
}

#[test]
fn liveness_is_checked_every_ten_seconds_once_a_session_exists() {
    let mut h = H::idle();
    assert!(!kinds(&h.tick(9)).contains(&"liveness"));
    assert!(kinds(&h.tick(1)).contains(&"liveness"));
    assert!(!kinds(&h.tick(1)).contains(&"liveness"));
    let mut none = H::new();
    assert!(!kinds(&none.tick(30)).contains(&"liveness"));
}

#[test]
fn a_task_that_ended_in_sylvode_frees_the_session_when_the_cli_is_idle_only() {
    // Terminal while the CLI still works: the session is the person's or the tail of the turn.
    let mut h = H::busy(T("a", 1));
    h.enqueue(T("b", 1));
    let actions = h.view(DONE);
    assert!(!kinds(&actions).contains(&"try_push"), "{actions:?}");
    assert_eq!(h.m.state, State::BusyUser);
    assert!(kinds(&h.hook(HookEvent::Stop)).contains(&"try_push"));
}

#[test]
fn recovered_state_is_supervised_like_a_pushed_task() {
    let mut h = H::new();
    h.m.adopt_running(h.now);
    h.m.adopt_current(T("a", 1), true, false, h.now);
    assert_eq!(h.m.state, State::Busy);
    assert!(h.m.current_task().is_some());
    h.hook(HookEvent::Stop);
    assert!(!h.view(DONE).iter().any(|a| matches!(a, Action::Fail { .. })));
    assert_eq!(h.m.state, State::Idle);
    let mut b = H::new();
    b.m.adopt_running(b.now);
    b.m.adopt_current(T("a", 1), false, true, b.now);
    assert!(matches!(b.m.state, State::Blocked { .. }));
}

type Input = (&'static str, fn(&mut H) -> Vec<Action<T>>);

/// Every state meets every input: nothing panics, and the invariants of the machine hold.
#[test]
fn every_state_meets_every_input() {
    type Reach = fn() -> H;
    let states: [(&str, Reach); 8] = [
        ("NoSession", H::new),
        ("Starting", || {
            let mut h = H::new();
            h.enqueue(T("q0", 1));
            h.m.on_ensure(EnsureOutcome::Started, h.now);
            h
        }),
        ("Idle", H::idle),
        ("Pushing", || {
            let mut h = H::idle();
            h.enqueue(T("p", 1));
            h.push(PushOutcome::Pushed);
            h
        }),
        ("Busy", || H::busy(T("p", 1))),
        ("BusyUser", || {
            let mut h = H::idle();
            h.hook(HookEvent::UserPromptSubmit);
            h
        }),
        ("Blocked", || {
            let mut h = H::busy(T("p", 1));
            h.hook(HookEvent::Notification);
            h
        }),
        ("Dead", || {
            let mut h = H::idle();
            h.m.on_session_gone(h.now);
            h
        }),
    ];
    let inputs: Vec<Input> = vec![
        ("enqueue", |h| h.enqueue(T("n", 1))),
        ("enqueue again", |h| h.enqueue(T("p", 2))),
        ("hook SessionStart", |h| h.hook(HookEvent::SessionStart)),
        ("hook UserPromptSubmit", |h| h.hook(HookEvent::UserPromptSubmit)),
        ("hook Stop", |h| h.hook(HookEvent::Stop)),
        ("hook Notification", |h| h.hook(HookEvent::Notification)),
        ("hook TurnEnded", |h| h.hook(HookEvent::TurnEnded)),
        ("tick 1", |h| h.tick(1)),
        ("tick 400", |h| h.tick(400)),
        ("tick 5000", |h| h.tick(5000)),
        ("view active", |h| h.view(ACTIVE)),
        ("view acked", |h| h.view(ACKED)),
        ("view done", |h| h.view(DONE)),
        ("push pushed", |h| h.push(PushOutcome::Pushed)),
        ("push unknown", |h| h.push(PushOutcome::Unknown)),
        ("push not submitted", |h| h.push(PushOutcome::PastedNotSubmitted)),
        ("push not ready", |h| h.push(PushOutcome::NotReady)),
        ("push gone", |h| h.push(PushOutcome::SessionGone)),
        ("push failed", |h| h.push(PushOutcome::Failed)),
        ("ensure started", |h| h.m.on_ensure(EnsureOutcome::Started, h.now)),
        ("ensure running", |h| h.m.on_ensure(EnsureOutcome::Running, h.now)),
        ("ensure declined", |h| {
            h.m.on_ensure(
                EnsureOutcome::Declined {
                    reason: "no".to_string(),
                    retryable: false,
                },
                h.now,
            )
        }),
        ("ready", |h| h.m.on_ready(h.now)),
        ("dialog trust", |h| h.m.on_dialog(Dialog::Trust, h.now)),
        ("dialog bypass", |h| h.m.on_dialog(Dialog::BypassConfirm, h.now)),
        ("dialog cli", |h| h.m.on_dialog(Dialog::Chooser, h.now)),
        ("session gone", |h| h.m.on_session_gone(h.now)),
        ("reminder sent", |h| {
            h.m.on_reminder(Reminder::NotAcknowledged, true, h.now)
        }),
        ("reminder not sent", |h| {
            h.m.on_reminder(Reminder::StillOpen, false, h.now)
        }),
    ];
    let mut combinations = 0;
    for (state_name, reach) in states {
        for (input_name, apply) in &inputs {
            let mut h = reach();
            let actions = apply(&mut h);
            combinations += 1;
            let context = format!("{state_name} + {input_name}: {:?} {actions:?}", h.m.state);
            // A current task exists only in states that supervise one.
            if h.m.current_task().is_some() {
                assert!(
                    matches!(h.m.state, State::Pushing { .. } | State::Busy | State::Blocked { .. }),
                    "{context}"
                );
            }
            // Never two TryPush at once, never TryPush with a current task.
            let pushes = actions.iter().filter(|a| matches!(a, Action::TryPush(_))).count();
            assert!(pushes <= 1, "{context}");
            if pushes == 1 {
                assert!(h.m.current_task().is_none(), "{context}");
                assert_eq!(h.m.state, State::Idle, "{context}");
            }
            // A failure never names a task that is still queued or current.
            for (id, _, _) in fails(&actions) {
                assert!(h.m.queue.position(id).is_none() || id == "p", "{context}");
                assert!(h.m.current_task().is_none_or(|t| t.0 != id), "{context}");
            }
            // The machine always goes on: a tick after any input does not panic.
            let _ = h.tick(1);
        }
    }
    assert_eq!(combinations, 8 * 29);
}

/// `Dead` and `NoSession` leave only through an ensure, and `Idle` with queued work always pushes.
#[test]
fn idle_with_queued_work_always_pushes_and_dead_always_ensures() {
    let mut h = H::idle();
    assert!(kinds(&h.enqueue(T("a", 1))).contains(&"try_push"));
    let mut d = H::idle();
    d.m.on_session_gone(d.now);
    assert!(kinds(&d.enqueue(T("a", 1))).contains(&"ensure"));
    let mut n = H::new();
    assert!(kinds(&n.enqueue(T("a", 1))).contains(&"ensure"));
}

/// A re-delivery of the task being supervised, at its attempt or an older one, is neither queued
/// nor pushed again; a queued task is not replaced by an older attempt.
#[test]
fn a_redelivery_of_the_current_task_changes_nothing() {
    let mut h = H::busy(T("a", 2));
    assert!(h.enqueue(T("a", 2)).is_empty(), "same attempt");
    assert!(h.enqueue(T("a", 1)).is_empty(), "older attempt");
    assert!(h.m.queue.is_empty());
    assert_eq!(h.m.current_task(), Some(&T("a", 2)));
    // Even after it ends, nothing of it was left queued to be typed again.
    h.view(DONE);
    h.hook(HookEvent::Stop);
    assert!(h.m.queue.is_empty());
}

/// A task that Sylvode no longer shows active is dropped before it is typed; the next one goes.
#[test]
fn an_inactive_task_is_dropped_and_never_typed() {
    let mut h = H::idle();
    h.enqueue(T("a", 1));
    h.enqueue(T("b", 1));
    // The worker found `a` inactive just before the push.
    let actions = h.push(PushOutcome::Inactive);
    assert_eq!(h.m.queue.len(), 1);
    assert!(actions.contains(&Action::TryPush(T("b", 1))), "{actions:?}");
    assert!(h.m.current_task().is_none());
}

/// A turn the hooks never closed (an interrupt runs no `Stop`) is closed by calm screen samples,
/// and only when the last event is old enough and the samples were calm.
#[test]
fn a_turn_without_a_stop_is_closed_by_calm_samples_only() {
    let t0 = Instant::now();
    let mut watch = TurnWatch::new(3, Duration::from_mins(1), t0);
    let at = |s: u64| t0 + Duration::from_secs(s);
    // Calm but too early: the last event is recent.
    for n in 1..=5 {
        assert!(!watch.sample(at(n), false, true));
    }
    // Busy or a draft in the box resets the count; three calm samples after 60 s close the turn.
    assert!(!watch.sample(at(70), true, true));
    assert!(!watch.sample(at(75), false, false));
    assert!(!watch.sample(at(80), false, true));
    assert!(!watch.sample(at(85), false, true));
    assert!(watch.sample(at(90), false, true), "three calm samples after 60 s");
    // An event starts over.
    watch.event(at(100));
    assert!(!watch.sample(at(105), false, true));
}

/// A screen that says busy while the hooks say idle holds the queue back only for a bounded time:
/// then the waiting tasks fail as retryable with the reason, and the session is not stalled for good.
#[test]
fn a_screen_hold_is_bounded_and_then_fails_the_waiting_tasks_retryably() {
    let mut h = H::idle();
    h.enqueue(T("a", 1));
    h.enqueue(T("b", 1));
    // Held for a while: nothing fails, the push is tried again each second.
    for _ in 0..9 {
        h.advance(60);
        assert!(fails(&h.push(PushOutcome::Held)).is_empty());
    }
    assert_eq!(h.m.queue.len(), 2);
    h.advance(121);
    let failed = fails(&h.push(PushOutcome::Held));
    assert_eq!(failed.len(), 2, "{failed:?}");
    assert!(
        failed
            .iter()
            .all(|(_, reason, retryable)| *retryable && reason.contains("10 minutes"))
    );
    assert!(h.m.queue.is_empty());
    // A new task starts the clock afresh; a push that went through clears it.
    h.enqueue(T("c", 1));
    h.advance(60);
    assert!(fails(&h.push(PushOutcome::Held)).is_empty());
    h.push(PushOutcome::Pushed);
}

/// A lost `UserPromptSubmit`: the agent's acknowledgement is proof that the turn started, so a task
/// that is `Pushing` or `Blocked` becomes `Busy` and is not failed as a permission wait.
#[test]
fn an_acknowledged_task_is_busy_whatever_the_hooks_said() {
    let mut h = H::idle();
    h.enqueue(T("a", 1));
    h.push(PushOutcome::Pushed);
    assert!(matches!(h.m.state, State::Pushing { .. }));
    h.view(ACKED);
    assert_eq!(h.m.state, State::Busy);
    // The same from Blocked (the Enter gate refused and reported the session blocked).
    let mut h = H::idle();
    h.enqueue(T("a", 1));
    h.push(PushOutcome::Pushed);
    h.m.on_enter_refused(h.now);
    assert!(matches!(h.m.state, State::Blocked { .. }));
    h.view(ACKED);
    assert_eq!(h.m.state, State::Busy);
    // Five minutes later the task has not been failed.
    assert!(fails(&h.tick(301)).is_empty());
    // Not acknowledged: still blocked, and it is failed as before after five minutes.
    let mut h = H::idle();
    h.enqueue(T("a", 1));
    h.push(PushOutcome::Pushed);
    h.m.on_enter_refused(h.now);
    h.view(ACTIVE);
    assert!(matches!(h.m.state, State::Blocked { .. }));
}
