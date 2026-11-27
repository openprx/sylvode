//! `GET /api/v1/catalogue`: the capability ids with their operations.

use std::collections::BTreeMap;

use axum::{Json, extract::Query};
use serde::Deserialize;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use super::types::{Entry, Exposure};
use crate::error::ApiError;
use crate::response::ApiResponse;

#[derive(Debug, Deserialize)]
pub struct CatalogueQuery {
    /// Only operations with this exposure: `console`, `bot`, `oauth`, `plugin` or `site`.
    pub exposure: Option<String>,
}

/// The catalogue document; operations are sorted by key and capabilities by id.
///
/// `version` is the SHA-256 of the canonical JSON (RFC 8785) bytes of the capability list.
///
/// # Errors
/// The canonical serialisation failed.
pub fn document(filter: Option<Exposure>) -> Result<Value, serde_json::Error> {
    let mut by_id: BTreeMap<&'static str, Vec<&'static Entry>> = BTreeMap::new();
    for entry in super::entries() {
        if filter.is_none_or(|exposure| entry.exposure.contains(exposure)) {
            by_id.entry(entry.capability()).or_default().push(entry);
        }
    }
    let mut capabilities = Vec::with_capacity(by_id.len());
    for (id, mut operations) in by_id {
        operations.sort_by_key(|entry| (entry.op, entry.method.as_str()));
        let Some(first) = operations.first() else {
            continue;
        };
        let (area, resource, verb) = first.id_parts().unwrap_or(("", "", ""));
        let risk = operations.iter().map(|entry| entry.risk).max().unwrap_or(first.risk);
        capabilities.push(json!({
            "id": id,
            "area": area,
            "resource": resource,
            "verb": verb,
            "risk": risk.as_str(),
            "label_key": format!("cap.{id}"),
            "operations": operations.iter().map(|entry| json!({
                "op": entry.op,
                "method": entry.method.as_str(),
                "path": entry.path,
                "scope": entry.scope.name(),
                "comment": entry.comment,
                "exposure": entry.exposure.names(),
                "stability": entry.stability.as_str(),
            })).collect::<Vec<_>>(),
        }));
    }
    let digest = Sha256::digest(serde_jcs::to_vec(&capabilities)?);
    Ok(json!({
        "version": format!("sha256:{}", hex::encode(digest)),
        "capabilities": capabilities,
    }))
}

/// `GET /api/v1/catalogue`, behind the bot-or-user authentication.
///
/// # Errors
/// `400` for an unknown exposure name.
#[allow(clippy::unused_async)]
pub async fn capabilities(Query(query): Query<CatalogueQuery>) -> Result<Json<ApiResponse<Value>>, ApiError> {
    let filter = query
        .exposure
        .as_deref()
        .map(|name| Exposure::parse(name).ok_or_else(|| ApiError::BadRequest("unknown exposure".to_string())))
        .transpose()?;
    let data = document(filter).map_err(|error| {
        tracing::error!(%error, "the catalogue document could not be serialised");
        ApiError::Internal
    })?;
    Ok(ApiResponse::success(data))
}

#[cfg(test)]
mod tests {
    use super::document;
    use crate::catalogue::types::Exposure;

    /// Every id appears once with its operations, sorted; the version changes with the content.
    #[test]
    fn the_document_lists_sorted_capabilities() {
        let all = document(None).unwrap_or_default();
        let list = all["capabilities"].as_array().cloned().unwrap_or_default();
        assert!(list.len() > 200, "{} capabilities", list.len());
        let ids: Vec<&str> = list.iter().filter_map(|c| c["id"].as_str()).collect();
        let mut sorted = ids.clone();
        sorted.sort_unstable();
        sorted.dedup();
        assert_eq!(ids, sorted);
        assert!(ids.contains(&"catalogue.capabilities.read"));
        let plugin = document(Some(Exposure::PLUGIN)).unwrap_or_default();
        let plugin_ids: Vec<&str> = plugin["capabilities"]
            .as_array()
            .map(|list| list.iter().filter_map(|c| c["id"].as_str()).collect())
            .unwrap_or_default();
        assert_eq!(plugin_ids.len(), 12, "{plugin_ids:?}");
        assert_ne!(all["version"], plugin["version"]);
        assert!(
            all["version"]
                .as_str()
                .is_some_and(|v| v.starts_with("sha256:") && v.len() == 71)
        );
    }
}
