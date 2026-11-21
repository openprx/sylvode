use std::cell::Cell;
use std::time::{Duration, Instant};

use sylvode_prereq::{Component, InstallKind, Mode, Status};

use super::doctor::{CACHE_FOR, Prereq, health_parts, install_kind};

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
    assert_eq!(install_kind(None, Mode::Auto), InstallKind::RunnerForwardOnly);
}

/// `/health` says `degraded` only when a required row failed while runner agents are configured.
#[test]
fn degraded_needs_a_runner_agent_and_a_failed_required_row() {
    let failed_required = vec![Component::new("tmux", false, Status::Fail, true)];
    let failed_optional = vec![Component::new("cli.codex", false, Status::Fail, false)];
    let plain = config("");
    let (_, degraded) = health_parts(&plain, &failed_required);
    assert!(!degraded, "no runner agent: nothing to degrade");
    let runner = crate::config::Config::from_toml(include_str!("../../api/tests/fixtures/runner-config-block.toml"))
        .expect("block");
    let (prereq, degraded) = health_parts(&runner, &failed_required);
    assert!(degraded);
    assert_eq!(prereq.get("tmux").and_then(|v| v.as_str()), Some("fail"));
    assert!(!health_parts(&runner, &failed_optional).1);
    assert!(!health_parts(&runner, &rows()).1);
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
