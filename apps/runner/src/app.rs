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
       sylvode-runner --help | --version

Arguments:
  [CONFIG]       Path to the TOML configuration file [default: config.toml]

Options:
  -h, --help     Print this help and exit
  -V, --version  Print the version and exit
";

#[derive(Debug, PartialEq, Eq)]
enum Command {
    Help,
    Version,
    Serve { config_path: String },
}

fn parse_command(first_arg: Option<OsString>) -> Result<Command, String> {
    let Some(arg) = first_arg else {
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
    let command = match parse_command(std::env::args_os().nth(1)) {
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

/// Logs how each `cli` agent's executor runs, and the MCP servers a claude-code agent is allowed
/// by default. An MCP config that cannot be read now is only warned about: the executor reads it
/// when a task runs, and the run fails then if it is still unreadable.
fn log_cli_agent_permissions(config: &config::Config) {
    for (agent, permissions) in config.cli_agent_permissions() {
        tracing::info!(agent = %agent.id, "cli executor permissions: {}", permissions.describe());
        let Some(cli) = agent.cli.as_ref() else { continue };
        if !permissions.needs_mcp_servers() {
            continue;
        }
        let Some(mcp_path) = cli.mcp_config_path.as_deref() else {
            continue;
        };
        let resolved = crate::executor_permissions::resolve_mcp_config_path(mcp_path, cli.workdir.as_deref());
        match crate::executor_permissions::mcp_server_names(&resolved) {
            Ok(servers) => tracing::info!(
                agent = %agent.id,
                "allowed MCP servers from mcp_config_path: [{}]",
                servers.join(", ")
            ),
            Err(e) => tracing::warn!(agent = %agent.id, "mcp_config_path cannot be used yet: {e}"),
        }
    }
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
    log_cli_agent_permissions(&config);

    let background = Background::new(config.runtime.cli_max_concurrency);
    let replay = tokio::sync::Mutex::new(crate::replay::ReplayGuard::new(config.security.max_delivery_age_secs));
    let has_runner_agents = config.agents.iter().any(|agent| agent.agent_type == "runner");
    let runner = crate::runner::RunnerRuntime::new(&config);
    let handshake = crate::handshake::Handshake::new(has_runner_agents);
    let state = Arc::new(AppState {
        config,
        background: background.clone(),
        replay,
        runner,
        handshake,
    });

    if has_runner_agents {
        let loop_config = Arc::new(state.config.clone());
        let http = state.runner.http.clone();
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

    #[test]
    fn no_argument_serves_the_default_config() {
        assert_eq!(
            parse_command(None),
            Ok(Command::Serve {
                config_path: "config.toml".to_string()
            })
        );
    }

    #[test]
    fn flags_and_paths_are_told_apart() {
        assert_eq!(parse_command(Some("--help".into())), Ok(Command::Help));
        assert_eq!(parse_command(Some("-h".into())), Ok(Command::Help));
        assert_eq!(parse_command(Some("--version".into())), Ok(Command::Version));
        assert_eq!(parse_command(Some("-V".into())), Ok(Command::Version));
        assert_eq!(
            parse_command(Some("/etc/sylvode-runner/config.toml".into())),
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
            let err = parse_command(Some(arg.into())).expect_err("refused");
            assert!(err.contains(&format!("unknown option `{arg}`")), "{err}");
            assert!(err.contains("--help"), "{err}");
        }
        assert_eq!(
            parse_command(Some("./-odd.toml".into())),
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
