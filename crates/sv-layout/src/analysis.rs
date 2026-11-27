//! Static capability analysis: which capabilities a layout implies.

use std::collections::{BTreeMap, BTreeSet};

use serde::Serialize;

use crate::model::{Action, DataSource, Layout};

/// Capability id for reading records (C1).
pub const CAP_RECORDS_READ: &str = "forms.records.read";
/// Capability id for creating records (C1).
pub const CAP_RECORDS_CREATE: &str = "forms.records.create";
/// Capability id for updating records (C1).
pub const CAP_RECORDS_UPDATE: &str = "forms.records.update";
/// Capability id for reading form descriptors (C1).
pub const CAP_FORMS_READ: &str = "forms.forms.read";
/// Capability id for invoking a package action export. C1 owns the final spelling; this is the
/// single place to change it.
pub const CAP_ACTION_INVOKE: &str = "extensions.actions.invoke";

/// One capability use.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CapabilityUse {
    /// Capability id.
    pub id: String,
    /// Target: a form reference, or the package id for invocations.
    pub target: String,
    /// JSON pointers of the data sources or actions that need it.
    pub paths: Vec<String>,
}

/// Result of [`analyse`].
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct Analysis {
    /// Capabilities, sorted by id then target.
    pub capabilities: Vec<CapabilityUse>,
    /// Package ids invoked, sorted.
    pub packages: Vec<String>,
    /// Form references used, sorted.
    pub forms: Vec<String>,
}

/// Collects the capabilities implied by data sources and actions.
///
/// Every declared source and action counts, whether or not a node uses it (conservative: the
/// review screen lists what the layout could ask for). Reads happen with the viewer's principal,
/// so these are checked against the viewer at run time, not granted.
#[must_use]
pub fn analyse(layout: &Layout) -> Analysis {
    let mut caps: BTreeMap<(String, String), Vec<String>> = BTreeMap::new();
    let mut packages = BTreeSet::new();
    let mut forms = BTreeSet::new();
    let mut add = |id: &str, target: &str, path: String| {
        caps.entry((id.to_owned(), target.to_owned())).or_default().push(path);
    };
    for (name, src) in &layout.data {
        let path = format!("/data/{}", esc(name));
        match src {
            DataSource::Record(s) => {
                forms.insert(s.form.clone());
                add(CAP_RECORDS_READ, &s.form, path);
            }
            DataSource::Query(s) => {
                forms.insert(s.form.clone());
                add(CAP_RECORDS_READ, &s.form, path);
            }
            DataSource::Form(s) => {
                forms.insert(s.form.clone());
                add(CAP_FORMS_READ, &s.form, path);
            }
            DataSource::Plugin(s) => {
                packages.insert(s.package.clone());
            }
        }
    }
    for (name, action) in &layout.actions {
        let path = format!("/actions/{}", esc(name));
        match action {
            Action::Invoke { package, .. } => {
                packages.insert(package.clone());
                add(CAP_ACTION_INVOKE, package, path);
            }
            Action::Submit { data, mode, .. } => {
                let form = match layout.data.get(data) {
                    Some(DataSource::Form(f)) => f.form.clone(),
                    _ => continue,
                };
                let id = match mode {
                    crate::model::SubmitMode::Create => CAP_RECORDS_CREATE,
                    crate::model::SubmitMode::Update => CAP_RECORDS_UPDATE,
                };
                forms.insert(form.clone());
                add(id, &form, path);
            }
            Action::Navigate { .. } => {}
        }
    }
    Analysis {
        capabilities: caps
            .into_iter()
            .map(|((id, target), mut paths)| {
                paths.sort();
                CapabilityUse { id, target, paths }
            })
            .collect(),
        packages: packages.into_iter().collect(),
        forms: forms.into_iter().collect(),
    }
}

fn esc(seg: &str) -> String {
    seg.replace('~', "~0").replace('/', "~1")
}
