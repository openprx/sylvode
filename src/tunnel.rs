use crate::{config::Config, dispatcher, signature};
use futures_util::{SinkExt, StreamExt};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};
use tokio::sync::mpsc;
use tokio::time::{sleep, Duration};
use tokio_tungstenite::{connect_async, tungstenite::http::Request, tungstenite::Message};
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

pub fn sign_envelope_body(envelope: &Envelope, secret: &str) -> Option<String> {
    let mut unsigned = envelope.clone();
    unsigned.sig = None;
    serde_json::to_vec(&unsigned)
        .ok()
        .map(|bytes| signature::sign_payload(&bytes, secret))
}

pub fn verify_envelope_signature(envelope: &Envelope, secret: &str) -> bool {
    match &envelope.sig {
        Some(sig) => sign_envelope_body(envelope, secret)
            .map(|expected| expected == sig.trim_start_matches("sha256="))
            .unwrap_or(false),
        None => true, // optional verification framework
    }
}

fn now_ts() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

fn build_envelope(
    msg_type: &str,
    agent_id: &str,
    payload: Value,
    hmac_secret: Option<&str>,
) -> Envelope {
    let mut env = Envelope {
        id: Uuid::new_v4().to_string(),
        msg_type: msg_type.to_string(),
        ts: now_ts(),
        agent_id: agent_id.to_string(),
        payload,
        sig: None,
    };

    if let Some(secret) = hmac_secret {
        env.sig = sign_envelope_body(&env, secret).map(|s| format!("sha256={}", s));
    }

    env
}

pub async fn run_tunnel_loop(config: Arc<Config>) {
    let Some(tunnel) = config.tunnel.clone() else {
        return;
    };

    if !tunnel.enabled {
        return;
    }

    let Some(url) = tunnel.url.clone() else {
        tracing::warn!("tunnel enabled but url is missing");
        return;
    };

    let agent_id = tunnel
        .agent_id
        .clone()
        .unwrap_or_else(|| "openpr-webhook".to_string());
    let reconnect_secs = tunnel.reconnect_secs.max(1);
    let heartbeat_secs = tunnel.heartbeat_secs.max(3);
    let hmac_secret = tunnel.hmac_secret.clone();

    loop {
        if !(url.starts_with("wss://") || url.starts_with("ws://")) {
            tracing::error!("tunnel url must start with ws:// or wss://: {}", url);
            sleep(Duration::from_secs(reconnect_secs)).await;
            continue;
        }

        if url.starts_with("ws://") {
            tracing::warn!("tunnel using insecure ws:// transport; prefer wss:// in production");
        }

        let mut req_builder = Request::builder().uri(&url);
        if let Some(token) = &tunnel.auth_token {
            req_builder = req_builder.header("Authorization", format!("Bearer {}", token));
        }

        let request = match req_builder.body(()) {
            Ok(r) => r,
            Err(e) => {
                tracing::error!("build tunnel request failed: {}", e);
                sleep(Duration::from_secs(reconnect_secs)).await;
                continue;
            }
        };

        match connect_async(request).await {
            Ok((ws_stream, _)) => {
                tracing::info!("tunnel connected: {}", url);
                let (mut writer, mut reader) = ws_stream.split();
                let (tx, mut rx) = mpsc::channel::<Envelope>(128);

                let writer_task = tokio::spawn(async move {
                    while let Some(msg) = rx.recv().await {
                        let text = match serde_json::to_string(&msg) {
                            Ok(t) => t,
                            Err(e) => {
                                tracing::warn!("serialize envelope failed: {}", e);
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
                        let hb = build_envelope(
                            "heartbeat",
                            &hb_agent,
                            json!({"alive": true}),
                            hb_secret.as_deref(),
                        );
                        if hb_tx.send(hb).await.is_err() {
                            break;
                        }
                    }
                });

                while let Some(message) = reader.next().await {
                    let Ok(message) = message else { break };
                    let Message::Text(text) = message else {
                        continue;
                    };

                    let envelope: Envelope = match serde_json::from_str(&text) {
                        Ok(v) => v,
                        Err(e) => {
                            tracing::warn!("invalid tunnel message: {}", e);
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

                    if let Some(secret) = &hmac_secret {
                        if !verify_envelope_signature(&envelope, secret) {
                            tracing::warn!(
                                "tunnel signature verify failed for msg_id={}",
                                envelope.id
                            );
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
                    }

                    if envelope.msg_type != "task.dispatch" {
                        continue;
                    }

                    let dispatch =
                        serde_json::from_value::<DispatchPayload>(envelope.payload.clone())
                            .unwrap_or(DispatchPayload {
                                run_id: None,
                                issue_id: None,
                                agent: None,
                                body: Some(envelope.payload.clone()),
                            });

                    let run_id = dispatch
                        .run_id
                        .unwrap_or_else(|| format!("run-{}", Uuid::new_v4()));

                    let issue_id = dispatch.issue_id.or_else(|| {
                        dispatch
                            .body
                            .as_ref()
                            .and_then(dispatcher::extract_issue_id)
                    });

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
                    tokio::spawn(async move {
                        let selected = if let Some(target_id) = route_agent {
                            task_cfg
                                .agents
                                .iter()
                                .find(|a| a.id == target_id && a.agent_type == "cli")
                        } else {
                            task_cfg.agents.iter().find(|a| a.agent_type == "cli")
                        };

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
                            return;
                        };

                        let report =
                            dispatcher::execute_cli_task(cli_agent, &body, Some(run_id_for_task))
                                .await;
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
                let _ = writer_task.await;
                tracing::warn!("tunnel disconnected, reconnecting in {}s", reconnect_secs);
            }
            Err(e) => {
                tracing::warn!("tunnel connect failed: {}", e);
            }
        }

        sleep(Duration::from_secs(reconnect_secs)).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn envelope_serialization_contains_required_fields() {
        let env = build_envelope(
            "task.ack",
            "agent-1",
            json!({"run_id":"r1","status":"accepted"}),
            None,
        );
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
        env.sig = Some(format!("sha256={}", sig));

        assert!(verify_envelope_signature(&env, "secret"));
        assert!(!verify_envelope_signature(&env, "wrong-secret"));
    }
}
