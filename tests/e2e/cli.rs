//! `cli` agents through the real executable: process-tree termination on timeout and shutdown,
//! and the terminal callback each of them sends.

use crate::support::{
    CANONICAL, SECRET, Server, StubApi, TempDir, bot_task, path_with, post_signed, read_pids, wait_all_dead,
    wait_lines, write_script,
};
use std::time::{Duration, Instant};

/// A fake `codex` that records its own pid and the pids of two grandchildren (one in a
/// subshell), then waits for them.
fn forking_codex(dir: &TempDir) -> std::path::PathBuf {
    let pids = dir.file("pids");
    write_script(
        dir.path(),
        "codex",
        &format!(
            "echo $$ >> '{p}'\nsleep 30 & echo $! >> '{p}'\n(sleep 30; true) & echo $! >> '{p}'\nwait",
            p = pids.display()
        ),
    );
    pids
}

fn cli_config(api: &StubApi, timeout_secs: u64) -> String {
    format!(
        "[server]\nlisten = \"127.0.0.1:0\"\n\n[security]\nwebhook_secrets = [\"{SECRET}\"]\n\n\
         [features]\ncli_enabled = true\ncallback_enabled = true\n\n\
         [[agents]]\nid = \"fixer\"\nname = \"Fixer\"\nagent_type = \"cli\"\n\n\
         [agents.cli]\nexecutor = \"codex\"\ntimeout_secs = {timeout_secs}\ncallback = \"api\"\n\
         callback_url = \"{}\"\ncallback_token = \"cb-token\"\n\
         update_state_on_start = \"in_progress\"\nupdate_state_on_fail = \"todo\"\n",
        api.url("/callback")
    )
}

/// After `timeout_secs` the executor, and every process it started, is gone before the timeout
/// callback reports the issue back to `update_state_on_fail`.
#[tokio::test(flavor = "multi_thread")]
async fn a_timeout_terminates_the_whole_executor_tree_before_reporting() {
    let bin = TempDir::new("bin");
    let pids = forking_codex(&bin);
    let mut api = StubApi::start().await;
    let server =
        Server::start(CANONICAL, &cli_config(&api, 1), &[("PATH", &path_with(bin.path()))]).expect("server starts");

    let (status, answer) = post_signed(&server.url("/webhook"), &bot_task("Fixer", "Slow fix").to_string()).await;
    assert_eq!(status, 200, "{answer}");
    let start = api.next(Duration::from_secs(10)).await.expect("start callback");
    assert_eq!(start.body["status"], "started");
    assert_eq!(start.body["state"], "in_progress");

    let begun = Instant::now();
    let last = api.next(Duration::from_secs(20)).await.expect("timeout callback");
    assert_eq!(last.body["status"], "timeout", "{:?}", last.body);
    assert_eq!(last.body["state"], "todo");
    assert!(begun.elapsed() < Duration::from_secs(10), "{:?}", begun.elapsed());

    let started = wait_lines(&pids, 3, Duration::from_secs(1));
    assert_eq!(started.len(), 3, "leader and two grandchildren recorded: {started:?}");
    let survivors = wait_all_dead(&read_pids(&pids), Duration::from_secs(1));
    assert!(
        survivors.is_empty(),
        "still running after the timeout callback: {survivors:?}"
    );
}

/// SIGTERM while an executor runs: the process tree is terminated, the terminal callback
/// (`cancelled`, issue back to `update_state_on_fail`) is sent, and the server exits 0.
#[tokio::test(flavor = "multi_thread")]
async fn sigterm_terminates_a_running_executor_and_sends_its_terminal_callback() {
    let bin = TempDir::new("bin");
    let pids = forking_codex(&bin);
    let mut api = StubApi::start().await;
    let mut server =
        Server::start(CANONICAL, &cli_config(&api, 900), &[("PATH", &path_with(bin.path()))]).expect("server starts");

    let (status, answer) = post_signed(&server.url("/webhook"), &bot_task("Fixer", "Long fix").to_string()).await;
    assert_eq!(status, 200, "{answer}");
    assert_eq!(
        api.next(Duration::from_secs(10)).await.expect("start callback").body["status"],
        "started"
    );
    assert_eq!(
        wait_lines(&pids, 3, Duration::from_secs(10)).len(),
        3,
        "executor tree started"
    );

    server.terminate();
    let last = api
        .next(Duration::from_secs(20))
        .await
        .expect("terminal callback on shutdown");
    assert_eq!(last.body["status"], "cancelled", "{:?}", last.body);
    assert_eq!(last.body["state"], "todo");
    assert_eq!(
        last.headers.get("authorization").and_then(|v| v.to_str().ok()),
        Some("Bearer cb-token")
    );

    let (code, _) = server
        .wait_exit(Duration::from_secs(20))
        .expect("server exits after SIGTERM");
    assert_eq!(code, Some(0));
    let survivors = wait_all_dead(&read_pids(&pids), Duration::from_secs(1));
    assert!(survivors.is_empty(), "still running after shutdown: {survivors:?}");
    assert!(
        server
            .stdout()
            .iter()
            .any(|line| line.contains("background run(s) to stop and report"))
    );
}

/// A fake `codex` that appends `start` and `end` around a one-second run.
fn sequenced_codex(dir: &TempDir) -> std::path::PathBuf {
    let log = dir.file("runs");
    write_script(
        dir.path(),
        "codex",
        &format!("echo start >> '{l}'\nsleep 1\necho end >> '{l}'", l = log.display()),
    );
    log
}

/// `[runtime].cli_max_concurrency` limits webhook-triggered runs too: with 1, two deliveries run
/// one after the other instead of at the same time.
#[tokio::test(flavor = "multi_thread")]
async fn webhook_triggered_runs_respect_cli_max_concurrency() {
    let bin = TempDir::new("bin");
    let log = sequenced_codex(&bin);
    let api = StubApi::start().await;
    let config = cli_config(&api, 30).replace("[features]", "[runtime]\ncli_max_concurrency = 1\n\n[features]");
    let server = Server::start(CANONICAL, &config, &[("PATH", &path_with(bin.path()))]).expect("server starts");

    for title in ["first", "second"] {
        let (status, answer) = post_signed(&server.url("/webhook"), &bot_task("Fixer", title).to_string()).await;
        assert_eq!(status, 200, "{answer}");
    }
    let runs = wait_lines(&log, 4, Duration::from_secs(15));
    assert_eq!(runs, ["start", "end", "start", "end"], "runs overlapped");
}

/// A worker AI task, as the Sylvode worker sends it, for bot `bot-1` and issue `REAL-ISSUE`,
/// whose free-form `payload` (written by whoever created the task) names another issue and
/// another agent type.
fn worker_task_with_hostile_payload(agent_type: Option<&str>) -> serde_json::Value {
    serde_json::json!({
        "task_id": uuid::Uuid::new_v4().to_string(),
        "project_id": "project-1",
        "ai_participant_id": "bot-1",
        "ai_participant_name": "Fixer",
        "ai_participant_agent_type": agent_type,
        "task_type": "issue_assigned",
        "reference_type": "work_item",
        "reference_id": "REAL-ISSUE",
        "payload": {"issue_id": "OTHER-PROJECT-ISSUE", "agent_type": "cli", "bot_agent_type": "cli"},
        "attempts": 1,
        "max_attempts": 3,
        "trigger_kind": "assigned"
    })
}

/// The callback, sent with the bot's token, targets the issue the task was issued for
/// (`reference_id`), never an issue named inside the task's free-form `payload`.
#[tokio::test(flavor = "multi_thread")]
async fn a_worker_task_payload_cannot_redirect_the_callback_to_another_issue() {
    let mut api = StubApi::start().await;
    let config = cli_config(&api, 30).replace("id = \"fixer\"", "id = \"bot-1\"");
    let server = Server::start(CANONICAL, &config, &[]).expect("server starts");

    let body = worker_task_with_hostile_payload(Some("cli")).to_string();
    let (status, answer) = post_signed(&server.url("/webhook"), &body).await;
    assert_eq!(status, 200, "{answer}");
    assert_eq!(answer["status"], "dispatched", "{answer}");
    let start = api.next(Duration::from_secs(10)).await.expect("start callback");
    assert_eq!(start.body["issue_id"], "REAL-ISSUE", "{:?}", start.body);
    let last = api.next(Duration::from_secs(10)).await.expect("final callback");
    assert_eq!(last.body["issue_id"], "REAL-ISSUE", "{:?}", last.body);
}

/// The agent type of a worker task is the bot's (`ai_participant_agent_type`); the task's
/// free-form `payload` cannot pick a `cli` agent for a bot that has none.
#[tokio::test(flavor = "multi_thread")]
async fn a_worker_task_payload_cannot_choose_the_agent_type() {
    let api = StubApi::start().await;
    let server = Server::start(CANONICAL, &cli_config(&api, 30), &[]).expect("server starts");

    let body = worker_task_with_hostile_payload(None).to_string();
    let (status, answer) = post_signed(&server.url("/webhook"), &body).await;
    assert_eq!(status, 200, "{answer}");
    assert_eq!(answer["status"], "no_agent", "{answer}");
    assert_eq!(answer["route"]["agent_type"], serde_json::Value::Null, "{answer}");
}

/// Safe mode, through either environment variable, really turns the `cli` path off in the
/// running service; without it the same delivery starts the executor.
#[tokio::test(flavor = "multi_thread")]
async fn safe_mode_variables_turn_the_cli_path_off() {
    for (env, disabled, notice) in [
        (Some("SYLVODE_WEBHOOK_SAFE_MODE"), true, false),
        (Some("OPENPR_WEBHOOK_SAFE_MODE"), true, true),
        (None, false, false),
    ] {
        let bin = TempDir::new("bin");
        let marker = bin.file("ran");
        write_script(bin.path(), "codex", &format!("echo ran > '{}'", marker.display()));
        let api = StubApi::start().await;
        let path = path_with(bin.path());
        let mut vars = vec![("PATH", path.as_str())];
        if let Some(name) = env {
            vars.push((name, "1"));
        }
        let mut server = Server::start(CANONICAL, &cli_config(&api, 30), &vars).expect("server starts");

        let (status, answer) = post_signed(&server.url("/webhook"), &bot_task("Fixer", "safe").to_string()).await;
        assert_eq!(status, 200, "{answer}");
        let result = answer["result"].as_str().unwrap_or_default().to_owned();
        if disabled {
            assert_eq!(result, "cli disabled by feature flag or safe mode", "{env:?}");
            assert!(
                wait_lines(&marker, 1, Duration::from_millis(500)).is_empty(),
                "{env:?}: executor ran"
            );
        } else {
            assert!(result.starts_with("cli agent spawned"), "{result}");
            assert_eq!(wait_lines(&marker, 1, Duration::from_secs(10)), ["ran"]);
        }
        server.terminate();
        let (_, stderr) = server.wait_exit(Duration::from_secs(20)).expect("server exits");
        assert_eq!(
            stderr
                .iter()
                .any(|line| line.contains("`OPENPR_WEBHOOK_SAFE_MODE` is deprecated")),
            notice,
            "{env:?}: {stderr:?}"
        );
    }
}
