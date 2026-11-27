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
    /// The old plugin system accepts installs and updates from workspace owners and admins only.
    LegacyPluginAdminOnly,
    /// A `formula` patch names a key the caller may not write.
    LegacyPluginPatchDenied,
    /// A validator hook refused the value.
    LegacyPluginValidationFailed,
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
            Self::LegacyPluginAdminOnly => "legacy_plugin_admin_only",
            Self::LegacyPluginPatchDenied => "legacy_plugin_patch_denied",
            Self::LegacyPluginValidationFailed => "legacy_plugin_validation_failed",
        }
    }

    /// The status of the envelope.
    #[must_use]
    pub const fn status(self) -> i32 {
        match self {
            Self::LegacyPluginValidationFailed => 422,
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
            Self::LegacyPluginAdminOnly => "only a workspace owner or admin may install or update a plugin",
            Self::LegacyPluginPatchDenied => "the plugin may not change this field",
            Self::LegacyPluginValidationFailed => "the plugin refused this value",
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
