//! `sylvode-runner setup claude` through the real executable, with a temporary `HOME`, temporary
//! project and state directories and a fake `claude`: nothing real is run and nothing outside the
//! temporary directories is read or written.

use crate::handshake::{BOT, config};
use crate::support::{RUNNER, TempDir, write_script};
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

struct Fixture {
    home: TempDir,
    project: PathBuf,
    state: TempDir,
    bin: TempDir,
    config: PathBuf,
    log: PathBuf,
    added: PathBuf,
    // Kept for the life of the test: the project lives in it.
    _root: TempDir,
}

impl Fixture {
    fn new() -> Self {
        let root = TempDir::new("setup-root");
        let canonical = root.path().canonicalize().unwrap_or_else(|_| root.path().to_path_buf());
        let project = canonical.join("qla");
        std::fs::create_dir_all(&project).expect("mkdir");
        let bin = TempDir::new("setup-bin");
        let state = TempDir::new("setup-state");
        let home = TempDir::new("setup-home");
        let log = bin.file("claude.log");
        let added = bin.file("claude.added");
        // The fake keeps the `sylvode` MCP entry in a file and records every call.
        write_script(
            bin.path(),
            "claude",
            &format!(
                "echo \"$*\" >> '{log}'\n\
                 if [ \"$1 $2\" = \"mcp get\" ]; then [ -f '{added}' ] && {{ echo \"sylvode: $(cat '{added}')\"; exit 0; }}; exit 1; fi\n\
                 if [ \"$1 $2\" = \"mcp add-json\" ]; then printf '%s' \"$6\" > '{added}'; exit 0; fi\nexit 1",
                log = log.display(),
                added = added.display()
            ),
        );
        write_script(bin.path(), "mcp-server", "exit 0");
        let text = format!(
            "{}\n[runner.mcp]\nserver_binary = \"{}\"\n[runner.cli.claude]\nbinary = \"{}\"\nlaunch_command = [\"{}\", \"--model\", \"x\"]\n",
            config("http://127.0.0.1:9", &state, &canonical.to_string_lossy()),
            bin.file("mcp-server").display(),
            bin.file("claude").display(),
            bin.file("claude").display(),
        );
        let config_path = bin.file("config.toml");
        std::fs::write(&config_path, text).expect("config");
        Self {
            home,
            project,
            state,
            bin,
            config: config_path,
            log,
            added,
            _root: root,
        }
    }

    fn setup(&self, extra: &[&str]) -> Output {
        Command::new(RUNNER)
            .args(["setup", "claude", "--project", "QLA", "--config"])
            .arg(&self.config)
            .arg("--dir")
            .arg(&self.project)
            .args(extra)
            .env("HOME", self.home.path())
            .env("PATH", format!("{}:/usr/bin:/bin", self.bin.path().display()))
            .env_remove("TMUX")
            .output()
            .expect("run setup")
    }

    fn calls(&self) -> Vec<String> {
        std::fs::read_to_string(&self.log)
            .unwrap_or_default()
            .lines()
            .map(str::to_string)
            .collect()
    }
}

fn tree(dir: &Path) -> Vec<String> {
    fn walk(dir: &Path, base: &Path, out: &mut Vec<String>) {
        for entry in std::fs::read_dir(dir).into_iter().flatten().flatten() {
            let path = entry.path();
            out.push(path.strip_prefix(base).unwrap_or(&path).to_string_lossy().into_owned());
            if path.is_dir() {
                walk(&path, base, out);
            }
        }
    }
    let mut out = Vec::new();
    walk(dir, dir, &mut out);
    out.sort();
    out
}

#[test]
fn setup_without_apply_prints_and_writes_nothing() {
    let fx = Fixture::new();
    let output = fx.setup(&[]);
    let text = String::from_utf8_lossy(&output.stdout).to_string();
    assert!(
        output.status.success(),
        "{text}\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(text.contains("Session name: QLA-claude"), "{text}");
    assert!(text.contains("tmux new-session -d -s QLA-claude -c "), "{text}");
    // The launch line: the operator's words, then the runner's MCP file and hook settings.
    assert!(
        text.contains("'--model' 'x' '--mcp-config' ") && text.contains("'--settings' "),
        "{text}"
    );
    assert!(
        text.contains(&format!("{BOT}.toml")) && text.contains("\"hooks\""),
        "{text}"
    );
    // No token anywhere in what is printed.
    assert!(!text.contains("opr_"), "{text}");
    // Nothing was written: no project file, no state directory, nothing in HOME, no claude call.
    assert!(tree(&fx.project).is_empty(), "{:?}", tree(&fx.project));
    assert!(tree(fx.home.path()).is_empty(), "{:?}", tree(fx.home.path()));
    assert!(!fx.state.path().join("s").exists(), "{:?}", tree(fx.state.path()));
    assert!(fx.calls().is_empty(), "{:?}", fx.calls());
}

#[test]
fn setup_apply_is_idempotent_and_keeps_what_is_not_the_runners() {
    let fx = Fixture::new();
    std::fs::create_dir_all(fx.project.join(".claude")).expect("mkdir");
    let settings = fx.project.join(".claude/settings.local.json");
    std::fs::write(
        &settings,
        r#"{"model":"opus","hooks":{"Stop":[{"hooks":[{"type":"command","command":"mine"}]}]}}"#,
    )
    .expect("settings");
    let first = fx.setup(&["--apply"]);
    assert!(first.status.success(), "{}", String::from_utf8_lossy(&first.stderr));
    let text = std::fs::read_to_string(&settings).expect("settings");
    let value: serde_json::Value = serde_json::from_str(&text).expect("json");
    assert_eq!(value["model"], "opus");
    assert_eq!(value["hooks"]["Stop"][0]["hooks"][0]["command"], "mine");
    assert_eq!(value["hooks"]["Stop"][1]["hooks"][0]["sylvode-runner"], 1);
    assert_eq!(value["hooks"]["SessionStart"][0]["matcher"], "startup");
    assert!(fx.added.is_file(), "the MCP entry was added through claude");
    let entry = std::fs::read_to_string(&fx.added).expect("entry");
    assert!(
        entry.contains(&format!("{BOT}.toml")) && !entry.contains("opr_"),
        "{entry}"
    );
    assert_eq!(
        fx.calls()
            .iter()
            .filter(|c| c.starts_with("mcp add-json --scope local sylvode "))
            .count(),
        1
    );
    let second = fx.setup(&["--apply"]);
    assert!(second.status.success());
    assert_eq!(std::fs::read_to_string(&settings).expect("settings"), text);
    assert_eq!(
        fx.calls().iter().filter(|c| c.starts_with("mcp add-json")).count(),
        1,
        "{:?}",
        fx.calls()
    );
    // Only the project's own `.claude` was touched; HOME stayed empty.
    assert!(tree(fx.home.path()).is_empty(), "{:?}", tree(fx.home.path()));
}

#[test]
fn setup_apply_leaves_a_foreign_sylvode_entry_alone_with_exit_code_two() {
    let fx = Fixture::new();
    std::fs::write(&fx.added, r#"{"args":["--config","/elsewhere/other.toml"]}"#).expect("foreign");
    let output = fx.setup(&["--apply"]);
    assert_eq!(
        output.status.code(),
        Some(2),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(String::from_utf8_lossy(&output.stdout).contains("left alone"));
    assert!(!fx.project.join(".claude").exists());
    assert_eq!(
        std::fs::read_to_string(&fx.added).expect("entry"),
        r#"{"args":["--config","/elsewhere/other.toml"]}"#
    );
}

#[test]
fn setup_needs_a_known_directory_a_key_and_a_bot_of_this_runner() {
    let fx = Fixture::new();
    let run = |args: &[&str]| {
        Command::new(RUNNER)
            .arg("setup")
            .args(args)
            .env("HOME", fx.home.path())
            .env("PATH", format!("{}:/usr/bin:/bin", fx.bin.path().display()))
            .output()
            .expect("run")
    };
    let config = fx.config.to_string_lossy().into_owned();
    // No --dir and no record of the project yet.
    let no_dir = run(&["claude", "--project", "QLA", "--config", &config]);
    assert!(!no_dir.status.success() && String::from_utf8_lossy(&no_dir.stderr).contains("--dir"));
    // A key that is not a project key.
    let dir = fx.project.to_string_lossy().into_owned();
    let bad_key = run(&["claude", "--project", "qla", "--config", &config, "--dir", &dir]);
    assert!(!bad_key.status.success());
    // A bot this runner does not serve.
    let other = run(&[
        "claude",
        "--project",
        "QLA",
        "--config",
        &config,
        "--dir",
        &dir,
        "--bot",
        "nope",
    ]);
    assert!(!other.status.success() && String::from_utf8_lossy(&other.stderr).contains("not one of"));
    assert!(tree(&fx.project).is_empty() && tree(fx.home.path()).is_empty());
}
