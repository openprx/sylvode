//! Inline marks on Loro text, and the read-only projection of a `loro-prosemirror` page.
//!
//! The command vocabulary stores marks on each tree node's `text` container. The page editor
//! stores the same marks on the root `doc` map (`loro-prosemirror`'s schema: a map with
//! `nodeName`, `attributes`, and `children`). Both are projected into [`InlineSpan`]s. A document
//! that has neither marks nor a prosemirror map is unchanged.
//!
//! Reading never calls `get_map` / `get_text`: those attach a root container even when the key is
//! absent. [`LoroDoc::get_by_str_path`] stays empty for a document that has never opened the editor.

use std::collections::BTreeMap;
use std::fmt::Write as _;

use loro::{Container, LoroDoc, LoroMap, LoroText, LoroValue, TextDelta, ToJson, ValueOrContainer};
use sha2::{Digest, Sha256};

use crate::error::CollabError;
use crate::operation::NodeId;
use crate::semantic::{InlineMark, InlineSpan, SemanticNode};

pub(crate) const MARK_BOLD: &str = "bold";
pub(crate) const MARK_ITALIC: &str = "italic";
pub(crate) const MARK_CODE: &str = "code";
pub(crate) const MARK_STRIKE: &str = "strike";
pub(crate) const MARK_LINK: &str = "link";

/// One block read from the editor's prosemirror container. `index` is its position among the
/// editor's top-level blocks; the caller turns that into an `order_key`.
pub(crate) struct EditorBlock {
    pub id: NodeId,
    pub index: usize,
    pub node: SemanticNode,
}

/// Applies or clears one mark. `start`/`end` are UTF-8 byte offsets on a character boundary.
pub(crate) fn apply_mark(
    text: &LoroText,
    start: usize,
    end: usize,
    mark: &str,
    value: Option<&str>,
) -> Result<(), CollabError> {
    let current = text.to_string();
    if start >= end || end > current.len() || current.get(start..end).is_none() {
        return Err(CollabError::OperationFailed {
            reason: "mark range is outside the block text or not on a character boundary".to_string(),
        });
    }
    match value {
        None => {
            let (scalar_start, scalar_end) = scalar_range(&current, start, end)?;
            text.unmark(scalar_start..scalar_end, mark)
                .map_err(|error| CollabError::OperationFailed {
                    reason: error.to_string(),
                })?;
        }
        Some(raw) => match mark {
            MARK_BOLD | MARK_ITALIC | MARK_CODE | MARK_STRIKE => {
                if raw != "true" {
                    return Err(CollabError::OperationFailed {
                        reason: "toggle marks are set with true and cleared with null".to_string(),
                    });
                }
                text.mark_utf8(start..end, mark, true)
                    .map_err(|error| CollabError::OperationFailed {
                        reason: error.to_string(),
                    })?;
            }
            MARK_LINK => {
                if !safe_href(raw) {
                    return Err(CollabError::OperationFailed {
                        reason: "link href must be an http(s), mailto, site-absolute, or fragment address".to_string(),
                    });
                }
                text.mark_utf8(start..end, mark, raw)
                    .map_err(|error| CollabError::OperationFailed {
                        reason: error.to_string(),
                    })?;
            }
            _ => {
                return Err(CollabError::OperationFailed {
                    reason: "unsupported inline mark".to_string(),
                });
            }
        },
    }
    Ok(())
}

/// Visible characters plus the marks that cover them. Unknown attribute keys are ignored so a
/// newer client cannot make an older reader drop the text.
pub(crate) fn read_rich_text(text: &LoroText) -> (String, Vec<InlineSpan>) {
    let mut plain = String::new();
    let mut spans = Vec::new();
    for delta in text.to_delta() {
        let TextDelta::Insert { insert, attributes } = delta else {
            continue;
        };
        let Ok(start) = u32::try_from(plain.len()) else {
            plain.push_str(&insert);
            continue;
        };
        plain.push_str(&insert);
        let Ok(end) = u32::try_from(plain.len()) else {
            continue;
        };
        let Some(attributes) = attributes else {
            continue;
        };
        let mut keys: Vec<&String> = attributes.keys().collect();
        keys.sort();
        for key in keys {
            let Some(value) = attributes.get(key) else {
                continue;
            };
            if let Some(span) = span_from_attribute(key, value, start, end) {
                spans.push(span);
            }
        }
    }
    normalize_spans(&mut spans);
    (plain, spans)
}

/// Top-level blocks of a `loro-prosemirror` document, when one is stored on this doc.
pub(crate) fn editor_blocks(doc: &LoroDoc) -> Vec<EditorBlock> {
    let Some(ValueOrContainer::Container(Container::Map(root))) = doc.get_by_str_path("doc") else {
        return Vec::new();
    };
    if map_string(&root, "nodeName").as_deref() != Some("doc") {
        return Vec::new();
    }
    let Some(ValueOrContainer::Container(Container::List(children))) = root.get("children") else {
        return Vec::new();
    };
    let mut blocks = Vec::new();
    for index in 0..children.len() {
        let Some(ValueOrContainer::Container(Container::Map(block))) = children.get(index) else {
            continue;
        };
        if let Some(editor_block) = read_editor_block(&block, index) {
            blocks.push(editor_block);
        }
    }
    blocks
}

fn read_editor_block(block: &LoroMap, index: usize) -> Option<EditorBlock> {
    let name = map_string(block, "nodeName")?;
    if !matches!(name.as_str(), "paragraph" | "heading" | "bulletItem" | "codeBlock") {
        return None;
    }
    let mut properties = BTreeMap::new();
    properties.insert("block_type".to_string(), name.clone());
    if let Some(ValueOrContainer::Container(Container::Map(attrs))) = block.get("attributes") {
        if let Some(indent) = attr_small_int(&attrs, "indent") {
            properties.insert("indent".to_string(), indent.min(7).to_string());
        }
        if name == "heading" {
            let level = attr_small_int(&attrs, "level").unwrap_or(1).clamp(1, 3);
            properties.insert("level".to_string(), level.to_string());
        }
    }
    let (text, spans) = block_text(block);
    Some(EditorBlock {
        id: editor_block_id(&format!("{:?}", block.id())),
        index,
        node: SemanticNode {
            parent: None,
            order_key: String::new(),
            kind: crate::operation::NodeKind::Block,
            text,
            properties,
            deleted: false,
            spans,
        },
    })
}

fn block_text(block: &LoroMap) -> (String, Vec<InlineSpan>) {
    let Some(ValueOrContainer::Container(Container::List(children))) = block.get("children") else {
        return (String::new(), Vec::new());
    };
    let mut text = String::new();
    let mut spans = Vec::new();
    for index in 0..children.len() {
        let Some(ValueOrContainer::Container(Container::Text(chunk))) = children.get(index) else {
            continue;
        };
        let (chunk_text, chunk_spans) = read_rich_text(&chunk);
        let Ok(offset) = u32::try_from(text.len()) else {
            text.push_str(&chunk_text);
            continue;
        };
        text.push_str(&chunk_text);
        for mut span in chunk_spans {
            span.start = span.start.saturating_add(offset);
            span.end = span.end.saturating_add(offset);
            spans.push(span);
        }
    }
    normalize_spans(&mut spans);
    (text, spans)
}

/// Stable logical id derived from the container id. The container id embeds an engine peer id,
/// so the snapshot stores only this digest.
fn editor_block_id(container_id: &str) -> NodeId {
    let digest = Sha256::digest(container_id.as_bytes());
    let mut hex = String::from("pm:");
    for byte in digest.iter().take(16) {
        let _ = write!(hex, "{byte:02x}");
    }
    NodeId::from(hex)
}

fn map_string(map: &LoroMap, key: &str) -> Option<String> {
    match map.get(key) {
        Some(ValueOrContainer::Value(LoroValue::String(value))) => Some(value.as_ref().to_string()),
        _ => None,
    }
}

#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
fn attr_small_int(map: &LoroMap, key: &str) -> Option<u32> {
    let ValueOrContainer::Value(value) = map.get(key)? else {
        return None;
    };
    let number = value.to_json_value().as_f64()?;
    if !number.is_finite() || !(0.0..=1024.0).contains(&number) || number.fract() != 0.0 {
        return None;
    }
    Some(number as u32)
}

fn span_from_attribute(key: &str, value: &LoroValue, start: u32, end: u32) -> Option<InlineSpan> {
    if start >= end {
        return None;
    }
    let json = value.to_json_value();
    match key {
        MARK_BOLD => toggle_span(InlineMark::Bold, &json, start, end),
        MARK_ITALIC => toggle_span(InlineMark::Italic, &json, start, end),
        MARK_CODE => toggle_span(InlineMark::Code, &json, start, end),
        MARK_STRIKE => toggle_span(InlineMark::Strike, &json, start, end),
        MARK_LINK => {
            let href = href_from_json(&json)?;
            if !safe_href(&href) {
                return None;
            }
            Some(InlineSpan {
                start,
                end,
                kind: InlineMark::Link,
                href: Some(href),
            })
        }
        _ => None,
    }
}

fn toggle_span(kind: InlineMark, json: &serde_json::Value, start: u32, end: u32) -> Option<InlineSpan> {
    if matches!(json, serde_json::Value::Null | serde_json::Value::Bool(false)) {
        return None;
    }
    Some(InlineSpan {
        start,
        end,
        kind,
        href: None,
    })
}

fn href_from_json(json: &serde_json::Value) -> Option<String> {
    match json {
        serde_json::Value::String(value) => Some(value.clone()),
        serde_json::Value::Object(map) => map.get("href").and_then(serde_json::Value::as_str).map(str::to_string),
        _ => None,
    }
}

fn normalize_spans(spans: &mut Vec<InlineSpan>) {
    spans.sort_by(|left, right| {
        left.start
            .cmp(&right.start)
            .then(left.end.cmp(&right.end))
            .then(mark_rank(left.kind).cmp(&mark_rank(right.kind)))
            .then(left.href.cmp(&right.href))
    });
    spans.dedup();
}

fn mark_rank(kind: InlineMark) -> u8 {
    match kind {
        InlineMark::Code => 0,
        InlineMark::Italic => 1,
        InlineMark::Bold => 2,
        InlineMark::Strike => 3,
        InlineMark::Link => 4,
    }
}

fn scalar_range(text: &str, start: usize, end: usize) -> Result<(usize, usize), CollabError> {
    let Some(head) = text.get(..start) else {
        return Err(CollabError::OperationFailed {
            reason: "mark range is not on a character boundary".to_string(),
        });
    };
    let Some(body) = text.get(start..end) else {
        return Err(CollabError::OperationFailed {
            reason: "mark range is not on a character boundary".to_string(),
        });
    };
    Ok((head.chars().count(), head.chars().count() + body.chars().count()))
}

/// Addresses the editor and the public page will turn into a link. `javascript:` and `data:`
/// are refused so a mark cannot become an active script in Markdown or HTML rendering.
pub(crate) fn safe_href(href: &str) -> bool {
    if href.is_empty() || href.len() > 2048 || href.chars().any(|ch| ch.is_control() || ch.is_whitespace()) {
        return false;
    }
    let lower = href.to_ascii_lowercase();
    if lower.starts_with("javascript:") || lower.starts_with("data:") || lower.starts_with("vbscript:") {
        return false;
    }
    lower.starts_with("https://")
        || lower.starts_with("http://")
        || lower.starts_with("mailto:")
        || (href.starts_with('/') && !href.starts_with("//"))
        || href.starts_with('#')
}
