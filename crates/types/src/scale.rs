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
