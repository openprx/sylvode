use super::recovery::{Facts, Step, plan_task};

fn plan(phase: Option<&str>, status: &str, acknowledged: bool, alive: bool, hit: bool) -> Step {
    plan_with(phase, status, acknowledged, alive, hit, false)
}

#[allow(clippy::fn_params_excessive_bools, reason = "a table helper")]
fn plan_with(phase: Option<&str>, status: &str, acknowledged: bool, alive: bool, hit: bool, recorded: bool) -> Step {
    plan_task(&Facts {
        phase,
        status,
        acknowledged,
        session_alive: alive,
        scrollback_hit: hit,
        recorded_pushed: recorded,
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
        // acknowledged counts as pushed whatever a stale phase says (the phase report is
        // fire-and-forget and the acknowledge does not change it)
        (
            Some("queued"),
            "acknowledged",
            true,
            true,
            false,
            Step::Adopt { acknowledged: true },
        ),
        (Some("queued"), "acknowledged", true, false, false, Step::FailGone),
        // queued or unknown, nothing shows a push: queue again
        (Some("queued"), "processing", false, true, false, Step::Requeue),
        (Some("queued"), "processing", false, false, false, Step::Requeue),
        (None, "processing", false, true, false, Step::Requeue),
        (None, "processing", false, false, false, Step::Requeue),
        // the message is in the scrollback: it was pushed, whatever the phase says
        (
            None,
            "processing",
            false,
            true,
            true,
            Step::Adopt { acknowledged: false },
        ),
        (
            Some("queued"),
            "processing",
            false,
            true,
            true,
            Step::Adopt { acknowledged: false },
        ),
        // a hit without a live session cannot happen; it still must not adopt or re-queue wrongly
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

/// The runner's own record of a push (written before the paste) counts as pushed whatever Sylvode
/// shows: the task is supervised, or failed when the session is gone, and never typed again.
#[test]
fn a_recorded_push_is_never_typed_again() {
    for phase in [None, Some("queued"), Some("pushed")] {
        assert_eq!(
            plan_with(phase, "processing", false, true, false, true),
            Step::Adopt { acknowledged: false },
            "{phase:?}"
        );
        assert_eq!(
            plan_with(phase, "processing", false, false, false, true),
            Step::FailGone,
            "{phase:?}"
        );
    }
    assert_eq!(
        plan_with(Some("queued"), "processing", false, true, false, false),
        Step::Requeue
    );
}
