# B-path Object Poller Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** A standalone `poller` binary that reads the shared `Predict` object's vault/limiter state on a timer and persists NAV/utilization/withdrawal inputs to Postgres.

**Architecture:** Independent binary (`crates/indexer/src/bin/poller.rs`), decoupled from the A-path stream binary. A pure parser (`object_state.rs`) turns a `sui_getObject{showContent}` JSON response into a `PredictState`; the poll loop inserts it with `object_version` as the dedup PK. All raw chain integers stored as `NUMERIC`; decoding (NAV/utilization, DUSDC `/1e6`) lives only in the `predict_latest` view.

**Tech Stack:** Rust, reqwest (JSON-RPC, already a dep), sqlx 0.8 (runtime `query().bind()`, no `query!`), serde_json, tokio. **Zero new dependencies. No sui-sdk / sui-types.**

## Global Constraints

- Store raw chain integers as `NUMERIC` (source of truth); decode only in the view. Numeric columns bound as `String` with `$n::numeric` cast (mirror `crates/indexer/src/postgres.rs`).
- Amounts are **DUSDC 6-dec** (`/1e6`) — NOT the oracle 1e9 scale. Mixing = 1000× error.
- Dedup PK = `object_version` (caps rows at distinct states, not poll count).
- Error policy (Rule 12): **network/HTTP error → WARN + retry next tick**; **parse/schema error → loud fatal stop**.
- NAV = `balance + total_mtm` is an UNVERIFIED mirror of `vault::vault_value` (body not decompiled). Store raw so only the view changes if corrected.
- Offline gate must stay green: `cargo test --workspace` (no DB), `cargo clippy --all-targets -- -D warnings`. DB integration tests are `#[sqlx::test]` + `#[ignore]`.
- On-chain coords (verified 2026-06-21): Predict object `0xc8736204d12f0a7277c86388a68bf8a194b0a14c5538ad13f22cbd8e2a38028a`, fullnode `https://fullnode.testnet.sui.io:443`.

---

### Task 1: Pure parser `object_state.rs` + golden/rejection tests

**Files:**
- Create: `crates/indexer/src/object_state.rs`
- Modify: `crates/indexer/src/lib.rs` (add `pub mod object_state;`)

**Interfaces:**
- Consumes: nothing (leaf module).
- Produces:
  - `pub struct PredictState { pub object_version: u64, pub vault_balance: u64, pub vault_total_mtm: u64, pub vault_total_max_payout: u64, pub wl_enabled: bool, pub wl_available: u64, pub wl_capacity: u64, pub wl_refill_rate_per_ms: u64, pub wl_last_updated_ms: u64 }`
  - `pub fn parse_predict_state(data: &serde_json::Value) -> anyhow::Result<PredictState>` — `data` is the `result.data` object of a `sui_getObject` response.

- [ ] **Step 1: Add the module declaration**

In `crates/indexer/src/lib.rs`, add after the existing `pub mod` lines:

```rust
pub mod object_state;
```

- [ ] **Step 2: Write the failing tests**

Create `crates/indexer/src/object_state.rs` with ONLY the tests first (no impl yet):

```rust
//! Pure parser for the shared `Predict` object's `sui_getObject{showContent}`
//! response. `vault` and `withdrawal_limiter` are inline struct fields, so one
//! object read yields every metric input. u64 fields arrive as decimal STRINGS.
//! Any missing/renamed/non-string-u64 field is a loud Err (on-chain layout drift
//! → the decode is wrong → fatal; same philosophy as the A-path decode rule).

#[cfg(test)]
mod tests {
    use super::*;

    // Real getObject{showContent} response (result.data), captured 2026-06-21.
    // Pins the exact field paths and DUSDC scale that break on a package upgrade.
    fn golden() -> serde_json::Value {
        serde_json::json!({
            "version": "910884609",
            "type": "0xf5ea2b...::predict::Predict",
            "content": { "dataType": "moveObject", "fields": {
                "vault": { "type": "0xf5ea2b...::vault::Vault", "fields": {
                    "balance": "1017919271295",
                    "total_mtm": "1481157422",
                    "total_max_payout": "3493960252"
                }},
                "withdrawal_limiter": { "type": "0xf5ea2b...::rate_limiter::RateLimiter", "fields": {
                    "enabled": false,
                    "available": "0",
                    "capacity": "0",
                    "refill_rate_per_ms": "0",
                    "last_updated_ms": "1776383327247"
                }}
            }}
        })
    }

    #[test]
    fn parses_golden_object() {
        let s = parse_predict_state(&golden()).unwrap();
        assert_eq!(s.object_version, 910_884_609);
        assert_eq!(s.vault_balance, 1_017_919_271_295);
        assert_eq!(s.vault_total_mtm, 1_481_157_422);
        assert_eq!(s.vault_total_max_payout, 3_493_960_252);
        assert!(!s.wl_enabled);
        assert_eq!(s.wl_available, 0);
        assert_eq!(s.wl_capacity, 0);
        assert_eq!(s.wl_refill_rate_per_ms, 0);
        assert_eq!(s.wl_last_updated_ms, 1_776_383_327_247);
    }

    #[test]
    fn missing_field_is_loud() {
        // WHY: a package upgrade that renames/drops a field must fail loudly, not
        // silently produce a wrong NAV.
        let mut v = golden();
        v["content"]["fields"]["vault"]["fields"]
            .as_object_mut().unwrap().remove("total_mtm");
        let err = parse_predict_state(&v).unwrap_err().to_string();
        assert!(err.contains("total_mtm"), "error must name the missing field: {err}");
    }

    #[test]
    fn non_string_u64_is_loud() {
        // WHY: showContent encodes u64 as decimal strings. A number (or anything
        // non-string) signals a format change we must not silently coerce.
        let mut v = golden();
        v["content"]["fields"]["vault"]["fields"]["balance"] =
            serde_json::json!(1017919271295u64);
        assert!(parse_predict_state(&v).is_err());
    }
}
```

- [ ] **Step 3: Run tests to verify they fail**

Run: `cargo test -p indexer --lib object_state`
Expected: FAIL — `cannot find function parse_predict_state` / `PredictState` not found.

- [ ] **Step 4: Write the minimal implementation**

Prepend to `crates/indexer/src/object_state.rs` (above the `#[cfg(test)]` block):

```rust
use anyhow::{Context, Result};
use serde_json::Value;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PredictState {
    pub object_version: u64,
    pub vault_balance: u64,
    pub vault_total_mtm: u64,
    pub vault_total_max_payout: u64,
    pub wl_enabled: bool,
    pub wl_available: u64,
    pub wl_capacity: u64,
    pub wl_refill_rate_per_ms: u64,
    pub wl_last_updated_ms: u64,
}

/// Read a decimal-string u64 field, loud on missing/non-string/unparseable.
fn u64_field(obj: &Value, key: &str) -> Result<u64> {
    let s = obj
        .get(key)
        .and_then(Value::as_str)
        .with_context(|| format!("missing or non-string u64 field `{key}`"))?;
    s.parse::<u64>()
        .with_context(|| format!("parse u64 field `{key}` from {s:?}"))
}

fn bool_field(obj: &Value, key: &str) -> Result<bool> {
    obj.get(key)
        .and_then(Value::as_bool)
        .with_context(|| format!("missing or non-bool field `{key}`"))
}

/// Parse the `result.data` object of a `sui_getObject{showContent}` response.
pub fn parse_predict_state(data: &Value) -> Result<PredictState> {
    let object_version = u64_field(data, "version").context("object version")?;
    let fields = data
        .pointer("/content/fields")
        .context("missing content.fields (object has no parsed content)")?;
    let vault = fields
        .pointer("/vault/fields")
        .context("missing vault.fields")?;
    let wl = fields
        .pointer("/withdrawal_limiter/fields")
        .context("missing withdrawal_limiter.fields")?;
    Ok(PredictState {
        object_version,
        vault_balance: u64_field(vault, "balance")?,
        vault_total_mtm: u64_field(vault, "total_mtm")?,
        vault_total_max_payout: u64_field(vault, "total_max_payout")?,
        wl_enabled: bool_field(wl, "enabled")?,
        wl_available: u64_field(wl, "available")?,
        wl_capacity: u64_field(wl, "capacity")?,
        wl_refill_rate_per_ms: u64_field(wl, "refill_rate_per_ms")?,
        wl_last_updated_ms: u64_field(wl, "last_updated_ms")?,
    })
}
```

- [ ] **Step 5: Run tests to verify they pass**

Run: `cargo test -p indexer --lib object_state`
Expected: PASS (3 tests).

- [ ] **Step 6: Clippy + commit**

Run: `cargo clippy -p indexer --all-targets -- -D warnings`
Expected: clean.

```bash
git add crates/indexer/src/object_state.rs crates/indexer/src/lib.rs
git commit -m "feat(poller): pure parse_predict_state from getObject showContent + golden/drift tests"
```

---

### Task 2: Schema migration + view + config const

**Files:**
- Create: `crates/indexer/migrations/0002_predict_state.sql`
- Modify: `crates/indexer/src/config.rs` (append `POLL_INTERVAL_SECS`)

**Interfaces:**
- Consumes: nothing.
- Produces: table `predict_state`, view `predict_latest`, const `pub const POLL_INTERVAL_SECS: u64`.

- [ ] **Step 1: Write the migration**

Create `crates/indexer/migrations/0002_predict_state.sql`:

```sql
-- B-path Predict-object state snapshots. Raw chain integers as NUMERIC (source of
-- truth); decoding (DUSDC /1e6, NAV/utilization) lives only in predict_latest.
-- Dedup key = object_version: it bumps on every mutation, so re-polling an
-- unchanged object is a no-op and row count is capped at distinct states.

CREATE TABLE IF NOT EXISTS predict_state (
  object_version          BIGINT      NOT NULL,
  vault_balance           NUMERIC     NOT NULL,
  vault_total_mtm         NUMERIC     NOT NULL,
  vault_total_max_payout  NUMERIC     NOT NULL,
  wl_enabled              BOOLEAN     NOT NULL,
  wl_available            NUMERIC     NOT NULL,
  wl_capacity             NUMERIC     NOT NULL,
  wl_refill_rate_per_ms   NUMERIC     NOT NULL,
  wl_last_updated_ms      NUMERIC     NOT NULL,
  ingested_at             TIMESTAMPTZ NOT NULL DEFAULT now(),
  PRIMARY KEY (object_version)
);

CREATE OR REPLACE VIEW predict_latest AS
SELECT
  object_version,
  -- NAV: mirrors on-chain vault::vault_value (body NOT decompiled). balance +
  -- total_mtm is our best guess; total_mtm is U64 unsigned so the sign convention
  -- is unverified. If decompile later shows `balance - total_mtm`, fix THIS LINE
  -- only (raw columns are the source of truth → no re-index needed).
  (vault_balance + vault_total_mtm)::float8 / 1e6                  AS nav,
  -- utilization: OUR definition (max_payout / balance), NOT the protocol's
  -- internal spread-utilization.
  vault_total_max_payout::float8 / NULLIF(vault_balance, 0)::float8 AS utilization,
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

- [ ] **Step 2: Append the config const**

Add to the end of `crates/indexer/src/config.rs`:

```rust
/// B-path object poll interval. The Predict object mutates on trades/supply/etc.;
/// 10s gives a usable NAV time series. `object_version` dedup makes a too-fast
/// poll cheap (unchanged → no row), so this is a comfort/cost knob, not correctness.
pub const POLL_INTERVAL_SECS: u64 = 10;
```

- [ ] **Step 3: Verify it compiles (migrate! embeds the dir at build time)**

Run: `cargo build -p indexer`
Expected: success. (`sqlx::migrate!("./migrations")` now embeds both `0001` and `0002`.)

- [ ] **Step 4: Commit**

```bash
git add crates/indexer/migrations/0002_predict_state.sql crates/indexer/src/config.rs
git commit -m "feat(poller): predict_state table + predict_latest view + POLL_INTERVAL_SECS"
```

---

### Task 3: Postgres insert + `#[sqlx::test]` integration

**Files:**
- Modify: `crates/indexer/src/object_state.rs` (add `insert_predict_state`)
- Create: `crates/indexer/tests/predict_state_integration.rs`

**Interfaces:**
- Consumes: `PredictState` (Task 1), `crate::postgres::connect_pool` (existing), `predict_state` table (Task 2).
- Produces: `pub async fn insert_predict_state(pool: &sqlx::PgPool, s: &PredictState) -> anyhow::Result<()>`.

- [ ] **Step 1: Write the insert function**

Append to `crates/indexer/src/object_state.rs` (outside the test module):

```rust
/// Idempotent insert: a repeated `object_version` is a no-op (the object did not
/// change between polls). Numerics bound as String + `$n::numeric`, mirroring the
/// A-path writer (no decimal crate, build needs no DB).
pub async fn insert_predict_state(pool: &sqlx::PgPool, s: &PredictState) -> Result<()> {
    sqlx::query(
        "INSERT INTO predict_state \
         (object_version,vault_balance,vault_total_mtm,vault_total_max_payout,\
          wl_enabled,wl_available,wl_capacity,wl_refill_rate_per_ms,wl_last_updated_ms) \
         VALUES ($1,$2::numeric,$3::numeric,$4::numeric,$5,$6::numeric,$7::numeric,$8::numeric,$9::numeric) \
         ON CONFLICT (object_version) DO NOTHING",
    )
    .bind(s.object_version as i64)
    .bind(s.vault_balance.to_string())
    .bind(s.vault_total_mtm.to_string())
    .bind(s.vault_total_max_payout.to_string())
    .bind(s.wl_enabled)
    .bind(s.wl_available.to_string())
    .bind(s.wl_capacity.to_string())
    .bind(s.wl_refill_rate_per_ms.to_string())
    .bind(s.wl_last_updated_ms.to_string())
    .execute(pool)
    .await
    .context("insert predict_state")?;
    Ok(())
}
```

- [ ] **Step 2: Write the failing integration tests**

Create `crates/indexer/tests/predict_state_integration.rs`:

```rust
//! Runtime-DB integration tests for the B-path poller sink. `#[sqlx::test]`
//! creates an isolated migrated DB per test (needs DATABASE_URL). `#[ignore]`d so
//! the offline `cargo test --workspace` stays green. Live smoke:
//! `cargo test -p indexer --test predict_state_integration -- --ignored`.

use indexer::object_state::{insert_predict_state, PredictState};

fn state(version: u64, balance: u64, mtm: u64) -> PredictState {
    PredictState {
        object_version: version,
        vault_balance: balance,
        vault_total_mtm: mtm,
        vault_total_max_payout: 3_493_960_252,
        wl_enabled: false,
        wl_available: 0,
        wl_capacity: 0,
        wl_refill_rate_per_ms: 0,
        wl_last_updated_ms: 1_776_383_327_247,
    }
}

#[sqlx::test]
#[ignore = "requires DATABASE_URL; run in live smoke with -- --ignored"]
async fn same_version_dedups(pool: sqlx::PgPool) {
    // WHY: polling an unchanged object re-reads the SAME version. The PK +
    // ON CONFLICT must absorb it, else the table grows per-poll not per-mutation.
    let s = state(100, 1_000_000_000, 5);
    insert_predict_state(&pool, &s).await.unwrap();
    insert_predict_state(&pool, &s).await.unwrap();
    let n: i64 = sqlx::query_scalar("SELECT count(*) FROM predict_state")
        .fetch_one(&pool).await.unwrap();
    assert_eq!(n, 1, "same object_version must not insert twice");
}

#[sqlx::test]
#[ignore = "requires DATABASE_URL; run in live smoke with -- --ignored"]
async fn latest_view_picks_max_version_and_decodes(pool: sqlx::PgPool) {
    // WHY: predict_latest must return the newest state and decode DUSDC /1e6.
    // balance=2_000_000 (2.0 DUSDC), mtm=1_000_000 (1.0) → nav=3.0;
    // max_payout=3_000_000 / balance=2_000_000 → utilization=1.5.
    insert_predict_state(&pool, &state(10, 9, 9)).await.unwrap();
    let mut newer = state(20, 2_000_000, 1_000_000);
    newer.vault_total_max_payout = 3_000_000;
    insert_predict_state(&pool, &newer).await.unwrap();

    let (ver, nav, util): (i64, f64, f64) =
        sqlx::query_as("SELECT object_version, nav, utilization FROM predict_latest")
            .fetch_one(&pool).await.unwrap();
    assert_eq!(ver, 20, "latest must be the max object_version");
    assert!((nav - 3.0).abs() < 1e-9, "nav decode wrong: {nav}");
    assert!((util - 1.5).abs() < 1e-9, "utilization decode wrong: {util}");
}
```

- [ ] **Step 3: Verify offline gate ignores them**

Run: `cargo test -p indexer --test predict_state_integration`
Expected: `2 ignored` (no DB needed; offline-clean).

- [ ] **Step 4 (optional, only if a Postgres is reachable): run them live**

Run: `DATABASE_URL=postgres://... cargo test -p indexer --test predict_state_integration -- --ignored`
Expected: 2 passed.

- [ ] **Step 5: Clippy + commit**

Run: `cargo clippy -p indexer --all-targets -- -D warnings`
Expected: clean.

```bash
git add crates/indexer/src/object_state.rs crates/indexer/tests/predict_state_integration.rs
git commit -m "feat(poller): insert_predict_state (version dedup) + sqlx integration tests"
```

---

### Task 4: Poller binary

**Files:**
- Create: `crates/indexer/src/bin/poller.rs`

**Interfaces:**
- Consumes: `config::{PREDICT_OBJECT_ID, FULLNODE_URL, POLL_INTERVAL_SECS}`, `object_state::{parse_predict_state, insert_predict_state}`, `postgres::connect_pool`.
- Produces: binary `poller` (cargo auto-discovers `src/bin/poller.rs`; no Cargo.toml change).

- [ ] **Step 1: Write the binary**

Create `crates/indexer/src/bin/poller.rs`:

```rust
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
```

- [ ] **Step 2: Build + clippy**

Run: `cargo build -p indexer --bin poller && cargo clippy -p indexer --all-targets -- -D warnings`
Expected: builds clean, no warnings.

- [ ] **Step 3: Full offline gate**

Run: `cargo test --workspace`
Expected: all pass; the 2 new `predict_state_integration` tests show `ignored`.

- [ ] **Step 4: Commit**

```bash
git add crates/indexer/src/bin/poller.rs
git commit -m "feat(poller): standalone Predict object poller binary (timer + getObject + dedup insert)"
```

- [ ] **Step 5: Live smoke (requires a reachable Postgres)**

Run:
```bash
createdb predict_poller 2>/dev/null || true
DATABASE_URL=postgres://localhost/predict_poller RUST_LOG=info \
  ./target/debug/poller
```
Expected within one interval: `persisted new Predict state version=… nav=… balance=…`. Let it run ~30s across ≥2 ticks, then Ctrl-C → `shutdown signal — stopping poller`.

Verify the view:
```bash
psql predict_poller -c "SELECT object_version, nav, utilization, withdrawal_available, wl_enabled FROM predict_latest;"
```
Expected: one row; `nav` ≈ balance+mtm in DUSDC (~1.0e6 range), `utilization` small positive, `withdrawal_available` NULL (testnet `wl_enabled=false`).

- [ ] **Step 6: Monkey tests**

1. **Wrong object id → loud parse error (fatal):** temporarily edit `config::PREDICT_OBJECT_ID` to a non-existent id (e.g. flip last hex digit), `cargo run --bin poller`. `getObject` returns `data: null` → `fetch_object` errors? No — it returns `result.data = null`, so `parse_predict_state` hits `missing content.fields` → **fatal stop**, process exits non-zero. Confirm: `echo $?` ≠ 0. Revert the id.
   - NB (lessons 2026-06-20): measure exit code on the built binary WITHOUT a pipe: `./target/debug/poller; echo $?`. Never `cargo run | head`.
2. **Idempotent across restart:** run the poller ~15s, Ctrl-C, note `count(*)` from `predict_state`; restart, let it run another 15s with the object unchanged. Row count must NOT grow per-poll (only on a real `object_version` bump). `psql -c "SELECT count(*), count(DISTINCT object_version) FROM predict_state;"` → counts equal.

---

## Self-Review

**Spec coverage:**
- Goal (NAV/utilization/withdrawal from Predict object) → Tasks 1–4. ✓
- `getObject{showContent}` hybrid read, inline vault+limiter → Task 4 `fetch_object`, Task 1 parser. ✓
- Raw NUMERIC + view decode (DUSDC /1e6), NAV/utilization/withdrawal formulas → Task 2 view. ✓
- `object_version` dedup → Task 2 PK, Task 3 insert + test. ✓
- Independent binary, 10s timer, no channel → Task 4. ✓
- Error policy (network WARN / parse fatal) → Task 4 loop. ✓
- Tests: golden + rejection (Task 1), dedup + view decode (Task 3), live + monkey (Task 4). ✓
- F1 JSON-RPC migration note → Task 4 `fetch_object` doc comment. ✓
- F3 chain-time limitation, F5 threat-model → documented in spec (no code action). ✓
- Per-strike inventory → explicitly deferred (not in plan). ✓

**Placeholder scan:** none — every code step is complete.

**Type consistency:** `PredictState` field names identical across Tasks 1/3/4; `parse_predict_state(&Value)`, `insert_predict_state(&PgPool, &PredictState)` signatures match consumers. ✓
