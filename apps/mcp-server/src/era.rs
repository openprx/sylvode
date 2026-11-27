//! Protocol eras: the legacy MCP protocol and revision 2026-07-28 are served side by side.
//!
//! A request is modern exactly when `params._meta["io.modelcontextprotocol/protocolVersion"]` is
//! present. The HTTP header `MCP-Protocol-Version` alone never selects the modern era, because
//! clients of the 2025-06-18 revision and later send it with a legacy value.

use crate::protocol::{JsonRpcError, JsonRpcRequest};
use serde_json::{Map, Value, json};

pub const MODERN_VERSION: &str = "2026-07-28";
pub const MODERN_VERSIONS: [&str; 1] = [MODERN_VERSION];
pub const LEGACY_VERSIONS: [&str; 4] = ["2025-11-25", "2025-06-18", "2025-03-26", "2024-11-05"];
pub const LEGACY_DEFAULT: &str = "2025-11-25";
/// The versions an `-32022` answer names (the interop specification's example).
pub const UNSUPPORTED_SUPPORTED_LIST: [&str; 2] = ["2026-07-28", "2025-11-25"];

pub const META_PROTOCOL_VERSION: &str = "io.modelcontextprotocol/protocolVersion";
pub const META_CLIENT_CAPABILITIES: &str = "io.modelcontextprotocol/clientCapabilities";
pub const META_CLIENT_INFO: &str = "io.modelcontextprotocol/clientInfo";
pub const META_SERVER_INFO: &str = "io.modelcontextprotocol/serverInfo";
pub const META_SUBSCRIPTION_ID: &str = "io.modelcontextprotocol/subscriptionId";
pub const TASK_PROTOCOL_EXTENSION: &str = "io.sylvode/task-protocol";

pub const TTL_DISCOVER_MS: u64 = 3_600_000;
pub const TTL_TOOLS_LISTEN_MS: u64 = 300_000;
pub const TTL_TOOLS_NO_LISTEN_MS: u64 = 60_000;
pub const TTL_CATALOGUE_MS: u64 = 3_600_000;
pub const TTL_BOARD_MS: u64 = 0;

/// The era one request belongs to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Era {
    Legacy,
    Modern {
        version: &'static str,
        client_info: Option<Value>,
        client_capabilities: Value,
    },
}

impl Era {
    pub const fn is_modern(&self) -> bool {
        matches!(self, Self::Modern { .. })
    }

    /// The label the request log carries.
    pub const fn label(&self) -> &'static str {
        match self {
            Self::Legacy => "legacy",
            Self::Modern { .. } => "modern",
        }
    }
}

/// Why a request could not be placed in an era.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EraError {
    /// `_meta` names a protocol version this server does not serve in the modern era.
    Unsupported { requested: String },
    /// `_meta` is present but its protocol version is not a string.
    MalformedMeta(&'static str),
}

impl EraError {
    pub fn into_rpc_error(self) -> JsonRpcError {
        match self {
            Self::Unsupported { requested } => JsonRpcError::unsupported_protocol_version(&requested),
            Self::MalformedMeta(detail) => JsonRpcError::invalid_params(detail),
        }
    }
}

/// Decides the era of one request (D6).
///
/// `_meta` absent: legacy. A protocol version that is not a string: malformed. A string outside
/// [`MODERN_VERSIONS`], legacy version strings included: unsupported. Otherwise modern; a missing
/// `clientCapabilities` counts as `{}` (D7).
pub fn classify(req: &JsonRpcRequest) -> Result<Era, EraError> {
    let Some(meta) = req
        .params
        .as_ref()
        .and_then(|params| params.get("_meta"))
        .and_then(Value::as_object)
    else {
        return Ok(Era::Legacy);
    };
    let Some(version) = meta.get(META_PROTOCOL_VERSION) else {
        return Ok(Era::Legacy);
    };
    let Some(version) = version.as_str() else {
        return Err(EraError::MalformedMeta("_meta protocolVersion must be a string"));
    };
    let Some(version) = MODERN_VERSIONS.iter().copied().find(|known| *known == version) else {
        return Err(EraError::Unsupported {
            requested: version.to_string(),
        });
    };
    let client_capabilities = meta.get(META_CLIENT_CAPABILITIES).cloned().unwrap_or_else(|| {
        tracing::debug!("modern request without clientCapabilities; treated as an empty object");
        json!({})
    });
    Ok(Era::Modern {
        version,
        client_info: meta.get(META_CLIENT_INFO).cloned(),
        client_capabilities,
    })
}

/// The legacy protocol version to answer `initialize` with: the requested one when known, else
/// [`LEGACY_DEFAULT`].
pub fn negotiate_legacy(requested: Option<&str>) -> &'static str {
    requested
        .and_then(|requested| LEGACY_VERSIONS.iter().copied().find(|known| *known == requested))
        .unwrap_or(LEGACY_DEFAULT)
}

/// How long a client may reuse a modern result.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CacheScope {
    Public,
    Private,
}

impl CacheScope {
    const fn as_str(self) -> &'static str {
        match self {
            Self::Public => "public",
            Self::Private => "private",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CachePolicy {
    None,
    Ttl { ms: u64, scope: CacheScope },
}

/// The `serverInfo` object of `initialize` and of the modern `_meta`.
pub fn server_info() -> Value {
    json!({ "name": "sylvode-mcp-server", "version": env!("CARGO_PKG_VERSION") })
}

/// Decorates a modern result: `resultType` `complete`, `_meta` `serverInfo` (merged into an
/// existing `_meta`) and, when the policy says so, `ttlMs` and `cacheScope`.
pub fn decorate_modern(result: &mut Value, cache: CachePolicy) {
    let Some(fields) = result.as_object_mut() else {
        return;
    };
    fields.insert("resultType".to_string(), json!("complete"));
    if let CachePolicy::Ttl { ms, scope } = cache {
        fields.insert("ttlMs".to_string(), json!(ms));
        fields.insert("cacheScope".to_string(), json!(scope.as_str()));
    }
    let meta = fields
        .entry("_meta".to_string())
        .or_insert_with(|| Value::Object(Map::new()));
    if !meta.is_object() {
        *meta = Value::Object(Map::new());
    }
    if let Some(meta) = meta.as_object_mut() {
        meta.insert(META_SERVER_INFO.to_string(), server_info());
    }
}

#[cfg(test)]
#[allow(clippy::indexing_slicing)]
mod tests {
    use super::*;

    fn request(params: Option<Value>) -> JsonRpcRequest {
        JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            id: Some(json!(1)),
            method: "tools/list".to_string(),
            params,
        }
    }

    fn with_meta(version: &Value) -> JsonRpcRequest {
        request(Some(json!({ "_meta": { META_PROTOCOL_VERSION: version } })))
    }

    #[test]
    fn classify_without_meta_is_legacy() {
        assert_eq!(classify(&request(None)), Ok(Era::Legacy));
        assert_eq!(classify(&request(Some(json!({})))), Ok(Era::Legacy));
        assert_eq!(classify(&request(Some(json!({"_meta": {}})))), Ok(Era::Legacy));
        assert_eq!(
            classify(&request(Some(json!({"_meta": {"progressToken": 1}})))),
            Ok(Era::Legacy)
        );
    }

    #[test]
    fn classify_modern_version_is_modern_with_defaults() {
        let era = classify(&with_meta(&json!("2026-07-28")));
        assert_eq!(
            era,
            Ok(Era::Modern {
                version: MODERN_VERSION,
                client_info: None,
                client_capabilities: json!({}),
            })
        );
    }

    #[test]
    fn classify_keeps_client_info_and_capabilities() {
        let params = Some(json!({"_meta": {
            META_PROTOCOL_VERSION: "2026-07-28",
            META_CLIENT_INFO: {"name": "prx"},
            META_CLIENT_CAPABILITIES: {"roots": {}},
        }}));
        assert_eq!(
            classify(&request(params)),
            Ok(Era::Modern {
                version: MODERN_VERSION,
                client_info: Some(json!({"name": "prx"})),
                client_capabilities: json!({"roots": {}}),
            })
        );
    }

    #[test]
    fn classify_unknown_version_is_unsupported() {
        assert_eq!(
            classify(&with_meta(&json!("1900-01-01"))),
            Err(EraError::Unsupported {
                requested: "1900-01-01".to_string()
            })
        );
    }

    #[test]
    fn classify_legacy_version_inside_meta_is_unsupported() {
        assert_eq!(
            classify(&with_meta(&json!("2025-11-25"))),
            Err(EraError::Unsupported {
                requested: "2025-11-25".to_string()
            })
        );
    }

    #[test]
    fn classify_non_string_version_is_malformed() {
        for value in [json!(20_260_728), json!(null), json!(["2026-07-28"]), json!({})] {
            assert!(matches!(classify(&with_meta(&value)), Err(EraError::MalformedMeta(_))));
        }
    }

    #[test]
    fn era_errors_map_to_json_rpc_codes() {
        let unsupported = EraError::Unsupported {
            requested: "1900-01-01".to_string(),
        }
        .into_rpc_error();
        assert_eq!(unsupported.code, -32022);
        assert_eq!(
            unsupported.data,
            Some(json!({"supported": ["2026-07-28", "2025-11-25"], "requested": "1900-01-01"}))
        );
        assert_eq!(EraError::MalformedMeta("x").into_rpc_error().code, -32602);
    }

    #[test]
    fn negotiate_legacy_echoes_known_versions() {
        for version in LEGACY_VERSIONS {
            assert_eq!(negotiate_legacy(Some(version)), version);
        }
    }

    #[test]
    fn negotiate_legacy_falls_back_to_default() {
        assert_eq!(negotiate_legacy(Some("1900-01-01")), LEGACY_DEFAULT);
        assert_eq!(negotiate_legacy(Some("2026-07-28")), LEGACY_DEFAULT);
        assert_eq!(negotiate_legacy(None), LEGACY_DEFAULT);
    }

    #[test]
    fn decorate_modern_adds_result_type_cache_and_server_info() {
        let mut result = json!({"tools": []});
        decorate_modern(
            &mut result,
            CachePolicy::Ttl {
                ms: 60_000,
                scope: CacheScope::Private,
            },
        );
        assert_eq!(result["resultType"], "complete");
        assert_eq!(result["ttlMs"], 60_000);
        assert_eq!(result["cacheScope"], "private");
        assert_eq!(result["_meta"][META_SERVER_INFO]["name"], "sylvode-mcp-server");
    }

    #[test]
    fn decorate_modern_without_cache_omits_ttl() {
        let mut result = json!({"content": []});
        decorate_modern(&mut result, CachePolicy::None);
        assert!(result.get("ttlMs").is_none());
        assert!(result.get("cacheScope").is_none());
    }

    #[test]
    fn decorate_modern_keeps_existing_meta_keys() {
        let mut result = json!({"_meta": {"other": 1}});
        decorate_modern(&mut result, CachePolicy::None);
        assert_eq!(result["_meta"]["other"], 1);
        assert!(result["_meta"].get(META_SERVER_INFO).is_some());
    }
}
