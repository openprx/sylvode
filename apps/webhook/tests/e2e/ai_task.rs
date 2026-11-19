//! `[ai_task_callback]` through the real executable: a worker AI task ends in Sylvode with
//! `complete` or `fail`, also when no agent matches it.

use crate::support::{CANONICAL, SECRET, Server, StubApi, TempDir, path_with, post_signed, write_script};
use std::time::Duration;

const BOT_TOKEN: &str = "e2e-bot-token";

/// A `cli` agent for bot `bot-1` whose executor is the fake `codex` on `PATH`. The issue
/// callback is off, so every request the stub API receives is an AI task callback.
fn config(api: &StubApi) -> String {
    format!(
        "[server]\nlisten = \"127.0.0.1:0\"\n\n[security]\nwebhook_secrets = [\"{SECRET}\"]\n\n\
         [features]\ncli_enabled = true\n\n\
         [ai_task_callback]\nenabled = true\napi_url = \"{}\"\ntoken = \"{BOT_TOKEN}\"\n\
         retry_delay_ms = 10\n\n\
         [[agents]]\nid = \"bot-1\"\nname = \"Fixer\"\nagent_type = \"cli\"\n\n\
         [agents.cli]\nexecutor = \"codex\"\ntimeout_secs = 30\n",
        api.url("")
    )
}

/// [`config`], with the agent routed to bots whose agent type is `cli` only, so a bot of another
/// type does not fall back to it.
fn routed_config(api: &StubApi) -> String {
    config(api).replace(
        "agent_type = \"cli\"\n\n",
        "agent_type = \"cli\"\n\n[agents.route]\nbot_agent_types = [\"cli\"]\n\n",
    )
}

/// [`config`], with the agent routed by `bot_ids = ["bot-1"]` alone: no other list is set, so
/// the bot id is the only constraint and the agent type of the task must not widen it.
fn bot_id_routed_config(api: &StubApi) -> String {
    config(api).replace(
        "agent_type = \"cli\"\n\n",
        "agent_type = \"cli\"\n\n[agents.route]\nbot_ids = [\"bot-1\"]\n\n",
    )
}

/// A worker AI task for `bot_id`, as the Sylvode worker delivers it.
fn worker_task(task_id: &str, bot_id: &str) -> serde_json::Value {
    serde_json::json!({
        "task_id": task_id,
        "project_id": "project-1",
        "ai_participant_id": bot_id,
        "ai_participant_name": "Fixer",
        "ai_participant_agent_type": "cli",
        "task_type": "issue_assigned",
        "reference_type": "work_item",
        "reference_id": "ISSUE-1",
        "payload": {},
        "attempts": 1,
        "max_attempts": 3,
        "trigger_kind": "assigned"
    })
}

fn bearer(recorded: &crate::support::Recorded) -> Option<&str> {
    recorded.headers.get("authorization").and_then(|v| v.to_str().ok())
}

/// A run that exits 0 completes the task with the bot token, its stdout in the result.
#[tokio::test(flavor = "multi_thread")]
async fn a_successful_cli_run_completes_the_worker_task() {
    let bin = TempDir::new("bin");
    write_script(bin.path(), "codex", "echo fixed it");
    let mut api = StubApi::start().await;
    let server = Server::start(CANONICAL, &config(&api), &[("PATH", &path_with(bin.path()))]).expect("server starts");

    let task_id = uuid::Uuid::new_v4().to_string();
    let (status, answer) = post_signed(&server.url("/webhook"), &worker_task(&task_id, "bot-1").to_string()).await;
    assert_eq!(status, 200, "{answer}");
    assert_eq!(answer["status"], "dispatched", "{answer}");

    let call = api.next(Duration::from_secs(20)).await.expect("complete call");
    assert_eq!(call.path, format!("/api/v1/ai/callbacks/task/{task_id}/complete"));
    assert_eq!(bearer(&call), Some(format!("Bearer {BOT_TOKEN}").as_str()));
    assert_eq!(call.body["source"], "sylvode-webhook");
    assert_eq!(call.body["agent_id"], "bot-1");
    assert_eq!(call.body["status"], "success", "{:?}", call.body);
    assert_eq!(call.body["exit_code"], 0);
    assert!(
        call.body["output"].as_str().unwrap_or_default().contains("fixed it"),
        "{:?}",
        call.body
    );
    assert!(api.next(Duration::from_millis(300)).await.is_none(), "one call only");
}

/// A run that exits non-zero fails the task, with an error message and the run's stderr.
#[tokio::test(flavor = "multi_thread")]
async fn a_failing_cli_run_fails_the_worker_task() {
    let bin = TempDir::new("bin");
    write_script(bin.path(), "codex", "echo broken >&2\nexit 3");
    let mut api = StubApi::start().await;
    let server = Server::start(CANONICAL, &config(&api), &[("PATH", &path_with(bin.path()))]).expect("server starts");

    let task_id = uuid::Uuid::new_v4().to_string();
    let (status, answer) = post_signed(&server.url("/webhook"), &worker_task(&task_id, "bot-1").to_string()).await;
    assert_eq!(status, 200, "{answer}");

    let call = api.next(Duration::from_secs(20)).await.expect("fail call");
    assert_eq!(call.path, format!("/api/v1/ai/callbacks/task/{task_id}/fail"));
    assert_eq!(bearer(&call), Some(format!("Bearer {BOT_TOKEN}").as_str()));
    assert_eq!(call.body["error_message"], "cli execution failed", "{:?}", call.body);
    assert_eq!(call.body["payload"]["status"], "failed");
    assert_eq!(call.body["payload"]["exit_code"], 3);
    assert!(
        call.body["payload"]["output"]
            .as_str()
            .unwrap_or_default()
            .contains("broken"),
        "{:?}",
        call.body
    );
    assert!(api.next(Duration::from_millis(300)).await.is_none(), "one call only");
}

/// A worker AI task that no agent matches is failed at once with a message naming the bot,
/// instead of staying `processing` in Sylvode.
#[tokio::test(flavor = "multi_thread")]
async fn a_worker_task_no_agent_matches_is_failed() {
    let mut api = StubApi::start().await;
    let server = Server::start(CANONICAL, &routed_config(&api), &[]).expect("server starts");

    let task_id = uuid::Uuid::new_v4().to_string();
    let mut task = worker_task(&task_id, "bot-unknown");
    task["ai_participant_agent_type"] = serde_json::Value::String("openclaw".to_owned());
    let (status, answer) = post_signed(&server.url("/webhook"), &task.to_string()).await;
    assert_eq!(status, 200, "{answer}");
    assert_eq!(answer["status"], "no_agent", "{answer}");

    let call = api.next(Duration::from_secs(10)).await.expect("fail call");
    assert_eq!(call.path, format!("/api/v1/ai/callbacks/task/{task_id}/fail"));
    assert_eq!(bearer(&call), Some(format!("Bearer {BOT_TOKEN}").as_str()));
    let message = call.body["error_message"].as_str().unwrap_or_default();
    assert!(
        message.contains("no agent") && message.contains("bot-unknown"),
        "{message}"
    );
    assert_eq!(call.body["payload"]["status"], "error");
    assert_eq!(call.body["payload"]["agent_id"], serde_json::Value::Null);
}

/// A delivery that is not a worker AI task, with no matching agent either, calls nothing.
#[tokio::test(flavor = "multi_thread")]
async fn a_direct_bot_task_no_agent_matches_calls_nothing() {
    let mut api = StubApi::start().await;
    let server = Server::start(CANONICAL, &routed_config(&api), &[]).expect("server starts");

    let mut body = crate::support::bot_task("Nobody", "unmatched");
    body["bot_context"]["bot_agent_type"] = serde_json::Value::String("openclaw".to_owned());
    let body = body.to_string();
    let (status, answer) = post_signed(&server.url("/webhook"), &body).await;
    assert_eq!(status, 200, "{answer}");
    assert_eq!(answer["status"], "no_agent", "{answer}");
    assert!(api.next(Duration::from_millis(500)).await.is_none());
}

/// A route that sets only `bot_ids` is a constraint: a task for another bot of the same agent
/// type is not dispatched (it used to match any bot) and is failed as unmatched, while the
/// listed bot still runs.
#[tokio::test(flavor = "multi_thread")]
async fn a_bot_ids_only_route_does_not_catch_another_bot() {
    let bin = TempDir::new("bin");
    write_script(bin.path(), "codex", "echo fixed it");
    let mut api = StubApi::start().await;
    let server = Server::start(
        CANONICAL,
        &bot_id_routed_config(&api),
        &[("PATH", &path_with(bin.path()))],
    )
    .expect("server starts");

    let other_id = uuid::Uuid::new_v4().to_string();
    let other = worker_task(&other_id, "bot-unknown");
    let (status, answer) = post_signed(&server.url("/webhook"), &other.to_string()).await;
    assert_eq!(status, 200, "{answer}");
    assert_eq!(answer["status"], "no_agent", "{answer}");
    let call = api.next(Duration::from_secs(10)).await.expect("fail call");
    assert_eq!(call.path, format!("/api/v1/ai/callbacks/task/{other_id}/fail"));
    assert!(api.next(Duration::from_millis(300)).await.is_none(), "one call only");

    let listed_id = uuid::Uuid::new_v4().to_string();
    let listed = worker_task(&listed_id, "bot-1");
    let (status, answer) = post_signed(&server.url("/webhook"), &listed.to_string()).await;
    assert_eq!(status, 200, "{answer}");
    assert_eq!(answer["status"], "dispatched", "{answer}");
    let call = api.next(Duration::from_secs(10)).await.expect("complete call");
    assert_eq!(call.path, format!("/api/v1/ai/callbacks/task/{listed_id}/complete"));
}
