pub mod error;
pub mod routes;
pub mod state;

use axum::{routing::get, Router};
use state::AppState;
use std::env;
use tower_http::cors::{Any, CorsLayer};
use tower_http::services::{ServeDir, ServeFile};

pub fn build_router(state: AppState) -> Router {
    let api = Router::new()
        .route("/api/health", get(|| async { "ok" }))
        .route("/api/vault", get(routes::vault::vault))
        .route("/api/oracles", get(routes::oracles::oracles))
        .route("/api/inventory", get(routes::inventory::inventory))
        .with_state(state);

    let web_dist = env::var("WEB_DIST").unwrap_or_else(|_| "web/dist".to_string());
    let index = format!("{web_dist}/index.html");
    let static_svc = ServeDir::new(&web_dist).fallback(ServeFile::new(index));

    let mut app = api.fallback_service(static_svc);
    if env::var("CORS_DEV").as_deref() == Ok("1") {
        app = app.layer(
            CorsLayer::new()
                .allow_origin(Any)
                .allow_methods(Any)
                .allow_headers(Any),
        );
    }
    app
}
