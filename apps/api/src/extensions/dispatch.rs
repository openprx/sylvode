//! A host call, dispatched through the API router (contract C1 §4.2).
//!
//! The guest names an operation of the catalogue and its arguments; the host checks the call
//! against the principal, builds an ordinary HTTP request with no credentials and sends it through
//! the router in this process. The handler therefore sees the invoking user and applies the user's
//! own role, policies and field rules; the catalogue guard and the authentication middleware
//! re-check the call on the way in.

use std::collections::{BTreeMap, BTreeSet};
use std::net::{Ipv4Addr, SocketAddr};
use std::sync::{Arc, OnceLock};
use std::time::Duration;

use axum::{
    Router,
    body::{Body, to_bytes},
    extract::ConnectInfo,
    http::{Request, header},
};
use platform::app::AppState;
use serde::Deserialize;
use serde_json::{Map, Value};
use tower::ServiceExt;
use uuid::Uuid;

use super::InternalCall;
use super::host::limits::CALL_BODY_MAX;
use super::resource::{canonical_uuid, primary_param, resolve_resource};
use crate::catalogue::{Entry, Exposure, Method, Principal, Refusal, allowed_pre, allowed_target, by_op};

static ROUTER: OnceLock<Router> = OnceLock::new();

/// Keeps the router host calls are sent through. Called once by `main`, right after the router is
/// built; before that every host call answers [`CallError::Unavailable`].
pub fn install(router: Router) -> bool {
    ROUTER.set(router).is_ok()
}

/// The typed failure of a host call; mirrors `call-error` of the WIT package and never carries an
/// input value or a handler message.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CallError {
    /// 401 or 403: the refusal code, or `forbidden`.
    Denied(String),
    NotFound,
    /// 400 or 422: the error code, or `bad_request`.
    Invalid(String),
    /// 409: the error code, or `conflict`.
    Conflict(String),
    /// 413, 429 or a host limit.
    Limit(String),
    /// 5xx, a timeout, or the router is not installed.
    Unavailable,
}

impl CallError {
    /// The code a log or a test can compare.
    #[must_use]
    pub fn code(&self) -> &str {
        match self {
            Self::Denied(code) | Self::Invalid(code) | Self::Conflict(code) | Self::Limit(code) => code,
            Self::NotFound => "not_found",
            Self::Unavailable => "unavailable",
        }
    }
}

impl From<Refusal> for CallError {
    fn from(refusal: Refusal) -> Self {
        Self::Denied(refusal.code.as_str().to_string())
    }
}

/// What a host call needs to know about the invocation it belongs to.
#[derive(Clone)]
pub struct CallEnv {
    pub state: AppState,
    /// A plugin principal, with the grants of this invocation.
    pub principal: Arc<Principal>,
    pub actor: Uuid,
    pub session_version: i64,
    pub installation_id: Uuid,
    pub generation: i32,
    pub request_id: Uuid,
}

/// The arguments of `catalogue.call`; `records` builds the same value.
#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CallRequest {
    #[serde(default)]
    pub path: BTreeMap<String, String>,
    #[serde(default)]
    pub query: BTreeMap<String, String>,
    #[serde(default)]
    pub body: Option<Value>,
}

/// The body keys a guest may not set: they carry provenance the host and the handlers own.
const STRIPPED_BODY_KEYS: [&str; 3] = ["source", "metadata", "idempotency_key"];

/// The operations whose request body has an `idempotency_key`. The host sets
/// `ext:<invocation>:<call index>` on them, so a create the guest retries after a timeout is not
/// stored twice (contract C1 §4.2, N-3).
const IDEMPOTENT_OPS: [&str; 2] = ["forms.records.create", "forms.records.update"];

/// Runs the operation `op` for the invocation `env`.
///
/// The order is fixed so that a guest learns nothing it is not granted: the operation must exist and
/// be exposed to plugins; the arguments must parse; the id must be requested and granted
/// ([`allowed_pre`]); only then are the path parameters checked and the target resolved, and an
/// unresolvable target is refused like one outside the workspace ([`allowed_target`]).
///
/// # Errors
/// A [`CallError`]; see its variants.
pub async fn call(
    env: &CallEnv,
    call_index: u32,
    op: &str,
    request: &str,
    timeout: Duration,
) -> Result<Value, CallError> {
    let entry = by_op(op)
        .filter(|entry| entry.exposure.contains(Exposure::PLUGIN))
        .ok_or_else(|| CallError::Denied("capability_not_exposed".to_string()))?;
    if request.len() > CALL_BODY_MAX {
        return Err(CallError::Limit("payload_too_large".to_string()));
    }
    let request: CallRequest =
        serde_json::from_str(request).map_err(|_| CallError::Invalid("bad_request".to_string()))?;
    allowed_pre(&env.principal, entry)?;
    let params = path_params(entry, &request.path)?;
    let resource = resolve_resource(&env.state.db, entry, &params)
        .await
        .map_err(|_| CallError::Unavailable)?;
    let Some(resource) = resource else {
        return Err(CallError::Denied("capability_scope_mismatch".to_string()));
    };
    allowed_target(&env.principal, entry, &resource)?;
    let internal = InternalCall {
        op: entry.op,
        principal: Arc::clone(&env.principal),
        actor: env.actor,
        session_version: env.session_version,
        installation_id: env.installation_id,
        generation: env.generation,
        depth: 1,
        request_id: env.request_id,
    };
    let http = build_request(internal, call_index, entry, &request, &params)?;
    let router = ROUTER.get().ok_or(CallError::Unavailable)?;
    // The router runs on its own task: the host function is polled on the guest's fiber stack, which
    // is small, and the axum stack, the database driver and the handlers must not run on it.
    // Dropping this future (timeout, client gone, shutdown) aborts the spawned call as well.
    let mut task = AbortOnDrop(tokio::spawn(router.clone().oneshot(http)));
    let response = match tokio::time::timeout(timeout, &mut task.0).await {
        Ok(Ok(Ok(response))) => response,
        _ => return Err(CallError::Unavailable),
    };
    let bytes = to_bytes(response.into_body(), CALL_BODY_MAX + 4096)
        .await
        .map_err(|_| CallError::Limit("payload_too_large".to_string()))?;
    map_envelope(&bytes)
}

/// Aborts the wrapped task when dropped, so a router call never outlives its invocation.
struct AbortOnDrop<T>(tokio::task::JoinHandle<T>);

impl<T> Drop for AbortOnDrop<T> {
    fn drop(&mut self) {
        self.0.abort();
    }
}

/// The template's parameter names, in order.
fn template_names(path: &str) -> Vec<&str> {
    path.split('/')
        .filter_map(|segment| segment.strip_prefix('{').and_then(|rest| rest.strip_suffix('}')))
        .collect()
}

fn plain_value(value: &str) -> bool {
    (1..=128).contains(&value.len())
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'.' | b':' | b'-'))
}

/// Each parameter of the template present, none extra; the parameter a `Find` resolves in canonical
/// UUID form, the others plain.
fn path_params(entry: &Entry, given: &BTreeMap<String, String>) -> Result<Vec<(String, String)>, CallError> {
    let bad = || CallError::Invalid("bad_request".to_string());
    let names = template_names(entry.path);
    let expected: BTreeSet<&str> = names.iter().copied().collect();
    if given.len() != expected.len() || !given.keys().all(|key| expected.contains(key.as_str())) {
        return Err(bad());
    }
    let primary = primary_param(entry);
    let mut params = Vec::with_capacity(names.len());
    for name in names {
        let value = given.get(name).ok_or_else(bad)?;
        let valid = if Some(name) == primary {
            canonical_uuid(value).is_some()
        } else {
            plain_value(value)
        };
        if !valid {
            return Err(bad());
        }
        params.push((name.to_string(), value.clone()));
    }
    Ok(params)
}

fn encode(value: &str, out: &mut String) {
    for byte in value.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.' | b'_' | b'~') {
            out.push(char::from(byte));
        } else {
            out.push_str(&format!("%{byte:02X}"));
        }
    }
}

fn build_request(
    internal: InternalCall,
    call_index: u32,
    entry: &'static Entry,
    request: &CallRequest,
    params: &[(String, String)],
) -> Result<Request<Body>, CallError> {
    let bad = || CallError::Invalid("bad_request".to_string());
    let mut uri = String::new();
    for segment in entry.path.split('/').filter(|segment| !segment.is_empty()) {
        uri.push('/');
        match segment.strip_prefix('{').and_then(|rest| rest.strip_suffix('}')) {
            Some(name) => {
                let value = params.iter().find(|(key, _)| key == name).ok_or_else(bad)?;
                uri.push_str(&value.1);
            }
            None => uri.push_str(segment),
        }
    }
    for (index, (key, value)) in request.query.iter().enumerate() {
        if key.is_empty() || key.len() > 64 || value.len() > 1024 {
            return Err(bad());
        }
        uri.push(if index == 0 { '?' } else { '&' });
        encode(key, &mut uri);
        uri.push('=');
        encode(value, &mut uri);
    }
    let writes = !matches!(entry.method, Method::Get | Method::Delete);
    let body = match (&request.body, writes) {
        (Some(Value::Object(object)), true) => {
            let mut cleaned: Map<String, Value> = object
                .iter()
                .filter(|(key, _)| !STRIPPED_BODY_KEYS.contains(&key.as_str()))
                .map(|(key, value)| (key.clone(), value.clone()))
                .collect();
            if IDEMPOTENT_OPS.contains(&entry.op) {
                cleaned.insert(
                    "idempotency_key".to_string(),
                    Value::String(format!("ext:{}:{call_index}", internal.request_id)),
                );
            }
            Some(serde_json::to_vec(&Value::Object(cleaned)).map_err(|_| bad())?)
        }
        (None | Some(Value::Null), _) => None,
        _ => return Err(bad()),
    };
    if body.as_ref().is_some_and(|bytes| bytes.len() > CALL_BODY_MAX) {
        return Err(CallError::Limit("payload_too_large".to_string()));
    }
    let mut builder = Request::builder().method(entry.method.as_str()).uri(uri);
    if body.is_some() {
        builder = builder.header(header::CONTENT_TYPE, "application/json");
    }
    let mut http = builder.body(Body::from(body.unwrap_or_default())).map_err(|_| bad())?;
    http.extensions_mut().insert(internal);
    http.extensions_mut()
        .insert(ConnectInfo(SocketAddr::from((Ipv4Addr::UNSPECIFIED, 0))));
    Ok(http)
}

/// The envelope of a handler's answer as a result; the handler's `message` is dropped.
fn map_envelope(bytes: &[u8]) -> Result<Value, CallError> {
    let envelope: Value = serde_json::from_slice(bytes).map_err(|_| CallError::Unavailable)?;
    let code = envelope
        .get("code")
        .and_then(Value::as_i64)
        .ok_or(CallError::Unavailable)?;
    let error_code = envelope
        .get("error_code")
        .and_then(Value::as_str)
        .filter(|code| plain_value(code))
        .map(str::to_owned);
    match code {
        0 => {
            let data = envelope.get("data").cloned().unwrap_or(Value::Null);
            let size = serde_json::to_vec(&data).map_err(|_| CallError::Unavailable)?.len();
            if size > CALL_BODY_MAX {
                Err(CallError::Limit("payload_too_large".to_string()))
            } else {
                Ok(data)
            }
        }
        401 | 403 => Err(CallError::Denied(error_code.unwrap_or_else(|| "forbidden".to_string()))),
        404 => Err(CallError::NotFound),
        400 | 422 => Err(CallError::Invalid(
            error_code.unwrap_or_else(|| "bad_request".to_string()),
        )),
        409 => Err(CallError::Conflict(
            error_code.unwrap_or_else(|| "conflict".to_string()),
        )),
        413 | 429 => Err(CallError::Limit(error_code.unwrap_or_else(|| "limit".to_string()))),
        _ => Err(CallError::Unavailable),
    }
}

#[cfg(test)]
#[allow(clippy::expect_used)]
mod tests {
    use std::collections::{BTreeMap, BTreeSet};
    use std::net::{Ipv4Addr, SocketAddr};
    use std::sync::Arc;

    use axum::{body::to_bytes, extract::ConnectInfo};
    use serde_json::{Value, json};
    use uuid::Uuid;

    use super::{CallError, CallRequest, InternalCall, build_request, map_envelope, path_params, template_names};
    use crate::catalogue::{InstallationView, Invoker, Principal, by_op};
    use crate::entities::workspace_member::WorkspaceRole;

    fn given(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
        pairs
            .iter()
            .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
            .collect()
    }

    #[test]
    fn the_template_names_are_read_in_order() {
        assert_eq!(template_names("/api/v1/forms/{form_id}/records"), vec!["form_id"]);
        assert!(template_names("/health").is_empty());
    }

    #[test]
    fn path_parameters_must_match_the_template_exactly() {
        let entry = by_op("forms.records.read").expect("entry");
        let id = "0b1d2f3e-4a5b-4c6d-8e7f-9a0b1c2d3e4f";
        assert!(path_params(entry, &given(&[("record_id", id)])).is_ok());
        for bad in [
            given(&[]),
            given(&[("record_id", id), ("extra", "x")]),
            given(&[("other", id)]),
            given(&[("record_id", "not-a-uuid")]),
            given(&[("record_id", &id.to_uppercase())]),
            given(&[("record_id", &format!("{id}/../x"))]),
        ] {
            assert_eq!(
                path_params(entry, &bad),
                Err(CallError::Invalid("bad_request".to_string())),
                "{bad:?}"
            );
        }
    }

    fn internal(op: &'static str) -> InternalCall {
        InternalCall {
            op,
            principal: Arc::new(Principal::Plugin {
                invoker: Invoker::User {
                    user_id: Uuid::from_u128(1),
                    workspace_id: Uuid::from_u128(2),
                    role: WorkspaceRole::Member,
                },
                installation: InstallationView {
                    installation_id: Uuid::from_u128(3),
                    generation: 1,
                    workspace_id: Uuid::from_u128(2),
                    project_id: None,
                    requested: BTreeSet::new(),
                    grants: Vec::new(),
                },
            }),
            actor: Uuid::from_u128(1),
            session_version: 0,
            installation_id: Uuid::from_u128(3),
            generation: 1,
            depth: 1,
            request_id: Uuid::from_u128(9),
        }
    }

    async fn built(
        op: &'static str,
        request: serde_json::Value,
        params: &[(&str, &str)],
    ) -> (axum::http::request::Parts, Value) {
        let entry = by_op(op).expect("entry");
        let params: Vec<(String, String)> = params
            .iter()
            .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
            .collect();
        let request: CallRequest = serde_json::from_value(request).expect("request");
        let http = build_request(internal(op), 7, entry, &request, &params).expect("built");
        let (parts, body) = http.into_parts();
        let bytes = to_bytes(body, 1 << 20).await.expect("body");
        let body = if bytes.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&bytes).expect("json")
        };
        (parts, body)
    }

    /// The host strips the provenance keys a guest sends, sets its own idempotency key, sends no
    /// credentials, and marks the request as an internal call from an untrusted peer.
    #[tokio::test]
    async fn the_request_carries_no_credentials_and_no_guest_provenance() {
        let id = "0b1d2f3e-4a5b-4c6d-8e7f-9a0b1c2d3e4f";
        let (parts, body) = built(
            "forms.records.update",
            json!({
                "path": {"record_id": id},
                "body": {"values": {"a": 1}, "source": {"type": "user"}, "metadata": {"x": 1}, "idempotency_key": "mine"}
            }),
            &[("record_id", id)],
        )
        .await;
        assert_eq!(parts.method, axum::http::Method::PATCH);
        assert_eq!(parts.uri.path(), format!("/api/v1/form-records/{id}"));
        assert_eq!(
            body,
            json!({"values": {"a": 1}, "idempotency_key": format!("ext:{}:7", Uuid::from_u128(9))})
        );
        let names: Vec<&str> = parts.headers.keys().map(axum::http::HeaderName::as_str).collect();
        assert_eq!(names, vec!["content-type"]);
        assert!(parts.extensions.get::<InternalCall>().is_some());
        let peer = parts.extensions.get::<ConnectInfo<SocketAddr>>().expect("peer");
        assert_eq!(peer.0, SocketAddr::from((Ipv4Addr::UNSPECIFIED, 0)));
    }

    #[tokio::test]
    async fn query_values_are_percent_encoded_and_a_read_has_no_body() {
        let id = "0b1d2f3e-4a5b-4c6d-8e7f-9a0b1c2d3e4f";
        let (parts, body) = built(
            "forms.records.read/list",
            json!({"path": {"form_id": id}, "query": {"filter[status]": "a b&c=d"}}),
            &[("form_id", id)],
        )
        .await;
        assert_eq!(parts.uri.query(), Some("filter%5Bstatus%5D=a%20b%26c%3Dd"));
        assert_eq!(body, Value::Null);
        assert!(parts.headers.is_empty());
    }

    #[test]
    fn a_body_that_is_not_an_object_is_refused() {
        let id = "0b1d2f3e-4a5b-4c6d-8e7f-9a0b1c2d3e4f";
        let entry = by_op("forms.records.update").expect("entry");
        let params = vec![("record_id".to_string(), id.to_string())];
        for bad in [json!([1]), json!("x"), json!(3)] {
            let request: CallRequest = serde_json::from_value(json!({"body": bad})).expect("request");
            assert!(build_request(internal("forms.records.update"), 1, entry, &request, &params).is_err());
        }
    }

    #[test]
    fn the_envelope_maps_to_a_typed_result() {
        let answer = |value: serde_json::Value| map_envelope(value.to_string().as_bytes());
        assert_eq!(
            answer(json!({"code": 0, "message": "ok", "data": {"a": 1}})),
            Ok(json!({"a": 1}))
        );
        assert_eq!(answer(json!({"code": 0, "message": "ok"})), Ok(json!(null)));
        assert_eq!(
            answer(json!({"code": 403, "message": "secret text", "error_code": "scoped_bot"})),
            Err(CallError::Denied("scoped_bot".to_string()))
        );
        assert_eq!(
            answer(json!({"code": 401, "message": "x"})),
            Err(CallError::Denied("forbidden".to_string()))
        );
        assert_eq!(answer(json!({"code": 404, "message": "x"})), Err(CallError::NotFound));
        assert_eq!(
            answer(json!({"code": 422, "message": "x", "error_code": "validation_failed"})),
            Err(CallError::Invalid("validation_failed".to_string()))
        );
        assert_eq!(
            answer(json!({"code": 409, "message": "x"})),
            Err(CallError::Conflict("conflict".to_string()))
        );
        assert_eq!(
            answer(json!({"code": 429, "message": "x", "error_code": "rate"})),
            Err(CallError::Limit("rate".to_string()))
        );
        assert_eq!(
            answer(json!({"code": 500, "message": "x"})),
            Err(CallError::Unavailable)
        );
        assert_eq!(map_envelope(b"<html>"), Err(CallError::Unavailable));
        // An error code with odd characters is not passed on.
        assert_eq!(
            answer(json!({"code": 409, "message": "x", "error_code": "a b<c>"})),
            Err(CallError::Conflict("conflict".to_string()))
        );
    }
}
