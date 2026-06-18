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
