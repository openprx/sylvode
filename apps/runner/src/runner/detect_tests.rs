use super::detect::{Dialog, classify_dialog, cut_for_log, input_is_empty, screens_equal};

#[test]
fn dialogs_are_recognised_by_the_verified_texts() {
    for (cli, screen, want) in [
        (
            "claude",
            "Is this a project you created or one you trust? (Like your own code)\n 1. Yes, I trust this folder",
            Some(Dialog::Untrusted),
        ),
        (
            "claude",
            "  Yes, I trust this folder\n  No, exit",
            Some(Dialog::Untrusted),
        ),
        (
            "claude",
            "Select login method:\n 1. Claude account",
            Some(Dialog::NotLoggedIn),
        ),
        ("claude", "Please run /login", Some(Dialog::NotLoggedIn)),
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
    assert_eq!(Dialog::Untrusted.code(), "workspace_untrusted");
    let reason = Dialog::Untrusted.reason("SVWL");
    assert!(
        reason.contains("sylvode-runner trust SVWL") && !reason.contains('/'),
        "{reason}"
    );
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
