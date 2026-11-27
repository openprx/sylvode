//! Defence in depth: a request whose route has no catalogue entry is refused.
//!
//! After a clean boot check this cannot happen; the layer closes the gap if a route or a method is
//! ever registered in a form the source scan does not see.

use axum::{
    extract::{MatchedPath, Request},
    http::StatusCode,
    middleware::Next,
    response::{IntoResponse, Response},
};

use super::refusal::{Refusal, RefusalCode};
use super::uncatalogued::is_uncatalogued;
use crate::error::ApiError;
use crate::extensions::InternalCall;

/// Decides a request by `(method, matched path)`:
///
/// * no matched route (the fallback answers 404, so nothing reveals which routes exist) and the
///   `UNCATALOGUED` OAuth paths pass unchanged;
/// * a method with an entry passes; `HEAD` is the router's own answer to a `GET` route and is
///   looked up as `GET`;
/// * a known path with no entry for the method is answered `405`, as the router answers a method
///   that is not registered, so a registered but uncatalogued method fails closed;
/// * a matched path with no entry at all is refused `capability_unclassified`.
///
/// A request the plugin host dispatched carries [`InternalCall`]: its route must be the entry of the
/// operation the host checked (`capability_not_exposed` otherwise) and must not be an `extensions.*`
/// route (`plugin_reentry`). The authentication branch then re-checks the principal against the
/// resolved target.
pub async fn guard(request: Request, next: Next) -> Response {
    let verdict = match request.extensions().get::<MatchedPath>() {
        None => Verdict::Pass,
        Some(matched) => match request.extensions().get::<InternalCall>() {
            Some(call) => internal_verdict(request.method().as_str(), matched.as_str(), call.op),
            None => verdict(request.method().as_str(), matched.as_str()),
        },
    };
    match verdict {
        Verdict::Pass => next.run(request).await,
        Verdict::Refuse(code) => ApiError::from(Refusal::new(code, None)).into_response(),
        Verdict::MethodNotAllowed => StatusCode::METHOD_NOT_ALLOWED.into_response(),
        Verdict::Unclassified => {
            ApiError::from(Refusal::new(RefusalCode::CapabilityUnclassified, None)).into_response()
        }
    }
}

#[derive(Debug, PartialEq, Eq)]
enum Verdict {
    Pass,
    Refuse(RefusalCode),
    MethodNotAllowed,
    Unclassified,
}

/// The verdict for a request that carries [`InternalCall`] with the operation `op`.
fn internal_verdict(method: &str, path: &str, op: &str) -> Verdict {
    match super::by_route(method, path) {
        Some(entry) if entry.op.starts_with("extensions.") => Verdict::Refuse(RefusalCode::PluginReentry),
        Some(entry) if entry.op == op => Verdict::Pass,
        _ => Verdict::Refuse(RefusalCode::CapabilityNotExposed),
    }
}

fn verdict(method: &str, path: &str) -> Verdict {
    if is_uncatalogued(path) || super::by_route(method, path).is_some() {
        Verdict::Pass
    } else if super::knows_path(path) {
        Verdict::MethodNotAllowed
    } else {
        Verdict::Unclassified
    }
}

#[cfg(test)]
#[allow(clippy::expect_used)]
mod tests {
    use axum::{
        Router,
        body::{Body, to_bytes},
        http::{Method, Request, StatusCode},
        middleware::from_fn,
        routing::{any, get},
    };
    use tower::ServiceExt;

    use super::guard;

    fn app() -> Router {
        Router::new()
            .route("/health", get(|| async { "ok" }))
            .route("/not-in-the-catalogue", get(|| async { "ok" }))
            .route("/api/v1/oauth/token", any(|| async { "ok" }))
            .layer(from_fn(guard))
    }

    async fn call(method: Method, uri: &str) -> (StatusCode, String) {
        let response = app()
            .oneshot(
                Request::builder()
                    .method(method)
                    .uri(uri)
                    .body(Body::empty())
                    .expect("request"),
            )
            .await
            .expect("response");
        let status = response.status();
        let body = to_bytes(response.into_body(), 1 << 16).await.expect("body");
        (status, String::from_utf8_lossy(&body).into_owned())
    }

    #[tokio::test]
    async fn unmatched_requests_get_the_plain_404() {
        assert_eq!(
            call(Method::GET, "/nothing-here").await,
            (StatusCode::NOT_FOUND, String::new())
        );
    }

    #[tokio::test]
    async fn a_catalogued_route_and_head_pass() {
        assert_eq!(call(Method::GET, "/health").await, (StatusCode::OK, "ok".to_string()));
        assert_eq!(call(Method::HEAD, "/health").await.0, StatusCode::OK);
    }

    #[tokio::test]
    async fn uncatalogued_oauth_paths_pass_for_every_method() {
        for method in [Method::GET, Method::POST, Method::DELETE] {
            assert_eq!(call(method, "/api/v1/oauth/token").await.0, StatusCode::OK);
        }
    }

    #[tokio::test]
    async fn a_route_without_an_entry_is_refused() {
        let (status, body) = call(Method::GET, "/not-in-the-catalogue").await;
        assert_eq!(status, StatusCode::OK);
        assert!(body.contains("capability_unclassified"), "{body}");
    }

    #[test]
    fn an_internal_call_must_match_its_operation_and_stay_out_of_the_extension_routes() {
        use super::{Verdict, internal_verdict};
        use crate::catalogue::RefusalCode;
        let path = "/api/v1/form-records/{record_id}";
        assert_eq!(internal_verdict("GET", path, "forms.records.read"), Verdict::Pass);
        assert_eq!(
            internal_verdict("PATCH", path, "forms.records.read"),
            Verdict::Refuse(RefusalCode::CapabilityNotExposed)
        );
        assert_eq!(
            internal_verdict("GET", "/api/v1/nothing", "forms.records.read"),
            Verdict::Refuse(RefusalCode::CapabilityNotExposed)
        );
    }

    #[tokio::test]
    async fn a_known_path_with_an_uncatalogued_method_is_405() {
        assert_eq!(call(Method::POST, "/health").await.0, StatusCode::METHOD_NOT_ALLOWED);
    }

    #[test]
    fn the_decision_for_a_known_path_with_an_uncatalogued_method_is_405() {
        use super::{Verdict, verdict};
        // `/api/v1/forms/schema/validate` is catalogued for POST only.
        let path = "/api/v1/forms/schema/validate";
        assert_eq!(verdict("POST", path), Verdict::Pass);
        for method in ["GET", "PUT", "PATCH", "DELETE"] {
            assert_eq!(verdict(method, path), Verdict::MethodNotAllowed, "{method}");
        }
        assert_eq!(verdict("GET", "/api/v1/nothing-registered"), Verdict::Unclassified);
    }
}
