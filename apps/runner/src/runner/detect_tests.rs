use super::detect::{
    Dialog, awaits_choice, bottom, classify_bottom, classify_dialog, cut_for_log, input_is_empty, input_text,
    looks_busy, screens_equal,
};
use super::profile::profile;

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

const IDLE_PLACEHOLDER_PLAIN: &str = include_str!("fixtures/claude-idle-placeholder.txt");
const IDLE_PLACEHOLDER_STYLED: &str = include_str!("fixtures/claude-idle-placeholder.ansi");

/// A real v2.1.330 idle screen: the prompt shows a dim suggestion, which is not a draft.
#[test]
fn a_faint_placeholder_in_the_real_capture_counts_as_empty() {
    assert_eq!(input_is_empty("claude", IDLE_PLACEHOLDER_STYLED), Some(true));
    assert_eq!(input_text("claude", IDLE_PLACEHOLDER_STYLED).as_deref(), Some(""));
    // Plain text carries no attribute: the suggestion looks like a draft there, so the decision
    // must be made on the styled capture.
    assert_eq!(input_is_empty("claude", IDLE_PLACEHOLDER_PLAIN), Some(false));
    // Every other classification reads the same on both captures.
    assert!(!looks_busy("claude", IDLE_PLACEHOLDER_STYLED));
    assert!(!awaits_choice("claude", IDLE_PLACEHOLDER_STYLED));
    assert_eq!(classify_bottom("claude", IDLE_PLACEHOLDER_STYLED), None);
    assert!(screens_equal(IDLE_PLACEHOLDER_STYLED, IDLE_PLACEHOLDER_PLAIN));
    assert!(!cut_for_log(IDLE_PLACEHOLDER_STYLED).contains('\u{1b}'));
}

#[test]
fn only_faint_text_is_a_placeholder() {
    for (screen, want) in [
        // The fixture line with normal intensity: a draft.
        (
            "\u{1b}[39m\u{276f}\u{a0}Try \"how do I log an error?\"\u{1b}[0m",
            Some(false),
        ),
        ("\u{1b}[39m\u{276f}\u{a0}\u{1b}[2mTry \"x\"\u{1b}[0m", Some(true)),
        // Faint ended by SGR 22.
        ("\u{276f} \u{1b}[2msuggest\u{1b}[22m", Some(true)),
        // Real text followed by a faint ghost completion.
        ("\u{276f}\u{a0}git sta\u{1b}[2mtus\u{1b}[0m", Some(false)),
        // Faint text followed by real text.
        ("\u{276f}\u{a0}\u{1b}[2mfa\u{1b}[0mreal", Some(false)),
        // Faint reset by SGR 0 and a bare reset; extended colours do not turn faint on.
        ("\u{276f} \u{1b}[2mx\u{1b}[m typed", Some(false)),
        ("\u{276f} \u{1b}[38;2;1;2;3mtyped\u{1b}[0m", Some(false)),
        // Plain screens.
        ("\u{276f}\u{a0}", Some(true)),
        ("> ", Some(true)),
        ("> draft", Some(false)),
        ("\u{1b}[2mno prompt here\u{1b}[0m", None),
        ("", None),
    ] {
        assert_eq!(input_is_empty("claude", screen), want, "{screen:?}");
    }
    assert_eq!(
        input_text("claude", "\u{276f}\u{a0}git sta\u{1b}[2mtus\u{1b}[0m").as_deref(),
        Some("git status")
    );
}

const CODEX_IDLE: &str = include_str!("fixtures/codex-idle-placeholder.ansi");
const CODEX_IDLE_BYPASS: &str = include_str!("fixtures/codex-idle-bypass.ansi");
const CODEX_AFTER_TURN: &str = include_str!("fixtures/codex-after-turn.ansi");
const CODEX_BUSY: &str = include_str!("fixtures/codex-busy.ansi");
const CODEX_DRAFT: &str = include_str!("fixtures/codex-draft.ansi");
const CODEX_PASTED: &str = include_str!("fixtures/codex-pasted.ansi");
const CODEX_LOGIN: &str = include_str!("fixtures/codex-login.ansi");
const CODEX_TRUST: &str = include_str!("fixtures/codex-trust.ansi");
const CODEX_TRUST_BYPASS: &str = include_str!("fixtures/codex-trust-git-dir-with-bypass-flag.ansi");
const CODEX_HOOK_TRUST: &str = include_str!("fixtures/codex-hook-trust.ansi");
const CODEX_PERMISSION: &str = include_str!("fixtures/codex-permission.ansi");
const CODEX_MCP_APPROVAL: &str = include_str!("fixtures/codex-mcp-approval.ansi");

/// Real codex 0.171.0 idle screens: the composer shows a faint placeholder, which is not a draft,
/// before and after a turn and with the bypass flag.
#[test]
fn codex_idle_screens_have_an_empty_input_and_no_dialog() {
    for (name, screen) in [
        ("idle", CODEX_IDLE),
        ("idle bypass", CODEX_IDLE_BYPASS),
        ("after turn", CODEX_AFTER_TURN),
    ] {
        assert_eq!(input_is_empty("codex", screen), Some(true), "{name}");
        assert_eq!(input_text("codex", screen).as_deref(), Some(""), "{name}");
        assert!(!looks_busy("codex", screen), "{name}");
        assert!(!awaits_choice("codex", screen), "{name}");
        assert_eq!(classify_bottom("codex", screen), None, "{name}");
    }
}

/// The non-faint placeholder the same composer draws in another mode still counts as empty.
#[test]
fn a_plain_codex_follow_up_placeholder_is_an_empty_input() {
    assert_eq!(input_is_empty("codex", "\u{203a} Ask a follow-up question"), Some(true));
    assert_eq!(
        input_text("codex", "\u{203a} Ask a follow-up question").as_deref(),
        Some("")
    );
    assert_eq!(
        input_is_empty("codex", "\u{203a} Ask a follow-up question now"),
        Some(false)
    );
    assert_eq!(input_is_empty("claude", "> Ask a follow-up question"), Some(false));
}

#[test]
fn codex_draft_and_pasted_text_are_input() {
    assert_eq!(input_is_empty("codex", CODEX_DRAFT), Some(false));
    assert_eq!(input_text("codex", CODEX_DRAFT).as_deref(), Some("run tests"));
    assert_eq!(input_is_empty("codex", CODEX_PASTED), Some(false));
    let pasted = input_text("codex", CODEX_PASTED).expect("input text");
    assert_eq!(pasted, "[Pasted Content 1663 chars]");
    assert!(profile("codex").is_some_and(|p| p.has_pasted_marker(&pasted)));
    assert!(!looks_busy("codex", CODEX_PASTED) && classify_bottom("codex", CODEX_PASTED).is_none());
}

/// Busy is the `Working (Ns . esc to interrupt)` line above the composer, which stays drawn with its
/// placeholder; the echoed task text in the transcript (it starts with the glyph too) is not the
/// input line.
#[test]
fn codex_busy_is_the_working_line_and_the_composer_stays_empty() {
    assert!(looks_busy("codex", CODEX_BUSY));
    assert_eq!(input_is_empty("codex", CODEX_BUSY), Some(true));
    assert!(!awaits_choice("codex", CODEX_BUSY));
    assert_eq!(classify_bottom("codex", CODEX_BUSY), None);
}

#[test]
fn codex_dialogs_are_recognised_on_the_captures() {
    for (name, screen, want) in [
        ("login", CODEX_LOGIN, Dialog::NotLoggedIn),
        ("trust", CODEX_TRUST, Dialog::Trust),
        ("trust with the bypass flag", CODEX_TRUST_BYPASS, Dialog::Trust),
        ("hooks review", CODEX_HOOK_TRUST, Dialog::HooksReview),
        ("command permission", CODEX_PERMISSION, Dialog::CommandPermission),
        ("mcp approval", CODEX_MCP_APPROVAL, Dialog::McpApproval),
    ] {
        assert_eq!(classify_dialog("codex", screen), Some(want), "{name}");
        assert_eq!(classify_bottom("codex", screen), Some(want), "{name} at the bottom");
        assert_eq!(
            want.code(),
            if want == Dialog::NotLoggedIn {
                "cli_not_logged_in"
            } else {
                "cli_dialog"
            }
        );
        assert!(want.retryable());
        // The pane is not an input line to push into.
        assert_ne!(input_is_empty("codex", screen), Some(true), "{name}");
    }
    // The approval prompts are choices the runner never answers.
    assert!(awaits_choice("codex", CODEX_PERMISSION));
    assert!(awaits_choice("codex", CODEX_MCP_APPROVAL));
    // A dialog text of another CLI does not match, and the codex idle screens show none.
    assert_eq!(classify_dialog("claude", CODEX_TRUST), None);
    assert_eq!(classify_dialog("codex", CODEX_IDLE), None);
}

/// The MCP approval text names any server; a line that only mentions the words does not count.
#[test]
fn the_codex_mcp_approval_matches_any_server_name() {
    for server in ["probe", "sylvode", "node_repl"] {
        let screen = format!("  Allow the {server} MCP server to run tool \"x\"?");
        assert_eq!(classify_dialog("codex", &screen), Some(Dialog::McpApproval), "{server}");
    }
    assert_eq!(classify_dialog("codex", "Allow the MCP server"), None);
    assert_eq!(classify_dialog("codex", "MCP server to run tool"), None);
}

/// Text of a dialog quoted in the transcript above the composer is not a dialog.
#[test]
fn codex_dialog_text_in_the_transcript_does_not_count() {
    let screen = format!(
        "\u{203a} Trust this folder? 1. Trust and continue\nWould you like to run the following command?\n\
         Hooks need review\n{CODEX_IDLE}"
    );
    assert_eq!(classify_bottom("codex", &screen), None);
}
