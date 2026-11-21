use super::cli::{Row, RowState, attach_command, prunable, release_check, render_rows, trust_instructions};

fn row(state: RowState, in_registry: bool, headless: bool) -> Row {
    Row {
        key: "SVWL".to_string(),
        project_id: "22222222-2222-4222-8222-222222222222".to_string(),
        cli: "claude".to_string(),
        state,
        in_registry,
        headless,
        owned: true,
    }
}

#[test]
fn only_idle_sessions_that_are_unregistered_or_headless_are_pruned() {
    for state in [RowState::Idle, RowState::Busy, RowState::Dead, RowState::Gone] {
        for in_registry in [true, false] {
            for headless in [true, false] {
                let want = state == RowState::Idle && (!in_registry || headless);
                assert_eq!(
                    prunable(&row(state, in_registry, headless)),
                    want,
                    "{state:?} {in_registry} {headless}"
                );
            }
        }
    }
}

/// A session whose pane id or PID is not the recorded one is not the runner's: never pruned.
#[test]
fn a_session_that_is_not_the_recorded_pane_is_never_pruned() {
    let foreign = Row {
        owned: false,
        ..row(RowState::Idle, false, true)
    };
    assert!(!prunable(&foreign));
    assert!(prunable(&row(RowState::Idle, false, true)));
}

#[test]
fn a_key_is_released_only_when_bound_and_without_a_session() {
    assert!(release_check("SVWL", true, false).is_ok());
    assert!(
        release_check("SVWL", false, false)
            .unwrap_err()
            .contains("not reserved")
    );
    assert!(release_check("SVWL", true, true).unwrap_err().contains("still exists"));
    assert!(release_check("SVWL", false, true).is_err());
}

#[allow(clippy::indexing_slicing)]
#[test]
fn the_listing_shows_key_project_cli_and_state() {
    assert_eq!(render_rows(&[]), "no sessions created by this runner\n");
    let text = render_rows(&[row(RowState::Idle, true, false), row(RowState::Gone, false, true)]);
    let lines: Vec<&str> = text.lines().collect();
    assert_eq!(lines.len(), 3, "{text}");
    assert!(lines[0].starts_with("KEY") && lines[0].ends_with("STATE"));
    assert!(
        lines[1].contains("SVWL")
            && lines[1].contains("22222222-2222-4222-8222-222222222222")
            && lines[1].ends_with("idle")
    );
    assert!(lines[2].ends_with("gone (project now uses headless)"), "{text}");
}

#[test]
fn trust_tells_what_to_answer_and_how_to_attach() {
    let text = trust_instructions("SVWL", "claude", "sylvode");
    assert!(
        text.contains("Yes, I trust this folder") && text.contains("tmux -L sylvode attach -t =SVWL"),
        "{text}"
    );
    assert_eq!(attach_command("SVWL", ""), "tmux attach -t =SVWL");
    assert!(trust_instructions("K", "codex", "s").contains("trusts the directory"));
}
