//! The title sanitiser (design section 6.5, step 2). Issue and proposal titles come from people
//! who can edit them, and the message that contains one is typed into a CLI's input box, so a
//! title must not be able to carry a terminal sequence, a slash command, a file mention or a
//! line break. [`sanitize_title`] is total: every `&str` gives a printable, single-line,
//! non-empty answer of at most [`MAX_CHARS`] characters.

use unicode_normalization::UnicodeNormalization;

/// The longest result, in characters, including the ellipsis.
pub const MAX_CHARS: usize = 120;

/// What an empty result becomes.
pub const UNTITLED: &str = "(untitled)";

/// Bidirectional controls (U+202A to U+202E, U+2066 to U+2069).
const fn is_bidi_control(c: char) -> bool {
    matches!(c, '\u{202A}'..='\u{202E}' | '\u{2066}'..='\u{2069}')
}

/// C0 and C1 controls and DEL.
const fn is_removed_control(c: char) -> bool {
    c.is_control()
}

/// Sanitises a title for the message text.
///
/// In order: `\r`, `\n` and `\t` become a space; the other C0 and C1 controls (which includes
/// ESC), DEL, U+2028, U+2029 and the bidi controls are removed; the text is put in Unicode NFC;
/// runs of whitespace collapse to one space; `"` becomes `'`; the text is trimmed and cut to
/// [`MAX_CHARS`] characters with a final `…`; `@` and `$` followed by a non-space character
/// become fullwidth `＠` and `＄` (in Claude Code `@path` attaches a file, in codex `@` opens file
/// search and `$` names a skill); an empty result becomes `(untitled)`.
///
/// Controls are removed before the NFC step so the result is stable (removing a control between a
/// base and a combining mark cannot create a composition afterwards), and the `@` and `$` rule is
/// applied to the final text so the cut cannot leave one in front of the ellipsis.
#[must_use]
pub fn sanitize_title(title: &str) -> String {
    let filtered: String = title
        .chars()
        .filter_map(|c| match c {
            '\r' | '\n' | '\t' => Some(' '),
            '\u{2028}' | '\u{2029}' => None,
            c if is_bidi_control(c) || is_removed_control(c) => None,
            c => Some(c),
        })
        .nfc()
        .collect();
    let mut collapsed = String::with_capacity(filtered.len());
    let mut pending_space = false;
    for c in filtered.chars() {
        if c.is_whitespace() {
            pending_space = !collapsed.is_empty();
        } else {
            if pending_space {
                collapsed.push(' ');
                pending_space = false;
            }
            collapsed.push(if c == '"' { '\'' } else { c });
        }
    }
    let cut = if collapsed.chars().count() > MAX_CHARS {
        let head: String = collapsed.chars().take(MAX_CHARS - 1).collect();
        format!("{}…", head.trim_end())
    } else {
        collapsed
    };
    let mut out = String::with_capacity(cut.len());
    let mut chars = cut.chars().peekable();
    while let Some(c) = chars.next() {
        let followed = chars.peek().is_some_and(|next| !next.is_whitespace());
        out.push(match c {
            '@' if followed => '＠',
            '$' if followed => '＄',
            c => c,
        });
    }
    if out.is_empty() { UNTITLED.to_string() } else { out }
}
