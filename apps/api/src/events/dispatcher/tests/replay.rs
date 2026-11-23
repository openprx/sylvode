//! Dispatcher database tests: requeue and replay.

use super::*;

/// `events-v1.md`/`gate-commands.md` `requeue_failed`: revives `event_deliveries` rows stuck
/// at `status='failed'` back into the live queue, clearing `document_id` on every revived row
/// (content or not) so a revived content delivery cannot collide with -- or jump the FIFO
/// queue ahead of -- a live `pending` row for the same document via the partial coalescing
/// unique index, and never revives `cancelled` rows (`subscriber_gone`'s deliberate terminal
/// state, not a retryable failure).
#[tokio::test]
async fn requeue_failed_revives_failed_content_rows_clears_document_id_and_never_revives_cancelled() {
    let scratch = scratch_or_skip!("requeue-failed");
    let workspace_id = seed_workspace(&scratch.db).await;
    let other_workspace_id = seed_workspace(&scratch.db).await;
    let document_id = Uuid::new_v4();

    async fn source_event(db: &DatabaseConnection, workspace_id: Uuid) -> Uuid {
        insert_business_event(
            db,
            BusinessEventInput {
                workspace_id,
                project_id: None,
                event_type: "flow.content.accepted".to_string(),
                aggregate_type: "flow_document".to_string(),
                aggregate_id: Uuid::new_v4().to_string(),
                actor_id: None,
                source: json!({ "surface": "system" }),
                payload: json!({}),
                metadata: json!({}),
                correlation_id: None,
                causation_id: None,
                idempotency_key: None,
            },
        )
        .await
        .expect("source business event inserts")
    }

    #[allow(clippy::too_many_arguments)]
    async fn insert_delivery(
        db: &DatabaseConnection,
        workspace_id: Uuid,
        event_id: Uuid,
        document_id: Option<Uuid>,
        status: &str,
    ) -> Uuid {
        let id = Uuid::new_v4();
        exec(
            db,
            "INSERT INTO event_deliveries \
             (id, event_id, workspace_id, subscriber_kind, subscriber_id, document_id, status, \
              attempts, terminated_at, last_error_code) \
             VALUES ($1, $2, $3, 'webhook', $4, $5, $6, 3, now(), 'webhook_5xx')",
            vec![
                id.into(),
                event_id.into(),
                workspace_id.into(),
                Uuid::new_v4().into(),
                document_id.into(),
                status.into(),
            ],
        )
        .await;
        id
    }

    let failed_content = insert_delivery(
        &scratch.db,
        workspace_id,
        source_event(&scratch.db, workspace_id).await,
        Some(document_id),
        "failed",
    )
    .await;
    let failed_non_content = insert_delivery(
        &scratch.db,
        workspace_id,
        source_event(&scratch.db, workspace_id).await,
        None,
        "failed",
    )
    .await;
    let cancelled = insert_delivery(
        &scratch.db,
        workspace_id,
        source_event(&scratch.db, workspace_id).await,
        Some(document_id),
        "cancelled",
    )
    .await;
    // A `failed` row in a *different* workspace must not be touched by this call.
    let other_workspace_failed = insert_delivery(
        &scratch.db,
        other_workspace_id,
        source_event(&scratch.db, other_workspace_id).await,
        None,
        "failed",
    )
    .await;

    let revived = requeue_failed(&scratch.db, workspace_id, Timestamp::now())
        .await
        .expect("requeue_failed runs");
    assert_eq!(
        revived, 2,
        "exactly the two failed rows in this workspace must be revived"
    );

    #[derive(sea_orm::FromQueryResult)]
    struct Row {
        status: String,
        document_id: Option<Uuid>,
        attempts: i32,
        terminated_at: Option<Timestamp>,
    }
    async fn fetch(db: &DatabaseConnection, id: Uuid) -> Row {
        Row::find_by_statement(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT status, document_id, attempts, terminated_at FROM event_deliveries WHERE id = $1",
            vec![id.into()],
        ))
        .one(db)
        .await
        .expect("query runs")
        .expect("row exists")
    }

    let revived_content = fetch(&scratch.db, failed_content).await;
    assert_eq!(revived_content.status, "pending");
    assert_eq!(
        revived_content.document_id, None,
        "document_id must be cleared on revival so this row cannot collide with -- or jump \
         the FIFO queue ahead of -- a live pending row for the same document"
    );
    assert_eq!(revived_content.attempts, 0);
    assert_eq!(revived_content.terminated_at, None);

    let revived_non_content = fetch(&scratch.db, failed_non_content).await;
    assert_eq!(revived_non_content.status, "pending");
    assert_eq!(revived_non_content.document_id, None);

    let cancelled_row = fetch(&scratch.db, cancelled).await;
    assert_eq!(
        cancelled_row.status, "cancelled",
        "cancelled is subscriber_gone's deliberate terminal state and must never be revived"
    );

    let other_row = fetch(&scratch.db, other_workspace_failed).await;
    assert_eq!(
        other_row.status, "failed",
        "requeue_failed must be scoped to the requested workspace only"
    );

    scratch.drop_self().await;
}

#[tokio::test]
async fn replay_is_windowed_deduplicated_and_crosses_delivery_retention_without_duplication() {
    let scratch = scratch_or_skip!("v08-replay-retention");
    let workspace_id = seed_workspace(&scratch.db).await;
    let webhook_id = seed_webhook(
        &scratch.db,
        workspace_id,
        "http://example.invalid/hook",
        &["flow.object.created"],
    )
    .await;
    let now = Timestamp::now();
    let event_id = commit_dispatch_work(
        &scratch.db,
        workspace_id,
        "flow.object.created",
        None,
        None,
        json!({"object_id": Uuid::new_v4()}),
    )
    .await;
    exec(
        &scratch.db,
        "DELETE FROM event_dispatch WHERE event_id=$1",
        vec![event_id.into()],
    )
    .await;
    exec(
        &scratch.db,
        "UPDATE business_events SET created_at=$2 WHERE id=$1",
        vec![event_id.into(), (now - time::Duration::days(29)).into()],
    )
    .await;
    let request = ReplayRequest {
        workspace_id,
        mode: ReplayMode::Rebuild,
        event_type: Some("flow.object.created".to_string()),
        subscriber_kind: Some("webhook".to_string()),
        subscriber_id: Some(webhook_id),
        from: now - time::Duration::days(REPLAY_MAX_WINDOW_DAYS) + time::Duration::milliseconds(1),
        to: now - time::Duration::days(28),
        dry_run: true,
    };

    let preview = replay_deliveries(&scratch.db, &request, now)
        .await
        .expect("preview succeeds");
    assert!(matches!(
        preview,
        ReplayResult::Rebuild {
            replayed: 1,
            skipped_already_delivered: 0,
            ref rebuilt_delivery_ids,
            ..
        } if rebuilt_delivery_ids.is_empty()
    ));
    assert_eq!(
        count(&scratch.db, "SELECT count(*) AS n FROM event_deliveries", vec![]).await,
        0,
        "dry-run must write no canonical delivery or source row"
    );

    let mut execute = request.clone();
    execute.dry_run = false;
    let first = replay_deliveries(&scratch.db, &execute, now)
        .await
        .expect("rebuild succeeds");
    let delivery_id = match first {
        ReplayResult::Rebuild {
            replayed: 1,
            skipped_already_delivered: 0,
            rebuilt_delivery_ids,
            ..
        } => rebuilt_delivery_ids[0],
        other => panic!("unexpected first replay result: {other:?}"),
    };
    exec(
        &scratch.db,
        "UPDATE event_delivery_sources SET created_at=$2 WHERE delivery_id=$1",
        vec![delivery_id.into(), (now - time::Duration::days(40)).into()],
    )
    .await;
    exec(
        &scratch.db,
        "UPDATE event_deliveries SET status='dispatched', terminated_at=$2 WHERE id=$1",
        vec![delivery_id.into(), (now - time::Duration::days(31)).into()],
    )
    .await;
    assert_eq!(reap_delivery_retention(&scratch.db, now).await.expect("reaper runs"), 1);
    assert_eq!(
        reap_delivery_source_tombstones(&scratch.db, now)
            .await
            .expect("source reaper runs"),
        0,
        "a 40-day source tombstone must survive the proposed 90-day retention"
    );
    assert_eq!(
        count(
            &scratch.db,
            "SELECT count(*) AS n FROM event_delivery_sources \
              WHERE source_event_id=$1 AND delivery_id IS NULL",
            vec![event_id.into()],
        )
        .await,
        1,
        "the source tombstone must outlive the 30-day delivery"
    );

    let second = replay_deliveries(&scratch.db, &execute, now)
        .await
        .expect("second replay succeeds");
    assert!(matches!(
        second,
        ReplayResult::Rebuild {
            replayed: 0,
            skipped_already_delivered: 1,
            ref rebuilt_delivery_ids,
            ..
        } if rebuilt_delivery_ids.is_empty()
    ));
    assert_eq!(
        count(&scratch.db, "SELECT count(*) AS n FROM event_deliveries", vec![]).await,
        0,
        "a retained source tombstone must prevent a replacement delivery"
    );

    let mut exact_boundary = request;
    exact_boundary.from = now - time::Duration::days(REPLAY_MAX_WINDOW_DAYS);
    assert!(replay_deliveries(&scratch.db, &exact_boundary, now).await.is_err());
    exact_boundary.from += time::Duration::milliseconds(1);
    assert!(replay_deliveries(&scratch.db, &exact_boundary, now).await.is_ok());

    scratch.drop_self().await;
}

#[tokio::test]
async fn flow_delivery_v08_concurrent_replay_reserves_before_building_and_never_merges() {
    let scratch = scratch_or_skip!("v08-concurrent-replay");
    let workspace_id = seed_workspace(&scratch.db).await;
    let webhook_id = seed_webhook(
        &scratch.db,
        workspace_id,
        "http://example.invalid/hook",
        &["flow.object.created"],
    )
    .await;
    let event_id = commit_dispatch_work(&scratch.db, workspace_id, "flow.object.created", None, None, json!({})).await;
    exec(
        &scratch.db,
        "DELETE FROM event_dispatch WHERE event_id=$1",
        vec![event_id.into()],
    )
    .await;

    // Negative fixture for an implementation that copies a document id into replay rows: a
    // live content row already occupies the partial-unique coalescing key. Correct replay rows
    // keep document_id/dispatch_id/range NULL, so they insert independently.
    let live_content_delivery = Uuid::new_v4();
    exec(
        &scratch.db,
        "INSERT INTO event_deliveries \
           (id,event_id,workspace_id,subscriber_kind,subscriber_id,document_id,status,next_attempt_at) \
         VALUES ($1,$2,$3,'webhook',$4,$5,'pending',now()+interval '1 hour')",
        vec![
            live_content_delivery.into(),
            event_id.into(),
            workspace_id.into(),
            webhook_id.into(),
            Uuid::new_v4().into(),
        ],
    )
    .await;

    let now = Timestamp::now();
    set_timestamp(
        &scratch.db,
        "business_events",
        "created_at",
        "id",
        event_id,
        now - time::Duration::seconds(30),
    )
    .await;
    let request = ReplayRequest {
        workspace_id,
        mode: ReplayMode::Rebuild,
        event_type: Some("flow.object.created".to_string()),
        subscriber_kind: Some("webhook".to_string()),
        subscriber_id: Some(webhook_id),
        from: now - time::Duration::minutes(1),
        to: now,
        dry_run: false,
    };
    let second = scratch.second_connection().await;
    let barrier = std::sync::Arc::new(tokio::sync::Barrier::new(2));
    let (left, right) = tokio::join!(
        TEST_REPLAY_CANDIDATE_BARRIER.scope(barrier.clone(), replay_deliveries(&scratch.db, &request, now),),
        TEST_REPLAY_CANDIDATE_BARRIER.scope(barrier, replay_deliveries(&second, &request, now)),
    );
    let results = [
        left.expect("first concurrent replay completes"),
        right.expect("second concurrent replay completes"),
    ];
    let replayed = results
        .iter()
        .map(|result| match result {
            ReplayResult::Rebuild { replayed, .. } => *replayed,
            other @ ReplayResult::RequeueFailed { .. } => {
                panic!("unexpected concurrent replay result: {other:?}")
            }
        })
        .sum::<u64>();
    let skipped = results
        .iter()
        .map(|result| match result {
            ReplayResult::Rebuild {
                skipped_already_delivered,
                ..
            } => *skipped_already_delivered,
            other @ ReplayResult::RequeueFailed { .. } => {
                panic!("unexpected concurrent replay result: {other:?}")
            }
        })
        .sum::<u64>();
    assert_eq!(
        (replayed, skipped),
        (1, 1),
        "the source unique key is a write-before-build reservation"
    );
    assert_eq!(
        count(
            &scratch.db,
            "SELECT count(*) AS n FROM event_delivery_sources \
              WHERE subscriber_id=$1 AND source_event_id=$2",
            vec![webhook_id.into(), event_id.into()],
        )
        .await,
        1
    );
    assert_eq!(
        count(
            &scratch.db,
            "SELECT count(*) AS n FROM event_deliveries \
              WHERE subscriber_id=$1 AND event_id=$2 AND dispatch_id IS NULL AND document_id IS NULL \
                AND first_seq IS NULL AND latest_seq IS NULL AND status='pending'",
            vec![webhook_id.into(), event_id.into()],
        )
        .await,
        1,
        "exactly one standalone replay row is created beside the active content row"
    );
    assert_eq!(
        count(
            &scratch.db,
            "SELECT count(*) AS n FROM event_deliveries WHERE id=$1 AND status='pending'",
            vec![live_content_delivery.into()],
        )
        .await,
        1,
        "replay must neither collide with nor merge into the active content delivery"
    );
    let third = replay_deliveries(&scratch.db, &request, now)
        .await
        .expect("sequential replay completes");
    assert!(matches!(
        third,
        ReplayResult::Rebuild {
            replayed: 0,
            skipped_already_delivered: 1,
            ref rebuilt_delivery_ids,
            ..
        } if rebuilt_delivery_ids.is_empty()
    ));

    second.close().await.expect("second connection closes");
    scratch.drop_self().await;
}

#[tokio::test]
async fn requeue_failed_filters_terminated_time_and_preserves_delivery_id() {
    let scratch = scratch_or_skip!("v08-requeue-failed");
    let workspace_id = seed_workspace(&scratch.db).await;
    let webhook_id = seed_webhook(
        &scratch.db,
        workspace_id,
        "http://example.invalid/hook",
        &["flow.object.created"],
    )
    .await;
    let now = Timestamp::now();
    let event_id = commit_dispatch_work(
        &scratch.db,
        workspace_id,
        "flow.object.created",
        None,
        None,
        json!({"object_id": Uuid::new_v4()}),
    )
    .await;
    exec(
        &scratch.db,
        "DELETE FROM event_dispatch WHERE event_id=$1",
        vec![event_id.into()],
    )
    .await;
    exec(
        &scratch.db,
        "UPDATE business_events SET created_at=$2 WHERE id=$1",
        vec![event_id.into(), (now - time::Duration::days(120)).into()],
    )
    .await;
    let delivery_id = Uuid::new_v4();
    exec(
        &scratch.db,
        "INSERT INTO event_deliveries \
           (id,event_id,workspace_id,subscriber_kind,subscriber_id,document_id,status,attempts,terminated_at) \
         VALUES ($1,$2,$3,'webhook',$4,$5,'failed',10,$6)",
        vec![
            delivery_id.into(),
            event_id.into(),
            workspace_id.into(),
            webhook_id.into(),
            Uuid::new_v4().into(),
            (now - time::Duration::days(1)).into(),
        ],
    )
    .await;
    exec(
        &scratch.db,
        "INSERT INTO event_delivery_sources \
           (workspace_id,delivery_id,subscriber_kind,subscriber_id,source_event_id) \
         VALUES ($1,$2,'webhook',$3,$4)",
        vec![
            workspace_id.into(),
            delivery_id.into(),
            webhook_id.into(),
            event_id.into(),
        ],
    )
    .await;
    let request = ReplayRequest {
        workspace_id,
        mode: ReplayMode::RequeueFailed,
        event_type: Some("flow.object.created".to_string()),
        subscriber_kind: Some("webhook".to_string()),
        subscriber_id: Some(webhook_id),
        from: now - time::Duration::days(2),
        to: now,
        dry_run: false,
    };
    let result = replay_deliveries(&scratch.db, &request, now)
        .await
        .expect("requeue succeeds");
    assert_eq!(
        result,
        ReplayResult::RequeueFailed {
            requeued: 1,
            skipped_not_failed: 0,
            requeued_delivery_ids: vec![delivery_id],
            window: crate::events::dispatcher::replay::ReplayWindow {
                from: request.from,
                to: request.to,
            },
        }
    );
    let status: String = get_col(
        &scratch.db,
        "SELECT status FROM event_deliveries WHERE id=$1",
        vec![delivery_id.into()],
        "status",
    )
    .await;
    let document_id: Option<Uuid> = get_col(
        &scratch.db,
        "SELECT document_id FROM event_deliveries WHERE id=$1",
        vec![delivery_id.into()],
        "document_id",
    )
    .await;
    assert_eq!(status, "pending");
    assert_eq!(document_id, None);
    assert_eq!(
        count(
            &scratch.db,
            "SELECT count(*) AS n FROM event_delivery_sources WHERE delivery_id=$1",
            vec![delivery_id.into()],
        )
        .await,
        1,
        "requeue must not replace or duplicate the source tombstone"
    );

    scratch.drop_self().await;
}
