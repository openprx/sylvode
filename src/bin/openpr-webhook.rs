//! Legacy executable name of Sylvode Webhook, kept for the whole Sylvode 1.x line.
//!
//! Runs the same code as `sylvode-webhook` and additionally writes one deprecation line to stderr.

fn main() -> std::process::ExitCode {
    sylvode_webhook::run(sylvode_webhook::Invocation::Legacy)
}
