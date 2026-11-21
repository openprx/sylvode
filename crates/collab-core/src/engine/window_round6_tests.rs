//! Round six: the cost of converting a text grows with its internal pieces, so the work measure
//! charges them (R6-1), and the per-block formatting allowance (R6-2).
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::print_stdout
)]

use loro::{ExportMode, LoroDoc};

use super::window_round2_tests::flat_page_of;
use super::window_round4_tests::first_text;
use super::{CollabEngine, LoroCollabEngine};
use crate::facts::BaseFacts;

/// A style config that accepts `keys` (and `bold`) without expanding.
pub(super) fn configure_styles(raw: &LoroDoc, keys: &[String]) {
    let mut styles = loro::StyleConfigMap::new();
    for key in keys.iter().map(String::as_str).chain(["bold"]) {
        styles.insert(
            key.into(),
            loro::StyleConfig {
                expand: loro::ExpandType::None,
            },
        );
    }
    raw.config_text_style(styles);
}

/// Fresh one- and two-letter mark keys.
pub(super) fn mark_keys(count: usize) -> Vec<String> {
    ('b'..='z')
        .map(String::from)
        .chain(('a'..='z').flat_map(|a| ('a'..='z').map(move |b| format!("{a}{b}"))))
        .take(count)
        .collect()
}

/// A flat page of `blocks` paragraphs, the first `fragmented` of which hold `chars` characters typed
/// in reverse order (every character inserted at position 0, so that the text keeps one internal
/// piece per character through a snapshot) under `marks` full-range marks with fresh keys and the
/// value `false`, which the reader skips before it checks the key. Built as a snapshot.
pub(super) fn fragmented_page(blocks: usize, fragmented: usize, chars: usize, marks: usize) -> Vec<u8> {
    let base = flat_page_of(blocks);
    let raw = LoroDoc::new();
    raw.import(&base).unwrap();
    raw.set_peer_id(48_000).unwrap();
    let keys = mark_keys(marks);
    configure_styles(&raw, &keys);
    for index in 0..fragmented {
        let text = first_text(&raw, index);
        let ch = "y";
        for _ in 0..chars {
            text.insert(0, ch).unwrap();
        }
        let len = text.len_unicode();
        for key in &keys {
            text.mark(0..len, key, false).unwrap();
        }
    }
    raw.commit();
    raw.export(ExportMode::snapshot()).unwrap()
}

/// R6-1: a text typed in reverse order stays one piece per character, and converting it costs the
/// pieces times the attributes of its segment, which the measure charged as one segment. The page
/// below is cheap by the old measure (about 5 items of marks per block) and expensive to project.
#[test]
fn r6_1_a_fragmented_text_under_full_range_marks_is_charged_by_its_pieces() {
    let base = fragmented_page(40, 40, 1_500, 15);
    let engine = LoroCollabEngine::load(&base).unwrap();
    let (items, budget) = super::window_round2_tests::items_and_budget(&base);
    println!("fragmented 40 x 1,500 x 15: items {items}, budget {budget}");
    assert!(
        items > budget,
        "the page costs {items} items against a budget of {budget}: the pieces are free"
    );
    assert!(BaseFacts::compute(&engine).is_none());
}

/// A flat page of 100 blocks whose first paragraph is 3,000 characters typed in reverse order under
/// as many full-range marks as keep the page within its work budget: it has facts, and the next mark
/// over the whole text is not within the budget.
pub(super) fn near_budget_fragmented_page() -> Vec<u8> {
    let mut best = None;
    for marks in 1..=60 {
        let candidate = fragmented_page(100, 1, 3_000, marks);
        let (items, budget) = super::window_round2_tests::items_and_budget(&candidate);
        if items > budget {
            break;
        }
        best = Some(candidate);
    }
    best.expect("a page within the budget")
}

/// The corpus extension of this round: a page near its budget whose first text is fragmented, one
/// more full-range mark (every piece gains an attribute) and one typed character. The decision is
/// the budget's, which the mutant that does not count the pieces gets wrong.
pub(super) fn round6_cases(scale: u64) -> Vec<super::window_fixtures::Case> {
    use super::window_fixtures::{Case, FactsFrom};
    let base = std::sync::Arc::new(near_budget_fragmented_page());
    let mut cases = Vec::new();
    for k in 0..scale {
        let mark = super::hand_encoded::raw_edit(&base, 49_000 + k, |raw| {
            let key = format!("zz{k}");
            configure_styles(raw, std::slice::from_ref(&key));
            let text = first_text(raw, 0);
            text.mark(0..text.len_unicode(), &key, false).unwrap();
        });
        let at = usize::try_from(1_000 + 7 * k).unwrap();
        let typed = super::hand_encoded::raw_edit(&base, 49_500 + k, |raw| {
            first_text(raw, 0).insert(at, "q").unwrap();
        });
        for (kind, update) in [("fragmented_full_range_mark", mark), ("fragmented_typed", typed)] {
            cases.push(Case {
                label: format!("round6/{kind}/{k}"),
                family: "round6",
                kind,
                base: base.clone(),
                update,
                expect: None,
                facts: FactsFrom::Base,
            });
        }
    }
    cases
}

/// How the text of every block of a [`formatted_page`] is formatted.
#[derive(Debug, Clone, Copy)]
pub(super) enum Formatting {
    /// `clusters` overlapping clusters of `bold`, `italic` and a `link`, 20 characters apart.
    Clusters(usize),
    /// Six marks (`bold`, `italic`, `strike`, `code`, `bold`, `link`) on overlapping ranges.
    SixMarks,
}

/// A flat page of `blocks` paragraphs of 80 characters (longer when the clusters need it), all
/// formatted like `formatting`: what an honest document with heavy formatting looks like. Built as a snapshot, with the real mark values
/// of the server (`true`, and a string for a link).
pub(super) fn formatted_page(blocks: usize, formatting: Formatting) -> Vec<u8> {
    let base = flat_page_of(blocks);
    let raw = LoroDoc::new();
    raw.import(&base).unwrap();
    raw.set_peer_id(48_100).unwrap();
    crate::page_inline::configure(&raw);
    let link = "https://example.test/a/page";
    for index in 0..blocks {
        let text = first_text(&raw, index);
        let length = match formatting {
            Formatting::Clusters(clusters) => (clusters * 30 + 20).max(80),
            Formatting::SixMarks => 80,
        };
        text.insert(1, &"abcdefghij".repeat(length.div_ceil(10))[1..length])
            .unwrap();
        match formatting {
            Formatting::Clusters(clusters) => {
                for cluster in 0..clusters {
                    let start = cluster * 30;
                    text.mark(start..start + 10, "bold", true).unwrap();
                    text.mark(start + 5..start + 15, "italic", true).unwrap();
                    text.mark(start + 8..start + 20, "link", link).unwrap();
                }
            }
            Formatting::SixMarks => {
                text.mark(0..10, "bold", true).unwrap();
                text.mark(5..15, "italic", true).unwrap();
                text.mark(12..30, "strike", true).unwrap();
                text.mark(20..40, "code", true).unwrap();
                text.mark(35..50, "bold", true).unwrap();
                text.mark(45..60, "link", link).unwrap();
            }
        }
    }
    raw.commit();
    raw.export(ExportMode::snapshot()).unwrap()
}

/// A one-character edit and a one-character mark on a page with this formatting are decided
/// incrementally, and the page has facts.
fn assert_formatted_page_is_incremental(blocks: usize, formatting: Formatting) {
    let base = formatted_page(blocks, formatting);
    let (items, budget) = super::window_round2_tests::items_and_budget(&base);
    println!("{blocks} blocks {formatting:?}: items {items}, budget {budget}");
    assert!(items <= budget, "{blocks} blocks {formatting:?}: {items} > {budget}");
    let edit = super::hand_encoded::raw_edit(&base, 48_200, |raw| {
        first_text(raw, blocks / 2).insert(2, "z").unwrap();
    });
    let judged = super::window_round2_tests::judge(&base, &edit);
    judged.assert_sound("an edit on a formatted page");
    assert!(judged.incremental, "{judged:?}");
    let marked = super::hand_encoded::raw_edit(&base, 48_201, |raw| {
        first_text(raw, blocks / 3).mark(60..61, "bold", true).unwrap();
    });
    let judged = super::window_round2_tests::judge(&base, &marked);
    judged.assert_sound("a mark on a formatted page");
    assert!(judged.incremental, "{judged:?}");
}

/// R6-2: ordinary overlapping formatting must not push a medium page off the incremental path.
#[test]
fn r6_2_a_page_of_1200_blocks_with_two_formatting_clusters_per_block_stays_incremental() {
    assert_formatted_page_is_incremental(1_200, Formatting::Clusters(2));
}

#[test]
fn r6_2_a_page_of_2000_blocks_with_six_marks_per_block_stays_incremental() {
    assert_formatted_page_is_incremental(2_000, Formatting::SixMarks);
}

#[test]
fn r6_2_a_page_at_the_block_limit_with_bold_italic_and_a_link_on_every_block_stays_incremental() {
    assert_formatted_page_is_incremental(crate::limits::DOCUMENT_BLOCK_COUNT_MAX - 50, Formatting::Clusters(1));
}

/// The allowance is per block and only for formatting: the same cluster formatting on a page of 100
/// blocks is far over what 100 blocks are allowed, so the page has no facts.
#[test]
fn r6_2_formatting_beyond_the_allowance_counts_in_full() {
    let base = formatted_page(100, Formatting::Clusters(40));
    let (items, budget) = super::window_round2_tests::items_and_budget(&base);
    println!("100 blocks, 40 clusters: items {items}, budget {budget}");
    assert!(items > budget);
    assert!(BaseFacts::compute(&LoroCollabEngine::load(&base).unwrap()).is_none());
}
