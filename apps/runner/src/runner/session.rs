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
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use tokio::sync::mpsc::error::TrySendError;
use tokio::sync::{mpsc, oneshot, watch};
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
use super::profile::profile;
use super::sv_client::SvClient;
use super::timing;
use super::tmux::{
    BracketedPasteConfirmed, ListedPane, NameSource, Pane, PaneId, PasteGuard, Refusal, Slot, SocketName, Tmux,
    TmuxError, TmuxName, WorkDir,
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
    /// The slot that serves the task: the project's own, or its bot's.
    pub slot: Slot,
    /// The tmux session the task is typed into, and where its name came from.
    pub tmux: TmuxName,
    pub name_source: NameSource,
    pub dir: PathBuf,
    /// The directory passed the allowed roots at delivery (it did not when the session existed
    /// then, and the runner must not create one in it).
    pub dir_checked: bool,
    pub cli: String,
    pub program: PathBuf,
    pub mcp_server: PathBuf,
    /// The work item the task belongs to (see `checks::item_key`), for the pool's item affinity.
    pub item: Option<String>,
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
            slot: accepted.slot.clone(),
            tmux: accepted.tmux.clone(),
            name_source: accepted.name_source,
            dir: accepted.dir.clone(),
            dir_checked: accepted.dir_checked,
            cli: accepted.cli.clone(),
            program: accepted.program.clone(),
            mcp_server: accepted.mcp_server.clone(),
            item: accepted.item.clone(),
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
    /// The runner may create the session when it is absent (a derived name only).
    pub allow_create: bool,
    /// What the runner recorded about the session, if anything.
    pub record: Option<&'a SessionRecord>,
    /// The bot of the task's connection.
    pub task_bot_id: &'a str,
}

/// What to do with the session of a task (design 1.3, E1 to E3, and T4).
#[derive(Debug, PartialEq, Eq)]
pub enum EnsureDecision {
    /// E1: no such session; the runner creates it (a derived name only).
    Create,
    /// No such session, and its name is a configured one: the runner never creates it.
    Absent,
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
        return if facts.allow_create {
            EnsureDecision::Create
        } else {
            EnsureDecision::Absent
        };
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
pub fn decline_for(decision: &EnsureDecision, name: &str, cli: &str, source: NameSource) -> Option<(String, bool)> {
    match decision {
        EnsureDecision::NotRunning => Some((
            if source == NameSource::Derived {
                format!(
                    "the CLI is not running in tmux session {name}; attach with tmux attach -t {name} and start it, \
                     or end the session so the runner starts it"
                )
            } else {
                format!("the CLI is not running in tmux session {name}; attach with tmux attach -t {name} and start it")
            },
            true,
        )),
        EnsureDecision::Absent => Some((
            format!(
                "tmux session {name} does not exist; start it and run {cli} in it (tmux new-session -s {name}); \
                 the runner does not create sessions with a configured name"
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

// ----- bot pool ------------------------------------------------------------------------------

/// What a session worker publishes about itself through a `watch` channel, for the choice of a
/// pool member. A member is healthy unless its session is gone; the stuck rules come with the
/// pool itself.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MemberStatus {
    pub state: State,
    /// Tasks queued behind the current one.
    pub queue_len: usize,
    pub healthy: bool,
    /// Since when the member has been unhealthy; `None` while it is healthy.
    pub unhealthy_since: Option<Instant>,
}

impl MemberStatus {
    /// The status of a worker that has not looked at its session yet.
    #[must_use]
    pub const fn fresh() -> Self {
        Self {
            state: State::NoSession,
            queue_len: 0,
            healthy: true,
            unhealthy_since: None,
        }
    }

    /// The status after a state change: the unhealthy time is kept while the member stays unhealthy.
    #[must_use]
    pub fn next(&self, state: State, queue_len: usize, now: Instant) -> Self {
        self.next_checked(state, queue_len, now, false)
    }

    /// Like [`Self::next`], with `unhealthy` set when the worker found another reason to distrust
    /// the member (absent session, declined session, blocked or held too long). A dead session is
    /// always unhealthy.
    #[must_use]
    pub fn next_checked(&self, state: State, queue_len: usize, now: Instant, unhealthy: bool) -> Self {
        let healthy = !unhealthy && !matches!(state, State::Dead);
        let unhealthy_since = if healthy {
            None
        } else {
            self.unhealthy_since.or(Some(now))
        };
        Self {
            state,
            queue_len,
            healthy,
            unhealthy_since,
        }
    }

    /// Whether the member has been unhealthy for at least `after` at `now`.
    #[must_use]
    pub fn unhealthy_for(&self, now: Instant, after: Duration) -> bool {
        !self.healthy
            && self
                .unhealthy_since
                .is_some_and(|since| now.saturating_duration_since(since) >= after)
    }

    const fn is_idle(&self) -> bool {
        matches!(self.state, State::Idle) && self.queue_len == 0
    }

    /// The queued tasks plus the one in flight.
    const fn load(&self) -> usize {
        let current = match self.state {
            State::Pushing { .. } | State::Busy | State::BusyUser | State::Blocked { .. } => 1,
            State::NoSession | State::Starting { .. } | State::Idle | State::Dead => 0,
        };
        self.queue_len + current
    }
}

/// How long a task's member is remembered after the task left it.
pub const TASK_AFFINITY_KEEP_SECS: u64 = 24 * 3600;
/// The most task and item entries the affinity store keeps (the oldest go first).
pub const AFFINITY_CAP: usize = 1000;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
struct TaskAffinity {
    member: u8,
    /// When the task left the member; `None` while it is queued or current there.
    left_at: Option<u64>,
    touched: u64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
struct ItemAffinity {
    member: u8,
    served_at: u64,
}

/// Which member served which task and work item. Pure data; times are Unix seconds given by the
/// caller. Its serialised form is what the pool will persist.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct AffinityStore {
    tasks: HashMap<String, TaskAffinity>,
    items: HashMap<String, ItemAffinity>,
}

impl AffinityStore {
    /// The member that holds or served `task_id`, unless it left more than a day ago.
    #[must_use]
    pub fn task_member(&self, task_id: &str, now: u64) -> Option<u8> {
        let entry = self.tasks.get(task_id)?;
        let expired = entry
            .left_at
            .is_some_and(|left| now.saturating_sub(left) > TASK_AFFINITY_KEEP_SECS);
        (!expired).then_some(entry.member)
    }

    /// The member that last served work item `item` within `ttl_secs`.
    #[must_use]
    pub fn item_member(&self, item: &str, now: u64, ttl_secs: u64) -> Option<u8> {
        let entry = self.items.get(item)?;
        (now.saturating_sub(entry.served_at) <= ttl_secs).then_some(entry.member)
    }

    /// `task_id` was queued on `member`.
    pub fn record_task(&mut self, task_id: &str, member: u8, now: u64) {
        self.tasks.insert(
            task_id.to_string(),
            TaskAffinity {
                member,
                left_at: None,
                touched: now,
            },
        );
        self.prune();
    }

    /// `task_id` left its member (its expiry clock starts).
    pub fn task_left(&mut self, task_id: &str, now: u64) {
        if let Some(entry) = self.tasks.get_mut(task_id) {
            entry.left_at = Some(now);
            entry.touched = now;
        }
    }

    /// `member` served work item `item` now.
    pub fn record_item(&mut self, item: &str, member: u8, now: u64) {
        self.items
            .insert(item.to_string(), ItemAffinity { member, served_at: now });
        self.prune();
    }

    /// Keeps at most [`AFFINITY_CAP`] entries of each kind, dropping the least recently touched.
    pub fn prune(&mut self) {
        while self.tasks.len() > AFFINITY_CAP {
            let Some(oldest) = self
                .tasks
                .iter()
                .min_by(|a, b| a.1.touched.cmp(&b.1.touched).then_with(|| a.0.cmp(b.0)))
                .map(|(id, _)| id.clone())
            else {
                break;
            };
            self.tasks.remove(&oldest);
        }
        while self.items.len() > AFFINITY_CAP {
            let Some(oldest) = self
                .items
                .iter()
                .min_by(|a, b| a.1.served_at.cmp(&b.1.served_at).then_with(|| a.0.cmp(b.0)))
                .map(|(id, _)| id.clone())
            else {
                break;
            };
            self.items.remove(&oldest);
        }
    }

    /// The number of task entries.
    #[must_use]
    pub fn task_count(&self) -> usize {
        self.tasks.len()
    }
}

/// Why [`select_member`] chose a member (the word is logged).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SelectReason {
    AffinityTask,
    AffinityItem,
    Idle,
    Shortest,
    Fallback,
}

impl SelectReason {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::AffinityTask => "affinity_task",
            Self::AffinityItem => "affinity_item",
            Self::Idle => "idle",
            Self::Shortest => "shortest",
            Self::Fallback => "fallback",
        }
    }
}

/// What the choice of a member looks at in a task.
#[derive(Clone, Copy, Debug)]
pub struct SelectInput<'a> {
    /// `None` skips task affinity (a queued task being moved to another member).
    pub task_id: Option<&'a str>,
    /// The work item the task belongs to, when the task view names one.
    pub item: Option<&'a str>,
}

/// Picks the member (an index into `statuses`, which is ordered by member number) for a task:
/// task affinity, item affinity, the lowest idle member, the shortest queue, else the primary.
/// An affinity member that is unhealthy or absent is skipped. Pure: no clock, no I/O.
#[must_use]
pub fn select_member(
    input: &SelectInput<'_>,
    affinity: &AffinityStore,
    statuses: &[MemberStatus],
    now: u64,
    item_ttl_secs: u64,
) -> (usize, SelectReason) {
    let healthy = |member: u8| statuses.get(usize::from(member)).is_some_and(|status| status.healthy);
    if let Some(member) = input.task_id.and_then(|id| affinity.task_member(id, now))
        && healthy(member)
    {
        return (usize::from(member), SelectReason::AffinityTask);
    }
    if let Some(member) = input
        .item
        .and_then(|item| affinity.item_member(item, now, item_ttl_secs))
        && healthy(member)
    {
        return (usize::from(member), SelectReason::AffinityItem);
    }
    if let Some(index) = statuses.iter().position(|status| status.healthy && status.is_idle()) {
        return (index, SelectReason::Idle);
    }
    // `min_by_key` keeps the first of equal keys, which is the lowest index.
    let shortest = statuses
        .iter()
        .enumerate()
        .filter(|(_, status)| status.healthy)
        .min_by_key(|(_, status)| status.load())
        .map(|(index, _)| index);
    shortest.map_or((0, SelectReason::Fallback), |index| (index, SelectReason::Shortest))
}

/// One member of a bot's pool as the manager sees it.
struct PoolMember {
    slot: Slot,
    status: watch::Receiver<MemberStatus>,
    /// The member's count of enqueues that are chosen or in its channel and not yet in its queue.
    pending: Arc<AtomicUsize>,
}

/// The tasks whose enqueue is under way, by task id: the member chosen for each and how many
/// enqueues of the task hold the mark. A second enqueue of the same task (a new attempt arriving
/// within the answer window) goes to the same member, so one task is never queued on two members.
type InFlight = Arc<Mutex<HashMap<String, (u8, usize)>>>;

/// Holds the in-flight mark of a task until the enqueue is over (or its future is dropped).
pub(super) struct InFlightMark {
    marks: InFlight,
    task_id: Option<String>,
}

impl Drop for InFlightMark {
    fn drop(&mut self) {
        let Some(id) = self.task_id.take() else { return };
        let mut marks = self.marks.lock();
        if let Some(entry) = marks.get_mut(&id) {
            entry.1 = entry.1.saturating_sub(1);
            if entry.1 == 0 {
                marks.remove(&id);
            }
        }
    }
}

/// The member chosen for a task, with what keeps the choice true until the member has the task.
pub(super) struct Chosen {
    slot: Slot,
    reason: SelectReason,
    /// Counted against the member from the moment of the choice.
    pending: Pending,
    mark: InFlightMark,
}

/// The sessions of one bot and CLI, ordered by member number, and what they served. The pool of a
/// bot has exactly one member until the operator lists more.
#[derive(Default)]
pub struct BotPool {
    members: Vec<PoolMember>,
    affinity: AffinityStore,
    inflight: InFlight,
    /// Bumped on every change worth writing to disk.
    version: u64,
    /// The newest version written, so an older snapshot finishing late never overwrites a newer one.
    persisted: Arc<Mutex<u64>>,
}

impl BotPool {
    /// A pool with the affinity the runner persisted for `primary`, if any.
    fn load(state: &StateDir, primary: &Slot) -> Self {
        let mut pool = Self::default();
        if let Some(mut affinity) = state
            .read_pool_affinity(primary)
            .and_then(|text| serde_json::from_str::<AffinityStore>(&text).ok())
        {
            affinity.prune();
            pool.affinity = affinity;
        }
        pool
    }

    /// Adds the member `slot` (a bot slot) with its status channel and pending count; a member
    /// already there is kept.
    fn attach(&mut self, slot: &Slot, status: watch::Receiver<MemberStatus>, pending: Arc<AtomicUsize>) {
        if self.members.iter().any(|member| member.slot == *slot) {
            return;
        }
        self.members.push(PoolMember {
            slot: slot.clone(),
            status,
            pending,
        });
        self.members.sort_by_key(|member| member.slot.member());
    }

    /// Chooses the member that takes the task, counts the task against it and marks it in flight,
    /// all under one hold of the marks, so concurrent choices see each other. Records no affinity:
    /// that is [`Self::record`], once the member has accepted the task. A member at its queue cap
    /// (`max_queued` of 0 means no cap), counting the enqueues on their way to it, is passed over
    /// while another member has room. A task already in flight keeps its member.
    fn choose(&self, input: &SelectInput<'_>, now: u64, item_ttl_secs: u64, max_queued: usize) -> Option<Chosen> {
        let mut marks = self.inflight.lock();
        let marked = input
            .task_id
            .and_then(|id| marks.get(id))
            .and_then(|(member, _)| self.members.iter().position(|m| m.slot.member() == *member));
        let (index, reason) = marked.map_or_else(
            || self.pick(input, now, item_ttl_secs, max_queued),
            |index| (index, SelectReason::AffinityTask),
        );
        let member = self.members.get(index)?;
        let pending = Pending::acquire(&member.pending);
        let task_id = input.task_id.map(str::to_string);
        if let Some(id) = &task_id {
            marks
                .entry(id.clone())
                .and_modify(|entry| entry.1 = entry.1.saturating_add(1))
                .or_insert_with(|| (member.slot.member(), 1));
        }
        drop(marks);
        Some(Chosen {
            slot: member.slot.clone(),
            reason,
            pending,
            mark: InFlightMark {
                marks: Arc::clone(&self.inflight),
                task_id,
            },
        })
    }

    /// The member [`select_member`] picks, counting the enqueues on their way to each member and
    /// passing over the members at the queue cap while another has room.
    fn pick(&self, input: &SelectInput<'_>, now: u64, item_ttl_secs: u64, max_queued: usize) -> (usize, SelectReason) {
        let mut statuses: Vec<MemberStatus> = self
            .members
            .iter()
            .map(|member| {
                let status = *member.status.borrow();
                MemberStatus {
                    queue_len: status.queue_len.saturating_add(member.pending.load(Ordering::Acquire)),
                    ..status
                }
            })
            .collect();
        if max_queued > 0 {
            let open: Vec<MemberStatus> = statuses
                .iter()
                .map(|status| MemberStatus {
                    healthy: status.healthy && status.queue_len < max_queued,
                    ..*status
                })
                .collect();
            if open.iter().any(|status| status.healthy) {
                statuses = open;
            }
        }
        select_member(input, &self.affinity, &statuses, now, item_ttl_secs)
    }

    /// Remembers that `slot` took the task (and the work item) of `input`.
    fn record(&mut self, input: &SelectInput<'_>, slot: &Slot, now: u64) {
        if let Some(id) = input.task_id {
            self.affinity.record_task(id, slot.member(), now);
        }
        if let Some(item) = input.item {
            self.affinity.record_item(item, slot.member(), now);
        }
        self.version = self.version.saturating_add(1);
    }

    /// `task_id` left its member.
    fn task_left(&mut self, task_id: &str, now: u64) {
        self.affinity.task_left(task_id, now);
        self.version = self.version.saturating_add(1);
    }

    /// Whether a member other than `member` is healthy.
    fn has_healthy_other(&self, member: u8) -> bool {
        self.members
            .iter()
            .any(|other| other.slot.member() != member && other.status.borrow().healthy)
    }

    /// What to write to disk: the version, the text and the version gate. `None` for a pool of one
    /// member, which keeps no file.
    fn snapshot(&self) -> Option<Snapshot> {
        if self.members.len() < 2 {
            return None;
        }
        let text = serde_json::to_string(&self.affinity).ok()?;
        Some(Snapshot {
            version: self.version,
            text,
            gate: Arc::clone(&self.persisted),
        })
    }
}

/// The affinity of a pool, serialised, with the gate that keeps writes in order.
struct Snapshot {
    version: u64,
    text: String,
    gate: Arc<Mutex<u64>>,
}

impl Snapshot {
    /// Writes the snapshot unless a newer one was written already.
    fn write(self, state: &StateDir, primary: &Slot) {
        let mut done = self.gate.lock();
        if self.version <= *done {
            return;
        }
        match state.write_pool_affinity(primary, &self.text) {
            Ok(()) => *done = self.version,
            Err(e) => tracing::warn!(pool = primary.as_str(), "the pool's affinity could not be saved: {e}"),
        }
    }
}

/// How many commands a session's channel holds.
const WORKER_CHANNEL: usize = 256;

pub(super) enum Cmd {
    Recover {
        adopt: Option<Adopt>,
        requeue: Vec<TmuxTask>,
    },
    Enqueue {
        task: Box<TmuxTask>,
        reply: oneshot::Sender<usize>,
        /// Counts the task as pending until the worker has taken the command (or it is dropped).
        pending: Pending,
    },
    Hook {
        event: HookEvent,
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
    /// The pane each session's worker recorded, for the hook listener (which checks a message
    /// before it spends anything on it).
    panes: PaneMap,
    /// Asked for a report when a session appears or changes, so Sylvode does not show the old
    /// state until the next periodic one.
    report_trigger: crate::handshake::ReportTrigger,
    /// The command channel of each session worker, by slot.
    workers: Mutex<HashMap<String, WorkerHandle>>,
    /// The pool of each bot and CLI, by the text of its member 0 slot.
    pools: Mutex<HashMap<String, BotPool>>,
}

impl Shared {
    /// Points `task` at the pool member `slot`: its slot, and the tmux session of that member.
    /// A member the configuration does not name keeps the task's tmux session.
    fn retarget(&self, task: &mut TmuxTask, slot: &Slot) {
        if let Some((tmux, _)) = super::naming::tmux_of_slot(&self.config.runner, slot) {
            task.tmux = tmux;
        }
        task.slot = slot.clone();
    }

    /// Writes the affinity snapshot of the pool of `primary` in the background.
    fn persist(self: &Arc<Self>, primary: &Slot, snapshot: Option<Snapshot>) {
        let Some(snapshot) = snapshot else { return };
        let (shared, primary) = (Arc::clone(self), primary.clone());
        // A runtime that is shutting down drops the write; the file is only a hint.
        drop(tokio::task::spawn_blocking(move || {
            snapshot.write(&shared.state, &primary);
        }));
    }

    /// Remembers that the member `slot` of a bot's pool took task `task_id` of work item `item`.
    fn record_affinity(self: &Arc<Self>, slot: &Slot, task_id: &str, item: Option<&str>) {
        if !slot.is_bot() {
            return;
        }
        let Ok(primary) = bot_primary(slot) else { return };
        let input = SelectInput {
            task_id: Some(task_id),
            item,
        };
        let snapshot = {
            let mut pools = self.pools.lock();
            pools.get_mut(primary.as_str()).and_then(|pool| {
                pool.record(&input, slot, unix_now());
                pool.snapshot()
            })
        };
        self.persist(&primary, snapshot);
    }

    /// The members the configuration names for the bot of `slot` and its CLI (1 when it names none).
    fn member_count(&self, slot: &Slot) -> usize {
        slot.bot_id()
            .and_then(|bot| self.config.runner.bot_session(bot, slot.cli()))
            .map_or(1, |entry| entry.names.len().max(1))
    }
}

/// `(tmux socket path, pane id)` to the name of the session that uses the pane.
type PaneMap = Arc<Mutex<HashMap<(String, String), String>>>;

/// One enqueue that is in a session's channel and not yet in its queue. Releases its count when
/// dropped.
pub(super) struct Pending {
    counter: Arc<AtomicUsize>,
    /// The number of pending enqueues of the session, this one included, when it was counted.
    count: usize,
}

impl Pending {
    fn acquire(counter: &Arc<AtomicUsize>) -> Self {
        let count = counter.fetch_add(1, Ordering::AcqRel).saturating_add(1);
        Self {
            counter: Arc::clone(counter),
            count,
        }
    }
}

impl Drop for Pending {
    fn drop(&mut self) {
        self.counter.fetch_sub(1, Ordering::AcqRel);
    }
}

/// Why an enqueue was not accepted.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EnqueueRefusal {
    /// The session's command channel is full.
    Full,
    /// The session holds `max_queued_per_session` tasks already.
    Capped,
    /// The session's worker has ended.
    Closed,
}

impl EnqueueRefusal {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Full => "channel_full",
            Self::Capped => "queue_cap",
            Self::Closed => "worker_closed",
        }
    }
}

/// What an enqueue may wait for and hold.
#[derive(Clone, Copy, Debug)]
pub struct EnqueueLimits {
    /// How long to wait for the queue position; zero waits without a bound.
    pub answer: Duration,
    /// The most tasks one session may hold queued; zero means no limit.
    pub max_queued: usize,
}

/// A session's command channel and what the manager may read of its state.
#[derive(Clone)]
pub(super) struct WorkerHandle {
    tx: mpsc::Sender<Cmd>,
    status: watch::Receiver<MemberStatus>,
    pending: Arc<AtomicUsize>,
}

impl WorkerHandle {
    pub(super) fn new(tx: mpsc::Sender<Cmd>, status: watch::Receiver<MemberStatus>) -> Self {
        Self {
            tx,
            status,
            pending: Arc::default(),
        }
    }

    /// Hands `task` to the worker without waiting for room, then waits for its queue position for
    /// at most `limits.answer`. `Ok(None)` means the worker did not answer in time; the task is
    /// queued all the same, in the order it arrived.
    pub(super) async fn enqueue(&self, task: TmuxTask, limits: EnqueueLimits) -> Result<Option<usize>, EnqueueRefusal> {
        let pending = Pending::acquire(&self.pending);
        self.enqueue_reserved(task, limits, pending).await
    }

    /// [`Self::enqueue`] for a task already counted against this session as `pending` (the pool
    /// counts a task when it chooses the member).
    async fn enqueue_reserved(
        &self,
        task: TmuxTask,
        limits: EnqueueLimits,
        pending: Pending,
    ) -> Result<Option<usize>, EnqueueRefusal> {
        if limits.max_queued > 0 {
            let queued = self.status.borrow().queue_len;
            if queued.saturating_add(pending.count) > limits.max_queued {
                return Err(EnqueueRefusal::Capped);
            }
        }
        let (reply, answer) = oneshot::channel();
        match self.tx.try_send(Cmd::Enqueue {
            task: Box::new(task),
            reply,
            pending,
        }) {
            Ok(()) => {}
            Err(TrySendError::Full(_)) => return Err(EnqueueRefusal::Full),
            Err(TrySendError::Closed(_)) => return Err(EnqueueRefusal::Closed),
        }
        if limits.answer.is_zero() {
            return answer.await.map(Some).map_err(|_| EnqueueRefusal::Closed);
        }
        match tokio::time::timeout(limits.answer, answer).await {
            Ok(Ok(position)) => Ok(Some(position)),
            Ok(Err(_)) => Err(EnqueueRefusal::Closed),
            Err(_) => Ok(None),
        }
    }

    /// Hands `task` to the worker without waiting for room or for an answer (a task moved from
    /// another member). A refusal gives the task back.
    fn enqueue_nowait(&self, task: TmuxTask, pending: Pending) -> Result<(), (EnqueueRefusal, Box<TmuxTask>)> {
        let backup = task.clone();
        let (reply, _answer) = oneshot::channel();
        match self.tx.try_send(Cmd::Enqueue {
            task: Box::new(task),
            reply,
            pending,
        }) {
            Ok(()) => Ok(()),
            Err(TrySendError::Full(_)) => Err((EnqueueRefusal::Full, Box::new(backup))),
            Err(TrySendError::Closed(_)) => Err((EnqueueRefusal::Closed, Box::new(backup))),
        }
    }

    /// Hands a hook event to the worker without waiting: a full channel drops the event (the
    /// screen read covers a missed turn event) so one stalled session never holds up the others.
    pub(super) fn hook(&self, event: HookEvent, seq: u64, peer_chain: Option<Vec<u32>>) -> Result<(), EnqueueRefusal> {
        match self.tx.try_send(Cmd::Hook { event, seq, peer_chain }) {
            Ok(()) => Ok(()),
            Err(TrySendError::Full(_)) => Err(EnqueueRefusal::Full),
            Err(TrySendError::Closed(_)) => Err(EnqueueRefusal::Closed),
        }
    }
}

/// Where a task goes: the slot, and for a pool member what keeps the choice true until the enqueue
/// is over.
struct Routed {
    slot: Slot,
    pending: Option<Pending>,
    mark: Option<InFlightMark>,
}

/// The sessions of this runner.
pub struct SessionManager {
    shared: Arc<Shared>,
    /// Hook events dropped because a session's channel was full, since the runner started.
    hooks_dropped: AtomicU64,
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
        report_trigger: crate::handshake::ReportTrigger,
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
                panes: Arc::new(Mutex::new(HashMap::new())),
                report_trigger,
                workers: Mutex::new(HashMap::new()),
                pools: Mutex::new(HashMap::new()),
            }),
            hooks_dropped: AtomicU64::new(0),
        })
    }

    /// The listener's pre-check: a message is worth reading further only when the pane it names
    /// (tmux socket path and pane id) is one a session worker recorded.
    pub fn pane_check(&self) -> super::hooks::Accept {
        let panes = Arc::clone(&self.shared.panes);
        Arc::new(move |message| panes.lock().contains_key(&(message.tmux.clone(), message.pane.clone())))
    }

    fn handle(&self, name: &Slot) -> WorkerHandle {
        let (tx, rx) = mpsc::channel(WORKER_CHANNEL);
        let (status_tx, status_rx) = watch::channel(MemberStatus::fresh());
        let handle = {
            let mut workers = self.shared.workers.lock();
            if let Some(existing) = workers.get(name.as_str()) {
                return existing.clone();
            }
            let handle = WorkerHandle::new(tx.clone(), status_rx.clone());
            workers.insert(name.as_str().to_string(), handle.clone());
            handle
        };
        self.attach_to_pool(name, status_rx, Arc::clone(&handle.pending));
        let worker = Worker::new(Arc::clone(&self.shared), name.clone(), rx, tx, status_tx);
        tokio::spawn(worker.run());
        handle
    }

    /// Puts a ready-made handle in place of a worker (the channel of a session that is not run
    /// by this manager).
    #[cfg(test)]
    pub(super) fn install(&self, name: &Slot, handle: WorkerHandle) {
        self.attach_to_pool(name, handle.status.clone(), Arc::clone(&handle.pending));
        self.shared.workers.lock().insert(name.as_str().to_string(), handle);
    }

    /// Adds a bot slot to its pool (created, with the persisted affinity, on first use).
    fn attach_to_pool(&self, name: &Slot, status: watch::Receiver<MemberStatus>, pending: Arc<AtomicUsize>) {
        if !name.is_bot() {
            return;
        }
        let Ok(primary) = bot_primary(name) else { return };
        self.shared
            .pools
            .lock()
            .entry(primary.as_str().to_string())
            .or_insert_with(|| BotPool::load(&self.shared.state, &primary))
            .attach(name, status, pending);
    }

    /// Starts the workers of every member the configuration names for the bot of `slot`, so the
    /// pool knows them all before it chooses.
    fn ensure_members(&self, slot: &Slot) {
        let (Some(bot), count) = (slot.bot_id(), self.shared.member_count(slot)) else {
            return;
        };
        for member in 0..count {
            if let Ok(member) = u8::try_from(member)
                && let Ok(slot) = Slot::bot_member(bot, slot.cli(), member)
            {
                drop(self.handle(&slot));
            }
        }
    }

    /// Queues a task on its slot (the project's session, or the bot's one session shared by every
    /// project) and returns its 1-based queue position, or `None` when the session's worker did
    /// not answer within `[runner] enqueue_answer_ms`.
    ///
    /// # Errors
    ///
    /// [`EnqueueRefusal`]: the channel is full, the session holds `max_queued_per_session` tasks,
    /// or the worker has ended. The first two are back-pressure (HTTP 503).
    pub async fn enqueue(&self, mut task: TmuxTask) -> Result<Option<usize>, EnqueueRefusal> {
        let runner = &self.shared.config.runner;
        let limits = EnqueueLimits {
            answer: Duration::from_millis(runner.enqueue_answer_ms),
            max_queued: usize::try_from(runner.max_queued_per_session).unwrap_or(usize::MAX),
        };
        let input_task = task.task_id.clone();
        let input_item = task.item.clone();
        let routed = self.route(&mut task, limits.max_queued);
        let handle = self.handle(&routed.slot);
        let result = match routed.pending {
            Some(pending) => handle.enqueue_reserved(task, limits, pending).await,
            None => handle.enqueue(task, limits).await,
        };
        // Affinity follows only a task the member took (a late answer still queued it). It is
        // recorded before the in-flight mark is released, so a concurrent enqueue of the same task
        // finds the mark or the record.
        if result.is_ok() {
            self.shared
                .record_affinity(&routed.slot, &input_task, input_item.as_deref());
        }
        drop(routed.mark);
        result
    }

    /// The slot that takes `task`: the project's own, or the member of the bot's pool chosen by
    /// [`select_member`] (the bot's only session while the pool has one member). The task is
    /// pointed at the member. For a pool member the choice also counts the task against the member
    /// and marks it in flight until the enqueue is over.
    fn route(&self, task: &mut TmuxTask, max_queued: usize) -> Routed {
        let unrouted = |slot: Slot| Routed {
            slot,
            pending: None,
            mark: None,
        };
        if !task.slot.is_bot() {
            return unrouted(task.slot.clone());
        }
        let Ok(primary) = bot_primary(&task.slot) else {
            return unrouted(task.slot.clone());
        };
        // The workers of the members attach to the pool when they are created.
        self.ensure_members(&task.slot);
        drop(self.handle(&task.slot));
        let input = SelectInput {
            task_id: Some(&task.task_id),
            item: task.item.as_deref(),
        };
        let ttl = self.shared.config.runner.tmux.pool.affinity_ttl_secs;
        let chosen = self
            .shared
            .pools
            .lock()
            .get(primary.as_str())
            .and_then(|pool| pool.choose(&input, unix_now(), ttl, max_queued));
        let Some(Chosen {
            slot,
            reason,
            pending,
            mark,
        }) = chosen
        else {
            return unrouted(task.slot.clone());
        };
        timing::member_selected(
            &task.task_id,
            task.slot.bot_id().unwrap_or_default(),
            slot.member(),
            reason.as_str(),
        );
        self.shared.retarget(task, &slot);
        Routed {
            slot,
            pending: Some(pending),
            mark: Some(mark),
        }
    }

    /// Points a task rebuilt by restart recovery at the pool member `slot`.
    pub fn retarget(&self, task: &mut TmuxTask, slot: &Slot) {
        self.shared.retarget(task, slot);
    }

    /// The member of a bot's pool that holds the task `task_id` of `attempt` after a restart: the
    /// one whose record or scrollback shows it was typed in, else the one the persisted affinity
    /// names, else the one [`select_member`] picks now. `primary` itself for a pool of one member.
    pub async fn locate_member(&self, primary: &Slot, task_id: &str, attempt: i64, item: Option<&str>) -> Slot {
        if !primary.is_bot() || self.shared.member_count(primary) < 2 {
            return primary.clone();
        }
        self.ensure_members(primary);
        for member in 0..self.shared.member_count(primary) {
            let Ok(member) = u8::try_from(member) else { continue };
            let Some(slot) = primary
                .bot_id()
                .and_then(|bot| Slot::bot_member(bot, primary.cli(), member).ok())
            else {
                continue;
            };
            let probe = self.probe(&slot, task_id, attempt).await;
            if probe.recorded_pushed || (probe.alive && probe.scrollback_hit) {
                return slot;
            }
        }
        let input = SelectInput {
            task_id: Some(task_id),
            item,
        };
        let ttl = self.shared.config.runner.tmux.pool.affinity_ttl_secs;
        // No cap: a recovered task is queued whatever the member holds.
        let chosen = self
            .shared
            .pools
            .lock()
            .get(primary.as_str())
            .and_then(|pool| pool.choose(&input, unix_now(), ttl, 0));
        chosen.map_or_else(|| primary.clone(), |chosen| chosen.slot)
    }

    /// A hook event from a CLI. Never waits: see [`Self::deliver_hook`].
    pub fn hook(&self, message: HookMessage) {
        // Only a pane a session worker recorded takes events.
        let name = self
            .shared
            .panes
            .lock()
            .get(&(message.tmux.clone(), message.pane.clone()))
            .cloned();
        // codex may import claude's hooks (and the reverse): a message applies only to the
        // session of its own CLI.
        let name = name.filter(|name| hook_is_for_session(name, &message.cli));
        if let Some(name) = name {
            self.deliver_hook(&name, message.event, message.seq, message.peer_chain);
        }
    }

    /// The hook events dropped so far because a session's channel was full.
    #[cfg(test)]
    pub(super) fn hooks_dropped(&self) -> u64 {
        self.hooks_dropped.load(Ordering::Relaxed)
    }

    /// Hands a hook event to the worker of session `name`. A full channel drops the event with a
    /// warning and a running count; the other sessions are not affected.
    pub(super) fn deliver_hook(&self, name: &str, event: HookEvent, seq: u64, peer_chain: Option<Vec<u32>>) {
        let handle = self.shared.workers.lock().get(name).cloned();
        let Some(handle) = handle else { return };
        match handle.hook(event, seq, peer_chain) {
            Ok(()) => {}
            Err(EnqueueRefusal::Closed) => tracing::debug!(slot = name, "hook dropped: the worker has ended"),
            Err(why) => {
                let count = self.hooks_dropped.fetch_add(1, Ordering::Relaxed).saturating_add(1);
                tracing::warn!(slot = name, reason = why.as_str(), count, "hook dropped");
            }
        }
    }

    /// Whether the session `name` has its recorded CLI pane alive and shows `id=<task_id>` in its
    /// last 200 lines. Reads tmux only.
    pub async fn probe(&self, name: &Slot, task_id: &str, attempt: i64) -> Probe {
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
        let pane = record.as_ref().and_then(pane_of);
        let needle = format!("id={task_id}");
        tokio::task::spawn_blocking(move || {
            let gone = Probe {
                recorded_pushed,
                ..Probe::default()
            };
            let Some(pane) = pane else { return gone };
            if !tmux.has_session(&pane.name).unwrap_or(false) || !pane_alive(&tmux, &pane) {
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

    /// The tmux session `name`'s record says tasks were typed into, for messages; the slot id
    /// when there is no record.
    pub fn recorded_tmux_name(&self, name: &Slot) -> String {
        self.shared
            .state
            .read_session_record(name)
            .and_then(|record| record.tmux())
            .map_or_else(|| name.as_str().to_string(), |tmux| tmux.as_str().to_string())
    }

    /// Hands recovered tasks to the worker of `name`: the pushed one is supervised again, the rest
    /// are queued in the order given.
    pub async fn recover(&self, name: Slot, adopt: Option<Adopt>, requeue: Vec<TmuxTask>) {
        let _ = self.handle(&name).tx.send(Cmd::Recover { adopt, requeue }).await;
    }
}

/// Member 0 of the pool a bot slot belongs to.
fn bot_primary(slot: &Slot) -> Result<Slot, TmuxError> {
    Slot::bot(slot.bot_id().unwrap_or_default(), slot.cli())
}

/// The pane a record names (id and PID as recorded), in the tmux session the record names.
fn pane_of(record: &SessionRecord) -> Option<Pane> {
    Some(Pane {
        name: record.tmux()?,
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

#[allow(clippy::struct_excessive_bools, reason = "independent flags of one session worker")]
struct Worker {
    shared: Arc<Shared>,
    name: Slot,
    machine: Machine<TmuxTask>,
    pane: Option<Pane>,
    rx: mpsc::Receiver<Cmd>,
    tx: mpsc::Sender<Cmd>,
    /// No hook of this session's pane has been admitted yet: turn events are read from the screen.
    /// The first admitted hook switches to hook mode.
    screen_mode: bool,
    last_screen: Option<String>,
    still_ticks: u32,
    screen_working: bool,
    /// A codex turn end (notify) was admitted and waits for the screen to show the idle, empty
    /// composer before the turn closes.
    pending_turn_end: Option<Instant>,
    /// The newest hook event applied, by the sender's clock: an older one is ignored.
    seq_gate: super::hooks::SeqGate,
    /// Closes a turn the hooks never closed (an interrupt runs no `Stop`).
    turn_watch: TurnWatch,
    ticks: u32,
    /// This session has already asked for a report since the runner started.
    report_requested: bool,
    /// When each queued task arrived, for the timing log only.
    enqueued_at: HashMap<(String, i64), Instant>,
    /// Logs the current task leaving the session (timing log only).
    watch: timing::CurrentWatch,
    /// Throttles `queue depth` (timing log only).
    depth_beat: timing::DepthBeat,
    /// Publishes this member's status for the pool.
    status_tx: watch::Sender<MemberStatus>,
    /// This session is a member of a pool of two or more: it is watched for health and its queued
    /// tasks may move.
    pooled: bool,
    /// The member's tmux session was not there at the last look (a pooled member with nothing to
    /// do is looked at every 10 s).
    absent: bool,
    /// Why the member is unhealthy now, as last logged.
    health_cause: Option<&'static str>,
    /// The tasks this member held (queued or current) at the last look, to see which left it.
    held_ids: std::collections::HashSet<String>,
    /// When queued tasks last moved off this member.
    last_rebalance: Instant,
}

/// Whether a session's record is worth an out-of-cycle report: there was no record before, its pane
/// or process changed, or this session has not asked for one since the runner started (the first
/// delivery after start, or a recovery).
#[must_use]
pub fn report_needed(previous: Option<&SessionRecord>, new: &SessionRecord, requested_since_start: bool) -> bool {
    previous.is_none_or(|previous| {
        previous.pane_id != new.pane_id || previous.pane_pid != new.pane_pid || !requested_since_start
    })
}

/// Whether a hook message may be applied: (where the platform names the sender) the sender runs
/// below the session's pane process. Refuses with the reason. (The pane pair was checked before.)
///
/// # Errors
///
/// The reason for dropping the message.
pub fn admit_hook(chain: Option<&[u32]>, pane_pid: Option<u32>) -> Result<(), &'static str> {
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

/// Records `pane` for the hook listener: the socket path of its server (asked once, here) with its
/// id, replacing whatever the session had recorded. A server that cannot say its socket path
/// leaves the pane unrecorded, and the session runs on the screen.
fn register_pane(tmux: &Tmux, panes: &PaneMap, slot: &Slot, pane: &Pane) {
    match tmux.socket_path(pane) {
        Ok(path) => record_pane(panes, slot, pane, path),
        Err(e) => tracing::warn!(session = slot.as_str(), "no hooks for this pane: {e}"),
    }
}

fn record_pane(panes: &PaneMap, slot: &Slot, pane: &Pane, socket_path: String) {
    let mut map = panes.lock();
    map.retain(|_, name| name != slot.as_str());
    map.insert((socket_path, pane.id.as_str().to_string()), slot.as_str().to_string());
}

fn unix_now() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs())
}

impl Worker {
    fn new(
        shared: Arc<Shared>,
        name: Slot,
        rx: mpsc::Receiver<Cmd>,
        tx: mpsc::Sender<Cmd>,
        status_tx: watch::Sender<MemberStatus>,
    ) -> Self {
        let pooled = shared.member_count(&name) > 1;
        let socket = shared.socket.as_str();
        let attach = if socket.is_empty() {
            format!("tmux attach -t {}", name.as_str())
        } else {
            format!("tmux -L {socket} attach -t {}", name.as_str())
        };
        let machine = Machine::new(
            Timers::from_config(&shared.config.runner),
            name.as_str(),
            &attach,
            Instant::now(),
        );
        let mut machine = machine;
        if name.cli() == "codex" {
            machine.set_enter_policy(CODEX_ENTER_RETRIES, CODEX_ENTER_SPACING);
        }
        Self {
            shared,
            name,
            machine,
            pane: None,
            rx,
            tx,
            screen_mode: true,
            last_screen: None,
            still_ticks: 0,
            screen_working: false,
            pending_turn_end: None,
            seq_gate: super::hooks::SeqGate::default(),
            turn_watch: TurnWatch::new(3, TURN_QUIET, Instant::now()),
            ticks: 0,
            report_requested: false,
            enqueued_at: HashMap::new(),
            watch: timing::CurrentWatch::default(),
            depth_beat: timing::DepthBeat::new(Instant::now()),
            pooled,
            absent: false,
            health_cause: None,
            held_ids: std::collections::HashSet::new(),
            last_rebalance: Instant::now(),
            status_tx,
        }
    }

    /// The timing log events that follow from what changed in the machine: the current task
    /// cleared (`task left session`) and a non-empty queue (`queue depth`, every heartbeat).
    fn observe(&mut self) {
        let now = Instant::now();
        let current = self
            .machine
            .current
            .as_ref()
            .map(|c| (c.task.task_id.clone(), c.task.attempt, c.pushed_at));
        self.watch.observe(current, now, self.name.as_str(), self.name.member());
        self.publish_status(now);
        self.note_departures();
        let machine = &self.machine;
        self.enqueued_at
            .retain(|(id, attempt), _| machine.queue.iter().any(|e| &e.task_id == id && e.attempt == *attempt));
        let beat = Duration::from_secs(self.shared.config.runner.heartbeat_secs.max(1));
        self.depth_beat.tick(
            now,
            beat,
            self.name.as_str(),
            self.name.member(),
            self.machine.queue.len(),
            state_word(&self.machine.state),
        );
    }

    /// Why a pooled member is not to be trusted now: its session is absent, was declined at the last
    /// delivery, has been blocked on a dialog, or has held pushes back because of its screen for
    /// `stuck_after_secs`. A session that ended is [`State::Dead`], which the status shows on its own.
    fn unhealthy_cause(&self, now: Instant) -> Option<&'static str> {
        if !self.pooled {
            return None;
        }
        if matches!(self.machine.state, State::Dead) {
            return Some("dead");
        }
        let stuck = Duration::from_secs(self.shared.config.runner.tmux.pool.stuck_after_secs);
        let past = |since: Instant| now.saturating_duration_since(since) >= stuck;
        if self.absent && matches!(self.machine.state, State::NoSession) {
            return Some("absent");
        }
        if self.machine.declined_at().is_some_and(|at| !past(at)) {
            return Some("declined");
        }
        if let State::Blocked { since } = self.machine.state
            && past(since)
        {
            return Some("blocked");
        }
        if self.machine.held_since().is_some_and(past) {
            return Some("held");
        }
        None
    }

    /// Publishes the member's state, queue length and health for the pool, and logs a change of
    /// health.
    fn publish_status(&mut self, now: Instant) {
        let (state, queue_len) = (self.machine.state, self.machine.queue.len());
        let cause = self.unhealthy_cause(now);
        self.status_tx.send_if_modified(|status| {
            let next = status.next_checked(state, queue_len, now, cause.is_some());
            let changed = next != *status;
            *status = next;
            changed
        });
        if self.pooled && cause != self.health_cause {
            self.health_cause = cause;
            timing::member_health(
                self.name.bot_id().unwrap_or_default(),
                self.name.member(),
                cause.is_none(),
                cause.unwrap_or("recovered"),
            );
        }
    }

    /// Tells the pool which tasks have left this member, so their affinity starts to expire.
    fn note_departures(&mut self) {
        if !self.pooled {
            return;
        }
        let mut ids: std::collections::HashSet<String> = self.machine.queue.iter().map(|e| e.task_id.clone()).collect();
        if let Some(current) = self.machine.current_task() {
            ids.insert(current.task_id.clone());
        }
        let left: Vec<String> = self.held_ids.difference(&ids).cloned().collect();
        self.held_ids = ids;
        if left.is_empty() {
            return;
        }
        let Ok(primary) = bot_primary(&self.name) else { return };
        let snapshot = {
            let mut pools = self.shared.pools.lock();
            pools.get_mut(primary.as_str()).and_then(|pool| {
                let now = unix_now();
                for id in &left {
                    pool.task_left(id, now);
                }
                pool.snapshot()
            })
        };
        self.shared.persist(&primary, snapshot);
    }

    /// Looks, every 10 s, for the tmux session of a pooled member that has nothing to do: a member
    /// whose session is not there is unhealthy, and one that is back is used again. Reads tmux only.
    async fn probe_presence(&mut self) {
        let idle = matches!(self.machine.state, State::NoSession | State::Dead);
        if !self.pooled || !idle || !self.machine.queue.is_empty() {
            self.absent = false;
            return;
        }
        if !self.ticks.is_multiple_of(10) {
            return;
        }
        let alive = self.alive().await;
        self.absent = !alive;
        if alive {
            self.machine.on_probe_alive();
        }
    }

    /// Moves the queued tasks that were never typed in to other members once this member has been
    /// unhealthy for `rebalance_after_secs` and another member is healthy. The task at the head
    /// of a push under way, and the task being worked on, never move.
    async fn rebalance(&mut self) {
        let now = Instant::now();
        let config = &self.shared.config.runner;
        let after = Duration::from_secs(config.tmux.pool.rebalance_after_secs);
        let status = *self.status_tx.borrow();
        if !self.pooled
            || self.machine.queue.is_empty()
            || !status.unhealthy_for(now, after)
            || now.saturating_duration_since(self.last_rebalance) < Duration::from_secs(5)
        {
            return;
        }
        self.last_rebalance = now;
        let Ok(primary) = bot_primary(&self.name) else { return };
        let ttl = config.tmux.pool.affinity_ttl_secs;
        let max_queued = usize::try_from(config.max_queued_per_session).unwrap_or(usize::MAX);
        let others = self
            .shared
            .pools
            .lock()
            .get(primary.as_str())
            .is_some_and(|pool| pool.has_healthy_other(self.name.member()));
        if !others {
            return;
        }
        let mut kept: Vec<TmuxTask> = Vec::new();
        for entry in self.machine.take_movable() {
            let mut task = entry.item;
            let input = SelectInput {
                task_id: None,
                item: task.item.as_deref(),
            };
            let chosen = self
                .shared
                .pools
                .lock()
                .get(primary.as_str())
                .and_then(|pool| pool.choose(&input, unix_now(), ttl, max_queued))
                .filter(|chosen| chosen.slot != self.name);
            let target = chosen.as_ref().and_then(|chosen| {
                self.shared
                    .workers
                    .lock()
                    .get(chosen.slot.as_str())
                    .cloned()
                    .map(|handle| (handle, chosen.slot.clone()))
            });
            let (Some(chosen), Some((handle, to))) = (chosen, target) else {
                kept.push(task);
                continue;
            };
            let task_id = task.task_id.clone();
            let item = task.item.clone();
            self.shared.retarget(&mut task, &to);
            match handle.enqueue_nowait(task, chosen.pending) {
                Ok(()) => {
                    self.shared.record_affinity(&to, &task_id, item.as_deref());
                    timing::rebalance(
                        self.name.bot_id().unwrap_or_default(),
                        self.name.member(),
                        to.member(),
                        &task_id,
                    );
                }
                Err((_, mut back)) => {
                    self.shared.retarget(&mut back, &self.name);
                    kept.push(*back);
                }
            }
        }
        let mut actions = Vec::new();
        for task in kept {
            actions.extend(self.machine.enqueue(task, now));
        }
        self.carry_out(actions).await;
    }

    /// Asks for an out-of-cycle report when [`report_needed`] says so.
    fn request_report(&mut self, previous: Option<&SessionRecord>, new: &SessionRecord) {
        if report_needed(previous, new, self.report_requested) {
            self.report_requested = true;
            self.shared.report_trigger.request();
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
                    actions.extend(self.confirm_turn_end_by_screen().await);
                    actions.extend(self.close_stale_turn().await);
                    self.probe_presence().await;
                    actions.extend(self.machine.on_tick(Instant::now()));
                    actions
                }
                command = self.rx.recv() => match command {
                    None => return,
                    Some(command) => self.handle(command).await,
                },
            };
            self.carry_out(actions).await;
            self.observe();
            self.rebalance().await;
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
                        // A bot's session serves several projects: its record names the last task's.
                        .filter(|record| self.name.is_bot() || record.project_id == adopt.task.project_id)
                        .and_then(pane_of);
                    let tmux = self.tmux();
                    let panes = Arc::clone(&self.shared.panes);
                    let slot = self.name.clone();
                    let alive = match pane.clone() {
                        Some(pane) => tokio::task::spawn_blocking(move || {
                            let alive = pane_alive(&tmux, &pane);
                            if alive {
                                register_pane(&tmux, &panes, &slot, &pane);
                            }
                            alive
                        })
                        .await
                        .unwrap_or(false),
                        None => false,
                    };
                    if let (Some(pane), Some(recorded), true) = (pane, record.clone(), alive) {
                        self.pane = Some(pane);
                        // Until a hook of the pane is admitted, the screen tells the turns.
                        self.screen_mode = true;
                        self.machine.adopt_running(now);
                        // The recovered session is what Sylvode should now be told about.
                        self.request_report(Some(&recorded), &recorded);
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
                        let shown = record
                            .as_ref()
                            .and_then(SessionRecord::tmux)
                            .map_or_else(|| self.name.as_str().to_string(), |tmux| tmux.as_str().to_string());
                        let reason = format!("runner restarted and session {shown} was gone");
                        actions.push(Action::Fail {
                            task: *adopt.task,
                            reason,
                            retryable: true,
                        });
                    }
                }
                for task in requeue {
                    self.enqueued_at.insert((task.task_id.clone(), task.attempt), now);
                    actions.extend(self.machine.enqueue(task, now));
                }
                actions
            }
            Cmd::Enqueue {
                task,
                reply,
                pending: _pending,
            } => {
                let id = task.task_id.clone();
                if self
                    .machine
                    .current_task()
                    .is_some_and(|t| t.task_id == id && t.attempt < task.attempt)
                {
                    self.watch.hint("superseded");
                }
                self.enqueued_at.entry((id.clone(), task.attempt)).or_insert(now);
                let actions = self.machine.enqueue(*task, now);
                // The queue length is published before the answer and before the pending count is
                // released, so the pool never sees the task in neither.
                self.publish_status(now);
                let _ = reply.send(self.machine.queue.position(&id).unwrap_or(0));
                actions
            }
            Cmd::Hook { event, seq, peer_chain } => {
                // The message came from this session's pane (the listener checked the pair) and
                // from below the pane's process; a session whose record cannot be read admits
                // nothing.
                let pane_pid = self.shared.state.read_session_record(&self.name).map(|r| r.pane_pid);
                if let Err(why) = admit_hook(peer_chain.as_deref(), pane_pid) {
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
                if event == HookEvent::TurnEnded {
                    // codex's notify can come before the screen is calm (and more than once per
                    // prompt): the turn closes when a screen poll shows the idle composer.
                    self.pending_turn_end = Some(now);
                    return Vec::new();
                }
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
                let push_started = Instant::now();
                let outcome = self.try_push(&task).await;
                if let Some(word) = pushed_word(&outcome) {
                    let queued = self
                        .enqueued_at
                        .get(&(task.task_id.clone(), task.attempt))
                        .map_or(Duration::ZERO, |at| push_started.saturating_duration_since(*at));
                    timing::task_pushed(
                        &task.task_id,
                        task.attempt,
                        self.name.as_str(),
                        self.name.member(),
                        queued,
                        push_started.elapsed(),
                        word,
                    );
                }
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
                    self.watch.hint("gone");
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

    /// The recorded pane of this session, if a record names one.
    fn recorded_pane(&self) -> Option<Pane> {
        self.shared
            .state
            .read_session_record(&self.name)
            .and_then(|record| pane_of(&record))
    }

    /// The tmux session this slot's tasks go to: that of the task being served or next in line,
    /// else the one the record names, else the one the slot's name or configuration gives.
    fn target(&self) -> Option<TmuxName> {
        self.machine
            .current_task()
            .or_else(|| self.machine.queue.front().map(|entry| &entry.item))
            .map(|task| task.tmux.clone())
            .or_else(|| {
                self.shared
                    .state
                    .read_session_record(&self.name)
                    .and_then(|record| record.tmux())
            })
            .or_else(|| super::naming::tmux_of_slot(&self.shared.config.runner, &self.name).map(|(tmux, _)| tmux))
    }

    async fn alive(&self) -> bool {
        let Some(pane) = self.pane.clone() else {
            // No pane is known yet; a session that exists is looked for once.
            let tmux = self.tmux();
            let Some(name) = self.target() else {
                return false;
            };
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
        tokio::task::spawn_blocking(move || tmux.capture_pane_styled(&pane, None).ok())
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

    /// Closes the turn a codex notification announced once the screen shows the CLI idle with an
    /// empty composer. A notification that finds no open turn (the screen already closed it, or a
    /// stray one) is dropped.
    async fn confirm_turn_end_by_screen(&mut self) -> Vec<Action<TmuxTask>> {
        if self.pending_turn_end.is_none() || self.pane.is_none() || !self.ticks.is_multiple_of(2) {
            return Vec::new();
        }
        if !self.machine.turn_open() && !matches!(self.machine.state, State::Pushing { .. }) {
            self.pending_turn_end = None;
            return Vec::new();
        }
        let Some(cli) = self.recorded_cli() else {
            return Vec::new();
        };
        let Some(screen) = self.capture().await else {
            return Vec::new();
        };
        if !turn_end_confirmed(&cli, &screen) {
            return Vec::new();
        }
        self.pending_turn_end = None;
        let now = Instant::now();
        self.turn_watch.event(now);
        self.machine.on_hook(HookEvent::TurnEnded, now)
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

    /// A launch began: the order counter starts over.
    fn launched(&mut self) {
        self.seq_gate = super::hooks::SeqGate::default();
        self.pending_turn_end = None;
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
        let socket = state.hook_socket();
        let config = &self.shared.config.runner;
        let Some(settings) = config.cli_settings(&task.cli) else {
            return Err("the CLI cannot run in a tmux session".to_string());
        };
        let program = settings
            .launch_command
            .first()
            .and_then(|word| sylvode_prereq::Checker::new(sylvode_prereq::InstallKind::RunnerTmux).locate(word))
            .ok_or_else(|| "the launch command of the CLI was not found on this runner".to_string())?;
        let argv = if task.cli == "codex" {
            let toml = state.mcp_toml(&task.bot.bot_id);
            let files = launch::CodexLaunchFiles {
                mcp_server: &task.mcp_server,
                mcp_toml: &toml,
                runner_exe: &self.shared.runner_exe,
                hook_socket: &socket,
            };
            launch::codex_session_argv(&program, &config.cli.codex, &files)
        } else {
            let claude_json = state.claude_mcp_json(&task.bot.bot_id);
            let claude_settings = state.claude_settings(&self.name);
            let files = LaunchFiles {
                claude_mcp_json: &claude_json,
                claude_settings: &claude_settings,
                runner_exe: &self.shared.runner_exe,
                hook_socket: &socket,
            };
            launch::session_argv(&program, &config.cli.claude, &files)
        }
        .map_err(|e| {
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
        // The slot can be served by another tmux session than before (the configuration changed):
        // the pane of the old one is let go.
        if self.pane.as_ref().is_some_and(|pane| pane.name != task.tmux) {
            self.pane = None;
        }
        let process_names = process_names(&self.shared.config, &task.cli);
        let tmux = self.tmux();
        let name = self.name.clone();
        let tmux_cfg = self.shared.config.runner.tmux.clone();
        let record = self.shared.state.read_session_record(&name);
        let previous_record = record.clone();
        let task = task.clone();
        let job = self.settings_job(&task);
        // The pane is recorded for the hook listener inside the blocking call, right after it is
        // known: its first hook (`SessionStart`) may arrive before the call returns.
        let panes = Arc::clone(&self.shared.panes);
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
                &panes,
            )
        })
        .await;
        let Ok(result) = result else {
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
                if launched {
                    self.launched();
                }
                if let Some(record) = record {
                    // Until a hook of the pane is admitted, the screen tells the turns.
                    self.screen_mode = true;
                    match self.shared.state.write_session_record(&record) {
                        Ok(()) => self.request_report(previous_record.as_ref(), &record),
                        Err(e) => {
                            tracing::warn!("could not save session.json of {}: {e}", self.name.as_str());
                        }
                    }
                }
                outcome
            }
            Err(e) => {
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
        let settle = Duration::from_millis(self.shared.config.runner.push_settle_ms(&task.cli));
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
        let dir = task.dir.to_string_lossy();
        if let Err(e) = self.shared.state.begin_push(&self.name, pushed, &task.project_id, &dir) {
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
        let settle = Duration::from_millis(self.shared.config.runner.push_settle_ms(&task.cli));
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

/// Writes the hook settings (which hold no secret) just before the CLI is started in a new
/// session.
struct SettingsJob {
    state: Arc<StateDir>,
    name: Slot,
    /// Hooks are on (claude, `[runner.cli.claude] hooks`).
    enabled: bool,
    runner_exe: PathBuf,
}

impl SettingsJob {
    fn write(&self) -> Result<(), TmuxError> {
        if !self.enabled {
            return Ok(());
        }
        let claude_json = self.state.root().join("unused.json");
        let settings = self.state.claude_settings(&self.name);
        let socket = self.state.hook_socket();
        let files = LaunchFiles {
            claude_mcp_json: &claude_json,
            claude_settings: &settings,
            runner_exe: &self.runner_exe,
            hook_socket: &socket,
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
    config
        .runner
        .cli_settings(cli)
        .map_or_else(|| vec![cli.to_string()], |settings| settings.process_names.to_vec())
}

/// codex takes a submitted paste a moment late: up to two Enter retries, never closer than one
/// second to the previous Enter (an Enter within about 10 ms of input inserts a newline).
const CODEX_ENTER_RETRIES: u8 = 2;
const CODEX_ENTER_SPACING: Duration = Duration::from_secs(1);

/// Whether a screen shows the CLI idle with an empty composer: no busy line, no choice waiting,
/// and an input line with nothing on it.
pub fn turn_end_confirmed(cli: &str, screen: &str) -> bool {
    !looks_busy(cli, screen) && !awaits_choice(cli, screen) && input_is_empty(cli, screen) == Some(true)
}

/// Whether a hook message of CLI `cli` belongs to the session called `session`.
pub fn hook_is_for_session(session: &str, cli: &str) -> bool {
    Slot::parse(session).is_ok_and(|name| name.cli() == cli)
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
    slot: &Slot,
    task: &TmuxTask,
    create: Create,
    process_names: &[String],
    record: Option<&SessionRecord>,
    job: &SettingsJob,
    cfg: &super::config::RunnerTmuxConfig,
    panes_for_hooks: &PaneMap,
) -> Result<EnsureResult, TmuxError> {
    let declined = |reason: String, retryable: bool| EnsureResult {
        outcome: EnsureOutcome::Declined { reason, retryable },
        pane: None,
        record: None,
        launched: false,
    };
    let target = &task.tmux;
    let new_record = |pane_id: &PaneId, pid: u32, created_by: CreatedBy| SessionRecord {
        name: slot.as_str().to_string(),
        project_id: task.project_id.clone(),
        bot_id: task.bot.bot_id.clone(),
        cli: task.cli.clone(),
        pane_id: pane_id.as_str().to_string(),
        pane_pid: pid,
        dir: task.dir.to_string_lossy().into_owned(),
        created_by,
        pushed: None,
        tmux_name: target.as_str().to_string(),
        name_source: task.name_source.as_str().to_string(),
    };
    let exists = tmux.has_session(target)?;
    let panes = if exists { tmux.list_panes(target)? } else { Vec::new() };
    let decision = decide_ensure(&EnsureFacts {
        session_exists: exists,
        panes: &panes,
        process_names,
        allow_create: task.name_source == NameSource::Derived,
        record,
        task_bot_id: &task.bot.bot_id,
    });
    if let Some((reason, retryable)) = decline_for(&decision, target.as_str(), &task.cli, task.name_source) {
        return Ok(declined(reason, retryable));
    }
    match decision {
        EnsureDecision::Create => {
            let (dir, argv) = match create {
                Ok(create) => create,
                Err(reason) => return Ok(declined(reason, false)),
            };
            if !task.dir_checked {
                return Ok(declined(
                    format!(
                        "tmux session {} is gone and its working directory is not inside this runner's allowed roots, \
                         so the runner will not start one there",
                        target.as_str()
                    ),
                    true,
                ));
            }
            // The session is created long after the delivery was checked: the directory must still
            // be the canonical path that was accepted (no link swapped in meanwhile).
            if !dir_unchanged(&task.dir) {
                return Ok(declined(
                    "the working directory changed since the task was accepted".to_string(),
                    false,
                ));
            }
            let created = job
                .write()
                .and_then(|()| tmux.new_session(target, &dir, cfg.window_width, cfg.window_height, &argv));
            let (pane, socket_path) = match created {
                Ok(created) => (created.pane, created.socket_path),
                Err(e) => {
                    tracing::error!("tmux session {} was not created: {e}", target.as_str());
                    return Ok(declined(
                        format!("the runner could not start tmux session {}", target.as_str()),
                        true,
                    ));
                }
            };
            // Recorded right away, from the answer of `new-session` itself: the CLI's first hook
            // may arrive within milliseconds.
            match socket_path {
                Some(path) => record_pane(panes_for_hooks, slot, &pane, path),
                None => register_pane(tmux, panes_for_hooks, slot, &pane),
            }
            let record = new_record(&pane.id, pane.pid, CreatedBy::Runner);
            Ok(EnsureResult {
                outcome: EnsureOutcome::Started,
                pane: Some(pane),
                record: Some(record),
                launched: true,
            })
        }
        EnsureDecision::Use { pane, created_by } => {
            let previous = record;
            let pane = Pane {
                name: target.clone(),
                id: pane.id.clone(),
                pid: pane.pid,
            };
            register_pane(tmux, panes_for_hooks, slot, &pane);
            let mut record = new_record(&pane.id, pane.pid, created_by);
            // The same pane keeps what is recorded of the task typed into it.
            record.pushed = previous
                .filter(|r| r.pane_id == pane.id.as_str() && r.pane_pid == pane.pid)
                .and_then(|r| r.pushed.clone());
            Ok(EnsureResult {
                outcome: EnsureOutcome::Running,
                pane: Some(pane),
                record: Some(record),
                launched: false,
            })
        }
        EnsureDecision::Absent
        | EnsureDecision::NotRunning
        | EnsureDecision::Ambiguous
        | EnsureDecision::BotMismatch => Ok(declined("the session cannot be used".to_string(), false)),
    }
}

/// The gate of the one Enter retry: the same preconditions as a push, plus the pasted text still
/// being the last thing on the input line. Pressing Enter blind could submit a person's draft or
/// confirm a dialog's default.
fn enter_retry_allowed(tmux: &Tmux, pane: &Pane, cli: &str, process_names: &[String], user_idle_secs: u64) -> bool {
    let Ok(screen) = tmux.capture_pane_styled(pane, None) else {
        return false;
    };
    if classify_bottom(cli, &screen).is_some() {
        return false;
    }
    let pasted = profile(cli).is_some_and(|p| input_text(cli, &screen).is_some_and(|text| p.has_pasted_marker(&text)));
    if !pasted {
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
pub enum PushTry {
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

/// The `outcome` word of `task pushed` for a push the machine counts as typed.
pub const fn pushed_word(outcome: &PushTry) -> Option<&'static str> {
    match outcome {
        PushTry::Pushed => Some("pushed"),
        PushTry::Unknown => Some("unknown"),
        PushTry::PastedNotSubmitted => Some("pasted_not_submitted"),
        _ => None,
    }
}

/// The state word of `queue depth`.
pub const fn state_word(state: &State) -> &'static str {
    match state {
        State::NoSession => "no_session",
        State::Starting { .. } => "starting",
        State::Idle => "idle",
        State::Pushing { .. } => "pushing",
        State::Busy => "busy",
        State::BusyUser => "busy_user",
        State::Blocked { .. } => "blocked",
        State::Dead => "dead",
    }
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
    let Ok(screen) = tmux.capture_pane_styled(pane, None) else {
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
