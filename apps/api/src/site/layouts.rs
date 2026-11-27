//! Site page layouts (contract C3 sections 1.3 and 6.2, M10).
//!
//! Validation in site context, the `binding:<key>` form references the site app reads, route
//! matching, and the hooks the layout routes call (site-context validation and the published-site rule for bot writes).

use std::collections::BTreeSet;

use platform::app::AppState;
use sea_orm::ConnectionTrait;
use serde_json::{Map, Value, json};
use sv_layout::{Context, Options, Validated};
use uuid::Uuid;

use super::admin::page_spec;
use super::drafts::PageTarget;
use super::store::{self, PageRow};
use crate::error::ApiError;

/// The binding keys of a site.
pub(crate) async fn binding_keys(db: &impl ConnectionTrait, site_id: Uuid) -> Result<BTreeSet<String>, ApiError> {
    Ok(store::list_bindings(db, site_id)
        .await?
        .into_iter()
        .map(|binding| binding.binding_key)
        .collect())
}

/// Validates a layout for a site with these bindings: only `submit` to a binding and route
/// `navigate`, every data source on a binding (`$bind.<key>` or `form:<key>`).
///
/// # Errors
/// `layout_invalid` with every error and its path.
pub(crate) fn validate_site_layout(spec: &Value, bindings: BTreeSet<String>) -> Result<Validated, ApiError> {
    let opts = Options {
        slot: None,
        context: Context::Site { bindings },
    };
    let checked = serde_json::to_vec(spec).map_or_else(
        |_| sv_layout::validate(spec, &opts),
        |bytes| sv_layout::validate_bytes(&bytes, &opts),
    );
    checked.map_err(|errors| ApiError::Coded {
        status: 400,
        code: "layout_invalid",
        message: "the layout does not validate for this site".to_string(),
        details: Some(json!({ "errors": errors })),
    })
}

/// The binding key a data source's form reference names, if any.
pub(super) fn binding_of(form: &str) -> Option<&str> {
    form.strip_prefix("$bind.").or_else(|| form.strip_prefix("form:"))
}

/// The layout as the site app reads it: every data source's `form` is `binding:<key>`.
pub(crate) fn with_binding_refs(spec: &Value) -> Value {
    let mut spec = spec.clone();
    if let Some(data) = spec.get_mut("data").and_then(Value::as_object_mut) {
        for source in data.values_mut() {
            if let Some(form) = source.get_mut("form")
                && let Some(key) = form.as_str().and_then(binding_of).map(str::to_owned)
            {
                *form = Value::String(format!("binding:{key}"));
            }
        }
    }
    spec
}

/// The page whose route matches `path`, with the route parameters. A static route wins over a
/// route with parameters; the route pattern itself also matches.
pub(crate) fn match_route<'a>(pages: &'a [PageRow], path: &str) -> Option<(&'a PageRow, Map<String, Value>)> {
    let path = if path.len() > 1 {
        path.trim_end_matches('/')
    } else {
        path
    };
    let wanted: Vec<&str> = path.split('/').filter(|part| !part.is_empty()).collect();
    let mut best: Option<(&PageRow, Map<String, Value>)> = None;
    for page in pages {
        if page.route == path {
            return Some((page, Map::new()));
        }
        let pattern: Vec<&str> = page.route.split('/').filter(|part| !part.is_empty()).collect();
        if pattern.len() != wanted.len() {
            continue;
        }
        let mut params = Map::new();
        let matched = pattern.iter().zip(&wanted).all(|(part, segment)| {
            part.strip_prefix(':').map_or_else(
                || part == segment,
                |name| {
                    params.insert(name.to_string(), Value::String((*segment).to_string()));
                    true
                },
            )
        });
        if matched && best.as_ref().is_none_or(|(_, current)| params.len() < current.len()) {
            best = Some((page, params));
        }
    }
    best
}

/// The binding keys of site `site_id` for validating a layout in its context (the layout
/// validator's `site_id`), after the caller's read access to the site's project was checked.
pub async fn site_context_bindings(
    db: &impl ConnectionTrait,
    site_id: Uuid,
) -> Result<(Uuid, BTreeSet<String>), ApiError> {
    let site = store::load_site(db, site_id)
        .await?
        .ok_or_else(|| ApiError::not_found("site not found"))?;
    Ok((site.project_id, binding_keys(db, site_id).await?))
}

/// The published-site rule for a bot's layout edit (LEAD C3-6, contract C3 section 3).
///
/// When a page
/// of a published site shows the layout, the edit is not applied but stored as a `site_page` draft
/// (`op: update` of the first such page, with `layout_id` and the new `layout_spec`, which the
/// approval writes into the layout in place, and the base: the page as it is and
/// `expected_version`).
///
/// Returns the draft id when it was stored; a person, or a layout no published page shows, gets
/// `None` and the edit applies directly. `tx` is the caller's write transaction: the sites of the
/// pages found stay share-locked in it, so a concurrent publish cannot slip between this decision
/// and the direct write.
///
/// # Errors
/// `version_conflict` when `expected_version` is not the layout's version; the draft's errors.
pub async fn draft_bot_layout_update_on_published_site(
    state: &AppState,
    tx: &impl ConnectionTrait,
    bot_id: Option<Uuid>,
    layout_id: Uuid,
    spec: &Value,
    expected_version: i32,
) -> Result<Option<Uuid>, ApiError> {
    let Some(bot_id) = bot_id else {
        return Ok(None);
    };
    let Some(page) = store::published_pages_with_layout(tx, layout_id, true)
        .await?
        .into_iter()
        .next()
    else {
        return Ok(None);
    };
    let layout = store::load_layout(&state.db, layout_id)
        .await?
        .ok_or_else(|| ApiError::not_found("layout not found"))?;
    if layout.version != expected_version {
        return Err(ApiError::conflict_coded(
            "version_conflict",
            "the layout changed since it was read",
            Some(json!({ "current_version": layout.version })),
        ));
    }
    let site = store::load_site(&state.db, page.site_id)
        .await?
        .ok_or_else(ApiError::internal)?;
    let target = PageTarget::update(site.id, &page, expected_version);
    let spec = page_spec(
        &page.route,
        &page.audience,
        page.nav.as_ref(),
        Some(page.position),
        Some(layout_id),
        Some(spec.clone()),
    );
    let draft_id = crate::drafts::handlers::create_site_page_draft(
        state,
        bot_id,
        site.workspace_id,
        site.project_id,
        &target,
        spec,
    )
    .await?;
    Ok(Some(draft_id))
}

/// A bot may not archive a layout that a page of a published site shows: the contract has no
/// draft for it (a page without its layout), so the archive is refused and left to a person.
///
/// # Errors
/// `layout_on_published_site` for such a bot archive.
pub async fn refuse_bot_layout_archive_on_published_site(
    db: &impl ConnectionTrait,
    is_bot: bool,
    layout_id: Uuid,
) -> Result<(), ApiError> {
    if is_bot && store::layout_on_published_site(db, layout_id).await? {
        return Err(ApiError::conflict_coded(
            "layout_on_published_site",
            "a page of a published site shows this layout; a person archives it",
            None,
        ));
    }
    Ok(())
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    fn page(route: &str) -> PageRow {
        PageRow {
            id: Uuid::new_v4(),
            site_id: Uuid::nil(),
            route: route.to_string(),
            layout_id: Uuid::nil(),
            audience: "visitor".to_string(),
            nav: None,
            position: 0,
            installation_id: None,
        }
    }

    #[test]
    fn routes_match_static_first_then_parameters() {
        let pages = vec![page("/requests/:id"), page("/requests/new"), page("/")];
        let (found, params) = match_route(&pages, "/requests/new").unwrap();
        assert_eq!(found.route, "/requests/new");
        assert!(params.is_empty());
        let (found, params) = match_route(&pages, "/requests/42/").unwrap();
        assert_eq!(found.route, "/requests/:id");
        assert_eq!(params["id"], "42");
        assert_eq!(match_route(&pages, "/").unwrap().0.route, "/");
        assert!(match_route(&pages, "/other").is_none());
    }

    #[test]
    fn form_references_become_binding_references() {
        let spec = json!({"data": {"a": {"source": "query", "form": "$bind.requests"},
                                   "b": {"source": "form", "form": "form:notes"},
                                   "c": {"source": "plugin"}}});
        let out = with_binding_refs(&spec);
        assert_eq!(out["data"]["a"]["form"], "binding:requests");
        assert_eq!(out["data"]["b"]["form"], "binding:notes");
        assert!(out["data"]["c"].get("form").is_none());
    }

    /// The page layout of the RT3 runbook: a visitor form and the visitor's own requests.
    #[test]
    fn the_runbook_layout_validates_for_its_site() {
        let spec = json!({
            "schema": "sv.layout.v1", "key": "desk.new", "title": {"en": "New request"},
            "data": {
                "form": {"source": "form", "form": "$bind.requests", "fields": ["title", "description", "contact_phone"]},
                "mine": {"source": "query", "form": "$bind.requests", "fields": ["title", "status", "created_at"],
                         "sort": [{"field": "created_at", "dir": "desc"}], "page_size": 20}
            },
            "actions": {"send": {"kind": "submit", "data": "form", "mode": "create"}},
            "root": {"type": "stack", "children": [
                {"type": "form", "props": {"data": "form", "submit": "send"}},
                {"type": "table", "props": {"rows": "$data.mine.items", "columns": [{"field": "title"}, {"field": "status"}]}}
            ]}
        });
        assert!(validate_site_layout(&spec, BTreeSet::from(["requests".to_string()])).is_ok());
        assert_eq!(with_binding_refs(&spec)["data"]["mine"]["form"], "binding:requests");
    }

    #[test]
    fn site_layouts_refuse_invoke_and_foreign_sources() {
        let spec = json!({
            "schema": "sv.layout.v1", "key": "site.home", "title": {"en": "Home"},
            "data": {"mine": {"source": "query", "form": "$bind.other", "fields": ["title"]}},
            "actions": {"close": {"kind": "invoke", "package": "a/b", "export": "x"}},
            "root": {"type": "stack", "children": []}
        });
        let ApiError::Coded {
            details: Some(details),
            code,
            ..
        } = validate_site_layout(&spec, BTreeSet::from(["requests".to_string()])).unwrap_err()
        else {
            panic!("a coded error");
        };
        assert_eq!(code, "layout_invalid");
        let text = details.to_string();
        assert!(
            text.contains("binding_missing") && text.contains("action_not_allowed_on_site"),
            "{text}"
        );
    }
}
