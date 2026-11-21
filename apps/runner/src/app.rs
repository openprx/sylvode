use crate::{AppState, background::Background, config, handler, process, tunnel};
use axum::{
    Json, Router,
    extract::State,
    routing::{get, post},
};
use std::ffi::OsString;
use std::future::IntoFuture;
use std::io::Write;
use std::process::ExitCode;
use std::sync::Arc;
use std::time::Duration;
use tower_http::trace::TraceLayer;

const DEFAULT_CONFIG_PATH: &str = "config.toml";
const DEFAULT_LOG_FILTER: &str = "sylvode_runner=info";
/// The executable name, as reported by `--version` and in the log.
const NAME: &str = "sylvode-runner";
/// Environment variable that forces the tunnel, CLI and callback paths off.
const SAFE_MODE_ENV: &str = "SYLVODE_RUNNER_SAFE_MODE";

/// Help text. It names the canonical executable whichever name the process was started under,
/// so stdout is byte-for-byte identical for both names.
const HELP: &str = "\
Sylvode runner: receives Sylvode task deliveries and runs agents.

Usage: sylvode-runner [CONFIG]
       sylvode-runner sessions [--prune] [--config PATH]
       sylvode-runner trust KEY [--config PATH]
       sylvode-runner release KEY [--config PATH]
       sylvode-runner --help | --version

Arguments:
  [CONFIG]       Path to the TOML configuration file [default: config.toml]

Options:
  -h, --help     Print this help and exit
  -V, --version  Print the version and exit

Commands:
  sessions       List the tmux sessions this runner created; --prune ends the idle ones whose
                 project no longer uses tmux
  trust KEY      Attach to session KEY to answer its trust or login dialog
  release KEY    Free the session name KEY once its tmux session is gone
";

#[derive(Debug, PartialEq, Eq)]
enum Command {
    Help,
    Version,
    Serve {
        config_path: String,
    },
    /// `sessions [--prune] [--config P]`.
    Sessions {
        config_path: String,
        prune: bool,
    },
    /// `trust KEY [--config P]`.
    Trust {
        config_path: String,
        key: String,
    },
    /// `release KEY [--config P]`.
    Release {
        config_path: String,
        key: String,
    },
    /// `hook --cli C --session K --event E --socket P`: report a CLI turn event to the runner.
    Hook {
        cli: String,
        session: String,
        event: String,
        socket: String,
        nonce: String,
    },
}

/// The value after `flag` in `args`; every other argument (codex appends a JSON one) is ignored.
fn flag_value(args: &[OsString], flag: &str) -> String {
    args.iter()
        .position(|arg| arg == flag)
        .and_then(|at| args.get(at + 1))
        .map(|value| value.to_string_lossy().into_owned())
        .unwrap_or_default()
}

/// `sessions [--prune]`, `trust KEY`, `release KEY`, each with an optional `--config PATH`.
fn parse_operator(word: &str, rest: &[OsString]) -> Result<Command, String> {
    let mut config_path = DEFAULT_CONFIG_PATH.to_string();
    let mut prune = false;
    let mut key: Option<String> = None;
    let mut items = rest.iter();
    while let Some(item) = items.next() {
        let text = item
            .to_str()
            .ok_or_else(|| "arguments must be valid UTF-8".to_string())?;
        match text {
            "--config" => {
                config_path = items
                    .next()
                    .and_then(|value| value.to_str())
                    .ok_or_else(|| "--config needs a path".to_string())?
                    .to_string();
            }
            "--prune" if word == "sessions" => prune = true,
            other if other.starts_with('-') => return Err(format!("unknown option `{other}` for `{word}`")),
            other if word != "sessions" && key.is_none() => key = Some(other.to_string()),
            other => return Err(format!("unexpected argument `{other}` for `{word}`")),
        }
    }
    match word {
        "sessions" => Ok(Command::Sessions { config_path, prune }),
        "trust" => Ok(Command::Trust {
            config_path,
            key: key.ok_or_else(|| "`trust` needs a session name".to_string())?,
        }),
        _ => Ok(Command::Release {
            config_path,
            key: key.ok_or_else(|| "`release` needs a session name".to_string())?,
        }),
    }
}

fn parse_command(args: &[OsString]) -> Result<Command, String> {
    if args.first().is_some_and(|first| first == "hook") {
        let rest = args.get(1..).unwrap_or_default();
        return Ok(Command::Hook {
            cli: flag_value(rest, "--cli"),
            session: flag_value(rest, "--session"),
            event: flag_value(rest, "--event"),
            socket: flag_value(rest, "--socket"),
            nonce: flag_value(rest, "--nonce"),
        });
    }
    if let Some(word) = args
        .first()
        .and_then(|first| first.to_str())
        .filter(|w| matches!(*w, "sessions" | "trust" | "release"))
    {
        return parse_operator(word, args.get(1..).unwrap_or_default());
    }
    let Some(arg) = args.first().cloned() else {
        return Ok(Command::Serve {
            config_path: DEFAULT_CONFIG_PATH.to_string(),
        });
    };
    let arg = arg
        .into_string()
        .map_err(|raw| format!("config path is not valid UTF-8: {}", raw.to_string_lossy()))?;
    Ok(match arg.as_str() {
        "-h" | "--help" => Command::Help,
        "-V" | "--version" => Command::Version,
        option if option.starts_with('-') => {
            return Err(format!(
                "unknown option `{option}`; see --help (write a config path that starts with `-` as ./{option})"
            ));
        }
        _ => Command::Serve { config_path: arg },
    })
}

fn version_line() -> String {
    format!("{} {}\n", NAME, env!("CARGO_PKG_VERSION"))
}

fn write_stdout(text: &str) -> ExitCode {
    let mut stdout = std::io::stdout().lock();
    if stdout.write_all(text.as_bytes()).and_then(|()| stdout.flush()).is_ok() {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    }
}

fn write_stderr_line(text: &str) {
    let mut stderr = std::io::stderr().lock();
    // Nothing else is left to report a failed write on.
    let _ = writeln!(stderr, "{text}");
}

/// Runs the service.
pub fn run() -> ExitCode {
    let args: Vec<OsString> = std::env::args_os().skip(1).collect();
    let command = match parse_command(&args) {
        Ok(command) => command,
        Err(e) => {
            write_stderr_line(&format!("error: {e}"));
            return ExitCode::FAILURE;
        }
    };

    match command {
        Command::Help => write_stdout(HELP),
        Command::Version => write_stdout(&version_line()),
        Command::Serve { config_path } => serve_blocking(&config_path),
        Command::Sessions { config_path, prune } => crate::runner::cli::run_sessions(&config_path, prune),
        Command::Trust { config_path, key } => crate::runner::cli::run_trust(&config_path, &key),
        Command::Release { config_path, key } => crate::runner::cli::run_release(&config_path, &key),
        Command::Hook {
            cli,
            session,
            event,
            socket,
            nonce,
        } => {
            // Never reads stdin, never fails: a hook must not block or break the CLI that runs it.
            if !socket.is_empty() {
                crate::runner::hooks::send(std::path::Path::new(&socket), &cli, &session, &event, &nonce);
            }
            ExitCode::SUCCESS
        }
    }
}

/// Sets up logging from `RUST_LOG`; an unset or invalid filter falls back to the default.
fn init_tracing() {
    let filter = std::env::var("RUST_LOG")
        .ok()
        .and_then(|directives| tracing_subscriber::EnvFilter::try_new(directives).ok())
        .unwrap_or_else(|| DEFAULT_LOG_FILTER.into());
    if let Err(e) = tracing_subscriber::fmt().with_env_filter(filter).try_init() {
        write_stderr_line(&format!("error: failed to initialise logging: {e}"));
    }
}

/// Whether the value of [`SAFE_MODE_ENV`] turns safe mode on.
fn safe_mode_flag(raw: Option<&str>) -> bool {
    raw.is_some_and(|raw| matches!(raw.trim().to_ascii_lowercase().as_str(), "1" | "true" | "yes" | "on"))
}

fn serve_blocking(config_path: &str) -> ExitCode {
    init_tracing();
    let safe_mode = safe_mode_flag(std::env::var(SAFE_MODE_ENV).ok().as_deref());

    let mut config = match config::Config::load(config_path) {
        Ok(c) => c,
        Err(e) => {
            tracing::error!("failed to load config from {config_path}: {e}");
            return ExitCode::FAILURE;
        }
    };
    config.safe_mode = safe_mode;
    for warning in config.startup_warnings() {
        tracing::warn!("{warning}");
    }

    let runtime = match tokio::runtime::Builder::new_multi_thread().enable_all().build() {
        Ok(runtime) => runtime,
        Err(e) => {
            tracing::error!("failed to start the async runtime: {e}");
            return ExitCode::FAILURE;
        }
    };
    runtime.block_on(serve(config))
}

/// How long shutdown may take in all, from the signal to the exit: the requests in flight and the
/// background runs share it. It allows the process-group grace period, then a terminal callback
/// of up to two requests (MCP sends a comment and a state update), plus slack.
fn shutdown_bound(http_timeout_secs: u64) -> Duration {
    process::TERMINATION_GRACE + Duration::from_secs(http_timeout_secs.saturating_mul(2)) + Duration::from_secs(5)
}

async fn serve(config: config::Config) -> ExitCode {
    let listen = config.server.listen.clone();
    let bound = shutdown_bound(config.runtime.http_timeout_secs);
    tracing::info!("Loaded {} agent(s)", config.agents.len());

    let background = Background::new(config.runtime.cli_max_concurrency);
    let replay = tokio::sync::Mutex::new(crate::replay::ReplayGuard::new(config.security.max_delivery_age_secs));
    let has_runner_agents = config.agents.iter().any(|agent| agent.agent_type == "runner");
    let runner = crate::runner::RunnerRuntime::new(&config);
    if has_runner_agents && let Err(e) = &runner.http {
        tracing::error!("{e}");
        return ExitCode::FAILURE;
    }
    let handshake = crate::handshake::Handshake::new(has_runner_agents);
    // One runner per state directory: a second instance refuses to start.
    let _state_lock = match (has_runner_agents, &runner.state) {
        (true, Ok(dir)) => match dir.lock() {
            Ok(lock) => Some(lock),
            Err(e) => {
                tracing::error!("{e}");
                return ExitCode::FAILURE;
            }
        },
        _ => None,
    };
    let sessions = start_sessions(&config, &runner, &background);
    let state = Arc::new(AppState {
        config,
        background: background.clone(),
        replay,
        runner,
        handshake,
        sessions,
    });

    if has_runner_agents {
        // Rebuild the sessions' state from Sylvode (restart recovery, design 7.8).
        tokio::spawn(crate::runner::recovery::recover_all(Arc::clone(&state)));
        for line in crate::runner::posture(&state.config) {
            if line.warn {
                tracing::warn!("runner CLI posture: {}", line.text);
            } else {
                tracing::info!("runner CLI posture: {}", line.text);
            }
        }
        let loop_config = Arc::new(state.config.clone());
        let Ok(http) = state.runner.http.clone() else {
            return ExitCode::FAILURE;
        };
        let loop_handshake = state.handshake.clone();
        let loop_background = background.clone();
        background.spawn(async move {
            crate::handshake::run_loop(loop_config, http, loop_handshake, loop_background).await;
        });
    }

    let tunnel_state = Arc::new(state.config.clone());
    if tunnel_state.tunnel_enabled() {
        let tunnel_background = background.clone();
        background.spawn(async move {
            tunnel::run_tunnel_loop(tunnel_state, tunnel_background).await;
        });
    } else {
        tracing::info!("tunnel subsystem disabled (feature flag or safe mode)");
    }

    let app = Router::new()
        .route("/webhook", post(handler::handle_webhook))
        .route("/health", get(health))
        .layer(TraceLayer::new_for_http())
        .with_state(state);

    let listener = match tokio::net::TcpListener::bind(&listen).await {
        Ok(l) => l,
        Err(e) => {
            tracing::error!("failed to bind {listen}: {e}");
            return ExitCode::FAILURE;
        }
    };
    // The address actually bound: with port 0 the system picks one, and that is the one to report.
    match listener.local_addr() {
        Ok(bound) => tracing::info!("{} listening on {bound}", NAME),
        Err(e) => tracing::info!("{} listening on {listen} (bound address unknown: {e})", NAME),
    }
    // On a signal, background runs are cancelled at once while the requests in flight finish.
    // From the moment shutdown begins, everything (those requests, then the background runs and
    // their terminal callbacks) shares one deadline, `bound`: a request that never completes,
    // such as a client that stalls half-way through its body, is cut off when it passes.
    let on_signal = background.clone();
    let http = axum::serve(listener, app)
        .with_graceful_shutdown(async move {
            shutdown_signal().await;
            on_signal.begin_shutdown();
        })
        .into_future();
    tokio::pin!(http);
    let finished_first = tokio::select! {
        served = &mut http => Some(served),
        () = background.cancelled() => None,
    };
    // Also after a server error: running executors are stopped and report before the exit.
    background.begin_shutdown();
    let deadline = tokio::time::Instant::now() + bound;
    let served = match finished_first {
        Some(served) => served,
        None => tokio::time::timeout_at(deadline, http).await.unwrap_or_else(|_| {
            tracing::warn!(
                "request(s) still open {}s after shutdown began; closing them",
                bound.as_secs()
            );
            Ok(())
        }),
    };

    let running = background.running();
    let left = deadline.saturating_duration_since(tokio::time::Instant::now());
    if running > 0 {
        tracing::info!(
            "waiting up to {}s for {running} background run(s) to stop and report",
            left.as_secs()
        );
    }
    if !background.finish(left).await {
        tracing::error!(
            "{} background run(s) did not finish within {}s of shutdown; exiting anyway",
            background.running(),
            bound.as_secs()
        );
    }

    if let Err(e) = served {
        tracing::error!("server error: {e}");
        return ExitCode::FAILURE;
    }
    tracing::info!("{NAME} stopped");
    ExitCode::SUCCESS
}

/// The session manager and the hook socket, when there are `runner` agents and a usable state
/// directory. Without them the tmux channel is refused.
fn start_sessions(
    config: &config::Config,
    runner: &crate::runner::RunnerRuntime,
    background: &Background,
) -> Option<Arc<crate::runner::session::SessionManager>> {
    use sylvode_prereq::{Checker, InstallKind};
    if !config.agents.iter().any(|agent| agent.agent_type == "runner") {
        return None;
    }
    let state = runner.state.as_ref().ok()?;
    let exe = std::env::current_exe()
        .ok()
        .and_then(|raw| crate::runner::session::stable_exe(&raw))?;
    let tmux_program = Checker::new(InstallKind::RunnerTmux).locate(&config.runner.tmux.binary);
    let manager = match crate::runner::session::SessionManager::new(
        Arc::new(config.clone()),
        Arc::clone(state),
        exe,
        tmux_program,
        background.clone(),
    ) {
        Ok(manager) => Arc::new(manager),
        Err(e) => {
            tracing::error!("the tmux channel is not available: {e}");
            return None;
        }
    };
    let (tx, mut rx) = tokio::sync::mpsc::channel(256);
    if let Err(e) = crate::runner::hooks::listen(&state.hook_socket(), tx) {
        tracing::error!("the tmux channel is not available: {e}");
        return None;
    }
    let router = Arc::clone(&manager);
    tokio::spawn(async move {
        while let Some(message) = rx.recv().await {
            router.hook(message).await;
        }
    });
    Some(manager)
}

/// `GET /health`: the service is up; `protocol` is the result of the handshake with Sylvode.
async fn health(State(state): State<Arc<AppState>>) -> Json<serde_json::Value> {
    Json(serde_json::json!({"status": "ok", "protocol": state.handshake.health()}))
}

/// Resolves on SIGINT or SIGTERM.
///
/// Without a handler a process running as PID 1 in a container ignores SIGTERM, so `docker stop`
/// would wait for its timeout and then kill it.
async fn shutdown_signal() {
    let interrupt = async {
        if let Err(e) = tokio::signal::ctrl_c().await {
            tracing::warn!("cannot listen for SIGINT: {e}");
            std::future::pending::<()>().await;
        }
    };

    let terminate = async {
        match tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()) {
            Ok(mut signal) => {
                signal.recv().await;
            }
            Err(e) => {
                tracing::warn!("cannot listen for SIGTERM: {e}");
                std::future::pending::<()>().await;
            }
        }
    };

    tokio::select! {
        () = interrupt => tracing::info!("received SIGINT, shutting down"),
        () = terminate => tracing::info!("received SIGTERM, shutting down"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(items: &[&str]) -> Vec<OsString> {
        items.iter().map(OsString::from).collect()
    }

    #[test]
    fn the_operator_commands_parse() {
        assert_eq!(
            parse_command(&args(&["sessions"])),
            Ok(Command::Sessions {
                config_path: "config.toml".to_string(),
                prune: false
            })
        );
        assert_eq!(
            parse_command(&args(&["sessions", "--prune", "--config", "/etc/r.toml"])),
            Ok(Command::Sessions {
                config_path: "/etc/r.toml".to_string(),
                prune: true
            })
        );
        assert_eq!(
            parse_command(&args(&["trust", "SVWL"])),
            Ok(Command::Trust {
                config_path: "config.toml".to_string(),
                key: "SVWL".to_string()
            })
        );
        assert_eq!(
            parse_command(&args(&["release", "--config", "c.toml", "SVWL"])),
            Ok(Command::Release {
                config_path: "c.toml".to_string(),
                key: "SVWL".to_string()
            })
        );
        for bad in [
            &["trust"][..],
            &["release"],
            &["trust", "A", "B"],
            &["trust", "--prune", "A"],
            &["sessions", "KEY"],
            &["sessions", "--bogus"],
            &["trust", "A", "--config"],
        ] {
            assert!(parse_command(&args(bad)).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn no_argument_serves_the_default_config() {
        assert_eq!(
            parse_command(&[]),
            Ok(Command::Serve {
                config_path: "config.toml".to_string()
            })
        );
    }

    #[test]
    fn flags_and_paths_are_told_apart() {
        assert_eq!(parse_command(&["--help".into()]), Ok(Command::Help));
        assert_eq!(parse_command(&["-h".into()]), Ok(Command::Help));
        assert_eq!(parse_command(&["--version".into()]), Ok(Command::Version));
        assert_eq!(parse_command(&["-V".into()]), Ok(Command::Version));
        assert_eq!(
            parse_command(&["/etc/sylvode-runner/config.toml".into()]),
            Ok(Command::Serve {
                config_path: "/etc/sylvode-runner/config.toml".to_string()
            })
        );
    }

    /// An unknown option is a usage error, not a config path; a path that starts with `-` can
    /// still be given as `./-name`.
    #[test]
    fn an_unknown_option_is_a_usage_error() {
        for arg in ["--bogus-flag", "-x", "--", "-", "--config=x.toml"] {
            let err = parse_command(&[arg.into()]).expect_err("refused");
            assert!(err.contains(&format!("unknown option `{arg}`")), "{err}");
            assert!(err.contains("--help"), "{err}");
        }
        assert_eq!(
            parse_command(&["./-odd.toml".into()]),
            Ok(Command::Serve {
                config_path: "./-odd.toml".to_string()
            })
        );
    }

    #[test]
    fn safe_mode_is_on_for_the_usual_truthy_spellings_only() {
        for (raw, expected) in [
            (Some("1"), true),
            (Some("true"), true),
            (Some(" ON "), true),
            (Some("yes"), true),
            (Some("0"), false),
            (Some(""), false),
            (None, false),
        ] {
            assert_eq!(safe_mode_flag(raw), expected, "{raw:?}");
        }
    }

    #[test]
    fn version_line_uses_the_canonical_name_and_crate_version() {
        assert_eq!(
            version_line(),
            format!("sylvode-runner {}\n", env!("CARGO_PKG_VERSION"))
        );
    }
}
