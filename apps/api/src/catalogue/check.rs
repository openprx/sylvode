//! The route-level checker: may this principal call this operation on this resource?

use super::principal::{InstallationView, Principal, Resource};
use super::refusal::{Refusal, RefusalCode};
use super::types::{Entry, Exposure};

/// The channel a principal calls through.
const fn channel(principal: &Principal) -> Exposure {
    match principal {
        Principal::User { .. } => Exposure::CONSOLE,
        Principal::Bot { .. } => Exposure::BOT,
        Principal::Delegated { .. } => Exposure::OAUTH,
        Principal::Plugin { .. } | Principal::Installation { .. } => Exposure::PLUGIN,
    }
}

/// Answers the route-level question; role, bot level in a project, form policy, field policy and
/// record scope stay with the handlers and `scoped_bot`, which see the same actor. The first
/// failure returns:
///
/// 1. the entry's exposure must contain the principal's channel;
/// 2. a user, a bot or a delegated caller needs nothing more here;
/// 3. a plugin or an installation needs the id in the generation's requested ids, a grant with
///    this id, a target in its own workspace, and a grant whose selector matches the target.
///
/// This is [`allowed_pre`] followed by [`allowed_target`]. The host calls the two halves around the
/// resolution of the target (see [`allowed_pre`]).
///
/// # Errors
/// The [`Refusal`] of the first failed step.
pub fn allowed(principal: &Principal, entry: &Entry, resource: &Resource) -> Result<(), Refusal> {
    allowed_pre(principal, entry)?;
    allowed_target(principal, entry, resource)
}

/// Steps 1 and 3a-3b: everything that can be decided without looking at the target.
///
/// A plugin host runs this **before** it resolves the target, so that a guest which is not allowed
/// to use an id at all learns nothing from the target it names, and runs [`allowed_target`] on the
/// resolved target only after this half passed. A target that cannot be resolved is then answered
/// with the same refusal as a target outside the installation's workspace
/// ([`RefusalCode::CapabilityScopeMismatch`]), so the answer never says whether an id exists.
///
/// # Errors
/// The [`Refusal`] of the first failed step.
pub fn allowed_pre(principal: &Principal, entry: &Entry) -> Result<(), Refusal> {
    let capability = entry.capability();
    let refuse = |code| Refusal::new(code, Some(capability));
    if !entry.exposure.contains(channel(principal)) {
        return Err(refuse(RefusalCode::CapabilityNotExposed));
    }
    let installation = match principal {
        Principal::User { .. } | Principal::Bot { .. } | Principal::Delegated { .. } => return Ok(()),
        Principal::Plugin { installation, .. } | Principal::Installation { installation, .. } => installation,
    };
    check_requested_and_granted(installation, capability).map_err(refuse)
}

/// Steps 3c-3d: the target against the installation's workspace, project and grant selectors.
/// Only meaningful after [`allowed_pre`] passed; users, bots and delegated callers pass.
///
/// # Errors
/// The [`Refusal`] of the first failed step.
pub fn allowed_target(principal: &Principal, entry: &Entry, resource: &Resource) -> Result<(), Refusal> {
    let capability = entry.capability();
    let installation = match principal {
        Principal::User { .. } | Principal::Bot { .. } | Principal::Delegated { .. } => return Ok(()),
        Principal::Plugin { installation, .. } | Principal::Installation { installation, .. } => installation,
    };
    check_target(installation, capability, resource).map_err(|code| Refusal::new(code, Some(capability)))
}

fn check_requested_and_granted(installation: &InstallationView, capability: &str) -> Result<(), RefusalCode> {
    if !installation.requested.contains(capability) {
        return Err(RefusalCode::CapabilityNotRequested);
    }
    if installation.grants.iter().any(|grant| grant.capability == capability) {
        Ok(())
    } else {
        Err(RefusalCode::CapabilityNotGranted)
    }
}

/// A target outside the installation's workspace is a scope mismatch; every other mismatch of
/// project or selector is `not_granted`, so a plugin learns nothing about objects outside its
/// grants. A selector key the resource cannot supply never matches.
fn check_target(installation: &InstallationView, capability: &str, resource: &Resource) -> Result<(), RefusalCode> {
    if resource.workspace_id != installation.workspace_id {
        return Err(RefusalCode::CapabilityScopeMismatch);
    }
    let in_project = installation
        .project_id
        .is_none_or(|project| resource.project_id == Some(project));
    let matched = in_project
        && installation
            .grants
            .iter()
            .filter(|grant| grant.capability == capability)
            .any(|grant| {
                grant.project.is_none_or(|project| resource.project_id == Some(project))
                    && grant.form.is_none_or(|form| resource.form_id == Some(form))
            });
    if matched {
        Ok(())
    } else {
        Err(RefusalCode::CapabilityNotGranted)
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use uuid::Uuid;

    use super::{allowed, allowed_pre, allowed_target};
    use crate::catalogue::principal::{Grant, InstallationView, Invoker, Principal, Resource};
    use crate::catalogue::refusal::RefusalCode;
    use crate::catalogue::types::dsl::*;
    use crate::entities::workspace_member::WorkspaceRole;

    const PLUGIN_READ: Entry = bot("forms.records.read", Get, "/x/{id}", Project(Find::Project("id")), Read).plugin();
    const CONSOLE_ONLY: Entry = console(
        "forms.records.delete",
        Delete,
        "/x/{id}",
        Project(Find::Project("id")),
        Destructive,
    );

    fn installation(workspace: Uuid, project: Option<Uuid>, grants: Vec<Grant>) -> InstallationView {
        InstallationView {
            installation_id: Uuid::from_u128(1),
            generation: 1,
            workspace_id: workspace,
            project_id: project,
            requested: BTreeSet::from(["forms.records.read".to_string()]),
            grants,
        }
    }

    fn grant(project: Option<Uuid>, form: Option<Uuid>) -> Grant {
        Grant {
            capability: "forms.records.read".to_string(),
            project,
            form,
        }
    }

    fn plugin(installation: InstallationView) -> Principal {
        Principal::Plugin {
            invoker: Invoker::User {
                user_id: Uuid::from_u128(9),
                workspace_id: installation.workspace_id,
                role: WorkspaceRole::Member,
            },
            installation,
        }
    }

    fn code(result: Result<(), crate::catalogue::refusal::Refusal>) -> Option<RefusalCode> {
        result.err().map(|refusal| refusal.code)
    }

    /// A user, a bot and a delegated caller pass when the channel is exposed; nothing else is checked here.
    #[test]
    fn user_bot_and_delegated_need_only_the_channel() {
        let workspace = Uuid::from_u128(2);
        let resource = Resource {
            workspace_id: workspace,
            project_id: None,
            form_id: None,
        };
        let user = Principal::User {
            user_id: Uuid::from_u128(3),
            workspace_id: workspace,
            role: WorkspaceRole::Member,
        };
        assert_eq!(code(allowed(&user, &CONSOLE_ONLY, &resource)), None);
        let delegated = Principal::Delegated {
            client_id: Uuid::from_u128(4),
            user_id: Uuid::from_u128(3),
            workspace_id: workspace,
        };
        assert_eq!(
            code(allowed(&delegated, &CONSOLE_ONLY, &resource)),
            Some(RefusalCode::CapabilityNotExposed)
        );
        assert_eq!(
            code(allowed(&delegated, &PLUGIN_READ, &resource)),
            Some(RefusalCode::CapabilityNotExposed)
        );
    }

    /// The plugin steps: exposure, request, grant, workspace, project and selector, in this order.
    #[test]
    fn a_plugin_needs_exposure_scope_request_and_grant() {
        let workspace = Uuid::from_u128(2);
        let project = Uuid::from_u128(5);
        let form = Uuid::from_u128(6);
        let resource = Resource {
            workspace_id: workspace,
            project_id: Some(project),
            form_id: Some(form),
        };
        let granted = plugin(installation(workspace, Some(project), vec![grant(None, None)]));
        assert_eq!(code(allowed(&granted, &PLUGIN_READ, &resource)), None);
        // Not exposed to plugins.
        assert_eq!(
            code(allowed(&granted, &CONSOLE_ONLY, &resource)),
            Some(RefusalCode::CapabilityNotExposed)
        );
        // Another workspace, another project, no project.
        let other_workspace = Resource {
            workspace_id: Uuid::from_u128(7),
            ..resource
        };
        let other_project = Resource {
            project_id: Some(Uuid::from_u128(8)),
            ..resource
        };
        let no_project = Resource {
            project_id: None,
            ..resource
        };
        assert_eq!(
            code(allowed(&granted, &PLUGIN_READ, &other_workspace)),
            Some(RefusalCode::CapabilityScopeMismatch)
        );
        for target in [other_project, no_project] {
            assert_eq!(
                code(allowed(&granted, &PLUGIN_READ, &target)),
                Some(RefusalCode::CapabilityNotGranted)
            );
        }
        // Not requested by the manifest.
        let mut unrequested = installation(workspace, Some(project), vec![grant(None, None)]);
        unrequested.requested.clear();
        assert_eq!(
            code(allowed(&plugin(unrequested), &PLUGIN_READ, &resource)),
            Some(RefusalCode::CapabilityNotRequested)
        );
        // Requested but not granted.
        assert_eq!(
            code(allowed(
                &plugin(installation(workspace, Some(project), vec![])),
                &PLUGIN_READ,
                &resource
            )),
            Some(RefusalCode::CapabilityNotGranted)
        );
    }

    /// A selector key must equal the target; an absent key is any value within the installation scope.
    #[test]
    fn a_grant_selector_narrows_the_target() {
        let workspace = Uuid::from_u128(2);
        let project = Uuid::from_u128(5);
        let form = Uuid::from_u128(6);
        let resource = Resource {
            workspace_id: workspace,
            project_id: Some(project),
            form_id: Some(form),
        };
        let by_form = |selected: Uuid| {
            plugin(installation(
                workspace,
                None,
                vec![grant(Some(project), Some(selected))],
            ))
        };
        assert_eq!(code(allowed(&by_form(form), &PLUGIN_READ, &resource)), None);
        assert_eq!(
            code(allowed(&by_form(Uuid::from_u128(99)), &PLUGIN_READ, &resource)),
            Some(RefusalCode::CapabilityNotGranted)
        );
        let by_project = plugin(installation(
            workspace,
            None,
            vec![grant(Some(Uuid::from_u128(99)), None)],
        ));
        assert_eq!(
            code(allowed(&by_project, &PLUGIN_READ, &resource)),
            Some(RefusalCode::CapabilityNotGranted)
        );
        // A grant for another capability does not count.
        let mut other = grant(None, None);
        other.capability = "forms.records.update".to_string();
        assert_eq!(
            code(allowed(
                &plugin(installation(workspace, None, vec![other])),
                &PLUGIN_READ,
                &resource
            )),
            Some(RefusalCode::CapabilityNotGranted)
        );
        // A workspace installation with an unselected grant reaches any project of the workspace.
        let any = plugin(installation(workspace, None, vec![grant(None, None)]));
        assert_eq!(code(allowed(&any, &PLUGIN_READ, &resource)), None);
    }

    /// The two halves of the check: the first needs no target, the second judges the resolved one,
    /// and together they equal [`allowed`]. A host that resolves a target only after the first half
    /// passed never lets a guest probe an id it may not use.
    #[test]
    fn the_check_splits_into_a_pre_resolution_and_a_target_half() {
        let workspace = Uuid::from_u128(2);
        let project = Uuid::from_u128(5);
        let granted = plugin(installation(workspace, Some(project), vec![grant(Some(project), None)]));
        assert_eq!(code(allowed_pre(&granted, &PLUGIN_READ)), None);
        assert_eq!(
            code(allowed_pre(&granted, &CONSOLE_ONLY)),
            Some(RefusalCode::CapabilityNotExposed)
        );
        let mut unrequested = installation(workspace, Some(project), vec![grant(None, None)]);
        unrequested.requested.clear();
        assert_eq!(
            code(allowed_pre(&plugin(unrequested), &PLUGIN_READ)),
            Some(RefusalCode::CapabilityNotRequested)
        );
        assert_eq!(
            code(allowed_pre(
                &plugin(installation(workspace, None, vec![])),
                &PLUGIN_READ
            )),
            Some(RefusalCode::CapabilityNotGranted)
        );
        // The first half passes whatever the target is; the second half is where it is judged.
        let other_workspace = Resource {
            workspace_id: Uuid::from_u128(7),
            project_id: Some(project),
            form_id: None,
        };
        let other_project = Resource {
            workspace_id: workspace,
            project_id: Some(Uuid::from_u128(8)),
            form_id: None,
        };
        let inside = Resource {
            workspace_id: workspace,
            project_id: Some(project),
            form_id: None,
        };
        assert_eq!(
            code(allowed_target(&granted, &PLUGIN_READ, &other_workspace)),
            Some(RefusalCode::CapabilityScopeMismatch)
        );
        assert_eq!(
            code(allowed_target(&granted, &PLUGIN_READ, &other_project)),
            Some(RefusalCode::CapabilityNotGranted)
        );
        assert_eq!(code(allowed_target(&granted, &PLUGIN_READ, &inside)), None);
        for resource in [&other_workspace, &other_project, &inside] {
            assert_eq!(
                code(allowed(&granted, &PLUGIN_READ, resource)),
                code(allowed_target(&granted, &PLUGIN_READ, resource))
            );
        }
    }
}
