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
    // `auto` with runner agents follows `[runner.tmux] enabled`.
    let off = crate::config::Config::from_toml(include_str!("../../api/tests/fixtures/runner-config-block.toml"))
        .expect("block");
    assert!(!off.runner.tmux.enabled);
    assert_eq!(install_kind(Some(&off), Mode::Auto), InstallKind::RunnerHeadless);
    let mut on = off.clone();
    on.runner.tmux.enabled = true;
    assert_eq!(install_kind(Some(&on), Mode::Auto), InstallKind::RunnerTmux);
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
