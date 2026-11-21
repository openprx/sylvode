//! The `[runner]` configuration: state directory, allowed roots, delivery checks, tmux, the CLIs
//! and the MCP server. Every struct refuses unknown keys, so a block written for a newer runner is
//! refused naming the key instead of being half read.

use serde::Deserialize;

/// The oldest tmux the runner supports.
pub const TMUX_FLOOR: (u64, u64) = (3, 2);

/// The values of `[runner.mcp] tool_mode`.
pub const TOOL_MODES: [&str; 2] = ["search", "full"];

/// The codex sandboxes.
pub const CODEX_SANDBOXES: [&str; 3] = ["read-only", "workspace-write", "danger-full-access"];

#[derive(Deserialize, Clone, Debug)]
#[serde(default, deny_unknown_fields)]
pub struct RunnerConfig {
    /// Directory for runner state; `~/` is expanded. Created with mode 0700.
    pub state_dir: Option<String>,
    /// Only these directories (and their descendants) may be project working directories. Empty
    /// means every project is declined.
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
    /// Passed as `-L`; empty means the user's default tmux server.
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
}

impl Default for RunnerTmuxConfig {
    fn default() -> Self {
        Self {
            binary: "tmux".to_string(),
            socket_name: "sylvode".to_string(),
            min_version: "3.2".to_string(),
            window_width: 200,
            window_height: 50,
            ready_timeout_secs: 90,
            push_settle_ms: 300,
            user_idle_secs: 10,
            ack_reminder_secs: 180,
            report_unacked_after_secs: 900,
            completion_grace_secs: 60,
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
    pub binary: String,
    pub permission_mode: String,
    /// Adds `mcp__sylvode__sylvode_write` to `--allowedTools`.
    pub allow_write_call: bool,
    pub allowed_tools: Vec<String>,
    pub disallowed_tools: Vec<String>,
    pub dangerously_bypass_permissions: bool,
    /// Foreground process names, for adopted sessions.
    pub process_names: Vec<String>,
}

impl Default for RunnerClaudeConfig {
    fn default() -> Self {
        Self {
            binary: "claude".to_string(),
            permission_mode: "dontAsk".to_string(),
            allow_write_call: false,
            allowed_tools: Vec::new(),
            disallowed_tools: Vec::new(),
            dangerously_bypass_permissions: false,
            process_names: vec!["claude".to_string(), "claude.exe".to_string()],
        }
    }
}

#[derive(Deserialize, Clone, Debug)]
#[serde(default, deny_unknown_fields)]
pub struct RunnerCodexConfig {
    /// Off until codex is verified on the tmux channel.
    pub enabled: bool,
    pub binary: String,
    pub sandbox: String,
    /// Only `never` is accepted: nobody answers an approval prompt in an unattended session.
    pub approval: String,
    pub dangerously_bypass_permissions: bool,
    pub process_names: Vec<String>,
}

impl Default for RunnerCodexConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            binary: "codex".to_string(),
            sandbox: "read-only".to_string(),
            approval: "never".to_string(),
            dangerously_bypass_permissions: false,
            process_names: vec!["codex".to_string()],
        }
    }
}

#[derive(Deserialize, Clone, Debug)]
#[serde(default, deny_unknown_fields)]
pub struct RunnerOpencodeConfig {
    /// Experimental until verified; `true` makes it selectable.
    pub enabled: bool,
    pub binary: String,
    pub process_names: Vec<String>,
}

impl Default for RunnerOpencodeConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            binary: "opencode".to_string(),
            process_names: vec!["opencode".to_string()],
        }
    }
}

#[derive(Deserialize, Clone, Debug)]
#[serde(default, deny_unknown_fields)]
pub struct RunnerMcpConfig {
    pub server_binary: String,
    /// `search` or `full`; written into the per-bot MCP config.
    pub tool_mode: String,
    /// The MCP config a headless run is given until the per-bot files exist.
    pub mcp_config_path: Option<String>,
}

impl Default for RunnerMcpConfig {
    fn default() -> Self {
        Self {
            server_binary: "mcp-server".to_string(),
            tool_mode: "search".to_string(),
            mcp_config_path: None,
        }
    }
}

impl RunnerConfig {
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
            .map_err(|_| "runner.tmux.socket_name must match ^[A-Za-z0-9_-]{1,32}$ or be empty".to_string())?;
        if !(20..=1000).contains(&tmux.window_width) || !(5..=500).contains(&tmux.window_height) {
            return Err("runner.tmux.window_width and window_height are out of range".to_string());
        }
        if !TOOL_MODES.contains(&self.mcp.tool_mode.as_str()) {
            return Err(format!(
                "runner.mcp.tool_mode must be one of: {}",
                TOOL_MODES.join(", ")
            ));
        }
        let codex = &self.cli.codex;
        if codex.approval != "never" {
            return Err("runner.cli.codex.approval must be \"never\": nobody answers approval prompts".to_string());
        }
        if !CODEX_SANDBOXES.contains(&codex.sandbox.as_str()) {
            return Err(format!(
                "runner.cli.codex.sandbox must be one of: {}",
                CODEX_SANDBOXES.join(", ")
            ));
        }
        for (key, binary) in [
            ("runner.tmux.binary", &tmux.binary),
            ("runner.cli.claude.binary", &self.cli.claude.binary),
            ("runner.cli.codex.binary", &codex.binary),
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

fn parse_major_minor(text: &str) -> Option<(u64, u64)> {
    let (major, minor) = text.trim().split_once('.')?;
    Some((major.parse().ok()?, minor.parse().ok()?))
}
