use api::flow::collab::limits::{PROCESS_DRAIN_GRACE_MS, PROCESS_DRAIN_RETRY_AFTER_MS};
use api::{response::ApiResponse, routes};
use axum::{Json, extract::State, response::IntoResponse};
use clap::Parser;
use platform::{
    app::{AppState, connect_db},
    config::{AppConfig, OpenPrConfig},
    logging,
};

use serde::Serialize;
use std::io::Write as _;
use std::path::PathBuf;
use std::time::Duration;

#[derive(Serialize)]
struct HealthResponse {
    status: &'static str,
    service: String,
}

#[derive(Serialize)]
struct BuildInfo {
    schema_version: &'static str,
    git_commit: Option<&'static str>,
    git_dirty: Option<bool>,
    git_committer_date: Option<&'static str>,
    source: &'static str,
}

/// Sylvode API server command-line surface.
///
/// The process accepts a configuration path or a side-effect-free build identity query.
#[derive(Parser)]
#[command(name = "api", about = "Sylvode API server")]
struct Args {
    /// Path to the configuration file. Defaults to config/sylvode.toml.
    #[arg(long, value_name = "PATH")]
    config: Option<PathBuf>,

    /// Print embedded build provenance as JSON and exit before loading configuration.
    #[arg(long)]
    build_info: bool,
}

/// Logs once per process, at startup, when this build has no `ADR-0014` isolated-apply boundary
/// (every target except Linux). Such a server still serves Projects, Forms, MCP, and Flow reads,
/// but every Flow collaborative write and object diff is refused with `server_rejected`
/// (`details.reason = "isolated_apply_unsupported_platform"`); an operator should learn that from
/// the log at boot, not from the first refused write.
fn warn_if_flow_collab_writes_unsupported() {
    if cfg!(not(target_os = "linux")) {
        tracing::warn!(
            os = std::env::consts::OS,
            "Flow collaborative writes are unavailable on this platform (ADR-0014): the isolated-apply \
             boundary requires a Linux server; Flow writes and object diffs will be rejected with \
             server_rejected"
        );
    }
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let args = Args::parse();
    if args.build_info {
        write_build_info()?;
        return Ok(());
    }
    let config = OpenPrConfig::load(args.config.as_deref())?;
    // Installed before the logger so that a configuration this process cannot publish aborts
    // startup rather than leaving half the service reading the fallback settings.
    api::config::install(&config).map_err(|err| anyhow::anyhow!("{err}"))?;
    logging::init(&config.logging, "api")?;
    warn_if_flow_collab_writes_unsupported();

    // from_config first: it reports a missing database url and signing key together, so the
    // operator fixes both in one pass instead of being walked through them one at a time.
    let cfg = AppConfig::from_config(&config, "api", "0.0.0.0:8081")?;
    let db = connect_db(&config.database_runtime()?).await?;
    migrate::run_migrations(&db, config.migrations).await?;
    migrate::verify_governance_schema(&db).await?;
    // Every router route has exactly one catalogue entry, or the API does not start.
    match api::catalogue::boot::check(router::ROUTER_SOURCES, &db).await {
        Ok(report) => tracing::info!(
            entries = report.entries,
            capabilities = report.capabilities,
            "operation catalogue checked"
        ),
        Err(error) => {
            tracing::error!(%error, "operation catalogue check failed");
            return Err(error.into());
        }
    }
    api::packages::boot::sync_builtin(&db, &config.packages).await?;
    let state = AppState {
        cfg: cfg.clone(),
        db,
        flow_permission_cache: platform::app::FlowPermissionCacheSlot::default(),
    };
    api::flow::collab::fanout::spawn_listener(state.clone(), &api::flow::collab::runtime::runtime().registry);
    // Proposal settlement deliberately does not run here. It used to run both as an API
    // background task and inline on `GET /api/v1/proposals*`, which made a read request write
    // decisions and trust scores. The worker polling pipeline is now the only writer.
    routes::webhook::spawn_stored_endpoint_audit(state.clone());
    // Conversion jobs left `started` past the stale threshold are failed at startup and then
    // every `flow.conversion_reap_interval_secs` (`rest-api-v1.md` v0.7); the loop stops when
    // shutdown begins.
    let (reaper_stop, reaper_stop_rx) = tokio::sync::watch::channel(false);
    tokio::spawn(api::flow::bridge::run_conversion_reaper(
        state.clone(),
        Duration::from_secs(config.flow.conversion_reap_interval_secs),
        reaper_stop_rx,
    ));

    let reader_db = state.db.clone();
    let app = router::build_router(state);
    // Host calls of the plugin host go through this router, in process.
    api::extensions::dispatch::install(app.clone());

    let listener = tokio::net::TcpListener::bind(&cfg.bind_addr).await?;
    api::flow::scope_runtime::spawn(reader_db, "api").await?;
    tracing::info!(bind_addr = %cfg.bind_addr, "api server started");
    axum::serve(
        listener,
        app.into_make_service_with_connect_info::<std::net::SocketAddr>(),
    )
    .with_graceful_shutdown(drain_then_shutdown(reaper_stop))
    .await?;
    Ok(())
}

fn write_build_info() -> anyhow::Result<()> {
    let info = embedded_build_info();
    let stdout = std::io::stdout();
    let mut output = stdout.lock();
    serde_json::to_writer(&mut output, &info)?;
    output.write_all(b"\n")?;
    Ok(())
}

fn embedded_build_info() -> BuildInfo {
    let commit = env!("OPENPR_EMBEDDED_GIT_COMMIT");
    let dirty = env!("OPENPR_EMBEDDED_GIT_DIRTY");
    let committer_date = env!("OPENPR_EMBEDDED_GIT_COMMITTER_DATE");
    BuildInfo {
        schema_version: "openpr.build-info.v1",
        git_commit: (commit != "unknown").then_some(commit),
        git_dirty: embedded_git_dirty(dirty),
        git_committer_date: (committer_date != "unknown").then_some(committer_date),
        source: env!("OPENPR_EMBEDDED_PROVENANCE_SOURCE"),
    }
}

fn embedded_git_dirty(value: &str) -> Option<bool> {
    match value {
        "true" => Some(true),
        "false" => Some(false),
        _ => None,
    }
}

#[cfg(test)]
mod delegated_router_tests;

#[cfg(test)]
mod item_api_tests;

#[cfg(test)]
mod issue_detail_api_tests;

#[cfg(test)]
mod issue_page_api_tests;

#[cfg(test)]
mod milestone_api_tests;

#[cfg(test)]
mod mention_api_tests;

#[cfg(test)]
mod identifier_api_tests;

#[cfg(test)]
mod notification_api_tests;

#[cfg(test)]
mod sprint_api_tests;

#[cfg(test)]
mod webhook_events_api_tests;

#[cfg(test)]
mod workflow_api_tests;

#[cfg(test)]
mod owner_lookup_api_tests;

#[cfg(test)]
mod label_api_tests;

#[cfg(test)]
mod export_import_api_tests;

#[cfg(test)]
mod admin_setup_api_tests;

#[cfg(test)]
mod plugin_template_api_tests;

#[cfg(test)]
mod legacy_auth_characterization_tests;

#[cfg(test)]
mod build_info_tests {
    use super::{embedded_build_info, embedded_git_dirty};
    use std::path::Path;
    use std::process::Command;

    #[test]
    fn embedded_build_info_preserves_dirty_true() {
        assert_eq!(embedded_git_dirty("true"), Some(true));
        if std::env::var_os("OPENPR_PROVENANCE_VERIFY_GIT_DIRTY").is_some() {
            let repo = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
            let status = Command::new("git")
                .arg("-C")
                .arg(repo)
                .args(["status", "--porcelain", "--untracked-files=normal"])
                .output()
                .expect("git status must be executable for the targeted provenance test");
            assert!(status.status.success());
            assert!(!status.stdout.is_empty(), "targeted test requires a dirty fixture");
            assert_eq!(embedded_build_info().git_dirty, Some(true));
        }
    }
}

/// Graceful shutdown: the production trigger for `server_draining` with
/// `details.reason = "drain"` (`contracts/error-mapping-v1.md`: that reason is for "实例/workspace
/// 正在停止接收或排空连接", as opposed to `contention`'s transient single-writer window).
///
/// On SIGTERM/Ctrl-C this marks the whole collab runtime draining *before* the listener is torn
/// down, which does three things at once, all through the one shared producer
/// (`flow::collab::runtime::CollabRuntime::begin_process_drain`) so no surface can invent its own
/// shape: every live WebSocket session is sent the structured `rejected` frame and then closed at
/// 4410, and every subsequent REST/MCP/CLI Flow call is refused with the `server_draining`
/// business envelope carrying the same `reason`/`retry_after_ms`. It then keeps serving for
/// [`PROCESS_DRAIN_GRACE_MS`] so those rejections and closes actually reach their clients instead
/// of racing the process exit.
///
/// Open event streams of connected applications end first. An event stream never ends on its own
/// while its client is connected, so `with_graceful_shutdown` would wait for it forever: the
/// first thing the shutdown path does is end every stream (the client sees the response end and
/// reconnects with its last event id to wherever the service comes back).
async fn drain_then_shutdown(reaper_stop: tokio::sync::watch::Sender<bool>) {
    drain_after(
        shutdown_signal(),
        reaper_stop,
        || api::flow::collab::runtime::runtime().begin_process_drain(PROCESS_DRAIN_RETRY_AFTER_MS),
        Duration::from_millis(PROCESS_DRAIN_GRACE_MS),
    )
    .await;
}

/// The shutdown sequence after `signal`, with the Flow drain and the grace period passed in so a
/// test can run the real sequence without putting the whole test process into drain mode.
async fn drain_after(
    signal: impl std::future::Future<Output = ()>,
    reaper_stop: tokio::sync::watch::Sender<bool>,
    begin_flow_drain: impl FnOnce() -> usize,
    grace: Duration,
) {
    signal.await;
    // Streams first: nothing below can make one end.
    api::oauth::stream::begin_shutdown();
    api::events::hints::begin_shutdown();
    // The receiver is gone only if the reaper already stopped; nothing is left to stop then.
    let _ = reaper_stop.send(true);
    let closed = begin_flow_drain();
    tracing::info!(
        closed_sessions = closed,
        retry_after_ms = PROCESS_DRAIN_RETRY_AFTER_MS,
        grace_ms = u64::try_from(grace.as_millis()).unwrap_or(u64::MAX),
        "api server draining: refusing new Flow work and closing collab sessions before shutdown"
    );
    tokio::time::sleep(grace).await;
}

async fn shutdown_signal() {
    #[cfg(unix)]
    {
        match tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()) {
            Ok(mut sigterm) => {
                tokio::select! {
                    res = tokio::signal::ctrl_c() => {
                        if let Err(err) = res {
                            tracing::warn!(error = %err, "ctrl_c signal error");
                        }
                    },
                    _ = sigterm.recv() => {},
                }
            }
            Err(err) => {
                tracing::warn!(error = %err, "failed to register SIGTERM handler, falling back to ctrl_c only");
                if let Err(err) = tokio::signal::ctrl_c().await {
                    tracing::warn!(error = %err, "ctrl_c signal error");
                }
            }
        }
    }
    #[cfg(not(unix))]
    {
        if let Err(err) = tokio::signal::ctrl_c().await {
            tracing::warn!(error = %err, "ctrl_c signal error");
        }
    }
}

async fn health(State(state): State<AppState>) -> Json<ApiResponse<HealthResponse>> {
    let body = HealthResponse {
        status: "ok",
        service: state.cfg.app_name,
    };
    ApiResponse::success(body)
}

async fn ready(State(state): State<AppState>) -> impl IntoResponse {
    match state.db.ping().await {
        Ok(()) => ApiResponse::success(serde_json::json!({"status":"ready"})).into_response(),
        Err(err) => {
            tracing::warn!(error = %err, "database not ready");
            (
                axum::http::StatusCode::SERVICE_UNAVAILABLE,
                ApiResponse::error(503, "database not ready"),
            )
                .into_response()
        }
    }
}

mod migrate;
mod router;

/// Proposal endpoints against a real PostgreSQL server.
///
/// Two properties are checked here that no unit test can reach: a proposal belonging to another
/// workspace is not readable, and reading a proposal whose voting window has expired no longer
/// settles it. Settlement belongs to the worker tick, which is driven explicitly.
///
/// Uses the same `OPENPR_TEST_DATABASE_URL` as the migration tests.
#[cfg(test)]
// Test modules index fixtures directly, print skip reasons, and mirror wire shapes;
// panicking there is the assertion rather than a defect.
#[allow(
    clippy::indexing_slicing,
    clippy::print_stderr,
    clippy::doc_markdown,
    clippy::redundant_pub_crate,
    clippy::struct_field_names,
    clippy::case_sensitive_file_extension_comparisons
)]
mod proposal_scope_database_tests;

/// The Flow surface answers database failures with typed envelopes, through the real router.
///
/// `routes::flow_surface::seal` is the one place that switches this on, and [`build_router`]
/// applies it to [`flow_surface_routes`] only. The first test needs no database: it drives every
/// registered route and checks, on the responses, which ones are behind the boundary. The second
/// injects real `PostgreSQL` failures (same `OPENPR_TEST_DATABASE_URL` as the migration tests).
#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::print_stderr,
    clippy::indexing_slicing,
    clippy::too_many_lines
)]
mod flow_surface_boundary_tests;
