//! Unit tests of the stream's pure parts: the open-stream limits, request parsing, the response
//! body and the mapping from grant state to an end reason.
#![allow(
    clippy::expect_used,
    clippy::unwrap_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::too_many_lines
)]

use futures_util::StreamExt;

use super::*;

fn hub(per_grant: usize, per_user: usize, per_process: usize) -> Arc<Hub> {
    Hub::new(
        StreamLimits {
            per_grant,
            per_user,
            per_process,
        },
        StreamTuning::default(),
    )
}

#[test]
fn the_contract_defaults() {
    let config = OAuthConfig::with_defaults("https://sv.example.test");
    let limits = StreamLimits::from_config(&config);
    assert_eq!(limits.per_grant, 3);
    assert_eq!(limits.per_user, 20);
    assert_eq!(limits.per_process, 2000);
    let tuning = StreamTuning::default();
    assert_eq!(tuning.keepalive, Duration::from_secs(20));
    assert_eq!(tuning.channel_capacity, 256);
    assert_eq!(tuning.lag_timeout, Duration::from_secs(30));
    assert_eq!(tuning.batch, 200);
    assert!(
        tuning.grant_cache_ttl < tuning.idle_tick,
        "events never outlive the revalidation tick"
    );
    assert!(tuning.idle_tick < Duration::from_secs(5), "the revocation bound is 5 s");
}

#[test]
fn limits_are_enforced_per_grant_per_user_and_per_process() {
    let hub = hub(3, 5, 6);
    let (user, other_user) = (Uuid::new_v4(), Uuid::new_v4());
    let grant = Uuid::new_v4();
    let mut held: Vec<Permit> = (0..3)
        .map(|_| hub.try_acquire(grant, user).expect("within the grant limit"))
        .collect();
    assert_eq!(hub.try_acquire(grant, user).err(), Some(LimitExceeded::Grant));
    // Another grant of the same user is still fine until the user limit.
    held.push(hub.try_acquire(Uuid::new_v4(), user).expect("second grant"));
    held.push(hub.try_acquire(Uuid::new_v4(), user).expect("third grant"));
    assert_eq!(hub.try_acquire(Uuid::new_v4(), user).err(), Some(LimitExceeded::User));
    // Another user is limited only by the process cap.
    held.push(hub.try_acquire(Uuid::new_v4(), other_user).expect("other user"));
    assert_eq!(hub.open_streams(), 6);
    assert_eq!(
        hub.try_acquire(Uuid::new_v4(), other_user).err(),
        Some(LimitExceeded::Process)
    );
    // A released place is available again, and the books balance.
    held.pop();
    assert_eq!(hub.open_streams(), 5);
    held.push(hub.try_acquire(Uuid::new_v4(), other_user).expect("room again"));
    drop(held);
    assert_eq!(hub.open_streams(), 0);
    assert!(hub.registry.lock().grants.is_empty() && hub.registry.lock().users.is_empty());
}

#[test]
fn types_parse_to_a_subset_of_the_catalogue() {
    let filter = parse_types("task.created, notification.read").expect("known types");
    assert!(filter.contains(EventKind::TaskCreated) && filter.contains(EventKind::NotificationRead));
    assert!(!filter.contains(EventKind::NotificationCreated));
    for bad in [
        "",
        "task",
        "task.created,",
        "task.created,,notification.read",
        "TASK.CREATED",
    ] {
        assert!(parse_types(bad).is_err(), "{bad:?}");
    }
}

fn headers(pairs: &[(&'static str, &'static str)]) -> HeaderMap {
    let mut map = HeaderMap::new();
    for (name, value) in pairs {
        map.append(*name, HeaderValue::from_static(value));
    }
    map
}

#[test]
fn only_one_bearer_header_is_accepted() {
    assert_eq!(
        bearer_token(&headers(&[("authorization", "Bearer sva_abc")])).expect("token"),
        "sva_abc"
    );
    assert_eq!(
        bearer_token(&headers(&[("authorization", "bearer sva_abc")])).expect("case-insensitive scheme"),
        "sva_abc"
    );
    let refused = [
        headers(&[]),
        headers(&[("authorization", "Basic abc")]),
        headers(&[("authorization", "Bearer")]),
        headers(&[("authorization", "Bearer ")]),
        headers(&[("authorization", "Bearer two tokens")]),
        headers(&[("authorization", "sva_abc")]),
        headers(&[("authorization", "Bearer sva_a"), ("authorization", "Bearer sva_b")]),
        headers(&[("cookie", "access_token=sva_abc")]),
    ];
    for map in refused {
        assert!(bearer_token(&map).is_err(), "{map:?}");
    }
}

#[test]
fn the_header_cursor_wins_over_the_query_cursor() {
    let query = HashMap::from([("cursor".to_owned(), "from-query".to_owned())]);
    assert_eq!(
        presented_cursor(&headers(&[("last-event-id", "from-header")]), &query),
        Some("from-header")
    );
    assert_eq!(presented_cursor(&headers(&[]), &query), Some("from-query"));
    assert_eq!(
        presented_cursor(&headers(&[("last-event-id", "")]), &query),
        Some("from-query")
    );
    assert_eq!(presented_cursor(&headers(&[]), &HashMap::new()), None);
    let blank = HashMap::from([("cursor".to_owned(), String::new())]);
    assert_eq!(presented_cursor(&headers(&[]), &blank), None);
}

type Mutation = fn(&mut LiveRow);

fn live_row() -> LiveRow {
    LiveRow {
        grant_status: "active".to_owned(),
        revoke_reason: None,
        grant_live: true,
        project_mode: "all".to_owned(),
        grant_session_version: 3,
        token_revoked: false,
        token_remaining_ms: 60_000,
        client_active: true,
        user_active: true,
        user_session_version: 3,
        is_member: true,
        allows_apps: true,
        project_ids: None,
    }
}

#[test]
fn each_failed_condition_maps_to_its_revocation_code() {
    assert_eq!(state_revocation(&live_row()), None);
    let cases: [(Mutation, RevokeReason); 6] = [
        (|row| row.grant_live = false, RevokeReason::Expired),
        (|row| row.client_active = false, RevokeReason::ClientDisabled),
        (|row| row.user_active = false, RevokeReason::AccountDisabled),
        (|row| row.user_session_version = 4, RevokeReason::PasswordChanged),
        (|row| row.is_member = false, RevokeReason::MembershipRemoved),
        (|row| row.allows_apps = false, RevokeReason::WorkspaceDisabled),
    ];
    for (mutate, expected) in cases {
        let mut row = live_row();
        mutate(&mut row);
        assert_eq!(state_revocation(&row), Some(expected), "{expected:?}");
    }
}

#[test]
fn end_reasons_have_fixed_audit_codes() {
    assert_eq!(EndReason::Revoked("anything".to_owned()).audit_code(), "revoked");
    assert_eq!(EndReason::Reauth("token_expired").audit_code(), "token_expired");
    assert_eq!(EndReason::Reauth("token_revoked").audit_code(), "token_revoked");
    assert_eq!(EndReason::Lagged.audit_code(), "lagged");
    assert_eq!(EndReason::ClientGone.audit_code(), "client_closed");
    assert_eq!(EndReason::Shutdown.audit_code(), "shutdown");
    assert_eq!(EndReason::Internal.audit_code(), "internal");
    for reason in [
        ResyncReason::CursorTooOld,
        ResyncReason::CursorAhead,
        ResyncReason::TooManyEvents,
    ] {
        assert!(!reason.as_str().is_empty());
    }
}

#[test]
fn the_backfill_window_never_exceeds_what_the_feed_keeps() {
    let mut config = OAuthConfig::with_defaults("https://sv.example.test");
    let context = |config: &OAuthConfig| StreamContext {
        config: Arc::new(config.clone()),
        hub: hub(3, 20, 100),
        key: OnceLock::new(),
        resource_metadata: None,
    };
    assert_eq!(context(&config).window_secs(), 72 * 3600);
    config.event_backfill_max_age_hours = 168;
    config.event_feed_retention_days = 1;
    assert_eq!(context(&config).window_secs(), 86_400, "retention caps the window");
}

fn frame(text: &'static str) -> Bytes {
    Bytes::from_static(text.as_bytes())
}

async fn collect(body: Body) -> Vec<String> {
    let mut stream = body.into_data_stream();
    let mut out = Vec::new();
    while let Some(chunk) = stream.next().await {
        out.push(String::from_utf8(chunk.expect("chunk").to_vec()).expect("utf8"));
    }
    out
}

#[tokio::test]
async fn the_body_delivers_queued_frames_then_ends_when_the_producer_does() {
    let hub = hub(3, 20, 100);
    let permit = hub.try_acquire(Uuid::new_v4(), Uuid::new_v4()).expect("permit");
    let (tx, rx) = mpsc::channel(8);
    let (_close_tx, close_rx) = watch::channel(None);
    tx.send(frame("a")).await.expect("queue");
    tx.send(frame("b")).await.expect("queue");
    drop(tx);
    assert_eq!(collect(body(rx, close_rx, permit)).await, ["a", "b"]);
    assert_eq!(hub.open_streams(), 0, "the place is released with the body");
}

#[tokio::test]
async fn a_closing_frame_replaces_whatever_is_still_queued() {
    let hub = hub(3, 20, 100);
    let permit = hub.try_acquire(Uuid::new_v4(), Uuid::new_v4()).expect("permit");
    let (tx, rx) = mpsc::channel(8);
    let (close_tx, close_rx) = watch::channel(None);
    for text in ["queued-1", "queued-2", "queued-3"] {
        tx.send(frame(text)).await.expect("queue");
    }
    close_tx.send_replace(Some(frame("revoked")));
    // The producer stays alive, as a stuck one would: the close still ends the body.
    let frames = tokio::time::timeout(Duration::from_secs(2), collect(body(rx, close_rx, permit)))
        .await
        .expect("the body ends");
    assert_eq!(frames, ["revoked"], "no queued event follows a revocation");
    drop(tx);
}

#[tokio::test]
async fn a_close_without_a_frame_just_ends_the_body() {
    let hub = hub(3, 20, 100);
    let permit = hub.try_acquire(Uuid::new_v4(), Uuid::new_v4()).expect("permit");
    let (tx, rx) = mpsc::channel(8);
    let (close_tx, close_rx) = watch::channel(None);
    tx.send(frame("queued")).await.expect("queue");
    close_tx.send_replace(None);
    let frames = tokio::time::timeout(Duration::from_secs(2), collect(body(rx, close_rx, permit)))
        .await
        .expect("the body ends");
    assert!(frames.is_empty());
    drop(tx);
}

#[tokio::test]
async fn a_dropped_body_releases_its_place() {
    let hub = hub(1, 20, 100);
    let permit = hub.try_acquire(Uuid::new_v4(), Uuid::new_v4()).expect("permit");
    let (_tx, rx) = mpsc::channel::<Bytes>(8);
    let (_close_tx, close_rx) = watch::channel(None);
    let response = body(rx, close_rx, permit);
    assert_eq!(hub.open_streams(), 1);
    drop(response);
    assert_eq!(hub.open_streams(), 0);
}

#[tokio::test]
async fn shutdown_reaches_every_live_hub_of_a_registry() {
    // A registry of its own: the process-wide one is shared with the other tests of this binary.
    let registry = Mutex::new(Vec::new());
    let (first, second) = (hub(3, 20, 100), hub(3, 20, 100));
    let dead = hub(3, 20, 100);
    for hub in [&first, &second, &dead] {
        registry.lock().push(Arc::downgrade(hub));
    }
    drop(dead);
    let mut watchers = [first.shutdown.subscribe(), second.shutdown.subscribe()];
    assert!(watchers.iter().all(|watcher| !*watcher.borrow()));
    shutdown_registered(&registry);
    for watcher in &mut watchers {
        watcher.changed().await.expect("signalled");
        assert!(*watcher.borrow());
    }
    assert_eq!(registry.lock().len(), 2, "a hub that is gone is forgotten");
}

#[test]
fn without_configuration_the_path_is_not_registered_as_a_stream() {
    // The router builds; the 404 behaviour is covered against the real router in the database tests.
    let _ = routes(None);
}
