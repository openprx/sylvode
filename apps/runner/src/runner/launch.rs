//! The command lines the runner builds: the argument vector of a session it creates and the
//! argument vectors of headless runs.
//!
//! - **No shell.** The vector is given to tmux after `--` (see the driver); no element is ever
//!   parsed by a shell. The only shell string is the hook command inside claude's settings file,
//!   which claude itself runs through a shell: it is built from fixed words and shell-quoted
//!   values the runner chose (binary path, session name, socket path), never from task content.
//! - **No tokens.** No vector holds a token: they name files (`0600` in a `0700` directory) that
//!   do (see `files.rs`).
//! - **The operator's posture.** What the CLI may do is what `[runner.cli.claude] launch_command`
//!   and `headless_args` say, and the operator's own CLI settings. The runner adds `--mcp-config`
//!   (and `--settings` for the hooks) to a session it creates, and `--mcp-config` plus the allow
//!   list of the Sylvode tools to a headless run. Nothing restricts the CLI's other tools, MCP
//!   servers or settings sources.

use std::path::Path;

use super::config::{RunnerClaudeConfig, RunnerCodexConfig};

/// The name of the MCP server in the per-bot files and in tool names (`mcp__sylvode__...`).
pub const SERVER_NAME: &str = "sylvode";

/// The tools every headless run may use: the entry tools and the five task tools. `sylvode_write`
/// is added only by `[runner.mcp] allow_write_call`.
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
}

impl std::fmt::Display for LaunchError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Unsafe(why) => write!(f, "{why}"),
        }
    }
}

impl std::error::Error for LaunchError {}

/// `mcp__sylvode__<tool>`.
pub fn mcp_tool(tool: &str) -> String {
    format!("mcp__{SERVER_NAME}__{tool}")
}

/// Where the files of one launch are, and who runs the hooks.
#[derive(Debug, Clone)]
pub struct LaunchFiles<'a> {
    /// `mcp/<bot>.claude.json`.
    pub claude_mcp_json: &'a Path,
    /// `sessions/<NAME>/claude-settings.json`.
    pub claude_settings: &'a Path,
    /// This executable, for the hook command.
    pub runner_exe: &'a Path,
    /// `state_dir/hook.sock`.
    pub hook_socket: &'a Path,
}

/// Where the files of one codex launch are. codex gets no settings file: the MCP server and the
/// turn hook go on its command line.
#[derive(Debug, Clone)]
pub struct CodexLaunchFiles<'a> {
    /// The MCP server executable.
    pub mcp_server: &'a Path,
    /// `mcp/<bot>.toml`, which holds the bot's token.
    pub mcp_toml: &'a Path,
    /// This executable, for the notify program.
    pub runner_exe: &'a Path,
    /// `state_dir/hook.sock`.
    pub hook_socket: &'a Path,
}

/// The placeholder of a codex launch command for the value of the MCP server's command override.
pub const PLACEHOLDER_MCP_COMMAND: &str = "{mcp_command_override}";

/// The placeholder of a codex launch command for the value of the MCP server's args override.
pub const PLACEHOLDER_MCP_ARGS: &str = "{mcp_args_override}";

/// The placeholder of a codex launch command for the value of the notify override.
pub const PLACEHOLDER_NOTIFY: &str = "{notify_override}";

/// The placeholder of a launch command for the path of the bot's MCP file.
pub const PLACEHOLDER_MCP_CONFIG: &str = "{mcp_config}";

/// The placeholder of a launch command for the path of the session's hook settings.
pub const PLACEHOLDER_SETTINGS: &str = "{settings}";

/// The argument vector of the CLI in a session the runner creates, `program` first.
///
/// The words of `launch_command` after the program, in order. A word that is exactly
/// `{mcp_config}` or `{settings}` is replaced by the path of that file (whole words only: a word
/// that merely contains one is left as it is). What the template does not place itself is
/// appended: `--mcp-config <file>` unless the template has `{mcp_config}`, and with `hooks` on
/// `--settings <file>` unless it has `{settings}`. A template that has a placeholder owns its
/// position, so nothing is appended for that file.
///
/// # Errors
///
/// [`LaunchError`] for a path that is not UTF-8 or an empty `launch_command`.
pub fn session_argv(
    program: &Path,
    cfg: &RunnerClaudeConfig,
    files: &LaunchFiles<'_>,
) -> Result<Vec<String>, LaunchError> {
    let Some((_, rest)) = cfg.launch_command.split_first() else {
        return Err(LaunchError::Unsafe("the launch command is empty".to_string()));
    };
    let mcp_config = utf8(files.claude_mcp_json)?;
    let settings = utf8(files.claude_settings)?;
    let has = |placeholder: &str| rest.iter().any(|word| word == placeholder);
    let (places_mcp, places_settings) = (has(PLACEHOLDER_MCP_CONFIG), has(PLACEHOLDER_SETTINGS));
    let mut argv = vec![utf8(program)?];
    argv.extend(rest.iter().map(|word| match word.as_str() {
        PLACEHOLDER_MCP_CONFIG => mcp_config.clone(),
        PLACEHOLDER_SETTINGS => settings.clone(),
        other => other.to_string(),
    }));
    if !places_mcp {
        argv.push("--mcp-config".to_string());
        argv.push(mcp_config);
    }
    if cfg.hooks && !places_settings {
        argv.push("--settings".to_string());
        argv.push(settings);
    }
    Ok(argv)
}

/// The argument vector of codex in a session the runner creates, `program` first.
///
/// The words of `launch_command` after the program, in order. A word that is exactly
/// `{mcp_command_override}`, `{mcp_args_override}` or `{notify_override}` is replaced by the value
/// of that `-c` override (the template writes the `-c` itself; whole words only). What the
/// template does not place is appended as `-c <value>`: the MCP server's command and args always,
/// the notify program only with `hooks` on. No value holds a token: the MCP file that does is
/// named, not read.
///
/// # Errors
///
/// [`LaunchError`] for a path that is not UTF-8 (or has control characters, for the notify
/// program) or an empty `launch_command`.
pub fn codex_session_argv(
    program: &Path,
    cfg: &RunnerCodexConfig,
    files: &CodexLaunchFiles<'_>,
) -> Result<Vec<String>, LaunchError> {
    let Some((_, rest)) = cfg.launch_command.split_first() else {
        return Err(LaunchError::Unsafe("the launch command is empty".to_string()));
    };
    let overrides = [
        (PLACEHOLDER_MCP_COMMAND, mcp_command_override(files.mcp_server)?),
        (PLACEHOLDER_MCP_ARGS, mcp_args_override(files.mcp_toml)?),
        (
            PLACEHOLDER_NOTIFY,
            notify_override(files.runner_exe, files.hook_socket)?,
        ),
    ];
    let mut argv = vec![utf8(program)?];
    argv.extend(rest.iter().map(|word| {
        overrides
            .iter()
            .find(|(placeholder, _)| word == placeholder)
            .map_or_else(|| word.clone(), |(_, value)| value.clone())
    }));
    for (placeholder, value) in overrides {
        let placed = rest.iter().any(|word| word == placeholder);
        if !placed && (placeholder != PLACEHOLDER_NOTIFY || cfg.hooks) {
            argv.push("-c".to_string());
            argv.push(value);
        }
    }
    Ok(argv)
}

fn mcp_command_override(mcp_server: &Path) -> Result<String, LaunchError> {
    Ok(format!(
        "mcp_servers.{SERVER_NAME}.command={}",
        toml_string(&utf8(mcp_server)?)
    ))
}

fn mcp_args_override(mcp_toml: &Path) -> Result<String, LaunchError> {
    let server_args = vec![
        "serve".to_string(),
        "--transport".to_string(),
        "stdio".to_string(),
        "--config".to_string(),
        utf8(mcp_toml)?,
    ];
    Ok(format!("mcp_servers.{SERVER_NAME}.args={}", toml_array(&server_args)))
}

fn notify_override(runner_exe: &Path, hook_socket: &Path) -> Result<String, LaunchError> {
    Ok(format!(
        "notify={}",
        toml_array(&hook_words_for(runner_exe, hook_socket, "codex", "turn-ended")?)
    ))
}

/// The argument vector of a headless claude run.
///
/// `-p <prompt>` comes first (so no variadic list below can swallow the prompt), then the
/// operator's `headless_args`, the runner's MCP file, and the allow list of the Sylvode tools
/// (`sylvode_write` too with `allow_write_call`).
///
/// # Errors
///
/// [`LaunchError`] for an invalid prompt or a path that is not UTF-8.
pub fn claude_headless_argv(
    program: &Path,
    cfg: &RunnerClaudeConfig,
    mcp_config: &Path,
    allow_write_call: bool,
    prompt: &str,
) -> Result<Vec<String>, LaunchError> {
    check_prompt(prompt)?;
    let mut argv = vec![utf8(program)?, "-p".to_string(), prompt.to_string()];
    argv.extend(cfg.headless_args.iter().cloned());
    argv.push("--mcp-config".to_string());
    argv.push(utf8(mcp_config)?);
    argv.push("--allowedTools".to_string());
    argv.extend(SEARCH_MODE_TOOLS.iter().map(|tool| mcp_tool(tool)));
    if allow_write_call {
        argv.push(mcp_tool(WRITE_TOOL));
    }
    Ok(argv)
}

/// A prompt that starts with `-` would be read as an option.
fn check_prompt(prompt: &str) -> Result<(), LaunchError> {
    if prompt.is_empty() || prompt.starts_with('-') {
        return Err(LaunchError::Unsafe(
            "the prompt is empty or starts with `-`".to_string(),
        ));
    }
    Ok(())
}

fn utf8(p: &Path) -> Result<String, LaunchError> {
    p.to_str()
        .map(str::to_string)
        .ok_or_else(|| LaunchError::Unsafe("a runner file path is not UTF-8".to_string()))
}

/// A TOML value as one `-c` argument: serialised by the TOML serialiser, never concatenated.
pub fn toml_string(text: &str) -> String {
    toml::Value::String(text.to_string()).to_string()
}

/// A TOML array of strings.
pub fn toml_array(items: &[String]) -> String {
    toml::Value::Array(items.iter().map(|item| toml::Value::String(item.clone())).collect()).to_string()
}

/// The words of a hook run by the CLI: this executable's `hook` subcommand for one event. They
/// hold no secret: the runner knows the sender by the tmux pane it runs in.
pub fn hook_words(files: &LaunchFiles<'_>, cli: &str, event: &str) -> Result<Vec<String>, LaunchError> {
    hook_words_for(files.runner_exe, files.hook_socket, cli, event)
}

pub fn hook_words_for(
    runner_exe: &Path,
    hook_socket: &Path,
    cli: &str,
    event: &str,
) -> Result<Vec<String>, LaunchError> {
    let text = |p: &Path| -> Result<String, LaunchError> {
        p.to_str()
            .filter(|t| !t.chars().any(char::is_control))
            .map(str::to_string)
            .ok_or_else(|| LaunchError::Unsafe("a runner file path is not UTF-8 or has control characters".to_string()))
    };
    Ok(vec![
        text(runner_exe)?,
        "hook".to_string(),
        "--cli".to_string(),
        cli.to_string(),
        "--event".to_string(),
        event.to_string(),
        "--socket".to_string(),
        text(hook_socket)?,
    ])
}

/// The argument vector of a headless codex run: `codex exec` with the runner's MCP server, the
/// operator's `headless_args`, the prompt last.
///
/// # Errors
///
/// [`LaunchError`] for an invalid prompt or a path that is not UTF-8.
pub fn codex_headless_argv(
    program: &Path,
    cfg: &RunnerCodexConfig,
    mcp_server: &Path,
    mcp_toml: &Path,
    prompt: &str,
) -> Result<Vec<String>, LaunchError> {
    check_prompt(prompt)?;
    let mut argv = vec![
        utf8(program)?,
        "exec".to_string(),
        "--skip-git-repo-check".to_string(),
        "-c".to_string(),
        mcp_command_override(mcp_server)?,
        "-c".to_string(),
        mcp_args_override(mcp_toml)?,
    ];
    argv.extend(cfg.headless_args.iter().cloned());
    argv.push(prompt.to_string());
    Ok(argv)
}

/// The argument vector of a headless opencode run: `opencode run <prompt>`; it follows its own
/// configuration.
///
/// # Errors
///
/// [`LaunchError`] for an invalid prompt or a path that is not UTF-8.
pub fn opencode_headless_argv(program: &Path, prompt: &str) -> Result<Vec<String>, LaunchError> {
    check_prompt(prompt)?;
    Ok(vec![utf8(program)?, "run".to_string(), prompt.to_string()])
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
pub fn claude_hook_command(files: &LaunchFiles<'_>, event: &str) -> Result<String, LaunchError> {
    Ok(hook_words(files, "claude", event)?
        .iter()
        .map(|word| shell_quote(word))
        .collect::<Vec<_>>()
        .join(" "))
}
