use crate::{
    AppState, ai_task, compat,
    config::{AgentConfig, RouteConfig},
    dispatcher,
    replay::Refusal,
    signature,
};
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

/// The trigger kind of a worker AI task: its top-level `trigger_kind`, one of the values
/// [`crate::config::TRIGGER_KINDS`] lists. `task_type` and the inner `payload` use other values (or
/// are free-form) and would never match a valid `route.trigger_kinds` entry.
fn worker_trigger_kind(payload: &Value) -> Option<String> {
    string_field(payload, "trigger_kind").map(ToString::to_string)
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
            .map(ToString::to_string),
        event: event_from_payload(payload),
        form_key: form_key_from_payload(payload),
    })
}

fn ai_task_envelope_target(payload: &Value) -> Option<BotDispatchTarget> {
    string_field(payload, "task_id")?;
    let bot_key = string_field(payload, "ai_participant_id")?.to_string();

    // The bot's agent type, as the worker reads it from the bot user. The task's inner `payload`
    // is free-form data from whoever created the task and must not choose the agent type.
    let agent_type = string_field(payload, "ai_participant_agent_type").map(ToString::to_string);

    Some(BotDispatchTarget {
        bot_name: None,
        bot_id: Some(bot_key.clone()),
        bot_key,
        has_bot_identity: true,
        agent_type,
        project_type: project_type_from_payload(payload),
        trigger_kind: worker_trigger_kind(payload),
        event: event_from_payload(payload),
        form_key: form_key_from_payload(payload),
    })
}

fn bot_dispatch_target(payload: &Value) -> Option<BotDispatchTarget> {
    bot_context_target(payload).or_else(|| ai_task_envelope_target(payload))
}

/// One route list. An empty list is "not configured" and restricts nothing; a configured list
/// matches only when the task's value is present and equals one entry (ignoring case and
/// surrounding whitespace), so a task without the value fails it.
fn list_matches_constraint(values: &[String], actual: Option<&str>) -> bool {
    values.is_empty()
        || actual.is_some_and(|actual| {
            values
                .iter()
                .any(|value| value.trim().eq_ignore_ascii_case(actual.trim()))
        })
}

/// The bot identity constraint is `bot_names` and `bot_ids` taken together: with neither
/// configured it restricts nothing; otherwise at least one configured list must contain the
/// bot's name or id. A list that is not configured never satisfies the constraint (an empty
/// list must not count as a match), so a route with only `bot_ids` rejects every other bot.
fn bot_identity_matches(route: &RouteConfig, target: &BotDispatchTarget) -> bool {
    if route.bot_names.is_empty() && route.bot_ids.is_empty() {
        return true;
    }
    let bot_name = target.bot_name.as_deref().unwrap_or(target.bot_key.as_str());
    let bot_id = target.bot_id.as_deref().unwrap_or(target.bot_key.as_str());
    (!route.bot_names.is_empty() && list_matches_constraint(&route.bot_names, Some(bot_name)))
        || (!route.bot_ids.is_empty() && list_matches_constraint(&route.bot_ids, Some(bot_id)))
}

fn agent_matches_route(agent: &AgentConfig, target: &BotDispatchTarget) -> bool {
    let Some(route) = agent.route.as_ref() else {
        return true;
    };

    bot_identity_matches(route, target)
        && list_matches_constraint(&route.bot_agent_types, target.agent_type.as_deref())
        && list_matches_constraint(&route.project_types, target.project_type.as_deref())
        && list_matches_constraint(&route.trigger_kinds, target.trigger_kind.as_deref())
        && list_matches_constraint(&route.events, target.event.as_deref())
        && list_matches_constraint(&route.form_keys, target.form_key.as_deref())
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

/// The outcome reported for a worker AI task that no configured agent matches.
fn no_agent_outcome(target: &BotDispatchTarget) -> ai_task::TaskOutcome {
    let summary = format!(
        "sylvode-webhook has no agent for this AI task (bot {}, agent type {}, trigger kind {}); \
         add an [[agents]] entry whose id, name or route matches the bot",
        target.bot_key,
        target.agent_type.as_deref().unwrap_or("unknown"),
        target.trigger_kind.as_deref().unwrap_or("unknown"),
    );
    ai_task::TaskOutcome {
        succeeded: false,
        status: "error".to_string(),
        summary,
        exit_code: None,
        duration_ms: None,
        output: None,
        // No agent will match on a retry either, unless the configuration changes.
        retryable: false,
    }
}

pub async fn handle_webhook(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    body: String,
) -> Result<Json<Value>, StatusCode> {
    // 1. Signature verification
    if !state.config.security.allow_unsigned {
        let inbound = match signature::extract_signature_from_headers(&headers) {
            Ok(inbound) => inbound,
            Err(e) => {
                tracing::warn!("Refused webhook signature headers: {e}");
                return Err(StatusCode::UNAUTHORIZED);
            }
        };
        let sig = inbound.value.unwrap_or_default();
        if !signature::verify_signature(body.as_bytes(), &sig, &state.config.security.webhook_secrets) {
            tracing::warn!("Invalid webhook signature");
            return Err(StatusCode::UNAUTHORIZED);
        }
        // Only a verified request may use up the once-per-process notice.
        if inbound.used_legacy_header {
            compat::warn_once(compat::LegacyEntry::SignatureHeader);
        }
    }

    // 2. Parse payload
    let payload: Value = serde_json::from_str(&body).map_err(|_| StatusCode::BAD_REQUEST)?;

    // 3. A signed request must not be a replay of an earlier one.
    if !state.config.security.allow_unsigned {
        let checked = state.replay.lock().await.check(&payload);
        match checked {
            Ok(()) => {}
            Err(Refusal::Stale { skew_secs }) => {
                tracing::warn!(
                    skew_secs,
                    "Refused a delivery whose timestamp is {skew_secs}s away from now (replay or clock skew)"
                );
                return Err(StatusCode::UNAUTHORIZED);
            }
            Err(Refusal::Duplicate { key }) => {
                tracing::warn!(key = ?key, "Refused a delivery that was already received (replay)");
                return Err(StatusCode::CONFLICT);
            }
        }
    }

    let event = payload.get("event").and_then(Value::as_str).unwrap_or("unknown");
    tracing::info!(event = ?event, "Received webhook event");

    // 4. Check if bot task. Supports both direct Sylvode webhook payloads
    // and worker-dispatched ai_tasks envelopes.
    let Some(target) = bot_dispatch_target(&payload) else {
        tracing::debug!(event = ?event, "Webhook event dropped: is_bot_task=false");
        return Ok(Json(json!({"status": "ignored", "reason": "not_bot_task"})));
    };

    // 5. Find matching agent
    let route = json!({
        "bot_key": &target.bot_key,
        "bot_name": target.bot_name.as_deref(),
        "bot_id": target.bot_id.as_deref(),
        "agent_type": target.agent_type.as_deref(),
        "project_type": target.project_type.as_deref(),
        "trigger_kind": target.trigger_kind.as_deref(),
        "event": target.event.as_deref(),
        "form_key": target.form_key.as_deref(),
    });

    // For a worker AI task, whether its outcome will be reported to the Sylvode API.
    let task_bot = ai_task_bot(&payload);
    let callback = |agent| task_bot.map(|bot| state.config.ai_task_callback_for(agent, bot).answer());

    if let Some(a) = select_agent(&state.config.agents, &target) {
        tracing::info!(agent = ?a.id, agent_type = ?a.agent_type, "Dispatching to agent");
        let result = dispatcher::dispatch(&state.config, &state.background, a, &payload).await;
        tracing::info!(agent = ?a.id, detail = ?result.detail, "Dispatch result");
        if result.outcome == dispatcher::Outcome::Refused {
            // The agent was not started; the worker fails the task on this answer.
            return Ok(Json(json!({
                "status": "refused",
                "agent": a.id,
                "reason": result.summary,
                "route": route,
                "ai_task_callback": callback(Some(a)),
            })));
        }
        Ok(Json(json!({
            "status": "dispatched",
            "agent": a.id,
            "result": result.summary,
            "route": route,
            "ai_task_callback": callback(Some(a)),
        })))
    } else {
        tracing::warn!(
            bot_key = ?target.bot_key,
            agent_type = ?target.agent_type,
            project_type = ?target.project_type,
            trigger_kind = ?target.trigger_kind,
            "No agent for bot task"
        );
        let outcome = no_agent_outcome(&target);
        let reason = outcome.summary.clone();
        // A worker AI task is `processing` in Sylvode from the moment it was delivered. The worker
        // fails it on this answer; with `[ai_task_callback]` enabled and a token of the task's bot
        // configured, it is reported failed here too (whichever arrives first ends it).
        if let Some(completion) = ai_task::Completion::for_unmatched(&state.config, &payload) {
            completion.report_in_background(outcome, &state.background);
        }
        Ok(Json(json!({
            "status": "no_agent",
            "bot_key": target.bot_key,
            "reason": reason,
            "route": route,
            "ai_task_callback": callback(None),
        })))
    }
}

/// The bot of a worker AI task delivery (`ai_participant_id` next to a `task_id`), or `None` for
/// any other delivery.
fn ai_task_bot(payload: &Value) -> Option<&str> {
    string_field(payload, "task_id")?;
    string_field(payload, "ai_participant_id")
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
            "trigger_kind": "assigned",
            "payload": {
                "agent_type": "webhook"
            }
        });

        let target = bot_dispatch_target(&payload).expect("target");
        assert_eq!(target.bot_key, "bot-user-1");
        assert_eq!(target.bot_id.as_deref(), Some("bot-user-1"));
        assert_eq!(target.agent_type.as_deref(), Some("cli"));
        assert_eq!(target.project_type.as_deref(), None);
        assert_eq!(target.trigger_kind.as_deref(), Some("assigned"));
        assert_eq!(target.event.as_deref(), None);
    }

    #[test]
    fn extracts_worker_ai_task_envelope_route_target() {
        let payload = json!({
            "task_id": "task-1",
            "ai_participant_id": "bot-user-1",
            "ai_participant_agent_type": "webhook",
            "task_type": "comment_requested",
            "trigger_kind": "mention",
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

    /// Sylvode retired connectors, so a delivery shaped like one carries no bot identity and
    /// must not be routed to an agent by its connector kind any more.
    #[test]
    fn ignores_a_connector_shaped_payload() {
        let payload = json!({
            "event": "form.record.created",
            "connector_id": "connector-1",
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
        });

        assert!(bot_dispatch_target(&payload).is_none());
    }

    #[test]
    fn ignores_non_bot_payload() {
        let payload = json!({"event": "issue.created"});
        assert!(bot_dispatch_target(&payload).is_none());
    }

    /// The trigger kind is read only from the fields whose values `route.trigger_kinds` accepts:
    /// `bot_context.trigger_reason` (or `trigger_kind`) and a worker task's top-level
    /// `trigger_kind`. `task_type`, `payload.trigger` and `payload.trigger_kind` carry other
    /// vocabularies, or free-form data, and are not consulted.
    #[test]
    fn trigger_kind_comes_only_from_the_fields_the_whitelist_describes() {
        let worker = bot_dispatch_target(&json!({
            "task_id": "t",
            "ai_participant_id": "bot",
            "task_type": "issue_assigned",
            "payload": {"trigger": "issue.create", "trigger_kind": "assigned"}
        }))
        .expect("target");
        assert_eq!(worker.trigger_kind, None);

        let worker = bot_dispatch_target(&json!({
            "task_id": "t",
            "ai_participant_id": "bot",
            "task_type": "issue_assigned",
            "trigger_kind": "assigned"
        }))
        .expect("target");
        assert_eq!(worker.trigger_kind.as_deref(), Some("assigned"));

        let delivery = bot_dispatch_target(&json!({
            "bot_context": {"is_bot_task": true, "bot_name": "Fixer"},
            "trigger_kind": "assigned",
            "payload": {"trigger_kind": "assigned"}
        }))
        .expect("target");
        assert_eq!(delivery.trigger_kind, None);

        let agents = vec![agent(
            "id = \"fixer\"\nname = \"Fixer\"\nagent_type = \"cli\"\n[route]\ntrigger_kinds = [\"assigned\"]\n",
        )];
        assert!(select_agent(&agents, &worker_with_kind_in_payload()).is_none());
    }

    fn worker_with_kind_in_payload() -> super::BotDispatchTarget {
        bot_dispatch_target(&json!({
            "task_id": "t",
            "ai_participant_id": "fixer",
            "task_type": "assigned",
            "payload": {"trigger_kind": "assigned"}
        }))
        .expect("target")
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

    /// A bot-task target with the given direct bot name and id.
    fn named_target(bot_name: &str, bot_id: Option<&str>) -> super::BotDispatchTarget {
        let context = json!({
            "is_bot_task": true,
            "bot_name": bot_name,
            "bot_id": bot_id,
            "bot_agent_type": "webhook"
        });
        bot_dispatch_target(&json!({"bot_context": context})).expect("target")
    }

    fn routed(route: &str) -> Vec<AgentConfig> {
        vec![agent(&format!(
            "id = \"routed\"\nname = \"Routed\"\nagent_type = \"webhook\"\n\n[route]\n{route}\n"
        ))]
    }

    #[test]
    fn a_bot_ids_only_route_rejects_every_other_bot() {
        let agents = routed(r#"bot_ids = ["bot-1"]"#);
        assert!(select_agent(&agents, &named_target("Other", Some("bot-unknown"))).is_none());
        // The same agent type does not let it fall back to the route's agent.
        assert!(select_agent(&agents, &named_target("Routed", Some("bot-unknown"))).is_none());
        assert!(select_agent(&agents, &named_target("Other", Some("bot-1"))).is_some());
        assert!(select_agent(&agents, &named_target("Other", Some(" BOT-1 "))).is_some());
    }

    #[test]
    fn a_bot_names_only_route_rejects_every_other_bot() {
        let agents = routed(r#"bot_names = ["Fixer"]"#);
        assert!(select_agent(&agents, &named_target("Stranger", Some("bot-1"))).is_none());
        assert!(select_agent(&agents, &named_target("fixer", Some("bot-9"))).is_some());
    }

    #[test]
    fn with_bot_names_and_bot_ids_either_list_is_enough() {
        let agents = routed("bot_names = [\"Fixer\"]\nbot_ids = [\"bot-1\"]");
        assert!(select_agent(&agents, &named_target("Fixer", Some("bot-9"))).is_some());
        assert!(select_agent(&agents, &named_target("Stranger", Some("bot-1"))).is_some());
        assert!(select_agent(&agents, &named_target("Stranger", Some("bot-9"))).is_none());
    }

    #[test]
    fn a_route_without_bot_identity_lists_does_not_restrict_the_bot() {
        let agents = routed(r#"bot_agent_types = ["webhook"]"#);
        assert!(select_agent(&agents, &named_target("Anyone", Some("bot-9"))).is_some());
        let none = routed("");
        assert!(select_agent(&none, &named_target("Anyone", None)).is_some());
    }

    #[test]
    fn a_bot_identity_constraint_still_combines_with_the_other_lists() {
        let agents = routed("bot_ids = [\"bot-1\"]\nproject_types = [\"contract_review\"]");
        // The bot matches but the task has no project type.
        assert!(select_agent(&agents, &named_target("Fixer", Some("bot-1"))).is_none());
    }

    #[test]
    fn a_worker_task_is_matched_by_its_participant_id() {
        let agents = routed(r#"bot_ids = ["bot-1"]"#);
        let task = |id: &str| {
            bot_dispatch_target(&json!({
                "task_id": "t-1",
                "ai_participant_id": id,
                "ai_participant_agent_type": "webhook",
                "trigger_kind": "assigned"
            }))
            .expect("target")
        };
        assert!(select_agent(&agents, &task("bot-1")).is_some());
        assert!(select_agent(&agents, &task("bot-unknown")).is_none());
    }
}
