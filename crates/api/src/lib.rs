pub mod error;
pub mod routes;
pub mod state;

use axum::http::StatusCode;
use axum::{routing::get, Router};
use state::AppState;
use std::env;
use tower_http::cors::{Any, CorsLayer};
use tower_http::services::{ServeDir, ServeFile};

pub fn build_router(state: AppState) -> Router {
    // Nested under /api with its own 404 fallback so unknown /api/* paths
    // return 404 instead of silently falling through to the SPA index.html.
    let api = Router::new()
        .route("/health", get(|| async { "ok" }))
        .route("/vault", get(routes::vault::vault))
        .route("/oracles", get(routes::oracles::oracles))
        .route("/inventory", get(routes::inventory::inventory))
        .fallback(|| async { (StatusCode::NOT_FOUND, "not found") })
        .with_state(state);

    let web_dist = env::var("WEB_DIST").unwrap_or_else(|_| "web/dist".to_string());
    let index = format!("{web_dist}/index.html");
    let static_svc = ServeDir::new(&web_dist).fallback(ServeFile::new(index));

    let mut app = Router::new().nest("/api", api).fallback_service(static_svc);
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
