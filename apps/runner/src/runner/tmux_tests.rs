#![allow(clippy::indexing_slicing, clippy::many_single_char_names)]

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use uuid::Uuid;

use super::{
    BracketedPasteConfirmed, BufferName, MAX_PUSH_BYTES, PaneId, PasteGuard, PushText, Refusal, SessionKey, SocketName,
    Tmux, TmuxError, WorkDir, exec_argv, one_line, parse_pane_state,
};
use super::{Pane, STATE_FORMAT};

struct Dir(PathBuf);

impl Dir {
    fn new() -> Self {
        let dir = std::env::temp_dir().join(format!("sylvode-tmux-test-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&dir).expect("create dir");
        Self(dir)
    }

    fn path(&self) -> &Path {
        &self.0
    }

    fn script(&self, name: &str, body: &str) -> PathBuf {
        let path = self.0.join(name);
        fs::write(&path, format!("#!/bin/sh\n{body}\n")).expect("write script");
        fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).expect("chmod");
        path
    }
}

impl Drop for Dir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn key(value: &str) -> SessionKey {
    SessionKey::new(value).expect("key")
}

fn pane(session: &str, id: &str) -> Pane {
    Pane {
        key: key(session),
        id: PaneId::new(id).expect("pane id"),
    }
}

fn dir(path: &str) -> WorkDir {
    WorkDir::new(Path::new(path)).expect("work dir")
}

fn driver(socket: &str) -> Tmux {
    Tmux::new(SocketName::new(socket).expect("socket")).with_systemd(false)
}

/// A driver on a socket name nobody else uses, so tests that run in parallel never share a
/// session lock.
fn fresh() -> Tmux {
    driver(&format!("t{}", &Uuid::new_v4().simple().to_string()[..12]))
}

fn strings(items: &[&str]) -> Vec<String> {
    items.iter().map(|s| (*s).to_string()).collect()
}

const TASK: &str = "0b9d6a52-3f7e-4c1a-9a55-1d2e3f4a5b6c";

fn task() -> Uuid {
    Uuid::parse_str(TASK).expect("uuid")
}

#[test]
fn socket_names_are_validated() {
    for good in ["", "sylvode", "a", "A_b-9", &"x".repeat(32)] {
        assert!(SocketName::new(good).is_ok(), "{good:?}");
    }
    for bad in ["a b", "a/b", "../x", "a.b", "a:b", "é", "a\n", "$(id)", &"x".repeat(33)] {
        assert!(matches!(SocketName::new(bad), Err(TmuxError::Invalid(_))), "{bad:?}");
    }
}

#[test]
fn session_keys_and_pane_ids_are_validated() {
    for good in ["A", "SVWL", "123", "AB12", &"K".repeat(32)] {
        assert!(SessionKey::new(good).is_ok(), "{good}");
    }
    for bad in [
        "",
        "svwl",
        "SV WL",
        "SV:WL",
        "SV.WL",
        "SV-WL",
        "=SV",
        "SV\n",
        "%1",
        "$1",
        "@1",
        "SVWL:0",
        "*",
        "SV*",
        "é",
        &"K".repeat(33),
    ] {
        assert!(matches!(SessionKey::new(bad), Err(TmuxError::Invalid(_))), "{bad:?}");
    }
    assert_eq!(key("SVWL").session_target(), "=SVWL");
    assert_eq!(key("123").session_target(), "=123");
    for good in ["%0", "%12", "%123456789"] {
        assert!(PaneId::new(good).is_ok(), "{good}");
    }
    for bad in [
        "",
        "%",
        "0",
        "%a",
        "%1 ",
        "%1\n",
        "%1234567890",
        "$1",
        "@1",
        "=A",
        "%-1",
        "%１",
    ] {
        assert!(matches!(PaneId::new(bad), Err(TmuxError::Invalid(_))), "{bad:?}");
    }
}

#[test]
fn the_working_directory_must_be_plain_and_absolute() {
    assert!(WorkDir::new(Path::new("/work/my project/svwl")).is_ok());
    for bad in ["relative", "", "-x", "./a", "/a\nb", "/a\u{1b}b", "/a\u{7f}"] {
        assert!(
            matches!(WorkDir::new(Path::new(bad)), Err(TmuxError::Invalid(_))),
            "{bad:?}"
        );
    }
    // `#` would be read by tmux as a format: `#{...}` substitutes and `#(...)` runs a command.
    for evil in ["/srv/#(curl x|sh)", "/srv/a#{session_name}b", "/srv/#", "/srv/dir#1"] {
        let error = WorkDir::new(Path::new(evil)).expect_err("refused");
        assert!(error.to_string().contains("'#'"), "{error}");
    }
}

#[test]
fn buffer_names_are_per_task() {
    assert_eq!(BufferName::for_task(&task()).as_str(), format!("sv-{TASK}"));
    assert_ne!(
        BufferName::for_task(&task()).as_str(),
        BufferName::for_task(&Uuid::new_v4()).as_str()
    );
}

#[test]
fn every_call_is_an_argument_vector_with_the_socket_the_empty_config_and_an_exact_target() {
    let t = driver("sylvode");
    let k = key("SVWL");
    let p = PaneId::new("%7").expect("id");
    let d = dir("/work/svwl");
    let argv = strings(&["claude", "--permission-mode", "dontAsk"]);
    let g = ["-L", "sylvode", "-f", "/dev/null"];
    let with = |rest: &[&str]| -> Vec<String> { g.iter().chain(rest.iter()).map(|s| (*s).to_string()).collect() };
    assert_eq!(Tmux::version_call().args, strings(&["-V"]));
    assert_eq!(t.has_session_call(&k).args, with(&["has-session", "-t", "=SVWL"]));
    assert_eq!(t.list_sessions_call().args, with(&["list-sessions"]));
    assert_eq!(
        t.new_session_call(&k, &d, 200, 50, &argv).expect("call").args,
        with(&[
            "new-session",
            "-d",
            "-P",
            "-F",
            "#{pane_id}",
            "-s",
            "SVWL",
            "-n",
            "cli",
            "-x",
            "200",
            "-y",
            "50",
            "-c",
            "/work/svwl",
            "--",
            "claude",
            "--permission-mode",
            "dontAsk"
        ])
    );
    assert_eq!(
        t.remain_on_exit_call(&p).args,
        with(&["set-option", "-w", "-t", "%7", "remain-on-exit", "on"])
    );
    assert_eq!(
        t.respawn_pane_call(&p, &d, &argv).expect("call").args,
        with(&[
            "respawn-pane",
            "-k",
            "-t",
            "%7",
            "-c",
            "/work/svwl",
            "--",
            "claude",
            "--permission-mode",
            "dontAsk"
        ])
    );
    assert_eq!(
        t.state_call(&p).args,
        with(&["display-message", "-p", "-t", "%7", STATE_FORMAT])
    );
    assert_eq!(
        STATE_FORMAT,
        "#{pane_dead} #{pane_in_mode} #{bracket_paste_flag} #{pane_pid} #{session_name}"
    );
    assert_eq!(
        t.field_call(&p, "#{pane_current_command}").args,
        with(&["display-message", "-p", "-t", "%7", "#{pane_current_command}"])
    );
    assert_eq!(
        t.capture_pane_call(&p, None).args,
        with(&["capture-pane", "-p", "-t", "%7"])
    );
    assert_eq!(
        t.capture_pane_call(&p, Some(200)).args,
        with(&["capture-pane", "-p", "-t", "%7", "-S", "-200"])
    );
    assert_eq!(
        t.list_clients_call(&k).args,
        with(&["list-clients", "-F", "#{client_activity}", "-t", "=SVWL"])
    );
    assert_eq!(
        t.adopt_call(&k).args,
        with(&["list-panes", "-t", "=SVWL:0", "-F", "#{pane_id}"])
    );
    let buffer = BufferName::for_task(&task());
    assert_eq!(
        t.paste_buffer_call(&buffer, &p).args,
        with(&["paste-buffer", "-p", "-d", "-b", &format!("sv-{TASK}"), "-t", "%7"])
    );
    assert_eq!(
        t.delete_buffer_call(&buffer).args,
        with(&["delete-buffer", "-b", &format!("sv-{TASK}")])
    );
    assert_eq!(t.send_enter_call(&p).args, with(&["send-keys", "-t", "%7", "Enter"]));
}

#[test]
fn no_call_but_load_buffer_carries_stdin_and_no_argument_carries_the_text() {
    let t = driver("sylvode");
    let text = "[Sylvode task] id=x\nIssue: A-1 \"; rm -rf ~ #\" $(id) `id`";
    let buffer = BufferName::for_task(&task());
    let call = t.load_buffer_call(&buffer, &PushText::new(text).expect("text"));
    assert_eq!(
        call.args,
        strings(&[
            "-L",
            "sylvode",
            "-f",
            "/dev/null",
            "load-buffer",
            "-b",
            &format!("sv-{TASK}"),
            "-"
        ])
    );
    assert_eq!(call.stdin.as_deref(), Some(text.as_bytes()));
    let p = PaneId::new("%1").expect("id");
    for other in [
        t.paste_buffer_call(&buffer, &p),
        t.send_enter_call(&p),
        t.has_session_call(&key("A")),
        t.state_call(&p),
        t.delete_buffer_call(&buffer),
    ] {
        assert_eq!(other.stdin, None);
        assert!(
            other
                .args
                .iter()
                .all(|arg| !arg.contains("Sylvode") && !arg.contains("rm -rf"))
        );
    }
}

#[test]
fn an_empty_socket_name_uses_the_default_server_and_omits_the_flag() {
    assert_eq!(
        driver("").has_session_call(&key("A")).args,
        strings(&["-f", "/dev/null", "has-session", "-t", "=A"])
    );
}

#[test]
fn a_config_file_can_be_given_for_tests() {
    let t = driver("s").with_config_file("/tmp/x.conf");
    assert_eq!(
        t.list_sessions_call().args,
        strings(&["-L", "s", "-f", "/tmp/x.conf", "list-sessions"])
    );
}

#[test]
fn a_single_argument_command_is_never_given_to_a_shell() {
    assert_eq!(
        exec_argv(&strings(&["cat"])),
        Ok(strings(&["/usr/bin/env", "--", "cat"]))
    );
    assert_eq!(
        exec_argv(&strings(&["sh -c 'x; y'"])),
        Ok(strings(&["/usr/bin/env", "--", "sh -c 'x; y'"]))
    );
    assert_eq!(exec_argv(&strings(&["a", "b"])), Ok(strings(&["a", "b"])));
    assert!(exec_argv(&[]).is_err());
    assert!(exec_argv(&strings(&["a", ""])).is_err());
    let t = driver("s");
    let call = t
        .new_session_call(&key("A"), &dir("/w"), 80, 24, &strings(&["cat"]))
        .expect("call");
    assert_eq!(&call.args[call.args.len() - 4..], ["--", "/usr/bin/env", "--", "cat"]);
    // An option-looking argument stays one argument after `--`.
    let call = t
        .new_session_call(&key("A"), &dir("/w"), 80, 24, &strings(&["cli", "-c", "$(id)"]))
        .expect("call");
    assert_eq!(&call.args[call.args.len() - 4..], ["--", "cli", "-c", "$(id)"]);
}

#[test]
fn pushed_text_refuses_unsafe_characters_and_long_text() {
    for ok in [
        "a",
        "line one\nline two",
        "日本語 🎉 ＠ＸＹ",
        "[Sylvode task]\n1. Call x.",
    ] {
        assert!(PushText::new(ok).is_ok(), "{ok:?}");
    }
    for bad in [
        "",
        "a\u{1b}[31m",
        "a\u{1b}",
        "a\rb",
        "a\tb",
        "a\u{0}b",
        "a\u{7f}",
        "a\u{85}b",
        "a\u{9b}31m",
        "a\u{8}",
        "\u{1b}[201~",
        "a\u{2028}b",
        "a\u{2029}b",
        "a\u{202e}b",
        "a\u{2066}b",
        "a\u{e0041}b",
        "a\u{feff}b",
        "a\u{200b}b",
        "a\u{200f}b",
        "a\u{e000}b",
        "a\u{fdd0}b",
    ] {
        assert!(matches!(PushText::new(bad), Err(TmuxError::Invalid(_))), "{bad:?}");
    }
    assert!(PushText::new(&"x".repeat(MAX_PUSH_BYTES)).is_ok());
    assert!(PushText::new(&"x".repeat(MAX_PUSH_BYTES + 1)).is_err());
    let error = PushText::new("secret\u{1b}").expect_err("refused").to_string();
    assert!(!error.contains("secret"), "{error}");
}

#[test]
fn pane_state_parses_every_field() {
    let state = parse_pane_state("0 0 1 4242 SVWL").expect("state");
    assert_eq!(
        (
            state.dead,
            state.in_mode,
            state.bracket_paste,
            state.pid,
            state.session_name.as_str()
        ),
        (false, 0, Some(true), 4242, "SVWL")
    );
    // Before tmux 3.7 the bracketed-paste field is empty.
    assert_eq!(parse_pane_state("0 0  7 A").map(|s| s.bracket_paste), Some(None));
    assert_eq!(
        parse_pane_state("1 2 0 7 name with spaces").map(|s| (s.dead, s.in_mode, s.session_name)),
        Some((true, 2, "name with spaces".to_string()))
    );
    for bad in [
        "",
        "2 0 1 7 A",
        "0 x 1 7 A",
        "0 0 2 7 A",
        "0 0 1 x A",
        "0 0 1 7",
        "0 0 1",
    ] {
        assert_eq!(parse_pane_state(bad), None, "{bad:?}");
    }
}

#[test]
fn only_a_single_clean_line_is_an_answer() {
    assert_eq!(one_line("a b\n"), Some("a b"));
    assert_eq!(one_line("\n"), Some(""));
    for bad in ["", "no newline", "a\nb\n", "a\r\n", "a\nb"] {
        assert_eq!(one_line(bad), None, "{bad:?}");
    }
}

/// A fake `tmux` that records every call (one argument per line, calls separated by `---`) and
/// the standard input of `load-buffer`, and answers `answers` for everything else; `answers` is
/// a `case "$*"` body.
fn fake_tmux(dir: &Dir, answers: &str) -> (PathBuf, PathBuf) {
    let log = dir.path().join("log");
    let stdin = dir.path().join("stdin");
    let body = format!(
        "echo --- >> '{log}'\nfor a in \"$@\"; do echo \"$a\" >> '{log}'; done\n\
         for a in \"$@\"; do [ \"$a\" = load-buffer ] && cat > '{stdin}'; done\n{answers}",
        log = log.display(),
        stdin = stdin.display(),
    );
    (dir.script("tmux", &body), log)
}

/// Answers for a healthy bracketed-paste CLI pane of session SVWL.
const HEALTHY: &str = "case \"$*\" in\n\
    *new-session*) echo %7;;\n\
    *STATE*) echo '0 0 1 4242 SVWL';;\n\
    *pane_current_command*) echo claude.exe;;\n\
    *pane_current_path*) echo '/work/my project';;\n\
    esac\nexit 0";

fn healthy_answers(state: &str, command: &str) -> String {
    format!(
        "case \"$*\" in\n\
         *new-session*) echo %7;;\n\
         *pane_dead*) echo '{state}';;\n\
         *pane_current_command*) echo '{command}';;\n\
         *pane_current_path*) echo '/work/my project';;\n\
         esac\nexit 0"
    )
}

fn calls(log: &Path) -> Vec<Vec<String>> {
    let text = fs::read_to_string(log).unwrap_or_default();
    let mut out: Vec<Vec<String>> = Vec::new();
    for line in text.lines() {
        if line == "---" {
            out.push(Vec::new());
        } else if let Some(last) = out.last_mut() {
            last.push(line.to_string());
        }
    }
    out
}

/// The tmux subcommand of each recorded call.
fn names(log: &Path) -> Vec<String> {
    calls(log)
        .iter()
        .map(|c| c.get(4).cloned().unwrap_or_default())
        .collect()
}

const GUARD: PasteGuard<'static> = PasteGuard {
    allowed_commands: &["claude.exe", "claude"],
    bracketed_confirmed: None,
};

fn push(t: &Tmux, text: &str) -> Result<(), TmuxError> {
    t.push_message(&pane("SVWL", "%7"), &task(), text, &GUARD, Duration::ZERO)
}

#[test]
fn a_push_checks_the_pane_loads_through_stdin_pastes_and_presses_enter() {
    let dir = Dir::new();
    let (program, log) = fake_tmux(&dir, &healthy_answers("0 0 1 4242 SVWL", "claude.exe"));
    let text = "[Sylvode task] id=1\nIssue: A-1";
    push(&fresh().with_program(&program), text).expect("pushed");
    assert_eq!(
        names(&log),
        [
            "display-message",
            "display-message",
            "load-buffer",
            "paste-buffer",
            "send-keys"
        ]
    );
    let calls = calls(&log);
    assert_eq!(
        calls[2][4..],
        strings(&["load-buffer", "-b", &format!("sv-{TASK}"), "-"])
    );
    assert_eq!(
        calls[3][4..],
        strings(&["paste-buffer", "-p", "-d", "-b", &format!("sv-{TASK}"), "-t", "%7"])
    );
    assert_eq!(calls[4][4..], strings(&["send-keys", "-t", "%7", "Enter"]));
    assert_eq!(fs::read_to_string(dir.path().join("stdin")).expect("stdin"), text);
}

fn refused(state: &str, command: &str, guard: &PasteGuard<'_>) -> Result<(), TmuxError> {
    let dir = Dir::new();
    let (program, log) = fake_tmux(&dir, &healthy_answers(state, command));
    let result =
        fresh()
            .with_program(&program)
            .push_message(&pane("SVWL", "%7"), &task(), "text", guard, Duration::ZERO);
    // Whatever the reason, a refused push never loaded or pasted anything.
    if result.is_err() {
        let names = names(&log);
        assert!(
            !names
                .iter()
                .any(|n| n == "load-buffer" || n == "paste-buffer" || n == "send-keys"),
            "{names:?}"
        );
    }
    result
}

#[test]
fn the_gate_refuses_every_pane_that_cannot_take_a_bracketed_paste() {
    assert_eq!(refused("0 0 1 1 SVWL", "claude.exe", &GUARD), Ok(()));
    assert_eq!(
        refused("1 0 1 1 SVWL", "claude.exe", &GUARD),
        Err(TmuxError::Refused(Refusal::PaneDead))
    );
    assert_eq!(
        refused("0 1 1 1 SVWL", "claude.exe", &GUARD),
        Err(TmuxError::Refused(Refusal::PaneInMode))
    );
    assert_eq!(
        refused("0 0 0 1 SVWL", "claude.exe", &GUARD),
        Err(TmuxError::Refused(Refusal::BracketedPasteOff))
    );
    assert_eq!(
        refused("0 0 1 1 OTHER", "claude.exe", &GUARD),
        Err(TmuxError::Refused(Refusal::NotOwned))
    );
    for shell in ["zsh", "bash", "sh", "vim", "claude.exe x", ""] {
        assert_eq!(
            refused("0 0 1 1 SVWL", shell, &GUARD),
            Err(TmuxError::Refused(Refusal::CommandNotAllowed)),
            "{shell:?}"
        );
    }
}

#[test]
fn a_tmux_that_cannot_report_bracketed_paste_needs_the_confirmation() {
    // Before 3.7 the field is empty.
    assert_eq!(
        refused("0 0  1 SVWL", "claude.exe", &GUARD),
        Err(TmuxError::Refused(Refusal::BracketedPasteUnconfirmed))
    );
    let confirmed = PasteGuard {
        bracketed_confirmed: Some(BracketedPasteConfirmed::from_readiness_check()),
        ..GUARD
    };
    assert_eq!(refused("0 0  1 SVWL", "claude.exe", &confirmed), Ok(()));
    // A confirmation never overrides a pane that reports bracketed paste as off, nor any other check.
    assert_eq!(
        refused("0 0 0 1 SVWL", "claude.exe", &confirmed),
        Err(TmuxError::Refused(Refusal::BracketedPasteOff))
    );
    assert_eq!(
        refused("1 0  1 SVWL", "claude.exe", &confirmed),
        Err(TmuxError::Refused(Refusal::PaneDead))
    );
    assert_eq!(
        refused("0 0  1 SVWL", "zsh", &confirmed),
        Err(TmuxError::Refused(Refusal::CommandNotAllowed))
    );
}

#[test]
fn text_that_is_refused_never_reaches_tmux() {
    let dir = Dir::new();
    let (program, log) = fake_tmux(&dir, &healthy_answers("0 0 1 1 SVWL", "claude.exe"));
    for bad in [
        "hello \u{1b}[201~ /exit",
        "a\u{2028}b",
        "",
        &"y".repeat(MAX_PUSH_BYTES + 1),
    ] {
        let error = push(&fresh().with_program(&program), bad).expect_err("refused");
        assert!(matches!(error, TmuxError::Invalid(_)));
    }
    assert!(!log.exists(), "tmux was run");
}

#[test]
fn a_failed_load_or_paste_deletes_the_buffer() {
    let dir = Dir::new();
    let healthy = healthy_answers("0 0 1 1 SVWL", "claude.exe");
    let (program, log) = fake_tmux(
        &dir,
        &format!("for a in \"$@\"; do [ \"$a\" = load-buffer ] && exit 1; done\n{healthy}"),
    );
    let error = push(&fresh().with_program(&program), "text").expect_err("fails");
    assert!(matches!(error, TmuxError::Failed { code: Some(1), .. }));
    assert_eq!(names(&log).last().map(String::as_str), Some("delete-buffer"));

    let dir = Dir::new();
    let (program, log) = fake_tmux(
        &dir,
        &format!("for a in \"$@\"; do [ \"$a\" = paste-buffer ] && exit 1; done\n{healthy}"),
    );
    let error = push(&fresh().with_program(&program), "text").expect_err("fails");
    assert!(matches!(error, TmuxError::Failed { code: Some(1), .. }));
    let calls = calls(&log);
    assert_eq!(
        calls.last().map(|c| c[4..].to_vec()),
        Some(strings(&["delete-buffer", "-b", &format!("sv-{TASK}")]))
    );
    assert!(!names(&log).iter().any(|n| n == "send-keys"));
}

#[test]
fn a_paste_that_times_out_is_reported_as_delivery_unknown_and_not_retried() {
    let dir = Dir::new();
    let healthy = healthy_answers("0 0 1 1 SVWL", "claude.exe");
    let (program, log) = fake_tmux(
        &dir,
        &format!("for a in \"$@\"; do [ \"$a\" = paste-buffer ] && exec /bin/sleep 30; done\n{healthy}"),
    );
    let started = Instant::now();
    let error = push(
        &fresh().with_program(&program).with_timeout(Duration::from_secs(5)),
        "text",
    )
    .expect_err("times out");
    assert_eq!(error, TmuxError::DeliveryUnknown, "calls: {:?}", calls(&log));
    assert!(started.elapsed() < Duration::from_secs(10));
    let names = names(&log);
    assert_eq!(names.iter().filter(|n| *n == "paste-buffer").count(), 1);
    assert_eq!(names.last().map(String::as_str), Some("delete-buffer"));
    assert!(!names.iter().any(|n| n == "send-keys"));
}

#[test]
fn an_enter_that_fails_after_the_paste_says_so() {
    let dir = Dir::new();
    let healthy = healthy_answers("0 0 1 1 SVWL", "claude.exe");
    let (program, _) = fake_tmux(
        &dir,
        &format!("for a in \"$@\"; do [ \"$a\" = send-keys ] && exit 1; done\n{healthy}"),
    );
    assert_eq!(
        push(&fresh().with_program(&program), "text"),
        Err(TmuxError::PastedNotSubmitted)
    );
}

#[test]
fn pushes_to_one_session_never_interleave() {
    let dir = Dir::new();
    let healthy = healthy_answers("0 0 1 1 SVWL", "claude.exe");
    // Pasting is slow enough for a second push to start meanwhile.
    let (program, log) = fake_tmux(
        &dir,
        &format!("for a in \"$@\"; do [ \"$a\" = paste-buffer ] && /bin/sleep 0.3; done\n{healthy}"),
    );
    let t = fresh().with_program(&program);
    let handles: Vec<_> = (0..3)
        .map(|_| {
            let t = t.clone();
            std::thread::spawn(move || {
                t.push_message(&pane("SVWL", "%7"), &Uuid::new_v4(), "text", &GUARD, Duration::ZERO)
            })
        })
        .collect();
    for handle in handles {
        handle.join().expect("thread").expect("pushed");
    }
    let names = names(&log);
    let one = [
        "display-message",
        "display-message",
        "load-buffer",
        "paste-buffer",
        "send-keys",
    ];
    let expected: Vec<&str> = std::iter::repeat_n(one, 3).flatten().collect();
    assert_eq!(names, expected);
}

#[test]
fn enter_can_be_retried_only_on_a_live_owned_pane() {
    let dir = Dir::new();
    let (program, log) = fake_tmux(&dir, &healthy_answers("0 0 1 1 SVWL", "claude.exe"));
    fresh()
        .with_program(&program)
        .retry_enter(&pane("SVWL", "%7"))
        .expect("sent");
    assert_eq!(names(&log), ["display-message", "send-keys"]);
    for (state, want) in [
        ("1 0 1 1 SVWL", Refusal::PaneDead),
        ("0 1 1 1 SVWL", Refusal::PaneInMode),
        ("0 0 1 1 OTHER", Refusal::NotOwned),
    ] {
        let dir = Dir::new();
        let (program, log) = fake_tmux(&dir, &healthy_answers(state, "claude.exe"));
        assert_eq!(
            fresh().with_program(&program).retry_enter(&pane("SVWL", "%7")),
            Err(TmuxError::Refused(want))
        );
        assert!(!names(&log).iter().any(|n| n == "send-keys"));
    }
}

#[test]
fn has_session_maps_exit_statuses() {
    let dir = Dir::new();
    for (code, want) in [(0, Ok(true)), (1, Ok(false))] {
        let (program, _) = fake_tmux(&dir, &format!("exit {code}"));
        assert_eq!(driver("s").with_program(&program).has_session(&key("A")), want);
    }
    let (program, _) = fake_tmux(&dir, "echo boom >&2; exit 2");
    assert_eq!(
        driver("s").with_program(&program).has_session(&key("A")),
        Err(TmuxError::Failed {
            code: Some(2),
            stderr: "boom".to_string()
        })
    );
}

#[test]
fn new_session_returns_the_pane_id_and_keeps_dead_panes() {
    let dir = Dir::new();
    let (program, log) = fake_tmux(&dir, &healthy_answers("0 0 1 1 SVWL", "claude.exe"));
    let pane = driver("sylvode")
        .with_program(&program)
        .new_session(
            &key("SVWL"),
            &dir_of("/work/svwl"),
            200,
            50,
            &strings(&["claude", "--x"]),
        )
        .expect("created");
    assert_eq!(
        pane,
        super::Pane {
            key: key("SVWL"),
            id: PaneId::new("%7").expect("id")
        }
    );
    let calls = calls(&log);
    assert_eq!(calls.len(), 2);
    assert_eq!(calls[0][4], "new-session");
    assert_eq!(
        calls[1][4..],
        strings(&["set-option", "-w", "-t", "%7", "remain-on-exit", "on"])
    );
    // An answer that is not a pane id is an error.
    let (bad, _) = fake_tmux(&dir, "echo 'oops'; exit 0");
    assert!(
        driver("s")
            .with_program(&bad)
            .new_session(&key("A"), &dir_of("/w"), 80, 24, &strings(&["a", "b"]))
            .is_err()
    );
}

fn dir_of(path: &str) -> WorkDir {
    dir(path)
}

#[test]
fn a_directory_with_a_hash_never_reaches_tmux() {
    let dir = Dir::new();
    let (program, log) = fake_tmux(&dir, "exit 0");
    assert!(WorkDir::new(Path::new("/srv/#(touch /tmp/pwned)")).is_err());
    assert!(!log.exists());
    let _ = program;
}

#[test]
fn pane_information_comes_from_separate_calls() {
    let dir = Dir::new();
    let (program, log) = fake_tmux(&dir, &healthy_answers("0 0 1 99 SVWL", "claude.exe"));
    let info = driver("s")
        .with_program(&program)
        .pane_info(&pane("SVWL", "%7"))
        .expect("info");
    assert_eq!(
        (
            info.dead,
            info.pid,
            info.current_command.as_str(),
            info.current_path.as_str()
        ),
        (false, 99, "claude.exe", "/work/my project")
    );
    assert_eq!(calls(&log).len(), 3);
}

#[test]
fn a_command_name_with_a_space_or_a_newline_cannot_shift_a_field() {
    let dir = Dir::new();
    let (program, _) = fake_tmux(&dir, &healthy_answers("0 0 1 99 SVWL", "claude.exe /fake/path"));
    let info = driver("s")
        .with_program(&program)
        .pane_info(&pane("SVWL", "%7"))
        .expect("info");
    assert_eq!(info.current_command, "claude.exe /fake/path");
    assert_eq!(info.current_path, "/work/my project");
    let (program, _) = fake_tmux(
        &dir,
        "case \"$*\" in\n*pane_dead*) echo '0 0 1 99 SVWL';;\n*pane_current_command*) printf 'a\\nb\\n';;\n*) echo /x;;\nesac",
    );
    assert!(
        driver("s")
            .with_program(&program)
            .pane_info(&pane("SVWL", "%7"))
            .is_err()
    );
}

#[test]
fn adopting_reads_the_first_pane_of_window_zero() {
    let dir = Dir::new();
    let (program, _) = fake_tmux(&dir, "printf '%%3\\n%%4\\n'");
    assert_eq!(
        driver("s").with_program(&program).adopt_pane(&key("A")),
        Ok(super::Pane {
            key: key("A"),
            id: PaneId::new("%3").expect("id")
        })
    );
    let (program, _) = fake_tmux(&dir, "echo 'junk'");
    assert!(driver("s").with_program(&program).adopt_pane(&key("A")).is_err());
}

#[test]
fn client_activity_lists_timestamps() {
    let dir = Dir::new();
    let (program, _) = fake_tmux(&dir, "printf '1700000000\\n1700000100\\n'");
    assert_eq!(
        driver("s").with_program(&program).client_activity(&key("A")),
        Ok(vec![1_700_000_000, 1_700_000_100])
    );
}

#[test]
fn tmux_gets_an_allow_list_of_variables_and_never_the_agent_ones() {
    let dir = Dir::new();
    let env_out = dir.path().join("env");
    let program = dir.script("tmux", &format!("/usr/bin/env > '{}'\nexit 0", env_out.display()));
    driver("s")
        .with_program(&program)
        .with_env("CLAUDECODE", "1")
        .with_env("CLAUDE_CODE_ENTRYPOINT", "cli")
        .with_env("TMUX", "/tmp/x,1,0")
        .with_env("TMUX_TMPDIR", "/tmp")
        .with_env("EXTRA_FROM_CALLER", "kept")
        .server_running()
        .expect("ran");
    let text = fs::read_to_string(&env_out).expect("env");
    let names: Vec<&str> = text.lines().filter_map(|l| l.split_once('=').map(|(n, _)| n)).collect();
    for gone in ["CLAUDECODE", "CLAUDE_CODE_ENTRYPOINT", "TMUX"] {
        assert!(!names.contains(&gone), "{gone} leaked");
    }
    // The test process runs under cargo, which sets many variables; none of them may pass.
    assert!(
        !names.iter().any(|n| n.starts_with("CARGO") || n.starts_with("RUST")),
        "{names:?}"
    );
    assert!(names.contains(&"PATH"));
    assert!(names.contains(&"TMUX_TMPDIR"));
    assert!(names.contains(&"EXTRA_FROM_CALLER"));
    for name in &names {
        let allowed = super::ENV_ALLOWED.contains(name)
            || name.starts_with("LC_")
            || *name == "EXTRA_FROM_CALLER"
            || *name == "PWD"
            || *name == "SHLVL"
            || *name == "_"
            || *name == "OLDPWD";
        assert!(allowed, "{name} is not on the allow-list");
    }
}

#[test]
fn a_new_server_under_systemd_is_started_in_its_own_scope() {
    let dir = Dir::new();
    let (program, log) = fake_tmux(
        &dir,
        "case \"$*\" in *list-sessions*) exit 1;; *new-session*) echo %2;; esac\nexit 0",
    );
    let scope_log = dir.path().join("scope");
    let systemd_run = dir.script(
        "systemd-run",
        &format!(
            "for a in \"$@\"; do echo \"$a\"; done > '{}'\nshift 3\nexec \"$@\"",
            scope_log.display()
        ),
    );
    driver("sylvode")
        .with_program(&program)
        .with_systemd(true)
        .with_systemd_run(&systemd_run)
        .new_session(&key("SVWL"), &dir_of("/w"), 80, 24, &strings(&["claude", "x"]))
        .expect("created");
    let scope: Vec<String> = fs::read_to_string(&scope_log)
        .expect("scope")
        .lines()
        .map(str::to_string)
        .collect();
    assert_eq!(
        &scope[..4],
        [
            "--user",
            "--scope",
            "--unit=sylvode-tmux-sylvode",
            program.to_str().expect("utf-8")
        ]
    );
    assert_eq!(names(&log), ["list-sessions", "new-session", "set-option"]);
}

#[test]
fn a_running_server_is_not_wrapped_again() {
    let dir = Dir::new();
    let (program, log) = fake_tmux(&dir, "case \"$*\" in *new-session*) echo %2;; esac\nexit 0");
    let scope_log = dir.path().join("scope");
    let systemd_run = dir.script(
        "systemd-run",
        &format!("echo used > '{}'\nshift 3\nexec \"$@\"", scope_log.display()),
    );
    driver("sylvode")
        .with_program(&program)
        .with_systemd(true)
        .with_systemd_run(&systemd_run)
        .new_session(&key("SVWL"), &dir_of("/w"), 80, 24, &strings(&["claude", "x"]))
        .expect("created");
    assert!(!scope_log.exists());
    assert_eq!(calls(&log).len(), 3);
}

#[test]
fn the_default_server_is_never_killed() {
    let dir = Dir::new();
    let (program, log) = fake_tmux(&dir, "exit 0");
    assert!(matches!(
        driver("").with_program(&program).kill_server(),
        Err(TmuxError::Invalid(_))
    ));
    assert!(!log.exists());
    driver("svtest-x").with_program(&program).kill_server().expect("killed");
    assert_eq!(
        calls(&log)[0][..],
        strings(&["-L", "svtest-x", "-f", "/dev/null", "kill-server"])
    );
}

#[test]
fn a_slow_tmux_is_killed_at_the_time_limit() {
    let dir = Dir::new();
    let program = dir.script("tmux", "exec /bin/sleep 30");
    let started = Instant::now();
    let result = driver("s")
        .with_program(&program)
        .with_timeout(Duration::from_millis(300))
        .server_running();
    assert_eq!(result, Err(TmuxError::Timeout));
    assert!(started.elapsed() < Duration::from_secs(5));
}

#[test]
fn a_missing_tmux_is_a_spawn_error() {
    let result = driver("s").with_program("/nonexistent/tmux").server_running();
    assert!(matches!(result, Err(TmuxError::Spawn(_))), "{result:?}");
}
