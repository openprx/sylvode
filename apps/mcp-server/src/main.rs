//! `mcp-server` — the Sylvode MCP server, plus the workspace command groups it shares with
//! `sylvode` (`mcp_server::cli`).

use axum::{
    Extension, Json, Router,
    extract::{Query, RawQuery, State},
    http::{HeaderMap, StatusCode},
    middleware,
    response::{
        IntoResponse, Response,
        sse::{Event, KeepAlive, Sse},
    },
    routing::{get, post},
};
use clap::{CommandFactory, Parser};
use mcp_server::cli::{self, Cli, Commands};
use mcp_server::client::OpenPrClient;
use mcp_server::era::{self, Era};
use mcp_server::http_modern;
use mcp_server::listen::{ListenAuth, ListenConfig, ListenRegistry, STALL_LIMIT, caller_key};
use mcp_server::oauth::{self, Caller, InboundAuth};
use mcp_server::protocol::{self, JsonRpcRequest, JsonRpcResponse};
use mcp_server::server::{McpServer, ToolSurface, requested_tool_mode};
use platform::config::{McpRuntime, McpToolMode, McpTransport};
use serde::Deserialize;
use serde_json::json;
use std::{collections::HashMap, convert::Infallible, sync::Arc, time::Duration};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::sync::{Mutex, mpsc, watch};
use tokio_stream::{
    StreamExt,
    wrappers::{ReceiverStream, UnboundedReceiverStream},
};
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

/// What the networked transports add to the JSON-RPC endpoint: the `Origin` allow list and the
/// registry of listen streams.
#[derive(Clone)]
struct NetOptions {
    allowed_origins: Arc<Vec<String>>,
    listen: ListenRegistry,
}

impl NetOptions {
    fn from_runtime(mcp: &McpRuntime) -> Self {
        Self {
            allowed_origins: Arc::new(mcp.allowed_origins.clone()),
            listen: ListenRegistry::new(ListenConfig::new(
                mcp.listen,
                usize::try_from(mcp.max_listen_streams).unwrap_or(usize::MAX),
            )),
        }
    }
}

impl Default for NetOptions {
    /// No allowed origins and no listen: what Flow-only serves with (it declares no push).
    fn default() -> Self {
        Self {
            allowed_origins: Arc::new(Vec::new()),
            listen: ListenRegistry::new(ListenConfig::new(false, ListenConfig::default().max_streams)),
        }
    }
}

/// The address of the peer that opened a connection, put on every request of that connection. It is
/// the socket's address, never a forwarding header: behind a proxy it is the proxy's, which only
/// makes the per-address bound coarser, never bypassable.
#[derive(Clone, Copy)]
struct PeerAddr(std::net::IpAddr);

/// How long a client may take to send the head of a request before its connection is closed.
const HEADER_READ_TIMEOUT: Duration = Duration::from_secs(10);

/// Longest time `serve` keeps answering the requests in flight after SIGTERM/SIGINT.
///
/// Below the 10 s a container runtime (Docker, Podman, compose) waits between SIGTERM and
/// SIGKILL, so the drain finishes, and the process exits 0, before the runtime gives up on it.
const SHUTDOWN_DRAIN_BOUND: Duration = Duration::from_secs(8);

/// How long the runtime may wait for blocking work when the process exits.
///
/// The stdio transport reads stdin on a blocking thread whose read cannot be cancelled; while the
/// parent keeps the pipe open that read never returns, and an unbounded runtime shutdown would
/// wait on it forever after the server has already decided to stop.
const RUNTIME_SHUTDOWN_GRACE: Duration = Duration::from_millis(250);

fn main() -> anyhow::Result<()> {
    let runtime = tokio::runtime::Builder::new_multi_thread().enable_all().build()?;
    let result = runtime.block_on(run());
    runtime.shutdown_timeout(RUNTIME_SHUTDOWN_GRACE);
    result
}

async fn run() -> anyhow::Result<()> {
    let cli = Cli::parse();

    if cli.check {
        if cli.command.is_some() {
            Cli::command()
                .error(
                    clap::error::ErrorKind::ArgumentConflict,
                    "--check cannot be combined with a subcommand",
                )
                .exit();
        }
        print_report(&mcp_server::doctor::check_report(&cli.global), true)?;
        return Ok(());
    }
    let Some(command) = &cli.command else {
        Cli::command()
            .error(
                clap::error::ErrorKind::MissingSubcommand,
                "a subcommand or --check is required",
            )
            .exit();
    };

    match command {
        Commands::Doctor(args) => {
            let options = mcp_server::doctor::DoctorOptions {
                clis: args.clis.iter().copied().map(Into::into).collect(),
                as_service: args.as_service,
            };
            let report = mcp_server::doctor::doctor_report(&cli.global, &options).await;
            print_report(&report, args.json)?;
            // Exit 0: no fail; 2: at least one fail. A checker that could not run returned an
            // error above, which exits 1.
            let code = report.exit_code();
            if code != 0 {
                std::process::exit(code);
            }
            Ok(())
        }
        Commands::Serve(args) if args.flow_only => {
            if args.tools.is_some() {
                anyhow::bail!("--tools is not valid with serve --flow-only: Flow-only lists its own bounded tools");
            }
            let mode = args.transport.map(McpTransport::from);
            let settings = mcp_server::flow_runtime::FlowRuntime::load(
                cli.global.config.as_deref(),
                cli.global.api_url.as_deref(),
                args.access_token.as_ref(),
                args.access_token_file.as_deref(),
                mode,
                args.bind_addr.as_deref(),
            )
            .map_err(anyhow::Error::msg)?;
            let client = settings.client().map_err(anyhow::Error::msg)?;
            let shutdown = Shutdown::install()?;
            // Flow-only lists its own definitions; the surface below is never consulted.
            let surface = ToolSurface::new(McpToolMode::default(), true);
            match settings.transport {
                McpTransport::Stdio => {
                    run_stdio(
                        client,
                        surface,
                        ListenRegistry::new(ListenConfig::new(false, ListenConfig::default().per_key_limit)),
                        shutdown,
                    )
                    .await
                }
                // Flow is not delegated in this release: `sva_` tokens are refused with 401.
                McpTransport::Http => {
                    run_http(
                        &settings.bind_addr,
                        client,
                        Arc::new(InboundAuth::legacy_only()),
                        surface,
                        NetOptions::default(),
                        shutdown,
                    )
                    .await
                }
                McpTransport::Sse => {
                    run_sse(
                        &settings.bind_addr,
                        client,
                        Arc::new(InboundAuth::legacy_only()),
                        surface,
                        NetOptions::default(),
                        shutdown,
                    )
                    .await
                }
            }
        }
        Commands::Serve(args) => {
            if args.access_token.is_some() || args.access_token_file.is_some() {
                anyhow::bail!("--access-token is only valid with serve --flow-only");
            }
            serve(&cli::prepare_runtime(&cli.global, Some(args))?).await
        }
    }
}

/// Prints a doctor report, as JSON or as the text table, and flushes it.
fn print_report(report: &sylvode_prereq::Report, json: bool) -> anyhow::Result<()> {
    use std::io::Write as _;
    let text = if json { report.to_json()? } else { report.to_table() };
    let mut out = std::io::stdout().lock();
    writeln!(out, "{}", text.trim_end())?;
    out.flush()?;
    Ok(())
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
///
/// Every transport stops on SIGTERM or SIGINT with exit code 0 once the requests in flight are
/// answered, or after [`SHUTDOWN_DRAIN_BOUND`] at the latest (see [`Shutdown`]).
async fn serve(mcp: &McpRuntime) -> anyhow::Result<()> {
    let shutdown = Shutdown::install()?;
    let surface = ToolSurface::new(mcp.tool_mode, mcp.allow_write_call).with_listen(mcp.listen);
    match mcp.transport {
        McpTransport::Stdio => {
            run_stdio(
                cli::build_client(mcp, Some(cli::configured_bot_token(mcp)?))?,
                surface,
                // The stdio pipe holds up to one credential's worth of streams.
                ListenRegistry::new(ListenConfig::new(mcp.listen, ListenConfig::default().per_key_limit)),
                shutdown,
            )
            .await
        }
        McpTransport::Http => {
            // Delegated (OAuth) callers are served on `POST /mcp/rpc` of this transport only.
            let inbound = match &mcp.oauth {
                Some(oauth) => InboundAuth::from_runtime(oauth, &mcp.api_url).map_err(anyhow::Error::msg)?,
                None => InboundAuth::legacy_only(),
            };
            run_http(
                &mcp.bind_addr,
                cli::build_client(mcp, None)?,
                Arc::new(inbound),
                surface,
                NetOptions::from_runtime(mcp),
                shutdown,
            )
            .await
        }
        McpTransport::Sse => {
            if mcp.oauth.is_some() {
                tracing::warn!(
                    "[mcp] oauth_* keys are set, but delegated access is served on the http transport only; \
                     the sse transport refuses delegated tokens"
                );
            }
            run_sse(
                &mcp.bind_addr,
                cli::build_client(mcp, None)?,
                Arc::new(InboundAuth::legacy_only()),
                surface,
                NetOptions::from_runtime(mcp),
                shutdown,
            )
            .await
        }
    }
}

/// The stop request of a `serve` process: SIGTERM or SIGINT (Ctrl-C).
///
/// The signal handlers are registered synchronously in [`Shutdown::install`], before the
/// transport starts, so a signal can never arrive while the default disposition (terminate the
/// process, exit 143) is still in place. A task turns the first signal into a `watch` flag that
/// any number of waiters can observe.
#[derive(Clone)]
struct Shutdown {
    requested: watch::Receiver<bool>,
}

impl Shutdown {
    fn install() -> anyhow::Result<Self> {
        let (sender, requested) = watch::channel(false);
        #[cfg(unix)]
        {
            use tokio::signal::unix::{SignalKind, signal};
            let mut terminate = signal(SignalKind::terminate())?;
            let mut interrupt = signal(SignalKind::interrupt())?;
            tokio::spawn(async move {
                let name = tokio::select! {
                    _ = terminate.recv() => "SIGTERM",
                    _ = interrupt.recv() => "SIGINT",
                };
                tracing::info!(signal = name, "MCP server shutdown requested");
                if sender.send(true).is_err() {
                    tracing::warn!("MCP server shutdown requested after every transport had already stopped");
                }
            });
        }
        #[cfg(not(unix))]
        {
            tokio::spawn(async move {
                if let Err(error) = tokio::signal::ctrl_c().await {
                    tracing::warn!(error = %error, "Ctrl-C handler failed; the MCP server cannot be stopped by signal");
                    return;
                }
                tracing::info!(signal = "ctrl_c", "MCP server shutdown requested");
                if sender.send(true).is_err() {
                    tracing::warn!("MCP server shutdown requested after every transport had already stopped");
                }
            });
        }
        Ok(Self { requested })
    }

    /// Resolves once a stop has been requested. Never resolves if the signal task is gone without
    /// having seen a signal, so a lost handler cannot be mistaken for a stop request.
    async fn requested(&self) {
        let mut requested = self.requested.clone();
        if requested.wait_for(|stop| *stop).await.is_err() {
            std::future::pending::<()>().await;
        }
    }

    /// Resolves [`SHUTDOWN_DRAIN_BOUND`] after a stop was requested.
    async fn drain_deadline(&self) {
        self.requested().await;
        tokio::time::sleep(SHUTDOWN_DRAIN_BOUND).await;
    }
}

/// Serves `app` until a stop is requested, then drains.
///
/// On the signal the listener stops accepting connections, idle keep-alive connections are
/// closed and every request in flight runs to completion. The SSE session table is emptied at
/// the same moment: each `GET /sse` stream then ends as soon as no `POST /messages` in flight
/// still holds its sender, which is after that message's result has been pushed onto it, so an
/// open stream neither loses an answer nor keeps the process alive. Whatever is still running
/// after [`SHUTDOWN_DRAIN_BOUND`] is abandoned and logged.
async fn serve_until_shutdown(
    listener: tokio::net::TcpListener,
    app: Router,
    sessions: SseSessions,
    listen: ListenRegistry,
    shutdown: Shutdown,
) -> anyhow::Result<()> {
    use hyper_util::rt::{TokioExecutor, TokioIo, TokioTimer};
    use hyper_util::server::{conn::auto::Builder, graceful::GracefulShutdown};
    use hyper_util::service::TowerToHyperService;

    // A connection that does not finish its request head in time is closed, so a caller that
    // opens sockets and sends nothing cannot hold file descriptors for ever.
    let mut builder = Builder::new(TokioExecutor::new());
    builder
        .http1()
        .timer(TokioTimer::new())
        .header_read_timeout(HEADER_READ_TIMEOUT);
    let graceful = GracefulShutdown::new();
    let serving = async {
        loop {
            let accepted = tokio::select! {
                accepted = listener.accept() => accepted,
                () = shutdown.requested() => break,
            };
            let (stream, peer) = match accepted {
                Ok(accepted) => accepted,
                Err(error) => {
                    tracing::warn!(error = %error, "Failed to accept a connection");
                    tokio::time::sleep(Duration::from_millis(50)).await;
                    continue;
                }
            };
            let connection = builder
                .serve_connection_with_upgrades(
                    TokioIo::new(stream),
                    TowerToHyperService::new(app.clone().layer(Extension(PeerAddr(peer.ip())))),
                )
                .into_owned();
            let connection = graceful.watch(connection);
            tokio::spawn(async move {
                if let Err(error) = connection.await {
                    tracing::debug!(error = %error, "A connection ended with an error");
                }
            });
        }
        tracing::info!(
            drain_bound_ms = u64::try_from(SHUTDOWN_DRAIN_BOUND.as_millis()).unwrap_or(u64::MAX),
            "MCP server draining: accepting no new connections and finishing the requests in flight"
        );
        sessions.lock().await.clear();
        // Every open listen stream ends with its final result, which lets its response finish.
        listen.shutdown();
        graceful.shutdown().await;
    };
    tokio::select! {
        () = serving => {
            tracing::info!("MCP server stopped");
        }
        () = shutdown.drain_deadline() => {
            tracing::warn!(
                drain_bound_ms = u64::try_from(SHUTDOWN_DRAIN_BOUND.as_millis()).unwrap_or(u64::MAX),
                "MCP server drain bound elapsed; stopping with requests still in flight"
            );
        }
    }
    Ok(())
}

/// Wraps the data carrying routes in the inbound authentication layer
/// ([`oauth::inbound_auth`]) and adds the routes that stay outside it.
///
/// `/health` stays outside it on purpose: it answers a constant `OK`, reads nothing and
/// discloses nothing a caller who already completed the TCP handshake does not know, and
/// orchestrator probes have no bot account to authenticate as. The protected resource metadata
/// document is public by definition (RFC 9728): a client reads it before it holds a token.
fn serve_router(state: SseState, protected: Router<SseState>) -> Router {
    // The `Origin` check is the outer layer: it runs before authentication, so a browser page
    // that is not allowed learns nothing, not even whether its token is valid.
    let protected = protected
        .route_layer(middleware::from_fn_with_state(
            Arc::clone(&state.inbound),
            oauth::inbound_auth,
        ))
        .route_layer(middleware::from_fn_with_state(
            Arc::clone(&state.net.allowed_origins),
            http_modern::origin_layer,
        ));
    let mut router = Router::new().merge(protected);
    if let Some(settings) = state.inbound.settings() {
        router = router.merge(oauth::well_known_routes(settings));
    }
    router.route("/health", get(health_check)).with_state(state)
}

async fn run_http(
    bind_addr: &str,
    client: OpenPrClient,
    inbound: Arc<InboundAuth>,
    surface: ToolSurface,
    net: NetOptions,
    shutdown: Shutdown,
) -> anyhow::Result<()> {
    let sessions: SseSessions = Arc::new(Mutex::new(HashMap::new()));
    let listen = net.listen.clone();
    let state = SseState {
        client,
        sessions: Arc::clone(&sessions),
        inbound,
        surface,
        net,
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
    serve_until_shutdown(listener, app, sessions, listen, shutdown).await
}

/// How the networked transports report their inbound authentication in the startup log.
///
/// A constant rather than a computed label because there is now only one answer; a log line
/// that could say something else would imply a setting that no longer exists.
const INBOUND_AUTH_DESCRIPTION: &str = "per-request caller bearer credential (Authorization: Bearer), required";

fn bad_tool_mode(message: &str) -> Response {
    (StatusCode::BAD_REQUEST, Json(json!({ "error": message }))).into_response()
}

async fn handle_jsonrpc(
    State(state): State<SseState>,
    Extension(caller): Extension<Caller>,
    peer: Option<Extension<PeerAddr>>,
    RawQuery(query): RawQuery,
    headers: HeaderMap,
    Json(req): Json<JsonRpcRequest>,
) -> Response {
    // The mode only shapes what a plain caller sees; a delegated caller ignores it, but a value
    // that is not a mode is still a malformed request.
    let mode = match requested_tool_mode(query.as_deref(), &headers, state.surface.mode) {
        Ok(mode) => mode,
        Err(message) => return bad_tool_mode(&message),
    };
    // Authentication already ran (route layer). Next: the era, then, for the modern era, the
    // headers, so an unauthenticated caller learns nothing from a header error.
    let era = match era::classify(&req) {
        Ok(era) => era,
        Err(error) => return refusal_response(req.id, error.into_rpc_error()),
    };
    if let Err(error) = http_modern::validate_headers(&headers, &req, &era) {
        return refusal_response(req.id, error);
    }
    if era.is_modern() && req.method == "subscriptions/listen" && req.id.is_some() {
        return open_http_listen(&state, &caller, peer.map(|Extension(peer)| peer.0), &era, &req).await;
    }
    oauth::handle_rpc(
        &state.client,
        &state.inbound,
        caller,
        state.surface.with_mode(mode),
        &era,
        req,
    )
    .await
}

/// A modern request that cannot be served, as the HTTP answer of the era: the error with its
/// status, or `202` for a notification, which never gets an answer.
fn refusal_response(id: Option<serde_json::Value>, error: protocol::JsonRpcError) -> Response {
    id.map_or_else(
        || (StatusCode::ACCEPTED, Json(json!({"status": "accepted"}))).into_response(),
        |id| http_modern::error_response(Some(id), error, true),
    )
}

/// What the registry needs to know about a caller: its credential's key, and for a token the
/// client that acts as it (a hub keeps that client, and so the token, for the life of its streams).
fn listen_auth(state: &SseState, caller: &Caller) -> ListenAuth {
    match caller {
        Caller::Legacy(token) => {
            let client = state.client.acting_as(token.clone());
            ListenAuth::Token {
                key: caller_key(token.expose()),
                server: Arc::new(McpServer::for_surface(client.clone(), state.surface)),
                client,
            }
        }
        Caller::Delegated(delegated) => ListenAuth::Delegated {
            key: caller_key(delegated.token.expose()),
        },
    }
}

/// Opens a listen stream on the Server-Sent Events response of a modern `subscriptions/listen`.
///
/// A credential the API refuses is answered `401` with the challenge before any SSE byte; an API
/// that cannot be asked is answered with a JSON-RPC error as plain JSON, so the client backs off.
async fn open_http_listen(
    state: &SseState,
    caller: &Caller,
    peer: Option<std::net::IpAddr>,
    era: &Era,
    req: &JsonRpcRequest,
) -> Response {
    let Some(id) = req.id.clone() else {
        return refusal_response(None, protocol::JsonRpcError::invalid_request("missing id"));
    };
    if let Err(error) = mcp_server::listen::validate_envelope(req) {
        let echo = Some(id).filter(|id| id.is_string() || id.is_number());
        return http_modern::error_response(echo, error, true);
    }
    let registry = &state.net.listen;
    if !registry.enabled() {
        return refusal_response(
            Some(id),
            protocol::JsonRpcError::method_not_found("Unknown method: subscriptions/listen"),
        );
    }
    let handle = match registry
        .open_from(peer, listen_auth(state, caller), &id, req.params.as_ref())
        .await
    {
        Ok(handle) => handle,
        Err(mcp_server::listen::OpenError::Unauthorized) => {
            tracing::info!(
                method = "subscriptions/listen",
                era = era.label(),
                "Refused a listen stream: the API does not accept the credential"
            );
            return oauth::credential_refused_response(&state.inbound);
        }
        Err(error) => {
            tracing::info!(
                method = "subscriptions/listen",
                era = era.label(),
                "Refused a listen stream: {error:?}"
            );
            return refusal_response(Some(id), error.into_rpc_error());
        }
    };
    tracing::info!(
        method = "subscriptions/listen",
        era = era.label(),
        id_kind = mcp_server::server::id_kind(Some(&id)),
        open_streams = registry.open_streams(),
        "Opened a listen stream"
    );
    let (tx, rx) = mpsc::channel::<String>(LISTEN_QUEUE);
    // The task owns the stream, so the slot is released when the task ends, whatever the
    // connection does.
    tokio::spawn(async move { handle.forward(&tx, STALL_LIMIT, |message| message).await });
    http_modern::listen_sse_response(ReceiverStream::new(rx), registry.keepalive())
}

/// Frames queued between a listen stream and its response.
const LISTEN_QUEUE: usize = 64;

/// One open SSE stream: the sender that feeds it, and the tool mode its connection asked for.
struct SseSession {
    sender: mpsc::UnboundedSender<SseServerEvent>,
    mode: McpToolMode,
}

/// Open SSE streams by session id.
type SseSessions = Arc<Mutex<HashMap<String, SseSession>>>;

#[derive(Clone)]
struct SseState {
    client: OpenPrClient,
    sessions: SseSessions,
    inbound: Arc<InboundAuth>,
    /// The server's own tool mode and write switch; a connection may choose another mode.
    surface: ToolSurface,
    net: NetOptions,
}

#[derive(Debug)]
enum SseServerEvent {
    Endpoint(String),
    Message(String),
}

struct SessionGuard {
    session_id: String,
    sessions: SseSessions,
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

async fn run_sse(
    bind_addr: &str,
    client: OpenPrClient,
    inbound: Arc<InboundAuth>,
    surface: ToolSurface,
    net: NetOptions,
    shutdown: Shutdown,
) -> anyhow::Result<()> {
    let sessions: SseSessions = Arc::new(Mutex::new(HashMap::new()));
    let listen = net.listen.clone();
    let state = SseState {
        client,
        sessions: Arc::clone(&sessions),
        inbound,
        surface,
        net,
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
    serve_until_shutdown(listener, app, sessions, listen, shutdown).await
}

async fn handle_sse_connect(State(state): State<SseState>, RawQuery(query): RawQuery, headers: HeaderMap) -> Response {
    // The mode is chosen once, when the stream opens, and belongs to the session.
    let mode = match requested_tool_mode(query.as_deref(), &headers, state.surface.mode) {
        Ok(mode) => mode,
        Err(message) => return bad_tool_mode(&message),
    };
    let session_id = Uuid::new_v4().to_string();
    let endpoint = format!("/messages?session_id={session_id}");
    let (tx, rx) = mpsc::unbounded_channel::<SseServerEvent>();

    state.sessions.lock().await.insert(
        session_id.clone(),
        SseSession {
            sender: tx.clone(),
            mode,
        },
    );

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

    Sse::new(stream).keep_alive(KeepAlive::default()).into_response()
}

/// The SSE session is only a delivery channel: the identity a request is served as comes
/// from the `POST /messages` that carried it, never from the `GET /sse` that opened the
/// stream. Binding identity to the session would let one caller's messages ride another
/// caller's stream, and would make the identity outlive the request that proved it.
async fn handle_sse_message(
    State(state): State<SseState>,
    Extension(caller): Extension<Caller>,
    Query(query): Query<MessagesQuery>,
    Json(req): Json<JsonRpcRequest>,
) -> impl IntoResponse {
    // The inbound layer lets a delegated caller through on `POST /mcp/rpc` only; on this route
    // there is no other kind of caller than a legacy one, and anything else is refused.
    let Some(token) = caller.into_legacy() else {
        return (
            StatusCode::FORBIDDEN,
            Json(json!({"error": "delegated access is available on POST /mcp/rpc only"})),
        );
    };
    let session = state
        .sessions
        .lock()
        .await
        .get(&query.session_id)
        .map(|session| (session.sender.clone(), session.mode));
    let Some((sender, mode)) = session else {
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
    let server = McpServer::for_surface(state.client.acting_as(token), state.surface.with_mode(mode));
    let response = server.handle_request_in(&Era::Legacy, req).await;
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
mod sse_delivery_tests {
    use super::{
        Caller, InboundAuth, McpToolMode, MessagesQuery, SseServerEvent, SseSession, SseState, ToolSurface,
        handle_sse_message,
    };
    use axum::{
        Extension, Json,
        extract::{Query, RawQuery, State},
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

    /// A counting stand-in for the Sylvode API.
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

    fn state_with_client(client: OpenPrClient, surface: ToolSurface, sessions: super::SseSessions) -> SseState {
        SseState {
            client,
            sessions,
            inbound: Arc::new(InboundAuth::legacy_only()),
            surface,
            net: super::NetOptions::default(),
        }
    }

    fn state_with(surface: ToolSurface, sessions: super::SseSessions) -> Result<SseState, String> {
        let client = OpenPrClient::new(ClientConfig {
            base_url: "http://127.0.0.1:1".to_string(),
            credential: None,
            workspace_id: Some(WORKSPACE.to_string()),
            transport_label: "sse",
        })?;
        Ok(state_with_client(client, surface, sessions))
    }

    /// The tool mode is chosen when the stream opens and belongs to that session: the query, the
    /// header or, absent both, the server's own; an invalid or conflicting choice opens no stream.
    #[tokio::test]
    async fn a_session_takes_the_tool_mode_its_connection_asked_for() -> TestResult {
        use axum::http::{HeaderMap, HeaderValue};
        let sessions: super::SseSessions = Arc::new(Mutex::new(HashMap::new()));
        let state = state_with(ToolSurface::new(McpToolMode::Search, true), Arc::clone(&sessions))?;
        let header = |value: &str| {
            let mut headers = HeaderMap::new();
            headers.insert(
                mcp_server::server::TOOL_MODE_HEADER,
                HeaderValue::from_str(value).expect("header value"),
            );
            headers
        };
        let open = |query: Option<&str>, headers: HeaderMap| {
            let state = state.clone();
            let query = query.map(|value| format!("tools={value}"));
            async move { super::handle_sse_connect(State(state), RawQuery(query), headers).await }
        };
        let modes = |sessions: &super::SseSessions| {
            let sessions = Arc::clone(sessions);
            async move {
                let mut modes = sessions
                    .lock()
                    .await
                    .values()
                    .map(|session| session.mode)
                    .collect::<Vec<_>>();
                modes.sort_by_key(|mode| mode.as_str());
                modes
            }
        };

        let mut streams = Vec::new();
        for (query, headers, expected) in [
            (None, HeaderMap::new(), McpToolMode::Search),
            (Some("full"), HeaderMap::new(), McpToolMode::Full),
            (None, header("full"), McpToolMode::Full),
            (Some("search"), header("search"), McpToolMode::Search),
        ] {
            let before = sessions.lock().await.len();
            let response = open(query, headers).await;
            assert_eq!(response.status(), StatusCode::OK);
            streams.push(response);
            let all = sessions.lock().await;
            assert_eq!(all.len(), before + 1);
            drop(all);
            assert!(modes(&sessions).await.contains(&expected));
        }
        assert_eq!(
            modes(&sessions).await,
            [
                McpToolMode::Full,
                McpToolMode::Full,
                McpToolMode::Search,
                McpToolMode::Search
            ]
        );

        let before = sessions.lock().await.len();
        for (query, headers) in [
            (Some("everything"), HeaderMap::new()),
            (None, header("everything")),
            (Some("full"), header("search")),
            (Some("full&tools=search"), HeaderMap::new()),
        ] {
            let response = open(query, headers).await;
            assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        }
        assert_eq!(
            sessions.lock().await.len(),
            before,
            "a refused connection left a session behind"
        );
        // The streams own their sessions: dropping one removes its entry, so they live until here.
        drop(streams);
        Ok(())
    }

    /// `allow_write_call = false` holds for a session in full mode: the direct write call is
    /// answered with the refusal and never reaches the API.
    #[tokio::test]
    async fn a_full_mode_session_cannot_write_when_writes_are_disabled() -> TestResult {
        let (api_url, calls) = counting_api().await?;
        let client = OpenPrClient::new(ClientConfig {
            base_url: api_url,
            credential: None,
            workspace_id: Some(WORKSPACE.to_string()),
            transport_label: "sse",
        })
        .map_err(|error| -> Box<dyn std::error::Error> { error.into() })?;
        let sessions: super::SseSessions = Arc::new(Mutex::new(HashMap::new()));
        let state = state_with_client(
            client,
            ToolSurface::new(McpToolMode::Search, false),
            Arc::clone(&sessions),
        );
        let (tx, mut rx) = mpsc::unbounded_channel::<SseServerEvent>();
        sessions.lock().await.insert(
            "full".to_string(),
            SseSession {
                sender: tx,
                mode: McpToolMode::Full,
            },
        );
        let accepted = handle_sse_message(
            State(state),
            Extension(Caller::Legacy(Secret::new(CALLER_TOKEN))),
            Query(MessagesQuery {
                session_id: "full".to_string(),
            }),
            Json(JsonRpcRequest {
                jsonrpc: "2.0".to_string(),
                id: Some(json!(1)),
                method: "tools/call".to_string(),
                params: Some(json!({ "name": "labels.create", "arguments": {"name": "b", "color": "#ff0000"} })),
            }),
        )
        .await
        .into_response();
        assert_eq!(accepted.status(), StatusCode::ACCEPTED);
        let Some(SseServerEvent::Message(payload)) = rx.recv().await else {
            return Err("no answer on the stream".into());
        };
        assert!(
            payload.contains("writes are disabled for this bot by the runner configuration"),
            "{payload}"
        );
        assert_eq!(calls.load(Ordering::SeqCst), 0, "a refused write reached the API");
        Ok(())
    }

    /// A message is served in the mode of the session it belongs to, whatever the server's default.
    #[tokio::test]
    async fn a_message_is_served_in_the_mode_of_its_session() -> TestResult {
        let sessions: super::SseSessions = Arc::new(Mutex::new(HashMap::new()));
        let state = state_with(ToolSurface::new(McpToolMode::Search, true), Arc::clone(&sessions))?;
        let mut streams = HashMap::new();
        for (id, mode) in [
            ("search-session", McpToolMode::Search),
            ("full-session", McpToolMode::Full),
        ] {
            let (tx, rx) = mpsc::unbounded_channel::<SseServerEvent>();
            sessions
                .lock()
                .await
                .insert(id.to_string(), SseSession { sender: tx, mode });
            streams.insert(id, rx);
        }
        for (id, expected) in [
            ("search-session", 9_usize),
            ("full-session", mcp_server::get_all_tool_definitions().len()),
        ] {
            let accepted = handle_sse_message(
                State(state.clone()),
                Extension(Caller::Legacy(Secret::new(CALLER_TOKEN))),
                Query(MessagesQuery {
                    session_id: id.to_string(),
                }),
                Json(JsonRpcRequest {
                    jsonrpc: "2.0".to_string(),
                    id: Some(json!(1)),
                    method: "tools/list".to_string(),
                    params: None,
                }),
            )
            .await
            .into_response();
            assert_eq!(accepted.status(), StatusCode::ACCEPTED);
            let Some(rx) = streams.get_mut(id) else {
                return Err("no stream".into());
            };
            let Some(SseServerEvent::Message(payload)) = rx.recv().await else {
                return Err("the answer was not pushed onto the stream".into());
            };
            let answer: serde_json::Value = serde_json::from_str(&payload)?;
            let count = answer
                .pointer("/result/tools")
                .and_then(|tools| tools.as_array())
                .map(Vec::len);
            assert_eq!(count, Some(expected), "{id}");
        }
        Ok(())
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
            workspace_id: Some(WORKSPACE.to_string()),
            transport_label: "sse",
        })
        .map_err(|error| -> Box<dyn std::error::Error> { error.into() })?;

        let (tx, rx) = mpsc::unbounded_channel::<SseServerEvent>();
        drop(rx);
        let session_id = "5f6a1a3c-0f4d-4a2e-9a1b-6d2f5c8e7b40".to_string();
        let mut registry = HashMap::new();
        registry.insert(
            session_id.clone(),
            SseSession {
                sender: tx,
                mode: McpToolMode::Full,
            },
        );
        let sessions = Arc::new(Mutex::new(registry));

        let state = SseState {
            client,
            sessions: Arc::clone(&sessions),
            inbound: Arc::new(InboundAuth::legacy_only()),
            surface: ToolSurface::new(McpToolMode::Full, true),
            net: super::NetOptions::default(),
        };
        let refusal = handle_sse_message(
            State(state),
            Extension(Caller::Legacy(Secret::new(CALLER_TOKEN))),
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

/// The messages stdout is to carry, each in the framing of the request it answers.
type StdioSender = mpsc::Sender<(String, StdioFrame)>;

/// Open listen streams of a stdio pipe by JSON-RPC id (as JSON text, so `7` and `"7"` differ).
type OpenListens = Arc<Mutex<HashMap<String, CancellationToken>>>;

/// Capacity of the channel in front of the stdout writer.
const STDIO_WRITER_CAPACITY: usize = 1_024;

/// The one task that owns stdout. Every message, whichever task produced it, goes through here, so
/// two messages never interleave on the pipe.
async fn stdio_writer(mut messages: mpsc::Receiver<(String, StdioFrame)>) {
    let mut stdout = tokio::io::stdout();
    while let Some((message, frame)) = messages.recv().await {
        if let Err(error) = write_stdio_response(&mut stdout, &message, frame).await {
            tracing::error!(error = %error, "Failed to write to stdout");
            break;
        }
    }
    if let Err(error) = stdout.flush().await {
        tracing::error!(error = %error, "Failed to flush stdout");
    }
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

/// The stdio transport.
///
/// Ordinary requests are handled one at a time, in order, as before. A modern
/// `subscriptions/listen` is a long-lived request: it runs as its own task, so the pipe keeps
/// serving other requests while it is open, and `notifications/cancelled` ends it. One writer task
/// owns stdout.
///
/// On SIGTERM/SIGINT or when stdin closes it stops reading. A message already being handled is
/// finished (bounded by [`SHUTDOWN_DRAIN_BOUND`]), every open listen stream gets its final result,
/// stdout is flushed and the transport returns, so the process exits 0 even though the parent
/// still holds stdin open.
async fn run_stdio(
    client: OpenPrClient,
    surface: ToolSurface,
    listen: ListenRegistry,
    shutdown: Shutdown,
) -> anyhow::Result<()> {
    tracing::info!("MCP stdio transport started");

    // A hub keeps a client that acts as the configured bot; the server below owns another.
    let listen_client = client.clone();
    let server = McpServer::for_surface(client, surface);
    let stdin = tokio::io::stdin();
    let mut reader = BufReader::new(stdin);
    let (out, out_rx) = mpsc::channel::<(String, StdioFrame)>(STDIO_WRITER_CAPACITY);
    let writer = tokio::spawn(stdio_writer(out_rx));
    let open: OpenListens = Arc::new(Mutex::new(HashMap::new()));

    loop {
        let mut line = String::new();
        let read = tokio::select! {
            biased;
            () = shutdown.requested() => {
                tracing::info!("MCP stdio transport stopping: no further input is read");
                break;
            }
            read = reader.read_line(&mut line) => read,
        };
        match read {
            Ok(0) => {
                tracing::info!("stdin closed, shutting down");
                break;
            }
            Ok(_) => {
                let trimmed = line.trim();
                if trimmed.is_empty() {
                    continue;
                }
                tokio::select! {
                    () = process_stdio_message(&server, &listen_client, &listen, &open, &out, &mut reader, trimmed) => {}
                    () = shutdown.drain_deadline() => {
                        tracing::warn!(
                            drain_bound_ms = u64::try_from(SHUTDOWN_DRAIN_BOUND.as_millis()).unwrap_or(u64::MAX),
                            "MCP stdio drain bound elapsed; stopping with a message still in flight"
                        );
                        break;
                    }
                }
            }
            Err(e) => {
                tracing::error!(error = %e, "Failed to read from stdin");
                break;
            }
        }
    }

    // Every open listen stream ends with its final result, then the writer drains and flushes.
    listen.shutdown();
    if tokio::time::timeout(SHUTDOWN_DRAIN_BOUND, listen.wait_idle())
        .await
        .is_err()
    {
        tracing::warn!("MCP stdio drain bound elapsed with listen streams still open");
    }
    drop(out);
    if tokio::time::timeout(SHUTDOWN_DRAIN_BOUND, writer).await.is_err() {
        tracing::warn!("MCP stdio drain bound elapsed with output still pending");
    }
    Ok(())
}

/// Sends one message to the stdout writer. A closed writer means stdout is gone; the message is
/// dropped.
async fn send_stdio(out: &StdioSender, message: &(impl serde::Serialize + Sync), frame: StdioFrame) {
    match serde_json::to_string(message) {
        Ok(json) => {
            if out.send((json, frame)).await.is_err() {
                tracing::error!("Failed to write response: the stdout writer is gone");
            }
        }
        Err(e) => tracing::error!(error = %e, "Failed to serialize response"),
    }
}

/// Reads the rest of one stdio message that starts with `first_line`, handles it and answers in
/// the framing the message arrived in.
async fn process_stdio_message(
    server: &McpServer,
    client: &OpenPrClient,
    listen: &ListenRegistry,
    open: &OpenListens,
    out: &StdioSender,
    reader: &mut BufReader<tokio::io::Stdin>,
    first_line: &str,
) {
    // Detect Content-Length framing (used by Codex, Claude Desktop)
    let (payload, frame) = if is_stdio_header_line(first_line) {
        let cl = read_headers_get_content_length(reader, first_line).await;
        if cl == 0 {
            return;
        }
        let mut body = vec![0u8; cl];
        if let Err(e) = reader.read_exact(&mut body).await {
            tracing::error!(error = %e, "Failed to read Content-Length body");
            return;
        }
        (body, StdioFrame::ContentLength)
    } else {
        // Line-delimited JSON
        (first_line.as_bytes().to_vec(), StdioFrame::LineDelimited)
    };

    let request: JsonRpcRequest = match serde_json::from_slice(&payload) {
        Ok(req) => req,
        Err(e) => {
            tracing::error!(error = %e, "Failed to parse request");
            let error_response =
                JsonRpcResponse::error(None, protocol::JsonRpcError::parse_error(format!("Invalid JSON: {e}")));
            send_stdio(out, &error_response, frame).await;
            return;
        }
    };

    tracing::debug!(method = %request.method, "Received request");

    if request.method == "notifications/cancelled" && request.id.is_none() {
        cancel_listen(open, request.params.as_ref()).await;
        return;
    }
    if request.method == "subscriptions/listen"
        && request.id.is_some()
        && matches!(era::classify(&request), Ok(Era::Modern { .. }))
    {
        open_stdio_listen(client, listen, open, out, request, frame).await;
        return;
    }

    if let Some(response) = server.handle_request(request).await {
        send_stdio(out, &response, frame).await;
    }
}

/// `notifications/cancelled`: ends the listen the `requestId` names, without a final result. Any
/// other id is ignored.
async fn cancel_listen(open: &OpenListens, params: Option<&serde_json::Value>) {
    let Some(request_id) = params.and_then(|params| params.get("requestId")) else {
        return;
    };
    if let Some(cancel) = open.lock().await.get(&request_id.to_string()) {
        cancel.cancel();
    }
}

/// Starts a listen stream as its own task writing through the stdout writer.
async fn open_stdio_listen(
    client: &OpenPrClient,
    listen: &ListenRegistry,
    open: &OpenListens,
    out: &StdioSender,
    request: JsonRpcRequest,
    frame: StdioFrame,
) {
    let Some(id) = request.id.clone() else { return };
    if let Err(error) = mcp_server::listen::validate_envelope(&request) {
        let echo = Some(id.clone()).filter(|id| id.is_string() || id.is_number());
        send_stdio(out, &JsonRpcResponse::error(echo, error), frame).await;
        return;
    }
    let refuse = |error: protocol::JsonRpcError| JsonRpcResponse::error(Some(id.clone()), error);
    if !listen.enabled() {
        let error = protocol::JsonRpcError::method_not_found("Unknown method: subscriptions/listen");
        send_stdio(out, &refuse(error), frame).await;
        return;
    }
    let key = id.to_string();
    if open.lock().await.contains_key(&key) {
        let error = protocol::JsonRpcError::invalid_request("duplicate request id");
        send_stdio(out, &refuse(error), frame).await;
        return;
    }
    // One pipe is one credential: the configured bot.
    let auth = ListenAuth::Token {
        key: caller_key("stdio"),
        server: Arc::new(McpServer::new(client.clone(), McpToolMode::Full)),
        client: client.clone(),
    };
    let mut handle = match listen.open(auth, &id, request.params.as_ref()).await {
        Ok(handle) => handle,
        Err(error) => {
            send_stdio(out, &refuse(error.into_rpc_error()), frame).await;
            return;
        }
    };
    open.lock().await.insert(key.clone(), handle.cancel_token());
    // The acknowledgment goes out before the stream's task exists, so nothing the reader answers
    // next (a duplicate id, for one) can overtake it.
    if let Some(ack) = handle.next_message().await {
        send_stdio(out, &ack, frame).await;
    }
    tracing::info!(
        method = "subscriptions/listen",
        era = "modern",
        id_kind = mcp_server::server::id_kind(Some(&id)),
        "Opened a listen stream"
    );
    let out = out.clone();
    let open = Arc::clone(open);
    tokio::spawn(async move {
        handle.forward(&out, STALL_LIMIT, |message| (message, frame)).await;
        open.lock().await.remove(&key);
    });
}
