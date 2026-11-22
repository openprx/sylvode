//! `scripts/install.sh` and `scripts/uninstall.sh`, run against a temporary `HOME` with fake
//! `uname`, `systemctl` and `launchctl` that record their calls: a fresh install, a second install
//! over the first, the binary search, and an uninstall. Both platforms are exercised on either
//! host, because the platform comes from the fake `uname`.

#![cfg(unix)]
// Test helpers outside `#[test]` functions may fail loudly; this is test code only.
#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

const ROOT: &str = env!("CARGO_MANIFEST_DIR");

const NAME: &str = "sylvode-runner";
const LABEL: &str = "dev.sylvode.runner";

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
        sandbox.add_binary(NAME);
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
            "systemd-run",
            &format!("echo \"systemd-run $*\" >> '{}'\nexit 0\n", log.display()),
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
        self.config_dir().join(NAME)
    }

    /// An executable file called `name` in the config directory.
    fn add_binary(&self, name: &str) -> PathBuf {
        let path = self.config_dir().join(name);
        // A stand-in that answers `doctor` the way the real one does for tmux: exit 2 with the row
        // and its hint when the mode needs tmux and the sandbox has none (only the sandbox's own
        // `bin` counts, never a tmux of the machine running the tests).
        let body = "#!/bin/sh\n\
            if [ \"$1\" = doctor ]; then\n\
              mode=''; cfg=''; while [ $# -gt 0 ]; do [ \"$1\" = --mode ] && mode=\"$2\"; [ \"$1\" = --config ] && cfg=\"$2\"; shift; done\n\
              if [ \"$mode\" = auto ] && grep -q 'enabled = true' \"$cfg\" 2>/dev/null; then mode=tmux; fi\n\
              here=\"$(cd \"$(dirname \"$0\")\" && pwd)\"\n\
              if [ \"$mode\" = tmux ] && [ ! -x \"$here/../bin/tmux\" ]; then\n\
                echo 'tmux  fail  not found  hint: install tmux >= 3.2: brew install tmux | sudo apt install tmux'\n\
                exit 2\n\
              fi\n\
              echo \"doctor ok ($mode)\"\n\
            fi\n\
            exit 0\n";
        std::fs::write(&path, body).unwrap();
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

    fn run_in(&self, dir: &Path, script: &str, args: &[&str]) -> Output {
        self.run_with_path(dir, script, args, "")
    }

    /// Like [`Sandbox::run_in`], with `extra` (a `PATH` entry) put before the system directories.
    fn run_with_path(&self, dir: &Path, script: &str, args: &[&str], extra: &str) -> Output {
        Command::new("bash")
            .arg(self.root.join("pkg/scripts").join(script))
            .args(args)
            .current_dir(dir)
            .env_clear()
            .env("HOME", self.root.join("home"))
            .env("USER", "tester")
            .env(
                "PATH",
                format!("{}{extra}:/usr/bin:/bin", self.root.join("bin").display()),
            )
            .output()
            .expect("run script")
    }

    /// Makes a `tmux` available on the sandbox's `PATH`.
    fn add_tmux(&self) {
        self.stub("tmux", "exit 0\n");
    }

    /// A configuration with a `runner` agent (the tmux channel is off).
    fn use_runner_config(&self) {
        std::fs::write(
            self.config_dir().join("config.toml"),
            "[[agents]]\nid = \"x\"\nname = \"X\"\nagent_type = \"runner\"\n",
        )
        .unwrap();
    }

    /// A configuration with a `runner` agent and the tmux channel switched on.
    fn use_tmux_config(&self) {
        std::fs::write(
            self.config_dir().join("config.toml"),
            "[runner.tmux]\nenabled = true\n[[agents]]\nid = \"x\"\nname = \"X\"\nagent_type = \"runner\"\n",
        )
        .unwrap();
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
fn a_fresh_install_writes_a_correctly_quoted_unit() {
    let sandbox = Sandbox::new(Platform::Linux);
    let output = sandbox.install();
    assert!(output.status.success(), "{}", stderr(&output));

    let unit = std::fs::read_to_string(sandbox.unit(NAME)).expect("unit written");
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
    let calls = sandbox.calls();
    assert_eq!(
        calls.first().map(String::as_str),
        Some("systemctl --user daemon-reload")
    );
    // Then doctor runs once more in the service context.
    assert!(
        calls
            .iter()
            .any(|c| c.starts_with("systemd-run --user --pipe --wait --setenv=PATH=")
                && c.contains(" doctor --mode auto ")),
        "{calls:?}"
    );
    assert!(stderr(&output).is_empty(), "{}", stderr(&output));
}

#[test]
fn installing_again_rewrites_the_unit_in_place() {
    let sandbox = Sandbox::new(Platform::Linux);
    let unit = sandbox.install_placeholder(NAME);
    let output = sandbox.install();
    assert!(output.status.success(), "{}", stderr(&output));
    let text = std::fs::read_to_string(&unit).unwrap();
    assert!(!text.contains("placeholder"), "the unit was rewritten: {text}");
    assert!(text.contains("journalctl --user -u sylvode-runner"), "{text}");
}

#[test]
fn uninstall_removes_the_unit() {
    let sandbox = Sandbox::new(Platform::Linux);
    sandbox.install_placeholder(NAME);
    let output = sandbox.run("uninstall.sh", &[]);
    assert!(output.status.success(), "{}", stderr(&output));
    assert!(!sandbox.unit(NAME).exists());
    let calls = sandbox.calls();
    assert!(
        calls.contains(&"systemctl --user is-active --quiet sylvode-runner".to_string()),
        "{calls:?}"
    );
}

#[test]
fn the_binary_is_found_in_the_working_directory_and_a_missing_one_is_refused() {
    let sandbox = Sandbox::new(Platform::Linux);
    let output = sandbox.install_autodetect();
    assert!(output.status.success(), "{}", stderr(&output));
    let unit = std::fs::read_to_string(sandbox.unit(NAME)).unwrap();
    // The shell reports its working directory with symbolic links resolved (/var is /private/var on macOS).
    let expected = std::fs::canonicalize(sandbox.config_dir()).unwrap().join(NAME);
    assert!(
        unit.contains(&format!("ExecStart=\"{}\"", quoted(expected.to_str().unwrap()))),
        "{unit}"
    );

    std::fs::remove_file(sandbox.binary()).unwrap();
    let refused = Sandbox::new(Platform::Linux);
    std::fs::remove_file(refused.binary()).unwrap();
    let output = refused.install_autodetect();
    assert!(!output.status.success());
    assert!(
        stderr(&output).contains("Could not locate the sylvode-runner binary"),
        "{}",
        stderr(&output)
    );
}

#[test]
fn a_fresh_macos_install_writes_the_agent() {
    let sandbox = Sandbox::new(Platform::MacOs);
    // Not root, whoever runs the test: a root run takes the service-context path of the next test.
    sandbox.stub("id", "echo 501\n");
    let output = sandbox.install();
    assert!(output.status.success(), "{}", stderr(&output));
    let plist = std::fs::read_to_string(sandbox.plist(LABEL)).expect("agent written");
    assert!(plist.contains("<string>dev.sylvode.runner</string>"), "{plist}");
    assert!(plist.contains("sylvode-runner.log"), "{plist}");
    assert!(!plist.contains("{{"), "unrendered placeholder: {plist}");
    let changes = sandbox.changes();
    assert_eq!(
        changes.first(),
        Some(&format!("launchctl load {}", sandbox.plist(LABEL).display()))
    );
    // `launchctl asuser` works for root only: as the user the second check is skipped, said so,
    // and never attempted.
    assert!(
        !changes.iter().any(|c| c.starts_with("launchctl asuser ")),
        "{changes:?}"
    );
    assert!(
        stderr(&output).contains("skipping the check in the service context"),
        "{}",
        stderr(&output)
    );
}

/// As root the second check runs through `launchctl asuser`.
#[test]
fn a_root_macos_install_checks_in_the_users_context() {
    let sandbox = Sandbox::new(Platform::MacOs);
    sandbox.stub("id", "echo 0\n");
    sandbox.stub("stat", "echo 501\n");
    let output = sandbox.install();
    assert!(output.status.success(), "{}", stderr(&output));
    let changes = sandbox.changes();
    assert!(
        changes
            .iter()
            .any(|c| c.starts_with("launchctl asuser ") && c.contains(" doctor --mode auto ")),
        "{changes:?}"
    );
}

#[test]
fn macos_uninstall_removes_the_agent() {
    let sandbox = Sandbox::new(Platform::MacOs);
    sandbox.install_placeholder(LABEL);
    let output = sandbox.run("uninstall.sh", &[]);
    assert!(output.status.success(), "{}", stderr(&output));
    assert!(!sandbox.plist(LABEL).exists());
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

/// Row 10 of the verification matrix: `--dry-run` without tmux, with a runner agent in the config:
/// non-zero, the tmux row and its hint are printed, nothing is installed.
#[test]
fn a_dry_run_without_tmux_prints_the_row_and_the_hint_and_installs_nothing() {
    for platform in [Platform::Linux, Platform::MacOs] {
        let sandbox = Sandbox::new(platform);
        sandbox.use_tmux_config();
        let output = sandbox.install_with(&["--dry-run"]);
        assert!(!output.status.success());
        let out = String::from_utf8_lossy(&output.stdout);
        assert!(out.contains("tmux") && out.contains("brew install tmux"), "{out}");
        assert!(stderr(&output).contains("nothing was installed"), "{}", stderr(&output));
        assert!(!sandbox.unit(NAME).exists() && !sandbox.plist(LABEL).exists());
        assert!(sandbox.calls().is_empty(), "{:?}", sandbox.calls());
    }
}

/// A headless-only install (tmux channel off, the default) needs no flag and no tmux: the mode is
/// `auto`, and the installer says nothing about tmux.
#[test]
fn a_headless_install_needs_neither_tmux_nor_a_flag() {
    let sandbox = Sandbox::new(Platform::Linux);
    sandbox.use_runner_config();
    let output = sandbox.install_with(&["--dry-run"]);
    assert!(output.status.success(), "{}", stderr(&output));
    assert!(!stderr(&output).contains("tmux"), "{}", stderr(&output));
    assert!(!sandbox.unit(NAME).exists());
    // The flag is gone.
    let refused = sandbox.install_with(&["--dry-run", "--headless-only"]);
    assert!(!refused.status.success());
}

/// A failed prerequisite stops a real install before the unit is written (the tmux channel is on,
/// so tmux is required, and the sandbox has none).
#[test]
fn a_failed_prerequisite_stops_the_install_before_the_unit_is_written() {
    let sandbox = Sandbox::new(Platform::Linux);
    sandbox.use_tmux_config();
    let output = sandbox.install();
    assert!(!output.status.success());
    assert!(!sandbox.unit(NAME).exists());
    assert!(sandbox.calls().is_empty());
}

/// With tmux available the install goes through; the configuration is never rewritten.
#[test]
fn an_install_with_tmux_goes_through_and_leaves_the_config_alone() {
    let sandbox = Sandbox::new(Platform::Linux);
    sandbox.use_tmux_config();
    sandbox.add_tmux();
    let before = std::fs::read(sandbox.config_dir().join("config.toml")).unwrap();
    let output = sandbox.install();
    assert!(output.status.success(), "{}", stderr(&output));
    assert!(sandbox.unit(NAME).exists());
    assert_eq!(std::fs::read(sandbox.config_dir().join("config.toml")).unwrap(), before);
    assert!(
        sandbox.calls().iter().any(|c| c.contains(" doctor --mode auto ")),
        "{:?}",
        sandbox.calls()
    );
}

/// The unit is put in place only when its content differs: a second identical install touches
/// nothing and reloads nothing; a changed `PATH` rewrites it.
#[test]
fn an_unchanged_unit_is_not_rewritten_and_a_changed_one_is() {
    let sandbox = Sandbox::new(Platform::Linux);
    assert!(sandbox.install().status.success());
    let unit = sandbox.unit(NAME);
    let first = std::fs::read_to_string(&unit).unwrap();
    // Age the file, so a rewrite would show in its modification time.
    let old = std::time::SystemTime::now() - std::time::Duration::from_hours(1);
    std::fs::File::options()
        .write(true)
        .open(&unit)
        .unwrap()
        .set_modified(old)
        .unwrap();
    std::fs::write(sandbox.root.join("calls.log"), "").unwrap();
    let again = sandbox.install();
    assert!(again.status.success(), "{}", stderr(&again));
    assert_eq!(std::fs::read_to_string(&unit).unwrap(), first);
    assert_eq!(
        std::fs::metadata(&unit).unwrap().modified().unwrap(),
        old,
        "the unit was rewritten"
    );
    assert!(
        !sandbox.calls().iter().any(|c| c.contains("daemon-reload")),
        "{:?}",
        sandbox.calls()
    );
    // Another PATH is another unit.
    let extra = sandbox.root.join("extra-bin");
    std::fs::create_dir_all(&extra).unwrap();
    let binary = sandbox.binary();
    let config_dir = sandbox.config_dir();
    let changed = sandbox.run_with_path(
        &sandbox.root,
        "install.sh",
        &[
            "--binary-path",
            binary.to_str().unwrap(),
            "--config-dir",
            config_dir.to_str().unwrap(),
        ],
        &format!(":{}", extra.display()),
    );
    assert!(changed.status.success(), "{}", stderr(&changed));
    assert!(std::fs::read_to_string(&unit).unwrap().contains("extra-bin"));
    assert!(sandbox.calls().iter().any(|c| c.contains("daemon-reload")));
}

/// Without linger the user service and its tmux server stop at logout: a warning with the command.
#[test]
fn a_user_without_linger_gets_the_exact_command() {
    let sandbox = Sandbox::new(Platform::Linux);
    sandbox.stub("loginctl", "echo Linger=no\n");
    let output = sandbox.install();
    assert!(output.status.success(), "{}", stderr(&output));
    assert!(
        stderr(&output).contains("loginctl enable-linger tester"),
        "{}",
        stderr(&output)
    );
    // With linger on there is no warning.
    let sandbox = Sandbox::new(Platform::Linux);
    sandbox.stub("loginctl", "echo Linger=yes\n");
    let output = sandbox.install();
    assert!(!stderr(&output).contains("linger"), "{}", stderr(&output));
}
