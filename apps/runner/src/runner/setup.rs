//! `sylvode-runner setup <claude|codex> --project KEY [--config P] [--dir D] [--bot ID] [--apply]`
//! (design 3.4): gives a CLI the operator started the same Sylvode MCP entry and hooks that a
//! session the runner creates gets. The text below describes claude; codex is at the end of the
//! file (`setup codex`: the same command line printed, `codex mcp add` with `--apply`, no hooks
//! written because codex has one `notify` program).
//!
//! - Without `--apply` it only prints: the session name, the `tmux` line that creates it, the full
//!   launch line, the MCP entry and the hooks. It writes nothing and shows no token, only the
//!   paths of files that hold one.
//! - With `--apply` it adds the `sylvode` MCP server with `claude mcp add-json --scope local` run
//!   in the directory (an entry that points elsewhere is reported and left alone) and merges the
//!   hooks and the permissions for the Sylvode tools into `<dir>/.claude/settings.local.json`.
//!   Hooks written by the runner carry `"sylvode-runner": 1`, so a second run replaces only those;
//!   everything else in the file stays.
//! - It never touches a tmux session.

use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode, Stdio};

use serde_json::{Value, json};
use sylvode_prereq::{Checker, InstallKind};

use super::checks::{Channel, valid_project_key};
use super::files::{RUNNER_MARK, StateDir, claude_hooks, mcp_server_entry};
use super::launch::{
    CodexLaunchFiles, LaunchFiles, SEARCH_MODE_TOOLS, SERVER_NAME, WRITE_TOOL, codex_session_argv, hook_words_for,
    mcp_tool, session_argv, shell_quote, toml_array, toml_string,
};
use super::tmux::{NameSource, Slot, TmuxName, WorkDir};
use crate::config::Config;

/// What the command line asked for.
#[derive(Debug, PartialEq, Eq)]
pub struct SetupArgs {
    /// `claude` or `codex`.
    pub cli: String,
    pub project: String,
    pub config_path: String,
    pub dir: Option<PathBuf>,
    pub bot: Option<String>,
    pub apply: bool,
}

/// Everything `setup` prints or applies, worked out before anything is done.
#[derive(Debug)]
pub struct Plan {
    /// The project key the plan was made for.
    pub key: String,
    /// The slot, `<KEY>-claude` or `bot-<uuid>-claude`: the state directory of the session.
    pub name: Slot,
    /// The tmux session the runner uses, and where its name comes from.
    pub tmux: TmuxName,
    pub source: NameSource,
    pub dir: PathBuf,
    pub launch: Vec<String>,
    pub mcp_entry: Value,
    pub mcp_toml: PathBuf,
    pub hooks: serde_json::Map<String, Value>,
    pub allow: Vec<String>,
}

/// What the plan needs from the machine, so that the plan itself is a pure function.
#[derive(Debug)]
pub struct Inputs<'a> {
    pub key: &'a str,
    pub dir: PathBuf,
    pub bot_id: &'a str,
    pub state_dir: &'a Path,
    pub runner_exe: &'a Path,
    pub mcp_server: &'a Path,
    pub launch_program: &'a Path,
    pub allow_write_call: bool,
}

/// Works out the launch line, the MCP entry, the hooks and the allowed tools.
///
/// # Errors
///
/// A message for a key, directory or path that cannot be used.
pub fn plan(config: &Config, inputs: &Inputs<'_>) -> Result<Plan, String> {
    if !valid_project_key(inputs.key) {
        return Err("the project key must match ^[A-Z0-9]{1,32}$".to_string());
    }
    let resolved = super::naming::resolve(&config.runner, inputs.bot_id, inputs.key, "claude", Channel::Tmux)
        .map_err(|r| r.reason)?;
    let name = resolved.slot;
    WorkDir::new(&inputs.dir).map_err(|e| e.to_string())?;
    let mcp_json = inputs
        .state_dir
        .join("mcp")
        .join(format!("{}.claude.json", inputs.bot_id));
    let mcp_toml = inputs.state_dir.join("mcp").join(format!("{}.toml", inputs.bot_id));
    let settings = inputs
        .state_dir
        .join("sessions")
        .join(name.as_str())
        .join("claude-settings.json");
    let hook_socket = inputs.state_dir.join("hook.sock");
    let files = LaunchFiles {
        claude_mcp_json: &mcp_json,
        claude_settings: &settings,
        runner_exe: inputs.runner_exe,
        hook_socket: &hook_socket,
    };
    let launch = session_argv(inputs.launch_program, &config.runner.cli.claude, &files).map_err(|e| e.to_string())?;
    let server = inputs
        .mcp_server
        .to_str()
        .ok_or_else(|| "the MCP server path is not UTF-8".to_string())?;
    let toml_text = mcp_toml
        .to_str()
        .ok_or_else(|| "the MCP config path is not UTF-8".to_string())?;
    let mut allow: Vec<String> = SEARCH_MODE_TOOLS.iter().map(|tool| mcp_tool(tool)).collect();
    if inputs.allow_write_call {
        allow.push(mcp_tool(WRITE_TOOL));
    }
    Ok(Plan {
        key: inputs.key.to_string(),
        name,
        tmux: resolved.tmux,
        source: resolved.source,
        dir: inputs.dir.clone(),
        launch,
        mcp_entry: mcp_server_entry(server, toml_text),
        mcp_toml,
        hooks: claude_hooks(&files, true)?,
        allow,
    })
}

/// The session name and how to create the session. A bot's session named in the configuration
/// serves the tasks of every project of the bot and is never created by the runner.
fn session_intro(slot: &Slot, tmux: &TmuxName, source: NameSource, dir: &str) -> String {
    let name = tmux.as_str();
    let create = format!("tmux new-session -d -s {name} -c {}", shell_quote(dir));
    match source {
        NameSource::Derived => format!(
            "Session name: {name}\n\nCreate the session yourself (or let the runner create it when it is absent):\n  {create}\n"
        ),
        NameSource::Local => format!(
            "Session name: {name}\n\nbot {}: tasks of every connected project go to tmux session {name}; \
             the runner does not create it (runner.tmux.sessions; the runner keeps its files under {})\n\n\
             Create the session yourself:\n  {create}\n",
            slot.bot_id().unwrap_or_default(),
            slot.as_str()
        ),
    }
}

/// The text `setup` prints without `--apply`.
pub fn render(plan: &Plan) -> String {
    let dir = plan.dir.to_string_lossy();
    let line = plan
        .launch
        .iter()
        .map(|word| shell_quote(word))
        .collect::<Vec<_>>()
        .join(" ");
    let mut out = String::new();
    let _ = out.write_str(&session_intro(&plan.name, &plan.tmux, plan.source, &dir));
    let _ = writeln!(
        out,
        "\nStart claude in it with this command (one line; the settings file is written when the runner \
         creates the session, or use --apply instead):\n  {line}"
    );
    let _ = writeln!(
        out,
        "\nOr give your own claude the Sylvode MCP server and hooks with `setup claude --project {} --apply`.",
        plan.key
    );
    let entry = serde_json::to_string_pretty(&plan.mcp_entry).unwrap_or_default();
    let _ = writeln!(
        out,
        "\nMCP server entry (names the file {}, which holds the bot's token; the token is not shown):\n{entry}",
        plan.mcp_toml.display()
    );
    let hooks = serde_json::to_string_pretty(&json!({"hooks": plan.hooks})).unwrap_or_default();
    let _ = writeln!(out, "\nHooks for <dir>/.claude/settings.local.json:\n{hooks}");
    out
}

/// Merges the runner's hooks and the allowed tools into the text of a settings file. Hook objects
/// the runner wrote before (they carry the mark) are replaced; every other key, hook and
/// permission is kept as it is.
///
/// # Errors
///
/// A message when the existing text is not a JSON object, or `hooks` or `permissions` have the
/// wrong shape (the file is then left alone).
pub fn merge_settings(
    existing: &str,
    hooks: &serde_json::Map<String, Value>,
    allow: &[String],
) -> Result<String, String> {
    let mut root: Value = if existing.trim().is_empty() {
        json!({})
    } else {
        serde_json::from_str(existing).map_err(|_| "settings.local.json is not valid JSON".to_string())?
    };
    let object = root
        .as_object_mut()
        .ok_or_else(|| "settings.local.json is not a JSON object".to_string())?;
    let all_hooks = object
        .entry("hooks")
        .or_insert_with(|| json!({}))
        .as_object_mut()
        .ok_or_else(|| "`hooks` in settings.local.json is not an object".to_string())?;
    for (event, entries) in hooks {
        let list = all_hooks
            .entry(event.clone())
            .or_insert_with(|| json!([]))
            .as_array_mut()
            .ok_or_else(|| format!("`hooks.{event}` in settings.local.json is not a list"))?;
        // Drop the runner's own hook objects, and the entries that held nothing else.
        list.retain_mut(|entry| {
            let Some(inner) = entry.get_mut("hooks").and_then(Value::as_array_mut) else {
                return true;
            };
            let before = inner.len();
            inner.retain(|hook| hook.get(RUNNER_MARK).is_none());
            !(inner.is_empty() && before > 0)
        });
        if let Some(new_entries) = entries.as_array() {
            list.extend(new_entries.iter().cloned());
        }
    }
    let allowed = object
        .entry("permissions")
        .or_insert_with(|| json!({}))
        .as_object_mut()
        .ok_or_else(|| "`permissions` in settings.local.json is not an object".to_string())?
        .entry("allow")
        .or_insert_with(|| json!([]))
        .as_array_mut()
        .ok_or_else(|| "`permissions.allow` in settings.local.json is not a list".to_string())?;
    for tool in allow {
        if !allowed.iter().any(|known| known.as_str() == Some(tool)) {
            allowed.push(json!(tool));
        }
    }
    let mut text = serde_json::to_string_pretty(&root).map_err(|e| e.to_string())?;
    text.push('\n');
    Ok(text)
}

/// How the `sylvode` MCP entry of the directory stands.
#[derive(Debug, PartialEq, Eq)]
pub enum McpState {
    /// No such entry: it is added.
    Absent,
    /// The entry already starts the MCP server on this bot's file.
    Ours,
    /// The entry exists and points elsewhere: left alone.
    Elsewhere,
}

/// Reads `claude mcp get sylvode` in `dir`.
fn mcp_state(claude: &Path, dir: &Path, toml: &Path) -> Result<McpState, String> {
    let output = Command::new(claude)
        .args(["mcp", "get", SERVER_NAME])
        .current_dir(dir)
        .stdin(Stdio::null())
        .output()
        .map_err(|e| format!("could not run claude: {}", e.kind()))?;
    if !output.status.success() {
        return Ok(McpState::Absent);
    }
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let ours = toml.to_str().is_some_and(|path| text.contains(path));
    Ok(if ours { McpState::Ours } else { McpState::Elsewhere })
}

/// What `--apply` did.
#[derive(Debug, PartialEq, Eq)]
pub struct Applied {
    pub text: String,
    /// 0, or 2 when a `sylvode` entry that points elsewhere was left alone.
    pub code: u8,
}

/// Applies the plan in its directory: the MCP entry through `claude`, then the settings file.
///
/// # Errors
///
/// A message for a failed `claude` call or an unusable settings file.
pub fn apply(plan: &Plan, claude: &Path) -> Result<Applied, String> {
    let mut text = String::new();
    match mcp_state(claude, &plan.dir, &plan.mcp_toml)? {
        McpState::Elsewhere => {
            return Ok(Applied {
                text: format!(
                    "A `{SERVER_NAME}` MCP server is already configured in this directory and points elsewhere; \
                     it was left alone and nothing was changed. Remove it (`claude mcp remove {SERVER_NAME}`) \
                     and run setup again if it should serve this bot.\n"
                ),
                code: 2,
            });
        }
        McpState::Ours => {
            let _ = writeln!(text, "MCP server `{SERVER_NAME}`: already set for this bot.");
        }
        McpState::Absent => {
            let output = Command::new(claude)
                .args(["mcp", "add-json", "--scope", "local", SERVER_NAME])
                .arg(plan.mcp_entry.to_string())
                .current_dir(&plan.dir)
                .stdin(Stdio::null())
                .output()
                .map_err(|e| format!("could not run claude: {}", e.kind()))?;
            if !output.status.success() {
                return Err("`claude mcp add-json` failed".to_string());
            }
            let _ = writeln!(text, "MCP server `{SERVER_NAME}`: added (scope local).");
        }
    }
    let settings = plan.dir.join(".claude").join("settings.local.json");
    let existing = match std::fs::read_to_string(&settings) {
        Ok(existing) => existing,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(e) => return Err(format!("could not read settings.local.json: {}", e.kind())),
    };
    let merged = merge_settings(&existing, &plan.hooks, &plan.allow)?;
    if merged == existing {
        let _ = writeln!(
            text,
            "Hooks and permissions in .claude/settings.local.json: already current."
        );
    } else {
        if let Some(parent) = settings.parent() {
            std::fs::create_dir_all(parent).map_err(|e| format!("could not create .claude: {}", e.kind()))?;
        }
        let temporary = settings.with_extension("json.sylvode-tmp");
        std::fs::write(&temporary, &merged)
            .map_err(|e| format!("could not write settings.local.json: {}", e.kind()))?;
        std::fs::rename(&temporary, &settings)
            .map_err(|e| format!("could not write settings.local.json: {}", e.kind()))?;
        let _ = writeln!(text, "Hooks and permissions written to .claude/settings.local.json.");
    }
    let _ = writeln!(
        text,
        "Restart your claude: MCP servers and hooks are read when it starts."
    );
    Ok(Applied { text, code: 0 })
}

/// What both CLIs' setup resolve first: the session, the bot, the directory and the programs both
/// need.
struct Resolved {
    state_dir: PathBuf,
    bot_id: String,
    dir: PathBuf,
    mcp_server: PathBuf,
    runner_exe: PathBuf,
}

fn resolve(args: &SetupArgs, config: &Config, name: &Slot) -> Result<Resolved, String> {
    let home = super::paths::service_user_home();
    let state_dir = super::state_dir(config, home.as_deref())
        .ok_or_else(|| "no state directory: set [runner] state_dir (no home directory is known)".to_string())?;
    // What the runner last saw for this project, if it ever served it: the bot and the directory.
    let record = state_dir
        .is_dir()
        .then(|| StateDir::open(&state_dir).ok())
        .flatten()
        .and_then(|state| state.read_session_record(name));
    let targets = crate::handshake::targets(config);
    let bot_id = match (&args.bot, &record) {
        (Some(bot), _) => bot.clone(),
        (None, Some(record)) if !record.bot_id.is_empty() => record.bot_id.clone(),
        _ => match targets.as_slice() {
            [only] => only.bot_id.clone(),
            [] => return Err("the configuration has no runner bot".to_string()),
            _ => return Err("this runner serves several bots; name the project's bot with --bot ID".to_string()),
        },
    };
    if !targets.iter().any(|t| t.bot_id.eq_ignore_ascii_case(&bot_id)) {
        return Err("that bot is not one of this runner's bots".to_string());
    }
    let dir = match (&args.dir, &record) {
        (Some(dir), _) => dir.clone(),
        (None, Some(record)) if !record.dir.is_empty() => PathBuf::from(&record.dir),
        _ => return Err("the directory is not known yet: give it with --dir D".to_string()),
    };
    let dir = std::fs::canonicalize(&dir).map_err(|_| "the directory does not exist".to_string())?;
    let mcp_server = Checker::new(InstallKind::RunnerTmux)
        .locate(&config.runner.mcp.server_binary)
        .ok_or_else(|| "the MCP server (mcp-server) was not found".to_string())?;
    let runner_exe = std::env::current_exe()
        .ok()
        .and_then(|raw| super::session::stable_exe(&raw))
        .ok_or_else(|| "the path of this executable cannot be used".to_string())?;
    Ok(Resolved {
        state_dir,
        bot_id,
        dir,
        mcp_server,
        runner_exe,
    })
}

/// Loads what the plan needs from the configuration and the machine.
fn gather(args: &SetupArgs, config: &Config) -> Result<(Plan, PathBuf), String> {
    let name = Slot::project(&args.project, "claude").map_err(|e| e.to_string())?;
    let resolved = resolve(args, config, &name)?;
    let checker = Checker::new(InstallKind::RunnerTmux);
    let claude = checker
        .locate(&config.runner.cli.claude.binary)
        .ok_or_else(|| "the claude CLI was not found".to_string())?;
    let launch_program = config
        .runner
        .cli
        .claude
        .launch_command
        .first()
        .and_then(|word| checker.locate(word))
        .ok_or_else(|| "the launch command of the CLI was not found".to_string())?;
    let plan = plan(
        config,
        &Inputs {
            key: &args.project,
            dir: resolved.dir,
            bot_id: &resolved.bot_id,
            state_dir: &resolved.state_dir,
            runner_exe: &resolved.runner_exe,
            mcp_server: &resolved.mcp_server,
            launch_program: &launch_program,
            allow_write_call: config.runner.mcp.allow_write_call,
        },
    )?;
    Ok((plan, claude))
}

/// Runs `setup`. Returns what to print and the exit code (0 done, 2 an MCP entry was left alone).
///
/// # Errors
///
/// A message for the operator.
pub fn execute(args: &SetupArgs) -> Result<Applied, String> {
    let config = Config::load(&args.config_path)?;
    if args.cli == "codex" {
        return execute_codex(args, &config);
    }
    let (plan, claude) = gather(args, &config)?;
    if args.apply {
        apply(&plan, &claude)
    } else {
        Ok(Applied {
            text: render(&plan),
            code: 0,
        })
    }
}

/// `setup` as a subcommand.
pub fn run_setup(args: &SetupArgs) -> ExitCode {
    use std::io::Write as _;
    match execute(args) {
        Ok(done) => {
            let _ = std::io::stdout().lock().write_all(done.text.as_bytes());
            ExitCode::from(done.code)
        }
        Err(e) => {
            let _ = writeln!(std::io::stderr().lock(), "error: {e}");
            ExitCode::FAILURE
        }
    }
}

// ----- codex ---------------------------------------------------------------------------------

/// Everything `setup codex` prints or applies, worked out before anything is done.
#[derive(Debug)]
pub struct CodexPlan {
    /// The project key the plan was made for.
    pub key: String,
    /// The slot, `<KEY>-codex` or `bot-<uuid>-codex`.
    pub name: Slot,
    /// The tmux session the runner uses, and where its name comes from.
    pub tmux: TmuxName,
    pub source: NameSource,
    pub dir: PathBuf,
    /// The full launch line, `codex` program first.
    pub launch: Vec<String>,
    pub mcp_server: PathBuf,
    pub mcp_toml: PathBuf,
    /// The words of the notify program.
    pub notify: Vec<String>,
}

/// What the codex plan needs from the machine.
#[derive(Debug)]
pub struct CodexInputs<'a> {
    pub key: &'a str,
    pub dir: PathBuf,
    pub bot_id: &'a str,
    pub state_dir: &'a Path,
    pub runner_exe: &'a Path,
    pub mcp_server: &'a Path,
    pub launch_program: &'a Path,
}

/// Works out codex's launch line and MCP entry.
///
/// # Errors
///
/// A message for a key, directory or path that cannot be used.
pub fn codex_plan(config: &Config, inputs: &CodexInputs<'_>) -> Result<CodexPlan, String> {
    if !valid_project_key(inputs.key) {
        return Err("the project key must match ^[A-Z0-9]{1,32}$".to_string());
    }
    let resolved = super::naming::resolve(&config.runner, inputs.bot_id, inputs.key, "codex", Channel::Tmux)
        .map_err(|r| r.reason)?;
    let name = resolved.slot;
    WorkDir::new(&inputs.dir).map_err(|e| e.to_string())?;
    let mcp_toml = inputs.state_dir.join("mcp").join(format!("{}.toml", inputs.bot_id));
    let hook_socket = inputs.state_dir.join("hook.sock");
    let files = CodexLaunchFiles {
        mcp_server: inputs.mcp_server,
        mcp_toml: &mcp_toml,
        runner_exe: inputs.runner_exe,
        hook_socket: &hook_socket,
    };
    let launch =
        codex_session_argv(inputs.launch_program, &config.runner.cli.codex, &files).map_err(|e| e.to_string())?;
    let notify = hook_words_for(inputs.runner_exe, &hook_socket, "codex", "turn-ended").map_err(|e| e.to_string())?;
    Ok(CodexPlan {
        key: inputs.key.to_string(),
        name,
        tmux: resolved.tmux,
        source: resolved.source,
        dir: inputs.dir.clone(),
        launch,
        mcp_server: inputs.mcp_server.to_path_buf(),
        mcp_toml,
        notify,
    })
}

/// The server's command and arguments as TOML, shared by the printed entry and `codex mcp add`.
fn codex_server_words(plan: &CodexPlan) -> Result<Vec<String>, String> {
    let server = plan
        .mcp_server
        .to_str()
        .ok_or_else(|| "the MCP server path is not UTF-8".to_string())?;
    let toml = plan
        .mcp_toml
        .to_str()
        .ok_or_else(|| "the MCP config path is not UTF-8".to_string())?;
    Ok(vec![
        server.to_string(),
        "serve".to_string(),
        "--transport".to_string(),
        "stdio".to_string(),
        "--config".to_string(),
        toml.to_string(),
    ])
}

/// The text `setup codex` prints without `--apply`.
///
/// # Errors
///
/// A message for a path that is not UTF-8.
pub fn render_codex(plan: &CodexPlan) -> Result<String, String> {
    let dir = plan.dir.to_string_lossy();
    let line = plan
        .launch
        .iter()
        .map(|word| shell_quote(word))
        .collect::<Vec<_>>()
        .join(" ");
    let words = codex_server_words(plan)?;
    let (command, args) = words
        .split_first()
        .ok_or_else(|| "the MCP server words are empty".to_string())?;
    let mut out = String::new();
    let _ = out.write_str(&session_intro(&plan.name, &plan.tmux, plan.source, &dir));
    let _ = writeln!(
        out,
        "\nStart codex in it with this command (one line; nothing is written to your codex configuration):\n  {line}"
    );
    let _ = writeln!(
        out,
        "\nTrust the folder once: attach and choose 1 when codex asks (the runner never answers that dialog)."
    );
    let _ = writeln!(
        out,
        "\nOr add the Sylvode MCP server to your own codex configuration with `setup codex --project {} --apply` \
         (the turn hook is not added: start codex with the line above to get it).",
        plan.key
    );
    let _ = writeln!(
        out,
        "\nMCP server entry for ~/.codex/config.toml (names the file {}, which holds the bot's token; the token is \
         not shown):\n[mcp_servers.{SERVER_NAME}]\ncommand = {}\nargs = {}",
        plan.mcp_toml.display(),
        toml_string(command),
        toml_array(args)
    );
    let _ = writeln!(
        out,
        "\nTurn hook (replaces any notify you have; optional):\nnotify = {}",
        toml_array(&plan.notify)
    );
    Ok(out)
}

/// `codex mcp get sylvode --json` in `dir`.
fn codex_mcp_state(codex: &Path, dir: &Path, toml: &Path) -> Result<McpState, String> {
    let output = Command::new(codex)
        .args(["mcp", "get", SERVER_NAME, "--json"])
        .current_dir(dir)
        .env_clear()
        .envs(super::tmux::agent_environment())
        .stdin(Stdio::null())
        .output()
        .map_err(|e| format!("could not run codex: {}", e.kind()))?;
    if !output.status.success() {
        return Ok(McpState::Absent);
    }
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let ours = toml.to_str().is_some_and(|path| text.contains(path));
    Ok(if ours { McpState::Ours } else { McpState::Elsewhere })
}

/// Applies the codex plan: `codex mcp add` when the entry is absent. The turn hook is never
/// written (codex has one `notify` program, and the operator's own would be lost).
///
/// # Errors
///
/// A message for a failed `codex` call.
pub fn apply_codex(plan: &CodexPlan, codex: &Path) -> Result<Applied, String> {
    let mut text = String::new();
    match codex_mcp_state(codex, &plan.dir, &plan.mcp_toml)? {
        McpState::Elsewhere => {
            return Ok(Applied {
                text: format!(
                    "A `{SERVER_NAME}` MCP server is already configured for codex and points elsewhere; it was left \
                     alone and nothing was changed. Remove it (`codex mcp remove {SERVER_NAME}`) and run setup again \
                     if it should serve this bot.\n"
                ),
                code: 2,
            });
        }
        McpState::Ours => {
            let _ = writeln!(text, "MCP server `{SERVER_NAME}`: already set for this bot.");
        }
        McpState::Absent => {
            let output = Command::new(codex)
                .args(["mcp", "add", SERVER_NAME, "--"])
                .args(codex_server_words(plan)?)
                .current_dir(&plan.dir)
                .env_clear()
                .envs(super::tmux::agent_environment())
                .stdin(Stdio::null())
                .output()
                .map_err(|e| format!("could not run codex: {}", e.kind()))?;
            if !output.status.success() {
                return Err("`codex mcp add` failed".to_string());
            }
            let _ = writeln!(
                text,
                "MCP server `{SERVER_NAME}`: added to your codex configuration (user scope)."
            );
        }
    }
    let _ = writeln!(
        text,
        "Turn hook: not written; start codex with the printed line to get it, otherwise turns are read from the screen."
    );
    let _ = writeln!(text, "Restart your codex: MCP servers are read when it starts.");
    Ok(Applied { text, code: 0 })
}

fn execute_codex(args: &SetupArgs, config: &Config) -> Result<Applied, String> {
    let name = Slot::project(&args.project, "codex").map_err(|e| e.to_string())?;
    let resolved = resolve(args, config, &name)?;
    if !config.runner.cli.codex.enabled {
        return Err("codex is not enabled on this runner ([runner.cli.codex] enabled)".to_string());
    }
    let checker = Checker::new(InstallKind::RunnerTmux);
    let codex = checker
        .locate(&config.runner.cli.codex.binary)
        .ok_or_else(|| "the codex CLI was not found".to_string())?;
    let launch_program = config
        .runner
        .cli
        .codex
        .launch_command
        .first()
        .and_then(|word| checker.locate(word))
        .ok_or_else(|| "the launch command of the CLI was not found".to_string())?;
    let plan = codex_plan(
        config,
        &CodexInputs {
            key: &args.project,
            dir: resolved.dir,
            bot_id: &resolved.bot_id,
            state_dir: &resolved.state_dir,
            runner_exe: &resolved.runner_exe,
            mcp_server: &resolved.mcp_server,
            launch_program: &launch_program,
        },
    )?;
    if args.apply {
        apply_codex(&plan, &codex)
    } else {
        Ok(Applied {
            text: render_codex(&plan)?,
            code: 0,
        })
    }
}
