//! The protocol handshake and the periodic report, through the real executable against a mock of
//! the Sylvode API.

use crate::mock_sv::MockSv;
use crate::support::{RUNNER, SECRET, Server, TempDir, post_signed};
use serde_json::{Value, json};
use std::time::Duration;

pub const BOT: &str = "00000000-0000-4000-8000-000000000001";
pub const TOKEN: &str = concat!("opr", "_", "cdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcd");

/// A `runner` agent for [`BOT`], connected to `api`, reporting every second.
pub fn config(api: &str, state: &TempDir, root: &str) -> String {
    format!(
        "[server]\nlisten = \"127.0.0.1:0\"\n\n[security]\nwebhook_secrets = [\"{SECRET}\"]\n\n\
         [ai_task_callback]\nenabled = true\napi_url = \"{api}\"\n\n\
         [[ai_task_callback.bots]]\nbot_id = \"{BOT}\"\nworkspace_id = \"11111111-1111-4111-8111-111111111111\"\ntoken = \"{TOKEN}\"\nsurface = \"mcp_stdio\"\n\n\
         [runner]\nstate_dir = \"{}/s\"\nallowed_roots = [\"{root}\"]\nreport_interval_secs = 1\n\
         delivery_check_timeout_secs = 2\n\n\
         [[agents]]\nid = \"{BOT}\"\nname = \"Bot\"\nagent_type = \"runner\"\n\n\
         [agents.route]\nbot_ids = [\"{BOT}\"]\n",
        state.path().display()
    )
}

/// Every key at any depth of a JSON value.
fn keys(value: &Value, out: &mut Vec<String>) {
    match value {
        Value::Object(map) => {
            for (key, inner) in map {
                out.push(key.clone());
                keys(inner, out);
            }
        }
        Value::Array(items) => items.iter().for_each(|item| keys(item, out)),
        _ => {}
    }
}

pub async fn health(url: &str) -> Value {
    reqwest::get(url)
        .await
        .expect("health")
        .json()
        .await
        .expect("health json")
}

async fn wait_health(url: &str, state: &str) -> Value {
    for _ in 0..100 {
        let answer = health(url).await;
        if answer["protocol"]["state"] == state {
            return answer;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    panic!("/health never showed protocol state {state}: {}", health(url).await);
}

fn root() -> TempDir {
    let dir = TempDir::new("hs-root");
    std::fs::create_dir_all(dir.path()).expect("mkdir");
    dir
}

#[tokio::test(flavor = "multi_thread")]
async fn a_matching_protocol_is_reported_at_start_and_again_after_the_interval() {
    let api = MockSv::start().await;
    let (state, root) = (TempDir::new("hs-state"), root());
    let mut server = Server::start(
        RUNNER,
        &config(&api.url(), &state, &root.path().to_string_lossy()),
        &[("RUST_LOG", "trace")],
    )
    .expect("starts");
    let health = wait_health(&server.url("/health"), "ok").await;
    assert_eq!(health["status"], "ok");
    assert_eq!(health["protocol"], json!({"runner": 1, "api": 1, "state": "ok"}));

    // The protocol call carries the bot's token and its transport.
    let calls = api
        .wait_for("GET", "/api/v1/ai/protocol", 1, Duration::from_secs(5))
        .await;
    assert_eq!(
        calls[0].authorization.as_deref(),
        Some(format!("Bearer {TOKEN}").as_str())
    );
    assert_eq!(calls[0].surface.as_deref(), Some("mcp_stdio"));

    // The report goes out after the handshake and again after the interval.
    let reports = api
        .wait_for("PUT", "/api/v1/ai/runner/report", 2, Duration::from_secs(10))
        .await;
    assert!(reports.len() >= 2, "{reports:?}");
    let body = &reports[0].body;
    assert_eq!(body["generated_by"]["task_protocol"], 1);
    assert_eq!(body["generated_by"]["version"], env!("CARGO_PKG_VERSION"));
    // The API row first, then the doctor's rows (tmux, the CLIs, the MCP server, this runner) and
    // the posture of the launch settings (no session rows: no session was ever recorded).
    let names: Vec<&str> = body["components"]
        .as_array()
        .expect("rows")
        .iter()
        .filter_map(|row| row["component"].as_str())
        .collect();
    assert_eq!(
        names,
        [
            "api_protocol",
            "tmux",
            "cli.claude",
            "cli.codex",
            "cli.opencode",
            "mcp_server",
            "runner",
            "posture"
        ]
    );
    // The defaults run with permission checks off, on both channels.
    assert_eq!(
        body["components"][7],
        json!({"component": "posture", "cli": "claude", "status": "warn", "bypass": {"tmux": true, "headless": true}})
    );
    assert_eq!(
        body["components"][0],
        json!({"component": "api_protocol", "status": "ok", "version": "0.5.0"})
    );
    // Only the keys Sylvode keeps: no path, hint or detail anywhere. The `runner` row says ok or warn
    // and nothing of the posture details.
    let mut all = Vec::new();
    keys(body, &mut all);
    all.sort();
    all.dedup();
    assert_eq!(
        all,
        [
            "bypass",
            "cli",
            "component",
            "components",
            "generated_by",
            "headless",
            "status",
            "task_protocol",
            "tmux",
            "version"
        ]
    );
    assert_eq!(reports[0].authorization, calls[0].authorization);

    // The token is never in the log, not even at trace level.
    let log = server.stdout().join("\n");
    assert!(!log.contains(TOKEN) && !log.contains("opr_"), "{log}");
}

#[tokio::test(flavor = "multi_thread")]
async fn another_protocol_refuses_runner_deliveries_with_a_reason_code() {
    let api = MockSv::start().await;
    api.script(|s| s.protocol["task_protocol"] = json!(2));
    let (state, root) = (TempDir::new("hs-state"), root());
    let mut server =
        Server::start(RUNNER, &config(&api.url(), &state, &root.path().to_string_lossy()), &[]).expect("starts");
    let health = wait_health(&server.url("/health"), "mismatch").await;
    assert_eq!(health["protocol"], json!({"runner": 1, "api": 2, "state": "mismatch"}));
    assert!(server.wait_for_line("task protocol mismatch: runner 1, api 2", Duration::from_secs(5)));
    // The mismatch shows in the report as a failing row.
    let reports = api
        .wait_for("PUT", "/api/v1/ai/runner/report", 1, Duration::from_secs(5))
        .await;
    assert_eq!(reports[0].body["components"][0]["status"], "fail");
    // A delivery is refused at once, without asking Sylvode for the task.
    let delivery = json!({
        "task_id": "11111111-1111-4111-8111-111111111111", "project_id": "22222222-2222-4222-8222-222222222222",
        "ai_participant_id": BOT, "attempts": 1, "max_attempts": 3, "project_key": "SVWL", "protocol": 1,
        "trigger_kind": "assigned",
    });
    let (status, answer) = post_signed(&server.url("/webhook"), &delivery.to_string()).await;
    assert_eq!(status, 200, "{answer}");
    assert_eq!(answer["status"], "refused");
    assert_eq!(answer["reason_code"], "protocol_mismatch");
    assert!(answer["reason"].as_str().expect("reason").contains("api 2"), "{answer}");
    assert!(api.requests_about_a_task("GET").is_empty());
}

#[tokio::test(flavor = "multi_thread")]
async fn a_bot_without_a_runner_row_is_logged_once_and_not_retried() {
    let api = MockSv::start().await;
    api.script(|s| s.report_status = 404);
    let (state, root) = (TempDir::new("hs-state"), root());
    let mut server =
        Server::start(RUNNER, &config(&api.url(), &state, &root.path().to_string_lossy()), &[]).expect("starts");
    // Several rounds happen (one per second); the report is sent once.
    api.wait_for("GET", "/api/v1/ai/protocol", 3, Duration::from_secs(10))
        .await;
    assert_eq!(api.requests_to("PUT", "/api/v1/ai/runner/report").len(), 1);
    let lines = server.stdout().to_vec();
    let logged = lines.iter().filter(|l| l.contains("no runner connection")).count();
    assert_eq!(logged, 1, "{lines:?}");
    // The protocol itself still checks out.
    assert_eq!(health(&server.url("/health")).await["protocol"]["state"], "ok");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_refused_token_is_logged_without_the_token_and_once() {
    let api = MockSv::start().await;
    api.script(|s| s.protocol_status = 401);
    let (state, root) = (TempDir::new("hs-state"), root());
    let mut server = Server::start(
        RUNNER,
        &config(&api.url(), &state, &root.path().to_string_lossy()),
        &[("RUST_LOG", "trace")],
    )
    .expect("starts");
    api.wait_for("GET", "/api/v1/ai/protocol", 3, Duration::from_secs(10))
        .await;
    let want = format!("bot {BOT}: token refused by Sylvode (revoked, rotated or wrong surface)");
    assert!(server.wait_for_line(&want, Duration::from_secs(5)));
    let lines = server.stdout().to_vec();
    assert_eq!(
        lines.iter().filter(|l| l.contains("token refused")).count(),
        1,
        "{lines:?}"
    );
    assert!(
        lines.iter().all(|l| !l.contains(TOKEN) && !l.contains("opr_")),
        "{lines:?}"
    );
    assert_eq!(health(&server.url("/health")).await["protocol"]["state"], "unreachable");
    // No report is sent for a bot whose token is refused.
    assert!(api.requests_to("PUT", "/api/v1/ai/runner/report").is_empty());
}

#[tokio::test(flavor = "multi_thread")]
async fn without_a_runner_agent_nothing_is_asked_and_health_says_unchecked() {
    let api = MockSv::start().await;
    let config = format!(
        "[server]\nlisten = \"127.0.0.1:0\"\n\n[security]\nwebhook_secrets = [\"{SECRET}\"]\n\n\
         [ai_task_callback]\nenabled = true\napi_url = \"{}\"\ntoken = \"{TOKEN}\"\n",
        api.url()
    );
    let server = Server::start(RUNNER, &config, &[]).expect("starts");
    let answer = health(&server.url("/health")).await;
    assert_eq!(
        answer["protocol"],
        json!({"runner": 1, "api": null, "state": "unchecked"})
    );
    tokio::time::sleep(Duration::from_millis(500)).await;
    assert!(api.requests().is_empty());
}

#[tokio::test(flavor = "multi_thread")]
async fn health_answers_exactly_the_documented_keys() {
    let api = MockSv::start().await;
    let (state, root) = (TempDir::new("hs-state"), root());
    let server =
        Server::start(RUNNER, &config(&api.url(), &state, &root.path().to_string_lossy()), &[]).expect("starts");
    let answer = wait_health(&server.url("/health"), "ok").await;
    let mut top: Vec<&String> = answer.as_object().expect("object").keys().collect();
    top.sort();
    assert_eq!(top, ["protocol", "status"]);
    let mut inner: Vec<&String> = answer["protocol"].as_object().expect("object").keys().collect();
    inner.sort();
    assert_eq!(inner, ["api", "runner", "state"]);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_body_over_the_limit_is_refused_before_the_signature_is_checked() {
    use crate::support::post_signed_by;
    use std::error::Error as _;
    use std::fmt::Write as _;
    let api = MockSv::start().await;
    let (state, root) = (TempDir::new("hs-state"), root());
    let server =
        Server::start(RUNNER, &config(&api.url(), &state, &root.path().to_string_lossy()), &[]).expect("starts");
    let big = "x".repeat(3 * 1024 * 1024);
    // The signature is deliberately wrong: if the server checked it before the size, the answer
    // would be 401, not 413. The server refuses the body while the client is still writing it and
    // closes the connection, so depending on timing a client sees the 413 status or a connection
    // that was closed during the body write (a broken pipe or a reset). Both are a refusal; a
    // timeout or any other failure is not.
    let result = reqwest::Client::new()
        .post(server.url("/webhook"))
        .header("content-type", "application/json")
        .header("x-webhook-signature", "v1=00")
        .body(big)
        .send()
        .await;
    match result {
        Ok(response) => assert_eq!(response.status().as_u16(), 413),
        Err(error) => {
            let mut chain = format!("{error:?}");
            let mut source = error.source();
            while let Some(cause) = source {
                let _ = write!(chain, " {cause:?}");
                source = cause.source();
            }
            assert!(
                error.is_request()
                    && !error.is_timeout()
                    && (chain.contains("BrokenPipe") || chain.contains("ConnectionReset")),
                "neither 413 nor a connection closed during the body write: {chain}"
            );
        }
    }
    // An ordinary body is still answered.
    let (status, _) = post_signed_by(&server.url("/webhook"), "{}", SECRET).await;
    assert_eq!(status, 200);
}
