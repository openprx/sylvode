//! The hook for plugin field rules (contract C2 §7). Lane A defines it here; the plugin host
//! implements [`ExternalRuleResolver`] and registers it once at boot with [`set_resolver`].
//!
//! Until a resolver is registered every plugin rule fails closed: schema saves refuse a plugin
//! reference (`plugin_rule_unavailable`) and a write that reaches one is refused with
//! `installation_not_active`.

use std::sync::{Arc, OnceLock};

use futures_util::future::BoxFuture;
use serde_json::{Map, Value};
use uuid::Uuid;

/// The stage a plugin rule runs in (= WIT `rule-types.stage`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ExternalStage {
    Normalize,
    Validate,
    Transform,
}

impl ExternalStage {
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Normalize => "normalize",
            Self::Validate => "validate",
            Self::Transform => "transform",
        }
    }
}

/// The writer of a record value.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Writer {
    Create,
    Update,
    ChildCreate,
    Import,
    Recalculate,
    FlowConversion,
    Site,
    Preview,
}

/// A schema `plugin` reference.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExternalRuleRef {
    pub package_key: String,
    pub rule: String,
}

/// Who is writing. Stands in for C1's `Principal` until the catalogue lands; the plugin host maps
/// it (audit and deps filtering only).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RuleCaller {
    /// The user or bot id; `None` for a system writer.
    pub actor_id: Option<Uuid>,
    /// The caller's workspace role.
    pub role: String,
    pub is_bot: bool,
}

impl RuleCaller {
    /// The server itself (recalculation after a child write).
    #[must_use]
    pub fn system() -> Self {
        Self {
            actor_id: None,
            role: "system".to_string(),
            is_bot: false,
        }
    }
}

/// One plugin rule invocation.
pub struct ExternalRuleCall<'a> {
    pub stage: ExternalStage,
    pub writer: Writer,
    pub workspace_id: Uuid,
    pub project_id: Uuid,
    pub form_id: Uuid,
    pub record_id: Option<Uuid>,
    pub field_key: &'a str,
    pub field_type: &'static str,
    /// The normalised value of this field only.
    pub value: &'a Value,
    /// Only the keys listed in `deps`, read-allowed for the caller.
    pub deps: &'a Map<String, Value>,
    /// The schema's `config`.
    pub config: &'a Value,
    pub caller: &'a RuleCaller,
}

/// What a plugin rule returned (= WIT `rule-outcome` / `rule-violation`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ExternalRuleOutcome {
    Keep,
    Replace(Value),
    Violation { code: String },
}

/// A refusal of the plugin host; the code is a C1 §7 code (`installation_not_active`,
/// `plugin_deadline_exceeded`, `plugin_failed`, ...).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ExternalRuleError {
    Refused(&'static str),
}

impl ExternalRuleError {
    #[must_use]
    pub const fn code(&self) -> &'static str {
        match self {
            Self::Refused(code) => code,
        }
    }
}

pub trait ExternalRule: Send + Sync {
    fn apply<'a>(&'a self, call: ExternalRuleCall<'a>)
    -> BoxFuture<'a, Result<ExternalRuleOutcome, ExternalRuleError>>;
}

pub trait ExternalRuleResolver: Send + Sync {
    /// Schema save: an installation of the package is active in the project (or its workspace) and
    /// its active manifest declares the rule for this stage and field type.
    fn check(
        &self,
        workspace_id: Uuid,
        project_id: Uuid,
        rule: &ExternalRuleRef,
        stage: ExternalStage,
        field_type: &str,
    ) -> Result<(), ExternalRuleError>;

    fn resolve(
        &self,
        workspace_id: Uuid,
        project_id: Uuid,
        rule: &ExternalRuleRef,
    ) -> Result<Arc<dyn ExternalRule>, ExternalRuleError>;
}

/// The resolver before the plugin host registers one: every plugin rule is unavailable.
struct NoPluginRules;

impl ExternalRuleResolver for NoPluginRules {
    fn check(
        &self,
        _workspace_id: Uuid,
        _project_id: Uuid,
        _rule: &ExternalRuleRef,
        _stage: ExternalStage,
        _field_type: &str,
    ) -> Result<(), ExternalRuleError> {
        Err(ExternalRuleError::Refused("installation_not_active"))
    }

    fn resolve(
        &self,
        _workspace_id: Uuid,
        _project_id: Uuid,
        _rule: &ExternalRuleRef,
    ) -> Result<Arc<dyn ExternalRule>, ExternalRuleError> {
        Err(ExternalRuleError::Refused("installation_not_active"))
    }
}

static RESOLVER: OnceLock<Arc<dyn ExternalRuleResolver>> = OnceLock::new();

/// Registers the plugin host's resolver; the first registration wins. Returns `false` when one
/// was already registered.
pub fn set_resolver(resolver: Arc<dyn ExternalRuleResolver>) -> bool {
    RESOLVER.set(resolver).is_ok()
}

/// The registered resolver, or the refusing default.
#[must_use]
pub fn resolver() -> Arc<dyn ExternalRuleResolver> {
    RESOLVER
        .get()
        .cloned()
        .unwrap_or_else(|| Arc::new(NoPluginRules) as Arc<dyn ExternalRuleResolver>)
}

/// Whether a plugin violation code is well formed (`^[a-z0-9_]{1,64}$`).
#[must_use]
pub fn is_violation_code(code: &str) -> bool {
    (1..=64).contains(&code.len())
        && code
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'_')
}
