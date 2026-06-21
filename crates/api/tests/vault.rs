use api::{build_router, state::AppState};
use axum::body::Body;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use sqlx::PgPool;
use tower::ServiceExt;

async fn get(pool: PgPool, uri: &str) -> (StatusCode, serde_json::Value) {
    // migrations live under the indexer crate
    sqlx::migrate!("../indexer/migrations").run(&pool).await.unwrap();
    let app = build_router(AppState { pool });
    let resp = app
        .oneshot(Request::builder().uri(uri).body(Body::empty()).unwrap())
        .await
        .unwrap();
    let status = resp.status();
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    let json = if bytes.is_empty() { serde_json::Value::Null }
               else { serde_json::from_slice(&bytes).unwrap() };
    (status, json)
}

#[sqlx::test]
#[ignore] // live: needs DATABASE_URL; run with `cargo test -p api -- --ignored`
async fn vault_empty_returns_null_not_500(pool: PgPool) {
    let (status, json) = get(pool, "/api/vault").await;
    assert_eq!(status, StatusCode::OK);
    assert!(json.is_null(), "empty predict_state must serialize to null");
}

#[sqlx::test]
#[ignore]
async fn vault_decodes_and_nulls_unlimited_withdrawal(pool: PgPool) {
    // wl_enabled=false → view emits withdrawal_available NULL (unlimited)
    sqlx::query(
        "INSERT INTO predict_state (object_version, vault_balance, vault_total_mtm, \
         vault_total_max_payout, wl_enabled, wl_available, wl_capacity, \
         wl_refill_rate_per_ms, wl_last_updated_ms) \
         VALUES (1, 2000000, 1000000, 500000, false, 0, 0, 0, 0)",
    ).execute(&pool).await.unwrap();

    let (status, json) = get(pool, "/api/vault").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(json["nav"].as_f64().unwrap(), 3.0);              // (2_000_000+1_000_000)/1e6
    assert_eq!(json["balance"].as_f64().unwrap(), 2.0);
    assert!(json["withdrawal_available"].is_null());            // unlimited
    assert!(!json["wl_enabled"].as_bool().unwrap());
}
