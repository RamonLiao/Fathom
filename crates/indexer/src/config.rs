//! On-chain coordinates (testnet, verified 2026-05-30) + indexer tuning consts.

/// DeepBook Predict package whose `oracle` module events we index.
pub const PACKAGE_ID: &str =
    "0xf5ea2b3749c65d6e56507cc35388719aadb28f9cab873696a2f8687f5c785138";

/// Shared `Predict` object (used by the B-path object poller next round).
pub const PREDICT_OBJECT_ID: &str =
    "0xc8736204d12f0a7277c86388a68bf8a194b0a14c5538ad13f22cbd8e2a38028a";

/// Testnet fullnode (used by the B-path object poller next round).
pub const FULLNODE_URL: &str = "https://fullnode.testnet.sui.io:443";

/// Testnet checkpoint remote store — the formal full-checkpoint object store the
/// ingestion framework streams from (NOT the JSON-RPC fullnode). Calibrate in the
/// live smoke if the host differs.
pub const REMOTE_STORE_URL: &str = "https://checkpoints.testnet.sui.io";

/// Bounded subscriber channel size; doubles as the ingestion backpressure signal.
pub const SUBSCRIBER_CHANNEL_SIZE: usize = 200;

/// How many checkpoints to rewind from the network tip on startup. The oracle
/// emits price/SVI batches roughly once per second, so a small window already
/// contains live events to decode — while staying within the bucket's retention
/// (old checkpoints are pruned). 0 would tail strictly from the tip.
pub const START_BACKFILL_CHECKPOINTS: u64 = 500;

/// Liveness window: if 0 oracle events are seen within this many checkpoints from
/// start, WARN (config drift — e.g. package redeployed → filter matches nothing).
pub const LIVENESS_WINDOW_CHECKPOINTS: u64 = 200;
