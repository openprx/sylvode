//! `scripts/install.sh`, run against a temporary `HOME` with fake `uname`, `systemctl` and
//! `launchctl` that record their calls, on a `PATH` that holds only those fakes and links to the
//! few system utilities the script needs. The real service manager is never reached, no tmux
//! (real or fake) is ever started, and a fake CLI records that nothing was registered. Both
//! platforms are exercised on either host, because the platform comes from the fake `uname`.
//!
//! The design's matrix row 12 (`install.sh --dry-run`, no runner, no tmux: exit 0, information
//! rows) is the first test.
#![cfg(unix)]
// Test helpers outside `#[test]` functions may fail loudly; this is test code only.
#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

mod support;

use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::atomic::{AtomicU32, Ordering};

use support::{McpSettings, write_config};

const ROOT: &str = env!("CARGO_MANIFEST_DIR");
const BIN: &str = env!("CARGO_BIN_EXE_mcp-server");
const WORKSPACE: &str = "11111111-1111-4111-8111-111111111111";

/// System utilities the script uses; linked into the sandbox `PATH`.
const UTILITIES: [&str; 8] = ["dirname", "ls", "cut", "mkdir", "chmod", "sed", "grep", "cat"];

struct Sandbox {
    root: PathBuf,
}

impl Sandbox {
    fn new(uname: &str) -> Self {
        static COUNTER: AtomicU32 = AtomicU32::new(0);
        let root = std::env::temp_dir().join(format!(
            "sylvode-mcp-install-{}-{}",
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        for dir in ["home", "bin", "pkg/scripts", "work"] {
            std::fs::create_dir_all(root.join(dir)).unwrap();
        }
        let sandbox = Self { root };
        // The script runs from a copy, so the workspace `target/release` of this machine can never
        // be found by its binary search.
        std::fs::copy(
            Path::new(ROOT).join("scripts/install.sh"),
            sandbox.root.join("pkg/scripts/install.sh"),
        )
        .unwrap();
        for name in UTILITIES {
            let source = find_on_path(name);
            std::os::unix::fs::symlink(source, sandbox.root.join("bin").join(name)).unwrap();
        }
        sandbox.stub("uname", &format!("echo {uname}"));
        let log = sandbox.calls_log();
        for name in ["systemctl", "launchctl"] {
            sandbox.stub(name, &format!("echo \"{name} $*\" >> '{}'\nexit 1", log.display()));
        }
        sandbox
    }

    fn calls_log(&self) -> PathBuf {
        self.root.join("calls.log")
    }

    fn calls(&self) -> String {
        std::fs::read_to_string(self.calls_log()).unwrap_or_default()
    }

    fn stub(&self, name: &str, body: &str) {
        let path = self.root.join("bin").join(name);
        std::fs::write(&path, format!("#!/bin/sh\n{body}\n")).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
    }

    fn config_dir(&self) -> PathBuf {
        self.root.join("work/odd dir")
    }

    fn write_config(&self, mode: u32) -> PathBuf {
        std::fs::create_dir_all(self.config_dir()).unwrap();
        let source = write_config(&McpSettings {
            api_url: "http://localhost:1",
            bot_token: None,
            workspace_id: Some(WORKSPACE),
            transport: Some("http"),
            bind_addr: None,
        })
        .unwrap();
        let target = self.config_dir().join("config.toml");
        std::fs::copy(source.path(), &target).unwrap();
        std::fs::set_permissions(&target, std::fs::Permissions::from_mode(mode)).unwrap();
        target
    }

    fn run_with(&self, binary: &str, extra: &[&str]) -> Output {
        let config_dir = self.config_dir();
        let mut args = vec!["--binary-path", binary, "--config-dir", config_dir.to_str().unwrap()];
        args.extend_from_slice(extra);
        Command::new(find_on_path("bash"))
            .arg(self.root.join("pkg/scripts/install.sh"))
            .args(args)
            .current_dir(self.root.join("work"))
            .env_clear()
            .env("HOME", self.root.join("home"))
            .env("PATH", self.root.join("bin"))
            .output()
            .unwrap()
    }

    fn run(&self, extra: &[&str]) -> Output {
        self.run_with(BIN, extra)
    }
}

impl Drop for Sandbox {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

fn find_on_path(name: &str) -> PathBuf {
    let path = std::env::var_os("PATH").unwrap();
    std::env::split_paths(&path)
        .map(|dir| dir.join(name))
        .find(|candidate| candidate.is_file())
        .unwrap_or_else(|| panic!("{name} is not on PATH"))
}

fn text(output: &Output) -> String {
    format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
}

#[test]
fn install_dry_run_without_runner_or_tmux_writes_nothing_and_exits_zero() {
    // Matrix row 12, on both platforms.
    for uname in ["Linux", "Darwin"] {
        let sandbox = Sandbox::new(uname);
        let output = sandbox.run(&["--dry-run"]);
        let all = text(&output);
        assert_eq!(output.status.code(), Some(0), "{uname}: {all}");
        assert!(
            !sandbox.config_dir().exists(),
            "the dry run created the config directory"
        );
        assert!(all.contains("Would create the config directory"), "{all}");
        assert!(all.contains("Dry run: nothing will be written"), "{all}");
        // The information rows of doctor: no tmux, no runner.
        assert!(all.contains("task delivery needs sylvode-runner"), "{all}");
        assert!(all.contains("tmux"), "{all}");
        // The script and doctor never reached a service manager or a tmux server.
        assert_eq!(sandbox.calls(), "", "{uname}");
        // Nothing but the sandbox that the test made.
        assert_eq!(std::fs::read_dir(sandbox.root.join("home")).unwrap().count(), 0);
    }
}

#[test]
fn install_prints_the_exact_registration_commands_and_registers_nothing() {
    let sandbox = Sandbox::new("Linux");
    sandbox.write_config(0o600);
    let registered = sandbox.root.join("claude.calls");
    sandbox.stub(
        "claude",
        &format!(
            "echo \"$*\" >> '{}'\ncase \"$1\" in --version) echo '2.1.328 (Claude Code)';; mcp) exit 1;; esac",
            registered.display()
        ),
    );
    let output = sandbox.run(&[]);
    let all = text(&output);
    assert_eq!(output.status.code(), Some(0), "{all}");
    let config = sandbox.config_dir().join("config.toml");
    let expected = format!(
        "claude mcp add --scope user sylvode -- {BIN} serve --transport stdio --config '{}'",
        config.display()
    );
    assert!(all.contains(&expected), "missing {expected}\n{all}");
    assert!(
        all.contains(&format!(
            "codex mcp add sylvode -- {BIN} serve --transport stdio --config '{}'",
            config.display()
        )),
        "{all}"
    );
    // The CLI was only asked its version and for the registration; nothing was added.
    let calls = std::fs::read_to_string(&registered).unwrap();
    assert!(!calls.contains("mcp add"), "{calls}");
    assert!(calls.contains("mcp get sylvode"), "{calls}");
    assert_eq!(sandbox.calls(), "");
}

#[test]
fn install_creates_only_the_config_directory_and_never_rewrites_a_config() {
    let sandbox = Sandbox::new("Darwin");
    let output = sandbox.run(&[]);
    assert_eq!(output.status.code(), Some(0), "{}", text(&output));
    let dir = sandbox.config_dir();
    assert_eq!(std::fs::metadata(&dir).unwrap().permissions().mode() & 0o777, 0o700);
    assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 0, "no config file is created");
    assert!(text(&output).contains("No configuration at"), "{}", text(&output));

    let config = sandbox.write_config(0o600);
    let before = std::fs::read(&config).unwrap();
    let modified = std::fs::metadata(&config).unwrap().modified().unwrap();
    let output = sandbox.run(&[]);
    assert_eq!(output.status.code(), Some(0), "{}", text(&output));
    assert_eq!(std::fs::read(&config).unwrap(), before);
    assert_eq!(std::fs::metadata(&config).unwrap().modified().unwrap(), modified);
}

#[test]
fn install_warns_about_a_config_that_others_can_read() {
    let sandbox = Sandbox::new("Linux");
    sandbox.write_config(0o644);
    let output = sandbox.run(&["--dry-run"]);
    let all = text(&output);
    assert_eq!(output.status.code(), Some(0), "{all}");
    assert!(all.contains("others can read it"), "{all}");
    assert!(all.contains("chmod 600"), "{all}");

    let private = Sandbox::new("Linux");
    private.write_config(0o600);
    let all = text(&private.run(&["--dry-run"]));
    assert!(all.contains("(mode ok)"), "{all}");
}

#[test]
fn install_refuses_a_missing_or_foreign_binary_and_an_unknown_system() {
    let sandbox = Sandbox::new("Linux");
    let missing = sandbox.root.join("work/absent");
    let output = sandbox.run_with(missing.to_str().unwrap(), &["--dry-run"]);
    assert_ne!(output.status.code(), Some(0));
    assert!(text(&output).contains("Binary not found"), "{}", text(&output));

    sandbox.stub("not-the-server", "exit 3");
    let foreign = sandbox.root.join("bin/not-the-server");
    let output = sandbox.run_with(foreign.to_str().unwrap(), &["--dry-run"]);
    assert_ne!(output.status.code(), Some(0));
    assert!(text(&output).contains("does not answer --check"), "{}", text(&output));

    let other = Sandbox::new("Plan9");
    let output = other.run(&["--dry-run"]);
    assert_ne!(output.status.code(), Some(0));
    assert!(text(&output).contains("Unsupported OS"), "{}", text(&output));
}

#[test]
fn install_exits_non_zero_when_doctor_finds_a_failure() {
    let sandbox = Sandbox::new("Linux");
    std::fs::create_dir_all(sandbox.config_dir()).unwrap();
    std::fs::write(sandbox.config_dir().join("config.toml"), "not = [valid\n").unwrap();
    std::fs::set_permissions(
        sandbox.config_dir().join("config.toml"),
        std::fs::Permissions::from_mode(0o600),
    )
    .unwrap();
    let output = sandbox.run(&["--dry-run"]);
    assert_ne!(output.status.code(), Some(0), "{}", text(&output));
    assert!(
        text(&output).contains("doctor reported at least one failure"),
        "{}",
        text(&output)
    );
}

#[test]
fn install_script_help_lists_its_options() {
    let sandbox = Sandbox::new("Linux");
    let output = Command::new(find_on_path("bash"))
        .arg(sandbox.root.join("pkg/scripts/install.sh"))
        .arg("--help")
        .env_clear()
        .env("PATH", sandbox.root.join("bin"))
        .output()
        .unwrap();
    let all = text(&output);
    assert_eq!(output.status.code(), Some(0), "{all}");
    for option in ["--binary-path", "--config-dir", "--dry-run"] {
        assert!(all.contains(option), "{option} missing: {all}");
    }
}
