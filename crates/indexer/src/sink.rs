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

/// A decoded oracle event with its sanity status, ready to emit.
#[derive(Debug)]
pub enum DecodedEvent {
    Svi { ev: OracleSviUpdated, status: SanityStatus },
    Prices(OraclePricesUpdated),
}

pub trait Sink {
    fn emit(&self, checkpoint_seq: u64, ev: &DecodedEvent);
}

pub struct StdoutSink;

impl Sink for StdoutSink {
    fn emit(&self, checkpoint_seq: u64, ev: &DecodedEvent) {
        match ev {
            DecodedEvent::Svi { ev, status } => {
                let svi = ev.to_svi();
                let sanity = match status {
                    SanityStatus::Untested => "untested",
                    SanityStatus::Checked(v) if v.is_clean() => "clean",
                    SanityStatus::Checked(_) => "dirty",
                };
                tracing::info!(
                    checkpoint = checkpoint_seq, oracle = %ev.oracle_id,
                    a = svi.a, b = svi.b, rho = svi.rho, m = svi.m, sigma = svi.sigma,
                    sanity,
                    "OracleSVIUpdated"
                );
                if let SanityStatus::Checked(Verdict::Dirty(reasons)) = status {
                    tracing::warn!(checkpoint = checkpoint_seq, ?reasons, "SVI failed no-arb sanity");
                }
            }
            DecodedEvent::Prices(p) => {
                tracing::info!(
                    checkpoint = checkpoint_seq, oracle = %p.oracle_id,
                    spot = p.spot, forward = p.forward,
                    "OraclePricesUpdated"
                );
            }
        }
    }
}
