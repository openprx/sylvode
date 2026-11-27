//! Rule pipeline tests: schema v2 checks, every stage's codes, the recalculation carry policy, and
//! the guarantees kept from the first enforcement (no value echo, compile-once plans, invalid stored
//! entries skipped on write).

#![allow(clippy::indexing_slicing, clippy::needless_pass_by_value)]

use std::collections::BTreeSet;

use serde_json::{Value, json};
use uuid::Uuid;

use super::{
    FieldErrors, PipelineInput, PipelineOutput, RulePolicy, SchemaScope, WriteContext, apply_defaults, check_schema,
    ensure_read_only_untouched,
    external::{RuleCaller, Writer},
    run, write_plan,
};
use crate::{error::ApiError, forms::schema::parse_fields};

fn schema(fields: Value) -> Value {
    json!({"version": "sv.form.schema.v2", "fields": fields})
}

fn ctx() -> WriteContext {
    WriteContext {
        writer: Writer::Create,
        workspace_id: Uuid::nil(),
        project_id: Uuid::nil(),
        form_id: Uuid::nil(),
        record_id: None,
        caller: RuleCaller {
            actor_id: Some(Uuid::nil()),
            role: "member".to_string(),
            is_bot: false,
        },
        now: 1_767_225_600,
        denied_read_fields: BTreeSet::new(),
    }
}

async fn write(
    schema: &Value,
    values: Value,
    existing: Option<&Value>,
    policy: RulePolicy,
) -> Result<PipelineOutput, FieldErrors> {
    let none = BTreeSet::new();
    run(
        schema,
        &ctx(),
        PipelineInput {
            values,
            existing,
            always_check: &none,
            defaulted: &none,
            policy,
            report_untouched: false,
        },
    )
    .await
    .unwrap_or_else(|error| {
        Err(FieldErrors::single(super::FieldError::new(
            "",
            "",
            super::Stage::Normalize,
            error.to_string(),
        )))
    })
}

fn codes(errors: &FieldErrors) -> Vec<(String, String)> {
    errors
        .errors
        .iter()
        .map(|error| (error.key.clone(), error.code.to_string()))
        .collect()
}

fn issue_codes(schema: &Value) -> Vec<(String, &'static str)> {
    check_schema(schema, None)
        .errors
        .into_iter()
        .map(|issue| (issue.path, issue.code))
        .collect()
}

fn order_schema() -> Value {
    schema(json!([
        {"field_id": "fld_qty_0001", "key": "qty", "type": "integer",
         "rules": {"validate": [{"rule": "min", "value": 1}, {"rule": "max", "value": 10}]}},
        {"field_id": "fld_code_0001", "key": "code", "type": "text",
         "rules": {"normalize": [{"op": "trim"}, {"op": "upper"}],
                   "validate": [{"rule": "pattern", "value": "^[A-Z]{3}-[0-9]+$", "code": "bad_code"},
                                {"rule": "max_length", "value": 8}],
                   "transform": [{"op": "truncate", "max": 6}]}},
        {"field_id": "fld_price_001", "key": "price", "type": "number",
         "rules": {"validate": [{"rule": "max", "value": "99.5"}]}},
        {"field_id": "fld_tags_0001", "key": "tags", "type": "multi_select", "options": ["a", "b", "c"],
         "rules": {"validate": [{"rule": "enum", "values": ["a", "b"]}, {"rule": "max_length", "value": 1}]}},
        {"field_id": "fld_note_0001", "key": "note", "type": "text",
         "rules": {"validate": [{"cel": "size(value) > 2 && record.qty < 9", "code": "note_rule"}]}},
        {"field_id": "fld_state_001", "key": "state", "type": "text", "read_only": true,
         "default": {"value": "new"}}
    ]))
}

#[test]
fn a_valid_v2_schema_passes_and_v1_keys_are_refused() {
    assert_eq!(issue_codes(&order_schema()), Vec::new());

    let v1 = json!({"version": "openpr.form.schema.v1", "fields": [
        {"field_id": "fld_qty_0001", "key": "qty", "type": "integer",
         "validation": {"max": 3}, "default_value": 1, "policy": {"read_only": true}}
    ]});
    let found = issue_codes(&v1);
    assert!(found.contains(&("/version".to_string(), "schema_version_unsupported")));
    for key in ["validation", "default_value", "policy"] {
        assert!(
            found.contains(&(format!("/fields/0/{key}"), "schema_key_removed")),
            "{key}"
        );
    }
}

#[test]
fn schema_rule_problems_are_reported_by_path() {
    let bad = schema(json!([
        {"field_id": "fld_a_000001", "key": "a", "type": "integer",
         "rules": {"validate": [{"rule": "min", "value": 5}, {"rule": "max", "value": 1}]}},
        {"field_id": "fld_b_000001", "key": "b", "type": "boolean",
         "rules": {"validate": [{"rule": "pattern", "value": "x"}]}},
        {"field_id": "fld_c_000001", "key": "c", "type": "text",
         "rules": {"validate": [{"cel": "secret == 1"}], "normalize": [{"op": "round", "scale": 1}]}},
        {"field_id": "fld_d_000001", "key": "d", "type": "integer", "default": {"value": "x"}},
        {"field_id": "fld_e_000001", "key": "e", "type": "date",
         "rules": {"validate": [{"rule": "min", "value": 20_260_101}]}},
        {"field_id": "fld_f_000001", "key": "f", "type": "text",
         "rules": {"validate": [{"plugin": {"package": "acme/x", "rule": "y"}}]}},
        {"field_id": "fld_g_000001", "key": "g", "type": "integer", "mask": {"outlets": ["list"]}},
        {"field_id": "fld_h_000001", "key": "h", "type": "text", "default": {"cel": "now"}}
    ]));
    let found = issue_codes(&bad);
    for expected in [
        ("/fields/0/rules/validate", "rule_param_invalid"),
        ("/fields/1/rules/validate/0/rule", "rule_not_allowed_for_type"),
        ("/fields/2/rules/normalize/0/op", "rule_not_allowed_for_type"),
        ("/fields/2/rules/validate/0/cel", "cel_unknown_variable"),
        ("/fields/3/default/value", "default_invalid"),
        ("/fields/4/rules/validate/0/value", "rule_param_invalid"),
        ("/fields/5/rules/validate/0/plugin", "plugin_rule_unavailable"),
        ("/fields/6/mask", "rule_not_allowed_for_type"),
        ("/fields/7/default/cel", "default_invalid"),
    ] {
        assert!(
            found.contains(&(expected.0.to_string(), expected.1)),
            "{expected:?} not in {found:?}"
        );
    }
}

#[test]
fn plugin_rules_need_an_installation_even_with_a_scope() {
    let with_plugin = schema(json!([
        {"field_id": "fld_f_000001", "key": "f", "type": "text",
         "rules": {"validate": [{"plugin": {"package": "acme/x", "rule": "y"}}]}}
    ]));
    let scope = SchemaScope {
        workspace_id: Uuid::nil(),
        project_id: Uuid::nil(),
    };
    let issues = check_schema(&with_plugin, Some(scope));
    assert_eq!(
        issues.errors.first().map(|issue| issue.code),
        Some("plugin_rule_unavailable")
    );
}

#[tokio::test]
async fn every_builtin_stage_refuses_with_its_code_and_never_echoes_values() {
    let errors = write(
        &order_schema(),
        json!({"qty": 11, "code": " abc_123456 ", "price": "100", "tags": ["a", "c"], "note": "x"}),
        None,
        RulePolicy::Refuse,
    )
    .await
    .expect_err("rules must refuse");
    let found = codes(&errors);
    for expected in [
        ("qty", "max"),
        ("code", "bad_code"),
        ("code", "max_length"),
        ("price", "max"),
        ("tags", "enum"),
        ("tags", "max_length"),
        ("note", "note_rule"),
    ] {
        assert!(
            found.contains(&(expected.0.to_string(), expected.1.to_string())),
            "{expected:?} not in {found:?}"
        );
    }
    let ApiError::Coded { message, details, .. } = errors.into_api_error() else {
        return;
    };
    let text = format!("{message} {details:?}");
    for secret in ["abc_123456", "ABC_123456", "100", "\"c\""] {
        assert!(!text.contains(secret), "{secret} echoed in {text}");
    }
    let details = details.unwrap_or_default();
    let qty = details["errors"]
        .as_array()
        .and_then(|items| items.iter().find(|item| item["field"] == "qty"))
        .cloned()
        .unwrap_or_default();
    assert_eq!(qty["rule"], "validate[1]");
    assert_eq!(qty["params"]["bound"], 10);
    assert_eq!(qty["stage"], "validate");
}

#[tokio::test]
async fn normalize_and_transform_ops_shape_the_stored_value() {
    let stored = write(
        &order_schema(),
        json!({"qty": 2, "code": " abc-12 ", "note": "fine"}),
        None,
        RulePolicy::Refuse,
    )
    .await
    .expect("valid write");
    assert_eq!(stored.values["code"], "ABC-12");
    assert_eq!(stored.values["qty"]["value"], 2);
}

#[tokio::test]
async fn unchanged_stored_values_are_not_rechecked_on_update() {
    let existing = json!({"qty": {"type": "integer", "value": 50}, "note": "ok"});
    let output = write(
        &order_schema(),
        json!({"note": "fine"}),
        Some(&existing),
        RulePolicy::Refuse,
    )
    .await;
    // `note` reads `record.qty` (50): the CEL rule fails although qty itself is not touched.
    assert_eq!(
        output.map(|_| ()).map_err(|errors| codes(&errors)),
        Err(vec![("note".to_string(), "note_rule".to_string())])
    );
    let output = write(
        &order_schema(),
        json!({"code": "ABC-1"}),
        Some(&existing),
        RulePolicy::Refuse,
    )
    .await
    .expect("qty 50 predates the rule and is not touched");
    assert_eq!(output.values["qty"]["value"], 50);
}

#[tokio::test]
async fn recalculation_carries_rule_failures_but_refuses_type_errors() {
    let existing = json!({"qty": {"type": "integer", "value": 2}});
    let output = write(
        &order_schema(),
        json!({"qty": 12}),
        Some(&existing),
        RulePolicy::CarryAll,
    )
    .await
    .expect("a recalculation is never refused by a rule");
    assert_eq!(output.values["qty"]["value"], 12);
    assert_eq!(output.carried.first().map(|error| error.code.as_ref()), Some("max"));

    let errors = write(
        &order_schema(),
        json!({"qty": "twelve"}),
        Some(&existing),
        RulePolicy::CarryAll,
    )
    .await
    .expect_err("a wrong type still refuses");
    assert_eq!(codes(&errors), vec![("qty".to_string(), "type_mismatch".to_string())]);
}

#[tokio::test]
async fn plugin_rules_fail_closed_until_the_host_registers() {
    let stored_with_plugin = schema(json!([
        {"field_id": "fld_f_000001", "key": "f", "type": "text",
         "rules": {"validate": [{"plugin": {"package": "acme/x", "rule": "y"}}]}}
    ]));
    let errors = write(&stored_with_plugin, json!({"f": "v"}), None, RulePolicy::Refuse)
        .await
        .expect_err("no resolver");
    assert_eq!(
        codes(&errors),
        vec![("f".to_string(), "installation_not_active".to_string())]
    );
}

#[tokio::test]
async fn a_stored_invalid_entry_is_skipped_on_write() {
    let legacy = schema(json!([
        {"field_id": "fld_legacy_01", "key": "code", "type": "text",
         "rules": {"validate": [{"rule": "pattern", "value": "(?<=x)y"}, {"rule": "max_length", "value": 3}]}}
    ]));
    assert!(!check_schema(&legacy, None).is_valid());
    let errors = write(&legacy, json!({"code": "abcd"}), None, RulePolicy::Refuse)
        .await
        .expect_err("the valid sibling rule still applies");
    assert_eq!(codes(&errors), vec![("code".to_string(), "max_length".to_string())]);
    let first = write_plan(&legacy).map(|plan| std::sync::Arc::as_ptr(&plan) as usize);
    let second = write_plan(&legacy).map(|plan| std::sync::Arc::as_ptr(&plan) as usize);
    assert_eq!(first, second, "plans are compiled once per schema content");
}

#[test]
fn defaults_fill_absent_keys_only_and_read_only_binds_every_caller() {
    let schema = order_schema();
    let (values, defaulted) = apply_defaults(&schema, json!({"qty": 2})).unwrap_or_default();
    assert_eq!(values["state"], "new");
    assert!(defaulted.contains("state"));
    let (values, _) = apply_defaults(&schema, json!({"state": null})).unwrap_or_default();
    assert_eq!(values["state"], Value::Null);

    let none = BTreeSet::new();
    let existing = json!({"state": "approved"});
    assert!(ensure_read_only_untouched(&schema, &json!({"state": "approved"}), Some(&existing), &none).is_ok());
    assert!(ensure_read_only_untouched(&schema, &json!({"state": "new"}), None, &none).is_ok());
    assert!(ensure_read_only_untouched(&schema, &json!({"state": "rejected"}), Some(&existing), &none).is_err());
    let denied: BTreeSet<String> = std::iter::once("state".to_string()).collect();
    assert!(
        ensure_read_only_untouched(&schema, &json!({"state": "approved"}), Some(&existing), &denied).is_err(),
        "a matching guess on a hidden field is refused like any other"
    );
}

#[test]
fn every_field_type_parses_through_the_registry() {
    let names = crate::forms::types::all_field_types()
        .iter()
        .map(|ty| ty.name())
        .collect::<Vec<_>>();
    let fields = names
        .iter()
        .enumerate()
        .map(|(index, name)| json!({"field_id": format!("fld_type_{index:04}"), "key": format!("f{index}"), "type": name}))
        .collect::<Vec<_>>();
    let parsed = parse_fields(&schema(Value::Array(fields))).unwrap_or_default();
    assert_eq!(parsed.len(), names.len());
    assert!(
        parse_fields(&schema(
            json!([{"field_id": "fld_bad_00001", "key": "x", "type": "nope"}])
        ))
        .is_err()
    );
}
