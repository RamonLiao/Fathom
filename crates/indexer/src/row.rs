//! Pure mapping: a decoded event + its identity → an insertable Postgres row.
//! Numeric chain values become decimal STRINGS (bound as `$n::numeric`) so this
//! module owns the only signed-decode path (rho/m) — see plan Global Constraints.

use types::events::I64Raw;
use crate::sink::{DecodedEvent, EventId, SanityStatus};
use pricing::invariants::Verdict;

pub struct PricesRow {
    pub tx_digest: String,
    pub event_index: i64,
    pub checkpoint_seq: i64,
    pub oracle_id: String,
    pub spot: String,
    pub forward: String,
    pub ts_chain_ms: String,
}

pub struct SviRow {
    pub tx_digest: String,
    pub event_index: i64,
    pub checkpoint_seq: i64,
    pub oracle_id: String,
    pub a: String,
    pub b: String,
    pub sigma: String,
    pub rho: String,
    pub m: String,
    pub ts_chain_ms: String,
    pub sanity_forward: Option<String>,
    pub sanity: String,
    pub sanity_reasons: Option<Vec<String>>,
}

pub enum Row {
    Prices(PricesRow),
    Svi(SviRow),
}

pub fn to_row(id: &EventId, checkpoint_seq: u64, ev: &DecodedEvent) -> Row {
    match ev {
        DecodedEvent::Prices(p) => Row::Prices(PricesRow {
            tx_digest: id.tx_digest.clone(),
            event_index: id.event_index as i64,
            checkpoint_seq: checkpoint_seq as i64,
            oracle_id: p.oracle_id.to_string(),
            spot: p.spot.to_string(),
            forward: p.forward.to_string(),
            ts_chain_ms: p.timestamp.to_string(),
        }),
        DecodedEvent::Svi { ev, status, forward_used } => {
            let (sanity, sanity_reasons) = match status {
                SanityStatus::Untested => ("untested", None),
                SanityStatus::Checked(Verdict::Clean) => ("clean", None),
                SanityStatus::Checked(Verdict::Dirty(reasons)) => ("dirty", Some(reasons.clone())),
            };
            Row::Svi(SviRow {
                tx_digest: id.tx_digest.clone(),
                event_index: id.event_index as i64,
                checkpoint_seq: checkpoint_seq as i64,
                oracle_id: ev.oracle_id.to_string(),
                a: ev.a.to_string(),
                b: ev.b.to_string(),
                sigma: ev.sigma.to_string(),
                rho: signed_str(ev.rho),
                m: signed_str(ev.m),
                ts_chain_ms: ev.timestamp.to_string(),
                sanity_forward: forward_used.map(|f| f.to_string()),
                sanity: sanity.to_string(),
                sanity_reasons,
            })
        }
    }
}

/// Sign-magnitude i64 → signed decimal string. `-0` (magnitude 0) → "0".
fn signed_str(r: I64Raw) -> String {
    if r.is_negative && r.magnitude != 0 {
        format!("-{}", r.magnitude)
    } else {
        r.magnitude.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use types::events::{ObjId, OraclePricesUpdated, OracleSviUpdated};

    fn eid() -> EventId { EventId { tx_digest: "Dx".into(), event_index: 3 } }

    #[test]
    fn negative_rho_stays_negative() {
        let ev = OracleSviUpdated {
            oracle_id: ObjId([0xAA; 32]),
            a: 5274, b: 638_806,
            rho: I64Raw { magnitude: 458_555_014, is_negative: true },
            m: I64Raw { magnitude: 1_380_256, is_negative: true },
            sigma: 1_181_366, timestamp: 7,
        };
        let de = DecodedEvent::Svi {
            ev, status: SanityStatus::Checked(Verdict::Clean), forward_used: Some(73_744_082_479_138),
        };
        let Row::Svi(r) = to_row(&eid(), 99, &de) else { panic!("expected Svi") };
        assert_eq!(r.rho, "-458555014");          // sign preserved (the bug pricing gate guards)
        assert_eq!(r.m, "-1380256");
        assert_eq!(r.a, "5274");
        assert_eq!(r.oracle_id, ObjId([0xAA; 32]).to_string()); // 0x… hex
        assert_eq!(r.event_index, 3);
        assert_eq!(r.checkpoint_seq, 99);
        assert_eq!(r.sanity, "clean");
        assert_eq!(r.sanity_forward.as_deref(), Some("73744082479138"));
        assert!(r.sanity_reasons.is_none());
    }

    #[test]
    fn negative_zero_is_plain_zero() {
        assert_eq!(signed_str(I64Raw { magnitude: 0, is_negative: true }), "0");
    }

    #[test]
    fn untested_has_no_forward_and_no_reasons() {
        let ev = OracleSviUpdated {
            oracle_id: ObjId([1; 32]), a: 1, b: 1,
            rho: I64Raw { magnitude: 1, is_negative: false },
            m: I64Raw { magnitude: 1, is_negative: false },
            sigma: 1, timestamp: 1,
        };
        let de = DecodedEvent::Svi { ev, status: SanityStatus::Untested, forward_used: None };
        let Row::Svi(r) = to_row(&eid(), 1, &de) else { panic!() };
        assert_eq!(r.sanity, "untested");
        assert!(r.sanity_forward.is_none());
        assert!(r.sanity_reasons.is_none());
    }

    #[test]
    fn dirty_carries_reasons() {
        let ev = OracleSviUpdated {
            oracle_id: ObjId([2; 32]), a: 1, b: 1,
            rho: I64Raw { magnitude: 1, is_negative: false },
            m: I64Raw { magnitude: 1, is_negative: false },
            sigma: 1, timestamp: 1,
        };
        let de = DecodedEvent::Svi {
            ev, status: SanityStatus::Checked(Verdict::Dirty(vec!["boom".into()])), forward_used: Some(5),
        };
        let Row::Svi(r) = to_row(&eid(), 1, &de) else { panic!() };
        assert_eq!(r.sanity, "dirty");
        assert_eq!(r.sanity_reasons.as_deref(), Some(&["boom".to_string()][..]));
    }

    #[test]
    fn prices_row_maps_u64_to_decimal_strings() {
        let p = OraclePricesUpdated {
            oracle_id: ObjId([3; 32]), spot: 73_833_860_000_000, forward: 73_832_220_000_000, timestamp: 42,
        };
        let Row::Prices(r) = to_row(&eid(), 7, &DecodedEvent::Prices(p)) else { panic!() };
        assert_eq!(r.spot, "73833860000000");
        assert_eq!(r.forward, "73832220000000");
        assert_eq!(r.ts_chain_ms, "42");
    }
}
