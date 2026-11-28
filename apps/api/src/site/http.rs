//! The layer stack of the public site API (contract C3 section 6.4).
//!
//! It has its own error envelope with real HTTP status codes, the origin and CSRF checks, CORS for
//! a separate-origin site, the 1 MiB body limit, per-address rate limits and the response headers.
//!
//! The site router never runs under the console's `CorsLayer::permissive()` or its 200 MiB body
//! limit: `router/mod.rs` merges it after those layers are applied.

use std::collections::{HashMap, VecDeque};
use std::net::{IpAddr, SocketAddr};
use std::sync::LazyLock;
use std::time::{Duration, Instant};

use axum::{
    Json,
    body::Bytes,
    extract::{ConnectInfo, FromRequest, Request},
    http::{HeaderMap, HeaderValue, Method, StatusCode, header},
    middleware::Next,
    response::{IntoResponse, Response},
};
use parking_lot::Mutex;
use platform::app::AppState;
use serde::de::DeserializeOwned;
use serde_json::{Value, json};

use super::store::{self, SiteRow};
use crate::error::ApiError;
use crate::response::ApiResponse;

/// Largest request body the site API reads.
pub const MAX_BODY_BYTES: usize = 1024 * 1024;
/// Requests per minute and client address (IPv6 per /64).
const REQUESTS_PER_MINUTE: u32 = 120;
/// Marker header every write must carry.
const SITE_HEADER: &str = "x-sv-site";
const SITE_PREFIX: &str = "/api/site/v1/";

/// A refusal of the site API: the console envelope with the real HTTP status (LEAD C3-1).
#[derive(Debug)]
pub struct SiteError {
    status: StatusCode,
    code: &'static str,
    message: String,
    details: Option<Value>,
    retry_after: Option<u64>,
}

impl SiteError {
    pub(crate) fn new(status: StatusCode, code: &'static str, message: impl Into<String>) -> Self {
        Self {
            status,
            code,
            message: message.into(),
            details: None,
            retry_after: None,
        }
    }

    pub(crate) fn with_details(mut self, details: Value) -> Self {
        self.details = Some(details);
        self
    }

    pub(crate) fn not_found() -> Self {
        Self::new(StatusCode::NOT_FOUND, "not_found", "not found")
    }

    pub(crate) fn rate_limited(retry_after: u64) -> Self {
        let mut error = Self::new(StatusCode::TOO_MANY_REQUESTS, "rate_limited", "too many requests");
        error.retry_after = Some(retry_after.max(1));
        error
    }

    pub(crate) const fn code(&self) -> &'static str {
        self.code
    }

    /// Replaces the details with `f` of the current ones (`null` when there are none).
    pub(crate) fn map_details(mut self, f: impl FnOnce(&Value) -> Value) -> Self {
        let details = self.details.take().unwrap_or(Value::Null);
        self.details = Some(f(&details));
        self
    }
}

impl From<ApiError> for SiteError {
    fn from(error: ApiError) -> Self {
        let status = |code: i32| {
            u16::try_from(code)
                .ok()
                .and_then(|code| StatusCode::from_u16(code).ok())
                .unwrap_or(StatusCode::INTERNAL_SERVER_ERROR)
        };
        match error {
            ApiError::Coded {
                status: code,
                code: error_code,
                message,
                details,
            } => Self {
                // C2 refuses record values with 400; the site API answers 422 (C3 section 6.6).
                status: if error_code == crate::forms::rules::VALIDATION_FAILED {
                    StatusCode::UNPROCESSABLE_ENTITY
                } else {
                    status(code)
                },
                code: error_code,
                message,
                details,
                retry_after: None,
            },
            ApiError::Typed { kind, message, details } => Self {
                status: status(kind.http_status_code()),
                code: kind.stable_code(),
                message,
                details,
                retry_after: None,
            },
            ApiError::BadRequest(message) => Self::new(StatusCode::BAD_REQUEST, "bad_request", message),
            ApiError::Unauthorized(_) => Self::new(StatusCode::UNAUTHORIZED, "unauthorized", "unauthorized"),
            ApiError::Forbidden(message) => Self::new(StatusCode::FORBIDDEN, "forbidden", message),
            ApiError::NotFound(_) => Self::not_found(),
            ApiError::Conflict(message) => Self::new(StatusCode::CONFLICT, "conflict", message),
            ApiError::VersionConflict(reason) => Self::new(
                StatusCode::CONFLICT,
                "version_conflict",
                "the record changed since it was read",
            )
            .with_details(json!({ "reason": reason })),
            ApiError::Internal => Self::new(StatusCode::INTERNAL_SERVER_ERROR, "internal", "internal server error"),
            ApiError::Database(error) => {
                tracing::error!(error = %error, "site API database error");
                Self::new(StatusCode::INTERNAL_SERVER_ERROR, "internal", "internal server error")
            }
        }
    }
}

impl From<sea_orm::DbErr> for SiteError {
    fn from(error: sea_orm::DbErr) -> Self {
        ApiError::Database(error).into()
    }
}

impl IntoResponse for SiteError {
    fn into_response(self) -> Response {
        let body = ApiResponse::<Value> {
            code: i32::from(self.status.as_u16()),
            message: self.message,
            data: None,
            error_code: Some(self.code),
            details: self.details,
        };
        let mut response = (self.status, Json(body)).into_response();
        if let Some(seconds) = self.retry_after
            && let Ok(value) = HeaderValue::from_str(&seconds.to_string())
        {
            response.headers_mut().insert(header::RETRY_AFTER, value);
        }
        response
    }
}

/// A success answer with a status code.
pub(crate) fn ok<T: serde::Serialize>(status: StatusCode, data: T) -> Response {
    (status, ApiResponse::success(data)).into_response()
}

/// A JSON body read with the site limit; an oversized body is `413 body_too_large`, a body that
/// does not parse into `T` is `400 body_invalid`.
pub struct SiteJson<T>(pub T);

impl<T: DeserializeOwned, S: Send + Sync> FromRequest<S> for SiteJson<T> {
    type Rejection = SiteError;

    async fn from_request(req: Request, _state: &S) -> Result<Self, Self::Rejection> {
        let bytes: Bytes = axum::body::to_bytes(req.into_body(), MAX_BODY_BYTES)
            .await
            .map_err(|_| body_too_large())?;
        serde_json::from_slice(&bytes)
            .map(SiteJson)
            .map_err(|_| SiteError::new(StatusCode::BAD_REQUEST, "body_invalid", "the request body is not valid"))
    }
}

fn body_too_large() -> SiteError {
    SiteError::new(
        StatusCode::PAYLOAD_TOO_LARGE,
        "body_too_large",
        "the request body is too large",
    )
}

/// The slug of a site API path, if any.
fn path_slug(path: &str) -> Option<&str> {
    path.strip_prefix(SITE_PREFIX)?
        .split('/')
        .next()
        .filter(|slug| !slug.is_empty())
}

/// How a site is served (contract C3 section 6.4).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Mode {
    /// Mode (a): its own origin, named exactly in the site settings.
    Separate(String),
    /// Mode (c): under the console origin, only with `site.same_origin_trial`.
    SameOrigin,
}

pub(crate) fn mode_of(site: &SiteRow) -> Option<Mode> {
    site.allowed_origin().map_or_else(
        || {
            crate::config::runtime()
                .site
                .same_origin_trial
                .then_some(Mode::SameOrigin)
        },
        |origin| Some(Mode::Separate(origin.to_string())),
    )
}

/// Whether `origin` is the origin a write may come from in `mode`. In the same-origin mode it is
/// this server's configured `public_url` origin, else the origin whose host is the request's
/// `Host`.
pub(crate) fn origin_allowed(mode: &Mode, origin: &str, host: Option<&str>, public_url: Option<&str>) -> bool {
    match mode {
        Mode::Separate(allowed) => origin == allowed,
        Mode::SameOrigin => {
            if let Some(public) = public_url.and_then(origin_of) {
                return origin == public;
            }
            let Some(host) = host else { return false };
            ["https://", "http://"]
                .iter()
                .any(|scheme| origin.strip_prefix(scheme) == Some(host))
        }
    }
}

/// `scheme://host[:port]` of a URL.
fn origin_of(url: &str) -> Option<String> {
    let parsed = url::Url::parse(url).ok()?;
    let origin = parsed.origin();
    origin.is_tuple().then(|| origin.ascii_serialization())
}

/// The rate-limit key of an address: IPv4 whole, IPv6 by its /64 prefix (S9).
pub(crate) fn address_key(ip: IpAddr) -> String {
    match ip.to_canonical() {
        IpAddr::V4(v4) => v4.to_string(),
        IpAddr::V6(v6) => {
            let segments = v6.segments();
            format!(
                "{:x}:{:x}:{:x}:{:x}::/64",
                segments[0], segments[1], segments[2], segments[3]
            )
        }
    }
}

/// The client address behind the configured trusted proxies.
pub(crate) fn client_address(headers: &HeaderMap, peer: Option<SocketAddr>) -> IpAddr {
    crate::security::client_ip(headers, peer, &crate::config::runtime().trusted_proxies)
}

/// The rate-limit buckets of the site API. Each has its own bounded table and a fixed window
/// length, so one bucket filling up never refuses keys of another.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Bucket {
    /// Every request, per client address (per minute).
    Request,
    /// Form submissions, per site, binding and client address (per hour).
    Submit,
}

impl Bucket {
    const COUNT: usize = 2;

    const fn index(self) -> usize {
        match self {
            Self::Request => 0,
            Self::Submit => 1,
        }
    }

    const fn window(self) -> Duration {
        match self {
            Self::Submit => HOUR,
            Self::Request => MINUTE,
        }
    }
}

/// One bucket's table: a key map plus the keys in the order their windows started. Every window of
/// a bucket has the same length, so the oldest windows are the first to expire and expiry only ever
/// looks at the front of the queue (amortised constant time, no scan of the table).
struct Table {
    windows: HashMap<String, Window>,
    started: VecDeque<(Instant, String)>,
}

#[derive(Debug, Clone, Copy)]
struct Window {
    started: Instant,
    hits: u32,
}

impl Table {
    fn new() -> Self {
        Self {
            windows: HashMap::new(),
            started: VecDeque::new(),
        }
    }

    /// Drops the windows that have run out; the queue holds exactly the live windows afterwards.
    fn expire(&mut self, now: Instant, length: Duration) {
        while let Some((started, _)) = self.started.front() {
            if now.duration_since(*started) < length {
                break;
            }
            if let Some((_, key)) = self.started.pop_front() {
                self.windows.remove(&key);
            }
        }
    }
}

/// In-process fixed windows, one bounded table per [`Bucket`]. Past its cap a table first drops its
/// expired windows; if it is still full a new key is refused (fail closed) and every existing
/// window keeps counting.
pub(crate) struct Windows {
    tables: [Mutex<Table>; Bucket::COUNT],
    capacity: usize,
}

const MAX_WINDOWS: usize = 50_000;

impl Windows {
    fn new(capacity: usize) -> Self {
        Self {
            tables: [Mutex::new(Table::new()), Mutex::new(Table::new())],
            capacity,
        }
    }

    /// Counts a hit; `Err(retry_after_secs)` when the window already holds `limit` hits, or when
    /// the bucket's table is full and `key` is new.
    pub(crate) fn hit(&self, bucket: Bucket, key: String, limit: u32) -> Result<(), u64> {
        let length = bucket.window();
        let now = Instant::now();
        let Some(slot) = self.tables.get(bucket.index()) else {
            return Ok(());
        };
        let mut table = slot.lock();
        table.expire(now, length);
        if let Some(entry) = table.windows.get_mut(&key) {
            if entry.hits >= limit {
                let left = length.saturating_sub(now.duration_since(entry.started));
                return Err(left.as_secs().max(1));
            }
            entry.hits += 1;
            return Ok(());
        }
        if table.windows.len() >= self.capacity {
            return Err(length.as_secs().clamp(1, 60));
        }
        if limit == 0 {
            return Err(length.as_secs().max(1));
        }
        table.started.push_back((now, key.clone()));
        table.windows.insert(key, Window { started: now, hits: 1 });
        drop(table);
        Ok(())
    }
}

pub(crate) static WINDOWS: LazyLock<Windows> = LazyLock::new(|| Windows::new(MAX_WINDOWS));

pub(crate) const MINUTE: Duration = Duration::from_mins(1);
pub(crate) const HOUR: Duration = Duration::from_hours(1);

/// The guard in front of every site API route.
///
/// In order: any `Authorization` header is refused; an unknown, disabled or unreachable site is
/// `404`; a preflight is answered for a separate-origin site; a write needs `X-SV-Site: 1` and the
/// exact site origin; a declared body over 1 MiB is refused; the address is rate limited. The site row goes to the handler as a request extension. Every answer gets the site headers and,
/// for its own origin, CORS.
pub async fn guard(
    axum::extract::State(state): axum::extract::State<AppState>,
    mut request: Request,
    next: Next,
) -> Response {
    let origin = header_text(request.headers(), header::ORIGIN.as_str());
    let (response, cors) = match admit(&state, &mut request).await {
        Ok(Admitted::Preflight(mode)) => (preflight(), allowed_cors(&mode, origin.as_deref())),
        Ok(Admitted::Pass(mode)) => (next.run(request).await, allowed_cors(&mode, origin.as_deref())),
        Err(error) => (error.into_response(), None),
    };
    finish(response, cors)
}

enum Admitted {
    Preflight(Mode),
    Pass(Mode),
}

async fn admit(state: &AppState, request: &mut Request) -> Result<Admitted, SiteError> {
    if request.headers().contains_key(header::AUTHORIZATION) {
        return Err(SiteError::new(
            StatusCode::UNAUTHORIZED,
            "site_credentials_refused",
            "the site API takes no Authorization header",
        ));
    }
    let slug = path_slug(request.uri().path())
        .map(str::to_owned)
        .ok_or_else(SiteError::not_found)?;
    let site = store::load_site_by_slug(&state.db, &slug)
        .await?
        .filter(|site| !site.is_disabled())
        .ok_or_else(SiteError::not_found)?;
    let mode = mode_of(&site).ok_or_else(SiteError::not_found)?;
    let headers = request.headers();
    let origin = header_text(headers, header::ORIGIN.as_str());
    let host = header_text(headers, header::HOST.as_str());
    let public_url = crate::config::runtime().public_url.clone();
    let origin_ok = |origin: Option<&str>| {
        origin.is_some_and(|origin| origin_allowed(&mode, origin, host.as_deref(), public_url.as_deref()))
    };
    let method = request.method().clone();
    if method == Method::OPTIONS {
        return if matches!(mode, Mode::Separate(_)) && origin_ok(origin.as_deref()) {
            Ok(Admitted::Preflight(mode))
        } else {
            Err(origin_refused())
        };
    }
    if method != Method::GET && method != Method::HEAD {
        let marked = header_text(headers, SITE_HEADER).is_some_and(|value| value == "1");
        if !marked || !origin_ok(origin.as_deref()) {
            return Err(origin_refused());
        }
    }
    let declared = header_text(headers, header::CONTENT_LENGTH.as_str()).and_then(|value| value.parse::<usize>().ok());
    if declared.is_some_and(|length| length > MAX_BODY_BYTES) {
        return Err(body_too_large());
    }
    let peer = request.extensions().get::<ConnectInfo<SocketAddr>>().map(|info| info.0);
    let address = address_key(client_address(headers, peer));
    WINDOWS
        .hit(Bucket::Request, address.clone(), REQUESTS_PER_MINUTE)
        .map_err(SiteError::rate_limited)?;
    request.extensions_mut().insert(ClientAddress(address));
    request.extensions_mut().insert(site);
    Ok(Admitted::Pass(mode))
}

/// The rate-limit key of the request's client address, set by [`guard`].
#[derive(Debug, Clone)]
pub struct ClientAddress(pub(crate) String);

fn origin_refused() -> SiteError {
    SiteError::new(
        StatusCode::FORBIDDEN,
        "origin_refused",
        "the request origin is not allowed",
    )
}

fn header_text(headers: &HeaderMap, name: &str) -> Option<String> {
    headers
        .get(name)
        .and_then(|value| value.to_str().ok())
        .map(str::to_owned)
}

/// The origin CORS answers to: only a separate-origin site's own origin, with credentials.
fn allowed_cors(mode: &Mode, origin: Option<&str>) -> Option<HeaderValue> {
    match (mode, origin) {
        (Mode::Separate(allowed), Some(origin)) if origin == allowed => HeaderValue::from_str(origin).ok(),
        _ => None,
    }
}

fn preflight() -> Response {
    let mut response = StatusCode::NO_CONTENT.into_response();
    let headers = response.headers_mut();
    headers.insert(
        header::ACCESS_CONTROL_ALLOW_METHODS,
        HeaderValue::from_static("GET, POST, PATCH"),
    );
    headers.insert(
        header::ACCESS_CONTROL_ALLOW_HEADERS,
        HeaderValue::from_static("content-type, x-sv-site"),
    );
    headers.insert(header::ACCESS_CONTROL_MAX_AGE, HeaderValue::from_static("600"));
    response
}

fn finish(mut response: Response, cors: Option<HeaderValue>) -> Response {
    let headers = response.headers_mut();
    headers.insert(header::X_CONTENT_TYPE_OPTIONS, HeaderValue::from_static("nosniff"));
    headers.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    headers.insert(
        header::CONTENT_SECURITY_POLICY,
        HeaderValue::from_static("default-src 'none'; frame-ancestors 'none'"),
    );
    headers.append(header::VARY, HeaderValue::from_static("Origin"));
    if let Some(origin) = cors {
        headers.insert(header::ACCESS_CONTROL_ALLOW_ORIGIN, origin);
        headers.insert(
            header::ACCESS_CONTROL_ALLOW_CREDENTIALS,
            HeaderValue::from_static("true"),
        );
    }
    response
}

/// Appends a `Set-Cookie` header.
pub(crate) fn set_cookie(response: &mut Response, cookie: &str) {
    if let Ok(value) = HeaderValue::from_str(cookie) {
        response.headers_mut().append(header::SET_COOKIE, value);
    }
}

/// An empty body for the routes that answer `{}`.
pub(crate) fn empty_object() -> Value {
    json!({})
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use axum::body::Body;

    #[test]
    fn ipv6_addresses_share_a_key_per_64() {
        let a: IpAddr = "2001:db8:1:2:aaaa::1".parse().unwrap();
        let b: IpAddr = "2001:db8:1:2:ffff::9".parse().unwrap();
        let c: IpAddr = "2001:db8:1:3::1".parse().unwrap();
        assert_eq!(address_key(a), address_key(b));
        assert_ne!(address_key(a), address_key(c));
        let mapped: IpAddr = "::ffff:192.0.2.7".parse().unwrap();
        assert_eq!(address_key(mapped), "192.0.2.7");
    }

    #[test]
    fn the_same_origin_mode_compares_with_host_or_public_url() {
        let mode = Mode::SameOrigin;
        assert!(origin_allowed(
            &mode,
            "http://qa.local:47097",
            Some("qa.local:47097"),
            None
        ));
        assert!(!origin_allowed(&mode, "http://evil.test", Some("qa.local:47097"), None));
        assert!(!origin_allowed(&mode, "http://qa.local:47097", None, None));
        assert!(origin_allowed(
            &mode,
            "https://sv.example.com",
            Some("x"),
            Some("https://sv.example.com/app")
        ));
        assert!(!origin_allowed(
            &mode,
            "https://qa.local",
            Some("qa.local"),
            Some("https://sv.example.com")
        ));
    }

    #[test]
    fn the_separate_mode_needs_the_exact_origin() {
        let mode = Mode::Separate("https://help.example.com".to_string());
        assert!(origin_allowed(&mode, "https://help.example.com", None, None));
        assert!(!origin_allowed(&mode, "https://help.example.com:444", None, None));
        assert!(!origin_allowed(&mode, "null", None, None));
    }

    #[test]
    fn a_window_counts_to_its_limit() {
        let windows = Windows::new(16);
        assert!(windows.hit(Bucket::Request, "k".into(), 2).is_ok());
        assert!(windows.hit(Bucket::Request, "k".into(), 2).is_ok());
        assert!(windows.hit(Bucket::Request, "k".into(), 2).is_err());
        assert!(windows.hit(Bucket::Request, "other".into(), 2).is_ok());
    }

    #[test]
    fn a_full_bucket_refuses_new_keys_without_touching_other_buckets() {
        let windows = Windows::new(2);
        for _ in 0..5 {
            let _ = windows.hit(Bucket::Request, "a".into(), 5);
        }
        assert!(windows.hit(Bucket::Request, "b".into(), 5).is_ok());
        assert!(
            windows.hit(Bucket::Request, "c".into(), 5).is_err(),
            "a new key is refused when its bucket is full"
        );
        assert!(
            windows.hit(Bucket::Request, "a".into(), 5).is_err(),
            "the used-up window survives"
        );
        assert!(windows.hit(Bucket::Submit, "x".into(), 20).is_ok());
        assert!(
            windows.hit(Bucket::Submit, "x".into(), 20).is_ok(),
            "another bucket is not starved"
        );
    }

    #[test]
    fn slugs_come_from_the_path() {
        assert_eq!(path_slug("/api/site/v1/help-desk/manifest"), Some("help-desk"));
        assert_eq!(path_slug("/api/site/v1/"), None);
        assert_eq!(path_slug("/api/v1/sites"), None);
    }

    #[tokio::test]
    async fn an_oversized_body_is_refused_with_its_code() {
        let request = Request::builder()
            .body(Body::from(vec![b' '; MAX_BODY_BYTES + 1]))
            .unwrap();
        let refused = SiteJson::<Value>::from_request(request, &()).await.err().unwrap();
        assert_eq!(refused.code(), "body_too_large");
        assert_eq!(refused.status, StatusCode::PAYLOAD_TOO_LARGE);
    }

    #[test]
    fn validation_failures_answer_422() {
        let error: SiteError = ApiError::Coded {
            status: 400,
            code: "validation_failed",
            message: "validation failed".into(),
            details: None,
        }
        .into();
        assert_eq!(error.status, StatusCode::UNPROCESSABLE_ENTITY);
        let conflict: SiteError = ApiError::VersionConflict("record_version_conflict").into();
        assert_eq!(conflict.status, StatusCode::CONFLICT);
    }
}
