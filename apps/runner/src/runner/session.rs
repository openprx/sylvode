//! The tmux channel: one worker per session (`<KEY>-<cli>`) that owns the session's queue and all
//! tmux I/O for it (design sections 1.3 and 3). The worker carries out what the pure machine
//! (`machine.rs`) decides, and feeds it what happens: hook events, poll results, push outcomes and
//! the clock.
//!
//! - **Use or create** (1.3): a session that exists is used as it is, whoever started it: the one
//!   pane running the CLI is the pane to type into. A session that does not exist is created by the
//!   runner. The runner never respawns, restarts or ends a session; [`decide_ensure`] is the pure
//!   rule.
//! - **Push** (6.5, 7.6): only when the CLI is idle, the prompt line is visible and empty, and no
//!   attached client typed within `user_idle_secs`. The text goes through the tmux driver's gate;
//!   its bracketed-paste confirmation is created here, per push, from the readiness check made
//!   right then (the foreground command is the CLI and its prompt is on the screen), never kept.
//! - **Hooks** (7.3): turn events arrive on the hook socket; a session without hooks (one the
//!   operator started) is read from the screen instead: a changing screen is a working CLI, a
//!   still screen with a prompt an idle one.
//! - **Liveness** (7.7) every 10 s and on every hook: `has-session`, `pane_dead`, the pane PID.
//!
//! Sessions keep running when the runner stops; nothing is killed on shutdown.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use parking_lot::Mutex;
use tokio::sync::{mpsc, oneshot};
use uuid::Uuid;

use super::checks::{Accepted, BotCreds};
use super::detect::{
    Dialog, awaits_choice, classify_bottom, classify_dialog, cut_for_log, input_is_empty, input_text, looks_busy,
    screens_equal,
};
use super::files::{CreatedBy, PushedTask, SessionRecord, StateDir};
use super::hooks::{HookEvent, HookMessage};
use super::launch::{self, LaunchFiles};
use super::machine::{
    Action, EnsureOutcome, Machine, PushOutcome, Reminder, State, Task, Timers, TurnWatch, ViewFacts,
};
use super::message::{reminder_not_acknowledged, reminder_still_open};
use super::sv_client::SvClient;
use super::tmux::{
    BracketedPasteConfirmed, ListedPane, Pane, PaneId, PasteGuard, Refusal, SessionName, SocketName, Tmux, TmuxError,
    WorkDir,
};
use crate::background::Background;
use crate::config::Config;

/// A task queued on a session, with what is needed to start the session and push it.
#[derive(Clone, Debug)]
pub struct TmuxTask {
    pub task_id: String,
    pub attempt: i64,
    pub max_attempts: i64,
    pub message: String,
    pub client: SvClient,
    pub bot: BotCreds,
    pub project_id: String,
    pub project_key: String,
    pub dir: PathBuf,
    pub cli: String,
    pub program: PathBuf,
    pub mcp_server: PathBuf,
}

impl Task for TmuxTask {
    fn id(&self) -> &str {
        &self.task_id
    }

    fn attempt(&self) -> i64 {
        self.attempt
    }
}

impl TmuxTask {
    /// From a delivery that passed the checks.
    pub fn from_accepted(accepted: &Accepted) -> Self {
        Self {
            task_id: accepted.task_id.clone(),
            attempt: accepted.attempt,
            max_attempts: accepted.max_attempts,
            message: accepted.message.clone(),
            client: accepted.client.clone(),
            bot: accepted.bot.clone(),
            project_id: accepted.project_id.clone(),
            project_key: accepted.project_key.clone(),
            dir: accepted.dir.clone(),
            cli: accepted.cli.clone(),
            program: accepted.program.clone(),
            mcp_server: accepted.mcp_server.clone(),
        }
    }
}

// ----- use or create --------------------------------------------------------------------------

/// What is known about a session when a task is about to be delivered.
#[derive(Debug)]
pub struct EnsureFacts<'a> {
    /// `has-session` says the session exists.
    pub session_exists: bool,
    /// Every pane of the session (empty when it does not exist).
    pub panes: &'a [ListedPane],
    /// The CLI's `process_names`.
    pub process_names: &'a [String],
    /// What the runner recorded about the session, if anything.
    pub record: Option<&'a SessionRecord>,
    /// The bot of the task's connection.
    pub task_bot_id: &'a str,
}

/// What to do with the session of a task (design 1.3, E1 to E3, and T4).
#[derive(Debug, PartialEq, Eq)]
pub enum EnsureDecision {
    /// E1: no such session; the runner creates it.
    Create,
    /// E2: use this pane as it is.
    Use { pane: ListedPane, created_by: CreatedBy },
    /// E3: the session has no pane running the CLI (a leftover shell, or a dead pane).
    NotRunning,
    /// E2: more than one pane runs the CLI.
    Ambiguous,
    /// T4: the session's record names another bot.
    BotMismatch,
}

/// The pane `record` names, when it is alive in `panes` with the recorded PID.
fn recorded_pane<'a>(record: &SessionRecord, panes: &'a [ListedPane]) -> Option<&'a ListedPane> {
    panes
        .iter()
        .find(|pane| !pane.dead && pane.id.as_str() == record.pane_id && pane.pid == record.pane_pid)
}

/// Design 1.3: which pane a task goes to, or what is wrong. Pure.
pub fn decide_ensure(facts: &EnsureFacts<'_>) -> EnsureDecision {
    if !facts.session_exists {
        return EnsureDecision::Create;
    }
    // A record whose pane is still there belongs to a live session; one whose pane is gone is stale
    // and says nothing about who serves the session now.
    let live_record = facts
        .record
        .filter(|record| recorded_pane(record, facts.panes).is_some());
    if live_record.is_some_and(|record| !record.bot_id.is_empty() && record.bot_id != facts.task_bot_id) {
        return EnsureDecision::BotMismatch;
    }
    let mut runs_cli = facts
        .panes
        .iter()
        .filter(|pane| !pane.dead && facts.process_names.contains(&pane.command));
    match (runs_cli.next(), runs_cli.next()) {
        (None, _) => EnsureDecision::NotRunning,
        (Some(_), Some(_)) => EnsureDecision::Ambiguous,
        (Some(pane), None) => {
            let by_runner = live_record
                .is_some_and(|record| record.created_by == CreatedBy::Runner && record.pane_id == pane.id.as_str());
            EnsureDecision::Use {
                pane: pane.clone(),
                created_by: if by_runner {
                    CreatedBy::Runner
                } else {
                    CreatedBy::Operator
                },
            }
        }
    }
}

/// The failure text of a decision that cannot be served; it names the session and never a path.
pub fn decline_for(decision: &EnsureDecision, name: &str) -> Option<(String, bool)> {
    match decision {
        EnsureDecision::NotRunning => Some((
            format!(
                "the CLI is not running in tmux session {name}; attach with tmux attach -t {name} and start it, \
                 or end the session so the runner starts it"
            ),
            true,
        )),
        EnsureDecision::Ambiguous => Some((
            format!("tmux session {name} has more than one pane running the CLI; close all but one"),
            false,
        )),
        EnsureDecision::BotMismatch => Some((
            format!("session_bot_mismatch: tmux session {name} serves another bot"),
            false,
        )),
        EnsureDecision::Create | EnsureDecision::Use { .. } => None,
    }
}

// ----- the manager ----------------------------------------------------------------------------

/// A task that was pushed before a restart and is supervised again.
#[derive(Debug)]
pub struct Adopt {
    pub task: Box<TmuxTask>,
    pub acknowledged: bool,
    pub blocked: bool,
}

/// What the runner finds in tmux for a recovered task.
#[derive(Debug, Default, Clone, Copy)]
pub struct Probe {
    pub alive: bool,
    pub scrollback_hit: bool,
    /// `session.json` records this task and attempt as typed in.
    pub recorded_pushed: bool,
}

enum Cmd {
    Recover {
        adopt: Option<Adopt>,
        requeue: Vec<TmuxTask>,
    },
    Enqueue {
        task: Box<TmuxTask>,
        reply: oneshot::Sender<usize>,
    },
    Hook {
        event: HookEvent,
        nonce: String,
        seq: u64,
        peer_chain: Option<Vec<u32>>,
    },
    View {
        task_id: String,
        attempt: i64,
        facts: Option<ViewFacts>,
    },
}

struct Shared {
    config: Arc<Config>,
    state: Arc<StateDir>,
    runner_exe: PathBuf,
    socket: SocketName,
    tmux_program: Option<PathBuf>,
    background: Background,
    /// The nonce of each session's current launch, for the hook listener (which checks a message
    /// before it spends anything on it), by session name.
    nonces: Arc<Mutex<HashMap<String, Vec<String>>>>,
}

/// The sessions of this runner.
pub struct SessionManager {
    shared: Arc<Shared>,
    workers: Mutex<HashMap<String, mpsc::Sender<Cmd>>>,
}

impl SessionManager {
    /// `tmux_program` is the tmux executable found on this runner.
    ///
    /// # Errors
    ///
    /// A message when the socket name is invalid.
    pub fn new(
        config: Arc<Config>,
        state: Arc<StateDir>,
        runner_exe: PathBuf,
        tmux_program: Option<PathBuf>,
        background: Background,
    ) -> Result<Self, String> {
        let socket = SocketName::new(&config.runner.tmux.socket_name).map_err(|e| e.to_string())?;
        Ok(Self {
            shared: Arc::new(Shared {
                config,
                state,
                runner_exe,
                socket,
                tmux_program,
                background,
                nonces: Arc::new(Mutex::new(HashMap::new())),
            }),
            workers: Mutex::new(HashMap::new()),
        })
    }

    /// The listener's pre-check: a message is worth reading further only when its session has a
    /// worker and it carries that launch's nonce.
    pub fn nonce_check(&self) -> super::hooks::Accept {
        let nonces = Arc::clone(&self.shared.nonces);
        Arc::new(move |message| {
            nonces
                .lock()
                .get(&message.session)
                .is_some_and(|known| known.iter().any(|nonce| super::hooks::ct_eq(nonce, &message.nonce)))
        })
    }

    fn sender(&self, name: &SessionName) -> mpsc::Sender<Cmd> {
        let (tx, rx) = mpsc::channel(256);
        {
            let mut workers = self.workers.lock();
            if let Some(sender) = workers.get(name.as_str()) {
                return sender.clone();
            }
            workers.insert(name.as_str().to_string(), tx.clone());
        }
        let worker = Worker::new(Arc::clone(&self.shared), name.clone(), rx, tx.clone());
        tokio::spawn(worker.run());
        tx
    }

    /// Queues a task on the session of its project and CLI and returns its 1-based queue position.
    pub async fn enqueue(&self, task: TmuxTask) -> Option<usize> {
        let name = SessionName::new(&task.project_key, &task.cli).ok()?;
        let (reply, answer) = oneshot::channel();
        self.sender(&name)
            .send(Cmd::Enqueue {
                task: Box::new(task),
                reply,
            })
            .await
            .ok()?;
        answer.await.ok()
    }

    /// A hook event from a CLI.
    pub async fn hook(&self, message: HookMessage) {
        let Ok(name) = SessionName::parse(&message.session) else {
            return;
        };
        // Only a session that has a worker (a task was queued or recovered) takes events.
        let sender = self.workers.lock().get(name.as_str()).cloned();
        if let Some(sender) = sender {
            let _ = sender
                .send(Cmd::Hook {
                    event: message.event,
                    nonce: message.nonce,
                    seq: message.seq,
                    peer_chain: message.peer_chain,
                })
                .await;
        }
    }

    /// Whether the session `name` has its recorded CLI pane alive and shows `id=<task_id>` in its
    /// last 200 lines. Reads tmux only.
    pub async fn probe(&self, name: &SessionName, task_id: &str, attempt: i64) -> Probe {
        let tmux = {
            let mut tmux = Tmux::new(self.shared.socket.clone());
            if let Some(program) = &self.shared.tmux_program {
                tmux = tmux.with_program(program);
            }
            tmux
        };
        let record = self.shared.state.read_session_record(name);
        let recorded_pushed = record.as_ref().is_some_and(|record| {
            record
                .pushed
                .as_ref()
                .is_some_and(|pushed| pushed.task_id == task_id && pushed.attempt == attempt)
        });
        let pane = record.as_ref().and_then(|record| pane_of(name, record));
        let name = name.clone();
        let needle = format!("id={task_id}");
        tokio::task::spawn_blocking(move || {
            let gone = Probe {
                recorded_pushed,
                ..Probe::default()
            };
            let Some(pane) = pane else { return gone };
            if !tmux.has_session(&name).unwrap_or(false) || !pane_alive(&tmux, &pane) {
                return gone;
            }
            let hit = tmux
                .capture_pane(&pane, Some(200))
                .is_ok_and(|screen| screen.contains(&needle));
            Probe {
                alive: true,
                scrollback_hit: hit,
                recorded_pushed,
            }
        })
        .await
        .unwrap_or_default()
    }

    /// Hands recovered tasks to the worker of `name`: the pushed one is supervised again, the rest
    /// are queued in the order given.
    pub async fn recover(&self, name: SessionName, adopt: Option<Adopt>, requeue: Vec<TmuxTask>) {
        let _ = self.sender(&name).send(Cmd::Recover { adopt, requeue }).await;
    }
}

/// The pane a record names (id and PID as recorded).
fn pane_of(name: &SessionName, record: &SessionRecord) -> Option<Pane> {
    Some(Pane {
        name: name.clone(),
        id: PaneId::new(&record.pane_id).ok()?,
        pid: record.pane_pid,
    })
}

/// The pane is alive and still the recorded one (session, PID).
fn pane_alive(tmux: &Tmux, pane: &Pane) -> bool {
    tmux.pane_state(pane)
        .is_ok_and(|state| !state.dead && state.session_name == pane.name.as_str() && state.pid == pane.pid)
}

// ----- the worker -----------------------------------------------------------------------------

struct Worker {
    shared: Arc<Shared>,
    name: SessionName,
    machine: Machine<TmuxTask>,
    pane: Option<Pane>,
    rx: mpsc::Receiver<Cmd>,
    tx: mpsc::Sender<Cmd>,
    /// The session has no hooks (the operator started it): turn events are read from the screen.
    screen_mode: bool,
    /// The secret of the current launch's hooks.
    nonce: String,
    last_screen: Option<String>,
    still_ticks: u32,
    screen_working: bool,
    /// The newest hook event applied, by the sender's clock: an older one is ignored.
    seq_gate: super::hooks::SeqGate,
    /// Closes a turn the hooks never closed (an interrupt runs no `Stop`).
    turn_watch: TurnWatch,
    ticks: u32,
}

/// Whether a hook message may be applied: it carries this launch's nonce, and (where the platform
/// names the sender) the sender runs below the session's pane process. Refuses with the reason.
///
/// # Errors
///
/// The reason for dropping the message.
pub fn admit_hook(
    own_nonce: &str,
    nonce: &str,
    chain: Option<&[u32]>,
    pane_pid: Option<u32>,
) -> Result<(), &'static str> {
    if !super::hooks::ct_eq(nonce, own_nonce) {
        return Err("wrong nonce");
    }
    if !super::hooks::chain_allows(chain, pane_pid) {
        return Err("not from the pane");
    }
    Ok(())
}

/// The path of this executable as hooks must name it. After an in-place upgrade of the binary on
/// Linux, `current_exe` reports `"<path> (deleted)"`; the new binary sits at the same path, so the
/// suffix is dropped, and the path must exist.
#[must_use]
pub fn stable_exe(raw: &Path) -> Option<PathBuf> {
    let text = raw.to_str()?;
    let path = PathBuf::from(text.strip_suffix(" (deleted)").unwrap_or(text));
    path.is_file().then_some(path)
}

/// How long after the last hook event a turn may be called over by calm screen samples.
const TURN_QUIET: Duration = Duration::from_mins(1);

/// Whether a recovered task is blocked again: its phase says `blocked` AND a dialog is really on
/// the screen. The phase is never re-reported once a dialog is answered, so on its own it is stale.
#[must_use]
pub const fn recovered_blocked(phase_blocked: bool, dialog_on_screen: bool) -> bool {
    phase_blocked && dialog_on_screen
}

/// The directory accepted at delivery is still the same canonical path (no link was swapped in
/// meanwhile). A session is created long after the delivery was checked.
pub fn dir_unchanged(dir: &Path) -> bool {
    std::fs::canonicalize(dir).is_ok_and(|now| now == dir)
}

/// A random secret of 32 hex characters (the 128 random bits of a version 4 UUID).
fn fresh_nonce() -> String {
    Uuid::new_v4().simple().to_string()
}

fn unix_now() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs())
}

impl Worker {
    fn new(shared: Arc<Shared>, name: SessionName, rx: mpsc::Receiver<Cmd>, tx: mpsc::Sender<Cmd>) -> Self {
        let socket = shared.socket.as_str();
        let attach = if socket.is_empty() {
            format!("tmux attach -t {}", name.as_str())
        } else {
            format!("tmux -L {socket} attach -t {}", name.as_str())
        };
        let nonce = shared
            .state
            .read_session_record(&name)
            .map(|record| record.nonce)
            .filter(|nonce| nonce.len() == 32)
            .unwrap_or_else(fresh_nonce);
        shared
            .nonces
            .lock()
            .insert(name.as_str().to_string(), vec![nonce.clone()]);
        let machine = Machine::new(
            Timers::from_config(&shared.config.runner),
            name.as_str(),
            &attach,
            Instant::now(),
        );
        Self {
            shared,
            name,
            machine,
            pane: None,
            rx,
            tx,
            screen_mode: false,
            nonce,
            last_screen: None,
            still_ticks: 0,
            screen_working: false,
            seq_gate: super::hooks::SeqGate::default(),
            turn_watch: TurnWatch::new(3, TURN_QUIET, Instant::now()),
            ticks: 0,
        }
    }

    fn tmux(&self) -> Tmux {
        let mut tmux = Tmux::new(self.shared.socket.clone());
        if let Some(program) = &self.shared.tmux_program {
            tmux = tmux.with_program(program);
        }
        tmux
    }

    async fn run(mut self) {
        let mut ticker = tokio::time::interval(Duration::from_secs(1));
        let cancelled = self.shared.background.cancelled();
        tokio::pin!(cancelled);
        loop {
            let actions = tokio::select! {
                () = &mut cancelled => return,
                _ = ticker.tick() => {
                    let mut actions = self.read_screen_signals().await;
                    actions.extend(self.confirm_turn_by_screen().await);
                    actions.extend(self.close_stale_turn().await);
                    actions.extend(self.machine.on_tick(Instant::now()));
                    actions
                }
                command = self.rx.recv() => match command {
                    None => return,
                    Some(command) => self.handle(command).await,
                },
            };
            self.carry_out(actions).await;
        }
    }

    async fn handle(&mut self, command: Cmd) -> Vec<Action<TmuxTask>> {
        let now = Instant::now();
        match command {
            Cmd::Recover { adopt, requeue } => {
                let mut actions = Vec::new();
                if let Some(adopt) = adopt {
                    let record = self.shared.state.read_session_record(&self.name);
                    let pane = record
                        .as_ref()
                        .filter(|record| record.project_id == adopt.task.project_id)
                        .and_then(|record| pane_of(&self.name, record));
                    let tmux = self.tmux();
                    let alive = match pane.clone() {
                        Some(pane) => tokio::task::spawn_blocking(move || pane_alive(&tmux, &pane))
                            .await
                            .unwrap_or(false),
                        None => false,
                    };
                    if let (Some(pane), Some(record), true) = (pane, record, alive) {
                        self.pane = Some(pane);
                        // A session the operator started has no hooks: the screen tells the turns.
                        self.screen_mode = !self.hooks_expected(&record);
                        self.machine.adopt_running(now);
                        // The `blocked` phase can be stale (nothing re-reports `pushed` after a
                        // dialog is answered): the session is blocked again only if a dialog is
                        // really on the screen.
                        let dialog_shown = if adopt.blocked {
                            let cli = adopt.task.cli.clone();
                            self.capture()
                                .await
                                .is_some_and(|screen| classify_bottom(&cli, &screen).is_some())
                        } else {
                            false
                        };
                        let blocked = recovered_blocked(adopt.blocked, dialog_shown);
                        self.machine
                            .adopt_current(*adopt.task, adopt.acknowledged, blocked, now);
                    } else {
                        // It was pushed, so it is not typed again: its session cannot be taken
                        // over, and the task fails as retryable (design 7.8).
                        let reason = format!("runner restarted and session {} was gone", self.name.as_str());
                        actions.push(Action::Fail {
                            task: *adopt.task,
                            reason,
                            retryable: true,
                        });
                    }
                }
                for task in requeue {
                    actions.extend(self.machine.enqueue(task, now));
                }
                actions
            }
            Cmd::Enqueue { task, reply } => {
                let id = task.task_id.clone();
                let actions = self.machine.enqueue(*task, now);
                let _ = reply.send(self.machine.queue.position(&id).unwrap_or(0));
                actions
            }
            Cmd::Hook {
                event,
                nonce,
                seq,
                peer_chain,
            } => {
                // The message must carry this launch's secret and come from below this session's
                // pane process; a session whose record cannot be read admits nothing.
                let pane_pid = self.shared.state.read_session_record(&self.name).map(|r| r.pane_pid);
                if let Err(why) = admit_hook(&self.nonce, &nonce, peer_chain.as_deref(), pane_pid) {
                    tracing::debug!(session = self.name.as_str(), "hook message dropped: {why}");
                    return Vec::new();
                }
                // Events are applied in the order their senders stamped them: a late one is stale.
                if !self.seq_gate.accept(seq) {
                    tracing::debug!(session = self.name.as_str(), "hook message dropped: out of order");
                    return Vec::new();
                }
                self.turn_watch.event(now);
                self.screen_mode = false;
                self.machine.on_hook(event, now)
            }
            Cmd::View {
                task_id,
                attempt,
                facts,
            } => {
                // A view of an older attempt must not free the session while a newer one runs.
                if self
                    .machine
                    .current_task()
                    .is_some_and(|t| t.task_id == task_id && t.attempt == attempt)
                    && let Some(facts) = facts
                {
                    self.machine.on_view(facts, now)
                } else {
                    Vec::new()
                }
            }
        }
    }

    /// Executes actions; the follow-ups of an action are executed too.
    async fn carry_out(&mut self, actions: Vec<Action<TmuxTask>>) {
        let mut pending: std::collections::VecDeque<Action<TmuxTask>> = actions.into();
        let mut guard = 0;
        while let Some(action) = pending.pop_front() {
            guard += 1;
            if guard > 200 {
                tracing::error!("a session worker produced too many follow-up actions; stopping the round");
                return;
            }
            let follow = self.execute(action).await;
            pending.extend(follow);
        }
    }

    async fn execute(&mut self, action: Action<TmuxTask>) -> Vec<Action<TmuxTask>> {
        let now = Instant::now();
        match action {
            Action::EnsureSession => match self.machine.queue.front().map(|e| e.item.clone()) {
                Some(task) => {
                    let outcome = self.ensure(&task).await;
                    self.machine.on_ensure(outcome, now)
                }
                None => self.machine.on_ensure(EnsureOutcome::Running, now),
            },
            Action::TryPush(task) => {
                let outcome = self.try_push(&task).await;
                if let PushTry::Dialog(dialog) = &outcome {
                    let mut actions = self.machine.on_push(PushOutcome::NotReady, now);
                    actions.extend(self.machine.on_dialog(*dialog, now));
                    return actions;
                }
                self.machine.on_push(outcome.into_outcome(), now)
            }
            Action::EnterRetry => {
                let Some(pane) = self.pane.clone() else {
                    return Vec::new();
                };
                let tmux = self.tmux();
                let cli = self
                    .machine
                    .current_task()
                    .map(|t| t.cli.clone())
                    .or_else(|| self.recorded_cli());
                let names = cli
                    .as_deref()
                    .map(|cli| process_names(&self.shared.config, cli))
                    .unwrap_or_default();
                let user_idle = self.shared.config.runner.tmux.user_idle_secs;
                let result = tokio::task::spawn_blocking(move || {
                    if !enter_retry_allowed(&tmux, &pane, cli.as_deref().unwrap_or("claude"), &names, user_idle) {
                        return false;
                    }
                    tmux.retry_enter(&pane).is_ok()
                })
                .await;
                if result.is_ok_and(|pressed| pressed) {
                    return Vec::new();
                }
                // Nothing was pressed: the state of the input line is unknown, so the session is
                // reported blocked rather than guessed at.
                tracing::warn!(session = self.name.as_str(), "no second Enter: the gate did not pass");
                self.machine.on_enter_refused(now)
            }
            Action::SendReminder(kind) => {
                let sent = self.send_reminder(kind).await;
                self.machine.on_reminder(kind, sent, now)
            }
            Action::Progress {
                task,
                kind,
                note,
                queue_position,
            } => {
                let timeout = self.request_timeout();
                tokio::spawn(async move {
                    let _ = task
                        .client
                        .progress(&task.task_id, task.attempt, kind, note, queue_position, timeout)
                        .await;
                });
                Vec::new()
            }
            Action::Fail {
                task,
                reason,
                retryable,
            } => {
                let timeout = self.request_timeout();
                tokio::spawn(fail_if_active(task, reason, retryable, timeout));
                Vec::new()
            }
            Action::Poll(task) => {
                let timeout = self.request_timeout();
                let tx = self.tx.clone();
                tokio::spawn(async move {
                    let facts = task
                        .client
                        .task_view(&task.task_id, task.attempt, timeout)
                        .await
                        .ok()
                        .map(|view| view_facts(&view));
                    let _ = tx
                        .send(Cmd::View {
                            task_id: task.task_id.clone(),
                            attempt: task.attempt,
                            facts,
                        })
                        .await;
                });
                Vec::new()
            }
            Action::CheckLiveness => {
                if self.alive().await {
                    Vec::new()
                } else {
                    self.machine.on_session_gone(now)
                }
            }
            Action::CheckDialog => self.check_screen_while_starting().await,
        }
    }

    fn request_timeout(&self) -> Duration {
        Duration::from_secs(self.shared.config.runner.delivery_check_timeout_secs)
    }

    // ----- liveness -----

    /// Whether hooks are expected from the session of `record`: the runner started it with its
    /// hook settings.
    fn hooks_expected(&self, record: &SessionRecord) -> bool {
        record.created_by == CreatedBy::Runner && self.shared.config.runner.cli.claude.hooks
    }

    /// The recorded pane of this session, if a record names one.
    fn recorded_pane(&self) -> Option<Pane> {
        self.shared
            .state
            .read_session_record(&self.name)
            .and_then(|record| pane_of(&self.name, &record))
    }

    async fn alive(&self) -> bool {
        let Some(pane) = self.pane.clone() else {
            // No pane is known yet; a session that exists is looked for once.
            let tmux = self.tmux();
            let name = self.name.clone();
            return tokio::task::spawn_blocking(move || tmux.has_session(&name).unwrap_or(false))
                .await
                .unwrap_or(false);
        };
        let tmux = self.tmux();
        tokio::task::spawn_blocking(move || tmux.has_session(&pane.name).unwrap_or(false) && pane_alive(&tmux, &pane))
            .await
            .unwrap_or(false)
    }

    // ----- the screen -----

    async fn capture(&self) -> Option<String> {
        let pane = self.pane.clone()?;
        let tmux = self.tmux();
        tokio::task::spawn_blocking(move || tmux.capture_pane(&pane, None).ok())
            .await
            .ok()
            .flatten()
    }

    /// While the CLI starts: a blocking dialog fails the waiting tasks; without a start hook (a
    /// CLI other than claude, or a session without hooks), a still screen with a prompt means it
    /// is ready.
    async fn check_screen_while_starting(&mut self) -> Vec<Action<TmuxTask>> {
        let now = Instant::now();
        let cli = self
            .machine
            .queue
            .front()
            .map(|e| e.item.cli.clone())
            .or_else(|| self.recorded_cli());
        let (Some(cli), Some(screen)) = (cli, self.capture().await) else {
            return Vec::new();
        };
        if let Some(dialog) = classify_dialog(&cli, &screen) {
            tracing::debug!(screen = %cut_for_log(&screen), "blocking dialog on screen");
            return self.machine.on_dialog(dialog, now);
        }
        if cli != "claude" || self.screen_mode {
            let still = self
                .last_screen
                .as_deref()
                .is_some_and(|before| screens_equal(before, &screen));
            self.last_screen = Some(screen.clone());
            self.still_ticks = if still { self.still_ticks + 1 } else { 0 };
            if self.still_ticks >= 2 && input_is_empty(&cli, &screen).is_some() {
                self.still_ticks = 0;
                return self.machine.on_ready(now);
            }
        }
        Vec::new()
    }

    fn recorded_cli(&self) -> Option<String> {
        self.shared.state.read_session_record(&self.name).map(|r| r.cli)
    }

    /// For a session without hooks, the turn events come from the screen: it changes while the CLI
    /// works and stays still, with the prompt showing, when it is idle.
    /// A turn whose `UserPromptSubmit` was lost: while a pushed task is `Pushing` or `Blocked`, a
    /// screen that shows the CLI at work is proof that the turn started, so the task is `Busy` and
    /// is neither pressed with Enter again nor failed as a permission wait.
    async fn confirm_turn_by_screen(&mut self) -> Vec<Action<TmuxTask>> {
        let waiting = matches!(self.machine.state, State::Pushing { .. } | State::Blocked { .. })
            && self.machine.current_task().is_some();
        if !waiting || self.screen_mode || self.pane.is_none() || !self.ticks.is_multiple_of(2) {
            return Vec::new();
        }
        let Some(cli) = self.recorded_cli() else {
            return Vec::new();
        };
        let Some(screen) = self.capture().await else {
            return Vec::new();
        };
        if !looks_busy(&cli, &screen) {
            return Vec::new();
        }
        let now = Instant::now();
        self.turn_watch.event(now);
        self.machine.on_hook(HookEvent::UserPromptSubmit, now)
    }

    /// A turn the hooks opened and never closed (claude runs no `Stop` when a person interrupts) is
    /// closed when the screen has been calm long enough: no busy marker, an empty visible input
    /// line, and no hook event for a minute.
    async fn close_stale_turn(&mut self) -> Vec<Action<TmuxTask>> {
        self.ticks = self.ticks.wrapping_add(1);
        if self.screen_mode || !self.machine.turn_open() || self.pane.is_none() || !self.ticks.is_multiple_of(5) {
            return Vec::new();
        }
        let Some(cli) = self.recorded_cli() else {
            return Vec::new();
        };
        let Some(screen) = self.capture().await else {
            return Vec::new();
        };
        let busy = looks_busy(&cli, &screen) || awaits_choice(&cli, &screen);
        let empty = input_is_empty(&cli, &screen) == Some(true);
        let now = Instant::now();
        if self.turn_watch.sample(now, busy, empty) {
            tracing::warn!(
                session = self.name.as_str(),
                "a turn ended without a Stop hook; closing it"
            );
            return self.machine.on_hook(HookEvent::Stop, now);
        }
        Vec::new()
    }

    async fn read_screen_signals(&mut self) -> Vec<Action<TmuxTask>> {
        // A CLI that is still starting draws its screen: that is no turn.
        if !self.screen_mode || self.pane.is_none() || matches!(self.machine.state, State::Starting { .. }) {
            return Vec::new();
        }
        let Some(cli) = self.recorded_cli() else {
            return Vec::new();
        };
        let Some(screen) = self.capture().await else {
            return Vec::new();
        };
        let now = Instant::now();
        let still = self
            .last_screen
            .as_deref()
            .is_some_and(|before| screens_equal(before, &screen));
        self.last_screen = Some(screen.clone());
        let mut actions = Vec::new();
        if !still && !self.screen_working {
            self.screen_working = true;
            self.still_ticks = 0;
            actions.extend(self.machine.on_hook(HookEvent::UserPromptSubmit, now));
        } else if still {
            self.still_ticks += 1;
            if self.screen_working && self.still_ticks >= 3 && input_is_empty(&cli, &screen).is_some() {
                self.screen_working = false;
                actions.extend(self.machine.on_hook(HookEvent::Stop, now));
            }
        }
        actions
    }

    // ----- ensure -----

    /// The listener knows exactly this worker's current secret.
    fn publish_nonce(&self) {
        self.shared
            .nonces
            .lock()
            .insert(self.name.as_str().to_string(), vec![self.nonce.clone()]);
    }

    /// A launch began: its nonce is published to the listener and the order counter starts over.
    fn launched(&mut self) {
        self.publish_nonce();
        self.seq_gate = super::hooks::SeqGate::default();
    }

    fn settings_job(&self, task: &TmuxTask) -> SettingsJob {
        SettingsJob {
            state: Arc::clone(&self.shared.state),
            name: self.name.clone(),
            enabled: task.cli == "claude" && self.shared.config.runner.cli.claude.hooks,
            runner_exe: self.shared.runner_exe.clone(),
        }
    }

    /// Writes the bot's MCP files (every delivery, as the design asks) and, for a session that is
    /// to be created, what the launch needs. The launch part is a `Result` so that a session that
    /// already exists is used even when the launch command cannot be resolved.
    fn prepare(&self, task: &TmuxTask) -> Result<Create, String> {
        let config = &self.shared.config;
        let state = &self.shared.state;
        state
            .write_bot_files(&task.bot.mcp_files(&config.runner), &task.mcp_server)
            .map_err(|e| {
                tracing::error!("could not write the MCP files of bot {}: {e}", task.bot.bot_id);
                "the runner could not prepare the MCP configuration".to_string()
            })?;
        Ok(self.launch_plan(task))
    }

    /// The launch of a session the runner creates: the working directory and the argument vector.
    fn launch_plan(&self, task: &TmuxTask) -> Create {
        let state = &self.shared.state;
        let claude_json = state.claude_mcp_json(&task.bot.bot_id);
        let settings = state.claude_settings(&self.name);
        let socket = state.hook_socket();
        let files = LaunchFiles {
            claude_mcp_json: &claude_json,
            claude_settings: &settings,
            runner_exe: &self.shared.runner_exe,
            hook_socket: &socket,
            nonce: &self.nonce,
        };
        let claude = &self.shared.config.runner.cli.claude;
        let program = claude
            .launch_command
            .first()
            .and_then(|word| sylvode_prereq::Checker::new(sylvode_prereq::InstallKind::RunnerTmux).locate(word))
            .ok_or_else(|| "the launch command of the CLI was not found on this runner".to_string())?;
        let argv = launch::session_argv(&program, claude, &files).map_err(|e| {
            tracing::error!("launch refused for session {}: {e}", self.name.as_str());
            "the runner's launch command for this CLI cannot be used".to_string()
        })?;
        let dir = WorkDir::new(&task.dir)
            .map_err(|_| "the working directory cannot be used for a tmux session".to_string())?;
        Ok((dir, argv))
    }

    async fn ensure(&mut self, task: &TmuxTask) -> EnsureOutcome {
        let declined = |reason: &str, retryable: bool| EnsureOutcome::Declined {
            reason: reason.to_string(),
            retryable,
        };
        let create = match self.prepare(task) {
            Ok(create) => create,
            Err(reason) => return declined(&reason, false),
        };
        let process_names = process_names(&self.shared.config, &task.cli);
        let tmux = self.tmux();
        let name = self.name.clone();
        let tmux_cfg = self.shared.config.runner.tmux.clone();
        let record = self.shared.state.read_session_record(&name);
        let task = task.clone();
        let job = self.settings_job(&task);
        // The launch's secret is known to the listener before the CLI starts: its first hook
        // (`SessionStart`) may arrive before this call returns.
        let launch_nonce = fresh_nonce();
        self.shared
            .nonces
            .lock()
            .entry(self.name.as_str().to_string())
            .or_default()
            .push(launch_nonce.clone());
        let result = tokio::task::spawn_blocking(move || {
            ensure_blocking(
                &tmux,
                &name,
                &task,
                create,
                &process_names,
                record.as_ref(),
                &job,
                &tmux_cfg,
                &launch_nonce,
            )
        })
        .await;
        let Ok(result) = result else {
            self.publish_nonce();
            return declined("the tmux session could not be checked", true);
        };
        match result {
            Ok(EnsureResult {
                outcome,
                pane,
                record,
                launched,
            }) => {
                self.pane = pane;
                if launched && let Some(record) = &record {
                    // A launch got a secret of its own; a session found running keeps its old one.
                    self.nonce.clone_from(&record.nonce);
                    self.launched();
                } else {
                    // No launch took place: the pending secret is forgotten.
                    self.publish_nonce();
                }
                if let Some(record) = record {
                    self.screen_mode = !self.hooks_expected(&record);
                    if let Err(e) = self.shared.state.write_session_record(&record) {
                        tracing::warn!("could not save session.json of {}: {e}", self.name.as_str());
                    }
                }
                outcome
            }
            Err(e) => {
                self.publish_nonce();
                tracing::error!("tmux session {}: {e}", self.name.as_str());
                EnsureOutcome::Declined {
                    reason: format!("the runner could not start tmux session {}", self.name.as_str()),
                    retryable: true,
                }
            }
        }
    }

    // ----- push -----

    async fn try_push(&mut self, task: &TmuxTask) -> PushTry {
        let Some(pane) = self.pane.clone() else {
            // Recovered without a pane: the recorded one, if it is there.
            return match self.recorded_pane() {
                Some(pane) => {
                    self.pane = Some(pane);
                    PushTry::NotReady
                }
                None => PushTry::SessionGone,
            };
        };
        let tmux = self.tmux();
        let names = process_names(&self.shared.config, &task.cli);
        let cli = task.cli.clone();
        let user_idle = self.shared.config.runner.tmux.user_idle_secs;
        let settle = Duration::from_millis(self.shared.config.runner.tmux.push_settle_ms);
        let task_id = Uuid::parse_str(&task.task_id).unwrap_or_else(|_| Uuid::new_v4());
        let message = task.message.clone();
        // Sylvode must still show the task active: a task swept or answered while it waited in the
        // queue is never typed.
        let timeout = self.request_timeout();
        match task.client.task_view(&task.task_id, task.attempt, timeout).await {
            Ok(view) if !view_facts(&view).active => return PushTry::Inactive,
            Err(super::sv_client::SvError::Status(400)) => return PushTry::Inactive,
            Ok(_) | Err(_) => {}
        }
        // Durable before the paste: restart recovery must never type this task a second time.
        let pushed = PushedTask {
            task_id: task.task_id.clone(),
            attempt: task.attempt,
        };
        if let Err(e) = self.shared.state.mark_pushed(&self.name, Some(pushed)) {
            tracing::warn!(
                session = self.name.as_str(),
                "nothing is typed: the push could not be recorded: {e}"
            );
            return PushTry::Failed;
        }
        let result = tokio::task::spawn_blocking(move || {
            push_blocking(&tmux, &pane, &cli, &names, user_idle, settle, &task_id, &message)
        })
        .await
        .unwrap_or(PushTry::Failed);
        if matches!(
            result,
            PushTry::NotReady | PushTry::Dialog(_) | PushTry::SessionGone | PushTry::Failed
        ) && let Err(e) = self.shared.state.mark_pushed(&self.name, None)
        {
            tracing::warn!(session = self.name.as_str(), "could not clear the push record: {e}");
        }
        result
    }

    async fn send_reminder(&self, kind: Reminder) -> bool {
        let (Some(pane), Some(task)) = (self.pane.clone(), self.machine.current_task().cloned()) else {
            return false;
        };
        let id = Uuid::parse_str(&task.task_id).unwrap_or_else(|_| Uuid::new_v4());
        let attempt = u32::try_from(task.attempt).unwrap_or(1);
        let text = match kind {
            Reminder::NotAcknowledged => reminder_not_acknowledged(&id, attempt),
            Reminder::StillOpen => reminder_still_open(&id, attempt),
        };
        let tmux = self.tmux();
        let names = process_names(&self.shared.config, &task.cli);
        let cli = task.cli.clone();
        let user_idle = self.shared.config.runner.tmux.user_idle_secs;
        let settle = Duration::from_millis(self.shared.config.runner.tmux.push_settle_ms);
        // A reminder is a push of its own: the same preconditions, a buffer of its own.
        let result = tokio::task::spawn_blocking(move || {
            push_blocking(&tmux, &pane, &cli, &names, user_idle, settle, &Uuid::new_v4(), &text)
        })
        .await;
        tracing::debug!(session = self.name.as_str(), ?result, "reminder push");
        matches!(
            result,
            Ok(PushTry::Pushed | PushTry::Unknown | PushTry::PastedNotSubmitted)
        )
    }
}

/// Writes the hook settings of one launch with that launch's nonce. Run just before the CLI is
/// started in a new session, so every launch has a secret of its own; a session that is only used
/// keeps the one it was started with.
struct SettingsJob {
    state: Arc<StateDir>,
    name: SessionName,
    /// Hooks are on (claude, `[runner.cli.claude] hooks`).
    enabled: bool,
    runner_exe: PathBuf,
}

impl SettingsJob {
    fn write(&self, nonce: &str) -> Result<(), TmuxError> {
        if !self.enabled {
            return Ok(());
        }
        // A new launch counts its hooks from the start.
        let _ = std::fs::remove_file(
            self.state
                .root()
                .join("sessions")
                .join(self.name.as_str())
                .join("hook.seq"),
        );
        let claude_json = self.state.root().join("unused.json");
        let settings = self.state.claude_settings(&self.name);
        let socket = self.state.hook_socket();
        let files = LaunchFiles {
            claude_mcp_json: &claude_json,
            claude_settings: &settings,
            runner_exe: &self.runner_exe,
            hook_socket: &socket,
            nonce,
        };
        self.state
            .write_session_settings(&self.name, &files)
            .map(|_| ())
            .map_err(|e| TmuxError::Invalid(format!("could not write the hook settings: {e}")))
    }
}

/// How a session the runner creates is started: its working directory and argument vector, or why
/// they cannot be had.
type Create = Result<(WorkDir, Vec<String>), String>;

/// The names that identify the CLI's pane.
fn process_names(config: &Config, cli: &str) -> Vec<String> {
    match cli {
        "claude" => config.runner.cli.claude.process_names.clone(),
        other => vec![other.to_string()],
    }
}

/// What Sylvode says about a task view.
pub fn view_facts(view: &serde_json::Value) -> ViewFacts {
    ViewFacts {
        active: view.get("active").and_then(serde_json::Value::as_bool) != Some(false),
        acknowledged: view.get("acknowledged_at").is_some_and(|v| !v.is_null())
            || view.get("status").and_then(serde_json::Value::as_str) == Some("acknowledged"),
    }
}

/// Ends a task with `source: "runner"`, but only when Sylvode still shows it active.
async fn fail_if_active(task: TmuxTask, reason: String, retryable: bool, timeout: Duration) {
    let still_active = task
        .client
        .task_view(&task.task_id, task.attempt, timeout)
        .await
        .map_or(true, |view| view_facts(&view).active);
    if still_active
        && let Err(e) = task
            .client
            .fail(&task.task_id, task.attempt, &reason, retryable, timeout)
            .await
    {
        tracing::warn!("could not fail a task: {e}");
    }
}

struct EnsureResult {
    outcome: EnsureOutcome,
    pane: Option<Pane>,
    record: Option<SessionRecord>,
    /// The runner started the CLI just now.
    launched: bool,
}

/// Use or create (design 1.3). Nothing here restarts, respawns or ends anything.
#[allow(clippy::too_many_arguments, reason = "one call gathers the facts of one session")]
fn ensure_blocking(
    tmux: &Tmux,
    name: &SessionName,
    task: &TmuxTask,
    create: Create,
    process_names: &[String],
    record: Option<&SessionRecord>,
    job: &SettingsJob,
    cfg: &super::config::RunnerTmuxConfig,
    launch_nonce: &str,
) -> Result<EnsureResult, TmuxError> {
    let declined = |reason: String, retryable: bool| EnsureResult {
        outcome: EnsureOutcome::Declined { reason, retryable },
        pane: None,
        record: None,
        launched: false,
    };
    let new_record = |pane_id: &PaneId, pid: u32, created_by: CreatedBy, nonce: &str| SessionRecord {
        name: name.as_str().to_string(),
        project_id: task.project_id.clone(),
        bot_id: task.bot.bot_id.clone(),
        cli: task.cli.clone(),
        pane_id: pane_id.as_str().to_string(),
        pane_pid: pid,
        dir: task.dir.to_string_lossy().into_owned(),
        created_by,
        nonce: nonce.to_string(),
        pushed: None,
    };
    let exists = tmux.has_session(name)?;
    let panes = if exists { tmux.list_panes(name)? } else { Vec::new() };
    let decision = decide_ensure(&EnsureFacts {
        session_exists: exists,
        panes: &panes,
        process_names,
        record,
        task_bot_id: &task.bot.bot_id,
    });
    if let Some((reason, retryable)) = decline_for(&decision, name.as_str()) {
        return Ok(declined(reason, retryable));
    }
    match decision {
        EnsureDecision::Create => {
            let (dir, argv) = match create {
                Ok(create) => create,
                Err(reason) => return Ok(declined(reason, false)),
            };
            // The session is created long after the delivery was checked: the directory must still
            // be the canonical path that was accepted (no link swapped in meanwhile).
            if !dir_unchanged(&task.dir) {
                return Ok(declined(
                    "the working directory changed since the task was accepted".to_string(),
                    false,
                ));
            }
            let created = job
                .write(launch_nonce)
                .and_then(|()| tmux.new_session(name, &dir, cfg.window_width, cfg.window_height, &argv));
            let pane = match created {
                Ok(pane) => pane,
                Err(e) => {
                    tracing::error!("tmux session {} was not created: {e}", name.as_str());
                    return Ok(declined(
                        format!("the runner could not start tmux session {}", name.as_str()),
                        true,
                    ));
                }
            };
            let record = new_record(&pane.id, pane.pid, CreatedBy::Runner, launch_nonce);
            Ok(EnsureResult {
                outcome: EnsureOutcome::Started,
                pane: Some(pane),
                record: Some(record),
                launched: true,
            })
        }
        EnsureDecision::Use { pane, created_by } => {
            // A session the runner started keeps the secret its hooks were written with.
            let previous = record;
            let nonce = previous
                .filter(|r| created_by == CreatedBy::Runner && r.pane_id == pane.id.as_str())
                .map_or("", |r| r.nonce.as_str());
            let mut record = new_record(&pane.id, pane.pid, created_by, nonce);
            // The same pane keeps what is recorded of the task typed into it.
            record.pushed = previous
                .filter(|r| r.pane_id == pane.id.as_str() && r.pane_pid == pane.pid)
                .and_then(|r| r.pushed.clone());
            Ok(EnsureResult {
                outcome: EnsureOutcome::Running,
                pane: Some(Pane {
                    name: name.clone(),
                    id: pane.id,
                    pid: pane.pid,
                }),
                record: Some(record),
                launched: false,
            })
        }
        EnsureDecision::NotRunning | EnsureDecision::Ambiguous | EnsureDecision::BotMismatch => {
            Ok(declined("the session cannot be used".to_string(), false))
        }
    }
}

/// The gate of the one Enter retry: the same preconditions as a push, plus the pasted text still
/// being the last thing on the input line. Pressing Enter blind could submit a person's draft or
/// confirm a dialog's default.
fn enter_retry_allowed(tmux: &Tmux, pane: &Pane, cli: &str, process_names: &[String], user_idle_secs: u64) -> bool {
    let Ok(screen) = tmux.capture_pane(pane, None) else {
        return false;
    };
    if classify_bottom(cli, &screen).is_some() {
        return false;
    }
    if !input_text(cli, &screen).is_some_and(|text| text.starts_with("[Sylvode")) {
        return false;
    }
    let now = unix_now();
    if tmux
        .client_activity(&pane.name)
        .unwrap_or_default()
        .iter()
        .any(|activity| now.saturating_sub(*activity) < user_idle_secs)
    {
        return false;
    }
    tmux.pane_info(pane)
        .is_ok_and(|info| !info.dead && process_names.contains(&info.current_command))
}

/// How a push attempt ended.
#[derive(Debug)]
enum PushTry {
    Inactive,
    Held,
    Pushed,
    Unknown,
    PastedNotSubmitted,
    NotReady,
    SessionGone,
    Failed,
    Dialog(Dialog),
}

impl PushTry {
    const fn into_outcome(self) -> PushOutcome {
        match self {
            Self::Pushed => PushOutcome::Pushed,
            Self::Unknown => PushOutcome::Unknown,
            Self::PastedNotSubmitted => PushOutcome::PastedNotSubmitted,
            Self::NotReady | Self::Dialog(_) => PushOutcome::NotReady,
            Self::SessionGone => PushOutcome::SessionGone,
            Self::Failed => PushOutcome::Failed,
            Self::Inactive => PushOutcome::Inactive,
            Self::Held => PushOutcome::Held,
        }
    }
}

/// The preconditions of a push (design 6.5 step 5 and 7.6), then the push itself.
#[allow(clippy::too_many_arguments, reason = "one call gathers the facts of one push")]
fn push_blocking(
    tmux: &Tmux,
    pane: &Pane,
    cli: &str,
    process_names: &[String],
    user_idle_secs: u64,
    settle: Duration,
    task_id: &Uuid,
    text: &str,
) -> PushTry {
    let Ok(screen) = tmux.capture_pane(pane, None) else {
        return PushTry::SessionGone;
    };
    if let Some(dialog) = classify_bottom(cli, &screen) {
        return PushTry::Dialog(dialog);
    }
    // An empty input line says nothing about a CLI that works: it is empty while it does. The
    // screen must not show the CLI at work, nor a choice it waits for.
    if looks_busy(cli, &screen) || awaits_choice(cli, &screen) {
        return PushTry::Held;
    }
    // The prompt must be on the screen and empty: nothing is pushed over a person's draft.
    if input_is_empty(cli, &screen) != Some(true) {
        return PushTry::NotReady;
    }
    let now = unix_now();
    let typing = tmux
        .client_activity(&pane.name)
        .unwrap_or_default()
        .iter()
        .any(|activity| now.saturating_sub(*activity) < user_idle_secs);
    if typing {
        return PushTry::NotReady;
    }
    let names: Vec<&str> = process_names.iter().map(String::as_str).collect();
    // The confirmation that bracketed paste is on (needed on tmux before 3.7) is made here, for
    // this push only: the prompt is on the screen right now, so the CLI is up, and the driver
    // checks that the foreground command is the CLI.
    let guard = PasteGuard {
        allowed_commands: &names,
        bracketed_confirmed: Some(BracketedPasteConfirmed::from_readiness_check()),
    };
    match tmux.push_message(pane, task_id, text, &guard, settle) {
        Ok(()) => PushTry::Pushed,
        Err(TmuxError::DeliveryUnknown) => PushTry::Unknown,
        Err(TmuxError::PastedNotSubmitted) => PushTry::PastedNotSubmitted,
        Err(TmuxError::Refused(Refusal::PaneDead | Refusal::NotOwned)) => PushTry::SessionGone,
        Err(TmuxError::Refused(_)) => PushTry::NotReady,
        Err(other) => {
            tracing::warn!("push into tmux session {} failed: {other}", pane.name.as_str());
            match tmux.has_session(&pane.name) {
                Ok(false) => PushTry::SessionGone,
                _ => PushTry::Failed,
            }
        }
    }
}
