#![allow(clippy::indexing_slicing)]

use std::io::Write;
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use serde_json::json;
use tokio::sync::mpsc;

use super::hooks::{
    CLIENT_BUDGET, HookEvent, MAX_LINE, Reject, SeqGate, ancestry, chain_allows, ct_eq, is_same_user, listen,
    listen_checked, listen_with_check, next_seq, parse_line, send,
};

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
        // The session's private directory, where its hook counter lives.
        std::fs::create_dir_all(dir.join("sessions").join("SVWL")).expect("mkdir");
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

/// Events are applied in the order the senders stamped them: a late one is ignored.
#[test]
fn an_event_older_than_the_last_applied_one_is_ignored() {
    let mut gate = SeqGate::default();
    assert!(gate.accept(10));
    assert!(gate.accept(20));
    assert!(!gate.accept(15), "overtaken");
    assert!(!gate.accept(20), "repeated");
    assert!(gate.accept(21));
    // The stamp is the sender's clock in nanoseconds and travels in the message.
    let message = parse_line(&line(&good())).expect("valid");
    assert_eq!(message.seq, 1_700_000_000);
}

/// A socket that is removed or replaced would silence every hook; the listener notices and binds
/// again.
#[tokio::test]
async fn a_removed_or_replaced_socket_is_bound_again() {
    let dir = Dir::new();
    let socket = dir.0.join("hook.sock");
    let (tx, mut rx) = mpsc::channel(8);
    let _server = listen_with_check(&socket, tx, Duration::from_millis(100)).expect("listen");
    for how in ["removed", "replaced"] {
        if how == "removed" {
            std::fs::remove_file(&socket).expect("remove");
        } else {
            std::fs::remove_file(&socket).ok();
            std::fs::write(&socket, "not a socket").expect("replace");
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
        let path = socket.clone();
        tokio::task::spawn_blocking(move || send(&path, "claude", "SVWL", "Stop", NONCE))
            .await
            .expect("sent");
        let message = tokio::time::timeout(Duration::from_secs(5), rx.recv())
            .await
            .unwrap_or_else(|_| panic!("{how}: no message after the socket was {how}"))
            .expect("message");
        assert_eq!(message.event, HookEvent::Stop, "{how}");
    }
}

/// A hook runs below its session's pane process: the sender's parent chain, read at the socket,
/// must reach the pane PID. A process and its parents are listed innermost first.
#[test]
fn the_ancestry_of_a_process_lists_its_parents() {
    let me = std::process::id();
    let mut child = std::process::Command::new("/bin/sleep")
        .arg("5")
        .spawn()
        .expect("spawn");
    let chain = ancestry(child.id());
    assert_eq!(chain.first(), Some(&child.id()));
    assert!(chain.contains(&me), "{chain:?}");
    // A process that is not below this one does not list it.
    assert!(!ancestry(1).contains(&me));
    assert_eq!(
        ancestry(4_000_000),
        vec![4_000_000],
        "an unknown process has no parents"
    );
    let _ = child.kill();
    let _ = child.wait();
}

/// The listener reads the sender's chain while it is connected: a message from a process below
/// this one carries a chain that contains this process.
#[tokio::test]
async fn the_listener_names_the_senders_ancestry() {
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
    let chain = message.peer_chain.expect("the platform names the peer");
    assert!(chain.contains(&std::process::id()), "{chain:?}");
}

/// A message of another user is never heard, whatever it says.
#[test]
fn another_user_is_not_the_runners_user() {
    assert!(is_same_user(501, 501));
    assert!(!is_same_user(0, 501));
    assert!(!is_same_user(502, 501));
}

#[test]
fn the_pane_must_be_in_the_chain_and_an_unknown_pane_refuses() {
    assert!(chain_allows(Some(&[5, 4, 3]), Some(4)));
    assert!(!chain_allows(Some(&[5, 4, 3]), Some(9)));
    assert!(!chain_allows(Some(&[5, 4, 3]), None));
    assert!(chain_allows(None, Some(9)));
}

/// The listener asks the worker's check before it spends anything: a message with a wrong nonce is
/// dropped there and never reaches the channel; the right one does.
#[tokio::test]
async fn a_message_with_a_wrong_nonce_never_reaches_the_session() {
    let dir = Dir::new();
    let socket = dir.0.join("hook.sock");
    let (tx, mut rx) = mpsc::channel(8);
    let accept: super::hooks::Accept = std::sync::Arc::new(|m| ct_eq(&m.nonce, NONCE));
    let _server = listen_checked(&socket, tx, Duration::from_secs(5), accept).expect("listen");
    let wrong = "ffffffffffffffffffffffffffffffff";
    for (nonce, event) in [(wrong, "Notification"), (NONCE, "Stop")] {
        let path = socket.clone();
        tokio::task::spawn_blocking(move || send(&path, "claude", "SVWL", event, nonce))
            .await
            .expect("sent");
    }
    let first = tokio::time::timeout(Duration::from_secs(5), rx.recv())
        .await
        .expect("in time")
        .expect("message");
    assert_eq!(first.event, HookEvent::Stop, "the wrong-nonce message was passed on");
    assert!(rx.try_recv().is_err());
}

/// The order stamp is the session's own counter: strictly increasing across concurrent hooks, from
/// one up, whatever the clock does.
#[test]
fn the_hook_counter_is_strictly_increasing_across_concurrent_hooks() {
    let dir = Dir::new();
    let state = dir.0.clone();
    let handles: Vec<_> = (0..8)
        .map(|_| {
            let state = state.clone();
            std::thread::spawn(move || (0..10).filter_map(|_| next_seq(&state, "SVWL")).collect::<Vec<u64>>())
        })
        .collect();
    let mut all: Vec<u64> = handles.into_iter().flat_map(|h| h.join().expect("thread")).collect();
    all.sort_unstable();
    assert_eq!(all, (1..=80).collect::<Vec<u64>>());
    // A session that has no private directory yet has no counter: nothing is stamped.
    assert!(next_seq(&state, "NOSESSION").is_none());
}
