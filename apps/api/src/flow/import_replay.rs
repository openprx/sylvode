//! The isolated validation of the documents of an uploaded Flow package.
//!
//! The snapshot and the update tail of an uploaded document are client-provided CRDT bytes. They are
//! never decoded in the API process: every document goes through the import operation of the
//! isolated worker (`collab_core::isolation::isolated_import_observed`), which loads, replays,
//! scans and projects them under the CPU, wall and memory ceilings of the import window, and
//! checks every content limit (`check_snapshot`, reserved roots, tree references) before anything is
//! stored. An import is one logical write of a whole document, so a package too large for the
//! ceilings is refused as `limit_exceeded` naming the limit, never accepted.
//!
//! The host then re-derives what it stores from the worker's validated head (the snapshot the
//! worker returned), not from the uploaded bytes, and refuses the import when its own projection of
//! that head disagrees with the worker's report. Both the workspace import
//! ([`super::package_import`]) and the personal-scope import ([`super::scope_import`]) validate
//! through this module, outside of any database transaction.
//!
//! # Every way content enters a Flow document
//!
//! Metered by the isolated worker before it becomes a base: collab updates (write window), REST
//! content commands, plugin and bot writes (the same write window), page transfers between
//! workspaces (built from the server's own documents, applied through `hydrate_and_apply`), restore
//! (the inverse of accepted history, applied through `hydrate_and_apply`), scope import and
//! workspace package import (the import window of this module). Not decoded from client bytes at
//! all: the portable page package (the server rebuilds the page from validated JSON blocks), the
//! portable graph package (it rebuilds each version from JSON blocks; its replay is bounded by
//! `portable_graph_package::MAX_REPLAY_MARK_EDITS`), server-built documents (records, collections,
//! scopes, page creation), snapshot and compaction (re-export of accepted heads), export (reads this
//! server's own storage), the projection rebuild and the other maintenance reads (this server's
//! own storage). The bytes of the last group only ever come from this server's own storage, so
//! nothing there is metered by the import window.

use base64::{Engine as _, engine::general_purpose::STANDARD};
use collab_core::isolation::wire::{ReplayDiffRequest, ReplayDiffResult};
use collab_core::isolation::{IsolatedApplyError, WorkerObservation};
use collab_core::limits::{DocumentLimits, check_snapshot};
use collab_core::{CollabEngine, CollabError, LoroCollabEngine, SemanticSnapshot};
use uuid::Uuid;

use super::package::{PackageSemanticModel, semantic_model};
use super::package_import::{PackageObject, PackagePlan, member_bytes};
use super::query::{ReplayBudget, isolated_replay_error};
use crate::error::ApiError;

#[cfg(test)]
tokio::task_local! {
    // Explicit developer unit-test isolation boundary only. No production environment/request
    // flag can populate this proof map; production always executes the isolated worker below.
    pub(crate) static TEST_VERIFIED_REPLAY: std::collections::BTreeMap<Uuid, ReplayDiffResult>;
}

/// One document of a package after the isolated import window and the host's own verification:
/// what the importing transaction stores, derived from the validated head and nothing else.
pub(crate) struct ValidatedDocument {
    pub(crate) source: Uuid,
    pub(crate) snapshot: Vec<u8>,
    pub(crate) frontier: Vec<u8>,
    pub(crate) semantic: SemanticSnapshot,
    pub(crate) title: String,
}

/// Validates every document of `plan` in the isolated worker, in package order, one worker per
/// document.
///
/// # Errors
/// `limit_exceeded` naming the limit a document or the import window reached, the typed refusals
/// of a shadow root or an unresolved tree reference, `checksum_mismatch` for a document that
/// disagrees with its manifest, and the busy or defect answers of the worker boundary.
pub(crate) async fn validate_documents(package: &[u8], plan: &PackagePlan) -> Result<Vec<ValidatedDocument>, ApiError> {
    let model = semantic_model(&plan.verified.manifest)?;
    let mut validated = Vec::with_capacity(plan.objects.len());
    for object in &plan.objects {
        validated.push(validate_document(package, plan, object, model).await?);
    }
    Ok(validated)
}

async fn validate_document(
    package: &[u8],
    plan: &PackagePlan,
    object: &PackageObject,
    model: PackageSemanticModel,
) -> Result<ValidatedDocument, ApiError> {
    let snapshot = member_bytes(
        package,
        &format!("documents/{}/snapshot.bin", object.source_document_id),
    )?;
    let frontier = STANDARD
        .decode(&object.accepted_frontier)
        .map_err(|_| ApiError::unsupported_format("invalid accepted frontier"))?;
    let request = ReplayDiffRequest {
        snapshot_frontier: frontier.clone(),
        updates: Vec::new(),
        from_frontier: frontier.clone(),
        to_frontier: frontier.clone(),
        package_updates_base64: Some(update_tail(package, plan, object)?),
        package_semantic_model: Some(model.wire_reader().to_owned()),
    };
    let report = replay_in_worker(snapshot, request, object.source_object_id).await?;
    let manifest_hash = object.semantic_hash.clone();
    let verified =
        tokio::task::spawn_blocking(move || verify_validated_head(&report, &frontier, model, &manifest_hash))
            .await
            .map_err(|_| ApiError::internal())??;
    Ok(ValidatedDocument {
        source: object.source_object_id,
        snapshot: verified.snapshot,
        frontier: verified.frontier,
        semantic: verified.semantic,
        title: verified.title,
    })
}

/// The update members of one document, in sequence, each base64-encoded for the worker request.
fn update_tail(package: &[u8], plan: &PackagePlan, object: &PackageObject) -> Result<Vec<String>, ApiError> {
    let prefix = format!("documents/{}/updates/", object.source_document_id);
    let mut updates = plan
        .verified
        .manifest
        .members
        .iter()
        .filter(|member| member.kind == "update" && member.path.starts_with(&prefix))
        .map(|member| {
            let seq = member
                .path
                .strip_prefix(&prefix)
                .and_then(|tail| tail.strip_suffix(".bin"))
                .and_then(|value| value.parse::<i64>().ok())
                .ok_or_else(|| ApiError::unsupported_format("invalid package update sequence"))?;
            Ok((seq, member.path.as_str()))
        })
        .collect::<Result<Vec<_>, ApiError>>()?;
    updates.sort_unstable_by_key(|(seq, _)| *seq);
    let first = object
        .accepted_seq
        .saturating_sub(i64::try_from(updates.len()).unwrap_or(i64::MAX))
        .saturating_add(1);
    let mut encoded = Vec::with_capacity(updates.len());
    for (index, (seq, path)) in updates.into_iter().enumerate() {
        if seq < 1 || seq != first.saturating_add(i64::try_from(index).unwrap_or(i64::MAX)) {
            return Err(ApiError::checksum_mismatch("package update tail has a sequence gap"));
        }
        encoded.push(STANDARD.encode(member_bytes(package, path)?));
    }
    Ok(encoded)
}

/// Runs the import operation of the worker for one document and maps its refusals.
async fn replay_in_worker(
    snapshot: Vec<u8>,
    request: ReplayDiffRequest,
    object: Uuid,
) -> Result<ReplayDiffResult, ApiError> {
    #[cfg(test)]
    if let Ok(Some(result)) = TEST_VERIFIED_REPLAY.try_with(|map| map.get(&object).cloned()) {
        return Ok(result);
    }
    let (result, observation) =
        tokio::task::spawn_blocking(move || collab_core::isolation::isolated_import_observed(&snapshot, &request))
            .await
            .map_err(|_| ApiError::internal())?;
    result.map_err(|error| import_error(error, &observation, object))
}

/// The refusal an import window gives, as a typed error. A content limit is `limit_exceeded`
/// naming its kind and not retryable; a reserved root of the wrong type or a tree reference that
/// would panic every client is `invalid_update` with its reason; a document that does not replay
/// to its accepted frontier is `checksum_mismatch`; the ceilings, the busy answers and the defects
/// are those of every replay (`query::isolated_replay_error`).
fn import_error(error: IsolatedApplyError, observation: &WorkerObservation, object: Uuid) -> ApiError {
    match &error {
        // A content limit (`check_snapshot`) or the size of an update of the tail: `limit_exceeded`
        // naming the kind, not retryable.
        IsolatedApplyError::Collab(collab) if collab.limit_kind().is_some() => super::command::map_collab_error(collab),
        IsolatedApplyError::Collab(CollabError::OperationFailed { reason })
            if reason == collab_core::reserved_roots::SHADOW_ROOT_REASON
                || reason == collab_core::reserved_roots::SCAN_INCOMPLETE_REASON
                || collab_core::tree_refs::is_tree_reference_reason(reason) =>
        {
            super::package::shadow_root_refusal(reason)
        }
        _ => isolated_replay_error(error, observation, object, ReplayBudget::Import, || {
            ApiError::checksum_mismatch("package snapshot cannot be reconstructed")
        }),
    }
}

/// What the host derived from the validated head of one document.
struct VerifiedHead {
    snapshot: Vec<u8>,
    frontier: Vec<u8>,
    semantic: SemanticSnapshot,
    title: String,
}

/// The divergence between the worker's report and the host's own projection of the head it
/// returned: refused as a defect, never stored, and never retryable.
fn divergence(what: &'static str) -> ApiError {
    tracing::error!(
        what,
        "package import: the host's projection of the validated head contradicts the worker"
    );
    ApiError::server_rejected("import_validation_divergence")
}

/// The host's side of the import: loads the head the worker validated, checks it again with the
/// rules the worker applied (reserved roots, tree references, content limits, the accepted
/// frontier), re-projects it and compares the projection, the title, the reference verdict and the
/// package fingerprint with the worker's report and the manifest. The head is bounded by what the
/// worker's window accepted; nothing here reads the uploaded bytes.
fn verify_validated_head(
    report: &ReplayDiffResult,
    frontier: &[u8],
    model: PackageSemanticModel,
    manifest_hash: &str,
) -> Result<VerifiedHead, ApiError> {
    let head = STANDARD
        .decode(
            report
                .validated_head_snapshot_base64
                .as_deref()
                .ok_or_else(|| ApiError::unsupported_protocol("isolated worker lacks package replay support"))?,
        )
        .map_err(|_| ApiError::internal())?;
    super::collab::bootstrap::check_bootstrap_decoded_bytes(u64::try_from(head.len()).unwrap_or(u64::MAX))?;
    let engine =
        LoroCollabEngine::load(&head).map_err(|_| ApiError::checksum_mismatch("package head cannot be decoded"))?;
    super::package::require_no_shadow_roots(&engine)?;
    if !engine.matches_frontier(frontier) {
        return Err(divergence("frontier"));
    }
    let semantic = engine.semantic_snapshot().map_err(|_| divergence("projection"))?;
    if semantic != report.to_snapshot || check_snapshot(&semantic, &DocumentLimits::default()).is_err() {
        return Err(divergence("projection"));
    }
    let title = engine.title().map_err(|_| divergence("title"))?;
    if title != report.to_title {
        return Err(divergence("title"));
    }
    let reported_hash = reported_fingerprint(report, model)?;
    if reported_hash != manifest_hash {
        return Err(ApiError::checksum_mismatch(
            "package semantic fingerprint disagrees with its versioned reader",
        ));
    }
    if model.semantic_hash(&engine)? != reported_hash {
        return Err(divergence("fingerprint"));
    }
    match report.package_reference_remap_required {
        Some(required)
            if required
                == collab_core::isolation::wire::package_reference_remap_required(&engine)
                    .map_err(|_| divergence("references"))? =>
        {
            if required {
                return Err(super::package::inner_reference_refusal());
            }
        }
        Some(_) => return Err(divergence("references")),
        None => {
            return Err(ApiError::unsupported_protocol(
                "isolated worker lacks package reference validation",
            ));
        }
    }
    Ok(VerifiedHead {
        snapshot: head,
        frontier: frontier.to_vec(),
        semantic,
        title,
    })
}

/// The fingerprint the worker read for `model`: the one reader's hash, or the hash of the
/// projection itself for the current model.
fn reported_fingerprint(report: &ReplayDiffResult, model: PackageSemanticModel) -> Result<String, ApiError> {
    let missing = |reader: &'static str| {
        ApiError::unsupported_protocol(format!("isolated worker lacks the {reader} package fingerprint reader"))
    };
    match model {
        PackageSemanticModel::V4 => report.package_semantic_hash_v4.clone().ok_or_else(|| missing("v4")),
        PackageSemanticModel::V5 => Ok(report.to_snapshot.semantic_hash()),
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;

    fn replayed(head: &[u8]) -> ReplayDiffResult {
        let engine = LoroCollabEngine::load(head).unwrap();
        ReplayDiffResult {
            semantic_diff: collab_core::SemanticDiff::default(),
            to_snapshot: engine.semantic_snapshot().unwrap(),
            package_semantic_hash_v4: None,
            from_title: String::new(),
            to_title: engine.title().unwrap(),
            package_reference_remap_required: Some(false),
            validated_head_snapshot_base64: Some(STANDARD.encode(head)),
        }
    }

    fn fixture_head() -> Vec<u8> {
        include_bytes!("../../../../frontend/tests/fixtures/editor-doc-v1/browser.snapshot").to_vec()
    }

    fn verified(report: &ReplayDiffResult) -> Result<VerifiedHead, ApiError> {
        let engine = LoroCollabEngine::load(&fixture_head()).unwrap();
        let frontier = engine.frontier().as_bytes().to_vec();
        let hash = report.to_snapshot.semantic_hash();
        verify_validated_head(report, &frontier, PackageSemanticModel::V5, &hash)
    }

    /// The host re-reads the head the worker returned: it is refused when it carries a reserved
    /// root of the wrong type, and accepted when it does not.
    #[test]
    fn a_validated_head_with_a_shadow_root_is_refused_by_the_host() {
        let clean = fixture_head();
        assert!(verified(&replayed(&clean)).is_ok());
        let doc = loro::LoroDoc::new();
        doc.import(&clean).unwrap();
        doc.set_peer_id(9_200_001).unwrap();
        doc.get_text("flow_editor_structure_meta").insert(0, "x").unwrap();
        doc.commit();
        let shadowed = doc.export(loro::ExportMode::Snapshot).unwrap();
        let error = verified(&replayed(&shadowed))
            .err()
            .expect("a shadow root must be refused");
        assert_eq!(error.kind(), crate::error::ApiErrorKind::InvalidUpdate);
        assert!(
            format!("{error:?}").contains("reserved_root_container_type"),
            "{error:?}"
        );
        // A head the worker did not export has nothing to check.
        let mut missing = replayed(&clean);
        missing.validated_head_snapshot_base64 = None;
        assert!(verified(&missing).is_err());
    }

    /// The host's own projection contradicts a worker that reports another document, another
    /// title or another reference verdict: the import is refused, never stored.
    #[test]
    fn a_report_that_the_hosts_projection_contradicts_is_refused() {
        let head = fixture_head();
        let other = include_bytes!("../../../../frontend/tests/fixtures/flow-navigator-order/browser-reedit.snapshot");
        let mut wrong_projection = replayed(&head);
        wrong_projection.to_snapshot = replayed(other).to_snapshot;
        let mut wrong_title = replayed(&head);
        wrong_title.to_title = "another title".into();
        let mut wrong_references = replayed(&head);
        wrong_references.package_reference_remap_required = Some(true);
        for (label, report) in [
            ("projection", wrong_projection),
            ("title", wrong_title),
            ("references", wrong_references),
        ] {
            let engine = LoroCollabEngine::load(&head).unwrap();
            let frontier = engine.frontier().as_bytes().to_vec();
            let hash = replayed(&head).to_snapshot.semantic_hash();
            let error = verify_validated_head(&report, &frontier, PackageSemanticModel::V5, &hash)
                .err()
                .unwrap_or_else(|| panic!("{label} must be refused"));
            assert_eq!(
                error.kind(),
                crate::error::ApiErrorKind::ServerRejected,
                "{label}: {error:?}"
            );
        }
        // A head that is not the accepted frontier is the same refusal.
        let error = verify_validated_head(
            &replayed(&head),
            LoroCollabEngine::new_empty(1).frontier().as_bytes(),
            PackageSemanticModel::V5,
            &replayed(&head).to_snapshot.semantic_hash(),
        )
        .err()
        .expect("another frontier must be refused");
        assert_eq!(error.kind(), crate::error::ApiErrorKind::ServerRejected);
        // A manifest that names another fingerprint is a checksum mismatch.
        let engine = LoroCollabEngine::load(&head).unwrap();
        let error = verify_validated_head(
            &replayed(&head),
            engine.frontier().as_bytes(),
            PackageSemanticModel::V5,
            "not the fingerprint",
        )
        .err()
        .expect("a wrong manifest fingerprint must be refused");
        assert_eq!(error.kind(), crate::error::ApiErrorKind::ChecksumMismatch);
    }

    #[test]
    fn the_refusals_of_the_import_window_are_typed_and_name_their_limit() {
        let observation = WorkerObservation::default();
        let limit = import_error(
            IsolatedApplyError::Collab(CollabError::LimitExceeded {
                limit_kind: "text_block_chars",
                limit: 100_000,
                observed: 100_001,
            }),
            &observation,
            Uuid::nil(),
        );
        assert_eq!(limit.kind(), crate::error::ApiErrorKind::LimitExceeded);
        let ApiError::Typed { details, .. } = &limit else {
            panic!("a typed error");
        };
        let details = details.as_ref().expect("details");
        assert_eq!(details["limit_kind"], "text_block_chars");
        assert_eq!(details["limit"], 100_000);
        assert_eq!(details["observed"], 100_001);
        for reason in [
            "tree_parent_missing",
            "tree_parent_cycle",
            "reserved_root_container_type",
        ] {
            let error = import_error(
                IsolatedApplyError::Collab(CollabError::OperationFailed { reason: reason.into() }),
                &observation,
                Uuid::nil(),
            );
            assert_eq!(error.kind(), crate::error::ApiErrorKind::InvalidUpdate, "{reason}");
            assert!(format!("{error:?}").contains(reason), "{reason}: {error:?}");
        }
        let mismatch = import_error(
            IsolatedApplyError::Collab(CollabError::OperationFailed {
                reason: "package accepted frontier does not match replayed head".into(),
            }),
            &observation,
            Uuid::nil(),
        );
        assert_eq!(mismatch.kind(), crate::error::ApiErrorKind::ChecksumMismatch);
        // A ceiling kill names the import budget, not the write window's.
        let cpu = import_error(IsolatedApplyError::CpuCeiling, &observation, Uuid::nil());
        let ApiError::Typed { details, .. } = &cpu else {
            panic!("a typed error");
        };
        let details = details.as_ref().expect("details");
        assert_eq!(details["limit_kind"], "decode_apply_cpu_ms");
        assert_eq!(details["limit"], collab_core::isolation::IMPORT_DECODE_CPU_MS_MAX);
        let memory = import_error(IsolatedApplyError::MemoryCeiling, &observation, Uuid::nil());
        let ApiError::Typed { details, .. } = &memory else {
            panic!("a typed error");
        };
        assert_eq!(
            details.as_ref().expect("details")["limit"],
            collab_core::isolation::IMPORT_ISOLATED_MEMORY_BYTES_MAX
        );
        // Off Linux the boundary is absent and the import is refused as a platform defect.
        let unsupported = import_error(
            IsolatedApplyError::UnsupportedPlatform { os: "macos" },
            &observation,
            Uuid::nil(),
        );
        assert_eq!(unsupported.kind(), crate::error::ApiErrorKind::ServerRejected);
        assert!(format!("{unsupported:?}").contains("isolated_apply_unsupported_platform"));
    }
}
