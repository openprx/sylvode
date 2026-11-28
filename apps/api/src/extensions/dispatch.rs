//! A host call, dispatched through the API router (contract C1 §4.2).
//!
//! The guest names an operation of the catalogue and its arguments; the host checks the call
//! against the principal, builds an ordinary HTTP request with no credentials and sends it through
//! the router in this process. The handler therefore sees the invoking user and applies the user's
//! own role, policies and field rules; the catalogue guard and the authentication middleware
//! re-check the call on the way in.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;
use std::net::{Ipv4Addr, SocketAddr};
use std::sync::{Arc, OnceLock};
use std::time::{Duration, Instant};

use axum::{
    Router,
    body::{Body, to_bytes},
    extract::ConnectInfo,
    http::{Request, header},
};
use platform::app::AppState;
use serde::Deserialize;
use serde_json::{Map, Value};
use sha2::{Digest, Sha256};
use tower::ServiceExt;
use uuid::Uuid;

use super::InternalCall;
use super::host::limits::CALL_BODY_MAX;
use super::install::live::{self, LoadError};
use super::resource::{canonical_uuid, primary_param, resolve_resource};
use crate::catalogue::{
    Entry, Exposure, Method, Principal, Refusal, RefusalCode, Risk, allowed_pre, allowed_target, by_op,
};

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
    /// The invocation must stop: the installation was disabled or revoked, its approver is gone, or
    /// the invoking user's session ended. The guest does not get to handle this one.
    Ended(RefusalCode),
}

impl CallError {
    /// The code a log or a test can compare.
    #[must_use]
    pub fn code(&self) -> &str {
        match self {
            Self::Denied(code) | Self::Invalid(code) | Self::Conflict(code) | Self::Limit(code) => code,
            Self::NotFound => "not_found",
            Self::Unavailable => "unavailable",
            Self::Ended(code) => code.as_str(),
        }
    }
}

impl From<Refusal> for CallError {
    fn from(refusal: Refusal) -> Self {
        Self::Denied(refusal.code.as_str().to_string())
    }
}

/// How a call finds out that its installation is still what it was when the invocation began.
#[derive(Clone, Copy, Debug)]
pub struct Live {
    /// The approving administrator of a non-interactive call (an installation principal); `None`
    /// for a call made on behalf of the invoking user.
    pub approver: Option<Uuid>,
}

/// What a host call needs to know about the invocation it belongs to.
#[derive(Clone)]
pub struct CallEnv {
    pub state: AppState,
    /// A plugin principal, with the grants of this invocation when it began.
    pub principal: Arc<Principal>,
    /// When set, every call loads the installation, its grants, the invoking user's session and (for
    /// an installation principal) the approver again, and uses what it finds instead of
    /// [`Self::principal`]. Production invocations always set it.
    pub live: Option<Live>,
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

/// The operations whose request body has an `idempotency_key`, and whether a repeat of the same
/// call within one invocation is the same call.
///
/// The host never forwards a guest's key (the handlers look keys up workspace-wide). It derives its
/// own, `ext:` and 32 hex characters of SHA-256 over the invocation, the installation, the operation,
/// the target and one of:
///
/// * a token the guest put in the body's `idempotency_key` (1-64 characters of `[A-Za-z0-9_.:-]`):
///   a retry with the same token is the same call, and two creates that must be distinct carry
///   distinct tokens;
/// * for a create without a token, the digest of the cleaned body: a guest that retries a create
///   after a timeout sends the same body and gets the record the first attempt stored;
/// * for an update without a token, the call index: updates are repeatable by nature and a guest may
///   set a value, change it and set it again, so only a token makes two updates one.
const IDEMPOTENT_OPS: [(&str, bool); 2] = [("forms.records.create", true), ("forms.records.update", false)];

/// The risk of the operation `op` if it is one plugins may call, else `None`.
#[must_use]
pub fn plugin_risk(op: &str) -> Option<Risk> {
    by_op(op)
        .filter(|entry| entry.exposure.contains(Exposure::PLUGIN))
        .map(|entry| entry.risk)
}

/// One audited host call: a write or destructive operation, as the audit table of migration 0151
/// stores it. Holds no input and no output, only the ids of the path.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CallAudit {
    pub call_index: u32,
    pub capability: &'static str,
    pub op: &'static str,
    pub method: &'static str,
    pub risk: &'static str,
    /// Path parameter name to canonical UUID, for the parameters that are ids.
    pub target_ids: Value,
    pub project_id: Option<Uuid>,
    /// `ok`, `denied`, `not_found`, `invalid`, `conflict`, `limit` or `unavailable`.
    pub outcome: &'static str,
    pub error_code: Option<String>,
    pub duration_ms: u64,
}

/// What a call learns on its way, for the audit row.
#[derive(Default)]
struct Trace {
    entry: Option<&'static Entry>,
    target_ids: Map<String, Value>,
    project_id: Option<Uuid>,
}

impl Trace {
    fn finish(self, call_index: u32, started: Instant, result: &Result<Value, CallError>) -> Option<CallAudit> {
        let entry = self
            .entry
            .filter(|entry| matches!(entry.risk, Risk::Write | Risk::Destructive))?;
        let (outcome, error_code) = match result {
            Ok(_) => ("ok", None),
            Err(error) => (
                match error {
                    CallError::Denied(_) | CallError::Ended(_) => "denied",
                    CallError::NotFound => "not_found",
                    CallError::Invalid(_) => "invalid",
                    CallError::Conflict(_) => "conflict",
                    CallError::Limit(_) => "limit",
                    CallError::Unavailable => "unavailable",
                },
                Some(error.code().to_string()),
            ),
        };
        Some(CallAudit {
            call_index,
            capability: entry.capability(),
            op: entry.op,
            method: entry.method.as_str(),
            risk: entry.risk.as_str(),
            target_ids: Value::Object(self.target_ids),
            project_id: self.project_id,
            outcome,
            error_code,
            duration_ms: u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX),
        })
    }
}

/// The result of [`call`] and, for a write or destructive operation that got as far as its catalogue
/// entry, the audit row of the call.
pub struct Called {
    pub result: Result<Value, CallError>,
    pub audit: Option<CallAudit>,
}

/// Runs the operation `op` for the invocation `env`.
///
/// The order is fixed so that a guest learns nothing it is not granted: the operation must exist and
/// be exposed to plugins; the arguments must parse; the id must be requested and granted
/// ([`allowed_pre`]); only then are the path parameters checked and the target resolved, and an
/// unresolvable target is refused like one outside the workspace ([`allowed_target`]).
///
/// The failure is a [`CallError`]; see its variants.
pub async fn call(env: &CallEnv, call_index: u32, op: &str, request: &str, timeout: Duration) -> Called {
    let started = Instant::now();
    let mut trace = Trace::default();
    let result = run(env, call_index, op, request, timeout, &mut trace).await;
    Called {
        audit: trace.finish(call_index, started, &result),
        result,
    }
}

async fn run(
    env: &CallEnv,
    call_index: u32,
    op: &str,
    request: &str,
    timeout: Duration,
    trace: &mut Trace,
) -> Result<Value, CallError> {
    let entry = by_op(op)
        .filter(|entry| entry.exposure.contains(Exposure::PLUGIN))
        .ok_or_else(|| CallError::Denied("capability_not_exposed".to_string()))?;
    trace.entry = Some(entry);
    if request.len() > CALL_BODY_MAX {
        return Err(CallError::Limit("payload_too_large".to_string()));
    }
    let request: CallRequest =
        serde_json::from_str(request).map_err(|_| CallError::Invalid("bad_request".to_string()))?;
    let principal = fresh_principal(env).await?;
    allowed_pre(&principal, entry)?;
    let params = path_params(entry, &request.path)?;
    trace.target_ids = params
        .iter()
        .filter(|(_, value)| canonical_uuid(value).is_some())
        .map(|(name, value)| (name.clone(), Value::String(value.clone())))
        .collect();
    let query = query_string(&request.query)?;
    let resource = resolve_resource(
        &env.state.db,
        entry,
        &principal,
        &params,
        query.strip_prefix('?'),
        request.body.as_ref(),
    )
    .await
    .map_err(|_| CallError::Unavailable)?;
    let Some(resource) = resource else {
        return Err(CallError::Denied("capability_scope_mismatch".to_string()));
    };
    trace.project_id = resource.project_id;
    allowed_target(&principal, entry, &resource)?;
    let internal = InternalCall {
        op: entry.op,
        principal,
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
    let Ok(Ok(Ok(response))) = tokio::time::timeout(timeout, &mut task.0).await else {
        return Err(CallError::Unavailable);
    };
    let bytes = to_bytes(response.into_body(), CALL_BODY_MAX + 4096)
        .await
        .map_err(|_| CallError::Limit("payload_too_large".to_string()))?;
    map_envelope(&bytes)
}

/// The principal for one call. A live call reads the installation, its grants and the invoking
/// user's session from the database again, so a disable, a revoke, a changed grant, a logout or a
/// lost approver takes effect at the next call. Anything that is no longer as it was ends the
/// invocation ([`CallError::Ended`]).
async fn fresh_principal(env: &CallEnv) -> Result<Arc<Principal>, CallError> {
    let Some(binding) = env.live else {
        return Ok(Arc::clone(&env.principal));
    };
    let db = &env.state.db;
    let unavailable = |_: LoadError| CallError::Unavailable;
    let loaded = match live::load(db, env.installation_id).await {
        Ok(loaded) => loaded,
        Err(LoadError::Refused(code)) => return Err(CallError::Ended(code)),
        Err(error) => return Err(unavailable(error)),
    };
    if loaded.view.generation != env.generation {
        return Err(CallError::Ended(RefusalCode::InstallationNotActive));
    }
    match live::actor_session(db, env.actor).await.map_err(unavailable)? {
        Some(version) if version == env.session_version => {}
        _ => return Err(CallError::Ended(RefusalCode::PluginSessionEnded)),
    }
    if let Some(approver) = binding.approver
        && !live::approver_is_admin(db, loaded.view.workspace_id, approver)
            .await
            .map_err(unavailable)?
    {
        live::disable_for_lost_approver(db, env.installation_id)
            .await
            .map_err(unavailable)?;
        return Err(CallError::Ended(RefusalCode::InstallationApproverLost));
    }
    Ok(Arc::new(match &*env.principal {
        Principal::Plugin { invoker, .. } => Principal::Plugin {
            invoker: invoker.clone(),
            installation: loaded.view,
        },
        Principal::Installation { approver_user_id, .. } => Principal::Installation {
            installation: loaded.view,
            approver_user_id: *approver_user_id,
        },
        other => other.clone(),
    }))
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
            let _ = write!(out, "%{byte:02X}");
        }
    }
}

/// The query of a call as it goes on the request URI: `?k=v&...`, or empty.
fn query_string(query: &BTreeMap<String, String>) -> Result<String, CallError> {
    let mut out = String::new();
    for (index, (key, value)) in query.iter().enumerate() {
        if key.is_empty() || key.len() > 64 || value.len() > 1024 {
            return Err(CallError::Invalid("bad_request".to_string()));
        }
        out.push(if index == 0 { '?' } else { '&' });
        encode(key, &mut out);
        out.push('=');
        encode(value, &mut out);
    }
    Ok(out)
}

/// The idempotency key the host sends for a call; see [`IDEMPOTENT_OPS`].
fn host_key(
    internal: &InternalCall,
    entry: &Entry,
    params: &[(String, String)],
    call_index: u32,
    body: &Map<String, Value>,
    token: Option<&str>,
    by_body: bool,
) -> String {
    let mut hasher = Sha256::new();
    let mut field = |label: &str, value: &[u8]| {
        hasher.update(label.as_bytes());
        hasher.update([0]);
        hasher.update(value);
        hasher.update([0]);
    };
    field("invocation", internal.request_id.as_bytes());
    field("installation", internal.installation_id.as_bytes());
    field("op", entry.op.as_bytes());
    for (name, value) in params {
        field(name, value.as_bytes());
    }
    match token {
        Some(token) => field("token", token.as_bytes()),
        None if by_body => field("body", &serde_json::to_vec(body).unwrap_or_default()),
        None => field("call", &call_index.to_be_bytes()),
    }
    let digest = hasher.finalize();
    format!("ext:{}", hex::encode(digest.get(..16).unwrap_or_default()))
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
    uri.push_str(&query_string(&request.query)?);
    let writes = !matches!(entry.method, Method::Get | Method::Delete);
    let body = match (&request.body, writes) {
        (Some(Value::Object(object)), true) => {
            let mut cleaned: Map<String, Value> = object
                .iter()
                .filter(|(key, _)| !STRIPPED_BODY_KEYS.contains(&key.as_str()))
                .map(|(key, value)| (key.clone(), value.clone()))
                .collect();
            if let Some((_, by_body)) = IDEMPOTENT_OPS.iter().find(|(op, _)| *op == entry.op) {
                let token = match object.get("idempotency_key") {
                    None | Some(Value::Null) => None,
                    Some(Value::String(token)) if plain_value(token) && token.len() <= 64 => Some(token.as_str()),
                    Some(_) => return Err(bad()),
                };
                let key = host_key(&internal, entry, params, call_index, &cleaned, token, *by_body);
                cleaned.insert("idempotency_key".to_string(), Value::String(key));
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
        // The guest's key and provenance are gone; the host's key is `ext:` and 32 hex characters.
        let key = body["idempotency_key"].as_str().expect("a host key");
        assert!(key.starts_with("ext:") && key.len() == 36 && key[4..].bytes().all(|b| b.is_ascii_hexdigit()));
        assert_eq!(body["values"], json!({"a": 1}));
        assert_eq!(body.as_object().map(serde_json::Map::len), Some(2));
        let names: Vec<&str> = parts.headers.keys().map(axum::http::HeaderName::as_str).collect();
        assert_eq!(names, vec!["content-type"]);
        assert!(parts.extensions.get::<InternalCall>().is_some());
        let peer = parts.extensions.get::<ConnectInfo<SocketAddr>>().expect("peer");
        assert_eq!(peer.0, SocketAddr::from((Ipv4Addr::UNSPECIFIED, 0)));
    }

    /// The key a call carries.
    async fn key_of(op: &'static str, call_index: u32, body: serde_json::Value, param: (&str, &str)) -> String {
        let entry = by_op(op).expect("entry");
        let params = vec![(param.0.to_string(), param.1.to_string())];
        let request: CallRequest = serde_json::from_value(json!({"body": body})).expect("request");
        let http = build_request(internal(op), call_index, entry, &request, &params).expect("built");
        let bytes = to_bytes(http.into_body(), 1 << 20).await.expect("body");
        let body: Value = serde_json::from_slice(&bytes).expect("json");
        body["idempotency_key"].as_str().expect("a key").to_string()
    }

    /// A create the guest retries after a timeout is the same call whatever the call index; a create
    /// with other values, or another token, is not. An update is the same call only with a token.
    #[tokio::test]
    async fn the_host_key_makes_a_retried_create_one_call() {
        let form = "0b1d2f3e-4a5b-4c6d-8e7f-9a0b1c2d3e4f";
        let create = |n, body| key_of("forms.records.create", n, body, ("form_id", form));
        let first = create(1, json!({"values": {"a": 1}})).await;
        assert_eq!(
            first,
            create(5, json!({"values": {"a": 1}})).await,
            "a retry is the same call"
        );
        assert_ne!(
            first,
            create(2, json!({"values": {"a": 2}})).await,
            "other values are another call"
        );
        let token_a = create(1, json!({"values": {"a": 1}, "idempotency_key": "one"})).await;
        assert_eq!(
            token_a,
            create(9, json!({"values": {"a": 1}, "idempotency_key": "one"})).await
        );
        assert_ne!(
            token_a,
            create(1, json!({"values": {"a": 1}, "idempotency_key": "two"})).await
        );
        assert_ne!(token_a, first, "a token replaces the body in the derivation");

        let record = "1c2d3e4f-5a6b-4c7d-8e9f-0a1b2c3d4e5f";
        let update = |n, body| key_of("forms.records.update", n, body, ("record_id", record));
        let first = update(1, json!({"values": {"a": 1}})).await;
        assert_ne!(
            first,
            update(2, json!({"values": {"a": 1}})).await,
            "a repeated update is a new call"
        );
        let token = json!({"values": {"a": 1}, "idempotency_key": "set-a"});
        assert_eq!(update(1, token.clone()).await, update(7, token).await);
    }

    /// A key that is not a plain short token is refused rather than forwarded.
    #[test]
    fn a_guest_key_that_is_not_a_plain_token_is_refused() {
        let form = "0b1d2f3e-4a5b-4c6d-8e7f-9a0b1c2d3e4f";
        let entry = by_op("forms.records.create").expect("entry");
        let params = vec![("form_id".to_string(), form.to_string())];
        for bad in [json!("a b"), json!("x".repeat(65)), json!(7)] {
            let request: CallRequest =
                serde_json::from_value(json!({"body": {"values": {}, "idempotency_key": bad}})).expect("request");
            assert!(build_request(internal("forms.records.create"), 1, entry, &request, &params).is_err());
        }
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
