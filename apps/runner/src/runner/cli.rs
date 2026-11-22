//! The operator subcommands that look at the runner's sessions: `sessions` and `release KEY`. They
//! read the state directory and tmux directly (the runner need not be running); the decisions are
//! pure functions. Nothing here types into, attaches to or ends a session.

use std::fmt::Write as _;
use std::io::Write as _;
use std::process::ExitCode;

use super::detect::{awaits_choice, input_is_empty, looks_busy};
use super::files::{CreatedBy, SessionRecord, StateDir};
use super::registry::SessionRegistry;
use super::tmux::{PaneId, SESSION_CLIS, SessionName, SocketName, Tmux};
use crate::config::Config;

/// What a session the runner uses is doing, as far as tmux shows.
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
    /// `<KEY>-<cli>`.
    pub name: String,
    pub project_id: String,
    pub cli: String,
    pub state: RowState,
    /// Who started the session, as the runner recorded it.
    pub created_by: CreatedBy,
}

/// The table `sessions` prints.
pub fn render_rows(rows: &[Row]) -> String {
    if rows.is_empty() {
        return "no sessions recorded by this runner\n".to_string();
    }
    let mut out = format!(
        "{:<44} {:<38} {:<9} {:<6} {}\n",
        "NAME", "PROJECT", "CLI", "STATE", "STARTED BY"
    );
    for row in rows {
        let _ = writeln!(
            out,
            "{:<44} {:<38} {:<9} {:<6} {}",
            row.name,
            row.project_id,
            row.cli,
            row.state.word(),
            row.created_by.word()
        );
    }
    out
}

/// The state of a live session from its screen: idle only when the input line is empty AND the CLI
/// is not at work and shows no choice. An empty input box alone says nothing (the CLI shows it
/// while it works), and a stopped runner does not mean an idle CLI.
pub fn live_state(cli: &str, screen: &str) -> RowState {
    if looks_busy(cli, screen) || awaits_choice(cli, screen) {
        return RowState::Busy;
    }
    if input_is_empty(cli, screen) == Some(true) {
        RowState::Idle
    } else {
        RowState::Busy
    }
}

/// `release KEY` frees a registry key only when it is bound and no session of the key exists.
///
/// # Errors
///
/// The text to print when the key cannot be released.
pub fn release_check(key: &str, bound: bool, session_exists: bool) -> Result<(), String> {
    if !bound {
        return Err(format!("project key {key} is not reserved on this runner"));
    }
    if session_exists {
        return Err(format!(
            "a tmux session of {key} still exists; end it first (the runner never ends a session)"
        ));
    }
    Ok(())
}

struct Env {
    state: std::sync::Arc<StateDir>,
    registry: SessionRegistry,
    tmux: Tmux,
}

fn load(config_path: &str) -> Result<Env, String> {
    let config = Config::load(config_path).map_err(|e| format!("failed to load config from {config_path}: {e}"))?;
    let runtime = super::RunnerRuntime::new(&config);
    let state = runtime.state?;
    let registry = runtime.registry?;
    let socket = SocketName::new(&config.runner.tmux.socket_name).map_err(|e| e.to_string())?;
    let mut tmux = Tmux::new(socket);
    if let Some(program) =
        sylvode_prereq::Checker::new(sylvode_prereq::InstallKind::RunnerTmux).locate(&config.runner.tmux.binary)
    {
        tmux = tmux.with_program(program);
    }
    Ok(Env { state, registry, tmux })
}

fn row_for(env: &Env, record: &SessionRecord) -> Option<Row> {
    let name = SessionName::parse(&record.name).ok()?;
    let pane = super::tmux::Pane {
        name: name.clone(),
        id: PaneId::new(&record.pane_id).ok()?,
        pid: record.pane_pid,
    };
    let state = if env.tmux.has_session(&name).unwrap_or(false) {
        match env.tmux.pane_state(&pane) {
            Ok(state) if state.dead => RowState::Dead,
            Ok(state) if state.pid == record.pane_pid && state.session_name == name.as_str() => env
                .tmux
                .capture_pane_styled(&pane, None)
                .map_or(RowState::Busy, |screen| live_state(&record.cli, &screen)),
            Ok(_) | Err(_) => RowState::Gone,
        }
    } else {
        RowState::Gone
    };
    Some(Row {
        name: record.name.clone(),
        project_id: record.project_id.clone(),
        cli: record.cli.clone(),
        state,
        created_by: record.created_by,
    })
}

fn rows(env: &Env) -> Vec<Row> {
    env.state
        .recorded_names()
        .iter()
        .filter_map(|name| SessionName::parse(name).ok())
        .filter_map(|name| env.state.read_session_record(&name))
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

/// `sessions`: lists what the runner recorded and what tmux shows. Changes nothing.
pub fn run_sessions(config_path: &str) -> ExitCode {
    let env = match load(config_path) {
        Ok(env) => env,
        Err(e) => return fail(&e),
    };
    say(&render_rows(&rows(&env)));
    ExitCode::SUCCESS
}

/// `release KEY`.
pub fn run_release(config_path: &str, key_text: &str) -> ExitCode {
    let env = match load(config_path) {
        Ok(env) => env,
        Err(e) => return fail(&e),
    };
    if !super::checks::valid_project_key(key_text) {
        return fail("not a valid project key");
    }
    // The registry is also written by a running runner: release only while none runs.
    let _lock = match env.state.lock() {
        Ok(lock) => lock,
        Err(e) => return fail(&format!("release needs the runner stopped: {e}")),
    };
    let exists = SESSION_CLIS
        .iter()
        .any(|cli| SessionName::new(key_text, cli).is_ok_and(|name| env.tmux.has_session(&name).unwrap_or(true)));
    let bound = env.registry.owner(key_text).is_some();
    if let Err(text) = release_check(key_text, bound, exists) {
        return fail(&text);
    }
    env.registry.release(key_text);
    say(&format!("released project key {key_text}\n"));
    ExitCode::SUCCESS
}
