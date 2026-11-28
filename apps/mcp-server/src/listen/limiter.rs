//! A bound on the work made-up credentials can cost: refusals counted per client address.
//!
//! Every distinct bearer token that is not a live credential costs the API one request when it is
//! offered to a listen request. This limiter counts the refusals the API answers per client
//! address; an address with too many in its window gets no further API request for a credential
//! the server does not already know, until the window ends. Credentials with a live hub are never
//! affected, so a client that is behind the same address (a proxy) as a noisy one keeps its streams.
//! Memory is bounded: a fixed number of addresses, the oldest dropped first.

use parking_lot::Mutex;
use std::collections::HashMap;
use std::net::{IpAddr, Ipv6Addr};
use std::time::{Duration, Instant};

/// Refusals an address may collect within one window before it is throttled.
pub const MAX_REFUSALS: u32 = 20;
/// How long refusals count, from the first one of the window.
pub const WINDOW: Duration = Duration::from_mins(1);
/// Addresses remembered at once.
pub const CAPACITY: usize = 4_096;

/// A client address as the limiter sees it: an IPv6 address stands for its /64, because that is
/// what one subscriber is given.
fn normalise(address: IpAddr) -> IpAddr {
    match address {
        IpAddr::V4(_) => address,
        IpAddr::V6(v6) => {
            let mut segments = v6.segments();
            for segment in segments.iter_mut().skip(4) {
                *segment = 0;
            }
            IpAddr::V6(Ipv6Addr::from(segments))
        }
    }
}

#[derive(Default)]
pub struct RefusalLimiter {
    entries: Mutex<HashMap<IpAddr, (Instant, u32)>>,
}

impl RefusalLimiter {
    /// Whether `address` has used up its refusals in the current window.
    pub fn throttled(&self, address: IpAddr) -> bool {
        let key = normalise(address);
        self.entries
            .lock()
            .get(&key)
            .is_some_and(|(start, count)| start.elapsed() < WINDOW && *count >= MAX_REFUSALS)
    }

    /// Counts one refusal for `address`.
    // The lookup, the eviction and the increment are one check-then-mutate under one guard.
    #[allow(clippy::significant_drop_tightening)]
    pub fn record(&self, address: IpAddr) {
        let key = normalise(address);
        let now = Instant::now();
        let mut entries = self.entries.lock();
        if !entries.contains_key(&key) && entries.len() >= CAPACITY {
            entries.retain(|_, (start, _)| now.duration_since(*start) < WINDOW);
            if entries.len() >= CAPACITY {
                let oldest = entries.iter().min_by_key(|(_, (start, _))| *start).map(|(key, _)| *key);
                if let Some(oldest) = oldest {
                    entries.remove(&oldest);
                }
            }
        }
        let entry = entries.entry(key).or_insert((now, 0));
        if now.duration_since(entry.0) >= WINDOW {
            *entry = (now, 0);
        }
        entry.1 = entry.1.saturating_add(1);
    }
}
