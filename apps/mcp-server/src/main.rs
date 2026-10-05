//! `mcp-server` — the Sylvode MCP server, plus the workspace command groups it shares with
//! `sylvode` (`mcp_server::cli`).

use axum::{
    Extension, Json, Router,
    extract::{Query, Request, State},
    http::{StatusCode, header},
    middleware::{self, Next},
    response::{
        IntoResponse, Response,
        sse::{Event, KeepAlive, Sse},
    },
    routing::{get, post},
};
use clap::{CommandFactory, FromArgMatches};
use mcp_server::cli::{self, Cli, Commands};
use mcp_server::client::OpenPrClient;
use mcp_server::protocol::{self, JsonRpcRequest, JsonRpcResponse};
use mcp_server::server::McpServer;
use platform::config::{McpRuntime, McpTransport, Secret};
use serde::Deserialize;
use serde_json::json;
use std::{collections::HashMap, convert::Infallible, sync::Arc};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::sync::{Mutex, mpsc};
use tokio_stream::{StreamExt, wrappers::UnboundedReceiverStream};
use uuid::Uuid;

/// Longest inbound caller bot token accepted, in bytes.
///
/// A bound on a value that is copied into an outbound header and held for the life of one
/// request. Comfortably above any real `OpenPR` bot token, which is tens of bytes.
const MAX_CALLER_TOKEN_LEN: usize = 8 * 1024;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let mut matches = Cli::command().get_matches();
    // Read before the conversion below, which moves the subcommand out of `matches`.
    let subcommand = cli::subcommand_path(&matches);
    let cli = Cli::from_arg_matches_mut(&mut matches)
        .map_err(|error| error.format(&mut Cli::command()))
        .unwrap_or_else(|error| error.exit());

    match &cli.command {
        // The executable name is the stable service label of a deployment, so `serve` on any
        // transport stays silent (ADR-0020 D2).
        Commands::Serve(args) => serve(&cli::prepare_runtime(&cli.global, Some(args))?).await,
        Commands::Business(command) => {
            cli::report_legacy_invocation(&subcommand);
            cli::run_business(&cli.global, command).await
        }
    }
}

/// Starts the transport `[mcp]` selected.
///
/// The two shapes of deployment differ in exactly one thing — where the bot token each API
/// call is made with comes from — and in nothing else:
///
/// * `stdio` is a pipe pair owned by the parent process. It has no per-request headers, so
///   the identity is `mcp.bot_token`: one person runs one process from their own MCP client,
///   configured with their own bot. That is already per-account.
/// * `http` and `sse` are shared listeners, where a single configured bot would make every
///   caller indistinguishable. Every request carries its own caller's bot token in
///   `Authorization: Bearer` and is served as that bot; the process itself holds no
///   identity, which is why [`cli::build_client`] is handed `None` here.
///
/// Both paths then do the same thing with the token they hold: forward it to the API
/// verbatim and let the API authenticate it. Neither one parses or validates its contents.
async fn serve(mcp: &McpRuntime) -> anyhow::Result<()> {
    match mcp.transport {
        McpTransport::Stdio => run_stdio(cli::build_client(mcp, Some(cli::configured_bot_token(mcp)?))?).await,
        McpTransport::Http => run_http(&mcp.bind_addr, cli::build_client(mcp, None)?).await,
        McpTransport::Sse => run_sse(&mcp.bind_addr, cli::build_client(mcp, None)?).await,
    }
}

/// The bot token one inbound HTTP/SSE request presented, on its way to the API.
///
/// Placed in the request extensions by [`require_caller_token`] and taken back out by the
/// handler. It exists as its own type so that the handler cannot be written without it: the
/// extractor for a missing extension rejects the request, so the failure mode of forgetting
/// the middleware is a refusal, not an anonymous call.
#[derive(Clone)]
struct CallerToken(Secret);

/// The token out of an `Authorization: Bearer <token>` header, if it is well formed.
fn bearer_token(header_value: &str) -> Option<&str> {
    let (scheme, token) = header_value.split_once(' ')?;
    if !scheme.eq_ignore_ascii_case("bearer") {
        return None;
    }
    let token = token.trim();
    if token.is_empty() { None } else { Some(token) }
}

/// The caller's bot token, if the request carries a usable one.
///
/// `to_str` limits the value to visible ASCII, which is also what makes it safe to copy into
/// an outbound header. The length bound is the only judgement passed on the contents:
/// whether this token names a real, enabled bot with rights to anything is the API's
/// question, and a verdict reached here would be a second, weaker authenticator sitting in
/// front of the real one.
fn caller_token(headers: &header::HeaderMap) -> Option<Secret> {
    headers
        .get(header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .and_then(bearer_token)
        .filter(|token| token.len() <= MAX_CALLER_TOKEN_LEN)
        .map(Secret::new)
}

/// Rejects any inbound request that does not carry a caller bot token, and hands the one it
/// does carry to the handler.
///
/// This is the whole inbound authentication story. There is no shared secret, no
/// configuration that relaxes it and no anonymous path: a networked MCP request is made as
/// somebody, or it is not made.
async fn require_caller_token(mut request: Request, next: Next) -> Response {
    let Some(token) = caller_token(request.headers()) else {
        // The rejection names no header value, on either the too-long or the absent branch.
        tracing::warn!(
            path = %request.uri().path(),
            "Rejected an inbound MCP request that carried no usable caller bot token"
        );
        return (
            StatusCode::UNAUTHORIZED,
            [(header::WWW_AUTHENTICATE, "Bearer")],
            Json(json!({
                "error": "Missing Authorization: Bearer <opr_ bot token>. This MCP server acts as its caller, so \
                          every request must present the caller's own workspace bot token."
            })),
        )
            .into_response();
    };

    request.extensions_mut().insert(CallerToken(token));
    next.run(request).await
}

/// Wraps the data carrying routes in the caller token layer.
///
/// `/health` stays outside it on purpose: it answers a constant `OK`, reads nothing and
/// discloses nothing a caller who already completed the TCP handshake does not know, and
/// orchestrator probes have no bot account to authenticate as.
fn serve_router(state: SseState, protected: Router<SseState>) -> Router {
    Router::new()
        .merge(protected.route_layer(middleware::from_fn(require_caller_token)))
        .route("/health", get(health_check))
        .with_state(state)
}

async fn run_http(bind_addr: &str, client: OpenPrClient) -> anyhow::Result<()> {
    let state = SseState {
        client,
        sessions: Arc::new(Mutex::new(HashMap::new())),
    };

    let protected = Router::new()
        .route("/mcp/rpc", post(handle_jsonrpc))
        .route("/sse", get(handle_sse_connect))
        .route("/messages", post(handle_sse_message));
    let app = serve_router(state, protected);

    let listener = tokio::net::TcpListener::bind(bind_addr).await?;
    tracing::info!(
        bind_addr = %bind_addr,
        inbound_auth = INBOUND_AUTH_DESCRIPTION,
        "MCP HTTP transport started (JSON-RPC + SSE)"
    );
    axum::serve(listener, app).await?;
    Ok(())
}

/// How the networked transports report their inbound authentication in the startup log.
///
/// A constant rather than a computed label because there is now only one answer; a log line
/// that could say something else would imply a setting that no longer exists.
const INBOUND_AUTH_DESCRIPTION: &str = "per-request caller bot token (Authorization: Bearer), required";

async fn handle_jsonrpc(
    State(state): State<SseState>,
    Extension(caller): Extension<CallerToken>,
    Json(req): Json<JsonRpcRequest>,
) -> impl IntoResponse {
    let server = McpServer::new(state.client.acting_as(caller.0));
    server.handle_request(req).await.map_or_else(
        || (StatusCode::ACCEPTED, Json(json!({"status": "accepted"}))),
        |response| (StatusCode::OK, Json(json!(response))),
    )
}

#[derive(Clone)]
struct SseState {
    client: OpenPrClient,
    sessions: Arc<Mutex<HashMap<String, mpsc::UnboundedSender<SseServerEvent>>>>,
}

#[derive(Debug)]
enum SseServerEvent {
    Endpoint(String),
    Message(String),
}

struct SessionGuard {
    session_id: String,
    sessions: Arc<Mutex<HashMap<String, mpsc::UnboundedSender<SseServerEvent>>>>,
}

impl Drop for SessionGuard {
    fn drop(&mut self) {
        let session_id = self.session_id.clone();
        let sessions = self.sessions.clone();
        tokio::spawn(async move {
            sessions.lock().await.remove(&session_id);
        });
    }
}

#[derive(Debug, Deserialize)]
struct MessagesQuery {
    session_id: String,
}

async fn run_sse(bind_addr: &str, client: OpenPrClient) -> anyhow::Result<()> {
    let state = SseState {
        client,
        sessions: Arc::new(Mutex::new(HashMap::new())),
    };

    let protected = Router::new()
        .route("/sse", get(handle_sse_connect))
        .route("/messages", post(handle_sse_message));
    let app = serve_router(state, protected);

    let listener = tokio::net::TcpListener::bind(bind_addr).await?;
    tracing::info!(
        bind_addr = %bind_addr,
        inbound_auth = INBOUND_AUTH_DESCRIPTION,
        "MCP SSE transport started"
    );
    axum::serve(listener, app).await?;
    Ok(())
}

async fn handle_sse_connect(
    State(state): State<SseState>,
) -> Sse<impl tokio_stream::Stream<Item = Result<Event, Infallible>>> {
    let session_id = Uuid::new_v4().to_string();
    let endpoint = format!("/messages?session_id={session_id}");
    let (tx, rx) = mpsc::unbounded_channel::<SseServerEvent>();

    state.sessions.lock().await.insert(session_id.clone(), tx.clone());

    let _ = tx.send(SseServerEvent::Endpoint(endpoint));

    let session_guard = SessionGuard {
        session_id,
        sessions: state.sessions.clone(),
    };

    let stream = UnboundedReceiverStream::new(rx).map(move |msg| {
        let _keep_guard_alive = &session_guard;
        let event = match msg {
            SseServerEvent::Endpoint(url) => Event::default().event("endpoint").data(url),
            SseServerEvent::Message(payload) => Event::default().event("message").data(payload),
        };
        Ok::<Event, Infallible>(event)
    });

    Sse::new(stream).keep_alive(KeepAlive::default())
}

/// The SSE session is only a delivery channel: the identity a request is served as comes
/// from the `POST /messages` that carried it, never from the `GET /sse` that opened the
/// stream. Binding identity to the session would let one caller's messages ride another
/// caller's stream, and would make the identity outlive the request that proved it.
async fn handle_sse_message(
    State(state): State<SseState>,
    Extension(caller): Extension<CallerToken>,
    Query(query): Query<MessagesQuery>,
    Json(req): Json<JsonRpcRequest>,
) -> impl IntoResponse {
    let sender = state.sessions.lock().await.get(&query.session_id).cloned();
    let Some(sender) = sender else {
        return (StatusCode::NOT_FOUND, Json(json!({"error": "Unknown SSE session_id"})));
    };

    // Asked before the call runs, not after it: `handle_request` performs the tool's side
    // effect, so running one whose answer has nowhere to go leaves the caller holding a
    // refusal for work that was in fact done — and a refusal is exactly what makes a client
    // retry, which performs the effect a second time.
    if sender.is_closed() {
        state.sessions.lock().await.remove(&query.session_id);
        return (StatusCode::GONE, Json(json!({"error": "SSE session is closed"})));
    }

    // Kept for the log below, because `handle_request` consumes the request. A method name
    // is a protocol constant, never a credential or an argument.
    let method = req.method.clone();
    let server = McpServer::new(state.client.acting_as(caller.0));
    let response = server.handle_request(req).await;
    let Some(response) = response else {
        return (StatusCode::ACCEPTED, Json(json!({"status": "accepted"})));
    };

    let response_json = match serde_json::to_string(&response) {
        Ok(v) => v,
        Err(e) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({"error": format!("Failed to serialize response: {e}")})),
            );
        }
    };

    if sender.send(SseServerEvent::Message(response_json)).is_err() {
        // The check above narrows this window but cannot close it: the client may drop its
        // stream at any instant *while* the call is running, and nothing in this process can
        // hold that connection open or take back a side effect the API has already applied.
        // What is left is the honest report of it — the call ran, and its result died here.
        // Logged rather than dropped silently, so "done but never acknowledged" is something
        // an operator can find when a caller retries and the effect appears twice.
        tracing::warn!(
            session_id = %query.session_id,
            method = %method,
            "SSE stream closed while its message was in flight: the call ran, but its result could not be delivered"
        );
        state.sessions.lock().await.remove(&query.session_id);
        return (StatusCode::GONE, Json(json!({"error": "SSE session is closed"})));
    }

    (StatusCode::ACCEPTED, Json(json!({"status": "accepted"})))
}

async fn health_check() -> impl IntoResponse {
    (StatusCode::OK, "OK")
}

#[cfg(test)]
mod tests {
    use super::{MAX_CALLER_TOKEN_LEN, bearer_token, caller_token, header};
    use platform::config::Secret;

    const TOKEN: &str = "opr_caller_bot_token_example";

    #[test]
    fn bearer_parsing_accepts_only_a_well_formed_bearer_header() {
        assert_eq!(bearer_token("Bearer abc"), Some("abc"));
        assert_eq!(bearer_token("bearer  abc  "), Some("abc"));
        assert_eq!(bearer_token("BEARER abc"), Some("abc"));
        assert_eq!(bearer_token("Basic abc"), None);
        assert_eq!(bearer_token("Bearer "), None);
        assert_eq!(bearer_token("abc"), None);
    }

    /// Builds the header map an inbound request would arrive with.
    fn headers(pairs: &[(header::HeaderName, &str)]) -> header::HeaderMap {
        let mut map = header::HeaderMap::new();
        for (name, value) in pairs {
            match header::HeaderValue::from_str(value) {
                Ok(value) => {
                    map.insert(name.clone(), value);
                }
                Err(error) => panic!("test header value is not a valid header: {error}"),
            }
        }
        map
    }

    /// The token the caller presented is the token that is carried onward, byte for byte.
    /// Trimming, re-casing or otherwise "cleaning" it would hand the API a credential the
    /// caller never sent.
    #[test]
    fn the_callers_token_is_taken_verbatim_from_the_authorization_header() {
        let extracted = caller_token(&headers(&[(header::AUTHORIZATION, &format!("Bearer {TOKEN}"))]));
        assert_eq!(extracted.as_ref().map(Secret::expose), Some(TOKEN));
    }

    /// Every way of not presenting a bot token has to come out the same: no identity, which
    /// the middleware turns into a 401. A `None` here that became "act as the server" is
    /// exactly the bug this change exists to remove.
    #[test]
    fn a_request_without_a_usable_bearer_token_yields_no_identity() {
        let too_long = "opr_".to_string() + &"x".repeat(MAX_CALLER_TOKEN_LEN);
        for map in [
            headers(&[]),
            headers(&[(header::AUTHORIZATION, "")]),
            headers(&[(header::AUTHORIZATION, "Bearer")]),
            headers(&[(header::AUTHORIZATION, "Bearer   ")]),
            headers(&[(header::AUTHORIZATION, &format!("Basic {TOKEN}"))]),
            headers(&[(header::AUTHORIZATION, TOKEN)]),
            // A different header is not the Authorization header, however suggestive.
            headers(&[(header::PROXY_AUTHORIZATION, &format!("Bearer {TOKEN}"))]),
            headers(&[(header::AUTHORIZATION, &format!("Bearer {too_long}"))]),
        ] {
            assert!(
                caller_token(&map).is_none(),
                "a request with {map:?} was given an identity"
            );
        }
    }

    /// The bound is on the token, not on the whole header, and a token at the limit is
    /// still served: a cap that rejected legitimate credentials would be an availability
    /// bug wearing a security hat.
    #[test]
    fn a_token_at_the_length_limit_is_still_accepted() {
        let at_limit = "x".repeat(MAX_CALLER_TOKEN_LEN);
        let extracted = caller_token(&headers(&[(header::AUTHORIZATION, &format!("Bearer {at_limit}"))]));
        assert_eq!(extracted.as_ref().map(Secret::expose), Some(at_limit.as_str()));
    }
}

#[cfg(test)]
mod sse_delivery_tests {
    use super::{CallerToken, MessagesQuery, SseServerEvent, SseState, handle_sse_message};
    use axum::{
        Extension, Json,
        extract::{Query, State},
        http::StatusCode,
        response::IntoResponse,
    };
    use mcp_server::client::{ClientConfig, OpenPrClient};
    use mcp_server::protocol::JsonRpcRequest;
    use platform::config::Secret;
    use serde_json::json;
    use std::collections::HashMap;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::sync::{Mutex, mpsc};

    type TestResult = Result<(), Box<dyn std::error::Error>>;

    const CALLER_TOKEN: &str = "opr_caller_bot_token_example";
    const WORKSPACE: &str = "11111111-1111-4111-8111-111111111111";

    /// A counting stand-in for the `OpenPR` API.
    ///
    /// One accepted connection is one outbound call this process made, which is what "the
    /// tool ran" looks like from outside it. Asserting on the count rather than on the
    /// answer is the point: a tool that ran and whose result was then thrown away leaves no
    /// trace in this process at all, only at the API it already called.
    async fn counting_api() -> Result<(String, Arc<AtomicUsize>), Box<dyn std::error::Error>> {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
        let addr = listener.local_addr()?;
        let calls = Arc::new(AtomicUsize::new(0));
        let counter = Arc::clone(&calls);

        tokio::spawn(async move {
            const BODY: &[u8] = br#"{"code":0,"message":"ok","data":[]}"#;
            while let Ok((mut socket, _)) = listener.accept().await {
                counter.fetch_add(1, Ordering::SeqCst);
                let mut scratch = [0_u8; 2048];
                let _ = socket.read(&mut scratch).await;
                let head = format!(
                    "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n",
                    BODY.len()
                );
                let _ = socket.write_all(head.as_bytes()).await;
                let _ = socket.write_all(BODY).await;
                let _ = socket.shutdown().await;
            }
        });

        Ok((format!("http://{addr}"), calls))
    }

    /// A message posted onto a session whose stream is gone is refused *before* its tool runs.
    ///
    /// The state is built here rather than reached through a real client, because a client
    /// that drops its connection has its receiver dropped and its registry entry reaped
    /// within microseconds of each other: end to end, the message lands on the "unknown
    /// session" branch and never exercises this one. Registered sender, dead receiver is
    /// nonetheless the state the ordering is about — it is what every in-flight message
    /// passes through — and running the call in it performs the tool's side effect and then
    /// answers `410`, which tells the caller nothing happened when something did, and invites
    /// the retry that does it twice.
    #[tokio::test]
    async fn a_message_for_a_stream_that_is_gone_is_refused_before_its_tool_runs() -> TestResult {
        let (api_url, calls) = counting_api().await?;
        let client = OpenPrClient::new(ClientConfig {
            base_url: api_url,
            credential: None,
            workspace_id: WORKSPACE.to_string(),
            transport_label: "sse",
        })
        .map_err(|error| -> Box<dyn std::error::Error> { error.into() })?;

        let (tx, rx) = mpsc::unbounded_channel::<SseServerEvent>();
        drop(rx);
        let session_id = "5f6a1a3c-0f4d-4a2e-9a1b-6d2f5c8e7b40".to_string();
        let mut registry = HashMap::new();
        registry.insert(session_id.clone(), tx);
        let sessions = Arc::new(Mutex::new(registry));

        let state = SseState {
            client,
            sessions: Arc::clone(&sessions),
        };
        let refusal = handle_sse_message(
            State(state),
            Extension(CallerToken(Secret::new(CALLER_TOKEN))),
            Query(MessagesQuery {
                session_id: session_id.clone(),
            }),
            Json(JsonRpcRequest {
                jsonrpc: "2.0".to_string(),
                id: Some(json!(1)),
                method: "tools/call".to_string(),
                params: Some(json!({ "name": "projects.list", "arguments": {} })),
            }),
        )
        .await
        .into_response();

        assert_eq!(
            refusal.status(),
            StatusCode::GONE,
            "a message for a stream that cannot receive its answer was not refused"
        );
        assert_eq!(
            calls.load(Ordering::SeqCst),
            0,
            "the tool ran for a stream that could not be told what it did"
        );
        assert!(
            sessions.lock().await.is_empty(),
            "the dead session was left in the registry to be found again"
        );
        Ok(())
    }
}

/// Whether the line is a Content-Length or Content-Type header (case-insensitive).
fn is_stdio_header_line(line: &str) -> bool {
    let lower = line.to_ascii_lowercase();
    lower.starts_with("content-length:") || lower.starts_with("content-type:")
}

/// Parse content-length value from a header line.
fn parse_content_length(line: &str) -> Option<usize> {
    let lower = line.to_ascii_lowercase();
    if lower.starts_with("content-length:") {
        line[15..].trim().parse::<usize>().ok()
    } else {
        None
    }
}

/// Whether we used Content-Length framing or line-delimited.
#[derive(Copy, Clone)]
enum StdioFrame {
    LineDelimited,
    ContentLength,
}

async fn write_stdio_response(
    stdout: &mut tokio::io::Stdout,
    response_json: &str,
    frame: StdioFrame,
) -> anyhow::Result<()> {
    match frame {
        StdioFrame::LineDelimited => {
            stdout.write_all(response_json.as_bytes()).await?;
            stdout.write_all(b"\n").await?;
        }
        StdioFrame::ContentLength => {
            let len = response_json.len();
            let header = format!("Content-Length: {len}\r\n\r\n");
            stdout.write_all(header.as_bytes()).await?;
            stdout.write_all(response_json.as_bytes()).await?;
        }
    }
    stdout.flush().await?;
    Ok(())
}

/// Read HTTP-style headers until empty line; return the Content-Length value (0 if absent).
async fn read_headers_get_content_length(reader: &mut BufReader<tokio::io::Stdin>, first_line: &str) -> usize {
    let mut content_length = parse_content_length(first_line);
    loop {
        let mut header_line = String::new();
        match reader.read_line(&mut header_line).await {
            Ok(0) | Err(_) => break,
            Ok(_) => {
                let ht = header_line.trim();
                if ht.is_empty() {
                    break;
                }
                content_length = content_length.or_else(|| parse_content_length(ht));
            }
        }
    }
    content_length.unwrap_or(0)
}

async fn run_stdio(client: OpenPrClient) -> anyhow::Result<()> {
    tracing::info!("MCP stdio transport started");

    let server = McpServer::new(client);
    let stdin = tokio::io::stdin();
    let mut stdout = tokio::io::stdout();
    let mut reader = BufReader::new(stdin);

    loop {
        let mut line = String::new();
        match reader.read_line(&mut line).await {
            Ok(0) => {
                tracing::info!("stdin closed, shutting down");
                break;
            }
            Ok(_) => {
                let trimmed = line.trim();
                if trimmed.is_empty() {
                    continue;
                }

                // Detect Content-Length framing (used by Codex, Claude Desktop)
                let (payload, frame) = if is_stdio_header_line(trimmed) {
                    let cl = read_headers_get_content_length(&mut reader, trimmed).await;
                    if cl == 0 {
                        continue;
                    }
                    let mut body = vec![0u8; cl];
                    if let Err(e) = reader.read_exact(&mut body).await {
                        tracing::error!(error = %e, "Failed to read Content-Length body");
                        continue;
                    }
                    (body, StdioFrame::ContentLength)
                } else {
                    // Line-delimited JSON
                    (trimmed.as_bytes().to_vec(), StdioFrame::LineDelimited)
                };

                let request: JsonRpcRequest = match serde_json::from_slice(&payload) {
                    Ok(req) => req,
                    Err(e) => {
                        tracing::error!(error = %e, "Failed to parse request");
                        let error_response = JsonRpcResponse::error(
                            None,
                            protocol::JsonRpcError::parse_error(format!("Invalid JSON: {e}")),
                        );
                        if let Ok(rj) = serde_json::to_string(&error_response) {
                            let _ = write_stdio_response(&mut stdout, &rj, frame).await;
                        }
                        continue;
                    }
                };

                tracing::debug!(method = %request.method, "Received request");

                let response = server.handle_request(request).await;
                let Some(response) = response else {
                    continue;
                };

                match serde_json::to_string(&response) {
                    Ok(response_json) => {
                        if let Err(e) = write_stdio_response(&mut stdout, &response_json, frame).await {
                            tracing::error!(error = %e, "Failed to write response");
                        }
                    }
                    Err(e) => {
                        tracing::error!(error = %e, "Failed to serialize response");
                    }
                }
            }
            Err(e) => {
                tracing::error!(error = %e, "Failed to read from stdin");
                break;
            }
        }
    }

    Ok(())
}
