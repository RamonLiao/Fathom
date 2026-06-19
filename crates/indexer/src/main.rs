//! Live A-path entry: stream testnet checkpoints via the ingestion framework,
//! filter `oracle` module events for our package, and feed their raw BCS
//! contents to the pure `pipeline`. No Store / DB this round — the framework's
//! pipeline (Processor + Handler + Store) is for the B path + Postgres next
//! round; here we drive the lower-level `IngestionService` directly and emit to
//! stdout via `StdoutSink`.

use std::sync::Arc;

use anyhow::{Context, Result};
use prometheus::Registry;
use url::Url;

use sui_indexer_alt_framework::ingestion::ingestion_client::IngestionClientArgs;
use sui_indexer_alt_framework::ingestion::{ClientArgs, IngestionConfig, IngestionService};

use indexer::config::{
    FULLNODE_URL, PACKAGE_ID, REMOTE_STORE_URL, START_BACKFILL_CHECKPOINTS,
    SUBSCRIBER_CHANNEL_SIZE,
};
use indexer::pipeline::{check_liveness, handle_event, PipelineState};
use indexer::sink::StdoutSink;

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt::init();

    let registry = Registry::new();
    let client_args = ClientArgs {
        ingestion: IngestionClientArgs {
            remote_store_url: Some(
                Url::parse(REMOTE_STORE_URL).context("parse REMOTE_STORE_URL")?,
            ),
            ..Default::default()
        },
        ..Default::default()
    };

    let mut svc = IngestionService::new(
        client_args,
        IngestionConfig::default(),
        Some("transparency_indexer"),
        &registry,
    )
    .context("build IngestionService")?;

    let mut rx = svc.subscribe_bounded(SUBSCRIBER_CHANNEL_SIZE);
    // NB: we deliberately do NOT use `svc.latest_checkpoint_number()`. That reads
    // the remote store's `_metadata/watermark` blob, which the public testnet
    // checkpoint bucket does not publish → it silently returns 0 and we would
    // backfill from genesis. Read the real tip from the fullnode instead.
    let tip = fetch_network_tip().await.context("fetch network tip")?;
    let start = tip.saturating_sub(START_BACKFILL_CHECKPOINTS);
    tracing::info!(
        start,
        tip,
        %REMOTE_STORE_URL,
        "starting oracle event indexer (backfilling from tip - START_BACKFILL_CHECKPOINTS)"
    );

    // Drives checkpoint fetching as background tasks.
    let service = svc.run(start..).await.context("start ingestion")?;

    // Consume the checkpoint stream concurrently with the ingestion service.
    let consumer = tokio::spawn(async move {
        let sink = StdoutSink;
        let mut state = PipelineState::default();
        while let Some(envelope) = rx.recv().await {
            if let Err(e) = process_checkpoint(&envelope, &mut state, &sink) {
                // A decode failure is a loud, fatal schema-drift signal (Rule 12):
                // stop rather than silently skipping oracle data.
                tracing::error!(error = %e, "fatal decode failure — stopping indexer");
                return Err::<(), anyhow::Error>(e);
            }
        }
        Ok(())
    });

    service.main().await.context("ingestion service")?;
    consumer.await.context("consumer task panicked")??;
    Ok(())
}

/// Fetch the latest checkpoint sequence number from the fullnode's JSON-RPC.
/// The result field is a decimal string (`U64` is JSON-encoded as a string).
async fn fetch_network_tip() -> Result<u64> {
    let body = serde_json::json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "sui_getLatestCheckpointSequenceNumber",
        "params": [],
    });
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(10))
        .build()
        .context("build http client")?;
    let resp: serde_json::Value = client
        .post(FULLNODE_URL)
        .json(&body)
        .send()
        .await
        .context("POST sui_getLatestCheckpointSequenceNumber")?
        .error_for_status()
        .context("fullnode returned error status")?
        .json()
        .await
        .context("parse JSON-RPC response")?;

    // A JSON-RPC 2.0 server can return HTTP 200 with an `error` object and no
    // `result`; surface it rather than reporting a misleading "missing result".
    let seq = resp.get("result").and_then(|r| r.as_str()).with_context(|| {
        match resp.get("error") {
            Some(err) => format!("fullnode JSON-RPC error: {err}"),
            None => "missing/non-string `result` in JSON-RPC response".to_string(),
        }
    })?;
    seq.parse::<u64>()
        .with_context(|| format!("parse checkpoint sequence number from {seq:?}"))
}

/// Extract `oracle`-module events for our package from one checkpoint and run
/// them through the pure pipeline.
fn process_checkpoint(
    envelope: &Arc<sui_indexer_alt_framework::ingestion::ingestion_client::CheckpointEnvelope>,
    state: &mut PipelineState,
    sink: &StdoutSink,
) -> Result<()> {
    let checkpoint = &envelope.checkpoint;
    let seq = checkpoint.summary.sequence_number;

    for tx in &checkpoint.transactions {
        let Some(events) = &tx.events else { continue };
        for event in &events.data {
            // Filter: our package (defining address of the event type) + oracle module.
            if event.type_.address.to_canonical_string(true) == PACKAGE_ID
                && event.type_.module.as_str() == "oracle"
            {
                let name = event.type_.name.as_str();
                handle_event(seq, name, &event.contents, state, sink)?;
            }
        }
    }
    check_liveness(seq, state);
    Ok(())
}
