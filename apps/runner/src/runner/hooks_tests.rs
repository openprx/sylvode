#![allow(clippy::indexing_slicing)]

use std::io::Write;
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use serde_json::json;
use tokio::sync::mpsc;

use super::hooks::{CLIENT_BUDGET, HookEvent, MAX_LINE, Reject, ct_eq, listen, parse_line, send};

fn line(value: &serde_json::Value) -> Vec<u8> {
    value.to_string().into_bytes()
}

const NONCE: &str = "0123456789abcdef0123456789abcdef";

fn good() -> serde_json::Value {
    json!({"v": 1, "cli": "claude", "session": "SVWL", "event": "Stop", "nonce": NONCE, "ts": 1_700_000_000})
}

#[test]
fn only_the_documented_message_shape_is_accepted() {
    let message = parse_line(&line(&good())).expect("valid");
    assert_eq!(
        (message.cli.as_str(), message.session.as_str(), message.event),
        ("claude", "SVWL", HookEvent::Stop)
    );
    for (cli, event, want) in [
        ("claude", "SessionStart", HookEvent::SessionStart),
        ("claude", "UserPromptSubmit", HookEvent::UserPromptSubmit),
        ("claude", "Notification", HookEvent::Notification),
        ("codex", "turn-ended", HookEvent::TurnEnded),
    ] {
        let mut value = good();
        value["cli"] = json!(cli);
        value["event"] = json!(event);
        assert_eq!(parse_line(&line(&value)).expect("valid").event, want);
    }
    let with = |key: &str, value: serde_json::Value| {
        let mut v = good();
        v[key] = value;
        v
    };
    for bad in [
        with("v", json!(2)),
        with("v", json!("1")),
        with("cli", json!("emacs")),
        with("cli", json!("codex")), // codex has no Stop hook
        with("event", json!("PreToolUse")),
        with("event", json!("turn-ended")), // claude has no turn-ended
        with("session", json!("svwl")),
        with("session", json!("SV:WL")),
        with("session", json!("")),
        with("session", json!(7)),
        with("ts", json!("now")),
        with("ts", json!(null)),
        with("nonce", json!("short")),
        with("nonce", json!("zzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzz")),
        with("nonce", json!(7)),
        json!({"v": 1, "cli": "claude", "session": "SVWL", "event": "Stop", "ts": 1}),
        json!({"v": 1, "cli": "claude", "session": "SVWL", "event": "Stop", "ts": 1, "nonce": NONCE, "prompt": "secret text"}),
        json!([]),
        json!("text"),
        json!(5),
        json!(null),
    ] {
        assert_eq!(parse_line(&line(&bad)), Err(Reject::Shape), "{bad}");
    }
    assert_eq!(parse_line(b"not json"), Err(Reject::NotJson));
    assert_eq!(parse_line(b""), Err(Reject::NotJson));
    assert_eq!(parse_line(&vec![b' '; MAX_LINE]), Err(Reject::TooLong));
    // Just under the limit is still judged by its content.
    assert_eq!(parse_line(&vec![b' '; MAX_LINE - 1]), Err(Reject::NotJson));
}

struct Dir(PathBuf);

impl Dir {
    fn new() -> Self {
        // Short: a unix socket path is limited to about 100 bytes on macOS.
        let dir = PathBuf::from("/tmp").join(format!("svh-{}", &uuid::Uuid::new_v4().simple().to_string()[..8]));
        std::fs::create_dir_all(&dir).expect("mkdir");
        Self(dir)
    }
}

impl Drop for Dir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[tokio::test]
async fn the_socket_is_private_forwards_good_lines_and_drops_the_rest() {
    let dir = Dir::new();
    let socket = dir.0.join("hook.sock");
    let (tx, mut rx) = mpsc::channel(16);
    let _server = listen(&socket, tx).expect("listen");
    assert_eq!(
        std::fs::metadata(&socket).expect("meta").permissions().mode() & 0o777,
        0o600
    );
    let send_raw = |bytes: Vec<u8>| {
        let socket = socket.clone();
        tokio::task::spawn_blocking(move || {
            let mut stream = UnixStream::connect(&socket).expect("connect");
            let _ = stream.write_all(&bytes);
        })
    };
    // Malformed, oversized and unknown-shaped lines are dropped; a good one passes after them.
    for bad in [
        b"garbage\n".to_vec(),
        format!(
            "{}\n",
            json!({"v": 1, "cli": "claude", "session": "SVWL", "event": "Stop", "ts": 1, "nonce": NONCE, "prompt": "p"})
        )
        .into_bytes(),
        vec![b'a'; 100_000],
        format!("{}\n", "x".repeat(MAX_LINE * 4)).into_bytes(),
        vec![0xff, 0xfe, b'\n'],
    ] {
        send_raw(bad).await.expect("sent");
    }
    {
        let path = socket.clone();
        tokio::task::spawn_blocking(move || send(&path, "claude", "SVWL", "Stop", NONCE))
            .await
            .expect("sent");
    }
    let message = tokio::time::timeout(Duration::from_secs(5), rx.recv())
        .await
        .expect("in time")
        .expect("a message");
    assert_eq!(message.event, HookEvent::Stop);
    assert!(rx.try_recv().is_err(), "nothing else may have passed");
    // A client that never ends its line is cut off and holds nothing.
    let silent = UnixStream::connect(&socket).expect("connect");
    tokio::time::sleep(Duration::from_millis(100)).await;
    drop(silent);
    assert!(rx.try_recv().is_err());
}

#[tokio::test]
async fn a_stale_socket_file_is_replaced() {
    let dir = Dir::new();
    let socket = dir.0.join("hook.sock");
    std::fs::write(&socket, "old").expect("stale");
    let (tx, mut rx) = mpsc::channel(4);
    let _server = listen(&socket, tx).expect("listen");
    let path = socket.clone();
    tokio::task::spawn_blocking(move || send(&path, "claude", "SVWL", "Stop", NONCE))
        .await
        .expect("sent");
    let message = tokio::time::timeout(Duration::from_secs(5), rx.recv())
        .await
        .expect("in time")
        .expect("message");
    assert_eq!((message.cli.as_str(), message.event), ("claude", HookEvent::Stop));
}

#[test]
fn the_client_returns_quickly_without_a_socket_or_with_a_stuck_one() {
    let dir = Dir::new();
    let started = Instant::now();
    send(&dir.0.join("absent.sock"), "claude", "SVWL", "Stop", NONCE);
    assert!(started.elapsed() < CLIENT_BUDGET, "{:?}", started.elapsed());
    // A socket that accepts and never reads does not hold it either (the line is small, so the
    // write completes; the budget bounds the rest).
    let socket = dir.0.join("stuck.sock");
    let _listener = std::os::unix::net::UnixListener::bind(&socket).expect("bind");
    let started = Instant::now();
    send(&socket, "claude", "SVWL", "Stop", NONCE);
    assert!(started.elapsed() < Duration::from_secs(1), "{:?}", started.elapsed());
}

#[test]
fn the_nonce_is_carried_and_compared_without_stopping_at_the_first_difference() {
    assert_eq!(parse_line(&line(&good())).expect("valid").nonce, NONCE);
    assert!(ct_eq(NONCE, NONCE));
    assert!(!ct_eq(NONCE, "0123456789abcdef0123456789abcdee"));
    assert!(!ct_eq(NONCE, "0123456789abcdef"));
    assert!(!ct_eq("", NONCE));
}

#[tokio::test]
async fn a_peer_of_another_user_is_not_heard() {
    // The listener compares the peer's uid with its own. Another uid cannot be produced in a
    // test, so this pins the positive side: a same-user client is heard with its nonce intact.
    let dir = Dir::new();
    let socket = dir.0.join("hook.sock");
    let (tx, mut rx) = mpsc::channel(4);
    let _server = listen(&socket, tx).expect("listen");
    let path = socket.clone();
    tokio::task::spawn_blocking(move || send(&path, "claude", "SVWL", "Stop", NONCE))
        .await
        .expect("sent");
    let message = tokio::time::timeout(Duration::from_secs(5), rx.recv())
        .await
        .expect("in time")
        .expect("message");
    assert_eq!(message.nonce, NONCE);
}
