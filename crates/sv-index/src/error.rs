//! Validation findings: every one names the JSON path it belongs to.

use std::fmt;

/// One finding: a JSON path such as `$.plugins[3].versions[0].sha256` and what is wrong there.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Issue {
    /// JSON path, rooted at `$`.
    pub path: String,
    /// Human-readable description.
    pub message: String,
}

impl fmt::Display for Issue {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.path, self.message)
    }
}

/// A document that failed validation; holds every finding (at most [`crate::limits::MAX_ISSUES`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ValidationError {
    /// The findings, in discovery order. Never empty.
    pub issues: Vec<Issue>,
}

impl fmt::Display for ValidationError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for (i, issue) in self.issues.iter().enumerate() {
            if i > 0 {
                writeln!(f)?;
            }
            write!(f, "{issue}")?;
        }
        Ok(())
    }
}

impl std::error::Error for ValidationError {}

/// Collects findings up to a cap, so a hostile document cannot make a report unbounded.
#[derive(Debug, Default)]
pub struct Issues {
    list: Vec<Issue>,
    truncated: bool,
}

impl Issues {
    /// An empty collector.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Records a finding; findings beyond the cap are dropped.
    pub fn push(&mut self, path: &str, message: impl Into<String>) {
        if self.list.len() < crate::limits::MAX_ISSUES {
            self.list.push(Issue {
                path: path.to_owned(),
                message: message.into(),
            });
        } else {
            self.truncated = true;
        }
    }

    /// True when nothing has been recorded.
    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.list.is_empty()
    }

    /// The findings, with a final note when some were dropped.
    #[must_use]
    pub fn finish(mut self) -> Vec<Issue> {
        if self.truncated {
            self.list.push(Issue {
                path: "$".to_owned(),
                message: "further findings omitted".to_owned(),
            });
        }
        self.list
    }

    /// `Ok(())` when empty, else the error holding every finding.
    pub fn into_result(self) -> Result<(), ValidationError> {
        let issues = self.finish();
        if issues.is_empty() {
            Ok(())
        } else {
            Err(ValidationError { issues })
        }
    }
}

/// Child path of an object member.
#[must_use]
pub fn member(base: &str, name: &str) -> String {
    format!("{base}.{name}")
}

/// Child path of an array element.
#[must_use]
pub fn element(base: &str, index: usize) -> String {
    format!("{base}[{index}]")
}
