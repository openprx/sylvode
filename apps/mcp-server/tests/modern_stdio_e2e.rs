#![allow(clippy::indexing_slicing)]
//! The modern era over stdio on the shipped binary: several listen requests on one pipe next to
//! ordinary requests, cancellation, duplicate ids and the graceful end.

mod support;

use axum::{Json, Router, routing::get};
use serde_json::{Value, json};
use std::error::Error;
use std::process::Stdio;
use std::time::Duration;
use support::{McpSettings, write_config_with_extra_mcp_keys};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader, Lines};
use tokio::process::{Child, ChildStdin, ChildStdout, Command};

type TestResult = Result<(), Box<dyn Error>>;

const META: &str = "io.modelcontextprotocol/protocolVersion";
const SUBSCRIPTION_ID: &str = "io.modelcontextprotocol/subscriptionId";

struct Pipe {
    child: Child,
    stdin: ChildStdin,
    lines: Lines<BufReader<ChildStdout>>,
    _config: support::ConfigFile,
}

impl Pipe {
    /// Starts the server with the configured bot token, whose hint stream the stand-in holds open.
    async fn start(extra_keys: &str) -> Result<Self, Box<dyn Error>> {
        let router = Router::new()
            .route("/api/v1/bot/change-hints", get(support::held_change_hints))
            .fallback(get(|| async {
                Json(json!({"code": 0, "message": "ok", "data": {"items": [], "total": 0}}))
            }));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
        let api_url = format!("http://{}", listener.local_addr()?);
        tokio::spawn(async move {
            let _ = axum::serve(listener, router).await;
        });
        let config = write_config_with_extra_mcp_keys(
            &McpSettings {
                api_url: &api_url,
                bot_token: Some("opr_modern_stdio_bot_token"),
                workspace_id: Some("11111111-1111-4111-8111-111111111111"),
                transport: Some("stdio"),
                bind_addr: None,
            },
            &format!("{}{extra_keys}", support::FULL_TOOLS),
        )?;
        let mut child = Command::new(env!("CARGO_BIN_EXE_mcp-server"))
            .args(["serve", "--config"])
            .arg(config.path())
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .kill_on_drop(true)
            .spawn()?;
        let stdin = child.stdin.take().ok_or("no stdin")?;
        let lines = BufReader::new(child.stdout.take().ok_or("no stdout")?).lines();
        Ok(Self {
            child,
            stdin,
            lines,
            _config: config,
        })
    }

    async fn send(&mut self, message: &Value) -> TestResult {
        self.stdin.write_all(format!("{message}\n").as_bytes()).await?;
        self.stdin.flush().await?;
        Ok(())
    }

    async fn next(&mut self) -> Result<Option<Value>, Box<dyn Error>> {
        let line = tokio::time::timeout(Duration::from_secs(20), self.lines.next_line()).await??;
        line.map(|line| serde_json::from_str(&line).map_err(Into::into))
            .transpose()
    }

    /// Reads messages until one answers `id` (a message with that `id`), keeping the others.
    async fn answer_to(&mut self, id: i64, others: &mut Vec<Value>) -> Result<Value, Box<dyn Error>> {
        loop {
            let message = self.next().await?.ok_or("stdout closed")?;
            if message.get("id") == Some(&json!(id)) {
                return Ok(message);
            }
            others.push(message);
        }
    }
}

fn modern(id: i64, method: &str, params: Value) -> Value {
    let mut params = params;
    if let Some(fields) = params.as_object_mut() {
        fields.insert("_meta".to_string(), json!({ META: "2026-07-28" }));
    }
    json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params})
}

fn listen(id: i64) -> Value {
    modern(
        id,
        "subscriptions/listen",
        json!({"notifications": {"toolsListChanged": true}}),
    )
}

fn is_ack_for(message: &Value, id: i64) -> bool {
    message["method"] == "notifications/subscriptions/acknowledged" && message["params"]["_meta"][SUBSCRIPTION_ID] == id
}

#[tokio::test]
async fn two_listens_share_a_pipe_with_ordinary_requests() -> TestResult {
    let mut pipe = Pipe::start("").await?;
    pipe.send(&listen(1)).await?;
    pipe.send(&listen(2)).await?;
    let mut seen = Vec::new();
    for _ in 0..2 {
        seen.push(pipe.next().await?.ok_or("closed")?);
    }
    assert!(seen.iter().any(|m| is_ack_for(m, 1)), "{seen:?}");
    assert!(seen.iter().any(|m| is_ack_for(m, 2)), "{seen:?}");

    // An ordinary request is answered while both streams are open.
    pipe.send(&modern(3, "tools/list", json!({}))).await?;
    let mut others = Vec::new();
    let answer = pipe.answer_to(3, &mut others).await?;
    assert_eq!(answer["result"]["resultType"], "complete", "{answer}");
    assert!(answer["result"]["tools"].as_array().is_some_and(|t| !t.is_empty()));

    // A legacy initialize still works on the same pipe.
    pipe.send(&json!({"jsonrpc": "2.0", "id": 4, "method": "initialize", "params": {"protocolVersion": "2025-11-25"}}))
        .await?;
    let init = pipe.answer_to(4, &mut others).await?;
    assert_eq!(init["result"]["protocolVersion"], "2025-11-25");
    Ok(())
}

#[tokio::test]
async fn a_cancel_ends_one_listen_and_leaves_the_other() -> TestResult {
    let mut pipe = Pipe::start("").await?;
    pipe.send(&listen(1)).await?;
    pipe.send(&listen(2)).await?;
    for _ in 0..2 {
        pipe.next().await?.ok_or("closed")?;
    }
    pipe.send(&json!({"jsonrpc": "2.0", "method": "notifications/cancelled",
                      "params": {"_meta": {META: "2026-07-28"}, "requestId": 1}}))
        .await?;
    // The id 1 is free again once the task ended: a new listen with it is acknowledged.
    let mut reopened = false;
    for _ in 0..50 {
        pipe.send(&listen(1)).await?;
        let message = pipe.next().await?.ok_or("closed")?;
        if is_ack_for(&message, 1) {
            reopened = true;
            break;
        }
        assert_eq!(message["error"]["code"], -32600, "{message}");
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert!(reopened, "the cancelled id was never released");

    // No final result was sent for the cancelled stream; the other one is untouched: a request
    // is answered and no stray message precedes the answer.
    pipe.send(&modern(5, "tools/list", json!({}))).await?;
    let mut others = Vec::new();
    pipe.answer_to(5, &mut others).await?;
    assert!(others.is_empty(), "unexpected messages: {others:?}");
    Ok(())
}

#[tokio::test]
async fn a_duplicate_listen_id_is_refused() -> TestResult {
    let mut pipe = Pipe::start("").await?;
    pipe.send(&listen(1)).await?;
    assert!(is_ack_for(&pipe.next().await?.ok_or("closed")?, 1));
    pipe.send(&listen(1)).await?;
    let refusal = pipe.next().await?.ok_or("closed")?;
    assert_eq!(refusal["id"], 1);
    assert_eq!(refusal["error"]["code"], -32600, "{refusal}");
    assert_eq!(refusal["error"]["message"], "duplicate request id");
    Ok(())
}

#[tokio::test]
async fn listen_with_the_switch_off_is_method_not_found() -> TestResult {
    let mut pipe = Pipe::start("listen = false\n").await?;
    pipe.send(&listen(1)).await?;
    let answer = pipe.next().await?.ok_or("closed")?;
    assert_eq!(answer["error"]["code"], -32601, "{answer}");
    Ok(())
}

#[tokio::test]
async fn sigterm_sends_the_final_result_of_every_open_listen_and_exits_zero() -> TestResult {
    let mut pipe = Pipe::start("").await?;
    pipe.send(&listen(1)).await?;
    pipe.send(&listen(2)).await?;
    for _ in 0..2 {
        pipe.next().await?.ok_or("closed")?;
    }
    let pid = pipe.child.id().ok_or("exited")?;
    assert!(
        Command::new("kill")
            .arg("-TERM")
            .arg(pid.to_string())
            .status()
            .await?
            .success()
    );

    let mut finals = Vec::new();
    while let Some(message) = pipe.next().await? {
        finals.push(message);
    }
    assert_eq!(finals.len(), 2, "{finals:?}");
    for id in [1, 2] {
        let result = finals.iter().find(|m| m["id"] == id).ok_or("final result missing")?;
        assert_eq!(result["result"]["resultType"], "complete");
        assert_eq!(result["result"]["_meta"][SUBSCRIPTION_ID], id);
    }
    let exit = tokio::time::timeout(Duration::from_secs(10), pipe.child.wait()).await??;
    assert_eq!(exit.code(), Some(0));
    Ok(())
}

/// A listen that asks for nothing is the acknowledgment and then the final result, and its id is
/// free again at once.
#[tokio::test]
async fn a_listen_that_asks_for_nothing_ends_with_the_final_result() -> TestResult {
    let mut pipe = Pipe::start("").await?;
    for round in 1..=3 {
        pipe.send(&modern(round, "subscriptions/listen", json!({"notifications": {}})))
            .await?;
        assert!(is_ack_for(&pipe.next().await?.ok_or("closed")?, round));
        let end = pipe.next().await?.ok_or("closed")?;
        assert_eq!(end["id"], round, "{end}");
        assert_eq!(end["result"]["resultType"], "complete");
    }
    Ok(())
}

/// The acknowledgment of a listen id is on the pipe before the refusal of its duplicate.
#[tokio::test]
async fn the_acknowledgment_precedes_the_refusal_of_a_duplicate_id() -> TestResult {
    let mut pipe = Pipe::start("").await?;
    pipe.send(&listen(1)).await?;
    pipe.send(&listen(1)).await?;
    let first = pipe.next().await?.ok_or("closed")?;
    assert!(is_ack_for(&first, 1), "{first}");
    let second = pipe.next().await?.ok_or("closed")?;
    assert_eq!(second["error"]["code"], -32600, "{second}");
    Ok(())
}

#[tokio::test]
async fn a_listen_with_a_bad_envelope_is_refused() -> TestResult {
    let mut pipe = Pipe::start("").await?;
    let mut bad_version = listen(1);
    bad_version["jsonrpc"] = json!("1.0");
    pipe.send(&bad_version).await?;
    let answer = pipe.next().await?.ok_or("closed")?;
    assert_eq!(answer["error"]["code"], -32600, "{answer}");
    let mut object_id = listen(2);
    object_id["id"] = json!({"a": 1});
    pipe.send(&object_id).await?;
    let answer = pipe.next().await?.ok_or("closed")?;
    assert_eq!(answer["error"]["code"], -32600, "{answer}");
    assert!(answer["id"].is_null());
    Ok(())
}
