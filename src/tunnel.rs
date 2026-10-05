use crate::{background::Background, config::Config, dispatcher, signature};
use futures_util::{SinkExt, StreamExt};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};
use tokio::sync::mpsc;
use tokio::time::{Duration, sleep};
use tokio_tungstenite::{connect_async, tungstenite::Message, tungstenite::http::Request};
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Envelope {
    pub id: String,
    #[serde(rename = "type")]
    pub msg_type: String,
    pub ts: u64,
    pub agent_id: String,
    pub payload: Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sig: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
struct DispatchPayload {
    #[serde(default)]
    run_id: Option<String>,
    #[serde(default)]
    issue_id: Option<String>,
    #[serde(default)]
    agent: Option<String>,
    #[serde(default)]
    body: Option<Value>,
}

/// The bytes a tunnel signature covers: the envelope serialized without `sig`.
fn unsigned_envelope_bytes(envelope: &Envelope) -> Option<Vec<u8>> {
    let mut unsigned = envelope.clone();
    unsigned.sig = None;
    serde_json::to_vec(&unsigned)
        .map_err(|e| tracing::warn!("serialize envelope for signing failed: {e}"))
        .ok()
}

pub fn sign_envelope_body(envelope: &Envelope, secret: &str) -> Option<String> {
    unsigned_envelope_bytes(envelope).and_then(|bytes| signature::sign_payload(&bytes, secret).ok())
}

/// Whether the envelope carries a valid `sig` under `secret`, compared in constant time.
///
/// Every leading `sha256=` is removed from `sig` before it is decoded.
pub fn verify_envelope_signature(envelope: &Envelope, secret: &str) -> bool {
    envelope.sig.as_ref().is_some_and(|sig| {
        unsigned_envelope_bytes(envelope).is_some_and(|bytes| {
            signature::verify_digest_hex(&bytes, sig.trim_start_matches("sha256="), std::iter::once(secret))
        })
    })
}

fn now_ts() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs())
}

fn build_envelope(msg_type: &str, agent_id: &str, payload: Value, hmac_secret: Option<&str>) -> Envelope {
    let mut env = Envelope {
        id: Uuid::new_v4().to_string(),
        msg_type: msg_type.to_string(),
        ts: now_ts(),
        agent_id: agent_id.to_string(),
        payload,
        sig: None,
    };

    if let Some(secret) = hmac_secret {
        env.sig = sign_envelope_body(&env, secret).map(|s| format!("sha256={s}"));
    }

    env
}

/// Sleeps for `secs`, or less when shutdown begins. Returns whether shutdown has begun.
async fn pause_unless_shutdown(background: &Background, secs: u64) -> bool {
    tokio::select! {
        () = sleep(Duration::from_secs(secs)) => background.is_shutting_down(),
        () = background.cancelled() => true,
    }
}

pub async fn run_tunnel_loop(config: Arc<Config>, background: Background) {
    if !config.tunnel_enabled() {
        return;
    }

    let Some(tunnel) = config.tunnel.clone() else {
        return;
    };

    let Some(url) = tunnel.url.clone() else {
        tracing::warn!("tunnel enabled but url is missing");
        return;
    };

    // `agent_id` has no default: the control plane matches it against the agent it registered,
    // so it must come from the operator's config.
    let Some(agent_id) = tunnel.agent_id.clone().filter(|id| !id.trim().is_empty()) else {
        tracing::warn!("tunnel enabled but agent_id is missing");
        return;
    };

    if tunnel.auth_token.as_deref().unwrap_or(" ").trim().is_empty() {
        tracing::warn!("tunnel enabled but auth_token is missing");
        return;
    }

    let base_reconnect_secs = tunnel.reconnect_secs.max(1);
    let heartbeat_secs = tunnel.heartbeat_secs.max(3);
    let hmac_secret = tunnel.hmac_secret.clone();
    let require_inbound_sig = tunnel.require_inbound_sig;
    let max_backoff_secs = config
        .runtime
        .tunnel_reconnect_backoff_max_secs
        .max(base_reconnect_secs);
    let mut retry_secs = base_reconnect_secs;

    loop {
        if !(url.starts_with("wss://") || url.starts_with("ws://")) {
            tracing::error!("tunnel url must start with ws:// or wss://: {url}");
            if pause_unless_shutdown(&background, retry_secs).await {
                return;
            }
            retry_secs = (retry_secs.saturating_mul(2)).min(max_backoff_secs);
            continue;
        }

        if url.starts_with("ws://") {
            tracing::warn!("tunnel using insecure ws:// transport; prefer wss:// in production");
        }

        let mut req_builder = Request::builder().uri(&url);
        if let Some(token) = &tunnel.auth_token {
            req_builder = req_builder.header("Authorization", format!("Bearer {token}"));
        }

        let request = match req_builder.body(()) {
            Ok(r) => r,
            Err(e) => {
                tracing::error!("build tunnel request failed: {e}");
                if pause_unless_shutdown(&background, retry_secs).await {
                    return;
                }
                retry_secs = (retry_secs.saturating_mul(2)).min(max_backoff_secs);
                continue;
            }
        };

        let connected = tokio::select! {
            connected = connect_async(request) => connected,
            () = background.cancelled() => return,
        };
        match connected {
            Ok((ws_stream, _)) => {
                retry_secs = base_reconnect_secs;
                tracing::info!("tunnel connected: {url}");
                let (mut writer, mut reader) = ws_stream.split();
                let (tx, mut rx) = mpsc::channel::<Envelope>(128);

                let writer_task = tokio::spawn(async move {
                    while let Some(msg) = rx.recv().await {
                        let text = match serde_json::to_string(&msg) {
                            Ok(t) => t,
                            Err(e) => {
                                tracing::warn!("serialize envelope failed: {e}");
                                continue;
                            }
                        };
                        if writer.send(Message::Text(text.into())).await.is_err() {
                            break;
                        }
                    }
                });

                let hb_tx = tx.clone();
                let hb_agent = agent_id.clone();
                let hb_secret = hmac_secret.clone();
                let heartbeat_task = tokio::spawn(async move {
                    loop {
                        sleep(Duration::from_secs(heartbeat_secs)).await;
                        let hb = build_envelope("heartbeat", &hb_agent, json!({"alive": true}), hb_secret.as_deref());
                        if hb_tx.send(hb).await.is_err() {
                            break;
                        }
                    }
                });

                // Shutdown stops reading new tasks; runs already accepted are cancelled by the
                // same signal and still report their result before the writer closes.
                while let Some(message) = tokio::select! {
                    message = reader.next() => message,
                    () = background.cancelled() => None,
                } {
                    let Ok(message) = message else { break };
                    let Message::Text(text) = message else {
                        continue;
                    };

                    let envelope: Envelope = match serde_json::from_str(&text) {
                        Ok(v) => v,
                        Err(e) => {
                            tracing::warn!("invalid tunnel message: {e}");
                            let _ = tx
                                .send(build_envelope(
                                    "error",
                                    &agent_id,
                                    json!({"reason":"invalid_json"}),
                                    hmac_secret.as_deref(),
                                ))
                                .await;
                            continue;
                        }
                    };

                    if require_inbound_sig && envelope.sig.is_none() {
                        tracing::warn!(
                            "tunnel missing signature while require_inbound_sig=true for msg_id={}",
                            envelope.id
                        );
                        let _ = tx
                            .send(build_envelope(
                                "error",
                                &agent_id,
                                json!({"reason":"missing_signature","msg_id":envelope.id}),
                                hmac_secret.as_deref(),
                            ))
                            .await;
                        continue;
                    }

                    if let Some(secret) = &hmac_secret
                        && !verify_envelope_signature(&envelope, secret)
                    {
                        tracing::warn!("tunnel signature verify failed for msg_id={}", envelope.id);
                        let _ = tx
                            .send(build_envelope(
                                "error",
                                &agent_id,
                                json!({"reason":"bad_signature","msg_id":envelope.id}),
                                hmac_secret.as_deref(),
                            ))
                            .await;
                        continue;
                    }

                    if envelope.msg_type != "task.dispatch" {
                        continue;
                    }

                    let dispatch =
                        serde_json::from_value::<DispatchPayload>(envelope.payload.clone()).unwrap_or_else(|_| {
                            DispatchPayload {
                                run_id: None,
                                issue_id: None,
                                agent: None,
                                body: Some(envelope.payload.clone()),
                            }
                        });

                    let run_id = dispatch.run_id.unwrap_or_else(|| format!("run-{}", Uuid::new_v4()));

                    let issue_id = dispatch
                        .issue_id
                        .or_else(|| dispatch.body.as_ref().and_then(dispatcher::extract_issue_id));

                    let _ = tx
                        .send(build_envelope(
                            "task.ack",
                            &agent_id,
                            json!({
                                "run_id": run_id,
                                "issue_id": issue_id,
                                "status": "accepted"
                            }),
                            hmac_secret.as_deref(),
                        ))
                        .await;

                    let task_tx = tx.clone();
                    let task_cfg = config.clone();
                    let task_agent = agent_id.clone();
                    let task_secret = hmac_secret.clone();
                    let body = dispatch.body.unwrap_or_else(|| envelope.payload.clone());
                    let route_agent = dispatch.agent.clone();
                    let run_id_for_task = run_id.clone();
                    let task_background = background.clone();
                    background.spawn(async move {
                        let Some(permit) = task_background.cli_permit().await else {
                            let _ = task_tx
                                .send(build_envelope(
                                    "task.result",
                                    &task_agent,
                                    json!({
                                        "run_id": run_id_for_task,
                                        "issue_id": issue_id,
                                        "status": "cancelled",
                                        "summary": "cancelled by shutdown before it started"
                                    }),
                                    task_secret.as_deref(),
                                ))
                                .await;
                            return;
                        };

                        let selected = route_agent.map_or_else(
                            || task_cfg.agents.iter().find(|a| a.agent_type == "cli"),
                            |target_id| {
                                task_cfg
                                    .agents
                                    .iter()
                                    .find(|a| a.id == target_id && a.agent_type == "cli")
                            },
                        );

                        let Some(cli_agent) = selected else {
                            let _ = task_tx
                                .send(build_envelope(
                                    "task.result",
                                    &task_agent,
                                    json!({
                                        "run_id": run_id_for_task,
                                        "issue_id": issue_id,
                                        "status": "failed",
                                        "summary": "no cli agent configured"
                                    }),
                                    task_secret.as_deref(),
                                ))
                                .await;
                            drop(permit);
                            return;
                        };

                        let report = dispatcher::execute_cli_task(
                            task_cfg.as_ref(),
                            cli_agent,
                            &body,
                            Some(run_id_for_task),
                            task_background.cancelled(),
                        )
                        .await;
                        drop(permit);

                        let _ = task_tx
                            .send(build_envelope(
                                "task.result",
                                &task_agent,
                                json!({
                                    "run_id": report.run_id,
                                    "issue_id": report.issue_id,
                                    "status": report.status,
                                    "summary": report.summary
                                }),
                                task_secret.as_deref(),
                            ))
                            .await;
                    });
                }

                heartbeat_task.abort();
                drop(tx);
                let _ = writer_task.await;
                if background.is_shutting_down() {
                    tracing::info!("tunnel closed for shutdown");
                    return;
                }
                tracing::warn!("tunnel disconnected, reconnecting in {retry_secs}s");
            }
            Err(e) => {
                tracing::warn!("tunnel connect failed: {e}");
            }
        }

        if pause_unless_shutdown(&background, retry_secs).await {
            return;
        }
        retry_secs = (retry_secs.saturating_mul(2)).min(max_backoff_secs);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn envelope_serialization_contains_required_fields() {
        let env = build_envelope("task.ack", "agent-1", json!({"run_id":"r1","status":"accepted"}), None);
        let s = serde_json::to_string(&env).expect("serialize envelope");
        assert!(s.contains("\"id\""));
        assert!(s.contains("\"type\":\"task.ack\""));
        assert!(s.contains("\"ts\""));
        assert!(s.contains("\"agent_id\":\"agent-1\""));
        assert!(s.contains("\"payload\""));
    }

    #[test]
    fn envelope_signature_sign_and_verify() {
        let mut env = build_envelope("heartbeat", "agent-1", json!({"alive":true}), None);
        let sig = sign_envelope_body(&env, "secret").expect("sign");
        env.sig = Some(format!("sha256={sig}"));

        assert!(verify_envelope_signature(&env, "secret"));
        assert!(!verify_envelope_signature(&env, "wrong-secret"));
    }

    #[test]
    fn unsigned_envelope_fails_verification() {
        let env = build_envelope("heartbeat", "agent-1", json!({"alive":true}), None);
        assert!(!verify_envelope_signature(&env, "secret"));
    }

    fn signed(secret: &str) -> (Envelope, String) {
        let env = build_envelope("task.dispatch", "agent-1", json!({"run_id":"r1"}), None);
        let hex = sign_envelope_body(&env, secret).expect("sign");
        (env, hex)
    }

    fn with_sig(mut env: Envelope, sig: &str) -> Envelope {
        env.sig = Some(sig.to_string());
        env
    }

    #[test]
    fn envelope_signature_is_accepted_with_any_number_of_prefixes() {
        let (env, hex) = signed("secret");
        assert!(verify_envelope_signature(&with_sig(env.clone(), &hex), "secret"));
        assert!(verify_envelope_signature(
            &with_sig(env, &format!("sha256=sha256={hex}")),
            "secret"
        ));
    }

    #[test]
    fn envelope_signature_in_uppercase_hex_is_refused() {
        let (env, hex) = signed("secret");
        assert!(!verify_envelope_signature(
            &with_sig(env, &format!("sha256={}", hex.to_ascii_uppercase())),
            "secret"
        ));
    }

    #[test]
    fn malformed_envelope_signatures_are_refused() {
        let (env, hex) = signed("secret");
        for sig in [
            format!("sha256={}", &hex[..63]),
            format!("sha256={}", &hex[..62]),
            format!("sha256={hex}00"),
            format!("sha256=g{}", &hex[1..]),
            "sha256=".to_string(),
            String::new(),
        ] {
            assert!(
                !verify_envelope_signature(&with_sig(env.clone(), &sig), "secret"),
                "{sig:?} must be refused"
            );
        }
    }

    #[test]
    fn envelope_signature_does_not_survive_a_changed_payload() {
        let (mut env, hex) = signed("secret");
        env.payload = json!({"run_id":"r2"});
        assert!(!verify_envelope_signature(&with_sig(env, &hex), "secret"));
    }
}
