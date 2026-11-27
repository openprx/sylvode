//! Plugin field rules (contract C2 §7, C1 §4.1): the `ExternalRuleResolver` the field engine asks
//! for a `plugin` rule of a form schema, and the rule it returns.
//!
//! * [`RuleResolver::check`] answers a schema save and is synchronous, so it reads a small in-memory
//!   registry of the rules of active installations ([`refresh`] rebuilds it at start and after every
//!   change of an installation). A stale answer can only let a schema name a rule that has just
//!   gone: the write that reaches it fails closed, because
//! * [`InstalledRule::apply`] never trusts the registry: it loads the installation, its active
//!   generation and the rule's component again, runs the component in the `field-rule` world (no
//!   imports, 50 ms, 2e6 fuel, 8 MiB) and maps the result.
//!
//! A violation code `c` of the guest reaches users as `plugin.<package_key>.<c>` (the field engine
//! builds the name); a refusal of the host is the C1 code itself.

use std::collections::{BTreeSet, HashMap};
use std::sync::{Arc, LazyLock};

use futures_util::future::BoxFuture;
use parking_lot::RwLock;
use platform::app::AppState;
use sea_orm::{ConnectionTrait, DbBackend, FromQueryResult, Statement};
use serde_json::{Value, json};
use sv_package::RuleEntry;
use uuid::Uuid;

use super::host::engine::{compile_cache, engine};
use super::host::{World, invoke};
use super::install::live::{self, LoadError};
use crate::catalogue::{InstallationView, Principal, RefusalCode};
use crate::forms::rules::external::{
    ExternalRule, ExternalRuleCall, ExternalRuleError, ExternalRuleOutcome, ExternalRuleRef, ExternalRuleResolver,
    ExternalStage, set_resolver,
};
use crate::packages::open;

/// The rules one active installation provides.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Active {
    workspace_id: Uuid,
    project_id: Option<Uuid>,
    package_key: String,
    rules: Vec<RuleEntry>,
}

static REGISTRY: LazyLock<RwLock<Vec<Active>>> = LazyLock::new(|| RwLock::new(Vec::new()));

#[derive(FromQueryResult)]
struct Row {
    workspace_id: Uuid,
    project_id: Option<Uuid>,
    package_key: String,
    rules: Value,
}

/// Rebuilds the registry from the database: the rules declared by the package of the active
/// generation of every active installation.
///
/// # Errors
/// A database failure; the registry is left as it was.
pub async fn refresh(db: &impl ConnectionTrait) -> Result<(), sea_orm::DbErr> {
    let rows = Row::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "SELECT i.workspace_id, i.project_id, i.package_key, COALESCE(p.manifest->'rules', '[]'::jsonb) AS rules \
         FROM installations i \
         JOIN installation_generations g ON g.installation_id = i.id AND g.generation = i.active_generation \
              AND g.status = 'approved' \
         JOIN packages p ON p.id = g.package_row_id \
         WHERE i.status = 'active'",
        vec![],
    ))
    .all(db)
    .await?;
    let active = rows
        .into_iter()
        .map(|row| Active {
            workspace_id: row.workspace_id,
            project_id: row.project_id,
            package_key: row.package_key,
            rules: serde_json::from_value(row.rules).unwrap_or_default(),
        })
        .filter(|active| !active.rules.is_empty())
        .collect();
    *REGISTRY.write() = active;
    Ok(())
}

/// Lets a schema that names a rule of the package being approved pass its save check while the
/// approval creates the package's forms.
///
/// The entry lives until the next [`refresh`], which the approval runs whether it commits or not.
pub fn stage(workspace_id: Uuid, project_id: Option<Uuid>, package_key: &str, rules: &[RuleEntry]) {
    let mut registry = REGISTRY.write();
    registry.retain(|active| {
        !(active.workspace_id == workspace_id && active.project_id == project_id && active.package_key == package_key)
    });
    registry.push(Active {
        workspace_id,
        project_id,
        package_key: package_key.to_owned(),
        rules: rules.to_vec(),
    });
}

fn declares(rules: &[RuleEntry], name: &str, stage: ExternalStage, field_type: &str) -> bool {
    rules.iter().any(|rule| {
        rule.name == name
            && rule.stages.iter().any(|candidate| candidate == stage.name())
            && rule.field_types.iter().any(|candidate| candidate == field_type)
    })
}

/// The installation scope that serves a form of `project_id`: the project's own installation first,
/// then the workspace's.
fn providers(workspace_id: Uuid, project_id: Uuid, package_key: &str) -> Vec<Active> {
    let registry = REGISTRY.read();
    let mut found: Vec<Active> = registry
        .iter()
        .filter(|active| {
            active.workspace_id == workspace_id
                && active.package_key == package_key
                && active.project_id.is_none_or(|project| project == project_id)
        })
        .cloned()
        .collect();
    drop(registry);
    found.sort_by_key(|active| active.project_id.is_none());
    found
}

/// The resolver registered with the field engine.
pub struct RuleResolver {
    state: AppState,
}

impl ExternalRuleResolver for RuleResolver {
    fn check(
        &self,
        workspace_id: Uuid,
        project_id: Uuid,
        rule: &ExternalRuleRef,
        stage: ExternalStage,
        field_type: &str,
    ) -> Result<(), ExternalRuleError> {
        if providers(workspace_id, project_id, &rule.package_key)
            .iter()
            .any(|active| declares(&active.rules, &rule.rule, stage, field_type))
        {
            Ok(())
        } else {
            Err(ExternalRuleError::Refused(RefusalCode::InstallationNotActive.as_str()))
        }
    }

    fn resolve(
        &self,
        workspace_id: Uuid,
        project_id: Uuid,
        rule: &ExternalRuleRef,
    ) -> Result<Arc<dyn ExternalRule>, ExternalRuleError> {
        if providers(workspace_id, project_id, &rule.package_key).is_empty() {
            return Err(ExternalRuleError::Refused(RefusalCode::InstallationNotActive.as_str()));
        }
        Ok(Arc::new(InstalledRule {
            state: self.state.clone(),
            reference: rule.clone(),
        }))
    }
}

/// Registers the resolver with the field engine and fills the registry. Called once by `main`.
///
/// # Errors
/// A database failure.
pub async fn register(state: AppState) -> Result<(), sea_orm::DbErr> {
    refresh(&state.db).await?;
    if !set_resolver(Arc::new(RuleResolver { state })) {
        tracing::warn!(target: "sv::extensions", "a plugin rule resolver was already registered");
    }
    Ok(())
}

/// A rule of an installed package, resolved for one write. It does no database work: everything it
/// needs was loaded by [`PreparedRules::prepare`] before the writer's transaction opened.
struct InstalledRule {
    state: AppState,
    reference: ExternalRuleRef,
}

/// The plugin rules of one write, loaded before its transaction opens.
///
/// A writer calls [`PreparedRules::prepare`] with the form's schema before `begin()` and runs the
/// write inside [`PreparedRules::scope`]. For every `plugin` rule the schema names it loads the
/// installation and its active generation, the rule's declaration and the component bytes, and warms
/// the compile cache, so that [`InstalledRule::apply`] needs no pooled connection and no compile while
/// the transaction is open. A rule that was not prepared (a writer that does not prepare) refuses
/// `plugin_failed`: it never falls back to I/O. An installation that was disabled between the prepare
/// and the apply is the same race as a disable concurrent with the write.
#[derive(Clone, Default)]
pub struct PreparedRules {
    entries: Arc<HashMap<(String, String), Prepared>>,
}

enum Prepared {
    Ready(Box<Ready>),
    Refused(RefusalCode),
}

struct Ready {
    installation: InstallationView,
    approver: Option<Uuid>,
    bytes: Arc<Vec<u8>>,
    declared: RuleEntry,
}

tokio::task_local! {
    static PREPARED: PreparedRules;
}

/// The `(package, rule)` pairs a schema's field rules name.
fn plugin_refs(schema: &Value) -> BTreeSet<(String, String)> {
    let mut found = BTreeSet::new();
    let fields = schema.get("fields").and_then(Value::as_array).into_iter().flatten();
    for stages in fields.filter_map(|field| field.get("rules").and_then(Value::as_object)) {
        for rule in stages.values().filter_map(Value::as_array).flatten() {
            let plugin = rule.get("plugin");
            let text = |key: &str| plugin?.get(key)?.as_str().map(str::to_owned);
            if let (Some(package), Some(name)) = (text("package"), text("rule")) {
                found.insert((package, name));
            }
        }
    }
    found
}

impl PreparedRules {
    /// Loads what the plugin rules of `schema` need, for a write in `project_id`.
    ///
    /// # Errors
    /// Never for a rule that cannot run (that rule is recorded as refused); only for a failure that
    /// the caller should see as a server error.
    pub async fn prepare(
        state: &AppState,
        workspace_id: Uuid,
        project_id: Uuid,
        schema: &Value,
    ) -> Result<Self, crate::error::ApiError> {
        let mut entries = HashMap::new();
        for (package, name) in plugin_refs(schema) {
            let entry = load_one(state, workspace_id, project_id, &package, &name).await;
            entries.insert((package, name), entry);
        }
        Ok(Self {
            entries: Arc::new(entries),
        })
    }

    /// Prepares the rules of `schema` and runs `future` with them; for the writers and previews that
    /// open no transaction of their own.
    ///
    /// # Errors
    /// As [`Self::prepare`].
    pub async fn around<F: std::future::Future>(
        state: &AppState,
        workspace_id: Uuid,
        project_id: Uuid,
        schema: &Value,
        future: F,
    ) -> Result<F::Output, crate::error::ApiError> {
        Ok(Self::prepare(state, workspace_id, project_id, schema)
            .await?
            .scope(future)
            .await)
    }

    /// Runs `future` with these rules available to `InstalledRule::apply`.
    pub async fn scope<F: std::future::Future>(self, future: F) -> F::Output {
        PREPARED.scope(self, future).await
    }
}

async fn load_one(state: &AppState, workspace_id: Uuid, project_id: Uuid, package: &str, name: &str) -> Prepared {
    let db = &state.db;
    let failed = Prepared::Refused(RefusalCode::PluginFailed);
    if providers(workspace_id, project_id, package).is_empty() {
        return Prepared::Refused(RefusalCode::InstallationNotActive);
    }
    let Ok(found) = find_installation(db, workspace_id, project_id, package).await else {
        return failed;
    };
    let Some((installation_id, _)) = found else {
        return Prepared::Refused(RefusalCode::InstallationNotActive);
    };
    let loaded = match live::load(db, installation_id).await {
        Ok(loaded) => loaded,
        Err(LoadError::Refused(code)) => return Prepared::Refused(code),
        Err(LoadError::Unavailable) => return failed,
    };
    let Ok(rules) = manifest_rules(db, loaded.package_row_id).await else {
        return failed;
    };
    let Some(declared) = rules.into_iter().find(|rule| rule.name == name) else {
        return Prepared::Refused(RefusalCode::PluginActionUnknown);
    };
    let Ok(Some(stored)) = open::find_by_row(db, workspace_id, loaded.package_row_id).await else {
        return Prepared::Refused(RefusalCode::InstallationNotActive);
    };
    let Ok(bytes) = open::component_bytes(db, workspace_id, &stored, &declared.component).await else {
        return Prepared::Refused(RefusalCode::PluginInvalidComponent);
    };
    // Compile now, outside any transaction; the cache serves the run.
    let compiled = match engine() {
        Ok(engine) => compile_cache().get(engine, &bytes).await.is_ok(),
        Err(_) => false,
    };
    if !compiled {
        return Prepared::Refused(RefusalCode::PluginInvalidComponent);
    }
    Prepared::Ready(Box::new(Ready {
        installation: loaded.view,
        approver: loaded.approver,
        bytes,
        declared,
    }))
}

/// The active installation of the package that serves `project_id`, project scope first.
async fn find_installation(
    db: &impl ConnectionTrait,
    workspace_id: Uuid,
    project_id: Uuid,
    package_key: &str,
) -> Result<Option<(Uuid, String)>, sea_orm::DbErr> {
    let row = db
        .query_one(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT id, status FROM installations \
             WHERE workspace_id = $1 AND package_key = $2 AND status <> 'revoked' \
               AND (project_id IS NULL OR project_id = $3) \
             ORDER BY (status = 'active') DESC, (project_id IS NULL), created_at LIMIT 1",
            vec![workspace_id.into(), package_key.to_owned().into(), project_id.into()],
        ))
        .await?;
    Ok(row.and_then(|row| Some((row.try_get("", "id").ok()?, row.try_get("", "status").ok()?))))
}

const fn refused(code: RefusalCode) -> ExternalRuleError {
    ExternalRuleError::Refused(code.as_str())
}

impl ExternalRule for InstalledRule {
    fn apply<'a>(
        &'a self,
        call: ExternalRuleCall<'a>,
    ) -> BoxFuture<'a, Result<ExternalRuleOutcome, ExternalRuleError>> {
        Box::pin(async move {
            let key = (self.reference.package_key.clone(), self.reference.rule.clone());
            // Fail closed: a rule that was not prepared never loads anything here.
            let prepared = PREPARED
                .try_with(|rules| Arc::clone(&rules.entries))
                .map_err(|_| refused(RefusalCode::PluginFailed))?;
            let ready = match prepared.get(&key) {
                Some(Prepared::Ready(ready)) => ready,
                Some(Prepared::Refused(code)) => return Err(refused(*code)),
                None => return Err(refused(RefusalCode::PluginFailed)),
            };
            let declared = &ready.declared;
            if !declared.stages.iter().any(|stage| stage == call.stage.name())
                || !declared
                    .field_types
                    .iter()
                    .any(|field_type| field_type == call.field_type)
            {
                return Err(refused(RefusalCode::PluginActionUnknown));
            }
            let input = json!({
                "stage": call.stage.name(),
                "field_key": call.field_key,
                "field_type": call.field_type,
                "value": call.value,
                "deps": call.deps.iter().map(|(key, value)| json!([key, value])).collect::<Vec<_>>(),
                "config": call.config,
            })
            .to_string();
            let principal = rule_principal(ready.installation.clone(), ready.approver);
            let outcome = invoke(
                &self.state,
                &principal,
                &ready.bytes,
                World::FieldRule,
                call.stage.name(),
                &input,
            )
            .await;
            match outcome.result {
                Ok(value) => Ok(value.get("replace").map_or(ExternalRuleOutcome::Keep, |replacement| {
                    ExternalRuleOutcome::Replace(replacement.clone())
                })),
                Err(error) => match error.guest_code {
                    Some(code) if error.code == RefusalCode::PluginGuestError => {
                        Ok(ExternalRuleOutcome::Violation { code })
                    }
                    _ => Err(refused(error.code)),
                },
            }
        })
    }
}

/// A rule has no host calls and no user: it runs as the installation, which only names the
/// installation and generation in the invocation log.
fn rule_principal(installation: InstallationView, approver: Option<Uuid>) -> Principal {
    Principal::Installation {
        installation,
        approver_user_id: approver.unwrap_or_else(Uuid::nil),
    }
}

async fn manifest_rules(db: &impl ConnectionTrait, package_row_id: Uuid) -> Result<Vec<RuleEntry>, ExternalRuleError> {
    let row = db
        .query_one(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT COALESCE(manifest->'rules', '[]'::jsonb) AS rules FROM packages WHERE id = $1",
            vec![package_row_id.into()],
        ))
        .await
        .map_err(|_| refused(RefusalCode::PluginFailed))?
        .ok_or_else(|| refused(RefusalCode::InstallationNotActive))?;
    let value: Value = row
        .try_get("", "rules")
        .map_err(|_| refused(RefusalCode::PluginFailed))?;
    serde_json::from_value(value).map_err(|_| refused(RefusalCode::PluginFailed))
}

#[cfg(test)]
mod tests {
    use sv_package::RuleEntry;
    use uuid::Uuid;

    use serde_json::{Map, json};

    use super::{InstalledRule, PreparedRules, declares, plugin_refs, providers, stage};
    use crate::forms::rules::external::ExternalStage;
    use crate::forms::rules::external::{
        ExternalRule, ExternalRuleCall, ExternalRuleError, ExternalRuleRef, RuleCaller, Writer,
    };

    fn rule() -> RuleEntry {
        RuleEntry {
            name: "reference_code".to_string(),
            world: "field-rule".to_string(),
            component: "contents/components/rules.wasm".to_string(),
            stages: vec!["validate".to_string()],
            field_types: vec!["text".to_string()],
            config_schema: None,
        }
    }

    #[test]
    fn a_rule_is_declared_for_its_stages_and_types_only() {
        let rules = [rule()];
        assert!(declares(&rules, "reference_code", ExternalStage::Validate, "text"));
        assert!(!declares(&rules, "reference_code", ExternalStage::Normalize, "text"));
        assert!(!declares(&rules, "reference_code", ExternalStage::Validate, "number"));
        assert!(!declares(&rules, "other", ExternalStage::Validate, "text"));
    }

    /// A project's own installation is listed before the workspace's, and another project's is not.
    #[test]
    fn providers_are_scoped_and_ordered() {
        let (workspace, project, other) = (Uuid::new_v4(), Uuid::new_v4(), Uuid::new_v4());
        let key = format!("acme/{}", Uuid::new_v4().simple());
        stage(workspace, None, &key, &[rule()]);
        stage(workspace, Some(project), &key, &[rule()]);
        stage(workspace, Some(other), &key, &[rule()]);
        let found = providers(workspace, project, &key);
        assert_eq!(found.len(), 2);
        assert_eq!(found.first().and_then(|active| active.project_id), Some(project));
        assert_eq!(found.last().and_then(|active| active.project_id), None);
    }

    #[test]
    fn the_plugin_rules_of_a_schema_are_found_at_every_stage() {
        let schema = json!({"fields": [
            {"key": "a", "rules": {"validate": [{"rule": "min", "value": 1}, {"plugin": {"package": "qa/x", "rule": "r"}}],
                                    "normalize": [{"plugin": {"package": "qa/x", "rule": "r"}}]}},
            {"key": "b", "rules": {"transform": [{"plugin": {"package": "qa/y", "rule": "s"}}]}},
            {"key": "c"}
        ]});
        let found: Vec<_> = plugin_refs(&schema).into_iter().collect();
        assert_eq!(
            found,
            [
                ("qa/x".to_string(), "r".to_string()),
                ("qa/y".to_string(), "s".to_string())
            ]
        );
    }

    /// A rule that was not prepared never loads anything: it refuses `plugin_failed`, with or without
    /// a prepared set in scope.
    #[tokio::test]
    async fn an_unprepared_rule_fails_closed() {
        let state = crate::extensions::host::test_state();
        let rule = InstalledRule {
            state,
            reference: ExternalRuleRef {
                package_key: "qa/x".to_string(),
                rule: "r".to_string(),
            },
        };
        let (value, deps, config) = (json!("v"), Map::new(), json!(null));
        let caller = RuleCaller::system();
        let call = || ExternalRuleCall {
            stage: crate::forms::rules::external::ExternalStage::Validate,
            writer: Writer::Create,
            workspace_id: Uuid::nil(),
            project_id: Uuid::nil(),
            form_id: Uuid::nil(),
            record_id: None,
            field_key: "f",
            field_type: "text",
            value: &value,
            deps: &deps,
            config: &config,
            caller: &caller,
        };
        let refused = ExternalRuleError::Refused("plugin_failed");
        assert_eq!(rule.apply(call()).await.expect_err("no scope"), refused);
        let empty = PreparedRules::default();
        assert_eq!(empty.scope(rule.apply(call())).await.expect_err("no entry"), refused);
    }
}
