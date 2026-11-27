//! Refusal codes of the catalogue and of the legacy plugin containment (contract C1 §7).

use serde_json::json;

use crate::error::ApiError;

/// A stable refusal code. No value of the request is ever echoed with it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RefusalCode {
    /// The route has no catalogue entry (the guard layer).
    CapabilityUnclassified,
    /// The principal's channel is not in the route's exposure.
    CapabilityNotExposed,
    /// The target is outside the installation's scope, or cannot be resolved.
    CapabilityScopeMismatch,
    /// The id is not in the generation's requested capabilities.
    CapabilityNotRequested,
    /// No grant covers the id and the target.
    CapabilityNotGranted,
    /// Plugin host: a plugin may not call the extension routes.
    PluginReentry,
    /// Plugin host: the component was built for another plugin interface version.
    PluginWitVersion,
    /// Plugin host: the component does not export the requested world.
    PluginWorldMismatch,
    /// Plugin host: the component imports something this host does not provide.
    PluginUnknownImport,
    /// Plugin host: the component is not a valid plugin component.
    PluginInvalidComponent,
    /// Plugin host: the component is larger than the limit.
    PluginComponentTooLarge,
    /// Plugin host: the input is larger than the limit.
    PluginInputTooLarge,
    /// Plugin host: the output is larger than the limit.
    PluginOutputTooLarge,
    /// Plugin host: the plugin ran past its time limit.
    PluginDeadlineExceeded,
    /// Plugin host: the plugin used up its compute budget.
    PluginFuelExhausted,
    /// Plugin host: the plugin asked for more memory than the limit.
    PluginMemoryLimit,
    /// Plugin host: the plugin made more host calls than the limit.
    PluginHostCallLimit,
    /// Plugin host: too many plugin runs at the same time.
    PluginConcurrencyLimit,
    /// Plugin host: the plugin reported an error.
    PluginGuestError,
    /// Plugin host: the plugin failed.
    PluginFailed,
    /// Plugin host: the action is not one the plugin declares.
    PluginActionUnknown,
    /// The old plugin system accepts installs and updates from workspace owners and admins only.
    LegacyPluginAdminOnly,
    /// A `formula` patch names a key the caller may not write.
    LegacyPluginPatchDenied,
    /// A validator hook refused the value.
    LegacyPluginValidationFailed,
    /// Too many concurrent runs of one old-system plugin.
    LegacyPluginBusy,
}

impl RefusalCode {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::CapabilityUnclassified => "capability_unclassified",
            Self::CapabilityNotExposed => "capability_not_exposed",
            Self::CapabilityScopeMismatch => "capability_scope_mismatch",
            Self::CapabilityNotRequested => "capability_not_requested",
            Self::CapabilityNotGranted => "capability_not_granted",
            Self::PluginReentry => "plugin_reentry",
            Self::PluginWitVersion => "plugin_wit_version",
            Self::PluginWorldMismatch => "plugin_world_mismatch",
            Self::PluginUnknownImport => "plugin_unknown_import",
            Self::PluginInvalidComponent => "plugin_invalid_component",
            Self::PluginComponentTooLarge => "plugin_component_too_large",
            Self::PluginInputTooLarge => "plugin_input_too_large",
            Self::PluginOutputTooLarge => "plugin_output_too_large",
            Self::PluginDeadlineExceeded => "plugin_deadline_exceeded",
            Self::PluginFuelExhausted => "plugin_fuel_exhausted",
            Self::PluginMemoryLimit => "plugin_memory_limit",
            Self::PluginHostCallLimit => "plugin_host_call_limit",
            Self::PluginConcurrencyLimit => "plugin_concurrency_limit",
            Self::PluginGuestError => "plugin_guest_error",
            Self::PluginFailed => "plugin_failed",
            Self::PluginActionUnknown => "plugin_action_unknown",
            Self::LegacyPluginAdminOnly => "legacy_plugin_admin_only",
            Self::LegacyPluginPatchDenied => "legacy_plugin_patch_denied",
            Self::LegacyPluginValidationFailed => "legacy_plugin_validation_failed",
            Self::LegacyPluginBusy => "legacy_plugin_busy",
        }
    }

    /// The status of the envelope.
    #[must_use]
    pub const fn status(self) -> i32 {
        match self {
            Self::LegacyPluginValidationFailed
            | Self::PluginWitVersion
            | Self::PluginWorldMismatch
            | Self::PluginUnknownImport
            | Self::PluginInvalidComponent
            | Self::PluginDeadlineExceeded
            | Self::PluginFuelExhausted
            | Self::PluginMemoryLimit
            | Self::PluginHostCallLimit
            | Self::PluginGuestError
            | Self::PluginFailed => 422,
            Self::PluginComponentTooLarge | Self::PluginInputTooLarge | Self::PluginOutputTooLarge => 413,
            Self::LegacyPluginBusy | Self::PluginConcurrencyLimit => 429,
            Self::PluginActionUnknown => 404,
            _ => 403,
        }
    }

    /// The fixed English text of the refusal.
    #[must_use]
    pub const fn message(self) -> &'static str {
        match self {
            Self::CapabilityUnclassified => "this route is not in the operation catalogue",
            Self::CapabilityNotExposed => "this operation is not available to this caller",
            Self::CapabilityScopeMismatch => "the target is outside the scope of this caller",
            Self::CapabilityNotRequested => "this operation was not requested",
            Self::CapabilityNotGranted => "this operation was not granted",
            Self::PluginReentry => "a plugin may not call the extension routes",
            Self::PluginWitVersion => "the component was built for another plugin interface version",
            Self::PluginWorldMismatch => "the component does not export the requested world",
            Self::PluginUnknownImport => "the component imports something this host does not provide",
            Self::PluginInvalidComponent => "the component is not a valid plugin component",
            Self::PluginComponentTooLarge => "the component is larger than the limit",
            Self::PluginInputTooLarge => "the input is larger than the limit",
            Self::PluginOutputTooLarge => "the output is larger than the limit",
            Self::PluginDeadlineExceeded => "the plugin ran past its time limit",
            Self::PluginFuelExhausted => "the plugin used up its compute budget",
            Self::PluginMemoryLimit => "the plugin asked for more memory than the limit",
            Self::PluginHostCallLimit => "the plugin made more host calls than the limit",
            Self::PluginConcurrencyLimit => "too many plugin runs at the same time",
            Self::PluginGuestError => "the plugin reported an error",
            Self::PluginFailed => "the plugin failed",
            Self::PluginActionUnknown => "the plugin has no such action",
            Self::LegacyPluginAdminOnly => "only a workspace owner or admin may install or update a plugin",
            Self::LegacyPluginPatchDenied => "the plugin may not change this field",
            Self::LegacyPluginValidationFailed => "the plugin refused this value",
            Self::LegacyPluginBusy => "too many concurrent runs of this plugin",
        }
    }
}

/// A refusal: a code and, when it names one, the capability id.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Refusal {
    pub code: RefusalCode,
    pub capability: Option<&'static str>,
}

impl Refusal {
    #[must_use]
    pub const fn new(code: RefusalCode, capability: Option<&'static str>) -> Self {
        Self { code, capability }
    }
}

impl From<Refusal> for ApiError {
    fn from(refusal: Refusal) -> Self {
        Self::Coded {
            status: refusal.code.status(),
            code: refusal.code.as_str(),
            message: refusal.code.message().to_string(),
            details: refusal.capability.map(|capability| json!({ "capability": capability })),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{Refusal, RefusalCode};
    use crate::error::ApiError;

    /// A refusal becomes a coded error carrying only the fixed text and the capability id.
    #[test]
    fn a_refusal_names_the_capability_and_nothing_else() {
        let error = ApiError::from(Refusal::new(
            RefusalCode::CapabilityNotGranted,
            Some("forms.records.read"),
        ));
        match error {
            ApiError::Coded {
                status,
                code,
                message,
                details,
            } => {
                assert_eq!(status, 403);
                assert_eq!(code, "capability_not_granted");
                assert_eq!(message, RefusalCode::CapabilityNotGranted.message());
                assert_eq!(details, Some(serde_json::json!({ "capability": "forms.records.read" })));
            }
            other => assert!(matches!(other, ApiError::Coded { .. }), "unexpected error {other:?}"),
        }
    }

    #[test]
    fn the_status_of_each_code() {
        assert_eq!(RefusalCode::LegacyPluginValidationFailed.status(), 422);
        assert_eq!(RefusalCode::LegacyPluginAdminOnly.status(), 403);
        assert_eq!(RefusalCode::CapabilityUnclassified.status(), 403);
    }
}
