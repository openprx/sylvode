//! The legacy `openpr-webhook` executable must behave exactly like `sylvode-webhook`.
//!
//! What is compared, and why:
//! - `--help` and `--version`: the whole stdout, byte for byte, and the exit code. Neither output
//!   prints the invocation name, so any difference at all is a regression.
//! - A start-up that fails (missing config file): the exit code, and stdout byte for byte after
//!   dropping the leading timestamp of each log line. The timestamp differs between any two
//!   processes, not between the two names; everything after it must be identical.
//! - stderr: the legacy name writes exactly one deprecation line naming the old name, the
//!   replacement and the earliest removal; the canonical name writes nothing.

// Test helpers outside `#[test]` functions may fail loudly; this is test code only.
#![allow(clippy::expect_used)]

use std::process::{Command, Output};

const CANONICAL: &str = env!("CARGO_BIN_EXE_sylvode-webhook");
const LEGACY: &str = env!("CARGO_BIN_EXE_openpr-webhook");
const NOTICE: &str = "warning: the executable name `openpr-webhook` is deprecated, use `sylvode-webhook` instead; \
                      `openpr-webhook` is not removed before Sylvode v2.0";

fn run(binary: &str, args: &[&str]) -> Output {
    Command::new(binary)
        .args(args)
        .env_remove("RUST_LOG")
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

fn assert_notice_only_for_legacy(canonical: &Output, legacy: &Output) {
    assert!(
        canonical.stderr.is_empty(),
        "canonical name must not warn, stderr was: {}",
        String::from_utf8_lossy(&canonical.stderr)
    );
    assert_eq!(
        stderr_lines(legacy),
        vec![NOTICE.to_string()],
        "legacy name must warn exactly once"
    );
}

#[test]
fn help_is_byte_identical_and_only_the_legacy_name_warns() {
    let canonical = run(CANONICAL, &["--help"]);
    let legacy = run(LEGACY, &["--help"]);

    assert_eq!(canonical.status.code(), Some(0));
    assert_eq!(legacy.status.code(), canonical.status.code());
    let help = String::from_utf8_lossy(&canonical.stdout);
    assert!(
        help.contains("Usage: sylvode-webhook [CONFIG]"),
        "unexpected help: {help}"
    );
    assert!(
        help.contains("is not removed before Sylvode v2.0."),
        "help must name the Sylvode release: {help}"
    );
    assert_eq!(legacy.stdout, canonical.stdout, "help stdout must be byte-identical");
    assert_notice_only_for_legacy(&canonical, &legacy);
}

#[test]
fn version_is_byte_identical_and_only_the_legacy_name_warns() {
    for flag in ["--version", "-V"] {
        let canonical = run(CANONICAL, &[flag]);
        let legacy = run(LEGACY, &[flag]);

        assert_eq!(canonical.status.code(), Some(0));
        assert_eq!(legacy.status.code(), canonical.status.code());
        assert_eq!(
            canonical.stdout,
            format!("sylvode-webhook {}\n", env!("CARGO_PKG_VERSION")).into_bytes()
        );
        assert_eq!(legacy.stdout, canonical.stdout, "{flag} stdout must be byte-identical");
        assert_notice_only_for_legacy(&canonical, &legacy);
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
    let missing = std::env::temp_dir().join("sylvode-webhook-test-missing-config.toml");
    let missing = missing.to_str().expect("utf-8 temp path");

    let canonical = run(CANONICAL, &[missing]);
    let legacy = run(LEGACY, &[missing]);

    assert_eq!(canonical.status.code(), Some(1));
    assert_eq!(legacy.status.code(), canonical.status.code());
    let canonical_lines = without_timestamps(&canonical.stdout);
    assert!(
        canonical_lines
            .iter()
            .any(|line| line.contains("failed to load config from")),
        "expected the config error on stdout, got {canonical_lines:?}"
    );
    assert_eq!(without_timestamps(&legacy.stdout), canonical_lines);
    assert_notice_only_for_legacy(&canonical, &legacy);
}
