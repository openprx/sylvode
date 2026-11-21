//! The tmux channel end to end: the real runner, a real tmux server of its own (random socket,
//! `-f /dev/null`, a private `TMUX_TMPDIR` under `/tmp`, killed at the end), a mock Sylvode API and
//! a stand-in for the CLI: a shell script that turns bracketed paste on, shows a prompt, reports
//! the turn events through the runner's own `hook` subcommand and records what it was typed.
//!
//! Ignored unless `SV_TMUX_TESTS=1`. No real CLI agent is ever started, and nothing touches the
//! default tmux socket.

use crate::mock_sv::MockSv;
use crate::support::{RUNNER, SECRET, Server, TempDir, post_signed, write_script};
use serde_json::{Value, json};
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::process::Command;
use std::time::{Duration, Instant};

const BOT: &str = "00000000-0000-4000-8000-000000000001";
const TOKEN: &str = concat!("opr", "_", "9a9a9a9a9a9a9a9a9a9a9a9a9a9a9a9a9a9a");
const PROJECT: &str = "22222222-2222-4222-8222-222222222222";

fn enabled() -> bool {
    std::env::var("SV_TMUX_TESTS").as_deref() == Ok("1")
}

fn find_tmux() -> Option<PathBuf> {
    std::env::split_paths(&std::env::var_os("PATH")?)
        .map(|dir| dir.join("tmux"))
        .find(|path| path.is_file())
}

/// A private tmux server: killed on drop.
struct Tmux {
    program: PathBuf,
    socket: String,
    dir: PathBuf,
}

impl Tmux {
    fn new(program: PathBuf) -> Self {
        let id = &uuid::Uuid::new_v4().simple().to_string()[..8];
        let dir = PathBuf::from("/tmp").join(format!("svt-{id}"));
        std::fs::create_dir_all(&dir).expect("scratch");
        Self {
            program,
            socket: format!("svtest-{id}"),
            dir,
        }
    }

    fn raw(&self, args: &[&str]) -> std::process::Output {
        assert!(self.socket.starts_with("svtest-"));
        Command::new(&self.program)
            .args(["-L", &self.socket, "-f", "/dev/null"])
            .args(args)
            .env("TMUX_TMPDIR", &self.dir)
            .env_remove("TMUX")
            .output()
            .expect("run tmux")
    }

    fn state(&self) -> PathBuf {
        self.dir.join("st")
    }
}

impl Drop for Tmux {
    fn drop(&mut self) {
        let _ = self.raw(&["kill-server"]);
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

struct Fixture {
    api: MockSv,
    tmux: Tmux,
    root: TempDir,
    project: PathBuf,
    bin: TempDir,
    log: PathBuf,
}

/// The stand-in CLI, compiled once per test run with `rustc` (`standin_cli.rs`).
fn standin_binary() -> PathBuf {
    static BINARY: std::sync::OnceLock<PathBuf> = std::sync::OnceLock::new();
    BINARY
        .get_or_init(|| {
            // One directory per version of the source, reused by every run.
            let text = include_str!("standin_cli.rs");
            let hash = {
                use std::hash::{Hash, Hasher};
                let mut hasher = std::collections::hash_map::DefaultHasher::new();
                text.hash(&mut hasher);
                hasher.finish()
            };
            let dir = PathBuf::from("/tmp").join(format!("svs-{hash:016x}"));
            if dir.join("claude").is_file() {
                return dir.join("claude");
            }
            std::fs::create_dir_all(&dir).expect("scratch");
            let source = dir.join("standin_cli.rs");
            std::fs::write(&source, text).expect("source");
            let out = dir.join("claude");
            let rustc = std::env::var("RUSTC").unwrap_or_else(|_| "rustc".to_string());
            let status = Command::new(rustc)
                .args(["--edition", "2021", "-O", "-o"])
                .arg(&out)
                .arg(&source)
                .status()
                .expect("rustc runs");
            assert!(status.success(), "the stand-in did not compile");
            out
        })
        .clone()
}

/// What the stand-in CLI does: `Normal` runs turns, `Untrusted` shows the trust dialog and
/// nothing else, `Long` takes a long time over a turn.
#[derive(Clone, Copy)]
enum Standin {
    Normal,
    Untrusted,
    Long,
}

impl Fixture {
    async fn new(tmux_program: PathBuf, standin: Standin) -> Self {
        let api = MockSv::start().await;
        let tmux = Tmux::new(tmux_program);
        let root = TempDir::new("ts-root");
        let canonical = root.path().canonicalize().unwrap_or_else(|_| root.path().to_path_buf());
        let project = canonical.join("svwl");
        std::fs::create_dir_all(&project).expect("mkdir");
        let bin = TempDir::new("ts-bin");
        // The state directory exists before the runner starts, so the stand-in has somewhere to
        // log and to read its control files from; the runner accepts it (mode 0700).
        std::os::unix::fs::DirBuilderExt::mode(std::fs::DirBuilder::new().recursive(true), 0o700)
            .create(tmux.state())
            .expect("state dir");
        let log = tmux.state().join("standin.log");
        let cfg = match standin {
            Standin::Normal => "",
            Standin::Untrusted => "trust=dialog",
            Standin::Long => "turn=forever",
        };
        std::fs::write(tmux.state().join("standin.cfg"), cfg).expect("cfg");
        // The stand-in is a real executable called `claude`: the pane's foreground process has
        // the CLI's name, so the runner's foreground check is exercised, not bypassed.
        std::fs::copy(standin_binary(), bin.file("claude")).expect("install the stand-in");
        write_script(bin.path(), "mcp-server", "exit 0");
        Self {
            api,
            tmux,
            root,
            project,
            bin,
            log,
        }
    }

    fn config(&self) -> String {
        let canonical = self
            .root
            .path()
            .canonicalize()
            .unwrap_or_else(|_| self.root.path().to_path_buf());
        format!(
            "[server]\nlisten = \"127.0.0.1:0\"\n[security]\nwebhook_secrets = [\"{SECRET}\"]\n\
             [ai_task_callback]\nenabled = true\napi_url = \"{}\"\n\
             [[ai_task_callback.bots]]\nbot_id = \"{BOT}\"\ntoken = \"{TOKEN}\"\nsurface = \"mcp_stdio\"\n\
             [runner]\nstate_dir = \"{}\"\nallowed_roots = [\"{}\"]\nreport_interval_secs = 600\ndelivery_check_timeout_secs = 3\n\
             [runner.tmux]\nenabled = true\nbinary = \"{}\"\nsocket_name = \"{}\"\npush_settle_ms = 100\nready_timeout_secs = 20\n\
             [runner.cli.claude]\nbinary = \"{}\"\n\
             [runner.mcp]\nserver_binary = \"{}\"\n\
             [[agents]]\nid = \"{BOT}\"\nname = \"Bot\"\nagent_type = \"runner\"\n[agents.route]\nbot_ids = [\"{BOT}\"]\n",
            self.api.url(),
            self.tmux.state().display(),
            canonical.display(),
            self.tmux.program.display(),
            self.tmux.socket,
            self.bin.file("claude").display(),
            self.bin.file("mcp-server").display(),
        )
    }

    fn serve(&self) -> Server {
        Server::start(
            RUNNER,
            &self.config(),
            &[("TMUX_TMPDIR", &self.tmux.dir.to_string_lossy())],
        )
        .expect("starts")
    }

    fn view(&self, task: &str, active: bool, acked: bool) -> Value {
        json!({
            "task_id": task, "attempt": 1, "active": active,
            "acknowledged_at": if acked { json!("2026-11-22T10:00:00Z") } else { Value::Null },
            "source": {"kind": "issue_assignment", "issue": {"id": "33333333-3333-4333-8333-333333333333", "identifier": "SVWL-7", "title": "Fix the login"}},
            "project": {"id": PROJECT, "key": "SVWL", "name": "n", "workspace_id": PROJECT},
            "execution": {"enabled": true, "channel": "tmux", "cli": "claude",
                          "working_directory": self.project.to_string_lossy(), "approval_gate": "none", "version": 1},
        })
    }

    fn set_view(&self, task: &str, active: bool, acked: bool) {
        let view = self.view(task, active, acked);
        self.api.script(|s| {
            s.views.insert(task.to_string(), vec![(200, view)]);
        });
    }

    fn delivery(task: &str) -> String {
        json!({
            "task_id": task, "project_id": PROJECT, "ai_participant_id": BOT, "attempts": 1, "max_attempts": 3,
            "project_key": "SVWL", "protocol": 1, "trigger_kind": "assigned",
        })
        .to_string()
    }

    async fn deliver(url: String, task: &str) -> Value {
        post_signed(&url, &Self::delivery(task)).await.1
    }

    /// Sets what the stand-in shows (`idle` removes the override).
    fn set_mode(&self, mode: &str) {
        let file = self.tmux.state().join("standin.mode");
        if mode == "idle" {
            let _ = std::fs::remove_file(file);
        } else {
            std::fs::write(file, mode).expect("mode");
        }
    }

    /// How many Enter presses without a paste the stand-in saw.
    fn enters(&self) -> usize {
        std::fs::read_to_string(&self.log)
            .unwrap_or_default()
            .lines()
            .filter(|l| *l == "ENTER")
            .count()
    }

    fn typed(&self) -> Vec<String> {
        std::fs::read_to_string(&self.log)
            .unwrap_or_default()
            .lines()
            .filter_map(|l| {
                l.strip_prefix("TEXT: ")
                    .filter(|t| t.contains("[Sylvode task]"))
                    .map(str::to_string)
            })
            .collect()
    }

    async fn wait_typed(&self, count: usize, secs: u64) -> Vec<String> {
        let deadline = Instant::now() + Duration::from_secs(secs);
        loop {
            let typed = self.typed();
            if typed.len() >= count || Instant::now() >= deadline {
                return typed;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    }
}

const T1: &str = "11111111-1111-4111-8111-111111111111";
const T2: &str = "44444444-4444-4444-8444-444444444444";

#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs a real tmux; run with SV_TMUX_TESTS=1 and --ignored"]
async fn a_task_is_typed_into_a_session_the_runner_created_and_the_next_waits_its_turn() {
    if !enabled() {
        return;
    }
    let Some(program) = find_tmux() else { return };
    let fx = Fixture::new(program, Standin::Normal).await;
    fx.set_view(T1, true, true);
    fx.set_view(T2, true, false);
    let mut server = fx.serve();
    let first = Fixture::deliver(server.url("/webhook"), T1).await;
    assert_eq!(first["status"], "dispatched", "{first}");
    assert_eq!(
        (
            first["channel"].as_str(),
            first["session"].as_str(),
            first["queue_position"].as_u64()
        ),
        (Some("tmux"), Some("SVWL"), Some(1))
    );
    let second = Fixture::deliver(server.url("/webhook"), T2).await;
    assert_eq!(second["queue_position"], 2, "{second}");

    // The first task is typed in as one bracketed paste: the whole message, in order.
    let typed = fx.wait_typed(1, 40).await;
    assert_eq!(
        typed.len(),
        1,
        "{typed:?}\nPANE:\n{}\nLOG:\n{}",
        String::from_utf8_lossy(&fx.tmux.raw(&["capture-pane", "-p", "-t", "=SVWL:"]).stdout),
        server
            .stdout()
            .iter()
            .filter(|l| !l.contains("posture"))
            .map(|l| l.chars().skip(27).take(200).collect::<String>())
            .collect::<Vec<_>>()
            .join("\n")
    );
    assert!(
        typed[0].contains(&format!("[Sylvode task] id={T1} attempt=1/3")),
        "{typed:?}"
    );
    assert!(
        typed[0].contains("Issue: SVWL-7, title (untrusted, quoted from the work item): \"Fix the login\""),
        "{typed:?}"
    );
    assert!(typed[0].contains("sylvode_task_complete"), "{typed:?}");
    // The second is not typed while the first is open, however long we wait.
    tokio::time::sleep(Duration::from_secs(4)).await;
    assert_eq!(fx.typed().len(), 1, "{:?}", fx.typed());
    // The agent acknowledges and completes the first: the second goes in.
    fx.set_view(T1, true, true);
    fx.set_view(T1, false, true);
    let typed = fx.wait_typed(2, 40).await;
    assert_eq!(
        typed.len(),
        2,
        "PANE:\n{}\nLOG:{}\n{typed:?}\n{:#?}\n{:#?}",
        String::from_utf8_lossy(&fx.tmux.raw(&["capture-pane", "-p", "-t", "=SVWL:"]).stdout),
        std::fs::read_to_string(&fx.log)
            .unwrap_or_default()
            .lines()
            .filter(|l| l.starts_with("STEP"))
            .collect::<Vec<_>>()
            .join(" / "),
        server
            .stdout()
            .iter()
            .filter(|l| !l.contains("posture"))
            .map(|l| l.chars().skip(27).take(220).collect::<String>())
            .collect::<Vec<_>>(),
        fx.api
            .requests()
            .iter()
            .map(|r| format!(
                "{} {} {}",
                r.method,
                r.target.chars().skip(15).collect::<String>(),
                r.body
            ))
            .collect::<Vec<_>>()
    );
    assert!(typed[1].contains(&format!("id={T2}")), "{typed:?}");

    // Progress reports: queued, pushed.
    let progress = fx.api.requests_to("POST", "/api/v1/ai/tasks/");
    let kinds: Vec<(String, String)> = progress
        .iter()
        .filter(|r| r.path().ends_with("/progress"))
        .map(|r| {
            (
                r.path().to_string(),
                r.body["kind"].as_str().unwrap_or_default().to_string(),
            )
        })
        .collect();
    assert!(
        kinds.iter().any(|(p, k)| p.contains(T1) && k == "runner.queued"),
        "{kinds:?}"
    );
    assert!(
        kinds.iter().any(|(p, k)| p.contains(T1) && k == "runner.pushed"),
        "{kinds:?}"
    );
    assert!(
        kinds.iter().any(|(p, k)| p.contains(T2) && k == "runner.queued"),
        "{kinds:?}"
    );

    // What the CLI was started with: the runner's files, no token anywhere on its command line or
    // in its environment, and private state files.
    let argv = std::fs::read_to_string(&fx.log)
        .expect("log")
        .lines()
        .find(|l| l.starts_with("argv: "))
        .map(str::to_string)
        .expect("argv");
    assert!(
        argv.contains("--strict-mcp-config") && argv.contains("--setting-sources user"),
        "{argv}"
    );
    assert!(!argv.contains("opr_"), "{argv}");
    let env = std::fs::read_to_string(format!("{}.env", fx.log.display())).unwrap_or_default();
    assert!(!env.contains("opr_") && !env.contains(TOKEN), "{env}");
    // `ps` for the pane's process: no token on any command line of the session.
    let ps = Command::new("/bin/ps").args(["-axo", "args="]).output().expect("ps");
    let ps = String::from_utf8_lossy(&ps.stdout);
    assert!(
        ps.lines()
            .filter(|l| l.contains("svtest-") || l.contains("standin") || l.contains("--mcp-config"))
            .all(|l| !l.contains("opr_")),
        "a token is on a command line"
    );
    let state = fx.tmux.state();
    assert_eq!(
        std::fs::metadata(&state).expect("meta").permissions().mode() & 0o777,
        0o700
    );
    assert_eq!(
        std::fs::metadata(state.join("hook.sock"))
            .expect("meta")
            .permissions()
            .mode()
            & 0o777,
        0o600
    );
    let log = server.stdout().join("\n");
    assert!(!log.contains(TOKEN));
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs a real tmux; run with SV_TMUX_TESTS=1 and --ignored"]
async fn a_session_name_is_matched_exactly_and_another_projects_session_is_left_alone() {
    if !enabled() {
        return;
    }
    let Some(program) = find_tmux() else { return };
    let fx = Fixture::new(program, Standin::Normal).await;
    // Someone else's sessions: a longer name that starts with ours, and a number.
    assert!(
        fx.tmux
            .raw(&["new-session", "-d", "-s", "SVWLX", "-c", "/tmp", "--", "cat"])
            .status
            .success()
    );
    // The server is marked as the runner's own, as one the runner started earlier would be: this
    // test is about the exact session name, not about a foreign server (see the next test).
    assert!(
        fx.tmux
            .raw(&["set-option", "-g", "@sylvode_runner", "1"])
            .status
            .success()
    );
    fx.set_view(T1, true, false);
    let server = fx.serve();
    let answer = Fixture::deliver(server.url("/webhook"), T1).await;
    assert_eq!(answer["status"], "dispatched", "{answer}");
    assert_eq!(fx.wait_typed(1, 40).await.len(), 1);
    let listed = String::from_utf8_lossy(&fx.tmux.raw(&["list-sessions", "-F", "#{session_name}"]).stdout).to_string();
    let mut names: Vec<&str> = listed.lines().collect();
    names.sort_unstable();
    assert_eq!(names, ["SVWL", "SVWLX"], "{listed}");
    // The other session got nothing.
    let other = String::from_utf8_lossy(&fx.tmux.raw(&["capture-pane", "-p", "-t", "=SVWLX:"]).stdout).to_string();
    assert!(!other.contains("Sylvode"), "{other}");
    drop(server);
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs a real tmux; run with SV_TMUX_TESTS=1 and --ignored"]
async fn an_untrusted_directory_fails_the_task_without_a_retry() {
    if !enabled() {
        return;
    }
    let Some(program) = find_tmux() else { return };
    let fx = Fixture::new(program, Standin::Untrusted).await;
    fx.set_view(T1, true, false);
    let server = fx.serve();
    assert_eq!(
        Fixture::deliver(server.url("/webhook"), T1).await["status"],
        "dispatched"
    );
    let fails = fx.api.wait_for("POST", "/fail", 1, Duration::from_secs(40)).await;
    assert_eq!(fails.len(), 1, "{fails:?}");
    assert_eq!(fails[0].body["retryable"], false);
    let reason = fails[0].body["reason"].as_str().expect("reason");
    assert!(
        reason.contains("sylvode-runner trust SVWL") && !reason.contains('/'),
        "{reason}"
    );
    assert_eq!(fails[0].body["source"], "runner");
    assert!(fx.typed().is_empty(), "nothing is typed into a dialog");
    drop(server);
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs a real tmux; run with SV_TMUX_TESTS=1 and --ignored"]
async fn a_killed_session_fails_the_running_task_as_retryable() {
    if !enabled() {
        return;
    }
    let Some(program) = find_tmux() else { return };
    let fx = Fixture::new(program, Standin::Long).await;
    fx.set_view(T1, true, true);
    let server = fx.serve();
    assert_eq!(
        Fixture::deliver(server.url("/webhook"), T1).await["status"],
        "dispatched"
    );
    assert_eq!(fx.wait_typed(1, 40).await.len(), 1);
    // The session is killed under the task.
    assert!(fx.tmux.raw(&["kill-session", "-t", "=SVWL"]).status.success());
    let fails = fx.api.wait_for("POST", "/fail", 1, Duration::from_secs(40)).await;
    assert_eq!(fails.len(), 1, "{fails:?}");
    assert_eq!(fails[0].body["retryable"], true);
    assert_eq!(
        fails[0].body["reason"],
        "tmux session SVWL ended while the task was running"
    );
    drop(server);
}

impl Fixture {
    /// The path of a config file for the operator subcommands.
    fn config_file(&self) -> PathBuf {
        let path = self.bin.file("config.toml");
        std::fs::write(&path, self.config()).expect("config file");
        path
    }

    fn operator(&self, args: &[&str]) -> std::process::Output {
        Command::new(RUNNER)
            .args(args)
            .args(["--config", &self.config_file().to_string_lossy()])
            .env("TMUX_TMPDIR", &self.tmux.dir)
            .env_remove("TMUX")
            .output()
            .expect("run operator command")
    }

    /// An active-task view as Sylvode lists it, with a runner phase.
    fn active_view(&self, task: &str, phase: Option<&str>, acked: bool) -> Value {
        let mut view = self.view(task, true, acked);
        view["runner_phase"] = phase.map_or(Value::Null, |p| json!(p));
        view["status"] = json!(if acked { "acknowledged" } else { "processing" });
        view["max_attempts"] = json!(3);
        view
    }
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs a real tmux; run with SV_TMUX_TESTS=1 and --ignored"]
async fn after_a_runner_restart_a_pushed_task_is_supervised_again_and_the_next_one_follows() {
    if !enabled() {
        return;
    }
    let Some(program) = find_tmux() else { return };
    let fx = Fixture::new(program, Standin::Normal).await;
    fx.set_view(T1, true, true);
    fx.set_view(T2, true, false);
    let mut first = fx.serve();
    assert_eq!(
        Fixture::deliver(first.url("/webhook"), T1).await["status"],
        "dispatched"
    );
    assert_eq!(fx.wait_typed(1, 40).await.len(), 1);
    first.terminate();
    assert!(first.wait_exit(Duration::from_secs(20)).is_some());
    // The session outlives the runner.
    assert!(fx.tmux.raw(&["has-session", "-t", "=SVWL"]).status.success());
    // Sylvode shows T1 acknowledged with a STALE phase (`queued`: the pushed report was lost or
    // overtaken); T2 is delivered after the restart. T1 must not be typed a second time.
    let listed = fx.active_view(T1, Some("queued"), true);
    fx.api.script(|s| s.active = vec![listed]);
    let second = fx.serve();
    // The restarted runner must not push anything while T1 is open.
    assert_eq!(Fixture::deliver(second.url("/webhook"), T2).await["queue_position"], 1);
    tokio::time::sleep(Duration::from_secs(5)).await;
    assert_eq!(fx.typed().len(), 1, "{:?}", fx.typed());
    // T1 ends in Sylvode: the session is free and T2 goes in.
    fx.set_view(T1, false, true);
    let typed = fx.wait_typed(2, 40).await;
    assert_eq!(typed.len(), 2, "{typed:?}");
    assert!(typed[1].contains(&format!("id={T2}")), "{typed:?}");
    assert!(
        fx.api
            .requests_to("POST", "/api/v1/ai/tasks/")
            .iter()
            .all(|r| !r.path().ends_with("/fail"))
    );
    drop(second);
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs a real tmux; run with SV_TMUX_TESTS=1 and --ignored"]
async fn after_a_restart_a_pushed_task_whose_session_is_gone_fails_and_a_queued_one_is_queued_again() {
    if !enabled() {
        return;
    }
    let Some(program) = find_tmux() else { return };
    let fx = Fixture::new(program, Standin::Normal).await;
    fx.set_view(T1, true, true);
    fx.set_view(T2, true, false);
    let mut first = fx.serve();
    assert_eq!(
        Fixture::deliver(first.url("/webhook"), T1).await["status"],
        "dispatched"
    );
    assert_eq!(fx.wait_typed(1, 40).await.len(), 1);
    first.terminate();
    assert!(first.wait_exit(Duration::from_secs(20)).is_some());
    assert!(fx.tmux.raw(&["kill-session", "-t", "=SVWL"]).status.success());
    let (pushed, queued) = (
        fx.active_view(T1, Some("pushed"), true),
        fx.active_view(T2, Some("queued"), false),
    );
    fx.api.script(|s| s.active = vec![pushed, queued]);
    let second = fx.serve();
    let fails = fx.api.wait_for("POST", "/fail", 1, Duration::from_secs(40)).await;
    assert_eq!(fails.len(), 1, "{fails:?}");
    assert!(fails[0].path().contains(T1), "{fails:?}");
    assert_eq!(fails[0].body["retryable"], true);
    assert_eq!(fails[0].body["reason"], "runner restarted and session SVWL was gone");
    // The queued task is queued again: the session is created and T2 is typed.
    let typed = fx.wait_typed(2, 60).await;
    assert!(
        typed.last().is_some_and(|t| t.contains(&format!("id={T2}"))),
        "{typed:?}"
    );
    drop(second);
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs a real tmux; run with SV_TMUX_TESTS=1 and --ignored"]
async fn the_operator_commands_list_refuse_and_release() {
    if !enabled() {
        return;
    }
    let Some(program) = find_tmux() else { return };
    let fx = Fixture::new(program, Standin::Normal).await;
    fx.set_view(T1, true, true);
    let mut server = fx.serve();
    assert_eq!(
        Fixture::deliver(server.url("/webhook"), T1).await["status"],
        "dispatched"
    );
    assert_eq!(fx.wait_typed(1, 40).await.len(), 1);
    // While the runner runs, release and prune are refused (it owns the registry and the sessions).
    for args in [&["release", "SVWL"][..], &["sessions", "--prune"]] {
        let refused = fx.operator(args);
        assert!(
            !refused.status.success() && String::from_utf8_lossy(&refused.stderr).contains("runner stopped")
                || String::from_utf8_lossy(&refused.stderr).contains("needs the runner stopped"),
            "{args:?}: {}",
            String::from_utf8_lossy(&refused.stderr)
        );
    }
    server.terminate();
    assert!(server.wait_exit(Duration::from_secs(20)).is_some());
    let listing = fx.operator(&["sessions"]);
    let text = String::from_utf8_lossy(&listing.stdout).to_string();
    assert!(
        listing.status.success() && text.contains("SVWL") && text.contains("claude") && text.contains(PROJECT),
        "{text}"
    );
    // A registered, tmux-using session is not pruned, whatever its state.
    let pruned = String::from_utf8_lossy(&fx.operator(&["sessions", "--prune"]).stdout).to_string();
    assert!(!pruned.contains("pruned"), "{pruned}");
    assert!(fx.tmux.raw(&["has-session", "-t", "=SVWL"]).status.success());
    // The key cannot be released while its session exists, and a stranger's session is not ours.
    let refused = fx.operator(&["release", "SVWL"]);
    assert!(!refused.status.success() && String::from_utf8_lossy(&refused.stderr).contains("still exists"));
    let stranger = fx.operator(&["trust", "NOPE"]);
    assert!(!stranger.status.success());
    assert!(fx.tmux.raw(&["kill-session", "-t", "=SVWL"]).status.success());
    let released = fx.operator(&["release", "SVWL"]);
    assert!(
        released.status.success(),
        "{}",
        String::from_utf8_lossy(&released.stderr)
    );
    let again = fx.operator(&["release", "SVWL"]);
    assert!(!again.status.success() && String::from_utf8_lossy(&again.stderr).contains("not reserved"));
}

/// A tmux server on the runner's socket that the runner did not start (no owner mark) is refused:
/// its environment and options are somebody else's. The task is failed, nothing is typed, and the
/// foreign session is left alone.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs a real tmux; run with SV_TMUX_TESTS=1 and --ignored"]
async fn a_server_the_runner_did_not_start_is_refused() {
    if !enabled() {
        return;
    }
    let Some(program) = find_tmux() else { return };
    let fx = Fixture::new(program, Standin::Normal).await;
    // Started by "somebody else", with a sentinel in its global environment.
    assert!(
        fx.tmux
            .raw(&["new-session", "-d", "-s", "OTHER", "-c", "/tmp", "--", "cat"])
            .status
            .success()
    );
    assert!(
        fx.tmux
            .raw(&["set-environment", "-g", "SV_FOREIGN_SENTINEL", "leak"])
            .status
            .success()
    );
    fx.set_view(T1, true, false);
    let server = fx.serve();
    assert_eq!(
        Fixture::deliver(server.url("/webhook"), T1).await["status"],
        "dispatched"
    );
    let fails = fx.api.wait_for("POST", "/fail", 1, Duration::from_secs(40)).await;
    assert_eq!(fails.len(), 1, "{fails:?}");
    assert_eq!(fails[0].body["retryable"], false);
    assert!(
        fails[0].body["reason"]
            .as_str()
            .unwrap_or_default()
            .contains("did not start"),
        "{fails:?}"
    );
    assert!(fx.typed().is_empty());
    let listed = String::from_utf8_lossy(&fx.tmux.raw(&["list-sessions", "-F", "#{session_name}"]).stdout).to_string();
    assert_eq!(listed.trim(), "OTHER", "{listed}");
    drop(server);
}

/// Nothing of `env_passthrough` reaches a tmux pane: the pane gets the tmux server's environment,
/// which the runner started with its allow-list only.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs a real tmux; run with SV_TMUX_TESTS=1 and --ignored"]
async fn env_passthrough_and_other_variables_do_not_reach_a_pane() {
    if !enabled() {
        return;
    }
    let Some(program) = find_tmux() else { return };
    let fx = Fixture::new(program, Standin::Normal).await;
    fx.set_view(T1, true, false);
    let config = fx.config().replace(
        "[runner.cli.claude]\n",
        "[runner.cli.claude]\nenv_passthrough = [\"SV_PASS_ME\"]\n",
    );
    let server = Server::start(
        RUNNER,
        &config,
        &[
            ("TMUX_TMPDIR", &fx.tmux.dir.to_string_lossy()),
            ("SV_PASS_ME", "kept-for-headless"),
            ("SV_OTHER_SECRET", "leak"),
            ("GITHUB_TOKEN", "leak"),
        ],
    )
    .expect("starts");
    assert_eq!(
        Fixture::deliver(server.url("/webhook"), T1).await["status"],
        "dispatched"
    );
    assert_eq!(fx.wait_typed(1, 40).await.len(), 1);
    let env = std::fs::read_to_string(format!("{}.env", fx.log.display())).expect("env dump");
    for name in ["SV_PASS_ME", "SV_OTHER_SECRET", "GITHUB_TOKEN"] {
        assert!(!env.contains(name), "{name} reached the pane");
    }
    assert!(!env.contains("leak"), "{env}");
    assert!(env.contains("CLAUDE_CONFIG_DIR="), "the runner's own variable is set");
    drop(server);
}

/// A person attached to the session and typing: a real terminal client (through `script`), so
/// tmux's `client_activity` moves. The input is a terminal focus event, which tmux consumes and counts as
/// activity: the pane's input line stays empty, so only the client's activity can hold a push back. Stopped on drop.
struct TypingClient {
    child: std::process::Child,
    stop: std::sync::Arc<std::sync::atomic::AtomicBool>,
}

impl TypingClient {
    fn start(tmux: &Tmux, key: &str) -> Self {
        use std::io::Write;
        let attach = [
            "-L",
            tmux.socket.as_str(),
            "-f",
            "/dev/null",
            "attach",
            "-t",
            &format!("={key}"),
        ];
        let mut command = if cfg!(target_os = "macos") {
            let mut c = Command::new("script");
            c.args(["-q", "/dev/null"]).arg(&tmux.program).args(attach);
            c
        } else {
            let mut c = Command::new("script");
            let line = format!("{} {}", tmux.program.display(), attach.join(" "));
            c.args(["-qefc", &line, "/dev/null"]);
            c
        };
        let mut child = command
            .env("TMUX_TMPDIR", &tmux.dir)
            .env("TERM", "xterm")
            .env_remove("TMUX")
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .expect("script runs");
        let mut stdin = child.stdin.take().expect("stdin");
        let stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let flag = std::sync::Arc::clone(&stop);
        std::thread::spawn(move || {
            while !flag.load(std::sync::atomic::Ordering::SeqCst) {
                if stdin.write_all(b"\x1b[I").and_then(|()| stdin.flush()).is_err() {
                    break;
                }
                std::thread::sleep(Duration::from_millis(300));
            }
        });
        Self { child, stop }
    }

    /// The person stops typing but stays attached.
    fn stop_typing(&self) {
        self.stop.store(true, std::sync::atomic::Ordering::SeqCst);
    }
}

impl Drop for TypingClient {
    fn drop(&mut self) {
        self.stop_typing();
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// Waits until session `key` exists on the test's server.
async fn wait_session(fx: &Fixture, key: &str, secs: u64) {
    let deadline = Instant::now() + Duration::from_secs(secs);
    while Instant::now() < deadline {
        if fx.tmux.raw(&["has-session", "-t", &format!("={key}")]).status.success() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
    panic!("session {key} never appeared");
}

/// While a task's turn runs the CLI shows an empty input box; with the hooks saying idle (a stray
/// Stop), nothing may be typed into a screen that shows work. Typed as soon as it stops.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs a real tmux; run with SV_TMUX_TESTS=1 and --ignored"]
async fn nothing_is_typed_into_a_busy_screen_that_shows_an_empty_prompt() {
    if !enabled() {
        return;
    }
    let Some(program) = find_tmux() else { return };
    let fx = Fixture::new(program, Standin::Normal).await;
    fx.set_view(T1, true, true);
    fx.set_view(T2, true, false);
    let server = fx.serve();
    assert_eq!(
        Fixture::deliver(server.url("/webhook"), T1).await["status"],
        "dispatched"
    );
    assert_eq!(fx.wait_typed(1, 40).await.len(), 1);
    // T1 ends in Sylvode; the CLI keeps showing work (and an empty prompt) after its Stop hook.
    fx.set_mode("busy");
    fx.set_view(T1, false, true);
    assert_eq!(
        Fixture::deliver(server.url("/webhook"), T2).await["status"],
        "dispatched"
    );
    tokio::time::sleep(Duration::from_secs(7)).await;
    assert_eq!(fx.typed().len(), 1, "typed into a busy screen: {:?}", fx.typed());
    fx.set_mode("idle");
    let typed = fx.wait_typed(2, 30).await;
    assert_eq!(typed.len(), 2, "{typed:?}");
    drop(server);
}

/// A shell (or any program that is not the CLI) in the foreground: the foreground check refuses,
/// although the screen shows an empty prompt.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs a real tmux; run with SV_TMUX_TESTS=1 and --ignored"]
async fn nothing_is_typed_when_the_foreground_is_not_the_cli() {
    if !enabled() {
        return;
    }
    let Some(program) = find_tmux() else { return };
    let fx = Fixture::new(program, Standin::Normal).await;
    fx.set_view(T1, true, true);
    fx.set_view(T2, true, false);
    let server = fx.serve();
    assert_eq!(
        Fixture::deliver(server.url("/webhook"), T1).await["status"],
        "dispatched"
    );
    assert_eq!(fx.wait_typed(1, 40).await.len(), 1);
    tokio::time::sleep(Duration::from_secs(3)).await;
    // The CLI is replaced by another program that leaves the prompt on the screen.
    fx.set_mode("shell");
    tokio::time::sleep(Duration::from_secs(1)).await;
    fx.set_view(T1, false, true);
    assert_eq!(
        Fixture::deliver(server.url("/webhook"), T2).await["status"],
        "dispatched"
    );
    tokio::time::sleep(Duration::from_secs(8)).await;
    let pane = String::from_utf8_lossy(&fx.tmux.raw(&["capture-pane", "-p", "-t", "=SVWL:"]).stdout).to_string();
    assert!(!pane.contains(T2), "text reached the foreground program: {pane}");
    assert_eq!(fx.typed().len(), 1);
    drop(server);
}

/// A permission dialog, an elicitation dialog, a placeholder in the box and a person's draft: none
/// is typed into; the task goes in once the screen is a plain empty prompt again.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs a real tmux; run with SV_TMUX_TESTS=1 and --ignored"]
async fn nothing_is_typed_over_dialogs_placeholders_or_drafts() {
    if !enabled() {
        return;
    }
    let Some(program) = find_tmux() else { return };
    for mode in ["permission", "elicitation", "placeholder", "draft"] {
        let fx = Fixture::new(program.clone(), Standin::Normal).await;
        fx.set_mode(mode);
        fx.set_view(T1, true, false);
        let server = fx.serve();
        assert_eq!(
            Fixture::deliver(server.url("/webhook"), T1).await["status"],
            "dispatched"
        );
        tokio::time::sleep(Duration::from_secs(7)).await;
        assert!(fx.typed().is_empty(), "{mode}: typed over the screen");
        fx.set_mode("idle");
        assert_eq!(
            fx.wait_typed(1, 30).await.len(),
            1,
            "{mode}: not typed once the screen was plain"
        );
        drop(server);
    }
}

/// A person attached to the session and typing: nothing is typed for `user_idle_secs` (the real
/// default, 10 s) after the last key.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs a real tmux; run with SV_TMUX_TESTS=1 and --ignored"]
async fn nothing_is_typed_while_a_person_types() {
    if !enabled() {
        return;
    }
    let Some(program) = find_tmux() else { return };
    let fx = Fixture::new(program, Standin::Normal).await;
    fx.set_view(T1, true, false);
    // The screen shows work until the person is attached, so the first push waits for them.
    fx.set_mode("busy");
    let server = fx.serve();
    assert_eq!(
        Fixture::deliver(server.url("/webhook"), T1).await["status"],
        "dispatched"
    );
    wait_session(&fx, "SVWL", 30).await;
    let client = TypingClient::start(&fx.tmux, "SVWL");
    tokio::time::sleep(Duration::from_secs(2)).await;
    fx.set_mode("idle");
    tokio::time::sleep(Duration::from_secs(7)).await;
    assert!(fx.typed().is_empty(), "typed while a person was typing");
    client.stop_typing();
    // Ten quiet seconds later it goes in.
    let typed = fx.wait_typed(1, 40).await;
    assert_eq!(
        typed.len(),
        1,
        "PANE:\n{}\nCLIENTS:{}\nNOW:{:?}",
        String::from_utf8_lossy(&fx.tmux.raw(&["capture-pane", "-p", "-t", "=SVWL:"]).stdout),
        String::from_utf8_lossy(
            &fx.tmux
                .raw(&["list-clients", "-F", "#{client_activity}", "-t", "=SVWL"])
                .stdout
        ),
        std::time::SystemTime::now()
    );
    drop(client);
    drop(server);
}

/// The Enter retry presses Enter only when the pasted text is still on the input line; a person's
/// draft in its place means nothing is pressed and the session is reported blocked.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs a real tmux; run with SV_TMUX_TESTS=1 and --ignored"]
async fn the_enter_retry_needs_the_pasted_text_on_the_input_line() {
    if !enabled() {
        return;
    }
    let Some(program) = find_tmux() else { return };
    // Deaf CLI whose input line still shows the paste: one Enter is pressed again.
    let fx = Fixture::new(program.clone(), Standin::Normal).await;
    fx.set_mode("deaf");
    fx.set_view(T1, true, false);
    let server = fx.serve();
    assert_eq!(
        Fixture::deliver(server.url("/webhook"), T1).await["status"],
        "dispatched"
    );
    assert_eq!(fx.wait_typed(1, 40).await.len(), 1);
    tokio::time::sleep(Duration::from_secs(14)).await;
    assert_eq!(fx.enters(), 1, "one retry, with the text on the line");
    drop(server);
    // The same, but a person replaced the line with a draft: nothing is pressed.
    let fx = Fixture::new(program, Standin::Normal).await;
    fx.set_mode("deaf");
    fx.set_view(T1, true, false);
    let server = fx.serve();
    assert_eq!(
        Fixture::deliver(server.url("/webhook"), T1).await["status"],
        "dispatched"
    );
    assert_eq!(fx.wait_typed(1, 40).await.len(), 1);
    fx.set_mode("draft");
    let blocked = fx
        .api
        .wait_for("POST", "/progress", 3, Duration::from_secs(25))
        .await
        .iter()
        .any(|r| r.body["kind"] == "runner.blocked");
    assert!(blocked, "the session was not reported blocked");
    assert_eq!(fx.enters(), 0, "an Enter was pressed over a draft");
    drop(server);
}

/// The trust flow end to end: the dialog fails the task (not retryable) naming `trust`; the
/// operator answers it in the session; the CLI starts; the next task is typed.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs a real tmux; run with SV_TMUX_TESTS=1 and --ignored"]
async fn the_trust_dialog_flow_ends_with_the_next_task_typed() {
    if !enabled() {
        return;
    }
    let Some(program) = find_tmux() else { return };
    let fx = Fixture::new(program, Standin::Untrusted).await;
    fx.set_view(T1, true, false);
    fx.set_view(T2, true, false);
    let server = fx.serve();
    assert_eq!(
        Fixture::deliver(server.url("/webhook"), T1).await["status"],
        "dispatched"
    );
    let fails = fx.api.wait_for("POST", "/fail", 1, Duration::from_secs(40)).await;
    assert_eq!(fails.len(), 1, "{fails:?}");
    assert!(
        fails[0].body["reason"]
            .as_str()
            .unwrap_or_default()
            .contains("sylvode-runner trust SVWL")
    );
    // The operator attaches and answers (what `trust` leads to).
    assert!(
        fx.tmux
            .raw(&["send-keys", "-t", "=SVWL:", "1", "Enter"])
            .status
            .success()
    );
    tokio::time::sleep(Duration::from_secs(3)).await;
    assert_eq!(
        Fixture::deliver(server.url("/webhook"), T2).await["status"],
        "dispatched"
    );
    let typed = fx.wait_typed(1, 40).await;
    assert_eq!(typed.len(), 1, "{typed:?}");
    assert!(typed[0].contains(T2));
    drop(server);
}
