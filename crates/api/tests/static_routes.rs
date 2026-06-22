use api::{build_router, state::AppState};
use axum::body::Body;
use axum::http::{Request, StatusCode};
use sqlx::PgPool;
use tower::ServiceExt;

#[sqlx::test]
#[ignore]
async fn api_health_not_shadowed_by_static(pool: PgPool) {
    let app = build_router(AppState { pool });
    let resp = app
        .oneshot(Request::builder().uri("/api/health").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK); // not intercepted by ServeDir/fallback
}

#[sqlx::test]
#[ignore]
async fn unknown_api_path_returns_404(pool: PgPool) {
    let app = build_router(AppState { pool });
    let resp = app
        .oneshot(Request::builder().uri("/api/does-not-exist").body(Body::empty()).unwrap())
        .await
        .unwrap();
    // Unknown /api/* must 404, not fall through to the SPA index.html (200).
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}
