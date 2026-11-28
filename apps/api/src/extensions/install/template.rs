//! Installing a built-in template package into a project (contract C3 §4.1 and §5.3).
//!
//! Project creation and the "new form" chooser use this instead of hard-coded templates: the
//! template is a signed built-in package, and it goes through the ordinary installation flow
//! (request, then approve) so the installation, the forms and the pages are recorded like any other.
//!
//! The installation is approved on the spot only when the review would have nothing to decide: the
//! package is built-in (signed with the key of publisher `sv`), has no code (`custom_code` is
//! false, so no actions, rules or `custom-frame`), asks for no capability, and the caller is a
//! workspace owner or admin who is a person. Every other package goes through the console review.
//! A template never writes form permissions, project agent policy or roles (contract C3 §5.3).

use platform::app::AppState;
use sea_orm::{ConnectionTrait, DatabaseTransaction, DbBackend, FromQueryResult, Statement, TransactionTrait};
use serde_json::{Map, Value, json};
use uuid::Uuid;

use super::approve::{ApproveRequest, approve_in, refresh_rules};
use super::store;
use crate::error::ApiError;
use crate::packages::open;

/// What a template install created.
#[derive(Debug, Clone)]
pub struct Installed {
    pub installation_id: Uuid,
    pub package_key: String,
    pub version: String,
    /// The forms the package created, by bind name.
    pub forms: Vec<(String, Uuid)>,
}

#[derive(FromQueryResult)]
struct Digest {
    digest: String,
}

fn not_a_template(message: &str) -> ApiError {
    ApiError::bad_request_coded("template_package_invalid", message)
}

/// The digest of the newest built-in package with this key that can still be installed.
async fn newest_builtin_digest(state: &AppState, package_key: &str) -> Result<String, ApiError> {
    Digest::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "SELECT p.digest FROM packages p JOIN publisher_keys k ON k.id = p.publisher_key_id \
         WHERE p.tier = 'builtin' AND p.package_key = $1 AND p.installable AND k.status = 'active' \
         ORDER BY p.created_at DESC, p.id LIMIT 1",
        vec![package_key.to_owned().into()],
    ))
    .one(&state.db)
    .await?
    .map(|row| row.digest)
    .ok_or_else(|| ApiError::not_found("template package not found"))
}

/// The bind map that creates every form of the package.
fn create_all(binds: impl Iterator<Item = String>) -> Value {
    Value::Object(
        binds
            .map(|name| (name, json!({ "create": true })))
            .collect::<Map<_, _>>(),
    )
}

/// A built-in template package that passed every check that does not need the project.
pub struct Prepared {
    stored: open::StoredPackage,
    opened: open::Opened,
    bind_map: Value,
}

impl Prepared {
    /// The id and version of the package.
    #[must_use]
    pub fn package(&self) -> (&str, &str) {
        let manifest = &self.opened.package.manifest;
        (&manifest.id, &manifest.version)
    }
}

/// Finds the newest built-in package `package_key` and checks that it may be installed without a
/// review, so a caller can refuse before it creates anything.
///
/// # Errors
/// `not_found` when no such built-in package exists; `template_package_invalid` when the package is
/// not a template that may be approved without a review (it has code or asks for capabilities, is a
/// plugin, or binds to an existing form); a database failure.
pub async fn prepare(state: &AppState, workspace_id: Uuid, package_key: &str) -> Result<Prepared, ApiError> {
    let review = "the package has code or needs a capability review: install it from the project's installations";
    let digest = newest_builtin_digest(state, package_key).await?;
    let stored = open::find(&state.db, workspace_id, &digest)
        .await?
        .ok_or_else(|| ApiError::not_found("template package not found"))?;
    if stored.tier != "builtin" || !stored.installable || stored.key_status != "active" {
        return Err(not_a_template("the package is not an installable built-in package"));
    }
    if stored.custom_code || stored.kind == "plugin" {
        return Err(not_a_template(review));
    }
    let opened = open::open(&state.db, workspace_id, &stored).await?;
    let manifest = &opened.package.manifest;
    if opened.custom_code || !manifest.requests.is_empty() {
        return Err(not_a_template(review));
    }
    if manifest.binds.values().any(|bind| bind.create_from.is_none()) {
        return Err(not_a_template(
            "the package binds to an existing form and is not a template",
        ));
    }
    // Every bind creates its form: a template is installed into a project that has nothing to bind to.
    let bind_map = create_all(manifest.binds.keys().cloned());
    Ok(Prepared {
        stored,
        opened,
        bind_map,
    })
}

/// Installs a prepared template into `project_id` and approves it, as `admin`.
///
/// Runs inside the caller's transaction (the project may be a row of it). The caller commits, then
/// rebuilds the rule registry with [`approve::refresh_rules`](super::approve::refresh_rules).
///
/// # Errors
/// `install_invalid`, `installation_exists`, `form_key_taken`, `layout_key_taken`, `slot_occupied`,
/// `page_route_taken` from the installation flow; a database failure.
pub async fn install_prepared_in(
    state: &AppState,
    tx: &DatabaseTransaction,
    admin: Uuid,
    workspace_id: Uuid,
    project_id: Uuid,
    prepared: &Prepared,
) -> Result<Installed, ApiError> {
    let manifest = &prepared.opened.package.manifest;
    let installation =
        store::insert_installation(tx, workspace_id, Some(project_id), &manifest.id, "user", admin).await?;
    store::insert_generation(
        tx,
        installation.id,
        1,
        prepared.stored.id,
        &[],
        &prepared.bind_map,
        "user",
        admin,
    )
    .await?;
    approve_in(
        state,
        tx,
        admin,
        installation.id,
        &ApproveRequest {
            generation: 1,
            grants: Vec::new(),
        },
    )
    .await?;

    // Approval records the form each `create` bind produced in the generation's bind map.
    let generation = store::generation(tx, installation.id, 1)
        .await?
        .ok_or_else(ApiError::internal)?;
    let forms = generation
        .bind_map
        .as_object()
        .into_iter()
        .flatten()
        .filter_map(|(name, choice)| {
            let id = choice.get("form_id")?.as_str()?;
            Some((name.clone(), Uuid::parse_str(id).ok()?))
        })
        .collect();
    Ok(Installed {
        installation_id: installation.id,
        package_key: manifest.id.clone(),
        version: manifest.version.clone(),
        forms,
    })
}

/// Installs a prepared template into an existing project in one transaction. The package's
/// `project` block is not applied to an existing project.
///
/// # Errors
/// As [`install_prepared_in`].
pub async fn install_prepared(
    state: &AppState,
    admin: Uuid,
    workspace_id: Uuid,
    project_id: Uuid,
    prepared: Prepared,
) -> Result<Installed, ApiError> {
    let outcome = async {
        let tx = state.db.begin().await?;
        let installed = install_prepared_in(state, &tx, admin, workspace_id, project_id, &prepared).await?;
        tx.commit().await?;
        Ok(installed)
    }
    .await;
    refresh_rules(state).await;
    outcome
}

/// The name of an i18n map for `locale`, falling back to `en`.
fn localised<'a>(map: &'a std::collections::BTreeMap<String, String>, locale: &str) -> &'a str {
    map.get(locale).or_else(|| map.get("en")).map_or("", String::as_str)
}

impl Prepared {
    /// The project type the package names, if any.
    #[must_use]
    pub fn type_key(&self) -> Option<&str> {
        self.opened
            .package
            .manifest
            .project
            .as_ref()
            .and_then(|project| project.type_key.as_deref())
    }

    /// The workflow id created for the project when the package has workflow states.
    fn project_block(&self) -> Option<&sv_package::ProjectBlock> {
        self.opened.package.manifest.project.as_ref()
    }
}

/// Inserts the workflow of a project block with its states and points the project at it.
async fn insert_workflow(
    tx: &DatabaseTransaction,
    states: &[sv_package::ProjectState],
    workspace_id: Uuid,
    project_id: Uuid,
    project_label: &str,
    package_key: &str,
    locale: &str,
) -> Result<(), ApiError> {
    let workflow_id = Uuid::new_v4();
    tx.execute(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "INSERT INTO workflows (id, workspace_id, name, description, is_system_default, created_at, updated_at) \
         VALUES ($1, $2, $3, $4, false, now(), now())",
        vec![
            workflow_id.into(),
            workspace_id.into(),
            project_label.to_owned().into(),
            format!("Created from template {package_key}").into(),
        ],
    ))
    .await?;
    for (index, state) in states.iter().enumerate() {
        let position = i32::try_from(index + 1).map_err(|_| ApiError::internal())?;
        tx.execute(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "INSERT INTO workflow_states (id, workflow_id, key, display_name, category, position, color, \
                                          is_initial, is_terminal, created_at, updated_at) \
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, now(), now())",
            vec![
                Uuid::new_v4().into(),
                workflow_id.into(),
                state.key.clone().into(),
                localised(&state.name, locale).to_owned().into(),
                state.category.clone().into(),
                position.into(),
                state.color.clone().into(),
                state.initial.into(),
                matches!(state.category.as_str(), "completed" | "cancelled").into(),
            ],
        ))
        .await?;
    }
    tx.execute(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "UPDATE projects SET workflow_id = $2 WHERE id = $1",
        vec![project_id.into(), workflow_id.into()],
    ))
    .await?;
    Ok(())
}

/// Creates the workflow and the labels of the package's `project` block for a new project.
///
/// Points the project at the workflow and returns whether a workflow was created. Runs on the
/// create's transaction; the labels are the workspace's, matched by `lower(name)`, and an existing
/// label is kept as it is.
///
/// # Errors
/// A database failure.
pub async fn apply_project_block(
    tx: &DatabaseTransaction,
    prepared: &Prepared,
    workspace_id: Uuid,
    project_id: Uuid,
    project_name: &str,
    project_key: &str,
    locale: &str,
) -> Result<bool, ApiError> {
    let Some(block) = prepared.project_block() else {
        return Ok(false);
    };
    let package_key = &prepared.opened.package.manifest.id;
    let created = if let Some(states) = &block.workflow_states {
        let project_label = format!("{project_name} ({project_key})");
        insert_workflow(
            tx,
            states,
            workspace_id,
            project_id,
            &project_label,
            package_key,
            locale,
        )
        .await?;
        true
    } else {
        false
    };
    for label in block.labels.iter().flatten() {
        tx.execute(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "INSERT INTO labels (id, workspace_id, name, color, description, created_at) \
             VALUES ($1, $2, $3, $4, $5, now()) ON CONFLICT (workspace_id, lower(name)) DO NOTHING",
            vec![
                Uuid::new_v4().into(),
                workspace_id.into(),
                localised(&label.name, locale).to_owned().into(),
                label.color.clone().into(),
                format!("Created from template {package_key}").into(),
            ],
        ))
        .await?;
    }
    Ok(created)
}

/// What creating a project from a template would apply, for the create-from-template preview. Writes
/// nothing and carries no ids.
///
/// # Errors
/// As [`prepare`]; `bad_request` for a malformed or unknown `type_key`.
pub async fn preview_create(
    state: &AppState,
    workspace_id: Uuid,
    package_key: &str,
    caller_type_key: Option<&str>,
    auto_approve: bool,
    locale: &str,
) -> Result<Value, ApiError> {
    let prepared = prepare(state, workspace_id, package_key).await?;
    let manifest = &prepared.opened.package.manifest;
    if let Some(type_key) = caller_type_key {
        let known = state
            .db
            .query_one(Statement::from_sql_and_values(
                DbBackend::Postgres,
                "SELECT 1 FROM project_types WHERE key = $1",
                vec![type_key.to_owned().into()],
            ))
            .await?
            .is_some();
        if !known {
            return Err(ApiError::BadRequest("project type does not exist".to_string()));
        }
    }
    let (type_key, source) = match (caller_type_key, prepared.type_key()) {
        (Some(key), _) => (key.to_owned(), "caller"),
        (None, Some(key)) => (key.to_owned(), "manifest"),
        (None, None) => ("custom_form".to_owned(), "default"),
    };
    let block = prepared.project_block();
    let states: Vec<Value> = block
        .and_then(|block| block.workflow_states.as_ref())
        .into_iter()
        .flatten()
        .map(|state| {
            json!({
                "key": state.key,
                "name": localised(&state.name, locale),
                "category": state.category,
                "initial": state.initial,
                "terminal": matches!(state.category.as_str(), "completed" | "cancelled"),
                "color": state.color,
            })
        })
        .collect();
    let mut labels = Vec::new();
    for label in block.and_then(|block| block.labels.as_ref()).into_iter().flatten() {
        let name = localised(&label.name, locale);
        let exists = state
            .db
            .query_one(Statement::from_sql_and_values(
                DbBackend::Postgres,
                "SELECT 1 FROM labels WHERE workspace_id = $1 AND lower(name) = lower($2)",
                vec![workspace_id.into(), name.to_owned().into()],
            ))
            .await?
            .is_some();
        labels.push(json!({ "name": name, "color": label.color, "exists": exists }));
    }
    Ok(json!({
        "package": {
            "package_key": manifest.id,
            "version": manifest.version,
            "kind": serde_json::to_value(manifest.kind).unwrap_or(Value::Null),
        },
        "project_settings": "applied",
        "auto_approve": auto_approve,
        "type_key": {"value": type_key, "source": source},
        "workflow": {
            "source": if states.is_empty() { "workspace_default" } else { "template" },
            "states": states,
        },
        "labels": labels,
        "forms": manifest.binds.values().filter_map(|bind| bind.create_from.as_deref()).collect::<Vec<_>>(),
        "pages": manifest.pages.iter().map(|page| page.route.as_str()).collect::<Vec<_>>(),
        "layouts": manifest.layouts.iter().map(|layout| layout.key.as_str()).collect::<Vec<_>>(),
    }))
}

/// [`prepare`] and [`install_prepared`] in one call.
///
/// # Errors
/// As [`prepare`] and [`install_prepared`].
pub async fn install_builtin(
    state: &AppState,
    admin: Uuid,
    workspace_id: Uuid,
    project_id: Uuid,
    package_key: &str,
) -> Result<Installed, ApiError> {
    let prepared = prepare(state, workspace_id, package_key).await?;
    install_prepared(state, admin, workspace_id, project_id, prepared).await
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::panic, clippy::indexing_slicing)]
mod tests {
    use std::collections::{BTreeMap, BTreeSet};
    use std::path::{Path, PathBuf};

    use serde_json::Value;
    use sv_layout::{Context, Options};
    use sv_package::{KeyLookup, Kind, Limits, generate_pkcs8, keypair_from_pkcs8, pack, public_key, unpack};
    use uuid::Uuid;

    use super::super::validate::{Check, check_components, check_layout_slots, check_requests, parse_template};
    use crate::packages::open::Opened;

    fn builtin_dir() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../packages/builtin")
    }

    fn collect(root: &Path, dir: &Path, out: &mut BTreeMap<String, Vec<u8>>) {
        for entry in std::fs::read_dir(dir).expect("directory") {
            let path = entry.expect("entry").path();
            if path.is_dir() {
                collect(root, &path, out);
            } else {
                let rel = path
                    .strip_prefix(root)
                    .expect("inside")
                    .to_string_lossy()
                    .replace('\\', "/");
                out.insert(rel, std::fs::read(&path).expect("file"));
            }
        }
    }

    /// Every source directory packed with a throw-away key, as `build.sh` does, and verified again.
    fn packages() -> Vec<(String, Opened)> {
        let pair = keypair_from_pkcs8(&generate_pkcs8().expect("key")).expect("pair");
        let mut out = Vec::new();
        let mut dirs: Vec<PathBuf> = std::fs::read_dir(builtin_dir())
            .expect("packages/builtin")
            .map(|entry| entry.expect("entry").path())
            .filter(|path| path.join("sv-package.json").is_file())
            .collect();
        dirs.sort();
        for dir in dirs {
            let mut contents = BTreeMap::new();
            collect(&dir, &dir.join("contents"), &mut contents);
            if let Ok(list) = std::fs::read_to_string(dir.join("components.list")) {
                let dist = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples/plugins/dist");
                for name in list.lines().filter(|line| !line.trim().is_empty()) {
                    contents.insert(
                        format!("contents/components/{name}.wasm"),
                        std::fs::read(dist.join(format!("{name}.wasm"))).expect("component"),
                    );
                }
            }
            let manifest: Value =
                serde_json::from_slice(&std::fs::read(dir.join("sv-package.json")).expect("manifest")).expect("json");
            let bytes = pack(manifest, &contents, &pair).expect("the package packs");
            let package = unpack(&bytes, &Limits::default())
                .expect("unpacked")
                .verify(|_| KeyLookup::Active(public_key(&pair)))
                .expect("verified");
            let custom_code = package.manifest.computed_custom_code();
            out.push((
                dir.file_name().expect("name").to_string_lossy().into_owned(),
                Opened { package, custom_code },
            ));
        }
        out
    }

    /// Every built-in package packs, verifies and fits an empty project: its form templates are valid
    /// schemas, its layouts fit their slots or the site, its components load, its requests exist.
    #[tokio::test]
    async fn every_built_in_package_fits_an_empty_project() {
        let packages = packages();
        assert_eq!(
            packages.len(),
            8,
            "{:?}",
            packages.iter().map(|(n, _)| n).collect::<Vec<_>>()
        );
        let mut ids = BTreeSet::new();
        for (name, opened) in &packages {
            let manifest = &opened.package.manifest;
            assert_eq!(manifest.id, format!("sv/{name}"));
            assert!(ids.insert(manifest.id.clone()));
            let mut check = Check::default();
            check_components(opened, &mut check).await;
            check_requests(manifest, &mut check);
            check_layout_slots(opened, &mut check);
            for (bind, spec) in &manifest.binds {
                let Some(from) = &spec.create_from else {
                    continue;
                };
                let template = parse_template(opened, bind, from, Uuid::new_v4(), Uuid::new_v4(), &mut check);
                let template = template.expect("the template parses");
                let fields = template.schema["fields"].as_array().expect("fields");
                for required in spec.requires_fields.keys() {
                    assert!(fields.iter().any(|f| f["key"] == *required), "{name}: {required}");
                }
            }
            let bindings: BTreeSet<String> = manifest.binds.keys().cloned().collect();
            for page in &manifest.pages {
                let entry = manifest.layouts.iter().find(|l| l.key == page.layout).expect("layout");
                let opts = Options {
                    slot: None,
                    context: Context::Site {
                        bindings: bindings.clone(),
                    },
                };
                let bytes = opened.package.files.get(&entry.path).expect("file");
                sv_layout::validate_bytes(bytes, &opts).expect("the page layout fits the site");
            }
            assert!(check.valid(), "{name}: {:?}", check.problems);
            // Only the service desk has code; every other package may be approved without a review.
            assert_eq!(opened.custom_code, name == "service-desk", "{name}");
            if opened.custom_code {
                assert!(matches!(manifest.kind, Kind::App));
            } else {
                assert!(manifest.requests.is_empty());
                assert!(manifest.binds.values().all(|bind| bind.create_from.is_some()));
            }
        }
    }

    /// The project blocks of the built-in packages are what the old scenario templates had: a type,
    /// five or six states ending in a completed one, and four labels, each named in both languages.
    #[test]
    fn the_scenario_templates_keep_their_type_states_and_labels() {
        let expected = [
            ("code-delivery", "code_project", 6),
            ("contract-review", "custom_form", 5),
            ("equipment-maintenance", "custom_form", 5),
            ("quality-corrective-action", "custom_form", 5),
            ("customer-delivery", "custom_form", 5),
            ("restaurant-ordering", "custom_form", 5),
        ];
        let packages = packages();
        for (name, type_key, states) in expected {
            let (_, opened) = packages.iter().find(|(n, _)| n == name).expect("package");
            let block = opened.package.manifest.project.as_ref().expect("project block");
            assert_eq!(block.type_key.as_deref(), Some(type_key), "{name}");
            let workflow = block.workflow_states.as_ref().expect("states");
            assert_eq!(workflow.len(), states, "{name}");
            assert_eq!(workflow.iter().filter(|s| s.initial).count(), 1, "{name}");
            assert!(workflow.iter().any(|s| s.category == "completed"), "{name}");
            let labels = block.labels.as_ref().expect("labels");
            assert_eq!(labels.len(), 4, "{name}");
            assert!(
                labels
                    .iter()
                    .all(|l| l.name.contains_key("en") && l.name.contains_key("zh"))
            );
        }
    }

    /// The templates use the typed fields the contract asks for.
    #[test]
    fn owners_approvers_phones_and_signatures_use_their_field_types() {
        let types: BTreeMap<(&str, &str), &str> = BTreeMap::from([
            (("release_check", "owner"), "member"),
            (("risk_clause", "owner"), "member"),
            (("approval_record", "approver"), "member"),
            (("equipment", "owner"), "member"),
            (("repair_order", "repair_owner"), "member"),
            (("corrective_action", "owner"), "member"),
            (("customer", "phone"), "phone"),
            (("inspection_record", "signature"), "signature"),
        ]);
        let mut seen = BTreeSet::new();
        for (name, opened) in packages() {
            for (path, bytes) in &opened.package.files {
                if !path.starts_with("contents/forms/") {
                    continue;
                }
                let form: Value = serde_json::from_slice(bytes).expect("form");
                let key = form["key"].as_str().expect("key").to_owned();
                for field in form["schema"]["fields"].as_array().expect("fields") {
                    let field_key = field["key"].as_str().expect("field key");
                    if let Some(expected) = types.get(&(key.as_str(), field_key)) {
                        assert_eq!(field["type"], *expected, "{name}/{key}.{field_key}");
                        seen.insert((key.clone(), field_key.to_owned()));
                    }
                }
            }
        }
        assert_eq!(seen.len(), types.len(), "{seen:?}");
    }
}
