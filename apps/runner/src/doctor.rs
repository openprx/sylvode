//! `sylvode-runner doctor`, `--check` and `config check`: what the machine offers the runner, as
//! the shared prerequisite report (`sylvode.doctor.v1`, crate `sylvode-prereq`), and the runner's
//! lazily refreshed copy of it for `/health` and the periodic report.
//!
//! - `doctor [--json] [--config PATH] [--mode tmux|headless|auto] [--as-service] [--project KEY]
//!   [--show-paths]`: exit 0 when no row failed, 2 when one did, 1 when the checker itself could
//!   not run. Warnings never change the exit code.
//! - `--check`: `doctor --json` with no network call and no secret read: what the other binaries
//!   (and the installers) run to learn this one's version and protocol.
//! - `config check [--json] [--config PATH]`: whether the configuration parses and validates,
//!   naming the keys that do not and never echoing a value.
//!
//! The report never holds a token, an URL with credentials or configuration content. Project
//! directories appear as labels unless `--show-paths` is given.

use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::{Duration, Instant};

use parking_lot::Mutex;
use serde_json::{Value, json};
use sylvode_prereq::{
    AgentGuide, Checker, Component, ConfigState, Context, DirectoryState, GeneratedBy, Host, InstallKind, Mode, Peer,
    ProjectCheck, Report, RunOutcome, Status, Trusted, directory_state, path_source, project_component,
};

use crate::config::Config;
use crate::runner::sv_client::TASK_PROTOCOL;

/// How long the runner keeps one snapshot of the prerequisites before it probes again.
pub const CACHE_FOR: Duration = Duration::from_mins(1);

/// The options of `doctor`.
#[derive(Debug, PartialEq, Eq)]
#[allow(clippy::struct_excessive_bools, reason = "independent command-line switches")]
pub struct Options {
    pub json: bool,
    pub config_path: String,
    pub mode: Mode,
    pub as_service: bool,
    pub project: Option<String>,
    pub show_paths: bool,
    /// `--check`: no network, no secrets.
    pub check_only: bool,
}

/// What kind of install the configuration and the mode make this.
pub fn install_kind(config: Option<&Config>, mode: Mode) -> InstallKind {
    let runner_agents = config.is_some_and(|c| c.agents.iter().any(|a| a.agent_type == "runner"));
    match mode {
        Mode::Tmux => InstallKind::RunnerTmux,
        Mode::Headless => InstallKind::RunnerHeadless,
        // The channel is chosen per project in Sylvode, so a runner with agents may be asked for
        // tmux sessions at any time.
        Mode::Auto if runner_agents => InstallKind::RunnerTmux,
        Mode::Auto => InstallKind::RunnerForwardOnly,
    }
}

/// The mode the running service needs: tmux sessions are always possible, so tmux.
pub const fn service_mode() -> Mode {
    Mode::Tmux
}

/// The rows that need no network: tmux, the CLIs, the MCP server and this runner.
pub fn components(config: Option<&Config>, config_state: ConfigState, mode: Mode, checker: &Checker) -> Vec<Component> {
    let runner_agents = config.is_some_and(|c| c.agents.iter().any(|a| a.agent_type == "runner"));
    let socket = config.map_or("", |c| c.runner.tmux.socket_name.as_str());
    let mut rows = Vec::new();
    let mut tmux = checker.check_tmux(socket);
    if sylvode_prereq::in_container() && runner_agents {
        tmux.status = Status::Fail;
        tmux.found = false;
        tmux.hint = Some("tmux: unsupported in container image".to_string());
        tmux = tmux.with_detail("state", json!("unsupported in container image"));
    }
    rows.push(tmux);
    let claude = config.map(|c| PathBuf::from(&c.runner.cli.claude.binary));
    let codex = config.map(|c| PathBuf::from(&c.runner.cli.codex.binary));
    let opencode = config.map(|c| PathBuf::from(&c.runner.cli.opencode.binary));
    let selected_claude = runner_agents || mode != Mode::Auto;
    let selected_codex = config.is_some_and(|c| c.runner.cli.codex.enabled);
    let selected_opencode = config.is_some_and(|c| c.runner.cli.opencode.enabled);
    rows.push(checker.check_cli(sylvode_prereq::Cli::Claude, selected_claude, claude.as_deref()));
    rows.push(checker.check_cli(sylvode_prereq::Cli::Codex, selected_codex, codex.as_deref()));
    rows.push(checker.check_cli(sylvode_prereq::Cli::Opencode, selected_opencode, opencode.as_deref()));
    let server = config.map(|c| PathBuf::from(&c.runner.mcp.server_binary));
    rows.push(checker.check_peer(Peer::McpServer, server.as_deref(), Some(TASK_PROTOCOL)));
    rows.push(runner_row(config, config_state, checker));
    rows
}

/// The `runner` row: this binary, its configuration, its service and (on Linux) linger.
fn runner_row(config: Option<&Config>, config_state: ConfigState, checker: &Checker) -> Component {
    let (service, linger) = service_state(checker);
    let config_word = match config_state {
        ConfigState::Found => "parsed",
        ConfigState::Missing => "missing",
        ConfigState::Invalid => "invalid",
    };
    let status = if config_state == ConfigState::Invalid {
        Status::Fail
    } else {
        Status::Ok
    };
    let mut row = Component::new("runner", true, status, false)
        .with_detail("config", json!(config_word))
        .with_detail("service", json!(service))
        .with_detail("linger", json!(linger));
    // The posture: the launch command and headless arguments, and whether they run with
    // permission checks off (the defaults do).
    if let Some(config) = config {
        let postures = crate::runner::session_report::postures_of(&config.runner);
        let claude = &config.runner.cli.claude;
        let codex = &config.runner.cli.codex;
        let bypass: serde_json::Map<String, Value> = postures
            .iter()
            .map(|(cli, posture)| {
                (
                    (*cli).to_string(),
                    json!({"tmux": posture.tmux, "headless": posture.headless}),
                )
            })
            .collect();
        row = row
            .with_detail("launch_command", json!(claude.launch_command))
            .with_detail("headless_args", json!(claude.headless_args))
            .with_detail("bypass", Value::Object(bypass));
        if codex.enabled {
            row = row
                .with_detail("codex_launch_command", json!(codex.launch_command))
                .with_detail("codex_headless_args", json!(codex.headless_args));
        }
        if postures.iter().any(|(_, posture)| posture.tmux || posture.headless) && row.status == Status::Ok {
            row.status = Status::Warn;
            row.hint = Some(
                "permission checks are off in the launch command or the headless arguments: tasks run with every \
                 permission of this user"
                    .to_string(),
            );
        }
    }
    row = mark_insecure_transport(row, config);
    row.version = Some(env!("CARGO_PKG_VERSION").to_string());
    row.path = std::env::current_exe().ok().map(|p| p.to_string_lossy().into_owned());
    if config_state == ConfigState::Invalid {
        row.hint = Some("the configuration does not load; run `sylvode-runner config check`".to_string());
    }
    row
}

/// An acknowledged plain-http server address outside loopback is a warning on the `runner` row:
/// the bot tokens and every task cross the network in clear text unless the network is encrypted.
pub fn mark_insecure_transport(mut row: Component, config: Option<&Config>) -> Component {
    if !config.is_some_and(Config::insecure_transport_acknowledged) {
        return row;
    }
    row = row.with_detail("insecure_transport", json!(true));
    if row.status == Status::Ok {
        row.status = Status::Warn;
    }
    let note = "the server address is http:// outside loopback (acknowledged with \
                ai_task_callback.allow_insecure_transport): bot tokens and tasks are in clear text unless the \
                network is encrypted";
    row.hint = Some(
        row.hint
            .take()
            .map_or_else(|| note.to_string(), |existing| format!("{existing}; {note}")),
    );
    row
}

/// `(service, linger)` as the report words them.
fn service_state(checker: &Checker) -> (&'static str, &'static str) {
    let output = |program: &str, args: &[&str]| -> Option<(Option<i32>, String)> {
        let path = checker.locate(program)?;
        match checker.run(&path, args) {
            RunOutcome::Exited { code, stdout } => Some((code, stdout)),
            _ => None,
        }
    };
    if cfg!(target_os = "macos") {
        let target = format!("gui/{}/dev.sylvode.runner", nix::unistd::Uid::effective().as_raw());
        return match output("launchctl", &["print", &target]) {
            Some((Some(0), text)) if text.contains("state = running") => ("registered-running", "n/a"),
            Some((Some(0), _)) => ("registered-stopped", "n/a"),
            _ => ("not-registered", "n/a"),
        };
    }
    let service = match output("systemctl", &["--user", "is-active", "sylvode-runner"]) {
        Some((_, text)) if text.trim() == "active" => "registered-running",
        _ => match output("systemctl", &["--user", "is-enabled", "sylvode-runner"]) {
            Some((Some(0), _)) => "registered-stopped",
            Some((_, text)) if matches!(text.trim(), "disabled" | "enabled") => "registered-stopped",
            _ => "not-registered",
        },
    };
    let user = std::env::var("USER").unwrap_or_default();
    let linger = match output("loginctl", &["show-user", &user, "-p", "Linger"]) {
        Some((_, text)) if text.contains("Linger=yes") => "yes",
        Some((_, text)) if text.contains("Linger=no") => "no",
        _ => "n/a",
    };
    (service, linger)
}

/// The `api_protocol` row from `GET /api/v1/ai/protocol` with the first configured bot's token.
pub async fn api_component(config: &Config, http: &reqwest::Client) -> Option<Component> {
    use sylvode_prereq::{ComponentKind, Finding, severity};
    let target = crate::handshake::targets(config).into_iter().next()?;
    let client = crate::runner::sv_client::SvClient::new(http.clone(), &target.api_url, &target.token, &target.surface);
    let answer = client.protocol(Duration::from_secs(5)).await;
    let kind = install_kind(Some(config), service_mode());
    let unreachable = (
        Finding::Unparsable,
        None,
        None,
        Some("Sylvode did not answer the protocol call".to_string()),
    );
    let (finding, version, protocol, hint) = answer.as_ref().map_or(unreachable, |data| {
        let api = data.get("task_protocol").and_then(Value::as_u64).unwrap_or(0);
        let version = data.get("api_version").and_then(Value::as_str).map(str::to_string);
        if api == u64::from(TASK_PROTOCOL) {
            (Finding::Healthy, version, Some(api), None)
        } else {
            (
                Finding::ProtocolMismatch,
                version,
                Some(api),
                Some(format!("task protocol mismatch: runner {TASK_PROTOCOL}, api {api}")),
            )
        }
    });
    let status = severity(kind, ComponentKind::ApiProtocol, finding);
    let mut row = Component::new("api_protocol", answer.is_ok(), status, true);
    row.version = version;
    row.hint = hint;
    if let Some(protocol) = protocol {
        row = row.with_detail("task_protocol", json!(protocol));
    }
    Some(row)
}

/// The `project.<KEY>` row from what the runner itself knows (its `session.json` for the key).
pub fn project_row(config: &Config, key: &str, show_paths: bool) -> Component {
    let runtime_state = crate::runner::state_dir(config, crate::runner::paths::service_user_home().as_deref());
    let state = runtime_state.and_then(|dir| crate::runner::files::StateDir::open(&dir).ok());
    // The first recorded CLI of the key describes the project (claude before codex).
    let record = state.and_then(|state| {
        crate::runner::tmux::SESSION_CLIS.iter().find_map(|cli| {
            crate::runner::tmux::Slot::project(key, cli)
                .ok()
                .and_then(|name| state.read_session_record(&name))
        })
    });
    let Some(record) = record else {
        let check = ProjectCheck {
            key,
            directory: DirectoryState::Missing,
            trusted: Trusted::Unknown,
            agent_guide: AgentGuide::Absent,
            directory_path: None,
        };
        return project_component(&check, show_paths)
            .with_hint("the runner has no session record for this project key yet (no task ran)");
    };
    let dir = PathBuf::from(&record.dir);
    let roots: Vec<PathBuf> = config.runner.allowed_roots.iter().map(PathBuf::from).collect();
    let root_refs: Vec<&Path> = roots.iter().map(PathBuf::as_path).collect();
    let guide = match crate::runner::guide::guide_state(
        &dir,
        &record.cli,
        &crate::runner::guide::EMBEDDED,
        env!("CARGO_PKG_VERSION"),
    ) {
        "current" => AgentGuide::Current,
        "stale" => AgentGuide::Stale,
        "conflict" => AgentGuide::Conflict,
        _ => AgentGuide::Absent,
    };
    let check = ProjectCheck {
        key,
        directory: directory_state(&dir, &root_refs),
        trusted: crate::runner::paths::service_user_home()
            .map_or(Trusted::Unknown, |home| trust_state(&home, &record.cli, &dir)),
        agent_guide: guide,
        directory_path: Some(&dir),
    };
    project_component(&check, show_paths)
}

/// Whether the CLI trusts the directory, read (never written) from the CLI's own files under
/// `home`: claude's `~/.claude.json` (`projects["<dir>"].hasTrustDialogAccepted`) and codex's
/// `~/.codex/config.toml` (`projects."<dir>".trust_level`, `trusted` or not). A missing file, key
/// or CLI is `unknown`.
#[must_use]
pub fn trust_state(home: &Path, cli: &str, dir: &Path) -> Trusted {
    let Some(dir) = dir.to_str() else {
        return Trusted::Unknown;
    };
    match cli {
        "claude" => std::fs::read_to_string(home.join(".claude.json"))
            .ok()
            .and_then(|text| serde_json::from_str::<Value>(&text).ok())
            .and_then(|value| {
                value
                    .pointer(&format!("/projects/{}/hasTrustDialogAccepted", pointer_escape(dir)))
                    .cloned()
            })
            .and_then(|accepted| accepted.as_bool())
            .map_or(
                Trusted::Unknown,
                |accepted| if accepted { Trusted::Yes } else { Trusted::No },
            ),
        "codex" => std::fs::read_to_string(home.join(".codex").join("config.toml"))
            .ok()
            .and_then(|text| text.parse::<toml::Table>().ok())
            .and_then(|table| {
                table
                    .get("projects")?
                    .get(dir)?
                    .get("trust_level")?
                    .as_str()
                    .map(|level| if level == "trusted" { Trusted::Yes } else { Trusted::No })
            })
            .unwrap_or(Trusted::Unknown),
        _ => Trusted::Unknown,
    }
}

/// A JSON pointer token: `~` and `/` escaped.
fn pointer_escape(token: &str) -> String {
    token.replace('~', "~0").replace('/', "~1")
}

/// The session rows: `session.<KEY>-<cli>` for what the runner can see of the project's derived
/// tmux sessions, one per CLI the runner has a record of (`claude` when it has none), and
/// `session.bot-<uuid>-<cli>` (and `-<n>` for each further pool member) for every
/// `[[runner.tmux.sessions]]` entry, with or without a record. No project row for a key that is not a project key.
pub fn session_rows(config: &Config, key: &str) -> Vec<Component> {
    let state = crate::runner::state_dir(config, crate::runner::paths::service_user_home().as_deref())
        .and_then(|dir| crate::runner::files::StateDir::open(&dir).ok());
    let mut clis: Vec<&str> = crate::runner::tmux::SESSION_CLIS
        .iter()
        .copied()
        .filter(|cli| {
            state.as_ref().is_some_and(|state| {
                crate::runner::tmux::Slot::project(key, cli)
                    .ok()
                    .is_some_and(|name| state.read_session_record(&name).is_some())
            })
        })
        .collect();
    if clis.is_empty() {
        clis.push("claude");
    }
    let projects = clis
        .into_iter()
        .filter_map(|cli| crate::runner::tmux::Slot::project(key, cli).ok())
        .map(|slot| session_row(config, &slot));
    let bots = config
        .runner
        .tmux
        .sessions
        .iter()
        .flat_map(|entry| {
            (0..entry.names.len())
                .filter_map(|member| u8::try_from(member).ok())
                .filter_map(|member| crate::runner::tmux::Slot::bot_member(&entry.bot, &entry.cli, member).ok())
        })
        .map(|slot| session_row(config, &slot));
    projects.chain(bots).collect()
}

/// The `session.<slot>` row of a project slot (`<KEY>-<cli>`) or a bot slot (`bot-<uuid>-<cli>`).
pub fn session_row(config: &Config, name: &crate::runner::tmux::Slot) -> Component {
    use crate::runner::session_report::{SessionState, assess_named};
    let cli = name.cli();
    let component = format!("session.{}", name.as_str());
    let Some(session) = assess_named(config, name) else {
        return Component::new(component, false, Status::Ok, false)
            .with_detail("state", json!("unknown"))
            .with_hint("tmux or the runner's state directory is not available to look at the session");
    };
    let mut row = Component::new(component, session.state != SessionState::Absent, session.status, false)
        .with_detail("state", json!(session.state.word()));
    if let Some(created_by) = session.created_by {
        row = row.with_detail("created_by", json!(created_by.word()));
    }
    if let Some(bot) = name.bot_id() {
        row = row.with_detail("bot", json!(bot));
    }
    row = row
        .with_detail("tmux_session", json!(session.tmux.as_str()))
        .with_detail("name_source", json!(session.source.as_str()));
    row = row.with_detail("pane", json!(session.pane_id.as_deref().unwrap_or("none")));
    if session.state == SessionState::Ambiguous {
        row = row.with_detail("pane", json!("ambiguous"));
    }
    if let Some(foreground) = &session.foreground {
        row = row.with_detail("foreground", json!(foreground));
    }
    if let Some(hooks) = session.hooks {
        let seen = session
            .hooks_seen
            .and_then(|at| at.elapsed().ok())
            .map_or_else(|| "none".to_string(), |ago| format!("seen {}s ago", ago.as_secs()));
        row = row.with_detail("hooks", json!(if hooks { seen } else { "none".to_string() }));
    }
    if let Some(permissions) = session.permissions {
        row = row.with_detail("permissions", json!(permissions.word()));
    }
    if let Some(mcp) = session.mcp {
        row = row.with_detail("mcp", json!(mcp.word()));
    }
    let derived = session.source == crate::runner::tmux::NameSource::Derived;
    let name = session.tmux.as_str();
    row.hint = match session.state {
        SessionState::CliNotRunning if derived => Some(format!(
            "attach with tmux attach -t {name} and start the CLI, or end the session so the runner starts it"
        )),
        SessionState::CliNotRunning => Some(format!("attach with tmux attach -t {name} and start the CLI")),
        SessionState::Ambiguous => Some(format!("tmux session {name} has more than one pane running the CLI")),
        SessionState::Dialog => Some(format!(
            "the CLI shows a dialog; attach with tmux attach -t {name} and answer it"
        )),
        SessionState::Absent if derived => Some("not started: the runner starts it with the next task".to_string()),
        SessionState::Absent => Some(format!(
            "tmux session {name} does not exist; start it and run {cli} in it: the runner does not create sessions with a configured name"
        )),
        SessionState::Running => match (session.mcp, session.permissions) {
            (Some(crate::runner::session_report::Mcp::Other), _) if !derived => {
                Some("the session uses its own sylvode entry; it must hold this bot's token".to_string())
            }
            (Some(crate::runner::session_report::Mcp::Missing | crate::runner::session_report::Mcp::Other), _) => {
                Some(format!(
                    "the CLI has no Sylvode MCP server for this bot: run `sylvode-runner setup {cli} --project KEY`"
                ))
            }
            (Some(crate::runner::session_report::Mcp::Stale), _) => {
                Some("the credentials changed after the CLI started: restart it to load them".to_string())
            }
            (_, Some(crate::runner::session_report::Permissions::Bypass)) => {
                Some("the CLI runs with permission checks off".to_string())
            }
            _ if session.hooks == Some(false) => {
                Some("no hook seen from this pane: turn tracking is from the screen only".to_string())
            }
            _ => row.hint.take(),
        },
    };
    row
}

/// The worktree warning of the guide installer: a project whose `.git` is a file cannot have its
/// exclude line written (it belongs to the main repository), so the skill directory could be
/// committed. Reported for the project's directory when it is a git worktree.
pub fn worktree_warning(dir: &Path) -> Option<Component> {
    let outcome = crate::runner::guide::ensure_git_exclude_check(dir)?;
    let hint = outcome.doctor_warning()?;
    Some(Component::new("agent_guide.git_exclude", true, Status::Warn, false).with_hint(hint))
}

/// Builds the whole report.
pub fn report(options: &Options, config: Option<&Config>, state: ConfigState, api: Option<Component>) -> Report {
    let checker = Checker::new(install_kind(config, options.mode));
    let mut rows = components(config, state, options.mode, &checker);
    if let Some(api) = api {
        rows.push(api);
    }
    if let (Some(config), Some(key)) = (config, &options.project) {
        rows.push(project_row(config, key, options.show_paths));
        rows.extend(session_rows(config, key));
        let dir_known = rows.last().and_then(|r| {
            r.detail
                .get("directory_path")
                .and_then(Value::as_str)
                .map(PathBuf::from)
        });
        if let Some(warning) = dir_known.as_deref().and_then(worktree_warning) {
            rows.push(warning);
        }
    }
    let host = Host::detect(&checker);
    let context = Context {
        mode: options.mode,
        as_service: options.as_service,
        path_source: path_source(options.as_service),
        config: state,
    };
    Report::new(GeneratedBy::new("sylvode-runner", TASK_PROTOCOL), host, context, rows)
}

/// Runs `doctor` (or `--check`) and prints the report.
pub fn run(options: &Options) -> ExitCode {
    let (config, state) = match Config::load(&options.config_path) {
        Ok(config) => (Some(config), ConfigState::Found),
        Err(_) if !Path::new(&options.config_path).exists() => (None, ConfigState::Missing),
        Err(_) => (None, ConfigState::Invalid),
    };
    let api = if options.check_only {
        None
    } else {
        config.as_ref().and_then(|config| {
            let http = crate::runner::build_http_client(&config.runner.http).ok()?;
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .ok()?;
            runtime.block_on(api_component(config, &http))
        })
    };
    let report = report(options, config.as_ref(), state, api);
    let text = if options.json {
        match report.to_json() {
            Ok(json) => json,
            Err(_) => return ExitCode::from(1),
        }
    } else {
        report.to_table()
    };
    let mut out = std::io::stdout().lock();
    if std::io::Write::write_all(&mut out, format!("{text}\n").as_bytes()).is_err() {
        return ExitCode::from(1);
    }
    ExitCode::from(u8::try_from(report.exit_code()).unwrap_or(1))
}

/// Removes what a parser message quotes. A message about a wrong value quotes the value (in
/// quotes or backticks): both go. A message about an unknown key quotes key names in backticks,
/// which stay; only quoted strings go.
fn without_values(message: &str) -> String {
    let wrong_value = message.starts_with("invalid");
    let mut out = String::new();
    let mut open: Option<char> = None;
    for c in message.chars() {
        let quote = c == '"' || (c == '`' && wrong_value);
        if quote {
            match open {
                Some(o) if o == c => {
                    out.push_str("<value>");
                    open = None;
                }
                None => open = Some(c),
                Some(_) => {}
            }
            continue;
        }
        if open.is_none() {
            out.push(c);
        }
    }
    out
}

/// The `[table]` and the key of the line that holds `offset` in `text`, for an error message that
/// names where without saying what was written.
fn locate(text: &str, offset: usize) -> (Option<String>, Option<String>) {
    let offset = offset.min(text.len());
    let start = text[..offset].rfind('\n').map_or(0, |at| at + 1);
    let line = text[start..].lines().next().unwrap_or("");
    let key = line
        .split_once('=')
        .map(|(k, _)| k.trim().to_string())
        .filter(|k| !k.is_empty());
    let table = text[..start]
        .lines()
        .rev()
        .map(str::trim)
        .find(|l| l.starts_with('[') && l.ends_with(']'))
        .map(|l| l.trim_matches(['[', ']']).to_string());
    (table, key)
}

/// `config check`: whether the file loads. Errors name keys and never carry a value.
pub fn config_check(path: &str, json: bool) -> ExitCode {
    let result = std::fs::read_to_string(path)
        .map_err(|e| format!("cannot read the configuration: {}", e.kind()))
        .and_then(|text| {
            // Parse first, for the location of a syntax, type or key error; then validate.
            toml::from_str::<Config>(&text)
                .map_err(|e| {
                    let (table, key) = e.span().map_or((None, None), |s| locate(&text, s.start));
                    let line = e
                        .span()
                        .map(|s| text[..s.start.min(text.len())].matches('\n').count() + 1);
                    let at = match (table, key, line) {
                        (table, Some(key), line) => format!(
                            " (key `{key}`{}{})",
                            table.map_or_else(String::new, |t| format!(" in [{t}]")),
                            line.map_or_else(String::new, |l| format!(", line {l}"))
                        ),
                        (_, None, Some(line)) => format!(" (line {line})"),
                        _ => String::new(),
                    };
                    format!("{}{at}", without_values(e.message()))
                })
                .and_then(|_| {
                    Config::from_toml(&text)
                        .map(|_| ())
                        .map_err(|e| without_values(&strip_excerpt(&e)))
                })
        });
    let ok = result.is_ok();
    let errors: Vec<String> = result.err().into_iter().collect();
    let body = json!({"schema": "sylvode.config-check.v1", "ok": ok, "errors": errors});
    let text = if json {
        body.to_string()
    } else if ok {
        "configuration ok".to_string()
    } else {
        errors.join("\n")
    };
    let mut out = std::io::stdout().lock();
    let _ = std::io::Write::write_all(&mut out, format!("{text}\n").as_bytes());
    ExitCode::from(if ok { 0 } else { 2 })
}

/// A toml error text carries the offending source lines after the first line; only the first
/// (the message) is kept.
fn strip_excerpt(text: &str) -> String {
    text.lines()
        .filter(|l| !l.trim_start().starts_with('|') && !l.contains(" | "))
        .collect::<Vec<_>>()
        .join(" ")
}

/// A lazily refreshed copy of the component rows, for `/health` and the periodic report. Probing
/// runs programs, so it happens at most once per [`CACHE_FOR`], and not while holding anything else.
#[derive(Default)]
pub struct Prereq {
    cached: Mutex<Option<(Instant, Vec<Component>)>>,
}

impl Prereq {
    /// The rows as of at most [`CACHE_FOR`] ago; `probe` makes them when they are older.
    pub fn rows_at(&self, now: Instant, probe: impl FnOnce() -> Vec<Component>) -> Vec<Component> {
        let mut cached = self.cached.lock();
        if let Some((at, rows)) = cached.as_ref()
            && now.duration_since(*at) < CACHE_FOR
        {
            return rows.clone();
        }
        let rows = probe();
        *cached = Some((now, rows.clone()));
        rows
    }

    /// The current rows for the running service (blocking: it runs programs).
    pub fn rows(&self, config: &Config) -> Vec<Component> {
        self.rows_at(Instant::now(), || {
            let mode = service_mode();
            let checker = Checker::new(install_kind(Some(config), mode));
            components(Some(config), ConfigState::Found, mode, &checker)
        })
    }
}
