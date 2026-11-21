//! Redirect URI matching (contract section 8.6) and the helpers that build redirect targets.
//!
//! A registered value is a complete URI. A requested value matches when it is byte-identical to a
//! registered value, or when both are `http` loopback URIs (`127.0.0.1` or `[::1]`, spelled
//! exactly so) with the same path, no user-info, no query, no fragment, the registered value has
//! no port and the requested port is 1024 to 65535 written in plain decimal digits (RFC 8252
//! section 7.3, OAuth 2.1 section 8.4.2). Nothing else is normalised: no case folding, no
//! trailing-slash or percent-encoding equivalence, no `localhost`.
//!
//! The parser is deliberately small and strict instead of a general URL library, because a
//! general parser normalises exactly the things this matcher must not.

/// Lowest port a loopback redirect may name.
pub const MIN_LOOPBACK_PORT: u16 = 1024;
/// Longest redirect URI accepted at all.
pub const MAX_REDIRECT_URI_LEN: usize = 512;

/// A loopback host literal.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LoopbackHost {
    V4,
    V6,
}

impl LoopbackHost {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::V4 => "127.0.0.1",
            Self::V6 => "[::1]",
        }
    }
}

/// The parts of a `http` loopback URI that matching looks at.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Loopback<'a> {
    host: LoopbackHost,
    port: Option<u16>,
    path: &'a str,
}

fn is_plain_ascii(text: &str) -> bool {
    text.bytes().all(|b| (0x21..=0x7e).contains(&b))
}

/// Parses `http://<127.0.0.1|[::1]>[:port]/<path>` with no user-info, query or fragment.
fn parse_loopback(uri: &str) -> Option<Loopback<'_>> {
    if uri.len() > MAX_REDIRECT_URI_LEN || !is_plain_ascii(uri) {
        return None;
    }
    let rest = uri.strip_prefix("http://")?;
    if rest.contains(['?', '#', '\\', '@']) {
        return None;
    }
    let slash = rest.find('/')?;
    let (authority, path) = rest.split_at(slash);
    let (host, port_text) = if let Some(after) = authority.strip_prefix("[::1]") {
        (LoopbackHost::V6, after)
    } else {
        (LoopbackHost::V4, authority.strip_prefix("127.0.0.1")?)
    };
    let port = if port_text.is_empty() {
        None
    } else {
        let digits = port_text.strip_prefix(':')?;
        // 1 to 5 plain digits, no sign, no leading zero: `08080` and `+8080` are other spellings.
        if digits.is_empty()
            || digits.len() > 5
            || digits.starts_with('0')
            || !digits.bytes().all(|b| b.is_ascii_digit())
        {
            return None;
        }
        Some(digits.parse::<u16>().ok()?)
    };
    Some(Loopback { host, port, path })
}

/// Whether `requested` matches one of the `registered` URIs.
pub fn matches_registered<S: AsRef<str>>(registered: &[S], requested: &str) -> bool {
    if requested.is_empty() || !is_plain_ascii(requested) || requested.len() > MAX_REDIRECT_URI_LEN {
        return false;
    }
    if registered.iter().any(|value| value.as_ref() == requested) {
        return true;
    }
    let Some(wanted) = parse_loopback(requested) else {
        return false;
    };
    let Some(port) = wanted.port else {
        // Identical-string matching above already covered the portless form.
        return false;
    };
    if port < MIN_LOOPBACK_PORT {
        return false;
    }
    registered.iter().any(|value| {
        parse_loopback(value.as_ref())
            .is_some_and(|have| have.port.is_none() && have.host == wanted.host && have.path == wanted.path)
    })
}

/// The host shown to the user on the consent screen for a validated redirect URI.
pub fn display_host(uri: &str) -> Option<&'static str> {
    parse_loopback(uri).map(|parsed| parsed.host.as_str())
}

fn hex_digit(nibble: u8) -> char {
    char::from_digit(u32::from(nibble), 16).map_or('0', |digit| digit.to_ascii_uppercase())
}

/// Percent-encodes everything except RFC 3986 unreserved characters.
pub fn percent_encode(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for byte in text.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.' | b'_' | b'~') {
            out.push(char::from(byte));
        } else {
            out.push('%');
            out.push(hex_digit(byte >> 4));
            out.push(hex_digit(byte & 0x0f));
        }
    }
    out
}

/// Appends response parameters to a validated redirect URI. A validated loopback URI has no
/// query, so the parameters start a new one. Values are encoded; `state` goes through untouched.
pub fn with_query(redirect_uri: &str, params: &[(&str, &str)]) -> String {
    let mut out = String::with_capacity(redirect_uri.len() + 64);
    out.push_str(redirect_uri);
    out.push(if redirect_uri.contains('?') { '&' } else { '?' });
    for (index, (name, value)) in params.iter().enumerate() {
        if index > 0 {
            out.push('&');
        }
        out.push_str(name);
        out.push('=');
        out.push_str(&percent_encode(value));
    }
    out
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::unwrap_used, clippy::panic, clippy::indexing_slicing)]
mod tests {
    use super::*;

    const REGISTERED: [&str; 2] = [
        "http://127.0.0.1/oauth/sylvode/callback",
        "http://[::1]/oauth/sylvode/callback",
    ];

    fn ok(uri: &str) -> bool {
        matches_registered(&REGISTERED, uri)
    }

    #[test]
    fn any_loopback_port_from_1024_matches_for_both_literals() {
        assert!(ok("http://127.0.0.1:1024/oauth/sylvode/callback"));
        assert!(ok("http://127.0.0.1:54321/oauth/sylvode/callback"));
        assert!(ok("http://127.0.0.1:65535/oauth/sylvode/callback"));
        assert!(ok("http://[::1]:8080/oauth/sylvode/callback"));
        assert!(ok("http://127.0.0.1/oauth/sylvode/callback"), "byte-identical");
        assert!(ok("http://[::1]/oauth/sylvode/callback"), "byte-identical");
    }

    #[test]
    fn port_rules() {
        for port in [
            "1023", "80", "0", "1", "65536", "99999", "100000", "-1", "+8080", "08080", "8080a", "", " 8080",
        ] {
            assert!(
                !ok(&format!("http://127.0.0.1:{port}/oauth/sylvode/callback")),
                "port {port:?}"
            );
        }
        assert!(!ok("http://127.0.0.1:/oauth/sylvode/callback"), "empty port");
        assert!(!ok("http://127.0.0.1:8080:9090/oauth/sylvode/callback"));
        assert!(!ok("http://127.0.0.1:0x1f90/oauth/sylvode/callback"));
        assert!(
            !ok("http://127.0.0.1:８０８０/oauth/sylvode/callback"),
            "full width digits"
        );
    }

    #[test]
    fn hosts_other_than_the_exact_literals_are_refused() {
        for host in [
            "localhost",
            "LOCALHOST",
            "127.0.0.2",
            "127.1",
            "2130706433",
            "0x7f.0.0.1",
            "0177.0.0.1",
            "127.0.0.1.",
            "127.0.0.1.evil.example",
            "evil.example",
            "[::ffff:127.0.0.1]",
            "[0:0:0:0:0:0:0:1]",
            "[::1].",
            "[::1",
            "::1",
            "[::2]",
            "0.0.0.0",
            "",
        ] {
            assert!(
                !ok(&format!("http://{host}:8080/oauth/sylvode/callback")),
                "host {host:?}"
            );
        }
    }

    #[test]
    fn userinfo_query_fragment_and_backslash_are_refused() {
        for uri in [
            "http://user@127.0.0.1:8080/oauth/sylvode/callback",
            "http://user:pw@127.0.0.1:8080/oauth/sylvode/callback",
            "http://evil.example@127.0.0.1:8080/oauth/sylvode/callback",
            "http://127.0.0.1:8080@evil.example/oauth/sylvode/callback",
            "http://127.0.0.1:8080/oauth/sylvode/callback?x=1",
            "http://127.0.0.1:8080/oauth/sylvode/callback?",
            "http://127.0.0.1:8080/oauth/sylvode/callback#frag",
            "http://127.0.0.1:8080/oauth/sylvode/callback#",
            "http://127.0.0.1:8080\\@evil.example/oauth/sylvode/callback",
            "http://127.0.0.1:8080/oauth\\sylvode/callback",
            "http://127.0.0.1:8080?/oauth/sylvode/callback",
            "http://127.0.0.1:8080#/oauth/sylvode/callback",
        ] {
            assert!(!ok(uri), "{uri}");
        }
    }

    #[test]
    fn path_must_be_identical_with_no_normalisation() {
        for uri in [
            "http://127.0.0.1:8080/oauth/sylvode/callback/",
            "http://127.0.0.1:8080/oauth/sylvode/Callback",
            "http://127.0.0.1:8080/OAUTH/sylvode/callback",
            "http://127.0.0.1:8080/oauth/sylvode/callbac%6b",
            "http://127.0.0.1:8080/oauth%2Fsylvode/callback",
            "http://127.0.0.1:8080/oauth/sylvode/../sylvode/callback",
            "http://127.0.0.1:8080/oauth/./sylvode/callback",
            "http://127.0.0.1:8080//oauth/sylvode/callback",
            "http://127.0.0.1:8080/oauth/sylvode/callback;x",
            "http://127.0.0.1:8080/oauth/sylvode/callback%00",
            "http://127.0.0.1:8080/other",
            "http://127.0.0.1:8080",
            "http://127.0.0.1:8080/",
        ] {
            assert!(!ok(uri), "{uri}");
        }
    }

    #[test]
    fn scheme_must_be_exactly_http() {
        for uri in [
            "https://127.0.0.1:8080/oauth/sylvode/callback",
            "HTTP://127.0.0.1:8080/oauth/sylvode/callback",
            "Http://127.0.0.1:8080/oauth/sylvode/callback",
            "http:127.0.0.1:8080/oauth/sylvode/callback",
            "http:///127.0.0.1:8080/oauth/sylvode/callback",
            "//127.0.0.1:8080/oauth/sylvode/callback",
            "ramflux://127.0.0.1:8080/oauth/sylvode/callback",
            "javascript://127.0.0.1:8080/oauth/sylvode/callback",
            "127.0.0.1:8080/oauth/sylvode/callback",
            "/oauth/sylvode/callback",
        ] {
            assert!(!ok(uri), "{uri}");
        }
    }

    #[test]
    fn whitespace_control_characters_non_ascii_and_oversize_are_refused() {
        for uri in [
            "http://127.0.0.1:8080/oauth/sylvode/callback ",
            " http://127.0.0.1:8080/oauth/sylvode/callback",
            "http://127.0.0.1:8080/oauth/sylvode/callback\n",
            "http://127.0.0.1:8080/oauth/sylvode/call\tback",
            "http://127.0.0.1:8080/oauth/sylvode/callback\0",
            "http://127.0.0.1:8080/oauth/sylvode/callbäck",
            "",
        ] {
            assert!(!ok(uri), "{uri:?}");
        }
        let long = format!("http://127.0.0.1:8080/{}", "a".repeat(MAX_REDIRECT_URI_LEN));
        assert!(!ok(&long));
    }

    #[test]
    fn a_registered_value_with_a_port_only_matches_itself() {
        let registered = ["http://127.0.0.1:9000/cb"];
        assert!(matches_registered(&registered, "http://127.0.0.1:9000/cb"));
        assert!(!matches_registered(&registered, "http://127.0.0.1:9001/cb"));
    }

    #[test]
    fn the_host_family_must_match_the_registered_one() {
        let only_v4 = ["http://127.0.0.1/cb"];
        assert!(matches_registered(&only_v4, "http://127.0.0.1:5000/cb"));
        assert!(!matches_registered(&only_v4, "http://[::1]:5000/cb"));
        let none: [&str; 0] = [];
        assert!(!matches_registered(&none, "http://127.0.0.1:5000/cb"));
    }

    #[test]
    fn display_host_names_the_loopback_literal() {
        assert_eq!(display_host("http://127.0.0.1:5000/cb"), Some("127.0.0.1"));
        assert_eq!(display_host("http://[::1]:5000/cb"), Some("[::1]"));
        assert_eq!(display_host("https://example.test/cb"), None);
    }

    #[test]
    fn query_building_encodes_values_and_keeps_state_recoverable() {
        assert_eq!(percent_encode("aZ09-._~"), "aZ09-._~");
        assert_eq!(percent_encode("a b&c=d/é"), "a%20b%26c%3Dd%2F%C3%A9");
        let url = with_query(
            "http://127.0.0.1:5000/cb",
            &[
                ("error", "access_denied"),
                ("state", "a&b=c d"),
                ("iss", "http://127.0.0.1:18097"),
            ],
        );
        assert_eq!(
            url,
            "http://127.0.0.1:5000/cb?error=access_denied&state=a%26b%3Dc%20d&iss=http%3A%2F%2F127.0.0.1%3A18097"
        );
    }
}
