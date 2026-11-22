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
//! - **Nothing from the project.** A repository must not add allow rules, hooks or MCP servers to
//!   a session. claude is therefore started with flags that stop it loading them (claude
//!   2.1.330, `claude --help`; the help text does not say in which version each flag appeared):
//!   `--strict-mcp-config` ("only use MCP servers from --mcp-config, ignoring all other MCP
//!   configurations": no `.mcp.json`, no user or project server), `--mcp-config <runner file>`,
//!   `--setting-sources user` ("comma-separated list of setting sources to load (user, project,
//!   local)": the committed `.claude/settings.json` and `.claude/settings.local.json` are not
//!   loaded; only the runner user's own `~/.claude/settings.json` is), `--allowedTools <explicit
//!   list>`, and `--permission-mode`. What stays: the operator's own user settings, which
//!   `--help` offers no flag to switch off separately. codex has no verified equivalent, so a
//!   headless codex or opencode run is refused in a directory that holds the CLI's own
//!   configuration unless the operator trusts the project (`[runner] trust_project_files`).
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

/// What the CLI must never read.
///
/// The state directory (every bot's token) and the configuration file. Each is denied in the form
/// it is configured and in its canonical form, so a link in the path does not make the rule miss.
#[derive(Clone, Copy, Debug)]
pub struct Protected<'a> {
    pub state_dir: &'a Path,
    pub config_file: Option<&'a Path>,
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
    /// The state directory: held back from the CLI's reads (it holds every bot's token).
    pub state_dir: &'a Path,
    /// The configuration file (it holds every token): denied to the CLI as well.
    pub config_file: Option<&'a Path>,
    /// The per-launch secret the hooks present (32 hex characters, random, never from task
    /// content): it binds a hook message to the launch that wrote it.
    pub nonce: &'a str,
}

fn permissions(settings: &crate::executor_permissions::ExecutorSettings) -> Result<ExecutorPermissions, LaunchError> {
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
    let ClaudeParts {
        mode,
        allowed,
        disallowed,
    } = claude_parts(
        cfg,
        dir,
        &Protected {
            state_dir: files.state_dir,
            config_file: files.config_file,
        },
    )?;
    let path = utf8(files.claude_mcp_json)?;
    let mut argv = vec![
        utf8(program)?,
        "--permission-mode".to_string(),
        mode,
        "--mcp-config".to_string(),
        path,
        "--strict-mcp-config".to_string(),
        "--setting-sources".to_string(),
        "user".to_string(),
        "--settings".to_string(),
        utf8(files.claude_settings)?,
        "--allowedTools".to_string(),
    ];
    argv.extend(allowed);
    if !disallowed.is_empty() {
        argv.push("--disallowedTools".to_string());
        argv.extend(disallowed);
    }
    Ok(argv)
}

/// The argument vector of a headless claude run.
///
/// `-p <prompt>` first, then the same posture as the interactive session (runner MCP file only, no
/// project settings, explicit allow list), without the hook settings (a headless run reports
/// nothing through hooks).
///
/// # Errors
///
/// [`LaunchError`] for an unusable directory, an invalid prompt or invalid permission settings.
pub fn claude_headless_argv(
    program: &Path,
    cfg: &RunnerClaudeConfig,
    dir: &Path,
    mcp_config: &Path,
    protected: &Protected<'_>,
    prompt: &str,
) -> Result<Vec<String>, LaunchError> {
    let ClaudeParts {
        mode,
        allowed,
        disallowed,
    } = claude_parts(cfg, dir, protected)?;
    check_prompt(prompt)?;
    // `-p` and the prompt come first, so no variadic list below can swallow the prompt.
    let mut argv = vec![
        utf8(program)?,
        "-p".to_string(),
        prompt.to_string(),
        "--permission-mode".to_string(),
        mode,
        "--mcp-config".to_string(),
        utf8(mcp_config)?,
        "--strict-mcp-config".to_string(),
        "--setting-sources".to_string(),
        "user".to_string(),
        "--allowedTools".to_string(),
    ];
    argv.extend(allowed);
    if !disallowed.is_empty() {
        argv.push("--disallowedTools".to_string());
        argv.extend(disallowed);
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

struct ClaudeParts {
    mode: String,
    allowed: Vec<String>,
    disallowed: Vec<String>,
}

/// The permission mode and the tool lists of claude: the design's default list, plus
/// `sylvode_write` with `allow_write_call`, plus the configured extras (`{dir}` expanded).
fn claude_parts(cfg: &RunnerClaudeConfig, dir: &Path, protected: &Protected<'_>) -> Result<ClaudeParts, LaunchError> {
    let dir_text = rule_safe_dir(dir)?;
    // The configured form must be expressible in a rule; the canonical form is added when it is.
    let mut state_texts = vec![rule_safe_dir(protected.state_dir)?.to_string()];
    if let Ok(canonical) = protected.state_dir.canonicalize()
        && let Ok(text) = rule_safe_dir(&canonical)
        && !state_texts.iter().any(|known| known == text)
    {
        state_texts.push(text.to_string());
    }
    let mut file_texts: Vec<String> = Vec::new();
    if let Some(file) = protected.config_file {
        file_texts.push(rule_safe_dir(file)?.to_string());
        if let Ok(canonical) = file.canonicalize()
            && let Ok(text) = rule_safe_dir(&canonical)
            && !file_texts.iter().any(|known| known == text)
        {
            file_texts.push(text.to_string());
        }
    }
    // The same validation the headless channel applies.
    let mut settings = crate::executor_permissions::ExecutorSettings::new("claude-code");
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
    // The state directory is never readable by a task, whatever the allow rules say (a deny rule
    // wins): it holds the token of every bot.
    let mut disallowed: Vec<String> = state_texts
        .iter()
        .flat_map(|text| ["Read", "Glob", "Grep"].map(|tool| format!("{tool}(/{text}/**)")))
        .collect();
    disallowed.extend(file_texts.iter().map(|text| format!("Read(/{text})")));
    disallowed.extend(cfg.disallowed_tools.iter().map(expand));
    for entry in allowed.iter().chain(&disallowed) {
        if entry.is_empty() || entry.starts_with('-') || entry.chars().any(char::is_control) {
            return Err(LaunchError::Permissions(
                "a tool entry is empty, starts with `-` or has control characters".to_string(),
            ));
        }
    }
    Ok(ClaudeParts {
        mode: permissions.mode.to_string(),
        allowed,
        disallowed,
    })
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
    if files.nonce.len() != 32 || !files.nonce.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err(LaunchError::Unsafe(
            "the hook nonce is not 32 hex characters".to_string(),
        ));
    }
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
        "--nonce".to_string(),
        files.nonce.to_string(),
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
    let mut settings = crate::executor_permissions::ExecutorSettings::new("codex");
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

/// The argument vector of a headless codex run: `codex exec` with the sandbox and the runner's
/// MCP server, the prompt last.
///
/// # Errors
///
/// [`LaunchError`] for an unusable directory, an invalid prompt or invalid permission settings.
pub fn codex_headless_argv(
    program: &Path,
    cfg: &RunnerCodexConfig,
    dir: &Path,
    mcp_server: &Path,
    mcp_toml: &Path,
    prompt: &str,
) -> Result<Vec<String>, LaunchError> {
    rule_safe_dir(dir)?;
    let mut settings = crate::executor_permissions::ExecutorSettings::new("codex");
    settings.sandbox = Some(cfg.sandbox.clone());
    settings.dangerously_bypass_permissions = cfg.dangerously_bypass_permissions;
    let ExecutorPermissions::Codex(CodexPermissions::Sandbox(sandbox)) = permissions(&settings)? else {
        return Err(LaunchError::Permissions(
            "codex is launched with a sandbox only".to_string(),
        ));
    };
    check_prompt(prompt)?;
    let server_args = vec![
        "serve".to_string(),
        "--transport".to_string(),
        "stdio".to_string(),
        "--config".to_string(),
        utf8(mcp_toml)?,
    ];
    Ok(vec![
        utf8(program)?,
        "exec".to_string(),
        "--sandbox".to_string(),
        sandbox.to_string(),
        "--skip-git-repo-check".to_string(),
        "-c".to_string(),
        format!("mcp_servers.{SERVER_NAME}.command={}", toml_string(&utf8(mcp_server)?)),
        "-c".to_string(),
        format!("mcp_servers.{SERVER_NAME}.args={}", toml_array(&server_args)),
        prompt.to_string(),
    ])
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
