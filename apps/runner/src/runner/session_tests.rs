use super::files::{CreatedBy, SessionRecord};
use super::machine::State;
use super::session::{
    AFFINITY_CAP, AffinityStore, EnsureDecision, EnsureFacts, MemberStatus, SelectInput, SelectReason,
    TASK_AFFINITY_KEEP_SECS, admit_hook, decide_ensure, decline_for, dir_unchanged, recovered_blocked, report_needed,
    select_member, stable_exe, view_facts,
};
use super::tmux::{ListedPane, NameSource, PaneId};
use std::time::Instant;

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
        tmux_name: "SVWL-claude".to_string(),
        name_source: "derived".to_string(),
    }
}

fn decide(panes: &[ListedPane], record: Option<&SessionRecord>) -> EnsureDecision {
    let n = names();
    decide_ensure(&EnsureFacts {
        session_exists: true,
        panes,
        process_names: &n,
        allow_create: true,
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
                allow_create: true,
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
        let (text, retryable) = decline_for(&decision, "SVWL-claude", "claude", NameSource::Derived).expect("failure");
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
    let (text, retryable) = decline_for(&decision, "SVWL-claude", "claude", NameSource::Derived).expect("failure");
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
    let (text, retryable) = decline_for(&decision, "SVWL-claude", "claude", NameSource::Derived).expect("failure");
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
    assert!(decline_for(&EnsureDecision::Create, "SVWL-claude", "claude", NameSource::Derived).is_none());
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

/// A session with a configured name is never created: when it is absent the decision is `Absent`,
/// whatever the record says, and the text says how to start it. Only a derived name is created.
#[test]
fn an_absent_configured_session_is_declined_and_only_a_derived_one_is_created() {
    let n = names();
    let stale = record(BOT, "%1", 10, CreatedBy::Runner);
    for record in [None, Some(&stale)] {
        let facts = |allow_create| EnsureFacts {
            session_exists: false,
            panes: &[],
            process_names: &n,
            allow_create,
            record,
            task_bot_id: BOT,
        };
        assert_eq!(decide_ensure(&facts(true)), EnsureDecision::Create);
        assert_eq!(decide_ensure(&facts(false)), EnsureDecision::Absent);
    }
    let (text, retryable) = decline_for(&EnsureDecision::Absent, "sv", "claude", NameSource::Local).expect("failure");
    assert!(retryable);
    assert_eq!(
        text,
        "tmux session sv does not exist; start it and run claude in it (tmux new-session -s sv); \
         the runner does not create sessions with a configured name"
    );
    // An existing configured session is decided like any other.
    let panes = [pane("%0", 100, "zsh", false), pane("%1", 200, "claude", false)];
    assert!(matches!(decide(&panes, None), EnsureDecision::Use { .. }));
}

/// The offer to end the session so the runner starts it exists only for a derived name.
#[test]
fn only_a_derived_session_is_offered_to_the_runner_to_start() {
    let derived = decline_for(
        &EnsureDecision::NotRunning,
        "SVWL-claude",
        "claude",
        NameSource::Derived,
    )
    .expect("text");
    assert!(
        derived.0.contains("end the session so the runner starts it") && derived.1,
        "{derived:?}"
    );
    let (text, retryable) = decline_for(&EnsureDecision::NotRunning, "sv", "claude", NameSource::Local).expect("text");
    assert!(retryable);
    assert!(
        text.contains("not running in tmux session sv; attach with tmux attach -t sv and start it")
            && !text.contains("end the session"),
        "{text}"
    );
}

// ----- select_member -------------------------------------------------------------------------

const NOW: u64 = 1_000_000;
const TTL: u64 = 3600;

fn status(state: State, queue_len: usize) -> MemberStatus {
    MemberStatus {
        state,
        queue_len,
        healthy: true,
        unhealthy_since: None,
    }
}

fn idle() -> MemberStatus {
    status(State::Idle, 0)
}

fn busy(queued: usize) -> MemberStatus {
    status(State::Busy, queued)
}

fn dead() -> MemberStatus {
    MemberStatus {
        state: State::Dead,
        queue_len: 0,
        healthy: false,
        unhealthy_since: Some(Instant::now()),
    }
}

fn pick(
    task: Option<&str>,
    item: Option<&str>,
    affinity: &AffinityStore,
    statuses: &[MemberStatus],
) -> (usize, SelectReason) {
    select_member(&SelectInput { task_id: task, item }, affinity, statuses, NOW, TTL)
}

#[test]
fn a_pool_of_one_always_picks_member_zero() {
    let affinity = AffinityStore::default();
    for member in [idle(), busy(0), busy(5), dead(), status(State::NoSession, 0)] {
        let (index, _) = pick(Some("t"), Some("i"), &affinity, &[member]);
        assert_eq!(index, 0, "{member:?}");
    }
    assert_eq!(pick(Some("t"), None, &affinity, &[idle()]).1, SelectReason::Idle);
    assert_eq!(pick(Some("t"), None, &affinity, &[busy(3)]).1, SelectReason::Shortest);
    assert_eq!(pick(Some("t"), None, &affinity, &[dead()]).1, SelectReason::Fallback);
}

#[test]
fn no_members_fall_back_to_the_primary() {
    assert_eq!(
        pick(Some("t"), None, &AffinityStore::default(), &[]),
        (0, SelectReason::Fallback)
    );
}

#[test]
fn every_pool_size_follows_the_rules_in_order() {
    for n in 1..=8usize {
        let affinity = AffinityStore::default();
        // Everyone idle: the lowest index.
        let all_idle = vec![idle(); n];
        assert_eq!(
            pick(Some("t"), None, &affinity, &all_idle),
            (0, SelectReason::Idle),
            "n={n}"
        );
        // Everyone busy with equal load: the lowest index by the shortest-queue tie.
        let all_busy = vec![busy(2); n];
        assert_eq!(
            pick(Some("t"), None, &affinity, &all_busy),
            (0, SelectReason::Shortest),
            "n={n}"
        );
        // The last member is the only idle one.
        let mut last_idle = vec![busy(0); n];
        last_idle[n - 1] = idle();
        let expect = if n == 1 {
            (0, SelectReason::Idle)
        } else {
            (n - 1, SelectReason::Idle)
        };
        assert_eq!(pick(Some("t"), None, &affinity, &last_idle), expect, "n={n}");
        // The last member has the shortest queue; the others are longer.
        let mut shortest_last: Vec<MemberStatus> = (0..n).map(|i| busy(5 + i)).collect();
        shortest_last[n - 1] = busy(1);
        assert_eq!(
            pick(Some("t"), None, &affinity, &shortest_last),
            (n - 1, SelectReason::Shortest),
            "n={n}"
        );
        // Nobody is healthy: the primary.
        let all_dead = vec![dead(); n];
        assert_eq!(
            pick(Some("t"), None, &affinity, &all_dead),
            (0, SelectReason::Fallback),
            "n={n}"
        );
        // Only the last member is healthy: it takes the task even when busy.
        let mut only_last = vec![dead(); n];
        only_last[n - 1] = busy(4);
        assert_eq!(
            pick(Some("t"), None, &affinity, &only_last),
            (n - 1, SelectReason::Shortest),
            "n={n}"
        );
    }
}

#[test]
fn the_load_counts_the_current_task_and_the_queue() {
    // Member 0 holds one queued task and none running: load 1. Member 1 runs one task: load 1.
    // Member 2 runs one task and has one queued: load 2. The tie goes to member 0.
    let statuses = [status(State::Idle, 1), busy(0), busy(1)];
    assert_eq!(
        pick(Some("t"), None, &AffinityStore::default(), &statuses),
        (0, SelectReason::Shortest)
    );
    // A member with a queue is not idle even when its state says so.
    assert_eq!(
        pick(None, None, &AffinityStore::default(), &[status(State::Idle, 2), idle()]),
        (1, SelectReason::Idle)
    );
}

#[test]
fn task_affinity_wins_while_the_member_is_healthy() {
    let mut affinity = AffinityStore::default();
    affinity.record_task("t1", 2, NOW - 10);
    let statuses = [idle(), idle(), busy(9)];
    assert_eq!(
        pick(Some("t1"), None, &affinity, &statuses),
        (2, SelectReason::AffinityTask)
    );
    // Another task does not use it.
    assert_eq!(pick(Some("t2"), None, &affinity, &statuses), (0, SelectReason::Idle));
    // Moving a task (no task id) skips task affinity.
    assert_eq!(pick(None, None, &affinity, &statuses), (0, SelectReason::Idle));
    // An unhealthy affinity member is skipped: the rules go on with idle.
    let statuses = [busy(1), idle(), dead()];
    assert_eq!(pick(Some("t1"), None, &affinity, &statuses), (1, SelectReason::Idle));
    // An affinity member that is no longer in the pool is skipped as well.
    assert_eq!(
        pick(Some("t1"), None, &affinity, &[busy(1), busy(0)]),
        (1, SelectReason::Shortest)
    );
}

#[test]
fn task_affinity_beats_item_affinity_and_expires_a_day_after_the_task_left() {
    let mut affinity = AffinityStore::default();
    affinity.record_task("t1", 1, NOW - 100);
    affinity.record_item("W-1", 2, NOW - 50);
    let statuses = [idle(), busy(3), busy(3)];
    assert_eq!(
        pick(Some("t1"), Some("W-1"), &affinity, &statuses),
        (1, SelectReason::AffinityTask)
    );
    affinity.task_left("t1", NOW - 100);
    assert_eq!(
        pick(Some("t1"), Some("W-1"), &affinity, &statuses),
        (1, SelectReason::AffinityTask),
        "still remembered"
    );
    let later = NOW + TASK_AFFINITY_KEEP_SECS;
    assert_eq!(affinity.task_member("t1", later), None);
    assert_eq!(affinity.task_member("t1", later - 100), Some(1));
}

#[test]
fn item_affinity_holds_within_the_ttl_only() {
    let mut affinity = AffinityStore::default();
    affinity.record_item("W-1", 1, NOW - TTL);
    let statuses = [idle(), busy(3)];
    assert_eq!(
        pick(Some("t"), Some("W-1"), &affinity, &statuses),
        (1, SelectReason::AffinityItem)
    );
    // One second past the ttl.
    let expired = select_member(
        &SelectInput {
            task_id: Some("t"),
            item: Some("W-1"),
        },
        &affinity,
        &statuses,
        NOW + 1,
        TTL,
    );
    assert_eq!(expired, (0, SelectReason::Idle));
    // Another item has no affinity; an unhealthy member is skipped.
    assert_eq!(
        pick(Some("t"), Some("W-2"), &affinity, &statuses),
        (0, SelectReason::Idle)
    );
    assert_eq!(
        pick(Some("t"), Some("W-1"), &affinity, &[idle(), dead()]),
        (0, SelectReason::Idle)
    );
}

#[test]
fn the_affinity_store_is_capped_and_round_trips_through_json() {
    let mut affinity = AffinityStore::default();
    for i in 0..(AFFINITY_CAP + 10) {
        affinity.record_task(&format!("t{i}"), 1, NOW + i as u64);
        affinity.record_item(&format!("W-{i}"), 1, NOW + i as u64);
    }
    assert_eq!(affinity.task_count(), AFFINITY_CAP);
    assert_eq!(affinity.task_member("t0", NOW), None, "the oldest went first");
    let newest = format!("t{}", AFFINITY_CAP + 9);
    assert_eq!(affinity.task_member(&newest, NOW), Some(1));
    assert_eq!(affinity.item_member("W-0", NOW, u64::MAX), None);
    let text = serde_json::to_string(&affinity).expect("serialise");
    let back: AffinityStore = serde_json::from_str(&text).expect("deserialise");
    assert_eq!(back, affinity);
}

#[test]
fn the_status_keeps_its_unhealthy_time_until_the_member_recovers() {
    let t0 = Instant::now();
    let later = t0 + std::time::Duration::from_secs(30);
    let down = MemberStatus::fresh().next(State::Dead, 0, t0);
    assert!(!down.healthy);
    assert_eq!(down.unhealthy_since, Some(t0));
    let still = down.next(State::Dead, 1, later);
    assert_eq!(still.unhealthy_since, Some(t0));
    let up = still.next(State::Idle, 0, later);
    assert!(up.healthy);
    assert_eq!(up.unhealthy_since, None);
    assert!(MemberStatus::fresh().healthy);
}
