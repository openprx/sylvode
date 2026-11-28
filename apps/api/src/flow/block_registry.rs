//! Private Page identity registry. No recipient DTO or authorization is derived from these rows.
//! Caller must hold the accepted Page-head lock and use its existing `DatabaseTransaction`.
use super::collab::limits::DOCUMENT_LOCK_HOLD_MS_MAX;
use crate::error::ApiError;
use collab_core::block_identity::{Alias, AliasState, reconcile_alias};
use sea_orm::{ConnectionTrait, DatabaseTransaction, DbBackend, FromQueryResult, Statement};
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::{Duration, Instant};
use uuid::Uuid;

#[derive(Clone, Debug)]
pub struct PreparedBlock {
    pub physical_cid: String,
    pub parent_physical_cid: Option<String>,
    pub order_key: String,
    pub asset_id: Option<Uuid>,
    /// Readable identifier a tree-prose Page carried before it was initialized into the editor doc.
    pub tree_block_alias: Option<String>,
    /// Digest of normalized typed content only; never Page metadata or private comments.
    pub content_hash: [u8; 32],
}
/// Prepare hashes outside the accepted-head lock from validated active semantic content.
/// Unknown property bags, private discussions, source parent/title/sequence are excluded.
pub fn prepare(snapshot: &collab_core::SemanticSnapshot) -> Result<Vec<PreparedBlock>, ApiError> {
    use sha2::{Digest, Sha256};
    let mut result = Vec::new();
    let canonical = snapshot.nodes.values().any(|node| {
        node.properties
            .get("content_source")
            .is_some_and(|source| source == "editor_doc")
    });
    for (cid, node) in &snapshot.nodes {
        if node.deleted
            || node.kind != collab_core::NodeKind::Block
            || (canonical
                && node
                    .properties
                    .get("content_source")
                    .is_none_or(|source| source != "editor_doc"))
        {
            continue;
        }
        let kind = node.properties.get("block_type").map_or("paragraph", String::as_str);
        let slots: &[&str] = match kind {
            "heading" => &["level"],
            "orderedItem" => &["order"],
            "todoItem" => &["checked"],
            "toggle" => &["collapsed"],
            "image" => &["asset_id", "alt", "caption"],
            "attachment" => &["asset_id", "name", "mime", "caption"],
            // Legacy opaque blocks receive identity, but cannot be published by the typed projector.
            _ => &[],
        };
        let mut content = serde_json::Map::new();
        content.insert("kind".into(), serde_json::json!(kind));
        content.insert(
            "text".into(),
            serde_json::json!(if kind == "childPage" { "" } else { &node.text }),
        );
        for key in slots {
            if let Some(value) = node.properties.get(*key) {
                content.insert((*key).into(), serde_json::json!(value));
            }
        }
        let spans: Vec<collab_core::page_inline::InlineSpan> = node
            .properties
            .get("inline_spans")
            .map(|raw| serde_json::from_str(raw))
            .transpose()
            .map_err(|_| ApiError::invalid_update("invalid typed inline spans"))?
            .unwrap_or_default();
        for span in &spans {
            if span.start >= span.end
                || span.end > node.text.len()
                || !node.text.is_char_boundary(span.start)
                || !node.text.is_char_boundary(span.end)
                || (span.mark == collab_core::page_inline::InlineMark::Link
                    && !span.href.as_deref().is_some_and(collab_core::page_inline::safe_href))
                || (span.mark != collab_core::page_inline::InlineMark::Link && span.href.is_some())
            {
                return Err(ApiError::invalid_update("invalid shared inline range"));
            }
        }
        content.insert(
            "spans".into(),
            serde_json::to_value(spans).map_err(|_| ApiError::internal())?,
        );
        let encoded = serde_json::to_vec(&content).map_err(|_| ApiError::internal())?;
        result.push(PreparedBlock {
            physical_cid: cid.to_string(),
            parent_physical_cid: node.parent.as_ref().map(ToString::to_string),
            tree_block_alias: node
                .properties
                .get("logical_id")
                .filter(|alias| !alias.is_empty() && alias.chars().count() <= 128 && alias.len() <= 256)
                .cloned(),
            order_key: node.order_key.clone(),
            asset_id: if matches!(kind, "image" | "attachment") {
                Some(
                    node.properties
                        .get("asset_id")
                        .and_then(|id| Uuid::parse_str(id).ok())
                        .ok_or_else(|| ApiError::invalid_update("invalid typed asset reference"))?,
                )
            } else {
                None
            },
            content_hash: Sha256::digest(encoded).into(),
        });
    }
    Ok(result)
}

/// Digest source-independent selected content and relationships. A single block's private
/// parent/order never changes this value; relative selected member ordering does.
pub fn visible_graph_hash(blocks: &[PreparedBlock], members: &[(Uuid, String)]) -> Result<[u8; 32], ApiError> {
    use sha2::{Digest, Sha256};
    let by_cid = blocks
        .iter()
        .map(|block| (block.physical_cid.as_str(), block))
        .collect::<std::collections::BTreeMap<_, _>>();
    let member_by_cid = members
        .iter()
        .map(|(id, cid)| (cid.as_str(), *id))
        .collect::<std::collections::BTreeMap<_, _>>();
    let mut selected = Vec::new();
    for (id, cid) in members {
        let block = by_cid
            .get(cid.as_str())
            .ok_or_else(|| ApiError::invalid_update("missing selected logical placement"))?;
        selected.push((*id, *block));
    }
    selected
        .sort_by(|(left_id, left), (right_id, right)| left.order_key.cmp(&right.order_key).then(left_id.cmp(right_id)));
    let mut normalized = Vec::new();
    for (id, block) in selected {
        let parent = block
            .parent_physical_cid
            .as_deref()
            .and_then(|cid| member_by_cid.get(cid))
            .copied();
        normalized.push((id, parent, block.content_hash));
    }
    let bytes = serde_json::to_vec(&normalized).map_err(|_| ApiError::internal())?;
    Ok(Sha256::digest(bytes).into())
}

/// Test-only statement recorder: proves the statement count and the rows per statement of
/// `reconcile` do not depend on page size, and keeps the elapsed time of every statement so a test
/// can print where the time goes.
#[cfg(test)]
pub(crate) mod statement_probe {
    use std::cell::RefCell;
    use std::future::Future;
    use std::time::Duration;

    /// One recorded statement: the rows it is bounded to carry and how long it took.
    #[derive(Clone, Debug)]
    pub struct Record {
        pub rows: usize,
        pub elapsed: Duration,
        pub label: String,
    }

    tokio::task_local! {
        static RECORDS: RefCell<Vec<Record>>;
    }

    /// Record one finished database round trip for the surrounding `count` scope, if any.
    pub fn record(rows: usize, sql: &str, elapsed: Duration) {
        let label = sql.split_whitespace().take(6).collect::<Vec<_>>().join(" ");
        let _ = RECORDS.try_with(|records| {
            records.borrow_mut().push(Record { rows, elapsed, label });
        });
    }

    /// Run `future` and return its output with the statements recorded while it ran.
    pub async fn count<F: Future>(future: F) -> (F::Output, Vec<Record>) {
        RECORDS
            .scope(RefCell::new(Vec::new()), async {
                let output = future.await;
                (output, RECORDS.with(|records| records.borrow().clone()))
            })
            .await
    }

    /// The `limit` slowest statements, slowest first, as printable lines.
    pub fn slowest(records: &[Record], limit: usize) -> Vec<String> {
        let mut ordered = records.iter().collect::<Vec<_>>();
        ordered.sort_by_key(|record| std::cmp::Reverse(record.elapsed));
        ordered
            .into_iter()
            .take(limit)
            .map(|record| format!("{:?} rows={} {}", record.elapsed, record.rows, record.label))
            .collect()
    }
}

/// Maximum number of rows any registry statement whose size depends on the page carries. The
/// per-statement cap (`DOCUMENT_LOCK_HOLD_MS_MAX`, 100 ms) was reached by a single statement of
/// about 10,000 rows on a loaded database, so every such statement (the locking read, the removal
/// scan, the retirement, every bulk write) is issued in chunks of at most this
/// many rows. The statement count therefore depends only on the number of chunks.
pub(crate) const REGISTRY_ROWS_PER_STATEMENT: usize = 500;

/// Upper bound on new placements one bounded transaction registers, so one write never adds more
/// than this many rows to the lock wait of a collaborator, even on fast hardware.
pub(crate) const REGISTRATION_ROWS_PER_TRANSACTION_MAX: usize = 4 * REGISTRY_ROWS_PER_STATEMENT;

/// Part of the staging budget kept back after registration for the checkpoint and share statements.
pub(crate) const REGISTRATION_TAIL_RESERVE: Duration = Duration::from_millis(DOCUMENT_LOCK_HOLD_MS_MAX);

/// How many blocks without an active placement ("pending" blocks) one transaction may register.
#[derive(Clone, Copy, Debug)]
pub(crate) enum RegistrationAllowance {
    /// Register every pending block in this transaction (creation, import, packages).
    Complete,
    /// Register pending blocks in chunk groups while the deadline and the row bound allow.
    Bounded {
        /// No further group starts when `now + the last group's measured duration` passes it.
        deadline: Instant,
        /// Upper bound on newly registered rows in this transaction.
        max_rows: usize,
        /// Groups are admitted regardless of the deadline until this many rows are registered,
        /// which guarantees progress to callers that loop until the page is complete.
        min_rows: usize,
    },
}

impl RegistrationAllowance {
    /// The write-path allowance: the deadline is `REGISTRATION_TAIL_RESERVE` before the end of a
    /// staging `budget` that started at `started`.
    pub(crate) fn within(started: Instant, budget: Duration) -> Self {
        Self::bounded(started, budget, REGISTRATION_ROWS_PER_TRANSACTION_MAX, 0)
    }

    /// Like [`Self::within`] with explicit row bounds.
    pub(crate) fn bounded(started: Instant, budget: Duration, max_rows: usize, min_rows: usize) -> Self {
        Self::Bounded {
            deadline: started
                .checked_add(budget.saturating_sub(REGISTRATION_TAIL_RESERVE))
                .unwrap_or(started),
            max_rows,
            min_rows,
        }
    }
}

/// What one `reconcile` call did beyond its writes.
#[derive(Debug)]
pub(crate) struct ReconcileOutcome {
    /// Logical ids whose shares were suspended; private audit metadata, read by tests only.
    #[cfg(test)]
    pub suspended: Vec<Uuid>,
    /// Snapshot blocks still without an active placement after this transaction.
    pub pending: usize,
    /// Whether the incremental path ran; tests only.
    #[cfg(test)]
    pub incremental: bool,
}

/// Ranges of at most `REGISTRY_ROWS_PER_STATEMENT` consecutive rows covering `0..len`, in order.
fn chunk_ranges(len: usize) -> impl Iterator<Item = std::ops::Range<usize>> {
    (0..len)
        .step_by(REGISTRY_ROWS_PER_STATEMENT)
        .map(move |start| start..(start + REGISTRY_ROWS_PER_STATEMENT).min(len))
}

/// The rows of one chunk as an owned column, or an internal error if the range is out of bounds.
fn column<T: Clone>(rows: &[T], range: &std::ops::Range<usize>) -> Result<Vec<T>, ApiError> {
    rows.get(range.clone())
        .map(<[T]>::to_vec)
        .ok_or_else(ApiError::internal)
}

/// Run one registry statement. `rows` is the number of rows the statement is bounded to carry; it
/// never exceeds `REGISTRY_ROWS_PER_STATEMENT`, and tests record it with the elapsed time.
pub(crate) async fn execute(
    tx: &DatabaseTransaction,
    rows: usize,
    sql: &str,
    values: Vec<sea_orm::Value>,
) -> Result<sea_orm::ExecResult, ApiError> {
    debug_assert!(rows <= REGISTRY_ROWS_PER_STATEMENT);
    #[cfg(test)]
    let started = Instant::now();
    let result = tx
        .execute(Statement::from_sql_and_values(DbBackend::Postgres, sql, values))
        .await;
    #[cfg(test)]
    statement_probe::record(rows, sql, started.elapsed());
    Ok(result?)
}

/// Like [`execute`], for a statement that returns rows.
pub(crate) async fn query_all(
    tx: &DatabaseTransaction,
    rows: usize,
    sql: &str,
    values: Vec<sea_orm::Value>,
) -> Result<Vec<sea_orm::QueryResult>, ApiError> {
    debug_assert!(rows <= REGISTRY_ROWS_PER_STATEMENT);
    #[cfg(test)]
    let started = Instant::now();
    let result = tx
        .query_all(Statement::from_sql_and_values(DbBackend::Postgres, sql, values))
        .await;
    #[cfg(test)]
    statement_probe::record(rows, sql, started.elapsed());
    Ok(result?)
}

/// Like [`execute`], for a statement that returns at most one row.
pub(crate) async fn query_one(
    tx: &DatabaseTransaction,
    sql: &str,
    values: Vec<sea_orm::Value>,
) -> Result<Option<sea_orm::QueryResult>, ApiError> {
    #[cfg(test)]
    let started = Instant::now();
    let result = tx
        .query_one(Statement::from_sql_and_values(DbBackend::Postgres, sql, values))
        .await;
    #[cfg(test)]
    statement_probe::record(1, sql, started.elapsed());
    Ok(result?)
}

/// Latest placement of one physical CID, as locked by the chunked read of `reconcile`.
#[derive(FromQueryResult)]
struct LatestPlacement {
    id: Uuid,
    physical_cid: String,
    logical_id: Uuid,
    generation: i64,
    state: String,
    content_hash: Vec<u8>,
    /// Alias fence of the CID, if one exists; read without locking it.
    fence_first_logical_id: Option<Uuid>,
    fence_ever_retired: Option<bool>,
}

/// Column-wise bulk write plan built by the decision pass; no database access happens while filling it.
#[derive(Default)]
struct WritePlan {
    fresh_ids: Vec<Uuid>,
    fence_cids: Vec<String>,
    fence_logical: Vec<Uuid>,
    fence_retired: Vec<bool>,
    hash_ids: Vec<Uuid>,
    hash_values: Vec<Vec<u8>>,
    activate_ids: Vec<Uuid>,
    new_ids: Vec<Uuid>,
    new_logical: Vec<Uuid>,
    new_cids: Vec<String>,
    new_generations: Vec<i64>,
    new_hashes: Vec<Vec<u8>>,
}

/// Decide one block against the latest placement of its CID and append its writes to `plan`.
fn plan_block(
    plan: &mut WritePlan,
    suspend: &mut Vec<Uuid>,
    block: &PreparedBlock,
    old: Option<&LatestPlacement>,
) -> Result<(), ApiError> {
    let alias = old.map(|old| Alias {
        logical_id: old.logical_id,
        generation: old.generation.unsigned_abs(),
        state: match old.state.as_str() {
            "active" => AliasState::Active,
            "deleted" => AliasState::Deleted,
            _ => AliasState::Moved,
        },
    });
    let decision = reconcile_alias(alias, Uuid::new_v4())
        .map_err(|_| ApiError::invalid_update("logical placement generation conflict"))?;
    let generation = i64::try_from(decision.generation)
        .map_err(|_| ApiError::invalid_update("logical placement generation exhausted"))?;
    if decision.fresh_identity {
        plan.fresh_ids.push(decision.logical_id);
    }
    let retired = alias.is_some_and(|alias| alias.state != AliasState::Active);
    // The fence upsert changes a row only when it is missing, or not yet retired and either
    // retired now or pointing at another first identity; skip every fence that is already right.
    let fence_changes = old.is_none_or(|old| match (old.fence_first_logical_id, old.fence_ever_retired) {
        (Some(first), Some(ever_retired)) => !ever_retired && (retired || first != decision.logical_id),
        _ => true,
    });
    if fence_changes {
        plan.fence_cids.push(block.physical_cid.clone());
        plan.fence_logical.push(decision.logical_id);
        plan.fence_retired.push(retired);
    }
    if let Some(old) = old.filter(|row| row.state == "active") {
        if old.content_hash != block.content_hash {
            plan.hash_ids.push(old.id);
            plan.hash_values.push(block.content_hash.to_vec());
        }
    } else {
        plan.activate_ids.push(decision.logical_id);
        plan.new_ids.push(Uuid::new_v4());
        plan.new_logical.push(decision.logical_id);
        plan.new_cids.push(block.physical_cid.clone());
        plan.new_generations.push(generation);
        plan.new_hashes.push(block.content_hash.to_vec());
    }
    if decision.suspend_shares {
        suspend.push(decision.logical_id);
    }
    Ok(())
}

/// Execute the chunked write statements of one plan, in dependency order.
async fn flush_plan(tx: &DatabaseTransaction, scope_id: Uuid, page_id: Uuid, plan: &WritePlan) -> Result<(), ApiError> {
    for range in chunk_ranges(plan.fresh_ids.len()) {
        execute(
            tx,
            range.len(),
            "INSERT INTO flow_logical_blocks(id,state) SELECT v.id,'active' FROM unnest($1::uuid[]) WITH ORDINALITY AS v(id,ord) ORDER BY v.ord",
            vec![column(&plan.fresh_ids, &range)?.into()],
        )
        .await?;
    }
    for range in chunk_ranges(plan.fence_cids.len()) {
        // Data-identical to the former per-block upsert, but an unchanged fence is not rewritten.
        execute(
            tx,
            range.len(),
            "INSERT INTO flow_block_alias_fences(scope_id,page_id,physical_cid,first_logical_id,ever_retired) SELECT $1,$2,v.cid,v.logical_id,v.retired FROM unnest($3::text[],$4::uuid[],$5::bool[]) WITH ORDINALITY AS v(cid,logical_id,retired,ord) ORDER BY v.ord ON CONFLICT(scope_id,page_id,physical_cid) DO UPDATE SET ever_retired=true WHERE NOT flow_block_alias_fences.ever_retired AND (EXCLUDED.ever_retired OR flow_block_alias_fences.first_logical_id<>EXCLUDED.first_logical_id)",
            vec![
                scope_id.into(),
                page_id.into(),
                column(&plan.fence_cids, &range)?.into(),
                column(&plan.fence_logical, &range)?.into(),
                column(&plan.fence_retired, &range)?.into(),
            ],
        )
        .await?;
    }
    for range in chunk_ranges(plan.hash_ids.len()) {
        execute(
            tx,
            range.len(),
            "UPDATE flow_block_placements p SET content_hash=v.hash FROM unnest($1::uuid[],$2::bytea[]) AS v(id,hash) WHERE p.id=v.id AND p.content_hash IS DISTINCT FROM v.hash",
            vec![
                column(&plan.hash_ids, &range)?.into(),
                column(&plan.hash_values, &range)?.into(),
            ],
        )
        .await?;
    }
    for range in chunk_ranges(plan.activate_ids.len()) {
        execute(
            tx,
            range.len(),
            "UPDATE flow_logical_blocks SET state='active' WHERE id=ANY($1::uuid[]) AND state<>'active'",
            vec![column(&plan.activate_ids, &range)?.into()],
        )
        .await?;
    }
    for range in chunk_ranges(plan.new_ids.len()) {
        execute(
            tx,
            range.len(),
            "INSERT INTO flow_block_placements(id,logical_id,scope_id,page_id,physical_cid,generation,state,content_hash) SELECT v.id,v.logical_id,$1,$2,v.cid,v.generation,'active',v.hash FROM unnest($3::uuid[],$4::uuid[],$5::text[],$6::int8[],$7::bytea[]) WITH ORDINALITY AS v(id,logical_id,cid,generation,hash,ord) ORDER BY v.ord",
            vec![
                scope_id.into(),
                page_id.into(),
                column(&plan.new_ids, &range)?.into(),
                column(&plan.new_logical, &range)?.into(),
                column(&plan.new_cids, &range)?.into(),
                column(&plan.new_generations, &range)?.into(),
                column(&plan.new_hashes, &range)?.into(),
            ],
        )
        .await?;
    }
    Ok(())
}

/// Which reconcile a caller asks for. The caller never decides soundness: `Incremental` is only a
/// permission, and the guard in [`reconcile_in_mode`] decides whether it is used.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ReconcileMode {
    /// Every block of the Page is read, locked and compared, and every active placement is scanned.
    Full,
    /// Only blocks whose (CID, content hash) differ from the registry baseline are read and locked,
    /// and only CIDs that left the document are retired, when the checkpoint guard allows it.
    Incremental,
}

/// The (CID, content hash) set a reconcile left in the registry, remembered per Page by this process.
struct BaselineEntry {
    head: i64,
    digest: [u8; 32],
    blocks: Arc<HashMap<String, [u8; 32]>>,
}

/// Process-wide memory of the last registry states. It is only a hint: an entry is used when the
/// committed checkpoint names the same head and the same digest, and never otherwise.
#[derive(Default)]
struct BaselineCache {
    pages: HashMap<Uuid, (u64, Vec<BaselineEntry>)>,
    clock: u64,
    cids: usize,
}

/// Pages remembered at most, and CIDs remembered in total (about 30 MB at the largest CID size).
const BASELINE_PAGES_MAX: usize = 64;
const BASELINE_CIDS_MAX: usize = 200_000;
/// Entries kept per Page: the newest and its predecessor, so a rolled-back write does not make the
/// next write fall back to the full reconcile.
const BASELINE_ENTRIES_PER_PAGE: usize = 2;

#[cfg(not(test))]
static BASELINES: std::sync::LazyLock<parking_lot::Mutex<BaselineCache>> = std::sync::LazyLock::new(Default::default);

#[cfg(test)]
thread_local! {
    /// Tests run one per thread on a current-thread runtime, so a per-thread memory gives every
    /// test the cache of its own Pages: a test running in parallel can neither evict its entries
    /// (the page and CID bounds are process-wide) nor see them.
    static TEST_BASELINES: std::cell::RefCell<BaselineCache> = std::cell::RefCell::new(BaselineCache::default());
}

/// Run `f` on the remembered registry states of this process (of this test thread under `cfg(test)`).
fn with_baselines<R>(f: impl FnOnce(&mut BaselineCache) -> R) -> R {
    #[cfg(not(test))]
    {
        f(&mut BASELINES.lock())
    }
    #[cfg(test)]
    {
        TEST_BASELINES.with(|cache| f(&mut cache.borrow_mut()))
    }
}

impl BaselineCache {
    fn find(&mut self, page: Uuid, head: i64, digest: &[u8]) -> Option<Arc<HashMap<String, [u8; 32]>>> {
        self.clock = self.clock.wrapping_add(1);
        let clock = self.clock;
        let (stamp, entries) = self.pages.get_mut(&page)?;
        let entry = entries
            .iter()
            .find(|entry| entry.head == head && entry.digest.as_slice() == digest)?;
        *stamp = clock;
        Some(Arc::clone(&entry.blocks))
    }

    fn store(&mut self, page: Uuid, entry: BaselineEntry) {
        self.clock = self.clock.wrapping_add(1);
        let clock = self.clock;
        let slot = self.pages.entry(page).or_insert_with(|| (clock, Vec::new()));
        slot.0 = clock;
        slot.1.retain(|old| old.head != entry.head);
        slot.1.insert(0, entry);
        slot.1.truncate(BASELINE_ENTRIES_PER_PAGE);
        self.recount();
        while (self.pages.len() > BASELINE_PAGES_MAX || self.cids > BASELINE_CIDS_MAX) && self.pages.len() > 1 {
            let oldest = self
                .pages
                .iter()
                .filter(|(id, _)| **id != page)
                .min_by_key(|(_, (stamp, _))| *stamp)
                .map(|(id, _)| *id);
            let Some(oldest) = oldest else { break };
            self.pages.remove(&oldest);
            self.recount();
        }
    }

    fn forget(&mut self, page: Uuid) {
        self.pages.remove(&page);
        self.recount();
    }

    fn recount(&mut self) {
        self.cids = self
            .pages
            .values()
            .flat_map(|(_, entries)| entries.iter())
            .map(|entry| entry.blocks.len())
            .sum();
    }
}

/// Digest of the registry-relevant content of a block set: the sorted (CID, content hash) pairs.
/// Parent, order and the other `PreparedBlock` fields never reach a placement row.
fn registry_digest(blocks: &[PreparedBlock]) -> [u8; 32] {
    use sha2::{Digest, Sha256};
    let mut ordered = blocks.iter().collect::<Vec<_>>();
    ordered.sort_unstable_by(|left, right| left.physical_cid.cmp(&right.physical_cid));
    let mut hasher = Sha256::new();
    for block in ordered {
        hasher.update(block.physical_cid.len().to_be_bytes());
        hasher.update(block.physical_cid.as_bytes());
        hasher.update(block.content_hash);
    }
    hasher.finalize().into()
}

/// What changed between the baseline and the new block set, in document order.
struct Delta<'a> {
    /// Blocks whose CID is new or whose content hash differs: the only blocks that are locked and planned.
    touched: Vec<&'a PreparedBlock>,
    /// CIDs of the baseline that are no longer in the document.
    removed: Vec<String>,
}

fn diff<'a>(baseline: &HashMap<String, [u8; 32]>, blocks: &'a [PreparedBlock]) -> Delta<'a> {
    #[cfg(test)]
    let mutation = mutation::current();
    let present = blocks
        .iter()
        .map(|block| block.physical_cid.as_str())
        .collect::<HashSet<_>>();
    let mut removed = baseline
        .keys()
        .filter(|cid| !present.contains(cid.as_str()))
        .cloned()
        .collect::<Vec<_>>();
    removed.sort_unstable();
    let mut touched = Vec::new();
    for block in blocks {
        match baseline.get(&block.physical_cid) {
            None => touched.push(block),
            Some(hash) if *hash != block.content_hash => touched.push(block),
            Some(_) => {}
        }
    }
    #[cfg(test)]
    mutation::apply(mutation, baseline, &mut touched, &mut removed);
    Delta { touched, removed }
}

/// Deliberate bugs for the mutation checks of the differential test. Task-local, so a mutated test
/// never disturbs a test running in parallel.
#[cfg(test)]
pub(crate) mod mutation {
    use super::PreparedBlock;
    use std::collections::HashMap;

    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    pub enum Mutation {
        /// CIDs that left the document are never retired.
        SkipDeletes,
        /// The guard accepts the newest remembered state without comparing it with the checkpoint.
        TrustStaleCheckpoint,
        /// A new CID whose content equals a removed CID is treated as the same (moved) block.
        MissMovedBlock,
        /// Content changes of a CID that stays are ignored.
        IgnoreContentChanges,
    }

    tokio::task_local! {
        static ACTIVE: Mutation;
    }

    pub fn current() -> Option<Mutation> {
        ACTIVE.try_with(|mutation| *mutation).ok()
    }

    pub async fn with<F: std::future::Future>(mutation: Mutation, future: F) -> F::Output {
        ACTIVE.scope(mutation, future).await
    }

    pub fn apply(
        mutation: Option<Mutation>,
        baseline: &HashMap<String, [u8; 32]>,
        touched: &mut Vec<&PreparedBlock>,
        removed: &mut Vec<String>,
    ) {
        match mutation {
            Some(Mutation::SkipDeletes) => removed.clear(),
            Some(Mutation::MissMovedBlock) => {
                let removed_hashes = removed.iter().filter_map(|cid| baseline.get(cid)).collect::<Vec<_>>();
                touched.retain(|block| {
                    baseline.contains_key(&block.physical_cid) || !removed_hashes.contains(&&block.content_hash)
                });
            }
            Some(Mutation::IgnoreContentChanges) => touched.retain(|block| !baseline.contains_key(&block.physical_cid)),
            Some(Mutation::TrustStaleCheckpoint) | None => {}
        }
    }
}

/// Compare the registry of one Page with the full reconcile of `blocks`, writing and locking nothing.
///
/// Returns up to `limit` human-readable differences. An empty list means one active placement per
/// block with the block's content hash and an active logical identity, no other active placement,
/// and nothing pending.
///
/// Statements: one keyset scan round per `REGISTRY_ROWS_PER_STATEMENT` active placements and one
/// checkpoint read. Meant for tests and for a later periodic or on-demand verification.
pub async fn verify_registry(
    tx: &DatabaseTransaction,
    scope_id: Uuid,
    page_id: Uuid,
    blocks: &[PreparedBlock],
    limit: usize,
) -> Result<Vec<String>, ApiError> {
    let expected = blocks
        .iter()
        .map(|block| (block.physical_cid.as_str(), block.content_hash))
        .collect::<HashMap<_, _>>();
    let mut differences = Vec::new();
    let mut note = |text: String| {
        if differences.len() < limit {
            differences.push(text);
        }
    };
    let pending = query_one(
        tx,
        "SELECT pending_registrations FROM flow_block_checkpoints WHERE scope_id=$1 AND page_id=$2",
        vec![scope_id.into(), page_id.into()],
    )
    .await?
    .map(|row| row.try_get::<i32>("", "pending_registrations"))
    .transpose()?;
    if pending != Some(0) {
        note(format!("pending registrations: {pending:?}"));
    }
    let scan_limit = i64::try_from(REGISTRY_ROWS_PER_STATEMENT).map_err(|_| ApiError::internal())?;
    let mut seen = HashSet::new();
    let mut cursor = String::new();
    loop {
        let rows = query_all(
            tx,
            REGISTRY_ROWS_PER_STATEMENT,
            "SELECT p.physical_cid,p.content_hash,l.state AS logical_state FROM flow_block_placements p JOIN flow_logical_blocks l ON l.id=p.logical_id WHERE p.scope_id=$1 AND p.page_id=$2 AND p.state='active' AND p.physical_cid>$3 ORDER BY p.physical_cid LIMIT $4",
            vec![scope_id.into(), page_id.into(), cursor.clone().into(), scan_limit.into()],
        )
        .await?;
        let round = rows.len();
        for row in rows {
            cursor = row.try_get::<String>("", "physical_cid")?;
            let hash = row.try_get::<Vec<u8>>("", "content_hash")?;
            match expected.get(cursor.as_str()) {
                None => note(format!("active placement without a block: {cursor}")),
                Some(want) if want.as_slice() != hash.as_slice() => note(format!("content hash differs: {cursor}")),
                Some(_) => {}
            }
            if row.try_get::<String>("", "logical_state")? != "active" {
                note(format!("active placement of an inactive identity: {cursor}"));
            }
            seen.insert(cursor.clone());
        }
        if round < REGISTRY_ROWS_PER_STATEMENT {
            break;
        }
    }
    for cid in expected.keys() {
        if !seen.contains(*cid) {
            note(format!("block without an active placement: {cid}"));
        }
    }
    Ok(differences)
}

/// Reconcile one accepted head. No independent commit, connection or raw client identity input.
///
/// Multi-Page operations must lock all Page heads in UUID order before calling this function.
/// Affected shares are suspended in this transaction; returned IDs are private audit metadata.
///
/// Rows whose content did not change are neither rewritten nor carried by any write statement.
/// Every statement whose size depends on the page carries at most `REGISTRY_ROWS_PER_STATEMENT`
/// rows: the locking read and the removal scan walk the page in chunks, and so do all writes.
/// Blocks that have an active placement are always processed in full. Blocks without one ("pending")
/// are registered in chunk groups while `allowance` permits; the rest stays pending, is counted in
/// `ReconcileOutcome::pending` and in the checkpoint, and is registered by a later call..
pub(crate) async fn reconcile(
    tx: &DatabaseTransaction,
    scope_id: Uuid,
    page_id: Uuid,
    accepted_head: i64,
    blocks: &[PreparedBlock],
    allowance: RegistrationAllowance,
) -> Result<ReconcileOutcome, ApiError> {
    reconcile_in_mode(
        tx,
        scope_id,
        page_id,
        accepted_head,
        blocks,
        allowance,
        ReconcileMode::Full,
    )
    .await
}

/// [`reconcile`] with a choice of mode.
///
/// The incremental path is taken only when ALL of these hold, read under the checkpoint row lock
/// (so they cannot change before this transaction ends):
/// 1. `mode` is `Incremental` and `allowance` is `Bounded` (the accepted-write path; creation,
///    import and package callers pass `Complete` and always reconcile in full);
/// 2. the checkpoint has no pending registrations;
/// 3. `accepted_head` is exactly one past the checkpoint's `accepted_head`;
/// 4. the checkpoint's `registry_digest` is set and equals the digest of a block set this process
///    remembers for exactly that checkpoint head (the set the last reconcile registered).
///
/// Every other case (first registration, resumable registration, a lagging, missing or foreign
/// checkpoint, a digest cleared by a repair, another process or an older binary having written the
/// Page, a cold process) runs the full reconcile, which also rewrites the digest. The incremental
/// path locks, plans and writes the blocks that are new or changed against the remembered set and
/// retires the CIDs that left the document; the result equals the full reconcile of the same input
/// because the guard proves the registry holds exactly the remembered (CID, content hash) set.
///
/// Lock order is the same in both paths: the caller's Page head lock, the checkpoint row, the
/// latest placement rows in document order (a subset of the full path's rows), the retired rows,
/// then shares. Only the set of locked rows is smaller.
pub(crate) async fn reconcile_in_mode(
    tx: &DatabaseTransaction,
    scope_id: Uuid,
    page_id: Uuid,
    accepted_head: i64,
    blocks: &[PreparedBlock],
    allowance: RegistrationAllowance,
    mode: ReconcileMode,
) -> Result<ReconcileOutcome, ApiError> {
    let mut seen = HashSet::with_capacity(blocks.len());
    if accepted_head < 0
        || blocks
            .iter()
            .any(|b| b.physical_cid.is_empty() || b.physical_cid.len() > 256 || !seen.insert(b.physical_cid.as_str()))
    {
        return Err(ApiError::invalid_update("invalid logical block reconciliation input"));
    }
    // The checkpoint lock additionally serializes initial registration for existing Pages.
    execute(
        tx,
        1,
        "INSERT INTO flow_block_checkpoints(scope_id,page_id,accepted_head) VALUES($1,$2,0) ON CONFLICT DO NOTHING",
        vec![scope_id.into(), page_id.into()],
    )
    .await?;
    let checkpoint = query_one(
        tx,
        "SELECT accepted_head,pending_registrations,registry_digest FROM flow_block_checkpoints WHERE scope_id=$1 AND page_id=$2 FOR UPDATE",
        vec![scope_id.into(), page_id.into()],
    )
    .await?
    .ok_or_else(|| ApiError::invalid_update("missing block checkpoint"))?;
    let checkpoint_head = checkpoint.try_get::<i64>("", "accepted_head")?;
    if checkpoint_head > accepted_head {
        return Err(ApiError::invalid_update("stale block reconciliation head"));
    }
    let digest = registry_digest(blocks);
    let baseline = if mode == ReconcileMode::Incremental
        && matches!(allowance, RegistrationAllowance::Bounded { .. })
        && checkpoint.try_get::<i32>("", "pending_registrations")? == 0
        && checkpoint_head.checked_add(1) == Some(accepted_head)
    {
        let witness = checkpoint.try_get::<Option<Vec<u8>>>("", "registry_digest")?;
        #[cfg(test)]
        let trust_stale = mutation::current() == Some(mutation::Mutation::TrustStaleCheckpoint);
        #[cfg(not(test))]
        let trust_stale = false;
        with_baselines(|cache| {
            if trust_stale {
                cache
                    .pages
                    .get(&page_id)
                    .and_then(|(_, entries)| entries.first())
                    .map(|entry| Arc::clone(&entry.blocks))
            } else {
                witness.and_then(|witness| cache.find(page_id, checkpoint_head, &witness))
            }
        })
    } else {
        None
    };
    let delta = baseline.as_deref().map(|baseline| diff(baseline, blocks));
    let work = delta
        .as_ref()
        .map_or_else(|| blocks.iter().collect::<Vec<_>>(), |delta| delta.touched.clone());
    // Locks exactly the rows the former per-block query locked: the latest placement of every CID.
    // Every writer of these rows holds the Page head lock first, so locking chunk by chunk cannot
    // deadlock and the set of locked rows is the same as one statement over all CIDs.
    let mut latest_by_cid = HashMap::with_capacity(work.len());
    for chunk in work.chunks(REGISTRY_ROWS_PER_STATEMENT) {
        let cids = chunk.iter().map(|b| b.physical_cid.clone()).collect::<Vec<_>>();
        let rows = query_all(
            tx,
            chunk.len(),
            "SELECT p.id,p.physical_cid,p.logical_id,p.generation,p.state,p.content_hash,f.first_logical_id AS fence_first_logical_id,f.ever_retired AS fence_ever_retired FROM flow_block_placements p LEFT JOIN flow_block_alias_fences f ON f.scope_id=p.scope_id AND f.page_id=p.page_id AND f.physical_cid=p.physical_cid WHERE p.id IN (SELECT DISTINCT ON (h.physical_cid) h.id FROM flow_block_placements h WHERE h.scope_id=$1 AND h.page_id=$2 AND h.physical_cid=ANY($3::text[]) ORDER BY h.physical_cid,h.alias_order DESC) ORDER BY p.id FOR UPDATE OF p",
            vec![scope_id.into(), page_id.into(), cids.into()],
        )
        .await?;
        for row in rows {
            let row = LatestPlacement::from_query_result(&row, "")?;
            if latest_by_cid.insert(row.physical_cid.clone(), row).is_some() {
                return Err(ApiError::internal());
            }
        }
    }
    let mut suspend = Vec::new();
    let mut active_plan = WritePlan::default();
    let mut pending = Vec::new();
    for block in work.iter().copied() {
        let old = latest_by_cid.get(&block.physical_cid);
        if old.is_some_and(|old| old.state == "active") {
            plan_block(&mut active_plan, &mut suspend, block, old)?;
        } else {
            pending.push(block);
        }
    }
    flush_plan(tx, scope_id, page_id, &active_plan).await?;
    // Pending blocks are registered in document order, one chunk group at a time. Registering a
    // CID at a later head gives the same identity decision as if it had first appeared there: the
    // decision depends only on the latest placement of the CID, which no writer changes while the
    // CID is pending.
    let mut registered = 0_usize;
    let mut last_group = Duration::ZERO;
    while registered < pending.len() {
        let take = match allowance {
            RegistrationAllowance::Complete => REGISTRY_ROWS_PER_STATEMENT,
            RegistrationAllowance::Bounded {
                deadline,
                max_rows,
                min_rows,
            } => {
                let remaining = max_rows.saturating_sub(registered);
                let forced = registered < min_rows;
                if remaining == 0
                    || (!forced && Instant::now().checked_add(last_group).is_none_or(|end| end > deadline))
                {
                    break;
                }
                remaining.min(REGISTRY_ROWS_PER_STATEMENT)
            }
        };
        let started = Instant::now();
        let mut plan = WritePlan::default();
        for block in pending.iter().skip(registered).take(take) {
            plan_block(&mut plan, &mut suspend, block, latest_by_cid.get(&block.physical_cid))?;
        }
        flush_plan(tx, scope_id, page_id, &plan).await?;
        registered = registered.saturating_add(take).min(pending.len());
        last_group = started.elapsed();
    }
    let pending_left = pending.len().saturating_sub(registered);
    // Placements of CIDs no longer present are found by a keyset scan over the Page's active
    // placements (unlocked: the head and checkpoint locks are held) and retired in chunks. The
    // retirement takes the row locks of exactly the rows it changes. The incremental path retires
    // the CIDs that left the baseline directly.
    let mut removed_ids = Vec::new();
    if let Some(delta) = &delta {
        for chunk in delta.removed.chunks(REGISTRY_ROWS_PER_STATEMENT) {
            let removed = query_all(
                tx,
                chunk.len(),
                "UPDATE flow_block_placements SET state='deleted',retired_at=now() WHERE scope_id=$1 AND page_id=$2 AND physical_cid=ANY($3::text[]) AND state='active' RETURNING logical_id",
                vec![scope_id.into(), page_id.into(), chunk.to_vec().into()],
            )
            .await?;
            for row in removed {
                removed_ids.push(row.try_get::<Uuid>("", "logical_id")?);
            }
        }
    } else {
        let present = blocks
            .iter()
            .map(|block| block.physical_cid.as_str())
            .collect::<HashSet<_>>();
        let mut absent = Vec::new();
        let mut cursor = String::new();
        let scan_limit = i64::try_from(REGISTRY_ROWS_PER_STATEMENT).map_err(|_| ApiError::internal())?;
        loop {
            let rows = query_all(
                tx,
                REGISTRY_ROWS_PER_STATEMENT,
                "SELECT id,physical_cid FROM flow_block_placements WHERE scope_id=$1 AND page_id=$2 AND state='active' AND physical_cid>$3 ORDER BY physical_cid LIMIT $4",
                vec![scope_id.into(), page_id.into(), cursor.clone().into(), scan_limit.into()],
            )
            .await?;
            let round = rows.len();
            for row in rows {
                cursor = row.try_get::<String>("", "physical_cid")?;
                if !present.contains(cursor.as_str()) {
                    absent.push(row.try_get::<Uuid>("", "id")?);
                }
            }
            if round < REGISTRY_ROWS_PER_STATEMENT {
                break;
            }
        }
        for range in chunk_ranges(absent.len()) {
            let removed = query_all(
                tx,
                range.len(),
                "UPDATE flow_block_placements SET state='deleted',retired_at=now() WHERE id=ANY($1::uuid[]) AND state='active' RETURNING logical_id",
                vec![column(&absent, &range)?.into()],
            )
            .await?;
            for row in removed {
                removed_ids.push(row.try_get::<Uuid>("", "logical_id")?);
            }
        }
    }
    for range in chunk_ranges(removed_ids.len()) {
        execute(
            tx,
            range.len(),
            "UPDATE flow_logical_blocks SET state='deleted' WHERE id=ANY($1::uuid[])",
            vec![column(&removed_ids, &range)?.into()],
        )
        .await?;
    }
    suspend.extend(removed_ids);
    execute(
        tx,
        1,
        "UPDATE flow_block_checkpoints SET accepted_head=$3,pending_registrations=$4,registry_digest=$5 WHERE scope_id=$1 AND page_id=$2",
        vec![
            scope_id.into(),
            page_id.into(),
            accepted_head.into(),
            i32::try_from(pending_left).map_err(|_| ApiError::internal())?.into(),
            (pending_left == 0).then(|| digest.to_vec()).into(),
        ],
    )
    .await?;
    suspend.sort_unstable();
    suspend.dedup();
    if !suspend.is_empty() {
        for chunk in suspend.chunks(REGISTRY_ROWS_PER_STATEMENT) {
            execute(tx, chunk.len(),
                "UPDATE flow_block_shares SET state='suspended',auth_revision=auth_revision+1 WHERE state='active' AND id IN (SELECT share_id FROM flow_block_share_members WHERE logical_id=ANY($1::uuid[]))",
                vec![chunk.to_vec().into()]).await?;
        }
    }
    // Project only fixed member relationships and relative ordering, never absolute source position.
    let shares = query_all(tx, 1,
        "SELECT id,projection_hash FROM flow_block_shares WHERE source_scope_id=$1 AND source_page_id=$2 AND state<>'revoked' ORDER BY id FOR UPDATE",
        vec![scope_id.into(),page_id.into()]).await?;
    // Built only when the Page has shares: it is the one remaining O(page) structure of this phase.
    let by_cid = if shares.is_empty() {
        HashMap::new()
    } else {
        blocks
            .iter()
            .map(|block| (block.physical_cid.as_str(), block))
            .collect::<HashMap<_, _>>()
    };
    for share in shares {
        let share_id = share.try_get::<Uuid>("", "id")?;
        let rows = query_all(tx, 1,
            "SELECT m.member_id,p.physical_cid FROM flow_block_share_members m JOIN flow_block_placements p ON p.logical_id=m.logical_id AND p.state='active' WHERE m.share_id=$1 AND p.scope_id=$2 AND p.page_id=$3",
            vec![share_id.into(),scope_id.into(),page_id.into()]).await?;
        let mut members = Vec::new();
        for row in rows {
            members.push((
                row.try_get::<Uuid>("", "member_id")?,
                row.try_get::<String>("", "physical_cid")?,
            ));
        }
        for (member, cid) in &members {
            if let Some(asset) = by_cid.get(cid.as_str()).and_then(|block| block.asset_id) {
                execute(tx, 1,"INSERT INTO flow_block_share_asset_aliases(share_id,member_id,alias_id,source_scope_id,source_page_id,asset_id) VALUES($1,$2,$3,$4,$5,$6) ON CONFLICT(share_id,member_id,asset_id) DO NOTHING",vec![share_id.into(),(*member).into(),Uuid::new_v4().into(),scope_id.into(),page_id.into(),asset.into()]).await?;
            }
        }
        let hash = visible_graph_hash(blocks, &members)?;
        if share.try_get::<Vec<u8>>("", "projection_hash")? != hash {
            execute(
                tx,
                1,
                "UPDATE flow_block_shares SET projection_hash=$2,content_revision=content_revision+1 WHERE id=$1",
                vec![share_id.into(), hash.to_vec().into()],
            )
            .await?;
        }
    }
    if pending_left == 0 {
        // Remembered before the caller commits. If the transaction rolls back the checkpoint keeps
        // the older head and digest, and the guard refuses this entry.
        let remembered = BaselineEntry {
            head: accepted_head,
            digest,
            blocks: Arc::new(
                blocks
                    .iter()
                    .map(|block| (block.physical_cid.clone(), block.content_hash))
                    .collect(),
            ),
        };
        with_baselines(|cache| cache.store(page_id, remembered));
    } else {
        with_baselines(|cache| cache.forget(page_id));
    }
    Ok(ReconcileOutcome {
        #[cfg(test)]
        suspended: suspend,
        pending: pending_left,
        #[cfg(test)]
        incremental: delta.is_some(),
    })
}

/// Accepted-write hook, inactive until the complete registry-capable process barrier is raised.
/// The process barrier is separate from client editor model5.
pub(crate) async fn reconcile_accepted(
    tx: &DatabaseTransaction,
    object: Uuid,
    seq: i64,
    blocks: &[PreparedBlock],
    allowance: RegistrationAllowance,
) -> Result<(), ApiError> {
    let identity = super::scope::identity(tx, object).await?;
    if identity.object_type != "page" {
        return Ok(());
    }
    reconcile_in_mode(
        tx,
        identity.scope_id,
        object,
        seq,
        blocks,
        allowance,
        ReconcileMode::Incremental,
    )
    .await?;
    Ok(())
}

/// Only the server-generated initializer of a tree-prose Page into the editor doc may carry an
/// existing block identity to its new canonical Map. Raw client aliases never enter this transition.
pub(crate) async fn carry_tree_block_identities(
    tx: &DatabaseTransaction,
    page: Uuid,
    blocks: &[PreparedBlock],
) -> Result<(), ApiError> {
    let scope = super::scope::identity(tx, page).await?.scope_id;
    let mut moved_any = false;
    for block in blocks {
        let Some(alias) = &block.tree_block_alias else {
            continue;
        };
        if alias == &block.physical_cid {
            continue;
        }
        let old=tx.query_one(Statement::from_sql_and_values(DbBackend::Postgres,"SELECT id,logical_id,generation FROM flow_block_placements WHERE scope_id=$1 AND page_id=$2 AND physical_cid=$3 AND state='active' FOR UPDATE",vec![scope.into(),page.into(),alias.clone().into()])).await?;
        let Some(old) = old else {
            continue;
        };
        if tx
            .query_one(Statement::from_sql_and_values(
                DbBackend::Postgres,
                "SELECT physical_cid FROM flow_block_alias_fences WHERE scope_id=$1 AND page_id=$2 AND physical_cid=$3",
                vec![scope.into(), page.into(), block.physical_cid.clone().into()],
            ))
            .await?
            .is_some()
        {
            return Err(ApiError::policy_rejected("editor carrier identity already exists"));
        }
        let logical = old.try_get::<Uuid>("", "logical_id")?;
        let generation = old
            .try_get::<i64>("", "generation")?
            .checked_add(1)
            .ok_or_else(ApiError::internal)?;
        tx.execute(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "UPDATE flow_block_placements SET state='moved',retired_at=now() WHERE id=$1",
            vec![old.try_get::<Uuid>("", "id")?.into()],
        ))
        .await?;
        tx.execute(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "INSERT INTO flow_block_alias_fences(scope_id,page_id,physical_cid,first_logical_id) VALUES($1,$2,$3,$4)",
            vec![
                scope.into(),
                page.into(),
                block.physical_cid.clone().into(),
                logical.into(),
            ],
        ))
        .await?;
        tx.execute(Statement::from_sql_and_values(DbBackend::Postgres,"INSERT INTO flow_block_placements(id,logical_id,scope_id,page_id,physical_cid,generation,state,content_hash) VALUES($1,$2,$3,$4,$5,$6,'active',$7)",vec![Uuid::new_v4().into(),logical.into(),scope.into(),page.into(),block.physical_cid.clone().into(),generation.into(),block.content_hash.to_vec().into()])).await?;
        moved_any = true;
    }
    if moved_any {
        // Placements changed outside `reconcile`: the next reconcile of the Page runs in full.
        tx.execute(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "UPDATE flow_block_checkpoints SET registry_digest=NULL WHERE scope_id=$1 AND page_id=$2",
            vec![scope.into(), page.into()],
        ))
        .await?;
    }
    Ok(())
}

/// Page fixtures shared by the registry and anchor tests.
#[cfg(test)]
pub(crate) mod fixtures {
    use super::PreparedBlock;
    use crate::routes::context::tenant_fixture::exec;
    use sea_orm::{DatabaseConnection, TransactionTrait};
    use uuid::Uuid;

    /// A personal scope with one navigator and one Page. Returns `(actor, scope, page)`.
    pub async fn page(db: &DatabaseConnection) -> (Uuid, Uuid, Uuid) {
        let actor = Uuid::new_v4();
        let scope = Uuid::new_v4();
        let page = Uuid::new_v4();
        exec(db,"INSERT INTO users(id,email,password_hash,name,role,is_active) VALUES($1,$2,'!','Registry fixture','user',true)",vec![actor.into(),format!("{actor}@registry.test").into()]).await;
        exec(
            db,
            "INSERT INTO flow_scopes(id,kind,owner_user_id) VALUES($1,'personal',$2)",
            vec![scope.into(), actor.into()],
        )
        .await;
        exec(
            db,
            "INSERT INTO flow_scope_settings(scope_id) VALUES($1)",
            vec![scope.into()],
        )
        .await;
        let tx = db.begin().await.unwrap();
        let navigator = Uuid::new_v4();
        crate::flow::scope::insert_object_with_identity(
            &tx,
            scope,
            actor,
            "navigator",
            None,
            "",
            None,
            navigator,
            Uuid::new_v4(),
        )
        .await
        .unwrap();
        crate::flow::scope::insert_object_with_identity(
            &tx,
            scope,
            actor,
            "page",
            Some(navigator),
            "Registry fixture",
            None,
            page,
            Uuid::new_v4(),
        )
        .await
        .unwrap();
        tx.commit().await.unwrap();
        (actor, scope, page)
    }

    /// Block `index` at content `version`; the hash is unique per pair.
    pub fn block(index: usize, version: u8) -> PreparedBlock {
        let mut content_hash = [version; 32];
        content_hash[..8].copy_from_slice(&u64::try_from(index).unwrap().to_be_bytes());
        PreparedBlock {
            physical_cid: format!("cid:{index}:Map"),
            parent_physical_cid: None,
            order_key: format!("{index:08}"),
            asset_id: None,
            tree_block_alias: None,
            content_hash,
        }
    }

    pub fn blocks(count: usize, version: u8) -> Vec<PreparedBlock> {
        (0..count).map(|index| block(index, version)).collect()
    }
}

#[cfg(test)]
mod incremental_tests;

#[cfg(test)]
#[allow(clippy::indexing_slicing, clippy::print_stderr)]
mod tests {
    use super::*;
    use crate::{routes::context::tenant_fixture::exec, scratch_or_skip};
    use sea_orm::TransactionTrait;

    async fn logical(tx: &DatabaseTransaction, page: Uuid) -> (Uuid, i64) {
        let row = tx
            .query_one(Statement::from_sql_and_values(
                DbBackend::Postgres,
                "SELECT logical_id,generation FROM flow_block_placements WHERE page_id=$1 AND state='active'",
                vec![page.into()],
            ))
            .await
            .unwrap()
            .unwrap();
        (
            row.try_get("", "logical_id").unwrap(),
            row.try_get("", "generation").unwrap(),
        )
    }

    #[test]
    fn actual_semantic_derived_indent_and_private_move_do_not_change_single_projection() {
        let mut engine = collab_core::LoroCollabEngine::new_empty(99220);
        engine.initialize_page_editor().unwrap();
        engine.initialize_page_structure().unwrap();
        let root = engine.semantic_snapshot().unwrap().nodes.keys().next().unwrap().clone();
        for (key, value) in [("block_type", "orderedItem"), ("order", "7")] {
            engine
                .apply_content_operation(&collab_core::Operation::SetProperty {
                    id: root.clone(),
                    key: key.into(),
                    value: value.into(),
                })
                .unwrap();
        }
        engine
            .apply_content_operation(&collab_core::Operation::CreateNode {
                id: "private-parent".into(),
                parent: None,
                index: 1,
                kind: collab_core::NodeKind::Block,
            })
            .unwrap();
        let before = engine.semantic_snapshot().unwrap();
        let before_blocks = prepare(&before).unwrap();
        let selected = vec![(Uuid::from_u128(1), root.to_string())];
        let digest = visible_graph_hash(&before_blocks, &selected).unwrap();
        engine
            .apply_content_operation(&collab_core::Operation::MoveNode {
                id: root.clone(),
                new_parent: Some("private-parent".into()),
                index: 0,
            })
            .unwrap();
        let after = engine.semantic_snapshot().unwrap();
        assert_ne!(
            before.nodes.get(&root).unwrap().properties.get("indent"),
            after.nodes.get(&root).unwrap().properties.get("indent")
        );
        assert_eq!(after.nodes.get(&root).unwrap().properties.get("order").unwrap(), "7");
        let after_blocks = prepare(&after).unwrap();
        assert_eq!(digest, visible_graph_hash(&after_blocks, &selected).unwrap());
        assert_eq!(
            before_blocks
                .iter()
                .find(|block| block.physical_cid == root.as_ref())
                .unwrap()
                .content_hash,
            after_blocks
                .iter()
                .find(|block| block.physical_cid == root.as_ref())
                .unwrap()
                .content_hash
        );
    }

    #[test]
    fn projection_revision_ignores_private_position_but_tracks_selected_graph() {
        let a = Uuid::from_u128(1);
        let b = Uuid::from_u128(2);
        let mut blocks = vec![
            PreparedBlock {
                physical_cid: "a".into(),
                parent_physical_cid: Some("hidden".into()),
                order_key: "10".into(),
                asset_id: None,
                tree_block_alias: None,
                content_hash: [1; 32],
            },
            PreparedBlock {
                physical_cid: "b".into(),
                parent_physical_cid: None,
                order_key: "20".into(),
                asset_id: None,
                tree_block_alias: None,
                content_hash: [2; 32],
            },
        ];
        let single = vec![(a, "a".into())];
        let initial = visible_graph_hash(&blocks, &single).unwrap();
        blocks
            .get_mut(0)
            .expect("first fixture block exists")
            .parent_physical_cid = Some("another private parent".into());
        blocks.get_mut(0).expect("first fixture block exists").order_key = "11".into();
        assert_eq!(initial, visible_graph_hash(&blocks, &single).unwrap());
        let both = vec![(a, "a".into()), (b, "b".into())];
        let original = visible_graph_hash(&blocks, &both).unwrap();
        blocks
            .get_mut(1)
            .expect("second fixture block exists")
            .parent_physical_cid = Some("a".into());
        assert_ne!(original, visible_graph_hash(&blocks, &both).unwrap());
        blocks
            .get_mut(1)
            .expect("second fixture block exists")
            .parent_physical_cid = None;
        blocks.get_mut(1).expect("second fixture block exists").order_key = "00".into();
        assert_ne!(original, visible_graph_hash(&blocks, &both).unwrap());
    }

    #[test]
    fn prepare_ignores_private_metadata_and_rejects_unsafe_inline() {
        let mut snapshot = collab_core::SemanticSnapshot::default();
        let id: collab_core::NodeId = "cid:fixture:Map".into();
        snapshot.nodes.insert(
            id.clone(),
            collab_core::SemanticNode {
                parent: None,
                order_key: "0".into(),
                kind: collab_core::NodeKind::Block,
                text: "中文😀".into(),
                properties: std::collections::BTreeMap::new(),
                deleted: false,
            },
        );
        let original = prepare(&snapshot).unwrap();
        let node = snapshot.nodes.get_mut(&id).unwrap();
        node.parent = Some("private-parent".into());
        node.order_key = "unrelated reorder".into();
        node.properties.insert("private_comments".into(), "secret".into());
        node.properties.insert("page_title".into(), "secret title".into());
        assert_eq!(
            prepare(&snapshot)
                .unwrap()
                .first()
                .expect("prepared block exists")
                .content_hash,
            original.first().expect("original block exists").content_hash
        );
        snapshot.nodes.get_mut(&id).unwrap().text.push('!');
        assert_ne!(
            prepare(&snapshot)
                .unwrap()
                .first()
                .expect("prepared block exists")
                .content_hash,
            original.first().expect("original block exists").content_hash
        );
        snapshot.nodes.get_mut(&id).unwrap().properties.insert(
            "inline_spans".into(),
            r#"[{"start":0,"end":3,"mark":"link","href":"javascript:bad"}]"#.into(),
        );
        assert!(prepare(&snapshot).is_err());
    }

    #[tokio::test]
    async fn registry_reconcile_restore_moved_alias_and_rollback() {
        let scratch = scratch_or_skip!(&format!("block_registry_{}", Uuid::new_v4().simple()));
        let db = &scratch.db;
        let actor = Uuid::new_v4();
        let scope = Uuid::new_v4();
        let page = Uuid::new_v4();
        exec(db,"INSERT INTO users(id,email,password_hash,name,role,is_active) VALUES($1,$2,'!','Registry fixture','user',true)",vec![actor.into(),format!("{actor}@registry.test").into()]).await;
        exec(
            db,
            "INSERT INTO flow_scopes(id,kind,owner_user_id) VALUES($1,'personal',$2)",
            vec![scope.into(), actor.into()],
        )
        .await;
        exec(
            db,
            "INSERT INTO flow_scope_settings(scope_id) VALUES($1)",
            vec![scope.into()],
        )
        .await;
        let tx = db.begin().await.unwrap();
        let navigator = Uuid::new_v4();
        super::super::scope::insert_object_with_identity(
            &tx,
            scope,
            actor,
            "navigator",
            None,
            "",
            None,
            navigator,
            Uuid::new_v4(),
        )
        .await
        .unwrap();
        super::super::scope::insert_object_with_identity(
            &tx,
            scope,
            actor,
            "page",
            Some(navigator),
            "Registry fixture",
            None,
            page,
            Uuid::new_v4(),
        )
        .await
        .unwrap();
        let blocks = [PreparedBlock {
            physical_cid: "cid:fixture:Map".into(),
            parent_physical_cid: None,
            order_key: "0".into(),
            asset_id: None,
            tree_block_alias: None,
            content_hash: [7; 32],
        }];
        complete(&tx, scope, page, 0, &blocks).await.unwrap();
        let original = logical(&tx, page).await;
        let share = Uuid::new_v4();
        let member = Uuid::new_v4();
        tx.execute(Statement::from_sql_and_values(DbBackend::Postgres,"INSERT INTO flow_block_shares(id,source_scope_id,source_page_id,created_by,publisher_user_id,state,source_root_cid,selection,root_logical_id) VALUES($1,$2,$3,$4,$4,'active','cid:fixture:Map','block',$5)",vec![share.into(),scope.into(),page.into(),actor.into(),original.0.into()])).await.unwrap();
        tx.execute(Statement::from_sql_and_values(DbBackend::Postgres,"INSERT INTO flow_block_share_members(share_id,member_id,logical_id,member_order,content_hash) VALUES($1,$2,$3,0,$4)",vec![share.into(),member.into(),original.0.into(),vec![7_u8;32].into()])).await.unwrap();
        let initial_hash = visible_graph_hash(&blocks, &[(member, blocks[0].physical_cid.clone())]).unwrap();
        tx.execute(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "UPDATE flow_block_shares SET projection_hash=$2 WHERE id=$1",
            vec![share.into(), initial_hash.to_vec().into()],
        ))
        .await
        .unwrap();
        complete(&tx, scope, page, 2, &blocks).await.unwrap();
        let revision = tx
            .query_one(Statement::from_sql_and_values(
                DbBackend::Postgres,
                "SELECT content_revision FROM flow_block_shares WHERE id=$1",
                vec![share.into()],
            ))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(revision.try_get::<i64>("", "content_revision").unwrap(), 1);
        let edited = [PreparedBlock {
            physical_cid: blocks[0].physical_cid.clone(),
            parent_physical_cid: None,
            order_key: "0".into(),
            asset_id: None,
            tree_block_alias: None,
            content_hash: [8; 32],
        }];
        complete(&tx, scope, page, 3, &edited).await.unwrap();
        let revision = tx
            .query_one(Statement::from_sql_and_values(
                DbBackend::Postgres,
                "SELECT content_revision FROM flow_block_shares WHERE id=$1",
                vec![share.into()],
            ))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(revision.try_get::<i64>("", "content_revision").unwrap(), 2);
        complete(&tx, scope, page, 4, &[]).await.unwrap();
        let suspended = complete(&tx, scope, page, 5, &blocks).await.unwrap().suspended;
        let status = tx
            .query_one(Statement::from_sql_and_values(
                DbBackend::Postgres,
                "SELECT state FROM flow_block_shares WHERE id=$1",
                vec![share.into()],
            ))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(status.try_get::<String>("", "state").unwrap(), "suspended");
        let restored = logical(&tx, page).await;
        assert_eq!(original.0, restored.0);
        assert_eq!(restored.1, original.1 + 1);
        assert_eq!(suspended, vec![original.0]);
        tx.execute(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "UPDATE flow_block_placements SET state='moved',retired_at=now() WHERE page_id=$1 AND state='active'",
            vec![page.into()],
        ))
        .await
        .unwrap();
        complete(&tx, scope, page, 6, &blocks).await.unwrap();
        let after_move_restore = logical(&tx, page).await;
        assert_ne!(after_move_restore.0, original.0);
        assert!(complete(&tx, scope, page, 5, &blocks).await.is_err());
        tx.rollback().await.unwrap();
        let row = db
            .query_one(Statement::from_string(
                DbBackend::Postgres,
                "SELECT count(*) AS n FROM flow_logical_blocks",
            ))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(row.try_get::<i64>("", "n").unwrap(), 0);
        scratch.drop_self().await;
    }

    use super::fixtures::{block, blocks, page};
    use sea_orm::DatabaseConnection;
    use std::collections::{BTreeSet, HashMap};
    use std::time::{Duration, Instant};

    /// Verbatim copy of the per-block implementation that `reconcile` replaced.
    #[derive(FromQueryResult)]
    struct ReferencePlacement {
        id: Uuid,
        logical_id: Uuid,
        generation: i64,
        state: String,
    }
    async fn reconcile_reference(
        tx: &DatabaseTransaction,
        scope_id: Uuid,
        page_id: Uuid,
        accepted_head: i64,
        blocks: &[PreparedBlock],
    ) -> Result<Vec<Uuid>, ApiError> {
        let mut seen = BTreeSet::new();
        if accepted_head < 0
            || blocks.iter().any(|b| {
                b.physical_cid.is_empty() || b.physical_cid.len() > 256 || !seen.insert(b.physical_cid.as_str())
            })
        {
            return Err(ApiError::invalid_update("invalid logical block reconciliation input"));
        }
        // The checkpoint lock additionally serializes initial registration for existing Pages.
        tx.execute(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "INSERT INTO flow_block_checkpoints(scope_id,page_id,accepted_head) VALUES($1,$2,0) ON CONFLICT DO NOTHING",
            vec![scope_id.into(), page_id.into()],
        ))
        .await?;
        let checkpoint = tx
            .query_one(Statement::from_sql_and_values(
                DbBackend::Postgres,
                "SELECT accepted_head FROM flow_block_checkpoints WHERE scope_id=$1 AND page_id=$2 FOR UPDATE",
                vec![scope_id.into(), page_id.into()],
            ))
            .await?
            .ok_or_else(|| ApiError::invalid_update("missing block checkpoint"))?;
        if checkpoint.try_get::<i64>("", "accepted_head")? > accepted_head {
            return Err(ApiError::invalid_update("stale block reconciliation head"));
        }
        let mut suspend = Vec::new();
        for block in blocks {
            let old = ReferencePlacement::find_by_statement(Statement::from_sql_and_values(DbBackend::Postgres,
                "SELECT id,logical_id,generation,state FROM flow_block_placements WHERE scope_id=$1 AND page_id=$2 AND physical_cid=$3 ORDER BY alias_order DESC LIMIT 1 FOR UPDATE",
                vec![scope_id.into(),page_id.into(),block.physical_cid.clone().into()])).one(tx).await?;
            let alias = old.as_ref().map(|old| Alias {
                logical_id: old.logical_id,
                generation: old.generation.unsigned_abs(),
                state: match old.state.as_str() {
                    "active" => AliasState::Active,
                    "deleted" => AliasState::Deleted,
                    _ => AliasState::Moved,
                },
            });
            let decision = reconcile_alias(alias, Uuid::new_v4())
                .map_err(|_| ApiError::invalid_update("logical placement generation conflict"))?;
            let generation = i64::try_from(decision.generation)
                .map_err(|_| ApiError::invalid_update("logical placement generation exhausted"))?;
            if decision.fresh_identity {
                tx.execute(Statement::from_sql_and_values(
                    DbBackend::Postgres,
                    "INSERT INTO flow_logical_blocks(id,state) VALUES($1,'active')",
                    vec![decision.logical_id.into()],
                ))
                .await?;
            }
            tx.execute(Statement::from_sql_and_values(DbBackend::Postgres,"INSERT INTO flow_block_alias_fences(scope_id,page_id,physical_cid,first_logical_id,ever_retired) VALUES($1,$2,$3,$4,$5) ON CONFLICT(scope_id,page_id,physical_cid) DO UPDATE SET ever_retired=flow_block_alias_fences.ever_retired OR EXCLUDED.ever_retired OR flow_block_alias_fences.first_logical_id<>EXCLUDED.first_logical_id",vec![scope_id.into(),page_id.into(),block.physical_cid.clone().into(),decision.logical_id.into(),alias.is_some_and(|alias|alias.state!=AliasState::Active).into()])).await?;
            if let Some(old) = old.filter(|row| row.state == "active") {
                tx.execute(Statement::from_sql_and_values(
                    DbBackend::Postgres,
                    "UPDATE flow_block_placements SET content_hash=$2 WHERE id=$1",
                    vec![old.id.into(), block.content_hash.to_vec().into()],
                ))
                .await?;
            } else {
                tx.execute(Statement::from_sql_and_values(
                    DbBackend::Postgres,
                    "UPDATE flow_logical_blocks SET state='active' WHERE id=$1",
                    vec![decision.logical_id.into()],
                ))
                .await?;
                tx.execute(Statement::from_sql_and_values(DbBackend::Postgres,
                    "INSERT INTO flow_block_placements(id,logical_id,scope_id,page_id,physical_cid,generation,state,content_hash) VALUES($1,$2,$3,$4,$5,$6,'active',$7)",
                    vec![Uuid::new_v4().into(),decision.logical_id.into(),scope_id.into(),page_id.into(),block.physical_cid.clone().into(),generation.into(),block.content_hash.to_vec().into()])).await?;
            }
            if decision.suspend_shares {
                suspend.push(decision.logical_id);
            }
        }
        let cids = blocks.iter().map(|b| b.physical_cid.clone()).collect::<Vec<_>>();
        let removed = tx.query_all(Statement::from_sql_and_values(DbBackend::Postgres,
            "UPDATE flow_block_placements SET state='deleted',retired_at=now() WHERE scope_id=$1 AND page_id=$2 AND state='active' AND NOT(physical_cid=ANY($3::text[])) RETURNING logical_id",
            vec![scope_id.into(),page_id.into(),cids.into()])).await?;
        for row in removed {
            let id = row.try_get::<Uuid>("", "logical_id")?;
            tx.execute(Statement::from_sql_and_values(
                DbBackend::Postgres,
                "UPDATE flow_logical_blocks SET state='deleted' WHERE id=$1",
                vec![id.into()],
            ))
            .await?;
            suspend.push(id);
        }
        tx.execute(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "UPDATE flow_block_checkpoints SET accepted_head=$3 WHERE scope_id=$1 AND page_id=$2",
            vec![scope_id.into(), page_id.into(), accepted_head.into()],
        ))
        .await?;
        suspend.sort_unstable();
        suspend.dedup();
        if !suspend.is_empty() {
            tx.execute(Statement::from_sql_and_values(DbBackend::Postgres,
                "UPDATE flow_block_shares SET state='suspended',auth_revision=auth_revision+1 WHERE state='active' AND id IN (SELECT share_id FROM flow_block_share_members WHERE logical_id=ANY($1::uuid[]))",
                vec![suspend.clone().into()])).await?;
        }
        // Project only fixed member relationships and relative ordering, never absolute source position.
        let shares=tx.query_all(Statement::from_sql_and_values(DbBackend::Postgres,
            "SELECT id,projection_hash FROM flow_block_shares WHERE source_scope_id=$1 AND source_page_id=$2 AND state<>'revoked' ORDER BY id FOR UPDATE",
            vec![scope_id.into(),page_id.into()])).await?;
        for share in shares {
            let share_id = share.try_get::<Uuid>("", "id")?;
            let rows=tx.query_all(Statement::from_sql_and_values(DbBackend::Postgres,
                "SELECT m.member_id,p.physical_cid FROM flow_block_share_members m JOIN flow_block_placements p ON p.logical_id=m.logical_id AND p.state='active' WHERE m.share_id=$1 AND p.scope_id=$2 AND p.page_id=$3",
                vec![share_id.into(),scope_id.into(),page_id.into()])).await?;
            let mut members = Vec::new();
            for row in rows {
                members.push((
                    row.try_get::<Uuid>("", "member_id")?,
                    row.try_get::<String>("", "physical_cid")?,
                ));
            }
            for (member, cid) in &members {
                if let Some(asset) = blocks
                    .iter()
                    .find(|block| block.physical_cid == *cid)
                    .and_then(|block| block.asset_id)
                {
                    tx.execute(Statement::from_sql_and_values(DbBackend::Postgres,"INSERT INTO flow_block_share_asset_aliases(share_id,member_id,alias_id,source_scope_id,source_page_id,asset_id) VALUES($1,$2,$3,$4,$5,$6) ON CONFLICT(share_id,member_id,asset_id) DO NOTHING",vec![share_id.into(),(*member).into(),Uuid::new_v4().into(),scope_id.into(),page_id.into(),asset.into()])).await?;
                }
            }
            let hash = visible_graph_hash(blocks, &members)?;
            if share.try_get::<Vec<u8>>("", "projection_hash")? != hash {
                tx.execute(Statement::from_sql_and_values(
                    DbBackend::Postgres,
                    "UPDATE flow_block_shares SET projection_hash=$2,content_revision=content_revision+1 WHERE id=$1",
                    vec![share_id.into(), hash.to_vec().into()],
                ))
                .await?;
            }
        }
        Ok(suspend)
    }

    /// Masks the random Loro peer id of a CID, e.g. `cid:2@123:Map` becomes `cid:2@peer:Map`. Creating
    /// a Page registers its initial editor block under such a CID.
    fn mask_peer(cid: &str) -> String {
        match cid.split_once('@') {
            Some((head, tail)) => format!("{head}@peer{}", tail.find(':').map_or("", |at| &tail[at..])),
            None => cid.to_string(),
        }
    }

    /// Normalised view of everything a reconciliation writes for one Page. Random UUIDs are replaced
    /// by the index of their first appearance, so two Pages that went through the same history compare equal.
    async fn dump(tx: &DatabaseTransaction, page: Uuid) -> (Vec<String>, HashMap<Uuid, String>) {
        let placements = tx
            .query_all(Statement::from_sql_and_values(
                DbBackend::Postgres,
                "SELECT physical_cid,logical_id,generation,state,encode(content_hash,'hex') AS hash,(retired_at IS NOT NULL) AS retired FROM flow_block_placements WHERE page_id=$1 ORDER BY physical_cid COLLATE \"C\",alias_order",
                vec![page.into()],
            ))
            .await
            .unwrap();
        let mut index = HashMap::<Uuid, usize>::new();
        let mut cid_of = HashMap::<Uuid, String>::new();
        let mut lines = Vec::new();
        for row in &placements {
            let logical: Uuid = row.try_get("", "logical_id").unwrap();
            let next = index.len();
            let id = *index.entry(logical).or_insert(next);
            let cid = mask_peer(&row.try_get::<String>("", "physical_cid").unwrap());
            cid_of.insert(logical, cid.clone());
            lines.push(format!(
                "placement {cid} logical#{id} gen={} {} {} retired={}",
                row.try_get::<i64>("", "generation").unwrap(),
                row.try_get::<String>("", "state").unwrap(),
                row.try_get::<String>("", "hash").unwrap(),
                row.try_get::<bool>("", "retired").unwrap(),
            ));
        }
        for row in tx
            .query_all(Statement::from_sql_and_values(
                DbBackend::Postgres,
                "SELECT physical_cid,first_logical_id,ever_retired FROM flow_block_alias_fences WHERE page_id=$1 ORDER BY physical_cid COLLATE \"C\"",
                vec![page.into()],
            ))
            .await
            .unwrap()
        {
            let first: Uuid = row.try_get("", "first_logical_id").unwrap();
            lines.push(format!(
                "fence {} first=logical#{:?} retired={}",
                mask_peer(&row.try_get::<String>("", "physical_cid").unwrap()),
                index.get(&first),
                row.try_get::<bool>("", "ever_retired").unwrap(),
            ));
        }
        let mut ordered = index.iter().collect::<Vec<_>>();
        ordered.sort_by_key(|(_, id)| **id);
        for (logical, id) in ordered {
            let state: String = tx
                .query_one(Statement::from_sql_and_values(
                    DbBackend::Postgres,
                    "SELECT state FROM flow_logical_blocks WHERE id=$1",
                    vec![(*logical).into()],
                ))
                .await
                .unwrap()
                .unwrap()
                .try_get("", "state")
                .unwrap();
            lines.push(format!("logical#{id} {state}"));
        }
        let checkpoint = tx
            .query_one(Statement::from_sql_and_values(
                DbBackend::Postgres,
                "SELECT accepted_head FROM flow_block_checkpoints WHERE page_id=$1",
                vec![page.into()],
            ))
            .await
            .unwrap()
            .unwrap();
        lines.push(format!(
            "checkpoint head={}",
            checkpoint.try_get::<i64>("", "accepted_head").unwrap(),
        ));
        for row in tx
            .query_all(Statement::from_sql_and_values(
                DbBackend::Postgres,
                "SELECT state,auth_revision,content_revision,encode(projection_hash,'hex') AS hash FROM flow_block_shares WHERE source_page_id=$1 ORDER BY source_root_cid",
                vec![page.into()],
            ))
            .await
            .unwrap()
        {
            lines.push(format!(
                "share {} auth={} content={} {}",
                row.try_get::<String>("", "state").unwrap(),
                row.try_get::<i64>("", "auth_revision").unwrap(),
                row.try_get::<i64>("", "content_revision").unwrap(),
                row.try_get::<String>("", "hash").unwrap(),
            ));
        }
        for row in tx
            .query_all(Statement::from_sql_and_values(
                DbBackend::Postgres,
                "SELECT original_block_id,state,origin_generation,logical_id FROM flow_block_comment_anchors WHERE source_page_id=$1 ORDER BY original_block_id",
                vec![page.into()],
            ))
            .await
            .unwrap()
        {
            let logical: Option<Uuid> = row.try_get("", "logical_id").unwrap();
            lines.push(format!(
                "anchor {} {} gen={:?} logical#{:?}",
                row.try_get::<String>("", "original_block_id").unwrap(),
                row.try_get::<String>("", "state").unwrap(),
                row.try_get::<Option<i64>>("", "origin_generation").unwrap(),
                logical.and_then(|id| index.get(&id)),
            ));
        }
        (lines, cid_of)
    }

    /// Suspended logical identities of one reconciliation, as sorted physical CIDs.
    pub(super) fn suspended_cids(suspend: &[Uuid], cid_of: &HashMap<Uuid, String>) -> Vec<String> {
        let mut cids = suspend
            .iter()
            .map(|id| cid_of.get(id).cloned().unwrap_or_else(|| "<unknown>".into()))
            .collect::<Vec<_>>();
        cids.sort();
        cids
    }

    /// Share lines without the revision counter: restoring several members one step at a time
    /// legitimately bumps `content_revision` once per visible projection change.
    fn without_content_revision(lines: &[String]) -> Vec<String> {
        lines
            .iter()
            .map(|line| {
                if line.starts_with("share ") {
                    line.split(' ')
                        .filter(|token| !token.starts_with("content="))
                        .collect::<Vec<_>>()
                        .join(" ")
                } else {
                    line.clone()
                }
            })
            .collect()
    }

    /// Run the new and the reference implementation on two Pages and require identical results.
    /// A third Page registers its pending blocks one row per transaction and must converge to the
    /// same state, modulo the revision counter.
    async fn apply_both(
        tx: &DatabaseTransaction,
        (scope_a, page_a): (Uuid, Uuid),
        (scope_b, page_b): (Uuid, Uuid),
        (scope_c, page_c): (Uuid, Uuid),
        head: i64,
        blocks: &[PreparedBlock],
    ) {
        let new = complete(tx, scope_a, page_a, head, blocks).await.unwrap().suspended;
        let old = reconcile_reference(tx, scope_b, page_b, head, blocks).await.unwrap();
        let (new_lines, new_cids) = dump(tx, page_a).await;
        let (old_lines, old_cids) = dump(tx, page_b).await;
        assert_eq!(new_lines, old_lines, "registry rows diverge at head {head}");
        assert_eq!(
            suspended_cids(&new, &new_cids),
            suspended_cids(&old, &old_cids),
            "suspended identities diverge at head {head}"
        );
        let mut stepwise = Vec::new();
        loop {
            let outcome = bounded(tx, scope_c, page_c, head, blocks, 1).await.unwrap();
            stepwise.extend(outcome.suspended);
            if outcome.pending == 0 {
                break;
            }
        }
        let (step_lines, step_cids) = dump(tx, page_c).await;
        assert_eq!(
            without_content_revision(&step_lines),
            without_content_revision(&old_lines),
            "single-row registration steps diverge at head {head}"
        );
        stepwise.sort_unstable();
        stepwise.dedup();
        assert_eq!(
            suspended_cids(&stepwise, &step_cids),
            suspended_cids(&old, &old_cids),
            "single-row registration steps suspend other identities at head {head}"
        );
    }

    pub(super) async fn add_share(
        tx: &DatabaseTransaction,
        (actor, scope, page): (Uuid, Uuid, Uuid),
        blocks: &[PreparedBlock],
        cid: &str,
    ) {
        let logical: Uuid = tx
            .query_one(Statement::from_sql_and_values(
                DbBackend::Postgres,
                "SELECT logical_id FROM flow_block_placements WHERE page_id=$1 AND physical_cid=$2 AND state='active'",
                vec![page.into(), cid.into()],
            ))
            .await
            .unwrap()
            .unwrap()
            .try_get("", "logical_id")
            .unwrap();
        let share = Uuid::new_v4();
        let member = Uuid::from_u128(77);
        tx.execute(Statement::from_sql_and_values(DbBackend::Postgres,"INSERT INTO flow_block_shares(id,source_scope_id,source_page_id,created_by,publisher_user_id,state,source_root_cid,selection,root_logical_id) VALUES($1,$2,$3,$4,$4,'active',$5,'block',$6)",vec![share.into(),scope.into(),page.into(),actor.into(),cid.into(),logical.into()])).await.unwrap();
        tx.execute(Statement::from_sql_and_values(DbBackend::Postgres,"INSERT INTO flow_block_share_members(share_id,member_id,logical_id,member_order,content_hash) VALUES($1,$2,$3,0,$4)",vec![share.into(),member.into(),logical.into(),vec![7_u8;32].into()])).await.unwrap();
        let hash = visible_graph_hash(blocks, &[(member, cid.to_string())]).unwrap();
        tx.execute(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "UPDATE flow_block_shares SET projection_hash=$2 WHERE id=$1",
            vec![share.into(), hash.to_vec().into()],
        ))
        .await
        .unwrap();
    }

    pub(super) async fn scratch_with_leases(label: &str) -> Option<crate::routes::context::tenant_fixture::Scratch> {
        let scratch = crate::routes::context::tenant_fixture::scratch(label).await?;
        Some(scratch)
    }

    #[tokio::test]
    async fn bulk_reconcile_matches_reference_on_mixed_history() {
        let Some(scratch) = scratch_with_leases(&format!("block_registry_eq_{}", Uuid::new_v4().simple())).await else {
            eprintln!("SKIPPED (no database)");
            return;
        };
        let db = &scratch.db;
        let (actor_a, scope_a, page_a) = page(db).await;
        let (actor_b, scope_b, page_b) = page(db).await;
        let (actor_c, scope_c, page_c) = page(db).await;
        let (a, b, c) = ((scope_a, page_a), (scope_b, page_b), (scope_c, page_c));
        let tx = db.begin().await.unwrap();
        // 1. Three fresh blocks.
        let step1 = vec![block(0, 1), block(1, 1), block(2, 1)];
        apply_both(&tx, a, b, c, 0, &step1).await;
        // 2. Change one block, add two, remove one.
        let step2 = vec![block(0, 1), block(1, 2), block(3, 1), block(4, 1)];
        apply_both(&tx, a, b, c, 1, &step2).await;
        // 3. Restore the removed CID: deleted alias, generation + 1, suspension.
        let step3 = vec![block(0, 1), block(1, 2), block(2, 1), block(3, 1), block(4, 1)];
        apply_both(&tx, a, b, c, 2, &step3).await;
        // 4. A moved placement reconciled again takes a fresh identity.
        for page in [page_a, page_b, page_c] {
            tx.execute(Statement::from_sql_and_values(
                DbBackend::Postgres,
                "UPDATE flow_block_placements SET state='moved',retired_at=now() WHERE page_id=$1 AND physical_cid=$2 AND state='active'",
                vec![page.into(), block(0, 1).physical_cid.into()],
            ))
            .await
            .unwrap();
        }
        apply_both(&tx, a, b, c, 3, &step3).await;
        // 5. Shares with members, then a content change and a member removal.
        for (actor, scope, page) in [
            (actor_a, scope_a, page_a),
            (actor_b, scope_b, page_b),
            (actor_c, scope_c, page_c),
        ] {
            add_share(&tx, (actor, scope, page), &step3, &block(1, 2).physical_cid).await;
            add_share(&tx, (actor, scope, page), &step3, &block(3, 1).physical_cid).await;
        }
        apply_both(&tx, a, b, c, 4, &step3).await;
        let step5 = vec![block(0, 1), block(1, 3), block(2, 1), block(3, 1), block(4, 1)];
        apply_both(&tx, a, b, c, 5, &step5).await;
        let step6 = vec![block(0, 1), block(1, 3), block(2, 1), block(4, 1)];
        apply_both(&tx, a, b, c, 6, &step6).await;
        let (lines, _) = dump(&tx, page_a).await;
        assert!(
            lines.iter().any(|line| line.starts_with("share suspended")),
            "the member removal must suspend its share: {lines:?}"
        );
        // 7. Everything removed, then everything restored.
        apply_both(&tx, a, b, c, 7, &[]).await;
        apply_both(&tx, a, b, c, 8, &step3).await;
        tx.rollback().await.unwrap();
        scratch.drop_self().await;
    }

    /// `reconcile` with every pending block registered in this call.
    async fn complete(
        tx: &DatabaseTransaction,
        scope: Uuid,
        page: Uuid,
        head: i64,
        blocks: &[PreparedBlock],
    ) -> Result<ReconcileOutcome, ApiError> {
        reconcile(tx, scope, page, head, blocks, RegistrationAllowance::Complete).await
    }

    /// `reconcile` registering at most `max_rows` pending blocks, with no deadline.
    async fn bounded(
        tx: &DatabaseTransaction,
        scope: Uuid,
        page: Uuid,
        head: i64,
        blocks: &[PreparedBlock],
        max_rows: usize,
    ) -> Result<ReconcileOutcome, ApiError> {
        let allowance = RegistrationAllowance::Bounded {
            deadline: Instant::now() + Duration::from_hours(1),
            max_rows,
            min_rows: 0,
        };
        reconcile(tx, scope, page, head, blocks, allowance).await
    }

    async fn counted(
        tx: &DatabaseTransaction,
        scope: Uuid,
        page: Uuid,
        head: i64,
        blocks: &[PreparedBlock],
        allowance: RegistrationAllowance,
    ) -> (ReconcileOutcome, Vec<statement_probe::Record>) {
        let (result, records) = statement_probe::count(reconcile(tx, scope, page, head, blocks, allowance)).await;
        (result.unwrap(), records)
    }

    async fn pending_of(db: &impl ConnectionTrait, page: Uuid) -> i32 {
        db.query_one(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT pending_registrations FROM flow_block_checkpoints WHERE page_id=$1",
            vec![page.into()],
        ))
        .await
        .unwrap()
        .unwrap()
        .try_get("", "pending_registrations")
        .unwrap()
    }

    /// Rounds of the removal scan over `active` placements: it ends at the first round below the chunk size.
    fn scan_rounds(active: usize) -> usize {
        active / REGISTRY_ROWS_PER_STATEMENT + 1
    }

    #[tokio::test]
    async fn reconcile_statements_carry_bounded_rows() {
        let Some(scratch) = scratch_with_leases(&format!("block_registry_count_{}", Uuid::new_v4().simple())).await
        else {
            eprintln!("SKIPPED (no database)");
            return;
        };
        let db = &scratch.db;
        let chunk = REGISTRY_ROWS_PER_STATEMENT;
        let sizes = [50, chunk, 2 * chunk + 1, 10_000];
        let mut first = Vec::new();
        let mut steady = Vec::new();
        for size in sizes {
            let (_, scope, page) = page(db).await;
            let tx = db.begin().await.unwrap();
            let initial = blocks(size, 1);
            let (outcome, records) = counted(&tx, scope, page, 0, &initial, RegistrationAllowance::Complete).await;
            assert_eq!(outcome.pending, 0);
            assert!(
                records.iter().all(|record| record.rows <= chunk),
                "size {size}: a first-registration statement carries more than {chunk} rows"
            );
            first.push(records.len());
            let mut changed = initial.clone();
            changed[0] = block(0, 2);
            let (_, records) = counted(&tx, scope, page, 1, &changed, RegistrationAllowance::Complete).await;
            assert!(
                records.iter().all(|record| record.rows <= chunk),
                "size {size}: a steady-state statement carries more than {chunk} rows"
            );
            eprintln!(
                "size {size}: slowest steady statements {:?}",
                statement_probe::slowest(&records, 3)
            );
            steady.push(records.len());
            tx.rollback().await.unwrap();
        }
        eprintln!("reconcile statements: first registration {first:?}, steady state {steady:?}");
        // Steady state: the locking read has one statement per chunk of blocks and the removal scan
        // one round per chunk of active placements (the scan's last round is the short one).
        for (size, count) in sizes.iter().zip(&steady) {
            let expected = steady[0] + (size.div_ceil(chunk) - 1) + (scan_rounds(*size) - scan_rounds(sizes[0]));
            assert_eq!(*count, expected, "size {size}: {steady:?}");
        }
        // First registration adds, per further chunk of new blocks, the four statements of a chunk
        // group (logical blocks, fences, activation, placements) and one locking read. The scan also
        // sees the registered rows and the Page's own initial block, and the share suspension
        // (every fresh identity plus the retired initial block) is chunked as well.
        for (size, count) in sizes.iter().zip(&first) {
            let groups = size.div_ceil(chunk);
            let expected = first[0]
                + 5 * (groups - 1)
                + (scan_rounds(size + 1) - scan_rounds(sizes[0] + 1))
                + ((size + 1).div_ceil(chunk) - (sizes[0] + 1).div_ceil(chunk));
            assert_eq!(*count, expected, "size {size}: {first:?}");
        }
        // A bounded registration issues exactly one chunk group per `chunk` rows of allowance.
        let size = 2 * chunk + 1;
        let mut counts = Vec::new();
        for max_rows in [0, chunk, 2 * chunk] {
            let (_, scope, page) = page(db).await;
            let tx = db.begin().await.unwrap();
            let allowance = RegistrationAllowance::Bounded {
                deadline: Instant::now() + Duration::from_hours(1),
                max_rows,
                min_rows: 0,
            };
            let (outcome, records) = counted(&tx, scope, page, 0, &blocks(size, 1), allowance).await;
            assert_eq!(outcome.pending, size - max_rows);
            assert_eq!(pending_of(&tx, page).await, i32::try_from(size - max_rows).unwrap());
            // Everything else that grows with the registered rows: the scan rounds and the suspension chunks.
            counts.push((
                records.len(),
                scan_rounds(max_rows + 1) + (max_rows + 1).div_ceil(chunk),
            ));
            tx.rollback().await.unwrap();
        }
        for (groups, window) in counts.windows(2).enumerate() {
            let (before, after) = (window[0], window[1]);
            assert_eq!(
                after.0 - before.0,
                4 + (after.1 - before.1),
                "allowance step {groups} must add exactly one chunk group: {counts:?}"
            );
        }
        scratch.drop_self().await;
    }

    /// Registering through bounded steps ends in the same state as one complete registration.
    #[tokio::test]
    async fn bounded_registration_converges_to_complete_registration() {
        let Some(scratch) = scratch_with_leases(&format!("block_registry_conv_{}", Uuid::new_v4().simple())).await
        else {
            eprintln!("SKIPPED (no database)");
            return;
        };
        let db = &scratch.db;
        let (_, reference_scope, reference_page) = page(db).await;
        let tx = db.begin().await.unwrap();
        let reference_blocks = blocks(1_100, 1);
        complete(&tx, reference_scope, reference_page, 1, &reference_blocks)
            .await
            .unwrap();
        let (expected, _) = dump(&tx, reference_page).await;
        for (size, max_rows) in [(40_usize, 1_usize), (1_100, 499), (1_100, 1_000)] {
            let (_, scope, page_id) = page(db).await;
            let target = blocks(size, 1);
            let mut pending = size;
            let mut steps = 0;
            while pending > 0 {
                let outcome = bounded(&tx, scope, page_id, 1, &target, max_rows).await.unwrap();
                assert_eq!(
                    outcome.pending,
                    pending - max_rows.min(pending),
                    "size {size}, allowance {max_rows}: each step registers exactly min(allowance, pending)"
                );
                assert_eq!(pending_of(&tx, page_id).await, i32::try_from(outcome.pending).unwrap());
                pending = outcome.pending;
                steps += 1;
            }
            assert_eq!(steps, size.div_ceil(max_rows));
            if size == 1_100 {
                let (lines, _) = dump(&tx, page_id).await;
                let diff = lines.iter().zip(&expected).find(|(left, right)| left != right);
                assert_eq!(
                    (lines.len(), diff),
                    (expected.len(), None),
                    "allowance {max_rows} must converge to the reference"
                );
            }
        }
        // A deadline already in the past registers nothing, succeeds, and leaves everything pending.
        let (_, scope, page_id) = page(db).await;
        let target = blocks(600, 1);
        let past = Instant::now().checked_sub(Duration::from_secs(1)).unwrap();
        let outcome = reconcile(
            &tx,
            scope,
            page_id,
            1,
            &target,
            RegistrationAllowance::Bounded {
                deadline: past,
                max_rows: usize::MAX,
                min_rows: 0,
            },
        )
        .await
        .unwrap();
        assert_eq!(outcome.pending, 600);
        // ... unless a minimum is demanded, which guarantees progress for completion loops.
        let outcome = reconcile(
            &tx,
            scope,
            page_id,
            1,
            &target,
            RegistrationAllowance::Bounded {
                deadline: past,
                max_rows: usize::MAX,
                min_rows: 1,
            },
        )
        .await
        .unwrap();
        assert_eq!(
            outcome.pending, 100,
            "the first group is forced: one chunk of {REGISTRY_ROWS_PER_STATEMENT}"
        );
        tx.rollback().await.unwrap();
        scratch.drop_self().await;
    }

    pub(super) async fn row_versions(tx: &DatabaseTransaction, page: Uuid) -> Vec<String> {
        tx.query_all(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT 'p:'||id::text||':'||xmin::text AS v FROM flow_block_placements WHERE page_id=$1 UNION ALL SELECT 'f:'||physical_cid||':'||xmin::text FROM flow_block_alias_fences WHERE page_id=$1 ORDER BY 1",
            vec![page.into()],
        ))
        .await
        .unwrap()
        .iter()
        .map(|row| row.try_get::<String>("", "v").unwrap())
        .collect()
    }

    #[tokio::test]
    async fn unchanged_reconcile_rewrites_no_rows() {
        let Some(scratch) = scratch_with_leases(&format!("block_registry_xmin_{}", Uuid::new_v4().simple())).await
        else {
            eprintln!("SKIPPED (no database)");
            return;
        };
        let db = &scratch.db;
        let (_, scope, page) = page(db).await;
        let initial = blocks(500, 1);
        let tx = db.begin().await.unwrap();
        complete(&tx, scope, page, 0, &initial).await.unwrap();
        tx.commit().await.unwrap();
        // A separate transaction gets a new xid, so any rewritten row would show a new xmin.
        let tx = db.begin().await.unwrap();
        let before = row_versions(&tx, page).await;
        // 500 placements and 500 fences, plus the Page's own initial block (removed at the first head).
        assert_eq!(before.len(), 1_002);
        complete(&tx, scope, page, 1, &initial).await.unwrap();
        assert_eq!(
            before,
            row_versions(&tx, page).await,
            "identical content must rewrite no rows"
        );
        let mut changed = initial.clone();
        changed[7] = block(7, 2);
        complete(&tx, scope, page, 2, &changed).await.unwrap();
        let after = row_versions(&tx, page).await;
        let rewritten = before.iter().zip(&after).filter(|(old, new)| old != new).count();
        assert_eq!(rewritten, 1, "a single changed block must rewrite exactly one row");
        tx.rollback().await.unwrap();
        scratch.drop_self().await;
    }

    /// Mirrors the production per-statement caps (100 ms statement, 80 ms lock) of the locked phase.
    pub(super) async fn begin_capped(db: &DatabaseConnection) -> DatabaseTransaction {
        let tx = db.begin().await.unwrap();
        crate::flow::collab::write::set_locked_phase_statement_budgets(&tx, 1)
            .await
            .unwrap();
        tx
    }

    /// The caps `reconcile_10k_blocks_under_production_statement_caps` runs under: the production
    /// ones when the machine is qualified for timing assertions (`OPENPR_FLOW_QUIET_PG_QUALIFIED=1`),
    /// otherwise a 1 s statement and 800 ms lock timeout so CI load cannot fail the structural checks.
    async fn begin_for_timing_mode(db: &DatabaseConnection) -> DatabaseTransaction {
        if quiet_qualified() {
            return begin_capped(db).await;
        }
        let tx = db.begin().await.unwrap();
        tx.execute_unprepared("SET LOCAL lock_timeout = '800ms'").await.unwrap();
        tx.execute_unprepared("SET LOCAL statement_timeout = '1s'")
            .await
            .unwrap();
        tx
    }

    /// The locked-phase staging budget of `collab::write` (4 x the document lock hold limit).
    const STAGING_BUDGET: Duration = crate::flow::collab::write::LOCKED_PHASE_STAGING_BUDGET;

    fn quiet_qualified() -> bool {
        std::env::var("OPENPR_FLOW_QUIET_PG_QUALIFIED").is_ok_and(|value| value == "1")
    }

    /// One capped transaction of a completion-style registration: at least one chunk group, the
    /// production row bound, the production deadline. Returns the outcome, its wall time and statements.
    async fn capped_step(
        db: &DatabaseConnection,
        scope: Uuid,
        page: Uuid,
        head: i64,
        blocks: &[PreparedBlock],
    ) -> (ReconcileOutcome, Duration, Vec<statement_probe::Record>) {
        let tx = begin_for_timing_mode(db).await;
        let started = Instant::now();
        let allowance = RegistrationAllowance::bounded(
            started,
            STAGING_BUDGET,
            REGISTRATION_ROWS_PER_TRANSACTION_MAX,
            REGISTRY_ROWS_PER_STATEMENT,
        );
        let (outcome, records) = counted(&tx, scope, page, head, blocks, allowance).await;
        let elapsed = started.elapsed();
        tx.commit().await.unwrap();
        (outcome, elapsed, records)
    }

    #[tokio::test]
    async fn reconcile_10k_blocks_under_production_statement_caps() {
        let Some(scratch) = scratch_with_leases(&format!("block_registry_10k_{}", Uuid::new_v4().simple())).await
        else {
            eprintln!("SKIPPED (no database)");
            return;
        };
        let db = &scratch.db;
        let (_, scope, page_id) = page(db).await;
        let initial = blocks(10_000, 1);
        let mut steps = Vec::new();
        let mut interleaved_write = false;
        loop {
            let (outcome, elapsed, records) = capped_step(db, scope, page_id, 0, &initial).await;
            eprintln!(
                "10k registration step {}: {elapsed:?}, pending {}, slowest statements {:?}",
                steps.len() + 1,
                outcome.pending,
                statement_probe::slowest(&records, 5)
            );
            assert!(records.iter().all(|record| record.rows <= REGISTRY_ROWS_PER_STATEMENT));
            steps.push(elapsed);
            if outcome.pending == 0 {
                break;
            }
            assert!(steps.len() < 64, "registration must converge");
            if !interleaved_write {
                // The page stays writable while it is partly registered.
                let tx = begin_for_timing_mode(db).await;
                let mut changed = initial.clone();
                changed[5_000] = block(5_000, 2);
                bounded(&tx, scope, page_id, 0, &changed, 0).await.unwrap();
                tx.commit().await.unwrap();
                interleaved_write = true;
            }
        }
        assert!(
            interleaved_write,
            "the first registration step must leave blocks pending"
        );
        let mut changed = initial.clone();
        changed[5_000] = block(5_000, 3);
        let tx = db.begin().await.unwrap();
        let before = row_versions(&tx, page_id).await;
        tx.rollback().await.unwrap();
        let tx = begin_for_timing_mode(db).await;
        let started = Instant::now();
        let (outcome, records) = counted(&tx, scope, page_id, 1, &changed, RegistrationAllowance::Complete).await;
        let steady = started.elapsed();
        let after = row_versions(&tx, page_id).await;
        tx.commit().await.unwrap();
        eprintln!(
            "10k steady write: {steady:?}, slowest statements {:?}",
            statement_probe::slowest(&records, 5)
        );
        assert_eq!(outcome.pending, 0);
        assert!(records.iter().all(|record| record.rows <= REGISTRY_ROWS_PER_STATEMENT));
        assert_eq!(before.len(), after.len());
        let rewritten = before.iter().zip(&after).filter(|(old, new)| old != new).count();
        assert_eq!(rewritten, 1, "a single changed block must rewrite exactly one row");
        if quiet_qualified() {
            for step in &steps {
                assert!(*step < STAGING_BUDGET, "registration step took {step:?}");
            }
            assert!(steady < Duration::from_millis(100), "steady state took {steady:?}");
        }
        scratch.drop_self().await;
    }

    /// A large Page written before the registry was active stays writable while its first
    /// registration takes several bounded writes, and the final state equals the old per-block
    /// loop's.
    #[tokio::test]
    async fn large_page_registration_stays_writable() {
        let Some(scratch) = scratch_with_leases(&format!("block_registry_upgrade_{}", Uuid::new_v4().simple())).await
        else {
            eprintln!("SKIPPED (no database)");
            return;
        };
        let db = &scratch.db;
        let (_, _, page_a) = page(db).await;
        let (_, scope_b, page_b) = page(db).await;
        let registered = blocks(5_000, 1);
        let mut timings = Vec::new();
        for expected in [3_000, 1_000, 0] {
            let tx = begin_for_timing_mode(db).await;
            let started = Instant::now();
            let (statements, outcome) = {
                // The row bound drives the chunking; a wall-clock deadline would make the pending
                // counts depend on runner speed. Timing is asserted only on qualified machines.
                let allowance = RegistrationAllowance::Bounded {
                    deadline: started + Duration::from_hours(1),
                    max_rows: 2_000,
                    min_rows: 0,
                };
                let (result, records) =
                    statement_probe::count(reconcile_accepted(&tx, page_a, 0, &registered, allowance)).await;
                result.unwrap();
                (records, pending_of(&tx, page_a).await)
            };
            timings.push(started.elapsed());
            tx.commit().await.unwrap();
            eprintln!(
                "registration write: {:?}, pending {outcome}, slowest statements {:?}",
                timings.last().unwrap(),
                statement_probe::slowest(&statements, 5)
            );
            assert_eq!(outcome, expected);
        }
        let tx = db.begin().await.unwrap();
        complete(&tx, scope_b, page_b, 0, &registered).await.unwrap();
        let (new_lines, _) = dump(&tx, page_a).await;
        let (old_lines, _) = dump(&tx, page_b).await;
        assert!(
            new_lines.len() > 15_000,
            "the page must be registered: {}",
            new_lines.len()
        );
        assert_eq!(new_lines, old_lines);
        tx.rollback().await.unwrap();
        if quiet_qualified() {
            for timing in &timings {
                assert!(*timing < STAGING_BUDGET, "capped step took {timing:?}");
            }
        }
        scratch.drop_self().await;
    }

    /// A block that is still pending has no identity yet: binding a comment to it is retryable,
    /// and an unknown block stays a plain refusal once the registration is complete.
    #[tokio::test]
    async fn bind_new_on_pending_block_is_retryable() {
        let Some(scratch) = scratch_with_leases(&format!("block_registry_bind_{}", Uuid::new_v4().simple())).await
        else {
            eprintln!("SKIPPED (no database)");
            return;
        };
        let db = &scratch.db;
        let (actor, scope, page_id) = page(db).await;
        let target = blocks(700, 1);
        let tx = db.begin().await.unwrap();
        let outcome = bounded(&tx, scope, page_id, 0, &target, REGISTRY_ROWS_PER_STATEMENT)
            .await
            .unwrap();
        assert_eq!(outcome.pending, 200);
        tx.commit().await.unwrap();
        let comment = |block: &str| {
            let id = Uuid::new_v4();
            let block = block.to_string();
            async move {
                exec(db,"INSERT INTO flow_comments(id,scope_id,object_id,block_id,author_id,body) VALUES($1,$2,$3,$4,$5,'Anchor note')",vec![id.into(),scope.into(),page_id.into(),block.into(),actor.into()]).await;
                id
            }
        };
        let contention = crate::error::ApiErrorKind::ServerDraining(crate::error::ServerDrainingReason::Contention);
        let pending_block = comment("cid:650:Map").await;
        assert_eq!(
            super::super::block_anchors::bind_new(db, pending_block, page_id, Some("cid:650:Map"), None)
                .await
                .unwrap_err()
                .kind(),
            contention
        );
        let registered_block = comment("cid:10:Map").await;
        super::super::block_anchors::bind_new(db, registered_block, page_id, Some("cid:10:Map"), None)
            .await
            .unwrap();
        let tx = db.begin().await.unwrap();
        assert_eq!(
            bounded(&tx, scope, page_id, 0, &target, REGISTRY_ROWS_PER_STATEMENT)
                .await
                .unwrap()
                .pending,
            0
        );
        tx.commit().await.unwrap();
        // The refused comment's transaction rolls back in production; its retry is a new comment.
        let pending_block = comment("cid:650:Map").await;
        super::super::block_anchors::bind_new(db, pending_block, page_id, Some("cid:650:Map"), None)
            .await
            .unwrap();
        let state = db
            .query_one(Statement::from_sql_and_values(
                DbBackend::Postgres,
                "SELECT state FROM flow_block_comment_anchors WHERE comment_id=$1",
                vec![pending_block.into()],
            ))
            .await
            .unwrap()
            .unwrap()
            .try_get::<String>("", "state")
            .unwrap();
        assert_eq!(state, "known");
        let unknown = comment("cid:99999:Map").await;
        let refusal = super::super::block_anchors::bind_new(db, unknown, page_id, Some("cid:99999:Map"), None)
            .await
            .unwrap_err();
        assert_eq!(refusal.kind(), crate::error::ApiErrorKind::InvalidUpdate);
        scratch.drop_self().await;
    }
}
