//! `scripts/install.sh` and `scripts/uninstall.sh`, run against a temporary `HOME` with fake
//! `uname`, `systemctl` and `launchctl` that record their calls: a fresh install, an in-place
//! update of a unit installed under a former name, `--migrate`, the refusal when two names are
//! installed, the order in which the binary is looked for, and an uninstall that removes all
//! three names. Both platforms are exercised on either host, because the platform comes from the
//! fake `uname`.

#![cfg(unix)]
// Test helpers outside `#[test]` functions may fail loudly; this is test code only.
#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

const ROOT: &str = env!("CARGO_MANIFEST_DIR");

const CANONICAL: &str = "sylvode-runner";
const WEBHOOK: &str = "sylvode-webhook";
const LEGACY: &str = "openpr-webhook";
const CANONICAL_LABEL: &str = "dev.sylvode.runner";
const WEBHOOK_LABEL: &str = "dev.sylvode.webhook";
const LEGACY_LABEL: &str = "dev.openpr.webhook";

#[derive(Clone, Copy)]
enum Platform {
    Linux,
    MacOs,
}

struct Sandbox {
    root: PathBuf,
    platform: Platform,
}

impl Sandbox {
    /// A temporary HOME, a copy of the scripts and templates, a binary and a config directory
    /// whose paths contain a space, `&`, `%` and `$`, and fake `uname`, `systemctl` and
    /// `launchctl`.
    fn new(platform: Platform) -> Self {
        let root = std::env::temp_dir().join(format!("sylvode-runner-install-{}", uuid::Uuid::new_v4()));
        let sandbox = Self { root, platform };
        for dir in ["home", "bin", "pkg/scripts", "odd dir & 100% $HOME"] {
            std::fs::create_dir_all(sandbox.root.join(dir)).unwrap();
        }
        // The scripts run from a copy, so the workspace `target/release` of the machine that runs
        // the tests can never be found by the binary search.
        for entry in std::fs::read_dir(Path::new(ROOT).join("scripts")).unwrap() {
            let entry = entry.unwrap();
            std::fs::copy(entry.path(), sandbox.root.join("pkg/scripts").join(entry.file_name())).unwrap();
        }
        sandbox.add_binary(CANONICAL);
        std::fs::write(sandbox.config_dir().join("config.toml"), "").unwrap();
        let uname = match platform {
            Platform::Linux => "Linux",
            Platform::MacOs => "Darwin",
        };
        sandbox.stub("uname", &format!("echo {uname}\n"));
        let log = sandbox.root.join("calls.log");
        sandbox.stub(
            "systemctl",
            &format!(
                "echo \"systemctl $*\" >> '{}'\ncase \"$2\" in is-active|is-enabled) exit 1 ;; esac\nexit 0\n",
                log.display()
            ),
        );
        sandbox.stub(
            "launchctl",
            &format!("echo \"launchctl $*\" >> '{}'\nexit 0\n", log.display()),
        );
        sandbox
    }

    fn stub(&self, name: &str, body: &str) {
        let path = self.root.join("bin").join(name);
        std::fs::write(&path, format!("#!/bin/sh\n{body}")).unwrap();
        make_executable(&path);
    }

    fn config_dir(&self) -> PathBuf {
        self.root.join("odd dir & 100% $HOME")
    }

    fn binary(&self) -> PathBuf {
        self.config_dir().join(CANONICAL)
    }

    /// An executable file called `name` in the config directory.
    fn add_binary(&self, name: &str) -> PathBuf {
        let path = self.config_dir().join(name);
        std::fs::write(&path, "#!/bin/sh\n").unwrap();
        make_executable(&path);
        path
    }

    fn unit(&self, name: &str) -> PathBuf {
        self.root.join(format!("home/.config/systemd/user/{name}.service"))
    }

    fn plist(&self, label: &str) -> PathBuf {
        self.root.join(format!("home/Library/LaunchAgents/{label}.plist"))
    }

    /// Writes a placeholder file for an installed unit or agent and returns its path.
    fn install_placeholder(&self, name: &str) -> PathBuf {
        let path = match self.platform {
            Platform::Linux => self.unit(name),
            Platform::MacOs => self.plist(name),
        };
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, "placeholder\n").unwrap();
        path
    }

    fn installed(&self, name: &str) -> PathBuf {
        match self.platform {
            Platform::Linux => self.unit(name),
            Platform::MacOs => self.plist(name),
        }
    }

    fn run_in(&self, dir: &Path, script: &str, args: &[&str]) -> Output {
        Command::new("bash")
            .arg(self.root.join("pkg/scripts").join(script))
            .args(args)
            .current_dir(dir)
            .env_clear()
            .env("HOME", self.root.join("home"))
            .env("PATH", format!("{}:/usr/bin:/bin", self.root.join("bin").display()))
            .output()
            .expect("run script")
    }

    fn run(&self, script: &str, args: &[&str]) -> Output {
        self.run_in(&self.root, script, args)
    }

    fn install_with(&self, extra: &[&str]) -> Output {
        let binary = self.binary();
        let config_dir = self.config_dir();
        let mut args = vec![
            "--binary-path",
            binary.to_str().unwrap(),
            "--config-dir",
            config_dir.to_str().unwrap(),
        ];
        args.extend_from_slice(extra);
        self.run("install.sh", &args)
    }

    fn install(&self) -> Output {
        self.install_with(&[])
    }

    /// Installs with no `--binary-path` from the config directory, which holds the binaries.
    fn install_autodetect(&self) -> Output {
        self.run_in(&self.config_dir(), "install.sh", &[])
    }

    fn calls(&self) -> Vec<String> {
        std::fs::read_to_string(self.root.join("calls.log"))
            .unwrap_or_default()
            .lines()
            .map(str::to_owned)
            .collect()
    }

    /// The calls that change something: `launchctl list` only reads.
    fn changes(&self) -> Vec<String> {
        self.calls()
            .into_iter()
            .filter(|call| call != "launchctl list")
            .collect()
    }

    fn clear_calls(&self) {
        let _ = std::fs::remove_file(self.root.join("calls.log"));
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

/// The lines of `calls` that name `needle`, in order.
fn calls_with(calls: &[String], needle: &str) -> Vec<String> {
    calls.iter().filter(|call| call.contains(needle)).cloned().collect()
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
    let sandbox = Sandbox::new(Platform::Linux);
    let output = sandbox.install();
    assert!(output.status.success(), "{}", stderr(&output));

    let unit = std::fs::read_to_string(sandbox.unit(CANONICAL)).expect("canonical unit written");
    assert!(!sandbox.unit(WEBHOOK).exists());
    assert!(!sandbox.unit(LEGACY).exists());
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
    assert!(unit.contains("journalctl --user -u sylvode-runner"), "{unit}");
    assert!(unit.contains("Description=Sylvode Runner Service"), "{unit}");
    assert!(!unit.contains("{{"), "unrendered placeholder: {unit}");
    assert!(sandbox.calls().contains(&"systemctl --user daemon-reload".to_string()));
    assert!(!stderr(&output).contains("deprecated"), "{}", stderr(&output));
}

#[test]
fn a_unit_installed_under_a_former_name_is_updated_in_place() {
    for (name, expected_notice) in [
        (
            WEBHOOK,
            "sylvode-webhook.service is deprecated, use sylvode-runner.service instead",
        ),
        (
            LEGACY,
            "openpr-webhook.service is deprecated, use sylvode-runner.service instead",
        ),
    ] {
        let sandbox = Sandbox::new(Platform::Linux);
        let old = sandbox.install_placeholder(name);
        let output = sandbox.install();
        assert!(output.status.success(), "{name}: {}", stderr(&output));
        let unit = std::fs::read_to_string(&old).unwrap();
        assert!(!unit.contains("placeholder"), "{name}: the unit was rewritten: {unit}");
        assert!(unit.contains(&format!("journalctl --user -u {name}")), "{name}: {unit}");
        assert!(!sandbox.unit(CANONICAL).exists(), "{name}: no second unit");
        let err = stderr(&output);
        assert!(err.contains(expected_notice), "{name}: {err}");
        assert!(
            err.contains("--migrate"),
            "{name}: the notice must name --migrate: {err}"
        );
        // An update in place neither stops nor enables anything.
        assert_eq!(sandbox.calls(), ["systemctl --user daemon-reload"], "{name}");
    }
}

#[test]
fn migrate_replaces_a_former_unit_with_the_canonical_one() {
    for name in [WEBHOOK, LEGACY] {
        let sandbox = Sandbox::new(Platform::Linux);
        let old = sandbox.install_placeholder(name);
        let output = sandbox.install_with(&["--migrate"]);
        assert!(output.status.success(), "{name}: {}", stderr(&output));
        assert!(!old.exists(), "{name}: the former unit file is removed");
        let unit = std::fs::read_to_string(sandbox.unit(CANONICAL)).expect("canonical unit written");
        let binary = sandbox.binary();
        assert!(
            unit.contains(&format!("ExecStart=\"{}\"", quoted(binary.to_str().unwrap()))),
            "{name}: {unit}"
        );
        assert!(unit.contains("journalctl --user -u sylvode-runner"), "{name}: {unit}");
        assert_eq!(
            sandbox.calls(),
            [
                format!("systemctl --user disable --now {name}"),
                "systemctl --user daemon-reload".to_string(),
                "systemctl --user enable --now sylvode-runner".to_string(),
                "systemctl --user daemon-reload".to_string(),
            ],
            "{name}"
        );

        // Running it again changes nothing that matters: no former unit, so nothing to stop.
        let canonical_before = std::fs::read_to_string(sandbox.unit(CANONICAL)).unwrap();
        sandbox.clear_calls();
        let again = sandbox.install_with(&["--migrate"]);
        assert!(again.status.success(), "{name}: {}", stderr(&again));
        assert_eq!(
            std::fs::read_to_string(sandbox.unit(CANONICAL)).unwrap(),
            canonical_before,
            "{name}"
        );
        assert!(
            !sandbox.unit(WEBHOOK).exists() && !sandbox.unit(LEGACY).exists(),
            "{name}"
        );
        assert!(
            calls_with(&sandbox.calls(), "disable").is_empty(),
            "{:?}",
            sandbox.calls()
        );
    }
}

#[test]
fn two_or_more_names_installed_is_refused() {
    for names in [
        vec![LEGACY, CANONICAL],
        vec![WEBHOOK, CANONICAL],
        vec![LEGACY, WEBHOOK],
        vec![LEGACY, WEBHOOK, CANONICAL],
    ] {
        for extra in [&[][..], &["--migrate"][..]] {
            let sandbox = Sandbox::new(Platform::Linux);
            for name in &names {
                sandbox.install_placeholder(name);
            }
            let output = sandbox.install_with(extra);
            assert!(!output.status.success(), "{names:?} {extra:?}");
            assert!(
                stderr(&output).contains("would bind the same port"),
                "{names:?}: {}",
                stderr(&output)
            );
            for name in &names {
                assert_eq!(
                    std::fs::read_to_string(sandbox.unit(name)).unwrap(),
                    "placeholder\n",
                    "{names:?}: {name} must not be touched"
                );
            }
            assert!(sandbox.calls().is_empty(), "{:?}", sandbox.calls());
        }
    }
}

#[test]
fn uninstall_removes_all_three_units() {
    let sandbox = Sandbox::new(Platform::Linux);
    for name in [LEGACY, WEBHOOK, CANONICAL] {
        sandbox.install_placeholder(name);
    }
    let output = sandbox.run("uninstall.sh", &[]);
    assert!(output.status.success(), "{}", stderr(&output));
    for name in [LEGACY, WEBHOOK, CANONICAL] {
        assert!(!sandbox.unit(name).exists(), "{name}");
        let calls = sandbox.calls();
        assert!(
            calls.contains(&format!("systemctl --user is-active --quiet {name}")),
            "{calls:?}"
        );
    }
}

#[test]
fn the_binary_is_looked_for_under_the_runner_name_first() {
    // Only the old executable names are present: the former name is used, with its notice.
    for (present, notice) in [
        (
            vec![WEBHOOK, LEGACY],
            Some("executable name sylvode-webhook is deprecated"),
        ),
        (vec![LEGACY], Some("executable name openpr-webhook is deprecated")),
        (vec![CANONICAL, WEBHOOK, LEGACY], None),
    ] {
        let sandbox = Sandbox::new(Platform::Linux);
        std::fs::remove_file(sandbox.binary()).unwrap();
        for name in &present {
            sandbox.add_binary(name);
        }
        let output = sandbox.install_autodetect();
        assert!(output.status.success(), "{present:?}: {}", stderr(&output));
        let unit = std::fs::read_to_string(sandbox.unit(CANONICAL)).unwrap();
        let first = present.first().copied().expect("at least one binary");
        // The shell reports its working directory with symbolic links resolved (/var is /private/var on macOS).
        let expected = std::fs::canonicalize(sandbox.config_dir()).unwrap().join(first);
        assert!(
            unit.contains(&format!("ExecStart=\"{}\"", quoted(expected.to_str().unwrap()))),
            "{present:?}: {unit}"
        );
        let err = stderr(&output);
        match notice {
            Some(notice) => assert!(err.contains(notice), "{present:?}: {err}"),
            None => assert!(!err.contains("executable name"), "{present:?}: {err}"),
        }
    }
}

#[test]
fn a_fresh_macos_install_writes_the_canonical_agent() {
    let sandbox = Sandbox::new(Platform::MacOs);
    let output = sandbox.install();
    assert!(output.status.success(), "{}", stderr(&output));
    let plist = std::fs::read_to_string(sandbox.plist(CANONICAL_LABEL)).expect("canonical agent written");
    assert!(plist.contains("<string>dev.sylvode.runner</string>"), "{plist}");
    assert!(plist.contains("sylvode-runner.log"), "{plist}");
    assert!(!plist.contains("{{"), "unrendered placeholder: {plist}");
    assert!(!sandbox.plist(WEBHOOK_LABEL).exists() && !sandbox.plist(LEGACY_LABEL).exists());
    assert_eq!(
        sandbox.changes(),
        [format!("launchctl load {}", sandbox.plist(CANONICAL_LABEL).display())]
    );
}

#[test]
fn a_macos_agent_installed_under_a_former_name_is_updated_in_place() {
    for label in [WEBHOOK_LABEL, LEGACY_LABEL] {
        let sandbox = Sandbox::new(Platform::MacOs);
        let old = sandbox.install_placeholder(label);
        let output = sandbox.install();
        assert!(output.status.success(), "{label}: {}", stderr(&output));
        let plist = std::fs::read_to_string(&old).unwrap();
        assert!(plist.contains(&format!("<string>{label}</string>")), "{label}: {plist}");
        assert!(!sandbox.plist(CANONICAL_LABEL).exists(), "{label}: no second agent");
        let err = stderr(&output);
        assert!(
            err.contains(&format!("{label} is deprecated, use dev.sylvode.runner instead")),
            "{label}: {err}"
        );
        assert!(err.contains("--migrate"), "{label}: {err}");
        assert_eq!(
            sandbox.changes(),
            [format!("launchctl load {}", old.display())],
            "{label}"
        );
    }
}

#[test]
fn macos_migrate_replaces_a_former_agent_with_the_canonical_one() {
    for label in [WEBHOOK_LABEL, LEGACY_LABEL] {
        let sandbox = Sandbox::new(Platform::MacOs);
        let old = sandbox.install_placeholder(label);
        let output = sandbox.install_with(&["--migrate"]);
        assert!(output.status.success(), "{label}: {}", stderr(&output));
        assert!(!old.exists(), "{label}: the former agent file is removed");
        assert!(sandbox.plist(CANONICAL_LABEL).exists(), "{label}");
        assert_eq!(
            sandbox.changes(),
            [
                format!("launchctl unload {}", old.display()),
                format!("launchctl load {}", sandbox.plist(CANONICAL_LABEL).display()),
            ],
            "{label}"
        );

        sandbox.clear_calls();
        let again = sandbox.install_with(&["--migrate"]);
        assert!(again.status.success(), "{label}: {}", stderr(&again));
        assert!(!sandbox.plist(WEBHOOK_LABEL).exists() && !sandbox.plist(LEGACY_LABEL).exists());
        assert!(
            calls_with(&sandbox.changes(), "unload").is_empty(),
            "{:?}",
            sandbox.changes()
        );
    }
}

#[test]
fn macos_two_agents_installed_is_refused_and_uninstall_removes_all_three() {
    let sandbox = Sandbox::new(Platform::MacOs);
    sandbox.install_placeholder(WEBHOOK_LABEL);
    sandbox.install_placeholder(CANONICAL_LABEL);
    let output = sandbox.install();
    assert!(!output.status.success());
    assert!(
        stderr(&output).contains("would bind the same port"),
        "{}",
        stderr(&output)
    );

    sandbox.install_placeholder(LEGACY_LABEL);
    let output = sandbox.run("uninstall.sh", &[]);
    assert!(output.status.success(), "{}", stderr(&output));
    for label in [CANONICAL_LABEL, WEBHOOK_LABEL, LEGACY_LABEL] {
        assert!(!sandbox.installed(label).exists(), "{label}");
    }
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
    let mut plist = std::fs::read_to_string(Path::new(ROOT).join("scripts/dev.sylvode.runner.plist.template")).unwrap();
    for key in ["BINARY_PATH", "CONFIG_DIR", "PATH", "LOG_DIR", "LABEL", "LOG_NAME"] {
        plist = plist.replace(&format!("{{{{{key}}}}}"), &xml);
    }
    let mut unit = std::fs::read_to_string(Path::new(ROOT).join("scripts/sylvode-runner.service.template")).unwrap();
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
render_template "@ROOT@/dev.sylvode.runner.plist.template" "@ROOT@/out.plist" xml_escape \
    "BINARY_PATH=$v" "CONFIG_DIR=$v" "PATH=$v" "LOG_DIR=$v" "LABEL=$v" "LOG_NAME=$v"
render_template "@ROOT@/sylvode-runner.service.template" "@ROOT@/out.service" systemd_quoted_escape \
    "BINARY_PATH=$v" "PATH=$v" "UNIT_NAME=$v"
render_template "@ROOT@/out.service" "@ROOT@/out.service" systemd_arg_escape "CONFIG_DIR=$v"
render_template "@ROOT@/out.service" "@ROOT@/out.service" systemd_path_escape "WORKING_DIRECTORY=$v"
"#;

/// A directory holding both templates and the renderer, for one bash to render into.
fn render_dir(label: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("sylvode-runner-render-{label}-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&dir).unwrap();
    for template in ["dev.sylvode.runner.plist.template", "sylvode-runner.service.template"] {
        std::fs::copy(Path::new(ROOT).join("scripts").join(template), dir.join(template)).unwrap();
    }
    dir
}

/// Compares what one bash rendered in `dir` with the expected output. `dir` is removed first, so
/// a failing comparison leaves nothing behind.
fn assert_rendered(dir: &Path, output: &Output, bash: &str) {
    let plist = std::fs::read_to_string(dir.join("out.plist")).unwrap_or_default();
    let unit = std::fs::read_to_string(dir.join("out.service")).unwrap_or_default();
    let _ = std::fs::remove_dir_all(dir);
    assert!(output.status.success(), "{bash}: {}", stderr(output));
    let (expected_plist, expected_unit) = expected_renderings();
    assert_eq!(plist, expected_plist, "{bash}: plist");
    assert_eq!(unit, expected_unit, "{bash}: unit");
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
// A check that is skipped must say so in the test output.
#[allow(clippy::print_stderr)]
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
