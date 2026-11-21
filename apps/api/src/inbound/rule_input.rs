//! The body of a rule create or replace, validated without touching the database.
//!
//! What needs the database (the bot is a member of the workspace, the labels and recipients belong
//! to it, the bound source belongs to the project) is checked by the management handlers.

use serde::Deserialize;
use serde_json::{Map, Value};
use uuid::Uuid;

use super::{
    rules::{ACTION_WORK_ITEM, ACTION_WORK_ITEM_AND_TASK, valid_kind_pattern},
    severity::Severity,
};

const NAME_MAX: usize = 100;
const INSTRUCTIONS_MAX: usize = 4_000;
const PATTERNS_MAX: usize = 32;
const LIST_MAX: usize = 50;

/// A rule as an administrator writes it. Every field but `name` is optional on create and takes
/// the documented default; a replace states the whole rule.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RuleInput {
    pub name: String,
    pub enabled: Option<bool>,
    pub source_id: Option<Uuid>,
    pub kind_patterns: Option<Vec<String>>,
    pub min_severity: Option<String>,
    pub action: Option<String>,
    pub bot_user_id: Option<Uuid>,
    pub instructions: Option<String>,
    pub priority_map: Option<Value>,
    pub label_ids: Option<Vec<Uuid>>,
    pub notify_user_ids: Option<Vec<Uuid>>,
    pub max_tasks_per_hour: Option<i32>,
    pub max_active_tasks: Option<i32>,
    pub dedup_window_secs: Option<i32>,
    pub rearm_after_secs: Option<i32>,
    pub task_max_attempts: Option<i32>,
    /// Required to replace a rule, refused on create.
    pub expected_version: Option<i32>,
}

/// A rule whose values are all inside their limits.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ValidatedRule {
    pub name: String,
    pub enabled: bool,
    pub source_id: Option<Uuid>,
    pub kind_patterns: Vec<String>,
    pub min_severity: Severity,
    pub action: &'static str,
    pub bot_user_id: Option<Uuid>,
    pub instructions: String,
    pub priority_map: Value,
    pub label_ids: Vec<Uuid>,
    pub notify_user_ids: Vec<Uuid>,
    pub max_tasks_per_hour: i32,
    pub max_active_tasks: i32,
    pub dedup_window_secs: i32,
    pub rearm_after_secs: i32,
    pub task_max_attempts: i32,
}

fn in_range(field: &str, value: Option<i32>, default: i32, low: i32, high: i32) -> Result<i32, String> {
    let value = value.unwrap_or(default);
    if (low..=high).contains(&value) {
        Ok(value)
    } else {
        Err(format!("{field} must be between {low} and {high}"))
    }
}

fn dedup_ids(field: &str, ids: Option<Vec<Uuid>>) -> Result<Vec<Uuid>, String> {
    let mut out: Vec<Uuid> = Vec::new();
    for id in ids.unwrap_or_default() {
        if !out.contains(&id) {
            out.push(id);
        }
    }
    if out.len() > LIST_MAX {
        return Err(format!("{field} has more than {LIST_MAX} entries"));
    }
    Ok(out)
}

/// The default map from severity to work item priority.
pub fn default_priority_map() -> Map<String, Value> {
    Severity::ALL
        .into_iter()
        .map(|severity| {
            (
                severity.as_str().to_owned(),
                Value::String(severity.default_work_item_priority().to_owned()),
            )
        })
        .collect()
}

fn priority_map(input: Option<Value>) -> Result<Value, String> {
    let mut map = default_priority_map();
    match input {
        None | Some(Value::Null) => {}
        Some(Value::Object(given)) => {
            for (severity, priority) in given {
                if Severity::parse(&severity).is_none() {
                    return Err(format!("priority_map names an unknown severity: {severity}"));
                }
                match priority.as_str() {
                    Some(text @ ("low" | "medium" | "high" | "urgent")) => {
                        map.insert(severity, Value::String(text.to_owned()));
                    }
                    _ => return Err("priority_map values must be one of low, medium, high, urgent".to_owned()),
                }
            }
        }
        Some(_) => return Err("priority_map must be an object".to_owned()),
    }
    Ok(Value::Object(map))
}

impl RuleInput {
    /// Checks every value against its limit and applies the defaults. `caller` is the rule's
    /// author, who is the default recipient.
    pub fn validate(self, caller: Uuid) -> Result<ValidatedRule, String> {
        let name = self.name.trim().to_owned();
        if name.is_empty() || name.chars().count() > NAME_MAX {
            return Err("name must be 1-100 characters".to_owned());
        }
        let min_severity = match self.min_severity.as_deref() {
            None => Severity::High,
            Some(text) => Severity::parse(text)
                .ok_or_else(|| "min_severity must be one of info, low, medium, high, critical".to_owned())?,
        };
        // D4: handing the alert to a bot is the default for high and critical rules only.
        let action = match self.action.as_deref() {
            None if min_severity >= Severity::High => ACTION_WORK_ITEM_AND_TASK,
            None | Some(ACTION_WORK_ITEM) => ACTION_WORK_ITEM,
            Some(ACTION_WORK_ITEM_AND_TASK) => ACTION_WORK_ITEM_AND_TASK,
            Some(_) => return Err("action must be work_item or work_item_and_task".to_owned()),
        };
        match (action, self.bot_user_id) {
            (ACTION_WORK_ITEM_AND_TASK, None) => {
                return Err("bot_user_id is required for work_item_and_task".to_owned());
            }
            (ACTION_WORK_ITEM, Some(_)) => return Err("bot_user_id is only valid with work_item_and_task".to_owned()),
            _ => {}
        }
        let mut kind_patterns: Vec<String> = Vec::new();
        for pattern in self.kind_patterns.unwrap_or_default() {
            if !valid_kind_pattern(&pattern) {
                return Err(format!(
                    "kind_patterns entry is not a kind or a prefix wildcard: {pattern}"
                ));
            }
            if !kind_patterns.contains(&pattern) {
                kind_patterns.push(pattern);
            }
        }
        if kind_patterns.len() > PATTERNS_MAX {
            return Err("kind_patterns has more than 32 entries".to_owned());
        }
        let instructions = self.instructions.unwrap_or_default();
        if instructions.chars().count() > INSTRUCTIONS_MAX {
            return Err("instructions are longer than 4000 characters".to_owned());
        }
        let notify_user_ids = match self.notify_user_ids {
            Some(ids) => dedup_ids("notify_user_ids", Some(ids))?,
            None => vec![caller],
        };
        Ok(ValidatedRule {
            name,
            enabled: self.enabled.unwrap_or(true),
            source_id: self.source_id,
            kind_patterns,
            min_severity,
            action,
            bot_user_id: self.bot_user_id,
            instructions,
            priority_map: priority_map(self.priority_map)?,
            label_ids: dedup_ids("label_ids", self.label_ids)?,
            notify_user_ids,
            max_tasks_per_hour: in_range("max_tasks_per_hour", self.max_tasks_per_hour, 6, 1, 120)?,
            max_active_tasks: in_range("max_active_tasks", self.max_active_tasks, 2, 1, 20)?,
            dedup_window_secs: in_range("dedup_window_secs", self.dedup_window_secs, 3_600, 60, 604_800)?,
            rearm_after_secs: in_range("rearm_after_secs", self.rearm_after_secs, 900, 0, 86_400)?,
            task_max_attempts: in_range("task_max_attempts", self.task_max_attempts, 3, 1, 10)?,
        })
    }
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::unwrap_used, clippy::indexing_slicing)]
mod tests {
    use super::*;
    use serde_json::json;

    fn input(extra: &Value) -> Result<ValidatedRule, String> {
        let mut body = json!({ "name": "WAF high" });
        if let (Some(base), Some(extra)) = (body.as_object_mut(), extra.as_object()) {
            base.extend(extra.clone());
        }
        serde_json::from_value::<RuleInput>(body)
            .map_err(|error| error.to_string())?
            .validate(Uuid::nil())
    }

    #[test]
    fn defaults_follow_the_documented_values() {
        let bot = Uuid::new_v4();
        let rule = input(&json!({ "bot_user_id": bot })).unwrap();
        assert!(rule.enabled);
        assert_eq!(rule.min_severity, Severity::High);
        assert_eq!(
            rule.action, ACTION_WORK_ITEM_AND_TASK,
            "high and above default to a task"
        );
        assert_eq!(
            (
                rule.max_tasks_per_hour,
                rule.max_active_tasks,
                rule.dedup_window_secs,
                rule.rearm_after_secs,
                rule.task_max_attempts
            ),
            (6, 2, 3_600, 900, 3)
        );
        assert_eq!(
            rule.notify_user_ids,
            vec![Uuid::nil()],
            "the author is the default recipient"
        );
        assert_eq!(rule.priority_map["critical"], "urgent");
        assert_eq!(rule.priority_map["info"], "low");
    }

    #[test]
    fn a_lower_threshold_defaults_to_opening_a_work_item_only() {
        let rule = input(&json!({ "min_severity": "medium" })).unwrap();
        assert_eq!(rule.action, ACTION_WORK_ITEM);
    }

    #[test]
    fn the_bot_goes_with_the_task_action_only() {
        assert!(
            input(&json!({ "action": "work_item_and_task" }))
                .unwrap_err()
                .contains("bot_user_id")
        );
        assert!(
            input(&json!({ "action": "work_item", "bot_user_id": Uuid::new_v4() }))
                .unwrap_err()
                .contains("bot_user_id")
        );
        assert!(input(&json!({ "action": "work_item" })).is_ok());
        assert!(input(&json!({ "action": "dispatch" })).unwrap_err().contains("action"));
    }

    #[test]
    fn every_limit_is_enforced_at_both_ends() {
        for (field, low, high) in [
            ("max_tasks_per_hour", 1, 120),
            ("max_active_tasks", 1, 20),
            ("dedup_window_secs", 60, 604_800),
            ("rearm_after_secs", 0, 86_400),
            ("task_max_attempts", 1, 10),
        ] {
            let ok = |value: i32| input(&json!({ "action": "work_item", field: value })).is_ok();
            assert!(ok(low) && ok(high), "{field}");
            assert!(!ok(low - 1) && !ok(high + 1), "{field}");
        }
    }

    #[test]
    fn name_instructions_and_patterns_are_bounded() {
        assert!(input(&json!({ "name": "  ", "action": "work_item" })).is_err());
        assert!(input(&json!({ "name": "n".repeat(101), "action": "work_item" })).is_err());
        assert!(input(&json!({ "instructions": "i".repeat(4001), "action": "work_item" })).is_err());
        assert!(input(&json!({ "instructions": "i".repeat(4000), "action": "work_item" })).is_ok());
        assert!(input(&json!({ "kind_patterns": ["waf.*", "sd.threat_detected"], "action": "work_item" })).is_ok());
        for bad in ["*", "WAF", "waf.**", ""] {
            assert!(
                input(&json!({ "kind_patterns": [bad], "action": "work_item" })).is_err(),
                "{bad}"
            );
        }
        let many: Vec<String> = (0..33).map(|i| format!("k{i}")).collect();
        assert!(input(&json!({ "kind_patterns": many, "action": "work_item" })).is_err());
    }

    #[test]
    fn the_priority_map_is_merged_over_the_defaults_and_checked() {
        let rule = input(&json!({ "action": "work_item", "priority_map": { "high": "urgent" } })).unwrap();
        assert_eq!(rule.priority_map["high"], "urgent");
        assert_eq!(rule.priority_map["medium"], "medium");
        for bad in [
            json!({ "huge": "low" }),
            json!({ "high": "blocker" }),
            json!({ "high": 1 }),
            json!(["x"]),
        ] {
            assert!(input(&json!({ "action": "work_item", "priority_map": bad })).is_err());
        }
    }

    #[test]
    fn recipients_are_deduplicated_and_unknown_fields_refused() {
        let id = Uuid::new_v4();
        let rule = input(&json!({ "action": "work_item", "notify_user_ids": [id, id] })).unwrap();
        assert_eq!(rule.notify_user_ids, vec![id]);
        let rule = input(&json!({ "action": "work_item", "notify_user_ids": [] })).unwrap();
        assert!(rule.notify_user_ids.is_empty(), "an explicit empty list stays empty");
        assert!(input(&json!({ "action": "work_item", "position": 3 })).is_err());
        assert!(input(&json!({ "action": "work_item", "workspace_id": Uuid::new_v4() })).is_err());
    }
}
