//! The safety net for an incremental write window.
//!
//! The write window (`decode_apply_and_check` in the isolated-apply worker) imports a client
//! update into the trusted base, projects the whole document and checks its limits. A later work
//! package replaces the projection and the check with an incremental computation. This module
//! fixes what that replacement must preserve, against today's full computation:
//!
//! * [`WindowImpl`] is the seam: it takes the trusted base (snapshot bytes) and the update, and
//!   returns the decision (accept, or the exact refusal: error, `limit_kind`, `observed`,
//!   `limit`), the post-import engine and the aggregates the implementation derived. The harness
//!   derives everything the host consumes from that engine (see [`Outputs`]).
//! * [`FullComputation`] is today's behaviour. The incremental implementation plugs in beside it
//!   and is compared case by case ([`compare`]).
//! * The cases come from `window_fixtures`: generated, boundary and hostile.
//! * Deliberately wrong implementations ([`mutants`]) prove that the harness detects the errors
//!   an incremental check is prone to.
//! * The counting test measures the work of one window on pages of 200 and 2,000 blocks. Today
//!   it grows with the page; the same test, inverted, is kept ignored for the incremental
//!   implementation.
//!
//! Run size: `WINDOW_DIFF_SCALE` (default 1) multiplies the seeds per base model and size. The
//! default finishes in well under a minute; `WINDOW_DIFF_SCALE=5` is about five times the cases.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::print_stdout
)]

use std::collections::BTreeMap;
use std::sync::OnceLock;
use std::time::Instant;

use loro::LoroValue;

use super::window_fixtures::{
    BaseSpec, Case, Expect, FactsFrom, History, MODELS, Model, author_update, boundary_cases, build_base,
    generated_cases, hostile_cases, run_parallel,
};
use super::{CollabEngine, LoroCollabEngine};
use crate::error::CollabError;
use crate::facts::BaseFacts;
use crate::frontier::Frontier;
use crate::limits::{DocumentLimits, check_snapshot};
use crate::operation::{NodeId, NodeKind, Operation};
use crate::semantic::SemanticSnapshot;
use crate::window_check::fault::Fault;
use crate::window_check::{
    FallbackReason, WindowAccepted, WindowDivergence, WindowReport, compare_with_projection, run_window,
};
use crate::{test_alloc, walk_counter};

// ---------------------------------------------------------------------------------------------
// The seam
// ---------------------------------------------------------------------------------------------

/// The decision of a window: accepted, or refused with the exact error.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum Decision {
    Accept,
    Refuse(CollabError),
}

impl Decision {
    /// A short stable code: the error variant, or the reason of an `OperationFailed`.
    fn code(&self) -> String {
        match self {
            Self::Accept => "accept".into(),
            Self::Refuse(error) => match error {
                CollabError::EmptyInput { .. } => "empty_input".into(),
                CollabError::InputTooLarge { .. } => "input_too_large".into(),
                CollabError::DecodeFailed { .. } => "decode_failed".into(),
                CollabError::UnknownNode { .. } => "unknown_node".into(),
                CollabError::DuplicateNode { .. } => "duplicate_node".into(),
                CollabError::CycleRejected { .. } => "cycle_rejected".into(),
                CollabError::OperationFailed { reason } => reason.clone(),
                CollabError::LimitExceeded { .. } => "limit_exceeded".into(),
                CollabError::CausalDependencyMissing => "causal_dependency_missing".into(),
            },
        }
    }

    /// `(limit_kind, observed, limit)` of a limit refusal; empty and zeros otherwise.
    fn numbers(&self) -> (&'static str, u64, u64) {
        match self {
            Self::Refuse(CollabError::LimitExceeded {
                limit_kind,
                limit,
                observed,
            }) => (limit_kind, *observed, *limit),
            Self::Refuse(CollabError::InputTooLarge {
                actual_bytes,
                max_bytes,
                ..
            }) => ("update_bytes", *actual_bytes as u64, *max_bytes as u64),
            _ => ("", 0, 0),
        }
    }

    /// Whether this decision is the recorded `expect`.
    fn matches(&self, expect: Expect) -> bool {
        match (self, expect) {
            (Self::Accept, Expect::Accept) => true,
            (
                Self::Refuse(_),
                Expect::Refuse {
                    code,
                    limit_kind,
                    observed,
                    limit,
                },
            ) => self.code() == code && self.numbers() == (limit_kind, observed, limit),
            _ => false,
        }
    }

    /// The `RECORDED` entry that records this decision.
    fn source(&self) -> String {
        let (limit_kind, observed, limit) = self.numbers();
        match self {
            Self::Accept => "Expect::Accept".into(),
            Self::Refuse(_) if self.code() == "limit_exceeded" => {
                format!("over({limit_kind:?}, {observed}, {limit})")
            }
            Self::Refuse(_) if limit_kind.is_empty() => format!("refuse({:?})", self.code()),
            Self::Refuse(_) => format!(
                "Expect::Refuse {{ code: {:?}, limit_kind: {limit_kind:?}, observed: {observed}, limit: {limit} }}",
                self.code()
            ),
        }
    }

    fn describe(&self) -> String {
        match self {
            Self::Accept => "accept".into(),
            Self::Refuse(error) => format!("refuse({error})"),
        }
    }
}

/// The five aggregates `check_snapshot` compares with the limits, derived independently of
/// `limits.rs` (so the property "the facts of the incremental path equal the facts of a full
/// projection" is checked against a second implementation).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct Aggregates {
    pub max_depth: usize,
    pub live_navigators: usize,
    pub live_blocks: usize,
    pub longest_live_text: usize,
    pub total_live_text: usize,
}

pub(super) fn aggregates(snapshot: &SemanticSnapshot) -> Aggregates {
    let depth_of = |start: &NodeId| {
        let mut current = start.clone();
        let mut depth = 0;
        while let Some(parent) = snapshot.nodes.get(&current).and_then(|node| node.parent.clone()) {
            depth += 1;
            current = parent;
            if depth > snapshot.nodes.len() + 1 {
                return snapshot.nodes.len();
            }
        }
        depth
    };
    let live = || snapshot.nodes.values().filter(|node| !node.deleted);
    Aggregates {
        max_depth: snapshot.nodes.keys().map(depth_of).max().unwrap_or(0),
        live_navigators: live().filter(|node| node.kind == NodeKind::NavigatorNode).count(),
        live_blocks: live().filter(|node| node.kind == NodeKind::Block).count(),
        longest_live_text: live().map(|node| node.text.chars().count()).max().unwrap_or(0),
        total_live_text: live().map(|node| node.text.chars().count()).sum(),
    }
}

/// `None` when an incremental implementation's report agrees with the aggregates of the full
/// projection: the totals are equal and the bounds are at least the exact maxima.
pub(super) fn report_against_facts(report: &WindowReport, facts: &Aggregates) -> Option<String> {
    let exact = [
        ("blocks", report.blocks, facts.live_blocks as u64),
        ("navigators", report.navigators, facts.live_navigators as u64),
        ("chars", report.chars, facts.total_live_text as u64),
    ];
    for (name, reported, projected) in exact {
        if reported != projected {
            return Some(format!(
                "report differs: {name} is {reported}, the projection has {projected}"
            ));
        }
    }
    if (facts.max_depth as u64) > report.depth_bound {
        return Some(format!(
            "report differs: depth bound {} below the projection's {}",
            report.depth_bound, facts.max_depth
        ));
    }
    if (facts.longest_live_text as u64) > report.longest_bound {
        return Some(format!(
            "report differs: longest bound {} below the projection's {}",
            report.longest_bound, facts.longest_live_text
        ));
    }
    None
}

/// What an implementation returns for one case.
pub(super) struct WindowResult {
    pub decision: Decision,
    /// The post-import engine of an accepted update: the document the host persists. Everything
    /// downstream is read from it by [`outputs`].
    pub engine: Option<LoroCollabEngine>,
    /// The aggregates the implementation derived for the accepted document, when it derives them.
    pub facts: Option<Aggregates>,
    /// What an incremental implementation established: exact totals and bounds on the maxima
    /// (see [`report_against_facts`]).
    pub report: Option<WindowReport>,
}

/// One implementation of the write window.
///
/// `run` is the whole window including the unmetered setup (loading the base). When the
/// incremental implementation needs the setup and the metered part apart (base facts computed in
/// setup, for example), it does that inside `run`; the counting test then measures the metered
/// part directly.
pub(super) trait WindowImpl: Sync {
    fn name(&self) -> &'static str;
    fn run(&self, base: &[u8], update: &[u8]) -> WindowResult;
}

fn refused(error: CollabError) -> WindowResult {
    WindowResult {
        decision: Decision::Refuse(error),
        engine: None,
        facts: None,
        report: None,
    }
}

/// The window exactly as the worker runs it after the base is loaded: import the client update,
/// project, check the limits. Returns the aggregates of the projection on acceptance.
fn decode_apply_and_check(engine: &mut LoroCollabEngine, update: &[u8]) -> Result<Aggregates, CollabError> {
    // `next`'s import: the id caches rebuilt in full (see `next_oracle`).
    engine.next_import_client_update(update)?;
    let semantic = engine.semantic_snapshot()?;
    check_snapshot(&semantic, &DocumentLimits::default())?;
    Ok(aggregates(&semantic))
}

/// Today's full computation.
pub(super) struct FullComputation;

impl WindowImpl for FullComputation {
    fn name(&self) -> &'static str {
        "full"
    }

    fn run(&self, base: &[u8], update: &[u8]) -> WindowResult {
        let mut engine = LoroCollabEngine::load(base).expect("the trusted base loads");
        match decode_apply_and_check(&mut engine, update) {
            Ok(facts) => WindowResult {
                decision: Decision::Accept,
                engine: Some(engine),
                facts: Some(facts),
                report: None,
            },
            Err(error) => refused(error),
        }
    }
}

/// Today's full computation after the worker's setup prewarm.
pub(super) struct PrewarmedFull;

impl WindowImpl for PrewarmedFull {
    fn name(&self) -> &'static str {
        "full computation after prewarm_for_writes"
    }

    fn run(&self, base: &[u8], update: &[u8]) -> WindowResult {
        let mut engine = LoroCollabEngine::load(base).expect("the trusted base loads");
        engine.prewarm_for_writes();
        match decode_apply_and_check(&mut engine, update) {
            Ok(facts) => WindowResult {
                decision: Decision::Accept,
                engine: Some(engine),
                facts: Some(facts),
                report: None,
            },
            Err(error) => refused(error),
        }
    }
}

/// How an [`Incremental`] window is made wrong on purpose.
#[derive(Clone, Copy)]
pub(super) enum Defect {
    /// The window as the worker runs it.
    None,
    /// A fault inside the check itself.
    Fault(Fault),
    /// Base facts that are stale: every unit's pre-image has one character less than the totals
    /// were built from.
    StaleBaseFacts,
}

/// The write window as the worker runs it: setup (load, prewarm, base facts), then the metered
/// part, `window_check::run_window`.
pub(super) struct Incremental {
    pub name: &'static str,
    pub defect: Defect,
}

impl Incremental {
    pub(super) const WORKER: Self = Self {
        name: "incremental",
        defect: Defect::None,
    };

    /// The window, keeping how the check decided.
    pub(super) fn window(&self, base: &[u8], update: &[u8]) -> (Result<WindowAccepted, CollabError>, LoroCollabEngine) {
        self.window_with(base, update, &FactsFrom::Base)
    }

    /// [`Self::window`] with the base facts taken from `from`.
    pub(super) fn window_with(
        &self,
        base: &[u8],
        update: &[u8],
        from: &FactsFrom,
    ) -> (Result<WindowAccepted, CollabError>, LoroCollabEngine) {
        let mut engine = LoroCollabEngine::load(base).expect("the trusted base loads");
        engine.prewarm_for_writes();
        let mut facts = match from {
            FactsFrom::Base | FactsFrom::Tampered(_) => BaseFacts::compute(&engine),
            FactsFrom::OtherBase(other) => {
                let other = LoroCollabEngine::load(other).expect("the other base loads");
                BaseFacts::compute(&other)
            }
        };
        if let (FactsFrom::Tampered(tamper), Some(facts)) = (from, facts.as_mut()) {
            tamper(facts);
        }
        if matches!(self.defect, Defect::StaleBaseFacts)
            && let Some(facts) = facts.as_mut()
        {
            for unit in facts.units.values_mut() {
                unit.chars = unit.chars.saturating_sub(1);
            }
        }
        let _injected = match self.defect {
            Defect::Fault(fault) => Some(crate::window_check::fault::inject(fault)),
            _ => None,
        };
        let outcome = run_window(&mut engine, facts.as_ref(), update);
        (outcome, engine)
    }
}

impl WindowImpl for Incremental {
    fn name(&self) -> &'static str {
        self.name
    }

    fn run(&self, base: &[u8], update: &[u8]) -> WindowResult {
        match self.window(base, update) {
            (Ok(accepted), engine) => WindowResult {
                decision: Decision::Accept,
                engine: Some(engine),
                facts: None,
                report: accepted.report,
            },
            (Err(error), _) => refused(error),
        }
    }
}

/// What the host consumes from an accepted window, read from the post-import engine: the
/// accepted document itself (state value, root list, snapshot bytes, frontier) and the host's
/// projection of it (`semantic_snapshot_with_floor`, title, navigator order, editor-model floor).
/// Large values are kept as digests so that the results of thousands of cases can be held at once;
/// equal digests are taken as equal values.
#[derive(Debug, PartialEq)]
struct Outputs {
    frontier: Frontier,
    deep_value: u64,
    roots: u64,
    snapshot: u64,
    /// The op log as exported updates: independent of which containers' states were decoded, so
    /// equal histories give equal bytes whatever the reads that came before.
    history: u64,
    /// The exported snapshot, loaded again: its value and its projection. Equal documents must
    /// reload to the same document even when their snapshot bytes differ.
    reloaded: (u64, Result<String, CollabError>),
    semantic: Result<(String, u32), CollabError>,
    title: Result<String, CollabError>,
    navigator_order: Result<BTreeMap<String, String>, CollabError>,
    floor: Result<u32, CollabError>,
}

fn digest(value: &impl std::hash::Hash) -> u64 {
    use std::hash::Hasher;
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    value.hash(&mut hasher);
    hasher.finish()
}

/// The names of [`Outputs`]' fields, for a diff message. With `snapshot_bytes` false the byte
/// comparison of the exported snapshot is skipped. A Loro snapshot is not canonical: a container whose state was decoded before the export is encoded
/// from that state, one that was not is copied as it was read, so equal documents can export
/// different bytes. Everything else, the exported value and the roots included, is still compared.
fn output_diff_with(a: &Outputs, b: &Outputs, snapshot_bytes: bool) -> Option<&'static str> {
    if a.frontier != b.frontier {
        Some("frontier")
    } else if a.deep_value != b.deep_value {
        Some("get_deep_value (a root was registered or a value differs)")
    } else if a.roots != b.roots {
        Some("get_value (the list of roots differs)")
    } else if a.history != b.history {
        Some("export(all updates) bytes (the history differs)")
    } else if a.reloaded != b.reloaded {
        Some("the exported snapshot reloads to another document")
    } else if snapshot_bytes && a.snapshot != b.snapshot {
        Some("export(Snapshot) bytes")
    } else if a.semantic != b.semantic {
        Some("semantic_snapshot_with_floor")
    } else if a.title != b.title {
        Some("title")
    } else if a.navigator_order != b.navigator_order {
        Some("navigator_order")
    } else if a.floor != b.floor {
        Some("minimum_editor_model")
    } else {
        None
    }
}

fn outputs(engine: &LoroCollabEngine) -> Outputs {
    // The document surfaces first, before any engine read: they show what the window itself did.
    let deep_value = digest(&format!("{:?}", engine.doc.get_deep_value()));
    let roots = digest(&format!("{:?}", engine.doc.get_value()));
    let snapshot = digest(&engine.export_snapshot().unwrap());
    let history = digest(&engine.doc.export(loro::ExportMode::all_updates()).unwrap());
    let reloaded = LoroCollabEngine::load(&engine.export_snapshot().unwrap()).unwrap();
    let reloaded = (
        digest(&format!("{:?}", reloaded.doc.get_deep_value())),
        reloaded.semantic_snapshot().map(|semantic| semantic.semantic_hash()),
    );
    Outputs {
        frontier: engine.frontier(),
        deep_value,
        roots,
        snapshot,
        history,
        reloaded,
        semantic: engine
            .semantic_snapshot_with_floor()
            .map(|(semantic, floor)| (semantic.semantic_hash(), floor)),
        title: engine.title(),
        navigator_order: engine.navigator_order(),
        floor: engine.minimum_editor_model(),
    }
}

/// Everything compared about one run of an implementation on one case.
pub(super) struct Observed {
    name: &'static str,
    decision: Decision,
    facts: Option<Aggregates>,
    report: Option<WindowReport>,
    outputs: Option<Outputs>,
}

impl Observed {
    /// Whether the exported snapshot bytes of this run and `other` are equal (`None` unless both
    /// accepted).
    pub(super) fn same_snapshot_bytes(&self, other: &Self) -> Option<bool> {
        Some(self.outputs.as_ref()?.snapshot == other.outputs.as_ref()?.snapshot)
    }
}

pub(super) fn observe(implementation: &dyn WindowImpl, case: &Case) -> Observed {
    let result = implementation.run(&case.base, &case.update);
    Observed {
        name: implementation.name(),
        decision: result.decision,
        facts: result.facts,
        report: result.report,
        outputs: result.engine.as_ref().map(outputs),
    }
}

/// `None` when two runs agree on the decision (the whole error, not only its code), on the facts
/// when both derive them, and on every output.
pub(super) fn difference(a: &Observed, b: &Observed) -> Option<String> {
    difference_with(a, b, true)
}

/// [`difference`] with the snapshot bytes compared only when `snapshot_bytes` (see
/// [`output_diff_with`]).
pub(super) fn difference_with(a: &Observed, b: &Observed, snapshot_bytes: bool) -> Option<String> {
    if a.decision != b.decision {
        return Some(format!(
            "decision differs: {} says {}, {} says {}",
            a.name,
            a.decision.describe(),
            b.name,
            b.decision.describe()
        ));
    }
    if let (Some(x), Some(y)) = (&a.facts, &b.facts)
        && x != y
    {
        return Some(format!("facts differ: {x:?} against {y:?}"));
    }
    for (report, facts) in [(&a.report, &b.facts), (&b.report, &a.facts)] {
        if let (Some(report), Some(facts)) = (report, facts)
            && let Some(message) = report_against_facts(report, facts)
        {
            return Some(message);
        }
    }
    match (&a.outputs, &b.outputs) {
        (Some(x), Some(y)) => output_diff_with(x, y, snapshot_bytes).map(|field| format!("output differs: {field}")),
        (None, None) => None,
        _ => Some("one implementation accepted without returning its engine".into()),
    }
}

/// Compares two implementations on one case.
pub(super) fn compare(case: &Case, left: &dyn WindowImpl, right: &dyn WindowImpl) -> Option<String> {
    difference(&observe(left, case), &observe(right, case))
}

// ---------------------------------------------------------------------------------------------
// The case set
// ---------------------------------------------------------------------------------------------

fn scale() -> u64 {
    std::env::var("WINDOW_DIFF_SCALE")
        .ok()
        .and_then(|value| value.parse::<u64>().ok())
        .filter(|scale| *scale >= 1)
        .unwrap_or(1)
}

/// A family of cases, built once per test run, with the time it took.
struct Family {
    cases: Vec<Case>,
    build_millis: u128,
}

fn family(cell: &'static OnceLock<Family>, build: impl FnOnce() -> Vec<Case>) -> &'static Family {
    cell.get_or_init(|| {
        let started = Instant::now();
        let cases = build();
        Family {
            cases,
            build_millis: started.elapsed().as_millis(),
        }
    })
}

fn generated() -> &'static Family {
    static CELL: OnceLock<Family> = OnceLock::new();
    family(&CELL, || generated_cases(scale()))
}

fn boundary() -> &'static Family {
    static CELL: OnceLock<Family> = OnceLock::new();
    family(&CELL, boundary_cases)
}

fn hostile() -> &'static Family {
    static CELL: OnceLock<Family> = OnceLock::new();
    family(&CELL, hostile_cases)
}

fn round2() -> &'static Family {
    static CELL: OnceLock<Family> = OnceLock::new();
    family(&CELL, || super::window_round2_tests::round2_cases(scale()))
}

fn all_cases() -> Vec<&'static Case> {
    [generated(), boundary(), hostile(), round2()]
        .into_iter()
        .flat_map(|family| family.cases.iter())
        .collect()
}

/// The full computation's run on every case of [`all_cases`], in the same order, computed once.
fn references() -> &'static [Observed] {
    static CELL: OnceLock<Vec<Observed>> = OnceLock::new();
    CELL.get_or_init(|| parallel(&all_cases(), &|case| observe(&FullComputation, case)))
}

/// Runs `job` over `cases` on all cores, keeping the order.
fn parallel<T: Send>(cases: &[&Case], job: &(dyn Fn(&Case) -> T + Sync)) -> Vec<T> {
    run_parallel(cases, &|_, case| job(case))
}

// ---------------------------------------------------------------------------------------------
// Tests: the full computation against itself, boundary and hostile expectations
// ---------------------------------------------------------------------------------------------

/// A run of the full computation twice on every case: the same decision and the same outputs,
/// and no wall-clock outlier.
#[test]
fn generated_cases_are_deterministic_under_the_full_computation() {
    let set = generated();
    let cases: Vec<&Case> = set.cases.iter().collect();
    let started = Instant::now();
    let results = parallel(&cases, &|case| {
        let begin = Instant::now();
        let diff = compare(case, &FullComputation, &FullComputation);
        (
            diff,
            FullComputation.run(&case.base, &case.update).decision,
            begin.elapsed(),
        )
    });
    let elapsed = started.elapsed();
    let mut by_kind: BTreeMap<&str, (usize, usize)> = BTreeMap::new();
    let mut by_model: BTreeMap<String, usize> = BTreeMap::new();
    let mut accepted = 0;
    let mut slowest = (std::time::Duration::ZERO, String::new());
    for (case, (diff, decision, took)) in cases.iter().zip(&results) {
        assert!(diff.is_none(), "{}: {}", case.label, diff.clone().unwrap());
        let entry = by_kind.entry(case.kind).or_default();
        if *decision == Decision::Accept {
            entry.0 += 1;
            accepted += 1;
        } else {
            entry.1 += 1;
        }
        *by_model
            .entry(case.label.split('/').next().unwrap().to_owned())
            .or_default() += 1;
        if *took > slowest.0 {
            slowest = (*took, case.label.clone());
        }
    }
    println!(
        "generated: {} cases (scale {}), {accepted} accepted, {} refused; built in {} ms, compared in {} ms; slowest {:?} {}",
        cases.len(),
        scale(),
        cases.len() - accepted,
        set.build_millis,
        elapsed.as_millis(),
        slowest.0,
        slowest.1
    );
    println!("generated by model: {by_model:?}");
    for (kind, (accept, refuse)) in &by_kind {
        println!("  {kind:<42} accept {accept:>5} refuse {refuse:>5}");
    }
    // Coverage the harness promises: every model and every kind of update is present, and both
    // outcomes occur.
    for model in MODELS {
        assert!(by_model.contains_key(model.name()), "no case for {}", model.name());
    }
    for kind in [
        "insert_one",
        "insert_fifty",
        "text_insert",
        "text_delete",
        "text_edit_many_blocks",
        "delete_one",
        "delete_several",
        "move_one",
        "marks",
        "insert_atom",
        "touch_every_root",
        "no_op",
        "already_known",
        "causally_incomplete",
        "concurrent_move_cycle",
        "concurrent_delete_then_edit_same_block",
        "edit_then_revert",
    ] {
        assert!(by_kind.contains_key(kind), "no case of kind {kind}");
    }
    assert!(accepted > 0 && accepted < cases.len());
}

/// Boundary cases: each limit at its ceiling and one past it, with the exact `observed` and
/// `limit`, and the independent aggregates agree with `check_snapshot` about them.
#[test]
fn boundary_cases_match_the_recorded_decisions() {
    let cases: Vec<&Case> = boundary().cases.iter().collect();
    let results = parallel(&cases, &|case| {
        (
            compare(case, &FullComputation, &FullComputation),
            FullComputation.run(&case.base, &case.update),
        )
    });
    for (case, (diff, result)) in cases.iter().zip(&results) {
        assert!(diff.is_none(), "{}: {}", case.label, diff.clone().unwrap());
        let expect = case.expect.expect("boundary cases carry an expectation");
        assert!(
            result.decision.matches(expect),
            "{}: expected {expect:?}, today's decision is {}",
            case.label,
            result.decision.describe()
        );
        // The independent aggregates never exceed a limit the full path accepted.
        if let Some(facts) = result.facts {
            let limits = DocumentLimits::default();
            assert!(facts.max_depth <= limits.tree_depth_max, "{}", case.label);
            assert!(facts.live_navigators <= limits.container_count_max, "{}", case.label);
            assert!(facts.live_blocks <= limits.document_block_count_max, "{}", case.label);
            assert!(facts.longest_live_text <= limits.text_block_chars_max, "{}", case.label);
            assert!(
                facts.total_live_text <= limits.document_text_chars_max,
                "{}",
                case.label
            );
        }
        // For a limit refusal, the observed number is the aggregate of the post-import document.
        if let Decision::Refuse(CollabError::LimitExceeded {
            limit_kind, observed, ..
        }) = &result.decision
        {
            let mut engine = LoroCollabEngine::load(&case.base).unwrap();
            engine.import_client_update(&case.update).unwrap();
            let facts = aggregates(&engine.semantic_snapshot().unwrap());
            let aggregate = match *limit_kind {
                "tree_depth" => facts.max_depth,
                "container_count" => facts.live_navigators,
                "document_block_count" => facts.live_blocks,
                "text_block_chars" => facts.longest_live_text,
                "document_text_chars" => facts.total_live_text,
                other => panic!("{}: unexpected limit kind {other}", case.label),
            };
            assert_eq!(
                aggregate as u64, *observed,
                "{}: observed against aggregate",
                case.label
            );
        }
    }
    println!("boundary: {} cases", cases.len());
    for (case, (_, result)) in cases.iter().zip(&results) {
        println!("  {:<60} {}", case.label, result.decision.describe());
    }
}

/// The hostile corpus: every shape has a recorded decision, today's code still makes it, and two
/// runs agree. A shape without a record prints the line to add to `RECORDED`.
#[test]
fn hostile_corpus_matches_the_recorded_decisions() {
    let cases: Vec<&Case> = hostile().cases.iter().collect();
    let results = parallel(&cases, &|case| {
        let begin = Instant::now();
        let diff = compare(case, &FullComputation, &FullComputation);
        (
            diff,
            FullComputation.run(&case.base, &case.update).decision,
            begin.elapsed(),
        )
    });
    let mut unrecorded = Vec::new();
    let mut wrong = Vec::new();
    let mut slowest = (std::time::Duration::ZERO, String::new());
    for (case, (diff, decision, took)) in cases.iter().zip(&results) {
        assert!(diff.is_none(), "{}: {}", case.label, diff.clone().unwrap());
        if *took > slowest.0 {
            slowest = (*took, case.label.clone());
        }
        match case.expect {
            None => unrecorded.push(format!("    ({:?}, {}),", case.label, decision.source())),
            Some(expect) if !decision.matches(expect) => {
                wrong.push(format!(
                    "{}: recorded {expect:?}, today {}",
                    case.label,
                    decision.describe()
                ));
            }
            Some(_) => {}
        }
    }
    println!("hostile: {} cases; slowest {:?} {}", cases.len(), slowest.0, slowest.1);
    assert!(
        unrecorded.is_empty(),
        "unrecorded hostile decisions:\n{}",
        unrecorded.join("\n")
    );
    assert!(wrong.is_empty(), "decisions changed:\n{}", wrong.join("\n"));
    // Every recorded name is a live shape (no stale record).
    for (name, _) in super::window_fixtures::RECORDED {
        assert!(cases.iter().any(|case| case.label == *name), "stale record: {name}");
    }
}

// ---------------------------------------------------------------------------------------------
// Mutants: deliberately wrong windows the harness must detect
// ---------------------------------------------------------------------------------------------

/// Runs a mutant: the update is imported as in the full path, and `judge` decides instead of the
/// projection and the check. `judge` gets the pristine base and the post-import engine.
fn mutant_run(
    base: &[u8],
    update: &[u8],
    judge: impl FnOnce(&LoroCollabEngine, &LoroCollabEngine) -> Result<(), CollabError>,
) -> WindowResult {
    let pristine = LoroCollabEngine::load(base).expect("the trusted base loads");
    let mut engine = LoroCollabEngine::load(base).expect("the trusted base loads");
    if let Err(error) = engine.import_client_update(update) {
        return refused(error);
    }
    match judge(&pristine, &engine) {
        Ok(()) => WindowResult {
            decision: Decision::Accept,
            engine: Some(engine),
            facts: None,
            report: None,
        },
        Err(error) => refused(error),
    }
}

fn full_check(engine: &LoroCollabEngine) -> Result<(), CollabError> {
    let semantic = engine.semantic_snapshot()?;
    check_snapshot(&semantic, &DocumentLimits::default())?;
    Ok(())
}

/// Counts only the blocks the update changed or added, as if the base's totals were not needed.
struct ChangedBlocksOnly;

impl WindowImpl for ChangedBlocksOnly {
    fn name(&self) -> &'static str {
        "mutant: counts only changed blocks, without the base"
    }

    fn run(&self, base: &[u8], update: &[u8]) -> WindowResult {
        mutant_run(base, update, |pristine, engine| {
            let post = engine.semantic_snapshot()?;
            let Ok(before) = pristine.semantic_snapshot() else {
                return full_check(engine);
            };
            let changed = SemanticSnapshot {
                nodes: post
                    .nodes
                    .into_iter()
                    .filter(|(id, node)| before.nodes.get(id) != Some(node))
                    .collect(),
            };
            check_snapshot(&changed, &DocumentLimits::default())?;
            Ok(())
        })
    }
}

/// Never subtracts a delete: a block live in the base and deleted by the update still counts.
struct IgnoresDeletes;

impl WindowImpl for IgnoresDeletes {
    fn name(&self) -> &'static str {
        "mutant: ignores deletes"
    }

    fn run(&self, base: &[u8], update: &[u8]) -> WindowResult {
        mutant_run(base, update, |pristine, engine| {
            let mut post = engine.semantic_snapshot()?;
            if let Ok(before) = pristine.semantic_snapshot() {
                for (id, node) in &mut post.nodes {
                    if node.deleted && before.nodes.get(id).is_some_and(|old| !old.deleted) {
                        node.deleted = false;
                    }
                }
            }
            check_snapshot(&post, &DocumentLimits::default())?;
            Ok(())
        })
    }
}

/// Skips the check when the update leaves the editor document (`doc`) alone: "touches no block
/// root". Legacy tree nodes, the placement tree, the alias map and the structure meta are then
/// never looked at.
struct SkipsWhenNoBlockRootTouched;

impl SkipsWhenNoBlockRootTouched {
    fn doc_value(engine: &LoroCollabEngine) -> Option<LoroValue> {
        let value = engine.doc.get_deep_value();
        value.as_map().and_then(|roots| roots.get("doc").cloned())
    }
}

impl WindowImpl for SkipsWhenNoBlockRootTouched {
    fn name(&self) -> &'static str {
        "mutant: skips the check when the update touches no block root"
    }

    fn run(&self, base: &[u8], update: &[u8]) -> WindowResult {
        mutant_run(base, update, |pristine, engine| {
            if Self::doc_value(pristine) == Self::doc_value(engine) {
                return Ok(());
            }
            full_check(engine)
        })
    }
}

/// Checks every limit except the depth.
struct NoDepthCheck;

impl WindowImpl for NoDepthCheck {
    fn name(&self) -> &'static str {
        "mutant: no tree depth check"
    }

    fn run(&self, base: &[u8], update: &[u8]) -> WindowResult {
        mutant_run(base, update, |_, engine| {
            let semantic = engine.semantic_snapshot()?;
            let limits = DocumentLimits {
                tree_depth_max: usize::MAX,
                ..DocumentLimits::default()
            };
            check_snapshot(&semantic, &limits)?;
            Ok(())
        })
    }
}

/// Decides like the full path, but reads a root by its typed getter, which registers an empty
/// root in the accepted document: a read with a side effect.
struct ReadRegistersARoot;

impl WindowImpl for ReadRegistersARoot {
    fn name(&self) -> &'static str {
        "mutant: a read registers a root"
    }

    fn run(&self, base: &[u8], update: &[u8]) -> WindowResult {
        mutant_run(base, update, |_, engine| {
            full_check(engine)?;
            let _ = engine.doc.get_map(crate::reserved_roots::ALIASES);
            Ok(())
        })
    }
}

/// Treats a decode failure as a causally incomplete update: the same refusal with a wrong code.
struct WrongRefusalCode;

impl WindowImpl for WrongRefusalCode {
    fn name(&self) -> &'static str {
        "mutant: reports a limit refusal with observed one too low"
    }

    fn run(&self, base: &[u8], update: &[u8]) -> WindowResult {
        let mut result = FullComputation.run(base, update);
        if let Decision::Refuse(CollabError::LimitExceeded {
            limit_kind,
            limit,
            observed,
        }) = result.decision
        {
            result.decision = Decision::Refuse(CollabError::LimitExceeded {
                limit_kind,
                limit,
                observed: observed - 1,
            });
        }
        result
    }
}

fn mutants() -> Vec<Box<dyn WindowImpl>> {
    vec![
        Box::new(ChangedBlocksOnly),
        Box::new(IgnoresDeletes),
        Box::new(SkipsWhenNoBlockRootTouched),
        Box::new(NoDepthCheck),
        Box::new(ReadRegistersARoot),
        Box::new(WrongRefusalCode),
    ]
}

/// Every mutant is caught by at least one generated, boundary or hostile case, and the harness
/// reports which and how many.
#[test]
fn the_harness_detects_every_mutant() {
    let cases = all_cases();
    let started = Instant::now();
    // Every boundary and hostile case, and every third generated one (the generated cases are
    // far from the limits; the mutants differ on the limits).
    let sample: Vec<(usize, &Case)> = cases
        .iter()
        .copied()
        .enumerate()
        .filter(|(index, case)| case.family != "generated" || index % 3 == 0)
        .collect();
    println!("mutant detection over {} of {} cases", sample.len(), cases.len());
    for mutant in mutants() {
        let mutant: &dyn WindowImpl = mutant.as_ref();
        let diffs = run_parallel(&sample, &|_, (index, case)| {
            difference(&references()[*index], &observe(mutant, case))
        });
        let mut by_family: BTreeMap<&str, usize> = BTreeMap::new();
        let mut first: Option<(String, String)> = None;
        let mut total = 0;
        for ((_, case), diff) in sample.iter().zip(&diffs) {
            if let Some(diff) = diff {
                total += 1;
                *by_family.entry(case.family).or_default() += 1;
                first.get_or_insert_with(|| (case.label.clone(), diff.clone()));
            }
        }
        println!(
            "  {}\n    caught by {total} cases {by_family:?}; first: {:?}",
            mutant.name(),
            first
        );
        assert!(total > 0, "{} was not detected by any case", mutant.name());
    }
    println!("mutant run time: {} ms", started.elapsed().as_millis());
}

/// The harness must not report a difference between an implementation and itself, or the mutant
/// counts above would be noise.
#[test]
fn an_implementation_agrees_with_itself_on_every_case() {
    let cases = all_cases();
    let diffs = run_parallel(&cases, &|index, case| {
        difference(&references()[index], &observe(&FullComputation, case))
    });
    for (case, diff) in cases.iter().zip(&diffs) {
        assert!(diff.is_none(), "{}: {}", case.label, diff.clone().unwrap());
    }
}

// ---------------------------------------------------------------------------------------------
// The incremental window against the full computation
// ---------------------------------------------------------------------------------------------

/// The prewarmed full computation on every case of [`all_cases`], in the same order, computed once:
/// the reference for the byte-exact comparison of the incremental window, which prewarms too.
fn prewarmed_references() -> &'static [Observed] {
    static CELL: OnceLock<Vec<Observed>> = OnceLock::new();
    CELL.get_or_init(|| parallel(&all_cases(), &|case| observe(&PrewarmedFull, case)))
}

/// How the incremental window decided one case.
struct Decided {
    observed: Observed,
    fallback: Option<FallbackReason>,
    /// The reason of the fallback when the window fell back, whether the full computation then
    /// accepted or refused.
    reason: Option<String>,
    /// The refusal came from the incremental check's exact totals, without the full computation.
    direct_refusal: bool,
    /// The oracle of the host: the report against the full projection of the accepted document.
    oracle: Option<Result<(), WindowDivergence>>,
    /// What the host does with an accepted document, whichever path decided: export it, load the
    /// snapshot into a fresh engine, project and check. Independent of every id cache of the worker.
    fresh: Option<Result<(), CollabError>>,
}

fn decide(implementation: &Incremental, case: &Case) -> Decided {
    walk_counter::reset();
    let (outcome, engine) = implementation.window_with(&case.base, &case.update, &case.facts);
    let reason = walk_counter::fallback_reasons().last().cloned();
    let direct_refusal = walk_counter::window_counts().direct_refusals > 0;
    match outcome {
        Ok(accepted) => {
            // The host's oracle: it projects the document it loads from the snapshot the worker
            // returned, not the worker's own engine (whose id caches can differ from a fresh load).
            // A document it cannot project at all is a divergence too.
            let oracle = accepted.report.as_ref().map(|report| {
                let host = engine
                    .export_snapshot()
                    .ok()
                    .and_then(|snapshot| LoroCollabEngine::load(&snapshot).ok());
                host.and_then(|host| host.semantic_snapshot().ok()).map_or(
                    Err(WindowDivergence {
                        field: "projection_error",
                        reported: 0,
                        projected: 0,
                    }),
                    |projection| compare_with_projection(report, &projection),
                )
            });
            let fresh = Some(engine.fresh_load_decision());
            let result = WindowResult {
                decision: Decision::Accept,
                engine: Some(engine),
                facts: None,
                report: accepted.report,
            };
            Decided {
                observed: Observed {
                    name: implementation.name,
                    decision: result.decision,
                    facts: None,
                    report: result.report,
                    outputs: result.engine.as_ref().map(outputs),
                },
                fallback: accepted.fallback,
                reason,
                direct_refusal,
                oracle,
                fresh,
            }
        }
        Err(error) => Decided {
            observed: Observed {
                name: implementation.name,
                decision: Decision::Refuse(error),
                facts: None,
                report: None,
                outputs: None,
            },
            fallback: None,
            reason,
            direct_refusal,
            oracle: None,
            fresh: None,
        },
    }
}

/// The incremental window against the full computation on every generated, boundary and hostile
/// case: the same decision (the whole error, so the same reason, limit kind, `observed` and
/// `limit`), the same frontier, exported value, roots, history and projection, and a snapshot that
/// reloads to the same document. The snapshot bytes themselves are not compared: Loro encodes a
/// container from its decoded state when that exists and copies the stored bytes when it does not,
/// so they depend on which reads preceded the export, and the incremental window reads less than
/// the projection does (the count of accepted cases with identical bytes is printed). Every report
/// the incremental check produced agrees with the projection of the accepted document, and the
/// oracle the host runs never fires.
#[test]
fn the_incremental_window_agrees_with_the_full_computation_on_every_case() {
    let cases = all_cases();
    let started = Instant::now();
    let decided = run_parallel(&cases, &|index, case| {
        let decided = decide(&Incremental::WORKER, case);
        // Snapshot bytes are not independent of which reads came before the export (see
        // `output_diff_with`), and the incremental window reads far less than the projection: they
        // are compared only where the reads agree. The reloaded document is always compared.
        let exact = difference_with(&prewarmed_references()[index], &decided.observed, false);
        let against_plain = difference_with(&references()[index], &decided.observed, false);
        let same_bytes = prewarmed_references()[index].same_snapshot_bytes(&decided.observed);
        (
            decided.fallback,
            decided.observed.report,
            decided.oracle,
            exact,
            against_plain,
            same_bytes,
            decided.reason,
            decided.direct_refusal,
            decided.fresh,
        )
    });
    let elapsed = started.elapsed();
    let mut incremental = 0;
    let mut accepted_full = 0;
    let mut refused = 0;
    let mut by_reason: BTreeMap<String, usize> = BTreeMap::new();
    let mut by_model: BTreeMap<String, (usize, usize)> = BTreeMap::new();
    let mut same_bytes_count = 0;
    let mut every_reason: BTreeMap<String, usize> = BTreeMap::new();
    let mut refused_after_fallback = 0;
    let mut refused_directly = 0;
    for (case, (fallback, report, oracle, exact, against_plain, same_bytes, reason, direct, fresh)) in
        cases.iter().zip(&decided)
    {
        if let Some(Err(error)) = fresh {
            panic!(
                "{}: the accepted document is refused by a fresh load: {error}",
                case.label
            );
        }
        if let Some(reason) = reason {
            *every_reason.entry(reason.clone()).or_default() += 1;
        }
        same_bytes_count += usize::from(*same_bytes == Some(true));
        assert!(exact.is_none(), "{}: {}", case.label, exact.clone().unwrap());
        assert!(
            against_plain.is_none(),
            "{}: {}",
            case.label,
            against_plain.clone().unwrap()
        );
        if let Some(Err(divergence)) = oracle {
            panic!("{}: the oracle fires: {divergence:?}", case.label);
        }
        let group = if case.family == "generated" {
            case.label.split('/').next().unwrap().to_owned()
        } else {
            case.family.to_owned()
        };
        let entry = by_model.entry(group).or_default();
        if report.is_some() {
            incremental += 1;
            entry.0 += 1;
        } else if let Some(reason) = fallback {
            accepted_full += 1;
            entry.1 += 1;
            *by_reason.entry(format!("{reason:?}")).or_default() += 1;
        } else {
            refused += 1;
            refused_after_fallback += usize::from(reason.is_some());
            refused_directly += usize::from(*direct);
        }
    }
    println!(
        "incremental window: {} cases in {} ms: {incremental} decided incrementally, {accepted_full} accepted by the \
         full computation after a fallback, {refused} refused by the import; snapshot bytes identical to the \
         prewarmed full computation in {same_bytes_count} accepted cases",
        cases.len(),
        elapsed.as_millis()
    );
    println!("  by model (incremental, accepted after fallback): {by_model:?}");
    println!("  fallbacks of accepted updates by reason: {by_reason:?}");
    println!("  fallbacks of every case (accepted or refused afterwards) by reason: {every_reason:?}");
    println!(
        "  of the {refused} refusals: {refused_directly} produced by the incremental check from its totals, \
         {refused_after_fallback} by the full computation after a fallback, {} by the import",
        refused - refused_directly - refused_after_fallback
    );
    assert!(incremental > 0 && accepted_full > 0);
    // The incremental path must carry the ordinary updates of the models it describes.
    for model in ["model1", "legacy", "mixed"] {
        let (done, fell_back) = by_model.get(model).copied().unwrap_or_default();
        assert!(
            done * 2 > fell_back,
            "{model}: only {done} incremental decisions against {fell_back} fallbacks"
        );
    }
}

/// With the configuration switch off (no facts: the full window), the worker is `next` on every
/// input: the same decision (the whole error), the same id caches, the same accepted document, the
/// same projection, and a fresh load of the accepted document agrees. `next` here is
/// `next_oracle`: its own import with its own full id-cache rebuild, written without the helpers
/// of the branch.
#[test]
fn with_the_switch_off_the_window_is_next_on_every_case() {
    let cases = all_cases();
    let results = run_parallel(&cases, &|_, case| {
        let mut next = LoroCollabEngine::load(&case.base).unwrap();
        let next_decision = next.next_window(&case.update);
        let mut worker = LoroCollabEngine::load(&case.base).unwrap();
        worker.prewarm_for_writes();
        let worker_decision = run_window(&mut worker, None, &case.update).map(|_| ());
        if next_decision != worker_decision {
            return Err(format!(
                "decision differs: next {next_decision:?}, worker {worker_decision:?}"
            ));
        }
        if next_decision.is_ok() {
            if worker.id_to_tree != next.id_to_tree || worker.tree_to_id != next.tree_to_id {
                return Err("the id caches differ from next's".to_owned());
            }
            if worker.semantic_snapshot().map(|s| s.semantic_hash())
                != next.semantic_snapshot().map(|s| s.semantic_hash())
            {
                return Err("the projections differ".to_owned());
            }
            if digest(&worker.doc.get_deep_value()) != digest(&next.doc.get_deep_value()) {
                return Err("the documents differ".to_owned());
            }
            if worker.frontier() != next.frontier() {
                return Err("the frontiers differ".to_owned());
            }
            worker
                .fresh_load_decision()
                .map_err(|e| format!("fresh load refuses: {e}"))?;
        }
        Ok(next_decision.is_ok())
    });
    let mut accepted = 0;
    for (case, result) in cases.iter().zip(results) {
        accepted += usize::from(result.unwrap_or_else(|message| panic!("{}: {message}", case.label)));
    }
    println!(
        "the switched-off window is next in {} cases ({accepted} accepted)",
        cases.len()
    );
}

/// The incremental window with a default prewarm and facts, refused cases included, hands the host
/// exactly what the full computation would: a refused update fell back, so the error is the full
/// computation's. This counts, per refusal code, the refusals that came out of a fallback.
#[test]
fn every_refusal_of_the_incremental_window_is_the_full_computations() {
    let cases = all_cases();
    let verdicts = run_parallel(&cases, &|index, case| {
        let decided = decide(&Incremental::WORKER, case);
        (references()[index].decision.clone(), decided.observed.decision)
    });
    let mut refusals = 0;
    for (case, (full, incremental)) in cases.iter().zip(&verdicts) {
        assert_eq!(full, incremental, "{}", case.label);
        refusals += usize::from(matches!(full, Decision::Refuse(_)));
    }
    assert!(refusals > 100, "the corpus has too few refusals: {refusals}");
}

/// Every reason the check can give for not deciding is reached by some case of the default run, in
/// the cases the full computation then accepts and in those it then refuses. A reason added to
/// [`FallbackReason`] without a case that reaches it fails here, so no fallback goes untested.
#[test]
fn every_fallback_reason_is_reached_by_some_case() {
    let cases = all_cases();
    let reasons = run_parallel(&cases, &|_, case| decide(&Incremental::WORKER, case).reason);
    let mut reached: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for (case, reason) in cases.iter().zip(&reasons) {
        if let Some(reason) = reason {
            let examples = reached.entry(reason.clone()).or_default();
            if examples.len() < 2 {
                examples.push(case.label.clone());
            }
        }
    }
    for (reason, examples) in &reached {
        println!("  {reason}: e.g. {examples:?}");
    }
    let missing: Vec<String> = FallbackReason::ALL
        .iter()
        .map(|reason| format!("{reason:?}"))
        .filter(|name| !reached.contains_key(name))
        .collect();
    assert!(missing.is_empty(), "no case reaches the fallback reason(s) {missing:?}");
}

/// A refusal by a total over its limit is produced by the incremental check itself, with the limit
/// kind, `observed` and `limit` of the full computation, on every boundary case that the check
/// can decide: no fallback, no full computation.
#[test]
fn a_refusal_by_a_total_is_produced_without_a_fallback_on_every_boundary() {
    let cases: Vec<&Case> = boundary().cases.iter().collect();
    let results = parallel(&cases, &|case| {
        let decided = decide(&Incremental::WORKER, case);
        (decided.direct_refusal, decided.reason, decided.observed.decision)
    });
    let mut direct = 0;
    for (case, (direct_refusal, reason, decision)) in cases.iter().zip(&results) {
        let Decision::Refuse(CollabError::LimitExceeded { limit_kind, .. }) = decision else {
            continue;
        };
        let total = matches!(
            *limit_kind,
            "container_count" | "document_block_count" | "document_text_chars"
        );
        let facts_exist = !case.label.starts_with("structured");
        if total && facts_exist && reason.is_none() {
            assert!(
                *direct_refusal,
                "{}: a total refused, but not by the check itself",
                case.label
            );
        }
        if *direct_refusal {
            assert!(total, "{}: a direct refusal of {limit_kind}", case.label);
            assert!(reason.is_none(), "{}: a direct refusal that also fell back", case.label);
            direct += 1;
        }
    }
    println!("{direct} boundary refusals produced directly");
    assert!(direct >= 8, "only {direct} direct refusals on the boundary");
}

/// The id caches of an engine after an import that rebuilt them from the update's ops are the
/// caches a full rebuild gives (the replay import of the same update into the same base rebuilds
/// them in full), on every case the window accepts, and so are the uncached nodes and the
/// duplicate flag.
#[test]
fn the_incremental_id_cache_rebuild_equals_the_full_rebuild() {
    let cases = all_cases();
    let checked = run_parallel(&cases, &|_, case| {
        let (outcome, engine) = Incremental::WORKER.window_with(&case.base, &case.update, &case.facts);
        if outcome.is_err() {
            return Ok(false);
        }
        // The same base and update through the replay import, which rebuilds the caches in full.
        let mut full = LoroCollabEngine::load(&case.base).unwrap();
        if full.import_update(&case.update).is_err() {
            return Ok(false);
        }
        let same = engine.id_to_tree == full.id_to_tree
            && engine.tree_to_id == full.tree_to_id
            && engine.uncached_tree_nodes == full.uncached_tree_nodes
            && engine.duplicate_logical_ids == full.duplicate_logical_ids;
        if same {
            Ok(true)
        } else {
            Err(format!(
                "{}: the id caches differ from a full rebuild: id_to_tree {} vs {}, tree_to_id {} vs {}, uncached {:?} vs {:?}, duplicate {} vs {}",
                case.label,
                engine.id_to_tree.len(),
                full.id_to_tree.len(),
                engine.tree_to_id.len(),
                full.tree_to_id.len(),
                engine.uncached_tree_nodes,
                full.uncached_tree_nodes,
                engine.duplicate_logical_ids,
                full.duplicate_logical_ids
            ))
        }
    });
    let mut compared = 0;
    for result in checked {
        compared += usize::from(result.unwrap());
    }
    assert!(compared > 1_000, "only {compared} cases compared");
    println!("id caches equal the full rebuild's in {compared} cases");
}

/// Which way the incremental window decided every hostile and boundary shape, printed so that a
/// change of path is visible in review. A shape the full computation refuses never ends as an
/// incremental accept (the decision comparison above); this records where each one went.
#[test]
fn hostile_and_boundary_shapes_report_their_path() {
    let cases: Vec<&Case> = [boundary(), hostile()]
        .into_iter()
        .flat_map(|family| family.cases.iter())
        .collect();
    let paths = run_parallel(&cases, &|_, case| {
        let decided = decide(&Incremental::WORKER, case);
        let fell_back = decided.reason.is_some();
        match (&decided.observed.decision, decided.observed.report, decided.fallback) {
            (Decision::Refuse(_), _, _) if decided.direct_refusal => {
                "refused by the incremental check from its totals".to_owned()
            }
            (Decision::Refuse(_), _, _) if fell_back => {
                format!(
                    "refused by the full computation after a fallback ({})",
                    decided.reason.clone().unwrap_or_default()
                )
            }
            (Decision::Refuse(_), _, _) => "refused by the import".to_owned(),
            (Decision::Accept, Some(_), _) => "incremental accept".to_owned(),
            (Decision::Accept, None, Some(reason)) => format!("accept after fallback {reason:?}"),
            (Decision::Accept, None, None) => "accept (no report)".to_owned(),
        }
    });
    for (case, path) in cases.iter().zip(&paths) {
        println!("  {:<12} {:<95} {path}", case.family, case.label);
    }
}

/// Two peers concurrently move A under B and B under A in the legacy tree: the library resolves
/// the cycle by ignoring one move, which changes a node the second update does not name. The
/// incremental check does not model tree moves, so the update takes the full computation, with the
/// same decision and the same document.
#[test]
fn a_legacy_cycle_through_concurrent_moves_is_decided_by_the_full_computation() {
    use super::window_fixtures::{BaseSpec, History, Model, author_update, build_base};
    let built = build_base(BaseSpec {
        model: Model::LegacyTree,
        blocks: 12,
        seed: 21,
        history: History::None,
    });
    let (a, b) = (built.ids[0].clone(), built.ids[1].clone());
    let mv = |id: String, parent: String| {
        move |engine: &mut LoroCollabEngine| {
            engine
                .apply_operation(&Operation::MoveNode {
                    id: NodeId::from(id.as_str()),
                    new_parent: Some(NodeId::from(parent.as_str())),
                    index: 0,
                })
                .expect("the move applies locally");
        }
    };
    // Both nodes sit under the root, so both moves are valid on the base.
    let first = author_update(&built.author, 98_101, mv(a.clone(), b.clone()));
    let second = author_update(&built.author, 98_102, mv(b, a));
    let mut base_engine = LoroCollabEngine::load(&built.snapshot).unwrap();
    base_engine.import_update(&first).unwrap();
    let base = base_engine.export_snapshot().unwrap();
    let case = Case {
        label: "legacy cycle".into(),
        family: "hostile",
        kind: "cycle",
        base: std::sync::Arc::new(base),
        update: second,
        expect: None,
        facts: FactsFrom::Base,
    };
    let decided = decide(&Incremental::WORKER, &case);
    assert_eq!(
        decided.fallback,
        Some(FallbackReason::NotSequential),
        "an update concurrent with the base is not decided incrementally"
    );
    let reference = observe(&FullComputation, &case);
    assert_eq!(difference_with(&reference, &decided.observed, false), None);
}

/// The mutants of the incremental window: ways it goes wrong that the harness must catch, each by
/// at least one case, with the case that catches it first.
fn incremental_mutants() -> Vec<Incremental> {
    vec![
        Incremental {
            name: "mutant: stale base facts",
            defect: Defect::StaleBaseFacts,
        },
        Incremental {
            name: "mutant: a touched unit is missed",
            defect: Defect::Fault(Fault::TouchedUnitMissed),
        },
        Incremental {
            name: "mutant: the delta is applied twice",
            defect: Defect::Fault(Fault::DeltaAppliedTwice),
        },
        Incremental {
            name: "mutant: removed blocks are not subtracted",
            defect: Defect::Fault(Fault::RemovedBlocksIgnored),
        },
        Incremental {
            name: "mutant: no fallback on a structural change",
            defect: Defect::Fault(Fault::StructuralFallbackSkipped),
        },
        Incremental {
            name: "mutant: a created tree node is taken for new although it existed",
            defect: Defect::Fault(Fault::CreatedTargetChecksSkipped),
        },
        Incremental {
            name: "mutant: no fallback when the page's items are over the budget",
            defect: Defect::Fault(Fault::WorkBudgetSkipped),
        },
        Incremental {
            name: "mutant: no fallback when an existing node's logical id is rewritten",
            defect: Defect::Fault(Fault::IdRewriteSkipped),
        },
        Incremental {
            name: "mutant: the sequential gate is removed (the id caches are rebuilt from the update's ops whatever it is)",
            defect: Defect::Fault(Fault::SequentialGateRemoved),
        },
        Incremental {
            name: "mutant: a container without a path is skipped as unreachable",
            defect: Defect::Fault(Fault::PathlessContainerSkipped),
        },
        Incremental {
            name: "mutant: a refusal by a total reports observed + 1",
            defect: Defect::Fault(Fault::RefusalOffByOne),
        },
    ]
}

/// Whether a mutant's wrong report is also what the host's oracle (the report against the full
/// projection) is expected to catch. A mutant that changes the path but not the decision or the
/// report (the length limit), or that refuses with a wrong number (no report), is caught by the
/// harness alone.
fn oracle_expected(mutant: &Incremental) -> bool {
    !matches!(
        mutant.defect,
        Defect::Fault(Fault::WorkBudgetSkipped | Fault::RefusalOffByOne)
    )
}

/// Every mutant of the incremental window is caught by the harness (a different decision, output
/// or report from the full computation) and, separately, by the oracle the host runs (the report
/// against the full projection of the accepted document).
#[test]
fn the_harness_detects_every_incremental_mutant() {
    let cases = all_cases();
    // The path the worker takes on every case: a mutant that decides the same but along another
    // path (it skips a fallback) is caught by the path.
    let worker_paths = run_parallel(&cases, &|_, case| {
        let decided = decide(&Incremental::WORKER, case);
        (decided.reason, decided.direct_refusal)
    });
    for mutant in incremental_mutants() {
        let results = run_parallel(&cases, &|index, case| {
            let decided = decide(&mutant, case);
            let path_differs = (decided.reason.clone(), decided.direct_refusal) != worker_paths[index];
            let diff = difference_with(&prewarmed_references()[index], &decided.observed, false);
            (
                diff.or_else(|| path_differs.then(|| "the path differs from the worker's".to_owned())),
                matches!(decided.oracle, Some(Err(_))),
            )
        });
        let mut by_family: BTreeMap<&str, usize> = BTreeMap::new();
        let mut first: Option<(String, String)> = None;
        let mut oracle_catches = 0;
        let mut caught = 0;
        for (case, (diff, oracle)) in cases.iter().zip(&results) {
            oracle_catches += usize::from(*oracle);
            if let Some(diff) = diff {
                caught += 1;
                *by_family.entry(case.family).or_default() += 1;
                first.get_or_insert_with(|| (case.label.clone(), diff.clone()));
            }
        }
        println!(
            "  {}\n    harness catches it in {caught} cases {by_family:?}; first: {first:?}\n    oracle fires in {oracle_catches} cases",
            mutant.name
        );
        assert!(caught > 0, "{} was not detected by the harness", mutant.name);
        if oracle_expected(&mutant) {
            assert!(oracle_catches > 0, "{} was not detected by the oracle", mutant.name);
        }
    }
}

// ---------------------------------------------------------------------------------------------
// The counting test
// ---------------------------------------------------------------------------------------------

/// The work one window did: the walk counters and the bytes allocated on the calling thread.
#[derive(Debug, Clone, Copy)]
struct WindowWork {
    counts: walk_counter::WindowCounts,
    allocated: usize,
    /// Why the incremental check did not decide, when it did not.
    fallback: Option<FallbackReason>,
    /// The bytes Loro's own import of the same update allocates on an identically prepared
    /// document, with nothing around it. In a build with debug assertions the library checks its
    /// history against the block headers on every import, which reads the whole change store;
    /// that is the library's cost, not the window's, and it is what `allocated - raw_import`
    /// leaves out.
    raw_import: usize,
}

impl WindowWork {
    /// What the window itself allocated: everything but the library's import.
    const fn own(&self) -> usize {
        self.allocated.saturating_sub(self.raw_import)
    }
}

/// The shape of the update of a counting run.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Shape {
    /// One character typed into an existing block: one unit changes.
    OneBlockEdit,
    /// One new block with its text: one unit is added.
    OneBlockInsert,
}

/// Measures the metered part of one window (`run_window`: import and check) for `shape` on a page of
/// `blocks` blocks of `model`. The base is loaded, prewarmed and its facts computed before the
/// counters start, as the worker does in its unmetered setup.
fn measure_window(model: Model, blocks: usize, shape: Shape) -> WindowWork {
    let built = build_base(BaseSpec {
        model,
        blocks,
        seed: 3,
        history: History::None,
    });
    let target = if model == Model::LegacyTree { "n1" } else { "b1" }.to_owned();
    let update = author_update(&built.author, 880_001, |engine| match shape {
        Shape::OneBlockEdit => {
            if model == Model::LegacyTree {
                // Only a block node takes text: find one.
                let id = built
                    .ids
                    .iter()
                    .find(|id| {
                        engine
                            .apply_content_operation(&Operation::InsertText {
                                id: NodeId::from(id.as_str()),
                                index: 0,
                                text: "x".into(),
                            })
                            .is_ok()
                    })
                    .expect("a legacy block to edit");
                let _ = id;
            } else {
                engine
                    .apply_content_operation(&Operation::InsertText {
                        id: NodeId::from(target.as_str()),
                        index: 0,
                        text: "x".into(),
                    })
                    .unwrap();
            }
        }
        Shape::OneBlockInsert => {
            engine
                .apply_content_operation(&Operation::CreateNode {
                    id: NodeId::from("fresh"),
                    parent: None,
                    index: 0,
                    kind: NodeKind::Block,
                })
                .unwrap();
            engine
                .apply_content_operation(&Operation::InsertText {
                    id: NodeId::from("fresh"),
                    index: 0,
                    text: "typed".into(),
                })
                .unwrap();
        }
    });
    let raw = LoroCollabEngine::load(&built.snapshot).unwrap();
    raw.prewarm_for_writes();
    let raw_before = test_alloc::allocated_bytes();
    raw.doc.import(&update).unwrap();
    let raw_import = test_alloc::allocated_bytes().saturating_sub(raw_before);
    let mut engine = LoroCollabEngine::load(&built.snapshot).unwrap();
    engine.prewarm_for_writes();
    let facts = BaseFacts::compute(&engine);
    walk_counter::reset();
    let allocated_before = test_alloc::allocated_bytes();
    let accepted = run_window(&mut engine, facts.as_ref(), &update).expect("the update is accepted");
    let allocated = test_alloc::allocated_bytes().saturating_sub(allocated_before);
    WindowWork {
        counts: walk_counter::window_counts(),
        allocated,
        fallback: accepted.fallback,
        raw_import,
    }
}

const SMALL_PAGE: usize = 200;
const LARGE_PAGE: usize = 2_000;

/// The structured pages (models 4 and 5) still take the full computation until the structured
/// path of the incremental check exists (work package C2), so the work of one window for a
/// one-block update still grows with the page: every window makes at least one pass over the whole
/// block list and the bytes it allocates grow roughly with the number of blocks.
///
/// Work package C2 inverts this test for the structured models: add them to the list of
/// [`one_window_does_not_grow_with_the_page`] and delete this one.
#[test]
fn structured_pages_window_grows_with_the_page_until_c2() {
    for model in [Model::Structured4, Model::Structured5] {
        for shape in [Shape::OneBlockEdit, Shape::OneBlockInsert] {
            let small = measure_window(model, SMALL_PAGE, shape);
            let large = measure_window(model, LARGE_PAGE, shape);
            let tenths = large.allocated * 10 / small.allocated.max(1);
            println!(
                "{:<8} {:<15} {SMALL_PAGE} blocks: {:>9} bytes, {:?}\n{:<8} {:<15} {LARGE_PAGE} blocks: {:>9} bytes, {:?}  (x{}.{})",
                model.name(),
                format!("{shape:?}"),
                small.allocated,
                small.counts,
                model.name(),
                format!("{shape:?}"),
                large.allocated,
                large.counts,
                tenths / 10,
                tenths % 10,
            );
            // C2 MUST INVERT THESE.
            assert_eq!(
                (small.fallback, large.fallback),
                (Some(FallbackReason::FactsAbsent), Some(FallbackReason::FactsAbsent)),
                "{} {shape:?}: a structured page no longer falls back; if C2 exists, invert this test",
                model.name()
            );
            assert!(
                small.counts.block_reads >= 1 && large.counts.block_reads >= 1,
                "{} {shape:?}: the window no longer reads the whole block list ({:?}, {:?}); \
                 if this is the incremental window, invert the assertions",
                model.name(),
                small.counts,
                large.counts
            );
            assert!(
                large.allocated > 3 * small.allocated,
                "{} {shape:?}: the window's allocation no longer grows with the page \
                 ({} bytes on {SMALL_PAGE} blocks, {} on {LARGE_PAGE}); if this is the incremental \
                 window, invert the assertions",
                model.name(),
                small.allocated,
                large.allocated
            );
        }
    }
}

/// The counting test of the incremental window, for flat, legacy tree and mixed pages: no full
/// block pass, no placement walk and no fallback; exactly one unit evaluated for a one-block edit
/// (and for a one-block insert); allocation within 1.2x on a page ten times larger (the window's own
/// allocation in every build, the whole window too in a build without debug assertions).
#[test]
fn one_window_does_not_grow_with_the_page() {
    for model in [Model::Flat, Model::LegacyTree, Model::Mixed] {
        for shape in [Shape::OneBlockEdit, Shape::OneBlockInsert] {
            let small = measure_window(model, SMALL_PAGE, shape);
            let large = measure_window(model, LARGE_PAGE, shape);
            println!(
                "{:<8} {:<15} {SMALL_PAGE} blocks: {:>9} bytes, {:?}\n{:<8} {:<15} {LARGE_PAGE} blocks: {:>9} bytes, {:?}",
                model.name(),
                format!("{shape:?}"),
                small.allocated,
                small.counts,
                model.name(),
                format!("{shape:?}"),
                large.allocated,
                large.counts,
            );
            for work in [small, large] {
                assert_eq!(work.fallback, None, "{} {shape:?}: the check fell back", model.name());
                assert_eq!(
                    work.counts.block_reads,
                    0,
                    "{} {shape:?}: full block passes",
                    model.name()
                );
                assert_eq!(
                    work.counts.placement_walks,
                    0,
                    "{} {shape:?}: placement walks",
                    model.name()
                );
                assert_eq!(work.counts.fallbacks, 0, "{} {shape:?}: fallbacks", model.name());
                assert_eq!(
                    work.counts.unit_evaluations,
                    1,
                    "{} {shape:?}: units evaluated",
                    model.name()
                );
            }
            // What the window allocates besides Loro's own import: within 1.2x on a page ten times
            // larger, plus 8 KiB for the difference between two separate runs of the library's
            // import, which is what this subtracts (a growth that is linear in the page is hundreds
            // of KiB at 2,000 blocks, far above it).
            assert!(
                large.own() * 10 <= small.own() * 12 + 80 * 1024,
                "{} {shape:?}: the window allocated {} bytes of its own on {LARGE_PAGE} blocks against {} on \
                 {SMALL_PAGE} (whole window {} and {}, Loro's import alone {} and {})",
                model.name(),
                large.own(),
                small.own(),
                large.allocated,
                small.allocated,
                large.raw_import,
                small.raw_import
            );
            // Without debug assertions in the library nothing but the update's own work remains, and
            // the whole window, import included, stays within 1.2x.
            if !cfg!(debug_assertions) {
                assert!(
                    large.allocated * 10 <= small.allocated * 12,
                    "{} {shape:?}: allocated {} bytes on {LARGE_PAGE} blocks against {} on {SMALL_PAGE}",
                    model.name(),
                    large.allocated,
                    small.allocated
                );
            }
        }
    }
}
