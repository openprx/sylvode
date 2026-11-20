//! A minimal CIDR block, enough to say which peers are trusted reverse proxies.

use std::fmt;
use std::net::IpAddr;

/// An IP network such as `10.0.0.0/8`, or a single address (`/32` or `/128`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct IpCidr {
    network: IpAddr,
    prefix: u8,
}

impl IpCidr {
    /// A block holding exactly one address.
    pub const fn host(addr: IpAddr) -> Self {
        let prefix = match addr {
            IpAddr::V4(_) => 32,
            IpAddr::V6(_) => 128,
        };
        Self { network: addr, prefix }
    }

    /// Builds a block, masking the host bits of `network`. `None` when `prefix` is too long.
    pub fn new(network: IpAddr, prefix: u8) -> Option<Self> {
        let max = match network {
            IpAddr::V4(_) => 32,
            IpAddr::V6(_) => 128,
        };
        if prefix > max {
            return None;
        }
        let network = match network {
            IpAddr::V4(addr) => IpAddr::V4((u32::from(addr) & v4_mask(prefix)).into()),
            IpAddr::V6(addr) => IpAddr::V6((u128::from(addr) & v6_mask(prefix)).into()),
        };
        Some(Self { network, prefix })
    }

    /// Parses `addr` or `addr/prefix`. Host bits in the address are masked off.
    pub fn parse(text: &str) -> Result<Self, String> {
        let text = text.trim();
        let (addr_text, prefix_text) = match text.split_once('/') {
            Some((addr, prefix)) => (addr, Some(prefix)),
            None => (text, None),
        };
        let addr: IpAddr = addr_text
            .parse()
            .map_err(|_| format!("{text:?} is not an IP address or CIDR block"))?;
        match prefix_text {
            None => Ok(Self::host(addr)),
            Some(prefix) => {
                let prefix: u8 = prefix
                    .parse()
                    .map_err(|_| format!("{text:?} has an invalid prefix length"))?;
                Self::new(addr, prefix).ok_or_else(|| format!("{text:?} has a prefix length that is too long"))
            }
        }
    }

    /// Whether `addr` is inside the block. An IPv4-mapped IPv6 address matches as its IPv4 form.
    pub fn contains(&self, addr: IpAddr) -> bool {
        match (self.network, addr.to_canonical()) {
            (IpAddr::V4(net), IpAddr::V4(addr)) => u32::from(net) == u32::from(addr) & v4_mask(self.prefix),
            (IpAddr::V6(net), IpAddr::V6(addr)) => u128::from(net) == u128::from(addr) & v6_mask(self.prefix),
            _ => false,
        }
    }
}

impl fmt::Display for IpCidr {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}/{}", self.network, self.prefix)
    }
}

fn v4_mask(prefix: u8) -> u32 {
    u32::MAX.checked_shl(32 - u32::from(prefix)).unwrap_or(0)
}

fn v6_mask(prefix: u8) -> u128 {
    u128::MAX.checked_shl(128 - u32::from(prefix)).unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::IpCidr;
    use std::net::IpAddr;

    fn ip(text: &str) -> IpAddr {
        text.parse().unwrap()
    }

    #[test]
    fn matches_inside_and_outside() {
        let block = IpCidr::parse("203.0.113.0/25").unwrap();
        assert!(block.contains(ip("203.0.113.127")));
        assert!(!block.contains(ip("203.0.113.128")));
        assert!(!block.contains(ip("::1")));
        assert!(IpCidr::parse("::1/128").unwrap().contains(ip("::1")));
        assert!(IpCidr::parse("0.0.0.0/0").unwrap().contains(ip("8.8.8.8")));
        assert!(IpCidr::parse("fc00::/7").unwrap().contains(ip("fd12::1")));
    }

    #[test]
    fn single_address_and_mapped_v6() {
        let block = IpCidr::parse("198.51.100.3").unwrap();
        assert!(block.contains(ip("198.51.100.3")));
        assert!(block.contains(ip("::ffff:198.51.100.3")));
        assert!(!block.contains(ip("198.51.100.4")));
    }

    #[test]
    fn rejects_malformed() {
        for bad in [
            "",
            "10.0.0.0/33",
            "::/129",
            "10.0.0.0/x",
            "nope",
            "10.0.0/8",
            "10.0.0.0/-1",
        ] {
            assert!(IpCidr::parse(bad).is_err(), "{bad}");
        }
    }
}
