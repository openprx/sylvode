//! The tmux driver against a real tmux, on a server of its own.
//!
//! Ignored unless run with `SV_TMUX_TESTS=1 cargo test -p sylvode-runner --test tmux_integration
//! -- --ignored`. The server uses a random socket name (`svtest-<random>`), a temporary
//! `TMUX_TMPDIR` and `-f /dev/null`, so it never meets the user's server, sessions or
//! configuration; it is killed when the test ends, on every path. Nothing here starts a CLI agent:
//! the sessions run `cat`.

#![cfg(unix)]
// Test helpers outside `#[test]` functions may fail loudly; this is test code only.
#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic, clippy::indexing_slicing)]
// The module under test is compiled into this test, and only part of it is used here.
#![allow(dead_code)]

#[path = "../src/runner/tmux.rs"]
mod tmux;

use std::path::PathBuf;
use std::thread::sleep;
use std::time::{Duration, Instant};

use tmux::{Call, SessionKey, SocketName, Tmux, TmuxError};

/// The server of one test run; killed on drop.
struct Server {
    tmux: Tmux,
    socket: String,
    dir: PathBuf,
}

impl Server {
    fn new() -> Self {
        let socket = format!("svtest-{}", &uuid::Uuid::new_v4().simple().to_string()[..12]);
        // Short on purpose: a unix socket path is limited to about 100 bytes on macOS.
        let dir = PathBuf::from("/tmp").join(format!("svt-{}", &uuid::Uuid::new_v4().simple().to_string()[..8]));
        std::fs::create_dir_all(&dir).expect("create the scratch directory");
        let tmux = Tmux::new(SocketName::new(&socket).expect("socket name"))
            .with_config_file("/dev/null")
            .with_env("TMUX_TMPDIR", &dir)
            .with_systemd(false);
        Self { tmux, socket, dir }
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        let kill = Call {
            args: vec!["-L".to_string(), self.socket.clone(), "kill-server".to_string()],
            stdin: None,
        };
        let _ = self.tmux.run(&kill);
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

fn key(value: &str) -> SessionKey {
    SessionKey::new(value).expect("key")
}

fn wait_for(what: &str, mut condition: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(10);
    while Instant::now() < deadline {
        if condition() {
            return;
        }
        sleep(Duration::from_millis(50));
    }
    panic!("timed out waiting for {what}");
}

#[test]
#[ignore = "needs a real tmux; run with SV_TMUX_TESTS=1 and --ignored"]
fn the_driver_against_a_real_tmux() {
    if std::env::var("SV_TMUX_TESTS").as_deref() != Ok("1") {
        return;
    }
    let server = Server::new();
    let t = &server.tmux;
    let work = server.dir.clone();
    let cat = vec!["cat".to_string()];

    let version = t.version().expect("tmux -V");
    assert!(version.starts_with("tmux "), "{version}");

    // No server yet: no session, no running server.
    assert_eq!(t.server_running(), Ok(false));
    assert_eq!(t.has_session(&key("ABC")), Ok(false));

    // `=AB` never reaches `ABC`.
    t.new_session(&key("ABC"), &work, 100, 30, &cat).expect("create ABC");
    assert_eq!(t.server_running(), Ok(true));
    assert_eq!(t.has_session(&key("ABC")), Ok(true));
    assert_eq!(t.has_session(&key("AB")), Ok(false));
    assert!(t.pane_info(&key("AB")).is_err(), "AB must not resolve to ABC");
    assert!(t.send_enter(&key("AB")).is_err());
    let info = t.pane_info(&key("ABC")).expect("ABC pane");
    assert!(!info.dead);
    assert_eq!(info.current_command, "cat");
    assert!(info.pid > 1);

    // An all-digit key is a name, not an index.
    t.new_session(&key("123"), &work, 100, 30, &cat).expect("create 123");
    assert_eq!(t.has_session(&key("123")), Ok(true));
    assert!(!t.pane_info(&key("123")).expect("123 pane").dead);
    assert_eq!(t.has_session(&key("12")), Ok(false));
    assert_eq!(t.has_session(&key("1")), Ok(false));

    // A duplicate name is refused instead of reaching the other session.
    assert!(matches!(
        t.new_session(&key("ABC"), &work, 100, 30, &cat),
        Err(TmuxError::Failed { .. })
    ));

    // A bracketed paste of several lines stays text: shell syntax is not interpreted, and the
    // lines arrive in the pane of the session that was addressed and in no other.
    let text = "[Sylvode task] id=1\nIssue: A-1 \"$(echo INJECTED) `echo INJECTED`; echo INJECTED\"\nlast line";
    t.paste_text(&key("ABC"), &uuid::Uuid::new_v4(), text).expect("paste");
    wait_for("the pasted text to show", || {
        t.capture_pane(&key("ABC"), None)
            .expect("capture")
            .contains("last line")
    });
    let shown = t.capture_pane(&key("ABC"), None).expect("capture");
    assert!(shown.contains("[Sylvode task] id=1"), "{shown}");
    assert!(shown.contains("$(echo INJECTED)"), "{shown}");
    assert!(!shown.contains("INJECTED\nINJECTED"), "{shown}");
    assert!(
        !t.capture_pane(&key("123"), None)
            .expect("capture")
            .contains("last line"),
        "the paste reached another session"
    );
    t.send_enter(&key("ABC")).expect("enter");
    wait_for("cat to echo the line", || {
        t.capture_pane(&key("ABC"), None)
            .expect("capture")
            .matches("last line")
            .count()
            >= 2
    });
    // The buffer was deleted by the paste, so a second paste starts from a clean state.
    t.paste_text(&key("ABC"), &uuid::Uuid::new_v4(), "second")
        .expect("second paste");

    // Text with an escape is refused before tmux is involved.
    let before = t.capture_pane(&key("ABC"), None).expect("capture");
    assert!(matches!(
        t.paste_text(&key("ABC"), &uuid::Uuid::new_v4(), "x\u{1b}[201~ y"),
        Err(TmuxError::Invalid(_))
    ));
    assert_eq!(t.capture_pane(&key("ABC"), None).expect("capture"), before);

    // Scrollback can be captured.
    assert!(t.capture_pane(&key("ABC"), Some(200)).is_ok());
    // Nobody is attached to the test server.
    assert_eq!(t.client_activity(&key("ABC")), Ok(Vec::new()));

    // A pane whose program ends stays as a dead pane, and respawn brings it back.
    let pid = t.pane_info(&key("123")).expect("pane").pid;
    let status = std::process::Command::new("kill")
        .arg(pid.to_string())
        .status()
        .expect("kill");
    assert!(status.success());
    wait_for("the pane to be dead", || t.pane_info(&key("123")).expect("pane").dead);
    assert_eq!(
        t.has_session(&key("123")),
        Ok(true),
        "the window must survive the program"
    );
    t.respawn_pane(&key("123"), &work, &cat).expect("respawn");
    wait_for("the pane to run again", || {
        !t.pane_info(&key("123")).expect("pane").dead
    });
    assert_ne!(t.pane_info(&key("123")).expect("pane").pid, pid);
}
