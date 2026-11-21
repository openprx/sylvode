//! The tmux driver: every interaction with tmux as an argument vector.
//!
//! # Safety rules (design sections 6.4 and 6.5)
//!
//! - **No shell.** Every call is `std::process::Command` with an explicit argument vector. Nothing
//!   is ever interpolated into a command line, so no value can be split, globbed or expanded.
//! - **Own server.** Every call carries `-L <socket>` (an empty socket name means the user's
//!   default server and omits the flag).
//! - **Exact targets.** A session is addressed as `=KEY` and window 0 as `=KEY:0`, only through
//!   [`SessionKey`], which accepts `^[A-Z0-9]{1,32}$`. The `=` forces an exact name match, so `AB`
//!   never reaches `ABC`, and an all-digit key such as `123` is a name, not an index. `:` and `.`
//!   cannot occur in a key, so a key cannot name a window or pane of another session.
//! - **Text only through stdin and a buffer.** Message text reaches tmux through
//!   `load-buffer -b sv-<task uuid> -` on standard input, then `paste-buffer -p -d -b sv-<task
//!   uuid> -t =KEY:0` (`-p` wraps the paste in bracketed-paste markers, `-d` deletes the buffer).
//!   No argument contains message text. [`PushText`] refuses control characters other than line
//!   feed, so an escape byte can never reach the terminal, even from a caller that skipped the
//!   title sanitiser.
//! - **`send-keys` sends the single key name `Enter` and nothing else.** There is no other
//!   `send-keys` call, in particular no `C-c`.
//! - **The CLI is the pane's command.** `new-session` and `respawn-pane` pass the CLI argument
//!   vector after `--`, so no shell parses it.
//! - **Clean environment.** `CLAUDECODE`, `CLAUDE_CODE_ENTRYPOINT` and `TMUX` are removed from every
//!   tmux process, so a server started from inside another agent session does not inherit them.
//!   Under systemd (`INVOCATION_ID` set) a server that is not yet running is started through
//!   `systemd-run --user --scope --unit=sylvode-tmux-<socket>`, so restarting the service does
//!   not take the server down with it.
//!
//! # Version floor
//!
//! The minimum tmux is 3.2. Checked against tmux's `CHANGES` (3.7b): the argument-vector form of
//! `new-session` and `respawn-pane` exists since 2.0, `respawn-pane -c` since 2.6, `=` exact
//! targets since 2.1, `load-buffer -` since long before 2.0, bracketed paste since 1.x. Nothing
//! the driver relies on is newer than 3.2, so the floor stays 3.2 (it is the oldest version the
//! supported distributions ship). Since 3.4 `paste-buffer` refuses a dead pane, which the driver
//! reports as an ordinary failure.

use std::ffi::OsString;
use std::io::{Read, Write};
use std::path::Path;
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use uuid::Uuid;

/// How long one tmux call may take.
pub const CALL_TIMEOUT: Duration = Duration::from_secs(10);

const ENV_REMOVED: [&str; 3] = ["CLAUDECODE", "CLAUDE_CODE_ENTRYPOINT", "TMUX"];

/// The name of the pane window the runner owns.
pub const WINDOW_NAME: &str = "cli";

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
}

impl std::fmt::Display for TmuxError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Invalid(why) => write!(f, "refused tmux argument: {why}"),
            Self::Spawn(why) => write!(f, "could not run tmux: {why}"),
            Self::Timeout => write!(f, "tmux did not answer in time"),
            Self::Failed { code, stderr } => write!(f, "tmux failed (status {code:?}): {stderr}"),
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

    /// The exact target of window 0, `=KEY:0`.
    pub fn window_target(&self) -> String {
        format!("={}:0", self.0)
    }
}

/// A paste buffer name, `sv-<task uuid>`: one buffer per task, so concurrent pushes to different
/// sessions cannot paste each other's text.
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

/// Text that may be pasted: no control character other than line feed. That excludes ESC and
/// every other C0 and C1 control and DEL, so nothing in it can act as a terminal sequence or end
/// the bracketed paste early.
#[derive(Debug, PartialEq, Eq)]
pub struct PushText<'a>(&'a str);

impl<'a> PushText<'a> {
    /// # Errors
    ///
    /// [`TmuxError::Invalid`] naming the kind of character, never the text.
    pub fn new(text: &'a str) -> Result<Self, TmuxError> {
        if text.is_empty() {
            return Err(TmuxError::Invalid("empty text".to_string()));
        }
        if text.chars().any(|c| c.is_control() && c != '\n') {
            return Err(TmuxError::Invalid("text contains a control character".to_string()));
        }
        Ok(Self(text))
    }
}

/// What `display-message` reported about window 0 of a session.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PaneInfo {
    pub dead: bool,
    pub pid: u32,
    pub current_command: String,
    pub current_path: String,
}

/// The format string of [`Tmux::pane_info`], as the design gives it.
pub const PANE_FORMAT: &str = "#{pane_dead} #{pane_pid} #{pane_current_command} #{pane_current_path}";

/// Parses the answer to [`PANE_FORMAT`]. The path comes last, so it may contain spaces.
pub fn parse_pane_info(line: &str) -> Option<PaneInfo> {
    let mut parts = line.trim_end_matches(['\n', '\r']).splitn(4, ' ');
    let dead = match parts.next()? {
        "0" => false,
        "1" => true,
        _ => return None,
    };
    let pid = parts.next()?.parse().ok()?;
    let current_command = parts.next()?.to_string();
    let current_path = parts.next()?.to_string();
    Some(PaneInfo {
        dead,
        pid,
        current_command,
        current_path,
    })
}

/// One tmux call: its argument vector (after the program name) and what goes to stdin.
#[derive(Debug, PartialEq, Eq)]
pub struct Call {
    pub args: Vec<String>,
    pub stdin: Option<Vec<u8>>,
}

/// A finished tmux call.
#[derive(Debug)]
pub struct Output {
    pub code: Option<i32>,
    pub stdout: String,
    pub stderr: String,
}

/// The tmux driver for one server.
#[derive(Clone, Debug)]
pub struct Tmux {
    program: OsString,
    systemd_run: OsString,
    socket: SocketName,
    config_file: Option<OsString>,
    extra_env: Vec<(OsString, OsString)>,
    under_systemd: bool,
    timeout: Duration,
}

impl Tmux {
    /// A driver that runs `tmux` from the `PATH` against the server on `socket`.
    pub fn new(socket: SocketName) -> Self {
        Self {
            program: OsString::from("tmux"),
            systemd_run: OsString::from("systemd-run"),
            socket,
            config_file: None,
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

    /// Starts servers with `-f <file>` instead of the user's configuration.
    #[must_use]
    pub fn with_config_file(mut self, file: impl Into<OsString>) -> Self {
        self.config_file = Some(file.into());
        self
    }

    /// Sets one more environment variable on every tmux process (`TMUX_TMPDIR`, say).
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

    /// The global options every call starts with: `-L <socket>` and, when set, `-f <file>`.
    fn global(&self) -> Vec<String> {
        let mut args = Vec::new();
        if !self.socket.as_str().is_empty() {
            args.push("-L".to_string());
            args.push(self.socket.as_str().to_string());
        }
        if let Some(file) = &self.config_file {
            args.push("-f".to_string());
            args.push(file.to_string_lossy().into_owned());
        }
        args
    }

    fn call(&self, rest: &[&str]) -> Call {
        let mut args = self.global();
        args.extend(rest.iter().map(|arg| (*arg).to_string()));
        Call { args, stdin: None }
    }

    /// `tmux -V`.
    pub fn version_call() -> Call {
        Call {
            args: vec!["-V".to_string()],
            stdin: None,
        }
    }

    /// `tmux -L S has-session -t =KEY`.
    pub fn has_session_call(&self, key: &SessionKey) -> Call {
        self.call(&["has-session", "-t", &key.session_target()])
    }

    /// `tmux -L S list-sessions` (is a server running).
    pub fn list_sessions_call(&self) -> Call {
        self.call(&["list-sessions"])
    }

    /// `tmux -L S new-session -d -s KEY -n cli -x W -y H -c DIR -- ARGV...`.
    ///
    /// # Errors
    ///
    /// [`TmuxError::Invalid`] for a relative directory or an empty command.
    pub fn new_session_call(
        &self,
        key: &SessionKey,
        dir: &Path,
        width: u16,
        height: u16,
        argv: &[String],
    ) -> Result<Call, TmuxError> {
        let dir = checked_dir(dir)?;
        checked_argv(argv)?;
        let (width, height) = (width.to_string(), height.to_string());
        let mut call = self.call(&[
            "new-session",
            "-d",
            "-s",
            key.as_str(),
            "-n",
            WINDOW_NAME,
            "-x",
            &width,
            "-y",
            &height,
            "-c",
            &dir,
            "--",
        ]);
        call.args.extend(argv.iter().cloned());
        Ok(call)
    }

    /// `tmux -L S set-option -w -t =KEY:0 remain-on-exit on`.
    pub fn remain_on_exit_call(&self, key: &SessionKey) -> Call {
        self.call(&["set-option", "-w", "-t", &key.window_target(), "remain-on-exit", "on"])
    }

    /// `tmux -L S respawn-pane -k -t =KEY:0 -c DIR -- ARGV...`.
    ///
    /// # Errors
    ///
    /// [`TmuxError::Invalid`] for a relative directory or an empty command.
    pub fn respawn_pane_call(&self, key: &SessionKey, dir: &Path, argv: &[String]) -> Result<Call, TmuxError> {
        let dir = checked_dir(dir)?;
        checked_argv(argv)?;
        let mut call = self.call(&["respawn-pane", "-k", "-t", &key.window_target(), "-c", &dir, "--"]);
        call.args.extend(argv.iter().cloned());
        Ok(call)
    }

    /// `tmux -L S display-message -p -t =KEY:0 '<PANE_FORMAT>'`.
    pub fn pane_info_call(&self, key: &SessionKey) -> Call {
        self.call(&["display-message", "-p", "-t", &key.window_target(), PANE_FORMAT])
    }

    /// `tmux -L S capture-pane -p -t =KEY:0`, with `-S -<lines>` for scrollback.
    pub fn capture_pane_call(&self, key: &SessionKey, scrollback_lines: Option<u32>) -> Call {
        let start = scrollback_lines.map(|lines| format!("-{lines}"));
        let mut rest = vec!["capture-pane", "-p", "-t"];
        let target = key.window_target();
        rest.push(&target);
        if let Some(start) = &start {
            rest.push("-S");
            rest.push(start);
        }
        self.call(&rest)
    }

    /// `tmux -L S list-clients -F '#{client_activity}' -t =KEY`.
    pub fn list_clients_call(&self, key: &SessionKey) -> Call {
        self.call(&["list-clients", "-F", "#{client_activity}", "-t", &key.session_target()])
    }

    /// `tmux -L S load-buffer -b sv-<task uuid> -`, the text on stdin.
    pub fn load_buffer_call(&self, buffer: &BufferName, text: &PushText<'_>) -> Call {
        let mut call = self.call(&["load-buffer", "-b", buffer.as_str(), "-"]);
        call.stdin = Some(text.0.as_bytes().to_vec());
        call
    }

    /// `tmux -L S paste-buffer -p -d -b sv-<task uuid> -t =KEY:0`.
    pub fn paste_buffer_call(&self, buffer: &BufferName, key: &SessionKey) -> Call {
        self.call(&[
            "paste-buffer",
            "-p",
            "-d",
            "-b",
            buffer.as_str(),
            "-t",
            &key.window_target(),
        ])
    }

    /// `tmux -L S send-keys -t =KEY:0 Enter`: the one key name, never text.
    pub fn send_enter_call(&self, key: &SessionKey) -> Call {
        self.call(&["send-keys", "-t", &key.window_target(), "Enter"])
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
        for (key, value) in &self.extra_env {
            command.env(key, value);
        }
        // Removal wins over anything set above.
        for name in ENV_REMOVED {
            command.env_remove(name);
        }
        command
    }

    /// Runs a call with the time limit. A non-zero exit is not an error here; see the methods.
    ///
    /// # Errors
    ///
    /// [`TmuxError::Spawn`] and [`TmuxError::Timeout`].
    pub fn run(&self, call: &Call) -> Result<Output, TmuxError> {
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
        let mut child = command.spawn().map_err(|e| TmuxError::Spawn(e.kind().to_string()))?;
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

    /// Creates session `key` with window 0 running `argv`, then sets `remain-on-exit` so a CLI that
    /// exits leaves a dead pane instead of destroying the window. When the server is not yet
    /// running and the process is a systemd service, the server is started in its own scope.
    ///
    /// # Errors
    ///
    /// Invalid arguments and tmux failures.
    pub fn new_session(
        &self,
        key: &SessionKey,
        dir: &Path,
        width: u16,
        height: u16,
        argv: &[String],
    ) -> Result<(), TmuxError> {
        let call = self.new_session_call(key, dir, width, height, argv)?;
        let scope = self.under_systemd && !self.server_running()?;
        let output = self.run_command(self.command(scope), &call)?;
        if output.code != Some(0) {
            return Err(failed(&output));
        }
        self.run_ok(&self.remain_on_exit_call(key)).map(|_| ())
    }

    /// Restarts window 0 with `argv` in `dir`.
    ///
    /// # Errors
    ///
    /// Invalid arguments and tmux failures.
    pub fn respawn_pane(&self, key: &SessionKey, dir: &Path, argv: &[String]) -> Result<(), TmuxError> {
        self.run_ok(&self.respawn_pane_call(key, dir, argv)?).map(|_| ())
    }

    /// State of window 0.
    ///
    /// # Errors
    ///
    /// A failure status (no such session) or an answer that does not parse.
    pub fn pane_info(&self, key: &SessionKey) -> Result<PaneInfo, TmuxError> {
        let output = self.run_ok(&self.pane_info_call(key))?;
        parse_pane_info(&output.stdout).ok_or_else(|| TmuxError::Failed {
            code: Some(0),
            stderr: "unreadable pane information".to_string(),
        })
    }

    /// The visible text of window 0, with `scrollback_lines` more lines above it when given.
    ///
    /// # Errors
    ///
    /// A failure status.
    pub fn capture_pane(&self, key: &SessionKey, scrollback_lines: Option<u32>) -> Result<String, TmuxError> {
        Ok(self.run_ok(&self.capture_pane_call(key, scrollback_lines))?.stdout)
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

    /// Loads `text` into the task's buffer and pastes it into window 0 as a bracketed paste. The
    /// buffer is deleted by the paste; when the paste fails the buffer is deleted here.
    ///
    /// # Errors
    ///
    /// Refused text and tmux failures.
    pub fn paste_text(&self, key: &SessionKey, task_id: &Uuid, text: &str) -> Result<(), TmuxError> {
        let text = PushText::new(text)?;
        let buffer = BufferName::for_task(task_id);
        self.run_ok(&self.load_buffer_call(&buffer, &text))?;
        if let Err(error) = self.run_ok(&self.paste_buffer_call(&buffer, key)) {
            let _ = self.run(&self.call(&["delete-buffer", "-b", buffer.as_str()]));
            return Err(error);
        }
        Ok(())
    }

    /// Presses Enter in window 0.
    ///
    /// # Errors
    ///
    /// A failure status.
    pub fn send_enter(&self, key: &SessionKey) -> Result<(), TmuxError> {
        self.run_ok(&self.send_enter_call(key)).map(|_| ())
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

fn checked_dir(dir: &Path) -> Result<String, TmuxError> {
    if !dir.is_absolute() {
        return Err(TmuxError::Invalid("working directory must be absolute".to_string()));
    }
    dir.to_str()
        .map(str::to_string)
        .ok_or_else(|| TmuxError::Invalid("working directory must be UTF-8".to_string()))
}

fn checked_argv(argv: &[String]) -> Result<(), TmuxError> {
    if argv.is_empty() || argv.iter().any(String::is_empty) {
        return Err(TmuxError::Invalid(
            "the command needs a program and no empty argument".to_string(),
        ));
    }
    Ok(())
}
