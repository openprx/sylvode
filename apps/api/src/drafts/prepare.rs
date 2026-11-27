//! What a reviewer is shown and what a draft is judged by: the validation report, the capability
//! diff, the public-exposure list and the digest over all of it. The same functions run at
//! creation and again at approval, against the state of that moment.

use std::collections::HashMap;
use std::sync::LazyLock;

use sea_orm::{ConnectionTrait, DatabaseTransaction, DbBackend, FromQueryResult, Statement};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use sv_layout::{Analysis, CAP_ACTION_INVOKE, SlotDescriptor};
use uuid::Uuid;

use super::store::DraftRow;
use crate::{
    catalogue::{self, Risk},
    error::{ApiError, ApiErrorKind},
    forms::{
        rules::SchemaScope,
        schema::{ensure_schema_field_ids, normalize_key, validate_schema_report},
    },
    layouts::{
        access::{Scope, ScopeKind, resolve_scope},
        apply::{draft_slot, validate_for_slot},
    },
    site::drafts::{self as site_drafts, Base, PageTarget},
};

pub(super) const KIND_FORM_SCHEMA: &str = "form_schema";
pub(super) const KIND_LAYOUT: &str = "layout";
pub(super) const KIND_SITE_PAGE: &str = "site_page";

/// The largest serialised spec a draft accepts. The column check counts the text form of the
/// stored `jsonb`, which is a little longer than the compact form, so this leaves room under the
/// 512 KiB of the contract.
pub(super) const MAX_SPEC_BYTES: usize = 380_000;

/// `target` of a `form_schema` draft: an existing form (with the schema version the draft was
/// written against) or a new one.
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(super) struct FormTarget {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(super) form_id: Option<Uuid>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(super) base_schema_version: Option<i32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(super) key: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(super) name: Option<String>,
}

/// `target` of a `layout` draft: the slot and the scope the layout is placed in.
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(super) struct LayoutTarget {
    pub(super) slot_id: String,
    pub(super) scope_kind: String,
    pub(super) scope_id: Uuid,
}

impl FormTarget {
    /// What a later draft of the same author for the same form or new key replaces.
    pub(super) fn identity(&self) -> String {
        self.form_id.map_or_else(
            || format!("new:{}", self.key.as_deref().unwrap_or_default()),
            |id| id.to_string(),
        )
    }
}

impl LayoutTarget {
    pub(super) fn identity(&self) -> String {
        format!("{}/{}/{}", self.slot_id, self.scope_kind, self.scope_id)
    }
}

/// The judged state of a draft.
#[derive(Debug, Clone)]
pub(super) struct Prepared {
    /// The spec to store and to save: a schema comes back with its field ids filled.
    pub(super) spec: Value,
    pub(super) validation: Value,
    pub(super) capability_diff: Value,
    pub(super) public: Vec<Value>,
}

impl Prepared {
    pub(super) fn valid(&self) -> bool {
        self.validation.get("valid").and_then(Value::as_bool).unwrap_or(false)
    }

    pub(super) fn digest(&self) -> Result<String, ApiError> {
        review_digest(&self.spec, &self.validation, &self.capability_diff, &self.public)
    }
}

/// `"sha256:" + hex(sha256(JCS({spec, validation, capability_diff, public})))`: everything the
/// reviewer saw, so an approval of one digest never applies another state.
pub(super) fn review_digest(
    spec: &Value,
    validation: &Value,
    capability_diff: &Value,
    public: &[Value],
) -> Result<String, ApiError> {
    let document = json!({
        "spec": spec,
        "validation": validation,
        "capability_diff": capability_diff,
        "public": public,
    });
    let bytes = serde_jcs::to_vec(&document).map_err(|_| ApiError::internal())?;
    Ok(format!("sha256:{}", hex::encode(Sha256::digest(bytes))))
}

/// The pages, bindings and visitor fields of published sites that a draft would show visitors,
/// against the current state: a form schema through the site bindings of its form, a `site_page`
/// draft through the page (or the layout edit, or the title) it changes. A layout draft places a
/// layout in a console slot, which no site shows; a new form has no binding yet.
pub(super) async fn public_exposure(
    db: &impl ConnectionTrait,
    workspace_id: Uuid,
    project_id: Uuid,
    kind: &str,
    target: &Value,
    spec: &Value,
) -> Result<Vec<Value>, ApiError> {
    match kind {
        KIND_FORM_SCHEMA => {
            let form_id = target
                .get("form_id")
                .and_then(Value::as_str)
                .and_then(|id| id.parse::<Uuid>().ok());
            match form_id {
                Some(form_id) => site_drafts::form_exposure(db, form_id, spec).await,
                None => Ok(Vec::new()),
            }
        }
        KIND_SITE_PAGE => match serde_json::from_value::<PageTarget>(target.clone()) {
            Ok(target) => site_drafts::page_exposure(db, workspace_id, project_id, &target, spec).await,
            Err(_) => Ok(Vec::new()),
        },
        _ => Ok(Vec::new()),
    }
}

/// Judges a `site_page` draft against its site; a site or page that is gone is `not_found`.
pub(super) async fn site_page(
    db: &impl ConnectionTrait,
    workspace_id: Uuid,
    project_id: Uuid,
    target: &PageTarget,
    spec: &Value,
) -> Result<Prepared, ApiError> {
    let judged = site_drafts::judge(db, workspace_id, project_id, target, spec).await?;
    Ok(Prepared {
        capability_diff: judged.analysis.as_ref().map_or_else(|| json!([]), capability_diff),
        spec: judged.spec,
        validation: judged.validation,
        public: Vec::new(),
    })
}

/// Judges a form schema in the project it is written for.
pub(super) fn form_schema(workspace_id: Uuid, project_id: Uuid, spec: Value) -> Prepared {
    match ensure_schema_field_ids(spec.clone()) {
        Ok(schema) => {
            let issues = validate_schema_report(
                &schema,
                Some(SchemaScope {
                    workspace_id,
                    project_id,
                }),
            );
            let validation = json!({
                "valid": issues.is_valid(),
                "errors": issues.errors,
                "warnings": issues.warnings,
            });
            Prepared {
                spec: schema,
                validation,
                capability_diff: json!([]),
                public: Vec::new(),
            }
        }
        Err(_) => Prepared {
            spec,
            validation: json!({
                "valid": false,
                "errors": [{ "path": "", "code": "field_invalid" }],
                "warnings": [],
            }),
            capability_diff: json!([]),
            public: Vec::new(),
        },
    }
}

/// The risk of a capability id as the catalogue holds it.
static RISKS: LazyLock<HashMap<&'static str, Risk>> = LazyLock::new(|| {
    let entries: Vec<_> = catalogue::entries().copied().collect();
    catalogue::boot::capability_ids(&entries)
        .into_iter()
        .map(|id| (id.id, id.risk))
        .collect()
});

fn capability_diff(analysis: &Analysis) -> Value {
    Value::Array(
        analysis
            .capabilities
            .iter()
            .map(|usage| {
                let selector = if usage.id == CAP_ACTION_INVOKE {
                    json!({ "package": usage.target })
                } else {
                    json!({ "form": usage.target })
                };
                // An id the catalogue does not know is shown as a write, never as harmless.
                let risk = RISKS.get(usage.id.as_str()).copied().unwrap_or(Risk::Write);
                json!({
                    "capability": usage.id,
                    "selector": selector,
                    "risk": risk.as_str(),
                    "new": true,
                    "paths": usage.paths,
                })
            })
            .collect(),
    )
}

/// Judges a layout for the slot it will sit in.
pub(super) fn layout(slot: &SlotDescriptor, spec: Value) -> Prepared {
    match validate_for_slot(&spec, slot) {
        Ok(validated) => Prepared {
            capability_diff: capability_diff(&validated.analysis),
            validation: json!({ "valid": true, "errors": [], "analysis": validated.analysis }),
            spec,
            public: Vec::new(),
        },
        Err(errors) => Prepared {
            spec,
            validation: json!({ "valid": false, "errors": errors, "analysis": null }),
            capability_diff: json!([]),
            public: Vec::new(),
        },
    }
}

/// How an approved draft is saved, resolved against the state at approval.
pub(super) enum Plan {
    UpdateForm { form_id: Uuid },
    CreateForm { key: String, name: String },
    PlaceLayout { scope: Scope, slot: SlotDescriptor },
    SitePage { target: PageTarget },
}

fn stale(reason: &'static str, details: Value) -> ApiError {
    ApiError::conflict_coded(
        "draft_stale",
        match reason {
            "form_missing" => "the form of this draft no longer exists",
            "scope_missing" => "the scope of this draft no longer exists",
            "page_missing" => "the site or page of this draft no longer exists",
            _ => "the target changed since the draft was written",
        },
        Some(details),
    )
}

fn stored<T: for<'de> Deserialize<'de>>(target: &Value) -> Result<T, ApiError> {
    serde_json::from_value(target.clone()).map_err(|_| ApiError::internal())
}

/// Judges a stored draft again against the current state (its public-exposure list included) and
/// resolves how it would be saved. A form row is locked for the rest of the transaction. Refuses
/// `draft_stale` when the form moved on since the draft was written, or the site or page of a
/// `site_page` draft is gone, and `draft_kind_unsupported` for kinds a later batch adds.
pub(super) async fn recompute(tx: &DatabaseTransaction, row: &DraftRow) -> Result<(Prepared, Plan), ApiError> {
    let (mut prepared, plan) = judge_stored(tx, row).await?;
    prepared.public = public_exposure(
        tx,
        row.workspace_id,
        row.project_id,
        &row.kind,
        &row.target,
        &prepared.spec,
    )
    .await?;
    Ok((prepared, plan))
}

async fn judge_stored(tx: &DatabaseTransaction, row: &DraftRow) -> Result<(Prepared, Plan), ApiError> {
    match row.kind.as_str() {
        KIND_FORM_SCHEMA => {
            let target: FormTarget = stored(&row.target)?;
            let plan = match target.form_id {
                Some(form_id) => {
                    check_form_unchanged(tx, row, form_id, target.base_schema_version).await?;
                    Plan::UpdateForm { form_id }
                }
                None => {
                    let key = target.key.ok_or_else(ApiError::internal)?;
                    let name = target.name.ok_or_else(ApiError::internal)?;
                    ensure_key_free(tx, row.project_id, &key).await?;
                    Plan::CreateForm { key, name }
                }
            };
            Ok((form_schema(row.workspace_id, row.project_id, row.spec.clone()), plan))
        }
        KIND_LAYOUT => {
            let target: LayoutTarget = stored(&row.target)?;
            let kind = ScopeKind::parse(&target.scope_kind)?;
            let scope = resolve_scope(tx, kind, target.scope_id)
                .await
                .map_err(|_| stale("scope_missing", json!({})))?;
            if scope.workspace_id != row.workspace_id || scope.project_id.is_some_and(|id| id != row.project_id) {
                return Err(stale("scope_moved", json!({})));
            }
            let slot = draft_slot(&target.slot_id, kind)?;
            Ok((layout(&slot, row.spec.clone()), Plan::PlaceLayout { scope, slot }))
        }
        KIND_SITE_PAGE => {
            let target: PageTarget = stored(&row.target)?;
            // The page and its layout stay locked; a draft never overwrites a later change.
            match site_drafts::check_base(tx, &target).await? {
                Base::Unchanged => {}
                Base::Missing => return Err(stale("page_missing", json!({}))),
                Base::Changed => return Err(stale("target_changed", json!({}))),
            }
            let prepared = site_page(tx, row.workspace_id, row.project_id, &target, &row.spec)
                .await
                .map_err(|error| {
                    if error.kind() == ApiErrorKind::NotFound {
                        stale("page_missing", json!({}))
                    } else {
                        error
                    }
                })?;
            Ok((prepared, Plan::SitePage { target }))
        }
        _ => Err(ApiError::bad_request_coded(
            "draft_kind_unsupported",
            "this draft kind cannot be reviewed yet",
        )),
    }
}

async fn check_form_unchanged(
    tx: &DatabaseTransaction,
    row: &DraftRow,
    form_id: Uuid,
    base_schema_version: Option<i32>,
) -> Result<(), ApiError> {
    #[derive(FromQueryResult)]
    struct Current {
        project_id: Uuid,
        schema_version: i32,
        archived: bool,
    }
    let current = Current::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "SELECT project_id, schema_version, archived_at IS NOT NULL AS archived \
         FROM project_forms WHERE id = $1 FOR UPDATE",
        vec![form_id.into()],
    ))
    .one(tx)
    .await?
    .filter(|form| form.project_id == row.project_id)
    .ok_or_else(|| stale("form_missing", json!({})))?;
    if current.archived {
        return Err(ApiError::conflict_coded(
            "form_archived",
            "the form of this draft is archived",
            None,
        ));
    }
    if base_schema_version != Some(current.schema_version) {
        return Err(stale(
            "form_changed",
            json!({ "base_schema_version": base_schema_version, "current_schema_version": current.schema_version }),
        ));
    }
    Ok(())
}

/// A new form needs a key no form of the project has, archived ones included.
pub(super) async fn ensure_key_free(db: &impl ConnectionTrait, project_id: Uuid, key: &str) -> Result<(), ApiError> {
    #[derive(FromQueryResult)]
    struct Count {
        total: i64,
    }
    let taken = Count::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "SELECT COUNT(*)::bigint AS total FROM project_forms WHERE project_id = $1 AND key = $2",
        vec![project_id.into(), key.to_owned().into()],
    ))
    .one(db)
    .await?
    .is_some_and(|row| row.total > 0);
    if taken {
        return Err(ApiError::conflict_coded(
            "form_key_taken",
            "a form with this key already exists in the project",
            None,
        ));
    }
    Ok(())
}

/// A new form's key and name as the create route accepts them.
pub(super) fn new_form_identity(target: &FormTarget) -> Result<(String, String), ApiError> {
    let key = normalize_key(target.key.as_deref().unwrap_or_default())
        .map_err(|_| ApiError::bad_request_coded("target_invalid", "target.key must be a form key"))?;
    let name = target.name.as_deref().map(str::trim).unwrap_or_default();
    if name.is_empty() || name.chars().count() > 200 {
        return Err(ApiError::bad_request_coded(
            "target_invalid",
            "target.name must be 1 to 200 characters",
        ));
    }
    Ok((key, name.to_owned()))
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::unwrap_used)]
mod tests {
    use super::*;

    fn slot() -> SlotDescriptor {
        sv_layout::slot("form.record.detail").expect("the record detail slot exists")
    }

    fn layout_spec() -> Value {
        json!({
            "schema": "sv.layout.v1",
            "key": "sample.summary",
            "title": { "en": "Summary" },
            "root": { "type": "stack", "props": { "gap": "md" }, "children": [
                { "type": "text", "props": { "value": { "en": "Hello" } } }
            ] }
        })
    }

    #[test]
    fn a_valid_layout_is_judged_valid_with_an_empty_diff() {
        let prepared = layout(&slot(), layout_spec());
        assert!(prepared.valid(), "{}", prepared.validation);
        assert_eq!(prepared.capability_diff, json!([]));
        assert_eq!(prepared.validation["analysis"]["capabilities"], json!([]));
    }

    #[test]
    fn an_invalid_layout_keeps_its_typed_report_and_cannot_be_valid() {
        let prepared = layout(&slot(), json!({ "schema": "sv.layout.v1" }));
        assert!(!prepared.valid());
        assert!(
            prepared.validation["errors"]
                .as_array()
                .is_some_and(|errors| !errors.is_empty())
        );
        assert_eq!(prepared.validation["analysis"], Value::Null);
    }

    #[test]
    fn a_layout_that_reads_a_form_shows_the_capability_with_its_catalogue_risk() {
        let mut spec = layout_spec();
        spec["data"] = json!({ "rows": { "source": "query", "form": "form:orders", "fields": ["title"] } });
        let prepared = layout(&slot(), spec);
        assert!(prepared.valid(), "{}", prepared.validation);
        let diff = prepared.capability_diff.as_array().expect("a diff");
        assert_eq!(diff.len(), 1);
        assert_eq!(diff[0]["capability"], "forms.records.read");
        assert_eq!(diff[0]["selector"], json!({ "form": "form:orders" }));
        assert_eq!(diff[0]["risk"], "read");
        assert_eq!(diff[0]["new"], true);
    }

    #[test]
    fn a_schema_that_is_not_an_object_is_invalid_not_an_error() {
        let prepared = form_schema(Uuid::nil(), Uuid::nil(), json!([1, 2]));
        assert!(!prepared.valid());
        assert_eq!(prepared.validation["errors"][0]["code"], "field_invalid");
    }

    #[test]
    fn an_empty_v2_schema_is_valid_and_gets_its_field_ids_filled_once() {
        let spec =
            json!({ "version": "sv.form.schema.v2", "fields": [{ "key": "title", "label": "Title", "type": "text" }] });
        let first = form_schema(Uuid::nil(), Uuid::nil(), spec);
        assert!(first.valid(), "{}", first.validation);
        let second = form_schema(Uuid::nil(), Uuid::nil(), first.spec.clone());
        assert_eq!(first.spec, second.spec, "stored schemas keep their ids");
        assert_eq!(first.digest().unwrap(), second.digest().unwrap());
    }

    #[test]
    fn the_digest_covers_the_spec_the_report_the_diff_and_the_public_list() {
        let base = review_digest(&json!({"a": 1}), &json!({"valid": true}), &json!([]), &[]).unwrap();
        assert!(base.starts_with("sha256:") && base.len() == 7 + 64);
        assert_eq!(
            base,
            review_digest(&json!({"a": 1}), &json!({"valid": true}), &json!([]), &[]).unwrap()
        );
        for changed in [
            review_digest(&json!({"a": 2}), &json!({"valid": true}), &json!([]), &[]),
            review_digest(&json!({"a": 1}), &json!({"valid": false}), &json!([]), &[]),
            review_digest(&json!({"a": 1}), &json!({"valid": true}), &json!([{"c": 1}]), &[]),
            review_digest(
                &json!({"a": 1}),
                &json!({"valid": true}),
                &json!([]),
                &[json!({"site": "s"})],
            ),
        ] {
            assert_ne!(base, changed.unwrap());
        }
    }

    #[test]
    fn the_digest_does_not_depend_on_key_order() {
        let one = review_digest(&json!({"a": 1, "b": 2}), &json!({}), &json!([]), &[]).unwrap();
        let two = review_digest(&json!({"b": 2, "a": 1}), &json!({}), &json!([]), &[]).unwrap();
        assert_eq!(one, two);
    }

    #[test]
    fn targets_name_one_identity_per_form_or_slot_scope() {
        let id = Uuid::from_u128(7);
        let existing = FormTarget {
            form_id: Some(id),
            base_schema_version: Some(3),
            ..FormTarget::default()
        };
        assert_eq!(existing.identity(), id.to_string());
        let new = FormTarget {
            key: Some("orders".to_owned()),
            ..FormTarget::default()
        };
        assert_eq!(new.identity(), "new:orders");
        let target = LayoutTarget {
            slot_id: "form.record.detail".to_owned(),
            scope_kind: "form".to_owned(),
            scope_id: id,
        };
        assert_eq!(target.identity(), format!("form.record.detail/form/{id}"));
    }

    #[test]
    fn a_new_form_needs_a_key_and_a_name() {
        let target = |key: Option<&str>, name: Option<&str>| FormTarget {
            key: key.map(str::to_owned),
            name: name.map(str::to_owned),
            ..FormTarget::default()
        };
        assert_eq!(
            new_form_identity(&target(Some(" Orders "), Some(" Orders "))).unwrap(),
            ("orders".to_owned(), "Orders".to_owned())
        );
        assert!(new_form_identity(&target(None, Some("Orders"))).is_err());
        assert!(new_form_identity(&target(Some("orders"), None)).is_err());
        assert!(new_form_identity(&target(Some("bad key"), Some("Orders"))).is_err());
    }

    #[test]
    fn target_documents_refuse_unknown_fields() {
        assert!(serde_json::from_value::<FormTarget>(json!({"form_id": Uuid::nil(), "extra": 1})).is_err());
        assert!(serde_json::from_value::<LayoutTarget>(json!({"slot_id": "x", "scope_kind": "form"})).is_err());
    }
}
