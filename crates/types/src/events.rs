//! Raw on-chain oracle event structs (as decoded from `event.parsed_json`)
//! plus conversion into the real-valued `Svi`. This is the single place where
//! chain wire-format (sign-magnitude i64, 1e9 u64 strings) becomes domain types.

use serde::Deserialize;

use crate::fixed::{decode_i64, u64_to_f64};
use crate::svi::Svi;

/// u64 fields arrive as decimal strings in parsed_json.
fn de_u64_str<'de, D: serde::Deserializer<'de>>(d: D) -> Result<u64, D::Error> {
    let s = String::deserialize(d)?;
    s.parse::<u64>().map_err(serde::de::Error::custom)
}

/// On-chain `i64::I64` sign-magnitude pair.
#[derive(Debug, Clone, Copy, Deserialize, PartialEq, Eq)]
pub struct I64Raw {
    #[serde(deserialize_with = "de_u64_str")]
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
/// NOTE: wire key (`oracle_id`) to be calibrated against live `/oracles` in Task 9.
#[derive(Debug, Clone, Deserialize)]
pub struct OracleSviUpdated {
    pub oracle_id: String,
    #[serde(deserialize_with = "de_u64_str")]
    pub a: u64,
    #[serde(deserialize_with = "de_u64_str")]
    pub b: u64,
    pub rho: I64Raw,
    pub m: I64Raw,
    #[serde(deserialize_with = "de_u64_str")]
    pub sigma: u64,
}

impl OracleSviUpdated {
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
#[derive(Debug, Clone, Deserialize)]
pub struct OraclePricesUpdated {
    pub oracle_id: String,
    #[serde(deserialize_with = "de_u64_str")]
    pub spot: u64,
    #[serde(deserialize_with = "de_u64_str")]
    pub forward: u64,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fixed::ONE;

    // Golden = architecture doc live BTC sample: rho = -0.94 (the sign-magnitude trap).
    #[test]
    fn decode_svi_negative_rho() {
        let j = serde_json::json!({
            "oracle_id": "0x1b4856",
            "a": "7116", "b": "193619",
            "rho": { "magnitude": "940000000", "is_negative": true },
            "m":   { "magnitude": "457000",    "is_negative": true },
            "sigma": "1000000"
        });
        let ev: OracleSviUpdated = serde_json::from_value(j).unwrap();
        let s = ev.to_svi();
        assert!((s.rho - (-0.94)).abs() < 1e-12, "rho was {}", s.rho);
        assert!((s.m - (-0.000457)).abs() < 1e-12, "m was {}", s.m);
        assert!((s.a - 7116.0 / ONE as f64).abs() < 1e-18);
        assert!((s.sigma - 0.001).abs() < 1e-12);
    }

    #[test]
    fn decode_prices() {
        let j = serde_json::json!({ "oracle_id": "0x1b4856", "spot": "73833860000000", "forward": "73832220000000" });
        let ev: OraclePricesUpdated = serde_json::from_value(j).unwrap();
        assert_eq!(ev.forward, 73_832_220_000_000);
    }

    // is_negative=true with magnitude 0 must decode to +0.0, not -0.0 surprises.
    #[test]
    fn decode_negative_zero() {
        let r = I64Raw { magnitude: 0, is_negative: true };
        assert_eq!(r.to_f64(), 0.0);
    }
}
