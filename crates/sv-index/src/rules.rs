//! Field-level rules: identifiers, display text, localised text, URLs, image paths, version ranges.

use std::collections::BTreeSet;

use semver::{Comparator, Op, Prerelease, Version, VersionReq};
use unicode_normalization::is_nfc;
use url::{Host, Url};

use crate::error::{Issues, member};
use crate::limits;
use crate::model::I18nText;

/// Decides which download hosts a source accepts (the operator's `index_download_hosts`).
pub trait HostPolicy {
    /// True when `host` (lower-case, punycode) may serve packages.
    fn allows(&self, host: &str) -> bool;
}

/// Any public https host.
#[derive(Debug, Clone, Copy, Default)]
pub struct AnyHost;

impl HostPolicy for AnyHost {
    fn allows(&self, _host: &str) -> bool {
        true
    }
}

/// Exact host names; an empty list allows any host.
#[derive(Debug, Clone, Default)]
pub struct ExactHosts(pub Vec<String>);

impl HostPolicy for ExactHosts {
    fn allows(&self, host: &str) -> bool {
        self.0.is_empty() || self.0.iter().any(|h| h.eq_ignore_ascii_case(host))
    }
}

impl<F: Fn(&str) -> bool> HostPolicy for F {
    fn allows(&self, host: &str) -> bool {
        self(host)
    }
}

const fn lower_alnum(b: u8) -> bool {
    b.is_ascii_lowercase() || b.is_ascii_digit()
}

/// `^[a-z0-9][a-z0-9-]{min-1..max-1}$`.
fn dns_label(s: &str, max: usize) -> bool {
    let b = s.as_bytes();
    (1..=max).contains(&b.len())
        && b.first().copied().is_some_and(lower_alnum)
        && b.iter().all(|c| lower_alnum(*c) || *c == b'-')
}

/// Publisher id: `^[a-z0-9][a-z0-9-]{0,38}$`, `sv` reserved.
pub fn publisher_id_problem(s: &str) -> Option<&'static str> {
    if !dns_label(s, 39) {
        Some("publisher id must match ^[a-z0-9][a-z0-9-]{0,38}$")
    } else if s == "sv" {
        Some("publisher id `sv` is reserved")
    } else {
        None
    }
}

/// Plugin id `<publisher>/<name>`; returns the publisher part.
pub fn plugin_id_publisher(id: &str) -> Option<&str> {
    let (p, n) = id.split_once('/')?;
    (publisher_id_problem(p).is_none() && dns_label(n, 64)).then_some(p)
}

/// `^[a-z][a-z0-9-]{0,31}$` for category and label ids.
pub fn meta_id_ok(s: &str) -> bool {
    let b = s.as_bytes();
    (1..=32).contains(&b.len())
        && b.first().is_some_and(u8::is_ascii_lowercase)
        && b.iter().all(|c| lower_alnum(*c) || *c == b'-')
}

/// GitHub login: 1..39 ASCII alphanumerics or hyphens, not starting or ending with a hyphen.
pub fn github_login_ok(s: &str) -> bool {
    let b = s.as_bytes();
    (1..=39).contains(&b.len())
        && b.iter().all(|c| c.is_ascii_alphanumeric() || *c == b'-')
        && b.first().is_some_and(u8::is_ascii_alphanumeric)
        && b.last().is_some_and(u8::is_ascii_alphanumeric)
}

/// `prefix` + exactly `len` lower-case hex digits.
pub fn hex_tail(s: &str, prefix: &str, len: usize) -> bool {
    s.strip_prefix(prefix)
        .is_some_and(|h| h.len() == len && h.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)))
}

/// `ed25519:` + 32 hex.
pub fn key_id_ok(s: &str) -> bool {
    hex_tail(s, "ed25519:", 32)
}

/// 64 lower-case hex characters.
pub fn sha256_ok(s: &str) -> bool {
    hex_tail(s, "", 64)
}

/// `sha256:` + 64 hex.
pub fn digest_ok(s: &str) -> bool {
    hex_tail(s, "sha256:", 64)
}

/// Unicode general category Cf plus the line and paragraph separators: invisible or direction-changing.
pub(crate) fn is_format_char(c: char) -> bool {
    matches!(
        u32::from(c),
        0x00AD
            | 0x0600..=0x0605
            | 0x061C
            | 0x06DD
            | 0x070F
            | 0x0890..=0x0891
            | 0x08E2
            | 0x180E
            | 0x200B..=0x200F
            | 0x2028..=0x202E
            | 0x2060..=0x2064
            | 0x2066..=0x206F
            | 0xFEFF
            | 0xFFF9..=0xFFFB
            | 0x110BD
            | 0x110CD
            | 0x13430..=0x1343F
            | 0x1BCA0..=0x1BCA3
            | 0x1D173..=0x1D17A
            | 0xE0001
            | 0xE0020..=0xE007F
    )
}

/// Whether a text may contain line breaks and tabs (descriptions and changelogs).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Text {
    /// One line.
    Line,
    /// `CommonMark`: `\n` and `\t` allowed.
    Block,
}

/// Why a text is not acceptable, if it is not: NFC, trimmed, 1..=`max` characters, no control or
/// bidi-override characters.
pub fn text_problem(text: &str, max: usize, kind: Text) -> Option<String> {
    let chars = text.chars().count();
    if chars == 0 || text.trim().is_empty() {
        return Some("text is empty".to_owned());
    }
    if chars > max {
        return Some(format!("text has {chars} characters, at most {max} allowed"));
    }
    if text.trim() != text {
        return Some("text has leading or trailing whitespace".to_owned());
    }
    if !is_nfc(text) {
        return Some("text is not NFC-normalised".to_owned());
    }
    let allowed = |c: char| kind == Text::Block && (c == '\n' || c == '\t');
    if text
        .chars()
        .any(|c| (c.is_control() && !allowed(c)) || is_format_char(c))
    {
        return Some("text has control or bidi-override characters".to_owned());
    }
    None
}

/// Language key `xx` or `xx-XX`.
fn lang_ok(lang: &str) -> bool {
    match lang.as_bytes() {
        [a, b] => a.is_ascii_lowercase() && b.is_ascii_lowercase(),
        [a, b, b'-', c, d] => {
            a.is_ascii_lowercase() && b.is_ascii_lowercase() && c.is_ascii_uppercase() && d.is_ascii_uppercase()
        }
        _ => false,
    }
}

/// Checks a localised text map: `en` present, at most 8 keys, key grammar, every text valid.
pub fn check_i18n(issues: &mut Issues, path: &str, map: &I18nText, max: usize, kind: Text) {
    if map.len() > limits::MAX_LANGS {
        issues.push(path, format!("at most {} languages allowed", limits::MAX_LANGS));
    }
    if !map.contains_key("en") {
        issues.push(path, "`en` is required");
    }
    for (lang, text) in map {
        let p = member(path, lang);
        if !lang_ok(lang) {
            issues.push(&p, "language key must match ^[a-z]{2}(-[A-Z]{2})?$");
        }
        if let Some(why) = text_problem(text, max, kind) {
            issues.push(&p, why);
        }
    }
}

/// Checks a single display string.
pub fn check_text(issues: &mut Issues, path: &str, text: &str, max: usize) {
    if let Some(why) = text_problem(text, max, Text::Line) {
        issues.push(path, why);
    }
}

fn url_shape_problem(raw: &str) -> Option<&'static str> {
    if raw.chars().count() > limits::MAX_URL_CHARS {
        return Some("URL is longer than 512 characters");
    }
    // The URL parser silently drops tabs and newlines; refuse them and any non-printable input.
    if !raw.bytes().all(|b| (0x21..0x7f).contains(&b)) {
        return Some("URL must be printable ASCII without spaces");
    }
    None
}

fn parse_https(raw: &str) -> Result<(Url, String), &'static str> {
    if let Some(why) = url_shape_problem(raw) {
        return Err(why);
    }
    let url = Url::parse(raw).map_err(|_| "URL does not parse")?;
    if url.scheme() != "https" {
        return Err("URL must use https");
    }
    if !url.username().is_empty() || url.password().is_some() {
        return Err("URL must not carry credentials");
    }
    let host = match url.host() {
        Some(Host::Domain(d)) if !d.is_empty() => d.to_owned(),
        Some(Host::Domain(_)) | None => return Err("URL has no host"),
        Some(Host::Ipv4(_) | Host::Ipv6(_)) => return Err("URL host must be a DNS name, not an IP address"),
    };
    Ok((url, host))
}

/// Download URL rules: https, port 443 or none, DNS-name host accepted by `hosts`, no credentials,
/// query or fragment, at most 512 characters.
pub fn download_url_problem(raw: &str, hosts: &dyn HostPolicy) -> Option<String> {
    let (url, host) = match parse_https(raw) {
        Ok(v) => v,
        Err(why) => return Some(why.to_owned()),
    };
    if url.port().is_some() {
        return Some("URL must use port 443 or none".to_owned());
    }
    if url.query().is_some() || url.fragment().is_some() {
        return Some("URL must not have a query or fragment".to_owned());
    }
    if !hosts.allows(&host) {
        return Some(format!("download host `{host}` is not allowed by the source"));
    }
    None
}

/// Rules for informational links (homepage, repository): https, DNS-name host, no credentials.
pub fn web_url_problem(raw: &str) -> Option<String> {
    parse_https(raw).err().map(str::to_owned)
}

/// Image file formats of the index.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ImageFormat {
    /// PNG.
    Png,
    /// JPEG.
    Jpeg,
    /// WebP.
    Webp,
}

impl ImageFormat {
    /// The format a lower-case file extension names.
    #[must_use]
    pub fn from_extension(ext: &str) -> Option<Self> {
        match ext {
            "png" => Some(Self::Png),
            "jpg" | "jpeg" => Some(Self::Jpeg),
            "webp" => Some(Self::Webp),
            _ => None,
        }
    }
}

/// The lower-case extension of a path's last segment.
fn extension(path: &str) -> Option<&str> {
    path.rsplit('/').next()?.rsplit_once('.').map(|(_, e)| e)
}

/// A relative path inside the plugin directory: no URL, no `..`, no absolute path, no backslash,
/// conservative characters, a known image extension.
fn relative_image_problem(path: &str) -> Option<&'static str> {
    if path.is_empty() || path.chars().count() > limits::MAX_PATH_CHARS {
        return Some("path must be 1..200 characters");
    }
    if path.starts_with('/') || path.contains(':') || path.contains('\\') {
        return Some("path must be relative to the plugin directory (no URL, drive or absolute path)");
    }
    if !path
        .bytes()
        .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-' | b'/'))
    {
        return Some("path may use only letters, digits, `.`, `_`, `-` and `/`");
    }
    if path
        .split('/')
        .any(|s| s.is_empty() || s == "." || s == ".." || s.starts_with('.'))
    {
        return Some("path must not have empty, hidden or dot segments");
    }
    match extension(path).and_then(ImageFormat::from_extension) {
        Some(_) => None,
        None => Some("image must be .png, .jpg, .jpeg or .webp (lower case); SVG is not allowed"),
    }
}

/// Icon path: `icon.png` or `icon.webp` in the plugin directory.
pub fn icon_path_problem(path: &str) -> Option<&'static str> {
    if let Some(why) = relative_image_problem(path) {
        return Some(why);
    }
    if matches!(path, "icon.png" | "icon.webp") {
        None
    } else {
        Some("icon must be `icon.png` or `icon.webp`")
    }
}

/// Screenshot path: `screenshots/<file>` with an image extension.
pub fn screenshot_path_problem(path: &str) -> Option<&'static str> {
    if let Some(why) = relative_image_problem(path) {
        return Some(why);
    }
    let ok = path
        .strip_prefix("screenshots/")
        .is_some_and(|rest| !rest.contains('/'));
    (!ok).then_some("screenshot must be `screenshots/<file>`")
}

/// Parses an exact version: semver without build metadata.
pub fn parse_exact_version(s: &str) -> Result<Version, String> {
    let v = Version::parse(s).map_err(|e| format!("not a semver version: {e}"))?;
    if !v.build.is_empty() {
        return Err("build metadata is not allowed".to_owned());
    }
    Ok(v)
}

/// Lower bound a single comparator implies.
fn comparator_floor(c: &Comparator) -> Version {
    let (minor, patch) = (c.minor.unwrap_or(0), c.patch.unwrap_or(0));
    let at = |major, minor, patch| Version {
        major,
        minor,
        patch,
        pre: Prerelease::EMPTY,
        build: semver::BuildMetadata::EMPTY,
    };
    match c.op {
        Op::Greater => match (c.minor, c.patch) {
            (None, _) => at(c.major.saturating_add(1), 0, 0),
            (Some(m), None) => at(c.major, m.saturating_add(1), 0),
            (Some(m), Some(p)) => at(c.major, m, p.saturating_add(1)),
        },
        Op::GreaterEq | Op::Exact | Op::Tilde | Op::Caret => Version {
            major: c.major,
            minor,
            patch,
            pre: c.pre.clone(),
            build: semver::BuildMetadata::EMPTY,
        },
        _ => at(0, 0, 0),
    }
}

/// The lowest version `req` admits, or `None` when it admits none (contradictory bounds).
///
/// Exact for the lower-bound forms (`>=`, `>`, `^`, `~`, `=`, wildcards) and their comma-joined
/// conjunctions; used to compare against the manifest's `sv.min`.
#[must_use]
pub fn lowest_admitted(req: &VersionReq) -> Option<Version> {
    let floor = req
        .comparators
        .iter()
        .map(comparator_floor)
        .max()
        .unwrap_or_else(|| Version::new(0, 0, 0));
    req.matches(&floor).then_some(floor)
}

/// Parses a `sylvode` compatibility range and checks that it admits some version.
pub fn parse_compat(s: &str) -> Result<VersionReq, String> {
    if s.chars().count() > limits::MAX_CONTENT_CHARS {
        return Err("range is too long".to_owned());
    }
    let req = VersionReq::parse(s).map_err(|e| format!("not a semver range: {e}"))?;
    if lowest_admitted(&req).is_none() {
        return Err("range admits no version".to_owned());
    }
    Ok(req)
}

/// Reports duplicates in a list of identifiers.
pub fn check_unique<'a>(issues: &mut Issues, path: &str, items: impl Iterator<Item = &'a String>, what: &str) {
    let mut seen = BTreeSet::new();
    for (i, s) in items.enumerate() {
        if !seen.insert(s.as_str()) {
            issues.push(&crate::error::element(path, i), format!("duplicate {what} `{s}`"));
        }
    }
}
