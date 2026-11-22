//! The import operation of the isolated worker (`isolation::isolated_import_observed`) through the
//! real worker process: an uploaded snapshot and update tail are decoded, replayed, scanned and
//! checked inside the metered window, and the answer carries the validated head.
//!
//! Linux only: the worker is Linux-only (`isolated_apply_unsupported_platform` elsewhere).

#![cfg(target_os = "linux")]
#![allow(
    unsafe_code,
    clippy::expect_used,
    clippy::panic,
    clippy::print_stdout,
    clippy::indexing_slicing,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::too_many_lines
)]

use base64::Engine as _;
use collab_core::isolation::wire::ReplayDiffRequest;
use collab_core::isolation::{
    IMPORT_DECODE_CPU_MS_MAX, IsolatedApplyError, IsolatedDiffSuccess, WORKER_BINARY_PATH_ENV, WorkerObservation,
    isolated_import_observed,
};
use collab_core::{CollabEngine, CollabError, LoroCollabEngine, NodeId, NodeKind, Operation};
use loro::{LoroDoc, TreeID, TreeParentId};

fn use_the_built_worker() {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| {
        // SAFETY: runs once, before any test of this binary spawns a worker; no other thread of
        // this process reads or writes the environment concurrently with this call.
        unsafe {
            std::env::set_var(
                WORKER_BINARY_PATH_ENV,
                env!("CARGO_BIN_EXE_collab-isolated-apply-worker"),
            );
        }
    });
}

/// An uploaded document as the package carries it: a snapshot, the update tail, and the accepted
/// frontier the head must replay to.
struct Upload {
    snapshot: Vec<u8>,
    tail: Vec<Vec<u8>>,
    frontier: Vec<u8>,
}

fn import(upload: &Upload) -> (Result<IsolatedDiffSuccess, IsolatedApplyError>, WorkerObservation) {
    import_with_model(upload, Some("v5"))
}

fn import_with_model(
    upload: &Upload,
    model: Option<&str>,
) -> (Result<IsolatedDiffSuccess, IsolatedApplyError>, WorkerObservation) {
    use_the_built_worker();
    let request = ReplayDiffRequest {
        snapshot_frontier: upload.frontier.clone(),
        updates: Vec::new(),
        from_frontier: upload.frontier.clone(),
        to_frontier: upload.frontier.clone(),
        package_updates_base64: Some(
            upload
                .tail
                .iter()
                .map(|bytes| base64::engine::general_purpose::STANDARD.encode(bytes))
                .collect(),
        ),
        package_semantic_model: model.map(str::to_owned),
    };
    isolated_import_observed(&upload.snapshot, &request)
}

const BLOCKS_PER_BATCH: usize = 50;

fn block_ops(index: usize, text: &str) -> [Operation; 2] {
    let id = NodeId::from(format!("b{index:05}"));
    [
        Operation::CreateNode {
            id: id.clone(),
            parent: None,
            index: 0,
            kind: NodeKind::Block,
        },
        Operation::InsertText {
            id,
            index: 0,
            text: text.to_owned(),
        },
    ]
}

/// A page of `blocks` blocks of `chars` characters each, reached by 50-block batches the way a
/// client writes it. `model` 1 is the flat page, 4 a page with the placement tree.
fn page(blocks: usize, chars: usize, model: u32) -> LoroCollabEngine {
    let mut engine = LoroCollabEngine::new_empty(1);
    engine.initialize_page_editor().expect("editor document initialises");
    if model >= 4 {
        engine
            .initialize_page_structure_model(model)
            .expect("structure model initialises");
    }
    let text = "x".repeat(chars);
    for start in (0..blocks).step_by(BLOCKS_PER_BATCH) {
        for index in start..(start + BLOCKS_PER_BATCH).min(blocks) {
            for operation in block_ops(index, &text) {
                engine.apply_content_operation(&operation).expect("operation applies");
            }
        }
    }
    engine
}

/// A page of `blocks` blocks whose history is long: every block is typed one character at a time at
/// the front, a third of the characters are deleted again, so the document holds about a million
/// operations for the same state as a short history.
fn history_heavy_page(blocks: usize, edits: usize) -> LoroCollabEngine {
    let mut engine = LoroCollabEngine::new_empty(1);
    engine.initialize_page_editor().expect("editor document initialises");
    engine
        .initialize_page_structure_model(4)
        .expect("structure model initialises");
    for index in 0..blocks {
        let id = NodeId::from(format!("b{index:05}"));
        engine
            .apply_content_operation(&Operation::CreateNode {
                id: id.clone(),
                parent: None,
                index: 0,
                kind: NodeKind::Block,
            })
            .expect("create applies");
        for edit in 0..edits {
            engine
                .apply_content_operation(&Operation::InsertText {
                    id: id.clone(),
                    index: 0,
                    text: "x".to_owned(),
                })
                .expect("insert applies");
            if edit % 3 == 2 {
                engine
                    .apply_content_operation(&Operation::DeleteText {
                        id: id.clone(),
                        index: 0,
                        len: 1,
                    })
                    .expect("delete applies");
            }
        }
    }
    engine
}

/// A navigator document of `nodes` navigator nodes whose first `depth` nodes form a chain.
fn navigator(nodes: usize, depth: usize) -> LoroCollabEngine {
    let mut engine = LoroCollabEngine::new_empty(1);
    for index in 0..nodes {
        let parent = match index {
            0 => None,
            _ if index < depth => Some(NodeId::from(format!("n{:05}", index - 1))),
            _ => Some(NodeId::from(format!("n{:05}", index % depth))),
        };
        engine
            .apply_content_operation(&Operation::CreateNode {
                id: NodeId::from(format!("n{index:05}")),
                parent,
                index: 0,
                kind: NodeKind::NavigatorNode,
            })
            .expect("navigator node creates");
    }
    engine
}

fn upload_of(engine: &LoroCollabEngine) -> Upload {
    Upload {
        snapshot: engine.export_snapshot().expect("snapshot exports"),
        tail: Vec::new(),
        frontier: engine.frontier().as_bytes().to_vec(),
    }
}

fn accepted(upload: &Upload) -> (IsolatedDiffSuccess, WorkerObservation) {
    accepted_with_model(upload, "v5")
}

fn accepted_with_model(upload: &Upload, model: &str) -> (IsolatedDiffSuccess, WorkerObservation) {
    let (result, observation) = import_with_model(upload, Some(model));
    match result {
        Ok(success) => (success, observation),
        Err(error) => panic!("the import was refused: {error:?} ({observation:?})"),
    }
}

fn refused_collab(upload: &Upload) -> CollabError {
    match import(upload).0 {
        Err(IsolatedApplyError::Collab(error)) => error,
        other => panic!("expected a refusal of the content, got {other:?}"),
    }
}

fn validated_head(success: &IsolatedDiffSuccess) -> LoroCollabEngine {
    let head = base64::engine::general_purpose::STANDARD
        .decode(
            success
                .validated_head_snapshot_base64
                .as_deref()
                .expect("a validated head"),
        )
        .expect("the head is base64");
    LoroCollabEngine::load(&head).expect("the validated head loads")
}

#[test]
fn a_snapshot_and_its_update_tail_import_to_the_validated_head() {
    let mut engine = page(40, 20, 1);
    let snapshot = engine.export_snapshot().expect("snapshot exports");
    let before = engine.frontier();
    for operation in block_ops(40, "tail block") {
        engine.apply_content_operation(&operation).expect("operation applies");
    }
    let tail = engine.export_from(&before).expect("update exports");
    let upload = Upload {
        snapshot,
        tail: vec![tail],
        frontier: engine.frontier().as_bytes().to_vec(),
    };
    let (success, observation) = accepted(&upload);
    let head = validated_head(&success);
    assert!(head.matches_frontier(&upload.frontier));
    assert_eq!(head.semantic_snapshot().expect("projects"), success.to_snapshot);
    assert_eq!(success.package_reference_remap_required, Some(false));
    assert!(observation.metered_cpu_us.is_some());
}

/// Every versioned package reader has its fingerprint read in the window, and only the one the
/// request names; a request without a known reader is refused.
#[test]
fn the_window_reads_the_fingerprint_of_the_named_package_reader() {
    let engine = page(20, 10, 1);
    let upload = upload_of(&engine);
    let expected = [
        ("v1", engine.legacy_package_semantic_snapshot_v1()),
        ("v2", engine.legacy_package_semantic_snapshot_v2()),
        ("v3", engine.legacy_package_semantic_snapshot_v3()),
        ("v4", engine.legacy_package_semantic_snapshot_v4()),
    ];
    for (model, snapshot) in expected {
        let hash = snapshot.expect("the reader projects").semantic_hash();
        let (result, _) = import_with_model(&upload, Some(model));
        let success = result.expect("the import is accepted");
        let reported = [
            success.legacy_package_semantic_hash_v1.as_ref(),
            success.legacy_package_semantic_hash_v2.as_ref(),
            success.legacy_package_semantic_hash_v3.as_ref(),
            success.legacy_package_semantic_hash_v4.as_ref(),
        ];
        let index = usize::from(model.as_bytes()[1] - b'1');
        for (position, value) in reported.into_iter().enumerate() {
            if position == index {
                assert_eq!(value, Some(&hash), "{model}");
            } else {
                assert_eq!(value, None, "{model} reports no other reader");
            }
        }
    }
    let (v5, _) = import_with_model(&upload, Some("v5"));
    assert!(
        v5.expect("the import is accepted")
            .legacy_package_semantic_hash_v1
            .is_none()
    );
    for model in [None, Some("v6"), Some("")] {
        let CollabError::OperationFailed { reason } = (match import_with_model(&upload, model).0 {
            Err(IsolatedApplyError::Collab(error)) => error,
            other => panic!("expected a refusal, got {other:?}"),
        }) else {
            panic!("expected an operation failure");
        };
        assert!(reason.contains("package semantic model"), "{reason}");
    }
}

#[test]
fn a_replayed_head_that_is_not_the_accepted_frontier_is_refused() {
    let engine = page(10, 5, 1);
    let mut upload = upload_of(&engine);
    upload.frontier = LoroCollabEngine::new_empty(1).frontier().as_bytes().to_vec();
    let error = refused_collab(&upload);
    assert!(
        format!("{error}").contains("frontier does not match"),
        "unexpected refusal: {error}"
    );
}

#[test]
fn an_undecodable_snapshot_is_refused_without_a_crash() {
    let upload = Upload {
        snapshot: b"not a loro snapshot at all".to_vec(),
        tail: Vec::new(),
        frontier: Vec::new(),
    };
    assert!(matches!(
        refused_collab(&upload),
        CollabError::DecodeFailed { .. } | CollabError::OperationFailed { .. }
    ));
}

/// A-2(a): the snapshot is not a 64 KiB update, so nothing but the import window bounds its content
/// to the documented limits. Each content limit refuses it, naming its kind.
#[test]
fn an_uploaded_snapshot_over_a_content_limit_is_refused_naming_the_limit() {
    let limits = collab_core::limits::DocumentLimits::default();
    // One block over `text_block_chars_max`.
    let long = page(1, limits.text_block_chars_max + 1, 1);
    let CollabError::LimitExceeded { limit_kind, .. } = refused_collab(&upload_of(&long)) else {
        panic!("expected a limit refusal");
    };
    assert_eq!(limit_kind, "text_block_chars");
    // One navigator node over `container_count_max`.
    let wide = navigator(limits.container_count_max + 1, 8);
    let CollabError::LimitExceeded { limit_kind, .. } = refused_collab(&upload_of(&wide)) else {
        panic!("expected a limit refusal");
    };
    assert_eq!(limit_kind, "container_count");
    // A chain one level deeper than `tree_depth_max`.
    let deep = navigator(limits.tree_depth_max + 2, limits.tree_depth_max + 2);
    let CollabError::LimitExceeded { limit_kind, .. } = refused_collab(&upload_of(&deep)) else {
        panic!("expected a limit refusal");
    };
    assert_eq!(limit_kind, "tree_depth");
}

/// A-2(c): a page whose later writes could never succeed is never stored. Eleven blocks of
/// `text_block_chars_max` hold one more character than `document_text_chars_max`.
#[test]
fn an_uploaded_snapshot_over_the_document_text_limit_is_refused() {
    let limits = collab_core::limits::DocumentLimits::default();
    let blocks = limits.document_text_chars_max / limits.text_block_chars_max;
    let mut engine = page(blocks, limits.text_block_chars_max, 1);
    for operation in block_ops(blocks, "y") {
        engine.apply_content_operation(&operation).expect("operation applies");
    }
    let CollabError::LimitExceeded { limit_kind, .. } = refused_collab(&upload_of(&engine)) else {
        panic!("expected a limit refusal");
    };
    assert_eq!(limit_kind, "document_text_chars");
}

#[test]
fn an_uploaded_history_with_a_shadow_root_is_refused() {
    let engine = page(3, 5, 1);
    let snapshot = engine.export_snapshot().expect("snapshot exports");
    let doc = LoroDoc::new();
    doc.import(&snapshot).expect("snapshot imports");
    doc.set_peer_id(9_200_001).expect("peer id");
    doc.get_text("flow_editor_structure_meta")
        .insert(0, "x")
        .expect("text insert");
    doc.commit();
    let upload = Upload {
        snapshot: doc.export(loro::ExportMode::Snapshot).expect("exports"),
        tail: Vec::new(),
        frontier: LoroCollabEngine::load(&doc.export(loro::ExportMode::Snapshot).expect("exports"))
            .expect("loads")
            .frontier()
            .as_bytes()
            .to_vec(),
    };
    let CollabError::OperationFailed { reason } = refused_collab(&upload) else {
        panic!("expected a refusal of the shadow root");
    };
    assert_eq!(reason, "reserved_root_container_type");
}

/// The tail of an upload names a tree parent that does not exist when the op runs: a library
/// client would panic on it, so the replay refuses it as `tree_parent_missing`.
#[test]
fn an_uploaded_tail_with_a_tree_op_under_a_missing_parent_is_refused() {
    let engine = navigator(1, 1);
    let snapshot = engine.export_snapshot().expect("snapshot exports");
    let author = LoroDoc::new();
    author.import(&snapshot).expect("snapshot imports");
    author.set_peer_id(91).expect("peer id");
    let from = author.oplog_vv();
    let tree = author.get_tree("tree");
    tree.enable_fractional_index(0);
    let anchor: TreeID = tree.roots().first().copied().expect("the anchor node");
    tree.create(TreeParentId::Node(anchor)).expect("child creates");
    author.commit();
    let mut json = serde_json::to_value(
        author
            .export_json_updates_without_peer_compression(&from, &author.oplog_vv())
            .expect("json updates export"),
    )
    .expect("json updates serialise");
    let mut rewritten = 0;
    for change in json["changes"].as_array_mut().expect("changes") {
        for op in change["ops"].as_array_mut().expect("ops") {
            if op["content"]["type"] == "create" {
                op["content"]["parent"] = serde_json::Value::String("5@9999".to_owned());
                rewritten += 1;
            }
        }
    }
    assert!(rewritten > 0, "no create op to rewrite");
    let hostile = LoroDoc::new();
    hostile.import(&snapshot).expect("snapshot imports");
    let before = hostile.oplog_vv();
    hostile
        .import_json_updates(serde_json::to_string(&json).expect("json"))
        .expect("the hand-encoded update imports");
    let tail = hostile
        .export(loro::ExportMode::updates(&before))
        .expect("update exports");
    let frontier = LoroCollabEngine::load(&hostile.export(loro::ExportMode::Snapshot).expect("exports"))
        .expect("loads")
        .frontier()
        .as_bytes()
        .to_vec();
    let upload = Upload {
        snapshot,
        tail: vec![tail],
        frontier,
    };
    let CollabError::OperationFailed { reason } = refused_collab(&upload) else {
        panic!("expected a refusal of the tree reference");
    };
    assert_eq!(reason, "tree_parent_missing");
}

/// The round trip at documented sizes in a build that a debug worker can carry: a page the product
/// writes and exports is imported again under the import budget.
#[test]
fn a_page_of_a_thousand_blocks_imports_again() {
    let engine = page(1_000, 100, 4);
    let (success, _) = accepted(&upload_of(&engine));
    assert_eq!(
        validated_head(&success).semantic_snapshot().expect("projects"),
        engine.semantic_snapshot().expect("projects")
    );
}

/// The round trip at the documented content limits: 9,990 blocks of 100 characters (one million
/// characters, the alias limit being 10,000 with the first block of an empty page), flat and with
/// the placement tree, a navigator at `container_count_max` with the deepest allowed chain, and a
/// page of the full block count whose history is a million operations long.
/// Every one of them is accepted under the import budget and the head the worker returns is the
/// page that was exported.
///
/// The budget is sized for the release worker the product ships. A debug worker is an order of
/// magnitude slower and cannot carry these pages, so this test runs in release builds only:
/// `cargo test -p collab-core --release --test import_worker`.
#[test]
#[cfg_attr(
    debug_assertions,
    ignore = "needs the release worker: the import budget is sized for it"
)]
fn pages_at_the_documented_limits_import_again_under_the_import_budget() {
    let limits = collab_core::limits::DocumentLimits::default();
    let blocks = limits.document_block_count_max - 10;
    let chars = limits.document_text_chars_max / blocks;
    for (label, engine) in [
        ("flat page", page(blocks, chars, 1)),
        ("page with placement tree", page(blocks, chars, 4)),
        (
            "navigator",
            navigator(limits.container_count_max - 1, limits.tree_depth_max - 1),
        ),
        (
            "page with a million operations of history",
            history_heavy_page(blocks, 100),
        ),
    ] {
        let upload = upload_of(&engine);
        // The reader the current raw exporter declares: the costliest fingerprint the window reads.
        let (success, observation) = accepted_with_model(&upload, "v4");
        println!(
            "IMPORT_AT_LIMITS {label}: snapshot {} bytes, cpu {:?} us, wall {:?} us (budget {} ms cpu)",
            upload.snapshot.len(),
            observation.metered_cpu_us,
            observation.metered_wall_us,
            IMPORT_DECODE_CPU_MS_MAX,
        );
        assert_eq!(
            validated_head(&success).semantic_snapshot().expect("projects"),
            engine.semantic_snapshot().expect("projects"),
            "{label}"
        );
    }
}
