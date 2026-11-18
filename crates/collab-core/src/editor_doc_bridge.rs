//! Canonical flat editor prose, using the installed loro-prosemirror 0.4.3 layout.
//! This bridge never recreates an existing block/text container and never writes tree prose.
use crate::CollabError;
use loro::{Container, ContainerTrait, LoroDoc, LoroList, LoroMap, LoroText, LoroValue, ToJson, ValueOrContainer};
use serde_json::Value;

#[derive(Debug, Clone)]
pub struct EditorBlock {
    pub id: String,
    pub kind: String,
    pub text: String,
    pub attributes: Value,
    pub logical_id: Option<String>,
    pub inline_spans: Vec<crate::page_inline::InlineSpan>,
    pub parent: Option<String>,
}
fn invalid(reason: &str) -> CollabError {
    CollabError::OperationFailed {
        reason: reason.to_owned(),
    }
}
fn string(map: &LoroMap, key: &str) -> Option<String> {
    match map.get(key) {
        Some(ValueOrContainer::Value(LoroValue::String(s))) => Some(s.to_string()),
        _ => None,
    }
}
fn children(map: &LoroMap) -> Option<LoroList> {
    match map.get("children") {
        Some(ValueOrContainer::Container(Container::List(l))) => Some(l),
        _ => None,
    }
}
fn root_children(doc: &LoroDoc) -> Option<LoroList> {
    children(&doc.get_map("doc"))
}
fn supported(kind: &str) -> bool {
    matches!(
        kind,
        "paragraph"
            | "heading"
            | "bulletItem"
            | "codeBlock"
            | "todoItem"
            | "orderedItem"
            | "blockquote"
            | "toggle"
            | "divider"
            | "image"
            | "attachment"
            | "childPage"
    )
}
fn text_children(map: &LoroMap) -> Result<Vec<LoroText>, CollabError> {
    let list = children(map).ok_or_else(|| invalid("editor_block_missing_children"))?;
    let mut texts = Vec::new();
    for i in 0..list.len() {
        match list.get(i) {
            Some(ValueOrContainer::Container(Container::Text(t))) => texts.push(t),
            _ => return Err(invalid("editor_block_unsupported_inline_container")),
        }
    }
    Ok(texts)
}
/// None means a legacy tree-only document. IDs are actual live map CIDs, not ordinals.
pub fn editor_blocks(doc: &LoroDoc) -> Result<Option<Vec<EditorBlock>>, CollabError> {
    read_blocks(doc, false)
}
pub fn legacy_editor_blocks(doc: &LoroDoc) -> Result<Option<Vec<EditorBlock>>, CollabError> {
    read_blocks(doc, true)
}
fn read_blocks(doc: &LoroDoc, legacy: bool) -> Result<Option<Vec<EditorBlock>>, CollabError> {
    let Some(list) = root_children(doc) else {
        return Ok(None);
    };
    let aliases = doc.get_map("flow_editor_aliases");
    if aliases.len() > crate::limits::DOCUMENT_BLOCK_COUNT_MAX {
        return Err(invalid("editor_block_alias_limit_exceeded"));
    }
    let mut reverse_aliases = std::collections::BTreeMap::new();
    for key in aliases.keys() {
        if let Some(cid) = string(&aliases, key.as_ref()) {
            reverse_aliases.entry(cid).or_insert_with(|| key.to_string());
        }
    }
    let placements = if legacy {
        None
    } else {
        crate::editor_structure::placements(doc)?
    };
    let mut blocks = Vec::new();
    for i in 0..list.len() {
        let Some(ValueOrContainer::Container(Container::Map(map))) = list.get(i) else {
            return Err(invalid("editor_doc_unsupported_block_container"));
        };
        if placements
            .as_ref()
            .is_some_and(|entries| !entries.iter().any(|entry| entry.cid == map.id().to_string()))
        {
            continue;
        }
        let kind = string(&map, "nodeName").ok_or_else(|| invalid("editor_block_missing_node_name"))?;
        // Non-prose dependency nodes must not expand private content into search/export.
        if !supported(&kind)
            || (legacy && !matches!(kind.as_str(), "paragraph" | "heading" | "bulletItem" | "codeBlock"))
        {
            continue;
        }
        let attributes = match map.get("attributes") {
            Some(ValueOrContainer::Container(Container::Map(attrs))) => attrs.get_deep_value().to_json_value(),
            _ => Value::Object(serde_json::Map::new()),
        };
        if !crate::editor_media::is_atom(&kind)
            && ["embed_type", "embed_object_id", "child_page_id", "reference_id"]
                .iter()
                .any(|key| attributes.get(*key).is_some())
        {
            continue;
        }
        let texts = text_children(&map)?;
        if crate::editor_media::is_atom(&kind) {
            crate::editor_media::validate(&kind, &attributes)?;
            if !texts.is_empty() {
                return Err(invalid("editor_atom_children_not_empty"));
            }
        }
        let text = if crate::editor_media::is_atom(&kind) {
            crate::editor_media::searchable_text(&kind, &attributes)
        } else {
            texts.iter().map(LoroText::to_string).collect::<String>()
        };
        let mut inline_spans = Vec::new();
        let mut offset = 0;
        for text in texts {
            for mut span in if legacy {
                Vec::new()
            } else {
                crate::page_inline::spans(&text)?
            } {
                span.start += offset;
                span.end += offset;
                inline_spans.push(span);
            }
            offset += text.to_string().len();
        }
        let logical_id = reverse_aliases.get(&map.id().to_string()).cloned();
        blocks.push(EditorBlock {
            id: map.id().to_string(),
            kind,
            text,
            attributes,
            logical_id,
            inline_spans,
            parent: None,
        });
    }
    if let Some(placements) = placements {
        let positions: std::collections::BTreeMap<_, _> = placements
            .iter()
            .enumerate()
            .map(|(index, entry)| (entry.cid.as_str(), index))
            .collect();
        blocks.sort_by_key(|block| positions.get(block.id.as_str()).copied().unwrap_or(usize::MAX));
        for block in &mut blocks {
            if let Some(entry) = placements.iter().find(|entry| entry.cid == block.id) {
                block.parent.clone_from(&entry.parent);
                block
                    .attributes
                    .as_object_mut()
                    .ok_or_else(|| invalid("editor_block_invalid_attributes"))?
                    .insert("indent".into(), serde_json::json!(entry.depth));
            }
        }
    }
    Ok(Some(blocks))
}
/// Resolve only live top-level editor maps. Tombstoned aliases cannot resurrect blocks.
pub fn resolve_editor_block_id(doc: &LoroDoc, id: &str) -> Option<String> {
    let list = root_children(doc)?;
    let canonical = string(&doc.get_map("flow_editor_aliases"), id).unwrap_or_else(|| id.to_owned());
    if crate::editor_structure::placements(doc)
        .ok()?
        .is_some_and(|entries| !entries.iter().any(|entry| entry.cid == canonical))
    {
        return None;
    }
    for i in 0..list.len() {
        if let Some(ValueOrContainer::Container(Container::Map(map))) = list.get(i)
            && map.id().to_string() == canonical
        {
            return Some(canonical);
        }
    }
    None
}

/// Apply one UTF-8 byte-indexed edit to an actual reachable editor block CID.
/// Returns false for non-editor IDs; caller must reject legacy shadow prose after initialization.
/// Both byte boundaries are validated before any mutation. Loro's explicit UTF-8 API performs
/// the coordinate translation; browser UTF-16 offsets must never be passed here as byte offsets.
pub fn apply_editor_text(
    doc: &LoroDoc,
    id: &str,
    index: u32,
    delete_len: Option<u32>,
    insert: Option<&str>,
) -> Result<bool, CollabError> {
    let Some(list) = root_children(doc) else {
        return Ok(false);
    };
    let Some(canonical_id) = resolve_editor_block_id(doc, id) else {
        return Ok(false);
    };
    let mut target = None;
    for i in 0..list.len() {
        if let Some(ValueOrContainer::Container(Container::Map(map))) = list.get(i)
            && map.id().to_string() == canonical_id
        {
            target = Some(map);
            break;
        }
    }
    let Some(map) = target else { return Ok(false) };
    let kind = string(&map, "nodeName").ok_or_else(|| invalid("editor_block_missing_node_name"))?;
    if !supported(&kind) || crate::editor_media::is_atom(&kind) || kind == "divider" {
        return Err(invalid("editor_block_unsupported_text_kind"));
    }
    if let Some(ValueOrContainer::Container(Container::Map(attrs))) = map.get("attributes")
        && ["embed_type", "embed_object_id", "child_page_id", "reference_id"]
            .iter()
            .any(|key| attrs.get(key).is_some())
    {
        return Err(invalid("editor_block_dependency_not_prose"));
    }
    if delete_len.is_some() == insert.is_some() {
        return Err(invalid("editor_text_requires_exactly_one_edit"));
    }
    let texts = text_children(&map)?;
    let value = texts.iter().map(LoroText::to_string).collect::<String>();
    let start = index as usize;
    let end = start
        .checked_add(delete_len.unwrap_or(0) as usize)
        .ok_or_else(|| invalid("editor_text_offset_out_of_bounds"))?;
    if end > value.len() || !value.is_char_boundary(start) || !value.is_char_boundary(end) {
        return Err(invalid("editor_text_invalid_utf8_boundary"));
    }
    for text in &texts {
        crate::page_inline::spans(text)?;
    }
    if let Some(value) = insert {
        if value.is_empty() {
            return Ok(true);
        }
        let mut offset = 0;
        for text in &texts {
            let until = offset + text.len_utf8();
            if start <= until {
                text.insert_utf8(start - offset, value)
                    .map_err(|e| invalid(&e.to_string()))?;
                return Ok(true);
            }
            offset = until;
        }
        children(&map)
            .ok_or_else(|| invalid("editor_block_missing_children"))?
            .insert_container(0, LoroText::new())
            .map_err(|e| invalid(&e.to_string()))?
            .insert_utf8(0, value)
            .map_err(|e| invalid(&e.to_string()))?;
    } else {
        let mut offset = 0;
        for text in &texts {
            let until = offset + text.len_utf8();
            let from = start.max(offset);
            let to = end.min(until);
            if from < to {
                text.delete_utf8(from - offset, to - from)
                    .map_err(|e| invalid(&e.to_string()))?;
            }
            offset = until;
        }
    }
    Ok(true)
}

/// Apply validated UTF-8 mark edits on the existing canonical text CID.
pub fn apply_editor_marks(doc: &LoroDoc, id: &str, edits: &[crate::page_inline::MarkEdit]) -> Result<(), CollabError> {
    let canonical = resolve_editor_block_id(doc, id).ok_or_else(|| invalid("editor_block_not_found"))?;
    let list = root_children(doc).ok_or_else(|| invalid("editor_doc_required"))?;
    for index in 0..list.len() {
        if let Some(ValueOrContainer::Container(Container::Map(map))) = list.get(index)
            && map.id().to_string() == canonical
        {
            if !string(&map, "nodeName").is_some_and(|kind| {
                supported(&kind) && kind != "codeBlock" && !crate::editor_media::is_atom(&kind) && kind != "divider"
            }) {
                return Err(invalid("editor_marks_not_allowed_on_block"));
            }
            let texts = text_children(&map)?;
            let value = texts.iter().map(LoroText::to_string).collect::<String>();
            crate::page_inline::validate_edits(&value, edits)?;
            for text in &texts {
                crate::page_inline::spans(text)?;
            }
            let mut offset = 0;
            for text in &texts {
                let end = offset + text.len_utf8();
                let local = edits
                    .iter()
                    .filter_map(|edit| {
                        let start = edit.start.max(offset);
                        let stop = edit.end.min(end);
                        (start < stop).then(|| crate::page_inline::MarkEdit {
                            start: start - offset,
                            end: stop - offset,
                            ..edit.clone()
                        })
                    })
                    .collect::<Vec<_>>();
                crate::page_inline::apply_edits(text, &local)?;
                offset = end;
            }
            return Ok(());
        }
    }
    Err(invalid("editor_block_not_found"))
}

#[derive(Debug, Clone)]
pub struct EditorInitialBlock {
    pub id: String,
    pub kind: String,
    pub text: String,
    pub indent: u32,
    pub level: Option<u32>,
}
/// One-time initialization only. Callers supply a validated legacy flat read model and
/// commit the private candidate atomically; existing editor state is never replaced.
pub fn initialize_editor_doc(doc: &LoroDoc, blocks: &[EditorInitialBlock]) -> Result<bool, CollabError> {
    let root = doc.get_map("doc");
    if root.get("children").is_some() {
        return Ok(false);
    }
    if string(&root, "nodeName").is_some_and(|kind| kind != "doc") {
        return Err(invalid("editor_doc_invalid_root"));
    }
    if root
        .get("attributes")
        .is_some_and(|value| !matches!(value, ValueOrContainer::Container(Container::Map(_))))
    {
        return Err(invalid("editor_doc_invalid_attributes"));
    }
    let aliases = doc.get_map("flow_editor_aliases");
    if blocks.len() > crate::limits::DOCUMENT_BLOCK_COUNT_MAX
        || aliases.len().saturating_add(blocks.len()) > crate::limits::DOCUMENT_BLOCK_COUNT_MAX
    {
        return Err(invalid("editor_block_alias_limit_exceeded"));
    }
    let mut ids = std::collections::BTreeSet::new();
    for block in blocks {
        if block.id.is_empty()
            || block.id.len() > 256
            || block.id.starts_with("cid:")
            || !ids.insert(block.id.as_str())
            || aliases.get(&block.id).is_some()
        {
            return Err(invalid("editor_block_invalid_or_duplicate_logical_id"));
        }
        if !supported(&block.kind)
            || block.indent > 7
            || block.text.chars().count() > crate::limits::TEXT_BLOCK_CHARS_MAX
        {
            return Err(invalid("editor_initial_block_unsupported"));
        }
        if (block.kind == "heading" && !block.level.is_some_and(|level| (1..=3).contains(&level)))
            || (block.kind != "heading" && block.level.is_some())
        {
            return Err(invalid("editor_block_invalid_attribute"));
        }
    }
    // Build detached containers so invalid input cannot leave an initialized half-document.
    let list = LoroList::new();
    let defaults = [EditorInitialBlock {
        id: String::new(),
        kind: "paragraph".into(),
        text: String::new(),
        indent: 0,
        level: None,
    }];
    let source = if blocks.is_empty() { defaults.as_slice() } else { blocks };
    for block in source {
        let map = LoroMap::new();
        map.insert("nodeName", block.kind.as_str())
            .map_err(|e| invalid(&e.to_string()))?;
        let attrs = map
            .insert_container("attributes", LoroMap::new())
            .map_err(|e| invalid(&e.to_string()))?;
        attrs
            .insert("indent", block.indent)
            .map_err(|e| invalid(&e.to_string()))?;
        if let Some(level) = block.level {
            attrs.insert("level", level).map_err(|e| invalid(&e.to_string()))?;
        }
        let inline = map
            .insert_container("children", LoroList::new())
            .map_err(|e| invalid(&e.to_string()))?;
        if !block.text.is_empty() {
            inline
                .insert_container(0, LoroText::new())
                .map_err(|e| invalid(&e.to_string()))?
                .insert_utf8(0, &block.text)
                .map_err(|e| invalid(&e.to_string()))?;
        }
        list.insert_container(list.len(), map)
            .map_err(|e| invalid(&e.to_string()))?;
    }
    let attached = root
        .insert_container("children", list)
        .map_err(|e| invalid(&e.to_string()))?;
    root.insert("nodeName", "doc").map_err(|e| invalid(&e.to_string()))?;
    if root.get("attributes").is_none() {
        root.insert_container("attributes", LoroMap::new())
            .map_err(|e| invalid(&e.to_string()))?;
    }
    for (index, block) in blocks.iter().enumerate() {
        let Some(ValueOrContainer::Container(Container::Map(map))) = attached.get(index) else {
            return Err(invalid("editor_initial_block_attach_failed"));
        };
        aliases
            .insert(&block.id, map.id().to_string())
            .map_err(|e| invalid(&e.to_string()))?;
    }
    Ok(true)
}

/// Structural edits are deliberately limited to the flat shipped schema. Existing map IDs
/// are retained; requested creation IDs are bounded aliases, never guessed from text/order.
pub fn apply_editor_structure(doc: &LoroDoc, operation: &crate::Operation) -> Result<bool, CollabError> {
    let Some(list) = root_children(doc) else {
        return Ok(false);
    };
    if let crate::Operation::CreateNode {
        id,
        parent,
        index,
        kind,
    } = operation
    {
        if *kind != crate::NodeKind::Block {
            return Ok(false);
        }
        let indexed = crate::editor_structure::ready(doc);
        if parent.is_some() && !indexed {
            return Err(invalid("editor_block_parent_must_be_root"));
        }
        let parent_cid = parent
            .as_ref()
            .map(|id| resolve_editor_block_id(doc, id).ok_or_else(|| invalid("editor_structure_parent_not_found")))
            .transpose()?;
        if indexed {
            let placements =
                crate::editor_structure::placements(doc)?.ok_or_else(|| invalid("editor_structure_required"))?;
            if parent_cid
                .as_ref()
                .is_some_and(|cid| placements.iter().any(|entry| entry.cid == *cid && entry.depth >= 7))
            {
                return Err(invalid("editor_structure_depth_limit"));
            }
            let count = placements
                .iter()
                .filter(|entry| entry.parent.as_deref() == parent_cid.as_deref())
                .count();
            if *index as usize > count {
                return Err(invalid("editor_block_index_out_of_bounds"));
            }
        }
        if id.is_empty() || id.len() > 256 || id.starts_with("cid:") {
            return Err(invalid("editor_block_invalid_logical_id"));
        }
        let aliases = doc.get_map("flow_editor_aliases");
        if aliases.get(id.as_ref()).is_some() {
            return Err(CollabError::DuplicateNode { id: id.to_string() });
        }
        if list.len() >= crate::limits::DOCUMENT_BLOCK_COUNT_MAX
            || aliases.len() >= crate::limits::DOCUMENT_BLOCK_COUNT_MAX
        {
            return Err(invalid("editor_block_alias_limit_exceeded"));
        }
        if *index as usize > list.len() {
            return Err(invalid("editor_block_index_out_of_bounds"));
        }
        let map = LoroMap::new();
        map.insert("nodeName", "paragraph")
            .map_err(|e| invalid(&e.to_string()))?;
        let attrs = map
            .insert_container("attributes", LoroMap::new())
            .map_err(|e| invalid(&e.to_string()))?;
        attrs.insert("indent", 0).map_err(|e| invalid(&e.to_string()))?;
        map.insert_container("children", LoroList::new())
            .map_err(|e| invalid(&e.to_string()))?;
        let attached = list
            .insert_container(if indexed { list.len() } else { *index as usize }, map)
            .map_err(|e| invalid(&e.to_string()))?;
        if indexed {
            crate::editor_structure::insert(doc, &attached.id().to_string(), parent_cid.as_deref(), *index as usize)?;
        }
        aliases
            .insert(id.as_ref(), attached.id().to_string())
            .map_err(|e| invalid(&e.to_string()))?;
        return Ok(true);
    }
    let (crate::Operation::DeleteNode { id }
    | crate::Operation::MoveNode { id, .. }
    | crate::Operation::SetProperty { id, .. }) = operation
    else {
        return Ok(false);
    };
    let canonical = string(&doc.get_map("flow_editor_aliases"), id.as_ref()).unwrap_or_else(|| id.to_string());
    let mut target = None;
    for i in 0..list.len() {
        if let Some(ValueOrContainer::Container(Container::Map(map))) = list.get(i)
            && map.id().to_string() == canonical
        {
            target = Some((i, map));
            break;
        }
    }
    let Some((position, map)) = target else {
        if doc.get_map("flow_editor_aliases").get(id.as_ref()).is_some() {
            return Err(CollabError::UnknownNode { id: id.to_string() });
        }
        return Ok(false);
    };
    if !string(&map, "nodeName").is_some_and(|kind| supported(&kind)) {
        return Err(invalid("editor_block_unsupported_kind"));
    }
    match operation {
        crate::Operation::DeleteNode { .. } => {
            if crate::editor_structure::ready(doc) {
                crate::editor_structure::delete(doc, &canonical)?;
                return Ok(true);
            }
            if list.len() == 1 {
                return Err(invalid("editor_doc_requires_one_block"));
            }
            list.delete(position, 1).map_err(|e| invalid(&e.to_string()))?;
        }
        crate::Operation::MoveNode { new_parent, index, .. } => {
            if crate::editor_structure::ready(doc) {
                let parent = new_parent
                    .as_ref()
                    .map(|id| {
                        resolve_editor_block_id(doc, id).ok_or_else(|| invalid("editor_structure_parent_not_found"))
                    })
                    .transpose()?;
                crate::editor_structure::move_block(doc, &canonical, parent.as_deref(), *index as usize)?;
                return Ok(true);
            }
            if new_parent.is_some() {
                return Err(invalid("editor_block_parent_must_be_root"));
            }
            if *index as usize != position {
                return Err(invalid("editor_block_identity_preserving_move_unsupported"));
            }
        }
        crate::Operation::SetProperty { key, value, .. } => {
            let Some(ValueOrContainer::Container(Container::Map(attrs))) = map.get("attributes") else {
                return Err(invalid("editor_block_missing_attributes"));
            };
            match key.as_str() {
                "block_type" => {
                    if !supported(value) || crate::editor_media::is_atom(value) {
                        return Err(invalid("editor_block_unsupported_kind"));
                    }
                    if value == "codeBlock"
                        && text_children(&map)?
                            .iter()
                            .any(|text| crate::page_inline::spans(text).is_ok_and(|spans| !spans.is_empty()))
                    {
                        return Err(invalid("editor_code_block_requires_unmarked_text"));
                    }
                    if value == "divider" && text_children(&map)?.iter().any(|text| !text.to_string().is_empty()) {
                        return Err(invalid("editor_divider_requires_empty_text"));
                    }
                    for (key, kind) in [
                        ("level", "heading"),
                        ("checked", "todoItem"),
                        ("order", "orderedItem"),
                        ("collapsed", "toggle"),
                    ] {
                        if value != kind {
                            attrs.delete(key).map_err(|e| invalid(&e.to_string()))?;
                        }
                    }
                    match value.as_str() {
                        "heading" => {
                            attrs.insert("level", 1).map_err(|e| invalid(&e.to_string()))?;
                        }
                        "todoItem" => {
                            attrs.insert("checked", false).map_err(|e| invalid(&e.to_string()))?;
                        }
                        "orderedItem" => {
                            attrs.insert("order", 1).map_err(|e| invalid(&e.to_string()))?;
                        }
                        "toggle" => {
                            attrs.insert("collapsed", false).map_err(|e| invalid(&e.to_string()))?;
                        }
                        _ => {}
                    }
                    map.insert("nodeName", value.as_str())
                        .map_err(|e| invalid(&e.to_string()))?;
                }
                "alt" | "caption" | "name" | "mime"
                    if string(&map, "nodeName")
                        .is_some_and(|kind| crate::editor_media::is_atom(&kind) && kind != "childPage") =>
                {
                    let max = match key.as_str() {
                        "name" => 255,
                        "mime" => 128,
                        _ => 4096,
                    };
                    if value.len() > max {
                        return Err(invalid("editor_atom_invalid_text_attribute"));
                    }
                    attrs
                        .insert(key, value.as_str())
                        .map_err(|error| invalid(&error.to_string()))?;
                }
                "checked" | "collapsed" => {
                    let kind = string(&map, "nodeName");
                    if (key == "checked" && kind.as_deref() != Some("todoItem"))
                        || (key == "collapsed" && kind.as_deref() != Some("toggle"))
                    {
                        return Err(invalid("editor_block_invalid_attribute"));
                    }
                    let value = value
                        .parse::<bool>()
                        .map_err(|_| invalid("editor_block_invalid_attribute"))?;
                    attrs.insert(key, value).map_err(|e| invalid(&e.to_string()))?;
                }
                "order" => {
                    let value = value
                        .parse::<u32>()
                        .map_err(|_| invalid("editor_block_invalid_attribute"))?;
                    if value == 0 || value > 1_000_000 || string(&map, "nodeName").as_deref() != Some("orderedItem") {
                        return Err(invalid("editor_block_invalid_attribute"));
                    }
                    attrs.insert(key, value).map_err(|e| invalid(&e.to_string()))?;
                }
                "indent" if crate::editor_structure::ready(doc) => {
                    return Err(invalid("editor_structure_requires_move_for_indent"));
                }
                "indent" | "level" => {
                    let number = value
                        .parse::<i32>()
                        .map_err(|_| invalid("editor_block_invalid_attribute"))?;
                    if (key == "indent" && !(0..=7).contains(&number))
                        || (key == "level"
                            && (!(1..=3).contains(&number) || string(&map, "nodeName").as_deref() != Some("heading")))
                    {
                        return Err(invalid("editor_block_invalid_attribute"));
                    }
                    attrs.insert(key, number).map_err(|e| invalid(&e.to_string()))?;
                }
                _ => return Err(invalid("editor_block_unsupported_property")),
            }
        }
        _ => return Ok(false),
    }
    Ok(true)
}

/// Server-side atomic typed body insertion. The candidate is exported as one accepted update.
pub fn insert_atom(
    doc: &LoroDoc,
    id: &str,
    parent: Option<&str>,
    index: usize,
    kind: &str,
    attributes: &Value,
) -> Result<String, CollabError> {
    crate::editor_media::validate(kind, attributes)?;
    if id.is_empty() || id.len() > 256 || id.starts_with("cid:") || doc.get_map("flow_editor_aliases").get(id).is_some()
    {
        return Err(invalid("editor_block_invalid_or_duplicate_logical_id"));
    }
    let list = root_children(doc).ok_or_else(|| invalid("editor_doc_required"))?;
    let placements = crate::editor_structure::placements(doc)?.ok_or_else(|| invalid("editor_structure_required"))?;
    let parent = parent
        .map(|id| resolve_editor_block_id(doc, id).ok_or_else(|| invalid("editor_structure_parent_not_found")))
        .transpose()?;
    let count = placements
        .iter()
        .filter(|placement| placement.parent.as_deref() == parent.as_deref())
        .count();
    if index > count
        || list.len() >= crate::limits::DOCUMENT_BLOCK_COUNT_MAX
        || parent.as_ref().is_some_and(|cid| {
            placements
                .iter()
                .any(|placement| placement.cid == *cid && placement.depth >= 7)
        })
    {
        return Err(invalid("editor_atom_placement_out_of_bounds"));
    }
    let map = LoroMap::new();
    map.insert("nodeName", kind)
        .map_err(|error| invalid(&error.to_string()))?;
    let attrs = map
        .insert_container("attributes", LoroMap::new())
        .map_err(|error| invalid(&error.to_string()))?;
    for (key, value) in attributes
        .as_object()
        .ok_or_else(|| invalid("editor_atom_invalid_attributes"))?
    {
        let value: LoroValue = serde_json::from_value(value.clone()).map_err(|error| invalid(&error.to_string()))?;
        attrs.insert(key, value).map_err(|error| invalid(&error.to_string()))?;
    }
    map.insert_container("children", LoroList::new())
        .map_err(|error| invalid(&error.to_string()))?;
    let attached = list
        .insert_container(list.len(), map)
        .map_err(|error| invalid(&error.to_string()))?;
    let cid = attached.id().to_string();
    crate::editor_structure::insert(doc, &cid, parent.as_deref(), index)?;
    doc.get_map("flow_editor_aliases")
        .insert(id, cid.clone())
        .map_err(|error| invalid(&error.to_string()))?;
    doc.get_map(crate::editor_structure::META)
        .insert("model", "rich_page_v5")
        .map_err(|error| invalid(&error.to_string()))?;
    Ok(cid)
}

pub fn replace_atom(doc: &LoroDoc, id: &str, kind: &str, attributes: &Value) -> Result<(), CollabError> {
    crate::editor_media::validate(kind, attributes)?;
    if kind == "childPage" {
        return Err(invalid("editor_asset_replacement_kind"));
    }
    let cid = resolve_editor_block_id(doc, id).ok_or_else(|| invalid("editor_block_not_found"))?;
    let list = root_children(doc).ok_or_else(|| invalid("editor_doc_required"))?;
    for index in 0..list.len() {
        if let Some(ValueOrContainer::Container(Container::Map(map))) = list.get(index)
            && map.id().to_string() == cid
        {
            if !string(&map, "nodeName").is_some_and(|kind| matches!(kind.as_str(), "image" | "attachment")) {
                return Err(invalid("editor_replace_requires_media_block"));
            }
            let Some(ValueOrContainer::Container(Container::Map(attrs))) = map.get("attributes") else {
                return Err(invalid("editor_atom_invalid_attributes"));
            };
            let keys: Vec<_> = attrs.keys().map(|key| key.to_string()).collect();
            for key in keys {
                attrs.delete(&key).map_err(|error| invalid(&error.to_string()))?;
            }
            for (key, value) in attributes
                .as_object()
                .ok_or_else(|| invalid("editor_atom_invalid_attributes"))?
            {
                let value: LoroValue =
                    serde_json::from_value(value.clone()).map_err(|error| invalid(&error.to_string()))?;
                attrs.insert(key, value).map_err(|error| invalid(&error.to_string()))?;
            }
            map.insert("nodeName", kind)
                .map_err(|error| invalid(&error.to_string()))?;
            return Ok(());
        }
    }
    Err(invalid("editor_block_not_found"))
}

#[cfg(test)]
mod tests {
    use super::*;
    const SNAPSHOT: &[u8] = include_bytes!("../../../frontend/tests/fixtures/editor-doc-v1/browser.snapshot");
    fn fixture() -> LoroDoc {
        let doc = LoroDoc::new();
        doc.import(SNAPSHOT).expect("real JS library snapshot");
        doc
    }
    #[test]
    fn real_js_snapshot_reader_and_utf8_edits_preserve_cids() {
        let doc = fixture();
        let before = editor_blocks(&doc).unwrap().unwrap();
        let first = before.first().unwrap();
        assert_eq!(first.text, "Browser 😀 prose");
        assert!(apply_editor_text(&doc, &first.id, 12, None, Some(" agent")).unwrap());
        assert_eq!(
            editor_blocks(&doc).unwrap().unwrap().first().unwrap().text,
            "Browser 😀 agent prose"
        );
        assert!(apply_editor_text(&doc, &first.id, 12, Some(6), None).unwrap());
        let after = editor_blocks(&doc).unwrap().unwrap();
        assert_eq!(after.first().unwrap().text, first.text);
        assert_eq!(
            before.iter().map(|b| &b.id).collect::<Vec<_>>(),
            after.iter().map(|b| &b.id).collect::<Vec<_>>()
        );
    }
    #[test]
    fn canonical_text_and_marks_cross_existing_text_container_boundaries() {
        use crate::page_inline::{InlineMark, MarkEdit};
        let doc = fixture();
        crate::page_inline::configure(&doc);
        let list = root_children(&doc).unwrap();
        let Some(ValueOrContainer::Container(Container::Map(root))) = list.get(0) else {
            panic!("fixture root block missing");
        };
        let id = root.id().to_string();
        let text_list = children(&root).unwrap();
        let suffix = text_list.insert_container(text_list.len(), LoroText::new()).unwrap();
        suffix.insert_utf8(0, "中😀尾").unwrap();
        let before = editor_blocks(&doc).unwrap().unwrap().remove(0);
        let split = "Browser 😀 prose".len();
        let mark = |start, end, clear| MarkEdit {
            start,
            end,
            mark: InlineMark::Italic,
            clear,
            href: None,
        };
        assert!(
            apply_editor_marks(
                &doc,
                &id,
                &[mark(split - 2, split + 3, false), mark(split + 1, split + 3, false)]
            )
            .is_err()
        );
        assert_eq!(
            editor_blocks(&doc).unwrap().unwrap().first().unwrap().inline_spans,
            before.inline_spans
        );
        apply_editor_marks(&doc, &id, &[mark(split - 2, split + 7, false)]).unwrap();
        let marked = editor_blocks(&doc).unwrap().unwrap().remove(0);
        assert_eq!(
            marked
                .inline_spans
                .iter()
                .map(|span| (span.start, span.end))
                .collect::<Vec<_>>(),
            vec![(split - 2, split), (split, split + 7)]
        );
        apply_editor_marks(&doc, &id, &[mark(split - 2, split + 7, true)]).unwrap();
        assert!(
            editor_blocks(&doc)
                .unwrap()
                .unwrap()
                .first()
                .unwrap()
                .inline_spans
                .is_empty()
        );
        assert!(apply_editor_text(&doc, &id, u32::try_from(split - 2).unwrap(), Some(5), None).unwrap());
        assert!(apply_editor_text(&doc, &id, u32::try_from(split - 2).unwrap(), None, Some("eé")).unwrap());
        let after = editor_blocks(&doc).unwrap().unwrap().remove(0);
        assert_eq!(after.id, before.id);
        assert_eq!(after.text, "Browser 😀 proeé😀尾");
        assert_eq!(text_children(&root).unwrap().len(), 2);
    }

    #[test]
    fn rust_output_fixture_preserves_js_ids_and_carries_agent_edit() {
        let doc = fixture();
        doc.set_peer_id(72001).unwrap();
        let vv = doc.oplog_vv();
        let id = editor_blocks(&doc).unwrap().unwrap().first().unwrap().id.clone();
        assert!(apply_editor_text(&doc, &id, 12, None, Some(" + Rust agent")).unwrap());
        doc.commit();
        if std::env::var("WRITE_EDITOR_INTEROP_FIXTURE").as_deref() == Ok("1") {
            let base =
                std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../frontend/tests/fixtures/editor-doc-v1");
            std::fs::write(
                base.join("rust-agent.update"),
                doc.export(loro::ExportMode::updates(&vv)).unwrap(),
            )
            .unwrap();
            std::fs::write(
                base.join("rust-agent.snapshot"),
                doc.export(loro::ExportMode::Snapshot).unwrap(),
            )
            .unwrap();
        }
        assert_eq!(
            editor_blocks(&doc).unwrap().unwrap().first().unwrap().text,
            "Browser 😀 + Rust agent prose"
        );
    }
    #[test]
    fn structural_creation_alias_and_deletion_keep_existing_identity() {
        let doc = fixture();
        let before = editor_blocks(&doc).unwrap().unwrap();
        let create = crate::Operation::CreateNode {
            id: "agent-block".into(),
            parent: None,
            index: 1,
            kind: crate::NodeKind::Block,
        };
        assert!(apply_editor_structure(&doc, &create).unwrap());
        assert!(apply_editor_text(&doc, "agent-block", 0, None, Some("Agent created 😀")).unwrap());
        let blocks = editor_blocks(&doc).unwrap().unwrap();
        let created = blocks
            .iter()
            .find(|b| b.logical_id.as_deref() == Some("agent-block"))
            .unwrap();
        assert_eq!(created.text, "Agent created 😀");
        assert!(created.id.starts_with("cid:"));
        assert_eq!(blocks.first().unwrap().id, before.first().unwrap().id);
        assert!(
            apply_editor_structure(
                &doc,
                &crate::Operation::SetProperty {
                    id: "agent-block".into(),
                    key: "block_type".into(),
                    value: "heading".into()
                }
            )
            .unwrap()
        );
        assert!(
            apply_editor_structure(
                &doc,
                &crate::Operation::SetProperty {
                    id: "agent-block".into(),
                    key: "level".into(),
                    value: "2".into()
                }
            )
            .unwrap()
        );
        let frontier = doc.oplog_vv();
        assert!(
            apply_editor_structure(
                &doc,
                &crate::Operation::MoveNode {
                    id: "agent-block".into(),
                    new_parent: None,
                    index: 0
                }
            )
            .is_err()
        );
        assert_eq!(doc.oplog_vv(), frontier);
        assert!(
            apply_editor_structure(
                &doc,
                &crate::Operation::DeleteNode {
                    id: "agent-block".into()
                }
            )
            .unwrap()
        );
        assert_eq!(editor_blocks(&doc).unwrap().unwrap().len(), before.len());
        assert!(apply_editor_structure(&doc, &create).is_err());
        assert!(!apply_editor_text(&doc, "agent-block", 0, None, Some("resurrect")).unwrap());
        if std::env::var("WRITE_EDITOR_INTEROP_FIXTURE").as_deref() == Ok("1") {
            // A second fresh fixture captures the positive create/alias/edit shape for JS.
            let fresh = fixture();
            apply_editor_structure(&fresh, &create).unwrap();
            apply_editor_text(&fresh, "agent-block", 0, None, Some("Agent created 😀")).unwrap();
            fresh.commit();
            let base =
                std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../frontend/tests/fixtures/editor-doc-v1");
            std::fs::write(
                base.join("rust-created.snapshot"),
                fresh.export(loro::ExportMode::Snapshot).unwrap(),
            )
            .unwrap();
        }
    }
    #[test]
    fn unknown_marks_and_private_dependency_attrs_fail_before_text_mutation() {
        let doc = fixture();
        let list = root_children(&doc).unwrap();
        let Some(ValueOrContainer::Container(Container::Map(map))) = list.get(0) else {
            panic!("fixture map")
        };
        let text = text_children(&map).unwrap().remove(0);
        let mut styles = loro::StyleConfigMap::new();
        styles.insert(
            "unknown_future_mark".into(),
            loro::StyleConfig {
                expand: loro::ExpandType::After,
            },
        );
        doc.config_text_style(styles);
        text.mark(0..1, "unknown_future_mark", true).unwrap();
        doc.commit();
        let before = doc.oplog_vv();
        assert!(apply_editor_text(&doc, &map.id().to_string(), 0, None, Some("bad")).is_err());
        assert_eq!(doc.oplog_vv(), before);
        let doc = fixture();
        let list = root_children(&doc).unwrap();
        let Some(ValueOrContainer::Container(Container::Map(map))) = list.get(0) else {
            panic!("fixture map")
        };
        let Some(ValueOrContainer::Container(Container::Map(attrs))) = map.get("attributes") else {
            panic!("fixture attrs")
        };
        attrs.insert("embed_object_id", "private").unwrap();
        doc.commit();
        let before = doc.oplog_vv();
        assert!(apply_editor_text(&doc, &map.id().to_string(), 0, None, Some("bad")).is_err());
        assert_eq!(doc.oplog_vv(), before);
        assert_eq!(editor_blocks(&doc).unwrap().unwrap().len(), 3);
    }
    #[test]
    fn initialization_validates_all_input_and_never_overwrites_an_existing_doc() {
        let doc = LoroDoc::new();
        let bad = EditorInitialBlock {
            id: "legacy-id".into(),
            kind: "unknown".into(),
            text: "legacy".into(),
            indent: 0,
            level: None,
        };
        let frontier = doc.oplog_vv();
        assert!(initialize_editor_doc(&doc, std::slice::from_ref(&bad)).is_err());
        assert_eq!(doc.oplog_vv(), frontier);
        assert!(root_children(&doc).is_none());
        let good = EditorInitialBlock {
            kind: "paragraph".into(),
            ..bad
        };
        assert!(initialize_editor_doc(&doc, &[good]).unwrap());
        let blocks = editor_blocks(&doc).unwrap().unwrap();
        assert_eq!(blocks.first().unwrap().text, "legacy");
        assert_eq!(blocks.first().unwrap().logical_id.as_deref(), Some("legacy-id"));
        let frontier = doc.oplog_vv();
        assert!(!initialize_editor_doc(&doc, &[]).unwrap());
        assert_eq!(doc.oplog_vv(), frontier);
        assert!(apply_editor_text(&doc, "legacy-id", 6, None, Some(" + agent")).unwrap());
        assert_eq!(
            editor_blocks(&doc).unwrap().unwrap().first().unwrap().text,
            "legacy + agent"
        );
        let fresh = LoroDoc::new();
        assert!(initialize_editor_doc(&fresh, &[]).unwrap());
        assert_eq!(editor_blocks(&fresh).unwrap().unwrap().len(), 1);
        if std::env::var("WRITE_EDITOR_INTEROP_FIXTURE").as_deref() == Ok("1") {
            let base =
                std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../frontend/tests/fixtures/editor-doc-v1");
            std::fs::write(
                base.join("rust-initialized.snapshot"),
                doc.export(loro::ExportMode::Snapshot).unwrap(),
            )
            .unwrap();
            std::fs::write(
                base.join("rust-empty.snapshot"),
                fresh.export(loro::ExportMode::Snapshot).unwrap(),
            )
            .unwrap();
        }
    }
    #[test]
    fn rich_snapshot_roundtrip_and_frozen_legacy_fingerprints() {
        use crate::CollabEngine;
        let mut engine = crate::LoroCollabEngine::load(SNAPSHOT).unwrap();
        let old = engine.legacy_package_semantic_snapshot_v3().unwrap();
        let current = engine.semantic_snapshot().unwrap();
        assert_eq!(old, current);
        let id = current
            .nodes
            .iter()
            .find(|(_, node)| node.text == "Browser 😀 prose")
            .unwrap()
            .0
            .clone();
        engine
            .apply_page_marks(
                &id,
                &[crate::page_inline::MarkEdit {
                    start: 0,
                    end: 7,
                    mark: crate::page_inline::InlineMark::Bold,
                    clear: false,
                    href: None,
                }],
            )
            .unwrap();
        let rich = engine.semantic_snapshot().unwrap();
        assert_ne!(rich.semantic_hash(), old.semantic_hash());
        assert_eq!(engine.legacy_package_semantic_snapshot_v3().unwrap(), old);
        let loaded = crate::LoroCollabEngine::load(&engine.export_snapshot().unwrap()).unwrap();
        assert_eq!(loaded.semantic_snapshot().unwrap(), rich);
        assert_eq!(loaded.resolve_content_node_id(&id), id);
        engine
            .apply_operation(&crate::Operation::InsertText {
                id: id.clone(),
                index: 7,
                text: " new".into(),
            })
            .unwrap();
        assert!(
            engine
                .semantic_snapshot()
                .unwrap()
                .nodes
                .get(&id)
                .unwrap()
                .properties
                .contains_key("inline_spans")
        );
    }
    #[test]
    fn invalid_unicode_boundaries_and_unknown_ids_leave_frontier_unchanged() {
        let doc = fixture();
        let blocks = editor_blocks(&doc).unwrap().unwrap();
        let id = &blocks.first().unwrap().id;
        let before = doc.oplog_vv();
        assert!(apply_editor_text(&doc, id, 9, None, Some("bad")).is_err());
        assert!(apply_editor_text(&doc, id, 8, Some(1), None).is_err());
        assert!(!apply_editor_text(&doc, "tree-block", 0, None, Some("bad")).unwrap());
        assert_eq!(doc.oplog_vv(), before);
    }
}
