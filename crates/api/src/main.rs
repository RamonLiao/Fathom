mod state;

use axum::{routing::get, Router};
use state::AppState;
use std::env;

pub fn build_router(state: AppState) -> Router {
    Router::new()
        .route("/api/health", get(|| async { "ok" }))
        .with_state(state)
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .init();

    let database_url = env::var("DATABASE_URL")
        .map_err(|_| anyhow::anyhow!("DATABASE_URL must be set"))?;
    let bind = env::var("API_BIND").unwrap_or_else(|_| "0.0.0.0:8080".to_string());

    let pool = sqlx::postgres::PgPoolOptions::new()
        .max_connections(5)
        .connect(&database_url)
        .await?;

    let app = build_router(AppState { pool });
    let listener = tokio::net::TcpListener::bind(&bind).await?;
    tracing::info!("api listening on {bind}");
    axum::serve(listener, app).await?;
    Ok(())
}
