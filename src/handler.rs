use crate::{dispatcher, signature, AppState};
use axum::{
    extract::State,
    http::{HeaderMap, StatusCode},
    Json,
};
use serde_json::{json, Value};
use std::sync::Arc;

pub async fn handle_webhook(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    body: String,
) -> Result<Json<Value>, StatusCode> {
    // 1. Signature verification
    if !state.config.security.allow_unsigned {
        let sig_header = headers
            .get("x-webhook-signature")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("");
        let sig = sig_header.strip_prefix("sha256=").unwrap_or(sig_header);
        if !signature::verify_signature(
            body.as_bytes(),
            sig,
            &state.config.security.webhook_secrets,
        ) {
            tracing::warn!("Invalid webhook signature");
            return Err(StatusCode::UNAUTHORIZED);
        }
    }

    // 2. Parse payload
    let payload: Value = serde_json::from_str(&body).map_err(|_| StatusCode::BAD_REQUEST)?;

    let event = payload
        .get("event")
        .and_then(|v| v.as_str())
        .unwrap_or("unknown");
    tracing::info!("Received webhook event: {}", event);

    // 3. Check if bot task
    let bot_context = payload.get("bot_context");
    let is_bot_task = bot_context
        .and_then(|bc| bc.get("is_bot_task"))
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    if !is_bot_task {
        return Ok(Json(json!({"status": "ignored", "reason": "not_bot_task"})));
    }

    // 4. Find matching agent
    let bot_name = bot_context
        .and_then(|bc| bc.get("bot_name"))
        .and_then(|v| v.as_str())
        .unwrap_or("");
    let agent_type = bot_context
        .and_then(|bc| bc.get("bot_agent_type"))
        .and_then(|v| v.as_str())
        .unwrap_or("openclaw");

    let agent = state
        .config
        .agents
        .iter()
        .find(|a| a.id == bot_name || a.name.to_lowercase() == bot_name.to_lowercase())
        .or_else(|| state.config.agents.iter().find(|a| a.agent_type == agent_type));

    match agent {
        Some(a) => {
            tracing::info!("Dispatching to agent: {} ({})", a.name, a.agent_type);
            let result = dispatcher::dispatch(a, &payload).await;
            Ok(Json(
                json!({"status": "dispatched", "agent": a.id, "result": result}),
            ))
        }
        None => {
            tracing::warn!("No agent for bot_name={} type={}", bot_name, agent_type);
            Ok(Json(json!({"status": "no_agent", "bot_name": bot_name})))
        }
    }
}
