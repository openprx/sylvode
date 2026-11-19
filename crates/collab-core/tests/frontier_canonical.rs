//! Frontier byte stability across the two ways the server rebuilds a document.
//!
//! A writer persists `head_frontier` from an engine that imported every update incrementally,
//! while history restore (`scope_history::restore_page`) rebuilds the engine from the stored
//! snapshot plus the retained tail and then sends `engine.frontier()` back as its
//! `expected_frontier`. Both describe the same causal version, so they must be byte-identical.
//! Loro serializes its version vector in hash-map iteration order, which depends on how the map
//! was built; the peer sets below were chosen because they produce different iteration orders
//! for the incremental, snapshot+tail, and full-snapshot paths.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::indexing_slicing)]

use collab_core::{CollabEngine, LoroCollabEngine};

/// `new_empty(seed)` uses peer id `seed + 1`; these seeds name the colliding peers directly.
const COLLIDING_PEER_SETS: [[u64; 3]; 4] = [
    [0xdc1b_77ae_0bf3_4dac, 0x64f0_eeb9_026e_6076, 0x7b07_ce91_e590_6136],
    [0x9ad2_e144_d6e8_f2cf, 0xd9aa_792e_1af4_70ea, 0xddaa_4e85_b0d6_e28b],
    [0x8f8e_a9d3_4942_8d8e, 0x08f4_74ff_b8e8_ab15, 0x2ead_8547_56d7_1f03],
    [0xb809_d78e_2a2f_9b10, 0x4621_7f10_4eb3_0026, 0x8947_b5fe_fda6_afa6],
];

/// One title edit per peer, each exported as an incremental delta against the prior head.
fn per_peer_updates(seeds: &[u64]) -> Vec<Vec<u8>> {
    let mut head: Option<Vec<u8>> = None;
    let mut updates = Vec::with_capacity(seeds.len());
    for (index, seed) in seeds.iter().enumerate() {
        let mut writer = LoroCollabEngine::new_empty(*seed);
        if let Some(snapshot) = &head {
            writer.import_update(snapshot).unwrap();
        }
        let before = writer.frontier();
        writer.set_title(&format!("title {index}")).unwrap();
        updates.push(writer.export_from(&before).unwrap());
        head = Some(writer.export_snapshot().unwrap());
    }
    updates
}

#[test]
fn frontier_bytes_match_between_incremental_snapshot_tail_and_full_snapshot_paths() {
    let mut mismatches = Vec::new();
    for seeds in COLLIDING_PEER_SETS {
        let updates = per_peer_updates(&seeds);

        // Writer path: the head engine imported every accepted update in order.
        let mut incremental = LoroCollabEngine::new_empty(0xdead);
        for update in &updates {
            incremental.import_update(update).unwrap();
        }

        // Restore path: stored snapshot (after the first two updates) plus the retained tail.
        let mut snapshot_source = LoroCollabEngine::new_empty(0xbeef);
        snapshot_source.import_update(&updates[0]).unwrap();
        snapshot_source.import_update(&updates[1]).unwrap();
        let mut snapshot_tail = LoroCollabEngine::load(&snapshot_source.export_snapshot().unwrap()).unwrap();
        snapshot_tail.import_update(&updates[2]).unwrap();

        // Compaction path: a full snapshot of the head.
        let full = LoroCollabEngine::load(&incremental.export_snapshot().unwrap()).unwrap();

        let reference = incremental.frontier();
        for (path, frontier) in [
            ("snapshot+tail", snapshot_tail.frontier()),
            ("full snapshot", full.frontier()),
        ] {
            if frontier != reference {
                mismatches.push(format!(
                    "seeds {seeds:x?}: incremental {:?} != {path} {:?}",
                    reference.as_bytes(),
                    frontier.as_bytes()
                ));
            }
        }
    }
    assert!(
        mismatches.is_empty(),
        "frontier bytes depend on the rebuild path:\n{}",
        mismatches.join("\n")
    );
}
