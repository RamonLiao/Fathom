# B-path Object Poller — Design Spec

_Date: 2026-06-21 · Status: approved, ready for plan_

## Goal

Index the shared `Predict` object's vault/risk state into Postgres so the dashboard can show
**NAV**, **utilization**, and **withdrawal availability**. This is the "B path" — distinct from the
A path (oracle event stream): there are no events for vault state (the PLP module emits none), so the
only source is reading the object's current state.

**This round:** NAV + utilization + withdrawal. Per-strike inventory
(`vault.oracle_matrices: Table<ID, StrikeMatrix>`) is **deferred** — it is a Sui `Table` (dynamic
fields), so it needs `getDynamicFields` + per-entry fetch, an order of magnitude more work.

## On-chain facts (live-verified 2026-06-21 via `sui_getObject{showContent}`)

Object: `0xc8736204d12f0a7277c86388a68bf8a194b0a14c5538ad13f22cbd8e2a38028a`
(type `…::predict::Predict`, version 910884609 at verification time).

`vault` and `withdrawal_limiter` are **inline struct fields** of `Predict` → a single
`getObject{showContent}` returns all three metrics' raw inputs, zero dynamic-field traversal.

```
Predict.vault (…::vault::Vault):
  balance           = "1017919271295"   U64, DUSDC 6-dec  (~1,017,919 DUSDC)
  total_mtm         = "1481157422"      U64 unsigned      (~1,481)
  total_max_payout  = "3493960252"      U64               (~3,494)
  oracle_matrices   = Table<ID,StrikeMatrix>  size=23     ← DEFERRED (dynamic fields)
  settled_oracles   = Table              size=4644        ← not needed
  balances          = Bag                size=1           ← not needed

Predict.withdrawal_limiter (…::rate_limiter::RateLimiter):
  enabled            = false
  available          = "0"      U64
  capacity           = "0"      U64
  refill_rate_per_ms = "0"      U64
  last_updated_ms    = "1776383327247"  U64
```

`showContent` returns parsed Move fields as JSON with `u64` encoded as **decimal strings**.
(This is the still-supported `getObject` JSON-RPC path — unrelated to the retired checkpoint-stream
`SuiEvent.parsed_json`.)

## Architecture

New independent binary `crates/indexer/src/bin/poller.rs`. Independent lifecycle from the A-path
stream binary — runs and fails on its own. Reuses the existing `config.rs`
(`PREDICT_OBJECT_ID`, `FULLNODE_URL` already present) and `reqwest` (already a dependency).
**Zero new dependencies; no sui-sdk / sui-types.**

- **`crates/indexer/src/object_state.rs` (new, pure):**
  - `PredictState` — raw `u64` fields + `object_version: u64`.
  - `parse_predict_state(&serde_json::Value) -> anyhow::Result<PredictState>` — pure, testable with a
    golden JSON fixture. Loud `Err` on any missing field or non-string-u64 (schema drift).
- **Poll loop:** every `POLL_INTERVAL_SECS` (config const, default 10) →
  `getObject{showContent}` → `parse_predict_state` → `INSERT … ON CONFLICT (object_version) DO NOTHING`.
  Low frequency (~0.1 Hz) → **no bounded-channel/writer-task machinery** (unlike the A-path firehose);
  direct insert per poll (Rule 2: don't over-engineer).
- Writer: a small `connect_pool` + parameterized insert. Reuse `crates/indexer/src/postgres.rs`
  patterns (sqlx 0.8, rustls, runtime `query().bind()`, numeric via `$n::numeric` string cast — no
  `query!`, no DB at build time).

## Schema (`migrations/0002_predict_state.sql`)

Same philosophy as the A path: **store raw chain integers as `NUMERIC` (source of truth); all decoding
lives only in the view.** Dedup key = `object_version` (bumps on every object mutation → re-polling an
unchanged object is absorbed by `ON CONFLICT DO NOTHING`, naturally idempotent).

```sql
CREATE TABLE IF NOT EXISTS predict_state (
  object_version        BIGINT      NOT NULL,
  vault_balance         NUMERIC     NOT NULL,
  vault_total_mtm       NUMERIC     NOT NULL,
  vault_total_max_payout NUMERIC    NOT NULL,
  wl_enabled            BOOLEAN     NOT NULL,
  wl_available          NUMERIC     NOT NULL,
  wl_capacity           NUMERIC     NOT NULL,
  wl_refill_rate_per_ms NUMERIC     NOT NULL,
  wl_last_updated_ms    NUMERIC     NOT NULL,
  ingested_at           TIMESTAMPTZ NOT NULL DEFAULT now(),
  PRIMARY KEY (object_version)
);

CREATE OR REPLACE VIEW predict_latest AS
SELECT
  object_version,
  -- NAV: mirrors the on-chain `vault::vault_value(&Vault)`. Body NOT decompiled —
  -- balance + total_mtm is our best guess of the formula. total_mtm is U64 unsigned
  -- so sign convention is unverified; if decompile later shows `balance - total_mtm`,
  -- fix THIS LINE only (raw columns are the source of truth, no re-index needed).
  (vault_balance + vault_total_mtm)::float8 / 1e6        AS nav,
  -- utilization: OUR definition (max_payout / balance), NOT the protocol's internal
  -- spread-utilization (utilization_multiplier / max_total_exposure_pct).
  vault_total_max_payout::float8 / NULLIF(vault_balance,0)::float8  AS utilization,
  vault_balance::float8          / 1e6 AS balance,
  vault_total_mtm::float8        / 1e6 AS total_mtm,
  vault_total_max_payout::float8 / 1e6 AS total_max_payout,
  -- withdrawal_available: OUR mirror. enabled=false → unlimited → NULL.
  CASE WHEN wl_enabled THEN wl_available::float8 / 1e6 ELSE NULL END AS withdrawal_available,
  wl_enabled,
  ingested_at
FROM predict_state
ORDER BY object_version DESC
LIMIT 1;
```

**Scale gotcha:** amounts are **DUSDC 6-dec** (`/1e6`) — NOT the oracle 1e9 scale of the A path.
Mixing them is a 1000× error.

## Error handling (Rule 12 — distinguish transient vs drift)

- **Network / HTTP error** (timeout, connection, non-200) → `WARN`, retry next tick. A transient
  fullnode hiccup must not kill the poller.
- **Parse / schema error** (missing field, renamed field, non-string-u64) → **loud fatal stop**. This
  means the on-chain layout changed (package upgrade) and the decode is now wrong — needs human
  attention, same philosophy as the A-path decode-error-is-fatal rule.

## Testing (Rule 9 — encode WHY)

- **Golden JSON fixture** = the real `getObject` response captured 2026-06-21 → `parse_predict_state`
  golden test. Pins the exact field paths and the DUSDC scale (these are what break on upgrade).
- **Rejection tests:** missing field and non-string-u64 → loud `Err` (schema drift must be loud).
- **`#[sqlx::test]` integration** (gated `#[ignore]` for offline-clean `cargo test --workspace`,
  consistent with the A-path follow-up): insert two versions; re-insert same version is a no-op (dedup);
  `predict_latest` returns the max-version row with correct `nav` / `utilization`.
- **Monkey:** wrong object id → loud error; polling an unchanged object repeatedly → no duplicate rows.

## Decisions locked

- Metrics this round: NAV + utilization + withdrawal. Per-strike inventory deferred.
- Read mechanism: `getObject{showContent}` for all three (vault + limiter inline) + raw fields stored;
  no devInspect, no PTB, no sui-sdk.
- Withdrawal from RateLimiter fields directly (testnet `enabled=false` → reported as unlimited/NULL),
  not from `predict::available_withdrawal` devInspect.
- Independent binary, timer poll (10s default), `object_version` dedup, direct insert (no channel).
- NAV formula `balance + total_mtm` is **unverified** (vault_value body not decompiled); raw columns
  stored so the view can be corrected without re-indexing.
