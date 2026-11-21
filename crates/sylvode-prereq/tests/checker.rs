//! The checker against fake programs on a temporary `PATH`. Nothing here depends on what is
//! installed on the machine, starts a real tmux or runs a real CLI.
#![cfg(unix)]
// Test helpers outside `#[test]` functions may fail loudly; this is test code only.
#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use serde_json::json;
use sylvode_prereq::{
    AgentGuide, Checker, Cli, Component, ConfigState, Context, DirectoryState, GeneratedBy, Host, InstallKind,
    McpScope, McpState, Mode, PathSource, Peer, ProjectCheck, Report, ServiceManager, Status, Trusted, directory_state,
    mcp_registration_component, project_component, service_manager,
};

static COUNTER: AtomicUsize = AtomicUsize::new(0);

/// A temporary directory that is removed on drop.
struct TempDir(PathBuf);

impl TempDir {
    fn new() -> Self {
        let dir = std::env::temp_dir().join(format!(
            "sylvode-prereq-test-{}-{}",
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&dir).expect("create temp dir");
        Self(dir)
    }

    fn path(&self) -> &Path {
        &self.0
    }

    /// Writes an executable `/bin/sh` script named `name`.
    fn script(&self, name: &str, body: &str) -> PathBuf {
        let path = self.0.join(name);
        std::fs::write(&path, format!("#!/bin/sh\n{body}\n")).expect("write script");
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).expect("chmod");
        path
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn checker(install: InstallKind, dir: &TempDir) -> Checker {
    Checker::new(install).with_path(dir.path())
}

fn fake_tmux(dir: &TempDir, version_line: &str, server_up: bool) {
    let list = if server_up { "exit 0" } else { "exit 1" };
    dir.script(
        "tmux",
        &format!(
            "case \"$1\" in\n-V) echo '{version_line}';;\n-L) [ \"$3\" = list-sessions ] || exit 9; {list};;\nesac"
        ),
    );
}

fn detail<'a>(component: &'a Component, key: &str) -> &'a serde_json::Value {
    component.detail.get(key).unwrap_or(&serde_json::Value::Null)
}

#[test]
fn tmux_found_with_a_running_server() {
    let dir = TempDir::new();
    fake_tmux(&dir, "tmux 3.7b", true);
    let component = checker(InstallKind::RunnerTmux, &dir).check_tmux("sylvode");
    assert_eq!(component.component, "tmux");
    assert!(component.found);
    assert_eq!(component.version.as_deref(), Some("3.7b"));
    assert_eq!(
        component.path.as_deref(),
        Some(dir.path().join("tmux").to_string_lossy().as_ref())
    );
    assert_eq!(component.status, Status::Ok);
    assert!(component.required);
    assert_eq!(component.hint, None);
    assert_eq!(
        component.detail,
        json!({"min_version": "3.2", "socket": "sylvode", "server_running": true})
    );
}

#[test]
fn tmux_without_a_server_is_still_ok() {
    let dir = TempDir::new();
    fake_tmux(&dir, "tmux next-3.5", false);
    let component = checker(InstallKind::RunnerTmux, &dir).check_tmux("sylvode");
    assert_eq!(component.status, Status::Ok);
    assert_eq!(component.version.as_deref(), Some("next-3.5"));
    assert_eq!(detail(&component, "server_running"), &json!(false));
}

#[test]
fn a_missing_tmux_fails_the_tmux_install_and_only_warns_headless() {
    let dir = TempDir::new();
    let component = checker(InstallKind::RunnerTmux, &dir).check_tmux("sylvode");
    assert!(!component.found);
    assert_eq!(component.status, Status::Fail);
    assert!(component.required);
    assert_eq!(component.path, None);
    let hint = component.hint.as_deref().unwrap_or_default();
    assert!(
        hint.starts_with("install tmux >= 3.2: brew install tmux | sudo apt install tmux"),
        "{hint}"
    );

    let headless = checker(InstallKind::RunnerHeadless, &dir).check_tmux("sylvode");
    assert_eq!(headless.status, Status::Warn);
    assert!(!headless.required);
    let forward = checker(InstallKind::RunnerForwardOnly, &dir).check_tmux("sylvode");
    assert_eq!(forward.status, Status::Info);
}

#[test]
fn a_tmux_older_than_the_minimum_fails() {
    let dir = TempDir::new();
    fake_tmux(&dir, "tmux 3.1", true);
    let component = checker(InstallKind::RunnerTmux, &dir).check_tmux("sylvode");
    assert!(component.found);
    assert_eq!(component.version.as_deref(), Some("3.1"));
    assert_eq!(component.status, Status::Fail);
    assert!(component.hint.as_deref().unwrap_or_default().contains("tmux >= 3.2"));
    assert_eq!(
        checker(InstallKind::RunnerHeadless, &dir).check_tmux("sylvode").status,
        Status::Warn
    );
}

#[test]
fn an_unparsable_tmux_answer_fails_without_a_version() {
    let dir = TempDir::new();
    fake_tmux(&dir, "tmux: something else entirely", true);
    let component = checker(InstallKind::RunnerTmux, &dir).check_tmux("sylvode");
    assert!(component.found);
    assert_eq!(component.version, None);
    assert_eq!(component.status, Status::Fail);
    assert!(component.hint.as_deref().unwrap_or_default().contains("tmux -V"));
}

#[test]
fn a_tmux_that_exits_non_zero_is_unparsable() {
    let dir = TempDir::new();
    dir.script("tmux", "echo 'tmux 3.7b'; exit 3");
    let component = checker(InstallKind::RunnerTmux, &dir).check_tmux("sylvode");
    assert_eq!(component.version, None);
    assert_eq!(component.status, Status::Fail);
}

#[test]
fn an_invalid_socket_name_is_not_probed_or_printed() {
    let dir = TempDir::new();
    fake_tmux(&dir, "tmux 3.7b", true);
    let component = checker(InstallKind::RunnerTmux, &dir).check_tmux("a b;rm");
    assert_eq!(detail(&component, "socket"), &json!("invalid"));
    assert_eq!(detail(&component, "server_running"), &json!(false));
}

#[test]
fn a_slow_binary_is_killed_at_the_time_limit() {
    let dir = TempDir::new();
    dir.script("claude", "exec /bin/sleep 30");
    let started = Instant::now();
    let component = checker(InstallKind::RunnerTmux, &dir)
        .with_timeout(Duration::from_millis(300))
        .check_cli(Cli::Claude, true, None);
    assert!(started.elapsed() < Duration::from_secs(5), "{:?}", started.elapsed());
    assert!(component.found);
    assert_eq!(component.version, None);
    // The program exists but cannot be read: a warning, not a failure.
    assert_eq!(component.status, Status::Warn);
}

#[test]
fn cli_versions_are_read_from_each_cli() {
    let dir = TempDir::new();
    dir.script("claude", "echo '2.1.328 (Claude Code)'");
    dir.script("codex", "echo 'codex-cli 0.170.0'");
    let checker = checker(InstallKind::RunnerTmux, &dir);
    let claude = checker.check_cli(Cli::Claude, true, None);
    assert_eq!(claude.component, "cli.claude");
    assert_eq!(claude.version.as_deref(), Some("2.1.328"));
    assert_eq!(claude.status, Status::Ok);
    assert!(claude.required);
    let codex = checker.check_cli(Cli::Codex, true, None);
    assert_eq!(codex.component, "cli.codex");
    assert_eq!(codex.version.as_deref(), Some("0.170.0"));
    assert_eq!(codex.status, Status::Ok);
}

#[test]
fn a_missing_selected_cli_fails_and_an_unselected_one_is_information() {
    let dir = TempDir::new();
    let checker = checker(InstallKind::RunnerTmux, &dir);
    let selected = checker.check_cli(Cli::Claude, true, None);
    assert!(!selected.found);
    assert_eq!(selected.status, Status::Fail);
    assert!(selected.required);
    assert_eq!(
        selected.hint.as_deref(),
        Some("binary moved; re-run install.sh or set [runner.cli.claude] binary to an absolute path")
    );
    let other = checker.check_cli(Cli::Codex, false, None);
    assert_eq!(other.status, Status::Info);
    assert!(!other.required);
}

#[test]
fn a_cli_binary_can_be_given_as_an_absolute_path() {
    let dir = TempDir::new();
    let elsewhere = TempDir::new();
    let tool = elsewhere.script("my-claude", "echo '2.0.0'");
    let component = checker(InstallKind::RunnerTmux, &dir).check_cli(Cli::Claude, true, Some(&tool));
    assert_eq!(component.status, Status::Ok);
    assert_eq!(component.path.as_deref(), Some(tool.to_string_lossy().as_ref()));
}

#[test]
fn opencode_is_unverified() {
    let dir = TempDir::new();
    dir.script("opencode", "echo '1.2.3'");
    let selected = checker(InstallKind::RunnerTmux, &dir).check_cli(Cli::Opencode, true, None);
    assert_eq!(selected.status, Status::Warn);
    assert_eq!(selected.version.as_deref(), Some("1.2.3"));
    assert_eq!(detail(&selected, "state"), &json!("unverified"));
    let other = checker(InstallKind::RunnerTmux, &dir).check_cli(Cli::Opencode, false, None);
    assert_eq!(other.status, Status::Info);
}

const CHECK_OUTPUT: &str = r#"{"schema":"sylvode.doctor.v1","generated_by":{"binary":"mcp-server","version":"0.5.0","task_protocol":1},"components":[]}"#;

#[test]
fn a_peer_binary_reports_its_version_and_protocol() {
    let dir = TempDir::new();
    // Exit code 2 means the peer's own checks failed; its report is still read.
    dir.script(
        "mcp-server",
        &format!("[ \"$1\" = --check ] || exit 9; echo '{CHECK_OUTPUT}'; exit 2"),
    );
    let component = checker(InstallKind::RunnerTmux, &dir).check_peer(Peer::McpServer, None, Some(1));
    assert_eq!(component.component, "mcp_server");
    assert!(component.found);
    assert_eq!(component.version.as_deref(), Some("0.5.0"));
    assert_eq!(detail(&component, "task_protocol"), &json!(1));
    assert_eq!(component.status, Status::Ok);
    assert!(component.required);
}

#[test]
fn a_peer_on_another_protocol_is_a_mismatch() {
    let dir = TempDir::new();
    dir.script("mcp-server", &format!("echo '{CHECK_OUTPUT}'"));
    let tmux = checker(InstallKind::RunnerTmux, &dir).check_peer(Peer::McpServer, None, Some(2));
    assert_eq!(tmux.status, Status::Fail);
    assert!(
        tmux.hint
            .as_deref()
            .unwrap_or_default()
            .contains("task protocol mismatch: this binary 2, mcp-server 1")
    );
    let forward = checker(InstallKind::RunnerForwardOnly, &dir).check_peer(Peer::McpServer, None, Some(2));
    assert_eq!(forward.status, Status::Warn);
}

#[test]
fn a_missing_peer_follows_the_severity_table() {
    let dir = TempDir::new();
    let mcp = checker(InstallKind::RunnerTmux, &dir).check_peer(Peer::McpServer, None, Some(1));
    assert!(!mcp.found);
    assert_eq!(mcp.status, Status::Fail);
    assert!(mcp.required);
    let runner = checker(InstallKind::McpServerAlone, &dir).check_peer(Peer::Runner, None, Some(1));
    assert!(!runner.found);
    assert_eq!(runner.status, Status::Info);
    assert!(!runner.required);
    assert_eq!(
        runner.hint.as_deref(),
        Some("task delivery needs sylvode-runner and tmux")
    );
}

#[test]
fn a_peer_without_a_usable_report_is_at_most_a_warning() {
    let dir = TempDir::new();
    dir.script("sylvode-runner", "echo 'not json'");
    dir.script("mcp-server", "exit 1");
    let alone = checker(InstallKind::McpServerAlone, &dir).check_peer(Peer::Runner, None, Some(1));
    assert!(alone.found);
    assert_eq!(alone.version, None);
    assert_eq!(alone.status, Status::Info);
    let mcp = checker(InstallKind::RunnerTmux, &dir).check_peer(Peer::McpServer, None, Some(1));
    assert!(mcp.found);
    assert_eq!(mcp.version, None);
    assert_eq!(mcp.status, Status::Warn);
}

/// A string shaped like a credential, built at run time so no such literal sits in the source.
fn token_like() -> String {
    format!("{}{}", ["s", "k", "-"].concat(), "Z9".repeat(16))
}

#[test]
fn nothing_a_program_prints_reaches_the_report() {
    let dir = TempDir::new();
    let token = token_like();
    fake_tmux(&dir, &format!("tmux 3.7b {token}"), true);
    dir.script("claude", &format!("echo '2.1.328 (Claude Code) token={token}'"));
    dir.script("codex", &format!("echo 'codex-cli 0.170.0-{token}+{token}'"));
    dir.script("opencode", &format!("echo '{token}'"));
    dir.script(
        "mcp-server",
        &format!(r#"echo '{{"generated_by":{{"version":"0.5.0-{token}","task_protocol":1}},"note":"{token}"}}'"#),
    );
    let checker = checker(InstallKind::RunnerTmux, &dir);
    let components = vec![
        checker.check_tmux("sylvode"),
        checker.check_cli(Cli::Claude, true, None),
        checker.check_cli(Cli::Codex, true, None),
        checker.check_cli(Cli::Opencode, true, None),
        checker.check_peer(Peer::McpServer, None, Some(1)),
    ];
    let report = Report::new(
        GeneratedBy::new("sylvode-runner", 1),
        Host::detect(&checker),
        Context {
            mode: Mode::Tmux,
            as_service: false,
            path_source: PathSource::Shell,
            config: ConfigState::Found,
        },
        components,
    );
    let json = report.to_json().expect("serialises");
    let table = report.to_table();
    assert!(!json.contains(&token), "{json}");
    assert!(!table.contains(&token), "{table}");
    assert!(!json.contains("Z9Z9"), "{json}");
    // What can be parsed is still reported.
    let codex = report
        .components
        .iter()
        .find(|c| c.component == "cli.codex")
        .expect("codex row");
    assert_eq!(codex.version.as_deref(), Some("0.170.0"));
    let mcp = report
        .components
        .iter()
        .find(|c| c.component == "mcp_server")
        .expect("mcp row");
    assert_eq!(mcp.version.as_deref(), Some("0.5.0"));
}

#[test]
fn locate_ignores_relative_path_entries_and_files_that_are_not_executable() {
    let dir = TempDir::new();
    let path = dir.path().join("plain");
    std::fs::write(&path, "data").expect("write");
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).expect("chmod");
    dir.script("real", "exit 0");
    let joined = std::env::join_paths([Path::new("relative-dir"), dir.path()]).expect("join");
    let checker = Checker::new(InstallKind::RunnerTmux).with_path(joined);
    assert_eq!(checker.locate("real"), Some(dir.path().join("real")));
    assert_eq!(checker.locate("plain"), None);
    assert_eq!(checker.locate("absent"), None);
    assert_eq!(
        checker.locate(&dir.path().join("real").to_string_lossy()),
        Some(dir.path().join("real"))
    );
    assert_eq!(checker.locate("sub/real"), None);
}

#[test]
fn service_manager_follows_the_os_and_systemctl() {
    let empty = TempDir::new();
    let with_systemctl = TempDir::new();
    with_systemctl.script("systemctl", "exit 0");
    let without = checker(InstallKind::RunnerTmux, &empty);
    let with = checker(InstallKind::RunnerTmux, &with_systemctl);
    assert_eq!(service_manager("macos", &without), ServiceManager::Launchd);
    assert_eq!(service_manager("linux", &with), ServiceManager::SystemdUser);
    assert_eq!(service_manager("linux", &without), ServiceManager::None);
    assert_eq!(service_manager("windows", &with), ServiceManager::None);
}

#[test]
fn container_detection_reads_dockerenv_and_the_cgroup_of_process_one() {
    let root = TempDir::new();
    assert!(!sylvode_prereq::report::in_container_at(root.path()));
    std::fs::create_dir_all(root.path().join("proc/1")).expect("mkdir");
    std::fs::write(root.path().join("proc/1/cgroup"), "0::/user.slice/session-1.scope\n").expect("write");
    assert!(!sylvode_prereq::report::in_container_at(root.path()));
    std::fs::write(root.path().join("proc/1/cgroup"), "0::/docker/abcdef\n").expect("write");
    assert!(sylvode_prereq::report::in_container_at(root.path()));
    std::fs::remove_file(root.path().join("proc/1/cgroup")).expect("remove");
    std::fs::write(root.path().join(".dockerenv"), "").expect("write");
    assert!(sylvode_prereq::report::in_container_at(root.path()));
}

#[test]
fn the_report_has_the_frozen_fields() {
    let tmux = {
        let mut c = Component::new("tmux", true, Status::Ok, true)
            .with_detail("min_version", json!("3.2"))
            .with_detail("socket", json!("sylvode"))
            .with_detail("server_running", json!(true));
        c.version = Some("3.7b".to_string());
        c.path = Some("/opt/homebrew/bin/tmux".to_string());
        c
    };
    let registration = mcp_registration_component(
        InstallKind::RunnerTmux,
        "claude",
        McpState::RunnerInjected,
        McpScope::PerSession,
        None,
    );
    let warning = Component::new("cli.opencode", true, Status::Warn, false)
        .with_hint("opencode integration is unverified")
        .with_detail("state", json!("unverified"));
    let report = Report::new(
        GeneratedBy {
            binary: "sylvode-runner".to_string(),
            version: "0.2.540".to_string(),
            task_protocol: 1,
        },
        Host {
            os: "macos".to_string(),
            arch: "aarch64".to_string(),
            in_container: false,
            service_manager: ServiceManager::Launchd,
        },
        Context {
            mode: Mode::Tmux,
            as_service: true,
            path_source: PathSource::ServiceUnit,
            config: ConfigState::Found,
        },
        vec![tmux, registration, warning],
    );
    let expected = json!({
        "schema": "sylvode.doctor.v1",
        "generated_by": {"binary": "sylvode-runner", "version": "0.2.540", "task_protocol": 1},
        "host": {"os": "macos", "arch": "aarch64", "in_container": false, "service_manager": "launchd"},
        "context": {"mode": "tmux", "as_service": true, "path_source": "service-unit", "config": "found"},
        "components": [
            {"component": "tmux", "found": true, "version": "3.7b", "path": "/opt/homebrew/bin/tmux",
             "status": "ok", "required": true, "hint": null,
             "detail": {"min_version": "3.2", "socket": "sylvode", "server_running": true}},
            {"component": "mcp_registration.claude", "found": true, "version": null, "path": null,
             "status": "ok", "required": true, "hint": null,
             "detail": {"state": "runner_injected", "scope": "per-session"}},
            {"component": "cli.opencode", "found": true, "version": null, "path": null,
             "status": "warn", "required": false, "hint": "opencode integration is unverified",
             "detail": {"state": "unverified"}}
        ],
        "summary": {"ok": 2, "warn": 1, "fail": 0}
    });
    let actual: serde_json::Value = serde_json::from_str(&report.to_json().expect("serialises")).expect("parses");
    assert_eq!(actual, expected);
    // Warnings never change the exit code.
    assert_eq!(report.exit_code(), 0);
    let back: Report = serde_json::from_value(actual).expect("round trip");
    assert_eq!(back, report);
}

#[test]
fn info_counts_as_ok_and_a_failure_exits_with_two() {
    let host = Host {
        os: "linux".to_string(),
        arch: "x86_64".to_string(),
        in_container: true,
        service_manager: ServiceManager::None,
    };
    let context = Context {
        mode: Mode::Auto,
        as_service: false,
        path_source: PathSource::Shell,
        config: ConfigState::Missing,
    };
    let mut components = vec![
        Component::new("a", true, Status::Info, false),
        Component::new("b", true, Status::Ok, false),
    ];
    let report = Report::new(
        GeneratedBy::new("mcp-server", 1),
        host.clone(),
        context.clone(),
        components.clone(),
    );
    assert_eq!((report.summary.ok, report.summary.warn, report.summary.fail), (2, 0, 0));
    assert_eq!(report.exit_code(), 0);
    assert_eq!(report.generated_by.version, sylvode_prereq::VERSION);

    components.push(Component::new("c", false, Status::Fail, true).with_hint("install c"));
    let failed = Report::new(GeneratedBy::new("mcp-server", 1), host, context, components);
    assert_eq!(failed.exit_code(), 2);
    let table = failed.to_table();
    assert!(table.contains("summary: 2 ok, 0 warn, 1 fail"), "{table}");
    assert!(table.contains("hint: install c"), "{table}");
    assert!(table.contains("not found"), "{table}");
}

#[test]
fn project_rows_show_labels_and_never_paths_unless_asked() {
    let root = TempDir::new();
    let inside = root.path().join("svwl");
    std::fs::create_dir_all(&inside).expect("mkdir");
    let outside = TempDir::new();
    assert_eq!(
        directory_state(&inside, &[root.path()]),
        DirectoryState::InsideAllowlist
    );
    assert_eq!(
        directory_state(outside.path(), &[root.path()]),
        DirectoryState::OutsideAllowlist
    );
    assert_eq!(
        directory_state(&root.path().join("absent"), &[root.path()]),
        DirectoryState::Missing
    );
    // The parent of a root is not inside it, and `..` cannot step out.
    assert_eq!(
        directory_state(&inside.join("..").join(".."), &[&inside]),
        DirectoryState::OutsideAllowlist
    );

    let check = ProjectCheck {
        key: "SVWL/../x",
        directory: DirectoryState::InsideAllowlist,
        trusted: Trusted::Yes,
        agent_guide: AgentGuide::Current,
        directory_path: Some(&inside),
    };
    let hidden = project_component(&check, false);
    assert_eq!(hidden.component, "project.SVWLx");
    assert_eq!(hidden.status, Status::Ok);
    assert_eq!(hidden.path, None);
    let text = serde_json::to_string(&hidden).expect("serialises");
    assert!(!text.contains(&*inside.to_string_lossy()), "{text}");
    assert_eq!(
        hidden.detail,
        json!({"directory": "inside_allowlist", "trusted": "yes", "agent_guide": "current"})
    );
    let shown = project_component(&check, true);
    assert_eq!(detail(&shown, "directory_path"), &json!(inside.to_string_lossy()));

    for (directory, trusted, guide, want) in [
        (DirectoryState::Missing, Trusted::Yes, AgentGuide::Current, Status::Fail),
        (
            DirectoryState::OutsideAllowlist,
            Trusted::Yes,
            AgentGuide::Current,
            Status::Fail,
        ),
        (
            DirectoryState::InsideAllowlist,
            Trusted::No,
            AgentGuide::Current,
            Status::Warn,
        ),
        (
            DirectoryState::InsideAllowlist,
            Trusted::Unknown,
            AgentGuide::Absent,
            Status::Warn,
        ),
        (
            DirectoryState::InsideAllowlist,
            Trusted::Yes,
            AgentGuide::Stale,
            Status::Warn,
        ),
        (
            DirectoryState::InsideAllowlist,
            Trusted::Yes,
            AgentGuide::Conflict,
            Status::Warn,
        ),
        (
            DirectoryState::InsideAllowlist,
            Trusted::Yes,
            AgentGuide::Absent,
            Status::Ok,
        ),
    ] {
        let row = project_component(
            &ProjectCheck {
                key: "P",
                directory,
                trusted,
                agent_guide: guide,
                directory_path: None,
            },
            false,
        );
        assert_eq!(row.status, want, "{directory:?} {trusted:?} {guide:?}");
    }
}

#[test]
fn mcp_registration_rows_follow_the_install_kind() {
    let row = |install, state| mcp_registration_component(install, "claude", state, McpScope::User, Some("hint"));
    let ok = row(InstallKind::RunnerTmux, McpState::Connected);
    assert_eq!(ok.status, Status::Ok);
    assert!(ok.found);
    assert_eq!(detail(&ok, "state"), &json!("connected"));
    assert_eq!(detail(&ok, "scope"), &json!("user"));
    let failing = row(InstallKind::RunnerTmux, McpState::RegisteredFailing);
    assert_eq!(failing.status, Status::Fail);
    assert!(failing.required);
    assert!(!failing.found);
    assert_eq!(failing.hint.as_deref(), Some("hint"));
    assert_eq!(
        row(InstallKind::McpServerAlone, McpState::NotRegistered).status,
        Status::Warn
    );
    assert_eq!(
        row(InstallKind::RunnerForwardOnly, McpState::BinaryMissing).status,
        Status::Info
    );
    assert_eq!(row(InstallKind::RunnerTmux, McpState::Unverified).status, Status::Warn);
}

#[cfg(feature = "http")]
mod api {
    use std::time::Duration;

    use sylvode_prereq::InstallKind;
    use sylvode_prereq::Status;
    use sylvode_prereq::probe::api::{ApiProbe, check_api_protocol};
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    /// Serves one request with `response` and returns the address and the request text.
    async fn serve_once(response: &'static str) -> (String, tokio::task::JoinHandle<String>) {
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let address = format!("http://{}", listener.local_addr().expect("address"));
        let handle = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.expect("accept");
            let mut buffer = vec![0_u8; 8192];
            let read = socket.read(&mut buffer).await.expect("read");
            let request = String::from_utf8_lossy(buffer.get(..read).unwrap_or_default()).into_owned();
            socket.write_all(response.as_bytes()).await.expect("write");
            let _ = socket.shutdown().await;
            request
        });
        (address, handle)
    }

    fn ok_body(protocol: u32) -> String {
        let body =
            format!(r#"{{"code":0,"message":"success","data":{{"task_protocol":{protocol},"api_version":"0.5.0"}}}}"#);
        format!(
            "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
            body.len()
        )
    }

    const fn probe<'a>(base_url: &'a str, token: &'a str) -> ApiProbe<'a> {
        ApiProbe {
            base_url,
            token,
            surface: Some("mcp_stdio"),
            timeout: Duration::from_secs(5),
            own_protocol: 1,
        }
    }

    #[tokio::test]
    async fn the_protocol_is_read_with_the_token_and_surface_header() {
        let response: &'static str = Box::leak(ok_body(1).into_boxed_str());
        let (address, request) = serve_once(response).await;
        let token = super::token_like();
        let component = check_api_protocol(InstallKind::RunnerTmux, &probe(&address, &token)).await;
        assert_eq!(component.component, "api_protocol");
        assert_eq!(component.status, Status::Ok);
        assert_eq!(component.version.as_deref(), Some("0.5.0"));
        assert_eq!(
            component.detail,
            serde_json::json!({"state": "answered", "task_protocol": 1})
        );
        let request = request.await.expect("request").to_lowercase();
        assert!(request.starts_with("get /api/v1/ai/protocol "), "{request}");
        assert!(
            request.contains(&format!("authorization: bearer {}", token.to_lowercase())),
            "{request}"
        );
        assert!(request.contains("x-sylvode-mcp-surface: mcp_stdio"), "{request}");
        let text = serde_json::to_string(&component).expect("serialises");
        assert!(!text.contains(&token) && !text.contains(&address), "{text}");
    }

    #[tokio::test]
    async fn another_protocol_is_a_mismatch() {
        let response: &'static str = Box::leak(ok_body(2).into_boxed_str());
        let (address, _request) = serve_once(response).await;
        let component = check_api_protocol(InstallKind::RunnerTmux, &probe(&address, "t")).await;
        assert_eq!(component.status, Status::Fail);
        assert!(
            component
                .hint
                .as_deref()
                .unwrap_or_default()
                .contains("this binary 1, api 2")
        );
        let (address, _request) = serve_once(response).await;
        let alone = check_api_protocol(InstallKind::McpServerAlone, &probe(&address, "t")).await;
        assert_eq!(alone.status, Status::Fail);
        let (address, _request) = serve_once(response).await;
        let forward = check_api_protocol(InstallKind::RunnerForwardOnly, &probe(&address, "t")).await;
        assert_eq!(forward.status, Status::Warn);
    }

    #[tokio::test]
    async fn refusals_and_bad_answers_carry_a_state_and_no_text() {
        for (response, state) in [
            (
                "HTTP/1.1 401 Unauthorized\r\ncontent-length: 0\r\nconnection: close\r\n\r\n",
                "unauthorized",
            ),
            (
                "HTTP/1.1 500 Oops\r\ncontent-length: 4\r\nconnection: close\r\n\r\nboom",
                "bad_answer",
            ),
            (
                "HTTP/1.1 200 OK\r\ncontent-length: 2\r\nconnection: close\r\n\r\n{}",
                "bad_answer",
            ),
            (
                "HTTP/1.1 302 Found\r\nlocation: http://127.0.0.1:1/x\r\ncontent-length: 0\r\nconnection: close\r\n\r\n",
                "bad_answer",
            ),
        ] {
            let (address, _request) = serve_once(response).await;
            let component = check_api_protocol(InstallKind::RunnerTmux, &probe(&address, "t")).await;
            assert_eq!(component.status, Status::Fail, "{state}");
            assert_eq!(component.detail, serde_json::json!({"state": state}));
            assert!(!component.found);
        }
    }

    #[tokio::test]
    async fn an_unreachable_or_invalid_address_is_reported_by_state() {
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let address = format!("http://{}", listener.local_addr().expect("address"));
        drop(listener);
        let component = check_api_protocol(InstallKind::RunnerTmux, &probe(&address, "t")).await;
        assert_eq!(component.detail, serde_json::json!({"state": "unreachable"}));
        let invalid = check_api_protocol(InstallKind::RunnerTmux, &probe("ftp://example.invalid", "t")).await;
        assert_eq!(invalid.detail, serde_json::json!({"state": "invalid_url"}));
        let forward = check_api_protocol(InstallKind::RunnerForwardOnly, &probe(&address, "t")).await;
        assert_eq!(forward.status, Status::Warn);
    }
}
