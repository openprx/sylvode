//! Canonical inline runs shared by Page projections and rendered exports.
use loro::{LoroText, TextDelta, ToJson};
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InlineMark {
    Bold,
    Italic,
    Strike,
    Code,
    Link,
}
impl InlineMark {
    pub const fn key(self) -> &'static str {
        match self {
            Self::Bold => "bold",
            Self::Italic => "italic",
            Self::Strike => "strike",
            Self::Code => "code",
            Self::Link => "link",
        }
    }
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InlineSpan {
    /// UTF-8 byte offsets, as used by the command API.
    pub start: usize,
    pub end: usize,
    pub mark: InlineMark,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub href: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MarkEdit {
    pub start: usize,
    pub end: usize,
    pub mark: InlineMark,
    #[serde(default)]
    pub clear: bool,
    #[serde(default)]
    pub href: Option<String>,
}
pub(crate) fn validate_edits(value: &str, edits: &[MarkEdit]) -> Result<(), crate::CollabError> {
    if edits.len() > 256 {
        return Err(invalid("editor_mark_edit_limit"));
    }
    for edit in edits {
        if edit.start >= edit.end
            || edit.end > value.len()
            || !value.is_char_boundary(edit.start)
            || !value.is_char_boundary(edit.end)
        {
            return Err(invalid("editor_mark_invalid_utf8_range"));
        }
        if !edit.clear && edit.mark == InlineMark::Link && !edit.href.as_deref().is_some_and(safe_href) {
            return Err(invalid("editor_link_invalid_href"));
        }
        if edit.mark != InlineMark::Link && edit.href.is_some() {
            return Err(invalid("editor_mark_unexpected_href"));
        }
    }
    Ok(())
}

pub fn apply_edits(text: &LoroText, edits: &[MarkEdit]) -> Result<(), crate::CollabError> {
    let value = text.to_string();
    validate_edits(&value, edits)?;
    for edit in edits {
        if edit.clear {
            let start = value[..edit.start].chars().count();
            let end = value[..edit.end].chars().count();
            text.unmark(start..end, edit.mark.key())
                .map_err(|e| invalid(&e.to_string()))?;
        } else {
            let attrs = edit
                .href
                .as_ref()
                .map_or_else(|| serde_json::json!({}), |href| serde_json::json!({"href":href}));
            let attrs: loro::LoroValue = serde_json::from_value(attrs).map_err(|e| invalid(&e.to_string()))?;
            text.mark_utf8(edit.start..edit.end, edit.mark.key(), attrs)
                .map_err(|e| invalid(&e.to_string()))?;
        }
    }
    Ok(())
}
pub fn configure(doc: &loro::LoroDoc) {
    let mut styles = loro::StyleConfigMap::new();
    for mark in [
        InlineMark::Bold,
        InlineMark::Italic,
        InlineMark::Strike,
        InlineMark::Code,
        InlineMark::Link,
    ] {
        styles.insert(
            mark.key().into(),
            loro::StyleConfig {
                expand: if mark == InlineMark::Link {
                    loro::ExpandType::None
                } else {
                    loro::ExpandType::After
                },
            },
        );
    }
    doc.config_text_style(styles);
}

pub fn safe_href(href: &str) -> bool {
    if href.is_empty() || href.len() > 2048 || href.chars().any(|c| c.is_control() || c.is_whitespace()) {
        return false;
    }
    let lower = href.to_ascii_lowercase();
    lower.starts_with("https://")
        || lower.starts_with("http://")
        || lower.starts_with("mailto:")
        || (href.starts_with('/') && !href.starts_with("//"))
        || href.starts_with('#')
}
pub fn spans(text: &LoroText) -> Result<Vec<InlineSpan>, crate::CollabError> {
    let mut offset = 0;
    let mut spans = Vec::new();
    for delta in text.to_delta() {
        let TextDelta::Insert { insert, attributes } = delta else {
            continue;
        };
        let end = offset + insert.len();
        if let Some(attributes) = attributes {
            for (mark, value) in attributes {
                let value = value.to_json_value();
                if value.is_null() || value == Value::Bool(false) {
                    continue;
                }
                let href = match mark.as_str() {
                    "strong" | "em" | "bold" | "italic" | "strike" | "code"
                        if value == Value::Bool(true) || value.is_object() =>
                    {
                        None
                    }
                    "link" => {
                        let href = value.as_str().or_else(|| value.get("href").and_then(Value::as_str));
                        let Some(href) = href.filter(|href| safe_href(href)) else {
                            return Err(invalid("editor_link_invalid_href"));
                        };
                        Some(href.to_owned())
                    }
                    _ => return Err(invalid("editor_text_marks_unsupported")),
                };
                let mark = match mark.as_str() {
                    "strong" | "bold" => InlineMark::Bold,
                    "em" | "italic" => InlineMark::Italic,
                    "strike" => InlineMark::Strike,
                    "code" => InlineMark::Code,
                    _ => InlineMark::Link,
                };
                spans.push(InlineSpan {
                    start: offset,
                    end,
                    mark,
                    href,
                });
            }
        }
        offset = end;
    }
    spans.sort_by(|a, b| (a.start, a.end, &a.mark, &a.href).cmp(&(b.start, b.end, &b.mark, &b.href)));
    Ok(spans)
}
fn invalid(reason: &str) -> crate::CollabError {
    crate::CollabError::OperationFailed { reason: reason.into() }
}

/// `CommonMark` inline HTML avoids delimiter-run ambiguity at crossing mark boundaries.
/// Every segment is balanced and text/attributes are escaped, so HTML consumers use the same IR.
pub fn markdown(text: &str, spans: &[InlineSpan]) -> String {
    let mut boundaries = vec![0, text.len()];
    for span in spans {
        if span.start < span.end
            && span.end <= text.len()
            && text.is_char_boundary(span.start)
            && text.is_char_boundary(span.end)
        {
            boundaries.extend([span.start, span.end]);
        }
    }
    boundaries.sort_unstable();
    boundaries.dedup();
    let mut output = String::new();
    for range in boundaries.windows(2) {
        let [start, end] = range else {
            continue;
        };
        let (start, end) = (*start, *end);
        let active: Vec<_> = spans.iter().filter(|s| s.start <= start && s.end >= end).collect();
        let escape = |value: &str| {
            value
                .replace('&', "&amp;")
                .replace('<', "&lt;")
                .replace('>', "&gt;")
                .replace('"', "&quot;")
        };
        let mut segment = String::new();
        for ch in text[start..end].chars() {
            if ch.is_ascii_punctuation() {
                use std::fmt::Write;
                let _ = write!(&mut segment, "&#{};", u32::from(ch));
            } else {
                segment.push(ch);
            }
        }
        for (mark, tag) in [
            (InlineMark::Code, "code"),
            (InlineMark::Italic, "em"),
            (InlineMark::Bold, "strong"),
            (InlineMark::Strike, "s"),
        ] {
            if active.iter().any(|span| span.mark == mark) {
                segment = format!("<{tag}>{segment}</{tag}>");
            }
        }
        if let Some(href) = active
            .iter()
            .find(|span| span.mark == InlineMark::Link)
            .and_then(|span| span.href.as_deref())
            .filter(|href| safe_href(href))
        {
            segment = format!("<a href=\"{}\">{segment}</a>", escape(href));
        }
        output.push_str(&segment);
    }
    output
}

#[cfg(test)]
mod tests {
    use super::*;
    use loro::ContainerTrait;
    #[test]
    fn crossing_marks_render_as_balanced_equivalent_commonmark_html() {
        let spans = vec![
            InlineSpan {
                start: 0,
                end: 2,
                mark: InlineMark::Bold,
                href: None,
            },
            InlineSpan {
                start: 1,
                end: 3,
                mark: InlineMark::Italic,
                href: None,
            },
        ];
        assert_eq!(
            markdown("abc", &spans),
            "<strong>a</strong><strong><em>b</em></strong><em>c</em>"
        );
        let link = vec![InlineSpan {
            start: 0,
            end: 3,
            mark: InlineMark::Link,
            href: Some("https://x.test/?q=\"&".into()),
        }];
        assert_eq!(
            markdown("<&>", &link),
            "<a href=\"https://x.test/?q=&quot;&amp;\">&#60;&#38;&#62;</a>"
        );
    }
    #[test]
    fn literal_markdown_punctuation_is_not_reinterpreted_as_marks() {
        assert_eq!(
            markdown("* _ ` [ ] < &", &[]),
            "&#42; &#95; &#96; &#91; &#93; &#60; &#38;"
        );
    }
    #[test]
    fn rich_marks_utf8_overlap_clear_and_link_preserve_text_cid() {
        let doc = loro::LoroDoc::new();
        configure(&doc);
        let text = doc.get_text("text");
        text.insert_utf8(0, "A😀中文Z").unwrap();
        let cid = text.id();
        apply_edits(
            &text,
            &[
                MarkEdit {
                    start: 1,
                    end: 11,
                    mark: InlineMark::Bold,
                    clear: false,
                    href: None,
                },
                MarkEdit {
                    start: 5,
                    end: 11,
                    mark: InlineMark::Italic,
                    clear: false,
                    href: None,
                },
                MarkEdit {
                    start: 1,
                    end: 5,
                    mark: InlineMark::Link,
                    clear: false,
                    href: Some("https://example.test/a".into()),
                },
            ],
        )
        .unwrap();
        assert_eq!(text.to_string(), "A😀中文Z");
        assert_eq!(text.id(), cid);
        assert!(
            spans(&text)
                .unwrap()
                .iter()
                .any(|s| s.mark == InlineMark::Link && s.start == 1 && s.end == 5)
        );
        apply_edits(
            &text,
            &[MarkEdit {
                start: 5,
                end: 8,
                mark: InlineMark::Bold,
                clear: true,
                href: None,
            }],
        )
        .unwrap();
        assert!(
            !spans(&text)
                .unwrap()
                .iter()
                .any(|s| s.mark == InlineMark::Bold && s.start <= 5 && s.end >= 8)
        );
        let before = text.to_delta();
        assert!(
            apply_edits(
                &text,
                &[MarkEdit {
                    start: 2,
                    end: 5,
                    mark: InlineMark::Bold,
                    clear: false,
                    href: None
                }]
            )
            .is_err()
        );
        assert_eq!(text.to_delta(), before);
        assert!(!safe_href("javascript:alert(1)"));
        assert!(!safe_href("//other.test"));
        assert!(safe_href("/flow/page"));
    }
}
