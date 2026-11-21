//! The prerequisite checker shared by `sylvode-runner` and `mcp-server` (`doctor`, `--check`).
//!
//! It probes tmux, the agent CLIs and the two sylvode binaries, judges each finding with the
//! severity matrix of the install kind, and renders the `sylvode.doctor.v1` report. It depends
//! on `std`, `serde`, `serde_json` and `semver` only; the feature `http` adds the API protocol
//! probe.
//!
//! The report is meant to be pasted into bug reports: binary paths appear, project paths appear
//! only as labels unless the caller asks for them, and nothing a probed program prints is copied
//! into it except a parsed version and protocol number.

pub mod component;
pub mod probe;
pub mod report;
pub mod severity;

pub use component::{
    AgentGuide, Component, DirectoryState, McpScope, McpState, ProjectCheck, Status, Trusted, directory_state,
    mcp_registration_component, project_component,
};
pub use probe::{Checker, Cli, DEFAULT_TIMEOUT, Peer, RunOutcome, TMUX_MIN_VERSION, TmuxVersion};
pub use report::{
    ConfigState, Context, EXIT_CHECKER_ERROR, EXIT_FAILED, EXIT_OK, GeneratedBy, Host, Mode, PathSource, Report,
    SCHEMA, ServiceManager, Summary, VERSION, in_container, path_source, service_manager,
};
pub use severity::{ComponentKind, Finding, InstallKind, severity};
