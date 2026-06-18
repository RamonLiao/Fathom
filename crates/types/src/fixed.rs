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
