//! Runtime-DB integration tests for the B-path poller sink. `#[sqlx::test]`
//! creates an isolated migrated DB per test (needs DATABASE_URL). `#[ignore]`d so
//! the offline `cargo test --workspace` stays green. Live smoke:
//! `cargo test -p indexer --test predict_state_integration -- --ignored`.

use indexer::object_state::{insert_predict_state, PredictState};

fn state(version: u64, balance: u64, mtm: u64) -> PredictState {
    PredictState {
        object_version: version,
        vault_balance: balance,
        vault_total_mtm: mtm,
        vault_total_max_payout: 3_493_960_252,
        wl_enabled: false,
        wl_available: 0,
        wl_capacity: 0,
        wl_refill_rate_per_ms: 0,
        wl_last_updated_ms: 1_776_383_327_247,
    }
}

#[sqlx::test]
#[ignore = "requires DATABASE_URL; run in live smoke with -- --ignored"]
async fn same_version_dedups(pool: sqlx::PgPool) {
    // WHY: polling an unchanged object re-reads the SAME version. The PK +
    // ON CONFLICT must absorb it, else the table grows per-poll not per-mutation.
    let s = state(100, 1_000_000_000, 5);
    insert_predict_state(&pool, &s).await.unwrap();
    insert_predict_state(&pool, &s).await.unwrap();
    let n: i64 = sqlx::query_scalar("SELECT count(*) FROM predict_state")
        .fetch_one(&pool).await.unwrap();
    assert_eq!(n, 1, "same object_version must not insert twice");
}

#[sqlx::test]
#[ignore = "requires DATABASE_URL; run in live smoke with -- --ignored"]
async fn latest_view_picks_max_version_and_decodes(pool: sqlx::PgPool) {
    // WHY: predict_latest must return the newest state and decode DUSDC /1e6.
    // balance=2_000_000 (2.0 DUSDC), mtm=1_000_000 (1.0) → nav=3.0;
    // max_payout=3_000_000 / balance=2_000_000 → utilization=1.5.
    insert_predict_state(&pool, &state(10, 9, 9)).await.unwrap();
    let mut newer = state(20, 2_000_000, 1_000_000);
    newer.vault_total_max_payout = 3_000_000;
    insert_predict_state(&pool, &newer).await.unwrap();

    let (ver, nav, util): (i64, f64, f64) =
        sqlx::query_as("SELECT object_version, nav, utilization FROM predict_latest")
            .fetch_one(&pool).await.unwrap();
    assert_eq!(ver, 20, "latest must be the max object_version");
    assert!((nav - 3.0).abs() < 1e-9, "nav decode wrong: {nav}");
    assert!((util - 1.5).abs() < 1e-9, "utilization decode wrong: {util}");
}
