//! The update-scoped downgrade guard against the algorithm it replaced.
//!
//! `reference_import` is the previous `import_update*` verbatim (floor walk before, import, floor
//! walk after, refuse a fall from 5). Every update in the tests below goes through both, and the
//! decisions must agree. The guard's cost is checked by counting work, not by timing.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::print_stdout
)]

use std::collections::BTreeMap;

use loro::{Container, ContainerTrait, ExportMode, LoroDoc, LoroList, LoroMap, LoroValue, ValueOrContainer};
use serde_json::Value;

use super::{CollabEngine, LoroCollabEngine, model5_marker};
use crate::error::{CollabError, InputLimits};
use crate::frontier::Frontier;
use crate::operation::{NodeId, NodeKind, Operation};
use crate::walk_counter;

const ASSET: &str = "00000000-0000-4000-8000-000000000001";

fn image_attrs() -> Value {
    serde_json::json!({"asset_id": ASSET, "alt": "", "caption": "", "indent": 0})
}

struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
    fn below(&mut self, n: u64) -> u64 {
        self.next() % n.max(1)
    }
    fn index(&mut self, len: usize) -> usize {
        usize::try_from(self.below(len as u64)).unwrap()
    }
}

/// The importer as it was before the update-scoped guard: a walk before, a walk after.
/// Runs on a fork, so the engine under test is untouched. Returns the post-import engine.
fn reference_import(engine: &LoroCollabEngine, update: &[u8], causal: bool) -> Result<LoroCollabEngine, CollabError> {
    let mut fork = engine.fork()?;
    let prior_model = fork.minimum_editor_model()?;
    InputLimits::default().validate_update(update)?;
    fork.invalidate_editor_model_floor();
    let status = fork.doc.import(update).map_err(|e| CollabError::DecodeFailed {
        input: "update",
        reason: e.to_string(),
    })?;
    if causal && status.pending.is_some() {
        return Err(CollabError::CausalDependencyMissing);
    }
    fork.rebuild_id_cache()?;
    if prior_model >= 5 && fork.minimum_editor_model()? < 5 {
        return Err(CollabError::OperationFailed {
            reason: "editor_model_floor_downgrade".into(),
        });
    }
    Ok(fork)
}

fn is_downgrade<T>(result: &Result<T, CollabError>) -> bool {
    matches!(result, Err(CollabError::OperationFailed { reason }) if reason == "editor_model_floor_downgrade")
}

/// Which branch of the guard decided an import.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Arm {
    /// The marker is set after the import: nothing can be refused.
    MarkerSetAfter,
    /// No marker before, no suspect op: accepted from the ops alone.
    NoSuspectOp,
    /// The marker was set before and is gone: the narrow scan decides.
    MarkerCleared,
    /// A suspect op, but an atom block map is still present.
    SuspectAtomRemains,
    /// A suspect op and no atom left: the pre-import state decides.
    SuspectBaseRead,
}

struct Step {
    arm: Arm,
    accepted: bool,
    downgrade: bool,
}

/// Runs `update` through the guard and the reference and asserts that they decide alike. The
/// base must be valid (it passes the floor walk). Returns what happened. Leaves `engine` holding
/// the imported state exactly as the importer does, accepted or not.
fn step(engine: &mut LoroCollabEngine, update: &[u8], causal: bool, label: &str) -> Step {
    assert!(
        engine.compute_minimum_editor_model().is_ok(),
        "{label}: the base must be valid"
    );
    let reference = reference_import(engine, update, causal);
    let marker_before = model5_marker(&engine.doc);
    walk_counter::reset();
    let new = if causal {
        engine.import_update_causally_complete(update)
    } else {
        engine.import_update(update)
    };
    let (_, narrow, historical) = walk_counter::guard_counts();
    let (floor_walks, block_reads) = walk_counter::counts();
    assert_eq!(
        (floor_walks, block_reads),
        (0, 0),
        "{label}: the guard walked the document"
    );
    let marker_after = model5_marker(&engine.doc);

    // Decision equivalence. An update that leaves an unreadable document is refused by the
    // projection that follows every import in the worker, so "accepted" is import + projection.
    let reference_state_ok = reference
        .as_ref()
        .is_ok_and(|post| post.semantic_snapshot_with_floor().is_ok());
    let new_state_ok = new.is_ok() && engine.semantic_snapshot_with_floor().is_ok();
    assert_eq!(
        reference.is_ok() && reference_state_ok,
        new_state_ok,
        "{label}: accept/refuse differs (reference {:?}, guard {new:?})",
        reference.as_ref().map(|_| ())
    );
    let reference_err = reference.as_ref().err();
    if reference_err.is_some_and(
        |error| matches!(error, CollabError::OperationFailed { reason } if reason == "editor_model_floor_downgrade"),
    ) {
        assert!(is_downgrade(&new), "{label}: the guard missed a downgrade, got {new:?}");
    }
    if is_downgrade(&new) {
        assert!(
            reference.is_err(),
            "{label}: the guard refused what the reference accepted"
        );
    }

    let arm = if marker_after {
        Arm::MarkerSetAfter
    } else if historical > 0 {
        Arm::SuspectBaseRead
    } else if narrow > 0 && marker_before {
        Arm::MarkerCleared
    } else if narrow > 0 {
        Arm::SuspectAtomRemains
    } else {
        Arm::NoSuspectOp
    };
    Step {
        arm,
        accepted: new_state_ok,
        downgrade: is_downgrade(&new),
    }
}

fn block(engine: &mut LoroCollabEngine, id: &str, index: u32) {
    engine
        .apply_content_operation(&Operation::CreateNode {
            id: NodeId::from(id),
            parent: None,
            index,
            kind: NodeKind::Block,
        })
        .unwrap();
}

/// A page at editor floor 1 (flat), 4 (structured) or 5 (structured, marker and an image).
fn page(floor: u32, blocks: usize, seed: u64) -> LoroCollabEngine {
    let mut engine = LoroCollabEngine::new_empty(seed);
    engine.initialize_page_editor().unwrap();
    if floor >= 4 {
        engine.initialize_page_structure_model(floor).unwrap();
    }
    if floor >= 5 {
        engine
            .insert_page_atom("seed-image", None, 0, "image", &image_attrs())
            .unwrap();
    }
    for index in 0..blocks {
        block(&mut engine, &format!("s{index}"), 0);
    }
    assert_eq!(engine.minimum_editor_model().unwrap(), floor);
    engine
}

fn children(doc: &LoroDoc) -> LoroList {
    match doc.get_map("doc").get("children") {
        Some(ValueOrContainer::Container(Container::List(list))) => list,
        _ => panic!("no root list"),
    }
}

fn node_name(map: &LoroMap) -> Option<String> {
    match map.get("nodeName") {
        Some(ValueOrContainer::Value(LoroValue::String(kind))) => Some(kind.to_string()),
        _ => None,
    }
}

/// Indices in `doc.children` whose block map has an atom (`atom`) or non-atom (`!atom`) kind.
fn indices_of(doc: &LoroDoc, atom: bool) -> Vec<usize> {
    let list = children(doc);
    (0..list.len())
        .filter(|&index| match list.get(index) {
            Some(ValueOrContainer::Container(Container::Map(map))) => {
                node_name(&map).is_some_and(|kind| crate::editor_media::is_atom(&kind) == atom)
            }
            _ => false,
        })
        .collect()
}

fn map_at(doc: &LoroDoc, index: usize) -> LoroMap {
    match children(doc).get(index) {
        Some(ValueOrContainer::Container(Container::Map(map))) => map,
        _ => panic!("no block map at {index}"),
    }
}

/// An update authored by a raw Loro peer (not an engine) on top of `engine`'s current state.
fn raw_update(engine: &LoroCollabEngine, peer: u64, edit: impl FnOnce(&LoroDoc)) -> Vec<u8> {
    let raw = LoroDoc::new();
    raw.import(&engine.export_snapshot().unwrap()).unwrap();
    raw.set_peer_id(peer).unwrap();
    let before = raw.oplog_vv();
    edit(&raw);
    raw.commit();
    raw.export(ExportMode::updates(&before)).unwrap()
}

/// Appends a raw atom block map (no placement, no marker): valid on a flat page, where it alone
/// holds the floor at 5.
fn push_raw_atom(raw: &LoroDoc, kind: &str) -> loro::ContainerID {
    let list = children(raw);
    let map = list.insert_container(list.len(), LoroMap::new()).unwrap();
    map.insert("nodeName", kind).unwrap();
    let attrs = map.insert_container("attributes", LoroMap::new()).unwrap();
    let key = if kind == "childPage" {
        "child_page_id"
    } else {
        "asset_id"
    };
    attrs.insert(key, ASSET).unwrap();
    map.insert_container("children", LoroList::new()).unwrap();
    map.id()
}

fn set_model(raw: &LoroDoc, model: &str) {
    raw.get_map(crate::editor_structure::META)
        .insert("model", model)
        .unwrap();
}

/// A flat page whose only floor-5 support is a raw atom block map (no marker).
fn flat_page_with_raw_atom(seed: u64) -> LoroCollabEngine {
    let mut engine = page(1, 6, seed);
    let update = raw_update(&engine, 50_000 + seed, |raw| {
        push_raw_atom(raw, "image");
    });
    engine.import_update(&update).unwrap();
    assert_eq!(engine.minimum_editor_model().unwrap(), 5);
    assert!(!model5_marker(&engine.doc));
    engine
}

#[derive(Default)]
struct Tally {
    arms: BTreeMap<(Arm, bool), usize>,
    downgrades: usize,
    accepted: usize,
    refused: usize,
    floors: BTreeMap<u32, usize>,
}

impl Tally {
    fn record(&mut self, step: &Step) {
        *self.arms.entry((step.arm, step.accepted)).or_default() += 1;
        self.downgrades += usize::from(step.downgrade);
        if step.accepted {
            self.accepted += 1;
        } else {
            self.refused += 1;
        }
    }
}

/// Runs one update through `step`; if it leaves an unreadable document the engine is restored
/// to the accepted state it had (the worker discards such an engine).
fn checked_step(engine: &mut LoroCollabEngine, update: &[u8], causal: bool, label: &str, tally: &mut Tally) {
    let backup = engine.fork().unwrap();
    let outcome = step(engine, update, causal, label);
    tally.record(&outcome);
    // The fused floor must equal the walk on every state, or both must fail.
    let fused = engine.semantic_snapshot_with_floor();
    let walk = engine.compute_minimum_editor_model();
    match (&fused, &walk) {
        (Ok((_, floor)), Ok(walk_floor)) => {
            assert_eq!(floor, walk_floor, "{label}: fused floor differs from the walk");
            *tally.floors.entry(*floor).or_default() += 1;
        }
        (Err(_), Err(_)) => {}
        _ => panic!("{label}: fused {:?} vs walk {walk:?}", fused.as_ref().map(|(_, f)| f)),
    }
    if walk.is_err() || fused.is_err() || (!outcome.accepted && !outcome.downgrade) {
        *engine = backup;
    }
}

/// One random edit by a client engine, the way a browser or agent would author it.
#[allow(clippy::too_many_lines)]
fn client_edit(
    client: &mut LoroCollabEngine,
    rng: &mut Rng,
    ids: &mut Vec<String>,
    next_id: &mut u32,
    history: &[Frontier],
) {
    let pick = |rng: &mut Rng, ids: &[String]| ids.get(rng.index(ids.len().max(1))).cloned();
    match rng.below(14) {
        0..=2 => {
            let id = format!("c{next_id}");
            *next_id += 1;
            let parent = if rng.below(4) == 0 { pick(rng, ids) } else { None };
            let index = u32::try_from(rng.below(3)).unwrap();
            if client
                .apply_content_operation(&Operation::CreateNode {
                    id: NodeId::from(id.as_str()),
                    parent: parent.as_deref().map(NodeId::from),
                    index,
                    kind: NodeKind::Block,
                })
                .is_ok()
            {
                ids.push(id);
            }
        }
        3 | 4 => {
            if let Some(id) = pick(rng, ids) {
                let _ = client.apply_content_operation(&Operation::InsertText {
                    id: NodeId::from(id.as_str()),
                    index: 0,
                    text: "word ".into(),
                });
            }
        }
        5 | 6 => {
            if let Some(id) = pick(rng, ids) {
                let _ = client.apply_content_operation(&Operation::DeleteNode {
                    id: NodeId::from(id.as_str()),
                });
            }
        }
        7 => {
            if let (Some(id), parent) = (pick(rng, ids), pick(rng, ids)) {
                let _ = client.apply_content_operation(&Operation::MoveNode {
                    id: NodeId::from(id.as_str()),
                    new_parent: if rng.below(2) == 0 {
                        None
                    } else {
                        parent.as_deref().map(NodeId::from)
                    },
                    index: 0,
                });
            }
        }
        8 => {
            if let Some(id) = pick(rng, ids) {
                let _ = client.apply_page_marks(
                    &id,
                    &[crate::page_inline::MarkEdit {
                        start: 0,
                        end: 1,
                        mark: crate::page_inline::InlineMark::Bold,
                        clear: rng.below(2) == 0,
                        href: None,
                    }],
                );
            }
        }
        9 => {
            let id = format!("a{next_id}");
            *next_id += 1;
            let kind = ["image", "attachment", "childPage"][rng.index(3)];
            let attrs = match kind {
                "image" => image_attrs(),
                "attachment" => {
                    serde_json::json!({"asset_id": ASSET, "name": "f", "mime": "text/plain", "caption": "", "indent": 0})
                }
                _ => serde_json::json!({"child_page_id": ASSET, "indent": 0}),
            };
            if client.insert_page_atom(&id, None, 0, kind, &attrs).is_ok() {
                ids.push(id);
            }
        }
        10 => {
            let _ = client.initialize_page_structure_model(if rng.below(2) == 0 { 4 } else { 5 });
        }
        11 | 12 => {
            // Undo to an earlier accepted point, or redo (a revert back to the latest one).
            if let Some(frontier) = history.get(rng.index(history.len().max(1))) {
                let _ = client.revert_to_frontier(frontier);
            }
        }
        _ => {
            let _ = client.set_title("t");
        }
    }
}

/// Raw adversarial update kinds, each authored against the current state of `engine`. Returns
/// one or two updates (the second is concurrent with the first, from the same base).
#[allow(clippy::too_many_lines)]
fn adversarial_updates(engine: &LoroCollabEngine, rng: &mut Rng, peer: u64) -> Vec<Vec<u8>> {
    let atoms = indices_of(&engine.doc, true);
    let plain = indices_of(&engine.doc, false);
    let list_len = children(&engine.doc).len();
    match rng.below(12) {
        0 => {
            // Delete an atom block map.
            let Some(&index) = atoms.get(rng.index(atoms.len().max(1))) else {
                return vec![raw_update(engine, peer, |raw| {
                    push_raw_atom(raw, "image");
                })];
            };
            vec![raw_update(engine, peer, |raw| {
                children(raw).delete(index, 1).unwrap();
            })]
        }
        1 => {
            // Re-type an atom to a paragraph.
            let Some(&index) = atoms.get(rng.index(atoms.len().max(1))) else {
                return Vec::new();
            };
            vec![raw_update(engine, peer, |raw| {
                map_at(raw, index).insert("nodeName", "paragraph").unwrap();
            })]
        }
        2 => {
            // Re-type a paragraph to an image (invalid attributes: the state is refused).
            let Some(&index) = plain.get(rng.index(plain.len().max(1))) else {
                return Vec::new();
            };
            vec![raw_update(engine, peer, |raw| {
                map_at(raw, index).insert("nodeName", "image").unwrap();
            })]
        }
        3 => {
            // Re-type a paragraph to a heading: suspect, but no atom involved.
            let Some(&index) = plain.get(rng.index(plain.len().max(1))) else {
                return Vec::new();
            };
            vec![raw_update(engine, peer, |raw| {
                map_at(raw, index).insert("nodeName", "heading").unwrap();
            })]
        }
        4 => {
            // Replace `doc.children` with a new list holding one fresh atom or paragraph.
            let atom = rng.below(2) == 0;
            vec![raw_update(engine, peer, |raw| {
                let list = raw
                    .get_map("doc")
                    .insert_container("children", LoroList::new())
                    .unwrap();
                let map = list.insert_container(0, LoroMap::new()).unwrap();
                map.insert("nodeName", if atom { "image" } else { "paragraph" })
                    .unwrap();
                let attrs = map.insert_container("attributes", LoroMap::new()).unwrap();
                if atom {
                    attrs.insert("asset_id", ASSET).unwrap();
                }
                map.insert_container("children", LoroList::new()).unwrap();
            })]
        }
        5 => {
            // Flip the marker to v4 and leave everything else alone.
            vec![raw_update(engine, peer, |raw| set_model(raw, "rich_page_v4"))]
        }
        6 => {
            // Flip the marker and delete every atom: a guest dropping a model-5 floor.
            vec![raw_update(engine, peer, |raw| {
                set_model(raw, "rich_page_v4");
                for &index in atoms.iter().rev() {
                    children(raw).delete(index, 1).unwrap();
                }
            })]
        }
        7 => {
            // Two peers delete the same atom concurrently: the second deletes a deleted element.
            let Some(&index) = atoms.get(rng.index(atoms.len().max(1))) else {
                return Vec::new();
            };
            vec![
                raw_update(engine, peer, |raw| children(raw).delete(index, 1).unwrap()),
                raw_update(engine, peer + 1, |raw| children(raw).delete(index, 1).unwrap()),
            ]
        }
        8 => {
            // Create consecutive atom maps and delete them backwards, all in one update.
            vec![raw_update(engine, peer, |raw| {
                let list = children(raw);
                let start = list.len();
                let maps: Vec<LoroMap> = (0..3)
                    .map(|k| list.insert_container(start + k, LoroMap::new()).unwrap())
                    .collect();
                for map in &maps {
                    map.insert("nodeName", "image").unwrap();
                }
                for k in (0..3).rev() {
                    list.delete(start + k, 1).unwrap();
                }
            })]
        }
        9 => {
            // Delete a run of consecutive blocks from the end of the list (plain or atom).
            let count = list_len.min(3);
            if count == 0 {
                return Vec::new();
            }
            vec![raw_update(engine, peer, |raw| {
                let list = children(raw);
                for k in 0..count {
                    list.delete(list.len() - 1 - k.min(list.len() - 1), 1).unwrap();
                }
            })]
        }
        10 => {
            // Add a raw atom: raises a flat page's floor with no marker.
            vec![raw_update(engine, peer, |raw| {
                push_raw_atom(raw, ["image", "attachment"][usize::from(peer.is_multiple_of(2))]);
            })]
        }
        _ => {
            // Insert then retype in one update: creates a map and changes its kind.
            vec![raw_update(engine, peer, |raw| {
                let id = push_raw_atom(raw, "image");
                let list = children(raw);
                if let Some(ValueOrContainer::Container(Container::Map(map))) = list.get(list.len() - 1) {
                    assert_eq!(map.id(), id);
                    map.insert("nodeName", "paragraph").unwrap();
                }
            })]
        }
    }
}

fn run_sequence(floor: u32, seed: u64, steps: usize, tally: &mut Tally) {
    let mut rng = Rng(seed ^ (u64::from(floor) << 40) | 1);
    let blocks = 50 + rng.index(70);
    let mut server = page(floor, blocks, seed % 1_000);
    // A client holding the same history under another peer id.
    let mut client = LoroCollabEngine::new_empty(seed % 1_000 + 77);
    client.import_update(&server.export_snapshot().unwrap()).unwrap();
    let mut ids: Vec<String> = (0..blocks.min(20)).map(|index| format!("s{index}")).collect();
    let mut next_id = 0_u32;
    let mut history = vec![server.frontier()];
    let mut snapshots = vec![(server.frontier(), server.export_snapshot().unwrap())];
    for index in 0..steps {
        let label = format!("floor {floor} seed {seed:#x} step {index}");
        let causal = rng.below(2) == 0;
        match rng.below(10) {
            0..=3 => {
                // A client edit, sometimes concurrent with a server-side edit.
                if rng.below(3) == 0 {
                    let id = format!("srv{index}");
                    let _ = server.apply_content_operation(&Operation::CreateNode {
                        id: NodeId::from(id.as_str()),
                        parent: None,
                        index: 0,
                        kind: NodeKind::Block,
                    });
                }
                client_edit(&mut client, &mut rng, &mut ids, &mut next_id, &history);
                let update = client.export_from(&server.frontier()).unwrap();
                checked_step(&mut server, &update, causal, &label, tally);
                let back = server.export_from(&client.frontier()).unwrap();
                let _ = client.import_update(&back);
            }
            4..=6 => {
                let updates = adversarial_updates(&server, &mut rng, 100_000 + 2 * index as u64);
                for (n, update) in updates.iter().enumerate() {
                    checked_step(&mut server, update, causal, &format!("{label} raw {n}"), tally);
                }
                if rng.below(2) == 0 {
                    let back = server.export_from(&client.frontier()).unwrap();
                    let _ = client.import_update(&back);
                }
            }
            7 => {
                // A cold engine: snapshot load, then the update.
                server = LoroCollabEngine::load(&server.export_snapshot().unwrap()).unwrap();
                client_edit(&mut client, &mut rng, &mut ids, &mut next_id, &history);
                let update = client.export_from(&server.frontier()).unwrap();
                checked_step(&mut server, &update, causal, &format!("{label} cold"), tally);
            }
            8 => {
                // Snapshot-plus-tail rebuild, the host's bootstrap: lenient import onto an old snapshot.
                let (frontier, snapshot) = &snapshots[rng.index(snapshots.len())];
                let tail = server.export_from(frontier).unwrap();
                let mut rebuilt = LoroCollabEngine::load(snapshot).unwrap();
                if rebuilt.compute_minimum_editor_model().is_ok() {
                    checked_step(&mut rebuilt, &tail, false, &format!("{label} rebuild"), tally);
                    if rebuilt.frontier() == server.frontier() && rebuilt.compute_minimum_editor_model().is_ok() {
                        server = rebuilt;
                    }
                }
            }
            _ => {
                // A causally incomplete update: refused when complete-only, buffered otherwise.
                let mut author = server.fork().unwrap();
                author.set_title("dependency").unwrap();
                let after = author.frontier();
                author.set_title("dependent").unwrap();
                let dependent = author.export_from(&after).unwrap();
                checked_step(&mut server, &dependent, causal, &format!("{label} pending"), tally);
            }
        }
        if index % 10 == 0 {
            history.push(server.frontier());
            snapshots.push((server.frontier(), server.export_snapshot().unwrap()));
        }
    }
}

/// The randomized equivalence test: across seeded random histories on pages starting at floors
/// 1, 4 and 5 (50-120 blocks), the guard and the reference agree on every update, the fused
/// floor equals the walk on every state, and no guard run walks the document.
#[test]
fn floor_guard_matches_reference_across_random_histories() {
    let seeds: [u64; 10] = [
        0x9e37_79b9_7f4a_7c15,
        0x2545_f491_4f6c_dd1d,
        0xdead_beef_cafe_f00d,
        7,
        42,
        0x0123_4567_89ab_cdef,
        0xfeed_face_0bad_c0de,
        0x5151_5151_a5a5_a5a5,
        1_000_003,
        0xc001_d00d_1234_5678,
    ];
    let mut tally = Tally::default();
    for floor in [1, 4, 5] {
        for seed in seeds {
            run_sequence(floor, seed, 200, &mut tally);
        }
    }
    println!(
        "guard arms (arm, accepted) -> count: {:?}\nimports accepted {} refused {} (floor downgrades {})\nfloors reached: {:?}",
        tally.arms, tally.accepted, tally.refused, tally.downgrades, tally.floors
    );
    for arm in [
        (Arm::MarkerSetAfter, true),
        (Arm::NoSuspectOp, true),
        (Arm::MarkerCleared, true),
        (Arm::MarkerCleared, false),
        (Arm::SuspectAtomRemains, true),
        (Arm::SuspectBaseRead, true),
        (Arm::SuspectBaseRead, false),
    ] {
        assert!(tally.arms.get(&arm).copied().unwrap_or(0) > 0, "arm {arm:?} never hit");
    }
    for floor in [1, 4, 5] {
        assert!(tally.floors.contains_key(&floor), "floor {floor} never reached");
    }
    assert!(tally.downgrades > 0, "no downgrade was exercised");
}

// ---------------------------------------------------------------------------------------
// Fallback paths, one by one: each proves it is taken (arm and counters) and that it decides
// exactly as the reference does.
// ---------------------------------------------------------------------------------------

fn expect(engine: &mut LoroCollabEngine, update: &[u8], label: &str) -> (Step, (usize, usize, usize)) {
    let outcome = step(engine, update, true, label);
    (outcome, walk_counter::guard_counts())
}

#[test]
fn marker_cleared_with_atoms_remaining_is_accepted_by_the_narrow_scan() {
    let mut engine = page(5, 8, 1);
    let update = raw_update(&engine, 70_001, |raw| set_model(raw, "rich_page_v4"));
    let (outcome, counts) = expect(&mut engine, &update, "marker flip, atom stays");
    assert_eq!(outcome.arm, Arm::MarkerCleared);
    assert!(outcome.accepted && !outcome.downgrade);
    assert_eq!((counts.1, counts.2), (1, 0), "one narrow scan, no historical read");
}

#[test]
fn marker_cleared_with_every_atom_removed_is_refused_as_a_downgrade() {
    let mut engine = page(5, 8, 2);
    let atoms = indices_of(&engine.doc, true);
    let update = raw_update(&engine, 70_002, |raw| {
        set_model(raw, "rich_page_v4");
        for &index in atoms.iter().rev() {
            children(raw).delete(index, 1).unwrap();
        }
    });
    let (outcome, counts) = expect(&mut engine, &update, "marker flip, atoms deleted");
    assert_eq!(outcome.arm, Arm::MarkerCleared);
    assert!(outcome.downgrade);
    assert_eq!((counts.1, counts.2), (1, 0));
}

#[test]
fn deleting_the_only_unmarked_atom_reads_the_base_and_refuses() {
    let mut engine = flat_page_with_raw_atom(3);
    let index = *indices_of(&engine.doc, true).first().unwrap();
    let update = raw_update(&engine, 70_003, |raw| children(raw).delete(index, 1).unwrap());
    let (outcome, counts) = expect(&mut engine, &update, "delete the only atom");
    assert_eq!(outcome.arm, Arm::SuspectBaseRead);
    assert!(outcome.downgrade);
    assert_eq!((counts.1, counts.2), (1, 1), "narrow scan, then one historical read");
    assert_eq!(
        engine.minimum_editor_model().unwrap(),
        1,
        "the refused state is what was imported"
    );
}

#[test]
fn deleting_one_of_two_atoms_keeps_the_floor_and_skips_the_base_read() {
    let mut engine = flat_page_with_raw_atom(4);
    let second = raw_update(&engine, 70_004, |raw| {
        push_raw_atom(raw, "attachment");
    });
    engine.import_update(&second).unwrap();
    let index = *indices_of(&engine.doc, true).first().unwrap();
    let update = raw_update(&engine, 70_005, |raw| children(raw).delete(index, 1).unwrap());
    let (outcome, counts) = expect(&mut engine, &update, "delete one of two atoms");
    assert_eq!(outcome.arm, Arm::SuspectAtomRemains);
    assert!(outcome.accepted);
    assert_eq!((counts.1, counts.2), (1, 0));
}

#[test]
fn retyping_a_paragraph_on_an_atom_free_page_is_suspect_and_accepted_after_the_base_read() {
    let mut engine = page(1, 8, 5);
    let index = *indices_of(&engine.doc, false).first().unwrap();
    let update = raw_update(&engine, 70_006, |raw| {
        map_at(raw, index).insert("nodeName", "heading").unwrap();
    });
    let (outcome, counts) = expect(&mut engine, &update, "paragraph to heading");
    assert_eq!(outcome.arm, Arm::SuspectBaseRead);
    assert!(outcome.accepted);
    assert_eq!((counts.1, counts.2), (1, 1));
}

#[test]
fn retyping_an_atom_to_a_paragraph_is_judged_on_the_base() {
    let mut engine = flat_page_with_raw_atom(6);
    let index = *indices_of(&engine.doc, true).first().unwrap();
    let update = raw_update(&engine, 70_007, |raw| {
        map_at(raw, index).insert("nodeName", "paragraph").unwrap();
    });
    let (outcome, counts) = expect(&mut engine, &update, "atom to paragraph");
    assert_eq!(outcome.arm, Arm::SuspectBaseRead);
    assert!(outcome.downgrade);
    assert_eq!(counts.2, 1);
}

#[test]
fn replacing_the_root_list_is_decided_by_the_base_not_by_a_blanket_answer() {
    // Base without atoms: replacing the list lowers nothing, so the update is accepted even
    // though the `doc` map was touched.
    let mut clean = page(1, 8, 7);
    let replace = |engine: &LoroCollabEngine, peer| {
        raw_update(engine, peer, |raw| {
            let list = raw
                .get_map("doc")
                .insert_container("children", LoroList::new())
                .unwrap();
            let map = list.insert_container(0, LoroMap::new()).unwrap();
            map.insert("nodeName", "paragraph").unwrap();
            map.insert_container("attributes", LoroMap::new()).unwrap();
            map.insert_container("children", LoroList::new()).unwrap();
        })
    };
    let update = replace(&clean, 70_008);
    let (outcome, counts) = expect(&mut clean, &update, "replace list, no atoms before");
    assert_eq!(outcome.arm, Arm::SuspectBaseRead);
    assert!(outcome.accepted, "an atom-free base cannot be downgraded");
    assert_eq!(counts.2, 1);

    // Base with an unmarked atom: replacing the list drops it.
    let mut atomic = flat_page_with_raw_atom(8);
    let update = replace(&atomic, 70_009);
    let (outcome, counts) = expect(&mut atomic, &update, "replace list, atom before");
    assert_eq!(outcome.arm, Arm::SuspectBaseRead);
    assert!(outcome.downgrade);
    assert_eq!(counts.2, 1);
}

#[test]
fn deleting_an_already_deleted_atom_is_judged_on_the_current_base() {
    let mut engine = flat_page_with_raw_atom(9);
    let index = *indices_of(&engine.doc, true).first().unwrap();
    let first = raw_update(&engine, 70_010, |raw| children(raw).delete(index, 1).unwrap());
    let second = raw_update(&engine, 70_011, |raw| children(raw).delete(index, 1).unwrap());
    let (outcome, _) = expect(&mut engine, &first, "first delete");
    assert!(outcome.downgrade);
    // The atom is already gone: this delete takes nothing away from the (floor 1) base.
    let (outcome, counts) = expect(&mut engine, &second, "second delete");
    assert!(outcome.accepted, "the base no longer holds an atom");
    assert_eq!(outcome.arm, Arm::SuspectBaseRead);
    assert_eq!(counts.2, 1);
}

#[test]
fn an_atom_created_and_deleted_by_one_update_is_not_suspect() {
    let mut engine = flat_page_with_raw_atom(10);
    let update = raw_update(&engine, 70_012, |raw| {
        let list = children(raw);
        let start = list.len();
        let maps: Vec<LoroMap> = (0..3)
            .map(|k| list.insert_container(start + k, LoroMap::new()).unwrap())
            .collect();
        for map in &maps {
            map.insert("nodeName", "image").unwrap();
        }
        for k in (0..3).rev() {
            list.delete(start + k, 1).unwrap();
        }
    });
    let (outcome, counts) = expect(&mut engine, &update, "create and delete atoms");
    assert_eq!(outcome.arm, Arm::NoSuspectOp, "the elements did not exist in the base");
    assert_eq!((counts.1, counts.2), (0, 0));
    assert!(counts.0 > 0);
}

#[test]
fn a_backward_run_of_deletes_naming_an_atom_is_suspect() {
    let mut engine = page(1, 4, 11);
    let start = children(&engine.doc).len();
    // Consecutive element ids, so that the deletion below is one backward span.
    let base = raw_update(&engine, 70_013, |raw| {
        let list = children(raw);
        let maps: Vec<LoroMap> = (0..3)
            .map(|k| list.insert_container(start + k, LoroMap::new()).unwrap())
            .collect();
        for (map, kind) in maps.iter().zip(["paragraph", "image", "paragraph"]) {
            map.insert("nodeName", kind).unwrap();
            if kind == "image" {
                let attrs = map.insert_container("attributes", LoroMap::new()).unwrap();
                attrs.insert("asset_id", ASSET).unwrap();
            }
            map.insert_container("children", LoroList::new()).unwrap();
        }
    });
    engine.import_update(&base).unwrap();
    assert_eq!(engine.minimum_editor_model().unwrap(), 5);
    let update = raw_update(&engine, 70_014, |raw| {
        let list = children(raw);
        for k in (0..3).rev() {
            list.delete(start + k, 1).unwrap();
        }
    });
    let (outcome, counts) = expect(&mut engine, &update, "backward delete of three blocks");
    assert_eq!(outcome.arm, Arm::SuspectBaseRead);
    assert!(outcome.downgrade);
    assert_eq!(counts.2, 1);
}

// ---------------------------------------------------------------------------------------
// Cost: counted, not timed.
// ---------------------------------------------------------------------------------------

/// Total ops in the update that takes `from` to the author's head, counted through a different
/// Loro API than the guard's.
fn op_count(author: &LoroCollabEngine, from: &loro::VersionVector) -> usize {
    author
        .doc
        .export_json_updates(from, &author.doc.oplog_vv())
        .unwrap()
        .changes
        .iter()
        .map(|change| change.ops.len())
        .sum()
}

fn sized_page(floor: u32, blocks: usize) -> LoroCollabEngine {
    let mut engine = LoroCollabEngine::new_empty(900 + u64::from(floor));
    engine.initialize_page_editor().unwrap();
    if floor >= 4 {
        engine.initialize_page_structure_model(floor).unwrap();
    }
    if floor >= 5 {
        engine
            .insert_page_atom("seed-image", None, 0, "image", &image_attrs())
            .unwrap();
    }
    // Fill in one commit's worth of operations; the flat page needs no placement work.
    for index in 0..blocks {
        block(&mut engine, &format!("s{index}"), 0);
    }
    engine
}

/// The guard's floor work for an ordinary update does not grow with the document: zero floor
/// walks, zero block reads, no narrow or historical atom scan, and the ops it visits equal the
/// update's own op count, identical at 200 and 2,000 blocks.
#[test]
fn guard_cost_for_ordinary_updates_is_independent_of_document_size() {
    for floor in [1, 4, 5] {
        let mut visited: BTreeMap<&str, Vec<(usize, usize)>> = BTreeMap::new();
        for blocks in [200_usize, 2_000] {
            let author = sized_page(floor, blocks);
            let base = author.export_snapshot().unwrap();
            for shape in ["one-block edit", "50-block batch", "block deletion"] {
                let mut author = author.fork().unwrap();
                let from_frontier = author.frontier();
                let from = author.doc.oplog_vv();
                match shape {
                    "one-block edit" => {
                        author
                            .apply_content_operation(&Operation::InsertText {
                                id: NodeId::from("s1"),
                                index: 0,
                                text: "typed".into(),
                            })
                            .unwrap();
                    }
                    "50-block batch" => {
                        for index in 0..50 {
                            let id = format!("n{index}");
                            block(&mut author, &id, 0);
                            author
                                .apply_content_operation(&Operation::InsertText {
                                    id: NodeId::from(id.as_str()),
                                    index: 0,
                                    text: "batch".into(),
                                })
                                .unwrap();
                        }
                    }
                    _ => {
                        author
                            .apply_content_operation(&Operation::DeleteNode { id: NodeId::from("s2") })
                            .unwrap();
                    }
                }
                let update = author.export_from(&from_frontier).unwrap();
                let expected_ops = op_count(&author, &from);
                let mut worker = LoroCollabEngine::load(&base).unwrap();
                walk_counter::reset();
                worker.import_update_causally_complete(&update).unwrap();
                assert_eq!(
                    walk_counter::counts(),
                    (0, 0),
                    "floor {floor}, {blocks} blocks, {shape}: floor walks and block reads"
                );
                let (ops_scanned, narrow, historical) = walk_counter::guard_counts();
                assert_eq!(
                    (narrow, historical),
                    (0, 0),
                    "floor {floor}, {blocks} blocks, {shape}: fallback scans"
                );
                // With the model-5 marker set after the import nothing can be refused, so the guard
                // returns before reading any op.
                let expected_scan = if floor == 5 { 0 } else { expected_ops };
                assert_eq!(
                    ops_scanned, expected_scan,
                    "floor {floor}, {blocks} blocks, {shape}: ops visited equal the update's ops"
                );
                visited.entry(shape).or_default().push((blocks, ops_scanned));
            }
        }
        for (shape, runs) in visited {
            let counts: Vec<usize> = runs.iter().map(|&(_, count)| count).collect();
            assert!(
                counts.windows(2).all(|pair| pair[0] == pair[1]),
                "floor {floor}, {shape}: ops visited differ with document size: {runs:?}"
            );
        }
    }
}

/// `semantic_snapshot_with_floor` fills the memo for the state it read, so the host's separate
/// floor call costs no walk, and it never fills the memo on a failed read.
#[test]
fn fused_floor_fills_the_memo_and_never_on_failure() {
    for floor in [1, 4, 5] {
        let engine = LoroCollabEngine::load(&page(floor, 30, 3).export_snapshot().unwrap()).unwrap();
        walk_counter::reset();
        let (_, fused) = engine.semantic_snapshot_with_floor().unwrap();
        assert_eq!(fused, floor);
        assert_eq!(engine.minimum_editor_model().unwrap(), floor);
        assert_eq!(walk_counter::counts().0, 0, "floor {floor}: the memo answered");
    }
    // A document the reader rejects leaves no memo entry.
    let mut engine = page(1, 4, 4);
    let broken = raw_update(&engine, 80_001, |raw| {
        map_at(raw, 0)
            .insert("nodeName", Value::Null.to_string().as_str())
            .unwrap();
        map_at(raw, 1).delete("nodeName").unwrap();
    });
    engine.import_update(&broken).unwrap();
    assert!(engine.semantic_snapshot_with_floor().is_err());
    assert!(
        engine.editor_model_floor.get().is_none(),
        "no memo after a failed projection"
    );
    assert!(engine.minimum_editor_model().is_err());
}
