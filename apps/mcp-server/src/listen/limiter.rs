//! A bound on the work made-up credentials can cost: refusals counted per client address.
//!
//! Every distinct bearer token that is not a live credential costs the API one request when it is
//! offered to a listen request. This limiter counts the refusals the API answers per client
//! address; an address with too many in its window gets no further API request for a credential
//! the server does not already know, until the window ends. Credentials with a live hub are never
//! affected. A second, much higher ceiling counts the refusals of the whole process, so a client
//! whose address cannot be told (a trusted proxy that sent no `X-Real-IP`) is still bounded, and so
//! is an attacker with many addresses. Memory is bounded: a fixed number of addresses, the oldest
//! dropped first.

use super::RETRY_AFTER_SECS;
use parking_lot::Mutex;
use std::collections::HashMap;
use std::net::{IpAddr, Ipv6Addr};
use std::time::{Duration, Instant};

/// Refusals an address may collect within one window before it is throttled.
pub const MAX_REFUSALS: u32 = 20;
/// Refusals the whole process may collect within one window before every unknown credential is
/// throttled.
pub const MAX_PROCESS_REFUSALS: u32 = 600;
/// How long refusals count, from the first one of the window. It equals the `Retry-After` of a
/// throttled request, so a client that waits as told is not refused again for the same window.
pub const WINDOW: Duration = Duration::from_secs(RETRY_AFTER_SECS);
/// Addresses remembered at once.
pub const CAPACITY: usize = 4_096;

/// Where a listen request came from, as far as the limiter is concerned.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Client {
    /// The client's address (a trusted proxy's `X-Real-IP`, or the socket peer).
    Address(IpAddr),
    /// A trusted proxy that did not say who the client is: only the process-wide ceiling applies.
    Unattributed,
}

/// A client address as the limiter groups it: IPv4 (an IPv4-mapped IPv6 address counts as the IPv4
/// one) as is, real IPv6 by its /64, which is what one subscriber is given.
fn normalise(address: IpAddr) -> IpAddr {
    match address.to_canonical() {
        IpAddr::V6(v6) => {
            let mut segments = v6.segments();
            for segment in segments.iter_mut().skip(4) {
                *segment = 0;
            }
            IpAddr::V6(Ipv6Addr::from(segments))
        }
        v4 @ IpAddr::V4(_) => v4,
    }
}

#[derive(Default)]
struct State {
    entries: HashMap<IpAddr, (Instant, u32)>,
    process: Option<(Instant, u32)>,
}

#[derive(Default)]
pub struct RefusalLimiter {
    state: Mutex<State>,
}

impl RefusalLimiter {
    /// Whether `client` has used up its refusals in the current window, or the process has.
    pub fn throttled(&self, client: Client) -> bool {
        let state = self.state.lock();
        if state
            .process
            .is_some_and(|(start, count)| start.elapsed() < WINDOW && count >= MAX_PROCESS_REFUSALS)
        {
            return true;
        }
        match client {
            Client::Address(address) => state
                .entries
                .get(&normalise(address))
                .is_some_and(|(start, count)| start.elapsed() < WINDOW && *count >= MAX_REFUSALS),
            Client::Unattributed => false,
        }
    }

    /// Counts one refusal for `client` and for the process.
    // The lookup, the eviction and the increment are one check-then-mutate under one guard.
    #[allow(clippy::significant_drop_tightening)]
    pub fn record(&self, client: Client) {
        let now = Instant::now();
        let mut state = self.state.lock();
        let process = state.process.get_or_insert((now, 0));
        if now.duration_since(process.0) >= WINDOW {
            *process = (now, 0);
        }
        process.1 = process.1.saturating_add(1);
        let Client::Address(address) = client else { return };
        let key = normalise(address);
        if !state.entries.contains_key(&key) && state.entries.len() >= CAPACITY {
            state
                .entries
                .retain(|_, (start, _)| now.duration_since(*start) < WINDOW);
            if state.entries.len() >= CAPACITY {
                let oldest = state
                    .entries
                    .iter()
                    .min_by_key(|(_, (start, _))| *start)
                    .map(|(key, _)| *key);
                if let Some(oldest) = oldest {
                    state.entries.remove(&oldest);
                }
            }
        }
        let entry = state.entries.entry(key).or_insert((now, 0));
        if now.duration_since(entry.0) >= WINDOW {
            *entry = (now, 0);
        }
        entry.1 = entry.1.saturating_add(1);
    }
}
