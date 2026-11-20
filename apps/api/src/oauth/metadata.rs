//! The discovery documents: RFC 8414 authorization server metadata and RFC 9728 protected
//! resource metadata for the API.
//!
//! The MCP server's own document is served by the MCP server (it knows its own listener); the
//! authorization server only names the issuer there.
//!
//! Both documents exist only when `[oauth] issuer` is configured. Without it the routes answer
//! 404 and nothing advertises an authorization server that is not there.

use std::sync::Arc;

use axum::{
    Json, Router,
    http::{HeaderValue, StatusCode, Uri, header},
    response::{IntoResponse, Response},
    routing::get,
};
use platform::config::OAuthConfig;
use serde_json::{Value, json};

use super::error::OAuthError;
use super::scopes::{ScopeSet, TokenResource};

/// Where the authorization server metadata is served (the issuer has no path).
pub const AUTHORIZATION_SERVER_WELL_KNOWN: &str = "/.well-known/oauth-authorization-server";
/// RFC 9728 well-known prefix for protected resource metadata.
pub const PROTECTED_RESOURCE_WELL_KNOWN: &str = "/.well-known/oauth-protected-resource";
/// Documents may be cached for an hour.
pub const CACHE_CONTROL_VALUE: &str = "public, max-age=3600";

fn scope_names(set: ScopeSet) -> Vec<&'static str> {
    set.iter().map(super::scopes::Scope::as_str).collect()
}

/// RFC 8414 document for the configured issuer.
///
/// No `registration_endpoint` (no dynamic registration), no `introspection_endpoint`, and
/// `client_id_metadata_document_supported` is `false`: Ramflux is the one pre-registered client.
pub fn authorization_server_metadata(config: &OAuthConfig) -> Value {
    let issuer = config.issuer.as_str();
    json!({
        "issuer": issuer,
        "authorization_endpoint": format!("{issuer}/oauth/authorize"),
        "token_endpoint": format!("{issuer}/api/v1/oauth/token"),
        "revocation_endpoint": format!("{issuer}/api/v1/oauth/revoke"),
        "response_types_supported": ["code"],
        "response_modes_supported": ["query"],
        "grant_types_supported": ["authorization_code", "refresh_token"],
        "code_challenge_methods_supported": ["S256"],
        "token_endpoint_auth_methods_supported": ["none"],
        "revocation_endpoint_auth_methods_supported": ["none"],
        "scopes_supported": scope_names(ScopeSet::all()),
        "authorization_response_iss_parameter_supported": true,
        "client_id_metadata_document_supported": false,
    })
}

/// RFC 9728 document for the API resource.
pub fn api_protected_resource_metadata(config: &OAuthConfig) -> Value {
    json!({
        "resource": config.api_resource,
        "authorization_servers": [config.issuer],
        "bearer_methods_supported": ["header"],
        "scopes_supported": scope_names(TokenResource::Api.scopes_supported()),
        "resource_name": "Sylvode API",
    })
}

/// The path of a resource's protected resource metadata (RFC 9728 section 3.1): the well-known
/// prefix inserted between the host and the resource's path. `None` for a value that is not an
/// absolute URL.
pub fn protected_resource_well_known_path(resource: &str) -> Option<String> {
    let (_, rest) = resource.split_once("://")?;
    let rest = rest.split(['?', '#']).next().unwrap_or(rest);
    let path = rest.find('/').and_then(|at| rest.get(at..)).unwrap_or("");
    Some(if path.is_empty() || path == "/" {
        PROTECTED_RESOURCE_WELL_KNOWN.to_owned()
    } else {
        format!("{PROTECTED_RESOURCE_WELL_KNOWN}{path}")
    })
}

/// The URL named in `resource_metadata` of the API's bearer challenges.
pub fn api_resource_metadata_url(config: &OAuthConfig) -> Option<String> {
    let path = protected_resource_well_known_path(&config.api_resource)?;
    Some(format!("{}{path}", origin_of(&config.api_resource)?))
}

/// `scheme://authority` of an absolute URL.
fn origin_of(url: &str) -> Option<&str> {
    let (scheme, rest) = url.split_once("://")?;
    let end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
    url.get(..scheme.len() + 3 + end)
}

fn document(value: Value) -> Response {
    let mut response = Json(value).into_response();
    response
        .headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static(CACHE_CONTROL_VALUE));
    response
}

fn authorization_server_response(config: Option<&OAuthConfig>) -> Response {
    config.map_or_else(
        || OAuthError::not_found().into_response(),
        |config| document(authorization_server_metadata(config)),
    )
}

fn protected_resource_response(config: Option<&OAuthConfig>, request_path: &str) -> Response {
    let Some(config) = config else {
        return OAuthError::not_found().into_response();
    };
    let served = protected_resource_well_known_path(&config.api_resource);
    if served.as_deref() == Some(request_path) {
        document(api_protected_resource_metadata(config))
    } else {
        (StatusCode::NOT_FOUND, Json(json!({"error": "not_found"}))).into_response()
    }
}

/// The two public routes. `config` is `None` when the feature is off, in which case both answer
/// 404 like any other path that does not exist.
pub fn routes<S>(config: Option<Arc<OAuthConfig>>) -> Router<S>
where
    S: Clone + Send + Sync + 'static,
{
    let authorization_server = config.clone();
    let protected_resource_root = config.clone();
    let protected_resource_path = config;
    Router::new()
        .route(
            AUTHORIZATION_SERVER_WELL_KNOWN,
            get(move || {
                let config = authorization_server.clone();
                async move { authorization_server_response(config.as_deref()) }
            }),
        )
        .route(
            PROTECTED_RESOURCE_WELL_KNOWN,
            get(move |uri: Uri| {
                let config = protected_resource_root.clone();
                async move { protected_resource_response(config.as_deref(), uri.path()) }
            }),
        )
        .route(
            "/.well-known/oauth-protected-resource/{*resource_path}",
            get(move |uri: Uri| {
                let config = protected_resource_path.clone();
                async move { protected_resource_response(config.as_deref(), uri.path()) }
            }),
        )
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::unwrap_used, clippy::panic, clippy::indexing_slicing)]
mod tests {
    use super::*;
    use axum::body::{Body, to_bytes};
    use axum::http::Request;
    use tower::ServiceExt;

    fn config() -> OAuthConfig {
        OAuthConfig::with_defaults("https://sv.example.test")
    }

    fn object(value: &Value) -> &serde_json::Map<String, Value> {
        value.as_object().expect("a JSON object")
    }

    #[test]
    fn the_authorization_server_document_matches_the_contract_field_by_field() {
        let document = authorization_server_metadata(&config());
        let expected = json!({
            "issuer": "https://sv.example.test",
            "authorization_endpoint": "https://sv.example.test/oauth/authorize",
            "token_endpoint": "https://sv.example.test/api/v1/oauth/token",
            "revocation_endpoint": "https://sv.example.test/api/v1/oauth/revoke",
            "response_types_supported": ["code"],
            "response_modes_supported": ["query"],
            "grant_types_supported": ["authorization_code", "refresh_token"],
            "code_challenge_methods_supported": ["S256"],
            "token_endpoint_auth_methods_supported": ["none"],
            "revocation_endpoint_auth_methods_supported": ["none"],
            "scopes_supported": [
                "projects:read", "work_items:read", "work_items:write", "forms:read", "forms:write",
                "notifications:read", "notifications:write", "tasks:read", "surfaces:embed", "offline_access"
            ],
            "authorization_response_iss_parameter_supported": true,
            "client_id_metadata_document_supported": false
        });
        assert_eq!(document, expected);
        // Field by field, so a failure names the field.
        let map = object(&document);
        for (key, value) in object(&expected) {
            assert_eq!(map.get(key), Some(value), "{key}");
        }
        assert_eq!(map.len(), 13);
        for absent in [
            "registration_endpoint",
            "introspection_endpoint",
            "jwks_uri",
            "userinfo_endpoint",
        ] {
            assert!(!map.contains_key(absent), "{absent} must not be advertised");
        }
    }

    #[test]
    fn the_issuer_is_taken_from_configuration_byte_for_byte() {
        let document = authorization_server_metadata(&OAuthConfig::with_defaults("http://127.0.0.1:18097"));
        assert_eq!(document["issuer"], "http://127.0.0.1:18097");
        assert_eq!(document["token_endpoint"], "http://127.0.0.1:18097/api/v1/oauth/token");
        assert_eq!(
            document["authorization_endpoint"],
            "http://127.0.0.1:18097/oauth/authorize"
        );
    }

    #[test]
    fn the_api_resource_document_matches_the_contract_field_by_field() {
        let document = api_protected_resource_metadata(&config());
        let expected = json!({
            "resource": "https://sv.example.test/api",
            "authorization_servers": ["https://sv.example.test"],
            "bearer_methods_supported": ["header"],
            "scopes_supported": [
                "projects:read", "work_items:read", "work_items:write", "forms:read", "forms:write",
                "notifications:read", "notifications:write", "tasks:read", "surfaces:embed"
            ],
            "resource_name": "Sylvode API"
        });
        assert_eq!(document, expected);
        assert_eq!(object(&document).len(), 5);
        assert!(
            !document["scopes_supported"]
                .as_array()
                .unwrap()
                .iter()
                .any(|s| s == "offline_access"),
            "offline_access belongs to the authorization server's list only"
        );
        let mut custom = config();
        custom.api_resource = "https://sv.example.test/api/v2".to_owned();
        assert_eq!(
            api_protected_resource_metadata(&custom)["resource"],
            "https://sv.example.test/api/v2"
        );
    }

    #[test]
    fn well_known_paths_follow_rfc_9728() {
        let cases = [
            (
                "https://sv.example.test/api",
                "/.well-known/oauth-protected-resource/api",
            ),
            (
                "https://sv.example.test/api/v2",
                "/.well-known/oauth-protected-resource/api/v2",
            ),
            ("https://sv.example.test", "/.well-known/oauth-protected-resource"),
            ("https://sv.example.test/", "/.well-known/oauth-protected-resource"),
            (
                "http://127.0.0.1:8090/mcp/rpc",
                "/.well-known/oauth-protected-resource/mcp/rpc",
            ),
            (
                "https://sv.example.test/api?x=1",
                "/.well-known/oauth-protected-resource/api",
            ),
        ];
        for (resource, path) in cases {
            assert_eq!(
                protected_resource_well_known_path(resource).as_deref(),
                Some(path),
                "{resource}"
            );
        }
        assert_eq!(protected_resource_well_known_path("not a url"), None);
        assert_eq!(
            api_resource_metadata_url(&config()).as_deref(),
            Some("https://sv.example.test/.well-known/oauth-protected-resource/api")
        );
        let mut elsewhere = config();
        elsewhere.api_resource = "https://api.example.test:8443/v1".to_owned();
        assert_eq!(
            api_resource_metadata_url(&elsewhere).as_deref(),
            Some("https://api.example.test:8443/.well-known/oauth-protected-resource/v1")
        );
    }

    async fn get(router: Router, path: &str) -> (StatusCode, axum::http::HeaderMap, Vec<u8>) {
        let response = router
            .oneshot(Request::builder().uri(path).body(Body::empty()).unwrap())
            .await
            .expect("response");
        let (parts, body) = response.into_parts();
        (
            parts.status,
            parts.headers,
            to_bytes(body, 64 * 1024).await.unwrap().to_vec(),
        )
    }

    #[tokio::test]
    async fn the_routes_serve_both_documents_with_a_one_hour_cache() {
        let router: Router = routes(Some(Arc::new(config())));
        let (status, headers, body) = get(router.clone(), AUTHORIZATION_SERVER_WELL_KNOWN).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(headers[header::CACHE_CONTROL], "public, max-age=3600");
        assert!(
            headers[header::CONTENT_TYPE]
                .to_str()
                .unwrap()
                .starts_with("application/json")
        );
        assert_eq!(
            serde_json::from_slice::<Value>(&body).unwrap(),
            authorization_server_metadata(&config())
        );

        let (status, headers, body) = get(router.clone(), "/.well-known/oauth-protected-resource/api").await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(headers[header::CACHE_CONTROL], "public, max-age=3600");
        assert_eq!(
            serde_json::from_slice::<Value>(&body).unwrap(),
            api_protected_resource_metadata(&config())
        );

        // Only the API's own path is served here: the MCP document belongs to the MCP server, and
        // the root form does not name an API whose resource has a path.
        for path in [
            "/.well-known/oauth-protected-resource",
            "/.well-known/oauth-protected-resource/mcp",
            "/.well-known/oauth-protected-resource/mcp/rpc",
            "/.well-known/oauth-protected-resource/api/extra",
        ] {
            assert_eq!(get(router.clone(), path).await.0, StatusCode::NOT_FOUND, "{path}");
        }
    }

    #[tokio::test]
    async fn a_resource_without_a_path_is_served_at_the_root_form() {
        let mut config = config();
        config.api_resource = "https://sv.example.test".to_owned();
        let router: Router = routes(Some(Arc::new(config)));
        assert_eq!(
            get(router.clone(), "/.well-known/oauth-protected-resource").await.0,
            StatusCode::OK
        );
        assert_eq!(
            get(router, "/.well-known/oauth-protected-resource/api").await.0,
            StatusCode::NOT_FOUND
        );
    }

    #[tokio::test]
    async fn without_an_issuer_every_document_is_a_404() {
        let router: Router = routes(None);
        for path in [
            AUTHORIZATION_SERVER_WELL_KNOWN,
            "/.well-known/oauth-protected-resource",
            "/.well-known/oauth-protected-resource/api",
        ] {
            let (status, headers, _) = get(router.clone(), path).await;
            assert_eq!(status, StatusCode::NOT_FOUND, "{path}");
            assert_eq!(
                headers[header::CACHE_CONTROL],
                "no-store",
                "a missing document is not cached"
            );
        }
    }

    #[tokio::test]
    async fn the_application_router_answers_404_when_the_feature_is_unset() {
        // The process never installs a configuration in unit tests, so `runtime().oauth` is `None`.
        assert!(crate::config::runtime().oauth.is_none());
        let router: Router = routes(crate::config::runtime().oauth.clone());
        assert_eq!(
            get(router, AUTHORIZATION_SERVER_WELL_KNOWN).await.0,
            StatusCode::NOT_FOUND
        );
    }
}
