use super::sanitize::{MAX_CHARS, UNTITLED, sanitize_title};

#[test]
fn plain_titles_pass_unchanged() {
    assert_eq!(sanitize_title("Fix the login page"), "Fix the login page");
    assert_eq!(sanitize_title("Überprüfung 修复 🎉"), "Überprüfung 修复 🎉");
}

#[test]
fn each_rule_on_its_own() {
    for (input, want) in [
        // NFC: e + combining acute becomes the precomposed character.
        ("Cafe\u{301}", "Caf\u{e9}"),
        // C0, C1 and DEL are removed; ESC takes its sequence's introducer with it.
        ("a\u{1}b\u{7f}c\u{85}d\u{9b}e", "abcde"),
        ("red \u{1b}[31mtext\u{1b}[0m", "red [31mtext[0m"),
        // Line separators and bidi controls are removed.
        ("a\u{2028}b\u{2029}c", "abc"),
        ("a\u{202a}b\u{202e}c\u{2066}d\u{2069}e", "abcde"),
        // CR, LF and tab become a space, and runs of whitespace collapse.
        ("a\r\nb\tc", "a b c"),
        ("  a \n\n \t b  ", "a b"),
        ("a\u{a0}\u{2003}b", "a b"),
        // Double quotes become single quotes.
        ("say \"hi\"", "say 'hi'"),
        // @ and $ before a non-space become fullwidth; before a space or at the end they stay.
        ("@file and $skill", "＠file and ＄skill"),
        ("a @ b $ c @", "a @ b $ c @"),
        ("x@y $5", "x＠y ＄5"),
        // Empty results.
        ("", UNTITLED),
        ("   ", UNTITLED),
        ("\u{1}\u{2}\u{1b}\u{202e}", UNTITLED),
        ("\n\r\t", UNTITLED),
    ] {
        assert_eq!(sanitize_title(input), want, "{input:?}");
    }
}

#[test]
fn a_title_that_is_only_control_characters_is_untitled() {
    let controls: String = ('\u{0}'..='\u{1f}').chain('\u{7f}'..='\u{9f}').collect();
    assert_eq!(sanitize_title(&controls), UNTITLED);
}

#[test]
fn long_titles_are_cut_to_120_characters_with_an_ellipsis() {
    let long = "x".repeat(500);
    let out = sanitize_title(&long);
    assert_eq!(out.chars().count(), MAX_CHARS);
    assert!(out.ends_with('…'));
    assert!(out.starts_with(&"x".repeat(119)));
    let exact = "y".repeat(MAX_CHARS);
    assert_eq!(sanitize_title(&exact), exact);
    let one_more = "y".repeat(MAX_CHARS + 1);
    assert_eq!(sanitize_title(&one_more), format!("{}…", "y".repeat(119)));
    // Multi-byte characters count as one each, and a cut never splits one.
    let wide = "修".repeat(300);
    assert_eq!(sanitize_title(&wide).chars().count(), MAX_CHARS);
    // A cut that lands after a space does not leave the space before the ellipsis.
    let spaced = format!("{} z", "a ".repeat(70));
    assert!(!sanitize_title(&spaced).contains(" …"));
}

#[test]
fn an_at_sign_at_the_cut_does_not_touch_the_ellipsis() {
    let title = format!("{}@{}", "a".repeat(118), "b".repeat(50));
    let out = sanitize_title(&title);
    assert!(out.ends_with("＠…"), "{out}");
}

#[test]
fn the_hostile_example_from_the_design() {
    let out = sanitize_title("\"/help\n$(id) @/etc/passwd \u{1b}[31m");
    assert_eq!(out, "'/help ＄(id) ＠/etc/passwd [31m");
    assert!(!out.contains('\u{1b}') && !out.contains('\n') && !out.contains('"'));
}

/// A small deterministic generator (xorshift64*), so the property tests need no extra crate and
/// every failure reproduces from its seed.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_f491_4f6c_dd1d)
    }

    fn below(&mut self, n: u64) -> u64 {
        self.next() % n
    }

    fn pick<'a, T>(&mut self, items: &'a [T]) -> &'a T {
        let index = usize::try_from(self.below(items.len() as u64)).unwrap_or(0);
        items.get(index).or_else(|| items.first()).expect("a non-empty list")
    }

    /// Any Unicode scalar value, drawn from a mix of ranges that are hard cases for the
    /// sanitiser and from the whole code space.
    fn any_char(&mut self) -> char {
        const SPECIAL: &[char] = &[
            '@', '$', '"', '\'', '\n', '\r', '\t', ' ', '\u{1b}', '\u{7f}', '\u{85}', '\u{2028}', '\u{2029}',
            '\u{202a}', '\u{202e}', '\u{2066}', '\u{2069}', '\u{a0}', '\u{3000}', '\u{200b}', '\u{301}', '\u{fe0f}',
            '＠', '＄', '…', '/', '!', '#', '[', '~',
        ];
        match self.below(6) {
            0 | 1 => *self.pick(SPECIAL),
            2 => char::from_u32(u32::try_from(self.below(0x80)).unwrap_or(0)).unwrap_or(' '),
            3 => char::from_u32(u32::try_from(self.below(0xa0)).unwrap_or(0)).unwrap_or(' '),
            4 => char::from_u32(u32::try_from(0x300 + self.below(0x100)).unwrap_or(0)).unwrap_or('a'),
            _ => char::from_u32(u32::try_from(self.below(0x11_0000)).unwrap_or(0)).unwrap_or('\u{fffd}'),
        }
    }

    fn string(&mut self) -> String {
        let length = match self.below(10) {
            0 => 0,
            1..=7 => self.below(40),
            8 => self.below(200),
            _ => 100 + self.below(400),
        };
        (0..length).map(|_| self.any_char()).collect()
    }
}

fn is_bidi(c: char) -> bool {
    matches!(c, '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}')
}

/// For any input the output is non-empty, one line, printable, free of terminal controls, line
/// separators, bidi controls and double quotes, at most 120 characters, trimmed, with single
/// spaces, and every `@` or `$` is followed by a space or ends the text.
#[test]
fn the_result_is_safe_for_any_input() {
    let mut rng = Rng(0x5eed_1234_abcd_ef01);
    for case in 0..20_000 {
        let input = rng.string();
        let out = sanitize_title(&input);
        let context = || format!("case {case}: {input:?} -> {out:?}");
        assert!(!out.is_empty(), "{}", context());
        assert!(out.chars().count() <= MAX_CHARS, "{}", context());
        assert!(out.chars().all(|c| !c.is_control()), "{}", context());
        assert!(
            out.chars().all(|c| c != '\u{2028}' && c != '\u{2029}' && !is_bidi(c)),
            "{}",
            context()
        );
        assert!(!out.contains('"'), "{}", context());
        assert_eq!(out.trim(), out, "{}", context());
        assert!(!out.contains("  "), "{}", context());
        assert!(out.chars().all(|c| !c.is_whitespace() || c == ' '), "{}", context());
        let chars: Vec<char> = out.chars().collect();
        for pair in chars.windows(2) {
            if let [first, second] = pair {
                assert!(!matches!(first, '@' | '$') || second.is_whitespace(), "{}", context());
            }
        }
    }
}

/// Sanitising twice changes nothing.
#[test]
fn sanitising_is_idempotent() {
    let mut rng = Rng(0x0dd_ba11_cafe_f00d);
    for case in 0..20_000 {
        let input = rng.string();
        let once = sanitize_title(&input);
        let twice = sanitize_title(&once);
        assert_eq!(once, twice, "case {case}: {input:?}");
    }
}

/// Text that is already safe and short is returned as it is (after NFC).
#[test]
fn safe_ascii_is_unchanged() {
    let mut rng = Rng(42);
    for _ in 0..2_000 {
        let length = rng.below(100);
        let input: String = (0..length)
            .map(|_| {
                let c = char::from_u32(u32::try_from(0x20 + rng.below(0x5f)).unwrap_or(0x20)).unwrap_or('a');
                if matches!(c, '@' | '$' | '"') { 'q' } else { c }
            })
            .collect();
        let collapsed = input.split_whitespace().collect::<Vec<_>>().join(" ");
        let want = if collapsed.is_empty() {
            UNTITLED.to_string()
        } else {
            collapsed
        };
        assert_eq!(sanitize_title(&input), want, "{input:?}");
    }
}
