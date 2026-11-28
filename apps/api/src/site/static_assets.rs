//! The built site app, served under the API origin at `site.mount_path`, `/site` by default (decision SITE-STATIC-1/2).
//!
//! Active only when `site.same_origin_trial` is on and `site.assets_dir` is set; otherwise
//! [`service`] returns `None` and the mount path answers the standard 404. Paths are resolved by
//! `ServeDir`, which refuses every path component that is not a plain name, so nothing outside
//! `assets_dir` is reachable. A path that names no file answers `index.html` (client-side routes).

use std::path::Path;

use axum::{
    Router,
    extract::Request,
    http::{HeaderValue, header},
    middleware::{self, Next},
    response::Response,
};
use tower_http::services::{ServeDir, ServeFile};

/// Contract C3 section 6.4. Static files carry `frame-ancestors 'none'` for every site.
pub const CONTENT_SECURITY_POLICY: &str = "default-src 'self'; script-src 'self'; object-src 'none'; \
base-uri 'none'; form-action 'self'; frame-ancestors 'none'";

/// The router of the static site app, `None` when the configuration does not serve it.
#[must_use]
pub fn service<S>(site: &platform::config::SiteConfig) -> Option<Router<S>>
where
    S: Clone + Send + Sync + 'static,
{
    site.served_assets_dir().map(|dir| router_for(dir, &site.mount_path))
}

fn router_for<S>(dir: &Path, mount_path: &str) -> Router<S>
where
    S: Clone + Send + Sync + 'static,
{
    let files = ServeDir::new(dir)
        .append_index_html_on_directories(true)
        .fallback(ServeFile::new(dir.join("index.html")));
    Router::new()
        .nest_service(mount_path, files)
        .layer(middleware::from_fn(security_headers))
}

async fn security_headers(request: Request, next: Next) -> Response {
    let mut response = next.run(request).await;
    let headers = response.headers_mut();
    headers.insert(
        header::CONTENT_SECURITY_POLICY,
        HeaderValue::from_static(CONTENT_SECURITY_POLICY),
    );
    headers.insert(header::X_CONTENT_TYPE_OPTIONS, HeaderValue::from_static("nosniff"));
    response
}

#[cfg(test)]
#[allow(clippy::expect_used)]
mod tests {
    use axum::{
        body::Body,
        http::{Request, StatusCode, header},
    };
    use platform::config::SiteConfig;
    use tower::ServiceExt;

    use super::{CONTENT_SECURITY_POLICY, service};

    #[test]
    fn nothing_is_mounted_without_the_trial_switch_or_the_directory() {
        let dir = Some(std::path::PathBuf::from("site-dist"));
        let no_switch = SiteConfig {
            assets_dir: dir,
            ..SiteConfig::default()
        };
        assert!(service::<()>(&no_switch).is_none());
        assert!(service::<()>(&SiteConfig::default()).is_none());
    }

    async fn get(app: axum::Router, uri: &str) -> axum::response::Response {
        app.oneshot(
            Request::builder()
                .uri(uri)
                .body(Body::empty())
                .expect("the request is valid"),
        )
        .await
        .expect("the service answers")
    }

    fn assert_security_headers(response: &axum::response::Response) {
        assert_eq!(
            response
                .headers()
                .get(header::CONTENT_SECURITY_POLICY)
                .and_then(|v| v.to_str().ok()),
            Some(CONTENT_SECURITY_POLICY)
        );
        assert_eq!(
            response
                .headers()
                .get(header::X_CONTENT_TYPE_OPTIONS)
                .and_then(|v| v.to_str().ok()),
            Some("nosniff")
        );
    }

    fn site_dir(tag: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("sv-static-assets-{tag}-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("the test directory is writable");
        std::fs::write(dir.join("index.html"), "<html></html>").expect("the index is writable");
        dir
    }

    #[tokio::test]
    async fn an_unknown_path_answers_the_index_at_the_default_mount_path() {
        let dir = site_dir("default");
        let site = SiteConfig {
            same_origin_trial: true,
            assets_dir: Some(dir.clone()),
            ..SiteConfig::default()
        };
        let app = service::<()>(&site).expect("both keys are set");
        let response = get(app.clone(), "/site/some/deep/path").await;
        assert_eq!(response.status(), StatusCode::OK);
        assert_security_headers(&response);
        // Outside the mount path nothing is served.
        assert_eq!(get(app, "/portal/x").await.status(), StatusCode::NOT_FOUND);
        std::fs::remove_dir_all(&dir).expect("the test directory is removable");
    }

    #[tokio::test]
    async fn a_custom_mount_path_serves_the_app_there_and_not_at_site() {
        let dir = site_dir("custom");
        let site = SiteConfig {
            same_origin_trial: true,
            assets_dir: Some(dir.clone()),
            mount_path: "/portal".to_string(),
            ..SiteConfig::default()
        };
        let app = service::<()>(&site).expect("both keys are set");
        let response = get(app.clone(), "/portal/some/deep/path").await;
        assert_eq!(response.status(), StatusCode::OK);
        assert_security_headers(&response);
        assert_eq!(get(app.clone(), "/portal").await.status(), StatusCode::OK);
        assert_eq!(get(app, "/site/x").await.status(), StatusCode::NOT_FOUND);
        std::fs::remove_dir_all(&dir).expect("the test directory is removable");
    }
}
