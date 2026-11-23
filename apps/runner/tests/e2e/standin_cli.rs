// A stand-in for the CLI, compiled by the e2e tests with `rustc` and installed as `claude` (and, as a
// copy, as `codex`: the program name selects the flavour), so the pane's foreground process really
// is called what the runner expects. It does what the CLI does that the runner
// depends on: it turns bracketed paste on, reads its hook commands from the settings file named by
// `--settings` and runs them at the right moments, and draws screens on command.
//
// The codex flavour has no settings file: it reads the notify program from `-c notify=[...]` (the
// runner's state directory is the parent of the `--socket` path in it), draws codex's screens
// (`›` composer with a faint placeholder, `[Pasted Content N chars]`, `• Working (Ns • esc to
// interrupt)`) and at the end of a turn runs the notify program with one JSON argument the way
// codex does: first a notification without `client` (a sub-thread), then the main one. An Enter
// within 400 ms of a paste does not submit (codex inserts a newline).
//
// Control files live in the runner's state directory (three levels above the settings file):
//   standin.cfg   `turn=forever` (a turn never ends), `trust=dialog` (a trust dialog first)
//   standin.mode  idle (default) | busy | placeholder | permission | elicitation | draft | deaf | shell
// What it sees is appended to `standin.log`; the NAMES of its environment variables go to
// `standin.env.names` and the values of the test-owned variables (`SVT_*`) and of
// `CLAUDE_CONFIG_DIR` to `standin.env.values`. No other value is ever written anywhere. It writes
// nothing at all without a `--settings` argument (the runner's probes call it without one), so a
// run can never leave files in the working directory.

use std::fs::OpenOptions;
use std::io::{BufRead, Write};
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

fn append(path: &Path, line: &str) {
    if path.as_os_str().is_empty() {
        return;
    }
    if let Ok(mut file) = OpenOptions::new().create(true).append(true).open(path) {
        let _ = writeln!(file, "{line}");
    }
}

fn mode(state: &Path) -> String {
    std::fs::read_to_string(state.join("standin.mode"))
        .unwrap_or_default()
        .trim()
        .to_string()
}

/// The hook command registered for `event` in the settings file.
fn hook_command(settings: &str, event: &str) -> Option<String> {
    let from = settings.find(&format!("\"{event}\""))?;
    let rest = &settings[from..];
    let key = rest.find("\"command\": \"")? + "\"command\": \"".len();
    let mut out = String::new();
    let mut chars = rest[key..].chars();
    while let Some(c) = chars.next() {
        match c {
            '"' => return Some(out),
            '\\' => match chars.next() {
                Some('n') => out.push('\n'),
                Some(other) => out.push(other),
                None => return None,
            },
            other => out.push(other),
        }
    }
    None
}

/// The words of `-c notify=[...]`, a TOML array of strings.
fn codex_notify(args: &[String]) -> Vec<String> {
    let Some(value) = args
        .windows(2)
        .find(|pair| pair[0] == "-c" && pair[1].starts_with("notify="))
        .map(|pair| pair[1]["notify=".len()..].to_string())
    else {
        return Vec::new();
    };
    let mut words = Vec::new();
    let mut chars = value.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '"' || c == '\'' {
            let mut word = String::new();
            while let Some(d) = chars.next() {
                if d == c {
                    break;
                }
                if d == '\\' && c == '"' {
                    if let Some(e) = chars.next() {
                        word.push(e);
                    }
                } else {
                    word.push(d);
                }
            }
            words.push(word);
        }
    }
    words
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let codex = Path::new(&args[0]).file_name().and_then(|n| n.to_str()) == Some("codex");
    let notify = if codex { codex_notify(&args) } else { Vec::new() };
    let settings_path = args
        .iter()
        .position(|a| a == "--settings")
        .and_then(|i| args.get(i + 1))
        .cloned()
        .unwrap_or_default();
    let state: PathBuf = if codex {
        notify
            .iter()
            .position(|w| w == "--socket")
            .and_then(|at| notify.get(at + 1))
            .and_then(|socket| Path::new(socket).parent().map(Path::to_path_buf))
            .unwrap_or_default()
    } else {
        Path::new(&settings_path)
            .parent()
            .and_then(Path::parent)
            .and_then(Path::parent)
            .map(Path::to_path_buf)
            .unwrap_or_default()
    };
    if args.iter().any(|a| a == "--version") {
        println!("{}", if codex { "codex-cli 0.171.0" } else { "2.1.330 (Claude Code)" });
        return;
    }
    // Without a settings file there is no state directory to write to: write nothing.
    let log = if state.as_os_str().is_empty() {
        PathBuf::new()
    } else {
        state.join("standin.log")
    };
    append(&log, &format!("argv: {}", args[1..].join(" ")));
    if !state.as_os_str().is_empty() {
        let mut names: Vec<String> = std::env::vars().map(|(k, _)| k).collect();
        names.sort();
        for name in names {
            append(&state.join("standin.env.names"), &name);
        }
        for (k, v) in std::env::vars() {
            if k.starts_with("SVT_") || k == "CLAUDE_CONFIG_DIR" {
                append(&state.join("standin.env.values"), &format!("{k}={v}"));
            }
        }
    }
    let cfg = std::fs::read_to_string(state.join("standin.cfg")).unwrap_or_default();
    let turn_forever = cfg.contains("turn=forever");
    let settings = std::fs::read_to_string(&settings_path).unwrap_or_default();
    let fire = {
        let state = state.clone();
        move |event: &str| {
            // A deaf CLI reports nothing.
            if mode(&state) == "deaf" && event != "SessionStart" {
                return;
            }
            if codex {
                // Only the end of a turn is reported, as codex's notify does.
                if event == "Stop" && !notify.is_empty() {
                    let run = |payload: &str| {
                        let _ = Command::new(&notify[0])
                            .args(&notify[1..])
                            .arg(payload)
                            .stdin(Stdio::null())
                            .status();
                    };
                    run("{\"type\":\"agent-turn-complete\",\"thread-id\":\"sub\",\"last-assistant-message\":\"x\"}");
                    run("{\"type\":\"approval-requested\",\"client\":\"codex-tui\"}");
                    run("{\"type\":\"agent-turn-complete\",\"thread-id\":\"main\",\"client\":\"codex-tui\",\"last-assistant-message\":\"x\"}");
                }
                return;
            }
            if let Some(command) = hook_command(&settings, event) {
                let _ = Command::new("/bin/sh")
                    .arg("-c")
                    .arg(command)
                    .stdin(Stdio::null())
                    .status();
            }
        }
    };
    print!("\x1b[?2004h");
    let _ = std::io::stdout().flush();
    let stdin = std::io::stdin();
    let mut lines = stdin.lock().lines();
    if cfg.contains("trust=dialog") && codex {
        print!("\x1b[2J\x1b[H  Folder access\r\n  Trust this folder? Codex can read, edit, and run files here.\r\n\u{203a} 1. Trust and continue\r\n  2. Back to Agent Command Center\r\n  enter continue \u{b7} esc back\r\n");
        let _ = std::io::stdout().flush();
        let _ = lines.next();
        append(&log, "TRUST answered");
    } else if cfg.contains("trust=dialog") {
        print!("\x1b[2J\x1b[HIs this a project you created or one you trust?\r\n  Yes, I trust this folder\r\n");
        let _ = std::io::stdout().flush();
        let _ = lines.next();
        append(&log, "TRUST answered");
    }
    let turn_active = Arc::new(AtomicBool::new(false));
    let pasted: Arc<Mutex<Option<String>>> = Arc::new(Mutex::new(None));
    {
        let (state, turn_active, pasted) = (state.clone(), Arc::clone(&turn_active), Arc::clone(&pasted));
        let codex = codex;
        std::thread::spawn(move || {
            let mut last = String::new();
            loop {
                let wanted = mode(&state);
                if wanted == "shell" {
                    let _ = Command::new("/bin/cat").exec();
                }
                let composer = "\x1b[1m\u{203a}\x1b[0m \x1b[2mAsk Codex to do anything\x1b[0m";
                let working = "\u{2022} Working (2s \u{2022} esc to interrupt)";
                let screen = if codex {
                    match wanted.as_str() {
                        "busy" => format!("{working}\r\n{composer}"),
                        "permission" => "  Would you like to run the following command?\r\n  $ ls\r\n\u{203a} 1. Yes, proceed (y)\r\n  2. No, and tell Codex what to do differently (esc)\r\n  Press enter to confirm or esc to cancel".to_string(),
                        "draft" => "\u{203a} half written".to_string(),
                        _ => {
                            if let Some(text) = pasted.lock().ok().and_then(|p| p.clone()) {
                                format!("\u{203a} [Pasted Content {} chars]", text.len())
                            } else if turn_active.load(Ordering::SeqCst) {
                                format!("{working}\r\n{composer}")
                            } else {
                                composer.to_string()
                            }
                        }
                    }
                } else {
                    match wanted.as_str() {
                    "busy" => "\u{273b} Working\u{2026} (esc to interrupt)\r\n> ".to_string(),
                    "placeholder" => "> Try \"fix lint errors\"".to_string(),
                    "permission" => "Do you want to proceed?\r\n \u{276f} 1. Yes\r\n   2. No\r\n Esc to cancel".to_string(),
                    "elicitation" => {
                        "Which option?\r\n \u{276f} 1. Alpha\r\n   2. Beta\r\n Enter to select \u{b7} Esc to cancel".to_string()
                    }
                    "draft" => "> half written".to_string(),
                    _ => {
                        if let Some(text) = pasted.lock().ok().and_then(|p| p.clone()) {
                            format!("> {text}")
                        } else if turn_active.load(Ordering::SeqCst) {
                            "\u{273b} Working\u{2026} (esc to interrupt)\r\n> ".to_string()
                        } else {
                            "> ".to_string()
                        }
                    }
                    }
                };
                if screen != last {
                    print!("\x1b[2J\x1b[H{screen}");
                    let _ = std::io::stdout().flush();
                    last = screen;
                }
                std::thread::sleep(Duration::from_millis(150));
            }
        });
    }
    fire("SessionStart");
    let end = "\x1b[201~";
    let mut text = String::new();
    let mut in_paste = false;
    while let Some(Ok(line)) = lines.next() {
        if !in_paste && line.is_empty() {
            // An Enter with nothing pasted: the retry the runner makes.
            append(&log, "ENTER");
            continue;
        }
        if !text.is_empty() {
            text.push('|');
        }
        text.push_str(&line);
        in_paste = true;
        if line.contains(end) {
            in_paste = false;
            let done = std::mem::take(&mut text);
            let first = done.split('|').next().unwrap_or("").replace("\x1b[200~", "").replace(end, "");
            if let Ok(mut p) = pasted.lock() {
                *p = Some(first);
            }
            append(&log, &format!("TEXT: {done}"));
            if mode(&state) == "deaf" {
                // The text sits in the input box, never submitted, until an Enter comes.
                continue;
            }
            submit(&fire, &turn_active, &pasted, turn_forever);
        }
    }
}

fn submit(fire: &impl Fn(&str), turn_active: &Arc<AtomicBool>, pasted: &Arc<Mutex<Option<String>>>, forever: bool) {
    if let Ok(mut p) = pasted.lock() {
        *p = None;
    }
    turn_active.store(true, Ordering::SeqCst);
    fire("UserPromptSubmit");
    if !forever {
        std::thread::sleep(Duration::from_secs(1));
        turn_active.store(false, Ordering::SeqCst);
        fire("Stop");
    }
}
