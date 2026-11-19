//! Runs the real executables and checks inbound signature handling over HTTP.
//!
//! Every accepted header verifies a good signature (200) and refuses a bad one (401); headers
//! that disagree are refused (401) even when one of them is valid; both executable names answer
//! identically; and only the legacy `x-openpr-signature` header (and the legacy executable name)
//! write a deprecation line to stderr.

#![cfg(unix)]
// Test helpers outside `#[test]` functions may fail loudly; this is test code only.
#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

use hmac::{Hmac, Mac};
use sha2::Sha256;
use std::io::{BufRead, BufReader};
use std::process::{Child, Command, Stdio};
use std::time::Duration;

const CANONICAL: &str = env!("CARGO_BIN_EXE_sylvode-webhook");
const LEGACY: &str = env!("CARGO_BIN_EXE_openpr-webhook");
const SECRET: &str = "integration-secret";
const BODY: &str = r#"{"event":"issue.created","data":{"title":"signature test"}}"#;
const EXECUTABLE_NOTICE: &str = "warning: the executable name `openpr-webhook` is deprecated, use `sylvode-webhook` \
                                 instead; `openpr-webhook` is not removed before Sylvode v2.0";
const HEADER_NOTICE: &str = "warning: the inbound signature header `x-openpr-signature` is deprecated, use \
                             `x-sylvode-signature` instead; `x-openpr-signature` is not removed before Sylvode v2.0";

fn sign(secret: &str) -> String {
    let mut mac = Hmac::<Sha256>::new_from_slice(secret.as_bytes()).unwrap();
    mac.update(BODY.as_bytes());
    format!("sha256={}", hex::encode(mac.finalize().into_bytes()))
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
    let dir = std::env::temp_dir().join(format!("sylvode-webhook-it-{}", uuid::Uuid::new_v4()));
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
        .env_remove("SYLVODE_WEBHOOK_SAFE_MODE")
        .env_remove("OPENPR_WEBHOOK_SAFE_MODE")
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
fn cases(good: &str, bad: &str) -> Vec<Case> {
    vec![
        ("sylvode good", vec![("x-sylvode-signature", good.to_owned())], 200),
        ("sylvode bad", vec![("x-sylvode-signature", bad.to_owned())], 401),
        ("webhook good", vec![("x-webhook-signature", good.to_owned())], 200),
        ("webhook bad", vec![("x-webhook-signature", bad.to_owned())], 401),
        ("openpr good", vec![("x-openpr-signature", good.to_owned())], 200),
        ("openpr bad", vec![("x-openpr-signature", bad.to_owned())], 401),
        ("no signature", vec![], 401),
        (
            "all agree",
            vec![
                ("x-sylvode-signature", good.to_owned()),
                ("x-webhook-signature", good.to_owned()),
                ("x-openpr-signature", good.to_owned()),
            ],
            200,
        ),
        (
            "good sylvode + bad openpr",
            vec![
                ("x-sylvode-signature", good.to_owned()),
                ("x-openpr-signature", bad.to_owned()),
            ],
            401,
        ),
        (
            "good webhook + bad sylvode",
            vec![
                ("x-webhook-signature", good.to_owned()),
                ("x-sylvode-signature", bad.to_owned()),
            ],
            401,
        ),
    ]
}

async fn answers(binary: &str) -> (Vec<(u16, String)>, Vec<String>) {
    let (good, bad) = (sign(SECRET), sign("not-the-secret"));
    assert_ne!(good, bad);
    let server = start(binary);
    let mut results = Vec::new();
    for (label, headers, expected) in cases(&good, &bad) {
        let borrowed: Vec<(&str, &str)> = headers.iter().map(|(n, v)| (*n, v.as_str())).collect();
        let (status, body) = post(&server, &borrowed).await;
        assert_eq!(status, expected, "{binary}: {label}");
        results.push((status, body));
    }
    (results, stop(server))
}

#[tokio::test]
async fn both_names_answer_every_header_case_identically() {
    let (canonical, canonical_stderr) = answers(CANONICAL).await;
    let (legacy, legacy_stderr) = answers(LEGACY).await;
    assert_eq!(legacy, canonical, "responses must not depend on the executable name");

    // Both processes saw x-openpr-signature several times: one header notice each, no repeats.
    assert_eq!(canonical_stderr, vec![HEADER_NOTICE.to_string()]);
    assert_eq!(
        legacy_stderr,
        vec![EXECUTABLE_NOTICE.to_string(), HEADER_NOTICE.to_string()]
    );
}

#[tokio::test]
async fn canonical_name_and_headers_write_no_notice() {
    let good = sign(SECRET);
    let server = start(CANONICAL);
    assert_eq!(post(&server, &[("x-sylvode-signature", &good)]).await.0, 200);
    assert_eq!(post(&server, &[("x-webhook-signature", &good)]).await.0, 200);
    let stderr = stop(server);
    assert!(
        stderr.is_empty(),
        "canonical name and headers must not warn: {stderr:?}"
    );
}

#[cfg(unix)]
#[tokio::test]
async fn sigterm_stops_the_server_cleanly() {
    let mut server = start(CANONICAL);
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
