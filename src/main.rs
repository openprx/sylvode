use axum::{
    routing::{get, post},
    Router,
};
use std::sync::Arc;
use tower_http::trace::TraceLayer;

mod config;
mod dispatcher;
mod handler;
mod signature;

pub struct AppState {
    pub config: config::Config,
}

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "openpr_webhook=info".into()),
        )
        .init();

    let config_path = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "config.toml".into());
    let config = config::Config::load(&config_path);

    let listen = config.server.listen.clone();
    tracing::info!("Loaded {} agent(s)", config.agents.len());

    let state = Arc::new(AppState { config });

    let app = Router::new()
        .route("/webhook", post(handler::handle_webhook))
        .route("/health", get(|| async { "ok" }))
        .layer(TraceLayer::new_for_http())
        .with_state(state);

    tracing::info!("openpr-webhook listening on {}", listen);
    let listener = tokio::net::TcpListener::bind(&listen).await.unwrap();
    axum::serve(listener, app).await.unwrap();
}
