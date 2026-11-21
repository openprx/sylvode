//! `mcp-server doctor` and `mcp-server --check`, run as the shipped binary against fake programs on
//! a temporary `PATH`. Nothing here starts a real tmux, runs a real CLI agent or touches the
//! service manager: the child's `PATH` holds only the fakes a test writes.
//!
//! The matrix rows of the design (section 10.4) covered here: 7 (no runner), 8 (no tmux), 9 (CLI
//! present, server not registered), and the `--check` contract the runner's checker relies on.
#![cfg(unix)]
// Test helpers outside `#[test]` functions may fail loudly; this is test code only.
#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic, clippy::indexing_slicing)]

mod support;

use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};

use parking_lot::Mutex;

use axum::extract::State;
use axum::http::HeaderMap;
use axum::routing::get;
use axum::{Json, Router};
use serde_json::{Value, json};
use support::{McpSettings, write_config, write_config_with_extra_mcp_keys};
use sylvode_prereq::{Checker, InstallKind, Peer, Status};

const BIN: &str = env!("CARGO_BIN_EXE_mcp-server");
const WORKSPACE: &str = "11111111-1111-4111-8111-111111111111";
/// Built from halves so no token-like literal sits in the source.
const TOKEN: &str = concat!("opr_", "doctor_test_bot_token");

/// A temporary directory with a `bin` directory that is the child's whole `PATH`.
struct Sandbox {
    root: PathBuf,
}

impl Sandbox {
    fn new() -> Self {
        static COUNTER: AtomicU32 = AtomicU32::new(0);
        let root = std::env::temp_dir().join(format!(
            "sylvode-mcp-doctor-{}-{}",
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(root.join("bin")).unwrap();
        Self { root }
    }

    fn bin(&self) -> PathBuf {
        self.root.join("bin")
    }

    fn fake(&self, name: &str, body: &str) {
        let path = self.bin().join(name);
        std::fs::write(&path, format!("#!/bin/sh\n{body}\n")).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
    }

    fn fake_tmux(&self) {
        let log = self.root.join("tmux.calls");
        self.fake(
            "tmux",
            &format!(
                "echo \"$*\" >> '{}'\ncase \"$1\" in -V) echo 'tmux 3.4';; *) exit 1;; esac",
                log.display()
            ),
        );
    }

    fn command(&self, args: &[&str]) -> Command {
        let mut command = Command::new(BIN);
        command
            .args(args)
            .env_clear()
            .env("PATH", self.bin())
            .env("HOME", &self.root)
            .current_dir(&self.root);
        command
    }

    fn run(&self, args: &[&str]) -> Output {
        self.command(args).output().unwrap()
    }
}

impl Drop for Sandbox {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

/// A configuration with a bot token on `stdio`, readable by its owner only.
fn private_config(api_url: &str, extra: &str) -> support::ConfigFile {
    let config = write_config_with_extra_mcp_keys(
        &McpSettings {
            api_url,
            bot_token: Some(TOKEN),
            workspace_id: WORKSPACE,
            transport: None,
            bind_addr: None,
        },
        extra,
    )
    .unwrap();
    std::fs::set_permissions(config.path(), std::fs::Permissions::from_mode(0o600)).unwrap();
    config
}

/// An HTTP-transport configuration with no token: nothing for the API probe to ask with.
fn tokenless_config() -> support::ConfigFile {
    let config = write_config(&McpSettings {
        api_url: "http://localhost:1",
        bot_token: None,
        workspace_id: WORKSPACE,
        transport: Some("http"),
        bind_addr: None,
    })
    .unwrap();
    std::fs::set_permissions(config.path(), std::fs::Permissions::from_mode(0o600)).unwrap();
    config
}

fn json_of(output: &Output) -> Value {
    serde_json::from_slice(&output.stdout).unwrap_or_else(|error| {
        panic!(
            "stdout is not JSON ({error}): {}",
            String::from_utf8_lossy(&output.stdout)
        )
    })
}

fn row<'a>(report: &'a Value, name: &str) -> &'a Value {
    report["components"]
        .as_array()
        .unwrap()
        .iter()
        .find(|component| component["component"] == name)
        .unwrap_or_else(|| panic!("no row {name} in {report}"))
}

fn text(output: &Output) -> String {
    format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
}

fn path_str(path: &Path) -> &str {
    path.to_str().unwrap()
}

#[test]
fn doctor_check_is_what_the_prereq_parser_reads() {
    let sandbox = Sandbox::new();
    let component = Checker::new(InstallKind::RunnerTmux)
        .with_path(sandbox.bin())
        .check_peer(Peer::McpServer, Some(Path::new(BIN)), Some(1));
    assert_eq!(component.component, "mcp_server");
    assert!(component.found, "{component:?}");
    assert_eq!(component.status, Status::Ok, "{component:?}");
    assert_eq!(component.version.as_deref(), Some(env!("CARGO_PKG_VERSION")));
    assert_eq!(component.detail["task_protocol"], json!(1));
    // A runner on another protocol is seen as a mismatch, so the number is really compared.
    let mismatch = Checker::new(InstallKind::RunnerTmux)
        .with_path(sandbox.bin())
        .check_peer(Peer::McpServer, Some(Path::new(BIN)), Some(2));
    assert_eq!(mismatch.status, Status::Fail, "{mismatch:?}");
}

#[test]
fn doctor_check_prints_one_row_and_no_secret() {
    let sandbox = Sandbox::new();
    let config = private_config("http://localhost:1", "tool_mode = \"full\"\n");
    let output = sandbox.run(&["--check", "--config", path_str(config.path())]);
    assert_eq!(output.status.code(), Some(0), "{}", text(&output));
    let report = json_of(&output);
    assert_eq!(report["schema"], "sylvode.doctor.v1");
    assert_eq!(report["generated_by"]["binary"], "mcp-server");
    assert_eq!(report["generated_by"]["task_protocol"], json!(1));
    assert_eq!(report["context"]["config"], "found");
    assert_eq!(report["components"].as_array().unwrap().len(), 1);
    let own = row(&report, "mcp_server");
    assert_eq!(own["detail"]["tool_mode"], "full");
    let all = text(&output);
    assert!(!all.contains(TOKEN), "{all}");
    assert!(!all.contains("localhost"), "{all}");
}

#[test]
fn doctor_check_survives_a_missing_and_a_broken_configuration() {
    let sandbox = Sandbox::new();
    let missing = sandbox.root.join("absent.toml");
    let output = sandbox.run(&["--check", "--config", path_str(&missing)]);
    assert_eq!(output.status.code(), Some(0), "{}", text(&output));
    let report = json_of(&output);
    assert_eq!(report["context"]["config"], "missing");
    assert_eq!(row(&report, "mcp_server")["detail"]["tool_mode"], "search");

    let broken = sandbox.root.join("broken.toml");
    std::fs::write(&broken, "this is = not [valid\n").unwrap();
    let output = sandbox.run(&["--check", "--config", path_str(&broken)]);
    assert_eq!(output.status.code(), Some(0), "{}", text(&output));
    assert_eq!(json_of(&output)["context"]["config"], "invalid");
}

#[test]
fn doctor_check_refuses_a_subcommand() {
    let sandbox = Sandbox::new();
    let output = sandbox.run(&["--check", "doctor"]);
    assert_eq!(output.status.code(), Some(2), "{}", text(&output));
}

#[test]
fn doctor_without_runner_or_tmux_is_all_information() {
    // Matrix rows 7 and 8.
    let sandbox = Sandbox::new();
    let config = tokenless_config();
    let output = sandbox.run(&["doctor", "--json", "--config", path_str(config.path())]);
    assert_eq!(output.status.code(), Some(0), "{}", text(&output));
    let report = json_of(&output);
    let runner = row(&report, "runner");
    assert_eq!(runner["found"], false);
    assert_eq!(runner["status"], "info");
    assert!(
        runner["hint"]
            .as_str()
            .unwrap()
            .contains("task delivery needs sylvode-runner")
    );
    assert_eq!(row(&report, "tmux")["status"], "info");
    assert_eq!(row(&report, "tmux")["found"], false);
    assert_eq!(row(&report, "api_protocol")["detail"]["state"], "not_configured");
    assert_eq!(row(&report, "mcp_server")["detail"]["config"], "parsed");
    assert_eq!(report["summary"]["fail"], 0);
}

#[test]
fn doctor_an_old_tmux_is_still_only_information_for_the_mcp_server_alone() {
    let sandbox = Sandbox::new();
    sandbox.fake("tmux", "case \"$1\" in -V) echo 'tmux 3.1';; *) exit 1;; esac");
    let config = tokenless_config();
    let output = sandbox.run(&["doctor", "--json", "--config", path_str(config.path())]);
    assert_eq!(output.status.code(), Some(0), "{}", text(&output));
    assert_eq!(row(&json_of(&output), "tmux")["status"], "info");
}

#[test]
fn doctor_reports_a_cli_that_is_present_but_not_registered_with_the_exact_command() {
    // Matrix row 9.
    let sandbox = Sandbox::new();
    sandbox.fake_tmux();
    sandbox.fake(
        "claude",
        "case \"$1\" in --version) echo '2.1.328 (Claude Code)';; mcp) exit 1;; esac",
    );
    let config = tokenless_config();
    let output = sandbox.run(&[
        "doctor",
        "--json",
        "--cli",
        "claude",
        "--config",
        path_str(config.path()),
    ]);
    assert_eq!(output.status.code(), Some(0), "{}", text(&output));
    let report = json_of(&output);
    assert_eq!(row(&report, "cli.claude")["version"], "2.1.328");
    let registration = row(&report, "mcp_registration.claude");
    assert_eq!(registration["status"], "warn");
    assert_eq!(registration["detail"]["state"], "not_registered");
    let expected = format!(
        "claude mcp add --scope user sylvode -- {} serve --transport stdio --config {}",
        std::fs::canonicalize(BIN).unwrap().display(),
        config.path().display()
    );
    assert!(
        registration["hint"].as_str().unwrap().contains(&expected),
        "{registration}\nexpected {expected}"
    );
    // The fake tmux is the only tmux there is, and it was only asked its version and for sessions.
    let calls = std::fs::read_to_string(sandbox.root.join("tmux.calls")).unwrap();
    assert!(
        calls
            .lines()
            .all(|line| line == "-V" || line == "-L sylvode list-sessions"),
        "{calls}"
    );
}

#[test]
fn doctor_reads_the_registration_state_of_claude() {
    let config = tokenless_config();
    for (status_line, state, status) in [
        ("Status: ✓ Connected", "connected", "ok"),
        ("Status: ✗ Failed to connect", "registered_failing", "warn"),
        ("Status: ⚠ Pending approval", "not_registered", "warn"),
    ] {
        let sandbox = Sandbox::new();
        // The fake prints a token in its output: it must never reach the report.
        sandbox.fake(
            "claude",
            &format!(
                "case \"$1\" in --version) echo '2.1.328 (Claude Code)';; mcp) printf 'sylvode:\\n  Scope: Project config\\n  {status_line}\\n  Args: --bot-token {TOKEN}\\n';; esac"
            ),
        );
        let output = sandbox.run(&[
            "doctor",
            "--json",
            "--cli",
            "claude",
            "--config",
            path_str(config.path()),
        ]);
        assert_eq!(output.status.code(), Some(0), "{}", text(&output));
        assert!(!text(&output).contains(TOKEN));
        let report = json_of(&output);
        let registration = row(&report, "mcp_registration.claude");
        assert_eq!(registration["detail"]["state"], state, "{status_line}");
        assert_eq!(registration["status"], status, "{status_line}");
        assert_eq!(registration["detail"]["scope"], "project");
    }
}

#[test]
fn doctor_codex_is_judged_by_exit_status_and_opencode_is_unverified() {
    let config = tokenless_config();
    let sandbox = Sandbox::new();
    sandbox.fake(
        "codex",
        "case \"$1\" in --version) echo 'codex-cli 0.170.0';; mcp) exit 0;; esac",
    );
    sandbox.fake("opencode", "echo '1.2.3'");
    let output = sandbox.run(&[
        "doctor",
        "--json",
        "--cli",
        "codex",
        "opencode",
        "--config",
        path_str(config.path()),
    ]);
    assert_eq!(output.status.code(), Some(0), "{}", text(&output));
    let report = json_of(&output);
    assert_eq!(row(&report, "mcp_registration.codex")["detail"]["state"], "connected");
    let opencode = row(&report, "mcp_registration.opencode");
    assert_eq!(opencode["status"], "warn");
    assert_eq!(opencode["detail"]["state"], "unverified");

    let other = Sandbox::new();
    other.fake(
        "codex",
        "case \"$1\" in --version) echo 'codex-cli 0.170.0';; mcp) exit 1;; esac",
    );
    let output = other.run(&[
        "doctor",
        "--json",
        "--cli",
        "codex",
        "--config",
        path_str(config.path()),
    ]);
    let report = json_of(&output);
    let registration = row(&report, "mcp_registration.codex");
    assert_eq!(registration["detail"]["state"], "not_registered");
    assert!(
        registration["hint"]
            .as_str()
            .unwrap()
            .contains("codex mcp add sylvode -- ")
    );
}

#[test]
fn doctor_a_cli_that_is_not_installed_is_binary_missing() {
    let sandbox = Sandbox::new();
    let config = tokenless_config();
    let output = sandbox.run(&[
        "doctor",
        "--json",
        "--cli",
        "claude",
        "--config",
        path_str(config.path()),
    ]);
    assert_eq!(output.status.code(), Some(0), "{}", text(&output));
    let report = json_of(&output);
    assert_eq!(
        row(&report, "mcp_registration.claude")["detail"]["state"],
        "binary_missing"
    );
    assert_eq!(row(&report, "cli.claude")["status"], "info");
}

#[test]
fn doctor_a_runner_on_another_protocol_fails_and_a_matching_one_is_reported() {
    let sandbox = Sandbox::new();
    let config = tokenless_config();
    let runner = |protocol: u32| {
        format!(
            "echo '{{\"schema\":\"sylvode.doctor.v1\",\"generated_by\":{{\"binary\":\"sylvode-runner\",\"version\":\"0.5.0\",\"task_protocol\":{protocol}}}}}'"
        )
    };
    sandbox.fake("sylvode-runner", &runner(1));
    let output = sandbox.run(&["doctor", "--json", "--config", path_str(config.path())]);
    assert_eq!(output.status.code(), Some(0), "{}", text(&output));
    let report = json_of(&output);
    assert_eq!(row(&report, "runner")["version"], "0.5.0");
    assert_eq!(row(&report, "runner")["status"], "ok");

    sandbox.fake("sylvode-runner", &runner(2));
    let output = sandbox.run(&["doctor", "--json", "--config", path_str(config.path())]);
    assert_eq!(output.status.code(), Some(2), "{}", text(&output));
    assert_eq!(row(&json_of(&output), "runner")["status"], "fail");
}

#[test]
fn doctor_configuration_problems_are_rows_not_panics() {
    let sandbox = Sandbox::new();
    // Missing: a warning.
    let missing = sandbox.root.join("absent.toml");
    let output = sandbox.run(&["doctor", "--json", "--config", path_str(&missing)]);
    assert_eq!(output.status.code(), Some(0), "{}", text(&output));
    let report = json_of(&output);
    assert_eq!(report["context"]["config"], "missing");
    assert_eq!(row(&report, "mcp_server")["status"], "warn");

    // Broken: a failure that quotes nothing of the file.
    let broken = sandbox.root.join("broken.toml");
    std::fs::write(&broken, format!("[mcp]\nbot_token = {TOKEN:?} oops\n")).unwrap();
    let output = sandbox.run(&["doctor", "--config", path_str(&broken)]);
    assert_eq!(output.status.code(), Some(2), "{}", text(&output));
    assert!(!text(&output).contains(TOKEN));
    assert!(!text(&output).contains("panicked"));
    let output = sandbox.run(&["doctor", "--json", "--config", path_str(&broken)]);
    let report = json_of(&output);
    assert_eq!(report["context"]["config"], "invalid");
    assert_eq!(row(&report, "mcp_server")["detail"]["config"], "invalid");
}

#[test]
fn doctor_a_token_file_open_to_others_fails_and_names_the_fix() {
    let sandbox = Sandbox::new();
    let config = private_config("http://localhost:1", "");
    std::fs::set_permissions(config.path(), std::fs::Permissions::from_mode(0o644)).unwrap();
    let output = sandbox.run(&["doctor", "--json", "--config", path_str(config.path())]);
    assert_eq!(output.status.code(), Some(2), "{}", text(&output));
    let own = json_of(&output);
    let own = row(&own, "mcp_server");
    assert_eq!(own["status"], "fail");
    assert_eq!(own["detail"]["config_mode"], "too_open");
    assert!(own["hint"].as_str().unwrap().contains("chmod 600"));
    assert!(!text(&output).contains(TOKEN));
}

#[test]
fn doctor_prints_a_table_without_json() {
    let sandbox = Sandbox::new();
    sandbox.fake_tmux();
    let config = tokenless_config();
    let output = sandbox.run(&["doctor", "--config", path_str(config.path())]);
    assert_eq!(output.status.code(), Some(0), "{}", text(&output));
    let table = text(&output);
    assert!(table.contains("COMPONENT"), "{table}");
    assert!(table.contains("mcp_server"), "{table}");
    assert!(table.contains("summary: "), "{table}");
}

/// What the stand-in API saw.
#[derive(Clone, Default)]
struct Seen {
    headers: Arc<Mutex<Vec<(String, String)>>>,
}

async fn spawn_api(protocol: u64, seen: Seen) -> String {
    async fn handler(State((protocol, seen)): State<(u64, Seen)>, headers: HeaderMap) -> Json<Value> {
        for name in ["authorization", "x-sylvode-mcp-surface"] {
            if let Some(value) = headers.get(name).and_then(|value| value.to_str().ok()) {
                seen.headers.lock().push((name.to_string(), value.to_string()));
            }
        }
        Json(json!({ "code": 0, "message": "ok", "data": { "task_protocol": protocol, "api_version": "0.5.0" } }))
    }
    let router = Router::new()
        .route("/api/v1/ai/protocol", get(handler))
        .with_state((protocol, seen));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        let _ = axum::serve(listener, router).await;
    });
    format!("http://{addr}")
}

async fn run_doctor_against(api_url: &str) -> (Output, Sandbox) {
    let sandbox = Sandbox::new();
    let config = private_config(api_url, "");
    let path = config.path().to_path_buf();
    let mut command =
        tokio::process::Command::from(sandbox.command(&["doctor", "--json", "--config", path_str(&path)]));
    let output = command.output().await.unwrap();
    drop(config);
    (output, sandbox)
}

#[tokio::test(flavor = "multi_thread")]
async fn doctor_api_protocol_matches_and_the_token_goes_only_to_the_api() {
    let seen = Seen::default();
    let url = spawn_api(1, seen.clone()).await;
    let (output, _sandbox) = run_doctor_against(&url).await;
    assert_eq!(output.status.code(), Some(0), "{}", text(&output));
    let report = json_of(&output);
    let api = row(&report, "api_protocol");
    assert_eq!(api["status"], "ok");
    assert_eq!(api["detail"]["task_protocol"], json!(1));
    let all = text(&output);
    assert!(!all.contains(TOKEN), "{all}");
    assert!(!all.contains(&url), "{all}");
    let headers = seen.headers.lock().clone();
    assert!(
        headers.contains(&("authorization".to_string(), format!("Bearer {TOKEN}"))),
        "{headers:?}"
    );
    assert!(
        headers.contains(&("x-sylvode-mcp-surface".to_string(), "mcp_stdio".to_string())),
        "{headers:?}"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn doctor_api_protocol_mismatch_fails() {
    let url = spawn_api(2, Seen::default()).await;
    let (output, _sandbox) = run_doctor_against(&url).await;
    assert_eq!(output.status.code(), Some(2), "{}", text(&output));
    let report = json_of(&output);
    assert_eq!(row(&report, "api_protocol")["status"], "fail");
    assert_eq!(row(&report, "api_protocol")["detail"]["task_protocol"], json!(2));
}
