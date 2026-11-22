//! The hook socket (design section 3.5): the CLIs report turn events by running
//! `sylvode-runner hook ...`, which sends one JSON line to `state_dir/hook.sock`.
//!
//! The socket accepts exactly one message shape,
//! `{"v":2,"cli","event","tmux","pane","seq","ts"}`, with a known CLI, an event that CLI can send,
//! the socket path of the tmux server and the pane id the sender runs in (both read by the `hook`
//! subcommand from `TMUX` and `TMUX_PANE`), in at most [`MAX_LINE`] bytes. Anything else, version 1
//! lines included, is dropped with a `debug` log of its length only: hook input can hold prompt
//! text, which must not reach logs. The socket lives in the private state directory and is set to
//! mode 0600. No secret travels: the runner admits a message by who sent it (same user), where it
//! says it ran (a pane a worker recorded) and whether the sender is below that pane's process.
//!
//! The `hook` subcommand ([`send`]) never reads stdin, ignores extra arguments, does nothing
//! outside tmux, and gives up within [`CLIENT_BUDGET`]: it must never block the CLI that runs it.

use std::io::Write;
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt, PermissionsExt};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use serde_json::Value;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixListener;
use tokio::sync::mpsc;

use super::tmux::PaneId;

/// The longest hook line accepted, in bytes (newline included).
pub const MAX_LINE: usize = 1024;

/// How long the `hook` subcommand may take in all.
pub const CLIENT_BUDGET: Duration = Duration::from_millis(900);

/// How long a connection may take to deliver its line.
const READ_TIMEOUT: Duration = Duration::from_secs(2);

/// A turn event of a CLI.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HookEvent {
    /// claude: the session is interactive.
    SessionStart,
    /// claude: a prompt was submitted (by the runner or by a person).
    UserPromptSubmit,
    /// claude: the turn ended.
    Stop,
    /// claude: a permission prompt or other notification.
    Notification,
    /// codex: the turn ended.
    TurnEnded,
}

impl HookEvent {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::SessionStart => "SessionStart",
            Self::UserPromptSubmit => "UserPromptSubmit",
            Self::Stop => "Stop",
            Self::Notification => "Notification",
            Self::TurnEnded => "turn-ended",
        }
    }

    /// Whether this event ends a turn.
    pub const fn ends_turn(self) -> bool {
        matches!(self, Self::Stop | Self::TurnEnded)
    }

    fn parse(cli: &str, event: &str) -> Option<Self> {
        match (cli, event) {
            ("claude", "SessionStart") => Some(Self::SessionStart),
            ("claude", "UserPromptSubmit") => Some(Self::UserPromptSubmit),
            ("claude", "Stop") => Some(Self::Stop),
            ("claude", "Notification") => Some(Self::Notification),
            ("codex", "turn-ended") => Some(Self::TurnEnded),
            _ => None,
        }
    }
}

/// A valid hook message.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HookMessage {
    pub cli: String,
    pub event: HookEvent,
    /// The socket path of the tmux server the sender runs under.
    pub tmux: String,
    /// The pane id (`%N`) the sender runs in.
    pub pane: String,
    /// The pane's own hook counter: events of one pane are applied in this order, and one older
    /// than the last applied is ignored.
    pub seq: u64,
    /// The sender and its parents, innermost first, read while the sender was still connected
    /// (where the platform names the peer): the session's pane process must be among them.
    pub peer_chain: Option<Vec<u32>>,
}

/// The process `pid` and its parents, innermost first (at most 64). On Linux each level is read
/// from `/proc/<pid>/stat`, with no process spawned. macOS offers no safe call for it (`sysctl` and
/// `proc_pidinfo` need `unsafe`, which this crate denies), so the process table is read once with
/// one `ps` and walked in memory. The chain ends at the first process that cannot be read.
#[must_use]
pub fn ancestry(pid: u32) -> Vec<u32> {
    #[cfg(target_os = "linux")]
    let parent_of = |pid: u32| -> Option<u32> {
        // `pid (comm) state ppid ...`: the name may hold spaces and parentheses, so cut at the last
        // closing parenthesis.
        let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
        let after = stat.rsplit_once(')')?.1;
        after.split_whitespace().nth(1)?.parse().ok()
    };
    #[cfg(not(target_os = "linux"))]
    let table = process_table();
    #[cfg(not(target_os = "linux"))]
    let parent_of = |pid: u32| -> Option<u32> { table.get(&pid).copied() };
    let mut chain = vec![pid];
    let mut current = pid;
    for _ in 0..64 {
        if current <= 1 {
            break;
        }
        let Some(parent) = parent_of(current) else { break };
        if parent == current || chain.contains(&parent) {
            break;
        }
        chain.push(parent);
        current = parent;
    }
    chain
}

/// `pid -> ppid` of every process, from one `ps` call with a cleared environment.
#[cfg(not(target_os = "linux"))]
fn process_table() -> std::collections::HashMap<u32, u32> {
    let output = std::process::Command::new("/bin/ps")
        .env_clear()
        .args(["-A", "-o", "pid=", "-o", "ppid="])
        .output();
    let mut table = std::collections::HashMap::new();
    if let Ok(output) = output {
        for line in String::from_utf8_lossy(&output.stdout).lines() {
            let mut fields = line.split_whitespace();
            if let (Some(pid), Some(ppid)) = (
                fields.next().and_then(|v| v.parse::<u32>().ok()),
                fields.next().and_then(|v| v.parse::<u32>().ok()),
            ) {
                table.insert(pid, ppid);
            }
        }
    }
    table
}

/// Whether a peer with user id `peer` may speak to a runner running as `own`.
#[must_use]
pub const fn is_same_user(peer: u32, own: u32) -> bool {
    peer == own
}

/// Whether a message from a sender with this parent chain may reach the session whose pane process
/// is `pane_pid`. Where the platform names the peer (`Some` chain) the pane must be in the chain,
/// and an unknown pane is a refusal (fail closed); where it does not (`None`), the pane pair alone
/// decides.
#[must_use]
pub fn chain_allows(chain: Option<&[u32]>, pane_pid: Option<u32>) -> bool {
    match (chain, pane_pid) {
        (None, _) => true,
        (Some(chain), Some(pane)) => chain.contains(&pane),
        (Some(_), None) => false,
    }
}

/// Where a hook of this process runs: the tmux server's socket path and PID (from `TMUX`, which
/// tmux sets as `<socket path>,<server pid>,<session index>`) and the pane (from `TMUX_PANE`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PaneIdentity {
    pub socket: String,
    pub server_pid: u32,
    pub pane: String,
}

impl PaneIdentity {
    /// `None` outside tmux and for any value that is not well formed: the hook then does nothing.
    #[must_use]
    pub fn from_env(tmux: Option<&str>, pane: Option<&str>) -> Option<Self> {
        let mut parts = tmux?.rsplitn(3, ',');
        let (_index, pid, socket) = (parts.next()?, parts.next()?, parts.next()?);
        let server_pid: u32 = pid.parse().ok()?;
        let pane = pane?;
        PaneId::new(pane).ok()?;
        valid_socket_path(socket).then(|| Self {
            socket: socket.to_string(),
            server_pid,
            pane: pane.to_string(),
        })
    }

    /// The counter file name part: `<server pid>-<pane number>`.
    fn counter(&self) -> String {
        format!("{}-{}", self.server_pid, self.pane.trim_start_matches('%'))
    }
}

fn valid_socket_path(path: &str) -> bool {
    path.starts_with('/') && path.len() <= 512 && !path.chars().any(char::is_control)
}

/// The next number of a pane's own counter, kept in `<state_dir>/panes/<server pid>-<N>.seq` under
/// a lock. Every hook of a pane takes the next number, so numbers follow the order the hooks ran
/// in, whatever the wall clock does. `None` when the counter cannot be used.
pub fn next_seq(state_dir: &Path, who: &PaneIdentity) -> Option<u64> {
    use std::io::{Read, Seek, SeekFrom};
    let dir = state_dir.join("panes");
    std::fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(&dir)
        .ok()?;
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .mode(0o600)
        .open(dir.join(format!("{}.seq", who.counter())))
        .ok()?;
    let started = std::time::Instant::now();
    let mut locked = loop {
        match nix::fcntl::Flock::lock(file, nix::fcntl::FlockArg::LockExclusiveNonblock) {
            Ok(locked) => break locked,
            Err((back, _)) if started.elapsed() < Duration::from_millis(300) => {
                file = back;
                std::thread::sleep(Duration::from_millis(5));
            }
            Err(_) => return None,
        }
    };
    let mut text = String::new();
    locked.read_to_string(&mut text).ok()?;
    let next = text.trim().parse::<u64>().unwrap_or(0).checked_add(1)?;
    locked.seek(SeekFrom::Start(0)).ok()?;
    locked.set_len(0).ok()?;
    locked.write_all(next.to_string().as_bytes()).ok()?;
    Some(next)
}

/// The counter file of a pane, for `doctor` (its modification time is when a hook last ran).
#[must_use]
pub fn seq_file(state_dir: &Path, server_pid: u32, pane: &str) -> PathBuf {
    state_dir
        .join("panes")
        .join(format!("{server_pid}-{}.seq", pane.trim_start_matches('%')))
}

/// Applies events of one session in the order their senders stamped them: an event not newer than
/// the last applied one is stale (it was overtaken on its way in) and is ignored.
#[derive(Debug, Default)]
pub struct SeqGate {
    last: u64,
}

impl SeqGate {
    /// Whether an event stamped `seq` is to be applied; applying it moves the gate forward.
    pub const fn accept(&mut self, seq: u64) -> bool {
        if seq <= self.last {
            return false;
        }
        self.last = seq;
        true
    }
}

/// Why a line was dropped.
#[derive(Debug, PartialEq, Eq)]
pub enum Reject {
    TooLong,
    NotJson,
    Shape,
}

/// Parses one line (without its newline) into a message, or says why not.
///
/// # Errors
///
/// [`Reject`].
pub fn parse_line(line: &[u8]) -> Result<HookMessage, Reject> {
    if line.len() >= MAX_LINE {
        return Err(Reject::TooLong);
    }
    let value: Value = serde_json::from_slice(line).map_err(|_| Reject::NotJson)?;
    let object = value.as_object().ok_or(Reject::Shape)?;
    if object.len() != 7
        || object.get("v").and_then(Value::as_u64) != Some(2)
        || object.get("ts").and_then(Value::as_u64).is_none()
    {
        return Err(Reject::Shape);
    }
    let text = |key: &str| object.get(key).and_then(Value::as_str);
    let (Some(cli), Some(event), Some(tmux), Some(pane)) = (text("cli"), text("event"), text("tmux"), text("pane"))
    else {
        return Err(Reject::Shape);
    };
    let seq = object.get("seq").and_then(Value::as_u64).ok_or(Reject::Shape)?;
    if !valid_socket_path(tmux) {
        return Err(Reject::Shape);
    }
    PaneId::new(pane).map_err(|_| Reject::Shape)?;
    let event = HookEvent::parse(cli, event).ok_or(Reject::Shape)?;
    Ok(HookMessage {
        cli: cli.to_string(),
        event,
        tmux: tmux.to_string(),
        pane: pane.to_string(),
        seq,
        peer_chain: None,
    })
}

/// Listens on `path` and forwards valid messages to `tx` until `tx` is closed.
///
/// A stale socket file from an earlier run is removed first (the state directory is locked by one
/// runner, so it is ours). The socket is set to mode 0600 right after it is bound; the directory
/// around it is 0700 anyway.
///
/// # Errors
///
/// A message when the socket cannot be bound.
pub fn listen(path: &Path, tx: mpsc::Sender<HookMessage>) -> Result<tokio::task::JoinHandle<()>, String> {
    listen_with_check(path, tx, Duration::from_secs(5))
}

/// The identity of the socket file: device and inode.
fn identity(path: &Path) -> Option<(u64, u64)> {
    use std::os::unix::fs::MetadataExt;
    std::fs::symlink_metadata(path)
        .ok()
        .map(|meta| (meta.dev(), meta.ino()))
}

fn bind(path: &Path) -> Result<UnixListener, String> {
    if std::fs::symlink_metadata(path).is_ok() {
        std::fs::remove_file(path).map_err(|e| format!("could not remove the old hook socket: {}", e.kind()))?;
    }
    let listener = UnixListener::bind(path).map_err(|e| format!("could not bind the hook socket: {}", e.kind()))?;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))
        .map_err(|e| format!("could not restrict the hook socket: {}", e.kind()))?;
    Ok(listener)
}

/// [`listen`], looking every `every` whether the socket file is still the one that was bound: when
/// it was removed or replaced, every hook would be lost silently, so the listener binds again and
/// logs it at `error`.
///
/// # Errors
///
/// A message when the socket cannot be bound.
pub fn listen_with_check(
    path: &Path,
    tx: mpsc::Sender<HookMessage>,
    every: Duration,
) -> Result<tokio::task::JoinHandle<()>, String> {
    listen_checked(path, tx, every, Arc::new(|_| true))
}

/// What decides, per message, whether it may go on (before the sender's ancestry is read, which
/// costs): the pane the message names is one a session worker recorded.
pub type Accept = Arc<dyn Fn(&HookMessage) -> bool + Send + Sync>;

/// The most connections read at once; a flood of same-user connections cannot use up the
/// descriptors of the runner.
const MAX_READERS: usize = 32;

/// [`listen_with_check`] with `accept`: a message it refuses is dropped without any further work
/// (no ancestry walk) and logged at most once every ten seconds.
///
/// # Errors
///
/// A message when the socket cannot be bound.
pub fn listen_checked(
    path: &Path,
    tx: mpsc::Sender<HookMessage>,
    every: Duration,
    accept: Accept,
) -> Result<tokio::task::JoinHandle<()>, String> {
    let mut listener = bind(path)?;
    let mut bound = identity(path);
    let path = path.to_path_buf();
    let readers = Arc::new(tokio::sync::Semaphore::new(MAX_READERS));
    let last_warn = Arc::new(AtomicU64::new(0));
    Ok(tokio::spawn(async move {
        let mut ticker = tokio::time::interval(every);
        loop {
            let accepted = tokio::select! {
                accepted = listener.accept() => accepted,
                _ = ticker.tick() => {
                    if identity(&path) != bound {
                        tracing::error!("the hook socket was removed or replaced; binding it again");
                        match bind(&path) {
                            Ok(fresh) => {
                                listener = fresh;
                                bound = identity(&path);
                            }
                            Err(e) => tracing::error!("{e}"),
                        }
                    }
                    continue;
                }
            };
            let Ok((stream, _)) = accepted else {
                tokio::time::sleep(Duration::from_millis(100)).await;
                continue;
            };
            if tx.is_closed() {
                return;
            }
            // Only the runner's own user may speak here, whatever the socket's mode says: asked
            // first, while the sender is certainly connected, before anything is read or spent.
            let peer = stream.peer_cred().ok();
            let same_user = peer
                .as_ref()
                .is_some_and(|peer| is_same_user(peer.uid(), nix::unistd::Uid::effective().as_raw()));
            if !same_user {
                tracing::debug!("hook connection dropped: the peer is not this user");
                continue;
            }
            let Ok(permit) = Arc::clone(&readers).try_acquire_owned() else {
                tracing::debug!("hook connection dropped: too many at once");
                continue;
            };
            let peer_pid = peer.and_then(|peer| peer.pid()).and_then(|pid| u32::try_from(pid).ok());
            let (tx, accept, last_warn) = (tx.clone(), Arc::clone(&accept), Arc::clone(&last_warn));
            tokio::spawn(async move {
                let _permit = permit;
                let mut stream = stream;
                let read = tokio::time::timeout(READ_TIMEOUT, read_line(&mut stream)).await;
                let message = match read {
                    Ok(Some(line)) => match parse_line(&line) {
                        Ok(message) => Some(message),
                        Err(reject) => {
                            tracing::debug!(length = line.len(), ?reject, "hook line dropped");
                            None
                        }
                    },
                    Ok(None) => {
                        tracing::debug!("hook line dropped: too long or unreadable");
                        None
                    }
                    Err(_) => {
                        tracing::debug!("hook connection dropped: no line in time");
                        None
                    }
                };
                // Only a message the worker would accept earns the ancestry walk.
                let message = match message {
                    Some(message) if accept(&message) => Some(message),
                    Some(_) => {
                        warn_rarely(&last_warn, "a hook message from a pane no session uses was dropped");
                        None
                    }
                    None => None,
                };
                let chain = match (&message, peer_pid) {
                    (Some(_), Some(pid)) => tokio::task::spawn_blocking(move || ancestry(pid)).await.ok(),
                    _ => None,
                };
                // The sender waited for this: it is still connected while its chain is read.
                let _ = stream.write_all(b"k").await;
                if let Some(mut message) = message {
                    message.peer_chain = chain;
                    let _ = tx.send(message).await;
                }
            });
        }
    }))
}

/// Logs at `warn` at most once every ten seconds.
fn warn_rarely(last: &AtomicU64, text: &str) {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());
    let before = last.load(Ordering::Relaxed);
    if now.saturating_sub(before) >= 10
        && last
            .compare_exchange(before, now, Ordering::Relaxed, Ordering::Relaxed)
            .is_ok()
    {
        tracing::warn!("{text}");
    }
}

/// Reads one line of at most [`MAX_LINE`] bytes; `None` when it is longer or the read fails.
async fn read_line(stream: &mut tokio::net::UnixStream) -> Option<Vec<u8>> {
    let mut reader = BufReader::new(stream.take((MAX_LINE + 1) as u64));
    let mut line = Vec::new();
    reader.read_until(b'\n', &mut line).await.ok()?;
    if line.last() == Some(&b'\n') {
        line.pop();
    }
    (line.len() < MAX_LINE).then_some(line)
}

/// The `hook` subcommand: sends one message to `socket` and returns. Never reads stdin, never
/// panics, does nothing outside tmux (`who` is `None`), and returns within [`CLIENT_BUDGET`] even
/// when the socket is absent or stuck.
pub fn send(socket: &Path, cli: &str, event: &str, who: Option<&PaneIdentity>) {
    let Some(who) = who else { return };
    // The order stamp is the pane's own counter (next to the socket), not a clock.
    let Some(seq) = socket.parent().and_then(|dir| next_seq(dir, who)) else {
        return;
    };
    let ts = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());
    let line = serde_json::json!({
        "v": 2, "cli": cli, "event": event, "tmux": who.socket, "pane": who.pane, "seq": seq, "ts": ts,
    })
    .to_string();
    let socket: PathBuf = socket.to_path_buf();
    let (done, wait) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        if let Ok(mut stream) = UnixStream::connect(&socket) {
            let _ = stream.set_write_timeout(Some(Duration::from_millis(300)));
            if stream.write_all(format!("{line}\n").as_bytes()).is_ok() {
                // Stay connected until the runner has looked at us (it checks who we are).
                let _ = stream.set_read_timeout(Some(Duration::from_millis(550)));
                let mut ack = [0u8; 1];
                let _ = std::io::Read::read(&mut stream, &mut ack);
            }
        }
        let _ = done.send(());
    });
    let _ = wait.recv_timeout(CLIENT_BUDGET);
}
