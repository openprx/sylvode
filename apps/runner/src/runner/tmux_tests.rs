#![allow(clippy::indexing_slicing, clippy::many_single_char_names)]

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use uuid::Uuid;

use super::{
    BracketedPasteConfirmed, BufferName, LIST_PANES_FORMAT, MAX_PUSH_BYTES, PaneId, PasteGuard, PushText, Refusal,
    Slot, SocketName, Tmux, TmuxError, TmuxName, WorkDir, exec_argv, one_line, parse_listed_pane, parse_pane_state,
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

/// The derived tmux session `<key>-claude`.
fn name(key: &str) -> TmuxName {
    TmuxName::derived(&Slot::project(key, "claude").expect("name")).expect("derived")
}

/// The recorded pane of session SVWL-claude: pane id as given, process 1 (what the fake answers).
fn pane(session: &str, id: &str) -> Pane {
    Pane {
        name: name(session),
        id: PaneId::new(id).expect("pane id"),
        pid: 1,
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

const BOT: &str = "8c1f0a52-3d4e-4b6a-9c7d-1e2f3a4b5c6d";
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
fn session_names_and_pane_ids_are_validated() {
    for (key, cli) in [
        ("A", "claude"),
        ("SVWL", "codex"),
        ("123", "opencode"),
        ("AB12", "claude"),
        (&"K".repeat(32), "claude"),
    ] {
        let name = Slot::project(key, cli).expect("valid");
        assert_eq!(name.as_str(), format!("{key}-{cli}"));
        assert_eq!((name.key(), name.cli()), (Some(key), cli));
        assert_eq!(Slot::parse(name.as_str()), Ok(name));
    }
    for bad_key in [
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
        assert!(
            matches!(Slot::project(bad_key, "claude"), Err(TmuxError::Invalid(_))),
            "{bad_key:?}"
        );
    }
    for bad_cli in ["", "Claude", "gemini", "claude ", "claude-x", "cla.ude"] {
        assert!(
            matches!(Slot::project("SVWL", bad_cli), Err(TmuxError::Invalid(_))),
            "{bad_cli:?}"
        );
    }
    // Parsing a name back needs exactly the pattern `^[A-Z0-9]{1,32}-(claude|codex|opencode)$`.
    for bad in [
        "SVWL",
        "SVWL-",
        "-claude",
        "SVWL-claude-x",
        "svwl-claude",
        "SVWL-Claude",
        "SVWL_claude",
        "SVWL-claude\n",
        "=SVWL-claude",
        "SVWL-claude:0",
        "SV-WL-claude",
    ] {
        assert!(Slot::parse(bad).is_err(), "{bad:?}");
    }
    // The exact target cannot match another session.
    assert_eq!(name("SVWL").session_target(), "=SVWL-claude");
    assert_eq!(name("123").session_target(), "=123-claude");
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

/// A configured name is accepted by pattern and never looks like a derived name of any project.
#[test]
fn a_configured_tmux_name_follows_the_pattern_and_never_looks_derived() {
    for good in [
        "sv",
        "rf",
        "prc-cli",
        "cli",
        "_x",
        "a",
        "SV",
        "claude",
        "SV-main",
        &"a".repeat(64),
    ] {
        assert!(TmuxName::new(good).is_ok(), "{good}");
    }
    for bad in [
        "",
        "-x",
        "a b",
        "a:b",
        "a.b",
        "a=b",
        "%1",
        "a#b",
        "a/b",
        "é",
        "a\nb",
        &"a".repeat(65),
        // The derived shape, in any case, for any key.
        "SV-claude",
        "sv-claude",
        "Sv-Codex",
        "X1-opencode",
        "sv-CLAUDE",
        "0-claude",
    ] {
        assert!(matches!(TmuxName::new(bad), Err(TmuxError::Invalid(_))), "{bad:?}");
    }
    // 33 key characters are not a derived shape, so the name is a plain custom one.
    assert!(TmuxName::new(&format!("{}-claude", "A".repeat(33))).is_ok());
    assert_eq!(TmuxName::new("sv").expect("name").session_target(), "=sv");
    let slot = Slot::project("SV", "codex").expect("slot");
    assert_eq!(TmuxName::derived(&slot).expect("derived").as_str(), "SV-codex");
    // A bot slot has no derived name.
    assert!(TmuxName::derived(&Slot::bot(BOT, "codex").expect("bot slot")).is_none());
}

/// Every call that addresses a session takes the exact target of the name it is given, a
/// configured one included, and the call set is the same as before.
#[test]
fn the_call_builders_address_a_configured_session_exactly() {
    let t = driver("sylvode");
    let k = TmuxName::new("sv").expect("name");
    let with = |rest: &[&str]| -> Vec<String> {
        ["-L", "sylvode"]
            .iter()
            .chain(rest.iter())
            .map(|s| (*s).to_string())
            .collect()
    };
    assert_eq!(t.has_session_call(&k).args, with(&["has-session", "-t", "=sv"]));
    assert_eq!(
        t.list_panes_call(&k).args,
        with(&["list-panes", "-s", "-t", "=sv", "-F", LIST_PANES_FORMAT])
    );
    assert_eq!(
        t.list_clients_call(&k).args,
        with(&["list-clients", "-F", "#{client_activity}", "-t", "=sv"])
    );
}

#[test]
fn every_call_is_an_argument_vector_with_the_socket_and_an_exact_target() {
    let t = driver("sylvode");
    let k = name("SVWL");
    let p = PaneId::new("%7").expect("id");
    let d = dir("/work/svwl");
    let argv = strings(&["claude", "--permission-mode", "bypassPermissions"]);
    let g = ["-L", "sylvode"];
    let with = |rest: &[&str]| -> Vec<String> { g.iter().chain(rest.iter()).map(|s| (*s).to_string()).collect() };
    assert_eq!(Tmux::version_call().args, strings(&["-V"]));
    assert_eq!(
        t.has_session_call(&k).args,
        with(&["has-session", "-t", "=SVWL-claude"])
    );
    assert_eq!(
        t.server_probe_call().args,
        with(&["list-panes", "-a", "-F", "#{pane_id}"])
    );
    assert_eq!(
        t.list_panes_call(&k).args,
        with(&["list-panes", "-s", "-t", "=SVWL-claude", "-F", LIST_PANES_FORMAT])
    );
    assert_eq!(
        LIST_PANES_FORMAT,
        "#{pane_id} #{pane_pid} #{pane_dead} #{pane_current_command}"
    );
    assert_eq!(
        t.new_session_call(&k, &d, 200, 50, &argv).expect("call").args,
        with(&[
            "new-session",
            "-d",
            "-P",
            "-F",
            "#{pane_id} #{pane_pid} #{socket_path}",
            "-s",
            "SVWL-claude",
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
            "bypassPermissions"
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
        t.capture_pane_call(&p, None, false).args,
        with(&["capture-pane", "-p", "-t", "%7"])
    );
    assert_eq!(
        t.capture_pane_call(&p, None, true).args,
        with(&["capture-pane", "-p", "-e", "-t", "%7"])
    );
    assert_eq!(
        t.capture_pane_call(&p, Some(200), false).args,
        with(&["capture-pane", "-p", "-t", "%7", "-S", "-200"])
    );
    assert_eq!(
        t.list_clients_call(&k).args,
        with(&["list-clients", "-F", "#{client_activity}", "-t", "=SVWL-claude"])
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

/// The runner types into a session it did not start and shares the operator's tmux server, so the
/// calls that exist are exactly the harmless ones: no `-f`, no option, no respawn, kill, rename,
/// resize or other key than Enter.
#[test]
fn only_the_listed_tmux_calls_exist_and_none_passes_a_config_file() {
    let t = driver("sylvode");
    let k = name("SVWL");
    let p = PaneId::new("%7").expect("id");
    let buffer = BufferName::for_task(&task());
    let text = PushText::new("x").expect("text");
    let new_session = t
        .new_session_call(&k, &dir("/w"), 80, 24, &strings(&["claude", "x"]))
        .expect("call");
    let all = [
        Tmux::version_call(),
        t.has_session_call(&k),
        t.server_probe_call(),
        t.list_panes_call(&k),
        new_session,
        t.state_call(&p),
        t.field_call(&p, "#{pane_current_command}"),
        t.capture_pane_call(&p, Some(5), false),
        t.capture_pane_call(&p, Some(5), true),
        t.list_clients_call(&k),
        t.load_buffer_call(&buffer, &text),
        t.paste_buffer_call(&buffer, &p),
        t.delete_buffer_call(&buffer),
        t.send_enter_call(&p),
    ];
    let mut commands: Vec<String> = Vec::new();
    for call in &all {
        // After the global options (`-L <socket>`), the first word is the tmux command.
        let rest: Vec<&String> = if call.args.first().is_some_and(|a| a == "-L") {
            call.args.iter().skip(2).collect()
        } else {
            call.args.iter().collect()
        };
        assert!(!call.args.iter().any(|a| a == "-f"), "{:?}", call.args);
        let command = rest.first().map(|c| (*c).clone()).unwrap_or_default();
        if command == "send-keys" {
            assert_eq!(rest.last().map(|a| a.as_str()), Some("Enter"), "{:?}", call.args);
            assert_eq!(rest.len(), 4, "send-keys takes only `-t %N Enter`: {:?}", call.args);
        }
        commands.push(command);
    }
    commands.sort();
    commands.dedup();
    assert_eq!(
        commands,
        [
            "-V",
            "capture-pane",
            "delete-buffer",
            "display-message",
            "has-session",
            "list-clients",
            "list-panes",
            "load-buffer",
            "new-session",
            "paste-buffer",
            "send-keys"
        ]
    );
    // The new session sets no option and names no window beyond its first.
    assert!(
        !all.iter()
            .flat_map(|c| c.args.iter())
            .any(|a| a == "set-option" || a == "respawn-pane" || a == "kill-session" || a == "remain-on-exit")
    );
}

#[test]
fn no_call_but_load_buffer_carries_stdin_and_no_argument_carries_the_text() {
    let t = driver("sylvode");
    let text = "[Sylvode task] id=x\nIssue: A-1 \"; rm -rf ~ #\" $(id) `id`";
    let buffer = BufferName::for_task(&task());
    let call = t.load_buffer_call(&buffer, &PushText::new(text).expect("text"));
    assert_eq!(
        call.args,
        strings(&["-L", "sylvode", "load-buffer", "-b", &format!("sv-{TASK}"), "-"])
    );
    assert_eq!(call.stdin.as_deref(), Some(text.as_bytes()));
    let p = PaneId::new("%1").expect("id");
    for other in [
        t.paste_buffer_call(&buffer, &p),
        t.send_enter_call(&p),
        t.has_session_call(&name("A")),
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
    // The default server, and the operator's own tmux.conf: neither `-L` nor `-f`.
    assert_eq!(
        driver("").has_session_call(&name("A")).args,
        strings(&["has-session", "-t", "=A-claude"])
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
        .new_session_call(&name("A"), &dir("/w"), 80, 24, &strings(&["cat"]))
        .expect("call");
    assert_eq!(&call.args[call.args.len() - 4..], ["--", "/usr/bin/env", "--", "cat"]);
    // An option-looking argument stays one argument after `--`.
    let call = t
        .new_session_call(&name("A"), &dir("/w"), 80, 24, &strings(&["cli", "-c", "$(id)"]))
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
    let state = parse_pane_state("0 0 1 1 SVWL-claude").expect("state");
    assert_eq!(
        (
            state.dead,
            state.in_mode,
            state.bracket_paste,
            state.pid,
            state.session_name.as_str()
        ),
        (false, 0, Some(true), 1, "SVWL-claude")
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
        .map(|c| c.get(2).cloned().unwrap_or_default())
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
    let (program, log) = fake_tmux(&dir, &healthy_answers("0 0 1 1 SVWL-claude", "claude.exe"));
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
        calls[2][2..],
        strings(&["load-buffer", "-b", &format!("sv-{TASK}"), "-"])
    );
    assert_eq!(
        calls[3][2..],
        strings(&["paste-buffer", "-p", "-d", "-b", &format!("sv-{TASK}"), "-t", "%7"])
    );
    assert_eq!(calls[4][2..], strings(&["send-keys", "-t", "%7", "Enter"]));
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
    assert_eq!(refused("0 0 1 1 SVWL-claude", "claude.exe", &GUARD), Ok(()));
    assert_eq!(
        refused("1 0 1 1 SVWL-claude", "claude.exe", &GUARD),
        Err(TmuxError::Refused(Refusal::PaneDead))
    );
    assert_eq!(
        refused("0 1 1 1 SVWL-claude", "claude.exe", &GUARD),
        Err(TmuxError::Refused(Refusal::PaneInMode))
    );
    assert_eq!(
        refused("0 0 0 1 SVWL-claude", "claude.exe", &GUARD),
        Err(TmuxError::Refused(Refusal::BracketedPasteOff))
    );
    assert_eq!(
        refused("0 0 1 1 OTHER-claude", "claude.exe", &GUARD),
        Err(TmuxError::Refused(Refusal::NotOwned))
    );
    // The pane id is right and the session too, but the pane's process is not the recorded one
    // (the pane was replaced, or the server restarted and reused the id): refused.
    assert_eq!(
        refused("0 0 1 2 SVWL-claude", "claude.exe", &GUARD),
        Err(TmuxError::Refused(Refusal::NotOwned))
    );
    for shell in ["zsh", "bash", "sh", "vim", "claude.exe x", ""] {
        assert_eq!(
            refused("0 0 1 1 SVWL-claude", shell, &GUARD),
            Err(TmuxError::Refused(Refusal::CommandNotAllowed)),
            "{shell:?}"
        );
    }
}

#[test]
fn a_tmux_that_cannot_report_bracketed_paste_needs_the_confirmation() {
    // Before 3.7 the field is empty.
    assert_eq!(
        refused("0 0  1 SVWL-claude", "claude.exe", &GUARD),
        Err(TmuxError::Refused(Refusal::BracketedPasteUnconfirmed))
    );
    let confirmed = PasteGuard {
        bracketed_confirmed: Some(BracketedPasteConfirmed::from_readiness_check()),
        ..GUARD
    };
    assert_eq!(refused("0 0  1 SVWL-claude", "claude.exe", &confirmed), Ok(()));
    // A confirmation never overrides a pane that reports bracketed paste as off, nor any other check.
    assert_eq!(
        refused("0 0 0 1 SVWL-claude", "claude.exe", &confirmed),
        Err(TmuxError::Refused(Refusal::BracketedPasteOff))
    );
    assert_eq!(
        refused("1 0  1 SVWL-claude", "claude.exe", &confirmed),
        Err(TmuxError::Refused(Refusal::PaneDead))
    );
    assert_eq!(
        refused("0 0  1 SVWL-claude", "zsh", &confirmed),
        Err(TmuxError::Refused(Refusal::CommandNotAllowed))
    );
}

#[test]
fn text_that_is_refused_never_reaches_tmux() {
    let dir = Dir::new();
    let (program, log) = fake_tmux(&dir, &healthy_answers("0 0 1 1 SVWL-claude", "claude.exe"));
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
    let healthy = healthy_answers("0 0 1 1 SVWL-claude", "claude.exe");
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
        calls.last().map(|c| c[2..].to_vec()),
        Some(strings(&["delete-buffer", "-b", &format!("sv-{TASK}")]))
    );
    assert!(!names(&log).iter().any(|n| n == "send-keys"));
}

#[test]
fn a_paste_that_times_out_is_reported_as_delivery_unknown_and_not_retried() {
    let dir = Dir::new();
    let healthy = healthy_answers("0 0 1 1 SVWL-claude", "claude.exe");
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
    // The 5 s timeout is what is under test; the bound only says the call did not hang.
    assert!(started.elapsed() < Duration::from_mins(1));
    let names = names(&log);
    assert_eq!(names.iter().filter(|n| *n == "paste-buffer").count(), 1);
    assert_eq!(names.last().map(String::as_str), Some("delete-buffer"));
    assert!(!names.iter().any(|n| n == "send-keys"));
}

#[test]
fn an_enter_that_fails_after_the_paste_says_so() {
    let dir = Dir::new();
    let healthy = healthy_answers("0 0 1 1 SVWL-claude", "claude.exe");
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
    let healthy = healthy_answers("0 0 1 1 SVWL-claude", "claude.exe");
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
    let (program, log) = fake_tmux(&dir, &healthy_answers("0 0 1 1 SVWL-claude", "claude.exe"));
    fresh()
        .with_program(&program)
        .retry_enter(&pane("SVWL", "%7"))
        .expect("sent");
    assert_eq!(names(&log), ["display-message", "send-keys"]);
    for (state, want) in [
        ("1 0 1 1 SVWL-claude", Refusal::PaneDead),
        ("0 1 1 1 SVWL-claude", Refusal::PaneInMode),
        ("0 0 1 1 OTHER-claude", Refusal::NotOwned),
        ("0 0 1 2 SVWL-claude", Refusal::NotOwned),
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
        assert_eq!(driver("s").with_program(&program).has_session(&name("A")), want);
    }
    let (program, _) = fake_tmux(&dir, "echo boom >&2; exit 2");
    assert_eq!(
        driver("s").with_program(&program).has_session(&name("A")),
        Err(TmuxError::Failed {
            code: Some(2),
            stderr: "boom".to_string()
        })
    );
}

#[test]
fn new_session_returns_the_pane_id_and_pid_and_sets_no_option() {
    let dir = Dir::new();
    let (program, log) = fake_tmux(
        &dir,
        "case \"$*\" in *new-session*) echo '%7 4242 /tmp/tmux-1/s';; esac\nexit 0",
    );
    let pane = driver("sylvode")
        .with_program(&program)
        .new_session(
            &name("SVWL"),
            &dir_of("/work/svwl"),
            200,
            50,
            &strings(&["claude", "--x"]),
        )
        .expect("created");
    assert_eq!(
        pane,
        super::Created {
            pane: super::Pane {
                name: name("SVWL"),
                id: PaneId::new("%7").expect("id"),
                pid: 4242
            },
            socket_path: Some("/tmp/tmux-1/s".to_string()),
        }
    );
    // Only the probe for a running server and the creation: no option is set on anything.
    assert_eq!(names(&log), ["list-panes", "new-session"]);
    // An answer that is not a pane id and a pid is an error.
    for junk in ["oops", "%7", "%7 x", "%7 4242", "7 1 /tmp/s"] {
        let (bad, _) = fake_tmux(&dir, &format!("echo '{junk}'; exit 0"));
        assert!(
            driver("s")
                .with_program(&bad)
                .new_session(&name("A"), &dir_of("/w"), 80, 24, &strings(&["a", "b"]))
                .is_err(),
            "{junk}"
        );
    }
}

/// The CLI in a session the runner creates inherits the operator's environment: `new-session`
/// gets the runner's own variables minus the three agent ones, while every other call gets only the
/// allow-list.
#[test]
fn new_session_has_the_operators_environment_and_other_calls_the_allow_list() {
    let dir = Dir::new();
    let env_new = dir.path().join("env-new");
    let env_other = dir.path().join("env-other");
    let program = dir.script(
        "tmux",
        &format!(
            "case \"$*\" in *new-session*) /usr/bin/env > '{}'; echo '%1 2 /tmp/s';; *) /usr/bin/env > '{}';; esac\nexit 0",
            env_new.display(),
            env_other.display()
        ),
    );
    let t = driver("s")
        .with_program(&program)
        .with_env("CLAUDECODE", "1")
        .with_env("TMUX", "/tmp/x,1,0")
        .with_env("OPERATOR_SETTING", "kept");
    t.new_session(&name("A"), &dir_of("/w"), 80, 24, &strings(&["a", "b"]))
        .expect("created");
    let vars = |path: &Path| -> Vec<String> {
        fs::read_to_string(path)
            .expect("env")
            .lines()
            .filter_map(|l| l.split_once('=').map(|(n, _)| n.to_string()))
            .collect()
    };
    let new = vars(&env_new);
    // The operator's variables (cargo's, here) reach the session; the agent ones do not.
    assert!(new.iter().any(|n| n == "CARGO_MANIFEST_DIR"), "{new:?}");
    for gone in ["CLAUDECODE", "CLAUDE_CODE_ENTRYPOINT", "TMUX"] {
        assert!(!new.iter().any(|n| n == gone), "{gone}");
    }
    // A variable set on the driver is passed on as well.
    assert!(new.iter().any(|n| n == "OPERATOR_SETTING"));
    // The probe before it ran with the allow-list only.
    let other = vars(&env_other);
    assert!(
        other.iter().any(|n| n == "PATH") && !other.iter().any(|n| n == "CARGO_MANIFEST_DIR"),
        "{other:?}"
    );
}

/// `list-panes` is parsed pane by pane: id, process id, dead flag, and the foreground command last
/// (it may hold spaces).
#[test]
fn the_panes_of_a_session_are_listed_with_their_foreground_command() {
    let dir = Dir::new();
    let (program, log) = fake_tmux(
        &dir,
        "printf '%%0 100 0 zsh\\n%%3 200 0 claude\\n%%4 300 1 claude.exe\\n%%5 400 0 my tool\\n'",
    );
    let panes = driver("s")
        .with_program(&program)
        .list_panes(&name("A"))
        .expect("panes");
    let shown: Vec<(String, u32, bool, String)> = panes
        .iter()
        .map(|p| (p.id.as_str().to_string(), p.pid, p.dead, p.command.clone()))
        .collect();
    assert_eq!(
        shown,
        [
            ("%0".to_string(), 100, false, "zsh".to_string()),
            ("%3".to_string(), 200, false, "claude".to_string()),
            ("%4".to_string(), 300, true, "claude.exe".to_string()),
            ("%5".to_string(), 400, false, "my tool".to_string()),
        ]
    );
    assert_eq!(names(&log), ["list-panes"]);
    for bad in ["junk", "%1 x 0 a", "%1 5 2 a", "1 5 0 a", "%1 5 0"] {
        assert_eq!(parse_listed_pane(bad), None, "{bad:?}");
    }
    for junk in ["echo junk", "echo '%1 5 0'"] {
        let (program, _) = fake_tmux(&dir, junk);
        assert!(
            driver("s").with_program(&program).list_panes(&name("A")).is_err(),
            "{junk}"
        );
    }
    // No such session: a failure, not an empty list.
    let (program, _) = fake_tmux(&dir, "echo \"can't find session\" >&2; exit 1");
    assert!(driver("s").with_program(&program).list_panes(&name("A")).is_err());
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
    let (program, log) = fake_tmux(&dir, &healthy_answers("0 0 1 99 SVWL-claude", "claude.exe"));
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
    let (program, _) = fake_tmux(&dir, &healthy_answers("0 0 1 99 SVWL-claude", "claude.exe /fake/path"));
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
fn client_activity_lists_timestamps() {
    let dir = Dir::new();
    let (program, _) = fake_tmux(&dir, "printf '1700000000\\n1700000100\\n'");
    assert_eq!(
        driver("s").with_program(&program).client_activity(&name("A")),
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
        "case \"$*\" in *list-panes*) exit 1;; *new-session*) echo '%2 99 /tmp/s';; esac\nexit 0",
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
        .new_session(&name("SVWL"), &dir_of("/w"), 80, 24, &strings(&["claude", "x"]))
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
    assert_eq!(
        names(&log),
        ["list-panes", "new-session"],
        "no option is set on the new server"
    );
}

#[test]
fn a_running_server_is_not_wrapped_again() {
    let dir = Dir::new();
    let (program, log) = fake_tmux(&dir, "case \"$*\" in *new-session*) echo '%2 99 /tmp/s';; esac\nexit 0");
    let scope_log = dir.path().join("scope");
    let systemd_run = dir.script(
        "systemd-run",
        &format!("echo used > '{}'\nshift 3\nexec \"$@\"", scope_log.display()),
    );
    driver("sylvode")
        .with_program(&program)
        .with_systemd(true)
        .with_systemd_run(&systemd_run)
        .new_session(&name("SVWL"), &dir_of("/w"), 80, 24, &strings(&["claude", "x"]))
        .expect("created");
    assert!(!scope_log.exists());
    assert_eq!(calls(&log).len(), 2);
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

/// A fake program written a moment before it runs can be "text file busy" while another thread's
/// forked child still holds the write descriptor; the driver tries again instead of failing the
/// call. Many threads writing and running scripts at once make that window common.
#[test]
fn a_freshly_written_program_that_is_briefly_busy_still_runs() {
    let handles: Vec<_> = (0..4)
        .map(|_| {
            std::thread::spawn(|| {
                let dir = Dir::new();
                for _ in 0..12 {
                    let program = dir.script("tmux", "echo 'tmux 3.4'");
                    let version = fresh().with_program(&program).version();
                    assert_eq!(version.as_deref(), Ok("tmux 3.4"));
                }
            })
        })
        .collect();
    for handle in handles {
        handle.join().expect("no thread failed");
    }
}

/// A bot slot is `bot-<uuid>-<cli>`: it parses back, carries the bot and the CLI, has no key, and
/// can never be read as a project slot, a derived name or a state directory of a key.
#[test]
fn a_bot_slot_formats_and_parses_and_never_folds_onto_a_project_slot() {
    for cli in ["claude", "codex"] {
        let slot = Slot::bot(BOT, cli).expect("bot slot");
        assert_eq!(slot.as_str(), format!("bot-{BOT}-{cli}"));
        assert_eq!((slot.key(), slot.bot_id(), slot.cli()), (None, Some(BOT), cli));
        assert!(slot.is_bot());
        assert_eq!(Slot::parse(slot.as_str()), Ok(slot));
    }
    // Any form of the id gives the canonical lower case one.
    assert_eq!(
        Slot::bot(&BOT.to_uppercase(), "claude").expect("upper").as_str(),
        format!("bot-{BOT}-claude")
    );
    assert!(Slot::bot("not-a-uuid", "claude").is_err());
    assert!(Slot::bot(BOT, "gemini").is_err());
    // A project slot has a key and no bot.
    let project = Slot::project("SV", "claude").expect("project slot");
    assert_eq!(
        (project.key(), project.bot_id(), project.is_bot()),
        (Some("SV"), None, false)
    );
    // Reading back needs the canonical lower case form.
    let upper = BOT.to_uppercase();
    for bad in [
        format!("bot-{upper}-claude"),
        format!("BOT-{BOT}-claude"),
        format!("bot-{BOT}"),
        format!("bot-{BOT}-gemini"),
        format!("bot-{BOT}-claude-x"),
        format!("bot-{}-claude", BOT.replace('-', "")),
        format!("bot-{BOT}-claude\n"),
        "bot-claude".to_string(),
    ] {
        assert!(Slot::parse(&bad).is_err(), "{bad:?}");
    }
    // The first part is lower case: a project key never parses from it, and it is no derived name.
    assert!(Slot::project("bot", "claude").is_err());
    assert!(
        TmuxName::new(&format!("bot-{BOT}-claude")).is_ok(),
        "a plain name of this length"
    );
    let slot = Slot::bot(BOT, "claude").expect("bot slot");
    assert_ne!(slot.as_str().to_ascii_uppercase(), slot.as_str());
}
