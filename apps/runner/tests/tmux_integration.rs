//! The tmux driver against a real tmux, on a server of its own.
//!
//! Ignored unless run with `SV_TMUX_TESTS=1 cargo test -p sylvode-runner --test tmux_integration
//! -- --ignored`. The server uses a random socket name (`svtest-<random>`), a short temporary
//! `TMUX_TMPDIR` under `/tmp` (a unix socket path is limited to about 100 bytes on macOS) and a
//! scratch `HOME` (the driver passes no `-f`, so tmux reads `~/.tmux.conf`: that is the test's own
//! file or none), so it never meets the user's default server, sessions or configuration; it is
//! killed when the test ends, on every path. Nothing here starts a CLI agent: the sessions run
//! `cat`.
//!
//! Needs `tmux` (3.2 or newer), `/usr/bin/env`, `sh`, `cat` and `kill` on the `PATH`; it passes on
//! Linux and macOS. A CI job runs it with tmux installed from the distribution's package.

#![cfg(unix)]
// Test helpers outside `#[test]` functions may fail loudly; this is test code only.
#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic, clippy::indexing_slicing)]

use sylvode_runner::tmux_for_tests as tmux;

use std::path::{Path, PathBuf};
use std::process::Command;
use std::thread::sleep;
use std::time::{Duration, Instant};

use tmux::{BracketedPasteConfirmed, Pane, PasteGuard, Refusal, Slot, SocketName, Tmux, TmuxError, TmuxName, WorkDir};

/// The server of one test; killed on drop.
struct Server {
    tmux: Tmux,
    socket: String,
    dir: PathBuf,
}

impl Server {
    fn new(config: Option<&Path>) -> Self {
        let socket = format!("svtest-{}", &uuid::Uuid::new_v4().simple().to_string()[..12]);
        let dir = PathBuf::from("/tmp").join(format!("svt-{}", &uuid::Uuid::new_v4().simple().to_string()[..8]));
        std::fs::create_dir_all(&dir).expect("create the scratch directory");
        // The driver passes no `-f`: tmux reads `~/.tmux.conf` of this scratch home, which is the
        // test's own configuration or nothing.
        let home = dir.join("home");
        std::fs::create_dir_all(&home).expect("create the scratch home");
        if let Some(config) = config {
            std::fs::copy(config, home.join(".tmux.conf")).expect("copy the configuration");
        }
        let tmux = Tmux::new(SocketName::new(&socket).expect("socket name"))
            .with_env("TMUX_TMPDIR", &dir)
            .with_env("HOME", &home)
            .with_env("XDG_CONFIG_HOME", home.join("xdg"))
            .with_systemd(false);
        Self { tmux, socket, dir }
    }

    /// A tmux command run directly on this server, for what the driver does not offer (entering
    /// copy mode, renaming a session). Always on the test's own socket.
    fn raw(&self, args: &[&str]) -> std::process::Output {
        assert!(self.socket.starts_with("svtest-"));
        Command::new("tmux")
            .args(["-L", &self.socket])
            .args(args)
            .env("TMUX_TMPDIR", &self.dir)
            .env("HOME", self.dir.join("home"))
            .env_remove("TMUX")
            .output()
            .expect("run tmux")
    }

    fn workdir(&self) -> WorkDir {
        WorkDir::new(&self.dir).expect("work dir")
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        // The driver has no way to end a server, on purpose: the test does it on its own socket.
        assert!(self.socket.starts_with("svtest-"));
        let _ = self.raw(&["kill-server"]);
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

/// The derived tmux session `<key>-claude`.
fn key(value: &str) -> TmuxName {
    TmuxName::derived(&Slot::project(value, "claude").expect("name")).expect("derived")
}

/// `new_session`, keeping only the pane.
trait Make {
    fn make(&self, name: &TmuxName, dir: &WorkDir, width: u16, height: u16, argv: &[String])
    -> Result<Pane, TmuxError>;
}

impl Make for Tmux {
    fn make(
        &self,
        name: &TmuxName,
        dir: &WorkDir,
        width: u16,
        height: u16,
        argv: &[String],
    ) -> Result<Pane, TmuxError> {
        self.new_session(name, dir, width, height, argv)
            .map(|created| created.pane)
    }
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

fn enabled() -> bool {
    std::env::var("SV_TMUX_TESTS").as_deref() == Ok("1")
}

fn strings(items: &[&str]) -> Vec<String> {
    items.iter().map(|s| (*s).to_string()).collect()
}

/// A program that turns bracketed paste on, as a CLI's input box does, then echoes its input
/// with control characters made visible (`^[` for ESC).
fn bracketed_cat() -> Vec<String> {
    strings(&["sh", "-c", "printf '\\033[?2004h'; exec cat -v"])
}

/// The guard for `cat`. tmux 3.7 and later report whether bracketed paste is on; for older
/// servers the test plays the session manager's readiness check, which it can do because it
/// knows what the program does.
fn guard_for(server: &Server, pane: &Pane, program_enables_bracketed_paste: bool) -> PasteGuard<'static> {
    let reports = server.tmux.pane_state(pane).expect("state").bracket_paste.is_some();
    PasteGuard {
        allowed_commands: &["cat"],
        bracketed_confirmed: (!reports && program_enables_bracketed_paste)
            .then(BracketedPasteConfirmed::from_readiness_check),
    }
}

fn push(server: &Server, pane: &Pane, text: &str, guard: &PasteGuard<'_>) -> Result<(), TmuxError> {
    server
        .tmux
        .push_message(pane, &uuid::Uuid::new_v4(), text, guard, Duration::from_millis(100))
}

#[test]
#[ignore = "needs a real tmux; run with SV_TMUX_TESTS=1 and --ignored"]
fn the_driver_against_a_real_tmux() {
    if !enabled() {
        return;
    }
    let server = Server::new(None);
    let t = &server.tmux;
    let work = server.workdir();
    let cat = strings(&["cat"]);

    let version = t.version().expect("tmux -V");
    assert!(version.starts_with("tmux "), "{version}");

    // No server yet: no session, no running server.
    assert_eq!(t.server_running(), Ok(false));
    assert!(t.list_panes(&key("ABC")).is_err());
    assert_eq!(t.has_session(&key("ABC")), Ok(false));

    // `=AB` never reaches `ABC`. A single-element command runs without a shell, as `cat`.
    let created = t.new_session(&key("ABC"), &work, 100, 30, &cat).expect("create ABC");
    let abc = created.pane;
    // The socket path new-session reports is the one the panes see in `TMUX`.
    let socket_path = created.socket_path.expect("this tmux reports its socket path");
    assert_eq!(t.socket_path(&abc), Ok(socket_path.clone()));
    assert!(socket_path.ends_with(&server.socket), "{socket_path}");
    assert_eq!(t.server_running(), Ok(true));
    assert_eq!(t.has_session(&key("ABC")), Ok(true));
    assert_eq!(t.has_session(&key("AB")), Ok(false));
    let info = t.pane_info(&abc).expect("ABC pane");
    assert!(!info.dead);
    assert_eq!(info.current_command, "cat");
    assert!(info.pid > 1);
    // A pane id that exists but belongs to another session is refused for the wrong key.
    let wrong = Pane {
        name: key("AB"),
        id: abc.id.clone(),
        pid: abc.pid,
    };
    assert_eq!(t.retry_enter(&wrong), Err(TmuxError::Refused(Refusal::NotOwned)));

    // An all-digit key is a name, not an index.
    let digits = t.make(&key("123"), &work, 100, 30, &cat).expect("create 123");
    assert_eq!(t.has_session(&key("123")), Ok(true));
    assert!(!t.pane_info(&digits).expect("123 pane").dead);
    assert_eq!(t.has_session(&key("12")), Ok(false));

    // A duplicate name is refused instead of reaching the other session.
    assert!(matches!(
        t.make(&key("ABC"), &work, 100, 30, &cat),
        Err(TmuxError::Failed { .. })
    ));

    // The pane is found again by listing the session: its id, its process, the foreground command.
    let panes = t.list_panes(&key("ABC")).expect("list");
    assert_eq!(panes.len(), 1);
    assert_eq!(
        (
            panes[0].id.clone(),
            panes[0].pid,
            panes[0].dead,
            panes[0].command.as_str()
        ),
        (abc.id.clone(), abc.pid, false, "cat")
    );
    assert_eq!(info.pid, abc.pid, "the pid new-session printed is the pane's process");
    assert!(t.list_panes(&key("AB")).is_err());
    // The runner sets no option on the server or the session it created.
    let options = server.raw(&["show-options", "-g"]);
    assert!(
        !String::from_utf8_lossy(&options.stdout).contains("remain-on-exit on"),
        "the runner set remain-on-exit"
    );
    // The pane id with another PID is not the recorded pane.
    let other_pid = Pane {
        pid: abc.pid + 1,
        ..abc.clone()
    };
    assert_eq!(t.retry_enter(&other_pid), Err(TmuxError::Refused(Refusal::NotOwned)));

    // Nobody is attached; scrollback can be captured.
    assert_eq!(t.client_activity(&key("ABC")), Ok(Vec::new()));
    assert!(t.capture_pane(&abc, Some(200)).is_ok());
}

#[test]
#[ignore = "needs a real tmux; run with SV_TMUX_TESTS=1 and --ignored"]
fn a_push_is_a_bracketed_paste_and_is_refused_where_that_cannot_be_promised() {
    if !enabled() {
        return;
    }
    let server = Server::new(None);
    let t = &server.tmux;
    let work = server.workdir();

    // A program that enabled bracketed paste: the text arrives between the markers, as one
    // paste, with shell syntax untouched.
    let good = t.make(&key("GOOD"), &work, 100, 30, &bracketed_cat()).expect("create");
    wait_for("the program to run", || {
        t.pane_info(&good).is_ok_and(|i| i.current_command == "cat")
    });
    // Give cat time to receive the escape that enables bracketed paste.
    wait_for("bracketed paste to be on", || {
        t.pane_state(&good).is_ok_and(|s| s.bracket_paste != Some(false))
    });
    let guard = guard_for(&server, &good, true);
    let text = "[Sylvode task] id=1\nIssue: A-1, title (untrusted): \"$(echo INJECTED) ; echo INJECTED\"\nlast line";
    push(&server, &good, text, &guard).expect("push");
    wait_for("the pasted text to show", || {
        t.capture_pane(&good, None)
            .expect("capture")
            .contains("last line^[[201~")
    });
    let shown = t.capture_pane(&good, None).expect("capture");
    assert!(shown.contains("^[[200~[Sylvode task] id=1"), "{shown}");
    assert!(shown.contains("$(echo INJECTED)"), "{shown}");
    assert!(!shown.contains("INJECTED\nINJECTED"), "{shown}");
    // Enter was pressed once: the line was echoed by cat.
    t.retry_enter(&good).expect("enter again");

    // A program without bracketed paste: every line would be submitted on its own, so the push is
    // refused. On tmux 3.7 and later the pane reports it; on older servers there is no
    // confirmation to give.
    let plain = t
        .make(&key("PLAIN"), &work, 100, 30, &strings(&["cat"]))
        .expect("create");
    wait_for("the program to run", || {
        t.pane_info(&plain).is_ok_and(|i| i.current_command == "cat")
    });
    let guard = guard_for(&server, &plain, false);
    let reports = t.pane_state(&plain).expect("state").bracket_paste.is_some();
    let want = if reports {
        Refusal::BracketedPasteOff
    } else {
        Refusal::BracketedPasteUnconfirmed
    };
    assert_eq!(push(&server, &plain, "one\ntwo", &guard), Err(TmuxError::Refused(want)));
    sleep(Duration::from_millis(300));
    assert!(
        !t.capture_pane(&plain, None).expect("capture").contains("one"),
        "text reached the pane"
    );

    // Copy mode would take the Enter: refused.
    assert!(server.raw(&["copy-mode", "-t", good.id.as_str()]).status.success());
    assert_eq!(t.pane_state(&good).expect("state").in_mode, 1);
    let guard = guard_for(&server, &good, true);
    assert_eq!(
        push(&server, &good, "text", &guard),
        Err(TmuxError::Refused(Refusal::PaneInMode))
    );
    assert_eq!(t.retry_enter(&good), Err(TmuxError::Refused(Refusal::PaneInMode)));
    assert!(server.raw(&["send-keys", "-t", good.id.as_str(), "q"]).status.success());
    wait_for("copy mode to end", || t.pane_state(&good).is_ok_and(|s| s.in_mode == 0));

    // A foreground program that is not the CLI: refused.
    let other = t
        .make(&key("OTHER"), &work, 100, 30, &strings(&["sleep", "300"]))
        .expect("create");
    wait_for("sleep to run", || {
        t.pane_info(&other).is_ok_and(|i| i.current_command == "sleep")
    });
    let guard = PasteGuard {
        allowed_commands: &["cat"],
        bracketed_confirmed: Some(BracketedPasteConfirmed::from_readiness_check()),
    };
    assert_eq!(
        push(&server, &other, "text", &guard),
        Err(TmuxError::Refused(Refusal::CommandNotAllowed))
    );

    // A pane that moved to another session name is not ours any more.
    assert!(
        server
            .raw(&["rename-session", "-t", "=GOOD-claude", "RENAMED"])
            .status
            .success()
    );
    let guard = guard_for(&server, &good, true);
    assert_eq!(
        push(&server, &good, "text", &guard),
        Err(TmuxError::Refused(Refusal::NotOwned))
    );
}

#[test]
#[ignore = "needs a real tmux; run with SV_TMUX_TESTS=1 and --ignored"]
fn a_dead_pane_is_detected_and_refused_and_an_exited_cli_ends_the_session() {
    if !enabled() {
        return;
    }
    let server = Server::new(None);
    let t = &server.tmux;
    let work = server.workdir();
    // The runner sets no `remain-on-exit`: when the CLI of a session it created exits, the session
    // is gone and the next delivery creates it again.
    let gone = t.make(&key("GONE"), &work, 100, 30, &bracketed_cat()).expect("create");
    wait_for("the program to run", || {
        t.pane_info(&gone).is_ok_and(|i| i.current_command == "cat")
    });
    assert!(
        Command::new("kill")
            .arg(gone.pid.to_string())
            .status()
            .expect("kill")
            .success()
    );
    wait_for("the session to end", || t.has_session(&key("GONE")) == Ok(false));
    assert!(t.list_panes(&key("GONE")).is_err());
    // An operator who keeps dead panes (their own `remain-on-exit`) is served as well: a dead pane
    // is seen as dead and nothing is typed into it. (The server ended with its last session, so a
    // keeper session holds it up while the option is set.)
    t.make(&key("KEEP"), &work, 100, 30, &strings(&["sleep", "300"]))
        .expect("create keeper");
    assert!(
        server
            .raw(&["set-option", "-g", "remain-on-exit", "on"])
            .status
            .success()
    );
    let pane = t.make(&key("DEAD"), &work, 100, 30, &bracketed_cat()).expect("create");
    wait_for("the program to run", || {
        t.pane_info(&pane).is_ok_and(|i| i.current_command == "cat")
    });
    let pid = pane.pid;
    assert!(
        Command::new("kill")
            .arg(pid.to_string())
            .status()
            .expect("kill")
            .success()
    );
    wait_for("the pane to be dead", || t.pane_info(&pane).expect("pane").dead);
    assert_eq!(t.has_session(&key("DEAD")), Ok(true));
    assert!(t.list_panes(&key("DEAD")).expect("list")[0].dead);
    let guard = guard_for(&server, &pane, true);
    assert_eq!(
        push(&server, &pane, "text", &guard),
        Err(TmuxError::Refused(Refusal::PaneDead))
    );
}

#[test]
#[ignore = "needs a real tmux; run with SV_TMUX_TESTS=1 and --ignored"]
fn the_pane_is_found_by_id_under_a_user_configuration_and_after_window_moves() {
    if !enabled() {
        return;
    }
    let scratch = PathBuf::from("/tmp").join(format!("svt-conf-{}", &uuid::Uuid::new_v4().simple().to_string()[..8]));
    std::fs::create_dir_all(&scratch).expect("scratch");
    let conf = scratch.join("tmux.conf");
    // The kind of configuration that broke addressing window 0 by index.
    std::fs::write(&conf, "set -g base-index 1\nset -g pane-base-index 1\n").expect("conf");
    let server = Server::new(Some(&conf));
    let t = &server.tmux;
    let work = server.workdir();
    let pane = t
        .make(&key("BASE"), &work, 100, 30, &bracketed_cat())
        .expect("create under base-index 1");
    wait_for("the program to run", || {
        t.pane_info(&pane).is_ok_and(|i| i.current_command == "cat")
    });
    // The window is 1, not 0, yet every operation addresses the pane.
    assert!(t.pane_info(&pane).is_ok());
    wait_for("bracketed paste to be on", || {
        t.pane_state(&pane).is_ok_and(|s| s.bracket_paste != Some(false))
    });
    let guard = guard_for(&server, &pane, true);
    push(&server, &pane, "after base-index", &guard).expect("push");
    // Moving the window does not redirect anything.
    assert!(
        server
            .raw(&["move-window", "-s", "=BASE-claude:1", "-t", "=BASE-claude:9"])
            .status
            .success()
    );
    push(&server, &pane, "after a window move", &guard).expect("push");
    wait_for("the second text", || {
        t.capture_pane(&pane, None)
            .expect("capture")
            .contains("after a window move")
    });
    let _ = std::fs::remove_dir_all(&scratch);
}

#[test]
#[ignore = "needs a real tmux; run with SV_TMUX_TESTS=1 and --ignored"]
#[allow(
    clippy::print_stderr,
    reason = "reports what this tmux version does with a format in -c"
)]
fn directories_commands_and_arguments_are_not_expanded_or_shelled() {
    if !enabled() {
        return;
    }
    let server = Server::new(None);
    let t = &server.tmux;

    // 1. A directory with `#` never reaches tmux: the type refuses it.
    let marker = server.dir.join("pwned");
    let evil = server.dir.join(format!("#(touch {})", marker.display()));
    std::fs::create_dir_all(&evil).expect("evil dir");
    let error = WorkDir::new(&evil).expect_err("refused");
    assert!(error.to_string().contains("'#'"), "{error}");

    // 2. What tmux itself does with it (documented in the module): the same directory handed to
    // tmux directly. The command inside `#(...)` must not run on the tested version; `#{...}` is
    // expanded, which is why `#` is refused.
    let raw = server.raw(&[
        "new-session",
        "-d",
        "-P",
        "-F",
        "#{pane_id}",
        "-s",
        "RAW",
        "-c",
        &evil.to_string_lossy(),
        "--",
        "cat",
        "-",
    ]);
    sleep(Duration::from_millis(1500));
    assert!(!marker.exists(), "tmux ran the command in a directory name: {raw:?}");
    let formatted = server.dir.join("A#{session_name}B");
    std::fs::create_dir_all(&formatted).expect("dir");
    let raw = server.raw(&[
        "new-session",
        "-d",
        "-P",
        "-F",
        "#{pane_id}",
        "-s",
        "RAW2",
        "-c",
        &formatted.to_string_lossy(),
        "--",
        "cat",
        "-",
    ]);
    let id = String::from_utf8_lossy(&raw.stdout).trim().to_string();
    let path = server.raw(&["display-message", "-p", "-t", &id, "#{pane_current_path}"]);
    eprintln!(
        "tmux {} expands -c as a format: requested {:?}, pane runs in {:?}",
        t.version().unwrap_or_default(),
        formatted,
        String::from_utf8_lossy(&path.stdout).trim()
    );

    // 3. The elements of the program's argument vector are not format-expanded.
    let out = server.dir.join("argv.out");
    let work = server.workdir();
    let script = format!("printf %s \"$0\" > '{}'; sleep 20", out.display());
    t.make(
        &key("ARGV"),
        &work,
        80,
        24,
        &strings(&["sh", "-c", &script, "#{session_name}"]),
    )
    .expect("create");
    wait_for("the program to write", || out.exists());
    sleep(Duration::from_millis(100));
    assert_eq!(std::fs::read_to_string(&out).expect("read"), "#{session_name}");

    // 4. A one-element command is not given to a shell.
    let shell_out = server.dir.join("shell.out");
    let single = format!("echo SHELLRAN > '{}'; sleep 20", shell_out.display());
    let _ = t.make(&key("ONE"), &work, 80, 24, &[single]);
    sleep(Duration::from_millis(1500));
    assert!(!shell_out.exists(), "a one-element command ran through a shell");
}
