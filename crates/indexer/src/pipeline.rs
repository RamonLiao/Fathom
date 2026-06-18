//! Pure, network-free event handling: (event_name, parsed_json) → decode →
//! sanity → sink. Kept separate from the Processor so it is unit-testable
//! without constructing a CheckpointEnvelope.

use std::collections::HashMap;

use anyhow::{anyhow, Result};
use serde_json::Value;

use pricing::invariants::check_svi_arb_free;
use types::events::{OraclePricesUpdated, OracleSviUpdated};

use crate::sink::{DecodedEvent, SanityStatus, Sink};

/// Tracks last-seen forward PER oracle (an SVI must be checked against its own
/// oracle's forward — the protocol runs many oracles concurrently) plus event
/// liveness.
#[derive(Default)]
pub struct PipelineState {
    pub forward_1e9_by_oracle: HashMap<String, u64>,
    pub oracle_events_seen: u64,
    pub first_checkpoint: Option<u64>,
    pub liveness_warned: bool,
}

/// Handle one oracle event by its Move struct name. Returns Err on DECODE
/// failure (loud — schema drift). Sanity failures are NOT errors: they are
/// tagged on the emitted event.
pub fn handle_event(
    checkpoint_seq: u64,
    struct_name: &str,
    parsed_json: &Value,
    state: &mut PipelineState,
    sink: &dyn Sink,
) -> Result<()> {
    match struct_name {
        "OracleSVIUpdated" => {
            let ev: OracleSviUpdated = serde_json::from_value(parsed_json.clone())
                .map_err(|e| anyhow!("decode OracleSVIUpdated: {e}"))?;
            state.oracle_events_seen += 1;
            let status = match state.forward_1e9_by_oracle.get(&ev.oracle_id) {
                Some(&fwd) => SanityStatus::Checked(check_svi_arb_free(&ev.to_svi(), fwd)),
                // No forward seen yet for THIS oracle → cannot run the curve check.
                // Report Untested (NOT clean) rather than borrow another oracle's forward.
                None => SanityStatus::Untested,
            };
            sink.emit(checkpoint_seq, &DecodedEvent::Svi { ev, status });
            Ok(())
        }
        "OraclePricesUpdated" => {
            let ev: OraclePricesUpdated = serde_json::from_value(parsed_json.clone())
                .map_err(|e| anyhow!("decode OraclePricesUpdated: {e}"))?;
            state.oracle_events_seen += 1;
            state
                .forward_1e9_by_oracle
                .insert(ev.oracle_id.clone(), ev.forward);
            sink.emit(checkpoint_seq, &DecodedEvent::Prices(ev));
            Ok(())
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
    use crate::sink::{DecodedEvent, SanityStatus};
    use std::cell::RefCell;

    struct CaptureSink(RefCell<Vec<String>>);
    impl Sink for CaptureSink {
        fn emit(&self, _seq: u64, ev: &DecodedEvent) {
            let tag = match ev {
                DecodedEvent::Svi { status, .. } => match status {
                    SanityStatus::Untested => "svi:untested".to_string(),
                    SanityStatus::Checked(v) => format!("svi:{}", v.is_clean()),
                },
                DecodedEvent::Prices(_) => "prices".to_string(),
            };
            self.0.borrow_mut().push(tag);
        }
    }

    fn svi_json(oracle_id: &str) -> Value {
        serde_json::json!({
            "oracle_id": oracle_id,
            "a": "5274", "b": "638806",
            "rho": { "magnitude": "458555014", "is_negative": true },
            "m":   { "magnitude": "1380256",   "is_negative": true },
            "sigma": "1181366"
        })
    }

    fn prices_json(oracle_id: &str, forward: &str) -> Value {
        serde_json::json!({ "oracle_id": oracle_id, "spot": forward, "forward": forward })
    }

    #[test]
    fn prices_then_svi_runs_sanity_clean() {
        let sink = CaptureSink(RefCell::new(vec![]));
        let mut st = PipelineState::default();
        handle_event(1, "OraclePricesUpdated",
            &prices_json("0xA", "73744082479138"), &mut st, &sink).unwrap();
        handle_event(1, "OracleSVIUpdated", &svi_json("0xA"), &mut st, &sink).unwrap();
        assert_eq!(*sink.0.borrow(), vec!["prices", "svi:true"]);
        assert_eq!(st.oracle_events_seen, 2);
    }

    // Regression for the global-forward bug: an SVI for oracle B must NOT be
    // sanity-checked against oracle A's forward. With B having no forward yet,
    // the gate must emit clean-untested, never borrow A's forward (which here
    // would be a wildly mismatched price → spurious Dirty/garbage verdict).
    #[test]
    fn svi_does_not_borrow_another_oracles_forward() {
        let sink = CaptureSink(RefCell::new(vec![]));
        let mut st = PipelineState::default();
        // Oracle A reports a forward 100x off from B's strikes.
        handle_event(1, "OraclePricesUpdated",
            &prices_json("0xA", "737440824791380"), &mut st, &sink).unwrap();
        // Oracle B's SVI arrives before B's own prices.
        handle_event(1, "OracleSVIUpdated", &svi_json("0xB"), &mut st, &sink).unwrap();
        // B's SVI must be reported Untested (no forward for B), NOT checked vs A
        // and NOT silently "clean".
        assert_eq!(*sink.0.borrow(), vec!["prices", "svi:untested"]);
    }

    #[test]
    fn malformed_svi_is_loud_error() {
        let sink = CaptureSink(RefCell::new(vec![]));
        let mut st = PipelineState::default();
        // rho missing is_negative → decode must Err, not silently produce garbage.
        let bad = serde_json::json!({
            "a": "1", "b": "1", "rho": { "magnitude": "1" }, "m": { "magnitude": "1", "is_negative": false }, "sigma": "1"
        });
        let r = handle_event(1, "OracleSVIUpdated", &bad, &mut st, &sink);
        assert!(r.is_err(), "malformed event must error loudly");
        assert!(sink.0.borrow().is_empty(), "nothing emitted on decode failure");
    }

    #[test]
    fn unknown_struct_ignored() {
        let sink = CaptureSink(RefCell::new(vec![]));
        let mut st = PipelineState::default();
        handle_event(1, "OracleSettled", &serde_json::json!({}), &mut st, &sink).unwrap();
        assert!(sink.0.borrow().is_empty());
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
