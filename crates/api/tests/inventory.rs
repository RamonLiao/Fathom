use api::{build_router, state::AppState};
use axum::body::Body;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use sqlx::PgPool;
use tower::ServiceExt;

async fn get_json(pool: PgPool, uri: &str) -> (StatusCode, serde_json::Value) {
    sqlx::migrate!("../indexer/migrations").run(&pool).await.unwrap();
    let app = build_router(AppState { pool });
    let resp = app
        .oneshot(Request::builder().uri(uri).body(Body::empty()).unwrap())
        .await
        .unwrap();
    let status = resp.status();
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    (status, serde_json::from_slice(&bytes).unwrap())
}

async fn seed_matrix(pool: &PgPool, id: &str, minted_min: &str) {
    sqlx::query(
        "INSERT INTO strike_matrix_state (matrix_object_id, oracle_id, matrix_version, mtm, \
         range_qty, min_strike, max_strike, minted_min_strike, minted_max_strike, tick_size, page_leaves) \
         VALUES ($1, '0xorc', 7, 1000000, 18446744073709551615, 50000000000000, 150000000000000, \
         $2, 140000000000000, 1000000000, '[{\"q_up\":\"123\",\"q_dn\":\"45\"}]'::jsonb)",
    )
    .bind(id)
    .bind(minted_min)
    .execute(pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO oracle_matrix_listing (matrix_object_id, oracle_id, last_version) VALUES ($1, '0xorc', 7)",
    )
    .bind(id)
    .execute(pool)
    .await
    .unwrap();
}

#[sqlx::test]
#[ignore]
async fn inventory_keeps_range_qty_raw_and_passes_page_leaves(pool: PgPool) {
    seed_matrix(&pool, "0xm1", "60000000000000").await;
    let (status, json) = get_json(pool, "/api/inventory").await;
    assert_eq!(status, StatusCode::OK);
    let row = &json.as_array().unwrap()[0];
    // range_qty is the raw u64 18446744073709551615 — MUST be a string, not a lossy float
    assert_eq!(row["range_qty"].as_str().unwrap(), "18446744073709551615");
    assert_eq!(row["min_strike"].as_f64().unwrap(), 50000.0); // /1e9
    assert_eq!(row["page_leaves"][0]["q_up"].as_str().unwrap(), "123");
    assert_eq!(row["minted_min_strike"].as_f64().unwrap(), 60000.0);
    assert_eq!(row["minted_max_strike"].as_f64().unwrap(), 140000.0); // 140_000_000_000_000 / 1e9
}

#[sqlx::test]
#[ignore]
async fn inventory_nulls_minted_when_sentinel(pool: PgPool) {
    // minted_min_strike = u64::MAX sentinel → view maps to NULL ("none minted")
    seed_matrix(&pool, "0xm2", "18446744073709551615").await;
    let (_s, json) = get_json(pool, "/api/inventory").await;
    let row = &json.as_array().unwrap()[0];
    assert!(row["minted_min_strike"].is_null());
    assert!(row["minted_max_strike"].is_null());
}
