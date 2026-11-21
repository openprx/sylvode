//! The hook socket (design section 7.3): the CLIs report turn events by running
//! `sylvode-runner hook ...`, which sends one JSON line to `state_dir/hook.sock`.
//!
//! The socket accepts exactly one message shape, `{"v":1,"cli","session","event","ts"}` with a known
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
use tokio::io::{AsyncBufReadExt, AsyncReadExt, BufReader};
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
    if object.len() != 5
        || object.get("v").and_then(Value::as_u64) != Some(1)
        || !object.get("ts").is_some_and(Value::is_number)
    {
        return Err(Reject::Shape);
    }
    let text = |key: &str| object.get(key).and_then(Value::as_str);
    let (Some(cli), Some(session), Some(event)) = (text("cli"), text("session"), text("event")) else {
        return Err(Reject::Shape);
    };
    SessionKey::new(session).map_err(|_| Reject::Shape)?;
    let event = HookEvent::parse(cli, event).ok_or(Reject::Shape)?;
    Ok(HookMessage {
        cli: cli.to_string(),
        session: session.to_string(),
        event,
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
    if std::fs::symlink_metadata(path).is_ok() {
        std::fs::remove_file(path).map_err(|e| format!("could not remove the old hook socket: {}", e.kind()))?;
    }
    let listener = UnixListener::bind(path).map_err(|e| format!("could not bind the hook socket: {}", e.kind()))?;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))
        .map_err(|e| format!("could not restrict the hook socket: {}", e.kind()))?;
    Ok(tokio::spawn(async move {
        loop {
            let Ok((stream, _)) = listener.accept().await else {
                tokio::time::sleep(Duration::from_millis(100)).await;
                continue;
            };
            if tx.is_closed() {
                return;
            }
            let tx = tx.clone();
            tokio::spawn(async move {
                let read = tokio::time::timeout(READ_TIMEOUT, read_line(stream)).await;
                match read {
                    Ok(Some(line)) => match parse_line(&line) {
                        Ok(message) => {
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
async fn read_line(stream: tokio::net::UnixStream) -> Option<Vec<u8>> {
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
pub fn send(socket: &Path, cli: &str, session: &str, event: &str) {
    let line = serde_json::json!({
        "v": 1, "cli": cli, "session": session, "event": event,
        "ts": std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs()),
    })
    .to_string();
    let socket: PathBuf = socket.to_path_buf();
    let (done, wait) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        if let Ok(mut stream) = UnixStream::connect(&socket) {
            let _ = stream.set_write_timeout(Some(Duration::from_millis(500)));
            let _ = stream.write_all(format!("{line}\n").as_bytes());
        }
        let _ = done.send(());
    });
    let _ = wait.recv_timeout(CLIENT_BUDGET);
}
