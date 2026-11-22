//! The `hook` subcommand: it never reads stdin, ignores extra arguments, returns within a second,
//! with or without a socket, and sends nothing outside tmux.

use crate::support::RUNNER;
use std::io::Write;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

/// Runs `hook` with an endless stdin and returns how long it took and its exit code.
fn run(args: &[&str]) -> (Duration, Option<i32>) {
    let mut child = Command::new(RUNNER)
        .arg("hook")
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn hook");
    let mut stdin = child.stdin.take().expect("stdin");
    // Endless input: a hook that read it would never finish.
    let feeder = std::thread::spawn(move || {
        let chunk = vec![b'x'; 64 * 1024];
        while stdin.write_all(&chunk).is_ok() {}
    });
    let started = Instant::now();
    let status = loop {
        if let Some(status) = child.try_wait().expect("try_wait") {
            break Some(status.code());
        }
        if started.elapsed() > Duration::from_secs(10) {
            let _ = child.kill();
            let _ = child.wait();
            break None;
        }
        std::thread::sleep(Duration::from_millis(5));
    };
    let took = started.elapsed();
    let _ = feeder.join();
    (took, status.flatten())
}

#[test]
fn the_hook_returns_at_once_without_reading_stdin_even_without_a_socket() {
    let (took, code) = run(&[
        "--cli",
        "claude",
        "--event",
        "Stop",
        "--socket",
        "/tmp/svh-no-such.sock",
    ]);
    assert_eq!(code, Some(0));
    assert!(took < Duration::from_secs(3), "{took:?}");
}

#[test]
fn extra_arguments_and_missing_ones_are_ignored() {
    // codex appends a JSON argument with the turn's text; it must not matter.
    let (took, code) = run(&[
        "--cli",
        "codex",
        "--event",
        "turn-ended",
        "--socket",
        "/tmp/svh-no-such.sock",
        "{\"type\":\"agent-turn-complete\",\"last-assistant-message\":\"secret prompt text\"}",
        "--unknown",
        "x",
    ]);
    assert_eq!((code, took < Duration::from_secs(3)), (Some(0), true));
    // No arguments at all, and no socket argument: still a clean exit.
    assert_eq!(run(&[]).1, Some(0));
    assert_eq!(run(&["--cli", "claude", "--event", "Stop"]).1, Some(0));
}

/// Outside tmux (no `TMUX`, no `TMUX_PANE`, or a malformed one) the hook sends nothing and writes
/// nothing; inside, it sends one version 2 line naming the server's socket and the pane.
#[test]
fn a_hook_sends_one_line_inside_tmux_and_nothing_outside() {
    use std::io::Read;
    use std::os::unix::net::UnixListener;
    // Short: a unix socket path is limited to about 100 bytes on macOS.
    let base =
        std::path::PathBuf::from("/tmp").join(format!("svh-{}", &uuid::Uuid::new_v4().simple().to_string()[..8]));
    let state = base.join("s");
    std::fs::create_dir_all(&state).expect("state");
    let socket = state.join("hook.sock");
    let listener = UnixListener::bind(&socket).expect("bind");
    listener.set_nonblocking(true).expect("nonblocking");
    let hook = |tmux: Option<&str>, pane: Option<&str>| {
        let mut command = Command::new(RUNNER);
        command
            .args(["hook", "--cli", "claude", "--event", "Stop", "--socket"])
            .arg(&socket)
            .env_remove("TMUX")
            .env_remove("TMUX_PANE")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        if let Some(tmux) = tmux {
            command.env("TMUX", tmux);
        }
        if let Some(pane) = pane {
            command.env("TMUX_PANE", pane);
        }
        let status = command.status().expect("run hook");
        assert!(status.success(), "a hook always exits 0");
    };
    for (tmux, pane) in [
        (None, None),
        (Some("/tmp/tmux-1/default,99,0"), None),
        (None, Some("%3")),
        (Some("garbage"), Some("%3")),
        (Some("/tmp/tmux-1/default,99,0"), Some("three")),
    ] {
        hook(tmux, pane);
        assert!(listener.accept().is_err(), "sent something with {tmux:?} {pane:?}");
    }
    assert!(!state.join("panes").exists(), "an outside hook wrote a counter");
    hook(Some("/tmp/tmux-1/default,99,0"), Some("%3"));
    let (mut stream, _) = listener.accept().expect("one line was sent");
    stream.set_nonblocking(false).expect("blocking");
    let mut line = String::new();
    let _ = stream.read_to_string(&mut line);
    let value: serde_json::Value = serde_json::from_str(line.trim()).expect("json");
    assert_eq!(
        (
            &value["v"],
            &value["cli"],
            &value["event"],
            &value["tmux"],
            &value["pane"],
            &value["seq"]
        ),
        (
            &serde_json::json!(2),
            &serde_json::json!("claude"),
            &serde_json::json!("Stop"),
            &serde_json::json!("/tmp/tmux-1/default"),
            &serde_json::json!("%3"),
            &serde_json::json!(1)
        ),
        "{line}"
    );
    assert!(value.get("session").is_none() && value.get("nonce").is_none());
    assert!(state.join("panes/99-3.seq").is_file());
    let _ = std::fs::remove_dir_all(&base);
}
