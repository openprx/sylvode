use std::cell::Cell;
use std::time::{Duration, Instant};

use sylvode_prereq::{Component, InstallKind, Mode, Status};

use super::doctor::{CACHE_FOR, Prereq, install_kind};

fn rows() -> Vec<Component> {
    vec![Component::new("tmux", true, Status::Ok, true)]
}

/// The prerequisites are probed at most once a minute: a second look inside the minute reuses the
/// first, and one after it probes again (so installing tmux takes effect without a restart).
#[test]
fn the_snapshot_is_reused_for_a_minute_and_then_probed_again() {
    let prereq = Prereq::default();
    let probes = Cell::new(0);
    let probe = || {
        probes.set(probes.get() + 1);
        rows()
    };
    let t0 = Instant::now();
    prereq.rows_at(t0, probe);
    prereq.rows_at(t0 + Duration::from_secs(59), probe);
    assert_eq!(probes.get(), 1);
    prereq.rows_at(t0 + CACHE_FOR, probe);
    assert_eq!(probes.get(), 2);
    prereq.rows_at(t0 + CACHE_FOR + Duration::from_secs(10), probe);
    assert_eq!(probes.get(), 2);
}

fn config(extra: &str) -> crate::config::Config {
    crate::config::Config::from_toml(&format!(
        "[server]\nlisten = \"127.0.0.1:1\"\n[security]\nallow_unsigned = true\n{extra}"
    ))
    .expect("config")
}

#[test]
fn the_install_kind_follows_the_mode_and_the_agents() {
    let plain = config("");
    assert_eq!(install_kind(Some(&plain), Mode::Tmux), InstallKind::RunnerTmux);
    assert_eq!(install_kind(Some(&plain), Mode::Headless), InstallKind::RunnerHeadless);
    assert_eq!(install_kind(Some(&plain), Mode::Auto), InstallKind::RunnerForwardOnly);
    // `auto` with runner agents may be asked for tmux sessions: the channel is chosen in Sylvode.
    let block = crate::config::Config::from_toml(include_str!("../../api/tests/fixtures/runner-config-block.toml"))
        .expect("block");
    assert_eq!(install_kind(Some(&block), Mode::Auto), InstallKind::RunnerTmux);
    assert_eq!(install_kind(None, Mode::Auto), InstallKind::RunnerForwardOnly);
}

/// A project that is a git worktree (`.git` is a file) cannot get its exclude line written; the
/// guide installer's warning shows in `doctor --project`.
#[test]
fn a_git_worktree_gets_the_not_excluded_warning() {
    let dir = std::env::temp_dir().join(format!("sylvode-doctor-wt-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&dir).expect("mkdir");
    assert!(super::doctor::worktree_warning(&dir).is_none(), "no git at all");
    std::fs::write(dir.join(".git"), "gitdir: /elsewhere/.git/worktrees/x\n").expect("file");
    let warning = super::doctor::worktree_warning(&dir).expect("a worktree");
    assert_eq!(warning.component, "agent_guide.git_exclude");
    assert_eq!(warning.status, Status::Warn);
    assert!(
        warning.hint.as_deref().unwrap_or_default().contains("git worktree"),
        "{warning:?}"
    );
    std::fs::remove_file(dir.join(".git")).expect("rm");
    std::fs::create_dir(dir.join(".git")).expect("git dir");
    assert!(super::doctor::worktree_warning(&dir).is_none(), "a plain checkout");
    let _ = std::fs::remove_dir_all(&dir);
}

fn temp_home() -> std::path::PathBuf {
    let home = std::env::temp_dir().join(format!("sylvode-trust-test-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&home).expect("home");
    home
}

/// claude's trust is read from `~/.claude.json`, per directory.
#[test]
fn claude_trust_is_read_from_the_dialog_flag_of_the_project() {
    use super::doctor::trust_state;
    use std::path::Path;
    use sylvode_prereq::Trusted;
    let home = temp_home();
    // No file, and a file without the project: unknown.
    assert_eq!(trust_state(&home, "claude", Path::new("/w/a")), Trusted::Unknown);
    std::fs::write(
        home.join(".claude.json"),
        r#"{"projects":{"/w/a":{"hasTrustDialogAccepted":true},"/w/b":{"hasTrustDialogAccepted":false},"/w/c":{"other":1}}}"#,
    )
    .expect("write");
    assert_eq!(trust_state(&home, "claude", Path::new("/w/a")), Trusted::Yes);
    assert_eq!(trust_state(&home, "claude", Path::new("/w/b")), Trusted::No);
    assert_eq!(trust_state(&home, "claude", Path::new("/w/c")), Trusted::Unknown);
    assert_eq!(trust_state(&home, "claude", Path::new("/w/missing")), Trusted::Unknown);
    // A file that is not JSON, and an unknown CLI, are unknown too; nothing is written.
    std::fs::write(home.join(".claude.json"), "not json").expect("write");
    assert_eq!(trust_state(&home, "claude", Path::new("/w/a")), Trusted::Unknown);
    assert_eq!(trust_state(&home, "opencode", Path::new("/w/a")), Trusted::Unknown);
    assert_eq!(
        std::fs::read_to_string(home.join(".claude.json")).expect("read"),
        "not json"
    );
    std::fs::remove_dir_all(&home).expect("clean");
}

/// codex's trust is `projects."<dir>".trust_level` in `~/.codex/config.toml`.
#[test]
fn codex_trust_is_read_from_the_trust_level_of_the_project() {
    use super::doctor::trust_state;
    use std::path::Path;
    use sylvode_prereq::Trusted;
    let home = temp_home();
    assert_eq!(trust_state(&home, "codex", Path::new("/w/a")), Trusted::Unknown);
    std::fs::create_dir_all(home.join(".codex")).expect("mkdir");
    std::fs::write(
        home.join(".codex/config.toml"),
        "model = \"x\"\n[projects.\"/w/a\"]\ntrust_level = \"trusted\"\n[projects.\"/w/b\"]\ntrust_level = \"untrusted\"\n[projects.\"/w/c\"]\nnote = 1\n",
    )
    .expect("write");
    assert_eq!(trust_state(&home, "codex", Path::new("/w/a")), Trusted::Yes);
    assert_eq!(trust_state(&home, "codex", Path::new("/w/b")), Trusted::No);
    assert_eq!(trust_state(&home, "codex", Path::new("/w/c")), Trusted::Unknown);
    assert_eq!(trust_state(&home, "codex", Path::new("/w/missing")), Trusted::Unknown);
    std::fs::write(home.join(".codex/config.toml"), "= broken").expect("write");
    assert_eq!(trust_state(&home, "codex", Path::new("/w/a")), Trusted::Unknown);
    // The claude file does not answer for codex.
    std::fs::write(
        home.join(".claude.json"),
        r#"{"projects":{"/w/a":{"hasTrustDialogAccepted":true}}}"#,
    )
    .expect("write");
    assert_eq!(trust_state(&home, "codex", Path::new("/w/a")), Trusted::Unknown);
    std::fs::remove_dir_all(&home).expect("clean");
}
