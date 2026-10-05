//! Installation of the global tracing subscriber described by the `[logging]` section.

use tracing_subscriber::fmt::writer::BoxMakeWriter;
use tracing_subscriber::{EnvFilter, fmt};

use crate::config::{LogFormat, LogOutput, LoggingConfig, StdoutRole};
use crate::error::{AppError, AppResult};

/// Installs the global tracing subscriber for a binary whose stdout carries nothing but logs.
///
/// The filter comes from the configuration file, never from `RUST_LOG`: a deployment's log level
/// is part of its configuration, and reading it from the environment made the effective level
/// depend on whatever the surrounding shell happened to export.
///
/// The stream comes from `logging.output`, which defaults to stderr. Use
/// [`init_reserving_stdout`] instead from a binary that writes a protocol on stdout.
pub fn init(logging: &LoggingConfig, service_name: &str) -> AppResult<()> {
    install(logging, service_name, logging.effective_output(StdoutRole::Free))
}

/// A `logging.output` the process could not honour.
///
/// Returned by [`init_reserving_stdout`] for the caller to log under its own target, because the
/// default filter (`<service>=info,tower_http=info`) enables only the calling binary's target: a
/// warning emitted from this module, under `platform::logging`, was dropped by exactly the
/// configuration that most needs to see it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct StreamOverride {
    /// What `logging.output` asked for.
    pub configured: LogOutput,
    /// Where logs actually go.
    pub effective: LogOutput,
}

impl StreamOverride {
    /// The text the caller logs.
    pub const MESSAGE: &'static str =
        "logging.output was overridden: this process frames a protocol on stdout, so logs go to stderr";
}

/// Installs the global tracing subscriber for a binary whose stdout carries a protocol.
///
/// Logs go to stderr unconditionally. The MCP server's stdio transport frames JSON-RPC on stdout,
/// where a single log line corrupts a frame and ends the session; that is a property of the
/// transport, not a preference, so `logging.output = "stdout"` cannot override it. One file
/// configures three binaries, and the setting the API wants must not be able to break the MCP
/// server. An overridden setting is returned rather than dropped silently, and the caller logs
/// it (see [`StreamOverride`]) so the operator learns that the file says something the process
/// cannot honour.
pub fn init_reserving_stdout(logging: &LoggingConfig, service_name: &str) -> AppResult<Option<StreamOverride>> {
    let effective = logging.effective_output(StdoutRole::Protocol);
    install(logging, service_name, effective)?;
    Ok((logging.output != effective).then_some(StreamOverride {
        configured: logging.output,
        effective,
    }))
}

/// Runs `emit` under a subscriber built from `[logging]` that writes to stderr, without
/// installing anything process-wide.
///
/// For a process that never installs a global subscriber — the `sylvode` Flow commands, whose
/// stderr carries only their own diagnostics — but still has to write one log line, such as
/// the ADR-0020 D2 legacy configuration notice. The format and filter are the file's; the
/// stream is always stderr, because such a process prints its result on stdout.
pub fn with_stderr_subscriber(logging: &LoggingConfig, service_name: &str, emit: impl FnOnce()) -> AppResult<()> {
    let filter = filter_for(logging, service_name)?;
    let writer = BoxMakeWriter::new(std::io::stderr);
    match logging.format {
        LogFormat::Json => {
            let subscriber = fmt()
                .with_env_filter(filter)
                .with_writer(writer)
                .json()
                .with_current_span(true)
                .with_span_list(true)
                .finish();
            tracing::subscriber::with_default(subscriber, emit);
        }
        LogFormat::Text => {
            let subscriber = fmt().with_env_filter(filter).with_writer(writer).finish();
            tracing::subscriber::with_default(subscriber, emit);
        }
    }
    Ok(())
}

/// Builds the subscriber and makes it the process-wide default.
fn install(logging: &LoggingConfig, service_name: &str, output: LogOutput) -> AppResult<()> {
    let filter = filter_for(logging, service_name)?;
    let writer = match output {
        LogOutput::Stderr => BoxMakeWriter::new(std::io::stderr),
        LogOutput::Stdout => BoxMakeWriter::new(std::io::stdout),
    };

    let installed = match logging.format {
        LogFormat::Json => fmt()
            .with_env_filter(filter)
            .with_writer(writer)
            .json()
            .with_current_span(true)
            .with_span_list(true)
            .try_init(),
        LogFormat::Text => fmt().with_env_filter(filter).with_writer(writer).try_init(),
    };

    installed.map_err(|err| AppError::Config(format!("the tracing subscriber could not be installed: {err}")))
}

/// Resolves `logging.filter` into a subscriber filter.
fn filter_for(logging: &LoggingConfig, service_name: &str) -> AppResult<EnvFilter> {
    let directives = logging.filter_or_default(service_name);
    EnvFilter::try_new(&directives).map_err(|err| {
        AppError::Config(format!(
            "logging.filter {directives} is not a valid tracing filter: {err}"
        ))
    })
}

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::pedantic,
    clippy::nursery
)]
mod tests {
    use super::{AppError, LogOutput, LoggingConfig, StdoutRole, filter_for};
    use std::io::Write;
    use std::sync::Arc;
    use tracing_subscriber::fmt::MakeWriter;

    /// A writer that keeps everything the subscriber wrote, for the test to read back.
    #[derive(Clone, Default)]
    struct Captured(Arc<parking_lot::Mutex<Vec<u8>>>);

    impl Write for Captured {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            self.0.lock().extend_from_slice(buf);
            Ok(buf.len())
        }

        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    impl<'a> MakeWriter<'a> for Captured {
        type Writer = Self;

        fn make_writer(&'a self) -> Self::Writer {
            self.clone()
        }
    }

    impl Captured {
        fn text(&self) -> String {
            String::from_utf8_lossy(&self.0.lock()).into_owned()
        }
    }

    fn logging(output: LogOutput) -> LoggingConfig {
        LoggingConfig {
            output,
            ..LoggingConfig::default()
        }
    }

    #[test]
    fn the_default_stream_is_stderr_so_a_log_line_can_never_corrupt_stdout() {
        let logging = LoggingConfig::default();
        assert_eq!(logging.output, LogOutput::Stderr);
        assert_eq!(logging.effective_output(StdoutRole::Free), LogOutput::Stderr);
    }

    #[test]
    fn a_configured_stdout_stream_is_honoured_when_stdout_carries_nothing_else() {
        assert_eq!(
            logging(LogOutput::Stdout).effective_output(StdoutRole::Free),
            LogOutput::Stdout
        );
    }

    #[test]
    fn stdout_is_never_used_by_a_process_that_frames_a_protocol_on_it() {
        for configured in [LogOutput::Stderr, LogOutput::Stdout] {
            assert_eq!(
                logging(configured).effective_output(StdoutRole::Protocol),
                LogOutput::Stderr,
                "logging.output = {} must not reach a stdio transport",
                configured.as_str()
            );
        }
    }

    #[test]
    fn the_filter_falls_back_to_the_calling_binary_name() {
        let filter = filter_for(&LoggingConfig::default(), "mcp_server").expect("the default filter is valid");
        assert!(filter.to_string().contains("mcp_server=info"), "{filter}");
    }

    #[test]
    fn an_unusable_filter_is_reported_as_a_configuration_error_rather_than_installed() {
        let logging = LoggingConfig {
            filter: Some("api=verbose".to_string()),
            ..LoggingConfig::default()
        };
        let error = filter_for(&logging, "api").expect_err("a malformed filter must not be installed");
        assert!(matches!(error, AppError::Config(_)));
        assert!(error.to_string().contains("logging.filter"), "{error}");
    }

    /// Under the default filter a warning emitted from this module (`platform::logging`) is
    /// dropped while one under the binary's own target is kept, which is why the stdout override
    /// is handed back to the caller instead of being logged here. Nothing else changes: other
    /// targets are filtered exactly as before.
    #[test]
    fn the_default_filter_keeps_the_service_target_and_drops_this_module() {
        let captured = Captured::default();
        let subscriber = tracing_subscriber::fmt()
            .with_env_filter(filter_for(&LoggingConfig::default(), "mcp_server").expect("default filter"))
            .with_writer(captured.clone())
            .finish();
        tracing::subscriber::with_default(subscriber, || {
            tracing::warn!(target: "platform::logging", "from-platform");
            tracing::warn!(target: "mcp_server::cli", "from-service");
            tracing::info!(target: "tower_http::trace", "from-tower");
            tracing::warn!(target: "sqlx::query", "from-sqlx");
        });
        let text = captured.text();
        assert!(text.contains("from-service"), "{text}");
        assert!(text.contains("from-tower"), "{text}");
        assert!(!text.contains("from-platform"), "{text}");
        assert!(!text.contains("from-sqlx"), "{text}");
    }
}
