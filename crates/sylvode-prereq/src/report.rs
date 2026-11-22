//! The `sylvode.doctor.v1` report: frozen fields, the text table, the exit code.

use std::fmt::Write as _;
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::component::{Component, Status};
use crate::probe::Checker;

/// The schema name of the report.
pub const SCHEMA: &str = "sylvode.doctor.v1";

/// The version of this build; every binary of the workspace shares the workspace version.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// Exit code: no `fail`.
pub const EXIT_OK: i32 = 0;
/// Exit code: the checker itself could not run.
pub const EXIT_CHECKER_ERROR: i32 = 1;
/// Exit code: at least one `fail`. Warnings never change the exit code.
pub const EXIT_FAILED: i32 = 2;

/// Which binary produced the report.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct GeneratedBy {
    pub binary: String,
    pub version: String,
    pub task_protocol: u32,
}

impl GeneratedBy {
    /// `binary` at the version of this build, speaking `task_protocol`.
    #[must_use]
    pub fn new(binary: &str, task_protocol: u32) -> Self {
        Self {
            binary: binary.to_string(),
            version: VERSION.to_string(),
            task_protocol,
        }
    }
}

/// How the host runs services.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ServiceManager {
    #[serde(rename = "launchd")]
    Launchd,
    #[serde(rename = "systemd-user")]
    SystemdUser,
    #[serde(rename = "none")]
    None,
}

/// The host the checker ran on.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Host {
    pub os: String,
    pub arch: String,
    pub in_container: bool,
    pub service_manager: ServiceManager,
}

impl Host {
    /// The current host. `checker` supplies the `PATH` on which `systemctl` is looked for.
    #[must_use]
    pub fn detect(checker: &Checker) -> Self {
        let os = std::env::consts::OS;
        Self {
            os: os.to_string(),
            arch: std::env::consts::ARCH.to_string(),
            in_container: in_container(),
            service_manager: service_manager(os, checker),
        }
    }
}

/// The service manager of `os`: launchd on macOS; on Linux, systemd user units when `systemctl`
/// is found, else none.
#[must_use]
pub fn service_manager(os: &str, checker: &Checker) -> ServiceManager {
    match os {
        "macos" => ServiceManager::Launchd,
        "linux" if checker.locate("systemctl").is_some() => ServiceManager::SystemdUser,
        _ => ServiceManager::None,
    }
}

/// Whether the process runs in a container: `/.dockerenv` exists or the cgroup of process 1 names
/// a container runtime.
#[must_use]
pub fn in_container() -> bool {
    in_container_at(Path::new("/"))
}

/// [`in_container`] against another root directory.
#[must_use]
pub fn in_container_at(root: &Path) -> bool {
    if root.join(".dockerenv").exists() {
        return true;
    }
    std::fs::read_to_string(root.join("proc/1/cgroup")).is_ok_and(|cgroup| {
        ["docker", "kubepods", "containerd", "libpod", "podman", "lxc"]
            .iter()
            .any(|runtime| cgroup.contains(runtime))
    })
}

/// The channel mode the check was made for.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Mode {
    Tmux,
    Headless,
    Auto,
}

/// Where the `PATH` came from.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum PathSource {
    #[serde(rename = "shell")]
    Shell,
    #[serde(rename = "service-unit")]
    ServiceUnit,
}

/// Whether the configuration file was usable.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ConfigState {
    Found,
    Missing,
    Invalid,
}

/// What the check was made for.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Context {
    pub mode: Mode,
    pub as_service: bool,
    pub path_source: PathSource,
    pub config: ConfigState,
}

/// Counts of component statuses; `info` counts as `ok`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Summary {
    pub ok: usize,
    pub warn: usize,
    pub fail: usize,
}

/// The whole report.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Report {
    pub schema: String,
    pub generated_by: GeneratedBy,
    pub host: Host,
    pub context: Context,
    pub components: Vec<Component>,
    pub summary: Summary,
}

impl Report {
    /// Builds the report and its summary.
    #[must_use]
    pub fn new(generated_by: GeneratedBy, host: Host, context: Context, components: Vec<Component>) -> Self {
        let mut summary = Summary::default();
        for component in &components {
            match component.status {
                Status::Ok | Status::Info => summary.ok += 1,
                Status::Warn => summary.warn += 1,
                Status::Fail => summary.fail += 1,
            }
        }
        Self {
            schema: SCHEMA.to_string(),
            generated_by,
            host,
            context,
            components,
            summary,
        }
    }

    /// `0` when no component failed, `2` otherwise. Warnings never change it. `1` is for a
    /// checker that could not run and is never returned for a report that exists.
    #[must_use]
    pub const fn exit_code(&self) -> i32 {
        if self.summary.fail > 0 { EXIT_FAILED } else { EXIT_OK }
    }

    /// The report as pretty-printed JSON (`--json`).
    ///
    /// # Errors
    ///
    /// Only if serialisation fails, which the field types do not allow in practice.
    pub fn to_json(&self) -> Result<String, serde_json::Error> {
        serde_json::to_string_pretty(self)
    }

    /// The same rows as a text table, one component per line with its hint below.
    #[must_use]
    pub fn to_table(&self) -> String {
        let width = self
            .components
            .iter()
            .map(|component| component.component.len())
            .max()
            .unwrap_or(0)
            .max("COMPONENT".len());
        let mut out = String::new();
        let _ = writeln!(
            out,
            "{} {} on {}/{}",
            self.generated_by.binary, self.generated_by.version, self.host.os, self.host.arch
        );
        let _ = writeln!(out, "{:<width$}  {:<6}  {:<12}  PATH", "COMPONENT", "STATUS", "VERSION");
        for component in &self.components {
            let status = match component.status {
                Status::Ok => "ok",
                Status::Info => "info",
                Status::Warn => "warn",
                Status::Fail => "fail",
            };
            let version = component.version.as_deref().unwrap_or("-");
            let path = component
                .path
                .as_deref()
                .unwrap_or(if component.found { "-" } else { "not found" });
            let _ = writeln!(
                out,
                "{:<width$}  {status:<6}  {version:<12}  {path}",
                component.component
            );
            if let Some(hint) = &component.hint {
                let _ = writeln!(out, "{:<width$}  hint: {hint}", "");
            }
        }
        let _ = writeln!(
            out,
            "summary: {} ok, {} warn, {} fail",
            self.summary.ok, self.summary.warn, self.summary.fail
        );
        out
    }
}

/// The `PATH` source for a run: the service unit's when `as_service`, else the shell's.
#[must_use]
pub const fn path_source(as_service: bool) -> PathSource {
    if as_service {
        PathSource::ServiceUnit
    } else {
        PathSource::Shell
    }
}
