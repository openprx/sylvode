//! The title sanitiser (design section 6.5, step 2, extended by the security review). Issue and
//! proposal titles come from people who can edit them, and the message that contains one is typed
//! into a CLI's input box, so a title must not be able to carry a terminal sequence, a slash
//! command, a file mention, a line break, invisible text, a closing quote or the header line of
//! the message itself. [`sanitize_title`] is total: every `&str` gives a printable, single-line,
//! non-empty answer of at most [`MAX_CHARS`] characters.

use unicode_normalization::UnicodeNormalization;
use unicode_properties::{GeneralCategory, UnicodeGeneralCategory};

/// The longest result, in characters, including the ellipsis.
pub const MAX_CHARS: usize = 120;

/// What an empty result becomes.
pub const UNTITLED: &str = "(untitled)";

/// Characters that are invisible or that render as nothing although they are not in a category
/// the sanitiser removes wholesale: the combining grapheme joiner, Hangul fillers, Khmer inherent
/// vowels, Mongolian and general variation selectors.
const fn is_listed_invisible(c: char) -> bool {
    matches!(
        c,
        '\u{34f}'
            | '\u{115f}'
            | '\u{1160}'
            | '\u{17b4}'
            | '\u{17b5}'
            | '\u{180b}'..='\u{180d}'
            | '\u{3164}'
            | '\u{fe00}'..='\u{fe0f}'
            | '\u{ffa0}'
            | '\u{e0100}'..='\u{e01ef}'
    )
}

/// Whether `c` is removed outright: C0, C1 and DEL, format characters (zero-width and directional
/// marks, bidi controls, the BOM, Unicode tag characters), surrogates, private use, unassigned
/// code points and noncharacters, the line and paragraph separators, and the listed invisibles.
fn is_removed(c: char) -> bool {
    matches!(
        c.general_category(),
        GeneralCategory::Control
            | GeneralCategory::Format
            | GeneralCategory::Surrogate
            | GeneralCategory::PrivateUse
            | GeneralCategory::Unassigned
            | GeneralCategory::LineSeparator
            | GeneralCategory::ParagraphSeparator
    ) || is_listed_invisible(c)
}

/// Quotes that could close the quoted title (the ASCII `"` and its look-alikes) and backticks
/// (command substitution in a shell, code spans in markdown) become `'`.
const fn is_quote_like(c: char) -> bool {
    matches!(
        c,
        '"' | '`'
            | '\u{201c}'
            | '\u{201d}'
            | '\u{201e}'
            | '\u{201f}'
            | '\u{ff02}'
            | '\u{2033}'
            | '\u{2036}'
            | '\u{275d}'
            | '\u{275e}'
            | '\u{301d}'..='\u{301f}' | '\u{ab}' | '\u{bb}' | '\u{ff40}' | '\u{2cb}'
    )
}

/// Sanitises a title for the message text.
///
/// In order:
/// 1. `\r`, `\n` and `\t` become a space; every character [`is_removed`] is dropped (this
///    includes ESC and the other controls, so no terminal sequence survives);
/// 2. the text is put in Unicode NFC;
/// 3. runs of whitespace collapse to one space, quote-like characters and backticks become `'`,
///    and square brackets become parentheses (so no title can contain the `[Sylvode` that starts
///    every message);
/// 4. the text is trimmed and cut to [`MAX_CHARS`] characters with a final `…`;
/// 5. `@` and `$` followed by a non-space character become fullwidth `＠` and `＄` (in Claude Code
///    `@path` attaches a file, in codex `@` opens file search and `$` names a skill);
/// 6. an empty result becomes `(untitled)`.
///
/// Controls are removed before the NFC step so the result is stable, characters that NFC turns
/// into a backtick or a semicolon are handled after it, and the `@` and `$` rule is applied to
/// the final text so the cut cannot leave one in front of the ellipsis.
#[must_use]
pub fn sanitize_title(title: &str) -> String {
    let filtered: String = title
        .chars()
        .filter_map(|c| match c {
            '\r' | '\n' | '\t' => Some(' '),
            c if is_removed(c) => None,
            c => Some(c),
        })
        .nfc()
        .collect();
    let mut collapsed = String::with_capacity(filtered.len());
    let mut pending_space = false;
    for c in filtered.chars() {
        // NFC can produce characters that are removed or that need a mapping (U+1FEF is a
        // backtick); judge them again here.
        if is_removed(c) {
            continue;
        }
        if c.is_whitespace() {
            pending_space = !collapsed.is_empty();
        } else {
            if pending_space {
                collapsed.push(' ');
                pending_space = false;
            }
            collapsed.push(match c {
                c if is_quote_like(c) => '\'',
                '[' => '(',
                ']' => ')',
                c => c,
            });
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
