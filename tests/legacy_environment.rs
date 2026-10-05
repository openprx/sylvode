//! Legacy environment spellings, exercised through the real canonical executable.
//!
//! Each test starts `sylvode-webhook` against a config file that does not exist, so the process
//! resolves its environment, logs the config error and exits 1 without opening a port. That is
//! enough to observe whether a setting was honoured (the log line appears or not), whether it
//! warned (stderr), and whether a conflict was refused (exit code and message).

// Test helpers outside `#[test]` functions may fail loudly; this is test code only.
#![allow(clippy::expect_used)]

use std::process::{Command, Output};

const CANONICAL: &str = env!("CARGO_BIN_EXE_sylvode-webhook");
const SAFE_MODE_NOTICE: &str = "warning: the environment variable `OPENPR_WEBHOOK_SAFE_MODE` is deprecated, use \
                                `SYLVODE_WEBHOOK_SAFE_MODE` instead; `OPENPR_WEBHOOK_SAFE_MODE` is not removed \
                                before Sylvode v2.0";
const LOG_TARGET_NOTICE: &str = "warning: the RUST_LOG target `openpr_webhook` is deprecated, use `sylvode_webhook` \
                                 instead; `openpr_webhook` is not removed before Sylvode v2.0";

fn run(env: &[(&str, &str)]) -> Output {
    let missing = std::env::temp_dir().join("sylvode-webhook-env-test-missing.toml");
    let mut command = Command::new(CANONICAL);
    command
        .arg(missing)
        .env_remove("RUST_LOG")
        .env_remove("SYLVODE_WEBHOOK_SAFE_MODE")
        .env_remove("OPENPR_WEBHOOK_SAFE_MODE")
        .env("NO_COLOR", "1");
    for (key, value) in env {
        command.env(key, value);
    }
    command.output().expect("spawn executable")
}

fn stdout(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).into_owned()
}

fn stderr_lines(output: &Output) -> Vec<String> {
    String::from_utf8_lossy(&output.stderr)
        .lines()
        .map(str::to_owned)
        .collect()
}

#[test]
fn legacy_safe_mode_variable_works_and_warns() {
    let output = run(&[("OPENPR_WEBHOOK_SAFE_MODE", "1")]);
    assert_eq!(output.status.code(), Some(1));
    assert!(stdout(&output).contains("failed to load config"), "{}", stdout(&output));
    assert_eq!(stderr_lines(&output), vec![SAFE_MODE_NOTICE.to_string()]);
}

#[test]
fn canonical_safe_mode_variable_works_without_a_notice() {
    let output = run(&[("SYLVODE_WEBHOOK_SAFE_MODE", "1")]);
    assert_eq!(output.status.code(), Some(1));
    assert!(stdout(&output).contains("failed to load config"), "{}", stdout(&output));
    assert!(output.stderr.is_empty(), "{:?}", stderr_lines(&output));
}

#[test]
fn safe_mode_variables_that_disagree_refuse_to_start() {
    let output = run(&[("SYLVODE_WEBHOOK_SAFE_MODE", "1"), ("OPENPR_WEBHOOK_SAFE_MODE", "0")]);
    assert_eq!(output.status.code(), Some(1));
    let out = stdout(&output);
    assert!(out.contains("disagree"), "{out}");
    assert!(
        !out.contains("failed to load config"),
        "start-up must stop before the config: {out}"
    );
}

#[test]
fn legacy_log_target_still_selects_this_service_and_warns() {
    // Without the rewrite, `openpr_webhook=error` would match no event and stdout would be empty.
    let output = run(&[("RUST_LOG", "openpr_webhook=error")]);
    assert_eq!(output.status.code(), Some(1));
    assert!(stdout(&output).contains("failed to load config"), "{}", stdout(&output));
    assert_eq!(stderr_lines(&output), vec![LOG_TARGET_NOTICE.to_string()]);

    let silenced = run(&[("RUST_LOG", "openpr_webhook=off")]);
    assert_eq!(silenced.status.code(), Some(1));
    assert!(silenced.stdout.is_empty(), "{}", stdout(&silenced));
}

#[test]
fn canonical_log_target_works_without_a_notice() {
    let output = run(&[("RUST_LOG", "sylvode_webhook=error")]);
    assert_eq!(output.status.code(), Some(1));
    assert!(stdout(&output).contains("failed to load config"), "{}", stdout(&output));
    assert!(output.stderr.is_empty(), "{:?}", stderr_lines(&output));
}

#[test]
fn log_directives_that_disagree_refuse_to_start() {
    let output = run(&[("RUST_LOG", "openpr_webhook=debug,sylvode_webhook=error")]);
    assert_eq!(output.status.code(), Some(1));
    assert!(output.stdout.is_empty(), "{}", stdout(&output));
    let stderr = stderr_lines(&output);
    assert_eq!(stderr.len(), 1, "{stderr:?}");
    assert!(stderr.iter().all(|line| line.contains("disagree")), "{stderr:?}");
}
