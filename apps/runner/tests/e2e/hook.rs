//! The `hook` subcommand: it never reads stdin, ignores extra arguments and returns within a
//! second, with or without a socket.

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
        "--session",
        "SVWL",
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
        "--session",
        "SVWL",
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
