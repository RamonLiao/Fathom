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
