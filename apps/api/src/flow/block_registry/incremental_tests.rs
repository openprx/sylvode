//! Incremental reconcile against a real database: row-for-row equality with the full reconcile on
//! generated write histories, bounded work, one test per fallback trigger, and mutation checks.
//!
//! Every test drives the registry functions directly (no accept path, no isolated worker), so all
//! of them run on macOS as well as on Linux.
#![allow(clippy::indexing_slicing, clippy::print_stderr)]

use super::fixtures::page;
use super::mutation::{self, Mutation};
use super::tests::{add_share, begin_capped, row_versions, scratch_with_leases, suspended_cids};
use super::*;
use sea_orm::{DatabaseConnection, TransactionTrait};
use std::collections::BTreeMap;

// ---------------------------------------------------------------------------------------------
// Deterministic generator
// ---------------------------------------------------------------------------------------------

struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    fn below(&mut self, bound: usize) -> usize {
        usize::try_from(self.next() % u64::try_from(bound.max(1)).unwrap_or(1)).unwrap_or(0)
    }

    fn chance(&mut self, percent: usize) -> bool {
        self.below(100) < percent
    }
}

#[derive(Clone)]
struct Blk {
    content: u64,
    parent: Option<String>,
    order: u64,
}

/// A document as the registry sees it: blocks keyed by CID with a content identity, a parent and an order.
#[derive(Clone, Default)]
struct Doc {
    blocks: BTreeMap<String, Blk>,
    next: u64,
    grave: Vec<(String, u64)>,
}

impl Doc {
    fn fresh_cid(&mut self) -> String {
        self.next += 1;
        format!("blk:{}:Map", self.next)
    }

    fn insert_new(&mut self, content: Option<u64>) -> String {
        let cid = self.fresh_cid();
        let content = content.unwrap_or(self.next * 7 + 1);
        let order = self.next;
        self.blocks.insert(
            cid.clone(),
            Blk {
                content,
                parent: None,
                order,
            },
        );
        cid
    }

    fn pick(&self, rng: &mut Rng) -> Option<String> {
        let at = rng.below(self.blocks.len());
        self.blocks.keys().nth(at).cloned()
    }

    fn prepared(&self) -> Vec<PreparedBlock> {
        self.blocks
            .iter()
            .map(|(cid, blk)| {
                let mut content_hash = [0xAB_u8; 32];
                content_hash[..8].copy_from_slice(&blk.content.to_be_bytes());
                PreparedBlock {
                    physical_cid: cid.clone(),
                    parent_physical_cid: blk.parent.clone(),
                    order_key: format!("{:012}", blk.order),
                    asset_id: None,
                    legacy_alias: None,
                    content_hash,
                }
            })
            .collect()
    }

    fn seeded(count: usize) -> Self {
        let mut doc = Self::default();
        for _ in 0..count {
            doc.insert_new(None);
        }
        doc
    }
}

/// One generated change; the returned text names it in a failure report.
fn mutate(doc: &mut Doc, rng: &mut Rng, history: &[Doc], bulk: bool) -> String {
    let kind = rng.below(if bulk { 10 } else { 9 });
    match kind {
        0 => {
            let n = 1 + rng.below(8);
            for _ in 0..n {
                doc.insert_new(None);
            }
            format!("insert {n}")
        }
        1 | 2 => {
            let n = 1 + rng.below(5);
            for _ in 0..n {
                if let Some(cid) = doc.pick(rng) {
                    doc.next += 1;
                    let fresh = doc.next * 7 + 1;
                    if let Some(blk) = doc.blocks.get_mut(&cid) {
                        blk.content = fresh;
                    }
                }
            }
            format!("edit {n}")
        }
        3 => {
            let n = 1 + rng.below(4);
            for _ in 0..n {
                if let (Some(cid), Some(parent)) = (doc.pick(rng), doc.pick(rng)) {
                    doc.next += 1;
                    let order = doc.next;
                    if let Some(blk) = doc.blocks.get_mut(&cid) {
                        blk.parent = (parent != cid).then_some(parent);
                        blk.order = order;
                    }
                }
            }
            format!("move {n}")
        }
        4 => {
            let n = 1 + rng.below(5);
            for _ in 0..n {
                if let Some(cid) = doc.pick(rng)
                    && let Some(blk) = doc.blocks.remove(&cid)
                {
                    doc.grave.push((cid, blk.content));
                }
            }
            format!("delete {n}")
        }
        5 => {
            // Delete, then re-create the same CID, with the same or with new content.
            if doc.grave.is_empty() {
                return "revive (nothing deleted yet)".into();
            }
            let at = rng.below(doc.grave.len());
            let (cid, content) = doc.grave.swap_remove(at);
            let same = rng.chance(50);
            doc.next += 1;
            let content = if same { content } else { doc.next * 7 + 1 };
            let order = doc.next;
            doc.blocks.insert(
                cid.clone(),
                Blk {
                    content,
                    parent: None,
                    order,
                },
            );
            format!("revive {cid} same_content={same}")
        }
        6 => {
            // Cut and paste: the block disappears and a new CID carries its content.
            let Some(cid) = doc.pick(rng) else {
                return "cut (empty)".into();
            };
            let Some(removed) = doc.blocks.remove(&cid) else {
                return "cut (gone)".into();
            };
            doc.grave.push((cid.clone(), removed.content));
            let pasted = doc.insert_new(Some(removed.content));
            format!("cut {cid} paste {pasted}")
        }
        7 => {
            // Restore an earlier state of the document wholesale.
            if history.is_empty() {
                return "restore (no history)".into();
            }
            let at = rng.below(history.len());
            let keep_next = doc.next;
            let keep_grave = doc.grave.clone();
            *doc = history[at].clone();
            doc.next = keep_next.max(doc.next);
            doc.grave = keep_grave;
            format!("restore history[{at}]")
        }
        8 => "no change".into(),
        _ => {
            let n = 1_990 + rng.below(40);
            for _ in 0..n {
                doc.insert_new(None);
            }
            format!("bulk insert {n}")
        }
    }
}

/// A fixture page registers its initial block under a CID with a random Loro peer id.
fn mask_peer(cid: &str) -> String {
    match cid.split_once('@') {
        Some((head, tail)) => format!("{head}@peer{}", tail.find(':').map_or("", |at| &tail[at..])),
        None => cid.to_string(),
    }
}

// ---------------------------------------------------------------------------------------------
// Twin pages and the observable state
// ---------------------------------------------------------------------------------------------

/// Everything a reconcile writes for one Page, ids masked by order of first appearance, plus the
/// logical-id to CID map used to name suspended identities.
async fn rows(tx: &DatabaseTransaction, page: Uuid) -> (Vec<String>, HashMap<Uuid, String>) {
    let mut index = HashMap::<Uuid, usize>::new();
    let mut cid_of = HashMap::new();
    let mut lines = Vec::new();
    for row in tx
        .query_all(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT p.physical_cid,p.logical_id,p.generation,p.state,encode(p.content_hash,'hex') AS hash,(p.retired_at IS NOT NULL) AS retired,l.state AS logical_state FROM flow_block_placements p JOIN flow_logical_blocks l ON l.id=p.logical_id WHERE p.page_id=$1 ORDER BY p.physical_cid COLLATE \"C\",p.alias_order",
            vec![page.into()],
        ))
        .await
        .unwrap()
    {
        let logical: Uuid = row.try_get("", "logical_id").unwrap();
        let next = index.len();
        let id = *index.entry(logical).or_insert(next);
        let cid = mask_peer(&row.try_get::<String>("", "physical_cid").unwrap());
        cid_of.insert(logical, cid.clone());
        lines.push(format!(
            "placement {cid} logical#{id} gen={} {} {} retired={} identity={}",
            row.try_get::<i64>("", "generation").unwrap(),
            row.try_get::<String>("", "state").unwrap(),
            row.try_get::<String>("", "hash").unwrap(),
            row.try_get::<bool>("", "retired").unwrap(),
            row.try_get::<String>("", "logical_state").unwrap(),
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
    let checkpoint = tx
        .query_one(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT accepted_head,pending_registrations,legacy_anchors_sealed FROM flow_block_checkpoints WHERE page_id=$1",
            vec![page.into()],
        ))
        .await
        .unwrap()
        .unwrap();
    lines.push(format!(
        "checkpoint head={} pending={} sealed={}",
        checkpoint.try_get::<i64>("", "accepted_head").unwrap(),
        checkpoint.try_get::<i32>("", "pending_registrations").unwrap(),
        checkpoint.try_get::<bool>("", "legacy_anchors_sealed").unwrap(),
    ));
    for row in tx
        .query_all(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT state,auth_revision,content_revision,encode(projection_hash,'hex') AS hash FROM flow_block_shares WHERE source_page_id=$1 ORDER BY source_root_cid,state,auth_revision,content_revision,projection_hash",
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
    (lines, cid_of)
}

#[derive(Clone, Copy)]
struct Pg {
    actor: Uuid,
    scope: Uuid,
    page: Uuid,
}

async fn new_pg(db: &DatabaseConnection) -> Pg {
    let (actor, scope, page) = page(db).await;
    Pg { actor, scope, page }
}

fn production_allowance() -> RegistrationAllowance {
    RegistrationAllowance::Bounded {
        deadline: Instant::now() + Duration::from_hours(1),
        max_rows: REGISTRATION_ROWS_PER_TRANSACTION_MAX,
        min_rows: 0,
    }
}

async fn run_mutated<F: std::future::Future>(mutation: Option<Mutation>, future: F) -> F::Output {
    match mutation {
        Some(mutation) => mutation::with(mutation, future).await,
        None => future.await,
    }
}

/// The twin of the incremental write: the full reconcile.
async fn full(
    tx: &DatabaseTransaction,
    pg: Pg,
    head: i64,
    blocks: &[PreparedBlock],
    allowance: RegistrationAllowance,
) -> ReconcileOutcome {
    reconcile(tx, pg.scope, pg.page, head, blocks, allowance).await.unwrap()
}

async fn incremental(
    tx: &DatabaseTransaction,
    pg: Pg,
    head: i64,
    blocks: &[PreparedBlock],
    allowance: RegistrationAllowance,
    mutation: Option<Mutation>,
) -> ReconcileOutcome {
    run_mutated(
        mutation,
        reconcile_in_mode(
            tx,
            pg.scope,
            pg.page,
            head,
            blocks,
            allowance,
            ReconcileMode::Incremental,
        ),
    )
    .await
    .unwrap()
}

/// Rows of both twins must be identical, and so must the identities suspended by this write.
async fn same_rows(
    tx: &DatabaseTransaction,
    (a, out_a): (Pg, &ReconcileOutcome),
    (b, out_b): (Pg, &ReconcileOutcome),
) -> Result<(), String> {
    let (lines_a, map_a) = rows(tx, a.page).await;
    let (lines_b, map_b) = rows(tx, b.page).await;
    if lines_a != lines_b {
        let at = lines_a
            .iter()
            .zip(&lines_b)
            .position(|(x, y)| x != y)
            .unwrap_or_else(|| lines_a.len().min(lines_b.len()));
        return Err(format!(
            "rows differ at line {at} ({} vs {} lines): incremental {:?} / full {:?}",
            lines_a.len(),
            lines_b.len(),
            lines_a.get(at),
            lines_b.get(at)
        ));
    }
    if suspended_cids(&out_a.suspended, &map_a) != suspended_cids(&out_b.suspended, &map_b) {
        return Err("suspended identities differ".into());
    }
    if out_a.pending != out_b.pending {
        return Err(format!("pending {} vs {}", out_a.pending, out_b.pending));
    }
    Ok(())
}

// ---------------------------------------------------------------------------------------------
// The differential driver
// ---------------------------------------------------------------------------------------------

#[derive(Default, Debug)]
struct CaseStats {
    writes: usize,
    incremental: usize,
    full: usize,
    rollbacks: usize,
    triggers: BTreeMap<&'static str, usize>,
}

/// One generated history on a twin pair. After every write the incremental twin must equal the
/// full twin; a mismatch is returned as a description naming the seed and the step.
async fn run_case(
    db: &DatabaseConnection,
    seed: u64,
    initial: usize,
    steps: usize,
    bulk: bool,
    mutation: Option<Mutation>,
) -> Result<CaseStats, String> {
    let mut rng = Rng(seed);
    let (a, b) = (new_pg(db).await, new_pg(db).await);
    let mut doc = Doc::seeded(initial);
    let mut history: Vec<Doc> = Vec::new();
    let mut stats = CaseStats::default();
    let mut head = 0_i64;
    // First registration of both twins: complete, like page creation.
    {
        let tx = db.begin().await.unwrap();
        let blocks = doc.prepared();
        let out_a = incremental(&tx, a, head, &blocks, RegistrationAllowance::Complete, mutation).await;
        let out_b = full(&tx, b, head, &blocks, RegistrationAllowance::Complete).await;
        same_rows(&tx, (a, &out_a), (b, &out_b)).await?;
        tx.commit().await.unwrap();
    }
    let mut shared = 0;
    for step in 0..steps {
        let before = doc.clone();
        let mut ops = Vec::new();
        for _ in 0..=rng.below(3) {
            let bulk_now = bulk && rng.chance(8);
            ops.push(mutate(&mut doc, &mut rng, &history, bulk_now));
        }
        let trigger = match rng.below(100) {
            0..=5 => "gap",
            6..=10 => "repair",
            11..=15 => "cold",
            16..=20 => "rollback",
            21..=24 => "complete",
            25..=29 => "older_binary",
            30..=34 => "stale_same_head",
            _ => "none",
        };
        *stats.triggers.entry(trigger).or_default() += 1;
        match trigger {
            "gap" => head += 2,
            "repair" => {
                exec_ignore(
                    db,
                    "UPDATE flow_block_checkpoints SET registry_digest=NULL WHERE page_id=$1",
                    a.page,
                )
                .await;
                head += 1;
            }
            "cold" => {
                BASELINES.lock().forget(a.page);
                head += 1;
            }
            _ => head += 1,
        }
        let context =
            |message: String| format!("seed {seed} step {step} trigger {trigger} ops {ops:?} head {head}: {message}");
        let blocks = doc.prepared();
        let tx = db.begin().await.unwrap();
        let (out_a, out_b);
        if trigger == "older_binary" {
            // A binary without this change reconciles in full and leaves the digest column alone.
            let digest_before = digest_of(&tx, a.page).await;
            out_a = full(&tx, a, head, &blocks, production_allowance()).await;
            set_digest(&tx, a.page, digest_before).await;
            out_b = full(&tx, b, head, &blocks, production_allowance()).await;
        } else {
            let allowance = if trigger == "complete" {
                RegistrationAllowance::Complete
            } else {
                production_allowance()
            };
            out_a = incremental(&tx, a, head, &blocks, allowance, mutation).await;
            out_b = full(&tx, b, head, &blocks, allowance).await;
        }
        stats.writes += 1;
        if out_a.incremental {
            stats.incremental += 1;
        } else {
            stats.full += 1;
        }
        if let Err(message) = same_rows(&tx, (a, &out_a), (b, &out_b)).await {
            return Err(context(message));
        }
        if out_a.pending == 0 && trigger != "older_binary" {
            let drift = verify_registry(&tx, a.scope, a.page, &blocks, 3).await.unwrap();
            if !drift.is_empty() {
                return Err(context(format!("verify_registry: {drift:?}")));
            }
        }
        if trigger == "rollback" {
            tx.rollback().await.unwrap();
            stats.rollbacks += 1;
            head -= 1;
            doc = before;
            continue;
        }
        tx.commit().await.unwrap();
        if trigger == "stale_same_head" {
            // Another process committed this head with other content: the remembered state is foreign.
            let stale = history.last().cloned().unwrap_or_default();
            BASELINES.lock().store(
                a.page,
                BaselineEntry {
                    head,
                    digest: [0xEE; 32],
                    blocks: Arc::new(
                        stale
                            .prepared()
                            .into_iter()
                            .map(|block| (block.physical_cid, block.content_hash))
                            .collect(),
                    ),
                },
            );
        }
        history.push(before);
        if history.len() > 6 {
            history.remove(0);
        }
        // Shares on random blocks, on both twins, once the registry holds them.
        if out_a.pending == 0
            && shared < 4
            && rng.chance(20)
            && let Some(cid) = doc.pick(&mut rng)
        {
            {
                let tx = db.begin().await.unwrap();
                let active = tx
                    .query_one(Statement::from_sql_and_values(
                        DbBackend::Postgres,
                        "SELECT 1 AS one FROM flow_block_placements WHERE page_id=$1 AND physical_cid=$2 AND state='active'",
                        vec![a.page.into(), cid.clone().into()],
                    ))
                    .await
                    .unwrap()
                    .is_some();
                if active {
                    for pg in [a, b] {
                        add_share(&tx, (pg.actor, pg.scope, pg.page), &blocks, &cid).await;
                    }
                    shared += 1;
                }
                tx.commit().await.unwrap();
            }
        }
    }
    Ok(stats)
}

async fn exec_ignore(db: &DatabaseConnection, sql: &str, page: Uuid) {
    db.execute(Statement::from_sql_and_values(
        DbBackend::Postgres,
        sql,
        vec![page.into()],
    ))
    .await
    .unwrap();
}

async fn digest_of(tx: &DatabaseTransaction, page: Uuid) -> Option<Vec<u8>> {
    tx.query_one(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "SELECT registry_digest FROM flow_block_checkpoints WHERE page_id=$1",
        vec![page.into()],
    ))
    .await
    .unwrap()
    .unwrap()
    .try_get("", "registry_digest")
    .unwrap()
}

async fn set_digest(tx: &DatabaseTransaction, page: Uuid, digest: Option<Vec<u8>>) {
    tx.execute(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "UPDATE flow_block_checkpoints SET registry_digest=$2 WHERE page_id=$1",
        vec![page.into(), digest.into()],
    ))
    .await
    .unwrap();
}

macro_rules! scratch {
    ($label:expr) => {
        match scratch_with_leases(&format!("{}_{}", $label, Uuid::new_v4().simple())).await {
            Some(scratch) => scratch,
            None => {
                eprintln!("SKIPPED (no database)");
                return;
            }
        }
    };
}

const SEEDS: std::ops::Range<u64> = 1..25;

// ---------------------------------------------------------------------------------------------
// 1. Differential tests
// ---------------------------------------------------------------------------------------------

#[tokio::test]
async fn incremental_equals_full_on_generated_histories() {
    let scratch = scratch!("incr_diff");
    let mut total = CaseStats::default();
    for seed in SEEDS {
        let initial = 20 + usize::try_from(seed).unwrap() * 7;
        let stats = run_case(&scratch.db, seed, initial, 40, false, None)
            .await
            .unwrap_or_else(|message| panic!("{message}"));
        total.writes += stats.writes;
        total.incremental += stats.incremental;
        total.full += stats.full;
        total.rollbacks += stats.rollbacks;
        for (name, count) in stats.triggers {
            *total.triggers.entry(name).or_default() += count;
        }
    }
    eprintln!(
        "differential: seeds {SEEDS:?}, {} writes, {} incremental, {} full, {} rolled back, triggers {:?}",
        total.writes, total.incremental, total.full, total.rollbacks, total.triggers
    );
    assert!(
        total.incremental * 2 > total.writes,
        "the incremental path must carry most writes: {total:?}"
    );
    assert!(total.full > 0, "the fallbacks must be exercised: {total:?}");
    scratch.drop_self().await;
}

#[tokio::test]
async fn incremental_equals_full_near_the_row_limit() {
    let scratch = scratch!("incr_bulk");
    for seed in [101_u64, 102, 103] {
        // Bulk inserts of 1,990-2,029 blocks sit on both sides of the 2,000 rows one write registers.
        let stats = run_case(&scratch.db, seed, 60, 14, true, None)
            .await
            .unwrap_or_else(|message| panic!("{message}"));
        eprintln!("bulk seed {seed}: {stats:?}");
    }
    scratch.drop_self().await;
}

// ---------------------------------------------------------------------------------------------
// 2. Mutation checks
// ---------------------------------------------------------------------------------------------

#[tokio::test]
async fn deliberate_bugs_are_caught_by_the_differential_test() {
    let scratch = scratch!("incr_mutation");
    // The unmutated run is the control: the same seeds pass.
    for seed in SEEDS.take(4) {
        run_case(&scratch.db, seed, 30, 40, false, None)
            .await
            .unwrap_or_else(|message| panic!("control {message}"));
    }
    for mutation in [
        Mutation::SkipDeletes,
        Mutation::TrustStaleCheckpoint,
        Mutation::MissMovedBlock,
        Mutation::IgnoreContentChanges,
    ] {
        let mut caught = None;
        for seed in SEEDS {
            if let Err(message) = run_case(&scratch.db, seed, 30, 40, false, Some(mutation)).await {
                caught = Some(message);
                break;
            }
        }
        eprintln!("mutation {mutation:?} caught by: {caught:?}");
        assert!(caught.is_some(), "{mutation:?} survived every case");
    }
    scratch.drop_self().await;
}

// ---------------------------------------------------------------------------------------------
// 3. Fallback triggers: each one takes the full reconcile, and the next write is incremental again
// ---------------------------------------------------------------------------------------------

/// A committed page with `count` registered blocks at head 1 and a warm baseline.
async fn warm(db: &DatabaseConnection, count: usize) -> (Pg, Doc) {
    let pg = new_pg(db).await;
    let doc = Doc::seeded(count);
    let tx = db.begin().await.unwrap();
    full(&tx, pg, 0, &doc.prepared(), RegistrationAllowance::Complete).await;
    tx.commit().await.unwrap();
    let mut next = doc.clone();
    next.insert_new(None);
    let tx = db.begin().await.unwrap();
    let out = incremental(&tx, pg, 1, &next.prepared(), production_allowance(), None).await;
    assert!(out.incremental, "the warm-up write is incremental");
    tx.commit().await.unwrap();
    (pg, next)
}

/// One more write at `head`; returns whether it ran incrementally and checks `verify_registry`.
async fn step(
    db: &DatabaseConnection,
    pg: Pg,
    doc: &mut Doc,
    head: i64,
    allowance: RegistrationAllowance,
) -> ReconcileOutcome {
    doc.insert_new(None);
    if let Some(cid) = doc.pick(&mut Rng(head.unsigned_abs())) {
        doc.next += 1;
        let fresh = doc.next * 7 + 1;
        if let Some(blk) = doc.blocks.get_mut(&cid) {
            blk.content = fresh;
        }
    }
    let tx = db.begin().await.unwrap();
    let out = incremental(&tx, pg, head, &doc.prepared(), allowance, None).await;
    if out.pending == 0 {
        assert_eq!(
            verify_registry(&tx, pg.scope, pg.page, &doc.prepared(), 5)
                .await
                .unwrap(),
            Vec::<String>::new()
        );
    }
    tx.commit().await.unwrap();
    out
}

#[tokio::test]
async fn every_fallback_trigger_takes_the_full_reconcile() {
    let scratch = scratch!("incr_triggers");
    let db = &scratch.db;

    // Control: with a warm baseline the next write is incremental.
    let (pg, mut doc) = warm(db, 120).await;
    assert!(step(db, pg, &mut doc, 2, production_allowance()).await.incremental);

    // T1 first registration: a page without checkpoint or baseline.
    let fresh = new_pg(db).await;
    let mut fresh_doc = Doc::seeded(30);
    exec_ignore(db, "DELETE FROM flow_block_checkpoints WHERE page_id=$1", fresh.page).await;
    BASELINES.lock().forget(fresh.page);
    let tx = db.begin().await.unwrap();
    let out = incremental(&tx, fresh, 0, &fresh_doc.prepared(), production_allowance(), None).await;
    assert!(!out.incremental, "T1 first registration");
    tx.commit().await.unwrap();
    assert!(
        step(db, fresh, &mut fresh_doc, 1, production_allowance())
            .await
            .incremental
    );

    // T2 a pending resumable registration: the writes that leave blocks pending, and the one that
    // completes them, are full; the write after that is incremental.
    let big = new_pg(db).await;
    let mut big_doc = Doc::seeded(30);
    let tx = db.begin().await.unwrap();
    full(&tx, big, 0, &big_doc.prepared(), RegistrationAllowance::Complete).await;
    tx.commit().await.unwrap();
    for _ in 0..70 {
        big_doc.insert_new(None);
    }
    let limited = RegistrationAllowance::Bounded {
        deadline: Instant::now() + Duration::from_hours(1),
        max_rows: 40,
        min_rows: 0,
    };
    let mut head = 1;
    let mut rounds = 0;
    loop {
        let tx = db.begin().await.unwrap();
        let out = incremental(&tx, big, head, &big_doc.prepared(), limited, None).await;
        tx.commit().await.unwrap();
        head += 1;
        rounds += 1;
        // The first round starts from a complete registry, so it may be incremental; every round that
        // starts from a pending registry must be full.
        assert!(
            rounds == 1 || !out.incremental,
            "T2 pending registration, round {rounds}"
        );
        if out.pending == 0 {
            break;
        }
        assert!(rounds < 10);
    }
    assert!(rounds >= 2);
    assert!(
        step(db, big, &mut big_doc, head, production_allowance())
            .await
            .incremental
    );
    head += 1;

    // T3 lagging checkpoint (a crash between commit and reconcile, or a reader version that
    // skipped the registry): the head advanced by two.
    assert!(
        !step(db, big, &mut big_doc, head + 1, production_allowance())
            .await
            .incremental
    );
    head += 2;
    assert!(
        step(db, big, &mut big_doc, head, production_allowance())
            .await
            .incremental
    );
    head += 1;

    // T4 a repaired or migrated page: digest cleared.
    exec_ignore(
        db,
        "UPDATE flow_block_checkpoints SET registry_digest=NULL WHERE page_id=$1",
        big.page,
    )
    .await;
    assert!(
        !step(db, big, &mut big_doc, head, production_allowance())
            .await
            .incremental
    );
    head += 1;
    assert!(
        step(db, big, &mut big_doc, head, production_allowance())
            .await
            .incremental
    );
    head += 1;

    // T5 a write by an older binary: the head advances, the digest column does not.
    let tx = db.begin().await.unwrap();
    let digest = digest_of(&tx, big.page).await;
    big_doc.insert_new(None);
    full(&tx, big, head, &big_doc.prepared(), production_allowance()).await;
    set_digest(&tx, big.page, digest).await;
    tx.commit().await.unwrap();
    head += 1;
    assert!(
        !step(db, big, &mut big_doc, head, production_allowance())
            .await
            .incremental
    );
    head += 1;
    assert!(
        step(db, big, &mut big_doc, head, production_allowance())
            .await
            .incremental
    );
    head += 1;

    // T6 a cold process or another instance: no remembered state, or a foreign one for the same head.
    BASELINES.lock().forget(big.page);
    assert!(
        !step(db, big, &mut big_doc, head, production_allowance())
            .await
            .incremental
    );
    head += 1;
    BASELINES.lock().store(
        big.page,
        BaselineEntry {
            head: head - 1,
            digest: [0x11; 32],
            blocks: Arc::new(HashMap::new()),
        },
    );
    assert!(
        !step(db, big, &mut big_doc, head, production_allowance())
            .await
            .incremental
    );
    head += 1;

    // T7 import, package and creation callers pass `Complete`: always full.
    assert!(
        !step(db, big, &mut big_doc, head, RegistrationAllowance::Complete)
            .await
            .incremental
    );
    head += 1;

    // T8 the head must be exactly one past the checkpoint, never equal (a repeated reconcile of the
    // same head, as the comment anchor path does).
    let tx = db.begin().await.unwrap();
    let out = incremental(&tx, big, head - 1, &big_doc.prepared(), production_allowance(), None).await;
    assert!(!out.incremental, "T8 same head");
    tx.commit().await.unwrap();

    // T9 a rolled-back write does not poison the next one: the previous state is still remembered,
    // so the retry stays incremental and correct.
    let tx = db.begin().await.unwrap();
    let mut doomed = big_doc.clone();
    doomed.insert_new(None);
    let out = incremental(&tx, big, head, &doomed.prepared(), production_allowance(), None).await;
    assert!(out.incremental);
    tx.rollback().await.unwrap();
    assert!(
        step(db, big, &mut big_doc, head, production_allowance())
            .await
            .incremental
    );

    // T10 the full reconcile's budget behaviour is untouched: an expired deadline registers nothing
    // on the incremental path either, and leaves the page pending for the full path.
    head += 1;
    let mut more = big_doc.clone();
    for _ in 0..5 {
        more.insert_new(None);
    }
    let past = Instant::now().checked_sub(Duration::from_secs(1)).unwrap();
    let tx = db.begin().await.unwrap();
    let out = incremental(
        &tx,
        big,
        head,
        &more.prepared(),
        RegistrationAllowance::Bounded {
            deadline: past,
            max_rows: usize::MAX,
            min_rows: 0,
        },
        None,
    )
    .await;
    assert_eq!(out.pending, 5);
    tx.commit().await.unwrap();
    head += 1;
    let tx = db.begin().await.unwrap();
    let out = incremental(&tx, big, head, &more.prepared(), production_allowance(), None).await;
    assert!(
        !out.incremental && out.pending == 0,
        "pending registration resumes in full"
    );
    tx.commit().await.unwrap();
    scratch.drop_self().await;
}

// ---------------------------------------------------------------------------------------------
// 4. Counting test: work depends on the change, not on the page
// ---------------------------------------------------------------------------------------------

const EDITED: usize = 50;
const INSERTED: usize = 10;
const DELETED: usize = 5;

/// Register `size` blocks, then apply one write of `EDITED` edits, `INSERTED` inserts and `DELETED`
/// deletes in `mode`. Returns the statements of that write and the registry rows it rewrote.
async fn measured_write(
    db: &DatabaseConnection,
    size: usize,
    mode: ReconcileMode,
) -> (Vec<statement_probe::Record>, usize, Duration, bool) {
    let pg = new_pg(db).await;
    let mut doc = Doc::seeded(size);
    let tx = db.begin().await.unwrap();
    reconcile(
        &tx,
        pg.scope,
        pg.page,
        0,
        &doc.prepared(),
        RegistrationAllowance::Complete,
    )
    .await
    .unwrap();
    tx.commit().await.unwrap();
    let keys = doc.blocks.keys().cloned().collect::<Vec<_>>();
    for cid in keys.iter().take(EDITED) {
        doc.next += 1;
        let fresh = doc.next * 7 + 1;
        if let Some(blk) = doc.blocks.get_mut(cid) {
            blk.content = fresh;
        }
    }
    for cid in keys.iter().rev().take(DELETED) {
        doc.blocks.remove(cid);
    }
    for _ in 0..INSERTED {
        doc.insert_new(None);
    }
    let tx = begin_capped(db).await;
    let before = row_versions(&tx, pg.page).await;
    let started = Instant::now();
    let (outcome, records) = statement_probe::count(reconcile_in_mode(
        &tx,
        pg.scope,
        pg.page,
        1,
        &doc.prepared(),
        production_allowance(),
        mode,
    ))
    .await;
    let elapsed = started.elapsed();
    let outcome = outcome.unwrap();
    let after = row_versions(&tx, pg.page).await;
    let known = before.iter().collect::<HashSet<_>>();
    let rewritten = after.iter().filter(|line| !known.contains(line)).count();
    assert_eq!(outcome.pending, 0);
    assert_eq!(
        verify_registry(&tx, pg.scope, pg.page, &doc.prepared(), 3)
            .await
            .unwrap(),
        Vec::<String>::new()
    );
    tx.commit().await.unwrap();
    (records, rewritten, elapsed, outcome.incremental)
}

#[tokio::test]
async fn incremental_work_is_bounded_by_the_change() {
    let scratch = scratch!("incr_count");
    let db = &scratch.db;
    let sizes = [200_usize, 2_000, 10_000];
    // Rows rewritten: EDITED hash updates, INSERTED new placements and fences, DELETED retirements
    // (each of which also marks its fence retired, through the placement trigger).
    let expected_rows = EDITED + 2 * INSERTED + 2 * DELETED;
    let bound = EDITED + INSERTED;
    let mut incremental_runs = Vec::new();
    let mut full_runs = Vec::new();
    for size in sizes {
        let (records, rewritten, elapsed, took_incremental) =
            measured_write(db, size, ReconcileMode::Incremental).await;
        assert!(took_incremental, "size {size}");
        assert_eq!(rewritten, expected_rows, "size {size}: rows written");
        assert!(
            records.iter().all(|record| record.rows <= bound),
            "size {size}: a statement carries more than the change ({bound} rows): {:?}",
            statement_probe::slowest(&records, 3)
        );
        eprintln!(
            "incremental size {size}: {} statements, {rewritten} rows written, {elapsed:?}, statements took {:?}",
            records.len(),
            records.iter().map(|record| record.elapsed).sum::<Duration>()
        );
        incremental_runs.push(records.len());
        let (records, rewritten, elapsed, took_incremental) = measured_write(db, size, ReconcileMode::Full).await;
        assert!(!took_incremental);
        assert_eq!(rewritten, expected_rows);
        eprintln!(
            "full        size {size}: {} statements, {rewritten} rows written, {elapsed:?}, widest statement {} rows",
            records.len(),
            records.iter().map(|record| record.rows).max().unwrap_or(0)
        );
        full_runs.push((
            records.len(),
            records.iter().map(|record| record.rows).max().unwrap_or(0),
        ));
    }
    // The statement count of the incremental write is the same at every page size ...
    assert!(
        incremental_runs.windows(2).all(|pair| pair[0] == pair[1]),
        "statements per write depend on the page size: {incremental_runs:?}"
    );
    // ... and the bound itself: checkpoint (2), locking read, writes (at most 5 chunk statements
    // for the touched blocks), retirement (2), suspension, checkpoint update, shares, anchors.
    assert!(incremental_runs[0] <= 16, "{incremental_runs:?}");
    // Negative control, which is what the code did before: the full reconcile's statement count and
    // widest statements grow with the page. The assertion below is the one the old code fails.
    assert!(
        full_runs.windows(2).all(|pair| pair[0].0 < pair[1].0),
        "the full path must grow with the page: {full_runs:?}"
    );
    assert!(
        full_runs.last().is_some_and(|last| last.1 > bound),
        "the full path locks whole chunks: {full_runs:?}"
    );
    scratch.drop_self().await;
}

// ---------------------------------------------------------------------------------------------
// 5. Timing report (printed, never asserted)
// ---------------------------------------------------------------------------------------------

#[tokio::test]
async fn timing_report_of_one_fifty_block_write() {
    let scratch = scratch!("incr_timing");
    let db = &scratch.db;
    for size in [1_000_usize, 5_000, 10_000] {
        for (name, mode) in [
            ("full", ReconcileMode::Full),
            ("incremental", ReconcileMode::Incremental),
        ] {
            let mut samples = Vec::new();
            let mut statements = 0;
            let mut rows = 0;
            for _ in 0..3 {
                let (records, rewritten, elapsed, _) = measured_write(db, size, mode).await;
                statements = records.len();
                rows = rewritten;
                samples.push(elapsed);
            }
            samples.sort();
            eprintln!(
                "timing {name:11} blocks {size:6}: median {:?} (min {:?}, max {:?}), {statements} statements, {rows} rows rewritten",
                samples[1], samples[0], samples[2]
            );
        }
    }
    scratch.drop_self().await;
}

// ---------------------------------------------------------------------------------------------
// 6. The verification function
// ---------------------------------------------------------------------------------------------

#[tokio::test]
async fn verify_registry_reports_drift() {
    let scratch = scratch!("incr_verify");
    let db = &scratch.db;
    let pg = new_pg(db).await;
    let doc = Doc::seeded(40);
    let tx = db.begin().await.unwrap();
    full(&tx, pg, 0, &doc.prepared(), RegistrationAllowance::Complete).await;
    assert!(
        verify_registry(&tx, pg.scope, pg.page, &doc.prepared(), 10)
            .await
            .unwrap()
            .is_empty()
    );
    let cids = doc.blocks.keys().cloned().collect::<Vec<_>>();
    // A wrong content hash, a missing placement and an extra one.
    tx.execute(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "UPDATE flow_block_placements SET content_hash=decode(repeat('11',32),'hex') WHERE page_id=$1 AND physical_cid=$2 AND state='active'",
        vec![pg.page.into(), cids[0].clone().into()],
    ))
    .await
    .unwrap();
    tx.execute(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "UPDATE flow_block_placements SET state='deleted',retired_at=now() WHERE page_id=$1 AND physical_cid=$2 AND state='active'",
        vec![pg.page.into(), cids[1].clone().into()],
    ))
    .await
    .unwrap();
    let mut shorter = doc.clone();
    shorter.blocks.remove(&cids[2]);
    let drift = verify_registry(&tx, pg.scope, pg.page, &shorter.prepared(), 10)
        .await
        .unwrap();
    assert_eq!(drift.len(), 3, "{drift:?}");
    assert!(drift.iter().any(|line| line.contains("content hash differs")));
    assert!(
        drift
            .iter()
            .any(|line| line.contains("block without an active placement"))
    );
    assert!(
        drift
            .iter()
            .any(|line| line.contains("active placement without a block"))
    );
    tx.rollback().await.unwrap();
    scratch.drop_self().await;
}
