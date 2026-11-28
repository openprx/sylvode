//! Runs the real executable and checks inbound signature handling over HTTP.
//!
//! Every accepted header verifies a good signature (200) and refuses a bad one (401), and headers
//! that disagree are refused (401) even when one of them is valid. Nothing is written to stderr.

#![cfg(unix)]
// Test helpers outside `#[test]` functions may fail loudly; this is test code only.
#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

use hmac::{Hmac, Mac};
use sha2::Sha256;
use std::io::{BufRead, BufReader};
use std::process::{Child, Command, Stdio};
use std::time::Duration;

const RUNNER: &str = env!("CARGO_BIN_EXE_sylvode-runner");
const SECRET: &str = "integration-secret";
const BODY: &str = r#"{"event":"issue.created","data":{"title":"signature test"}}"#;

/// The `v1=<hex>` signature over `<timestamp>.<body>`.
fn sign_at(secret: &str, timestamp: &str) -> String {
    let mut mac = Hmac::<Sha256>::new_from_slice(secret.as_bytes()).unwrap();
    mac.update(timestamp.as_bytes());
    mac.update(b".");
    mac.update(BODY.as_bytes());
    format!("v1={}", hex::encode(mac.finalize().into_bytes()))
}

fn now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs()
}

/// A running server. Dropping it kills and reaps the process, then removes its temp directory,
/// so a failed assertion or panic anywhere in a test cannot leave the process running.
struct Server {
    /// `None` once `stop` has reaped the process.
    child: Option<Child>,
    base: String,
    _dir: TempDir,
}

impl Server {
    const fn child(&mut self) -> &mut Child {
        self.child.as_mut().expect("server process already reaped")
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        if let Some(mut child) = self.child.take() {
            // Killing a process that has already exited fails harmlessly; wait reaps it either way.
            let _ = child.kill();
            let _ = child.wait();
        }
        // `_dir` is dropped after this, once the process can no longer use it.
    }
}

struct TempDir(std::path::PathBuf);

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// Starts `binary` on a port the system chooses and reads that port from the listening line, so
/// parallel tests cannot collide on a port reserved and released beforehand.
fn start(binary: &str) -> Server {
    let dir = std::env::temp_dir().join(format!("sylvode-runner-it-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&dir).unwrap();
    let config = dir.join("config.toml");
    std::fs::write(
        &config,
        format!("[server]\nlisten = \"127.0.0.1:0\"\n\n[security]\nwebhook_secrets = [\"{SECRET}\"]\nallow_unsigned = false\n"),
    )
    .unwrap();
    let mut child = Command::new(binary)
        .arg(&config)
        .env_remove("RUST_LOG")
        .env_remove("SYLVODE_RUNNER_SAFE_MODE")
        .env("NO_COLOR", "1")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn server");
    let stdout = child.stdout.take().expect("piped stdout");
    let (tx, rx) = std::sync::mpsc::channel();
    // Keeps draining stdout for the life of the process, so it never blocks on a full pipe.
    std::thread::spawn(move || {
        for line in BufReader::new(stdout).lines().map_while(Result::ok) {
            let _ = tx.send(line);
        }
    });
    let mut server = Server {
        child: Some(child),
        base: String::new(),
        _dir: TempDir(dir),
    };
    while let Ok(line) = rx.recv_timeout(Duration::from_secs(20)) {
        if let Some((_, addr)) = line.split_once(" listening on ") {
            server.base = format!("http://{}", addr.trim());
            return server;
        }
    }
    let _ = server.child().kill();
    panic!("{binary} did not report a listening address");
}

/// Stops the server and returns its stderr lines.
fn stop(mut server: Server) -> Vec<String> {
    let mut child = server.child.take().expect("server process already reaped");
    child.kill().unwrap();
    let output = child.wait_with_output().unwrap();
    String::from_utf8_lossy(&output.stderr)
        .lines()
        .map(str::to_owned)
        .collect()
}

async fn post(server: &Server, headers: &[(&str, &str)]) -> (u16, String) {
    let mut req = reqwest::Client::new()
        .post(format!("{}/webhook", server.base))
        .header("content-type", "application/json")
        .body(BODY);
    for (name, value) in headers {
        req = req.header(*name, *value);
    }
    let resp = req.send().await.unwrap();
    (resp.status().as_u16(), resp.text().await.unwrap())
}

/// A labelled request (its signature headers) and the status the service must answer it with.
type Case = (&'static str, Vec<(&'static str, String)>, u16);

/// Every request below and the status the service must answer it with.
fn cases(good: &str, bad: &str, ts: &str) -> Vec<Case> {
    let (sylvode_ts, webhook_ts) = (
        ("x-sylvode-timestamp", ts.to_owned()),
        ("x-webhook-timestamp", ts.to_owned()),
    );
    vec![
        (
            "sylvode good",
            vec![("x-sylvode-signature", good.to_owned()), sylvode_ts.clone()],
            200,
        ),
        (
            "sylvode bad",
            vec![("x-sylvode-signature", bad.to_owned()), sylvode_ts.clone()],
            401,
        ),
        (
            "webhook good",
            vec![("x-webhook-signature", good.to_owned()), webhook_ts.clone()],
            200,
        ),
        (
            "webhook bad",
            vec![("x-webhook-signature", bad.to_owned()), webhook_ts.clone()],
            401,
        ),
        (
            "good signature, no timestamp",
            vec![("x-webhook-signature", good.to_owned())],
            401,
        ),
        (
            "old sha256 prefix",
            vec![
                ("x-webhook-signature", good.replace("v1=", "sha256=")),
                webhook_ts.clone(),
            ],
            401,
        ),
        (
            "good signature, stale timestamp",
            vec![
                ("x-webhook-signature", sign_at(SECRET, "1000")),
                ("x-webhook-timestamp", "1000".to_owned()),
            ],
            401,
        ),
        ("no signature", vec![], 401),
        (
            "all agree",
            vec![
                ("x-sylvode-signature", good.to_owned()),
                ("x-webhook-signature", good.to_owned()),
                sylvode_ts.clone(),
                webhook_ts.clone(),
            ],
            200,
        ),
        (
            "good sylvode + bad webhook",
            vec![
                ("x-sylvode-signature", good.to_owned()),
                ("x-webhook-signature", bad.to_owned()),
                webhook_ts.clone(),
            ],
            401,
        ),
        (
            "good webhook + bad sylvode",
            vec![
                ("x-webhook-signature", good.to_owned()),
                ("x-sylvode-signature", bad.to_owned()),
                webhook_ts,
            ],
            401,
        ),
    ]
}

#[tokio::test]
async fn every_header_case_is_answered_as_expected_and_nothing_is_written_to_stderr() {
    let ts = now().to_string();
    let (good, bad) = (sign_at(SECRET, &ts), sign_at("not-the-secret", &ts));
    assert_ne!(good, bad);
    let server = start(RUNNER);
    for (label, headers, expected) in cases(&good, &bad, &ts) {
        let borrowed: Vec<(&str, &str)> = headers.iter().map(|(n, v)| (*n, v.as_str())).collect();
        let (status, _) = post(&server, &borrowed).await;
        assert_eq!(status, expected, "{label}");
    }
    let stderr = stop(server);
    assert!(stderr.is_empty(), "nothing may be written to stderr: {stderr:?}");
}

#[cfg(unix)]
#[tokio::test]
async fn sigterm_stops_the_server_cleanly() {
    let mut server = start(RUNNER);
    let status = Command::new("kill")
        .args(["-TERM", &server.child().id().to_string()])
        .status()
        .unwrap();
    assert!(status.success());
    for _ in 0..200 {
        if let Some(exit) = server.child().try_wait().unwrap() {
            assert_eq!(exit.code(), Some(0), "graceful shutdown exits 0");
            return;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    // Dropping `server` while unwinding kills and reaps the process.
    panic!("server did not stop within 5s of SIGTERM");
}
