//! `[extensions]` — settings of the plugin host.

use serde::Deserialize;

/// The validated `[extensions]` section.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ExtensionsConfig {
    /// Registers the temporary instance-admin route that runs a plugin component as any member
    /// (`POST /api/v1/admin/extensions/dev-invoke`). Off by default; only test stacks enable it.
    pub dev_invoke_enabled: bool,
}

#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub(super) struct RawExtensions {
    #[serde(default)]
    dev_invoke_enabled: bool,
}

impl RawExtensions {
    pub(super) const fn validate(self) -> ExtensionsConfig {
        ExtensionsConfig {
            dev_invoke_enabled: self.dev_invoke_enabled,
        }
    }
}
