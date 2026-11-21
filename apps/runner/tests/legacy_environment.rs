//! Legacy environment spellings, exercised through the real canonical executable.
//!
//! Each test starts `sylvode-runner` against a config file that does not exist, so the process
//! resolves its environment, logs the config error and exits 1 without opening a port. That is
//! enough to observe whether a setting was read and warned about (stderr), whether a log filter
//! selects this service's events, and whether a conflict was refused (exit code and message).
//! That safe mode actually turns the `cli` path off in a running service is shown by
//! `tests/e2e/cli.rs` (`safe_mode_variables_turn_the_cli_path_off`).

#![cfg(unix)]
// Test helpers outside `#[test]` functions may fail loudly; this is test code only.
#![allow(clippy::expect_used)]

use std::process::{Command, Output};

const CANONICAL: &str = env!("CARGO_BIN_EXE_sylvode-runner");
const WEBHOOK_SAFE_MODE_NOTICE: &str = "warning: the environment variable `SYLVODE_WEBHOOK_SAFE_MODE` is deprecated, \
                                        use `SYLVODE_RUNNER_SAFE_MODE` instead; `SYLVODE_WEBHOOK_SAFE_MODE` is not \
                                        removed before Sylvode v2.0";
const SAFE_MODE_NOTICE: &str = "warning: the environment variable `OPENPR_WEBHOOK_SAFE_MODE` is deprecated, use \
                                `SYLVODE_RUNNER_SAFE_MODE` instead; `OPENPR_WEBHOOK_SAFE_MODE` is not removed \
                                before Sylvode v2.0";
const WEBHOOK_LOG_TARGET_NOTICE: &str = "warning: the RUST_LOG target `sylvode_webhook` is deprecated, use \
                                         `sylvode_runner` instead; `sylvode_webhook` is not removed before Sylvode v2.0";
const LOG_TARGET_NOTICE: &str = "warning: the RUST_LOG target `openpr_webhook` is deprecated, use `sylvode_runner` \
                                 instead; `openpr_webhook` is not removed before Sylvode v2.0";

fn run(env: &[(&str, &str)]) -> Output {
    let missing = std::env::temp_dir().join("sylvode-runner-env-test-missing.toml");
    let mut command = Command::new(CANONICAL);
    command
        .arg(missing)
        .env_remove("RUST_LOG")
        .env_remove("SYLVODE_RUNNER_SAFE_MODE")
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
fn legacy_safe_mode_variables_are_read_and_each_warns_once() {
    for (name, notice) in [
        ("SYLVODE_WEBHOOK_SAFE_MODE", WEBHOOK_SAFE_MODE_NOTICE),
        ("OPENPR_WEBHOOK_SAFE_MODE", SAFE_MODE_NOTICE),
    ] {
        let output = run(&[(name, "1")]);
        assert_eq!(output.status.code(), Some(1), "{name}");
        assert!(
            stdout(&output).contains("failed to load config"),
            "{name}: {}",
            stdout(&output)
        );
        assert_eq!(stderr_lines(&output), vec![notice.to_string()], "{name}");
    }
}

#[test]
fn canonical_safe_mode_variable_is_read_without_a_notice() {
    let output = run(&[("SYLVODE_RUNNER_SAFE_MODE", "1")]);
    assert_eq!(output.status.code(), Some(1));
    assert!(stdout(&output).contains("failed to load config"), "{}", stdout(&output));
    assert!(output.stderr.is_empty(), "{:?}", stderr_lines(&output));
}

#[test]
fn safe_mode_variables_that_disagree_refuse_to_start() {
    for other in ["SYLVODE_WEBHOOK_SAFE_MODE", "OPENPR_WEBHOOK_SAFE_MODE"] {
        let output = run(&[("SYLVODE_RUNNER_SAFE_MODE", "1"), (other, "0")]);
        assert_eq!(output.status.code(), Some(1), "{other}");
        let out = stdout(&output);
        assert!(out.contains("disagree"), "{other}: {out}");
        assert!(
            !out.contains("failed to load config"),
            "start-up must stop before the config: {out}"
        );
    }
}

#[test]
fn legacy_log_targets_still_select_this_service_and_warn() {
    for (target, notice) in [
        ("sylvode_webhook", WEBHOOK_LOG_TARGET_NOTICE),
        ("openpr_webhook", LOG_TARGET_NOTICE),
    ] {
        // Without the rewrite, the legacy target would match no event and stdout would be empty.
        let output = run(&[("RUST_LOG", &format!("{target}=error"))]);
        assert_eq!(output.status.code(), Some(1), "{target}");
        assert!(
            stdout(&output).contains("failed to load config"),
            "{target}: {}",
            stdout(&output)
        );
        assert_eq!(stderr_lines(&output), vec![notice.to_string()], "{target}");

        let silenced = run(&[("RUST_LOG", &format!("{target}=off"))]);
        assert_eq!(silenced.status.code(), Some(1), "{target}");
        assert!(silenced.stdout.is_empty(), "{target}: {}", stdout(&silenced));
    }
}

#[test]
fn canonical_log_target_works_without_a_notice() {
    let output = run(&[("RUST_LOG", "sylvode_runner=error")]);
    assert_eq!(output.status.code(), Some(1));
    assert!(stdout(&output).contains("failed to load config"), "{}", stdout(&output));
    assert!(output.stderr.is_empty(), "{:?}", stderr_lines(&output));
}

#[test]
fn log_directives_that_disagree_refuse_to_start() {
    for legacy in ["openpr_webhook=debug", "sylvode_webhook=debug"] {
        let output = run(&[("RUST_LOG", &format!("{legacy},sylvode_runner=error"))]);
        assert_eq!(output.status.code(), Some(1), "{legacy}");
        assert!(output.stdout.is_empty(), "{legacy}: {}", stdout(&output));
        let stderr = stderr_lines(&output);
        assert_eq!(stderr.len(), 1, "{stderr:?}");
        assert!(stderr.iter().all(|line| line.contains("disagree")), "{stderr:?}");
    }
}
