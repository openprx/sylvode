//! Fixtures for the classes of shape that the review of the incremental window found the first
//! corpus could not produce: updates that are hand-encoded (tree ops the API cannot author),
//! updates that write the identity or kind of nodes that exist, updates that make ids collide,
//! containers the page models do not use, pages grown past what the incremental path may take,
//! updates that touch every block and then cross a limit, and base facts that do not describe the
//! base. They are generated for every base of the matching model, and a few fixed shapes add the
//! sizes and numbers the generators do not reach.
//!
//! Nothing here decides anything: every case is run by the full computation and by the incremental
//! window, and the harness demands that they agree.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::print_stdout
)]

use std::sync::Arc;

use loro::{
    Container, LoroCounter, LoroList, LoroMap, LoroMovableList, LoroText, TreeID, TreeParentId, ValueOrContainer,
};

use super::floor_equivalence_tests::children;
use super::hand_encoded::{accepted, create_unborn, raw_edit, set_node, tree_id_of, unborn_update};
use super::window_fixtures::{Built, Case, FactsFrom, Model, Rng, hostile};
use super::{CollabEngine, LoroCollabEngine};
use crate::operation::NodeKind;

/// A generated case on a base other than the spec's own snapshot (a base that already holds the
/// first of two updates).
fn case_on(built: &Built, kind: &'static str, base: &Arc<Vec<u8>>, update: Vec<u8>) -> Case {
    Case {
        label: format!("{} :: {kind}", built.spec.label()),
        family: "generated",
        kind,
        base: Arc::clone(base),
        update,
        expect: None,
        facts: FactsFrom::Base,
    }
}

/// The logical ids of the live legacy tree nodes of `built`, in a stable order.
fn tree_ids(built: &Built) -> Vec<String> {
    let engine = LoroCollabEngine::load(&built.snapshot).unwrap();
    // Only live nodes: a node cannot take children, or have its metadata written, once deleted.
    let mut ids: Vec<String> = engine
        .id_to_tree
        .iter()
        .filter(|(_, node)| engine.tree.is_node_deleted(node) == Ok(false))
        .map(|(id, _)| id.to_string())
        .collect();
    ids.sort();
    ids
}

fn block_count(built: &Built) -> usize {
    LoroCollabEngine::load(&built.snapshot)
        .unwrap()
        .root_list_len()
        .unwrap_or(0)
}

/// The inline text container of block `index` in `raw`, when it has one.
fn first_inline_text(raw: &loro::LoroDoc, index: usize) -> Option<LoroText> {
    let ValueOrContainer::Container(Container::Map(map)) = children(raw).get(index)? else {
        return None;
    };
    let ValueOrContainer::Container(Container::List(inline)) = map.get("children")? else {
        return None;
    };
    match inline.get(0)? {
        ValueOrContainer::Container(Container::Text(text)) => Some(text),
        _ => None,
    }
}

fn block_at(raw: &loro::LoroDoc, index: usize) -> Option<LoroMap> {
    match children(raw).get(index)? {
        ValueOrContainer::Container(Container::Map(map)) => Some(map),
        _ => None,
    }
}

/// Every class of shape, for one base.
#[allow(clippy::too_many_lines)]
pub(super) fn class_cases(built: &Built, rng: &mut Rng) -> Vec<Case> {
    let mut cases = Vec::new();
    let model = built.spec.model;
    let base = &built.snapshot;
    let seed = built.spec.seed;
    let peer_base = 6_000_000 + seed * 1_000 + (built.spec.blocks as u64) * 10;
    let mut next_peer = {
        let mut peer = peer_base;
        move || {
            peer += 1;
            peer
        }
    };

    // --- classes of tree shapes (pages with a legacy tree) ---
    if matches!(model, Model::LegacyTree | Model::Mixed) {
        let ids = tree_ids(built);
        let pick = |rng: &mut Rng| ids[rng.index(ids.len())].clone();
        if !ids.is_empty() {
            let host = tree_id_of(base, &pick(rng));
            let host2 = tree_id_of(base, &pick(rng));
            // Hand-encoded: a node the base holds only through a move, named by a later create.
            let (px, p) = (next_peer(), next_peer());
            let (update1, x) = unborn_update(base, px, p, |author, x| {
                let tree = author.get_tree("tree");
                tree.enable_fractional_index(0);
                tree.mov(x, TreeParentId::Node(host)).unwrap();
            });
            let base1 = accepted(base, &update1);
            cases.push(case_on(built, "hand_encoded_move_of_unborn_node", base, update1));
            cases.push(case_on(
                built,
                "create_of_node_held_by_a_move",
                &base1,
                create_unborn(&base1, px, Some(host2), "late_x", x),
            ));
            // The same node, with named children hanging from it.
            let (px, p) = (next_peer(), next_peer());
            let (update1, x) = unborn_update(base, px, p, |author, x| {
                let tree = author.get_tree("tree");
                tree.enable_fractional_index(0);
                tree.mov(x, TreeParentId::Node(host)).unwrap();
                let c0 = tree.create(TreeParentId::Node(x)).unwrap();
                set_node(author, c0, &format!("hc0_{px}"), "navigator_node");
                let c1 = tree.create(TreeParentId::Node(c0)).unwrap();
                set_node(author, c1, &format!("hc1_{px}"), "block");
            });
            let base1 = accepted(base, &update1);
            cases.push(case_on(
                built,
                "create_of_node_with_hidden_children",
                &base1,
                create_unborn(&base1, px, Some(host2), "late_parent", x),
            ));
            cases.push(case_on(
                built,
                "create_of_node_with_hidden_children_at_the_root",
                &base1,
                raw_edit(&base1, px, |raw| {
                    let tree = raw.get_tree("tree");
                    tree.enable_fractional_index(0);
                    assert_eq!(tree.create(TreeParentId::Root).unwrap(), x);
                }),
            ));
            // The same, deleted in the base: re-creating its root revives the subtree.
            let (px, p) = (next_peer(), next_peer());
            let (update1, x) = unborn_update(base, px, p, |author, x| {
                let tree = author.get_tree("tree");
                tree.enable_fractional_index(0);
                tree.mov(x, TreeParentId::Node(host)).unwrap();
                for index in 0..3 {
                    let n = tree.create(TreeParentId::Node(x)).unwrap();
                    set_node(author, n, &format!("hd{index}_{px}"), "navigator_node");
                }
                tree.delete(x).unwrap();
            });
            let base1 = accepted(base, &update1);
            cases.push(case_on(
                built,
                "create_revives_deleted_subtree",
                &base1,
                create_unborn(&base1, px, Some(host2), "revived", x),
            ));
            // An existing node moves under a parent that does not exist yet (the move cannot
            // apply); the parent is created later.
            let (px, p) = (next_peer(), next_peer());
            let (update1, x) = unborn_update(base, px, p, |author, x| {
                let tree = author.get_tree("tree");
                tree.enable_fractional_index(0);
                if tree.contains(host) {
                    let _ = tree.mov(host, TreeParentId::Node(x));
                }
            });
            let base1 = accepted(base, &update1);
            cases.push(case_on(
                built,
                "create_of_a_parent_an_earlier_move_waits_for",
                &base1,
                create_unborn(&base1, px, None, "waited_for", x),
            ));

            // Identity and kind of nodes that exist.
            let victim = pick(rng);
            let other = pick(rng);
            let victim_id = tree_id_of(base, &victim);
            for (kind, value) in [
                ("rewrite_logical_id_to_another_live_id", Some(other.clone())),
                ("rewrite_logical_id_to_a_fresh_id", Some(format!("fresh_{peer_base}"))),
                ("delete_logical_id", None),
            ] {
                let p = next_peer();
                cases.push(case_on(
                    built,
                    kind,
                    base,
                    raw_edit(base, p, |raw| {
                        let meta = raw.get_tree("tree").get_meta(victim_id).unwrap();
                        match &value {
                            Some(value) => meta.insert("logical_id", value.as_str()).unwrap(),
                            None => meta.delete("logical_id").unwrap(),
                        }
                    }),
                ));
            }
            for (kind, to) in [
                ("rewrite_kind_to_navigator", "navigator_node"),
                ("rewrite_kind_to_block", "block"),
                ("rewrite_kind_to_unknown", "no_such_kind"),
            ] {
                let p = next_peer();
                cases.push(case_on(
                    built,
                    kind,
                    base,
                    raw_edit(base, p, |raw| {
                        raw.get_tree("tree")
                            .get_meta(victim_id)
                            .unwrap()
                            .insert("kind", to)
                            .unwrap();
                    }),
                ));
            }
            // `content_source` decides whether the projection keeps a node on a page with an
            // editor document: set it, and (a second update) remove it again.
            let p = next_peer();
            let set_source = raw_edit(base, p, |raw| {
                raw.get_tree("tree")
                    .get_meta(victim_id)
                    .unwrap()
                    .insert("prop:content_source", "editor_doc")
                    .unwrap();
            });
            let dropped = accepted(base, &set_source);
            cases.push(case_on(built, "set_content_source_on_existing_node", base, set_source));
            let p = next_peer();
            cases.push(case_on(
                built,
                "remove_content_source_from_dropped_node",
                &dropped,
                raw_edit(&dropped, p, |raw| {
                    raw.get_tree("tree")
                        .get_meta(victim_id)
                        .unwrap()
                        .delete("prop:content_source")
                        .unwrap();
                }),
            ));
            // A node re-named in the same update that gives its text a mergeable container.
            let p = next_peer();
            cases.push(case_on(
                built,
                "mergeable_text_and_counter_in_node_meta",
                base,
                raw_edit(base, p, |raw| {
                    let meta = raw.get_tree("tree").get_meta(victim_id).unwrap();
                    meta.ensure_mergeable_text("extra")
                        .unwrap()
                        .insert(0, "mergeable")
                        .unwrap();
                    let counter = meta.insert_container("counter", LoroCounter::new()).unwrap();
                    counter.increment(2.0).unwrap();
                    meta.insert_container("movable", LoroMovableList::new())
                        .unwrap()
                        .insert(0, 1)
                        .unwrap();
                }),
            ));

            // Created nodes whose ids collide.
            let create_named = |raw: &loro::LoroDoc, parent: Option<TreeID>, logical: &str| {
                let tree = raw.get_tree("tree");
                tree.enable_fractional_index(0);
                let node = tree
                    .create(parent.map_or(TreeParentId::Root, TreeParentId::Node))
                    .unwrap();
                set_node(raw, node, logical, "navigator_node");
            };
            let p = next_peer();
            cases.push(case_on(
                built,
                "create_with_an_existing_logical_id",
                base,
                raw_edit(base, p, |raw| create_named(raw, Some(host), &other)),
            ));
            let p = next_peer();
            cases.push(case_on(
                built,
                "create_two_nodes_with_one_logical_id",
                base,
                raw_edit(base, p, |raw| {
                    create_named(raw, None, "twin");
                    create_named(raw, Some(host), "twin");
                }),
            ));
            let p = next_peer();
            cases.push(case_on(
                built,
                "create_with_a_container_id_as_logical_id",
                base,
                raw_edit(base, p, |raw| create_named(raw, None, "cid:1@1:Map")),
            ));
            let p = next_peer();
            cases.push(case_on(
                built,
                "create_under_a_container_id_named_node",
                base,
                raw_edit(base, p, |raw| {
                    let tree = raw.get_tree("tree");
                    tree.enable_fractional_index(0);
                    let parent = tree.create(TreeParentId::Root).unwrap();
                    set_node(raw, parent, "cid:2@2:Map", "block");
                    let child = tree.create(TreeParentId::Node(parent)).unwrap();
                    set_node(raw, child, "under_cid", "navigator_node");
                }),
            ));
            if model == Model::Mixed
                && let Some(cid) = LoroCollabEngine::load(base)
                    .unwrap()
                    .root_block_ids()
                    .unwrap()
                    .and_then(|ids| ids.first().map(super::floor_equivalence_tests::id_text))
            {
                let p = next_peer();
                cases.push(case_on(
                    built,
                    "create_named_like_an_existing_block",
                    base,
                    raw_edit(base, p, |raw| create_named(raw, None, &cid)),
                ));
            }
            // Tree nodes without a logical id: many of them, accepted and counted by no limit.
            let p = next_peer();
            cases.push(case_on(
                built,
                "create_many_unnamed_nodes",
                base,
                raw_edit(base, p, |raw| {
                    let tree = raw.get_tree("tree");
                    tree.enable_fractional_index(0);
                    let mut parent = TreeParentId::Node(host);
                    for index in 0..120 {
                        let node = tree
                            .create(if index % 30 == 0 { TreeParentId::Root } else { parent })
                            .unwrap();
                        parent = TreeParentId::Node(node);
                    }
                }),
            ));
        }
    }

    // --- classes of block shapes (pages with an editor document and no placement tree) ---
    if matches!(model, Model::Flat | Model::Mixed) && block_count(built) > 0 {
        let blocks = block_count(built);
        let at = rng.index(blocks);
        // An element inserted and deleted in one update.
        let p = next_peer();
        cases.push(case_on(
            built,
            "insert_then_delete_the_same_element",
            base,
            raw_edit(base, p, |raw| {
                let list = children(raw);
                let map = list.insert_container(at.min(list.len()), LoroMap::new()).unwrap();
                map.insert("nodeName", "paragraph").unwrap();
                map.insert_container("children", LoroList::new()).unwrap();
                list.delete(at.min(list.len().saturating_sub(1)), 1).unwrap();
            }),
        ));
        let p = next_peer();
        cases.push(case_on(
            built,
            "delete_one_insert_two_then_delete_one",
            base,
            raw_edit(base, p, |raw| {
                let list = children(raw);
                list.delete(at, 1).unwrap();
                for _ in 0..2 {
                    let map = list.insert_container(0, LoroMap::new()).unwrap();
                    map.insert("nodeName", "paragraph").unwrap();
                    map.insert_container("children", LoroList::new()).unwrap();
                }
                list.delete(0, 1).unwrap();
            }),
        ));
        // Containers the page models do not use, inside a block.
        let p = next_peer();
        cases.push(case_on(
            built,
            "counter_and_movable_list_in_block_attributes",
            base,
            raw_edit(base, p, |raw| {
                let block = block_at(raw, at).unwrap();
                let attrs = block.insert_container("attributes", LoroMap::new()).unwrap();
                attrs
                    .insert_container("n", LoroCounter::new())
                    .unwrap()
                    .increment(3.0)
                    .unwrap();
                attrs
                    .insert_container("ml", LoroMovableList::new())
                    .unwrap()
                    .insert(0, 1)
                    .unwrap();
            }),
        ));
        let p = next_peer();
        cases.push(case_on(
            built,
            "mergeable_text_in_a_block_map",
            base,
            raw_edit(base, p, |raw| {
                block_at(raw, at)
                    .unwrap()
                    .ensure_mergeable_text("extra")
                    .unwrap()
                    .insert(0, "hello")
                    .unwrap();
            }),
        ));
        // Block maps no reader supports, and prose blocks that are dependency nodes.
        let p = next_peer();
        cases.push(case_on(
            built,
            "unsupported_and_dependency_block_maps",
            base,
            raw_edit(base, p, |raw| {
                let list = children(raw);
                for index in 0..40 {
                    let map = list.insert_container(list.len(), LoroMap::new()).unwrap();
                    if index % 2 == 0 {
                        map.insert("nodeName", "z").unwrap();
                    } else {
                        map.insert("nodeName", "paragraph").unwrap();
                        let attrs = map.insert_container("attributes", LoroMap::new()).unwrap();
                        attrs.insert("embed_object_id", "o1").unwrap();
                        map.insert_container("children", LoroList::new()).unwrap();
                    }
                }
            }),
        ));
        // Every block gains a character in one update, and an inline text is replaced.
        let p = next_peer();
        cases.push(case_on(
            built,
            "touch_every_block",
            base,
            raw_edit(base, p, |raw| {
                for index in 0..blocks {
                    if let Some(text) = first_inline_text(raw, index) {
                        text.insert(0, "t").unwrap();
                    }
                }
            }),
        ));
        let p = next_peer();
        cases.push(case_on(
            built,
            "touch_every_block_and_delete_half",
            base,
            raw_edit(base, p, |raw| {
                for index in 0..blocks {
                    if let Some(text) = first_inline_text(raw, index) {
                        text.insert(0, "t").unwrap();
                    }
                }
                let list = children(raw);
                for index in (0..blocks).rev().step_by(2) {
                    list.delete(index, 1).unwrap();
                }
            }),
        ));
        // A block map written with a key a tree node's identity uses.
        let p = next_peer();
        cases.push(case_on(
            built,
            "logical_id_key_on_a_block_map",
            base,
            raw_edit(base, p, |raw| {
                block_at(raw, at).unwrap().insert("logical_id", "elsewhere").unwrap();
            }),
        ));
    }

    // --- facts that do not describe the base ---
    if matches!(model, Model::LegacyTree | Model::Flat | Model::Mixed) {
        // The facts of the page before one more edit, applied to an update on the page after it.
        let p = next_peer();
        let older = raw_edit(base, p, |raw| {
            raw.get_map("meta").insert("title", "older").unwrap();
        });
        let newer = accepted(base, &older);
        let p = next_peer();
        let mut case = case_on(
            built,
            "facts_of_an_older_state",
            &newer,
            raw_edit(&newer, p, |raw| {
                raw.get_map("meta").insert("title", "newer").unwrap();
            }),
        );
        case.facts = FactsFrom::OtherBase(Arc::clone(base));
        cases.push(case);
    }
    // Kinds the engine knows are named in the tree cases above; this keeps the import of the
    // module's kinds visible.
    let _ = NodeKind::Block;
    cases
}

/// A flat page of `live` blocks of `chars` characters each.
fn flat_with_text(seed: u64, live: usize, chars: usize) -> Arc<Vec<u8>> {
    let engine = super::window_fixtures::big_flat_page(seed, live, 0, |_| "x".repeat(chars));
    Arc::new(engine.export_snapshot().unwrap())
}

/// `count` maps of a kind no reader supports, appended to the root list.
fn unsupported_maps(base: &[u8], peer: u64, count: usize) -> Vec<u8> {
    raw_edit(base, peer, |raw| {
        let list = children(raw);
        for _ in 0..count {
            let map = list.insert_container(list.len(), LoroMap::new()).unwrap();
            map.insert("nodeName", "z").unwrap();
        }
    })
}

/// The fixed shapes: the review's inputs, and the sizes the generators do not reach.
#[allow(clippy::too_many_lines)]
pub(super) fn class_hostile() -> Vec<Case> {
    let mut cases = Vec::new();

    // --- F1: a created node that already existed ---
    {
        const PEER_X: u64 = 4_242;
        let mut engine = LoroCollabEngine::new_empty(7);
        let create = |engine: &mut LoroCollabEngine, id: &str, parent: Option<&str>| {
            engine
                .apply_content_operation(&crate::operation::Operation::CreateNode {
                    id: crate::operation::NodeId::from(id),
                    parent: parent.map(crate::operation::NodeId::from),
                    index: 0,
                    kind: NodeKind::NavigatorNode,
                })
                .unwrap();
        };
        create(&mut engine, "a0", None);
        for i in 1..30 {
            create(&mut engine, &format!("a{i}"), Some(&format!("a{}", i - 1)));
        }
        let base0 = Arc::new(engine.export_snapshot().unwrap());
        let a0 = tree_id_of(&base0, "a0");
        let (update1, x) = unborn_update(&base0, PEER_X, 99, |author, x| {
            let tree = author.get_tree("tree");
            tree.enable_fractional_index(0);
            tree.mov(x, TreeParentId::Node(a0)).unwrap();
            let c0 = tree.create(TreeParentId::Node(x)).unwrap();
            set_node(author, c0, "c0", "navigator_node");
            let c1 = tree.create(TreeParentId::Node(c0)).unwrap();
            set_node(author, c1, "c1", "navigator_node");
            let c2 = tree.create(TreeParentId::Node(c1)).unwrap();
            set_node(author, c2, "c2", "navigator_node");
        });
        let base1 = accepted(&base0, &update1);
        let a29 = tree_id_of(&base1, "a29");
        cases.push(hostile(
            "tree: a late created parent deepens its hidden children to depth 33",
            "unborn_create",
            &base1,
            create_unborn(&base1, PEER_X, Some(a29), "x", x),
        ));
        cases.push(hostile(
            "tree: a late created parent leaves its hidden children at depth 32",
            "unborn_create",
            &base1,
            create_unborn(&base1, PEER_X, Some(tree_id_of(&base1, "a28")), "x", x),
        ));
    }
    {
        const PEER_X: u64 = 4_244;
        let mut engine = LoroCollabEngine::new_empty(16);
        engine
            .apply_content_operation(&crate::operation::Operation::CreateNode {
                id: crate::operation::NodeId::from("anchor"),
                parent: None,
                index: 0,
                kind: NodeKind::NavigatorNode,
            })
            .unwrap();
        let base0 = Arc::new(engine.export_snapshot().unwrap());
        let anchor = tree_id_of(&base0, "anchor");
        let (update1, x) = unborn_update(&base0, PEER_X, 98, |author, x| {
            let tree = author.get_tree("tree");
            tree.enable_fractional_index(0);
            tree.mov(x, TreeParentId::Node(anchor)).unwrap();
            for i in 0..20 {
                let n = tree.create(TreeParentId::Node(x)).unwrap();
                set_node(author, n, &format!("hidden{i}"), "navigator_node");
            }
            tree.delete(x).unwrap();
        });
        let base1 = accepted(&base0, &update1);
        cases.push(hostile(
            "tree: re-creating the unnamed root of a deleted subtree revives 20 navigators",
            "unborn_create",
            &base1,
            raw_edit(&base1, PEER_X, |raw| {
                let tree = raw.get_tree("tree");
                tree.enable_fractional_index(0);
                assert_eq!(tree.create(TreeParentId::Root).unwrap(), x);
            }),
        ));
    }

    // --- F3: block maps of unsupported kinds ---
    {
        let base0 = flat_with_text(11, 10, 10);
        let mut base = base0;
        for (index, count) in [2_500_usize; 5].into_iter().enumerate() {
            let update = unsupported_maps(&base, 20_001 + index as u64, count);
            base = accepted(&base, &update);
        }
        let update = unsupported_maps(&base, 20_010, 2_480);
        base = accepted(&base, &update);
        // 14,990 elements: below the length the incremental path takes, which is 15,000.
        cases.push(hostile(
            "list: 10 more unsupported maps keep a 14990-element list within what the incremental path takes",
            "oversized_list",
            &base,
            unsupported_maps(&base, 20_020, 10),
        ));
        cases.push(hostile(
            "list: 11 more unsupported maps take a 14990-element list over what the incremental path takes",
            "oversized_list",
            &base,
            unsupported_maps(&base, 20_021, 11),
        ));
        let over = accepted(&base, &unsupported_maps(&base, 20_022, 20));
        cases.push(hostile(
            "list: a keystroke on a 15010-element list",
            "oversized_list",
            &over,
            raw_edit(&over, 20_023, |raw| {
                first_inline_text(raw, 0).unwrap().insert(0, "k").unwrap();
            }),
        ));
    }

    // --- F5: a page that every block of an update touches, then crosses a limit ---
    {
        let base = flat_with_text(13, 1_000, 999);
        cases.push(hostile(
            "limit: every one of 1000 blocks gains one character, 1000 characters under the document limit",
            "touch_all_then_limit",
            &base,
            raw_edit(&base, 77_001, |raw| {
                for index in 0..1_000 {
                    first_inline_text(raw, index).unwrap().insert(0, "y").unwrap();
                }
            }),
        ));
        cases.push(hostile(
            "limit: every one of 1000 blocks gains two characters, 1000 characters over the document limit",
            "touch_all_then_limit",
            &base,
            raw_edit(&base, 77_002, |raw| {
                for index in 0..1_000 {
                    first_inline_text(raw, index).unwrap().insert(0, "yy").unwrap();
                }
            }),
        ));
        // One long block among 999 of 900 characters: the page is under both text limits.
        let long = super::window_fixtures::big_flat_page(14, 1_000, 0, |index| {
            "x".repeat(if index == 0 { 99_000 } else { 900 })
        });
        let long = Arc::new(long.export_snapshot().unwrap());
        cases.push(hostile(
            "limit: every block gains a character, one block 1 past the block limit and the document 100 past its own",
            "touch_all_then_limit",
            &long,
            raw_edit(&long, 77_003, |raw| {
                first_inline_text(raw, 0)
                    .unwrap()
                    .insert(0, &"z".repeat(1_001))
                    .unwrap();
                for index in 1..1_000 {
                    first_inline_text(raw, index).unwrap().insert(0, "y").unwrap();
                }
            }),
        ));
    }

    // --- facts that do not describe the base ---
    {
        let base = flat_with_text(17, 8, 20);
        let mut tamper_blocks: fn(&mut crate::facts::BaseFacts) = |facts| facts.blocks = 0;
        cases.push(Case {
            facts: FactsFrom::Tampered(tamper_blocks),
            ..hostile(
                "facts: a block is deleted from a page whose facts count no blocks",
                "tampered_facts",
                &base,
                raw_edit(&base, 77_010, |raw| children(raw).delete(0, 1).unwrap()),
            )
        });
        tamper_blocks = |facts| {
            facts.units.insert(
                crate::operation::NodeId::from("claimed"),
                crate::facts::UnitFact {
                    live: true,
                    kind: NodeKind::NavigatorNode,
                    chars: 0,
                    depth: 0,
                },
            );
        };
        let legacy = {
            let mut engine = LoroCollabEngine::new_empty(19);
            engine
                .apply_content_operation(&crate::operation::Operation::CreateNode {
                    id: crate::operation::NodeId::from("root"),
                    parent: None,
                    index: 0,
                    kind: NodeKind::NavigatorNode,
                })
                .unwrap();
            Arc::new(engine.export_snapshot().unwrap())
        };
        cases.push(Case {
            facts: FactsFrom::Tampered(tamper_blocks),
            ..hostile(
                "facts: a created node takes an id the facts already hold",
                "tampered_facts",
                &legacy,
                raw_edit(&legacy, 77_011, |raw| {
                    let tree = raw.get_tree("tree");
                    tree.enable_fractional_index(0);
                    let node = tree.create(TreeParentId::Root).unwrap();
                    set_node(raw, node, "claimed", "navigator_node");
                }),
            )
        });
    }
    cases
}
