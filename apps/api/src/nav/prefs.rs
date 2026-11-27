//! `sv.nav.prefs.v1`: what one person changed on top of a navigation document.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::doc::{NavError, valid_id};

pub const SCHEMA: &str = "sv.nav.prefs.v1";
pub const MAX_PREFS_BYTES: usize = 16 * 1024;
const MAX_IDS: usize = 200;

/// Ids only; an id the document no longer has is ignored when the navigation is resolved.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Prefs {
    pub schema: String,
    #[serde(default)]
    pub hidden: Vec<String>,
    #[serde(default)]
    pub order: BTreeMap<String, Vec<String>>,
    #[serde(default)]
    pub pinned: Vec<String>,
    #[serde(default)]
    pub unpinned: Vec<String>,
    #[serde(default)]
    pub collapsed: Vec<String>,
    /// Sections the administrator collapsed by default that this person keeps open.
    #[serde(default)]
    pub expanded: Vec<String>,
}

impl Prefs {
    /// What a person without a stored row has.
    #[must_use]
    pub fn empty() -> Self {
        Self {
            schema: SCHEMA.to_string(),
            ..Self::default()
        }
    }
}

fn check_ids(ids: &[String], path: &str) -> Result<(), NavError> {
    if ids.len() > MAX_IDS {
        return Err(NavError::invalid(path, "too_large"));
    }
    if ids.iter().any(|id| !valid_id(id)) {
        return Err(NavError::invalid(path, "schema"));
    }
    Ok(())
}

/// Parses and checks a preferences document.
///
/// # Errors
/// The first [`NavError`] found.
pub fn parse(raw: &Value) -> Result<Prefs, NavError> {
    let size = serde_json::to_vec(raw).map_or(usize::MAX, |bytes| bytes.len());
    if size > MAX_PREFS_BYTES {
        return Err(NavError::invalid("", "too_large"));
    }
    let prefs: Prefs = serde_json::from_value(raw.clone()).map_err(|_| NavError::invalid("", "schema"))?;
    if prefs.schema != SCHEMA {
        return Err(NavError::invalid("/schema", "schema"));
    }
    check_ids(&prefs.hidden, "/hidden")?;
    check_ids(&prefs.pinned, "/pinned")?;
    check_ids(&prefs.unpinned, "/unpinned")?;
    check_ids(&prefs.collapsed, "/collapsed")?;
    check_ids(&prefs.expanded, "/expanded")?;
    if prefs.order.len() > MAX_IDS {
        return Err(NavError::invalid("/order", "too_large"));
    }
    for (section, ids) in &prefs.order {
        if !valid_id(section) {
            return Err(NavError::invalid("/order", "schema"));
        }
        check_ids(ids, &format!("/order/{section}"))?;
    }
    Ok(prefs)
}

#[cfg(test)]
#[allow(clippy::expect_used)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn a_valid_document_parses_and_bad_ones_are_refused() {
        let ok = json!({"schema": "sv.nav.prefs.v1", "hidden": ["handbook"], "order": {"main": ["intake", "forms"]}, "pinned": ["desk"]});
        assert_eq!(parse(&ok).expect("parses").hidden, ["handbook"]);
        assert!(parse(&json!({"schema": "sv.nav.prefs.v2"})).is_err());
        assert!(parse(&json!({"schema": SCHEMA, "hidden": ["Bad Id"]})).is_err());
        assert!(parse(&json!({"schema": SCHEMA, "extra": 1})).is_err());
        let many: Vec<String> = (0..201).map(|i| format!("e{i}")).collect();
        assert_eq!(
            parse(&json!({"schema": SCHEMA, "hidden": many}))
                .expect_err("refused")
                .reason,
            "too_large"
        );
    }
}
