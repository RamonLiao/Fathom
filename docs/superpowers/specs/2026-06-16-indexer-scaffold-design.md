# Indexer Scaffold — Design Spec

_Date: 2026-06-16 · Status: approved (brainstorm), pending spec review_

## Scope (this round)

Scaffold `indexer/` to the point where the **A path (event-driven)** runs end-to-end against
testnet: subscribe → decode `oracle::*` events → sanity-check via `pricing` → emit to stdout.

**Deferred to next rounds (explicitly out of scope here):**
- B path: per-checkpoint `Predict` object polling + diff.
- Postgres persistence (`PostgresSink`, schema migrations, idempotent insert).

Rationale: the A-path decode (`i64` sign-magnitude + dual 1e9/6-dec scale) is the highest-risk
"will silently corrupt NAV" surface; prove it against real chain data first. B path and Postgres
are independently scoped follow-ups (one module task per chat, per project conventions).

## Decisions (brainstorm log)

1. **Scope = "skeleton + runnable gRPC subscription (A path), stdout, no DB"** (option 2).
2. **Ingestion = `sui-indexer-alt-framework` (`Processor` trait), official SDK** (option 1).
   - Checkpoint is Sui's native consistency boundary → A (events) and B (object state) share one
     checkpoint stream. Next round's B path = add a second `Processor` (object tracker) to the same
     `Service`; processors run as independent tasks, one failing does not affect others.
   - JSON-RPC (deprecated ~2026-04) unaffected — this is checkpoint-based ingestion.
   - **API verified via sui-indexer skill (v1.72 / Protocol 124, 2026-06-16):** trait is
     `Processor { const NAME; async fn process(&self, &CheckpointEnvelope) }`; wired via
     `Service::builder().ingestion_client(StoreIngestionClient::new_remote(url)).add_processor(..).build().main()`.
     `FANOUT` / `checkpoint_lag` removed (adaptive concurrency). v1.72 `rpc-index` DB v4 → first
     start re-indexes full object history (matters when B path / Postgres lands, not this round).
3. **Type ownership = new shared `types` crate** (option 3 structure) **with full migration of
   pricing's SVI/fixed/scale types into it** (option 2 depth). Single source of truth for scale +
   sign-magnitude — the structural root cause of the "1000× NAV" bug class.
   - **Boundary in full now, content on demand.** `types` holds only what is used this round
     (existing SVI/fixed/scale + the 2 events decoded here). Do NOT pre-add `OracleSettled`,
     `PositionMinted`, or Postgres row types — add when their consumer is real (YAGNI / Rule 2).

## Architecture

Workspace grows to 3 members:

```
crates/
  types/      ← new: scale consts, i64 sign-magnitude, SVI param types, this round's 2 event structs
  pricing/    ← changed: use types (SVI/fixed/scale types moved out; numeric logic untouched)
  indexer/    ← new: Data Ingestion Framework Worker + oracle event decode
```

Dependency graph (acyclic): `indexer → types`, `pricing → types`, `indexer → pricing`
(indexer runs pricing invariants as a decode sanity gate).

## Module Structure

### `crates/types/`
```
src/
  lib.rs        pub mod scale; svi; fixed; events;
  scale.rs      const FIXED_ONE: u64 = 1_000_000_000;  DUSDC_DECIMALS = 6
                newtype Fixed1e9(u64), Dusdc(u64) — compile-time anti-mix
  fixed.rs      moved from pricing/src/fixed.rs (1e9 fixed-point helpers)
  svi.rs        moved from pricing: SviParams { a:u64, b:u64, rho:I64, m:I64, sigma:u64 }
                I64 { magnitude:u64, is_negative:bool } sign-magnitude + from_parts/to_f64
  events.rs     new: OracleSviUpdated, OraclePricesUpdated (chain raw → strong type)
```

**Newtype adoption is gradual.** `Fixed1e9`/`Dusdc` are used only at the indexer output boundary
this round; pricing internals keep bare `u64` to keep the migration diff minimal and not break
green (Rule 3).

### `crates/pricing/` (minimal change)
- `src/svi.rs`, `src/fixed.rs` bodies removed → replaced with `pub use types::{svi, fixed};`
  re-export, **preserving the `pricing::svi::*` path** so existing tests/fixtures import unchanged.
- `digital.rs`, `fixture.rs` untouched.
- Acceptance: `cargo test --workspace` still all-green = no regression.

### `crates/indexer/`
```
src/
  main.rs       tokio entry: Service::builder().ingestion_client(StoreIngestionClient::new_remote(url))
                  .add_processor(OracleProcessor).build().main()
  config.rs     package id, Predict object id, fullnode/checkpoint URL (consts from architecture doc)
  processor.rs  impl Processor for OracleProcessor: const NAME; process(&CheckpointEnvelope):
                  scan envelope.data.transactions[].events, filter type_.module=="oracle" + package==ours
  decode.rs     event.parsed_json → types::events::* (i64 sign-magnitude + 1e9). Core risk, centralized.
  sanity.rs     post-decode: call pricing's invariant API on the SVI (NOT reimplemented here).
                  pricing exposes `pub fn check_invariants(&SviParams) -> Verdict` (parity / monotone /
                  compute_price==up), reusing its golden_vectors logic — single source of truth.
                  fail → WARN log, tag dirty (no panic) — mirrors quarantine lessons.
  sink.rs       trait EventSink { fn emit(&self, ev) }; this round only StdoutSink
                  (next round: PostgresSink behind same trait).
```

Two seams left for next round:
- `sink.rs` trait → next round's `PostgresSink` (single method, concrete known consumer).
- B path = a **second `Processor`** (`PredictObjectProcessor`) added to the same `Service`, not a
  change to `OracleProcessor`. Independent task, isolated failure.

**To pin in writing-plans (via `sui-docs-query`):** exact `sui-indexer-alt-framework` git rev /
version, `parsed_json` field shape for `OracleSVIUpdated` (confirm i64 nested struct keys), and
testnet fullnode vs checkpoint-archive URL for `StoreIngestionClient`.

## Data Flow (A path)

```
StoreIngestionClient → Service feeds CheckpointEnvelope to OracleProcessor
  → processor.rs: scan envelope.data.transactions[].events, filter type_.module=="oracle" + package==PKG
  → match OracleSVIUpdated / OraclePricesUpdated
  → decode.rs: event.parsed_json → types::events::* (i64 sign-magnitude + 1e9)
  → sanity.rs: pricing invariants (monotone/parity) → clean | dirty(WARN tag)
  → sink.rs: StdoutSink.emit (checkpoint_seq + decoded struct + sanity verdict)
```
Checkpoint cursor / concurrency managed by framework (adaptive). Next round's B path = second
`Processor` on the same `Service` polling the `Predict` object per checkpoint.

## Error Handling (Rule 12 — fail loud, with the right split)

| Class | Handling |
|---|---|
| **transport** (stream drop, checkpoint fetch fail) | framework retry + backoff; sustained failure → ERROR log, never swallow |
| **decode failure** (missing field, type mismatch) | **panic / return Err** — schema-drift hard signal, must be loud. Never silently skip (would fake healthy while dropping data) |
| **sanity failure** (decode ok but invariant broken) | **no panic** — tag `dirty` + WARN + still emit. Separates "our decode bug" from "upstream data dirty" |
| **config drift** (package redeployed → filter matches 0) | **liveness check**: if 0 oracle events seen in first N checkpoints → WARN (not fatal — testnet may genuinely be quiet, but silence must be loud, not assumed healthy) |

Critical boundary: **decode error (our bug) = loud fail; data dirty (upstream phenomenon) = tag,
no fail.** Conflating these was the near-miss that almost mislabeled the pricing formula as wrong.

## Testing

1. **types decode unit tests:** fixed raw event → expected `SviParams`, **including negative rho**
   (sign-magnitude's most error-prone path; golden = architecture doc's live `rho = -0.94` sample).
2. **sanity integration:** known clean oracle from pricing fixtures → invariants pass; known dirty
   (quarantined `0x2709d76a`) → tagged dirty, no panic.
3. **scale anti-mix:** `Fixed1e9` and `Dusdc` not mutually assignable (compile-time; trybuild or
   commented compile-fail example).
4. **workspace regression:** `cargo test --workspace` all-green (proves pricing migration clean).
5. **Monkey (test.md):** malformed events (magnitude overflow, is_negative with magnitude=0,
   missing field) → decode loud-fails, never silently produces a bad value.

**Live smoke:** the "actually connect to testnet and print" part needs a real connection. Tests
mock checkpoints (no network dependency); real connect is a manual acceptance step
(`cargo run` prints a few checkpoints), not in CI (network flaky).

## Acceptance Criteria

- [ ] `cargo build --workspace` clean; 3 members.
- [ ] `cargo test --workspace` all-green (pricing no regression + new types/indexer tests).
- [ ] `cargo clippy --workspace` clean.
- [ ] `cargo run -p indexer` against testnet prints decoded `OracleSVIUpdated`/`OraclePricesUpdated`
      with correct sign-magnitude + 1e9 scale and per-event sanity verdict.
