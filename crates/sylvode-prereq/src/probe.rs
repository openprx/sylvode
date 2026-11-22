//! Probes of real programs: tmux, the agent CLIs and the two sylvode binaries.
//!
//! A probe runs `<program> --version` (or `--check`), with a time limit, and keeps only what it
//! can parse: a version number and a protocol number. The raw output is never copied into a
//! [`Component`], so nothing a program prints (a token in a banner, say) reaches the report.

use std::ffi::{OsStr, OsString};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use serde_json::{Value, json};

use crate::component::{Component, Status, moved_binary_hint, tmux_hint};
use crate::severity::{ComponentKind, Finding, InstallKind, severity};

/// How long a probed program may run before it is killed.
pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(5);

/// The oldest tmux the runner supports (design section 7.10).
pub const TMUX_MIN_VERSION: (u64, u64) = (3, 2);

/// The default tmux socket name of the runner (`tmux -L sylvode`).
pub const DEFAULT_TMUX_SOCKET: &str = "sylvode";

/// At most this much of a probed program's output is read.
const OUTPUT_LIMIT: u64 = 64 * 1024;

const POLL_INTERVAL: Duration = Duration::from_millis(10);

/// The CLI agents a project can select.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Cli {
    Claude,
    Codex,
    Opencode,
}

impl Cli {
    /// The name used in component names and config keys.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Claude => "claude",
            Self::Codex => "codex",
            Self::Opencode => "opencode",
        }
    }
}

/// A binary the checker reports on besides tmux and the CLIs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Peer {
    McpServer,
    Runner,
}

impl Peer {
    const fn component(self) -> &'static str {
        match self {
            Self::McpServer => "mcp_server",
            Self::Runner => "runner",
        }
    }

    const fn binary(self) -> &'static str {
        match self {
            Self::McpServer => "mcp-server",
            Self::Runner => "sylvode-runner",
        }
    }

    const fn kind(self) -> ComponentKind {
        match self {
            Self::McpServer => ComponentKind::McpServer,
            Self::Runner => ComponentKind::Runner,
        }
    }
}

/// What running a program produced.
#[derive(Debug)]
pub enum RunOutcome {
    /// The program could not be started.
    SpawnFailed,
    /// The program was still running at the time limit and was killed.
    TimedOut,
    Exited {
        /// `None` when a signal ended the program.
        code: Option<i32>,
        stdout: String,
    },
}

/// A tmux version. `3.7b` is `Release`, `next-3.5` is a development build on the way to 3.5
/// (compared as 3.5), and `master` is the development head (newer than every minimum).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TmuxVersion {
    pub display: String,
    kind: TmuxKind,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum TmuxKind {
    Numbered { major: u64, minor: u64 },
    Master,
}

impl TmuxVersion {
    /// Whether this version is at least `min` (major, minor).
    #[must_use]
    pub fn meets(&self, min: (u64, u64)) -> bool {
        match self.kind {
            TmuxKind::Master => true,
            TmuxKind::Numbered { major, minor } => {
                semver::Version::new(major, minor, 0) >= semver::Version::new(min.0, min.1, 0)
            }
        }
    }
}

/// Parses the output of `tmux -V`: `tmux 3.7b`, `tmux 3.2a`, `tmux next-3.5`, `tmux master`.
/// Anything else, including a version followed by other text, is `None`.
#[must_use]
pub fn parse_tmux_version(output: &str) -> Option<TmuxVersion> {
    let line = output.lines().next()?.trim();
    let token = line.strip_prefix("tmux ")?.trim();
    if token == "master" {
        return Some(TmuxVersion {
            display: token.to_string(),
            kind: TmuxKind::Master,
        });
    }
    let numbers = token.strip_prefix("next-").unwrap_or(token);
    let (major, rest) = numbers.split_once('.')?;
    let minor_digits: String = rest.chars().take_while(char::is_ascii_digit).collect();
    let suffix = rest.get(minor_digits.len()..)?;
    if suffix.len() > 2 || !suffix.chars().all(|c| c.is_ascii_lowercase()) {
        return None;
    }
    if major.is_empty() || !major.chars().all(|c| c.is_ascii_digit()) || minor_digits.is_empty() {
        return None;
    }
    Some(TmuxVersion {
        display: token.to_string(),
        kind: TmuxKind::Numbered {
            major: major.parse().ok()?,
            minor: minor_digits.parse().ok()?,
        },
    })
}

/// The first `major.minor.patch` number in the first line of `output`, as in `2.1.328 (Claude
/// Code)` or `codex-cli 0.170.0`. Pre-release and build parts are dropped: they are free text.
#[must_use]
pub fn parse_version_token(output: &str) -> Option<String> {
    let line = output.lines().next()?;
    line.split_whitespace().find_map(|word| {
        let version = semver::Version::parse(word.trim_start_matches('v')).ok()?;
        Some(format!("{}.{}.{}", version.major, version.minor, version.patch))
    })
}

fn valid_socket_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 64
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
}

fn is_executable_file(path: &Path) -> bool {
    let Ok(metadata) = path.metadata() else {
        return false;
    };
    if !metadata.is_file() {
        return false;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        metadata.permissions().mode() & 0o111 != 0
    }
    #[cfg(not(unix))]
    {
        true
    }
}

/// Spawns `command`. A program that was written a moment ago can still be reported busy while
/// another thread's `fork` holds the file open for writing; that clears within milliseconds, so it
/// is retried a few times.
/// The variables of the caller's environment a probed program is given (and `LC_*`); `PATH` is set
/// separately. The same list the runner gives every CLI it starts.
const ENV_ALLOWED: [&str; 9] = [
    "HOME",
    "USER",
    "LOGNAME",
    "LANG",
    "LANGUAGE",
    "TERM",
    "TZ",
    "SHELL",
    "TMUX_TMPDIR",
];

fn spawn_retrying(command: &mut Command) -> Option<std::process::Child> {
    for _ in 0..10 {
        match command.spawn() {
            Ok(child) => return Some(child),
            Err(error) if error.kind() == std::io::ErrorKind::ExecutableFileBusy => {
                thread::sleep(Duration::from_millis(20));
            }
            Err(_) => return None,
        }
    }
    None
}

/// Runs the probes for one install kind, against one `PATH`.
///
/// The `PATH` is explicit so the same probes serve the shell and the service unit (`doctor
/// --as-service` evaluates the PATH the installer embeds): the programs are looked up on it and
/// run with it.
#[derive(Clone, Debug)]
pub struct Checker {
    install: InstallKind,
    path: Option<OsString>,
    timeout: Duration,
}

impl Checker {
    /// A checker for `install` that uses the process's `PATH` and [`DEFAULT_TIMEOUT`].
    #[must_use]
    pub const fn new(install: InstallKind) -> Self {
        Self {
            install,
            path: None,
            timeout: DEFAULT_TIMEOUT,
        }
    }

    /// Looks programs up on, and runs them with, this `PATH` instead of the process's.
    #[must_use]
    pub fn with_path(mut self, path: impl Into<OsString>) -> Self {
        self.path = Some(path.into());
        self
    }

    /// Kills a probed program after `timeout` instead of [`DEFAULT_TIMEOUT`].
    #[must_use]
    pub const fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }

    /// The install kind the findings are judged for.
    #[must_use]
    pub const fn install(&self) -> InstallKind {
        self.install
    }

    fn path_var(&self) -> Option<OsString> {
        self.path.clone().or_else(|| std::env::var_os("PATH"))
    }

    /// The absolute path of `name`: itself when it is a path to an executable file, else the first
    /// executable file of that name in an absolute directory of the `PATH`.
    #[must_use]
    pub fn locate(&self, name: &str) -> Option<PathBuf> {
        if name.contains('/') {
            let path = PathBuf::from(name);
            return (path.is_absolute() && is_executable_file(&path)).then_some(path);
        }
        let path_var = self.path_var()?;
        std::env::split_paths(&path_var)
            .filter(|dir| dir.is_absolute())
            .map(|dir| dir.join(name))
            .find(|candidate| is_executable_file(candidate))
    }

    fn locate_override(&self, default: &str, binary: Option<&Path>) -> Option<PathBuf> {
        binary.map_or_else(|| self.locate(default), |path| self.locate(&path.to_string_lossy()))
    }

    /// Runs `program args...` with the checker's `PATH`, no stdin and the time limit. At most 64 KiB
    /// of standard output is kept; standard error is dropped.
    #[must_use]
    pub fn run<S: AsRef<OsStr>>(&self, program: &Path, args: &[S]) -> RunOutcome {
        let mut command = Command::new(program);
        command
            .args(args)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null());
        // A probed program gets the allow-list environment and nothing else of the caller's: the
        // caller may hold tokens that no probe has any business seeing.
        command.env_clear();
        for (name, value) in std::env::vars_os() {
            let text = name.to_string_lossy();
            if ENV_ALLOWED.contains(&&*text) || text.starts_with("LC_") {
                command.env(&name, value);
            }
        }
        if let Some(path) = self.path_var() {
            command.env("PATH", path);
        }
        let Some(mut child) = spawn_retrying(&mut command) else {
            return RunOutcome::SpawnFailed;
        };
        let Some(stdout) = child.stdout.take() else {
            let _ = child.kill();
            let _ = child.wait();
            return RunOutcome::SpawnFailed;
        };
        let reader = thread::spawn(move || {
            let mut bytes = Vec::new();
            let _ = stdout.take(OUTPUT_LIMIT).read_to_end(&mut bytes);
            bytes
        });
        let deadline = Instant::now() + self.timeout;
        let status = loop {
            match child.try_wait() {
                Ok(Some(status)) => break Some(status),
                Ok(None) if Instant::now() < deadline => thread::sleep(POLL_INTERVAL),
                Ok(None) | Err(_) => break None,
            }
        };
        let Some(status) = status else {
            let _ = child.kill();
            let _ = child.wait();
            // The reader ends when the pipe closes; it is not joined, in case a grandchild
            // that outlived the kill still holds the pipe.
            drop(reader);
            return RunOutcome::TimedOut;
        };
        let bytes = reader.join().unwrap_or_default();
        RunOutcome::Exited {
            code: status.code(),
            stdout: String::from_utf8_lossy(&bytes).into_owned(),
        }
    }

    fn status(&self, component: ComponentKind, finding: Finding) -> (Status, bool) {
        (
            severity(self.install, component, finding),
            severity(self.install, component, Finding::Missing) == Status::Fail,
        )
    }

    /// The `tmux` row: found, version against [`TMUX_MIN_VERSION`], and whether a server runs on
    /// `socket` (`tmux -L <socket> list-sessions` exits 0; this never starts a server).
    #[must_use]
    pub fn check_tmux(&self, socket: &str) -> Component {
        let min = format!("{}.{}", TMUX_MIN_VERSION.0, TMUX_MIN_VERSION.1);
        // An empty name is the operator's default tmux server: no `-L`.
        let socket_ok = socket.is_empty() || valid_socket_name(socket);
        let detail_socket = match (socket_ok, socket.is_empty()) {
            (false, _) => "invalid",
            (true, true) => "default",
            (true, false) => socket,
        };
        let Some(path) = self.locate("tmux") else {
            let (status, required) = self.status(ComponentKind::Tmux, Finding::Missing);
            return Component::new("tmux", false, status, required)
                .with_hint(tmux_hint(&min))
                .with_detail("min_version", json!(min))
                .with_detail("socket", json!(detail_socket))
                .with_detail("server_running", json!(false));
        };
        let parsed = match self.run(&path, &["-V"]) {
            RunOutcome::Exited { code: Some(0), stdout } => parse_tmux_version(&stdout),
            _ => None,
        };
        let finding = match &parsed {
            None => Finding::Unparsable,
            Some(version) if version.meets(TMUX_MIN_VERSION) => Finding::Healthy,
            Some(_) => Finding::TooOld,
        };
        let (status, required) = self.status(ComponentKind::Tmux, finding);
        let server_running = socket_ok
            && matches!(
                if socket.is_empty() {
                    self.run(&path, &["list-sessions"])
                } else {
                    self.run(&path, &["-L", socket, "list-sessions"])
                },
                RunOutcome::Exited { code: Some(0), .. }
            );
        let mut component = Component::new("tmux", true, status, required)
            .with_detail("min_version", json!(min))
            .with_detail("socket", json!(detail_socket))
            .with_detail("server_running", json!(server_running));
        component.path = Some(path.to_string_lossy().into_owned());
        component.version = parsed.map(|version| version.display);
        match finding {
            Finding::TooOld => component.hint = Some(tmux_hint(&min)),
            Finding::Unparsable => component.hint = Some("could not read the tmux version from `tmux -V`".to_string()),
            _ => {}
        }
        component
    }

    /// The `cli.<name>` row from `<cli> --version`. Only a `selected` CLI can fail; any other is
    /// reported as information. opencode is never verified: it is `warn` with
    /// `detail.state = "unverified"` (`info` when not selected).
    #[must_use]
    pub fn check_cli(&self, cli: Cli, selected: bool, binary: Option<&Path>) -> Component {
        let name = format!("cli.{}", cli.name());
        let judge = |finding: Finding| -> (Status, bool) {
            if selected {
                self.status(ComponentKind::Cli, finding)
            } else {
                (
                    if finding == Finding::Healthy {
                        Status::Ok
                    } else {
                        Status::Info
                    },
                    false,
                )
            }
        };
        let Some(path) = self.locate_override(cli.name(), binary) else {
            let (status, required) = judge(Finding::Missing);
            return Component::new(name, false, status, required).with_hint(moved_binary_hint(cli.name()));
        };
        let version = match self.run(&path, &["--version"]) {
            RunOutcome::Exited { code: Some(0), stdout } => parse_version_token(&stdout),
            _ => None,
        };
        let finding = match (cli, &version) {
            (_, None) => Finding::Unparsable,
            (Cli::Opencode, Some(_)) => Finding::Unverified,
            (_, Some(_)) => Finding::Healthy,
        };
        let (status, required) = judge(finding);
        let mut component = Component::new(name, true, status, required);
        component.path = Some(path.to_string_lossy().into_owned());
        component.version = version;
        if cli == Cli::Opencode {
            component = component.with_detail("state", json!("unverified"));
            if finding == Finding::Unverified {
                component.hint = Some("opencode integration is unverified".to_string());
            }
        }
        if finding == Finding::Unparsable {
            component.hint = Some(format!("could not read the version from `{} --version`", cli.name()));
        }
        component
    }

    /// The `mcp_server` or `runner` row from `<binary> --check`, which prints a
    /// `sylvode.doctor.v1` report: its `generated_by.version` and `generated_by.task_protocol`
    /// are reported as `version` and `detail.task_protocol`. When `own_protocol` is given, a peer
    /// on another protocol is a [`Finding::ProtocolMismatch`].
    #[must_use]
    pub fn check_peer(&self, peer: Peer, binary: Option<&Path>, own_protocol: Option<u32>) -> Component {
        let kind = peer.kind();
        let Some(path) = self.locate_override(peer.binary(), binary) else {
            let (status, required) = self.status(kind, Finding::Missing);
            let hint = match self.install {
                InstallKind::McpServerAlone if peer == Peer::Runner => "task delivery needs sylvode-runner and tmux",
                _ => "binary not found; run its installer or put it on PATH",
            };
            return Component::new(peer.component(), false, status, required).with_hint(hint);
        };
        let reported = match self.run(&path, &["--check"]) {
            // 2 means the peer's own checks found a failure; its report is still complete.
            RunOutcome::Exited {
                code: Some(0 | 2),
                stdout,
            } => parse_check_output(&stdout),
            _ => None,
        };
        let mut hint = None;
        let finding = match (&reported, own_protocol) {
            (None, _) => {
                hint = Some(format!("`{} --check` gave no usable report", peer.binary()));
                Finding::Unparsable
            }
            (Some((_, protocol)), Some(own)) if *protocol != u64::from(own) => {
                hint = Some(format!(
                    "task protocol mismatch: this binary {own}, {} {protocol}; upgrade {}",
                    peer.binary(),
                    peer.binary()
                ));
                Finding::ProtocolMismatch
            }
            (Some(_), _) => Finding::Healthy,
        };
        let (status, required) = self.status(kind, finding);
        let mut component = Component::new(peer.component(), true, status, required);
        component.path = Some(path.to_string_lossy().into_owned());
        component.hint = hint;
        if let Some((version, protocol)) = reported {
            component.version = Some(version);
            component = component.with_detail("task_protocol", json!(protocol));
        }
        component
    }
}

/// Reads `generated_by.version` and `generated_by.task_protocol` from a peer's `--check` output.
/// The version must be a plain semantic version; free text in it is refused.
fn parse_check_output(output: &str) -> Option<(String, u64)> {
    let value: Value = serde_json::from_str(output).ok()?;
    let generated_by = value.get("generated_by")?;
    let version = generated_by.get("version")?.as_str()?;
    let parsed = semver::Version::parse(version).ok()?;
    let protocol = generated_by.get("task_protocol")?.as_u64()?;
    Some((format!("{}.{}.{}", parsed.major, parsed.minor, parsed.patch), protocol))
}

/// The `api_protocol` row: `GET <base_url>/api/v1/ai/protocol` with the bot token.
///
/// The token and the URL are used for the request only; neither is ever put in the row, and no
/// error text is copied: a failure is `detail.state` (`unreachable`, `unauthorized`,
/// `bad_answer`, `invalid_url`).
#[cfg(feature = "http")]
pub mod api {
    use std::time::Duration;

    use serde_json::{Value, json};

    use crate::component::{Component, Status};
    use crate::severity::{ComponentKind, Finding, InstallKind, severity};

    /// What to send to the API.
    #[derive(Clone, Copy, Debug)]
    pub struct ApiProbe<'a> {
        pub base_url: &'a str,
        pub token: &'a str,
        /// The `X-Sylvode-MCP-Surface` value, when the bot has one other than `rest`.
        pub surface: Option<&'a str>,
        pub timeout: Duration,
        /// The task protocol of this binary.
        pub own_protocol: u32,
    }

    fn row(install: InstallKind, finding: Finding, state: &str) -> Component {
        let status = severity(install, ComponentKind::ApiProtocol, finding);
        let required = severity(install, ComponentKind::ApiProtocol, Finding::Missing) == Status::Fail;
        Component::new("api_protocol", finding != Finding::Unparsable, status, required)
            .with_detail("state", json!(state))
    }

    /// Probes the API. Redirects are not followed, so the token never leaves the configured host.
    pub async fn check_api_protocol(install: InstallKind, probe: &ApiProbe<'_>) -> Component {
        let url = format!("{}/api/v1/ai/protocol", probe.base_url.trim_end_matches('/'));
        let Ok(client) = reqwest::Client::builder()
            .timeout(probe.timeout)
            .redirect(reqwest::redirect::Policy::none())
            .build()
        else {
            return row(install, Finding::Unparsable, "unreachable");
        };
        if !(probe.base_url.starts_with("http://") || probe.base_url.starts_with("https://")) {
            return row(install, Finding::Unparsable, "invalid_url");
        }
        let mut request = client.get(&url).bearer_auth(probe.token);
        if let Some(surface) = probe.surface {
            request = request.header("X-Sylvode-MCP-Surface", surface);
        }
        let Ok(response) = request.send().await else {
            return row(install, Finding::Unparsable, "unreachable");
        };
        let code = response.status();
        if code == reqwest::StatusCode::UNAUTHORIZED || code == reqwest::StatusCode::FORBIDDEN {
            return row(install, Finding::Unparsable, "unauthorized");
        }
        let body: Option<Value> = if code.is_success() {
            response.json().await.ok()
        } else {
            None
        };
        let data = body.as_ref().and_then(|body| body.get("data"));
        let protocol = data.and_then(|data| data.get("task_protocol")).and_then(Value::as_u64);
        let version = data
            .and_then(|data| data.get("api_version"))
            .and_then(Value::as_str)
            .and_then(|version| semver::Version::parse(version).ok())
            .map(|version| format!("{}.{}.{}", version.major, version.minor, version.patch));
        let Some(protocol) = protocol else {
            return row(install, Finding::Unparsable, "bad_answer");
        };
        let own = u64::from(probe.own_protocol);
        let finding = if protocol == own {
            Finding::Healthy
        } else {
            Finding::ProtocolMismatch
        };
        let mut component = row(install, finding, "answered").with_detail("task_protocol", json!(protocol));
        component.version = version;
        if finding == Finding::ProtocolMismatch {
            component.hint = Some(format!(
                "task protocol mismatch: this binary {own}, api {protocol}; upgrade the older component"
            ));
        }
        component
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tmux_versions_parse() {
        for (output, display, meets_3_2) in [
            ("tmux 3.7b\n", "3.7b", true),
            ("tmux 3.2a", "3.2a", true),
            ("tmux 3.2", "3.2", true),
            ("tmux 3.1c", "3.1c", false),
            ("tmux 3.1", "3.1", false),
            ("tmux 2.9a", "2.9a", false),
            ("tmux 10.0", "10.0", true),
            ("tmux next-3.5", "next-3.5", true),
            ("tmux next-3.1", "next-3.1", false),
            ("tmux master", "master", true),
        ] {
            let version = parse_tmux_version(output).unwrap_or_else(|| panic!("{output} parses"));
            assert_eq!(version.display, display, "{output}");
            assert_eq!(version.meets((3, 2)), meets_3_2, "{output}");
        }
    }

    #[test]
    fn unreadable_tmux_output_is_refused() {
        for output in [
            "",
            "tmux",
            "tmux ",
            "3.7b",
            "tmux three",
            "tmux 3",
            "tmux 3.",
            "tmux 3.x",
            "tmux 3.7B",
            "tmux 3.7bcd",
            "tmux 3.7b extra",
            "tmux next-",
            "tmux master-2",
            "usage: tmux [-2CDlNuVv]",
            "tmux: command not found",
        ] {
            assert_eq!(parse_tmux_version(output), None, "{output:?}");
        }
    }

    #[test]
    fn cli_versions_parse() {
        for (output, want) in [
            ("2.1.328 (Claude Code)\n", Some("2.1.328")),
            ("codex-cli 0.170.0", Some("0.170.0")),
            ("v1.4.2", Some("1.4.2")),
            ("tool 1.0.0-rc.1+build.7", Some("1.0.0")),
            ("", None),
            ("no digits here", None),
            ("1.2", None),
            ("usage: claude [options]\n2.1.328", None),
        ] {
            assert_eq!(parse_version_token(output).as_deref(), want, "{output:?}");
        }
    }

    #[test]
    fn peer_reports_need_a_plain_version_and_a_number() {
        let good = r#"{"schema":"sylvode.doctor.v1","generated_by":{"binary":"mcp-server","version":"0.5.0","task_protocol":1}}"#;
        assert_eq!(parse_check_output(good), Some(("0.5.0".to_string(), 1)));
        for bad in [
            "",
            "not json",
            "{}",
            r#"{"generated_by":{"version":"0.5.0"}}"#,
            r#"{"generated_by":{"version":"nope","task_protocol":1}}"#,
            r#"{"generated_by":{"version":"0.5.0","task_protocol":"1"}}"#,
        ] {
            assert_eq!(parse_check_output(bad), None, "{bad:?}");
        }
    }

    #[test]
    fn socket_names_are_restricted() {
        assert!(valid_socket_name("sylvode"));
        assert!(valid_socket_name("a-b_c.1"));
        for name in ["", "a b", "a/b", "-x;", "é"] {
            assert!(!valid_socket_name(name), "{name:?}");
        }
        assert!(!valid_socket_name(&"a".repeat(65)));
    }

    #[test]
    fn the_default_time_limit_is_five_seconds() {
        assert_eq!(DEFAULT_TIMEOUT, Duration::from_secs(5));
    }
}
