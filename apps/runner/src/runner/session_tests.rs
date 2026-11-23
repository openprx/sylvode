use super::files::{CreatedBy, SessionRecord};
use super::session::{
    EnsureDecision, EnsureFacts, admit_hook, decide_ensure, decline_for, dir_unchanged, recovered_blocked,
    report_needed, stable_exe, view_facts,
};
use super::tmux::{ListedPane, PaneId};

const BOT: &str = "00000000-0000-4000-8000-000000000001";
const OTHER_BOT: &str = "00000000-0000-4000-8000-000000000002";

fn names() -> Vec<String> {
    vec!["claude".to_string(), "claude.exe".to_string()]
}

fn pane(id: &str, pid: u32, command: &str, dead: bool) -> ListedPane {
    ListedPane {
        id: PaneId::new(id).expect("pane id"),
        pid,
        dead,
        command: command.to_string(),
    }
}

fn record(bot: &str, pane: &str, pid: u32, created_by: CreatedBy) -> SessionRecord {
    SessionRecord {
        name: "SVWL-claude".to_string(),
        project_id: "p".to_string(),
        bot_id: bot.to_string(),
        cli: "claude".to_string(),
        pane_id: pane.to_string(),
        pane_pid: pid,
        dir: "/x".to_string(),
        created_by,
        pushed: None,
    }
}

fn decide(panes: &[ListedPane], record: Option<&SessionRecord>) -> EnsureDecision {
    let n = names();
    decide_ensure(&EnsureFacts {
        session_exists: true,
        panes,
        process_names: &n,
        record,
        task_bot_id: BOT,
    })
}

/// E1: no session, no server: the runner creates it, whatever else is known.
#[test]
fn a_missing_session_is_created() {
    let n = names();
    let stale = record(BOT, "%1", 10, CreatedBy::Runner);
    for record in [None, Some(&stale)] {
        assert_eq!(
            decide_ensure(&EnsureFacts {
                session_exists: false,
                panes: &[],
                process_names: &n,
                record,
                task_bot_id: BOT,
            }),
            EnsureDecision::Create
        );
    }
}

/// E2: an existing session is used as it is, whoever started it: the one pane that runs the CLI.
#[test]
fn an_existing_session_is_used_through_its_one_cli_pane() {
    // The operator's: a shell window beside the CLI window does not matter.
    let panes = [pane("%0", 100, "zsh", false), pane("%1", 200, "claude", false)];
    assert_eq!(
        decide(&panes, None),
        EnsureDecision::Use {
            pane: panes[1].clone(),
            created_by: CreatedBy::Operator
        }
    );
    // Every configured process name counts.
    let panes = [pane("%3", 300, "claude.exe", false)];
    assert!(matches!(decide(&panes, None), EnsureDecision::Use { .. }));
    // The runner's own: the record names this pane and PID.
    let panes = [pane("%0", 100, "claude", false)];
    let own = record(BOT, "%0", 100, CreatedBy::Runner);
    assert_eq!(
        decide(&panes, Some(&own)),
        EnsureDecision::Use {
            pane: panes[0].clone(),
            created_by: CreatedBy::Runner
        }
    );
    // A record whose pane has another PID is stale: the pane is the operator's.
    let recycled = record(BOT, "%0", 999, CreatedBy::Runner);
    assert_eq!(
        decide(&panes, Some(&recycled)),
        EnsureDecision::Use {
            pane: panes[0].clone(),
            created_by: CreatedBy::Operator
        }
    );
    // A record that says the operator started it stays the operator's.
    let theirs = record(BOT, "%0", 100, CreatedBy::Operator);
    assert!(matches!(
        decide(&panes, Some(&theirs)),
        EnsureDecision::Use {
            created_by: CreatedBy::Operator,
            ..
        }
    ));
}

/// E3: a leftover shell, a dead pane or nothing: "CLI not running", retryable, nothing typed or
/// restarted. E2: two CLI panes are ambiguous, not retryable.
#[test]
fn no_cli_pane_and_two_cli_panes_are_failures_with_the_session_name() {
    for panes in [
        vec![pane("%0", 100, "zsh", false)],
        vec![pane("%0", 100, "bash", false), pane("%1", 101, "vim", false)],
        vec![pane("%0", 100, "claude", true)],
        vec![],
    ] {
        let decision = decide(&panes, None);
        assert_eq!(decision, EnsureDecision::NotRunning, "{panes:?}");
        let (text, retryable) = decline_for(&decision, "SVWL-claude").expect("failure");
        assert!(retryable);
        assert!(
            text.contains("not running in tmux session SVWL-claude")
                && text.contains("tmux attach -t SVWL-claude")
                && !text.contains('/'),
            "{text}"
        );
    }
    let two = [pane("%0", 100, "claude", false), pane("%1", 101, "claude", false)];
    let decision = decide(&two, None);
    assert_eq!(decision, EnsureDecision::Ambiguous);
    let (text, retryable) = decline_for(&decision, "SVWL-claude").expect("failure");
    assert!(
        !retryable && text.contains("more than one pane running the CLI"),
        "{text}"
    );
    // A dead second CLI pane does not make it ambiguous.
    let one_dead = [pane("%0", 100, "claude", false), pane("%1", 101, "claude", true)];
    assert!(matches!(decide(&one_dead, None), EnsureDecision::Use { .. }));
}

/// E4 and T4: nothing about the directory or CLI configuration enters the decision (the fact set
/// has no such field), and a live session recorded for another bot is refused before anything is
/// typed.
#[test]
fn a_session_recorded_for_another_bot_is_refused() {
    let panes = [pane("%0", 100, "claude", false)];
    let other = record(OTHER_BOT, "%0", 100, CreatedBy::Runner);
    let decision = decide(&panes, Some(&other));
    assert_eq!(decision, EnsureDecision::BotMismatch);
    let (text, retryable) = decline_for(&decision, "SVWL-claude").expect("failure");
    assert!(!retryable && text.starts_with("session_bot_mismatch"), "{text}");
    // The same record with a stale pane says nothing about who serves the session now.
    let stale = record(OTHER_BOT, "%0", 999, CreatedBy::Runner);
    assert!(matches!(decide(&panes, Some(&stale)), EnsureDecision::Use { .. }));
    // A record without a bot (never written by this version) never blocks.
    let blank = record("", "%0", 100, CreatedBy::Runner);
    assert!(matches!(decide(&panes, Some(&blank)), EnsureDecision::Use { .. }));
    // The same bot is served.
    let mine = record(BOT, "%0", 100, CreatedBy::Runner);
    assert!(matches!(decide(&panes, Some(&mine)), EnsureDecision::Use { .. }));
    // Usable decisions carry no failure text.
    assert!(decline_for(&EnsureDecision::Create, "SVWL-claude").is_none());
}

#[test]
fn a_task_view_says_whether_it_is_active_and_acknowledged() {
    let active = view_facts(&serde_json::json!({"active": true, "status": "queued"}));
    assert!(active.active && !active.acknowledged);
    let acked = view_facts(&serde_json::json!({"status": "acknowledged"}));
    assert!(acked.active && acked.acknowledged);
    let done = view_facts(&serde_json::json!({"active": false}));
    assert!(!done.active);
}

/// After an in-place upgrade on Linux `current_exe` says `<path> (deleted)`; hooks must name the
/// path itself, which holds the new binary, and a path that does not exist is not used.
#[test]
fn the_executable_path_survives_an_in_place_upgrade() {
    let dir = std::env::temp_dir().join(format!("sylvode-exe-test-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&dir).expect("mkdir");
    let exe = dir.join("sylvode-runner");
    std::fs::write(&exe, "x").expect("write");
    assert_eq!(stable_exe(&exe), Some(exe.clone()));
    let deleted = std::path::PathBuf::from(format!("{} (deleted)", exe.display()));
    assert_eq!(stable_exe(&deleted), Some(exe.clone()));
    std::fs::remove_file(&exe).expect("remove");
    assert_eq!(stable_exe(&deleted), None);
    assert_eq!(stable_exe(&exe), None);
    let _ = std::fs::remove_dir_all(&dir);
}

/// A hook message is dropped unless, where the platform names the sender, it comes from below the
/// session's pane process; an unknown pane is a refusal. (The pane pair was checked before.)
#[test]
fn a_hook_message_from_outside_the_pane_is_dropped() {
    let pane = 4000;
    assert_eq!(admit_hook(Some(&[9, 8, pane, 1]), Some(pane)), Ok(()));
    // A sender outside the pane's process tree.
    assert_eq!(admit_hook(Some(&[9, 8, 7, 1]), Some(pane)), Err("not from the pane"));
    // Fail closed: a chain was read but the pane is not known (session.json unreadable).
    assert_eq!(admit_hook(Some(&[9, pane]), None), Err("not from the pane"));
    // A platform that cannot name the sender leaves the pane pair to decide.
    assert_eq!(admit_hook(None, None), Ok(()));
}

/// The directory accepted at delivery must still be the same canonical path before a tmux session
/// is created or restarted: a link swapped in meanwhile, or a vanished directory, is not.
#[test]
fn the_accepted_directory_is_rechecked_before_a_session_starts() {
    let base = std::env::temp_dir().join(format!("sylvode-dir-test-{}", uuid::Uuid::new_v4()));
    let real = base.join("project");
    std::fs::create_dir_all(&real).expect("mkdir");
    let canonical = real.canonicalize().expect("canonical");
    assert!(dir_unchanged(&canonical));
    // The directory is replaced by a link to somewhere else.
    let elsewhere = base.join("elsewhere");
    std::fs::create_dir_all(&elsewhere).expect("mkdir");
    std::fs::remove_dir_all(&real).expect("remove");
    std::os::unix::fs::symlink(&elsewhere, &real).expect("link");
    assert!(!dir_unchanged(&canonical), "a swapped link is caught");
    // And a directory that is gone.
    std::fs::remove_file(&real).expect("unlink");
    assert!(!dir_unchanged(&canonical));
    let _ = std::fs::remove_dir_all(&base);
}

/// A `blocked` phase is stale after the dialog was answered: it counts only with a dialog on the
/// screen.
#[test]
fn a_stale_blocked_phase_does_not_block_a_recovered_task() {
    assert!(recovered_blocked(true, true));
    assert!(!recovered_blocked(true, false), "stale phase");
    assert!(!recovered_blocked(false, true));
    assert!(!recovered_blocked(false, false));
}

/// An out-of-cycle report is asked for when a session is new, its pane or process changed, or it has
/// not asked since the runner started; never for the same pane again once it has asked.
#[test]
fn report_needed_on_first_record_new_pane_and_first_since_start() {
    let before = record(BOT, "%1", 10, CreatedBy::Runner);
    let same = record(BOT, "%1", 10, CreatedBy::Runner);
    let new_pane = record(BOT, "%2", 10, CreatedBy::Runner);
    let new_pid = record(BOT, "%1", 11, CreatedBy::Runner);
    for (label, previous, new, requested, expected) in [
        ("first record", None, &same, true, true),
        ("first record, nothing requested", None, &same, false, true),
        ("a new pane", Some(&before), &new_pane, true, true),
        ("a new process in the same pane", Some(&before), &new_pid, true, true),
        (
            "first delivery since start, same pane",
            Some(&before),
            &same,
            false,
            true,
        ),
        (
            "same pane, already requested since start",
            Some(&before),
            &same,
            true,
            false,
        ),
    ] {
        assert_eq!(report_needed(previous, new, requested), expected, "{label}");
    }
}

/// A codex turn end closes the turn only on a screen that shows the idle, empty composer.
#[test]
fn a_codex_turn_end_is_confirmed_only_by_an_idle_empty_composer() {
    use super::session::turn_end_confirmed;
    let idle = include_str!("fixtures/codex-after-turn.ansi");
    let busy = include_str!("fixtures/codex-busy.ansi");
    let draft = include_str!("fixtures/codex-draft.ansi");
    let pasted = include_str!("fixtures/codex-pasted.ansi");
    let approval = include_str!("fixtures/codex-permission.ansi");
    assert!(turn_end_confirmed("codex", idle));
    for (name, screen) in [
        ("busy", busy),
        ("draft", draft),
        ("pasted", pasted),
        ("approval", approval),
    ] {
        assert!(!turn_end_confirmed("codex", screen), "{name}");
    }
    assert!(!turn_end_confirmed("codex", "no prompt on this screen"));
}
