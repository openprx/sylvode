//! `init_reserving_stdout` installs a process-wide subscriber, so it runs in this test binary of
//! its own, once.

use platform::config::{LogOutput, LoggingConfig};
use platform::logging::{StreamOverride, init_reserving_stdout};

/// A `logging.output = "stdout"` the process cannot honour is handed back to the caller, which
/// is the one that can log it under a target the default filter enables.
#[test]
fn a_stdout_request_is_returned_to_the_caller_as_an_override() -> Result<(), Box<dyn std::error::Error>> {
    let logging = LoggingConfig {
        output: LogOutput::Stdout,
        ..LoggingConfig::default()
    };
    let overridden = init_reserving_stdout(&logging, "reserved_stdout_logging")?;
    assert_eq!(
        overridden,
        Some(StreamOverride {
            configured: LogOutput::Stdout,
            effective: LogOutput::Stderr,
        })
    );
    Ok(())
}
