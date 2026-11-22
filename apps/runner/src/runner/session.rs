//! The tmux channel: one worker per session key that owns the session's queue and all tmux I/O for
//! that key (design sections 7.2 to 7.7). The worker carries out what the pure machine
//! (`machine.rs`) decides, and feeds it what happens: hook events, poll results, push outcomes and
//! the clock.
//!
//! - **Ensure the session** (7.2): create it, respawn the CLI in a runner-owned session whose pane
//!   is dead or whose directory or CLI changed (when idle), adopt a session that already runs the
//!   CLI, respawn an adopted idle shell, and decline anything else; [`decide_ensure`] is the pure
//!   rule.
//! - **Push** (6.5, 7.6): only when the CLI is idle, the prompt line is visible and empty, and no
//!   attached client typed within `user_idle_secs`. The text goes through the tmux driver's gate;
//!   its bracketed-paste confirmation is created here, per push, from the readiness check made
//!   right then (the foreground command is the CLI and its prompt is on the screen), never kept.
//! - **Hooks** (7.3): turn events arrive on the hook socket; a session without hooks (an adopted
//!   one) is read from the screen instead: a changing screen is a working CLI, a still screen with
//!   a prompt an idle one.
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
use super::files::{BotFiles, PushedTask, SessionRecord, StateDir};
use super::hooks::{HookEvent, HookMessage};
use super::launch::{self, LaunchFiles};
use super::machine::{
    Action, EnsureOutcome, Machine, PushOutcome, Reminder, State, Task, Timers, TurnWatch, ViewFacts,
};
use super::message::{reminder_not_acknowledged, reminder_still_open};
use super::sv_client::SvClient;
use super::tmux::{
    BracketedPasteConfirmed, Pane, PasteGuard, Refusal, SessionKey, SocketName, Tmux, TmuxError, WorkDir,
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

// ----- the rule for an existing session ------------------------------------------------------

/// What is known about a session that already exists.
#[derive(Debug)]
#[allow(clippy::struct_excessive_bools, reason = "independent facts of one session")]
pub struct EnsureFacts<'a> {
    /// The runner created it: `session.json` names this pane and PID, for this project.
    pub runner_owned: bool,
    pub pane_dead: bool,
    /// The foreground process of the pane.
    pub foreground: &'a str,
    /// The pane's process has child processes.
    pub has_children: bool,
    /// The CLI's `process_names`.
    pub process_names: &'a [String],
    /// The recorded (or, for an adopted session, the current) directory or CLI differs.
    pub changed: bool,
    /// The CLI is idle (a respawn for a change waits for that).
    pub idle: bool,
    /// The session's recorded project is another project.
    pub other_project: bool,
    /// The runner has a socket of its own: a session on it that the runner did not create is
    /// somebody else's and is never adopted.
    pub socket_dedicated: bool,
}

/// What to do with an existing session.
#[derive(Debug, PartialEq, Eq)]
pub enum EnsureDecision {
    /// Restart the CLI in window 0.
    Respawn,
    /// It runs the CLI already.
    Running,
    /// A change is pending but the CLI is busy: wait for idle.
    Wait,
    /// Cannot be used; the text names no path.
    Decline(String),
}

const SHELLS: [&str; 6] = ["sh", "bash", "zsh", "fish", "dash", "ksh"];

/// The name of a foreign process as it may appear in a failure text: short and plain.
fn plain_name(name: &str) -> String {
    if !name.is_empty()
        && name.len() <= 32
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'))
    {
        name.to_string()
    } else {
        "another program".to_string()
    }
}

/// Design 7.2, "ensure session" for a session that exists.
pub fn decide_ensure(key: &str, cli: &str, facts: &EnsureFacts<'_>) -> EnsureDecision {
    if facts.other_project {
        return EnsureDecision::Decline(format!("tmux session {key} belongs to another project on this runner"));
    }
    if !facts.runner_owned && facts.socket_dedicated {
        return EnsureDecision::Decline(format!(
            "tmux session {key} is on the runner's socket but was not created by sylvode-runner; \
             end it, or run `sylvode-runner release {key}` once it is gone"
        ));
    }
    if facts.pane_dead {
        return EnsureDecision::Respawn;
    }
    let runs_cli = facts.process_names.iter().any(|name| name == facts.foreground);
    if facts.runner_owned || runs_cli {
        return match (facts.changed, facts.idle) {
            (false, _) => EnsureDecision::Running,
            (true, true) => EnsureDecision::Respawn,
            (true, false) => EnsureDecision::Wait,
        };
    }
    if SHELLS.contains(&facts.foreground) && !facts.has_children {
        return EnsureDecision::Respawn;
    }
    EnsureDecision::Decline(format!(
        "window 0 of tmux session {key} runs {}, not {cli}; close it or let sylvode-runner manage window 0",
        plain_name(facts.foreground)
    ))
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
    /// before it spends anything on it).
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

    fn sender(&self, key: &SessionKey) -> mpsc::Sender<Cmd> {
        let (tx, rx) = mpsc::channel(256);
        {
            let mut workers = self.workers.lock();
            if let Some(sender) = workers.get(key.as_str()) {
                return sender.clone();
            }
            workers.insert(key.as_str().to_string(), tx.clone());
        }
        let worker = Worker::new(Arc::clone(&self.shared), key.clone(), rx, tx.clone());
        tokio::spawn(worker.run());
        tx
    }

    /// Queues a task on the session of its project key and returns its 1-based queue position.
    pub async fn enqueue(&self, task: TmuxTask) -> Option<usize> {
        let key = SessionKey::new(&task.project_key).ok()?;
        let (reply, answer) = oneshot::channel();
        self.sender(&key)
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
        let Ok(key) = SessionKey::new(&message.session) else {
            return;
        };
        // Only a session that has a worker (a task was queued or recovered) takes events.
        let sender = self.workers.lock().get(key.as_str()).cloned();
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

    /// Whether the session of `key` has a live CLI pane and shows `id=<task_id>` in its last 200
    /// lines. Reads tmux only.
    pub async fn probe(&self, key: &SessionKey, task_id: &str, attempt: i64) -> Probe {
        let tmux = {
            let mut tmux = Tmux::new(self.shared.socket.clone());
            if let Some(program) = &self.shared.tmux_program {
                tmux = tmux.with_program(program);
            }
            tmux
        };
        let recorded_pushed = self.shared.state.read_session_record(key).is_some_and(|record| {
            record
                .pushed
                .is_some_and(|pushed| pushed.task_id == task_id && pushed.attempt == attempt)
        });
        let key = key.clone();
        let needle = format!("id={task_id}");
        tokio::task::spawn_blocking(move || {
            let gone = Probe {
                recorded_pushed,
                ..Probe::default()
            };
            if !tmux.has_session(&key).unwrap_or(false) {
                return gone;
            }
            let Ok(pane) = tmux.adopt_pane(&key) else {
                return gone;
            };
            if tmux.pane_state(&pane).map_or(true, |state| state.dead) {
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

    /// Hands recovered tasks to the worker of `key`: the pushed one is supervised again, the rest
    /// are queued in the order given.
    pub async fn recover(&self, key: SessionKey, adopt: Option<Adopt>, requeue: Vec<TmuxTask>) {
        let _ = self.sender(&key).send(Cmd::Recover { adopt, requeue }).await;
    }
}

// ----- the worker -----------------------------------------------------------------------------

struct Worker {
    shared: Arc<Shared>,
    key: SessionKey,
    machine: Machine<TmuxTask>,
    pane: Option<Pane>,
    rx: mpsc::Receiver<Cmd>,
    tx: mpsc::Sender<Cmd>,
    /// The session has no hooks (adopted): turn events are read from the screen.
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
/// meanwhile). A session is created or restarted long after the delivery was checked.
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
    fn new(shared: Arc<Shared>, key: SessionKey, rx: mpsc::Receiver<Cmd>, tx: mpsc::Sender<Cmd>) -> Self {
        let socket = shared.socket.as_str();
        let attach = if socket.is_empty() {
            format!("tmux attach -t ={}", key.as_str())
        } else {
            format!("tmux -L {socket} attach -t ={}", key.as_str())
        };
        let nonce = shared
            .state
            .read_session_record(&key)
            .map(|record| record.nonce)
            .filter(|nonce| nonce.len() == 32)
            .unwrap_or_else(fresh_nonce);
        shared
            .nonces
            .lock()
            .insert(key.as_str().to_string(), vec![nonce.clone()]);
        let machine = Machine::new(
            Timers::from_config(&shared.config.runner),
            key.as_str(),
            &attach,
            Instant::now(),
        );
        Self {
            shared,
            key,
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
                    let tmux = self.tmux();
                    let key = self.key.clone();
                    let pane = tokio::task::spawn_blocking(move || tmux.adopt_pane(&key)).await;
                    let owned = match &pane {
                        Ok(Ok(pane)) => self.owns(pane, &adopt.task.project_id).await,
                        _ => false,
                    };
                    if let (Ok(Ok(pane)), true) = (pane, owned) {
                        self.pane = Some(pane);
                        // A session without session.json was not started by this runner: no hooks.
                        self.screen_mode = self.shared.state.read_session_record(&self.key).is_none();
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
                        let reason = format!("runner restarted and session {} was gone", self.key.as_str());
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
                let pane_pid = self.shared.state.read_session_record(&self.key).map(|r| r.pane_pid);
                if let Err(why) = admit_hook(&self.nonce, &nonce, peer_chain.as_deref(), pane_pid) {
                    tracing::debug!(session = self.key.as_str(), "hook message dropped: {why}");
                    return Vec::new();
                }
                // Events are applied in the order their senders stamped them: a late one is stale.
                if !self.seq_gate.accept(seq) {
                    tracing::debug!(session = self.key.as_str(), "hook message dropped: out of order");
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
                if matches!(outcome, PushTry::Restarted) {
                    let mut actions = self.machine.on_push(PushOutcome::NotReady, now);
                    self.machine.on_cli_restarted(now);
                    actions.extend(self.machine.on_tick(now));
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
                tracing::warn!(session = self.key.as_str(), "no second Enter: the gate did not pass");
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

    /// Whether `pane` is the session this runner created for `project_id`: `session.json` names
    /// this pane id, this pane PID and this project. On a socket of the runner's own nothing else
    /// is adopted.
    async fn owns(&self, pane: &Pane, project_id: &str) -> bool {
        if self.shared.config.runner.tmux.socket_name.is_empty() {
            return true;
        }
        let Some(record) = self.shared.state.read_session_record(&self.key) else {
            return false;
        };
        let tmux = self.tmux();
        let pane_clone = pane.clone();
        let pid = tokio::task::spawn_blocking(move || tmux.pane_state(&pane_clone).map(|state| state.pid)).await;
        record.project_id == project_id
            && record.pane_id == pane.id.as_str()
            && matches!(pid, Ok(Ok(pid)) if pid == record.pane_pid)
    }

    async fn alive(&self) -> bool {
        let Some(pane) = self.pane.clone() else {
            // No pane is known yet; a session that exists is looked for once.
            let tmux = self.tmux();
            let key = self.key.clone();
            return tokio::task::spawn_blocking(move || tmux.has_session(&key).unwrap_or(false))
                .await
                .unwrap_or(false);
        };
        let tmux = self.tmux();
        tokio::task::spawn_blocking(move || {
            tmux.has_session(&pane.key).unwrap_or(false) && tmux.pane_state(&pane).is_ok_and(|state| !state.dead)
        })
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

    /// While the CLI starts: a blocking dialog fails the waiting tasks; for a CLI without a start
    /// hook (everything but claude), a still screen with a prompt means it is ready.
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
        if cli != "claude" {
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
        self.shared.state.read_session_record(&self.key).map(|r| r.cli)
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
                session = self.key.as_str(),
                "a turn ended without a Stop hook; closing it"
            );
            return self.machine.on_hook(HookEvent::Stop, now);
        }
        Vec::new()
    }

    async fn read_screen_signals(&mut self) -> Vec<Action<TmuxTask>> {
        if !self.screen_mode || self.pane.is_none() {
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
            .insert(self.key.as_str().to_string(), vec![self.nonce.clone()]);
    }

    /// A launch began: its nonce is published to the listener and the order counter starts over.
    fn launched(&mut self) {
        self.publish_nonce();
        self.seq_gate = super::hooks::SeqGate::default();
    }

    fn settings_job(&self, task: &TmuxTask) -> SettingsJob {
        SettingsJob {
            state: Arc::clone(&self.shared.state),
            key: self.key.clone(),
            claude: task.cli == "claude",
            mcp_server: task.mcp_server.clone(),
            runner_exe: self.shared.runner_exe.clone(),
        }
    }

    fn files_for(&self, task: &TmuxTask) -> Result<EnsureFiles, String> {
        let config = &self.shared.config;
        let state = &self.shared.state;
        let runner = &config.runner;
        let allow_write = task.cli == "claude" && runner.cli.claude.allow_write_call;
        state
            .write_bot_files(
                &BotFiles {
                    bot_id: &task.bot.bot_id,
                    api_url: &task.bot.api_url,
                    workspace_id: &task.bot.workspace_id,
                    token: &task.bot.token,
                    tool_mode: &runner.mcp.tool_mode,
                    allow_write_call: allow_write,
                },
                &task.mcp_server,
            )
            .map_err(|e| {
                tracing::error!("could not write the MCP files of bot {}: {e}", task.bot.bot_id);
                "the runner could not prepare the MCP configuration".to_string()
            })?;
        let toml = state.mcp_toml(&task.bot.bot_id);
        let claude_json = state.claude_mcp_json(&task.bot.bot_id);
        let settings = state.claude_settings(&self.key);
        let socket = state.hook_socket();
        let files = LaunchFiles {
            mcp_server: &task.mcp_server,
            mcp_toml: &toml,
            claude_mcp_json: &claude_json,
            claude_settings: &settings,
            runner_exe: &self.shared.runner_exe,
            hook_socket: &socket,
            nonce: &self.nonce,
            state_dir: state.root(),
            config_file: self.shared.config.config_path.as_deref(),
        };
        let argv = match task.cli.as_str() {
            "claude" => launch::claude_argv(&task.program, &runner.cli.claude, &task.dir, &files),
            "codex" => launch::codex_argv(&task.program, &runner.cli.codex, &task.dir, &files, &self.key),
            _ => launch::opencode_argv(&task.program),
        }
        .map_err(|e| {
            tracing::error!("launch refused for session {}: {e}", self.key.as_str());
            "the runner's settings for this CLI do not allow a session".to_string()
        })?;
        let argv = if task.cli == "claude" {
            // claude reads its settings, hooks and plugins from its configuration directory; the
            // runner gives it one of its own, so the operator's `~/.claude` is not loaded.
            let dir = state.ensure_claude_config_dir(&task.bot.bot_id).map_err(|e| {
                tracing::error!("could not prepare the claude configuration directory: {e}");
                "the runner could not prepare the CLI's configuration directory".to_string()
            })?;
            let mut wrapped = vec![
                "/usr/bin/env".to_string(),
                format!("CLAUDE_CONFIG_DIR={}", dir.to_string_lossy()),
            ];
            wrapped.extend(argv);
            wrapped
        } else {
            argv
        };
        Ok(EnsureFiles { argv })
    }

    async fn ensure(&mut self, task: &TmuxTask) -> EnsureOutcome {
        let declined = |reason: &str| EnsureOutcome::Declined {
            reason: reason.to_string(),
            retryable: false,
        };
        if !dir_unchanged(&task.dir) {
            return declined("the working directory changed since the task was accepted");
        }
        let files = match self.files_for(task) {
            Ok(files) => files,
            Err(reason) => return declined(&reason),
        };
        let Ok(dir) = WorkDir::new(&task.dir) else {
            return declined("the working directory cannot be used for a tmux session");
        };
        let process_names = process_names(&self.shared.config, &task.cli);
        let tmux = self.tmux();
        let key = self.key.clone();
        let tmux_cfg = self.shared.config.runner.tmux.clone();
        let record = self.shared.state.read_session_record(&key);
        let task = task.clone();
        let job = self.settings_job(&task);
        // The launch's secret is known to the listener before the CLI starts: its first hook
        // (`SessionStart`) may arrive before this call returns.
        let launch_nonce = fresh_nonce();
        self.shared
            .nonces
            .lock()
            .entry(self.key.as_str().to_string())
            .or_default()
            .push(launch_nonce.clone());
        let result = tokio::task::spawn_blocking(move || {
            ensure_blocking(
                &tmux,
                &key,
                &dir,
                &task,
                &files.argv,
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
            return declined("the tmux session could not be checked");
        };
        if !matches!(&result, Ok(EnsureResult { record: Some(_), .. })) {
            // No launch took place: the pending secret is forgotten.
            self.publish_nonce();
        }
        match result {
            Ok(EnsureResult { outcome, pane, record }) => {
                self.pane = pane;
                // A launch got a secret of its own; a session found running keeps its old one.
                if let Some(record) = &record {
                    self.nonce.clone_from(&record.nonce);
                    self.launched();
                }
                if let Some(record) = record
                    && let Err(e) = self.shared.state.write_session_record(&record)
                {
                    tracing::warn!("could not save session.json of {}: {e}", self.key.as_str());
                }
                // A session the runner did not start has no hooks.
                self.screen_mode = matches!(outcome, EnsureOutcome::Running)
                    && self
                        .shared
                        .state
                        .read_session_record(&self.key)
                        .is_none_or(|r| r.project_id.is_empty());
                outcome
            }
            Err(TmuxError::Refused(super::tmux::Refusal::ForeignServer)) => EnsureOutcome::Declined {
                reason: "a tmux server runs on the runner's socket that sylvode-runner did not start; stop it \
                         (`tmux -L <socket> kill-server`) so the runner can start its own"
                    .to_string(),
                retryable: false,
            },
            Err(e) => {
                tracing::error!("tmux session {}: {e}", self.key.as_str());
                EnsureOutcome::Declined {
                    reason: "the tmux session could not be created or checked".to_string(),
                    retryable: true,
                }
            }
        }
    }

    // ----- push -----

    async fn try_push(&mut self, task: &TmuxTask) -> PushTry {
        let Some(pane) = self.pane.clone() else {
            // Recovered without a pane: find window 0 once.
            let tmux = self.tmux();
            let key = self.key.clone();
            return match tokio::task::spawn_blocking(move || tmux.adopt_pane(&key)).await {
                Ok(Ok(pane)) => {
                    self.pane = Some(pane);
                    PushTry::NotReady
                }
                _ => PushTry::SessionGone,
            };
        };
        // The directory or CLI changed since the session was made: restart it first (the CLI is
        // idle, or nothing would be pushed).
        if let Some(record) = self.shared.state.read_session_record(&self.key)
            && (record.cli != task.cli || Path::new(&record.dir) != task.dir)
        {
            // Ending a CLI that is working (the hooks may be wrong) or whose person is typing
            // would lose their work: restart it only when the screen is calm.
            let (tmux, calm_pane, cli) = (self.tmux(), pane.clone(), record.cli.clone());
            let user_idle = self.shared.config.runner.tmux.user_idle_secs;
            let calm = tokio::task::spawn_blocking(move || screen_calm(&tmux, &calm_pane, &cli, user_idle))
                .await
                .unwrap_or(false);
            if !calm {
                return PushTry::Held;
            }
            return match self.respawn_for(task, &pane).await {
                Ok(()) => PushTry::Restarted,
                Err(()) => PushTry::Failed,
            };
        }
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
        if let Err(e) = self.shared.state.mark_pushed(&self.key, Some(pushed)) {
            tracing::warn!(
                session = self.key.as_str(),
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
            PushTry::NotReady | PushTry::Dialog(_) | PushTry::SessionGone | PushTry::Failed | PushTry::Restarted
        ) && let Err(e) = self.shared.state.mark_pushed(&self.key, None)
        {
            tracing::warn!(session = self.key.as_str(), "could not clear the push record: {e}");
        }
        result
    }

    async fn respawn_for(&mut self, task: &TmuxTask, pane: &Pane) -> Result<(), ()> {
        if !dir_unchanged(&task.dir) {
            return Err(());
        }
        // A new launch gets a new secret; the old one stops authenticating anything.
        self.nonce = fresh_nonce();
        self.launched();
        let files = self.files_for(task).map_err(|_| ())?;
        self.settings_job(task).write(&self.nonce).map_err(|_| ())?;
        let dir = WorkDir::new(&task.dir).map_err(|_| ())?;
        let tmux = self.tmux();
        let pane_clone = pane.clone();
        let argv = files.argv;
        let dir_clone = dir.clone();
        tokio::task::spawn_blocking(move || tmux.respawn_pane(&pane_clone, &dir_clone, &argv))
            .await
            .map_err(|_| ())?
            .map_err(|_| ())?;
        let tmux = self.tmux();
        let pane_clone = pane.clone();
        let state = tokio::task::spawn_blocking(move || tmux.pane_state(&pane_clone))
            .await
            .map_err(|_| ())?
            .map_err(|_| ())?;
        let record = SessionRecord {
            key: self.key.as_str().to_string(),
            project_id: task.project_id.clone(),
            pane_id: pane.id.as_str().to_string(),
            pane_pid: state.pid,
            cli: task.cli.clone(),
            dir: task.dir.to_string_lossy().into_owned(),
            started_at: unix_now(),
            headless: false,
            nonce: self.nonce.clone(),
            pushed: None,
        };
        self.shared.state.write_session_record(&record).map_err(|_| ())
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
        tracing::debug!(session = self.key.as_str(), ?result, "reminder push");
        matches!(
            result,
            Ok(PushTry::Pushed | PushTry::Unknown | PushTry::PastedNotSubmitted)
        )
    }
}

/// Writes the hook settings of one launch with that launch's nonce. Run just before the CLI is
/// started (a new session, or a respawn), so every launch has a secret of its own; a session that
/// is only found running keeps the one it was started with.
struct SettingsJob {
    state: Arc<StateDir>,
    key: SessionKey,
    claude: bool,
    mcp_server: PathBuf,
    runner_exe: PathBuf,
}

impl SettingsJob {
    fn write(&self, nonce: &str) -> Result<(), TmuxError> {
        if !self.claude {
            return Ok(());
        }
        // A new launch counts its hooks from the start.
        let _ = std::fs::remove_file(
            self.state
                .root()
                .join("sessions")
                .join(self.key.as_str())
                .join("hook.seq"),
        );
        let toml = self.state.root().join("unused.toml");
        let claude_json = self.state.root().join("unused.json");
        let settings = self.state.claude_settings(&self.key);
        let socket = self.state.hook_socket();
        let files = LaunchFiles {
            mcp_server: &self.mcp_server,
            mcp_toml: &toml,
            claude_mcp_json: &claude_json,
            claude_settings: &settings,
            runner_exe: &self.runner_exe,
            hook_socket: &socket,
            nonce,
            state_dir: self.state.root(),
            config_file: None,
        };
        self.state
            .write_session_settings(&self.key, &files)
            .map(|_| ())
            .map_err(|e| TmuxError::Invalid(format!("could not write the hook settings: {e}")))
    }
}

struct EnsureFiles {
    argv: Vec<String>,
}

fn process_names(config: &Config, cli: &str) -> Vec<String> {
    match cli {
        "claude" => config.runner.cli.claude.process_names.clone(),
        "codex" => config.runner.cli.codex.process_names.clone(),
        _ => config.runner.cli.opencode.process_names.clone(),
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
}

#[allow(clippy::too_many_arguments, reason = "one call gathers the facts of one session")]
fn ensure_blocking(
    tmux: &Tmux,
    key: &SessionKey,
    dir: &WorkDir,
    task: &TmuxTask,
    argv: &[String],
    process_names: &[String],
    record: Option<&SessionRecord>,
    job: &SettingsJob,
    cfg: &super::config::RunnerTmuxConfig,
    launch_nonce: &str,
) -> Result<EnsureResult, TmuxError> {
    // A server on the runner's socket that the runner did not start is not used.
    tmux.verify_server()?;
    let launch_nonce = launch_nonce.to_string();
    let new_record = |pane: &Pane, pid: u32| SessionRecord {
        nonce: launch_nonce.clone(),
        pushed: None,
        key: key.as_str().to_string(),
        project_id: task.project_id.clone(),
        pane_id: pane.id.as_str().to_string(),
        pane_pid: pid,
        cli: task.cli.clone(),
        dir: task.dir.to_string_lossy().into_owned(),
        started_at: unix_now(),
        headless: false,
    };
    if !tmux.has_session(key)? {
        job.write(&launch_nonce)?;
        let pane = tmux.new_session(key, dir, cfg.window_width, cfg.window_height, argv)?;
        let pid = tmux.pane_state(&pane)?.pid;
        let record = new_record(&pane, pid);
        return Ok(EnsureResult {
            outcome: EnsureOutcome::Started,
            pane: Some(pane),
            record: Some(record),
        });
    }
    let recorded_pane = record
        .and_then(|r| super::tmux::PaneId::new(&r.pane_id).ok())
        .map(|id| Pane { key: key.clone(), id });
    let pane = match recorded_pane {
        Some(pane) if tmux.pane_state(&pane).is_ok_and(|s| s.session_name == key.as_str()) => pane,
        _ => tmux.adopt_pane(key)?,
    };
    let info = tmux.pane_info(&pane)?;
    let runner_owned = record.is_some_and(|r| r.pane_id == pane.id.as_str() && r.pane_pid == info.pid);
    let other_project = record.is_some_and(|r| !r.project_id.is_empty() && r.project_id != task.project_id);
    let recorded_changed = record.is_some_and(|r| r.cli != task.cli || Path::new(&r.dir) != task.dir);
    let changed = if runner_owned {
        recorded_changed
    } else {
        std::fs::canonicalize(&info.current_path).map_or(true, |p| p != task.dir)
    };
    let has_children = std::process::Command::new("/usr/bin/pgrep")
        .env_clear()
        .args(["-P", &info.pid.to_string()])
        .output()
        .map_or(true, |out| !out.stdout.is_empty());
    let facts = EnsureFacts {
        runner_owned,
        pane_dead: info.dead,
        foreground: &info.current_command,
        has_children,
        process_names,
        changed,
        idle: info.dead || screen_calm(tmux, &pane, &task.cli, cfg.user_idle_secs),
        other_project,
        socket_dedicated: !cfg.socket_name.is_empty(),
    };
    match decide_ensure(key.as_str(), &task.cli, &facts) {
        EnsureDecision::Respawn => {
            job.write(&launch_nonce)?;
            tmux.respawn_pane(&pane, dir, argv)?;
            let pid = tmux.pane_state(&pane)?.pid;
            let record = new_record(&pane, pid);
            Ok(EnsureResult {
                outcome: EnsureOutcome::Restarted,
                pane: Some(pane),
                record: Some(record),
            })
        }
        EnsureDecision::Running | EnsureDecision::Wait => Ok(EnsureResult {
            outcome: EnsureOutcome::Running,
            pane: Some(pane),
            record: None,
        }),
        EnsureDecision::Decline(reason) => Ok(EnsureResult {
            outcome: EnsureOutcome::Declined {
                reason,
                retryable: false,
            },
            pane: None,
            record: None,
        }),
    }
}

/// Whether the CLI of `pane` may be ended and started again: its screen shows neither work nor a
/// choice, and no client typed within `user_idle_secs`.
fn screen_calm(tmux: &Tmux, pane: &Pane, cli: &str, user_idle_secs: u64) -> bool {
    let Ok(screen) = tmux.capture_pane(pane, None) else {
        return false;
    };
    if looks_busy(cli, &screen) || awaits_choice(cli, &screen) {
        return false;
    }
    let now = unix_now();
    !tmux
        .client_activity(&pane.key)
        .unwrap_or_default()
        .iter()
        .any(|activity| now.saturating_sub(*activity) < user_idle_secs)
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
        .client_activity(&pane.key)
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
    Restarted,
}

impl PushTry {
    const fn into_outcome(self) -> PushOutcome {
        match self {
            Self::Pushed => PushOutcome::Pushed,
            Self::Unknown => PushOutcome::Unknown,
            Self::PastedNotSubmitted => PushOutcome::PastedNotSubmitted,
            Self::NotReady | Self::Dialog(_) | Self::Restarted => PushOutcome::NotReady,
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
        .client_activity(&pane.key)
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
            tracing::warn!("push into tmux session {} failed: {other}", pane.key.as_str());
            match tmux.has_session(&pane.key) {
                Ok(false) => PushTry::SessionGone,
                _ => PushTry::Failed,
            }
        }
    }
}
