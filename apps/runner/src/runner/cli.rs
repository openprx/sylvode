//! The operator subcommands that look at the runner's sessions: `sessions [--prune]`,
//! `trust KEY` and `release KEY`. They read the state directory and tmux directly (the runner
//! need not be running); the decisions are pure functions.

use std::fmt::Write as _;
use std::io::Write as _;
use std::process::ExitCode;

use super::detect::input_is_empty;
use super::files::{SessionRecord, StateDir};
use super::registry::SessionRegistry;
use super::tmux::{SessionKey, SocketName, Tmux};
use crate::config::Config;

/// What a runner-owned session is doing, as far as tmux shows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RowState {
    /// The session exists, its pane is alive and the prompt is empty.
    Idle,
    /// The session exists and its pane is alive, but the prompt is not empty or not visible.
    Busy,
    /// The session exists and its pane is dead.
    Dead,
    /// No such session in tmux.
    Gone,
}

impl RowState {
    const fn word(self) -> &'static str {
        match self {
            Self::Idle => "idle",
            Self::Busy => "busy",
            Self::Dead => "dead",
            Self::Gone => "gone",
        }
    }
}

/// One line of `sessions`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Row {
    pub key: String,
    pub project_id: String,
    pub cli: String,
    pub state: RowState,
    pub in_registry: bool,
    pub headless: bool,
    /// The live pane is the one `session.json` names (same pane id and PID).
    pub owned: bool,
}

/// The table `sessions` prints.
pub fn render_rows(rows: &[Row]) -> String {
    if rows.is_empty() {
        return "no sessions created by this runner\n".to_string();
    }
    let mut out = format!("{:<12} {:<38} {:<9} {}\n", "KEY", "PROJECT", "CLI", "STATE");
    for row in rows {
        let note = if row.headless {
            " (project now uses headless)"
        } else {
            ""
        };
        let _ = writeln!(
            out,
            "{:<12} {:<38} {:<9} {}{}",
            row.key,
            row.project_id,
            row.cli,
            row.state.word(),
            note
        );
    }
    out
}

/// A session may be pruned when the runner made it, it is idle, and either its key is no longer in
/// the registry or its project now uses headless. A busy session is never removed.
pub fn prunable(row: &Row) -> bool {
    row.owned && row.state == RowState::Idle && (!row.in_registry || row.headless)
}

/// `release KEY` frees a registry key (A9) only when it is bound and no session exists.
///
/// # Errors
///
/// The text to print when the key cannot be released.
pub fn release_check(key: &str, bound: bool, session_exists: bool) -> Result<(), String> {
    if !bound {
        return Err(format!("session name {key} is not reserved on this runner"));
    }
    if session_exists {
        return Err(format!(
            "tmux session {key} still exists; end it first (`sylvode-runner sessions --prune` removes idle ones)"
        ));
    }
    Ok(())
}

/// What the operator answers in the CLI's dialog, per CLI (the dialog texts are the ones the
/// runner recognises).
pub fn trust_instructions(key: &str, cli: &str, socket: &str) -> String {
    let answer = match cli {
        "claude" => "choose \"Yes, I trust this folder\" (and log in if asked)",
        "codex" => "choose the option that trusts the directory (and log in if asked)",
        _ => "answer the question the CLI shows",
    };
    let attach = attach_command(key, socket);
    format!(
        "Attaching to session {key}. In the CLI, {answer}, then detach with Ctrl-b d.\n\
         The runner never answers these dialogs itself. Command: {attach}\n"
    )
}

/// The attach command line for `key`.
pub fn attach_command(key: &str, socket: &str) -> String {
    if socket.is_empty() {
        format!("tmux attach -t ={key}")
    } else {
        format!("tmux -L {socket} attach -t ={key}")
    }
}

struct Env {
    state: std::sync::Arc<StateDir>,
    registry: SessionRegistry,
    tmux: Tmux,
    socket: String,
}

fn load(config_path: &str) -> Result<Env, String> {
    let config = Config::load(config_path).map_err(|e| format!("failed to load config from {config_path}: {e}"))?;
    let runtime = super::RunnerRuntime::new(&config);
    let state = runtime.state?;
    let registry = runtime.registry?;
    let socket = SocketName::new(&config.runner.tmux.socket_name).map_err(|e| e.to_string())?;
    let name = socket.as_str().to_string();
    let mut tmux = Tmux::new(socket);
    if let Some(program) =
        sylvode_prereq::Checker::new(sylvode_prereq::InstallKind::RunnerTmux).locate(&config.runner.tmux.binary)
    {
        tmux = tmux.with_program(program);
    }
    Ok(Env {
        state,
        registry,
        tmux,
        socket: name,
    })
}

fn row_for(env: &Env, record: &SessionRecord) -> Option<Row> {
    let key = SessionKey::new(&record.key).ok()?;
    let mut owned = false;
    let state = if env.tmux.has_session(&key).unwrap_or(false) {
        env.tmux
            .adopt_pane(&key)
            .ok()
            .map_or(RowState::Gone, |pane| match env.tmux.pane_state(&pane) {
                Ok(state) if state.dead => RowState::Dead,
                Ok(state) => {
                    owned = pane.id.as_str() == record.pane_id && state.pid == record.pane_pid;
                    let empty = env
                        .tmux
                        .capture_pane(&pane, None)
                        .is_ok_and(|screen| input_is_empty(&record.cli, &screen) == Some(true));
                    if empty { RowState::Idle } else { RowState::Busy }
                }
                Err(_) => RowState::Gone,
            })
    } else {
        RowState::Gone
    };
    Some(Row {
        key: record.key.clone(),
        project_id: record.project_id.clone(),
        cli: record.cli.clone(),
        state,
        in_registry: env.registry.owner(&record.key).is_some(),
        headless: record.headless,
        owned,
    })
}

fn rows(env: &Env) -> Vec<Row> {
    env.state
        .recorded_keys()
        .iter()
        .filter_map(|key| SessionKey::new(key).ok())
        .filter_map(|key| env.state.read_session_record(&key))
        .filter_map(|record| row_for(env, &record))
        .collect()
}

/// Writes to standard output; a closed pipe is not an error worth reporting.
fn say(text: &str) {
    let _ = std::io::stdout().lock().write_all(text.as_bytes());
}

fn complain(text: &str) {
    let _ = writeln!(std::io::stderr().lock(), "{text}");
}

fn fail(text: &str) -> ExitCode {
    complain(&format!("error: {text}"));
    ExitCode::FAILURE
}

/// `sessions [--prune]`.
pub fn run_sessions(config_path: &str, prune: bool) -> ExitCode {
    let env = match load(config_path) {
        Ok(env) => env,
        Err(e) => return fail(&e),
    };
    let listed = rows(&env);
    say(&render_rows(&listed));
    if !prune {
        return ExitCode::SUCCESS;
    }
    // A running runner keeps its own picture of its sessions: nothing is ended behind its back.
    let _lock = match env.state.lock() {
        Ok(lock) => lock,
        Err(e) => return fail(&format!("--prune needs the runner stopped: {e}")),
    };
    let mut failed = false;
    for row in listed.iter().filter(|row| prunable(row)) {
        let Ok(key) = SessionKey::new(&row.key) else { continue };
        match env.tmux.kill_session(&key) {
            Ok(()) => say(&format!("pruned {}\n", row.key)),
            Err(e) => {
                complain(&format!("could not prune {}: {e}", row.key));
                failed = true;
            }
        }
    }
    if failed { ExitCode::FAILURE } else { ExitCode::SUCCESS }
}

/// `trust KEY`: prints what to answer, then attaches.
pub fn run_trust(config_path: &str, key_text: &str) -> ExitCode {
    let env = match load(config_path) {
        Ok(env) => env,
        Err(e) => return fail(&e),
    };
    let Ok(key) = SessionKey::new(key_text) else {
        return fail("not a valid session name");
    };
    let Some(record) = env.state.read_session_record(&key) else {
        return fail(&format!("{key_text} is not a session created by this runner"));
    };
    if !env.tmux.has_session(&key).unwrap_or(false) {
        return fail(&format!("tmux session {key_text} does not exist"));
    }
    say(&trust_instructions(key_text, &record.cli, &env.socket));
    match env.tmux.attach_command(&key).status() {
        Ok(status) if status.success() => ExitCode::SUCCESS,
        Ok(_) | Err(_) => fail("tmux attach did not succeed"),
    }
}

/// `release KEY`.
pub fn run_release(config_path: &str, key_text: &str) -> ExitCode {
    let env = match load(config_path) {
        Ok(env) => env,
        Err(e) => return fail(&e),
    };
    let Ok(key) = SessionKey::new(key_text) else {
        return fail("not a valid session name");
    };
    // The registry is also written by a running runner: release only while none runs.
    let _lock = match env.state.lock() {
        Ok(lock) => lock,
        Err(e) => return fail(&format!("release needs the runner stopped: {e}")),
    };
    let exists = env.tmux.has_session(&key).unwrap_or(true);
    let bound = env.registry.owner(key_text).is_some();
    if let Err(text) = release_check(key_text, bound, exists) {
        return fail(&text);
    }
    env.registry.release(key_text);
    say(&format!("released session name {key_text}\n"));
    ExitCode::SUCCESS
}
