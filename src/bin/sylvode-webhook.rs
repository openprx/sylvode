//! Canonical executable of Sylvode Webhook.

fn main() -> std::process::ExitCode {
    sylvode_webhook::run(sylvode_webhook::Invocation::Canonical)
}
