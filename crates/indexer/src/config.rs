//! On-chain coordinates (testnet, verified 2026-05-30) + indexer tuning consts.

/// DeepBook Predict package whose `oracle` module events we index.
pub const PACKAGE_ID: &str =
    "0xf5ea2b3749c65d6e56507cc35388719aadb28f9cab873696a2f8687f5c785138";

/// Shared `Predict` object (used by the B-path object poller next round).
pub const PREDICT_OBJECT_ID: &str =
    "0xc8736204d12f0a7277c86388a68bf8a194b0a14c5538ad13f22cbd8e2a38028a";

/// Testnet fullnode for StoreIngestionClient::new_remote.
pub const FULLNODE_URL: &str = "https://fullnode.testnet.sui.io:443";

/// Liveness window: if 0 oracle events are seen within this many checkpoints from
/// start, WARN (config drift — e.g. package redeployed → filter matches nothing).
pub const LIVENESS_WINDOW_CHECKPOINTS: u64 = 200;
