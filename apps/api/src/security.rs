//! Request-origin helpers shared by the routes.

use std::net::{IpAddr, Ipv4Addr, SocketAddr};

use axum::http::HeaderMap;
use platform::config::IpCidr;

/// Longest `X-Forwarded-For` chain examined. A longer header is not a real proxy chain.
const MAX_FORWARDED_HOPS: usize = 32;

/// Loopback plus the range Docker allocates compose bridge networks from, which is where the
/// bundled nginx (`frontend/nginx.conf`) connects from.
pub fn default_trusted_proxies() -> Vec<IpCidr> {
    ["127.0.0.0/8", "::1/128", "172.16.0.0/12"]
        .iter()
        .filter_map(|block| IpCidr::parse(block).ok())
        .collect()
}

fn is_trusted(trusted: &[IpCidr], addr: IpAddr) -> bool {
    trusted.iter().any(|block| block.contains(addr))
}

/// Parses one header token: a bare address, or `addr:port` / `[v6]:port`. Anything else is `None`.
fn parse_hop(token: &str) -> Option<IpAddr> {
    let token = token.trim();
    token
        .parse::<IpAddr>()
        .ok()
        .or_else(|| token.parse::<SocketAddr>().ok().map(|socket| socket.ip()))
        .map(|addr| addr.to_canonical())
}

/// The address of the client that made the request.
///
/// The TCP peer is the only address that cannot be forged, so it is the answer unless the peer is
/// one of `trusted_proxies`. Only then are the proxy's headers believed: the right-most
/// `X-Forwarded-For` entry that is not itself a trusted proxy (each trusted hop appends the
/// address it received the request from, so the right-most untrusted entry is the first address a
/// trusted proxy saw from outside), else `X-Real-IP`. A header that does not parse is never
/// guessed at; the lookup falls back to the peer. Without a peer (direct handler tests) the
/// unspecified address is returned, matching the previous behaviour of the share limiter.
pub fn client_ip(headers: &HeaderMap, peer: Option<SocketAddr>, trusted_proxies: &[IpCidr]) -> IpAddr {
    let Some(peer) = peer else {
        return IpAddr::V4(Ipv4Addr::UNSPECIFIED);
    };
    let peer_ip = peer.ip().to_canonical();
    if !is_trusted(trusted_proxies, peer_ip) {
        return peer_ip;
    }
    if let Some(addr) = forwarded_for_client(headers, trusted_proxies) {
        return addr;
    }
    if let Some(addr) = headers
        .get("x-real-ip")
        .and_then(|value| value.to_str().ok())
        .and_then(parse_hop)
    {
        return addr;
    }
    peer_ip
}

/// Walks `X-Forwarded-For` right to left. `None` when the header is absent, malformed or too
/// long; when every entry is a trusted proxy the left-most one is the client.
fn forwarded_for_client(headers: &HeaderMap, trusted: &[IpCidr]) -> Option<IpAddr> {
    let mut hops = Vec::new();
    for value in headers.get_all("x-forwarded-for") {
        for token in value.to_str().ok()?.split(',') {
            hops.push(parse_hop(token)?);
            if hops.len() > MAX_FORWARDED_HOPS {
                return None;
            }
        }
    }
    hops.iter()
        .rev()
        .find(|addr| !is_trusted(trusted, **addr))
        .or_else(|| hops.first())
        .copied()
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::HeaderValue;

    fn headers(pairs: &[(&'static str, &str)]) -> HeaderMap {
        let mut map = HeaderMap::new();
        for (name, value) in pairs {
            map.append(*name, HeaderValue::from_str(value).unwrap());
        }
        map
    }

    fn peer(text: &str) -> SocketAddr {
        text.parse().unwrap()
    }

    fn ip(text: &str) -> IpAddr {
        text.parse().unwrap()
    }

    #[test]
    fn trusted_peer_uses_right_most_untrusted_entry() {
        let trusted = default_trusted_proxies();
        // client-supplied "9.9.9.9" is spoofed; nginx appended the real client and its own hop.
        let h = headers(&[("x-forwarded-for", "9.9.9.9, 203.0.113.7, 172.18.0.5")]);
        assert_eq!(
            client_ip(&h, Some(peer("172.18.0.2:5555")), &trusted),
            ip("203.0.113.7")
        );
        let h = headers(&[("x-forwarded-for", "203.0.113.7")]);
        assert_eq!(client_ip(&h, Some(peer("127.0.0.1:1")), &trusted), ip("203.0.113.7"));
    }

    #[test]
    fn chain_split_over_several_header_lines() {
        let trusted = default_trusted_proxies();
        let h = headers(&[
            ("x-forwarded-for", "9.9.9.9"),
            ("x-forwarded-for", "203.0.113.7, 198.51.100.1"),
        ]);
        let custom = [
            IpCidr::parse("198.51.100.0/24").unwrap(),
            IpCidr::parse("127.0.0.1").unwrap(),
        ];
        assert_eq!(client_ip(&h, Some(peer("127.0.0.1:1")), &custom), ip("203.0.113.7"));
        // 198.51.100.1 is not trusted by the default list, so it is the answer there.
        assert_eq!(client_ip(&h, Some(peer("127.0.0.1:1")), &trusted), ip("198.51.100.1"));
    }

    #[test]
    fn untrusted_peer_ignores_spoofed_headers() {
        let trusted = default_trusted_proxies();
        let h = headers(&[("x-forwarded-for", "1.2.3.4"), ("x-real-ip", "5.6.7.8")]);
        assert_eq!(
            client_ip(&h, Some(peer("198.51.100.20:4000")), &trusted),
            ip("198.51.100.20")
        );
        // just outside 172.16.0.0/12
        assert_eq!(client_ip(&h, Some(peer("172.32.0.1:4000")), &trusted), ip("172.32.0.1"));
    }

    #[test]
    fn x_real_ip_used_without_forwarded_for() {
        let trusted = default_trusted_proxies();
        let h = headers(&[("x-real-ip", "203.0.113.9")]);
        assert_eq!(client_ip(&h, Some(peer("127.0.0.1:1")), &trusted), ip("203.0.113.9"));
    }

    #[test]
    fn forwarded_for_wins_over_real_ip() {
        let trusted = default_trusted_proxies();
        let h = headers(&[("x-forwarded-for", "203.0.113.7"), ("x-real-ip", "203.0.113.9")]);
        assert_eq!(client_ip(&h, Some(peer("127.0.0.1:1")), &trusted), ip("203.0.113.7"));
    }

    #[test]
    fn malformed_headers_fall_back_to_the_peer() {
        let trusted = default_trusted_proxies();
        for value in [
            "not-an-ip",
            "1.2.3.4, garbage",
            "",
            ",",
            "1.2.3.4,,5.6.7.8",
            "999.1.1.1",
        ] {
            let h = headers(&[("x-forwarded-for", value)]);
            assert_eq!(
                client_ip(&h, Some(peer("127.0.0.1:1")), &trusted),
                ip("127.0.0.1"),
                "{value:?}"
            );
        }
        let h = headers(&[("x-real-ip", "bogus")]);
        assert_eq!(client_ip(&h, Some(peer("127.0.0.1:1")), &trusted), ip("127.0.0.1"));
        let mut h = HeaderMap::new();
        h.insert("x-forwarded-for", HeaderValue::from_bytes(&[0xff, 0xfe]).unwrap());
        assert_eq!(client_ip(&h, Some(peer("127.0.0.1:1")), &trusted), ip("127.0.0.1"));
    }

    #[test]
    fn overlong_chain_is_rejected() {
        let trusted = default_trusted_proxies();
        let chain = vec!["203.0.113.7"; MAX_FORWARDED_HOPS + 1].join(",");
        let h = headers(&[("x-forwarded-for", &chain)]);
        assert_eq!(client_ip(&h, Some(peer("127.0.0.1:1")), &trusted), ip("127.0.0.1"));
    }

    #[test]
    fn ipv6_peers_clients_and_ports() {
        let trusted = default_trusted_proxies();
        let h = headers(&[("x-forwarded-for", "2001:db8::7")]);
        assert_eq!(client_ip(&h, Some(peer("[::1]:9000")), &trusted), ip("2001:db8::7"));
        let h = headers(&[("x-forwarded-for", "[2001:db8::8]:443, 127.0.0.1")]);
        assert_eq!(client_ip(&h, Some(peer("[::1]:9000")), &trusted), ip("2001:db8::8"));
        let h = headers(&[("x-forwarded-for", "203.0.113.7:5000")]);
        assert_eq!(client_ip(&h, Some(peer("127.0.0.1:1")), &trusted), ip("203.0.113.7"));
        // untrusted v6 peer
        assert_eq!(
            client_ip(&h, Some(peer("[2001:db8::99]:1")), &trusted),
            ip("2001:db8::99")
        );
        // mapped v4 loopback peer counts as loopback
        let h = headers(&[("x-forwarded-for", "203.0.113.7")]);
        assert_eq!(
            client_ip(&h, Some(peer("[::ffff:127.0.0.1]:1")), &trusted),
            ip("203.0.113.7")
        );
    }

    #[test]
    fn all_trusted_chain_yields_leftmost_and_empty_list_trusts_nobody() {
        let trusted = default_trusted_proxies();
        let h = headers(&[("x-forwarded-for", "127.0.0.2, 172.17.0.1")]);
        assert_eq!(client_ip(&h, Some(peer("127.0.0.1:1")), &trusted), ip("127.0.0.2"));
        let h = headers(&[("x-forwarded-for", "1.2.3.4")]);
        assert_eq!(client_ip(&h, Some(peer("127.0.0.1:1")), &[]), ip("127.0.0.1"));
    }

    #[test]
    fn missing_peer_is_unspecified() {
        let h = headers(&[("x-forwarded-for", "1.2.3.4")]);
        assert_eq!(
            client_ip(&h, None, &default_trusted_proxies()),
            IpAddr::V4(Ipv4Addr::UNSPECIFIED)
        );
    }
}
