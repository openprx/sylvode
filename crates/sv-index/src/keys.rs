//! Strict-mode member-name rules that a parsed `serde_json::Value` cannot show: duplicate members
//! (the parser keeps the last one) and member names written with escapes (they hide the real name
//! from a reviewer reading the file).
//!
//! The input has already parsed as JSON, so the scanner only follows its structure; it stays
//! within the nesting `serde_json` accepts.

use std::collections::BTreeSet;

use crate::error::{Issues, element, member};

const MAX_DEPTH: usize = 128;

/// Records every duplicate or escaped member name in `bytes`.
pub fn check_member_names(bytes: &[u8], issues: &mut Issues) {
    let mut scanner = Scanner { bytes, pos: 0, issues };
    scanner.value("$", 0);
}

struct Scanner<'a, 'i> {
    bytes: &'a [u8],
    pos: usize,
    issues: &'i mut Issues,
}

impl Scanner<'_, '_> {
    fn peek(&self) -> Option<u8> {
        self.bytes.get(self.pos).copied()
    }

    fn skip_space(&mut self) {
        while matches!(self.peek(), Some(b' ' | b'\t' | b'\n' | b'\r')) {
            self.pos += 1;
        }
    }

    /// Skips a string; returns its raw bytes including the quotes.
    fn string(&mut self) -> &[u8] {
        let start = self.pos;
        self.pos += 1;
        while let Some(b) = self.peek() {
            self.pos += 1;
            match b {
                b'\\' => self.pos += 1,
                b'"' => break,
                _ => {}
            }
        }
        self.bytes
            .get(start..self.pos.min(self.bytes.len()))
            .unwrap_or_default()
    }

    fn value(&mut self, path: &str, depth: usize) {
        self.skip_space();
        if depth > MAX_DEPTH {
            self.pos = self.bytes.len();
            return;
        }
        match self.peek() {
            Some(b'{') => self.object(path, depth),
            Some(b'[') => self.array(path, depth),
            Some(b'"') => {
                self.string();
            }
            Some(_) => {
                while !matches!(
                    self.peek(),
                    None | Some(b',' | b'}' | b']' | b' ' | b'\t' | b'\n' | b'\r')
                ) {
                    self.pos += 1;
                }
            }
            None => {}
        }
    }

    fn array(&mut self, path: &str, depth: usize) {
        self.pos += 1;
        let mut index = 0;
        loop {
            self.skip_space();
            match self.peek() {
                Some(b']') | None => break,
                Some(b',') => self.pos += 1,
                Some(_) => {
                    self.value(&element(path, index), depth + 1);
                    index += 1;
                }
            }
        }
        self.pos += 1;
    }

    fn object(&mut self, path: &str, depth: usize) {
        self.pos += 1;
        let mut seen: BTreeSet<String> = BTreeSet::new();
        loop {
            self.skip_space();
            match self.peek() {
                Some(b'}') | None => break,
                Some(b'"') => {
                    let raw = self.string().to_vec();
                    let name = serde_json::from_slice::<String>(&raw).unwrap_or_default();
                    let child = member(path, &name);
                    if raw.contains(&b'\\') {
                        self.issues.push(&child, "member names must not use escape sequences");
                    }
                    if !seen.insert(name) {
                        self.issues.push(&child, "duplicate member");
                    }
                    self.skip_space();
                    if self.peek() == Some(b':') {
                        self.pos += 1;
                    }
                    self.value(&child, depth + 1);
                }
                Some(_) => self.pos += 1,
            }
        }
        self.pos += 1;
    }
}
