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
use sea_orm::{DbBackend, FromQueryResult, Statement, TransactionTrait};
use serde_json::{Map, Value, json};
use uuid::Uuid;

use super::approve::{ApproveRequest, approve};
use super::store;
use super::validate;
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

/// Installs a prepared template into `project_id` and approves it, as `admin` (a workspace owner or
/// admin who is a person; the caller has checked that).
///
/// # Errors
/// `install_invalid`, `installation_exists`, `form_key_taken`, `layout_key_taken`, `slot_occupied`,
/// `page_route_taken` from the installation flow; a database failure.
pub async fn install_prepared(
    state: &AppState,
    admin: Uuid,
    workspace_id: Uuid,
    project_id: Uuid,
    prepared: Prepared,
) -> Result<Installed, ApiError> {
    let Prepared {
        stored,
        opened,
        bind_map,
    } = prepared;
    let manifest = &opened.package.manifest;
    let checked = validate::check(state, workspace_id, Some(project_id), &opened, &bind_map).await?;
    if !checked.valid() {
        return Err(validate::invalid(&checked.problems));
    }

    let tx = state.db.begin().await?;
    let installation =
        store::insert_installation(&tx, workspace_id, Some(project_id), &manifest.id, "user", admin).await?;
    store::insert_generation(&tx, installation.id, 1, stored.id, &[], &bind_map, "user", admin).await?;
    tx.commit().await?;

    approve(
        state,
        admin,
        installation.id,
        &ApproveRequest {
            generation: 1,
            grants: Vec::new(),
        },
    )
    .await?;

    // Approval records the form each `create` bind produced in the generation's bind map.
    let generation = store::generation(&state.db, installation.id, 1)
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
                Opened { custom_code, package },
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
                if let Some(from) = &spec.create_from {
                    let template = parse_template(opened, bind, from, Uuid::new_v4(), Uuid::new_v4(), &mut check);
                    let template = template.expect("the template parses");
                    let fields = template.schema["fields"].as_array().expect("fields");
                    for required in spec.requires_fields.keys() {
                        assert!(fields.iter().any(|f| f["key"] == *required), "{name}: {required}");
                    }
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
