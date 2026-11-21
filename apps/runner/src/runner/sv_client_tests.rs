#![allow(clippy::indexing_slicing, clippy::type_complexity, clippy::needless_pass_by_value)]

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use axum::Router;
use axum::http::{HeaderMap, StatusCode};
use axum::response::Redirect;
use axum::routing::{get, post};
use parking_lot::Mutex;
use serde_json::{Value, json};

use super::sv_client::{SvClient, SvError, token_refused};

const TOKEN: &str = concat!("opr", "_", "efefefefefefefefefefefefefefefefefef");
const WAIT: Duration = Duration::from_secs(5);

async fn bind() -> (tokio::net::TcpListener, SocketAddr) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let addr = listener.local_addr().expect("addr");
    (listener, addr)
}

fn client(addr: SocketAddr, surface: &str) -> SvClient {
    let http = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .expect("client");
    SvClient::new(http, &format!("http://{addr}/"), TOKEN, surface)
}

#[test]
fn debug_and_errors_never_show_the_token() {
    let c = SvClient::new(reqwest::Client::new(), "http://127.0.0.1:9", TOKEN, "mcp_stdio");
    let shown = format!("{c:?}");
    assert!(!shown.contains(TOKEN) && !shown.contains("opr_"), "{shown}");
    assert!(shown.contains("<redacted>"));
    for error in [
        SvError::Timeout,
        SvError::Transport,
        SvError::Status(401),
        SvError::Status(503),
    ] {
        let text = error.to_string();
        assert!(!text.contains("opr_") && !text.contains("http"), "{text}");
    }
    assert!(
        SvError::Timeout.is_transient() && SvError::Transport.is_transient() && SvError::Status(502).is_transient()
    );
    assert!(!SvError::Status(400).is_transient() && !SvError::Status(404).is_transient());
    assert!(token_refused(&SvError::Status(401)) && token_refused(&SvError::Status(403)));
    assert!(!token_refused(&SvError::Status(404)) && !token_refused(&SvError::Timeout));
}

#[tokio::test]
async fn requests_carry_the_bearer_token_and_the_surface_header_unless_rest() {
    let seen: Arc<Mutex<Vec<(String, Option<String>, Option<String>, Value)>>> = Arc::new(Mutex::new(Vec::new()));
    let record = {
        let seen = Arc::clone(&seen);
        move |uri: axum::http::Uri, headers: HeaderMap, body: axum::body::Bytes| {
            let seen = Arc::clone(&seen);
            async move {
                seen.lock().push((
                    uri.to_string(),
                    headers
                        .get("authorization")
                        .and_then(|v| v.to_str().ok())
                        .map(str::to_string),
                    headers
                        .get("x-sylvode-mcp-surface")
                        .and_then(|v| v.to_str().ok())
                        .map(str::to_string),
                    serde_json::from_slice(&body).unwrap_or(Value::Null),
                ));
                json!({"code": 0, "data": {"ok": true}}).to_string()
            }
        }
    };
    let (listener, addr) = bind().await;
    let app = Router::new().fallback(get(record.clone()).post(record.clone()).put(record));
    tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });
    let stdio = client(addr, "mcp_stdio");
    stdio.task_view("a b/../c", 3, WAIT).await.expect("view");
    stdio
        .progress("t1", 2, "runner.alive", "still here", Some(1), WAIT)
        .await
        .expect("progress");
    stdio.fail("t1", 2, "why", false, WAIT).await.expect("fail");
    stdio
        .put_report(&json!({"components": []}), WAIT)
        .await
        .expect("report");
    stdio.protocol(WAIT).await.expect("protocol");
    stdio.active_tasks(WAIT).await.expect("active");
    client(addr, "rest").protocol(WAIT).await.expect("protocol");
    let seen = seen.lock().clone();
    // A segment from a delivery is percent-encoded, never a path of its own.
    assert_eq!(seen[0].0, "/api/v1/ai/tasks/a%20b%2F..%2Fc?attempt=3");
    assert_eq!(seen[1].0, "/api/v1/ai/tasks/t1/progress?attempt=2");
    assert_eq!(
        seen[1].3,
        json!({"kind": "runner.alive", "note": "still here", "queue_position": 1})
    );
    assert_eq!(seen[2].0, "/api/v1/ai/tasks/t1/fail?attempt=2");
    assert_eq!(
        seen[2].3,
        json!({"reason": "why", "retryable": false, "source": "runner"})
    );
    assert_eq!(seen[3].0, "/api/v1/ai/runner/report");
    assert_eq!(seen[4].0, "/api/v1/ai/protocol");
    assert_eq!(seen[5].0, "/api/v1/ai/tasks/active");
    for request in &seen[..6] {
        assert_eq!(request.1.as_deref(), Some(format!("Bearer {TOKEN}").as_str()));
        assert_eq!(request.2.as_deref(), Some("mcp_stdio"));
    }
    assert_eq!(seen[6].1.as_deref(), Some(format!("Bearer {TOKEN}").as_str()));
    assert_eq!(seen[6].2, None, "the rest transport sends no header");
}

#[tokio::test]
async fn a_redirect_is_not_followed_so_the_token_never_leaves_the_configured_host() {
    let (other_listener, other) = bind().await;
    let reached = Arc::new(Mutex::new(Vec::<Option<String>>::new()));
    let reached_handler = Arc::clone(&reached);
    let other_app = Router::new().fallback(move |headers: HeaderMap| {
        let reached = Arc::clone(&reached_handler);
        async move {
            reached.lock().push(
                headers
                    .get("authorization")
                    .and_then(|v| v.to_str().ok())
                    .map(str::to_string),
            );
            "{}"
        }
    });
    tokio::spawn(async move {
        let _ = axum::serve(other_listener, other_app).await;
    });
    let (listener, addr) = bind().await;
    let target = format!("http://{other}/steal");
    let app = Router::new().fallback(move || {
        let target = target.clone();
        async move { Redirect::temporary(&target) }
    });
    tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });
    let error = client(addr, "mcp_stdio").protocol(WAIT).await.expect_err("redirect");
    assert_eq!(error, SvError::Status(307));
    assert!(reached.lock().is_empty(), "the token was sent to {other}");
    // The runner's shared client has the same rule.
    let config =
        crate::config::Config::from_toml("[server]\nlisten = \"127.0.0.1:1\"\n[security]\nallow_unsigned = true\n")
            .expect("config");
    let runtime = super::RunnerRuntime::new(&config);
    let shared = SvClient::new(runtime.http, &format!("http://{addr}"), TOKEN, "rest");
    assert_eq!(shared.protocol(WAIT).await.expect_err("redirect"), SvError::Status(307));
    assert!(reached.lock().is_empty());
}

#[tokio::test]
async fn statuses_timeouts_and_unreadable_answers_are_told_apart() {
    let (listener, addr) = bind().await;
    let app = Router::new()
        .route(
            "/api/v1/ai/protocol",
            get(|| async { (StatusCode::UNAUTHORIZED, "{}") }),
        )
        .route(
            "/api/v1/ai/tasks/slow",
            get(|| async {
                tokio::time::sleep(Duration::from_secs(3)).await;
                "{}"
            }),
        )
        .route("/api/v1/ai/tasks/junk", get(|| async { "not json" }))
        .route("/api/v1/ai/tasks/nodata", get(|| async { "{\"code\":0}" }))
        .route("/api/v1/ai/tasks/big", get(|| async { "x".repeat(2 * 1024 * 1024) }))
        .route("/api/v1/ai/runner/report", post(|| async { StatusCode::NOT_FOUND }));
    tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });
    let c = client(addr, "mcp_stdio");
    assert_eq!(c.protocol(WAIT).await, Err(SvError::Status(401)));
    assert_eq!(
        c.task_view("slow", 1, Duration::from_millis(300)).await,
        Err(SvError::Timeout)
    );
    assert_eq!(c.task_view("junk", 1, WAIT).await, Err(SvError::Transport));
    assert_eq!(c.task_view("nodata", 1, WAIT).await, Err(SvError::Transport));
    assert_eq!(c.task_view("big", 1, WAIT).await, Err(SvError::Transport));
    // PUT to a POST-only route is a 405: a status, not a transport failure.
    assert_eq!(c.put_report(&json!({}), WAIT).await, Err(SvError::Status(405)));
    let dead = {
        let (listener, addr) = bind().await;
        drop(listener);
        client(addr, "rest")
    };
    assert_eq!(dead.protocol(WAIT).await, Err(SvError::Transport));
}
