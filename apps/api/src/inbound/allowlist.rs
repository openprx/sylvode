//! The optional source IP allow-list: validation of what an administrator enters and the check at
//! ingest.
//!
//! An empty list means "any address". A non-empty list admits a request only when the resolved
//! client address lies inside one of its blocks. The address is the one every other limiter of the
//! API uses (`oauth::rate::client_address`, which is `security::client_ip` under the configured
//! `server.trusted_proxies`): the TCP peer, unless the peer is a trusted proxy, in which case the
//! right-most untrusted `X-Forwarded-For` entry. A forwarding header sent by a client that is not
//! behind a trusted proxy is never believed.

use std::net::IpAddr;

use platform::config::IpCidr;

/// Most blocks one source may list.
pub const MAX_ENTRIES: usize = 64;

/// Checks the entries an administrator submitted and returns them in the form stored: one
/// `address/prefix` block each, duplicates removed, first occurrence kept.
///
/// A bare address is a single-host block. An entry with bits set to the right of its prefix
/// (`192.0.2.5/24`) is refused rather than silently widened or narrowed, and so is a prefix of 0,
/// which is the same as no list: clear the list instead.
pub fn normalise_entries(entries: &[String]) -> Result<Vec<String>, String> {
    if entries.len() > MAX_ENTRIES {
        return Err(format!("ip_allowlist holds at most {MAX_ENTRIES} entries"));
    }
    let mut stored: Vec<String> = Vec::with_capacity(entries.len());
    for entry in entries {
        let text = entry.trim();
        let block = IpCidr::parse(text).map_err(|reason| format!("ip_allowlist entry {reason}"))?;
        let rendered = block.to_string();
        let Some((network, prefix)) = rendered.rsplit_once('/') else {
            return Err(format!("ip_allowlist entry {text:?} is not a CIDR block"));
        };
        if prefix == "0" {
            return Err(format!(
                "ip_allowlist entry {text:?} would admit every address; leave the list empty instead"
            ));
        }
        let given = text.split_once('/').map_or(text, |(address, _)| address);
        let same_network = match (given.parse::<IpAddr>(), network.parse::<IpAddr>()) {
            (Ok(given), Ok(network)) => given == network,
            _ => false,
        };
        if !same_network {
            return Err(format!(
                "ip_allowlist entry {text:?} has bits set beyond its prefix; write {rendered}"
            ));
        }
        if !stored.contains(&rendered) {
            stored.push(rendered);
        }
    }
    Ok(stored)
}

/// Whether `client` may use a source whose stored list is `stored`.
///
/// The entries are `address/prefix` text, as the database renders its `cidr[]`. An empty list
/// admits everyone; an entry that cannot be read admits nobody, so a damaged list fails closed.
pub fn allows(stored: &[String], client: IpAddr) -> bool {
    stored.is_empty()
        || stored
            .iter()
            .any(|entry| IpCidr::parse(entry).is_ok_and(|block| block.contains(client)))
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    fn list(entries: &[&str]) -> Vec<String> {
        entries.iter().map(|entry| (*entry).to_owned()).collect()
    }

    fn ip(text: &str) -> IpAddr {
        text.parse().unwrap()
    }

    #[test]
    fn entries_are_validated_and_normalised() {
        assert_eq!(
            normalise_entries(&list(&[
                "192.0.2.5",
                " 198.51.100.0/24 ",
                "2001:db8::/32",
                "192.0.2.5/32"
            ]))
            .unwrap(),
            list(&["192.0.2.5/32", "198.51.100.0/24", "2001:db8::/32"]),
            "a bare address is a host block and a duplicate is dropped"
        );
        assert_eq!(normalise_entries(&[]).unwrap(), Vec::<String>::new());
        for bad in [
            "",
            "nope",
            "192.0.2.0/33",
            "2001:db8::/129",
            "192.0.2.5/24",
            "2001:db8::1/32",
            "0.0.0.0/0",
            "::/0",
            "192.0.2.0/x",
        ] {
            assert!(normalise_entries(&list(&[bad])).is_err(), "{bad:?}");
        }
        let many: Vec<String> = (0..=MAX_ENTRIES).map(|index| format!("192.0.2.{index}")).collect();
        assert!(normalise_entries(&many).is_err());
        let at_limit: Vec<String> = many.iter().take(MAX_ENTRIES).cloned().collect();
        assert!(normalise_entries(&at_limit).is_ok());
    }

    #[test]
    fn an_empty_list_admits_everyone_and_a_list_admits_only_its_blocks() {
        assert!(allows(&[], ip("203.0.113.9")));
        let blocks = list(&["198.51.100.0/24", "2001:db8::/32", "192.0.2.77/32"]);
        for inside in ["198.51.100.0", "198.51.100.255", "2001:db8:ffff::1", "192.0.2.77"] {
            assert!(allows(&blocks, ip(inside)), "{inside}");
        }
        for outside in [
            "198.51.99.255",
            "198.51.101.0",
            "2001:db9::1",
            "192.0.2.76",
            "192.0.2.78",
            "::1",
        ] {
            assert!(!allows(&blocks, ip(outside)), "{outside}");
        }
    }

    #[test]
    fn cidr_edges_follow_the_prefix() {
        let half = list(&["192.0.2.128/25"]);
        assert!(!allows(&half, ip("192.0.2.127")));
        assert!(allows(&half, ip("192.0.2.128")));
        assert!(allows(&half, ip("192.0.2.255")));
        assert!(!allows(&half, ip("192.0.3.0")));
        let v6 = list(&["2001:db8:1::/48"]);
        assert!(allows(&v6, ip("2001:db8:1:ffff:ffff:ffff:ffff:ffff")));
        assert!(!allows(&v6, ip("2001:db8:2::")));
    }

    #[test]
    fn an_ipv4_mapped_client_matches_its_ipv4_block_and_families_do_not_mix() {
        let v4 = list(&["192.0.2.0/24"]);
        assert!(allows(&v4, ip("::ffff:192.0.2.9")));
        assert!(!allows(&list(&["2001:db8::/32"]), ip("192.0.2.9")));
    }

    #[test]
    fn a_damaged_list_fails_closed() {
        assert!(!allows(&list(&["not a block"]), ip("192.0.2.9")));
    }
}
