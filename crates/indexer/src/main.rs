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

use indexer::config::{PACKAGE_ID, REMOTE_STORE_URL, SUBSCRIBER_CHANNEL_SIZE};
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
    let start = svc
        .latest_checkpoint_number()
        .await
        .context("fetch latest checkpoint")?;
    tracing::info!(start, %REMOTE_STORE_URL, "starting oracle event indexer from network tip");

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
