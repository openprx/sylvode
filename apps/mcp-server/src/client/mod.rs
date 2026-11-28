use platform::config::{McpTransport, Secret};
use reqwest::{Client, RequestBuilder, multipart};
use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::{Value, json};
use std::sync::Arc;

mod hints;
pub use hints::{
    FIRST_OPEN_TIMEOUT, HINTS_PATH, HintEvent, HintOpenError, HintReadError, HintStream, RECONNECT_OPEN_TIMEOUT,
};
mod error_class;
pub use error_class::{ApiErrorClass, classify_api_error, is_backend_failure};
mod tasks_state;
mod workspace;
use workspace::{WORKSPACE_CACHE_CAPACITY, WORKSPACE_CACHE_TTL, WorkspaceCache, workspace_of_me};

/// The Sylvode API always answers with HTTP 200 and carries the real status in the
/// response envelope (`{ "code": 0, "message": "...", "data": ... }`): every handler
/// returns `ApiResponse<T>` (`apps/api/src/response.rs`) and every failure returns
/// `ApiError` (`apps/api/src/error.rs`). Any non-zero `code` is an API error and must
/// never be handed to callers as a successful payload.
///
/// A payload that is not such an envelope is rejected instead of being accepted as
/// success. Treating "no `code` field" as success was a fail-open default: an empty
/// body, a `204`, a proxy/error page or any response reached through a smuggled URL
/// looked exactly like a successful call, which is precisely how a policy lookup that
/// never hit `/agent-policy` could still authorize a tool.
fn check_response_envelope(payload: &Value, path: &str) -> Result<(), String> {
    let Some(envelope) = payload.as_object() else {
        return Err(format!(
            "Malformed response from {path}: expected a {{code, message, data}} envelope, got {}",
            describe_json_kind(payload)
        ));
    };
    match envelope.get("code").and_then(Value::as_i64) {
        Some(0) => Ok(()),
        Some(403) if envelope.get("message").and_then(Value::as_str) == Some(SCOPED_BOT_REFUSAL) => Err(format!(
            "API error 403 from {path}: {SCOPED_BOT_REFUSAL}; the token's project access does not allow this call, so \
             retrying it or using another tool will not help"
        )),
        Some(code) => Err(refusal_reason(code).map_or_else(
            || {
                let message = envelope
                    .get("message")
                    .and_then(Value::as_str)
                    .unwrap_or("unknown API error");
                format!("API error {code} from {path}: {message}")
            },
            |reason| format!("API error {code} from {path}: {reason}"),
        )),
        None => Err(format!(
            "Malformed response from {path}: the {{code, message, data}} envelope carries no integer `code`"
        )),
    }
}

/// What the API answers (code `403`) when a bot token limited to some projects acts in a project
/// whose level for it is too low, or in none it may reach. One text for every case, so it never
/// says which part failed.
const SCOPED_BOT_REFUSAL: &str = "this bot may not do this in this project";

/// The fixed wording an *authentication* failure is reported with, or `None` for every other
/// status, whose own message is relayed.
///
/// `401` is the one status where the caller has not been identified at all. The MCP server
/// forwards a credential it never verified, so a caller can put an arbitrary token on the
/// wire and read back whatever the backend says about it — and what the backend says is prose
/// written for an operator, which is where an internal hostname or a middleware name leaks.
/// That makes an unauthenticated stranger the one party who must be told the least, so they
/// are told only the verdict and how to act on it.
///
/// `403` is deliberately not on this list. A `403` means the credential *was* accepted and
/// the bot is known; the message then explains which rule refused a legitimate caller's
/// request ("policy denied", "not a member of this workspace"), and withholding it would cost
/// every real user their only explanation to buy nothing an authenticated caller could not
/// already learn by making the call.
const fn refusal_reason(code: i64) -> Option<&'static str> {
    match code {
        401 => Some(
            "the Sylvode API rejected the credential this call was made with; check that the caller bearer credential is \
             correct and not expired",
        ),
        _ => None,
    }
}

/// What a caller is told about a non-2xx answer.
///
/// The Sylvode API itself answers `200` and carries its status in the envelope, so a real
/// HTTP status here comes from something in front of it — a proxy, a gateway, a load
/// balancer — whose error page is not written for this caller at all. The same reasoning as
/// [`refusal_reason`] therefore applies to the same status, and to a body rather than to an
/// envelope message.
pub fn rejected_request_error(status: reqwest::StatusCode, path: &str, body: &str) -> String {
    refusal_reason(i64::from(status.as_u16())).map_or_else(
        || format!("HTTP {status} from {path}: {body}"),
        |reason| format!("HTTP {status} from {path}: {reason}"),
    )
}

const fn describe_json_kind(value: &Value) -> &'static str {
    match value {
        Value::Null => "an empty body",
        Value::Bool(_) => "a boolean",
        Value::Number(_) => "a number",
        Value::String(_) => "a string",
        Value::Array(_) => "an array",
        Value::Object(_) => "an object",
    }
}

/// `/api/v1/ai/tasks/{task_id}{suffix}?attempt={attempt}`. The caller has already checked that
/// `task_id` is a canonical UUID.
fn ai_task_path(task_id: &str, suffix: &str, attempt: u32) -> String {
    format!(
        "/api/v1/ai/tasks/{}{suffix}?attempt={attempt}",
        urlencoding::encode(task_id)
    )
}

/// Operation-log client label for the stdio transport.
pub const TRANSPORT_LABEL_STDIO: &str = "mcp_stdio";
/// Audit label of the HTTP transport.
pub const TRANSPORT_LABEL_HTTP: &str = "mcp_http";
/// Audit label of the SSE transport.
pub const TRANSPORT_LABEL_SSE: &str = "mcp_sse";
/// Audit label of a direct CLI command.
pub const TRANSPORT_LABEL_CLI: &str = "cli";
/// Audit label of the CLI's `tools call` escape hatch (`mcp-surface-v1.md`: "CLI 的 `tools call`
/// 经 stdio-style configured bot 时使用 `surface=cli_tools_call`"), distinct from the native
/// commands' `cli`.
pub const TRANSPORT_LABEL_CLI_TOOLS_CALL: &str = "cli_tools_call";

/// Attribution headers (ADR-0020 D3).
const MCP_TOOL_HEADER: &str = "X-Sylvode-MCP-Tool";
const MCP_SURFACE_HEADER: &str = "X-Sylvode-MCP-Surface";

/// Names the forwarding component on a delegated call (`X-Sylvode-Forwarded-By: mcp-server`).
const FORWARDED_BY_HEADER: &str = "X-Sylvode-Forwarded-By";
/// Value of [`FORWARDED_BY_HEADER`].
pub const FORWARDED_BY_VALUE: &str = "mcp-server";
/// Carries the internal forwarding credential on a delegated call.
const FORWARD_AUTH_HEADER: &str = "X-Sylvode-Forward-Auth";

/// The audit label a configured transport is reported under.
///
/// The three labels are a stable wire contract with the API, which stores them verbatim, so
/// they are spelled out here rather than derived from the enum's own `as_str`.
pub const fn transport_label(transport: McpTransport) -> &'static str {
    match transport {
        McpTransport::Stdio => TRANSPORT_LABEL_STDIO,
        McpTransport::Http => TRANSPORT_LABEL_HTTP,
        McpTransport::Sse => TRANSPORT_LABEL_SSE,
    }
}

/// What a client with no credential answers when something asks it to call the API.
///
/// Reaching this is a bug, not a deployment mistake: the networked transports install the
/// caller's credential before any handler runs, and the transports that have no caller
/// (stdio, the CLI subcommands) are configured with `mcp.bot_token`. It is an error rather
/// than a fallback so that the failure is a refusal, never an unattributed call made with
/// somebody else's authority.
const NO_OUTBOUND_CREDENTIAL: &str = "Refusing to call the Sylvode API with no credential: this request carried no caller identity \
     and this process holds none of its own";

/// Everything the client needs, already resolved from the configuration file and the CLI
/// flags that override it.
///
/// Carries no `Debug` on purpose: it may hold a credential, and the point of [`Secret`] is
/// that no derived `Debug` anywhere can start printing it.
pub struct ClientConfig {
    /// Base URL of the Sylvode API, from `mcp.api_url` or `--api-url`.
    pub base_url: String,
    /// The identity this client presents to the API.
    ///
    /// `None` for the networked transports, which hold no identity of their own and are
    /// handed the caller's through [`OpenPrClient::acting_as`] once per request.
    pub credential: Option<Secret>,
    /// Workspace the caller acts in, from `mcp.workspace_id` or `--workspace-id`.
    ///
    /// `None` lets the client take it from the credential's bot (`GET /api/v1/auth/me`) when a
    /// tool needs one. A networked caller's client never uses it either way: see
    /// [`OpenPrClient::acting_as`].
    pub workspace_id: Option<String>,
    /// Label the operation log reports this process's transport under.
    pub transport_label: &'static str,
}

#[derive(Clone)]
pub struct OpenPrClient {
    // `pub(crate)`, not private: `cli_app::api_client`'s `send_structured` builds requests the
    // same way `Self::send` does, and lives outside this dually-compiled module (see that
    // module's doc comment) so it cannot reach a private field here.
    pub(crate) client: Client,
    pub(crate) base_url: String,
    /// The bearer credential every outbound request carries. See [`ClientConfig::credential`].
    credential: Option<Secret>,
    /// The workspace workspace-scoped tools act in, when it is fixed: configured for `stdio` and
    /// the command line, the grant's for a delegated caller. `None` means the credential's bot
    /// names it, through `workspace_cache`.
    workspace_id: Option<String>,
    /// Workspace by token hash, shared by every client made from one another.
    workspace_cache: Arc<WorkspaceCache>,
    transport_label: &'static str,
    operation_tool_name: Option<String>,
    flow_only: bool,
    /// The internal forwarding credential, present only on a client acting for a delegated
    /// (OAuth) caller. Every outbound request of such a client carries it; no other client does.
    forward_credential: Option<Secret>,
    /// HTTP client that never follows a redirect. A delegated call carries the forwarding
    /// credential in a custom header, which a redirect would copy to whatever host the answer
    /// names, so those calls are made with this client instead of `client`.
    no_redirect_client: Client,
}

fn join_query(params: &[String]) -> String {
    if params.is_empty() {
        String::new()
    } else {
        format!("?{}", params.join("&"))
    }
}

pub fn encode_query_component(value: &str) -> String {
    urlencoding::encode(value)
}

fn event_query_params(event_type: Option<&str>, page: Option<u32>, per_page: Option<u32>) -> Vec<String> {
    let mut params = Vec::new();
    if let Some(event_type) = event_type {
        params.push(format!("event_type={}", urlencoding::encode(event_type)));
    }
    if let Some(page) = page {
        params.push(format!("page={page}"));
    }
    if let Some(per_page) = per_page {
        params.push(format!("per_page={per_page}"));
    }
    params
}

/// Query parameters supported by `GET /api/v1/projects/{project_id}/issues`
/// (`apps/api/src/routes/issue.rs` `ListIssuesQuery`).
#[derive(Debug, Clone, Copy, Default)]
pub struct ListWorkItemsQuery<'a> {
    pub page: Option<u64>,
    pub per_page: Option<u64>,
    pub state: Option<&'a str>,
    pub assignee_id: Option<&'a str>,
    pub priority: Option<&'a str>,
    /// A sprint id, or `none` for the items in no sprint.
    pub sprint_id: Option<&'a str>,
    pub search: Option<&'a str>,
    pub label_ids: Option<&'a str>,
    pub sort_by: Option<&'a str>,
    pub sort_order: Option<&'a str>,
    /// A milestone id, or `none` for the items in no milestone.
    pub milestone_id: Option<&'a str>,
}

/// Query parameters supported by `GET /api/v1/workspaces/{id}/milestones`.
#[derive(Debug, Default)]
pub struct ListMilestonesQuery<'a> {
    pub status: Option<&'a str>,
    pub page: Option<u64>,
    pub per_page: Option<u64>,
}

/// Query parameters supported by `GET /api/v1/proposals`
/// (`apps/api/src/routes/proposal.rs` `ListProposalsQuery`).
#[derive(Debug, Clone, Copy, Default)]
pub struct ListProposalsQuery<'a> {
    pub status: Option<&'a str>,
    pub proposal_type: Option<&'a str>,
    pub domain: Option<&'a str>,
    pub page: Option<u32>,
    pub per_page: Option<u32>,
    pub sort: Option<&'a str>,
}

/// Query parameters supported by `GET /api/v1/forms/{form_id}/records`
/// (`apps/api/src/routes/form.rs` `ListRecordsQuery`).
#[derive(Debug, Clone, Copy, Default)]
pub struct ListRecordsQuery<'a> {
    pub page: Option<u32>,
    pub per_page: Option<u32>,
    pub view_id: Option<&'a str>,
    pub sort: Option<&'a str>,
    pub filter_field: Option<&'a str>,
    pub filter_op: Option<&'a str>,
    pub filter_value: Option<&'a str>,
}

#[derive(Debug, Clone)]
pub struct UploadedFile {
    pub url: String,
    pub filename: String,
    pub storage_backend: Option<String>,
    pub object_key: Option<String>,
    pub thumbnail_url: Option<String>,
    pub thumbnail_object_key: Option<String>,
}

impl OpenPrClient {
    /// Builds a client from settings the caller has already resolved.
    ///
    /// Reads nothing from the process environment: every value arrives through
    /// [`ClientConfig`], which the binary fills from the configuration file.
    pub fn new(config: ClientConfig) -> Result<Self, String> {
        let client = Client::builder()
            .timeout(std::time::Duration::from_secs(30))
            .build()
            .map_err(|e| format!("Failed to build HTTP client: {e}"))?;
        let no_redirect_client = Client::builder()
            .timeout(std::time::Duration::from_secs(30))
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .map_err(|e| format!("Failed to build HTTP client: {e}"))?;
        Ok(Self {
            client,
            base_url: config.base_url,
            credential: config.credential,
            workspace_id: config.workspace_id,
            workspace_cache: Arc::new(WorkspaceCache::new(WORKSPACE_CACHE_TTL, WORKSPACE_CACHE_CAPACITY)),
            transport_label: config.transport_label,
            operation_tool_name: None,
            flow_only: false,
            forward_credential: None,
            no_redirect_client,
        })
    }

    #[must_use]
    pub const fn flow_only(mut self) -> Self {
        self.flow_only = true;
        self
    }
    #[must_use]
    pub const fn is_flow_only(&self) -> bool {
        self.flow_only
    }
    /// The same client, speaking to the API as `credential` instead of as itself.
    ///
    /// This is how a per-request caller identity reaches every live registry tool without threading a
    /// credential through every one of them: the request scoped client *is* the identity, so
    /// a tool cannot pick a different one, and neither can the policy gate or the audit
    /// report that run around it. The HTTP connection pool is shared because [`Client`] is
    /// an `Arc` inside, and so is the workspace cache, which is keyed by token hash.
    ///
    /// The workspace is the credential's own: a configured one is deliberately not carried over,
    /// so a caller cannot act in a workspace its token does not belong to.
    #[must_use]
    pub fn acting_as(&self, credential: Secret) -> Self {
        Self {
            client: self.client.clone(),
            base_url: self.base_url.clone(),
            credential: Some(credential),
            workspace_id: None,
            workspace_cache: Arc::clone(&self.workspace_cache),
            transport_label: self.transport_label,
            operation_tool_name: self.operation_tool_name.clone(),
            flow_only: self.flow_only,
            // A legacy caller never carries the forwarding credential, so it cannot obtain the
            // delegated surface at the API.
            forward_credential: None,
            no_redirect_client: self.no_redirect_client.clone(),
        }
    }

    /// The same client, speaking to the API for a delegated (OAuth) caller.
    ///
    /// `token` is the caller's own MCP-audience access token, forwarded as `Authorization: Bearer`
    /// together with the internal forwarding credential (deviation D2 of the authorization
    /// contract). `workspace_id` is the workspace of the token's grant, taken from the token
    /// context and nowhere else: the configured `[mcp] workspace_id` plays no part. Requests go
    /// out through a client that does not follow redirects.
    #[must_use]
    pub fn acting_as_delegated(&self, token: Secret, workspace_id: &str, forward_credential: Secret) -> Self {
        Self {
            client: self.no_redirect_client.clone(),
            base_url: self.base_url.clone(),
            credential: Some(token),
            workspace_id: Some(workspace_id.to_string()),
            workspace_cache: Arc::clone(&self.workspace_cache),
            transport_label: self.transport_label,
            operation_tool_name: self.operation_tool_name.clone(),
            flow_only: self.flow_only,
            forward_credential: Some(forward_credential),
            no_redirect_client: self.no_redirect_client.clone(),
        }
    }

    /// Changes the observability surface without changing identity or authorization.
    #[must_use]
    pub fn with_transport_label(&self, transport_label: &'static str) -> Self {
        Self {
            client: self.client.clone(),
            base_url: self.base_url.clone(),
            credential: self.credential.clone(),
            workspace_id: self.workspace_id.clone(),
            workspace_cache: Arc::clone(&self.workspace_cache),
            transport_label,
            operation_tool_name: self.operation_tool_name.clone(),
            flow_only: self.flow_only,
            forward_credential: self.forward_credential.clone(),
            no_redirect_client: self.no_redirect_client.clone(),
        }
    }

    /// Binds an internal registered tool name to every API request made for that call.
    ///
    /// The API treats both headers as untrusted record labels; they never participate in auth.
    #[must_use]
    pub fn recording_operation(&self, tool_name: &str) -> Self {
        let mut scoped = self.clone();
        scoped.operation_tool_name = Some(crate::tools::entry::attribution_name(tool_name).to_string());
        scoped
    }

    /// The workspace this client's workspace-scoped calls act in.
    ///
    /// A fixed workspace is returned as is. Otherwise it is the workspace of the bot the
    /// credential belongs to, asked of `GET /api/v1/auth/me` once per token per
    /// [`WORKSPACE_CACHE_TTL`]. A credential that belongs to no bot is refused with `401`, and a
    /// failed lookup is not remembered.
    pub async fn workspace(&self) -> Result<String, String> {
        if let Some(workspace_id) = &self.workspace_id {
            return Ok(workspace_id.clone());
        }
        let credential = self
            .credential
            .as_ref()
            .ok_or_else(|| NO_OUTBOUND_CREDENTIAL.to_string())?;
        if let Some(cached) = self.workspace_cache.get(credential.expose()).await {
            return Ok(cached);
        }
        let me: Value = self.get("/api/v1/auth/me").await?;
        let workspace_id = workspace_of_me(&me)?;
        self.workspace_cache
            .put(credential.expose(), workspace_id.clone())
            .await;
        Ok(workspace_id)
    }

    /// The `Authorization` header value for one outbound request, or the refusal that stands
    /// in for a credential this client does not have.
    pub(crate) fn authorization(&self) -> Result<String, String> {
        self.credential
            .as_ref()
            .map(|credential| format!("Bearer {}", credential.expose()))
            .ok_or_else(|| NO_OUTBOUND_CREDENTIAL.to_string())
    }

    pub(crate) fn operation_headers(&self, request: RequestBuilder) -> RequestBuilder {
        let request = self.forwarding_headers(request);
        let request = request.header(MCP_SURFACE_HEADER, self.transport_label);
        match self.operation_tool_name.as_deref() {
            Some(tool_name) => request.header(MCP_TOOL_HEADER, tool_name),
            None => request,
        }
    }

    /// The forwarding headers of a delegated call; the request unchanged for every other caller.
    ///
    /// The credential value is marked sensitive so that no debug output of the request shows it.
    fn forwarding_headers(&self, request: RequestBuilder) -> RequestBuilder {
        let Some(credential) = self.forward_credential.as_ref() else {
            return request;
        };
        // The configuration validates the secret's shape; if a value that cannot be a header
        // still arrives, the request goes out without it and the API refuses the token.
        let Ok(mut value) = reqwest::header::HeaderValue::from_str(credential.expose()) else {
            tracing::error!("The forwarding credential cannot be sent as a header value; the API will refuse the call");
            return request;
        };
        value.set_sensitive(true);
        request
            .header(FORWARDED_BY_HEADER, FORWARDED_BY_VALUE)
            .header(FORWARD_AUTH_HEADER, value)
    }

    /// Sends an authenticated request and rejects both transport-level failures and
    /// API-level envelope errors before the payload reaches the caller.
    async fn send<T: DeserializeOwned>(&self, request: RequestBuilder, path: &str) -> Result<T, String> {
        let resp = self
            .operation_headers(request)
            .header("Authorization", self.authorization()?)
            .send()
            .await
            .map_err(|e| format!("Request failed: {}", e.without_url()))?;

        let status = resp.status();
        let body = resp
            .text()
            .await
            .map_err(|e| format!("Failed to read response body from {path}: {e}"))?;

        if !status.is_success() {
            return Err(rejected_request_error(status, path, &body));
        }

        let payload: Value = if body.trim().is_empty() {
            Value::Null
        } else {
            serde_json::from_str(&body).map_err(|e| format!("Failed to deserialize response from {path}: {e}"))?
        };
        check_response_envelope(&payload, path)?;

        serde_json::from_value(payload).map_err(|e| format!("Failed to deserialize response from {path}: {e}"))
    }

    pub async fn get<T: DeserializeOwned>(&self, path: &str) -> Result<T, String> {
        let url = format!("{}{path}", self.base_url);
        self.send(self.client.get(&url), path).await
    }

    pub async fn post<T: DeserializeOwned, B: Serialize + Sync>(&self, path: &str, body: &B) -> Result<T, String> {
        let url = format!("{}{path}", self.base_url);
        self.send(self.client.post(&url).json(body), path).await
    }

    pub async fn patch<T: DeserializeOwned, B: Serialize + Sync>(&self, path: &str, body: &B) -> Result<T, String> {
        let url = format!("{}{path}", self.base_url);
        self.send(self.client.patch(&url).json(body), path).await
    }

    pub async fn put<T: DeserializeOwned, B: Serialize + Sync>(&self, path: &str, body: &B) -> Result<T, String> {
        let url = format!("{}{path}", self.base_url);
        self.send(self.client.put(&url).json(body), path).await
    }

    /// Returns the parsed response envelope so callers can report what the API actually
    /// did instead of assuming success.
    pub async fn delete(&self, path: &str) -> Result<Value, String> {
        let url = format!("{}{path}", self.base_url);
        self.send(self.client.delete(&url), path).await
    }

    // ---- Projects ----

    pub async fn list_projects(&self) -> Result<Value, String> {
        self.get(&format!(
            "/api/v1/workspaces/{}/projects",
            urlencoding::encode(&self.workspace().await?)
        ))
        .await
    }

    pub async fn get_project(&self, project_id: &str) -> Result<Value, String> {
        self.get(&format!("/api/v1/projects/{}", urlencoding::encode(project_id)))
            .await
    }

    pub async fn create_project(&self, body: Value) -> Result<Value, String> {
        self.post(
            &format!(
                "/api/v1/workspaces/{}/projects",
                urlencoding::encode(&self.workspace().await?)
            ),
            &body,
        )
        .await
    }

    pub async fn update_project(&self, project_id: &str, body: Value) -> Result<Value, String> {
        self.put(&format!("/api/v1/projects/{}", urlencoding::encode(project_id)), &body)
            .await
    }

    pub async fn delete_project(&self, project_id: &str) -> Result<Value, String> {
        self.delete(&format!("/api/v1/projects/{}", urlencoding::encode(project_id)))
            .await
    }

    // ---- Project Types / Resources ----

    pub async fn list_project_types(&self) -> Result<Value, String> {
        self.get(&format!(
            "/api/v1/workspaces/{}/project-types",
            urlencoding::encode(&self.workspace().await?)
        ))
        .await
    }

    pub async fn get_project_type(&self, key: &str) -> Result<Value, String> {
        self.get(&format!("/api/v1/project-types/{}", urlencoding::encode(key)))
            .await
    }

    // PARKED(project-resources): restore with the six registrations and the resource template; see RUNNER-WORK-PACKAGES-STAGES-3-6.md P3.2
    // pub async fn list_project_resources(&self, project_id: &str) -> Result<Value, String> {
    //     self.get(&format!(
    //         "/api/v1/projects/{}/resources",
    //         urlencoding::encode(project_id)
    //     ))
    //     .await
    // }
    //
    // pub async fn create_project_resource(&self, project_id: &str, body: Value) -> Result<Value, String> {
    //     self.post(
    //         &format!("/api/v1/projects/{}/resources", urlencoding::encode(project_id)),
    //         &body,
    //     )
    //     .await
    // }
    //
    // pub async fn update_project_resource(
    //     &self,
    //     project_id: &str,
    //     resource_id: &str,
    //     body: Value,
    // ) -> Result<Value, String> {
    //     self.patch(
    //         &format!(
    //             "/api/v1/projects/{}/resources/{}",
    //             urlencoding::encode(project_id),
    //             urlencoding::encode(resource_id)
    //         ),
    //         &body,
    //     )
    //     .await
    // }
    //
    // pub async fn delete_project_resource(&self, project_id: &str, resource_id: &str) -> Result<Value, String> {
    //     self.delete(&format!(
    //         "/api/v1/projects/{}/resources/{}",
    //         urlencoding::encode(project_id),
    //         urlencoding::encode(resource_id)
    //     ))
    //     .await
    // }

    // ---- Agent tasks (`/api/v1/ai/tasks`, own bot only) ----

    pub async fn get_ai_task(&self, task_id: &str, attempt: u32) -> Result<Value, String> {
        self.get(&ai_task_path(task_id, "", attempt)).await
    }

    pub async fn ack_ai_task(&self, task_id: &str, attempt: u32, body: Value) -> Result<Value, String> {
        self.post(&ai_task_path(task_id, "/ack", attempt), &body).await
    }

    pub async fn report_ai_task_progress(&self, task_id: &str, attempt: u32, body: Value) -> Result<Value, String> {
        self.post(&ai_task_path(task_id, "/progress", attempt), &body).await
    }

    pub async fn complete_ai_task(&self, task_id: &str, attempt: u32, body: Value) -> Result<Value, String> {
        self.post(&ai_task_path(task_id, "/complete", attempt), &body).await
    }

    pub async fn fail_ai_task(&self, task_id: &str, attempt: u32, body: Value) -> Result<Value, String> {
        self.post(&ai_task_path(task_id, "/fail", attempt), &body).await
    }

    // ---- Universal Forms ----

    pub async fn list_forms(
        &self,
        project_id: &str,
        page: Option<u32>,
        per_page: Option<u32>,
    ) -> Result<Value, String> {
        let mut query = Vec::new();
        if let Some(page) = page {
            query.push(format!("page={page}"));
        }
        if let Some(per_page) = per_page {
            query.push(format!("per_page={per_page}"));
        }
        self.get(&format!(
            "/api/v1/projects/{}/forms{}",
            urlencoding::encode(project_id),
            join_query(&query)
        ))
        .await
    }

    pub async fn get_form(&self, form_id: &str) -> Result<Value, String> {
        self.get(&format!("/api/v1/forms/{}", urlencoding::encode(form_id)))
            .await
    }

    pub async fn create_form(&self, project_id: &str, body: Value) -> Result<Value, String> {
        self.post(
            &format!("/api/v1/projects/{}/forms", urlencoding::encode(project_id)),
            &body,
        )
        .await
    }

    pub async fn update_form_schema(&self, form_id: &str, body: Value) -> Result<Value, String> {
        self.patch(&format!("/api/v1/forms/{}", urlencoding::encode(form_id)), &body)
            .await
    }

    pub async fn validate_form_schema(&self, body: Value) -> Result<Value, String> {
        self.post("/api/v1/forms/schema/validate", &body).await
    }

    pub async fn validate_field_rules(&self, body: Value) -> Result<Value, String> {
        self.post("/api/v1/forms/rules/validate", &body).await
    }

    pub async fn validate_layout(&self, body: Value) -> Result<Value, String> {
        self.post("/api/v1/layouts/validate", &body).await
    }

    /// Creates a draft of an artefact; a person reviews it in the console.
    pub async fn create_draft(&self, body: Value) -> Result<Value, String> {
        self.post("/api/v1/drafts", &body).await
    }

    pub async fn duplicate_form(&self, form_id: &str, body: Value) -> Result<Value, String> {
        self.post(
            &format!("/api/v1/forms/{}/duplicate", urlencoding::encode(form_id)),
            &body,
        )
        .await
    }

    pub async fn get_form_schema_summary(&self, form_id: &str) -> Result<Value, String> {
        self.get(&format!(
            "/api/v1/forms/{}/schema-summary",
            urlencoding::encode(form_id)
        ))
        .await
    }

    pub async fn get_form_field_usage(&self, form_id: &str) -> Result<Value, String> {
        self.get(&format!("/api/v1/forms/{}/field-usage", urlencoding::encode(form_id)))
            .await
    }

    pub async fn get_form_field_dependencies(&self, form_id: &str) -> Result<Value, String> {
        self.get(&format!(
            "/api/v1/forms/{}/field-dependencies",
            urlencoding::encode(form_id)
        ))
        .await
    }

    pub async fn list_form_schema_versions(&self, form_id: &str) -> Result<Value, String> {
        self.get(&format!(
            "/api/v1/forms/{}/schema-versions",
            urlencoding::encode(form_id)
        ))
        .await
    }

    pub async fn get_form_schema_version(&self, form_id: &str, version: i32) -> Result<Value, String> {
        self.get(&format!(
            "/api/v1/forms/{}/schema-versions/{version}",
            urlencoding::encode(form_id)
        ))
        .await
    }

    pub async fn get_form_permissions(&self, form_id: &str) -> Result<Value, String> {
        self.get(&format!("/api/v1/forms/{}/permissions", urlencoding::encode(form_id)))
            .await
    }

    pub async fn update_form_permissions(&self, form_id: &str, body: Value) -> Result<Value, String> {
        self.patch(
            &format!("/api/v1/forms/{}/permissions", urlencoding::encode(form_id)),
            &body,
        )
        .await
    }

    pub async fn list_form_views(&self, form_id: &str) -> Result<Value, String> {
        self.get(&format!("/api/v1/forms/{}/views", urlencoding::encode(form_id)))
            .await
    }

    pub async fn list_form_attachments(
        &self,
        form_id: &str,
        record_id: Option<&str>,
        field_key: Option<&str>,
        include_archived: Option<bool>,
        page: Option<u32>,
        per_page: Option<u32>,
    ) -> Result<Value, String> {
        let mut query = Vec::new();
        if let Some(record_id) = record_id {
            query.push(format!("record_id={}", urlencoding::encode(record_id)));
        }
        if let Some(field_key) = field_key {
            query.push(format!("field_key={}", urlencoding::encode(field_key)));
        }
        if let Some(include_archived) = include_archived {
            query.push(format!("include_archived={include_archived}"));
        }
        if let Some(page) = page {
            query.push(format!("page={page}"));
        }
        if let Some(per_page) = per_page {
            query.push(format!("per_page={per_page}"));
        }
        let query = if query.is_empty() {
            String::new()
        } else {
            format!("?{}", query.join("&"))
        };
        self.get(&format!(
            "/api/v1/forms/{}/attachments{}",
            urlencoding::encode(form_id),
            query
        ))
        .await
    }

    pub async fn create_form_attachment(&self, form_id: &str, body: Value) -> Result<Value, String> {
        self.post(
            &format!("/api/v1/forms/{}/attachments", urlencoding::encode(form_id)),
            &body,
        )
        .await
    }

    /// Reads one attachment (archived ones too) so the MCP server can resolve which project owns
    /// it. A failed read refuses the attachment tools instead of skipping the project agent policy.
    pub async fn get_form_attachment(&self, attachment_id: &str) -> Result<Value, String> {
        self.get(&format!(
            "/api/v1/form-attachments/{}",
            urlencoding::encode(attachment_id)
        ))
        .await
    }

    pub async fn archive_form_attachment(&self, attachment_id: &str) -> Result<Value, String> {
        self.delete(&format!(
            "/api/v1/form-attachments/{}",
            urlencoding::encode(attachment_id)
        ))
        .await
    }

    pub async fn restore_form_attachment(&self, attachment_id: &str) -> Result<Value, String> {
        self.post(
            &format!(
                "/api/v1/form-attachments/{}/restore",
                urlencoding::encode(attachment_id)
            ),
            &json!({}),
        )
        .await
    }

    pub async fn list_form_records(&self, form_id: &str, query: &ListRecordsQuery<'_>) -> Result<Value, String> {
        let mut params = Vec::new();
        if let Some(page) = query.page {
            params.push(format!("page={page}"));
        }
        if let Some(per_page) = query.per_page {
            params.push(format!("per_page={per_page}"));
        }
        if let Some(view_id) = query.view_id {
            params.push(format!("view_id={}", urlencoding::encode(view_id)));
        }
        if let Some(sort) = query.sort {
            params.push(format!("sort={}", urlencoding::encode(sort)));
        }
        if let Some(filter_field) = query.filter_field {
            params.push(format!("filter_field={}", urlencoding::encode(filter_field)));
        }
        if let Some(filter_op) = query.filter_op {
            params.push(format!("filter_op={}", urlencoding::encode(filter_op)));
        }
        if let Some(filter_value) = query.filter_value {
            params.push(format!("filter_value={}", urlencoding::encode(filter_value)));
        }
        self.get(&format!(
            "/api/v1/forms/{}/records{}",
            urlencoding::encode(form_id),
            join_query(&params)
        ))
        .await
    }

    pub async fn export_form_records(
        &self,
        form_id: &str,
        view_id: Option<&str>,
        columns: Option<&str>,
        format: Option<&str>,
        scope: Option<&str>,
        include_archived: Option<bool>,
        limit: Option<i64>,
    ) -> Result<Value, String> {
        let mut query = Vec::new();
        if let Some(view_id) = view_id {
            query.push(format!("view_id={}", urlencoding::encode(view_id)));
        }
        if let Some(columns) = columns {
            query.push(format!("columns={}", urlencoding::encode(columns)));
        }
        if let Some(format) = format {
            query.push(format!("format={}", urlencoding::encode(format)));
        }
        if let Some(scope) = scope {
            query.push(format!("scope={}", urlencoding::encode(scope)));
        }
        if let Some(include_archived) = include_archived {
            query.push(format!("include_archived={include_archived}"));
        }
        if let Some(limit) = limit {
            query.push(format!("limit={limit}"));
        }
        let query = if query.is_empty() {
            String::new()
        } else {
            format!("?{}", query.join("&"))
        };
        self.get(&format!(
            "/api/v1/forms/{}/records/export{}",
            urlencoding::encode(form_id),
            query
        ))
        .await
    }

    pub async fn preview_import_form_records(&self, form_id: &str, body: Value) -> Result<Value, String> {
        self.post(
            &format!("/api/v1/forms/{}/records/import-preview", urlencoding::encode(form_id)),
            &body,
        )
        .await
    }

    pub async fn import_form_records(&self, form_id: &str, body: Value) -> Result<Value, String> {
        self.post(
            &format!("/api/v1/forms/{}/records/import", urlencoding::encode(form_id)),
            &body,
        )
        .await
    }

    pub async fn get_form_record(&self, record_id: &str) -> Result<Value, String> {
        self.get(&format!("/api/v1/form-records/{}", urlencoding::encode(record_id)))
            .await
    }

    pub async fn create_form_record(&self, form_id: &str, body: Value) -> Result<Value, String> {
        self.post(
            &format!("/api/v1/forms/{}/records", urlencoding::encode(form_id)),
            &body,
        )
        .await
    }

    pub async fn update_form_record(&self, record_id: &str, body: Value) -> Result<Value, String> {
        self.patch(
            &format!("/api/v1/form-records/{}", urlencoding::encode(record_id)),
            &body,
        )
        .await
    }

    pub async fn create_child_form_record(&self, record_id: &str, body: Value) -> Result<Value, String> {
        self.post(
            &format!("/api/v1/form-records/{}/children", urlencoding::encode(record_id)),
            &body,
        )
        .await
    }

    pub async fn update_child_form_record(
        &self,
        record_id: &str,
        child_record_id: &str,
        body: Value,
    ) -> Result<Value, String> {
        self.patch(
            &format!(
                "/api/v1/form-records/{}/children/{}",
                urlencoding::encode(record_id),
                urlencoding::encode(child_record_id)
            ),
            &body,
        )
        .await
    }

    pub async fn archive_child_form_record(
        &self,
        record_id: &str,
        child_record_id: &str,
        relation_key: Option<&str>,
    ) -> Result<Value, String> {
        let query = relation_key
            .map(|relation_key| format!("?relation_key={}", urlencoding::encode(relation_key)))
            .unwrap_or_default();
        self.delete(&format!(
            "/api/v1/form-records/{}/children/{}{}",
            urlencoding::encode(record_id),
            urlencoding::encode(child_record_id),
            query
        ))
        .await
    }

    pub async fn restore_child_form_record(
        &self,
        record_id: &str,
        child_record_id: &str,
        relation_key: Option<&str>,
    ) -> Result<Value, String> {
        let query = relation_key
            .map(|relation_key| format!("?relation_key={}", urlencoding::encode(relation_key)))
            .unwrap_or_default();
        self.post(
            &format!(
                "/api/v1/form-records/{}/children/{}/restore{}",
                urlencoding::encode(record_id),
                urlencoding::encode(child_record_id),
                query
            ),
            &json!({}),
        )
        .await
    }

    pub async fn link_form_record(&self, record_id: &str, body: Value) -> Result<Value, String> {
        self.post(
            &format!("/api/v1/form-records/{}/links", urlencoding::encode(record_id)),
            &body,
        )
        .await
    }

    pub async fn aggregate_form_records(
        &self,
        form_id: &str,
        field_key: &str,
        aggregate: &str,
    ) -> Result<Value, String> {
        self.get(&format!(
            "/api/v1/forms/{}/aggregate?field_key={}&aggregate={}",
            urlencoding::encode(form_id),
            urlencoding::encode(field_key),
            urlencoding::encode(aggregate)
        ))
        .await
    }

    pub async fn list_form_relation_targets(
        &self,
        form_id: &str,
        field_key: Option<&str>,
        form_key: Option<&str>,
        q: Option<&str>,
        page: Option<u32>,
        per_page: Option<u32>,
    ) -> Result<Value, String> {
        let mut query = Vec::new();
        if let Some(field_key) = field_key {
            query.push(format!("field_key={}", urlencoding::encode(field_key)));
        }
        if let Some(form_key) = form_key {
            query.push(format!("form_key={}", urlencoding::encode(form_key)));
        }
        if let Some(q) = q {
            query.push(format!("q={}", urlencoding::encode(q)));
        }
        if let Some(page) = page {
            query.push(format!("page={page}"));
        }
        if let Some(per_page) = per_page {
            query.push(format!("per_page={per_page}"));
        }
        let query = if query.is_empty() {
            String::new()
        } else {
            format!("?{}", query.join("&"))
        };
        self.get(&format!(
            "/api/v1/forms/{}/relation-targets{}",
            urlencoding::encode(form_id),
            query
        ))
        .await
    }

    pub async fn list_form_record_children(
        &self,
        record_id: &str,
        relation_key: Option<&str>,
        child_form_key: Option<&str>,
        page: Option<u32>,
        per_page: Option<u32>,
    ) -> Result<Value, String> {
        let mut query = Vec::new();
        if let Some(relation_key) = relation_key {
            query.push(format!("relation_key={}", urlencoding::encode(relation_key)));
        }
        if let Some(child_form_key) = child_form_key {
            query.push(format!("child_form_key={}", urlencoding::encode(child_form_key)));
        }
        if let Some(page) = page {
            query.push(format!("page={page}"));
        }
        if let Some(per_page) = per_page {
            query.push(format!("per_page={per_page}"));
        }
        let query = if query.is_empty() {
            String::new()
        } else {
            format!("?{}", query.join("&"))
        };
        self.get(&format!(
            "/api/v1/form-records/{}/children{}",
            urlencoding::encode(record_id),
            query
        ))
        .await
    }

    pub async fn list_form_events(
        &self,
        form_id: &str,
        event_type: Option<&str>,
        page: Option<u32>,
        per_page: Option<u32>,
    ) -> Result<Value, String> {
        self.get(&format!(
            "/api/v1/forms/{}/events{}",
            urlencoding::encode(form_id),
            join_query(&event_query_params(event_type, page, per_page))
        ))
        .await
    }

    pub async fn list_form_record_events(
        &self,
        record_id: &str,
        event_type: Option<&str>,
        page: Option<u32>,
        per_page: Option<u32>,
    ) -> Result<Value, String> {
        self.get(&format!(
            "/api/v1/form-records/{}/events{}",
            urlencoding::encode(record_id),
            join_query(&event_query_params(event_type, page, per_page))
        ))
        .await
    }

    // ---- Plugins ----

    pub async fn list_plugins(
        &self,
        project_id: &str,
        page: Option<u32>,
        per_page: Option<u32>,
    ) -> Result<Value, String> {
        let mut params = Vec::new();
        if let Some(page) = page {
            params.push(format!("page={page}"));
        }
        if let Some(per_page) = per_page {
            params.push(format!("per_page={per_page}"));
        }
        self.get(&format!(
            "/api/v1/projects/{}/plugins{}",
            urlencoding::encode(project_id),
            join_query(&params)
        ))
        .await
    }

    pub async fn get_plugin(&self, plugin_id: &str) -> Result<Value, String> {
        self.get(&format!("/api/v1/plugins/{}", urlencoding::encode(plugin_id)))
            .await
    }

    pub async fn install_plugin(&self, project_id: &str, body: Value) -> Result<Value, String> {
        self.post(
            &format!("/api/v1/projects/{}/plugins", urlencoding::encode(project_id)),
            &body,
        )
        .await
    }

    pub async fn invoke_plugin(&self, plugin_id: &str, body: Value) -> Result<Value, String> {
        self.post(
            &format!("/api/v1/plugins/{}/invoke", urlencoding::encode(plugin_id)),
            &body,
        )
        .await
    }

    pub async fn list_plugin_invocations(
        &self,
        plugin_id: &str,
        page: Option<u32>,
        per_page: Option<u32>,
    ) -> Result<Value, String> {
        let mut params = Vec::new();
        if let Some(page) = page {
            params.push(format!("page={page}"));
        }
        if let Some(per_page) = per_page {
            params.push(format!("per_page={per_page}"));
        }
        self.get(&format!(
            "/api/v1/plugins/{}/invocations{}",
            urlencoding::encode(plugin_id),
            join_query(&params)
        ))
        .await
    }

    // ---- Project Context ----

    pub async fn get_project_context(&self, project_id: &str) -> Result<Value, String> {
        self.get(&format!("/api/v1/projects/{}/context", urlencoding::encode(project_id)))
            .await
    }

    pub async fn get_project_governance_context(&self, project_id: &str) -> Result<Value, String> {
        self.get(&format!(
            "/api/v1/projects/{}/governance-context",
            urlencoding::encode(project_id)
        ))
        .await
    }

    pub async fn get_project_agent_policy(&self, project_id: &str) -> Result<Value, String> {
        self.get(&format!(
            "/api/v1/projects/{}/agent-policy",
            urlencoding::encode(project_id)
        ))
        .await
    }

    pub async fn get_project_release_readiness(&self, project_id: &str) -> Result<Value, String> {
        self.get(&format!(
            "/api/v1/projects/{}/release-readiness",
            urlencoding::encode(project_id)
        ))
        .await
    }

    // ---- Work Items / Issues ----

    pub async fn list_work_items(&self, project_id: &str, query: &ListWorkItemsQuery<'_>) -> Result<Value, String> {
        let mut params = Vec::new();
        if let Some(page) = query.page {
            params.push(format!("page={page}"));
        }
        if let Some(per_page) = query.per_page {
            params.push(format!("per_page={per_page}"));
        }
        if let Some(state) = query.state {
            params.push(format!("state={}", urlencoding::encode(state)));
        }
        if let Some(assignee_id) = query.assignee_id {
            params.push(format!("assignee_id={}", urlencoding::encode(assignee_id)));
        }
        if let Some(priority) = query.priority {
            params.push(format!("priority={}", urlencoding::encode(priority)));
        }
        if let Some(sprint_id) = query.sprint_id {
            params.push(format!("sprint_id={}", urlencoding::encode(sprint_id)));
        }
        if let Some(search) = query.search {
            params.push(format!("search={}", urlencoding::encode(search)));
        }
        if let Some(label_ids) = query.label_ids {
            params.push(format!("label_ids={}", urlencoding::encode(label_ids)));
        }
        if let Some(sort_by) = query.sort_by {
            params.push(format!("sort_by={}", urlencoding::encode(sort_by)));
        }
        if let Some(sort_order) = query.sort_order {
            params.push(format!("sort_order={}", urlencoding::encode(sort_order)));
        }
        if let Some(milestone_id) = query.milestone_id {
            params.push(format!("milestone_id={}", urlencoding::encode(milestone_id)));
        }
        self.get(&format!(
            "/api/v1/projects/{}/issues{}",
            urlencoding::encode(project_id),
            join_query(&params)
        ))
        .await
    }

    pub async fn get_work_item(&self, work_item_id: &str) -> Result<Value, String> {
        self.get(&format!("/api/v1/issues/{}", urlencoding::encode(work_item_id)))
            .await
    }

    pub async fn create_work_item(&self, project_id: &str, body: Value) -> Result<Value, String> {
        self.post(
            &format!("/api/v1/projects/{}/issues", urlencoding::encode(project_id)),
            &body,
        )
        .await
    }

    pub async fn update_work_item(&self, work_item_id: &str, body: Value) -> Result<Value, String> {
        self.put(&format!("/api/v1/issues/{}", urlencoding::encode(work_item_id)), &body)
            .await
    }

    pub async fn delete_work_item(&self, work_item_id: &str) -> Result<Value, String> {
        self.delete(&format!("/api/v1/issues/{}", urlencoding::encode(work_item_id)))
            .await
    }

    /// Searches the workspace. `search_type` maps to the backend `type` filter
    /// (`issue` / `project` / `comment`), `limit` caps the returned rows.
    pub async fn search(&self, query: &str, search_type: Option<&str>, limit: Option<u32>) -> Result<Value, String> {
        let mut params = vec![format!("q={}", urlencoding::encode(query))];
        if let Some(search_type) = search_type {
            params.push(format!("type={}", urlencoding::encode(search_type)));
        }
        if let Some(limit) = limit {
            params.push(format!("limit={limit}"));
        }
        self.get(&format!("/api/v1/search{}", join_query(&params))).await
    }

    pub async fn add_label_to_issue(&self, issue_id: &str, label_id: &str) -> Result<Value, String> {
        self.post(
            &format!(
                "/api/v1/issues/{}/labels/{}",
                urlencoding::encode(issue_id),
                urlencoding::encode(label_id)
            ),
            &serde_json::json!({}),
        )
        .await
    }

    pub async fn remove_label_from_issue(&self, issue_id: &str, label_id: &str) -> Result<Value, String> {
        self.delete(&format!(
            "/api/v1/issues/{}/labels/{}",
            urlencoding::encode(issue_id),
            urlencoding::encode(label_id)
        ))
        .await
    }

    pub async fn get_issue_labels(&self, issue_id: &str) -> Result<Value, String> {
        self.get(&format!("/api/v1/issues/{}/labels", urlencoding::encode(issue_id)))
            .await
    }

    // ---- Work item <-> Flow page links ----

    pub async fn list_issue_pages(&self, issue_id: &str) -> Result<Value, String> {
        self.get(&format!("/api/v1/issues/{}/pages", urlencoding::encode(issue_id)))
            .await
    }

    pub async fn link_issue_page(&self, issue_id: &str, object_id: &str) -> Result<Value, String> {
        self.post(
            &format!("/api/v1/issues/{}/pages", urlencoding::encode(issue_id)),
            &serde_json::json!({ "object_id": object_id }),
        )
        .await
    }

    pub async fn unlink_issue_page(&self, issue_id: &str, object_id: &str) -> Result<Value, String> {
        self.delete(&format!(
            "/api/v1/issues/{}/pages/{}",
            urlencoding::encode(issue_id),
            urlencoding::encode(object_id)
        ))
        .await
    }

    // ---- Comments ----

    pub async fn list_comments(
        &self,
        issue_id: &str,
        page: Option<u32>,
        per_page: Option<u32>,
    ) -> Result<Value, String> {
        let mut query = Vec::new();
        if let Some(page) = page {
            query.push(format!("page={page}"));
        }
        if let Some(per_page) = per_page {
            query.push(format!("per_page={per_page}"));
        }
        let query = if query.is_empty() {
            String::new()
        } else {
            format!("?{}", query.join("&"))
        };
        self.get(&format!(
            "/api/v1/issues/{}/comments{query}",
            urlencoding::encode(issue_id)
        ))
        .await
    }

    pub async fn create_comment(&self, issue_id: &str, body: Value) -> Result<Value, String> {
        self.post(
            &format!("/api/v1/issues/{}/comments", urlencoding::encode(issue_id)),
            &body,
        )
        .await
    }

    pub async fn get_comment(&self, comment_id: &str) -> Result<Value, String> {
        self.get(&format!("/api/v1/comments/{}", urlencoding::encode(comment_id)))
            .await
    }

    pub async fn delete_comment(&self, comment_id: &str) -> Result<Value, String> {
        self.delete(&format!("/api/v1/comments/{}", urlencoding::encode(comment_id)))
            .await
    }

    // ---- Uploads ----

    pub async fn upload_file(&self, filename: &str, content: Vec<u8>) -> Result<UploadedFile, String> {
        let url = format!("{}/api/v1/upload", self.base_url);
        let part = multipart::Part::bytes(content)
            .file_name(filename.to_string())
            .mime_str(detect_mime_type_from_filename(filename))
            .map_err(|e| format!("Failed to build multipart part: {e}"))?;
        let form = multipart::Form::new().part("file", part);

        let resp = self
            .operation_headers(self.client.post(&url))
            .header("Authorization", self.authorization()?)
            .multipart(form)
            .send()
            .await
            .map_err(|e| format!("Request failed: {e}"))?;

        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            return Err(rejected_request_error(status, "/api/v1/upload", &body));
        }

        let payload: Value = resp
            .json()
            .await
            .map_err(|e| format!("Failed to deserialize response: {e}"))?;

        check_response_envelope(&payload, "/api/v1/upload")?;

        let data = payload
            .get("data")
            .ok_or_else(|| "Missing data in /api/v1/upload response".to_string())?;

        let url = data
            .get("url")
            .and_then(Value::as_str)
            .ok_or_else(|| "Missing data.url in /api/v1/upload response".to_string())?
            .to_string();
        let filename = data
            .get("filename")
            .and_then(Value::as_str)
            .ok_or_else(|| "Missing data.filename in /api/v1/upload response".to_string())?
            .to_string();
        let storage_backend = data.get("storage_backend").and_then(Value::as_str).map(str::to_string);
        let object_key = data.get("object_key").and_then(Value::as_str).map(str::to_string);
        let thumbnail_url = data.get("thumbnail_url").and_then(Value::as_str).map(str::to_string);
        let thumbnail_object_key = data
            .get("thumbnail_object_key")
            .and_then(Value::as_str)
            .map(str::to_string);

        Ok(UploadedFile {
            url,
            filename,
            storage_backend,
            object_key,
            thumbnail_url,
            thumbnail_object_key,
        })
    }

    // ---- Proposals ----

    /// The backend proposal list is workspace-wide: `apps/api/src/routes/proposal.rs`
    /// `ListProposalsQuery` has no project filter, so no `project_id` is sent.
    pub async fn list_proposals(&self, query: &ListProposalsQuery<'_>) -> Result<Value, String> {
        let mut params = Vec::new();
        if let Some(status) = query.status {
            params.push(format!("status={}", urlencoding::encode(status)));
        }
        if let Some(proposal_type) = query.proposal_type {
            params.push(format!("proposal_type={}", urlencoding::encode(proposal_type)));
        }
        if let Some(domain) = query.domain {
            params.push(format!("domain={}", urlencoding::encode(domain)));
        }
        if let Some(page) = query.page {
            params.push(format!("page={page}"));
        }
        if let Some(per_page) = query.per_page {
            params.push(format!("per_page={per_page}"));
        }
        if let Some(sort) = query.sort {
            params.push(format!("sort={}", urlencoding::encode(sort)));
        }
        self.get(&format!("/api/v1/proposals{}", join_query(&params))).await
    }

    pub async fn get_proposal(&self, proposal_id: &str) -> Result<Value, String> {
        self.get(&format!("/api/v1/proposals/{}", urlencoding::encode(proposal_id)))
            .await
    }

    pub async fn create_proposal(&self, body: Value) -> Result<Value, String> {
        self.post("/api/v1/proposals", &body).await
    }

    /// Reads one check result. Used to resolve the project that owns a check result
    /// before `proposals.create_from_result` is authorized against a project policy.
    pub async fn get_check_result(&self, check_result_id: &str) -> Result<Value, String> {
        self.get(&format!(
            "/api/v1/check-results/{}",
            urlencoding::encode(check_result_id)
        ))
        .await
    }

    pub async fn create_check_result(&self, project_id: &str, body: Value) -> Result<Value, String> {
        self.post(
            &format!("/api/v1/projects/{}/check-results", urlencoding::encode(project_id)),
            &body,
        )
        .await
    }

    pub async fn create_proposal_from_result(&self, check_result_id: &str, body: Value) -> Result<Value, String> {
        self.post(
            &format!(
                "/api/v1/check-results/{}/proposal",
                urlencoding::encode(check_result_id)
            ),
            &body,
        )
        .await
    }

    // ---- Members ----

    pub async fn list_members(&self) -> Result<Value, String> {
        self.get(&format!(
            "/api/v1/workspaces/{}/members",
            urlencoding::encode(&self.workspace().await?)
        ))
        .await
    }

    pub async fn list_bot_operation_logs(&self, query: &str) -> Result<Value, String> {
        self.get(&format!(
            "/api/v1/workspaces/{}/bot-operation-logs{query}",
            urlencoding::encode(&self.workspace().await?)
        ))
        .await
    }

    /// `GET /api/v1/workspaces/{workspace_id}/bots` — real, already-shipped, admin-only
    /// surface (`apps/api/src/routes/bot.rs`'s `list_bots`: "only workspace owners and admins
    /// can list bots"). `server.rs`'s `enforce_flow_admin_capability` calls this as a
    /// genuine, independent pre-flight probe for the "Flow admin capability" leg of
    /// `WorkspaceWide(admin)` (`mcp-surface-v1.md:17`) ahead of a Flow admin write — there is
    /// no Flow-specific admin capability distinct from generic workspace admin in today's
    /// data model (see that function's doc comment), so this is the closest real,
    /// already-admin-gated endpoint rather than a fabricated check. Takes `workspace_id` as
    /// an explicit argument, deliberately never `self.workspace_id`, because the caller's own
    /// declared workspace — not this process's configured default — is what must be checked
    /// (`mcp-surface-v1.md`: "不得从配置默认 workspace...推导 admin scope").
    pub async fn list_workspace_bots(&self, workspace_id: &str) -> Result<Value, String> {
        self.get(&format!(
            "/api/v1/workspaces/{}/bots",
            urlencoding::encode(workspace_id)
        ))
        .await
    }

    // ---- Sprints ----

    pub async fn create_sprint(&self, project_id: &str, body: Value) -> Result<Value, String> {
        self.post(
            &format!("/api/v1/projects/{}/sprints", urlencoding::encode(project_id)),
            &body,
        )
        .await
    }

    pub async fn list_sprints(&self, project_id: &str) -> Result<Value, String> {
        self.get(&format!("/api/v1/projects/{}/sprints", urlencoding::encode(project_id)))
            .await
    }

    pub async fn get_sprint(&self, sprint_id: &str) -> Result<Value, String> {
        self.get(&format!("/api/v1/sprints/{}", urlencoding::encode(sprint_id)))
            .await
    }

    pub async fn update_sprint(&self, sprint_id: &str, body: Value) -> Result<Value, String> {
        self.put(&format!("/api/v1/sprints/{}", urlencoding::encode(sprint_id)), &body)
            .await
    }

    pub async fn delete_sprint(&self, sprint_id: &str) -> Result<Value, String> {
        self.delete(&format!("/api/v1/sprints/{}", urlencoding::encode(sprint_id)))
            .await
    }

    // ---- Milestones ----

    pub async fn list_milestones(&self, query: &ListMilestonesQuery<'_>) -> Result<Value, String> {
        let mut params = Vec::new();
        if let Some(status) = query.status {
            params.push(format!("status={}", urlencoding::encode(status)));
        }
        if let Some(page) = query.page {
            params.push(format!("page={page}"));
        }
        if let Some(per_page) = query.per_page {
            params.push(format!("per_page={per_page}"));
        }
        self.get(&format!(
            "/api/v1/workspaces/{}/milestones{}",
            urlencoding::encode(&self.workspace().await?),
            join_query(&params)
        ))
        .await
    }

    pub async fn get_milestone(&self, milestone_id: &str) -> Result<Value, String> {
        self.get(&format!("/api/v1/milestones/{}", urlencoding::encode(milestone_id)))
            .await
    }

    pub async fn create_milestone(&self, body: Value) -> Result<Value, String> {
        self.post(
            &format!(
                "/api/v1/workspaces/{}/milestones",
                urlencoding::encode(&self.workspace().await?)
            ),
            &body,
        )
        .await
    }

    pub async fn update_milestone(&self, milestone_id: &str, body: Value) -> Result<Value, String> {
        self.put(
            &format!("/api/v1/milestones/{}", urlencoding::encode(milestone_id)),
            &body,
        )
        .await
    }

    pub async fn delete_milestone(&self, milestone_id: &str) -> Result<Value, String> {
        self.delete(&format!("/api/v1/milestones/{}", urlencoding::encode(milestone_id)))
            .await
    }

    // ---- Labels ----

    pub async fn create_label(&self, body: Value) -> Result<Value, String> {
        self.post(
            &format!(
                "/api/v1/workspaces/{}/labels",
                urlencoding::encode(&self.workspace().await?)
            ),
            &body,
        )
        .await
    }

    pub async fn list_labels(&self) -> Result<Value, String> {
        self.get(&format!(
            "/api/v1/workspaces/{}/labels",
            urlencoding::encode(&self.workspace().await?)
        ))
        .await
    }

    pub async fn list_project_labels(&self, project_id: &str) -> Result<Value, String> {
        self.get(&format!("/api/v1/projects/{}/labels", urlencoding::encode(project_id)))
            .await
    }

    pub async fn get_work_item_by_identifier(&self, identifier: &str) -> Result<Value, String> {
        self.get(&format!(
            "/api/v1/issues/by-identifier/{}",
            urlencoding::encode(identifier)
        ))
        .await
    }

    pub async fn add_labels_to_issue(&self, issue_id: &str, label_ids: &[String]) -> Result<Value, String> {
        self.post(
            &format!("/api/v1/issues/{}/labels/batch", urlencoding::encode(issue_id)),
            &serde_json::json!({ "label_ids": label_ids }),
        )
        .await
    }

    pub async fn update_label(&self, label_id: &str, body: Value) -> Result<Value, String> {
        self.put(&format!("/api/v1/labels/{}", urlencoding::encode(label_id)), &body)
            .await
    }

    pub async fn delete_label(&self, label_id: &str) -> Result<Value, String> {
        self.delete(&format!("/api/v1/labels/{}", urlencoding::encode(label_id)))
            .await
    }

    // ---- Flow ----
    //
    // Every path here mirrors `rest-api-v1.md` ("v0.4 Flow Alpha") verbatim; none of it is
    // invented. `get_flow_object`/`list_flow_objects`/`get_flow_object_history` address the
    // three read endpoints `apps/api/src/routes/flow.rs` shipped first; `.../collab` and
    // `.../collab/verify` (called by `cli_app::dispatch` directly through `get`/`post` rather
    // than a named method here — see that module's doc comment) landed in
    // `apps/api/src/routes/collab.rs` while this change was being built, and both were
    // exercised end to end against a live `apps/api` + PostgreSQL instance with the real
    // `sylvode` binary while developing this file. `get_flow_feature`/`set_flow_feature`
    // (`GET|PUT /workspaces/{workspace_id}/features/flow`) are the one v0.4 pair `apps/api`
    // does not implement yet at all; calling them against a real deployment answers 404 until
    // it does. Written against the frozen contract regardless, so the MCP tools and `sylvode`
    // CLI commands built on top do not need a second change once the endpoint exists.

    pub async fn get_flow_object(&self, object_id: &str, query: &str) -> Result<Value, String> {
        self.get(&format!(
            "/api/v2/flow/objects/{}{query}",
            urlencoding::encode(object_id)
        ))
        .await
    }

    /// The Flow scope bound to a workspace; the workspace routes of the Flow surface hang off it.
    pub async fn flow_scope_id(&self, workspace_id: &str) -> Result<String, String> {
        let path = format!("/api/v2/flow/workspaces/{}/scope", urlencoding::encode(workspace_id));
        let envelope: Value = self.get(&path).await?;
        envelope
            .pointer("/data/scope/id")
            .and_then(Value::as_str)
            .map(str::to_owned)
            .ok_or_else(|| format!("Malformed response from {path}: no scope id"))
    }

    pub async fn list_flow_objects(&self, workspace_id: &str, query: &str) -> Result<Value, String> {
        let scope = self.flow_scope_id(workspace_id).await?;
        self.get(&format!(
            "/api/v2/flow/scopes/{}/objects{query}",
            urlencoding::encode(&scope)
        ))
        .await
    }

    pub async fn get_flow_object_history(&self, object_id: &str, query: &str) -> Result<Value, String> {
        self.get(&format!(
            "/api/v2/flow/objects/{}/history{query}",
            urlencoding::encode(object_id)
        ))
        .await
    }

    pub async fn get_flow_feature(&self, workspace_id: &str) -> Result<Value, String> {
        self.get(&format!(
            "/api/v1/workspaces/{}/features/flow",
            urlencoding::encode(workspace_id)
        ))
        .await
    }

    pub async fn list_flow_comments(&self, object_id: &str) -> Result<Value, String> {
        self.get(&format!(
            "/api/v2/flow/objects/{}/comments",
            urlencoding::encode(object_id)
        ))
        .await
    }

    pub async fn create_flow_comment(&self, object_id: &str, body: &Value) -> Result<Value, String> {
        self.post(
            &format!("/api/v2/flow/objects/{}/comments", urlencoding::encode(object_id)),
            body,
        )
        .await
    }

    pub async fn update_flow_comment(&self, object_id: &str, comment_id: &str, body: &Value) -> Result<Value, String> {
        self.patch(
            &format!(
                "/api/v2/flow/objects/{}/comments/{}",
                urlencoding::encode(object_id),
                urlencoding::encode(comment_id)
            ),
            body,
        )
        .await
    }

    pub async fn delete_flow_comment(&self, object_id: &str, comment_id: &str) -> Result<Value, String> {
        self.delete(&format!(
            "/api/v2/flow/objects/{}/comments/{}",
            urlencoding::encode(object_id),
            urlencoding::encode(comment_id)
        ))
        .await
    }

    pub async fn resolve_flow_comment(&self, object_id: &str, comment_id: &str, body: &Value) -> Result<Value, String> {
        self.post(
            &format!(
                "/api/v2/flow/objects/{}/comments/{}/resolve",
                urlencoding::encode(object_id),
                urlencoding::encode(comment_id)
            ),
            body,
        )
        .await
    }

    pub async fn list_flow_share_links(&self, object_id: &str) -> Result<Value, String> {
        self.get(&format!(
            "/api/v2/flow/objects/{}/share-links",
            urlencoding::encode(object_id)
        ))
        .await
    }

    pub async fn create_flow_share_link(&self, object_id: &str, body: &Value) -> Result<Value, String> {
        self.post(
            &format!("/api/v2/flow/objects/{}/share-links", urlencoding::encode(object_id)),
            body,
        )
        .await
    }

    pub async fn set_flow_appearance(&self, object_id: &str, body: &Value) -> Result<Value, String> {
        self.put(
            &format!("/api/v2/flow/objects/{}/appearance", urlencoding::encode(object_id)),
            body,
        )
        .await
    }

    pub async fn upload_flow_appearance(&self, object_id: &str, body: &Value) -> Result<Value, String> {
        self.post(
            &format!(
                "/api/v2/flow/objects/{}/appearance/assets",
                urlencoding::encode(object_id)
            ),
            body,
        )
        .await
    }

    pub async fn restore_flow_version(&self, object_id: &str, body: &Value) -> Result<Value, String> {
        self.post(
            &format!(
                "/api/v2/flow/objects/{}/history/restore",
                urlencoding::encode(object_id)
            ),
            body,
        )
        .await
    }

    pub async fn list_flow_trash(
        &self,
        workspace_id: &str,
        project_id: Option<&str>,
        cursor: Option<&str>,
        limit: Option<u64>,
    ) -> Result<Value, String> {
        let mut pairs = Vec::new();
        if let Some(project_id) = project_id {
            pairs.push(format!("project_id={}", urlencoding::encode(project_id)));
        }
        if let Some(cursor) = cursor {
            pairs.push(format!("cursor={}", urlencoding::encode(cursor)));
        }
        if let Some(limit) = limit {
            pairs.push(format!("limit={limit}"));
        }
        let query = if pairs.is_empty() {
            String::new()
        } else {
            format!("?{}", pairs.join("&"))
        };
        let scope = self.flow_scope_id(workspace_id).await?;
        self.get(&format!(
            "/api/v2/flow/scopes/{}/trash{query}",
            urlencoding::encode(&scope)
        ))
        .await
    }

    pub async fn purge_flow_object(&self, object_id: &str) -> Result<Value, String> {
        self.post(
            &format!("/api/v2/flow/objects/{}/purge", urlencoding::encode(object_id)),
            &json!({}),
        )
        .await
    }

    pub async fn list_flow_mention_candidates(&self, object_id: &str, query: Option<&str>) -> Result<Value, String> {
        let suffix = query
            .filter(|value| !value.is_empty())
            .map(|value| format!("?q={}", urlencoding::encode(value)))
            .unwrap_or_default();
        self.get(&format!(
            "/api/v2/flow/objects/{}/mention-candidates{suffix}",
            urlencoding::encode(object_id)
        ))
        .await
    }

    pub async fn revoke_flow_share_link(&self, object_id: &str, link_id: &str) -> Result<Value, String> {
        self.post(
            &format!(
                "/api/v2/flow/objects/{}/share-links/{}/revoke",
                urlencoding::encode(object_id),
                urlencoding::encode(link_id)
            ),
            &json!({}),
        )
        .await
    }

    pub async fn set_flow_feature(&self, workspace_id: &str, body: Value) -> Result<Value, String> {
        self.put(
            &format!("/api/v1/workspaces/{}/features/flow", urlencoding::encode(workspace_id)),
            &body,
        )
        .await
    }

    // `collab inspect`/`collab verify` (`sylvode collab inspect|verify`) are the two v0.4
    // Flow surfaces with no MCP tool at all (`surface-coverage-v1.md` maps both REST endpoints
    // to `tool:objects.integrity@0.8`, a later version this change does not register), so
    // `cli_app::dispatch` is their only caller and it is reached solely through the `sylvode`
    // binary, which links this file through the `mcp_server` library rather than through
    // `mcp-server`'s own private module copy (`main.rs`'s `mod client;`) — a distinction that
    // matters here because a `pub` method a *library* crate never calls internally is still
    // legitimate public API and draws no dead-code warning, but the exact same method sitting
    // unused in a *binary* crate's own private copy of this file does. Rather than add two
    // named wrapper methods that would be genuinely dead code in that second, binary-only
    // compilation, `cli_app::dispatch` calls the generic `get`/`post` below directly for these
    // two endpoints — which are already exercised, in both compiled copies, by every other
    // domain method in this `impl` block.
}

fn detect_mime_type_from_filename(filename: &str) -> &'static str {
    use std::path::Path;
    let path = Path::new(filename);
    let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");

    // Special case: .tar.gz is a compound extension not handled by Path::extension
    if filename.to_ascii_lowercase().ends_with(".tar.gz") {
        return "application/gzip";
    }

    if ext.eq_ignore_ascii_case("png") {
        "image/png"
    } else if ext.eq_ignore_ascii_case("jpg") || ext.eq_ignore_ascii_case("jpeg") {
        "image/jpeg"
    } else if ext.eq_ignore_ascii_case("gif") {
        "image/gif"
    } else if ext.eq_ignore_ascii_case("webp") {
        "image/webp"
    } else if ext.eq_ignore_ascii_case("mp4") {
        "video/mp4"
    } else if ext.eq_ignore_ascii_case("webm") {
        "video/webm"
    } else if ext.eq_ignore_ascii_case("mov") {
        "video/quicktime"
    } else if ext.eq_ignore_ascii_case("avi") {
        "video/x-msvideo"
    } else if ext.eq_ignore_ascii_case("zip") {
        "application/zip"
    } else if ext.eq_ignore_ascii_case("gz") {
        "application/gzip"
    } else if ext.eq_ignore_ascii_case("log") || ext.eq_ignore_ascii_case("txt") {
        "text/plain"
    } else if ext.eq_ignore_ascii_case("pdf") {
        "application/pdf"
    } else if ext.eq_ignore_ascii_case("json") {
        "application/json"
    } else if ext.eq_ignore_ascii_case("csv") {
        "text/csv"
    } else if ext.eq_ignore_ascii_case("xml") {
        "application/xml"
    } else {
        "application/octet-stream"
    }
}

#[cfg(test)]
pub mod test_api {
    use super::{ClientConfig, OpenPrClient, TRANSPORT_LABEL_STDIO};
    use axum::Router;
    use platform::config::Secret;
    use std::error::Error;

    /// A client pointed at a throwaway API, with the settings every in-crate test shares.
    pub fn client(base_url: String) -> Result<OpenPrClient, String> {
        OpenPrClient::new(ClientConfig {
            base_url,
            credential: Some(Secret::new("opr_test_token")),
            workspace_id: Some("workspace-1".to_string()),
            transport_label: TRANSPORT_LABEL_STDIO,
        })
    }

    /// Starts a throwaway API server bound to an ephemeral port and returns its base URL.
    pub async fn spawn(router: Router) -> Result<String, Box<dyn Error>> {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
        let addr = listener.local_addr()?;
        tokio::spawn(async move {
            let _ = axum::serve(listener, router).await;
        });
        Ok(format!("http://{addr}"))
    }
}

#[cfg(test)]
mod tests {
    use super::{ListRecordsQuery, OpenPrClient, check_response_envelope, rejected_request_error, test_api};
    use axum::{
        Json, Router,
        extract::RawQuery,
        http::HeaderMap,
        routing::{delete, get},
    };
    use serde_json::{Value, json};
    use std::error::Error;

    fn client(base_url: String) -> Result<OpenPrClient, String> {
        test_api::client(base_url)
    }

    #[test]
    fn envelope_with_success_code_is_accepted() {
        assert!(check_response_envelope(&json!({ "code": 0, "data": {} }), "/p").is_ok());
    }

    /// The previous default accepted every 200 that simply had no `code` field, which
    /// made an empty body, a `204`, an HTML error page or a response fetched through a
    /// smuggled path look like a successful API call.
    #[test]
    fn payloads_that_are_not_api_envelopes_are_rejected() {
        for payload in [
            json!({ "items": [] }),
            json!({ "code": "0" }),
            json!(null),
            json!([]),
            json!("ok"),
        ] {
            let error = check_response_envelope(&payload, "/p").err().unwrap_or_default();
            assert!(error.contains("Malformed response from /p"), "{payload} -> {error}");
        }
    }

    #[test]
    fn envelope_with_error_code_is_rejected_with_api_message() {
        let error = check_response_envelope(&json!({ "code": 404, "message": "form not found" }), "/p")
            .err()
            .unwrap_or_default();

        assert!(error.contains("404"), "{error}");
        assert!(error.contains("form not found"), "{error}");
    }

    /// The one status whose message is not relayed. A caller that failed to authenticate has
    /// not been identified at all, so it is told the verdict and how to act on it, and nothing
    /// the backend said.
    #[test]
    fn a_rejected_credential_is_reported_without_the_backends_own_words() {
        let error = check_response_envelope(
            &json!({ "code": 401, "message": "looked it up in pg-primary-7.internal:5432" }),
            "/p",
        )
        .err()
        .unwrap_or_default();

        assert!(error.contains("401"), "{error}");
        assert!(error.contains("not expired"), "the refusal is not actionable: {error}");
        assert!(
            error.contains("the Sylvode API rejected the credential"),
            "the refusal names the product: {error}"
        );
        assert!(
            !error.contains("pg-primary-7.internal"),
            "the refusal relayed the backend's message: {error}"
        );
    }

    /// A `403` is an authenticated caller being refused a specific action, so its message is
    /// the only explanation that caller will ever get and is relayed in full.
    #[test]
    fn an_authenticated_caller_still_learns_why_it_was_refused() {
        let error = check_response_envelope(&json!({ "code": 403, "message": "policy denied" }), "/p")
            .err()
            .unwrap_or_default();
        assert!(error.contains("403") && error.contains("policy denied"), "{error}");
    }

    /// The same rule, applied to a real HTTP status — which, since the API answers 200 with an
    /// envelope, means something in front of it answered, with a body written for nobody here.
    #[test]
    fn a_scoped_bot_refusal_says_that_retrying_will_not_help() {
        let refusal = json!({"code": 403, "message": "this bot may not do this in this project", "data": null});
        let text = check_response_envelope(&refusal, "/api/v1/issues/1")
            .err()
            .unwrap_or_default();
        assert!(text.contains("this bot may not do this in this project"), "{text}");
        assert!(text.contains("will not help"), "{text}");
        // Any other 403 keeps the API's own words.
        let other = json!({"code": 403, "message": "not a member of this workspace", "data": null});
        assert_eq!(
            check_response_envelope(&other, "/x"),
            Err("API error 403 from /x: not a member of this workspace".to_string())
        );
    }

    #[test]
    fn a_refusal_from_in_front_of_the_api_is_reported_without_its_body() {
        let error = rejected_request_error(
            reqwest::StatusCode::UNAUTHORIZED,
            "/p",
            "<html>nginx/1.27 upstream api-7.internal</html>",
        );
        assert!(error.contains("401"), "{error}");
        assert!(!error.contains("api-7.internal"), "{error}");

        // Every other status keeps its body: it is about the caller's own request.
        let relayed = rejected_request_error(reqwest::StatusCode::BAD_GATEWAY, "/p", "upstream timed out");
        assert!(relayed.contains("upstream timed out"), "{relayed}");
    }

    #[tokio::test]
    async fn http_200_error_envelopes_become_errors_on_every_verb() -> Result<(), Box<dyn Error>> {
        async fn error_envelope() -> Json<Value> {
            Json(json!({ "code": 404, "message": "form not found", "data": null }))
        }
        let router = Router::new()
            .route("/api/v1/forms/f1/records", get(error_envelope).post(error_envelope))
            .route("/api/v1/form-attachments/a1", delete(error_envelope));
        let base_url = test_api::spawn(router).await?;
        let client = client(base_url)?;

        let listed = client.list_form_records("f1", &ListRecordsQuery::default()).await;
        let created = client.create_form_record("f1", json!({ "values": {} })).await;
        let archived = client.archive_form_attachment("a1").await;

        for result in [listed, created, archived] {
            let error = result.err().unwrap_or_default();
            assert!(error.contains("API error 404"), "{error}");
            assert!(error.contains("form not found"), "{error}");
        }
        Ok(())
    }

    #[tokio::test]
    async fn successful_envelope_is_returned_unchanged() -> Result<(), Box<dyn Error>> {
        let router = Router::new().route(
            "/api/v1/forms/f1/records",
            get(|| async { Json(json!({ "code": 0, "data": { "items": [], "total": 0 } })) }),
        );
        let base_url = test_api::spawn(router).await?;
        let client = client(base_url)?;

        let value = client.list_form_records("f1", &ListRecordsQuery::default()).await?;

        assert_eq!(value.get("code").and_then(Value::as_i64), Some(0));
        Ok(())
    }

    #[tokio::test]
    async fn operation_headers_are_attached_without_changing_authorization() -> Result<(), Box<dyn Error>> {
        let router = Router::new().route(
            "/record",
            get(|headers: HeaderMap| async move {
                let all = |name: &str| -> Vec<String> {
                    headers
                        .get_all(name)
                        .iter()
                        .filter_map(|value| value.to_str().ok().map(str::to_string))
                        .collect()
                };
                Json(json!({
                    "code": 0,
                    "data": {
                        "tool": all("x-sylvode-mcp-tool"),
                        "surface": all("x-sylvode-mcp-surface"),
                        "authorization": headers.get("authorization").and_then(|value| value.to_str().ok())
                    }
                }))
            }),
        );
        let base_url = test_api::spawn(router).await?;
        let client = client(base_url)?
            .with_transport_label(super::TRANSPORT_LABEL_CLI)
            .recording_operation("bot_operation_logs.list");

        let value: Value = client.get("/record").await?;
        let data = value.get("data").and_then(Value::as_object).ok_or("missing data")?;

        // ADR-0020 D3: the attribution headers, each exactly once.
        assert_eq!(data.get("tool"), Some(&json!(["bot_operation_logs.list"])));
        assert_eq!(data.get("surface"), Some(&json!(["cli"])));
        assert_eq!(
            data.get("authorization").and_then(Value::as_str),
            Some("Bearer opr_test_token")
        );
        Ok(())
    }

    #[tokio::test]
    async fn list_form_records_forwards_every_query_parameter() -> Result<(), Box<dyn Error>> {
        let router = Router::new().route(
            "/api/v1/forms/f1/records",
            get(|RawQuery(query): RawQuery| async move {
                Json(json!({ "code": 0, "data": { "query": query.unwrap_or_default() } }))
            }),
        );
        let base_url = test_api::spawn(router).await?;
        let client = client(base_url)?;

        let value = client
            .list_form_records(
                "f1",
                &ListRecordsQuery {
                    page: Some(3),
                    per_page: Some(200),
                    view_id: Some("v1"),
                    sort: Some("amount:desc"),
                    filter_field: Some("amount"),
                    filter_op: Some("gte"),
                    filter_value: Some("10000.00"),
                },
            )
            .await?;
        let query = value
            .get("data")
            .and_then(|data| data.get("query"))
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();

        for expected in [
            "page=3",
            "per_page=200",
            "view_id=v1",
            "sort=amount%3Adesc",
            "filter_field=amount",
            "filter_op=gte",
            "filter_value=10000.00",
        ] {
            assert!(query.contains(expected), "missing {expected} in {query}");
        }
        Ok(())
    }

    #[tokio::test]
    async fn transport_failures_still_surface_as_errors() -> Result<(), Box<dyn Error>> {
        let client = client("http://127.0.0.1:1".to_string())?;

        let error = client.get_form("f1").await.err().unwrap_or_default();

        assert!(error.contains("Request failed"), "{error}");
        Ok(())
    }
}

// Simple URL encoding helper (avoid extra dep)
mod urlencoding {
    use std::fmt::Write as _;

    pub fn encode(s: &str) -> String {
        let mut result = String::with_capacity(s.len());
        for c in s.chars() {
            match c {
                'A'..='Z' | 'a'..='z' | '0'..='9' | '-' | '_' | '.' | '~' => result.push(c),
                _ => {
                    for byte in c.to_string().as_bytes() {
                        let _ = write!(result, "%{byte:02X}");
                    }
                }
            }
        }
        result
    }
}
