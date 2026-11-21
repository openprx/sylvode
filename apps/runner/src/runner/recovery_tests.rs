use super::recovery::{Facts, Step, plan_task};

fn plan(phase: Option<&str>, status: &str, acknowledged: bool, alive: bool, hit: bool) -> Step {
    plan_task(&Facts {
        phase,
        status,
        acknowledged,
        session_alive: alive,
        scrollback_hit: hit,
    })
}

#[test]
fn the_restart_table() {
    // (phase, status, acknowledged_at set, session alive, scrollback hit) -> step
    let table = [
        // pushed: adopt when the session lives, fail when it is gone
        (
            Some("pushed"),
            "processing",
            false,
            true,
            false,
            Step::Adopt { acknowledged: false },
        ),
        (
            Some("pushed"),
            "acknowledged",
            true,
            true,
            false,
            Step::Adopt { acknowledged: true },
        ),
        (Some("pushed"), "processing", false, false, false, Step::FailGone),
        (Some("pushed"), "acknowledged", true, false, false, Step::FailGone),
        // acknowledged with an unknown phase was pushed
        (
            None,
            "acknowledged",
            true,
            true,
            false,
            Step::Adopt { acknowledged: true },
        ),
        (None, "acknowledged", true, false, false, Step::FailGone),
        (Some("queued"), "acknowledged", true, true, false, Step::Requeue),
        // queued or unknown: never pushed, queue again
        (Some("queued"), "processing", false, true, false, Step::Requeue),
        (Some("queued"), "processing", false, false, false, Step::Requeue),
        (None, "processing", false, true, false, Step::Requeue),
        (None, "processing", false, false, false, Step::Requeue),
        // unknown phase, but the message is in the scrollback: it was pushed
        (
            None,
            "processing",
            false,
            true,
            true,
            Step::Adopt { acknowledged: false },
        ),
        // a hit without a live session cannot happen; it still must not adopt
        (None, "processing", false, false, true, Step::Requeue),
        // blocked
        (Some("blocked"), "processing", false, true, false, Step::Blocked),
        (Some("blocked"), "processing", false, false, false, Step::FailGone),
        // a phase this runner does not know is left alone
        (Some("future"), "processing", false, true, false, Step::Skip),
    ];
    for (phase, status, acked, alive, hit, want) in table {
        assert_eq!(
            plan(phase, status, acked, alive, hit),
            want,
            "{phase:?} {status} {acked} {alive} {hit}"
        );
    }
}
