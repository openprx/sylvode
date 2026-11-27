//! `site_page` drafts (contract C3 sections 3, 6.6 and 7; LEAD C3-6): a bot's write to a published
//! site (a page, the title, or a layout a published page shows) is stored as a draft that a
//! workspace owner or admin approves. Also the public-exposure lists of drafts: which pages of
//! published sites, which bindings and which visitor fields a change puts in front of visitors.
//!
//! A `site_page` spec is the page's values (`route`, `audience`, `nav`, `position`) with the layout
//! it shows: `layout_id` alone points the page at an existing layout, `layout_spec` alone replaces
//! the page's own site layout (or stores a new one at the site's scope), and both together (a bot
//! edit of a layout shown by a published page) replace that layout's spec in place and change
//! nothing else. A title change is `{title}` with `page_id: null`; a delete has an empty spec.
//!
//! An update or delete records the page as it was when the draft was written (`base_page`), and
//! an update also the version of the page's layout (`base_layout_version`). Approval locks the page
//! and the layout and refuses `draft_stale` when either moved on, so a draft never undoes a
//! person's later change.

use std::collections::BTreeSet;

use axum::response::{IntoResponse, Response};
use platform::app::AppState;
use sea_orm::{ConnectionTrait, DatabaseTransaction};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sv_layout::{Analysis, Context, Options};
use uuid::Uuid;

use super::access::Who;
use super::admin::{check_page_layout, check_page_values, check_text, save_site_layout};
use super::layouts::{binding_keys, binding_of, validate_site_layout};
use super::projection::{Audience, Projection, effective_read, effective_write, site_fields};
use super::store::{self, ExposedBinding, PageRow, PageValues, SiteRow};
use crate::error::{ApiError, ApiErrorKind};
use crate::response::ApiResponse;

/// What a `site_page` draft does to its page.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum PageOp {
    Create,
    Update,
    Delete,
}

/// The page values a draft was written against.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct BasePage {
    route: String,
    audience: String,
    #[serde(default)]
    nav: Option<Value>,
    position: i32,
    layout_id: Uuid,
}

impl BasePage {
    fn of(page: &PageRow) -> Self {
        Self {
            route: page.route.clone(),
            audience: page.audience.clone(),
            nav: page.nav.clone(),
            position: page.position,
            layout_id: page.layout_id,
        }
    }
}

/// `target` of a `site_page` draft.
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct PageTarget {
    pub(crate) site_id: Uuid,
    #[serde(default)]
    pub(crate) page_id: Option<Uuid>,
    pub(crate) op: PageOp,
    /// Update and delete: the page when the draft was written.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) base_page: Option<BasePage>,
    /// Update: the version of the page's layout when the draft was written (for a layout edit,
    /// the `expected_version` of the request).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) base_layout_version: Option<i32>,
}

/// Whether the page and layout of a draft are still as the draft found them.
#[derive(Debug, PartialEq, Eq)]
pub enum Base {
    Unchanged,
    Missing,
    Changed,
}

impl PageTarget {
    pub(crate) const fn title(site_id: Uuid) -> Self {
        Self {
            site_id,
            page_id: None,
            op: PageOp::Update,
            base_page: None,
            base_layout_version: None,
        }
    }

    pub(crate) const fn create(site_id: Uuid) -> Self {
        Self {
            site_id,
            page_id: None,
            op: PageOp::Create,
            base_page: None,
            base_layout_version: None,
        }
    }

    pub(crate) fn update(site_id: Uuid, page: &PageRow, layout_version: i32) -> Self {
        Self {
            site_id,
            page_id: Some(page.id),
            op: PageOp::Update,
            base_page: Some(BasePage::of(page)),
            base_layout_version: Some(layout_version),
        }
    }

    pub(crate) fn delete(site_id: Uuid, page: &PageRow) -> Self {
        Self {
            site_id,
            page_id: Some(page.id),
            op: PageOp::Delete,
            base_page: Some(BasePage::of(page)),
            base_layout_version: None,
        }
    }

    /// What a later draft of the same author replaces: the same page (a layout edit apart from
    /// the page's own values), the same new route, or the title. Matches the SQL of
    /// `drafts::store::supersede`.
    pub(crate) fn identity(&self, spec: &Value) -> String {
        let site = self.site_id;
        match self.page_id {
            Some(page) => {
                let edit = if spec.get("layout_id").is_some() && spec.get("layout_spec").is_some() {
                    "/layout"
                } else {
                    ""
                };
                format!("{site}/{page}{edit}")
            }
            None if self.op == PageOp::Create => {
                let route = spec.get("route").and_then(Value::as_str).unwrap_or_default();
                format!("{site}/create:{route}")
            }
            None => format!("{site}/title"),
        }
    }
}

/// `spec` of a `site_page` draft.
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct PageSpec {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    title: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    route: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    audience: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    nav: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    position: Option<i32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    layout_id: Option<Uuid>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    layout_spec: Option<Value>,
}

impl PageSpec {
    const fn has_page_values(&self) -> bool {
        self.route.is_some()
            || self.audience.is_some()
            || self.nav.is_some()
            || self.position.is_some()
            || self.layout_id.is_some()
            || self.layout_spec.is_some()
    }
}

/// Stores a bot's write to a published site as a `site_page` draft and answers `{draft_id}`.
pub async fn submit(
    state: &AppState,
    who: &Who,
    site: &SiteRow,
    target: &PageTarget,
    spec: Value,
) -> Result<Response, ApiError> {
    let bot_id = who.bot_id().ok_or_else(ApiError::internal)?;
    let draft_id = crate::drafts::handlers::create_site_page_draft(
        state,
        bot_id,
        site.workspace_id,
        site.project_id,
        target,
        spec,
    )
    .await?;
    Ok(ApiResponse::success(json!({ "draft_id": draft_id, "status": "pending" })).into_response())
}

/// The judged state of a `site_page` draft.
pub struct Judgement {
    /// The spec as stored (normalised).
    pub(crate) spec: Value,
    /// `{valid, errors, analysis}`.
    pub(crate) validation: Value,
    /// The analysis of the layout the page would show, when the draft is valid.
    pub(crate) analysis: Option<Analysis>,
}

#[derive(Default)]
struct Report {
    errors: Vec<Value>,
    analysis: Option<Analysis>,
}

impl Report {
    fn push(&mut self, path: &str, code: &str) {
        self.errors.push(json!({ "path": path, "code": code }));
    }

    /// Layout errors (each with its own path) placed under `base`.
    fn push_layout_errors(&mut self, errors: Vec<Value>, base: &str) {
        for mut error in errors {
            if let Some(object) = error.as_object_mut() {
                let inner = object
                    .get("path")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_owned();
                object.insert("path".to_string(), json!(format!("{base}{inner}")));
            }
            self.errors.push(error);
        }
    }

    /// Records a refusal of a layout check as report entries; other errors pass through.
    fn layout_refusal(&mut self, error: ApiError, base: &str) -> Result<(), ApiError> {
        match error {
            ApiError::Coded {
                code: "layout_invalid",
                details,
                ..
            } => {
                let errors = details
                    .as_ref()
                    .and_then(|details| details.get("errors"))
                    .and_then(Value::as_array)
                    .cloned()
                    .unwrap_or_default();
                self.push_layout_errors(errors, base);
                Ok(())
            }
            error if error.kind() == ApiErrorKind::NotFound => {
                self.push(base, "layout_not_found");
                Ok(())
            }
            ApiError::Coded { code, .. } => {
                self.push(base, code);
                Ok(())
            }
            other => Err(other),
        }
    }

    fn finish(self, spec: Value) -> Judgement {
        let valid = self.errors.is_empty();
        let analysis = if valid { self.analysis } else { None };
        Judgement {
            validation: json!({ "valid": valid, "errors": self.errors, "analysis": analysis }),
            spec,
            analysis,
        }
    }
}

const fn code_of(error: &ApiError) -> Option<&'static str> {
    match error {
        ApiError::Coded { code, .. } => Some(*code),
        _ => None,
    }
}

/// The draft's site (inside its workspace and project) and page (on that site). A missing one is
/// `not_found`; an op that does not fit the page is `target_invalid`.
async fn resolve(
    db: &impl ConnectionTrait,
    workspace_id: Uuid,
    project_id: Uuid,
    target: &PageTarget,
) -> Result<(SiteRow, Option<PageRow>), ApiError> {
    let site = store::load_site(db, target.site_id)
        .await?
        .filter(|site| site.workspace_id == workspace_id && site.project_id == project_id)
        .ok_or_else(|| ApiError::not_found("site not found"))?;
    let page = match target.page_id {
        Some(page_id) => Some(
            store::load_page(db, page_id)
                .await?
                .filter(|page| page.site_id == site.id)
                .ok_or_else(|| ApiError::not_found("page not found"))?,
        ),
        None => None,
    };
    let fits = match (target.op, page.is_some()) {
        (PageOp::Create | PageOp::Update, false) => target.base_page.is_none() && target.base_layout_version.is_none(),
        (PageOp::Update, true) => target.base_page.is_some() && target.base_layout_version.is_some(),
        (PageOp::Delete, true) => target.base_page.is_some() && target.base_layout_version.is_none(),
        (PageOp::Create, true) | (PageOp::Delete, false) => false,
    };
    if !fits {
        return Err(ApiError::bad_request_coded(
            "target_invalid",
            "a create names no page; an update or delete names its page and the base it was written against",
        ));
    }
    Ok((site, page))
}

/// Locks the draft's page and its layout for the rest of the review transaction and compares them
/// with the base the draft recorded.
pub async fn check_base(tx: &DatabaseTransaction, target: &PageTarget) -> Result<Base, ApiError> {
    let Some(page_id) = target.page_id else {
        return Ok(Base::Unchanged);
    };
    let Some(page) = store::lock_page(tx, page_id).await? else {
        return Ok(Base::Missing);
    };
    if target.base_page.as_ref() != Some(&BasePage::of(&page)) {
        return Ok(Base::Changed);
    }
    if let Some(version) = target.base_layout_version {
        match store::lock_layout_version(tx, page.layout_id).await? {
            Some(current) if current == version => {}
            Some(_) => return Ok(Base::Changed),
            None => return Ok(Base::Missing),
        }
    }
    Ok(Base::Unchanged)
}

/// Judges a `site_page` draft against the current state of its site. A missing site or page is
/// `not_found` (the review turns it into `draft_stale`); everything else is in the report.
pub async fn judge(
    db: &impl ConnectionTrait,
    workspace_id: Uuid,
    project_id: Uuid,
    target: &PageTarget,
    spec: &Value,
) -> Result<Judgement, ApiError> {
    let (site, page) = resolve(db, workspace_id, project_id, target).await?;
    let mut report = Report::default();
    let Ok(parsed) = serde_json::from_value::<PageSpec>(spec.clone()) else {
        report.push("", "spec_invalid");
        return Ok(report.finish(spec.clone()));
    };
    match (target.op, page.as_ref()) {
        (PageOp::Update, None) => judge_title(&parsed, &mut report),
        (PageOp::Delete, Some(_)) => {
            if parsed.title.is_some() || parsed.has_page_values() {
                report.push("", "spec_invalid");
            }
        }
        (_, page) => judge_page(db, &site, page, &parsed, &mut report).await?,
    }
    let normal = serde_json::to_value(&parsed).map_err(|_| ApiError::internal())?;
    Ok(report.finish(normal))
}

fn judge_title(spec: &PageSpec, report: &mut Report) {
    match spec.title.as_ref() {
        Some(title) if !spec.has_page_values() => {
            if check_text(title, "title_invalid").is_err() {
                report.push("/title", "title_invalid");
            }
        }
        _ => report.push("", "spec_invalid"),
    }
}

async fn judge_page(
    db: &impl ConnectionTrait,
    site: &SiteRow,
    page: Option<&PageRow>,
    spec: &PageSpec,
    report: &mut Report,
) -> Result<(), ApiError> {
    if spec.title.is_some() {
        report.push("/title", "spec_invalid");
    }
    let (Some(route), Some(audience)) = (spec.route.as_deref(), spec.audience.as_deref()) else {
        report.push("", "spec_invalid");
        return Ok(());
    };
    if let Err(error) = check_page_values(route, audience, spec.nav.as_ref()) {
        let code = code_of(&error).unwrap_or("spec_invalid");
        let path = match code {
            "route_invalid" => "/route",
            "audience_invalid" => "/audience",
            "nav_invalid" => "/nav",
            _ => "",
        };
        report.push(path, code);
    } else if store::route_taken(db, site.id, route, page.map(|page| page.id)).await? {
        report.push("/route", "route_taken");
    }
    match (spec.layout_id, spec.layout_spec.as_ref(), page) {
        (Some(layout_id), Some(layout_spec), Some(page)) => {
            judge_layout_edit(db, site, page, layout_id, layout_spec, report).await?;
        }
        (Some(layout_id), None, _) => match check_page_layout(db, site, layout_id).await {
            Ok((_, validated)) => report.analysis = Some(validated.analysis),
            Err(error) => report.layout_refusal(error, "/layout_id")?,
        },
        (None, Some(layout_spec), _) => match validate_site_layout(layout_spec, binding_keys(db, site.id).await?) {
            Ok(validated) => report.analysis = Some(validated.analysis),
            Err(error) => report.layout_refusal(error, "/layout_spec")?,
        },
        _ => report.push("/layout_id", "layout_required"),
    }
    Ok(())
}

/// An in-place edit of the layout a page shows: still that page's layout, an active user layout
/// of the workspace, valid in site context for every site that shows it and in every console slot
/// it is placed in.
async fn judge_layout_edit(
    db: &impl ConnectionTrait,
    site: &SiteRow,
    page: &PageRow,
    layout_id: Uuid,
    spec: &Value,
    report: &mut Report,
) -> Result<(), ApiError> {
    if layout_id != page.layout_id {
        report.push("/layout_id", "layout_moved");
        return Ok(());
    }
    let usable = store::load_layout(db, layout_id).await?.is_some_and(|layout| {
        layout.workspace_id == site.workspace_id && layout.status == "active" && layout.installation_id.is_none()
    });
    if !usable {
        report.push("/layout_id", "layout_not_found");
        return Ok(());
    }
    for site_id in store::sites_showing_layout(db, layout_id).await? {
        match validate_site_layout(spec, binding_keys(db, site_id).await?) {
            Ok(validated) if site_id == site.id => report.analysis = Some(validated.analysis),
            Ok(_) => {}
            Err(error) => report.layout_refusal(error, "/layout_spec")?,
        }
    }
    for slot_id in store::layout_slots(db, layout_id).await? {
        let Some(slot) = sv_layout::slot(&slot_id) else {
            continue;
        };
        let opts = Options {
            slot: Some(&slot),
            context: Context::Console,
        };
        if let Err(errors) = sv_layout::validate(spec, &opts) {
            let errors = errors
                .iter()
                .filter_map(|error| serde_json::to_value(error).ok())
                .collect();
            report.push_layout_errors(errors, "/layout_spec");
        }
    }
    Ok(())
}

fn route_taken() -> ApiError {
    ApiError::conflict_coded("route_taken", "this site already has a page at this route", None)
}

/// Saves an approved `site_page` draft in the review's transaction. The approver is a human
/// workspace owner or admin (checked by the review), who may write every page of the site; new
/// site layouts are created in the approver's name. Returns `{page_id, layout_id}`.
pub async fn apply(
    tx: &DatabaseTransaction,
    workspace_id: Uuid,
    project_id: Uuid,
    target: &PageTarget,
    spec: &Value,
    approver: Uuid,
) -> Result<Value, ApiError> {
    let (site, page) = resolve(tx, workspace_id, project_id, target).await?;
    let spec: PageSpec = serde_json::from_value(spec.clone()).map_err(|_| ApiError::internal())?;
    match (target.op, page) {
        (PageOp::Update, None) => {
            let title = spec.title.as_ref().ok_or_else(ApiError::internal)?;
            store::update_title(tx, site.id, title)
                .await?
                .ok_or_else(|| ApiError::not_found("site not found"))?;
            Ok(json!({ "page_id": null, "layout_id": null }))
        }
        (PageOp::Delete, Some(page)) => {
            store::delete_page(tx, page.id).await?;
            store::delete_unused_site_layout(tx, page.layout_id).await?;
            Ok(json!({ "page_id": page.id, "layout_id": null }))
        }
        (PageOp::Create, None) => {
            let (route, audience) = page_values(&spec)?;
            let layout_id = match (spec.layout_id, spec.layout_spec.as_ref()) {
                (Some(layout_id), None) => layout_id,
                (None, Some(layout_spec)) => save_site_layout(tx, &site, Some(approver), layout_spec, None).await?,
                _ => return Err(ApiError::internal()),
            };
            let values = PageValues {
                route,
                layout_id,
                audience,
                nav: spec.nav.as_ref(),
                position: spec.position.unwrap_or(0),
            };
            let page = store::insert_page(tx, site.id, values).await?.ok_or_else(route_taken)?;
            Ok(json!({ "page_id": page.id, "layout_id": layout_id }))
        }
        (PageOp::Update, Some(page)) if spec.layout_id.is_some() && spec.layout_spec.is_some() => {
            // A layout edit changes the layout only, from the version the draft was written against.
            let layout_id = spec.layout_id.ok_or_else(ApiError::internal)?;
            let layout_spec = spec.layout_spec.as_ref().ok_or_else(ApiError::internal)?;
            let key = validate_site_layout(layout_spec, binding_keys(tx, site.id).await?)?
                .layout
                .key;
            if !store::update_site_layout(tx, layout_id, &key, layout_spec, target.base_layout_version).await? {
                return Err(ApiError::conflict_coded(
                    "layout_key_taken",
                    "another layout of this scope already has this key",
                    None,
                ));
            }
            Ok(json!({ "page_id": page.id, "layout_id": layout_id }))
        }
        (PageOp::Update, Some(page)) => {
            let (route, audience) = page_values(&spec)?;
            let own_site_layout = store::load_layout(tx, page.layout_id)
                .await?
                .filter(|layout| layout.scope_kind == "site" && layout.scope_id == site.id)
                .map(|layout| layout.id);
            let layout_id = match (spec.layout_id, spec.layout_spec.as_ref()) {
                (Some(layout_id), _) => layout_id,
                (None, Some(layout_spec)) => {
                    save_site_layout(tx, &site, Some(approver), layout_spec, own_site_layout).await?
                }
                (None, None) => page.layout_id,
            };
            let values = PageValues {
                route,
                layout_id,
                audience,
                nav: spec.nav.as_ref(),
                position: spec.position.unwrap_or(page.position),
            };
            let updated = store::update_page(tx, page.id, values).await?.ok_or_else(route_taken)?;
            if let Some(old) = own_site_layout.filter(|old| *old != layout_id) {
                store::delete_unused_site_layout(tx, old).await?;
            }
            Ok(json!({ "page_id": updated.id, "layout_id": layout_id }))
        }
        _ => Err(ApiError::internal()),
    }
}

fn page_values(spec: &PageSpec) -> Result<(&str, &str), ApiError> {
    match (spec.route.as_deref(), spec.audience.as_deref()) {
        (Some(route), Some(audience)) => Ok((route, audience)),
        _ => Err(ApiError::internal()),
    }
}

// ---------------------------------------------------------------------------------------------
// Public exposure

/// The binding keys a layout's data sources read (`$bind.<key>` or `form:<key>`).
pub fn layout_bindings(spec: &Value) -> BTreeSet<String> {
    spec.get("data")
        .and_then(Value::as_object)
        .map(|data| {
            data.values()
                .filter_map(|source| source.get("form").and_then(Value::as_str))
                .filter_map(binding_of)
                .map(str::to_owned)
                .collect()
        })
        .unwrap_or_default()
}

/// The visitor read and submit fields of a binding under `schema`, after the read-time checks.
fn visitor_fields(binding: &ExposedBinding, schema: &Value) -> (Vec<String>, Vec<String>) {
    let Ok(fields) = site_fields(schema) else {
        return (Vec::new(), Vec::new());
    };
    let projection = Projection::stored(&binding.projection);
    let read = projection
        .read(Audience::Visitor)
        .map(|read| effective_read(read, &fields, &binding.title_template, &BTreeSet::new()).fields)
        .unwrap_or_default();
    let submit = projection
        .visitor
        .submit
        .as_ref()
        .map(|submit| effective_write(&submit.fields, &fields, &BTreeSet::new()))
        .unwrap_or_default();
    (read, submit)
}

/// One item of a `public` list (contract C3 section 5.2).
fn item(site: &SiteRow, page: Option<&str>, binding: Option<&str>, read: &[String], submit: &[String]) -> Value {
    json!({
        "site_id": site.id,
        "site_status": site.status,
        "page": page,
        "binding": binding,
        "visitor_read": read,
        "visitor_submit": submit,
    })
}

/// The items of one visitor page of a published site showing `layout`: one per binding the layout
/// reads, or one without a binding.
async fn page_items(
    db: &impl ConnectionTrait,
    site: &SiteRow,
    route: &str,
    layout: &Value,
) -> Result<Vec<Value>, ApiError> {
    let reads = layout_bindings(layout);
    let mut items = Vec::new();
    for binding in store::exposed_bindings_of_site(db, site.id).await? {
        if reads.contains(&binding.binding_key) {
            let (read, submit) = visitor_fields(&binding, &binding.schema);
            items.push(item(site, Some(route), Some(&binding.binding_key), &read, &submit));
        }
    }
    if items.is_empty() {
        items.push(item(site, Some(route), None, &[], &[]));
    }
    Ok(items)
}

/// What a `site_page` draft would show visitors of a published site: the new title, or each
/// visitor page with the bindings and fields its layout reads. A layout edit lists every visitor
/// page of a published site that shows the layout. A delete exposes nothing new.
pub async fn page_exposure(
    db: &impl ConnectionTrait,
    workspace_id: Uuid,
    project_id: Uuid,
    target: &PageTarget,
    spec: &Value,
) -> Result<Vec<Value>, ApiError> {
    let (site, page) = match resolve(db, workspace_id, project_id, target).await {
        Ok(found) => found,
        Err(error) if error.kind() == ApiErrorKind::NotFound || code_of(&error).is_some() => {
            return Ok(Vec::new());
        }
        Err(other) => return Err(other),
    };
    let Ok(spec) = serde_json::from_value::<PageSpec>(spec.clone()) else {
        return Ok(Vec::new());
    };
    if !site.is_published() || target.op == PageOp::Delete {
        return Ok(Vec::new());
    }
    let Some(route) = spec.route.as_deref() else {
        let mut title = item(&site, None, None, &[], &[]);
        if let Some(object) = title.as_object_mut() {
            object.insert("title".to_string(), spec.title.clone().unwrap_or(Value::Null));
        }
        return Ok(vec![title]);
    };
    let audience = spec.audience.as_deref().unwrap_or_default();
    if let (Some(layout_spec), Some(page)) = (spec.layout_spec.as_ref(), page.as_ref()) {
        if let Some(layout_id) = spec.layout_id {
            // A layout edit changes the layout only: every page showing it, as each page is now.
            return shared_layout_items(db, layout_id, None, layout_spec).await;
        }
        // The page's own site layout is replaced in place: every page sharing it changes.
        let own = store::load_layout(db, page.layout_id)
            .await?
            .is_some_and(|layout| layout.scope_kind == "site" && layout.scope_id == site.id);
        if own {
            let this = Some((page.id, route, audience));
            return shared_layout_items(db, page.layout_id, this, layout_spec).await;
        }
    }
    if audience != "visitor" {
        return Ok(Vec::new());
    }
    let layout = match (spec.layout_spec, spec.layout_id) {
        (Some(layout_spec), _) => layout_spec,
        // Only a layout this site's pages may show: an out-of-scope id exposes nothing.
        (None, Some(layout_id)) => check_page_layout(db, &site, layout_id)
            .await
            .ok()
            .map_or(Value::Null, |(layout, _)| layout.spec),
        (None, None) => Value::Null,
    };
    page_items(db, &site, route, &layout).await
}

/// The items of every visitor page of a published site that shows `layout_id`, all showing
/// `layout_spec`; `this` gives the draft's own page its new route and audience.
async fn shared_layout_items(
    db: &impl ConnectionTrait,
    layout_id: Uuid,
    this: Option<(Uuid, &str, &str)>,
    layout_spec: &Value,
) -> Result<Vec<Value>, ApiError> {
    let mut items = Vec::new();
    for shown in store::published_pages_with_layout(db, layout_id, false).await? {
        let (route, audience) = match this {
            Some((page_id, route, audience)) if page_id == shown.id => (route, audience),
            _ => (shown.route.as_str(), shown.audience.as_str()),
        };
        if audience != "visitor" {
            continue;
        }
        let Some(shown_site) = store::load_site(db, shown.site_id).await? else {
            continue;
        };
        items.extend(page_items(db, &shown_site, route, layout_spec).await?);
    }
    Ok(items)
}

/// What a schema for form `form_id` would show visitors: per binding of a published site, the
/// visitor fields under that schema and the visitor pages whose layout reads the binding (an item
/// without a page when none does: the site API serves a binding without a page).
pub async fn form_exposure(db: &impl ConnectionTrait, form_id: Uuid, schema: &Value) -> Result<Vec<Value>, ApiError> {
    let mut items = Vec::new();
    for binding in store::bindings_of_form(db, form_id, true).await? {
        let (read, submit) = visitor_fields(&binding, schema);
        if read.is_empty() && submit.is_empty() {
            continue;
        }
        let Some(site) = store::load_site(db, binding.site_id).await? else {
            continue;
        };
        let mut routes = Vec::new();
        for page in store::list_pages(db, site.id).await? {
            if page.audience != "visitor" {
                continue;
            }
            let reads = store::load_layout(db, page.layout_id)
                .await?
                .is_some_and(|layout| layout_bindings(&layout.spec).contains(&binding.binding_key));
            if reads {
                routes.push(page.route);
            }
        }
        if routes.is_empty() {
            items.push(item(&site, None, Some(&binding.binding_key), &read, &submit));
        }
        for route in &routes {
            items.push(item(&site, Some(route), Some(&binding.binding_key), &read, &submit));
        }
    }
    Ok(items)
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    fn target(page_id: Option<Uuid>, op: PageOp) -> PageTarget {
        PageTarget {
            site_id: Uuid::from_u128(1),
            page_id,
            op,
            base_page: None,
            base_layout_version: None,
        }
    }

    #[test]
    fn targets_parse_with_a_null_page_and_refuse_unknown_fields() {
        let parsed: PageTarget =
            serde_json::from_value(json!({"site_id": Uuid::nil(), "page_id": null, "op": "update"})).unwrap();
        assert_eq!(parsed.op, PageOp::Update);
        assert!(parsed.page_id.is_none());
        assert_eq!(serde_json::to_value(&parsed).unwrap()["page_id"], Value::Null);
        assert!(serde_json::from_value::<PageTarget>(json!({"site_id": Uuid::nil(), "op": "move"})).is_err());
        assert!(serde_json::from_value::<PageTarget>(json!({"site_id": Uuid::nil(), "op": "create", "x": 1})).is_err());
    }

    #[test]
    fn update_and_delete_targets_carry_their_base() {
        let page = PageRow {
            id: Uuid::from_u128(2),
            site_id: Uuid::from_u128(1),
            route: "/requests".to_string(),
            layout_id: Uuid::from_u128(3),
            audience: "visitor".to_string(),
            nav: None,
            position: 4,
            installation_id: None,
        };
        let update = PageTarget::update(page.site_id, &page, 7);
        let stored = serde_json::to_value(&update).unwrap();
        assert_eq!(stored["base_layout_version"], 7);
        assert_eq!(stored["base_page"]["route"], "/requests");
        let back: PageTarget = serde_json::from_value(stored).unwrap();
        assert_eq!(back.base_page, Some(BasePage::of(&page)));
        let mut moved = page.clone();
        moved.audience = "operator".to_string();
        assert_ne!(back.base_page, Some(BasePage::of(&moved)));
        let delete = serde_json::to_value(PageTarget::delete(page.site_id, &page)).unwrap();
        assert!(delete.get("base_layout_version").is_none());
        let title = serde_json::to_value(PageTarget::title(page.site_id)).unwrap();
        assert!(title.get("base_page").is_none());
    }

    #[test]
    fn identities_separate_pages_new_routes_layout_edits_and_the_title() {
        let page = Uuid::from_u128(2);
        let site = Uuid::from_u128(1);
        assert_eq!(
            target(None, PageOp::Create).identity(&json!({"route": "/a"})),
            format!("{site}/create:/a")
        );
        assert_eq!(
            target(None, PageOp::Update).identity(&json!({"title": "T"})),
            format!("{site}/title")
        );
        assert_eq!(
            target(Some(page), PageOp::Update).identity(&json!({"layout_id": page})),
            format!("{site}/{page}")
        );
        assert_eq!(
            target(Some(page), PageOp::Delete).identity(&json!({})),
            format!("{site}/{page}")
        );
        assert_eq!(
            target(Some(page), PageOp::Update).identity(&json!({"layout_id": page, "layout_spec": {}})),
            format!("{site}/{page}/layout")
        );
    }

    #[test]
    fn specs_refuse_unknown_fields_and_know_page_values() {
        assert!(serde_json::from_value::<PageSpec>(json!({"route": "/", "slug": "x"})).is_err());
        let title: PageSpec = serde_json::from_value(json!({"title": "Help"})).unwrap();
        assert!(!title.has_page_values());
        let page: PageSpec = serde_json::from_value(json!({"route": "/", "audience": "visitor"})).unwrap();
        assert!(page.has_page_values());
    }

    #[test]
    fn a_title_spec_is_judged_on_its_own() {
        let mut report = Report::default();
        judge_title(
            &serde_json::from_value(json!({"title": {"en": "Help"}})).unwrap(),
            &mut report,
        );
        assert!(report.errors.is_empty());
        let mut report = Report::default();
        judge_title(&serde_json::from_value(json!({"title": ""})).unwrap(), &mut report);
        assert_eq!(report.errors, vec![json!({"path": "/title", "code": "title_invalid"})]);
        let mut report = Report::default();
        judge_title(
            &serde_json::from_value(json!({"title": "Help", "route": "/"})).unwrap(),
            &mut report,
        );
        assert_eq!(report.errors[0]["code"], "spec_invalid");
    }

    #[test]
    fn layout_refusals_become_report_entries_under_their_path() {
        let mut report = Report::default();
        let invalid = ApiError::Coded {
            status: 400,
            code: "layout_invalid",
            message: String::new(),
            details: Some(json!({"errors": [{"path": "/root", "code": "binding_missing", "message": "m"}]})),
        };
        report.layout_refusal(invalid, "/layout_spec").unwrap();
        report
            .layout_refusal(ApiError::not_found("layout not found"), "/layout_id")
            .unwrap();
        assert_eq!(report.errors[0]["path"], "/layout_spec/root");
        assert_eq!(report.errors[0]["code"], "binding_missing");
        assert_eq!(
            report.errors[1],
            json!({"path": "/layout_id", "code": "layout_not_found"})
        );
        assert!(report.layout_refusal(ApiError::internal(), "").is_err());
        let judged = report.finish(json!({}));
        assert_eq!(judged.validation["valid"], false);
        assert!(judged.analysis.is_none());
    }

    #[test]
    fn a_layout_reads_the_bindings_of_its_data_sources() {
        let spec = json!({"data": {"a": {"form": "$bind.requests"}, "b": {"form": "form:notes"},
                                   "c": {"source": "plugin"}, "d": {"form": "orders"}}});
        assert_eq!(
            layout_bindings(&spec),
            BTreeSet::from(["notes".to_string(), "requests".to_string()])
        );
        assert!(layout_bindings(&Value::Null).is_empty());
    }

    #[test]
    fn visitor_fields_follow_the_proposed_schema() {
        let binding = ExposedBinding {
            site_id: Uuid::nil(),
            binding_key: "requests".to_string(),
            projection: json!({"visitor": {"read": {"fields": ["title", "status"], "scope": "own"},
                                           "submit": {"fields": ["title"]}}}),
            schema: Value::Null,
            title_template: "{title}".to_string(),
        };
        let both = json!({"version": "sv.form.schema.v2", "fields": [
            {"field_id": "f_000001", "key": "title", "label": "Title", "type": "text"},
            {"field_id": "f_000002", "key": "status", "label": "Status", "type": "text"}]});
        let (read, submit) = visitor_fields(&binding, &both);
        assert_eq!(read, vec!["title".to_string(), "status".to_string()]);
        assert_eq!(submit, vec!["title".to_string()]);
        let only_title = json!({"version": "sv.form.schema.v2", "fields": [
            {"field_id": "f_000001", "key": "title", "label": "Title", "type": "text"}]});
        let (read, _) = visitor_fields(&binding, &only_title);
        assert_eq!(read, vec!["title".to_string()]);
    }
}
