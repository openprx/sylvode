//! Dispatcher database tests: sending, retry, cancellation and the delivery body.

use super::*;

#[test]
fn flow_delivery_retry_backoff_matches_every_frozen_attempt() {
    assert_eq!(
        (1..=10).map(delivery_backoff_ms).collect::<Vec<_>>(),
        vec![
            30_000, 60_000, 90_000, 120_000, 150_000, 180_000, 210_000, 240_000, 270_000, 300_000
        ]
    );
}

#[test]
fn delivery_envelope_redacts_record_content_from_metadata() {
    let event = BusinessEventRow {
        id: Uuid::new_v4(),
        workspace_id: Some(Uuid::new_v4()),
        source_flow_scope_id: None,
        project_id: None,
        event_type: "flow.record.created".to_string(),
        aggregate_type: "flow_record".to_string(),
        aggregate_id: Uuid::new_v4().to_string(),
        actor_id: Some(Uuid::new_v4()),
        source: json!({"surface": "rest"}),
        payload: json!({"collection_id": "collection", "record_id": "record"}),
        metadata: json!({
            "idempotency_body": {
                "properties": {"salary": "SECRET-SALARY-9001"},
                "body": "SECRET-BODY-NOTE"
            },
            "idempotency_fingerprint": "internal",
            "message": "SECRET-MESSAGE"
        }),
        correlation_id: None,
        causation_id: None,
        created_at: Timestamp::now(),
    };

    let envelope = envelope_json(&event, None, None, None);
    let encoded = serde_json::to_string(&envelope).expect("envelope serializes");
    assert_eq!(envelope["metadata"], json!({}));
    for secret in ["SECRET-SALARY-9001", "SECRET-BODY-NOTE", "SECRET-MESSAGE"] {
        assert!(!encoded.contains(secret), "delivery envelope leaked {secret}");
    }
}

/// The events a project bot connection writes carry the runner's working directory, a path on
/// the runner's host. No reader outside the admin group gets one: a webhook delivery withholds
/// the payload of any type without a delivery policy, and these types have none.
#[test]
fn connection_events_reach_a_webhook_without_their_payload() {
    for event_type in [
        "project.bot_connected",
        "project.bot_connection_updated",
        "project.bot_connection_disabled",
        "project.bot_connection_enabled",
        "project.bot_disconnected",
        "bot.credentials_rotated",
    ] {
        let event = BusinessEventRow {
            id: Uuid::new_v4(),
            workspace_id: Some(Uuid::new_v4()),
            source_flow_scope_id: None,
            project_id: Some(Uuid::new_v4()),
            event_type: event_type.to_string(),
            aggregate_type: "project".to_string(),
            aggregate_id: Uuid::new_v4().to_string(),
            actor_id: Some(Uuid::new_v4()),
            source: json!({"type": "user"}),
            payload: json!({
                "working_directory": "/srv/dev/SECRET-HOST-PATH",
                "before": {"working_directory": "/srv/dev/SECRET-OLD-PATH"},
                "after": {"working_directory": "/srv/dev/SECRET-NEW-PATH"},
            }),
            metadata: json!({}),
            correlation_id: None,
            causation_id: None,
            created_at: Timestamp::now(),
        };
        let encoded = serde_json::to_string(&envelope_json(&event, None, None, None)).expect("serializes");
        assert!(
            !encoded.contains("SECRET-"),
            "{event_type} leaked a host path: {encoded}"
        );
    }
}

/// Test-only constructor: production code only ever builds a `DispatchLeaseRow`/
/// `DeliveryLeaseRow` from a `RETURNING` clause; this lets [`build_delivery_body`] (a private
/// function this module tests directly) be exercised against a delivery already created by
/// [`run_tick`] above without re-running the lease `UPDATE`.

#[tokio::test]
async fn send_one_leases_a_delivery_and_retries_it_when_the_endpoint_is_rejected() {
    let scratch = scratch_or_skip!("send-retry");
    let workspace_id = seed_workspace(&scratch.db).await;
    // Loopback is refused by `resolve_outbound_target`'s SSRF guard under the fallback (no
    // `[outbound] allow_private`) runtime configuration every unit test in this binary shares,
    // exactly like `webhook_trigger.rs`'s own `internal_targets_are_refused_before_the_request_is_built`
    // test asserts. This proves the full lease -> build body -> validate -> retry path, short of
    // an actual successful send.
    seed_webhook(
        &scratch.db,
        workspace_id,
        "http://127.0.0.1:1/hook",
        &["flow.object.created"],
    )
    .await;
    let event_id = commit_dispatch_work(&scratch.db, workspace_id, "flow.object.created", None, None, json!({})).await;

    let state = state_for(scratch.db.clone());
    let client = reqwest::Client::new();
    expand_one(&scratch.db).await.expect("expansion runs");

    let outcome = send_one(&state, &client).await.expect("send_one runs");
    assert_eq!(
        outcome,
        Some(false),
        "a rejected endpoint is a handled failure, not a dispatcher error"
    );

    #[derive(sea_orm::FromQueryResult)]
    struct Row {
        status: String,
        attempts: i32,
        last_error_code: Option<String>,
        lease_token: Option<String>,
    }
    let row = Row::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "SELECT status, attempts, last_error_code, lease_token FROM event_deliveries WHERE event_id = $1",
        vec![event_id.into()],
    ))
    .one(&scratch.db)
    .await
    .expect("delivery query runs")
    .expect("one delivery row exists");
    assert_eq!(
        row.status, "pending",
        "non-content deliveries retry back to pending, never sealed"
    );
    assert_eq!(row.attempts, 1);
    assert_eq!(row.last_error_code.as_deref(), Some("endpoint_rejected"));
    assert!(
        row.lease_token.is_none(),
        "the lease must be released before the next attempt"
    );

    scratch.drop_self().await;
}

#[tokio::test]
async fn delivery_attempts_one_through_ten_write_the_frozen_database_backoff_and_never_attempt_eleven() {
    async fn read_request(socket: &mut tokio::net::TcpStream) {
        let mut bytes = Vec::new();
        loop {
            let mut chunk = [0_u8; 4096];
            let read = socket.read(&mut chunk).await.expect("request is readable");
            assert!(read > 0, "request ended before its declared body");
            bytes.extend_from_slice(&chunk[..read]);
            let Some(header_end) = bytes.windows(4).position(|window| window == b"\r\n\r\n") else {
                continue;
            };
            let headers = String::from_utf8_lossy(&bytes[..header_end]);
            let content_length = headers
                .lines()
                .find_map(|line| {
                    let (name, value) = line.split_once(':')?;
                    name.eq_ignore_ascii_case("content-length")
                        .then(|| value.trim().parse::<usize>().expect("content-length is numeric"))
                })
                .unwrap_or(0);
            if bytes.len() >= header_end + 4 + content_length {
                return;
            }
        }
    }

    let listener = TcpListener::bind("127.0.0.1:0").await.expect("loopback listener binds");
    let address = listener.local_addr().expect("listener address is available");
    let receiver = tokio::spawn(async move {
        for _ in 1..=10 {
            let (mut socket, _) = listener.accept().await.expect("delivery connection arrives");
            read_request(&mut socket).await;
            socket
                .write_all(b"HTTP/1.1 503 Service Unavailable\r\nContent-Length: 0\r\nConnection: close\r\n\r\n")
                .await
                .expect("failure response writes");
        }
    });

    let scratch = scratch_or_skip!("delivery-backoff-database");
    let workspace_id = seed_workspace(&scratch.db).await;
    let webhook_id = seed_webhook(
        &scratch.db,
        workspace_id,
        &format!("http://{address}/hook"),
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
    let replay = replay_deliveries(
        &scratch.db,
        &ReplayRequest {
            workspace_id,
            mode: ReplayMode::Rebuild,
            event_type: Some("flow.object.created".to_string()),
            subscriber_kind: Some("webhook".to_string()),
            subscriber_id: Some(webhook_id),
            from: now - time::Duration::minutes(1),
            to: now,
            dry_run: false,
        },
        now,
    )
    .await
    .expect("admin replay builds a delivery");
    let delivery_id = match replay {
        ReplayResult::Rebuild {
            replayed: 1,
            rebuilt_delivery_ids,
            ..
        } => rebuilt_delivery_ids[0],
        other => panic!("unexpected replay result: {other:?}"),
    };
    #[derive(FromQueryResult)]
    struct ReplayShape {
        dispatch_id: Option<Uuid>,
        document_id: Option<Uuid>,
        first_seq: Option<i64>,
        latest_seq: Option<i64>,
        status: String,
    }
    let replay_shape = ReplayShape::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "SELECT dispatch_id,document_id,first_seq,latest_seq,status FROM event_deliveries WHERE id=$1",
        vec![delivery_id.into()],
    ))
    .one(&scratch.db)
    .await
    .expect("replay shape query runs")
    .expect("replay delivery exists");
    assert_eq!(
        (
            replay_shape.dispatch_id,
            replay_shape.document_id,
            replay_shape.first_seq,
            replay_shape.latest_seq,
            replay_shape.status.as_str(),
        ),
        (None, None, None, None, "pending"),
        "replay rows bypass event_dispatch and cannot enter live document coalescing"
    );
    let state = state_for(scratch.db.clone());
    let client = reqwest::Client::new();

    #[derive(FromQueryResult)]
    struct RetryRow {
        status: String,
        attempts: i32,
        next_attempt_at: Timestamp,
    }
    for attempt in 1_i64..=10 {
        exec(
            &scratch.db,
            "UPDATE event_deliveries SET next_attempt_at=now() WHERE id=$1",
            vec![delivery_id.into()],
        )
        .await;
        let before: Timestamp = get_col(&scratch.db, "SELECT clock_timestamp() AS ts", vec![], "ts").await;
        let outcome = TEST_ALLOW_PRIVATE_DELIVERY_TARGET
            .scope(true, send_one(&state, &client))
            .await
            .expect("delivery attempt runs");
        assert_eq!(outcome, Some(false), "HTTP 503 is a handled failed attempt");
        let after: Timestamp = get_col(&scratch.db, "SELECT clock_timestamp() AS ts", vec![], "ts").await;
        let row = RetryRow::find_by_statement(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT status,attempts,next_attempt_at FROM event_deliveries WHERE id=$1",
            vec![delivery_id.into()],
        ))
        .one(&scratch.db)
        .await
        .expect("retry row query runs")
        .expect("retry row remains present");
        let expected = time::Duration::milliseconds((attempt * 30_000).min(300_000));
        assert!(
            row.next_attempt_at >= before + expected && row.next_attempt_at <= after + expected,
            "attempt {attempt} wrote {} outside the database-observed [{}, {}] schedule",
            row.next_attempt_at,
            before + expected,
            after + expected
        );
        assert_eq!(i64::from(row.attempts), attempt);
        assert_eq!(row.status, if attempt == 10 { "failed" } else { "pending" });
    }
    receiver.await.expect("receiver handled exactly ten attempts");
    assert_eq!(
        TEST_ALLOW_PRIVATE_DELIVERY_TARGET
            .scope(true, send_one(&state, &client))
            .await
            .expect("post-exhaustion dispatcher scan runs"),
        None,
        "a failed row cannot produce attempt eleven"
    );

    scratch.drop_self().await;
}

#[tokio::test]
async fn flow_delivery_failure_then_fresh_dispatcher_delivers_once_with_the_same_delivery_id() {
    async fn read_request(socket: &mut tokio::net::TcpStream) -> String {
        let mut bytes = Vec::new();
        loop {
            let mut chunk = [0_u8; 4096];
            let read = socket.read(&mut chunk).await.expect("request is readable");
            assert!(read > 0, "request ended before its declared body");
            bytes.extend_from_slice(&chunk[..read]);
            let Some(header_end) = bytes.windows(4).position(|window| window == b"\r\n\r\n") else {
                continue;
            };
            let headers = String::from_utf8_lossy(&bytes[..header_end]);
            let content_length = headers
                .lines()
                .find_map(|line| {
                    let (name, value) = line.split_once(':')?;
                    name.eq_ignore_ascii_case("content-length")
                        .then(|| value.trim().parse::<usize>().expect("content-length is numeric"))
                })
                .unwrap_or(0);
            if bytes.len() >= header_end + 4 + content_length {
                return String::from_utf8(bytes).expect("delivery request is UTF-8");
            }
        }
    }

    const CHILD_ROLE: &str = "OPENPR_DELIVERY_ATTEMPT_CHILD";
    const CHILD_DATABASE_URL: &str = "OPENPR_DELIVERY_ATTEMPT_DATABASE_URL";

    // The same libtest binary is the dispatcher executable for this fixture. The parent starts
    // it as a separate OS process and kills that whole process after the receiver has read the
    // request but before it responds. A fresh process then reclaims the durable lease and sends
    // the same row again. This is deliberately not a task abort or a fresh in-process client.
    if std::env::var_os(CHILD_ROLE).is_some() {
        let database_url = std::env::var(CHILD_DATABASE_URL).expect("child database URL is set");
        let db = Database::connect(database_url)
            .await
            .expect("child connects to scratch database");
        let result = TEST_ALLOW_PRIVATE_DELIVERY_TARGET
            .scope(true, send_one(&state_for(db.clone()), &reqwest::Client::new()))
            .await
            .expect("child dispatcher attempt runs");
        assert!(result.is_some(), "child must lease one delivery");
        db.close().await.expect("child closes its database pool");
        return;
    }

    let listener = TcpListener::bind("127.0.0.1:0").await.expect("loopback listener binds");
    let address = listener.local_addr().expect("listener address is available");
    let (first_request_started_tx, first_request_started_rx) = tokio::sync::oneshot::channel();
    let receiver = tokio::spawn(async move {
        let mut requests = Vec::new();
        let (mut first_socket, _) = listener.accept().await.expect("first delivery connection arrives");
        requests.push(read_request(&mut first_socket).await);
        first_request_started_tx
            .send(())
            .expect("parent still waits for the in-flight attempt");
        let mut eof = [0_u8; 1];
        assert_eq!(
            first_socket.read(&mut eof).await.expect("killed child closes socket"),
            0
        );

        let (mut second_socket, _) = listener.accept().await.expect("restarted delivery connection arrives");
        requests.push(read_request(&mut second_socket).await);
        second_socket
            .write_all(b"HTTP/1.1 204 No Content\r\nContent-Length: 0\r\nConnection: close\r\n\r\n")
            .await
            .expect("success response writes");
        second_socket.flush().await.expect("success response flushes");
        requests
    });

    let scratch = scratch_or_skip!("failure-restart-success");
    let workspace_id = seed_workspace(&scratch.db).await;
    seed_webhook(
        &scratch.db,
        workspace_id,
        &format!("http://{address}/hook"),
        &["flow.object.created"],
    )
    .await;
    let event_id = commit_dispatch_work(&scratch.db, workspace_id, "flow.object.created", None, None, json!({})).await;
    expand_one(&scratch.db).await.expect("expansion runs");
    let delivery_id: Uuid = get_col(
        &scratch.db,
        "SELECT id FROM event_deliveries WHERE event_id=$1",
        vec![event_id.into()],
        "id",
    )
    .await;

    let test_name = "events::dispatcher::tests::deliver::flow_delivery_failure_then_fresh_dispatcher_delivers_once_with_the_same_delivery_id";
    let mut killed_dispatcher = tokio::process::Command::new(std::env::current_exe().expect("test executable exists"))
        .arg("--exact")
        .arg(test_name)
        .arg("--nocapture")
        .env(CHILD_ROLE, "in-flight")
        .env(CHILD_DATABASE_URL, &scratch.url)
        .kill_on_drop(true)
        .spawn()
        .expect("dispatcher child starts");
    first_request_started_rx
        .await
        .expect("receiver observed the in-flight attempt");
    killed_dispatcher
        .kill()
        .await
        .expect("whole dispatcher process is killed");
    let killed_status = killed_dispatcher.wait().await.expect("killed dispatcher is reaped");
    assert!(!killed_status.success(), "the first dispatcher really died mid-attempt");

    #[derive(FromQueryResult)]
    struct LeasedRow {
        id: Uuid,
        status: String,
        lease_token: Option<String>,
    }
    let interrupted = LeasedRow::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "SELECT id,status,lease_token FROM event_deliveries WHERE id=$1",
        vec![delivery_id.into()],
    ))
    .one(&scratch.db)
    .await
    .expect("interrupted row query runs")
    .expect("interrupted delivery remains durable");
    assert_eq!(interrupted.id, delivery_id);
    assert_eq!(interrupted.status, "leased");
    assert!(
        interrupted.lease_token.is_some(),
        "the killed attempt left its durable lease"
    );
    exec(
        &scratch.db,
        "UPDATE event_deliveries SET lease_expires_at=now()-interval '1 millisecond' WHERE id=$1",
        vec![delivery_id.into()],
    )
    .await;
    assert_eq!(
        reclaim_expired_delivery_leases(&scratch.db)
            .await
            .expect("expired lease is reclaimed"),
        1
    );
    exec(
        &scratch.db,
        "UPDATE event_deliveries SET next_attempt_at=now() WHERE id=$1",
        vec![delivery_id.into()],
    )
    .await;

    let restarted_status = tokio::process::Command::new(std::env::current_exe().expect("test executable exists"))
        .arg("--exact")
        .arg(test_name)
        .arg("--nocapture")
        .env(CHILD_ROLE, "restarted")
        .env(CHILD_DATABASE_URL, &scratch.url)
        .status()
        .await
        .expect("restarted dispatcher child runs");
    assert!(
        restarted_status.success(),
        "fresh dispatcher process must deliver the reclaimed row"
    );

    #[derive(FromQueryResult)]
    struct Row {
        status: String,
        attempts: i32,
    }
    let row = Row::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "SELECT status,attempts FROM event_deliveries WHERE id=$1",
        vec![delivery_id.into()],
    ))
    .one(&scratch.db)
    .await
    .expect("delivery query runs")
    .expect("delivery remains visible");
    assert_eq!(row.status, "dispatched");
    assert_eq!(
        row.attempts, 1,
        "lease recovery consumes exactly one crash-attempt budget slot"
    );

    let requests = receiver.await.expect("receiver task completes");
    assert_eq!(requests.len(), 2);
    let expected_header = format!("x-sylvode-delivery-id: {delivery_id}");
    let mut consumer_seen_delivery_ids = std::collections::HashSet::new();
    for request in &requests {
        assert!(
            request.to_ascii_lowercase().contains(&expected_header),
            "every attempt carries the immutable consumer dedupe key"
        );
        let key = request
            .lines()
            .find_map(|line| {
                let (name, value) = line.split_once(':')?;
                name.eq_ignore_ascii_case("x-sylvode-delivery-id")
                    .then(|| value.trim().to_owned())
            })
            .expect("consumer reads delivery_id from the required header");
        consumer_seen_delivery_ids.insert(key);
    }
    assert_eq!(
        consumer_seen_delivery_ids.len(),
        1,
        "consumer delivery_id dedupe observes one delivery"
    );
    let first_body = requests[0].split("\r\n\r\n").nth(1).expect("first body exists");
    let second_body = requests[1].split("\r\n\r\n").nth(1).expect("second body exists");
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(first_body).unwrap()["delivery"]["attempt"],
        1
    );
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(second_body).unwrap()["delivery"]["attempt"],
        2
    );

    scratch.drop_self().await;
}

#[tokio::test]
async fn flow_delivery_v08_reads_the_current_endpoint_and_rotated_secret_only_at_send_time() {
    async fn read_request(socket: &mut tokio::net::TcpStream) -> String {
        let mut bytes = Vec::new();
        loop {
            let mut chunk = [0_u8; 4096];
            let read = socket.read(&mut chunk).await.expect("request is readable");
            assert!(read > 0, "request ended before its declared body");
            bytes.extend_from_slice(&chunk[..read]);
            let Some(header_end) = bytes.windows(4).position(|window| window == b"\r\n\r\n") else {
                continue;
            };
            let headers = String::from_utf8_lossy(&bytes[..header_end]);
            let content_length = headers
                .lines()
                .find_map(|line| {
                    let (name, value) = line.split_once(':')?;
                    name.eq_ignore_ascii_case("content-length")
                        .then(|| value.trim().parse::<usize>().expect("content-length is numeric"))
                })
                .unwrap_or(0);
            if bytes.len() >= header_end + 4 + content_length {
                return String::from_utf8(bytes).expect("request is UTF-8");
            }
        }
    }

    let listener = TcpListener::bind("127.0.0.1:0").await.expect("loopback listener binds");
    let address = listener.local_addr().expect("listener address is available");
    let receiver = tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.expect("delivery connection arrives");
        let request = read_request(&mut socket).await;
        socket
            .write_all(b"HTTP/1.1 204 No Content\r\nContent-Length: 0\r\nConnection: close\r\n\r\n")
            .await
            .expect("response writes");
        request
    });

    let scratch = scratch_or_skip!("current-endpoint-secret");
    let workspace_id = seed_workspace(&scratch.db).await;
    let webhook_id = seed_webhook(
        &scratch.db,
        workspace_id,
        "http://old.example.invalid/hook",
        &["flow.object.created"],
    )
    .await;
    let event_id = commit_dispatch_work(&scratch.db, workspace_id, "flow.object.created", None, None, json!({})).await;
    expand_one(&scratch.db)
        .await
        .expect("expansion takes the subscriber snapshot");
    exec(
        &scratch.db,
        "UPDATE webhooks SET url=$2,secret='rotated-secret' WHERE id=$1",
        vec![webhook_id.into(), format!("http://{address}/new-hook").into()],
    )
    .await;
    let outcome = TEST_ALLOW_PRIVATE_DELIVERY_TARGET
        .scope(true, send_one(&state_for(scratch.db.clone()), &reqwest::Client::new()))
        .await
        .expect("send runs");
    assert_eq!(outcome, Some(true), "the old expansion-time endpoint was not retained");
    let request = receiver.await.expect("receiver completes");
    assert!(request.starts_with("POST /new-hook "));
    let (headers, body) = request.split_once("\r\n\r\n").expect("request has a body boundary");
    let signature = headers
        .lines()
        .find_map(|line| {
            let (name, value) = line.split_once(':')?;
            name.eq_ignore_ascii_case(crate::webhook_trigger::WEBHOOK_SIGNATURE_HEADER)
                .then(|| value.trim())
        })
        .expect("signature header exists");
    let expected = crate::webhook_trigger::sign_payload("rotated-secret", body).expect("rotated payload signs");
    assert_eq!(signature, format!("sha256={expected}"));
    let user_agent = headers
        .lines()
        .find_map(|line| {
            let (name, value) = line.split_once(':')?;
            name.eq_ignore_ascii_case("user-agent").then(|| value.trim())
        })
        .expect("user-agent header exists");
    assert_eq!(
        user_agent, "Sylvode-Webhook/1.0",
        "the dispatcher sends the same User-Agent as every other webhook delivery"
    );
    assert_eq!(
        count(
            &scratch.db,
            "SELECT count(*) AS n FROM event_deliveries WHERE event_id=$1 AND status='dispatched'",
            vec![event_id.into()],
        )
        .await,
        1
    );

    scratch.drop_self().await;
}

#[tokio::test]
async fn build_delivery_body_reuses_the_same_delivery_id_across_retries_only_the_attempt_number_changes() {
    let scratch = scratch_or_skip!("dedupe-delivery-id");
    let workspace_id = seed_workspace(&scratch.db).await;
    seed_webhook(
        &scratch.db,
        workspace_id,
        "http://example.invalid/hook",
        &["flow.object.created"],
    )
    .await;
    let event_id = commit_dispatch_work(&scratch.db, workspace_id, "flow.object.created", None, None, json!({})).await;
    expand_one(&scratch.db).await.expect("expansion runs");

    let delivery_id: Uuid = get_col(
        &scratch.db,
        "SELECT id FROM event_deliveries WHERE event_id = $1",
        vec![event_id.into()],
        "id",
    )
    .await;

    let row_attempt_0 = delivery_lease_row_for_test(delivery_id, event_id, None);
    let mut row_attempt_2 = delivery_lease_row_for_test(delivery_id, event_id, None);
    row_attempt_2.attempts = 2;

    let body_first = build_delivery_body(&scratch.db, &row_attempt_0)
        .await
        .expect("body builds");
    let body_retry = build_delivery_body(&scratch.db, &row_attempt_2)
        .await
        .expect("body builds");

    assert_eq!(
        body_first["delivery"]["id"], body_retry["delivery"]["id"],
        "delivery_id must be immutable across retries"
    );
    assert_eq!(body_first["delivery"]["id"], json!(delivery_id));
    assert_eq!(body_first["delivery"]["attempt"], 1);
    assert_eq!(body_retry["delivery"]["attempt"], 3, "attempt is attempts+1");
    assert_eq!(
        body_first["event"]["event_id"], body_retry["event"]["event_id"],
        "retries do not mint a new event_id"
    );

    scratch.drop_self().await;
}

#[tokio::test]
async fn subscriber_deleted_mid_flight_cancels_the_delivery_as_a_terminal_state_distinct_from_failed() {
    let scratch = scratch_or_skip!("cancelled-distinct-terminal");
    let workspace_id = seed_workspace(&scratch.db).await;
    let webhook_id = seed_webhook(
        &scratch.db,
        workspace_id,
        "http://example.invalid/hook",
        &["flow.object.created"],
    )
    .await;
    let event_id = commit_dispatch_work(&scratch.db, workspace_id, "flow.object.created", None, None, json!({})).await;
    expand_one(&scratch.db).await.expect("expansion runs");

    // The subscriber is deleted after expansion but before the delivery is ever sent.
    delete_webhook(&scratch.db, webhook_id).await;

    let state = state_for(scratch.db.clone());
    let client = reqwest::Client::new();
    let outcome = send_one(&state, &client).await.expect("send_one runs");
    assert_eq!(outcome, Some(false));

    #[derive(sea_orm::FromQueryResult)]
    struct Row {
        status: String,
        last_error_code: Option<String>,
        attempts: i32,
    }
    let row = Row::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "SELECT status, last_error_code, attempts FROM event_deliveries WHERE event_id = $1",
        vec![event_id.into()],
    ))
    .one(&scratch.db)
    .await
    .expect("query runs")
    .expect("row exists");
    assert_eq!(
        row.status, "cancelled",
        "a gone subscriber must terminate the row as cancelled, not failed"
    );
    assert_eq!(row.last_error_code.as_deref(), Some("subscriber_gone"));
    assert_eq!(
        row.attempts, 0,
        "cancellation must not consume a retry attempt -- it is not a delivery failure"
    );

    scratch.drop_self().await;
}

#[tokio::test]
async fn flow_delivery_v08_deleting_a_subscriber_cancels_its_entire_backlog_without_dead_letter_or_audit_loss() {
    let scratch = scratch_or_skip!("subscriber-backlog-cancelled");
    let workspace_id = seed_workspace(&scratch.db).await;
    let webhook_id = seed_webhook(
        &scratch.db,
        workspace_id,
        "http://example.invalid/hook",
        &["flow.object.created"],
    )
    .await;
    let mut event_ids = Vec::new();
    for _ in 0..3 {
        let event_id =
            commit_dispatch_work(&scratch.db, workspace_id, "flow.object.created", None, None, json!({})).await;
        event_ids.push(event_id);
        expand_one(&scratch.db).await.expect("expansion runs");
    }
    delete_webhook(&scratch.db, webhook_id).await;
    let state = state_for(scratch.db.clone());
    let client = reqwest::Client::new();
    for _ in 0..3 {
        assert_eq!(
            send_one(&state, &client).await.expect("cancellation attempt runs"),
            Some(false)
        );
    }
    assert_eq!(send_one(&state, &client).await.expect("queue scan runs"), None);
    assert_eq!(
        count(
            &scratch.db,
            "SELECT count(*) AS n FROM event_deliveries \
              WHERE subscriber_id=$1 AND status='cancelled' AND last_error_code='subscriber_gone'",
            vec![webhook_id.into()],
        )
        .await,
        3
    );
    assert_eq!(
        count(
            &scratch.db,
            "SELECT count(*) AS n FROM event_deliveries WHERE workspace_id=$1 AND status='failed'",
            vec![workspace_id.into()],
        )
        .await,
        0,
        "cancelled backlog cannot inflate dead-letter count or oldest_failed_age"
    );
    assert_eq!(
        count(
            &scratch.db,
            "SELECT count(*) AS n FROM business_events WHERE id=ANY($1)",
            vec![event_ids.into()],
        )
        .await,
        3,
        "subscriber removal must not delete audit facts"
    );

    scratch.drop_self().await;
}

#[tokio::test]
async fn delivery_dead_letter_row_is_visible_immediately_and_survives_until_its_own_retention_window() {
    let scratch = scratch_or_skip!("dead-letter-visibility");
    let workspace_id = seed_workspace(&scratch.db).await;
    seed_webhook(
        &scratch.db,
        workspace_id,
        "http://127.0.0.1:1/hook",
        &["flow.object.created"],
    )
    .await;
    let event_id = commit_dispatch_work(&scratch.db, workspace_id, "flow.object.created", None, None, json!({})).await;
    expand_one(&scratch.db).await.expect("expansion runs");

    let state = state_for(scratch.db.clone());
    let client = reqwest::Client::new();

    // Drive it through all 10 attempts (`delivery_max_attempts`) by repeatedly clearing the
    // backoff and sending again -- clock-advancement via backdating `next_attempt_at`, not
    // sleeping through the real backoff ladder.
    for attempt in 1..=10 {
        exec(
            &scratch.db,
            "UPDATE event_deliveries SET next_attempt_at = now() WHERE event_id = $1",
            vec![event_id.into()],
        )
        .await;
        let outcome = send_one(&state, &client)
            .await
            .expect("send_one runs")
            .unwrap_or_else(|| panic!("a delivery was ready to lease on attempt {attempt}"));
        assert!(!outcome, "the refused endpoint fails every attempt");
    }

    #[derive(sea_orm::FromQueryResult)]
    struct Row {
        status: String,
        attempts: i32,
        last_error_code: Option<String>,
    }
    let row = Row::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "SELECT status, attempts, last_error_code FROM event_deliveries WHERE event_id = $1",
        vec![event_id.into()],
    ))
    .one(&scratch.db)
    .await
    .expect("query runs")
    .expect("row exists");
    assert_eq!(
        row.status, "failed",
        "attempts exhausted must land in the failed dead-letter terminal state"
    );
    assert_eq!(row.attempts, 10);
    assert!(row.last_error_code.is_some());

    // Dead-letter rows must remain queryable immediately, and are not touched by the reaper
    // until their own (`delivery_retention_days`) window elapses.
    assert_eq!(
        crate::events::dispatcher::retention::reap_delivery_retention(&scratch.db, Timestamp::now())
            .await
            .expect("reaper runs"),
        0
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

// =============================================================================================
// Golden wire fixtures (`events-v1.md` "投递报文与 delivery_id 的位置"): plain / coalesced /
// retry. This crate does not own `testing/fixtures/flow-delivery-v1/` (outside apps/api and
// apps/worker), so the three frozen shapes are asserted inline here instead of as checked-in
// fixture files.
// =============================================================================================

#[tokio::test]
async fn golden_wire_fixture_plain_delivery_body_matches_the_frozen_shape() {
    let scratch = scratch_or_skip!("golden-plain");
    let workspace_id = seed_workspace(&scratch.db).await;
    seed_webhook(
        &scratch.db,
        workspace_id,
        "http://example.invalid/hook",
        &["flow.object.created"],
    )
    .await;
    let event_id = commit_dispatch_work(
        &scratch.db,
        workspace_id,
        "flow.object.created",
        None,
        None,
        json!({ "object_id": "11111111-1111-1111-1111-111111111111" }),
    )
    .await;
    expand_one(&scratch.db).await.expect("expansion runs");
    let delivery_id: Uuid = get_col(
        &scratch.db,
        "SELECT id FROM event_deliveries WHERE event_id = $1",
        vec![event_id.into()],
        "id",
    )
    .await;

    let body = build_delivery_body(&scratch.db, &delivery_lease_row_for_test(delivery_id, event_id, None))
        .await
        .expect("body builds");

    // A non-coalesced delivery's body is frozen as exactly
    // `{delivery:{id,attempt,coalesced:false}, event:{...openpr.event.v1}}` -- not just "some
    // fields present".
    assert_eq!(body["delivery"]["id"], json!(delivery_id));
    assert_eq!(body["delivery"]["attempt"], json!(1));
    assert_eq!(body["delivery"]["coalesced"], json!(false));
    assert_eq!(
        body["delivery"].as_object().expect("object").len(),
        3,
        "plain delivery must have exactly {{id,attempt,coalesced}}, no range/source_event_ids/block_ids_truncated"
    );
    assert_eq!(body["event"]["version"], json!("openpr.event.v1"));
    assert_eq!(body["event"]["event_id"], json!(event_id));
    assert_eq!(body["event"]["event_type"], json!("flow.object.created"));
    assert_eq!(
        normalize_delivery_body(&body, 0, 0),
        frozen_delivery_fixture("plain"),
        "the real producer must remain byte-shape compatible with the frozen plain consumer fixture"
    );

    scratch.drop_self().await;
}

#[tokio::test]
async fn golden_wire_fixture_coalesced_delivery_body_matches_the_frozen_shape() {
    let scratch = scratch_or_skip!("golden-coalesced");
    let workspace_id = seed_workspace(&scratch.db).await;
    seed_webhook(
        &scratch.db,
        workspace_id,
        "http://example.invalid/hook",
        &["flow.content.accepted"],
    )
    .await;
    let document_id = Uuid::new_v4();
    let block_a = Uuid::new_v4();
    let block_b = Uuid::new_v4();
    let event_1 = commit_dispatch_work(
        &scratch.db,
        workspace_id,
        "flow.content.accepted",
        Some(document_id),
        Some(1),
        json!({ "changed_block_ids": [block_a] }),
    )
    .await;
    let event_2 = commit_dispatch_work(
        &scratch.db,
        workspace_id,
        "flow.content.accepted",
        Some(document_id),
        Some(2),
        json!({ "changed_block_ids": [block_b] }),
    )
    .await;
    let state = state_for(scratch.db.clone());
    run_tick(&state, &reqwest::Client::new(), 8).await;

    #[derive(sea_orm::FromQueryResult)]
    struct Row {
        id: Uuid,
        first_seq: Option<i64>,
        latest_seq: Option<i64>,
    }
    let row = Row::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "SELECT id, first_seq, latest_seq FROM event_deliveries WHERE subscriber_kind = 'webhook' \
         AND document_id = $1",
        vec![document_id.into()],
    ))
    .one(&scratch.db)
    .await
    .expect("query runs")
    .expect("coalesced delivery row exists");

    // Built directly from the real row's own `first_seq`/`latest_seq` (not the always-`None`
    // test constructor) so the golden `range` assertion below reflects DB truth.
    let lease_row = crate::events::dispatcher::deliver::DeliveryLeaseRow {
        id: row.id,
        event_id: event_1,
        subscriber_id: Uuid::new_v4(),
        document_id: Some(document_id),
        attempts: 0,
        max_attempts: 10,
        first_seq: row.first_seq,
        latest_seq: row.latest_seq,
    };
    let body = build_delivery_body(&scratch.db, &lease_row).await.expect("body builds");

    assert_eq!(body["delivery"]["id"], json!(row.id));
    assert_eq!(body["delivery"]["coalesced"], json!(true));
    assert_eq!(body["delivery"]["range"]["first_seq"], json!(1));
    assert_eq!(body["delivery"]["range"]["latest_seq"], json!(2));
    assert_eq!(body["delivery"]["block_ids_truncated"], json!(false));
    let source_ids = body["delivery"]["source_event_ids"].as_array().expect("array");
    assert_eq!(source_ids.len(), 2);
    assert!(source_ids.contains(&json!(event_1)));
    assert!(source_ids.contains(&json!(event_2)));
    let correct_consumer_keys =
        std::iter::once(body["delivery"]["id"].clone()).collect::<std::collections::HashSet<_>>();
    let wrong_event_id_consumer_keys = source_ids.iter().cloned().collect::<std::collections::HashSet<_>>();
    assert_eq!(
        correct_consumer_keys.len(),
        1,
        "one delivery_id means one consumer delivery"
    );
    assert_eq!(
        wrong_event_id_consumer_keys.len(),
        2,
        "negative consumer fixture: treating each coalesced event_id as the dedupe key observably processes one delivery twice"
    );
    assert_eq!(body["event"]["event_type"], json!("flow.content.accepted"));
    assert_eq!(
        body["event"]["event_id"],
        json!(event_1),
        "the envelope anchors on the first source event as lineage"
    );
    let block_ids = body["event"]["payload"]["changed_block_ids"].as_array().expect("array");
    assert_eq!(block_ids.len(), 2);
    assert!(block_ids.contains(&json!(block_a)));
    assert!(block_ids.contains(&json!(block_b)));
    assert_eq!(
        normalize_delivery_body(&body, 2, 2),
        frozen_delivery_fixture("coalesced"),
        "the real producer must remain byte-shape compatible with the frozen coalesced consumer fixture"
    );

    scratch.drop_self().await;
}

#[tokio::test]
async fn flow_delivery_v08_changed_block_union_exact_limit_and_plus_one_truncation() {
    let scratch = scratch_or_skip!("changed-block-union-boundary");
    let workspace_id = seed_workspace(&scratch.db).await;
    seed_webhook(
        &scratch.db,
        workspace_id,
        "http://example.invalid/hook",
        &["flow.content.accepted"],
    )
    .await;
    let document_id = Uuid::new_v4();
    let block_ids = (0..200).map(|_| Uuid::new_v4()).collect::<Vec<_>>();
    let event_1 = commit_dispatch_work(
        &scratch.db,
        workspace_id,
        "flow.content.accepted",
        Some(document_id),
        Some(1),
        json!({ "changed_block_ids": &block_ids[..100] }),
    )
    .await;
    commit_dispatch_work(
        &scratch.db,
        workspace_id,
        "flow.content.accepted",
        Some(document_id),
        Some(2),
        json!({ "changed_block_ids": &block_ids[100..] }),
    )
    .await;
    expand_one(&scratch.db).await.expect("first expansion runs");
    expand_one(&scratch.db).await.expect("second expansion runs");

    #[derive(FromQueryResult)]
    struct DeliveryRange {
        id: Uuid,
        first_seq: Option<i64>,
        latest_seq: Option<i64>,
    }
    let row = DeliveryRange::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "SELECT id,first_seq,latest_seq FROM event_deliveries WHERE subscriber_kind='webhook' AND document_id=$1",
        vec![document_id.into()],
    ))
    .one(&scratch.db)
    .await
    .expect("delivery query runs")
    .expect("coalesced row exists");
    let lease = crate::events::dispatcher::deliver::DeliveryLeaseRow {
        id: row.id,
        event_id: event_1,
        subscriber_id: Uuid::new_v4(),
        document_id: Some(document_id),
        attempts: 0,
        max_attempts: 10,
        first_seq: row.first_seq,
        latest_seq: row.latest_seq,
    };
    let exact = build_delivery_body(&scratch.db, &lease)
        .await
        .expect("exact body builds");
    assert_eq!(exact["delivery"]["block_ids_truncated"], json!(false));
    let exact_ids = exact["event"]["payload"]["changed_block_ids"]
        .as_array()
        .expect("exact union is present");
    assert_eq!(exact_ids.len(), 200);
    assert_eq!(
        exact_ids
            .iter()
            .cloned()
            .collect::<std::collections::HashSet<_>>()
            .len(),
        block_ids.len()
    );

    let forbidden_body_id = Uuid::new_v4();
    commit_dispatch_work(
        &scratch.db,
        workspace_id,
        "flow.content.accepted",
        Some(document_id),
        Some(3),
        json!({ "changed_block_ids": [forbidden_body_id], "body": "MUST-NOT-LEAK" }),
    )
    .await;
    expand_one(&scratch.db).await.expect("plus-one expansion runs");
    let plus_one_range = DeliveryRange::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "SELECT id,first_seq,latest_seq FROM event_deliveries WHERE id=$1",
        vec![row.id.into()],
    ))
    .one(&scratch.db)
    .await
    .expect("delivery query runs")
    .expect("same delivery remains");
    let plus_one = build_delivery_body(
        &scratch.db,
        &crate::events::dispatcher::deliver::DeliveryLeaseRow {
            latest_seq: plus_one_range.latest_seq,
            ..lease
        },
    )
    .await
    .expect("plus-one body builds");
    assert_eq!(plus_one["delivery"]["range"], json!({"first_seq": 1, "latest_seq": 3}));
    assert_eq!(plus_one["delivery"]["block_ids_truncated"], json!(true));
    assert_eq!(
        plus_one["event"]["payload"],
        json!({"changed_block_ids_truncated": true})
    );
    let encoded = serde_json::to_string(&plus_one).expect("body serializes");
    assert!(!encoded.contains(&forbidden_body_id.to_string()));
    assert!(!encoded.contains("MUST-NOT-LEAK"));

    scratch.drop_self().await;
}

#[tokio::test]
async fn golden_wire_fixture_retry_reuses_delivery_id_and_only_advances_attempt() {
    let scratch = scratch_or_skip!("golden-retry");
    let workspace_id = seed_workspace(&scratch.db).await;
    seed_webhook(
        &scratch.db,
        workspace_id,
        "http://example.invalid/hook",
        &["flow.object.created"],
    )
    .await;
    let event_id = commit_dispatch_work(&scratch.db, workspace_id, "flow.object.created", None, None, json!({})).await;
    expand_one(&scratch.db).await.expect("expansion runs");
    let delivery_id: Uuid = get_col(
        &scratch.db,
        "SELECT id FROM event_deliveries WHERE event_id = $1",
        vec![event_id.into()],
        "id",
    )
    .await;

    let attempt_1 = delivery_lease_row_for_test(delivery_id, event_id, None);
    let mut attempt_4 = delivery_lease_row_for_test(delivery_id, event_id, None);
    attempt_4.attempts = 3;

    let body_1 = build_delivery_body(&scratch.db, &attempt_1).await.expect("body builds");
    let body_4 = build_delivery_body(&scratch.db, &attempt_4).await.expect("body builds");

    assert_eq!(
        body_1["delivery"]["id"], body_4["delivery"]["id"],
        "retries must carry the same delivery_id header/body value -- the consumer's dedup key"
    );
    assert_eq!(body_1["delivery"]["attempt"], json!(1));
    assert_eq!(body_4["delivery"]["attempt"], json!(4));
    assert_eq!(
        body_1["event"], body_4["event"],
        "the event envelope itself is identical across retries"
    );
    assert_eq!(
        json!({
            "attempt_1": normalize_delivery_body(&body_1, 0, 0),
            "attempt_4": normalize_delivery_body(&body_4, 0, 0),
        }),
        frozen_delivery_fixture("retry"),
        "the real retry producer must remain compatible with the frozen consumer fixture"
    );

    scratch.drop_self().await;
}
