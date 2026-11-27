//! The typed outcome of a refused or failed invocation.

use serde_json::json;

use crate::catalogue::RefusalCode;
use crate::error::ApiError;

/// Why an invocation did not produce an output.
///
/// The text of a guest or of a handler never travels in it; only a [`RefusalCode`] and, for a guest
/// error, the validated code the guest chose.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HostError {
    pub code: RefusalCode,
    /// `details.guest_code` of `plugin_guest_error`, validated `[a-z0-9_]{1,64}`.
    pub guest_code: Option<String>,
}

impl HostError {
    #[must_use]
    pub const fn new(code: RefusalCode) -> Self {
        Self { code, guest_code: None }
    }

    #[must_use]
    pub const fn guest(code: String) -> Self {
        Self {
            code: RefusalCode::PluginGuestError,
            guest_code: Some(code),
        }
    }

    /// How the invocation log classes this error: `refused` before the guest ran or for a rule of
    /// the host, `limit` for a runtime ceiling, `guest_error` for the guest's own error code, `trap`
    /// for a failure of the guest.
    #[must_use]
    pub const fn outcome(&self) -> &'static str {
        match self.code {
            RefusalCode::PluginDeadlineExceeded
            | RefusalCode::PluginFuelExhausted
            | RefusalCode::PluginMemoryLimit
            | RefusalCode::PluginHostCallLimit
            | RefusalCode::PluginOutputTooLarge => "limit",
            RefusalCode::PluginGuestError => "guest_error",
            RefusalCode::PluginFailed => "trap",
            _ => "refused",
        }
    }
}

impl std::fmt::Display for HostError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(self.code.as_str())
    }
}

impl std::error::Error for HostError {}

impl From<HostError> for ApiError {
    fn from(error: HostError) -> Self {
        Self::Coded {
            status: error.code.status(),
            code: error.code.as_str(),
            message: error.code.message().to_string(),
            details: error.guest_code.map(|code| json!({ "guest_code": code })),
        }
    }
}

/// Whether `code` is a valid guest error code: `[a-z0-9_]{1,64}`.
#[must_use]
pub fn valid_guest_code(code: &str) -> bool {
    (1..=64).contains(&code.len())
        && code
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'_')
}

#[cfg(test)]
mod tests {
    use super::{HostError, valid_guest_code};
    use crate::catalogue::RefusalCode;

    #[test]
    fn guest_codes_are_validated() {
        assert!(valid_guest_code("not_ready_2"));
        assert!(!valid_guest_code(""));
        assert!(!valid_guest_code("Upper"));
        assert!(!valid_guest_code("white space"));
        assert!(!valid_guest_code(&"a".repeat(65)));
    }

    #[test]
    fn outcomes_follow_the_code() {
        assert_eq!(HostError::new(RefusalCode::PluginFuelExhausted).outcome(), "limit");
        assert_eq!(HostError::new(RefusalCode::PluginUnknownImport).outcome(), "refused");
        assert_eq!(HostError::new(RefusalCode::PluginFailed).outcome(), "trap");
        assert_eq!(HostError::guest("x".into()).outcome(), "guest_error");
    }
}
