use crate::{AppState, config::AgentConfig, dispatcher, signature};
use axum::{
    Json,
    extract::State,
    http::{HeaderMap, StatusCode},
};
use serde_json::{Value, json};
use std::sync::Arc;

#[derive(Debug, Clone, PartialEq, Eq)]
struct BotDispatchTarget {
    bot_key: String,
    has_bot_identity: bool,
    bot_name: Option<String>,
    bot_id: Option<String>,
    agent_type: Option<String>,
    project_type: Option<String>,
    trigger_kind: Option<String>,
    event: Option<String>,
    form_key: Option<String>,
    connector_kind: Option<String>,
}

fn string_field<'a>(value: &'a Value, key: &str) -> Option<&'a str> {
    value.get(key).and_then(Value::as_str).filter(|v| !v.is_empty())
}

fn nested_string_field<'a>(value: &'a Value, path: &[&str]) -> Option<&'a str> {
    let mut cursor = value;
    for key in path {
        cursor = cursor.get(*key)?;
    }
    cursor.as_str().filter(|v| !v.is_empty())
}

fn project_type_from_payload(payload: &Value) -> Option<String> {
    string_field(payload, "project_type")
        .or_else(|| string_field(payload, "type_key"))
        .or_else(|| nested_string_field(payload, &["payload", "project_type"]))
        .or_else(|| nested_string_field(payload, &["payload", "type_key"]))
        .or_else(|| nested_string_field(payload, &["payload", "envelope", "project_type"]))
        .or_else(|| nested_string_field(payload, &["payload", "envelope", "metadata", "project_type"]))
        .or_else(|| nested_string_field(payload, &["payload", "envelope", "payload", "project_type"]))
        .or_else(|| nested_string_field(payload, &["project", "project_type"]))
        .or_else(|| nested_string_field(payload, &["project", "type_key"]))
        .or_else(|| nested_string_field(payload, &["data", "project", "project_type"]))
        .or_else(|| nested_string_field(payload, &["data", "project", "type_key"]))
        .map(ToString::to_string)
}

fn event_from_payload(payload: &Value) -> Option<String> {
    string_field(payload, "event")
        .or_else(|| string_field(payload, "event_type"))
        .or_else(|| nested_string_field(payload, &["payload", "event"]))
        .or_else(|| nested_string_field(payload, &["payload", "event_type"]))
        .or_else(|| nested_string_field(payload, &["payload", "envelope", "event_type"]))
        .map(ToString::to_string)
}

fn trigger_kind_from_payload(payload: &Value) -> Option<String> {
    string_field(payload, "trigger_kind")
        .or_else(|| nested_string_field(payload, &["payload", "trigger_kind"]))
        .or_else(|| nested_string_field(payload, &["payload", "trigger"]))
        .or_else(|| string_field(payload, "task_type"))
        .map(ToString::to_string)
}

fn form_key_from_payload(payload: &Value) -> Option<String> {
    string_field(payload, "form_key")
        .or_else(|| nested_string_field(payload, &["payload", "form_key"]))
        .or_else(|| nested_string_field(payload, &["metadata", "form_key"]))
        .or_else(|| nested_string_field(payload, &["payload", "metadata", "form_key"]))
        .or_else(|| nested_string_field(payload, &["payload", "envelope", "metadata", "form_key"]))
        .or_else(|| nested_string_field(payload, &["payload", "envelope", "payload", "form_key"]))
        .map(ToString::to_string)
        .or_else(|| {
            nested_string_field(payload, &["payload", "envelope", "aggregate", "type"])
                .and_then(|value| value.strip_prefix("form."))
                .filter(|value| !value.is_empty())
                .map(ToString::to_string)
        })
}

fn connector_kind_from_payload(payload: &Value) -> Option<String> {
    string_field(payload, "connector_kind")
        .or_else(|| nested_string_field(payload, &["payload", "connector_kind"]))
        .map(ToString::to_string)
}

fn bot_context_target(payload: &Value) -> Option<BotDispatchTarget> {
    let bot_context = payload.get("bot_context")?;
    let is_bot_task = bot_context.get("is_bot_task").and_then(Value::as_bool).unwrap_or(false);
    if !is_bot_task {
        return None;
    }

    let bot_name = string_field(bot_context, "bot_name").map(ToString::to_string);
    let bot_id = string_field(bot_context, "bot_id").map(ToString::to_string);
    let bot_key = bot_name.clone().or_else(|| bot_id.clone())?;

    Some(BotDispatchTarget {
        bot_key,
        has_bot_identity: true,
        bot_name,
        bot_id,
        agent_type: string_field(bot_context, "bot_agent_type").map(ToString::to_string),
        project_type: string_field(bot_context, "project_type")
            .map(ToString::to_string)
            .or_else(|| project_type_from_payload(payload)),
        trigger_kind: string_field(bot_context, "trigger_kind")
            .or_else(|| string_field(bot_context, "trigger_reason"))
            .map(ToString::to_string)
            .or_else(|| trigger_kind_from_payload(payload)),
        event: event_from_payload(payload),
        form_key: form_key_from_payload(payload),
        connector_kind: connector_kind_from_payload(payload),
    })
}

fn ai_task_envelope_target(payload: &Value) -> Option<BotDispatchTarget> {
    string_field(payload, "task_id")?;
    let bot_key = string_field(payload, "ai_participant_id")?.to_string();

    let agent_type = string_field(payload, "agent_type")
        .or_else(|| string_field(payload, "ai_participant_agent_type"))
        .or_else(|| {
            payload
                .get("payload")
                .and_then(|inner| string_field(inner, "bot_agent_type"))
        })
        .or_else(|| {
            payload
                .get("payload")
                .and_then(|inner| string_field(inner, "agent_type"))
        })
        .map(ToString::to_string);

    Some(BotDispatchTarget {
        bot_name: None,
        bot_id: Some(bot_key.clone()),
        bot_key,
        has_bot_identity: true,
        agent_type,
        project_type: project_type_from_payload(payload),
        trigger_kind: trigger_kind_from_payload(payload),
        event: event_from_payload(payload),
        form_key: form_key_from_payload(payload),
        connector_kind: connector_kind_from_payload(payload),
    })
}

fn connector_envelope_target(payload: &Value) -> Option<BotDispatchTarget> {
    let connector_kind = connector_kind_from_payload(payload);
    let event = event_from_payload(payload);
    let has_connector_shape = connector_kind.is_some()
        || string_field(payload, "connector_id").is_some()
        || nested_string_field(payload, &["payload", "business_event_id"]).is_some()
        || nested_string_field(payload, &["payload", "outbox_id"]).is_some()
        || nested_string_field(payload, &["payload", "envelope", "version"]) == Some("openpr.event.v1");
    if !has_connector_shape {
        return None;
    }

    Some(BotDispatchTarget {
        bot_key: connector_kind
            .clone()
            .or_else(|| form_key_from_payload(payload))
            .or_else(|| event.clone())
            .unwrap_or_else(|| "connector".to_string()),
        has_bot_identity: false,
        bot_name: None,
        bot_id: None,
        agent_type: string_field(payload, "agent_type")
            .or_else(|| nested_string_field(payload, &["payload", "agent_type"]))
            .map(ToString::to_string),
        project_type: project_type_from_payload(payload),
        trigger_kind: trigger_kind_from_payload(payload).or_else(|| event.clone()),
        event,
        form_key: form_key_from_payload(payload),
        connector_kind,
    })
}

fn bot_dispatch_target(payload: &Value) -> Option<BotDispatchTarget> {
    bot_context_target(payload)
        .or_else(|| ai_task_envelope_target(payload))
        .or_else(|| connector_envelope_target(payload))
}

fn list_matches_constraint(values: &[String], actual: Option<&str>) -> bool {
    values.is_empty()
        || actual.is_some_and(|actual| {
            values
                .iter()
                .any(|value| value.trim().eq_ignore_ascii_case(actual.trim()))
        })
}

fn agent_matches_route(agent: &AgentConfig, target: &BotDispatchTarget) -> bool {
    let Some(route) = agent.route.as_ref() else {
        return true;
    };

    let has_bot_identity_constraint = !route.bot_names.is_empty() || !route.bot_ids.is_empty();
    let bot_name = target.bot_name.as_deref().unwrap_or(target.bot_key.as_str());
    let bot_id = target.bot_id.as_deref().unwrap_or(target.bot_key.as_str());
    let bot_identity_matches = !has_bot_identity_constraint
        || list_matches_constraint(&route.bot_names, Some(bot_name))
        || list_matches_constraint(&route.bot_ids, Some(bot_id));

    bot_identity_matches
        && list_matches_constraint(&route.bot_agent_types, target.agent_type.as_deref())
        && list_matches_constraint(&route.project_types, target.project_type.as_deref())
        && list_matches_constraint(&route.trigger_kinds, target.trigger_kind.as_deref())
        && list_matches_constraint(&route.events, target.event.as_deref())
        && list_matches_constraint(&route.form_keys, target.form_key.as_deref())
        && list_matches_constraint(&route.connector_kinds, target.connector_kind.as_deref())
}

fn select_agent<'a>(agents: &'a [AgentConfig], target: &BotDispatchTarget) -> Option<&'a AgentConfig> {
    if target.has_bot_identity {
        let exact = agents.iter().find(|agent| {
            (agent.id == target.bot_key || agent.name.eq_ignore_ascii_case(&target.bot_key))
                && agent_matches_route(agent, target)
        });
        if exact.is_some() {
            return exact;
        }
    }

    if let Some(agent_type) = target.agent_type.as_deref()
        && let Some(agent) = agents
            .iter()
            .find(|agent| agent.agent_type == agent_type && agent_matches_route(agent, target))
    {
        return Some(agent);
    }

    agents
        .iter()
        .find(|agent| agent.route.is_some() && agent_matches_route(agent, target))
}

pub async fn handle_webhook(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    body: String,
) -> Result<Json<Value>, StatusCode> {
    // 1. Signature verification
    if !state.config.security.allow_unsigned {
        let sig = signature::extract_signature_from_headers(&headers).unwrap_or_default();
        if !signature::verify_signature(body.as_bytes(), &sig, &state.config.security.webhook_secrets) {
            tracing::warn!("Invalid webhook signature");
            return Err(StatusCode::UNAUTHORIZED);
        }
    }

    // 2. Parse payload
    let payload: Value = serde_json::from_str(&body).map_err(|_| StatusCode::BAD_REQUEST)?;

    let event = payload.get("event").and_then(Value::as_str).unwrap_or("unknown");
    tracing::info!("Received webhook event: {event}");

    // 3. Check if bot task. Supports both direct OpenPR webhook payloads
    // and worker-dispatched ai_tasks envelopes.
    let Some(target) = bot_dispatch_target(&payload) else {
        tracing::debug!(event = %event, "Webhook event dropped: is_bot_task=false");
        return Ok(Json(json!({"status": "ignored", "reason": "not_bot_task"})));
    };

    // 4. Find matching agent
    let route = json!({
        "bot_key": &target.bot_key,
        "bot_name": target.bot_name.as_deref(),
        "bot_id": target.bot_id.as_deref(),
        "agent_type": target.agent_type.as_deref(),
        "project_type": target.project_type.as_deref(),
        "trigger_kind": target.trigger_kind.as_deref(),
        "event": target.event.as_deref(),
        "form_key": target.form_key.as_deref(),
        "connector_kind": target.connector_kind.as_deref(),
    });

    if let Some(a) = select_agent(&state.config.agents, &target) {
        tracing::info!("Dispatching to agent: {} ({})", a.name, a.agent_type);
        let result = dispatcher::dispatch(&state.config, a, &payload).await;
        Ok(Json(
            json!({"status": "dispatched", "agent": a.id, "result": result, "route": route}),
        ))
    } else {
        tracing::warn!(
            bot_key = %target.bot_key,
            agent_type = ?target.agent_type,
            project_type = ?target.project_type,
            trigger_kind = ?target.trigger_kind,
            "No agent for bot task"
        );
        Ok(Json(
            json!({"status": "no_agent", "bot_key": target.bot_key, "route": route}),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::{bot_dispatch_target, select_agent};
    use crate::config::AgentConfig;
    use serde_json::json;

    fn agent(toml: &str) -> AgentConfig {
        toml::from_str(toml).expect("agent config")
    }

    #[test]
    fn extracts_direct_bot_context_target() {
        let payload = json!({
            "event": "comment.created",
            "bot_context": {
                "is_bot_task": true,
                "bot_name": "Codex",
                "bot_agent_type": "cli"
            }
        });

        let target = bot_dispatch_target(&payload).expect("target");
        assert_eq!(target.bot_key, "Codex");
        assert_eq!(target.bot_name.as_deref(), Some("Codex"));
        assert_eq!(target.bot_id.as_deref(), None);
        assert_eq!(target.agent_type.as_deref(), Some("cli"));
        assert_eq!(target.project_type.as_deref(), None);
        assert_eq!(target.trigger_kind.as_deref(), None);
        assert_eq!(target.event.as_deref(), Some("comment.created"));
    }

    #[test]
    fn extracts_direct_bot_context_route_target() {
        let payload = json!({
            "event": "comment.created",
            "bot_context": {
                "is_bot_task": true,
                "bot_name": "Document review connection",
                "bot_agent_type": "webhook",
                "project_type": "contract_review",
                "trigger_kind": "mention"
            }
        });

        let target = bot_dispatch_target(&payload).expect("target");
        assert_eq!(target.bot_key, "Document review connection");
        assert_eq!(target.bot_name.as_deref(), Some("Document review connection"));
        assert_eq!(target.agent_type.as_deref(), Some("webhook"));
        assert_eq!(target.project_type.as_deref(), Some("contract_review"));
        assert_eq!(target.trigger_kind.as_deref(), Some("mention"));
        assert_eq!(target.event.as_deref(), Some("comment.created"));
    }

    #[test]
    fn extracts_worker_ai_task_envelope_target() {
        let payload = json!({
            "task_id": "task-1",
            "ai_participant_id": "bot-user-1",
            "ai_participant_agent_type": "cli",
            "task_type": "issue_assigned",
            "payload": {
                "agent_type": "webhook"
            }
        });

        let target = bot_dispatch_target(&payload).expect("target");
        assert_eq!(target.bot_key, "bot-user-1");
        assert_eq!(target.bot_id.as_deref(), Some("bot-user-1"));
        assert_eq!(target.agent_type.as_deref(), Some("cli"));
        assert_eq!(target.project_type.as_deref(), None);
        assert_eq!(target.trigger_kind.as_deref(), Some("issue_assigned"));
        assert_eq!(target.event.as_deref(), None);
    }

    #[test]
    fn extracts_worker_ai_task_envelope_route_target() {
        let payload = json!({
            "task_id": "task-1",
            "ai_participant_id": "bot-user-1",
            "ai_participant_agent_type": "webhook",
            "task_type": "mention",
            "payload": {
                "project_type": "equipment_maintenance"
            }
        });

        let target = bot_dispatch_target(&payload).expect("target");
        assert_eq!(target.bot_key, "bot-user-1");
        assert_eq!(target.bot_id.as_deref(), Some("bot-user-1"));
        assert_eq!(target.agent_type.as_deref(), Some("webhook"));
        assert_eq!(target.project_type.as_deref(), Some("equipment_maintenance"));
        assert_eq!(target.trigger_kind.as_deref(), Some("mention"));
        assert_eq!(target.event.as_deref(), None);
    }

    #[test]
    fn extracts_openpr_connector_form_event_target() {
        let payload = json!({
            "event": "form.record.created",
            "invocation_id": "invocation-1",
            "connector_id": "connector-1",
            "connector_kind": "print",
            "payload": {
                "business_event_id": "event-1",
                "outbox_id": "outbox-1",
                "envelope": {
                    "version": "openpr.event.v1",
                    "event_type": "form.record.created",
                    "project_id": "project-1",
                    "metadata": {
                        "project_type": "restaurant",
                        "form_key": "order",
                        "record_id": "record-1"
                    }
                }
            }
        });

        let target = bot_dispatch_target(&payload).expect("target");
        assert!(!target.has_bot_identity);
        assert_eq!(target.bot_key, "print");
        assert_eq!(target.event.as_deref(), Some("form.record.created"));
        assert_eq!(target.project_type.as_deref(), Some("restaurant"));
        assert_eq!(target.form_key.as_deref(), Some("order"));
        assert_eq!(target.connector_kind.as_deref(), Some("print"));
    }

    #[test]
    fn ignores_non_bot_payload() {
        let payload = json!({"event": "issue.created"});
        assert!(bot_dispatch_target(&payload).is_none());
    }

    #[test]
    fn selects_exact_agent_when_route_matches() {
        let agents = vec![agent(
            r#"
id = "contract-review"
name = "Document review connection"
agent_type = "webhook"

[route]
project_types = ["contract_review"]
trigger_kinds = ["mention"]
"#,
        )];
        let target = bot_dispatch_target(&json!({
            "bot_context": {
                "is_bot_task": true,
                "bot_name": "Document review connection",
                "bot_agent_type": "webhook",
                "project_type": "contract_review",
                "trigger_kind": "mention"
            }
        }))
        .expect("target");

        let selected = select_agent(&agents, &target).expect("selected agent");
        assert_eq!(selected.id, "contract-review");
    }

    #[test]
    fn rejects_exact_agent_when_route_mismatches() {
        let agents = vec![agent(
            r#"
id = "contract-review"
name = "Document review connection"
agent_type = "webhook"

[route]
project_types = ["contract_review"]
trigger_kinds = ["mention"]
"#,
        )];
        let target = bot_dispatch_target(&json!({
            "bot_context": {
                "is_bot_task": true,
                "bot_name": "Document review connection",
                "bot_agent_type": "webhook",
                "project_type": "equipment_maintenance",
                "trigger_kind": "mention"
            }
        }))
        .expect("target");

        assert!(select_agent(&agents, &target).is_none());
    }

    #[test]
    fn selects_route_specific_agent_by_project_and_trigger() {
        let agents = vec![
            agent(
                r#"
id = "contract-review"
name = "Contract Review"
agent_type = "webhook"

[route]
project_types = ["contract_review"]
trigger_kinds = ["mention"]
"#,
            ),
            agent(
                r#"
id = "maintenance"
name = "Maintenance"
agent_type = "webhook"

[route]
project_types = ["equipment_maintenance"]
trigger_kinds = ["mention"]
"#,
            ),
        ];
        let target = bot_dispatch_target(&json!({
            "bot_context": {
                "is_bot_task": true,
                "bot_name": "Any assistant",
                "bot_agent_type": "webhook",
                "project_type": "equipment_maintenance",
                "trigger_kind": "mention"
            }
        }))
        .expect("target");

        let selected = select_agent(&agents, &target).expect("selected agent");
        assert_eq!(selected.id, "maintenance");
    }

    #[test]
    fn matches_bot_id_route_when_bot_name_is_present() {
        let agents = vec![agent(
            r#"
id = "contract-review"
name = "Document review connection"
agent_type = "webhook"

[route]
bot_ids = ["bot-uuid-1"]
project_types = ["contract_review"]
"#,
        )];
        let target = bot_dispatch_target(&json!({
            "bot_context": {
                "is_bot_task": true,
                "bot_name": "Document review connection",
                "bot_id": "bot-uuid-1",
                "bot_agent_type": "webhook",
                "project_type": "contract_review"
            }
        }))
        .expect("target");

        let selected = select_agent(&agents, &target).expect("selected agent");
        assert_eq!(selected.id, "contract-review");
    }

    #[test]
    fn keeps_legacy_agent_type_fallback_without_route_config() {
        let agents = vec![agent(
            r#"
id = "legacy-webhook"
name = "Legacy Webhook"
agent_type = "webhook"
"#,
        )];
        let target = bot_dispatch_target(&json!({
            "bot_context": {
                "is_bot_task": true,
                "bot_name": "Unknown",
                "bot_agent_type": "webhook",
                "project_type": "contract_review",
                "trigger_kind": "mention"
            }
        }))
        .expect("target");

        let selected = select_agent(&agents, &target).expect("selected agent");
        assert_eq!(selected.id, "legacy-webhook");
    }

    #[test]
    fn selects_connector_route_by_event_form_and_connector_kind() {
        let agents = vec![
            agent(
                r#"
id = "print-orders"
name = "Print Orders"
agent_type = "webhook"

[route]
events = ["form.record.created"]
form_keys = ["order"]
connector_kinds = ["print"]
"#,
            ),
            agent(
                r#"
id = "print-lines"
name = "Print Lines"
agent_type = "webhook"

[route]
events = ["form.record.created"]
form_keys = ["order_line"]
connector_kinds = ["print"]
"#,
            ),
        ];
        let target = bot_dispatch_target(&json!({
            "event": "form.record.created",
            "connector_kind": "print",
            "payload": {
                "business_event_id": "event-1",
                "envelope": {
                    "version": "openpr.event.v1",
                    "metadata": {
                        "form_key": "order"
                    }
                }
            }
        }))
        .expect("target");

        let selected = select_agent(&agents, &target).expect("selected agent");
        assert_eq!(selected.id, "print-orders");
    }
}
