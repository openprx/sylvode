//! The reserved-root rules against the real client's fixtures: every stored snapshot is a clean
//! document, and every stored client update that applies to a stored snapshot is accepted by the
//! client write path exactly as the replay path accepts it (first-time editor initialisation,
//! structure creation, atom insertion, cut and paste).
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::print_stdout)]

use std::path::{Path, PathBuf};

use collab_core::{CollabEngine, LoroCollabEngine};

fn files(root: &Path, extension: &str, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(root) else {
        return;
    };
    for entry in entries {
        let path = entry.unwrap().path();
        if path.is_dir() {
            files(&path, extension, out);
        } else if path.extension().is_some_and(|found| found == extension) {
            out.push(path);
        }
    }
}

fn fixture_roots() -> Vec<PathBuf> {
    let manifest = Path::new(env!("CARGO_MANIFEST_DIR"));
    vec![
        manifest.join("tests/fixtures"),
        manifest.join("../../frontend/tests/fixtures"),
    ]
}

#[test]
fn every_fixture_snapshot_is_a_clean_document() {
    let mut snapshots = Vec::new();
    for root in fixture_roots() {
        files(&root, "snapshot", &mut snapshots);
    }
    let mut checked = 0;
    for path in &snapshots {
        let bytes = std::fs::read(path).unwrap();
        let Ok(engine) = LoroCollabEngine::load(&bytes) else {
            continue;
        };
        engine
            .refuse_shadow_roots()
            .unwrap_or_else(|error| panic!("{}: {error}", path.display()));
        checked += 1;
    }
    println!("clean fixture snapshots: {checked} of {}", snapshots.len());
    assert!(checked >= 20, "too few fixture snapshots were checked: {checked}");
}

#[test]
fn every_fixture_update_that_applies_is_accepted_by_the_client_path_as_by_replay() {
    let mut accepted = 0;
    let mut creates_reserved_root = 0_usize;
    for root in fixture_roots() {
        let mut directories = std::collections::BTreeSet::new();
        let mut updates = Vec::new();
        files(&root, "update", &mut updates);
        for update in &updates {
            directories.insert(update.parent().unwrap().to_path_buf());
        }
        for directory in directories {
            let mut snapshots = Vec::new();
            files(&directory, "snapshot", &mut snapshots);
            let mut updates = Vec::new();
            files(&directory, "update", &mut updates);
            for update_path in &updates {
                let update = std::fs::read(update_path).unwrap();
                for snapshot_path in &snapshots {
                    let snapshot = std::fs::read(snapshot_path).unwrap();
                    let Ok(mut replay) = LoroCollabEngine::load(&snapshot) else {
                        continue;
                    };
                    // Only the pairs where the update applies to that snapshot, completely.
                    if replay.import_update_causally_complete(&update).is_err() {
                        continue;
                    }
                    let mut client = LoroCollabEngine::load(&snapshot).unwrap();
                    client.import_client_update(&update).unwrap_or_else(|error| {
                        panic!("{} on {}: {error}", update_path.display(), snapshot_path.display())
                    });
                    assert_eq!(
                        client.export_snapshot().unwrap(),
                        replay.export_snapshot().unwrap(),
                        "{} on {}",
                        update_path.display(),
                        snapshot_path.display()
                    );
                    accepted += 1;
                    creates_reserved_root += usize::from(creates_reserved_root_by(&snapshot, &update));
                }
            }
        }
    }
    println!("fixture update pairs accepted: {accepted}; creating a root: {creates_reserved_root}");
    assert!(accepted >= 10, "too few update fixtures applied: {accepted}");
    assert!(
        creates_reserved_root >= 1,
        "no fixture update creates a reserved root with its right type"
    );
}

/// Whether applying `update` to `snapshot` adds an op on a reserved root (of the right type: this
/// is the real client's first-time initialisation).
fn creates_reserved_root_by(snapshot: &[u8], update: &[u8]) -> bool {
    let doc = loro::LoroDoc::new();
    doc.import(snapshot).unwrap();
    let before = doc.oplog_vv();
    doc.import(update).unwrap();
    doc.export_json_updates(&before, &doc.oplog_vv())
        .unwrap()
        .changes
        .iter()
        .flat_map(|change| change.ops.iter())
        .any(|op| {
            matches!(&op.container, loro::ContainerID::Root { name, container_type }
                if collab_core::reserved_roots::expected_type(name) == Some(*container_type))
        })
}
