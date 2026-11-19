//! Legacy executable name of Sylvode Webhook, kept for the whole Sylvode 1.x line.
//!
//! Runs the same code as `sylvode-webhook` and additionally writes one deprecation line to stderr.

#[cfg(unix)]
fn main() -> std::process::ExitCode {
    sylvode_webhook::run(sylvode_webhook::Invocation::Legacy)
}

/// The service needs Unix process groups and signals; the library is empty elsewhere.
#[cfg(not(unix))]
fn main() -> std::process::ExitCode {
    use std::io::Write;
    // The exit code reports the failure even when stderr is closed.
    let _ = writeln!(std::io::stderr(), "sylvode-webhook is not supported on this platform");
    std::process::ExitCode::FAILURE
}
