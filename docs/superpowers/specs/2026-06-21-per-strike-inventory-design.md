# B-path Per-Strike Inventory — Design Spec

_Date: 2026-06-21 · Status: approved, ready for plan_

## Goal

Extend the B-path object poller to index **per-oracle exposure** and a **page-bucket inventory
heatmap** from the `Predict` object's `vault.oracle_matrices: Table<ID, StrikeMatrix>`. This is the
deferred half of the object-poller round (NAV/utilization/withdrawal shipped in
`2026-06-21-object-poller-design.md`).

**This round (Tier 1 + page_tree):** the 23 `StrikeMatrix` top-level scalars (per-oracle exposure) +
the inline `page_tree` leaves (page-bucket heatmap). **Deferred (Tier 2, GTM):** the full per-strike
`StrikeNode` heatmap nested in `pages: Table<u64, vector<StrikeNode>>` — spec'd in §7, interface
pre-wired, not implemented.

## On-chain facts (live-verified 2026-06-21, testnet)

`vault.oracle_matrices` is a `Table<0x2::object::ID, …::strike_matrix::StrikeMatrix>`.

- Table object id (the dynamic-field parent): read from the Predict object at
  `content.fields.vault.fields.oracle_matrices.fields.id.id` (currently
  `0xfd0630aeb8c0e78f1d630be39b3f2035509037b6ffbaffe345906dfeff60e69e`). **Not hardcoded** — it is
  itself a child of Predict and may change; the poller reads it from the Predict object each tick.
- `suix_getDynamicFields` over that table returns 23 entries. Each `DynamicFieldInfo` carries
  `name.value` (= the oracle_id key), `objectId` (the `StrikeMatrix` child object), and `version`.
- The 23 `StrikeMatrix` children have **independent object versions** that bump on trades against
  that oracle. Mutating a dynamic-field child does **not** bump the parent `Predict` version — so the
  existing `predict_state` dedup (PK = Predict `object_version`) does not cover them. Per-matrix dedup
  needs its own key.
- 13/23 matrices are non-empty; empties carry `minted_min_strike = 18446744073709551615` (u64::MAX
  sentinel = "nothing minted").

### `StrikeMatrix` ABI (`sui_getNormalizedMoveStruct`, all fields `U64`)

```
StrikeMatrix {
  pages:                Table<u64, vector<StrikeNode>>   ← Tier 2 (deferred), see §7
  page_tree:            vector<PageSummary>              ← INLINE, 511 entries (segment tree)
  page_tree_leaf_count: u64    (= 256)
  tick_size:            u64    (1e9 scale)
  min_strike:           u64    (1e9 scale)
  max_strike:           u64    (1e9 scale)
  minted_min_strike:    u64    (u64::MAX = none)
  minted_max_strike:    u64
  mtm:                  u64    (DUSDC 6-dec, same as vault.total_mtm)
  range_qty:            u64
}

PageSummary { total_q_up: u64, total_q_dn: u64, best_prefix_up: u64, best_prefix_dn: u64 }
StrikeNode  { q_up, q_dn, agg_q_up, agg_qk_up, agg_q_dn, agg_qk_dn : u64 }   ← Tier 2
```

`page_tree` is a complete binary segment tree over 256 leaves: 256 leaves + 255 internal = **511
entries**. The 256 leaves are the page-bucket heatmap (`total_q_up`/`total_q_dn` per page); the 255
internal nodes hold prefix sums for on-chain query and are **discarded**.

`showContent` encodes every `u64` as a **decimal string**.

**Scale note:** strike fields are 1e9-scaled (`min_strike = 50000000000000` → 50,000;
`max_strike = 150000000000000` → 150,000 — a BTC oracle). `mtm` is DUSDC 6-dec (mirrors
`vault.total_mtm`). `range_qty` and the leaf `q_up`/`q_dn` quantity scales are **unverified** —
stored raw, decoded only in the view, **calibrated in live smoke** (same discipline as the
unverified NAV convention: raw is the source of truth, so a wrong scale is a view-only fix, no
re-index).

## Architecture

Extend the **existing** `crates/indexer/src/bin/poller.rs` loop — one "object-state" process,
shared HTTP client / pool / shutdown. No new binary.

Each 10s tick (after the existing Predict-object poll):

1. From the Predict object already fetched this tick, read the `oracle_matrices` table id.
2. `suix_getDynamicFields(table_id, cursor, 50)`, paginating to completion → `(oracle_id, objectId,
   version)` × 23. **One call** at the current size.
3. **Dedup before multiGet:** compare each `(objectId, version)` against an in-memory
   `HashMap<matrix_object_id, version>` carried across ticks. Keep only changed matrices. Steady
   state (no trades) → 0 matrices changed → 0 multiGet, the whole matrix step costs 1 call.
4. `sui_multiGetObjects(changed_ids, {showContent})` → top-level scalars + inline `page_tree`.
5. Pure parse → insert changed rows → update the version map.

### Module structure

- New `crates/indexer/src/strike_matrix.rs`, mirroring `object_state.rs`:
  - pure `parse_strike_matrices(fields: &[DynamicFieldInfo], objects: &[Value]) ->
    Result<Vec<StrikeMatrixState>>`
  - `async insert_strike_matrix_state(pool, &StrikeMatrixState)`
- `poller.rs`: append the matrix step; `last_matrix_versions: HashMap<String, u64>` lives outside the
  loop next to `last_version`.

### page_tree leaf extraction (live calibration)

The leaf slice within the 511-entry array (last-256 vs heap layout) is verified in live smoke, the
same way BCS field order was calibrated for the A path. **Calibration invariant:** the segment-tree
root's `total_q_up` ≡ Σ(leaf `total_q_up`) and likewise for `total_q_dn`. This is encoded as a unit
test (Rule 9 — it fails if the wrong slice is taken) and re-checked in live smoke. Extracting the
wrong slice fails loudly rather than silently producing a wrong heatmap.

## Schema (`migrations/0003_strike_matrix_state.sql`)

Append-only; raw chain integers as `NUMERIC` (source of truth); all decoding lives in the view.

```sql
CREATE TABLE IF NOT EXISTS strike_matrix_state (
  matrix_object_id   TEXT    NOT NULL,
  oracle_id          TEXT    NOT NULL,
  matrix_version     BIGINT  NOT NULL,
  mtm                NUMERIC NOT NULL,
  range_qty          NUMERIC NOT NULL,
  min_strike         NUMERIC NOT NULL,
  max_strike         NUMERIC NOT NULL,
  minted_min_strike  NUMERIC NOT NULL,
  minted_max_strike  NUMERIC NOT NULL,
  tick_size          NUMERIC NOT NULL,
  page_leaves        JSONB   NOT NULL,   -- [{q_up,q_dn}] × 256, raw u64 strings
  ingested_at        TIMESTAMPTZ NOT NULL DEFAULT now(),
  PRIMARY KEY (matrix_object_id, matrix_version)
);
```

- PK = `(matrix_object_id, matrix_version)`: re-polling an unchanged matrix is `ON CONFLICT DO
  NOTHING`; distinct matrices and distinct versions are distinct rows. Row count is capped at real
  state changes, not poll count.
- `matrix_version` as `BIGINT` (mirrors `predict_state.object_version` convention; chain versions fit
  i64 in practice, the insert guards with `i64::try_from`).
- `oracle_id` stored to join back to the A-path oracle event tables.
- Empty matrices (10/23) are stored too — they carry a version, leaves are all-zero, harmless.

View `strike_matrix_latest`: `DISTINCT ON (matrix_object_id) … ORDER BY matrix_object_id,
matrix_version DESC`, decoding strikes/`tick_size` `/1e9`, `mtm` `/1e6`, leaving `range_qty` and leaf
quantities raw (scale unverified — calibrate before exposing). Page→strike mapping
(`min_strike + page_idx * span`, `span = (max_strike - min_strike) / 256`) is computed in the view or
frontend from the stored scalars, not precomputed.

## Error handling (Rule 12, mirrors existing poller)

- Transport (timeout / connection / non-200) on `getDynamicFields` or `multiGetObjects` → WARN, retry
  next tick. The Predict-object poll has already committed this tick; the matrix step is independent
  and does not roll it back or block it.
- Deterministic RPC error, parse failure (renamed/missing field, non-string u64), or a broken leaf
  calibration invariant → **fatal** (on-chain layout drift → decode is wrong).

## Testing

- Unit (pure parser, golden from real `multiGetObjects` capture): parses a non-empty matrix; extracts
  exactly 256 leaves; the root-equals-sum-of-leaves invariant holds; a missing/renamed field is a
  loud Err; a non-string u64 is a loud Err; the u64::MAX `minted_min_strike` sentinel round-trips.
- Integration (`#[sqlx::test]`, `#[ignore]` so offline `cargo test --workspace` stays clean): insert
  + `ON CONFLICT` no-op on repeated `(matrix_object_id, matrix_version)`; the view returns one row
  per matrix at its latest version.
- Live smoke: run the poller against testnet; confirm 23 matrices, 13 non-empty, the leaf calibration
  invariant holds live, and the version-dedup steady state issues 0 multiGet when nothing trades.
- Monkey: wrong table id → fatal, actionable; kill fullnode mid-traversal → WARN+retry, Predict state
  still committed; restart → version-map repopulates, no duplicate rows.

## Tier 2 (GTM, deferred — spec + interface only)

Full per-strike heatmap lives in each `StrikeMatrix`'s `pages: Table<u64, vector<StrikeNode>>`. A
later round adds:

- Table `strike_node_state(matrix_object_id, matrix_version, strike_idx, q_up, q_dn, agg_q_up,
  agg_qk_up, agg_q_dn, agg_qk_dn, …)`, PK `(matrix_object_id, matrix_version, strike_idx)`.
- Pure `parse_strike_pages(...)` alongside `parse_strike_matrices`.
- Poller loop: after Tier 1 step 5, for each changed matrix, a nested
  `getDynamicFields(pages_table_id) + multiGetObjects` traversal (this is the order-of-magnitude cost
  — hundreds of calls per changed matrix — hence deferred).

**Interface seam (built this round so Tier 2 is additive):** `StrikeMatrixState` carries the matrix's
own `matrix_object_id` and version; Tier 2 fetches `pages` table id from the already-fetched matrix
object, so Tier 1's schema, parser signature, and the poller's existing steps **do not change** when
Tier 2 is added. Tier 2 dedups on the same `(matrix_object_id, matrix_version)` already tracked.
