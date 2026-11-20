//! Update-scoped inputs of the editor-model downgrade guard.
//!
//! The guard refuses an update that takes a page whose floor was at least 5 below 5. Floor >= 5
//! is "the model-5 marker is set, or some physical block map in `doc.children` has an atom
//! `nodeName`". The marker is an O(1) read. The physical-atom signal can only fall through ops
//! that name the containers involved, so scanning the *imported ops* decides, without walking the
//! document, whether the update removed or re-typed any block map. See
//! `FLOW-INCREMENTAL-EDITOR-FLOOR-DESIGN.md` section 5.2 for the full argument.
//!
//! Everything here is exact or errs towards a slower, exact fallback; nothing approximates.
use loro::{
    Container, ContainerID, ContainerTrait, ContainerType, Frontiers, ID, IdSpan, ImportStatus, JsonListOp, JsonMapOp,
    JsonOp, JsonOpContent, LoroDoc, LoroValue, ValueOrContainer,
};

use crate::error::CollabError;

/// Result of scanning the imported ops for anything that could lower the physical-atom signal.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RemovalScan {
    /// No block map that existed before the import was removed or re-typed, and `doc.children`
    /// was not touched: the physical-atom signal cannot have fallen.
    None,
    /// Some imported op may have removed or re-typed a pre-existing block map, or touched the
    /// `doc` map itself. The caller decides exactly with a narrow scan and, if needed,
    /// [`physical_atoms_at`].
    Suspect,
}

/// Whether `counter` of `peer` was created by the import `status` describes.
fn created_by_update(status: &ImportStatus, peer: u64, counter: i32) -> bool {
    status
        .success
        .get(&peer)
        .is_some_and(|&(start, end)| start <= counter && counter < end)
}

fn root_children_id(doc: &LoroDoc) -> Option<ContainerID> {
    match doc.get_map("doc").get("children") {
        Some(ValueOrContainer::Container(Container::List(list))) => Some(list.id()),
        _ => None,
    }
}

/// Whether the deleted root-list element `id` might have been an atom block, judged from its
/// current (deleted) state: true when it is not a map, is unreadable, has no string `nodeName`,
/// or its `nodeName` is an atom kind. A deleted map stays readable after deletion, so a plain
/// paragraph is recognised in O(1).
fn deleted_element_is_suspect(doc: &LoroDoc, id: ID) -> bool {
    let cid = ContainerID::new_normal(id, ContainerType::Map);
    let Some(Container::Map(map)) = doc.get_container(cid) else {
        return true;
    };
    match map.get("nodeName") {
        Some(ValueOrContainer::Value(LoroValue::String(kind))) => crate::editor_media::is_atom(kind.as_ref()),
        _ => true,
    }
}

/// The element ids a root-list `Delete { len, start_id }` names: `start_id`, `start_id + 1`, ...
/// for `|len|` elements. A negative `len` marks a backward deletion (the elements were deleted
/// from the end towards the start); Loro still reports the *lowest* id as `start_id`, which the
/// pin test `pin_list_delete_ids_resolve_to_deleted_block_maps` checks for both signs. `None`
/// when the span overflows or names more elements than a document may hold, which the caller
/// treats as suspect.
fn deleted_element_ids(start: ID, len: i32) -> Option<impl Iterator<Item = ID>> {
    let count = len.unsigned_abs();
    if usize::try_from(count).ok()? > crate::limits::DOCUMENT_BLOCK_COUNT_MAX {
        return None;
    }
    let count = i32::try_from(count).ok()?;
    // Reject spans whose last counter does not fit, instead of wrapping.
    start.counter.checked_add(count)?;
    Some((0..count).map(move |offset| ID::new(start.peer, start.counter + offset)))
}

/// Scans the ops `status` reports as applied for the three op kinds that can lower the
/// physical-atom signal (`P`):
///
/// 1. a `Delete` on the root `doc.children` list that names a block map the update did not
///    create and whose `nodeName` is an atom, missing or unreadable;
/// 2. a `nodeName` insert or delete on a map the update did not create;
/// 3. any op on the root `doc` map (its `children` key is the list itself).
///
/// Cost: O(imported ops) plus O(deleted elements). Independent of document size.
///
/// A document without a root list has no element to delete, so rule 1 cannot apply; rules 2 and 3
/// still do, and an update that creates the list from nothing only ever adds to `P`.
pub fn removal_scan(doc: &LoroDoc, status: &ImportStatus) -> RemovalScan {
    let root_list = root_children_id(doc);
    let doc_map = ContainerID::new_root("doc", ContainerType::Map);
    for (&peer, &(start, end)) in status.success.iter() {
        for change in doc.export_json_in_id_span(IdSpan::new(peer, start, end)) {
            for op in change.ops {
                #[cfg(test)]
                crate::walk_counter::record_scanned_op();
                if op.container == doc_map || op_removes_block(doc, status, root_list.as_ref(), &op) {
                    return RemovalScan::Suspect;
                }
            }
        }
    }
    RemovalScan::None
}

/// Rules 1 and 2 of [`removal_scan`] for one op.
fn op_removes_block(doc: &LoroDoc, status: &ImportStatus, root_list: Option<&ContainerID>, op: &JsonOp) -> bool {
    match &op.content {
        JsonOpContent::List(JsonListOp::Delete { len, start_id, .. }) if root_list == Some(&op.container) => {
            let Some(mut ids) = deleted_element_ids(*start_id, *len) else {
                return true;
            };
            ids.any(|id| !created_by_update(status, id.peer, id.counter) && deleted_element_is_suspect(doc, id))
        }
        JsonOpContent::Map(JsonMapOp::Insert { key, .. } | JsonMapOp::Delete { key }) if key == "nodeName" => {
            !matches!(
                op.container,
                ContainerID::Normal { peer, counter, .. } if created_by_update(status, peer, counter)
            )
        }
        _ => false,
    }
}

/// Whether the document at `before` had a physical block map with an atom `nodeName`: the exact
/// value the pre-import floor walk's `P` signal would have returned.
///
/// Checks out `before`, reads, and returns to the latest state. Used only when the update touched
/// something [`removal_scan`] flagged. The cost is a checkout in both directions (the same
/// machinery `LoroDoc::diff` uses), O(document) and well above the common path.
pub fn physical_atoms_at(doc: &LoroDoc, before: &Frontiers) -> Result<bool, CollabError> {
    #[cfg(test)]
    crate::walk_counter::record_historical_atom_scan();
    let after = doc.state_frontiers();
    doc.checkout(before).map_err(|error| CollabError::OperationFailed {
        reason: error.to_string(),
    })?;
    let atoms = crate::engine::physical_atoms_present(doc);
    doc.try_checkout_to_latest()
        .map_err(|error| CollabError::OperationFailed {
            reason: error.to_string(),
        })?;
    if doc.is_detached() || doc.state_frontiers() != after {
        return Err(CollabError::OperationFailed {
            reason: "editor_floor_guard_restore_failed".into(),
        });
    }
    atoms
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::indexing_slicing)]
mod tests {
    use super::*;
    use loro::{ExportMode, LoroList, LoroMap};

    /// A document with `doc.children` holding one block map per kind, authored by `peer`.
    fn page(kinds: &[&str], peer: u64) -> (LoroDoc, Vec<ContainerID>) {
        let doc = LoroDoc::new();
        doc.set_peer_id(peer).unwrap();
        let list = doc
            .get_map("doc")
            .insert_container("children", LoroList::new())
            .unwrap();
        // All maps first and the `nodeName` values afterwards, so that the element ids are
        // consecutive counters and one delete op can span several of them.
        let maps: Vec<LoroMap> = (0..kinds.len())
            .map(|index| list.insert_container(index, LoroMap::new()).unwrap())
            .collect();
        for (map, kind) in maps.iter().zip(kinds) {
            map.insert("nodeName", *kind).unwrap();
        }
        doc.commit();
        (doc, maps.iter().map(ContainerTrait::id).collect())
    }

    type Case<'a> = (&'a str, fn(&LoroList), &'a [usize], bool);

    fn children(doc: &LoroDoc) -> LoroList {
        match doc.get_map("doc").get("children") {
            Some(ValueOrContainer::Container(Container::List(list))) => list,
            _ => panic!("no root list"),
        }
    }

    /// A peer that holds `base`'s snapshot, runs `edit`, and returns the update it produced.
    fn update_from(base: &LoroDoc, peer: u64, edit: impl FnOnce(&LoroDoc)) -> Vec<u8> {
        let author = LoroDoc::new();
        author.set_peer_id(peer).unwrap();
        author.import(&base.export(ExportMode::Snapshot).unwrap()).unwrap();
        let from = author.oplog_vv();
        edit(&author);
        author.commit();
        author.export(ExportMode::updates(&from)).unwrap()
    }

    fn cold_copy(doc: &LoroDoc) -> LoroDoc {
        let copy = LoroDoc::new();
        copy.import(&doc.export(ExportMode::Snapshot).unwrap()).unwrap();
        copy
    }

    /// Loro behaviour the guard's proof relies on (design section 5.3, invariant 7): a block map
    /// deleted by an imported update stays readable, with its last `nodeName`. If a Loro upgrade
    /// breaks this, `deleted_element_is_suspect` would treat every deletion as suspect.
    #[test]
    fn pin_deleted_block_map_stays_readable_after_import() {
        let (base, ids) = page(&["paragraph", "image", "paragraph"], 1);
        let update = update_from(&base, 2, |author| {
            children(author).delete(1, 1).unwrap();
        });
        let replica = cold_copy(&base);
        replica.import(&update).unwrap();
        assert_eq!(children(&replica).len(), 2, "the delete was applied");
        let Some(Container::Map(deleted)) = replica.get_container(ids[1].clone()) else {
            panic!("a deleted block map must stay addressable");
        };
        assert!(deleted.is_deleted());
        assert!(
            matches!(
                deleted.get("nodeName"),
                Some(ValueOrContainer::Value(LoroValue::String(kind))) if kind.as_ref() == "image"
            ),
            "a deleted block map must keep its last nodeName"
        );
    }

    /// Loro behaviour the guard's proof relies on (invariant 7): the ids a root-list delete names
    /// (`start_id` and `len`, forward or backward) are the container ids of the deleted block
    /// maps, so the deleted elements can be found from the op alone.
    #[test]
    #[allow(clippy::excessive_nesting)]
    fn pin_list_delete_ids_resolve_to_deleted_block_maps() {
        let kinds = ["paragraph", "image", "heading", "attachment", "paragraph", "childPage"];
        let (base, ids) = page(&kinds, 1);
        let cases: [Case<'_>; 3] = [
            ("forward span", |list| list.delete(1, 3).unwrap(), &[1, 2, 3], false),
            (
                "backward span",
                |list| {
                    list.delete(3, 1).unwrap();
                    list.delete(2, 1).unwrap();
                    list.delete(1, 1).unwrap();
                },
                &[1, 2, 3],
                true,
            ),
            ("single element", |list| list.delete(5, 1).unwrap(), &[5], false),
        ];
        for (label, edit, deleted, expect_backward) in cases {
            let update = update_from(&base, 2, |author| edit(&children(author)));
            let replica = cold_copy(&base);
            let status = replica.import(&update).unwrap();
            let root = children(&replica).id();
            let mut named = Vec::new();
            let mut saw_backward = false;
            for (&peer, &(start, end)) in status.success.iter() {
                for change in replica.export_json_in_id_span(IdSpan::new(peer, start, end)) {
                    for op in change.ops {
                        if let JsonOpContent::List(JsonListOp::Delete { len, start_id, .. }) = op.content
                            && op.container == root
                        {
                            saw_backward |= len < 0;
                            let span = deleted_element_ids(start_id, len).expect("span in range");
                            named.extend(span.map(|id| ContainerID::new_normal(id, ContainerType::Map)));
                        }
                    }
                }
            }
            named.sort_by_key(ToString::to_string);
            let mut want: Vec<_> = deleted.iter().map(|&index| ids[index].clone()).collect();
            want.sort_by_key(ToString::to_string);
            assert_eq!(named, want, "{label}: delete span must name the deleted block maps");
            assert_eq!(saw_backward, expect_backward, "{label}: span direction");
        }
    }
}
