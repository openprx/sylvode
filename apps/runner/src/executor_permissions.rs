//! Permission settings of the CLI executors, per agent.
//!
//! Each executor has its own permission model, so the settings are executor specific:
//!
//! - `claude-code`: `permission_mode` (`--permission-mode`), `allowed_tools` (`--allowedTools`),
//!   `disallowed_tools` (`--disallowedTools`) and `strict_mcp_config` (`--strict-mcp-config`, only
//!   together with `mcp_config_path`).
//!   The default is `dontAsk`: a tool that would ask for permission is denied without asking, so
//!   the agent can read files in its working directory and use the tools that are allowed, and
//!   nothing else. Without `allowed_tools`, the allowed tools are every MCP server named in
//!   `mcp_config_path` (`mcp__<server>`), so an agent configured with an MCP config can use those
//!   tools non-interactively but cannot run shell commands or edit files unless configured to.
//! - `codex`: `sandbox` (`--sandbox`), default `read-only`.
//! - `opencode`: none. `opencode run` is started as before and follows opencode's own
//!   configuration; setting any of the keys above for it refuses start-up.
//!
//! Turning the executor's permission checks off (`permission_mode = "bypassPermissions"` for
//! claude-code; `--dangerously-bypass-approvals-and-sandbox` or `sandbox = "danger-full-access"`
//! for codex) needs `dangerously_bypass_permissions = true` as well, and logs a warning at
//! start-up.

use std::path::{Path, PathBuf};

use crate::config::CliAgentConfig;

/// `--permission-mode` values of the Claude Code CLI (`claude --help`).
pub const CLAUDE_PERMISSION_MODES: [&str; 6] =
    ["acceptEdits", "auto", "bypassPermissions", "dontAsk", "manual", "plan"];
/// Claude Code permission mode used when `permission_mode` is not set.
pub const DEFAULT_CLAUDE_PERMISSION_MODE: &str = "dontAsk";
const CLAUDE_BYPASS_MODE: &str = "bypassPermissions";

/// `--sandbox` values of `codex exec` (`codex exec --help`).
pub const CODEX_SANDBOX_MODES: [&str; 3] = ["read-only", "workspace-write", "danger-full-access"];
/// Codex sandbox used when `sandbox` is not set.
pub const DEFAULT_CODEX_SANDBOX: &str = "read-only";
const CODEX_FULL_ACCESS: &str = "danger-full-access";

/// The permission settings of one agent's executor, validated.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExecutorPermissions {
    ClaudeCode(ClaudeCodePermissions),
    Codex(CodexPermissions),
    OpenCode,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClaudeCodePermissions {
    pub mode: &'static str,
    /// `None` means the default: every server of `mcp_config_path`.
    pub allowed_tools: Option<Vec<String>>,
    pub disallowed_tools: Vec<String>,
    pub strict_mcp_config: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CodexPermissions {
    Sandbox(&'static str),
    /// `--dangerously-bypass-approvals-and-sandbox`.
    BypassApprovalsAndSandbox,
}

impl ExecutorPermissions {
    /// Validates the permission settings of `cfg` against its executor. An executor outside the
    /// whitelist is refused with the same message the dispatcher has always used.
    pub fn from_config(cfg: &CliAgentConfig) -> Result<Self, String> {
        match cfg.executor.as_str() {
            "claude-code" => claude_code(cfg).map(Self::ClaudeCode),
            "codex" => codex(cfg).map(Self::Codex),
            "opencode" => {
                refuse_set("opencode", &claude_keys(cfg))?;
                refuse_set("opencode", &codex_keys(cfg))?;
                if cfg.dangerously_bypass_permissions {
                    return Err(not_supported("opencode", "dangerously_bypass_permissions"));
                }
                Ok(Self::OpenCode)
            }
            other => Err(format!("executor not allowed: {other}")),
        }
    }

    /// Whether the executor's own permission checks are switched off.
    pub const fn bypasses_permissions(&self) -> bool {
        match self {
            Self::ClaudeCode(p) => matches!(p.mode.as_bytes(), b"bypassPermissions"),
            Self::Codex(CodexPermissions::BypassApprovalsAndSandbox) => true,
            Self::Codex(CodexPermissions::Sandbox(mode)) => matches!(mode.as_bytes(), b"danger-full-access"),
            Self::OpenCode => false,
        }
    }

    /// One line for the start-up log saying how the executor runs.
    pub fn describe(&self) -> String {
        match self {
            Self::ClaudeCode(p) => {
                let allowed = p.allowed_tools.as_ref().map_or_else(
                    || "every server in mcp_config_path".to_string(),
                    |tools| format!("[{}]", tools.join(", ")),
                );
                format!(
                    "claude-code permission_mode = {}, allowed_tools = {allowed}, disallowed_tools = [{}], \
                     strict_mcp_config = {}",
                    p.mode,
                    p.disallowed_tools.join(", "),
                    p.strict_mcp_config
                )
            }
            Self::Codex(CodexPermissions::Sandbox(mode)) => format!("codex sandbox = {mode}"),
            Self::Codex(CodexPermissions::BypassApprovalsAndSandbox) => {
                "codex --dangerously-bypass-approvals-and-sandbox (no sandbox, no approvals)".to_string()
            }
            Self::OpenCode => "opencode with its own permission configuration".to_string(),
        }
    }

    /// The executor's permission arguments, appended after the prompt.
    ///
    /// `mcp_servers` are the server names of `mcp_config_path`, used as the claude-code allow
    /// list when `allowed_tools` is not set.
    pub fn arguments(&self, mcp_config_path: Option<&str>, mcp_servers: &[String]) -> Vec<String> {
        let mut args = Vec::new();
        match self {
            Self::ClaudeCode(p) => {
                args.push("--permission-mode".to_string());
                args.push(p.mode.to_string());
                let allowed: Vec<String> = p
                    .allowed_tools
                    .clone()
                    .unwrap_or_else(|| mcp_servers.iter().map(|server| format!("mcp__{server}")).collect());
                // Each list is one variadic option followed by its entries; the next option ends
                // it, and the prompt is never after it, so no entry can be read as the prompt.
                if !allowed.is_empty() {
                    args.push("--allowedTools".to_string());
                    args.extend(allowed);
                }
                if !p.disallowed_tools.is_empty() {
                    args.push("--disallowedTools".to_string());
                    args.extend(p.disallowed_tools.iter().cloned());
                }
                // Without an MCP config the agent's own Claude Code configuration supplies the
                // servers, so strict mode (which would drop them all) only applies with one.
                if let Some(path) = mcp_config_path {
                    args.push("--mcp-config".to_string());
                    args.push(path.to_string());
                    if p.strict_mcp_config {
                        args.push("--strict-mcp-config".to_string());
                    }
                }
            }
            Self::Codex(CodexPermissions::Sandbox(mode)) => {
                args.push("--sandbox".to_string());
                args.push((*mode).to_string());
            }
            Self::Codex(CodexPermissions::BypassApprovalsAndSandbox) => {
                args.push("--dangerously-bypass-approvals-and-sandbox".to_string());
            }
            Self::OpenCode => {}
        }
        args
    }

    /// Whether building the arguments needs the server names of `mcp_config_path`.
    pub const fn needs_mcp_servers(&self) -> bool {
        matches!(
            self,
            Self::ClaudeCode(ClaudeCodePermissions {
                allowed_tools: None,
                ..
            })
        )
    }
}

fn claude_code(cfg: &CliAgentConfig) -> Result<ClaudeCodePermissions, String> {
    refuse_set("claude-code", &codex_keys(cfg))?;
    let requested = cfg
        .permission_mode
        .as_deref()
        .map_or(DEFAULT_CLAUDE_PERMISSION_MODE, str::trim);
    let mode = CLAUDE_PERMISSION_MODES
        .iter()
        .copied()
        .find(|mode| *mode == requested)
        .ok_or_else(|| {
            format!(
                "cli.permission_mode = {requested:?} is not a claude-code permission mode; valid values: {}",
                CLAUDE_PERMISSION_MODES.join(", ")
            )
        })?;
    if mode == CLAUDE_BYPASS_MODE && !cfg.dangerously_bypass_permissions {
        return Err(format!(
            "cli.permission_mode = \"{CLAUDE_BYPASS_MODE}\" turns every permission check of claude-code off; \
             set cli.dangerously_bypass_permissions = true as well if that is really intended, or use \
             permission_mode = \"{DEFAULT_CLAUDE_PERMISSION_MODE}\" with allowed_tools"
        ));
    }
    if cfg.dangerously_bypass_permissions && mode != CLAUDE_BYPASS_MODE {
        return Err(format!(
            "cli.dangerously_bypass_permissions = true has no effect with permission_mode = \"{mode}\"; \
             remove it, or set permission_mode = \"{CLAUDE_BYPASS_MODE}\""
        ));
    }
    let allowed_tools = cfg
        .allowed_tools
        .as_deref()
        .map(|tools| tool_list("allowed_tools", tools))
        .transpose()?;
    let disallowed_tools = tool_list("disallowed_tools", cfg.disallowed_tools.as_deref().unwrap_or_default())?;
    Ok(ClaudeCodePermissions {
        mode,
        allowed_tools,
        disallowed_tools,
        strict_mcp_config: cfg.strict_mcp_config.unwrap_or(true),
    })
}

fn codex(cfg: &CliAgentConfig) -> Result<CodexPermissions, String> {
    refuse_set("codex", &claude_keys(cfg))?;
    let sandbox = match cfg.sandbox.as_deref().map(str::trim) {
        None => None,
        Some(requested) => Some(
            CODEX_SANDBOX_MODES
                .iter()
                .copied()
                .find(|m| *m == requested)
                .ok_or_else(|| {
                    format!(
                        "cli.sandbox = {requested:?} is not a codex sandbox mode; valid values: {}",
                        CODEX_SANDBOX_MODES.join(", ")
                    )
                })?,
        ),
    };
    match (sandbox, cfg.dangerously_bypass_permissions) {
        (None, false) => Ok(CodexPermissions::Sandbox(DEFAULT_CODEX_SANDBOX)),
        (None, true) => Ok(CodexPermissions::BypassApprovalsAndSandbox),
        (Some(CODEX_FULL_ACCESS), true) => Ok(CodexPermissions::Sandbox(CODEX_FULL_ACCESS)),
        (Some(CODEX_FULL_ACCESS), false) => Err(format!(
            "cli.sandbox = \"{CODEX_FULL_ACCESS}\" removes the codex sandbox; set \
             cli.dangerously_bypass_permissions = true as well if that is really intended"
        )),
        (Some(mode), true) => Err(format!(
            "cli.dangerously_bypass_permissions = true contradicts cli.sandbox = \"{mode}\"; remove one of them"
        )),
        (Some(mode), false) => Ok(CodexPermissions::Sandbox(mode)),
    }
}

const fn claude_keys(cfg: &CliAgentConfig) -> [(&'static str, bool); 4] {
    [
        ("permission_mode", cfg.permission_mode.is_some()),
        ("allowed_tools", cfg.allowed_tools.is_some()),
        ("disallowed_tools", cfg.disallowed_tools.is_some()),
        ("strict_mcp_config", cfg.strict_mcp_config.is_some()),
    ]
}

const fn codex_keys(cfg: &CliAgentConfig) -> [(&'static str, bool); 1] {
    [("sandbox", cfg.sandbox.is_some())]
}

fn refuse_set(executor: &str, keys: &[(&'static str, bool)]) -> Result<(), String> {
    keys.iter()
        .find(|(_, set)| *set)
        .map_or(Ok(()), |(key, _)| Err(not_supported(executor, key)))
}

fn not_supported(executor: &str, key: &str) -> String {
    format!("cli.{key} is not a setting of the {executor} executor; remove it")
}

/// Validates a tool list: every entry is passed to the executor as its own argument, so it may
/// not be blank, start with `-` (it would be read as an option) or contain control characters.
fn tool_list(key: &str, tools: &[String]) -> Result<Vec<String>, String> {
    tools
        .iter()
        .map(|tool| {
            let tool = tool.trim();
            if tool.is_empty() {
                Err(format!("cli.{key} contains a blank entry"))
            } else if tool.starts_with('-') {
                Err(format!(
                    "cli.{key} entry {tool:?} starts with `-` and would be read as an option"
                ))
            } else if tool.chars().any(char::is_control) {
                Err(format!("cli.{key} entry {tool:?} contains a control character"))
            } else {
                Ok(tool.to_string())
            }
        })
        .collect()
}

/// The file `mcp_config_path` names, as the executor resolves it: relative to `workdir` when the
/// path is relative and a `workdir` is set (the executor runs there).
pub fn resolve_mcp_config_path(mcp_config_path: &str, workdir: Option<&str>) -> PathBuf {
    let path = Path::new(mcp_config_path);
    match workdir {
        Some(dir) if path.is_relative() => Path::new(dir).join(path),
        _ => path.to_path_buf(),
    }
}

/// The server names of an MCP config file (`{"mcpServers": {"<name>": {...}}}`).
pub fn mcp_server_names(path: &Path) -> Result<Vec<String>, String> {
    let text = std::fs::read_to_string(path).map_err(|e| format!("cannot read {}: {e}", path.display()))?;
    parse_mcp_server_names(&text).map_err(|e| format!("{}: {e}", path.display()))
}

fn parse_mcp_server_names(text: &str) -> Result<Vec<String>, String> {
    let value: serde_json::Value = serde_json::from_str(text).map_err(|e| format!("not JSON: {e}"))?;
    let servers = value
        .get("mcpServers")
        .and_then(serde_json::Value::as_object)
        .ok_or_else(|| "has no `mcpServers` object".to_string())?;
    let mut names: Vec<String> = servers.keys().cloned().collect();
    names.sort();
    Ok(names)
}

#[cfg(test)]
mod tests {
    use super::{
        ClaudeCodePermissions, CodexPermissions, ExecutorPermissions, parse_mcp_server_names, resolve_mcp_config_path,
    };
    use crate::config::CliAgentConfig;

    fn cli(executor: &str, extra: &str) -> CliAgentConfig {
        toml::from_str(&format!("executor = '{executor}'\n{extra}")).expect("cli config parses")
    }

    fn permissions(executor: &str, extra: &str) -> Result<ExecutorPermissions, String> {
        ExecutorPermissions::from_config(&cli(executor, extra))
    }

    fn args(executor: &str, extra: &str, mcp: Option<&str>, servers: &[&str]) -> Vec<String> {
        let servers: Vec<String> = servers.iter().map(ToString::to_string).collect();
        permissions(executor, extra).expect("valid").arguments(mcp, &servers)
    }

    #[test]
    fn claude_code_defaults_to_dont_ask_with_the_mcp_servers_allowed() {
        assert_eq!(
            args("claude-code", "", Some("mcp.json"), &["sylvode", "docs"]),
            [
                "--permission-mode",
                "dontAsk",
                "--allowedTools",
                "mcp__sylvode",
                "mcp__docs",
                "--mcp-config",
                "mcp.json",
                "--strict-mcp-config"
            ]
        );
        assert_eq!(
            args("claude-code", "", None, &[]),
            ["--permission-mode", "dontAsk"],
            "nothing allowed beyond what dontAsk permits without asking"
        );
    }

    #[test]
    fn claude_code_explicit_lists_replace_the_default() {
        assert_eq!(
            args(
                "claude-code",
                "permission_mode = 'acceptEdits'\nallowed_tools = ['mcp__sylvode', 'Bash(git status)']\n\
                 disallowed_tools = ['WebFetch']\nstrict_mcp_config = false",
                Some("/etc/mcp.json"),
                &["ignored"],
            ),
            [
                "--permission-mode",
                "acceptEdits",
                "--allowedTools",
                "mcp__sylvode",
                "Bash(git status)",
                "--disallowedTools",
                "WebFetch",
                "--mcp-config",
                "/etc/mcp.json"
            ]
        );
        assert_eq!(
            args("claude-code", "allowed_tools = []", Some("m.json"), &["sylvode"]),
            [
                "--permission-mode",
                "dontAsk",
                "--mcp-config",
                "m.json",
                "--strict-mcp-config"
            ],
            "an empty list allows nothing extra, not the default"
        );
    }

    #[test]
    fn claude_code_bypass_needs_the_explicit_flag() {
        let refused = permissions("claude-code", "permission_mode = 'bypassPermissions'").expect_err("refused");
        assert!(refused.contains("dangerously_bypass_permissions"), "{refused}");
        let bypass = permissions(
            "claude-code",
            "permission_mode = 'bypassPermissions'\ndangerously_bypass_permissions = true",
        )
        .expect("explicit opt-in");
        assert!(bypass.bypasses_permissions());
        assert_eq!(bypass.arguments(None, &[]), ["--permission-mode", "bypassPermissions"]);
        let stray = permissions("claude-code", "dangerously_bypass_permissions = true").expect_err("refused");
        assert!(stray.contains("no effect"), "{stray}");
    }

    #[test]
    fn unknown_modes_and_misplaced_keys_are_refused() {
        for (executor, extra, expected) in [
            (
                "claude-code",
                "permission_mode = 'yolo'",
                "not a claude-code permission mode",
            ),
            (
                "claude-code",
                "permission_mode = 'default'",
                "not a claude-code permission mode",
            ),
            (
                "claude-code",
                "sandbox = 'read-only'",
                "not a setting of the claude-code executor",
            ),
            ("claude-code", "allowed_tools = ['--verbose']", "starts with `-`"),
            ("claude-code", "disallowed_tools = ['  ']", "blank entry"),
            ("codex", "sandbox = 'none'", "not a codex sandbox mode"),
            (
                "codex",
                "permission_mode = 'dontAsk'",
                "not a setting of the codex executor",
            ),
            ("codex", "allowed_tools = ['x']", "not a setting of the codex executor"),
            (
                "opencode",
                "permission_mode = 'dontAsk'",
                "not a setting of the opencode executor",
            ),
            (
                "opencode",
                "sandbox = 'read-only'",
                "not a setting of the opencode executor",
            ),
            ("opencode", "dangerously_bypass_permissions = true", "opencode"),
            ("bash", "", "executor not allowed: bash"),
        ] {
            let err = permissions(executor, extra).expect_err(extra);
            assert!(err.contains(expected), "{executor} {extra}: {err}");
        }
    }

    #[test]
    fn codex_defaults_to_a_read_only_sandbox_and_bypass_is_opt_in() {
        assert_eq!(args("codex", "", None, &[]), ["--sandbox", "read-only"]);
        assert_eq!(
            args("codex", "sandbox = 'workspace-write'", None, &[]),
            ["--sandbox", "workspace-write"]
        );
        let full = permissions("codex", "sandbox = 'danger-full-access'").expect_err("refused");
        assert!(full.contains("dangerously_bypass_permissions"), "{full}");
        let full = permissions(
            "codex",
            "sandbox = 'danger-full-access'\ndangerously_bypass_permissions = true",
        )
        .expect("opt-in");
        assert!(full.bypasses_permissions());
        assert_eq!(full.arguments(None, &[]), ["--sandbox", "danger-full-access"]);
        let bypass = permissions("codex", "dangerously_bypass_permissions = true").expect("opt-in");
        assert_eq!(
            bypass,
            ExecutorPermissions::Codex(CodexPermissions::BypassApprovalsAndSandbox)
        );
        assert_eq!(
            bypass.arguments(None, &[]),
            ["--dangerously-bypass-approvals-and-sandbox"]
        );
        let contradiction =
            permissions("codex", "sandbox = 'read-only'\ndangerously_bypass_permissions = true").expect_err("refused");
        assert!(contradiction.contains("contradicts"), "{contradiction}");
    }

    #[test]
    fn opencode_is_unchanged() {
        let opencode = permissions("opencode", "").expect("valid");
        assert_eq!(opencode, ExecutorPermissions::OpenCode);
        assert!(opencode.arguments(Some("m.json"), &["s".to_string()]).is_empty());
        assert!(!opencode.bypasses_permissions());
    }

    #[test]
    fn the_log_line_names_the_mode() {
        let default = permissions("claude-code", "").expect("valid");
        assert!(
            default.describe().contains("permission_mode = dontAsk"),
            "{}",
            default.describe()
        );
        assert!(default.describe().contains("every server in mcp_config_path"));
        assert!(!default.bypasses_permissions());
        assert_eq!(
            permissions("codex", "").expect("valid").describe(),
            "codex sandbox = read-only"
        );
        assert!(matches!(
            default,
            ExecutorPermissions::ClaudeCode(ClaudeCodePermissions {
                strict_mcp_config: true,
                ..
            })
        ));
    }

    #[test]
    fn mcp_server_names_come_from_the_mcp_servers_object() {
        assert_eq!(
            parse_mcp_server_names(r#"{"mcpServers":{"sylvode":{"command":"x"},"a":{}}}"#).expect("parses"),
            ["a", "sylvode"]
        );
        assert!(parse_mcp_server_names(r#"{"servers":{}}"#).is_err());
        assert!(parse_mcp_server_names("not json").is_err());
        assert_eq!(
            resolve_mcp_config_path("mcp.json", Some("/work")),
            std::path::Path::new("/work/mcp.json")
        );
        assert_eq!(
            resolve_mcp_config_path("/abs/mcp.json", Some("/work")),
            std::path::Path::new("/abs/mcp.json")
        );
        assert_eq!(
            resolve_mcp_config_path("mcp.json", None),
            std::path::Path::new("mcp.json")
        );
    }
}
