//! Approving a generation (contract C1 §6, C3 §5.3): one transaction that creates what the package
//! brings, stores the grants the administrator chose and makes the generation the active one.
//!
//! Approving a subset is allowed: a capability that is not granted simply does not work, and the
//! feature that needs it degrades. Approving the active generation again replaces its grants, which
//! is how an id whose operations changed (a different `plugin_ops_digest`) is honoured again.

use std::collections::{BTreeMap, BTreeSet};

use platform::app::AppState;
use sea_orm::{ConnectionTrait, DatabaseTransaction, DbBackend, Statement, TransactionTrait};
use serde::Deserialize;
use serde_json::{Value, json};
use uuid::Uuid;

use super::artefacts::{PlacementScope, create_layouts, create_pages};
use super::store::{self, GenerationRow, InstallationRow};
use super::validate::{self, Check, Resolution};
use crate::catalogue::{Refusal, RefusalCode};
use crate::error::ApiError;
use crate::extensions::rules;
use crate::forms::native_create::{NativeFormCreateRequest, create_form_in_transaction};
use crate::packages::open;

/// One grant of the approve body.
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GrantInput {
    pub capability: String,
    #[serde(default)]
    pub selector: Value,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ApproveRequest {
    pub generation: i32,
    #[serde(default)]
    pub grants: Vec<GrantInput>,
}

fn refuse(code: RefusalCode) -> ApiError {
    Refusal::new(code, None).into()
}

/// The `$bind.<name>` a grant selector may use in place of a form id: the approval substitutes the
/// form the bind resolves to (the created form's id for a `create` bind).
fn bind_name(text: &str) -> Option<&str> {
    text.strip_prefix("$bind.")
}

/// A grant after validation: the capability, its normalised selector, and the id's digest now.
pub type ValidGrant = (String, Value, String);

/// Validates the grants of an approval against the generation and the scope.
///
/// # Errors
/// `grant_not_requested`, `grant_not_plugin_exposed` or `grant_selector_invalid`; a database failure.
pub async fn validate_grants(
    db: &impl ConnectionTrait,
    installation: &InstallationRow,
    generation: &GenerationRow,
    grants: &[GrantInput],
    forms_of_binds: &BTreeMap<String, Uuid>,
) -> Result<Vec<ValidGrant>, ApiError> {
    let requested: BTreeSet<&str> = generation.requested_capabilities.iter().map(String::as_str).collect();
    let mut out: Vec<ValidGrant> = Vec::new();
    for grant in grants {
        if !requested.contains(grant.capability.as_str()) {
            return Err(refuse(RefusalCode::GrantNotRequested));
        }
        let exposed = validate::capability_info(&grant.capability).is_some_and(|info| info.plugin_exposed);
        let digest = store::current_digest(db, &grant.capability).await?;
        let (true, Some(digest)) = (exposed, digest) else {
            return Err(refuse(RefusalCode::GrantNotPluginExposed));
        };
        let selector = normalise_selector(db, installation, &grant.selector, forms_of_binds).await?;
        if !out
            .iter()
            .any(|(capability, known, _)| *capability == grant.capability && *known == selector)
        {
            out.push((grant.capability.clone(), selector, digest));
        }
    }
    Ok(out)
}

async fn normalise_selector(
    db: &impl ConnectionTrait,
    installation: &InstallationRow,
    selector: &Value,
    forms_of_binds: &BTreeMap<String, Uuid>,
) -> Result<Value, ApiError> {
    let invalid = || refuse(RefusalCode::GrantSelectorInvalid);
    let object = match selector {
        Value::Null => return Ok(json!({})),
        Value::Object(object) => object,
        _ => return Err(invalid()),
    };
    let mut out = serde_json::Map::new();
    for (key, value) in object {
        let text = value.as_str().ok_or_else(invalid)?;
        match key.as_str() {
            "project" => {
                let project = Uuid::parse_str(text).map_err(|_| invalid())?;
                let inside = match installation.project_id {
                    Some(own) => own == project,
                    None => project_in_workspace(db, installation.workspace_id, project).await?,
                };
                if !inside {
                    return Err(invalid());
                }
                out.insert("project".to_owned(), json!(project));
            }
            "form" => {
                let form = match bind_name(text) {
                    Some(name) => *forms_of_binds.get(name).ok_or_else(invalid)?,
                    None => Uuid::parse_str(text).map_err(|_| invalid())?,
                };
                if !form_in_scope(db, installation, form).await? {
                    return Err(invalid());
                }
                out.insert("form".to_owned(), json!(form));
            }
            _ => return Err(invalid()),
        }
    }
    Ok(Value::Object(out))
}

async fn project_in_workspace(
    db: &impl ConnectionTrait,
    workspace_id: Uuid,
    project_id: Uuid,
) -> Result<bool, ApiError> {
    Ok(db
        .query_one(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT 1 FROM projects WHERE id = $1 AND workspace_id = $2",
            vec![project_id.into(), workspace_id.into()],
        ))
        .await?
        .is_some())
}

async fn form_in_scope(
    db: &impl ConnectionTrait,
    installation: &InstallationRow,
    form_id: Uuid,
) -> Result<bool, ApiError> {
    Ok(db
        .query_one(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT 1 FROM project_forms WHERE id = $1 AND workspace_id = $2 AND ($3::uuid IS NULL OR project_id = $3)",
            vec![
                form_id.into(),
                installation.workspace_id.into(),
                installation.project_id.into(),
            ],
        ))
        .await?
        .is_some())
}

/// What an approval needs before it writes: the stored package, its verified bytes and the checks.
struct Prepared {
    stored: open::StoredPackage,
    opened: open::Opened,
    checked: Check,
}

/// Loads and checks what `request.generation` of the installation brings; `conn` is the database or
/// the transaction that holds the installation.
async fn prepare(
    state: &AppState,
    conn: &impl ConnectionTrait,
    installation_id: Uuid,
    request: &ApproveRequest,
) -> Result<Prepared, ApiError> {
    let tx_probe = store::load_installation(conn, installation_id)
        .await?
        .ok_or_else(|| ApiError::not_found("installation not found"))?;
    let generation = store::generation(conn, installation_id, request.generation)
        .await?
        .ok_or_else(|| refuse(RefusalCode::InstallationGenerationStale))?;
    let stored = open::find_by_row(conn, tx_probe.workspace_id, generation.package_row_id)
        .await?
        .ok_or_else(|| ApiError::not_found("package not found"))?;
    if !stored.installable || stored.key_status != "active" {
        return Err(refuse(RefusalCode::InstallationPackageRevoked));
    }
    let opened = open::open(&state.db, tx_probe.workspace_id, &stored).await?;
    let checked: Check = validate::check(
        state,
        tx_probe.workspace_id,
        tx_probe.project_id,
        &opened,
        &generation.bind_map,
    )
    .await?;
    if !checked.valid() {
        return Err(validate::invalid(&checked.problems));
    }
    let manifest = &opened.package.manifest;
    // A schema that names a rule of this package passes its save check while the forms are created.
    rules::stage(
        tx_probe.workspace_id,
        tx_probe.project_id,
        &manifest.id,
        &manifest.rules,
    );
    Ok(Prepared {
        stored,
        opened,
        checked,
    })
}

/// Rebuilds the plugin rule registry from what the database now holds.
pub async fn refresh_rules(state: &AppState) {
    if let Err(error) = rules::refresh(&state.db).await {
        tracing::warn!(target: "sv::extensions", %error, "the plugin rule registry was not refreshed");
    }
}

/// Approves `request.generation` of the installation as `admin`.
///
/// # Errors
/// `installation_revoked`, `installation_generation_stale`, `installation_package_revoked`,
/// `install_invalid` (the package no longer fits), the grant refusals, `form_key_taken`,
/// `layout_key_taken`, `slot_occupied`, or a database failure.
pub async fn approve(
    state: &AppState,
    admin: Uuid,
    installation_id: Uuid,
    request: &ApproveRequest,
) -> Result<(), ApiError> {
    let outcome = async {
        let prepared = prepare(state, &state.db, installation_id, request).await?;
        let tx = state.db.begin().await?;
        write(&tx, admin, installation_id, request, &prepared).await?;
        tx.commit().await?;
        Ok(())
    }
    .await;
    // The registry is rebuilt from what the database now holds, whatever the outcome was.
    refresh_rules(state).await;
    outcome
}

/// [`approve`] inside a transaction the caller opened and commits: the installation and its
/// generation may be rows of that transaction. The caller rebuilds the rule registry afterwards
/// ([`refresh_rules`]), whatever the outcome was.
///
/// # Errors
/// As [`approve`].
pub async fn approve_in(
    state: &AppState,
    tx: &DatabaseTransaction,
    admin: Uuid,
    installation_id: Uuid,
    request: &ApproveRequest,
) -> Result<(), ApiError> {
    let prepared = prepare(state, tx, installation_id, request).await?;
    write(tx, admin, installation_id, request, &prepared).await
}

/// Refuses `installation_package_revoked` unless the package's key is active and the package still
/// installable, holding a share lock on the key until the transaction ends.
///
/// # Errors
/// `installation_package_revoked`, or a database failure.
pub async fn ensure_key_active(
    tx: &impl ConnectionTrait,
    workspace_id: Uuid,
    stored: &open::StoredPackage,
) -> Result<(), ApiError> {
    let locked = crate::packages::keys_lock_active(tx, stored.publisher_key_id).await?;
    let installable = open::find_by_row(tx, workspace_id, stored.id)
        .await?
        .is_some_and(|row| row.installable);
    if locked && installable {
        Ok(())
    } else {
        Err(refuse(RefusalCode::InstallationPackageRevoked))
    }
}

async fn write(
    tx: &DatabaseTransaction,
    admin: Uuid,
    installation_id: Uuid,
    request: &ApproveRequest,
    prepared: &Prepared,
) -> Result<(), ApiError> {
    let Prepared {
        stored,
        opened,
        checked,
    } = prepared;
    let installation = store::lock_installation(tx, installation_id)
        .await?
        .ok_or_else(|| ApiError::not_found("installation not found"))?;
    // The key was checked before the (slow) verification; check it again under a lock that makes a
    // concurrent revocation wait, so a revoked key never ends up with an active installation.
    ensure_key_active(tx, installation.workspace_id, stored).await?;
    if installation.status == "revoked" {
        return Err(refuse(RefusalCode::InstallationRevoked));
    }
    let generation = store::generation(tx, installation_id, request.generation)
        .await?
        .ok_or_else(|| refuse(RefusalCode::InstallationGenerationStale))?;
    let reapproval = generation.status == "approved" && installation.active_generation == Some(generation.generation);
    if generation.status != "pending" && !reapproval {
        return Err(refuse(RefusalCode::InstallationGenerationStale));
    }

    // Forms of `create` binds first: a grant selector may name them.
    let mut forms_of_binds: BTreeMap<String, Uuid> = BTreeMap::new();
    for (name, resolution) in &checked.resolutions {
        if let Resolution::Existing(id) = resolution {
            forms_of_binds.insert(name.clone(), *id);
        }
    }
    let mut bind_map = generation.bind_map.clone();
    if !reapproval {
        for template in &checked.templates {
            // `validate::check` accepts a `create` bind only in a project scope.
            let project_id = installation.project_id.ok_or_else(ApiError::internal)?;
            let taken = tx
                .query_one(Statement::from_sql_and_values(
                    DbBackend::Postgres,
                    "SELECT 1 FROM project_forms WHERE project_id = $1 AND key = $2",
                    vec![project_id.into(), template.key.clone().into()],
                ))
                .await?
                .is_some();
            if taken {
                return Err(ApiError::conflict_coded(
                    "form_key_taken",
                    "the project already has a form with the key the package creates",
                    None,
                ));
            }
            let form_id = Uuid::new_v4();
            create_form_in_transaction(
                tx,
                NativeFormCreateRequest {
                    form_id: Some(form_id),
                    workspace_id: installation.workspace_id,
                    project_id,
                    key: template.key.clone(),
                    name: template.name.clone(),
                    description: template.description.clone(),
                    icon: template.icon.clone(),
                    color: template.color.clone(),
                    title_template: template.title_template.clone(),
                    schema: template.schema.clone(),
                    detail_layout: template.detail_layout.clone(),
                    created_by: Some(admin),
                    source: json!({
                        "type": "user",
                        "actor_id": admin,
                        "origin": "package_install",
                        "installation": installation_id,
                        "generation": generation.generation,
                    }),
                    change_summary: "created by a package installation".to_string(),
                },
            )
            .await?;
            forms_of_binds.insert(template.bind.clone(), form_id);
            // A form the package created is an ordinary form from now on: the bind names it.
            if let Some(binds) = bind_map.as_object_mut() {
                binds.insert(template.bind.clone(), json!({ "form_id": form_id }));
            }
        }
    }
    let grants = validate_grants(tx, &installation, &generation, &request.grants, &forms_of_binds).await?;

    if !reapproval {
        tx.execute(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "UPDATE installation_generations SET bind_map = $2 WHERE id = $1",
            vec![generation.id.into(), bind_map.into()],
        ))
        .await?;
        tx.execute(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "UPDATE installation_generations SET status = 'superseded' \
             WHERE installation_id = $1 AND status = 'approved' AND generation <> $2",
            vec![installation_id.into(), generation.generation.into()],
        ))
        .await?;
    }
    store::review_generation(tx, generation.id, "approved", admin).await?;
    // `active` unless an administrator disabled it; a lost approver or a revoked key is cured by this
    // approval (the package verified again above).
    tx.execute(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "UPDATE installations SET active_generation = $2, \
           status = CASE WHEN status = 'disabled' AND disabled_reason = 'admin' THEN 'disabled' ELSE 'active' END, \
           disabled_reason = CASE WHEN status = 'disabled' AND disabled_reason = 'admin' THEN 'admin' ELSE NULL END, \
           updated_at = now() \
         WHERE id = $1",
        vec![installation_id.into(), generation.generation.into()],
    ))
    .await?;
    store::replace_grants(tx, generation.id, &grants, admin).await?;
    if !reapproval {
        let scope = PlacementScope {
            workspace_id: installation.workspace_id,
            project_id: installation.project_id,
        };
        let layouts = create_layouts(
            tx,
            &opened.package.manifest,
            &opened.package.files,
            scope,
            installation_id,
            generation.generation,
            admin,
        )
        .await?;
        create_pages(
            tx,
            &opened.package.manifest,
            &layouts,
            scope,
            installation_id,
            generation.generation,
            admin,
        )
        .await?;
    }
    Ok(())
}
