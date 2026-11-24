//! The `[runner]` configuration: state directory, allowed roots, delivery checks, tmux, the CLIs
//! and the MCP server. Every struct refuses unknown keys, so a key that was removed (or a block
//! written for a newer runner) is refused naming the key instead of being half read.

use serde::Deserialize;

/// The oldest tmux the runner supports.
pub const TMUX_FLOOR: (u64, u64) = (3, 2);

/// The values of `[runner.mcp] tool_mode`.
pub const TOOL_MODES: [&str; 2] = ["search", "full"];

#[derive(Deserialize, Clone, Debug)]
#[serde(default, deny_unknown_fields)]
pub struct RunnerConfig {
    /// Directory for runner state; `~/` is expanded. Created with mode 0700.
    pub state_dir: Option<String>,
    /// Only these directories (and their descendants) may be working directories the runner
    /// creates a session in or starts a headless run in. Empty means none may.
    pub allowed_roots: Vec<String>,
    /// How long the synchronous part of a delivery may take before the answer is 503.
    pub delivery_check_timeout_secs: u64,
    /// Heartbeat interval for queued and running tasks.
    pub heartbeat_secs: u64,
    /// Overrides the report interval the API advertises (seconds).
    pub report_interval_secs: Option<u64>,
    /// Install the skill or the managed `AGENTS.md` section into the project directory.
    pub install_agent_guide: bool,
    /// Add managed paths to `.git/info/exclude` when the directory has a `.git` directory.
    pub git_exclude_managed: bool,
    pub tmux: RunnerTmuxConfig,
    pub cli: RunnerCliConfig,
    pub mcp: RunnerMcpConfig,
}

impl Default for RunnerConfig {
    fn default() -> Self {
        Self {
            state_dir: None,
            allowed_roots: Vec::new(),
            delivery_check_timeout_secs: 5,
            heartbeat_secs: 600,
            report_interval_secs: None,
            install_agent_guide: true,
            git_exclude_managed: true,
            tmux: RunnerTmuxConfig::default(),
            cli: RunnerCliConfig::default(),
            mcp: RunnerMcpConfig::default(),
        }
    }
}

#[derive(Deserialize, Clone, Debug)]
#[serde(default, deny_unknown_fields)]
pub struct RunnerTmuxConfig {
    pub binary: String,
    /// Passed as `-L`; empty (the default) means the operator's default tmux server.
    pub socket_name: String,
    /// Refuse a tmux older than this; it cannot be set below 3.2.
    pub min_version: String,
    pub window_width: u16,
    pub window_height: u16,
    pub ready_timeout_secs: u64,
    pub push_settle_ms: u64,
    pub user_idle_secs: u64,
    pub ack_reminder_secs: u64,
    pub report_unacked_after_secs: u64,
    pub completion_grace_secs: u64,
    /// How long a screen that shows the CLI busy or waiting for a choice may hold back the queued
    /// tasks before they are failed as retryable (a false busy reading must not stall a session
    /// for good).
    pub screen_hold_secs: u64,
    /// Existing tmux sessions that serve a bot for a CLI, whatever the project of the task
    /// (`[[runner.tmux.sessions]]`). The runner never creates, renames or restarts them.
    pub sessions: Vec<SessionOverride>,
}

/// One `[[runner.tmux.sessions]]` entry: the existing tmux session `name` serves the tasks of bot
/// `bot` for `cli`, from every project the bot is connected to.
#[derive(Deserialize, Clone, Debug)]
#[serde(deny_unknown_fields)]
pub struct SessionOverride {
    /// The bot's id: a lower case, hyphenated UUID.
    pub bot: String,
    /// `claude` or `codex`.
    pub cli: String,
    /// The tmux session name, `^[A-Za-z0-9_][A-Za-z0-9_-]{0,63}$`, never shaped like `<KEY>-<cli>`.
    pub name: String,
}

impl Default for RunnerTmuxConfig {
    fn default() -> Self {
        Self {
            binary: "tmux".to_string(),
            socket_name: String::new(),
            min_version: "3.2".to_string(),
            window_width: 200,
            window_height: 50,
            ready_timeout_secs: 90,
            push_settle_ms: 300,
            user_idle_secs: 10,
            ack_reminder_secs: 180,
            report_unacked_after_secs: 900,
            completion_grace_secs: 60,
            screen_hold_secs: 600,
            sessions: Vec::new(),
        }
    }
}

#[derive(Deserialize, Clone, Debug, Default)]
#[serde(default, deny_unknown_fields)]
pub struct RunnerCliConfig {
    pub claude: RunnerClaudeConfig,
    pub codex: RunnerCodexConfig,
    pub opencode: RunnerOpencodeConfig,
}

/// claude is always enabled: there is no `enabled` key.
#[derive(Deserialize, Clone, Debug)]
#[serde(default, deny_unknown_fields)]
pub struct RunnerClaudeConfig {
    /// The program of a headless run.
    pub binary: String,
    /// The words of the command a session the runner creates runs.
    pub launch_command: Vec<String>,
    /// Arguments of a headless run, after the prompt.
    pub headless_args: Vec<String>,
    /// Whether the runner passes its hook settings (`--settings`) to a session it creates.
    pub hooks: bool,
    /// Foreground process names that identify the CLI's pane.
    pub process_names: Vec<String>,
}

impl Default for RunnerClaudeConfig {
    fn default() -> Self {
        Self {
            binary: "claude".to_string(),
            launch_command: vec![
                "claude".to_string(),
                "--permission-mode".to_string(),
                "bypassPermissions".to_string(),
            ],
            headless_args: vec!["--permission-mode".to_string(), "bypassPermissions".to_string()],
            hooks: true,
            process_names: vec!["claude".to_string(), "claude.exe".to_string()],
        }
    }
}

#[derive(Deserialize, Clone, Debug)]
#[serde(default, deny_unknown_fields)]
pub struct RunnerCodexConfig {
    /// Off until the operator turns the headless channel on for codex.
    pub enabled: bool,
    pub binary: String,
    /// Arguments of a headless run, before the prompt (a `--sandbox` choice, say).
    pub headless_args: Vec<String>,
    /// The words of the command a session the runner creates runs. Whole-word placeholders:
    /// `{mcp_command_override}`, `{mcp_args_override}`, `{notify_override}` (see `launch.rs`).
    pub launch_command: Vec<String>,
    /// Whether the runner passes its turn hook (`-c notify=[...]`) to a session it creates.
    pub hooks: bool,
    /// Foreground process names that identify the CLI's pane.
    pub process_names: Vec<String>,
    /// Pause between pasting a message and pressing Enter, in milliseconds. codex needs about
    /// half a second to take a paste before Enter submits it; the longer of this and
    /// `[runner.tmux] push_settle_ms` applies.
    pub push_settle_ms: u64,
}

/// The shortest `[runner.cli.codex] push_settle_ms` that submits reliably.
pub const CODEX_PUSH_SETTLE_FLOOR_MS: u64 = 500;

impl Default for RunnerCodexConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            binary: "codex".to_string(),
            headless_args: Vec::new(),
            launch_command: [
                "codex",
                "--no-daemon",
                "--no-alt-screen",
                "--dangerously-bypass-approvals-and-sandbox",
            ]
            .map(str::to_string)
            .to_vec(),
            hooks: true,
            process_names: vec!["codex".to_string()],
            push_settle_ms: 700,
        }
    }
}

/// The settings every tmux-capable CLI has in the same shape.
#[derive(Debug, Clone, Copy)]
pub struct CliSettings<'a> {
    pub launch_command: &'a [String],
    pub process_names: &'a [String],
}

#[derive(Deserialize, Clone, Debug)]
#[serde(default, deny_unknown_fields)]
pub struct RunnerOpencodeConfig {
    /// Experimental until verified; `true` makes it selectable.
    pub enabled: bool,
    pub binary: String,
}

impl Default for RunnerOpencodeConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            binary: "opencode".to_string(),
        }
    }
}

#[derive(Deserialize, Clone, Debug)]
#[serde(default, deny_unknown_fields)]
pub struct RunnerMcpConfig {
    pub server_binary: String,
    /// `search` or `full`; written into the per-bot MCP config.
    pub tool_mode: String,
    /// Adds `mcp__sylvode__sylvode_write` to a headless claude run's allowed tools and turns the
    /// write call on in the per-bot MCP config.
    pub allow_write_call: bool,
}

impl Default for RunnerMcpConfig {
    fn default() -> Self {
        Self {
            server_binary: "mcp-server".to_string(),
            tool_mode: "search".to_string(),
            allow_write_call: false,
        }
    }
}

impl RunnerConfig {
    /// The launch settings of `cli`; `None` for a CLI without the tmux channel.
    pub fn cli_settings(&self, cli: &str) -> Option<CliSettings<'_>> {
        match cli {
            "claude" => Some(CliSettings {
                launch_command: &self.cli.claude.launch_command,
                process_names: &self.cli.claude.process_names,
            }),
            "codex" => Some(CliSettings {
                launch_command: &self.cli.codex.launch_command,
                process_names: &self.cli.codex.process_names,
            }),
            _ => None,
        }
    }

    /// The configured session of bot `bot` (a UUID in any form) for `cli`.
    pub fn bot_session(&self, bot: &str, cli: &str) -> Option<&SessionOverride> {
        let bot = uuid::Uuid::parse_str(bot.trim()).ok()?.hyphenated().to_string();
        self.tmux.sessions.iter().find(|s| s.bot == bot && s.cli == cli)
    }

    /// The pause between a paste and Enter for `cli`: the global `[runner.tmux] push_settle_ms`,
    /// or codex's own when that is longer.
    pub fn push_settle_ms(&self, cli: &str) -> u64 {
        let own = if cli == "codex" {
            self.cli.codex.push_settle_ms
        } else {
            0
        };
        self.tmux.push_settle_ms.max(own)
    }

    /// Refuses values that parse but can never work.
    ///
    /// # Errors
    ///
    /// A message naming the key.
    pub fn validate(&self) -> Result<(), String> {
        if !(1..=60).contains(&self.delivery_check_timeout_secs) {
            return Err("runner.delivery_check_timeout_secs must be between 1 and 60".to_string());
        }
        if self
            .report_interval_secs
            .is_some_and(|secs| !(1..=86_400).contains(&secs))
        {
            return Err("runner.report_interval_secs must be between 1 and 86400".to_string());
        }
        if self.heartbeat_secs == 0 {
            return Err("runner.heartbeat_secs must be at least 1".to_string());
        }
        let tmux = &self.tmux;
        let Some((major, minor)) = parse_major_minor(&tmux.min_version) else {
            return Err("runner.tmux.min_version must look like 3.2".to_string());
        };
        if (major, minor) < TMUX_FLOOR {
            return Err("runner.tmux.min_version cannot be lower than 3.2".to_string());
        }
        super::tmux::SocketName::new(&tmux.socket_name)
            .map_err(|_| "runner.tmux.socket_name must match ^[A-Za-z0-9_-]{0,32}$".to_string())?;
        if !(20..=1000).contains(&tmux.window_width) || !(5..=500).contains(&tmux.window_height) {
            return Err("runner.tmux.window_width and window_height are out of range".to_string());
        }
        validate_sessions(&tmux.sessions)?;
        if !TOOL_MODES.contains(&self.mcp.tool_mode.as_str()) {
            return Err(format!(
                "runner.mcp.tool_mode must be one of: {}",
                TOOL_MODES.join(", ")
            ));
        }
        let words_ok = |words: &[String]| words.iter().all(|word| !word.chars().any(char::is_control));
        for cli in ["claude", "codex"] {
            let Some(settings) = self.cli_settings(cli) else {
                continue;
            };
            match settings.launch_command.first() {
                None => return Err(format!("runner.cli.{cli}.launch_command must not be empty")),
                Some(first) if first.trim().is_empty() || first.starts_with('-') => {
                    return Err(format!(
                        "runner.cli.{cli}.launch_command must start with a program, not an empty word or an option"
                    ));
                }
                Some(_) => {}
            }
            if !words_ok(settings.launch_command) {
                return Err(format!(
                    "runner.cli.{cli}.launch_command must not hold control characters"
                ));
            }
            if settings.process_names.is_empty() || settings.process_names.iter().any(|name| name.trim().is_empty()) {
                return Err(format!(
                    "runner.cli.{cli}.process_names must hold at least one non-empty name"
                ));
            }
        }
        for (key, words) in [
            ("runner.cli.claude.headless_args", &self.cli.claude.headless_args),
            ("runner.cli.codex.headless_args", &self.cli.codex.headless_args),
        ] {
            if !words_ok(words) {
                return Err(format!("{key} must not hold control characters"));
            }
        }
        if !(CODEX_PUSH_SETTLE_FLOOR_MS..=10_000).contains(&self.cli.codex.push_settle_ms) {
            return Err(format!(
                "runner.cli.codex.push_settle_ms must be between {CODEX_PUSH_SETTLE_FLOOR_MS} and 10000"
            ));
        }
        for (key, binary) in [
            ("runner.tmux.binary", &tmux.binary),
            ("runner.cli.claude.binary", &self.cli.claude.binary),
            ("runner.cli.codex.binary", &self.cli.codex.binary),
            ("runner.cli.opencode.binary", &self.cli.opencode.binary),
            ("runner.mcp.server_binary", &self.mcp.server_binary),
        ] {
            if binary.trim().is_empty() || binary.chars().any(char::is_control) {
                return Err(format!("{key} must be a program name or an absolute path"));
            }
        }
        Ok(())
    }
}

fn validate_sessions(sessions: &[SessionOverride]) -> Result<(), String> {
    for (i, entry) in sessions.iter().enumerate() {
        let at = format!("runner.tmux.sessions[{i}]");
        let canonical = uuid::Uuid::parse_str(&entry.bot).is_ok_and(|id| id.hyphenated().to_string() == entry.bot);
        if !canonical {
            return Err(format!("{at}.bot must be a bot id: a lower case, hyphenated UUID"));
        }
        if !matches!(entry.cli.as_str(), "claude" | "codex") {
            return Err(format!("{at}.cli must be claude or codex"));
        }
        super::tmux::TmuxName::new(&entry.name).map_err(|e| format!("{at}.name: {e}"))?;
        for (j, other) in sessions.iter().enumerate().take(i) {
            let earlier = format!("runner.tmux.sessions[{j}]");
            if other.bot == entry.bot && other.cli == entry.cli {
                return Err(format!("{at} repeats the bot and cli of {earlier}"));
            }
            if other.name.eq_ignore_ascii_case(&entry.name) {
                return Err(format!(
                    "{at}.name repeats the name of {earlier} (names compare without case)"
                ));
            }
        }
    }
    Ok(())
}

fn parse_major_minor(text: &str) -> Option<(u64, u64)> {
    let (major, minor) = text.trim().split_once('.')?;
    Some((major.parse().ok()?, minor.parse().ok()?))
}
