//! What an install request, a preview and an approval check in a package (contract C3 §5.3, C1 §6).
//!
//! Layout files were validated at upload; here the parts that need the target scope are checked:
//! every bind (an existing form with the fields the package needs, or a form template that is a valid
//! schema), every component (compiles, right world, imports the host provides), every requested
//! capability (exists and plugins may use it) and every layout in every slot it is placed in. The
//! checks collect problems instead of stopping at the first, so a preview can show them all.

use std::collections::{BTreeMap, BTreeSet};

use sea_orm::{DbBackend, FromQueryResult, Statement};
use serde::Serialize;
use serde_json::{Value, json};
use sv_layout::{Context, Options};
use sv_package::{Kind, Manifest};
use uuid::Uuid;

use crate::error::ApiError;
use crate::extensions::host::{World, verify_component};
use crate::forms::rules::{SchemaScope, check_schema};
use crate::packages::open::Opened;
use platform::app::AppState;

/// One thing wrong with a package for this scope.
#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
pub struct Problem {
    pub code: &'static str,
    pub path: String,
    pub message: String,
}

impl Problem {
    fn new(code: &'static str, path: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            code,
            path: path.into(),
            message: message.into(),
        }
    }
}

/// How a bind is resolved by the install request.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Resolution {
    /// A new form is created from the package's template when the installation is approved.
    Create,
    /// An existing form.
    Existing(Uuid),
}

/// A form the package creates: the template, parsed.
#[derive(Clone, Debug)]
pub struct FormTemplate {
    pub bind: String,
    pub key: String,
    pub name: String,
    pub description: String,
    pub icon: Option<String>,
    pub color: Option<String>,
    pub title_template: Option<String>,
    pub schema: Value,
    pub detail_layout: Value,
}

/// The state of one bind, as the preview lists it.
#[derive(Clone, Debug, Serialize)]
pub struct BindState {
    pub name: String,
    /// `create` or `existing`.
    pub resolution: &'static str,
    pub form_id: Option<Uuid>,
    pub missing_fields: Vec<String>,
    pub type_mismatches: Vec<Value>,
}

/// The outcome of the checks.
#[derive(Debug, Default)]
pub struct Check {
    pub problems: Vec<Problem>,
    pub binds: Vec<BindState>,
    pub templates: Vec<FormTemplate>,
    pub resolutions: BTreeMap<String, Resolution>,
}

impl Check {
    #[must_use]
    pub const fn valid(&self) -> bool {
        self.problems.is_empty()
    }
}

/// The error a request or an approval answers when the package does not fit: `422 install_invalid`
/// with the problems in `details.errors`.
#[must_use]
pub fn invalid(problems: &[Problem]) -> ApiError {
    ApiError::Coded {
        status: 422,
        code: "install_invalid",
        message: "the package does not fit this scope".to_string(),
        details: Some(json!({ "errors": problems })),
    }
}

fn world_of(name: &str) -> Option<World> {
    match name {
        "action" => Some(World::Action),
        "field-rule" => Some(World::FieldRule),
        _ => None,
    }
}

/// Runs every check of the package for the scope. `project_id` is `None` for a workspace
/// installation.
///
/// # Errors
/// A database failure; every other finding is a [`Problem`] in the result.
pub async fn check(
    state: &AppState,
    workspace_id: Uuid,
    project_id: Option<Uuid>,
    opened: &Opened,
    bind_map: &Value,
) -> Result<Check, ApiError> {
    let manifest = &opened.package.manifest;
    let mut out = Check::default();
    check_binds(state, workspace_id, project_id, opened, bind_map, &mut out).await?;
    check_components(opened, &mut out).await;
    check_requests(manifest, &mut out);
    check_layout_slots(opened, &mut out);
    Ok(out)
}

#[derive(FromQueryResult)]
struct FormRow {
    project_id: Uuid,
    schema: Value,
}

async fn check_binds(
    state: &AppState,
    workspace_id: Uuid,
    project_id: Option<Uuid>,
    opened: &Opened,
    bind_map: &Value,
    out: &mut Check,
) -> Result<(), ApiError> {
    let manifest = &opened.package.manifest;
    let Some(map) = bind_map.as_object() else {
        out.problems.push(Problem::new(
            "bind_map_invalid",
            "/bind_map",
            "bind_map must be an object",
        ));
        return Ok(());
    };
    for name in map.keys().filter(|name| !manifest.binds.contains_key(*name)) {
        out.problems.push(Problem::new(
            "bind_unknown",
            format!("/bind_map/{name}"),
            "the package has no such bind",
        ));
    }
    for (name, bind) in &manifest.binds {
        let path = format!("/bind_map/{name}");
        let Some(choice) = map.get(name).and_then(Value::as_object) else {
            out.problems.push(Problem::new(
                "bind_unresolved",
                path,
                "the bind needs {\"form_id\"} or {\"create\": true}",
            ));
            continue;
        };
        let wants_create = choice.get("create").and_then(Value::as_bool) == Some(true);
        let form_id = choice
            .get("form_id")
            .and_then(Value::as_str)
            .and_then(|text| Uuid::parse_str(text).ok());
        if choice.len() != 1 || wants_create == form_id.is_some() {
            out.problems.push(Problem::new(
                "bind_map_invalid",
                path,
                "the bind needs exactly one of {\"form_id\"} and {\"create\": true}",
            ));
            continue;
        }
        let (schema, resolution, id) = if wants_create {
            let Some(from) = bind.create_from.as_deref() else {
                out.problems.push(Problem::new(
                    "bind_create_unavailable",
                    path,
                    "the package has no template for this bind",
                ));
                continue;
            };
            let Some(project) = project_id else {
                out.problems.push(Problem::new(
                    "bind_create_needs_project",
                    path,
                    "a form is created in a project: install into a project",
                ));
                continue;
            };
            let Some(template) = parse_template(opened, name, from, workspace_id, project, out) else {
                continue;
            };
            let schema = template.schema.clone();
            out.templates.push(template);
            (schema, Resolution::Create, None)
        } else {
            let Some(id) = form_id else { continue };
            let row = FormRow::find_by_statement(Statement::from_sql_and_values(
                DbBackend::Postgres,
                "SELECT project_id, schema FROM project_forms WHERE id = $1 AND workspace_id = $2",
                vec![id.into(), workspace_id.into()],
            ))
            .one(&state.db)
            .await?;
            match row {
                Some(row) if project_id.is_none_or(|project| project == row.project_id) => {
                    (row.schema, Resolution::Existing(id), Some(id))
                }
                _ => {
                    out.problems.push(Problem::new(
                        "bind_form_not_found",
                        path,
                        "no such form in the scope of the installation",
                    ));
                    continue;
                }
            }
        };
        let (missing, mismatched) = compare_fields(&schema, &bind.requires_fields);
        if !missing.is_empty() || !mismatched.is_empty() {
            out.problems.push(Problem::new(
                "bind_incompatible",
                format!("/bind_map/{name}"),
                "the form lacks fields the package requires or has them with another type",
            ));
        }
        out.binds.push(BindState {
            name: name.clone(),
            resolution: if matches!(resolution, Resolution::Create) {
                "create"
            } else {
                "existing"
            },
            form_id: id,
            missing_fields: missing,
            type_mismatches: mismatched,
        });
        out.resolutions.insert(name.clone(), resolution);
    }
    Ok(())
}

/// The fields a form schema lacks, and those it has with another type, against `requires_fields`.
fn compare_fields(schema: &Value, required: &BTreeMap<String, String>) -> (Vec<String>, Vec<Value>) {
    let have: BTreeMap<&str, &str> = schema
        .get("fields")
        .and_then(Value::as_array)
        .map(|fields| {
            fields
                .iter()
                .filter_map(|field| {
                    Some((
                        field.get("key")?.as_str()?,
                        field.get("type").and_then(Value::as_str).unwrap_or(""),
                    ))
                })
                .collect()
        })
        .unwrap_or_default();
    let mut missing = Vec::new();
    let mut mismatched = Vec::new();
    for (key, expected) in required {
        match have.get(key.as_str()) {
            None => missing.push(key.clone()),
            Some(actual) if actual != expected => {
                mismatched.push(json!({"field": key, "expected": expected, "actual": actual}));
            }
            Some(_) => {}
        }
    }
    (missing, mismatched)
}

/// The form template of a `create` bind: `{key, name, schema, description?, icon?, color?,
/// title_template?, detail_layout?}`. The schema is checked like a schema save; a reference to a
/// plugin rule is judged at approval, when the installation that provides it is being activated.
fn parse_template(
    opened: &Opened,
    bind: &str,
    from: &str,
    workspace_id: Uuid,
    project_id: Uuid,
    out: &mut Check,
) -> Option<FormTemplate> {
    let path = format!("/files/{from}");
    let bad = |code: &'static str, message: &str| Problem::new(code, path.clone(), message);
    let parsed: Option<Value> = opened
        .package
        .files
        .get(from)
        .and_then(|bytes| serde_json::from_slice(bytes).ok());
    let Some(Value::Object(object)) = parsed else {
        out.problems
            .push(bad("form_template_invalid", "the template is not a JSON object"));
        return None;
    };
    let text = |key: &str| object.get(key).and_then(Value::as_str).map(str::to_owned);
    let (Some(key), Some(name), Some(schema)) = (text("key"), text("name"), object.get("schema").cloned()) else {
        out.problems
            .push(bad("form_template_invalid", "the template needs key, name and schema"));
        return None;
    };
    let issues = check_schema(
        &schema,
        Some(SchemaScope {
            workspace_id,
            project_id,
        }),
    );
    let mut ok = true;
    for issue in issues
        .errors
        .iter()
        .filter(|issue| issue.code != "plugin_rule_unavailable")
    {
        ok = false;
        out.problems.push(Problem::new(
            "form_template_invalid",
            format!("{path}{}", issue.path),
            issue.code,
        ));
    }
    ok.then(|| FormTemplate {
        bind: bind.to_owned(),
        key,
        name,
        description: text("description").unwrap_or_default(),
        icon: text("icon"),
        color: text("color"),
        title_template: text("title_template"),
        schema,
        detail_layout: object.get("detail_layout").cloned().unwrap_or_else(|| json!({})),
    })
}

async fn check_components(opened: &Opened, out: &mut Check) {
    let manifest = &opened.package.manifest;
    let entries = manifest
        .actions
        .iter()
        .map(|action| ("actions", &action.name, &action.world, &action.component))
        .chain(
            manifest
                .rules
                .iter()
                .map(|rule| ("rules", &rule.name, &rule.world, &rule.component)),
        );
    for (list, name, world, component) in entries {
        let path = format!("/{list}/{name}");
        let (Some(world), Some(bytes)) = (world_of(world), opened.package.files.get(component)) else {
            out.problems.push(Problem::new(
                "plugin_world_mismatch",
                path,
                "the entry names a world this host does not run, or a component that is not in the package",
            ));
            continue;
        };
        if let Err(error) = verify_component(bytes, world).await {
            out.problems
                .push(Problem::new(error.code.as_str(), path, error.code.message()));
        }
    }
}

/// What the catalogue says about a capability id a package may request.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CapabilityInfo {
    pub risk: &'static str,
    pub plugin_exposed: bool,
}

/// The catalogue's view of an id, `None` when it does not exist.
#[must_use]
pub fn capability_info(id: &str) -> Option<CapabilityInfo> {
    static IDS: std::sync::LazyLock<BTreeMap<&'static str, CapabilityInfo>> = std::sync::LazyLock::new(|| {
        let entries: Vec<_> = crate::catalogue::entries().copied().collect();
        crate::catalogue::boot::capability_ids(&entries)
            .into_iter()
            .map(|capability| {
                (
                    capability.id,
                    CapabilityInfo {
                        risk: capability.risk.as_str(),
                        plugin_exposed: capability.plugin_exposed,
                    },
                )
            })
            .collect()
    });
    IDS.get(id).cloned()
}

fn check_requests(manifest: &Manifest, out: &mut Check) {
    let mut seen = BTreeSet::new();
    for request in &manifest.requests {
        if !seen.insert(request.id.as_str()) {
            continue;
        }
        match capability_info(&request.id) {
            Some(info) if info.plugin_exposed => {}
            Some(_) => out.problems.push(Problem::new(
                "request_not_plugin_exposed",
                format!("/requests/{}", request.id),
                "plugins cannot use this capability",
            )),
            None => out.problems.push(Problem::new(
                "request_unknown",
                format!("/requests/{}", request.id),
                "no such capability",
            )),
        }
    }
}

/// Every layout is checked in every slot the package places it in.
fn check_layout_slots(opened: &Opened, out: &mut Check) {
    if matches!(opened.package.manifest.kind, Kind::FormTemplate) {
        return;
    }
    for entry in &opened.package.manifest.layouts {
        for slot_id in &entry.slots {
            let path = format!("/layouts/{}/{slot_id}", entry.key);
            let Some(slot) = sv_layout::slot(slot_id) else {
                out.problems.push(Problem::new("slot_unknown", path, "no such slot"));
                continue;
            };
            if slot.max_entries == 0 {
                out.problems
                    .push(Problem::new("slot_unknown", path, "this slot takes no placements"));
                continue;
            }
            let Some(bytes) = opened.package.files.get(&entry.path) else {
                continue;
            };
            let opts = Options {
                slot: Some(&slot),
                context: Context::Console,
            };
            if sv_layout::validate_bytes(bytes, &opts).is_err() {
                out.problems.push(Problem::new(
                    "slot_node_not_allowed",
                    path,
                    "the layout does not fit the slot",
                ));
            }
        }
    }
}

/// Confirms that a database round trip is not needed to decide a request is well formed.
#[must_use]
pub fn scope_error(kind: &str, project_id: Option<Uuid>) -> Option<&'static str> {
    match (kind, project_id.is_some()) {
        ("workspace", false) | ("project", true) => None,
        ("workspace", true) | ("project", false) => Some("scope.project_id must be given exactly for a project scope"),
        _ => Some("scope.kind must be workspace or project"),
    }
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::panic, clippy::indexing_slicing)]
mod tests {
    use std::collections::BTreeMap;

    use serde_json::{Value, json};
    use sv_package::{KeyLookup, Limits, generate_pkcs8, keypair_from_pkcs8, pack, public_key, unpack};
    use uuid::Uuid;

    use super::{Check, check_components, check_layout_slots, check_requests, compare_fields, parse_template};
    use crate::packages::open::Opened;

    const MANIFEST: &str = include_str!("../../../../../examples/plugins/service-desk-package/sv-package.json");
    const FORM: &str =
        include_str!("../../../../../examples/plugins/service-desk-package/contents/forms/requests.json");
    const LAYOUT: &str =
        include_str!("../../../../../examples/plugins/service-desk-package/contents/layouts/actions.json");
    const ACTION: &[u8] = include_bytes!("../../../../../examples/plugins/dist/service-desk-action.wasm");
    const RULE: &[u8] = include_bytes!("../../../../../examples/plugins/dist/service-desk-rule.wasm");

    /// The sample package of the real tests, signed with a throw-away key and verified again.
    fn sample() -> Opened {
        let pair = keypair_from_pkcs8(&generate_pkcs8().expect("key")).expect("pair");
        let contents: BTreeMap<String, Vec<u8>> = BTreeMap::from([
            ("contents/forms/requests.json".to_string(), FORM.as_bytes().to_vec()),
            ("contents/layouts/actions.json".to_string(), LAYOUT.as_bytes().to_vec()),
            (
                "contents/components/service-desk-action.wasm".to_string(),
                ACTION.to_vec(),
            ),
            ("contents/components/service-desk-rule.wasm".to_string(), RULE.to_vec()),
        ]);
        let manifest: Value = serde_json::from_str(MANIFEST).expect("manifest");
        let bytes = pack(manifest, &contents, &pair).expect("packed");
        let unpacked = unpack(&bytes, &Limits::default()).expect("unpacked");
        let package = unpacked
            .verify(|_| KeyLookup::Active(public_key(&pair)))
            .expect("verified");
        Opened {
            custom_code: true,
            package,
        }
    }

    /// The sample package fits a project: its component and rule load, its requests exist, its layout
    /// fits its slot, its form template is a valid schema, and the form carries the fields it requires.
    #[tokio::test]
    async fn the_sample_package_fits_a_project() {
        let opened = sample();
        let mut check = Check::default();
        check_components(&opened, &mut check).await;
        check_requests(&opened.package.manifest, &mut check);
        check_layout_slots(&opened, &mut check);
        let from = opened.package.manifest.binds["requests"]
            .create_from
            .clone()
            .expect("a template");
        let template = parse_template(&opened, "requests", &from, Uuid::new_v4(), Uuid::new_v4(), &mut check);
        assert!(check.valid(), "{:?}", check.problems);
        let template = template.expect("the template parses");
        assert_eq!(template.key, "service_requests");
        let (missing, mismatched) = compare_fields(
            &template.schema,
            &opened.package.manifest.binds["requests"].requires_fields,
        );
        assert!(missing.is_empty() && mismatched.is_empty());
        // A form without the fields is reported field by field.
        let (missing, mismatched) = compare_fields(
            &json!({"fields": [{"key": "title", "type": "number"}]}),
            &opened.package.manifest.binds["requests"].requires_fields,
        );
        assert_eq!(missing, ["assignee", "reference", "status"]);
        assert_eq!(mismatched.len(), 1);
    }

    /// A layout that does not fit the slot it is placed in is refused for that slot only.
    #[test]
    fn a_layout_is_checked_in_each_slot_it_is_placed_in() {
        let mut opened = sample();
        let mut manifest = opened.package.manifest.clone();
        manifest.layouts[0].slots = vec!["project.home.widgets".to_string(), "no.such.slot".to_string()];
        opened.package = {
            // Rebuild a verified package around the changed manifest by packing again.
            let pair = keypair_from_pkcs8(&generate_pkcs8().expect("key")).expect("pair");
            let contents: BTreeMap<String, Vec<u8>> = opened
                .package
                .files
                .iter()
                .map(|(path, data)| (path.clone(), data.clone()))
                .collect();
            let mut value = serde_json::to_value(&manifest).expect("value");
            value["files"] = json!({});
            let bytes = pack(value, &contents, &pair).expect("packed");
            unpack(&bytes, &Limits::default())
                .expect("unpacked")
                .verify(|_| KeyLookup::Active(public_key(&pair)))
                .expect("verified")
        };
        let mut check = Check::default();
        check_layout_slots(&opened, &mut check);
        let codes: Vec<(&str, &str)> = check.problems.iter().map(|p| (p.code, p.path.as_str())).collect();
        assert!(codes.contains(&(
            "slot_node_not_allowed",
            "/layouts/service.request.actions/project.home.widgets"
        )));
        assert!(codes.contains(&("slot_unknown", "/layouts/service.request.actions/no.such.slot")));
    }
}
