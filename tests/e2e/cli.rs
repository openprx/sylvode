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
