//! `[inbound]` — the alert chain's inbound event endpoint and its safety limits.
//!
//! The section is optional and the endpoint is off by default: without `enabled = true` the
//! endpoint answers 503 to every caller. Every other key bounds one resource, so an operator can
//! tighten a limit but never remove it: each key has an accepted range.

use std::ops::RangeInclusive;

use serde::Deserialize;

use super::raw::Issues;

/// Largest request body the inbound endpoint reads, in bytes, unless `[inbound]` overrides it.
pub const DEFAULT_INBOUND_MAX_BODY_BYTES: u32 = 65_536;
/// Accepted range of `inbound.max_body_bytes`.
pub const INBOUND_MAX_BODY_BYTES_RANGE: RangeInclusive<u32> = 4_096..=262_144;
/// Seconds a signed request's timestamp may differ from the server clock unless overridden.
pub const DEFAULT_INBOUND_SIGNATURE_TOLERANCE_SECS: u32 = 300;
/// Accepted range of `inbound.signature_tolerance_secs`.
pub const INBOUND_SIGNATURE_TOLERANCE_RANGE_SECS: RangeInclusive<u32> = 30..=900;
/// Failed authentications one client address may make per minute unless overridden.
pub const DEFAULT_INBOUND_AUTH_FAILURES_PER_MINUTE: u32 = 30;
/// Accepted range of `inbound.auth_failures_per_minute_per_ip`.
pub const INBOUND_AUTH_FAILURES_RANGE: RangeInclusive<u32> = 1..=1_000;
/// Alert tasks that may be active in one workspace at once unless overridden.
pub const DEFAULT_INBOUND_MAX_ACTIVE_ALERT_TASKS: u32 = 5;
/// Accepted range of `inbound.max_active_alert_tasks_per_workspace`.
pub const INBOUND_MAX_ACTIVE_ALERT_TASKS_RANGE: RangeInclusive<u32> = 1..=100;
/// Work items one rule may open per hour unless overridden.
pub const DEFAULT_INBOUND_MAX_ALERTS_PER_RULE_PER_HOUR: u32 = 60;
/// Accepted range of `inbound.max_alerts_per_rule_per_hour`.
pub const INBOUND_MAX_ALERTS_PER_RULE_RANGE: RangeInclusive<u32> = 1..=10_000;
/// Days an inbound event is kept unless overridden.
pub const DEFAULT_INBOUND_EVENT_RETENTION_DAYS: u32 = 30;
/// Accepted range of `inbound.event_retention_days`.
pub const INBOUND_EVENT_RETENTION_RANGE_DAYS: RangeInclusive<u32> = 1..=365;
/// Seconds between two ticks of the worker's alert follow-up job unless overridden.
pub const DEFAULT_INBOUND_FOLLOWUP_INTERVAL_SECS: u32 = 2;
/// Accepted range of `inbound.followup_interval_secs`.
pub const INBOUND_FOLLOWUP_INTERVAL_RANGE_SECS: RangeInclusive<u32> = 1..=60;

/// The validated `[inbound]` section.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct InboundConfig {
    /// Whether the inbound endpoint answers at all.
    pub enabled: bool,
    pub max_body_bytes: u32,
    pub signature_tolerance_secs: u32,
    pub auth_failures_per_minute_per_ip: u32,
    pub max_active_alert_tasks_per_workspace: u32,
    pub max_alerts_per_rule_per_hour: u32,
    pub event_retention_days: u32,
    pub followup_interval_secs: u32,
}

impl Default for InboundConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            max_body_bytes: DEFAULT_INBOUND_MAX_BODY_BYTES,
            signature_tolerance_secs: DEFAULT_INBOUND_SIGNATURE_TOLERANCE_SECS,
            auth_failures_per_minute_per_ip: DEFAULT_INBOUND_AUTH_FAILURES_PER_MINUTE,
            max_active_alert_tasks_per_workspace: DEFAULT_INBOUND_MAX_ACTIVE_ALERT_TASKS,
            max_alerts_per_rule_per_hour: DEFAULT_INBOUND_MAX_ALERTS_PER_RULE_PER_HOUR,
            event_retention_days: DEFAULT_INBOUND_EVENT_RETENTION_DAYS,
            followup_interval_secs: DEFAULT_INBOUND_FOLLOWUP_INTERVAL_SECS,
        }
    }
}

#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub(super) struct RawInbound {
    enabled: Option<bool>,
    max_body_bytes: Option<u32>,
    signature_tolerance_secs: Option<u32>,
    auth_failures_per_minute_per_ip: Option<u32>,
    max_active_alert_tasks_per_workspace: Option<u32>,
    max_alerts_per_rule_per_hour: Option<u32>,
    event_retention_days: Option<u32>,
    followup_interval_secs: Option<u32>,
}

/// A value inside its range, or the default and an issue naming the key and the range.
fn ranged(issues: &mut Issues, key: &str, value: Option<u32>, range: &RangeInclusive<u32>, default: u32) -> u32 {
    value.map_or(default, |value| {
        if range.contains(&value) {
            value
        } else {
            issues.push(format!(
                "inbound.{key} must be between {} and {}",
                range.start(),
                range.end()
            ));
            default
        }
    })
}

impl RawInbound {
    pub(super) fn validate(self, issues: &mut Issues) -> InboundConfig {
        InboundConfig {
            enabled: self.enabled.unwrap_or(false),
            max_body_bytes: ranged(
                issues,
                "max_body_bytes",
                self.max_body_bytes,
                &INBOUND_MAX_BODY_BYTES_RANGE,
                DEFAULT_INBOUND_MAX_BODY_BYTES,
            ),
            signature_tolerance_secs: ranged(
                issues,
                "signature_tolerance_secs",
                self.signature_tolerance_secs,
                &INBOUND_SIGNATURE_TOLERANCE_RANGE_SECS,
                DEFAULT_INBOUND_SIGNATURE_TOLERANCE_SECS,
            ),
            auth_failures_per_minute_per_ip: ranged(
                issues,
                "auth_failures_per_minute_per_ip",
                self.auth_failures_per_minute_per_ip,
                &INBOUND_AUTH_FAILURES_RANGE,
                DEFAULT_INBOUND_AUTH_FAILURES_PER_MINUTE,
            ),
            max_active_alert_tasks_per_workspace: ranged(
                issues,
                "max_active_alert_tasks_per_workspace",
                self.max_active_alert_tasks_per_workspace,
                &INBOUND_MAX_ACTIVE_ALERT_TASKS_RANGE,
                DEFAULT_INBOUND_MAX_ACTIVE_ALERT_TASKS,
            ),
            max_alerts_per_rule_per_hour: ranged(
                issues,
                "max_alerts_per_rule_per_hour",
                self.max_alerts_per_rule_per_hour,
                &INBOUND_MAX_ALERTS_PER_RULE_RANGE,
                DEFAULT_INBOUND_MAX_ALERTS_PER_RULE_PER_HOUR,
            ),
            event_retention_days: ranged(
                issues,
                "event_retention_days",
                self.event_retention_days,
                &INBOUND_EVENT_RETENTION_RANGE_DAYS,
                DEFAULT_INBOUND_EVENT_RETENTION_DAYS,
            ),
            followup_interval_secs: ranged(
                issues,
                "followup_interval_secs",
                self.followup_interval_secs,
                &INBOUND_FOLLOWUP_INTERVAL_RANGE_SECS,
                DEFAULT_INBOUND_FOLLOWUP_INTERVAL_SECS,
            ),
        }
    }
}
