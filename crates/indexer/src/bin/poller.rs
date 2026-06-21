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

        // Network failures are transient: warn and try again next tick.
        let data = match fetch_object(&client).await {
            Ok(d) => d,
            Err(e) => {
                tracing::warn!(error = %e, "object fetch failed — retrying next tick");
                continue;
            }
        };

        // Parse failure = on-chain layout drift = fatal (decode would be wrong).
        let state = parse_predict_state(&data).context("parse Predict object")?;

        if last_version == Some(state.object_version) {
            continue; // unchanged; ON CONFLICT would no-op anyway
        }
        insert_predict_state(&pool, &state).await.context("persist state")?;
        let nav = (state.vault_balance + state.vault_total_mtm) as f64 / 1e6;
        tracing::info!(
            version = state.object_version,
            nav,
            balance = state.vault_balance,
            "persisted new Predict state"
        );
        last_version = Some(state.object_version);
    }
}

/// Fetch the Predict object's parsed content (`result.data`) via JSON-RPC.
/// NB: JSON-RPC is officially deprecated (gRPC is GA); empirically still live on
/// testnet 2026-06-21. If sunset, swap this fn for gRPC `GetObject` — the parser
/// is transport-agnostic.
async fn fetch_object(client: &reqwest::Client) -> Result<serde_json::Value> {
    let body = serde_json::json!({
        "jsonrpc": "2.0", "id": 1, "method": "sui_getObject",
        "params": [PREDICT_OBJECT_ID, { "showContent": true }],
    });
    let resp: serde_json::Value = client
        .post(FULLNODE_URL)
        .json(&body)
        .send()
        .await
        .context("POST sui_getObject")?
        .error_for_status()
        .context("fullnode returned error status")?
        .json()
        .await
        .context("parse JSON-RPC response")?;

    // JSON-RPC 2.0 can return HTTP 200 with an `error` object and no `result`.
    resp.get("result")
        .and_then(|r| r.get("data"))
        .cloned()
        .with_context(|| match resp.get("error") {
            Some(err) => format!("fullnode JSON-RPC error: {err}"),
            None => "missing result.data in getObject response".to_string(),
        })
}
