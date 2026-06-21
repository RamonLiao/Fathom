use crate::{error::ApiError, state::AppState};
use axum::{extract::State, Json};
use serde::Serialize;
use sqlx::Row;

#[derive(Serialize)]
pub struct Oracle {
    pub oracle_id: String,
    pub a: Option<f64>,
    pub b: Option<f64>,
    pub rho: Option<f64>,
    pub m: Option<f64>,
    pub sigma: Option<f64>,
    pub svi_sanity: Option<String>,
    pub svi_checkpoint_seq: Option<i64>,
    pub spot: Option<f64>,
    pub forward: Option<f64>,
    pub prices_checkpoint_seq: Option<i64>,
}

pub async fn oracles(State(st): State<AppState>) -> Result<Json<Vec<Oracle>>, ApiError> {
    let rows = sqlx::query(
        "SELECT oracle_id, a, b, rho, m, sigma, svi_sanity, svi_checkpoint_seq, \
         spot, forward, prices_checkpoint_seq FROM oracle_latest ORDER BY oracle_id",
    )
    .fetch_all(&st.pool)
    .await?;

    let out = rows
        .into_iter()
        .map(|r| Oracle {
            oracle_id: r.get("oracle_id"),
            a: r.get("a"),
            b: r.get("b"),
            rho: r.get("rho"),
            m: r.get("m"),
            sigma: r.get("sigma"),
            svi_sanity: r.get("svi_sanity"),
            svi_checkpoint_seq: r.get("svi_checkpoint_seq"),
            spot: r.get("spot"),
            forward: r.get("forward"),
            prices_checkpoint_seq: r.get("prices_checkpoint_seq"),
        })
        .collect();
    Ok(Json(out))
}
