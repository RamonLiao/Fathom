//! Parse golden-corpus.json and decide which oracles are clean vs quarantined.

use serde::Deserialize;

use crate::fixed::ONE;
use crate::svi::Svi;

/// ATM quarantine threshold (R3): an oracle whose ATM (pct==0) `up` deviates from
/// 0.5e9 by more than this is a dirty/mid-SVI-update snapshot, excluded from the
/// numeric gate. Provenance: 2/19 corpus oracles captured mid-update have ATM up of
/// 0.463 and 0.549; clean oracles sit within ~5e-4 of 0.5. 0.03e9 separates them with
/// wide margin. Documented in spec §0 / §2.
pub const ATM_QUARANTINE_ABS_1E9: i64 = 30_000_000; // 0.03 * 1e9

/// Oracles quarantined by explicit ID: dirty/desynced snapshots the ATM-deviation
/// proxy cannot detect. 0x2709d76a's on-chain prices are internally inconsistent with
/// its own SVI params (all reproduction errors negative, steep low-sigma smile),
/// peaking at 1.36e-3 at ATM while its ATM-dev is only +0.99M (far under threshold).
/// Excluded from the SOFT numeric gate only; structural invariants still apply.
const QUARANTINED_BY_ID: &[&str] = &[
    "0x2709d76a", // prefix of 0x2709d76a224ea11f32a889b93a85a1502d8e8af81ea707351b0f57b81a0137f4
];

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
        if QUARANTINED_BY_ID.iter().any(|id| self.oracle_id.starts_with(id)) {
            return true;
        }
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
    use crate::fixed::u64_to_f64;

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
