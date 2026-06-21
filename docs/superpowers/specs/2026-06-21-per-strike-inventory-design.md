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
`max_strike = 150000000000000` → 150,000 — a BTC oracle). `mtm` is **assumed** DUSDC 6-dec (mirrors
`vault.total_mtm`) — like `range_qty` and the leaf `q_up`/`q_dn` quantity scales, it is **unverified
and calibrated in live smoke**, not asserted as fact. All are stored raw, decoded only in the view
(same discipline as the unverified NAV convention: raw is the source of truth, so a wrong scale is a
view-only fix, no re-index).

**Transport / deprecation (sui-architect + sui-indexer review):** `suix_getDynamicFields` and
`sui_multiGetObjects` are JSON-RPC, which Protocol 126 schedules for **permanent deactivation on
2026-07-31** (gRPC is GA, GraphQL beta). The A-path poller already depends on JSON-RPC, so this round
is consistent — but the cliff is real and ~6 weeks out. The parser (`parse_strike_matrices`) is pure
and transport-agnostic; the migration is to swap the two fetch calls for gRPC `GetObject` /
`ListDynamicFields`, the parse layer is untouched.

## Architecture

Extend the **existing** `crates/indexer/src/bin/poller.rs` loop — one "object-state" process,
shared HTTP client / pool / shutdown. No new binary.

Each 10s tick (after the existing Predict-object poll):

1. From the Predict object already fetched this tick, read the `oracle_matrices` table id.
2. `suix_getDynamicFields(table_id, cursor, 50)`, **looping while `hasNextPage`, threading
   `nextCursor`** → the authoritative current set of `(oracle_id, objectId, listing_version)` (23
   today, one page — but the loop must not rely on the single-page happy path).
3. **Dedup before multiGet:** compare each `(objectId, listing_version)` against an in-memory
   `HashMap<matrix_object_id, version>` carried across ticks. Keep only changed matrices. Steady
   state (no trades) → 0 matrices changed → 0 multiGet, the whole matrix step costs 1 call.
4. `sui_multiGetObjects(changed_ids, {showContent})`, **chunked to the server cap (50 objects/call)**,
   → top-level scalars + inline `page_tree`.
5. Pure parse → insert changed rows. **The persisted `matrix_version` is taken from the fetched
   object's own version (multiGetObjects content), NOT the step-2 listing version** — under a
   mid-tick read skew the listing may report vA while the fetch returns vB>vA; writing the fetched
   version keeps the row's content and label consistent. Dedup-compare still uses the listing version
   (next tick re-lists vB → "changed" → re-fetch → `ON CONFLICT (matrix_object_id, vB)` no-op →
   self-healing, no dup, no gap).
6. **Reconcile the map + listing mirror to the authoritative step-2 set:** prune
   `last_matrix_versions` keys absent from this tick's listing (a settled/delisted oracle drops out of
   the table), and replace the `oracle_matrix_listing` mirror table (below) with the current set so a
   delisted matrix stops surfacing in the `_latest` view.

> **Read-skew scope (sui-architect #3):** neither `getDynamicFields`+`multiGetObjects` nor a single
> `multiGetObjects` batch is a consistent checkpoint snapshot. Per-matrix rows are independent and
> self-healing (step 5), so this is harmless — but do **not** later add a cross-matrix reconciliation
> view that hard-asserts `Σ matrix.mtm == vault.total_mtm` treating one tick as a snapshot; that
> equality can transiently break.

### Module structure

- New `crates/indexer/src/strike_matrix.rs`, mirroring `object_state.rs`:
  - pure `parse_strike_matrices(fields: &[DynamicFieldInfo], objects: &[Value]) ->
    Result<Vec<StrikeMatrixState>>` (each `StrikeMatrixState.matrix_version` = the fetched object's
    version)
  - `async insert_strike_matrix_state(pool, &StrikeMatrixState)`
  - `async replace_matrix_listing(pool, &[(matrix_object_id, oracle_id, version)])` — mirror the
    current authoritative set (delete-absent + upsert) so the view filters delisted oracles
- `poller.rs`: append the matrix step; `last_matrix_versions: HashMap<String, u64>` lives outside the
  loop next to `last_version`, pruned each tick to the authoritative listing.

### page_tree leaf extraction (live calibration)

The leaf count is **driven by the `page_tree_leaf_count` field (N), not a literal 256**: assert
`page_tree.len() == 2*N - 1` (fail loud if a matrix is ever configured with a different leaf count),
and take the N leaves from the slice the layout dictates. Which slice (last-N vs heap layout, root at
index 0) is verified in live smoke, the same way BCS field order was calibrated for the A path.

**Calibration invariant:** the segment-tree root's `total_q_up` ≡ Σ(leaf `total_q_up`) and likewise
for `total_q_dn`. Encoded as a unit test (Rule 9 — fails if the wrong slice is taken) and re-checked
in live smoke. The invariant applies **only to the `total_q_*` (sum-semantics) fields** — it must
**not** be extended to `best_prefix_up`/`best_prefix_dn`, which are prefix-extremes, not sums (a
future contributor "strengthening" the test to cover them would be wrong). Extracting the wrong slice
fails loudly rather than silently producing a wrong heatmap.

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
  page_leaves        JSONB   NOT NULL,   -- [{q_up,q_dn}] × N leaves, raw u64 strings
  ingested_at        TIMESTAMPTZ NOT NULL DEFAULT now(),
  PRIMARY KEY (matrix_object_id, matrix_version)
);

-- Authoritative current set, replaced each tick from getDynamicFields (delete-absent + upsert).
-- Lets the _latest view drop matrices whose oracle has settled/delisted (append-only state can't
-- "remove"; this mirror is the tombstone). NOT append-only — it tracks the live membership only.
CREATE TABLE IF NOT EXISTS oracle_matrix_listing (
  matrix_object_id  TEXT    NOT NULL,
  oracle_id         TEXT    NOT NULL,
  last_version      BIGINT  NOT NULL,
  last_seen_at      TIMESTAMPTZ NOT NULL DEFAULT now(),
  PRIMARY KEY (matrix_object_id)
);
```

- PK = `(matrix_object_id, matrix_version)`: re-polling an unchanged matrix is `ON CONFLICT DO
  NOTHING`; distinct matrices and distinct versions are distinct rows. Row count is capped at real
  state changes, not poll count.
- `matrix_version` as `BIGINT` (mirrors `predict_state.object_version` convention; chain versions fit
  i64 in practice, the insert guards with `i64::try_from`). The `u64::MAX` `minted_min_strike`
  sentinel sits in a `NUMERIC` column (not the `BIGINT` path), so it round-trips without overflow.
- `oracle_id` stored to join back to the A-path oracle event tables.
- Empty matrices (10/23) are stored too — they carry a version, leaves are all-zero, harmless.

View `strike_matrix_latest`: `DISTINCT ON (s.matrix_object_id) … ORDER BY s.matrix_object_id,
s.matrix_version DESC` over `strike_matrix_state s` **INNER JOIN `oracle_matrix_listing` l ON
s.matrix_object_id = l.matrix_object_id** — the join filters out delisted matrices (only currently
listed ones appear). Decode strikes/`tick_size` `/1e9`, `mtm` `/1e6` (assumed — see scale note),
leaving `range_qty` and leaf quantities raw (calibrate before exposing). Page→strike mapping
(`min_strike + page_idx * span`, `span = (max_strike - min_strike) / NULLIF(N_leaves, ...)`) is
computed in the view/frontend from the stored scalars — **guard `max_strike = min_strike`** (degenerate
or empty matrix) against divide-by-zero. The data horizon is `MIN(ingested_at)` (poll cold-start);
there is no pre-poller history — surface this so a chart does not imply the matrix was empty before
then.

## Error handling (Rule 12, mirrors existing poller)

- Transport (timeout / connection / non-200) on `getDynamicFields` or `multiGetObjects` → WARN, retry
  next tick. The Predict-object poll has already committed this tick; the matrix step is independent
  and does not roll it back or block it.
- Deterministic RPC error, parse failure (renamed/missing field, non-string u64), or a broken leaf
  calibration invariant → **fatal** (on-chain layout drift → decode is wrong).

## Testing

- Unit (pure parser, golden from real `multiGetObjects` capture): parses a non-empty matrix; asserts
  `page_tree.len() == 2*page_tree_leaf_count - 1` and extracts exactly `page_tree_leaf_count` leaves;
  the root-equals-sum-of-leaves invariant holds for `total_q_*` (and the test does **not** assert on
  `best_prefix_*`); a missing/renamed field is a loud Err; a non-string u64 is a loud Err; the
  u64::MAX `minted_min_strike` sentinel round-trips; the persisted `matrix_version` comes from the
  fetched object, not the listing.
- Pagination: a multi-page `getDynamicFields` traversal with a forced small page size yields the full
  set (guards against silently truncating at the single-page happy path once matrices exceed 50).
- Batch chunking: `multiGetObjects` is chunked at the 50-object cap (synthetic >50 changed set).
- Integration (`#[sqlx::test]`, `#[ignore]` so offline `cargo test --workspace` stays clean): insert
  + `ON CONFLICT` no-op on repeated `(matrix_object_id, matrix_version)`; the view returns one row
  per matrix at its latest version; a matrix dropped from `oracle_matrix_listing` disappears from the
  view (delisting tombstone).
- Live smoke: run the poller against testnet; confirm 23 matrices, 13 non-empty, the leaf calibration
  invariant holds live, and the version-dedup steady state issues 0 multiGet when nothing trades.
- Monkey: wrong table id → fatal, actionable; kill fullnode mid-traversal → WARN+retry, Predict state
  still committed; restart → version-map repopulates from listing, no duplicate rows; a matrix removed
  from the listing → pruned from the map and dropped from the `_latest` view (no stale "live" row).

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
