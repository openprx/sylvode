//! The Sylvode runner executable.

#[cfg(unix)]
fn main() -> std::process::ExitCode {
    sylvode_runner::run()
}

/// The service needs Unix process groups and signals; the library is empty elsewhere.
#[cfg(not(unix))]
fn main() -> std::process::ExitCode {
    use std::io::Write;
    // The exit code reports the failure even when stderr is closed.
    let _ = writeln!(std::io::stderr(), "sylvode-runner is not supported on this platform");
    std::process::ExitCode::FAILURE
}
