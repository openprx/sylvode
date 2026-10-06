//! `mcp-server serve` stops on SIGTERM/SIGINT with exit code 0 on every transport, after the
//! request in flight has been answered.
//!
//! In a container the server is PID 1, and the container runtime stops it with SIGTERM and then,
//! after its grace period, SIGKILL. A process that dies of the SIGTERM (exit 143) drops whatever
//! call it was serving; a process that ignores it is killed mid-call ten seconds later. Each test
//! here starts the shipped binary, makes a call that the stand-in API holds open, sends SIGTERM
//! while the call is in flight and asserts that the answer still arrives and that the process
//! then exits 0 within a bound.

mod support;

use axum::{Json, Router, routing::get};
use serde_json::{Value, json};
use std::error::Error;
use std::process::{ExitStatus, Stdio};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};
use support::{ConfigFile, McpSettings, write_config};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::net::TcpStream;
use tokio::process::{Child, Command};

type TestResult = Result<(), Box<dyn Error>>;

const WORKSPACE: &str = "11111111-1111-4111-8111-111111111111";
const TOKEN: &str = "opr_graceful_shutdown_bot_token";

/// How long the stand-in API holds every `labels` call open.
const API_DELAY: Duration = Duration::from_millis(1500);

/// The longest a stop may take in these tests: the server's own drain bound (8 s) plus margin.
/// Kept below the 10 s a container runtime waits before SIGKILL.
const STOP_BOUND: Duration = Duration::from_secs(9);

/// The longest a stop with one call in flight may take: the call's remaining [`API_DELAY`] plus
/// margin, well short of the server's 8 s drain bound, so an exit that merely waited the bound
/// out fails.
const IN_FLIGHT_STOP: Duration = Duration::from_secs(4);

/// A stand-in API whose `labels` endpoint answers after [`API_DELAY`], counting arrivals so a test
/// can send the signal only once the call is really in flight.
async fn spawn_slow_api() -> Result<(String, Arc<AtomicUsize>), Box<dyn Error>> {
    let arrived = Arc::new(AtomicUsize::new(0));
    let counter = Arc::clone(&arrived);
    let router = Router::new().route(
        "/api/v1/workspaces/{workspace_id}/labels",
        get(move || {
            let counter = Arc::clone(&counter);
            async move {
                counter.fetch_add(1, Ordering::SeqCst);
                tokio::time::sleep(API_DELAY).await;
                Json(json!({ "code": 0, "message": "ok", "data": [{ "id": "label-1", "name": "slow-label" }] }))
            }
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let addr = listener.local_addr()?;
    tokio::spawn(async move {
        let _ = axum::serve(listener, router).await;
    });
    Ok((format!("http://{addr}"), arrived))
}

async fn wait_for_arrival(arrived: &AtomicUsize) -> TestResult {
    for _ in 0..200 {
        if arrived.load(Ordering::SeqCst) > 0 {
            return Ok(());
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    Err("the call never reached the stand-in API".into())
}

async fn free_bind_addr() -> Result<String, Box<dyn Error>> {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let addr = listener.local_addr()?;
    drop(listener);
    Ok(addr.to_string())
}

fn spawn_server(config: &ConfigFile, stdin: Stdio, stdout: Stdio) -> Result<Child, Box<dyn Error>> {
    Ok(Command::new(env!("CARGO_BIN_EXE_mcp-server"))
        .args(["serve", "--config"])
        .arg(config.path())
        .stdin(stdin)
        .stdout(stdout)
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .spawn()?)
}

async fn send_signal(child: &Child, signal: &str) -> TestResult {
    let pid = child.id().ok_or("the server exited before it was signalled")?;
    let status = Command::new("kill").arg(signal).arg(pid.to_string()).status().await?;
    if status.success() {
        Ok(())
    } else {
        Err(format!("kill {signal} {pid} failed: {status}").into())
    }
}

/// Waits for the exit and returns its status with the time it took.
async fn wait_exit(child: &mut Child) -> Result<(ExitStatus, Duration), Box<dyn Error>> {
    let started = Instant::now();
    let status = tokio::time::timeout(STOP_BOUND, child.wait())
        .await
        .map_err(|_| format!("the server did not exit within {STOP_BOUND:?} of the signal"))??;
    Ok((status, started.elapsed()))
}

async fn wait_until_ready(base_url: &str) -> TestResult {
    let client = reqwest::Client::new();
    for _ in 0..200 {
        if let Ok(response) = client.get(format!("{base_url}/health")).send().await
            && response.status().is_success()
        {
            return Ok(());
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    Err("the MCP server never became ready".into())
}

fn labels_call(id: i32) -> Value {
    json!({
        "jsonrpc": "2.0",
        "id": id,
        "method": "tools/call",
        "params": { "name": "labels.list", "arguments": {} }
    })
}

fn assert_labels_answer(response: &Value) {
    let text = response
        .pointer("/result/content/0/text")
        .and_then(Value::as_str)
        .unwrap_or_default();
    assert!(
        text.contains("slow-label"),
        "the in-flight call must be answered with its result: {response}"
    );
}

fn network_config(api_url: &str, transport: &str, bind_addr: &str) -> Result<ConfigFile, Box<dyn Error>> {
    write_config(&McpSettings {
        api_url,
        bot_token: None,
        workspace_id: WORKSPACE,
        transport: Some(transport),
        bind_addr: Some(bind_addr),
    })
}

#[tokio::test]
async fn http_transport_answers_the_call_in_flight_and_exits_zero_on_sigterm() -> TestResult {
    let (api_url, arrived) = spawn_slow_api().await?;
    let bind_addr = free_bind_addr().await?;
    let config = network_config(&api_url, "http", &bind_addr)?;
    let mut child = spawn_server(&config, Stdio::null(), Stdio::null())?;
    let base_url = format!("http://{bind_addr}");
    wait_until_ready(&base_url).await?;

    let call = tokio::spawn({
        let url = format!("{base_url}/mcp/rpc");
        async move {
            reqwest::Client::new()
                .post(url)
                .bearer_auth(TOKEN)
                .json(&labels_call(1))
                .send()
                .await?
                .json::<Value>()
                .await
        }
    });
    wait_for_arrival(&arrived).await?;
    send_signal(&child, "-TERM").await?;
    let signalled = Instant::now();

    let response = tokio::time::timeout(STOP_BOUND, call).await???;
    assert_labels_answer(&response);
    let (status, _) = wait_exit(&mut child).await?;
    assert_eq!(
        status.code(),
        Some(0),
        "SIGTERM must end the server with exit 0, got {status}"
    );
    assert!(
        signalled.elapsed() < IN_FLIGHT_STOP,
        "the stop must follow the in-flight answer, not the drain bound: {:?}",
        signalled.elapsed()
    );

    // The listener is gone once the process has exited.
    assert!(
        TcpStream::connect(&bind_addr).await.is_err(),
        "the listener outlived the process"
    );
    Ok(())
}

#[tokio::test]
async fn http_transport_exits_zero_on_sigint_when_idle() -> TestResult {
    let (api_url, _arrived) = spawn_slow_api().await?;
    let bind_addr = free_bind_addr().await?;
    let config = network_config(&api_url, "http", &bind_addr)?;
    let mut child = spawn_server(&config, Stdio::null(), Stdio::null())?;
    wait_until_ready(&format!("http://{bind_addr}")).await?;

    send_signal(&child, "-INT").await?;
    let (status, took) = wait_exit(&mut child).await?;
    assert_eq!(
        status.code(),
        Some(0),
        "SIGINT must end the server with exit 0, got {status}"
    );
    assert!(
        took < Duration::from_secs(2),
        "an idle server must stop at once, not wait out its drain bound: {took:?}"
    );
    Ok(())
}

/// One open `GET /sse` connection read off a raw socket, as in `sse_transport_e2e.rs`.
struct SseStream {
    socket: TcpStream,
    buffer: String,
    cursor: usize,
}

impl SseStream {
    async fn open(authority: &str) -> Result<Self, Box<dyn Error>> {
        let mut socket = TcpStream::connect(authority).await?;
        let request = format!(
            "GET /sse HTTP/1.1\r\nHost: {authority}\r\nAccept: text/event-stream\r\nAuthorization: Bearer {TOKEN}\r\n\r\n"
        );
        socket.write_all(request.as_bytes()).await?;
        socket.flush().await?;
        Ok(Self {
            socket,
            buffer: String::new(),
            cursor: 0,
        })
    }

    /// Reads one more chunk; `Ok(false)` on EOF.
    async fn read_more(&mut self) -> Result<bool, Box<dyn Error>> {
        let mut chunk = [0_u8; 4096];
        let read = tokio::time::timeout(STOP_BOUND, self.socket.read(&mut chunk)).await??;
        if read == 0 {
            return Ok(false);
        }
        self.buffer
            .push_str(&String::from_utf8_lossy(chunk.get(..read).unwrap_or_default()));
        Ok(true)
    }

    async fn next_event(&mut self, event_name: &str) -> Result<String, Box<dyn Error>> {
        let needle = format!("event: {event_name}\ndata: ");
        loop {
            if let Some(tail) = self.buffer.get(self.cursor..)
                && let Some(offset) = tail.find(&needle)
            {
                let payload_start = self.cursor + offset + needle.len();
                if let Some(rest) = self.buffer.get(payload_start..)
                    && let Some(end) = rest.find('\n')
                {
                    self.cursor = payload_start + end;
                    return Ok(self
                        .buffer
                        .get(payload_start..payload_start + end)
                        .unwrap_or_default()
                        .to_string());
                }
            }
            if !self.read_more().await? {
                return Err(format!("the stream ended before an `{event_name}` event").into());
            }
        }
    }

    /// Reads until the server closes the stream.
    async fn read_to_end(&mut self) -> TestResult {
        while self.read_more().await? {}
        Ok(())
    }
}

#[tokio::test]
async fn sse_transport_delivers_the_result_in_flight_then_ends_the_stream_and_exits_zero() -> TestResult {
    let (api_url, arrived) = spawn_slow_api().await?;
    let bind_addr = free_bind_addr().await?;
    let config = network_config(&api_url, "sse", &bind_addr)?;
    let mut child = spawn_server(&config, Stdio::null(), Stdio::null())?;
    let base_url = format!("http://{bind_addr}");
    wait_until_ready(&base_url).await?;

    let mut stream = SseStream::open(&bind_addr).await?;
    let endpoint = stream.next_event("endpoint").await?;

    let post = tokio::spawn({
        let url = format!("{base_url}{endpoint}");
        async move {
            reqwest::Client::new()
                .post(url)
                .bearer_auth(TOKEN)
                .json(&labels_call(7))
                .send()
                .await
                .map(|response| response.status())
        }
    });
    wait_for_arrival(&arrived).await?;
    send_signal(&child, "-TERM").await?;
    let signalled = Instant::now();

    let accepted = tokio::time::timeout(STOP_BOUND, post).await???;
    assert_eq!(
        accepted,
        reqwest::StatusCode::ACCEPTED,
        "the in-flight message was not accepted"
    );
    let message: Value = serde_json::from_str(&stream.next_event("message").await?)?;
    assert_eq!(message.get("id"), Some(&json!(7)));
    assert_labels_answer(&message);

    // The open stream must not hold the process: the server ends it once nothing is in flight.
    stream.read_to_end().await?;
    let (status, _) = wait_exit(&mut child).await?;
    assert_eq!(
        status.code(),
        Some(0),
        "SIGTERM must end the server with exit 0, got {status}"
    );
    assert!(
        signalled.elapsed() < IN_FLIGHT_STOP,
        "the stop must follow the in-flight answer, not the drain bound: {:?}",
        signalled.elapsed()
    );
    Ok(())
}

fn stdio_config(api_url: &str) -> Result<ConfigFile, Box<dyn Error>> {
    write_config(&McpSettings {
        api_url,
        bot_token: Some(TOKEN),
        workspace_id: WORKSPACE,
        transport: Some("stdio"),
        bind_addr: None,
    })
}

#[tokio::test]
async fn stdio_transport_writes_the_answer_in_flight_and_exits_zero_on_sigterm() -> TestResult {
    let (api_url, arrived) = spawn_slow_api().await?;
    let config = stdio_config(&api_url)?;
    let mut child = spawn_server(&config, Stdio::piped(), Stdio::piped())?;
    let mut stdin = child.stdin.take().ok_or("no stdin pipe")?;
    let stdout = child.stdout.take().ok_or("no stdout pipe")?;
    let mut lines = BufReader::new(stdout).lines();

    stdin.write_all(format!("{}\n", labels_call(3)).as_bytes()).await?;
    stdin.flush().await?;
    wait_for_arrival(&arrived).await?;
    send_signal(&child, "-TERM").await?;
    let signalled = Instant::now();

    let line = tokio::time::timeout(STOP_BOUND, lines.next_line())
        .await??
        .ok_or("stdout closed before the in-flight answer was written")?;
    let response: Value = serde_json::from_str(&line)?;
    assert_eq!(response.get("id"), Some(&json!(3)));
    assert_labels_answer(&response);

    // stdin stays open: the parent has not hung up, so only the signal can end the process.
    let (status, _) = wait_exit(&mut child).await?;
    assert_eq!(
        status.code(),
        Some(0),
        "SIGTERM must end the server with exit 0, got {status}"
    );
    assert!(
        signalled.elapsed() < IN_FLIGHT_STOP,
        "the stop must follow the in-flight answer, not the drain bound: {:?}",
        signalled.elapsed()
    );
    assert_eq!(
        lines.next_line().await?,
        None,
        "nothing may be written after the in-flight answer"
    );
    drop(stdin);
    Ok(())
}

#[tokio::test]
async fn stdio_transport_exits_zero_on_sigterm_while_waiting_for_input() -> TestResult {
    let (api_url, _arrived) = spawn_slow_api().await?;
    let config = stdio_config(&api_url)?;
    let mut child = spawn_server(&config, Stdio::piped(), Stdio::piped())?;
    let stdin = child.stdin.take().ok_or("no stdin pipe")?;
    let stdout = child.stdout.take().ok_or("no stdout pipe")?;
    let mut lines = BufReader::new(stdout).lines();

    // Wait until the process is serving: an `initialize` round trip proves the loop is reading.
    let mut stdin = stdin;
    let initialize = json!({
        "jsonrpc": "2.0", "id": 1, "method": "initialize",
        "params": { "protocolVersion": "2024-11-05", "clientInfo": { "name": "shutdown-e2e", "version": "0" } }
    });
    stdin.write_all(format!("{initialize}\n").as_bytes()).await?;
    stdin.flush().await?;
    tokio::time::timeout(STOP_BOUND, lines.next_line())
        .await??
        .ok_or("no initialize answer")?;

    send_signal(&child, "-TERM").await?;
    let (status, took) = wait_exit(&mut child).await?;
    assert_eq!(
        status.code(),
        Some(0),
        "SIGTERM must end the server with exit 0, got {status}"
    );
    assert!(
        took < Duration::from_secs(2),
        "a server blocked on stdin must still stop at once: {took:?}"
    );
    drop(stdin);
    Ok(())
}
