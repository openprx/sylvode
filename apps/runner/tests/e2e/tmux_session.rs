//! The tmux channel end to end: the real runner, a real tmux server of its own (random socket, a
//! private `TMUX_TMPDIR` under `/tmp`, killed at the end), a mock Sylvode API and
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
/// A second project of the same bot.
const PROJECT_B: &str = "55555555-5555-4555-8555-555555555555";
/// Another bot, for the record of a session it serves.
const OTHER_BOT: &str = "00000000-0000-4000-8000-000000000002";

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
            .args(["-L", &self.socket])
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
    /// The CLI of the project's connection: `claude`, or `codex` (which also enables codex).
    cli: &'static str,
    /// Extra configuration appended last: the `[[runner.tmux.sessions]]` entries of a test.
    sessions: String,
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
        // The same program under codex's name is codex's stand-in (the name selects its flavour).
        std::fs::copy(standin_binary(), bin.file("codex")).expect("install the codex stand-in");
        write_script(bin.path(), "mcp-server", "exit 0");
        Self {
            api,
            tmux,
            root,
            project,
            bin,
            log,
            cli: "claude",
            sessions: String::new(),
        }
    }

    /// The bot's claude tasks, from every project, go to the existing tmux session `name`.
    fn named(mut self, name: &str) -> Self {
        self.sessions = format!("[[runner.tmux.sessions]]\nbot = \"{BOT}\"\ncli = \"claude\"\nnames = [\"{name}\"]\n");
        self
    }

    /// The state directory of the bot's claude slot.
    fn bot_slot_dir(&self) -> PathBuf {
        self.tmux.state().join(format!("sessions/bot-{BOT}-claude"))
    }

    /// Starts the stand-in claude, as an operator would, in a tmux session called `name`. Its
    /// settings file does not exist, so it reports no hooks.
    fn start_operator_session(&self, name: &str) {
        let settings = self.tmux.state().join("sessions/SVWL-claude/claude-settings.json");
        assert!(
            self.tmux
                .raw(&[
                    "new-session",
                    "-d",
                    "-s",
                    name,
                    "-c",
                    &self.project.to_string_lossy(),
                    "--",
                    &self.bin.file("claude").to_string_lossy(),
                    "--settings",
                    &settings.to_string_lossy(),
                ])
                .status
                .success()
        );
    }

    fn pane_pid(&self, target: &str) -> String {
        String::from_utf8_lossy(
            &self
                .tmux
                .raw(&["display-message", "-p", "-t", target, "#{pane_pid}"])
                .stdout,
        )
        .trim()
        .to_string()
    }

    fn pane_id(&self, target: &str) -> String {
        String::from_utf8_lossy(
            &self
                .tmux
                .raw(&["display-message", "-p", "-t", target, "#{pane_id}"])
                .stdout,
        )
        .trim()
        .to_string()
    }

    fn has_session(&self, name: &str) -> bool {
        self.tmux
            .raw(&["has-session", "-t", &format!("={name}")])
            .status
            .success()
    }

    /// A fixture whose project connection uses codex (enabled on the runner).
    async fn codex(tmux_program: PathBuf, standin: Standin) -> Self {
        let mut fx = Self::new(tmux_program, standin).await;
        fx.cli = "codex";
        fx
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
             [[ai_task_callback.bots]]\nbot_id = \"{BOT}\"\nworkspace_id = \"11111111-1111-4111-8111-111111111111\"\ntoken = \"{TOKEN}\"\nsurface = \"mcp_stdio\"\n\
             [runner]\nstate_dir = \"{}\"\nallowed_roots = [\"{}\"]\nreport_interval_secs = 600\ndelivery_check_timeout_secs = 3\n\
             [runner.tmux]\nbinary = \"{}\"\nsocket_name = \"{}\"\npush_settle_ms = 100\nready_timeout_secs = 20\n\
             [runner.cli.claude]\nbinary = \"{standin}\"\nlaunch_command = [\"{standin}\", \"--permission-mode\", \"bypassPermissions\"]\n\
             [runner.cli.codex]\nenabled = {codex}\nbinary = \"{codex_bin}\"\n\
             launch_command = [\"{codex_bin}\", \"--no-daemon\", \"--no-alt-screen\", \"--dangerously-bypass-approvals-and-sandbox\"]\n\
             push_settle_ms = 500\n\
             [runner.mcp]\nserver_binary = \"{}\"\n\
             [[agents]]\nid = \"{BOT}\"\nname = \"Bot\"\nagent_type = \"runner\"\n[agents.route]\nbot_ids = [\"{BOT}\"]\n{sessions}",
            self.api.url(),
            self.tmux.state().display(),
            canonical.display(),
            self.tmux.program.display(),
            self.tmux.socket,
            self.bin.file("mcp-server").display(),
            standin = self.bin.file("claude").display(),
            codex = self.cli == "codex",
            codex_bin = self.bin.file("codex").display(),
            sessions = self.sessions,
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
        self.view_for(self.cli, task, active, acked)
    }

    fn view_for(&self, cli: &str, task: &str, active: bool, acked: bool) -> Value {
        json!({
            "task_id": task, "attempt": 1, "active": active,
            "acknowledged_at": if acked { json!("2026-11-22T10:00:00Z") } else { Value::Null },
            "source": {"kind": "issue_assignment", "issue": {"id": "33333333-3333-4333-8333-333333333333", "identifier": "SVWL-7", "title": "Fix the login"}},
            "project": {"id": PROJECT, "key": "SVWL", "name": "n", "workspace_id": PROJECT},
            "execution": {"enabled": true, "channel": "tmux", "cli": cli,
                          "working_directory": self.project.to_string_lossy(), "approval_gate": "none", "version": 1},
        })
    }

    /// The view of a task of project RFGUI (a second project of the same bot) whose working
    /// directory lies outside the allowed roots, on `channel`.
    fn view_b(&self, task: &str, channel: &str, active: bool, acked: bool) -> Value {
        let mut view = self.view_for("claude", task, active, acked);
        view["source"]["issue"]["identifier"] = json!("RFGUI-3");
        view["project"] = json!({"id": PROJECT_B, "key": "RFGUI", "name": "b", "workspace_id": PROJECT_B});
        view["execution"]["channel"] = json!(channel);
        view["execution"]["working_directory"] = json!("/var/empty/not-allowed");
        view
    }

    fn set_view_b(&self, task: &str, channel: &str, active: bool, acked: bool) {
        let view = self.view_b(task, channel, active, acked);
        self.api.script(|s| {
            s.views.insert(task.to_string(), vec![(200, view)]);
        });
    }

    /// The delivery of a task of project RFGUI.
    fn delivery_b(task: &str) -> String {
        json!({
            "task_id": task, "project_id": PROJECT_B, "ai_participant_id": BOT, "attempts": 1, "max_attempts": 3,
            "project_key": "RFGUI", "protocol": 1, "trigger_kind": "assigned",
        })
        .to_string()
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
        (Some("tmux"), Some("SVWL-claude"), Some(1))
    );
    let second = Fixture::deliver(server.url("/webhook"), T2).await;
    assert_eq!(second["queue_position"], 2, "{second}");

    // The first task is typed in as one bracketed paste: the whole message, in order.
    let typed = fx.wait_typed(1, 40).await;
    assert_eq!(
        typed.len(),
        1,
        "{typed:?}\nPANE:\n{}\nLOG:\n{}",
        String::from_utf8_lossy(&fx.tmux.raw(&["capture-pane", "-p", "-t", "=SVWL-claude:"]).stdout),
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
        String::from_utf8_lossy(&fx.tmux.raw(&["capture-pane", "-p", "-t", "=SVWL-claude:"]).stdout),
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
    // The launch command, the runner's MCP file and its hook settings; nothing stricter.
    assert!(
        argv.contains("--permission-mode bypassPermissions")
            && argv.contains("--mcp-config ")
            && argv.contains("--settings ")
            && !argv.contains("--strict-mcp-config")
            && !argv.contains("--setting-sources")
            && !argv.contains("--allowedTools"),
        "{argv}"
    );
    assert!(!argv.contains("opr_"), "{argv}");
    let env = std::fs::read_to_string(fx.tmux.state().join("standin.env.values")).unwrap_or_default();
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
    // The session the runner started: hooks were seen, the MCP file is in its arguments, and the
    // default launch command runs with permission checks off.
    let row = fx.session_row();
    let detail = &row["detail"];
    assert_eq!(detail["state"], "running", "{row}");
    assert_eq!(detail["created_by"], "runner", "{row}");
    assert!(
        detail["hooks"].as_str().is_some_and(|h| h.starts_with("seen ")),
        "{row}"
    );
    assert_eq!(detail["mcp"], "ok", "{row}");
    assert_eq!(detail["permissions"], "bypass", "{row}");
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs a real tmux; run with SV_TMUX_TESTS=1 and --ignored"]
async fn a_session_name_is_matched_exactly_and_other_sessions_are_left_alone() {
    if !enabled() {
        return;
    }
    let Some(program) = find_tmux() else { return };
    let fx = Fixture::new(program, Standin::Normal).await;
    // Someone else's sessions: a longer name that starts with ours, and the bare project key.
    for other in ["SVWL-claudeX", "SVWL"] {
        assert!(
            fx.tmux
                .raw(&["new-session", "-d", "-s", other, "-c", "/tmp", "--", "cat"])
                .status
                .success()
        );
    }
    fx.set_view(T1, true, false);
    let server = fx.serve();
    let answer = Fixture::deliver(server.url("/webhook"), T1).await;
    assert_eq!(answer["status"], "dispatched", "{answer}");
    assert_eq!(fx.wait_typed(1, 40).await.len(), 1);
    let listed = String::from_utf8_lossy(&fx.tmux.raw(&["list-sessions", "-F", "#{session_name}"]).stdout).to_string();
    let mut names: Vec<&str> = listed.lines().collect();
    names.sort_unstable();
    assert_eq!(names, ["SVWL", "SVWL-claude", "SVWL-claudeX"], "{listed}");
    // The others got nothing.
    for other in ["=SVWL-claudeX:", "=SVWL:"] {
        let pane = String::from_utf8_lossy(&fx.tmux.raw(&["capture-pane", "-p", "-t", other]).stdout).to_string();
        assert!(!pane.contains("Sylvode"), "{other}: {pane}");
    }
    drop(server);
}

/// A session the operator started is used as it is: the runner types into the pane that runs the
/// CLI, records that the operator started it, and never restarts it.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs a real tmux; run with SV_TMUX_TESTS=1 and --ignored"]
async fn a_session_the_operator_started_is_used_as_it_is_and_never_restarted() {
    if !enabled() {
        return;
    }
    let Some(program) = find_tmux() else { return };
    let fx = Fixture::new(program, Standin::Normal).await;
    // The operator's own `claude` (the stand-in) in the derived session. Its settings file does
    // not exist, so it reports no hooks: the runner reads its turns from the screen.
    let settings = fx.tmux.state().join("sessions/SVWL-claude/claude-settings.json");
    assert!(
        fx.tmux
            .raw(&[
                "new-session",
                "-d",
                "-s",
                "SVWL-claude",
                "-c",
                &fx.project.to_string_lossy(),
                "--",
                &fx.bin.file("claude").to_string_lossy(),
                "--settings",
                &settings.to_string_lossy(),
            ])
            .status
            .success()
    );
    let pane_pid = || {
        String::from_utf8_lossy(
            &fx.tmux
                .raw(&["display-message", "-p", "-t", "=SVWL-claude:", "#{pane_pid}"])
                .stdout,
        )
        .trim()
        .to_string()
    };
    let before = pane_pid();
    fx.set_view(T1, true, true);
    let server = fx.serve();
    let answer = Fixture::deliver(server.url("/webhook"), T1).await;
    assert_eq!(answer["status"], "dispatched", "{answer}");
    assert_eq!(fx.wait_typed(1, 40).await.len(), 1);
    assert_eq!(pane_pid(), before, "the CLI was restarted");
    let record = std::fs::read_to_string(fx.tmux.state().join("sessions/SVWL-claude/session.json")).expect("record");
    assert!(
        record.contains("\"created_by\": \"operator\"") && record.contains(BOT),
        "{record}"
    );
    drop(server);
}

/// A session without the CLI (a leftover shell) fails the task with the attach hint; nothing is
/// typed, nothing is restarted.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs a real tmux; run with SV_TMUX_TESTS=1 and --ignored"]
async fn a_session_without_the_cli_fails_the_task_and_nothing_is_typed() {
    if !enabled() {
        return;
    }
    let Some(program) = find_tmux() else { return };
    let fx = Fixture::new(program, Standin::Normal).await;
    assert!(
        fx.tmux
            .raw(&["new-session", "-d", "-s", "SVWL-claude", "-c", "/tmp", "--", "cat"])
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
    assert_eq!(fails[0].body["retryable"], true);
    let reason = fails[0].body["reason"].as_str().expect("reason");
    assert!(
        reason.contains("not running in tmux session SVWL-claude") && reason.contains("tmux attach -t SVWL-claude"),
        "{reason}"
    );
    let pane = String::from_utf8_lossy(&fx.tmux.raw(&["capture-pane", "-p", "-t", "=SVWL-claude:"]).stdout).to_string();
    assert!(
        !pane.contains("Sylvode"),
        "typed into a program that is not the CLI: {pane}"
    );
    assert!(fx.typed().is_empty());
    drop(server);
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs a real tmux; run with SV_TMUX_TESTS=1 and --ignored"]
async fn a_first_start_dialog_fails_the_task_as_retryable_with_the_attach_hint() {
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
    assert_eq!(fails[0].body["retryable"], true);
    let reason = fails[0].body["reason"].as_str().expect("reason");
    assert!(
        reason.contains("shows a dialog; attach with tmux attach -t SVWL-claude and answer it")
            && !reason.contains('/'),
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
    assert!(fx.tmux.raw(&["kill-session", "-t", "=SVWL-claude"]).status.success());
    let fails = fx.api.wait_for("POST", "/fail", 1, Duration::from_secs(40)).await;
    assert_eq!(fails.len(), 1, "{fails:?}");
    assert_eq!(fails[0].body["retryable"], true);
    assert_eq!(
        fails[0].body["reason"],
        "tmux session SVWL-claude ended while the task was running"
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
    restart_keeps_a_pushed_task(None).await;
}

/// The same with a configured session: the record names the tmux session the task was typed into,
/// so it is the one looked at after the restart, and nothing is typed twice.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs a real tmux; run with SV_TMUX_TESTS=1 and --ignored"]
async fn after_a_runner_restart_a_pushed_task_in_a_configured_session_is_supervised_again() {
    restart_keeps_a_pushed_task(Some("sv")).await;
}

async fn restart_keeps_a_pushed_task(configured: Option<&str>) {
    if !enabled() {
        return;
    }
    let Some(program) = find_tmux() else { return };
    let mut fx = Fixture::new(program, Standin::Normal).await;
    let session = configured.unwrap_or("SVWL-claude");
    if let Some(name) = configured {
        fx = fx.named(name);
        fx.start_operator_session(name);
    }
    fx.set_view(T1, true, true);
    // With a bot session the second task comes from another project of the bot.
    if configured.is_some() {
        fx.set_view_b(T2, "headless", true, false);
    } else {
        fx.set_view(T2, true, false);
    }
    let mut first = fx.serve();
    assert_eq!(
        Fixture::deliver(first.url("/webhook"), T1).await["status"],
        "dispatched"
    );
    assert_eq!(fx.wait_typed(1, 40).await.len(), 1);
    first.terminate();
    assert!(first.wait_exit(Duration::from_secs(20)).is_some());
    // The session outlives the runner.
    assert!(fx.has_session(session));
    // Sylvode shows T1 acknowledged with a STALE phase (`queued`: the pushed report was lost or
    // overtaken); T2 is delivered after the restart. T1 must not be typed a second time.
    let listed = fx.active_view(T1, Some("queued"), true);
    fx.api.script(|s| s.active = vec![listed]);
    let second = fx.serve();
    // The restarted runner must not push anything while T1 is open.
    let second_delivery = if configured.is_some() {
        post_signed(&second.url("/webhook"), &Fixture::delivery_b(T2)).await.1
    } else {
        Fixture::deliver(second.url("/webhook"), T2).await
    };
    assert_eq!(second_delivery["queue_position"], 1, "{second_delivery}");
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
    if configured.is_some() {
        assert!(typed[1].contains("|Project: RFGUI|"), "{typed:?}");
        assert!(!fx.has_session("SVWL-claude"), "a derived session was created");
        assert!(!fx.has_session("RFGUI-claude"), "a derived session was created");
        let record = std::fs::read_to_string(fx.bot_slot_dir().join("session.json")).expect("record");
        assert!(
            record.contains("\"tmux_name\": \"sv\"")
                && record.contains("\"name_source\": \"local\"")
                && record.contains(&format!("\"bot_id\": \"{BOT}\"")),
            "{record}"
        );
    }
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
    assert!(fx.tmux.raw(&["kill-session", "-t", "=SVWL-claude"]).status.success());
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
    assert_eq!(
        fails[0].body["reason"],
        "runner restarted and session SVWL-claude was gone"
    );
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
async fn the_operator_commands_list_and_release() {
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
    // While the runner runs, release is refused (it owns the registry).
    let refused = fx.operator(&["release", "SVWL"]);
    assert!(
        !refused.status.success() && String::from_utf8_lossy(&refused.stderr).contains("needs the runner stopped"),
        "{}",
        String::from_utf8_lossy(&refused.stderr)
    );
    server.terminate();
    assert!(server.wait_exit(Duration::from_secs(20)).is_some());
    // The listing names the session, its project, the CLI and who started it; it changes nothing.
    let listing = fx.operator(&["sessions"]);
    let text = String::from_utf8_lossy(&listing.stdout).to_string();
    assert!(
        listing.status.success()
            && text.contains("SVWL-claude")
            && text.contains("claude")
            && text.contains(PROJECT)
            && text.contains("runner"),
        "{text}"
    );
    // `trust` and `sessions --prune` do not exist any more, and nothing was ended.
    assert!(!fx.operator(&["sessions", "--prune"]).status.success());
    assert!(fx.tmux.raw(&["has-session", "-t", "=SVWL-claude"]).status.success());
    // The key cannot be released while its session exists.
    let refused = fx.operator(&["release", "SVWL"]);
    assert!(!refused.status.success() && String::from_utf8_lossy(&refused.stderr).contains("still exists"));
    assert!(fx.tmux.raw(&["kill-session", "-t", "=SVWL-claude"]).status.success());
    let released = fx.operator(&["release", "SVWL"]);
    assert!(
        released.status.success(),
        "{}",
        String::from_utf8_lossy(&released.stderr)
    );
    let again = fx.operator(&["release", "SVWL"]);
    assert!(!again.status.success() && String::from_utf8_lossy(&again.stderr).contains("not reserved"));
}

/// The CLI of a session the runner creates has the operator's environment (the runner's own, minus
/// the agent variables) and no configuration directory of the runner's.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs a real tmux; run with SV_TMUX_TESTS=1 and --ignored"]
async fn a_session_the_runner_creates_has_the_operators_environment() {
    if !enabled() {
        return;
    }
    let Some(program) = find_tmux() else { return };
    let fx = Fixture::new(program, Standin::Normal).await;
    fx.set_view(T1, true, false);
    let server = Server::start(
        RUNNER,
        &fx.config(),
        &[
            ("TMUX_TMPDIR", &fx.tmux.dir.to_string_lossy()),
            ("SVT_OPERATOR_SETTING", "operator"),
            ("SVT_OPERATOR_TOKEN", "operator"),
            ("CLAUDECODE", "1"),
            ("CLAUDE_CODE_ENTRYPOINT", "cli"),
            ("TMUX", "/tmp/tmux-0/default,1,0"),
        ],
    )
    .expect("starts");
    assert_eq!(
        Fixture::deliver(server.url("/webhook"), T1).await["status"],
        "dispatched"
    );
    assert_eq!(fx.wait_typed(1, 40).await.len(), 1);
    let names = std::fs::read_to_string(fx.tmux.state().join("standin.env.names")).expect("names");
    let values = std::fs::read_to_string(fx.tmux.state().join("standin.env.values")).expect("values");
    for name in ["SVT_OPERATOR_SETTING", "SVT_OPERATOR_TOKEN", "PATH", "HOME"] {
        assert!(
            names.lines().any(|n| n == name),
            "{name} did not reach the pane: {names}"
        );
    }
    for gone in ["CLAUDECODE", "CLAUDE_CODE_ENTRYPOINT"] {
        assert!(!names.lines().any(|n| n == gone), "{gone} reached the pane");
    }
    assert!(values.contains("SVT_OPERATOR_SETTING=operator"), "{values}");
    assert!(
        !values.contains("CLAUDE_CONFIG_DIR="),
        "the runner sets no configuration directory: {values}"
    );
    assert!(!fx.tmux.state().join("claude").exists());
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
        let attach = ["-L", tmux.socket.as_str(), "attach", "-t", &format!("={key}")];
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
    let pane = String::from_utf8_lossy(&fx.tmux.raw(&["capture-pane", "-p", "-t", "=SVWL-claude:"]).stdout).to_string();
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
    wait_session(&fx, "SVWL-claude", 30).await;
    let client = TypingClient::start(&fx.tmux, "SVWL-claude");
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
        String::from_utf8_lossy(&fx.tmux.raw(&["capture-pane", "-p", "-t", "=SVWL-claude:"]).stdout),
        String::from_utf8_lossy(
            &fx.tmux
                .raw(&["list-clients", "-F", "#{client_activity}", "-t", "=SVWL-claude"])
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

/// The first-start dialog end to end: it fails the task naming the attach command; the operator
/// answers it in the session; the CLI starts; the next task is typed.
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
            .contains("tmux attach -t SVWL-claude")
    );
    // The operator attaches and answers (what `trust` leads to).
    assert!(
        fx.tmux
            .raw(&["send-keys", "-t", "=SVWL-claude:", "1", "Enter"])
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

/// The allowed roots matter only when the runner would create the session: with none, a working
/// directory outside them is refused synchronously and no session is made; with the operator's
/// session in place the same task is typed into it.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs a real tmux; run with SV_TMUX_TESTS=1 and --ignored"]
async fn a_directory_outside_the_roots_matters_only_when_the_runner_would_create_the_session() {
    if !enabled() {
        return;
    }
    let Some(program) = find_tmux() else { return };
    let fx = Fixture::new(program, Standin::Normal).await;
    let outside = fx
        .root
        .path()
        .parent()
        .map_or_else(|| PathBuf::from("/tmp"), PathBuf::from);
    let outside = std::fs::canonicalize(outside).expect("outside");
    let set_outside = |task: &str| {
        let mut view = fx.view(task, true, true);
        view["execution"]["working_directory"] = json!(outside.to_string_lossy());
        fx.api.script(|s| {
            s.views.insert(task.to_string(), vec![(200, view.clone())]);
        });
    };
    set_outside(T1);
    let server = fx.serve();
    let refused = Fixture::deliver(server.url("/webhook"), T1).await;
    assert_eq!(refused["status"], "refused", "{refused}");
    assert_eq!(refused["reason_code"], "path_outside_allowlist", "{refused}");
    assert!(
        !fx.tmux.raw(&["has-session", "-t", "=SVWL-claude"]).status.success(),
        "no session was created"
    );
    // The operator's own session exists: the directory is not the runner's concern.
    let settings = fx.tmux.state().join("sessions/SVWL-claude/claude-settings.json");
    assert!(
        fx.tmux
            .raw(&[
                "new-session",
                "-d",
                "-s",
                "SVWL-claude",
                "-c",
                &fx.project.to_string_lossy(),
                "--",
                &fx.bin.file("claude").to_string_lossy(),
                "--settings",
                &settings.to_string_lossy(),
            ])
            .status
            .success()
    );
    set_outside(T2);
    let answer = Fixture::deliver(server.url("/webhook"), T2).await;
    assert_eq!(answer["status"], "dispatched", "{answer}");
    assert_eq!(fx.wait_typed(1, 40).await.len(), 1);
    drop(server);
}

impl Fixture {
    /// `doctor --json --project SVWL` and its `session.SVWL-claude` row.
    fn session_row(&self) -> Value {
        self.row("session.SVWL-claude")
    }

    /// The `session.bot-<uuid>-claude` row of `doctor --json --project SVWL`.
    fn bot_row(&self) -> Value {
        self.row(&format!("session.bot-{BOT}-claude"))
    }

    /// One row of `doctor --json --project SVWL`.
    fn row(&self, component: &str) -> Value {
        let output = Command::new(RUNNER)
            .args(["doctor", "--json", "--project", "SVWL", "--config"])
            .arg(self.config_file())
            .env("TMUX_TMPDIR", &self.tmux.dir)
            .env_remove("TMUX")
            .output()
            .expect("run doctor");
        let report: Value = serde_json::from_slice(&output.stdout).expect("doctor json");
        report["components"]
            .as_array()
            .and_then(|rows| rows.iter().find(|row| row["component"] == component))
            .cloned()
            .unwrap_or_else(|| panic!("no {component} row: {report}"))
    }
}

/// `doctor --project` for a session the operator started: every field of the row, read from tmux
/// and the CLI's process (the stand-in is started with the bypass flag and this bot's MCP file).
#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs a real tmux; run with SV_TMUX_TESTS=1 and --ignored"]
async fn doctor_describes_the_session_of_a_project() {
    if !enabled() {
        return;
    }
    let Some(program) = find_tmux() else { return };
    let fx = Fixture::new(program, Standin::Normal).await;
    // No session yet: absent, and that is fine.
    let absent = fx.session_row();
    assert_eq!(absent["detail"]["state"], "absent", "{absent}");
    assert_eq!(absent["status"], "ok");
    assert_eq!(absent["detail"]["tmux_session"], "SVWL-claude", "{absent}");
    assert_eq!(absent["detail"]["name_source"], "derived", "{absent}");
    assert_eq!(absent["found"], false);
    // The operator's claude, with permission checks off and this bot's MCP file in its arguments.
    let state = fx.tmux.state();
    let settings = state.join("sessions/SVWL-claude/claude-settings.json");
    let mcp_json = state.join(format!("mcp/{BOT}.claude.json"));
    assert!(
        fx.tmux
            .raw(&[
                "new-session",
                "-d",
                "-s",
                "SVWL-claude",
                "-c",
                &fx.project.to_string_lossy(),
                "--",
                &fx.bin.file("claude").to_string_lossy(),
                "--permission-mode",
                "bypassPermissions",
                "--mcp-config",
                &mcp_json.to_string_lossy(),
                "--settings",
                &settings.to_string_lossy(),
            ])
            .status
            .success()
    );
    fx.set_view(T1, true, true);
    let server = fx.serve();
    assert_eq!(
        Fixture::deliver(server.url("/webhook"), T1).await["status"],
        "dispatched"
    );
    assert_eq!(fx.wait_typed(1, 40).await.len(), 1);
    drop(server);
    let row = fx.session_row();
    let detail = &row["detail"];
    assert_eq!(detail["state"], "running", "{row}");
    assert_eq!(detail["created_by"], "operator", "{row}");
    assert_eq!(detail["permissions"], "bypass", "{row}");
    assert_eq!(detail["mcp"], "ok", "{row}");
    assert_eq!(detail["hooks"], "none", "no hook runs without settings: {row}");
    assert!(
        detail["pane"].as_str().is_some_and(|pane| pane.starts_with('%')),
        "{row}"
    );
    assert_eq!(detail["foreground"], "claude", "{row}");
    // Bypass and no hooks are warnings, not failures.
    assert_eq!(row["status"], "warn", "{row}");
    assert_eq!(row["found"], true);
    // A session that lost its CLI fails the row, with the attach hint.
    assert!(fx.tmux.raw(&["kill-session", "-t", "=SVWL-claude"]).status.success());
    assert!(
        fx.tmux
            .raw(&["new-session", "-d", "-s", "SVWL-claude", "-c", "/tmp", "--", "cat"])
            .status
            .success()
    );
    let gone = fx.session_row();
    assert_eq!(gone["detail"]["state"], "cli_not_running", "{gone}");
    assert_eq!(gone["status"], "fail");
    assert!(
        gone["hint"]
            .as_str()
            .unwrap_or_default()
            .contains("tmux attach -t SVWL-claude")
    );
}

/// With a report interval of 600 seconds, the session the runner creates for the first delivery is
/// reported within seconds, not at the next interval: the status Sylvode shows is current.
#[tokio::test]
async fn the_session_the_runner_creates_is_reported_without_waiting_for_the_interval() {
    if !enabled() {
        return;
    }
    let Some(program) = find_tmux() else { return };
    let fx = Fixture::new(program, Standin::Normal).await;
    fx.set_view(T1, true, true);
    let server = fx.serve();
    // The start-up report goes out first and has no session in it.
    let first = fx
        .api
        .wait_for("PUT", "/api/v1/ai/runner/report", 1, Duration::from_secs(10))
        .await;
    assert!(!first.is_empty(), "the start-up report");
    let has_session = |body: &Value| {
        body["components"]
            .as_array()
            .is_some_and(|rows| rows.iter().any(|row| row["component"] == "tmux_session"))
    };
    assert!(!has_session(&first[0].body), "{}", first[0].body);

    let delivered = Fixture::deliver(server.url("/webhook"), T1).await;
    assert_eq!(delivered["status"], "dispatched", "{delivered}");
    let started = Instant::now();
    loop {
        let reports = fx
            .api
            .wait_for("PUT", "/api/v1/ai/runner/report", 2, Duration::from_secs(1))
            .await;
        if reports.iter().any(|report| has_session(&report.body)) {
            break;
        }
        assert!(
            started.elapsed() < Duration::from_secs(30),
            "no report with the session within 30 s (the interval is 600 s): {reports:?}"
        );
    }
    // Asking is paced: the rounds are at least ten seconds apart, so a burst of events is not a storm.
    let reports = fx
        .api
        .requests()
        .into_iter()
        .filter(|r| r.path().ends_with("/runner/report"))
        .count();
    assert!(reports <= 3, "{reports} reports in a few seconds");
}

const T3: &str = "55555555-5555-4555-8555-555555555555";

fn pane_counters(state: &std::path::Path) -> Vec<String> {
    std::fs::read_dir(state.join("panes"))
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|entry| std::fs::read_to_string(entry.path()).ok())
        .map(|text| text.trim().to_string())
        .collect()
}

/// codex on the tmux channel with a stand-in codex: the session `SVWL-codex` is created with the
/// default no-sandbox launch command and the MCP server and notify hook as `-c` overrides (no
/// settings file, no token), the task is typed as one paste, only the main thread's notification
/// is sent by the hook, and the next task goes in once the first is over.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs a real tmux; run with SV_TMUX_TESTS=1 and --ignored"]
async fn a_codex_task_is_typed_with_overrides_and_only_the_main_turn_end_is_reported() {
    if !enabled() {
        return;
    }
    let Some(program) = find_tmux() else { return };
    let fx = Fixture::codex(program, Standin::Normal).await;
    fx.set_view(T1, true, true);
    fx.set_view(T2, true, false);
    let server = fx.serve();
    let first = Fixture::deliver(server.url("/webhook"), T1).await;
    assert_eq!(first["status"], "dispatched", "{first}");
    assert_eq!(first["session"], "SVWL-codex", "{first}");
    let typed = fx.wait_typed(1, 40).await;
    assert_eq!(typed.len(), 1, "{typed:?}");
    assert!(typed[0].contains(&format!("id={T1}")), "{typed:?}");
    let argv = std::fs::read_to_string(&fx.log)
        .expect("log")
        .lines()
        .find(|l| l.starts_with("argv: "))
        .map(str::to_string)
        .expect("argv");
    for wanted in [
        "--no-daemon",
        "--no-alt-screen",
        "--dangerously-bypass-approvals-and-sandbox",
        "-c mcp_servers.sylvode.command=",
        "-c mcp_servers.sylvode.args=",
        "-c notify=",
    ] {
        assert!(argv.contains(wanted), "{wanted}: {argv}");
    }
    assert!(
        !argv.contains("--settings") && !argv.contains("opr_") && !argv.contains(TOKEN),
        "{argv}"
    );
    assert!(
        !fx.tmux
            .state()
            .join("sessions/SVWL-codex/claude-settings.json")
            .exists()
    );
    // The stand-in sends three notifications at the end of every turn (a sub-thread without
    // `client`, another type, the main one); the hook sends only the one with `type =
    // agent-turn-complete` and a `client` key, so the pane's counter reads the number of turns
    // (the runner may type a reminder, which is a turn of its own).
    let turns = |log: &PathBuf| {
        std::fs::read_to_string(log)
            .unwrap_or_default()
            .lines()
            .filter(|l| l.starts_with("TEXT: "))
            .count()
    };
    let deadline = Instant::now() + Duration::from_secs(30);
    while pane_counters(&fx.tmux.state()).is_empty() && Instant::now() < deadline {
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
    tokio::time::sleep(Duration::from_secs(3)).await;
    let counters = pane_counters(&fx.tmux.state());
    assert_eq!(counters.len(), 1, "{counters:?}");
    let sent: usize = counters[0].parse().expect("a counter");
    assert!(
        sent >= 1 && sent <= turns(&fx.log),
        "{counters:?} of {} turns",
        turns(&fx.log)
    );
    // The next task waits for the first, then goes in.
    assert_eq!(Fixture::deliver(server.url("/webhook"), T2).await["queue_position"], 1);
    tokio::time::sleep(Duration::from_secs(3)).await;
    assert_eq!(fx.typed().len(), 1, "{:?}", fx.typed());
    fx.set_view(T1, false, true);
    let typed = fx.wait_typed(2, 40).await;
    assert_eq!(typed.len(), 2, "{typed:?}");
    assert!(typed[1].contains(&format!("id={T2}")), "{typed:?}");
    // Doctor describes the codex session: the runner started it, hooks seen, bypass, MCP in args.
    let row = fx.row("session.SVWL-codex");
    assert_eq!(row["detail"]["state"], "running", "{row}");
    assert_eq!(row["detail"]["created_by"], "runner", "{row}");
    assert_eq!(row["detail"]["permissions"], "bypass", "{row}");
    assert_eq!(row["detail"]["mcp"], "ok", "{row}");
    assert_eq!(row["detail"]["foreground"], "codex", "{row}");
    drop(server);
}

/// A trust dialog on codex's first start fails the task as retryable with the attach hint of the
/// codex session; nothing is typed into it and the runner never answers it.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs a real tmux; run with SV_TMUX_TESTS=1 and --ignored"]
async fn a_codex_trust_dialog_fails_the_task_with_the_attach_hint_and_the_next_one_is_typed() {
    if !enabled() {
        return;
    }
    let Some(program) = find_tmux() else { return };
    let fx = Fixture::codex(program, Standin::Untrusted).await;
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
            .contains("tmux attach -t SVWL-codex"),
        "{fails:?}"
    );
    assert_eq!(fails[0].body["retryable"], true);
    assert!(fx.typed().is_empty(), "{:?}", fx.typed());
    // The operator attaches and answers.
    assert!(
        fx.tmux
            .raw(&["send-keys", "-t", "=SVWL-codex:", "1", "Enter"])
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

/// A claude session and a codex session of the same project run side by side, each with its own
/// CLI and tasks, and `doctor` has a row for each.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs a real tmux; run with SV_TMUX_TESTS=1 and --ignored"]
async fn a_claude_and_a_codex_session_of_one_project_run_side_by_side() {
    if !enabled() {
        return;
    }
    let Some(program) = find_tmux() else { return };
    let fx = Fixture::codex(program, Standin::Normal).await;
    for (task, cli) in [(T1, "claude"), (T3, "codex")] {
        let view = fx.view_for(cli, task, true, true);
        fx.api.script(|s| {
            s.views.insert(task.to_string(), vec![(200, view)]);
        });
    }
    let server = fx.serve();
    for task in [T1, T3] {
        assert_eq!(
            Fixture::deliver(server.url("/webhook"), task).await["status"],
            "dispatched"
        );
    }
    let typed = fx.wait_typed(2, 60).await;
    assert_eq!(typed.len(), 2, "{typed:?}");
    for name in ["=SVWL-claude", "=SVWL-codex"] {
        assert!(fx.tmux.raw(&["has-session", "-t", name]).status.success(), "{name}");
    }
    assert_eq!(fx.row("session.SVWL-claude")["detail"]["foreground"], "claude");
    assert_eq!(fx.row("session.SVWL-codex")["detail"]["foreground"], "codex");
    drop(server);
}

/// Restart recovery groups a project's tasks by session, which is `(key, cli)`: a pushed codex task
/// is supervised again in `SVWL-codex` (the claude session of the key is not looked at), not typed
/// a second time, and the next one follows when it ends.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs a real tmux; run with SV_TMUX_TESTS=1 and --ignored"]
async fn after_a_restart_a_pushed_codex_task_is_supervised_again_in_its_own_session() {
    if !enabled() {
        return;
    }
    let Some(program) = find_tmux() else { return };
    let fx = Fixture::codex(program, Standin::Normal).await;
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
    assert!(fx.tmux.raw(&["has-session", "-t", "=SVWL-codex"]).status.success());
    assert!(!fx.tmux.raw(&["has-session", "-t", "=SVWL-claude"]).status.success());
    let listed = fx.active_view(T1, Some("queued"), true);
    fx.api.script(|s| s.active = vec![listed]);
    let second = fx.serve();
    assert_eq!(Fixture::deliver(second.url("/webhook"), T2).await["queue_position"], 1);
    tokio::time::sleep(Duration::from_secs(5)).await;
    assert_eq!(fx.typed().len(), 1, "{:?}", fx.typed());
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
    assert!(!fx.tmux.raw(&["has-session", "-t", "=SVWL-claude"]).status.success());
    drop(second);
}

/// A session named in the configuration is used as it is: the task is typed into its CLI pane, the
/// session is never restarted, no derived session is made, and the record says where the name came
/// from. The state directory stays that of the slot.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs a real tmux; run with SV_TMUX_TESTS=1 and --ignored"]
async fn a_configured_session_is_used_as_it_is_and_never_restarted() {
    if !enabled() {
        return;
    }
    let Some(program) = find_tmux() else { return };
    let fx = Fixture::new(program, Standin::Normal).await.named("sv");
    fx.start_operator_session("sv");
    let before = fx.pane_pid("=sv:");
    fx.set_view(T1, true, true);
    let server = fx.serve();
    let answer = Fixture::deliver(server.url("/webhook"), T1).await;
    assert_eq!(answer["status"], "dispatched", "{answer}");
    assert_eq!(answer["session"], "sv", "{answer}");
    assert_eq!(fx.wait_typed(1, 40).await.len(), 1);
    assert_eq!(fx.pane_pid("=sv:"), before, "the CLI was restarted");
    assert!(!fx.has_session("SVWL-claude"), "a derived session was created");
    let record = std::fs::read_to_string(fx.bot_slot_dir().join("session.json")).expect("record");
    assert!(
        record.contains("\"created_by\": \"operator\"")
            && record.contains("\"tmux_name\": \"sv\"")
            && record.contains("\"name_source\": \"local\""),
        "{record}"
    );
    assert!(
        std::fs::read_dir(fx.tmux.state().join("sessions"))
            .expect("sessions")
            .filter_map(Result::ok)
            .all(|entry| entry.file_name() != "sv"),
        "the name became a directory"
    );
    drop(server);
}

/// A configured session with several windows: only the pane that runs claude gets the task; the
/// codex window and the plain program are untouched.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs a real tmux; run with SV_TMUX_TESTS=1 and --ignored"]
async fn a_configured_session_with_several_windows_gets_the_task_in_the_claude_pane_only() {
    if !enabled() {
        return;
    }
    let Some(program) = find_tmux() else { return };
    let fx = Fixture::new(program, Standin::Normal).await.named("cli");
    fx.start_operator_session("cli");
    // A window whose foreground program is called `codex` (a program that only sleeps; a copy of
    // a system binary is not usable on every platform), and a `cat`.
    let source = fx.bin.file("sleeper.rs");
    std::fs::write(
        &source,
        "fn main() { std::thread::sleep(std::time::Duration::from_secs(600)); }",
    )
    .expect("source");
    let rustc = std::env::var("RUSTC").unwrap_or_else(|_| "rustc".to_string());
    let built = Command::new(rustc)
        .args(["--edition", "2021", "-o"])
        .arg(fx.bin.file("codex"))
        .arg(&source)
        .status()
        .expect("rustc runs");
    assert!(built.success(), "the sleeper did not compile");
    let project = fx.project.to_string_lossy().to_string();
    assert!(
        fx.tmux
            .raw(&[
                "new-window",
                "-d",
                "-t",
                "=cli:",
                "-c",
                &project,
                "--",
                &fx.bin.file("codex").to_string_lossy(),
            ])
            .status
            .success()
    );
    assert!(
        fx.tmux
            .raw(&["new-window", "-d", "-t", "=cli:", "-c", &project, "--", "cat"])
            .status
            .success()
    );
    let panes = |fx: &Fixture| {
        String::from_utf8_lossy(
            &fx.tmux
                .raw(&["list-panes", "-s", "-t", "=cli", "-F", "#{window_index} #{pane_pid}"])
                .stdout,
        )
        .to_string()
    };
    let before = panes(&fx);
    assert_eq!(before.lines().count(), 3, "{before}");
    fx.set_view(T1, true, true);
    let server = fx.serve();
    let answer = Fixture::deliver(server.url("/webhook"), T1).await;
    assert_eq!(answer["status"], "dispatched", "{answer}");
    assert_eq!(answer["session"], "cli", "{answer}");
    assert_eq!(fx.wait_typed(1, 40).await.len(), 1);
    assert_eq!(panes(&fx), before, "a window or process changed");
    for window in ["1", "2"] {
        let pane = String::from_utf8_lossy(
            &fx.tmux
                .raw(&["capture-pane", "-p", "-t", &format!("=cli:{window}")])
                .stdout,
        )
        .to_string();
        assert!(!pane.contains("Sylvode"), "window {window} got the task: {pane}");
    }
    assert!(
        fx.api
            .requests_to("POST", "/api/v1/ai/tasks/")
            .iter()
            .all(|r| !r.path().ends_with("/fail"))
    );
    drop(server);
}

/// A configured session that does not exist is never created: the task fails as retryable and
/// says how to start it.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs a real tmux; run with SV_TMUX_TESTS=1 and --ignored"]
async fn an_absent_configured_session_fails_retryable_and_is_not_created() {
    if !enabled() {
        return;
    }
    let Some(program) = find_tmux() else { return };
    let fx = Fixture::new(program, Standin::Normal).await.named("sv");
    fx.set_view(T1, true, false);
    let server = fx.serve();
    let answer = Fixture::deliver(server.url("/webhook"), T1).await;
    assert_eq!(answer["status"], "dispatched", "{answer}");
    let fails = fx.api.wait_for("POST", "/fail", 1, Duration::from_secs(40)).await;
    assert_eq!(fails.len(), 1, "{fails:?}");
    assert_eq!(fails[0].body["retryable"], true);
    let reason = fails[0].body["reason"].as_str().expect("reason");
    assert!(
        reason.contains("tmux session sv does not exist")
            && reason.contains("tmux new-session -s sv")
            && reason.contains("does not create sessions with a configured name"),
        "{reason}"
    );
    assert!(!fx.has_session("sv"), "the configured session was created");
    assert!(!fx.has_session("SVWL-claude"), "a derived session was created");
    assert!(fx.typed().is_empty());
    // The row for it is a warning that names the tmux session and its source.
    let row = fx.bot_row();
    assert_eq!(row["status"], "warn", "{row}");
    assert_eq!(row["detail"]["tmux_session"], "sv", "{row}");
    assert_eq!(row["detail"]["name_source"], "local", "{row}");
    assert_eq!(row["detail"]["state"], "absent", "{row}");
    drop(server);
}

/// `doctor` for a bot's configured session: the row is `session.bot-<uuid>-<cli>` and shows the
/// bot, the tmux name and where it came from; it exists before any task ran.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs a real tmux; run with SV_TMUX_TESTS=1 and --ignored"]
async fn doctor_describes_a_configured_session() {
    if !enabled() {
        return;
    }
    let Some(program) = find_tmux() else { return };
    let fx = Fixture::new(program, Standin::Normal).await.named("sv");
    let absent = fx.bot_row();
    assert_eq!(absent["status"], "warn", "{absent}");
    assert_eq!(absent["detail"]["state"], "absent", "{absent}");
    assert!(
        absent["hint"]
            .as_str()
            .unwrap_or_default()
            .contains("tmux session sv does not exist"),
        "{absent}"
    );
    fx.start_operator_session("sv");
    let row = fx.bot_row();
    assert_eq!(row["detail"]["state"], "running", "{row}");
    assert_eq!(row["detail"]["tmux_session"], "sv", "{row}");
    assert_eq!(row["detail"]["bot"], BOT, "{row}");
    assert_eq!(row["detail"]["name_source"], "local", "{row}");
    assert_eq!(row["detail"]["foreground"], "claude", "{row}");
    // A session without the CLI fails, with the attach hint for the configured name and no offer
    // to end the session.
    assert!(fx.tmux.raw(&["kill-session", "-t", "=sv"]).status.success());
    assert!(
        fx.tmux
            .raw(&["new-session", "-d", "-s", "sv", "-c", "/tmp", "--", "cat"])
            .status
            .success()
    );
    let gone = fx.bot_row();
    assert_eq!(gone["detail"]["state"], "cli_not_running", "{gone}");
    assert_eq!(gone["status"], "fail");
    let hint = gone["hint"].as_str().unwrap_or_default();
    assert!(
        hint.contains("tmux attach -t sv") && !hint.contains("end the session"),
        "{gone}"
    );
}

/// `sessions` shows slot, tmux name and bot of a bot's configured session; `release` has no key to
/// release for it.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs a real tmux; run with SV_TMUX_TESTS=1 and --ignored"]
async fn the_operator_commands_see_a_configured_session() {
    if !enabled() {
        return;
    }
    let Some(program) = find_tmux() else { return };
    let fx = Fixture::new(program, Standin::Normal).await.named("sv");
    fx.start_operator_session("sv");
    fx.set_view(T1, true, true);
    let mut server = fx.serve();
    assert_eq!(
        Fixture::deliver(server.url("/webhook"), T1).await["status"],
        "dispatched"
    );
    assert_eq!(fx.wait_typed(1, 40).await.len(), 1);
    server.terminate();
    assert!(server.wait_exit(Duration::from_secs(20)).is_some());
    let listing = fx.operator(&["sessions"]);
    let text = String::from_utf8_lossy(&listing.stdout).to_string();
    let line = text.lines().find(|l| l.starts_with("bot-")).unwrap_or_default();
    assert!(
        listing.status.success()
            && line.split_whitespace().next() == Some(&format!("bot-{BOT}-claude"))
            && line.split_whitespace().nth(1) == Some("sv")
            && line.split_whitespace().nth(2) == Some(BOT)
            && line.contains(PROJECT)
            && line.contains("operator")
            && (line.contains("idle") || line.contains("busy")),
        "{text}"
    );
    // Only the configured session exists. A bot's session binds no key, so there is nothing
    // to release and the live session does not matter to it.
    assert!(!fx.has_session("SVWL-claude"));
    let refused = fx.operator(&["release", "SVWL"]);
    assert!(
        !refused.status.success() && String::from_utf8_lossy(&refused.stderr).contains("not reserved"),
        "{}",
        String::from_utf8_lossy(&refused.stderr)
    );
    assert!(fx.has_session("sv"));
}

/// Tasks of two projects of one bot go into the bot's one session, in arrival order: the second
/// waits until the first is complete, its text names its project, no directory is looked at (the
/// second project's is outside the allowed roots) and the CLI is never restarted.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs a real tmux; run with SV_TMUX_TESTS=1 and --ignored"]
async fn tasks_of_two_projects_for_one_bot_go_into_its_one_session_in_order() {
    if !enabled() {
        return;
    }
    let Some(program) = find_tmux() else { return };
    let fx = Fixture::new(program, Standin::Normal).await.named("sv");
    fx.start_operator_session("sv");
    let before = fx.pane_pid("=sv:");
    fx.set_view(T1, true, true);
    fx.set_view_b(T2, "tmux", true, false);
    let server = fx.serve();
    let first = Fixture::deliver(server.url("/webhook"), T1).await;
    assert_eq!(first["status"], "dispatched", "{first}");
    assert_eq!(
        (first["session"].as_str(), first["queue_position"].as_u64()),
        (Some("sv"), Some(1)),
        "{first}"
    );
    // The stand-in records the line feeds of a message as `|`.
    let typed = fx.wait_typed(1, 40).await;
    assert_eq!(typed.len(), 1, "{typed:?}");
    assert!(
        typed[0].contains(&format!("id={T1}")) && typed[0].contains("|Project: SVWL|"),
        "{typed:?}"
    );
    // B arrives behind the open task A, in the same session: the first place in the queue.
    let second = post_signed(&server.url("/webhook"), &Fixture::delivery_b(T2)).await.1;
    assert_eq!(second["status"], "dispatched", "{second}");
    assert_eq!(
        (second["session"].as_str(), second["queue_position"].as_u64()),
        (Some("sv"), Some(1)),
        "{second}"
    );
    // B stays queued while A is open, however long we wait.
    tokio::time::sleep(Duration::from_secs(4)).await;
    assert_eq!(fx.typed().len(), 1, "{:?}", fx.typed());
    // A is complete: B goes into the same session.
    fx.set_view(T1, false, true);
    let typed = fx.wait_typed(2, 40).await;
    assert_eq!(typed.len(), 2, "{typed:?}");
    assert!(
        typed[1].contains(&format!("id={T2}")) && typed[1].contains("|Project: RFGUI|"),
        "{typed:?}"
    );
    assert!(
        !typed[1].contains("not-allowed") && !typed[1].contains("SVWL"),
        "no path and no other project in the text: {typed:?}"
    );
    assert_eq!(fx.pane_pid("=sv:"), before, "the CLI was restarted");
    for derived in ["SVWL-claude", "RFGUI-claude"] {
        assert!(!fx.has_session(derived), "{derived} was created");
    }
    assert!(
        fx.api
            .requests_to("POST", "/api/v1/ai/tasks/")
            .iter()
            .all(|r| !r.path().ends_with("/fail"))
    );
    // One slot, whichever project the task came from; the record keeps the last project.
    let slots: Vec<String> = std::fs::read_dir(fx.tmux.state().join("sessions"))
        .expect("sessions")
        .filter_map(Result::ok)
        .filter_map(|entry| entry.file_name().into_string().ok())
        .collect();
    assert!(slots.contains(&format!("bot-{BOT}-claude")), "{slots:?}");
    assert!(
        !slots
            .iter()
            .any(|name| name.starts_with("SVWL") || name.starts_with("RFGUI")),
        "{slots:?}"
    );
    let record = std::fs::read_to_string(fx.bot_slot_dir().join("session.json")).expect("record");
    assert!(record.contains(&format!("\"project_id\": \"{PROJECT_B}\"")), "{record}");
    drop(server);
}

/// A headless delivery for a bot that has a session is not typed into it: the entry serves the tmux
/// channel only, so the headless rules apply (the roots refuse a directory outside them).
#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs a real tmux; run with SV_TMUX_TESTS=1 and --ignored"]
async fn a_headless_delivery_for_a_bot_with_a_session_is_not_typed_into_it() {
    if !enabled() {
        return;
    }
    let Some(program) = find_tmux() else { return };
    let fx = Fixture::new(program, Standin::Normal).await.named("sv");
    fx.start_operator_session("sv");
    fx.set_view_b(T1, "headless", true, true);
    let server = fx.serve();
    let answer = post_signed(&server.url("/webhook"), &Fixture::delivery_b(T1)).await.1;
    assert_eq!(answer["status"], "refused", "{answer}");
    assert_eq!(answer["reason_code"], "path_outside_allowlist", "{answer}");
    tokio::time::sleep(Duration::from_secs(1)).await;
    assert!(
        fx.typed().is_empty(),
        "nothing is typed into the session: {:?}",
        fx.typed()
    );
    drop(server);
}

/// A derived session that another bot's tasks were typed into is never used for this bot: the
/// delivery is refused for good and nothing is typed.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs a real tmux; run with SV_TMUX_TESTS=1 and --ignored"]
async fn a_derived_session_of_another_bot_is_refused() {
    if !enabled() {
        return;
    }
    let Some(program) = find_tmux() else { return };
    let fx = Fixture::new(program, Standin::Normal).await;
    fx.start_operator_session("SVWL-claude");
    let process = fx.pane_pid("=SVWL-claude:");
    let pane = fx.pane_id("=SVWL-claude:");
    // What the runner recorded when another bot's task was typed into that session.
    let dir = fx.tmux.state().join("sessions/SVWL-claude");
    std::os::unix::fs::DirBuilderExt::mode(std::fs::DirBuilder::new().recursive(true), 0o700)
        .create(&dir)
        .expect("slot dir");
    let record = json!({
        "name": "SVWL-claude", "project_id": PROJECT, "bot_id": OTHER_BOT, "cli": "claude",
        "pane_id": pane, "pane_pid": process.parse::<u32>().expect("pid"),
        "dir": fx.project.to_string_lossy(), "created_by": "operator", "pushed": null,
        "tmux_name": "SVWL-claude", "name_source": "derived",
    });
    let file = dir.join("session.json");
    std::fs::write(&file, record.to_string()).expect("record");
    std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o600)).expect("chmod");
    fx.set_view(T1, true, false);
    let server = fx.serve();
    let answer = Fixture::deliver(server.url("/webhook"), T1).await;
    assert_eq!(answer["status"], "refused", "{answer}");
    assert_eq!(answer["reason_code"], "session_bot_conflict", "{answer}");
    assert!(
        answer["reason"]
            .as_str()
            .unwrap_or_default()
            .contains("tmux session SVWL-claude on this runner serves another bot"),
        "{answer}"
    );
    tokio::time::sleep(Duration::from_secs(2)).await;
    assert!(fx.typed().is_empty(), "{:?}", fx.typed());
    drop(server);
}
