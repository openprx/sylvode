//! Selected canonical block content only. No Page snapshot, aliases, ACL or private comments.
use collab_core::{
    NodeKind, SemanticNode, SemanticSnapshot,
    page_inline::{InlineMark, InlineSpan, safe_href},
};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use uuid::Uuid;
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum Content {
    Paragraph,
    Heading {
        level: u8,
    },
    Bullet,
    Ordered {
        order: u32,
    },
    Todo {
        checked: bool,
    },
    Quote,
    Toggle {
        collapsed: bool,
    },
    Code,
    Divider,
    Image {
        asset_id: Uuid,
        alt: String,
        caption: String,
    },
    Attachment {
        asset_id: Uuid,
        name: String,
        mime: String,
        caption: String,
    },
    ChildPage {
        child_page_id: Uuid,
    },
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Block {
    pub source_cid: String,
    pub parent_cid: Option<String>,
    pub preorder: u32,
    pub content: Content,
    pub text: String,
    pub spans: Vec<InlineSpan>,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    Unsupported,
    InvalidAttribute,
    InvalidMark,
    InvalidGraph,
    MissingRoot,
}
fn property<'a>(node: &'a SemanticNode, key: &str) -> Option<&'a str> {
    node.properties.get(key).map(String::as_str)
}
fn number(node: &SemanticNode, key: &str, default: u32) -> Result<u32, Error> {
    property(node, key).map_or(Ok(default), |value| value.parse().map_err(|_| Error::InvalidAttribute))
}
fn boolean(node: &SemanticNode, key: &str) -> Result<bool, Error> {
    match property(node, key) {
        None | Some("false") => Ok(false),
        Some("true") => Ok(true),
        _ => Err(Error::InvalidAttribute),
    }
}
fn reference(node: &SemanticNode, key: &str) -> Result<Uuid, Error> {
    property(node, key)
        .and_then(|value| Uuid::parse_str(value).ok())
        .filter(|id| !id.is_nil())
        .ok_or(Error::InvalidAttribute)
}
fn value(node: &SemanticNode, key: &str) -> String {
    property(node, key).unwrap_or_default().to_string()
}
pub fn read(snapshot: &SemanticSnapshot) -> Result<Vec<Block>, Error> {
    let mut nodes = snapshot
        .nodes
        .iter()
        .filter(|(_, node)| {
            !node.deleted && node.kind == NodeKind::Block && property(node, "content_source") == Some("editor_doc")
        })
        .collect::<Vec<_>>();
    nodes.sort_by(|(a, x), (b, y)| x.order_key.cmp(&y.order_key).then(a.cmp(b)));
    let mut out = Vec::new();
    for (index, (cid, node)) in nodes.into_iter().enumerate() {
        let content = match property(node, "block_type").unwrap_or("paragraph") {
            "paragraph" => Content::Paragraph,
            "heading" => {
                let level = u8::try_from(number(node, "level", 1)?).map_err(|_| Error::InvalidAttribute)?;
                if !(1..=6).contains(&level) {
                    return Err(Error::InvalidAttribute);
                }
                Content::Heading { level }
            }
            "bulletItem" => Content::Bullet,
            "orderedItem" => {
                let order = number(node, "order", 1)?;
                if order == 0 {
                    return Err(Error::InvalidAttribute);
                }
                Content::Ordered { order }
            }
            "todoItem" => Content::Todo {
                checked: boolean(node, "checked")?,
            },
            "blockquote" => Content::Quote,
            "toggle" => Content::Toggle {
                collapsed: boolean(node, "collapsed")?,
            },
            "codeBlock" => Content::Code,
            "divider" => Content::Divider,
            "image" => Content::Image {
                asset_id: reference(node, "asset_id")?,
                alt: value(node, "alt"),
                caption: value(node, "caption"),
            },
            "attachment" => Content::Attachment {
                asset_id: reference(node, "asset_id")?,
                name: value(node, "name"),
                mime: value(node, "mime"),
                caption: value(node, "caption"),
            },
            "childPage" => Content::ChildPage {
                child_page_id: reference(node, "child_page_id")?,
            },
            _ => return Err(Error::Unsupported),
        };
        let atom = matches!(
            content,
            Content::Divider | Content::Image { .. } | Content::Attachment { .. } | Content::ChildPage { .. }
        );
        let spans: Vec<InlineSpan> = property(node, "inline_spans")
            .map(serde_json::from_str)
            .transpose()
            .map_err(|_| Error::InvalidMark)?
            .unwrap_or_default();
        for span in &spans {
            if atom
                || span.start >= span.end
                || span.end > node.text.len()
                || !node.text.is_char_boundary(span.start)
                || !node.text.is_char_boundary(span.end)
                || (span.mark == InlineMark::Link && !span.href.as_deref().is_some_and(safe_href))
                || (span.mark != InlineMark::Link && span.href.is_some())
            {
                return Err(Error::InvalidMark);
            }
        }
        out.push(Block {
            source_cid: cid.to_string(),
            parent_cid: node.parent.as_ref().map(ToString::to_string),
            preorder: u32::try_from(index).map_err(|_| Error::InvalidGraph)?,
            content,
            text: if atom { String::new() } else { node.text.clone() },
            spans,
        });
    }
    let ids = out
        .iter()
        .map(|block| block.source_cid.as_str())
        .collect::<BTreeSet<_>>();
    if out
        .iter()
        .any(|block| block.parent_cid.as_deref().is_some_and(|parent| !ids.contains(parent)))
    {
        return Err(Error::InvalidGraph);
    }
    Ok(out)
}
pub fn select(blocks: &[Block], root: &str, subtree: bool) -> Result<Vec<Block>, Error> {
    let by_id = blocks
        .iter()
        .map(|block| (block.source_cid.as_str(), block))
        .collect::<BTreeMap<_, _>>();
    if !by_id.contains_key(root) {
        return Err(Error::MissingRoot);
    }
    let mut selected = Vec::new();
    for block in blocks {
        let mut cursor = Some(block.source_cid.as_str());
        let mut seen = BTreeSet::new();
        let mut include = false;
        while let Some(id) = cursor {
            if !seen.insert(id) {
                return Err(Error::InvalidGraph);
            }
            if id == root {
                include = true;
                break;
            }
            cursor = by_id.get(id).ok_or(Error::InvalidGraph)?.parent_cid.as_deref();
        }
        if include && (subtree || block.source_cid == root) {
            let mut block = block.clone();
            if block.source_cid == root {
                block.parent_cid = None;
            }
            selected.push(block);
        }
    }
    Ok(selected)
}
impl Block {
    pub const fn asset(&self) -> Option<Uuid> {
        match self.content {
            Content::Image { asset_id, .. } | Content::Attachment { asset_id, .. } => Some(asset_id),
            _ => None,
        }
    }
    pub fn remap_asset(&mut self, mapping: &BTreeMap<Uuid, Uuid>) -> Result<(), Error> {
        match &mut self.content {
            Content::Image { asset_id, .. } | Content::Attachment { asset_id, .. } => {
                *asset_id = *mapping.get(asset_id).ok_or(Error::InvalidAttribute)?;
            }
            _ => {}
        }
        Ok(())
    }
}
