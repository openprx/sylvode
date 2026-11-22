//! The components of a `sylvode.doctor.v1` report and the builders for the rows that need no
//! probing of their own (MCP registration, project).

use std::path::Path;

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};

use crate::severity::{ComponentKind, Finding, InstallKind, severity};

/// The status of a component. `info` is printed but counts as `ok` in the summary.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Status {
    Ok,
    Info,
    Warn,
    Fail,
}

/// One row of the report. The field set is frozen by `sylvode.doctor.v1`.
///
/// `path` is an absolute binary path and nothing else. No row carries a token, a URL, config
/// contents or user content.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Component {
    pub component: String,
    pub found: bool,
    pub version: Option<String>,
    pub path: Option<String>,
    pub status: Status,
    pub required: bool,
    /// One line, no secrets; per-platform commands where installing helps.
    pub hint: Option<String>,
    pub detail: Value,
}

impl Component {
    /// A row with an empty `detail`.
    #[must_use]
    pub fn new(component: impl Into<String>, found: bool, status: Status, required: bool) -> Self {
        Self {
            component: component.into(),
            found,
            version: None,
            path: None,
            status,
            required,
            hint: None,
            detail: Value::Object(Map::new()),
        }
    }

    /// Adds one `detail` entry.
    #[must_use]
    pub fn with_detail(mut self, key: &str, value: Value) -> Self {
        if let Value::Object(map) = &mut self.detail {
            map.insert(key.to_string(), value);
        }
        self
    }

    /// Sets the hint line.
    #[must_use]
    pub fn with_hint(mut self, hint: impl Into<String>) -> Self {
        self.hint = Some(hint.into());
        self
    }
}

/// The hint for a missing or too old tmux.
#[must_use]
pub fn tmux_hint(min_version: &str) -> String {
    format!(
        "install tmux >= {min_version}: brew install tmux | sudo apt install tmux | sudo dnf install tmux | sudo pacman -S tmux"
    )
}

/// The hint for a CLI that is not found on the PATH the checker used.
#[must_use]
pub fn moved_binary_hint(cli: &str) -> String {
    format!("binary moved; re-run install.sh or set [runner.cli.{cli}] binary to an absolute path")
}

/// How a CLI's MCP registration was found (section 7.11, "what MCP installed means per CLI").
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum McpState {
    Connected,
    BinaryMissing,
    NotRegistered,
    RegisteredFailing,
    /// The runner injects the registration into each session it starts.
    RunnerInjected,
    /// No way to verify the registration is known (opencode).
    Unverified,
}

impl McpState {
    const fn as_str(self) -> &'static str {
        match self {
            Self::Connected => "connected",
            Self::BinaryMissing => "binary_missing",
            Self::NotRegistered => "not_registered",
            Self::RegisteredFailing => "registered_failing",
            Self::RunnerInjected => "runner_injected",
            Self::Unverified => "unverified",
        }
    }

    const fn finding(self) -> Finding {
        match self {
            Self::Connected | Self::RunnerInjected => Finding::Healthy,
            Self::BinaryMissing | Self::NotRegistered | Self::RegisteredFailing => Finding::NotConnected,
            Self::Unverified => Finding::Unverified,
        }
    }
}

/// Where a registration lives.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum McpScope {
    PerSession,
    User,
    Project,
}

impl McpScope {
    const fn as_str(self) -> &'static str {
        match self {
            Self::PerSession => "per-session",
            Self::User => "user",
            Self::Project => "project",
        }
    }
}

/// The `mcp_registration.<cli>` row. `hint` is a one-line, secret-free command the person can run.
#[must_use]
pub fn mcp_registration_component(
    install: InstallKind,
    cli: &str,
    state: McpState,
    scope: McpScope,
    hint: Option<&str>,
) -> Component {
    let status = severity(install, ComponentKind::McpRegistration, state.finding());
    let required = severity(install, ComponentKind::McpRegistration, Finding::NotConnected) == Status::Fail;
    let mut component = Component::new(format!("mcp_registration.{cli}"), true, status, required)
        .with_detail("state", json!(state.as_str()))
        .with_detail("scope", json!(scope.as_str()));
    component.found = matches!(state, McpState::Connected | McpState::RunnerInjected);
    if let Some(hint) = hint {
        component.hint = Some(hint.to_string());
    }
    component
}

/// Where a project's working directory is relative to the runner's allow-list.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DirectoryState {
    InsideAllowlist,
    OutsideAllowlist,
    Missing,
}

impl DirectoryState {
    const fn as_str(self) -> &'static str {
        match self {
            Self::InsideAllowlist => "inside_allowlist",
            Self::OutsideAllowlist => "outside_allowlist",
            Self::Missing => "missing",
        }
    }
}

/// Classifies `directory` against the allow-list `roots`. Both sides are resolved first, so a
/// symbolic link cannot step outside a root. A directory that cannot be resolved is `Missing`.
#[must_use]
pub fn directory_state(directory: &Path, roots: &[&Path]) -> DirectoryState {
    let Ok(resolved) = directory.canonicalize() else {
        return DirectoryState::Missing;
    };
    if !resolved.is_dir() {
        return DirectoryState::Missing;
    }
    let inside = roots
        .iter()
        .filter_map(|root| root.canonicalize().ok())
        .any(|root| resolved.starts_with(root));
    if inside {
        DirectoryState::InsideAllowlist
    } else {
        DirectoryState::OutsideAllowlist
    }
}

/// Whether the CLI trusts the project directory.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Trusted {
    Yes,
    No,
    Unknown,
}

impl Trusted {
    const fn as_str(self) -> &'static str {
        match self {
            Self::Yes => "yes",
            Self::No => "no",
            Self::Unknown => "unknown",
        }
    }
}

/// The state of the agent guide installed in the project.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AgentGuide {
    Current,
    Stale,
    Conflict,
    Absent,
}

impl AgentGuide {
    const fn as_str(self) -> &'static str {
        match self {
            Self::Current => "current",
            Self::Stale => "stale",
            Self::Conflict => "conflict",
            Self::Absent => "absent",
        }
    }
}

/// What is known about one project.
#[derive(Clone, Copy, Debug)]
pub struct ProjectCheck<'a> {
    /// The project key, for example `SVWL`. Only ASCII letters, digits, `-` and `_` are kept.
    pub key: &'a str,
    pub directory: DirectoryState,
    pub trusted: Trusted,
    pub agent_guide: AgentGuide,
    /// The directory itself; printed only when `show_paths` is set.
    pub directory_path: Option<&'a Path>,
}

/// The `project.<key>` row. The directory is a label (`inside_allowlist`, ...), never a path,
/// unless `show_paths` is set, because the output is meant to be pasted into bug reports.
///
/// A directory that is missing or outside the allow-list is `fail` (a delivery would be refused);
/// an untrusted directory or a stale or conflicting guide is `warn`.
#[must_use]
pub fn project_component(check: &ProjectCheck<'_>, show_paths: bool) -> Component {
    let key: String = check
        .key
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_'))
        .take(32)
        .collect();
    let status = if check.directory != DirectoryState::InsideAllowlist {
        Status::Fail
    } else if check.trusted != Trusted::Yes || matches!(check.agent_guide, AgentGuide::Stale | AgentGuide::Conflict) {
        Status::Warn
    } else {
        Status::Ok
    };
    let mut component = Component::new(format!("project.{key}"), true, status, false)
        .with_detail("directory", json!(check.directory.as_str()))
        .with_detail("trusted", json!(check.trusted.as_str()))
        .with_detail("agent_guide", json!(check.agent_guide.as_str()));
    if show_paths && let Some(path) = check.directory_path {
        component = component.with_detail("directory_path", json!(path.to_string_lossy()));
    }
    component
}
