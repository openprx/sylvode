use axum::{
    Router,
    routing::{get, post},
};
use std::sync::Arc;
use tower_http::trace::TraceLayer;

mod callback;
mod config;
mod dispatcher;
mod handler;
mod signature;
mod tunnel;

pub struct AppState {
    pub config: config::Config,
}

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "openpr_webhook=info".into()),
        )
        .init();

    let config_path = std::env::args().nth(1).unwrap_or_else(|| "config.toml".into());
    let config = match config::Config::load(&config_path) {
        Ok(c) => c,
        Err(e) => {
            tracing::error!("failed to load config from {config_path}: {e}");
            std::process::exit(1);
        }
    };

    let listen = config.server.listen.clone();
    tracing::info!("Loaded {} agent(s)", config.agents.len());

    let state = Arc::new(AppState { config });

    let tunnel_state = Arc::new(state.config.clone());
    if tunnel_state.tunnel_enabled() {
        tokio::spawn(async move {
            tunnel::run_tunnel_loop(tunnel_state).await;
        });
    } else {
        tracing::info!("tunnel subsystem disabled (feature flag or safe mode)");
    }

    let app = Router::new()
        .route("/webhook", post(handler::handle_webhook))
        .route("/health", get(|| async { "ok" }))
        .layer(TraceLayer::new_for_http())
        .with_state(state);

    tracing::info!("openpr-webhook listening on {listen}");
    let listener = match tokio::net::TcpListener::bind(&listen).await {
        Ok(l) => l,
        Err(e) => {
            tracing::error!("failed to bind {listen}: {e}");
            std::process::exit(1);
        }
    };
    if let Err(e) = axum::serve(listener, app).await {
        tracing::error!("server error: {e}");
        std::process::exit(1);
    }
}
