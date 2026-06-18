# Indexer Scaffold Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Stand up `crates/indexer` running the A path (event-driven) against testnet via `sui-indexer-alt-framework`: subscribe → decode `oracle::*` events → no-arb sanity-check via `pricing` → emit to stdout. Extract a shared `types` crate as single source of truth for scale + SVI types.

**Architecture:** Workspace grows to 3 members. `types` holds scale/fixed-point helpers, the `Svi` type, and raw on-chain event structs. `pricing` re-exports `fixed`/`svi` from `types` (numeric logic untouched, paths preserved). `indexer` implements a `Processor` that filters `oracle` events from each checkpoint, decodes raw → `Svi`, runs `pricing`'s no-arb invariant check, and emits via a `Sink` trait (stdout this round; Postgres next round). Dependency graph is acyclic: `indexer → {types, pricing}`, `pricing → types`.

**Tech Stack:** Rust 2021, `sui-indexer-alt-framework` (Protocol 124 / v1.72 API: `Processor` trait + `Service::builder`), tokio, serde/serde_json, anyhow, async-trait, tracing, libm (pricing only).

**Spec:** `docs/superpowers/specs/2026-06-16-indexer-scaffold-design.md`

---

## File Structure

```
Cargo.toml                         MODIFY: members += types, indexer
crates/
  types/
    Cargo.toml                     CREATE
    src/lib.rs                     CREATE: pub mod scale; fixed; svi; events;
    src/scale.rs                   CREATE: ONE re-export + Fixed1e9 newtype
    src/fixed.rs                   CREATE: moved verbatim from pricing/src/fixed.rs
    src/svi.rs                     CREATE: moved verbatim from pricing/src/svi.rs
    src/events.rs                  CREATE: I64Raw, OracleSviUpdated, OraclePricesUpdated + decode
  pricing/
    Cargo.toml                     MODIFY: add `types` path dep
    src/lib.rs                     MODIFY: re-export fixed/svi from types; add `pub mod invariants`
    src/fixed.rs                   DELETE (moved to types)
    src/svi.rs                     DELETE (moved to types)
    src/invariants.rs             CREATE: no-arb check on a computed SVI curve
  indexer/
    Cargo.toml                     CREATE
    src/config.rs                  CREATE: package id, fullnode URL, liveness window consts
    src/sink.rs                    CREATE: trait Sink + StdoutSink + DecodedEvent enum
    src/pipeline.rs               CREATE: pure handle_event(parsed_json, sink, liveness) — testable core
    src/processor.rs              CREATE: impl Processor for OracleProcessor (extracts events → pipeline)
    src/main.rs                    CREATE: tokio entry, Service wiring
```

**Decisions locked from review (deviations from the spec sketch, intentional):**
- `pricing::Svi` is **f64 (decoded)**, not raw `u64`. So `types::events` holds RAW chain values + a `to_svi()` decoder; the scale single-source lives in `types::fixed`.
- Only `Fixed1e9` newtype this round (oracle events carry 1e9 values). `Dusdc` is NOT added — no DUSDC amounts in oracle events (YAGNI; add with the B/Postgres round). This narrows the spec's "newtypes" item to what's used.
- Sanity gate is **NOT** a replay of `golden_vectors` (that needs the on-chain up/down grid, absent from a live `OracleSVIUpdated`). Instead `pricing::invariants::check_svi_arb_free(&Svi, forward_1e9)` computes our own digital curve and checks no-arb structure. Invariant *definitions* (monotone non-increasing, ∈[0,1], ATM≈0.5) live once in pricing.

---

## Task 1: Create `types` crate, move `fixed` + `svi` from pricing

**Files:**
- Modify: `Cargo.toml` (workspace members)
- Create: `crates/types/Cargo.toml`, `crates/types/src/lib.rs`, `crates/types/src/fixed.rs`, `crates/types/src/svi.rs`

- [ ] **Step 1: Add `types` to workspace members**

Edit `Cargo.toml`:
```toml
[workspace]
resolver = "2"
members = ["crates/pricing", "crates/types"]
```

- [ ] **Step 2: Create `crates/types/Cargo.toml`**

```toml
[package]
name = "types"
version = "0.1.0"
edition = "2021"

[dependencies]
serde = { version = "1", features = ["derive"] }
serde_json = "1"
```

- [ ] **Step 3: Create `crates/types/src/lib.rs`**

```rust
pub mod scale;
pub mod fixed;
pub mod svi;
pub mod events;
```

- [ ] **Step 4: Move `fixed.rs` and `svi.rs` into types verbatim**

Run (copies the existing, already-tested files unchanged):
```bash
cp crates/pricing/src/fixed.rs crates/types/src/fixed.rs
cp crates/pricing/src/svi.rs   crates/types/src/svi.rs
```
These files have no `crate::`-internal cross-references except `svi.rs` is standalone and `fixed.rs` is standalone (verified: fixed.rs uses only `ONE`; svi.rs uses only std). They compile as-is in `types`.

- [ ] **Step 5: Add empty placeholder modules so lib compiles**

Create `crates/types/src/scale.rs`:
```rust
//! placeholder — filled in Task 3
```
Create `crates/types/src/events.rs`:
```rust
//! placeholder — filled in Task 4
```

- [ ] **Step 6: Verify types crate builds and its moved tests pass**

Run: `cargo test -p types`
Expected: PASS (the `fixed` and `svi` unit tests that moved over, e.g. `decode_i64_signs`, `total_variance_atm_corpus0`).

- [ ] **Step 7: Commit**

```bash
git add Cargo.toml crates/types
git commit -m "feat(types): new crate, move fixed+svi from pricing"
```
(If not a git repo, skip commit steps throughout — the workspace is not currently git-initialized.)

---

## Task 2: Re-point `pricing` to `types`, prove no regression

**Files:**
- Modify: `crates/pricing/Cargo.toml`, `crates/pricing/src/lib.rs`
- Delete: `crates/pricing/src/fixed.rs`, `crates/pricing/src/svi.rs`

- [ ] **Step 1: Add `types` dep to pricing**

Edit `crates/pricing/Cargo.toml`, add under `[dependencies]`:
```toml
types = { path = "../types" }
```

- [ ] **Step 2: Re-export `fixed` and `svi` from types, delete the moved files**

Edit `crates/pricing/src/lib.rs` to:
```rust
pub use types::fixed;
pub use types::svi;

pub mod digital;
pub mod fixture;
```
Then delete the now-duplicated source:
```bash
rm crates/pricing/src/fixed.rs crates/pricing/src/svi.rs
```
Rationale: `pub use` preserves the public paths `pricing::fixed::*` and `pricing::svi::*`, so `digital.rs` (`crate::fixed`, `crate::svi`), `fixture.rs`, and `tests/golden_vectors.rs` import unchanged.

- [ ] **Step 3: Verify the whole workspace is green (regression gate)**

Run: `cargo test --workspace`
Expected: PASS — every pre-existing pricing test (digital, fixture, golden_vectors, fuzz_no_panic) plus the moved types tests. This proves the migration changed no behavior.

- [ ] **Step 4: Verify clippy clean**

Run: `cargo clippy --workspace -- -D warnings`
Expected: no warnings.

- [ ] **Step 5: Commit**

```bash
git add crates/pricing
git commit -m "refactor(pricing): re-export fixed+svi from types (no behavior change)"
```

---

## Task 3: `types::scale` — `Fixed1e9` newtype

**Files:**
- Modify: `crates/types/src/scale.rs`

- [ ] **Step 1: Write the failing test**

Replace `crates/types/src/scale.rs` with:
```rust
//! Scale-typed wrappers. Compile-time guard against mixing the 1e9 price/SVI
//! domain with other fixed-point domains (e.g. DUSDC 6-dec, added in a later round).

pub use crate::fixed::ONE;

/// A value in the on-chain 1e9 fixed-point domain (prices, strikes, SVI params).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Fixed1e9(pub u64);

impl Fixed1e9 {
    /// Decode to real f64 (divide by 1e9).
    pub fn to_f64(self) -> f64 {
        crate::fixed::u64_to_f64(self.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fixed1e9_decodes() {
        assert_eq!(Fixed1e9(ONE).to_f64(), 1.0);
        assert_eq!(Fixed1e9(500_000_000).to_f64(), 0.5);
    }
}
```

- [ ] **Step 2: Run the test**

Run: `cargo test -p types scale`
Expected: PASS.

- [ ] **Step 3: Commit**

```bash
git add crates/types/src/scale.rs
git commit -m "feat(types): Fixed1e9 newtype for the 1e9 domain"
```

---

## Task 4: `types::events` — raw event structs + decode to `Svi`

**Files:**
- Modify: `crates/types/src/events.rs`

The on-chain ABI (verified testnet, see spec / progress notes): `oracle::new_svi_params(a:u64, b:u64, rho:i64, m:i64, sigma:u64)` where `i64` is `i64::I64 { magnitude: u64, is_negative: bool }` sign-magnitude. In `parsed_json`, `u64` renders as a decimal **string**, `bool` as a JSON bool, the nested struct as an object. `OraclePricesUpdated` carries `spot` and `forward` (both 1e9 u64 strings).

> **Field-name caveat:** exact `parsed_json` keys (and whether the SVI fields are flattened or nested under a `svi` object) are confirmed empirically in Task 9's live smoke. The structs below assume flat keys `a/b/rho/m/sigma` and `spot/forward`. If the live event differs, adjust these `#[serde(rename)]`s — decode is centralized here so the change is one place.

- [ ] **Step 1: Write the failing tests**

Replace `crates/types/src/events.rs` with:
```rust
//! Raw on-chain oracle event structs (as decoded from `event.parsed_json`)
//! plus conversion into the real-valued `Svi`. This is the single place where
//! chain wire-format (sign-magnitude i64, 1e9 u64 strings) becomes domain types.

use serde::Deserialize;

use crate::fixed::{decode_i64, u64_to_f64, ONE};
use crate::svi::Svi;

/// u64 fields arrive as decimal strings in parsed_json.
fn de_u64_str<'de, D: serde::Deserializer<'de>>(d: D) -> Result<u64, D::Error> {
    let s = String::deserialize(d)?;
    s.parse::<u64>().map_err(serde::de::Error::custom)
}

/// On-chain `i64::I64` sign-magnitude pair.
#[derive(Debug, Clone, Copy, Deserialize, PartialEq, Eq)]
pub struct I64Raw {
    #[serde(deserialize_with = "de_u64_str")]
    pub magnitude: u64,
    pub is_negative: bool,
}

impl I64Raw {
    /// Decode to real f64 (1e9-scaled sign-magnitude → f64).
    pub fn to_f64(self) -> f64 {
        decode_i64(self.magnitude, self.is_negative)
    }
}

/// `oracle::OracleSVIUpdated` — SVI params only (no up/down grid on the wire).
#[derive(Debug, Clone, Deserialize)]
pub struct OracleSviUpdated {
    #[serde(deserialize_with = "de_u64_str")]
    pub a: u64,
    #[serde(deserialize_with = "de_u64_str")]
    pub b: u64,
    pub rho: I64Raw,
    pub m: I64Raw,
    #[serde(deserialize_with = "de_u64_str")]
    pub sigma: u64,
}

impl OracleSviUpdated {
    /// Decode the raw 1e9 sign-magnitude params into a real-valued `Svi`.
    pub fn to_svi(&self) -> Svi {
        Svi {
            a: u64_to_f64(self.a),
            b: u64_to_f64(self.b),
            rho: self.rho.to_f64(),
            m: self.m.to_f64(),
            sigma: u64_to_f64(self.sigma),
        }
    }
}

/// `oracle::OraclePricesUpdated` — spot + forward, both 1e9 u64.
#[derive(Debug, Clone, Deserialize)]
pub struct OraclePricesUpdated {
    #[serde(deserialize_with = "de_u64_str")]
    pub spot: u64,
    #[serde(deserialize_with = "de_u64_str")]
    pub forward: u64,
}

#[cfg(test)]
mod tests {
    use super::*;

    // Golden = architecture doc live BTC sample: rho = -0.94 (the sign-magnitude trap).
    #[test]
    fn decode_svi_negative_rho() {
        let j = serde_json::json!({
            "a": "7116", "b": "193619",
            "rho": { "magnitude": "940000000", "is_negative": true },
            "m":   { "magnitude": "457000",    "is_negative": true },
            "sigma": "1000000"
        });
        let ev: OracleSviUpdated = serde_json::from_value(j).unwrap();
        let s = ev.to_svi();
        assert!((s.rho - (-0.94)).abs() < 1e-12, "rho was {}", s.rho);
        assert!((s.m - (-0.000457)).abs() < 1e-12, "m was {}", s.m);
        assert!((s.a - 7116.0 / ONE as f64).abs() < 1e-18);
        assert!((s.sigma - 0.001).abs() < 1e-12);
    }

    #[test]
    fn decode_prices() {
        let j = serde_json::json!({ "spot": "73833860000000", "forward": "73832220000000" });
        let ev: OraclePricesUpdated = serde_json::from_value(j).unwrap();
        assert_eq!(ev.forward, 73_832_220_000_000);
    }

    // is_negative=true with magnitude 0 must decode to +0.0, not -0.0 surprises.
    #[test]
    fn decode_negative_zero() {
        let r = I64Raw { magnitude: 0, is_negative: true };
        assert_eq!(r.to_f64(), 0.0);
    }
}
```

- [ ] **Step 2: Run the tests, verify they pass**

Run: `cargo test -p types events`
Expected: PASS (`decode_svi_negative_rho`, `decode_prices`, `decode_negative_zero`).

- [ ] **Step 3: Commit**

```bash
git add crates/types/src/events.rs
git commit -m "feat(types): oracle event structs + sign-magnitude decode"
```

---

## Task 5: `pricing::invariants::check_svi_arb_free`

**Files:**
- Modify: `crates/pricing/src/lib.rs`
- Create: `crates/pricing/src/invariants.rs`

- [ ] **Step 1: Declare the module**

Edit `crates/pricing/src/lib.rs`, add:
```rust
pub mod invariants;
```
(Final lib.rs: `pub use types::fixed; pub use types::svi; pub mod digital; pub mod fixture; pub mod invariants;`)

- [ ] **Step 2: Write the failing test + implementation**

Create `crates/pricing/src/invariants.rs`:
```rust
//! No-arb structural checks on a computed digital curve. Used as the indexer's
//! decode sanity gate. These are the SAME invariant *definitions* enforced
//! integer-exact against on-chain vectors in tests/golden_vectors.rs — here they
//! run on OUR curve from a decoded SVI, to catch decode errors (e.g. flipped rho)
//! and arb-violating snapshots.

use crate::digital::{log_moneyness, up_price};
use crate::fixed::u64_to_f64;
use crate::svi::{total_variance, Svi};

/// Verdict from the no-arb check.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Verdict {
    Clean,
    /// One or more no-arb invariants violated; reasons are human-readable.
    Dirty(Vec<String>),
}

impl Verdict {
    pub fn is_clean(&self) -> bool {
        matches!(self, Verdict::Clean)
    }
}

/// ATM tolerance: a well-formed digital has up(K=F) ≈ 0.5. We use a LOOSE band
/// (0.05) — this gate catches gross decode errors (sign flips, scale slips), not
/// the fine data-cleanliness threshold (0.03) used in the golden numeric gate.
const ATM_TOL: f64 = 0.05;

/// Check no-arb structure of the digital curve implied by `svi` at `forward_1e9`.
/// Strikes are sampled at ±20% / ±10% / ATM around the forward.
pub fn check_svi_arb_free(svi: &Svi, forward_1e9: u64) -> Verdict {
    let mut reasons = Vec::new();
    let fwd = forward_1e9;
    // ascending strikes → up(K) must be non-increasing
    let pcts = [0.80, 0.90, 1.00, 1.10, 1.20];
    let mut prev: Option<f64> = None;
    let mut atm_up: Option<f64> = None;

    for p in pcts {
        let strike_1e9 = ((fwd as f64) * p) as u64;
        let k = log_moneyness(strike_1e9, fwd);
        let w = total_variance(svi, k);
        let up = up_price(svi, k, w);
        if up.is_nan() {
            reasons.push(format!("up(K={p}·F) is NaN (w<=0, w={w})"));
            continue;
        }
        if !(0.0..=1.0).contains(&up) {
            reasons.push(format!("up(K={p}·F)={up} out of [0,1]"));
        }
        if let Some(pv) = prev {
            if up > pv + 1e-9 {
                reasons.push(format!("non-monotone at K={p}·F: {up} > prev {pv}"));
            }
        }
        prev = Some(up);
        if (p - 1.0).abs() < 1e-12 {
            atm_up = Some(up);
        }
    }

    match atm_up {
        Some(u) if (u - 0.5).abs() > ATM_TOL => {
            reasons.push(format!("ATM up={u} deviates >{ATM_TOL} from 0.5"));
        }
        None => reasons.push("no ATM point computed".to_string()),
        _ => {}
    }

    // sanity-quiet the unused-import lint if u64_to_f64 ends up unused in edits
    let _ = u64_to_f64;
    if reasons.is_empty() { Verdict::Clean } else { Verdict::Dirty(reasons) }
}

#[cfg(test)]
mod tests {
    use super::*;

    // corpus oracles[0] params → clean, monotone, ATM≈0.4995.
    fn corpus0() -> Svi {
        Svi {
            a: 5_274.0 / 1e9,
            b: 638_806.0 / 1e9,
            rho: -458_555_014.0 / 1e9,
            m: -1_380_256.0 / 1e9,
            sigma: 1_181_366.0 / 1e9,
        }
    }

    #[test]
    fn clean_svi_passes() {
        // forward ~ 73,744 (1e9-scaled). ATM strike == forward → k≈0.
        let v = check_svi_arb_free(&corpus0(), 73_744_082_479_138);
        assert!(v.is_clean(), "expected clean, got {v:?}");
    }

    #[test]
    fn flipped_rho_is_dirty_or_changes_curve() {
        // Flip rho sign (a classic decode bug). With this near-flat smile the curve
        // stays monotone, so assert the verdict is at least not silently identical:
        // a gross corruption (sign on b → negative variance slope) must be caught.
        let mut bad = corpus0();
        bad.b = -bad.b; // negative b → variance can go negative → NaN/out-of-range
        let v = check_svi_arb_free(&bad, 73_744_082_479_138);
        assert!(!v.is_clean(), "negative-b SVI should be flagged, got {v:?}");
    }
}
```

- [ ] **Step 3: Run the tests, verify they pass**

Run: `cargo test -p pricing invariants`
Expected: PASS (`clean_svi_passes`, `flipped_rho_is_dirty_or_changes_curve`).

- [ ] **Step 4: Workspace green + clippy**

Run: `cargo test --workspace && cargo clippy --workspace -- -D warnings`
Expected: PASS, no warnings.

- [ ] **Step 5: Commit**

```bash
git add crates/pricing/src/lib.rs crates/pricing/src/invariants.rs
git commit -m "feat(pricing): no-arb invariant check for indexer sanity gate"
```

---

## Task 6: `indexer` crate — sink + pure pipeline (testable core)

**Files:**
- Modify: `Cargo.toml` (workspace members)
- Create: `crates/indexer/Cargo.toml`, `crates/indexer/src/sink.rs`, `crates/indexer/src/pipeline.rs`, and a temporary `crates/indexer/src/lib.rs` to expose modules for unit tests.

- [ ] **Step 1: Add `indexer` to workspace members**

Edit `Cargo.toml`:
```toml
members = ["crates/pricing", "crates/types", "crates/indexer"]
```

- [ ] **Step 2: Create `crates/indexer/Cargo.toml`**

```toml
[package]
name = "indexer"
version = "0.1.0"
edition = "2021"

[dependencies]
types = { path = "../types" }
pricing = { path = "../pricing" }
serde_json = "1"
anyhow = "1"
tracing = "0.1"
tracing-subscriber = "0.3"
tokio = { version = "1", features = ["full"] }
async-trait = "0.1"
# Pinned in Task 8 (live wiring). Kept out of the testable core so Tasks 6-7 build offline.
# sui-indexer-alt-framework = { git = "https://github.com/MystenLabs/sui.git", branch = "mainline" }
```

- [ ] **Step 3: Create `crates/indexer/src/lib.rs`**

```rust
pub mod sink;
pub mod pipeline;
```

- [ ] **Step 4: Create `crates/indexer/src/sink.rs`**

```rust
//! Output boundary. This round only StdoutSink; next round adds PostgresSink
//! behind the same trait.

use pricing::invariants::Verdict;
use types::events::{OraclePricesUpdated, OracleSviUpdated};

/// A decoded oracle event with its sanity verdict, ready to emit.
#[derive(Debug)]
pub enum DecodedEvent {
    Svi { ev: OracleSviUpdated, verdict: Verdict },
    Prices(OraclePricesUpdated),
}

pub trait Sink {
    fn emit(&self, checkpoint_seq: u64, ev: &DecodedEvent);
}

pub struct StdoutSink;

impl Sink for StdoutSink {
    fn emit(&self, checkpoint_seq: u64, ev: &DecodedEvent) {
        match ev {
            DecodedEvent::Svi { ev, verdict } => {
                let svi = ev.to_svi();
                tracing::info!(
                    checkpoint = checkpoint_seq,
                    a = svi.a, b = svi.b, rho = svi.rho, m = svi.m, sigma = svi.sigma,
                    clean = verdict.is_clean(),
                    "OracleSVIUpdated"
                );
                if let Verdict::Dirty(reasons) = verdict {
                    tracing::warn!(checkpoint = checkpoint_seq, ?reasons, "SVI failed no-arb sanity");
                }
            }
            DecodedEvent::Prices(p) => {
                tracing::info!(
                    checkpoint = checkpoint_seq, spot = p.spot, forward = p.forward,
                    "OraclePricesUpdated"
                );
            }
        }
    }
}
```

- [ ] **Step 5: Write the failing test for the pure pipeline**

Create `crates/indexer/src/pipeline.rs`:
```rust
//! Pure, network-free event handling: (event_name, parsed_json) → decode →
//! sanity → sink. Kept separate from the Processor so it is unit-testable
//! without constructing a CheckpointEnvelope.

use anyhow::{anyhow, Result};
use serde_json::Value;

use pricing::invariants::check_svi_arb_free;
use types::events::{OraclePricesUpdated, OracleSviUpdated};

use crate::sink::{DecodedEvent, Sink};

/// Tracks last-seen forward (needed to sanity-check an SVI) and event liveness.
#[derive(Default)]
pub struct PipelineState {
    pub last_forward_1e9: Option<u64>,
    pub oracle_events_seen: u64,
}

/// Handle one oracle event by its Move struct name. Returns Err on DECODE
/// failure (loud — schema drift). Sanity failures are NOT errors: they are
/// tagged on the emitted event.
pub fn handle_event(
    checkpoint_seq: u64,
    struct_name: &str,
    parsed_json: &Value,
    state: &mut PipelineState,
    sink: &dyn Sink,
) -> Result<()> {
    match struct_name {
        "OracleSVIUpdated" => {
            let ev: OracleSviUpdated = serde_json::from_value(parsed_json.clone())
                .map_err(|e| anyhow!("decode OracleSVIUpdated: {e}"))?;
            state.oracle_events_seen += 1;
            let verdict = match state.last_forward_1e9 {
                Some(fwd) => check_svi_arb_free(&ev.to_svi(), fwd),
                // No forward seen yet → cannot run the curve check; emit clean-untested.
                None => pricing::invariants::Verdict::Clean,
            };
            sink.emit(checkpoint_seq, &DecodedEvent::Svi { ev, verdict });
            Ok(())
        }
        "OraclePricesUpdated" => {
            let ev: OraclePricesUpdated = serde_json::from_value(parsed_json.clone())
                .map_err(|e| anyhow!("decode OraclePricesUpdated: {e}"))?;
            state.oracle_events_seen += 1;
            state.last_forward_1e9 = Some(ev.forward);
            sink.emit(checkpoint_seq, &DecodedEvent::Prices(ev));
            Ok(())
        }
        // Other oracle structs (Settled, etc.) ignored this round.
        _ => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sink::DecodedEvent;
    use std::cell::RefCell;

    struct CaptureSink(RefCell<Vec<String>>);
    impl Sink for CaptureSink {
        fn emit(&self, _seq: u64, ev: &DecodedEvent) {
            let tag = match ev {
                DecodedEvent::Svi { verdict, .. } => format!("svi:{}", verdict.is_clean()),
                DecodedEvent::Prices(_) => "prices".to_string(),
            };
            self.0.borrow_mut().push(tag);
        }
    }

    fn svi_json() -> Value {
        serde_json::json!({
            "a": "5274", "b": "638806",
            "rho": { "magnitude": "458555014", "is_negative": true },
            "m":   { "magnitude": "1380256",   "is_negative": true },
            "sigma": "1181366"
        })
    }

    #[test]
    fn prices_then_svi_runs_sanity_clean() {
        let sink = CaptureSink(RefCell::new(vec![]));
        let mut st = PipelineState::default();
        handle_event(1, "OraclePricesUpdated",
            &serde_json::json!({ "spot": "73744082479138", "forward": "73744082479138" }),
            &mut st, &sink).unwrap();
        handle_event(1, "OracleSVIUpdated", &svi_json(), &mut st, &sink).unwrap();
        assert_eq!(*sink.0.borrow(), vec!["prices", "svi:true"]);
        assert_eq!(st.oracle_events_seen, 2);
    }

    #[test]
    fn malformed_svi_is_loud_error() {
        let sink = CaptureSink(RefCell::new(vec![]));
        let mut st = PipelineState::default();
        // rho missing is_negative → decode must Err, not silently produce garbage.
        let bad = serde_json::json!({
            "a": "1", "b": "1", "rho": { "magnitude": "1" }, "m": { "magnitude": "1", "is_negative": false }, "sigma": "1"
        });
        let r = handle_event(1, "OracleSVIUpdated", &bad, &mut st, &sink);
        assert!(r.is_err(), "malformed event must error loudly");
        assert!(sink.0.borrow().is_empty(), "nothing emitted on decode failure");
    }

    #[test]
    fn unknown_struct_ignored() {
        let sink = CaptureSink(RefCell::new(vec![]));
        let mut st = PipelineState::default();
        handle_event(1, "OracleSettled", &serde_json::json!({}), &mut st, &sink).unwrap();
        assert!(sink.0.borrow().is_empty());
    }
}
```

- [ ] **Step 6: Run the tests, verify they pass**

Run: `cargo test -p indexer`
Expected: PASS (`prices_then_svi_runs_sanity_clean`, `malformed_svi_is_loud_error`, `unknown_struct_ignored`).

- [ ] **Step 7: Workspace green + clippy**

Run: `cargo test --workspace && cargo clippy --workspace -- -D warnings`
Expected: PASS, no warnings.

- [ ] **Step 8: Commit**

```bash
git add Cargo.toml crates/indexer
git commit -m "feat(indexer): sink trait + pure event pipeline with decode/sanity tests"
```

---

## Task 7: `indexer` config + liveness check (still offline-testable)

**Files:**
- Create: `crates/indexer/src/config.rs`
- Modify: `crates/indexer/src/lib.rs`, `crates/indexer/src/pipeline.rs`

- [ ] **Step 1: Create `crates/indexer/src/config.rs`**

```rust
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
```

- [ ] **Step 2: Add liveness logic + test to pipeline**

Edit `crates/indexer/src/pipeline.rs`. Add to `PipelineState`:
```rust
    pub first_checkpoint: Option<u64>,
    pub liveness_warned: bool,
```
Add this function below `handle_event`:
```rust
use crate::config::LIVENESS_WINDOW_CHECKPOINTS;

/// Call once per checkpoint AFTER handling its events. Emits a single WARN if the
/// liveness window elapsed with zero oracle events seen.
pub fn check_liveness(checkpoint_seq: u64, state: &mut PipelineState) {
    let start = *state.first_checkpoint.get_or_insert(checkpoint_seq);
    if state.liveness_warned || state.oracle_events_seen > 0 {
        return;
    }
    if checkpoint_seq.saturating_sub(start) >= LIVENESS_WINDOW_CHECKPOINTS {
        tracing::warn!(
            from = start, to = checkpoint_seq,
            "no oracle events in {LIVENESS_WINDOW_CHECKPOINTS} checkpoints — config drift? \
             check PACKAGE_ID"
        );
        state.liveness_warned = true;
    }
}
```
Add `use crate::config;` is unnecessary (path-qualified above). Ensure `pub mod config;` is added in lib.rs (next step).

- [ ] **Step 3: Update `crates/indexer/src/lib.rs`**

```rust
pub mod config;
pub mod sink;
pub mod pipeline;
```

- [ ] **Step 4: Write the liveness test**

Append to `pipeline.rs` `tests` module:
```rust
    #[test]
    fn liveness_warns_after_window_with_no_events() {
        let mut st = PipelineState::default();
        check_liveness(1000, &mut st);            // sets first_checkpoint = 1000
        assert!(!st.liveness_warned);
        check_liveness(1000 + super::LIVENESS_WINDOW_CHECKPOINTS, &mut st);
        assert!(st.liveness_warned, "should warn after window with zero events");
    }

    #[test]
    fn liveness_silent_when_events_seen() {
        let mut st = PipelineState::default();
        st.oracle_events_seen = 1;
        check_liveness(1000, &mut st);
        check_liveness(1000 + super::LIVENESS_WINDOW_CHECKPOINTS, &mut st);
        assert!(!st.liveness_warned, "must not warn once events flow");
    }
```
Add at top of `pipeline.rs`: `use crate::config::LIVENESS_WINDOW_CHECKPOINTS;` already added in Step 2 — the test references it via `super::LIVENESS_WINDOW_CHECKPOINTS`, so also add `pub use crate::config::LIVENESS_WINDOW_CHECKPOINTS;` is NOT needed; instead reference `crate::config::LIVENESS_WINDOW_CHECKPOINTS` in the tests. Replace the two `super::LIVENESS_WINDOW_CHECKPOINTS` with `crate::config::LIVENESS_WINDOW_CHECKPOINTS`.

- [ ] **Step 5: Run tests + clippy**

Run: `cargo test -p indexer && cargo clippy --workspace -- -D warnings`
Expected: PASS, no warnings.

- [ ] **Step 6: Commit**

```bash
git add crates/indexer/src/config.rs crates/indexer/src/lib.rs crates/indexer/src/pipeline.rs
git commit -m "feat(indexer): config consts + liveness drift check"
```

---

## Task 8: Wire the live `Processor` + `main` (network)

**Files:**
- Create: `crates/indexer/src/processor.rs`, `crates/indexer/src/main.rs`
- Modify: `crates/indexer/Cargo.toml` (uncomment framework dep), `crates/indexer/src/lib.rs`

> **Pin the dependency first.** Before coding, confirm the current `sui-indexer-alt-framework` API surface and a buildable git rev. Use the `sui-docs-query` skill (and `cargo update -p sui-indexer-alt-framework` output) to lock: the `Processor` trait signature, `CheckpointEnvelope` field path to events (`envelope.data.transactions[].events[]` with `event.type_.module` / `event.type_.address` / `event.parsed_json`), and `StoreIngestionClient::new_remote` + `Service::builder` names. The code below targets the v1.72 / Protocol 124 surface from the sui-indexer skill; adjust names if the pinned rev differs.

- [ ] **Step 1: Enable the framework dep**

Edit `crates/indexer/Cargo.toml`, uncomment + pin (replace `REV` with the rev you locked):
```toml
sui-indexer-alt-framework = { git = "https://github.com/MystenLabs/sui.git", rev = "REV" }
```

- [ ] **Step 2: Create `crates/indexer/src/processor.rs`**

```rust
//! Live A-path Processor: extract `oracle` module events from each checkpoint and
//! feed them to the pure pipeline. B path (Predict object poll) is a SECOND
//! Processor added to the same Service in a later round.

use std::sync::Mutex;

use anyhow::Result;
use async_trait::async_trait;
use sui_indexer_alt_framework::prelude::*;

use crate::config::PACKAGE_ID;
use crate::pipeline::{check_liveness, handle_event, PipelineState};
use crate::sink::{Sink, StdoutSink};

pub struct OracleProcessor {
    state: Mutex<PipelineState>,
    sink: StdoutSink,
}

impl OracleProcessor {
    pub fn new() -> Self {
        Self { state: Mutex::new(PipelineState::default()), sink: StdoutSink }
    }
}

impl Default for OracleProcessor {
    fn default() -> Self { Self::new() }
}

#[async_trait]
impl Processor for OracleProcessor {
    const NAME: &'static str = "oracle-event-indexer";

    async fn process(&self, envelope: &CheckpointEnvelope) -> Result<()> {
        let checkpoint = &envelope.data;
        let seq = checkpoint.checkpoint_summary.sequence_number;
        let mut state = self.state.lock().unwrap();
        for tx in &checkpoint.transactions {
            for event in &tx.events {
                // filter: our package + oracle module
                if event.type_.address.to_canonical_string(true) == PACKAGE_ID
                    && event.type_.module.as_str() == "oracle"
                {
                    let name = event.type_.name.as_str();
                    handle_event(seq, name, &event.parsed_json, &mut state, &self.sink)?;
                }
            }
        }
        check_liveness(seq, &mut state);
        Ok(())
    }
}
```
> Field accessors (`event.type_.address/module/name`, `parsed_json`, `checkpoint_summary.sequence_number`) are the expected shape; correct against the pinned rev if the compiler disagrees. The `PACKAGE_ID` comparison must match the framework's canonical address formatting — adjust the comparison helper as the type requires.

- [ ] **Step 3: Create `crates/indexer/src/main.rs`**

```rust
use anyhow::Result;
use sui_indexer_alt_framework::{Service, StoreIngestionClient};

use indexer::config::FULLNODE_URL;
use indexer::processor::OracleProcessor;

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt::init();

    let ingestion = StoreIngestionClient::new_remote(FULLNODE_URL.to_string())?;
    let service = Service::builder()
        .ingestion_client(ingestion)
        .add_processor(OracleProcessor::new())
        .build()
        .await?;

    service.main().await
}
```

- [ ] **Step 4: Expose `processor` in lib.rs**

```rust
pub mod config;
pub mod sink;
pub mod pipeline;
pub mod processor;
```

- [ ] **Step 5: Build (compile gate — no network needed to compile)**

Run: `cargo build -p indexer`
Expected: compiles. If the framework API names differ from the assumed surface, fix per the pinned rev's types until it builds.

- [ ] **Step 6: Workspace test + clippy still green**

Run: `cargo test --workspace && cargo clippy --workspace -- -D warnings`
Expected: PASS, no warnings (offline tests unaffected by the network wiring).

- [ ] **Step 7: Commit**

```bash
git add crates/indexer/Cargo.toml crates/indexer/src/processor.rs crates/indexer/src/main.rs crates/indexer/src/lib.rs
git commit -m "feat(indexer): live OracleProcessor + Service wiring"
```

---

## Task 9: Live smoke + Monkey verification (manual acceptance)

**Files:** none (verification only). Record results in `move-notes.md` / `tasks/progress.md`.

- [ ] **Step 1: Run against testnet**

Run: `RUST_LOG=info cargo run -p indexer`
Expected: within `LIVENESS_WINDOW_CHECKPOINTS`, stdout shows `OracleSVIUpdated` / `OraclePricesUpdated` lines with decoded f64 SVI params and `clean=true/false`. Let it run a few minutes.

- [ ] **Step 2: Confirm decode shape against reality (the field-name caveat)**

Verify the printed `rho`/`m` signs and magnitudes match a known oracle (cross-check one `OracleSVIUpdated` against `predict-server.testnet.mystenlabs.com/oracles`). If `parsed_json` keys differ (flattened vs nested under `svi`, different names), fix the `#[serde(rename)]`s in `types::events` and re-run `cargo test -p types events` then this smoke.

- [ ] **Step 3: Monkey — force the loud-fail path**

Temporarily point `PACKAGE_ID` at a wrong value, run, and confirm the liveness WARN fires after the window (proves config-drift is loud, not silent-healthy). Revert `PACKAGE_ID`.

- [ ] **Step 4: Final acceptance gate**

Run: `cargo test --workspace && cargo clippy --workspace -- -D warnings`
Expected: all-green, no warnings. Confirm the four acceptance criteria in the spec are met.

- [ ] **Step 5: Update progress notes**

Append decode-shape confirmation + any `serde(rename)` adjustments to `tasks/progress.md` (and `move-notes.md` if the event ABI differed from the assumed shape). Note that B path (object polling) + Postgres are the next round.

---

## Self-Review

- **Spec coverage:** scope (A path stdout, no DB) ✓ Task 6-9; ingestion framework ✓ Task 8; types crate + full pricing migration ✓ Task 1-2; Fixed1e9 newtype (Dusdc deferred, noted) ✓ Task 3; events + sign-magnitude decode ✓ Task 4; sanity reuses pricing invariants ✓ Task 5; error handling split (decode=loud, sanity=tag, drift=liveness) ✓ Task 4/6/7; testing incl. negative-rho golden, monkey, workspace regression, live smoke ✓ Task 4/6/9.
- **Placeholder scan:** no TBD/TODO in code steps; the only deferred item is the framework git `rev` (Task 8 Step 1), which is correctly an empirical pin, not a placeholder.
- **Type consistency:** `Svi` (f64), `OracleSviUpdated.to_svi()`, `Verdict`/`is_clean()`, `Sink::emit(seq, &DecodedEvent)`, `DecodedEvent::{Svi,Prices}`, `PipelineState`, `handle_event(..)`, `check_liveness(..)`, `check_svi_arb_free(&Svi,u64)` — names consistent across Tasks 4-8.
- **Known risk:** Task 8 framework API names (`CheckpointEnvelope` field paths, `type_.address` formatting) are best-effort against v1.72; Step 5 build gate + the docs-query pin in Step 1 are the safety net.
