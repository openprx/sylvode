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
                "[ \"$1\" = --version ] && {{ echo '2.1.330 (Claude Code)'; exit 0; }}\npwd -P >> '{r}'\nfor a in \"$@\"; do printf '%s\\n' \"$a\" >> '{r}'; done\n/usr/bin/env | cut -d= -f1 | sort > '{r}.names'\n/usr/bin/env | grep -E '^(SVT_|CLAUDE_CONFIG_DIR=)' > '{r}.values'",
                r = record.display()
            ),
        );
        write_script(bin.path(), "mcp-server", "exit 0");
        Self {
            api,
            root,
            project,
            bin,
            record,
            state: TempDir::new("rn-state"),
        }
    }

    /// Replaces what the fake `claude` does after recording its directory, arguments and environment.
    fn claude_does(&self, tail: &str) {
        write_script(
            self.bin.path(),
            "claude",
            &format!(
                "[ \"$1\" = --version ] && {{ echo '2.1.330 (Claude Code)'; exit 0; }}\npwd -P >> '{r}'\nfor a in \"$@\"; do printf '%s\\n' \"$a\" >> '{r}'; done\n/usr/bin/env | cut -d= -f1 | sort > '{r}.names'\n/usr/bin/env | grep -E '^(SVT_|CLAUDE_CONFIG_DIR=)' > '{r}.values'\n{tail}",
                r = self.record.display()
            ),
        );
    }

    fn config(&self) -> String {
        self.config_with("")
    }

    fn config_with(&self, claude_extra: &str) -> String {
        let root = self
            .root
            .path()
            .canonicalize()
            .unwrap_or_else(|_| self.root.path().to_path_buf());
        format!(
            "{}\n[runner.mcp]\nserver_binary = \"{}\"\n[runner.cli.claude]\nbinary = \"{}\"\n{claude_extra}\n",
            config(&self.api.url(), &self.state, &root.to_string_lossy()),
            self.bin.file("mcp-server").display(),
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
    assert!(lines.iter().any(|l| l == "bypassPermissions"), "{lines:?}");
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
        if fx.api.requests_about_a_task("GET").len() >= 2 {
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
    assert!(fx.api.requests_about_a_task("GET").is_empty());
}

#[tokio::test(flavor = "multi_thread")]
async fn a_key_change_is_recorded_and_a_shared_key_is_refused() {
    let fx = Fixture::new().await;
    let server = fx.serve();
    fx.set_view(vec![(200, view(&fx.project, "headless", true))]);
    let (_, first) = post_signed(&server.url("/webhook"), &delivery()).await;
    assert_eq!(first["status"], "dispatched", "{first}");
    // Another project with the same key on this runner.
    let mut other = view(&fx.project, "headless", true);
    let other_project = "44444444-4444-4444-8444-444444444444";
    other["project"]["id"] = json!(other_project);
    fx.set_view(vec![(200, other)]);
    let mut body = serde_json::from_str::<Value>(&delivery_n(2)).expect("json");
    body["project_id"] = json!(other_project);
    let (_, answer) = post_signed(&server.url("/webhook"), &body.to_string()).await;
    assert_eq!(answer["reason_code"], "session_name_collision", "{answer}");
    // The registry is on disk with mode 0600.
    let registry = fx.state.path().join("s").join("sessions").join("registry.json");
    assert!(registry.is_file());
    assert_eq!(
        std::fs::metadata(&registry).expect("meta").permissions().mode() & 0o777,
        0o600
    );
}

fn state_path(fx: &Fixture) -> std::path::PathBuf {
    fx.state.path().join("s")
}

#[tokio::test(flavor = "multi_thread")]
async fn the_headless_cli_gets_the_runners_mcp_file_and_tool_list_and_nothing_stricter() {
    let fx = Fixture::new().await;
    // A project that tries to bring its own MCP servers and settings.
    std::fs::write(
        fx.project.join(".mcp.json"),
        r#"{"mcpServers":{"evil":{"command":"/bin/false"}}}"#,
    )
    .expect("mcp");
    std::fs::create_dir_all(fx.project.join(".claude")).expect("mkdir");
    std::fs::write(
        fx.project.join(".claude/settings.json"),
        r#"{"permissions":{"allow":["Bash(*)"]}}"#,
    )
    .expect("settings");
    fx.set_view(vec![(200, view(&fx.project, "headless", true))]);
    let mut server = fx.serve();
    let (_, answer) = post_signed(&server.url("/webhook"), &delivery()).await;
    assert_eq!(answer["status"], "dispatched", "{answer}");
    let lines = wait_lines(&fx.record, 16, Duration::from_secs(10));
    let argv = lines[1..].to_vec();
    let state = state_path(&fx);
    let json = state.join("mcp").join(format!("{BOT}.claude.json"));
    let at = |flag: &str| {
        argv.iter()
            .position(|a| a == flag)
            .unwrap_or_else(|| panic!("{flag} missing: {argv:?}"))
    };
    // The runner's own MCP file, outside the project; the operator's and the project's MCP servers
    // and settings are left alone (no strict config, no setting-source limit).
    assert_eq!(argv[at("--mcp-config") + 1], json.to_string_lossy());
    assert!(!json.starts_with(&fx.project));
    for absent in [
        "--strict-mcp-config",
        "--setting-sources",
        "--settings",
        "--disallowedTools",
    ] {
        assert!(!argv.iter().any(|a| a == absent), "{absent}: {argv:?}");
    }
    // The prompt follows `-p`; the operator's arguments (default: bypass) come next.
    assert_eq!(at("-p"), 0);
    assert_eq!(argv[at("--permission-mode") + 1], "bypassPermissions");
    // The allow list holds the eight task and entry tools only.
    let allowed_at = at("--allowedTools");
    let allowed: Vec<&String> = argv[allowed_at + 1..]
        .iter()
        .take_while(|a| !a.starts_with("--"))
        .collect();
    assert_eq!(allowed.len(), 8, "{allowed:?}");
    assert!(allowed.iter().any(|a| *a == "mcp__sylvode__sylvode_task_get"));
    assert!(
        allowed
            .iter()
            .all(|a| a.starts_with("mcp__sylvode__") && !a.contains("sylvode_write")),
        "{allowed:?}"
    );
    // The files hold the token once, with private modes, and no argument holds it.
    assert!(argv.iter().all(|a| !a.contains("opr_")), "{argv:?}");
    let toml = std::fs::read_to_string(state.join("mcp").join(format!("{BOT}.toml"))).expect("toml");
    assert!(toml.contains(TOKEN));
    assert!(!std::fs::read_to_string(&json).expect("json").contains("opr_"));
    for (path, mode) in [
        (&json, 0o600),
        (&state.join("mcp").join(format!("{BOT}.toml")), 0o600),
        (&state.join("mcp"), 0o700),
        (&state, 0o700),
    ] {
        assert_eq!(
            std::fs::metadata(path).expect("meta").permissions().mode() & 0o777,
            mode,
            "{path:?}"
        );
    }
    let log = server.stdout().join("\n");
    assert!(!log.contains(TOKEN));
}

#[tokio::test(flavor = "multi_thread")]
async fn the_headless_cli_gets_the_operators_environment_minus_the_agent_variables() {
    let fx = Fixture::new().await;
    fx.set_view(vec![(200, view(&fx.project, "headless", true))]);
    let server = {
        let config = fx.config_with("");
        Server::start(
            RUNNER,
            &config,
            &[
                ("SV_SENTINEL_SECRET", "operator"),
                ("SSH_AUTH_SOCK", "/tmp/operator-agent"),
                ("SVT_CANARY", "operator"),
                ("CLAUDE_CONFIG_DIR", "/operator/claude"),
                ("CLAUDECODE", "1"),
                ("CLAUDE_CODE_ENTRYPOINT", "cli"),
                ("TMUX", "/tmp/tmux-1/default,1,0"),
                ("LC_ALL", "C"),
            ],
        )
        .expect("starts")
    };
    let (_, answer) = post_signed(&server.url("/webhook"), &delivery()).await;
    assert_eq!(answer["status"], "dispatched", "{answer}");
    wait_lines(&fx.record, 5, Duration::from_secs(10));
    // The fake records the NAMES of its variables and the values of `SVT_*` and
    // `CLAUDE_CONFIG_DIR` only, beside the test's own record file.
    let names_file = format!("{}.names", fx.record.display());
    let values_file = format!("{}.values", fx.record.display());
    for _ in 0..100 {
        // Both files are written after they are created: wait for their content, not for the files.
        let written = |path: &str, last: &str| {
            std::fs::read_to_string(path).is_ok_and(|text| text.lines().any(|line| line.starts_with(last)))
        };
        if written(&values_file, "SVT_CANARY=") && written(&names_file, "PATH") {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    let names_text = std::fs::read_to_string(&names_file).expect("names");
    let env = std::fs::read_to_string(&values_file).expect("values");
    let names: Vec<&str> = names_text.lines().collect();
    // The operator's variables reach the CLI: it runs with their environment and login.
    for kept in ["SV_SENTINEL_SECRET", "SSH_AUTH_SOCK", "SVT_CANARY", "LC_ALL", "PATH"] {
        assert!(names.contains(&kept), "{kept} did not reach the CLI: {names:?}");
    }
    assert!(env.contains("SVT_CANARY=operator"), "{env}");
    // The three agent variables never do.
    for gone in ["CLAUDECODE", "CLAUDE_CODE_ENTRYPOINT", "TMUX"] {
        assert!(!names.contains(&gone), "{gone} reached the CLI");
    }
    // No configuration directory of the runner's: the operator's own value is passed untouched,
    // and nothing is created under the state directory.
    assert!(env.contains("CLAUDE_CONFIG_DIR=/operator/claude"), "{env}");
    assert!(!state_path(&fx).join("claude").exists());
}

#[tokio::test(flavor = "multi_thread")]
async fn a_directory_swapped_for_a_link_while_waiting_for_a_slot_is_never_run_in() {
    let fx = Fixture::new().await;
    // The first run holds the only slot for a while.
    fx.claude_does("/bin/sleep 3");
    let outside = TempDir::new("rn-outside");
    let outside_path = outside
        .path()
        .canonicalize()
        .unwrap_or_else(|_| outside.path().to_path_buf());
    fx.set_view(vec![(200, view(&fx.project, "headless", true))]);
    let server = fx.serve();
    let (_, first) = post_signed(&server.url("/webhook"), &delivery_n(1)).await;
    assert_eq!(first["status"], "dispatched", "{first}");
    wait_lines(&fx.record, 3, Duration::from_secs(10));
    let (_, second) = post_signed(&server.url("/webhook"), &delivery_n(2)).await;
    assert_eq!(second["status"], "dispatched", "{second}");
    // While the second waits, the directory becomes a link to somewhere else.
    let moved = fx.project.with_file_name("svwl-moved");
    std::fs::rename(&fx.project, &moved).expect("rename");
    std::os::unix::fs::symlink(&outside_path, &fx.project).expect("symlink");
    let fails = fx.api.wait_for("POST", "/fail", 2, Duration::from_secs(20)).await;
    let reasons: Vec<String> = fails
        .iter()
        .map(|f| f.body["reason"].as_str().unwrap_or_default().to_string())
        .collect();
    assert!(
        reasons
            .iter()
            .any(|r| r == "the project working directory changed after the task was accepted"),
        "{reasons:?}"
    );
    // The CLI ran once, in the real directory, never in the link's target.
    let record = std::fs::read_to_string(&fx.record).expect("record");
    assert_eq!(record.matches("[Sylvode task]").count(), 1, "{record}");
    assert!(!record.contains(&*outside_path.to_string_lossy()), "{record}");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_shutdown_during_a_run_fails_the_task_as_retryable() {
    let fx = Fixture::new().await;
    fx.claude_does("exec /bin/sleep 30");
    fx.set_view(vec![(200, view(&fx.project, "headless", true))]);
    let mut server = fx.serve();
    let (_, answer) = post_signed(&server.url("/webhook"), &delivery()).await;
    assert_eq!(answer["status"], "dispatched", "{answer}");
    wait_lines(&fx.record, 3, Duration::from_secs(10));
    server.terminate();
    assert!(
        server.wait_exit(Duration::from_secs(30)).is_some(),
        "the runner did not exit"
    );
    let fails = fx.api.requests_to("POST", "/api/v1/ai/tasks/");
    let fail = fails.iter().find(|r| r.path().ends_with("/fail")).expect("a fail call");
    assert_eq!(fail.body["retryable"], true, "{:?}", fail.body);
    assert_eq!(
        fail.body["reason"],
        "the runner stopped the run before the agent ended the task"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_hostile_directory_name_is_escaped_in_the_log() {
    let fx = Fixture::new().await;
    let hostile = std::path::PathBuf::from("/tmp/x\u{1b}[31mred\nforged log line");
    fx.set_view(vec![(200, view(&hostile, "headless", true))]);
    let mut server = Server::start(RUNNER, &fx.config(), &[("RUST_LOG", "info")]).expect("starts");
    let (_, answer) = post_signed(&server.url("/webhook"), &delivery()).await;
    assert_eq!(answer["reason_code"], "path_outside_allowlist", "{answer}");
    assert!(server.wait_for_line("working directory declined", Duration::from_secs(5)));
    let lines = server.stdout().to_vec();
    assert!(lines.iter().all(|l| !l.contains('\u{1b}')), "{lines:?}");
    assert!(lines.iter().all(|l| !l.starts_with("forged log line")), "{lines:?}");
    assert!(
        lines.iter().any(|l| l.contains("\\u{1b}") && l.contains("\\n")),
        "{lines:?}"
    );
}

const OTHER_BOT: &str = "00000000-0000-4000-8000-000000000002";

#[tokio::test(flavor = "multi_thread")]
async fn each_bots_deliveries_verify_only_with_its_own_secret() {
    use crate::support::{post_with, sign};
    let fx = Fixture::new().await;
    // A second bot on the same runner; each has its own signing secret and its own roots.
    let other_root = TempDir::new("rn-other");
    let other_root_path = other_root
        .path()
        .canonicalize()
        .unwrap_or_else(|_| other_root.path().to_path_buf());
    let own_root = fx
        .root
        .path()
        .canonicalize()
        .unwrap_or_else(|_| fx.root.path().to_path_buf());
    let config = fx
        .config()
        .replace(
            &format!("id = \"{BOT}\"\nname = \"Bot\"\nagent_type = \"runner\"\n"),
            &format!(
                "id = \"{BOT}\"\nname = \"Bot\"\nagent_type = \"runner\"\nallowed_roots = [\"{}\"]\n",
                own_root.display()
            ),
        )
        .replace(
            "\n[[agents]]",
            &format!(
                "\n[[ai_task_callback.bots]]\nbot_id = \"{OTHER_BOT}\"\nworkspace_id = \"11111111-1111-4111-8111-111111111111\"\ntoken = \"{}\"\nsurface = \"mcp_stdio\"\nwebhook_secret = \"secret-of-bot-two-xx\"\n\n[[agents]]\nid = \"{OTHER_BOT}\"\nname = \"Two\"\nagent_type = \"runner\"\nallowed_roots = [\"{}\"]\n[agents.route]\nbot_ids = [\"{OTHER_BOT}\"]\n\n[[agents]]",
                concat!("opr", "_", "ef01ef01ef01ef01ef01ef01ef01ef01ef01"),
                other_root_path.display()
            ),
        )
        .replace(
            &format!("bot_id = \"{BOT}\"\nworkspace_id = \"11111111-1111-4111-8111-111111111111\"\ntoken = \"{TOKEN}\"\nsurface = \"mcp_stdio\"\n"),
            &format!("bot_id = \"{BOT}\"\nworkspace_id = \"11111111-1111-4111-8111-111111111111\"\ntoken = \"{TOKEN}\"\nsurface = \"mcp_stdio\"\nwebhook_secret = \"secret-of-bot-one-xx\"\n"),
        );
    let server = Server::start(RUNNER, &config, &[]).expect("starts");
    let delivery_for = |bot: &str, attempt: u32| {
        let mut value: Value = serde_json::from_str(&delivery_n(attempt)).expect("json");
        value["ai_participant_id"] = json!(bot);
        value.to_string()
    };
    let post = |body: String, secret: &'static str| {
        let url = server.url("/webhook");
        async move {
            post_with(&url, &body, &[("x-webhook-signature", &sign(&body, secret))])
                .await
                .0
        }
    };
    // Each bot's delivery verifies with its own secret.
    assert_eq!(post(delivery_for(BOT, 1), "secret-of-bot-one-xx").await, 200);
    assert_eq!(post(delivery_for(OTHER_BOT, 2), "secret-of-bot-two-xx").await, 200);
    // Another bot's secret, the shared list and a made-up secret do not.
    assert_eq!(post(delivery_for(BOT, 3), "secret-of-bot-two-xx").await, 401);
    assert_eq!(post(delivery_for(OTHER_BOT, 4), "secret-of-bot-one-xx").await, 401);
    assert_eq!(post(delivery_for(BOT, 5), crate::support::SECRET).await, 401);
    assert_eq!(post(delivery_for(OTHER_BOT, 6), crate::support::SECRET).await, 401);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_delivery_for_a_bot_the_agent_does_not_serve_is_refused() {
    let fx = Fixture::new().await;
    // An agent without a route: a delivery that names the runner agent type reaches it for any bot.
    let config = fx
        .config()
        .replace(&format!("[agents.route]\nbot_ids = [\"{BOT}\"]\n"), "");
    let server = Server::start(RUNNER, &config, &[]).expect("starts");
    let mut body: Value = serde_json::from_str(&delivery()).expect("json");
    body["ai_participant_id"] = json!("00000000-0000-4000-8000-0000000000ee");
    body["ai_participant_agent_type"] = json!("runner");
    let (_, answer) = post_signed(&server.url("/webhook"), &body.to_string()).await;
    assert_eq!(answer["status"], "refused", "{answer}");
    assert_eq!(answer["reason_code"], "runner_agent_disabled");
    assert!(
        answer["reason"].as_str().expect("reason").contains("does not serve"),
        "{answer}"
    );
    assert!(
        fx.api.requests_about_a_task("GET").is_empty(),
        "Sylvode was asked about another bot's task"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn start_up_logs_the_posture_of_every_cli_and_warns_about_opencode() {
    let fx = Fixture::new().await;
    let config = fx
        .config_with("")
        .replace("[runner.mcp]", "[runner.cli.opencode]\nenabled = true\n[runner.mcp]");
    let mut server = Server::start(RUNNER, &config, &[]).expect("starts");
    // The defaults turn permission checks off: the start-up line says so, as a warning.
    assert!(server.wait_for_line(
        "runner CLI posture: claude: session launch command",
        Duration::from_secs(5)
    ));
    assert!(server.wait_for_line("PERMISSION CHECKS ARE OFF", Duration::from_secs(5)));
    assert!(server.wait_for_line("runner CLI posture: codex: disabled", Duration::from_secs(5)));
    assert!(server.wait_for_line(
        "opencode permissions are not controlled by sylvode-runner",
        Duration::from_secs(5)
    ));
    let warned = server
        .stdout()
        .iter()
        .any(|l| l.contains(" WARN ") && l.contains("opencode permissions are not controlled"));
    assert!(warned, "{:?}", server.stdout());
}

/// A missing tmux degrades the runner (it starts, `/health` says
/// so and names the row) instead of refusing to start, and a tmux delivery is refused at once with
/// a reason code, so the task fails in Sylvode instead of hanging.
#[tokio::test(flavor = "multi_thread")]
async fn a_missing_tmux_degrades_the_runner_instead_of_refusing_it() {
    let fx = Fixture::new().await;
    fx.set_view(vec![(200, view(&fx.project, "tmux", true))]);
    let empty = TempDir::new("rn-empty-path");
    let config = fx.config();
    let server = Server::start(RUNNER, &config, &[("PATH", &empty.path().to_string_lossy())])
        .expect("the runner starts although tmux is missing");
    // `/health` stays plain: the overall status only, with no prerequisite detail.
    let answer = crate::handshake::health(&server.url("/health")).await;
    assert_eq!(answer["status"], "ok", "{answer}");
    assert!(
        answer.get("prereq").is_none() && answer.get("degraded").is_none(),
        "{answer}"
    );
    // The delivery is refused at once, naming the missing prerequisite.
    let (status, refused) = post_signed(&server.url("/webhook"), &delivery()).await;
    assert_eq!(status, 200, "{refused}");
    assert_eq!(refused["status"], "refused", "{refused}");
    assert_eq!(refused["reason_code"], "tmux_unavailable", "{refused}");
}
