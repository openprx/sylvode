//! Findings and the two report formats (human text, JSON).

use std::collections::BTreeMap;

use serde::Serialize;
use serde_json::{Value, json};

/// Findings printed before the report says "more omitted".
const MAX_FINDINGS: usize = 1000;

/// One violation: where it is (file and JSON path), a machine code, and what is wrong.
#[derive(Debug, Clone, Serialize)]
pub struct Finding {
    /// Repository-relative file (`plugins/acme/pricing/plugin.json`), or `index.json`.
    pub file: String,
    /// JSON path inside the file (`$.versions[0].sha256`), or `$` when the finding is about the file.
    pub path: String,
    /// Stable machine code.
    pub code: &'static str,
    /// Human-readable description.
    pub message: String,
}

impl Finding {
    pub fn new(file: &str, path: &str, code: &'static str, message: impl Into<String>) -> Self {
        Self {
            file: file.to_owned(),
            path: path.to_owned(),
            code,
            message: message.into(),
        }
    }
}

/// Result of one package download and verification.
#[derive(Debug, Clone, Serialize)]
pub struct Download {
    pub plugin: String,
    pub version: String,
    pub url: String,
    /// `ok`, `failed` or `skipped`.
    pub outcome: &'static str,
    /// Why it failed or was skipped.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

/// Everything `check` found.
#[derive(Debug, Default)]
pub struct Report {
    pub findings: Vec<Finding>,
    pub publishers: usize,
    pub plugins: usize,
    pub versions: usize,
    pub downloads: Vec<Download>,
    /// What the head changes relative to the base, when a base was given.
    pub changes: Option<Value>,
    /// Steps that did not run, with the reason.
    pub notes: Vec<String>,
}

impl Report {
    pub const fn ok(&self) -> bool {
        self.findings.is_empty()
    }

    pub fn to_json(&self) -> Value {
        let count = |o: &str| self.downloads.iter().filter(|d| d.outcome == o).count();
        json!({
            "ok": self.ok(),
            "findings": self.findings,
            "summary": {
                "publishers": self.publishers,
                "plugins": self.plugins,
                "versions": self.versions,
                "downloads_ok": count("ok"),
                "downloads_failed": count("failed"),
                "downloads_skipped": count("skipped"),
            },
            "downloads": self.downloads,
            "changes": self.changes,
            "notes": self.notes,
        })
    }

    pub fn render(&self, command: &str) -> String {
        let mut lines: Vec<String> = Vec::new();
        let mut by_file: BTreeMap<&str, Vec<&Finding>> = BTreeMap::new();
        for f in self.findings.iter().take(MAX_FINDINGS) {
            by_file.entry(f.file.as_str()).or_default().push(f);
        }
        for (file, list) in by_file {
            lines.push(file.to_owned());
            lines.extend(
                list.iter()
                    .map(|f| format!("  {}  [{}]  {}", f.path, f.code, f.message)),
            );
        }
        if self.findings.len() > MAX_FINDINGS {
            lines.push(format!(
                "... {} more findings omitted",
                self.findings.len() - MAX_FINDINGS
            ));
        }
        lines.extend(self.notes.iter().map(|n| format!("note: {n}")));
        let count = |o: &str| self.downloads.iter().filter(|d| d.outcome == o).count();
        lines.push(format!(
            "checked {} publishers, {} plugins, {} versions; downloads: {} ok, {} failed, {} skipped",
            self.publishers,
            self.plugins,
            self.versions,
            count("ok"),
            count("failed"),
            count("skipped"),
        ));
        if self.ok() {
            lines.push(format!("sv-index {command}: OK"));
        } else {
            lines.push(format!("sv-index {command}: FAILED ({} findings)", self.findings.len()));
        }
        lines.push(String::new());
        lines.join("\n")
    }
}
