//! Delegated (OAuth) access to the MCP server: Sylvode is the authorization server, this server is
//! a protected resource, and Ramflux is the one pre-registered client.
//!
//! What lives here:
//!
//! * the RFC 9728 protected resource metadata document and the `WWW-Authenticate` challenges
//!   (RFC 6750, MCP authorization specification);
//! * the inbound authentication layer shared by every networked route: it tells an `opr_` bot
//!   token or a user token (forwarded verbatim, unchanged) from a delegated `sva_` token (resolved
//!   through the API's internal token-context endpoint before anything else happens);
//! * the token-context client, behind a trait so that tests can replace it;
//! * the scope gate every delegated `tools/call` passes (see [`crate::oauth_scopes`]).
//!
//! Deviation D2 of the authorization contract, stated here because it is visible in this file:
//! the MCP server forwards the caller's MCP-audience token to the Sylvode API together with an
//! internal forwarding credential, and the API accepts an MCP-audience token only with that
//! credential. The MCP specification asks a server not to pass a received token through; this
//! server is a stateless facade over its own API, holds no signing key and no database, and the API
//! remains the issuer and the policy point for both audiences.
//!
//! Failure policy: every uncertainty refuses the request. A token the API does not vouch for is a
//! `401`; an answer that cannot be obtained or understood is a `503`; neither ever serves the
//! request with broader access, with the configured workspace, or as another identity.
//!
//! Secrets: the token and the forwarding credential are [`Secret`]s. They are read from the
//! `Authorization` header (the token) and the configuration file (the credential) only, are never
//! formatted into a log line, an error or a tool result, and are never written anywhere.

use crate::client::OpenPrClient;
use crate::oauth_scopes::{self, ToolDecision};
use crate::protocol::{JsonRpcError, JsonRpcRequest, JsonRpcResponse};
use crate::server::{McpServer, ToolSurface};
use api::oauth::metadata::{PROTECTED_RESOURCE_WELL_KNOWN, protected_resource_well_known_path};
use api::oauth::scopes::{Scope, ScopeSet, TokenResource};
use axum::{
    Json, Router,
    extract::{Request, State},
    http::{HeaderMap, HeaderValue, Method, StatusCode, header},
    middleware::Next,
    response::{IntoResponse, Response},
    routing::get,
};
use platform::config::{MIN_FORWARD_SECRET_LEN, McpOAuthRuntime, Secret};
use serde_json::{Value, json};
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use uuid::Uuid;

/// Prefix of every delegated access token. No other token kind starts with it: bot tokens start
/// with `opr_` and user tokens are JWTs.
pub const DELEGATED_TOKEN_PREFIX: &str = "sva_";
/// The one endpoint that serves delegated callers (deviation D3: plain JSON-RPC over POST).
pub const DELEGATED_RPC_PATH: &str = "/mcp/rpc";
/// Path of the API's internal token-context endpoint.
pub const TOKEN_CONTEXT_PATH: &str = "/api/v1/oauth/internal/token-context";
/// `realm` of every challenge.
pub const REALM: &str = "sylvode";
/// Scopes a first connection needs for basic use, named in the challenge of a request that carries
/// no token (MCP scope selection strategy).
pub const FIRST_CONNECTION_SCOPE: &str = "projects:read work_items:read";
/// JSON-RPC code of an `insufficient_scope` refusal.
pub const JSONRPC_INSUFFICIENT_SCOPE: i32 = -32001;
/// JSON-RPC code of a refusal that no scope would cure.
pub const JSONRPC_NOT_AVAILABLE: i32 = -32002;
/// Cache lifetime of the metadata document, as for every metadata document of the contract.
const METADATA_CACHE_CONTROL: &str = "public, max-age=3600";
/// Longest inbound caller token accepted, in bytes.
///
/// A bound on a value that is copied into an outbound header and held for the life of one
/// request. Comfortably above any real Sylvode token, which is tens of bytes.
pub const MAX_CALLER_TOKEN_LEN: usize = 8 * 1024;
/// Time the token-context call may take before the request is refused with a `503`.
const CONTEXT_TIMEOUT: Duration = Duration::from_secs(5);
/// Largest token-context answer read. The real answer is a few hundred bytes.
const MAX_CONTEXT_BODY: usize = 16 * 1024;
/// The static documentation resources a delegated caller may read.
const STATIC_GUIDE_RESOURCES: [&str; 3] = [
    "sylvode://skills/openpr-mcp",
    "sylvode://guides/agents",
    "sylvode://guides/workflows",
];

/// Whether `uri` is one of the static guides, the only resources a delegated caller may read.
pub fn is_static_guide_resource(uri: &str) -> bool {
    STATIC_GUIDE_RESOURCES.contains(&uri)
}

// ---- Settings, metadata document, challenges -------------------------------------------------

/// The validated `[mcp] oauth_*` settings of a server that serves delegated callers.
#[derive(Clone)]
pub struct OAuthSettings {
    issuer: String,
    resource: String,
    forward_secret: Secret,
    metadata_path: String,
    metadata_url: String,
}

impl OAuthSettings {
    /// Checks the configured values and derives the metadata location (RFC 9728, section 3.1: the
    /// well-known prefix inserted between the host and the path of the resource identifier).
    pub fn from_runtime(runtime: &McpOAuthRuntime) -> Result<Self, String> {
        let issuer = absolute_http_url("mcp.oauth_issuer", &runtime.issuer)?;
        let resource = absolute_http_url("mcp.oauth_resource", &runtime.resource)?;
        if issuer.query().is_some() {
            return Err("mcp.oauth_issuer must not carry a query".to_string());
        }
        if runtime.forward_secret.expose().len() < MIN_FORWARD_SECRET_LEN {
            return Err(format!(
                "mcp.oauth_forward_secret must be at least {MIN_FORWARD_SECRET_LEN} bytes"
            ));
        }
        if HeaderValue::from_str(runtime.forward_secret.expose()).is_err() {
            return Err("mcp.oauth_forward_secret must be visible ASCII so that it can travel in a header".to_string());
        }
        let metadata_path = protected_resource_well_known_path(&runtime.resource)
            .ok_or_else(|| "mcp.oauth_resource must be an absolute URL".to_string())?;
        // Registered as an axum route, where braces start a parameter.
        if metadata_path.contains(['{', '}']) {
            return Err("mcp.oauth_resource must not contain braces in its path".to_string());
        }
        let metadata_url = format!("{}{metadata_path}", resource.origin().ascii_serialization());
        Ok(Self {
            issuer: runtime.issuer.clone(),
            resource: runtime.resource.clone(),
            forward_secret: runtime.forward_secret.clone(),
            metadata_path,
            metadata_url,
        })
    }

    /// The URL named in `resource_metadata` of every challenge.
    pub fn metadata_url(&self) -> &str {
        &self.metadata_url
    }

    /// Path-based location of the metadata document.
    pub fn metadata_path(&self) -> &str {
        &self.metadata_path
    }

    /// The RFC 9728 document. Its `resource` is the configured identifier byte for byte and its
    /// scope list is the MCP resource's.
    pub fn protected_resource_metadata(&self) -> Value {
        json!({
            "resource": self.resource,
            "authorization_servers": [self.issuer],
            "bearer_methods_supported": ["header"],
            "scopes_supported": TokenResource::Mcp.scopes_supported().iter().map(Scope::as_str).collect::<Vec<_>>(),
            "resource_name": "Sylvode MCP",
        })
    }
}

fn absolute_http_url(key: &str, value: &str) -> Result<reqwest::Url, String> {
    let url = reqwest::Url::parse(value).map_err(|_| format!("{key} must be an absolute URL"))?;
    if !matches!(url.scheme(), "http" | "https") || url.host_str().is_none() {
        return Err(format!("{key} must be an http or https URL with a host"));
    }
    if url.fragment().is_some() {
        return Err(format!("{key} must not carry a fragment"));
    }
    Ok(url)
}

/// A `quoted-string` of RFC 9110: backslash and double quote escaped.
fn quoted(value: &str) -> String {
    let mut out = String::with_capacity(value.len() + 2);
    out.push('"');
    for character in value.chars() {
        if matches!(character, '"' | '\\') {
            out.push('\\');
        }
        out.push(character);
    }
    out.push('"');
    out
}

/// Challenge of a request that carries no token.
///
/// With OAuth configured it points at the metadata document and names the scope a first connection
/// needs; without, it is the plain `Bearer` the server has always sent.
pub fn challenge_missing_token(settings: Option<&OAuthSettings>) -> String {
    settings.map_or_else(
        || "Bearer".to_string(),
        |settings| {
            format!(
                "Bearer realm={}, resource_metadata={}, scope={}",
                quoted(REALM),
                quoted(settings.metadata_url()),
                quoted(FIRST_CONNECTION_SCOPE)
            )
        },
    )
}

/// Challenge of a token that cannot be used: unknown, malformed, expired, revoked, issued for
/// another audience. One answer for all of them.
pub fn challenge_invalid_token(settings: Option<&OAuthSettings>) -> String {
    let mut challenge = format!("Bearer realm={}, error=\"invalid_token\"", quoted(REALM));
    if let Some(settings) = settings {
        challenge.push_str(", resource_metadata=");
        challenge.push_str(&quoted(settings.metadata_url()));
    }
    challenge
}

/// Challenge of a call the token's scopes do not allow: names every scope the call needs in one
/// value, the read scope below a write scope included (MCP scope challenge handling).
pub fn challenge_insufficient_scope(settings: &OAuthSettings, required: Scope) -> String {
    format!(
        "Bearer realm={}, error=\"insufficient_scope\", scope={}, resource_metadata={}, error_description={}",
        quoted(REALM),
        quoted(&ScopeSet::challenge_for(required).to_scope_string()),
        quoted(settings.metadata_url()),
        quoted("the connection does not grant the scope this call needs")
    )
}

fn with_challenge(status: StatusCode, challenge: &str, body: Value) -> Response {
    let mut response = (status, Json(body)).into_response();
    // Built from configuration the settings validated, so encoding cannot fail; if it ever did,
    // the status code still refuses the request.
    if let Ok(value) = HeaderValue::from_str(challenge) {
        response.headers_mut().insert(header::WWW_AUTHENTICATE, value);
    } else {
        tracing::error!("A WWW-Authenticate challenge could not be encoded as a header value");
    }
    response
}

fn invalid_token_response(settings: Option<&OAuthSettings>) -> Response {
    with_challenge(
        StatusCode::UNAUTHORIZED,
        &challenge_invalid_token(settings),
        json!({
            "error": "invalid_token",
            "error_description": "the access token is missing, invalid or expired",
        }),
    )
}

/// The metadata routes: the root form and the path-based form of RFC 9728. Registered outside the
/// token layer, because discovery happens before the client holds a token.
pub fn well_known_routes<S>(settings: &Arc<OAuthSettings>) -> Router<S>
where
    S: Clone + Send + Sync + 'static,
{
    let mut router = Router::new();
    let mut paths = vec![PROTECTED_RESOURCE_WELL_KNOWN];
    if settings.metadata_path() != PROTECTED_RESOURCE_WELL_KNOWN {
        paths.push(settings.metadata_path());
    }
    for path in paths {
        let settings = Arc::clone(settings);
        router = router.route(
            path,
            get(move || {
                let settings = Arc::clone(&settings);
                async move {
                    let mut response = Json(settings.protected_resource_metadata()).into_response();
                    response
                        .headers_mut()
                        .insert(header::CACHE_CONTROL, HeaderValue::from_static(METADATA_CACHE_CONTROL));
                    response
                }
            }),
        );
    }
    router
}

// ---- Token context ---------------------------------------------------------------------------

/// How much of the grant is exposed to this server: what the token's grant allows, as the API
/// answered for this one request.
#[derive(Clone, Debug)]
pub struct TokenContext {
    pub client_id: String,
    pub subject: Uuid,
    pub grant_id: Uuid,
    pub workspace_id: Uuid,
    pub scopes: ScopeSet,
    /// `all` or `selected`.
    pub project_mode: String,
    /// The projects of a `selected` grant; `None` for an `all` grant. The API enforces the set
    /// before any handler; this server also refuses a project argument outside it before it calls
    /// the API.
    pub project_ids: Option<Arc<[Uuid]>>,
    /// Expiry of the access token, seconds since the Unix epoch.
    pub expires_at: i64,
}

/// What a token-context answer was taken to mean.
#[derive(Clone, Debug)]
pub enum ContextOutcome {
    Active(TokenContext),
    /// The API does not vouch for the token. Unknown, expired, revoked and wrong-audience tokens
    /// are indistinguishable here by design.
    Invalid,
    /// The API could not be asked, or its answer could not be understood.
    Unavailable(UnavailableReason),
}

/// Why no verdict was obtained. Logged as a reason code; never carries a token or a body.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UnavailableReason {
    Unreachable,
    Timeout,
    UnexpectedStatus(u16),
    Malformed,
}

/// Parses a token-context answer (shaped after RFC 7662).
pub fn parse_token_context(body: &Value) -> ContextOutcome {
    let Some(map) = body.as_object() else {
        return ContextOutcome::Unavailable(UnavailableReason::Malformed);
    };
    match map.get("active").and_then(Value::as_bool) {
        Some(true) => {}
        Some(false) => return ContextOutcome::Invalid,
        None => return ContextOutcome::Unavailable(UnavailableReason::Malformed),
    }
    let text = |key: &str| map.get(key).and_then(Value::as_str);
    let id = |key: &str| text(key).and_then(|value| Uuid::parse_str(value).ok());
    let parsed = (|| {
        let project_mode = text("project_mode").filter(|mode| matches!(*mode, "all" | "selected"))?;
        // A grant limited to some projects must say which: an answer that does not is not
        // understood, and the request is refused rather than served with broader access.
        let project_ids = if project_mode == "selected" {
            let ids = map.get("project_ids")?.as_array()?;
            let parsed: Option<Vec<Uuid>> = ids
                .iter()
                .map(|value| value.as_str().and_then(|text| Uuid::parse_str(text).ok()))
                .collect();
            Some(Arc::from(parsed?))
        } else {
            None
        };
        Some(TokenContext {
            client_id: text("client_id").filter(|value| !value.is_empty())?.to_string(),
            subject: id("sub")?,
            grant_id: id("grant_id")?,
            workspace_id: id("workspace_id")?,
            scopes: text("scope").and_then(|scope| ScopeSet::parse(scope).ok())?,
            project_mode: project_mode.to_string(),
            project_ids,
            expires_at: map.get("exp").and_then(Value::as_i64)?,
        })
    })();
    parsed.map_or(
        ContextOutcome::Unavailable(UnavailableReason::Malformed),
        ContextOutcome::Active,
    )
}

/// A boxed future, so that [`TokenContextSource`] can be a trait object.
pub type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

/// Where the verdict on a delegated token comes from. The shipped implementation asks the API; a
/// test replaces it.
pub trait TokenContextSource: Send + Sync {
    fn resolve<'a>(&'a self, token: &'a Secret) -> BoxFuture<'a, ContextOutcome>;
}

/// Asks `GET {api}/api/v1/oauth/internal/token-context` with the caller's token and the
/// forwarding credential.
///
/// Answer mapping: `200` is parsed; `401` is [`ContextOutcome::Invalid`]; any other status
/// (including the `404` the API gives when it does not accept the forwarding credential), a
/// timeout, a connection failure and an unreadable answer are [`ContextOutcome::Unavailable`].
/// Redirects are not followed, because the request carries the forwarding credential in a custom
/// header that a redirect would copy to another host.
pub struct HttpTokenContextSource {
    client: reqwest::Client,
    url: String,
    forward_secret: Secret,
}

impl HttpTokenContextSource {
    pub fn new(api_base_url: &str, forward_secret: Secret) -> Result<Self, String> {
        let client = reqwest::Client::builder()
            .timeout(CONTEXT_TIMEOUT)
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .map_err(|error| format!("Failed to build the token-context HTTP client: {error}"))?;
        Ok(Self {
            client,
            url: format!("{}{TOKEN_CONTEXT_PATH}", api_base_url.trim_end_matches('/')),
            forward_secret,
        })
    }

    async fn ask(&self, token: &Secret) -> ContextOutcome {
        let sensitive = |value: &str| {
            HeaderValue::from_str(value).ok().map(|mut value| {
                value.set_sensitive(true);
                value
            })
        };
        let (Some(authorization), Some(forward_auth)) = (
            sensitive(&format!("Bearer {}", token.expose())),
            sensitive(self.forward_secret.expose()),
        ) else {
            // The inbound layer only hands over tokens that came out of a header.
            return ContextOutcome::Invalid;
        };
        let response = match self
            .client
            .get(&self.url)
            .header(header::AUTHORIZATION, authorization)
            .header("X-Sylvode-Forwarded-By", crate::client::FORWARDED_BY_VALUE)
            .header("X-Sylvode-Forward-Auth", forward_auth)
            .send()
            .await
        {
            Ok(response) => response,
            Err(error) if error.is_timeout() => return ContextOutcome::Unavailable(UnavailableReason::Timeout),
            Err(_) => return ContextOutcome::Unavailable(UnavailableReason::Unreachable),
        };
        let status = response.status();
        if status == StatusCode::UNAUTHORIZED {
            return ContextOutcome::Invalid;
        }
        if status != StatusCode::OK {
            return ContextOutcome::Unavailable(UnavailableReason::UnexpectedStatus(status.as_u16()));
        }
        let mut response = response;
        let mut body = Vec::new();
        loop {
            match response.chunk().await {
                Ok(Some(chunk)) => {
                    if body.len() + chunk.len() > MAX_CONTEXT_BODY {
                        return ContextOutcome::Unavailable(UnavailableReason::Malformed);
                    }
                    body.extend_from_slice(&chunk);
                }
                Ok(None) => break,
                Err(error) if error.is_timeout() => return ContextOutcome::Unavailable(UnavailableReason::Timeout),
                Err(_) => return ContextOutcome::Unavailable(UnavailableReason::Unreachable),
            }
        }
        serde_json::from_slice::<Value>(&body)
            .map_or(ContextOutcome::Unavailable(UnavailableReason::Malformed), |value| {
                parse_token_context(&value)
            })
    }
}

impl TokenContextSource for HttpTokenContextSource {
    fn resolve<'a>(&'a self, token: &'a Secret) -> BoxFuture<'a, ContextOutcome> {
        Box::pin(self.ask(token))
    }
}

// ---- Inbound authentication ------------------------------------------------------------------

/// The caller an inbound request was authenticated as. Placed in the request extensions by
/// [`inbound_auth`] and taken back out by the handler, which cannot be written without it.
#[derive(Clone)]
pub enum Caller {
    /// An `opr_` bot token or a user token. Forwarded to the API verbatim; the API authenticates it.
    Legacy(Secret),
    /// A delegated token the API vouched for.
    Delegated(DelegatedCaller),
}

impl Caller {
    /// The forwarded token of a legacy caller; `None` for a delegated one, which only the JSON-RPC
    /// endpoint serves.
    pub fn into_legacy(self) -> Option<Secret> {
        match self {
            Self::Legacy(token) => Some(token),
            Self::Delegated(_) => None,
        }
    }
}

/// A delegated caller: its token and what the API said that token allows.
#[derive(Clone)]
pub struct DelegatedCaller {
    pub token: Secret,
    pub context: TokenContext,
}

/// What a delegated caller was granted, as the scope gate sees it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DelegatedGrant {
    pub scopes: ScopeSet,
    pub workspace_id: Uuid,
    /// The projects of a grant limited to some; `None` reaches every project of the workspace.
    pub projects: Option<Arc<[Uuid]>>,
}

/// How the inbound layer authenticates: always the legacy way, and the delegated way as well when
/// `[mcp] oauth_*` is configured on a transport that serves it.
pub struct InboundAuth {
    delegated: Option<DelegatedAuth>,
}

struct DelegatedAuth {
    settings: Arc<OAuthSettings>,
    source: Arc<dyn TokenContextSource>,
}

impl InboundAuth {
    /// No delegated access: a `sva_` token is refused with `401`, never forwarded.
    pub const fn legacy_only() -> Self {
        Self { delegated: None }
    }

    /// Delegated access with an explicit token-context source.
    pub fn delegated(settings: OAuthSettings, source: Arc<dyn TokenContextSource>) -> Self {
        Self {
            delegated: Some(DelegatedAuth {
                settings: Arc::new(settings),
                source,
            }),
        }
    }

    /// Delegated access resolved through the API at `api_base_url`.
    pub fn from_runtime(runtime: &McpOAuthRuntime, api_base_url: &str) -> Result<Self, String> {
        let settings = OAuthSettings::from_runtime(runtime)?;
        let source = HttpTokenContextSource::new(api_base_url, runtime.forward_secret.clone())?;
        Ok(Self::delegated(settings, Arc::new(source)))
    }

    /// The settings, when delegated access is on.
    pub fn settings(&self) -> Option<&Arc<OAuthSettings>> {
        self.delegated.as_ref().map(|delegated| &delegated.settings)
    }
}

/// The token out of an `Authorization: Bearer <token>` header, if it is well formed.
pub fn bearer_token(header_value: &str) -> Option<&str> {
    let (scheme, token) = header_value.split_once(' ')?;
    if !scheme.eq_ignore_ascii_case("bearer") {
        return None;
    }
    let token = token.trim();
    if token.is_empty() { None } else { Some(token) }
}

/// The caller's token, if the request carries a usable one, and only from the `Authorization`
/// header.
///
/// `to_str` limits the value to visible ASCII, which is also what makes it safe to copy into an
/// outbound header. The length bound is the only judgement passed on the contents of a legacy
/// token: whether it names a real bot is the API's question, and a verdict reached here would be a
/// second, weaker authenticator sitting in front of the real one.
pub fn caller_token(headers: &HeaderMap) -> Option<Secret> {
    headers
        .get(header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .and_then(bearer_token)
        .filter(|token| token.len() <= MAX_CALLER_TOKEN_LEN)
        .map(Secret::new)
}

/// `sva_` followed by 64 lowercase hex characters, the only shape the authorization server mints.
fn has_delegated_token_shape(token: &str) -> bool {
    token
        .strip_prefix(DELEGATED_TOKEN_PREFIX)
        .is_some_and(|hex| hex.len() == 64 && hex.bytes().all(|byte| matches!(byte, b'0'..=b'9' | b'a'..=b'f')))
}

fn unix_now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| i64::try_from(elapsed.as_secs()).unwrap_or(i64::MAX))
}

/// Rejects any inbound request that does not carry a usable token, and hands the caller it does
/// carry to the handler.
///
/// A networked MCP request is made as somebody, or it is not made: there is no shared secret, no
/// configuration that relaxes this and no anonymous path. `/health` and the metadata document are
/// the only routes outside it.
///
/// * No token: `401`; with OAuth on, the challenge carries `resource_metadata` and `scope`.
/// * A token that does not start with `sva_`: forwarded verbatim, exactly as before.
/// * A `sva_` token: never forwarded as it came. Without delegated access, `401`. On any path but
///   `POST /mcp/rpc`, `403`. Otherwise the API is asked what the token allows: `401` when it does
///   not vouch for it, `503` when no answer can be had.
pub async fn inbound_auth(State(auth): State<Arc<InboundAuth>>, mut request: Request, next: Next) -> Response {
    let settings = auth.settings().map(AsRef::as_ref);
    let Some(token) = caller_token(request.headers()) else {
        // The rejection names no header value, on either the too-long or the absent branch.
        tracing::warn!(
            path = %request.uri().path(),
            "Rejected an inbound MCP request that carried no usable caller token"
        );
        return with_challenge(
            StatusCode::UNAUTHORIZED,
            &challenge_missing_token(settings),
            json!({
                "error": "Missing Authorization: Bearer <caller credential>. This MCP server forwards the actual caller; Flow-only user scopes require a current global-user access token, and legacy bot calls retain workspace authorization."
            }),
        );
    };

    if !token.expose().starts_with(DELEGATED_TOKEN_PREFIX) {
        request.extensions_mut().insert(Caller::Legacy(token));
        return next.run(request).await;
    }

    let Some(delegated) = auth.delegated.as_ref() else {
        tracing::warn!(
            path = %request.uri().path(),
            "Rejected a delegated token: this server does not serve delegated access"
        );
        return invalid_token_response(None);
    };
    if request.method() != Method::POST || request.uri().path() != DELEGATED_RPC_PATH {
        tracing::warn!(
            path = %request.uri().path(),
            "Rejected a delegated token on an endpoint that does not serve delegated access"
        );
        return (
            StatusCode::FORBIDDEN,
            Json(json!({
                "error": "forbidden",
                "error_description": "delegated access is available on POST /mcp/rpc only",
            })),
        )
            .into_response();
    }
    if !has_delegated_token_shape(token.expose()) {
        tracing::warn!("Rejected a delegated token that is not shaped like one");
        return invalid_token_response(Some(&delegated.settings));
    }

    match delegated.source.resolve(&token).await {
        ContextOutcome::Active(context) if context.expires_at > unix_now() => {
            tracing::debug!(
                grant_id = %context.grant_id,
                client_id = %context.client_id,
                "Accepted a delegated caller"
            );
            request
                .extensions_mut()
                .insert(Caller::Delegated(DelegatedCaller { token, context }));
            next.run(request).await
        }
        ContextOutcome::Active(_) | ContextOutcome::Invalid => {
            tracing::warn!("Rejected a delegated token the API does not vouch for");
            invalid_token_response(Some(&delegated.settings))
        }
        ContextOutcome::Unavailable(reason) => {
            tracing::warn!(reason = ?reason, "Refused a delegated request: the token context could not be obtained");
            (
                StatusCode::SERVICE_UNAVAILABLE,
                Json(json!({
                    "error": "server_error",
                    "error_description": "the access token could not be verified; try again later",
                })),
            )
                .into_response()
        }
    }
}

// ---- Scope gate ------------------------------------------------------------------------------

/// Why a delegated `tools/call` is refused before the policy chain runs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ToolRefusal {
    /// The tool exists for delegated callers, but the token's scopes do not cover it.
    InsufficientScope(Scope),
    /// No delegated caller may use the tool, or no such tool is known.
    NotAvailable,
    /// A `workspace_id` argument names a workspace other than the grant's.
    WorkspaceMismatch,
    /// A `project_id` argument names a project outside the grant's project set (or is not a
    /// project id at all). The answer is the one the API gives for every object outside a grant,
    /// whether it exists or not.
    OutsideGrant,
}

impl ToolRefusal {
    /// Text for a tool result. Names the missing scope, never another tool.
    pub fn message(self) -> String {
        match self {
            Self::InsufficientScope(scope) => format!(
                "insufficient_scope: the connection does not grant {}",
                ScopeSet::challenge_for(scope).to_scope_string()
            ),
            Self::NotAvailable => "tool not available to connected applications".to_string(),
            Self::WorkspaceMismatch => "workspace_id does not match the connected workspace".to_string(),
            Self::OutsideGrant => "the requested object is not available to this connection".to_string(),
        }
    }
}

/// The decision for one `tools/call` of a delegated caller.
///
/// The tool must be callable under the token's scopes ([`oauth_scopes`], the same table that
/// filters `tools/list`, so a tool that is not listed can not be called by guessing its name), and
/// a `workspace_id` argument, if present, must be the grant's workspace. A mismatch is refused,
/// never replaced: the caller learns nothing about the grant's workspace from the refusal. For a
/// grant limited to some projects, a `project_id` argument outside the set is refused here too,
/// before the API is called, with the answer the API gives for any object outside a grant, so the
/// refusal does not tell whether the project exists. The API checks the set again on every route.
pub fn gate_tool_call(grant: &DelegatedGrant, tool: &str, arguments: Option<&Value>) -> Result<(), ToolRefusal> {
    match oauth_scopes::decision(tool) {
        ToolDecision::NotAvailable => return Err(ToolRefusal::NotAvailable),
        ToolDecision::Requires(required) if !grant.scopes.allows(required) => {
            return Err(ToolRefusal::InsufficientScope(required));
        }
        ToolDecision::Requires(_) => {}
    }
    match arguments.and_then(|arguments| arguments.get("workspace_id")) {
        None | Some(Value::Null) => {}
        Some(Value::String(text)) if Uuid::parse_str(text).is_ok_and(|id| id == grant.workspace_id) => {}
        Some(_) => return Err(ToolRefusal::WorkspaceMismatch),
    }
    let Some(projects) = &grant.projects else {
        return Ok(());
    };
    match arguments.and_then(|arguments| arguments.get("project_id")) {
        None | Some(Value::Null) => Ok(()),
        Some(Value::String(text)) if Uuid::parse_str(text).is_ok_and(|id| projects.contains(&id)) => Ok(()),
        Some(_) => Err(ToolRefusal::OutsideGrant),
    }
}

fn refusal_response(id: Option<Value>, refusal: ToolRefusal, settings: &OAuthSettings) -> Response {
    let (code, message, data) = match refusal {
        ToolRefusal::InsufficientScope(scope) => (
            JSONRPC_INSUFFICIENT_SCOPE,
            "insufficient_scope".to_string(),
            Some(json!({"scope": ScopeSet::challenge_for(scope).to_scope_string()})),
        ),
        ToolRefusal::NotAvailable | ToolRefusal::WorkspaceMismatch | ToolRefusal::OutsideGrant => {
            (JSONRPC_NOT_AVAILABLE, refusal.message(), None)
        }
    };
    let body = JsonRpcResponse::error(id, JsonRpcError { code, message, data });
    match refusal {
        ToolRefusal::InsufficientScope(scope) => with_challenge(
            StatusCode::FORBIDDEN,
            &challenge_insufficient_scope(settings, scope),
            json!(body),
        ),
        // No challenge: no other scope would make the call succeed, and a step-up that cannot
        // succeed would only send the client round a loop.
        ToolRefusal::NotAvailable | ToolRefusal::WorkspaceMismatch | ToolRefusal::OutsideGrant => {
            (StatusCode::FORBIDDEN, Json(json!(body))).into_response()
        }
    }
}

// ---- JSON-RPC endpoint -----------------------------------------------------------------------

/// Serves one JSON-RPC request for an authenticated caller.
///
/// A legacy caller is served exactly as before: its token is forwarded verbatim and no forwarding
/// credential is sent. A delegated caller is served by a server built for that one request, acting
/// in the workspace of its token context with its token and the forwarding credential, narrowed by
/// the scope table.
///
/// `surface` is the tool listing the connection asked for; it applies to a legacy caller only. A
/// delegated caller is served the scoped registry whatever it says.
pub async fn handle_rpc(
    client: &OpenPrClient,
    auth: &InboundAuth,
    caller: Caller,
    surface: ToolSurface,
    request: JsonRpcRequest,
) -> Response {
    match caller {
        Caller::Legacy(token) => respond(McpServer::for_surface(client.acting_as(token), surface), request).await,
        Caller::Delegated(delegated) => {
            let Some(state) = auth.delegated.as_ref() else {
                // A delegated caller can only have been made by a layer that had delegated access.
                return invalid_token_response(None);
            };
            let DelegatedCaller { token, context } = delegated;
            let acting = client.acting_as_delegated(
                token,
                &context.workspace_id.to_string(),
                state.settings.forward_secret.clone(),
            );
            let server = McpServer::new_delegated(
                acting,
                DelegatedGrant {
                    scopes: context.scopes,
                    workspace_id: context.workspace_id,
                    projects: context.project_ids.clone(),
                },
            );
            if request.method == "tools/call"
                && let Some(refusal) = server.delegated_refusal(request.params.as_ref())
            {
                tracing::warn!(
                    grant_id = %context.grant_id,
                    refusal = ?refusal,
                    "Refused a delegated tools/call"
                );
                return refusal_response(request.id, refusal, &state.settings);
            }
            respond(server, request).await
        }
    }
}

async fn respond(server: McpServer, request: JsonRpcRequest) -> Response {
    server.handle_request(request).await.map_or_else(
        || (StatusCode::ACCEPTED, Json(json!({"status": "accepted"}))).into_response(),
        |response| (StatusCode::OK, Json(json!(response))).into_response(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    const TOKEN: &str = "opr_caller_bot_token_example";
    const DELEGATED: &str = "sva_0000000000000000000000000000000000000000000000000000000000000000";
    const WORKSPACE: Uuid = Uuid::from_u128(0x1111_1111_1111_4111_8111_1111_1111_1111);
    const PROJECT_ONE: Uuid = Uuid::from_u128(0x2222_2222_2222_4222_8222_2222_2222_2222);
    const PROJECT_TWO: Uuid = Uuid::from_u128(0x3333_3333_3333_4333_8333_3333_3333_3333);

    fn runtime(resource: &str) -> McpOAuthRuntime {
        McpOAuthRuntime {
            issuer: "https://sv.example.test".to_string(),
            resource: resource.to_string(),
            forward_secret: Secret::new("forward-secret-for-tests-0123456789abcdef"),
        }
    }

    fn settings(resource: &str) -> Result<OAuthSettings, String> {
        OAuthSettings::from_runtime(&runtime(resource))
    }

    #[test]
    fn bearer_parsing_accepts_only_a_well_formed_bearer_header() {
        assert_eq!(bearer_token("Bearer abc"), Some("abc"));
        assert_eq!(bearer_token("bearer  abc  "), Some("abc"));
        assert_eq!(bearer_token("BEARER abc"), Some("abc"));
        assert_eq!(bearer_token("Basic abc"), None);
        assert_eq!(bearer_token("Bearer "), None);
        assert_eq!(bearer_token("abc"), None);
    }

    fn headers(pairs: &[(header::HeaderName, &str)]) -> Result<HeaderMap, header::InvalidHeaderValue> {
        let mut map = HeaderMap::new();
        for (name, value) in pairs {
            map.insert(name.clone(), HeaderValue::from_str(value)?);
        }
        Ok(map)
    }

    /// The token the caller presented is the token that is carried onward, byte for byte.
    #[test]
    fn the_callers_token_is_taken_verbatim_from_the_authorization_header() -> Result<(), Box<dyn std::error::Error>> {
        let extracted = caller_token(&headers(&[(header::AUTHORIZATION, &format!("Bearer {TOKEN}"))])?);
        assert_eq!(extracted.as_ref().map(Secret::expose), Some(TOKEN));
        Ok(())
    }

    /// Every way of not presenting a token has to come out the same: no identity.
    #[test]
    fn a_request_without_a_usable_bearer_token_yields_no_identity() -> Result<(), Box<dyn std::error::Error>> {
        let too_long = "opr_".to_string() + &"x".repeat(MAX_CALLER_TOKEN_LEN);
        for map in [
            headers(&[])?,
            headers(&[(header::AUTHORIZATION, "")])?,
            headers(&[(header::AUTHORIZATION, "Bearer")])?,
            headers(&[(header::AUTHORIZATION, "Bearer   ")])?,
            headers(&[(header::AUTHORIZATION, &format!("Basic {TOKEN}"))])?,
            headers(&[(header::AUTHORIZATION, TOKEN)])?,
            headers(&[(header::PROXY_AUTHORIZATION, &format!("Bearer {TOKEN}"))])?,
            headers(&[(header::AUTHORIZATION, &format!("Bearer {too_long}"))])?,
        ] {
            assert!(
                caller_token(&map).is_none(),
                "a request with {map:?} was given an identity"
            );
        }
        Ok(())
    }

    /// A cap that rejected legitimate credentials would be an availability bug.
    #[test]
    fn a_token_at_the_length_limit_is_still_accepted() -> Result<(), Box<dyn std::error::Error>> {
        let at_limit = "x".repeat(MAX_CALLER_TOKEN_LEN);
        let extracted = caller_token(&headers(&[(header::AUTHORIZATION, &format!("Bearer {at_limit}"))])?);
        assert_eq!(extracted.as_ref().map(Secret::expose), Some(at_limit.as_str()));
        Ok(())
    }

    #[test]
    fn the_token_is_never_taken_from_anywhere_but_the_authorization_header() -> Result<(), Box<dyn std::error::Error>> {
        let map = headers(&[
            (header::COOKIE, &format!("access_token={DELEGATED}")),
            (header::PROXY_AUTHORIZATION, &format!("Bearer {DELEGATED}")),
        ])?;
        assert!(caller_token(&map).is_none());
        Ok(())
    }

    #[test]
    fn delegated_token_shape_is_prefix_plus_64_lowercase_hex() {
        assert!(has_delegated_token_shape(DELEGATED));
        assert!(!has_delegated_token_shape("sva_"));
        assert!(!has_delegated_token_shape(&format!("{DELEGATED}0")));
        assert!(!has_delegated_token_shape(&DELEGATED.to_uppercase()));
        assert!(!has_delegated_token_shape(&DELEGATED.replace("sva_", "svr_")));
        assert!(!has_delegated_token_shape(&format!("sva_{}", "g".repeat(64))));
    }

    #[test]
    fn metadata_location_follows_rfc_9728() -> Result<(), String> {
        let direct = settings("http://127.0.0.1:8090/mcp/rpc")?;
        assert_eq!(direct.metadata_path(), "/.well-known/oauth-protected-resource/mcp/rpc");
        assert_eq!(
            direct.metadata_url(),
            "http://127.0.0.1:8090/.well-known/oauth-protected-resource/mcp/rpc"
        );
        let proxied = settings("https://sv.example.test/mcp")?;
        assert_eq!(proxied.metadata_path(), "/.well-known/oauth-protected-resource/mcp");
        assert_eq!(
            proxied.metadata_url(),
            "https://sv.example.test/.well-known/oauth-protected-resource/mcp"
        );
        let root = settings("https://sv.example.test")?;
        assert_eq!(root.metadata_path(), "/.well-known/oauth-protected-resource");
        Ok(())
    }

    #[test]
    fn the_metadata_document_matches_the_contract_field_by_field() -> Result<(), String> {
        let document = settings("https://sv.example.test/mcp")?.protected_resource_metadata();
        let expected = json!({
            "resource": "https://sv.example.test/mcp",
            "authorization_servers": ["https://sv.example.test"],
            "bearer_methods_supported": ["header"],
            "scopes_supported": ["projects:read", "work_items:read", "work_items:write", "forms:read", "forms:write"],
            "resource_name": "Sylvode MCP"
        });
        assert_eq!(document, expected);
        let map = document.as_object().ok_or("not an object")?;
        assert_eq!(map.len(), 5);
        for (key, value) in expected.as_object().ok_or("not an object")? {
            assert_eq!(map.get(key), Some(value), "{key}");
        }
        let scopes =
            serde_json::to_string(map.get("scopes_supported").ok_or("no scopes")?).map_err(|e| e.to_string())?;
        assert!(
            !scopes.contains("offline_access"),
            "offline_access is not a resource scope"
        );
        assert!(
            !scopes.contains("notifications"),
            "the API's scopes are not the MCP server's"
        );
        Ok(())
    }

    #[test]
    fn invalid_settings_are_refused_by_name() {
        let mut bad = runtime("https://sv.example.test/mcp");
        bad.forward_secret = Secret::new("short");
        assert!(OAuthSettings::from_runtime(&bad).is_err_and(|e| e.contains("oauth_forward_secret")));
        for resource in [
            "not a url",
            "ftp://sv.example.test/mcp",
            "https://sv.example.test/mcp#frag",
            "https://sv.example.test/{x}",
        ] {
            assert!(settings(resource).is_err(), "{resource}");
        }
        let mut issuer = runtime("https://sv.example.test/mcp");
        issuer.issuer = "https://sv.example.test/?q=1".to_string();
        assert!(OAuthSettings::from_runtime(&issuer).is_err_and(|e| e.contains("oauth_issuer")));
    }

    /// Parses the parameters of a `Bearer` challenge, resolving quoted-string escapes.
    fn challenge_params(challenge: &str) -> Result<Vec<(String, String)>, String> {
        let rest = challenge.strip_prefix("Bearer ").ok_or("not a Bearer challenge")?;
        let mut params = Vec::new();
        let mut chars = rest.chars().peekable();
        while chars.peek().is_some() {
            let name: String = chars.by_ref().take_while(|c| *c != '=').collect();
            if chars.next() != Some('"') {
                return Err("value is not quoted".to_string());
            }
            let mut value = String::new();
            loop {
                match chars.next().ok_or("unterminated quoted string")? {
                    '\\' => value.push(chars.next().ok_or("dangling escape")?),
                    '"' => break,
                    other => value.push(other),
                }
            }
            params.push((name.trim().to_string(), value));
            match chars.next() {
                None => break,
                Some(',') => {
                    while chars.peek() == Some(&' ') {
                        chars.next();
                    }
                }
                Some(other) => return Err(format!("unexpected {other:?} between parameters")),
            }
        }
        Ok(params)
    }

    fn param<'a>(params: &'a [(String, String)], name: &str) -> Option<&'a str> {
        params
            .iter()
            .find(|(key, _)| key == name)
            .map(|(_, value)| value.as_str())
    }

    #[test]
    fn the_missing_token_challenge_carries_resource_metadata_and_scope() -> Result<(), String> {
        let settings = settings("http://127.0.0.1:8090/mcp/rpc")?;
        let params = challenge_params(&challenge_missing_token(Some(&settings)))?;
        assert_eq!(param(&params, "realm"), Some("sylvode"));
        assert_eq!(
            param(&params, "resource_metadata"),
            Some("http://127.0.0.1:8090/.well-known/oauth-protected-resource/mcp/rpc")
        );
        assert_eq!(param(&params, "scope"), Some("projects:read work_items:read"));
        assert_eq!(param(&params, "error"), None, "no token is not an invalid token");
        assert_eq!(challenge_missing_token(None), "Bearer");
        Ok(())
    }

    #[test]
    fn the_invalid_token_challenge_has_the_error_and_the_metadata_and_no_scope() -> Result<(), String> {
        let settings = settings("https://sv.example.test/mcp")?;
        let params = challenge_params(&challenge_invalid_token(Some(&settings)))?;
        assert_eq!(param(&params, "error"), Some("invalid_token"));
        assert_eq!(
            param(&params, "resource_metadata"),
            Some("https://sv.example.test/.well-known/oauth-protected-resource/mcp")
        );
        assert_eq!(param(&params, "scope"), None);
        assert_eq!(param(&params, "error_description"), None, "no reason is given");
        let without = challenge_params(&challenge_invalid_token(None))?;
        assert_eq!(param(&without, "error"), Some("invalid_token"));
        assert_eq!(param(&without, "resource_metadata"), None);
        Ok(())
    }

    #[test]
    fn the_insufficient_scope_challenge_names_the_whole_scope_set_in_one_value() -> Result<(), String> {
        let settings = settings("https://sv.example.test/mcp")?;
        let params = challenge_params(&challenge_insufficient_scope(&settings, Scope::WorkItemsWrite))?;
        assert_eq!(param(&params, "error"), Some("insufficient_scope"));
        assert_eq!(param(&params, "scope"), Some("work_items:read work_items:write"));
        assert!(param(&params, "resource_metadata").is_some());
        assert!(param(&params, "error_description").is_some());
        let read = challenge_params(&challenge_insufficient_scope(&settings, Scope::FormsRead))?;
        assert_eq!(param(&read, "scope"), Some("forms:read"));
        Ok(())
    }

    #[test]
    fn quoting_escapes_what_would_end_a_quoted_string() {
        assert_eq!(quoted(r#"a"b\c"#), r#""a\"b\\c""#);
    }

    fn active_body() -> Value {
        json!({
            "active": true, "client_id": "ramflux",
            "sub": "33333333-3333-4333-8333-333333333333",
            "grant_id": "44444444-4444-4444-8444-444444444444",
            "workspace_id": WORKSPACE.to_string(),
            "scope": "projects:read work_items:read", "project_mode": "selected",
            "project_ids": [PROJECT_ONE.to_string(), PROJECT_TWO.to_string()], "exp": 4_102_444_800_i64
        })
    }

    #[test]
    fn a_complete_active_answer_is_parsed() {
        match parse_token_context(&active_body()) {
            ContextOutcome::Active(context) => {
                assert_eq!(context.workspace_id, WORKSPACE);
                assert_eq!(context.scopes.to_scope_string(), "projects:read work_items:read");
                assert_eq!(context.project_mode, "selected");
                assert_eq!(
                    context.project_ids.as_deref(),
                    Some(&[PROJECT_ONE, PROJECT_TWO][..]),
                    "the project set of a selected grant"
                );
                assert_eq!(context.client_id, "ramflux");
            }
            other => assert!(matches!(other, ContextOutcome::Active(_)), "not active: {other:?}"),
        }
    }

    #[test]
    fn an_inactive_answer_is_invalid_and_every_malformed_answer_is_unavailable() {
        assert!(matches!(
            parse_token_context(&json!({"active": false})),
            ContextOutcome::Invalid
        ));
        let mut cases = vec![
            json!(null),
            json!([]),
            json!("active"),
            json!({}),
            json!({"active": "true"}),
        ];
        for key in [
            "client_id",
            "sub",
            "grant_id",
            "workspace_id",
            "scope",
            "project_mode",
            "exp",
        ] {
            let mut body = active_body();
            if let Some(map) = body.as_object_mut() {
                map.remove(key);
            }
            cases.push(body);
        }
        for (key, value) in [
            ("workspace_id", json!("not-a-uuid")),
            ("scope", json!("projects:read admin")),
            ("scope", json!("")),
            ("project_mode", json!("some")),
            ("exp", json!("soon")),
            ("client_id", json!("")),
        ] {
            let mut body = active_body();
            if let Some(map) = body.as_object_mut() {
                map.insert(key.to_string(), value);
            }
            cases.push(body);
        }
        for case in cases {
            assert!(
                matches!(
                    parse_token_context(&case),
                    ContextOutcome::Unavailable(UnavailableReason::Malformed)
                ),
                "{case}"
            );
        }
    }

    fn grant(scope: &str) -> Result<DelegatedGrant, api::oauth::scopes::ScopeError> {
        Ok(DelegatedGrant {
            scopes: ScopeSet::parse(scope)?,
            workspace_id: WORKSPACE,
            projects: None,
        })
    }

    fn set(body: &mut Value, key: &str, value: Value) {
        if let Some(map) = body.as_object_mut() {
            map.insert(key.to_string(), value);
        }
    }

    #[test]
    fn a_selected_grant_must_name_its_projects_and_an_all_grant_has_none() {
        let mut body = active_body();
        for broken in [
            // Missing, not a list, or holding something that is not a project id.
            None,
            Some(json!("2222")),
            Some(json!([PROJECT_ONE.to_string(), "not-a-uuid"])),
            Some(json!([42])),
        ] {
            match &broken {
                Some(value) => set(&mut body, "project_ids", value.clone()),
                None => {
                    body.as_object_mut().map(|map| map.remove("project_ids"));
                }
            }
            assert!(
                matches!(
                    parse_token_context(&body),
                    ContextOutcome::Unavailable(UnavailableReason::Malformed)
                ),
                "{broken:?}"
            );
        }
        // An empty set is a valid, empty grant: nothing is reachable.
        set(&mut body, "project_ids", json!([]));
        match parse_token_context(&body) {
            ContextOutcome::Active(context) => assert_eq!(context.project_ids.as_deref(), Some(&[][..])),
            other => assert!(matches!(other, ContextOutcome::Active(_)), "{other:?}"),
        }
        // `all` carries no set, and a stray one is ignored.
        set(&mut body, "project_mode", json!("all"));
        match parse_token_context(&body) {
            ContextOutcome::Active(context) => assert!(context.project_ids.is_none()),
            other => assert!(matches!(other, ContextOutcome::Active(_)), "{other:?}"),
        }
    }

    #[test]
    fn a_project_argument_outside_a_selected_grant_is_refused_without_saying_why()
    -> Result<(), api::oauth::scopes::ScopeError> {
        let mut selected = grant("projects:read work_items:read")?;
        selected.projects = Some(Arc::from(vec![PROJECT_ONE]));
        let ask = |project: Value| gate_tool_call(&selected, "work_items.list", Some(&json!({"project_id": project})));
        assert_eq!(ask(json!(PROJECT_ONE.to_string())), Ok(()));
        // Another project of the workspace, an unknown one, a malformed one and a non-string all
        // get the one answer.
        for outside in [
            json!(PROJECT_TWO.to_string()),
            json!(Uuid::from_u128(9).to_string()),
            json!("not-a-uuid"),
            json!(7),
            json!(["x"]),
        ] {
            assert_eq!(ask(outside.clone()), Err(ToolRefusal::OutsideGrant), "{outside}");
        }
        assert!(!ToolRefusal::OutsideGrant.message().contains("2222"));
        // No project argument: nothing to refuse here (the API checks what the tool resolves).
        assert_eq!(gate_tool_call(&selected, "projects.list", None), Ok(()));
        // An all-projects grant takes any project argument to the API.
        let all = grant("projects:read work_items:read")?;
        assert_eq!(
            gate_tool_call(
                &all,
                "work_items.list",
                Some(&json!({"project_id": PROJECT_TWO.to_string()}))
            ),
            Ok(())
        );
        // An empty selection reaches no project.
        let mut empty = grant("work_items:read")?;
        empty.projects = Some(Arc::from(Vec::<Uuid>::new()));
        assert_eq!(
            gate_tool_call(
                &empty,
                "work_items.list",
                Some(&json!({"project_id": PROJECT_ONE.to_string()}))
            ),
            Err(ToolRefusal::OutsideGrant)
        );
        Ok(())
    }

    #[test]
    fn the_gate_allows_a_scoped_read_tool_and_refuses_the_rest() -> Result<(), api::oauth::scopes::ScopeError> {
        let read = grant("projects:read")?;
        assert_eq!(gate_tool_call(&read, "projects.list", None), Ok(()));
        assert_eq!(
            gate_tool_call(&read, "work_items.list", None),
            Err(ToolRefusal::InsufficientScope(Scope::WorkItemsRead))
        );
        // Unknown names and withheld tools look the same.
        assert_eq!(
            gate_tool_call(&read, "no.such_tool", None),
            Err(ToolRefusal::NotAvailable)
        );
        assert_eq!(
            gate_tool_call(&read, "projects.delete", None),
            Err(ToolRefusal::NotAvailable)
        );
        Ok(())
    }

    /// A write scope unlocks the read tools it implies and nothing that changes data.
    #[test]
    fn a_write_scope_never_unlocks_a_write_tool() -> Result<(), api::oauth::scopes::ScopeError> {
        let all = grant("projects:read work_items:read work_items:write forms:read forms:write")?;
        for tool in [
            "work_items.create",
            "work_items.update",
            "comments.create",
            "form_records.create",
            "form_records.update",
        ] {
            assert_eq!(
                gate_tool_call(&all, tool, None),
                Err(ToolRefusal::NotAvailable),
                "{tool}"
            );
        }
        let write_only = grant("work_items:write")?;
        assert_eq!(gate_tool_call(&write_only, "work_items.get", None), Ok(()), "hierarchy");
        Ok(())
    }

    #[test]
    fn a_workspace_argument_must_be_the_grants_workspace() -> Result<(), api::oauth::scopes::ScopeError> {
        let read = grant("projects:read")?;
        let own = WORKSPACE.to_string();
        let other = "99999999-9999-4999-8999-999999999999";
        assert_eq!(
            gate_tool_call(&read, "projects.list", Some(&json!({"workspace_id": own}))),
            Ok(())
        );
        assert_eq!(
            gate_tool_call(&read, "projects.list", Some(&json!({"workspace_id": null}))),
            Ok(())
        );
        assert_eq!(gate_tool_call(&read, "projects.list", Some(&json!({}))), Ok(()));
        for value in [
            json!(other),
            json!("garbage"),
            json!(7),
            json!(true),
            json!([own]),
            json!({"id": own}),
        ] {
            assert_eq!(
                gate_tool_call(&read, "projects.list", Some(&json!({"workspace_id": value}))),
                Err(ToolRefusal::WorkspaceMismatch),
                "{value}"
            );
        }
        Ok(())
    }

    #[test]
    fn a_refusal_names_the_missing_scope_and_no_other_tool() {
        let message = ToolRefusal::InsufficientScope(Scope::WorkItemsRead).message();
        assert!(message.contains("work_items:read"));
        assert!(!message.contains('.'), "{message}");
        assert!(!ToolRefusal::WorkspaceMismatch.message().contains("1111"));
    }

    #[test]
    fn only_the_static_guides_are_readable_resources() {
        for uri in STATIC_GUIDE_RESOURCES {
            assert!(is_static_guide_resource(uri));
        }
        for uri in [
            "sylvode://scenario-templates",
            "sylvode://forms/22222222-2222-4222-8222-222222222222/records",
            "sylvode://skills/openpr-mcp/",
            "",
        ] {
            assert!(!is_static_guide_resource(uri), "{uri}");
        }
    }
}
