//! Runtime-DB integration tests for the per-strike inventory writers. `#[sqlx::test]`
//! creates an isolated, migrated DB per test (requires a reachable DATABASE_URL).
//! Each is `#[ignore]`d so `cargo test --workspace` stays green offline. Run live with:
//! `cargo test -p indexer --test strike_matrix_integration -- --ignored`.

use indexer::strike_matrix::{
    insert_strike_matrix_state, replace_matrix_listing, DynField, PageLeaf, StrikeMatrixState,
};

fn state(oid: &str, ver: u64, mtm: u64) -> StrikeMatrixState {
    StrikeMatrixState {
        matrix_object_id: oid.into(),
        oracle_id: "0xoracleA".into(),
        matrix_version: ver,
        mtm,
        range_qty: 301_396_529,
        min_strike: 50_000_000_000_000,
        max_strike: 150_000_000_000_000,
        minted_min_strike: u64::MAX,
        minted_max_strike: 0,
        tick_size: 1_000_000_000,
        page_leaves: vec![
            PageLeaf { q_up: 10, q_dn: 1 },
            PageLeaf { q_up: 20, q_dn: 2 },
        ],
    }
}

#[sqlx::test]
#[ignore]
async fn insert_dedup_and_latest_view(pool: sqlx::PgPool) {
    // Two versions of the same matrix + a re-insert of v100 (no-op).
    insert_strike_matrix_state(&pool, &state("0xm1", 100, 500)).await.unwrap();
    insert_strike_matrix_state(&pool, &state("0xm1", 200, 600)).await.unwrap();
    insert_strike_matrix_state(&pool, &state("0xm1", 100, 999)).await.unwrap(); // ON CONFLICT no-op

    let rows: i64 =
        sqlx::query_scalar("SELECT count(*) FROM strike_matrix_state WHERE matrix_object_id='0xm1'")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(rows, 2, "re-inserting v100 must be a no-op");

    // Listing must contain the matrix for the view to surface it.
    replace_matrix_listing(
        &pool,
        &[DynField {
            oracle_id: "0xoracleA".into(),
            object_id: "0xm1".into(),
            version: 200,
        }],
    )
    .await
    .unwrap();

    let latest_mtm: f64 =
        sqlx::query_scalar("SELECT mtm FROM strike_matrix_latest WHERE matrix_object_id='0xm1'")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert!(
        (latest_mtm - 600.0 / 1e6).abs() < 1e-12,
        "view must show v200 mtm decoded /1e6"
    );
}

#[sqlx::test]
#[ignore]
async fn delisting_tombstone_removes_from_view(pool: sqlx::PgPool) {
    // WHY: a settled/delisted oracle drops out of getDynamicFields; the append-only
    // state row remains, but the view must stop surfacing it (listing is the tombstone).
    insert_strike_matrix_state(&pool, &state("0xm1", 100, 500)).await.unwrap();
    replace_matrix_listing(
        &pool,
        &[DynField {
            oracle_id: "0xoracleA".into(),
            object_id: "0xm1".into(),
            version: 100,
        }],
    )
    .await
    .unwrap();
    let present: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM strike_matrix_latest WHERE matrix_object_id='0xm1'",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(present, 1);

    // Next tick: matrix no longer listed -> replace with an empty set.
    replace_matrix_listing(&pool, &[]).await.unwrap();
    let after: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM strike_matrix_latest WHERE matrix_object_id='0xm1'",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(after, 0, "delisted matrix must disappear from the view");
}
