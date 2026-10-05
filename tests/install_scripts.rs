//! `scripts/install.sh` and `scripts/uninstall.sh` on Linux, run against a temporary `HOME` with
//! a fake `systemctl` that records its calls: a fresh install, an in-place update of the legacy
//! unit, the refusal when both units exist, and an uninstall that removes both.

#![cfg(target_os = "linux")]
// Test helpers outside `#[test]` functions may fail loudly; this is test code only.
#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

const ROOT: &str = env!("CARGO_MANIFEST_DIR");

struct Sandbox {
    root: PathBuf,
}

impl Sandbox {
    /// A temporary HOME, a binary and a config directory whose paths contain a space, `&`, `%`
    /// and `$`, and a fake `systemctl`.
    fn new() -> Self {
        let root = std::env::temp_dir().join(format!("sylvode-webhook-install-{}", uuid::Uuid::new_v4()));
        let sandbox = Self { root };
        for dir in ["home", "bin", "odd dir & 100% $HOME"] {
            std::fs::create_dir_all(sandbox.root.join(dir)).unwrap();
        }
        let binary = sandbox.binary();
        std::fs::write(&binary, "#!/bin/sh\n").unwrap();
        make_executable(&binary);
        std::fs::write(sandbox.config_dir().join("config.toml"), "").unwrap();
        let systemctl = sandbox.root.join("bin/systemctl");
        std::fs::write(
            &systemctl,
            format!(
                "#!/bin/sh\necho \"$*\" >> '{}'\ncase \"$2\" in is-active|is-enabled) exit 1 ;; esac\nexit 0\n",
                sandbox.root.join("systemctl.log").display()
            ),
        )
        .unwrap();
        make_executable(&systemctl);
        sandbox
    }

    fn config_dir(&self) -> PathBuf {
        self.root.join("odd dir & 100% $HOME")
    }

    fn binary(&self) -> PathBuf {
        self.config_dir().join("sylvode-webhook")
    }

    fn unit(&self, name: &str) -> PathBuf {
        self.root.join(format!("home/.config/systemd/user/{name}.service"))
    }

    fn run(&self, script: &str, args: &[&str]) -> Output {
        Command::new("bash")
            .arg(Path::new(ROOT).join("scripts").join(script))
            .args(args)
            .env_clear()
            .env("HOME", self.root.join("home"))
            .env("PATH", format!("{}:/usr/bin:/bin", self.root.join("bin").display()))
            .output()
            .expect("run script")
    }

    fn install(&self) -> Output {
        let binary = self.binary();
        let config_dir = self.config_dir();
        self.run(
            "install.sh",
            &[
                "--binary-path",
                binary.to_str().unwrap(),
                "--config-dir",
                config_dir.to_str().unwrap(),
            ],
        )
    }

    fn systemctl_calls(&self) -> String {
        std::fs::read_to_string(self.root.join("systemctl.log")).unwrap_or_default()
    }
}

impl Drop for Sandbox {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

fn make_executable(path: &Path) {
    let mut perms = std::fs::metadata(path).unwrap().permissions();
    std::os::unix::fs::PermissionsExt::set_mode(&mut perms, 0o755);
    std::fs::set_permissions(path, perms).unwrap();
}

fn stderr(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

/// systemd's quoting of `value` inside a double-quoted setting.
fn quoted(value: &str) -> String {
    value.replace('\\', "\\\\").replace('"', "\\\"").replace('%', "%%")
}

/// The same for an `ExecStart=` argument, where `$` is expanded unless doubled.
fn quoted_arg(value: &str) -> String {
    quoted(value).replace('$', "$$")
}

#[test]
fn a_fresh_install_writes_a_correctly_quoted_canonical_unit() {
    let sandbox = Sandbox::new();
    let output = sandbox.install();
    assert!(output.status.success(), "{}", stderr(&output));

    let unit = std::fs::read_to_string(sandbox.unit("sylvode-webhook")).expect("canonical unit written");
    assert!(!sandbox.unit("openpr-webhook").exists());
    let binary = sandbox.binary();
    let dir = sandbox.config_dir();
    assert!(
        unit.contains(&format!(
            "ExecStart=\"{}\" \"{}/config.toml\"",
            quoted(binary.to_str().unwrap()),
            quoted_arg(dir.to_str().unwrap())
        )),
        "{unit}"
    );
    assert!(
        unit.contains(&format!(
            "WorkingDirectory={}\n",
            dir.to_str().unwrap().replace('%', "%%")
        )),
        "{unit}"
    );
    assert!(unit.contains("journalctl --user -u sylvode-webhook"), "{unit}");
    assert!(!unit.contains("{{"), "unrendered placeholder: {unit}");
    assert!(sandbox.systemctl_calls().contains("--user daemon-reload"));
}

#[test]
fn an_existing_legacy_unit_is_updated_in_place() {
    let sandbox = Sandbox::new();
    let legacy = sandbox.unit("openpr-webhook");
    std::fs::create_dir_all(legacy.parent().unwrap()).unwrap();
    std::fs::write(&legacy, "[Service]\nExecStart=/old/openpr-webhook\n").unwrap();

    let output = sandbox.install();
    assert!(output.status.success(), "{}", stderr(&output));
    let unit = std::fs::read_to_string(&legacy).unwrap();
    assert!(
        !unit.contains("/old/openpr-webhook"),
        "the legacy unit was rewritten: {unit}"
    );
    assert!(unit.contains("journalctl --user -u openpr-webhook"), "{unit}");
    assert!(!sandbox.unit("sylvode-webhook").exists(), "no second unit");
    assert!(
        stderr(&output).contains("openpr-webhook.service is deprecated, use sylvode-webhook.service"),
        "{}",
        stderr(&output)
    );
}

#[test]
fn both_units_present_is_refused() {
    let sandbox = Sandbox::new();
    for name in ["openpr-webhook", "sylvode-webhook"] {
        let unit = sandbox.unit(name);
        std::fs::create_dir_all(unit.parent().unwrap()).unwrap();
        std::fs::write(&unit, "[Service]\n").unwrap();
    }
    let output = sandbox.install();
    assert!(!output.status.success());
    assert!(
        stderr(&output).contains("would bind the same port"),
        "{}",
        stderr(&output)
    );
    assert_eq!(
        std::fs::read_to_string(sandbox.unit("sylvode-webhook")).unwrap(),
        "[Service]\n"
    );
}

#[test]
fn uninstall_removes_the_canonical_and_the_legacy_unit() {
    let sandbox = Sandbox::new();
    for name in ["openpr-webhook", "sylvode-webhook"] {
        let unit = sandbox.unit(name);
        std::fs::create_dir_all(unit.parent().unwrap()).unwrap();
        std::fs::write(&unit, "[Service]\n").unwrap();
    }
    let output = sandbox.run("uninstall.sh", &[]);
    assert!(output.status.success(), "{}", stderr(&output));
    assert!(!sandbox.unit("sylvode-webhook").exists());
    assert!(!sandbox.unit("openpr-webhook").exists());
    let calls = sandbox.systemctl_calls();
    assert!(calls.contains("--user is-active --quiet sylvode-webhook"), "{calls}");
    assert!(calls.contains("--user is-active --quiet openpr-webhook"), "{calls}");
}
