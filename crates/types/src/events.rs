//! Raw on-chain oracle event structs (as decoded from the BCS `event.contents`
//! of a checkpoint `Event`) plus conversion into the real-valued `Svi`. This is
//! the single place where chain wire-format (sign-magnitude i64, 1e9 u64,
//! 32-byte object IDs) becomes domain types.
//!
//! BCS is positional: field DECLARATION ORDER must mirror the on-chain Move
//! struct exactly (verified against `sui_getNormalizedMoveModule` for package
//! 0xf5ea2b…5138, module `oracle`, 2026-06-18):
//!   OracleSVIUpdated { oracle_id: ID, a: u64, b: u64, rho: I64, m: I64, sigma: u64, timestamp: u64 }
//!   OraclePricesUpdated { oracle_id: ID, spot: u64, forward: u64, timestamp: u64 }
//!   i64::I64 { magnitude: u64, is_negative: bool }
//! A missing/extra/reordered field makes `bcs::from_bytes` error (loud schema
//! drift) rather than silently mis-decode.

use serde::{Deserialize, Serialize};

use crate::fixed::{decode_i64, u64_to_f64};
use crate::svi::Svi;

/// On-chain `object::ID` — a single-field struct `{ bytes: address }`, which
/// BCS encodes as bare 32 bytes. A newtype over `[u8; 32]` round-trips
/// identically without pulling in a `sui-types` dependency.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Deserialize, Serialize)]
pub struct ObjId(pub [u8; 32]);

impl std::fmt::Display for ObjId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "0x")?;
        for b in self.0 {
            write!(f, "{b:02x}")?;
        }
        Ok(())
    }
}

/// On-chain `i64::I64` sign-magnitude pair (1e9-scaled). BCS: u64 then bool.
#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
pub struct I64Raw {
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
/// `oracle_id` ties the event to one of the protocol's many oracle objects;
/// without it the sanity gate cannot match an SVI to its own forward.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct OracleSviUpdated {
    pub oracle_id: ObjId,
    pub a: u64,
    pub b: u64,
    pub rho: I64Raw,
    pub m: I64Raw,
    pub sigma: u64,
    pub timestamp: u64,
}

impl OracleSviUpdated {
    /// Decode the BCS `event.contents` bytes. Errors loudly on schema drift
    /// (wrong length / field order / type).
    pub fn from_bcs(contents: &[u8]) -> Result<Self, bcs::Error> {
        bcs::from_bytes(contents)
    }

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
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct OraclePricesUpdated {
    pub oracle_id: ObjId,
    pub spot: u64,
    pub forward: u64,
    pub timestamp: u64,
}

impl OraclePricesUpdated {
    /// Decode the BCS `event.contents` bytes. Errors loudly on schema drift.
    pub fn from_bcs(contents: &[u8]) -> Result<Self, bcs::Error> {
        bcs::from_bytes(contents)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fixed::ONE;

    const OID: [u8; 32] = [0xAB; 32];

    fn le8(x: u64) -> [u8; 8] {
        x.to_le_bytes()
    }

    /// Round-trip via `bcs::to_bytes`: proves serde symmetry + that the
    /// negative-rho sign-magnitude trap decodes correctly.
    #[test]
    fn decode_svi_bcs_roundtrip_negative_rho() {
        let ev = OracleSviUpdated {
            oracle_id: ObjId(OID),
            a: 7116,
            b: 193_619,
            rho: I64Raw { magnitude: 940_000_000, is_negative: true },
            m: I64Raw { magnitude: 457_000, is_negative: true },
            sigma: 1_000_000,
            timestamp: 1_780_159_500_000,
        };
        let bytes = bcs::to_bytes(&ev).unwrap();
        let back = OracleSviUpdated::from_bcs(&bytes).unwrap();
        let s = back.to_svi();
        assert!((s.rho - (-0.94)).abs() < 1e-12, "rho was {}", s.rho);
        assert!((s.m - (-0.000457)).abs() < 1e-12, "m was {}", s.m);
        assert!((s.a - 7116.0 / ONE as f64).abs() < 1e-18);
        assert!((s.sigma - 0.001).abs() < 1e-12);
        assert_eq!(back.oracle_id, ObjId(OID));
    }

    /// Hand-built wire bytes in EXACT on-chain field order. This is the test
    /// that fails if anyone reorders the struct fields (the only thing standing
    /// between us and silent garbage, since BCS carries no field names).
    #[test]
    fn decode_svi_wire_order_handbuilt() {
        let mut w = Vec::new();
        w.extend_from_slice(&OID); // oracle_id: 32 bytes
        w.extend_from_slice(&le8(7116)); // a
        w.extend_from_slice(&le8(193_619)); // b
        w.extend_from_slice(&le8(940_000_000)); // rho.magnitude
        w.push(1); // rho.is_negative = true
        w.extend_from_slice(&le8(457_000)); // m.magnitude
        w.push(1); // m.is_negative = true
        w.extend_from_slice(&le8(1_000_000)); // sigma
        w.extend_from_slice(&le8(1_780_159_500_000)); // timestamp
        // 32 + a8 + b8 + (8+1) + (8+1) + sigma8 + ts8 = 82 bytes
        assert_eq!(w.len(), 82);

        let ev = OracleSviUpdated::from_bcs(&w).unwrap();
        assert_eq!(ev.a, 7116);
        assert_eq!(ev.b, 193_619);
        assert_eq!(ev.rho, I64Raw { magnitude: 940_000_000, is_negative: true });
        assert_eq!(ev.m, I64Raw { magnitude: 457_000, is_negative: true });
        assert_eq!(ev.sigma, 1_000_000);
        assert_eq!(ev.timestamp, 1_780_159_500_000);
        assert_eq!(ev.oracle_id, ObjId(OID));
    }

    /// Trailing bytes (e.g. an ADDED on-chain field we don't know about) must
    /// error, not silently succeed — BCS schema drift is loud (Rule 12).
    #[test]
    fn trailing_bytes_rejected() {
        let ev = OracleSviUpdated {
            oracle_id: ObjId(OID),
            a: 1,
            b: 1,
            rho: I64Raw { magnitude: 1, is_negative: false },
            m: I64Raw { magnitude: 1, is_negative: false },
            sigma: 1,
            timestamp: 1,
        };
        let mut bytes = bcs::to_bytes(&ev).unwrap();
        bytes.push(0xFF); // extra byte
        assert!(OracleSviUpdated::from_bcs(&bytes).is_err());
    }

    /// Truncated bytes (e.g. a REMOVED field, or we over-declared) must error.
    #[test]
    fn truncated_bytes_rejected() {
        let ev = OracleSviUpdated {
            oracle_id: ObjId(OID),
            a: 1,
            b: 1,
            rho: I64Raw { magnitude: 1, is_negative: false },
            m: I64Raw { magnitude: 1, is_negative: false },
            sigma: 1,
            timestamp: 1,
        };
        let mut bytes = bcs::to_bytes(&ev).unwrap();
        bytes.pop(); // drop last byte of timestamp
        assert!(OracleSviUpdated::from_bcs(&bytes).is_err());
    }

    #[test]
    fn decode_prices_roundtrip() {
        let ev = OraclePricesUpdated {
            oracle_id: ObjId(OID),
            spot: 73_833_860_000_000,
            forward: 73_832_220_000_000,
            timestamp: 42,
        };
        let bytes = bcs::to_bytes(&ev).unwrap();
        let back = OraclePricesUpdated::from_bcs(&bytes).unwrap();
        assert_eq!(back.forward, 73_832_220_000_000);
        assert_eq!(back.spot, 73_833_860_000_000);
    }

    // is_negative=true with magnitude 0 must decode to +0.0, not -0.0 surprises.
    #[test]
    fn decode_negative_zero() {
        let r = I64Raw { magnitude: 0, is_negative: true };
        assert_eq!(r.to_f64(), 0.0);
    }

    #[test]
    fn objid_display_is_hex_prefixed() {
        assert_eq!(
            ObjId([0u8; 32]).to_string(),
            "0x0000000000000000000000000000000000000000000000000000000000000000"
        );
    }
}
