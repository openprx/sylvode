use super::detect::{
    Dialog, awaits_choice, bottom, classify_bottom, classify_dialog, cut_for_log, input_is_empty, input_text,
    looks_busy, screens_equal,
};

/// The confirmation screen of claude's bypass-permissions mode, from `fixtures/` (reconstructed from
/// the text the CLI shows; the capture of hand check A5 replaces it).
const BYPASS_CONFIRMATION: &str = include_str!("fixtures/claude-bypass-confirmation.txt");

#[test]
fn dialogs_are_recognised_by_the_verified_texts() {
    for (cli, screen, want) in [
        (
            "claude",
            "Is this a project you created or one you trust? (Like your own code)\n 1. Yes, I trust this folder",
            Some(Dialog::Trust),
        ),
        ("claude", "  Yes, I trust this folder\n  No, exit", Some(Dialog::Trust)),
        (
            "claude",
            "Select login method:\n 1. Claude account",
            Some(Dialog::NotLoggedIn),
        ),
        ("claude", "Please run /login", Some(Dialog::NotLoggedIn)),
        // The bypass-permissions confirmation the default launch command meets on its first start.
        ("claude", BYPASS_CONFIRMATION, Some(Dialog::BypassConfirm)),
        ("claude", "In Bypass Permissions mode, nothing asks", None),
        ("codex", BYPASS_CONFIRMATION, None),
        ("claude", "> ", None),
        ("claude", "", None),
        (
            "codex",
            "You've hit your usage limit. Try again later.",
            Some(Dialog::UsageLimit),
        ),
        (
            "codex",
            "choose a model\n  enter select \u{b7} esc back",
            Some(Dialog::Chooser),
        ),
        ("codex", "\u{203a} ", None),
        // A text of one CLI does not trigger another's dialog.
        ("codex", "Yes, I trust this folder", None),
        ("claude", "enter select \u{b7} esc back", None),
        ("opencode", "Yes, I trust this folder", None),
    ] {
        assert_eq!(classify_dialog(cli, screen), want, "{cli}: {screen:?}");
    }
    let reason = Dialog::Trust.reason("SVWL-claude");
    assert!(
        reason.contains("tmux attach -t SVWL-claude") && !reason.contains("trust SVWL") && !reason.contains('/'),
        "{reason}"
    );
    // The confirmation is a dialog at the bottom of a screen too, and an answered one is gone.
    assert_eq!(
        classify_bottom("claude", BYPASS_CONFIRMATION),
        Some(Dialog::BypassConfirm)
    );
    assert_eq!(classify_bottom("claude", "done\n> "), None);
}

#[test]
fn the_input_line_is_empty_only_when_the_prompt_is_there_and_holds_nothing() {
    for (cli, screen, want) in [
        (
            "claude",
            "welcome\n\u{256d}----\u{256e}\n\u{2502} > \u{2502}\n\u{2570}----\u{256f}",
            Some(true),
        ),
        ("claude", "> ", Some(true)),
        ("claude", "output\n>", Some(true)),
        ("claude", "output\n> half typed", Some(false)),
        ("claude", "\u{2502} > fix the thing \u{2502}", Some(false)),
        ("claude", "\u{276f} ", Some(true)),
        ("claude", "no prompt on this screen", None),
        ("claude", "", None),
        // The last prompt line decides, not an earlier one.
        ("claude", "> old draft\nresult\n> ", Some(true)),
        ("claude", "> \nresult\n> typing", Some(false)),
        ("codex", "\u{203a} ", Some(true)),
        ("codex", "\u{203a} run tests", Some(false)),
        ("codex", "> ", None),
    ] {
        assert_eq!(input_is_empty(cli, screen), want, "{cli}: {screen:?}");
    }
}

#[test]
fn stable_screens_ignore_trailing_blanks() {
    assert!(screens_equal("a  \nb\n\n", "a\nb"));
    assert!(screens_equal("", "\n\n"));
    assert!(!screens_equal("a\nb", "a\nc"));
    assert!(!screens_equal("a", "a\nb"));
}

#[test]
fn logged_screens_are_cut() {
    assert_eq!(cut_for_log(&"x".repeat(1000)).len(), 400);
    assert_eq!(cut_for_log("short"), "short");
    assert_eq!(cut_for_log(&"é".repeat(500)).chars().count(), 400);
}

#[test]
fn the_input_text_is_what_follows_the_last_prompt_glyph() {
    assert_eq!(
        input_text("claude", "out\n> [Sylvode task] id=1 attempt=1/3\nIssue: x\n").as_deref(),
        Some("[Sylvode task] id=1 attempt=1/3")
    );
    assert_eq!(
        input_text("claude", "\u{2502} > half typed \u{2502}").as_deref(),
        Some("half typed")
    );
    assert_eq!(input_text("claude", "> ").as_deref(), Some(""));
    assert_eq!(input_text("claude", "no prompt"), None);
    assert_eq!(input_text("codex", "\u{203a} run tests").as_deref(), Some("run tests"));
}

const MARKERS: [&str; 6] = [
    "esc to interrupt",
    "Esc to cancel",
    "Enter to select",
    "\u{276f} 1. Yes",
    "Please run /login",
    "Yes, I trust this folder",
];

/// Text a task or the agent put on the screen (a title echoed in the pasted message, a reply) never
/// counts as the CLI's busy marker, choice or dialog: those are read from the CLI's own chrome at
/// the bottom only. An idle screen whose transcript holds every marker is idle.
#[test]
fn markers_in_the_transcript_do_not_count() {
    for marker in MARKERS {
        let screen = format!(
            "> [Sylvode task] id=1 attempt=1/3\nIssue: SVWL-7, title (untrusted, quoted from the work item): \"Build hangs; {marker} does nothing\"\n\
             Reason: assigned to you\n\u{25cf} Done. The build was stuck; {marker} was shown.\n(esc to interrupt)\n\
             \n  a line of the reply\n  another\n  and another\n\u{256d}----\u{256e}\n\u{2502} > \u{2502}\n\u{2570}----\u{256f}\n  ? for shortcuts"
        );
        assert!(!looks_busy("claude", &screen), "busy from: {marker}");
        assert!(!awaits_choice("claude", &screen), "choice from: {marker}");
        assert_eq!(classify_bottom("claude", &screen), None, "dialog from: {marker}");
        assert_eq!(input_is_empty("claude", &screen), Some(true));
    }
}

/// What the CLI itself draws at the bottom still counts.
#[test]
fn the_clis_own_chrome_counts() {
    let busy = "some transcript\n\u{273b} Thinking\u{2026} (12s \u{b7} esc to interrupt)\n\u{256d}----\u{256e}\n\u{2502} > \u{2502}\n\u{2570}----\u{256f}";
    assert!(looks_busy("claude", busy));
    assert_eq!(
        input_is_empty("claude", busy),
        Some(true),
        "an empty box says nothing about work"
    );
    let dialog = "transcript\nDo you want to proceed?\n \u{276f} 1. Yes\n   2. No\n Esc to cancel";
    assert!(awaits_choice("claude", dialog));
    let elicitation = "Which option?\n \u{276f} 1. Alpha\n   2. Beta\n Enter to select \u{b7} Esc to cancel";
    assert!(awaits_choice("claude", elicitation));
    let trust = "old lines\nIs this a project you created or one you trust?\n  Yes, I trust this folder";
    assert_eq!(classify_bottom("claude", trust), Some(Dialog::Trust));
    // A spinner needs the glyph, the text and the parenthesised part, ending the line.
    for not_spinner in [
        "esc to interrupt",
        "(esc to interrupt)",
        "- working (esc to interrupt) now",
        "> (esc to interrupt)",
    ] {
        let screen = format!("{not_spinner}\n> ");
        assert!(!looks_busy("claude", &screen), "{not_spinner}");
    }
}

#[test]
fn the_bottom_is_the_last_non_blank_lines() {
    assert_eq!(bottom("a\n\nb\n\n\nc\n\n", 2), "b\nc");
    assert_eq!(bottom("", 3), "");
}
