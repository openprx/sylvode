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
use super::profile::{McpProbe, profile};
use super::session::{EnsureDecision, EnsureFacts, decide_ensure};
use super::tmux::{ListedPane, NameSource, Pane, Slot, Tmux, TmuxName};
use super::words_bypass_permissions;

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
    /// The slot, `<KEY>-<cli>` or `bot-<uuid>-<cli>`.
    pub name: Slot,
    /// The tmux session looked at, and where its name came from.
    pub tmux: TmuxName,
    pub source: NameSource,
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
    pub name: Slot,
    /// The tmux session looked at; shown by `doctor` and `sessions`, never reported to Sylvode.
    pub tmux: TmuxName,
    pub source: NameSource,
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

/// Whether the CLI `cli` runs with permission checks off: its arguments say so, or (secondary)
/// its footer or banner does. Without arguments and without the footer nothing is known.
#[must_use]
pub fn permissions_for(cli: &str, cli_args: Option<&str>, screen: Option<&str>) -> Permissions {
    let footer = profile(cli)
        .and_then(|p| p.footer_bypass)
        .zip(screen)
        .is_some_and(|(text, screen)| screen.to_lowercase().contains(text));
    match cli_args {
        Some(args) => {
            let words: Vec<String> = args.split_whitespace().map(str::to_string).collect();
            match cli {
                "codex" => match codex_permissions(&words) {
                    Permissions::Unknown | Permissions::Normal if footer => Permissions::Bypass,
                    other => other,
                },
                _ => {
                    if words_bypass_permissions(&words) || footer {
                        Permissions::Bypass
                    } else {
                        Permissions::Normal
                    }
                }
            }
        }
        None if footer => Permissions::Bypass,
        None => Permissions::Unknown,
    }
}

/// What codex's launch words say about approvals and the sandbox: the bypass flag, or approval
/// `never` with sandbox `danger-full-access`, is `Bypass`; a sandbox `read-only` or
/// `workspace-write`, or approval `on-request`, is `Normal`; anything else (the user's own
/// configuration decides) is `Unknown`. Reads `-a v`, `--ask-for-approval v` and `--x=v`, the same
/// for `-s`/`--sandbox`, and the `-c approval_policy=v` / `-c sandbox_mode=v` forms.
#[must_use]
pub fn codex_permissions(words: &[String]) -> Permissions {
    if words.iter().any(|word| word == CODEX_BYPASS) {
        return Permissions::Bypass;
    }
    let value_of = |names: &[&str], config_key: &str| -> Option<String> {
        let unquote = |text: &str| text.trim_matches(|c| c == '"' || c == '\'').to_string();
        words.iter().enumerate().find_map(|(at, word)| {
            if let Some(name) = names.iter().find(|name| *word == **name) {
                let _ = name;
                return words.get(at + 1).map(|next| unquote(next));
            }
            if let Some(value) = names
                .iter()
                .filter(|name| name.starts_with("--"))
                .find_map(|name| word.strip_prefix(&format!("{name}=")))
            {
                return Some(unquote(value));
            }
            let config = if word == "-c" || word == "--config" {
                words.get(at + 1).map(String::as_str)
            } else {
                None
            };
            config
                .and_then(|text| text.strip_prefix(&format!("{config_key}=")))
                .map(unquote)
        })
    };
    let approval = value_of(&["-a", "--ask-for-approval"], "approval_policy");
    let sandbox = value_of(&["-s", "--sandbox"], "sandbox_mode");
    match (approval.as_deref(), sandbox.as_deref()) {
        (Some("never"), Some("danger-full-access")) => Permissions::Bypass,
        (_, Some("read-only" | "workspace-write")) | (Some("on-request"), _) => Permissions::Normal,
        _ => Permissions::Unknown,
    }
}

/// Reads an observation into the row.
#[must_use]
pub fn assess(obs: &Observation) -> Assessment {
    let created_by_record = obs.record.as_ref().map(|record| record.created_by);
    let mut row = Assessment {
        name: obs.name.clone(),
        tmux: obs.tmux.clone(),
        source: obs.source,
        // A bot slot is its bot's, with or without a record.
        bot_id: obs.name.bot_id().map_or_else(
            || {
                obs.record
                    .as_ref()
                    .map(|record| record.bot_id.clone())
                    .unwrap_or_default()
            },
            str::to_string,
        ),
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
        // A derived session starts with the next task; a configured one is never created, so its
        // tasks fail until the operator starts it.
        if obs.source != NameSource::Derived {
            row.status = Status::Warn;
        }
        return row;
    }
    let bot = obs.record.as_ref().map_or("", |record| record.bot_id.as_str());
    let decision = decide_ensure(&EnsureFacts {
        session_exists: true,
        panes: &obs.panes,
        process_names: &obs.process_names,
        allow_create: obs.source == NameSource::Derived,
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
        EnsureDecision::Create | EnsureDecision::Absent | EnsureDecision::BotMismatch => return row,
        EnsureDecision::Use { pane, created_by } => {
            row.created_by = Some(created_by);
            row.pane_id = Some(pane.id.as_str().to_string());
            row.foreground = Some(pane.command);
        }
    }
    let dialog = obs
        .screen
        .as_deref()
        .is_some_and(|screen| classify_bottom(obs.name.cli(), screen).is_some());
    row.state = if dialog {
        SessionState::Dialog
    } else {
        SessionState::Running
    };
    row.hooks_seen = obs.hooks_seen;
    row.hooks = Some(obs.hooks_seen.is_some());
    row.permissions = Some(permissions_for(
        obs.name.cli(),
        obs.cli_args.as_deref(),
        obs.screen.as_deref(),
    ));
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
    // A session with a configured name has its own `sylvode` entry; it must hold this bot's token,
    // which the runner cannot see: a warning, not a failure.
    let own_entry = row.mcp == Some(Mcp::Other) && obs.source != NameSource::Derived;
    let failing = matches!(row.mcp, Some(Mcp::Missing | Mcp::Other)) && !own_entry;
    let warning = own_entry
        || dialog
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

/// The CLI's own report of its `sylvode` MCP server in `dir`: absent, this bot's file, or another.
fn mcp_entry(program: &Path, probe: McpProbe, dir: &Path, toml: &Path) -> Option<McpEntry> {
    let output = Command::new(program)
        .args(probe.command())
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

/// The tmux session a slot is looked for in: for a bot slot the configured one (else the one its
/// record names), for a project slot the one its record names, else the derived name. `None` for
/// a bot slot that has neither (its entry was removed and nothing was recorded).
fn target_of(runner: &RunnerConfig, slot: &Slot, record: Option<&SessionRecord>) -> Option<(TmuxName, NameSource)> {
    if slot.is_bot() {
        return super::naming::tmux_of_slot(runner, slot)
            .or_else(|| record.and_then(|record| record.tmux().map(|name| (name, record.source()))));
    }
    record
        .and_then(|record| record.tmux().map(|name| (name, record.source())))
        .or_else(|| super::naming::tmux_of_slot(runner, slot))
}

/// Looks at one session with read-only calls; `None` for a bot slot whose session nothing names.
#[must_use]
pub fn observe(
    config: &crate::config::Config,
    tmux: &Tmux,
    state: &StateDir,
    state_path: &Path,
    program: Option<&Path>,
    name: &Slot,
) -> Option<Observation> {
    let process_names = config.runner.cli_settings(name.cli()).map_or_else(
        || vec![name.cli().to_string()],
        |settings| settings.process_names.to_vec(),
    );
    let record = state.read_session_record(name);
    let (target, source) = target_of(&config.runner, name, record.as_ref())?;
    let exists = tmux.has_session(&target).unwrap_or(false);
    let panes = if exists {
        tmux.list_panes(&target).unwrap_or_default()
    } else {
        Vec::new()
    };
    let mut obs = Observation {
        name: name.clone(),
        tmux: target.clone(),
        source,
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
        return Some(obs);
    }
    let bot = name
        .bot_id()
        .or_else(|| record.as_ref().map(|record| record.bot_id.as_str()))
        .unwrap_or_default();
    let chosen = decide_ensure(&EnsureFacts {
        session_exists: true,
        panes: &obs.panes,
        process_names: &obs.process_names,
        allow_create: source == NameSource::Derived,
        record: record.as_ref(),
        task_bot_id: bot,
    });
    let EnsureDecision::Use { pane: listed, .. } = chosen else {
        return Some(obs);
    };
    let pane = Pane {
        name: target,
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
    let probe = profile(name.cli()).and_then(|p| p.mcp_probe);
    let mcp_toml = state_path.join("mcp").join(format!("{bot}.toml"));
    let mcp_file = probe.map(|probe| state_path.join("mcp").join(probe.args_file(bot)));
    obs.mcp_in_args = !bot.is_empty()
        && mcp_file
            .as_deref()
            .and_then(Path::to_str)
            .zip(obs.cli_args.as_deref())
            .is_some_and(|(path, args)| args.contains(path));
    if !obs.mcp_in_args
        && !bot.is_empty()
        && let (Some(program), Some(probe), Ok(info)) = (program, probe, tmux.pane_info(&pane))
    {
        obs.mcp_entry = mcp_entry(program, probe, Path::new(&info.current_path), &mcp_toml);
    }
    obs.credentials_newer = process
        .and_then(|row| SystemTime::now().checked_sub(row.elapsed))
        .zip(std::fs::metadata(&mcp_toml).ok().and_then(|meta| meta.modified().ok()))
        .map(|(started, written)| written > started + Duration::from_secs(2));
    obs.foreground = Some(listed.command);
    obs.pane_id = Some(listed.id.as_str().to_string());
    Some(obs)
}

/// What looking at sessions needs: tmux, the state directory and the CLI programs.
struct Context {
    tmux: Tmux,
    state: StateDir,
    path: std::path::PathBuf,
    claude: Option<std::path::PathBuf>,
    codex: Option<std::path::PathBuf>,
}

impl Context {
    fn program_of(&self, cli: &str) -> Option<&Path> {
        match cli {
            "claude" => self.claude.as_deref(),
            "codex" => self.codex.as_deref(),
            _ => None,
        }
    }
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
        codex: checker.locate(&config.runner.cli.codex.binary),
    })
}

/// The assessments of every session the runner has a record of, and of every configured one.
#[must_use]
pub fn assess_recorded(config: &crate::config::Config) -> Vec<Assessment> {
    let Some(context) = context(config) else {
        return Vec::new();
    };
    let mut names: Vec<Slot> = context
        .state
        .recorded_names()
        .iter()
        .filter_map(|name| Slot::parse(name).ok())
        .collect();
    for entry in &config.runner.tmux.sessions {
        if let Ok(name) = Slot::bot(&entry.bot, &entry.cli)
            && !names.contains(&name)
        {
            names.push(name);
        }
    }
    names
        .into_iter()
        .filter_map(|name| {
            observe(
                config,
                &context.tmux,
                &context.state,
                &context.path,
                context.program_of(name.cli()),
                &name,
            )
        })
        .map(|obs| assess(&obs))
        .collect()
}

/// The assessment of one session, `None` when there is no tmux or state directory to look in.
#[must_use]
pub fn assess_named(config: &crate::config::Config, name: &Slot) -> Option<Assessment> {
    let context = context(config)?;
    let obs = observe(
        config,
        &context.tmux,
        &context.state,
        &context.path,
        context.program_of(name.cli()),
        name,
    )?;
    Some(assess(&obs))
}

/// Whether the configured launch settings turn permission checks off, per channel.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Posture {
    pub tmux: bool,
    pub headless: bool,
}

/// Whether codex command words turn its approvals and sandbox off.
#[must_use]
pub fn codex_bypasses(words: &[String]) -> bool {
    codex_permissions(words) == Permissions::Bypass
}

/// The codex flag that turns its approvals and sandbox off.
const CODEX_BYPASS: &str = "--dangerously-bypass-approvals-and-sandbox";

/// The posture of the configured commands: one row per CLI that can run (claude always, codex and
/// opencode when enabled). opencode's permissions are not controlled, so its headless channel
/// counts as bypass.
#[must_use]
pub fn postures_of(runner: &RunnerConfig) -> Vec<(&'static str, Posture)> {
    let claude = &runner.cli.claude;
    let mut rows = vec![(
        "claude",
        Posture {
            tmux: words_bypass_permissions(&claude.launch_command),
            headless: words_bypass_permissions(&claude.headless_args),
        },
    )];
    let codex = &runner.cli.codex;
    if codex.enabled {
        rows.push((
            "codex",
            Posture {
                tmux: codex_bypasses(&codex.launch_command),
                headless: codex_bypasses(&codex.headless_args),
            },
        ));
    }
    if runner.cli.opencode.enabled {
        rows.push((
            "opencode",
            Posture {
                tmux: false,
                headless: true,
            },
        ));
    }
    rows
}
