//! Shared harness: runs the real executable against a generated config, with fake executors on
//! `PATH`, an in-process stub of the Sylvode API that records every request, and an in-process
//! WebSocket control plane for the tunnel.

use axum::{Router, body::Bytes, extract::State, http::HeaderMap, routing::post};
use hmac::{Hmac, Mac};
use serde_json::Value;
use sha2::Sha256;
use std::io::{BufRead, BufReader};
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant};
use tokio::net::TcpListener;

pub const CANONICAL: &str = env!("CARGO_BIN_EXE_sylvode-webhook");
pub const LEGACY: &str = env!("CARGO_BIN_EXE_openpr-webhook");
pub const SECRET: &str = "e2e-webhook-secret";

/// A directory removed when dropped.
pub struct TempDir(pub PathBuf);

impl TempDir {
    pub fn new(label: &str) -> Self {
        let dir = std::env::temp_dir().join(format!("sylvode-webhook-e2e-{label}-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).expect("create temp dir");
        Self(dir)
    }

    pub fn path(&self) -> &Path {
        &self.0
    }

    pub fn file(&self, name: &str) -> PathBuf {
        self.0.join(name)
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// Writes an executable `sh` script called `name` into `dir`.
pub fn write_script(dir: &Path, name: &str, body: &str) -> PathBuf {
    let path = dir.join(name);
    std::fs::write(&path, format!("#!/bin/sh\n{body}\n")).expect("write script");
    let mut perms = std::fs::metadata(&path).expect("script metadata").permissions();
    std::os::unix::fs::PermissionsExt::set_mode(&mut perms, 0o755);
    std::fs::set_permissions(&path, perms).expect("chmod script");
    path
}

/// The `PATH` every server starts with: system tools only, so a real `codex`, `claude` or
/// `opencode` installed for the user running the tests can never be started by a test.
pub const SAFE_PATH: &str = "/usr/bin:/bin";

/// [`SAFE_PATH`] with `dir`, holding fake executors, in front.
pub fn path_with(dir: &Path) -> String {
    format!("{}:{SAFE_PATH}", dir.display())
}

/// A running server process. Dropping it kills and reaps the process.
pub struct Server {
    child: Option<Child>,
    pub addr: SocketAddr,
    lines: mpsc::Receiver<String>,
    seen: Vec<String>,
    stderr: Option<std::thread::JoinHandle<Vec<String>>>,
    _dir: TempDir,
}

/// What a process that did not come up wrote before it exited.
#[derive(Debug)]
pub struct Refused {
    pub code: Option<i32>,
    pub stdout: Vec<String>,
    pub stderr: Vec<String>,
}

fn spawn_line_reader(stream: impl std::io::Read + Send + 'static) -> mpsc::Receiver<String> {
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        for line in BufReader::new(stream).lines() {
            let Ok(line) = line else { break };
            if tx.send(line).is_err() {
                break;
            }
        }
    });
    rx
}

impl Server {
    /// Starts `binary` with `config` (which must listen on `127.0.0.1:0`) and waits until it logs
    /// the address it bound. `Err` carries the output of a process that exited instead.
    pub fn start(binary: &str, config: &str, env: &[(&str, &str)]) -> Result<Self, Refused> {
        let dir = TempDir::new("server");
        let config_path = dir.file("config.toml");
        std::fs::write(&config_path, config).expect("write config");
        let mut command = Command::new(binary);
        command
            .arg(&config_path)
            .env_remove("RUST_LOG")
            .env_remove("SYLVODE_WEBHOOK_SAFE_MODE")
            .env_remove("OPENPR_WEBHOOK_SAFE_MODE")
            .env("NO_COLOR", "1")
            .env("PATH", SAFE_PATH)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        for (key, value) in env {
            command.env(key, value);
        }
        let mut child = command.spawn().expect("spawn server");
        let lines = spawn_line_reader(child.stdout.take().expect("piped stdout"));
        let stderr_pipe = child.stderr.take().expect("piped stderr");
        let stderr = std::thread::spawn(move || {
            BufReader::new(stderr_pipe)
                .lines()
                .map_while(Result::ok)
                .collect::<Vec<_>>()
        });
        let mut server = Self {
            child: Some(child),
            addr: SocketAddr::from(([127, 0, 0, 1], 0)),
            lines,
            seen: Vec::new(),
            stderr: Some(stderr),
            _dir: dir,
        };
        let deadline = Instant::now() + Duration::from_secs(20);
        while Instant::now() < deadline {
            match server.lines.recv_timeout(Duration::from_millis(50)) {
                Ok(line) => {
                    let bound = line
                        .split_once(" listening on ")
                        .and_then(|(_, addr)| addr.trim().parse::<SocketAddr>().ok());
                    server.seen.push(line);
                    if let Some(addr) = bound {
                        server.addr = addr;
                        return Ok(server);
                    }
                }
                Err(mpsc::RecvTimeoutError::Timeout) => {
                    if server.child_mut().try_wait().expect("try_wait").is_some() {
                        return Err(server.into_refused());
                    }
                }
                Err(mpsc::RecvTimeoutError::Disconnected) => return Err(server.into_refused()),
            }
        }
        panic!("{binary} did not report a listening address: {:?}", server.seen);
    }

    const fn child_mut(&mut self) -> &mut Child {
        self.child.as_mut().expect("server process already reaped")
    }

    fn into_refused(mut self) -> Refused {
        let mut child = self.child.take().expect("server process already reaped");
        let status = child.wait().expect("wait for refused server");
        self.seen.extend(self.lines.try_iter());
        let stderr = self
            .stderr
            .take()
            .map(|h| h.join().unwrap_or_default())
            .unwrap_or_default();
        Refused {
            code: status.code(),
            stdout: std::mem::take(&mut self.seen),
            stderr,
        }
    }

    pub fn url(&self, path: &str) -> String {
        format!("http://{}{path}", self.addr)
    }

    pub fn pid(&mut self) -> u32 {
        self.child_mut().id()
    }

    /// Every stdout line written so far.
    pub fn stdout(&mut self) -> &[String] {
        self.seen.extend(self.lines.try_iter());
        &self.seen
    }

    /// Waits until a stdout line contains `needle`.
    pub fn wait_for_line(&mut self, needle: &str, timeout: Duration) -> bool {
        let deadline = Instant::now() + timeout;
        if self.stdout().iter().any(|line| line.contains(needle)) {
            return true;
        }
        while Instant::now() < deadline {
            if let Ok(line) = self.lines.recv_timeout(Duration::from_millis(50)) {
                let hit = line.contains(needle);
                self.seen.push(line);
                if hit {
                    return true;
                }
            }
        }
        false
    }

    /// Sends SIGTERM.
    pub fn terminate(&mut self) {
        let pid = self.pid().to_string();
        let status = Command::new("kill").args(["-TERM", &pid]).status().expect("run kill");
        assert!(status.success(), "kill -TERM {pid} failed");
    }

    /// Waits for the process to exit and returns its exit code and stderr lines.
    pub fn wait_exit(&mut self, timeout: Duration) -> Option<(Option<i32>, Vec<String>)> {
        let deadline = Instant::now() + timeout;
        while Instant::now() < deadline {
            if let Some(status) = self.child_mut().try_wait().expect("try_wait") {
                self.child = None;
                let _ = self.stdout();
                let stderr = self
                    .stderr
                    .take()
                    .map(|h| h.join().unwrap_or_default())
                    .unwrap_or_default();
                return Some((status.code(), stderr));
            }
            std::thread::sleep(Duration::from_millis(25));
        }
        None
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        if let Some(mut child) = self.child.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

pub fn sign(body: &str, secret: &str) -> String {
    let mut mac = Hmac::<Sha256>::new_from_slice(secret.as_bytes()).expect("hmac key");
    mac.update(body.as_bytes());
    format!("sha256={}", hex::encode(mac.finalize().into_bytes()))
}

/// Posts `body` to the `/webhook` URL `url` signed with [`SECRET`] and returns the status and the
/// JSON answer.
pub async fn post_signed(url: &str, body: &str) -> (u16, Value) {
    post_with(url, body, &[("x-webhook-signature", &sign(body, SECRET))]).await
}

pub async fn post_with(url: &str, body: &str, headers: &[(&str, &str)]) -> (u16, Value) {
    let mut request = reqwest::Client::new()
        .post(url)
        .header("content-type", "application/json")
        .body(body.to_owned());
    for (name, value) in headers {
        request = request.header(*name, *value);
    }
    let response = request.send().await.expect("post webhook");
    let status = response.status().as_u16();
    let text = response.text().await.expect("read answer");
    (status, serde_json::from_str(&text).unwrap_or(Value::String(text)))
}

/// One request the stub API received.
#[derive(Debug, Clone)]
pub struct Recorded {
    pub headers: HeaderMap,
    pub body: Value,
}

/// An HTTP server that answers every POST with a JSON-RPC success and records the request.
pub struct StubApi {
    pub addr: SocketAddr,
    received: tokio::sync::mpsc::UnboundedReceiver<Recorded>,
    task: tokio::task::JoinHandle<()>,
}

async fn record(
    State(tx): State<tokio::sync::mpsc::UnboundedSender<Recorded>>,
    headers: HeaderMap,
    body: Bytes,
) -> &'static str {
    let body = serde_json::from_slice(&body).unwrap_or(Value::Null);
    let _ = tx.send(Recorded { headers, body });
    r#"{"jsonrpc":"2.0","id":1,"result":{"content":[]}}"#
}

impl StubApi {
    pub async fn start() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind stub api");
        let addr = listener.local_addr().expect("stub addr");
        let (tx, received) = tokio::sync::mpsc::unbounded_channel();
        let app = Router::new().route("/{*path}", post(record)).with_state(tx);
        let task = tokio::spawn(async move {
            let _ = axum::serve(listener, app).await;
        });
        Self { addr, received, task }
    }

    pub fn url(&self, path: &str) -> String {
        format!("http://{}{path}", self.addr)
    }

    pub async fn next(&mut self, timeout: Duration) -> Option<Recorded> {
        tokio::time::timeout(timeout, self.received.recv()).await.ok().flatten()
    }
}

impl Drop for StubApi {
    fn drop(&mut self) {
        self.task.abort();
    }
}

/// Whether process `pid` is alive (a zombie counts as dead).
pub fn alive(pid: i32) -> bool {
    std::fs::read_to_string(format!("/proc/{pid}/stat")).is_ok_and(|stat| {
        stat.rsplit_once(')')
            .and_then(|(_, rest)| rest.split_whitespace().next())
            .is_some_and(|state| state != "Z" && state != "X")
    })
}

/// Reads one pid per line from `file`.
pub fn read_pids(file: &Path) -> Vec<i32> {
    std::fs::read_to_string(file)
        .unwrap_or_default()
        .lines()
        .filter_map(|line| line.trim().parse().ok())
        .collect()
}

/// Waits until none of `pids` is alive; returns the survivors otherwise.
pub fn wait_all_dead(pids: &[i32], timeout: Duration) -> Vec<i32> {
    let deadline = Instant::now() + timeout;
    loop {
        let survivors: Vec<i32> = pids.iter().copied().filter(|pid| alive(*pid)).collect();
        if survivors.is_empty() || Instant::now() >= deadline {
            return survivors;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

/// Waits until `file` has at least `count` lines.
pub fn wait_lines(file: &Path, count: usize, timeout: Duration) -> Vec<String> {
    let deadline = Instant::now() + timeout;
    loop {
        let lines: Vec<String> = std::fs::read_to_string(file)
            .unwrap_or_default()
            .lines()
            .map(str::to_owned)
            .collect();
        if lines.len() >= count || Instant::now() >= deadline {
            return lines;
        }
        std::thread::sleep(Duration::from_millis(25));
    }
}

/// A webhook delivery for a bot task assigned to bot `bot_name`.
pub fn bot_task(bot_name: &str, title: &str) -> Value {
    serde_json::json!({
        "id": uuid::Uuid::new_v4().to_string(),
        "event": "issue.assigned",
        "timestamp": chrono_now(),
        "project": {"id": "p1", "name": "Billing"},
        "actor": {"name": "Alice"},
        "data": {"issue": {"id": "ISSUE-1", "key": "BIL-1", "title": title}},
        "bot_context": {
            "is_bot_task": true,
            "bot_name": bot_name,
            "bot_agent_type": "cli",
            "trigger_reason": "assigned"
        }
    })
}

/// The current time in RFC 3339, as the Sylvode API writes `timestamp`.
pub fn chrono_now() -> String {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());
    rfc3339(secs)
}

/// Formats Unix seconds as `YYYY-MM-DDTHH:MM:SS+00:00`.
pub fn rfc3339(secs: u64) -> String {
    let days = secs / 86_400;
    let rem = secs % 86_400;
    let (hour, minute, second) = (rem / 3600, (rem % 3600) / 60, rem % 60);
    // Civil-from-days, Howard Hinnant's algorithm.
    let z = i64::try_from(days).expect("days fit i64") + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = if month <= 2 { y + 1 } else { y };
    format!("{year:04}-{month:02}-{d:02}T{hour:02}:{minute:02}:{second:02}+00:00")
}
