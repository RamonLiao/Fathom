//! Output boundary. This round only StdoutSink; next round adds PostgresSink
//! behind the same trait.

use pricing::invariants::Verdict;
use types::events::{OraclePricesUpdated, OracleSviUpdated};

/// Sanity outcome for an SVI. `Untested` is distinct from `Checked(Clean)`:
/// it means no forward for that oracle was seen yet, so the no-arb gate could
/// not run — we must NOT report such an event as "clean" (Rule 12: fail loud).
#[derive(Debug)]
pub enum SanityStatus {
    Untested,
    Checked(Verdict),
}

/// Globally-unique on-chain identity of an event: the content-addressed
/// transaction digest (base58) + its index within that tx's event list.
/// This is the Postgres dedup key (ON CONFLICT), so a re-backfill of the same
/// checkpoints inserts no duplicates.
#[derive(Debug, Clone)]
pub struct EventId {
    pub tx_digest: String,
    pub event_index: u64,
}

/// A decoded oracle event with its sanity status, ready to emit.
#[derive(Debug)]
pub enum DecodedEvent {
    Svi { ev: OracleSviUpdated, status: SanityStatus, forward_used: Option<u64> },
    Prices(OraclePricesUpdated),
}

pub trait Sink {
    /// Fallible so a sink whose downstream is a bounded channel can signal
    /// backpressure exhaustion (channel Full / writer dead) as a loud error
    /// rather than silently dropping events (Rule 12).
    fn emit(&self, id: &EventId, checkpoint_seq: u64, ev: &DecodedEvent) -> anyhow::Result<()>;
}

/// Fans each event out to every child sink, returning the first error (so a
/// failing PostgresSink takes the whole indexer down — fail loud).
pub struct TeeSink(pub Vec<Box<dyn Sink + Send + Sync>>);

impl Sink for TeeSink {
    fn emit(&self, id: &EventId, checkpoint_seq: u64, ev: &DecodedEvent) -> anyhow::Result<()> {
        for sink in &self.0 {
            sink.emit(id, checkpoint_seq, ev)?;
        }
        Ok(())
    }
}

pub struct StdoutSink;

impl Sink for StdoutSink {
    fn emit(&self, id: &EventId, checkpoint_seq: u64, ev: &DecodedEvent) -> anyhow::Result<()> {
        match ev {
            DecodedEvent::Svi { ev, status, forward_used } => {
                let svi = ev.to_svi();
                let sanity = match status {
                    SanityStatus::Untested => "untested",
                    SanityStatus::Checked(v) if v.is_clean() => "clean",
                    SanityStatus::Checked(_) => "dirty",
                };
                tracing::info!(
                    checkpoint = checkpoint_seq, tx = %id.tx_digest, ev_idx = id.event_index,
                    oracle = %ev.oracle_id,
                    a = svi.a, b = svi.b, rho = svi.rho, m = svi.m, sigma = svi.sigma,
                    forward_used = forward_used.unwrap_or(0), sanity,
                    "OracleSVIUpdated"
                );
                if let SanityStatus::Checked(Verdict::Dirty(reasons)) = status {
                    tracing::warn!(checkpoint = checkpoint_seq, ?reasons, "SVI failed no-arb sanity");
                }
            }
            DecodedEvent::Prices(p) => {
                tracing::info!(
                    checkpoint = checkpoint_seq, tx = %id.tx_digest, ev_idx = id.event_index,
                    oracle = %p.oracle_id, spot = p.spot, forward = p.forward,
                    "OraclePricesUpdated"
                );
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;

    struct CountSink(Arc<AtomicUsize>);
    impl Sink for CountSink {
        fn emit(&self, _: &EventId, _: u64, _: &DecodedEvent) -> anyhow::Result<()> {
            self.0.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }
    }
    struct ErrSink;
    impl Sink for ErrSink {
        fn emit(&self, _: &EventId, _: u64, _: &DecodedEvent) -> anyhow::Result<()> {
            Err(anyhow::anyhow!("boom"))
        }
    }

    fn sample() -> DecodedEvent {
        DecodedEvent::Prices(types::events::OraclePricesUpdated {
            oracle_id: types::events::ObjId([0; 32]),
            spot: 1,
            forward: 1,
            timestamp: 1,
        })
    }

    #[test]
    fn tee_fans_out_to_all() {
        let c = Arc::new(AtomicUsize::new(0));
        let tee = TeeSink(vec![
            Box::new(CountSink(c.clone())),
            Box::new(CountSink(c.clone())),
        ]);
        tee.emit(&EventId { tx_digest: "x".into(), event_index: 0 }, 1, &sample())
            .unwrap();
        assert_eq!(c.load(Ordering::SeqCst), 2);
    }

    #[test]
    fn tee_propagates_first_error() {
        let tee = TeeSink(vec![Box::new(ErrSink)]);
        let r = tee.emit(&EventId { tx_digest: "x".into(), event_index: 0 }, 1, &sample());
        assert!(r.is_err());
    }
}
