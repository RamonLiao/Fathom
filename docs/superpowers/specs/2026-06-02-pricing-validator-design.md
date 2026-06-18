# Pricing Validator — Design Spec

_Date: 2026-06-02 (rev. 2026-06-03) · Status: approved for planning_

## 0. Empirical validation log (2026-06-03)

Before locking the spec, the assumed formula was tested offline against the existing
fixtures (no chain needed — golden vectors ARE the on-chain truth). Findings:

- **Formula `d₂ = (−k − ½w)/√w` is CONFIRMED.** Across the 19-oracle corpus
  (`golden-corpus.json`, √w from 3e-4 to 9e-2), the textbook digital reproduces
  on-chain `up` to **maxerr < 4e-4 for 17/19 oracles** (most 1e-5…1e-7, near bit-level).
  Reverse-fit `β = N⁻¹(up) − (−k)/√w` came out **= −½√w exactly**, i.e. the textbook
  drift. No decompile needed; approach A (numeric reproduction) is viable at 1e-3.
- **2/19 oracles are dirty snapshots:** their ATM `up` deviates from 0.5
  (0.463 and 0.549) → median not at forward → captured mid-SVI-update (race window),
  not a formula error. maxerr ~3.6e-2, isolated to the pct=0 point.
- **`golden-binary-price.json` is one of those dirty snapshots** (ATM up=0.5488) →
  it must be replaced with a clean oracle as the single-vector golden.
- **T is NOT needed for numeric reproduction** — `d₂` uses only total variance `w`.
  The missing `prices.timestamp` in the fixture is a non-blocker. T (`IV=√(w/T)`) is
  only needed for IV-surface display, out of this validator's numeric-comparison path.

## 1. Purpose

Pin the off-chain SVI → IV → binary-digital price math against live on-chain golden
vectors **before** the indexer is written, so the indexer can reuse a validated
pricing module instead of re-deriving (and re-bugging) the fixed-point math.

This is a **Rust** crate (`pricing`). It becomes the single source of truth the
future Rust indexer depends on (`pricing = { path = "../pricing" }`), eliminating a
TS→Rust double-port of the fixed-point logic (the main NAV-1000× risk source).

## 2. Success criteria (approach B + A)

**B — comparison strategy:** soft numeric tolerance + strict structural invariants.
We do **not** chase bit-exact reproduction of on-chain fixed-point (`normal_cdf`/`ln`/
`sqrt` bodies are not decompiled). We compute the math "true value" in f64 (libm
`erf`) and assert it lands within tolerance of the on-chain golden; the gap ≈ the
protocol's own fixed-point approximation error, which we quantify and report.

**A — assertions:**
- **Numeric (soft):** per vector, `|my_up − onchain_up| ≤ 1e-3` (≤ 0.1% probability,
  ≤ 1_000_000 in 1e9 units). Failure panics with `(oracle, K, my, on, err)`.
  Collect and print `max_err` / `mean_err` / **signed mean** over all vectors.
- **Snapshot quarantine (NOT a CI failure):** an oracle whose ATM `up` deviates from
  0.5 by more than a threshold (`|up(k≈0) − 0.5e9| > 0.03e9`), or that is settled /
  `active=false`, is flagged a **dirty/stale snapshot**, reported, and **excluded** from
  the numeric gate (structural invariants still checked). This isolates the 2 known
  mid-update snapshots in the corpus without weakening the gate for clean data.
- **Structural (strict, zero tolerance):**
  1. `up + down == 1_000_000_000` (integer-exact, no f64).
  2. `compute_price == up` (integer-exact).
  3. `up(K_i) ≥ up(K_{i+1})` along ascending strike grid, and `up ∈ [0, 1e9]`.

## 3. Math (from architecture spec §3.2 L100–113)

All on-chain values 1e9-scaled; `rho`/`m` are `i64` sign-magnitude `{magnitude, is_negative}`.
Decode to real-valued f64 at the I/O boundary, do all math in f64.

- Log-moneyness: `k = ln(K / F)`, `F = forward` (from `OraclePricesUpdated`, no Pyth).
- Total variance: `w(k) = a + b·(ρ·(k−m) + √((k−m)² + σ²))`.
- IV: `IV = √(w / T)` (no Black–Scholes inversion — SVI *is* the IV param).
- Digital up price: `up(K) = N(d₂)`, `d₂ = (−k − ½w)/√w`, clamped to `[0, 1]`.
  `up = P(S_T ≥ K)` ⇒ monotone non-increasing in K (invariant #3).
- `normal_cdf(x) = 0.5 · erfc(−x/√2)` via libm.

### T (time-to-expiry) — NOT on the numeric path
`d₂` uses only total variance `w` (confirmed §0), so **T is not needed to reproduce
`up`/`compute_price`**. No fixture re-capture required. T is only needed for the IV
surface (`IV=√(w/T)`), which is a separate display deliverable (§8 out of scope here).
If/when computed: `T = (expiry_ms − prices.timestamp_ms)/MS_PER_YEAR` using the oracle's
stored timestamp (not wall-clock; `compute_price` takes no Clock, `binary_price_pair`'s
Clock is a staleness guard only).

## 4. Project structure (seeds the indexer)

Cargo **workspace** at repo root; first member is the `pricing` lib crate.

```
pricing/
├─ Cargo.toml              # [workspace] members = ["crates/pricing"]
├─ crates/pricing/
│  ├─ Cargo.toml           # deps: serde, serde_json, libm
│  ├─ src/
│  │  ├─ lib.rs            # re-exports
│  │  ├─ fixed.rs          # ONE=1e9; decode_i64(mag,neg); u64_to_f64; f64_to_1e9
│  │  ├─ svi.rs            # Svi{a,b,rho,m,sigma:f64}; total_variance(k); iv(w,T)
│  │  └─ digital.rs        # log_moneyness; normal_cdf; up_price
│  └─ tests/
│     └─ golden_vectors.rs # read fixtures → soft numeric + strict invariants
└─ fixtures/
   ├─ golden-binary-price.json   # REPLACE: current file is a dirty/mid-update snapshot
   │                             #   (ATM up=0.5488). Re-capture a clean oracle as the
   │                             #   single-vector golden (ATM up ≈ 0.5), or repoint the
   │                             #   test at a clean corpus oracle.
   └─ golden-corpus.json         # 19 oracles, kept as regression; 2 flagged known-anomaly
```

Existing `pricing/*.mjs` (fixture generators) are kept — they capture on-chain truth
(separate responsibility from offline recompute). The future indexer is added as a
second workspace member depending on `pricing`.

## 5. Module interfaces

```rust
// fixed.rs  (1e9 domain only — NO DUSDC 6-dec helper here; YAGNI, add at indexer/NAV stage)
pub const ONE: u64 = 1_000_000_000;
pub fn decode_i64(magnitude: u64, is_negative: bool) -> f64;  // → signed, /1e9
pub fn u64_to_f64(x_1e9: u64) -> f64;                          // → real
pub fn f64_to_1e9(x: f64) -> u64;                              // round half-up, clamp

// svi.rs
pub struct Svi { pub a: f64, pub b: f64, pub rho: f64, pub m: f64, pub sigma: f64 }
pub fn total_variance(s: &Svi, k: f64) -> f64;   // a + b(ρ(k−m)+√((k−m)²+σ²))
pub fn iv(w: f64, t_years: f64) -> f64;          // √(w/T)

// digital.rs
pub fn log_moneyness(strike_1e9: u64, forward_1e9: u64) -> f64;  // ln(K/F)
pub fn normal_cdf(x: f64) -> f64;                                // 0.5·erfc(−x/√2)
pub fn up_price(s: &Svi, k: f64, w: f64) -> f64;                 // N(d₂), clamp[0,1]
```

## 6. Error handling / edge cases (spec §7.3 degenerate inputs)

- `√w` with `w ≤ 0` (SVI keeps w>0 in theory; fuzz may breach) → NaN guard, mark the
  numeric point skipped (don't panic); invariants still checked.
- ATM `k≈0`, `sigma=0`, `b=0`, `|rho|→1`: `total_variance` is polynomial + sqrt, no
  divide-by-zero. Only `iv = √(w/T)` needs a `T ≤ 0` guard (expired oracle).
- `active=false` / settled oracles, and `T ≤ 0`: verify **structural invariants only**,
  skip numeric comparison.

## 7. Testing

- `cargo test` runs `tests/golden_vectors.rs` over both fixtures.
- Tail of the test prints `max_err`/`mean_err` (`cargo test -- --nocapture`).
- Monkey/fuzz (per project test rule + spec §9): random SVI params incl. `sigma=0`,
  `b=0`, extreme `rho` → assert no panic, only well-typed outputs / skips. (Added as a
  property test in a later step; out of scope for the first green run.)

## 8. Out of scope

- Bit-exact on-chain reproduction.
- Greeks (delta/gamma/vega/theta) — separate `pricing/` deliverable.
- The indexer itself.
- Walrus attestation hashing.
