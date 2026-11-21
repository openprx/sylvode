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
        let log = bin.file("standin.log");
        let socket = tmux.state().join("hook.sock");
        let hook = |event: &str| {
            format!(
                "'{RUNNER}' hook --cli claude --session SVWL --event {event} --socket '{}' </dev/null",
                socket.display()
            )
        };
        let body = if matches!(standin, Standin::Untrusted) {
            format!(
                "echo \"argv: $*\" >> '{log}'\nprintf 'Is this a project you created or one you trust?\\n  Yes, I trust this folder\\n'\n/bin/sleep 600",
                log = log.display()
            )
        } else {
            {
                let turn = if matches!(standin, Standin::Long) {
                    "/bin/sleep 600"
                } else {
                    "/bin/sleep 1"
                };
                format!(
                    "echo \"argv: $*\" >> '{log}'\n/usr/bin/env | /usr/bin/sort >> '{log}.env'\nprintf '\\033[?2004h'\n{start}\n\
                     end=$(printf '\\033[201~')\n\
                     while true; do\n  printf '> '\n  IFS= read -r first || exit 0\n  text=\"$first\"\n\
                     case \"$first\" in *\"$end\"*) ;; *) while IFS= read -r more; do text=\"$text|$more\"; case \"$more\" in *\"$end\"*) break;; esac; done;; esac\n\
                     {submit}\n  printf 'TEXT: %s\\n' \"$text\" >> '{log}'\n  {turn}\n  {stop}\ndone",
                    log = log.display(),
                    start = hook("SessionStart"),
                    submit = hook("UserPromptSubmit"),
                    stop = hook("Stop"),
                )
            }
        };
        write_script(bin.path(), "claude", &body);
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
             [runner.tmux]\nbinary = \"{}\"\nsocket_name = \"{}\"\npush_settle_ms = 100\nuser_idle_secs = 0\nready_timeout_secs = 20\n\
             [runner.cli.claude]\nbinary = \"{}\"\nprocess_names = [\"sh\", \"bash\", \"dash\"]\n\
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
    assert_eq!(typed.len(), 1, "{typed:?}");
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
