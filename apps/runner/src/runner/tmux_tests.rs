#![allow(clippy::indexing_slicing)]

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use uuid::Uuid;

use super::tmux::{
    BufferName, PANE_FORMAT, PaneInfo, PushText, SessionKey, SocketName, Tmux, TmuxError, parse_pane_info,
};

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

fn driver(socket: &str) -> Tmux {
    Tmux::new(SocketName::new(socket).expect("socket")).with_systemd(false)
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
fn session_keys_are_validated_and_targets_are_exact() {
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
    assert_eq!(key("SVWL").window_target(), "=SVWL:0");
    assert_eq!(key("123").window_target(), "=123:0");
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
fn every_call_is_an_argument_vector_with_the_socket_and_an_exact_target() {
    let t = driver("sylvode");
    let k = key("SVWL");
    let dir = Path::new("/work/svwl");
    let argv = strings(&["claude", "--permission-mode", "dontAsk"]);
    let call = |c: super::tmux::Call| c.args;
    assert_eq!(call(Tmux::version_call()), strings(&["-V"]));
    assert_eq!(
        call(t.has_session_call(&k)),
        strings(&["-L", "sylvode", "has-session", "-t", "=SVWL"])
    );
    assert_eq!(
        call(t.list_sessions_call()),
        strings(&["-L", "sylvode", "list-sessions"])
    );
    assert_eq!(
        call(t.new_session_call(&k, dir, 200, 50, &argv).expect("call")),
        strings(&[
            "-L",
            "sylvode",
            "new-session",
            "-d",
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
        call(t.remain_on_exit_call(&k)),
        strings(&[
            "-L",
            "sylvode",
            "set-option",
            "-w",
            "-t",
            "=SVWL:0",
            "remain-on-exit",
            "on"
        ])
    );
    assert_eq!(
        call(t.respawn_pane_call(&k, dir, &argv).expect("call")),
        strings(&[
            "-L",
            "sylvode",
            "respawn-pane",
            "-k",
            "-t",
            "=SVWL:0",
            "-c",
            "/work/svwl",
            "--",
            "claude",
            "--permission-mode",
            "dontAsk"
        ])
    );
    assert_eq!(
        call(t.pane_info_call(&k)),
        strings(&["-L", "sylvode", "display-message", "-p", "-t", "=SVWL:0", PANE_FORMAT])
    );
    assert_eq!(
        PANE_FORMAT,
        "#{pane_dead} #{pane_pid} #{pane_current_command} #{pane_current_path}"
    );
    assert_eq!(
        call(t.capture_pane_call(&k, None)),
        strings(&["-L", "sylvode", "capture-pane", "-p", "-t", "=SVWL:0"])
    );
    assert_eq!(
        call(t.capture_pane_call(&k, Some(200))),
        strings(&["-L", "sylvode", "capture-pane", "-p", "-t", "=SVWL:0", "-S", "-200"])
    );
    assert_eq!(
        call(t.list_clients_call(&k)),
        strings(&[
            "-L",
            "sylvode",
            "list-clients",
            "-F",
            "#{client_activity}",
            "-t",
            "=SVWL"
        ])
    );
    let buffer = BufferName::for_task(&task());
    assert_eq!(
        call(t.paste_buffer_call(&buffer, &k)),
        strings(&[
            "-L",
            "sylvode",
            "paste-buffer",
            "-p",
            "-d",
            "-b",
            &format!("sv-{TASK}"),
            "-t",
            "=SVWL:0"
        ])
    );
    assert_eq!(
        call(t.send_enter_call(&k)),
        strings(&["-L", "sylvode", "send-keys", "-t", "=SVWL:0", "Enter"])
    );
}

#[test]
fn the_text_goes_only_to_stdin_of_load_buffer() {
    let t = driver("sylvode");
    let text = "[Sylvode task] id=x\nIssue: A-1 \"; rm -rf ~ #\" $(id) `id`";
    let call = t.load_buffer_call(&BufferName::for_task(&task()), &PushText::new(text).expect("text"));
    assert_eq!(
        call.args,
        strings(&["-L", "sylvode", "load-buffer", "-b", &format!("sv-{TASK}"), "-"])
    );
    assert_eq!(call.stdin.as_deref(), Some(text.as_bytes()));
    // No other call carries stdin, and no argument of any call contains the text.
    let k = key("SVWL");
    for other in [
        t.paste_buffer_call(&BufferName::for_task(&task()), &k),
        t.send_enter_call(&k),
        t.has_session_call(&k),
        t.pane_info_call(&k),
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
    let t = driver("");
    assert_eq!(
        t.has_session_call(&key("A")).args,
        strings(&["has-session", "-t", "=A"])
    );
}

#[test]
fn a_config_file_is_passed_as_a_global_option() {
    let t = driver("s").with_config_file("/dev/null");
    assert_eq!(
        t.list_sessions_call().args,
        strings(&["-L", "s", "-f", "/dev/null", "list-sessions"])
    );
}

#[test]
fn the_working_directory_must_be_absolute_and_the_command_non_empty() {
    let t = driver("s");
    let k = key("A");
    let argv = strings(&["cat"]);
    for dir in ["relative", "", "-x", "./a"] {
        assert!(
            matches!(
                t.new_session_call(&k, Path::new(dir), 80, 24, &argv),
                Err(TmuxError::Invalid(_))
            ),
            "{dir:?}"
        );
        assert!(matches!(
            t.respawn_pane_call(&k, Path::new(dir), &argv),
            Err(TmuxError::Invalid(_))
        ));
    }
    assert!(t.new_session_call(&k, Path::new("/w"), 80, 24, &[]).is_err());
    assert!(
        t.new_session_call(&k, Path::new("/w"), 80, 24, &strings(&["cat", ""]))
            .is_err()
    );
    // An argument that looks like an option stays one argument after `--`.
    let call = t
        .new_session_call(&k, Path::new("/w"), 80, 24, &strings(&["cli", "-c", "$(id)"]))
        .expect("call");
    assert_eq!(&call.args[call.args.len() - 4..], ["--", "cli", "-c", "$(id)"]);
}

#[test]
fn pushed_text_refuses_every_control_character_but_newline() {
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
    ] {
        assert!(matches!(PushText::new(bad), Err(TmuxError::Invalid(_))), "{bad:?}");
    }
    // The refusal names the kind of character, never the text.
    let error = PushText::new("secret\u{1b}").expect_err("refused").to_string();
    assert!(!error.contains("secret"), "{error}");
}

#[test]
fn pane_information_parses_with_a_path_that_contains_spaces() {
    assert_eq!(
        parse_pane_info("0 4242 claude.exe /Users/x/my work\n"),
        Some(PaneInfo {
            dead: false,
            pid: 4242,
            current_command: "claude.exe".to_string(),
            current_path: "/Users/x/my work".to_string(),
        })
    );
    assert_eq!(parse_pane_info("1 7 zsh /tmp").map(|p| p.dead), Some(true));
    for bad in ["", "2 7 a /b", "x 7 a /b", "0 x a /b", "0 7 a", "0 7"] {
        assert_eq!(parse_pane_info(bad), None, "{bad:?}");
    }
}

/// A fake `tmux` that records every call (one argument per line, calls separated by `---`) and
/// the standard input of `load-buffer`, and answers according to `answers`.
fn fake_tmux(dir: &Dir, answers: &str) -> (PathBuf, PathBuf, PathBuf) {
    let log = dir.path().join("log");
    let stdin = dir.path().join("stdin");
    let env_log = dir.path().join("env");
    let body = format!(
        "echo --- >> '{log}'\nfor a in \"$@\"; do echo \"$a\" >> '{log}'; done\n\
         echo \"CLAUDECODE=${{CLAUDECODE-unset}} CLAUDE_CODE_ENTRYPOINT=${{CLAUDE_CODE_ENTRYPOINT-unset}} TMUX=${{TMUX-unset}}\" >> '{env}'\n\
         for a in \"$@\"; do [ \"$a\" = load-buffer ] && cat > '{stdin}'; done\n{answers}",
        log = log.display(),
        stdin = stdin.display(),
        env = env_log.display(),
    );
    (dir.script("tmux", &body), log, env_log)
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

#[test]
fn pasting_loads_the_buffer_through_stdin_then_pastes_it() {
    let dir = Dir::new();
    let (program, log, _) = fake_tmux(&dir, "exit 0");
    let text = "[Sylvode task] id=1\nIssue: A-1 \"x\"";
    driver("sylvode")
        .with_program(&program)
        .paste_text(&key("SVWL"), &task(), text)
        .expect("pasted");
    let calls = calls(&log);
    assert_eq!(calls.len(), 2);
    assert_eq!(
        calls[0],
        strings(&["-L", "sylvode", "load-buffer", "-b", &format!("sv-{TASK}"), "-"])
    );
    assert_eq!(
        calls[1],
        strings(&[
            "-L",
            "sylvode",
            "paste-buffer",
            "-p",
            "-d",
            "-b",
            &format!("sv-{TASK}"),
            "-t",
            "=SVWL:0"
        ])
    );
    assert_eq!(fs::read_to_string(dir.path().join("stdin")).expect("stdin"), text);
}

#[test]
fn text_with_an_escape_never_reaches_tmux() {
    let dir = Dir::new();
    let (program, log, _) = fake_tmux(&dir, "exit 0");
    let error = driver("sylvode")
        .with_program(&program)
        .paste_text(&key("SVWL"), &task(), "hello \u{1b}[201~ /exit")
        .expect_err("refused");
    assert!(matches!(error, TmuxError::Invalid(_)));
    assert!(!log.exists(), "tmux was run");
}

#[test]
fn a_failed_paste_deletes_the_buffer() {
    let dir = Dir::new();
    let (program, log, _) = fake_tmux(
        &dir,
        "for a in \"$@\"; do [ \"$a\" = paste-buffer ] && exit 1; done\nexit 0",
    );
    let error = driver("sylvode")
        .with_program(&program)
        .paste_text(&key("SVWL"), &task(), "text")
        .expect_err("fails");
    assert!(matches!(error, TmuxError::Failed { code: Some(1), .. }));
    let calls = calls(&log);
    assert_eq!(calls.len(), 3);
    assert_eq!(
        calls[2],
        strings(&["-L", "sylvode", "delete-buffer", "-b", &format!("sv-{TASK}")])
    );
}

#[test]
fn enter_is_the_only_key_sent() {
    let dir = Dir::new();
    let (program, log, _) = fake_tmux(&dir, "exit 0");
    driver("sylvode")
        .with_program(&program)
        .send_enter(&key("SVWL"))
        .expect("sent");
    assert_eq!(
        calls(&log),
        vec![strings(&["-L", "sylvode", "send-keys", "-t", "=SVWL:0", "Enter"])]
    );
}

#[test]
fn has_session_maps_exit_statuses() {
    let dir = Dir::new();
    for (code, want) in [(0, Ok(true)), (1, Ok(false))] {
        let (program, _, _) = fake_tmux(&dir, &format!("exit {code}"));
        assert_eq!(driver("s").with_program(&program).has_session(&key("A")), want);
    }
    let (program, _, _) = fake_tmux(&dir, "echo boom >&2; exit 2");
    assert_eq!(
        driver("s").with_program(&program).has_session(&key("A")),
        Err(TmuxError::Failed {
            code: Some(2),
            stderr: "boom".to_string()
        })
    );
}

#[test]
fn new_session_creates_the_window_then_keeps_dead_panes() {
    let dir = Dir::new();
    let (program, log, _) = fake_tmux(&dir, "exit 0");
    driver("sylvode")
        .with_program(&program)
        .new_session(&key("SVWL"), Path::new("/work/svwl"), 200, 50, &strings(&["claude"]))
        .expect("created");
    let calls = calls(&log);
    assert_eq!(calls.len(), 2);
    assert_eq!(calls[0][2], "new-session");
    assert_eq!(calls[1][2], "set-option");
}

#[test]
fn pane_information_is_read_with_display_message() {
    let dir = Dir::new();
    let (program, _, _) = fake_tmux(&dir, "echo '0 99 claude.exe /work/svwl'");
    let info = driver("s")
        .with_program(&program)
        .pane_info(&key("SVWL"))
        .expect("info");
    assert_eq!(
        (info.dead, info.pid, info.current_command.as_str()),
        (false, 99, "claude.exe")
    );
    let (program, _, _) = fake_tmux(&dir, "echo garbage");
    assert!(driver("s").with_program(&program).pane_info(&key("SVWL")).is_err());
}

#[test]
fn client_activity_lists_timestamps() {
    let dir = Dir::new();
    let (program, _, _) = fake_tmux(&dir, "printf '1700000000\\n1700000100\\n'");
    assert_eq!(
        driver("s").with_program(&program).client_activity(&key("A")),
        Ok(vec![1_700_000_000, 1_700_000_100])
    );
}

#[test]
fn the_agent_environment_is_removed_from_every_call() {
    let dir = Dir::new();
    let (program, _, env_log) = fake_tmux(&dir, "exit 0");
    // Even a value set through the driver cannot bring these variables back.
    driver("s")
        .with_program(&program)
        .with_env("CLAUDECODE", "1")
        .with_env("CLAUDE_CODE_ENTRYPOINT", "cli")
        .with_env("TMUX", "/tmp/x,1,0")
        .with_env("TMUX_TMPDIR", "/tmp")
        .send_enter(&key("A"))
        .expect("sent");
    assert_eq!(
        fs::read_to_string(env_log).expect("env").trim(),
        "CLAUDECODE=unset CLAUDE_CODE_ENTRYPOINT=unset TMUX=unset"
    );
}

#[test]
fn a_new_server_under_systemd_is_started_in_its_own_scope() {
    let dir = Dir::new();
    let (program, log, _) = fake_tmux(&dir, "case \"$*\" in *list-sessions*) exit 1;; esac\nexit 0");
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
        .new_session(&key("SVWL"), Path::new("/w"), 80, 24, &strings(&["claude"]))
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
    let names: Vec<String> = calls(&log).iter().map(|c| c[2].clone()).collect();
    assert_eq!(names, ["list-sessions", "new-session", "set-option"]);
}

#[test]
fn a_running_server_is_not_wrapped_again() {
    let dir = Dir::new();
    let (program, log, _) = fake_tmux(&dir, "exit 0");
    let scope_log = dir.path().join("scope");
    let systemd_run = dir.script(
        "systemd-run",
        &format!("echo used > '{}'\nshift 3\nexec \"$@\"", scope_log.display()),
    );
    driver("sylvode")
        .with_program(&program)
        .with_systemd(true)
        .with_systemd_run(&systemd_run)
        .new_session(&key("SVWL"), Path::new("/w"), 80, 24, &strings(&["claude"]))
        .expect("created");
    assert!(!scope_log.exists());
    assert_eq!(calls(&log).len(), 3);
}

#[test]
fn a_slow_tmux_is_killed_at_the_time_limit() {
    let dir = Dir::new();
    let program = dir.script("tmux", "exec /bin/sleep 30");
    let started = Instant::now();
    let result = driver("s")
        .with_program(&program)
        .with_timeout(Duration::from_millis(300))
        .send_enter(&key("A"));
    assert_eq!(result, Err(TmuxError::Timeout));
    assert!(started.elapsed() < Duration::from_secs(5));
}

#[test]
fn a_missing_tmux_is_a_spawn_error() {
    let result = driver("s").with_program("/nonexistent/tmux").send_enter(&key("A"));
    assert!(matches!(result, Err(TmuxError::Spawn(_))), "{result:?}");
}
