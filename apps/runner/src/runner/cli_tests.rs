use super::cli::{Row, RowState, live_state, release_check, render_rows};
use super::files::CreatedBy;

fn row(name: &str, state: RowState, created_by: CreatedBy) -> Row {
    Row {
        name: name.to_string(),
        tmux: name.to_string(),
        project_id: "22222222-2222-4222-8222-222222222222".to_string(),
        cli: "claude".to_string(),
        state,
        created_by,
    }
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
fn the_listing_shows_name_project_cli_state_and_who_started_it() {
    assert_eq!(render_rows(&[]), "no sessions recorded by this runner\n");
    let mut named = row("SVWL-codex", RowState::Idle, CreatedBy::Operator);
    named.tmux = "lead".to_string();
    let listed = render_rows(&[named]);
    let line = listed.lines().nth(1).expect("a row");
    assert_eq!(
        &line.split_whitespace().take(2).collect::<Vec<_>>(),
        &["SVWL-codex", "lead"],
        "{listed}"
    );
    assert!(
        listed
            .lines()
            .next()
            .is_some_and(|h| h.starts_with("NAME") && h.contains("TMUX SESSION"))
    );
    let text = render_rows(&[
        row("SVWL-claude", RowState::Idle, CreatedBy::Runner),
        row("QLA-claude", RowState::Gone, CreatedBy::Operator),
    ]);
    let lines: Vec<&str> = text.lines().collect();
    assert_eq!(lines.len(), 3, "{text}");
    assert!(lines[0].starts_with("NAME") && lines[0].ends_with("STARTED BY"));
    assert!(
        lines[1].contains("SVWL-claude")
            && lines[1].contains("22222222-2222-4222-8222-222222222222")
            && lines[1].contains("idle")
            && lines[1].ends_with("runner"),
        "{text}"
    );
    assert!(lines[2].contains("gone") && lines[2].ends_with("operator"), "{text}");
}

/// A CLI that is working shows an empty input box; it is not idle.
#[test]
fn a_working_cli_is_never_idle() {
    let working = "\u{273b} Thinking\u{2026} (12s \u{b7} esc to interrupt)\n\u{256d}----\u{256e}\n\u{2502} > \u{2502}\n\u{2570}----\u{256f}";
    let idle = "done\n\u{256d}----\u{256e}\n\u{2502} > \u{2502}\n\u{2570}----\u{256f}";
    let dialog = "Do you want to proceed?\n \u{276f} 1. Yes\n   2. No\n Esc to cancel";
    let draft = "> half written";
    assert_eq!(live_state("claude", idle), RowState::Idle);
    for (name, screen) in [
        ("working", working),
        ("dialog", dialog),
        ("draft", draft),
        ("no prompt", "text"),
    ] {
        assert_eq!(live_state("claude", screen), RowState::Busy, "{name}");
    }
}
