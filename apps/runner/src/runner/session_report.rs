//! What `doctor` and the periodic report say about the sessions and the posture (design 6.6 and
//! 5.3): per tmux session its state, who started it, whether hooks were seen, whether the CLI
//! runs with permission checks off and whether it has the Sylvode MCP server; and per channel
//! whether the configured launch settings turn permission checks off.
//!
//! [`assess`] is a pure function over an [`Observation`]; [`observe`] gathers one with read-only
//! calls (tmux queries, one `ps`, the counter file of the pane, and `claude mcp get` where the
//! CLI's arguments do not say). Nothing here types into, starts or ends anything.

use std::path::Path;
use std::process::{Command, Stdio};
use std::time::{Duration, SystemTime};

use sylvode_prereq::Status;

use super::config::RunnerConfig;
use super::detect::classify_bottom;
use super::files::{CreatedBy, SessionRecord, StateDir};
use super::session::{EnsureDecision, EnsureFacts, decide_ensure};
use super::tmux::{ListedPane, Pane, SessionName, Tmux};
use super::words_bypass_permissions;

/// The footer claude shows while permission checks are off.
const BYPASS_FOOTER: &str = "bypass permissions on";

/// `state` of a session row.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SessionState {
    Running,
    Absent,
    CliNotRunning,
    Ambiguous,
    Dialog,
}

impl SessionState {
    pub const fn word(self) -> &'static str {
        match self {
            Self::Running => "running",
            Self::Absent => "absent",
            Self::CliNotRunning => "cli_not_running",
            Self::Ambiguous => "ambiguous",
            Self::Dialog => "dialog",
        }
    }
}

/// `permissions` of a session row.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Permissions {
    Bypass,
    Normal,
    Unknown,
}

impl Permissions {
    pub const fn word(self) -> &'static str {
        match self {
            Self::Bypass => "bypass",
            Self::Normal => "normal",
            Self::Unknown => "unknown",
        }
    }
}

/// `mcp` of a session row.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mcp {
    Ok,
    Missing,
    Other,
    Stale,
}

impl Mcp {
    pub const fn word(self) -> &'static str {
        match self {
            Self::Ok => "ok",
            Self::Missing => "missing",
            Self::Other => "other",
            Self::Stale => "stale",
        }
    }
}

/// What `claude mcp get sylvode` said in the CLI's directory.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum McpEntry {
    Absent,
    /// Starts the MCP server on this bot's file.
    Ours,
    /// Points at another file.
    Elsewhere,
}

/// What was seen of one session.
#[derive(Clone, Debug)]
pub struct Observation {
    pub name: SessionName,
    pub exists: bool,
    pub panes: Vec<ListedPane>,
    pub process_names: Vec<String>,
    pub record: Option<SessionRecord>,
    /// The screen of the CLI's pane.
    pub screen: Option<String>,
    /// The arguments of the CLI's process.
    pub cli_args: Option<String>,
    /// The arguments carry `--mcp-config` with this bot's file.
    pub mcp_in_args: bool,
    /// `claude mcp get sylvode`, asked only when the arguments do not say.
    pub mcp_entry: Option<McpEntry>,
    /// The bot's credentials were rewritten after the CLI started (it keeps its old MCP process).
    pub credentials_newer: Option<bool>,
    /// When a hook of the pane last ran.
    pub hooks_seen: Option<SystemTime>,
    pub foreground: Option<String>,
    pub pane_id: Option<String>,
}

/// What `doctor` and the report show about one session.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Assessment {
    pub name: SessionName,
    /// The bot of the connection that serves the session, as recorded (empty when unknown).
    pub bot_id: String,
    pub state: SessionState,
    pub created_by: Option<CreatedBy>,
    pub hooks_seen: Option<SystemTime>,
    /// `Some` only for a session whose CLI runs.
    pub hooks: Option<bool>,
    pub permissions: Option<Permissions>,
    pub mcp: Option<Mcp>,
    pub pane_id: Option<String>,
    pub foreground: Option<String>,
    pub status: Status,
}

/// Whether the CLI runs with permission checks off: its arguments say so, or (secondary) its
/// footer does. Without arguments and without the footer nothing is known.
#[must_use]
pub fn permissions_of(cli_args: Option<&str>, screen: Option<&str>) -> Permissions {
    let footer = screen.is_some_and(|screen| screen.to_lowercase().contains(BYPASS_FOOTER));
    match cli_args {
        Some(args) => {
            let words: Vec<String> = args.split_whitespace().map(str::to_string).collect();
            if words_bypass_permissions(&words) || footer {
                Permissions::Bypass
            } else {
                Permissions::Normal
            }
        }
        None if footer => Permissions::Bypass,
        None => Permissions::Unknown,
    }
}

/// Reads an observation into the row.
#[must_use]
pub fn assess(obs: &Observation) -> Assessment {
    let created_by_record = obs.record.as_ref().map(|record| record.created_by);
    let mut row = Assessment {
        name: obs.name.clone(),
        bot_id: obs
            .record
            .as_ref()
            .map(|record| record.bot_id.clone())
            .unwrap_or_default(),
        state: SessionState::Absent,
        created_by: created_by_record,
        hooks_seen: None,
        hooks: None,
        permissions: None,
        mcp: None,
        pane_id: obs.pane_id.clone(),
        foreground: obs.foreground.clone(),
        status: Status::Ok,
    };
    if !obs.exists {
        // The session starts with the next task.
        return row;
    }
    let bot = obs.record.as_ref().map_or("", |record| record.bot_id.as_str());
    let decision = decide_ensure(&EnsureFacts {
        session_exists: true,
        panes: &obs.panes,
        process_names: &obs.process_names,
        record: obs.record.as_ref(),
        task_bot_id: bot,
    });
    match decision {
        EnsureDecision::NotRunning => {
            row.state = SessionState::CliNotRunning;
            row.status = Status::Fail;
            return row;
        }
        EnsureDecision::Ambiguous => {
            row.state = SessionState::Ambiguous;
            row.status = Status::Fail;
            return row;
        }
        EnsureDecision::Create | EnsureDecision::BotMismatch => return row,
        EnsureDecision::Use { pane, created_by } => {
            row.created_by = Some(created_by);
            row.pane_id = Some(pane.id.as_str().to_string());
            row.foreground = Some(pane.command);
        }
    }
    let dialog = obs
        .screen
        .as_deref()
        .is_some_and(|screen| classify_bottom("claude", screen).is_some());
    row.state = if dialog {
        SessionState::Dialog
    } else {
        SessionState::Running
    };
    row.hooks_seen = obs.hooks_seen;
    row.hooks = Some(obs.hooks_seen.is_some());
    row.permissions = Some(permissions_of(obs.cli_args.as_deref(), obs.screen.as_deref()));
    let mcp = if obs.mcp_in_args {
        Some(Mcp::Ok)
    } else {
        obs.mcp_entry.map(|entry| match entry {
            McpEntry::Absent => Mcp::Missing,
            McpEntry::Ours => Mcp::Ok,
            McpEntry::Elsewhere => Mcp::Other,
        })
    };
    row.mcp = match (mcp, obs.credentials_newer) {
        (Some(Mcp::Ok), Some(true)) => Some(Mcp::Stale),
        (other, _) => other,
    };
    let failing = matches!(row.mcp, Some(Mcp::Missing | Mcp::Other));
    let warning = dialog
        || row.permissions == Some(Permissions::Bypass)
        || row.hooks == Some(false)
        || row.mcp == Some(Mcp::Stale);
    row.status = if failing {
        Status::Fail
    } else if warning {
        Status::Warn
    } else {
        Status::Ok
    };
    row
}

/// One row of `ps -A -o pid=,ppid=,etime=,args=`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProcRow {
    pub pid: u32,
    pub ppid: u32,
    pub elapsed: Duration,
    pub args: String,
}

/// `[[dd-]hh:]mm:ss` as a duration.
#[must_use]
pub fn parse_etime(text: &str) -> Option<Duration> {
    let (days, rest) = text
        .split_once('-')
        .map_or((0, text), |(d, r)| (d.parse().unwrap_or(0), r));
    let mut seconds: u64 = 0;
    let parts: Vec<&str> = rest.split(':').collect();
    if parts.is_empty() || parts.len() > 3 {
        return None;
    }
    for part in parts {
        seconds = seconds * 60 + part.parse::<u64>().ok()?;
    }
    Some(Duration::from_secs(seconds + days * 86_400))
}

/// Parses the output of the `ps` call; a line that does not parse is skipped.
#[must_use]
pub fn parse_process_rows(text: &str) -> Vec<ProcRow> {
    text.lines()
        .filter_map(|line| {
            let mut fields = line.split_whitespace();
            let id = fields.next()?.parse().ok()?;
            let parent = fields.next()?.parse().ok()?;
            let elapsed = parse_etime(fields.next()?)?;
            let args = fields.collect::<Vec<_>>().join(" ");
            Some(ProcRow {
                pid: id,
                ppid: parent,
                elapsed,
                args,
            })
        })
        .collect()
}

fn process_table() -> Vec<ProcRow> {
    let output = Command::new("/bin/ps")
        .env_clear()
        .args(["-A", "-o", "pid=", "-o", "ppid=", "-o", "etime=", "-o", "args="])
        .stdin(Stdio::null())
        .output();
    output
        .map(|out| parse_process_rows(&String::from_utf8_lossy(&out.stdout)))
        .unwrap_or_default()
}

/// The process of the CLI: `root` or one of its descendants whose program is one of `names`.
#[must_use]
pub fn cli_process<'a>(table: &'a [ProcRow], root: u32, names: &[String]) -> Option<&'a ProcRow> {
    let mut family = vec![root];
    let mut grew = true;
    while grew {
        grew = false;
        for row in table {
            if family.contains(&row.ppid) && !family.contains(&row.pid) {
                family.push(row.pid);
                grew = true;
            }
        }
    }
    table.iter().find(|row| {
        family.contains(&row.pid)
            && row
                .args
                .split_whitespace()
                .next()
                .and_then(|program| Path::new(program).file_name())
                .and_then(|file| file.to_str())
                .is_some_and(|program| names.iter().any(|name| name == program))
    })
}

/// `claude mcp get sylvode` in `dir`: absent, this bot's file, or another.
fn mcp_entry(claude: &Path, dir: &Path, toml: &Path) -> Option<McpEntry> {
    let output = Command::new(claude)
        .args(["mcp", "get", super::launch::SERVER_NAME])
        .current_dir(dir)
        .env_clear()
        .envs(super::tmux::agent_environment())
        .stdin(Stdio::null())
        .output()
        .ok()?;
    if !output.status.success() {
        return Some(McpEntry::Absent);
    }
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    Some(if toml.to_str().is_some_and(|path| text.contains(path)) {
        McpEntry::Ours
    } else {
        McpEntry::Elsewhere
    })
}

/// The state directory, only when it exists (a report must not create it).
#[must_use]
pub fn open_state(config: &crate::config::Config) -> Option<(StateDir, std::path::PathBuf)> {
    let path = super::state_dir(config, super::paths::service_user_home().as_deref())?;
    if !path.is_dir() {
        return None;
    }
    StateDir::open(&path).ok().map(|state| (state, path))
}

/// Looks at one session with read-only calls.
#[must_use]
pub fn observe(
    config: &crate::config::Config,
    tmux: &Tmux,
    state: &StateDir,
    state_path: &Path,
    claude: Option<&Path>,
    name: &SessionName,
) -> Observation {
    let process_names = config.runner.cli.claude.process_names.clone();
    let record = state.read_session_record(name);
    let exists = tmux.has_session(name).unwrap_or(false);
    let panes = if exists {
        tmux.list_panes(name).unwrap_or_default()
    } else {
        Vec::new()
    };
    let mut obs = Observation {
        name: name.clone(),
        exists,
        panes,
        process_names,
        record: record.clone(),
        screen: None,
        cli_args: None,
        mcp_in_args: false,
        mcp_entry: None,
        credentials_newer: None,
        hooks_seen: None,
        foreground: None,
        pane_id: None,
    };
    if !exists {
        return obs;
    }
    let bot = record.as_ref().map_or("", |record| record.bot_id.as_str());
    let chosen = decide_ensure(&EnsureFacts {
        session_exists: true,
        panes: &obs.panes,
        process_names: &obs.process_names,
        record: record.as_ref(),
        task_bot_id: bot,
    });
    let EnsureDecision::Use { pane: listed, .. } = chosen else {
        return obs;
    };
    let pane = Pane {
        name: name.clone(),
        id: listed.id.clone(),
        pid: listed.pid,
    };
    obs.screen = tmux.capture_pane(&pane, None).ok();
    // The hooks of this pane: its counter file, named by the server's PID and the pane number.
    obs.hooks_seen = tmux
        .server_pid(&pane)
        .ok()
        .map(|pid| super::hooks::seq_file(state_path, pid, pane.id.as_str()))
        .and_then(|file| std::fs::metadata(file).ok())
        .and_then(|meta| meta.modified().ok());
    let table = process_table();
    let process = cli_process(&table, listed.pid, &obs.process_names);
    obs.cli_args = process.map(|row| row.args.clone());
    let (mcp_json, mcp_toml) = (
        state_path.join("mcp").join(format!("{bot}.claude.json")),
        state_path.join("mcp").join(format!("{bot}.toml")),
    );
    obs.mcp_in_args = !bot.is_empty()
        && mcp_json
            .to_str()
            .zip(obs.cli_args.as_deref())
            .is_some_and(|(path, args)| args.contains(path));
    if !obs.mcp_in_args
        && !bot.is_empty()
        && let (Some(claude), Ok(info)) = (claude, tmux.pane_info(&pane))
    {
        obs.mcp_entry = mcp_entry(claude, Path::new(&info.current_path), &mcp_toml);
    }
    obs.credentials_newer = process
        .and_then(|row| SystemTime::now().checked_sub(row.elapsed))
        .zip(std::fs::metadata(&mcp_toml).ok().and_then(|meta| meta.modified().ok()))
        .map(|(started, written)| written > started + Duration::from_secs(2));
    obs.foreground = Some(listed.command);
    obs.pane_id = Some(listed.id.as_str().to_string());
    obs
}

/// What looking at sessions needs: tmux, the state directory and the claude program.
struct Context {
    tmux: Tmux,
    state: StateDir,
    path: std::path::PathBuf,
    claude: Option<std::path::PathBuf>,
}

fn context(config: &crate::config::Config) -> Option<Context> {
    use sylvode_prereq::{Checker, InstallKind};
    let (state, path) = open_state(config)?;
    let checker = Checker::new(InstallKind::RunnerTmux);
    let program = checker.locate(&config.runner.tmux.binary)?;
    let socket = super::tmux::SocketName::new(&config.runner.tmux.socket_name).ok()?;
    Some(Context {
        tmux: Tmux::new(socket).with_program(program),
        state,
        path,
        claude: checker.locate(&config.runner.cli.claude.binary),
    })
}

/// The assessments of every session the runner has a record of.
#[must_use]
pub fn assess_recorded(config: &crate::config::Config) -> Vec<Assessment> {
    let Some(context) = context(config) else {
        return Vec::new();
    };
    context
        .state
        .recorded_names()
        .iter()
        .filter_map(|name| SessionName::parse(name).ok())
        .map(|name| {
            let obs = observe(
                config,
                &context.tmux,
                &context.state,
                &context.path,
                context.claude.as_deref(),
                &name,
            );
            assess(&obs)
        })
        .collect()
}

/// The assessment of one session, `None` when there is no tmux or state directory to look in.
#[must_use]
pub fn assess_named(config: &crate::config::Config, name: &SessionName) -> Option<Assessment> {
    let context = context(config)?;
    let obs = observe(
        config,
        &context.tmux,
        &context.state,
        &context.path,
        context.claude.as_deref(),
        name,
    );
    Some(assess(&obs))
}

/// Whether the configured launch settings turn permission checks off, per channel.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Posture {
    pub tmux: bool,
    pub headless: bool,
}

/// The codex flag that turns its approvals and sandbox off.
const CODEX_BYPASS: &str = "--dangerously-bypass-approvals-and-sandbox";

/// The posture of the configured commands.
#[must_use]
pub fn posture_of(runner: &RunnerConfig) -> Posture {
    let claude = &runner.cli.claude;
    let codex_bypass = runner.cli.codex.headless_args.iter().any(|word| word == CODEX_BYPASS);
    Posture {
        tmux: words_bypass_permissions(&claude.launch_command),
        headless: words_bypass_permissions(&claude.headless_args) || (runner.cli.codex.enabled && codex_bypass),
    }
}
