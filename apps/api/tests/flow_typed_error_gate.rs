//! Gate: every error the Flow surface constructs is a typed envelope.
//!
//! `error-mapping-v1.md` requires clients to branch on the stable `error_code` (plus its frozen
//! `details` discriminators), never on the English `message` or on the bare numeric `code`. The
//! legacy string-typed `ApiError` variants (`BadRequest(String)`, `Forbidden(String)`,
//! `NotFound(String)`, `Conflict(String)`, `Unauthorized(String)`, `Internal`, `Database(..)`)
//! serialize without an `error_code`, so every one of them that is reachable from a Flow route is
//! a response a contract-conforming client cannot classify.
//!
//! This test walks the Flow surface source (`src/flow/**`, `src/routes/flow.rs`,
//! `src/routes/collab.rs`), blanks comments, string/char literals and `#[cfg(test)]` items, and
//! lists every remaining mention of a legacy variant -- constructions *and* pattern matches, since
//! a `match err { ApiError::NotFound(_) => .. }` silently stops matching once the producer is
//! typed. A new non-typed construction point anywhere in that surface turns this test red.
//!
//! Not visible to a source scan, and therefore not claimed by this gate: an implicit
//! `From<sea_orm::DbErr>` conversion through `?`, and errors produced by shared helpers outside
//! the scanned tree (authentication middleware).
#![allow(clippy::print_stderr, clippy::panic, clippy::indexing_slicing, clippy::expect_used)]

use std::fmt::Write as _;
use std::path::{Path, PathBuf};

const LEGACY_VARIANTS: &[&str] = &[
    "BadRequest",
    "Unauthorized",
    "Forbidden",
    "NotFound",
    "Conflict",
    "Internal",
    "Database",
];

/// Replaces comments and the contents of string, byte-string, raw-string and char literals with
/// spaces, keeping every newline so line numbers survive.
fn blank_comments_and_literals(src: &str) -> String {
    let chars: Vec<char> = src.chars().collect();
    let mut out: Vec<char> = Vec::with_capacity(chars.len());
    let blank = |c: char| if c == '\n' { '\n' } else { ' ' };
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        let next = chars.get(i + 1).copied();
        // Line comment.
        if c == '/' && next == Some('/') {
            let end = chars[i..]
                .iter()
                .position(|ch| *ch == '\n')
                .map_or(chars.len(), |p| i + p);
            out.extend(std::iter::repeat_n(' ', end - i));
            i = end;
            continue;
        }
        // Block comment (nested).
        if c == '/' && next == Some('*') {
            let mut depth = 0usize;
            while i < chars.len() {
                if chars[i] == '/' && chars.get(i + 1) == Some(&'*') {
                    depth += 1;
                    out.extend(std::iter::repeat_n(' ', 2));
                    i += 2;
                } else if chars[i] == '*' && chars.get(i + 1) == Some(&'/') {
                    depth -= 1;
                    out.extend(std::iter::repeat_n(' ', 2));
                    i += 2;
                    if depth == 0 {
                        break;
                    }
                } else {
                    out.push(blank(chars[i]));
                    i += 1;
                }
            }
            continue;
        }
        let prev_is_ident = i > 0 && (chars[i - 1].is_alphanumeric() || chars[i - 1] == '_');
        // Raw string: r"..", r#".."#, br#".."#.
        if !prev_is_ident && (c == 'r' || (c == 'b' && next == Some('r'))) {
            let start = if c == 'b' { i + 2 } else { i + 1 };
            let mut j = start;
            while chars.get(j) == Some(&'#') {
                j += 1;
            }
            if chars.get(j) == Some(&'"') {
                let hashes = j - start;
                out.extend(std::iter::repeat_n(' ', j + 1 - i));
                i = blank_raw_string_body(&chars, j + 1, hashes, &mut out);
                continue;
            }
        }
        // String / byte string.
        if c == '"' || (c == 'b' && next == Some('"') && !prev_is_ident) {
            if c == 'b' {
                out.push(' ');
                i += 1;
            }
            out.push(' ');
            i += 1;
            while i < chars.len() {
                if chars[i] == '\\' {
                    out.push(' ');
                    if let Some(&escaped) = chars.get(i + 1) {
                        out.push(blank(escaped));
                    }
                    i += 2;
                    continue;
                }
                if chars[i] == '"' {
                    out.push(' ');
                    i += 1;
                    break;
                }
                out.push(blank(chars[i]));
                i += 1;
            }
            continue;
        }
        // Char literal (but not a lifetime).
        if c == '\'' {
            if next == Some('\\') {
                let mut j = i + 2;
                while j < chars.len() && chars[j] != '\'' {
                    j += 1;
                }
                out.extend(std::iter::repeat_n(' ', j.min(chars.len() - 1) + 1 - i));
                i = j + 1;
                continue;
            }
            if chars.get(i + 2) == Some(&'\'') {
                out.extend(std::iter::repeat_n(' ', 3));
                i += 3;
                continue;
            }
        }
        out.push(c);
        i += 1;
    }
    out.into_iter().collect()
}

/// Blanks a raw string's body starting at `i` (just past the opening quote) through its closing
/// quote and `hashes` hash marks; returns the index just past the literal.
fn blank_raw_string_body(chars: &[char], mut i: usize, hashes: usize, out: &mut Vec<char>) -> usize {
    while i < chars.len() {
        if chars[i] == '"' && (0..hashes).all(|h| chars.get(i + 1 + h) == Some(&'#')) {
            out.extend(std::iter::repeat_n(' ', hashes + 1));
            return i + 1 + hashes;
        }
        out.push(if chars[i] == '\n' { '\n' } else { ' ' });
        i += 1;
    }
    i
}

/// Index of the `}` closing the `{` at `open`, or the last index when unbalanced.
fn matching_brace(chars: &[char], open: usize) -> usize {
    let mut depth = 0i32;
    for (k, ch) in chars.iter().enumerate().skip(open) {
        match ch {
            '{' => depth += 1,
            '}' => {
                depth -= 1;
                if depth == 0 {
                    return k;
                }
            }
            _ => {}
        }
    }
    chars.len() - 1
}

/// Blanks every item annotated `#[cfg(test)]` (a test module, a test-only function, a test-only
/// statement). Must run on text whose comments and literals are already blanked, so braces are
/// structural.
fn blank_cfg_test_items(code: &str) -> String {
    const MARKER: &str = "#[cfg(test)]";
    let mut chars: Vec<char> = code.chars().collect();
    let marker: Vec<char> = MARKER.chars().collect();
    let mut i = 0;
    while i + marker.len() <= chars.len() {
        if chars[i..i + marker.len()] != marker[..] {
            i += 1;
            continue;
        }
        let start = i;
        let mut j = i + marker.len();
        let mut paren = 0i32;
        let mut end = None;
        while j < chars.len() {
            match chars[j] {
                '(' | '[' => paren += 1,
                ')' | ']' => paren -= 1,
                ';' if paren == 0 => {
                    end = Some(j);
                    break;
                }
                '{' if paren == 0 => {
                    end = Some(matching_brace(&chars, j));
                    break;
                }
                _ => {}
            }
            j += 1;
        }
        let end = end.unwrap_or(chars.len() - 1);
        for ch in &mut chars[start..=end] {
            if *ch != '\n' {
                *ch = ' ';
            }
        }
        i = end + 1;
    }
    chars.into_iter().collect()
}

/// Every `ApiError::<legacy variant>` mention in production code, as `(line, variant)`.
fn legacy_mentions(src: &str) -> Vec<(usize, &'static str)> {
    let code = blank_cfg_test_items(&blank_comments_and_literals(src));
    let mut found = Vec::new();
    for (line_index, line) in code.lines().enumerate() {
        let mut rest = line;
        while let Some(pos) = rest.find("ApiError::") {
            let before_ok = rest[..pos]
                .chars()
                .next_back()
                .is_none_or(|ch| !(ch.is_alphanumeric() || ch == '_'));
            let after = &rest[pos + "ApiError::".len()..];
            let ident: String = after
                .chars()
                .take_while(|ch| ch.is_alphanumeric() || *ch == '_')
                .collect();
            if before_ok && let Some(variant) = LEGACY_VARIANTS.iter().find(|v| **v == ident) {
                found.push((line_index + 1, *variant));
            }
            rest = &after[ident.len()..];
        }
    }
    found
}

fn collect_rs(dir: &Path, out: &mut Vec<PathBuf>) {
    let entries = std::fs::read_dir(dir).unwrap_or_else(|err| panic!("cannot read {}: {err}", dir.display()));
    for entry in entries {
        let path = entry.expect("directory entry is readable").path();
        if path.is_dir() {
            collect_rs(&path, out);
        } else if path.extension().is_some_and(|ext| ext == "rs") {
            out.push(path);
        }
    }
}

fn flow_surface_files() -> Vec<PathBuf> {
    let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut files = Vec::new();
    collect_rs(&src.join("flow"), &mut files);
    files.push(src.join("routes").join("flow.rs"));
    files.push(src.join("routes").join("collab.rs"));
    files.sort();
    files
}

#[test]
fn the_scanner_sees_legacy_mentions_and_ignores_comments_literals_and_test_items() {
    let sample = r##"
fn prod() -> Result<(), ApiError> {
    // ApiError::NotFound("in a comment")
    let _s = "ApiError::Forbidden(in a string)";
    let _r = r#"ApiError::Conflict"#;
    let _c = '"';
    if x { return Err(ApiError::BadRequest("real".into())); }
    match e { ApiError::NotFound(_) => {}, _ => {} }
    Err(ApiError::Internal)
}
#[cfg(test)]
mod tests {
    fn t() { let _ = ApiError::Conflict("test only".into()); }
}
#[cfg(test)]
inject().await?;
fn after() { let _ = ApiError::Typed { kind, message, details: None }; let _ = MyApiError::Internal; }
fn last() { let _ = ApiError::Database(err); }
"##;
    let found = legacy_mentions(sample);
    let variants: Vec<&str> = found.iter().map(|(_, v)| *v).collect();
    assert_eq!(
        variants,
        vec!["BadRequest", "NotFound", "Internal", "Database"],
        "{found:?}"
    );
    assert_eq!(found[0].0, 7);
    assert_eq!(found[3].0, 18);
}

#[test]
fn the_flow_surface_constructs_no_legacy_untyped_api_error() {
    let files = flow_surface_files();
    assert!(
        files.len() >= 30,
        "scan saw only {} files; the walk is broken",
        files.len()
    );

    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let mut report = String::new();
    let mut total = 0usize;
    let mut typed_mentions = 0usize;
    for path in &files {
        let src = std::fs::read_to_string(path).unwrap_or_else(|err| panic!("cannot read {}: {err}", path.display()));
        typed_mentions += src.matches("ApiError::Typed").count() + src.matches("ApiError::typed(").count();
        let rel = path.strip_prefix(root).unwrap_or(path).display().to_string();
        for (line, variant) in legacy_mentions(&src) {
            total += 1;
            let _ = writeln!(report, "  {rel}:{line}  ApiError::{variant}");
        }
    }
    // A scan that blanks everything would trivially pass; the surface must visibly use typed errors.
    assert!(
        typed_mentions > 0,
        "no typed ApiError found in the Flow surface; the scan is not reading source"
    );
    eprintln!(
        "flow typed-error gate: {total} non-typed ApiError mention(s) in {} files",
        files.len()
    );
    assert!(
        total == 0,
        "{total} non-typed ApiError mention(s) on the Flow surface (use ApiError::typed / the typed constructors):\n{report}"
    );
}
