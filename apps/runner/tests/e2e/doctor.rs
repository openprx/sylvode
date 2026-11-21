//! `doctor`, `--check` and `config check` through the real executable, with fake binaries on a
//! temporary `PATH`.

use crate::support::{RUNNER, TempDir, write_script};
use serde_json::Value;
use std::path::Path;
use std::process::{Command, Output};

/// A `PATH` directory with fakes: tmux 3.4 (no server running), claude, and an MCP server whose
/// `--check` reports task protocol 1.
fn fakes(with_tmux: bool) -> TempDir {
    let dir = TempDir::new("doctor-bin");
    if with_tmux {
        write_script(
            dir.path(),
            "tmux",
            "case \"$1\" in -V) echo 'tmux 3.4';; *) exit 1;; esac",
        );
    }
    write_script(dir.path(), "claude", "echo '2.1.330 (Claude Code)'");
    write_script(
        dir.path(),
        "mcp-server",
        "echo '{\"schema\":\"sylvode.doctor.v1\",\"generated_by\":{\"binary\":\"mcp-server\",\"version\":\"0.5.0\",\"task_protocol\":1},\"components\":[]}'",
    );
    dir
}

fn run(path: &Path, args: &[&str]) -> Output {
    Command::new(RUNNER)
        .args(args)
        .env_clear()
        .env("PATH", format!("{}:/usr/bin:/bin", path.display()))
        .env("HOME", path)
        .output()
        .expect("run the runner")
}

fn json(output: &Output) -> Value {
    serde_json::from_slice(&output.stdout).unwrap_or_else(|e| {
        panic!(
            "not JSON ({e}): {}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        )
    })
}

fn row<'a>(report: &'a Value, name: &str) -> &'a Value {
    report["components"]
        .as_array()
        .and_then(|rows| rows.iter().find(|r| r["component"] == name))
        .unwrap_or_else(|| panic!("no row {name}: {report}"))
}

#[test]
fn the_report_has_the_frozen_shape_and_exits_zero_when_nothing_failed() {
    let bin = fakes(true);
    let config = TempDir::new("doctor-cfg");
    let file = config.file("absent.toml");
    let output = run(
        bin.path(),
        &[
            "doctor",
            "--json",
            "--mode",
            "tmux",
            "--config",
            &file.to_string_lossy(),
        ],
    );
    let report = json(&output);
    assert_eq!(report["schema"], "sylvode.doctor.v1");
    assert_eq!(report["generated_by"]["binary"], "sylvode-runner");
    assert_eq!(report["generated_by"]["task_protocol"], 1);
    assert_eq!(report["context"]["mode"], "tmux");
    assert_eq!(report["context"]["config"], "missing");
    assert_eq!(report["context"]["as_service"], false);
    for name in [
        "tmux",
        "cli.claude",
        "cli.codex",
        "cli.opencode",
        "mcp_server",
        "runner",
    ] {
        let _ = row(&report, name);
    }
    assert_eq!(row(&report, "tmux")["found"], true);
    assert_eq!(row(&report, "tmux")["version"], "3.4");
    assert_eq!(row(&report, "tmux")["detail"]["server_running"], false);
    assert_eq!(row(&report, "cli.claude")["found"], true);
    assert_eq!(row(&report, "mcp_server")["detail"]["task_protocol"], 1);
    assert_eq!(row(&report, "runner")["detail"]["config"], "missing");
    assert_eq!(report["summary"]["fail"], 0, "{report}");
    assert_eq!(output.status.code(), Some(0));
}

#[test]
fn a_missing_tmux_fails_the_row_with_a_hint_and_exits_two() {
    let bin = fakes(false);
    let config = TempDir::new("doctor-cfg");
    let output = run(
        bin.path(),
        &[
            "doctor",
            "--json",
            "--mode",
            "tmux",
            "--config",
            &config.file("absent.toml").to_string_lossy(),
        ],
    );
    let report = json(&output);
    let tmux = row(&report, "tmux");
    assert_eq!(
        (tmux["found"].as_bool(), tmux["status"].as_str()),
        (Some(false), Some("fail"))
    );
    assert!(
        tmux["hint"].as_str().unwrap_or_default().contains("brew install tmux"),
        "{tmux}"
    );
    assert_eq!(output.status.code(), Some(2));
    // Headless: a missing tmux is only a warning, and the exit code is 0.
    let headless = run(
        bin.path(),
        &[
            "doctor",
            "--json",
            "--mode",
            "headless",
            "--config",
            &config.file("absent.toml").to_string_lossy(),
        ],
    );
    let report = json(&headless);
    assert_eq!(row(&report, "tmux")["status"], "warn", "{report}");
    assert_eq!(headless.status.code(), Some(0), "{report}");
}

#[test]
fn the_report_holds_no_secret_and_no_project_path() {
    let bin = fakes(true);
    let config = TempDir::new("doctor-cfg");
    let token = concat!("opr", "_", "dddddddddddddddddddddddddddddddddddd");
    let text = format!(
        "[server]\nlisten = \"127.0.0.1:0\"\n[security]\nwebhook_secrets = [\"a-real-random-secret-value\"]\n\
         [ai_task_callback]\nenabled = true\napi_url = \"http://127.0.0.1:1\"\ntoken = \"{token}\"\n\
         [runner]\nallowed_roots = [\"{}\"]\nstate_dir = \"{}\"\n",
        config.path().join("projdir-xyz").display(),
        config.path().join("state").display(),
    );
    std::fs::create_dir_all(config.path().join("projdir-xyz")).expect("mkdir");
    let file = config.file("config.toml");
    std::fs::write(&file, text).expect("config");
    for show in [false, true] {
        let file_text = file.to_string_lossy().into_owned();
        let mut args = vec!["doctor", "--json", "--config", &file_text, "--project", "SVWL"];
        if show {
            args.push("--show-paths");
        }
        let output = run(bin.path(), &args);
        let all = format!(
            "{}{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        for secret in ["opr_", "Bearer", "a-real-random-secret-value"] {
            assert!(!all.contains(secret), "{secret} in the report: {all}");
        }
        let report = json(&output);
        let project = row(&report, "project.SVWL");
        assert!(
            project["detail"].get("directory_path").is_none(),
            "no record, no path: {project}"
        );
        if !show {
            assert!(
                !all.contains("projdir-xyz"),
                "a project directory is not printed: {all}"
            );
        }
    }
}

#[test]
fn check_is_doctor_json_without_a_config_read_and_a_bad_option_is_a_checker_error() {
    let bin = fakes(true);
    let output = run(bin.path(), &["--check"]);
    let report = json(&output);
    assert_eq!(report["schema"], "sylvode.doctor.v1");
    assert_eq!(report["generated_by"]["version"], env!("CARGO_PKG_VERSION"));
    assert!(
        report["components"]
            .as_array()
            .is_some_and(|rows| !rows.iter().any(|r| r["component"] == "api_protocol"))
    );
    let bad = run(bin.path(), &["doctor", "--mode", "bogus"]);
    assert_eq!(bad.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&bad.stderr).contains("unknown mode"));
}

#[test]
fn config_check_names_the_key_and_never_echoes_a_value() {
    let dir = TempDir::new("doctor-cfg");
    let secret = "opr_SECRETVALUE0123456789";
    let cases = [
        ("[server]\nlisten = 12345\n[security]\nwebhook_secrets = []\n", "listen"),
        (
            &format!("[server]\nlisten = \"127.0.0.1:1\"\n[security]\nwebhook_secrets = \"{secret}\"\n"),
            "webhook_secrets",
        ),
        (
            &format!(
                "[server]\nlisten = \"127.0.0.1:1\"\n[security]\nwebhook_secrets = [\"x\"]\nmystery_key = \"{secret}\"\n"
            ),
            "mystery_key",
        ),
    ];
    for (text, key) in cases {
        let file = dir.file("c.toml");
        std::fs::write(&file, text).expect("write");
        let output = run(
            dir.path(),
            &["config", "check", "--json", "--config", &file.to_string_lossy()],
        );
        let all = String::from_utf8_lossy(&output.stdout).into_owned();
        let body: Value = serde_json::from_str(&all).expect("json");
        assert_eq!(body["ok"], false, "{all}");
        assert!(all.contains(key), "{key} not named: {all}");
        assert!(
            !all.contains("SECRETVALUE") && !all.contains("12345"),
            "a value was echoed: {all}"
        );
        assert_eq!(output.status.code(), Some(2));
    }
    let good = dir.file("ok.toml");
    std::fs::write(
        &good,
        "[server]\nlisten = \"127.0.0.1:1\"\n[security]\nwebhook_secrets = [\"a-real-random-secret-value\"]\n",
    )
    .expect("write");
    let output = run(
        dir.path(),
        &["config", "check", "--json", "--config", &good.to_string_lossy()],
    );
    assert_eq!(json(&output)["ok"], true);
    assert_eq!(output.status.code(), Some(0));
}
