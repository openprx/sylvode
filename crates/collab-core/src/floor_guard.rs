//! Update-scoped inputs of the editor-model downgrade guard.
//!
//! The guard refuses an update that takes a page whose floor was at least 5 below 5. Floor >= 5
//! is "the model-5 marker is set, or some physical block map in `doc.children` has an atom
//! `nodeName`". The marker is an O(1) read. The physical-atom signal `P` can only fall through
//! ops that remove an element of the root list, re-type an element, or replace the list, so
//! scanning the *imported ops* decides, without walking the document, whether the update could
//! have lowered it. See `FLOW-INCREMENTAL-EDITOR-FLOOR-DESIGN.md` section 5.2.
//!
//! Trust model: the update is hostile input. An op is cleared as harmless only from facts a
//! forged update cannot fake: the container the op belongs to (Loro resolves it from the op
//! itself, and a container id is the id of the op that created it), the op's kind, and the
//! document's own version vector before the import. The fields of a list delete (`start_id`,
//! `len`, `pos`) are *not* trusted: Loro applies a list delete by position, so the id a delete
//! names can disagree with the element it removes. Whenever the outcome would depend on which
//! element a delete removed, the exact path decides from the document's actual state.
//!
//! Everything here is exact or errs towards a slower, exact fallback; nothing approximates.
//!
//! # Caller audit (design section 6.4)
//!
//! The guard assumes: (1) the base engine holds a state that was accepted earlier, or is replayed
//! history that was accepted when it was written; (2) after an accepted import either the
//! floor-aware projection (`semantic_snapshot_with_floor`) or a fresh load and read follows, so a
//! document that cannot be read is refused there. Every call site of `import_update` and
//! `import_update_causally_complete` outside tests, classified:
//!
//! | Call site | Why the assumptions hold |
//! | --- | --- |
//! | `bin/isolated_apply_worker.rs` `decode_apply_and_check` (`import_client_update`) | The write window. The base is the accepted head. Hostile bytes. It is the causal import plus the reserved-root rule (below). The structural check runs in the same window: `window_check::run_window` evaluates the units the update touched (the ops of this same scan name them) with the projection's own per-unit code and falls back to `semantic_snapshot` and `check_snapshot` for anything it cannot classify, so a unit that cannot be read is refused there, and an untouched unit is as the valid base had it. |
//! | `bin/isolated_apply_worker.rs` `replay_diff`, package updates (`import_update_causally_complete`) | Replays accepted package history onto the base. The replayed head must match the request frontier, then `semantic_snapshot` and `check_snapshot` run. |
//! | `bin/isolated_apply_worker.rs` `replay_diff`, retained history (`import_update`) | Replays accepted rows. The snapshot frontier, each row's before and after frontier must match, so a row the guard refuses fails the request closed. |
//! | `flow/collab/write.rs` `hydrate_and_apply` | Bootstrap replay of the accepted tail before a write. Fails closed (internal error) if a row is refused; the new write then goes through the worker. |
//! | `flow/collab/snapshot.rs` `build_candidate` | Replay of the accepted tail for snapshot compaction. The head frontier is compared afterwards, and a refused row aborts the candidate. |
//! | `flow/collab/integrity.rs` `document_fingerprint` | Replay of the accepted tail. The head frontier is compared afterwards and the semantic projection is read; fails closed. |
//! | `flow/maintenance.rs` `rebuild_projection_audited` | Replay of the accepted tail, head frontier compared, then `semantic_snapshot`. |
//! | `flow/export.rs` `reconstruct_and_verify` | Replay of canonical update rows. Each row's content hash and after-frontier are verified. |
//! | `flow/package_import.rs` `reconstruct_head` | Bytes from an uploaded package (not produced by this server). Sequence gaps and member checksums are verified, the head is refused if it carries a shadow root (`refuse_shadow_roots`, below), and the caller loads the result fresh and runs `semantic_snapshot` and the semantic-hash check before anything is stored. |
//! | `flow/portable_graph_package.rs` `export_versions` | Replay of the accepted tail; a version capture (a projection) follows every update. |
//! | `flow/command.rs` `execute_content_command`, `flow/move_object.rs` `plan_document`, `flow/scope_move.rs` `navigator_positions` and `execute` | Bootstrap replay of the accepted tail onto the head before a host-authored edit. A refused row fails closed. The edit's own update is then admitted through the worker. |
//! | `flow/scope_history.rs` `restore_page` | Replay of the accepted tail, then a revert authored on the engine; the result is admitted through the worker. |
//! | `flow/scope_package.rs` `render`, `flow/scope_collections.rs` `execute_inner`, `flow/collections.rs` `engine_at_head`, `flow/document.rs` `engine_from_boot` | Read-only replay of the accepted tail. Failure is an error, and each caller reads the projection or the model floor from the replayed engine. |
//! | `flow/scope_editor.rs` `require_bootstrap_model` | Replay of the accepted tail from a bootstrap payload, followed by `minimum_editor_model`. |
//!
//! The worker's `replay_diff` with package updates (`scope_import`) takes uploaded bytes too: its
//! replayed head is checked with `refuse_shadow_roots` in the API process, outside the isolated
//! window (`flow/scope_import.rs` `require_head_without_shadow_roots`).
//!
//! # The reserved-root rule
//!
//! A root container is identified by its name and its type, so a client could add a second root
//! with a reserved name (`reserved_roots`) and another type, a "shadow root". Reading the real
//! root next to one costs a fork and two exports of the document on every read. Only the client
//! write path (`LoroCollabEngine::import_client_update`) and uploaded packages
//! (`LoroCollabEngine::refuse_shadow_roots`) refuse it; every replay entry keeps accepting
//! history that was accepted earlier, so a page that already carries one stays readable and
//! writable. See [`scan_imported_ops`] for the facts the rule rests on.
//!
//! Replay sites apply rows that the guarded write path (or the package importer, which projects)
//! already accepted; the guard decides every row as the walk it replaced did, apart from the three
//! differences listed in `CHANGELOG.md` (the fourth, the reserved-root rule, is not applied to
//! replay at all), so a row accepted then is accepted now. Test-only call
//! sites (`#[cfg(test)]`, `tests/`, examples) are not listed.
use loro::{
    Container, ContainerID, ContainerTrait, ContainerType, Frontiers, IdSpan, JsonListOp, JsonMapOp, JsonOp,
    JsonOpContent, LoroDoc, ValueOrContainer, VersionVector,
};

use crate::error::CollabError;

/// What [`scan_imported_ops_visiting`] shows to its visitor.
pub enum Scanned<'a> {
    /// A change of the imported span, before its ops (id, lamport and deps as stored).
    Change(&'a loro::JsonChange),
    /// An op of the change: its peer, the op, and the id of `doc.children` after the import.
    Op(u64, &'a JsonOp, Option<&'a ContainerID>),
}

/// Result of scanning the imported ops for anything that could lower the physical-atom signal.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RemovalScan {
    /// No element of the root list was removed or re-typed, and the `doc` map was not touched:
    /// the physical-atom signal cannot have fallen.
    None,
    /// Some imported op may have removed or re-typed a pre-existing block map, touched the `doc`
    /// map itself, or the scan could not prove it saw every imported op. The caller decides
    /// exactly with a narrow scan and, if needed, [`physical_atoms_at`].
    Suspect,
}

/// Whether the container `peer@counter` was created by an op the document did not hold before the
/// import (`before` is the op log version vector taken before it). A pre-existing container was
/// created by an op below that version, and a container id is the id of its creating op, so this
/// cannot be faked by an update.
fn created_after(before: &VersionVector, peer: u64, counter: i32) -> bool {
    counter >= before.get(&peer).copied().unwrap_or(0)
}

fn root_children_id(doc: &LoroDoc) -> Result<Option<ContainerID>, CollabError> {
    let Some(Container::Map(root)) = crate::editor_structure::reserved_root(doc, crate::reserved_roots::DOC)? else {
        return Ok(None);
    };
    Ok(match root.get("children") {
        Some(ValueOrContainer::Container(Container::List(list))) => Some(list.id()),
        _ => None,
    })
}

/// Scans every op the import added to the op log (`before` is the op log version vector taken
/// before the import) for the op kinds that can lower the physical-atom signal (`P`):
///
/// 1. any `Delete` on the root `doc.children` list, whatever ids it names;
/// 2. a `nodeName` insert or delete on a map that existed before the import;
/// 3. any op on the root `doc` map (its `children` key is the list itself).
///
/// The scan is also `Suspect` when the ops it read do not add up to the imported span, so an op it
/// did not see can never count as harmless.
///
/// Why each op not matched is harmless: `P` is a function of the elements of the current root
/// list and their `nodeName`. An element leaves the list only through a list delete (rule 1) or
/// by replacing the list, which is an op on the `doc` map (rule 3); a `List` has no move, and a
/// container cannot be re-parented. An element's `nodeName` changes only through a key insert or
/// delete on that map (rule 2), and a map created by the import cannot be an element that held
/// `P` up before it. Inserts into the list only add elements. Ops on any other container cannot
/// reach either input.
///
/// Cost: O(imported ops). Independent of document size.
///
/// # Errors
/// A failed root read (`editor_root_probe_failed`).
pub fn removal_scan(doc: &LoroDoc, before: &VersionVector) -> Result<RemovalScan, CollabError> {
    scan_imported_ops(doc, before, false)
}

use crate::reserved_roots::{SCAN_INCOMPLETE_REASON, SHADOW_ROOT_REASON};

/// One pass over the ops the import added (`before` is the op log version vector taken before it),
/// serving [`removal_scan`] and, with `check_roots`, the reserved-root rule.
///
/// The reserved-root rule (client write path only): an op whose container is a root container with
/// a reserved name (`reserved_roots`) and a type other than the expected one makes the update
/// invalid. Both the name and the type are read from the op's own container id, which the library
/// derives from the op itself: a root id is `(name, type)` and nothing else, and every container
/// inside a root can only have been created by an op that targets the root. So a shadow root cannot
/// gain content, or be referenced from a child, without an op that this rule sees. The update is
/// also invalid when the ops read do not add up to the imported span of every peer, so an op that
/// was not inspected can never pass. Both outcomes are errors, not verdicts: the caller discards
/// the engine, as for every other refusal after an import.
///
/// Without `check_roots` the scan stops at the first suspect op, as before. With it, it reads every
/// op, since a later op can still carry the violation.
///
/// Cost: O(imported ops), one comparison per op. Independent of document size.
///
/// # Errors
/// A failed root read, `reserved_root_container_type`, or `reserved_root_scan_incomplete`.
pub fn scan_imported_ops(doc: &LoroDoc, before: &VersionVector, check_roots: bool) -> Result<RemovalScan, CollabError> {
    scan_imported_ops_visiting(doc, before, check_roots, &mut |_| {})
}

/// [`scan_imported_ops`], showing every change it reads to `visit` ([`Scanned::Change`], before its
/// ops) and every op ([`Scanned::Op`], as `(peer, op, root list)`, where the root list is the id of
/// `doc.children` after the import). The incremental window check derives the units an update
/// touched, and whether the import was a strict causal extension of the base, from the same pass,
/// so the ops are decoded once.
///
/// With `check_roots` every op of the imported span is visited (a refusal ends the scan early, and
/// then the caller discards what `visit` saw). Without it the scan stops at the first suspect op.
///
/// # Errors
/// As [`scan_imported_ops`].
pub fn scan_imported_ops_visiting(
    doc: &LoroDoc,
    before: &VersionVector,
    check_roots: bool,
    visit: &mut dyn FnMut(Scanned<'_>),
) -> Result<RemovalScan, CollabError> {
    let refuse = |reason: &str| CollabError::OperationFailed { reason: reason.into() };
    let root_list = root_children_id(doc)?;
    let doc_map = ContainerID::new_root("doc", ContainerType::Map);
    let after = doc.oplog_vv();
    let mut removal = RemovalScan::None;
    for (&peer, &end) in after.iter() {
        let start = before.get(&peer).copied().unwrap_or(0);
        if start >= end {
            continue;
        }
        let mut covered: i64 = 0;
        for change in doc.export_json_in_id_span(IdSpan::new(peer, start, end)) {
            visit(Scanned::Change(&change));
            for op in change.ops {
                #[cfg(test)]
                crate::walk_counter::record_scanned_op();
                visit(Scanned::Op(peer, &op, root_list.as_ref()));
                if check_roots
                    && matches!(&op.container, ContainerID::Root { name, container_type }
                        if crate::reserved_roots::is_shadow_root(name, *container_type))
                {
                    return Err(refuse(SHADOW_ROOT_REASON));
                }
                if removal == RemovalScan::None
                    && (op.container == doc_map || op_removes_block(before, root_list.as_ref(), &op))
                {
                    removal = RemovalScan::Suspect;
                    if !check_roots {
                        return Ok(removal);
                    }
                }
                covered += i64::try_from(op.content.op_len()).unwrap_or(i64::MAX);
            }
        }
        if covered != i64::from(end) - i64::from(start) {
            if check_roots {
                return Err(refuse(SCAN_INCOMPLETE_REASON));
            }
            return Ok(RemovalScan::Suspect);
        }
    }
    Ok(removal)
}

/// Refuses a whole document that carries a shadow root, for a document that arrives from outside
/// (an uploaded package). Three facts, none of which a forged file can fake:
///
/// 1. the document is not shallow, so its op log is the whole history;
/// 2. no op in that history targets a reserved root of the wrong type
///    ([`scan_imported_ops`] from the empty version);
/// 3. no reserved name is listed in the state under the wrong type (the roots `LoroDoc::get_value`
///    lists), which is also what a root registered without ops would look like.
///
/// This is O(history) and meant for package ingestion, never for the timed write window.
///
/// # Errors
/// `reserved_root_container_type` or `reserved_root_scan_incomplete`, or a failed root read.
pub fn refuse_shadow_roots(doc: &LoroDoc) -> Result<(), CollabError> {
    let refuse = |reason: &str| CollabError::OperationFailed { reason: reason.into() };
    if doc.is_shallow() {
        return Err(refuse(SCAN_INCOMPLETE_REASON));
    }
    if let loro::LoroValue::Map(roots) = doc.get_value() {
        for (name, value) in roots.iter() {
            if matches!(value, loro::LoroValue::Container(ContainerID::Root { container_type, .. })
                if crate::reserved_roots::is_shadow_root(name, *container_type))
            {
                return Err(refuse(SHADOW_ROOT_REASON));
            }
        }
    }
    scan_imported_ops(doc, &VersionVector::default(), true).map(|_| ())
}

/// Rules 1 and 2 of [`removal_scan`] for one op.
fn op_removes_block(before: &VersionVector, root_list: Option<&ContainerID>, op: &JsonOp) -> bool {
    match &op.content {
        // Applied by position, so the ids named are metadata only: every delete is suspect.
        JsonOpContent::List(JsonListOp::Delete { .. }) => root_list == Some(&op.container),
        JsonOpContent::Map(JsonMapOp::Insert { key, .. } | JsonMapOp::Delete { key }) if key == "nodeName" => {
            !matches!(
                op.container,
                ContainerID::Normal { peer, counter, .. } if created_after(before, peer, counter)
            )
        }
        _ => false,
    }
}

/// Whether the document at `before` had a physical block map with an atom `nodeName`: the exact
/// value the pre-import floor walk's `P` signal would have returned.
///
/// Checks out `before`, reads, and returns to the latest state on every path, including when the
/// checkout or the read fails. Used only when the update touched something [`removal_scan`]
/// flagged. The cost is a checkout in both directions (the same machinery `LoroDoc::diff` uses),
/// O(document) and well above the common path.
///
/// # Errors
/// The first failure of the checkout, the read or the restore. The document is attached at its
/// latest state afterwards, or the error is `editor_floor_guard_restore_failed`.
pub fn physical_atoms_at(doc: &LoroDoc, before: &Frontiers) -> Result<bool, CollabError> {
    #[cfg(test)]
    crate::walk_counter::record_historical_atom_scan();
    let latest = doc.state_frontiers();
    let read = doc
        .checkout(before)
        .map_err(|error| CollabError::OperationFailed {
            reason: error.to_string(),
        })
        .and_then(|()| crate::engine::physical_atoms_present(doc));
    // Restore unconditionally: a failed checkout may already have detached the document.
    let restored = doc
        .try_checkout_to_latest()
        .map_err(|error| CollabError::OperationFailed {
            reason: error.to_string(),
        });
    let attached = if doc.is_detached() {
        doc.try_attach().map_err(|error| CollabError::OperationFailed {
            reason: error.to_string(),
        })
    } else {
        Ok(())
    };
    let atoms = read?;
    restored?;
    attached?;
    if doc.is_detached() || doc.state_frontiers() != latest {
        return Err(CollabError::OperationFailed {
            reason: "editor_floor_guard_restore_failed".into(),
        });
    }
    Ok(atoms)
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::indexing_slicing)]
mod tests {
    use super::*;
    use loro::{ExportMode, ID, LoroList, LoroMap};

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

    fn scan(base: &LoroDoc, update: &[u8]) -> RemovalScan {
        let replica = cold_copy(base);
        let before = replica.oplog_vv();
        replica.import(update).unwrap();
        removal_scan(&replica, &before).unwrap()
    }

    /// The scan reads every op of a multi-change update (several peers, text with multi-byte
    /// characters, marks, map keys) and clears it, which also proves the coverage check agrees
    /// with the library's op lengths.
    #[test]
    fn scan_covers_the_whole_imported_span() {
        let (base, _) = page(&["paragraph", "image"], 1);
        let update = update_from(&base, 2, |author| {
            let text = author.get_text("scratch");
            text.insert(0, "h\u{e9}llo \u{1f600} world").unwrap();
            text.mark(0..3, "bold", true).unwrap();
            author.commit();
            text.delete(1, 2).unwrap();
            author.commit();
            author.get_map("other").insert("k", 1).unwrap();
            author.commit();
            children(author)
                .insert_container(0, LoroMap::new())
                .unwrap()
                .insert("nodeName", "paragraph")
                .unwrap();
        });
        assert_eq!(scan(&base, &update), RemovalScan::None);
    }

    /// Every delete on the root list is suspect, whatever it names: the ids of a list delete are
    /// metadata, the library removes by position.
    #[test]
    fn every_root_list_delete_is_suspect() {
        let (base, _) = page(&["paragraph", "image", "paragraph"], 1);
        for index in 0..3 {
            let update = update_from(&base, 2, |author| children(author).delete(index, 1).unwrap());
            assert_eq!(scan(&base, &update), RemovalScan::Suspect, "delete at {index}");
        }
        // A delete in a list that is not the root list cannot lower the signal.
        let update = update_from(&base, 3, |author| {
            let nested = author.get_map("side").insert_container("l", LoroList::new()).unwrap();
            nested.insert(0, 1).unwrap();
            nested.delete(0, 1).unwrap();
        });
        assert_eq!(scan(&base, &update), RemovalScan::None);
    }

    /// `nodeName` ops are suspect on a pre-existing map and harmless on a map the update created.
    #[test]
    fn node_name_ops_are_judged_by_whether_the_map_existed() {
        let (base, _) = page(&["paragraph", "image"], 1);
        let retype = update_from(&base, 2, |author| {
            let Some(ValueOrContainer::Container(Container::Map(map))) = children(author).get(1) else {
                panic!("no map");
            };
            map.insert("nodeName", "paragraph").unwrap();
        });
        assert_eq!(scan(&base, &retype), RemovalScan::Suspect);
        let fresh = update_from(&base, 3, |author| {
            let map = children(author).insert_container(0, LoroMap::new()).unwrap();
            map.insert("nodeName", "paragraph").unwrap();
            map.insert("nodeName", "image").unwrap();
        });
        assert_eq!(scan(&base, &fresh), RemovalScan::None);
    }

    /// A failing checkout leaves the document attached at the state it had.
    #[test]
    fn physical_atoms_at_restores_the_document_when_the_checkout_fails() {
        let (doc, _) = page(&["paragraph", "image"], 1);
        let latest = doc.state_frontiers();
        let unknown = Frontiers::from(ID::new(999, 5));
        let outcome = physical_atoms_at(&doc, &unknown);
        assert!(outcome.is_err(), "a checkout to an unknown id must fail: {outcome:?}");
        assert!(!doc.is_detached());
        assert_eq!(doc.state_frontiers(), latest);
        // The document is still usable.
        children(&doc).insert_container(0, LoroMap::new()).unwrap();
        doc.commit();
        assert!(physical_atoms_at(&doc, &latest).unwrap());
        assert!(!doc.is_detached());
    }

    /// A read that fails at the checked-out state (here a non-map element in the list) still
    /// returns the document to its latest state.
    #[test]
    fn physical_atoms_at_restores_the_document_when_the_read_fails() {
        let (doc, _) = page(&["paragraph", "image"], 1);
        children(&doc).insert(0, 5).unwrap();
        doc.commit();
        let broken = doc.state_frontiers();
        children(&doc).delete(0, 1).unwrap();
        doc.commit();
        let latest = doc.state_frontiers();
        let outcome = physical_atoms_at(&doc, &broken);
        assert!(
            matches!(&outcome, Err(CollabError::OperationFailed { reason }) if reason == "editor_doc_unsupported_block_container"),
            "{outcome:?}"
        );
        assert!(!doc.is_detached());
        assert_eq!(doc.state_frontiers(), latest);
    }
}
