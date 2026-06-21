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

#[sqlx::test]
#[ignore = "requires DATABASE_URL; run in live smoke with -- --ignored"]
async fn zero_balance_utilization_is_null_not_divide_by_zero(pool: sqlx::PgPool) {
    // WHY: a drained vault (balance=0) must not blow up the view. NULLIF guards the
    // division → utilization is NULL, not an error or +Inf. testnet's RateLimiter
    // can legitimately sit at capacity=0, and an empty vault is a real boundary.
    insert_predict_state(&pool, &state(7, 0, 0)).await.unwrap();
    let util: Option<f64> =
        sqlx::query_scalar("SELECT utilization FROM predict_latest")
            .fetch_one(&pool).await.unwrap();
    assert_eq!(util, None, "zero balance must yield NULL utilization, not divide-by-zero");
}

#[sqlx::test]
#[ignore = "requires DATABASE_URL; run in live smoke with -- --ignored"]
async fn enabled_limiter_exposes_decoded_withdrawal(pool: sqlx::PgPool) {
    // WHY: the view's CASE has two branches. testnet runs wl_enabled=false (→ NULL,
    // covered elsewhere); this pins the OTHER branch — when the limiter is enabled,
    // withdrawal_available must decode available/1e6 (DUSDC), not stay NULL.
    let mut s = state(30, 5_000_000, 0);
    s.wl_enabled = true;
    s.wl_available = 2_500_000; // 2.5 DUSDC
    insert_predict_state(&pool, &s).await.unwrap();
    let w: Option<f64> =
        sqlx::query_scalar("SELECT withdrawal_available FROM predict_latest")
            .fetch_one(&pool).await.unwrap();
    assert!(w.is_some(), "enabled limiter must expose a value, not NULL");
    assert!((w.unwrap() - 2.5).abs() < 1e-9, "withdrawal decode wrong: {w:?}");
}
