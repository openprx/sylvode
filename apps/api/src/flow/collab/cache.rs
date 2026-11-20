//! The bounded per-document warm cache (`ADR-0010`, decision B).
//!
//! Budgets are the frozen [`super::limits`] numbers, enforced as real LRU eviction — never by
//! disabling eviction or raising a config knob (`ADR-0010`: "不能通过关闭 eviction 或无界提高配置来
//! 过 gate").
//!
//! Concurrency shape (this crate's iron rules + `ADR-0010`'s "写入算法"): the registry itself is a
//! `parking_lot::Mutex`, a short synchronous critical section per call — [`WarmCache::fork_matching`]
//! and [`WarmCache::put`] never hold the guard across an `.await` (there is no `.await` inside
//! either; [`collab_core::LoroCollabEngine::fork`] is CPU-only). The cache is an accelerator, not
//! an authority (`ADR-0010`): [`WarmCache::fork_matching`] hands the caller an independent forked
//! copy while leaving the registry's own entry untouched, so the caller can do the
//! (unbounded-duration) isolated-apply work below with no lock held at all; only a *successful,
//! committed* write ever replaces the registry's entry, via [`WarmCache::put`].

#![allow(clippy::too_long_first_doc_paragraph)]

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use collab_core::LoroCollabEngine;
use parking_lot::Mutex;
use uuid::Uuid;

use super::limits::{
    WARM_CACHE_DECODED_BYTES_MAX, WARM_CACHE_DOCUMENTS_MAX, WARM_CACHE_ENTRY_DECODED_BYTES_MAX,
    WARM_CACHE_IDLE_TTL_SECONDS,
};

/// One warm document (`ADR-0010`'s frozen entry shape): `document_id`/`engine`=`decoded_doc` are
/// the map key and the field below; `format_version`, `prepared_head_seq`,
/// `prepared_head_frontier`, `projection_input` (this package's write path recomputes projection
/// input from `engine` directly rather than caching it separately — see `write.rs`) and
/// `last_used` round out the rest.
pub struct CacheEntry {
    pub engine: LoroCollabEngine,
    pub format_version: String,
    pub prepared_head_seq: i64,
    pub prepared_head_frontier: Vec<u8>,
    /// The exact `export_snapshot` bytes of `engine`'s state, when known: what the isolated worker
    /// is sent as the base, so a hot document is not re-exported on every write. Always describes
    /// the same state as `engine` (the two are only ever stored together, or attached under the
    /// entry's own key by [`WarmCache::attach_base_bytes`]), and is dropped together with the entry
    /// on every replacement, eviction and invalidation.
    base_bytes: Option<Arc<Vec<u8>>>,
    /// Caller-supplied size accounting (production: the entry's `export_snapshot().len()`-order
    /// footprint) plus the length of `base_bytes` when present. The engine part is not measured by
    /// this module — see [`WarmCache::put`]'s doc comment for why that is a deliberate, documented
    /// choice.
    decoded_bytes: u64,
    last_used: Instant,
}

/// The head a caller has just read from the database and wants a cache entry for. An entry is
/// served only when **all three** parts equal the entry's own: a stale `head_seq`, a different
/// frontier (a rewritten or re-created document that happens to reuse a sequence number) or a
/// different format version is a miss, and the caller loads from the database instead.
///
/// The frontier is compared bytewise, not as a decoded version vector (`flow::frontier` is for
/// checks between two independently produced frontiers). Both sides of this comparison are copies
/// of bytes the database stored or the write path just wrote, so bytewise equality is the exact
/// test, and a spelling difference of one version could only ever cost a miss, never a false hit.
#[derive(Debug, Clone, Copy)]
pub struct HeadKey<'a> {
    pub head_seq: i64,
    pub frontier: &'a [u8],
    pub format_version: &'a str,
}

impl CacheEntry {
    fn matches(&self, key: &HeadKey<'_>) -> bool {
        let HeadKey {
            head_seq,
            frontier,
            format_version,
        } = *key;
        self.prepared_head_seq == head_seq
            && self.format_version == format_version
            && self.prepared_head_frontier == frontier
    }
}

/// What [`WarmCache::checkout`] hands back for a matching entry.
pub struct Checkout {
    /// The entry's base bytes, when it holds them.
    pub base_bytes: Option<Arc<Vec<u8>>>,
    /// An independent forked copy of the entry's engine. Present when requested, and always
    /// present when the entry holds no base bytes (the caller then needs it to export them).
    pub engine: Option<LoroCollabEngine>,
}

struct Registry {
    entries: HashMap<Uuid, CacheEntry>,
    total_bytes: u64,
}

pub struct WarmCache {
    inner: Mutex<Registry>,
}

impl Default for WarmCache {
    fn default() -> Self {
        Self::new()
    }
}

/// Outcome of [`WarmCache::put`], letting a caller (and a test) observe whether the entry it just
/// tried to cache actually ended up resident.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PutOutcome {
    Cached,
    /// The entry's own `decoded_bytes` exceeds [`WARM_CACHE_ENTRY_DECODED_BYTES_MAX`]
    /// (`ADR-0010`: "超过即不驻留并 fail closed/要求缩小或 snapshot,不得常驻更大 state"). The write
    /// this entry came from is unaffected — the cache is an accelerator, not an authority; a
    /// document just too large to cache always still bootstraps from the database on the next
    /// write.
    TooLargeToCache,
    /// [`WarmCache::seed_with_base`] found an entry for the document that is already ahead of the
    /// head being seeded, and left it in place.
    Superseded,
}

impl WarmCache {
    #[must_use]
    pub fn new() -> Self {
        Self {
            inner: Mutex::new(Registry {
                entries: HashMap::new(),
                total_bytes: 0,
            }),
        }
    }

    /// Non-destructively forks the entry for `document_id` if it is live (not idle-expired) *and*
    /// still represents the caller's observed head (`key`: sequence, frontier and format version)
    /// — the exact "hydrate to observed DB head" check `ADR-0010`'s write algorithm requires
    /// before an entry may be used as an isolated-apply base. A stale or absent entry returns
    /// `Ok(None)`, telling the caller to fall back to the full bootstrap loader.
    ///
    /// Deliberately non-destructive (unlike an earlier `take`-based design this module does not
    /// use): the only callers are write paths that are themselves serialized per document by
    /// [`super::coordinator::DocumentCoordinator`] within this process, and `put` (called once,
    /// after a successful commit) is the only thing that ever replaces this entry. Another API
    /// process can commit in between, which moves the database head: every attempt re-checks the
    /// observed head against the database before trusting the cache at all, and the locked phase
    /// re-verifies it under `SELECT ... FOR UPDATE`, so an entry that went stale can at worst cost
    /// a miss or a rebase, never a commit.
    ///
    /// # Errors
    /// Propagates [`collab_core::LoroCollabEngine::fork`]'s failure mode.
    pub fn fork_matching(
        &self,
        document_id: Uuid,
        key: &HeadKey<'_>,
    ) -> Result<Option<LoroCollabEngine>, collab_core::CollabError> {
        Ok(self
            .checkout(document_id, key, true)?
            .and_then(|checkout| checkout.engine))
    }

    /// [`Self::fork_matching`] plus the entry's cached base bytes. The engine is forked only when
    /// `want_engine` is set or the entry has no base bytes; a caller that only needs the bytes for
    /// an isolated apply pays no O(document) fork at all.
    ///
    /// # Errors
    /// Propagates [`collab_core::LoroCollabEngine::fork`]'s failure mode.
    #[allow(clippy::significant_drop_tightening)] // guard held through the fork, which is synchronous
    pub fn checkout(
        &self,
        document_id: Uuid,
        key: &HeadKey<'_>,
        want_engine: bool,
    ) -> Result<Option<Checkout>, collab_core::CollabError> {
        let mut registry = self.inner.lock();
        evict_stale(&mut registry);
        let Some(entry) = registry.entries.get_mut(&document_id) else {
            return Ok(None);
        };
        if !entry.matches(key) {
            return Ok(None);
        }
        entry.last_used = Instant::now();
        let base_bytes = entry.base_bytes.clone();
        let engine = if want_engine || base_bytes.is_none() {
            Some(entry.engine.fork()?)
        } else {
            None
        };
        Ok(Some(Checkout { base_bytes, engine }))
    }

    /// Stores `base_bytes` on the entry for `document_id` if, and only if, the entry still has
    /// exactly the head `key` names. Bytes are never attached to a different head, so they always
    /// describe the entry's own engine. A no-op when the entry is gone, differs, already holds
    /// bytes, or the bytes would push the entry over its single-entry ceiling.
    #[allow(clippy::significant_drop_tightening)] // the guard is the whole function
    pub fn attach_base_bytes(&self, document_id: Uuid, key: &HeadKey<'_>, base_bytes: Arc<Vec<u8>>) {
        let mut registry = self.inner.lock();
        let Some(entry) = registry.entries.get_mut(&document_id) else {
            return;
        };
        if !entry.matches(key) || entry.base_bytes.is_some() {
            return;
        }
        let extra = byte_len(&base_bytes);
        let accounted = entry.decoded_bytes.saturating_add(extra);
        if accounted > WARM_CACHE_ENTRY_DECODED_BYTES_MAX {
            return;
        }
        entry.base_bytes = Some(base_bytes);
        entry.decoded_bytes = accounted;
        registry.total_bytes = registry.total_bytes.saturating_add(extra);
        evict_to_budget(&mut registry, document_id);
    }

    /// Removes the entry for `document_id`, if any. For a path that rewrites a document's
    /// canonical state without going through a write that re-seeds the cache.
    pub fn invalidate(&self, document_id: Uuid) {
        let mut registry = self.inner.lock();
        if let Some(entry) = registry.entries.remove(&document_id) {
            registry.total_bytes = registry.total_bytes.saturating_sub(entry.decoded_bytes);
        }
    }

    /// Inserts (or replaces) the entry for `document_id`, evicting by LRU until both the entry
    /// count and total byte budgets are satisfied.
    ///
    /// `decoded_bytes` is a caller-supplied size hint rather than something this module measures
    /// from `engine` itself: `collab_core::LoroCollabEngine` exposes no cheap in-memory size
    /// probe (the only way to learn it is `export_snapshot().len()`, an O(n) encode this hot path
    /// should not pay on every cache write). Production call sites compute it once, right after
    /// the export they already need for `collab_updates`/hydrate bookkeeping; tests may pass a
    /// synthetic value to exercise the byte ceiling without allocating that many real bytes — the
    /// eviction *logic* below is exactly the same code path either way.
    pub fn put(
        &self,
        document_id: Uuid,
        engine: LoroCollabEngine,
        format_version: String,
        prepared_head_seq: i64,
        prepared_head_frontier: Vec<u8>,
        decoded_bytes: u64,
    ) -> PutOutcome {
        self.put_with_base(
            document_id,
            engine,
            format_version,
            prepared_head_seq,
            prepared_head_frontier,
            decoded_bytes,
            None,
        )
    }

    /// [`Self::put`] with the exact `export_snapshot` bytes of `engine`'s state. The caller
    /// guarantees the two describe the same state (production: the bytes are what the isolated
    /// worker returned and `engine` was loaded from them, or `engine` was exported to make them).
    /// The bytes count against the same budgets as the engine: the entry is accounted as
    /// `decoded_bytes + base_bytes.len()`, and when that exceeds the single-entry ceiling the
    /// entry is cached without its bytes rather than not at all.
    #[allow(clippy::too_many_arguments)]
    pub fn put_with_base(
        &self,
        document_id: Uuid,
        engine: LoroCollabEngine,
        format_version: String,
        prepared_head_seq: i64,
        prepared_head_frontier: Vec<u8>,
        decoded_bytes: u64,
        base_bytes: Option<Arc<Vec<u8>>>,
    ) -> PutOutcome {
        self.insert(
            document_id,
            engine,
            format_version,
            prepared_head_seq,
            prepared_head_frontier,
            decoded_bytes,
            base_bytes,
            false,
        )
    }

    /// [`Self::put_with_base`] for state that was *loaded* from the database at a head some time
    /// ago, not committed just now: it never replaces an entry whose head sequence is higher
    /// (that entry was cached by a later commit, and a stale load must not undo it). An entry at
    /// the same or a lower sequence is replaced, so a rewritten history at a reused sequence still
    /// heals.
    #[allow(clippy::too_many_arguments)]
    pub fn seed_with_base(
        &self,
        document_id: Uuid,
        engine: LoroCollabEngine,
        format_version: String,
        prepared_head_seq: i64,
        prepared_head_frontier: Vec<u8>,
        decoded_bytes: u64,
        base_bytes: Option<Arc<Vec<u8>>>,
    ) -> PutOutcome {
        self.insert(
            document_id,
            engine,
            format_version,
            prepared_head_seq,
            prepared_head_frontier,
            decoded_bytes,
            base_bytes,
            true,
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn insert(
        &self,
        document_id: Uuid,
        engine: LoroCollabEngine,
        format_version: String,
        prepared_head_seq: i64,
        prepared_head_frontier: Vec<u8>,
        decoded_bytes: u64,
        base_bytes: Option<Arc<Vec<u8>>>,
        keep_newer: bool,
    ) -> PutOutcome {
        if decoded_bytes > WARM_CACHE_ENTRY_DECODED_BYTES_MAX {
            return PutOutcome::TooLargeToCache;
        }
        let (base_bytes, accounted) = match base_bytes {
            Some(bytes) if decoded_bytes.saturating_add(byte_len(&bytes)) <= WARM_CACHE_ENTRY_DECODED_BYTES_MAX => {
                let accounted = decoded_bytes.saturating_add(byte_len(&bytes));
                (Some(bytes), accounted)
            }
            _ => (None, decoded_bytes),
        };
        let mut registry = self.inner.lock();
        evict_stale(&mut registry);

        if keep_newer
            && registry
                .entries
                .get(&document_id)
                .is_some_and(|old| old.prepared_head_seq > prepared_head_seq)
        {
            return PutOutcome::Superseded;
        }
        if let Some(old) = registry.entries.remove(&document_id) {
            registry.total_bytes = registry.total_bytes.saturating_sub(old.decoded_bytes);
        }

        while registry.entries.len() >= WARM_CACHE_DOCUMENTS_MAX {
            if !evict_one_lru(&mut registry) {
                break;
            }
        }
        while registry.total_bytes.saturating_add(accounted) > WARM_CACHE_DECODED_BYTES_MAX {
            if !evict_one_lru(&mut registry) {
                break;
            }
        }

        registry.entries.insert(
            document_id,
            CacheEntry {
                engine,
                format_version,
                prepared_head_seq,
                prepared_head_frontier,
                base_bytes,
                decoded_bytes: accounted,
                last_used: Instant::now(),
            },
        );
        registry.total_bytes = registry.total_bytes.saturating_add(accounted);
        PutOutcome::Cached
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.inner.lock().entries.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    #[must_use]
    pub fn total_bytes(&self) -> u64 {
        self.inner.lock().total_bytes
    }

    #[must_use]
    pub fn contains(&self, document_id: Uuid) -> bool {
        self.inner.lock().entries.contains_key(&document_id)
    }
}

fn byte_len(bytes: &Arc<Vec<u8>>) -> u64 {
    u64::try_from(bytes.len()).unwrap_or(u64::MAX)
}

/// Evicts least-recently-used entries other than `keep` until the byte budget holds again (an
/// entry that just grew in place must not evict itself).
fn evict_to_budget(registry: &mut Registry, keep: Uuid) {
    while registry.total_bytes > WARM_CACHE_DECODED_BYTES_MAX {
        let oldest = registry
            .entries
            .iter()
            .filter(|(id, _)| **id != keep)
            .min_by_key(|(_, entry)| entry.last_used)
            .map(|(id, _)| *id);
        let Some(id) = oldest else { break };
        if let Some(entry) = registry.entries.remove(&id) {
            registry.total_bytes = registry.total_bytes.saturating_sub(entry.decoded_bytes);
        }
    }
}

/// Removes every entry idle past [`WARM_CACHE_IDLE_TTL_SECONDS`]. Caller must hold `registry`'s
/// lock already (this is a private helper, never called with the lock unheld).
fn evict_stale(registry: &mut Registry) {
    let ttl = Duration::from_secs(WARM_CACHE_IDLE_TTL_SECONDS);
    let now = Instant::now();
    let stale: Vec<Uuid> = registry
        .entries
        .iter()
        .filter(|(_, entry)| now.duration_since(entry.last_used) >= ttl)
        .map(|(id, _)| *id)
        .collect();
    for id in stale {
        if let Some(entry) = registry.entries.remove(&id) {
            registry.total_bytes = registry.total_bytes.saturating_sub(entry.decoded_bytes);
        }
    }
}

/// Evicts the single least-recently-used entry. Returns `false` when the registry is empty (so a
/// caller's `while` loop cannot spin forever demanding more eviction than there is anything left
/// to evict).
fn evict_one_lru(registry: &mut Registry) -> bool {
    let oldest = registry
        .entries
        .iter()
        .min_by_key(|(_, entry)| entry.last_used)
        .map(|(id, _)| *id);
    let Some(id) = oldest else { return false };
    if let Some(entry) = registry.entries.remove(&id) {
        registry.total_bytes = registry.total_bytes.saturating_sub(entry.decoded_bytes);
    }
    true
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::indexing_slicing)]
mod tests {
    use super::{HeadKey, PutOutcome, WARM_CACHE_DOCUMENTS_MAX, WARM_CACHE_ENTRY_DECODED_BYTES_MAX, WarmCache};
    use collab_core::LoroCollabEngine;
    use std::sync::Arc;
    use uuid::Uuid;

    const fn key<'a>(head_seq: i64, frontier: &'a [u8], format_version: &'a str) -> HeadKey<'a> {
        HeadKey {
            head_seq,
            frontier,
            format_version,
        }
    }

    fn engine() -> LoroCollabEngine {
        LoroCollabEngine::new_empty(1)
    }

    #[test]
    fn put_then_fork_matching_returns_an_independent_engine_and_keeps_the_entry() {
        let cache = WarmCache::new();
        let id = Uuid::new_v4();
        let outcome = cache.put(id, engine(), "loro-1".to_string(), 5, vec![9, 9], 128);
        assert_eq!(outcome, PutOutcome::Cached);
        assert!(cache.contains(id));

        let forked = cache
            .fork_matching(id, &key(5, &[9, 9], "loro-1"))
            .expect("fork succeeds")
            .expect("head_seq/format_version match");
        drop(forked);
        assert!(cache.contains(id), "fork_matching is non-destructive");
    }

    #[test]
    fn fork_matching_returns_none_on_a_head_seq_mismatch() {
        let cache = WarmCache::new();
        let id = Uuid::new_v4();
        cache.put(id, engine(), "loro-1".to_string(), 5, vec![9, 9], 128);

        let result = cache
            .fork_matching(id, &key(6, &[9, 9], "loro-1"))
            .expect("fork call does not error");
        assert!(
            result.is_none(),
            "a stale prepared_head_seq must miss, not silently serve stale state"
        );
    }

    #[test]
    fn fork_matching_returns_none_on_a_format_version_mismatch() {
        let cache = WarmCache::new();
        let id = Uuid::new_v4();
        cache.put(id, engine(), "loro-1".to_string(), 5, vec![9, 9], 128);

        let result = cache
            .fork_matching(id, &key(5, &[9, 9], "loro-2"))
            .expect("fork call does not error");
        assert!(result.is_none());
    }

    #[test]
    fn an_entry_over_the_single_entry_ceiling_is_not_cached() {
        let cache = WarmCache::new();
        let id = Uuid::new_v4();
        let outcome = cache.put(
            id,
            engine(),
            "loro-1".to_string(),
            0,
            vec![],
            WARM_CACHE_ENTRY_DECODED_BYTES_MAX + 1,
        );
        assert_eq!(outcome, PutOutcome::TooLargeToCache);
        assert!(!cache.contains(id));
        assert_eq!(cache.total_bytes(), 0);
    }

    #[test]
    fn exactly_at_the_single_entry_ceiling_is_cached() {
        let cache = WarmCache::new();
        let id = Uuid::new_v4();
        let outcome = cache.put(
            id,
            engine(),
            "loro-1".to_string(),
            0,
            vec![],
            WARM_CACHE_ENTRY_DECODED_BYTES_MAX,
        );
        assert_eq!(outcome, PutOutcome::Cached);
        assert!(cache.contains(id));
    }

    #[test]
    fn entry_count_ceiling_evicts_the_least_recently_used() {
        let cache = WarmCache::new();
        let mut ids = Vec::new();
        for i in 0..WARM_CACHE_DOCUMENTS_MAX {
            let id = Uuid::new_v4();
            ids.push(id);
            cache.put(id, engine(), "loro-1".to_string(), i64::try_from(i).unwrap(), vec![], 1);
            // Force a strictly increasing `last_used` ordering so eviction order is deterministic
            // (two `Instant::now()` calls back to back can otherwise tie on some platforms).
            std::thread::sleep(std::time::Duration::from_micros(50));
        }
        assert_eq!(cache.len(), WARM_CACHE_DOCUMENTS_MAX);

        // One more document past the ceiling: the oldest (`ids[0]`) must be evicted, everything
        // else stays, and the count never exceeds the ceiling.
        let newest = Uuid::new_v4();
        cache.put(newest, engine(), "loro-1".to_string(), 999, vec![], 1);
        assert_eq!(
            cache.len(),
            WARM_CACHE_DOCUMENTS_MAX,
            "count must never exceed the ceiling"
        );
        assert!(
            !cache.contains(ids[0]),
            "the least-recently-used entry must have been evicted"
        );
        assert!(cache.contains(newest));
        assert!(
            cache.contains(ids[1]),
            "everything but the oldest survives a one-over insert"
        );
    }

    #[test]
    fn decoded_bytes_ceiling_evicts_until_the_new_entry_fits() {
        let cache = WarmCache::new();
        // Each entry sits exactly at the single-entry ceiling (128 MiB); the total budget is
        // 512 MiB = 4x that, so a 5th such entry cannot coexist with all 4 previous ones and
        // must force an LRU eviction on bytes alone -- 5 entries is nowhere near the 64-entry
        // count ceiling, so this exercises the byte budget specifically, not the count one.
        let per_entry = WARM_CACHE_ENTRY_DECODED_BYTES_MAX;
        let mut ids = Vec::new();
        for i in 0..4u32 {
            let id = Uuid::new_v4();
            ids.push(id);
            let outcome = cache.put(id, engine(), "loro-1".to_string(), i64::from(i), vec![], per_entry);
            assert_eq!(outcome, PutOutcome::Cached);
            std::thread::sleep(std::time::Duration::from_micros(50));
        }
        assert_eq!(cache.total_bytes(), per_entry.saturating_mul(4));
        assert_eq!(cache.total_bytes(), super::WARM_CACHE_DECODED_BYTES_MAX);

        // A 5th entry that would push total bytes over the ceiling must evict the oldest
        // (`ids[0]`) to make room, never simply exceed the byte budget.
        let fifth = Uuid::new_v4();
        let outcome = cache.put(fifth, engine(), "loro-1".to_string(), 4, vec![], per_entry);
        assert_eq!(outcome, PutOutcome::Cached);
        assert!(cache.total_bytes() <= super::WARM_CACHE_DECODED_BYTES_MAX);
        assert!(
            !cache.contains(ids[0]),
            "the least-recently-used entry must have been evicted for space"
        );
        assert!(cache.contains(ids[1]));
        assert!(cache.contains(ids[2]));
        assert!(cache.contains(ids[3]));
        assert!(cache.contains(fifth));
    }

    #[test]
    fn fork_matching_on_a_miss_returns_none_without_panicking() {
        let cache = WarmCache::new();
        assert!(
            cache
                .fork_matching(Uuid::new_v4(), &key(0, &[], "loro-1"))
                .expect("call does not error")
                .is_none()
        );
    }

    #[test]
    fn a_different_frontier_at_the_same_sequence_is_a_miss() {
        let cache = WarmCache::new();
        let id = Uuid::new_v4();
        cache.put(id, engine(), "loro-1".to_string(), 5, vec![9, 9], 128);
        assert!(
            cache.checkout(id, &key(5, &[9, 8], "loro-1"), true).unwrap().is_none(),
            "a re-created or rewritten document that reuses a sequence number must not hit"
        );
        assert!(cache.checkout(id, &key(5, &[9, 9], "loro-1"), true).unwrap().is_some());
    }

    #[test]
    fn checkout_returns_the_stored_bytes_and_forks_only_when_asked() {
        let cache = WarmCache::new();
        let id = Uuid::new_v4();
        let bytes = Arc::new(vec![1_u8, 2, 3]);
        cache.put_with_base(id, engine(), "loro-1".to_string(), 5, vec![9], 100, Some(bytes));
        assert_eq!(cache.total_bytes(), 103, "the bytes are accounted next to the engine");

        let bytes_only = cache.checkout(id, &key(5, &[9], "loro-1"), false).unwrap().unwrap();
        assert_eq!(bytes_only.base_bytes.as_deref(), Some(&vec![1_u8, 2, 3]));
        assert!(
            bytes_only.engine.is_none(),
            "no O(document) fork when only the bytes are needed"
        );

        let with_engine = cache.checkout(id, &key(5, &[9], "loro-1"), true).unwrap().unwrap();
        assert!(with_engine.engine.is_some());
    }

    #[test]
    fn an_entry_without_bytes_always_yields_an_engine_to_export_from() {
        let cache = WarmCache::new();
        let id = Uuid::new_v4();
        cache.put(id, engine(), "loro-1".to_string(), 5, vec![9], 100);
        let hit = cache.checkout(id, &key(5, &[9], "loro-1"), false).unwrap().unwrap();
        assert!(hit.base_bytes.is_none());
        assert!(hit.engine.is_some());
    }

    #[test]
    fn attach_base_bytes_only_attaches_to_the_exact_head() {
        let cache = WarmCache::new();
        let id = Uuid::new_v4();
        cache.put(id, engine(), "loro-1".to_string(), 5, vec![9], 100);
        cache.attach_base_bytes(id, &key(4, &[9], "loro-1"), Arc::new(vec![7; 10]));
        cache.attach_base_bytes(id, &key(5, &[8], "loro-1"), Arc::new(vec![7; 10]));
        assert_eq!(cache.total_bytes(), 100, "bytes for another head are ignored");
        assert!(
            cache
                .checkout(id, &key(5, &[9], "loro-1"), false)
                .unwrap()
                .unwrap()
                .base_bytes
                .is_none()
        );

        cache.attach_base_bytes(id, &key(5, &[9], "loro-1"), Arc::new(vec![7; 10]));
        assert_eq!(cache.total_bytes(), 110);
        assert!(
            cache
                .checkout(id, &key(5, &[9], "loro-1"), false)
                .unwrap()
                .unwrap()
                .base_bytes
                .is_some()
        );
    }

    #[test]
    fn a_replacement_drops_the_old_bytes_and_their_accounting() {
        let cache = WarmCache::new();
        let id = Uuid::new_v4();
        cache.put_with_base(
            id,
            engine(),
            "loro-1".to_string(),
            5,
            vec![9],
            100,
            Some(Arc::new(vec![1; 50])),
        );
        assert_eq!(cache.total_bytes(), 150);
        cache.put(id, engine(), "loro-1".to_string(), 6, vec![10], 100);
        assert_eq!(cache.total_bytes(), 100);
        assert!(cache.checkout(id, &key(5, &[9], "loro-1"), true).unwrap().is_none());
        assert!(
            cache
                .checkout(id, &key(6, &[10], "loro-1"), false)
                .unwrap()
                .unwrap()
                .base_bytes
                .is_none()
        );
    }

    #[test]
    fn a_seed_never_replaces_an_entry_that_is_ahead_of_it() {
        let cache = WarmCache::new();
        let id = Uuid::new_v4();
        cache.put_with_base(
            id,
            engine(),
            "loro-1".into(),
            7,
            vec![7],
            100,
            Some(Arc::new(vec![1; 10])),
        );

        let stale = cache.seed_with_base(
            id,
            engine(),
            "loro-1".into(),
            6,
            vec![6],
            100,
            Some(Arc::new(vec![2; 10])),
        );
        assert_eq!(stale, PutOutcome::Superseded);
        assert_eq!(
            cache.total_bytes(),
            110,
            "the newer entry and its accounting are untouched"
        );
        let kept = cache.checkout(id, &key(7, &[7], "loro-1"), false).unwrap().unwrap();
        assert_eq!(kept.base_bytes.as_deref().map(Vec::as_slice), Some(&[1u8; 10][..]));

        // The same sequence with another frontier (a restored history) and a newer sequence both
        // replace the entry.
        let same = cache.seed_with_base(id, engine(), "loro-1".into(), 7, vec![8], 100, None);
        assert_eq!(same, PutOutcome::Cached);
        assert!(cache.checkout(id, &key(7, &[8], "loro-1"), false).unwrap().is_some());
        let newer = cache.seed_with_base(id, engine(), "loro-1".into(), 9, vec![9], 100, None);
        assert_eq!(newer, PutOutcome::Cached);
        assert!(cache.checkout(id, &key(9, &[9], "loro-1"), false).unwrap().is_some());
    }

    #[test]
    fn invalidate_removes_the_entry_and_its_bytes() {
        let cache = WarmCache::new();
        let id = Uuid::new_v4();
        cache.put_with_base(
            id,
            engine(),
            "loro-1".to_string(),
            5,
            vec![9],
            100,
            Some(Arc::new(vec![1; 50])),
        );
        cache.invalidate(id);
        assert!(!cache.contains(id));
        assert_eq!(cache.total_bytes(), 0);
        assert!(cache.checkout(id, &key(5, &[9], "loro-1"), true).unwrap().is_none());
        cache.invalidate(id);
    }

    #[test]
    fn bytes_that_would_exceed_the_single_entry_ceiling_are_dropped_not_the_entry() {
        let cache = WarmCache::new();
        let id = Uuid::new_v4();
        let outcome = cache.put_with_base(
            id,
            engine(),
            "loro-1".to_string(),
            5,
            vec![9],
            WARM_CACHE_ENTRY_DECODED_BYTES_MAX - 1,
            Some(Arc::new(vec![0; 2])),
        );
        assert_eq!(outcome, PutOutcome::Cached);
        assert_eq!(cache.total_bytes(), WARM_CACHE_ENTRY_DECODED_BYTES_MAX - 1);
        assert!(
            cache
                .checkout(id, &key(5, &[9], "loro-1"), false)
                .unwrap()
                .unwrap()
                .base_bytes
                .is_none()
        );
    }

    #[test]
    fn the_byte_budget_counts_cached_bytes_and_evicts_the_least_recently_used() {
        let cache = WarmCache::new();
        let ceiling = WARM_CACHE_ENTRY_DECODED_BYTES_MAX;
        let mut ids = Vec::new();
        // Four entries that each account for exactly the single-entry ceiling (engine hint plus
        // 100 bytes of base bytes) fill the 4x total budget; a fifth must evict the oldest.
        for i in 0..5_i64 {
            let id = Uuid::new_v4();
            ids.push(id);
            let bytes = Arc::new(vec![0_u8; 100]);
            cache.put_with_base(
                id,
                engine(),
                "loro-1".to_string(),
                i,
                vec![],
                ceiling - 100,
                Some(bytes),
            );
            std::thread::sleep(std::time::Duration::from_micros(50));
        }
        assert!(cache.total_bytes() <= super::WARM_CACHE_DECODED_BYTES_MAX);
        assert_eq!(
            cache.total_bytes(),
            ceiling * 4,
            "the evicted entry's bytes left the total"
        );
        assert!(!cache.contains(ids[0]), "the oldest entry was evicted to make room");
        assert!(cache.contains(ids[4]));
    }

    #[test]
    fn attaching_bytes_never_evicts_the_entry_it_grows_and_stays_within_the_budget() {
        let cache = WarmCache::new();
        let ceiling = WARM_CACHE_ENTRY_DECODED_BYTES_MAX;
        let mut ids = Vec::new();
        for i in 0..4_i64 {
            let id = Uuid::new_v4();
            ids.push(id);
            cache.put(id, engine(), "loro-1".to_string(), i, vec![1], ceiling - 500);
            std::thread::sleep(std::time::Duration::from_micros(50));
        }
        let small = Uuid::new_v4();
        cache.put(small, engine(), "loro-1".to_string(), 9, vec![2], 500);
        assert_eq!(cache.len(), 5, "everything still fits");
        std::thread::sleep(std::time::Duration::from_micros(50));

        cache.attach_base_bytes(small, &key(9, &[2], "loro-1"), Arc::new(vec![0; 2000]));
        assert!(cache.contains(small), "the entry that grew is never the one evicted");
        assert!(
            !cache.contains(ids[0]),
            "the least recently used other entry paid for the growth"
        );
        assert!(cache.total_bytes() <= super::WARM_CACHE_DECODED_BYTES_MAX);
    }
}
