//! The burst queue, the source IP allow-list, the escalation notice, the 401 for a product secret
//! header and the narrowed schema, against PostgreSQL through the real inbound route.
#![cfg(test)]
#![allow(
    clippy::expect_used,
    clippy::unwrap_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::too_many_lines,
    clippy::similar_names,
    clippy::items_after_statements,
    clippy::print_stderr,
    clippy::doc_markdown
)]

use std::{net::SocketAddr, sync::Arc};

use axum::http::StatusCode;
use platform::{config::InboundConfig, timestamp::Timestamp};
use sea_orm::ConnectionTrait;
use serde_json::{Value, json};
use uuid::Uuid;

use super::{
    INBOUND_EVENTS_PATH,
    db_tests::{RuleSpec, World, enabled},
    followup::{run_once, run_once_at},
    queue::drain,
};
use crate::{
    oauth::{
        delegated::{DelegatedAuthContext, with_context},
        scopes::{ProjectScope, ScopeSet, TokenResource},
    },
    routes::context::tenant_fixture::exec,
};

macro_rules! world {
    ($label:expr) => {{
        let scratch = crate::scratch_or_skip!($label);
        World::new(scratch, "qu").await
    }};
}

fn kind(mut event: Value, kind: &str) -> Value {
    event["kind"] = json!(kind);
    event
}

async fn complete_active_tasks(w: &World) {
    exec(
        w.db(),
        "UPDATE ai_tasks SET status = 'completed', completed_at = now() WHERE status IN ('pending', 'processing')",
        vec![],
    )
    .await;
}

async fn state_of(w: &World, dedup_key: &str) -> String {
    use sea_orm::{DbBackend, FromQueryResult, Statement};
    #[derive(FromQueryResult)]
    struct Row {
        task_state: String,
    }
    Row::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "SELECT task_state FROM inbound_alerts WHERE dedup_key = $1",
        vec![dedup_key.to_owned().into()],
    ))
    .one(w.db())
    .await
    .unwrap()
    .unwrap()
    .task_state
}

fn second_rule(w: &World) -> RuleSpec {
    RuleSpec {
        name: "Network high to triage",
        position: 20,
        patterns: vec!["net.*".to_owned()],
        ..w.default_rule()
    }
}

// ---------------------------------------------------------------------------------------------
// A product secret header is no credential
// ---------------------------------------------------------------------------------------------

#[tokio::test]
async fn a_request_carrying_only_x_waf_secret_is_refused_with_401() {
    let w = world!("queue_xwaf");
    let body = serde_json::to_vec(&w.event("e-1", "k", "high")).unwrap();
    // The source token in the header a product-specific adapter would have read, with a body and
    // content type that would otherwise be accepted, and no Authorization header at all.
    let (status, _, answer) = w
        .send(
            enabled(),
            &[
                ("x-waf-secret", w.token.clone()),
                ("content-type", "application/json".to_owned()),
            ],
            &body,
            INBOUND_EVENTS_PATH,
        )
        .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED, "{answer}");
    assert_eq!(answer["message"], "invalid_credentials");
    // The same with a valid signature: the signature does not turn the header into a credential.
    let mut headers = w.signed_headers(&body, Timestamp::now().unix_timestamp());
    headers.retain(|(name, _)| *name != "authorization");
    headers.push(("x-waf-secret", w.token.clone()));
    let (status, _, answer) = w.send(enabled(), &headers, &body, INBOUND_EVENTS_PATH).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED, "{answer}");
    assert_eq!(answer["message"], "invalid_credentials");
    assert_eq!(w.n("SELECT count(*) AS count FROM inbound_events").await, 0);
    assert_eq!(w.n("SELECT count(*) AS count FROM work_items").await, 0);
    w.scratch.drop_self().await;
}

// ---------------------------------------------------------------------------------------------
// The queue
// ---------------------------------------------------------------------------------------------

#[tokio::test]
async fn queued_tasks_start_oldest_first_and_fair_across_rules() {
    let w = world!("queue_fair");
    exec(w.db(), "UPDATE alert_rules SET max_active_tasks = 5", vec![]).await;
    w.add_rule(second_rule(&w)).await;
    let config = InboundConfig {
        max_active_alert_tasks_per_workspace: 2,
        ..enabled()
    };
    // Two slots fill at once: one task of each rule. Then rule A queues two alerts before rule B
    // queues one, so the oldest-first order alone would serve A twice.
    for (id, key, name) in [("a1", "ka1", "waf.attack_detected"), ("b1", "kb1", "net.scan_detected")] {
        assert_eq!(
            w.post_with(config, &kind(w.event(id, key, "high"), name)).await.0,
            StatusCode::ACCEPTED
        );
    }
    for (id, key, name) in [
        ("a2", "ka2", "waf.attack_detected"),
        ("a3", "ka3", "waf.attack_detected"),
        ("b2", "kb2", "net.scan_detected"),
    ] {
        assert_eq!(
            w.post_with(config, &kind(w.event(id, key, "high"), name)).await.0,
            StatusCode::ACCEPTED
        );
    }
    assert_eq!(w.n("SELECT count(*) AS count FROM ai_tasks").await, 2);
    assert_eq!(
        w.n("SELECT count(*) AS count FROM inbound_alerts WHERE task_state = 'waiting'")
            .await,
        3
    );

    complete_active_tasks(&w).await;
    let report = run_once(w.db(), &config).await.unwrap();
    assert_eq!(report.started, 2, "two slots are free");
    assert_eq!(state_of(&w, "ka2").await, "created", "the first of rule A");
    assert_eq!(
        state_of(&w, "kb2").await,
        "created",
        "the first of rule B goes before the second of rule A, although that one is older"
    );
    assert_eq!(state_of(&w, "ka3").await, "waiting");

    complete_active_tasks(&w).await;
    assert_eq!(run_once(w.db(), &config).await.unwrap().started, 1);
    assert_eq!(state_of(&w, "ka3").await, "created");
    assert_eq!(w.n("SELECT count(*) AS count FROM ai_tasks").await, 5);
    w.scratch.drop_self().await;
}

#[tokio::test]
async fn a_queued_task_is_suppressed_after_an_hour_by_the_given_clock() {
    let w = world!("queue_expiry");
    exec(w.db(), "UPDATE alert_rules SET max_active_tasks = 1", vec![]).await;
    assert_eq!(w.post(&w.event("e-1", "k-1", "high")).await.0, StatusCode::ACCEPTED);
    assert_eq!(w.post(&w.event("e-2", "k-2", "high")).await.0, StatusCode::ACCEPTED);
    assert_eq!(state_of(&w, "k-2").await, "waiting");

    let now = Timestamp::now();
    let at = |minutes: i64| now + time::Duration::minutes(minutes);
    let report = run_once_at(w.db(), &enabled(), at(59)).await.unwrap();
    assert_eq!((report.started, report.expired), (0, 0));
    assert_eq!(state_of(&w, "k-2").await, "waiting", "59 minutes is inside the hour");

    let report = run_once_at(w.db(), &enabled(), at(61)).await.unwrap();
    assert_eq!(report.expired, 1);
    assert_eq!(state_of(&w, "k-2").await, "suppressed");
    assert_eq!(
        w.n(&format!(
            "SELECT count(*) AS count FROM notifications WHERE type = 'alert_suppressed' AND user_id = '{}' \
             AND content = 'alert_suppressed:WAF high to triage' AND related_issue_id IS NOT NULL",
            w.tenant.member_id
        ))
        .await,
        1,
        "the existing suppression notice"
    );
    assert_eq!(
        w.n(
            "SELECT count(*) AS count FROM business_events WHERE event_type = 'alert_rule.suppressed' \
             AND payload ->> 'cap' = 'queue_timeout'"
        )
        .await,
        1
    );
    assert_eq!(
        w.n("SELECT count(*) AS count FROM ai_tasks").await,
        1,
        "no task was created"
    );
    let report = run_once_at(w.db(), &enabled(), at(62)).await.unwrap();
    assert_eq!(report.expired, 0, "an entry expires once");
    assert_eq!(
        w.n("SELECT count(*) AS count FROM notifications WHERE type = 'alert_suppressed'")
            .await,
        1
    );
    w.scratch.drop_self().await;
}

#[tokio::test]
async fn the_queue_lives_in_the_database_and_two_workers_start_one_task_for_one_slot() {
    let w = world!("queue_restart");
    exec(w.db(), "UPDATE alert_rules SET max_active_tasks = 1", vec![]).await;
    assert_eq!(w.post(&w.event("e-1", "k-1", "high")).await.0, StatusCode::ACCEPTED);
    assert_eq!(w.post(&w.event("e-2", "k-2", "high")).await.0, StatusCode::ACCEPTED);
    assert_eq!(w.post(&w.event("e-3", "k-3", "high")).await.0, StatusCode::ACCEPTED);
    // Everything the queue knows is in the rows: the state, when it was queued and which event
    // asked for the task. A process that starts now has nothing else to go on.
    assert_eq!(
        w.n(
            "SELECT count(*) AS count FROM inbound_alerts a WHERE a.task_state = 'waiting' \
             AND a.task_queued_at IS NOT NULL \
             AND EXISTS (SELECT 1 FROM inbound_events e WHERE e.id = a.queued_event_id AND e.alert_id = a.id)"
        )
        .await,
        2
    );
    let waiting_since: i64 = w
        .n(
            "SELECT count(*) AS count FROM inbound_alerts WHERE task_state = 'waiting' \
            AND task_queued_at <= now()",
        )
        .await;
    assert_eq!(waiting_since, 2);

    // One slot frees. Two workers run the tick together: one task starts, not two.
    complete_active_tasks(&w).await;
    let (first, second) = tokio::join!(drain(w.db(), 5, Timestamp::now()), drain(w.db(), 5, Timestamp::now()));
    assert_eq!(first.unwrap().started + second.unwrap().started, 1);
    assert_eq!(
        w.n("SELECT count(*) AS count FROM ai_tasks WHERE status IN ('pending', 'processing')")
            .await,
        1
    );
    assert_eq!(
        w.n("SELECT count(*) AS count FROM inbound_alerts WHERE task_state = 'waiting'")
            .await,
        1
    );
    // A later tick with the slot taken starts nothing more.
    assert_eq!(drain(w.db(), 5, Timestamp::now()).await.unwrap().started, 0);
    w.scratch.drop_self().await;
}

#[tokio::test]
async fn a_queued_task_passes_the_dispatch_guard_again_when_it_starts() {
    let w = world!("queue_guard");
    exec(w.db(), "UPDATE alert_rules SET max_active_tasks = 1", vec![]).await;
    assert_eq!(w.post(&w.event("e-1", "k-1", "high")).await.0, StatusCode::ACCEPTED);
    assert_eq!(w.post(&w.event("e-2", "k-2", "high")).await.0, StatusCode::ACCEPTED);
    complete_active_tasks(&w).await;

    // Inside a delegated request the guard refuses agent dispatch: the start is refused, nothing
    // is created and the entry keeps waiting.
    let context = DelegatedAuthContext {
        grant_id: Uuid::new_v4(),
        token_id: Uuid::new_v4(),
        client_id: Arc::from("ramflux"),
        user_id: w.admin,
        workspace_id: w.tenant.workspace_id,
        scopes: ScopeSet::all(),
        projects: ProjectScope::All,
        resource: TokenResource::Api,
        surface: crate::middleware::bot_auth::EventSurface::Rest,
        tool_name: None,
        request_id: Uuid::new_v4(),
    };
    let report = with_context(context, async { drain(w.db(), 5, Timestamp::now()).await.unwrap() }).await;
    assert_eq!(report.started, 0);
    assert_eq!(state_of(&w, "k-2").await, "waiting");
    assert_eq!(w.n("SELECT count(*) AS count FROM ai_tasks").await, 1);

    assert_eq!(drain(w.db(), 5, Timestamp::now()).await.unwrap().started, 1);
    assert_eq!(state_of(&w, "k-2").await, "created");
    assert_eq!(
        w.n(
            "SELECT count(*) AS count FROM business_events WHERE event_type = 'ai_task.created' \
             AND source ->> 'type' = 'inbound_rule'"
        )
        .await,
        2,
        "the task started from the queue is recorded with the inbound rule origin like the first"
    );
    w.scratch.drop_self().await;
}

#[tokio::test]
async fn a_waiting_alert_is_dropped_when_its_work_item_closed_or_its_bot_left_or_its_rule_changed() {
    let w = world!("queue_drop");
    exec(w.db(), "UPDATE alert_rules SET max_active_tasks = 1", vec![]).await;
    for (id, key) in [("e-1", "k-1"), ("e-2", "k-2"), ("e-3", "k-3"), ("e-4", "k-4")] {
        assert_eq!(w.post(&w.event(id, key, "high")).await.0, StatusCode::ACCEPTED);
    }
    // k-2's work item is finished while it waits: no task for work nobody wants.
    exec(
        w.db(),
        "UPDATE work_items SET state = 'done' WHERE id = (SELECT work_item_id FROM inbound_alerts WHERE dedup_key = 'k-2')",
        vec![],
    )
    .await;
    complete_active_tasks(&w).await;
    let report = run_once(w.db(), &enabled()).await.unwrap();
    assert_eq!((report.started, report.dropped), (1, 1));
    assert_eq!(state_of(&w, "k-2").await, "suppressed");
    assert_eq!(state_of(&w, "k-3").await, "created");
    assert_eq!(state_of(&w, "k-4").await, "waiting");

    // The rule is disabled while k-4 waits.
    exec(w.db(), "UPDATE alert_rules SET enabled = false", vec![]).await;
    complete_active_tasks(&w).await;
    let report = run_once(w.db(), &enabled()).await.unwrap();
    assert_eq!((report.started, report.dropped), (0, 1));
    assert_eq!(state_of(&w, "k-4").await, "suppressed");
    assert_eq!(w.n("SELECT count(*) AS count FROM ai_tasks").await, 2);
    w.scratch.drop_self().await;
}

#[tokio::test]
async fn a_bot_that_left_while_its_task_waited_gets_no_task_and_the_rule_is_told() {
    let w = world!("queue_nobot");
    exec(w.db(), "UPDATE alert_rules SET max_active_tasks = 1", vec![]).await;
    assert_eq!(w.post(&w.event("e-1", "k-1", "high")).await.0, StatusCode::ACCEPTED);
    assert_eq!(w.post(&w.event("e-2", "k-2", "high")).await.0, StatusCode::ACCEPTED);
    exec(
        w.db(),
        "DELETE FROM workspace_members WHERE user_id = $1",
        vec![w.bot.into()],
    )
    .await;
    complete_active_tasks(&w).await;
    let report = run_once(w.db(), &enabled()).await.unwrap();
    assert_eq!((report.started, report.dropped), (0, 1));
    assert_eq!(state_of(&w, "k-2").await, "suppressed");
    assert_eq!(
        w.n("SELECT count(*) AS count FROM notifications WHERE type = 'alert_suppressed'")
            .await,
        1
    );
    w.scratch.drop_self().await;
}

/// A runner bot whose connection to the project was disabled or removed while its task waited is a
/// settled outcome like a bot that left: the entry is suppressed at once and the rule is told, not
/// kept waiting for the hour.
#[tokio::test]
async fn a_bot_without_an_enabled_connection_settles_the_waiting_task_at_once() {
    let w = world!("queue_noconn");
    super::admin_tests::connect_the_bot(&w).await;
    exec(w.db(), "UPDATE alert_rules SET max_active_tasks = 1", vec![]).await;
    for (id, key) in [("e-1", "k-1"), ("e-2", "k-2"), ("e-3", "k-3")] {
        assert_eq!(w.post(&w.event(id, key, "high")).await.0, StatusCode::ACCEPTED);
    }
    assert_eq!(state_of(&w, "k-2").await, "waiting");
    assert_eq!(state_of(&w, "k-3").await, "waiting");

    exec(
        w.db(),
        "UPDATE project_execution_settings SET enabled = false WHERE project_id = $1",
        vec![w.tenant.project_id.into()],
    )
    .await;
    complete_active_tasks(&w).await;
    let report = run_once(w.db(), &enabled()).await.unwrap();
    assert_eq!((report.started, report.dropped), (0, 2));
    assert_eq!(state_of(&w, "k-2").await, "suppressed");
    assert_eq!(state_of(&w, "k-3").await, "suppressed");
    assert_eq!(w.n("SELECT count(*) AS count FROM ai_tasks").await, 1);
    assert_eq!(
        w.n("SELECT count(*) AS count FROM notifications WHERE type = 'alert_suppressed'")
            .await,
        1,
        "the rule's recipients are told once"
    );

    // A removed connection settles a waiting task the same way.
    exec(
        w.db(),
        "UPDATE project_execution_settings SET enabled = true WHERE project_id = $1",
        vec![w.tenant.project_id.into()],
    )
    .await;
    assert_eq!(w.post(&w.event("e-4", "k-4", "high")).await.0, StatusCode::ACCEPTED);
    assert_eq!(w.post(&w.event("e-5", "k-5", "high")).await.0, StatusCode::ACCEPTED);
    assert_eq!(state_of(&w, "k-5").await, "waiting");
    exec(w.db(), "DELETE FROM project_bot_connections", vec![]).await;
    complete_active_tasks(&w).await;
    let report = run_once(w.db(), &enabled()).await.unwrap();
    assert_eq!((report.started, report.dropped), (0, 1));
    assert_eq!(state_of(&w, "k-5").await, "suppressed");
    w.scratch.drop_self().await;
}

/// At ingest the missing connection is settled before a cap could park the alert in the queue.
#[tokio::test]
async fn a_bot_without_an_enabled_connection_is_not_queued_behind_a_cap() {
    let w = world!("queue_noconn_ingest");
    super::admin_tests::connect_the_bot(&w).await;
    exec(w.db(), "UPDATE alert_rules SET max_active_tasks = 1", vec![]).await;
    assert_eq!(w.post(&w.event("e-1", "k-1", "high")).await.0, StatusCode::ACCEPTED);
    exec(
        w.db(),
        "UPDATE project_execution_settings SET enabled = false WHERE project_id = $1",
        vec![w.tenant.project_id.into()],
    )
    .await;
    // The cap is reached too, yet the alert is not queued.
    assert_eq!(w.post(&w.event("e-2", "k-2", "high")).await.0, StatusCode::ACCEPTED);
    assert_eq!(state_of(&w, "k-2").await, "none");
    assert_eq!(
        w.n("SELECT count(*) AS count FROM inbound_events WHERE outcome = 'bot_unavailable' AND task_id IS NULL")
            .await,
        1
    );
    assert_eq!(w.n("SELECT count(*) AS count FROM ai_tasks").await, 1);
    w.scratch.drop_self().await;
}

#[tokio::test]
async fn a_newcomer_does_not_jump_ahead_of_the_alerts_already_waiting_for_its_rule() {
    let w = world!("queue_newcomer");
    exec(w.db(), "UPDATE alert_rules SET max_active_tasks = 1", vec![]).await;
    assert_eq!(w.post(&w.event("e-1", "k-1", "high")).await.0, StatusCode::ACCEPTED);
    assert_eq!(w.post(&w.event("e-2", "k-2", "high")).await.0, StatusCode::ACCEPTED);
    // The slot frees, but the worker has not ticked yet.
    complete_active_tasks(&w).await;
    assert_eq!(w.post(&w.event("e-3", "k-3", "high")).await.0, StatusCode::ACCEPTED);
    assert_eq!(
        state_of(&w, "k-3").await,
        "waiting",
        "behind k-2, although a slot is free"
    );
    assert_eq!(w.n("SELECT count(*) AS count FROM ai_tasks").await, 1);

    assert_eq!(run_once(w.db(), &enabled()).await.unwrap().started, 1);
    assert_eq!(state_of(&w, "k-2").await, "created", "the older one starts first");
    assert_eq!(state_of(&w, "k-3").await, "waiting");
    w.scratch.drop_self().await;
}

#[tokio::test]
async fn an_alert_with_a_waiting_task_takes_a_new_occurrence_without_a_second_queue_entry() {
    let w = world!("queue_occurrence");
    exec(w.db(), "UPDATE alert_rules SET max_active_tasks = 1", vec![]).await;
    assert_eq!(w.post(&w.event("e-1", "k-1", "high")).await.0, StatusCode::ACCEPTED);
    assert_eq!(w.post(&w.event("e-2", "k-2", "high")).await.0, StatusCode::ACCEPTED);
    assert_eq!(w.post(&w.event("e-3", "k-2", "critical")).await.0, StatusCode::ACCEPTED);
    assert_eq!(
        w.n(
            "SELECT count(*) AS count FROM inbound_alerts WHERE dedup_key = 'k-2' AND occurrences = 2 \
             AND task_state = 'waiting' AND max_severity_rank = 4"
        )
        .await,
        1
    );
    assert_eq!(
        w.n("SELECT count(*) AS count FROM inbound_events WHERE outcome = 'task_queued'")
            .await,
        1,
        "the second occurrence is attached, not queued again"
    );
    // The task that starts carries the alert's highest severity.
    complete_active_tasks(&w).await;
    assert_eq!(run_once(w.db(), &enabled()).await.unwrap().started, 1);
    assert_eq!(
        w.n(
            "SELECT count(*) AS count FROM ai_tasks WHERE payload -> 'alert' ->> 'severity' = 'critical' \
             AND payload -> 'alert' ->> 'occurrences' = '2' AND priority = 40"
        )
        .await,
        1
    );
    w.scratch.drop_self().await;
}

#[tokio::test]
async fn the_hourly_alert_cap_counts_alerts_whose_task_is_queued() {
    let w = world!("queue_alert_cap");
    exec(w.db(), "UPDATE alert_rules SET max_active_tasks = 1", vec![]).await;
    let config = InboundConfig {
        max_alerts_per_rule_per_hour: 2,
        ..enabled()
    };
    for index in 0..3 {
        assert_eq!(
            w.post_with(config, &w.event(&format!("e-{index}"), &format!("k-{index}"), "high"))
                .await
                .0,
            StatusCode::ACCEPTED
        );
    }
    assert_eq!(w.n("SELECT count(*) AS count FROM work_items").await, 2);
    assert_eq!(
        w.n("SELECT count(*) AS count FROM inbound_events WHERE outcome = 'suppressed_alert_cap'")
            .await,
        1
    );
    w.scratch.drop_self().await;
}

// ---------------------------------------------------------------------------------------------
// Escalation
// ---------------------------------------------------------------------------------------------

#[tokio::test]
async fn a_rise_to_high_or_above_notifies_once_per_rise() {
    let w = world!("queue_escalation");
    exec(
        w.db(),
        "UPDATE alert_rules SET min_severity = 'info', action = 'work_item', bot_user_id = NULL",
        vec![],
    )
    .await;
    let escalated = "SELECT count(*) AS count FROM notifications WHERE type = 'alert_escalated'";
    assert_eq!(w.post(&w.event("e-1", "k", "medium")).await.0, StatusCode::ACCEPTED);
    assert_eq!(w.post(&w.event("e-2", "k", "medium")).await.0, StatusCode::ACCEPTED);
    assert_eq!(w.post(&w.event("e-3", "k", "low")).await.0, StatusCode::ACCEPTED);
    assert_eq!(w.n(escalated).await, 0, "no rise to high");

    assert_eq!(w.post(&w.event("e-4", "k", "high")).await.0, StatusCode::ACCEPTED);
    assert_eq!(w.n(escalated).await, 1);
    assert_eq!(
        w.n(&format!(
            "SELECT count(*) AS count FROM notifications WHERE type = 'alert_escalated' AND user_id = '{}' \
             AND title = 'notification.alertEscalatedTitle' AND content LIKE 'alert_escalated:HIGH:%' \
             AND payload ->> 'severity' = 'high' AND related_issue_id IS NOT NULL AND workspace_id IS NOT NULL",
            w.tenant.member_id
        ))
        .await,
        1
    );
    assert_eq!(w.post(&w.event("e-5", "k", "high")).await.0, StatusCode::ACCEPTED);
    assert_eq!(w.n(escalated).await, 1, "the same severity raises nothing");
    assert_eq!(w.post(&w.event("e-6", "k", "critical")).await.0, StatusCode::ACCEPTED);
    assert_eq!(w.n(escalated).await, 2);
    assert_eq!(
        w.n("SELECT count(*) AS count FROM business_events WHERE event_type = 'alert.escalated'")
            .await,
        2
    );
    assert_eq!(
        w.n("SELECT count(*) AS count FROM notifications WHERE type = 'alert_opened'")
            .await,
        1,
        "the opening is announced once"
    );
    w.scratch.drop_self().await;
}

// ---------------------------------------------------------------------------------------------
// The IP allow-list at ingest
// ---------------------------------------------------------------------------------------------

async fn post_from(w: &World, peer: Option<&str>, forwarded: &[(&str, &str)], event_id: &str) -> (StatusCode, Value) {
    let body = serde_json::to_vec(&w.event(event_id, "k", "info")).unwrap();
    let mut headers = w.signed_headers(&body, Timestamp::now().unix_timestamp());
    for (name, value) in forwarded {
        headers.push((name, (*value).to_owned()));
    }
    let peer: Option<SocketAddr> = peer.map(|text| text.parse().unwrap());
    let (status, _, answer) = w.send_from(enabled(), peer, &headers, &body, INBOUND_EVENTS_PATH).await;
    (status, answer)
}

fn trusted_proxy_precondition() {
    let trusted = &crate::config::runtime().trusted_proxies;
    assert!(
        trusted.iter().any(|block| block.contains("127.0.0.1".parse().unwrap())),
        "these tests assume the default trusted proxies (loopback)"
    );
    assert!(
        !trusted
            .iter()
            .any(|block| block.contains("203.0.113.9".parse().unwrap())),
        "a documentation address must not be a trusted proxy"
    );
}

#[tokio::test]
async fn the_allow_list_admits_listed_addresses_and_refuses_the_rest_before_any_allowance_is_used() {
    trusted_proxy_precondition();
    let w = world!("queue_allowlist");
    exec(
        w.db(),
        "UPDATE inbound_sources SET ip_allowlist = ARRAY['198.51.100.0/24', '192.0.2.77/32', '2001:db8::/32']::cidr[]",
        vec![],
    )
    .await;
    let hits = "SELECT count(*) AS count FROM oauth_rate_hits WHERE bucket = 'inbound_source_minute'";

    // Outside the list: refused, stored nothing, and the source's allowance is untouched.
    for (index, peer) in [
        "203.0.113.9:4000",
        "198.51.99.255:4000",
        "192.0.2.78:4000",
        "[2001:db9::1]:4000",
    ]
    .into_iter()
    .enumerate()
    {
        let (status, answer) = post_from(&w, Some(peer), &[], &format!("out-{index}")).await;
        assert_eq!(status, StatusCode::FORBIDDEN, "{peer}: {answer}");
        assert_eq!(answer["message"], "ip_not_allowed");
    }
    assert_eq!(w.n("SELECT count(*) AS count FROM inbound_events").await, 0);
    assert_eq!(
        w.n(hits).await,
        0,
        "an address outside the list cannot use up the source's quota"
    );
    assert!(
        w.n("SELECT count(*) AS count FROM oauth_rate_hits WHERE bucket = 'inbound_auth_fail'")
            .await
            >= 1,
        "the refusal counts as a failed authentication of that address"
    );
    assert!(
        w.n(
            "SELECT count(*) AS count FROM business_events WHERE event_type = 'inbound_event.rejected' \
             AND payload::text LIKE '%ip_not_allowed%'"
        )
        .await
            >= 1
    );

    // Inside the list, at the edges of the blocks, IPv4 and IPv6.
    for (index, peer) in [
        "198.51.100.0:4000",
        "198.51.100.255:4000",
        "192.0.2.77:4000",
        "[2001:db8:ffff::1]:4000",
        "[::ffff:198.51.100.7]:4000",
    ]
    .into_iter()
    .enumerate()
    {
        let (status, answer) = post_from(&w, Some(peer), &[], &format!("in-{index}")).await;
        assert_eq!(status, StatusCode::ACCEPTED, "{peer}: {answer}");
    }
    assert_eq!(w.n("SELECT count(*) AS count FROM inbound_events").await, 5);

    // A request with no peer information is outside any list; an empty list admits it.
    let (status, _) = post_from(&w, None, &[], "nopeer").await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    exec(w.db(), "UPDATE inbound_sources SET ip_allowlist = '{}'", vec![]).await;
    let (status, _) = post_from(&w, None, &[], "nopeer-open").await;
    assert_eq!(status, StatusCode::ACCEPTED);
    w.scratch.drop_self().await;
}

#[tokio::test]
async fn forwarding_headers_are_believed_only_from_a_trusted_proxy() {
    trusted_proxy_precondition();
    let w = world!("queue_allowlist_spoof");
    exec(
        w.db(),
        "UPDATE inbound_sources SET ip_allowlist = ARRAY['198.51.100.0/24']::cidr[]",
        vec![],
    )
    .await;
    // A client outside the list that is not behind a trusted proxy claims an allowed address.
    for (index, headers) in [
        vec![("x-forwarded-for", "198.51.100.7")],
        vec![("x-real-ip", "198.51.100.7")],
        vec![("x-forwarded-for", "198.51.100.7"), ("x-real-ip", "198.51.100.7")],
        vec![("forwarded", "for=198.51.100.7")],
    ]
    .into_iter()
    .enumerate()
    {
        let (status, answer) = post_from(&w, Some("203.0.113.9:4000"), &headers, &format!("spoof-{index}")).await;
        assert_eq!(status, StatusCode::FORBIDDEN, "{headers:?}: {answer}");
    }
    // Behind a trusted proxy (loopback), the address the proxy saw is used.
    let (status, answer) = post_from(
        &w,
        Some("127.0.0.1:4000"),
        &[("x-forwarded-for", "198.51.100.7")],
        "proxied",
    )
    .await;
    assert_eq!(status, StatusCode::ACCEPTED, "{answer}");
    // The client forged the first hop: the proxy appended the real one, which is outside the list.
    let (status, _) = post_from(
        &w,
        Some("127.0.0.1:4000"),
        &[("x-forwarded-for", "198.51.100.7, 203.0.113.9")],
        "forged-first-hop",
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    // An unparseable header is not guessed at: the proxy's own address is not on the list either.
    let (status, _) = post_from(
        &w,
        Some("127.0.0.1:4000"),
        &[("x-forwarded-for", "not-an-address")],
        "garbled",
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert_eq!(w.n("SELECT count(*) AS count FROM inbound_events").await, 1);
    w.scratch.drop_self().await;
}

// ---------------------------------------------------------------------------------------------
// The schema
// ---------------------------------------------------------------------------------------------

#[tokio::test]
async fn the_format_check_admits_only_the_native_format_and_no_adapter_column_remains() {
    let w = world!("queue_schema");
    let insert = |format: &str| {
        format!(
            "INSERT INTO inbound_sources (workspace_id, project_id, name, format, token_hash, token_prefix, \
             require_signature) VALUES ('{}', '{}', 'x', '{format}', '{}', 'svi_00000000', false)",
            w.tenant.workspace_id,
            w.tenant.project_id,
            Uuid::new_v4().simple().to_string().repeat(2)
        )
    };
    for adapter in ["prx_waf_v1", "prx_sd_generic_v1", "native_v2", ""] {
        assert!(
            w.db().execute_unprepared(&insert(adapter)).await.is_err(),
            "format {adapter:?} must be refused by the database"
        );
    }
    assert!(w.db().execute_unprepared(&insert("native_v1")).await.is_ok());
    assert_eq!(
        w.n(
            "SELECT count(*) AS count FROM information_schema.columns WHERE table_name = 'inbound_sources' \
             AND column_name = 'format_settings'"
        )
        .await,
        0
    );
    // The allow-list holds at most 64 blocks.
    let many = (0..65)
        .map(|index| format!("'192.0.2.{index}/32'"))
        .collect::<Vec<_>>()
        .join(", ");
    assert!(
        w.db()
            .execute_unprepared(&format!(
                "UPDATE inbound_sources SET ip_allowlist = ARRAY[{many}]::cidr[]"
            ))
            .await
            .is_err()
    );
    // The queue columns exist and a waiting task needs its timestamp.
    assert!(
        w.db()
            .execute_unprepared("UPDATE inbound_alerts SET task_state = 'waiting', task_queued_at = NULL")
            .await
            .is_ok(),
        "no alert rows yet, so the statement changes nothing"
    );
    w.scratch.drop_self().await;
}

#[tokio::test]
async fn a_waiting_alert_without_a_queue_timestamp_is_refused_by_the_database() {
    let w = world!("queue_schema_waiting");
    assert_eq!(w.post(&w.event("e-1", "k", "high")).await.0, StatusCode::ACCEPTED);
    assert!(
        w.db()
            .execute_unprepared("UPDATE inbound_alerts SET task_state = 'waiting', task_queued_at = NULL")
            .await
            .is_err()
    );
    assert!(
        w.db()
            .execute_unprepared("UPDATE inbound_alerts SET task_state = 'waiting', task_queued_at = now()")
            .await
            .is_ok()
    );
    w.scratch.drop_self().await;
}
