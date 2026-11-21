//! The `sylvode-runner` executable's command-line surface: `--help`, `--version` and a failed
//! start-up, each with its stdout, stderr and exit code.

#![cfg(unix)]
// Test helpers outside `#[test]` functions may fail loudly; this is test code only.
#![allow(clippy::expect_used)]

use std::process::{Command, Output};

const RUNNER: &str = env!("CARGO_BIN_EXE_sylvode-runner");

fn run(args: &[&str]) -> Output {
    Command::new(RUNNER)
        .args(args)
        .env_remove("RUST_LOG")
        .env_remove("SYLVODE_RUNNER_SAFE_MODE")
        .env("NO_COLOR", "1")
        .output()
        .expect("spawn executable")
}

#[test]
fn help_names_the_executable_and_writes_nothing_to_stderr() {
    let output = run(&["--help"]);
    assert_eq!(output.status.code(), Some(0));
    let help = String::from_utf8_lossy(&output.stdout);
    assert!(
        help.contains("Usage: sylvode-runner [CONFIG]"),
        "unexpected help: {help}"
    );
    assert!(output.stderr.is_empty(), "{}", String::from_utf8_lossy(&output.stderr));
}

#[test]
fn version_is_the_name_and_the_crate_version() {
    for flag in ["--version", "-V"] {
        let output = run(&[flag]);
        assert_eq!(output.status.code(), Some(0));
        assert_eq!(
            output.stdout,
            format!("sylvode-runner {}\n", env!("CARGO_PKG_VERSION")).into_bytes()
        );
        assert!(output.stderr.is_empty(), "{}", String::from_utf8_lossy(&output.stderr));
    }
}

#[test]
fn a_missing_config_file_fails_the_start_with_exit_code_1() {
    let missing = std::env::temp_dir().join("sylvode-runner-test-missing-config.toml");
    let output = run(&[missing.to_str().expect("utf-8 temp path")]);
    assert_eq!(output.status.code(), Some(1));
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("failed to load config from"), "{stdout}");
    assert!(output.stderr.is_empty(), "{}", String::from_utf8_lossy(&output.stderr));
}
