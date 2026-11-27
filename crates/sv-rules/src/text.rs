//! Text checks shared with the TypeScript mirror (`text.ts`). Both sides implement the same
//! hand-written grammar so the verdicts agree without a regex engine.

use time::{Date, Month};

const MAX_EMAIL_LEN: usize = 254;
const MAX_LOCAL_LEN: usize = 64;
const MAX_HOST_LEN: usize = 253;
const MAX_LABEL_LEN: usize = 63;
const MAX_URL_LEN: usize = 2048;

fn is_local_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || "!#$%&'*+/=?^_`{|}~.-".contains(c)
}

/// A host name: dot-separated labels of ASCII letters, digits and `-` (not at either end of a
/// label), 1..63 characters each, 253 characters in all. IPv4 addresses pass as numeric labels.
fn is_host_name(host: &str) -> bool {
    if host.is_empty() || host.len() > MAX_HOST_LEN {
        return false;
    }
    host.split('.').all(|label| {
        !label.is_empty()
            && label.len() <= MAX_LABEL_LEN
            && label.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
            && !label.starts_with('-')
            && !label.ends_with('-')
    })
}

/// The `email` rule: one `@`; local part of 1..64 characters from the RFC 5322 atom set plus `.`
/// (not leading, trailing or doubled); a host name with at least two labels whose last label is not
/// all digits; 254 characters in all. Non-ASCII addresses are refused.
#[must_use]
pub fn is_email(s: &str) -> bool {
    if s.len() > MAX_EMAIL_LEN || !s.is_ascii() {
        return false;
    }
    let mut parts = s.split('@');
    let (Some(local), Some(domain), None) = (parts.next(), parts.next(), parts.next()) else {
        return false;
    };
    if local.is_empty()
        || local.len() > MAX_LOCAL_LEN
        || !local.chars().all(is_local_char)
        || local.starts_with('.')
        || local.ends_with('.')
        || local.contains("..")
    {
        return false;
    }
    if !is_host_name(domain) {
        return false;
    }
    let mut labels = domain.rsplit('.');
    let last = labels.next().unwrap_or_default();
    labels.next().is_some() && !last.chars().all(|c| c.is_ascii_digit())
}

fn is_port(p: &str) -> bool {
    !p.is_empty()
        && p.len() <= 5
        && p.chars().all(|c| c.is_ascii_digit())
        && p.parse::<u32>().is_ok_and(|n| (1..=65_535).contains(&n))
}

fn is_ipv6_literal(inner: &str) -> bool {
    !inner.is_empty() && inner.contains(':') && inner.chars().all(|c| c.is_ascii_hexdigit() || c == ':' || c == '.')
}

fn is_authority(authority: &str) -> bool {
    if authority.is_empty() || authority.contains('@') {
        return false;
    }
    if let Some(rest) = authority.strip_prefix('[') {
        let Some((inner, after)) = rest.split_once(']') else {
            return false;
        };
        if !is_ipv6_literal(inner) {
            return false;
        }
        return after.is_empty() || after.strip_prefix(':').is_some_and(is_port);
    }
    match authority.split_once(':') {
        Some((host, port)) => is_host_name(host) && is_port(port),
        None => is_host_name(authority),
    }
}

/// The `url` rule: `<scheme>://<authority>[<path, query, fragment>]` where the scheme (ASCII case
/// ignored) is one of `schemes`, the authority has no user info and is a host name, IPv4 address
/// or bracketed IPv6 literal with an optional port 1..65535, and the whole string has no whitespace
/// or control characters and at most 2048 characters. Non-ASCII hosts must be given in punycode.
#[must_use]
pub fn is_url(s: &str, schemes: &[&str]) -> bool {
    if s.chars().count() > MAX_URL_LEN || s.chars().any(|c| c.is_control() || c.is_whitespace() || c == '\u{7f}') {
        return false;
    }
    let Some((scheme, rest)) = s.split_once("://") else {
        return false;
    };
    let scheme = scheme.to_ascii_lowercase();
    if !schemes.iter().any(|allowed| *allowed == scheme) {
        return false;
    }
    let end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
    rest.get(..end).is_some_and(is_authority)
}

/// A strict `YYYY-MM-DD` date with a year in 0001..9999.
pub fn parse_iso_date(s: &str) -> Option<Date> {
    let b = s.as_bytes();
    if b.len() != 10 || b.get(4) != Some(&b'-') || b.get(7) != Some(&b'-') {
        return None;
    }
    let num = |r: std::ops::Range<usize>| -> Option<u16> {
        let part = s.get(r)?;
        if !part.bytes().all(|c| c.is_ascii_digit()) {
            return None;
        }
        part.parse().ok()
    };
    let year = num(0..4)?;
    let month = u8::try_from(num(5..7)?).ok()?;
    let day = u8::try_from(num(8..10)?).ok()?;
    if year == 0 {
        return None;
    }
    Date::from_calendar_date(i32::from(year), Month::try_from(month).ok()?, day).ok()
}

/// Unicode scalar value count, the length used by `min_length`, `max_length` and CEL `size`.
pub fn scalar_len(s: &str) -> usize {
    s.chars().count()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn email_grammar() {
        assert!(is_email("a.b+c@example.com"));
        assert!(!is_email("a..b@example.com"));
        assert!(!is_email("a@b"));
        assert!(!is_email("a@1.2"));
        assert!(!is_email("a@@b.com"));
        assert!(!is_email("ä@b.com"));
    }

    #[test]
    fn url_grammar() {
        let https = ["https"];
        assert!(is_url("https://example.com/a?b#c", &https));
        assert!(is_url("HTTPS://example.com:8443", &https));
        assert!(is_url("https://[::1]:8080/", &https));
        assert!(!is_url("http://example.com", &https));
        assert!(!is_url("https://user@example.com", &https));
        assert!(!is_url("https://exa mple.com", &https));
        assert!(!is_url("https://", &https));
        assert!(!is_url("https://example.com:0", &https));
    }
}
