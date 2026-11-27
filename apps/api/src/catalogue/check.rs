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
/// # Errors
/// The [`Refusal`] of the first failed step.
pub fn allowed(principal: &Principal, entry: &Entry, resource: &Resource) -> Result<(), Refusal> {
    let capability = entry.capability();
    let refuse = |code| Refusal::new(code, Some(capability));
    if !entry.exposure.contains(channel(principal)) {
        return Err(refuse(RefusalCode::CapabilityNotExposed));
    }
    let installation = match principal {
        Principal::User { .. } | Principal::Bot { .. } | Principal::Delegated { .. } => return Ok(()),
        Principal::Plugin { installation, .. } | Principal::Installation { installation, .. } => installation,
    };
    check_installation(installation, capability, resource).map_err(refuse)
}

/// The order matters: a plugin must not learn from the answer whether a target exists. Once the id
/// is requested and granted somewhere, every mismatch of project or selector is `not_granted`;
/// only a target outside the installation's workspace is a scope mismatch. A selector key the
/// resource cannot supply never matches.
fn check_installation(
    installation: &InstallationView,
    capability: &str,
    resource: &Resource,
) -> Result<(), RefusalCode> {
    if !installation.requested.contains(capability) {
        return Err(RefusalCode::CapabilityNotRequested);
    }
    let mut grants = installation
        .grants
        .iter()
        .filter(|grant| grant.capability == capability)
        .peekable();
    if grants.peek().is_none() {
        return Err(RefusalCode::CapabilityNotGranted);
    }
    if resource.workspace_id != installation.workspace_id {
        return Err(RefusalCode::CapabilityScopeMismatch);
    }
    let in_project = installation
        .project_id
        .is_none_or(|project| resource.project_id == Some(project));
    let matched = in_project
        && grants.any(|grant| {
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

    use super::allowed;
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
}
