//! Fixtures for the write-window differential harness (`window_differential_tests`).
//!
//! Everything here is deterministic: a base document or an update is a pure function of its seed.
//! Three kinds of case are produced, all as `(trusted base snapshot bytes, update bytes)`:
//!
//! * generated cases: seeded base documents (legacy tree, editor models 1, 4 and 5, mixed
//!   legacy-plus-editor pages, with deletes, moves, concurrent peers, and snapshot-plus-tail
//!   histories) and client updates against them (inserts, text edits, moves, deletes, marks,
//!   atoms, edits of every root, no-ops, duplicates, causally incomplete updates, concurrent
//!   updates, reverts);
//! * boundary cases: updates that put a structural limit exactly at, and one past, its ceiling;
//! * the hostile corpus: forged documents and updates, each with the decision today's code makes
//!   recorded as the expected value.
//!
//! The expected decisions of the hostile corpus were recorded from the full computation
//! (`import_client_update`, `semantic_snapshot`, `check_snapshot`) on this branch. A change in
//! them is a change of behaviour and must be a deliberate one.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::print_stdout
)]

use std::sync::Arc;

use loro::{
    Container, ContainerTrait, ContainerType, ExportMode, LoroDoc, LoroList, LoroMap, LoroText, LoroTree, LoroValue,
    TreeID, TreeParentId, ValueOrContainer,
};
use serde_json::Value;

use super::floor_equivalence_tests::{children, image_attrs, push_raw_atom, raw_update, set_model};
use super::hand_encoded::raw_edit;
use super::{CollabEngine, LoroCollabEngine};
use crate::editor_doc_bridge::{EditorInitialBlock, initialize_editor_doc};
use crate::frontier::Frontier;
use crate::operation::{NodeId, NodeKind, Operation};
use crate::reserved_roots::{ALIASES, DOC, META, ORDER, RESERVED_ROOTS, TREE};

// ---------------------------------------------------------------------------------------------
// Cases and expectations
// ---------------------------------------------------------------------------------------------

/// The decision the full computation is recorded to make for a hostile or boundary case.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Expect {
    Accept,
    /// `code` is the error variant, or the reason of an `OperationFailed`; `limit_kind` is empty
    /// unless the refusal is a limit; `observed` and `limit` are 0 unless it is a limit.
    Refuse {
        code: &'static str,
        limit_kind: &'static str,
        observed: u64,
        limit: u64,
    },
}

/// Refusal with a plain reason and no numbers.
pub(super) const fn refuse(code: &'static str) -> Expect {
    Expect::Refuse {
        code,
        limit_kind: "",
        observed: 0,
        limit: 0,
    }
}

/// Refusal by a structural limit.
pub(super) const fn over(limit_kind: &'static str, observed: u64, limit: u64) -> Expect {
    Expect::Refuse {
        code: "limit_exceeded",
        limit_kind,
        observed,
        limit,
    }
}

/// Where the base facts a window starts from come from. The worker computes them from the base it
/// loaded; a case can hand the check facts that are not those.
#[derive(Clone)]
pub(super) enum FactsFrom {
    /// The facts of the base the update is imported into (what the worker does).
    Base,
    /// The facts of the base, computed under a setup guard with this limit in bytes
    /// (`crate::setup_guard`): none when the setup of the base costs more.
    Guarded(u64),
    /// The facts of another snapshot of the page: they describe a state the import does not start
    /// from.
    OtherBase(Arc<Vec<u8>>),
    /// The facts of the base after a deliberate corruption.
    Tampered(fn(&mut crate::facts::BaseFacts)),
}

/// One input of the harness.
pub(super) struct Case {
    pub label: String,
    /// `generated`, `boundary` or `hostile`.
    pub family: &'static str,
    /// The kind of update, for coverage tables.
    pub kind: &'static str,
    /// The trusted base, as the snapshot bytes the worker receives.
    pub base: Arc<Vec<u8>>,
    pub update: Vec<u8>,
    /// Recorded decision of today's code; `None` for generated cases.
    pub expect: Option<Expect>,
    /// The facts the incremental window starts from.
    pub facts: FactsFrom,
}

// ---------------------------------------------------------------------------------------------
// Deterministic randomness
// ---------------------------------------------------------------------------------------------

/// A xorshift generator; the same seed always gives the same stream.
pub(super) struct Rng(u64);

impl Rng {
    pub(super) fn new(seed: u64) -> Self {
        let mut rng = Self(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1);
        for _ in 0..4 {
            rng.next();
        }
        rng
    }

    pub(super) fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }

    pub(super) fn below(&mut self, n: u64) -> u64 {
        self.next() % n.max(1)
    }

    pub(super) fn index(&mut self, len: usize) -> usize {
        usize::try_from(self.below(len as u64)).unwrap()
    }
}

const TEXTS: [&str; 9] = [
    "",
    "a",
    "hello world",
    "emoji \u{1F600} family \u{1F468}\u{200D}\u{1F469}\u{200D}\u{1F467} end",
    "cafe\u{301} nai\u{308}ve",
    "\u{65E5}\u{672C}\u{8A9E}\u{306E}\u{30C6}\u{30AD}\u{30B9}\u{30C8}",
    "tab\tand\nnewline",
    "x",
    "a longer paragraph of ordinary prose that is a few dozen characters long",
];

fn random_text(rng: &mut Rng) -> &'static str {
    TEXTS[rng.index(TEXTS.len())]
}

// ---------------------------------------------------------------------------------------------
// Base documents
// ---------------------------------------------------------------------------------------------

/// The shape of a base document.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Model {
    /// Tree-only legacy page: navigator and block nodes in the engine tree, no editor document.
    LegacyTree,
    /// Editor model 1: flat editor blocks, no placement tree.
    Flat,
    /// Editor model 4: editor blocks with a placement tree (nesting up to depth 7).
    Structured4,
    /// Editor model 5: model 4 plus the marker and an atom block.
    Structured5,
    /// A legacy tree holding navigator nodes and embed blocks beside an editor document.
    Mixed,
}

pub(super) const MODELS: [Model; 5] = [
    Model::LegacyTree,
    Model::Flat,
    Model::Structured4,
    Model::Structured5,
    Model::Mixed,
];

impl Model {
    pub(super) const fn name(self) -> &'static str {
        match self {
            Self::LegacyTree => "legacy",
            Self::Flat => "model1",
            Self::Structured4 => "model4",
            Self::Structured5 => "model5",
            Self::Mixed => "mixed",
        }
    }

    const fn has_editor(self) -> bool {
        !matches!(self, Self::LegacyTree)
    }

    const fn structured(self) -> bool {
        matches!(self, Self::Structured4 | Self::Structured5)
    }
}

/// How the history of a base document was made before its snapshot was taken.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum History {
    /// Built in one go.
    None,
    /// `n` random sequential edits: creates, text edits, deletes, moves, marks.
    Edits(usize),
    /// A second peer edits concurrently for `n` steps and is merged.
    Concurrent(usize),
    /// A snapshot, then `n` edits replayed as tail updates on top of it.
    SnapshotTail(usize),
}

impl History {
    const fn name(self) -> &'static str {
        match self {
            Self::None => "plain",
            Self::Edits(_) => "edits",
            Self::Concurrent(_) => "concurrent",
            Self::SnapshotTail(_) => "tail",
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub(super) struct BaseSpec {
    pub model: Model,
    /// Blocks (editor models, mixed) or tree nodes (legacy). 0 is the empty document.
    pub blocks: usize,
    pub seed: u64,
    pub history: History,
}

impl BaseSpec {
    pub(super) fn label(&self) -> String {
        format!(
            "{}/{}blocks/{}/seed{}",
            self.model.name(),
            self.blocks,
            self.history.name(),
            self.seed
        )
    }
}

/// A built base document: the author's engine (to author updates against) and its snapshot.
pub(super) struct Built {
    pub spec: BaseSpec,
    pub author: LoroCollabEngine,
    /// Logical ids that were live when the document was built (some may have been deleted since).
    pub ids: Vec<String>,
    pub snapshot: Arc<Vec<u8>>,
}

const EDITOR_KINDS_FLAT: [&str; 4] = ["paragraph", "heading", "bulletItem", "codeBlock"];
const EDITOR_KINDS_RICH: [&str; 8] = [
    "paragraph",
    "heading",
    "bulletItem",
    "codeBlock",
    "todoItem",
    "orderedItem",
    "blockquote",
    "toggle",
];

fn editor_blocks(rng: &mut Rng, count: usize, model: Model) -> Vec<EditorInitialBlock> {
    let kinds: &[&str] = if matches!(model, Model::Flat | Model::Mixed) {
        &EDITOR_KINDS_FLAT
    } else {
        &EDITOR_KINDS_RICH
    };
    let mut blocks: Vec<EditorInitialBlock> = Vec::with_capacity(count);
    for index in 0..count {
        let kind = kinds[rng.index(kinds.len())];
        let indent = if model.structured() && index > 0 && rng.below(3) == 0 {
            let previous = blocks.last().map_or(0, |block| block.indent);
            u32::try_from(rng.below(u64::from(previous) + 2)).unwrap().min(7)
        } else {
            0
        };
        blocks.push(EditorInitialBlock {
            id: format!("b{index}"),
            kind: kind.to_owned(),
            text: random_text(rng).to_owned(),
            indent,
            level: (kind == "heading").then(|| u32::try_from(1 + rng.below(3)).unwrap()),
        });
    }
    blocks
}

fn create(engine: &mut LoroCollabEngine, id: &str, parent: Option<&str>, index: u32, kind: NodeKind) -> bool {
    engine
        .apply_content_operation(&Operation::CreateNode {
            id: NodeId::from(id),
            parent: parent.map(NodeId::from),
            index,
            kind,
        })
        .is_ok()
}

fn insert_text(engine: &mut LoroCollabEngine, id: &str, index: u32, text: &str) -> bool {
    engine
        .apply_content_operation(&Operation::InsertText {
            id: NodeId::from(id),
            index,
            text: text.to_owned(),
        })
        .is_ok()
}

/// Navigator and block nodes in the engine tree, nested at most eight deep.
fn build_legacy_tree(engine: &mut LoroCollabEngine, rng: &mut Rng, nodes: usize, prefix: &str) -> Vec<String> {
    let mut ids: Vec<String> = Vec::new();
    let mut depths: Vec<usize> = Vec::new();
    for index in 0..nodes {
        let id = format!("{prefix}{index}");
        let kind = if rng.below(3) == 0 {
            NodeKind::NavigatorNode
        } else {
            NodeKind::Block
        };
        let parent = if !ids.is_empty() && rng.below(2) == 0 {
            let at = rng.index(ids.len());
            (depths[at] < 8).then_some(at)
        } else {
            None
        };
        assert!(
            create(engine, &id, parent.map(|at| ids[at].as_str()), 0, kind),
            "legacy create {id}"
        );
        if kind == NodeKind::Block {
            let text = random_text(rng);
            if !text.is_empty() {
                assert!(insert_text(engine, &id, 0, text), "legacy text {id}");
            }
        }
        depths.push(parent.map_or(0, |at| depths[at] + 1));
        ids.push(id);
    }
    ids
}

/// Navigator nodes and embed blocks in the engine tree, to sit beside an editor document.
fn build_legacy_side(engine: &mut LoroCollabEngine, rng: &mut Rng, nodes: usize) -> Vec<String> {
    let mut ids = Vec::new();
    for index in 0..nodes {
        let id = format!("n{index}");
        if rng.below(2) == 0 {
            assert!(create(engine, &id, None, 0, NodeKind::NavigatorNode));
        } else {
            assert!(create(engine, &id, None, 0, NodeKind::Block));
            engine
                .apply_content_operation(&Operation::SetProperty {
                    id: NodeId::from(id.as_str()),
                    key: "embed_object_id".into(),
                    value: format!("embed-{index}"),
                })
                .unwrap();
        }
        ids.push(id);
    }
    ids
}

fn build_editor(engine: &mut LoroCollabEngine, rng: &mut Rng, blocks: usize, model: Model) -> Vec<String> {
    let list = editor_blocks(rng, blocks, model);
    initialize_editor_doc(&engine.doc, &list).unwrap();
    engine.doc.commit();
    engine.invalidate_editor_model_floor();
    match model {
        Model::Structured4 => {
            engine.initialize_page_structure_model(4).unwrap();
        }
        Model::Structured5 => {
            engine.initialize_page_structure_model(5).unwrap();
            engine
                .insert_page_atom("atom0", None, 0, "image", &image_attrs())
                .unwrap();
        }
        _ => {}
    }
    let mut ids: Vec<String> = list.iter().map(|block| block.id.clone()).collect();
    if model == Model::Structured5 {
        ids.push("atom0".into());
    }
    ids
}

/// One random edit as a client would author it, through the public engine API. Failures (an
/// illegal move, a missing block) are ignored: the edit simply did not happen.
pub(super) fn random_edit(
    engine: &mut LoroCollabEngine,
    rng: &mut Rng,
    ids: &mut Vec<String>,
    next: &mut u32,
    model: Model,
) {
    let pick = |rng: &mut Rng, ids: &[String]| (!ids.is_empty()).then(|| ids[rng.index(ids.len())].clone());
    match rng.below(10) {
        0..=2 => {
            let id = format!("h{next}");
            *next += 1;
            let parent = if rng.below(4) == 0 { pick(rng, ids) } else { None };
            let kind = if model == Model::LegacyTree && rng.below(3) == 0 {
                NodeKind::NavigatorNode
            } else {
                NodeKind::Block
            };
            let parent = if model.has_editor() && !model.structured() {
                None
            } else {
                parent
            };
            if create(
                engine,
                &id,
                parent.as_deref(),
                u32::try_from(rng.below(3)).unwrap(),
                kind,
            ) {
                if kind == NodeKind::Block {
                    insert_text(engine, &id, 0, random_text(rng));
                }
                ids.push(id);
            }
        }
        3 | 4 => {
            if let Some(id) = pick(rng, ids) {
                insert_text(engine, &id, 0, random_text(rng));
            }
        }
        5 => {
            if let Some(id) = pick(rng, ids) {
                let _ = engine.apply_content_operation(&Operation::DeleteText {
                    id: NodeId::from(id.as_str()),
                    index: 0,
                    len: 1,
                });
            }
        }
        6 | 7 => {
            if let Some(id) = pick(rng, ids)
                && engine
                    .apply_content_operation(&Operation::DeleteNode {
                        id: NodeId::from(id.as_str()),
                    })
                    .is_ok()
            {
                ids.retain(|known| *known != id);
            }
        }
        8 => {
            if let Some(id) = pick(rng, ids) {
                let parent = if model.has_editor() && !model.structured() {
                    None
                } else {
                    pick(rng, ids).filter(|parent| *parent != id)
                };
                let _ = engine.apply_content_operation(&Operation::MoveNode {
                    id: NodeId::from(id.as_str()),
                    new_parent: parent.as_deref().map(NodeId::from),
                    index: 0,
                });
            }
        }
        _ => {
            if model.has_editor()
                && let Some(id) = pick(rng, ids)
            {
                let _ = engine.apply_page_marks(
                    &id,
                    &[crate::page_inline::MarkEdit {
                        start: 0,
                        end: 1,
                        mark: crate::page_inline::InlineMark::Bold,
                        clear: rng.below(3) == 0,
                        href: None,
                    }],
                );
            }
        }
    }
}

fn run_edits(engine: &mut LoroCollabEngine, rng: &mut Rng, ids: &mut Vec<String>, model: Model, steps: usize) {
    let mut next = u32::try_from(ids.len() as u64 + 1_000 * (rng.below(9) + 1)).unwrap();
    for _ in 0..steps {
        random_edit(engine, rng, ids, &mut next, model);
    }
}

/// Builds the base document of `spec`.
pub(super) fn build_base(spec: BaseSpec) -> Built {
    let mut rng = Rng::new(spec.seed.wrapping_mul(7919).wrapping_add(spec.blocks as u64));
    let mut engine = LoroCollabEngine::new_empty(spec.seed.wrapping_add(10));
    let mut ids = match spec.model {
        Model::LegacyTree => build_legacy_tree(&mut engine, &mut rng, spec.blocks, "n"),
        Model::Mixed => {
            let mut ids = build_legacy_side(&mut engine, &mut rng, spec.blocks / 4 + 1);
            ids.extend(build_editor(&mut engine, &mut rng, spec.blocks, spec.model));
            ids
        }
        Model::Flat | Model::Structured4 | Model::Structured5 => {
            build_editor(&mut engine, &mut rng, spec.blocks, spec.model)
        }
    };
    match spec.history {
        History::None => {}
        History::Edits(steps) => run_edits(&mut engine, &mut rng, &mut ids, spec.model, steps),
        History::Concurrent(steps) => {
            let from = engine.frontier();
            let mut other = engine.fork().unwrap();
            other.doc.set_peer_id(900_000 + spec.seed).unwrap();
            let mut other_ids = ids.clone();
            run_edits(&mut other, &mut rng, &mut other_ids, spec.model, steps);
            let update = other.export_from(&from).unwrap();
            run_edits(&mut engine, &mut rng, &mut ids, spec.model, steps);
            engine.import_update(&update).unwrap();
            ids.retain(|id| !engine.has_editor_alias(id).unwrap_or(false) || other_ids.contains(id));
        }
        History::SnapshotTail(steps) => {
            run_edits(&mut engine, &mut rng, &mut ids, spec.model, steps);
            let snapshot = engine.export_snapshot().unwrap();
            let mut tail = Vec::new();
            for _ in 0..steps {
                let before = engine.frontier();
                run_edits(&mut engine, &mut rng, &mut ids, spec.model, 1);
                tail.push(engine.export_from(&before).unwrap());
            }
            let mut replay = LoroCollabEngine::load(&snapshot).unwrap();
            for update in &tail {
                replay.import_update(update).unwrap();
            }
            engine = replay;
        }
    }
    // A fixture bug must not pass as a hostile case: every generated base is a valid document.
    let projection = engine.semantic_snapshot();
    assert!(
        projection.is_ok(),
        "{}: base does not project: {projection:?}",
        spec.label()
    );
    if let Ok(semantic) = projection {
        assert!(
            crate::limits::check_snapshot(&semantic, &crate::limits::DocumentLimits::default()).is_ok(),
            "{}: base is over a limit",
            spec.label()
        );
    }
    let snapshot = Arc::new(engine.export_snapshot().unwrap());
    Built {
        spec,
        author: engine,
        ids,
        snapshot,
    }
}

// ---------------------------------------------------------------------------------------------
// Updates against a built base
// ---------------------------------------------------------------------------------------------

/// The update produced by `edit` on a fork of `author`, written by `peer`.
pub(super) fn author_update(author: &LoroCollabEngine, peer: u64, edit: impl FnOnce(&mut LoroCollabEngine)) -> Vec<u8> {
    let mut fork = author.fork().unwrap();
    fork.doc.set_peer_id(peer).unwrap();
    let before = fork.frontier();
    edit(&mut fork);
    fork.export_from(&before).unwrap()
}

/// A base snapshot that already holds `update` (the accepted history the concurrent update sits
/// beside).
fn base_with(author: &LoroCollabEngine, update: &[u8]) -> Arc<Vec<u8>> {
    let mut fork = author.fork().unwrap();
    fork.import_update(update).unwrap();
    Arc::new(fork.export_snapshot().unwrap())
}

fn generated(built: &Built, kind: &'static str, update: Vec<u8>) -> Case {
    Case {
        label: format!("{} :: {kind}", built.spec.label()),
        family: "generated",
        kind,
        base: Arc::clone(&built.snapshot),
        update,
        expect: None,
        facts: FactsFrom::Base,
    }
}

/// An edit authored on a fork of the base.
type Edit = Box<dyn Fn(&mut LoroCollabEngine)>;
/// A kind of concurrent update: the two edits of the two peers.
type ConcurrentPair = (&'static str, Edit, Edit);

const EMOJI_EDIT: &str = "\u{1F600}e\u{301}";

/// Every kind of client update against `built`, and the concurrent ones against bases that hold
/// a peer's update already.
#[allow(clippy::too_many_lines)]
pub(super) fn update_cases(built: &Built, rng: &mut Rng) -> Vec<Case> {
    let mut cases = Vec::new();
    let model = built.spec.model;
    let author = &built.author;
    let ids = &built.ids;
    let seed = built.spec.seed;
    let mut peer = 2_000_000 + seed * 1_000;
    let mut next_peer = || {
        peer += 1;
        peer
    };
    let pick = |rng: &mut Rng| (!ids.is_empty()).then(|| ids[rng.index(ids.len())].clone());
    let parent_for_new = |rng: &mut Rng| -> Option<String> {
        if model.structured() && rng.below(3) == 0 {
            pick(rng)
        } else {
            None
        }
    };

    // Inserts: one block, many blocks (up to the 50-block benchmark batch).
    let p = next_peer();
    let id = format!("u{p}");
    let parent = parent_for_new(rng);
    cases.push(generated(
        built,
        "insert_one",
        author_update(author, p, |e| {
            create(e, &id, parent.as_deref(), 0, NodeKind::Block);
            insert_text(e, &id, 0, "typed");
        }),
    ));
    for count in [5_u64, 50] {
        let p = next_peer();
        let seeds = (0..count).map(|_| rng.below(3)).collect::<Vec<_>>();
        cases.push(generated(
            built,
            if count == 5 { "insert_five" } else { "insert_fifty" },
            author_update(author, p, |e| {
                for (index, flavour) in seeds.iter().enumerate() {
                    let id = format!("m{p}_{index}");
                    let kind = if model == Model::LegacyTree && *flavour == 0 {
                        NodeKind::NavigatorNode
                    } else {
                        NodeKind::Block
                    };
                    if create(e, &id, None, 0, kind) && kind == NodeKind::Block {
                        insert_text(e, &id, 0, TEXTS[usize::try_from(*flavour).unwrap() + 2]);
                    }
                }
            }),
        ));
    }
    if model == Model::LegacyTree {
        let p = next_peer();
        cases.push(generated(
            built,
            "insert_navigator_chain",
            author_update(author, p, |e| {
                let mut parent: Option<String> = None;
                for index in 0..5 {
                    let id = format!("c{p}_{index}");
                    create(e, &id, parent.as_deref(), 0, NodeKind::NavigatorNode);
                    parent = Some(id);
                }
            }),
        ));
    }

    if let Some(id) = pick(rng) {
        // Text edits: insert (emoji, combining), delete, and edits in many blocks.
        let p = next_peer();
        cases.push(generated(
            built,
            "text_insert",
            author_update(author, p, |e| {
                insert_text(e, &id, 0, EMOJI_EDIT);
            }),
        ));
        let p = next_peer();
        cases.push(generated(
            built,
            "text_delete",
            author_update(author, p, |e| {
                let _ = e.apply_content_operation(&Operation::DeleteText {
                    id: NodeId::from(id.as_str()),
                    index: 0,
                    len: 1,
                });
            }),
        ));
        let p = next_peer();
        let many: Vec<String> = ids.iter().take(20).cloned().collect();
        cases.push(generated(
            built,
            "text_edit_many_blocks",
            author_update(author, p, |e| {
                for id in &many {
                    insert_text(e, id, 0, "x");
                }
            }),
        ));
        // Deletes: one, several.
        let p = next_peer();
        cases.push(generated(
            built,
            "delete_one",
            author_update(author, p, |e| {
                let _ = e.apply_content_operation(&Operation::DeleteNode {
                    id: NodeId::from(id.as_str()),
                });
            }),
        ));
        let p = next_peer();
        let several: Vec<String> = ids.iter().rev().take(5).cloned().collect();
        cases.push(generated(
            built,
            "delete_several",
            author_update(author, p, |e| {
                for id in &several {
                    let _ = e.apply_content_operation(&Operation::DeleteNode {
                        id: NodeId::from(id.as_str()),
                    });
                }
            }),
        ));
        // Delete then insert at the same time: the count does not move.
        let p = next_peer();
        let fresh = format!("r{p}");
        cases.push(generated(
            built,
            "delete_one_insert_one",
            author_update(author, p, |e| {
                let _ = e.apply_content_operation(&Operation::DeleteNode {
                    id: NodeId::from(id.as_str()),
                });
                create(e, &fresh, None, 0, NodeKind::Block);
            }),
        ));
        // Move: under another block on structured pages, to the front elsewhere.
        let p = next_peer();
        let other = pick(rng).filter(|other| *other != id);
        cases.push(generated(
            built,
            "move_one",
            author_update(author, p, |e| {
                let _ = e.apply_content_operation(&Operation::MoveNode {
                    id: NodeId::from(id.as_str()),
                    new_parent: if model.has_editor() && !model.structured() {
                        None
                    } else {
                        other.as_deref().map(NodeId::from)
                    },
                    index: 0,
                });
            }),
        ));
        // Set a property (legacy tree nodes and embed properties).
        let p = next_peer();
        cases.push(generated(
            built,
            "set_property",
            author_update(author, p, |e| {
                let _ = e.apply_content_operation(&Operation::SetProperty {
                    id: NodeId::from(id.as_str()),
                    key: "block_type".into(),
                    value: "heading".into(),
                });
            }),
        ));
        // Marks.
        if model.has_editor() {
            let p = next_peer();
            cases.push(generated(
                built,
                "marks",
                author_update(author, p, |e| {
                    insert_text(e, &id, 0, "marked");
                    let _ = e.apply_page_marks(
                        &id,
                        &[crate::page_inline::MarkEdit {
                            start: 0,
                            end: 3,
                            mark: crate::page_inline::InlineMark::Bold,
                            clear: false,
                            href: None,
                        }],
                    );
                }),
            ));
        }
        // Edit then revert to the state before it: a net no-op with ops.
        let p = next_peer();
        cases.push(generated(
            built,
            "edit_then_revert",
            author_update(author, p, |e| {
                let before = e.frontier();
                insert_text(e, &id, 0, "gone");
                let _ = e.revert_to_frontier(&before);
            }),
        ));
    }

    // Atoms and initialisation: raise the floor, or initialise the structure or editor document.
    let p = next_peer();
    if model.has_editor() {
        cases.push(generated(
            built,
            "insert_atom",
            author_update(author, p, |e| {
                let _ = e.insert_page_atom(&format!("atom{p}"), None, 0, "image", &image_attrs());
            }),
        ));
        let p = next_peer();
        cases.push(generated(
            built,
            "initialize_structure",
            author_update(author, p, |e| {
                let _ = e.initialize_page_structure_model(4);
            }),
        ));
    } else {
        cases.push(generated(
            built,
            "initialize_editor",
            author_update(author, p, |e| {
                let _ = e.initialize_page_editor();
            }),
        ));
    }

    // Every root in one update: title (`meta`), navigator order, alias tombstone, a new block.
    let p = next_peer();
    cases.push(generated(
        built,
        "touch_every_root",
        author_update(author, p, |e| {
            let _ = e.set_title("a title");
            let _ = e.set_navigator_order("x", "m");
            e.doc
                .get_map(ALIASES)
                .insert(&format!("tomb{p}"), "__deleted__")
                .unwrap();
            e.doc.commit();
            create(e, &format!("roots{p}"), None, 0, NodeKind::Block);
        }),
    ));
    let p = next_peer();
    cases.push(generated(
        built,
        "title_only",
        author_update(author, p, |e| {
            let _ = e.set_title("only a title");
        }),
    ));

    // No-op and duplicate updates.
    let p = next_peer();
    cases.push(generated(built, "no_op", author_update(author, p, |_| {})));
    cases.push(generated(
        built,
        "already_known",
        author.export_from(&Frontier::default()).unwrap(),
    ));

    // A causally incomplete update: the second of two sequential edits, delivered alone.
    {
        let p = next_peer();
        let mut fork = author.fork().unwrap();
        fork.doc.set_peer_id(p).unwrap();
        let before = fork.frontier();
        create(&mut fork, &format!("i{p}a"), None, 0, NodeKind::Block);
        let middle = fork.frontier();
        create(&mut fork, &format!("i{p}b"), None, 0, NodeKind::Block);
        let _ = before;
        cases.push(generated(
            built,
            "causally_incomplete",
            fork.export_from(&middle).unwrap(),
        ));
    }

    // Concurrent updates: the base already holds the other peer's update.
    if ids.len() >= 2 {
        let a = ids[rng.index(ids.len())].clone();
        let b = ids.iter().find(|other| **other != a).cloned().unwrap();
        let pairs: [ConcurrentPair; 6] = [
            (
                "concurrent_insert_insert",
                Box::new(|e| {
                    create(e, "ca", None, 0, NodeKind::Block);
                }),
                Box::new(|e| {
                    create(e, "cb", None, 0, NodeKind::Block);
                }),
            ),
            (
                "concurrent_delete_then_edit_same_block",
                Box::new({
                    let a = a.clone();
                    move |e| {
                        let _ = e.apply_content_operation(&Operation::DeleteNode {
                            id: NodeId::from(a.as_str()),
                        });
                    }
                }),
                Box::new({
                    let a = a.clone();
                    move |e| {
                        insert_text(e, &a, 0, "late edit");
                    }
                }),
            ),
            (
                "concurrent_delete_delete_same_block",
                Box::new({
                    let a = a.clone();
                    move |e| {
                        let _ = e.apply_content_operation(&Operation::DeleteNode {
                            id: NodeId::from(a.as_str()),
                        });
                    }
                }),
                Box::new({
                    let a = a.clone();
                    move |e| {
                        let _ = e.apply_content_operation(&Operation::DeleteNode {
                            id: NodeId::from(a.as_str()),
                        });
                    }
                }),
            ),
            (
                "concurrent_move_cycle",
                Box::new({
                    let (a, b) = (a.clone(), b.clone());
                    move |e| {
                        let _ = e.apply_content_operation(&Operation::MoveNode {
                            id: NodeId::from(a.as_str()),
                            new_parent: Some(NodeId::from(b.as_str())),
                            index: 0,
                        });
                    }
                }),
                Box::new({
                    let (a, b) = (a.clone(), b);
                    move |e| {
                        let _ = e.apply_content_operation(&Operation::MoveNode {
                            id: NodeId::from(b.as_str()),
                            new_parent: Some(NodeId::from(a.as_str())),
                            index: 0,
                        });
                    }
                }),
            ),
            (
                "concurrent_delete_parent_insert_child",
                Box::new({
                    let a = a.clone();
                    move |e| {
                        let _ = e.apply_content_operation(&Operation::DeleteNode {
                            id: NodeId::from(a.as_str()),
                        });
                    }
                }),
                Box::new({
                    let a = a.clone();
                    move |e| {
                        create(e, "child_of_deleted", Some(&a), 0, NodeKind::Block);
                    }
                }),
            ),
            (
                "concurrent_text_edit_text_edit",
                Box::new({
                    let a = a.clone();
                    move |e| {
                        insert_text(e, &a, 0, "left ");
                    }
                }),
                Box::new({
                    move |e| {
                        insert_text(e, &a, 0, "right ");
                    }
                }),
            ),
        ];
        for (kind, first, second) in pairs {
            let (pa, pb) = (next_peer(), next_peer());
            let ua = author_update(author, pa, |e| first(e));
            let ub = author_update(author, pb, |e| second(e));
            for (label, held, update) in [("", &ua, &ub), ("_reversed", &ub, &ua)] {
                cases.push(Case {
                    label: format!("{} :: {kind}{label}", built.spec.label()),
                    family: "generated",
                    kind,
                    base: base_with(author, held),
                    update: update.clone(),
                    expect: None,
                    facts: FactsFrom::Base,
                });
            }
        }
    }
    cases.extend(super::window_class_fixtures::class_cases(built, rng));
    cases
}

/// The base specs of the default run at `scale` (a multiplier of the seeds per cell).
pub(super) fn base_specs(scale: u64) -> Vec<BaseSpec> {
    let histories = [
        History::None,
        History::Edits(12),
        History::Concurrent(6),
        History::SnapshotTail(5),
    ];
    let mut specs = Vec::new();
    for model in MODELS {
        for blocks in [0_usize, 1, 3, 20, 100] {
            for seed in 0..4 * scale {
                specs.push(BaseSpec {
                    model,
                    blocks,
                    seed,
                    history: histories[usize::try_from(seed).unwrap() % histories.len()],
                });
            }
        }
        // Larger pages, fewer of them.
        for (blocks, history) in [(500_usize, History::Edits(10)), (1_000, History::None)] {
            specs.push(BaseSpec {
                model,
                blocks,
                seed: 0,
                history,
            });
        }
    }
    specs
}

/// Runs `job` over `items` on all cores, taking items from a shared counter so that a few slow
/// items do not leave other cores idle, and returns the results in the order of `items`.
pub(super) fn run_parallel<I: Sync, T: Send>(items: &[I], job: &(dyn Fn(usize, &I) -> T + Sync)) -> Vec<T> {
    use std::sync::atomic::{AtomicUsize, Ordering};
    let threads = std::thread::available_parallelism()
        .map_or(4, std::num::NonZero::get)
        .min(16);
    let next = AtomicUsize::new(0);
    let mut collected: Vec<(usize, T)> = std::thread::scope(|scope| {
        let handles: Vec<_> = (0..threads)
            .map(|_| {
                scope.spawn(|| {
                    let mut done = Vec::new();
                    loop {
                        let index = next.fetch_add(1, Ordering::Relaxed);
                        let Some(item) = items.get(index) else { break };
                        done.push((index, job(index, item)));
                    }
                    done
                })
            })
            .collect();
        handles.into_iter().flat_map(|handle| handle.join().unwrap()).collect()
    });
    collected.sort_by_key(|(index, _)| *index);
    collected.into_iter().map(|(_, value)| value).collect()
}

/// Every generated case at `scale`. The bases are built on all cores; the result does not depend
/// on the thread count (each base and its updates are a function of the base spec alone).
pub(super) fn generated_cases(scale: u64) -> Vec<Case> {
    let mut specs = base_specs(scale);
    // Largest first, so that the slow bases start early.
    specs.sort_by_key(|spec| std::cmp::Reverse(spec.blocks));
    let per_spec = run_parallel(&specs, &|_, spec| {
        let built = build_base(*spec);
        let mut rng = Rng::new(
            spec.seed
                .wrapping_mul(31)
                .wrapping_add(spec.blocks as u64)
                .wrapping_add(5),
        );
        update_cases(&built, &mut rng)
    });
    per_spec.into_iter().flatten().collect()
}

// ---------------------------------------------------------------------------------------------
// Raw document helpers (forged documents and updates)
// ---------------------------------------------------------------------------------------------

fn load_raw(base: &[u8], peer: u64) -> LoroDoc {
    let raw = LoroDoc::new();
    raw.import(base).unwrap();
    raw.set_peer_id(peer).unwrap();
    raw
}

fn tree_node_of(tree: &LoroTree, cid: &str) -> TreeID {
    tree.nodes()
        .into_iter()
        .find(|node| {
            matches!(
                tree.get_meta(*node).unwrap().get("block_cid"),
                Some(ValueOrContainer::Value(LoroValue::String(value))) if value.to_string() == cid
            )
        })
        .unwrap_or_else(|| panic!("no tree node for {cid}"))
}

/// Appends a paragraph block map holding `text` to `doc.children`, with an alias when `alias` is
/// given and a placement node under `parent` (a block CID) on structured pages. Returns its CID.
/// Where a raw block is placed in the placement tree.
#[derive(Clone, Copy)]
enum Place<'a> {
    /// Not at all (flat pages, or a deliberately unplaced block).
    Unplaced,
    /// At the top level.
    Root,
    /// Under the block with this CID.
    Under(&'a str),
}

fn raw_block(raw: &LoroDoc, alias: Option<&str>, text: &str, place: Place<'_>) -> String {
    let list = children(raw);
    let map = list.insert_container(list.len(), LoroMap::new()).unwrap();
    map.insert("nodeName", "paragraph").unwrap();
    let attrs = map.insert_container("attributes", LoroMap::new()).unwrap();
    attrs.insert("indent", 0).unwrap();
    let inline = map.insert_container("children", LoroList::new()).unwrap();
    if !text.is_empty() {
        inline
            .insert_container(0, LoroText::new())
            .unwrap()
            .insert(0, text)
            .unwrap();
    }
    let cid = map.id().to_string();
    if let Some(alias) = alias {
        raw.get_map(ALIASES).insert(alias, cid.as_str()).unwrap();
    }
    if !matches!(place, Place::Unplaced) {
        let tree = raw.get_tree(TREE);
        tree.enable_fractional_index(0);
        let parent = match place {
            Place::Under(parent) => Some(tree_node_of(&tree, parent)),
            _ => None,
        };
        let node = tree.create(parent).unwrap();
        tree.get_meta(node).unwrap().insert("block_cid", cid.as_str()).unwrap();
    }
    cid
}

/// CIDs of the block maps in `doc.children`, in list order.
fn block_cids(raw: &LoroDoc) -> Vec<String> {
    let list = children(raw);
    (0..list.len())
        .filter_map(|index| match list.get(index) {
            Some(ValueOrContainer::Container(container)) => Some(container.id().to_string()),
            _ => None,
        })
        .collect()
}

fn plain_page(model: Model, blocks: usize, seed: u64) -> Built {
    build_base(BaseSpec {
        model,
        blocks,
        seed,
        history: History::None,
    })
}

// ---------------------------------------------------------------------------------------------
// Boundary cases: limits crossed by exactly one
// ---------------------------------------------------------------------------------------------

fn boundary(label: String, kind: &'static str, base: &Arc<Vec<u8>>, update: Vec<u8>, expect: Expect) -> Case {
    Case {
        label,
        family: "boundary",
        kind,
        base: Arc::clone(base),
        update,
        expect: Some(expect),
        facts: FactsFrom::Base,
    }
}

/// A flat editor page with `live` blocks and, when set, a total of `aliases` alias entries (the
/// surplus as `__deleted__` tombstones), built in one commit. Texts are `text_of(index)`.
pub(super) fn big_flat_page(
    seed: u64,
    live: usize,
    aliases: usize,
    text_of: impl Fn(usize) -> String,
) -> LoroCollabEngine {
    let mut engine = LoroCollabEngine::new_empty(seed);
    let list: Vec<EditorInitialBlock> = (0..live)
        .map(|index| EditorInitialBlock {
            id: format!("b{index}"),
            kind: "paragraph".into(),
            text: text_of(index),
            indent: 0,
            level: None,
        })
        .collect();
    initialize_editor_doc(&engine.doc, &list).unwrap();
    for index in live..aliases {
        engine
            .doc
            .get_map(ALIASES)
            .insert(&format!("tomb{index}"), "__deleted__")
            .unwrap();
    }
    engine.doc.commit();
    engine.invalidate_editor_model_floor();
    engine
}

fn snapshot_arc(engine: &LoroCollabEngine) -> Arc<Vec<u8>> {
    Arc::new(engine.export_snapshot().unwrap())
}

/// The 65,536-byte boundary of the update: `(bytes_at_limit_or_below, bytes_one_over)` for an
/// insert of ASCII text into the first block.
fn update_size_pair(base: &[u8]) -> (Vec<u8>, Vec<u8>) {
    let make = |chars: usize| {
        raw_edit(base, 77_001, |raw| {
            let list = children(raw);
            let Some(ValueOrContainer::Container(Container::Map(map))) = list.get(0) else {
                panic!("no first block")
            };
            let Some(ValueOrContainer::Container(Container::List(inline))) = map.get("children") else {
                panic!("no inline list")
            };
            let text = inline.insert_container(inline.len(), LoroText::new()).unwrap();
            text.insert(0, &"a".repeat(chars)).unwrap();
        })
    };
    let mut chars = 65_000;
    loop {
        let update = make(chars);
        match update.len().cmp(&crate::limits::UPDATE_BYTES_MAX) {
            std::cmp::Ordering::Less => chars += crate::limits::UPDATE_BYTES_MAX - update.len(),
            std::cmp::Ordering::Equal => return (update, make(chars + 1)),
            std::cmp::Ordering::Greater => chars -= 1,
        }
    }
}

#[allow(clippy::too_many_lines)]
pub(super) fn boundary_cases() -> Vec<Case> {
    let limit = crate::limits::DOCUMENT_BLOCK_COUNT_MAX;
    let mut cases = Vec::new();

    // document_block_count on a flat editor page, by raw block insert without an alias.
    for (live, add, name) in [
        (limit - 1, 1, "block_count_to_limit"),
        (limit - 1, 2, "block_count_one_over"),
        (limit, 1, "block_count_at_limit_plus_one"),
        (limit, 0, "block_count_at_limit_no_change"),
    ] {
        let engine = big_flat_page(11, live, live, |_| String::new());
        let base = snapshot_arc(&engine);
        let update = raw_edit(&base, 77_010, |raw| {
            for _ in 0..add {
                raw_block(raw, None, "", Place::Unplaced);
            }
            if add == 0 {
                raw.get_map("order").insert("noop", "m").unwrap();
            }
        });
        let expect = if live + add > limit {
            over("document_block_count", (live + add) as u64, limit as u64)
        } else {
            Expect::Accept
        };
        cases.push(boundary(format!("flat {name}"), "block_count", &base, update, expect));
    }
    // The same crossing with an alias per new block: the alias map crosses its own ceiling first.
    {
        let engine = big_flat_page(12, limit, limit, |_| String::new());
        let base = snapshot_arc(&engine);
        let update = raw_edit(&base, 77_011, |raw| {
            raw_block(raw, Some("one-too-many"), "", Place::Unplaced);
        });
        cases.push(boundary(
            "flat block_count_with_alias".into(),
            "block_count",
            &base,
            update,
            refuse("editor_block_alias_limit_exceeded"),
        ));
    }
    // A delete and an insert at the limit leave the count where it is.
    {
        let engine = big_flat_page(13, limit, limit, |_| String::new());
        let base = snapshot_arc(&engine);
        let update = raw_edit(&base, 77_012, |raw| {
            children(raw).delete(0, 1).unwrap();
            raw_block(raw, None, "", Place::Unplaced);
        });
        cases.push(boundary(
            "flat delete_one_insert_one_at_limit".into(),
            "block_count",
            &base,
            update,
            Expect::Accept,
        ));
        let update = raw_edit(&base, 77_013, |raw| {
            children(raw).delete(0, 1).unwrap();
            raw_block(raw, None, "", Place::Unplaced);
            raw_block(raw, None, "", Place::Unplaced);
        });
        cases.push(boundary(
            "flat delete_one_insert_two_at_limit".into(),
            "block_count",
            &base,
            update,
            over("document_block_count", limit as u64 + 1, limit as u64),
        ));
    }
    // 10,000 aliases are readable, the 10,001st is not.
    {
        let engine = big_flat_page(14, 1, limit, |_| String::new());
        let base = snapshot_arc(&engine);
        let update = raw_edit(&base, 77_014, |raw| {
            raw.get_map(ALIASES).insert("alias-10001", "__deleted__").unwrap();
        });
        cases.push(boundary(
            "flat alias_10001".into(),
            "alias_count",
            &base,
            update,
            refuse("editor_block_alias_limit_exceeded"),
        ));
    }

    // Structured page at the block ceiling: a block with its placement node.
    for (add, name) in [
        (1, "structured block_count_to_limit"),
        (2, "structured block_count_one_over"),
    ] {
        let mut engine = LoroCollabEngine::new_empty(15);
        let list: Vec<EditorInitialBlock> = (0..limit - 1)
            .map(|index| EditorInitialBlock {
                id: format!("b{index}"),
                kind: "paragraph".into(),
                text: String::new(),
                indent: 0,
                level: None,
            })
            .collect();
        initialize_editor_doc(&engine.doc, &list).unwrap();
        engine.doc.commit();
        engine.invalidate_editor_model_floor();
        engine.initialize_page_structure_model(4).unwrap();
        let base = snapshot_arc(&engine);
        let update = raw_edit(&base, 77_015, |raw| {
            for _ in 0..add {
                raw_block(raw, None, "", Place::Root);
            }
        });
        let expect = if add == 1 {
            Expect::Accept
        } else {
            over("document_block_count", limit as u64 + 1, limit as u64)
        };
        cases.push(boundary(name.into(), "block_count", &base, update, expect));
    }

    // Legacy tree: block_count, container_count (navigators) and depth.
    for (kind_name, kind, limit_kind) in [
        ("block", NodeKind::Block, "document_block_count"),
        ("navigator", NodeKind::NavigatorNode, "container_count"),
    ] {
        let mut author = LoroCollabEngine::new_empty(16);
        for index in 0..limit {
            assert!(create(&mut author, &format!("t{index}"), None, 0, kind));
        }
        let base = snapshot_arc(&author);
        for (add, expect) in [
            (0_usize, Expect::Accept),
            (1, over(limit_kind, limit as u64 + 1, limit as u64)),
        ] {
            let update = author_update(&author, 77_020 + add as u64, |e| {
                for index in 0..add {
                    create(e, &format!("x{index}"), None, 0, kind);
                }
                if add == 0 {
                    let _ = e.set_title("t");
                }
            });
            cases.push(boundary(
                format!("legacy {kind_name}_count_at_limit_plus_{add}"),
                limit_kind,
                &base,
                update,
                expect,
            ));
        }
        // One below the ceiling: +1 reaches it exactly, +2 passes it.
        let mut below = LoroCollabEngine::new_empty(17);
        for index in 0..limit - 1 {
            assert!(create(&mut below, &format!("t{index}"), None, 0, kind));
        }
        let base = snapshot_arc(&below);
        for (add, expect) in [
            (1_usize, Expect::Accept),
            (2, over(limit_kind, limit as u64 + 1, limit as u64)),
        ] {
            let update = author_update(&below, 77_030 + add as u64, |e| {
                for index in 0..add {
                    create(e, &format!("x{index}"), None, 0, kind);
                }
            });
            cases.push(boundary(
                format!("legacy {kind_name}_count_below_limit_plus_{add}"),
                limit_kind,
                &base,
                update,
                expect,
            ));
        }
    }
    // A delete and a create at the ceiling leave the count at 10,000; two creates pass it.
    for (name, kind, limit_kind) in [
        ("navigator", NodeKind::NavigatorNode, "container_count"),
        ("block", NodeKind::Block, "document_block_count"),
    ] {
        let mut author = LoroCollabEngine::new_empty(18);
        for index in 0..limit {
            assert!(create(&mut author, &format!("t{index}"), None, 0, kind));
        }
        let base = snapshot_arc(&author);
        for (creates, expect) in [
            (1_usize, Expect::Accept),
            (2, over(limit_kind, limit as u64 + 1, limit as u64)),
        ] {
            let update = author_update(&author, 77_040 + creates as u64, |e| {
                let _ = e.apply_content_operation(&Operation::DeleteNode { id: NodeId::from("t0") });
                for index in 0..creates {
                    create(e, &format!("swap{index}"), None, 0, kind);
                }
            });
            cases.push(boundary(
                format!("legacy {name}_delete_one_create_{creates}_at_limit"),
                limit_kind,
                &base,
                update,
                expect,
            ));
        }
    }
    // Tree depth: a chain of 32 ancestors is the ceiling.
    {
        let mut author = LoroCollabEngine::new_empty(19);
        let mut parent: Option<String> = None;
        for index in 0..=crate::limits::TREE_DEPTH_MAX {
            let id = format!("d{index}");
            assert!(create(&mut author, &id, parent.as_deref(), 0, NodeKind::NavigatorNode));
            parent = Some(id);
        }
        let base = snapshot_arc(&author);
        let leaf = parent.unwrap();
        for (kind, parent_id, expect) in [
            ("depth_33_child_of_leaf", leaf, over("tree_depth", 33, 32)),
            (
                "depth_32_child_of_its_parent",
                format!("d{}", crate::limits::TREE_DEPTH_MAX - 1),
                Expect::Accept,
            ),
        ] {
            let update = author_update(&author, 77_050, |e| {
                create(e, "deep", Some(&parent_id), 0, NodeKind::NavigatorNode);
            });
            cases.push(boundary(format!("legacy {kind}"), "tree_depth", &base, update, expect));
        }
        // Moving the whole 33-node chain under a new root node pushes its leaf to depth 33.
        let update = author_update(&author, 77_051, |e| {
            create(e, "newroot", None, 0, NodeKind::NavigatorNode);
            let _ = e.apply_content_operation(&Operation::MoveNode {
                id: NodeId::from("d0"),
                new_parent: Some(NodeId::from("newroot")),
                index: 0,
            });
        });
        cases.push(boundary(
            "legacy depth_by_moving_the_chain".into(),
            "tree_depth",
            &base,
            update,
            over("tree_depth", 33, 32),
        ));
    }
    // Text of one block at 100,000 characters, and in total at 1,000,000.
    {
        let at = crate::limits::TEXT_BLOCK_CHARS_MAX;
        let engine = big_flat_page(
            20,
            2,
            2,
            |index| if index == 0 { "a".repeat(at) } else { String::new() },
        );
        let base = snapshot_arc(&engine);
        for (chars, expect) in [
            (0_usize, Expect::Accept),
            (1, over("text_block_chars", at as u64 + 1, at as u64)),
        ] {
            let update = raw_edit(&base, 77_060, |raw| {
                let list = children(raw);
                let Some(ValueOrContainer::Container(Container::Map(map))) = list.get(0) else {
                    panic!("no first block")
                };
                let Some(ValueOrContainer::Container(Container::List(inline))) = map.get("children") else {
                    panic!("no inline list")
                };
                let Some(ValueOrContainer::Container(Container::Text(text))) = inline.get(0) else {
                    panic!("no text")
                };
                if chars == 0 {
                    raw.get_map("order").insert("noop", "m").unwrap();
                } else {
                    text.insert(0, &"b".repeat(chars)).unwrap();
                }
            });
            cases.push(boundary(
                format!("flat block_text_at_limit_plus_{chars}"),
                "text_block_chars",
                &base,
                update,
                expect,
            ));
        }
        let total = crate::limits::DOCUMENT_TEXT_CHARS_MAX;
        let engine = big_flat_page(
            21,
            11,
            11,
            |index| if index < 10 { "a".repeat(at) } else { String::new() },
        );
        let base = snapshot_arc(&engine);
        for (chars, expect) in [
            (0_usize, Expect::Accept),
            (1, over("document_text_chars", total as u64 + 1, total as u64)),
        ] {
            let update = raw_edit(&base, 77_061, |raw| {
                let list = children(raw);
                let Some(ValueOrContainer::Container(Container::Map(map))) = list.get(10) else {
                    panic!("no eleventh block")
                };
                let Some(ValueOrContainer::Container(Container::List(inline))) = map.get("children") else {
                    panic!("no inline list")
                };
                if chars == 0 {
                    raw.get_map("order").insert("noop", "m").unwrap();
                } else {
                    inline
                        .insert_container(0, LoroText::new())
                        .unwrap()
                        .insert(0, &"c".repeat(chars))
                        .unwrap();
                }
            });
            cases.push(boundary(
                format!("flat document_text_at_limit_plus_{chars}"),
                "document_text_chars",
                &base,
                update,
                expect,
            ));
        }
    }
    // The update's own byte ceiling.
    {
        let mut author = LoroCollabEngine::new_empty(22);
        initialize_editor_doc(
            &author.doc,
            &[EditorInitialBlock {
                id: "b0".into(),
                kind: "paragraph".into(),
                text: String::new(),
                indent: 0,
                level: None,
            }],
        )
        .unwrap();
        author.doc.commit();
        author.invalidate_editor_model_floor();
        let base = snapshot_arc(&author);
        let (at, over_by_one) = update_size_pair(&base);
        assert_eq!(at.len(), crate::limits::UPDATE_BYTES_MAX);
        cases.push(boundary(
            "update_bytes_exactly_at_limit".into(),
            "update_bytes",
            &base,
            at,
            Expect::Accept,
        ));
        cases.push(boundary(
            "update_bytes_one_over".into(),
            "update_bytes",
            &base,
            over_by_one,
            Expect::Refuse {
                code: "input_too_large",
                limit_kind: "update_bytes",
                observed: crate::limits::UPDATE_BYTES_MAX as u64 + 1,
                limit: crate::limits::UPDATE_BYTES_MAX as u64,
            },
        ));
    }
    // Structured depth 7 (the placement ceiling) and 8.
    {
        let built = plain_page(Model::Structured4, 1, 23);
        let mut author = built.author;
        let mut parent = "b0".to_string();
        for depth in 1..=7 {
            let id = format!("deep{depth}");
            assert!(
                create(&mut author, &id, Some(&parent), 0, NodeKind::Block),
                "depth {depth}"
            );
            parent = id;
        }
        let base = snapshot_arc(&author);
        for (name, parent_id, expect) in [
            ("depth_7_leaf_under_depth_6", "deep6", Expect::Accept),
            (
                "depth_8_child_of_depth_7",
                "deep7",
                refuse("editor_structure_depth_limit"),
            ),
        ] {
            let update = raw_edit(&base, 77_070, |raw| {
                let cid = {
                    let aliases = raw.get_map(ALIASES);
                    match aliases.get(parent_id) {
                        Some(ValueOrContainer::Value(LoroValue::String(cid))) => cid.to_string(),
                        _ => panic!("no alias {parent_id}"),
                    }
                };
                raw_block(raw, None, "", Place::Under(cid.as_str()));
            });
            cases.push(boundary(
                format!("structured {name}"),
                "placement_depth",
                &base,
                update,
                expect,
            ));
        }
    }
    cases
}

// ---------------------------------------------------------------------------------------------
// Hostile corpus
// ---------------------------------------------------------------------------------------------

pub(super) fn hostile(name: &str, kind: &'static str, base: &Arc<Vec<u8>>, update: Vec<u8>) -> Case {
    Case {
        label: name.to_owned(),
        family: "hostile",
        kind,
        base: Arc::clone(base),
        update,
        // Filled in from the recorded table by `record`.
        expect: None,
        facts: FactsFrom::Base,
    }
}

/// An update whose root-list deletes are re-encoded through JSON with a forged `start_id` or
/// `len` (the review's F1). The author's honest `pos` is kept.
fn forged_delete_update(
    base: &[u8],
    peer: u64,
    edit: impl FnOnce(&LoroDoc),
    forged_id: impl FnOnce(&LoroDoc) -> Option<String>,
    forged_len: Option<i32>,
) -> Option<Vec<u8>> {
    let author = load_raw(base, peer);
    let from = author.oplog_vv();
    edit(&author);
    author.commit();
    let root = children(&author).id().to_string();
    let forged = forged_id(&author);
    let mut json = serde_json::to_value(
        author
            .export_json_updates_without_peer_compression(&from, &author.oplog_vv())
            .unwrap(),
    )
    .unwrap();
    for change in json["changes"].as_array_mut().unwrap() {
        for op in change["ops"].as_array_mut().unwrap() {
            if op["container"] == root && op["content"]["type"] == "delete" {
                if let Some(id) = &forged {
                    op["content"]["start_id"] = Value::String(id.clone());
                }
                if let Some(len) = forged_len {
                    op["content"]["len"] = Value::from(len);
                }
            }
        }
    }
    let rewritten = LoroDoc::new();
    rewritten.import(base).unwrap();
    // A forged delete that the library itself cannot apply (a length beyond the list) cannot be
    // re-encoded through a document: such a shape is not constructible this way.
    rewritten
        .import_json_updates(serde_json::to_string(&json).unwrap())
        .ok()?;
    Some(rewritten.export(ExportMode::updates(&from)).unwrap())
}

/// Adds a forged-delete shape when it could be constructed.
fn push_forged(cases: &mut Vec<Case>, name: &str, base: &Arc<Vec<u8>>, update: Option<Vec<u8>>) {
    match update {
        Some(update) => cases.push(hostile(name, "forged_delete", base, update)),
        None => println!("hostile shape not constructible: {name}"),
    }
}

fn element_id_of(raw: &LoroDoc, index: usize) -> String {
    super::floor_equivalence_tests::id_text(&match children(raw).get(index) {
        Some(ValueOrContainer::Container(container)) => container.id(),
        _ => panic!("no element at {index}"),
    })
}

fn write_root(raw: &LoroDoc, name: &str, kind: ContainerType) {
    match kind {
        ContainerType::Text => raw.get_text(name).insert(0, "x").unwrap(),
        ContainerType::List => raw.get_list(name).insert(0, 1).unwrap(),
        ContainerType::MovableList => raw.get_movable_list(name).insert(0, 1).unwrap(),
        ContainerType::Map => raw.get_map(name).insert("k", 1).unwrap(),
        ContainerType::Tree => {
            raw.get_tree(name).create(None).unwrap();
        }
        other => panic!("unsupported kind {other:?}"),
    }
}

/// A fixed set of cheap bases the hostile shapes are applied to.
struct Bases {
    legacy: Built,
    flat: Built,
    s4: Built,
    s5: Built,
}

fn bases() -> Bases {
    Bases {
        legacy: plain_page(Model::LegacyTree, 6, 101),
        flat: plain_page(Model::Flat, 6, 102),
        s4: plain_page(Model::Structured4, 6, 103),
        s5: plain_page(Model::Structured5, 6, 104),
    }
}

fn first_cid(raw: &LoroDoc) -> String {
    block_cids(raw).into_iter().next().unwrap()
}

fn inline_list(raw: &LoroDoc, index: usize) -> LoroList {
    let Some(ValueOrContainer::Container(Container::Map(map))) = children(raw).get(index) else {
        panic!("no block at {index}")
    };
    let Some(ValueOrContainer::Container(Container::List(list))) = map.get("children") else {
        panic!("no inline list")
    };
    list
}

fn block_map(raw: &LoroDoc, index: usize) -> LoroMap {
    let Some(ValueOrContainer::Container(Container::Map(map))) = children(raw).get(index) else {
        panic!("no block at {index}")
    };
    map
}

/// Every forged-document and forged-update shape. The decisions are attached by
/// [`hostile_cases`].
#[allow(clippy::too_many_lines)]
fn hostile_shapes() -> Vec<Case> {
    let b = bases();
    let mut cases = Vec::new();

    // --- forged root-list deletes (review F1) ---
    {
        // A flat page whose only floor-5 support is a raw atom block map.
        let mut engine = plain_page(Model::Flat, 4, 110).author;
        let atom_update = raw_update(&engine, 50_110, |raw| {
            push_raw_atom(raw, "image");
        });
        engine.import_update(&atom_update).unwrap();
        let base = snapshot_arc(&engine);
        let atom_at = children(&engine.doc).len() - 1;
        let atom_id = element_id_of(&engine.doc, atom_at);
        let plain_id = element_id_of(&engine.doc, 0);
        cases.push(hostile(
            "forged_delete: honest delete of the atom",
            "forged_delete",
            &base,
            raw_edit(&base, 90_001, |raw| children(raw).delete(atom_at, 1).unwrap()),
        ));
        push_forged(
            &mut cases,
            "forged_delete: delete a plain block, start_id names the atom",
            &base,
            forged_delete_update(
                &base,
                90_002,
                |raw| children(raw).delete(0, 1).unwrap(),
                |_| Some(atom_id.clone()),
                None,
            ),
        );
        push_forged(
            &mut cases,
            "forged_delete: delete the atom, start_id names a plain block",
            &base,
            forged_delete_update(
                &base,
                90_003,
                |raw| children(raw).delete(atom_at, 1).unwrap(),
                |_| Some(plain_id.clone()),
                None,
            ),
        );
        push_forged(
            &mut cases,
            "forged_delete: delete a plain block, len forged to 1000000",
            &base,
            forged_delete_update(
                &base,
                90_004,
                |raw| children(raw).delete(0, 1).unwrap(),
                |_| None,
                Some(1_000_000),
            ),
        );
        push_forged(
            &mut cases,
            "forged_delete: delete the atom, len forged to 0",
            &base,
            forged_delete_update(
                &base,
                90_005,
                |raw| children(raw).delete(atom_at, 1).unwrap(),
                |_| None,
                Some(0),
            ),
        );
        push_forged(
            &mut cases,
            "forged_delete: delete the atom, len forged to -1",
            &base,
            forged_delete_update(
                &base,
                90_006,
                |raw| children(raw).delete(atom_at, 1).unwrap(),
                |_| None,
                Some(-1),
            ),
        );
    }

    // --- reserved names with the wrong container type, on every kind of base ---
    for (base_name, built) in [
        ("legacy", &b.legacy),
        ("model1", &b.flat),
        ("model4", &b.s4),
        ("model5", &b.s5),
    ] {
        for (name, expected) in RESERVED_ROOTS {
            for kind in super::floor_equivalence_tests::SHADOWABLE {
                if kind == expected {
                    continue;
                }
                let update = raw_edit(&built.snapshot, 91_000, |raw| write_root(raw, name, kind));
                cases.push(hostile(
                    &format!("shadow_root: {base_name} {name} as {kind:?}"),
                    "shadow_root",
                    &built.snapshot,
                    update,
                ));
            }
        }
    }
    // Reserved names with the right type are ordinary writes; near-miss names are not reserved.
    for (name, kind) in [
        ("Doc", ContainerType::List),
        ("doc ", ContainerType::Text),
        ("orderx", ContainerType::Tree),
    ] {
        let update = raw_edit(&b.s4.snapshot, 91_100, |raw| write_root(raw, name, kind));
        cases.push(hostile(
            &format!("reserved_name_near_miss: model4 {name:?} as {kind:?}"),
            "reserved_name_near_miss",
            &b.s4.snapshot,
            update,
        ));
    }
    {
        // A wrong-typed `doc` reached only through a child container.
        let update = raw_edit(&b.flat.snapshot, 91_200, |raw| {
            raw.get_list(DOC).insert_container(0, LoroMap::new()).unwrap();
        });
        cases.push(hostile(
            "shadow_root: model1 doc as list with a child map",
            "shadow_root",
            &b.flat.snapshot,
            update,
        ));
        // A right-typed reserved root beside a wrong-typed one in one update.
        let update = raw_edit(&b.flat.snapshot, 91_201, |raw| {
            raw.get_map(ORDER).insert("k", "m").unwrap();
            raw.get_text(ORDER).insert(0, "t").unwrap();
        });
        cases.push(hostile(
            "shadow_root: model1 order map and order text in one update",
            "shadow_root",
            &b.flat.snapshot,
            update,
        ));
        // An update whose first op is harmless and whose last is a shadow.
        let update = raw_edit(&b.s4.snapshot, 91_202, |raw| {
            raw.get_map(META).insert("note", "ok").unwrap();
            raw.get_list(META).insert(0, 1).unwrap();
        });
        cases.push(hostile(
            "shadow_root: model4 meta map write then meta list write",
            "shadow_root",
            &b.s4.snapshot,
            update,
        ));
    }
    // A page that already carries a shadow root takes ordinary writes, and a second one.
    {
        let mut engine = plain_page(Model::Flat, 4, 120).author;
        let shadow = raw_update(&engine, 50_120, |raw| {
            raw.get_list(ALIASES).insert(0, 1).unwrap();
        });
        engine.import_update(&shadow).unwrap();
        let base = snapshot_arc(&engine);
        cases.push(hostile(
            "pre_existing_shadow: base has aliases as list, ordinary text edit",
            "pre_existing_shadow",
            &base,
            raw_edit(&base, 91_300, |raw| {
                inline_list(raw, 0)
                    .insert_container(0, LoroText::new())
                    .unwrap()
                    .insert(0, "ok")
                    .unwrap();
            }),
        ));
        cases.push(hostile(
            "pre_existing_shadow: base has aliases as list, a second shadow",
            "pre_existing_shadow",
            &base,
            raw_edit(&base, 91_301, |raw| {
                raw.get_text(ALIASES).insert(0, "t").unwrap();
            }),
        ));
    }

    // --- an update that lowers the editor-model floor ---
    {
        let atoms_s5 = |raw: &LoroDoc| {
            let list = children(raw);
            (0..list.len())
                .rev()
                .filter(|index| {
                    matches!(
                        list.get(*index),
                        Some(ValueOrContainer::Container(Container::Map(map)))
                            if matches!(map.get("nodeName"), Some(ValueOrContainer::Value(LoroValue::String(kind))) if kind.to_string() == "image")
                    )
                })
                .collect::<Vec<_>>()
        };
        let base = &b.s5.snapshot;
        cases.push(hostile(
            "floor_downgrade: model5 marker flipped to v4 only",
            "floor_downgrade",
            base,
            raw_edit(base, 92_001, |raw| set_model(raw, "rich_page_v4")),
        ));
        cases.push(hostile(
            "floor_downgrade: model5 marker flipped and every atom deleted",
            "floor_downgrade",
            base,
            raw_edit(base, 92_002, |raw| {
                set_model(raw, "rich_page_v4");
                for index in atoms_s5(raw) {
                    children(raw).delete(index, 1).unwrap();
                }
            }),
        ));
        cases.push(hostile(
            "floor_downgrade: model5 atom deleted, marker kept",
            "floor_downgrade",
            base,
            raw_edit(base, 92_003, |raw| {
                for index in atoms_s5(raw) {
                    children(raw).delete(index, 1).unwrap();
                }
            }),
        ));
        cases.push(hostile(
            "floor_downgrade: model5 atom retyped to a paragraph and marker flipped",
            "floor_downgrade",
            base,
            raw_edit(base, 92_004, |raw| {
                set_model(raw, "rich_page_v4");
                for index in atoms_s5(raw) {
                    block_map(raw, index).insert("nodeName", "paragraph").unwrap();
                }
            }),
        ));
        cases.push(hostile(
            "floor_downgrade: model5 marker removed by replacing the meta map entry",
            "floor_downgrade",
            base,
            raw_edit(base, 92_005, |raw| {
                raw.get_map(META).delete("model").unwrap();
            }),
        ));
        // A flat page held at floor 5 only by a raw atom.
        let mut engine = plain_page(Model::Flat, 4, 130).author;
        let atom_update = raw_update(&engine, 50_130, |raw| {
            push_raw_atom(raw, "attachment");
        });
        engine.import_update(&atom_update).unwrap();
        let flat5 = snapshot_arc(&engine);
        cases.push(hostile(
            "floor_downgrade: flat page held by a raw atom, atom deleted",
            "floor_downgrade",
            &flat5,
            raw_edit(&flat5, 92_006, |raw| {
                let at = children(raw).len() - 1;
                children(raw).delete(at, 1).unwrap();
            }),
        ));
        cases.push(hostile(
            "floor_downgrade: flat page held by a raw atom, doc.children replaced",
            "floor_downgrade",
            &flat5,
            raw_edit(&flat5, 92_007, |raw| {
                let list = raw.get_map(DOC).insert_container("children", LoroList::new()).unwrap();
                let map = list.insert_container(0, LoroMap::new()).unwrap();
                map.insert("nodeName", "paragraph").unwrap();
                map.insert_container("attributes", LoroMap::new()).unwrap();
                map.insert_container("children", LoroList::new()).unwrap();
            }),
        ));
    }

    // --- an update that changes only what a read could register ---
    {
        cases.push(hostile(
            "root_by_read: legacy page, only the order map",
            "root_by_read",
            &b.legacy.snapshot,
            raw_edit(&b.legacy.snapshot, 93_001, |raw| {
                raw.get_map(ORDER).insert("n0", "a").unwrap();
            }),
        ));
        cases.push(hostile(
            "root_by_read: legacy page, an alias map write without an editor document",
            "root_by_read",
            &b.legacy.snapshot,
            raw_edit(&b.legacy.snapshot, 93_002, |raw| {
                raw.get_map(ALIASES).insert("n0", "__deleted__").unwrap();
            }),
        ));
        cases.push(hostile(
            "root_by_read: legacy page, structure meta written without a tree",
            "root_by_read",
            &b.legacy.snapshot,
            raw_edit(&b.legacy.snapshot, 93_003, |raw| {
                let meta = raw.get_map(META);
                meta.insert("version", 1).unwrap();
                meta.insert("initialized", true).unwrap();
                meta.insert("model", "rich_page_v4").unwrap();
            }),
        ));
        cases.push(hostile(
            "root_by_read: model1 page, structure meta initialised without a tree",
            "root_by_read",
            &b.flat.snapshot,
            raw_edit(&b.flat.snapshot, 93_004, |raw| {
                let meta = raw.get_map(META);
                meta.insert("version", 1).unwrap();
                meta.insert("initialized", true).unwrap();
                meta.insert("model", "rich_page_v4").unwrap();
            }),
        ));
        cases.push(hostile(
            "root_by_read: model1 page, unknown root of every type",
            "root_by_read",
            &b.flat.snapshot,
            raw_edit(&b.flat.snapshot, 93_005, |raw| {
                write_root(raw, "unknown_a", ContainerType::Map);
                write_root(raw, "unknown_b", ContainerType::List);
                write_root(raw, "unknown_c", ContainerType::Tree);
            }),
        ));
    }

    // --- structure meta and the editor document replaced or malformed ---
    {
        let base = &b.s4.snapshot;
        cases.push(hostile(
            "meta: model4 initialized flipped to false",
            "structure_meta",
            base,
            raw_edit(base, 94_001, |raw| {
                raw.get_map(META).insert("initialized", false).unwrap();
            }),
        ));
        cases.push(hostile(
            "meta: model4 version set to 2",
            "structure_meta",
            base,
            raw_edit(base, 94_002, |raw| {
                raw.get_map(META).insert("version", 2).unwrap();
            }),
        ));
        cases.push(hostile(
            "meta: model4 model set to an unknown name",
            "structure_meta",
            base,
            raw_edit(base, 94_003, |raw| {
                raw.get_map(META).insert("model", "rich_page_v9").unwrap();
            }),
        ));
        cases.push(hostile(
            "meta: model4 initialized flipped to true is already true; model set to v5",
            "structure_meta",
            base,
            raw_edit(base, 94_004, |raw| {
                raw.get_map(META).insert("model", "rich_page_v5").unwrap();
            }),
        ));
        cases.push(hostile(
            "doc: model4 doc.children replaced by an empty list",
            "doc_replaced",
            base,
            raw_edit(base, 94_005, |raw| {
                raw.get_map(DOC).insert_container("children", LoroList::new()).unwrap();
            }),
        ));
        cases.push(hostile(
            "doc: model1 doc.children replaced by one block",
            "doc_replaced",
            &b.flat.snapshot,
            raw_edit(&b.flat.snapshot, 94_006, |raw| {
                let list = raw.get_map(DOC).insert_container("children", LoroList::new()).unwrap();
                let map = list.insert_container(0, LoroMap::new()).unwrap();
                map.insert("nodeName", "paragraph").unwrap();
                map.insert_container("attributes", LoroMap::new()).unwrap();
                map.insert_container("children", LoroList::new()).unwrap();
            }),
        ));
        cases.push(hostile(
            "doc: model1 doc.children replaced by a scalar",
            "doc_replaced",
            &b.flat.snapshot,
            raw_edit(&b.flat.snapshot, 94_007, |raw| {
                raw.get_map(DOC).insert("children", "not a list").unwrap();
            }),
        ));
        cases.push(hostile(
            "doc: model1 doc nodeName set to another kind",
            "doc_replaced",
            &b.flat.snapshot,
            raw_edit(&b.flat.snapshot, 94_008, |raw| {
                raw.get_map(DOC).insert("nodeName", "other").unwrap();
            }),
        ));
        cases.push(hostile(
            "doc: model1 doc attributes replaced by a scalar",
            "doc_replaced",
            &b.flat.snapshot,
            raw_edit(&b.flat.snapshot, 94_009, |raw| {
                raw.get_map(DOC).insert("attributes", 7).unwrap();
            }),
        ));
    }

    // --- reader validity of one block ---
    {
        let base = &b.flat.snapshot;
        cases.push(hostile(
            "block: a scalar element in doc.children",
            "block_malformed",
            base,
            raw_edit(base, 95_001, |raw| {
                children(raw).insert(0, "not a map").unwrap();
            }),
        ));
        cases.push(hostile(
            "block: a list element in doc.children",
            "block_malformed",
            base,
            raw_edit(base, 95_002, |raw| {
                children(raw).insert_container(0, LoroList::new()).unwrap();
            }),
        ));
        cases.push(hostile(
            "block: nodeName deleted",
            "block_malformed",
            base,
            raw_edit(base, 95_003, |raw| {
                block_map(raw, 0).delete("nodeName").unwrap();
            }),
        ));
        cases.push(hostile(
            "block: nodeName retyped to a number",
            "block_malformed",
            base,
            raw_edit(base, 95_004, |raw| {
                block_map(raw, 0).insert("nodeName", 5).unwrap();
            }),
        ));
        cases.push(hostile(
            "block: nodeName set to an unsupported kind",
            "block_malformed",
            base,
            raw_edit(base, 95_005, |raw| {
                block_map(raw, 0).insert("nodeName", "script").unwrap();
            }),
        ));
        cases.push(hostile(
            "block: inline child is a map",
            "block_malformed",
            base,
            raw_edit(base, 95_006, |raw| {
                inline_list(raw, 0).insert_container(0, LoroMap::new()).unwrap();
            }),
        ));
        cases.push(hostile(
            "block: inline child is a scalar",
            "block_malformed",
            base,
            raw_edit(base, 95_007, |raw| {
                inline_list(raw, 0).insert(0, "text").unwrap();
            }),
        ));
        cases.push(hostile(
            "block: children is not a list",
            "block_malformed",
            base,
            raw_edit(base, 95_008, |raw| {
                block_map(raw, 0).insert("children", "x").unwrap();
            }),
        ));
        cases.push(hostile(
            "block: attributes replaced by a scalar",
            "block_malformed",
            base,
            raw_edit(base, 95_009, |raw| {
                block_map(raw, 0).insert("attributes", "x").unwrap();
            }),
        ));
        cases.push(hostile(
            "block: image atom without an asset id",
            "atom_invalid",
            base,
            raw_edit(base, 95_010, |raw| {
                let map = block_map(raw, 0);
                map.insert("nodeName", "image").unwrap();
            }),
        ));
        cases.push(hostile(
            "block: valid atom carrying an inline text child",
            "atom_invalid",
            base,
            raw_edit(base, 95_011, |raw| {
                let cid = push_raw_atom(raw, "image");
                let list = children(raw);
                for index in 0..list.len() {
                    if let Some(ValueOrContainer::Container(container)) = list.get(index)
                        && container.id() == cid
                    {
                        let inline = inline_list(raw, index);
                        inline
                            .insert_container(0, LoroText::new())
                            .unwrap()
                            .insert(0, "x")
                            .unwrap();
                    }
                }
            }),
        ));
        cases.push(hostile(
            "block: atom with an unknown attribute",
            "atom_invalid",
            base,
            raw_edit(base, 95_012, |raw| {
                let cid = push_raw_atom(raw, "image");
                let list = children(raw);
                for index in 0..list.len() {
                    if let Some(ValueOrContainer::Container(container)) = list.get(index)
                        && container.id() == cid
                    {
                        let Some(ValueOrContainer::Container(Container::Map(attrs))) =
                            block_map(raw, index).get("attributes")
                        else {
                            panic!("no attributes")
                        };
                        attrs.insert("src", "https://example.test/x.png").unwrap();
                    }
                }
            }),
        ));
        cases.push(hostile(
            "block: valid raw atom appended to a flat page",
            "atom_invalid",
            base,
            raw_edit(base, 95_013, |raw| {
                push_raw_atom(raw, "childPage");
            }),
        ));
        // Marks: unsafe href and unsupported mark keys.
        let mark_update = |peer: u64, key: &str, value: LoroValue| {
            raw_edit(base, peer, |raw| {
                let text = inline_list(raw, 0).insert_container(0, LoroText::new()).unwrap();
                text.insert(0, "marked text").unwrap();
                text.mark(0..6, key, value).unwrap();
            })
        };
        cases.push(hostile(
            "mark: javascript href",
            "mark_unsafe",
            base,
            mark_update(95_020, "link", LoroValue::from("javascript:alert(1)")),
        ));
        cases.push(hostile(
            "mark: link with a safe href",
            "mark_unsafe",
            base,
            mark_update(95_021, "link", LoroValue::from("https://example.test/")),
        ));
        cases.push(hostile(
            "mark: data URI href",
            "mark_unsafe",
            base,
            mark_update(95_022, "link", LoroValue::from("data:text/html,x")),
        ));
        cases.push(hostile(
            "mark: unknown mark key",
            "mark_unsafe",
            base,
            mark_update(95_023, "underline", LoroValue::from(true)),
        ));
    }

    // --- oversized values, depth and fan-out extremes ---
    {
        let base = &b.flat.snapshot;
        cases.push(hostile(
            "oversized: alias value of 60000 bytes",
            "oversized",
            base,
            raw_edit(base, 96_001, |raw| {
                raw.get_map(ALIASES)
                    .insert("huge", "z".repeat(60_000).as_str())
                    .unwrap();
            }),
        ));
        cases.push(hostile(
            "oversized: alias key of 60000 bytes",
            "oversized",
            base,
            raw_edit(base, 96_002, |raw| {
                raw.get_map(ALIASES)
                    .insert("k".repeat(60_000).as_str(), "__deleted__")
                    .unwrap();
            }),
        ));
        cases.push(hostile(
            "oversized: block attribute value of 60000 bytes",
            "oversized",
            base,
            raw_edit(base, 96_003, |raw| {
                let Some(ValueOrContainer::Container(Container::Map(attrs))) = block_map(raw, 0).get("attributes")
                else {
                    panic!("no attributes")
                };
                attrs.insert("note", "n".repeat(60_000).as_str()).unwrap();
            }),
        ));
        cases.push(hostile(
            "oversized: nodeName of 60000 bytes",
            "oversized",
            base,
            raw_edit(base, 96_004, |raw| {
                block_map(raw, 0)
                    .insert("nodeName", "k".repeat(60_000).as_str())
                    .unwrap();
            }),
        ));
        cases.push(hostile(
            "oversized: one block text of 60000 characters",
            "oversized",
            base,
            raw_edit(base, 96_005, |raw| {
                inline_list(raw, 0)
                    .insert_container(0, LoroText::new())
                    .unwrap()
                    .insert(0, "t".repeat(60_000).as_str())
                    .unwrap();
            }),
        ));
        cases.push(hostile(
            "oversized: title of 60000 bytes",
            "oversized",
            base,
            raw_edit(base, 96_006, |raw| {
                raw.get_map("meta")
                    .insert("title", "T".repeat(60_000).as_str())
                    .unwrap();
            }),
        ));
        cases.push(hostile(
            "oversized: a 64-bit integer and a NaN attribute",
            "oversized",
            base,
            raw_edit(base, 96_007, |raw| {
                let Some(ValueOrContainer::Container(Container::Map(attrs))) = block_map(raw, 0).get("attributes")
                else {
                    panic!("no attributes")
                };
                attrs.insert("big", i64::MAX).unwrap();
                attrs.insert("nan", f64::NAN).unwrap();
                attrs.insert("indent", i64::MIN).unwrap();
            }),
        ));
        // Depth: attribute maps nested 200 deep; a legacy chain of 3000; fan-out of 1500 blocks,
        // 3000 inline texts in one block and 1000 siblings in the legacy tree.
        cases.push(hostile(
            "extreme: block attributes nested 200 maps deep",
            "extreme",
            base,
            raw_edit(base, 96_010, |raw| {
                let Some(ValueOrContainer::Container(Container::Map(attrs))) = block_map(raw, 0).get("attributes")
                else {
                    panic!("no attributes")
                };
                let mut current = attrs;
                for _ in 0..200 {
                    current = current.insert_container("n", LoroMap::new()).unwrap();
                }
            }),
        ));
        cases.push(hostile(
            "extreme: 3000 inline text containers in one block",
            "extreme",
            base,
            raw_edit(base, 96_011, |raw| {
                let inline = inline_list(raw, 0);
                for index in 0..3_000 {
                    inline.insert_container(index, LoroText::new()).unwrap();
                }
            }),
        ));
        cases.push(hostile(
            "extreme: 700 new blocks in one update",
            "extreme",
            base,
            raw_edit(base, 96_012, |raw| {
                for index in 0..700 {
                    raw_block(raw, Some(&format!("many{index}")), "", Place::Unplaced);
                }
            }),
        ));
        let legacy = &b.legacy.snapshot;
        cases.push(hostile(
            "extreme: legacy chain 1000 deep",
            "extreme",
            legacy,
            author_update(&b.legacy.author, 96_013, |e| {
                let mut parent: Option<String> = None;
                for index in 0..1_000 {
                    let id = format!("chain{index}");
                    create(e, &id, parent.as_deref(), 0, NodeKind::NavigatorNode);
                    parent = Some(id);
                }
            }),
        ));
        cases.push(hostile(
            "extreme: 1000 siblings in the legacy tree",
            "extreme",
            legacy,
            author_update(&b.legacy.author, 96_014, |e| {
                for index in 0..1_000 {
                    create(e, &format!("sib{index}"), None, 0, NodeKind::NavigatorNode);
                }
            }),
        ));
        cases.push(hostile(
            "extreme: a text of 60000 characters in a legacy block",
            "extreme",
            legacy,
            author_update(&b.legacy.author, 96_015, |e| {
                create(e, "bigtext", None, 0, NodeKind::Block);
                insert_text(e, "bigtext", 0, &"w".repeat(60_000));
            }),
        ));
        // Structured fan-out: 1000 siblings, and a chain 7 deep with a wide level.
        let s4 = &b.s4.snapshot;
        cases.push(hostile(
            "extreme: model4 600 sibling blocks under the root",
            "extreme",
            s4,
            raw_edit(s4, 96_016, |raw| {
                for _ in 0..600 {
                    raw_block(raw, None, "", Place::Root);
                }
            }),
        ));
    }

    // --- duplicate ids, orphaned placements, malformed tree ---
    {
        let s4 = &b.s4.snapshot;
        let alias_of = |raw: &LoroDoc, name: &str| match raw.get_map(ALIASES).get(name) {
            Some(ValueOrContainer::Value(LoroValue::String(cid))) => cid.to_string(),
            _ => panic!("no alias {name}"),
        };
        cases.push(hostile(
            "duplicate_id: two aliases name one block",
            "duplicate_id",
            s4,
            raw_edit(s4, 97_001, |raw| {
                let cid = alias_of(raw, "b0");
                raw.get_map(ALIASES).insert("second-name", cid.as_str()).unwrap();
            }),
        ));
        cases.push(hostile(
            "duplicate_id: an existing alias re-pointed at another block",
            "duplicate_id",
            s4,
            raw_edit(s4, 97_002, |raw| {
                let other = alias_of(raw, "b1");
                raw.get_map(ALIASES).insert("b0", other.as_str()).unwrap();
            }),
        ));
        cases.push(hostile(
            "duplicate_id: an alias naming a container that does not exist",
            "duplicate_id",
            s4,
            raw_edit(s4, 97_003, |raw| {
                raw.get_map(ALIASES).insert("ghost", "cid:99@12345:Map").unwrap();
            }),
        ));
        cases.push(hostile(
            "duplicate_id: an alias value that is not a string",
            "duplicate_id",
            s4,
            raw_edit(s4, 97_004, |raw| {
                raw.get_map(ALIASES).insert("number", 5).unwrap();
            }),
        ));
        cases.push(hostile(
            "duplicate_id: a new block takes the alias of an existing one",
            "duplicate_id",
            &b.flat.snapshot,
            raw_edit(&b.flat.snapshot, 97_005, |raw| {
                raw_block(raw, Some("b0"), "x", Place::Unplaced);
            }),
        ));
        cases.push(hostile(
            "duplicate_placement: a second active node for one block",
            "duplicate_placement",
            s4,
            raw_edit(s4, 97_010, |raw| {
                let cid = first_cid(raw);
                let tree = raw.get_tree(TREE);
                tree.enable_fractional_index(0);
                let node = tree.create(None).unwrap();
                tree.get_meta(node).unwrap().insert("block_cid", cid.as_str()).unwrap();
            }),
        ));
        {
            // One peer deletes the last placement node (accepted history). A second peer, working
            // from before the delete, moves that node and adds another node for its block: the
            // move may revive the first, leaving two active nodes for one block.
            let reader = load_raw(&b.s4.snapshot, 97_020);
            let victim = reader.get_tree(TREE).nodes().into_iter().next_back().unwrap();
            let cid = match reader.get_tree(TREE).get_meta(victim).unwrap().get("block_cid") {
                Some(ValueOrContainer::Value(LoroValue::String(cid))) => cid.to_string(),
                _ => panic!("no block_cid"),
            };
            let deleter = raw_edit(&b.s4.snapshot, 97_022, |raw| {
                raw.get_tree(TREE).delete(victim).unwrap();
            });
            let mut holder = LoroCollabEngine::load(&b.s4.snapshot).unwrap();
            holder.import_update(&deleter).unwrap();
            let base = snapshot_arc(&holder);
            let update = raw_edit(&b.s4.snapshot, 97_021, |raw| {
                let tree = raw.get_tree(TREE);
                tree.enable_fractional_index(0);
                let node = tree.create(None).unwrap();
                tree.get_meta(node).unwrap().insert("block_cid", cid.as_str()).unwrap();
                tree.mov(victim, TreeParentId::Root).unwrap();
            });
            cases.push(hostile(
                "duplicate_placement: a concurrent move may revive a deleted node beside a new node for the same block",
                "duplicate_placement",
                &base,
                update,
            ));
        }
        cases.push(hostile(
            "orphaned_placement: a node whose block_cid names no container",
            "orphaned_placement",
            s4,
            raw_edit(s4, 97_030, |raw| {
                let tree = raw.get_tree(TREE);
                tree.enable_fractional_index(0);
                let node = tree.create(None).unwrap();
                tree.get_meta(node)
                    .unwrap()
                    .insert("block_cid", "cid:77@4242:Map")
                    .unwrap();
            }),
        ));
        cases.push(hostile(
            "orphaned_placement: a node whose block_cid names a text container",
            "orphaned_placement",
            s4,
            raw_edit(s4, 97_031, |raw| {
                let tree = raw.get_tree(TREE);
                tree.enable_fractional_index(0);
                let text = raw.get_text("scratch");
                text.insert(0, "x").unwrap();
                let node = tree.create(None).unwrap();
                tree.get_meta(node)
                    .unwrap()
                    .insert("block_cid", text.id().to_string().as_str())
                    .unwrap();
            }),
        ));
        cases.push(hostile(
            "orphaned_placement: a node without a block_cid",
            "orphaned_placement",
            s4,
            raw_edit(s4, 97_032, |raw| {
                let tree = raw.get_tree(TREE);
                tree.enable_fractional_index(0);
                tree.create(None).unwrap();
            }),
        ));
        cases.push(hostile(
            "orphaned_placement: a physical block with no placement node",
            "orphaned_placement",
            s4,
            raw_edit(s4, 97_033, |raw| {
                raw_block(raw, Some("unplaced"), "x", Place::Unplaced);
            }),
        ));
        cases.push(hostile(
            "orphaned_placement: the only placement node deleted",
            "orphaned_placement",
            &plain_page(Model::Structured4, 1, 140).snapshot,
            {
                let one = plain_page(Model::Structured4, 1, 140);
                raw_edit(&one.snapshot, 97_034, |raw| {
                    let tree = raw.get_tree(TREE);
                    for node in tree.nodes() {
                        let _ = tree.delete(node);
                    }
                })
            },
        ));
        cases.push(hostile(
            "orphaned_placement: a placement node deleted, its block kept",
            "orphaned_placement",
            s4,
            raw_edit(s4, 97_035, |raw| {
                let tree = raw.get_tree(TREE);
                let node = tree.nodes().into_iter().next().unwrap();
                tree.delete(node).unwrap();
            }),
        ));
        cases.push(hostile(
            "orphaned_placement: a block deleted from the list, its node kept",
            "orphaned_placement",
            s4,
            raw_edit(s4, 97_036, |raw| {
                children(raw).delete(0, 1).unwrap();
            }),
        ));
        // Depth 8 through a move of a deep subtree.
        {
            let mut author = plain_page(Model::Structured4, 1, 141).author;
            let mut parent = "b0".to_string();
            for depth in 1..=6 {
                let id = format!("chain{depth}");
                assert!(create(&mut author, &id, Some(&parent), 0, NodeKind::Block));
                parent = id;
            }
            assert!(create(&mut author, "side", None, 0, NodeKind::Block));
            assert!(create(&mut author, "side_child", Some("side"), 0, NodeKind::Block));
            let base = snapshot_arc(&author);
            let update = raw_edit(&base, 97_040, |raw| {
                let cid_of = |name: &str| match raw.get_map(ALIASES).get(name) {
                    Some(ValueOrContainer::Value(LoroValue::String(cid))) => cid.to_string(),
                    _ => panic!("no alias {name}"),
                };
                let tree = raw.get_tree(TREE);
                let node = tree_node_of(&tree, &cid_of("side"));
                let target = tree_node_of(&tree, &cid_of("chain6"));
                tree.mov_to(node, TreeParentId::Node(target), 0).unwrap();
            });
            cases.push(hostile(
                "depth: model4 move of a two-level subtree under a depth-6 node (depth 8)",
                "placement_depth",
                &base,
                update,
            ));
        }
    }

    // --- cycles through concurrent moves ---
    {
        for (name, built) in [("model4", &b.s4), ("legacy", &b.legacy)] {
            let ids = &built.ids;
            let (a, c) = (ids[0].clone(), ids[1].clone());
            let mv = |id: &str, parent: &str| {
                let (id, parent) = (id.to_owned(), parent.to_owned());
                move |e: &mut LoroCollabEngine| {
                    let _ = e.apply_content_operation(&Operation::MoveNode {
                        id: NodeId::from(id.as_str()),
                        new_parent: Some(NodeId::from(parent.as_str())),
                        index: 0,
                    });
                }
            };
            let first = author_update(&built.author, 98_001, mv(&a, &c));
            let second = author_update(&built.author, 98_002, mv(&c, &a));
            let base = base_with(&built.author, &first);
            cases.push(hostile(
                &format!("cycle: {name} concurrent moves of {a} under {c} and {c} under {a}"),
                "cycle",
                &base,
                second,
            ));
        }
        // A cycle through a forged delete-and-move of a node under its own descendant.
        let mut author = plain_page(Model::Structured4, 1, 142).author;
        assert!(create(&mut author, "top", None, 0, NodeKind::Block));
        assert!(create(&mut author, "inner", Some("top"), 0, NodeKind::Block));
        let base = snapshot_arc(&author);
        cases.push(hostile(
            "cycle: model4 raw move of a node under its own child",
            "cycle",
            &base,
            raw_edit(&base, 98_010, |raw| {
                let cid_of = |name: &str| match raw.get_map(ALIASES).get(name) {
                    Some(ValueOrContainer::Value(LoroValue::String(cid))) => cid.to_string(),
                    _ => panic!("no alias {name}"),
                };
                let tree = raw.get_tree(TREE);
                let top = tree_node_of(&tree, &cid_of("top"));
                let inner = tree_node_of(&tree, &cid_of("inner"));
                let _ = tree.mov(top, TreeParentId::Node(inner));
            }),
        ));
    }

    // --- operations on deleted containers ---
    {
        let mut author = plain_page(Model::Flat, 4, 143).author;
        let before = author.frontier();
        let early = {
            let mut fork = author.fork().unwrap();
            fork.doc.set_peer_id(99_001).unwrap();
            let f = fork.frontier();
            insert_text(&mut fork, "b1", 0, "edit of a block deleted later");
            fork.export_from(&f).unwrap()
        };
        let _ = before;
        let _ = author.apply_content_operation(&Operation::DeleteNode { id: NodeId::from("b1") });
        let base = snapshot_arc(&author);
        cases.push(hostile(
            "deleted_container: an edit of a block map the base already deleted",
            "deleted_container",
            &base,
            early,
        ));
        // Ops under the child containers of a deleted block, authored raw.
        let built = plain_page(Model::Flat, 4, 144);
        let raw = load_raw(&built.snapshot, 99_002);
        let victim = block_map(&raw, 1);
        let victim_inline = inline_list(&raw, 1);
        let from = raw.oplog_vv();
        victim_inline
            .insert_container(0, LoroText::new())
            .unwrap()
            .insert(0, "x")
            .unwrap();
        raw.commit();
        let early = raw.export(ExportMode::updates(&from)).unwrap();
        let _ = victim;
        let base = {
            let other = load_raw(&built.snapshot, 99_003);
            children(&other).delete(1, 1).unwrap();
            other.commit();
            Arc::new(other.export(ExportMode::Snapshot).unwrap())
        };
        cases.push(hostile(
            "deleted_container: an inline text inserted under a block deleted in the base",
            "deleted_container",
            &base,
            early,
        ));
    }

    // --- malformed bytes ---
    {
        let base = &b.s4.snapshot;
        let good = author_update(&b.s4.author, 99_100, |e| {
            create(e, "mal", None, 0, NodeKind::Block);
            insert_text(e, "mal", 0, "some typed text to give the update some bytes");
        });
        assert!(good.len() > 40);
        cases.push(hostile("bytes: empty", "malformed_bytes", base, Vec::new()));
        cases.push(hostile("bytes: one zero byte", "malformed_bytes", base, vec![0]));
        cases.push(hostile("bytes: 128 pseudo-random bytes", "malformed_bytes", base, {
            let mut rng = Rng::new(424_242);
            (0..128).map(|_| rng.next().to_le_bytes()[0]).collect()
        }));
        cases.push(hostile(
            "bytes: the Loro magic and then garbage",
            "malformed_bytes",
            base,
            [b"loro".as_slice(), &[0xFF; 64]].concat(),
        ));
        cases.push(hostile(
            "bytes: header only (first 8 bytes of a valid update)",
            "malformed_bytes",
            base,
            good[..8].to_vec(),
        ));
        cases.push(hostile(
            "bytes: truncated to half",
            "malformed_bytes",
            base,
            good[..good.len() / 2].to_vec(),
        ));
        cases.push(hostile(
            "bytes: truncated by the last byte",
            "malformed_bytes",
            base,
            good[..good.len() - 1].to_vec(),
        ));
        cases.push(hostile(
            "bytes: valid update with trailing garbage",
            "malformed_bytes",
            base,
            [good.as_slice(), &[0xAB; 16]].concat(),
        ));
        for at in [4, 12, good.len() / 2, good.len() - 5] {
            let mut flipped = good.clone();
            flipped[at] ^= 0x55;
            cases.push(hostile(
                &format!("bytes: valid update with byte {at} flipped"),
                "malformed_bytes",
                base,
                flipped,
            ));
        }
        cases.push(hostile(
            "bytes: a full snapshot sent as an update",
            "malformed_bytes",
            base,
            b.flat.snapshot.as_ref().clone(),
        ));
        cases.push(hostile(
            "bytes: 65537 zero bytes",
            "malformed_bytes",
            base,
            vec![0; crate::limits::UPDATE_BYTES_MAX + 1],
        ));
    }

    // --- an update addressed to another document ---
    {
        let stranger = plain_page(Model::Flat, 6, 500);
        let from_stranger = author_update(&stranger.author, 99_200, |e| {
            create(e, "alien", None, 0, NodeKind::Block);
        });
        cases.push(hostile(
            "other_document: an update authored on an unrelated page, base model1",
            "other_document",
            &b.flat.snapshot,
            from_stranger,
        ));
        // Same seed as the base: the same peer id and counters, a different history.
        let twin = plain_page(Model::Flat, 7, 102);
        let from_twin = author_update(&twin.author, 99_201, |e| {
            create(e, "twin", None, 0, NodeKind::Block);
        });
        cases.push(hostile(
            "other_document: an update from a page with the same peer ids and counters",
            "other_document",
            &b.flat.snapshot,
            from_twin,
        ));
        let whole = twin.author.export_from(&Frontier::default()).unwrap();
        cases.push(hostile(
            "other_document: the whole history of a different page",
            "other_document",
            &b.flat.snapshot,
            whole,
        ));
        let legacy_other = plain_page(Model::LegacyTree, 6, 501);
        cases.push(hostile(
            "other_document: a legacy page's whole history sent to a model4 page",
            "other_document",
            &b.s4.snapshot,
            legacy_other.author.export_from(&Frontier::default()).unwrap(),
        ));
    }

    // --- causally incomplete ---
    {
        let mut fork = b.s4.author.fork().unwrap();
        fork.doc.set_peer_id(99_300).unwrap();
        create(&mut fork, "first", None, 0, NodeKind::Block);
        let middle = fork.frontier();
        create(&mut fork, "second", None, 0, NodeKind::Block);
        cases.push(hostile(
            "incomplete: the second of two sequential updates, delivered alone",
            "causal",
            &b.s4.snapshot,
            fork.export_from(&middle).unwrap(),
        ));
    }
    cases.extend(super::window_class_fixtures::class_hostile());
    cases
}

/// The hostile corpus with the recorded decision attached to each case. A shape that has no entry
/// in [`RECORDED`] is a case someone added without recording today's decision.
pub(super) fn hostile_cases() -> Vec<Case> {
    let mut cases = hostile_shapes();
    for case in &mut cases {
        case.expect = RECORDED
            .iter()
            .find(|(name, _)| *name == case.label)
            .map(|(_, expect)| *expect);
    }
    cases
}

/// What today's code decides for each hostile shape, recorded from the full computation.
pub(super) const RECORDED: &[(&str, Expect)] = &[
    (
        "forged_delete: honest delete of the atom",
        refuse("editor_model_floor_downgrade"),
    ),
    (
        "forged_delete: delete a plain block, start_id names the atom",
        Expect::Accept,
    ),
    (
        "forged_delete: delete the atom, start_id names a plain block",
        refuse("editor_model_floor_downgrade"),
    ),
    (
        "forged_delete: delete the atom, len forged to -1",
        refuse("editor_model_floor_downgrade"),
    ),
    (
        "shadow_root: legacy doc as Text",
        refuse("reserved_root_container_type"),
    ),
    (
        "shadow_root: legacy doc as List",
        refuse("reserved_root_container_type"),
    ),
    (
        "shadow_root: legacy doc as MovableList",
        refuse("reserved_root_container_type"),
    ),
    (
        "shadow_root: legacy doc as Tree",
        refuse("reserved_root_container_type"),
    ),
    (
        "shadow_root: legacy flow_editor_structure as Text",
        refuse("reserved_root_container_type"),
    ),
    (
        "shadow_root: legacy flow_editor_structure as List",
        refuse("reserved_root_container_type"),
    ),
    (
        "shadow_root: legacy flow_editor_structure as MovableList",
        refuse("reserved_root_container_type"),
    ),
    (
        "shadow_root: legacy flow_editor_structure as Map",
        refuse("reserved_root_container_type"),
    ),
    (
        "shadow_root: legacy flow_editor_structure_meta as Text",
        refuse("reserved_root_container_type"),
    ),
    (
        "shadow_root: legacy flow_editor_structure_meta as List",
        refuse("reserved_root_container_type"),
    ),
    (
        "shadow_root: legacy flow_editor_structure_meta as MovableList",
        refuse("reserved_root_container_type"),
    ),
    (
        "shadow_root: legacy flow_editor_structure_meta as Tree",
        refuse("reserved_root_container_type"),
    ),
    (
        "shadow_root: legacy flow_editor_aliases as Text",
        refuse("reserved_root_container_type"),
    ),
    (
        "shadow_root: legacy flow_editor_aliases as List",
        refuse("reserved_root_container_type"),
    ),
    (
        "shadow_root: legacy flow_editor_aliases as MovableList",
        refuse("reserved_root_container_type"),
    ),
    (
        "shadow_root: legacy flow_editor_aliases as Tree",
        refuse("reserved_root_container_type"),
    ),
    (
        "shadow_root: legacy order as Text",
        refuse("reserved_root_container_type"),
    ),
    (
        "shadow_root: legacy order as List",
        refuse("reserved_root_container_type"),
    ),
    (
        "shadow_root: legacy order as MovableList",
        refuse("reserved_root_container_type"),
    ),
    (
        "shadow_root: legacy order as Tree",
        refuse("reserved_root_container_type"),
    ),
    (
        "shadow_root: legacy tree as Text",
        refuse("reserved_root_container_type"),
    ),
    (
        "shadow_root: legacy tree as List",
        refuse("reserved_root_container_type"),
    ),
    (
        "shadow_root: legacy tree as MovableList",
        refuse("reserved_root_container_type"),
    ),
    (
        "shadow_root: legacy tree as Map",
        refuse("reserved_root_container_type"),
    ),
    (
        "shadow_root: legacy meta as Text",
        refuse("reserved_root_container_type"),
    ),
    (
        "shadow_root: legacy meta as List",
        refuse("reserved_root_container_type"),
    ),
    (
        "shadow_root: legacy meta as MovableList",
        refuse("reserved_root_container_type"),
    ),
    (
        "shadow_root: legacy meta as Tree",
        refuse("reserved_root_container_type"),
    ),
    (
        "shadow_root: model1 doc as Text",
        refuse("reserved_root_container_type"),
    ),
    (
        "shadow_root: model1 doc as List",
        refuse("reserved_root_container_type"),
    ),
    (
        "shadow_root: model1 doc as MovableList",
        refuse("reserved_root_container_type"),
    ),
    (
        "shadow_root: model1 doc as Tree",
        refuse("reserved_root_container_type"),
    ),
    (
        "shadow_root: model1 flow_editor_structure as Text",
        refuse("reserved_root_container_type"),
    ),
    (
        "shadow_root: model1 flow_editor_structure as List",
        refuse("reserved_root_container_type"),
    ),
    (
        "shadow_root: model1 flow_editor_structure as MovableList",
        refuse("reserved_root_container_type"),
    ),
    (
        "shadow_root: model1 flow_editor_structure as Map",
        refuse("reserved_root_container_type"),
    ),
    (
        "shadow_root: model1 flow_editor_structure_meta as Text",
        refuse("reserved_root_container_type"),
    ),
    (
        "shadow_root: model1 flow_editor_structure_meta as List",
        refuse("reserved_root_container_type"),
    ),
    (
        "shadow_root: model1 flow_editor_structure_meta as MovableList",
        refuse("reserved_root_container_type"),
    ),
    (
        "shadow_root: model1 flow_editor_structure_meta as Tree",
        refuse("reserved_root_container_type"),
    ),
    (
        "shadow_root: model1 flow_editor_aliases as Text",
        refuse("reserved_root_container_type"),
    ),
    (
        "shadow_root: model1 flow_editor_aliases as List",
        refuse("reserved_root_container_type"),
    ),
    (
        "shadow_root: model1 flow_editor_aliases as MovableList",
        refuse("reserved_root_container_type"),
    ),
    (
        "shadow_root: model1 flow_editor_aliases as Tree",
        refuse("reserved_root_container_type"),
    ),
    (
        "shadow_root: model1 order as Text",
        refuse("reserved_root_container_type"),
    ),
    (
        "shadow_root: model1 order as List",
        refuse("reserved_root_container_type"),
    ),
    (
        "shadow_root: model1 order as MovableList",
        refuse("reserved_root_container_type"),
    ),
    (
        "shadow_root: model1 order as Tree",
        refuse("reserved_root_container_type"),
    ),
    (
        "shadow_root: model1 tree as Text",
        refuse("reserved_root_container_type"),
    ),
    (
        "shadow_root: model1 tree as List",
        refuse("reserved_root_container_type"),
    ),
    (
        "shadow_root: model1 tree as MovableList",
        refuse("reserved_root_container_type"),
    ),
    (
        "shadow_root: model1 tree as Map",
        refuse("reserved_root_container_type"),
    ),
    (
        "shadow_root: model1 meta as Text",
        refuse("reserved_root_container_type"),
    ),
    (
        "shadow_root: model1 meta as List",
        refuse("reserved_root_container_type"),
    ),
    (
        "shadow_root: model1 meta as MovableList",
        refuse("reserved_root_container_type"),
    ),
    (
        "shadow_root: model1 meta as Tree",
        refuse("reserved_root_container_type"),
    ),
    (
        "shadow_root: model4 doc as Text",
        refuse("reserved_root_container_type"),
    ),
    (
        "shadow_root: model4 doc as List",
        refuse("reserved_root_container_type"),
    ),
    (
        "shadow_root: model4 doc as MovableList",
        refuse("reserved_root_container_type"),
    ),
    (
        "shadow_root: model4 doc as Tree",
        refuse("reserved_root_container_type"),
    ),
    (
        "shadow_root: model4 flow_editor_structure as Text",
        refuse("reserved_root_container_type"),
    ),
    (
        "shadow_root: model4 flow_editor_structure as List",
        refuse("reserved_root_container_type"),
    ),
    (
        "shadow_root: model4 flow_editor_structure as MovableList",
        refuse("reserved_root_container_type"),
    ),
    (
        "shadow_root: model4 flow_editor_structure as Map",
        refuse("reserved_root_container_type"),
    ),
    (
        "shadow_root: model4 flow_editor_structure_meta as Text",
        refuse("reserved_root_container_type"),
    ),
    (
        "shadow_root: model4 flow_editor_structure_meta as List",
        refuse("reserved_root_container_type"),
    ),
    (
        "shadow_root: model4 flow_editor_structure_meta as MovableList",
        refuse("reserved_root_container_type"),
    ),
    (
        "shadow_root: model4 flow_editor_structure_meta as Tree",
        refuse("reserved_root_container_type"),
    ),
    (
        "shadow_root: model4 flow_editor_aliases as Text",
        refuse("reserved_root_container_type"),
    ),
    (
        "shadow_root: model4 flow_editor_aliases as List",
        refuse("reserved_root_container_type"),
    ),
    (
        "shadow_root: model4 flow_editor_aliases as MovableList",
        refuse("reserved_root_container_type"),
    ),
    (
        "shadow_root: model4 flow_editor_aliases as Tree",
        refuse("reserved_root_container_type"),
    ),
    (
        "shadow_root: model4 order as Text",
        refuse("reserved_root_container_type"),
    ),
    (
        "shadow_root: model4 order as List",
        refuse("reserved_root_container_type"),
    ),
    (
        "shadow_root: model4 order as MovableList",
        refuse("reserved_root_container_type"),
    ),
    (
        "shadow_root: model4 order as Tree",
        refuse("reserved_root_container_type"),
    ),
    (
        "shadow_root: model4 tree as Text",
        refuse("reserved_root_container_type"),
    ),
    (
        "shadow_root: model4 tree as List",
        refuse("reserved_root_container_type"),
    ),
    (
        "shadow_root: model4 tree as MovableList",
        refuse("reserved_root_container_type"),
    ),
    (
        "shadow_root: model4 tree as Map",
        refuse("reserved_root_container_type"),
    ),
    (
        "shadow_root: model4 meta as Text",
        refuse("reserved_root_container_type"),
    ),
    (
        "shadow_root: model4 meta as List",
        refuse("reserved_root_container_type"),
    ),
    (
        "shadow_root: model4 meta as MovableList",
        refuse("reserved_root_container_type"),
    ),
    (
        "shadow_root: model4 meta as Tree",
        refuse("reserved_root_container_type"),
    ),
    (
        "shadow_root: model5 doc as Text",
        refuse("reserved_root_container_type"),
    ),
    (
        "shadow_root: model5 doc as List",
        refuse("reserved_root_container_type"),
    ),
    (
        "shadow_root: model5 doc as MovableList",
        refuse("reserved_root_container_type"),
    ),
    (
        "shadow_root: model5 doc as Tree",
        refuse("reserved_root_container_type"),
    ),
    (
        "shadow_root: model5 flow_editor_structure as Text",
        refuse("reserved_root_container_type"),
    ),
    (
        "shadow_root: model5 flow_editor_structure as List",
        refuse("reserved_root_container_type"),
    ),
    (
        "shadow_root: model5 flow_editor_structure as MovableList",
        refuse("reserved_root_container_type"),
    ),
    (
        "shadow_root: model5 flow_editor_structure as Map",
        refuse("reserved_root_container_type"),
    ),
    (
        "shadow_root: model5 flow_editor_structure_meta as Text",
        refuse("reserved_root_container_type"),
    ),
    (
        "shadow_root: model5 flow_editor_structure_meta as List",
        refuse("reserved_root_container_type"),
    ),
    (
        "shadow_root: model5 flow_editor_structure_meta as MovableList",
        refuse("reserved_root_container_type"),
    ),
    (
        "shadow_root: model5 flow_editor_structure_meta as Tree",
        refuse("reserved_root_container_type"),
    ),
    (
        "shadow_root: model5 flow_editor_aliases as Text",
        refuse("reserved_root_container_type"),
    ),
    (
        "shadow_root: model5 flow_editor_aliases as List",
        refuse("reserved_root_container_type"),
    ),
    (
        "shadow_root: model5 flow_editor_aliases as MovableList",
        refuse("reserved_root_container_type"),
    ),
    (
        "shadow_root: model5 flow_editor_aliases as Tree",
        refuse("reserved_root_container_type"),
    ),
    (
        "shadow_root: model5 order as Text",
        refuse("reserved_root_container_type"),
    ),
    (
        "shadow_root: model5 order as List",
        refuse("reserved_root_container_type"),
    ),
    (
        "shadow_root: model5 order as MovableList",
        refuse("reserved_root_container_type"),
    ),
    (
        "shadow_root: model5 order as Tree",
        refuse("reserved_root_container_type"),
    ),
    (
        "shadow_root: model5 tree as Text",
        refuse("reserved_root_container_type"),
    ),
    (
        "shadow_root: model5 tree as List",
        refuse("reserved_root_container_type"),
    ),
    (
        "shadow_root: model5 tree as MovableList",
        refuse("reserved_root_container_type"),
    ),
    (
        "shadow_root: model5 tree as Map",
        refuse("reserved_root_container_type"),
    ),
    (
        "shadow_root: model5 meta as Text",
        refuse("reserved_root_container_type"),
    ),
    (
        "shadow_root: model5 meta as List",
        refuse("reserved_root_container_type"),
    ),
    (
        "shadow_root: model5 meta as MovableList",
        refuse("reserved_root_container_type"),
    ),
    (
        "shadow_root: model5 meta as Tree",
        refuse("reserved_root_container_type"),
    ),
    ("reserved_name_near_miss: model4 \"Doc\" as List", Expect::Accept),
    ("reserved_name_near_miss: model4 \"doc \" as Text", Expect::Accept),
    ("reserved_name_near_miss: model4 \"orderx\" as Tree", Expect::Accept),
    (
        "shadow_root: model1 doc as list with a child map",
        refuse("reserved_root_container_type"),
    ),
    (
        "shadow_root: model1 order map and order text in one update",
        refuse("reserved_root_container_type"),
    ),
    (
        "shadow_root: model4 meta map write then meta list write",
        refuse("reserved_root_container_type"),
    ),
    (
        "pre_existing_shadow: base has aliases as list, ordinary text edit",
        Expect::Accept,
    ),
    (
        "pre_existing_shadow: base has aliases as list, a second shadow",
        refuse("reserved_root_container_type"),
    ),
    ("floor_downgrade: model5 marker flipped to v4 only", Expect::Accept),
    (
        "floor_downgrade: model5 marker flipped and every atom deleted",
        refuse("editor_model_floor_downgrade"),
    ),
    (
        "floor_downgrade: model5 atom deleted, marker kept",
        refuse("editor_structure_active_block_not_in_storage"),
    ),
    (
        "floor_downgrade: model5 atom retyped to a paragraph and marker flipped",
        refuse("editor_model_floor_downgrade"),
    ),
    (
        "floor_downgrade: model5 marker removed by replacing the meta map entry",
        refuse("editor_structure_model_unsupported"),
    ),
    (
        "floor_downgrade: flat page held by a raw atom, atom deleted",
        refuse("editor_model_floor_downgrade"),
    ),
    (
        "floor_downgrade: flat page held by a raw atom, doc.children replaced",
        refuse("editor_model_floor_downgrade"),
    ),
    ("root_by_read: legacy page, only the order map", Expect::Accept),
    (
        "root_by_read: legacy page, an alias map write without an editor document",
        Expect::Accept,
    ),
    (
        "root_by_read: legacy page, structure meta written without a tree",
        Expect::Accept,
    ),
    (
        "root_by_read: model1 page, structure meta initialised without a tree",
        refuse("editor_structure_missing_tree"),
    ),
    ("root_by_read: model1 page, unknown root of every type", Expect::Accept),
    ("meta: model4 initialized flipped to false", Expect::Accept),
    (
        "meta: model4 version set to 2",
        refuse("editor_structure_model_unsupported"),
    ),
    (
        "meta: model4 model set to an unknown name",
        refuse("editor_structure_model_unsupported"),
    ),
    (
        "meta: model4 initialized flipped to true is already true; model set to v5",
        Expect::Accept,
    ),
    (
        "doc: model4 doc.children replaced by an empty list",
        refuse("editor_structure_active_block_not_in_storage"),
    ),
    ("doc: model1 doc.children replaced by one block", Expect::Accept),
    ("doc: model1 doc.children replaced by a scalar", Expect::Accept),
    ("doc: model1 doc nodeName set to another kind", Expect::Accept),
    ("doc: model1 doc attributes replaced by a scalar", Expect::Accept),
    (
        "block: a scalar element in doc.children",
        refuse("editor_doc_unsupported_block_container"),
    ),
    (
        "block: a list element in doc.children",
        refuse("editor_doc_unsupported_block_container"),
    ),
    ("block: nodeName deleted", refuse("editor_block_missing_node_name")),
    (
        "block: nodeName retyped to a number",
        refuse("editor_block_missing_node_name"),
    ),
    ("block: nodeName set to an unsupported kind", Expect::Accept),
    (
        "block: inline child is a map",
        refuse("editor_block_unsupported_inline_container"),
    ),
    (
        "block: inline child is a scalar",
        refuse("editor_block_unsupported_inline_container"),
    ),
    ("block: children is not a list", refuse("editor_block_missing_children")),
    ("block: attributes replaced by a scalar", Expect::Accept),
    (
        "block: image atom without an asset id",
        refuse("editor_atom_unknown_attribute"),
    ),
    (
        "block: valid atom carrying an inline text child",
        refuse("editor_atom_children_not_empty"),
    ),
    (
        "block: atom with an unknown attribute",
        refuse("editor_atom_unknown_attribute"),
    ),
    ("block: valid raw atom appended to a flat page", Expect::Accept),
    ("mark: javascript href", refuse("editor_link_invalid_href")),
    ("mark: link with a safe href", Expect::Accept),
    ("mark: data URI href", refuse("editor_link_invalid_href")),
    ("mark: unknown mark key", refuse("editor_text_marks_unsupported")),
    ("oversized: alias value of 60000 bytes", Expect::Accept),
    ("oversized: alias key of 60000 bytes", Expect::Accept),
    ("oversized: block attribute value of 60000 bytes", Expect::Accept),
    ("oversized: nodeName of 60000 bytes", Expect::Accept),
    ("oversized: one block text of 60000 characters", Expect::Accept),
    ("oversized: title of 60000 bytes", Expect::Accept),
    ("oversized: a 64-bit integer and a NaN attribute", Expect::Accept),
    ("extreme: block attributes nested 200 maps deep", Expect::Accept),
    ("extreme: 3000 inline text containers in one block", Expect::Accept),
    ("extreme: 700 new blocks in one update", Expect::Accept),
    ("extreme: legacy chain 1000 deep", over("tree_depth", 999, 32)),
    ("extreme: 1000 siblings in the legacy tree", Expect::Accept),
    ("extreme: a text of 60000 characters in a legacy block", Expect::Accept),
    ("extreme: model4 600 sibling blocks under the root", Expect::Accept),
    ("duplicate_id: two aliases name one block", Expect::Accept),
    (
        "duplicate_id: an existing alias re-pointed at another block",
        Expect::Accept,
    ),
    (
        "duplicate_id: an alias naming a container that does not exist",
        Expect::Accept,
    ),
    ("duplicate_id: an alias value that is not a string", Expect::Accept),
    (
        "duplicate_id: a new block takes the alias of an existing one",
        Expect::Accept,
    ),
    (
        "duplicate_placement: a second active node for one block",
        refuse("editor_structure_duplicate_active_block_cid"),
    ),
    (
        "duplicate_placement: a concurrent move may revive a deleted node beside a new node for the same block",
        refuse("editor_structure_duplicate_active_block_cid"),
    ),
    (
        "orphaned_placement: a node whose block_cid names no container",
        refuse("editor_structure_invalid_block_cid"),
    ),
    (
        "orphaned_placement: a node whose block_cid names a text container",
        refuse("editor_structure_invalid_block_cid"),
    ),
    (
        "orphaned_placement: a node without a block_cid",
        refuse("editor_structure_missing_block_cid"),
    ),
    (
        "orphaned_placement: a physical block with no placement node",
        refuse("editor_structure_unindexed_physical_block"),
    ),
    (
        "orphaned_placement: the only placement node deleted",
        refuse("editor_doc_requires_one_block"),
    ),
    (
        "orphaned_placement: a placement node deleted, its block kept",
        Expect::Accept,
    ),
    (
        "orphaned_placement: a block deleted from the list, its node kept",
        refuse("editor_structure_active_block_not_in_storage"),
    ),
    (
        "depth: model4 move of a two-level subtree under a depth-6 node (depth 8)",
        refuse("editor_structure_depth_limit"),
    ),
    (
        "cycle: model4 concurrent moves of b0 under b1 and b1 under b0",
        Expect::Accept,
    ),
    (
        "cycle: legacy concurrent moves of n0 under n1 and n1 under n0",
        Expect::Accept,
    ),
    ("cycle: model4 raw move of a node under its own child", Expect::Accept),
    (
        "deleted_container: an edit of a block map the base already deleted",
        Expect::Accept,
    ),
    (
        "deleted_container: an inline text inserted under a block deleted in the base",
        Expect::Accept,
    ),
    ("bytes: empty", refuse("empty_input")),
    ("bytes: one zero byte", refuse("decode_failed")),
    ("bytes: 128 pseudo-random bytes", refuse("decode_failed")),
    ("bytes: the Loro magic and then garbage", refuse("decode_failed")),
    (
        "bytes: header only (first 8 bytes of a valid update)",
        refuse("decode_failed"),
    ),
    ("bytes: truncated to half", refuse("decode_failed")),
    ("bytes: truncated by the last byte", refuse("decode_failed")),
    ("bytes: valid update with trailing garbage", refuse("decode_failed")),
    ("bytes: valid update with byte 4 flipped", Expect::Accept),
    ("bytes: valid update with byte 12 flipped", Expect::Accept),
    ("bytes: valid update with byte 180 flipped", refuse("decode_failed")),
    ("bytes: valid update with byte 356 flipped", refuse("decode_failed")),
    ("bytes: a full snapshot sent as an update", Expect::Accept),
    (
        "bytes: 65537 zero bytes",
        Expect::Refuse {
            code: "input_too_large",
            limit_kind: "update_bytes",
            observed: 65537,
            limit: 65536,
        },
    ),
    (
        "other_document: an update authored on an unrelated page, base model1",
        refuse("causal_dependency_missing"),
    ),
    (
        "other_document: an update from a page with the same peer ids and counters",
        refuse("causal_dependency_missing"),
    ),
    (
        "other_document: the whole history of a different page",
        refuse("decode_failed"),
    ),
    (
        "other_document: a legacy page's whole history sent to a model4 page",
        Expect::Accept,
    ),
    (
        "incomplete: the second of two sequential updates, delivered alone",
        refuse("causal_dependency_missing"),
    ),
    (
        "tree: a late created parent deepens its hidden children to depth 33",
        over("tree_depth", 33, 32),
    ),
    (
        "tree: a late created parent leaves its hidden children at depth 32",
        Expect::Accept,
    ),
    (
        "tree: re-creating the unnamed root of a deleted subtree revives 20 navigators",
        Expect::Accept,
    ),
    (
        "list: 10 more unsupported maps keep a 14990-element list within what the incremental path takes",
        Expect::Accept,
    ),
    (
        "list: 11 more unsupported maps take a 14990-element list over what the incremental path takes",
        Expect::Accept,
    ),
    ("list: a keystroke on a 15010-element list", Expect::Accept),
    (
        "limit: every one of 1000 blocks gains one character, 1000 characters under the document limit",
        Expect::Accept,
    ),
    (
        "limit: every one of 1000 blocks gains two characters, 1000 characters over the document limit",
        over("document_text_chars", 1_001_000, 1_000_000),
    ),
    (
        "limit: every block gains a character, one block 1 past the block limit and the document 100 past its own",
        over("text_block_chars", 100_001, 100_000),
    ),
    (
        "facts: a block is deleted from a page whose facts count no blocks",
        Expect::Accept,
    ),
    (
        "facts: a created node takes an id the facts already hold",
        Expect::Accept,
    ),
];
