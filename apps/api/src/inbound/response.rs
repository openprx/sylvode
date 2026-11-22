//! Answers of the inbound endpoint.
//!
//! Real HTTP statuses (the external products that post here
//! check them) with the API's `{code, message, data}` body, where `code` equals the status and
//! `message` is the stable machine name of the answer.

use axum::{
    Json,
    http::{HeaderValue, StatusCode, header::RETRY_AFTER},
    response::{IntoResponse, Response},
};
use serde_json::{Value, json};

/// A refusal of the inbound endpoint.
#[derive(Debug)]
pub struct InboundError {
    pub status: StatusCode,
    /// Stable machine name, e.g. `invalid_credentials`.
    pub name: &'static str,
    /// Extra fields of `data`, e.g. the field path of an invalid envelope.
    pub data: Value,
    pub retry_after_secs: Option<u64>,
}

impl InboundError {
    pub const fn new(status: StatusCode, name: &'static str) -> Self {
        Self {
            status,
            name,
            data: Value::Null,
            retry_after_secs: None,
        }
    }

    #[must_use]
    pub fn with_data(mut self, data: Value) -> Self {
        self.data = data;
        self
    }

    pub const fn disabled() -> Self {
        Self::new(StatusCode::SERVICE_UNAVAILABLE, "inbound_disabled")
    }

    pub const fn store_unavailable() -> Self {
        Self::new(StatusCode::SERVICE_UNAVAILABLE, "service_unavailable")
    }

    /// Unknown, disabled and rotated tokens all answer this, so the endpoint is not an oracle.
    pub const fn invalid_credentials() -> Self {
        Self::new(StatusCode::UNAUTHORIZED, "invalid_credentials")
    }

    pub const fn invalid_signature() -> Self {
        Self::new(StatusCode::UNAUTHORIZED, "invalid_signature")
    }

    pub const fn source_suspended() -> Self {
        Self::new(StatusCode::FORBIDDEN, "source_suspended")
    }

    /// The source is valid but the request came from an address outside its allow-list.
    pub const fn ip_not_allowed() -> Self {
        Self::new(StatusCode::FORBIDDEN, "ip_not_allowed")
    }

    pub const fn rate_limited(retry_after_secs: u64) -> Self {
        Self {
            status: StatusCode::TOO_MANY_REQUESTS,
            name: "rate_limited",
            data: Value::Null,
            retry_after_secs: Some(retry_after_secs),
        }
    }

    pub fn invalid_envelope(path: &str, reason: &str) -> Self {
        Self::new(StatusCode::BAD_REQUEST, "invalid_envelope").with_data(json!({ "field": path, "reason": reason }))
    }

    pub const fn event_id_conflict() -> Self {
        Self::new(StatusCode::CONFLICT, "event_id_conflict")
    }
}

impl IntoResponse for InboundError {
    fn into_response(self) -> Response {
        let body = json!({
            "code": self.status.as_u16(),
            "message": self.name,
            "data": self.data,
        });
        let mut response = (self.status, Json(body)).into_response();
        if let Some(secs) = self.retry_after_secs
            && let Ok(value) = HeaderValue::from_str(&secs.to_string())
        {
            response.headers_mut().insert(RETRY_AFTER, value);
        }
        response
    }
}

/// A success answer: 202 for a stored event, 200 for a duplicate.
pub fn answer(status: StatusCode, name: &'static str, inbound_event_id: uuid::Uuid) -> Response {
    let body = json!({
        "code": status.as_u16(),
        "message": name,
        "data": { "status": name, "inbound_event_id": inbound_event_id },
    });
    (status, Json(body)).into_response()
}
