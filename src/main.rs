use matrix_identity_admin::config::Config;
use matrix_identity_admin::{build_router, build_state};
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt, EnvFilter};

#[tokio::main]
async fn main() {
    // Load .env file if present (ignored if missing).
    dotenvy::dotenv().ok();

    tracing_subscriber::registry()
        .with(EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()))
        .with(tracing_subscriber::fmt::layer())
        .init();

    let config = Config::from_env();

    tracing::info!(bind_addr = %config.bind_addr, "Starting matrix-identity-admin");

    let state = build_state(&config)
        .await
        .expect("Failed to initialise application state");

    let app = build_router(state);

    let bind_addr = config.bind_addr.clone();
    let listener = tokio::net::TcpListener::bind(&bind_addr)
        .await
        .unwrap_or_else(|_| panic!("Failed to bind to {bind_addr}"));

    tracing::info!("Listening on http://{bind_addr}");

    axum::serve(listener, app).await.expect("Server error");
}
