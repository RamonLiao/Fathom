pub mod error;
pub mod routes;
pub mod state;

use axum::{routing::get, Router};
use state::AppState;

pub fn build_router(state: AppState) -> Router {
    Router::new()
        .route("/api/health", get(|| async { "ok" }))
        .route("/api/vault", get(routes::vault::vault))
        .route("/api/oracles", get(routes::oracles::oracles))
        .route("/api/inventory", get(routes::inventory::inventory))
        .with_state(state)
}
