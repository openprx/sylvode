//! The runner's state directory and the files in it that hold secrets or drive hooks (design
//! section 6.7).
//!
//! ```text
//! <state_dir>/                     0700, created with that mode
//!   runner.lock                    exclusive flock: one runner per state directory
//!   hook.sock                      the hook socket
//!   mcp/<bot_id>.toml              0600: the bot's token and the MCP server's settings
//!   mcp/<bot_id>.claude.json       0600: references the TOML, holds no token
//!   sessions/<NAME>/session.json  0600: what the runner knows about a session (NAME = <KEY>-<cli>)
//!   sessions/<NAME>/claude-settings.json   0600: the hook commands
//! ```
//!
//! Every directory the runner creates is made with mode 0700 and every file with mode 0600 at
//! creation (the mode is given to `open`, never changed afterwards), exclusively and through a
//! temporary name followed by a rename, so no reader sees a half-written file and no moment exists
//! at which a secret is group- or world-readable. A file is rewritten only when its content (or
//! its mode) differs. Directories are opened without following symbolic links, so a link planted
//! under the state directory is refused. A path component is a validated bot UUID or session name,
//! never text from a task. Tokens are never logged: the types that hold them redact `Debug`.

use std::fs::File;
use std::os::fd::{AsFd, OwnedFd};
use std::os::unix::fs::DirBuilderExt;
use std::path::{Path, PathBuf};

use nix::errno::Errno;
use nix::sys::stat::{Mode, fstat, mkdirat};
use serde_json::json;

use super::guide::{Entry, Read_, classify, create_file, open_dir, open_root, read_regular, replace_file};
use super::launch::{LaunchError, LaunchFiles, SERVER_NAME, claude_hook_command};
use super::tmux::{NameSource, SessionName, TmuxName};

/// The most bytes of one of these files that are read back.
const MAX_FILE_BYTES: u64 = 64 * 1024;

/// The events the claude hooks report.
pub const CLAUDE_EVENTS: [&str; 4] = ["SessionStart", "UserPromptSubmit", "Stop", "Notification"];

/// The `hooks` object of a claude settings file: one entry per event, each running the `hook`
/// subcommand. With `marked`, each hook object carries `"sylvode-runner": 1`, so a settings file the
/// operator shares can be updated without touching hooks that are not the runner's.
///
/// # Errors
///
/// A message when a path cannot be used in a command.
pub fn claude_hooks(
    files: &LaunchFiles<'_>,
    marked: bool,
) -> Result<serde_json::Map<String, serde_json::Value>, String> {
    let mut hooks = serde_json::Map::new();
    for event in CLAUDE_EVENTS {
        let command = claude_hook_command(files, event).map_err(|e: LaunchError| e.to_string())?;
        // `SessionStart` also fires after compaction, `/clear` and resume, and `Notification`
        // also for an idle prompt: only the first start and the permission prompts are wanted.
        let matcher = match event {
            "SessionStart" => Some("startup"),
            "Notification" => Some("permission_prompt|elicitation_dialog"),
            _ => None,
        };
        let mut hook = json!({"type": "command", "command": command});
        if let (true, Some(object)) = (marked, hook.as_object_mut()) {
            object.insert(RUNNER_MARK.to_string(), json!(1));
        }
        let mut entry = json!({"hooks": [hook]});
        if let (Some(matcher), Some(object)) = (matcher, entry.as_object_mut()) {
            object.insert("matcher".to_string(), json!(matcher));
        }
        hooks.insert(event.to_string(), json!([entry]));
    }
    Ok(hooks)
}

/// The `sylvode` entry of an MCP configuration: the MCP server started on the bot's TOML. It
/// names the file, never the token.
pub fn mcp_server_entry(server: &str, toml_path: &str) -> serde_json::Value {
    json!({
        "type": "stdio",
        "command": server,
        "args": ["serve", "--transport", "stdio", "--config", toml_path],
    })
}

/// The key that marks a hook object as the runner's own in a shared settings file.
pub const RUNNER_MARK: &str = "sylvode-runner";

const fn private_mode() -> Mode {
    Mode::from_bits_truncate(0o700)
}

/// What the per-bot MCP configuration needs. `Debug` hides the token.
pub struct BotFiles<'a> {
    pub bot_id: &'a str,
    pub api_url: &'a str,
    /// The workspace the bot belongs to; the MCP server refuses to start without it.
    pub workspace_id: &'a str,
    pub token: &'a str,
    pub tool_mode: &'a str,
    pub allow_write_call: bool,
}

impl std::fmt::Debug for BotFiles<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("BotFiles")
            .field("bot_id", &self.bot_id)
            .field("api_url", &self.api_url)
            .field("workspace_id", &self.workspace_id)
            .field("token", &"<redacted>")
            .field("tool_mode", &self.tool_mode)
            .field("allow_write_call", &self.allow_write_call)
            .finish()
    }
}

/// Who started a session: the runner (and the recorded pane id and PID still match), or the
/// operator.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CreatedBy {
    Runner,
    Operator,
}

impl CreatedBy {
    pub const fn word(self) -> &'static str {
        match self {
            Self::Runner => "runner",
            Self::Operator => "operator",
        }
    }
}

/// What the runner knows about a session it uses: which project and bot, which pane, which CLI.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct SessionRecord {
    /// `<KEY>-<cli>`.
    pub name: String,
    pub project_id: String,
    /// The bot of the connection that serves the session.
    pub bot_id: String,
    pub cli: String,
    pub pane_id: String,
    pub pane_pid: u32,
    /// The working directory the task that last used the session named (for `doctor`).
    pub dir: String,
    pub created_by: CreatedBy,
    /// The task whose text was typed (or was about to be) into this session. Written and synced
    /// BEFORE the paste, consulted by restart recovery only to prevent a second paste.
    #[serde(default)]
    pub pushed: Option<PushedTask>,
    /// The tmux session the CLI runs in; empty in a record written before the name could differ
    /// from `name` (then it is the derived name).
    #[serde(default)]
    pub tmux_name: String,
    /// `local`, `server` or `derived`; empty means `derived`.
    #[serde(default)]
    pub name_source: String,
}

impl SessionRecord {
    /// The where-from of the tmux name.
    pub fn source(&self) -> NameSource {
        NameSource::from_record(&self.name_source)
    }

    /// The tmux session this record says the CLI runs in: the derived name for a derived (or
    /// old) record, else the recorded name. `None` when it cannot be read.
    pub fn tmux(&self) -> Option<TmuxName> {
        match self.source() {
            NameSource::Derived => SessionName::parse(&self.name).ok().map(|slot| TmuxName::derived(&slot)),
            NameSource::Local | NameSource::Server => TmuxName::new(&self.tmux_name).ok(),
        }
    }
}

/// A task typed into a session.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct PushedTask {
    pub task_id: String,
    pub attempt: i64,
}

/// What was written.
#[derive(Debug, PartialEq, Eq)]
pub struct Written {
    pub changed: bool,
}

/// The state directory.
#[derive(Debug)]
pub struct StateDir {
    root: PathBuf,
    fd: OwnedFd,
}

fn valid_bot_id(bot_id: &str) -> bool {
    uuid::Uuid::parse_str(bot_id).is_ok_and(|id| id.hyphenated().to_string() == bot_id.to_ascii_lowercase())
}

/// Opens (and when missing creates, with mode 0700) a directory below `parent`, never through a
/// link, and refuses one that group or others can reach.
fn private_dir(parent: &impl AsFd, name: &str) -> Result<OwnedFd, String> {
    match classify(parent, name)? {
        Entry::Absent => {
            mkdirat(parent, name, private_mode()).map_err(|e| format!("could not create `{name}`: {e}"))?;
        }
        Entry::Dir => {}
        Entry::File | Entry::Other => return Err(format!("`{name}` in the state directory is not a plain directory")),
    }
    let fd = open_dir(parent, name)?;
    check_private(&fd, name)?;
    Ok(fd)
}

fn check_private(fd: &OwnedFd, what: &str) -> Result<(), String> {
    let stat = fstat(fd).map_err(|e| format!("could not inspect `{what}`: {e}"))?;
    // `st_mode` is `u16` on macOS and `u32` on Linux; `u32::from` is lossless on both.
    #[allow(clippy::useless_conversion)]
    let mode = u32::from(stat.st_mode) & 0o777;
    if mode & 0o077 != 0 {
        return Err(format!(
            "`{what}` is accessible to group or others (mode {mode:o}); it holds secrets, so run chmod 700 on it"
        ));
    }
    Ok(())
}

impl StateDir {
    /// Opens the state directory, creating it (and missing parents) with mode 0700.
    ///
    /// # Errors
    ///
    /// A message when it cannot be created or opened, or is reachable by group or others.
    pub fn open(root: &Path) -> Result<Self, String> {
        std::fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(root)
            .map_err(|e| format!("could not create the state directory: {}", e.kind()))?;
        let fd = open_root(root)?;
        check_private(&fd, "the state directory")?;
        Ok(Self {
            root: root.to_path_buf(),
            fd,
        })
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn hook_socket(&self) -> PathBuf {
        self.root.join("hook.sock")
    }

    pub fn mcp_toml(&self, bot_id: &str) -> PathBuf {
        self.root.join("mcp").join(format!("{bot_id}.toml"))
    }

    pub fn claude_mcp_json(&self, bot_id: &str) -> PathBuf {
        self.root.join("mcp").join(format!("{bot_id}.claude.json"))
    }

    pub fn claude_settings(&self, name: &SessionName) -> PathBuf {
        self.root
            .join("sessions")
            .join(name.as_str())
            .join("claude-settings.json")
    }

    /// Takes the exclusive lock of this state directory. Hold the result for the life of the
    /// process; a second runner on the same directory gets an error.
    ///
    /// # Errors
    ///
    /// A message when the lock is held or cannot be taken.
    pub fn lock(&self) -> Result<nix::fcntl::Flock<File>, String> {
        let path = self.root.join("runner.lock");
        let file = std::fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .mode_0600()
            .open(&path)
            .map_err(|e| format!("could not open runner.lock: {}", e.kind()))?;
        nix::fcntl::Flock::lock(file, nix::fcntl::FlockArg::LockExclusiveNonblock).map_err(|(_, errno)| {
            if errno == Errno::EWOULDBLOCK {
                "another runner is using this state directory (runner.lock is held)".to_string()
            } else {
                format!("could not lock runner.lock: {errno}")
            }
        })
    }

    /// Writes the per-bot files: `mcp/<bot_id>.toml` (the token) and `mcp/<bot_id>.claude.json`
    /// (which references the TOML and holds no token).
    ///
    /// # Errors
    ///
    /// A message (never the token).
    pub fn write_bot_files(&self, bot: &BotFiles<'_>, mcp_server: &Path) -> Result<Written, String> {
        if !valid_bot_id(bot.bot_id) {
            return Err("the bot id is not a UUID".to_string());
        }
        if !bot.token.starts_with("opr_") || bot.token.chars().any(|c| c.is_control() || c.is_whitespace()) {
            return Err("the bot token has an unexpected shape".to_string());
        }
        if !uuid::Uuid::parse_str(bot.workspace_id).is_ok_and(|id| !id.is_nil()) {
            return Err("the workspace id is not a UUID".to_string());
        }
        let toml = format!(
            "# Written by sylvode-runner; rewritten when it differs. Holds the bot's token: keep mode 0600.\n\
             [mcp]\napi_url = {}\nworkspace_id = {}\nbot_token = {}\ntool_mode = {}\nallow_write_call = {}\n",
            super::launch::toml_string(bot.api_url),
            super::launch::toml_string(bot.workspace_id),
            super::launch::toml_string(bot.token),
            super::launch::toml_string(bot.tool_mode),
            bot.allow_write_call
        );
        let toml_path = self.mcp_toml(bot.bot_id);
        let server = mcp_server
            .to_str()
            .ok_or_else(|| "the MCP server path is not UTF-8".to_string())?;
        let toml_text = toml_path
            .to_str()
            .ok_or_else(|| "the MCP config path is not UTF-8".to_string())?;
        let json = serde_json::to_string_pretty(&json!({
            "mcpServers": { SERVER_NAME: mcp_server_entry(server, toml_text) }
        }))
        .map_err(|e| e.to_string())?;
        let mcp = private_dir(&self.fd, "mcp")?;
        let first = write_private(&mcp, &format!("{}.toml", bot.bot_id), toml.as_bytes())?;
        let second = write_private(
            &mcp,
            &format!("{}.claude.json", bot.bot_id),
            format!("{json}\n").as_bytes(),
        )?;
        Ok(Written {
            changed: first || second,
        })
    }

    /// Writes `sessions/<NAME>/claude-settings.json`: the four hooks, each the `hook` subcommand.
    /// The file holds no secret.
    ///
    /// # Errors
    ///
    /// A message when a path cannot be used or a file cannot be written.
    pub fn write_session_settings(&self, name: &SessionName, files: &LaunchFiles<'_>) -> Result<Written, String> {
        let hooks = claude_hooks(files, false)?;
        let text = serde_json::to_string_pretty(&json!({"hooks": hooks})).map_err(|e| e.to_string())?;
        let sessions = private_dir(&self.fd, "sessions")?;
        let dir = private_dir(&sessions, name.as_str())?;
        write_private(&dir, "claude-settings.json", format!("{text}\n").as_bytes()).map(|changed| Written { changed })
    }

    /// Writes `sessions/<NAME>/session.json`: what the runner knows about a session it uses.
    ///
    /// # Errors
    ///
    /// A message when it cannot be written.
    pub fn write_session_record(&self, record: &SessionRecord) -> Result<(), String> {
        let name = SessionName::parse(&record.name).map_err(|e| e.to_string())?;
        let text = serde_json::to_string_pretty(record).map_err(|e| e.to_string())?;
        let sessions = private_dir(&self.fd, "sessions")?;
        let dir = private_dir(&sessions, name.as_str())?;
        write_private(&dir, "session.json", format!("{text}\n").as_bytes()).map(|_| ())
    }

    /// Reads `sessions/<NAME>/session.json`; `None` when there is none or it is not ours (a link, a
    /// special file, unreadable or not the expected shape).
    pub fn read_session_record(&self, name: &SessionName) -> Option<SessionRecord> {
        let sessions = open_dir(&self.fd, "sessions").ok()?;
        let dir = open_dir(&sessions, name.as_str()).ok()?;
        match read_regular(&dir, "session.json", MAX_FILE_BYTES).ok()? {
            Read_::Data { bytes, .. } => serde_json::from_slice(&bytes).ok(),
            _ => None,
        }
    }

    /// Records, durably, that `task_id` at `attempt` is about to be typed into session `name`.
    ///
    /// # Errors
    ///
    /// A message when there is no `session.json` or it cannot be written; the caller must then not
    /// paste.
    pub fn mark_pushed(&self, name: &SessionName, pushed: Option<PushedTask>) -> Result<(), String> {
        let record = self
            .read_session_record(name)
            .ok_or_else(|| "the session has no session.json".to_string())?;
        if record.pushed == pushed {
            return Ok(());
        }
        self.write_session_record(&SessionRecord { pushed, ..record })
    }

    /// The names that have a `session.json`, for the `sessions` command.
    pub fn recorded_names(&self) -> Vec<String> {
        let Ok(sessions) = open_dir(&self.fd, "sessions") else {
            return Vec::new();
        };
        let mut names: Vec<String> = std::fs::read_dir(self.root.join("sessions"))
            .into_iter()
            .flatten()
            .filter_map(Result::ok)
            .filter_map(|entry| entry.file_name().into_string().ok())
            .filter(|name| SessionName::parse(name).is_ok())
            .filter(|name| {
                open_dir(&sessions, name).ok().is_some_and(|dir| {
                    matches!(
                        read_regular(&dir, "session.json", MAX_FILE_BYTES),
                        Ok(Read_::Data { .. })
                    )
                })
            })
            .collect();
        names.sort();
        names
    }

    /// Modes that are wider than allowed, for `doctor`: the state directory, its subdirectories
    /// and every file in `mcp/` and `sessions/*/`.
    pub fn wide_modes(&self) -> Vec<String> {
        let mut found = Vec::new();
        let mut check = |path: &Path, limit: u32| {
            use std::os::unix::fs::PermissionsExt;
            if let Ok(meta) = std::fs::symlink_metadata(path)
                && meta.permissions().mode() & 0o777 & !limit != 0
            {
                found.push(format!(
                    "{} has mode {:o}",
                    path.display(),
                    meta.permissions().mode() & 0o777
                ));
            }
        };
        check(&self.root, 0o700);
        for sub in ["mcp", "sessions", "panes"] {
            let dir = self.root.join(sub);
            check(&dir, 0o700);
            for (path, limit) in entries_below(&dir) {
                check(&path, limit);
            }
        }
        found
    }
}

/// The entries of `dir` and of its subdirectories, with the widest mode each may have.
fn entries_below(dir: &Path) -> Vec<(PathBuf, u32)> {
    let mut out = Vec::new();
    let Ok(read) = std::fs::read_dir(dir) else { return out };
    for entry in read.filter_map(Result::ok) {
        let path = entry.path();
        if path.is_dir() {
            out.push((path.clone(), 0o700));
            out.extend(
                std::fs::read_dir(&path)
                    .into_iter()
                    .flatten()
                    .filter_map(Result::ok)
                    .map(|file| (file.path(), 0o600)),
            );
        } else {
            out.push((path, 0o600));
        }
    }
    out
}

/// Writes `content` to `name` in `dir` with mode 0600 only when the file differs (or has a wider
/// mode); returns whether it wrote.
fn write_private(dir: &OwnedFd, name: &str, content: &[u8]) -> Result<bool, String> {
    match read_regular(dir, name, MAX_FILE_BYTES)? {
        Read_::Data { bytes, mode } if bytes == content && mode.trailing_zeros() >= 6 => return Ok(false),
        Read_::Absent => create_file(dir, name, content, 0o600).or_else(|e| {
            // A file that appeared meanwhile is replaced like any other.
            replace_file(dir, name, content, 0o600).map_err(|_| e)
        })?,
        // Different content, a wider mode, a link or a special file: replaced through a temporary
        // file and a rename, which replaces a link itself rather than writing through it.
        _ => replace_file(dir, name, content, 0o600)?,
    }
    Ok(true)
}

trait OpenOptionsMode {
    fn mode_0600(&mut self) -> &mut Self;
}

impl OpenOptionsMode for std::fs::OpenOptions {
    fn mode_0600(&mut self) -> &mut Self {
        use std::os::unix::fs::OpenOptionsExt;
        self.mode(0o600)
    }
}
