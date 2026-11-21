//! The tmux driver: every interaction with tmux as an argument vector.
//!
//! # Safety rules (design sections 6.4 and 6.5, amended by the security review)
//!
//! - **No shell.** Every call is `std::process::Command` with an explicit argument vector. Nothing
//!   is ever interpolated into a command line.
//! - **Own server, no user configuration.** Every call carries `-L <socket>` (an empty socket
//!   name means the user's default server and omits the flag) and `-f /dev/null`, so a server the
//!   runner starts loads no `tmux.conf` (no `base-index`, hooks or `default-command` of the user).
//! - **Panes by id.** [`Tmux::new_session`] prints the new pane's id (`%N`, unique for the life
//!   of the server) and returns it as a [`Pane`]. Every later call targets that id, never a window
//!   index, so `move-window`, `swap-window` or `base-index` cannot redirect it. Operations that
//!   change the pane first check that it still belongs to session `KEY` (`#{session_name}`).
//!   Session targets, where one is needed, are `=KEY` (exact match: `AB` never reaches `ABC`, and
//!   an all-digit key is a name). Keys are `^[A-Z0-9]{1,32}$`, which has no `:`, `.`, `=`, `%`.
//! - **Format strings.** tmux expands several arguments as formats. Checked on tmux 3.7b with an
//!   isolated server (`tests/tmux_integration.rs`): `-c DIR` of `new-session` and `respawn-pane`
//!   IS expanded (`#{session_name}` in a directory name was replaced, and a `#(...)` part was
//!   consumed; on 3.7b the command inside `#(...)` did not run, but 3.2 to 3.6 were not available
//!   to test, and the documented behaviour of `#(...)` is to run `/bin/sh`). A [`WorkDir`
//!   therefore refuses `#` altogether. The elements of the program argument vector after `--` are
//!   NOT expanded (verified). The other arguments are constants (`-n cli`, `-F`, the
//!   `display-message` formats, `set-option` values) or validated types (`-s KEY`, `-t`).
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
//!   alive, in no mode (copy mode would swallow Enter), owned by the session, and its foreground
//!   command is one of the CLI's process names the caller passes. Then, on a server that reports
//!   the flag, the flag must be `1`. On a server that does not (3.2 to 3.6), tmux cannot tell, so
//!   the caller must hand over a [`BracketedPasteConfirmed`], which only the session manager's
//!   readiness check may create (see its documentation for what P5.5 must do). Without it the push
//!   is refused.
//! - **One key.** The only `send-keys` is `send-keys -t %N Enter`. No `C-c`, ever.
//! - **No shell for the CLI either.** `new-session` and `respawn-pane` pass the program argument
//!   vector after `--`. tmux runs a single argument through `/bin/sh -c` (verified), so a
//!   one-element vector is run as `/usr/bin/env -- <program>`, which tmux executes directly.
//! - **Serialisation and partial failure.** Pushes and Enter retries for one session are
//!   serialised inside the driver. A paste that times out may or may not have been delivered, so
//!   it is reported as [`TmuxError::DeliveryUnknown`] and is never retried here; the buffer is
//!   deleted on every failure path.
//! - **Environment.** tmux processes get an allow-list of variables ([`ENV_ALLOWED`]); the agent
//!   variables `CLAUDECODE`, `CLAUDE_CODE_ENTRYPOINT` and `TMUX` are never passed. Under systemd
//!   (`INVOCATION_ID` set) a server that is not yet running is started through
//!   `systemd-run --user --scope --unit=sylvode-tmux-<socket>`.
//! - **Remaining trust assumption.** Everything on one socket runs as one OS user. tmux sets
//!   `TMUX` inside each pane, so an agent that may run shell commands can drive every other
//!   session on the same server (`tmux send-keys -t ...`). Nothing in-process prevents that; use a
//!   separate socket (and OS user) per trust level.
//! - **Only typed operations are public.** The call builders and the process runner are private.
//!
//! # Version floor
//!
//! The minimum tmux is 3.2. Checked against tmux's `CHANGES` (3.7b): the argument-vector form of
//! `new-session` and `respawn-pane` exists since 2.0, `respawn-pane -c` since 2.6, `=` exact
//! targets since 2.1, `new-session -P -F` and pane ids long before 2.0. Nothing the driver relies
//! on is newer than 3.2 except `#{bracket_paste_flag}` (3.7), which is optional as described
//! above. Since 3.4 `paste-buffer` refuses a dead pane, which is reported as an ordinary failure.

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

/// The variables a process of the runner's gets from the runner's own environment: the
/// allow-list of [`ENV_ALLOWED`] and `LC_*`, the names in `extra` (an operator's explicit
/// pass-through list), and for the `systemd-run --user` wrapper the two variables it needs. The
/// agent variables are never passed. This one definition serves the tmux server and the headless
/// CLI, so the two channels give a CLI the same environment.
pub fn allowed_environment(extra: &[String], for_systemd_run: bool) -> Vec<(OsString, OsString)> {
    std::env::vars_os()
        .filter(|(name, _)| {
            let name = name.to_string_lossy();
            let allowed = ENV_ALLOWED.contains(&&*name)
                || name.starts_with("LC_")
                || extra.iter().any(|wanted| *wanted == *name)
                || (for_systemd_run && ENV_ALLOWED_SYSTEMD.contains(&&*name));
            allowed && !ENV_REMOVED.contains(&&*name)
        })
        .collect()
}

/// Needed only by the `systemd-run --user` wrapper.
const ENV_ALLOWED_SYSTEMD: [&str; 2] = ["XDG_RUNTIME_DIR", "DBUS_SESSION_BUS_ADDRESS"];

const ENV_REMOVED: [&str; 3] = ["CLAUDECODE", "CLAUDE_CODE_ENTRYPOINT", "TMUX"];

/// The name of the pane window the runner owns.
pub const WINDOW_NAME: &str = "cli";

/// Why a push was refused before any text was loaded.
#[derive(Debug, PartialEq, Eq)]
pub enum Refusal {
    /// The pane no longer belongs to the session (it was moved or the session renamed).
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

/// A project key used as a session name: `^[A-Z0-9]{1,32}$`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SessionKey(String);

impl SessionKey {
    /// # Errors
    ///
    /// [`TmuxError::Invalid`] for any other value.
    pub fn new(key: &str) -> Result<Self, TmuxError> {
        if !key.is_empty() && key.len() <= 32 && key.chars().all(|c| c.is_ascii_uppercase() || c.is_ascii_digit()) {
            Ok(Self(key.to_string()))
        } else {
            Err(TmuxError::Invalid(
                "session key must match ^[A-Z0-9]{1,32}$".to_string(),
            ))
        }
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// The exact session target, `=KEY`.
    pub fn session_target(&self) -> String {
        format!("={}", self.0)
    }
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

/// The CLI pane of a session: what the runner records at creation and uses ever after.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Pane {
    pub key: SessionKey,
    pub id: PaneId,
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
    config_file: OsString,
    extra_env: Vec<(OsString, OsString)>,
    under_systemd: bool,
    timeout: Duration,
}

impl Tmux {
    /// A driver that runs `tmux` from the `PATH` against the server on `socket`, with no user
    /// configuration (`-f /dev/null`).
    pub fn new(socket: SocketName) -> Self {
        Self {
            program: OsString::from("tmux"),
            systemd_run: OsString::from("systemd-run"),
            socket,
            config_file: OsString::from("/dev/null"),
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

    /// Starts servers with `-f <file>` instead of `/dev/null`. For tests that need a
    /// configuration (the production runner never loads one).
    #[must_use]
    pub fn with_config_file(mut self, file: impl Into<OsString>) -> Self {
        self.config_file = file.into();
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

    /// The global options every call starts with: `-L <socket>` and `-f <file>`.
    fn global(&self) -> Vec<String> {
        let mut args = Vec::new();
        if !self.socket.as_str().is_empty() {
            args.push("-L".to_string());
            args.push(self.socket.as_str().to_string());
        }
        args.push("-f".to_string());
        args.push(self.config_file.to_string_lossy().into_owned());
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

    fn has_session_call(&self, key: &SessionKey) -> Call {
        self.call(&["has-session", "-t", &key.session_target()])
    }

    fn list_sessions_call(&self) -> Call {
        self.call(&["list-sessions"])
    }

    fn new_session_call(
        &self,
        key: &SessionKey,
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
            "#{pane_id}",
            "-s",
            key.as_str(),
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

    fn remain_on_exit_call(&self, pane: &PaneId) -> Call {
        self.call(&["set-option", "-w", "-t", pane.as_str(), "remain-on-exit", "on"])
    }

    fn respawn_pane_call(&self, pane: &PaneId, dir: &WorkDir, argv: &[String]) -> Result<Call, TmuxError> {
        let argv = exec_argv(argv)?;
        let mut call = self.call(&["respawn-pane", "-k", "-t", pane.as_str(), "-c", dir.as_str(), "--"]);
        call.args.extend(argv);
        Ok(call)
    }

    fn adopt_call(&self, key: &SessionKey) -> Call {
        self.call(&["list-panes", "-t", &format!("={}:0", key.as_str()), "-F", "#{pane_id}"])
    }

    fn state_call(&self, pane: &PaneId) -> Call {
        self.call(&["display-message", "-p", "-t", pane.as_str(), STATE_FORMAT])
    }

    fn field_call(&self, pane: &PaneId, format: &str) -> Call {
        self.call(&["display-message", "-p", "-t", pane.as_str(), format])
    }

    fn capture_pane_call(&self, pane: &PaneId, scrollback_lines: Option<u32>) -> Call {
        let start = scrollback_lines.map(|lines| format!("-{lines}"));
        let mut rest = vec!["capture-pane", "-p", "-t", pane.as_str()];
        if let Some(start) = &start {
            rest.push("-S");
            rest.push(start);
        }
        self.call(&rest)
    }

    fn list_clients_call(&self, key: &SessionKey) -> Call {
        self.call(&["list-clients", "-F", "#{client_activity}", "-t", &key.session_target()])
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

    fn command(&self, wrap_in_scope: bool) -> Command {
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
        for (name, value) in allowed_environment(&[], wrap_in_scope) {
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
        self.run_command(self.command(false), call)
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

    fn lock_for(&self, key: &SessionKey) -> Arc<Mutex<()>> {
        let name = format!("{}\0{}", self.socket.as_str(), key.as_str());
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

    /// Whether session `key` exists. Exit status 1 (no such session, or no server) is `false`.
    ///
    /// # Errors
    ///
    /// Any other failure.
    pub fn has_session(&self, key: &SessionKey) -> Result<bool, TmuxError> {
        let output = self.run(&self.has_session_call(key))?;
        match output.code {
            Some(0) => Ok(true),
            Some(1) => Ok(false),
            _ => Err(failed(&output)),
        }
    }

    /// Whether a server is running on the socket.
    ///
    /// # Errors
    ///
    /// Spawn failures and timeouts.
    pub fn server_running(&self) -> Result<bool, TmuxError> {
        Ok(self.run(&self.list_sessions_call())?.code == Some(0))
    }

    /// Creates session `key` with one window running `argv` and returns its pane (the caller
    /// records it). Then sets `remain-on-exit` so a CLI that exits leaves a dead pane. When the
    /// server is not yet running and the process is a systemd service, the server is started in
    /// its own scope.
    ///
    /// # Errors
    ///
    /// Invalid arguments and tmux failures.
    pub fn new_session(
        &self,
        key: &SessionKey,
        dir: &WorkDir,
        width: u16,
        height: u16,
        argv: &[String],
    ) -> Result<Pane, TmuxError> {
        let call = self.new_session_call(key, dir, width, height, argv)?;
        let scope = self.under_systemd && !self.server_running()?;
        let output = self.run_command(self.command(scope), &call)?;
        if output.code != Some(0) {
            return Err(failed(&output));
        }
        let id = one_line(&output.stdout).ok_or_else(|| unreadable("pane id"))?;
        let pane = Pane {
            key: key.clone(),
            id: PaneId::new(id)?,
        };
        self.run_ok(&self.remain_on_exit_call(&pane.id))?;
        Ok(pane)
    }

    /// The pane of window 0 of a session the runner did not create. Window 0 is looked up once,
    /// here; the caller records the id and uses it from then on.
    ///
    /// # Errors
    ///
    /// A failure status (no such session or window) or an unreadable answer.
    pub fn adopt_pane(&self, key: &SessionKey) -> Result<Pane, TmuxError> {
        let output = self.run_ok(&self.adopt_call(key))?;
        let first = output.stdout.lines().next().ok_or_else(|| unreadable("pane id"))?;
        Ok(Pane {
            key: key.clone(),
            id: PaneId::new(first)?,
        })
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

    fn require_owned(&self, pane: &Pane) -> Result<PaneState, TmuxError> {
        let state = self.pane_state(pane)?;
        if state.session_name == pane.key.as_str() {
            Ok(state)
        } else {
            Err(TmuxError::Refused(Refusal::NotOwned))
        }
    }

    /// Restarts the pane with `argv` in `dir`, after checking it still belongs to the session.
    ///
    /// # Errors
    ///
    /// Invalid arguments, a pane that is no longer the session's, and tmux failures.
    pub fn respawn_pane(&self, pane: &Pane, dir: &WorkDir, argv: &[String]) -> Result<(), TmuxError> {
        let call = self.respawn_pane_call(&pane.id, dir, argv)?;
        let lock = self.lock_for(&pane.key);
        let _guard = lock.lock();
        self.require_owned(pane)?;
        self.run_ok(&call).map(|_| ())
    }

    /// The visible text of the pane, with `scrollback_lines` more lines above it when given.
    ///
    /// # Errors
    ///
    /// A failure status.
    pub fn capture_pane(&self, pane: &Pane, scrollback_lines: Option<u32>) -> Result<String, TmuxError> {
        Ok(self.run_ok(&self.capture_pane_call(&pane.id, scrollback_lines))?.stdout)
    }

    /// The `client_activity` timestamps (seconds) of the clients attached to the session.
    ///
    /// # Errors
    ///
    /// A failure status.
    pub fn client_activity(&self, key: &SessionKey) -> Result<Vec<u64>, TmuxError> {
        let output = self.run_ok(&self.list_clients_call(key))?;
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
        let lock = self.lock_for(&pane.key);
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
        let lock = self.lock_for(&pane.key);
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

    /// Ends session `key` (exact match). Only for sessions the runner created.
    ///
    /// # Errors
    ///
    /// A failure status or a timeout.
    pub fn kill_session(&self, key: &SessionKey) -> Result<(), TmuxError> {
        self.run_ok(&self.call(&["kill-session", "-t", &key.session_target()]))
            .map(|_| ())
    }

    /// The command that attaches the operator's terminal to session `key`. It keeps the caller's
    /// environment (the terminal needs `TERM`) and standard streams.
    pub fn attach_command(&self, key: &SessionKey) -> Command {
        let mut command = Command::new(&self.program);
        if !self.socket.as_str().is_empty() {
            command.args(["-L", self.socket.as_str()]);
        }
        command.args(["attach", "-t", &key.session_target()]);
        command
    }

    /// Stops the server on a socket of the runner's own. Refused for the user's default server
    /// (an empty socket name). Meant for tests and for pruning.
    ///
    /// # Errors
    ///
    /// [`TmuxError::Invalid`] for the default server; spawn failures and timeouts.
    pub fn kill_server(&self) -> Result<(), TmuxError> {
        if self.socket.as_str().is_empty() {
            return Err(TmuxError::Invalid("will not stop the default tmux server".to_string()));
        }
        self.run(&self.call(&["kill-server"])).map(|_| ())
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
