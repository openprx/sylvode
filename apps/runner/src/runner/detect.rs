//! What the screen of a CLI says: blocking dialogs, and whether the input line is empty
//! (design sections 7.2, 7.3 and 7.6). The screen text (`capture-pane`) is used only for these
//! classifications; it is never logged above `debug`, cut to 400 characters there, and never sent
//! to Sylvode. What differs per CLI is read from its profile (`profile.rs`).

use super::profile::profile;

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
    /// codex asks to review or trust its hooks.
    HooksReview,
    /// codex asks whether it may run a command.
    CommandPermission,
    /// codex asks whether an MCP server may run a tool.
    McpApproval,
}

impl Dialog {
    /// The `reason_code` of the failure this dialog causes.
    pub const fn code(self) -> &'static str {
        match self {
            Self::Trust
            | Self::BypassConfirm
            | Self::Chooser
            | Self::HooksReview
            | Self::CommandPermission
            | Self::McpApproval => "cli_dialog",
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
            Self::Trust
            | Self::BypassConfirm
            | Self::Chooser
            | Self::HooksReview
            | Self::CommandPermission
            | Self::McpApproval => format!(
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
    let dialogs = profile(cli)?.dialogs;
    dialogs(&plain(screen))
}

/// The text after the prompt glyph of `line`, when the line is the CLI's input line.
fn prompt_rest(cli: &str, line: &str) -> Option<String> {
    let profile = profile(cli);
    let glyphs: &[char] = profile.map_or(&['>'], |p| p.prompt_glyphs);
    let stripped = line.trim_start_matches(|c: char| c.is_whitespace() || matches!(c, '\u{2502}' | '\u{2503}' | '|'));
    let first = stripped.chars().next()?;
    if !glyphs.contains(&first) {
        return None;
    }
    let rest = &stripped[first.len_utf8()..];
    if profile.is_some_and(|p| p.options_use_prompt_glyph) && is_numbered_option(rest) {
        return None;
    }
    Some(rest.to_string())
}

/// Whether `text` starts like `1. Yes`: a dialog option, not something typed.
fn is_numbered_option(text: &str) -> bool {
    let rest = text.trim_start();
    let digits = rest.chars().take_while(char::is_ascii_digit).count();
    digits > 0 && rest[digits..].starts_with(". ")
}

/// One screen cell: the character and whether it is drawn faint (SGR 2).
type Cell = (char, bool);

/// The cells of one captured line (`capture-pane -p -e`): escape sequences are dropped, the faint
/// attribute is tracked (SGR 2 on, SGR 0 or 22 off; extended colours are skipped over).
fn cells(line: &str) -> Vec<Cell> {
    let mut out = Vec::with_capacity(line.len());
    let mut faint = false;
    let mut chars = line.chars();
    while let Some(c) = chars.next() {
        if c != '\u{1b}' {
            out.push((c, faint));
            continue;
        }
        match chars.next() {
            Some('[') => {
                let mut params = String::new();
                let mut last = None;
                for p in chars.by_ref() {
                    if ('\u{40}'..='\u{7e}').contains(&p) {
                        last = Some(p);
                        break;
                    }
                    params.push(p);
                }
                if last == Some('m') {
                    faint = apply_sgr(&params, faint);
                }
            }
            Some(']') => {
                // An operating system command ends with BEL or ESC \.
                while let Some(p) = chars.next() {
                    if p == '\u{7}' {
                        break;
                    }
                    if p == '\u{1b}' {
                        chars.next();
                        break;
                    }
                }
            }
            _ => {}
        }
    }
    out
}

/// The faint state after the SGR parameters `params`.
fn apply_sgr(params: &str, mut faint: bool) -> bool {
    let mut it = params.split([';', ':']);
    while let Some(p) = it.next() {
        match p {
            "" | "0" | "22" => faint = false,
            "2" => faint = true,
            "38" | "48" | "58" => match it.next() {
                Some("5") => {
                    it.next();
                }
                Some("2") => {
                    it.nth(2);
                }
                _ => {}
            },
            _ => {}
        }
    }
    faint
}

/// The screen without escape sequences: what every classification but the input line reads.
fn plain(screen: &str) -> std::borrow::Cow<'_, str> {
    if !screen.contains('\u{1b}') {
        return std::borrow::Cow::Borrowed(screen);
    }
    let lines: Vec<String> = screen
        .lines()
        .map(|l| cells(l).into_iter().map(|(c, _)| c).collect())
        .collect();
    std::borrow::Cow::Owned(lines.join("\n"))
}

const fn is_blank_or_box(c: char) -> bool {
    matches!(c, '\u{2502}' | '\u{2503}' | '|') || c.is_whitespace()
}

/// The last prompt line of a (possibly styled) screen: the text after the glyph and whether all of
/// its visible characters are faint, which is how claude draws its suggestion in an empty input box.
fn last_prompt(cli: &str, screen: &str) -> Option<(String, bool)> {
    screen.lines().rev().find_map(|line| {
        let cs = cells(line);
        let text: String = cs.iter().map(|(c, _)| *c).collect();
        let rest = prompt_rest(cli, &text)?;
        let skip = cs.len() - rest.chars().count();
        let ghost = cs
            .get(skip..)?
            .iter()
            .filter(|(c, _)| !is_blank_or_box(*c))
            .all(|(_, f)| *f);
        let ghost = ghost
            || profile(cli).is_some_and(|p| {
                let text = rest.trim_matches(is_blank_or_box);
                p.placeholders.contains(&text)
            });
        Some((rest, ghost))
    })
}

/// Whether the input line is empty: the last line that starts with the CLI's prompt glyph (after
/// optional box-drawing characters) holds nothing but whitespace and box characters, or only faint
/// text (a placeholder suggestion; real typing is never faint). Reads a screen captured with escapes.
///
/// `None` when no prompt line is on the screen: nothing is known, so nothing is pushed.
pub fn input_is_empty(cli: &str, screen: &str) -> Option<bool> {
    let (rest, ghost) = last_prompt(cli, screen)?;
    Some(ghost || rest.chars().all(is_blank_or_box))
}

/// The text on the input line: what follows the prompt glyph on the last prompt line. A faint
/// suggestion (see [`input_is_empty`]) is not text.
///
/// `None` when no prompt line is on the screen.
pub fn input_text(cli: &str, screen: &str) -> Option<String> {
    let (rest, ghost) = last_prompt(cli, screen)?;
    Some(if ghost {
        String::new()
    } else {
        rest.trim_matches(is_blank_or_box).to_string()
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
    let screen = &*plain(screen);
    bottom_lines(screen, n).join("\n")
}

/// [`classify_dialog`] on the CLI's own chrome at the bottom only: a dialog is drawn there, and a
/// phrase in the transcript does not count.
pub fn classify_bottom(cli: &str, screen: &str) -> Option<Dialog> {
    let screen = &*plain(screen);
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
    let screen = &*plain(screen);
    let lines = bottom_lines(screen, usize::MAX);
    chrome(cli, &lines).iter().any(|line| is_spinner_line(line))
}

/// Whether the screen shows a choice the CLI waits for (a permission or elicitation dialog): a
/// numbered option with the selection marker in the chrome, or the usual footer in the last lines.
/// Nothing is typed into it.
pub fn awaits_choice(cli: &str, screen: &str) -> bool {
    let screen = &*plain(screen);
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
    let (a, b) = (&*plain(a), &*plain(b));
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
    plain(screen).chars().take(400).collect()
}
