//! The command line of an interactive CLI session (design section 6.6): the explicit argument
//! vector the tmux pane runs, with the permission posture of `[runner.cli.*]` and nothing broader.
//!
//! - **No shell.** The vector is given to tmux after `--` (see the driver); no element is ever
//!   parsed by a shell. The only shell string is the hook command inside claude's settings file,
//!   which claude itself runs through a shell: it is built from fixed words and shell-quoted
//!   values the runner chose (binary path, session key, socket path), never from task content.
//! - **No tokens.** Neither vector holds a token: they name files (`0600` in a `0700` directory)
//!   that do (see `files.rs`).
//! - **Permissions.** claude runs with `--permission-mode` (default `dontAsk`), `--strict-mcp-config`
//!   with the runner's own MCP file (a project `.mcp.json` is ignored), and an explicit allow list:
//!   the eight search-mode tools, `Read`/`Glob`/`Grep` confined to the canonical working directory,
//!   `sylvode_write` only with `allow_write_call`, and the configured extras. Bypassing permissions
//!   needs `dangerously_bypass_permissions`, validated by the same code as the headless channel.
//!   codex runs with `-s <sandbox> -a never` (nobody answers approvals) and `-C <dir>`.
//!   opencode is not launched with any permission argument of the runner's.
//! - **Directory in rules.** A permission rule holds the directory, so a directory whose name has
//!   characters with a meaning in rules (`(`, `)`, `*`, `?`, `[`, `{`, `,`, quotes, `#`, ...) is
//!   refused.

use std::path::Path;

use super::config::{RunnerClaudeConfig, RunnerCodexConfig};
use super::tmux::SessionKey;
use crate::executor_permissions::{CodexPermissions, ExecutorPermissions};

/// The name of the MCP server in the per-bot files and in tool names (`mcp__sylvode__...`).
pub const SERVER_NAME: &str = "sylvode";

/// The tools every session may use: the entry tools and the five task tools. `sylvode_write` is
/// added only by `allow_write_call`.
pub const SEARCH_MODE_TOOLS: [&str; 8] = [
    "sylvode_overview",
    "sylvode_search",
    "sylvode_read",
    "sylvode_task_get",
    "sylvode_task_ack",
    "sylvode_task_progress",
    "sylvode_task_complete",
    "sylvode_task_fail",
];

/// The write entry tool.
pub const WRITE_TOOL: &str = "sylvode_write";

/// Why a command line could not be built.
#[derive(Debug, PartialEq, Eq)]
pub enum LaunchError {
    /// A value cannot be used safely (the message never holds the value).
    Unsafe(String),
    /// The permission settings are invalid or need an acknowledgement.
    Permissions(String),
}

impl std::fmt::Display for LaunchError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Unsafe(why) | Self::Permissions(why) => write!(f, "{why}"),
        }
    }
}

impl std::error::Error for LaunchError {}

/// `mcp__sylvode__<tool>`.
pub fn mcp_tool(tool: &str) -> String {
    format!("mcp__{SERVER_NAME}__{tool}")
}

/// The directory as it may appear in a permission rule or an argument: UTF-8, absolute, and only
/// characters that mean nothing special in a rule.
///
/// # Errors
///
/// [`LaunchError::Unsafe`].
pub fn rule_safe_dir(dir: &Path) -> Result<&str, LaunchError> {
    let text = dir
        .to_str()
        .filter(|text| text.starts_with('/'))
        .ok_or_else(|| LaunchError::Unsafe("the working directory is not an absolute UTF-8 path".to_string()))?;
    let safe =
        |c: char| c.is_alphanumeric() || matches!(c, ' ' | '_' | '-' | '.' | '+' | '/' | '@' | '~' | '=' | '%' | ':');
    if text.chars().all(safe) {
        Ok(text)
    } else {
        Err(LaunchError::Unsafe(
            "the working directory has characters that cannot be used in a permission rule".to_string(),
        ))
    }
}

/// Where the files of one launch are, and who runs the hooks.
#[derive(Debug, Clone)]
pub struct LaunchFiles<'a> {
    /// The MCP server executable (absolute).
    pub mcp_server: &'a Path,
    /// `mcp/<bot>.toml`.
    pub mcp_toml: &'a Path,
    /// `mcp/<bot>.claude.json`.
    pub claude_mcp_json: &'a Path,
    /// `sessions/<KEY>/claude-settings.json`.
    pub claude_settings: &'a Path,
    /// This executable, for the codex `notify` hook.
    pub runner_exe: &'a Path,
    /// `state_dir/hook.sock`.
    pub hook_socket: &'a Path,
}

fn permissions(settings: &crate::config::CliAgentConfig) -> Result<ExecutorPermissions, LaunchError> {
    ExecutorPermissions::from_config(settings).map_err(LaunchError::Permissions)
}

/// The argument vector of an interactive claude session, `program` first.
///
/// # Errors
///
/// [`LaunchError`] for an unusable directory or invalid permission settings.
pub fn claude_argv(
    program: &Path,
    cfg: &RunnerClaudeConfig,
    dir: &Path,
    files: &LaunchFiles<'_>,
) -> Result<Vec<String>, LaunchError> {
    let dir_text = rule_safe_dir(dir)?;
    // The same validation the headless channel applies.
    let mut settings = crate::config::CliAgentConfig::headless("claude-code", dir_text, 1);
    settings.permission_mode = Some(cfg.permission_mode.clone());
    settings.dangerously_bypass_permissions = cfg.dangerously_bypass_permissions;
    let ExecutorPermissions::ClaudeCode(permissions) = permissions(&settings)? else {
        return Err(LaunchError::Permissions("not a claude configuration".to_string()));
    };
    let expand = |entry: &String| entry.replace("{dir}", dir_text);
    let mut allowed: Vec<String> = SEARCH_MODE_TOOLS.iter().map(|tool| mcp_tool(tool)).collect();
    if cfg.allow_write_call {
        allowed.push(mcp_tool(WRITE_TOOL));
    }
    for tool in ["Read", "Glob", "Grep"] {
        allowed.push(format!("{tool}(/{dir_text}/**)"));
    }
    allowed.extend(cfg.allowed_tools.iter().map(expand));
    let disallowed: Vec<String> = cfg.disallowed_tools.iter().map(expand).collect();
    for entry in allowed.iter().chain(&disallowed) {
        if entry.is_empty() || entry.starts_with('-') || entry.chars().any(char::is_control) {
            return Err(LaunchError::Permissions(
                "a tool entry is empty, starts with `-` or has control characters".to_string(),
            ));
        }
    }
    let path = |p: &Path| -> Result<String, LaunchError> {
        p.to_str()
            .map(str::to_string)
            .ok_or_else(|| LaunchError::Unsafe("a runner file path is not UTF-8".to_string()))
    };
    let mut argv = vec![
        path(program)?,
        "--permission-mode".to_string(),
        permissions.mode.to_string(),
        "--mcp-config".to_string(),
        path(files.claude_mcp_json)?,
        "--strict-mcp-config".to_string(),
        "--settings".to_string(),
        path(files.claude_settings)?,
        "--allowedTools".to_string(),
    ];
    argv.extend(allowed);
    if !disallowed.is_empty() {
        argv.push("--disallowedTools".to_string());
        argv.extend(disallowed);
    }
    Ok(argv)
}

/// A TOML value as one `-c` argument: serialised by the TOML serialiser, never concatenated.
pub fn toml_string(text: &str) -> String {
    toml::Value::String(text.to_string()).to_string()
}

/// A TOML array of strings.
pub fn toml_array(items: &[String]) -> String {
    toml::Value::Array(items.iter().map(|item| toml::Value::String(item.clone())).collect()).to_string()
}

/// The words of a hook run by the CLI: this executable's `hook` subcommand for one event.
pub fn hook_words(
    files: &LaunchFiles<'_>,
    cli: &str,
    key: &SessionKey,
    event: &str,
) -> Result<Vec<String>, LaunchError> {
    let text = |p: &Path| -> Result<String, LaunchError> {
        p.to_str()
            .filter(|t| !t.chars().any(char::is_control))
            .map(str::to_string)
            .ok_or_else(|| LaunchError::Unsafe("a runner file path is not UTF-8 or has control characters".to_string()))
    };
    Ok(vec![
        text(files.runner_exe)?,
        "hook".to_string(),
        "--cli".to_string(),
        cli.to_string(),
        "--session".to_string(),
        key.as_str().to_string(),
        "--event".to_string(),
        event.to_string(),
        "--socket".to_string(),
        text(files.hook_socket)?,
    ])
}

/// The argument vector of an interactive codex session, `program` first.
///
/// # Errors
///
/// [`LaunchError`] for an unusable directory or invalid permission settings.
pub fn codex_argv(
    program: &Path,
    cfg: &RunnerCodexConfig,
    dir: &Path,
    files: &LaunchFiles<'_>,
    key: &SessionKey,
) -> Result<Vec<String>, LaunchError> {
    let dir_text = rule_safe_dir(dir)?;
    let mut settings = crate::config::CliAgentConfig::headless("codex", dir_text, 1);
    settings.sandbox = Some(cfg.sandbox.clone());
    settings.dangerously_bypass_permissions = cfg.dangerously_bypass_permissions;
    let ExecutorPermissions::Codex(CodexPermissions::Sandbox(sandbox)) = permissions(&settings)? else {
        return Err(LaunchError::Permissions(
            "codex is launched with a sandbox only".to_string(),
        ));
    };
    let utf8 = |p: &Path| -> Result<String, LaunchError> {
        p.to_str()
            .map(str::to_string)
            .ok_or_else(|| LaunchError::Unsafe("a runner file path is not UTF-8".to_string()))
    };
    let server_args = vec![
        "serve".to_string(),
        "--transport".to_string(),
        "stdio".to_string(),
        "--config".to_string(),
        utf8(files.mcp_toml)?,
    ];
    let notify = hook_words(files, "codex", key, "turn-ended")?;
    Ok(vec![
        utf8(program)?,
        "-s".to_string(),
        sandbox.to_string(),
        "-a".to_string(),
        "never".to_string(),
        "-C".to_string(),
        dir_text.to_string(),
        "-c".to_string(),
        format!(
            "mcp_servers.{SERVER_NAME}.command={}",
            toml_string(&utf8(files.mcp_server)?)
        ),
        "-c".to_string(),
        format!("mcp_servers.{SERVER_NAME}.args={}", toml_array(&server_args)),
        "-c".to_string(),
        format!("notify={}", toml_array(&notify)),
    ])
}

/// The argument vector of an opencode session: the program alone. opencode follows its own
/// configuration, which the runner does not control (`doctor` warns about that).
pub fn opencode_argv(program: &Path) -> Result<Vec<String>, LaunchError> {
    program
        .to_str()
        .map(|p| vec![p.to_string()])
        .ok_or_else(|| LaunchError::Unsafe("a runner file path is not UTF-8".to_string()))
}

/// Quotes one word for a POSIX shell: single quotes, with `'` written as `'\''`.
///
/// Only claude's hook command needs it (claude runs it through a shell).
pub fn shell_quote(word: &str) -> String {
    format!("'{}'", word.replace('\'', "'\\''"))
}

/// The command string of one claude hook.
///
/// # Errors
///
/// [`LaunchError::Unsafe`] for a path with control characters.
pub fn claude_hook_command(files: &LaunchFiles<'_>, key: &SessionKey, event: &str) -> Result<String, LaunchError> {
    Ok(hook_words(files, "claude", key, event)?
        .iter()
        .map(|word| shell_quote(word))
        .collect::<Vec<_>>()
        .join(" "))
}
