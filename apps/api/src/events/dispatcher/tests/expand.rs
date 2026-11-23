//! Dispatcher database tests: expansion of dispatch work into deliveries.

use super::*;

#[tokio::test]
async fn event_deliveries_are_created_only_by_a_dispatcher_tick_never_by_the_domain_transaction() {
    let scratch = scratch_or_skip!("post-commit-expansion");
    let workspace_id = seed_workspace(&scratch.db).await;
    seed_webhook(
        &scratch.db,
        workspace_id,
        "http://example.invalid/hook",
        &["flow.object.created"],
    )
    .await;

    // The "domain transaction" step: exactly what `flow::command::create_object` does, with
    // nothing about dispatching that runs the dispatcher itself.
    let event_id = commit_dispatch_work(&scratch.db, workspace_id, "flow.object.created", None, None, json!({})).await;

    assert_eq!(
        count(
            &scratch.db,
            "SELECT count(*) AS n FROM event_dispatch WHERE event_id = $1",
            vec![event_id.into()],
        )
        .await,
        1,
        "the domain transaction must write exactly one event_dispatch row"
    );
    assert_eq!(
        count(
            &scratch.db,
            "SELECT count(*) AS n FROM event_deliveries WHERE event_id = $1",
            vec![event_id.into()],
        )
        .await,
        0,
        "no event_deliveries row may exist before the dispatcher ever runs"
    );

    // Now, and only now, run the dispatcher tick — a step entirely separate from (and later
    // than) the commit above.
    let state = state_for(scratch.db.clone());
    let client = reqwest::Client::new();
    let report = run_tick(&state, &client, 4).await;
    assert_eq!(report.expanded, 1, "{report:?}");

    assert_eq!(
        count(
            &scratch.db,
            "SELECT count(*) AS n FROM event_deliveries WHERE event_id = $1",
            vec![event_id.into()],
        )
        .await,
        1,
        "expansion must have created exactly one delivery for the one registered subscriber"
    );
    let dispatch_status: String = scratch
        .db
        .query_one(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT status FROM event_dispatch WHERE event_id = $1",
            vec![event_id.into()],
        ))
        .await
        .expect("status query runs")
        .expect("status query returns a row")
        .try_get("", "status")
        .expect("status column reads");
    assert_eq!(dispatch_status, "expanded");

    scratch.drop_self().await;
}

#[tokio::test]
async fn zero_subscribers_terminalizes_the_work_and_creates_zero_deliveries() {
    let scratch = scratch_or_skip!("no-subscribers");
    let workspace_id = seed_workspace(&scratch.db).await;
    // No webhook registered at all.
    let event_id = commit_dispatch_work(&scratch.db, workspace_id, "flow.object.archived", None, None, json!({})).await;

    let state = state_for(scratch.db.clone());
    let client = reqwest::Client::new();
    let report = run_tick(&state, &client, 4).await;
    assert_eq!(report.no_subscribers, 1, "{report:?}");
    assert_eq!(report.expanded, 0, "{report:?}");

    assert_eq!(
        count(
            &scratch.db,
            "SELECT count(*) AS n FROM event_deliveries WHERE event_id = $1",
            vec![event_id.into()],
        )
        .await,
        0
    );
    assert_eq!(
        count(
            &scratch.db,
            "SELECT count(*) AS n FROM event_delivery_sources WHERE source_event_id = $1",
            vec![event_id.into()],
        )
        .await,
        0
    );

    let dispatch_status: String = scratch
        .db
        .query_one(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT status FROM event_dispatch WHERE event_id = $1",
            vec![event_id.into()],
        ))
        .await
        .expect("status query runs")
        .expect("status query returns a row")
        .try_get("", "status")
        .expect("status column reads");
    assert_eq!(dispatch_status, "no_subscribers");

    scratch.drop_self().await;
}

/// The `subscribers_per_workspace_max` ceiling, both halves, on the path the contract's row
/// describes ("dispatcher fan-out 的放大边界"): a workspace holding exactly
/// `SUBSCRIBERS_PER_WORKSPACE_MAX` active subscribers expands normally and produces exactly
/// that many `event_deliveries` rows, and one subscriber past it makes the expansion refuse
/// with `limit_exceeded{limit_kind:"workspace_subscribers"}` instead of quietly expanding a
/// truncated subset. The `+1` arm writes the extra subscriber straight to the table, because
/// `ensure_workspace_subscriber_slot` refuses it through the registration path -- the state
/// under test is one only pre-ceiling rows (or a write around the API) can produce.
#[tokio::test]
async fn workspace_subscribers_exact_boundary_expands_and_the_next_subscriber_is_rejected_with_the_frozen_limit_kind() {
    let scratch = scratch_or_skip!("workspace-subscribers-ceiling");
    let workspace_id = seed_workspace(&scratch.db).await;
    const CEILING: i64 = SUBSCRIBERS_PER_WORKSPACE_MAX;

    for _ in 0..CEILING {
        seed_webhook(
            &scratch.db,
            workspace_id,
            "http://example.invalid/hook",
            &["flow.object.created"],
        )
        .await;
    }
    assert_eq!(
        workspace_subscriber_count(&scratch.db, workspace_id)
            .await
            .expect("subscriber count query runs"),
        CEILING,
        "the exact ceiling must be reachable, not one short of it"
    );

    // Exact boundary: accepted, and the fan-out is exactly the ceiling.
    let at_ceiling =
        commit_dispatch_work(&scratch.db, workspace_id, "flow.object.created", None, None, json!({})).await;
    let outcome = expand_one(&scratch.db)
        .await
        .expect("expansion at the exact ceiling runs")
        .expect("the committed work item is pending");
    assert!(matches!(outcome, ExpansionOutcome::Expanded), "{outcome:?}");
    assert_eq!(
        count(
            &scratch.db,
            "SELECT count(*) AS n FROM event_deliveries WHERE event_id = $1",
            vec![at_ceiling.into()],
        )
        .await,
        CEILING,
        "one work item at the ceiling must expand into exactly SUBSCRIBERS_PER_WORKSPACE_MAX deliveries"
    );

    // Boundary + 1.
    seed_webhook(
        &scratch.db,
        workspace_id,
        "http://example.invalid/hook",
        &["flow.object.created"],
    )
    .await;
    let past_ceiling =
        commit_dispatch_work(&scratch.db, workspace_id, "flow.object.created", None, None, json!({})).await;
    let err = expand_one(&scratch.db)
        .await
        .expect_err("expansion must refuse a workspace past subscribers_per_workspace_max");
    let ApiError::Typed { kind, details, .. } = &err else {
        panic!("expected a typed limit_exceeded rejection, got {err:?}");
    };
    assert_eq!(kind.stable_code(), "limit_exceeded");
    let details = details.as_ref().expect("limit_exceeded always carries details");
    assert_eq!(details["limit_kind"], json!("workspace_subscribers"));
    assert_eq!(details["limit"], json!(CEILING));
    assert_eq!(details["observed"], json!(CEILING + 1));

    assert_eq!(
        count(
            &scratch.db,
            "SELECT count(*) AS n FROM event_deliveries WHERE event_id = $1",
            vec![past_ceiling.into()],
        )
        .await,
        0,
        "a refused expansion must not write a single delivery row"
    );
    assert_eq!(
        count(
            &scratch.db,
            "SELECT count(*) AS n FROM event_delivery_sources WHERE source_event_id = $1",
            vec![past_ceiling.into()],
        )
        .await,
        0,
        "a refused expansion must not reserve a single source row either"
    );
    assert_eq!(
        count(
            &scratch.db,
            "SELECT count(*) AS n FROM business_events WHERE id = $1",
            vec![past_ceiling.into()],
        )
        .await,
        1,
        "the canonical business_events row is untouched by a refused expansion"
    );
    let status: String = get_col(
        &scratch.db,
        "SELECT status FROM event_dispatch WHERE event_id = $1",
        vec![past_ceiling.into()],
        "status",
    )
    .await;
    assert_eq!(
        status, "pending",
        "a refused expansion goes back to pending, not expanded"
    );
    let attempts: i32 = get_col(
        &scratch.db,
        "SELECT attempts FROM event_dispatch WHERE event_id = $1",
        vec![past_ceiling.into()],
        "attempts",
    )
    .await;
    assert_eq!(
        attempts, 1,
        "the refusal is charged as one expansion attempt, so it dead-letters instead of spinning"
    );

    scratch.drop_self().await;
}

/// The registration half of the same ceiling: the subscriber that lands exactly on
/// `SUBSCRIBERS_PER_WORKSPACE_MAX` is accepted, the next one is refused with the frozen
/// `limit_kind`, an already-active subscriber can still be re-saved at the ceiling, and an
/// inactive webhook does not occupy a slot (the dispatcher's directory query requires
/// `active = true`, so neither may this).
#[tokio::test]
async fn workspace_subscribers_registration_ceiling_rejects_the_subscriber_past_the_frozen_maximum() {
    let scratch = scratch_or_skip!("workspace-subscribers-registration");
    let workspace_id = seed_workspace(&scratch.db).await;
    const CEILING: i64 = SUBSCRIBERS_PER_WORKSPACE_MAX;

    let mut last = Uuid::nil();
    for _ in 0..(CEILING - 1) {
        last = seed_webhook(
            &scratch.db,
            workspace_id,
            "http://example.invalid/hook",
            &["flow.object.created"],
        )
        .await;
    }
    ensure_workspace_subscriber_slot(&scratch.db, workspace_id, None)
        .await
        .expect("the subscriber landing exactly on the ceiling is accepted");

    let at_ceiling = seed_webhook(
        &scratch.db,
        workspace_id,
        "http://example.invalid/hook",
        &["flow.object.created"],
    )
    .await;
    let err = ensure_workspace_subscriber_slot(&scratch.db, workspace_id, None)
        .await
        .expect_err("the subscriber past the ceiling is refused");
    let ApiError::Typed { kind, details, .. } = &err else {
        panic!("expected a typed limit_exceeded rejection, got {err:?}");
    };
    assert_eq!(kind.stable_code(), "limit_exceeded");
    let details = details.as_ref().expect("limit_exceeded always carries details");
    assert_eq!(details["limit_kind"], json!("workspace_subscribers"));
    assert_eq!(details["limit"], json!(CEILING));
    assert_eq!(details["observed"], json!(CEILING + 1));

    ensure_workspace_subscriber_slot(&scratch.db, workspace_id, Some(at_ceiling))
        .await
        .expect("re-saving an already-active subscriber at the ceiling is not a new slot");

    exec(
        &scratch.db,
        "UPDATE webhooks SET active = false WHERE id = $1",
        vec![last.into()],
    )
    .await;
    ensure_workspace_subscriber_slot(&scratch.db, workspace_id, None)
        .await
        .expect("an inactive webhook is not a subscriber and frees its slot");

    scratch.drop_self().await;
}

#[tokio::test]
async fn re_expanding_the_same_work_after_a_simulated_crash_does_not_change_delivery_id_or_duplicate_sources() {
    let scratch = scratch_or_skip!("reexpansion-idempotent");
    let workspace_id = seed_workspace(&scratch.db).await;
    seed_webhook(
        &scratch.db,
        workspace_id,
        "http://example.invalid/hook",
        &["flow.object.created"],
    )
    .await;
    let event_id = commit_dispatch_work(&scratch.db, workspace_id, "flow.object.created", None, None, json!({})).await;

    let first = expand_one(&scratch.db)
        .await
        .expect("expansion runs")
        .expect("one work item was pending");
    assert!(matches!(first, ExpansionOutcome::Expanded));

    let delivery_id: Uuid = scratch
        .db
        .query_one(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT id FROM event_deliveries WHERE event_id = $1",
            vec![event_id.into()],
        ))
        .await
        .expect("delivery query runs")
        .expect("exactly one delivery row exists")
        .try_get("", "id")
        .expect("id column reads");

    // Simulate a dispatcher crash mid-flight: the work item is put back to `pending` with no
    // lease, exactly what `reclaim_expired_dispatch_leases` would have done to a genuinely
    // abandoned lease. `expand_one` must then re-expand it without minting a second delivery.
    exec(
        &scratch.db,
        "UPDATE event_dispatch SET status = 'pending', expanded_at = NULL, lease_token = NULL, lease_expires_at = NULL \
         WHERE event_id = $1",
        vec![event_id.into()],
    )
    .await;

    let second = expand_one(&scratch.db)
        .await
        .expect("re-expansion runs")
        .expect("the work item is pending again");
    assert!(matches!(second, ExpansionOutcome::Expanded));

    assert_eq!(
        count(
            &scratch.db,
            "SELECT count(*) AS n FROM event_deliveries WHERE event_id = $1",
            vec![event_id.into()],
        )
        .await,
        1,
        "re-expansion must not mint a second delivery row"
    );
    let delivery_id_after: Uuid = scratch
        .db
        .query_one(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT id FROM event_deliveries WHERE event_id = $1",
            vec![event_id.into()],
        ))
        .await
        .expect("delivery query runs")
        .expect("exactly one delivery row exists")
        .try_get("", "id")
        .expect("id column reads");
    assert_eq!(
        delivery_id_after, delivery_id,
        "delivery_id must stay stable across re-expansion"
    );

    assert_eq!(
        count(
            &scratch.db,
            "SELECT count(*) AS n FROM event_delivery_sources WHERE source_event_id = $1",
            vec![event_id.into()],
        )
        .await,
        1,
        "the source table's ON CONFLICT DO NOTHING must keep this at exactly one row"
    );

    scratch.drop_self().await;
}

// =============================================================================================
// Gate: business_event_dispatch_same_transaction
// =============================================================================================

#[tokio::test]
async fn business_event_dispatch_same_transaction_touches_zero_subscription_queries() {
    let scratch = scratch_or_skip!("zero-sub-query");
    let workspace_id = seed_workspace(&scratch.db).await;
    seed_webhook(
        &scratch.db,
        workspace_id,
        "http://example.invalid/hook",
        &["flow.object.created"],
    )
    .await;

    // A second connection holds an ACCESS EXCLUSIVE lock on `webhooks` for the whole test body.
    // That lock blocks *every* statement against that table, not just writes -- including a
    // plain unlocked SELECT, which a lighter lock (e.g. row-level FOR UPDATE) would not. If the
    // domain transaction below queried the subscription directory even once, it would block
    // here until the lock is released, and the timeout turns that into a failed assertion
    // instead of a hang.
    let locker = scratch.second_connection().await;
    let lock_tx = locker.begin().await.expect("lock tx begins");
    lock_tx
        .execute_unprepared("LOCK TABLE webhooks IN ACCESS EXCLUSIVE MODE")
        .await
        .expect("lock acquired");

    let result = tokio::time::timeout(
        std::time::Duration::from_secs(2),
        commit_dispatch_work(&scratch.db, workspace_id, "flow.object.created", None, None, json!({})),
    )
    .await;
    assert!(
        result.is_ok(),
        "the domain transaction blocked on a table lock held on `webhooks`: it must issue zero \
         subscription-directory queries in the domain transaction (ADR-0011)"
    );

    lock_tx.rollback().await.expect("lock released");
    scratch.drop_self().await;
}

#[tokio::test]
async fn business_event_dispatch_same_transaction_rollback_leaves_zero_residue() {
    let scratch = scratch_or_skip!("rollback-residue");
    let workspace_id = seed_workspace(&scratch.db).await;

    let tx = scratch.db.begin().await.expect("tx begins");
    let event_id = insert_business_event(
        &tx,
        BusinessEventInput {
            workspace_id,
            project_id: None,
            event_type: "flow.object.created".to_string(),
            aggregate_type: "flow_object".to_string(),
            aggregate_id: Uuid::new_v4().to_string(),
            actor_id: None,
            source: json!({ "surface": "rest" }),
            payload: json!({}),
            metadata: json!({}),
            correlation_id: None,
            causation_id: None,
            idempotency_key: None,
        },
    )
    .await
    .expect("business event insert succeeds inside the open transaction");
    tx.execute(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "INSERT INTO event_dispatch (id, event_id, workspace_id, event_type, max_attempts) \
         VALUES ($1, $2, $3, $4, 10)",
        vec![
            Uuid::new_v4().into(),
            event_id.into(),
            workspace_id.into(),
            "flow.object.created".into(),
        ],
    ))
    .await
    .expect("event_dispatch insert succeeds inside the open transaction");

    // Simulate the rest of the domain transaction failing after both rows above were written --
    // e.g. a later step in the same transaction hit a constraint violation -- by rolling back
    // explicitly. This has the identical server-visible effect as an error percolating up
    // through `?` before `tx.commit()` is ever reached.
    tx.rollback().await.expect("rollback succeeds");

    assert_eq!(
        count(
            &scratch.db,
            "SELECT count(*) AS n FROM business_events WHERE id = $1",
            vec![event_id.into()],
        )
        .await,
        0,
        "a rolled-back domain transaction must leave zero business_events rows"
    );
    assert_eq!(
        count(
            &scratch.db,
            "SELECT count(*) AS n FROM event_dispatch WHERE event_id = $1",
            vec![event_id.into()],
        )
        .await,
        0,
        "a rolled-back domain transaction must leave zero event_dispatch rows"
    );

    scratch.drop_self().await;
}

// =============================================================================================
// Gate: dispatch_expansion_snapshot_semantics
// =============================================================================================

#[tokio::test]
async fn dispatch_expansion_delivers_to_a_subscriber_created_after_the_event_committed_but_before_expansion() {
    let scratch = scratch_or_skip!("snapshot-late-subscriber");
    let workspace_id = seed_workspace(&scratch.db).await;

    // The event commits with zero subscribers registered yet.
    let event_id = commit_dispatch_work(&scratch.db, workspace_id, "flow.object.created", None, None, json!({})).await;

    // A webhook is registered *after* that commit, but the dispatcher has not expanded the work
    // item yet (nothing has called `expand_one` in between).
    seed_webhook(
        &scratch.db,
        workspace_id,
        "http://example.invalid/hook",
        &["flow.object.created"],
    )
    .await;

    let outcome = expand_one(&scratch.db)
        .await
        .expect("expansion runs")
        .expect("one work item was pending");
    assert!(
        matches!(outcome, ExpansionOutcome::Expanded),
        "events-v1.md: the receiver set is whoever is active *at expansion time*, not at commit \
         time -- a webhook registered after commit but before expansion must receive this event, \
         which is defined-in-spec behavior, not a bug"
    );
    assert_eq!(
        count(
            &scratch.db,
            "SELECT count(*) AS n FROM event_deliveries WHERE event_id = $1",
            vec![event_id.into()],
        )
        .await,
        1
    );

    scratch.drop_self().await;
}

#[tokio::test]
async fn dispatch_expansion_skips_a_subscriber_deleted_after_commit_but_before_expansion() {
    let scratch = scratch_or_skip!("snapshot-deleted-subscriber");
    let workspace_id = seed_workspace(&scratch.db).await;
    let webhook_id = seed_webhook(
        &scratch.db,
        workspace_id,
        "http://example.invalid/hook",
        &["flow.object.created"],
    )
    .await;
    let event_id = commit_dispatch_work(&scratch.db, workspace_id, "flow.object.created", None, None, json!({})).await;

    // The subscriber is deleted before the dispatcher ever expands the work item.
    delete_webhook(&scratch.db, webhook_id).await;

    let outcome = expand_one(&scratch.db)
        .await
        .expect("expansion runs")
        .expect("one work item was pending");
    assert!(
        matches!(outcome, ExpansionOutcome::NoSubscribers),
        "a subscriber deleted before expansion must not receive the event, and with no other \
         subscriber left the work item must terminalize as no_subscribers"
    );
    assert_eq!(
        count(
            &scratch.db,
            "SELECT count(*) AS n FROM event_deliveries WHERE event_id = $1",
            vec![event_id.into()],
        )
        .await,
        0
    );

    scratch.drop_self().await;
}

#[tokio::test]
async fn dispatch_expansion_skips_a_subscriber_disabled_after_commit_but_before_expansion() {
    let scratch = scratch_or_skip!("snapshot-disabled-subscriber");
    let workspace_id = seed_workspace(&scratch.db).await;
    let webhook_id = seed_webhook(
        &scratch.db,
        workspace_id,
        "http://example.invalid/hook",
        &["flow.object.created"],
    )
    .await;
    let event_id = commit_dispatch_work(&scratch.db, workspace_id, "flow.object.created", None, None, json!({})).await;

    // Disabled is judged the same as deleted (events-v1.md "订阅目录与 active 的判据").
    exec(
        &scratch.db,
        "UPDATE webhooks SET active = false WHERE id = $1",
        vec![webhook_id.into()],
    )
    .await;

    let outcome = expand_one(&scratch.db)
        .await
        .expect("expansion runs")
        .expect("one work item was pending");
    assert!(matches!(outcome, ExpansionOutcome::NoSubscribers));
    assert_eq!(
        count(
            &scratch.db,
            "SELECT count(*) AS n FROM event_deliveries WHERE event_id = $1",
            vec![event_id.into()],
        )
        .await,
        0
    );

    scratch.drop_self().await;
}

#[tokio::test]
async fn dispatch_expansion_delivers_to_exactly_the_subscribers_active_at_expansion_moment() {
    let scratch = scratch_or_skip!("snapshot-exact-set");
    let workspace_id = seed_workspace(&scratch.db).await;
    // Three subscribers: one for the right event type, one for a different event type (must
    // not receive), one disabled (must not receive).
    seed_webhook(
        &scratch.db,
        workspace_id,
        "http://example.invalid/hook-a",
        &["flow.object.created"],
    )
    .await;
    seed_webhook(
        &scratch.db,
        workspace_id,
        "http://example.invalid/hook-b",
        &["flow.object.archived"],
    )
    .await;
    let disabled = seed_webhook(
        &scratch.db,
        workspace_id,
        "http://example.invalid/hook-c",
        &["flow.object.created"],
    )
    .await;
    exec(
        &scratch.db,
        "UPDATE webhooks SET active = false WHERE id = $1",
        vec![disabled.into()],
    )
    .await;

    let event_id = commit_dispatch_work(&scratch.db, workspace_id, "flow.object.created", None, None, json!({})).await;
    let report = run_tick(&state_for(scratch.db.clone()), &reqwest::Client::new(), 4).await;
    assert_eq!(report.expanded, 1, "{report:?}");
    assert_eq!(
        count(
            &scratch.db,
            "SELECT count(*) AS n FROM event_deliveries WHERE event_id = $1",
            vec![event_id.into()],
        )
        .await,
        1,
        "exactly one of the three registered webhooks matches both the event type and active=true"
    );

    scratch.drop_self().await;
}

#[tokio::test]
async fn no_subscribers_work_does_not_block_a_later_work_item_in_the_same_batch() {
    let scratch = scratch_or_skip!("no-sub-non-blocking");
    let workspace_id = seed_workspace(&scratch.db).await;
    seed_webhook(
        &scratch.db,
        workspace_id,
        "http://example.invalid/hook",
        &["flow.object.created"],
    )
    .await;

    // A no-subscriber event (archived, nobody subscribes) followed by a real one.
    commit_dispatch_work(&scratch.db, workspace_id, "flow.object.archived", None, None, json!({})).await;
    let event_id = commit_dispatch_work(&scratch.db, workspace_id, "flow.object.created", None, None, json!({})).await;

    let report = run_tick(&state_for(scratch.db.clone()), &reqwest::Client::new(), 8).await;
    assert_eq!(report.no_subscribers, 1, "{report:?}");
    assert_eq!(report.expanded, 1, "{report:?}");
    assert_eq!(
        count(
            &scratch.db,
            "SELECT count(*) AS n FROM event_deliveries WHERE event_id = $1",
            vec![event_id.into()],
        )
        .await,
        1
    );

    scratch.drop_self().await;
}

#[tokio::test]
async fn killed_dispatcher_backlog_is_drained_after_restart_with_zero_duplicate_delivery() {
    let scratch = scratch_or_skip!("kill-restart-backlog");
    let workspace_id = seed_workspace(&scratch.db).await;
    seed_webhook(
        &scratch.db,
        workspace_id,
        "http://example.invalid/hook",
        &["flow.object.created"],
    )
    .await;

    let mut event_ids = Vec::new();
    for _ in 0..5 {
        let event_id =
            commit_dispatch_work(&scratch.db, workspace_id, "flow.object.created", None, None, json!({})).await;
        event_ids.push(event_id);
    }

    // Simulate a worker that claimed every one of these leases and then vanished before
    // completing expansion: replicate exactly the lease-claiming UPDATE `expand_one` issues,
    // then never run the expansion transaction that would normally follow it in the same call.
    exec(
        &scratch.db,
        "UPDATE event_dispatch SET lease_token = 'dead-worker', \
         lease_expires_at = now() - interval '1 minute' WHERE workspace_id = $1 AND status = 'pending'",
        vec![workspace_id.into()],
    )
    .await;

    // "Restart": a fresh dispatcher process reclaims the abandoned leases, then works the
    // backlog exactly as `run_tick`'s normal loop does.
    let reclaimed = reclaim_expired_dispatch_leases(&scratch.db)
        .await
        .expect("reclaim runs");
    assert_eq!(reclaimed, 5, "every abandoned lease must be reclaimed on restart");

    let report = run_tick(&state_for(scratch.db.clone()), &reqwest::Client::new(), 16).await;
    assert_eq!(report.expanded, 5, "{report:?}");

    for event_id in &event_ids {
        assert_eq!(
            count(
                &scratch.db,
                "SELECT count(*) AS n FROM event_deliveries WHERE event_id = $1",
                vec![(*event_id).into()],
            )
            .await,
            1,
            "event {event_id} must have exactly one delivery row after drain, zero duplicates"
        );
        assert_eq!(
            count(
                &scratch.db,
                "SELECT count(*) AS n FROM event_delivery_sources WHERE source_event_id = $1",
                vec![(*event_id).into()],
            )
            .await,
            1,
            "event {event_id} must have exactly one source-table row after drain"
        );
    }
    assert_eq!(
        count(
            &scratch.db,
            "SELECT count(*) AS n FROM event_dispatch WHERE workspace_id = $1 AND status = 'pending'",
            vec![workspace_id.into()],
        )
        .await,
        0,
        "no work item may remain pending after the backlog is drained"
    );

    scratch.drop_self().await;
}

#[tokio::test]
async fn a_true_mid_expansion_crash_via_injected_failure_recovers_cleanly_on_the_next_tick() {
    let scratch = scratch_or_skip!("kill-mid-expansion-injected");
    let workspace_id = seed_workspace(&scratch.db).await;
    seed_webhook(
        &scratch.db,
        workspace_id,
        "http://example.invalid/hook",
        &["flow.object.created"],
    )
    .await;
    let event_id = commit_dispatch_work(&scratch.db, workspace_id, "flow.object.created", None, None, json!({})).await;

    // "Kill" the worker exactly mid-expansion: step (a) (the source reservation) has already
    // run on the open transaction, then step (b) fails and the whole transaction rolls back --
    // the same server-visible effect as the process actually dying at that instant.
    let crashed = FAIL_EXPANSION_STEP_B
        .scope(std::cell::Cell::new(true), expand_one(&scratch.db))
        .await;
    assert!(
        crashed.is_err(),
        "the injected failure must propagate as an Err from expand_one"
    );

    assert_eq!(
        count(
            &scratch.db,
            "SELECT count(*) AS n FROM event_deliveries WHERE event_id = $1",
            vec![event_id.into()],
        )
        .await,
        0,
        "the rolled-back transaction must leave zero delivery rows"
    );
    assert_eq!(
        count(
            &scratch.db,
            "SELECT count(*) AS n FROM event_delivery_sources WHERE source_event_id = $1",
            vec![event_id.into()],
        )
        .await,
        0,
        "the rolled-back transaction must leave zero source-table rows -- step (a)'s reservation \
         rolled back along with everything else in the same transaction"
    );

    let status: String = get_col(
        &scratch.db,
        "SELECT status FROM event_dispatch WHERE event_id = $1",
        vec![event_id.into()],
        "status",
    )
    .await;
    assert_eq!(
        status, "pending",
        "a failed expansion must leave the work item back in pending"
    );

    // The failed attempt backed off `next_attempt_at` (`dispatch_backoff_ms`); advance the
    // virtual clock past it by backdating that column directly, not by sleeping through the
    // real backoff window.
    let dispatch_id: Uuid = get_col(
        &scratch.db,
        "SELECT id FROM event_dispatch WHERE event_id = $1",
        vec![event_id.into()],
        "id",
    )
    .await;
    backdate(
        &scratch.db,
        "event_dispatch",
        "next_attempt_at",
        "id",
        dispatch_id,
        time::Duration::milliseconds(1),
    )
    .await;

    // "Restart": the next tick, with the fault no longer armed, must expand cleanly with no
    // leftover from the crashed attempt.
    let report = run_tick(&state_for(scratch.db.clone()), &reqwest::Client::new(), 4).await;
    assert_eq!(report.expanded, 1, "{report:?}");
    assert_eq!(
        count(
            &scratch.db,
            "SELECT count(*) AS n FROM event_deliveries WHERE event_id = $1",
            vec![event_id.into()],
        )
        .await,
        1,
        "exactly one delivery row after the crash-then-retry, no duplicate from the aborted attempt"
    );

    scratch.drop_self().await;
}

#[tokio::test]
async fn concurrent_dispatcher_instances_racing_the_same_backlog_never_double_expand_or_double_send() {
    let scratch = scratch_or_skip!("concurrent-race");
    let workspace_id = seed_workspace(&scratch.db).await;
    seed_webhook(
        &scratch.db,
        workspace_id,
        "http://127.0.0.1:1/hook", // refused by the SSRF guard, so `send_one` reliably retries
        &["flow.object.created"],
    )
    .await;

    let mut event_ids = Vec::new();
    for _ in 0..10 {
        let event_id =
            commit_dispatch_work(&scratch.db, workspace_id, "flow.object.created", None, None, json!({})).await;
        event_ids.push(event_id);
    }

    // Five simulated dispatcher instances race `expand_one` against the same ten-item backlog
    // concurrently, sharing one `DatabaseConnection` (itself pool-backed, so this genuinely
    // exercises concurrent connections, not just concurrent async tasks on one connection).
    let handles: Vec<_> = (0..5)
        .map(|_| {
            let db = scratch.db.clone();
            tokio::spawn(async move { drain_expand_one(&db).await })
        })
        .collect();

    let mut total_expanded = 0u32;
    for handle in handles {
        total_expanded += handle.await.expect("worker task does not panic");
    }
    assert_eq!(
        total_expanded, 10,
        "every work item must be expanded exactly once across all racing workers"
    );

    for event_id in &event_ids {
        assert_eq!(
            count(
                &scratch.db,
                "SELECT count(*) AS n FROM event_deliveries WHERE event_id = $1",
                vec![(*event_id).into()],
            )
            .await,
            1,
            "event {event_id} must have exactly one delivery row despite five racing expanders"
        );
    }

    // Now race `send_one` the same way; the coalescing/FIFO predicates that matter for content
    // events are exercised separately elsewhere, this proves the plain fan-out lease query
    // itself never double-leases a delivery row.
    let send_handles: Vec<_> = (0..5)
        .map(|_| {
            let db = scratch.db.clone();
            tokio::spawn(async move { drain_send_one(state_for(db)).await })
        })
        .collect();
    let mut total_sent = 0u32;
    for handle in send_handles {
        total_sent += handle.await.expect("send task does not panic");
    }
    assert_eq!(
        total_sent, 10,
        "every delivery must be leased and attempted exactly once per pass"
    );

    for event_id in &event_ids {
        let attempts: i32 = get_col(
            &scratch.db,
            "SELECT attempts FROM event_deliveries WHERE event_id = $1",
            vec![(*event_id).into()],
            "attempts",
        )
        .await;
        assert_eq!(
            attempts, 1,
            "each delivery must have been attempted exactly once, not raced twice"
        );
    }

    scratch.drop_self().await;
}
