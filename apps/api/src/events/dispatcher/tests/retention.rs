//! Dispatcher database tests: retention reapers.

use super::*;

// =============================================================================================
// Gate: no_subscribers_terminalized_and_reaped
// =============================================================================================

#[tokio::test]
async fn no_subscribers_row_survives_exactly_at_the_retention_boundary_and_is_reaped_one_ms_past_it() {
    let scratch = scratch_or_skip!("no-sub-retention-boundary");
    let workspace_id = seed_workspace(&scratch.db).await;
    let event_id = commit_dispatch_work(&scratch.db, workspace_id, "flow.object.archived", None, None, json!({})).await;

    let dispatch_id: Uuid = get_col(
        &scratch.db,
        "SELECT id FROM event_dispatch WHERE event_id = $1",
        vec![event_id.into()],
        "id",
    )
    .await;

    let outcome = expand_one(&scratch.db)
        .await
        .expect("expansion runs")
        .expect("one work item was pending");
    assert!(matches!(outcome, ExpansionOutcome::NoSubscribers));

    // `dispatch_no_subscribers_retention_hours` is frozen at 24h. Freeze one reference instant
    // in Rust and use it for both the row's `expanded_at` and the reaper's `now` argument, so
    // "exactly at the boundary" is not at the mercy of two independent Postgres `now()` calls
    // drifting apart between the `UPDATE` and the `DELETE`.
    let reference_now = Timestamp::now();
    set_timestamp(
        &scratch.db,
        "event_dispatch",
        "expanded_at",
        "id",
        dispatch_id,
        reference_now - time::Duration::hours(24),
    )
    .await;
    let reaped_at_boundary = crate::events::dispatcher::retention::reap_dispatch_retention(&scratch.db, reference_now)
        .await
        .expect("reaper runs");
    assert_eq!(
        reaped_at_boundary, 0,
        "a row exactly at the retention boundary must survive"
    );
    assert_eq!(
        count(
            &scratch.db,
            "SELECT count(*) AS n FROM event_dispatch WHERE id = $1",
            vec![dispatch_id.into()],
        )
        .await,
        1
    );

    // One millisecond older crosses the boundary (same reference instant for the reaper call).
    set_timestamp(
        &scratch.db,
        "event_dispatch",
        "expanded_at",
        "id",
        dispatch_id,
        reference_now - time::Duration::hours(24) - time::Duration::milliseconds(1),
    )
    .await;
    let reaped_past_boundary =
        crate::events::dispatcher::retention::reap_dispatch_retention(&scratch.db, reference_now)
            .await
            .expect("reaper runs");
    assert_eq!(
        reaped_past_boundary, 1,
        "one millisecond past the retention boundary the row must be reaped"
    );
    assert_eq!(
        count(
            &scratch.db,
            "SELECT count(*) AS n FROM event_dispatch WHERE id = $1",
            vec![dispatch_id.into()],
        )
        .await,
        0
    );
    assert_eq!(
        count(
            &scratch.db,
            "SELECT count(*) AS n FROM business_events WHERE id = $1",
            vec![event_id.into()],
        )
        .await,
        1,
        "no-subscriber retention must never delete the permanent audit event"
    );

    scratch.drop_self().await;
}

/// `events-v1.md` "reaper 的谓词" / "来源表的保留期与外键语义": `event_delivery_sources` has no
/// `status`, so its reaper cannot use the `WHERE status IN (...)` shape the other two reapers
/// share -- its safety property is entirely the `delivery_id IS NULL` predicate. This proves
/// the three-way partition literally: a tombstone (`delivery_id IS NULL`) past retention is
/// reaped; a tombstone not yet past retention survives; and a row whose `delivery_id` is still
/// non-NULL survives *regardless of age*, because deleting it would destroy the
/// `(subscriber_kind, subscriber_id, source_event_id)` dedup evidence a still-referenced
/// delivery depends on.
#[tokio::test]
async fn tombstone_reaper_deletes_only_delivery_id_null_rows_past_retention_and_never_a_still_referenced_row() {
    let scratch = scratch_or_skip!("tombstone-reaper");
    let workspace_id = seed_workspace(&scratch.db).await;

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

    let live_delivery_id = Uuid::new_v4();
    exec(
        &scratch.db,
        "INSERT INTO event_deliveries (id, event_id, workspace_id, subscriber_kind, subscriber_id) \
         VALUES ($1, $2, $3, 'webhook', $4)",
        vec![
            live_delivery_id.into(),
            source_event(&scratch.db, workspace_id).await.into(),
            workspace_id.into(),
            Uuid::new_v4().into(),
        ],
    )
    .await;

    let now = Timestamp::now();
    let old_created_at = now - time::Duration::days(crate::events::dispatcher::DELIVERY_SOURCE_RETENTION_DAYS + 1);
    let fresh_created_at = now - time::Duration::days(crate::events::dispatcher::DELIVERY_SOURCE_RETENTION_DAYS - 1);

    async fn insert_source_row(
        db: &DatabaseConnection,
        workspace_id: Uuid,
        delivery_id: Option<Uuid>,
        source_event_id: Uuid,
        created_at: Timestamp,
    ) -> Uuid {
        let id = Uuid::new_v4();
        exec(
            db,
            "INSERT INTO event_delivery_sources \
             (id, workspace_id, delivery_id, subscriber_kind, subscriber_id, source_event_id, created_at) \
             VALUES ($1, $2, $3, 'webhook', $4, $5, $6)",
            vec![
                id.into(),
                workspace_id.into(),
                delivery_id.into(),
                Uuid::new_v4().into(),
                source_event_id.into(),
                created_at.into(),
            ],
        )
        .await;
        id
    }

    let old_tombstone = insert_source_row(
        &scratch.db,
        workspace_id,
        None,
        source_event(&scratch.db, workspace_id).await,
        old_created_at,
    )
    .await;
    let fresh_tombstone = insert_source_row(
        &scratch.db,
        workspace_id,
        None,
        source_event(&scratch.db, workspace_id).await,
        fresh_created_at,
    )
    .await;
    let old_but_still_referenced = insert_source_row(
        &scratch.db,
        workspace_id,
        Some(live_delivery_id),
        source_event(&scratch.db, workspace_id).await,
        old_created_at,
    )
    .await;

    let reaped = reap_delivery_source_tombstones(&scratch.db, now)
        .await
        .expect("tombstone reaper runs");
    assert_eq!(reaped, 1, "exactly one row -- the old tombstone -- must be reaped");

    assert_eq!(
        count(
            &scratch.db,
            "SELECT count(*) AS n FROM event_delivery_sources WHERE id = $1",
            vec![old_tombstone.into()],
        )
        .await,
        0,
        "a delivery_id=NULL row past retention must be deleted"
    );
    assert_eq!(
        count(
            &scratch.db,
            "SELECT count(*) AS n FROM event_delivery_sources WHERE id = $1",
            vec![fresh_tombstone.into()],
        )
        .await,
        1,
        "a delivery_id=NULL row not yet past retention must survive"
    );
    assert_eq!(
        count(
            &scratch.db,
            "SELECT count(*) AS n FROM event_delivery_sources WHERE id = $1",
            vec![old_but_still_referenced.into()],
        )
        .await,
        1,
        "a row whose delivery_id is still non-NULL must survive regardless of age -- deleting \
         it would destroy dedup evidence a still-live delivery depends on"
    );

    scratch.drop_self().await;
}

#[tokio::test]
async fn delivery_retention_exact_boundary_and_never_deletes_a_non_terminal_row_regardless_of_age() {
    let scratch = scratch_or_skip!("delivery-retention-boundary");
    let workspace_id = seed_workspace(&scratch.db).await;
    seed_webhook(
        &scratch.db,
        workspace_id,
        "http://example.invalid/hook",
        &["flow.object.created"],
    )
    .await;

    // A dispatched (terminal) delivery, backdated on `terminated_at` -- the frozen anchor for
    // this retention window (never `created_at`/`updated_at`, both of which would let the
    // boundary be gamed, per events-v1.md "计时锚点").
    let dispatched_event =
        commit_dispatch_work(&scratch.db, workspace_id, "flow.object.created", None, None, json!({})).await;
    expand_one(&scratch.db).await.expect("expansion runs");
    let dispatched_delivery: Uuid = get_col(
        &scratch.db,
        "SELECT id FROM event_deliveries WHERE event_id = $1",
        vec![dispatched_event.into()],
        "id",
    )
    .await;
    exec(
        &scratch.db,
        "UPDATE event_deliveries SET status = 'dispatched', terminated_at = now(), \
         lease_token = NULL, lease_expires_at = NULL WHERE id = $1",
        vec![dispatched_delivery.into()],
    )
    .await;

    // A `pending` delivery, artificially aged far past the retention window -- the reaper must
    // never touch a non-terminal row on age alone, regardless of which column looks old.
    let stale_pending_event =
        commit_dispatch_work(&scratch.db, workspace_id, "flow.object.created", None, None, json!({})).await;
    expand_one(&scratch.db).await.expect("expansion runs");
    let stale_pending_delivery: Uuid = get_col(
        &scratch.db,
        "SELECT id FROM event_deliveries WHERE event_id = $1",
        vec![stale_pending_event.into()],
        "id",
    )
    .await;
    backdate(
        &scratch.db,
        "event_deliveries",
        "created_at",
        "id",
        stale_pending_delivery,
        time::Duration::days(3650),
    )
    .await;

    let reference_now = Timestamp::now();

    // Exactly at the 30-day boundary: must survive.
    set_timestamp(
        &scratch.db,
        "event_deliveries",
        "terminated_at",
        "id",
        dispatched_delivery,
        reference_now - time::Duration::days(30),
    )
    .await;
    let reaped_at_boundary = crate::events::dispatcher::retention::reap_delivery_retention(&scratch.db, reference_now)
        .await
        .expect("reaper runs");
    assert_eq!(reaped_at_boundary, 0);
    assert_eq!(
        count(
            &scratch.db,
            "SELECT count(*) AS n FROM event_deliveries WHERE id = $1",
            vec![dispatched_delivery.into()],
        )
        .await,
        1
    );

    // One millisecond past it: must be reaped.
    set_timestamp(
        &scratch.db,
        "event_deliveries",
        "terminated_at",
        "id",
        dispatched_delivery,
        reference_now - time::Duration::days(30) - time::Duration::milliseconds(1),
    )
    .await;
    let reaped_past_boundary =
        crate::events::dispatcher::retention::reap_delivery_retention(&scratch.db, reference_now)
            .await
            .expect("reaper runs");
    assert_eq!(reaped_past_boundary, 1);
    assert_eq!(
        count(
            &scratch.db,
            "SELECT count(*) AS n FROM event_deliveries WHERE id = $1",
            vec![dispatched_delivery.into()],
        )
        .await,
        0
    );
    assert_eq!(
        count(
            &scratch.db,
            "SELECT count(*) AS n FROM business_events WHERE id = $1",
            vec![dispatched_event.into()],
        )
        .await,
        1,
        "delivery retention must never delete the permanent audit event"
    );

    // The stale pending row must have survived both reaper passes above untouched.
    assert_eq!(
        count(
            &scratch.db,
            "SELECT count(*) AS n FROM event_deliveries WHERE id = $1",
            vec![stale_pending_delivery.into()],
        )
        .await,
        1,
        "a non-terminal row must never be deleted by the retention reaper regardless of age"
    );

    scratch.drop_self().await;
}

#[tokio::test]
async fn dispatch_expanded_and_failed_retention_use_expanded_at_and_differ_from_each_other() {
    let scratch = scratch_or_skip!("dispatch-expanded-failed-retention");
    let workspace_id = seed_workspace(&scratch.db).await;
    seed_webhook(
        &scratch.db,
        workspace_id,
        "http://example.invalid/hook",
        &["flow.object.created"],
    )
    .await;

    let expanded_event =
        commit_dispatch_work(&scratch.db, workspace_id, "flow.object.created", None, None, json!({})).await;
    expand_one(&scratch.db).await.expect("expansion runs");
    let expanded_dispatch: Uuid = get_col(
        &scratch.db,
        "SELECT id FROM event_dispatch WHERE event_id = $1",
        vec![expanded_event.into()],
        "id",
    )
    .await;

    let failed_event =
        commit_dispatch_work(&scratch.db, workspace_id, "flow.object.archived", None, None, json!({})).await;
    let failed_dispatch: Uuid = get_col(
        &scratch.db,
        "SELECT id FROM event_dispatch WHERE event_id = $1",
        vec![failed_event.into()],
        "id",
    )
    .await;
    exec(
        &scratch.db,
        "UPDATE event_dispatch SET status = 'failed', expanded_at = now(), \
         last_error_code = 'expansion_failed' WHERE id = $1",
        vec![failed_dispatch.into()],
    )
    .await;

    let reference_now = Timestamp::now();

    // `dispatch_expanded_retention_days` = 14: 14 days old survives, 15 is reaped. The `failed`
    // row (still fresh at `reference_now`) must be unaffected by either pass.
    set_timestamp(
        &scratch.db,
        "event_dispatch",
        "expanded_at",
        "id",
        expanded_dispatch,
        reference_now - time::Duration::days(14),
    )
    .await;
    assert_eq!(
        crate::events::dispatcher::retention::reap_dispatch_retention(&scratch.db, reference_now)
            .await
            .expect("reaper runs"),
        0
    );
    set_timestamp(
        &scratch.db,
        "event_dispatch",
        "expanded_at",
        "id",
        expanded_dispatch,
        reference_now - time::Duration::days(15),
    )
    .await;
    assert_eq!(
        crate::events::dispatcher::retention::reap_dispatch_retention(&scratch.db, reference_now)
            .await
            .expect("reaper runs"),
        1
    );

    // `dispatch_failed_retention_days` = 90, deliberately far longer -- dead-letter rows must
    // stay visible long after an `expanded` row of the same age would already be gone.
    set_timestamp(
        &scratch.db,
        "event_dispatch",
        "expanded_at",
        "id",
        failed_dispatch,
        reference_now - time::Duration::days(15),
    )
    .await;
    assert_eq!(
        count(
            &scratch.db,
            "SELECT count(*) AS n FROM event_dispatch WHERE id = $1",
            vec![failed_dispatch.into()],
        )
        .await,
        1,
        "a failed (dead-letter) row must still be visible 15 days in, well past the expanded window"
    );
    set_timestamp(
        &scratch.db,
        "event_dispatch",
        "expanded_at",
        "id",
        failed_dispatch,
        reference_now - time::Duration::days(91),
    )
    .await;
    assert_eq!(
        crate::events::dispatcher::retention::reap_dispatch_retention(&scratch.db, reference_now)
            .await
            .expect("reaper runs"),
        1
    );

    scratch.drop_self().await;
}
