#![allow(clippy::indexing_slicing)]

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

use serde_json::{Value, json};

use super::files::RUNNER_MARK;
use super::setup::{Applied, Inputs, Plan, apply, merge_settings, plan, render};
use crate::config::Config;

const BOT: &str = "00000000-0000-4000-8000-000000000001";

struct Dir(PathBuf);

impl Dir {
    fn new() -> Self {
        let dir = std::env::temp_dir().join(format!("sylvode-setup-test-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&dir).expect("mkdir");
        Self(dir.canonicalize().expect("canonical"))
    }

    fn sub(&self, name: &str) -> PathBuf {
        let path = self.0.join(name);
        fs::create_dir_all(&path).expect("mkdir");
        path
    }

    /// Everything below, as relative paths, sorted.
    fn listing(&self) -> Vec<String> {
        fn walk(dir: &Path, base: &Path, out: &mut Vec<String>) {
            for entry in fs::read_dir(dir).into_iter().flatten().flatten() {
                let path = entry.path();
                out.push(path.strip_prefix(base).unwrap_or(&path).to_string_lossy().into_owned());
                if path.is_dir() {
                    walk(&path, base, out);
                }
            }
        }
        let mut out = Vec::new();
        walk(&self.0, &self.0, &mut out);
        out.sort();
        out
    }
}

impl Drop for Dir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn config() -> Config {
    Config::from_toml(include_str!("../../../api/tests/fixtures/runner-config-block.toml")).expect("block")
}

fn plan_in(base: &Dir, project: &Path) -> Plan {
    let state = base.0.join("state");
    plan(
        &config(),
        &Inputs {
            key: "QLA",
            dir: project.to_path_buf(),
            bot_id: BOT,
            state_dir: &state,
            runner_exe: Path::new("/opt/sylvode/bin/sylvode-runner"),
            mcp_server: Path::new("/opt/sylvode/bin/mcp-server"),
            launch_program: Path::new("/usr/local/bin/claude"),
            allow_write_call: false,
        },
    )
    .expect("plan")
}

/// A stand-in for `claude` that records its calls and keeps the MCP entry in a file, so nothing
/// real is ever run or written.
fn fake_claude(base: &Dir) -> (PathBuf, PathBuf, PathBuf) {
    let log = base.0.join("claude.log");
    let added = base.0.join("claude.added");
    let script = base.0.join("claude");
    fs::write(
        &script,
        format!(
            "#!/bin/sh\necho \"$PWD|$*\" >> '{log}'\n\
             if [ \"$1 $2\" = \"mcp get\" ]; then [ -f '{added}' ] && {{ echo \"sylvode: $(cat '{added}')\"; exit 0; }}; exit 1; fi\n\
             if [ \"$1 $2\" = \"mcp add-json\" ]; then printf '%s' \"$6\" > '{added}'; exit 0; fi\nexit 1\n",
            log = log.display(),
            added = added.display()
        ),
    )
    .expect("script");
    fs::set_permissions(&script, fs::Permissions::from_mode(0o755)).expect("chmod");
    (script, log, added)
}

fn calls(log: &Path) -> Vec<String> {
    fs::read_to_string(log)
        .unwrap_or_default()
        .lines()
        .map(str::to_string)
        .collect()
}

#[test]
fn the_plan_has_the_launch_line_the_mcp_entry_and_marked_hooks() {
    let base = Dir::new();
    let project = base.sub("project");
    let plan = plan_in(&base, &project);
    let state = base.0.join("state");
    assert_eq!(plan.name.as_str(), "QLA-claude");
    assert_eq!(
        plan.launch,
        [
            "/usr/local/bin/claude".to_string(),
            "--permission-mode".to_string(),
            "bypassPermissions".to_string(),
            "--mcp-config".to_string(),
            state
                .join(format!("mcp/{BOT}.claude.json"))
                .to_string_lossy()
                .into_owned(),
            "--settings".to_string(),
            state
                .join("sessions/QLA-claude/claude-settings.json")
                .to_string_lossy()
                .into_owned(),
        ]
    );
    assert_eq!(plan.mcp_entry["type"], "stdio");
    assert_eq!(plan.mcp_entry["command"], "/opt/sylvode/bin/mcp-server");
    assert_eq!(
        plan.mcp_entry["args"][4].as_str(),
        state.join(format!("mcp/{BOT}.toml")).to_str()
    );
    assert_eq!(plan.hooks.len(), 4);
    for (event, entries) in &plan.hooks {
        let hook = &entries[0]["hooks"][0];
        assert_eq!(hook[RUNNER_MARK], 1, "{event}");
        let command = hook["command"].as_str().expect("command");
        assert!(
            command.starts_with("'/opt/sylvode/bin/sylvode-runner' 'hook' '--cli' 'claude' '--event' ")
                && command.ends_with("/state/hook.sock'")
                && !command.contains("--nonce")
                && !command.contains("--session"),
            "{command}"
        );
    }
    assert_eq!(plan.allow.len(), 8);
    assert!(plan.allow.iter().all(|tool| tool.starts_with("mcp__sylvode__")));
    // A key that is not a project key, and a directory with `#`, are refused.
    let bad = plan_with_key(&base, &project, "qla");
    assert!(bad.is_err());
}

fn plan_with_key(base: &Dir, project: &Path, key: &str) -> Result<Plan, String> {
    let state = base.0.join("state");
    plan(
        &config(),
        &Inputs {
            key,
            dir: project.to_path_buf(),
            bot_id: BOT,
            state_dir: &state,
            runner_exe: Path::new("/r"),
            mcp_server: Path::new("/m"),
            launch_program: Path::new("/c"),
            allow_write_call: true,
        },
    )
}

#[test]
fn the_printed_text_names_the_session_and_shows_no_token() {
    let base = Dir::new();
    let project = base.sub("my project");
    let plan = plan_in(&base, &project);
    let text = render(&plan);
    assert!(text.contains("Session name: QLA-claude"), "{text}");
    assert!(
        text.contains(&format!("tmux new-session -d -s QLA-claude -c '{}'", project.display())),
        "{text}"
    );
    assert!(
        text.contains("'--mcp-config'") && text.contains("'--settings'"),
        "{text}"
    );
    assert!(text.contains("\"mcpServers\"") || text.contains("\"command\": \"/opt/sylvode/bin/mcp-server\""));
    assert!(text.contains("\"hooks\"") && text.contains(RUNNER_MARK), "{text}");
    // Only the path of the file that holds the token.
    assert!(text.contains(&format!("{BOT}.toml")));
    assert!(!text.contains("opr_") && !text.contains("bot_token"), "{text}");
    // Rendering writes nothing.
    assert_eq!(base.listing(), ["my project"]);
    // The write call widens the allowed tools.
    let wide = plan_with_key(&base, &project, "QLA").expect("plan");
    assert!(wide.allow.contains(&"mcp__sylvode__sylvode_write".to_string()));
}

fn hooks_of(base: &Dir) -> serde_json::Map<String, Value> {
    plan_in(base, &base.0).hooks
}

#[test]
fn merging_keeps_everything_that_is_not_the_runners_and_is_idempotent() {
    let base = Dir::new();
    let hooks = hooks_of(&base);
    let allow = vec!["mcp__sylvode__sylvode_read".to_string()];
    let existing = json!({
        "model": "opus",
        "env": {"A": "1"},
        "permissions": {"allow": ["Bash(git status)"], "deny": ["Read(.env)"]},
        "hooks": {
            "Stop": [{"hooks": [{"type": "command", "command": "notify-send done"}]}],
            "PreToolUse": [{"matcher": "Bash", "hooks": [{"type": "command", "command": "audit"}]}]
        }
    })
    .to_string();
    let once = merge_settings(&existing, &hooks, &allow).expect("merged");
    let value: Value = serde_json::from_str(&once).expect("json");
    assert_eq!(value["model"], "opus");
    assert_eq!(value["env"], json!({"A": "1"}));
    assert_eq!(value["permissions"]["deny"], json!(["Read(.env)"]));
    assert_eq!(
        value["permissions"]["allow"],
        json!(["Bash(git status)", "mcp__sylvode__sylvode_read"])
    );
    assert_eq!(value["hooks"]["PreToolUse"][0]["hooks"][0]["command"], "audit");
    // The operator's own Stop hook stays, ours is added beside it.
    let stop = value["hooks"]["Stop"].as_array().expect("stop");
    assert_eq!(stop.len(), 2);
    assert_eq!(stop[0]["hooks"][0]["command"], "notify-send done");
    assert_eq!(stop[1]["hooks"][0][RUNNER_MARK], 1);
    assert_eq!(value["hooks"].as_object().map(serde_json::Map::len), Some(5));
    // A second run changes nothing, byte for byte.
    assert_eq!(merge_settings(&once, &hooks, &allow).expect("again"), once);
    // A changed runner path replaces only the runner's hooks.
    let mut moved = hooks.clone();
    if let Some(stop) = moved.get_mut("Stop") {
        stop[0]["hooks"][0]["command"] = json!("'/new/sylvode-runner' 'hook'");
    }
    let replaced: Value = serde_json::from_str(&merge_settings(&once, &moved, &allow).expect("moved")).expect("json");
    let stop = replaced["hooks"]["Stop"].as_array().expect("stop");
    assert_eq!(stop.len(), 2, "{stop:?}");
    assert_eq!(stop[0]["hooks"][0]["command"], "notify-send done");
    assert_eq!(stop[1]["hooks"][0]["command"], "'/new/sylvode-runner' 'hook'");
    // A hook object of the runner inside an entry that holds the operator's too: only ours goes.
    let shared = json!({"hooks": {"Stop": [{"hooks": [
        {"type": "command", "command": "mine"},
        {"type": "command", "command": "old", RUNNER_MARK: 1}
    ]}]}})
    .to_string();
    let merged: Value = serde_json::from_str(&merge_settings(&shared, &hooks, &allow).expect("shared")).expect("json");
    assert_eq!(
        merged["hooks"]["Stop"][0]["hooks"],
        json!([{"type": "command", "command": "mine"}])
    );
}

#[test]
fn merging_into_nothing_makes_the_file() {
    let base = Dir::new();
    let hooks = hooks_of(&base);
    for empty in ["", "  \n", "{}"] {
        let value: Value =
            serde_json::from_str(&merge_settings(empty, &hooks, &["t".to_string()]).expect("merged")).expect("json");
        assert_eq!(value["hooks"].as_object().map(serde_json::Map::len), Some(4));
        assert_eq!(value["permissions"]["allow"], json!(["t"]));
    }
}

#[test]
fn a_settings_file_of_the_wrong_shape_is_refused_not_overwritten() {
    let base = Dir::new();
    let hooks = hooks_of(&base);
    for bad in [
        "not json",
        "[]",
        r#"{"hooks": []}"#,
        r#"{"hooks": {"Stop": {}}}"#,
        r#"{"permissions": []}"#,
        r#"{"permissions": {"allow": "x"}}"#,
    ] {
        assert!(merge_settings(bad, &hooks, &[]).is_err(), "{bad}");
    }
}

fn applied(base: &Dir, project: &Path, claude: &Path) -> Applied {
    apply(&plan_in(base, project), claude).expect("applied")
}

#[test]
fn apply_adds_the_entry_and_the_settings_once_and_never_twice() {
    let base = Dir::new();
    let project = base.sub("project");
    let (claude, log, _added) = fake_claude(&base);
    let first = applied(&base, &project, &claude);
    assert_eq!(first.code, 0);
    assert!(
        first.text.contains("added (scope local)") && first.text.contains("Restart your claude"),
        "{}",
        first.text
    );
    let settings = project.join(".claude/settings.local.json");
    let text = fs::read_to_string(&settings).expect("settings");
    let value: Value = serde_json::from_str(&text).expect("json");
    assert_eq!(value["hooks"].as_object().map(serde_json::Map::len), Some(4));
    assert_eq!(value["permissions"]["allow"].as_array().map(Vec::len), Some(8));
    // `claude` ran in the directory: first a look, then one `add-json --scope local`.
    let first_calls = calls(&log);
    assert_eq!(first_calls.len(), 2, "{first_calls:?}");
    assert!(first_calls[0].ends_with("|mcp get sylvode") && first_calls[0].starts_with(&*project.to_string_lossy()));
    assert!(
        first_calls[1].contains("|mcp add-json --scope local sylvode {"),
        "{first_calls:?}"
    );
    assert!(!first_calls[1].contains("opr_") && first_calls[1].contains(&format!("{BOT}.toml")));
    // The second run finds both in place: nothing is added, nothing is rewritten.
    let before = fs::metadata(&settings).expect("meta").modified().expect("mtime");
    let second = applied(&base, &project, &claude);
    assert_eq!(second.code, 0);
    assert!(
        second.text.contains("already set") && second.text.contains("already current"),
        "{}",
        second.text
    );
    assert_eq!(fs::read_to_string(&settings).expect("settings"), text);
    assert_eq!(
        fs::metadata(&settings).expect("meta").modified().expect("mtime"),
        before
    );
    let all = calls(&log);
    assert_eq!(all.len(), 3, "{all:?}");
    assert!(all[2].ends_with("|mcp get sylvode"));
    // No temporary file is left.
    assert_eq!(
        fs::read_dir(project.join(".claude")).expect("dir").count(),
        1,
        "{:?}",
        base.listing()
    );
}

#[test]
fn apply_leaves_a_foreign_sylvode_entry_alone_and_changes_nothing() {
    let base = Dir::new();
    let project = base.sub("project");
    let (claude, log, added) = fake_claude(&base);
    fs::write(
        &added,
        r#"{"command":"/other/server","args":["--config","/elsewhere/other.toml"]}"#,
    )
    .expect("foreign");
    let done = applied(&base, &project, &claude);
    assert_eq!(done.code, 2);
    assert!(
        done.text.contains("points elsewhere") && done.text.contains("left alone"),
        "{}",
        done.text
    );
    assert!(!project.join(".claude").exists(), "no settings are written");
    assert_eq!(calls(&log).len(), 1, "only the look: {:?}", calls(&log));
    assert_eq!(
        fs::read_to_string(&added).expect("entry"),
        r#"{"command":"/other/server","args":["--config","/elsewhere/other.toml"]}"#
    );
}

#[test]
fn apply_keeps_an_existing_settings_file_and_refuses_one_it_cannot_read() {
    let base = Dir::new();
    let project = base.sub("project");
    let (claude, _log, _added) = fake_claude(&base);
    fs::create_dir_all(project.join(".claude")).expect("mkdir");
    let settings = project.join(".claude/settings.local.json");
    fs::write(
        &settings,
        r#"{"model":"opus","hooks":{"Stop":[{"hooks":[{"type":"command","command":"mine"}]}]}}"#,
    )
    .expect("settings");
    applied(&base, &project, &claude);
    let value: Value = serde_json::from_str(&fs::read_to_string(&settings).expect("settings")).expect("json");
    assert_eq!(value["model"], "opus");
    assert_eq!(value["hooks"]["Stop"][0]["hooks"][0]["command"], "mine");
    assert_eq!(value["hooks"]["Stop"][1]["hooks"][0][RUNNER_MARK], 1);
    // Broken JSON: an error, and the file is exactly as it was.
    fs::write(&settings, "{broken").expect("settings");
    assert!(apply(&plan_in(&base, &project), &claude).is_err());
    assert_eq!(fs::read_to_string(&settings).expect("settings"), "{broken");
}

#[test]
fn a_failing_claude_is_an_error_and_writes_no_settings() {
    let base = Dir::new();
    let project = base.sub("project");
    let script = base.0.join("claude");
    fs::write(&script, "#!/bin/sh\nexit 1\n").expect("script");
    fs::set_permissions(&script, fs::Permissions::from_mode(0o755)).expect("chmod");
    assert!(apply(&plan_in(&base, &project), &script).is_err());
    assert!(!project.join(".claude").exists());
    assert!(apply(&plan_in(&base, &project), &base.0.join("absent")).is_err());
}
