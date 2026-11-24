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
fn root_children(doc: &LoroDoc) -> Result<Option<LoroList>, CollabError> {
    Ok(crate::editor_structure::root_map(doc, crate::reserved_roots::DOC)?.and_then(|root| children(&root)))
}
/// The alias map, read without registering it: `None` when the document has none.
fn aliases_map(doc: &LoroDoc) -> Result<Option<LoroMap>, CollabError> {
    crate::editor_structure::root_map(doc, crate::reserved_roots::ALIASES)
}
/// Whether the alias map holds `id` (tombstones included). A read: registers nothing.
pub fn has_alias_entry(doc: &LoroDoc, id: &str) -> Result<bool, CollabError> {
    Ok(aliases_map(doc)?.is_some_and(|aliases| aliases.get(id).is_some()))
}
fn alias_target(doc: &LoroDoc, id: &str) -> Result<Option<String>, CollabError> {
    Ok(aliases_map(doc)?.and_then(|aliases| string(&aliases, id)))
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
/// Whether the block reader converts the inline texts of this block (`page_inline::spans`): the
/// kind is supported and, unless it is an atom, the block is not a dependency node. The same
/// conditions as [`current_read`], without building the attributes.
pub fn reads_inline_marks(map: &LoroMap) -> bool {
    let Some(kind) = string(map, "nodeName") else {
        return false;
    };
    if !supported(&kind) {
        return false;
    }
    if crate::editor_media::is_atom(&kind) {
        return true;
    }
    match map.get("attributes") {
        Some(ValueOrContainer::Container(Container::Map(attributes))) => {
            ["embed_type", "embed_object_id", "child_page_id", "reference_id"]
                .iter()
                .all(|key| attributes.get(key).is_none())
        }
        _ => true,
    }
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
/// Constant-time lookups over a placement list, answering exactly what the linear scans
/// `entries.iter().any(..)`, `entries.iter().find(..)` and a `BTreeMap` collected from
/// `entries.iter().enumerate()` answer, including when a CID occurs more than once: `find` yields
/// the first entry for a CID, the collected position map keeps the last index.
struct PlacementIndex<'a> {
    first: std::collections::HashMap<&'a str, &'a crate::editor_structure::Placement>,
    last_position: std::collections::HashMap<&'a str, usize>,
}
impl<'a> PlacementIndex<'a> {
    fn new(entries: &'a [crate::editor_structure::Placement]) -> Self {
        let mut first = std::collections::HashMap::with_capacity(entries.len());
        let mut last_position = std::collections::HashMap::with_capacity(entries.len());
        for (index, entry) in entries.iter().enumerate() {
            first.entry(entry.cid.as_str()).or_insert(entry);
            last_position.insert(entry.cid.as_str(), index);
        }
        Self { first, last_position }
    }
    fn contains(&self, cid: &str) -> bool {
        self.first.contains_key(cid)
    }
    fn first(&self, cid: &str) -> Option<&'a crate::editor_structure::Placement> {
        self.first.get(cid).copied()
    }
    fn position(&self, cid: &str) -> Option<usize> {
        self.last_position.get(cid).copied()
    }
}
/// None means a legacy tree-only document. IDs are actual live map CIDs, not ordinals.
pub fn editor_blocks(doc: &LoroDoc) -> Result<Option<Vec<EditorBlock>>, CollabError> {
    read_blocks(doc, Mode::Current, || crate::editor_structure::placements(doc))
}
pub fn legacy_editor_blocks(doc: &LoroDoc) -> Result<Option<Vec<EditorBlock>>, CollabError> {
    read_blocks(doc, Mode::Legacy, || Ok(None))
}

/// What the editor-model floor needs to know about the blocks [`editor_blocks`] would return.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct EditorModelSignals {
    /// Some block is a model-5 atom (`image`, `attachment`, `childPage`).
    pub atom: bool,
    /// Some block carries inline marks or is not one of the model-1 prose kinds.
    pub beyond_model1: bool,
}

/// [`editor_blocks`] reduced to [`EditorModelSignals`], for `minimum_editor_model`.
///
/// `placements` must be what `editor_structure::placements(doc)` returns for this unchanged
/// document (the caller has already evaluated it, and its error, for the same state). Visits the
/// same blocks in the same order and fails with exactly the error `editor_blocks` would, but skips
/// work that only shapes the returned values: block text, logical-id aliases.
pub fn editor_model_signals(
    doc: &LoroDoc,
    placements: Option<Vec<crate::editor_structure::Placement>>,
) -> Result<Option<EditorModelSignals>, CollabError> {
    let Some(blocks) = read_blocks(doc, Mode::Floor, || Ok(placements))? else {
        return Ok(None);
    };
    Ok(Some(signals_of(&blocks)))
}

fn signals_of(blocks: &[EditorBlock]) -> EditorModelSignals {
    EditorModelSignals {
        atom: blocks.iter().any(|block| crate::editor_media::is_atom(&block.kind)),
        beyond_model1: blocks.iter().any(|block| {
            !block.inline_spans.is_empty()
                || !matches!(
                    block.kind.as_str(),
                    "paragraph" | "heading" | "bulletItem" | "codeBlock"
                )
        }),
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Mode {
    /// Package v2/v3 prose: model-1 kinds only, no structure, no inline marks.
    Legacy,
    /// The current reader.
    Current,
    /// `Current`'s filtering and validation, without block text or logical ids.
    Floor,
}

fn is_legacy_kind(kind: &str) -> bool {
    matches!(kind, "paragraph" | "heading" | "bulletItem" | "codeBlock")
}
fn block_attributes(map: &LoroMap) -> Value {
    match map.get("attributes") {
        Some(ValueOrContainer::Container(Container::Map(attrs))) => attrs.get_deep_value().to_json_value(),
        _ => Value::Object(serde_json::Map::new()),
    }
}
/// Non-prose dependency nodes must not expand private content into search/export.
fn is_dependency(attributes: &Value) -> bool {
    ["embed_type", "embed_object_id", "child_page_id", "reference_id"]
        .iter()
        .any(|key| attributes.get(*key).is_some())
}
fn reverse_aliases(aliases: Option<&LoroMap>) -> std::collections::BTreeMap<String, String> {
    let mut reverse = std::collections::BTreeMap::new();
    let Some(aliases) = aliases else {
        return reverse;
    };
    for key in aliases.keys() {
        if let Some(cid) = string(aliases, key.as_ref()) {
            reverse.entry(cid).or_insert_with(|| key.to_string());
        }
    }
    reverse
}
/// The tail of a non-legacy block read, after kind and dependency filtering and once the inline
/// text containers are known: atom validation, text, inline spans, logical id.
fn finish_block(
    cid: String,
    kind: String,
    attributes: Value,
    texts: &[LoroText],
    collect_values: bool,
    reverse_aliases: &std::collections::BTreeMap<String, String>,
) -> Result<EditorBlock, CollabError> {
    let atom = crate::editor_media::is_atom(&kind);
    if atom {
        crate::editor_media::validate(&kind, &attributes)?;
        if !texts.is_empty() {
            return Err(invalid("editor_atom_children_not_empty"));
        }
    }
    // Each inline text is materialized once; its byte length is the next span offset.
    let strings: Vec<String> = if collect_values {
        texts.iter().map(LoroText::to_string).collect()
    } else {
        Vec::new()
    };
    let text = if !collect_values {
        String::new()
    } else if atom {
        crate::editor_media::searchable_text(&kind, &attributes)
    } else {
        strings.concat()
    };
    let mut inline_spans = Vec::new();
    let mut offset = 0;
    for (position, text) in texts.iter().enumerate() {
        for mut span in crate::page_inline::spans(text)? {
            span.start += offset;
            span.end += offset;
            inline_spans.push(span);
        }
        offset += strings.get(position).map_or(0, String::len);
    }
    let logical_id = if collect_values {
        reverse_aliases.get(&cid).cloned()
    } else {
        None
    };
    Ok(EditorBlock {
        id: cid,
        kind,
        text,
        attributes,
        logical_id,
        inline_spans,
        parent: None,
    })
}
/// Orders blocks by placement and attaches parent and depth, as structured pages require.
fn apply_placements(blocks: &mut [EditorBlock], index: &PlacementIndex<'_>) -> Result<(), CollabError> {
    blocks.sort_by_key(|block| index.position(&block.id).unwrap_or(usize::MAX));
    for block in blocks.iter_mut() {
        if let Some(entry) = index.first(&block.id) {
            block.parent.clone_from(&entry.parent);
            block
                .attributes
                .as_object_mut()
                .ok_or_else(|| invalid("editor_block_invalid_attributes"))?
                .insert("indent".into(), serde_json::json!(entry.depth));
        }
    }
    Ok(())
}
fn read_blocks(
    doc: &LoroDoc,
    mode: Mode,
    load_placements: impl FnOnce() -> Result<Option<Vec<crate::editor_structure::Placement>>, CollabError>,
) -> Result<Option<Vec<EditorBlock>>, CollabError> {
    #[cfg(test)]
    crate::walk_counter::record_block_read();
    let legacy = mode == Mode::Legacy;
    let collect_values = mode != Mode::Floor;
    let Some(list) = root_children(doc)? else {
        return Ok(None);
    };
    let aliases = aliases_map(doc)?;
    if aliases
        .as_ref()
        .is_some_and(|aliases| aliases.len() > crate::limits::DOCUMENT_BLOCK_COUNT_MAX)
    {
        return Err(invalid("editor_block_alias_limit_exceeded"));
    }
    let reverse_aliases = if collect_values {
        reverse_aliases(aliases.as_ref())
    } else {
        std::collections::BTreeMap::new()
    };
    let placements = load_placements()?;
    let index = placements.as_deref().map(PlacementIndex::new);
    let mut blocks = Vec::new();
    for i in 0..list.len() {
        let Some(ValueOrContainer::Container(Container::Map(map))) = list.get(i) else {
            return Err(invalid("editor_doc_unsupported_block_container"));
        };
        let cid = map.id().to_string();
        if index.as_ref().is_some_and(|index| !index.contains(&cid)) {
            continue;
        }
        let kind = string(&map, "nodeName").ok_or_else(|| invalid("editor_block_missing_node_name"))?;
        if !supported(&kind) || (legacy && !is_legacy_kind(&kind)) {
            continue;
        }
        let attributes = block_attributes(&map);
        if !crate::editor_media::is_atom(&kind) && is_dependency(&attributes) {
            continue;
        }
        let texts = text_children(&map)?;
        if legacy {
            // Legacy prose never carries atoms (filtered above) and never reads inline marks.
            let text = texts.iter().map(LoroText::to_string).collect::<String>();
            let logical_id = reverse_aliases.get(&cid).cloned();
            blocks.push(EditorBlock {
                id: cid,
                kind,
                text,
                attributes,
                logical_id,
                inline_spans: Vec::new(),
                parent: None,
            });
        } else {
            blocks.push(finish_block(
                cid,
                kind,
                attributes,
                &texts,
                collect_values,
                &reverse_aliases,
            )?);
        }
    }
    if let Some(index) = index {
        apply_placements(&mut blocks, &index)?;
    }
    Ok(Some(blocks))
}

/// What the current reader made of one block map.
#[derive(Debug)]
pub enum CurrentRead {
    /// Not read: the caller did not ask, the block is of an unsupported kind, or it is a
    /// dependency node.
    Skipped,
    /// The block the current reader returns.
    Block(EditorBlock),
    /// The error the current reader holds for this block.
    Invalid(CollabError),
}

/// What [`evaluate_block`] found in one block map.
#[derive(Debug)]
pub struct BlockVisit {
    /// The map's `nodeName` is an atom kind, placed or not, supported or not.
    pub physical_atom: bool,
    /// The legacy reader lists this block: a model-1 prose kind that is not a dependency node.
    pub legacy_listed: bool,
    pub current: CurrentRead,
}

/// The per-block body of [`legacy_ids_and_editor_blocks`]: everything the legacy reader and the
/// current reader do with one physical block map, in one place, so that the full projection and the
/// incremental window check (`window_check`) cannot disagree about a block. It reads only the
/// map's own containers (`nodeName`, `attributes`, the inline texts and their marks).
///
/// The legacy reader's error is `Err`: it returns immediately in the full pass and takes
/// precedence over everything the current reader reports. The current reader's outcome for this
/// block is `current`; it is evaluated only when `want_current` (the caller has no earlier current
/// error and the block is placed), so a block the full pass would not read is not read here.
///
/// # Errors
/// `editor_block_missing_node_name`, or the `text_children` error of a legacy-listed block.
pub fn evaluate_block(
    map: &LoroMap,
    cid: &str,
    want_current: bool,
    reverse_aliases: &std::collections::BTreeMap<String, String>,
) -> Result<BlockVisit, CollabError> {
    let kind = string(map, "nodeName");
    let physical_atom = kind.as_deref().is_some_and(crate::editor_media::is_atom);
    let mut attributes = None;
    let mut texts = None;
    let mut legacy_listed = false;

    // Legacy reader: every block, model-1 kinds only.
    let Some(kind) = kind else {
        return Err(invalid("editor_block_missing_node_name"));
    };
    if supported(&kind) && is_legacy_kind(&kind) {
        let legacy_attributes = attributes.insert(block_attributes(map));
        if !is_dependency(legacy_attributes) {
            texts = Some(text_children(map)?);
            legacy_listed = true;
        }
    }

    let current = if want_current {
        current_read(map, cid, kind, attributes, texts, reverse_aliases)
    } else {
        CurrentRead::Skipped
    };
    Ok(BlockVisit {
        physical_atom,
        legacy_listed,
        current,
    })
}

/// The current reader's half of [`evaluate_block`], reusing what the legacy half already read.
fn current_read(
    map: &LoroMap,
    cid: &str,
    kind: String,
    attributes: Option<Value>,
    texts: Option<Vec<LoroText>>,
    reverse_aliases: &std::collections::BTreeMap<String, String>,
) -> CurrentRead {
    if !supported(&kind) {
        return CurrentRead::Skipped;
    }
    let attributes = attributes.unwrap_or_else(|| block_attributes(map));
    if !crate::editor_media::is_atom(&kind) && is_dependency(&attributes) {
        return CurrentRead::Skipped;
    }
    let texts = match texts.map_or_else(|| text_children(map), Ok) {
        Ok(texts) => texts,
        Err(error) => return CurrentRead::Invalid(error),
    };
    match finish_block(cid.to_owned(), kind, attributes, &texts, true, reverse_aliases) {
        Ok(block) => CurrentRead::Block(block),
        Err(error) => CurrentRead::Invalid(error),
    }
}

/// What the editor-model floor needs from the single pass in [`legacy_ids_and_editor_blocks`]:
/// the two block signals of [`editor_model_signals`], and whether any *physical* block map (placed
/// or not, supported or not) has an atom `nodeName`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct FloorFacts {
    pub physical_atom: bool,
    pub signals: EditorModelSignals,
}

/// The block ids `legacy_editor_blocks` returns, what `editor_blocks` returns, and the floor facts
/// of the same pass.
pub type LegacyIdsAndBlocks = (Vec<String>, Vec<EditorBlock>, FloorFacts);

/// `legacy_editor_blocks` (reduced to block ids) and `editor_blocks`, evaluated in that order but
/// in one pass over the editor document. The result -- values, `None`, and which typed error is
/// returned -- equals calling the two readers in sequence: an error the legacy reader would
/// return wins over anything the current reader would report, then the current reader's
/// placement error, then its first per-block error.
///
/// The returned [`FloorFacts`] equal what the floor walk derives (`physical_atoms_present` and
/// [`editor_model_signals`]) whenever this succeeds, because both read the same root containers
/// (by typed id) and the same physical maps and current blocks. The reverse does not hold: this
/// pass can fail where the walk succeeds, for example on an unplaced block map without
/// `nodeName` on a structured page, which the walk skips and this reader rejects. Nothing is
/// derived from a failed pass.
pub fn legacy_ids_and_editor_blocks(doc: &LoroDoc) -> Result<Option<LegacyIdsAndBlocks>, CollabError> {
    #[cfg(test)]
    crate::walk_counter::record_block_read();
    // Both readers stop here identically: no editor document, or too many aliases.
    let Some(list) = root_children(doc)? else {
        return Ok(None);
    };
    let aliases = aliases_map(doc)?;
    if aliases
        .as_ref()
        .is_some_and(|aliases| aliases.len() > crate::limits::DOCUMENT_BLOCK_COUNT_MAX)
    {
        return Err(invalid("editor_block_alias_limit_exceeded"));
    }
    let reverse_aliases = reverse_aliases(aliases.as_ref());
    // The current reader's placement error is reported only if the legacy reader succeeds.
    let placements = crate::editor_structure::placements(doc);
    let index = match &placements {
        Ok(Some(entries)) => Some(PlacementIndex::new(entries)),
        _ => None,
    };
    let mut current_error = placements.as_ref().err().cloned();
    let mut legacy_ids = Vec::new();
    let mut blocks = Vec::new();
    let mut physical_atom = false;
    for i in 0..list.len() {
        let Some(ValueOrContainer::Container(Container::Map(map))) = list.get(i) else {
            return Err(invalid("editor_doc_unsupported_block_container"));
        };
        let cid = map.id().to_string();
        // The current reader's first error is held until the legacy reader has visited every
        // block, and it reads placed blocks only.
        let want_current = current_error.is_none() && index.as_ref().is_none_or(|index| index.contains(&cid));
        // A legacy reader error returns immediately.
        let visit = evaluate_block(&map, &cid, want_current, &reverse_aliases)?;
        physical_atom |= visit.physical_atom;
        if visit.legacy_listed {
            legacy_ids.push(cid);
        }
        match visit.current {
            CurrentRead::Skipped => {}
            CurrentRead::Block(block) => blocks.push(block),
            CurrentRead::Invalid(error) => current_error = Some(error),
        }
    }
    if let Some(error) = current_error {
        return Err(error);
    }
    if let Some(index) = index {
        apply_placements(&mut blocks, &index)?;
    }
    let signals = signals_of(&blocks);
    Ok(Some((legacy_ids, blocks, FloorFacts { physical_atom, signals })))
}
/// Resolve only live top-level editor maps. Tombstoned aliases cannot resurrect blocks.
///
/// # Errors
/// A failed root read (`editor_root_probe_failed`), which is neither "found" nor "not found". Any
/// other failure of the placement index reads as not found, as before.
pub fn resolve_editor_block_id(doc: &LoroDoc, id: &str) -> Result<Option<String>, CollabError> {
    let Some(list) = root_children(doc)? else {
        return Ok(None);
    };
    let canonical = alias_target(doc, id)?.unwrap_or_else(|| id.to_owned());
    match crate::editor_structure::placements(doc) {
        Ok(placements) => {
            if placements.is_some_and(|entries| !entries.iter().any(|entry| entry.cid == canonical)) {
                return Ok(None);
            }
        }
        Err(error) if is_probe_failure(&error) => return Err(error),
        Err(_) => return Ok(None),
    }
    for i in 0..list.len() {
        if let Some(ValueOrContainer::Container(Container::Map(map))) = list.get(i)
            && map.id().to_string() == canonical
        {
            return Ok(Some(canonical));
        }
    }
    Ok(None)
}
/// Whether `error` is the typed failure of a root read, as opposed to a verdict about the content.
fn is_probe_failure(error: &CollabError) -> bool {
    matches!(error, CollabError::OperationFailed { reason } if reason.starts_with("editor_root_probe_failed"))
}

/// Apply one UTF-8 byte-indexed edit to an actual reachable editor block CID.
/// Returns false for non-editor IDs; caller must reject legacy shadow prose after initialization.
/// Both byte boundaries are validated before any mutation. Loro's explicit UTF-8 API performs
/// the coordinate translation; browser UTF-16 offsets must never be passed here as byte offsets.
#[cfg(test)]
pub fn apply_editor_text(
    doc: &LoroDoc,
    id: &str,
    index: u32,
    delete_len: Option<u32>,
    insert: Option<&str>,
) -> Result<bool, CollabError> {
    let Some(list) = root_children(doc)? else {
        return Ok(false);
    };
    let Some(canonical_id) = resolve_editor_block_id(doc, id)? else {
        return Ok(false);
    };
    let Some(map) = scan_block_map(&list, &canonical_id) else {
        return Ok(false);
    };
    apply_text_to_block(&map, index, delete_len, insert)
}

/// [`apply_editor_text`] with the block located through the session index: on a structured page
/// the placement index answers "is this CID an active block" (which also proves the CID is a live
/// element of `doc.children`), so neither the placement tree nor the block list is walked.
/// Without an initialised structure the block is located exactly as before.
///
/// An error drops the index.
///
/// # Errors
/// As [`apply_editor_text`].
pub fn apply_editor_text_session(
    doc: &LoroDoc,
    id: &str,
    index: u32,
    delete_len: Option<u32>,
    insert: Option<&str>,
    slot: &mut Option<crate::editor_structure::PlacementIndex>,
) -> Result<bool, CollabError> {
    let result = text_in_session(doc, id, index, delete_len, insert, slot);
    if result.is_err() {
        *slot = None;
    }
    result
}

fn text_in_session(
    doc: &LoroDoc,
    id: &str,
    index: u32,
    delete_len: Option<u32>,
    insert: Option<&str>,
    slot: &mut Option<crate::editor_structure::PlacementIndex>,
) -> Result<bool, CollabError> {
    let Some(list) = root_children(doc)? else {
        return Ok(false);
    };
    let map = if crate::editor_structure::ready(doc)? {
        let canonical = alias_target(doc, id)?.unwrap_or_else(|| id.to_owned());
        match session_index(doc, slot) {
            Ok(placements) if placements.contains(&canonical) => {
                match live_block_map(doc, &canonical).or_else(|| scan_block_map(&list, &canonical)) {
                    Some(map) => map,
                    None => return Ok(false),
                }
            }
            Err(error) if is_probe_failure(&error) => return Err(error),
            Ok(_) | Err(_) => return Ok(false),
        }
    } else {
        let Some(canonical_id) = resolve_editor_block_id(doc, id)? else {
            return Ok(false);
        };
        let Some(map) = scan_block_map(&list, &canonical_id) else {
            return Ok(false);
        };
        map
    };
    apply_text_to_block(&map, index, delete_len, insert)
}

/// The block map with container id `canonical` among the elements of `list`, by scanning it.
fn scan_block_map(list: &LoroList, canonical: &str) -> Option<LoroMap> {
    for i in 0..list.len() {
        if let Some(ValueOrContainer::Container(Container::Map(map))) = list.get(i)
            && map.id().to_string() == canonical
        {
            return Some(map);
        }
    }
    None
}

/// The map container `canonical` names, looked up by id. Only valid as the block map when the
/// caller knows `canonical` is an active placement (active blocks are live elements of
/// `doc.children`); `None` when the id does not name a map.
fn live_block_map(doc: &LoroDoc, canonical: &str) -> Option<LoroMap> {
    let id = loro::ContainerID::try_from(canonical).ok()?;
    match doc.get_container(id) {
        Some(Container::Map(map)) => Some(map),
        _ => None,
    }
}

/// The session index of a structured page: the one in `slot`, or a fresh full build that is then
/// kept in `slot`.
fn session_index<'a>(
    doc: &LoroDoc,
    slot: &'a mut Option<crate::editor_structure::PlacementIndex>,
) -> Result<&'a mut crate::editor_structure::PlacementIndex, CollabError> {
    if slot
        .as_ref()
        .is_none_or(crate::editor_structure::PlacementIndex::is_stale)
    {
        *slot = Some(crate::editor_structure::PlacementIndex::build(doc)?);
    }
    slot.as_mut().ok_or_else(|| invalid("editor_structure_required"))
}

/// [`resolve_editor_block_id`] on a structured page, answered by the session index. The caller
/// has established that the structure is initialised.
fn resolve_in_session(
    doc: &LoroDoc,
    id: &str,
    slot: &mut Option<crate::editor_structure::PlacementIndex>,
) -> Result<Option<String>, CollabError> {
    if root_children(doc)?.is_none() {
        return Ok(None);
    }
    let canonical = alias_target(doc, id)?.unwrap_or_else(|| id.to_owned());
    match session_index(doc, slot) {
        Ok(placements) => Ok(placements.contains(&canonical).then_some(canonical)),
        Err(error) if is_probe_failure(&error) => Err(error),
        Err(_) => Ok(None),
    }
}

fn apply_text_to_block(
    map: &LoroMap,
    index: u32,
    delete_len: Option<u32>,
    insert: Option<&str>,
) -> Result<bool, CollabError> {
    let kind = string(map, "nodeName").ok_or_else(|| invalid("editor_block_missing_node_name"))?;
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
    let texts = text_children(map)?;
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
        children(map)
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
    let canonical = resolve_editor_block_id(doc, id)?.ok_or_else(|| invalid("editor_block_not_found"))?;
    let list = root_children(doc)?.ok_or_else(|| invalid("editor_doc_required"))?;
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
///
/// This is the recompute-every-time reference: every operation rebuilds the placement index from
/// the whole tree. Production uses [`apply_editor_structure_session`]; the differential tests
/// compare the two byte for byte.
#[cfg(test)]
pub fn apply_editor_structure(doc: &LoroDoc, operation: &crate::Operation) -> Result<bool, CollabError> {
    let Some(list) = root_children(doc)? else {
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
        let indexed = crate::editor_structure::ready(doc)?;
        if parent.is_some() && !indexed {
            return Err(invalid("editor_block_parent_must_be_root"));
        }
        let parent_cid = parent
            .as_ref()
            .map(|id| resolve_editor_block_id(doc, id)?.ok_or_else(|| invalid("editor_structure_parent_not_found")))
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
        if has_alias_entry(doc, id.as_ref())? {
            return Err(CollabError::DuplicateNode { id: id.to_string() });
        }
        if list.len() >= crate::limits::DOCUMENT_BLOCK_COUNT_MAX
            || aliases_map(doc)?.is_some_and(|aliases| aliases.len() >= crate::limits::DOCUMENT_BLOCK_COUNT_MAX)
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
        doc.get_map(crate::reserved_roots::ALIASES)
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
    let canonical = alias_target(doc, id.as_ref())?.unwrap_or_else(|| id.to_string());
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
        if has_alias_entry(doc, id.as_ref())? {
            return Err(CollabError::UnknownNode { id: id.to_string() });
        }
        return Ok(false);
    };
    if !string(&map, "nodeName").is_some_and(|kind| supported(&kind)) {
        return Err(invalid("editor_block_unsupported_kind"));
    }
    match operation {
        crate::Operation::DeleteNode { .. } => {
            if crate::editor_structure::ready(doc)? {
                crate::editor_structure::delete(doc, &canonical)?;
                return Ok(true);
            }
            if list.len() == 1 {
                return Err(invalid("editor_doc_requires_one_block"));
            }
            list.delete(position, 1).map_err(|e| invalid(&e.to_string()))?;
        }
        crate::Operation::MoveNode { new_parent, index, .. } => {
            if crate::editor_structure::ready(doc)? {
                let parent = new_parent
                    .as_ref()
                    .map(|id| {
                        resolve_editor_block_id(doc, id)?.ok_or_else(|| invalid("editor_structure_parent_not_found"))
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
            set_block_property(doc, &map, key, value)?;
        }
        _ => return Ok(false),
    }
    Ok(true)
}

/// [`apply_editor_structure`] against the session index in `slot`: the placement index is built
/// once (on first use) and updated by each operation, so a command build of many structural edits
/// reads the placement tree once. The operations performed on the document, and their order, are
/// those of the reference, so the resulting document is byte-for-byte the same.
///
/// An error drops the index (a failed operation may have written part of its change).
///
/// # Errors
/// As [`apply_editor_structure`].
pub fn apply_editor_structure_session(
    doc: &LoroDoc,
    operation: &crate::Operation,
    slot: &mut Option<crate::editor_structure::PlacementIndex>,
) -> Result<bool, CollabError> {
    let result = structure_in_session(doc, operation, slot);
    if result.is_err() {
        *slot = None;
    }
    result
}

fn structure_in_session(
    doc: &LoroDoc,
    operation: &crate::Operation,
    slot: &mut Option<crate::editor_structure::PlacementIndex>,
) -> Result<bool, CollabError> {
    let Some(list) = root_children(doc)? else {
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
        let indexed = crate::editor_structure::ready(doc)?;
        if parent.is_some() && !indexed {
            return Err(invalid("editor_block_parent_must_be_root"));
        }
        let parent_cid = parent
            .as_ref()
            .map(|id| resolve_in_session(doc, id, slot)?.ok_or_else(|| invalid("editor_structure_parent_not_found")))
            .transpose()?;
        if indexed {
            let placements = session_index(doc, slot)?;
            let (count, parent_depth) = placements.placement_bounds(parent_cid.as_deref()).unwrap_or((0, None));
            if parent_depth.is_some_and(|depth| depth >= 7) {
                return Err(invalid("editor_structure_depth_limit"));
            }
            if *index as usize > count {
                return Err(invalid("editor_block_index_out_of_bounds"));
            }
        }
        if id.is_empty() || id.len() > 256 || id.starts_with("cid:") {
            return Err(invalid("editor_block_invalid_logical_id"));
        }
        if has_alias_entry(doc, id.as_ref())? {
            return Err(CollabError::DuplicateNode { id: id.to_string() });
        }
        if list.len() >= crate::limits::DOCUMENT_BLOCK_COUNT_MAX
            || aliases_map(doc)?.is_some_and(|aliases| aliases.len() >= crate::limits::DOCUMENT_BLOCK_COUNT_MAX)
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
            crate::editor_structure::insert_indexed(
                session_index(doc, slot)?,
                &attached.id().to_string(),
                parent_cid.as_deref(),
                *index as usize,
            )?;
        }
        doc.get_map(crate::reserved_roots::ALIASES)
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
    let canonical = alias_target(doc, id.as_ref())?.unwrap_or_else(|| id.to_string());
    // With a valid index that lists the block as active, the block is a live element of
    // `doc.children` and is fetched by id; otherwise the list is scanned as before.
    let known = slot
        .as_ref()
        .is_some_and(|placements| !placements.is_stale() && placements.contains(&canonical));
    let target = known
        .then(|| live_block_map(doc, &canonical).map(|map| (None, map)))
        .flatten()
        .or_else(|| scan_block_position(&list, &canonical).map(|(position, map)| (Some(position), map)));
    let Some((position, map)) = target else {
        if has_alias_entry(doc, id.as_ref())? {
            return Err(CollabError::UnknownNode { id: id.to_string() });
        }
        return Ok(false);
    };
    let position = || position.or_else(|| scan_block_position(&list, &canonical).map(|(position, _)| position));
    if !string(&map, "nodeName").is_some_and(|kind| supported(&kind)) {
        return Err(invalid("editor_block_unsupported_kind"));
    }
    match operation {
        crate::Operation::DeleteNode { .. } => {
            if crate::editor_structure::ready(doc)? {
                crate::editor_structure::delete_indexed(session_index(doc, slot)?, &canonical)?;
                return Ok(true);
            }
            if list.len() == 1 {
                return Err(invalid("editor_doc_requires_one_block"));
            }
            list.delete(position().unwrap_or_default(), 1)
                .map_err(|e| invalid(&e.to_string()))?;
        }
        crate::Operation::MoveNode { new_parent, index, .. } => {
            if crate::editor_structure::ready(doc)? {
                let parent = new_parent
                    .as_ref()
                    .map(|id| {
                        resolve_in_session(doc, id, slot)?.ok_or_else(|| invalid("editor_structure_parent_not_found"))
                    })
                    .transpose()?;
                crate::editor_structure::move_indexed(
                    session_index(doc, slot)?,
                    &canonical,
                    parent.as_deref(),
                    *index as usize,
                )?;
                return Ok(true);
            }
            if new_parent.is_some() {
                return Err(invalid("editor_block_parent_must_be_root"));
            }
            if Some(*index as usize) != position() {
                return Err(invalid("editor_block_identity_preserving_move_unsupported"));
            }
        }
        crate::Operation::SetProperty { key, value, .. } => {
            set_block_property(doc, &map, key, value)?;
        }
        _ => return Ok(false),
    }
    Ok(true)
}

/// The position and map of the block `canonical` in `list`, by scanning it.
fn scan_block_position(list: &LoroList, canonical: &str) -> Option<(usize, LoroMap)> {
    for i in 0..list.len() {
        if let Some(ValueOrContainer::Container(Container::Map(map))) = list.get(i)
            && map.id().to_string() == canonical
        {
            return Some((i, map));
        }
    }
    None
}

/// The `SetProperty` edit of one editor block (no placement index is read or written).
fn set_block_property(doc: &LoroDoc, map: &LoroMap, key: &str, value: &str) -> Result<(), CollabError> {
    let Some(ValueOrContainer::Container(Container::Map(attrs))) = map.get("attributes") else {
        return Err(invalid("editor_block_missing_attributes"));
    };
    match key {
        "block_type" => {
            if !supported(value) || crate::editor_media::is_atom(value) {
                return Err(invalid("editor_block_unsupported_kind"));
            }
            if value == "codeBlock"
                && text_children(map)?
                    .iter()
                    .any(|text| crate::page_inline::spans(text).is_ok_and(|spans| !spans.is_empty()))
            {
                return Err(invalid("editor_code_block_requires_unmarked_text"));
            }
            if value == "divider" && text_children(map)?.iter().any(|text| !text.to_string().is_empty()) {
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
            match value {
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
            map.insert("nodeName", value).map_err(|e| invalid(&e.to_string()))?;
        }
        "alt" | "caption" | "name" | "mime"
            if string(map, "nodeName")
                .is_some_and(|kind| crate::editor_media::is_atom(&kind) && kind != "childPage") =>
        {
            let max = match key {
                "name" => 255,
                "mime" => 128,
                _ => 4096,
            };
            if value.len() > max {
                return Err(invalid("editor_atom_invalid_text_attribute"));
            }
            attrs.insert(key, value).map_err(|error| invalid(&error.to_string()))?;
        }
        "checked" | "collapsed" => {
            let kind = string(map, "nodeName");
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
            if value == 0 || value > 1_000_000 || string(map, "nodeName").as_deref() != Some("orderedItem") {
                return Err(invalid("editor_block_invalid_attribute"));
            }
            attrs.insert(key, value).map_err(|e| invalid(&e.to_string()))?;
        }
        "indent" if crate::editor_structure::ready(doc)? => {
            return Err(invalid("editor_structure_requires_move_for_indent"));
        }
        "indent" | "level" => {
            let number = value
                .parse::<i32>()
                .map_err(|_| invalid("editor_block_invalid_attribute"))?;
            if (key == "indent" && !(0..=7).contains(&number))
                || (key == "level"
                    && (!(1..=3).contains(&number) || string(map, "nodeName").as_deref() != Some("heading")))
            {
                return Err(invalid("editor_block_invalid_attribute"));
            }
            attrs.insert(key, number).map_err(|e| invalid(&e.to_string()))?;
        }
        _ => return Err(invalid("editor_block_unsupported_property")),
    }
    Ok(())
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
    if id.is_empty() || id.len() > 256 || id.starts_with("cid:") || has_alias_entry(doc, id)? {
        return Err(invalid("editor_block_invalid_or_duplicate_logical_id"));
    }
    let list = root_children(doc)?.ok_or_else(|| invalid("editor_doc_required"))?;
    let placements = crate::editor_structure::placements(doc)?.ok_or_else(|| invalid("editor_structure_required"))?;
    let parent = parent
        .map(|id| resolve_editor_block_id(doc, id)?.ok_or_else(|| invalid("editor_structure_parent_not_found")))
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
    let cid = resolve_editor_block_id(doc, id)?.ok_or_else(|| invalid("editor_block_not_found"))?;
    let list = root_children(doc)?.ok_or_else(|| invalid("editor_doc_required"))?;
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

    /// The index must answer exactly like the linear scans it replaced, including for duplicate
    /// CIDs (first entry for `find`, last index for the collected position map) and absent ones.
    #[test]
    fn placement_index_matches_linear_scans_with_duplicates() {
        use crate::editor_structure::Placement;
        let entry = |cid: &str, depth: u32, counter: i32| Placement {
            cid: cid.to_owned(),
            parent: (depth > 0).then(|| format!("parent-of-{cid}")),
            depth,
            tree_id: loro::TreeID { peer: 7, counter },
            position: usize::try_from(counter).unwrap_or(0),
        };
        let cases = [
            vec![],
            vec![entry("a", 0, 0)],
            vec![entry("a", 0, 0), entry("b", 1, 1), entry("a", 2, 2)],
            vec![entry("x", 3, 0), entry("x", 1, 1), entry("x", 0, 2), entry("y", 0, 3)],
            vec![entry("", 0, 0), entry("b", 0, 1), entry("", 4, 2), entry("b", 2, 3)],
        ];
        for entries in &cases {
            let index = PlacementIndex::new(entries);
            let positions: std::collections::BTreeMap<_, _> = entries
                .iter()
                .enumerate()
                .map(|(position, entry)| (entry.cid.as_str(), position))
                .collect();
            for cid in ["a", "b", "x", "y", "", "missing"] {
                assert_eq!(
                    index.contains(cid),
                    entries.iter().any(|entry| entry.cid == cid),
                    "{cid}"
                );
                let linear = entries.iter().find(|entry| entry.cid == cid);
                assert_eq!(
                    index
                        .first(cid)
                        .map(|entry| (entry.tree_id, entry.depth, entry.parent.clone())),
                    linear.map(|entry| (entry.tree_id, entry.depth, entry.parent.clone())),
                    "{cid}"
                );
                assert_eq!(index.position(cid), positions.get(cid).copied(), "{cid}");
            }
        }
    }
    /// A page with `blocks` paragraphs (structured when `structured`), as a raw Loro document
    /// that tests can damage.
    fn damageable_page(seed: u64, blocks: usize, structured: bool) -> LoroDoc {
        use crate::{CollabEngine, LoroCollabEngine, NodeKind, Operation};
        let mut engine = LoroCollabEngine::new_empty(seed);
        engine.initialize_page_editor().unwrap();
        if structured {
            engine.initialize_page_structure_model(4).unwrap();
        }
        for index in 0..blocks {
            let id = format!("p{index}");
            engine
                .apply_content_operation(&Operation::CreateNode {
                    id: id.as_str().into(),
                    parent: None,
                    index: u32::try_from(index + 1).unwrap(),
                    kind: NodeKind::Block,
                })
                .unwrap();
            engine
                .apply_content_operation(&Operation::InsertText {
                    id: id.as_str().into(),
                    index: 0,
                    text: format!("text {index}"),
                })
                .unwrap();
        }
        if structured {
            engine
                .apply_content_operation(&Operation::DeleteNode { id: "p1".into() })
                .unwrap();
        }
        let doc = LoroDoc::new();
        crate::page_inline::configure(&doc);
        doc.import(&engine.export_snapshot().unwrap()).unwrap();
        doc.set_peer_id(77).unwrap();
        doc
    }

    fn damaged_block(doc: &LoroDoc, index: usize) -> LoroMap {
        match root_children(doc).unwrap().unwrap().get(index) {
            Some(ValueOrContainer::Container(Container::Map(map))) => map,
            _ => panic!("block {index}"),
        }
    }

    /// One way to break block `index` (or the whole page for the structure-level defects).
    fn damage(doc: &LoroDoc, defect: usize, index: usize) {
        let map = damaged_block(doc, index);
        match defect {
            0 => {
                map.delete("children").unwrap();
            }
            1 => {
                map.delete("nodeName").unwrap();
            }
            2 => {
                map.insert("nodeName", "image").unwrap();
            }
            3 => {
                if let Some(text) = text_children(&map).ok().and_then(|texts| texts.into_iter().next()) {
                    text.mark(0..2, "link", "javascript:x").unwrap();
                }
            }
            4 => {
                map.insert("nodeName", "todoItem").unwrap();
                map.delete("children").unwrap();
            }
            5 => {
                doc.get_map(crate::editor_structure::META)
                    .insert("model", "rich_page_v0")
                    .unwrap();
            }
            _ => {
                let list = root_children(doc).unwrap().unwrap();
                list.insert_container(list.len(), LoroText::new()).unwrap();
            }
        }
        doc.commit();
    }

    fn summarize(blocks: Vec<EditorBlock>) -> Vec<String> {
        blocks
            .into_iter()
            .map(|block| {
                format!(
                    "{}|{}|{}|{}|{:?}|{:?}|{:?}",
                    block.id,
                    block.kind,
                    block.text,
                    block.attributes,
                    block.logical_id,
                    block.inline_spans,
                    block.parent
                )
            })
            .collect()
    }

    /// `legacy_editor_blocks` then `editor_blocks`, as `semantic_snapshot` used to call them.
    type Summary = Option<(Vec<String>, Vec<String>)>;
    fn sequential_reads(doc: &LoroDoc) -> Result<Summary, CollabError> {
        let legacy = legacy_editor_blocks(doc)?;
        let current = editor_blocks(doc)?;
        Ok(legacy
            .zip(current)
            .map(|(legacy, current)| (legacy.into_iter().map(|block| block.id).collect(), summarize(current))))
    }

    /// Asserts that the single-pass reader agrees with the sequential readers on `doc`; returns
    /// the shared error text when both fail.
    fn assert_single_pass_matches(doc: &LoroDoc, label: &str) -> Option<String> {
        let fused = legacy_ids_and_editor_blocks(doc).map(|read| read.map(|(ids, blocks, _)| (ids, summarize(blocks))));
        let sequential = sequential_reads(doc);
        assert_eq!(sequential, fused, "{label}");
        sequential.err().map(|error| error.to_string())
    }

    /// The single-pass reader must return exactly what the two readers return in sequence --
    /// including which error wins -- for every pair of defects at several pairs of positions,
    /// on flat and structured pages (structured pages include a block behind a deleted
    /// placement, which only the legacy reader visits).
    #[test]
    fn single_pass_prose_read_matches_sequential_readers_under_combined_defects() {
        let positions = [(1, 3), (3, 1), (2, 2), (1, 2), (4, 1)];
        let cases = [false, true].into_iter().flat_map(|structured| {
            (0..7).flat_map(move |first| {
                (0..7).flat_map(move |second| positions.map(move |(a, b)| (structured, first, second, a, b)))
            })
        });
        let mut errors = std::collections::BTreeSet::new();
        for (seed, (structured, first, second, a, b)) in (500_u64..).zip(cases) {
            let doc = damageable_page(seed, 4, structured);
            damage(&doc, first, a);
            damage(&doc, second, b);
            let label = format!("structured={structured} defects=({first}@{a},{second}@{b})");
            errors.extend(assert_single_pass_matches(&doc, &label));
        }
        assert!(
            errors.len() >= 6,
            "defects did not exercise enough distinct errors: {errors:?}"
        );
    }

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
        let list = root_children(&doc).unwrap().unwrap();
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
        let list = root_children(&doc).unwrap().unwrap();
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
        let list = root_children(&doc).unwrap().unwrap();
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
        assert!(root_children(&doc).unwrap().is_none());
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
    fn rich_snapshot_roundtrip() {
        use crate::CollabEngine;
        let mut engine = crate::LoroCollabEngine::load(SNAPSHOT).unwrap();
        let current = engine.semantic_snapshot().unwrap();
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
        assert_ne!(rich.semantic_hash(), current.semantic_hash());
        let loaded = crate::LoroCollabEngine::load(&engine.export_snapshot().unwrap()).unwrap();
        assert_eq!(loaded.semantic_snapshot().unwrap(), rich);
        assert_eq!(loaded.resolve_content_node_id(&id).unwrap(), id);
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
