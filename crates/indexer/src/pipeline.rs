//! Pure, network-free event handling: (event_name, BCS contents) → decode →
//! sanity → sink. Kept separate from the ingestion loop so it is unit-testable
//! without constructing a CheckpointEnvelope.

use std::collections::HashMap;

use anyhow::{anyhow, Result};

use pricing::invariants::check_svi_arb_free;
use types::events::{OraclePricesUpdated, OracleSviUpdated};

use crate::sink::{DecodedEvent, EventId, SanityStatus, Sink};

/// Tracks last-seen forward PER oracle (an SVI must be checked against its own
/// oracle's forward — the protocol runs many oracles concurrently) plus event
/// liveness. Keyed by the raw 32-byte object ID (no allocation / hex churn).
#[derive(Default)]
pub struct PipelineState {
    pub forward_1e9_by_oracle: HashMap<[u8; 32], u64>,
    pub oracle_events_seen: u64,
    pub first_checkpoint: Option<u64>,
    pub liveness_warned: bool,
}

/// Handle one oracle event by its Move struct name, decoding from the raw BCS
/// `event.contents`. Returns Err on DECODE failure (loud — schema drift).
/// Sanity failures are NOT errors: they are tagged on the emitted event.
pub fn handle_event(
    checkpoint_seq: u64,
    struct_name: &str,
    contents: &[u8],
    id: &EventId,
    state: &mut PipelineState,
    sink: &dyn Sink,
) -> Result<()> {
    match struct_name {
        "OracleSVIUpdated" => {
            let ev = OracleSviUpdated::from_bcs(contents)
                .map_err(|e| anyhow!("decode OracleSVIUpdated: {e}"))?;
            state.oracle_events_seen += 1;
            let forward_used = state.forward_1e9_by_oracle.get(&ev.oracle_id.0).copied();
            let status = match forward_used {
                Some(fwd) => SanityStatus::Checked(check_svi_arb_free(&ev.to_svi(), fwd)),
                None => SanityStatus::Untested,
            };
            sink.emit(id, checkpoint_seq, &DecodedEvent::Svi { ev, status, forward_used })
        }
        "OraclePricesUpdated" => {
            let ev = OraclePricesUpdated::from_bcs(contents)
                .map_err(|e| anyhow!("decode OraclePricesUpdated: {e}"))?;
            state.oracle_events_seen += 1;
            state.forward_1e9_by_oracle.insert(ev.oracle_id.0, ev.forward);
            sink.emit(id, checkpoint_seq, &DecodedEvent::Prices(ev))
        }
        // Other oracle structs (Settled, etc.) ignored this round.
        _ => Ok(()),
    }
}

/// Call once per checkpoint AFTER handling its events. Emits a single WARN if the
/// liveness window elapsed with zero oracle events seen.
pub fn check_liveness(checkpoint_seq: u64, state: &mut PipelineState) {
    let start = *state.first_checkpoint.get_or_insert(checkpoint_seq);
    if state.liveness_warned || state.oracle_events_seen > 0 {
        return;
    }
    let window = crate::config::LIVENESS_WINDOW_CHECKPOINTS;
    if checkpoint_seq.saturating_sub(start) >= window {
        tracing::warn!(
            from = start, to = checkpoint_seq,
            "no oracle events in {window} checkpoints — config drift? check PACKAGE_ID"
        );
        state.liveness_warned = true;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sink::{DecodedEvent, EventId, SanityStatus};
    use std::cell::RefCell;
    use types::events::{I64Raw, ObjId, OraclePricesUpdated, OracleSviUpdated};

    struct CaptureSink(RefCell<Vec<String>>);
    impl Sink for CaptureSink {
        fn emit(&self, _id: &EventId, _seq: u64, ev: &DecodedEvent) -> anyhow::Result<()> {
            let tag = match ev {
                DecodedEvent::Svi { status, .. } => match status {
                    SanityStatus::Untested => "svi:untested".to_string(),
                    SanityStatus::Checked(v) => format!("svi:{}", v.is_clean()),
                },
                DecodedEvent::Prices(_) => "prices".to_string(),
            };
            self.0.borrow_mut().push(tag);
            Ok(())
        }
    }

    // A capturing sink that records forward_used for SVI events.
    struct ForwardSink(RefCell<Vec<Option<u64>>>);
    impl Sink for ForwardSink {
        fn emit(&self, _id: &EventId, _seq: u64, ev: &DecodedEvent) -> anyhow::Result<()> {
            if let DecodedEvent::Svi { forward_used, .. } = ev {
                self.0.borrow_mut().push(*forward_used);
            }
            Ok(())
        }
    }

    fn eid() -> EventId { EventId { tx_digest: "test".into(), event_index: 0 } }

    // The single clean BTC-like oracle sample from the architecture doc.
    fn svi_bytes(oracle: [u8; 32]) -> Vec<u8> {
        let ev = OracleSviUpdated {
            oracle_id: ObjId(oracle),
            a: 5274,
            b: 638_806,
            rho: I64Raw { magnitude: 458_555_014, is_negative: true },
            m: I64Raw { magnitude: 1_380_256, is_negative: true },
            sigma: 1_181_366,
            timestamp: 1,
        };
        bcs::to_bytes(&ev).unwrap()
    }

    fn prices_bytes(oracle: [u8; 32], forward: u64) -> Vec<u8> {
        let ev = OraclePricesUpdated { oracle_id: ObjId(oracle), spot: forward, forward, timestamp: 1 };
        bcs::to_bytes(&ev).unwrap()
    }

    #[test]
    fn prices_then_svi_runs_sanity_clean() {
        let sink = CaptureSink(RefCell::new(vec![]));
        let mut st = PipelineState::default();
        let a = [0xAA; 32];
        handle_event(1, "OraclePricesUpdated", &prices_bytes(a, 73_744_082_479_138), &eid(), &mut st, &sink).unwrap();
        handle_event(1, "OracleSVIUpdated", &svi_bytes(a), &eid(), &mut st, &sink).unwrap();
        assert_eq!(*sink.0.borrow(), vec!["prices", "svi:true"]);
        assert_eq!(st.oracle_events_seen, 2);
    }

    // Regression for the global-forward bug: an SVI for oracle B must NOT be
    // sanity-checked against oracle A's forward. With B having no forward yet,
    // the gate must emit Untested, never borrow A's forward.
    #[test]
    fn svi_does_not_borrow_another_oracles_forward() {
        let sink = CaptureSink(RefCell::new(vec![]));
        let mut st = PipelineState::default();
        let a = [0xAA; 32];
        let b = [0xBB; 32];
        // Oracle A reports a forward 10x off from B's strikes.
        handle_event(1, "OraclePricesUpdated", &prices_bytes(a, 737_440_824_791_380), &eid(), &mut st, &sink).unwrap();
        // Oracle B's SVI arrives before B's own prices.
        handle_event(1, "OracleSVIUpdated", &svi_bytes(b), &eid(), &mut st, &sink).unwrap();
        assert_eq!(*sink.0.borrow(), vec!["prices", "svi:untested"]);
    }

    #[test]
    fn malformed_svi_is_loud_error() {
        let sink = CaptureSink(RefCell::new(vec![]));
        let mut st = PipelineState::default();
        // Truncated BCS (too few bytes for the declared struct) → decode must
        // Err, not silently produce garbage.
        let bad = svi_bytes([0xCC; 32]);
        let truncated = &bad[..bad.len() - 1];
        let r = handle_event(1, "OracleSVIUpdated", truncated, &eid(), &mut st, &sink);
        assert!(r.is_err(), "malformed event must error loudly");
        assert!(sink.0.borrow().is_empty(), "nothing emitted on decode failure");
    }

    #[test]
    fn unknown_struct_ignored() {
        let sink = CaptureSink(RefCell::new(vec![]));
        let mut st = PipelineState::default();
        handle_event(1, "OracleSettled", &[], &eid(), &mut st, &sink).unwrap();
        assert!(sink.0.borrow().is_empty());
    }

    #[test]
    fn svi_carries_the_forward_it_was_checked_against() {
        let sink = ForwardSink(RefCell::new(vec![]));
        let mut st = PipelineState::default();
        let a = [0xAA; 32];
        // No forward yet → Untested → forward_used None.
        handle_event(1, "OracleSVIUpdated", &svi_bytes(a), &eid(), &mut st, &sink).unwrap();
        // Forward arrives, then SVI again → forward_used Some(that forward).
        handle_event(1, "OraclePricesUpdated", &prices_bytes(a, 73_744_082_479_138), &eid(), &mut st, &sink).unwrap();
        handle_event(1, "OracleSVIUpdated", &svi_bytes(a), &eid(), &mut st, &sink).unwrap();
        assert_eq!(*sink.0.borrow(), vec![None, Some(73_744_082_479_138)]);
    }

    #[test]
    fn liveness_warns_after_window_with_no_events() {
        let mut st = PipelineState::default();
        check_liveness(1000, &mut st); // sets first_checkpoint = 1000
        assert!(!st.liveness_warned);
        check_liveness(1000 + crate::config::LIVENESS_WINDOW_CHECKPOINTS, &mut st);
        assert!(st.liveness_warned, "should warn after window with zero events");
    }

    #[test]
    fn liveness_silent_when_events_seen() {
        let mut st = PipelineState { oracle_events_seen: 1, ..Default::default() };
        check_liveness(1000, &mut st);
        check_liveness(1000 + crate::config::LIVENESS_WINDOW_CHECKPOINTS, &mut st);
        assert!(!st.liveness_warned, "must not warn once events flow");
    }
}
