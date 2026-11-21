//! Alert rules: matching and the values a rule contributes.
//!
//! Rules of a project are evaluated in `position` order, then `id`. The first enabled rule that
//! matches the event's source, kind and severity handles it; exactly one rule handles an event.

use sea_orm::{ConnectionTrait, DbBackend, FromQueryResult, Statement};
use serde::Serialize;
use serde_json::Value;
use uuid::Uuid;

use super::{envelope::valid_kind, severity::Severity};

/// An alert rule row.
#[derive(Clone, Debug, FromQueryResult, Serialize)]
pub struct RuleRow {
    pub id: Uuid,
    pub workspace_id: Uuid,
    pub project_id: Uuid,
    pub name: String,
    pub position: i32,
    pub enabled: bool,
    pub source_id: Option<Uuid>,
    pub kind_patterns: Vec<String>,
    pub min_severity: String,
    pub action: String,
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
    pub version: i32,
}

/// Every column of [`RuleRow`], for statements that return it.
pub const RULE_COLUMNS: &str = "id, workspace_id, project_id, name, position, enabled, source_id, kind_patterns, \
     min_severity, action, bot_user_id, instructions, priority_map, label_ids, notify_user_ids, \
     max_tasks_per_hour, max_active_tasks, dedup_window_secs, rearm_after_secs, task_max_attempts, version";

/// The action of a rule that also hands the alert to a bot.
pub const ACTION_WORK_ITEM_AND_TASK: &str = "work_item_and_task";
/// The action of a rule that only opens a work item and notifies.
pub const ACTION_WORK_ITEM: &str = "work_item";

impl RuleRow {
    pub fn wants_task(&self) -> bool {
        self.action == ACTION_WORK_ITEM_AND_TASK
    }

    /// The work item priority for `severity`: the rule's map, or the default when the map lacks
    /// the severity or names a priority work items do not have.
    pub fn priority_for(&self, severity: Severity) -> String {
        self.priority_map
            .get(severity.as_str())
            .and_then(Value::as_str)
            .filter(|priority| matches!(*priority, "low" | "medium" | "high" | "urgent"))
            .unwrap_or_else(|| severity.default_work_item_priority())
            .to_owned()
    }
}

/// Whether a pattern is an exact kind or a trailing-wildcard prefix such as `waf.*`.
pub fn valid_kind_pattern(pattern: &str) -> bool {
    pattern
        .strip_suffix(".*")
        .map_or_else(|| valid_kind(pattern), valid_kind)
}

/// Whether `kind` matches one pattern: exact, or `prefix.*` for any kind that starts with
/// `prefix.`.
pub fn pattern_matches(pattern: &str, kind: &str) -> bool {
    pattern.strip_suffix(".*").map_or_else(
        || pattern == kind,
        |prefix| kind.strip_prefix(prefix).is_some_and(|rest| rest.starts_with('.')),
    )
}

/// An empty pattern list matches every kind.
pub fn kind_matches(patterns: &[String], kind: &str) -> bool {
    patterns.is_empty() || patterns.iter().any(|pattern| pattern_matches(pattern, kind))
}

/// Whether `rule` handles an event of `source_id`, `kind` and `severity`.
pub fn rule_matches(rule: &RuleRow, source_id: Uuid, kind: &str, severity: Severity) -> bool {
    rule.enabled
        && rule.source_id.is_none_or(|bound| bound == source_id)
        && kind_matches(&rule.kind_patterns, kind)
        && Severity::parse(&rule.min_severity).is_some_and(|minimum| severity >= minimum)
}

/// The first enabled rule of `project_id` that matches, in `position`, `id` order.
pub async fn first_matching_rule<C: ConnectionTrait>(
    db: &C,
    project_id: Uuid,
    source_id: Uuid,
    kind: &str,
    severity: Severity,
) -> Result<Option<RuleRow>, sea_orm::DbErr> {
    let candidates = RuleRow::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        format!(
            "SELECT {RULE_COLUMNS} FROM alert_rules \
              WHERE project_id = $1 AND enabled AND (source_id IS NULL OR source_id = $2) \
              ORDER BY position, id"
        ),
        vec![project_id.into(), source_id.into()],
    ))
    .all(db)
    .await?;
    Ok(candidates
        .into_iter()
        .find(|rule| rule_matches(rule, source_id, kind, severity)))
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::unwrap_used)]
mod tests {
    use super::*;
    use serde_json::json;

    fn rule() -> RuleRow {
        RuleRow {
            id: Uuid::new_v4(),
            workspace_id: Uuid::new_v4(),
            project_id: Uuid::new_v4(),
            name: "r".into(),
            position: 0,
            enabled: true,
            source_id: None,
            kind_patterns: Vec::new(),
            min_severity: "high".into(),
            action: ACTION_WORK_ITEM_AND_TASK.into(),
            bot_user_id: None,
            instructions: String::new(),
            priority_map: json!({"critical":"urgent","high":"high","medium":"medium","low":"low","info":"low"}),
            label_ids: Vec::new(),
            notify_user_ids: Vec::new(),
            max_tasks_per_hour: 6,
            max_active_tasks: 2,
            dedup_window_secs: 3600,
            rearm_after_secs: 900,
            task_max_attempts: 3,
            version: 1,
        }
    }

    #[test]
    fn patterns_are_exact_or_trailing_wildcards() {
        assert!(pattern_matches("waf.attack_detected", "waf.attack_detected"));
        assert!(!pattern_matches("waf.attack_detected", "waf.attack_detected.x"));
        assert!(pattern_matches("waf.*", "waf.attack_detected"));
        assert!(pattern_matches("waf.*", "waf.a.b"));
        assert!(
            !pattern_matches("waf.*", "waf"),
            "the prefix itself is not under the wildcard"
        );
        assert!(
            !pattern_matches("waf.*", "wafx.attack"),
            "the wildcard stops at a segment boundary"
        );
        assert!(!pattern_matches("waf.*", "sd.waf.attack"));
    }

    #[test]
    fn an_empty_list_matches_every_kind() {
        assert!(kind_matches(&[], "anything.at.all"));
        assert!(kind_matches(&["a".into(), "b.*".into()], "b.c"));
        assert!(!kind_matches(&["a".into(), "b.*".into()], "c"));
    }

    #[test]
    fn pattern_syntax_is_validated() {
        for good in ["waf", "waf.*", "waf.attack_detected", "a.b.c.*"] {
            assert!(valid_kind_pattern(good), "{good}");
        }
        for bad in ["*", ".*", "WAF.*", "waf.**", "waf*", "waf.*.x", ""] {
            assert!(!valid_kind_pattern(bad), "{bad}");
        }
    }

    #[test]
    fn severity_threshold_source_binding_and_enabled_all_apply() {
        let source = Uuid::new_v4();
        let mut r = rule();
        assert!(rule_matches(&r, source, "k", Severity::High));
        assert!(rule_matches(&r, source, "k", Severity::Critical));
        assert!(!rule_matches(&r, source, "k", Severity::Medium));
        r.source_id = Some(Uuid::new_v4());
        assert!(
            !rule_matches(&r, source, "k", Severity::High),
            "bound to another source"
        );
        r.source_id = Some(source);
        assert!(rule_matches(&r, source, "k", Severity::High));
        r.enabled = false;
        assert!(!rule_matches(&r, source, "k", Severity::High));
        r.enabled = true;
        r.min_severity = "bogus".into();
        assert!(
            !rule_matches(&r, source, "k", Severity::Critical),
            "a corrupt threshold matches nothing"
        );
    }

    #[test]
    fn priority_follows_the_map_with_a_safe_default() {
        let mut r = rule();
        assert_eq!(r.priority_for(Severity::Critical), "urgent");
        assert_eq!(r.priority_for(Severity::Info), "low");
        r.priority_map = json!({"high": "urgent", "medium": "nonsense"});
        assert_eq!(r.priority_for(Severity::High), "urgent");
        assert_eq!(
            r.priority_for(Severity::Medium),
            "medium",
            "an invalid priority falls back"
        );
        assert_eq!(r.priority_for(Severity::Low), "low", "a missing severity falls back");
    }
}
