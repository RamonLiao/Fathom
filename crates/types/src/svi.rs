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
