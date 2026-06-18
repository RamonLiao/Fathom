# Pricing Validator Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Build a Rust `pricing` crate that reproduces DeepBook Predict's on-chain SVI → binary-digital `up` price off-chain and pins it against live golden vectors, so the future Rust indexer reuses validated fixed-point math instead of re-porting it.

**Architecture:** Cargo workspace at repo root; first member `crates/pricing` is a lib crate (fixed-point decode + SVI + digital pricing). Validation is approach B: soft numeric tolerance (`|my−onchain| ≤ 1e-3`) + strict integer-exact structural invariants, with dirty/stale snapshots quarantined out of the numeric gate. All math in f64 (libm `erf`); chain values are 1e9-scaled, `rho`/`m` are i64 sign-magnitude.

**Tech Stack:** Rust 1.94, Cargo workspace, `serde` + `serde_json` (fixture parse), `libm` (`erfc`).

**Scope guard (R4):** This crate does **NOT** compute NAV or touch DUSDC 6-dec amounts. NAV/`vault_value` mirroring is deferred to the indexer stage. No DUSDC helper in `fixed.rs` (YAGNI).

**Data-source note (R1):** JSON-RPC removal (Protocol 124, ~2026-04) makes re-capturing fixtures via the old `*.mjs` devInspect path risky. We do **not** re-capture. The dirty single-vector `golden-binary-price.json` is left untouched as a known-anomaly artifact; the golden test points at a **clean corpus oracle** (corpus `oracles[0]`, ATM `up=499510178` ≈ 0.4995e9). Existing fixtures are the on-chain truth — no chain access needed for this plan.

**Fixture shape (verified 2026-06-15):**
- `fixtures/golden-corpus.json`: `{ oracles: [ { oracle_id, underlying, expiry_ms, active, settlement_price, spot, forward, svi:{a,b,rho,m,sigma}, err, vectors:[{pct, strike, up, down, compute_price}] } ] }`. All numeric fields are **decimal strings**. `a/b/sigma` are unsigned 1e9; `rho/m` are **signed** decimal strings 1e9-scaled (e.g. `"-458555014"`). `pct` is the strike's % offset from forward → `pct==0` is the ATM point.
- `fixtures/golden-binary-price.json`: single oracle, `oracle_state.{svi, prices.forward, expiry_ms, active}` + `vectors:[{strike, up, down, compute_price}]` (no `pct`). Dirty (ATM up=0.5488). Not used by the numeric gate.

---

### Task 1: Workspace + crate skeleton

**Files:**
- Create: `Cargo.toml` (workspace root — repo root of `02-sui-transparency-hub/`)
- Create: `crates/pricing/Cargo.toml`
- Create: `crates/pricing/src/lib.rs`

- [ ] **Step 1: Create workspace root manifest**

Create `Cargo.toml`:

```toml
[workspace]
resolver = "2"
members = ["crates/pricing"]
```

- [ ] **Step 2: Create crate manifest**

Create `crates/pricing/Cargo.toml`:

```toml
[package]
name = "pricing"
version = "0.1.0"
edition = "2021"

[dependencies]
serde = { version = "1", features = ["derive"] }
serde_json = "1"
libm = "0.2"
```

- [ ] **Step 3: Create empty lib root**

Create `crates/pricing/src/lib.rs`:

```rust
pub mod fixed;
pub mod svi;
pub mod digital;
pub mod fixture;
```

- [ ] **Step 4: Create the four module files as empty stubs so the crate compiles**

Create `crates/pricing/src/fixed.rs`, `crates/pricing/src/svi.rs`, `crates/pricing/src/digital.rs`, `crates/pricing/src/fixture.rs` each containing a single line:

```rust
// implemented in later tasks
```

- [ ] **Step 5: Verify it compiles**

Run: `cargo build`
Expected: PASS (`Compiling pricing v0.1.0` … `Finished`). Warnings about empty modules are fine.

- [ ] **Step 6: Commit**

```bash
git add Cargo.toml crates/pricing/Cargo.toml crates/pricing/src
git commit -m "feat(pricing): scaffold cargo workspace + pricing crate"
```

---

### Task 2: `fixed.rs` — fixed-point decode helpers

**Files:**
- Modify: `crates/pricing/src/fixed.rs`

- [ ] **Step 1: Write the failing tests**

Replace `crates/pricing/src/fixed.rs` with:

```rust
//! 1e9 fixed-point domain only. NO DUSDC 6-dec helper (YAGNI — add at indexer/NAV stage).

/// On-chain fixed-point scale for prices, strikes, and SVI params.
pub const ONE: u64 = 1_000_000_000;

/// Decode an on-chain `i64` sign-magnitude pair to a real f64.
/// Chain stores rho/m as `{ magnitude: u64, is_negative: bool }`, both 1e9-scaled.
/// (The corpus fixture already stores them as signed strings; this helper is for the
/// future indexer reading raw BCS.)
pub fn decode_i64(magnitude: u64, is_negative: bool) -> f64 {
    let v = magnitude as f64 / ONE as f64;
    if is_negative { -v } else { v }
}

/// Decode an unsigned 1e9-scaled value to real f64.
pub fn u64_to_f64(x_1e9: u64) -> f64 {
    x_1e9 as f64 / ONE as f64
}

/// Encode a real f64 back to 1e9 units, round half-up, clamp to [0, ONE].
/// Used to compare a computed probability against an on-chain `up` value.
pub fn f64_to_1e9(x: f64) -> u64 {
    let scaled = (x * ONE as f64).round();
    if scaled <= 0.0 {
        0
    } else if scaled >= ONE as f64 {
        ONE
    } else {
        scaled as u64
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decode_i64_signs() {
        assert_eq!(decode_i64(940_000_000, true), -0.94);
        assert_eq!(decode_i64(457_398, false), 0.000_457_398);
        assert_eq!(decode_i64(0, true), 0.0);
    }

    #[test]
    fn u64_to_f64_scales() {
        assert_eq!(u64_to_f64(ONE), 1.0);
        assert_eq!(u64_to_f64(500_000_000), 0.5);
    }

    #[test]
    fn f64_to_1e9_rounds_and_clamps() {
        assert_eq!(f64_to_1e9(0.5), 500_000_000);
        assert_eq!(f64_to_1e9(-0.1), 0);          // clamp low
        assert_eq!(f64_to_1e9(1.5), ONE);          // clamp high
        assert_eq!(f64_to_1e9(0.499_999_999_4), 499_999_999); // round half-up boundary
    }
}
```

- [ ] **Step 2: Run tests to verify they pass**

Run: `cargo test -p pricing fixed::`
Expected: PASS (4 tests). (Implementation is written alongside the tests here because the functions are pure one-liners; the test encodes the sign-magnitude and round-half-up intent.)

- [ ] **Step 3: Commit**

```bash
git add crates/pricing/src/fixed.rs
git commit -m "feat(pricing): fixed-point decode helpers (1e9 domain)"
```

---

### Task 3: `svi.rs` — SVI total variance

**Files:**
- Modify: `crates/pricing/src/svi.rs`

- [ ] **Step 1: Write the failing test**

Replace `crates/pricing/src/svi.rs` with:

```rust
//! SVI total-variance parameterization. Real-valued (decode at I/O boundary first).

/// SVI params in real (decoded) units.
#[derive(Debug, Clone, Copy)]
pub struct Svi {
    pub a: f64,
    pub b: f64,
    pub rho: f64,
    pub m: f64,
    pub sigma: f64,
}

/// Total variance w(k) = a + b·(ρ·(k−m) + √((k−m)² + σ²)).
pub fn total_variance(s: &Svi, k: f64) -> f64 {
    let km = k - s.m;
    s.a + s.b * (s.rho * km + (km * km + s.sigma * s.sigma).sqrt())
}

/// Implied vol IV = √(w / T). Caller guards T > 0 (expired oracle → don't call).
pub fn iv(w: f64, t_years: f64) -> f64 {
    (w / t_years).sqrt()
}

#[cfg(test)]
mod tests {
    use super::*;

    // Reproduces corpus oracles[0] ATM point (k≈0). Hand-computed expected w ≈ 6.03e-6.
    #[test]
    fn total_variance_atm_corpus0() {
        let s = Svi {
            a: 5_274.0 / 1e9,
            b: 638_806.0 / 1e9,
            rho: -458_555_014.0 / 1e9,
            m: -1_380_256.0 / 1e9,
            sigma: 1_181_366.0 / 1e9,
        };
        let w = total_variance(&s, 0.0);
        assert!((w - 6.03e-6).abs() < 1e-7, "w was {w}");
    }

    #[test]
    fn iv_basic() {
        assert!((iv(0.04, 0.25) - 0.4).abs() < 1e-12);
    }
}
```

- [ ] **Step 2: Run tests to verify they pass**

Run: `cargo test -p pricing svi::`
Expected: PASS (2 tests).

- [ ] **Step 3: Commit**

```bash
git add crates/pricing/src/svi.rs
git commit -m "feat(pricing): SVI total_variance + iv"
```

---

### Task 4: `digital.rs` — log-moneyness, normal CDF, binary up price

**Files:**
- Modify: `crates/pricing/src/digital.rs`

- [ ] **Step 1: Write the failing test**

Replace `crates/pricing/src/digital.rs` with:

```rust
//! Binary-digital option pricing. up(K) = N(d₂), d₂ = (−k − ½w)/√w.

use crate::fixed::u64_to_f64;
use crate::svi::Svi;

/// Log-moneyness k = ln(K / F). Strike and forward are both 1e9-scaled, so the
/// scale cancels in the ratio.
pub fn log_moneyness(strike_1e9: u64, forward_1e9: u64) -> f64 {
    (u64_to_f64(strike_1e9) / u64_to_f64(forward_1e9)).ln()
}

/// Standard normal CDF: N(x) = 0.5·erfc(−x/√2).
pub fn normal_cdf(x: f64) -> f64 {
    0.5 * libm::erfc(-x / std::f64::consts::SQRT_2)
}

/// Binary digital up price up(K) = P(S_T ≥ K) = N(d₂), clamped to [0,1].
/// w is total variance (caller passes `svi::total_variance(s, k)`).
/// Returns NaN if w ≤ 0 (caller treats NaN as "skip numeric point").
pub fn up_price(_s: &Svi, k: f64, w: f64) -> f64 {
    if w <= 0.0 {
        return f64::NAN;
    }
    let sqrt_w = w.sqrt();
    let d2 = (-k - 0.5 * w) / sqrt_w;
    normal_cdf(d2).clamp(0.0, 1.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normal_cdf_known_points() {
        assert!((normal_cdf(0.0) - 0.5).abs() < 1e-12);
        assert!((normal_cdf(1.96) - 0.975).abs() < 1e-3);
        assert!((normal_cdf(-1.96) - 0.025).abs() < 1e-3);
    }

    #[test]
    fn log_moneyness_cancels_scale() {
        // K == F → k == 0
        assert!(log_moneyness(73_744_082_479_138, 73_744_082_479_138).abs() < 1e-12);
        // K > F → k > 0
        assert!(log_moneyness(74_000_000_000_000, 73_000_000_000_000) > 0.0);
    }

    // Reproduces corpus oracles[0] ATM up=499510178 (0.49951).
    #[test]
    fn up_price_atm_corpus0() {
        let s = Svi {
            a: 5_274.0 / 1e9,
            b: 638_806.0 / 1e9,
            rho: -458_555_014.0 / 1e9,
            m: -1_380_256.0 / 1e9,
            sigma: 1_181_366.0 / 1e9,
        };
        let k = 0.0; // ATM strike == forward
        let w = crate::svi::total_variance(&s, k);
        let up = up_price(&s, k, w);
        assert!((up - 0.499_510_178).abs() < 1e-3, "up was {up}");
    }

    #[test]
    fn up_price_nan_on_nonpositive_w() {
        let s = Svi { a: 0.0, b: 0.0, rho: 0.0, m: 0.0, sigma: 0.0 };
        assert!(up_price(&s, 0.0, 0.0).is_nan());
    }
}
```

- [ ] **Step 2: Run tests to verify they pass**

Run: `cargo test -p pricing digital::`
Expected: PASS (4 tests). The ATM reproduction within 1e-3 is the core formula confirmation.

- [ ] **Step 3: Commit**

```bash
git add crates/pricing/src/digital.rs
git commit -m "feat(pricing): digital up_price = N(d2) + normal_cdf + log_moneyness"
```

---

### Task 5: `fixture.rs` — corpus parsing + ATM quarantine

**Files:**
- Modify: `crates/pricing/src/fixture.rs`

- [ ] **Step 1: Write the failing test**

Replace `crates/pricing/src/fixture.rs` with:

```rust
//! Parse golden-corpus.json and decide which oracles are clean vs quarantined.

use serde::Deserialize;

use crate::fixed::{u64_to_f64, ONE};
use crate::svi::Svi;

/// ATM quarantine threshold (R3): an oracle whose ATM (pct==0) `up` deviates from
/// 0.5e9 by more than this is a dirty/mid-SVI-update snapshot, excluded from the
/// numeric gate. Provenance: 2/19 corpus oracles captured mid-update have ATM up of
/// 0.463 and 0.549; clean oracles sit within ~5e-4 of 0.5. 0.03e9 separates them with
/// wide margin. Documented in spec §0 / §2.
pub const ATM_QUARANTINE_ABS_1E9: i64 = 30_000_000; // 0.03 * 1e9

#[derive(Debug, Deserialize)]
pub struct Corpus {
    pub oracles: Vec<OracleRaw>,
}

#[derive(Debug, Deserialize)]
pub struct OracleRaw {
    pub oracle_id: String,
    pub underlying: String,
    pub expiry_ms: String,
    pub active: bool,
    pub settlement_price: Option<String>,
    pub spot: String,
    pub forward: String,
    pub svi: SviRaw,
    pub err: Option<String>,
    pub vectors: Vec<VectorRaw>,
}

#[derive(Debug, Deserialize)]
pub struct SviRaw {
    pub a: String,
    pub b: String,
    pub rho: String,   // signed decimal string, 1e9-scaled
    pub m: String,     // signed decimal string, 1e9-scaled
    pub sigma: String,
}

#[derive(Debug, Deserialize)]
pub struct VectorRaw {
    pub pct: f64,
    pub strike: String,
    pub up: String,
    pub down: String,
    pub compute_price: String,
}

impl SviRaw {
    /// Decode signed 1e9 strings to real f64.
    pub fn decode(&self) -> Svi {
        let p = |s: &str| s.parse::<i64>().expect("svi field i64") as f64 / ONE as f64;
        Svi { a: p(&self.a), b: p(&self.b), rho: p(&self.rho), m: p(&self.m), sigma: p(&self.sigma) }
    }
}

impl VectorRaw {
    pub fn strike_u64(&self) -> u64 { self.strike.parse().expect("strike u64") }
    pub fn up_u64(&self) -> u64 { self.up.parse().expect("up u64") }
    pub fn down_u64(&self) -> u64 { self.down.parse().expect("down u64") }
    pub fn compute_price_u64(&self) -> u64 { self.compute_price.parse().expect("cp u64") }
}

impl OracleRaw {
    pub fn forward_u64(&self) -> u64 { self.forward.parse().expect("forward u64") }

    /// True if this oracle should be excluded from the SOFT NUMERIC gate.
    /// Quarantined when: not active, settled, has a capture error, or its ATM (pct==0)
    /// `up` deviates from 0.5e9 by more than the threshold. Structural invariants are
    /// still checked for quarantined oracles.
    pub fn is_quarantined(&self) -> bool {
        if !self.active || self.settlement_price.is_some() || self.err.is_some() {
            return true;
        }
        match self.vectors.iter().find(|v| v.pct == 0.0) {
            Some(atm) => {
                let dev = atm.up_u64() as i64 - (ONE as i64) / 2;
                dev.abs() > ATM_QUARANTINE_ABS_1E9
            }
            None => true, // no ATM point → can't validate cleanliness
        }
    }
}

/// Load the corpus from a JSON string.
pub fn parse_corpus(json: &str) -> Corpus {
    serde_json::from_str(json).expect("parse golden-corpus.json")
}

#[cfg(test)]
mod tests {
    use super::*;

    const CORPUS: &str = include_str!("../../../pricing/fixtures/golden-corpus.json");

    #[test]
    fn parses_all_oracles() {
        let c = parse_corpus(CORPUS);
        assert!(c.oracles.len() >= 19, "got {} oracles", c.oracles.len());
    }

    // R2: scale sanity — decoded BTC forward must be a plausible price, catching a
    // 1e9 scale slip before it reaches NAV.
    #[test]
    fn btc_forward_scale_sane() {
        let c = parse_corpus(CORPUS);
        let btc = c.oracles.iter().find(|o| o.underlying == "BTC").expect("a BTC oracle");
        let f = u64_to_f64(btc.forward_u64());
        assert!((1_000.0..1_000_000.0).contains(&f), "BTC forward looked off-scale: {f}");
    }

    #[test]
    fn corpus0_is_clean() {
        let c = parse_corpus(CORPUS);
        assert!(!c.oracles[0].is_quarantined(), "oracles[0] should be clean (ATM≈0.4995)");
    }

    #[test]
    fn at_least_two_quarantined() {
        let c = parse_corpus(CORPUS);
        let n = c.oracles.iter().filter(|o| o.is_quarantined()).count();
        assert!(n >= 2, "expected ≥2 dirty/settled snapshots, got {n}");
    }
}
```

- [ ] **Step 2: Run tests to verify they pass**

Run: `cargo test -p pricing fixture::`
Expected: PASS (4 tests). If `parses_all_oracles` fails on a serde field-shape mismatch, inspect the offending JSON object and align the `#[derive(Deserialize)]` field names — do not loosen types to `Value`.

- [ ] **Step 3: Commit**

```bash
git add crates/pricing/src/fixture.rs
git commit -m "feat(pricing): corpus parse + ATM quarantine classifier"
```

---

### Task 6: `tests/golden_vectors.rs` — the validation gate

**Files:**
- Create: `crates/pricing/tests/golden_vectors.rs`

This is the integration test that enforces approach B: soft numeric on clean oracles + strict structural invariants on every oracle, with an error report.

- [ ] **Step 1: Write the integration test**

Create `crates/pricing/tests/golden_vectors.rs`:

```rust
//! Pins off-chain pricing against live on-chain golden corpus.
//! - Strict (zero-tolerance, integer-exact) invariants on EVERY oracle/vector.
//! - Soft numeric gate (|my−onchain| ≤ 1e-3) on CLEAN oracles only.
//! - Quarantined oracles (settled/inactive/dirty ATM) excluded from numeric gate.

use pricing::digital::{log_moneyness, up_price};
use pricing::fixed::{f64_to_1e9, ONE};
use pricing::fixture::parse_corpus;
use pricing::svi::total_variance;

const CORPUS: &str = include_str!("../../../pricing/fixtures/golden-corpus.json");
const NUMERIC_TOL_1E9: i64 = 1_000_000; // 1e-3 of probability

#[test]
fn golden_vectors_invariants_and_numeric() {
    let corpus = parse_corpus(CORPUS);

    // --- error report accumulators (numeric gate, clean oracles only) ---
    let mut max_err: i64 = 0;
    let mut sum_abs: i128 = 0;
    let mut sum_signed: i128 = 0;
    let mut n: i64 = 0;
    let mut numeric_failures: Vec<String> = Vec::new();
    let mut clean = 0usize;
    let mut quarantined = 0usize;

    for o in &corpus.oracles {
        let q = o.is_quarantined();
        if q { quarantined += 1; } else { clean += 1; }

        let svi = o.svi.decode();
        let fwd = o.forward_u64();
        let mut prev_up: Option<u64> = None;

        for v in &o.vectors {
            let up = v.up_u64();
            let down = v.down_u64();
            let cp = v.compute_price_u64();

            // --- STRICT invariants (every oracle, integer-exact) ---
            // 1. parity
            assert_eq!(up + down, ONE,
                "parity fail oracle={} strike={} up={} down={}", o.oracle_id, v.strike, up, down);
            // 2. compute_price == up
            assert_eq!(cp, up,
                "compute_price!=up oracle={} strike={}", o.oracle_id, v.strike);
            // 3a. up ∈ [0, 1e9]
            assert!(up <= ONE, "up>1e9 oracle={} strike={}", o.oracle_id, v.strike);
            // 3b. monotone non-increasing in ascending strike (vectors are pct-ordered ascending)
            if let Some(p) = prev_up {
                assert!(up <= p,
                    "non-monotone oracle={} strike={} up={} > prev={}", o.oracle_id, v.strike, up, p);
            }
            prev_up = Some(up);

            // --- SOFT numeric (clean oracles only) ---
            if q { continue; }
            let k = log_moneyness(v.strike_u64(), fwd);
            let w = total_variance(&svi, k);
            let my = up_price(&svi, k, w);
            if my.is_nan() { continue; } // w<=0 guard → skip numeric point
            let my_1e9 = f64_to_1e9(my) as i64;
            let err = my_1e9 - up as i64;
            let abs = err.abs();
            n += 1;
            sum_abs += abs as i128;
            sum_signed += err as i128;
            if abs > max_err { max_err = abs; }
            if abs > NUMERIC_TOL_1E9 {
                numeric_failures.push(format!(
                    "oracle={} strike={} my={} on={} err={}", o.oracle_id, v.strike, my_1e9, up, err));
            }
        }
    }

    let mean_abs = if n > 0 { sum_abs as f64 / n as f64 } else { 0.0 };
    let mean_signed = if n > 0 { sum_signed as f64 / n as f64 } else { 0.0 };
    println!(
        "numeric points={n} clean_oracles={clean} quarantined={quarantined} \
         max_err={max_err} mean_abs={mean_abs:.1} mean_signed={mean_signed:.1} (1e9 units)");

    assert!(numeric_failures.is_empty(),
        "{} numeric vectors exceeded tol 1e-3:\n{}",
        numeric_failures.len(), numeric_failures.join("\n"));
    assert!(clean >= 15, "expected ≥15 clean oracles, got {clean}");
}
```

- [ ] **Step 2: Run the test, capturing the report**

Run: `cargo test -p pricing --test golden_vectors -- --nocapture`
Expected: PASS. Report line prints e.g. `max_err` in the low hundreds-of-thousands (≈4e-4 of probability) per spec §0. If numeric failures appear on a clean oracle, do NOT loosen the tolerance — investigate per lessons.md (likely a dirty point the quarantine missed, or a decode bug); fix the cause.

- [ ] **Step 3: Commit**

```bash
git add crates/pricing/tests/golden_vectors.rs
git commit -m "test(pricing): golden-corpus validation gate (soft numeric + strict invariants)"
```

---

### Task 7: Fuzz/monkey property test (project test rule — extreme inputs)

**Files:**
- Modify: `crates/pricing/Cargo.toml`
- Create: `crates/pricing/tests/fuzz_no_panic.rs`

Per project `test.md` (Monkey Testing) and spec §7: random SVI params incl. `sigma=0`, `b=0`, extreme `rho` must never panic — only well-typed outputs or NaN skips. Uses a tiny deterministic LCG (no rand dep, reproducible).

- [ ] **Step 1: Write the fuzz test**

Create `crates/pricing/tests/fuzz_no_panic.rs`:

```rust
//! Monkey test: hammer pricing with degenerate/extreme SVI params; assert no panic
//! and outputs are either NaN (skipped) or a valid probability in [0,1].

use pricing::digital::up_price;
use pricing::svi::{total_variance, Svi};

// Deterministic LCG so failures reproduce (no Date/rand).
struct Lcg(u64);
impl Lcg {
    fn next_f64(&mut self, lo: f64, hi: f64) -> f64 {
        self.0 = self.0.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        let u = (self.0 >> 11) as f64 / (1u64 << 53) as f64; // [0,1)
        lo + u * (hi - lo)
    }
}

#[test]
fn fuzz_up_price_never_panics() {
    let mut rng = Lcg(0x1234_5678_9abc_def0);
    for _ in 0..100_000 {
        let s = Svi {
            a: rng.next_f64(-0.01, 0.1),       // includes negative a (can drive w<=0)
            b: rng.next_f64(0.0, 0.01),        // includes b=0
            rho: rng.next_f64(-1.0, 1.0),      // includes |rho|→1
            m: rng.next_f64(-0.05, 0.05),
            sigma: rng.next_f64(0.0, 0.01),    // includes sigma=0
        };
        let k = rng.next_f64(-0.5, 0.5);
        let w = total_variance(&s, k);
        let up = up_price(&s, k, w);
        assert!(up.is_nan() || (0.0..=1.0).contains(&up),
            "bad up={up} for svi={s:?} k={k} w={w}");
    }
}
```

- [ ] **Step 2: Run the fuzz test**

Run: `cargo test -p pricing --test fuzz_no_panic`
Expected: PASS (no panic across 100k iterations).

- [ ] **Step 3: Commit**

```bash
git add crates/pricing/tests/fuzz_no_panic.rs
git commit -m "test(pricing): monkey/fuzz degenerate SVI inputs never panic"
```

---

### Task 8: Full verification + workspace gate

**Files:** none (verification only)

- [ ] **Step 1: Run the entire suite**

Run: `cargo test --workspace -- --nocapture`
Expected: ALL PASS. Note the printed `max_err`/`mean_signed` from Task 6.

- [ ] **Step 2: Lint gate**

Run: `cargo clippy --workspace --all-targets -- -D warnings`
Expected: no warnings. Fix any clippy findings (likely needless casts/borrows) — do not `#[allow]` to silence.

- [ ] **Step 3: Commit any lint fixes**

```bash
git add -A
git commit -m "chore(pricing): clippy clean"
```

---

## Self-Review (completed against spec rev 06-03)

**Spec coverage:**
- §2 approach B soft tolerance 1e-3 → Task 6 `NUMERIC_TOL_1E9`. ✓
- §2 snapshot quarantine (ATM dev / settled / inactive) → Task 5 `is_quarantined` + Task 6 gate skip. ✓
- §2 structural invariants 1/2/3 (parity, compute_price==up, monotone+range) → Task 6 strict asserts (integer-exact). ✓
- §3 math (k, w, IV, d₂, normal_cdf) → Tasks 3 & 4. ✓
- §4 workspace + crate layout → Task 1 (note: workspace member is `crates/pricing`, fixtures stay at existing `pricing/fixtures/` — tests reference them via relative `include_str!`). ✓
- §5 module interfaces (fixed/svi/digital signatures) → Tasks 2–4, signatures match spec exactly. ✓
- §6 edge cases (w≤0 NaN guard, T≤0 → structural only, degenerate inputs) → Task 4 NaN guard + Task 7 fuzz. ✓
- §7 cargo test + --nocapture report + monkey/fuzz → Tasks 6, 7, 8. ✓
- §8 out of scope (NAV, Greeks, indexer, Walrus) → none implemented; scope guard at top. ✓

**Review additions (R1–R4):** R1 → no re-capture, golden test repointed at clean corpus oracle (header note). R2 → Task 5 `btc_forward_scale_sane`. R3 → Task 5 threshold const with provenance comment. R4 → top-of-plan scope guard.

**Path note:** Workspace lives at repo root; fixtures remain at `pricing/fixtures/`. `include_str!("../../../pricing/fixtures/golden-corpus.json")` resolves from `crates/pricing/{src,tests}/`. If the worker relocates fixtures under the crate, update the three `include_str!` paths consistently.

**Placeholder scan:** none — every code step has complete code. **Type consistency:** `Svi{a,b,rho,m,sigma}`, `total_variance(&Svi,f64)`, `up_price(&Svi,f64,f64)`, `f64_to_1e9`, `is_quarantined`, `parse_corpus` used identically across tasks. ✓
