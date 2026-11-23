//! A failed hand-over of a worker AI task, through the real executable: the runner ends the task
//! with the task protocol (`POST /api/v1/ai/tasks/{id}/fail`, `source: "runner"`), also when no
//! agent matches it. A successful hand-over reports nothing; the receiving side owns the task.

use crate::support::{RUNNER, SECRET, Server, StubApi, TempDir, post_signed, write_script};
use std::path::Path;
use std::time::Duration;

const BOT_TOKEN: &str = "e2e-bot-token";

/// A `custom` (forwarding) agent for bot `bot-1` that runs `command`. Every request the stub API
/// receives is a report of the runner.
fn config(api: &StubApi, command: &Path) -> String {
    format!(
        "[server]\nlisten = \"127.0.0.1:0\"\n\n[security]\nwebhook_secrets = [\"{SECRET}\"]\n\n\
         [ai_task_callback]\nenabled = true\napi_url = \"{}\"\ntoken = \"{BOT_TOKEN}\"\n\
         retry_delay_ms = 10\n\n\
         [[agents]]\nid = \"bot-1\"\nname = \"Fixer\"\nagent_type = \"custom\"\n\n\
         [agents.custom]\ncommand = \"{}\"\nargs = [\"{{message}}\"]\n",
        api.url(""),
        command.display()
    )
}

/// [`config`], with the agent routed to bots whose agent type is `custom` only, so a bot of
/// another type does not fall back to it.
fn routed_config(api: &StubApi, command: &Path) -> String {
    config(api, command).replace(
        "agent_type = \"custom\"\n\n",
        "agent_type = \"custom\"\n\n[agents.route]\nbot_agent_types = [\"custom\"]\n\n",
    )
}

/// [`config`], with the agent routed by `bot_ids = ["bot-1"]` alone: no other list is set, so the
/// bot id is the only constraint and the agent type of the task must not widen it.
fn bot_id_routed_config(api: &StubApi, command: &Path) -> String {
    config(api, command).replace(
        "agent_type = \"custom\"\n\n",
        "agent_type = \"custom\"\n\n[agents.route]\nbot_ids = [\"bot-1\"]\n\n",
    )
}

/// A worker AI task for `bot_id`, as the Sylvode worker delivers it.
fn worker_task(task_id: &str, bot_id: &str) -> serde_json::Value {
    serde_json::json!({
        "task_id": task_id,
        "project_id": "project-1",
        "ai_participant_id": bot_id,
        "ai_participant_name": "Fixer",
        "ai_participant_agent_type": "custom",
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

/// A hand-over that works completes the task through the protocol, once: the task ends, so its
/// lease never expires into a redelivery that would run the forwarded work again.
#[tokio::test(flavor = "multi_thread")]
async fn a_successful_forward_completes_the_task_once() {
    let bin = TempDir::new("bin");
    let command = write_script(bin.path(), "forward", "exit 0");
    let mut api = StubApi::start().await;
    let server = Server::start(RUNNER, &config(&api, &command), &[]).expect("server starts");

    let task_id = uuid::Uuid::new_v4().to_string();
    let mut task = worker_task(&task_id, "bot-1");
    task["attempts"] = serde_json::json!(2);
    let (status, answer) = post_signed(&server.url("/webhook"), &task.to_string()).await;
    assert_eq!(status, 200, "{answer}");
    assert_eq!(answer["status"], "dispatched", "{answer}");
    let call = api.next(Duration::from_secs(20)).await.expect("complete call");
    assert_eq!(call.path, format!("/api/v1/ai/tasks/{task_id}/complete"));
    assert_eq!(call.query.as_deref(), Some("attempt=2"));
    assert_eq!(bearer(&call), Some(format!("Bearer {BOT_TOKEN}").as_str()));
    assert_eq!(call.body["outcome"], "done", "{:?}", call.body);
    assert!(call.body["summary"].as_str().unwrap_or_default().contains("bot-1"));
    // One end only: nothing is sent again, and nothing is left open for the lease to redeliver.
    assert!(api.next(Duration::from_millis(1500)).await.is_none(), "one call only");
}

/// A hand-over that fails ends the task through the protocol: attempt named, bot token, `source`
/// runner, retryable, and a reason that holds no output of the command.
#[tokio::test(flavor = "multi_thread")]
async fn a_failed_forward_fails_the_task_through_the_protocol() {
    let bin = TempDir::new("bin");
    let command = write_script(bin.path(), "forward", "echo secret-output >&2\nexit 3");
    let mut api = StubApi::start().await;
    let server = Server::start(RUNNER, &config(&api, &command), &[]).expect("server starts");

    let task_id = uuid::Uuid::new_v4().to_string();
    let mut task = worker_task(&task_id, "bot-1");
    task["attempts"] = serde_json::json!(2);
    let (status, answer) = post_signed(&server.url("/webhook"), &task.to_string()).await;
    assert_eq!(status, 200, "{answer}");

    let call = api.next(Duration::from_secs(20)).await.expect("fail call");
    assert_eq!(call.path, format!("/api/v1/ai/tasks/{task_id}/fail"));
    assert_eq!(call.query.as_deref(), Some("attempt=2"));
    assert_eq!(bearer(&call), Some(format!("Bearer {BOT_TOKEN}").as_str()));
    assert_eq!(call.body["source"], "runner", "{:?}", call.body);
    assert_eq!(call.body["retryable"], true, "{:?}", call.body);
    let reason = call.body["reason"].as_str().unwrap_or_default();
    assert!(
        reason.contains("status 3") && !reason.contains("secret-output"),
        "{reason}"
    );
    assert!(api.next(Duration::from_millis(300)).await.is_none(), "one call only");
}

/// A worker AI task that no agent matches is failed at once with a message naming the bot,
/// instead of staying `processing` in Sylvode, and is not retried.
#[tokio::test(flavor = "multi_thread")]
async fn a_worker_task_no_agent_matches_is_failed() {
    let bin = TempDir::new("bin");
    let command = write_script(bin.path(), "forward", "exit 0");
    let mut api = StubApi::start().await;
    let server = Server::start(RUNNER, &routed_config(&api, &command), &[]).expect("server starts");

    let task_id = uuid::Uuid::new_v4().to_string();
    let mut task = worker_task(&task_id, "bot-unknown");
    task["ai_participant_agent_type"] = serde_json::Value::String("openclaw".to_owned());
    let (status, answer) = post_signed(&server.url("/webhook"), &task.to_string()).await;
    assert_eq!(status, 200, "{answer}");
    assert_eq!(answer["status"], "no_agent", "{answer}");

    let call = api.next(Duration::from_secs(10)).await.expect("fail call");
    assert_eq!(call.path, format!("/api/v1/ai/tasks/{task_id}/fail"));
    assert_eq!(bearer(&call), Some(format!("Bearer {BOT_TOKEN}").as_str()));
    let reason = call.body["reason"].as_str().unwrap_or_default();
    assert!(
        reason.contains("no agent") && reason.contains("bot-unknown"),
        "{reason}"
    );
    assert_eq!(call.body["source"], "runner");
    assert_eq!(call.body["retryable"], false, "{:?}", call.body);
    assert_eq!(answer["ai_task_callback"], "report", "{answer}");
    assert!(
        answer["reason"].as_str().unwrap_or_default().contains("bot-unknown"),
        "the answer says why, for the worker: {answer}"
    );
}

const BOT_A: &str = "0b7e2d4c-2f0a-4b8e-8f6c-3a1d9e5c7b20";
const BOT_B: &str = "5989a9a2-c7f1-4a74-ba05-cffe01e33ad4";
const BOT_C: &str = "9d3c6f0e-1b2a-4c5d-8e7f-0a1b2c3d4e5f";

/// One receiver for two bots: bot A owns the global token and is routed to the agent, bot B has no
/// agent and its own `[[ai_task_callback.bots]]` token, and bot C has neither.
fn two_bot_config(api: &StubApi, command: &Path) -> String {
    format!(
        "[server]\nlisten = \"127.0.0.1:0\"\n\n[security]\nwebhook_secrets = [\"{SECRET}\"]\n\n\
         [ai_task_callback]\nenabled = true\napi_url = \"{}\"\ntoken = \"bot-a-token\"\nbot_id = \"{BOT_A}\"\n\
         retry_delay_ms = 10\n\n\
         [[ai_task_callback.bots]]\nbot_id = \"{BOT_B}\"\ntoken = \"bot-b-token\"\n\n\
         [[agents]]\nid = \"bot-a\"\nname = \"Fixer\"\nagent_type = \"custom\"\n\n\
         [agents.route]\nbot_ids = [\"{BOT_A}\"]\n\n\
         [agents.custom]\ncommand = \"{}\"\n",
        api.url(""),
        command.display()
    )
}

/// Bot B's task, which no agent matches, is failed with bot B's token and names its attempt; bot
/// C's task is answered `no_token` and nothing is called with bot A's token.
#[tokio::test(flavor = "multi_thread")]
async fn each_bots_unmatched_task_uses_its_own_token_or_none() {
    let bin = TempDir::new("bin");
    let command = write_script(bin.path(), "forward", "exit 0");
    let mut api = StubApi::start().await;
    let server = Server::start(RUNNER, &two_bot_config(&api, &command), &[]).expect("server starts");

    let task_b = uuid::Uuid::new_v4().to_string();
    let mut task = worker_task(&task_b, BOT_B);
    task["attempts"] = serde_json::json!(2);
    let (status, answer) = post_signed(&server.url("/webhook"), &task.to_string()).await;
    assert_eq!(status, 200, "{answer}");
    assert_eq!(answer["status"], "no_agent", "{answer}");
    assert_eq!(answer["ai_task_callback"], "report", "{answer}");
    let call = api.next(Duration::from_secs(10)).await.expect("fail call");
    assert_eq!(call.path, format!("/api/v1/ai/tasks/{task_b}/fail"));
    assert_eq!(call.query.as_deref(), Some("attempt=2"));
    assert_eq!(bearer(&call), Some("Bearer bot-b-token"));

    let task_c = uuid::Uuid::new_v4().to_string();
    let (status, answer) = post_signed(&server.url("/webhook"), &worker_task(&task_c, BOT_C).to_string()).await;
    assert_eq!(status, 200, "{answer}");
    assert_eq!(answer["status"], "no_agent", "{answer}");
    assert_eq!(answer["ai_task_callback"], "no_token", "{answer}");
    assert!(
        api.next(Duration::from_millis(500)).await.is_none(),
        "bot C's task is not reported with another bot's token"
    );
}

/// A delivery that is not a worker AI task, with no matching agent either, calls nothing.
#[tokio::test(flavor = "multi_thread")]
async fn a_direct_bot_task_no_agent_matches_calls_nothing() {
    let bin = TempDir::new("bin");
    let command = write_script(bin.path(), "forward", "exit 0");
    let mut api = StubApi::start().await;
    let server = Server::start(RUNNER, &routed_config(&api, &command), &[]).expect("server starts");

    let mut body = crate::support::bot_task("Nobody", "unmatched");
    body["bot_context"]["bot_agent_type"] = serde_json::Value::String("openclaw".to_owned());
    let body = body.to_string();
    let (status, answer) = post_signed(&server.url("/webhook"), &body).await;
    assert_eq!(status, 200, "{answer}");
    assert_eq!(answer["status"], "no_agent", "{answer}");
    assert!(api.next(Duration::from_millis(500)).await.is_none());
}

/// A route that sets only `bot_ids` is a constraint: a task for another bot of the same agent
/// type is not dispatched and is failed as unmatched, while the listed bot is forwarded and
/// reports nothing.
#[tokio::test(flavor = "multi_thread")]
async fn a_bot_ids_only_route_does_not_catch_another_bot() {
    let bin = TempDir::new("bin");
    let command = write_script(bin.path(), "forward", "exit 0");
    let mut api = StubApi::start().await;
    let server = Server::start(RUNNER, &bot_id_routed_config(&api, &command), &[]).expect("server starts");

    let other_id = uuid::Uuid::new_v4().to_string();
    let other = worker_task(&other_id, "bot-unknown");
    let (status, answer) = post_signed(&server.url("/webhook"), &other.to_string()).await;
    assert_eq!(status, 200, "{answer}");
    assert_eq!(answer["status"], "no_agent", "{answer}");
    let call = api.next(Duration::from_secs(10)).await.expect("fail call");
    assert_eq!(call.path, format!("/api/v1/ai/tasks/{other_id}/fail"));
    assert!(api.next(Duration::from_millis(300)).await.is_none(), "one call only");

    let listed_id = uuid::Uuid::new_v4().to_string();
    let listed = worker_task(&listed_id, "bot-1");
    let (status, answer) = post_signed(&server.url("/webhook"), &listed.to_string()).await;
    assert_eq!(status, 200, "{answer}");
    assert_eq!(answer["status"], "dispatched", "{answer}");
    let call = api.next(Duration::from_secs(10)).await.expect("complete call");
    assert_eq!(call.path, format!("/api/v1/ai/tasks/{listed_id}/complete"));
    assert!(api.next(Duration::from_millis(500)).await.is_none(), "one call only");
}

/// `agent_type = "cli"` is an unknown agent type: refused at start-up with the valid ones listed.
#[test]
fn a_cli_agent_is_refused_at_start_up() {
    let config = format!(
        "[server]\nlisten = \"127.0.0.1:0\"\n\n[security]\nwebhook_secrets = [\"{SECRET}\"]\n\n\
         [[agents]]\nid = \"old\"\nname = \"Old\"\nagent_type = \"cli\"\n"
    );
    let Err(refused) = Server::start(RUNNER, &config, &[]) else {
        panic!("a cli agent must not start");
    };
    let text = refused
        .stdout
        .iter()
        .chain(&refused.stderr)
        .cloned()
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        text.contains("unknown agent_type `cli`")
            && text.contains("valid values: runner, openclaw, openprx, webhook, custom"),
        "{text}"
    );
    assert_ne!(refused.code, Some(0));
}

/// The three legacy callback endpoints are gone from the runner: no request goes to them.
#[tokio::test(flavor = "multi_thread")]
async fn no_report_uses_a_legacy_callback_path() {
    let bin = TempDir::new("bin");
    let command = write_script(bin.path(), "forward", "exit 3");
    let mut api = StubApi::start().await;
    let server = Server::start(RUNNER, &config(&api, &command), &[]).expect("server starts");
    let task_id = uuid::Uuid::new_v4().to_string();
    let (status, _) = post_signed(&server.url("/webhook"), &worker_task(&task_id, "bot-1").to_string()).await;
    assert_eq!(status, 200);
    while let Some(call) = api.next(Duration::from_secs(2)).await {
        assert!(!call.path.contains("/ai/callbacks/"), "{}", call.path);
    }
}
