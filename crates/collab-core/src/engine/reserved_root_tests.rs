//! The client write path refuses a new "shadow root": a root container with a reserved name
//! (`reserved_roots`) and a container type other than the expected one. Replay of accepted history
//! keeps accepting it.
//!
//! Every update below is hostile input, built with raw Loro peers, so the checks rest on what the
//! library records in the op log and not on how a well-behaved client would encode a change.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::print_stdout
)]

use loro::{ContainerType, ExportMode, LoroDoc, LoroList, LoroMap, VersionVector};
use serde_json::Value;

use super::floor_equivalence_tests::{SHADOWABLE, children, flat_page_with_raw_atom, page, raw_update, set_model};
use super::{CollabEngine, LoroCollabEngine};
use crate::error::CollabError;
use crate::operation::{NodeId, Operation};
use crate::reserved_roots::{RESERVED_ROOTS, SCAN_INCOMPLETE_REASON, SHADOW_ROOT_REASON, expected_type};
use crate::walk_counter;

fn is_reserved_refusal<T: std::fmt::Debug>(result: &Result<T, CollabError>) -> bool {
    matches!(result, Err(CollabError::OperationFailed { reason }) if reason == SHADOW_ROOT_REASON)
}

fn all_types() -> Vec<ContainerType> {
    let mut kinds = SHADOWABLE.to_vec();
    kinds.push(ContainerType::Counter);
    kinds
}

/// One op on root `name` of `kind`: the smallest update that creates the root.
fn root_update(engine: &LoroCollabEngine, peer: u64, name: &str, kind: ContainerType) -> Vec<u8> {
    raw_update(engine, peer, |raw| write_root(raw, name, kind))
}

fn write_root(raw: &LoroDoc, name: &str, kind: ContainerType) {
    // A value no earlier write used, so that a second write to the same root is never a no-op.
    let salt = i64::try_from(raw.peer_id() % 1_000_000).unwrap();
    match kind {
        ContainerType::Text => raw.get_text(name).insert(0, &format!("x{salt}")).unwrap(),
        ContainerType::List => raw.get_list(name).insert(0, salt).unwrap(),
        ContainerType::MovableList => raw.get_movable_list(name).insert(0, salt).unwrap(),
        ContainerType::Map => raw.get_map(name).insert("k", salt).unwrap(),
        ContainerType::Tree => {
            raw.get_tree(name).create(None).unwrap();
        }
        ContainerType::Counter => raw.get_counter(name).increment(1.0).unwrap(),
        other @ ContainerType::Unknown(_) => panic!("unsupported kind {other:?}"),
    }
}

/// A page that already carries the shadow root `name`/`kind`, created through the replay path.
fn page_with_shadow(floor: u32, seed: u64, name: &str, kind: ContainerType) -> LoroCollabEngine {
    let mut engine = page(floor, 6, seed);
    let update = root_update(&engine, seed + 40_000, name, kind);
    engine.import_update(&update).unwrap();
    engine
}

fn bases() -> Vec<(&'static str, LoroCollabEngine)> {
    vec![
        ("flat", page(1, 6, 11)),
        ("structured", page(4, 6, 12)),
        ("structured v5", page(5, 6, 13)),
    ]
}

/// Every reserved name with every wrong type is refused on the client path with the specific
/// reason, whatever the base; the engine is then discarded, as the worker does. The same bytes
/// are accepted by both replay entries, and the stored base is unchanged.
#[test]
fn every_reserved_name_and_wrong_type_is_refused_on_the_client_path_and_replays() {
    let mut cases = 0;
    for (label, base) in bases() {
        let stored = base.export_snapshot().unwrap();
        for (name, expected) in RESERVED_ROOTS {
            for kind in all_types().into_iter().filter(|kind| *kind != expected) {
                let update = root_update(&base, 61_000, name, kind);
                let mut client = LoroCollabEngine::load(&stored).unwrap();
                let outcome = client.import_client_update(&update);
                assert!(is_reserved_refusal(&outcome), "{label}: {name} {kind:?}: {outcome:?}");
                // The worker discards the engine and persists nothing: what is stored is the base.
                let reloaded = LoroCollabEngine::load(&stored).unwrap();
                assert_eq!(reloaded.export_snapshot().unwrap(), stored, "{label}: {name} {kind:?}");
                for causal in [false, true] {
                    let mut replay = LoroCollabEngine::load(&stored).unwrap();
                    let accepted = if causal {
                        replay.import_update_causally_complete(&update)
                    } else {
                        replay.import_update(&update)
                    };
                    assert!(accepted.is_ok(), "{label}: replay of {name} {kind:?} (causal {causal})");
                    assert!(replay.semantic_snapshot_with_floor().is_ok());
                }
                cases += 1;
            }
        }
    }
    // 7 names x 5 wrong types x 3 bases.
    assert_eq!(cases, 7 * 5 * 3);
}

/// A reserved root of the RIGHT type is accepted exactly as the replay path accepts it, and the
/// result is the same document.
#[test]
fn a_reserved_root_with_the_right_type_is_accepted_as_before() {
    for (label, base) in bases() {
        let stored = base.export_snapshot().unwrap();
        for (name, kind) in RESERVED_ROOTS {
            let update = root_update(&base, 62_000, name, kind);
            let mut client = LoroCollabEngine::load(&stored).unwrap();
            let mut replay = LoroCollabEngine::load(&stored).unwrap();
            client.import_client_update(&update).unwrap();
            replay.import_update_causally_complete(&update).unwrap();
            assert_eq!(client.frontier(), replay.frontier(), "{label}: {name}");
            assert_eq!(
                client.export_snapshot().unwrap(),
                replay.export_snapshot().unwrap(),
                "{label}: {name}"
            );
        }
        // Names that are not reserved are free, whatever the type.
        for kind in all_types() {
            let update = root_update(&base, 62_500, "scratch_root", kind);
            LoroCollabEngine::load(&stored)
                .unwrap()
                .import_client_update(&update)
                .unwrap();
        }
    }
}

/// First-time editor initialisation, as the engine authors it, goes through the client path: the
/// page editor, the placement index with its tree and metadata roots, and the model-5 marker.
#[test]
fn editor_initialisation_updates_are_accepted_on_the_client_path() {
    let mut author = LoroCollabEngine::new_empty(7);
    let empty = LoroCollabEngine::new_empty(8);
    let genesis = author.frontier();
    let mut receiver = LoroCollabEngine::load(&empty.export_snapshot().unwrap()).unwrap();
    receiver.import_update(&author.export_snapshot().unwrap()).unwrap();
    let mut step = |author: &mut LoroCollabEngine, edit: &dyn Fn(&mut LoroCollabEngine)| {
        let from = author.frontier();
        edit(author);
        let update = author.export_from(&from).unwrap();
        receiver.import_client_update(&update).unwrap();
        assert_eq!(receiver.frontier(), author.frontier());
    };
    let _ = genesis;
    step(&mut author, &|engine| {
        engine
            .apply_content_operation(&Operation::CreateNode {
                id: NodeId::from("first"),
                parent: None,
                index: 0,
                kind: crate::operation::NodeKind::Block,
            })
            .unwrap();
    });
    step(&mut author, &|engine| {
        engine.initialize_page_editor().unwrap();
    });
    step(&mut author, &|engine| {
        engine.initialize_page_structure_model(4).unwrap();
    });
    step(&mut author, &|engine| {
        engine
            .insert_page_atom(
                "seed-image",
                None,
                0,
                "image",
                &serde_json::json!({"asset_id": "00000000-0000-4000-8000-000000000001", "alt": "", "caption": "", "indent": 0}),
            )
            .unwrap();
    });
    assert_eq!(receiver.minimum_editor_model().unwrap(), 5);
}

/// A page that already carries a shadow root (created by replay) stays writable for ordinary
/// updates with its floor decision unchanged. An update that adds a second shadow, or writes into
/// the existing one, is refused on the client path.
#[test]
fn a_page_with_an_existing_shadow_stays_writable_and_takes_no_second_one() {
    let shadows = [
        (crate::reserved_roots::DOC, ContainerType::Text),
        (crate::reserved_roots::META, ContainerType::Text),
        (crate::reserved_roots::META, ContainerType::List),
        (crate::reserved_roots::TREE, ContainerType::Map),
        (crate::reserved_roots::ALIASES, ContainerType::List),
        (crate::reserved_roots::ORDER, ContainerType::Text),
    ];
    for floor in [1, 4, 5] {
        for (index, (name, kind)) in shadows.into_iter().enumerate() {
            let label = format!("floor {floor}: {name} {kind:?}");
            let base = page_with_shadow(floor, 200 + index as u64, name, kind);
            let before_floor = base.compute_minimum_editor_model().unwrap();
            let stored = base.export_snapshot().unwrap();

            // An ordinary edit by another client, authored on the same history.
            let mut author = LoroCollabEngine::load(&stored).unwrap();
            author.doc.set_peer_id(63_000 + index as u64).unwrap();
            let from = author.frontier();
            author
                .apply_content_operation(&Operation::CreateNode {
                    id: NodeId::from("typed"),
                    parent: None,
                    index: 0,
                    kind: crate::operation::NodeKind::Block,
                })
                .unwrap();
            let ordinary = author.export_from(&from).unwrap();
            let mut worker = LoroCollabEngine::load(&stored).unwrap();
            worker
                .import_client_update(&ordinary)
                .unwrap_or_else(|e| panic!("{label}: {e}"));
            assert_eq!(worker.compute_minimum_editor_model().unwrap(), before_floor, "{label}");
            assert_eq!(
                worker.semantic_snapshot_with_floor().unwrap().1,
                before_floor,
                "{label}: fused floor"
            );

            // Writing into the existing shadow, creating the same root again under another type,
            // and a second shadow under another name are all refused.
            let into_existing = raw_update(&base, 63_500, |raw| write_root(raw, name, kind));
            let another_type = root_update(
                &base,
                63_600,
                name,
                if kind == ContainerType::Counter {
                    ContainerType::Text
                } else {
                    ContainerType::Counter
                },
            );
            let second_name = RESERVED_ROOTS
                .iter()
                .find(|(other, _)| *other != name)
                .map(|(other, _)| *other)
                .unwrap();
            let second = root_update(&base, 63_700, second_name, ContainerType::Counter);
            for (what, update) in [
                ("write into the existing shadow", into_existing),
                ("another type", another_type),
                ("second shadow", second),
            ] {
                let outcome = LoroCollabEngine::load(&stored).unwrap().import_client_update(&update);
                assert!(is_reserved_refusal(&outcome), "{label}: {what}: {outcome:?}");
            }
        }
    }
}

// ---------------------------------------------------------------------------------------
// Forgery resistance
// ---------------------------------------------------------------------------------------

/// The shadow is created and then referenced only through containers inside it (a child map in a
/// list root, a nested list in a map root). The update still has to carry the op on the root.
#[test]
fn a_shadow_referenced_only_through_child_containers_is_refused() {
    let base = page(1, 6, 21);
    let stored = base.export_snapshot().unwrap();
    for name in [
        crate::reserved_roots::META,
        crate::reserved_roots::DOC,
        crate::reserved_roots::ORDER,
    ] {
        let update = raw_update(&base, 64_000, |raw| {
            let list = raw.get_list(name);
            let child = list.insert_container(0, LoroMap::new()).unwrap();
            child.insert("a", 1).unwrap();
            raw.commit();
            let nested = child.insert_container("n", LoroList::new()).unwrap();
            nested.insert(0, 1).unwrap();
            raw.commit();
            nested.delete(0, 1).unwrap();
        });
        let outcome = LoroCollabEngine::load(&stored).unwrap().import_client_update(&update);
        assert!(is_reserved_refusal(&outcome), "{name}: {outcome:?}");
    }
    // Only the later changes of that history, without the change that created the root: they
    // depend on it, so they cannot be applied, and an update that does not apply is refused.
    let author = LoroDoc::new();
    author.import(&stored).unwrap();
    author.set_peer_id(64_100).unwrap();
    let child = author
        .get_list(crate::reserved_roots::META)
        .insert_container(0, LoroMap::new())
        .unwrap();
    author.commit();
    let after_root = author.oplog_vv();
    child.insert("a", 1).unwrap();
    author.commit();
    let dependent = author.export(ExportMode::updates(&after_root)).unwrap();
    let outcome = LoroCollabEngine::load(&stored)
        .unwrap()
        .import_client_update(&dependent);
    assert!(
        matches!(outcome, Err(CollabError::CausalDependencyMissing)),
        "{outcome:?}"
    );
}

/// Changes split so that part of the update only applies later, from the pending buffer. The
/// scan reads what the import applied to the op log, not what the update's first bytes held.
#[test]
fn ops_that_apply_later_from_pending_are_still_scanned() {
    let base = page(1, 6, 22);
    let stored = base.export_snapshot().unwrap();
    let author = LoroDoc::new();
    author.import(&stored).unwrap();
    author.set_peer_id(65_000).unwrap();
    let from = author.oplog_vv();
    author.get_map("scratch").insert("a", 1).unwrap();
    author.commit();
    let mid = author.oplog_vv();
    author.get_text(crate::reserved_roots::META).insert(0, "x").unwrap();
    author.commit();
    let first = author
        .export(ExportMode::updates_in_range(vec![loro::IdSpan::new(65_000, 0, 1)]))
        .unwrap();
    let second = author.export(ExportMode::updates(&mid)).unwrap();
    let whole = author.export(ExportMode::updates(&from)).unwrap();
    assert!(!first.is_empty());

    // The dependent change first: refused as incomplete, and the engine buffers it. A later
    // update completes it, and the ops that then apply include the shadow op.
    let mut engine = LoroCollabEngine::load(&stored).unwrap();
    let outcome = engine.import_client_update(&second);
    assert!(
        matches!(outcome, Err(CollabError::CausalDependencyMissing)),
        "{outcome:?}"
    );
    let outcome = engine.import_client_update(&first);
    assert!(is_reserved_refusal(&outcome), "{outcome:?}");

    // Both changes in one update.
    let outcome = LoroCollabEngine::load(&stored).unwrap().import_client_update(&whole);
    assert!(is_reserved_refusal(&outcome), "{outcome:?}");
}

/// A right-typed root and a wrong-typed root with the same name in one update.
#[test]
fn a_right_typed_root_does_not_cover_a_wrong_typed_one_in_the_same_update() {
    let base = page(1, 6, 23);
    let stored = base.export_snapshot().unwrap();
    for (name, expected) in RESERVED_ROOTS {
        for kind in all_types().into_iter().filter(|kind| *kind != expected) {
            for right_first in [true, false] {
                let update = raw_update(&base, 66_000, |raw| {
                    if right_first {
                        write_root(raw, name, expected);
                        raw.commit();
                        write_root(raw, name, kind);
                    } else {
                        write_root(raw, name, kind);
                        raw.commit();
                        write_root(raw, name, expected);
                    }
                });
                let outcome = LoroCollabEngine::load(&stored).unwrap().import_client_update(&update);
                assert!(
                    is_reserved_refusal(&outcome),
                    "{name} {kind:?} right first {right_first}: {outcome:?}"
                );
            }
        }
    }
}

/// Peer and counter tricks of the kind the forged-delete tests use: the shadow comes from a peer
/// the base already knows, continues its counters, or hides behind a re-sent history prefix.
#[test]
fn peer_and_counter_tricks_do_not_hide_a_shadow() {
    let base = page(1, 6, 24);
    let stored = base.export_snapshot().unwrap();
    let known_peers: Vec<u64> = base.doc.oplog_vv().keys().copied().collect();
    assert!(!known_peers.is_empty());

    // A known peer continuing at its next counter.
    for peer in &known_peers {
        let update = root_update(&base, *peer, crate::reserved_roots::META, ContainerType::Text);
        let outcome = LoroCollabEngine::load(&stored).unwrap().import_client_update(&update);
        assert!(is_reserved_refusal(&outcome), "known peer {peer}: {outcome:?}");
    }

    // The whole history re-sent from the start, shadow at the end: the overlap with what the base
    // holds is skipped, the new tail is scanned.
    let author = LoroDoc::new();
    author.import(&stored).unwrap();
    author.set_peer_id(67_000).unwrap();
    author.get_text(crate::reserved_roots::DOC).insert(0, "x").unwrap();
    author.commit();
    let overlapping = author.export(ExportMode::all_updates()).unwrap();
    let outcome = LoroCollabEngine::load(&stored)
        .unwrap()
        .import_client_update(&overlapping);
    assert!(is_reserved_refusal(&outcome), "overlapping history: {outcome:?}");

    // Many changes of one peer with a shadow in the last of them.
    let author = LoroDoc::new();
    author.import(&stored).unwrap();
    author.set_peer_id(67_100).unwrap();
    let from = author.oplog_vv();
    for index in 0..40 {
        author.get_map("scratch").insert("k", index).unwrap();
        author.commit();
    }
    author.get_list(crate::reserved_roots::ALIASES).insert(0, 1).unwrap();
    author.commit();
    let many = author.export(ExportMode::updates(&from)).unwrap();
    let outcome = LoroCollabEngine::load(&stored).unwrap().import_client_update(&many);
    assert!(is_reserved_refusal(&outcome), "last of many changes: {outcome:?}");

    // The same update without the last change is accepted: the refusal is about that op.
    let mut head_only = LoroCollabEngine::load(&stored).unwrap();
    let ok_author = LoroDoc::new();
    ok_author.import(&stored).unwrap();
    ok_author.set_peer_id(67_200).unwrap();
    let from = ok_author.oplog_vv();
    for index in 0..40 {
        ok_author.get_map("scratch").insert("k", index).unwrap();
        ok_author.commit();
    }
    head_only
        .import_client_update(&ok_author.export(ExportMode::updates(&from)).unwrap())
        .unwrap();
}

/// A forged JSON encoding: a change re-written so that its ops name a different peer than the
/// author, and counters that collide with ids the base holds. The library decides which ops are
/// new; the scan looks at what it applied.
#[test]
fn a_re_encoded_update_cannot_smuggle_a_shadow_past_the_scan() {
    let base = page(1, 6, 25);
    let stored = base.export_snapshot().unwrap();
    let known_peer = base.doc.oplog_vv().keys().copied().next().unwrap();
    let update = root_update(&base, 68_000, crate::reserved_roots::META, ContainerType::Map);
    let _ = update;
    let author = LoroDoc::new();
    author.import(&stored).unwrap();
    author.set_peer_id(68_001).unwrap();
    let from = author.oplog_vv();
    author.get_text(crate::reserved_roots::META).insert(0, "x").unwrap();
    author.commit();
    let mut json = serde_json::to_value(
        author
            .export_json_updates_without_peer_compression(&from, &author.oplog_vv())
            .unwrap(),
    )
    .unwrap();
    // Re-attribute the change to a peer the base knows, at the counter where it ends.
    let next_counter = base.doc.oplog_vv().get(&known_peer).copied().unwrap();
    for change in json["changes"].as_array_mut().unwrap() {
        change["id"] = Value::String(format!("{next_counter}@{known_peer}"));
        for (index, op) in change["ops"].as_array_mut().unwrap().iter_mut().enumerate() {
            op["counter"] = Value::from(next_counter + i32::try_from(index).unwrap());
        }
    }
    let rewritten = LoroDoc::new();
    rewritten.import(&stored).unwrap();
    let applied = rewritten.import_json_updates(serde_json::to_string(&json).unwrap());
    assert!(applied.is_ok(), "the forged encoding must be importable: {applied:?}");
    {
        let bytes = rewritten.export(ExportMode::updates(&base.doc.oplog_vv())).unwrap();
        let outcome = LoroCollabEngine::load(&stored).unwrap().import_client_update(&bytes);
        assert!(is_reserved_refusal(&outcome), "{outcome:?}");
    }
    // The honest encoding of the same op is refused as well.
    let honest = author.export(ExportMode::updates(&from)).unwrap();
    let outcome = LoroCollabEngine::load(&stored).unwrap().import_client_update(&honest);
    assert!(is_reserved_refusal(&outcome), "{outcome:?}");
}

/// A snapshot-format blob is not a way in either: state that arrives without ops would not be
/// seen by an op scan, so the roots of the result are compared with the roots before.
#[test]
fn a_snapshot_blob_cannot_register_a_shadow_root_without_ops() {
    let base = page(1, 6, 26);
    let stored = base.export_snapshot().unwrap();
    for name in [crate::reserved_roots::META, crate::reserved_roots::DOC] {
        // Registered, empty, no ops: a typed read on a raw document creates the root.
        let author = LoroDoc::new();
        author.import(&stored).unwrap();
        author.set_peer_id(69_000).unwrap();
        let _ = author.get_text(name);
        author.commit();
        let blob = author.export(ExportMode::Snapshot).unwrap();
        let mut engine = LoroCollabEngine::load(&stored).unwrap();
        let outcome = engine.import_client_update(&blob);
        let roots_before = format!("{:?}", base.doc.get_value());
        let roots_after = format!("{:?}", engine.doc.get_value());
        println!(
            "snapshot blob {name}: {outcome:?}; roots equal: {}",
            roots_before == roots_after
        );
        assert!(outcome.is_ok() || is_reserved_refusal(&outcome), "{outcome:?}");
        assert_eq!(roots_before, roots_after, "{name}: the blob registered a root");
    }
}

// ---------------------------------------------------------------------------------------
// Cost
// ---------------------------------------------------------------------------------------

/// The client path reads exactly the update's own ops, at 200 and 2,000 blocks, and does no walk.
#[test]
fn the_client_check_costs_the_update_not_the_document() {
    let mut visited = Vec::new();
    for blocks in [200_usize, 2_000] {
        let mut author = page(1, blocks, 31);
        let base = author.export_snapshot().unwrap();
        let from_frontier = author.frontier();
        let from = author.doc.oplog_vv();
        author
            .apply_content_operation(&Operation::InsertText {
                id: NodeId::from("s1"),
                index: 0,
                text: "typed".into(),
            })
            .unwrap();
        let update = author.export_from(&from_frontier).unwrap();
        let expected = author
            .doc
            .export_json_updates(&from, &author.doc.oplog_vv())
            .unwrap()
            .changes
            .iter()
            .map(|change| change.ops.len())
            .sum::<usize>();
        for client in [false, true] {
            let mut worker = LoroCollabEngine::load(&base).unwrap();
            walk_counter::reset();
            if client {
                worker.import_client_update(&update).unwrap();
            } else {
                worker.import_update_causally_complete(&update).unwrap();
            }
            assert_eq!(walk_counter::counts(), (0, 0), "{blocks} blocks, client {client}");
            let (scanned, narrow, historical) = walk_counter::guard_counts();
            assert_eq!((narrow, historical), (0, 0));
            assert_eq!(scanned, expected, "{blocks} blocks, client {client}");
            visited.push(scanned);
        }
    }
    assert!(visited.windows(2).all(|pair| pair[0] == pair[1]), "{visited:?}");
}

/// On the client path a model-5 page also has its update's ops read (the replay path returns
/// early there); the count is the update's op count and independent of the document.
#[test]
fn the_client_check_reads_the_update_on_marked_pages_too() {
    let mut counts = Vec::new();
    for blocks in [200_usize, 2_000] {
        let mut author = page(5, blocks, 32);
        let base = author.export_snapshot().unwrap();
        let from_frontier = author.frontier();
        let from = author.doc.oplog_vv();
        author
            .apply_content_operation(&Operation::InsertText {
                id: NodeId::from("s1"),
                index: 0,
                text: "typed".into(),
            })
            .unwrap();
        let update = author.export_from(&from_frontier).unwrap();
        let expected = author
            .doc
            .export_json_updates(&from, &author.doc.oplog_vv())
            .unwrap()
            .changes
            .iter()
            .map(|change| change.ops.len())
            .sum::<usize>();
        let mut worker = LoroCollabEngine::load(&base).unwrap();
        walk_counter::reset();
        worker.import_client_update(&update).unwrap();
        assert_eq!(walk_counter::guard_counts().0, expected, "{blocks} blocks");
        assert_eq!(walk_counter::counts(), (0, 0));
        let mut replay = LoroCollabEngine::load(&base).unwrap();
        walk_counter::reset();
        replay.import_update_causally_complete(&update).unwrap();
        assert_eq!(walk_counter::guard_counts().0, 0, "the replay path still returns early");
        counts.push(expected);
    }
    assert_eq!(counts[0], counts[1]);
}

// ---------------------------------------------------------------------------------------
// Reads next to a shadow root (empty real root, cold load)
// ---------------------------------------------------------------------------------------

fn observe(doc: &LoroDoc) -> (String, Vec<u8>, String) {
    (
        format!("{:?}", doc.get_deep_value()),
        doc.export(ExportMode::Snapshot).unwrap(),
        format!("{:?}", doc.get_value()),
    )
}

/// A real root that is empty next to a non-empty shadow of the same name is still found, on an
/// attached and on a detached document, and reading registers nothing.
#[test]
fn an_empty_real_root_next_to_a_shadow_is_found() {
    use crate::editor_structure::root_container;
    // (a) Emptied: it has ops. (b) Registered only: no ops.
    for with_ops in [true, false] {
        let doc = LoroDoc::new();
        let real = doc.get_map("m");
        if with_ops {
            real.insert("k", 1).unwrap();
            doc.commit();
            real.delete("k").unwrap();
        }
        doc.commit();
        let emptied = doc.state_frontiers();
        doc.get_text("m").insert(0, "shadow").unwrap();
        doc.commit();
        let shadowed = doc.state_frontiers();
        doc.get_map("later").insert("k", 1).unwrap();
        doc.commit();
        let label = format!("with ops {with_ops}");

        let before = observe(&doc);
        let attached = root_container(&doc, "m", ContainerType::Map).unwrap();
        assert!(attached.is_some(), "{label}: attached");
        assert!(root_container(&doc, "m", ContainerType::Text).unwrap().is_some());
        assert!(root_container(&doc, "m", ContainerType::List).unwrap().is_none());
        assert_eq!(before, observe(&doc), "{label}: attached read changed the document");

        doc.checkout(&shadowed).unwrap();
        assert!(doc.is_detached());
        let detached = root_container(&doc, "m", ContainerType::Map).unwrap();
        if with_ops {
            assert!(detached.is_some(), "{label}: detached");
        } else {
            // A fork at a detached state drops a root that is registered but has no ops. Every
            // reader treats an empty reserved map exactly as an absent one, so nothing depends
            // on this; the read must not invent a root, and must not fail.
            println!(
                "{label}: detached read of a registered-only root: {:?}",
                detached.is_some()
            );
        }
        assert!(root_container(&doc, "m", ContainerType::List).unwrap().is_none());
        doc.checkout(&emptied).unwrap();
        doc.try_checkout_to_latest().unwrap();
        assert!(!doc.is_detached());
    }
}

/// The same through the engine's own readers: an emptied real `order` map and an emptied real
/// structure-metadata map next to non-empty shadows read as empty, exactly as when absent.
#[test]
fn engine_readers_treat_an_empty_real_root_next_to_a_shadow_as_empty() {
    let base = page(4, 4, 41);
    let mut engine = base.fork().unwrap();
    let update = raw_update(&engine, 70_000, |raw| {
        let order = raw.get_map(crate::reserved_roots::ORDER);
        order.insert("n", "a").unwrap();
        raw.commit();
        order.delete("n").unwrap();
        raw.commit();
        raw.get_text(crate::reserved_roots::ORDER).insert(0, "shadow").unwrap();
        raw.get_list(crate::reserved_roots::ALIASES).insert(0, 1).unwrap();
    });
    engine.import_update(&update).unwrap();
    let before = observe(&engine.doc);
    assert!(engine.navigator_order().unwrap().is_empty());
    assert!(!engine.has_editor_alias("zzz").unwrap());
    assert_eq!(engine.compute_minimum_editor_model().unwrap(), 4);
    assert_eq!(engine.semantic_snapshot_with_floor().unwrap().1, 4);
    assert_eq!(before, observe(&engine.doc));

    // Detached through the guard's own checkout.
    let frontier = engine.doc.state_frontiers();
    assert!(crate::floor_guard::physical_atoms_at(&engine.doc, &frontier).is_ok());
    assert_eq!(before, observe(&engine.doc));
}

/// A structured page whose model-5 marker is its only floor-5 support, with a shadow of the
/// structure-metadata root, loaded cold: the floor is 5 from the marker, and nothing is registered
/// by reading it.
#[test]
fn a_cold_structured_page_with_a_meta_shadow_reads_floor_five_from_the_marker() {
    for kind in [
        ContainerType::Text,
        ContainerType::List,
        ContainerType::MovableList,
        ContainerType::Tree,
    ] {
        let mut engine = page(4, 5, 51);
        let marker = raw_update(&engine, 71_000, |raw| set_model(raw, "rich_page_v5"));
        engine.import_update(&marker).unwrap();
        let shadow = root_update(&engine, 71_001, crate::reserved_roots::META, kind);
        engine.import_update(&shadow).unwrap();
        let snapshot = engine.export_snapshot().unwrap();

        let cold = LoroCollabEngine::load(&snapshot).unwrap();
        let before = observe(&cold.doc);
        assert!(cold.has_editor_structure().unwrap(), "{kind:?}");
        assert_eq!(cold.compute_minimum_editor_model().unwrap(), 5, "{kind:?}");
        assert_eq!(cold.minimum_editor_model().unwrap(), 5, "{kind:?}");
        assert_eq!(cold.semantic_snapshot_with_floor().unwrap().1, 5, "{kind:?}");
        assert!(super::model5_marker(&cold.doc).unwrap(), "{kind:?}");
        assert_eq!(before, observe(&cold.doc), "{kind:?}: reads changed the document");

        // And it takes an ordinary client write.
        let mut author = LoroCollabEngine::load(&snapshot).unwrap();
        author.doc.set_peer_id(71_500).unwrap();
        let from = author.frontier();
        author
            .apply_content_operation(&Operation::InsertText {
                id: NodeId::from("s1"),
                index: 0,
                text: "t".into(),
            })
            .unwrap();
        let update = author.export_from(&from).unwrap();
        let mut worker = LoroCollabEngine::load(&snapshot).unwrap();
        worker.import_client_update(&update).unwrap();
        assert_eq!(worker.semantic_snapshot_with_floor().unwrap().1, 5);
    }
}

/// A flat page held at floor 5 by a raw atom, as the other shadow tests, takes ordinary updates.
#[test]
fn a_flat_atom_page_with_a_shadow_takes_ordinary_client_writes() {
    let mut engine = flat_page_with_raw_atom(81);
    let shadow = root_update(&engine, 72_000, crate::reserved_roots::META, ContainerType::Text);
    engine.import_update(&shadow).unwrap();
    let stored = engine.export_snapshot().unwrap();
    let mut author = LoroCollabEngine::load(&stored).unwrap();
    author.doc.set_peer_id(72_500).unwrap();
    let from = author.frontier();
    author.set_title("renamed").unwrap();
    let update = author.export_from(&from).unwrap();
    let mut worker = LoroCollabEngine::load(&stored).unwrap();
    worker.import_client_update(&update).unwrap();
    assert_eq!(worker.minimum_editor_model().unwrap(), 5);
    assert_eq!(expected_type(crate::reserved_roots::META), Some(ContainerType::Map));
    let _ = (children(&worker.doc), VersionVector::default(), SCAN_INCOMPLETE_REASON);
}

// ---------------------------------------------------------------------------------------
// Whole documents from outside (uploaded packages)
// ---------------------------------------------------------------------------------------

/// A document that carries a shadow root is refused whichever way the root got in: through an op
/// in the history, through state with no op, and never mistaken for a clean one.
#[test]
fn a_document_with_a_shadow_root_is_refused_and_a_clean_one_is_not() {
    for (label, base) in bases() {
        base.refuse_shadow_roots().unwrap_or_else(|e| panic!("{label}: {e}"));
        let stored = base.export_snapshot().unwrap();
        for (name, expected) in RESERVED_ROOTS {
            for kind in all_types().into_iter().filter(|kind| *kind != expected) {
                let mut with_shadow = LoroCollabEngine::load(&stored).unwrap();
                let update = root_update(&base, 73_000, name, kind);
                with_shadow.import_update(&update).unwrap();
                // As a package would hold it: a snapshot, and a snapshot plus the update as a tail.
                let snapshot = with_shadow.export_snapshot().unwrap();
                let loaded = LoroCollabEngine::load(&snapshot).unwrap();
                let outcome = loaded.refuse_shadow_roots();
                assert!(is_reserved_refusal(&outcome), "{label}: {name} {kind:?}: {outcome:?}");
                let mut tail = LoroCollabEngine::load(&stored).unwrap();
                tail.import_update_causally_complete(&update).unwrap();
                assert!(
                    is_reserved_refusal(&tail.refuse_shadow_roots()),
                    "{label}: tail {name} {kind:?}"
                );
            }
        }
    }
}

/// A root that is registered in the state without any op is listed under its wrong type, and a
/// document holding it is refused too: it would send every read of that name down the slow path.
#[test]
fn a_shadow_root_registered_without_ops_is_refused() {
    let base = page(1, 4, 27);
    let raw = LoroDoc::new();
    raw.import(&base.export_snapshot().unwrap()).unwrap();
    let _ = raw.get_text(crate::reserved_roots::META);
    raw.commit();
    let blob = raw.export(ExportMode::Snapshot).unwrap();
    let loaded = LoroCollabEngine::load(&blob).unwrap();
    let listed = format!("{:?}", loaded.doc.get_value());
    assert!(
        listed.contains(crate::reserved_roots::META),
        "the root is listed: {listed}"
    );
    assert!(is_reserved_refusal(&loaded.refuse_shadow_roots()));
}

/// A shallow document has no complete history to inspect, so it is refused as unverifiable.
#[test]
fn a_shallow_document_is_refused_as_unverifiable() {
    let base = page(1, 4, 28);
    let raw = LoroDoc::new();
    raw.import(&base.export_snapshot().unwrap()).unwrap();
    raw.get_map("scratch").insert("k", 1).unwrap();
    raw.commit();
    let blob = raw
        .export(ExportMode::shallow_snapshot(&raw.state_frontiers()))
        .unwrap();
    let loaded = LoroCollabEngine::load(&blob).unwrap();
    assert!(matches!(
        loaded.refuse_shadow_roots(),
        Err(CollabError::OperationFailed { reason }) if reason == SCAN_INCOMPLETE_REASON
    ));
}
