//! What kind of failure an API error string describes.
//!
//! The client reports every failure as text. The two stable shapes it produces for an answer of
//! the API are `API error {code} from {path}: ...` (the envelope code of an HTTP 200 answer) and
//! `HTTP {status} from {path}: ...` (a real HTTP status from something in front of the API).
//! A caller that has to tell "not there" from "not allowed" from "broken" reads the status out of
//! those shapes here instead of matching text of its own.

/// The kind of failure an API error string carries.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ApiErrorClass {
    NotFound,
    Forbidden,
    Unauthorized,
    /// Any other status, and text that is no recognised API failure.
    Other,
}

/// The markers of every failure the client reports for a request it made, answer or not.
const BACKEND_MARKERS: [&str; 6] = [
    "API error ",
    "HTTP ",
    "Malformed response from ",
    "Request failed:",
    "Failed to read response body from ",
    "Failed to deserialize response from ",
];

/// The status that follows the first `API error ` or `HTTP ` marker in `error`.
fn status_of(error: &str) -> Option<u16> {
    ["API error ", "HTTP "]
        .iter()
        .filter_map(|marker| error.find(marker).map(|at| (at, *marker)))
        .min_by_key(|(at, _)| *at)
        .and_then(|(at, marker)| {
            let digits: String = error[at + marker.len()..]
                .chars()
                .take_while(char::is_ascii_digit)
                .collect();
            digits.parse().ok()
        })
}

/// Classifies an error string produced by the client. A message that wraps one (`Failed to load
/// ...: API error 404 from ...`) is classified by the failure inside it.
pub fn classify_api_error(error: &str) -> ApiErrorClass {
    match status_of(error) {
        Some(404) => ApiErrorClass::NotFound,
        Some(403) => ApiErrorClass::Forbidden,
        Some(401) => ApiErrorClass::Unauthorized,
        _ => ApiErrorClass::Other,
    }
}

/// Whether `error` reports a request the client made and could not complete, as opposed to a
/// decision made by this server.
pub fn is_backend_failure(error: &str) -> bool {
    BACKEND_MARKERS.iter().any(|marker| error.contains(marker))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::client::{check_response_envelope, rejected_request_error};
    use reqwest::StatusCode;
    use serde_json::json;

    fn envelope(code: i64, message: &str) -> String {
        check_response_envelope(&json!({"code": code, "message": message, "data": null}), "/api/v1/x")
            .err()
            .unwrap_or_default()
    }

    #[test]
    fn envelope_codes_are_classified() {
        assert_eq!(classify_api_error(&envelope(404, "not found")), ApiErrorClass::NotFound);
        assert_eq!(classify_api_error(&envelope(403, "denied")), ApiErrorClass::Forbidden);
        assert_eq!(
            classify_api_error(&envelope(401, "bad token")),
            ApiErrorClass::Unauthorized
        );
        assert_eq!(classify_api_error(&envelope(500, "boom")), ApiErrorClass::Other);
        assert_eq!(classify_api_error(&envelope(400, "bad")), ApiErrorClass::Other);
    }

    #[test]
    fn the_scoped_bot_refusal_is_forbidden() {
        let error = envelope(403, "this bot may not do this in this project");
        assert_eq!(classify_api_error(&error), ApiErrorClass::Forbidden);
    }

    #[test]
    fn http_statuses_are_classified() {
        let status = |status| rejected_request_error(status, "/api/v1/x", "body");
        assert_eq!(
            classify_api_error(&status(StatusCode::NOT_FOUND)),
            ApiErrorClass::NotFound
        );
        assert_eq!(
            classify_api_error(&status(StatusCode::FORBIDDEN)),
            ApiErrorClass::Forbidden
        );
        assert_eq!(
            classify_api_error(&status(StatusCode::UNAUTHORIZED)),
            ApiErrorClass::Unauthorized
        );
        assert_eq!(
            classify_api_error(&status(StatusCode::BAD_GATEWAY)),
            ApiErrorClass::Other
        );
    }

    #[test]
    fn a_wrapped_failure_is_classified_by_the_failure_inside() {
        let wrapped = format!("Failed to read issue resource: {}", envelope(404, "no such issue"));
        assert_eq!(classify_api_error(&wrapped), ApiErrorClass::NotFound);
    }

    #[test]
    fn text_that_is_no_api_failure_is_other() {
        for text in [
            "",
            "Tool 'forms.get' is disabled by project agent policy for project p",
            "Invalid params: project_id 'x' is not a canonical UUID",
            "Request failed: connection refused",
            "Malformed response from /api/v1/x: expected a {code, message, data} envelope, got an array",
        ] {
            assert_eq!(classify_api_error(text), ApiErrorClass::Other, "{text}");
        }
    }

    #[test]
    fn backend_failures_are_told_from_server_decisions() {
        assert!(is_backend_failure(&envelope(500, "boom")));
        assert!(is_backend_failure(&rejected_request_error(
            StatusCode::BAD_GATEWAY,
            "/p",
            "x"
        )));
        assert!(is_backend_failure("Request failed: connection refused"));
        assert!(is_backend_failure("Failed to read response body from /p: eof"));
        assert!(is_backend_failure("Failed to deserialize response from /p: x"));
        assert!(!is_backend_failure(
            "Tool 'forms.get' is disabled by project agent policy for project p"
        ));
        assert!(!is_backend_failure(
            "Refusing 'forms.get': the agent-policy response carries no policy object"
        ));
    }
}
