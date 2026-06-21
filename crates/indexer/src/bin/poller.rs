//! B-path object poller. Reads the shared `Predict` object's current state via
//! `sui_getObject{showContent}` on a timer and persists each new `object_version`
//! to Postgres. Decoupled from the A-path stream binary (own process, own
//! lifecycle). Network errors are transient (WARN + retry); a parse/schema error
//! is fatal (on-chain layout drift → decode is wrong).

use std::time::Duration;

use std::collections::{HashMap, HashSet};

use anyhow::{Context, Result};
use indexer::config::{FULLNODE_URL, POLL_INTERVAL_SECS, PREDICT_OBJECT_ID};
use indexer::object_state::{insert_predict_state, parse_predict_state};
use indexer::strike_matrix::{
    chunk_ids, insert_strike_matrix_state, parse_dynamic_fields_page,
    parse_oracle_matrices_table_id, parse_strike_matrices, replace_matrix_listing, DynField,
};

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
    // Per-matrix version dedup, carried across ticks. Pruned each tick to the
    // authoritative getDynamicFields listing so delisted matrices don't leak.
    let mut last_matrix_versions: HashMap<String, u64> = HashMap::new();
    // Server cap for sui_multiGetObjects.
    const MULTI_GET_CAP: usize = 50;
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

        // The Predict object itself only bumps when its inline vault/limiter fields
        // change; skip its insert when unchanged. The matrix step below still polls
        // every tick (StrikeMatrix children version independently of the parent).
        if last_version != Some(state.object_version) {
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

        // ---- B-path per-strike inventory (oracle_matrices dynamic fields) ----
        // Transport failures inside poll_matrices are swallowed there (WARN + early
        // Ok). Reaching the error arm means a deterministic/parse error → fatal
        // (layout drift), consistent with parse_predict_state.
        if let Err(e) =
            poll_matrices(&client, &pool, data, &mut last_matrix_versions, MULTI_GET_CAP).await
        {
            return Err(e.context("poll oracle_matrices"));
        }
    }
}

/// Index the oracle_matrices dynamic fields for this tick. Transport failures
/// (timeout/connection/non-200) are swallowed as WARN + `Ok(())` (retry next tick);
/// the Predict state has already been committed and must not be rolled back. Parse /
/// deterministic-RPC errors propagate as `Err` → fatal (layout drift).
async fn poll_matrices(
    client: &reqwest::Client,
    pool: &sqlx::PgPool,
    predict_data: &serde_json::Value,
    last_versions: &mut HashMap<String, u64>,
    cap: usize,
) -> Result<()> {
    let table_id =
        parse_oracle_matrices_table_id(predict_data).context("read oracle_matrices table id")?;

    // 1. List the full authoritative set (paginate while hasNextPage).
    let mut listing: Vec<DynField> = Vec::new();
    let mut cursor: Option<String> = None;
    loop {
        let resp = match fetch_dynamic_fields(client, &table_id, cursor.as_deref()).await {
            Ok(r) => r,
            Err(e) => {
                tracing::warn!(error = %e, "getDynamicFields failed — retrying next tick");
                return Ok(());
            }
        };
        // getDynamicFields returns a Page directly as `result`; a deterministic RPC
        // failure is a TOP-LEVEL `/error` (unlike getObject's embedded result.error).
        if let Some(err) = resp.pointer("/error") {
            anyhow::bail!("getDynamicFields RPC error (check oracle_matrices table id): {err}");
        }
        let result = resp
            .pointer("/result")
            .context("getDynamicFields missing result")?;
        let (mut items, next) = parse_dynamic_fields_page(result)?;
        listing.append(&mut items);
        match next {
            Some(c) => cursor = Some(c),
            None => break,
        }
    }

    // A wrong/stale table id does NOT return an RPC error — getDynamicFields just
    // yields an empty set. The id is derived from the freshly-parsed Predict object
    // each tick (a renamed field would already be fatal in parse_oracle_matrices_table_id),
    // so an empty listing on a live protocol is an anomaly worth surfacing — WARN,
    // not fatal (a legitimately-empty table must not crash the poller).
    if listing.is_empty() {
        tracing::warn!(
            table = %table_id,
            "oracle_matrices listing empty — config drift or stale table id?"
        );
    }

    // 2. Dedup: fetch only matrices whose listing version advanced.
    let changed: Vec<String> = listing
        .iter()
        .filter(|d| last_versions.get(&d.object_id) != Some(&d.version))
        .map(|d| d.object_id.clone())
        .collect();

    // 3. Fetch changed matrices, chunked to the server cap; parse + persist.
    for chunk in chunk_ids(&changed, cap) {
        let resp = match fetch_objects(client, chunk).await {
            Ok(r) => r,
            Err(e) => {
                tracing::warn!(error = %e, "multiGetObjects failed — retrying next tick");
                return Ok(());
            }
        };
        if let Some(err) = resp.pointer("/error") {
            anyhow::bail!("multiGetObjects RPC error: {err}");
        }
        let objs = resp
            .pointer("/result")
            .and_then(|v| v.as_array())
            .context("multiGetObjects missing result array")?;
        let states = parse_strike_matrices(objs)?;
        for s in &states {
            insert_strike_matrix_state(pool, s)
                .await
                .context("persist strike_matrix_state")?;
            last_versions.insert(s.matrix_object_id.clone(), s.matrix_version);
            tracing::info!(
                matrix = %s.matrix_object_id, oracle = %s.oracle_id,
                version = s.matrix_version, mtm = s.mtm, "persisted strike matrix"
            );
        }
    }

    // 4. Mirror the authoritative set (tombstone) + prune the in-memory map.
    replace_matrix_listing(pool, &listing)
        .await
        .context("replace matrix listing")?;
    let live: HashSet<&str> = listing.iter().map(|d| d.object_id.as_str()).collect();
    last_versions.retain(|k, _| live.contains(k.as_str()));
    Ok(())
}

/// `suix_getDynamicFields(table_id, cursor, 50)` raw JSON-RPC response. Transport
/// failures only are `Err` (caller retries); the caller inspects `result.error`.
async fn fetch_dynamic_fields(
    client: &reqwest::Client,
    table_id: &str,
    cursor: Option<&str>,
) -> Result<serde_json::Value> {
    let body = serde_json::json!({
        "jsonrpc": "2.0", "id": 1, "method": "suix_getDynamicFields",
        "params": [table_id, cursor, 50],
    });
    client
        .post(FULLNODE_URL)
        .json(&body)
        .send()
        .await
        .context("POST getDynamicFields")?
        .error_for_status()
        .context("fullnode error status")?
        .json()
        .await
        .context("parse getDynamicFields response")
}

/// `sui_multiGetObjects(ids, {showContent})` raw JSON-RPC response.
async fn fetch_objects(client: &reqwest::Client, ids: &[String]) -> Result<serde_json::Value> {
    let body = serde_json::json!({
        "jsonrpc": "2.0", "id": 1, "method": "sui_multiGetObjects",
        "params": [ids, { "showContent": true }],
    });
    client
        .post(FULLNODE_URL)
        .json(&body)
        .send()
        .await
        .context("POST multiGetObjects")?
        .error_for_status()
        .context("fullnode error status")?
        .json()
        .await
        .context("parse multiGetObjects response")
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
