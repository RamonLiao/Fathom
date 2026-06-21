use crate::{error::ApiError, state::AppState};
use axum::{extract::State, Json};
use serde::Serialize;
use sqlx::Row;

#[derive(Serialize)]
pub struct Vault {
    pub object_version: i64,
    pub nav: f64,
    pub utilization: Option<f64>,         // view guards div-by-zero → NULL
    pub balance: f64,
    pub total_mtm: f64,
    pub total_max_payout: f64,
    pub withdrawal_available: Option<f64>, // NULL when wl_enabled=false
    pub wl_enabled: bool,
    pub ingested_at: chrono::DateTime<chrono::Utc>,
}

pub async fn vault(State(st): State<AppState>) -> Result<Json<Option<Vault>>, ApiError> {
    let row = sqlx::query(
        "SELECT object_version, nav, utilization, balance, total_mtm, \
         total_max_payout, withdrawal_available, wl_enabled, ingested_at \
         FROM predict_latest",
    )
    .fetch_optional(&st.pool)
    .await?;

    let v = row.map(|r| Vault {
        object_version: r.get("object_version"),
        nav: r.get("nav"),
        utilization: r.get("utilization"),
        balance: r.get("balance"),
        total_mtm: r.get("total_mtm"),
        total_max_payout: r.get("total_max_payout"),
        withdrawal_available: r.get("withdrawal_available"),
        wl_enabled: r.get("wl_enabled"),
        ingested_at: r.get("ingested_at"),
    });
    Ok(Json(v))
}
