//! A bound on the work made-up credentials can cost: refusals counted per client address.
//!
//! Every distinct bearer token that is not a live credential costs the API one request when it is
//! offered to a listen request. This limiter counts the refusals the API answers per client
//! address; an address with too many in its window gets no further API request for a credential
//! the server does not already know, until the window ends. Credentials with a live hub are never
//! affected. A client whose address cannot be told (a trusted proxy that sent no `X-Real-IP`) is
//! bounded by one shared, much higher ceiling instead; a client with an address is bound only by its
//! own limit, so one noisy client cannot throttle the others.
//!
//! An attempt is counted when it is admitted, before the API is asked, and stays counted unless it is
//! explicitly given back after a definite answer that is not a refusal of the credential. A request
//! that is dropped half way (a client that hung up after the request reached the API) therefore keeps
//! its attempt, so the limit holds however many requests arrive at once. Memory is bounded: a fixed
//! number of addresses, the oldest dropped first.

use super::RETRY_AFTER_SECS;
use parking_lot::Mutex;
use std::collections::HashMap;
use std::net::{IpAddr, Ipv6Addr};
use std::time::{Duration, Instant};

/// Refusals an address may collect within one window before it is throttled.
pub const MAX_REFUSALS: u32 = 20;
/// Refusals clients without a known address may collect within one window, together.
pub const MAX_UNATTRIBUTED_REFUSALS: u32 = 600;
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
    unattributed: Option<(Instant, u32)>,
}

#[derive(Default)]
pub struct RefusalLimiter {
    state: Mutex<State>,
}

impl RefusalLimiter {
    /// Whether `client` has used up its attempts in the current window. A look only; [`Self::admit`]
    /// is what counts.
    #[allow(clippy::significant_drop_tightening)]
    pub fn throttled(&self, client: Client) -> bool {
        let state = self.state.lock();
        let (slot, max) = match client {
            Client::Address(address) => (state.entries.get(&normalise(address)).copied(), MAX_REFUSALS),
            Client::Unattributed => (state.unattributed, MAX_UNATTRIBUTED_REFUSALS),
        };
        slot.is_some_and(|(start, count)| start.elapsed() < WINDOW && count >= max)
    }

    /// Counts one attempt for `client` and admits it, or `None` when its window is full. The attempt
    /// stays counted unless [`Attempt::give_back`] is called; dropping it keeps the place taken.
    // The lookup, the eviction and the increment are one check-then-mutate under one guard.
    #[allow(clippy::significant_drop_tightening)]
    pub fn admit(&self, client: Client) -> Option<Attempt<'_>> {
        let now = Instant::now();
        let mut state = self.state.lock();
        let (slot, max) = match client {
            Client::Unattributed => (&mut state.unattributed, MAX_UNATTRIBUTED_REFUSALS),
            Client::Address(address) => {
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
                return Self::take(entry, max_for(client), self, client);
            }
        };
        let entry = slot.get_or_insert((now, 0));
        if now.duration_since(entry.0) >= WINDOW {
            *entry = (now, 0);
        }
        Self::take(entry, max, self, client)
    }

    const fn take<'a>(entry: &mut (Instant, u32), max: u32, limiter: &'a Self, client: Client) -> Option<Attempt<'a>> {
        if entry.1 >= max {
            return None;
        }
        entry.1 += 1;
        Some(Attempt {
            limiter,
            client,
            window: entry.0,
        })
    }

    /// Gives one attempt of the window that began at `window` back.
    #[allow(clippy::significant_drop_tightening)]
    fn release(&self, client: Client, window: Instant) {
        let mut state = self.state.lock();
        let entry = match client {
            Client::Address(address) => state.entries.get_mut(&normalise(address)),
            Client::Unattributed => state.unattributed.as_mut(),
        };
        if let Some(entry) = entry
            && entry.0 == window
        {
            entry.1 = entry.1.saturating_sub(1);
        }
    }
}

const fn max_for(client: Client) -> u32 {
    match client {
        Client::Address(_) => MAX_REFUSALS,
        Client::Unattributed => MAX_UNATTRIBUTED_REFUSALS,
    }
}

/// One counted attempt. It stays counted when dropped; only [`Attempt::give_back`] releases it.
pub struct Attempt<'a> {
    limiter: &'a RefusalLimiter,
    client: Client,
    window: Instant,
}

impl Attempt<'_> {
    /// The API answered, and the answer does not refuse the credential: the place is given back.
    pub fn give_back(self) {
        self.limiter.release(self.client, self.window);
    }
}
