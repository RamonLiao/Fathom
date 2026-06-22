use api::{build_router, state::AppState};
use axum::body::Body;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use sqlx::PgPool;
use tower::ServiceExt;

async fn get_json(pool: PgPool, uri: &str) -> (StatusCode, serde_json::Value) {
    sqlx::migrate!("../indexer/migrations").run(&pool).await.unwrap();
    let app = build_router(AppState { pool });
    let resp = app.oneshot(Request::builder().uri(uri).body(Body::empty()).unwrap()).await.unwrap();
    let status = resp.status();
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    (status, serde_json::from_slice(&bytes).unwrap())
}

#[sqlx::test]
#[ignore]
async fn prices_only_oracle_has_null_svi(pool: PgPool) {
    sqlx::query(
        "INSERT INTO prices_update (tx_digest, event_index, checkpoint_seq, oracle_id, spot, forward, ts_chain_ms) \
         VALUES ('tx1', 0, 100, '0xaaa', 63000000000000, 63010000000000, 1)",
    ).execute(&pool).await.unwrap();

    let (status, json) = get_json(pool, "/api/oracles").await;
    assert_eq!(status, StatusCode::OK);
    let row = &json.as_array().unwrap()[0];
    assert_eq!(row["oracle_id"], "0xaaa");
    assert_eq!(row["spot"].as_f64().unwrap(), 63000.0); // /1e9
    assert!(row["a"].is_null());                        // no SVI yet
    assert!(row["svi_sanity"].is_null());
}

#[sqlx::test]
#[ignore]
async fn svi_only_oracle_has_null_prices_and_keeps_sign(pool: PgPool) {
    // rho stored signed (negative) in raw NUMERIC; view divides by 1e9 preserving sign
    sqlx::query(
        "INSERT INTO svi_update (tx_digest, event_index, checkpoint_seq, oracle_id, a, b, sigma, rho, m, ts_chain_ms, sanity) \
         VALUES ('tx2', 0, 101, '0xbbb', 7000, 190000, 1000, -400000000, -450000, 1, 'clean')",
    ).execute(&pool).await.unwrap();

    let (_status, json) = get_json(pool, "/api/oracles").await;
    let row = &json.as_array().unwrap()[0];
    assert_eq!(row["oracle_id"], "0xbbb");
    assert!(row["spot"].is_null());                       // prices-only columns NULL
    assert!(row["rho"].as_f64().unwrap() < 0.0);          // sign preserved
    assert_eq!(row["svi_sanity"], "clean");
}
