//! Runtime-DB integration tests. `#[sqlx::test]` creates an isolated, migrated
//! database per test (requires DATABASE_URL → a reachable Postgres). Not part of
//! the offline gate — each is `#[ignore]`d so `cargo test --workspace` stays
//! green without a DB. Run in the live smoke with:
//! `cargo test -p indexer --test postgres_integration -- --ignored`.

use indexer::postgres::{run_writer, PostgresSink, channel};
use indexer::sink::Sink;
use indexer::sink::{DecodedEvent, EventId, SanityStatus};
use types::events::{I64Raw, ObjId, OracleSviUpdated, OraclePricesUpdated};
use pricing::invariants::Verdict;

fn svi_event(oracle: [u8; 32]) -> OracleSviUpdated {
    OracleSviUpdated {
        oracle_id: ObjId(oracle), a: 5274, b: 638_806,
        rho: I64Raw { magnitude: 458_555_014, is_negative: true },
        m: I64Raw { magnitude: 1_380_256, is_negative: true },
        sigma: 1_181_366, timestamp: 1,
    }
}

// Drive a list of (EventId, DecodedEvent) through a PostgresSink + writer to the pool.
async fn ingest(pool: sqlx::PgPool, items: Vec<(EventId, u64, DecodedEvent)>) {
    let (tx, rx) = channel();
    let writer = tokio::spawn(run_writer(rx, pool));
    let sink = PostgresSink::new(tx);
    for (id, seq, ev) in &items {
        sink.emit(id, *seq, ev).unwrap();
    }
    drop(sink); // close channel → writer drains and returns
    writer.await.unwrap().unwrap();
}

#[sqlx::test]
#[ignore = "requires DATABASE_URL; run in live smoke with -- --ignored"]
async fn reinserting_same_event_id_is_idempotent(pool: sqlx::PgPool) {
    // WHY: startup re-backfills from tip-N, so the SAME tx re-appears. Its digest
    // is content-addressed → identical (tx_digest, event_index) → ON CONFLICT must
    // dedup. This test fails if the key or conflict clause is wrong.
    let id = EventId { tx_digest: "AbC".into(), event_index: 0 };
    let de = || DecodedEvent::Svi { ev: svi_event([0xAA; 32]), status: SanityStatus::Untested, forward_used: None };
    ingest(pool.clone(), vec![(id.clone(), 10, de()), (id.clone(), 10, de())]).await;
    let n: i64 = sqlx::query_scalar("SELECT count(*) FROM svi_update").fetch_one(&pool).await.unwrap();
    assert_eq!(n, 1, "duplicate (tx_digest,event_index) must not insert twice");
}

#[sqlx::test]
#[ignore = "requires DATABASE_URL; run in live smoke with -- --ignored"]
async fn sanity_is_reproducible_regardless_of_replay_order(pool: sqlx::PgPool) {
    // WHY: the stored verdict is a pure function of (raw SVI, sanity_forward).
    // Two ingests of the SAME svi event with the SAME forward_used must yield the
    // same row, independent of when prices arrived in the stream.
    let id = EventId { tx_digest: "RpL".into(), event_index: 1 };
    let de = DecodedEvent::Svi {
        ev: svi_event([0xBB; 32]),
        status: SanityStatus::Checked(Verdict::Clean),
        forward_used: Some(73_744_082_479_138),
    };
    ingest(pool.clone(), vec![(id, 5, de)]).await;
    let (sanity, fwd): (String, sqlx::types::BigDecimal) =
        sqlx::query_as("SELECT sanity, sanity_forward FROM svi_update WHERE tx_digest='RpL'")
            .fetch_one(&pool).await.unwrap();
    assert_eq!(sanity, "clean");
    assert_eq!(fwd.to_string(), "73744082479138");
}

#[sqlx::test]
#[ignore = "requires DATABASE_URL; run in live smoke with -- --ignored"]
async fn oracle_latest_view_returns_most_recent_per_oracle(pool: sqlx::PgPool) {
    let o = [0xCC; 32];
    let mk = |idx: u64, seq: u64| (
        EventId { tx_digest: format!("d{idx}"), event_index: idx },
        seq,
        DecodedEvent::Prices(OraclePricesUpdated { oracle_id: ObjId(o), spot: seq, forward: seq, timestamp: seq }),
    );
    ingest(pool.clone(), vec![mk(0, 100), mk(1, 200)]).await; // seq 200 is newer
    let spot: f64 = sqlx::query_scalar("SELECT spot FROM oracle_latest WHERE oracle_id=$1")
        .bind(ObjId(o).to_string()).fetch_one(&pool).await.unwrap();
    assert!((spot - 200.0 / 1e9).abs() < 1e-18, "view must surface the latest checkpoint");
}
