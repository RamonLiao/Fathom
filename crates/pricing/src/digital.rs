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
