//! What the screen of a CLI says: blocking dialogs, and whether the input line is empty
//! (design sections 7.2, 7.3 and 7.6). The screen text (`capture-pane`) is used only for these
//! classifications; it is never logged above `debug`, cut to 400 characters there, and never sent
//! to Sylvode.

/// A dialog that blocks the CLI.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Dialog {
    /// The CLI asks whether to trust the directory.
    Trust,
    /// claude asks to confirm bypass-permissions mode (the default launch command starts it).
    BypassConfirm,
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
            Self::Trust | Self::BypassConfirm | Self::Chooser => "cli_dialog",
            Self::NotLoggedIn => "cli_not_logged_in",
            Self::UsageLimit => "cli_usage_limit",
        }
    }

    /// Whether the failed task may be tried again: everything but a usage limit waits for the
    /// operator to answer in the session once.
    pub const fn retryable(self) -> bool {
        !matches!(self, Self::UsageLimit)
    }

    /// The failure text, with the fix. Names the session, never a path.
    pub fn reason(self, session: &str) -> String {
        match self {
            Self::Trust | Self::BypassConfirm | Self::Chooser => format!(
                "the CLI in tmux session {session} shows a dialog; attach with tmux attach -t {session} and answer it"
            ),
            Self::NotLoggedIn => {
                format!("the CLI in tmux session {session} is not logged in; attach and log in")
            }
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
                Some(Dialog::Trust)
            } else if screen.contains("Bypass Permissions mode") && screen.contains("Yes, I accept") {
                Some(Dialog::BypassConfirm)
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

/// The last `n` lines that are not blank: the part of the screen the CLI itself draws at the bottom.
/// Text that the task or the agent put on the screen sits above it, in the transcript.
fn bottom_lines(screen: &str, n: usize) -> Vec<&str> {
    let lines: Vec<&str> = screen.lines().filter(|l| !l.trim().is_empty()).collect();
    let skip = lines.len().saturating_sub(n);
    lines.into_iter().skip(skip).collect()
}

/// The bottom of the screen as text, for the classifications that must not be fooled by text higher
/// up (a task title, the agent's reply).
pub fn bottom(screen: &str, n: usize) -> String {
    bottom_lines(screen, n).join("\n")
}

/// [`classify_dialog`] on the CLI's own chrome at the bottom only: a dialog is drawn there, and a
/// phrase in the transcript does not count.
pub fn classify_bottom(cli: &str, screen: &str) -> Option<Dialog> {
    let lines = bottom_lines(screen, usize::MAX);
    classify_dialog(cli, &chrome(cli, &lines).join("\n"))
}

/// The CLI's own chrome: the lines from three above the last prompt line (where the spinner is
/// drawn) down to the bottom; without a prompt line, the last twelve lines.
fn chrome<'a>(cli: &str, lines: &'a [&'a str]) -> &'a [&'a str] {
    let last_prompt = lines.iter().rposition(|line| prompt_rest(cli, line).is_some());
    let from = last_prompt.map_or_else(|| lines.len().saturating_sub(12), |at| at.saturating_sub(3));
    lines.get(from..).unwrap_or_default()
}

/// Whether a line is the spinner of a working CLI: a glyph, some text, then a parenthesised part
/// that holds "esc to interrupt" and ends the line.
fn is_spinner_line(line: &str) -> bool {
    let trimmed = line.trim();
    let Some(first) = trimmed.chars().next() else {
        return false;
    };
    if first.is_ascii_alphanumeric() || matches!(first, '>' | '(' | '|' | '-' | '"' | '\'' | '[') {
        return false;
    }
    let lower = trimmed.to_lowercase();
    lower.ends_with(')')
        && lower
            .rfind('(')
            .is_some_and(|open| lower[open..].contains("esc to interrupt") && open > first.len_utf8())
}

/// Whether the screen shows the CLI at work: its spinner line, in the CLI's own chrome at the
/// bottom. An empty input line says nothing (it is empty while the CLI works), so a push needs this
/// as well; text elsewhere on the screen never counts.
pub fn looks_busy(cli: &str, screen: &str) -> bool {
    let lines = bottom_lines(screen, usize::MAX);
    chrome(cli, &lines).iter().any(|line| is_spinner_line(line))
}

/// Whether the screen shows a choice the CLI waits for (a permission or elicitation dialog): a
/// numbered option with the selection marker in the chrome, or the usual footer in the last lines.
/// Nothing is typed into it.
pub fn awaits_choice(cli: &str, screen: &str) -> bool {
    let lines = bottom_lines(screen, usize::MAX);
    let selected_option = chrome(cli, &lines).iter().any(|line| {
        let rest = line
            .trim_start_matches(|c: char| c.is_whitespace() || matches!(c, '\u{2502}' | '\u{2503}' | '|'))
            .trim_start_matches(['\u{276f}', '>', '\u{203a}'])
            .trim_start();
        let digits = rest.chars().take_while(char::is_ascii_digit).count();
        digits > 0 && rest[digits..].starts_with(". ") && line.contains(['\u{276f}', '\u{203a}'])
    });
    let footer = lines
        .iter()
        .skip(lines.len().saturating_sub(3))
        .copied()
        .collect::<Vec<_>>()
        .join("\n")
        .to_lowercase();
    selected_option || footer.contains("enter to select") || footer.contains("esc to cancel")
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
