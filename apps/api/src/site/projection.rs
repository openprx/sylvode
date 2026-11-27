//! Binding projections (contract C3 section 6.2): what a visitor and an operator may read, filter,
//! sort, submit and update on the bound form.
//!
//! A projection is checked against the form schema when it is saved ([`Projection::parse`]) and
//! again on every read ([`effective_read`], [`effective_write`]): a field is used only while its
//! key exists with a `Value` site outlet (C2 section 4.1), a derived field only while every input
//! is listed for the same audience, and the title only while every title-template input is.

use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};

use crate::error::ApiError;
use crate::forms::rules::write_plan;
use crate::forms::types::{Outlet, OutletRule};

/// The record columns a projection may list besides the form fields: readable and sortable,
/// never filterable or writable. A form field with the same key wins.
pub(crate) const RECORD_COLUMNS: [&str; 2] = ["created_at", "updated_at"];

const MAX_FIELDS: usize = 200;
const DEFAULT_MAX_PER_HOUR: u32 = 20;

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Projection {
    #[serde(default)]
    pub(crate) visitor: VisitorProjection,
    #[serde(default)]
    pub(crate) operator: OperatorProjection,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct VisitorProjection {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) read: Option<ReadProjection>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) submit: Option<SubmitProjection>,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct OperatorProjection {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) read: Option<ReadProjection>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) update: Option<UpdateProjection>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ReadProjection {
    pub(crate) fields: Vec<String>,
    pub(crate) scope: String,
    #[serde(default)]
    pub(crate) filter: Vec<String>,
    #[serde(default)]
    pub(crate) sort: Vec<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct SubmitProjection {
    pub(crate) fields: Vec<String>,
    #[serde(default = "default_max_per_hour")]
    pub(crate) max_per_hour: u32,
    #[serde(default)]
    pub(crate) captcha: bool,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct UpdateProjection {
    pub(crate) fields: Vec<String>,
}

const fn default_max_per_hour() -> u32 {
    DEFAULT_MAX_PER_HOUR
}

/// The two audiences of a binding.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Audience {
    Visitor,
    Operator,
}

/// One problem of a projection: a JSON pointer into the projection and a code.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct ProjectionIssue {
    pub(crate) path: String,
    pub(crate) code: &'static str,
}

fn issue(path: impl Into<String>, code: &'static str) -> ProjectionIssue {
    ProjectionIssue {
        path: path.into(),
        code,
    }
}

/// What the schema says about one field, as the projection checks need it.
#[derive(Debug, Clone)]
pub(crate) struct SiteField {
    pub(crate) key: String,
    pub(crate) label: String,
    pub(crate) field_type: String,
    pub(crate) required: bool,
    pub(crate) options: Vec<String>,
    pub(crate) site_read: bool,
    pub(crate) site_submit: bool,
    pub(crate) filter: bool,
    pub(crate) sort: bool,
    /// The fields a derived value is computed from; `None` when they cannot be named, which
    /// keeps the field off the site.
    pub(crate) inputs: Option<BTreeSet<String>>,
    /// Masked on the site (`mask.outlets` lists `site`).
    pub(crate) masked: bool,
    /// The `rules.validate` entries the client mirror can evaluate (plugin references left out).
    pub(crate) rules_client: Vec<Value>,
}

/// The fields of a form schema in schema order, with their site facts.
pub(crate) fn site_fields(schema: &Value) -> Result<Vec<SiteField>, ApiError> {
    let plan = write_plan(schema).map_err(|_| ApiError::internal())?;
    let raw_fields: Vec<&Map<String, Value>> = schema
        .get("fields")
        .and_then(Value::as_array)
        .map(|fields| fields.iter().filter_map(Value::as_object).collect())
        .unwrap_or_default();
    let mut out = Vec::with_capacity(plan.fields.len());
    for field_plan in &plan.fields {
        let field = &field_plan.field;
        let raw = raw_fields
            .iter()
            .find(|raw| raw.get("key").and_then(Value::as_str) == Some(field.key.as_str()));
        let outlet = |o: Outlet| field_plan.ty.outlet(o) == OutletRule::Value;
        let formula = raw.and_then(|raw| raw.get("formula")).and_then(Value::as_object);
        let derived = field_plan.derived || formula.is_some();
        let inputs = if derived {
            derived_inputs(&field.field_type, formula)
        } else {
            Some(BTreeSet::new())
        };
        let options = field
            .options
            .iter()
            .filter(|option| !field.option_disabled.contains(option) && !field.option_archived.contains(option))
            .cloned()
            .collect();
        let rules_client = raw
            .and_then(|raw| raw.get("rules"))
            .and_then(|rules| rules.get("validate"))
            .and_then(Value::as_array)
            .map(|rules| {
                rules
                    .iter()
                    .filter(|rule| rule.get("plugin").is_none())
                    .cloned()
                    .collect()
            })
            .unwrap_or_default();
        out.push(SiteField {
            key: field.key.clone(),
            label: field.label.clone(),
            field_type: field.field_type.clone(),
            required: field.required,
            options,
            site_read: outlet(Outlet::SiteRead),
            site_submit: outlet(Outlet::SiteSubmit) && !derived,
            filter: outlet(Outlet::Filter),
            sort: outlet(Outlet::Sort),
            inputs,
            masked: field_plan
                .mask
                .as_ref()
                .is_some_and(|mask| mask.outlets.contains(&Outlet::SiteRead)),
            rules_client,
        });
    }
    Ok(out)
}

/// The inputs of a derived field: the field arguments of a formula; nothing for an autonumber;
/// `None` for an AI summary and a child aggregate, whose inputs are not fields of this record.
fn derived_inputs(field_type: &str, formula: Option<&Map<String, Value>>) -> Option<BTreeSet<String>> {
    if let Some(formula) = formula {
        if formula
            .get("op")
            .and_then(Value::as_str)
            .is_none_or(|op| op.starts_with("child_"))
        {
            return None;
        }
        let mut inputs = BTreeSet::new();
        for arg in formula.get("args").and_then(Value::as_array)? {
            match arg {
                Value::String(text) => {
                    if text.trim().parse::<f64>().is_err() {
                        inputs.insert(text.clone());
                    }
                }
                Value::Object(object) => {
                    if let Some(field) = object.get("field").and_then(Value::as_str) {
                        inputs.insert(field.to_string());
                    } else if object.get("value").is_none() {
                        return None;
                    }
                }
                _ => return None,
            }
        }
        return Some(inputs);
    }
    match field_type {
        "autonumber" => Some(BTreeSet::new()),
        _ => None,
    }
}

impl Projection {
    /// Parses and checks a projection against the bound form's schema (save time).
    ///
    /// # Errors
    /// Every problem found, refused as `projection_invalid`.
    pub(crate) fn parse(raw: &Value, schema: &Value) -> Result<Self, ApiError> {
        let projection: Self =
            serde_json::from_value(raw.clone()).map_err(|_| invalid(vec![issue("", "shape_invalid")]))?;
        let fields = site_fields(schema)?;
        let mut issues = Vec::new();
        if let Some(read) = &projection.visitor.read {
            check_read(read, &fields, "/visitor/read", &["own", "none"], &mut issues);
        }
        if let Some(submit) = &projection.visitor.submit {
            check_write(&submit.fields, &fields, "/visitor/submit/fields", &mut issues);
            if !(1..=1000).contains(&submit.max_per_hour) {
                issues.push(issue("/visitor/submit/max_per_hour", "limit_invalid"));
            }
            if submit.captcha {
                issues.push(issue("/visitor/submit/captcha", "captcha_unsupported"));
            }
        }
        if let Some(read) = &projection.operator.read {
            check_read(read, &fields, "/operator/read", &["all", "none"], &mut issues);
        }
        if let Some(update) = &projection.operator.update {
            check_write(&update.fields, &fields, "/operator/update/fields", &mut issues);
        }
        if issues.is_empty() {
            Ok(projection)
        } else {
            Err(invalid(issues))
        }
    }

    /// The stored projection of a binding; a row that no longer parses grants nothing.
    pub(crate) fn stored(raw: &Value) -> Self {
        serde_json::from_value(raw.clone()).unwrap_or_default()
    }

    pub(crate) fn read(&self, audience: Audience) -> Option<&ReadProjection> {
        match audience {
            Audience::Visitor => self.visitor.read.as_ref(),
            Audience::Operator => self.operator.read.as_ref(),
        }
        .filter(|read| read.scope != "none")
    }
}

fn invalid(issues: Vec<ProjectionIssue>) -> ApiError {
    ApiError::Coded {
        status: 400,
        code: "projection_invalid",
        message: "the projection does not fit the form".to_string(),
        details: Some(json!({ "errors": issues })),
    }
}

fn find<'a>(fields: &'a [SiteField], key: &str) -> Option<&'a SiteField> {
    fields.iter().find(|field| field.key == key)
}

fn is_record_column(fields: &[SiteField], key: &str) -> bool {
    RECORD_COLUMNS.contains(&key) && find(fields, key).is_none()
}

fn check_list(list: &[String], path: &str, issues: &mut Vec<ProjectionIssue>) {
    if list.len() > MAX_FIELDS {
        issues.push(issue(path, "limit_invalid"));
    }
    let mut seen = BTreeSet::new();
    for (index, key) in list.iter().enumerate() {
        if !seen.insert(key) {
            issues.push(issue(format!("{path}/{index}"), "field_duplicate"));
        }
    }
}

fn check_read(
    read: &ReadProjection,
    fields: &[SiteField],
    base: &str,
    scopes: &[&str],
    issues: &mut Vec<ProjectionIssue>,
) {
    if !scopes.contains(&read.scope.as_str()) {
        issues.push(issue(format!("{base}/scope"), "scope_invalid"));
    }
    check_list(&read.fields, &format!("{base}/fields"), issues);
    for (index, key) in read.fields.iter().enumerate() {
        let path = format!("{base}/fields/{index}");
        if is_record_column(fields, key) {
            continue;
        }
        match find(fields, key) {
            None => issues.push(issue(path, "field_unknown")),
            Some(field) if !field.site_read => issues.push(issue(path, "field_not_allowed")),
            Some(field) if !inputs_listed(field, &read.fields) => issues.push(issue(path, "derived_input_not_listed")),
            Some(_) => {}
        }
    }
    for (list, name, outlet_ok) in [
        (
            &read.filter,
            "filter",
            (|field: &SiteField| field.filter) as fn(&SiteField) -> bool,
        ),
        (&read.sort, "sort", |field: &SiteField| field.sort),
    ] {
        check_list(list, &format!("{base}/{name}"), issues);
        for (index, key) in list.iter().enumerate() {
            let path = format!("{base}/{name}/{index}");
            if !read.fields.contains(key) {
                issues.push(issue(path, "field_not_readable"));
            } else if is_record_column(fields, key) {
                if name == "filter" {
                    issues.push(issue(path, "field_not_filterable"));
                }
            } else if !find(fields, key).is_some_and(outlet_ok) {
                issues.push(issue(path, "field_not_filterable"));
            }
        }
    }
}

fn check_write(list: &[String], fields: &[SiteField], base: &str, issues: &mut Vec<ProjectionIssue>) {
    check_list(list, base, issues);
    for (index, key) in list.iter().enumerate() {
        let path = format!("{base}/{index}");
        match find(fields, key) {
            None if RECORD_COLUMNS.contains(&key.as_str()) => issues.push(issue(path, "field_not_allowed")),
            None => issues.push(issue(path, "field_unknown")),
            Some(field) if !field.site_submit => issues.push(issue(path, "field_not_allowed")),
            Some(_) => {}
        }
    }
}

fn inputs_listed(field: &SiteField, listed: &[String]) -> bool {
    field
        .inputs
        .as_ref()
        .is_some_and(|inputs| inputs.iter().all(|input| listed.contains(input)))
}

/// The read side of one audience, as it applies to this request.
#[derive(Debug, Clone, Default)]
pub(crate) struct EffectiveRead {
    /// Readable keys in projection order (form fields and record columns).
    pub(crate) fields: Vec<String>,
    pub(crate) filter: BTreeSet<String>,
    pub(crate) sort: BTreeSet<String>,
    /// Form fields among `fields` that are shown masked.
    pub(crate) masked: BTreeSet<String>,
    /// Whether the title may be shown (every title-template input is readable).
    pub(crate) title: bool,
    /// Listed fields dropped by the read-time check.
    pub(crate) dropped: usize,
}

impl EffectiveRead {
    pub(crate) fn allows(&self, key: &str) -> bool {
        self.fields.iter().any(|field| field == key)
    }
}

/// The read-time check (M12) of one audience's read list. `excluded` holds keys the principal
/// itself may not read (an operator's own field policy); they are dropped too.
pub(crate) fn effective_read(
    read: &ReadProjection,
    fields: &[SiteField],
    title_template: &str,
    excluded: &BTreeSet<String>,
) -> EffectiveRead {
    let mut kept: Vec<String> = Vec::new();
    let mut dropped = 0;
    // A derived field may depend on a field dropped in this same pass, so repeat until stable.
    let mut candidates: Vec<&String> = read.fields.iter().collect();
    loop {
        let listed: Vec<String> = candidates.iter().map(|key| (*key).clone()).collect();
        let next: Vec<&String> = candidates
            .iter()
            .copied()
            .filter(|key| {
                if excluded.contains(*key) {
                    return false;
                }
                if is_record_column(fields, key) {
                    return true;
                }
                find(fields, key).is_some_and(|field| field.site_read && inputs_listed(field, &listed))
            })
            .collect();
        if next.len() == candidates.len() {
            break;
        }
        candidates = next;
    }
    for key in &read.fields {
        if candidates.contains(&key) {
            kept.push(key.clone());
        } else {
            dropped += 1;
        }
    }
    let filter = read
        .filter
        .iter()
        .filter(|key| kept.contains(key) && !is_record_column(fields, key))
        .filter(|key| find(fields, key).is_some_and(|field| field.filter))
        .cloned()
        .collect();
    let sort = read
        .sort
        .iter()
        .filter(|key| kept.contains(key))
        .filter(|key| is_record_column(fields, key) || find(fields, key).is_some_and(|field| field.sort))
        .cloned()
        .collect();
    let masked = kept
        .iter()
        .filter(|key| find(fields, key).is_some_and(|field| field.masked))
        .cloned()
        .collect();
    let title = template_keys(title_template)
        .iter()
        .all(|key| key == "id" || (kept.contains(key) && !is_record_column(fields, key)));
    EffectiveRead {
        fields: kept,
        filter,
        sort,
        masked,
        title,
        dropped,
    }
}

/// The write side of one audience: listed keys that still exist with a `Value` `SiteSubmit`
/// outlet, minus `excluded`.
pub(crate) fn effective_write(list: &[String], fields: &[SiteField], excluded: &BTreeSet<String>) -> Vec<String> {
    list.iter()
        .filter(|key| !excluded.contains(*key))
        .filter(|key| find(fields, key).is_some_and(|field| field.site_submit))
        .cloned()
        .collect()
}

/// The lists of a projection after the read-time checks, one per audience side: visitor read,
/// visitor submit, operator read, operator update.
fn effective_lists(projection: &Projection, fields: &[SiteField], title_template: &str) -> [Vec<String>; 4] {
    let none = BTreeSet::new();
    let read = |audience| {
        projection
            .read(audience)
            .map(|read| effective_read(read, fields, title_template, &none).fields)
            .unwrap_or_default()
    };
    let submit = projection
        .visitor
        .submit
        .as_ref()
        .map(|submit| effective_write(&submit.fields, fields, &none))
        .unwrap_or_default();
    let update = projection
        .operator
        .update
        .as_ref()
        .map(|update| effective_write(&update.fields, fields, &none))
        .unwrap_or_default();
    [read(Audience::Visitor), submit, read(Audience::Operator), update]
}

/// Whether what the site shows of a field differs between two schemas.
fn shown_differently(old: &SiteField, new: &SiteField) -> bool {
    old.label != new.label
        || old.field_type != new.field_type
        || old.required != new.required
        || old.options != new.options
        || old.masked != new.masked
}

/// What a schema change does to a saved projection: the listed fields that stop qualifying under
/// the new schema (`lost`), and the fields still shown whose label, type, options, requiredness
/// or masking change (`changed`). Both sorted, without duplicates.
pub(crate) fn affected(
    raw: &Value,
    title_template: &str,
    old: &[SiteField],
    new: &[SiteField],
) -> (Vec<String>, Vec<String>) {
    let projection = Projection::stored(raw);
    let before = effective_lists(&projection, old, title_template);
    let after = effective_lists(&projection, new, title_template);
    let mut lost = BTreeSet::new();
    let mut changed = BTreeSet::new();
    for (before, after) in before.iter().zip(&after) {
        lost.extend(before.iter().filter(|key| !after.contains(key)).cloned());
        for key in after {
            if let (Some(old_field), Some(new_field)) = (find(old, key), find(new, key))
                && shown_differently(old_field, new_field)
            {
                changed.insert(key.clone());
            }
        }
    }
    (lost.into_iter().collect(), changed.into_iter().collect())
}

/// The `{key}` placeholders of a title template.
pub(crate) fn template_keys(template: &str) -> Vec<String> {
    let mut keys = Vec::new();
    let mut rest = template;
    while let Some(open) = rest.find('{') {
        let after = rest.get(open + 1..).unwrap_or_default();
        let Some(close) = after.find('}') else { break };
        let key = after.get(..close).unwrap_or_default();
        if !key.is_empty() && key.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_') {
            keys.push(key.to_string());
            rest = after.get(close + 1..).unwrap_or_default();
        } else {
            rest = after;
        }
    }
    keys
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::unwrap_used)]
mod tests {
    use super::*;

    fn schema() -> Value {
        json!({
            "version": "sv.form.schema.v2",
            "fields": [
                {"field_id": "f_000001", "key": "title", "label": "Title", "type": "text", "required": true},
                {"field_id": "f_000002", "key": "status", "label": "Status", "type": "single_select",
                 "options": ["open", "closed"]},
                {"field_id": "f_000003", "key": "phone", "label": "Phone", "type": "phone",
                 "mask": {"keep_last": 2, "outlets": ["site"]}},
                {"field_id": "f_000004", "key": "owner", "label": "Owner", "type": "member"},
                {"field_id": "f_000005", "key": "qty", "label": "Qty", "type": "integer"},
                {"field_id": "f_000006", "key": "price", "label": "Price", "type": "integer"},
                {"field_id": "f_000007", "key": "total", "label": "Total", "type": "formula",
                 "formula": {"op": "mul", "args": ["qty", "price"]}}
            ]
        })
    }

    fn codes(error: ApiError) -> Vec<(String, &'static str)> {
        let ApiError::Coded {
            details: Some(details),
            code,
            ..
        } = error
        else {
            panic!("a coded error");
        };
        assert_eq!(code, "projection_invalid");
        serde_json::from_value::<Vec<Value>>(details["errors"].clone())
            .unwrap()
            .into_iter()
            .map(|item| {
                let code: &'static str = Box::leak(item["code"].as_str().unwrap().to_string().into_boxed_str());
                (item["path"].as_str().unwrap().to_string(), code)
            })
            .collect()
    }

    #[test]
    fn a_valid_projection_parses() {
        let raw = json!({
            "visitor": {"read": {"fields": ["title", "status", "created_at"], "scope": "own",
                                 "filter": ["status"], "sort": ["created_at"]},
                        "submit": {"fields": ["title", "phone"], "max_per_hour": 20}},
            "operator": {"read": {"fields": ["title", "status", "phone"], "scope": "all"},
                         "update": {"fields": ["status"]}}
        });
        let projection = Projection::parse(&raw, &schema()).expect("valid");
        assert!(projection.read(Audience::Visitor).is_some());
        assert_eq!(projection.operator.update.unwrap().fields, vec!["status".to_string()]);
    }

    #[test]
    fn members_unknown_keys_and_bad_scopes_are_refused() {
        let raw = json!({
            "visitor": {"read": {"fields": ["owner", "nope"], "scope": "all", "filter": ["created_at"]},
                        "submit": {"fields": ["owner", "created_at", "total"], "captcha": true, "max_per_hour": 0}}
        });
        let found = codes(Projection::parse(&raw, &schema()).expect_err("invalid"));
        for expected in [
            ("/visitor/read/scope", "scope_invalid"),
            ("/visitor/read/fields/0", "field_not_allowed"),
            ("/visitor/read/fields/1", "field_unknown"),
            ("/visitor/read/filter/0", "field_not_readable"),
            ("/visitor/submit/fields/0", "field_not_allowed"),
            ("/visitor/submit/fields/1", "field_not_allowed"),
            ("/visitor/submit/fields/2", "field_not_allowed"),
            ("/visitor/submit/max_per_hour", "limit_invalid"),
            ("/visitor/submit/captcha", "captcha_unsupported"),
        ] {
            assert!(
                found.contains(&(expected.0.to_string(), expected.1)),
                "{expected:?} in {found:?}"
            );
        }
    }

    #[test]
    fn unknown_projection_keys_are_refused() {
        let raw = json!({"visitor": {"read": {"fields": [], "scope": "own", "extra": 1}}});
        assert_eq!(
            codes(Projection::parse(&raw, &schema()).expect_err("invalid")),
            vec![(String::new(), "shape_invalid")]
        );
    }

    #[test]
    fn a_formula_needs_its_inputs_listed() {
        let raw = json!({"visitor": {"read": {"fields": ["total", "qty"], "scope": "own"}}});
        let found = codes(Projection::parse(&raw, &schema()).expect_err("invalid"));
        assert_eq!(
            found,
            vec![("/visitor/read/fields/0".to_string(), "derived_input_not_listed")]
        );
        let ok = json!({"visitor": {"read": {"fields": ["total", "qty", "price"], "scope": "own"}}});
        assert!(Projection::parse(&ok, &schema()).is_ok());
    }

    #[test]
    fn the_read_check_drops_fields_that_no_longer_qualify() {
        let read = ReadProjection {
            fields: vec![
                "title".into(),
                "total".into(),
                "qty".into(),
                "price".into(),
                "gone".into(),
                "phone".into(),
            ],
            scope: "own".into(),
            filter: vec!["title".into()],
            sort: vec!["created_at".into()],
        };
        let fields = site_fields(&schema()).unwrap();
        let excluded = BTreeSet::from(["price".to_string()]);
        let effective = effective_read(&read, &fields, "{title} #{id}", &excluded);
        assert_eq!(
            effective.fields,
            vec!["title".to_string(), "qty".to_string(), "phone".to_string()]
        );
        assert_eq!(effective.dropped, 3, "price (excluded), total (input dropped) and gone");
        assert!(effective.title);
        assert!(effective.masked.contains("phone"));
        assert!(effective.filter.contains("title") && effective.sort.is_empty());
        let no_title = effective_read(&read, &fields, "{status}", &BTreeSet::new());
        assert!(!no_title.title, "status is not listed");
    }

    #[test]
    fn writes_keep_only_submittable_fields() {
        let fields = site_fields(&schema()).unwrap();
        let list = vec![
            "title".to_string(),
            "owner".to_string(),
            "total".to_string(),
            "phone".to_string(),
        ];
        assert_eq!(
            effective_write(&list, &fields, &BTreeSet::from(["phone".to_string()])),
            vec!["title".to_string()]
        );
    }

    #[test]
    fn a_schema_change_reports_lost_and_changed_projection_fields() {
        let projection = json!({
            "visitor": {"read": {"fields": ["title", "status", "created_at"], "scope": "own"},
                        "submit": {"fields": ["title", "phone"]}},
            "operator": {"read": {"fields": ["title", "qty"], "scope": "all"}}
        });
        let old = site_fields(&schema()).unwrap();
        assert_eq!(affected(&projection, "{title}", &old, &old), (vec![], vec![]));
        let mut changed = schema();
        // `phone` leaves the form; `status` gets a new label; `qty` (operator only) a new label too.
        let fields = changed["fields"].as_array_mut().unwrap();
        fields.retain(|field| field["key"] != "phone");
        for field in fields.iter_mut() {
            if field["key"] == "status" || field["key"] == "qty" {
                field["label"] = json!("Renamed");
            }
        }
        let new = site_fields(&changed).unwrap();
        let (lost, changed) = affected(&projection, "{title}", &old, &new);
        assert_eq!(lost, vec!["phone".to_string()]);
        assert_eq!(changed, vec!["qty".to_string(), "status".to_string()]);
    }

    #[test]
    fn template_keys_are_found() {
        assert_eq!(
            template_keys("{title} - {id} {not a key} {x"),
            vec!["title".to_string(), "id".to_string()]
        );
    }
}
