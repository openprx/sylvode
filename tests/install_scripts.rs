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

/// The value every placeholder of the rendering tests is set to: a space, `&`, `%`, `$`, a
/// backslash, a double quote and `<`, each of which some format treats specially.
const ODD: &str = r#"/Users/me/odd dir & 100% $HOME \ "q" <x>"#;

/// `render_template` and the escape functions, cut out of `scripts/install.sh`, followed by
/// `driver`, written to a file in `dir` that a bash at `/w` (the directory mounted in the
/// container, or `dir` itself) can source.
fn renderer_script(dir: &Path, root: &str, driver: &str) -> PathBuf {
    let script = std::fs::read_to_string(Path::new(ROOT).join("scripts/install.sh")).unwrap();
    let start = script.find("shopt -u patsub_replacement").expect("renderer start");
    let end = script.find("# Platform: Linux").expect("renderer end");
    let path = dir.join("render.sh");
    std::fs::write(
        &path,
        format!("{}\n{}", &script[start..end], driver.replace("@ROOT@", root)),
    )
    .unwrap();
    path
}

/// What the installer writes for `ODD` in every placeholder, rendered here independently of
/// bash: the plist with XML escaping, the unit with systemd quoting.
fn expected_renderings() -> (String, String) {
    let xml = ODD.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;");
    let mut plist =
        std::fs::read_to_string(Path::new(ROOT).join("scripts/dev.sylvode.webhook.plist.template")).unwrap();
    for key in ["BINARY_PATH", "CONFIG_DIR", "PATH", "LOG_DIR", "LABEL", "LOG_NAME"] {
        plist = plist.replace(&format!("{{{{{key}}}}}"), &xml);
    }
    let mut unit = std::fs::read_to_string(Path::new(ROOT).join("scripts/sylvode-webhook.service.template")).unwrap();
    for key in ["BINARY_PATH", "PATH", "UNIT_NAME"] {
        unit = unit.replace(&format!("{{{{{key}}}}}"), &quoted(ODD));
    }
    unit = unit.replace("{{CONFIG_DIR}}", &quoted_arg(ODD));
    unit = unit.replace("{{WORKING_DIRECTORY}}", &ODD.replace('%', "%%"));
    (
        format!("{}\n", plist.trim_end_matches('\n')),
        format!("{}\n", unit.trim_end_matches('\n')),
    )
}

/// Renders both templates the way `install_linux` and `install_macos` call `render_template`.
const RENDER_DRIVER: &str = r#"
v='/Users/me/odd dir & 100% $HOME \ "q" <x>'
render_template "@ROOT@/dev.sylvode.webhook.plist.template" "@ROOT@/out.plist" xml_escape \
    "BINARY_PATH=$v" "CONFIG_DIR=$v" "PATH=$v" "LOG_DIR=$v" "LABEL=$v" "LOG_NAME=$v"
render_template "@ROOT@/sylvode-webhook.service.template" "@ROOT@/out.service" systemd_quoted_escape \
    "BINARY_PATH=$v" "PATH=$v" "UNIT_NAME=$v"
render_template "@ROOT@/out.service" "@ROOT@/out.service" systemd_arg_escape "CONFIG_DIR=$v"
render_template "@ROOT@/out.service" "@ROOT@/out.service" systemd_path_escape "WORKING_DIRECTORY=$v"
"#;

/// A directory holding both templates and the renderer, for one bash to render into.
fn render_dir(label: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("sylvode-webhook-render-{label}-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&dir).unwrap();
    for template in ["dev.sylvode.webhook.plist.template", "sylvode-webhook.service.template"] {
        std::fs::copy(Path::new(ROOT).join("scripts").join(template), dir.join(template)).unwrap();
    }
    dir
}

fn assert_rendered(dir: &Path, output: &Output, bash: &str) {
    assert!(output.status.success(), "{bash}: {}", stderr(output));
    let (plist, unit) = expected_renderings();
    assert_eq!(
        std::fs::read_to_string(dir.join("out.plist")).unwrap(),
        plist,
        "{bash}: plist"
    );
    assert_eq!(
        std::fs::read_to_string(dir.join("out.service")).unwrap(),
        unit,
        "{bash}: unit"
    );
    let _ = std::fs::remove_dir_all(dir);
}

/// The renderer inserts every value byte for byte under the system bash, and under the same bash
/// emulating bash 3.2 (`BASH_COMPAT=3.2`): before bash 4.3 a quoted replacement in
/// `${var//pattern/"$value"}` kept its quotes, which put literal `"` around every value.
#[test]
fn the_renderer_inserts_values_literally_under_the_system_bash_and_in_bash_3_2_mode() {
    for compat in [None, Some("3.2"), Some("4.2")] {
        let dir = render_dir("system");
        let script = renderer_script(&dir, dir.to_str().unwrap(), RENDER_DRIVER);
        let mut command = Command::new("bash");
        command.arg(&script).env_clear().env("PATH", "/usr/bin:/bin");
        if let Some(compat) = compat {
            command.env("BASH_COMPAT", compat);
        }
        let output = command.output().expect("run bash");
        assert_rendered(&dir, &output, &format!("bash BASH_COMPAT={compat:?}"));
    }
}

/// The same under a real bash 3.2, the version macOS ships, when the `docker.io/library/bash:3.2`
/// image is present for podman (it is not pulled). Without it the test says so and checks nothing.
#[test]
fn the_renderer_inserts_values_literally_under_a_real_bash_3_2_when_available() {
    const IMAGE: &str = "docker.io/library/bash:3.2";
    let available = Command::new("podman")
        .args(["image", "exists", IMAGE])
        .output()
        .is_ok_and(|output| output.status.success());
    if !available {
        eprintln!("SKIPPED: podman image {IMAGE} not present; bash 3.2 rendering not checked here");
        return;
    }
    let dir = render_dir("bash32");
    let script = renderer_script(&dir, "/w", RENDER_DRIVER);
    assert!(script.ends_with("render.sh"));
    let output = Command::new("podman")
        .args(["run", "--rm", "--network=none", "-v"])
        .arg(format!("{}:/w:Z", dir.display()))
        .args([IMAGE, "bash", "-c", "echo \"$BASH_VERSION\" >&2; . /w/render.sh"])
        .output()
        .expect("run podman");
    assert!(
        String::from_utf8_lossy(&output.stderr).starts_with("3.2."),
        "{}",
        stderr(&output)
    );
    assert_rendered(&dir, &output, "bash 3.2 (podman)");
}
