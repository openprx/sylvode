//! Tests of the write base: the warm cache's validation, invalidation and bounds, the single base
//! load and export per write, and the guarantee that a stale base can never be committed.
//!
//! Every test but the `real_worker_*` ones registers its document with
//! [`in_process_apply::enable`], so the isolated apply runs in this process (the worker's own
//! import entry point, projection and limit check, without the Linux-only worker process). That
//! keeps them runnable on every platform. The `real_worker_*` tests do not register their document:
//! they go through the real isolated worker and, like every other write-path test that does, need
//! Linux (on another platform the write is refused as `isolated_apply_unsupported_platform`).

#![allow(
    clippy::cast_possible_wrap,
    clippy::cast_possible_truncation,
    clippy::excessive_nesting
)]

use std::sync::Arc;

use collab_core::{CollabEngine, LoroCollabEngine};
use sea_orm::{ConnectionTrait, DbBackend, FromQueryResult, Statement};
use serde_json::{Value, json};
use uuid::Uuid;

use super::{
    AppState, DocumentCoordinator, SessionRegistry, SnapshotAdvancer, TEST_DATABASE_URL_ENV, bootstrap, command_input,
    create_page, head_seq_of, plain_text_of, scratch, seed_workspace, state_for,
};
use crate::flow::collab::cache::{HeadKey, WarmCache};
use crate::flow::collab::frame::RejectedCode;
use crate::flow::collab::write::{
    AcceptOutcome, BaseOutcome, HydrateOutcome, LockedOutcome, UpdateRequest, accept_update, base_counters, cold_cache,
    find_prior_update, finish_committed, hydrate_and_apply, in_process_apply, prepare_build_engine, read_observed_head,
    resolve_base, run_locked_phase,
};
use crate::flow::collab::{authz, runtime};
use crate::flow::command::execute_command;

fn request_for(
    document_id: Uuid,
    update_id: Uuid,
    bytes: Vec<u8>,
    workspace_id: Uuid,
    actor_id: Uuid,
    checked_epoch: i64,
) -> UpdateRequest {
    UpdateRequest {
        origin: crate::flow::event_origin::CommandOrigin::first_request_from(
            crate::flow::event_origin::EventSurface::Rest,
        ),
        document_id,
        update_id,
        bytes,
        idempotency_key: None,
        event_idempotency_key: None,
        origin_client_id: Some("base-cache-test".to_string()),
        message: None,
        actor_id,
        actor_is_bot: false,
        workspace_id,
        checked_epoch,
        expected_frontier: None,
    }
}

/// One simulated API process: its own warm cache and coordinator over the shared database.
struct Process {
    cache: WarmCache,
    coordinator: DocumentCoordinator,
    registry: SessionRegistry,
    snapshot: SnapshotAdvancer,
}

impl Process {
    fn new() -> Self {
        Self {
            cache: WarmCache::new(),
            coordinator: DocumentCoordinator::new(),
            registry: SessionRegistry::new(),
            snapshot: SnapshotAdvancer::new(),
        }
    }

    /// What `execute_content_command` does, against this process's own cache: build `change` on a
    /// private engine at the head the database reports (no permit held), then accept the update
    /// through the ordinary path.
    async fn write(
        &self,
        state: &AppState,
        ids: (Uuid, Uuid, Uuid),
        update_id: Uuid,
        change: impl FnOnce(&mut LoroCollabEngine),
    ) -> AcceptOutcome {
        let (workspace_id, actor_id, document_id) = ids;
        let mut engine = build_engine(state, &self.cache, document_id, update_id).await;
        let frontier = engine.frontier();
        change(&mut engine);
        let bytes = engine.export_from(&frontier).expect("update exports");
        self.accept_bytes(state, (workspace_id, actor_id, document_id), update_id, bytes)
            .await
    }

    /// A write that goes through the public accept path with a pre-built update (no held base).
    async fn accept_bytes(
        &self,
        state: &AppState,
        ids: (Uuid, Uuid, Uuid),
        update_id: Uuid,
        bytes: Vec<u8>,
    ) -> AcceptOutcome {
        let (workspace_id, actor_id, document_id) = ids;
        let epoch = authz::read_epoch(&state.db, workspace_id).await.expect("epoch reads");
        accept_update(
            &state.db,
            &self.cache,
            &self.coordinator,
            &self.registry,
            &self.snapshot,
            10,
            None,
            request_for(document_id, update_id, bytes, workspace_id, actor_id, epoch),
        )
        .await
        .expect("accept does not error")
    }
}

async fn build_engine(state: &AppState, cache: &WarmCache, document_id: Uuid, update_id: Uuid) -> LoroCollabEngine {
    prepare_build_engine(&state.db, cache, document_id, update_id, None)
        .await
        .expect("base resolves")
        .unwrap_or_else(|_| panic!("a build engine is expected"))
}

fn accepted_seq(outcome: &AcceptOutcome) -> i64 {
    match outcome {
        AcceptOutcome::Accepted(accepted) => accepted.head_seq,
        AcceptOutcome::Rejected(rejected) => panic!("expected an accepted write, got {rejected:?}"),
    }
}

async fn head_of(state: &AppState, document_id: Uuid) -> (i64, Vec<u8>, String) {
    let observed = read_observed_head(&state.db, document_id)
        .await
        .expect("head reads")
        .expect("document exists");
    (observed.head_seq, observed.head_frontier, observed.format_version)
}

async fn rebuild_from_database(state: &AppState, document_id: Uuid) -> LoroCollabEngine {
    let boot = bootstrap::load(&state.db, document_id).await.expect("bootstrap loads");
    let mut engine = LoroCollabEngine::load(&boot.snapshot).expect("snapshot decodes");
    for tail in &boot.tail_updates {
        engine.import_update(&tail.bytes).expect("tail replays");
    }
    engine
}

fn projection_of(engine: &LoroCollabEngine) -> Value {
    let semantic = engine.semantic_snapshot().expect("semantic snapshot");
    crate::flow::projection::state_json(&semantic).expect("state json")
}

/// The cache entry for the document's current head, if it has one: engine state and bytes.
fn cached_state(cache: &WarmCache, document_id: Uuid, head: &(i64, Vec<u8>, String)) -> Option<(Value, Arc<Vec<u8>>)> {
    let key = HeadKey {
        head_seq: head.0,
        frontier: &head.1,
        format_version: &head.2,
    };
    let hit = cache.checkout(document_id, &key, true).expect("checkout")?;
    let bytes = hit.base_bytes?;
    let engine = hit.engine?;
    Some((projection_of(&engine), bytes))
}

/// The cache is only an optimisation of reading the same committed state: whenever it serves the
/// head, its engine and its base bytes decode to exactly what the database rebuilds.
async fn assert_cache_equals_database(state: &AppState, cache: &WarmCache, document_id: Uuid) {
    let head = head_of(state, document_id).await;
    let (cached_projection, bytes) = cached_state(cache, document_id, &head).expect("the cache serves the head");
    let rebuilt = rebuild_from_database(state, document_id).await;
    assert_eq!(
        cached_projection,
        projection_of(&rebuilt),
        "cached engine == database state"
    );
    let from_bytes = LoroCollabEngine::load(&bytes).expect("cached bytes decode");
    assert_eq!(
        projection_of(&from_bytes),
        projection_of(&rebuilt),
        "cached base bytes == database state"
    );
    assert!(
        from_bytes.matches_frontier(&head.1) && rebuilt.matches_frontier(&head.1),
        "cached bytes and rebuilt state sit exactly at the database head frontier"
    );
}

fn title_change(title: String) -> impl FnOnce(&mut LoroCollabEngine) {
    move |engine| engine.set_title(&title).expect("title sets")
}

/// A change that inserts one block with distinct text. Unlike a title (last writer wins), a lost
/// update shows up as a missing block.
fn block_change(id: &str, text: &str) -> impl FnOnce(&mut LoroCollabEngine) {
    let operations: Vec<collab_core::Operation> = serde_json::from_value(json!([
        {"op": "create_node", "id": id, "parent": null, "index": 0, "kind": "block"},
        {"op": "insert_text", "id": id, "index": 0, "text": text},
    ]))
    .expect("operations parse");
    move |engine| {
        engine.initialize_page_editor().expect("page editor initializes");
        for operation in &operations {
            engine.apply_content_operation(operation).expect("operation applies");
        }
    }
}

/// Every text is in the committed document, in the stored projection state and in the stored
/// plain text, exactly once. The three are what a reader sees; a lost update is missing from all.
async fn assert_every_text_landed(state: &AppState, object_id: Uuid, document_id: Uuid, texts: &[String]) {
    let rebuilt = projection_of(&rebuild_from_database(state, document_id).await).to_string();
    let stored = stored_projection_state(state, object_id).await.to_string();
    let plain = plain_text_of(state, object_id).await;
    for text in texts {
        assert_eq!(
            rebuilt.matches(text.as_str()).count(),
            1,
            "{text} is in the document once"
        );
        assert_eq!(
            stored.matches(text.as_str()).count(),
            1,
            "{text} is in the stored state once"
        );
        assert_eq!(
            plain.matches(text.as_str()).count(),
            1,
            "{text} is in the plain text once"
        );
    }
}

struct Fixture {
    state: AppState,
    workspace_id: Uuid,
    owner_id: Uuid,
    object_id: Uuid,
    document_id: Uuid,
}

impl Fixture {
    const fn ids(&self) -> (Uuid, Uuid, Uuid) {
        (self.workspace_id, self.owner_id, self.document_id)
    }
}

async fn fixture(scratch: &super::Scratch) -> Fixture {
    fixture_with(scratch, true).await
}

/// `in_process: false` leaves the document on the real isolated worker.
async fn fixture_with(scratch: &super::Scratch, in_process: bool) -> Fixture {
    let state = state_for(scratch.db.clone());
    let (workspace_id, owner_id) = seed_workspace(&state).await;
    let (object_id, document_id) = create_page(&state, workspace_id, owner_id).await;
    if in_process {
        in_process_apply::enable(document_id);
    }
    Fixture {
        state,
        workspace_id,
        owner_id,
        object_id,
        document_id,
    }
}

fn patch_batch(start: usize, count: usize) -> Value {
    let mut operations = Vec::new();
    for offset in 0..count {
        let id = format!("b{:05}", start + offset);
        operations.push(json!({"op": "create_node", "id": id, "parent": null, "index": 0, "kind": "block"}));
        operations.push(json!({"op": "insert_text", "id": id, "index": 0, "text": format!("block {id} alpha")}));
    }
    json!({ "operations": operations })
}

async fn rest_patch(f: &Fixture, payload: Value, key: &str) -> crate::flow::model::AcceptedChange {
    execute_command(
        &f.state,
        command_input(f.object_id, f.owner_id, "semantic_patch", payload, key),
    )
    .await
    .expect("the command is accepted")
}

// ---------------------------------------------------------------------------------------------
// Counting: the work per write must not grow with the number of writes.
// ---------------------------------------------------------------------------------------------

/// Constant bound on the expensive base steps over any number of consecutive REST writes to one
/// page in one process: at most one full load from the database and one snapshot export (the
/// first write, which finds nothing cached).
const BASE_LOADS_MAX: usize = 1;
const BASE_EXPORTS_MAX: usize = 1;
const COUNTED_WRITES: usize = 12;

#[tokio::test]
async fn consecutive_rest_writes_load_and_export_the_base_a_constant_number_of_times() {
    let scratch = scratch_or_skip!("base-cache-counting");
    let f = fixture(&scratch).await;
    let head_before = head_seq_of(&f.state, f.document_id).await;

    for batch in 0..COUNTED_WRITES {
        rest_patch(&f, patch_batch(batch * 5, 5), &Uuid::new_v4().to_string()).await;
    }
    let counts = base_counters::of(f.document_id);
    assert!(
        counts.database_loads <= BASE_LOADS_MAX,
        "{COUNTED_WRITES} writes loaded the base from the database {} times (bound {BASE_LOADS_MAX})",
        counts.database_loads
    );
    assert!(
        counts.snapshot_exports <= BASE_EXPORTS_MAX,
        "{COUNTED_WRITES} writes exported the base snapshot {} times (bound {BASE_EXPORTS_MAX})",
        counts.snapshot_exports
    );
    assert_eq!(
        head_seq_of(&f.state, f.document_id).await,
        head_before + COUNTED_WRITES as i64
    );
    assert_cache_equals_database(&f.state, &runtime::runtime().cache, f.document_id).await;
    scratch.drop_self().await;
}

/// Negative control for the counting test: with the cache forced off, the same writes load and
/// export on every write, twice (once to build the update, once to apply it), which is what the
/// counting test above must (and does) refuse. This is the shape of the code before the base was
/// cached.
#[tokio::test]
async fn the_counting_test_detects_a_cold_path_that_loads_on_every_write() {
    let scratch = scratch_or_skip!("base-cache-counting-cold");
    let f = fixture(&scratch).await;
    cold_cache::enable(f.document_id);

    for batch in 0..COUNTED_WRITES {
        rest_patch(&f, patch_batch(batch * 5, 5), &Uuid::new_v4().to_string()).await;
    }
    let counts = base_counters::of(f.document_id);
    assert_eq!(
        counts.database_loads,
        2 * COUNTED_WRITES,
        "two database loads per write when cold: the build and the apply"
    );
    assert_eq!(
        counts.snapshot_exports,
        2 * COUNTED_WRITES,
        "two exports per write when cold: the build and the apply"
    );
    assert!(counts.database_loads > BASE_LOADS_MAX && counts.snapshot_exports > BASE_EXPORTS_MAX);
    scratch.drop_self().await;
}

// ---------------------------------------------------------------------------------------------
// Differential: cached path vs forced cold path.
// ---------------------------------------------------------------------------------------------

#[tokio::test]
async fn the_cached_path_and_the_forced_cold_path_agree_on_every_observable_result() {
    let scratch = scratch_or_skip!("base-cache-differential");
    let warm = fixture(&scratch).await;
    let cold = fixture(&scratch).await;
    cold_cache::enable(cold.document_id);

    // The same command sequence: block batches, a text edit, a retry of an earlier key (replay).
    let commands: Vec<(String, Value)> = vec![
        ("k1".into(), patch_batch(0, 8)),
        ("k2".into(), patch_batch(8, 8)),
        (
            "k3".into(),
            json!({"operations": [{"op": "insert_text", "id": "b00003", "index": 0, "text": "edited "}]}),
        ),
        ("k4".into(), patch_batch(16, 3)),
        (
            "k5".into(),
            json!({"operations": [{"op": "delete_text", "id": "b00001", "index": 0, "len": 3}]}),
        ),
    ];
    let mut warm_receipts = Vec::new();
    let mut cold_receipts = Vec::new();
    let mut warm_events = Vec::new();
    let mut cold_events = Vec::new();
    for (key, payload) in &commands {
        let w = rest_patch(&warm, payload.clone(), key).await;
        let c = rest_patch(&cold, payload.clone(), key).await;
        warm_receipts.push((w.accepted_seq, w.projection_seq, w.affected_object_ids.len()));
        cold_receipts.push((c.accepted_seq, c.projection_seq, c.affected_object_ids.len()));
        warm_events.push(w.event_id);
        cold_events.push(c.event_id);
    }
    // Replaying an earlier key returns the original receipt on both paths.
    let w_replay = rest_patch(&warm, commands[1].1.clone(), "k2").await;
    let c_replay = rest_patch(&cold, commands[1].1.clone(), "k2").await;
    assert_eq!(
        w_replay.event_id, warm_events[1],
        "the replay returns the original event"
    );
    assert_eq!(
        c_replay.event_id, cold_events[1],
        "the replay returns the original event"
    );

    assert_eq!(warm_receipts, cold_receipts, "receipts agree");
    let warm_head = head_seq_of(&warm.state, warm.document_id).await;
    let cold_head = head_seq_of(&cold.state, cold.document_id).await;
    assert_eq!(warm_head, cold_head, "head sequence agrees");

    let warm_doc = rebuild_from_database(&warm.state, warm.document_id).await;
    let cold_doc = rebuild_from_database(&cold.state, cold.document_id).await;
    assert_eq!(
        normalized(&projection_of(&warm_doc)),
        normalized(&projection_of(&cold_doc)),
        "committed document content agrees"
    );
    assert_eq!(
        plain_text_of(&warm.state, warm.object_id).await,
        plain_text_of(&cold.state, cold.object_id).await,
        "projection rows agree"
    );
    assert_eq!(
        normalized(&stored_projection_state(&warm.state, warm.object_id).await),
        normalized(&stored_projection_state(&cold.state, cold.object_id).await),
        "stored projection state agrees"
    );
    assert_cache_equals_database(&warm.state, &runtime::runtime().cache, warm.document_id).await;
    assert_eq!(
        base_counters::of(cold.document_id).database_loads,
        2 * commands.len(),
        "two loads per cold write: the build and the apply"
    );
    scratch.drop_self().await;
}

/// The projection with the peer-dependent container ids removed: two documents that received the
/// same commands agree on everything else (order, logical ids, text, deleted flags, properties).
fn normalized(state: &Value) -> Vec<(String, Value)> {
    let nodes = state["nodes"].as_object().expect("nodes object");
    let mut rows: Vec<(String, Value)> = nodes
        .values()
        .map(|node| {
            (
                node["order_key"].as_str().expect("order key").to_string(),
                json!([
                    node["deleted"],
                    node["kind"],
                    node["parent"],
                    node["properties"],
                    node["text"]
                ]),
            )
        })
        .collect();
    rows.sort_by(|a, b| a.0.cmp(&b.0));
    rows
}

async fn stored_projection_state(state: &AppState, object_id: Uuid) -> Value {
    #[derive(FromQueryResult)]
    struct Row {
        state: Value,
    }
    Row::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "SELECT state FROM flow_object_projections WHERE object_id = $1",
        vec![object_id.into()],
    ))
    .one(&state.db)
    .await
    .expect("projection query runs")
    .expect("projection row exists")
    .state
}

// ---------------------------------------------------------------------------------------------
// Invalidation: every way the database head can move away from an entry.
// ---------------------------------------------------------------------------------------------

#[tokio::test]
async fn a_committed_write_by_this_process_reseeds_the_entry_for_the_new_head() {
    let scratch = scratch_or_skip!("base-cache-own-write");
    let f = fixture(&scratch).await;
    let process = Process::new();
    let seq = accepted_seq(
        &process
            .write(&f.state, f.ids(), Uuid::new_v4(), title_change("one".into()))
            .await,
    );
    assert_eq!(seq, head_seq_of(&f.state, f.document_id).await);
    assert_cache_equals_database(&f.state, &process.cache, f.document_id).await;

    let loads_before = base_counters::of(f.document_id);
    process
        .write(&f.state, f.ids(), Uuid::new_v4(), title_change("two".into()))
        .await;
    assert_eq!(
        base_counters::of(f.document_id),
        loads_before,
        "the second write was built and applied on the entry the first one seeded"
    );
    assert_cache_equals_database(&f.state, &process.cache, f.document_id).await;
    scratch.drop_self().await;
}

/// Two API processes over one database. A write through A must not let B build or apply on its
/// stale entry, and B's write must include A's.
#[tokio::test]
async fn a_write_through_another_process_makes_this_processs_entry_a_miss() {
    let scratch = scratch_or_skip!("base-cache-two-processes");
    let f = fixture(&scratch).await;
    let a = Process::new();
    let b = Process::new();

    accepted_seq(
        &b.write(&f.state, f.ids(), Uuid::new_v4(), title_change("b1".into()))
            .await,
    );
    let b_head = head_of(&f.state, f.document_id).await;
    assert!(
        cached_state(&b.cache, f.document_id, &b_head).is_some(),
        "B holds the head"
    );

    // A writes: from A's point of view this is a first write (miss), and it moves the head.
    accepted_seq(
        &a.write(&f.state, f.ids(), Uuid::new_v4(), title_change("a1".into()))
            .await,
    );
    let a_head = head_of(&f.state, f.document_id).await;
    assert_ne!(a_head.0, b_head.0);
    assert!(
        cached_state(&b.cache, f.document_id, &a_head).is_none(),
        "B's entry names the old head, so it does not serve the new one"
    );

    let loads_before = base_counters::of(f.document_id).database_loads;
    accepted_seq(
        &b.write(&f.state, f.ids(), Uuid::new_v4(), title_change("b2".into()))
            .await,
    );
    assert_eq!(
        base_counters::of(f.document_id).database_loads,
        loads_before + 1,
        "B reloaded from the database instead of building on its stale entry"
    );
    let rebuilt = rebuild_from_database(&f.state, f.document_id).await;
    assert_eq!(rebuilt.title().expect("title"), "b2");
    assert_cache_equals_database(&f.state, &b.cache, f.document_id).await;
    scratch.drop_self().await;
}

/// The cache entry of one process names an old head after another process committed. An update
/// built on the new head and accepted by the first process must be applied to the new head's
/// state, not to the entry: the committed document holds every block, and the update prepared at
/// the old head cannot commit. Checked by content, not by how often the database was read.
#[tokio::test]
async fn a_cached_entry_for_an_old_head_is_never_the_base_of_a_new_head() {
    let scratch = scratch_or_skip!("base-cache-stale-base");
    let f = fixture(&scratch).await;
    let a = Process::new();
    let b = Process::new();
    accepted_seq(
        &a.write(&f.state, f.ids(), Uuid::new_v4(), block_change("blocka1", "TEXTA1"))
            .await,
    );
    let old_head = head_of(&f.state, f.document_id).await;
    assert!(
        cached_state(&a.cache, f.document_id, &old_head).is_some(),
        "A holds the head it committed"
    );

    // An update prepared by A at that head, kept to be committed later.
    let stale_update_id = Uuid::new_v4();
    let mut engine = build_engine(&f.state, &a.cache, f.document_id, stale_update_id).await;
    let frontier = engine.frontier();
    block_change("blockstale", "TEXTSTALE")(&mut engine);
    let stale_bytes = engine.export_from(&frontier).expect("exports");
    let prepared_at_old_head = match hydrate_and_apply(
        &f.state.db,
        &a.cache,
        f.document_id,
        stale_update_id,
        &stale_bytes,
        None,
    )
    .await
    .expect("hydrates")
    {
        HydrateOutcome::Prepared(prepared) => prepared,
        HydrateOutcome::Rejected(_) => panic!("a valid update prepares"),
    };
    assert_eq!(prepared_at_old_head.observed.head_seq, old_head.0);

    // Another process commits first: A's entry now names an old head.
    accepted_seq(
        &b.write(&f.state, f.ids(), Uuid::new_v4(), block_change("blockb1", "TEXTB1"))
            .await,
    );
    let new_head = head_seq_of(&f.state, f.document_id).await;
    assert_eq!(new_head, old_head.0 + 1);

    // 1. The locked phase refuses to commit what was prepared at the old head, and writes nothing.
    let epoch = authz::read_epoch(&f.state.db, f.workspace_id).await.expect("epoch");
    let request = request_for(
        f.document_id,
        stale_update_id,
        stale_bytes.clone(),
        f.workspace_id,
        f.owner_id,
        epoch,
    );
    let outcome = run_locked_phase(
        &f.state.db,
        &request,
        &prepared_at_old_head,
        10,
        crate::flow::collab::write::LOCKED_PHASE_STAGING_BUDGET,
    )
    .await;
    assert!(
        matches!(outcome, LockedOutcome::NotApplied(_)),
        "a stale prepared head must come back as a rebase, never a commit"
    );
    assert_eq!(
        head_seq_of(&f.state, f.document_id).await,
        new_head,
        "nothing was written"
    );

    // 2. A accepts an update built on the new head. It is applied to the new head's state, so the
    // committed document holds A's first block, B's block and this one.
    let mut next = rebuild_from_database(&f.state, f.document_id).await;
    let next_frontier = next.frontier();
    block_change("blocka2", "TEXTA2")(&mut next);
    let next_bytes = next.export_from(&next_frontier).expect("exports");
    accepted_seq(&a.accept_bytes(&f.state, f.ids(), Uuid::new_v4(), next_bytes).await);
    assert_every_text_landed(
        &f.state,
        f.object_id,
        f.document_id,
        &["TEXTA1".into(), "TEXTB1".into(), "TEXTA2".into()],
    )
    .await;
    assert_cache_equals_database(&f.state, &a.cache, f.document_id).await;
    scratch.drop_self().await;
}

#[tokio::test]
async fn a_head_that_moved_by_a_path_that_does_not_touch_this_cache_is_a_miss() {
    // Revert, restore, import, package ingestion, move and scope writes all advance
    // `collab_documents.head_seq` and none of them re-seeds *this* process's entry (or they seed
    // the runtime cache with their own key). From the entry's point of view they are all the same
    // event: the database head is no longer the entry's head.
    let scratch = scratch_or_skip!("base-cache-foreign-advance");
    let f = fixture(&scratch).await;
    let a = Process::new();
    let foreign = Process::new();
    accepted_seq(
        &a.write(&f.state, f.ids(), Uuid::new_v4(), title_change("a".into()))
            .await,
    );
    let before = head_of(&f.state, f.document_id).await;
    assert!(cached_state(&a.cache, f.document_id, &before).is_some());

    accepted_seq(
        &foreign
            .write(&f.state, f.ids(), Uuid::new_v4(), title_change("foreign".into()))
            .await,
    );
    let after = head_of(&f.state, f.document_id).await;
    assert!(cached_state(&a.cache, f.document_id, &after).is_none());

    a.write(&f.state, f.ids(), Uuid::new_v4(), title_change("a2".into()))
        .await;
    assert_cache_equals_database(&f.state, &a.cache, f.document_id).await;
    assert_eq!(
        rebuild_from_database(&f.state, f.document_id)
            .await
            .title()
            .expect("title"),
        "a2"
    );
    scratch.drop_self().await;
}

#[tokio::test]
async fn a_rewritten_frontier_at_the_same_sequence_is_a_miss() {
    // A document re-created or restored from a backup can reuse a sequence number. The entry's
    // frontier no longer equals the stored one, so it must not be served.
    let scratch = scratch_or_skip!("base-cache-frontier-rewrite");
    let f = fixture(&scratch).await;
    let a = Process::new();
    accepted_seq(
        &a.write(&f.state, f.ids(), Uuid::new_v4(), title_change("a".into()))
            .await,
    );
    let head = head_of(&f.state, f.document_id).await;
    assert!(cached_state(&a.cache, f.document_id, &head).is_some());

    let mut other_frontier = head.1.clone();
    other_frontier.push(0);
    let rewritten = (head.0, other_frontier, head.2.clone());
    assert!(
        cached_state(&a.cache, f.document_id, &rewritten).is_none(),
        "same sequence, different frontier: miss"
    );
    scratch.drop_self().await;
}

#[tokio::test]
async fn a_snapshot_rewrite_keeps_the_entry_valid_because_the_state_is_the_same() {
    let scratch = scratch_or_skip!("base-cache-snapshot-advance");
    let f = fixture(&scratch).await;
    let a = Process::new();
    for i in 0..3 {
        accepted_seq(
            &a.write(&f.state, f.ids(), Uuid::new_v4(), title_change(format!("t{i}")))
                .await,
        );
    }
    let advancer = SnapshotAdvancer::new();
    crate::flow::collab::snapshot::advance(&f.state.db, f.document_id, &advancer)
        .await
        .expect("snapshot advances");
    assert_cache_equals_database(&f.state, &a.cache, f.document_id).await;

    let loads_before = base_counters::of(f.document_id).database_loads;
    a.write(&f.state, f.ids(), Uuid::new_v4(), title_change("after".into()))
        .await;
    assert_eq!(base_counters::of(f.document_id).database_loads, loads_before);
    assert_cache_equals_database(&f.state, &a.cache, f.document_id).await;
    scratch.drop_self().await;
}

#[tokio::test]
async fn an_epoch_change_refuses_the_write_and_leaves_cache_and_database_in_step() {
    let scratch = scratch_or_skip!("base-cache-epoch");
    let f = fixture(&scratch).await;
    let a = Process::new();
    accepted_seq(
        &a.write(&f.state, f.ids(), Uuid::new_v4(), title_change("a".into()))
            .await,
    );

    // Build with the epoch read now, then move the epoch before the commit.
    let update_id = Uuid::new_v4();
    let stale_epoch = authz::read_epoch(&f.state.db, f.workspace_id).await.expect("epoch");
    f.state
        .db
        .execute(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "UPDATE flow_workspace_settings SET authz_epoch = authz_epoch + 1 WHERE workspace_id = $1",
            vec![f.workspace_id.into()],
        ))
        .await
        .expect("epoch moves");
    let mut engine = rebuild_from_database(&f.state, f.document_id).await;
    let frontier = engine.frontier();
    engine.set_title("refused").expect("title");
    let bytes = engine.export_from(&frontier).expect("exports");
    let head_before = head_seq_of(&f.state, f.document_id).await;
    let outcome = accept_update(
        &f.state.db,
        &a.cache,
        &a.coordinator,
        &a.registry,
        &a.snapshot,
        10,
        None,
        request_for(f.document_id, update_id, bytes, f.workspace_id, f.owner_id, stale_epoch),
    )
    .await
    .expect("no hard error");
    assert!(
        matches!(outcome, AcceptOutcome::Rejected(_)),
        "the stale epoch is refused"
    );
    assert_eq!(head_seq_of(&f.state, f.document_id).await, head_before);
    assert_cache_equals_database(&f.state, &a.cache, f.document_id).await;
    scratch.drop_self().await;
}

#[tokio::test]
async fn an_evicted_entry_is_rebuilt_from_the_database_and_the_write_is_correct() {
    let scratch = scratch_or_skip!("base-cache-eviction");
    let f = fixture(&scratch).await;
    let a = Process::new();
    accepted_seq(
        &a.write(&f.state, f.ids(), Uuid::new_v4(), title_change("a".into()))
            .await,
    );
    for _ in 0..=crate::flow::collab::limits::WARM_CACHE_DOCUMENTS_MAX {
        a.cache.put(
            Uuid::new_v4(),
            LoroCollabEngine::new_empty(1),
            "loro-1".to_string(),
            0,
            vec![],
            1,
        );
        std::thread::sleep(std::time::Duration::from_micros(50));
    }
    assert!(!a.cache.contains(f.document_id), "pressure evicted the entry");

    let loads_before = base_counters::of(f.document_id).database_loads;
    a.write(&f.state, f.ids(), Uuid::new_v4(), title_change("b".into()))
        .await;
    assert_eq!(base_counters::of(f.document_id).database_loads, loads_before + 1);
    assert_cache_equals_database(&f.state, &a.cache, f.document_id).await;
    scratch.drop_self().await;
}

#[tokio::test]
async fn invalidate_makes_the_next_write_load_from_the_database() {
    let scratch = scratch_or_skip!("base-cache-invalidate");
    let f = fixture(&scratch).await;
    let a = Process::new();
    accepted_seq(
        &a.write(&f.state, f.ids(), Uuid::new_v4(), title_change("a".into()))
            .await,
    );
    a.cache.invalidate(f.document_id);
    assert_eq!(a.cache.total_bytes(), 0);
    let loads_before = base_counters::of(f.document_id).database_loads;
    a.write(&f.state, f.ids(), Uuid::new_v4(), title_change("b".into()))
        .await;
    assert_eq!(base_counters::of(f.document_id).database_loads, loads_before + 1);
    scratch.drop_self().await;
}

// ---------------------------------------------------------------------------------------------
// Idempotency and races.
// ---------------------------------------------------------------------------------------------

#[tokio::test]
async fn a_replayed_update_id_returns_the_original_receipt_without_a_second_row() {
    let scratch = scratch_or_skip!("base-cache-replay");
    let f = fixture(&scratch).await;
    let a = Process::new();
    let update_id = Uuid::new_v4();
    let first = accepted_seq(&a.write(&f.state, f.ids(), update_id, title_change("once".into())).await);
    let again = accepted_seq(&a.write(&f.state, f.ids(), update_id, title_change("once".into())).await);
    assert_eq!(first, again);
    assert_eq!(head_seq_of(&f.state, f.document_id).await, first);
    assert_eq!(super::count_collab_updates(&f.state, f.document_id, update_id).await, 1);
    scratch.drop_self().await;
}

const RACE_ROUNDS: usize = 20;
const WRITERS_PER_PROCESS: usize = 3;

/// Two processes, several concurrent writers each, on one page, each inserting a block with its own
/// text: every accepted write holds a unique, gap-free sequence, the head advances by exactly the
/// number of accepted writes, every block is in the committed document and in the stored
/// projection (a lost update would be a missing block), and both caches end up equal to the
/// database. Repeated with a fresh document each round.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_writers_in_two_processes_lose_nothing_and_never_duplicate_a_sequence() {
    let scratch = scratch_or_skip!("base-cache-race");
    for round in 0..RACE_ROUNDS {
        let f = Arc::new(fixture(&scratch).await);
        let processes = [Arc::new(Process::new()), Arc::new(Process::new())];
        let head_before = head_seq_of(&f.state, f.document_id).await;

        let mut tasks = Vec::new();
        for (p, process) in processes.iter().enumerate() {
            for w in 0..WRITERS_PER_PROCESS {
                let (process, f) = (process.clone(), f.clone());
                tasks.push(tokio::spawn(async move {
                    // A client retries a contention refusal under the same update identity.
                    let update_id = Uuid::new_v4();
                    let mut outcome;
                    let mut attempts = 0;
                    loop {
                        attempts += 1;
                        outcome = process
                            .write(
                                &f.state,
                                f.ids(),
                                update_id,
                                block_change(&format!("blk-{p}-{w}"), &format!("ROUND{round:02}P{p}W{w}")),
                            )
                            .await;
                        let contended = matches!(
                            &outcome,
                            AcceptOutcome::Rejected(r) if r.code == RejectedCode::ServerDraining && r.recoverable
                        );
                        if !contended || attempts >= 20 {
                            break;
                        }
                    }
                    (update_id, outcome)
                }));
            }
        }
        let mut accepted = Vec::new();
        for task in tasks {
            let (update_id, outcome) = task.await.expect("writer joins");
            match outcome {
                AcceptOutcome::Accepted(a) => accepted.push((update_id, a.head_seq)),
                AcceptOutcome::Rejected(r) => panic!("round {round}: a writer was refused: {r:?}"),
            }
        }
        let total = 2 * WRITERS_PER_PROCESS;
        let mut seqs: Vec<i64> = accepted.iter().map(|(_, seq)| *seq).collect();
        seqs.sort_unstable();
        let expected: Vec<i64> = (1..=total as i64).map(|n| head_before + n).collect();
        assert_eq!(seqs, expected, "round {round}: unique, gap-free sequences");
        assert_eq!(
            head_seq_of(&f.state, f.document_id).await,
            head_before + total as i64,
            "round {round}: head advanced by exactly the accepted writes"
        );
        for (update_id, seq) in &accepted {
            assert_eq!(
                super::count_collab_updates(&f.state, f.document_id, *update_id).await,
                1
            );
            assert_eq!(super::update_id_at_seq(&f.state, f.document_id, *seq).await, *update_id);
        }
        let texts: Vec<String> = (0..2)
            .flat_map(|p| (0..WRITERS_PER_PROCESS).map(move |w| format!("ROUND{round:02}P{p}W{w}")))
            .collect();
        assert_every_text_landed(&f.state, f.object_id, f.document_id, &texts).await;
        // Each process's entry, if it names the head, equals the database; otherwise it is a miss.
        let head = head_of(&f.state, f.document_id).await;
        let rebuilt = projection_of(&rebuild_from_database(&f.state, f.document_id).await);
        for process in &processes {
            if let Some((cached, _)) = cached_state(&process.cache, f.document_id, &head) {
                assert_eq!(cached, rebuilt, "round {round}: a served entry equals the database");
            }
        }
    }
    scratch.drop_self().await;
}

/// The REST path, concurrently, on one page in one process: every distinct command lands.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_rest_commands_on_one_page_all_land_exactly_once() {
    let scratch = scratch_or_skip!("base-cache-rest-race");
    let f = Arc::new(fixture(&scratch).await);
    let head_before = head_seq_of(&f.state, f.document_id).await;
    let writers = 8usize;
    let mut tasks = Vec::new();
    for w in 0..writers {
        let f = f.clone();
        tasks.push(tokio::spawn(async move {
            let id = format!("race-{w}");
            execute_command(
                &f.state,
                command_input(
                    f.object_id,
                    f.owner_id,
                    "insert_block",
                    json!({"block_id": id, "index": 0, "text": format!("RACETEXT{w}")}),
                    &Uuid::new_v4().to_string(),
                ),
            )
            .await
            .map(|change| change.accepted_seq)
        }));
    }
    let mut seqs = Vec::new();
    for task in tasks {
        seqs.push(task.await.expect("joins").expect("the command is accepted"));
    }
    seqs.sort_unstable();
    let expected: Vec<i64> = (1..=writers as i64).map(|n| head_before + n).collect();
    assert_eq!(seqs, expected);
    let texts: Vec<String> = (0..writers).map(|w| format!("RACETEXT{w}")).collect();
    assert_every_text_landed(&f.state, f.object_id, f.document_id, &texts).await;
    assert_cache_equals_database(&f.state, &runtime::runtime().cache, f.document_id).await;
    scratch.drop_self().await;
}

/// The WebSocket-style accept path (no held base) shares the entry a REST-style write seeded, and
/// seeds the next one.
#[tokio::test]
async fn an_accept_without_a_held_base_uses_and_reseeds_the_same_entry() {
    let scratch = scratch_or_skip!("base-cache-ws-style");
    let f = fixture(&scratch).await;
    let a = Process::new();
    accepted_seq(
        &a.write(&f.state, f.ids(), Uuid::new_v4(), title_change("rest".into()))
            .await,
    );

    let mut client = rebuild_from_database(&f.state, f.document_id).await;
    let frontier = client.frontier();
    client.set_title("ws").expect("title");
    let bytes = client.export_from(&frontier).expect("exports");
    let loads_before = base_counters::of(f.document_id);
    accepted_seq(&a.accept_bytes(&f.state, f.ids(), Uuid::new_v4(), bytes).await);
    assert_eq!(
        base_counters::of(f.document_id),
        loads_before,
        "no load or export for the second write"
    );
    assert_cache_equals_database(&f.state, &a.cache, f.document_id).await;
    scratch.drop_self().await;
}

// ---------------------------------------------------------------------------------------------
// What is cached must be what was committed.
// ---------------------------------------------------------------------------------------------

/// A lost `COMMIT` response is recovered by finding the row under the write's `update_id`. When a
/// client retried the same logical write against another process that committed it, that row is
/// the other process's commit over a base this process's candidate does not share. Caching the
/// candidate under that row's head would make the next write here build on a state the database
/// does not have; the entry must be dropped instead, and the next write must contain the other
/// process's block and not this process's.
#[tokio::test]
async fn recovering_a_commit_that_another_process_made_never_caches_the_local_candidate() {
    let scratch = scratch_or_skip!("base-cache-commit-unknown-foreign");
    let f = fixture(&scratch).await;
    let p = Process::new();
    let q = Process::new();
    accepted_seq(
        &p.write(&f.state, f.ids(), Uuid::new_v4(), block_change("blkseed", "TEXTSEED"))
            .await,
    );

    // P prepares its update for logical write `x` on the head it sees.
    let x = Uuid::new_v4();
    let mut engine = build_engine(&f.state, &p.cache, f.document_id, x).await;
    let frontier = engine.frontier();
    block_change("blkp", "TEXTP")(&mut engine);
    let bytes_p = engine.export_from(&frontier).expect("exports");
    let prepared = match hydrate_and_apply(&f.state.db, &p.cache, f.document_id, x, &bytes_p, None)
        .await
        .expect("hydrates")
    {
        HydrateOutcome::Prepared(prepared) => prepared,
        HydrateOutcome::Rejected(_) => panic!("a valid update prepares"),
    };

    // The retry reached Q, which commits `x` with its own content at the next sequence.
    let q_seq = accepted_seq(&q.write(&f.state, f.ids(), x, block_change("blkq", "TEXTQ")).await);
    assert_eq!(q_seq, prepared.observed.head_seq + 1);

    // P's own COMMIT returned an error; its recovery finds Q's row under `x`.
    let prior = find_prior_update(&f.state.db, f.document_id, x)
        .await
        .expect("lookup runs")
        .expect("the row exists");
    let epoch = authz::read_epoch(&f.state.db, f.workspace_id).await.expect("epoch");
    let request = request_for(f.document_id, x, bytes_p, f.workspace_id, f.owner_id, epoch);
    finish_committed(&p.cache, &p.registry, None, &request, prepared, prior, false);

    let head = head_of(&f.state, f.document_id).await;
    assert!(
        cached_state(&p.cache, f.document_id, &head).is_none(),
        "P's candidate is not Q's committed state and must not be served at Q's head"
    );
    accepted_seq(
        &p.write(&f.state, f.ids(), Uuid::new_v4(), block_change("blknext", "TEXTNEXT"))
            .await,
    );
    assert_every_text_landed(
        &f.state,
        f.object_id,
        f.document_id,
        &["TEXTSEED".into(), "TEXTQ".into(), "TEXTNEXT".into()],
    )
    .await;
    assert!(
        !projection_of(&rebuild_from_database(&f.state, f.document_id).await)
            .to_string()
            .contains("TEXTP"),
        "P's update was never committed"
    );
    assert_cache_equals_database(&f.state, &p.cache, f.document_id).await;
    scratch.drop_self().await;
}

/// The recovery's normal case still seeds the cache: the row found is this write's own commit.
#[tokio::test]
async fn recovering_a_commit_this_process_made_still_seeds_the_cache() {
    let scratch = scratch_or_skip!("base-cache-commit-unknown-own");
    let f = fixture(&scratch).await;
    let p = Process::new();
    let x = Uuid::new_v4();
    let mut engine = build_engine(&f.state, &p.cache, f.document_id, x).await;
    let frontier = engine.frontier();
    block_change("blkp", "TEXTP")(&mut engine);
    let bytes = engine.export_from(&frontier).expect("exports");
    let prepared = match hydrate_and_apply(&f.state.db, &p.cache, f.document_id, x, &bytes, None)
        .await
        .expect("hydrates")
    {
        HydrateOutcome::Prepared(prepared) => prepared,
        HydrateOutcome::Rejected(_) => panic!("a valid update prepares"),
    };
    let epoch = authz::read_epoch(&f.state.db, f.workspace_id).await.expect("epoch");
    let request = request_for(f.document_id, x, bytes, f.workspace_id, f.owner_id, epoch);
    let outcome = run_locked_phase(
        &f.state.db,
        &request,
        &prepared,
        10,
        crate::flow::collab::write::LOCKED_PHASE_STAGING_BUDGET,
    )
    .await;
    assert!(matches!(outcome, LockedOutcome::Committed(_)), "the write commits");
    // The response of the commit is treated as lost: the row is found by its update id.
    let prior = find_prior_update(&f.state.db, f.document_id, x)
        .await
        .expect("lookup runs")
        .expect("the row exists");
    finish_committed(&p.cache, &p.registry, None, &request, prepared, prior, false);
    assert_cache_equals_database(&f.state, &p.cache, f.document_id).await;
    assert_every_text_landed(&f.state, f.object_id, f.document_id, &["TEXTP".into()]).await;
    scratch.drop_self().await;
}

/// `observe_head` and the bootstrap load are two reads. When another process commits between
/// them, the loaded state belongs to the newer head: it is used for this write and never cached
/// under the head that was observed.
#[tokio::test]
async fn a_state_loaded_at_a_newer_head_than_observed_is_never_cached_under_the_old_head() {
    let scratch = scratch_or_skip!("base-cache-load-race");
    let f = fixture(&scratch).await;
    let b = Process::new();
    accepted_seq(
        &b.write(&f.state, f.ids(), Uuid::new_v4(), block_change("blkone", "TEXTONE"))
            .await,
    );
    let observed = read_observed_head(&f.state.db, f.document_id)
        .await
        .expect("head reads")
        .expect("document exists");
    let old_head = (
        observed.head_seq,
        observed.head_frontier.clone(),
        observed.format_version.clone(),
    );

    // Another process commits after the head was observed and before the load.
    accepted_seq(
        &b.write(&f.state, f.ids(), Uuid::new_v4(), block_change("blktwo", "TEXTTWO"))
            .await,
    );

    let cache = WarmCache::new();
    let update_id = Uuid::new_v4();
    for want_engine in [false, true] {
        let observed = read_observed_head(&f.state.db, f.document_id)
            .await
            .expect("head reads")
            .expect("document exists");
        // Re-create the stale observation: the head as it was before the second commit.
        let stale = crate::flow::collab::write::ObservedHead {
            head_seq: old_head.0,
            head_frontier: old_head.1.clone(),
            format_version: old_head.2.clone(),
            ..observed
        };
        let resolved = match resolve_base(&f.state.db, &cache, f.document_id, update_id, stale, want_engine)
            .await
            .expect("base resolves")
        {
            BaseOutcome::Ready(resolved) => resolved,
            BaseOutcome::Rejected(_) => panic!("a base is expected"),
        };
        let loaded = LoroCollabEngine::load(&resolved.snapshot_bytes).expect("bytes decode");
        assert!(
            projection_of(&loaded).to_string().contains("TEXTTWO"),
            "the loaded state is the newer head's"
        );
        assert!(
            cached_state(&cache, f.document_id, &old_head).is_none(),
            "the newer state must not be served as the older head (want_engine={want_engine})"
        );
        assert!(!cache.contains(f.document_id), "nothing was seeded");
    }
    scratch.drop_self().await;
}

/// Paths such as `scope_write`, `scope_move` and page transfers advance the head and put their own
/// engine into the cache *without* base bytes. The entry they replace carried the bytes of the
/// previous head; those must go with it, or the next write would send the worker a base one head
/// behind the database.
#[tokio::test]
async fn an_entry_put_without_bytes_over_one_with_bytes_never_serves_the_old_bytes() {
    let scratch = scratch_or_skip!("base-cache-put-without-bytes");
    let f = fixture(&scratch).await;
    let a = Process::new();
    let b = Process::new();
    accepted_seq(
        &a.write(&f.state, f.ids(), Uuid::new_v4(), block_change("blka1", "TEXTA1"))
            .await,
    );
    let before = head_of(&f.state, f.document_id).await;
    assert!(
        cached_state(&a.cache, f.document_id, &before).is_some(),
        "A holds an entry with bytes"
    );

    accepted_seq(
        &b.write(&f.state, f.ids(), Uuid::new_v4(), block_change("blkb1", "TEXTB1"))
            .await,
    );
    // The foreign path caches the committed engine at the new head, without bytes.
    let head = head_of(&f.state, f.document_id).await;
    let rebuilt = rebuild_from_database(&f.state, f.document_id).await;
    a.cache
        .put(f.document_id, rebuilt, head.2.clone(), head.0, head.1.clone(), 1);
    assert!(
        cached_state(&a.cache, f.document_id, &head).is_none(),
        "the replaced entry holds no bytes"
    );

    accepted_seq(
        &a.write(&f.state, f.ids(), Uuid::new_v4(), block_change("blka2", "TEXTA2"))
            .await,
    );
    assert_every_text_landed(
        &f.state,
        f.object_id,
        f.document_id,
        &["TEXTA1".into(), "TEXTB1".into(), "TEXTA2".into()],
    )
    .await;
    assert_cache_equals_database(&f.state, &a.cache, f.document_id).await;
    scratch.drop_self().await;
}

// ---------------------------------------------------------------------------------------------
// The REST build holds no lock.
// ---------------------------------------------------------------------------------------------

/// A command that fails while it is built never reaches the accept, so it must never wait for the
/// document's coordinator permit: with another writer holding the permit it is refused with its
/// own error at once, not with `contention` after the acquire timeout.
#[tokio::test]
async fn a_failing_rest_command_does_not_wait_for_the_document_permit() {
    let scratch = scratch_or_skip!("base-cache-failing-command-no-permit");
    let f = fixture(&scratch).await;
    let held = runtime::runtime()
        .coordinator
        .acquire(f.document_id)
        .await
        .expect("the permit is free");
    let started = std::time::Instant::now();
    let outcome = execute_command(
        &f.state,
        command_input(
            f.object_id,
            f.owner_id,
            "delete_block",
            json!({"block_id": "no-such-block"}),
            &Uuid::new_v4().to_string(),
        ),
    )
    .await;
    let Err(error) = outcome else {
        panic!("deleting a block that does not exist must fail");
    };
    let waited = started.elapsed();
    assert!(
        !matches!(error.kind(), crate::error::ApiErrorKind::ServerDraining(_)),
        "the failure is the command's own, not contention: {error:?}"
    );
    assert!(
        waited < std::time::Duration::from_millis(crate::flow::collab::limits::COORDINATOR_ACQUIRE_TIMEOUT_MS),
        "the failing command did not queue behind the permit (took {waited:?})"
    );
    drop(held);
    scratch.drop_self().await;
}

/// A successful REST write queues for the permit only inside the accept: with the permit held for a
/// moment it still lands once the permit is released.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_rest_write_waits_for_the_permit_only_to_accept_and_then_lands() {
    let scratch = scratch_or_skip!("base-cache-permit-wait");
    let f = Arc::new(fixture(&scratch).await);
    let held = runtime::runtime()
        .coordinator
        .acquire(f.document_id)
        .await
        .expect("the permit is free");
    let writer = {
        let f = f.clone();
        tokio::spawn(async move {
            execute_command(
                &f.state,
                command_input(
                    f.object_id,
                    f.owner_id,
                    "insert_block",
                    json!({"block_id": "waited", "index": 0, "text": "TEXTWAITED"}),
                    &Uuid::new_v4().to_string(),
                ),
            )
            .await
        })
    };
    tokio::time::sleep(std::time::Duration::from_millis(150)).await;
    drop(held);
    writer.await.expect("joins").expect("the command is accepted");
    assert_every_text_landed(&f.state, f.object_id, f.document_id, &["TEXTWAITED".into()]).await;
    scratch.drop_self().await;
}

// ---------------------------------------------------------------------------------------------
// Through the real isolated worker (Linux).
// ---------------------------------------------------------------------------------------------

/// No stand-in: the document is applied by the real isolated worker, whose returned snapshot is
/// what the cache keeps as the next write's base. After every write the cached engine and the
/// cached bytes decode to what the database rebuilds, at the database head frontier, and every
/// block written so far is in the stored projection. Needs Linux, like every write-path test that
/// does not register the stand-in (filter: `real_worker_`).
#[tokio::test]
async fn real_worker_consecutive_rest_writes_keep_the_cached_bytes_equal_to_the_database() {
    let scratch = scratch_or_skip!("base-cache-real-worker");
    let f = fixture_with(&scratch, false).await;
    let writes = 4usize;
    let mut texts = Vec::new();
    for write in 0..writes {
        let text = format!("REALWORKER{write}");
        let change = rest_patch(
            &f,
            json!({"operations": [
                {"op": "create_node", "id": format!("rw{write}"), "parent": null, "index": 0, "kind": "block"},
                {"op": "insert_text", "id": format!("rw{write}"), "index": 0, "text": text},
            ]}),
            &Uuid::new_v4().to_string(),
        )
        .await;
        texts.push(text);
        assert_eq!(change.accepted_seq, head_seq_of(&f.state, f.document_id).await);
        assert_cache_equals_database(&f.state, &runtime::runtime().cache, f.document_id).await;
        assert_every_text_landed(&f.state, f.object_id, f.document_id, &texts).await;
    }
    let counts = base_counters::of(f.document_id);
    assert!(
        counts.database_loads <= BASE_LOADS_MAX && counts.snapshot_exports <= BASE_EXPORTS_MAX,
        "{writes} writes through the real worker loaded {} / exported {} times",
        counts.database_loads,
        counts.snapshot_exports
    );
    scratch.drop_self().await;
}
