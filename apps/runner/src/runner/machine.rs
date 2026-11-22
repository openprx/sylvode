//! The state machine of one tmux session (design sections 7.3 to 7.7): what the runner does next,
//! given the signals it has seen. It is pure: it never touches tmux, the network or the clock. The
//! worker (`session.rs`) feeds it hook events, poll results, push outcomes and the current time as
//! an [`Instant`], and carries out the [`Action`]s it returns. That is what lets every timer be
//! tested with a made-up clock.
//!
//! ```text
//! NoSession -> Starting -> Idle -> Pushing -> Busy -> Idle
//!                            \-> BusyUser -> Idle        (a person's own turn)
//!      Pushing | Busy | BusyUser -> Blocked -> Busy | Idle   (permission prompt, dialog)
//!      any -> Dead -> Starting                               (session or CLI gone)
//! ```
//!
//! The session is free for the next task only when the current task is terminal in Sylvode and the
//! CLI is idle. A busy CLI is never nagged. A person's own turn never counts as forgetting.

use std::time::{Duration, Instant};

use super::detect::Dialog;
use super::hooks::HookEvent;
use super::queue::{Enqueued, Entry, TaskQueue};

/// A task the machine can queue.
pub trait Task: Clone {
    fn id(&self) -> &str;
    fn attempt(&self) -> i64;
}

/// Every duration the machine acts on.
#[derive(Clone, Copy, Debug)]
pub struct Timers {
    /// `runner.queued` and `runner.alive` interval.
    pub heartbeat: Duration,
    /// Idle time after a push before the "not acknowledged" reminder.
    pub ack_reminder: Duration,
    /// Time after a push at which an unacknowledged task is failed.
    pub report_unacked: Duration,
    /// Wait after a turn ended before the "still open" reminder.
    pub completion_grace: Duration,
    /// A blocked session fails its task after this long.
    pub blocked_fail: Duration,
    /// CLI start until it is ready.
    pub ready_timeout: Duration,
    /// Time to see the turn start after a push, before and after the one Enter retry.
    pub push_confirm: Duration,
    /// Interval of the task poll while a task is pushed.
    pub poll: Duration,
    /// Interval of the liveness check.
    pub liveness: Duration,
    /// Minimum time between two push attempts of a task that could not be pushed yet.
    pub push_retry: Duration,
    /// How long the screen may hold pushes back before the waiting tasks are failed.
    pub screen_hold: Duration,
}

impl Timers {
    /// The design's values, from the `[runner]` and `[runner.tmux]` settings.
    pub fn from_config(runner: &super::config::RunnerConfig) -> Self {
        let tmux = &runner.tmux;
        Self {
            heartbeat: Duration::from_secs(runner.heartbeat_secs.max(1)),
            ack_reminder: Duration::from_secs(tmux.ack_reminder_secs),
            report_unacked: Duration::from_secs(tmux.report_unacked_after_secs),
            completion_grace: Duration::from_secs(tmux.completion_grace_secs),
            blocked_fail: Duration::from_mins(5),
            ready_timeout: Duration::from_secs(tmux.ready_timeout_secs),
            push_confirm: Duration::from_secs(10),
            poll: Duration::from_secs(10),
            liveness: Duration::from_secs(10),
            push_retry: Duration::from_secs(1),
            screen_hold: Duration::from_secs(tmux.screen_hold_secs.max(30)),
        }
    }
}

/// The state of the session.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum State {
    /// No session known yet, or it must be (re)created.
    NoSession,
    /// The CLI is starting; waiting for its readiness signal.
    Starting { since: Instant },
    /// The CLI is idle and no task of the runner is current.
    Idle,
    /// A task was typed in; waiting to see the turn start.
    Pushing { since: Instant, enter_retried: bool },
    /// The current task is being worked on, or is open after a turn.
    Busy,
    /// A person's own turn.
    BusyUser,
    /// A permission prompt or dialog is waiting for a person.
    Blocked { since: Instant },
    /// The session or the CLI is gone.
    Dead,
}

/// The kinds of reminder.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Reminder {
    NotAcknowledged,
    StillOpen,
}

/// What Sylvode says about the current task.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ViewFacts {
    pub active: bool,
    pub acknowledged: bool,
}

/// What the worker must do.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Action<T> {
    /// Create, adopt or respawn the session.
    EnsureSession,
    /// Check the preconditions (CLI idle is known; input empty, user not typing) and push `T`.
    TryPush(T),
    /// Press Enter once more.
    EnterRetry,
    /// Send a reminder when the CLI is idle and the input is empty, then report it back.
    SendReminder(Reminder),
    /// A progress report with a `runner.*` kind.
    Progress {
        task: T,
        kind: &'static str,
        note: &'static str,
        queue_position: Option<i32>,
    },
    /// End a task with `source: "runner"`, but only when Sylvode still shows it active.
    Fail { task: T, reason: String, retryable: bool },
    /// Read the current task's view and report it back.
    Poll(T),
    /// Check that the session and the CLI are alive.
    CheckLiveness,
    /// Look at the screen for a blocking dialog.
    CheckDialog,
}

/// How an [`Action::EnsureSession`] ended.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum EnsureOutcome {
    /// A new session was created; its CLI is starting.
    Started,
    /// An existing session runs the CLI and is used as it is.
    Running,
    /// The session cannot be used.
    Declined { reason: String, retryable: bool },
}

/// How an [`Action::TryPush`] ended.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PushOutcome {
    /// The text was typed in and Enter was sent.
    Pushed,
    /// The paste timed out: the text may be in the input box. Never pasted again; Enter only.
    Unknown,
    /// The text was pasted but Enter failed.
    PastedNotSubmitted,
    /// A precondition does not hold yet (input not empty, a person is typing, no prompt seen, the
    /// pane is not ready for a bracketed paste).
    NotReady,
    /// The screen shows the CLI busy or waiting for a choice, though the hooks say idle: nothing
    /// is typed. Bounded by [`Timers::screen_hold`].
    Held,
    /// The session is gone.
    SessionGone,
    /// tmux failed in another way.
    Failed,
    /// Sylvode no longer shows the task active (swept, cancelled, answered): it is dropped from
    /// the queue and never typed.
    Inactive,
}

/// The task the runner pushed and supervises.
#[derive(Clone, Debug)]
#[allow(clippy::struct_excessive_bools, reason = "independent flags of one supervised task")]
pub struct Current<T> {
    pub task: T,
    pushed_at: Instant,
    acked: bool,
    cli_idle: bool,
    turn_ends: u32,
    idle_since: Option<Instant>,
    reminded_unacked_at_turn: Option<u32>,
    reminded_open_at_turn: Option<u32>,
    reminder_in_flight: bool,
    last_poll: Instant,
    last_alive: Instant,
    last_view: Instant,
    blocked_reported: bool,
    failed_blocked: bool,
}

/// The machine.
#[derive(Debug)]
#[allow(clippy::struct_excessive_bools, reason = "independent flags of one state machine")]
pub struct Machine<T> {
    timers: Timers,
    key: String,
    attach_hint: String,
    pub state: State,
    pub queue: TaskQueue<T>,
    pub current: Option<Current<T>>,
    /// A dialog seen while starting: queued tasks fail with its reason instead of waiting.
    starting_dialog: Option<Dialog>,
    ensuring: bool,
    push_in_flight: bool,
    next_push_at: Instant,
    /// Since when pushes have been held back by the screen (see [`PushOutcome::Held`]).
    held_since: Option<Instant>,
    next_ensure_at: Instant,
    consecutive_push_failures: u32,
    last_queue_beat: Instant,
    last_liveness: Instant,
    /// The CLI is known to be working although no task of the runner is current.
    cli_working: bool,
}

impl<T: Task> Machine<T> {
    pub fn new(timers: Timers, key: &str, attach_hint: &str, now: Instant) -> Self {
        Self {
            timers,
            key: key.to_string(),
            attach_hint: attach_hint.to_string(),
            state: State::NoSession,
            queue: TaskQueue::default(),
            current: None,
            starting_dialog: None,
            ensuring: false,
            push_in_flight: false,
            next_push_at: now,
            held_since: None,
            next_ensure_at: now,
            consecutive_push_failures: 0,
            last_queue_beat: now,
            last_liveness: now,
            cli_working: false,
        }
    }

    /// The session is known to run the CLI and needs no creation (a recovered session).
    pub const fn adopt_running(&mut self, now: Instant) {
        self.state = State::Idle;
        self.last_liveness = now;
    }

    /// A task that was pushed before a restart and is still active: supervise it as current.
    pub fn adopt_current(&mut self, task: T, acknowledged: bool, blocked: bool, now: Instant) {
        self.current = Some(Current {
            task,
            pushed_at: now,
            acked: acknowledged,
            cli_idle: true,
            turn_ends: 0,
            idle_since: None,
            reminded_unacked_at_turn: None,
            reminded_open_at_turn: None,
            reminder_in_flight: false,
            last_poll: now,
            last_alive: now,
            last_view: now,
            blocked_reported: blocked,
            failed_blocked: false,
        });
        self.state = if blocked {
            State::Blocked { since: now }
        } else {
            State::Busy
        };
    }

    fn fail_text(&self, text: &str) -> String {
        text.replace("KEY", &self.key)
    }

    /// A task arrives from a delivery.
    pub fn enqueue(&mut self, task: T, now: Instant) -> Vec<Action<T>> {
        let mut actions = Vec::new();
        if let Some(dialog) = self.starting_dialog
            && matches!(self.state, State::Starting { .. })
        {
            actions.push(Action::Fail {
                reason: dialog.reason(&self.key),
                retryable: false,
                task,
            });
            return actions;
        }
        // A re-delivery of the task being supervised, at its attempt or an older one, changes
        // nothing: it is neither queued nor pushed again.
        if self
            .current
            .as_ref()
            .is_some_and(|c| c.task.id() == task.id() && c.task.attempt() >= task.attempt())
        {
            return actions;
        }
        // A new attempt of the task being supervised: the old attempt is over.
        if self
            .current
            .as_ref()
            .is_some_and(|c| c.task.id() == task.id() && c.task.attempt() < task.attempt())
        {
            self.current = None;
            self.state = if self.cli_working_now() {
                State::BusyUser
            } else {
                State::Idle
            };
        }
        let (id, attempt) = (task.id().to_string(), task.attempt());
        match self.queue.enqueue(&id, attempt, task.clone()) {
            Enqueued::Added(position) | Enqueued::Replaced(position) => actions.push(Action::Progress {
                task,
                kind: "runner.queued",
                note: "queued behind other tasks of this session",
                queue_position: i32::try_from(position).ok(),
            }),
            Enqueued::Duplicate => {}
        }
        actions.extend(self.drive(now));
        actions
    }

    fn cli_working_now(&self) -> bool {
        self.cli_working || self.current.as_ref().is_some_and(|c| !c.cli_idle)
    }

    /// Starts whatever the state allows.
    fn drive(&mut self, now: Instant) -> Vec<Action<T>> {
        let mut actions = Vec::new();
        if self.queue.is_empty() {
            return actions;
        }
        match self.state {
            State::NoSession | State::Dead if !self.ensuring && now >= self.next_ensure_at => {
                self.ensuring = true;
                actions.push(Action::EnsureSession);
            }
            State::Idle if !self.push_in_flight && self.current.is_none() && now >= self.next_push_at => {
                if let Some(head) = self.queue.front() {
                    self.push_in_flight = true;
                    actions.push(Action::TryPush(head.item.clone()));
                }
            }
            _ => {}
        }
        actions
    }

    /// The outcome of [`Action::EnsureSession`].
    pub fn on_ensure(&mut self, outcome: EnsureOutcome, now: Instant) -> Vec<Action<T>> {
        // An answer to a request that was never made (or was overtaken by a dead session) is
        // ignored.
        if !self.ensuring {
            return Vec::new();
        }
        self.ensuring = false;
        let mut actions = Vec::new();
        match outcome {
            EnsureOutcome::Started => {
                self.state = State::Starting { since: now };
                self.starting_dialog = None;
            }
            EnsureOutcome::Running => {
                self.state = State::Idle;
                self.last_liveness = now;
            }
            EnsureOutcome::Declined { reason, retryable } => {
                // The head task fails; the next one is tried a moment later.
                if let Some(head) = self.queue.pop_front() {
                    actions.push(Action::Fail {
                        task: head.item,
                        reason,
                        retryable,
                    });
                }
                self.next_ensure_at = now + self.timers.push_retry;
            }
        }
        actions.extend(self.drive(now));
        actions
    }

    /// The CLI's readiness signal (`SessionStart`, or a stable screen with the prompt).
    pub fn on_ready(&mut self, now: Instant) -> Vec<Action<T>> {
        if matches!(self.state, State::Starting { .. }) {
            self.state = State::Idle;
            self.starting_dialog = None;
            self.last_liveness = now;
        }
        self.drive(now)
    }

    /// The CLI of the session was restarted (a directory or CLI change, or a recovered pane):
    /// wait for its readiness again.
    pub const fn on_cli_restarted(&mut self, now: Instant) {
        self.push_in_flight = false;
        self.cli_working = false;
        self.state = State::Starting { since: now };
        self.starting_dialog = None;
    }

    /// Whether the CLI is mid-turn as far as the hooks said (a `UserPromptSubmit` without its
    /// `Stop`).
    pub const fn turn_open(&self) -> bool {
        self.cli_working
    }

    /// The one Enter retry was not allowed to run (the screen or the person did not permit it):
    /// nothing was pressed, and the session is reported blocked.
    pub fn on_enter_refused(&mut self, now: Instant) -> Vec<Action<T>> {
        if matches!(self.state, State::Pushing { .. }) {
            return self.block(now);
        }
        Vec::new()
    }

    /// A blocking dialog is on the screen.
    pub fn on_dialog(&mut self, dialog: Dialog, now: Instant) -> Vec<Action<T>> {
        let mut actions = Vec::new();
        match self.state {
            State::Starting { .. } => {
                self.starting_dialog = Some(dialog);
                for entry in self.queue.drain_all() {
                    actions.push(Action::Fail {
                        task: entry.item,
                        reason: dialog.reason(&self.key),
                        retryable: false,
                    });
                }
            }
            State::Pushing { .. } | State::Busy | State::BusyUser => actions.extend(self.block(now)),
            _ => {}
        }
        actions
    }

    fn block(&mut self, now: Instant) -> Vec<Action<T>> {
        let mut actions = Vec::new();
        self.state = State::Blocked { since: now };
        if let Some(current) = self.current.as_mut()
            && !current.blocked_reported
        {
            current.blocked_reported = true;
            actions.push(Action::Progress {
                task: current.task.clone(),
                kind: "runner.blocked",
                note: "the CLI is waiting for a permission decision or dialog",
                queue_position: None,
            });
        }
        actions
    }

    /// A hook event from the CLI.
    pub fn on_hook(&mut self, event: HookEvent, now: Instant) -> Vec<Action<T>> {
        let mut actions = Vec::new();
        match event {
            // The CLI's own start (the hook is registered for `startup` only): readiness while the
            // session starts. In any other state it means nothing: a restart is told by the pane
            // (its PID or its death), never by this event, and a running task is never failed or
            // re-pushed because of it.
            HookEvent::SessionStart => {
                if matches!(self.state, State::Starting { .. }) {
                    self.state = State::Idle;
                    self.starting_dialog = None;
                    self.last_liveness = now;
                }
            }
            HookEvent::UserPromptSubmit => self.on_turn_start(now),
            HookEvent::Stop | HookEvent::TurnEnded => actions.extend(self.on_turn_end(now)),
            HookEvent::Notification => {
                // A permission prompt or dialog with something to protect: a task in flight, or a
                // person's turn. With nothing running there is nothing to block.
                if matches!(self.state, State::Pushing { .. } | State::Busy | State::BusyUser) {
                    actions.extend(self.block(now));
                }
            }
        }
        actions.extend(self.drive(now));
        actions
    }

    const fn on_turn_start(&mut self, now: Instant) {
        self.cli_working = true;
        match self.state {
            State::Pushing { .. } | State::Blocked { .. } if self.current.is_some() => {
                if let Some(current) = self.current.as_mut() {
                    current.cli_idle = false;
                    current.idle_since = None;
                    current.last_alive = now;
                }
                self.state = State::Busy;
            }
            State::Busy => {
                if let Some(current) = self.current.as_mut() {
                    current.cli_idle = false;
                    current.idle_since = None;
                }
            }
            // No push is pending: this is a person's own turn.
            State::Idle | State::Blocked { .. } | State::Starting { .. } | State::BusyUser => {
                self.state = State::BusyUser;
            }
            State::NoSession | State::Dead | State::Pushing { .. } => {}
        }
    }

    fn on_turn_end(&mut self, now: Instant) -> Vec<Action<T>> {
        self.cli_working = false;
        let mut actions = Vec::new();
        match self.state {
            State::Pushing { .. } | State::Busy | State::Blocked { .. } if self.current.is_some() => {
                if let Some(current) = self.current.as_mut() {
                    current.cli_idle = true;
                    current.turn_ends += 1;
                    current.idle_since = Some(now);
                    actions.push(Action::Poll(current.task.clone()));
                }
                self.state = State::Busy;
            }
            State::BusyUser | State::Blocked { .. } | State::Starting { .. } => self.state = State::Idle,
            _ => {}
        }
        actions
    }

    /// The outcome of [`Action::TryPush`].
    #[allow(clippy::needless_pass_by_value, reason = "an outcome is consumed by the call")]
    pub fn on_push(&mut self, outcome: PushOutcome, now: Instant) -> Vec<Action<T>> {
        if !matches!(outcome, PushOutcome::Held | PushOutcome::NotReady) {
            self.held_since = None;
        }
        self.push_in_flight = false;
        let mut actions = Vec::new();
        match outcome {
            PushOutcome::Pushed | PushOutcome::Unknown | PushOutcome::PastedNotSubmitted => {
                self.consecutive_push_failures = 0;
                if let Some(head) = self.queue.pop_front() {
                    let task = head.item;
                    actions.push(Action::Progress {
                        task: task.clone(),
                        kind: "runner.pushed",
                        note: "typed into the session",
                        queue_position: None,
                    });
                    self.current = Some(Current {
                        task,
                        pushed_at: now,
                        acked: false,
                        cli_idle: false,
                        turn_ends: 0,
                        idle_since: None,
                        reminded_unacked_at_turn: None,
                        reminded_open_at_turn: None,
                        reminder_in_flight: false,
                        last_poll: now,
                        last_alive: now,
                        last_view: now,
                        blocked_reported: false,
                        failed_blocked: false,
                    });
                    // After a paste that timed out, or a paste whose Enter failed, Enter was not
                    // sent: press it now. That is the only thing done again; the text is never
                    // pasted a second time. The one Enter retry of the confirmation stays available.
                    self.state = State::Pushing {
                        since: now,
                        enter_retried: false,
                    };
                    if outcome != PushOutcome::Pushed {
                        actions.push(Action::EnterRetry);
                    }
                }
            }
            PushOutcome::NotReady => self.next_push_at = now + self.timers.push_retry,
            PushOutcome::Held => {
                self.next_push_at = now + self.timers.push_retry;
                let since = *self.held_since.get_or_insert(now);
                if now.duration_since(since) >= self.timers.screen_hold {
                    // A false reading of the screen must not stall the session for good: the
                    // waiting tasks fail as retryable, with the reason, and the clock starts over.
                    self.held_since = None;
                    let minutes = self.timers.screen_hold.as_secs() / 60;
                    let reason = self.fail_text(&format!(
                        "the screen of tmux session KEY showed the CLI busy or waiting for a choice for more \
                         than {minutes} minutes while its hooks said idle; attach to tmux session KEY and look"
                    ));
                    for entry in self.queue.drain_all() {
                        actions.push(Action::Fail {
                            task: entry.item,
                            reason: reason.clone(),
                            retryable: true,
                        });
                    }
                }
            }
            PushOutcome::Inactive => {
                self.held_since = None;
                self.queue.pop_front();
                self.next_push_at = now;
                actions.extend(self.drive(now));
            }
            PushOutcome::SessionGone => actions.extend(self.on_session_gone(now)),
            PushOutcome::Failed => {
                self.consecutive_push_failures += 1;
                self.next_push_at = now + self.timers.push_retry;
                if self.consecutive_push_failures >= 3
                    && let Some(head) = self.queue.pop_front()
                {
                    self.consecutive_push_failures = 0;
                    actions.push(Action::Fail {
                        task: head.item,
                        reason: self.fail_text("the task could not be typed into tmux session KEY"),
                        retryable: true,
                    });
                }
            }
        }
        actions
    }

    /// The liveness check failed: the session or the CLI is gone.
    pub fn on_session_gone(&mut self, now: Instant) -> Vec<Action<T>> {
        let mut actions = Vec::new();
        if let Some(current) = self.current.take() {
            actions.push(Action::Fail {
                task: current.task,
                reason: self.fail_text("tmux session KEY ended while the task was running"),
                retryable: true,
            });
        }
        self.cli_working = false;
        self.push_in_flight = false;
        self.ensuring = false;
        self.state = State::Dead;
        actions.extend(self.drive(now));
        actions
    }

    /// What Sylvode says about the current task.
    pub fn on_view(&mut self, facts: ViewFacts, now: Instant) -> Vec<Action<T>> {
        let Some(current) = self.current.as_mut() else {
            return Vec::new();
        };
        current.last_poll = now;
        current.last_view = now;
        if !facts.active {
            // Terminal in Sylvode: the session is free once the CLI is idle.
            let idle = current.cli_idle;
            self.current = None;
            self.state = if idle && !self.cli_working {
                State::Idle
            } else {
                State::BusyUser
            };
            return self.drive(now);
        }
        current.acked |= facts.acknowledged;
        // The agent acknowledged the task, so the turn has started, whatever the hooks said (a lost
        // `UserPromptSubmit` must not leave a working task `Pushing` or `Blocked`).
        if current.acked && matches!(self.state, State::Pushing { .. } | State::Blocked { .. }) {
            current.cli_idle = false;
            current.idle_since = None;
            current.blocked_reported = false;
            self.cli_working = true;
            self.state = State::Busy;
        }
        self.rules(now)
    }

    /// The outcome of [`Action::SendReminder`].
    pub fn on_reminder(&mut self, kind: Reminder, sent: bool, now: Instant) -> Vec<Action<T>> {
        let Some(current) = self.current.as_mut() else {
            return Vec::new();
        };
        current.reminder_in_flight = false;
        if !sent {
            return Vec::new();
        }
        match kind {
            Reminder::NotAcknowledged => current.reminded_unacked_at_turn = Some(current.turn_ends),
            Reminder::StillOpen => current.reminded_open_at_turn = Some(current.turn_ends),
        }
        current.last_alive = now;
        vec![Action::Progress {
            task: current.task.clone(),
            kind: "runner.reminded",
            note: "reminded the agent",
            queue_position: None,
        }]
    }

    /// The forgotten-acknowledge and forgotten-completion rules (design 7.5).
    fn rules(&mut self, now: Instant) -> Vec<Action<T>> {
        let (ack_reminder, report_unacked, grace) = (
            self.timers.ack_reminder,
            self.timers.report_unacked,
            self.timers.completion_grace,
        );
        let attach = self.attach_hint.clone();
        let Some(current) = self.current.as_mut() else {
            return Vec::new();
        };
        if self.state != State::Busy {
            return Vec::new();
        }
        let mut actions = Vec::new();
        let fail = |current: &Current<T>, reason: String, retryable: bool| Action::Fail {
            task: current.task.clone(),
            reason,
            retryable,
        };
        let mut finished = false;
        if current.acked {
            // Acknowledged, a turn ended, the task is still open.
            if current.cli_idle
                && current.turn_ends > 0
                && let Some(idle_since) = current.idle_since
                && now.duration_since(idle_since) >= grace
                // The view must be newer than the grace period: the model may be calling
                // `complete` right now.
                && current.last_view >= idle_since + grace
            {
                match (current.reminded_open_at_turn, current.reminder_in_flight) {
                    (None, false) => {
                        current.reminder_in_flight = true;
                        actions.push(Action::SendReminder(Reminder::StillOpen));
                    }
                    (Some(turn), _) if current.turn_ends > turn => {
                        actions.push(fail(
                            current,
                            format!(
                                "agent ended its turn without completing or failing the task; check the session with `{attach}`"
                            ),
                            false,
                        ));
                        finished = true;
                    }
                    _ => {}
                }
            }
        } else {
            let unacked_for = now.duration_since(current.pushed_at);
            if unacked_for >= report_unacked {
                actions.push(fail(current, "agent did not acknowledge the task".to_string(), true));
                finished = true;
            } else if current.cli_idle {
                match (current.reminded_unacked_at_turn, current.reminder_in_flight) {
                    (None, false) if current.turn_ends > 0 || unacked_for >= ack_reminder => {
                        current.reminder_in_flight = true;
                        actions.push(Action::SendReminder(Reminder::NotAcknowledged));
                    }
                    (Some(turn), _) if current.turn_ends > turn => {
                        actions.push(fail(current, "agent did not acknowledge the task".to_string(), true));
                        finished = true;
                    }
                    _ => {}
                }
            }
        }
        if finished {
            let idle = current.cli_idle;
            self.current = None;
            self.state = if idle && !self.cli_working {
                State::Idle
            } else {
                State::BusyUser
            };
        }
        actions
    }

    /// Time passed.
    pub fn on_tick(&mut self, now: Instant) -> Vec<Action<T>> {
        let mut actions = Vec::new();
        match self.state {
            State::Pushing { since, enter_retried } if now.duration_since(since) >= self.timers.push_confirm => {
                if enter_retried {
                    actions.extend(self.block(now));
                } else {
                    self.state = State::Pushing {
                        since: now,
                        enter_retried: true,
                    };
                    actions.push(Action::EnterRetry);
                }
            }
            State::Starting { since } => {
                actions.push(Action::CheckDialog);
                if now.duration_since(since) >= self.timers.ready_timeout {
                    let seconds = self.timers.ready_timeout.as_secs();
                    if let Some(head) = self.queue.pop_front() {
                        actions.push(Action::Fail {
                            task: head.item,
                            reason: format!("cli did not become ready within {seconds} s"),
                            retryable: true,
                        });
                    }
                    self.state = State::Starting { since: now };
                }
            }
            State::Blocked { since } => {
                actions.push(Action::CheckDialog);
                // A blocked task is still watched: when Sylvode ends it, the session is freed.
                if let Some(current) = self.current.as_mut()
                    && now.duration_since(current.last_poll) >= self.timers.poll
                {
                    current.last_poll = now;
                    actions.push(Action::Poll(current.task.clone()));
                }
                if now.duration_since(since) >= self.timers.blocked_fail
                    && let Some(current) = self.current.as_mut()
                    && !current.failed_blocked
                {
                    current.failed_blocked = true;
                    actions.push(Action::Fail {
                        task: current.task.clone(),
                        reason: self.fail_text(
                            "the CLI is waiting for a permission decision or dialog; attach to tmux session KEY",
                        ),
                        retryable: false,
                    });
                    // The session is kept; the task is over.
                    self.current = None;
                }
            }
            State::Busy | State::Pushing { .. } => {
                if let Some(current) = self.current.as_mut() {
                    if now.duration_since(current.last_poll) >= self.timers.poll {
                        current.last_poll = now;
                        actions.push(Action::Poll(current.task.clone()));
                    }
                    if now.duration_since(current.last_alive) >= self.timers.heartbeat {
                        current.last_alive = now;
                        actions.push(Action::Progress {
                            task: current.task.clone(),
                            kind: "runner.alive",
                            note: "the session is working on the task",
                            queue_position: None,
                        });
                    }
                }
                actions.extend(self.rules(now));
            }
            _ => {}
        }
        // Queued tasks keep their lease alive. Heartbeats stop in Blocked and Dead only for the
        // current task; a queued task is still waiting and still reports.
        if !self.queue.is_empty() && now.duration_since(self.last_queue_beat) >= self.timers.heartbeat {
            self.last_queue_beat = now;
            for (index, entry) in self.queue.iter().enumerate() {
                actions.push(Action::Progress {
                    task: entry.item.clone(),
                    kind: "runner.queued",
                    note: "queued behind other tasks of this session",
                    queue_position: i32::try_from(index + 1).ok(),
                });
            }
        }
        if !matches!(self.state, State::NoSession | State::Dead)
            && now.duration_since(self.last_liveness) >= self.timers.liveness
        {
            self.last_liveness = now;
            actions.push(Action::CheckLiveness);
        }
        actions.extend(self.drive(now));
        actions
    }

    /// Everything still queued or current, for shutdown.
    pub fn take_queued(&mut self) -> Vec<Entry<T>> {
        self.queue.drain_all()
    }

    /// The task being supervised.
    pub fn current_task(&self) -> Option<&T> {
        self.current.as_ref().map(|c| &c.task)
    }
}

/// Decides, from screen samples, that a turn the hooks never closed is over: claude does not run
/// its `Stop` hook when a person interrupts a turn, so without this the session would stay busy for
/// good. A turn is called over only when the last hook event is older than `quiet_for`, and
/// `needed` consecutive samples showed no busy marker and an empty, visible input line.
#[derive(Debug)]
pub struct TurnWatch {
    needed: u32,
    quiet_for: std::time::Duration,
    calm_samples: u32,
    last_event: Instant,
}

impl TurnWatch {
    pub const fn new(needed: u32, quiet_for: std::time::Duration, now: Instant) -> Self {
        Self {
            needed,
            quiet_for,
            calm_samples: 0,
            last_event: now,
        }
    }

    /// A hook event arrived: the turn state is known, start counting afresh.
    pub const fn event(&mut self, now: Instant) {
        self.last_event = now;
        self.calm_samples = 0;
    }

    /// One screen sample taken while a turn is open. `true` when the turn is over.
    pub fn sample(&mut self, now: Instant, busy: bool, input_empty: bool) -> bool {
        if busy || !input_empty {
            self.calm_samples = 0;
            return false;
        }
        self.calm_samples += 1;
        if self.calm_samples >= self.needed && now.duration_since(self.last_event) >= self.quiet_for {
            self.calm_samples = 0;
            return true;
        }
        false
    }
}
