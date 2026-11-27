//! HTTP rules of the modern era: `Mcp-*` header validation, the status of a JSON-RPC error, the
//! `Origin` check and the Server-Sent Events response of a listen request.

use crate::era::Era;
use crate::protocol::{JsonRpcError, JsonRpcRequest, JsonRpcResponse};
use axum::{
    Json,
    extract::{Request, State},
    http::{HeaderMap, HeaderValue, StatusCode, header},
    middleware::Next,
    response::{
        IntoResponse, Response,
        sse::{Event, KeepAlive, Sse},
    },
};
use base64::{Engine, engine::general_purpose::STANDARD};
use serde_json::{Value, json};
use std::{convert::Infallible, sync::Arc, time::Duration};
use tokio_stream::Stream;

pub const HEADER_PROTOCOL_VERSION: &str = "MCP-Protocol-Version";
pub const HEADER_METHOD: &str = "Mcp-Method";
pub const HEADER_NAME: &str = "Mcp-Name";

/// The text of a header value. A value written `=?base64?<standard base64>?=` is decoded (UTF-8)
/// first; one that cannot be decoded is not text.
fn header_text(value: &HeaderValue) -> Option<String> {
    let raw = value.to_str().ok()?;
    match raw.strip_prefix("=?base64?").and_then(|rest| rest.strip_suffix("?=")) {
        Some(encoded) => String::from_utf8(STANDARD.decode(encoded).ok()?).ok(),
        None => Some(raw.to_string()),
    }
}

fn check_header(headers: &HeaderMap, name: &'static str, expected: &str) -> Result<(), JsonRpcError> {
    let Some(value) = headers.get(name) else {
        return Err(JsonRpcError::header_mismatch(name, "missing"));
    };
    if header_text(value).as_deref() == Some(expected) {
        Ok(())
    } else {
        Err(JsonRpcError::header_mismatch(name, "mismatch"))
    }
}

/// The body field `Mcp-Name` has to repeat, for the methods that name something.
fn named_parameter(req: &JsonRpcRequest) -> Option<&str> {
    let key = match req.method.as_str() {
        "tools/call" | "prompts/get" => "name",
        "resources/read" => "uri",
        _ => return None,
    };
    req.params.as_ref()?.get(key)?.as_str()
}

/// Checks the headers of a modern request against its body.
///
/// `MCP-Protocol-Version` is the `_meta` version, `Mcp-Method` the method and, for `tools/call`,
/// `resources/read` and `prompts/get`, `Mcp-Name` the name or URI. A legacy request is never
/// checked. A body that lacks the name is left to the method's own validation.
pub fn validate_headers(headers: &HeaderMap, req: &JsonRpcRequest, era: &Era) -> Result<(), JsonRpcError> {
    let Era::Modern { version, .. } = era else {
        return Ok(());
    };
    check_header(headers, HEADER_PROTOCOL_VERSION, version)?;
    check_header(headers, HEADER_METHOD, &req.method)?;
    if let Some(name) = named_parameter(req) {
        check_header(headers, HEADER_NAME, name)?;
    }
    Ok(())
}

/// The HTTP status a JSON-RPC error has in the modern era; a legacy request always gets `200`.
pub const fn status_for(error: &JsonRpcError, modern: bool) -> StatusCode {
    if !modern {
        return StatusCode::OK;
    }
    match error.code {
        -32020 | -32022 | -32700 | -32600 => StatusCode::BAD_REQUEST,
        -32601 => StatusCode::NOT_FOUND,
        -32000 => StatusCode::TOO_MANY_REQUESTS,
        _ => StatusCode::OK,
    }
}

/// [`status_for`] for a whole response: `200` when it carries a result.
pub fn status_for_response(response: &JsonRpcResponse, modern: bool) -> StatusCode {
    response
        .error
        .as_ref()
        .map_or(StatusCode::OK, |error| status_for(error, modern))
}

/// A JSON-RPC error as an HTTP response with the status of [`status_for`]; the too-many-streams
/// refusal carries `Retry-After`.
pub fn error_response(id: Option<Value>, error: JsonRpcError, modern: bool) -> Response {
    let status = status_for(&error, modern);
    let retry = error.code == -32000 && modern;
    let mut response = (status, Json(json!(JsonRpcResponse::error(id, error)))).into_response();
    if retry {
        response
            .headers_mut()
            .insert(header::RETRY_AFTER, HeaderValue::from(crate::listen::RETRY_AFTER_SECS));
    }
    response
}

/// Whether a request may proceed as far as its `Origin` goes: without the header it may (agent
/// CLIs and servers send none), with it the value must be configured exactly.
pub fn origin_allowed(headers: &HeaderMap, allowed: &[String]) -> bool {
    let mut origins = headers.get_all(header::ORIGIN).iter();
    let Some(origin) = origins.next() else {
        return true;
    };
    // A browser sends one `Origin`; a request with several is not from one.
    origins.next().is_none()
        && origin
            .to_str()
            .is_ok_and(|origin| allowed.iter().any(|known| known == origin))
}

/// Lets a log line through at most once per second, so a caller that is refused over and over
/// cannot fill the log.
pub struct LogRate {
    last_second: std::sync::atomic::AtomicU64,
}

impl LogRate {
    pub const fn new() -> Self {
        Self {
            last_second: std::sync::atomic::AtomicU64::new(0),
        }
    }

    /// Whether a line may be written now.
    pub fn allow(&self) -> bool {
        use std::sync::atomic::Ordering;
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |elapsed| elapsed.as_secs().max(1));
        let last = self.last_second.load(Ordering::Relaxed);
        last != now
            && self
                .last_second
                .compare_exchange(last, now, Ordering::Relaxed, Ordering::Relaxed)
                .is_ok()
    }
}

impl Default for LogRate {
    fn default() -> Self {
        Self::new()
    }
}

static ORIGIN_REFUSAL_LOG: LogRate = LogRate::new();

/// The answer to a request whose `Origin` is not allowed.
pub fn origin_refusal() -> Response {
    (StatusCode::FORBIDDEN, Json(json!({ "error": "origin not allowed" }))).into_response()
}

/// The layer that applies [`origin_allowed`] to a route. It runs before authentication.
pub async fn origin_layer(State(allowed): State<Arc<Vec<String>>>, request: Request, next: Next) -> Response {
    if origin_allowed(request.headers(), &allowed) {
        return next.run(request).await;
    }
    if ORIGIN_REFUSAL_LOG.allow() {
        tracing::warn!(path = %request.uri().path(), "Refused an MCP request with an Origin that is not allowed");
    }
    origin_refusal()
}

/// The response of a listen request: one `data:` event per JSON-RPC message, a comment line every
/// `keepalive`, no buffering by a proxy. The response ends when `messages` ends.
pub fn listen_sse_response<S>(messages: S, keepalive: Duration) -> Response
where
    S: Stream<Item = String> + Send + 'static,
{
    use tokio_stream::StreamExt;
    let events = messages.map(|message| Ok::<Event, Infallible>(Event::default().data(message)));
    let mut response = Sse::new(events)
        .keep_alive(KeepAlive::new().interval(keepalive).text(""))
        .into_response();
    response
        .headers_mut()
        .insert("x-accel-buffering", HeaderValue::from_static("no"));
    response
}

#[cfg(test)]
#[allow(clippy::indexing_slicing)]
mod tests {
    use super::*;
    use crate::era::MODERN_VERSION;

    fn modern() -> Era {
        Era::Modern {
            version: MODERN_VERSION,
            client_info: None,
            client_capabilities: json!({}),
        }
    }

    fn request(method: &str, params: Value) -> JsonRpcRequest {
        JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            id: Some(json!(1)),
            method: method.to_string(),
            params: Some(params),
        }
    }

    fn headers(pairs: &[(&str, &str)]) -> HeaderMap {
        let mut map = HeaderMap::new();
        for (name, value) in pairs {
            let (Ok(name), Ok(value)) = (
                header::HeaderName::from_bytes(name.as_bytes()),
                HeaderValue::from_str(value),
            ) else {
                continue;
            };
            map.insert(name, value);
        }
        map
    }

    fn detail(result: Result<(), JsonRpcError>) -> Option<(i32, String, String)> {
        result.err().map(|error| {
            let data = error.data.unwrap_or(Value::Null);
            (
                error.code,
                data["header"].as_str().unwrap_or_default().to_string(),
                data["detail"].as_str().unwrap_or_default().to_string(),
            )
        })
    }

    const GOOD: [(&str, &str); 2] = [("MCP-Protocol-Version", "2026-07-28"), ("Mcp-Method", "tools/list")];

    #[test]
    fn a_matching_request_passes_and_names_are_case_insensitive() {
        let req = request("tools/list", json!({}));
        assert!(validate_headers(&headers(&GOOD), &req, &modern()).is_ok());
        let lower = headers(&[("mcp-protocol-version", "2026-07-28"), ("MCP-METHOD", "tools/list")]);
        assert!(validate_headers(&lower, &req, &modern()).is_ok());
    }

    #[test]
    fn a_legacy_request_is_never_checked() {
        let req = request("tools/call", json!({"name": "x"}));
        assert!(validate_headers(&HeaderMap::new(), &req, &Era::Legacy).is_ok());
    }

    #[test]
    fn a_missing_or_different_protocol_version_is_refused() {
        let req = request("tools/list", json!({}));
        assert_eq!(
            detail(validate_headers(
                &headers(&[("Mcp-Method", "tools/list")]),
                &req,
                &modern()
            )),
            Some((-32020, "MCP-Protocol-Version".into(), "missing".into()))
        );
        let other = headers(&[("MCP-Protocol-Version", "2025-06-18"), ("Mcp-Method", "tools/list")]);
        assert_eq!(
            detail(validate_headers(&other, &req, &modern())),
            Some((-32020, "MCP-Protocol-Version".into(), "mismatch".into()))
        );
    }

    #[test]
    fn a_missing_or_different_method_is_refused() {
        let req = request("tools/list", json!({}));
        let none = headers(&[("MCP-Protocol-Version", "2026-07-28")]);
        assert_eq!(
            detail(validate_headers(&none, &req, &modern())),
            Some((-32020, "Mcp-Method".into(), "missing".into()))
        );
        let other = headers(&[("MCP-Protocol-Version", "2026-07-28"), ("Mcp-Method", "tools/call")]);
        assert_eq!(
            detail(validate_headers(&other, &req, &modern())),
            Some((-32020, "Mcp-Method".into(), "mismatch".into()))
        );
    }

    #[test]
    fn the_name_header_is_required_for_calls_reads_and_prompts() {
        let base = [("MCP-Protocol-Version", "2026-07-28"), ("Mcp-Method", "tools/call")];
        let call = request("tools/call", json!({"name": "projects.list"}));
        assert_eq!(
            detail(validate_headers(&headers(&base), &call, &modern())),
            Some((-32020, "Mcp-Name".into(), "missing".into()))
        );
        let wrong = headers(&[base[0], base[1], ("Mcp-Name", "other")]);
        assert_eq!(
            detail(validate_headers(&wrong, &call, &modern())),
            Some((-32020, "Mcp-Name".into(), "mismatch".into()))
        );
        let right = headers(&[base[0], base[1], ("Mcp-Name", "projects.list")]);
        assert!(validate_headers(&right, &call, &modern()).is_ok());

        let read = request("resources/read", json!({"uri": "sylvode://issues/PRX-1"}));
        let read_headers = headers(&[
            ("MCP-Protocol-Version", "2026-07-28"),
            ("Mcp-Method", "resources/read"),
            ("Mcp-Name", "sylvode://issues/PRX-1"),
        ]);
        assert!(validate_headers(&read_headers, &read, &modern()).is_ok());
        let prompt = request("prompts/get", json!({"name": "p"}));
        let prompt_headers = headers(&[("MCP-Protocol-Version", "2026-07-28"), ("Mcp-Method", "prompts/get")]);
        assert!(validate_headers(&prompt_headers, &prompt, &modern()).is_err());
    }

    #[test]
    fn a_body_without_the_name_is_left_to_the_method() {
        let call = request("tools/call", json!({}));
        let only = headers(&[("MCP-Protocol-Version", "2026-07-28"), ("Mcp-Method", "tools/call")]);
        assert!(validate_headers(&only, &call, &modern()).is_ok());
    }

    #[test]
    fn the_base64_sentinel_is_decoded_before_comparison() {
        let call = request("tools/call", json!({"name": "projekt.liste ü"}));
        let encoded = STANDARD.encode("projekt.liste ü");
        let sentinel = format!("=?base64?{encoded}?=");
        let good = headers(&[
            ("MCP-Protocol-Version", "2026-07-28"),
            ("Mcp-Method", "tools/call"),
            ("Mcp-Name", &sentinel),
        ]);
        assert!(validate_headers(&good, &call, &modern()).is_ok());
        for broken in ["=?base64?%%%?=", "=?base64?/w==?=", "=?base64?cHJvamVrdA==?"] {
            let bad = headers(&[
                ("MCP-Protocol-Version", "2026-07-28"),
                ("Mcp-Method", "tools/call"),
                ("Mcp-Name", broken),
            ]);
            assert_eq!(
                detail(validate_headers(&bad, &call, &modern())),
                Some((-32020, "Mcp-Name".into(), "mismatch".into())),
                "{broken}"
            );
        }
    }

    #[test]
    fn modern_statuses_follow_the_table_and_legacy_is_always_ok() {
        let code = |code| JsonRpcError {
            code,
            message: String::new(),
            data: None,
        };
        for (code_value, status) in [
            (-32020, StatusCode::BAD_REQUEST),
            (-32022, StatusCode::BAD_REQUEST),
            (-32700, StatusCode::BAD_REQUEST),
            (-32600, StatusCode::BAD_REQUEST),
            (-32601, StatusCode::NOT_FOUND),
            (-32000, StatusCode::TOO_MANY_REQUESTS),
            (-32602, StatusCode::OK),
            (-32603, StatusCode::OK),
            (-32002, StatusCode::OK),
        ] {
            assert_eq!(status_for(&code(code_value), true), status, "{code_value}");
            assert_eq!(status_for(&code(code_value), false), StatusCode::OK, "{code_value}");
        }
    }

    #[test]
    fn the_log_rate_allows_one_line_per_second() {
        let rate = LogRate::new();
        assert!(rate.allow());
        assert!(!rate.allow());
        assert!(!rate.allow());
    }

    #[test]
    fn a_too_many_streams_response_carries_retry_after() {
        let response = error_response(Some(json!(1)), JsonRpcError::too_many_streams(1000), true);
        assert_eq!(response.status(), StatusCode::TOO_MANY_REQUESTS);
        assert_eq!(
            response
                .headers()
                .get(header::RETRY_AFTER)
                .and_then(|v| v.to_str().ok()),
            Some("30")
        );
        let plain = error_response(
            Some(json!(1)),
            JsonRpcError::header_mismatch("Mcp-Name", "missing"),
            true,
        );
        assert!(plain.headers().get(header::RETRY_AFTER).is_none());
    }

    #[test]
    fn origin_absent_is_accepted_and_present_must_be_listed() {
        let allowed = vec!["https://app.example.com".to_string()];
        assert!(origin_allowed(&HeaderMap::new(), &[]));
        assert!(origin_allowed(&HeaderMap::new(), &allowed));
        assert!(origin_allowed(
            &headers(&[("Origin", "https://app.example.com")]),
            &allowed
        ));
        for origin in [
            "https://evil.example.com",
            "https://app.example.com:8443",
            "null",
            "HTTPS://app.example.com",
        ] {
            assert!(!origin_allowed(&headers(&[("Origin", origin)]), &allowed), "{origin}");
        }
        // With nothing configured every present Origin is refused.
        assert!(!origin_allowed(&headers(&[("Origin", "https://app.example.com")]), &[]));
        // More than one Origin header is refused even when the first one is allowed.
        let mut two = headers(&[("Origin", "https://app.example.com")]);
        two.append(header::ORIGIN, HeaderValue::from_static("https://evil.example.com"));
        assert!(!origin_allowed(&two, &allowed));
        let mut same = headers(&[("Origin", "https://app.example.com")]);
        same.append(header::ORIGIN, HeaderValue::from_static("https://app.example.com"));
        assert!(!origin_allowed(&same, &allowed));
        assert_eq!(origin_refusal().status(), StatusCode::FORBIDDEN);
    }

    /// No tool declares an `x-mcp-header` parameter, so no `Mcp-Param-*` header is ever checked.
    #[test]
    fn no_tool_schema_declares_an_mcp_header_parameter() {
        for tools in [
            crate::tools::get_all_tool_definitions(),
            crate::tools::entry::search_mode_listing(),
        ] {
            for tool in tools {
                assert!(!tool.input_schema.to_string().contains("x-mcp-header"), "{}", tool.name);
            }
        }
    }
}
