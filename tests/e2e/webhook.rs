//! The `/webhook` endpoint through the real executable.

use crate::support::{CANONICAL, SECRET, Server, bot_task, post_signed, rfc3339};

fn config() -> String {
    format!("[server]\nlisten = \"127.0.0.1:0\"\n\n[security]\nwebhook_secrets = [\"{SECRET}\"]\n")
}

/// A captured signed delivery cannot be sent again: the second copy is refused with 409.
#[tokio::test(flavor = "multi_thread")]
async fn a_replayed_delivery_is_refused() {
    let server = Server::start(CANONICAL, &config(), &[]).expect("server starts");
    let body = bot_task("Nobody", "replay me").to_string();
    let (first, answer) = post_signed(&server.url("/webhook"), &body).await;
    assert_eq!(first, 200, "{answer}");
    assert_eq!(answer["status"], "no_agent");
    let (second, answer) = post_signed(&server.url("/webhook"), &body).await;
    assert_eq!(second, 409, "{answer}");
}

/// A signed delivery whose timestamp is older than `max_delivery_age_secs` is refused with 401.
#[tokio::test(flavor = "multi_thread")]
async fn a_stale_delivery_is_refused() {
    let server = Server::start(CANONICAL, &config(), &[]).expect("server starts");
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("clock after 1970")
        .as_secs();
    let mut body = bot_task("Nobody", "stale");
    body["timestamp"] = serde_json::Value::String(rfc3339(now - 3600));
    let (status, answer) = post_signed(&server.url("/webhook"), &body.to_string()).await;
    assert_eq!(status, 401, "{answer}");

    body["timestamp"] = serde_json::Value::String(rfc3339(now - 10));
    let (status, answer) = post_signed(&server.url("/webhook"), &body.to_string()).await;
    assert_eq!(status, 200, "{answer}");
}
