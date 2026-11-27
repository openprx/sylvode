//! A schema's write plan: per field, its descriptor, `read_only`, `default`, the parsed rule
//! stages and the mask (contract C2 §2).
//!
//! Plans are parsed in one of two modes. A schema save parses strictly and reports every problem
//! (`schema_invalid`). A write parses tolerantly: an entry that cannot hold is skipped and logged by
//! field key and code (never its content), so a stored schema that predates a check never makes its
//! form unwritable. Tolerant plans are compiled once per schema content and cached.

use std::sync::{Arc, LazyLock};

use hashlink::LruCache;
use parking_lot::Mutex;
use serde_json::{Map, Value};
use sha2::{Digest, Sha256};
use sv_rules::{Bound, BuiltinRule, PluginRuleRef, RuleSpec, RuleSpecKind, parse_rule};
use uuid::Uuid;

use super::{
    external::{ExternalRuleRef, ExternalStage, resolver},
    ops::BuiltinOp,
    report::{SchemaIssue, SchemaIssues},
};
use crate::forms::{
    schema::{FormField, parse_field_at},
    types::{FieldType, MaskRule, Outlet, OutletRule, field_type},
};

/// The schema version every stored schema carries after migration 0136.
pub const SCHEMA_VERSION: &str = "sv.form.schema.v2";

const MAX_SCHEMA_BYTES: usize = 512 * 1024;
const MAX_NORMALIZE: usize = 8;
const MAX_VALIDATE: usize = 16;
const MAX_TRANSFORM: usize = 4;
const MAX_CEL_PER_FORM: usize = 64;
const MAX_PLUGIN_REFS_PER_FORM: usize = 16;
const MAX_MASK_KEEP: u64 = 16;
const REMOVED_KEYS: &[&str] = &["validation", "default_value", "policy"];
const PLAN_CACHE_CAPACITY: usize = 256;

/// Where a schema lives, for the plugin rule checks at schema save.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SchemaScope {
    pub workspace_id: Uuid,
    pub project_id: Uuid,
}

/// A field's `default`: a fixed value, or a CEL expression over `now` and `user` evaluated at
/// create time.
#[derive(Debug)]
pub enum DefaultSpec {
    Value(Value),
    Cel(sv_rules::Program),
}

/// One entry of `rules.normalize` or `rules.transform`.
#[derive(Clone, Debug)]
pub enum OpStep {
    Builtin {
        index: usize,
        op: BuiltinOp,
    },
    Plugin {
        index: usize,
        rule: PluginRuleRef,
        run_on_empty: bool,
    },
}

/// One entry of `rules.validate`.
#[derive(Debug)]
pub struct ValidateStep {
    pub index: usize,
    pub spec: RuleSpec,
}

/// The plan of one field.
pub struct FieldPlan {
    pub field: FormField,
    pub ty: &'static dyn FieldType,
    pub read_only: bool,
    pub default: Option<DefaultSpec>,
    pub normalize: Vec<OpStep>,
    pub validate: Vec<ValidateStep>,
    pub transform: Vec<OpStep>,
    pub mask: Option<MaskRule>,
    /// Written by the server (a formula, autonumber, AI summary).
    pub derived: bool,
}

impl FieldPlan {
    /// Whether the field carries any rule stage.
    pub const fn has_rules(&self) -> bool {
        !self.normalize.is_empty() || !self.validate.is_empty() || !self.transform.is_empty()
    }
}

/// The plan of one schema, in field order.
pub struct FormPlan {
    pub fields: Vec<FieldPlan>,
}

impl FormPlan {
    pub fn field(&self, key: &str) -> Option<&FieldPlan> {
        self.fields.iter().find(|plan| plan.field.key == key)
    }
}

static PLAN_CACHE: LazyLock<Mutex<LruCache<[u8; 32], Arc<FormPlan>>>> =
    LazyLock::new(|| Mutex::new(LruCache::new(PLAN_CACHE_CAPACITY)));

/// The tolerant write plan of a stored schema, compiled once per schema content.
pub fn write_plan(schema: &Value) -> Result<Arc<FormPlan>, String> {
    let bytes = serde_json::to_vec(schema).map_err(|_| "schema is not serialisable".to_string())?;
    let digest: [u8; 32] = Sha256::digest(bytes).into();
    if let Some(plan) = PLAN_CACHE.lock().get(&digest) {
        return Ok(Arc::clone(plan));
    }
    let mut issues = SchemaIssues::default();
    let plan = Arc::new(parse_plan(schema, Mode::Tolerant, &mut issues)?);
    for issue in &issues.errors {
        tracing::warn!(path = %issue.path, code = issue.code, "stored form schema entry is invalid; skipped on write");
    }
    PLAN_CACHE.lock().insert(digest, Arc::clone(&plan));
    Ok(plan)
}

/// Checks a schema for a save: version, size, removed keys, field definitions, rules, defaults,
/// masks and plugin references. Field structure errors are reported too.
pub fn check_schema(schema: &Value, scope: Option<SchemaScope>) -> SchemaIssues {
    let mut issues = SchemaIssues::default();
    let Some(object) = schema.as_object() else {
        issues
            .errors
            .push(SchemaIssue::new("", "field_invalid", "schema must be a JSON object"));
        return issues;
    };
    if object.get("version").and_then(Value::as_str) != Some(SCHEMA_VERSION) {
        issues.errors.push(SchemaIssue::new(
            "/version",
            "schema_version_unsupported",
            format!("schema version must be {SCHEMA_VERSION}"),
        ));
    }
    if serde_json::to_vec(schema).map_or(true, |bytes| bytes.len() > MAX_SCHEMA_BYTES) {
        issues
            .errors
            .push(SchemaIssue::new("", "limit_exceeded", "schema is larger than 512 KiB"));
        return issues;
    }
    if let Err(detail) = parse_plan(schema, Mode::Strict(scope), &mut issues) {
        // `parse_plan` only fails on the shape of `fields`; the issue is already recorded.
        tracing::debug!(%detail, "schema check stopped early");
    }
    issues
}

#[derive(Clone, Copy)]
enum Mode {
    Strict(Option<SchemaScope>),
    Tolerant,
}

struct Counters {
    cel: usize,
    plugins: usize,
}

fn parse_plan(schema: &Value, mode: Mode, issues: &mut SchemaIssues) -> Result<FormPlan, String> {
    let raw_fields = match schema.get("fields") {
        None => return Ok(FormPlan { fields: Vec::new() }),
        Some(Value::Array(items)) => items,
        Some(_) => {
            issues.errors.push(SchemaIssue::new(
                "/fields",
                "field_invalid",
                "schema.fields must be an array",
            ));
            return Err("schema.fields must be an array".to_string());
        }
    };
    let mut fields = Vec::with_capacity(raw_fields.len());
    let mut objects = Vec::with_capacity(raw_fields.len());
    let mut structure_failed = false;
    for (index, raw) in raw_fields.iter().enumerate() {
        match parse_field_at(raw) {
            Ok(field) => {
                fields.push(field);
                objects.push((index, raw.as_object().cloned().unwrap_or_default()));
            }
            Err(error) => {
                if matches!(mode, Mode::Tolerant) {
                    return Err(error.detail);
                }
                structure_failed = true;
                issues.errors.push(SchemaIssue::new(
                    format!("/fields/{index}{}", error.path),
                    error.code,
                    error.detail,
                ));
            }
        }
    }
    if matches!(mode, Mode::Strict(_)) {
        check_field_set(&fields, &objects, issues);
    }
    if structure_failed {
        return Ok(FormPlan { fields: Vec::new() });
    }
    let mut counters = Counters { cel: 0, plugins: 0 };
    let mut plans = Vec::with_capacity(fields.len());
    for (field, (index, object)) in fields.iter().zip(&objects) {
        let base = format!("/fields/{index}");
        let ty = field_type(&field.field_type).ok_or_else(|| "unsupported field type".to_string())?;
        let plan = parse_field_plan(field.clone(), ty, &base, object, &fields, mode, &mut counters, issues);
        plans.push(plan);
    }
    if matches!(mode, Mode::Strict(_)) {
        if counters.cel > MAX_CEL_PER_FORM {
            issues.errors.push(SchemaIssue::new(
                "/fields",
                "limit_exceeded",
                "more than 64 CEL expressions",
            ));
        }
        if counters.plugins > MAX_PLUGIN_REFS_PER_FORM {
            issues.errors.push(SchemaIssue::new(
                "/fields",
                "limit_exceeded",
                "more than 16 plugin rules",
            ));
        }
    }
    Ok(FormPlan { fields: plans })
}

/// Duplicate ids and keys, and the descriptors' cross-field checks.
fn check_field_set(fields: &[FormField], objects: &[(usize, Map<String, Value>)], issues: &mut SchemaIssues) {
    let mut ids = std::collections::BTreeSet::new();
    let mut keys = std::collections::BTreeSet::new();
    for (field, (index, _)) in fields.iter().zip(objects) {
        if !ids.insert(field.field_id.as_str()) {
            issues.errors.push(SchemaIssue::new(
                format!("/fields/{index}/field_id"),
                "field_duplicate",
                format!("duplicate field_id: {}", field.field_id),
            ));
        }
        if !keys.insert(field.key.as_str()) {
            issues.errors.push(SchemaIssue::new(
                format!("/fields/{index}/key"),
                "field_duplicate",
                format!("duplicate field key: {}", field.key),
            ));
        }
        if let Some(ty) = field_type(&field.field_type)
            && let Err(error) = ty.validate_definition(field, fields)
        {
            issues.errors.push(SchemaIssue::new(
                format!("/fields/{index}{}", error.path),
                error.code,
                error.detail,
            ));
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn parse_field_plan(
    field: FormField,
    ty: &'static dyn FieldType,
    base: &str,
    object: &Map<String, Value>,
    all: &[FormField],
    mode: Mode,
    counters: &mut Counters,
    issues: &mut SchemaIssues,
) -> FieldPlan {
    let strict = matches!(mode, Mode::Strict(_));
    let mut issue = |path: String, code: &'static str, detail: &str| {
        issues.errors.push(SchemaIssue::new(path, code, detail));
    };
    if strict {
        for key in REMOVED_KEYS {
            if object.contains_key(*key) {
                issue(
                    format!("{base}/{key}"),
                    "schema_key_removed",
                    "key removed in schema v2",
                );
            }
        }
    }
    let read_only = match object.get("read_only") {
        None | Some(Value::Null) => false,
        Some(Value::Bool(flag)) => *flag,
        Some(_) => {
            issue(
                format!("{base}/read_only"),
                "field_invalid",
                "read_only must be a boolean",
            );
            false
        }
    };
    let derived = object.contains_key("formula") || ty.server_written();
    let mut plan = FieldPlan {
        field,
        ty,
        read_only,
        default: None,
        normalize: Vec::new(),
        validate: Vec::new(),
        transform: Vec::new(),
        mask: None,
        derived,
    };
    match object.get("rules") {
        None | Some(Value::Null) => {}
        Some(Value::Object(rules)) => {
            parse_rules(&mut plan, base, rules, all, mode, counters, issues);
        }
        Some(_) => issues.errors.push(SchemaIssue::new(
            format!("{base}/rules"),
            "rule_param_invalid",
            "rules must be an object",
        )),
    }
    plan.default = parse_default(&plan, base, object.get("default"), counters, issues);
    plan.mask = parse_mask(&plan, base, object.get("mask"), issues);
    plan
}

fn parse_rules(
    plan: &mut FieldPlan,
    base: &str,
    rules: &Map<String, Value>,
    all: &[FormField],
    mode: Mode,
    counters: &mut Counters,
    issues: &mut SchemaIssues,
) {
    for key in rules.keys() {
        if !matches!(key.as_str(), "normalize" | "validate" | "transform") {
            issues.errors.push(SchemaIssue::new(
                format!("{base}/rules/{key}"),
                "rule_param_invalid",
                "unknown rules stage",
            ));
        }
    }
    let stage_entries = |stage: &str, limit: usize, issues: &mut SchemaIssues| -> Vec<(usize, Value)> {
        match rules.get(stage) {
            None | Some(Value::Null) => Vec::new(),
            Some(Value::Array(items)) => {
                if items.len() > limit {
                    issues.errors.push(SchemaIssue::new(
                        format!("{base}/rules/{stage}"),
                        "limit_exceeded",
                        "too many entries in this stage",
                    ));
                }
                items.iter().take(limit).cloned().enumerate().collect()
            }
            Some(_) => {
                issues.errors.push(SchemaIssue::new(
                    format!("{base}/rules/{stage}"),
                    "rule_param_invalid",
                    "a rules stage must be an array",
                ));
                Vec::new()
            }
        }
    };
    for (index, entry) in stage_entries("normalize", MAX_NORMALIZE, issues) {
        let path = format!("{base}/rules/normalize/{index}");
        if let Some(step) = parse_op_step(
            plan,
            &path,
            index,
            &entry,
            ExternalStage::Normalize,
            all,
            mode,
            counters,
            issues,
        ) {
            plan.normalize.push(step);
        }
    }
    for (index, entry) in stage_entries("validate", MAX_VALIDATE, issues) {
        let path = format!("{base}/rules/validate/{index}");
        match parse_rule(&entry, plan.ty.allowed_rules()) {
            Ok(spec) => {
                if let Err(issue) = check_validate_spec(plan, &path, &spec, all, mode, counters) {
                    issues.errors.push(issue);
                } else {
                    plan.validate.push(ValidateStep { index, spec });
                }
            }
            Err(error) => issues
                .errors
                .push(SchemaIssue::new(format!("{path}{}", error.path), error.code, "")),
        }
    }
    check_bounds_order(plan, base, issues);
    for (index, entry) in stage_entries("transform", MAX_TRANSFORM, issues) {
        let path = format!("{base}/rules/transform/{index}");
        if let Some(step) = parse_op_step(
            plan,
            &path,
            index,
            &entry,
            ExternalStage::Transform,
            all,
            mode,
            counters,
            issues,
        ) {
            plan.transform.push(step);
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn parse_op_step(
    plan: &FieldPlan,
    path: &str,
    index: usize,
    entry: &Value,
    stage: ExternalStage,
    all: &[FormField],
    mode: Mode,
    counters: &mut Counters,
    issues: &mut SchemaIssues,
) -> Option<OpStep> {
    let Some(object) = entry.as_object() else {
        issues
            .errors
            .push(SchemaIssue::new(path, "rule_param_invalid", "an op must be an object"));
        return None;
    };
    if object.contains_key("plugin") {
        return match parse_rule(entry, plan.ty.allowed_rules()) {
            Ok(RuleSpec {
                kind: RuleSpecKind::Plugin(rule),
                run_on_empty,
                ..
            }) => match check_plugin_ref(plan, path, &rule, stage, all, mode, counters) {
                Ok(()) => Some(OpStep::Plugin {
                    index,
                    rule,
                    run_on_empty,
                }),
                Err(issue) => {
                    issues.errors.push(issue);
                    None
                }
            },
            Ok(_) => {
                issues
                    .errors
                    .push(SchemaIssue::new(path, "rule_param_invalid", "invalid plugin op"));
                None
            }
            Err(error) => {
                issues
                    .errors
                    .push(SchemaIssue::new(format!("{path}{}", error.path), error.code, ""));
                None
            }
        };
    }
    let allowed = match stage {
        ExternalStage::Transform => plan.ty.transform_ops(),
        _ => plan.ty.normalize_ops(),
    };
    match BuiltinOp::parse(path, object, allowed) {
        Ok(op) => Some(OpStep::Builtin { index, op }),
        Err(issue) => {
            issues.errors.push(issue);
            None
        }
    }
}

fn check_validate_spec(
    plan: &FieldPlan,
    path: &str,
    spec: &RuleSpec,
    all: &[FormField],
    mode: Mode,
    counters: &mut Counters,
) -> Result<(), SchemaIssue> {
    match &spec.kind {
        RuleSpecKind::Builtin(BuiltinRule::Min { bound, .. } | BuiltinRule::Max { bound, .. }) => {
            if plan.ty.accepts_bound(bound) {
                Ok(())
            } else {
                Err(SchemaIssue::new(
                    format!("{path}/value"),
                    "rule_param_invalid",
                    "bound does not fit the field type",
                ))
            }
        }
        RuleSpecKind::Builtin(_) => Ok(()),
        RuleSpecKind::Cel(_) => {
            counters.cel += 1;
            Ok(())
        }
        RuleSpecKind::Plugin(rule) => check_plugin_ref(plan, path, rule, ExternalStage::Validate, all, mode, counters),
    }
}

/// `min` must not exceed `max` (an unsatisfiable field).
fn check_bounds_order(plan: &FieldPlan, base: &str, issues: &mut SchemaIssues) {
    let bound = |want_min: bool| {
        plan.validate.iter().find_map(|step| match &step.spec.kind {
            RuleSpecKind::Builtin(BuiltinRule::Min { bound, .. }) if want_min => Some(bound.clone()),
            RuleSpecKind::Builtin(BuiltinRule::Max { bound, .. }) if !want_min => Some(bound.clone()),
            _ => None,
        })
    };
    let (Some(min), Some(max)) = (bound(true), bound(false)) else {
        return;
    };
    let decimal = |b: &Bound| match b {
        Bound::Int(i) => Some(rust_decimal::Decimal::from(*i)),
        Bound::Decimal(d) => Some(*d),
        _ => None,
    };
    let inverted = match (&min, &max) {
        (Bound::Date(a), Bound::Date(b)) => a > b,
        (Bound::DateTime(a), Bound::DateTime(b)) => a > b,
        _ => matches!((decimal(&min), decimal(&max)), (Some(a), Some(b)) if a > b),
    };
    if inverted {
        issues.errors.push(SchemaIssue::new(
            format!("{base}/rules/validate"),
            "rule_param_invalid",
            "min exceeds max",
        ));
    }
}

fn check_plugin_ref(
    plan: &FieldPlan,
    path: &str,
    rule: &PluginRuleRef,
    stage: ExternalStage,
    all: &[FormField],
    mode: Mode,
    counters: &mut Counters,
) -> Result<(), SchemaIssue> {
    counters.plugins += 1;
    for dep in &rule.deps {
        let valid = dep != &plan.field.key
            && all.iter().any(|field| {
                &field.key == dep
                    && field_type(&field.field_type)
                        .is_some_and(|ty| ty.outlet(Outlet::PluginInput) == OutletRule::Value)
            });
        if !valid {
            return Err(SchemaIssue::new(
                format!("{path}/deps"),
                "plugin_dep_invalid",
                "a dep must name another plugin-readable field",
            ));
        }
    }
    let Mode::Strict(scope) = mode else {
        return Ok(());
    };
    let reference = ExternalRuleRef {
        package_key: rule.package.clone(),
        rule: rule.rule.clone(),
    };
    let available = scope.is_some_and(|scope| {
        resolver()
            .check(scope.workspace_id, scope.project_id, &reference, stage, plan.ty.name())
            .is_ok()
    });
    if available {
        Ok(())
    } else {
        Err(SchemaIssue::new(
            format!("{path}/plugin"),
            "plugin_rule_unavailable",
            "no active installation provides this rule",
        ))
    }
}

fn parse_default(
    plan: &FieldPlan,
    base: &str,
    raw: Option<&Value>,
    counters: &mut Counters,
    issues: &mut SchemaIssues,
) -> Option<DefaultSpec> {
    let path = format!("{base}/default");
    let raw = match raw {
        None | Some(Value::Null) => return None,
        Some(raw) => raw,
    };
    let Some(object) = raw.as_object().filter(|object| object.len() == 1) else {
        issues.errors.push(SchemaIssue::new(
            path,
            "default_invalid",
            "default must be {\"value\"} or {\"cel\"}",
        ));
        return None;
    };
    if let Some(source) = object.get("cel") {
        counters.cel += 1;
        let path = format!("{path}/cel");
        if plan.ty.cel_type().is_none() {
            issues.errors.push(SchemaIssue::new(
                path,
                "rule_not_allowed_for_type",
                "this field type has no CEL mapping",
            ));
            return None;
        }
        let Some(source) = source.as_str() else {
            issues.errors.push(SchemaIssue::new(
                path,
                "default_invalid",
                "default.cel must be a string",
            ));
            return None;
        };
        return match sv_rules::compile(source, sv_rules::Env::Default) {
            Ok(program) => Some(DefaultSpec::Cel(program)),
            Err(error) => {
                issues.errors.push(SchemaIssue::new(path, error.code, ""));
                None
            }
        };
    }
    let Some(value) = object.get("value") else {
        issues.errors.push(SchemaIssue::new(
            path,
            "default_invalid",
            "default must be {\"value\"} or {\"cel\"}",
        ));
        return None;
    };
    if value.is_null() {
        return None;
    }
    if super::pipeline::check_default_value(plan, value) {
        Some(DefaultSpec::Value(value.clone()))
    } else {
        issues.errors.push(SchemaIssue::new(
            format!("{path}/value"),
            "default_invalid",
            "default is not a valid value under the field rules",
        ));
        None
    }
}

fn parse_mask(plan: &FieldPlan, base: &str, raw: Option<&Value>, issues: &mut SchemaIssues) -> Option<MaskRule> {
    let path = format!("{base}/mask");
    let raw = match raw {
        None | Some(Value::Null) => return None,
        Some(raw) => raw,
    };
    if !plan.ty.maskable() {
        issues.errors.push(SchemaIssue::new(
            path,
            "rule_not_allowed_for_type",
            "mask is only allowed on text-like fields",
        ));
        return None;
    }
    let invalid = |key: &str| SchemaIssue::new(format!("{path}/{key}"), "rule_param_invalid", "invalid mask member");
    let Some(object) = raw.as_object() else {
        issues.errors.push(invalid(""));
        return None;
    };
    if let Some(key) = object
        .keys()
        .find(|key| !matches!(key.as_str(), "keep_first" | "keep_last" | "char" | "outlets"))
    {
        issues.errors.push(invalid(key));
        return None;
    }
    let keep = |key: &str| -> Result<usize, SchemaIssue> {
        match object.get(key) {
            None => Ok(0),
            Some(value) => value
                .as_u64()
                .filter(|keep| *keep <= MAX_MASK_KEEP)
                .and_then(|keep| usize::try_from(keep).ok())
                .ok_or_else(|| invalid(key)),
        }
    };
    let mask_char = match object.get("char") {
        None => Some('*'),
        Some(value) => value.as_str().and_then(|text| {
            let mut chars = text.chars();
            chars.next().filter(|_| chars.next().is_none())
        }),
    };
    let outlets = object.get("outlets").and_then(Value::as_array).and_then(|items| {
        items
            .iter()
            .map(|item| match item.as_str() {
                Some("site") => Some(Outlet::SiteRead),
                Some("list") => Some(Outlet::List),
                Some("export") => Some(Outlet::Export),
                _ => None,
            })
            .collect::<Option<Vec<_>>>()
            .filter(|outlets| !outlets.is_empty())
    });
    match (keep("keep_first"), keep("keep_last"), mask_char, outlets) {
        (Ok(keep_first), Ok(keep_last), Some(mask_char), Some(outlets)) => Some(MaskRule {
            keep_first,
            keep_last,
            mask_char,
            outlets,
        }),
        (Err(issue), ..) | (_, Err(issue), ..) => {
            issues.errors.push(issue);
            None
        }
        (_, _, None, _) => {
            issues.errors.push(invalid("char"));
            None
        }
        (_, _, _, None) => {
            issues.errors.push(invalid("outlets"));
            None
        }
    }
}
