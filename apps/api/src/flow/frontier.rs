//! Semantic frontier comparison for every API-side frontier check.
//!
//! A frontier is an encoded Loro version vector. Releases up to 0.2.539 emitted it in hash-map
//! iteration order, so one causal version can be stored under several byte strings: a writer that
//! imported updates incrementally and a reader that rebuilt the same document from snapshot plus
//! tail can disagree bytewise while naming the same version. New frontiers are canonical
//! (peer-sorted), but rows written earlier keep their original bytes. Every equality check between
//! frontiers therefore goes through this module and compares decoded version vectors.
//!
//! An undecodable frontier is never equal to anything: it is a typed error.

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use collab_core::LoroCollabEngine;

use crate::error::ApiError;

/// Whether two server-held frontiers (stored columns, receipts, engine output) name the same
/// version.
///
/// # Errors
/// `server_rejected` (internal) when either side does not decode; stored frontiers are always
/// engine-produced, so a decode failure is corruption, logged and never reported as a match.
pub fn same(left: &[u8], right: &[u8]) -> Result<bool, ApiError> {
    LoroCollabEngine::frontiers_equivalent(left, right).map_err(|error| {
        tracing::error!(%error, "a server-held Flow frontier failed to decode");
        ApiError::internal()
    })
}

/// Whether a caller-supplied expected frontier names the same version as a server-held one.
///
/// # Errors
/// `invalid_update` when the caller's frontier does not decode (it can never match, and is not a
/// concurrency conflict), or [`same`]'s error when the server-held side does not decode.
pub fn client_matches(client: &[u8], server: &[u8]) -> Result<bool, ApiError> {
    if let Err(error) = LoroCollabEngine::canonical_frontier(client) {
        tracing::debug!(%error, "rejected an undecodable caller-supplied Flow frontier");
        return Err(ApiError::invalid_update("expected frontier is not a valid frontier"));
    }
    same(client, server)
}

/// [`same`] over two server-held frontiers carried as standard base64 (receipts, previews).
///
/// # Errors
/// `server_rejected` when either side is not base64 or does not decode as a frontier.
pub fn same_base64(left: &str, right: &str) -> Result<bool, ApiError> {
    same(&server_base64(left)?, &server_base64(right)?)
}

/// [`client_matches`] for a caller-supplied base64 frontier against a server-held base64 one.
///
/// # Errors
/// `invalid_update` when the caller's value is not base64 of a frontier; `server_rejected` when
/// the server-held value is not.
pub fn client_matches_base64(client: &str, server: &str) -> Result<bool, ApiError> {
    let client = STANDARD
        .decode(client)
        .map_err(|_| ApiError::invalid_update("expected frontier is not a valid frontier"))?;
    client_matches(&client, &server_base64(server)?)
}

fn server_base64(value: &str) -> Result<Vec<u8>, ApiError> {
    STANDARD.decode(value).map_err(|error| {
        tracing::error!(%error, "a server-held Flow frontier is not base64");
        ApiError::internal()
    })
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;
    use crate::error::ApiErrorKind;

    /// Loro's version-vector wire format for small values: entry count, then `peer`,
    /// zigzag(`counter`) pairs, each a single-byte varint below 64.
    fn wire(entries: &[(u8, u8)]) -> Vec<u8> {
        let mut out = vec![u8::try_from(entries.len()).unwrap()];
        for (peer, counter) in entries {
            out.extend([*peer, counter * 2]);
        }
        out
    }

    #[test]
    fn canonical_and_legacy_orders_of_one_version_match_in_both_directions() {
        let canonical = wire(&[(31, 4), (47, 9)]);
        let legacy = wire(&[(47, 9), (31, 4)]);
        assert_ne!(canonical, legacy);
        assert!(same(&canonical, &legacy).unwrap());
        assert!(same(&legacy, &canonical).unwrap());
        assert!(client_matches(&canonical, &legacy).unwrap());
        assert!(client_matches(&legacy, &canonical).unwrap());
        assert!(!same(&canonical, &wire(&[(31, 4), (47, 10)])).unwrap());
        assert!(!client_matches(&legacy, &wire(&[(31, 4)])).unwrap());
    }

    #[test]
    fn undecodable_frontiers_are_typed_errors_never_matches() {
        let valid = wire(&[(31, 4)]);
        for garbage in [b"fenced".as_slice(), &[2], &[1, 31, 8, 0]] {
            assert_eq!(same(garbage, &valid).unwrap_err().kind(), ApiErrorKind::ServerRejected);
            assert_eq!(same(&valid, garbage).unwrap_err().kind(), ApiErrorKind::ServerRejected);
            assert_eq!(same(garbage, garbage).unwrap_err().kind(), ApiErrorKind::ServerRejected);
            assert_eq!(
                client_matches(garbage, &valid).unwrap_err().kind(),
                ApiErrorKind::InvalidUpdate
            );
            assert_eq!(
                client_matches(garbage, garbage).unwrap_err().kind(),
                ApiErrorKind::InvalidUpdate
            );
            assert_eq!(
                client_matches(&valid, garbage).unwrap_err().kind(),
                ApiErrorKind::ServerRejected
            );
        }
    }

    #[test]
    fn base64_variants_compare_versions_and_type_their_failures() {
        let canonical = STANDARD.encode(wire(&[(31, 4), (47, 9)]));
        let legacy = STANDARD.encode(wire(&[(47, 9), (31, 4)]));
        assert!(same_base64(&canonical, &legacy).unwrap());
        assert!(client_matches_base64(&legacy, &canonical).unwrap());
        assert!(!client_matches_base64(&STANDARD.encode(wire(&[(31, 4)])), &canonical).unwrap());
        for garbage in ["not-base64!!", "ZmVuY2Vk"] {
            assert_eq!(
                client_matches_base64(garbage, &canonical).unwrap_err().kind(),
                ApiErrorKind::InvalidUpdate
            );
            assert_eq!(
                client_matches_base64(&canonical, garbage).unwrap_err().kind(),
                ApiErrorKind::ServerRejected
            );
            assert_eq!(
                same_base64(garbage, &legacy).unwrap_err().kind(),
                ApiErrorKind::ServerRejected
            );
        }
    }

    /// Stored frontiers written before canonical encoding keep their byte order. A document whose
    /// stored head is the legacy order of the canonical snapshot frontier must still load, and a
    /// client acknowledging the canonical bytes must still match it. Linux is not required: these
    /// paths read and verify persisted state without the isolated write worker.
    #[tokio::test]
    async fn persisted_legacy_order_frontiers_match_canonical_ones_at_database_sites() {
        use crate::{routes::context::tenant_fixture::exec, scratch_or_skip};
        use collab_core::CollabEngine;
        use sea_orm::{ConnectionTrait, TransactionTrait};
        use uuid::Uuid;

        let scratch = scratch_or_skip!(&format!("frontier_order_{}", Uuid::new_v4().simple()));
        let db = &scratch.db;
        for component in ["api", "worker", "maintenance"] {
            super::super::scope_runtime::renew(db, Uuid::new_v4(), component)
                .await
                .unwrap();
        }
        db.execute_unprepared("UPDATE flow_runtime_capabilities SET minimum_reader_version=4 WHERE singleton")
            .await
            .unwrap();
        let (owner, scope, page, document) = (Uuid::new_v4(), Uuid::new_v4(), Uuid::new_v4(), Uuid::new_v4());
        exec(db,"INSERT INTO users(id,email,password_hash,name,role,is_active) VALUES($1,$2,'!','Frontier fixture','user',true)",vec![owner.into(),format!("{owner}@frontier.test").into()]).await;
        exec(
            db,
            "INSERT INTO flow_scopes(id,kind,owner_user_id) VALUES($1,'personal',$2)",
            vec![scope.into(), owner.into()],
        )
        .await;
        exec(
            db,
            "INSERT INTO flow_scope_settings(scope_id) VALUES($1)",
            vec![scope.into()],
        )
        .await;
        let tx = db.begin().await.unwrap();
        let navigator = Uuid::new_v4();
        super::super::scope::insert_object_with_identity(
            &tx,
            scope,
            owner,
            "navigator",
            None,
            "",
            None,
            navigator,
            Uuid::new_v4(),
        )
        .await
        .unwrap();
        super::super::scope::insert_object_with_identity(
            &tx,
            scope,
            owner,
            "page",
            Some(navigator),
            "Frontier Page",
            None,
            page,
            document,
        )
        .await
        .unwrap();
        tx.commit().await.unwrap();

        // Two editing peers, so the version vector has two entries whose order can differ.
        let mut first = collab_core::LoroCollabEngine::new_empty(1);
        first.set_title("first").unwrap();
        let mut second = collab_core::LoroCollabEngine::new_empty(2);
        second.import_update(&first.export_snapshot().unwrap()).unwrap();
        second.set_title("second").unwrap();
        let snapshot = second.export_snapshot().unwrap();
        let canonical = second.frontier().as_bytes().to_vec();
        let legacy = {
            // count, then two (peer, counter) pairs of single-varint fields each.
            let (&count, payload) = canonical.split_first().unwrap();
            assert_eq!(count, 2);
            let fields = payload.split_inclusive(|byte| byte & 0x80 == 0).collect::<Vec<_>>();
            let [peer_a, counter_a, peer_b, counter_b] = fields.as_slice() else {
                panic!("expected two peer/counter pairs");
            };
            [&[count], *peer_b, *counter_b, *peer_a, *counter_a].concat()
        };
        assert_ne!(legacy, canonical);
        let store = |snapshot_frontier: Vec<u8>, head_frontier: Vec<u8>| {
            exec(
                db,
                "UPDATE collab_documents SET snapshot=$2,snapshot_checksum=encode(digest($2,'sha256'),'hex'),byte_count=octet_length($2),snapshot_frontier=$3,head_frontier=$4 WHERE id=$1",
                vec![
                    document.into(),
                    snapshot.clone().into(),
                    snapshot_frontier.into(),
                    head_frontier.into(),
                ],
            )
        };

        // Snapshot frontier canonical, stored head legacy: the chain check is semantic.
        store(canonical.clone(), legacy.clone()).await;
        let boot = super::super::collab::bootstrap::load(db, document).await.unwrap();
        assert_eq!(boot.head_frontier, legacy, "stored bytes are preserved, not rewritten");
        assert!(same(&boot.head_frontier, second.frontier().as_bytes()).unwrap());
        // A client acknowledging the canonical bytes of the stored legacy head is accepted.
        assert!(
            super::super::collab::compaction::record_client_ack(
                db,
                document,
                "canonical-client",
                boot.head_seq,
                &canonical
            )
            .await
            .unwrap()
        );
        // An undecodable client frontier is refused, never matched.
        assert!(
            !super::super::collab::compaction::record_client_ack(
                db,
                document,
                "garbage-client",
                boot.head_seq,
                b"fenced"
            )
            .await
            .unwrap()
        );

        // An undecodable stored head fails closed with a typed error, never a panic or a match.
        store(canonical.clone(), b"fenced".to_vec()).await;
        let error = super::super::collab::bootstrap::load(db, document).await.unwrap_err();
        assert_eq!(error.kind(), ApiErrorKind::ResyncRequired);

        // A genuinely different version is still a mismatch.
        store(canonical, first.frontier().as_bytes().to_vec()).await;
        let error = super::super::collab::bootstrap::load(db, document).await.unwrap_err();
        assert_eq!(error.kind(), ApiErrorKind::ResyncRequired);
        scratch.drop_self().await;
    }
}
