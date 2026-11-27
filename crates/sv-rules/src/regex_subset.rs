//! The regex subset accepted by the `pattern` rule and CEL `matches`.
//!
//! The browser compiles a pattern with `new RegExp(p, 'u' + (ci ? 'i' : ''))` (the `pattern` rule) or
//! with cel-js `matches` (no flags); the server compiles a translation with the `regex` crate. A
//! whitelist scanner, written the same way in `regex-subset.ts`, accepts only constructs that mean
//! the same in all three, and refuses the rest with `pattern_unsupported`:
//!
//! - escapes: `\d \w \s \D \W \S`, `\t \n \r \v \f`, `\xHH`, `\uHHHH` (no surrogates), and the
//!   syntax characters `^ $ \ . * + ? ( ) [ ] { } | /` (`-` too inside a class). Everything else,
//!   including `\b \B`, `\p{..}`, back-references, `\u{..}` and `\A \z`, is refused;
//! - groups: `( .. )` and `(?: .. )` only (no inline flags, names or lookaround);
//! - classes: no nesting or POSIX classes, no `]` as the first member, no empty class, and a class
//!   escape cannot be a range end (members are emitted as `\x{..}` escapes, so Rust set operators
//!   such as `&&` never apply);
//! - quantifiers `* + ? {n} {n,} {n,m}` (bounds up to 1000, optionally lazy) only after an atom;
//! - a size budget: a literal costs 1 unit, a class, class escape or `.` costs 10, `{n,m}`
//!   multiplies its atom by `m` (`n` when open), and the pattern may cost at most 4,500 units, so
//!   the server's compiled size stays within its limit wherever the browser accepts the pattern;
//! - unescaped `]` and `}` outside a class, and characters outside the Basic Multilingual Plane.
//!
//! The server translation fixes the remaining differences: `\d \w \s` become the ASCII / ECMAScript
//! sets, and `.` excludes the four ECMAScript line terminators.
//!
//! Errors that the scanner accepts but an engine still refuses (for example a reversed range) are
//! `pattern_invalid`.

use std::fmt::Write;

use regex::{Regex, RegexBuilder};

use crate::limits::{CLASS_UNITS, MAX_PATTERN_BYTES, MAX_REPEAT, MAX_REPEAT_UNITS, REGEX_SIZE_LIMIT};

const UNSUPPORTED: &str = "pattern_unsupported";
const INVALID: &str = "pattern_invalid";

/// ECMAScript `\s`: `WhiteSpace` and `LineTerminator` code points, as a Rust class body.
const JS_SPACE: &str =
    r"\t\n\x0B\x0C\r \x{A0}\x{1680}\x{2000}-\x{200A}\x{2028}\x{2029}\x{202F}\x{205F}\x{3000}\x{FEFF}";
const DIGIT: &str = "0-9";
const WORD: &str = "0-9A-Za-z_";
/// ECMAScript `.` without the `s` flag.
const ANY: &str = r"[^\n\r\x{2028}\x{2029}]";

const SYNTAX_CHARS: &str = "^$\\.*+?()[]{}|/";

/// A scanned atom inside a class: a single code point (usable in a range) or a class escape.
enum ClassItem {
    Char(u32),
    Set(String),
}

struct Scanner<'a> {
    chars: std::iter::Peekable<std::str::Chars<'a>>,
    out: String,
}

const fn class_escape(c: char) -> Option<(&'static str, bool)> {
    match c {
        'd' => Some((DIGIT, false)),
        'w' => Some((WORD, false)),
        's' => Some((JS_SPACE, false)),
        'D' => Some((DIGIT, true)),
        'W' => Some((WORD, true)),
        'S' => Some((JS_SPACE, true)),
        _ => None,
    }
}

fn push_code_point(out: &mut String, cp: u32) {
    let _ = write!(out, "\\x{{{cp:X}}}");
}

impl Scanner<'_> {
    fn next(&mut self) -> Option<char> {
        self.chars.next()
    }

    fn peek(&mut self) -> Option<char> {
        self.chars.peek().copied()
    }

    fn hex(&mut self, n: usize) -> Result<u32, &'static str> {
        let mut v: u32 = 0;
        for _ in 0..n {
            let d = self.next().and_then(|c| c.to_digit(16)).ok_or(UNSUPPORTED)?;
            v = v * 16 + d;
        }
        Ok(v)
    }

    /// Scans the escape after `\`. Returns the code point for a literal or the class body.
    fn escape(&mut self, in_class: bool) -> Result<ClassItem, &'static str> {
        let c = self.next().ok_or(INVALID)?;
        if let Some((body, negated)) = class_escape(c) {
            let caret = if negated { "^" } else { "" };
            return Ok(ClassItem::Set(format!("[{caret}{body}]")));
        }
        let cp = match c {
            't' => 0x09,
            'n' => 0x0A,
            'v' => 0x0B,
            'f' => 0x0C,
            'r' => 0x0D,
            'x' => self.hex(2)?,
            'u' => {
                let v = self.hex(4)?;
                if (0xD800..=0xDFFF).contains(&v) {
                    return Err(UNSUPPORTED);
                }
                v
            }
            c if SYNTAX_CHARS.contains(c) || (in_class && c == '-') => u32::from(c),
            _ => return Err(UNSUPPORTED),
        };
        Ok(ClassItem::Char(cp))
    }

    fn literal_char(c: char) -> Result<u32, &'static str> {
        let cp = u32::from(c);
        if cp > 0xFFFF {
            return Err(UNSUPPORTED);
        }
        Ok(cp)
    }

    fn class(&mut self) -> Result<(), &'static str> {
        let mut body = String::new();
        let negated = if self.peek() == Some('^') {
            self.next();
            true
        } else {
            false
        };
        if self.peek() == Some(']') {
            return Err(UNSUPPORTED);
        }
        loop {
            let c = self.next().ok_or(INVALID)?;
            match c {
                ']' => break,
                '[' => return Err(UNSUPPORTED),
                _ => {}
            }
            let item = if c == '\\' {
                self.escape(true)?
            } else {
                ClassItem::Char(Self::literal_char(c)?)
            };
            // A range `a-b`: `-` followed by something other than `]`.
            let is_range = self.peek() == Some('-') && {
                let mut look = self.chars.clone();
                look.next();
                look.peek().is_some_and(|n| *n != ']')
            };
            if is_range {
                self.next();
                let hi_c = self.next().ok_or(INVALID)?;
                if hi_c == '[' {
                    return Err(UNSUPPORTED);
                }
                let hi = if hi_c == '\\' {
                    self.escape(true)?
                } else {
                    ClassItem::Char(Self::literal_char(hi_c)?)
                };
                let (ClassItem::Char(lo), ClassItem::Char(hi)) = (item, hi) else {
                    return Err(INVALID);
                };
                if lo > hi {
                    return Err(INVALID);
                }
                push_code_point(&mut body, lo);
                body.push('-');
                push_code_point(&mut body, hi);
                continue;
            }
            match item {
                ClassItem::Char(cp) => push_code_point(&mut body, cp),
                ClassItem::Set(set) => body.push_str(&set),
            }
        }
        if body.is_empty() {
            return Err(UNSUPPORTED);
        }
        self.out.push('[');
        if negated {
            self.out.push('^');
        }
        self.out.push_str(&body);
        self.out.push(']');
        Ok(())
    }

    fn number(&mut self) -> Option<u32> {
        let mut digits = String::new();
        while let Some(c) = self.peek().filter(char::is_ascii_digit) {
            digits.push(c);
            self.next();
        }
        if digits.is_empty() {
            return None;
        }
        // Saturate so an over-long bound reports `pattern_unsupported`, not a parse failure.
        Some(digits.parse::<u32>().unwrap_or(u32::MAX))
    }

    /// Parses `{n}`, `{n,}`, `{n,m}` and returns the repetition weight (`m`, or `n` if open).
    fn braces(&mut self) -> Result<u32, &'static str> {
        let lo = self.number().ok_or(INVALID)?;
        let (comma, hi) = if self.peek() == Some(',') {
            self.next();
            (true, self.number())
        } else {
            (false, Some(lo))
        };
        if self.next() != Some('}') {
            return Err(INVALID);
        }
        if lo > MAX_REPEAT || hi.is_some_and(|h| h > MAX_REPEAT) {
            return Err(UNSUPPORTED);
        }
        if hi.is_some_and(|h| h < lo) {
            return Err(INVALID);
        }
        self.out.push('{');
        self.out.push_str(&lo.to_string());
        if comma {
            self.out.push(',');
            if let Some(h) = hi {
                self.out.push_str(&h.to_string());
            }
        }
        self.out.push('}');
        Ok(hi.unwrap_or(lo).max(1))
    }

    fn run(&mut self) -> Result<(), &'static str> {
        // Size units of the expanded pattern, one entry per open group: a literal costs 1, a
        // class, class escape or `.` costs CLASS_UNITS, and `{n,m}` multiplies its atom by `m`
        // (by `n` when open). The total stays within MAX_REPEAT_UNITS, so the server's compiled
        // size stays within its limit wherever the browser accepts the pattern.
        let mut frames: Vec<u64> = vec![0];
        // Units of the last atom, the operand of a following quantifier.
        let mut atom: Option<u64> = None;
        while let Some(c) = self.next() {
            let units = match c {
                '\\' => match self.escape(false)? {
                    ClassItem::Char(cp) => {
                        push_code_point(&mut self.out, cp);
                        1
                    }
                    ClassItem::Set(set) => {
                        self.out.push_str(&set);
                        CLASS_UNITS
                    }
                },
                '[' => {
                    self.class()?;
                    CLASS_UNITS
                }
                '(' => {
                    if self.peek() == Some('?') {
                        self.next();
                        if self.next() != Some(':') {
                            return Err(UNSUPPORTED);
                        }
                        self.out.push_str("(?:");
                    } else {
                        self.out.push('(');
                    }
                    frames.push(0);
                    atom = None;
                    continue;
                }
                ')' => {
                    if frames.len() == 1 {
                        return Err(INVALID);
                    }
                    self.out.push(')');
                    frames.pop().unwrap_or(0)
                }
                '*' | '+' | '?' | '{' => {
                    let base = atom.ok_or(INVALID)?;
                    let bound = if c == '{' {
                        self.braces()?
                    } else {
                        self.out.push(c);
                        1
                    };
                    if let Some(top) = frames.last_mut() {
                        *top = top.saturating_add(base.saturating_mul(u64::from(bound) - 1));
                    }
                    if self.peek() == Some('?') {
                        self.next();
                        self.out.push('?');
                    }
                    atom = None;
                    if frames.iter().sum::<u64>() > MAX_REPEAT_UNITS {
                        return Err(UNSUPPORTED);
                    }
                    continue;
                }
                ']' | '}' => return Err(UNSUPPORTED),
                '^' | '$' | '|' => {
                    self.out.push(c);
                    atom = None;
                    continue;
                }
                '.' => {
                    self.out.push_str(ANY);
                    CLASS_UNITS
                }
                _ => {
                    push_code_point(&mut self.out, Self::literal_char(c)?);
                    1
                }
            };
            if let Some(top) = frames.last_mut() {
                *top = top.saturating_add(units);
            }
            atom = Some(units);
        }
        if frames.len() != 1 {
            return Err(INVALID);
        }
        if frames.iter().sum::<u64>() > MAX_REPEAT_UNITS {
            return Err(UNSUPPORTED);
        }
        Ok(())
    }
}

fn translate(pattern: &str) -> Result<String, &'static str> {
    let mut scanner = Scanner {
        chars: pattern.chars().peekable(),
        out: String::with_capacity(pattern.len() * 2),
    };
    scanner.run()?;
    Ok(scanner.out)
}

/// Checks a pattern against the subset without compiling it.
///
/// # Errors
/// `rule_param_invalid` over [`MAX_PATTERN_BYTES`], `pattern_unsupported` outside the subset,
/// `pattern_invalid` for a malformed pattern.
pub fn check_pattern(pattern: &str) -> Result<(), &'static str> {
    compile_pattern(pattern, false).map(|_| ())
}

/// Checks and compiles a pattern. Matching is unanchored; authors anchor with `^...$`.
///
/// # Errors
/// As [`check_pattern`]; `pattern_invalid` also when the translated pattern does not compile within
/// [`REGEX_SIZE_LIMIT`].
pub fn compile_pattern(pattern: &str, case_insensitive: bool) -> Result<Regex, &'static str> {
    if pattern.len() > MAX_PATTERN_BYTES {
        return Err("rule_param_invalid");
    }
    let translated = translate(pattern)?;
    RegexBuilder::new(&translated)
        .case_insensitive(case_insensitive)
        .size_limit(REGEX_SIZE_LIMIT)
        .build()
        .map_err(|_| INVALID)
}

/// Maps a subject to UTF-16 code units, one `char` per unit, so a BMP-only pattern matches it the
/// way an ECMAScript regex without the `u` flag (cel-js `matches`) matches the original string:
/// surrogate units become private-use characters of plane 15 that no subset pattern names.
pub fn utf16_view(subject: &str) -> String {
    subject
        .encode_utf16()
        .map(|unit| {
            let cp = if (0xD800..=0xDFFF).contains(&unit) {
                0xF_0000 + u32::from(unit - 0xD800)
            } else {
                u32::from(unit)
            };
            char::from_u32(cp).unwrap_or(char::REPLACEMENT_CHARACTER)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn code(p: &str) -> &'static str {
        match check_pattern(p) {
            Ok(()) => "ok",
            Err(c) => c,
        }
    }

    #[test]
    fn subset_verdicts() {
        assert_eq!(code(r"^\+?[0-9 ]+$"), "ok");
        assert_eq!(code(r"^(?:ab|cd){2,3}?$"), "ok");
        assert_eq!(code(r"[\d\-a-f]"), "ok");
        assert_eq!(code(r"\bword"), UNSUPPORTED);
        assert_eq!(code(r"(?i)abc"), UNSUPPORTED);
        assert_eq!(code(r"(?=a)"), UNSUPPORTED);
        assert_eq!(code(r"\p{L}"), UNSUPPORTED);
        assert_eq!(code(r"(a)\1"), UNSUPPORTED);
        assert_eq!(code(r"[[:alpha:]]"), UNSUPPORTED);
        assert_eq!(code(r"\u{41}"), UNSUPPORTED);
        assert_eq!(code("a]"), UNSUPPORTED);
        assert_eq!(code("😀"), UNSUPPORTED);
        assert_eq!(code("a**"), INVALID);
        assert_eq!(code("(ab"), INVALID);
        assert_eq!(code("[z-a]"), INVALID);
        assert_eq!(code("a{2,1}"), INVALID);
        assert_eq!(code("a{1001}"), UNSUPPORTED);
        assert_eq!(code("(?:a{10}){450}"), "ok");
        assert_eq!(code("(?:a{10}){451}"), UNSUPPORTED);
        assert_eq!(code(r"\S{450}"), "ok");
        assert_eq!(code(r"\S{451}"), UNSUPPORTED);
        assert_eq!(code(&"a".repeat(513)), "rule_param_invalid");
    }

    #[test]
    fn translation_matches_ecmascript() {
        let digits = compile_pattern(r"^\d+$", false);
        assert!(digits.as_ref().is_ok_and(|r| r.is_match("123")));
        assert!(digits.as_ref().is_ok_and(|r| !r.is_match("١٢٣")));
        let dot = compile_pattern("^.$", false);
        assert!(dot.as_ref().is_ok_and(|r| !r.is_match("\r")));
        assert!(dot.as_ref().is_ok_and(|r| r.is_match(&utf16_view("é"))));
        assert!(dot.as_ref().is_ok_and(|r| !r.is_match(&utf16_view("😀"))));
        let ci = compile_pattern("^abc$", true);
        assert!(ci.is_ok_and(|r| r.is_match("ABC")));
    }
}
