//! `runner` agents through the real executable: the delivery checks and the headless channel,
//! against a mock of the Sylvode API and a fake `claude`.

use crate::handshake::{BOT, TOKEN, config};
use crate::mock_sv::MockSv;
use crate::support::{RUNNER, Server, TempDir, post_signed, wait_lines, write_script};
use serde_json::{Value, json};
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::time::Duration;

const TASK: &str = "11111111-1111-4111-8111-111111111111";
const PROJECT: &str = "22222222-2222-4222-8222-222222222222";

fn delivery() -> String {
    delivery_n(1)
}

/// The delivery of attempt `n`: a signed delivery is remembered by task id and attempt, so a test
/// that sends several uses several attempts.
fn delivery_n(n: u32) -> String {
    json!({
        "task_id": TASK, "project_id": PROJECT, "ai_participant_id": BOT, "attempts": n, "max_attempts": 9,
        "project_key": "SVWL", "protocol": 1, "trigger_kind": "assigned", "task_type": "issue_assigned",
    })
    .to_string()
}

fn view(dir: &Path, channel: &str, active: bool) -> Value {
    json!({
        "task_id": TASK, "attempt": 1, "active": active,
        "source": {"kind": "issue_assigned", "issue": {"id": "33333333-3333-4333-8333-333333333333"}},
        "project": {"id": PROJECT, "key": "SVWL", "name": "n", "workspace_id": PROJECT},
        "execution": {"enabled": true, "channel": channel, "cli": "claude",
                      "working_directory": dir.to_string_lossy(), "approval_gate": "none", "version": 1},
    })
}

struct Fixture {
    api: MockSv,
    root: TempDir,
    project: std::path::PathBuf,
    bin: TempDir,
    record: std::path::PathBuf,
    state: TempDir,
}

impl Fixture {
    async fn new() -> Self {
        let api = MockSv::start().await;
        let root = TempDir::new("rn-root");
        let project = root
            .path()
            .canonicalize()
            .unwrap_or_else(|_| root.path().to_path_buf())
            .join("svwl");
        std::fs::create_dir_all(&project).expect("mkdir");
        let bin = TempDir::new("rn-bin");
        let record = bin.file("record");
        write_script(
            bin.path(),
            "claude",
            &format!(
                "pwd -P >> '{r}'\nfor a in \"$@\"; do printf '%s\\n' \"$a\" >> '{r}'; done",
                r = record.display()
            ),
        );
        Self {
            api,
            root,
            project,
            bin,
            record,
            state: TempDir::new("rn-state"),
        }
    }

    fn config(&self) -> String {
        let root = self
            .root
            .path()
            .canonicalize()
            .unwrap_or_else(|_| self.root.path().to_path_buf());
        format!(
            "{}\n[runner.cli.claude]\nbinary = \"{}\"\n",
            config(&self.api.url(), &self.state, &root.to_string_lossy()),
            self.bin.file("claude").display()
        )
        .replace("\n[[agents]]", "\n[[agents]]")
    }

    fn serve(&self) -> Server {
        // The `[runner.cli.claude]` table must come before `[[agents]]` only for readability; TOML
        // does not care.
        Server::start(RUNNER, &self.config(), &[("RUST_LOG", "trace")]).expect("starts")
    }

    fn set_view(&self, views: Vec<(u16, Value)>) {
        self.api.script(|s| {
            s.views.insert(TASK.to_string(), views);
        });
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn a_headless_task_runs_in_the_canonical_directory_and_is_failed_when_left_open() {
    let fx = Fixture::new().await;
    // The check sees an active task; after the run it is still active.
    fx.set_view(vec![(200, view(&fx.project, "headless", true))]);
    let mut server = fx.serve();
    let (status, answer) = post_signed(&server.url("/webhook"), &delivery()).await;
    assert_eq!(status, 200, "{answer}");
    assert_eq!(answer["status"], "dispatched", "{answer}");
    assert_eq!(answer["channel"], "headless");
    assert_eq!(answer["agent"], BOT);

    // The fake claude ran once, in the canonical directory, with the permission flags and a prompt
    // that starts with the task header.
    let lines = wait_lines(&fx.record, 6, Duration::from_secs(10));
    assert_eq!(Path::new(&lines[0]), fx.project.as_path(), "{lines:?}");
    assert!(lines.iter().any(|l| l == "--permission-mode"), "{lines:?}");
    assert!(lines.iter().any(|l| l == "dontAsk"), "{lines:?}");
    assert!(
        lines
            .iter()
            .any(|l| l == &format!("[Sylvode task] id={TASK} attempt=1/9")),
        "{lines:?}"
    );
    assert!(lines.iter().any(|l| l.contains("sylvode_task_complete")), "{lines:?}");

    // The runner told Sylvode it started, then failed the task the agent left open.
    let progress = fx.api.wait_for("POST", "/progress", 1, Duration::from_secs(10)).await;
    assert_eq!(progress[0].body["kind"], "runner.pushed");
    assert!(progress[0].target.ends_with("?attempt=1"), "{:?}", progress[0].target);
    let fails = fx.api.wait_for("POST", "/fail", 1, Duration::from_secs(10)).await;
    assert_eq!(fails.len(), 1, "{fails:?}");
    assert_eq!(
        fails[0].body,
        json!({"reason": "agent ended without completing or failing the task", "retryable": false, "source": "runner"})
    );
    assert_eq!(
        fails[0].authorization.as_deref(),
        Some(format!("Bearer {TOKEN}").as_str())
    );
    assert_eq!(fails[0].surface.as_deref(), Some("mcp_stdio"));
    // No legacy callback was used.
    assert!(fx.api.requests().iter().all(|r| !r.path().contains("/callbacks/")));
    // The token is in no log line.
    let log = server.stdout().join("\n");
    assert!(!log.contains(TOKEN) && !log.contains("opr_"), "{log}");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_task_the_agent_ended_is_not_failed_again() {
    let fx = Fixture::new().await;
    // Active at the check, ended by the agent when the runner looks after the run.
    fx.set_view(vec![
        (200, view(&fx.project, "headless", true)),
        (200, view(&fx.project, "headless", false)),
    ]);
    let server = fx.serve();
    let (_, answer) = post_signed(&server.url("/webhook"), &delivery()).await;
    assert_eq!(answer["status"], "dispatched", "{answer}");
    wait_lines(&fx.record, 3, Duration::from_secs(10));
    // Two views were read (check and after the run); nothing was failed.
    for _ in 0..100 {
        if fx.api.requests_to("GET", "/api/v1/ai/tasks/").len() >= 2 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert!(
        fx.api
            .requests_to("POST", "/api/v1/ai/tasks/")
            .iter()
            .all(|r| !r.path().ends_with("/fail"))
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn refusals_carry_a_code_and_never_a_path() {
    let fx = Fixture::new().await;
    let outside = TempDir::new("rn-outside");
    let outside_path = outside
        .path()
        .canonicalize()
        .unwrap_or_else(|_| outside.path().to_path_buf());
    let mut cases: Vec<(Value, &str)> = vec![
        (view(&outside_path, "headless", true), "path_outside_allowlist"),
        (view(&fx.project.join("absent"), "headless", true), "path_missing"),
        (view(&fx.project, "tmux", true), "tmux_unavailable"),
    ];
    let mut disabled = view(&fx.project, "headless", true);
    disabled["execution"]["enabled"] = json!(false);
    cases.push((disabled, "execution_disabled"));
    let mut codex = view(&fx.project, "headless", true);
    codex["execution"]["cli"] = json!("codex");
    cases.push((codex, "cli_disabled"));
    let mut mismatch = view(&fx.project, "headless", true);
    mismatch["project"]["key"] = json!("OTHER");
    cases.push((mismatch, "delivery_mismatch"));
    let server = fx.serve();
    let secret_parts = [
        fx.project.to_string_lossy().into_owned(),
        outside_path.to_string_lossy().into_owned(),
    ];
    for (n, (view, code)) in (1_u32..).zip(cases) {
        fx.set_view(vec![(200, view)]);
        let (status, answer) = post_signed(&server.url("/webhook"), &delivery_n(n)).await;
        assert_eq!(status, 200, "{answer}");
        assert_eq!(answer["status"], "refused", "{code}: {answer}");
        assert_eq!(answer["reason_code"], code, "{answer}");
        assert_eq!(answer["agent"], BOT);
        let reason = answer["reason"].as_str().expect("reason");
        assert!(secret_parts.iter().all(|p| !reason.contains(p.as_str())), "{reason}");
    }
    // Nothing ran.
    assert!(!fx.record.exists());
}

#[tokio::test(flavor = "multi_thread")]
async fn a_missing_protocol_field_and_a_task_that_is_no_longer_active() {
    let fx = Fixture::new().await;
    let server = fx.serve();
    let mut old = serde_json::from_str::<Value>(&delivery()).expect("json");
    old.as_object_mut().expect("object").remove("protocol");
    let (_, answer) = post_signed(&server.url("/webhook"), &old.to_string()).await;
    assert_eq!(answer["status"], "refused");
    assert_eq!(answer["reason_code"], "protocol_mismatch");
    fx.set_view(vec![(400, Value::Null)]);
    let (status, answer) = post_signed(&server.url("/webhook"), &delivery_n(2)).await;
    assert_eq!(status, 200);
    assert_eq!(answer["status"], "ignored", "{answer}");
    assert_eq!(answer["reason"], "attempt 2 is no longer active in Sylvode");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_slow_or_failing_sylvode_is_answered_503() {
    let fx = Fixture::new().await;
    let server = fx.serve();
    fx.set_view(vec![(500, Value::Null)]);
    let (status, _) = post_signed(&server.url("/webhook"), &delivery_n(1)).await;
    assert_eq!(status, 503);
    fx.set_view(vec![(200, view(&fx.project, "headless", true))]);
    fx.api.script(|s| s.view_delay = Duration::from_secs(5));
    let started = std::time::Instant::now();
    let (status, _) = post_signed(&server.url("/webhook"), &delivery_n(2)).await;
    assert_eq!(status, 503);
    // delivery_check_timeout_secs is 2 in the test configuration.
    assert!(started.elapsed() < Duration::from_secs(5), "{:?}", started.elapsed());
}

#[tokio::test(flavor = "multi_thread")]
async fn invalid_roots_refuse_runner_agents_only() {
    let fx = Fixture::new().await;
    let config = fx.config().replace(
        &format!(
            "allowed_roots = [\"{}\"]",
            fx.root
                .path()
                .canonicalize()
                .unwrap_or_else(|_| fx.root.path().to_path_buf())
                .display()
        ),
        "allowed_roots = [\"/\"]",
    );
    let mut server = Server::start(RUNNER, &config, &[]).expect("starts");
    assert!(server.wait_for_line("`runner` agents will refuse every task", Duration::from_secs(5)));
    let (_, answer) = post_signed(&server.url("/webhook"), &delivery()).await;
    assert_eq!(answer["status"], "refused");
    assert_eq!(answer["reason_code"], "runner_agent_disabled");
    assert!(fx.api.requests_to("GET", "/api/v1/ai/tasks/").is_empty());
}

#[tokio::test(flavor = "multi_thread")]
async fn a_key_change_is_recorded_and_a_shared_key_is_refused() {
    let fx = Fixture::new().await;
    let server = fx.serve();
    fx.set_view(vec![(200, view(&fx.project, "tmux", true))]);
    let (_, first) = post_signed(&server.url("/webhook"), &delivery()).await;
    assert_eq!(first["reason_code"], "tmux_unavailable");
    // Another project with the same key on this runner.
    let mut other = view(&fx.project, "tmux", true);
    let other_project = "44444444-4444-4444-8444-444444444444";
    other["project"]["id"] = json!(other_project);
    fx.set_view(vec![(200, other)]);
    let mut body = serde_json::from_str::<Value>(&delivery_n(2)).expect("json");
    body["project_id"] = json!(other_project);
    let (_, answer) = post_signed(&server.url("/webhook"), &body.to_string()).await;
    assert_eq!(answer["reason_code"], "session_name_collision", "{answer}");
    // The registry is on disk with mode 0600.
    let registry = fx.state.path().join("sessions").join("registry.json");
    assert!(registry.is_file());
    assert_eq!(
        std::fs::metadata(&registry).expect("meta").permissions().mode() & 0o777,
        0o600
    );
}
