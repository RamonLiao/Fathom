//! B-path object poller. Reads the shared `Predict` object's current state via
//! `sui_getObject{showContent}` on a timer and persists each new `object_version`
//! to Postgres. Decoupled from the A-path stream binary (own process, own
//! lifecycle). Network errors are transient (WARN + retry); a parse/schema error
//! is fatal (on-chain layout drift → decode is wrong).

use std::time::Duration;

use anyhow::{Context, Result};
use indexer::config::{FULLNODE_URL, POLL_INTERVAL_SECS, PREDICT_OBJECT_ID};
use indexer::object_state::{insert_predict_state, parse_predict_state};

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt::init();

    // The poller's whole job is to write to the DB; missing URL is fatal up front.
    let database_url = std::env::var("DATABASE_URL")
        .context("DATABASE_URL must be set for the poller")?;
    let pool = indexer::postgres::connect_pool(&database_url)
        .await
        .context("init postgres")?;

    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(10))
        .build()
        .context("build http client")?;

    tracing::info!(
        object = PREDICT_OBJECT_ID,
        interval_s = POLL_INTERVAL_SECS,
        "starting Predict object poller"
    );

    let mut last_version: Option<u64> = None;
    loop {
        tokio::select! {
            _ = tokio::signal::ctrl_c() => {
                tracing::info!("shutdown signal — stopping poller");
                return Ok(());
            }
            _ = tokio::time::sleep(Duration::from_secs(POLL_INTERVAL_SECS)) => {}
        }

        // Transport failures (timeout, connection, non-200) are transient: warn
        // and try again next tick.
        let resp = match fetch_object(&client).await {
            Ok(r) => r,
            Err(e) => {
                tracing::warn!(error = %e, "object fetch failed — retrying next tick");
                continue;
            }
        };

        // A deterministic RPC `result.error` (notExists / deleted / wrong id) is a
        // config error, NOT transient — fail loud rather than WARN-spin forever.
        if let Some(err) = resp.pointer("/result/error") {
            anyhow::bail!("getObject returned an error (check PREDICT_OBJECT_ID): {err}");
        }
        let data = resp
            .pointer("/result/data")
            .with_context(|| match resp.get("error") {
                Some(err) => format!("fullnode JSON-RPC error: {err}"),
                None => "missing result.data in getObject response".to_string(),
            })?;

        // Parse failure = on-chain layout drift = fatal (decode would be wrong).
        let state = parse_predict_state(data).context("parse Predict object")?;

        if last_version == Some(state.object_version) {
            continue; // unchanged; ON CONFLICT would no-op anyway
        }
        insert_predict_state(&pool, &state).await.context("persist state")?;
        // f64 sum (not u64) — the canonical NAV is the NUMERIC view; this is a
        // log-only convenience and must never panic on a debug overflow.
        let nav = (state.vault_balance as f64 + state.vault_total_mtm as f64) / 1e6;
        tracing::info!(
            version = state.object_version,
            nav,
            balance = state.vault_balance,
            "persisted new Predict state"
        );
        last_version = Some(state.object_version);
    }
}

/// Make the `sui_getObject{showContent}` call and return the raw JSON-RPC
/// response. Only TRANSPORT failures (timeout, connection, non-200, unparseable
/// body) are `Err` here — those are transient and the caller retries. The caller
/// inspects `result.error` / `result.data` to distinguish a deterministic RPC
/// error (fatal) from real data.
/// NB: JSON-RPC is officially deprecated (gRPC is GA); empirically still live on
/// testnet 2026-06-21. If sunset, swap this fn for gRPC `GetObject` — the parser
/// is transport-agnostic.
async fn fetch_object(client: &reqwest::Client) -> Result<serde_json::Value> {
    let body = serde_json::json!({
        "jsonrpc": "2.0", "id": 1, "method": "sui_getObject",
        "params": [PREDICT_OBJECT_ID, { "showContent": true }],
    });
    client
        .post(FULLNODE_URL)
        .json(&body)
        .send()
        .await
        .context("POST sui_getObject")?
        .error_for_status()
        .context("fullnode returned error status")?
        .json()
        .await
        .context("parse JSON-RPC response")
}
