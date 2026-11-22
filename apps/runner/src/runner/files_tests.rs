#![allow(clippy::indexing_slicing)]

use std::fs;
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::path::{Path, PathBuf};

use super::files::{BotFiles, CLAUDE_EVENTS, CreatedBy, PushedTask, SessionRecord, StateDir};
use super::launch::LaunchFiles;
use super::tmux::SessionName;

const BOT: &str = "00000000-0000-4000-8000-000000000001";
// Built so no token-shaped literal sits in the source.
fn token() -> String {
    format!("opr_{}", "k7".repeat(16))
}

struct Dir(PathBuf);

impl Dir {
    fn new() -> Self {
        let dir = std::env::temp_dir().join(format!("sylvode-files-test-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&dir).expect("create dir");
        Self(dir)
    }

    fn state(&self) -> PathBuf {
        self.0.join("state")
    }
}

impl Drop for Dir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn mode(path: &Path) -> u32 {
    fs::symlink_metadata(path).expect("meta").permissions().mode() & 0o7777
}

fn bot(token: &str) -> BotFiles<'_> {
    BotFiles {
        bot_id: BOT,
        api_url: "https://sylvode.example.net",
        workspace_id: "11111111-1111-4111-8111-111111111111",
        token,
        tool_mode: "search",
        allow_write_call: false,
    }
}

fn launch_files(state: &StateDir) -> (PathBuf, PathBuf, PathBuf) {
    let _ = state;
    (
        PathBuf::from("/opt/x/sylvode-runner"),
        PathBuf::from("/opt/x/mcp-server"),
        PathBuf::from("/s/hook.sock"),
    )
}

#[test]
fn the_state_directory_is_created_private_and_a_wider_one_is_refused() {
    let base = Dir::new();
    let state = StateDir::open(&base.state().join("deep").join("er")).expect("open");
    assert_eq!(mode(state.root()), 0o700);
    assert_eq!(mode(&base.state()), 0o700, "created parents are private as well");
    // An existing directory is accepted when private, refused when group or others can reach it.
    let wide = base.0.join("wide");
    fs::create_dir_all(&wide).expect("mkdir");
    fs::set_permissions(&wide, fs::Permissions::from_mode(0o755)).expect("chmod");
    let error = StateDir::open(&wide).expect_err("refused");
    assert!(error.contains("chmod 700"), "{error}");
    fs::set_permissions(&wide, fs::Permissions::from_mode(0o750)).expect("chmod");
    assert!(StateDir::open(&wide).is_err());
    fs::set_permissions(&wide, fs::Permissions::from_mode(0o700)).expect("chmod");
    assert!(StateDir::open(&wide).is_ok());
}

#[test]
fn bot_files_have_the_final_mode_hold_the_token_once_and_the_json_none() {
    let base = Dir::new();
    let state = StateDir::open(&base.state()).expect("open");
    let t = token();
    let written = state
        .write_bot_files(&bot(&t), Path::new("/opt/x/mcp-server"))
        .expect("write");
    assert!(written.changed);
    let toml_path = state.mcp_toml(BOT);
    let json_path = state.claude_mcp_json(BOT);
    assert_eq!(mode(&state.root().join("mcp")), 0o700);
    assert_eq!(mode(&toml_path), 0o600);
    assert_eq!(mode(&json_path), 0o600);
    let toml_text = fs::read_to_string(&toml_path).expect("toml");
    assert!(toml_text.contains(&t));
    let parsed: toml::Table = toml::from_str(&toml_text).expect("valid toml");
    let mcp = parsed["mcp"].as_table().expect("[mcp]");
    assert_eq!(mcp["bot_token"].as_str(), Some(t.as_str()));
    assert_eq!(
        mcp["workspace_id"].as_str(),
        Some("11111111-1111-4111-8111-111111111111")
    );
    assert_eq!(mcp["tool_mode"].as_str(), Some("search"));
    assert_eq!(mcp["allow_write_call"].as_bool(), Some(false));
    assert_eq!(mcp["api_url"].as_str(), Some("https://sylvode.example.net"));
    // The claude file names the server and the TOML, and holds no token.
    let json_text = fs::read_to_string(&json_path).expect("json");
    assert!(!json_text.contains("opr_") && !json_text.contains(&t), "{json_text}");
    let json: serde_json::Value = serde_json::from_str(&json_text).expect("json");
    assert_eq!(json["mcpServers"]["sylvode"]["command"], "/opt/x/mcp-server");
    assert_eq!(
        json["mcpServers"]["sylvode"]["args"],
        serde_json::json!(["serve", "--transport", "stdio", "--config", toml_path.to_string_lossy()])
    );
    assert!(state.wide_modes().is_empty());
}

#[test]
fn files_are_rewritten_only_when_they_differ_and_never_through_a_link() {
    let base = Dir::new();
    let state = StateDir::open(&base.state()).expect("open");
    let t = token();
    state
        .write_bot_files(&bot(&t), Path::new("/opt/x/mcp-server"))
        .expect("write");
    let path = state.mcp_toml(BOT);
    let inode = fs::metadata(&path).expect("meta").ino();
    // Same content: nothing is written.
    assert!(
        !state
            .write_bot_files(&bot(&t), Path::new("/opt/x/mcp-server"))
            .expect("write")
            .changed
    );
    assert_eq!(fs::metadata(&path).expect("meta").ino(), inode);
    // A changed setting is written through a new file, atomically.
    let widened = BotFiles {
        allow_write_call: true,
        ..bot(&t)
    };
    assert!(
        state
            .write_bot_files(&widened, Path::new("/opt/x/mcp-server"))
            .expect("write")
            .changed
    );
    assert_ne!(fs::metadata(&path).expect("meta").ino(), inode);
    assert!(
        fs::read_to_string(&path)
            .expect("toml")
            .contains("allow_write_call = true")
    );
    assert_eq!(mode(&path), 0o600);
    // No temporary file is left behind.
    let names: Vec<_> = fs::read_dir(state.root().join("mcp"))
        .expect("dir")
        .filter_map(Result::ok)
        .collect();
    assert_eq!(names.len(), 2);
    // A file that someone made world-readable is replaced even when the content is the same.
    fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).expect("chmod");
    assert_eq!(state.wide_modes().len(), 1);
    assert!(
        state
            .write_bot_files(&widened, Path::new("/opt/x/mcp-server"))
            .expect("write")
            .changed
    );
    assert_eq!(mode(&path), 0o600);
    // A symbolic link in place of the file is replaced by a file; its target is never written.
    let target = base.0.join("target");
    fs::write(&target, "untouched").expect("write");
    fs::remove_file(&path).expect("rm");
    std::os::unix::fs::symlink(&target, &path).expect("symlink");
    state
        .write_bot_files(&widened, Path::new("/opt/x/mcp-server"))
        .expect("write");
    assert_eq!(fs::read_to_string(&target).expect("read"), "untouched");
    assert!(!fs::symlink_metadata(&path).expect("meta").file_type().is_symlink());
    // A symbolic link in place of the mcp directory is refused.
    fs::remove_dir_all(state.root().join("mcp")).expect("rm");
    let elsewhere = base.0.join("elsewhere");
    fs::create_dir_all(&elsewhere).expect("mkdir");
    std::os::unix::fs::symlink(&elsewhere, state.root().join("mcp")).expect("symlink");
    assert!(state.write_bot_files(&widened, Path::new("/opt/x/mcp-server")).is_err());
    assert_eq!(fs::read_dir(&elsewhere).expect("dir").count(), 0);
}

#[test]
fn a_token_or_bot_id_of_the_wrong_shape_is_refused_without_echoing_it() {
    let base = Dir::new();
    let state = StateDir::open(&base.state()).expect("open");
    for token in ["", "plain", "opr_a b", "opr_a\nb"] {
        let error = state
            .write_bot_files(&bot(token), Path::new("/x"))
            .expect_err("refused");
        assert!(!error.contains("plain") && !error.contains("opr_a"), "{error}");
    }
    for id in ["", "../../etc", "not-a-uuid", "0000000000000000000000000000000x"] {
        let t = token();
        let refused = BotFiles { bot_id: id, ..bot(&t) };
        assert!(state.write_bot_files(&refused, Path::new("/x")).is_err(), "{id:?}");
    }
    assert!(!state.root().join("mcp").exists() || fs::read_dir(state.root().join("mcp")).expect("dir").count() == 0);
}

#[test]
fn debug_output_hides_the_token() {
    let t = token();
    let shown = format!("{:?}", bot(&t));
    assert!(!shown.contains(&t) && !shown.contains("opr_"), "{shown}");
}

#[test]
fn session_settings_hold_the_four_hooks_and_are_private() {
    let base = Dir::new();
    let state = StateDir::open(&base.state()).expect("open");
    let name = SessionName::new("SVWL", "claude").expect("name");
    let (exe, _server, socket) = launch_files(&state);
    let json = state.claude_mcp_json(BOT);
    let settings = state.claude_settings(&name);
    assert_eq!(settings, state.root().join("sessions/SVWL-claude/claude-settings.json"));
    let files = LaunchFiles {
        claude_mcp_json: &json,
        claude_settings: &settings,
        runner_exe: &exe,
        hook_socket: &socket,
    };
    assert!(state.write_session_settings(&name, &files).expect("write").changed);
    assert!(!state.write_session_settings(&name, &files).expect("again").changed);
    assert_eq!(mode(&settings), 0o600);
    assert_eq!(mode(settings.parent().expect("dir")), 0o700);
    let value: serde_json::Value = serde_json::from_str(&fs::read_to_string(&settings).expect("read")).expect("json");
    for event in CLAUDE_EVENTS {
        let command = value["hooks"][event][0]["hooks"][0]["command"]
            .as_str()
            .expect("command");
        assert!(
            command.starts_with("'/opt/x/sylvode-runner' 'hook' '--cli' 'claude' '--event' "),
            "{command}"
        );
        assert!(command.contains(&format!("'--event' '{event}'")), "{command}");
        assert_eq!(value["hooks"][event][0]["hooks"][0]["type"], "command");
        // The settings hold no secret: no session name, no nonce, only the socket path.
        assert!(command.ends_with("'--socket' '/s/hook.sock'"), "{command}");
        assert!(
            !command.contains("--nonce") && !command.contains("--session"),
            "{command}"
        );
    }
    // The hook words never carry task content: only fixed words, the key and the paths.
    assert!(!value.to_string().contains("opr_"));
}

#[test]
fn the_lock_admits_one_runner_per_state_directory() {
    let base = Dir::new();
    let state = StateDir::open(&base.state()).expect("open");
    let first = state.lock().expect("first lock");
    let error = state.lock().expect_err("second");
    assert!(error.contains("another runner"), "{error}");
    drop(first);
    assert!(state.lock().is_ok());
    assert_eq!(mode(&state.root().join("runner.lock")), 0o600);
}

fn record() -> SessionRecord {
    SessionRecord {
        name: "SVWL-claude".to_string(),
        project_id: "p".to_string(),
        bot_id: BOT.to_string(),
        cli: "claude".to_string(),
        pane_id: "%1".to_string(),
        pane_pid: 7,
        dir: "/x".to_string(),
        created_by: CreatedBy::Operator,
        pushed: None,
    }
}

/// The record is kept under the session's derived name, round-trips with who started the session,
/// and a `pushed` mark needs a record to exist.
#[test]
fn a_session_record_is_kept_by_name_with_who_started_the_session() {
    let dir = Dir::new();
    let state = StateDir::open(&dir.state()).expect("open");
    let name = SessionName::new("SVWL", "claude").expect("name");
    assert!(state.read_session_record(&name).is_none());
    assert!(
        state
            .mark_pushed(
                &name,
                Some(PushedTask {
                    task_id: "t".to_string(),
                    attempt: 1
                })
            )
            .is_err(),
        "no record, no push"
    );
    assert!(state.recorded_names().is_empty());
    state.write_session_record(&record()).expect("write");
    assert_eq!(state.read_session_record(&name), Some(record()));
    assert_eq!(state.recorded_names(), vec!["SVWL-claude".to_string()]);
    assert_eq!(mode(&state.root().join("sessions/SVWL-claude/session.json")), 0o600);
    let pushed = PushedTask {
        task_id: "t".to_string(),
        attempt: 2,
    };
    state.mark_pushed(&name, Some(pushed.clone())).expect("mark");
    assert_eq!(
        state.read_session_record(&name),
        Some(SessionRecord {
            pushed: Some(pushed),
            ..record()
        })
    );
    state.mark_pushed(&name, None).expect("clear");
    assert_eq!(state.read_session_record(&name), Some(record()));
    // The stored form names who started it, in words.
    let text = fs::read_to_string(state.root().join("sessions/SVWL-claude/session.json")).expect("read");
    assert!(text.contains("\"created_by\": \"operator\""), "{text}");
    assert!(!text.contains("started_at") && !text.contains("headless"), "{text}");
    // A record under a name that is not a derived session name is refused.
    let bad = SessionRecord {
        name: "SVWL".to_string(),
        ..record()
    };
    assert!(state.write_session_record(&bad).is_err());
}

/// `SessionStart` fires again after compaction, `/clear` and resume, and `Notification` also for an
/// idle prompt: the hooks are registered for the first start and for permission prompts only.
#[test]
fn session_start_and_notification_hooks_have_narrow_matchers() {
    let base = Dir::new();
    let state = StateDir::open(&base.state()).expect("open");
    let name = SessionName::new("SVWL", "claude").expect("name");
    let (exe, _server, socket) = launch_files(&state);
    let (json, settings) = (state.claude_mcp_json(BOT), state.claude_settings(&name));
    let files = LaunchFiles {
        claude_mcp_json: &json,
        claude_settings: &settings,
        runner_exe: &exe,
        hook_socket: &socket,
    };
    state.write_session_settings(&name, &files).expect("write");
    let value: serde_json::Value = serde_json::from_str(&fs::read_to_string(&settings).expect("read")).expect("json");
    assert_eq!(value["hooks"]["SessionStart"][0]["matcher"], "startup");
    assert_eq!(
        value["hooks"]["Notification"][0]["matcher"],
        "permission_prompt|elicitation_dialog"
    );
    assert!(value["hooks"]["Stop"][0].get("matcher").is_none());
    assert!(value["hooks"]["UserPromptSubmit"][0].get("matcher").is_none());
}
