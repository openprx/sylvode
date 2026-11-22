//! The root reads of one ordinary client update must not cost more as the page grows.
//!
//! Every reader of a reserved root first asks whether the root exists (`editor_structure::
//! root_container`), several times per write, inside the isolated worker's CPU window. A probe
//! that values every root of the document (`LoroDoc::get_value`) made each of those reads cost
//! as much as the page and pushed the window past its limit. The cost is counted, not timed: the
//! bytes each probe allocates on the calling thread measure the work it did.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::print_stdout
)]

use super::floor_equivalence_tests::page;
use super::{CollabEngine, LoroCollabEngine};
use crate::editor_structure::TREE;
use crate::operation::{NodeId, NodeKind, Operation};
use crate::walk_counter;

/// The root probes of one `import_client_update` of a five-block edit and of the projection after
/// it, on a freshly loaded page of `blocks` blocks at editor floor `floor`: `(root name, bytes
/// allocated)` in order.
fn probes_of_one_update(floor: u32, blocks: usize) -> Vec<(String, usize)> {
    let mut author = page(floor, blocks, 77);
    let base = author.export_snapshot().unwrap();
    let from = author.frontier();
    // What the benchmark's last batch does: new blocks at the top, each with its text.
    for index in 0..5 {
        let id = NodeId::from(format!("new{index}"));
        author
            .apply_content_operation(&Operation::CreateNode {
                id: id.clone(),
                parent: None,
                index: 0,
                kind: NodeKind::Block,
            })
            .unwrap();
        author
            .apply_content_operation(&Operation::InsertText {
                id,
                index: 0,
                text: "typed".into(),
            })
            .unwrap();
    }
    let update = author.export_from(&from).unwrap();
    let mut worker = LoroCollabEngine::load(&base).unwrap();
    walk_counter::reset();
    worker.import_client_update(&update).unwrap();
    // The worker's window goes on to project the page.
    worker.semantic_snapshot().unwrap();
    assert_eq!(
        walk_counter::root_forks(),
        0,
        "no ordinary page has a shadow root to fork for"
    );
    walk_counter::root_probes()
}

/// Bytes a probe may allocate beyond the same probe on a 10x smaller page: the handle and the
/// path lookup, nothing that scales with the page.
const SLACK_BYTES: usize = 4 * 1024;

/// A probe of a map root costs the same on a 200-block and a 2,000-block page, apart from the
/// document's first probe (the library lists the keys of a lazily loaded store once, which no
/// root read can avoid), and the number of probes does not depend on the page.
#[test]
fn root_probes_of_an_update_do_not_grow_with_the_page() {
    for floor in [1_u32, 4, 5] {
        let small = probes_of_one_update(floor, 200);
        let large = probes_of_one_update(floor, 2_000);
        println!("floor {floor}: small {small:?}\nfloor {floor}: large {large:?}");
        assert!(
            !small.is_empty(),
            "floor {floor}: the update read no root: the counter is not wired"
        );
        assert_eq!(
            small.iter().map(|probe| &probe.0).collect::<Vec<_>>(),
            large.iter().map(|probe| &probe.0).collect::<Vec<_>>(),
            "floor {floor}: the page size changed which roots are read"
        );
        for (index, (small, large)) in small.iter().zip(&large).enumerate().skip(1) {
            if small.0 == TREE {
                // The editor tree is a tree root: the library values the whole named root to
                // decide whether it is empty. It is probed once per update, not per read.
                continue;
            }
            assert!(
                large.1 <= small.1 + SLACK_BYTES,
                "floor {floor}: probe {index} of root {} allocated {} bytes on 2,000 blocks and {} on 200",
                small.0,
                large.1,
                small.1
            );
        }
        let tree_probes = large.iter().filter(|probe| probe.0 == TREE).count();
        assert!(
            tree_probes <= 2,
            "floor {floor}: {tree_probes} probes of the editor tree in one update"
        );
    }
}
