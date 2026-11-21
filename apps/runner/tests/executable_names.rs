//! The former `sylvode-webhook` and `openpr-webhook` executables must behave exactly like
//! `sylvode-runner`.
//!
//! What is compared, and why:
//! - `--help` and `--version`: the whole stdout, byte for byte, and the exit code. Neither output
//!   prints the invocation name, so any difference at all is a regression.
//! - A start-up that fails (missing config file): the exit code, and stdout byte for byte after
//!   dropping the leading timestamp of each log line. The timestamp differs between any two
//!   processes, not between the two names; everything after it must be identical.
//! - stderr: each former name writes exactly one deprecation line naming the old name and the
//!   replacement; the canonical name writes nothing.

#![cfg(unix)]
// Test helpers outside `#[test]` functions may fail loudly; this is test code only.
#![allow(clippy::expect_used)]

use std::process::{Command, Output};

const CANONICAL: &str = env!("CARGO_BIN_EXE_sylvode-runner");
const ALIAS: &str = env!("CARGO_BIN_EXE_sylvode-webhook");
const LEGACY: &str = env!("CARGO_BIN_EXE_openpr-webhook");
const ALIAS_NOTICE: &str = "sylvode-webhook is now sylvode-runner; the old name is kept until Sylvode v2.0";
const LEGACY_NOTICE: &str = "warning: the executable name `openpr-webhook` is deprecated, use `sylvode-runner` instead; \
                             `openpr-webhook` is not removed before Sylvode v2.0";

fn run(binary: &str, args: &[&str]) -> Output {
    Command::new(binary)
        .args(args)
        .env_remove("RUST_LOG")
        .env_remove("SYLVODE_RUNNER_SAFE_MODE")
        .env_remove("SYLVODE_WEBHOOK_SAFE_MODE")
        .env_remove("OPENPR_WEBHOOK_SAFE_MODE")
        .env("NO_COLOR", "1")
        .output()
        .expect("spawn executable")
}

fn stderr_lines(output: &Output) -> Vec<String> {
    String::from_utf8_lossy(&output.stderr)
        .lines()
        .map(str::to_owned)
        .collect()
}

fn assert_notices_only_for_former_names(canonical: &Output, alias: &Output, legacy: &Output) {
    assert!(
        canonical.stderr.is_empty(),
        "canonical name must not warn, stderr was: {}",
        String::from_utf8_lossy(&canonical.stderr)
    );
    assert_eq!(
        stderr_lines(alias),
        vec![ALIAS_NOTICE.to_string()],
        "sylvode-webhook must warn exactly once"
    );
    assert_eq!(
        stderr_lines(legacy),
        vec![LEGACY_NOTICE.to_string()],
        "openpr-webhook must warn exactly once"
    );
}

#[test]
fn help_is_byte_identical_and_only_the_former_names_warn() {
    let canonical = run(CANONICAL, &["--help"]);
    let alias = run(ALIAS, &["--help"]);
    let legacy = run(LEGACY, &["--help"]);

    assert_eq!(canonical.status.code(), Some(0));
    assert_eq!(alias.status.code(), canonical.status.code());
    assert_eq!(legacy.status.code(), canonical.status.code());
    let help = String::from_utf8_lossy(&canonical.stdout);
    assert!(
        help.contains("Usage: sylvode-runner [CONFIG]"),
        "unexpected help: {help}"
    );
    assert!(
        help.contains("are not removed before Sylvode v2.0."),
        "help must name the Sylvode release: {help}"
    );
    assert_eq!(alias.stdout, canonical.stdout, "help stdout must be byte-identical");
    assert_eq!(legacy.stdout, canonical.stdout, "help stdout must be byte-identical");
    assert_notices_only_for_former_names(&canonical, &alias, &legacy);
}

#[test]
fn version_is_byte_identical_and_only_the_former_names_warn() {
    for flag in ["--version", "-V"] {
        let canonical = run(CANONICAL, &[flag]);
        let alias = run(ALIAS, &[flag]);
        let legacy = run(LEGACY, &[flag]);

        assert_eq!(canonical.status.code(), Some(0));
        assert_eq!(alias.status.code(), canonical.status.code());
        assert_eq!(legacy.status.code(), canonical.status.code());
        assert_eq!(
            canonical.stdout,
            format!("sylvode-runner {}\n", env!("CARGO_PKG_VERSION")).into_bytes()
        );
        assert_eq!(alias.stdout, canonical.stdout, "{flag} stdout must be byte-identical");
        assert_eq!(legacy.stdout, canonical.stdout, "{flag} stdout must be byte-identical");
        assert_notices_only_for_former_names(&canonical, &alias, &legacy);
    }
}

fn without_timestamps(stdout: &[u8]) -> Vec<String> {
    String::from_utf8_lossy(stdout)
        .lines()
        .map(|line| {
            let (timestamp, rest) = line.split_once(' ').unwrap_or(("", line));
            assert!(
                timestamp.starts_with(|c: char| c.is_ascii_digit()),
                "log line does not start with a timestamp: {line}"
            );
            rest.to_owned()
        })
        .collect()
}

#[test]
fn failed_start_has_the_same_exit_code_and_log_output() {
    let missing = std::env::temp_dir().join("sylvode-runner-test-missing-config.toml");
    let missing = missing.to_str().expect("utf-8 temp path");

    let canonical = run(CANONICAL, &[missing]);
    let alias = run(ALIAS, &[missing]);
    let legacy = run(LEGACY, &[missing]);

    assert_eq!(canonical.status.code(), Some(1));
    assert_eq!(alias.status.code(), canonical.status.code());
    assert_eq!(legacy.status.code(), canonical.status.code());
    let canonical_lines = without_timestamps(&canonical.stdout);
    assert!(
        canonical_lines
            .iter()
            .any(|line| line.contains("failed to load config from")),
        "expected the config error on stdout, got {canonical_lines:?}"
    );
    assert_eq!(without_timestamps(&alias.stdout), canonical_lines);
    assert_eq!(without_timestamps(&legacy.stdout), canonical_lines);
    assert_notices_only_for_former_names(&canonical, &alias, &legacy);
}
