#![allow(clippy::indexing_slicing)]

use std::io::Write;
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use serde_json::json;
use tokio::sync::mpsc;

use super::hooks::{
    CLIENT_BUDGET, HookEvent, MAX_LINE, PaneIdentity, Reject, SeqGate, ancestry, chain_allows, is_same_user, listen,
    listen_checked, listen_with_check, next_seq, parse_line, send, seq_file,
};

fn line(value: &serde_json::Value) -> Vec<u8> {
    value.to_string().into_bytes()
}

const TMUX_SOCKET: &str = "/tmp/tmux-501/default";

fn good() -> serde_json::Value {
    json!({
        "v": 2, "cli": "claude", "event": "Stop", "tmux": TMUX_SOCKET, "pane": "%7", "seq": 5, "ts": 1_700_000_000
    })
}

/// Where a hook runs, as `TMUX` and `TMUX_PANE` say.
fn who() -> PaneIdentity {
    PaneIdentity::from_env(Some("/tmp/tmux-501/default,4242,0"), Some("%7")).expect("inside tmux")
}

#[test]
fn only_the_documented_message_shape_is_accepted() {
    let message = parse_line(&line(&good())).expect("valid");
    assert_eq!(
        (
            message.cli.as_str(),
            message.event,
            message.tmux.as_str(),
            message.pane.as_str(),
            message.seq
        ),
        ("claude", HookEvent::Stop, TMUX_SOCKET, "%7", 5)
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
    let without = |key: &str| {
        let mut v = good();
        if let Some(object) = v.as_object_mut() {
            object.remove(key);
        }
        v
    };
    for bad in [
        // Version 1 lines (session and nonce) are not heard.
        json!({"v": 1, "cli": "claude", "session": "SVWL-claude", "event": "Stop",
               "nonce": "0123456789abcdef0123456789abcdef", "ts": 1}),
        with("v", json!(1)),
        with("v", json!(3)),
        with("v", json!("2")),
        with("cli", json!("emacs")),
        with("cli", json!("codex")), // codex has no Stop hook
        with("event", json!("PreToolUse")),
        with("event", json!("turn-ended")), // claude has no turn-ended
        with("tmux", json!("relative/socket")),
        with("tmux", json!("")),
        with("tmux", json!("/tmp/a\nb")),
        with("tmux", json!(7)),
        with("pane", json!("7")),
        with("pane", json!("%")),
        with("pane", json!("%7 ")),
        with("pane", json!("%1234567890")),
        with("seq", json!("1")),
        with("seq", json!(-1)),
        with("ts", json!("now")),
        with("ts", json!(null)),
        without("tmux"),
        without("pane"),
        without("seq"),
        without("ts"),
        with("session", json!("SVWL-claude")),
        with("nonce", json!("0123456789abcdef0123456789abcdef")),
        with("prompt", json!("secret text")),
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

/// The hook reads `TMUX` (`<socket path>,<server pid>,<session index>`) and `TMUX_PANE`; outside
/// tmux, or with anything malformed, it has no identity and sends nothing.
#[test]
fn the_hook_reads_tmux_and_the_pane_from_its_environment() {
    let identity = who();
    assert_eq!(
        (identity.socket.as_str(), identity.server_pid, identity.pane.as_str()),
        (TMUX_SOCKET, 4242, "%7")
    );
    // A socket path may hold a comma.
    let comma = PaneIdentity::from_env(Some("/tmp/a,b/default,17,3"), Some("%0")).expect("identity");
    assert_eq!((comma.socket.as_str(), comma.server_pid), ("/tmp/a,b/default", 17));
    for (tmux, pane) in [
        (None, Some("%7")),
        (Some("/tmp/x,1,0"), None),
        (Some(""), Some("%7")),
        (Some("/tmp/x"), Some("%7")),
        (Some("/tmp/x,1"), Some("%7")),
        (Some("/tmp/x,abc,0"), Some("%7")),
        (Some("relative,1,0"), Some("%7")),
        (Some("/tmp/x\n,1,0"), Some("%7")),
        (Some("/tmp/x,1,0"), Some("7")),
        (Some("/tmp/x,1,0"), Some("%")),
        (Some("/tmp/x,1,0"), Some("%a")),
    ] {
        assert_eq!(PaneIdentity::from_env(tmux, pane), None, "{tmux:?} {pane:?}");
    }
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
            json!({"v": 2, "cli": "claude", "event": "Stop", "tmux": TMUX_SOCKET, "pane": "%7", "seq": 1, "ts": 1, "prompt": "p"})
        )
        .into_bytes(),
        // A version 1 line, as an older hook command would send it.
        format!(
            "{}\n",
            json!({"v": 1, "cli": "claude", "session": "SVWL-claude", "event": "Stop", "nonce": "0123456789abcdef0123456789abcdef", "ts": 1})
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
        tokio::task::spawn_blocking(move || send(&path, "claude", "Stop", Some(&who())))
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
    tokio::task::spawn_blocking(move || send(&path, "claude", "Stop", Some(&who())))
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
    send(&dir.0.join("absent.sock"), "claude", "Stop", Some(&who()));
    assert!(started.elapsed() < CLIENT_BUDGET, "{:?}", started.elapsed());
    // A socket that accepts and never reads does not hold it either (the line is small, so the
    // write completes; the budget bounds the rest).
    let socket = dir.0.join("stuck.sock");
    let _listener = std::os::unix::net::UnixListener::bind(&socket).expect("bind");
    let started = Instant::now();
    send(&socket, "claude", "Stop", Some(&who()));
    assert!(started.elapsed() < Duration::from_secs(1), "{:?}", started.elapsed());
}

/// Nothing is sent outside tmux, and the message names the pane and a counter value.
#[tokio::test]
async fn a_hook_outside_tmux_sends_nothing_and_inside_names_its_pane() {
    let dir = Dir::new();
    let socket = dir.0.join("hook.sock");
    let (tx, mut rx) = mpsc::channel(4);
    let _server = listen(&socket, tx).expect("listen");
    let path = socket.clone();
    tokio::task::spawn_blocking(move || send(&path, "claude", "Stop", None))
        .await
        .expect("sent");
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert!(rx.try_recv().is_err(), "a hook outside tmux sent something");
    assert!(!dir.0.join("panes").exists(), "and wrote a counter");
    let path = socket.clone();
    tokio::task::spawn_blocking(move || send(&path, "claude", "Stop", Some(&who())))
        .await
        .expect("sent");
    let message = tokio::time::timeout(Duration::from_secs(5), rx.recv())
        .await
        .expect("in time")
        .expect("message");
    assert_eq!(
        (message.tmux.as_str(), message.pane.as_str(), message.seq),
        (TMUX_SOCKET, "%7", 1)
    );
    assert!(seq_file(&dir.0, 4242, "%7").is_file());
}

#[tokio::test]
async fn a_peer_of_another_user_is_not_heard() {
    // The listener compares the peer's uid with its own. Another uid cannot be produced in a
    // test, so this pins the positive side: a same-user client is heard.
    let dir = Dir::new();
    let socket = dir.0.join("hook.sock");
    let (tx, mut rx) = mpsc::channel(4);
    let _server = listen(&socket, tx).expect("listen");
    let path = socket.clone();
    tokio::task::spawn_blocking(move || send(&path, "claude", "Stop", Some(&who())))
        .await
        .expect("sent");
    let message = tokio::time::timeout(Duration::from_secs(5), rx.recv())
        .await
        .expect("in time")
        .expect("message");
    assert_eq!(message.pane, "%7");
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
    // The stamp is the pane's own counter and travels in the message.
    let message = parse_line(&line(&good())).expect("valid");
    assert_eq!(message.seq, 5);
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
        tokio::task::spawn_blocking(move || send(&path, "claude", "Stop", Some(&who())))
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
    tokio::task::spawn_blocking(move || send(&path, "claude", "Stop", Some(&who())))
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

/// The listener asks the worker's check before it spends anything: a message from a pane no
/// session uses is dropped there and never reaches the channel; one from a known pane does.
#[tokio::test]
async fn a_message_from_an_unknown_pane_never_reaches_the_session() {
    let dir = Dir::new();
    let socket = dir.0.join("hook.sock");
    let (tx, mut rx) = mpsc::channel(8);
    let accept: super::hooks::Accept = std::sync::Arc::new(|m| m.tmux == TMUX_SOCKET && m.pane == "%7");
    let _server = listen_checked(&socket, tx, Duration::from_secs(5), accept).expect("listen");
    let elsewhere = PaneIdentity::from_env(Some("/tmp/tmux-501/other,9,0"), Some("%7")).expect("identity");
    let other_pane = PaneIdentity::from_env(Some("/tmp/tmux-501/default,4242,0"), Some("%8")).expect("identity");
    for (who, event) in [
        (elsewhere, "Notification"),
        (other_pane, "Notification"),
        (who(), "Stop"),
    ] {
        let path = socket.clone();
        tokio::task::spawn_blocking(move || send(&path, "claude", event, Some(&who)))
            .await
            .expect("sent");
    }
    let first = tokio::time::timeout(Duration::from_secs(5), rx.recv())
        .await
        .expect("in time")
        .expect("message");
    assert_eq!(
        first.event,
        HookEvent::Stop,
        "a message from an unknown pane was passed on"
    );
    assert!(rx.try_recv().is_err());
}

/// The order stamp is the pane's own counter: strictly increasing across concurrent hooks, from
/// one up, whatever the clock does; each pane (and each server) has its own.
#[test]
fn the_hook_counter_is_strictly_increasing_across_concurrent_hooks() {
    let dir = Dir::new();
    let state = dir.0.clone();
    // All threads must be running before any is joined: the collect is needed.
    #[allow(clippy::needless_collect)]
    let handles: Vec<_> = (0..8)
        .map(|_| {
            let state = state.clone();
            std::thread::spawn(move || (0..10).filter_map(|_| next_seq(&state, &who())).collect::<Vec<u64>>())
        })
        .collect();
    let mut all: Vec<u64> = handles.into_iter().flat_map(|h| h.join().expect("thread")).collect();
    all.sort_unstable();
    assert_eq!(all, (1..=80).collect::<Vec<u64>>());
    // Another pane, and another server with the same pane id, count from one on their own.
    let other_pane = PaneIdentity::from_env(Some("/tmp/tmux-501/default,4242,0"), Some("%8")).expect("identity");
    let other_server = PaneIdentity::from_env(Some("/tmp/tmux-501/default,4243,0"), Some("%7")).expect("identity");
    assert_eq!(next_seq(&state, &other_pane), Some(1));
    assert_eq!(next_seq(&state, &other_server), Some(1));
    for name in ["4242-7.seq", "4242-8.seq", "4243-7.seq"] {
        let path = state.join("panes").join(name);
        assert_eq!(
            std::fs::metadata(&path).expect("counter file").permissions().mode() & 0o777,
            0o600,
            "{name}"
        );
    }
    assert_eq!(
        std::fs::metadata(state.join("panes"))
            .expect("dir")
            .permissions()
            .mode()
            & 0o777,
        0o700
    );
}

/// codex's notify fires several times per prompt: only `agent-turn-complete` with a `client` key
/// is the main thread's turn end; the rest, an absent or oversized or malformed payload included,
/// is dropped. Other CLIs are never filtered.
#[test]
fn only_the_main_threads_codex_turn_end_is_sent() {
    use super::hooks::{MAX_NOTIFY_PAYLOAD, event_counts};
    let main = json!({"type": "agent-turn-complete", "client": "codex-tui", "last-assistant-message": "x"}).to_string();
    assert!(event_counts("codex", Some(&main)));
    for (name, payload) in [
        ("no client", json!({"type": "agent-turn-complete"}).to_string()),
        (
            "other type",
            json!({"type": "approval-requested", "client": "codex-tui"}).to_string(),
        ),
        ("no type", json!({"client": "codex-tui"}).to_string()),
        ("not an object", "[1]".to_string()),
        ("not json", "agent-turn-complete".to_string()),
        (
            "too large",
            format!(
                "{{\"type\":\"agent-turn-complete\",\"client\":\"c\",\"pad\":\"{}\"}}",
                "x".repeat(MAX_NOTIFY_PAYLOAD)
            ),
        ),
    ] {
        assert!(!event_counts("codex", Some(&payload)), "{name}");
    }
    assert!(!event_counts("codex", None));
    assert!(event_counts("claude", None) && event_counts("claude", Some("anything")));
}

/// A hook message applies only to the session of its own CLI.
#[test]
fn a_hook_message_applies_only_to_the_session_of_its_cli() {
    use super::session::hook_is_for_session;
    assert!(hook_is_for_session("QLB-codex", "codex"));
    assert!(hook_is_for_session("QLB-claude", "claude"));
    assert!(!hook_is_for_session("QLB-claude", "codex"));
    assert!(!hook_is_for_session("QLB-codex", "claude"));
    assert!(!hook_is_for_session("not a name", "codex"));
}
