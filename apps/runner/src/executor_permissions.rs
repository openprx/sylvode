//! Permission settings of the CLI executors, as `[runner.cli.*]` gives them to a run.
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

use serde::Deserialize;

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

/// The settings one run of a CLI is validated from: the executor and the `[runner.cli.*]` and
/// `[runner.mcp]` values that decide its permissions. They are not a configuration section of
/// their own.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct ExecutorSettings {
    pub executor: String,
    pub permission_mode: Option<String>,
    pub allowed_tools: Option<Vec<String>>,
    pub disallowed_tools: Option<Vec<String>>,
    pub strict_mcp_config: Option<bool>,
    pub sandbox: Option<String>,
    #[serde(default)]
    pub dangerously_bypass_permissions: bool,
}

impl ExecutorSettings {
    /// Settings for `executor` with every permission key unset.
    pub fn new(executor: &str) -> Self {
        Self {
            executor: executor.to_string(),
            ..Self::default()
        }
    }
}

/// The permission settings of one executor, validated.
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
    pub fn from_config(cfg: &ExecutorSettings) -> Result<Self, String> {
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
}

fn claude_code(cfg: &ExecutorSettings) -> Result<ClaudeCodePermissions, String> {
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

fn codex(cfg: &ExecutorSettings) -> Result<CodexPermissions, String> {
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

const fn claude_keys(cfg: &ExecutorSettings) -> [(&'static str, bool); 4] {
    [
        ("permission_mode", cfg.permission_mode.is_some()),
        ("allowed_tools", cfg.allowed_tools.is_some()),
        ("disallowed_tools", cfg.disallowed_tools.is_some()),
        ("strict_mcp_config", cfg.strict_mcp_config.is_some()),
    ]
}

const fn codex_keys(cfg: &ExecutorSettings) -> [(&'static str, bool); 1] {
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

#[cfg(test)]
mod tests {
    use super::{ClaudeCodePermissions, CodexPermissions, ExecutorPermissions, ExecutorSettings};

    fn cli(executor: &str, extra: &str) -> ExecutorSettings {
        toml::from_str(&format!("executor = '{executor}'\n{extra}")).expect("cli config parses")
    }

    fn permissions(executor: &str, extra: &str) -> Result<ExecutorPermissions, String> {
        ExecutorPermissions::from_config(&cli(executor, extra))
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
        assert_eq!(
            permissions("codex", "").expect("valid"),
            ExecutorPermissions::Codex(CodexPermissions::Sandbox("read-only"))
        );
        assert_eq!(
            permissions("codex", "sandbox = 'workspace-write'").expect("valid"),
            ExecutorPermissions::Codex(CodexPermissions::Sandbox("workspace-write"))
        );
        let full = permissions("codex", "sandbox = 'danger-full-access'").expect_err("refused");
        assert!(full.contains("dangerously_bypass_permissions"), "{full}");
        let full = permissions(
            "codex",
            "sandbox = 'danger-full-access'\ndangerously_bypass_permissions = true",
        )
        .expect("opt-in");
        assert!(full.bypasses_permissions());
        let bypass = permissions("codex", "dangerously_bypass_permissions = true").expect("opt-in");
        assert_eq!(
            bypass,
            ExecutorPermissions::Codex(CodexPermissions::BypassApprovalsAndSandbox)
        );
        let contradiction =
            permissions("codex", "sandbox = 'read-only'\ndangerously_bypass_permissions = true").expect_err("refused");
        assert!(contradiction.contains("contradicts"), "{contradiction}");
    }

    #[test]
    fn opencode_is_unchanged() {
        let opencode = permissions("opencode", "").expect("valid");
        assert_eq!(opencode, ExecutorPermissions::OpenCode);
        assert!(!opencode.bypasses_permissions());
    }

    #[test]
    fn the_default_claude_settings_are_strict_and_do_not_bypass() {
        let default = permissions("claude-code", "").expect("valid");
        assert!(!default.bypasses_permissions());
        assert!(matches!(
            default,
            ExecutorPermissions::ClaudeCode(ClaudeCodePermissions {
                strict_mcp_config: true,
                ..
            })
        ));
    }
}
