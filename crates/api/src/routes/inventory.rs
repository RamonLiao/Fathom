use crate::{error::ApiError, state::AppState};
use axum::{extract::State, Json};
use serde::Serialize;
use sqlx::Row;

#[derive(Serialize)]
pub struct Matrix {
    pub matrix_object_id: String,
    pub oracle_id: String,
    pub matrix_version: i64,
    pub mtm: f64,
    pub range_qty: String,               // raw u64, scale unverified — NOT f64
    pub min_strike: f64,
    pub max_strike: f64,
    pub tick_size: f64,
    pub minted_min_strike: Option<f64>,  // NULL = none minted
    pub minted_max_strike: Option<f64>,
    pub page_leaves: serde_json::Value,  // passthrough (raw u64 strings inside)
    pub ingested_at: chrono::DateTime<chrono::Utc>,
}

pub async fn inventory(State(st): State<AppState>) -> Result<Json<Vec<Matrix>>, ApiError> {
    let rows = sqlx::query(
        "SELECT matrix_object_id, oracle_id, matrix_version, mtm, range_qty::text AS range_qty, \
         min_strike, max_strike, tick_size, minted_min_strike, minted_max_strike, \
         page_leaves, ingested_at FROM strike_matrix_latest ORDER BY oracle_id, matrix_object_id",
    )
    .fetch_all(&st.pool)
    .await?;

    let out = rows
        .into_iter()
        .map(|r| Matrix {
            matrix_object_id: r.get("matrix_object_id"),
            oracle_id: r.get("oracle_id"),
            matrix_version: r.get("matrix_version"),
            mtm: r.get("mtm"),
            range_qty: r.get("range_qty"),
            min_strike: r.get("min_strike"),
            max_strike: r.get("max_strike"),
            tick_size: r.get("tick_size"),
            minted_min_strike: r.get("minted_min_strike"),
            minted_max_strike: r.get("minted_max_strike"),
            page_leaves: r.get("page_leaves"),
            ingested_at: r.get("ingested_at"),
        })
        .collect();
    Ok(Json(out))
}
