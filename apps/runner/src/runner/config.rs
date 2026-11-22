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
    /// Project keys whose directories may hold CLI configuration files the runner cannot switch
    /// off for codex and opencode (a headless run is refused in a directory that has them, unless
    /// the operator trusts the project here).
    pub trust_project_files: Vec<String>,
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
            trust_project_files: Vec::new(),
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
    /// The tmux channel is off until the operator turns it on: it types into a live CLI, and the
    /// hand checks of the security review (README, "The tmux channel") must have been done on the
    /// pinned CLI version first.
    pub enabled: bool,
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
    /// How long a screen that shows the CLI busy or waiting for a choice may hold back the queued
    /// tasks before they are failed as retryable (a false busy reading must not stall a session
    /// for good).
    pub screen_hold_secs: u64,
}

impl Default for RunnerTmuxConfig {
    fn default() -> Self {
        Self {
            enabled: false,
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
            screen_hold_secs: 600,
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
    /// Variables of the runner's own environment the CLI is given besides the allow-list (an API
    /// key, a config directory, a proxy). Nothing else is inherited.
    pub env_passthrough: Vec<String>,
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
            env_passthrough: Vec::new(),
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
    /// See `[runner.cli.claude] env_passthrough`.
    pub env_passthrough: Vec<String>,
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
            env_passthrough: Vec::new(),
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
    /// See `[runner.cli.claude] env_passthrough`.
    pub env_passthrough: Vec<String>,
}

impl Default for RunnerOpencodeConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            binary: "opencode".to_string(),
            process_names: vec!["opencode".to_string()],
            env_passthrough: Vec::new(),
        }
    }
}

#[derive(Deserialize, Clone, Debug)]
#[serde(default, deny_unknown_fields)]
pub struct RunnerMcpConfig {
    pub server_binary: String,
    /// `search` or `full`; written into the per-bot MCP config.
    pub tool_mode: String,
}

impl Default for RunnerMcpConfig {
    fn default() -> Self {
        Self {
            server_binary: "mcp-server".to_string(),
            tool_mode: "search".to_string(),
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
        if self.cli.claude.permission_mode != "dontAsk" {
            return Err(format!(
                "runner.cli.claude.permission_mode = {:?} is not allowed for runner agents: only \"dontAsk\" \
                 (anything that would ask is denied) keeps the task tools and the project's files as the \
                 whole of what a task can do",
                self.cli.claude.permission_mode
            ));
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
        for (key, names) in [
            ("runner.cli.claude.env_passthrough", &self.cli.claude.env_passthrough),
            ("runner.cli.codex.env_passthrough", &codex.env_passthrough),
            (
                "runner.cli.opencode.env_passthrough",
                &self.cli.opencode.env_passthrough,
            ),
        ] {
            for name in names {
                let valid = !name.is_empty()
                    && name.len() <= 64
                    && name
                        .chars()
                        .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_')
                    && !name.starts_with(|c: char| c.is_ascii_digit());
                if !valid
                    || matches!(
                        name.as_str(),
                        "CLAUDECODE" | "CLAUDE_CODE_ENTRYPOINT" | "TMUX" | "CLAUDE_CONFIG_DIR"
                    )
                {
                    return Err(format!(
                        "{key} holds a name that is not an environment variable the CLI may be given"
                    ));
                }
            }
        }
        for key in &self.trust_project_files {
            if !super::checks::valid_project_key(key) {
                return Err("runner.trust_project_files holds a value that is not a project key".to_string());
            }
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
