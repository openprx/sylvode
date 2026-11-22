//! `mcp-server doctor` and `mcp-server --check`: the MCP side of the prerequisite checker
//! (`crates/sylvode-prereq`, design sections 4.11 and 7.11).
//!
//! The report is the shared `sylvode.doctor.v1` document, judged for the "MCP server alone"
//! install: tmux, the agent CLIs and the runner are `info`, a CLI without a registration is
//! `warn` (with the exact command to register it), an API on another task protocol is `fail`.
//!
//! What the output never contains: a token, a URL, the contents of the configuration file, or
//! anything a probed program printed beyond the version and protocol numbers the crate parses.
//! Configuration problems are rows (`detail.config`), not panics and not messages: the
//! messages of the loader can quote a configured value, so only the state and a pointer to
//! `serve` (which prints the loader's own message to the operator's terminal) are reported.
//!
//! [`check_report`] is what `mcp-server --check` prints and what `sylvode-runner doctor` parses
//! to learn this binary's version and task protocol. It has the one `mcp_server` row and probes
//! nothing else: the runner's own `--check` probes `mcp-server --check`, so a `--check` that
//! probed its peer would recurse.

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::time::Duration;

use platform::config::{DEFAULT_CONFIG_PATH, McpRuntime, McpToolMode, OpenPrConfig};
use serde_json::json;
use sylvode_prereq::probe::api::{ApiProbe, check_api_protocol};
use sylvode_prereq::{
    Checker, Cli, Component, ConfigState, Context, GeneratedBy, Host, InstallKind, McpScope, McpState, Mode, Peer,
    Report, RunOutcome, Status, mcp_registration_component, path_source,
};

use crate::cli::{GlobalArgs, apply_cli_overrides};
use crate::client::transport_label;
use crate::tools::entry::TASK_PROTOCOL_VERSION;

/// The name of this binary in `generated_by.binary`.
pub const BINARY: &str = "mcp-server";

/// The name the MCP server is registered under in the agent CLIs.
pub const REGISTERED_NAME: &str = "sylvode";

/// The tmux socket the runner uses by default; `doctor` only asks whether a server runs on it.
const TMUX_SOCKET: &str = "sylvode";

/// How long the API protocol probe may take.
const API_TIMEOUT: Duration = Duration::from_secs(5);

/// How long `<cli> mcp get sylvode` may take: it starts the registered server to check it.
const REGISTRATION_TIMEOUT: Duration = Duration::from_secs(15);

/// What `doctor` was asked for.
#[derive(Debug, Default)]
pub struct DoctorOptions {
    /// The CLIs whose presence and MCP registration are checked.
    pub clis: Vec<Cli>,
    /// Evaluate programs on the `PATH` of a service unit instead of the shell's.
    pub as_service: bool,
}

/// The task protocol this binary speaks.
fn task_protocol() -> u32 {
    u32::try_from(TASK_PROTOCOL_VERSION).unwrap_or(u32::MAX)
}

/// The configuration as `serve` reads it, reduced to what the report needs.
struct Loaded {
    state: ConfigState,
    runtime: Option<McpRuntime>,
    tool_mode: McpToolMode,
}

/// Reads the configuration the way `serve` does: the same file discovery, the same validation of
/// `[mcp]`, the same command-line overrides. A problem is a state, never an error.
fn load(global: &GlobalArgs) -> Loaded {
    let mut loaded = Loaded {
        state: ConfigState::Invalid,
        runtime: None,
        tool_mode: McpToolMode::default(),
    };
    let mut config = match OpenPrConfig::load(global.config.as_deref()) {
        Ok(config) => config,
        Err(platform::config::ConfigError::NotFound { .. }) => {
            loaded.state = ConfigState::Missing;
            return loaded;
        }
        Err(_) => return loaded,
    };
    loaded.tool_mode = config.mcp.tool_mode;
    if apply_cli_overrides(&mut config.mcp, global, None).is_err() {
        return loaded;
    }
    if let Ok(runtime) = config.mcp_runtime() {
        loaded.tool_mode = runtime.tool_mode;
        loaded.runtime = Some(runtime);
        loaded.state = ConfigState::Found;
    }
    loaded
}

/// The identity row: this binary, its version and task protocol, its tool mode.
fn identity_row(tool_mode: McpToolMode) -> Component {
    let mut row = Component::new("mcp_server", true, Status::Ok, true)
        .with_detail("task_protocol", json!(task_protocol()))
        .with_detail("tool_mode", json!(tool_mode.as_str()));
    row.version = Some(sylvode_prereq::VERSION.to_string());
    row.path = std::env::current_exe()
        .ok()
        .map(|path| path.to_string_lossy().into_owned());
    row
}

const fn context(mode_as_service: bool, config: ConfigState) -> Context {
    Context {
        mode: Mode::Auto,
        as_service: mode_as_service,
        path_source: path_source(mode_as_service),
        config,
    }
}

/// The report `mcp-server --check` prints: one `mcp_server` row, no probes, no secrets.
///
/// Only `[mcp] tool_mode` is taken from the configuration; a missing or unusable file leaves the
/// default. The report is complete either way: it is about this binary, not about the setup.
#[must_use]
pub fn check_report(global: &GlobalArgs) -> Report {
    let loaded = load(global);
    let host = Host::detect(&Checker::new(InstallKind::McpServerAlone));
    Report::new(
        GeneratedBy::new(BINARY, task_protocol()),
        host,
        context(false, loaded.state),
        vec![identity_row(loaded.tool_mode)],
    )
}

/// The `PATH` a service unit would hold: the fixed locations the installers add, without the
/// shell's own `PATH`.
fn service_path() -> OsString {
    service_path_for(std::env::var_os("HOME").map(PathBuf::from).as_deref())
}

fn service_path_for(home: Option<&Path>) -> OsString {
    let mut dirs: Vec<PathBuf> = Vec::new();
    if let Some(home) = home {
        dirs.push(home.join(".cargo/bin"));
        dirs.push(home.join(".local/bin"));
    }
    for fixed in [
        "/usr/local/bin",
        "/opt/homebrew/bin",
        "/usr/bin",
        "/bin",
        "/usr/sbin",
        "/sbin",
    ] {
        dirs.push(PathBuf::from(fixed));
    }
    std::env::join_paths(dirs).unwrap_or_default()
}

fn absolute(path: &Path) -> PathBuf {
    if path.is_absolute() {
        return path.to_path_buf();
    }
    std::env::current_dir().map_or_else(|_| path.to_path_buf(), |cwd| cwd.join(path))
}

/// Whether the file can be read by its group or by others. `None` when it cannot be examined.
fn readable_beyond_owner(path: &Path) -> Option<bool> {
    let metadata = std::fs::metadata(path).ok()?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        Some(metadata.permissions().mode() & 0o077 != 0)
    }
    #[cfg(not(unix))]
    {
        let _ = metadata;
        Some(false)
    }
}

/// Quotes `value` for a POSIX shell when it needs it.
fn shell_word(value: &str) -> String {
    let plain = !value.is_empty()
        && value.chars().all(|c| {
            c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | '/' | ':' | '@' | '%' | '+' | '=' | ',' | '-')
        });
    if plain {
        value.to_string()
    } else {
        format!("'{}'", value.replace('\'', "'\\''"))
    }
}

/// The command that registers this server in `cli`, as the installer prints it. The token never
/// appears: the configuration file is passed by path.
#[must_use]
pub fn registration_command(cli: Cli, binary: &str, config: &str) -> String {
    let server = format!(
        "{REGISTERED_NAME} -- {} serve --transport stdio --config {}",
        shell_word(binary),
        shell_word(config)
    );
    match cli {
        Cli::Claude => format!("claude mcp add --scope user {server}"),
        Cli::Codex => format!("codex mcp add {server}"),
        Cli::Opencode => format!(
            "add an `mcp` entry named {REGISTERED_NAME} to opencode.json: local command [{}, serve, --transport, stdio, --config, {}]",
            shell_word(binary),
            shell_word(config)
        ),
    }
}

/// What `claude mcp get sylvode` said, reduced to a state and a scope.
fn parse_claude_get(stdout: &str) -> (McpState, McpScope, Option<&'static str>) {
    let mut status = None;
    let mut scope = McpScope::User;
    for line in stdout.lines() {
        let lower = line.trim().to_ascii_lowercase();
        if let Some(value) = lower.strip_prefix("status:") {
            status = Some(value.trim().to_string());
        } else if let Some(value) = lower.strip_prefix("scope:") {
            // A "local" registration belongs to one project directory of this person.
            scope = if value.contains("project") || value.contains("local") {
                McpScope::Project
            } else {
                McpScope::User
            };
        }
    }
    let Some(status) = status else {
        return (McpState::Unverified, scope, None);
    };
    if status.contains("pending") {
        return (
            McpState::NotRegistered,
            scope,
            Some("approve the sylvode server in claude, then run `claude mcp get sylvode`"),
        );
    }
    let failing = ["fail", "disconnected", "not connected", "error", "needs auth"];
    if failing.iter().any(|word| status.contains(word)) {
        return (
            McpState::RegisteredFailing,
            scope,
            Some(
                "claude cannot start the registered server: run `claude mcp get sylvode`, then check the binary path and the configuration file it names",
            ),
        );
    }
    if status.contains("connected") {
        return (McpState::Connected, scope, None);
    }
    (McpState::Unverified, scope, None)
}

/// The `mcp_registration.<cli>` row.
fn registration_row(checker: &Checker, cli: Cli, command: &str) -> Component {
    let install = checker.install();
    let name = cli.name();
    let row = |state, scope, hint: Option<&str>| mcp_registration_component(install, name, state, scope, hint);
    if cli == Cli::Opencode {
        return row(McpState::Unverified, McpScope::User, Some(command));
    }
    let Some(path) = checker.locate(name) else {
        let hint = format!("{name} not found on PATH; install it, then run: {command}");
        return row(McpState::BinaryMissing, McpScope::User, Some(&hint));
    };
    let patient = checker.clone().with_timeout(REGISTRATION_TIMEOUT);
    match patient.run(&path, &["mcp", "get", REGISTERED_NAME]) {
        RunOutcome::SpawnFailed => {
            let hint = format!("{name} could not be started; install it again, then run: {command}");
            row(McpState::BinaryMissing, McpScope::User, Some(&hint))
        }
        RunOutcome::TimedOut => {
            let hint = format!("`{name} mcp get {REGISTERED_NAME}` did not answer in time; run it by hand");
            row(McpState::RegisteredFailing, McpScope::User, Some(&hint))
        }
        // Only the exit status of codex is understood; claude's text is parsed for its status.
        RunOutcome::Exited { code: Some(0), stdout } if cli == Cli::Codex => {
            let _ = stdout;
            row(McpState::Connected, McpScope::User, None)
        }
        RunOutcome::Exited { code: Some(0), stdout } => {
            let (state, scope, hint) = parse_claude_get(&stdout);
            match state {
                McpState::Unverified => row(
                    McpState::RegisteredFailing,
                    scope,
                    Some("could not read the status from `claude mcp get sylvode`; run it by hand"),
                ),
                _ => row(state, scope, hint),
            }
        }
        RunOutcome::Exited { .. } => {
            let hint = format!("run: {command}");
            row(McpState::NotRegistered, McpScope::User, Some(&hint))
        }
    }
}

/// The `api_protocol` row, or an information row when no identity is configured to ask with.
async fn api_row(loaded: &Loaded) -> Component {
    let probe = loaded
        .runtime
        .as_ref()
        .and_then(|runtime| runtime.bot_token.as_ref().map(|token| (runtime, token)));
    let Some((runtime, token)) = probe else {
        return Component::new("api_protocol", false, Status::Info, false)
            .with_detail("state", json!("not_configured"))
            .with_hint("no bot token is configured, so the API protocol was not checked");
    };
    check_api_protocol(
        InstallKind::McpServerAlone,
        &ApiProbe {
            base_url: &runtime.api_url,
            token: token.expose(),
            surface: Some(transport_label(runtime.transport)),
            timeout: API_TIMEOUT,
            own_protocol: task_protocol(),
        },
    )
    .await
}

/// Decorates the identity row with the state of the configuration file.
fn config_row(mut row: Component, loaded: &Loaded, config_path: &Path) -> Component {
    let token_held = loaded
        .runtime
        .as_ref()
        .is_some_and(|runtime| runtime.bot_token.is_some() || runtime.oauth.is_some());
    let open = if loaded.state == ConfigState::Found && token_held {
        readable_beyond_owner(config_path)
    } else {
        None
    };
    let (config, status, hint) = match loaded.state {
        ConfigState::Found => ("parsed", Status::Ok, None),
        ConfigState::Missing => (
            "missing",
            Status::Warn,
            Some("no configuration file: copy config/sylvode.example.toml, fill in [mcp] and pass it with --config"),
        ),
        ConfigState::Invalid => (
            "invalid",
            Status::Fail,
            Some("the configuration is unusable: run `mcp-server serve` to see the loader's message"),
        ),
    };
    row = row.with_detail("config", json!(config));
    row.status = status;
    row.hint = hint.map(str::to_string);
    if open == Some(true) {
        row.status = Status::Fail;
        row.hint = Some("the configuration file holds a token and others can read it: chmod 600 the file".to_string());
    }
    let mode = match open {
        Some(true) => "too_open",
        Some(false) => "private",
        None => "unknown",
    };
    row.with_detail("config_mode", json!(mode))
}

/// Runs every check of `mcp-server doctor` and returns the report.
pub async fn doctor_report(global: &GlobalArgs, options: &DoctorOptions) -> Report {
    let loaded = load(global);
    let mut checker = Checker::new(InstallKind::McpServerAlone);
    if options.as_service {
        checker = checker.with_path(service_path());
    }
    let config_path = absolute(
        global
            .config
            .as_deref()
            .unwrap_or_else(|| Path::new(DEFAULT_CONFIG_PATH)),
    );
    let exe = std::env::current_exe().map_or_else(|_| BINARY.to_string(), |path| path.to_string_lossy().into_owned());
    let config_text = config_path.to_string_lossy().into_owned();

    let mut components = vec![config_row(identity_row(loaded.tool_mode), &loaded, &config_path)];
    components.push(checker.check_tmux(TMUX_SOCKET));
    for cli in [Cli::Claude, Cli::Codex, Cli::Opencode] {
        components.push(checker.check_cli(cli, options.clis.contains(&cli), None));
    }
    for cli in &options.clis {
        components.push(registration_row(
            &checker,
            *cli,
            &registration_command(*cli, &exe, &config_text),
        ));
    }
    components.push(checker.check_peer(Peer::Runner, None, Some(task_protocol())));
    components.push(api_row(&loaded).await);

    let host = Host::detect(&checker);
    Report::new(
        GeneratedBy::new(BINARY, task_protocol()),
        host,
        context(options.as_service, loaded.state),
        components,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn claude_statuses_map_to_states() {
        for (text, state) in [
            (
                "sylvode:\n  Scope: User config\n  Status: ✓ Connected\n",
                McpState::Connected,
            ),
            ("Status: ✗ Failed to connect", McpState::RegisteredFailing),
            ("Status: Pending approval", McpState::NotRegistered),
            ("Status: Needs authentication", McpState::RegisteredFailing),
            ("Status: something new", McpState::Unverified),
            ("no status line", McpState::Unverified),
        ] {
            assert_eq!(parse_claude_get(text).0, state, "{text:?}");
        }
        assert_eq!(
            parse_claude_get("Scope: Project config\nStatus: Connected").1,
            McpScope::Project
        );
        assert_eq!(
            parse_claude_get("Scope: User config\nStatus: Connected").1,
            McpScope::User
        );
    }

    #[test]
    fn the_service_path_is_fixed_and_ignores_the_shell() {
        let home = Path::new("home-dir");
        let path = service_path_for(Some(home));
        let dirs: Vec<PathBuf> = std::env::split_paths(&path).collect();
        assert_eq!(dirs.first(), Some(&home.join(".cargo/bin")));
        assert!(dirs.contains(&PathBuf::from("/usr/bin")));
        assert_eq!(std::env::split_paths(&service_path_for(None)).count(), 6);
        let report_context = context(true, ConfigState::Found);
        assert_eq!(report_context.path_source, path_source(true));
        assert!(report_context.as_service);
    }

    #[test]
    fn registration_commands_quote_paths_and_name_no_token() {
        let claude = registration_command(Cli::Claude, "/a/mcp-server", "/b/c.toml");
        assert_eq!(
            claude,
            "claude mcp add --scope user sylvode -- /a/mcp-server serve --transport stdio --config /b/c.toml"
        );
        let codex = registration_command(Cli::Codex, "/a b/mcp-server", "/it's/c.toml");
        assert_eq!(
            codex,
            "codex mcp add sylvode -- '/a b/mcp-server' serve --transport stdio --config '/it'\\''s/c.toml'"
        );
    }
}
