//! The `InstallPreview` of contract C3 §5.2: what an installation would create and request, and
//! whether the package fits. The same shape answers a preview and the review of a pending generation.

use std::collections::BTreeSet;

use serde_json::{Value, json};
use sv_package::Kind;
use uuid::Uuid;

use super::validate::{Check, Resolution, capability_info};
use crate::packages::open::Opened;

/// The installation that already exists for the package in the scope, if any.
#[derive(Clone, Debug, Default)]
pub struct Existing {
    pub installation_id: Option<Uuid>,
    pub active_generation: Option<i32>,
    /// What the active generation requests, to flag what is new.
    pub active_requests: BTreeSet<String>,
}

/// Builds the preview.
#[must_use]
pub fn preview(
    opened: &Opened,
    check: &Check,
    scope_kind: &str,
    generation: i32,
    existing: &Existing,
    extras: &Value,
) -> Value {
    let manifest = &opened.package.manifest;
    let forms: Vec<&str> = check
        .resolutions
        .iter()
        .filter(|(_, resolution)| matches!(resolution, Resolution::Create))
        .map(|(name, _)| name.as_str())
        .collect();
    let placements: Vec<Value> = manifest
        .layouts
        .iter()
        .flat_map(|layout| layout.slots.iter())
        .map(|slot| json!({"slot_id": slot, "scope_kind": scope_kind}))
        .collect();
    let requests: Vec<Value> = manifest
        .requests
        .iter()
        .map(|request| {
            let bind = request.form.as_deref().and_then(|form| form.strip_prefix("$bind."));
            let selector_form = bind.and_then(|name| match check.resolutions.get(name) {
                Some(Resolution::Existing(id)) => Some(id.to_string()),
                _ => None,
            });
            let info = capability_info(&request.id);
            json!({
                "capability": request.id,
                "selector": {"form": selector_form},
                "form_bind": bind,
                "risk": info.as_ref().map_or("unknown", |info| info.risk),
                "label_key": format!("cap.{}", request.id),
                "new": !existing.active_requests.contains(&request.id),
            })
        })
        .collect();
    let mut preview = json!({
        "package": {
            "digest": opened.package.digest,
            "package_key": manifest.id,
            "version": manifest.version,
            "kind": kind_name(manifest.kind),
            "custom_code": opened.custom_code,
        },
        "generation": generation,
        "valid": check.valid(),
        "errors": check.problems,
        "binds": check.binds,
        "creates": {
            "forms": forms,
            "layouts": manifest.layouts.iter().map(|layout| layout.key.as_str()).collect::<Vec<_>>(),
            "placements": placements,
            "pages": manifest.pages.iter().map(|page| page.route.as_str()).collect::<Vec<_>>(),
        },
        "actions": manifest.actions.iter().map(|action| json!({"name": action.name, "label": action.label})).collect::<Vec<_>>(),
        "rules": manifest.rules.iter().map(|rule| json!({"name": rule.name, "stages": rule.stages, "field_types": rule.field_types})).collect::<Vec<_>>(),
        "requests": requests,
        "public": [],
        "existing": {
            "installation_id": existing.installation_id,
            "active_generation": existing.active_generation,
        },
    });
    // A package's `project` block is applied only when a project is created from a built-in template;
    // installing into an existing project never touches its settings.
    if manifest.project.is_some()
        && let Some(target) = preview.as_object_mut()
    {
        target.insert("project_settings".to_owned(), json!("not_applied"));
    }
    if let (Some(target), Some(extra)) = (preview.as_object_mut(), extras.as_object()) {
        for (key, value) in extra {
            target.insert(key.clone(), value.clone());
        }
    }
    preview
}

pub const fn kind_name(kind: Kind) -> &'static str {
    match kind {
        Kind::FormTemplate => "form-template",
        Kind::PageTemplate => "page-template",
        Kind::App => "app",
        Kind::Plugin => "plugin",
    }
}
