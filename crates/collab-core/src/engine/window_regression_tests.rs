//! Regression tests for the findings of the adversarial review of the incremental write window:
//! hostile inputs that made the incremental check decide differently from the full computation, or
//! let a page grow past what the full computation allows.
//!
//! Each test builds its input from raw documents (`hand_encoded`), runs the full computation and
//! the worker's window on it, and asserts that the decision is the full computation's, that the
//! report of an incremental accept agrees with the host's projection of the accepted document (the
//! oracle), and what path the window took. The module depends only on interfaces that existed
//! before the fixes, and compares paths by name, so it can be run against the revision that was
//! reviewed: each test fails there.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::print_stdout
)]

use loro::{ExportMode, LoroDoc, LoroMap, TreeID, TreeParentId};

use super::floor_equivalence_tests::children;
use super::hand_encoded::{
    accepted, create_unborn, frontier_strings, raw_edit, rewrite_deps, set_node, tree_id_of, unborn_update,
};
use super::{CollabEngine, LoroCollabEngine};
use crate::editor_doc_bridge::{EditorInitialBlock, initialize_editor_doc};
use crate::facts::BaseFacts;
use crate::limits::{DocumentLimits, check_snapshot};
use crate::operation::{NodeId, NodeKind, Operation};
use crate::walk_counter;
use crate::window_check::{compare_with_projection, run_window};

fn create(engine: &mut LoroCollabEngine, id: &str, parent: Option<&str>, kind: NodeKind) {
    engine
        .apply_content_operation(&Operation::CreateNode {
            id: NodeId::from(id),
            parent: parent.map(NodeId::from),
            index: 0,
            kind,
        })
        .unwrap();
}

/// What the full computation and the worker's window made of one update.
struct Judged {
    /// Today's full computation: `Ok`, or the refusal as text.
    full: Result<(), String>,
    /// The window: `Ok` (with whether the check decided incrementally) or the refusal as text.
    window: Result<bool, String>,
    /// The name of the fallback reason, when the window fell back.
    fallback: Option<String>,
    /// How many fallbacks to the full computation the window ran.
    fallbacks: usize,
    /// The host's oracle on the report, when there is one.
    oracle: Option<String>,
}

impl Judged {
    /// The window decided as the full computation did, and its report (if any) holds.
    fn assert_agrees(&self, label: &str) {
        assert_eq!(self.full.is_ok(), self.window.is_ok(), "{label}: {self:?}");
        if let (Err(full), Err(window)) = (&self.full, &self.window) {
            assert_eq!(full, window, "{label}: the refusals differ");
        }
        if let Some(oracle) = &self.oracle {
            assert_eq!(oracle, "Ok(())", "{label}: the oracle disagrees: {self:?}");
        }
    }

    fn incremental(&self) -> bool {
        self.window == Ok(true)
    }
}

impl std::fmt::Debug for Judged {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "full {:?} | window {:?} | fallback {:?} ({} run) | oracle {:?}",
            self.full, self.window, self.fallback, self.fallbacks, self.oracle
        )
    }
}

fn full_decision(base: &[u8], update: &[u8]) -> Result<(), String> {
    let mut engine = LoroCollabEngine::load(base).unwrap();
    engine.next_import_client_update(update).map_err(|e| e.to_string())?;
    let semantic = engine.semantic_snapshot().map_err(|e| e.to_string())?;
    check_snapshot(&semantic, &DocumentLimits::default())
        .map_err(|e| crate::error::CollabError::from(e).to_string())?;
    Ok(())
}

fn judge(base: &[u8], update: &[u8]) -> Judged {
    let full = full_decision(base, update);
    let mut engine = LoroCollabEngine::load(base).unwrap();
    engine.prewarm_for_writes();
    let facts = BaseFacts::compute(&engine);
    walk_counter::reset();
    let outcome = run_window(&mut engine, facts.as_ref(), update);
    let fallbacks = walk_counter::window_counts().fallbacks;
    match outcome {
        Ok(accepted) => {
            let oracle = accepted.report.as_ref().map(|report| {
                let host = LoroCollabEngine::load(&engine.export_snapshot().unwrap()).unwrap();
                match host.semantic_snapshot() {
                    Ok(projection) => format!("{:?}", compare_with_projection(report, &projection)),
                    Err(e) => format!("host projection error {e:?}"),
                }
            });
            Judged {
                full,
                window: Ok(accepted.report.is_some()),
                fallback: accepted.fallback.map(|reason| format!("{reason:?}")),
                fallbacks,
                oracle,
            }
        }
        Err(e) => Judged {
            full,
            window: Err(e.to_string()),
            fallback: None,
            fallbacks,
            oracle: None,
        },
    }
}

fn legacy_chain(peer: u64, length: usize) -> LoroCollabEngine {
    let mut engine = LoroCollabEngine::new_empty(peer);
    create(&mut engine, "a0", None, NodeKind::NavigatorNode);
    for i in 1..length {
        create(
            &mut engine,
            &format!("a{i}"),
            Some(&format!("a{}", i - 1)),
            NodeKind::NavigatorNode,
        );
    }
    engine
}

// ---------------------------------------------------------------------------------------------
// F1: a created node that already existed adopts units the check believes untouched
// ---------------------------------------------------------------------------------------------

/// A node X exists in the base only through a hand-encoded move (its create op is not in the
/// base) and carries named children, whose depth starts at 0 because X has no logical id. The
/// update's first op from X's peer is a create whose target is X, under the end of a chain of 30:
/// the children now hang at depth 31 and 32 and 33.
#[test]
fn a_late_created_parent_deepens_children_the_check_believes_untouched() {
    const PEER_X: u64 = 4_242;
    let base0 = legacy_chain(7, 30).export_snapshot().unwrap();
    let (update1, x) = unborn_update(&base0, PEER_X, 99, |author, x| {
        let tree = author.get_tree("tree");
        tree.enable_fractional_index(0);
        let a0 = tree.roots().into_iter().find(|root| *root != x).unwrap();
        tree.mov(x, TreeParentId::Node(a0)).unwrap();
        let c0 = tree.create(TreeParentId::Node(x)).unwrap();
        set_node(author, c0, "c0", "navigator_node");
        let c1 = tree.create(TreeParentId::Node(c0)).unwrap();
        set_node(author, c1, "c1", "navigator_node");
        let c2 = tree.create(TreeParentId::Node(c1)).unwrap();
        set_node(author, c2, "c2", "navigator_node");
    });
    // The building update is accepted today (the check does not decide it either).
    judge(&base0, &update1).assert_agrees("building update");
    assert!(full_decision(&base0, &update1).is_ok());
    let base1 = accepted(&base0, &update1);
    let a29 = tree_id_of(&base1, "a29");
    let update2 = create_unborn(&base1, PEER_X, Some(a29), "x", x);
    let judged = judge(&base1, &update2);
    println!("{judged:?}");
    assert!(
        judged.full.as_ref().is_err_and(|e| e.contains("tree_depth")),
        "the full computation refuses on depth: {judged:?}"
    );
    judged.assert_agrees("late created parent");
    assert!(!judged.incremental(), "{judged:?}");
}

/// The adopted subtree was deleted in the base: re-creating its unnamed root revives every node
/// under it, which no limit counted while it was deleted.
#[test]
fn a_late_created_parent_revives_a_deleted_subtree() {
    const PEER_X: u64 = 4_244;
    let mut engine = LoroCollabEngine::new_empty(16);
    create(&mut engine, "anchor", None, NodeKind::NavigatorNode);
    let base0 = engine.export_snapshot().unwrap();
    let anchor = *engine.id_to_tree.get("anchor").unwrap();
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
    assert!(full_decision(&base0, &update1).is_ok());
    let base1 = accepted(&base0, &update1);
    let update2 = create_unborn_unnamed(&base1, PEER_X, x);
    let judged = judge(&base1, &update2);
    println!("{judged:?}");
    judged.assert_agrees("revive");
    assert!(!judged.incremental(), "{judged:?}");
}

/// A plain create of `x` at the root, without a logical id.
fn create_unborn_unnamed(base: &[u8], peer_x: u64, x: TreeID) -> Vec<u8> {
    raw_edit(base, peer_x, |raw| {
        let tree = raw.get_tree("tree");
        tree.enable_fractional_index(0);
        assert_eq!(tree.create(TreeParentId::Root).unwrap(), x);
    })
}

/// A node that the base holds through a hand-encoded move and that has no children is "created"
/// later and named: the create targets a node the tree already held.
#[test]
fn a_create_of_a_node_the_base_held_without_its_create_is_not_a_new_unit() {
    const PEER_X: u64 = 4_245;
    let base0 = legacy_chain(17, 3).export_snapshot().unwrap();
    let a0 = tree_id_of(&base0, "a0");
    let (update1, x) = unborn_update(&base0, PEER_X, 97, |author, x| {
        let tree = author.get_tree("tree");
        tree.enable_fractional_index(0);
        tree.mov(x, TreeParentId::Node(a0)).unwrap();
    });
    let base1 = accepted(&base0, &update1);
    let update2 = create_unborn(&base1, PEER_X, Some(a0), "x", x);
    let judged = judge(&base1, &update2);
    println!("{judged:?}");
    judged.assert_agrees("create of a held node");
    assert_eq!(judged.fallback.as_deref(), Some("ResolvedNodeId"), "{judged:?}");
}

/// Existing nodes whose move could not apply while their new parent was missing, and that the
/// creation of that parent (with an older stamp) makes apply. The nodes are not named by the
/// update, and move.
#[test]
fn a_create_that_makes_an_earlier_move_apply_is_not_decided_incrementally() {
    const PEER_X: u64 = 4_246;
    let mut engine = LoroCollabEngine::new_empty(18);
    create(&mut engine, "a", None, NodeKind::NavigatorNode);
    create(&mut engine, "b", None, NodeKind::NavigatorNode);
    let base0 = engine.export_snapshot().unwrap();
    let a = *engine.id_to_tree.get("a").unwrap();
    // Peer 96 moves `a` under X, which the base does not hold: the move cannot apply.
    let (update1, x) = unborn_update(&base0, PEER_X, 96, |author, x| {
        let tree = author.get_tree("tree");
        tree.enable_fractional_index(0);
        tree.mov(a, TreeParentId::Node(x)).unwrap();
    });
    let base1 = accepted(&base0, &update1);
    let parent_before = LoroCollabEngine::load(&base1).unwrap().tree.parent(a);
    let update2 = create_unborn(&base1, PEER_X, None, "x", x);
    let judged = judge(&base1, &update2);
    let after = accepted(&base1, &update2);
    let parent_after = LoroCollabEngine::load(&after).unwrap().tree.parent(a);
    println!("parent of a: {parent_before:?} -> {parent_after:?}; {judged:?}");
    judged.assert_agrees("adopting create");
    if parent_before != parent_after {
        assert!(!judged.incremental(), "an existing node moved: {judged:?}");
    }
}

/// Many hidden chains linked by one update: the window must not take it, and the oracle must not
/// be handed a wrong report. (The amplification of the review: with the report accepted, the host
/// recomputed a document that no limit bounded.)
#[test]
fn hidden_chains_linked_by_one_update_are_not_decided_incrementally() {
    let (base, update) = hidden_chains(12, 31);
    let judged = judge(&base, &update);
    println!("{judged:?}");
    judged.assert_agrees("hidden chains");
    assert!(!judged.incremental(), "{judged:?}");
}

/// `chains` hidden chains of `len` named nodes, each hanging from an unnamed node `X_i` that exists
/// in the base only through a move. One update names every `X_i` and hangs `X_(i+1)` under the
/// bottom of chain `i`.
fn hidden_chains(chains: usize, len: usize) -> (Vec<u8>, Vec<u8>) {
    const PX: u64 = 4_243;
    const OPS_PER_NODE: usize = 3;
    let mut engine = LoroCollabEngine::new_empty(9);
    create(&mut engine, "anchor", None, NodeKind::NavigatorNode);
    let mut base = engine.export_snapshot().unwrap();
    let author = LoroDoc::new();
    author.import(&base).unwrap();
    author.set_peer_id(PX).unwrap();
    let tree = author.get_tree("tree");
    tree.enable_fractional_index(0);
    let mut xs = Vec::new();
    for i in 0..chains {
        let x = tree.create(TreeParentId::Root).unwrap();
        assert_eq!(x, TreeID::new(PX, i32::try_from(i * OPS_PER_NODE).unwrap()));
        for k in 1..OPS_PER_NODE {
            author
                .get_map("scratch")
                .insert("k", i64::try_from(k).unwrap())
                .unwrap();
        }
        xs.push(x);
    }
    author.commit();
    let px_ops = author.export(ExportMode::updates(&LoroDoc::new().oplog_vv())).unwrap();
    let anchor = *engine.id_to_tree.get("anchor").unwrap();
    let mut bottoms = Vec::new();
    let mut peer = 1_000u64;
    for batch in xs.chunks(8) {
        let deps = {
            let d = LoroDoc::new();
            d.import(&base).unwrap();
            frontier_strings(&d)
        };
        let author = LoroDoc::new();
        author.import(&base).unwrap();
        author.import(&px_ops).unwrap();
        author.set_peer_id(peer).unwrap();
        let tree = author.get_tree("tree");
        tree.enable_fractional_index(0);
        peer += 1;
        let from = author.oplog_vv();
        for x in batch {
            tree.mov(*x, TreeParentId::Node(anchor)).unwrap();
            let mut parent = *x;
            for j in 0..len {
                let node = tree.create(TreeParentId::Node(parent)).unwrap();
                let kind = if j % 2 == 0 { "navigator_node" } else { "block" };
                set_node(
                    &author,
                    node,
                    &format!("h{}-{j}", node.counter + i32::try_from(peer).unwrap() * 100_000),
                    kind,
                );
                parent = node;
            }
            bottoms.push(parent);
        }
        author.commit();
        let update = rewrite_deps(&base, &author, &from, &deps);
        assert!(update.len() <= 65_536, "building update {} bytes", update.len());
        base = accepted(&base, &update).as_ref().clone();
    }
    let author2 = LoroDoc::new();
    author2.import(&base).unwrap();
    author2.set_peer_id(PX).unwrap();
    let before = author2.oplog_vv();
    let tree2 = author2.get_tree("tree");
    tree2.enable_fractional_index(0);
    for (i, x) in xs.iter().enumerate() {
        let parent = if i == 0 {
            TreeParentId::Root
        } else {
            TreeParentId::Node(bottoms[i - 1])
        };
        let created = tree2.create(parent).unwrap();
        assert_eq!(created, *x);
        set_node(&author2, created, &format!("x{i}"), "navigator_node");
    }
    author2.commit();
    let update = author2.export(ExportMode::updates(&before)).unwrap();
    (base, update)
}

// ---------------------------------------------------------------------------------------------
// F2: a rewritten logical id of an existing node
// ---------------------------------------------------------------------------------------------

fn rename_update(base: &[u8], node: TreeID, value: Option<&str>) -> Vec<u8> {
    raw_edit(base, 555, |raw| {
        let meta = raw.get_tree("tree").get_meta(node).unwrap();
        match value {
            Some(value) => meta.insert("logical_id", value).unwrap(),
            None => meta.delete("logical_id").unwrap(),
        }
    })
}

/// The id caches never re-read a node they hold, so the worker keeps the old id of a renamed node
/// while a fresh load reads the new one. The decision must be the full computation's, and no
/// report may disagree with the host.
#[test]
fn a_rewritten_logical_id_of_an_existing_node_is_decided_by_the_full_computation() {
    let mut engine = LoroCollabEngine::new_empty(8);
    create(&mut engine, "n", None, NodeKind::NavigatorNode);
    create(&mut engine, "m", None, NodeKind::NavigatorNode);
    let base = engine.export_snapshot().unwrap();
    let n = *engine.id_to_tree.get("n").unwrap();
    for (label, value) in [
        ("renamed to the id of another live node", Some("m")),
        ("renamed to a fresh id", Some("fresh")),
        ("id deleted", None),
    ] {
        let judged = judge(&base, &rename_update(&base, n, value));
        println!("{label}: {judged:?}");
        judged.assert_agrees(label);
        assert_eq!(judged.fallback.as_deref(), Some("IdRewritten"), "{label}: {judged:?}");
    }
}

// ---------------------------------------------------------------------------------------------
// F3: block maps no reader supports grow a page with no bound
// ---------------------------------------------------------------------------------------------

fn flat_page(blocks: usize, chars_each: usize) -> Vec<u8> {
    let mut engine = LoroCollabEngine::new_empty(11);
    let list: Vec<EditorInitialBlock> = (0..blocks)
        .map(|index| EditorInitialBlock {
            id: format!("b{index}"),
            kind: "paragraph".into(),
            text: "x".repeat(chars_each),
            indent: 0,
            level: None,
        })
        .collect();
    initialize_editor_doc(&engine.doc, &list).unwrap();
    engine.doc.commit();
    engine.invalidate_editor_model_floor();
    engine.export_snapshot().unwrap()
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

fn list_len(base: &[u8]) -> usize {
    let raw = LoroDoc::new();
    raw.import(base).unwrap();
    children(&raw).len()
}

/// The items of a page (`crate::work`) and the budget of its live totals, read from a fresh load.
fn items_and_budget(base: &[u8]) -> (u64, u64) {
    let engine = LoroCollabEngine::load(base).unwrap();
    let items = engine.projection_items(u64::MAX).unwrap().unwrap();
    let semantic = engine.semantic_snapshot().unwrap();
    let live = || semantic.nodes.values().filter(|node| !node.deleted);
    let count = |kind: NodeKind| live().filter(|node| node.kind == kind).count() as u64;
    let chars: usize = live().map(|node| node.text.chars().count()).sum();
    (
        items,
        crate::work::item_budget(count(NodeKind::Block), count(NodeKind::NavigatorNode), chars as u64),
    )
}

/// Block maps no reader supports count toward no limit, so they cannot be allowed to grow a page
/// through the incremental window: every incremental accept leaves the page within the work budget
/// (`crate::work`), and once the update could step over it the full computation decides, which is
/// what stops such a page on `next`.
#[test]
fn unsupported_block_maps_cannot_grow_a_page_through_the_incremental_window() {
    let mut base = flat_page(10, 10);
    let mut peer = 20_000;
    let mut incremental_accepts = 0;
    let mut first_fallback = None;
    for step in 0..8 {
        peer += 1;
        let update = unsupported_maps(&base, peer, 1_000);
        assert!(update.len() <= 65_536, "update {} bytes", update.len());
        let judged = judge(&base, &update);
        judged.assert_agrees("growing the root list");
        base = accepted(&base, &update).as_ref().clone();
        let (items, budget) = items_and_budget(&base);
        if judged.incremental() {
            incremental_accepts += 1;
            assert!(
                items <= budget,
                "the incremental window accepted a page of {items} items over its budget {budget}"
            );
        } else if first_fallback.is_none() {
            first_fallback = Some((step, judged.fallback.clone(), list_len(&base), items, budget));
        }
    }
    println!("unsupported maps: {incremental_accepts} incremental accepts, first fallback {first_fallback:?}");
    assert!(
        incremental_accepts >= 3,
        "the early writes are incremental: {incremental_accepts}"
    );
    let (_, reason, _, items, budget) = first_fallback.expect("the incremental path stops");
    assert_eq!(reason.as_deref(), Some("WorkBudget"));
    assert!(items > budget - 5_000, "it stops near the budget: {items} of {budget}");
    assert!(
        BaseFacts::compute(&LoroCollabEngine::load(&base).unwrap()).is_none(),
        "the grown page has no facts: every window is the full computation"
    );
    // A keystroke on the grown page is the full computation's too.
    let key = raw_edit(&base, 99, |raw| {
        let Some(loro::ValueOrContainer::Container(loro::Container::Map(map))) = children(raw).get(0) else {
            panic!("no first block")
        };
        let Some(loro::ValueOrContainer::Container(loro::Container::List(inline))) = map.get("children") else {
            panic!("no inline list")
        };
        let Some(loro::ValueOrContainer::Container(loro::Container::Text(text))) = inline.get(0) else {
            panic!("no inline text")
        };
        text.insert(0, "k").unwrap();
    });
    let judged = judge(&base, &key);
    judged.assert_agrees("keystroke on the grown page");
    assert!(!judged.incremental(), "{judged:?}");
}

/// An update that steps over the budget is not decided from facts even though the base is below it.
#[test]
fn an_update_that_steps_over_the_work_budget_takes_the_full_computation() {
    let mut base = flat_page(10, 10);
    let mut peer = 20_100;
    // Grow in steps the incremental window still decides, until the base is within one step of the
    // budget.
    loop {
        let (items, budget) = items_and_budget(&base);
        if budget.saturating_sub(items) < 3_000 {
            break;
        }
        peer += 1;
        let update = unsupported_maps(&base, peer, 200);
        base = accepted(&base, &update).as_ref().clone();
    }
    let (items, budget) = items_and_budget(&base);
    assert!(items <= budget, "the base is within the budget: {items} of {budget}");
    assert!(BaseFacts::compute(&LoroCollabEngine::load(&base).unwrap()).is_some());
    let small = judge(&base, &unsupported_maps(&base, 20_900, 1));
    small.assert_agrees("one element");
    let big = judge(&base, &unsupported_maps(&base, 20_901, 1_500));
    big.assert_agrees("1,500 elements");
    assert!(!big.incremental(), "{big:?}");
    assert_eq!(big.fallback.as_deref(), Some("WorkBudget"), "{big:?}");
    let _ = small;
}

// ---------------------------------------------------------------------------------------------
// F5: a refusal by a total is produced directly
// ---------------------------------------------------------------------------------------------

/// A page just under `document_text_chars` whose every block gains two characters in one update:
/// the total is over its limit, the refusal is exact, and the window does not run the full
/// computation to find it.
#[test]
fn a_refusal_by_a_total_is_produced_without_the_full_computation() {
    let blocks = 200;
    let base = flat_page(blocks, 4_999);
    let update = raw_edit(&base, 77, |raw| {
        let list = children(raw);
        for i in 0..blocks {
            let Some(loro::ValueOrContainer::Container(loro::Container::Map(map))) = list.get(i) else {
                panic!("no block {i}")
            };
            let Some(loro::ValueOrContainer::Container(loro::Container::List(inline))) = map.get("children") else {
                panic!("no inline list")
            };
            let Some(loro::ValueOrContainer::Container(loro::Container::Text(text))) = inline.get(0) else {
                panic!("no inline text")
            };
            text.insert(0, "yy").unwrap();
        }
    });
    let judged = judge(&base, &update);
    println!("{judged:?}");
    judged.assert_agrees("document text over its limit");
    assert!(
        judged.window.as_ref().is_err_and(|e| e.contains("document_text_chars")),
        "{judged:?}"
    );
    assert_eq!(judged.fallbacks, 0, "{judged:?}");
}

// ---------------------------------------------------------------------------------------------
// F4: the host's oracle
// ---------------------------------------------------------------------------------------------

/// The oracle computes the depth of a hostile projection once and in linear time, however deep it
/// is: a chain of 30,000 nodes, which took the reviewed oracle (two passes of a quadratic walk)
/// minutes, is refused by the oracle in well under a second.
#[test]
fn the_oracle_does_not_cost_more_than_linear_work_on_a_very_deep_projection() {
    use crate::semantic::{SemanticNode, SemanticSnapshot};
    let mut projection = SemanticSnapshot::default();
    for index in 0..30_000_usize {
        projection.nodes.insert(
            NodeId::from(format!("n{index}").as_str()),
            SemanticNode {
                parent: (index > 0).then(|| NodeId::from(format!("n{}", index - 1).as_str())),
                order_key: "0".to_owned(),
                kind: NodeKind::Block,
                text: String::new(),
                properties: std::collections::BTreeMap::new(),
                deleted: false,
            },
        );
    }
    let report = crate::window_check::WindowReport {
        blocks: 30_000,
        navigators: 0,
        chars: 0,
        depth_bound: 5,
        longest_bound: 0,
    };
    let started = std::time::Instant::now();
    let verdict = compare_with_projection(&report, &projection);
    let took = started.elapsed();
    println!("oracle on a 30000-deep chain: {took:?}: {verdict:?}");
    assert_eq!(verdict.map_err(|divergence| divergence.field), Err("depth_bound"));
    assert!(took < std::time::Duration::from_secs(2), "the oracle took {took:?}");
}

/// A created node that has children the update did not create is not a new unit: the defence in
/// depth of the id-cache check, read from the document after the import.
#[test]
fn a_created_node_with_children_it_did_not_create_is_detected() {
    use std::collections::HashSet;
    let mut engine = LoroCollabEngine::new_empty(31);
    create(&mut engine, "x", None, NodeKind::NavigatorNode);
    create(&mut engine, "c", Some("x"), NodeKind::NavigatorNode);
    create(&mut engine, "d", Some("c"), NodeKind::NavigatorNode);
    let x = *engine.id_to_tree.get("x").unwrap();
    let c = *engine.id_to_tree.get("c").unwrap();
    let d = *engine.id_to_tree.get("d").unwrap();
    assert!(
        !engine.created_nodes_adopt_nothing(&HashSet::from([x])),
        "c hangs under x"
    );
    assert!(
        !engine.created_nodes_adopt_nothing(&HashSet::from([x, d])),
        "c is not created"
    );
    assert!(
        engine.created_nodes_adopt_nothing(&HashSet::from([x, c, d])),
        "every child is created"
    );
    assert!(
        engine.created_nodes_adopt_nothing(&HashSet::from([d])),
        "d has no children"
    );
    assert!(engine.created_nodes_adopt_nothing(&HashSet::new()));
}

// ---------------------------------------------------------------------------------------------
// F5: what "incremental, then fallback" costs against "full only"
// ---------------------------------------------------------------------------------------------

/// Measurement, not a pass/fail test (timings are local and noisy): the window against the full
/// computation alone, on a 5,000-block page, for a refusal by a total (produced directly) and for
/// a refusal by a bound (the check stops at the first unit over it and falls back).
/// Run: `cargo test --release -p collab-core --lib fallback_cost -- --ignored --nocapture`.
#[test]
#[ignore = "timing measurement"]
fn fallback_cost_against_the_full_computation_alone() {
    let blocks = 5_000;
    let touch_all = |base: &[u8], first: &str, rest: &str| {
        raw_edit(base, 77, |raw| {
            let list = children(raw);
            for i in 0..blocks {
                let Some(loro::ValueOrContainer::Container(loro::Container::Map(map))) = list.get(i) else {
                    panic!("no block {i}")
                };
                let Some(loro::ValueOrContainer::Container(loro::Container::List(inline))) = map.get("children") else {
                    panic!("no inline list")
                };
                let Some(loro::ValueOrContainer::Container(loro::Container::Text(text))) = inline.get(0) else {
                    panic!("no inline text")
                };
                text.insert(0, if i == 0 { first } else { rest }).unwrap();
            }
        })
    };
    let median = |mut samples: Vec<std::time::Duration>| {
        samples.sort();
        samples[samples.len() / 2]
    };
    let measure = |label: &str, base: &[u8], update: &[u8]| {
        let (mut full, mut window) = (Vec::new(), Vec::new());
        let mut path = String::new();
        for _ in 0..7 {
            let mut engine = LoroCollabEngine::load(base).unwrap();
            engine.prewarm_for_writes();
            let started = std::time::Instant::now();
            let _ = engine
                .import_client_update(update)
                .and_then(|_| engine.semantic_snapshot())
                .map(|semantic| check_snapshot(&semantic, &DocumentLimits::default()));
            full.push(started.elapsed());
            let mut engine = LoroCollabEngine::load(base).unwrap();
            engine.prewarm_for_writes();
            let facts = BaseFacts::compute(&engine);
            walk_counter::reset();
            let started = std::time::Instant::now();
            let outcome = run_window(&mut engine, facts.as_ref(), update);
            window.push(started.elapsed());
            let counts = walk_counter::window_counts();
            path = format!(
                "{} (fallbacks {}, direct refusals {}, unit evaluations {})",
                outcome.map_or_else(|e| e.to_string(), |_| "accepted".to_owned()),
                counts.fallbacks,
                counts.direct_refusals,
                counts.unit_evaluations
            );
        }
        println!(
            "{label}: full only {:?}, window {:?} -> {path}",
            median(full),
            median(window)
        );
    };
    // A total over its limit: 5,000 x 199 characters, two more in every block.
    let base = flat_page(blocks, 199);
    measure("total over (document_text_chars)", &base, &touch_all(&base, "yy", "yy"));
    // A bound over its limit: block 0 close to the block limit, every block touched. The first
    // unit over the bound ends the check, wherever it is evaluated.
    let engine = {
        let mut engine = LoroCollabEngine::new_empty(11);
        let list: Vec<EditorInitialBlock> = (0..blocks)
            .map(|index| EditorInitialBlock {
                id: format!("b{index}"),
                kind: "paragraph".into(),
                text: "x".repeat(if index == 0 { 99_990 } else { 100 }),
                indent: 0,
                level: None,
            })
            .collect();
        initialize_editor_doc(&engine.doc, &list).unwrap();
        engine.doc.commit();
        engine.invalidate_editor_model_floor();
        engine
    };
    let base = engine.export_snapshot().unwrap();
    measure(
        "bound over (text_block_chars), every block touched",
        &base,
        &touch_all(&base, &"z".repeat(20), "y"),
    );
    // The same update without the violation: the incremental accept.
    measure("accepted, every block touched", &base, &touch_all(&base, "z", "y"));
}
