use std::path::PathBuf;

use api::config::RuntimeConfig;
use clap::Parser;
use platform::{
    app::{AppState, FlowPermissionCacheSlot, connect_db},
    config::{AppConfig, OpenPrConfig},
    logging,
};
use sea_orm::{ConnectionTrait, DbBackend, Statement};

mod ai_tasks;
mod flow;
mod oauth_feed;

/// Operation-log retention runs once per day; the first run happens at worker startup.
const OPERATION_LOG_CLEANUP_INTERVAL: std::time::Duration = std::time::Duration::from_hours(24);

#[derive(Debug, Parser)]
struct WorkerArgs {
    /// Path of the configuration file. Defaults to `config/sylvode.toml` relative to the working
    /// directory, with `config/openpr.toml` as a legacy fallback. The worker reads no environment
    /// variables, so this file is its only input.
    #[arg(long, value_name = "PATH")]
    config: Option<PathBuf>,
    #[arg(long, default_value_t = 4)]
    concurrency: usize,
}

/// Publishes the configuration file to the API library this binary links, and hands back the
/// installed value.
///
/// The worker is a second process, so it owns its own copy of the API library's process global and
/// has to install into it exactly like `api::main` does. It used to skip this, which left every
/// code path the worker reaches through that library running on `RuntimeConfig::fallback`: local
/// object storage under `./uploads` and an empty outbound allowlist. The
/// form export, form import and attachment packaging jobs the polling loop runs are API library
/// code that reads object storage that way, so an operator who pointed `[storage] dir` somewhere
/// else — or switched the backend to S3 — got a worker writing to a different place than the API
/// read from, with nothing on either side reporting a problem.
///
/// The installed value is returned rather than left for the caller to fetch so that startup cannot
/// run jobs against a configuration it forgot to publish.
fn install_runtime_config(config: &OpenPrConfig) -> anyhow::Result<&'static RuntimeConfig> {
    // A second call is an error, not a no-op, and a worker that cannot publish its configuration
    // must fail here rather than poll with the fallback settings.
    api::config::install(config).map_err(|err| anyhow::anyhow!("{err}"))?;
    Ok(api::config::runtime())
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let args = WorkerArgs::parse();
    let config = OpenPrConfig::load(args.config.as_deref())?;
    let operation_log_retention_days = i32::try_from(config.audit.operation_log_retention_days)
        .map_err(|_| anyhow::anyhow!("audit.operation_log_retention_days exceeds the worker range"))?;
    // Installed before the logger, as in `api::main`: a configuration this process cannot publish
    // aborts startup instead of leaving the jobs it runs on the fallback settings.
    let runtime = install_runtime_config(&config)?;
    // Initialised only once the file parsed, so a configuration error is reported by the process
    // exit rather than swallowed by a subscriber the file was supposed to describe.
    logging::init(&config.logging, "worker")?;
    // Emitted only now: the logger it goes through did not exist while the file was loading.
    if let Some(notice) = platform::config::take_legacy_discovery_notice() {
        tracing::warn!("{notice}");
    }
    // from_config first: it reports a missing database url and signing key together, so the
    // operator fixes both in one pass instead of being walked through them one at a time.
    let cfg = AppConfig::from_config(&config, "worker", "0.0.0.0:8081")?;
    let db = connect_db(&config.database_runtime()?).await?;
    api::flow::scope_runtime::spawn(db.clone(), "worker").await?;
    api::flow::scope_runtime::spawn(db.clone(), "maintenance").await?;
    tokio::spawn(oauth_feed::run(db.clone(), runtime.oauth.clone()));
    let state = AppState {
        cfg: cfg.clone(),
        db: db.clone(),
        flow_permission_cache: FlowPermissionCacheSlot::default(),
    };
    // Redirects are disabled so a validated public endpoint cannot bounce the request into the
    // internal network after the target checks have already passed.
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(10))
        .connect_timeout(std::time::Duration::from_secs(5))
        .redirect(reqwest::redirect::Policy::none())
        .build()?;
    // The storage settings are logged because the form jobs below write through them and there is
    // otherwise no way to tell from outside which of the two processes reads which directory.
    // Only the backend name and the local root are logged; the S3 section carries credentials.
    let ai_task_processing_timeout_secs = config.ai_tasks.processing_timeout_secs;
    tracing::info!(
        concurrency = args.concurrency,
        ai_task_processing_timeout_secs,
        app = %cfg.app_name,
        storage_backend = ?runtime.storage.backend,
        storage_dir = %runtime.storage.dir.display(),
        "worker started"
    );

    let shutdown = shutdown_signal();
    tokio::pin!(shutdown);
    let mut next_operation_log_cleanup = tokio::time::Instant::now();

    loop {
        if tokio::time::Instant::now() >= next_operation_log_cleanup {
            match cleanup_bot_operation_logs(&db, operation_log_retention_days).await {
                Ok(deleted) => tracing::info!(
                    deleted,
                    retention_days = operation_log_retention_days,
                    "bot operation log retention completed"
                ),
                Err(error) => tracing::warn!(error = %error, "bot operation log retention failed"),
            }
            next_operation_log_cleanup = tokio::time::Instant::now() + OPERATION_LOG_CLEANUP_INTERVAL;
        }

        // Attempts nobody ended within the processing timeout are taken back first: re-queued
        // tasks are picked up again once their retry delay has passed, exhausted ones are failed.
        // `FOR UPDATE SKIP LOCKED` makes this safe to run from several worker replicas at once.
        match ai_tasks::end_stalled_tasks(&state, ai_task_processing_timeout_secs, args.concurrency).await {
            Ok(0) => {}
            Ok(ended) => tracing::info!(ended, "stalled AI task attempts ended"),
            Err(err) => tracing::warn!(error = %err, "stalled AI task sweep failed"),
        }

        tokio::select! {
            () = &mut shutdown => {
                tracing::info!("worker shutting down");
                break;
            }
            result = ai_tasks::process_pending_tasks(&state, &client, args.concurrency, ai_task_processing_timeout_secs) => {
                if let Err(err) = result {
                    tracing::warn!(error = %err, "task polling failed");
                }
            }
        }

        if let Err(err) =
            api::routes::form::process_pending_form_jobs_from_worker(state.clone(), args.concurrency).await
        {
            tracing::warn!(error = %err, "form job polling failed");
        }

        // Proposal settlement lives here rather than on the API read path, where an unauthorized
        // `GET` used to open a write transaction. The tick is idempotent and guarded by a
        // per-proposal advisory lock, so several worker replicas may run it concurrently.
        if let Err(err) = api::routes::proposal::governance_tick(&state).await {
            tracing::warn!(error = %err, "governance polling failed");
        }

        // ADR-0011 dispatcher: expands pending `event_dispatch` work (Flow's `flow.object.*` /
        // `flow.content.accepted` / `flow.feature.*` events, and any future non-Flow producer)
        // into `event_deliveries`, then sends what is ready. `--concurrency` is the same batch
        // multiplier every other job on this loop already treats it as, not parallelism — see
        // `process_pending_tasks` above. `FOR UPDATE SKIP LOCKED` throughout makes this safe to
        // run from several worker replicas at once, matching every other job here.
        let dispatch_report = api::events::dispatcher::run_tick(&state, &client, args.concurrency).await;
        tracing::debug!(?dispatch_report, "flow event dispatcher tick");
        if api::events::dispatcher::backlog_alert(
            &dispatch_report,
            api::events::dispatcher::OLDEST_PENDING_AGE_ALERT_MS,
        ) {
            tracing::warn!(
                ?dispatch_report,
                threshold_ms = api::events::dispatcher::OLDEST_PENDING_AGE_ALERT_MS,
                "flow event dispatcher backlog exceeds oldest_pending_age threshold"
            );
        }

        // ADR-0009 search projection: content is copied only from the accepted projection table.
        // This is intentionally independent from the event dispatcher; a lost/duplicated event
        // cannot make the rebuildable search index diverge permanently from accepted state.
        match flow::search::run_tick(&db, args.concurrency.saturating_mul(10)).await {
            Ok(changed) => tracing::debug!(changed, "flow projection index tick"),
            Err(error) => tracing::warn!(error = %error, "flow projection index tick failed"),
        }

        match flow::search::run_rebuild_jobs(&db, args.concurrency.saturating_mul(4)).await {
            Ok(completed) => tracing::debug!(completed, "flow search rebuild job tick"),
            Err(error) => tracing::warn!(error = %error, "flow search rebuild job tick failed"),
        }

        match api::flow::scope_operations::run_quarantine_jobs(&db, args.concurrency.saturating_mul(4)).await {
            Ok(completed) => tracing::debug!(completed, "scoped quarantine queue tick"),
            Err(error) => tracing::warn!(%error,"scoped quarantine queue tick failed"),
        }
        match api::flow::scope_operations::run_compaction_jobs(&db, args.concurrency.saturating_mul(4)).await {
            Ok(completed) => tracing::debug!(completed, "scoped compaction queue tick"),
            Err(error) => tracing::warn!(%error,"scoped compaction queue tick failed"),
        }
        match flow::compaction::run_tick(&db, args.concurrency.saturating_mul(4)).await {
            Ok(report) => tracing::debug!(?report, "flow compaction tick"),
            Err(error) => tracing::warn!(error = %error, "flow compaction tick failed"),
        }

        match flow::projection::run_tick(&db, args.concurrency.saturating_mul(4)).await {
            Ok(report) => tracing::debug!(?report, "flow projection rebuild tick"),
            Err(error) => tracing::warn!(error = %error, "flow projection rebuild tick failed"),
        }

        match flow::integrity::run_tick(&db, args.concurrency.saturating_mul(4)).await {
            Ok(report) => tracing::debug!(?report, "flow integrity scan tick"),
            Err(error) => tracing::warn!(error = %error, "flow integrity scan tick failed"),
        }

        match flow::retention::run_tick(&db, args.concurrency.saturating_mul(4)).await {
            Ok(report) => tracing::debug!(?report, "flow irreversible object retention tick"),
            Err(error) => tracing::warn!(error = %error, "flow irreversible object retention tick failed"),
        }

        tokio::select! {
            () = &mut shutdown => {
                tracing::info!("worker shutting down");
                break;
            }
            () = tokio::time::sleep(std::time::Duration::from_secs(5)) => {}
        }
    }

    Ok(())
}

async fn cleanup_bot_operation_logs(db: &sea_orm::DatabaseConnection, retention_days: i32) -> anyhow::Result<u64> {
    let result = db
        .execute(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "DELETE FROM bot_operation_logs WHERE created_at < now() - make_interval(days => $1::int)",
            vec![retention_days.into()],
        ))
        .await?;
    Ok(result.rows_affected())
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
