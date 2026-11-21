//! The hook socket (design section 7.3): the CLIs report turn events by running
//! `sylvode-runner hook ...`, which sends one JSON line to `state_dir/hook.sock`.
//!
//! The socket accepts exactly one message shape, `{"v":1,"cli","session","event","nonce","ts"}` with a known
//! CLI, a valid session key and an event that CLI can send, in at most [`MAX_LINE`] bytes. Anything
//! else is dropped with a `debug` log of its length only: hook input can hold prompt text, which
//! must not reach logs. The socket lives in the private state directory and is set to mode 0600.
//!
//! The `hook` subcommand ([`send`]) never reads stdin, ignores extra arguments, and gives up within
//! [`CLIENT_BUDGET`]: it must never block the CLI that runs it.

use std::io::Write;
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde_json::Value;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixListener;
use tokio::sync::mpsc;

use super::tmux::SessionKey;

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
    pub session: String,
    pub event: HookEvent,
    /// The per-launch secret of the session; the receiver compares it, in constant time, with the
    /// one it wrote into that launch's hook command.
    pub nonce: String,
    /// When the hook process sent it, in nanoseconds since the epoch: events of one session are
    /// applied in this order, and one older than the last applied is ignored.
    pub seq: u64,
    /// The sender and its parents, innermost first, read while the sender was still connected
    /// (where the platform names the peer): the session's pane process must be among them.
    pub peer_chain: Option<Vec<u32>>,
}

/// The process `pid` and its parents, innermost first (at most 64), read with `ps`. Ends at the
/// first process that cannot be read.
#[must_use]
pub fn ancestry(pid: u32) -> Vec<u32> {
    let mut chain = vec![pid];
    let mut current = pid;
    for _ in 0..64 {
        if current <= 1 {
            break;
        }
        let out = std::process::Command::new("/bin/ps")
            .args(["-o", "ppid=", "-p", &current.to_string()])
            .output();
        let Some(parent) = out
            .ok()
            .and_then(|o| String::from_utf8(o.stdout).ok())
            .and_then(|text| text.trim().parse::<u32>().ok())
        else {
            break;
        };
        if parent == current || chain.contains(&parent) {
            break;
        }
        chain.push(parent);
        current = parent;
    }
    chain
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

/// Compares two secrets without stopping at the first difference.
#[must_use]
pub fn ct_eq(a: &str, b: &str) -> bool {
    if a.len() != b.len() {
        return false;
    }
    a.bytes().zip(b.bytes()).fold(0u8, |diff, (x, y)| diff | (x ^ y)) == 0
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
    if object.len() != 6
        || object.get("v").and_then(Value::as_u64) != Some(1)
        || object.get("ts").and_then(Value::as_u64).is_none()
    {
        return Err(Reject::Shape);
    }
    let text = |key: &str| object.get(key).and_then(Value::as_str);
    let (Some(cli), Some(session), Some(event)) = (text("cli"), text("session"), text("event")) else {
        return Err(Reject::Shape);
    };
    let nonce = text("nonce").ok_or(Reject::Shape)?;
    if nonce.len() != 32 || !nonce.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err(Reject::Shape);
    }
    SessionKey::new(session).map_err(|_| Reject::Shape)?;
    let event = HookEvent::parse(cli, event).ok_or(Reject::Shape)?;
    Ok(HookMessage {
        cli: cli.to_string(),
        session: session.to_string(),
        event,
        nonce: nonce.to_string(),
        seq: object.get("ts").and_then(Value::as_u64).unwrap_or(0),
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
    let mut listener = bind(path)?;
    let mut bound = identity(path);
    let path = path.to_path_buf();
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
            let tx = tx.clone();
            tokio::spawn(async move {
                let mut stream = stream;
                let read = tokio::time::timeout(READ_TIMEOUT, read_line(&mut stream)).await;
                // Only the runner's own user may speak here, whatever the socket's mode says. The
                // peer is asked after its line is read: the client waits for the acknowledgement
                // below, so it is still connected (macOS cannot name a peer that has gone).
                let peer = stream.peer_cred().ok();
                let same_user = peer
                    .as_ref()
                    .is_some_and(|peer| peer.uid() == nix::unistd::Uid::effective().as_raw());
                let peer_pid = peer.and_then(|peer| peer.pid()).and_then(|pid| u32::try_from(pid).ok());
                // Who the sender is must be read while it is still connected, so the chain is made
                // before the acknowledgement lets it go.
                let peer_chain = match peer_pid {
                    Some(pid) => tokio::task::spawn_blocking(move || ancestry(pid)).await.ok(),
                    None => None,
                };
                let _ = stream.write_all(b"k").await;
                if !same_user {
                    tracing::debug!("hook line dropped: the peer is not this user");
                    return;
                }
                match read {
                    Ok(Some(line)) => match parse_line(&line) {
                        Ok(mut message) => {
                            message.peer_chain = peer_chain;
                            let _ = tx.send(message).await;
                        }
                        Err(reject) => tracing::debug!(length = line.len(), ?reject, "hook line dropped"),
                    },
                    Ok(None) => tracing::debug!("hook line dropped: too long or unreadable"),
                    Err(_) => tracing::debug!("hook connection dropped: no line in time"),
                }
            });
        }
    }))
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
/// panics, and returns within [`CLIENT_BUDGET`] even when the socket is absent or stuck.
pub fn send(socket: &Path, cli: &str, session: &str, event: &str, nonce: &str) {
    let line = serde_json::json!({
        "v": 1, "cli": cli, "session": session, "event": event, "nonce": nonce,
        "ts": std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| u64::try_from(d.as_nanos()).unwrap_or(u64::MAX)),
    })
    .to_string();
    let socket: PathBuf = socket.to_path_buf();
    let (done, wait) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        if let Ok(mut stream) = UnixStream::connect(&socket) {
            let _ = stream.set_write_timeout(Some(Duration::from_millis(500)));
            if stream.write_all(format!("{line}\n").as_bytes()).is_ok() {
                // Stay connected until the runner has looked at us (it checks who we are).
                let _ = stream.set_read_timeout(Some(Duration::from_millis(500)));
                let mut ack = [0u8; 1];
                let _ = std::io::Read::read(&mut stream, &mut ack);
            }
        }
        let _ = done.send(());
    });
    let _ = wait.recv_timeout(CLIENT_BUDGET);
}
