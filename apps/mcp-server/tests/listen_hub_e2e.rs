#![allow(clippy::indexing_slicing)]
//! The listen hubs against a stand-in API that serves `/api/v1/bot/change-hints`: acknowledgment
//! order, the honoured subset, `tools/list_changed` delivery and coalescing, revocation, the
//! negative cache, reconnect, hub isolation per credential, slot ownership and the caps.

use axum::{
    Json, Router,
    body::Body,
    extract::State,
    http::{HeaderMap, StatusCode, header},
    response::{IntoResponse, Response},
    routing::get,
};
use mcp_server::client::{ClientConfig, OpenPrClient, TRANSPORT_LABEL_HTTP};
use mcp_server::listen::{ListenAuth, ListenConfig, ListenHandle, ListenRegistry, OpenError, caller_key};
use parking_lot::Mutex;
use platform::config::Secret;
use serde_json::{Value, json};
use std::collections::HashMap;
use std::convert::Infallible;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::mpsc;
use tokio_stream::{StreamExt, wrappers::UnboundedReceiverStream};

const TOOLS: &str = "event: tools\ndata: {}\n\n";
const REVOKED: &str = "event: revoked\ndata: {}\n\n";
const EXPIRED: &str = "event: revoked\ndata: {\"reason\":\"expired\"}\n\n";

#[derive(Clone, Copy, PartialEq, Eq)]
enum Rule {
    Bot,
    Unauthorized,
    NotABot,
    Unavailable,
}

#[derive(Default)]
struct Stand {
    rules: Mutex<HashMap<String, Rule>>,
    /// The bearer token of every upstream request, in order.
    seen: Mutex<Vec<String>>,
    feeds: Mutex<HashMap<String, Vec<mpsc::UnboundedSender<String>>>>,
}

impl Stand {
    fn rule(&self, token: &str, rule: Rule) {
        self.rules.lock().insert(token.to_string(), rule);
    }

    fn requests(&self, token: &str) -> usize {
        self.seen.lock().iter().filter(|seen| *seen == token).count()
    }

    /// Sends raw SSE text on the newest stream of `token`.
    fn send(&self, token: &str, text: &str) {
        if let Some(feed) = self.feeds.lock().get(token).and_then(|feeds| feeds.last()) {
            let _ = feed.send(text.to_string());
        }
    }

    /// Closes every stream of `token` from the API side.
    fn end(&self, token: &str) {
        self.feeds.lock().remove(token);
    }

    /// Whether the newest upstream stream of `token` is still held open by the MCP server.
    fn upstream_open(&self, token: &str) -> bool {
        self.feeds
            .lock()
            .get(token)
            .and_then(|feeds| feeds.last())
            .is_some_and(|feed| !feed.is_closed())
    }
}

async fn hints(State(stand): State<Arc<Stand>>, headers: HeaderMap) -> Response {
    let token = headers
        .get(header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("Bearer "))
        .unwrap_or_default()
        .to_string();
    stand.seen.lock().push(token.clone());
    let rule = stand.rules.lock().get(&token).copied().unwrap_or(Rule::Unauthorized);
    match rule {
        Rule::Unauthorized => Json(json!({"code": 401, "message": "no"})).into_response(),
        Rule::NotABot => {
            Json(json!({"code": 403, "message": "change hints are available to bot tokens"})).into_response()
        }
        Rule::Unavailable => StatusCode::SERVICE_UNAVAILABLE.into_response(),
        Rule::Bot => {
            let (tx, rx) = mpsc::unbounded_channel::<String>();
            let _ = tx.send("event: hello\ndata: {\"bot_id\":\"b\",\"workspace_id\":\"w\"}\n\n".to_string());
            stand.feeds.lock().entry(token).or_default().push(tx);
            let body = Body::from_stream(UnboundedReceiverStream::new(rx).map(Ok::<_, Infallible>));
            let mut response = Response::new(body);
            response.headers_mut().insert(
                header::CONTENT_TYPE,
                "text/event-stream"
                    .parse()
                    .unwrap_or_else(|_| header::HeaderValue::from_static("text/plain")),
            );
            response
        }
    }
}

async fn start() -> Result<(Arc<Stand>, String), Box<dyn std::error::Error>> {
    let stand = Arc::new(Stand::default());
    let router = Router::new()
        .route("/api/v1/bot/change-hints", get(hints))
        .with_state(Arc::clone(&stand));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let base = format!("http://{}", listener.local_addr()?);
    tokio::spawn(async move {
        let _ = axum::serve(listener, router).await;
    });
    Ok((stand, base))
}

fn auth(base: &str, token: &str) -> Result<ListenAuth, Box<dyn std::error::Error>> {
    let client = OpenPrClient::new(ClientConfig {
        base_url: base.to_string(),
        credential: None,
        workspace_id: None,
        transport_label: TRANSPORT_LABEL_HTTP,
    })?
    .acting_as(Secret::new(token));
    Ok(ListenAuth::Token {
        key: caller_key(token),
        client,
    })
}

fn registry() -> ListenRegistry {
    ListenRegistry::new(ListenConfig::default())
}

fn tools_params() -> Value {
    json!({"notifications": {"toolsListChanged": true}})
}

async fn open(
    registry: &ListenRegistry,
    base: &str,
    token: &str,
    id: i64,
) -> Result<ListenHandle, Box<dyn std::error::Error>> {
    let params = tools_params();
    registry
        .open(auth(base, token)?, &json!(id), Some(&params))
        .await
        .map_err(|error| format!("open failed: {error:?}").into())
}

async fn next(handle: &mut ListenHandle) -> Result<Value, Box<dyn std::error::Error>> {
    tokio::time::timeout(Duration::from_secs(5), handle.next_message())
        .await?
        .ok_or_else(|| "the stream ended".into())
}

async fn silent(handle: &mut ListenHandle) -> bool {
    tokio::time::timeout(Duration::from_millis(250), handle.next_message())
        .await
        .is_err()
}

fn is_list_changed(message: &Value, id: i64) -> bool {
    message["method"] == "notifications/tools/list_changed"
        && message["params"]["_meta"]["io.modelcontextprotocol/subscriptionId"] == id
}

fn is_final(message: &Value, id: i64) -> bool {
    message["id"] == id && message["result"]["resultType"] == "complete"
}

type TestResult = Result<(), Box<dyn std::error::Error>>;

#[tokio::test]
async fn the_acknowledgment_comes_first_and_a_tools_hint_is_one_list_changed() -> TestResult {
    let (stand, base) = start().await?;
    stand.rule("opr_a", Rule::Bot);
    let registry = registry();
    let mut stream = open(&registry, &base, "opr_a", 7).await?;
    let ack = next(&mut stream).await?;
    assert_eq!(ack["method"], "notifications/subscriptions/acknowledged");
    assert_eq!(ack["params"]["_meta"]["io.modelcontextprotocol/subscriptionId"], 7);
    assert_eq!(ack["params"]["notifications"], json!({"toolsListChanged": true}));
    assert!(silent(&mut stream).await, "nothing is sent before a hint");

    stand.send("opr_a", TOOLS);
    let notification = next(&mut stream).await?;
    assert!(is_list_changed(&notification, 7), "{notification}");
    assert!(silent(&mut stream).await);
    Ok(())
}

#[tokio::test]
async fn hints_that_are_not_about_tools_are_not_forwarded() -> TestResult {
    let (stand, base) = start().await?;
    stand.rule("opr_a", Rule::Bot);
    let registry = registry();
    let mut stream = open(&registry, &base, "opr_a", 1).await?;
    next(&mut stream).await?;
    for text in [
        "event: task\ndata: {\"id\":\"x\"}\n\n",
        "event: inbox\ndata: {}\n\n",
        "event: issue\ndata: {\"id\":\"x\"}\n\n",
        "event: surprise\ndata: {}\n\n",
        ": comment\n\n",
    ] {
        stand.send("opr_a", text);
    }
    assert!(silent(&mut stream).await);
    Ok(())
}

#[tokio::test]
async fn a_burst_of_hints_with_the_consumer_paused_is_one_pending_notification() -> TestResult {
    let (stand, base) = start().await?;
    stand.rule("opr_a", Rule::Bot);
    let registry = registry();
    let mut stream = open(&registry, &base, "opr_a", 3).await?;
    next(&mut stream).await?;
    for _ in 0..10 {
        stand.send("opr_a", TOOLS);
    }
    tokio::time::sleep(Duration::from_millis(300)).await;
    let first = next(&mut stream).await?;
    assert!(is_list_changed(&first, 3), "{first}");
    assert!(silent(&mut stream).await, "the burst must coalesce to one notification");
    Ok(())
}

#[tokio::test]
async fn a_user_token_gets_the_empty_subset_then_the_final_result_and_no_hub() -> TestResult {
    let (stand, base) = start().await?;
    stand.rule("user_token", Rule::NotABot);
    let registry = registry();
    let mut stream = open(&registry, &base, "user_token", 4).await?;
    let ack = next(&mut stream).await?;
    assert_eq!(ack["params"]["notifications"], json!({}));
    let end = next(&mut stream).await?;
    assert!(is_final(&end, 4), "{end}");
    drop(stream);
    assert_eq!(registry.hub_count(), 0);
    assert_eq!(registry.open_streams(), 0);
    // The verdict is remembered for the refusal window: no second upstream request.
    let mut again = open(&registry, &base, "user_token", 5).await?;
    next(&mut again).await?;
    assert!(is_final(&next(&mut again).await?, 5));
    assert_eq!(stand.requests("user_token"), 1);
    Ok(())
}

#[tokio::test]
async fn a_delegated_caller_gets_the_empty_subset_and_the_final_result_without_the_api() -> TestResult {
    let (stand, _base) = start().await?;
    let registry = registry();
    let params = tools_params();
    let mut stream = registry
        .open(
            ListenAuth::Delegated {
                key: caller_key("sva_token"),
            },
            &json!(8),
            Some(&params),
        )
        .await
        .map_err(|error| format!("{error:?}"))?;
    let ack = next(&mut stream).await?;
    assert_eq!(ack["params"]["notifications"], json!({}));
    assert!(is_final(&next(&mut stream).await?, 8));
    assert!(stand.seen.lock().is_empty());
    assert_eq!(registry.hub_count(), 0);
    Ok(())
}

#[tokio::test]
async fn a_credential_the_api_refuses_is_unauthorized_and_remembered() -> TestResult {
    let (stand, base) = start().await?;
    let registry = registry();
    let params = tools_params();
    for round in 0..3 {
        let result = registry
            .open(auth(&base, "opr_bad")?, &json!(round), Some(&params))
            .await;
        assert!(matches!(result, Err(OpenError::Unauthorized)), "round {round}");
    }
    assert_eq!(stand.requests("opr_bad"), 1, "the refusal is cached");
    assert_eq!(registry.open_streams(), 0);
    assert_eq!(registry.hub_count(), 0);
    Ok(())
}

#[tokio::test]
async fn an_unavailable_api_is_not_a_verdict_and_holds_nothing() -> TestResult {
    let (stand, base) = start().await?;
    stand.rule("opr_a", Rule::Unavailable);
    let registry = registry();
    let params = tools_params();
    for round in 0..2 {
        let result = registry.open(auth(&base, "opr_a")?, &json!(round), Some(&params)).await;
        assert!(matches!(result, Err(OpenError::Unavailable)), "round {round}");
    }
    assert_eq!(stand.requests("opr_a"), 2, "an outage is asked again");
    assert_eq!(registry.open_streams(), 0);
    Ok(())
}

#[tokio::test]
async fn upstream_revocation_ends_every_stream_of_that_credential_only() -> TestResult {
    let (stand, base) = start().await?;
    stand.rule("opr_a", Rule::Bot);
    stand.rule("opr_b", Rule::Bot);
    let registry = registry();
    let mut a1 = open(&registry, &base, "opr_a", 1).await?;
    let mut a2 = open(&registry, &base, "opr_a", 2).await?;
    let mut b = open(&registry, &base, "opr_b", 3).await?;
    for stream in [&mut a1, &mut a2, &mut b] {
        next(stream).await?;
    }
    assert_eq!(
        stand.requests("opr_a"),
        1,
        "two streams of one credential share one upstream"
    );
    assert_eq!(registry.hub_count(), 2);

    stand.rule("opr_a", Rule::Unauthorized);
    stand.send("opr_a", REVOKED);
    let started = std::time::Instant::now();
    assert!(is_final(&next(&mut a1).await?, 1));
    assert!(is_final(&next(&mut a2).await?, 2));
    assert!(
        started.elapsed() < Duration::from_secs(1),
        "revocation must end streams at once"
    );
    assert!(silent(&mut b).await, "another credential is untouched");
    stand.send("opr_b", TOOLS);
    assert!(is_list_changed(&next(&mut b).await?, 3));

    // New listens with the revoked credential are refused at once, without asking the API.
    let params = tools_params();
    let refused = registry.open(auth(&base, "opr_a")?, &json!(9), Some(&params)).await;
    assert!(matches!(refused, Err(OpenError::Unauthorized)));
    assert_eq!(stand.requests("opr_a"), 1);
    Ok(())
}

#[tokio::test]
async fn an_expired_credential_ends_the_stream_and_the_refusal_lapses_after_ten_seconds() -> TestResult {
    let (stand, base) = start().await?;
    stand.rule("opr_a", Rule::Bot);
    let registry = registry();
    let mut stream = open(&registry, &base, "opr_a", 1).await?;
    next(&mut stream).await?;
    stand.rule("opr_a", Rule::Unauthorized);
    stand.send("opr_a", EXPIRED);
    assert!(is_final(&next(&mut stream).await?, 1));
    drop(stream);
    assert_eq!(registry.hub_count(), 0);

    let params = tools_params();
    assert!(matches!(
        registry.open(auth(&base, "opr_a")?, &json!(2), Some(&params)).await,
        Err(OpenError::Unauthorized)
    ));
    assert_eq!(stand.requests("opr_a"), 1);

    // After the refusal window the API is asked again (here it accepts the credential again).
    stand.rule("opr_a", Rule::Bot);
    tokio::time::sleep(mcp_server::listen::REFUSAL_CACHE + Duration::from_millis(500)).await;
    let mut again = open(&registry, &base, "opr_a", 3).await?;
    let ack = next(&mut again).await?;
    assert_eq!(ack["params"]["notifications"], json!({"toolsListChanged": true}));
    assert_eq!(stand.requests("opr_a"), 2);
    Ok(())
}

#[tokio::test]
async fn an_upstream_drop_reconnects_and_tells_the_streams_the_list_may_have_changed() -> TestResult {
    let (stand, base) = start().await?;
    stand.rule("opr_a", Rule::Bot);
    let registry = registry();
    let mut stream = open(&registry, &base, "opr_a", 5).await?;
    next(&mut stream).await?;

    stand.end("opr_a");
    let notification = next(&mut stream).await?;
    assert!(is_list_changed(&notification, 5), "{notification}");
    assert_eq!(stand.requests("opr_a"), 2, "the hub reconnected");
    // Hints flow on the new upstream.
    stand.send("opr_a", TOOLS);
    assert!(is_list_changed(&next(&mut stream).await?, 5));
    Ok(())
}

#[tokio::test]
async fn a_credential_the_api_refuses_on_reconnect_ends_its_streams() -> TestResult {
    let (stand, base) = start().await?;
    stand.rule("opr_a", Rule::Bot);
    let registry = registry();
    let mut stream = open(&registry, &base, "opr_a", 6).await?;
    next(&mut stream).await?;
    stand.rule("opr_a", Rule::Unauthorized);
    stand.end("opr_a");
    assert!(is_final(&next(&mut stream).await?, 6));
    Ok(())
}

#[tokio::test]
async fn two_credentials_never_share_a_hub_and_each_upstream_carries_its_own_token() -> TestResult {
    let (stand, base) = start().await?;
    stand.rule("opr_a", Rule::Bot);
    stand.rule("opr_b", Rule::Bot);
    let registry = registry();
    let mut a = open(&registry, &base, "opr_a", 1).await?;
    let mut b = open(&registry, &base, "opr_b", 1).await?;
    next(&mut a).await?;
    next(&mut b).await?;
    assert_eq!(registry.hub_count(), 2);
    let mut seen = stand.seen.lock().clone();
    seen.sort();
    assert_eq!(seen, vec!["opr_a".to_string(), "opr_b".to_string()]);

    stand.send("opr_a", TOOLS);
    assert!(is_list_changed(&next(&mut a).await?, 1));
    assert!(silent(&mut b).await, "a hint for one credential never reaches another");
    Ok(())
}

#[tokio::test]
async fn concurrent_first_listens_of_one_credential_share_one_upstream() -> TestResult {
    let (stand, base) = start().await?;
    stand.rule("opr_a", Rule::Bot);
    let registry = registry();
    let mut opens = Vec::new();
    for id in 0..20 {
        let registry = registry.clone();
        let auth = auth(&base, "opr_a")?;
        opens.push(tokio::spawn(async move {
            let params = tools_params();
            registry.open(auth, &json!(id), Some(&params)).await
        }));
    }
    let mut handles = Vec::new();
    for open in opens {
        handles.push(open.await?.map_err(|error| format!("{error:?}"))?);
    }
    assert_eq!(stand.requests("opr_a"), 1);
    assert_eq!(registry.hub_count(), 1);
    assert_eq!(registry.open_streams(), 20);
    drop(handles);
    Ok(())
}

#[tokio::test]
async fn the_last_stream_out_drops_the_hub_and_closes_the_upstream() -> TestResult {
    let (stand, base) = start().await?;
    stand.rule("opr_a", Rule::Bot);
    let registry = registry();
    let first = open(&registry, &base, "opr_a", 1).await?;
    let second = open(&registry, &base, "opr_a", 2).await?;
    assert!(stand.upstream_open("opr_a"));
    drop(first);
    assert_eq!(registry.hub_count(), 1);
    assert!(stand.upstream_open("opr_a"));
    drop(second);
    assert_eq!(registry.hub_count(), 0);
    let mut closed = false;
    for _ in 0..40 {
        if !stand.upstream_open("opr_a") {
            closed = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert!(closed, "the upstream stream must be closed once no stream is left");
    Ok(())
}

#[tokio::test]
async fn a_consumer_that_never_reads_does_not_hold_up_revocation() -> TestResult {
    let (stand, base) = start().await?;
    stand.rule("opr_a", Rule::Bot);
    let registry = registry();
    let mut slow = open(&registry, &base, "opr_a", 1).await?;
    let mut live = open(&registry, &base, "opr_a", 2).await?;
    next(&mut live).await?;
    // The slow consumer reads nothing while thousands of hints and then a revocation arrive.
    for _ in 0..5_000 {
        stand.send("opr_a", TOOLS);
    }
    stand.rule("opr_a", Rule::Unauthorized);
    stand.send("opr_a", REVOKED);
    // The live consumer is told at once although the other one has not read a byte.
    // (Hints still being fanned out may produce more list_changed first; coalescing bounds them.)
    let started = std::time::Instant::now();
    let mut ended = false;
    while started.elapsed() < Duration::from_secs(5) {
        if is_final(&next(&mut live).await?, 2) {
            ended = true;
            break;
        }
    }
    assert!(ended);
    let ack = next(&mut slow).await?;
    assert_eq!(ack["method"], "notifications/subscriptions/acknowledged");
    assert!(
        is_final(&next(&mut slow).await?, 1),
        "the end wins over the pending notification"
    );
    Ok(())
}

#[tokio::test]
async fn a_stalled_consumer_loses_its_stream_and_its_slot() -> TestResult {
    let (stand, base) = start().await?;
    stand.rule("opr_a", Rule::Bot);
    let registry = registry();
    let stream = open(&registry, &base, "opr_a", 1).await?;
    let (tx, _rx) = mpsc::channel::<String>(1);
    let forward = tokio::spawn(async move {
        stream.forward(&tx, Duration::from_millis(200), |message| message).await;
    });
    // The acknowledgment fills the one-place queue; the next message waits and times out.
    stand.send("opr_a", TOOLS);
    tokio::time::timeout(Duration::from_secs(5), forward).await??;
    assert_eq!(registry.open_streams(), 0);
    assert_eq!(registry.hub_count(), 0, "the stalled stream no longer pins its hub");
    Ok(())
}

#[tokio::test]
async fn the_per_credential_cap_counts_held_streams_and_the_slot_returns_on_drop() -> TestResult {
    let (stand, base) = start().await?;
    stand.rule("opr_a", Rule::Bot);
    stand.rule("opr_b", Rule::Bot);
    let registry = ListenRegistry::new(ListenConfig {
        per_key_limit: 3,
        ..ListenConfig::default()
    });
    let mut held = Vec::new();
    for id in 0..3 {
        held.push(open(&registry, &base, "opr_a", id).await?);
    }
    let params = tools_params();
    let refused = registry.open(auth(&base, "opr_a")?, &json!(9), Some(&params)).await;
    assert!(matches!(refused, Err(OpenError::TooMany(3))));
    // Another credential has its own allowance.
    let _other = open(&registry, &base, "opr_b", 1).await?;
    held.pop();
    let _again = open(&registry, &base, "opr_a", 10).await?;
    Ok(())
}

#[tokio::test]
async fn a_process_stop_ends_held_streams_with_the_final_result() -> TestResult {
    let (stand, base) = start().await?;
    stand.rule("opr_a", Rule::Bot);
    let registry = registry();
    let mut stream = open(&registry, &base, "opr_a", 1).await?;
    next(&mut stream).await?;
    registry.shutdown();
    assert!(is_final(&next(&mut stream).await?, 1));
    drop(stream);
    tokio::time::timeout(Duration::from_secs(2), registry.wait_idle()).await?;
    Ok(())
}

#[tokio::test]
async fn a_cancelled_stream_ends_without_a_final_result_and_frees_its_slot() -> TestResult {
    let (stand, base) = start().await?;
    stand.rule("opr_a", Rule::Bot);
    let registry = registry();
    let mut stream = open(&registry, &base, "opr_a", 1).await?;
    next(&mut stream).await?;
    stream.cancel_token().cancel();
    assert!(stream.next_message().await.is_none());
    drop(stream);
    assert_eq!(registry.open_streams(), 0);
    assert_eq!(registry.hub_count(), 0);
    Ok(())
}
