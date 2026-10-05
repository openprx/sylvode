use crate::{AppState, background::Background, compat, config, handler, process, tunnel};
use axum::{
    Router,
    routing::{get, post},
};
use std::ffi::OsString;
use std::io::Write;
use std::process::ExitCode;
use std::sync::Arc;
use std::time::Duration;
use tower_http::trace::TraceLayer;

const DEFAULT_CONFIG_PATH: &str = "config.toml";
const DEFAULT_LOG_FILTER: &str = "sylvode_webhook=info";

/// Help text. It names the canonical executable whichever name the process was started under,
/// so stdout is byte-for-byte identical for both names.
const HELP: &str = "\
Sylvode Webhook (formerly OpenPR Webhook): receives Sylvode webhook deliveries and dispatches them to agents.

Usage: sylvode-webhook [CONFIG]
       sylvode-webhook --help | --version

Arguments:
  [CONFIG]       Path to the TOML configuration file [default: config.toml]

Options:
  -h, --help     Print this help and exit
  -V, --version  Print the version and exit

The legacy executable name openpr-webhook runs the same program and is not removed before Sylvode v2.0.
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
    format!("{} {}\n", compat::CANONICAL_NAME, env!("CARGO_PKG_VERSION"))
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

/// Runs the service. Both executables call this; `invocation` only decides whether the
/// deprecation notice for the legacy executable name is written.
pub fn run(invocation: compat::Invocation) -> ExitCode {
    if invocation == compat::Invocation::Legacy {
        compat::warn_once(compat::LegacyEntry::ExecutableName);
    }

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

/// Sets up logging from `RUST_LOG`, honouring directives for the legacy `openpr_webhook` target.
fn init_tracing() -> Result<(), String> {
    let resolved = compat::resolve_log_filter(std::env::var("RUST_LOG").ok().as_deref())?;
    if resolved.used_legacy {
        compat::warn_once(compat::LegacyEntry::LogTarget);
    }
    let filter = resolved
        .value
        .and_then(|directives| tracing_subscriber::EnvFilter::try_new(directives).ok())
        .unwrap_or_else(|| DEFAULT_LOG_FILTER.into());
    if let Err(e) = tracing_subscriber::fmt().with_env_filter(filter).try_init() {
        write_stderr_line(&format!("error: failed to initialise logging: {e}"));
    }
    Ok(())
}

fn resolve_safe_mode() -> Result<bool, String> {
    let canonical = std::env::var(compat::SAFE_MODE_ENV).ok();
    let legacy = std::env::var(compat::LEGACY_SAFE_MODE_ENV).ok();
    let resolved = compat::resolve_safe_mode(canonical.as_deref(), legacy.as_deref())?;
    if resolved.used_legacy {
        compat::warn_once(compat::LegacyEntry::SafeModeEnv);
    }
    Ok(resolved.value)
}

fn serve_blocking(config_path: &str) -> ExitCode {
    // Logging is not up yet when its own configuration is refused, so that error goes to stderr.
    if let Err(e) = init_tracing() {
        write_stderr_line(&format!("error: {e}"));
        return ExitCode::FAILURE;
    }

    let safe_mode = match resolve_safe_mode() {
        Ok(safe_mode) => safe_mode,
        Err(e) => {
            tracing::error!("{e}");
            return ExitCode::FAILURE;
        }
    };

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

/// How long shutdown waits for background runs: the process-group grace period, then a
/// terminal callback of up to two requests (MCP sends a comment and a state update), plus slack.
fn shutdown_bound(http_timeout_secs: u64) -> Duration {
    process::TERMINATION_GRACE + Duration::from_secs(http_timeout_secs.saturating_mul(2)) + Duration::from_secs(5)
}

async fn serve(config: config::Config) -> ExitCode {
    let listen = config.server.listen.clone();
    let bound = shutdown_bound(config.runtime.http_timeout_secs);
    tracing::info!("Loaded {} agent(s)", config.agents.len());

    let background = Background::new(config.runtime.cli_max_concurrency);
    let replay = tokio::sync::Mutex::new(crate::replay::ReplayGuard::new(config.security.max_delivery_age_secs));
    let state = Arc::new(AppState {
        config,
        background: background.clone(),
        replay,
    });

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
        .route("/health", get(|| async { "ok" }))
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
        Ok(bound) => tracing::info!("{} listening on {bound}", compat::CANONICAL_NAME),
        Err(e) => tracing::info!(
            "{} listening on {listen} (bound address unknown: {e})",
            compat::CANONICAL_NAME
        ),
    }
    // On a signal, background runs are cancelled at once, while in-flight requests finish.
    let on_signal = background.clone();
    let served = axum::serve(listener, app)
        .with_graceful_shutdown(async move {
            shutdown_signal().await;
            on_signal.begin_shutdown();
        })
        .await;

    // Also after a server error: running executors are stopped and report before the exit.
    background.begin_shutdown();
    let running = background.running();
    if running > 0 {
        tracing::info!(
            "waiting up to {}s for {running} background run(s) to stop and report",
            bound.as_secs()
        );
    }
    if !background.finish(bound).await {
        tracing::error!(
            "{} background run(s) did not finish within {}s; exiting anyway",
            background.running(),
            bound.as_secs()
        );
    }

    if let Err(e) = served {
        tracing::error!("server error: {e}");
        return ExitCode::FAILURE;
    }
    tracing::info!("{} stopped", compat::CANONICAL_NAME);
    ExitCode::SUCCESS
}

/// Resolves on SIGINT or, on Unix, SIGTERM.
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

    #[cfg(unix)]
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
    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

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
            parse_command(Some("/etc/sylvode-webhook/config.toml".into())),
            Ok(Command::Serve {
                config_path: "/etc/sylvode-webhook/config.toml".to_string()
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
    fn version_line_uses_the_canonical_name_and_crate_version() {
        assert_eq!(
            version_line(),
            format!("sylvode-webhook {}\n", env!("CARGO_PKG_VERSION"))
        );
    }
}
