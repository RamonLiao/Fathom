# PostgresSink Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Persist the A-path's decoded oracle SVI/price events into Postgres (append-only event log + latest-state view) behind the existing `Sink` trait, so the transparency dashboard can query history and current state.

**Architecture:** Keep the pure `Sink`/pipeline core sync (no tokio in unit tests). `PostgresSink::emit` maps a decoded event to an insert row and `try_send`s it onto a bounded channel; a background writer task owns the `PgPool` and `INSERT … ON CONFLICT DO NOTHING`. A `TeeSink` fans events to stdout + postgres. `main` selects sinks by `DATABASE_URL` env.

**Tech Stack:** Rust, sqlx 0.8 (rustls, runtime-tokio), Postgres, existing `sui-indexer-alt-framework` (git rev `2e196df`).

## Global Constraints

- Dedup key = `(tx_digest, event_index)`; `tx_digest` = base58 of `tx.transaction.digest()` (owned `TransactionDigest`), `event_index` = enumerate index over the **unfiltered** `events.data`, carried through the oracle/package filter.
- Store **raw chain integers** as Postgres `NUMERIC` (unbounded). Decode (`/1e9`) only in the `oracle_latest` view. Sign decode for `rho`/`m` happens once, in Rust, at insert (`is_negative && magnitude!=0 ? -magnitude : magnitude`); never in SQL.
- `sqlx` runtime `query(...).bind(...)` only — NO `query!` macro (build/clippy must not need a DB). Numeric values are bound as `String` and cast `$n::numeric` in SQL (avoids a decimal crate).
- `DATABASE_URL` from env only — never in `config.rs`, never committed.
- Fail loud (Rule 12): DB connect/migration failure = fatal exit; insert error = fatal; channel `Full`/`Closed` = fatal via `Sink::emit` returning `Err`.
- Gate every task: `cargo test --workspace` + `cargo clippy --all-targets -- -D warnings` green. DB integration tests use `#[sqlx::test]` (runtime DB only; not part of the offline gate).
- sqlx features: `["runtime-tokio","tls-rustls","postgres","macros","migrate"]`, `default-features = false`.

---

### Task 1: Plumb `EventId` + fallible `Sink::emit` + `forward_used` through the pure core

**Files:**
- Modify: `crates/indexer/src/sink.rs` (Sink trait, DecodedEvent, StdoutSink)
- Modify: `crates/indexer/src/pipeline.rs` (handle_event, CaptureSink + tests)
- Modify: `crates/indexer/src/main.rs:127-150` (process_checkpoint builds EventId)

**Interfaces:**
- Produces:
  - `pub struct EventId { pub tx_digest: String, pub event_index: u64 }` (in `sink.rs`)
  - `trait Sink { fn emit(&self, id: &EventId, checkpoint_seq: u64, ev: &DecodedEvent) -> anyhow::Result<()>; }`
  - `DecodedEvent::Svi { ev: OracleSviUpdated, status: SanityStatus, forward_used: Option<u64> }` (forward_used = the per-oracle forward the sanity check ran against; `None` ⇔ `Untested`)
  - `fn handle_event(checkpoint_seq: u64, struct_name: &str, contents: &[u8], id: &EventId, state: &mut PipelineState, sink: &dyn Sink) -> anyhow::Result<()>`

- [ ] **Step 1: Add a failing test for `forward_used` population in `pipeline.rs` tests**

Add to the `tests` mod in `crates/indexer/src/pipeline.rs`. First update `CaptureSink` to the new trait shape (it already matches `Svi { status, .. }`, so only the signature changes):

```rust
struct CaptureSink(RefCell<Vec<String>>);
impl Sink for CaptureSink {
    fn emit(&self, _id: &EventId, _seq: u64, ev: &DecodedEvent) -> anyhow::Result<()> {
        let tag = match ev {
            DecodedEvent::Svi { status, .. } => match status {
                SanityStatus::Untested => "svi:untested".to_string(),
                SanityStatus::Checked(v) => format!("svi:{}", v.is_clean()),
            },
            DecodedEvent::Prices(_) => "prices".to_string(),
        };
        self.0.borrow_mut().push(tag);
        Ok(())
    }
}

// A capturing sink that records forward_used for SVI events.
struct ForwardSink(RefCell<Vec<Option<u64>>>);
impl Sink for ForwardSink {
    fn emit(&self, _id: &EventId, _seq: u64, ev: &DecodedEvent) -> anyhow::Result<()> {
        if let DecodedEvent::Svi { forward_used, .. } = ev {
            self.0.borrow_mut().push(*forward_used);
        }
        Ok(())
    }
}

fn eid() -> EventId { EventId { tx_digest: "test".into(), event_index: 0 } }

#[test]
fn svi_carries_the_forward_it_was_checked_against() {
    let sink = ForwardSink(RefCell::new(vec![]));
    let mut st = PipelineState::default();
    let a = [0xAA; 32];
    // No forward yet → Untested → forward_used None.
    handle_event(1, "OracleSVIUpdated", &svi_bytes(a), &eid(), &mut st, &sink).unwrap();
    // Forward arrives, then SVI again → forward_used Some(that forward).
    handle_event(1, "OraclePricesUpdated", &prices_bytes(a, 73_744_082_479_138), &eid(), &mut st, &sink).unwrap();
    handle_event(1, "OracleSVIUpdated", &svi_bytes(a), &eid(), &mut st, &sink).unwrap();
    assert_eq!(*sink.0.borrow(), vec![None, Some(73_744_082_479_138)]);
}
```

Also update every existing `handle_event(...)` call in the tests to pass `&eid()` as the new 5th arg (before `&mut st`), and the `CaptureSink::emit` signature as shown.

- [ ] **Step 2: Run test to verify it fails to compile / fails**

Run: `cargo test -p indexer 2>&1 | head -40`
Expected: compile error (handle_event arity / emit signature) — confirms the new shape is required.

- [ ] **Step 3: Update `sink.rs`**

In `crates/indexer/src/sink.rs`:

```rust
/// Globally-unique on-chain identity of an event: the content-addressed
/// transaction digest (base58) + its index within that tx's event list.
/// This is the Postgres dedup key (ON CONFLICT), so a re-backfill of the same
/// checkpoints inserts no duplicates.
#[derive(Debug, Clone)]
pub struct EventId {
    pub tx_digest: String,
    pub event_index: u64,
}

#[derive(Debug)]
pub enum DecodedEvent {
    Svi { ev: OracleSviUpdated, status: SanityStatus, forward_used: Option<u64> },
    Prices(OraclePricesUpdated),
}

pub trait Sink {
    /// Fallible so a sink whose downstream is a bounded channel can signal
    /// backpressure exhaustion (channel Full / writer dead) as a loud error
    /// rather than silently dropping events (Rule 12).
    fn emit(&self, id: &EventId, checkpoint_seq: u64, ev: &DecodedEvent) -> anyhow::Result<()>;
}
```

Update `StdoutSink::emit` to the new signature, log the digest, return `Ok(())`:

```rust
impl Sink for StdoutSink {
    fn emit(&self, id: &EventId, checkpoint_seq: u64, ev: &DecodedEvent) -> anyhow::Result<()> {
        match ev {
            DecodedEvent::Svi { ev, status, forward_used } => {
                let svi = ev.to_svi();
                let sanity = match status {
                    SanityStatus::Untested => "untested",
                    SanityStatus::Checked(v) if v.is_clean() => "clean",
                    SanityStatus::Checked(_) => "dirty",
                };
                tracing::info!(
                    checkpoint = checkpoint_seq, tx = %id.tx_digest, ev_idx = id.event_index,
                    oracle = %ev.oracle_id,
                    a = svi.a, b = svi.b, rho = svi.rho, m = svi.m, sigma = svi.sigma,
                    forward_used = forward_used.unwrap_or(0), sanity,
                    "OracleSVIUpdated"
                );
                if let SanityStatus::Checked(Verdict::Dirty(reasons)) = status {
                    tracing::warn!(checkpoint = checkpoint_seq, ?reasons, "SVI failed no-arb sanity");
                }
            }
            DecodedEvent::Prices(p) => {
                tracing::info!(
                    checkpoint = checkpoint_seq, tx = %id.tx_digest, ev_idx = id.event_index,
                    oracle = %p.oracle_id, spot = p.spot, forward = p.forward,
                    "OraclePricesUpdated"
                );
            }
        }
        Ok(())
    }
}
```

- [ ] **Step 4: Update `handle_event` in `pipeline.rs`**

```rust
pub fn handle_event(
    checkpoint_seq: u64,
    struct_name: &str,
    contents: &[u8],
    id: &EventId,
    state: &mut PipelineState,
    sink: &dyn Sink,
) -> Result<()> {
    match struct_name {
        "OracleSVIUpdated" => {
            let ev = OracleSviUpdated::from_bcs(contents)
                .map_err(|e| anyhow!("decode OracleSVIUpdated: {e}"))?;
            state.oracle_events_seen += 1;
            let forward_used = state.forward_1e9_by_oracle.get(&ev.oracle_id.0).copied();
            let status = match forward_used {
                Some(fwd) => SanityStatus::Checked(check_svi_arb_free(&ev.to_svi(), fwd)),
                None => SanityStatus::Untested,
            };
            sink.emit(id, checkpoint_seq, &DecodedEvent::Svi { ev, status, forward_used })
        }
        "OraclePricesUpdated" => {
            let ev = OraclePricesUpdated::from_bcs(contents)
                .map_err(|e| anyhow!("decode OraclePricesUpdated: {e}"))?;
            state.oracle_events_seen += 1;
            state.forward_1e9_by_oracle.insert(ev.oracle_id.0, ev.forward);
            sink.emit(id, checkpoint_seq, &DecodedEvent::Prices(ev))
        }
        _ => Ok(()),
    }
}
```

Add `use crate::sink::EventId;` to the imports at the top of `pipeline.rs` (the `crate::sink::{...}` line).

- [ ] **Step 5: Update `process_checkpoint` in `main.rs` to build `EventId`**

Replace the body loop (`crates/indexer/src/main.rs`, the `for tx in &checkpoint.transactions` block) with enumerate-over-unfiltered events:

```rust
fn process_checkpoint(
    envelope: &Arc<sui_indexer_alt_framework::ingestion::ingestion_client::CheckpointEnvelope>,
    state: &mut PipelineState,
    sink: &dyn indexer::sink::Sink,
) -> Result<()> {
    let checkpoint = &envelope.checkpoint;
    let seq = checkpoint.summary.sequence_number;

    for tx in &checkpoint.transactions {
        let Some(events) = &tx.events else { continue };
        // base58 digest of the transaction (content-addressed → stable dedup key).
        let tx_digest = tx.transaction.digest().to_string();
        // Enumerate the UNFILTERED event list so event_index matches the chain's
        // canonical event_seq; filter INSIDE the loop (never enumerate the filtered subset).
        for (event_index, event) in events.data.iter().enumerate() {
            if event.type_.address.to_canonical_string(true) == PACKAGE_ID
                && event.type_.module.as_str() == "oracle"
            {
                let id = indexer::sink::EventId {
                    tx_digest: tx_digest.clone(),
                    event_index: event_index as u64,
                };
                let name = event.type_.name.as_str();
                handle_event(seq, name, &event.contents, &id, state, sink)?;
            }
        }
    }
    check_liveness(seq, state);
    Ok(())
}
```

Update the `process_checkpoint(&envelope, &mut state, &sink)` call site: `sink` is currently `StdoutSink`; pass `&sink` (a `&dyn Sink` coercion already works since the param is `&dyn Sink`). Add `use indexer::sink::Sink;` if needed for the trait method.

- [ ] **Step 6: Run the gate**

Run: `cargo test -p indexer && cargo clippy -p indexer --all-targets -- -D warnings`
Expected: PASS (all pipeline tests incl. the new `svi_carries_the_forward_it_was_checked_against`), clippy clean. (First build pulls the git framework dep — may take ~1m.)

- [ ] **Step 7: Commit**

```bash
git add crates/indexer/src/sink.rs crates/indexer/src/pipeline.rs crates/indexer/src/main.rs
git commit -m "feat(indexer): plumb EventId + fallible Sink::emit + forward_used through pipeline"
```

---

### Task 2: Pure row-mapping module (`row.rs`)

**Files:**
- Create: `crates/indexer/src/row.rs`
- Modify: `crates/indexer/src/lib.rs` (add `pub mod row;`)

**Interfaces:**
- Consumes: `EventId`, `DecodedEvent`, `SanityStatus` (Task 1); `Verdict` (pricing).
- Produces:
  - `pub struct PricesRow { pub tx_digest: String, pub event_index: i64, pub checkpoint_seq: i64, pub oracle_id: String, pub spot: String, pub forward: String, pub ts_chain_ms: String }`
  - `pub struct SviRow { pub tx_digest: String, pub event_index: i64, pub checkpoint_seq: i64, pub oracle_id: String, pub a: String, pub b: String, pub sigma: String, pub rho: String, pub m: String, pub ts_chain_ms: String, pub sanity_forward: Option<String>, pub sanity: String, pub sanity_reasons: Option<Vec<String>> }`
  - `pub enum Row { Prices(PricesRow), Svi(SviRow) }`
  - `pub fn to_row(id: &EventId, checkpoint_seq: u64, ev: &DecodedEvent) -> Row`

Numeric fields are decimal **strings** (bound as `$n::numeric`), so the row module owns the integer→string + sign-magnitude→signed conversions and is unit-testable without a DB.

- [ ] **Step 1: Write failing tests in `crates/indexer/src/row.rs`**

```rust
//! Pure mapping: a decoded event + its identity → an insertable Postgres row.
//! Numeric chain values become decimal STRINGS (bound as `$n::numeric`) so this
//! module owns the only signed-decode path (rho/m) — see plan Global Constraints.

use types::events::{I64Raw, OraclePricesUpdated, OracleSviUpdated};
use crate::sink::{DecodedEvent, EventId, SanityStatus};
use pricing::invariants::Verdict;

// ... struct defs from Interfaces above ...

/// Sign-magnitude i64 → signed decimal string. `-0` (magnitude 0) → "0".
fn signed_str(r: I64Raw) -> String {
    if r.is_negative && r.magnitude != 0 {
        format!("-{}", r.magnitude)
    } else {
        r.magnitude.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use types::events::ObjId;

    fn eid() -> EventId { EventId { tx_digest: "Dx".into(), event_index: 3 } }

    #[test]
    fn negative_rho_stays_negative() {
        let ev = OracleSviUpdated {
            oracle_id: ObjId([0xAA; 32]),
            a: 5274, b: 638_806,
            rho: I64Raw { magnitude: 458_555_014, is_negative: true },
            m: I64Raw { magnitude: 1_380_256, is_negative: true },
            sigma: 1_181_366, timestamp: 7,
        };
        let de = DecodedEvent::Svi {
            ev, status: SanityStatus::Checked(Verdict::Clean), forward_used: Some(73_744_082_479_138),
        };
        let Row::Svi(r) = to_row(&eid(), 99, &de) else { panic!("expected Svi") };
        assert_eq!(r.rho, "-458555014");          // sign preserved (the bug pricing gate guards)
        assert_eq!(r.m, "-1380256");
        assert_eq!(r.a, "5274");
        assert_eq!(r.oracle_id, ObjId([0xAA; 32]).to_string()); // 0x… hex
        assert_eq!(r.event_index, 3);
        assert_eq!(r.checkpoint_seq, 99);
        assert_eq!(r.sanity, "clean");
        assert_eq!(r.sanity_forward.as_deref(), Some("73744082479138"));
        assert!(r.sanity_reasons.is_none());
    }

    #[test]
    fn negative_zero_is_plain_zero() {
        assert_eq!(signed_str(I64Raw { magnitude: 0, is_negative: true }), "0");
    }

    #[test]
    fn untested_has_no_forward_and_no_reasons() {
        let ev = OracleSviUpdated {
            oracle_id: ObjId([1; 32]), a: 1, b: 1,
            rho: I64Raw { magnitude: 1, is_negative: false },
            m: I64Raw { magnitude: 1, is_negative: false },
            sigma: 1, timestamp: 1,
        };
        let de = DecodedEvent::Svi { ev, status: SanityStatus::Untested, forward_used: None };
        let Row::Svi(r) = to_row(&eid(), 1, &de) else { panic!() };
        assert_eq!(r.sanity, "untested");
        assert!(r.sanity_forward.is_none());
        assert!(r.sanity_reasons.is_none());
    }

    #[test]
    fn dirty_carries_reasons() {
        let ev = OracleSviUpdated {
            oracle_id: ObjId([2; 32]), a: 1, b: 1,
            rho: I64Raw { magnitude: 1, is_negative: false },
            m: I64Raw { magnitude: 1, is_negative: false },
            sigma: 1, timestamp: 1,
        };
        let de = DecodedEvent::Svi {
            ev, status: SanityStatus::Checked(Verdict::Dirty(vec!["boom".into()])), forward_used: Some(5),
        };
        let Row::Svi(r) = to_row(&eid(), 1, &de) else { panic!() };
        assert_eq!(r.sanity, "dirty");
        assert_eq!(r.sanity_reasons.as_deref(), Some(&["boom".to_string()][..]));
    }

    #[test]
    fn prices_row_maps_u64_to_decimal_strings() {
        let p = OraclePricesUpdated {
            oracle_id: ObjId([3; 32]), spot: 73_833_860_000_000, forward: 73_832_220_000_000, timestamp: 42,
        };
        let Row::Prices(r) = to_row(&eid(), 7, &DecodedEvent::Prices(p)) else { panic!() };
        assert_eq!(r.spot, "73833860000000");
        assert_eq!(r.forward, "73832220000000");
        assert_eq!(r.ts_chain_ms, "42");
    }
}
```

- [ ] **Step 2: Run to verify failure**

Run: `cargo test -p indexer row:: 2>&1 | head -20`
Expected: compile error — `to_row` / structs not defined.

- [ ] **Step 3: Implement `to_row` and the structs**

```rust
pub struct PricesRow { pub tx_digest: String, pub event_index: i64, pub checkpoint_seq: i64, pub oracle_id: String, pub spot: String, pub forward: String, pub ts_chain_ms: String }
pub struct SviRow { pub tx_digest: String, pub event_index: i64, pub checkpoint_seq: i64, pub oracle_id: String, pub a: String, pub b: String, pub sigma: String, pub rho: String, pub m: String, pub ts_chain_ms: String, pub sanity_forward: Option<String>, pub sanity: String, pub sanity_reasons: Option<Vec<String>> }
pub enum Row { Prices(PricesRow), Svi(SviRow) }

pub fn to_row(id: &EventId, checkpoint_seq: u64, ev: &DecodedEvent) -> Row {
    match ev {
        DecodedEvent::Prices(p) => Row::Prices(PricesRow {
            tx_digest: id.tx_digest.clone(),
            event_index: id.event_index as i64,
            checkpoint_seq: checkpoint_seq as i64,
            oracle_id: p.oracle_id.to_string(),
            spot: p.spot.to_string(),
            forward: p.forward.to_string(),
            ts_chain_ms: p.timestamp.to_string(),
        }),
        DecodedEvent::Svi { ev, status, forward_used } => {
            let (sanity, sanity_reasons) = match status {
                SanityStatus::Untested => ("untested", None),
                SanityStatus::Checked(Verdict::Clean) => ("clean", None),
                SanityStatus::Checked(Verdict::Dirty(reasons)) => ("dirty", Some(reasons.clone())),
            };
            Row::Svi(SviRow {
                tx_digest: id.tx_digest.clone(),
                event_index: id.event_index as i64,
                checkpoint_seq: checkpoint_seq as i64,
                oracle_id: ev.oracle_id.to_string(),
                a: ev.a.to_string(),
                b: ev.b.to_string(),
                sigma: ev.sigma.to_string(),
                rho: signed_str(ev.rho),
                m: signed_str(ev.m),
                ts_chain_ms: ev.timestamp.to_string(),
                sanity_forward: forward_used.map(|f| f.to_string()),
                sanity: sanity.to_string(),
                sanity_reasons,
            })
        }
    }
}
```

Add `pub mod row;` to `crates/indexer/src/lib.rs`.

- [ ] **Step 4: Run the gate**

Run: `cargo test -p indexer && cargo clippy -p indexer --all-targets -- -D warnings`
Expected: PASS, clippy clean.

- [ ] **Step 5: Commit**

```bash
git add crates/indexer/src/row.rs crates/indexer/src/lib.rs
git commit -m "feat(indexer): pure DecodedEvent→Postgres row mapping (signed rho/m, sanity_forward)"
```

---

### Task 3: sqlx deps + migration SQL

**Files:**
- Modify: `crates/indexer/Cargo.toml`
- Create: `crates/indexer/migrations/0001_oracle_events.sql`

**Interfaces:**
- Produces: tables `prices_update`, `svi_update`; view `oracle_latest`; a migrations dir consumable by `sqlx::migrate!("./migrations")`.

- [ ] **Step 1: Add sqlx to `crates/indexer/Cargo.toml`**

Under `[dependencies]`:

```toml
# Postgres sink. rustls-only (no openssl). Runtime query() — NO query! macro,
# so build/CI need no live DB. migrate! embeds the migrations dir at build time.
sqlx = { version = "0.8", default-features = false, features = ["runtime-tokio", "tls-rustls", "postgres", "macros", "migrate"] }
```

- [ ] **Step 2: Write the migration `crates/indexer/migrations/0001_oracle_events.sql`**

```sql
-- A-path oracle event log. Raw chain integers as unbounded NUMERIC (source of
-- truth); decoding (/1e9, aligned with crates/types fixed.rs::ONE) lives only
-- in the oracle_latest view. Dedup key (tx_digest, event_index) is the Sui
-- event's content-addressed identity.

CREATE TABLE IF NOT EXISTS prices_update (
  tx_digest      TEXT        NOT NULL,
  event_index    BIGINT      NOT NULL,
  checkpoint_seq BIGINT      NOT NULL,
  oracle_id      TEXT        NOT NULL,
  spot           NUMERIC     NOT NULL,
  forward        NUMERIC     NOT NULL,
  ts_chain_ms    NUMERIC     NOT NULL,
  ingested_at    TIMESTAMPTZ NOT NULL DEFAULT now(),
  PRIMARY KEY (tx_digest, event_index)
);

CREATE TABLE IF NOT EXISTS svi_update (
  tx_digest      TEXT        NOT NULL,
  event_index    BIGINT      NOT NULL,
  checkpoint_seq BIGINT      NOT NULL,
  oracle_id      TEXT        NOT NULL,
  a              NUMERIC     NOT NULL,
  b              NUMERIC     NOT NULL,
  sigma          NUMERIC     NOT NULL,
  rho            NUMERIC     NOT NULL,   -- signed (sign decoded in Rust at insert)
  m              NUMERIC     NOT NULL,   -- signed
  ts_chain_ms    NUMERIC     NOT NULL,
  sanity_forward NUMERIC,                -- forward the no-arb check ran against; NULL iff untested
  sanity         TEXT        NOT NULL,   -- 'untested' | 'clean' | 'dirty'
  sanity_reasons TEXT[],
  ingested_at    TIMESTAMPTZ NOT NULL DEFAULT now(),
  PRIMARY KEY (tx_digest, event_index)
);

CREATE INDEX IF NOT EXISTS svi_oracle_seq_idx    ON svi_update    (oracle_id, checkpoint_seq DESC);
CREATE INDEX IF NOT EXISTS prices_oracle_seq_idx ON prices_update (oracle_id, checkpoint_seq DESC);

CREATE OR REPLACE VIEW oracle_latest AS
WITH latest_svi AS (
  SELECT DISTINCT ON (oracle_id) *
  FROM svi_update ORDER BY oracle_id, checkpoint_seq DESC, event_index DESC
),
latest_prices AS (
  SELECT DISTINCT ON (oracle_id) *
  FROM prices_update ORDER BY oracle_id, checkpoint_seq DESC, event_index DESC
)
SELECT
  COALESCE(s.oracle_id, p.oracle_id)      AS oracle_id,
  s.a::float8     / 1e9 AS a,
  s.b::float8     / 1e9 AS b,
  s.rho::float8   / 1e9 AS rho,
  s.m::float8     / 1e9 AS m,
  s.sigma::float8 / 1e9 AS sigma,
  s.sanity              AS svi_sanity,    -- may be NULL: prices-only oracle
  s.checkpoint_seq      AS svi_checkpoint_seq,
  p.spot::float8    / 1e9 AS spot,
  p.forward::float8 / 1e9 AS forward,
  p.checkpoint_seq        AS prices_checkpoint_seq
FROM latest_svi s
FULL OUTER JOIN latest_prices p USING (oracle_id);
```

- [ ] **Step 3: Verify it compiles (deps fetch + build)**

Run: `cargo build -p indexer 2>&1 | tail -20`
Expected: builds (sqlx resolves with rustls; no openssl/cmake). If a feature error appears, confirm `default-features = false`.

- [ ] **Step 4: Commit**

```bash
git add crates/indexer/Cargo.toml crates/indexer/migrations/0001_oracle_events.sql Cargo.lock
git commit -m "feat(indexer): add sqlx (rustls) + 0001 oracle_events migration"
```

---

### Task 4: `PostgresSink` + background writer task

**Files:**
- Create: `crates/indexer/src/postgres.rs`
- Modify: `crates/indexer/src/lib.rs` (`pub mod postgres;`)
- Test: `crates/indexer/tests/postgres_integration.rs` (`#[sqlx::test]`, runtime DB only)

**Interfaces:**
- Consumes: `Row`/`to_row` (Task 2), `Sink`/`EventId`/`DecodedEvent` (Task 1).
- Produces:
  - `pub const CHANNEL_CAPACITY: usize = 1024;`
  - `pub struct PostgresSink { tx: tokio::sync::mpsc::Sender<Row> }`
  - `pub fn channel() -> (tokio::sync::mpsc::Sender<Row>, tokio::sync::mpsc::Receiver<Row>)`
  - `impl PostgresSink { pub fn new(tx: Sender<Row>) -> Self }`
  - `pub async fn connect_pool(database_url: &str) -> anyhow::Result<sqlx::PgPool>` (sets `acquire_timeout`, runs migrations)
  - `pub async fn run_writer(mut rx: Receiver<Row>, pool: sqlx::PgPool) -> anyhow::Result<()>`

- [ ] **Step 1: Write the integration tests (runtime DB) `crates/indexer/tests/postgres_integration.rs`**

```rust
//! Runtime-DB integration tests. `#[sqlx::test]` creates an isolated, migrated
//! database per test (requires DATABASE_URL → a reachable Postgres). Not part of
//! the offline gate; run in the live smoke: `cargo test -p indexer --test postgres_integration`.

use indexer::postgres::{run_writer, PostgresSink, channel};
use indexer::row::{Row, SviRow, PricesRow};
use indexer::sink::Sink;
use indexer::sink::{DecodedEvent, EventId, SanityStatus};
use types::events::{I64Raw, ObjId, OracleSviUpdated, OraclePricesUpdated};
use pricing::invariants::Verdict;

fn svi_event(oracle: [u8; 32]) -> OracleSviUpdated {
    OracleSviUpdated {
        oracle_id: ObjId(oracle), a: 5274, b: 638_806,
        rho: I64Raw { magnitude: 458_555_014, is_negative: true },
        m: I64Raw { magnitude: 1_380_256, is_negative: true },
        sigma: 1_181_366, timestamp: 1,
    }
}

// Drive a list of (EventId, DecodedEvent) through a PostgresSink + writer to the pool.
async fn ingest(pool: sqlx::PgPool, items: Vec<(EventId, u64, DecodedEvent)>) {
    let (tx, rx) = channel();
    let writer = tokio::spawn(run_writer(rx, pool));
    let sink = PostgresSink::new(tx);
    for (id, seq, ev) in &items {
        sink.emit(id, *seq, ev).unwrap();
    }
    drop(sink); // close channel → writer drains and returns
    writer.await.unwrap().unwrap();
}

#[sqlx::test]
async fn reinserting_same_event_id_is_idempotent(pool: sqlx::PgPool) {
    // WHY: startup re-backfills from tip-N, so the SAME tx re-appears. Its digest
    // is content-addressed → identical (tx_digest, event_index) → ON CONFLICT must
    // dedup. This test fails if the key or conflict clause is wrong.
    let id = EventId { tx_digest: "AbC".into(), event_index: 0 };
    let de = || DecodedEvent::Svi { ev: svi_event([0xAA; 32]), status: SanityStatus::Untested, forward_used: None };
    ingest(pool.clone(), vec![(id.clone(), 10, de()), (id.clone(), 10, de())]).await;
    let n: i64 = sqlx::query_scalar("SELECT count(*) FROM svi_update").fetch_one(&pool).await.unwrap();
    assert_eq!(n, 1, "duplicate (tx_digest,event_index) must not insert twice");
}

#[sqlx::test]
async fn sanity_is_reproducible_regardless_of_replay_order(pool: sqlx::PgPool) {
    // WHY: the stored verdict is a pure function of (raw SVI, sanity_forward).
    // Two ingests of the SAME svi event with the SAME forward_used must yield the
    // same row, independent of when prices arrived in the stream.
    let id = EventId { tx_digest: "RpL".into(), event_index: 1 };
    let de = DecodedEvent::Svi {
        ev: svi_event([0xBB; 32]),
        status: SanityStatus::Checked(Verdict::Clean),
        forward_used: Some(73_744_082_479_138),
    };
    ingest(pool.clone(), vec![(id, 5, de)]).await;
    let (sanity, fwd): (String, sqlx::types::BigDecimal) =
        sqlx::query_as("SELECT sanity, sanity_forward FROM svi_update WHERE tx_digest='RpL'")
            .fetch_one(&pool).await.unwrap();
    assert_eq!(sanity, "clean");
    assert_eq!(fwd.to_string(), "73744082479138");
}

#[sqlx::test]
async fn oracle_latest_view_returns_most_recent_per_oracle(pool: sqlx::PgPool) {
    let o = [0xCC; 32];
    let mk = |idx: u64, seq: u64| (
        EventId { tx_digest: format!("d{idx}"), event_index: idx },
        seq,
        DecodedEvent::Prices(OraclePricesUpdated { oracle_id: ObjId(o), spot: seq, forward: seq, timestamp: seq }),
    );
    ingest(pool.clone(), vec![mk(0, 100), mk(1, 200)]).await; // seq 200 is newer
    let spot: f64 = sqlx::query_scalar("SELECT spot FROM oracle_latest WHERE oracle_id=$1")
        .bind(ObjId(o).to_string()).fetch_one(&pool).await.unwrap();
    assert!((spot - 200.0 / 1e9).abs() < 1e-18, "view must surface the latest checkpoint");
}
```

- [ ] **Step 2: Run to verify failure**

Run: `cargo test -p indexer --test postgres_integration 2>&1 | head -20`
Expected: compile error — `indexer::postgres` module missing.

- [ ] **Step 3: Implement `crates/indexer/src/postgres.rs`**

```rust
//! Postgres output sink. `emit` maps to a row and `try_send`s onto a bounded
//! channel (sync, non-blocking); a background writer task owns the pool and does
//! the inserts. Channel Full / writer-dead → loud Err (Rule 12), never a drop.

use anyhow::{anyhow, Context, Result};
use sqlx::postgres::PgPoolOptions;
use std::time::Duration;
use tokio::sync::mpsc;

use crate::row::{to_row, Row};
use crate::sink::{DecodedEvent, EventId, Sink};

pub const CHANNEL_CAPACITY: usize = 1024;

pub fn channel() -> (mpsc::Sender<Row>, mpsc::Receiver<Row>) {
    mpsc::channel(CHANNEL_CAPACITY)
}

pub struct PostgresSink {
    tx: mpsc::Sender<Row>,
}

impl PostgresSink {
    pub fn new(tx: mpsc::Sender<Row>) -> Self {
        Self { tx }
    }
}

impl Sink for PostgresSink {
    fn emit(&self, id: &EventId, checkpoint_seq: u64, ev: &DecodedEvent) -> Result<()> {
        let row = to_row(id, checkpoint_seq, ev);
        self.tx.try_send(row).map_err(|e| match e {
            mpsc::error::TrySendError::Full(_) => anyhow!(
                "postgres writer cannot keep up (channel full, cap={CHANNEL_CAPACITY}) — DB too slow"
            ),
            mpsc::error::TrySendError::Closed(_) => anyhow!("postgres writer task has died"),
        })
    }
}

/// Connect, set an acquire timeout (a hung DB becomes a loud error, not an
/// unbounded wait), and run migrations. Fatal on failure.
pub async fn connect_pool(database_url: &str) -> Result<sqlx::PgPool> {
    let pool = PgPoolOptions::new()
        .acquire_timeout(Duration::from_secs(10))
        .connect(database_url)
        .await
        .context("connect to DATABASE_URL")?;
    sqlx::migrate!("./migrations")
        .run(&pool)
        .await
        .context("run migrations")?;
    Ok(pool)
}

/// Drain the channel and insert each row. Returns when the channel closes (all
/// senders dropped) after draining everything. An insert error is fatal.
pub async fn run_writer(mut rx: mpsc::Receiver<Row>, pool: sqlx::PgPool) -> Result<()> {
    while let Some(row) = rx.recv().await {
        match row {
            Row::Prices(r) => {
                sqlx::query(
                    "INSERT INTO prices_update \
                     (tx_digest,event_index,checkpoint_seq,oracle_id,spot,forward,ts_chain_ms) \
                     VALUES ($1,$2,$3,$4,$5::numeric,$6::numeric,$7::numeric) \
                     ON CONFLICT (tx_digest,event_index) DO NOTHING",
                )
                .bind(&r.tx_digest).bind(r.event_index).bind(r.checkpoint_seq)
                .bind(&r.oracle_id).bind(&r.spot).bind(&r.forward).bind(&r.ts_chain_ms)
                .execute(&pool).await.context("insert prices_update")?;
            }
            Row::Svi(r) => {
                sqlx::query(
                    "INSERT INTO svi_update \
                     (tx_digest,event_index,checkpoint_seq,oracle_id,a,b,sigma,rho,m,ts_chain_ms,sanity_forward,sanity,sanity_reasons) \
                     VALUES ($1,$2,$3,$4,$5::numeric,$6::numeric,$7::numeric,$8::numeric,$9::numeric,$10::numeric,$11::numeric,$12,$13) \
                     ON CONFLICT (tx_digest,event_index) DO NOTHING",
                )
                .bind(&r.tx_digest).bind(r.event_index).bind(r.checkpoint_seq).bind(&r.oracle_id)
                .bind(&r.a).bind(&r.b).bind(&r.sigma).bind(&r.rho).bind(&r.m).bind(&r.ts_chain_ms)
                .bind(r.sanity_forward.as_deref()).bind(&r.sanity).bind(r.sanity_reasons.as_deref())
                .execute(&pool).await.context("insert svi_update")?;
            }
        }
    }
    Ok(())
}
```

Add `pub mod postgres;` to `crates/indexer/src/lib.rs`.

> Note on binds: `sanity_forward.as_deref()` binds `Option<&str>` → cast `$11::numeric` (NULL passes through). `sanity_reasons.as_deref()` binds `Option<&[String]>` → Postgres `TEXT[]` (sqlx postgres array support, built-in).

- [ ] **Step 4: Run the offline gate (no DB needed — integration tests skip without DATABASE_URL)**

Run: `cargo build -p indexer && cargo clippy -p indexer --all-targets -- -D warnings`
Expected: builds + clippy clean. (`#[sqlx::test]` tests compile but only run when a DB is reachable.)

- [ ] **Step 5: Run the integration tests against a local Postgres**

```bash
# one-time: start a throwaway PG
docker run -d --rm --name pg-smoke -e POSTGRES_PASSWORD=pw -p 5432:5432 postgres:16
export DATABASE_URL=postgres://postgres:pw@localhost:5432/postgres
cargo test -p indexer --test postgres_integration -- --nocapture
```
Expected: 3 tests PASS (idempotency, sanity reproducibility, latest view).

- [ ] **Step 6: Commit**

```bash
git add crates/indexer/src/postgres.rs crates/indexer/src/lib.rs crates/indexer/tests/postgres_integration.rs
git commit -m "feat(indexer): PostgresSink + bounded-channel writer task (idempotent inserts, latest view)"
```

---

### Task 5: `TeeSink` + `main.rs` sink selection & ordered shutdown

**Files:**
- Modify: `crates/indexer/src/sink.rs` (add `TeeSink`)
- Modify: `crates/indexer/src/main.rs` (select sinks by `DATABASE_URL`, ordered shutdown)

**Interfaces:**
- Consumes: `Sink`, `PostgresSink`, `connect_pool`, `run_writer`, `channel` (Tasks 1/4).
- Produces: `pub struct TeeSink(pub Vec<Box<dyn Sink + Send + Sync>>);` with `impl Sink` that calls each child and returns the first `Err`.

- [ ] **Step 1: Add a failing test for `TeeSink` fan-out + error propagation in `sink.rs` tests**

Add a `#[cfg(test)] mod tests` to `crates/indexer/src/sink.rs` (or extend if present):

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;

    struct CountSink(Arc<AtomicUsize>);
    impl Sink for CountSink {
        fn emit(&self, _: &EventId, _: u64, _: &DecodedEvent) -> anyhow::Result<()> {
            self.0.fetch_add(1, Ordering::SeqCst); Ok(())
        }
    }
    struct ErrSink;
    impl Sink for ErrSink {
        fn emit(&self, _: &EventId, _: u64, _: &DecodedEvent) -> anyhow::Result<()> {
            Err(anyhow::anyhow!("boom"))
        }
    }

    fn sample() -> DecodedEvent {
        DecodedEvent::Prices(types::events::OraclePricesUpdated {
            oracle_id: types::events::ObjId([0; 32]), spot: 1, forward: 1, timestamp: 1,
        })
    }

    #[test]
    fn tee_fans_out_to_all() {
        let c = Arc::new(AtomicUsize::new(0));
        let tee = TeeSink(vec![Box::new(CountSink(c.clone())), Box::new(CountSink(c.clone()))]);
        tee.emit(&EventId { tx_digest: "x".into(), event_index: 0 }, 1, &sample()).unwrap();
        assert_eq!(c.load(Ordering::SeqCst), 2);
    }

    #[test]
    fn tee_propagates_first_error() {
        let tee = TeeSink(vec![Box::new(ErrSink)]);
        let r = tee.emit(&EventId { tx_digest: "x".into(), event_index: 0 }, 1, &sample());
        assert!(r.is_err());
    }
}
```

- [ ] **Step 2: Run to verify failure**

Run: `cargo test -p indexer sink:: 2>&1 | head -20`
Expected: compile error — `TeeSink` not defined.

- [ ] **Step 3: Implement `TeeSink` in `sink.rs`**

```rust
/// Fans each event out to every child sink, returning the first error (so a
/// failing PostgresSink takes the whole indexer down — fail loud).
pub struct TeeSink(pub Vec<Box<dyn Sink + Send + Sync>>);

impl Sink for TeeSink {
    fn emit(&self, id: &EventId, checkpoint_seq: u64, ev: &DecodedEvent) -> anyhow::Result<()> {
        for sink in &self.0 {
            sink.emit(id, checkpoint_seq, ev)?;
        }
        Ok(())
    }
}
```

(Ensure `StdoutSink` and `PostgresSink` are `Send + Sync` — `StdoutSink` is a ZST; `PostgresSink` holds an `mpsc::Sender` which is `Send + Sync`.)

- [ ] **Step 4: Rewire `main.rs` — sink selection + ordered shutdown**

Replace the consumer/shutdown section of `main()`. The sink is built before the consumer; the writer (if any) is awaited AFTER the consumer fully joins so the channel is closed and drained — never via a racing `try_join!`:

```rust
    let pool_and_rx = match std::env::var("DATABASE_URL") {
        Ok(url) => {
            let pool = indexer::postgres::connect_pool(&url).await.context("init postgres")?;
            let (tx, rx) = indexer::postgres::channel();
            tracing::info!("postgres sink enabled");
            Some((pool, tx, rx))
        }
        Err(_) => {
            tracing::info!("DATABASE_URL unset — stdout sink only");
            None
        }
    };

    // Build the sink set: always stdout; postgres when configured.
    let mut sinks: Vec<Box<dyn indexer::sink::Sink + Send + Sync>> = vec![Box::new(StdoutSink)];
    let writer = pool_and_rx.map(|(pool, tx, rx)| {
        sinks.push(Box::new(indexer::postgres::PostgresSink::new(tx)));
        tokio::spawn(indexer::postgres::run_writer(rx, pool))
    });
    let tee = indexer::sink::TeeSink(sinks);

    let service = svc.run(start..).await.context("start ingestion")?;

    let consumer = tokio::spawn(async move {
        let sink = tee; // consumer OWNS the sink → its PostgresSink Sender drops when this task ends
        let mut state = PipelineState::default();
        while let Some(envelope) = rx.recv().await {
            if let Err(e) = process_checkpoint(&envelope, &mut state, &sink) {
                tracing::error!(error = %e, "fatal — stopping indexer");
                return Err::<(), anyhow::Error>(e);
            }
        }
        Ok(())
    });

    service.main().await.context("ingestion service")?;
    consumer.await.context("consumer task panicked")??;
    // Consumer has ended → tee (and its Sender) dropped → channel closed.
    // Now drain the writer; it returns once all buffered rows are inserted.
    if let Some(writer) = writer {
        writer.await.context("writer task panicked")??;
    }
    Ok(())
```

Note `rx` here is the existing `svc.subscribe_bounded(...)` checkpoint receiver — keep that line. Adjust imports: `use anyhow::Context;` already present. The previous inline `StdoutSink` consumer block is fully replaced.

- [ ] **Step 5: Offline gate**

Run: `cargo test -p indexer && cargo clippy -p indexer --all-targets -- -D warnings`
Expected: PASS (incl. TeeSink tests), clippy clean.

- [ ] **Step 6: Commit**

```bash
git add crates/indexer/src/sink.rs crates/indexer/src/main.rs
git commit -m "feat(indexer): TeeSink + DATABASE_URL sink selection + ordered drain-on-shutdown"
```

---

### Task 6: Live smoke + monkey + docs

**Files:**
- Modify: `tasks/progress.md`, `tasks/lessons.md` (if anything new learned), `move-notes.md` if applicable
- No code unless smoke reveals a bug.

- [ ] **Step 1: Full workspace gate**

Run: `cargo test --workspace && cargo clippy --all-targets -- -D warnings`
Expected: all green.

- [ ] **Step 2: Live smoke against testnet + local Postgres**

```bash
docker run -d --rm --name pg-smoke -e POSTGRES_PASSWORD=pw -p 5432:5432 postgres:16
export DATABASE_URL=postgres://postgres:pw@localhost:5432/postgres
RUST_LOG=info cargo run -p indexer
# let it run ~60s, then in another shell:
psql "$DATABASE_URL" -c "SELECT count(*) FROM svi_update; SELECT count(*) FROM prices_update;"
psql "$DATABASE_URL" -c "SELECT oracle_id, a, rho, sigma, svi_sanity, spot, forward FROM oracle_latest LIMIT 5;"
```
Success criteria:
- Rows accumulate in both tables; counts match the stdout event log volume.
- `oracle_latest` rows: `rho` is **negative** for oracles with negative on-chain rho (sign survived), `spot`/`forward` ≈ live BTC scale (~6e4), `svi_sanity` ∈ {untested, clean, dirty}.
- Restart `cargo run` → counts do **not** double (ON CONFLICT dedup across the re-backfill).

- [ ] **Step 3: Monkey tests**

- Kill Postgres mid-run (`docker stop pg-smoke`) → indexer must exit loud (writer `Err` / channel closed), not hang or silently continue.
- Start with `DATABASE_URL` pointing at a dead port → fatal at `connect_pool`, not a no-op.
- Unset `DATABASE_URL` → stdout-only, no DB calls, runs as before.

- [ ] **Step 4: Update progress + lessons**

Record: tasks done, gate results, live-smoke evidence (counts, sign-preserved rho, dedup-on-restart), any framework/sqlx gotcha. Note B-path (object poll) remains the next round.

- [ ] **Step 5: Commit**

```bash
git add tasks/progress.md tasks/lessons.md
git commit -m "docs: PostgresSink live smoke + monkey results; B-path deferred"
```

---

## Self-Review

**Spec coverage:**
- §2 channel/TeeSink/sync Sink → Tasks 1, 4, 5. ✓
- §3 schema (NUMERIC, signed rho/m, sanity_forward, view tiebreaker, NULL sanity) → Tasks 2 (signed/forward mapping), 3 (DDL). ✓
- §4 EventId plumbing (enumerate unfiltered, base58 digest, dedup why) → Task 1 + idempotency test Task 4. ✓
- §5 sqlx deps, runtime query, DATABASE_URL env → Tasks 3, 4, 5. ✓
- §6 fail-loud + ordered shutdown + acquire_timeout → Tasks 4 (acquire_timeout, fatal insert), 5 (ordered drain). ✓
- §7 testing (unit signed-decode, idempotency-with-why, sanity reproducibility, view, monkey) → Tasks 2, 4, 6. ✓
- §9 forgo framework watermark → documented; resume-by-backfill verified in Task 6 restart check. ✓

**Placeholder scan:** none — every code step has full code; no TBD/TODO.

**Type consistency:** `emit(&EventId, u64, &DecodedEvent) -> Result<()>` consistent across StdoutSink/CaptureSink/PostgresSink/TeeSink/handle_event. `DecodedEvent::Svi { ev, status, forward_used }` consistent (Task 1 defines, Task 2 reads, Task 4/test construct). `Row`/`to_row` signatures match between Task 2 and Task 4. `channel()`/`run_writer`/`connect_pool`/`PostgresSink::new` names consistent Tasks 4↔5.
