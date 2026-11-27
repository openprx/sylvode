use std::fmt;

/// A rule or expression refused at schema save.
///
/// `path` is a JSON Pointer relative to the rule object passed to [`crate::parse_rule`] (empty for
/// the object itself, and always empty from [`crate::compile`]); the caller prefixes the field path.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RuleDefError {
    pub path: String,
    pub code: &'static str,
}

impl RuleDefError {
    pub(crate) fn new(path: impl Into<String>, code: &'static str) -> Self {
        Self {
            path: path.into(),
            code,
        }
    }

    pub(crate) fn root(code: &'static str) -> Self {
        Self::new(String::new(), code)
    }
}

impl fmt::Display for RuleDefError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.path.is_empty() {
            f.write_str(self.code)
        } else {
            write!(f, "{} at {}", self.code, self.path)
        }
    }
}

impl std::error::Error for RuleDefError {}

/// An evaluation failure: `cel_error` (type mismatch, missing key, division by zero, non-boolean
/// result) or `cel_limit` (a bound value over a runtime limit). Either way the rule fails closed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RuleEvalError {
    pub code: &'static str,
}

impl RuleEvalError {
    pub(crate) const fn error() -> Self {
        Self { code: "cel_error" }
    }

    pub(crate) const fn limit() -> Self {
        Self { code: "cel_limit" }
    }
}

impl fmt::Display for RuleEvalError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.code)
    }
}

impl std::error::Error for RuleEvalError {}
