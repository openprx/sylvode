//! The tmux driver: every interaction with tmux as an argument vector.
//!
//! # Safety rules
//!
//! - **No shell.** Every call is `std::process::Command` with an explicit argument vector. Nothing
//!   is ever interpolated into a command line.
//! - **The operator's server.** Every call carries `-L <socket>` when a socket name is configured;
//!   with none it addresses the operator's default server. The runner passes no `-f`: a server it
//!   starts loads the operator's normal `tmux.conf`, like any server the operator starts. It never
//!   sets an option, restarts, respawns, renames, resizes or ends a session, window or pane: the
//!   only calls that exist are `-V`, `has-session`, `new-session`, `list-panes`, `display-message`,
//!   `capture-pane`, `list-clients`, `load-buffer`, `paste-buffer`, `delete-buffer` and
//!   `send-keys ... Enter` (a unit test pins the set).
//! - **Sessions by exact name.** A session is named by a [`TmuxName`] (derived `<KEY>-<cli>`, or an
//!   existing session the operator configured) and is targeted as `=<name>` (exact match:
//!   `AB-claude` never reaches `ABC-claude`). The pattern has no `:`, `.`, `=` or `%`. The slot id
//!   `<KEY>-<cli>` is a [`SessionName`]; it is not a tmux target.
//! - **Panes by id and PID.** A [`Pane`] holds the pane's id (`%N`, unique for the life of the
//!   server) and the PID of its process, as the runner recorded them. Every call that types checks
//!   that the pane still has that id, still belongs to the session and still has that PID, so a
//!   moved pane, a renamed session or a server restarted with reused ids is refused.
//! - **Format strings.** tmux expands several arguments as formats. Checked on tmux 3.7b with an
//!   isolated server (`tests/tmux_integration.rs`): `-c DIR` of `new-session` IS expanded
//!   (`#{session_name}` in a directory name was replaced, and a `#(...)` part was consumed; on
//!   3.7b the command inside `#(...)` did not run, but 3.2 to 3.6 were not available to test, and
//!   the documented behaviour of `#(...)` is to run `/bin/sh`). A [`WorkDir`] therefore refuses `#`
//!   altogether. The elements of the program argument vector after `--` are NOT expanded
//!   (verified). The other arguments are constants (`-n cli`, `-F`, the `display-message`
//!   formats) or validated types (`-s NAME`, `-t`).
//! - **Text only through stdin and a buffer, only after a gate.** [`Tmux::push_message`] loads the
//!   text with `load-buffer -b sv-<task uuid> -` on standard input and pastes it with
//!   `paste-buffer -p -d -b sv-<task uuid> -t %N`. No argument contains message text.
//!   [`PushText`] refuses control characters (other than line feed), format, private-use and
//!   unassigned characters, line and paragraph separators, and anything over
//!   [`MAX_PUSH_BYTES`].
//! - **Bracketed paste is not assumed.** `paste-buffer -p` wraps the text in bracketed-paste
//!   markers only when the pane's program has asked for them; otherwise every line feed becomes a
//!   carriage return (Enter) and each line is submitted on its own (verified on 3.7b with `cat`).
//!   `#{bracket_paste_flag}` reports the state, but it exists only since tmux 3.7; older servers
//!   print an empty string. The gate therefore requires, before any text is loaded: the pane is
//!   alive, in no mode (copy mode would swallow Enter), still the recorded pane of the session, and
//!   its foreground command is one of the CLI's process names the caller passes. Then, on a server
//!   that reports the flag, the flag must be `1`. On a server that does not (3.2 to 3.6), tmux
//!   cannot tell, so the caller must hand over a [`BracketedPasteConfirmed`], which only the
//!   session manager's readiness check may create. Without it the push is refused.
//! - **One key.** The only `send-keys` is `send-keys -t %N Enter`. No `C-c`, ever.
//! - **No shell for the CLI either.** `new-session` passes the program argument vector after
//!   `--`. tmux runs a single argument through `/bin/sh -c` (verified), so a one-element vector is
//!   run as `/usr/bin/env -- <program>`, which tmux executes directly.
//! - **Serialisation and partial failure.** Pushes and Enter retries for one session are
//!   serialised inside the driver. A paste that times out may or may not have been delivered, so
//!   it is reported as [`TmuxError::DeliveryUnknown`] and is never retried here; the buffer is
//!   deleted on every failure path.
//! - **Environment.** `new-session`, which may start the server, gives it the runner's own
//!   environment minus the agent variables `CLAUDECODE`, `CLAUDE_CODE_ENTRYPOINT` and `TMUX`
//!   ([`agent_environment`]), so the CLI in a session the runner creates has the operator's
//!   environment and login. Every other tmux call gets only an allow-list ([`ENV_ALLOWED`]). Under
//!   systemd (`INVOCATION_ID` set) a server that is not yet running is started through
//!   `systemd-run --user --scope --unit=sylvode-tmux-<socket>` so it outlives the runner service.
//! - **Remaining trust assumption.** Everything on one server runs as one OS user. tmux sets `TMUX`
//!   inside each pane, so an agent that may run shell commands can drive every other session on
//!   the same server (`tmux send-keys -t ...`). Nothing in-process prevents that.
//! - **Only typed operations are public.** The call builders and the process runner are private.
//!
//! # Version floor
//!
//! The minimum tmux is 3.2. Checked against tmux's `CHANGES` (3.7b): the argument-vector form of
//! `new-session` exists since 2.0, `=` exact targets since 2.1, `new-session -P -F` and pane ids
//! long before 2.0. Nothing the driver relies on is newer than 3.2 except
//! `#{bracket_paste_flag}` (3.7), which is optional as described above. Since 3.4 `paste-buffer`
//! refuses a dead pane, which is reported as an ordinary failure.

// The driver is also exposed (feature `test-support`) to its integration test, which makes its
// items reachable; the long first paragraphs of their docs are deliberate.
#![allow(clippy::too_long_first_doc_paragraph)]

use std::collections::HashMap;
use std::ffi::OsString;
use std::io::{Read, Write};
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::{Arc, OnceLock};
use std::thread;
use std::time::{Duration, Instant};

use parking_lot::Mutex;
use unicode_properties::{GeneralCategory, UnicodeGeneralCategory};
use uuid::Uuid;

/// How long one tmux call may take.
pub const CALL_TIMEOUT: Duration = Duration::from_secs(10);

/// The most bytes a pushed message may have: the task template is about 1.1 KiB with a 120
/// character title, so this leaves a wide margin and still stops a huge paste.
pub const MAX_PUSH_BYTES: usize = 4096;

/// Environment variables tmux processes receive (and `LC_*`); everything else is dropped.
pub const ENV_ALLOWED: [&str; 10] = [
    "PATH",
    "HOME",
    "USER",
    "LOGNAME",
    "LANG",
    "LANGUAGE",
    "TERM",
    "TZ",
    "SHELL",
    "TMUX_TMPDIR",
];

/// The variables a tmux call of the runner's gets from the runner's own environment: the
/// allow-list of [`ENV_ALLOWED`] and `LC_*`, and for the `systemd-run --user` wrapper the two
/// variables it needs. The agent variables are never passed.
pub fn allowed_environment(for_systemd_run: bool) -> Vec<(OsString, OsString)> {
    std::env::vars_os()
        .filter(|(name, _)| {
            let name = name.to_string_lossy();
            let allowed = ENV_ALLOWED.contains(&&*name)
                || name.starts_with("LC_")
                || (for_systemd_run && ENV_ALLOWED_SYSTEMD.contains(&&*name));
            allowed && !ENV_REMOVED.contains(&&*name)
        })
        .collect()
}

/// The environment of a CLI the runner starts (a headless run, or the tmux server that a
/// `new-session` starts and so the panes of a session it creates): the runner's own environment
/// minus the agent variables, so the CLI has the operator's variables and login.
pub fn agent_environment() -> Vec<(OsString, OsString)> {
    agent_environment_from(std::env::vars_os())
}

/// [`agent_environment`] over any list of variables.
pub fn agent_environment_from(vars: impl IntoIterator<Item = (OsString, OsString)>) -> Vec<(OsString, OsString)> {
    vars.into_iter()
        .filter(|(name, _)| !ENV_REMOVED.contains(&&*name.to_string_lossy()))
        .collect()
}

/// Needed only by the `systemd-run --user` wrapper.
const ENV_ALLOWED_SYSTEMD: [&str; 2] = ["XDG_RUNTIME_DIR", "DBUS_SESSION_BUS_ADDRESS"];

const ENV_REMOVED: [&str; 3] = ["CLAUDECODE", "CLAUDE_CODE_ENTRYPOINT", "TMUX"];

/// The name of the window of a session the runner creates.
pub const WINDOW_NAME: &str = "cli";

/// Why a push was refused before any text was loaded.
#[derive(Debug, PartialEq, Eq)]
pub enum Refusal {
    /// The pane is no longer the recorded pane of the session (moved, the session renamed, or the
    /// pane's process is another one).
    NotOwned,
    /// The pane's program has ended.
    PaneDead,
    /// The pane is in copy mode or another mode, which would take the Enter.
    PaneInMode,
    /// The foreground command is not one of the CLI's process names.
    CommandNotAllowed,
    /// The program has not enabled bracketed paste (`#{bracket_paste_flag}` is 0).
    BracketedPasteOff,
    /// This tmux cannot report bracketed paste (before 3.7) and no confirmation was given.
    BracketedPasteUnconfirmed,
}

/// Why a tmux call did not work.
#[derive(Debug, PartialEq, Eq)]
pub enum TmuxError {
    /// An argument was refused before anything was run.
    Invalid(String),
    /// tmux could not be started.
    Spawn(String),
    /// tmux did not finish in time and was killed.
    Timeout,
    /// tmux exited with a failure status.
    Failed { code: Option<i32>, stderr: String },
    /// The push gate refused; nothing was loaded or pasted.
    Refused(Refusal),
    /// The paste timed out: the text may or may not be in the input box. Do not paste again.
    DeliveryUnknown,
    /// The text was pasted but Enter was not sent; only Enter may be retried ([`Tmux::retry_enter`]).
    PastedNotSubmitted,
}

impl std::fmt::Display for TmuxError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Invalid(why) => write!(f, "refused tmux argument: {why}"),
            Self::Spawn(why) => write!(f, "could not run tmux: {why}"),
            Self::Timeout => write!(f, "tmux did not answer in time"),
            Self::Failed { code, stderr } => write!(f, "tmux failed (status {code:?}): {stderr}"),
            Self::Refused(why) => write!(f, "push refused: {why:?}"),
            Self::DeliveryUnknown => write!(f, "delivery unknown: the paste timed out"),
            Self::PastedNotSubmitted => write!(f, "the text was pasted but Enter was not sent"),
        }
    }
}

impl std::error::Error for TmuxError {}

/// A tmux socket name: `^[A-Za-z0-9_-]{1,32}$`, or empty for the user's default server.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SocketName(String);

impl SocketName {
    /// # Errors
    ///
    /// [`TmuxError::Invalid`] for any other value.
    pub fn new(name: &str) -> Result<Self, TmuxError> {
        let ok = name.is_empty()
            || (name.len() <= 32 && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-'));
        if ok {
            Ok(Self(name.to_string()))
        } else {
            Err(TmuxError::Invalid(
                "socket name must match ^[A-Za-z0-9_-]{1,32}$ or be empty".to_string(),
            ))
        }
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// The CLI types a session can be for.
pub const SESSION_CLIS: [&str; 3] = ["claude", "codex", "opencode"];

/// A session name, `<KEY>-<cli>`: the project key (`^[A-Z0-9]{1,32}$`), one hyphen and the CLI type
/// in lower case (`claude`, `codex` or `opencode`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SessionName {
    text: String,
    key_len: usize,
}

impl SessionName {
    /// The session of project `key` for `cli`.
    ///
    /// # Errors
    ///
    /// [`TmuxError::Invalid`] for a key or CLI type that does not fit the pattern.
    pub fn new(key: &str, cli: &str) -> Result<Self, TmuxError> {
        if key.is_empty() || key.len() > 32 || !key.chars().all(|c| c.is_ascii_uppercase() || c.is_ascii_digit()) {
            return Err(TmuxError::Invalid(
                "session key must match ^[A-Z0-9]{1,32}$".to_string(),
            ));
        }
        if !SESSION_CLIS.contains(&cli) {
            return Err(TmuxError::Invalid(
                "session CLI must be claude, codex or opencode".to_string(),
            ));
        }
        Ok(Self {
            text: format!("{key}-{cli}"),
            key_len: key.len(),
        })
    }

    /// A name read back, for example from a hook message or a directory name.
    ///
    /// # Errors
    ///
    /// [`TmuxError::Invalid`] unless it matches `^[A-Z0-9]{1,32}-(claude|codex|opencode)$`.
    pub fn parse(name: &str) -> Result<Self, TmuxError> {
        let (key, cli) = name
            .split_once('-')
            .ok_or_else(|| TmuxError::Invalid("session name must look like KEY-cli".to_string()))?;
        Self::new(key, cli)
    }

    pub fn as_str(&self) -> &str {
        &self.text
    }

    /// The project key part.
    pub fn key(&self) -> &str {
        self.text.get(..self.key_len).unwrap_or_default()
    }

    /// The CLI type part.
    pub fn cli(&self) -> &str {
        self.text.get(self.key_len + 1..).unwrap_or_default()
    }

    /// The exact session target, `=<name>`.
    pub fn session_target(&self) -> String {
        format!("={}", self.text)
    }
}

/// Where the tmux name of a session came from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NameSource {
    /// `<KEY>-<cli>`, derived from the slot; the only kind the runner creates.
    Derived,
    /// Named by the runner's own configuration (`[[runner.tmux.sessions]]`).
    Local,
    /// Named by the server (not read in this stage).
    Server,
}

impl NameSource {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Derived => "derived",
            Self::Local => "local",
            Self::Server => "server",
        }
    }

    /// Reads a recorded value; empty or unknown is `Derived` (a record written before the field existed).
    pub fn from_record(text: &str) -> Self {
        match text {
            "local" => Self::Local,
            "server" => Self::Server,
            _ => Self::Derived,
        }
    }
}

/// The name of a tmux session as tmux knows it: `^[A-Za-z0-9_][A-Za-z0-9_-]{0,63}$`. A name that
/// is configured (not derived) must not look like a derived name of any project, compared case
/// insensitively; [`TmuxName::derived`] is the only way to make one that does.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TmuxName(String);

impl TmuxName {
    /// A configured name.
    ///
    /// # Errors
    ///
    /// [`TmuxError::Invalid`] for a name outside the pattern or one shaped like `<KEY>-<cli>`.
    pub fn new(name: &str) -> Result<Self, TmuxError> {
        let mut chars = name.chars();
        let first_ok = chars.next().is_some_and(|c| c.is_ascii_alphanumeric() || c == '_');
        let ok = first_ok && name.len() <= 64 && chars.all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-');
        if !ok {
            return Err(TmuxError::Invalid(
                "tmux session name must match ^[A-Za-z0-9_][A-Za-z0-9_-]{0,63}$".to_string(),
            ));
        }
        if looks_derived(name) {
            return Err(TmuxError::Invalid(
                "tmux session name must not look like a derived name (<KEY>-claude, <KEY>-codex or <KEY>-opencode)"
                    .to_string(),
            ));
        }
        Ok(Self(name.to_string()))
    }

    /// The derived name of a slot: the same text as the slot id.
    pub fn derived(slot: &SessionName) -> Self {
        Self(slot.as_str().to_string())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// The exact session target, `=<name>`.
    pub fn session_target(&self) -> String {
        format!("={}", self.0)
    }
}

/// Whether `name` has the shape `^[A-Z0-9]{1,32}-(claude|codex|opencode)$`, ignoring case.
fn looks_derived(name: &str) -> bool {
    let upper = name.to_ascii_uppercase();
    let lower = name.to_ascii_lowercase();
    SESSION_CLIS.iter().any(|cli| {
        lower
            .strip_suffix(cli)
            .and_then(|head| head.strip_suffix('-'))
            .is_some_and(|key| {
                (1..=32).contains(&key.len())
                    && upper
                        .get(..key.len())
                        .is_some_and(|k| k.chars().all(|c| c.is_ascii_uppercase() || c.is_ascii_digit()))
            })
    })
}

/// A pane id, `%N`: unique for the life of the tmux server and unaffected by window moves.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PaneId(String);

impl PaneId {
    /// # Errors
    ///
    /// [`TmuxError::Invalid`] unless the value is `%` and 1 to 9 digits.
    pub fn new(id: &str) -> Result<Self, TmuxError> {
        let digits = id.strip_prefix('%').unwrap_or("");
        if (1..=9).contains(&digits.len()) && digits.chars().all(|c| c.is_ascii_digit()) {
            Ok(Self(id.to_string()))
        } else {
            Err(TmuxError::Invalid("pane id must match ^%[0-9]{1,9}$".to_string()))
        }
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// The CLI pane of a session: its id and the PID of its process, as the runner recorded them and
/// uses them ever after.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Pane {
    pub name: TmuxName,
    pub id: PaneId,
    pub pid: u32,
}

/// A session [`Tmux::new_session`] created.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Created {
    pub pane: Pane,
    /// The socket path of the server, asked in the same call so the pane can be recorded for hooks
    /// at once; `None` when this tmux does not say.
    pub socket_path: Option<String>,
}

/// One pane of a session, from [`Tmux::list_panes`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ListedPane {
    pub id: PaneId,
    pub pid: u32,
    pub dead: bool,
    /// The foreground command.
    pub command: String,
}

/// The format of [`Tmux::list_panes`]; the command comes last and may contain spaces.
pub const LIST_PANES_FORMAT: &str = "#{pane_id} #{pane_pid} #{pane_dead} #{pane_current_command}";

/// Parses one line of the answer to [`LIST_PANES_FORMAT`].
pub fn parse_listed_pane(line: &str) -> Option<ListedPane> {
    let mut parts = line.splitn(4, ' ');
    let id = PaneId::new(parts.next()?).ok()?;
    let pid = parts.next()?.parse().ok()?;
    let dead = match parts.next()? {
        "0" => false,
        "1" => true,
        _ => return None,
    };
    let command = parts.next()?.to_string();
    Some(ListedPane { id, pid, dead, command })
}

/// A working directory tmux may be given: absolute, UTF-8, without control characters and
/// without `#`. tmux reads `-c` as a format, where `#{...}` substitutes and `#(...)` is documented
/// to run a shell command, so a directory such as `.../#(cmd)` must never reach it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WorkDir(String);

impl WorkDir {
    /// # Errors
    ///
    /// [`TmuxError::Invalid`] with the reason.
    pub fn new(dir: &Path) -> Result<Self, TmuxError> {
        if !dir.is_absolute() {
            return Err(TmuxError::Invalid("working directory must be absolute".to_string()));
        }
        let text = dir
            .to_str()
            .ok_or_else(|| TmuxError::Invalid("working directory must be UTF-8".to_string()))?;
        if text.contains('#') {
            return Err(TmuxError::Invalid(
                "working directory must not contain '#': tmux would read it as a format".to_string(),
            ));
        }
        if text.chars().any(char::is_control) {
            return Err(TmuxError::Invalid(
                "working directory must not contain control characters".to_string(),
            ));
        }
        Ok(Self(text.to_string()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// A paste buffer name, `sv-<task uuid>`: one buffer per task.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BufferName(String);

impl BufferName {
    pub fn for_task(task_id: &Uuid) -> Self {
        Self(format!("sv-{}", task_id.hyphenated()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Whether `c` may not be pasted: controls (other than the line feed handled by the caller),
/// format characters (zero-width, directional, tag), private-use, unassigned, surrogate, and the
/// line and paragraph separators.
fn is_unsafe_to_paste(c: char) -> bool {
    matches!(
        c.general_category(),
        GeneralCategory::Control
            | GeneralCategory::Format
            | GeneralCategory::Surrogate
            | GeneralCategory::PrivateUse
            | GeneralCategory::Unassigned
            | GeneralCategory::LineSeparator
            | GeneralCategory::ParagraphSeparator
    )
}

/// Text that may be pasted: non-empty, at most [`MAX_PUSH_BYTES`], and no character that is
/// unsafe to paste except line feed. That excludes ESC and every other C0 and C1 control and DEL
/// (so nothing can act as a terminal sequence or end the bracketed paste early), U+2028 and
/// U+2029, bidi and other format characters.
#[derive(Debug, PartialEq, Eq)]
pub struct PushText<'a>(&'a str);

impl<'a> PushText<'a> {
    /// # Errors
    ///
    /// [`TmuxError::Invalid`] naming the kind of problem, never the text.
    pub fn new(text: &'a str) -> Result<Self, TmuxError> {
        if text.is_empty() {
            return Err(TmuxError::Invalid("empty text".to_string()));
        }
        if text.len() > MAX_PUSH_BYTES {
            return Err(TmuxError::Invalid("text is longer than the push limit".to_string()));
        }
        if text.chars().any(|c| c != '\n' && is_unsafe_to_paste(c)) {
            return Err(TmuxError::Invalid(
                "text contains a control, format, separator or unassigned character".to_string(),
            ));
        }
        Ok(Self(text))
    }
}

/// Proof that the pane's program has bracketed paste on, for tmux servers that cannot report it
/// (before 3.7).
///
/// Only the session manager's readiness check may create one, by
/// [`BracketedPasteConfirmed::from_readiness_check`], and only when all of these hold at that
/// moment: the CLI's foreground process is running, its readiness signal arrived (claude: the
/// `SessionStart` hook; codex: the prompt glyph on a stable screen), and the CLI is one that
/// enables bracketed paste once its input box is up (claude and codex do). The confirmation is
/// per push: a session manager that remembers one across a CLI restart defeats the gate.
#[derive(Clone, Copy, Debug)]
pub struct BracketedPasteConfirmed(());

impl BracketedPasteConfirmed {
    pub const fn from_readiness_check() -> Self {
        Self(())
    }
}

/// What a push needs to know about the pane's program.
#[derive(Clone, Copy, Debug)]
pub struct PasteGuard<'a> {
    /// The CLI's process names (`pane_current_command` must equal one of them).
    pub allowed_commands: &'a [&'a str],
    /// Required on tmux servers that cannot report `#{bracket_paste_flag}`.
    pub bracketed_confirmed: Option<BracketedPasteConfirmed>,
}

/// State of a pane, from one `display-message`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PaneState {
    pub dead: bool,
    /// Number of modes the pane is in (copy mode and so on).
    pub in_mode: u32,
    /// `None` when this tmux does not know the variable (before 3.7).
    pub bracket_paste: Option<bool>,
    pub pid: u32,
    pub session_name: String,
}

/// The format of [`Tmux::pane_state`]. All fields but the last cannot contain a space.
pub const STATE_FORMAT: &str = "#{pane_dead} #{pane_in_mode} #{bracket_paste_flag} #{pane_pid} #{session_name}";

/// Parses the answer to [`STATE_FORMAT`]; the session name comes last and may contain spaces.
pub fn parse_pane_state(line: &str) -> Option<PaneState> {
    let mut parts = line.splitn(5, ' ');
    let dead = match parts.next()? {
        "0" => false,
        "1" => true,
        _ => return None,
    };
    let in_mode = parts.next()?.parse().ok()?;
    let bracket_paste = match parts.next()? {
        "" => None,
        "0" => Some(false),
        "1" => Some(true),
        _ => return None,
    };
    let pid = parts.next()?.parse().ok()?;
    let session_name = parts.next()?.to_string();
    Some(PaneState {
        dead,
        in_mode,
        bracket_paste,
        pid,
        session_name,
    })
}

/// What the runner reads about the pane, each field from its own call so no value can shift into
/// another.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PaneInfo {
    pub dead: bool,
    pub pid: u32,
    pub current_command: String,
    pub current_path: String,
}

/// One tmux call: its argument vector (after the program name) and what goes to stdin.
#[derive(Debug, PartialEq, Eq)]
struct Call {
    args: Vec<String>,
    stdin: Option<Vec<u8>>,
}

/// A finished tmux call.
#[derive(Debug)]
struct Output {
    code: Option<i32>,
    stdout: String,
    stderr: String,
}

/// The single line a `display-message -p` answered: exactly one trailing line feed and no other
/// line break. A value that contains a line break gives `None` instead of a shifted field.
fn one_line(stdout: &str) -> Option<&str> {
    let line = stdout.strip_suffix('\n')?;
    (!line.contains(['\n', '\r'])).then_some(line)
}

/// The argument vector tmux is given for the CLI: a one-element vector would be run through
/// `/bin/sh -c`, so it is run as `/usr/bin/env -- <program>` instead.
fn exec_argv(argv: &[String]) -> Result<Vec<String>, TmuxError> {
    if argv.is_empty() || argv.iter().any(String::is_empty) {
        return Err(TmuxError::Invalid(
            "the command needs a program and no empty argument".to_string(),
        ));
    }
    if argv.len() == 1 {
        let mut wrapped = vec!["/usr/bin/env".to_string(), "--".to_string()];
        wrapped.extend(argv.iter().cloned());
        return Ok(wrapped);
    }
    Ok(argv.to_vec())
}

fn session_locks() -> &'static Mutex<HashMap<String, Arc<Mutex<()>>>> {
    static LOCKS: OnceLock<Mutex<HashMap<String, Arc<Mutex<()>>>>> = OnceLock::new();
    LOCKS.get_or_init(|| Mutex::new(HashMap::new()))
}

/// The tmux driver for one server.
#[derive(Clone, Debug)]
pub struct Tmux {
    program: OsString,
    systemd_run: OsString,
    socket: SocketName,
    extra_env: Vec<(OsString, OsString)>,
    under_systemd: bool,
    timeout: Duration,
}

impl Tmux {
    /// A driver that runs `tmux` from the `PATH` against the server on `socket` (the operator's
    /// default server when the name is empty).
    pub fn new(socket: SocketName) -> Self {
        Self {
            program: OsString::from("tmux"),
            systemd_run: OsString::from("systemd-run"),
            socket,
            extra_env: Vec::new(),
            under_systemd: std::env::var_os("INVOCATION_ID").is_some(),
            timeout: CALL_TIMEOUT,
        }
    }

    /// Runs this program instead of `tmux` from the `PATH`.
    #[must_use]
    pub fn with_program(mut self, program: impl Into<OsString>) -> Self {
        self.program = program.into();
        self
    }

    /// Runs this program instead of `systemd-run` when a server is started in its own scope.
    #[must_use]
    pub fn with_systemd_run(mut self, program: impl Into<OsString>) -> Self {
        self.systemd_run = program.into();
        self
    }

    /// Sets one more environment variable on every tmux process (`TMUX_TMPDIR`, say). The agent
    /// variables stay removed.
    #[must_use]
    pub fn with_env(mut self, key: impl Into<OsString>, value: impl Into<OsString>) -> Self {
        self.extra_env.push((key.into(), value.into()));
        self
    }

    /// Overrides the detection of a systemd service (`INVOCATION_ID`).
    #[must_use]
    pub const fn with_systemd(mut self, under_systemd: bool) -> Self {
        self.under_systemd = under_systemd;
        self
    }

    /// Kills a call after `timeout` instead of [`CALL_TIMEOUT`].
    #[must_use]
    pub const fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }

    /// The global options every call starts with: `-L <socket>` when a socket is configured.
    fn global(&self) -> Vec<String> {
        let mut args = Vec::new();
        if !self.socket.as_str().is_empty() {
            args.push("-L".to_string());
            args.push(self.socket.as_str().to_string());
        }
        args
    }

    fn call(&self, rest: &[&str]) -> Call {
        let mut args = self.global();
        args.extend(rest.iter().map(|arg| (*arg).to_string()));
        Call { args, stdin: None }
    }

    fn version_call() -> Call {
        Call {
            args: vec!["-V".to_string()],
            stdin: None,
        }
    }

    fn has_session_call(&self, name: &TmuxName) -> Call {
        self.call(&["has-session", "-t", &name.session_target()])
    }

    /// Every pane of every session of the server: only the exit status is used, to tell a running
    /// server from none.
    fn server_probe_call(&self) -> Call {
        self.call(&["list-panes", "-a", "-F", "#{pane_id}"])
    }

    fn list_panes_call(&self, name: &TmuxName) -> Call {
        self.call(&[
            "list-panes",
            "-s",
            "-t",
            &name.session_target(),
            "-F",
            LIST_PANES_FORMAT,
        ])
    }

    fn new_session_call(
        &self,
        name: &TmuxName,
        dir: &WorkDir,
        width: u16,
        height: u16,
        argv: &[String],
    ) -> Result<Call, TmuxError> {
        let argv = exec_argv(argv)?;
        let (width, height) = (width.to_string(), height.to_string());
        let mut call = self.call(&[
            "new-session",
            "-d",
            "-P",
            "-F",
            "#{pane_id} #{pane_pid} #{socket_path}",
            "-s",
            name.as_str(),
            "-n",
            WINDOW_NAME,
            "-x",
            &width,
            "-y",
            &height,
            "-c",
            dir.as_str(),
            "--",
        ]);
        call.args.extend(argv);
        Ok(call)
    }

    fn state_call(&self, pane: &PaneId) -> Call {
        self.call(&["display-message", "-p", "-t", pane.as_str(), STATE_FORMAT])
    }

    fn field_call(&self, pane: &PaneId, format: &str) -> Call {
        self.call(&["display-message", "-p", "-t", pane.as_str(), format])
    }

    fn capture_pane_call(&self, pane: &PaneId, scrollback_lines: Option<u32>, styled: bool) -> Call {
        let start = scrollback_lines.map(|lines| format!("-{lines}"));
        let mut rest = vec!["capture-pane", "-p"];
        if styled {
            rest.push("-e");
        }
        rest.extend(["-t", pane.as_str()]);
        if let Some(start) = &start {
            rest.push("-S");
            rest.push(start);
        }
        self.call(&rest)
    }

    fn list_clients_call(&self, name: &TmuxName) -> Call {
        self.call(&["list-clients", "-F", "#{client_activity}", "-t", &name.session_target()])
    }

    fn load_buffer_call(&self, buffer: &BufferName, text: &PushText<'_>) -> Call {
        let mut call = self.call(&["load-buffer", "-b", buffer.as_str(), "-"]);
        call.stdin = Some(text.0.as_bytes().to_vec());
        call
    }

    fn paste_buffer_call(&self, buffer: &BufferName, pane: &PaneId) -> Call {
        self.call(&["paste-buffer", "-p", "-d", "-b", buffer.as_str(), "-t", pane.as_str()])
    }

    fn delete_buffer_call(&self, buffer: &BufferName) -> Call {
        self.call(&["delete-buffer", "-b", buffer.as_str()])
    }

    fn send_enter_call(&self, pane: &PaneId) -> Call {
        self.call(&["send-keys", "-t", pane.as_str(), "Enter"])
    }

    /// A tmux process. `inherit_environment` gives it the runner's environment minus the agent
    /// variables (only `new-session`, which may start the server the CLI inherits from); otherwise
    /// it gets the allow-list.
    fn command(&self, wrap_in_scope: bool, inherit_environment: bool) -> Command {
        let mut command = if wrap_in_scope {
            let unit = if self.socket.as_str().is_empty() {
                "sylvode-tmux-default".to_string()
            } else {
                format!("sylvode-tmux-{}", self.socket.as_str())
            };
            let mut command = Command::new(&self.systemd_run);
            command
                .args(["--user", "--scope", &format!("--unit={unit}")])
                .arg(&self.program);
            command
        } else {
            Command::new(&self.program)
        };
        command.env_clear();
        let environment = if inherit_environment {
            agent_environment()
        } else {
            allowed_environment(wrap_in_scope)
        };
        for (name, value) in environment {
            command.env(name, value);
        }
        for (key, value) in &self.extra_env {
            command.env(key, value);
        }
        // Removal wins over anything set above.
        for name in ENV_REMOVED {
            command.env_remove(name);
        }
        command
    }

    fn run(&self, call: &Call) -> Result<Output, TmuxError> {
        self.run_command(self.command(false, false), call)
    }

    fn run_command(&self, mut command: Command, call: &Call) -> Result<Output, TmuxError> {
        command
            .args(&call.args)
            .stdin(if call.stdin.is_some() {
                Stdio::piped()
            } else {
                Stdio::null()
            })
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        // A program file that was written a moment ago can be "busy" (ETXTBSY) while another
        // thread's forked child still holds the write descriptor before its own exec; that ends in
        // milliseconds, so the spawn is tried again a few times. Any other error is final.
        let mut child = {
            let mut attempts = 0;
            loop {
                match command.spawn() {
                    Ok(child) => break child,
                    Err(e) if e.kind() == std::io::ErrorKind::ExecutableFileBusy && attempts < 50 => {
                        attempts += 1;
                        thread::sleep(Duration::from_millis(20));
                    }
                    Err(e) => return Err(TmuxError::Spawn(e.kind().to_string())),
                }
            }
        };
        let writer = child.stdin.take().zip(call.stdin.clone()).map(|(mut stdin, data)| {
            thread::spawn(move || {
                let _ = stdin.write_all(&data);
            })
        });
        let stdout = child.stdout.take().map(read_all);
        let stderr = child.stderr.take().map(read_all);
        let deadline = Instant::now() + self.timeout;
        let status = loop {
            match child.try_wait() {
                Ok(Some(status)) => break Some(status),
                Ok(None) if Instant::now() < deadline => thread::sleep(Duration::from_millis(5)),
                Ok(None) | Err(_) => break None,
            }
        };
        let Some(status) = status else {
            let _ = child.kill();
            let _ = child.wait();
            return Err(TmuxError::Timeout);
        };
        if let Some(writer) = writer {
            let _ = writer.join();
        }
        let join = |handle: Option<thread::JoinHandle<String>>| handle.and_then(|h| h.join().ok()).unwrap_or_default();
        Ok(Output {
            code: status.code(),
            stdout: join(stdout),
            stderr: join(stderr),
        })
    }

    fn run_ok(&self, call: &Call) -> Result<Output, TmuxError> {
        let output = self.run(call)?;
        if output.code == Some(0) {
            Ok(output)
        } else {
            Err(failed(&output))
        }
    }

    fn lock_for(&self, name: &TmuxName) -> Arc<Mutex<()>> {
        let name = format!("{}\0{}", self.socket.as_str(), name.as_str());
        Arc::clone(session_locks().lock().entry(name).or_default())
    }

    /// The text of `tmux -V`.
    ///
    /// # Errors
    ///
    /// See [`Tmux::run`]; also a failure status.
    pub fn version(&self) -> Result<String, TmuxError> {
        Ok(self.run_ok(&Self::version_call())?.stdout.trim().to_string())
    }

    /// Whether session `name` exists. Exit status 1 (no such session, or no server) is `false`.
    ///
    /// # Errors
    ///
    /// Any other failure.
    pub fn has_session(&self, name: &TmuxName) -> Result<bool, TmuxError> {
        let output = self.run(&self.has_session_call(name))?;
        match output.code {
            Some(0) => Ok(true),
            Some(1) => Ok(false),
            _ => Err(failed(&output)),
        }
    }

    /// Whether a server is running (on the socket, or the default one).
    ///
    /// # Errors
    ///
    /// Spawn failures and timeouts.
    pub fn server_running(&self) -> Result<bool, TmuxError> {
        Ok(self.run(&self.server_probe_call())?.code == Some(0))
    }

    /// Creates session `name` with one window running `argv` and returns its pane and the server's
    /// socket path (the caller records them). Sets no option. The server, if this starts it, and so the CLI, gets the
    /// runner's environment minus the agent variables. When the server is not yet running and the
    /// process is a systemd service, the server is started in its own scope.
    ///
    /// # Errors
    ///
    /// Invalid arguments and tmux failures.
    pub fn new_session(
        &self,
        name: &TmuxName,
        dir: &WorkDir,
        width: u16,
        height: u16,
        argv: &[String],
    ) -> Result<Created, TmuxError> {
        let call = self.new_session_call(name, dir, width, height, argv)?;
        let started_server = !self.server_running()?;
        let scope = self.under_systemd && started_server;
        let output = self.run_command(self.command(scope, true), &call)?;
        if output.code != Some(0) {
            return Err(failed(&output));
        }
        let line = one_line(&output.stdout).ok_or_else(|| unreadable("pane id"))?;
        // The socket path comes last: it may hold a space.
        let mut parts = line.splitn(3, ' ');
        let (Some(id), Some(pid), Some(socket)) = (parts.next(), parts.next(), parts.next()) else {
            return Err(unreadable("pane id"));
        };
        Ok(Created {
            pane: Pane {
                name: name.clone(),
                id: PaneId::new(id)?,
                pid: pid.parse().map_err(|_| unreadable("pane pid"))?,
            },
            socket_path: socket.starts_with('/').then(|| socket.to_string()),
        })
    }

    /// Every pane of the session (all windows), in tmux's order.
    ///
    /// # Errors
    ///
    /// A failure status (no such session) or an unreadable answer.
    pub fn list_panes(&self, name: &TmuxName) -> Result<Vec<ListedPane>, TmuxError> {
        let output = self.run_ok(&self.list_panes_call(name))?;
        output
            .stdout
            .lines()
            .map(|line| parse_listed_pane(line).ok_or_else(|| unreadable("pane list")))
            .collect()
    }

    /// State of the pane, from one call.
    ///
    /// # Errors
    ///
    /// A failure status (no such pane) or an answer that does not parse.
    pub fn pane_state(&self, pane: &Pane) -> Result<PaneState, TmuxError> {
        let output = self.run_ok(&self.state_call(&pane.id))?;
        one_line(&output.stdout)
            .and_then(parse_pane_state)
            .ok_or_else(|| unreadable("pane state"))
    }

    fn field(&self, pane: &Pane, format: &str) -> Result<String, TmuxError> {
        let output = self.run_ok(&self.field_call(&pane.id, format))?;
        one_line(&output.stdout)
            .map(str::to_string)
            .ok_or_else(|| unreadable("pane field"))
    }

    /// The socket path of the server the pane lives on, as the panes' programs see it in `TMUX`.
    ///
    /// # Errors
    ///
    /// A failure status or an unreadable answer (a tmux that does not know the variable).
    pub fn socket_path(&self, pane: &Pane) -> Result<String, TmuxError> {
        let path = self.field(pane, "#{socket_path}")?;
        if path.starts_with('/') {
            Ok(path)
        } else {
            Err(unreadable("socket path"))
        }
    }

    /// The process id of the tmux server the pane lives on (for the pane's hook counter file).
    ///
    /// # Errors
    ///
    /// A failure status or an unreadable answer.
    pub fn server_pid(&self, pane: &Pane) -> Result<u32, TmuxError> {
        self.field(pane, "#{pid}")?
            .parse()
            .map_err(|_| unreadable("server pid"))
    }

    /// State, foreground command and directory of the pane, each from its own call.
    ///
    /// # Errors
    ///
    /// A failure status or an unreadable answer.
    pub fn pane_info(&self, pane: &Pane) -> Result<PaneInfo, TmuxError> {
        let state = self.pane_state(pane)?;
        Ok(PaneInfo {
            dead: state.dead,
            pid: state.pid,
            current_command: self.field(pane, "#{pane_current_command}")?,
            current_path: self.field(pane, "#{pane_current_path}")?,
        })
    }

    /// The pane's state, after checking it is still the recorded pane: it belongs to the session
    /// and its process is the recorded one (the id is what is asked, so the id matches by
    /// construction).
    fn require_owned(&self, pane: &Pane) -> Result<PaneState, TmuxError> {
        let state = self.pane_state(pane)?;
        if state.session_name == pane.name.as_str() && state.pid == pane.pid {
            Ok(state)
        } else {
            Err(TmuxError::Refused(Refusal::NotOwned))
        }
    }

    /// The visible text of the pane, with `scrollback_lines` more lines above it when given.
    ///
    /// # Errors
    ///
    /// A failure status.
    pub fn capture_pane(&self, pane: &Pane, scrollback_lines: Option<u32>) -> Result<String, TmuxError> {
        Ok(self
            .run_ok(&self.capture_pane_call(&pane.id, scrollback_lines, false))?
            .stdout)
    }

    /// The pane as text with escape sequences (`capture-pane -e`): the input-line decision reads the
    /// faint attribute from it. [`super::detect`] strips the escapes for every other use.
    pub fn capture_pane_styled(&self, pane: &Pane, scrollback_lines: Option<u32>) -> Result<String, TmuxError> {
        Ok(self
            .run_ok(&self.capture_pane_call(&pane.id, scrollback_lines, true))?
            .stdout)
    }

    /// The `client_activity` timestamps (seconds) of the clients attached to the session.
    ///
    /// # Errors
    ///
    /// A failure status.
    pub fn client_activity(&self, name: &TmuxName) -> Result<Vec<u64>, TmuxError> {
        let output = self.run_ok(&self.list_clients_call(name))?;
        Ok(output
            .stdout
            .lines()
            .filter_map(|line| line.trim().parse().ok())
            .collect())
    }

    /// The gate every push passes (see the module documentation).
    fn check_gate(&self, pane: &Pane, guard: &PasteGuard<'_>) -> Result<(), TmuxError> {
        let state = self.require_owned(pane)?;
        if state.dead {
            return Err(TmuxError::Refused(Refusal::PaneDead));
        }
        if state.in_mode != 0 {
            return Err(TmuxError::Refused(Refusal::PaneInMode));
        }
        let command = self.field(pane, "#{pane_current_command}")?;
        if !guard.allowed_commands.contains(&command.as_str()) {
            return Err(TmuxError::Refused(Refusal::CommandNotAllowed));
        }
        match state.bracket_paste {
            Some(true) => Ok(()),
            Some(false) => Err(TmuxError::Refused(Refusal::BracketedPasteOff)),
            None if guard.bracketed_confirmed.is_some() => Ok(()),
            None => Err(TmuxError::Refused(Refusal::BracketedPasteUnconfirmed)),
        }
    }

    /// Types `text` into the pane as a bracketed paste, waits `settle`, and presses Enter. The
    /// whole push holds the session's lock, so two pushes (or a push and an Enter retry) for one
    /// session never interleave.
    ///
    /// # Errors
    ///
    /// - [`TmuxError::Invalid`]: the text is refused; nothing ran.
    /// - [`TmuxError::Refused`]: the gate refused; nothing was loaded.
    /// - [`TmuxError::DeliveryUnknown`]: the paste timed out; do not paste again.
    /// - [`TmuxError::PastedNotSubmitted`]: the paste worked, Enter did not; retry only Enter.
    /// - any other error: nothing was pasted (the buffer is deleted).
    pub fn push_message(
        &self,
        pane: &Pane,
        task_id: &Uuid,
        text: &str,
        guard: &PasteGuard<'_>,
        settle: Duration,
    ) -> Result<(), TmuxError> {
        let text = PushText::new(text)?;
        let buffer = BufferName::for_task(task_id);
        let lock = self.lock_for(&pane.name);
        let _guard = lock.lock();
        self.check_gate(pane, guard)?;
        if let Err(error) = self.run_ok(&self.load_buffer_call(&buffer, &text)) {
            let _ = self.run(&self.delete_buffer_call(&buffer));
            return Err(error);
        }
        if let Err(error) = self.run_ok(&self.paste_buffer_call(&buffer, &pane.id)) {
            let _ = self.run(&self.delete_buffer_call(&buffer));
            return Err(if error == TmuxError::Timeout {
                TmuxError::DeliveryUnknown
            } else {
                error
            });
        }
        thread::sleep(settle);
        self.run_ok(&self.send_enter_call(&pane.id))
            .map(|_| ())
            .map_err(|_| TmuxError::PastedNotSubmitted)
    }

    /// Presses Enter again (the only retry design 6.5 allows), under the session's lock and after
    /// checking the pane still belongs to the session and is alive and out of any mode.
    ///
    /// # Errors
    ///
    /// A refusal or a failure status.
    pub fn retry_enter(&self, pane: &Pane) -> Result<(), TmuxError> {
        let lock = self.lock_for(&pane.name);
        let _guard = lock.lock();
        let state = self.require_owned(pane)?;
        if state.dead {
            return Err(TmuxError::Refused(Refusal::PaneDead));
        }
        if state.in_mode != 0 {
            return Err(TmuxError::Refused(Refusal::PaneInMode));
        }
        self.run_ok(&self.send_enter_call(&pane.id)).map(|_| ())
    }
}

fn read_all<R: Read + Send + 'static>(mut source: R) -> thread::JoinHandle<String> {
    thread::spawn(move || {
        let mut bytes = Vec::new();
        let _ = source.by_ref().take(1 << 20).read_to_end(&mut bytes);
        String::from_utf8_lossy(&bytes).into_owned()
    })
}

fn failed(output: &Output) -> TmuxError {
    TmuxError::Failed {
        code: output.code,
        stderr: output.stderr.trim().chars().take(200).collect(),
    }
}

fn unreadable(what: &str) -> TmuxError {
    TmuxError::Failed {
        code: Some(0),
        stderr: format!("unreadable {what}"),
    }
}

#[cfg(test)]
#[path = "tmux_tests.rs"]
mod tests;
