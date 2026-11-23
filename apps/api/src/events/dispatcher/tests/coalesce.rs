//! Dispatcher database tests: content delivery coalescing and the per-document fifo barrier.

use super::*;

#[tokio::test]
async fn content_events_on_the_same_document_coalesce_into_one_pending_delivery() {
    let scratch = scratch_or_skip!("coalescing");
    let workspace_id = seed_workspace(&scratch.db).await;
    seed_webhook(
        &scratch.db,
        workspace_id,
        "http://example.invalid/hook",
        &["flow.content.accepted"],
    )
    .await;
    let document_id = Uuid::new_v4();

    let event_1 = commit_dispatch_work(
        &scratch.db,
        workspace_id,
        "flow.content.accepted",
        Some(document_id),
        Some(1),
        json!({ "changed_block_ids": [Uuid::new_v4()] }),
    )
    .await;
    let event_2 = commit_dispatch_work(
        &scratch.db,
        workspace_id,
        "flow.content.accepted",
        Some(document_id),
        Some(2),
        json!({ "changed_block_ids": [Uuid::new_v4()] }),
    )
    .await;

    let state = state_for(scratch.db.clone());
    let client = reqwest::Client::new();
    // Two work items to expand, head-of-queue ordered (seq 1 before seq 2).
    let report = run_tick(&state, &client, 8).await;
    assert_eq!(report.expanded, 2, "{report:?}");

    let deliveries = count(
        &scratch.db,
        "SELECT count(*) AS n FROM event_deliveries WHERE subscriber_kind = 'webhook' AND document_id = $1",
        vec![document_id.into()],
    )
    .await;
    assert_eq!(
        deliveries, 1,
        "both accepted updates must coalesce into one delivery row"
    );

    #[derive(sea_orm::FromQueryResult)]
    struct Row {
        id: Uuid,
        first_seq: Option<i64>,
        latest_seq: Option<i64>,
    }
    let row = Row::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "SELECT id, first_seq, latest_seq FROM event_deliveries WHERE subscriber_kind = 'webhook' AND document_id = $1",
        vec![document_id.into()],
    ))
    .one(&scratch.db)
    .await
    .expect("delivery query runs")
    .expect("one coalesced delivery row exists");
    assert_eq!(row.first_seq, Some(1));
    assert_eq!(row.latest_seq, Some(2));

    let source_rows = count(
        &scratch.db,
        "SELECT count(*) AS n FROM event_delivery_sources WHERE delivery_id = $1",
        vec![row.id.into()],
    )
    .await;
    assert_eq!(source_rows, 2, "one source row per merged accepted update");

    let body = build_delivery_body(&scratch.db, &delivery_lease_row_for_test(row.id, event_1, None))
        .await
        .expect("body builds");
    assert_eq!(body["delivery"]["coalesced"], true, "{body}");
    let source_ids = body["delivery"]["source_event_ids"].as_array().expect("array");
    assert_eq!(source_ids.len(), 2);
    assert!(source_ids.contains(&json!(event_1)));
    assert!(source_ids.contains(&json!(event_2)));
    assert_eq!(body["event"]["event_type"], "flow.content.accepted", "{body}");

    scratch.drop_self().await;
}

// =============================================================================================
// Gate: flow_content_delivery_coalescing
// =============================================================================================

#[tokio::test]
async fn coalescing_only_merges_into_a_pending_row_never_into_a_sealed_or_leased_one() {
    let scratch = scratch_or_skip!("coalesce-pending-only");
    let workspace_id = seed_workspace(&scratch.db).await;
    seed_webhook(
        &scratch.db,
        workspace_id,
        "http://example.invalid/hook",
        &["flow.content.accepted"],
    )
    .await;
    let document_id = Uuid::new_v4();

    commit_dispatch_work(
        &scratch.db,
        workspace_id,
        "flow.content.accepted",
        Some(document_id),
        Some(1),
        json!({}),
    )
    .await;
    let outcome = expand_one(&scratch.db)
        .await
        .expect("expansion runs")
        .expect("one work item was pending");
    assert!(matches!(outcome, ExpansionOutcome::Expanded));

    let first_delivery_id: Uuid = get_col(
        &scratch.db,
        "SELECT id FROM event_deliveries WHERE subscriber_kind = 'webhook' AND document_id = $1",
        vec![document_id.into()],
        "id",
    )
    .await;

    // Seal it directly -- exactly the state a real row is in the instant `send_one` leases it
    // (pending/sealed -> leased), or once it has hit the coalescing cap.
    exec(
        &scratch.db,
        "UPDATE event_deliveries SET status = 'sealed' WHERE id = $1",
        vec![first_delivery_id.into()],
    )
    .await;

    commit_dispatch_work(
        &scratch.db,
        workspace_id,
        "flow.content.accepted",
        Some(document_id),
        Some(2),
        json!({}),
    )
    .await;
    let outcome2 = expand_one(&scratch.db)
        .await
        .expect("expansion runs")
        .expect("the seq-2 work item is now head");
    assert!(matches!(outcome2, ExpansionOutcome::Expanded));

    #[derive(sea_orm::FromQueryResult)]
    struct Row {
        id: Uuid,
        status: String,
        first_seq: Option<i64>,
    }
    let rows = Row::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "SELECT id, status, first_seq FROM event_deliveries WHERE subscriber_kind = 'webhook' \
         AND document_id = $1 ORDER BY first_seq",
        vec![document_id.into()],
    ))
    .all(&scratch.db)
    .await
    .expect("query runs");

    assert_eq!(
        rows.len(),
        2,
        "a sealed row must not absorb the next event -- a new row opens"
    );
    assert_eq!(rows[0].id, first_delivery_id);
    assert_eq!(rows[0].status, "sealed");
    assert_eq!(rows[0].first_seq, Some(1));
    assert_eq!(rows[1].status, "pending");
    assert_eq!(rows[1].first_seq, Some(2));

    scratch.drop_self().await;
}

#[tokio::test]
async fn coalescing_freezes_the_range_and_source_set_the_instant_the_row_is_leased() {
    let scratch = scratch_or_skip!("coalesce-freeze-on-lease");
    let workspace_id = seed_workspace(&scratch.db).await;
    seed_webhook(
        &scratch.db,
        workspace_id,
        "http://127.0.0.1:1/hook",
        &["flow.content.accepted"],
    )
    .await;
    let document_id = Uuid::new_v4();

    commit_dispatch_work(
        &scratch.db,
        workspace_id,
        "flow.content.accepted",
        Some(document_id),
        Some(1),
        json!({}),
    )
    .await;
    commit_dispatch_work(
        &scratch.db,
        workspace_id,
        "flow.content.accepted",
        Some(document_id),
        Some(2),
        json!({}),
    )
    .await;
    let state = state_for(scratch.db.clone());
    let client = reqwest::Client::new();
    let report = run_tick(&state, &client, 8).await;
    assert_eq!(report.expanded, 2, "{report:?}");

    let delivery_id: Uuid = get_col(
        &scratch.db,
        "SELECT id FROM event_deliveries WHERE subscriber_kind = 'webhook' AND document_id = $1",
        vec![document_id.into()],
        "id",
    )
    .await;

    // Force the debounce to have elapsed and lease it, exactly as `send_one` would.
    exec(
        &scratch.db,
        "UPDATE event_deliveries SET next_attempt_at = now() WHERE id = $1",
        vec![delivery_id.into()],
    )
    .await;
    send_one(&state, &client).await.expect("send_one runs");

    #[derive(sea_orm::FromQueryResult)]
    struct RangeRow {
        status: String,
        first_seq: Option<i64>,
        latest_seq: Option<i64>,
    }
    let leased_before = RangeRow::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "SELECT status, first_seq, latest_seq FROM event_deliveries WHERE id = $1",
        vec![delivery_id.into()],
    ))
    .one(&scratch.db)
    .await
    .expect("query runs")
    .expect("row exists");
    // A failed send against a refused endpoint retries a content delivery straight to
    // `sealed` (never back to `pending`) -- either way it must not still be `leased` once
    // `send_one` returns.
    assert_ne!(leased_before.status, "pending", "{:?}", leased_before.status);
    let source_count_before = count(
        &scratch.db,
        "SELECT count(*) AS n FROM event_delivery_sources WHERE delivery_id = $1",
        vec![delivery_id.into()],
    )
    .await;
    assert_eq!(source_count_before, 2);

    // A third accepted update on the same document must open a *new* pending row -- it must
    // never be able to touch the row that was in flight, whose range and source set are frozen.
    commit_dispatch_work(
        &scratch.db,
        workspace_id,
        "flow.content.accepted",
        Some(document_id),
        Some(3),
        json!({}),
    )
    .await;
    let outcome3 = expand_one(&scratch.db)
        .await
        .expect("expansion runs")
        .expect("the seq-3 work item is head once seq 1/2's work items are already resolved");
    assert!(matches!(outcome3, ExpansionOutcome::Expanded));

    let rows = count(
        &scratch.db,
        "SELECT count(*) AS n FROM event_deliveries WHERE subscriber_kind = 'webhook' AND document_id = $1",
        vec![document_id.into()],
    )
    .await;
    assert_eq!(rows, 2, "the frozen row plus one fresh pending row for seq 3");

    let after = RangeRow::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "SELECT status, first_seq, latest_seq FROM event_deliveries WHERE id = $1",
        vec![delivery_id.into()],
    ))
    .one(&scratch.db)
    .await
    .expect("query runs")
    .expect("row still exists");
    assert_eq!(
        (after.first_seq, after.latest_seq),
        (leased_before.first_seq, leased_before.latest_seq),
        "the once-leased row's range must be unchanged by the later seq-3 event"
    );
    let source_count_after = count(
        &scratch.db,
        "SELECT count(*) AS n FROM event_delivery_sources WHERE delivery_id = $1",
        vec![delivery_id.into()],
    )
    .await;
    assert_eq!(
        source_count_after, source_count_before,
        "the once-leased row's source-table entries must be unchanged by the later seq-3 event"
    );

    scratch.drop_self().await;
}

#[tokio::test]
async fn coalescing_cap_freezes_the_row_and_opens_a_new_one_with_exact_source_counts() {
    let scratch = scratch_or_skip!("coalesce-cap");
    let workspace_id = seed_workspace(&scratch.db).await;
    seed_webhook(
        &scratch.db,
        workspace_id,
        "http://example.invalid/hook",
        &["flow.content.accepted"],
    )
    .await;
    let document_id = Uuid::new_v4();

    // COALESCED_SOURCE_EVENTS_MAX is 20 (private to this module, read through `super::` below
    // rather than restated): committing and expanding CAP+1 sequential accepted updates on the
    // same document must fill exactly one row to the cap and open a second one for the overflow
    // event, never drop or summarize a source entry.
    const CAP: i64 = crate::events::dispatcher::COALESCED_SOURCE_EVENTS_MAX;
    for seq in 1..=(CAP + 1) {
        commit_dispatch_work(
            &scratch.db,
            workspace_id,
            "flow.content.accepted",
            Some(document_id),
            Some(seq),
            json!({}),
        )
        .await;
        let outcome = expand_one(&scratch.db)
            .await
            .unwrap_or_else(|err| panic!("expansion of seq {seq} runs: {err}"))
            .unwrap_or_else(|| panic!("seq {seq}'s work item is head"));
        assert!(matches!(outcome, ExpansionOutcome::Expanded), "seq {seq}");
    }

    #[derive(sea_orm::FromQueryResult)]
    struct Row {
        id: Uuid,
        status: String,
        first_seq: Option<i64>,
        latest_seq: Option<i64>,
    }
    let rows = Row::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "SELECT id, status, first_seq, latest_seq FROM event_deliveries WHERE subscriber_kind = 'webhook' \
         AND document_id = $1 ORDER BY first_seq",
        vec![document_id.into()],
    ))
    .all(&scratch.db)
    .await
    .expect("query runs");

    assert_eq!(
        rows.len(),
        2,
        "the cap must open a second row instead of dropping or summarizing the overflow event"
    );
    assert_eq!(
        rows[0].status, "sealed",
        "the first row must be sealed once it reaches the cap"
    );
    assert_eq!(rows[0].first_seq, Some(1));
    assert_eq!(rows[0].latest_seq, Some(CAP));
    assert_eq!(rows[1].status, "pending");
    assert_eq!(rows[1].first_seq, Some(CAP + 1));

    let sources_first = count(
        &scratch.db,
        "SELECT count(*) AS n FROM event_delivery_sources WHERE delivery_id = $1",
        vec![rows[0].id.into()],
    )
    .await;
    assert_eq!(
        sources_first, CAP,
        "the sealed row's source count must equal the cap exactly, no summary"
    );
    let sources_second = count(
        &scratch.db,
        "SELECT count(*) AS n FROM event_delivery_sources WHERE delivery_id = $1",
        vec![rows[1].id.into()],
    )
    .await;
    assert_eq!(sources_second, 1);

    scratch.drop_self().await;
}

// =============================================================================================
// Gate: coalescing_seal_and_source_first_expansion
// =============================================================================================

#[tokio::test]
async fn dispatch_fifo_barrier_blocks_a_newer_same_document_work_item_while_an_older_one_is_in_flight() {
    let scratch = scratch_or_skip!("fifo-barrier-blocks");
    let workspace_id = seed_workspace(&scratch.db).await;
    seed_webhook(
        &scratch.db,
        workspace_id,
        "http://example.invalid/hook",
        &["flow.content.accepted"],
    )
    .await;
    let document_id = Uuid::new_v4();
    commit_dispatch_work(
        &scratch.db,
        workspace_id,
        "flow.content.accepted",
        Some(document_id),
        Some(1),
        json!({}),
    )
    .await;
    commit_dispatch_work(
        &scratch.db,
        workspace_id,
        "flow.content.accepted",
        Some(document_id),
        Some(2),
        json!({}),
    )
    .await;
    let seq1_id: Uuid = get_col(
        &scratch.db,
        "SELECT id FROM event_dispatch WHERE workspace_id = $1 AND accepted_seq = 1",
        vec![workspace_id.into()],
        "id",
    )
    .await;

    // A genuine second, still-open transaction on an independent connection takes the exact
    // `FOR UPDATE` row lock `expand_one`'s own candidate-selection subquery's `FOR UPDATE SKIP
    // LOCKED` would contend on -- not a fabricated `lease_token`/`lease_expires_at` stamp on
    // the same connection. `gate-commands.md`'s literal requirement: "把旧 sealed 行锁在另一个
    // 事务里再启动第二个 worker" -- a materially stronger proof than an UPDATE, because it
    // exercises the real Postgres row-lock contention `SKIP LOCKED` must respect, not just the
    // lease columns `expand_one`'s WHERE clause also happens to check.
    let locker = scratch.second_connection().await;
    let locker_tx = locker.begin().await.expect("locker transaction begins");
    locker_tx
        .query_one(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT id FROM event_dispatch WHERE id = $1 FOR UPDATE",
            vec![seq1_id.into()],
        ))
        .await
        .expect("lock query runs")
        .expect("seq 1's row exists to be locked");

    // Seq 2 must not be pickable while seq 1 -- still `status = 'pending'`, merely row-locked
    // by the still-open second transaction above -- is an older eligible predecessor on the
    // same document.
    let outcome = expand_one(&scratch.db).await.expect("query runs");
    assert!(
        outcome.is_none(),
        "seq 2 must not be selectable while seq 1 (its older same-document predecessor) is \
         still pending and genuinely row-locked by a real, held second transaction -- SKIP \
         LOCKED must not let a worker skip past a busy head to a newer sibling"
    );

    // Release the real lock and confirm the head is now genuinely leasable -- proves the
    // negative result above came from the held lock, not from some other reason expand_one
    // returned nothing.
    locker_tx
        .rollback()
        .await
        .expect("locker transaction releases its lock");
    let outcome = expand_one(&scratch.db)
        .await
        .expect("query runs")
        .expect("seq 1 becomes leasable the instant the real row lock is released");
    assert!(matches!(
        outcome,
        ExpansionOutcome::NoSubscribers | ExpansionOutcome::Expanded
    ));

    scratch.drop_self().await;
}

#[tokio::test]
async fn dispatch_fifo_barrier_orders_by_accepted_seq_not_created_at_when_both_rows_share_a_timestamp() {
    // `gate-commands.md`'s literal requirement: "并补一组 created_at 相同、first_seq 不同的固定
    // fixture" -- an implementation that orders the head-of-queue candidate by `created_at`
    // (or insertion order) instead of `accepted_seq` would pass every other FIFO fixture in
    // this file (they all insert in seq order) but silently misorder this one.
    let scratch = scratch_or_skip!("fifo-barrier-same-created-at");
    let workspace_id = seed_workspace(&scratch.db).await;
    let document_id = Uuid::new_v4();

    let event_2 = commit_dispatch_work(
        &scratch.db,
        workspace_id,
        "flow.content.accepted",
        Some(document_id),
        Some(2),
        json!({}),
    )
    .await;
    let dispatch_2_id: Uuid = get_col(
        &scratch.db,
        "SELECT id FROM event_dispatch WHERE event_id = $1",
        vec![event_2.into()],
        "id",
    )
    .await;
    let same_created_at: Timestamp = get_col(
        &scratch.db,
        "SELECT created_at FROM event_dispatch WHERE event_id = $1",
        vec![event_2.into()],
        "created_at",
    )
    .await;

    let event_1 = commit_dispatch_work(
        &scratch.db,
        workspace_id,
        "flow.content.accepted",
        Some(document_id),
        Some(1),
        json!({}),
    )
    .await;
    let dispatch_1_id: Uuid = get_col(
        &scratch.db,
        "SELECT id FROM event_dispatch WHERE event_id = $1",
        vec![event_1.into()],
        "id",
    )
    .await;
    // Pin the seq-1 row's `created_at` to exactly the seq-2 row's own -- two independent
    // `commit_dispatch_work` calls would each stamp their own `now()` and could never
    // reproduce an exact tie deterministically otherwise.
    exec(
        &scratch.db,
        "UPDATE event_dispatch SET created_at = $2 WHERE id = $1",
        vec![dispatch_1_id.into(), same_created_at.into()],
    )
    .await;

    seed_webhook(
        &scratch.db,
        workspace_id,
        "http://example.invalid/hook",
        &["flow.content.accepted"],
    )
    .await;

    // Real `expand_one` call (not a hand-duplicated copy of its candidate-selection SQL): the
    // seq-2 row must still be `pending` afterwards, and the seq-1 row must be the one that
    // actually got expanded, even though both share the exact same `created_at`.
    let outcome = expand_one(&scratch.db)
        .await
        .expect("expansion query runs")
        .expect("one of the two same-created_at rows is head");
    assert!(matches!(outcome, ExpansionOutcome::Expanded));

    let dispatch_1_status: String = get_col(
        &scratch.db,
        "SELECT status FROM event_dispatch WHERE id = $1",
        vec![dispatch_1_id.into()],
        "status",
    )
    .await;
    let dispatch_2_status: String = get_col(
        &scratch.db,
        "SELECT status FROM event_dispatch WHERE id = $1",
        vec![dispatch_2_id.into()],
        "status",
    )
    .await;
    assert_eq!(
        dispatch_1_status, "expanded",
        "accepted_seq=1 must be the row expand_one picked, even though created_at ties with seq=2"
    );
    assert_eq!(
        dispatch_2_status, "pending",
        "accepted_seq=2 must still be blocked behind its older same-document sibling"
    );

    scratch.drop_self().await;
}

#[tokio::test]
async fn dispatch_head_definition_three_branches_release_the_queue_head() {
    let scratch = scratch_or_skip!("head-three-branches");
    let workspace_id = seed_workspace(&scratch.db).await;
    let document_id = Uuid::new_v4();

    // Branch 1: `no_subscribers` (a resolved-but-not-`expanded` terminal state) must release
    // the head for its successor, even though it "was never expanded" in the literal sense.
    commit_dispatch_work(
        &scratch.db,
        workspace_id,
        "flow.content.accepted",
        Some(document_id),
        Some(1),
        json!({}),
    )
    .await;
    // No webhook registered yet: seq 1 terminalizes as no_subscribers.
    let outcome1 = expand_one(&scratch.db)
        .await
        .expect("expansion runs")
        .expect("seq 1 is head");
    assert!(matches!(outcome1, ExpansionOutcome::NoSubscribers));

    seed_webhook(
        &scratch.db,
        workspace_id,
        "http://example.invalid/hook",
        &["flow.content.accepted"],
    )
    .await;
    commit_dispatch_work(
        &scratch.db,
        workspace_id,
        "flow.content.accepted",
        Some(document_id),
        Some(2),
        json!({}),
    )
    .await;
    let outcome2 = expand_one(&scratch.db)
        .await
        .expect("expansion runs")
        .expect("seq 2 must be head now that seq 1 has left status=pending");
    assert!(
        matches!(outcome2, ExpansionOutcome::Expanded),
        "a no_subscribers predecessor must not permanently block its successor"
    );

    // Branch 2: `failed` (dispatch-side dead-letter, via exhausted lease reclaims) must also
    // release the head, leaving a seq gap for the consumer's own connectivity checking.
    commit_dispatch_work(
        &scratch.db,
        workspace_id,
        "flow.content.accepted",
        Some(document_id),
        Some(3),
        json!({}),
    )
    .await;
    let dispatch3_id: Uuid = get_col(
        &scratch.db,
        "SELECT id FROM event_dispatch WHERE document_id = $1 AND accepted_seq = 3",
        vec![document_id.into()],
        "id",
    )
    .await;
    exec(
        &scratch.db,
        "UPDATE event_dispatch SET status = 'failed', expanded_at = now(), \
         last_error_code = 'expansion_failed' WHERE id = $1",
        vec![dispatch3_id.into()],
    )
    .await;

    commit_dispatch_work(
        &scratch.db,
        workspace_id,
        "flow.content.accepted",
        Some(document_id),
        Some(4),
        json!({}),
    )
    .await;
    let outcome4 = expand_one(&scratch.db)
        .await
        .expect("expansion runs")
        .expect("seq 4 must be head: failed predecessors release the head too");
    assert!(matches!(outcome4, ExpansionOutcome::Expanded));

    // Branch 3: a `pending` row that is merely *leased* (not yet resolved) does NOT release the
    // head -- already covered by `dispatch_fifo_barrier_blocks_...` above; asserted here as a
    // negative control against the same document to complete the three-way partition.
    commit_dispatch_work(
        &scratch.db,
        workspace_id,
        "flow.content.accepted",
        Some(document_id),
        Some(5),
        json!({}),
    )
    .await;
    commit_dispatch_work(
        &scratch.db,
        workspace_id,
        "flow.content.accepted",
        Some(document_id),
        Some(6),
        json!({}),
    )
    .await;
    exec(
        &scratch.db,
        "UPDATE event_dispatch SET lease_token = 'in-flight', lease_expires_at = now() + interval '1 minute' \
         WHERE document_id = $1 AND accepted_seq = 5",
        vec![document_id.into()],
    )
    .await;
    let outcome6 = expand_one(&scratch.db).await.expect("query runs");
    assert!(
        outcome6.is_none(),
        "an in-flight (still pending) seq 5 must keep blocking seq 6"
    );

    scratch.drop_self().await;
}

#[tokio::test]
async fn non_content_work_is_not_starved_while_a_content_documents_queue_head_is_blocked() {
    let scratch = scratch_or_skip!("anti-starvation");
    let workspace_id = seed_workspace(&scratch.db).await;
    seed_webhook(
        &scratch.db,
        workspace_id,
        "http://example.invalid/hook",
        &["flow.content.accepted", "flow.object.created"],
    )
    .await;
    let document_id = Uuid::new_v4();

    // A content document whose head is permanently blocked (an in-flight lease on seq 1) sits
    // in the backlog first...
    commit_dispatch_work(
        &scratch.db,
        workspace_id,
        "flow.content.accepted",
        Some(document_id),
        Some(1),
        json!({}),
    )
    .await;
    exec(
        &scratch.db,
        "UPDATE event_dispatch SET lease_token = 'in-flight', lease_expires_at = now() + interval '5 minutes' \
         WHERE document_id = $1 AND accepted_seq = 1",
        vec![document_id.into()],
    )
    .await;
    commit_dispatch_work(
        &scratch.db,
        workspace_id,
        "flow.content.accepted",
        Some(document_id),
        Some(2),
        json!({}),
    )
    .await;

    // ...followed by an unrelated non-content event.
    let non_content_event =
        commit_dispatch_work(&scratch.db, workspace_id, "flow.object.created", None, None, json!({})).await;

    // The blocked document's seq 2 must never be selectable, but the non-content work item must
    // still be picked up promptly -- the head-of-queue predicate only ever applies to content
    // events (`events-v1.md`'s third "坑").
    let outcome = expand_one(&scratch.db)
        .await
        .expect("expansion runs")
        .expect("the unrelated non-content work item must be selectable");
    assert!(matches!(outcome, ExpansionOutcome::Expanded));
    assert_eq!(
        count(
            &scratch.db,
            "SELECT count(*) AS n FROM event_deliveries WHERE event_id = $1",
            vec![non_content_event.into()],
        )
        .await,
        1,
        "the non-content event must not be starved by the blocked content document"
    );

    let still_none = count(
        &scratch.db,
        "SELECT count(*) AS n FROM event_deliveries WHERE document_id = $1",
        vec![document_id.into()],
    )
    .await;
    assert_eq!(still_none, 0, "seq 2 on the blocked document must remain unexpanded");

    scratch.drop_self().await;
}

/// The merged-row counterpart of `re_expanding_the_same_work_after_a_simulated_crash_...`
/// above: `ADR-0011` Â§2.1 requires the crash/re-expansion idempotency proof specifically for a
/// *coalesced* delivery (multiple source events bound to one row), not just a plain one-event
/// delivery -- the two have different code paths (`bind_content_delivery`'s merge branch vs.
/// `bind_plain_delivery`) and a bug could exist in either independently.
#[tokio::test]
async fn re_expanding_a_coalesced_delivery_after_a_simulated_crash_does_not_duplicate_source_rows() {
    let scratch = scratch_or_skip!("reexpansion-idempotent-coalesced");
    let workspace_id = seed_workspace(&scratch.db).await;
    seed_webhook(
        &scratch.db,
        workspace_id,
        "http://example.invalid/hook",
        &["flow.content.accepted"],
    )
    .await;
    let document_id = Uuid::new_v4();
    for seq in 1..=3 {
        commit_dispatch_work(
            &scratch.db,
            workspace_id,
            "flow.content.accepted",
            Some(document_id),
            Some(seq),
            json!({}),
        )
        .await;
    }
    let report = run_tick(&state_for(scratch.db.clone()), &reqwest::Client::new(), 8).await;
    assert_eq!(report.expanded, 3, "{report:?}");

    let delivery_id: Uuid = get_col(
        &scratch.db,
        "SELECT id FROM event_deliveries WHERE subscriber_kind = 'webhook' AND document_id = $1",
        vec![document_id.into()],
        "id",
    )
    .await;
    assert_eq!(
        count(
            &scratch.db,
            "SELECT count(*) AS n FROM event_delivery_sources WHERE delivery_id = $1",
            vec![delivery_id.into()],
        )
        .await,
        3,
        "all 3 merged accepted updates must be registered as source rows before the simulated crash"
    );

    // Simulate the dispatcher crashing after all 3 work items were already expanded into the
    // one merged delivery row: put all 3 dispatch rows back to pending, exactly what
    // `reclaim_expired_dispatch_leases` does to abandoned leases.
    exec(
        &scratch.db,
        "UPDATE event_dispatch SET status = 'pending', expanded_at = NULL, lease_token = NULL, \
         lease_expires_at = NULL WHERE document_id = $1",
        vec![document_id.into()],
    )
    .await;
    let report2 = run_tick(&state_for(scratch.db.clone()), &reqwest::Client::new(), 8).await;
    assert_eq!(report2.expanded, 3, "{report2:?}");

    assert_eq!(
        count(
            &scratch.db,
            "SELECT count(*) AS n FROM event_delivery_sources WHERE delivery_id = $1",
            vec![delivery_id.into()],
        )
        .await,
        3,
        "no source event may be counted twice across the crash + re-expansion (ADR-0011 Â§2.1)"
    );
    assert_eq!(
        count(
            &scratch.db,
            "SELECT count(*) AS n FROM event_deliveries WHERE subscriber_kind = 'webhook' AND document_id = $1",
            vec![document_id.into()],
        )
        .await,
        1,
        "no second delivery row was minted by the re-expansion"
    );

    scratch.drop_self().await;
}
