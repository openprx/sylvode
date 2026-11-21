//! What the screen of a CLI says: blocking dialogs, and whether the input line is empty
//! (design sections 7.2, 7.3 and 7.6). The screen text (`capture-pane`) is used only for these
//! classifications; it is never logged above `debug`, cut to 400 characters there, and never sent
//! to Sylvode.

/// A dialog that blocks the CLI.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Dialog {
    /// The CLI asks whether to trust the directory.
    Untrusted,
    /// The CLI asks to log in.
    NotLoggedIn,
    /// A chooser the runner cannot answer (codex's rate-limit chooser).
    Chooser,
    /// The account's usage limit is reached.
    UsageLimit,
}

impl Dialog {
    /// The `reason_code` of the failure this dialog causes.
    pub const fn code(self) -> &'static str {
        match self {
            Self::Untrusted => "workspace_untrusted",
            Self::NotLoggedIn => "cli_not_logged_in",
            Self::Chooser => "cli_dialog",
            Self::UsageLimit => "cli_usage_limit",
        }
    }

    /// The failure text, with the fix. Never holds a path.
    pub fn reason(self, key: &str) -> String {
        match self {
            Self::Untrusted => format!(
                "the CLI asks to trust the project directory; run `sylvode-runner trust {key}` and answer the dialog"
            ),
            Self::NotLoggedIn => format!("the CLI is not logged in; attach to tmux session {key} and log in"),
            Self::Chooser => format!("the CLI shows a dialog the runner cannot answer; attach to tmux session {key}"),
            Self::UsageLimit => "the CLI reports that its usage limit is reached".to_string(),
        }
    }
}

/// The dialog on `screen`, if any, by the texts verified for each CLI.
pub fn classify_dialog(cli: &str, screen: &str) -> Option<Dialog> {
    match cli {
        "claude" => {
            if screen.contains("Is this a project you created or one you trust")
                || screen.contains("Yes, I trust this folder")
            {
                Some(Dialog::Untrusted)
            } else if screen.contains("Select login method") || screen.contains("Please run /login") {
                Some(Dialog::NotLoggedIn)
            } else {
                None
            }
        }
        "codex" => {
            if screen.contains("usage limit") {
                Some(Dialog::UsageLimit)
            } else if screen.contains("enter select \u{b7} esc back") {
                Some(Dialog::Chooser)
            } else {
                None
            }
        }
        _ => None,
    }
}

/// The prompt glyphs: claude's `>` (also `❯`) and codex's `›`.
fn prompt_rest(cli: &str, line: &str) -> Option<String> {
    let stripped = line.trim_start_matches(|c: char| c.is_whitespace() || matches!(c, '\u{2502}' | '\u{2503}' | '|'));
    let glyphs: &[char] = match cli {
        "claude" => &['>', '\u{276f}'],
        "codex" => &['\u{203a}'],
        _ => &['>'],
    };
    let first = stripped.chars().next()?;
    if !glyphs.contains(&first) {
        return None;
    }
    Some(stripped[first.len_utf8()..].to_string())
}

/// Whether the input line is empty: the last line that starts with the CLI's prompt glyph (after
/// optional box-drawing characters) holds nothing but whitespace and box characters.
///
/// `None` when no prompt line is on the screen: nothing is known, so nothing is pushed.
pub fn input_is_empty(cli: &str, screen: &str) -> Option<bool> {
    let rest = screen.lines().rev().find_map(|line| prompt_rest(cli, line))?;
    Some(
        rest.chars()
            .all(|c| c.is_whitespace() || matches!(c, '\u{2502}' | '\u{2503}' | '|')),
    )
}

/// The text on the input line: what follows the prompt glyph on the last prompt line.
///
/// `None` when no prompt line is on the screen.
pub fn input_text(cli: &str, screen: &str) -> Option<String> {
    screen
        .lines()
        .rev()
        .find_map(|line| prompt_rest(cli, line))
        .map(|rest| {
            rest.trim_matches(|c: char| c.is_whitespace() || matches!(c, '\u{2502}' | '\u{2503}' | '|'))
                .to_string()
        })
}

/// Whether the screen shows the CLI at work: claude and codex both print "esc to interrupt" while
/// a turn runs. An input line that looks empty says nothing about this (it is empty while the CLI
/// works), so a push needs this independent signal as well.
pub fn looks_busy(_cli: &str, screen: &str) -> bool {
    screen.to_lowercase().contains("esc to interrupt")
}

/// Whether the screen shows a choice the CLI waits for (a permission or elicitation dialog): a
/// numbered option with the selection marker, or the usual footer. Nothing is typed into it.
pub fn awaits_choice(_cli: &str, screen: &str) -> bool {
    let selected_option = screen.lines().any(|line| {
        let rest = line
            .trim_start_matches(|c: char| c.is_whitespace() || matches!(c, '\u{2502}' | '\u{2503}' | '|'))
            .trim_start_matches(['\u{276f}', '>', '\u{203a}'])
            .trim_start();
        let digits = rest.chars().take_while(char::is_ascii_digit).count();
        digits > 0 && rest[digits..].starts_with(". ") && line.contains(['\u{276f}', '\u{203a}'])
    });
    let lower = screen.to_lowercase();
    selected_option || lower.contains("enter to select") || lower.contains("esc to cancel")
}

/// Whether two captures show the same screen (trailing blanks ignored): the stability signal of
/// the readiness fallback.
pub fn screens_equal(a: &str, b: &str) -> bool {
    let norm = |s: &str| {
        s.lines()
            .map(str::trim_end)
            .collect::<Vec<_>>()
            .join("\n")
            .trim_end()
            .to_string()
    };
    norm(a) == norm(b)
}

/// The text cut for a `debug` log: at most 400 characters.
pub fn cut_for_log(screen: &str) -> String {
    screen.chars().take(400).collect()
}
