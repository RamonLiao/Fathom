# Per-Strike Inventory (Tier 1 + page_tree) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Extend the B-path object poller to index per-oracle `StrikeMatrix` exposure scalars + the inline `page_tree` page-bucket heatmap from the `Predict` object's `vault.oracle_matrices: Table<ID, StrikeMatrix>` dynamic fields into Postgres.

**Architecture:** Each 10s poller tick (after the existing Predict-object poll) lists the `oracle_matrices` dynamic fields (`suix_getDynamicFields`, paginated), dedups against an in-memory version map, `sui_multiGetObjects{showContent}` only the changed children (chunked to 50/call), pure-parses each self-describing object into a `StrikeMatrixState` (scalars + 256 extracted page_tree leaves), and appends to `strike_matrix_state` (PK `(matrix_object_id, matrix_version)`, `ON CONFLICT DO NOTHING`). A `oracle_matrix_listing` mirror table is replaced each tick so the `_latest` view drops delisted oracles.

**Tech Stack:** Rust, reqwest (JSON-RPC, rustls), sqlx 0.8 (Postgres, runtime `query()`), serde_json, anyhow, tracing. No new dependencies.

## Global Constraints

- **Zero new dependencies** — reqwest/serde_json/sqlx/anyhow/tracing already in `crates/indexer/Cargo.toml`.
- **No sui-sdk** — raw JSON-RPC over reqwest, same as the existing poller.
- **Store raw chain integers** as `NUMERIC` (source of truth); all decoding (`/1e9` strikes, `/1e6` mtm) lives only in the view. A wrong scale is a view-only fix, never a re-index.
- **u64 fields arrive as decimal STRINGS** in `showContent`; bind to Postgres as `String` + `$n::numeric` (no decimal crate). `matrix_version` is the only `BIGINT`, guarded by `i64::try_from`.
- **Fail loud (Rule 12):** transport errors → WARN + retry next tick; deterministic RPC error / parse failure / broken leaf invariant → fatal (on-chain layout drift).
- **Tests encode WHY (Rule 9):** the root≡Σleaves invariant test fails if the wrong leaf slice is taken; the missing-field test fails if a package upgrade silently changes layout.
- **Migrations auto-apply:** `connect_pool` runs `sqlx::migrate!("./migrations")`; a new `0003_*.sql` is picked up automatically.
- **JSON-RPC EOL 2026-07-31** (Protocol 126): the parser is transport-agnostic; only the fetch fns change at migration.

### On-chain facts (live-verified 2026-06-21, testnet)

- `oracle_matrices` Table object id: read from the Predict object at `content.fields.vault.fields.oracle_matrices.fields.id.id` (currently `0xfd0630aeb8c0e78f1d630be39b3f2035509037b6ffbaffe345906dfeff60e69e`). **Not hardcoded.**
- 23 dynamic fields; each `getDynamicFields` entry: `name.value` = oracle_id, `objectId` = StrikeMatrix child, `version`.
- Each child object (from `multiGetObjects{showContent}`) is **self-describing**: `data.objectId`, `data.version`, `content.fields.name` (= oracle_id string), `content.fields.value.fields` (= the `StrikeMatrix`).
- `StrikeMatrix` fields, all U64 decimal-strings: `mtm`, `range_qty`, `min_strike`, `max_strike`, `minted_min_strike` (u64::MAX = "none"), `minted_max_strike`, `tick_size`, `page_tree_leaf_count` (= 256), `page_tree` (vector of 511 `PageSummary`), `pages` (Table — Tier 2, ignored this round).
- `PageSummary` fields: `total_q_up`, `total_q_dn`, `best_prefix_up`, `best_prefix_dn` (all U64).
- **Leaf slice = the last N entries of `page_tree`** (`page_tree[N-1 ..= 2N-2]`, N = `page_tree_leaf_count`). 0-indexed binary heap: root at index 0 (internal nodes 0..N-2), leaves N-1..2N-2. **Live-confirmed:** matrix `0x7baeeaff6d4be1029666253746a597e1e8dbc6756241004d8f99c584f80ad04a`, root `total_q_up` = 1643938631 = Σ(last-256 leaves' `total_q_up`).
- Live counts: 23 matrices, 13 non-empty. Empty → `minted_min_strike = 18446744073709551615`, leaves all-zero.

---

## File Structure

- **Create** `crates/indexer/src/strike_matrix.rs` — pure parsers (`StrikeMatrixState`, `PageLeaf`, `DynField`, `extract_leaves`, `parse_strike_matrix`, `parse_strike_matrices`, `parse_dynamic_fields_page`, `parse_oracle_matrices_table_id`, `chunk_ids`) + DB writers (`insert_strike_matrix_state`, `replace_matrix_listing`).
- **Create** `crates/indexer/migrations/0003_strike_matrix_state.sql` — `strike_matrix_state` table + `oracle_matrix_listing` table + `strike_matrix_latest` view.
- **Create** `crates/indexer/tests/strike_matrix_integration.rs` — `#[sqlx::test] #[ignore]` DB tests.
- **Modify** `crates/indexer/src/lib.rs` — add `pub mod strike_matrix;`.
- **Modify** `crates/indexer/src/bin/poller.rs` — add the matrix step to the loop + fetch helpers.

---

## Task 1: Pure parser module `strike_matrix.rs`

**Files:**
- Create: `crates/indexer/src/strike_matrix.rs`
- Modify: `crates/indexer/src/lib.rs` (add `pub mod strike_matrix;`)

**Interfaces:**
- Consumes: nothing (leaf module).
- Produces:
  - `pub struct PageLeaf { pub q_up: u64, pub q_dn: u64 }`
  - `pub struct StrikeMatrixState { pub matrix_object_id: String, pub oracle_id: String, pub matrix_version: u64, pub mtm: u64, pub range_qty: u64, pub min_strike: u64, pub max_strike: u64, pub minted_min_strike: u64, pub minted_max_strike: u64, pub tick_size: u64, pub page_leaves: Vec<PageLeaf> }`
  - `pub struct DynField { pub oracle_id: String, pub object_id: String, pub version: u64 }`
  - `pub fn extract_leaves(page_tree: &serde_json::Value, leaf_count: usize) -> anyhow::Result<Vec<PageLeaf>>`
  - `pub fn parse_strike_matrix(obj_data: &serde_json::Value) -> anyhow::Result<StrikeMatrixState>`
  - `pub fn parse_strike_matrices(objects: &[serde_json::Value]) -> anyhow::Result<Vec<StrikeMatrixState>>`
  - `pub fn parse_dynamic_fields_page(resp: &serde_json::Value) -> anyhow::Result<(Vec<DynField>, Option<String>)>`
  - `pub fn parse_oracle_matrices_table_id(predict_data: &serde_json::Value) -> anyhow::Result<String>`
  - `pub fn chunk_ids(ids: &[String], size: usize) -> Vec<&[String]>`

- [ ] **Step 1: Register the module**

In `crates/indexer/src/lib.rs`, add after the existing `pub mod object_state;` line:

```rust
pub mod strike_matrix;
```

- [ ] **Step 2: Write the module skeleton with the u64-string helper (mirrors object_state.rs)**

Create `crates/indexer/src/strike_matrix.rs`:

```rust
//! Pure parsers for the `Predict.vault.oracle_matrices` dynamic-field children
//! (`StrikeMatrix`). Each child is read via `sui_multiGetObjects{showContent}` and
//! is self-describing (`data.objectId`/`data.version`/`content.fields.name` =
//! oracle_id / `content.fields.value.fields` = the StrikeMatrix). u64 fields arrive
//! as decimal STRINGS. Any missing/renamed field or a broken page_tree invariant is
//! a loud Err (on-chain layout drift → decode is wrong → fatal).

use anyhow::{bail, Context, Result};
use serde_json::Value;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PageLeaf {
    pub q_up: u64,
    pub q_dn: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StrikeMatrixState {
    pub matrix_object_id: String,
    pub oracle_id: String,
    pub matrix_version: u64,
    pub mtm: u64,
    pub range_qty: u64,
    pub min_strike: u64,
    pub max_strike: u64,
    pub minted_min_strike: u64,
    pub minted_max_strike: u64,
    pub tick_size: u64,
    pub page_leaves: Vec<PageLeaf>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DynField {
    pub oracle_id: String,
    pub object_id: String,
    pub version: u64,
}

/// Read a decimal-string u64 field, loud on missing/non-string/unparseable.
fn u64_field(obj: &Value, key: &str) -> Result<u64> {
    let s = obj
        .get(key)
        .and_then(Value::as_str)
        .with_context(|| format!("missing or non-string u64 field `{key}`"))?;
    s.parse::<u64>()
        .with_context(|| format!("parse u64 field `{key}` from {s:?}"))
}
```

- [ ] **Step 3: Write the failing test for `extract_leaves` (synthetic small tree)**

Append to `crates/indexer/src/strike_matrix.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    // A complete binary heap over N leaves has 2N-1 nodes: internal nodes
    // [0..N-1), leaves [N-1..2N-1). Build one with N=4 (7 nodes). Leaves carry
    // real q_up/q_dn; the root (index 0) must equal the SUM of the 4 leaves.
    fn page_tree_n4() -> Value {
        let node = |up: u64, dn: u64| {
            serde_json::json!({ "fields": {
                "total_q_up": up.to_string(), "total_q_dn": dn.to_string(),
                "best_prefix_up": "0", "best_prefix_dn": "0"
            }})
        };
        // leaves: (10,1) (20,2) (30,3) (40,4) → root totals (100,10)
        serde_json::json!([
            node(100, 10),                 // 0 root
            node(30, 3), node(70, 7),      // 1,2 internal
            node(10, 1), node(20, 2),      // 3,4 leaves[0],[1]
            node(30, 3), node(40, 4),      // 5,6 leaves[2],[3]
        ])
    }

    #[test]
    fn extract_leaves_takes_last_n_and_checks_root_sum() {
        let leaves = extract_leaves(&page_tree_n4(), 4).unwrap();
        assert_eq!(leaves.len(), 4);
        assert_eq!(leaves[0], PageLeaf { q_up: 10, q_dn: 1 });
        assert_eq!(leaves[3], PageLeaf { q_up: 40, q_dn: 4 });
    }

    #[test]
    fn extract_leaves_rejects_wrong_node_count() {
        // WHY: len must be exactly 2N-1; a different leaf_count means we mis-slice.
        let mut pt = page_tree_n4();
        pt.as_array_mut().unwrap().pop();
        assert!(extract_leaves(&pt, 4).is_err());
    }

    #[test]
    fn extract_leaves_rejects_broken_sum_invariant() {
        // WHY: root != Σleaves means we took the wrong slice → loud, not a wrong heatmap.
        let mut pt = page_tree_n4();
        pt[0]["fields"]["total_q_up"] = serde_json::json!("999");
        assert!(extract_leaves(&pt, 4).is_err());
    }
}
```

- [ ] **Step 4: Run the tests to verify they fail**

Run: `cargo test -p indexer --lib strike_matrix::tests::extract_leaves 2>&1 | tail -20`
Expected: FAIL — `cannot find function extract_leaves`.

- [ ] **Step 5: Implement `extract_leaves`**

Add to `crates/indexer/src/strike_matrix.rs` (above the `#[cfg(test)]` mod):

```rust
/// Extract the N leaf `PageLeaf`s from the inline `page_tree` segment tree.
/// `page_tree` is a complete binary heap of `2*leaf_count - 1` `PageSummary` nodes
/// (root at index 0, leaves last). We keep only the leaves' `total_q_up`/`total_q_dn`.
/// Verifies the sum invariant `root.total_q_* == Σ leaf.total_q_*` (sum-semantics
/// fields only — `best_prefix_*` are prefix-extremes and are intentionally ignored).
pub fn extract_leaves(page_tree: &Value, leaf_count: usize) -> Result<Vec<PageLeaf>> {
    let nodes = page_tree
        .as_array()
        .context("page_tree is not an array")?;
    let expected = 2 * leaf_count - 1;
    if nodes.len() != expected {
        bail!(
            "page_tree length {} != 2*leaf_count-1 ({}) — layout drift",
            nodes.len(),
            expected
        );
    }
    let summary = |n: &Value| -> Result<(u64, u64)> {
        let f = n.get("fields").context("page node missing fields")?;
        Ok((u64_field(f, "total_q_up")?, u64_field(f, "total_q_dn")?))
    };
    let (root_up, root_dn) = summary(&nodes[0])?;
    let mut leaves = Vec::with_capacity(leaf_count);
    let (mut sum_up, mut sum_dn) = (0u128, 0u128);
    for n in &nodes[leaf_count - 1..] {
        let (up, dn) = summary(n)?;
        sum_up += up as u128;
        sum_dn += dn as u128;
        leaves.push(PageLeaf { q_up: up, q_dn: dn });
    }
    if sum_up != root_up as u128 || sum_dn != root_dn as u128 {
        bail!(
            "page_tree root != Σleaves (up {root_up} vs {sum_up}, dn {root_dn} vs {sum_dn}) — wrong leaf slice"
        );
    }
    Ok(leaves)
}
```

- [ ] **Step 6: Run the tests to verify they pass**

Run: `cargo test -p indexer --lib strike_matrix::tests::extract_leaves 2>&1 | tail -20`
Expected: PASS (3 tests).

- [ ] **Step 7: Write the failing test for `parse_strike_matrix` (self-describing object golden)**

Add inside the `tests` mod:

```rust
    // Real multiGetObjects element shape (result[i]), trimmed page_tree to N=2.
    fn matrix_obj() -> Value {
        let node = |up: u64, dn: u64| serde_json::json!({ "fields": {
            "total_q_up": up.to_string(), "total_q_dn": dn.to_string(),
            "best_prefix_up": "0", "best_prefix_dn": "0" }});
        serde_json::json!({
            "objectId": "0x1104586103fce6a5dfcdcd767c4303dfbb280aaed5f45d0fa51c6d5cc2fc5646",
            "version": "910924146",
            "type": "0x2::dynamic_field::Field<0x2::object::ID, ...::strike_matrix::StrikeMatrix>",
            "content": { "dataType": "moveObject", "fields": {
                "id": { "id": "0x1104586103fce6a5dfcdcd767c4303dfbb280aaed5f45d0fa51c6d5cc2fc5646" },
                "name": "0xed2cc924940c74b0eed46f174e2cecf5dee602ef1f4246b8d2acc35c31af3159",
                "value": { "type": "...::strike_matrix::StrikeMatrix", "fields": {
                    "mtm": "615919646",
                    "range_qty": "301396529",
                    "min_strike": "50000000000000",
                    "max_strike": "150000000000000",
                    "minted_min_strike": "18446744073709551615",
                    "minted_max_strike": "0",
                    "tick_size": "1000000000",
                    "page_tree_leaf_count": "2",
                    "page_tree": [ node(30, 3), node(10, 1), node(20, 2) ],
                    "pages": { "type": "0x2::table::Table<u64, vector<...>>", "fields": {
                        "id": { "id": "0xabc" }, "size": "2" }}
                }}
            }}
        })
    }

    #[test]
    fn parses_self_describing_matrix() {
        let s = parse_strike_matrix(&matrix_obj()).unwrap();
        assert_eq!(s.matrix_object_id, "0x1104586103fce6a5dfcdcd767c4303dfbb280aaed5f45d0fa51c6d5cc2fc5646");
        assert_eq!(s.oracle_id, "0xed2cc924940c74b0eed46f174e2cecf5dee602ef1f4246b8d2acc35c31af3159");
        assert_eq!(s.matrix_version, 910_924_146);
        assert_eq!(s.mtm, 615_919_646);
        assert_eq!(s.range_qty, 301_396_529);
        assert_eq!(s.min_strike, 50_000_000_000_000);
        assert_eq!(s.minted_min_strike, u64::MAX); // sentinel round-trips
        assert_eq!(s.tick_size, 1_000_000_000);
        assert_eq!(s.page_leaves, vec![
            PageLeaf { q_up: 10, q_dn: 1 }, PageLeaf { q_up: 20, q_dn: 2 }]);
    }

    #[test]
    fn missing_field_is_loud() {
        // WHY: a package upgrade dropping/renaming a field must fail, not silently mis-decode.
        let mut v = matrix_obj();
        v["content"]["fields"]["value"]["fields"].as_object_mut().unwrap().remove("mtm");
        let err = parse_strike_matrix(&v).unwrap_err().to_string();
        assert!(err.contains("mtm"), "error must name the missing field: {err}");
    }

    #[test]
    fn non_string_u64_is_loud() {
        let mut v = matrix_obj();
        v["content"]["fields"]["value"]["fields"]["mtm"] = serde_json::json!(615919646u64);
        assert!(parse_strike_matrix(&v).is_err());
    }
```

- [ ] **Step 8: Run to verify failure**

Run: `cargo test -p indexer --lib strike_matrix::tests::parses_self 2>&1 | tail -20`
Expected: FAIL — `cannot find function parse_strike_matrix`.

- [ ] **Step 9: Implement `parse_strike_matrix` and `parse_strike_matrices`**

Add above the test mod:

```rust
/// Parse one `multiGetObjects` `result[i].data` element (a self-describing
/// `Field<ID, StrikeMatrix>` dynamic-field child). The persisted version is the
/// FETCHED object's `data.version` (not any listing version) — under a mid-tick
/// read skew this keeps the row's content and label consistent.
pub fn parse_strike_matrix(obj_data: &Value) -> Result<StrikeMatrixState> {
    let matrix_object_id = obj_data
        .get("objectId")
        .and_then(Value::as_str)
        .context("matrix object missing objectId")?
        .to_string();
    let matrix_version = u64_field(obj_data, "version").context("matrix object version")?;
    let fields = obj_data
        .pointer("/content/fields")
        .context("missing content.fields (object has no parsed content)")?;
    let oracle_id = fields
        .get("name")
        .and_then(Value::as_str)
        .context("dynamic-field `name` (oracle_id) missing or non-string")?
        .to_string();
    let m = fields
        .pointer("/value/fields")
        .context("missing value.fields (StrikeMatrix)")?;
    let leaf_count = u64_field(m, "page_tree_leaf_count")? as usize;
    let page_tree = m.get("page_tree").context("missing page_tree")?;
    let page_leaves = extract_leaves(page_tree, leaf_count)
        .with_context(|| format!("extract leaves for matrix {matrix_object_id}"))?;
    Ok(StrikeMatrixState {
        matrix_object_id,
        oracle_id,
        matrix_version,
        mtm: u64_field(m, "mtm")?,
        range_qty: u64_field(m, "range_qty")?,
        min_strike: u64_field(m, "min_strike")?,
        max_strike: u64_field(m, "max_strike")?,
        minted_min_strike: u64_field(m, "minted_min_strike")?,
        minted_max_strike: u64_field(m, "minted_max_strike")?,
        tick_size: u64_field(m, "tick_size")?,
        page_leaves,
    })
}

/// Parse the `result` array of a `sui_multiGetObjects{showContent}` response.
/// Each element is `{ data: { ... } }`; a per-element `error` (deleted/notExists)
/// is a loud Err (we only ever fetch ids we just listed → a missing one is drift).
pub fn parse_strike_matrices(objects: &[Value]) -> Result<Vec<StrikeMatrixState>> {
    objects
        .iter()
        .map(|o| {
            let data = o
                .get("data")
                .with_context(|| match o.get("error") {
                    Some(e) => format!("multiGetObjects element error: {e}"),
                    None => "multiGetObjects element missing data".to_string(),
                })?;
            parse_strike_matrix(data)
        })
        .collect()
}
```

- [ ] **Step 10: Run to verify pass**

Run: `cargo test -p indexer --lib strike_matrix 2>&1 | tail -20`
Expected: PASS (all strike_matrix tests).

- [ ] **Step 11: Write failing tests for `parse_dynamic_fields_page`, `parse_oracle_matrices_table_id`, `chunk_ids`**

Add inside the test mod:

```rust
    fn getdf_page(has_next: bool) -> Value {
        serde_json::json!({
            "data": [
                { "name": { "type": "0x2::object::ID", "value": "0xoracleA" },
                  "objectType": "...::strike_matrix::StrikeMatrix",
                  "objectId": "0xmatrixA", "version": 910924146u64 },
                { "name": { "type": "0x2::object::ID", "value": "0xoracleB" },
                  "objectId": "0xmatrixB", "version": 910956162u64 }
            ],
            "nextCursor": if has_next { serde_json::json!("0xcursor1") } else { Value::Null },
            "hasNextPage": has_next
        })
    }

    #[test]
    fn parses_dynamic_fields_page_with_cursor() {
        // WHY: hasNextPage must surface the cursor so the loop does not truncate at page 1.
        let (items, next) = parse_dynamic_fields_page(&getdf_page(true)).unwrap();
        assert_eq!(items.len(), 2);
        assert_eq!(items[0], DynField {
            oracle_id: "0xoracleA".into(), object_id: "0xmatrixA".into(), version: 910_924_146 });
        assert_eq!(next, Some("0xcursor1".to_string()));
    }

    #[test]
    fn parses_dynamic_fields_last_page() {
        let (_items, next) = parse_dynamic_fields_page(&getdf_page(false)).unwrap();
        assert_eq!(next, None);
    }

    #[test]
    fn extracts_oracle_matrices_table_id() {
        let predict = serde_json::json!({ "content": { "fields": { "vault": { "fields": {
            "oracle_matrices": { "type": "0x2::table::Table<...>",
                "fields": { "id": { "id": "0xfd0630" }, "size": "23" } } } } } } });
        assert_eq!(parse_oracle_matrices_table_id(&predict).unwrap(), "0xfd0630");
    }

    #[test]
    fn chunk_ids_respects_cap() {
        // WHY: sui_multiGetObjects caps at 50 objects/call; >50 changed must be chunked.
        let ids: Vec<String> = (0..130).map(|i| format!("0x{i}")).collect();
        let chunks = chunk_ids(&ids, 50);
        assert_eq!(chunks.len(), 3);
        assert_eq!(chunks[0].len(), 50);
        assert_eq!(chunks[2].len(), 30);
    }
```

- [ ] **Step 12: Run to verify failure**

Run: `cargo test -p indexer --lib strike_matrix 2>&1 | tail -20`
Expected: FAIL — `cannot find function parse_dynamic_fields_page` (and the others).

- [ ] **Step 13: Implement the three helpers**

Add above the test mod:

```rust
/// Parse one `suix_getDynamicFields` page → (items, next_cursor). A `None` cursor
/// or `hasNextPage == false` ends pagination. `version` is a JSON number here
/// (unlike the string-encoded u64s inside showContent).
pub fn parse_dynamic_fields_page(resp: &Value) -> Result<(Vec<DynField>, Option<String>)> {
    let data = resp.get("data").and_then(Value::as_array).context("getDynamicFields missing data")?;
    let mut items = Vec::with_capacity(data.len());
    for e in data {
        let oracle_id = e
            .pointer("/name/value")
            .and_then(Value::as_str)
            .context("dynamic field name.value (oracle_id) missing")?
            .to_string();
        let object_id = e.get("objectId").and_then(Value::as_str).context("objectId missing")?.to_string();
        let version = e.get("version").and_then(Value::as_u64).context("version missing or non-numeric")?;
        items.push(DynField { oracle_id, object_id, version });
    }
    let has_next = resp.get("hasNextPage").and_then(Value::as_bool).unwrap_or(false);
    let next = if has_next {
        Some(resp.get("nextCursor").and_then(Value::as_str).context("hasNextPage but no nextCursor")?.to_string())
    } else {
        None
    };
    Ok((items, next))
}

/// Read the `oracle_matrices` Table object id from a Predict `getObject` data blob.
pub fn parse_oracle_matrices_table_id(predict_data: &Value) -> Result<String> {
    predict_data
        .pointer("/content/fields/vault/fields/oracle_matrices/fields/id/id")
        .and_then(Value::as_str)
        .context("missing vault.oracle_matrices.id.id in Predict object")
        .map(str::to_string)
}

/// Split ids into chunks of at most `size` (the multiGetObjects server cap).
pub fn chunk_ids(ids: &[String], size: usize) -> Vec<&[String]> {
    ids.chunks(size).collect()
}
```

- [ ] **Step 14: Run all module tests + clippy**

Run: `cargo test -p indexer --lib strike_matrix 2>&1 | tail -20`
Expected: PASS (all ~10 tests).
Run: `cargo clippy -p indexer --lib --all-targets -- -D warnings 2>&1 | tail -10`
Expected: clean.

- [ ] **Step 15: Commit**

```bash
git add crates/indexer/src/strike_matrix.rs crates/indexer/src/lib.rs
git commit -m "feat(poller): pure StrikeMatrix parsers (scalars + page_tree leaves)"
```

---

## Task 2: Schema migration + DB writers

**Files:**
- Create: `crates/indexer/migrations/0003_strike_matrix_state.sql`
- Modify: `crates/indexer/src/strike_matrix.rs` (add `insert_strike_matrix_state`, `replace_matrix_listing`)
- Test: `crates/indexer/tests/strike_matrix_integration.rs`

**Interfaces:**
- Consumes: `StrikeMatrixState`, `PageLeaf`, `DynField` (Task 1).
- Produces:
  - `pub async fn insert_strike_matrix_state(pool: &sqlx::PgPool, s: &StrikeMatrixState) -> anyhow::Result<()>`
  - `pub async fn replace_matrix_listing(pool: &sqlx::PgPool, listing: &[DynField]) -> anyhow::Result<()>`

- [ ] **Step 1: Write the migration**

Create `crates/indexer/migrations/0003_strike_matrix_state.sql`:

```sql
-- B-path per-strike inventory: per-oracle StrikeMatrix exposure + page-bucket
-- heatmap from Predict.vault.oracle_matrices dynamic fields. Raw chain integers as
-- NUMERIC (source of truth); decoding (1e9 strikes, 1e6 mtm) lives only in the view.
-- Dedup key = (matrix_object_id, matrix_version): a child object's version bumps on
-- every mutation (independent of the parent Predict version), so re-polling an
-- unchanged matrix is a no-op and row count is capped at distinct states.

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
  page_leaves        JSONB   NOT NULL,   -- [{"q_up":"..","q_dn":".."}] × N, raw u64 strings
  ingested_at        TIMESTAMPTZ NOT NULL DEFAULT now(),
  PRIMARY KEY (matrix_object_id, matrix_version)
);

-- Authoritative current membership, REPLACED each tick from getDynamicFields.
-- NOT append-only: it is the tombstone that lets the _latest view drop matrices
-- whose oracle has settled/delisted (append-only state cannot "remove").
CREATE TABLE IF NOT EXISTS oracle_matrix_listing (
  matrix_object_id  TEXT    NOT NULL,
  oracle_id         TEXT    NOT NULL,
  last_version      BIGINT  NOT NULL,
  last_seen_at      TIMESTAMPTZ NOT NULL DEFAULT now(),
  PRIMARY KEY (matrix_object_id)
);

-- Latest state per CURRENTLY-LISTED matrix. INNER JOIN drops delisted oracles.
-- Scales: strikes/tick_size /1e9; mtm /1e6 (ASSUMED — calibrate in live smoke).
-- range_qty and leaf quantities left raw (scale unverified). page->strike mapping
-- is left to the frontend from the raw scalars; span guards max==min (div-by-zero).
CREATE OR REPLACE VIEW strike_matrix_latest AS
SELECT DISTINCT ON (s.matrix_object_id)
  s.matrix_object_id,
  s.oracle_id,
  s.matrix_version,
  s.mtm::float8        / 1e6 AS mtm,
  s.range_qty,                                   -- raw (scale unverified)
  s.min_strike::float8 / 1e9 AS min_strike,
  s.max_strike::float8 / 1e9 AS max_strike,
  s.tick_size::float8  / 1e9 AS tick_size,
  CASE WHEN s.minted_min_strike = 18446744073709551615 THEN NULL
       ELSE s.minted_min_strike::float8 / 1e9 END AS minted_min_strike,
  CASE WHEN s.minted_min_strike = 18446744073709551615 THEN NULL
       ELSE s.minted_max_strike::float8 / 1e9 END AS minted_max_strike,
  s.page_leaves,                                 -- raw (scale unverified)
  s.ingested_at
FROM strike_matrix_state s
JOIN oracle_matrix_listing l ON l.matrix_object_id = s.matrix_object_id
ORDER BY s.matrix_object_id, s.matrix_version DESC;
```

- [ ] **Step 2: Write the failing integration tests**

Create `crates/indexer/tests/strike_matrix_integration.rs`:

```rust
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
        page_leaves: vec![PageLeaf { q_up: 10, q_dn: 1 }, PageLeaf { q_up: 20, q_dn: 2 }],
    }
}

#[sqlx::test]
#[ignore]
async fn insert_dedup_and_latest_view(pool: sqlx::PgPool) {
    // Two versions of the same matrix + a re-insert of v1 (no-op).
    insert_strike_matrix_state(&pool, &state("0xm1", 100, 500)).await.unwrap();
    insert_strike_matrix_state(&pool, &state("0xm1", 200, 600)).await.unwrap();
    insert_strike_matrix_state(&pool, &state("0xm1", 100, 999)).await.unwrap(); // ON CONFLICT no-op

    let rows: i64 = sqlx::query_scalar("SELECT count(*) FROM strike_matrix_state WHERE matrix_object_id='0xm1'")
        .fetch_one(&pool).await.unwrap();
    assert_eq!(rows, 2, "re-inserting v100 must be a no-op");

    // Listing must contain the matrix for the view to surface it.
    replace_matrix_listing(&pool, &[DynField {
        oracle_id: "0xoracleA".into(), object_id: "0xm1".into(), version: 200 }]).await.unwrap();

    let latest_mtm: f64 = sqlx::query_scalar("SELECT mtm FROM strike_matrix_latest WHERE matrix_object_id='0xm1'")
        .fetch_one(&pool).await.unwrap();
    assert!((latest_mtm - 600.0 / 1e6).abs() < 1e-12, "view must show v200 mtm decoded /1e6");
}

#[sqlx::test]
#[ignore]
async fn delisting_tombstone_removes_from_view(pool: sqlx::PgPool) {
    // WHY: a settled/delisted oracle drops out of getDynamicFields; the append-only
    // state row remains, but the view must stop surfacing it (listing is the tombstone).
    insert_strike_matrix_state(&pool, &state("0xm1", 100, 500)).await.unwrap();
    replace_matrix_listing(&pool, &[DynField {
        oracle_id: "0xoracleA".into(), object_id: "0xm1".into(), version: 100 }]).await.unwrap();
    let present: i64 = sqlx::query_scalar("SELECT count(*) FROM strike_matrix_latest WHERE matrix_object_id='0xm1'")
        .fetch_one(&pool).await.unwrap();
    assert_eq!(present, 1);

    // Next tick: matrix no longer listed → replace with an empty set.
    replace_matrix_listing(&pool, &[]).await.unwrap();
    let after: i64 = sqlx::query_scalar("SELECT count(*) FROM strike_matrix_latest WHERE matrix_object_id='0xm1'")
        .fetch_one(&pool).await.unwrap();
    assert_eq!(after, 0, "delisted matrix must disappear from the view");
}
```

- [ ] **Step 3: Run to verify failure (compile error — fns not defined)**

Run: `cargo test -p indexer --test strike_matrix_integration --no-run 2>&1 | tail -20`
Expected: FAIL — `cannot find function insert_strike_matrix_state` / `replace_matrix_listing`.

- [ ] **Step 4: Implement the two DB writers**

Add to `crates/indexer/src/strike_matrix.rs` (above the test mod). Add `use serde_json::json;` is not needed — build the JSONB with `serde_json::Value`:

```rust
/// Idempotent insert: a repeated `(matrix_object_id, matrix_version)` is a no-op.
/// Numerics bound as String + `$n::numeric` (no decimal crate); page_leaves bound as
/// JSONB with raw u64s as strings (source of truth; decode lives in the view).
pub async fn insert_strike_matrix_state(pool: &sqlx::PgPool, s: &StrikeMatrixState) -> Result<()> {
    let leaves = serde_json::Value::Array(
        s.page_leaves
            .iter()
            .map(|l| serde_json::json!({ "q_up": l.q_up.to_string(), "q_dn": l.q_dn.to_string() }))
            .collect(),
    );
    sqlx::query(
        "INSERT INTO strike_matrix_state \
         (matrix_object_id,oracle_id,matrix_version,mtm,range_qty,min_strike,max_strike,\
          minted_min_strike,minted_max_strike,tick_size,page_leaves) \
         VALUES ($1,$2,$3,$4::numeric,$5::numeric,$6::numeric,$7::numeric,$8::numeric,$9::numeric,$10::numeric,$11) \
         ON CONFLICT (matrix_object_id,matrix_version) DO NOTHING",
    )
    .bind(&s.matrix_object_id)
    .bind(&s.oracle_id)
    .bind(i64::try_from(s.matrix_version).context("matrix_version exceeds i64::MAX")?)
    .bind(s.mtm.to_string())
    .bind(s.range_qty.to_string())
    .bind(s.min_strike.to_string())
    .bind(s.max_strike.to_string())
    .bind(s.minted_min_strike.to_string())
    .bind(s.minted_max_strike.to_string())
    .bind(s.tick_size.to_string())
    .bind(leaves)
    .execute(pool)
    .await
    .context("insert strike_matrix_state")?;
    Ok(())
}

/// Replace the authoritative current membership in one transaction: clear the
/// table, then insert the current set. Small (≤ a few dozen rows); the view INNER
/// JOINs this so delisted matrices vanish.
pub async fn replace_matrix_listing(pool: &sqlx::PgPool, listing: &[DynField]) -> Result<()> {
    let mut tx = pool.begin().await.context("begin listing tx")?;
    sqlx::query("DELETE FROM oracle_matrix_listing")
        .execute(&mut *tx)
        .await
        .context("clear oracle_matrix_listing")?;
    for d in listing {
        sqlx::query(
            "INSERT INTO oracle_matrix_listing (matrix_object_id,oracle_id,last_version) VALUES ($1,$2,$3)",
        )
        .bind(&d.object_id)
        .bind(&d.oracle_id)
        .bind(i64::try_from(d.version).context("listing version exceeds i64::MAX")?)
        .execute(&mut *tx)
        .await
        .context("insert oracle_matrix_listing")?;
    }
    tx.commit().await.context("commit listing tx")?;
    Ok(())
}
```

- [ ] **Step 5: Verify compile + offline gate stays clean**

Run: `cargo test -p indexer --test strike_matrix_integration --no-run 2>&1 | tail -10`
Expected: compiles.
Run: `DATABASE_URL= cargo test -p indexer --test strike_matrix_integration 2>&1 | tail -10`
Expected: `2 ignored` (no DB needed offline).

- [ ] **Step 6: Run the live DB tests (requires Postgres)**

Run: `cargo test -p indexer --test strike_matrix_integration -- --ignored 2>&1 | tail -20`
Expected: PASS (2 tests). (If no local Postgres, defer to Task 4 live smoke and note it.)

- [ ] **Step 7: Clippy + commit**

Run: `cargo clippy -p indexer --all-targets -- -D warnings 2>&1 | tail -10`
Expected: clean.

```bash
git add crates/indexer/migrations/0003_strike_matrix_state.sql crates/indexer/src/strike_matrix.rs crates/indexer/tests/strike_matrix_integration.rs
git commit -m "feat(poller): strike_matrix_state schema + insert/listing writers + view"
```

---

## Task 3: Wire the matrix step into the poller loop

**Files:**
- Modify: `crates/indexer/src/bin/poller.rs`

**Interfaces:**
- Consumes: `parse_oracle_matrices_table_id`, `parse_dynamic_fields_page`, `parse_strike_matrices`, `chunk_ids`, `insert_strike_matrix_state`, `replace_matrix_listing`, `DynField` (Tasks 1–2); existing `parse_predict_state`/`insert_predict_state` and the `fetch_object` JSON-RPC pattern.
- Produces: a self-contained poller binary; no new public API.

> **Note on testing this task:** the loop is network glue over already-unit-tested pure functions. Its correctness is verified by Task 4's live smoke + Monkey tests (the loop cannot be meaningfully unit-tested without a live fullnode). Steps here are compile + clippy gated; behavioural verification is Task 4.

- [ ] **Step 1: Add imports and the cross-tick version map**

In `crates/indexer/src/bin/poller.rs`, extend the `use indexer::...` imports:

```rust
use indexer::strike_matrix::{
    chunk_ids, insert_strike_matrix_state, parse_dynamic_fields_page,
    parse_oracle_matrices_table_id, parse_strike_matrices, replace_matrix_listing, DynField,
};
use std::collections::HashMap;
```

Just before the `loop {` (next to `let mut last_version`), add:

```rust
    // Per-matrix version dedup, carried across ticks. Pruned each tick to the
    // authoritative getDynamicFields listing so delisted matrices don't leak.
    let mut last_matrix_versions: HashMap<String, u64> = HashMap::new();
    // Server cap for sui_multiGetObjects.
    const MULTI_GET_CAP: usize = 50;
```

- [ ] **Step 2: Capture the Predict object data for table-id extraction**

The loop currently does `let state = parse_predict_state(data)...`. The `data` JSON pointer is borrowed from `resp`. After the existing `insert_predict_state` + logging block (after `last_version = Some(...)`), the matrix step needs `data` still in scope — it is (it lives until end of loop body). Add the matrix step at the END of the loop body, after the Predict logging. Insert this block right before the closing `}` of the `loop`:

```rust
        // ---- B-path per-strike inventory (oracle_matrices dynamic fields) ----
        if let Err(e) = poll_matrices(&client, &pool, data, &mut last_matrix_versions, MULTI_GET_CAP).await {
            // Transport failures inside poll_matrices are already swallowed there
            // (WARN + early return Ok). Reaching here means a deterministic/parse
            // error — fatal (layout drift), consistent with parse_predict_state.
            return Err(e.context("poll oracle_matrices"));
        }
```

- [ ] **Step 3: Implement `poll_matrices` (paginate → dedup → chunked multiGet → parse → persist → prune)**

Add this function after `fetch_object` at the bottom of `poller.rs`:

```rust
/// Index the oracle_matrices dynamic fields for this tick. Transport failures
/// (timeout/connection/non-200) are swallowed as WARN + `Ok(())` (retry next tick);
/// the Predict state has already been committed and must not be rolled back. Parse /
/// deterministic-RPC errors propagate as `Err` → fatal (layout drift).
async fn poll_matrices(
    client: &reqwest::Client,
    pool: &sqlx::PgPool,
    predict_data: &serde_json::Value,
    last_versions: &mut std::collections::HashMap<String, u64>,
    cap: usize,
) -> Result<()> {
    let table_id = parse_oracle_matrices_table_id(predict_data)
        .context("read oracle_matrices table id")?;

    // 1. List the full authoritative set (paginate while hasNextPage).
    let mut listing: Vec<DynField> = Vec::new();
    let mut cursor: Option<String> = None;
    loop {
        let resp = match fetch_dynamic_fields(client, &table_id, cursor.as_deref()).await {
            Ok(r) => r,
            Err(e) => {
                tracing::warn!(error = %e, "getDynamicFields failed — retrying next tick");
                return Ok(());
            }
        };
        if let Some(err) = resp.pointer("/result/error") {
            anyhow::bail!("getDynamicFields error (check oracle_matrices table id): {err}");
        }
        let result = resp.pointer("/result").context("getDynamicFields missing result")?;
        let (mut items, next) = parse_dynamic_fields_page(result)?;
        listing.append(&mut items);
        match next {
            Some(c) => cursor = Some(c),
            None => break,
        }
    }

    // 2. Dedup: fetch only matrices whose listing version advanced.
    let changed: Vec<String> = listing
        .iter()
        .filter(|d| last_versions.get(&d.object_id) != Some(&d.version))
        .map(|d| d.object_id.clone())
        .collect();

    // 3. Fetch changed matrices, chunked to the server cap; parse + persist.
    for chunk in chunk_ids(&changed, cap) {
        let resp = match fetch_objects(client, chunk).await {
            Ok(r) => r,
            Err(e) => {
                tracing::warn!(error = %e, "multiGetObjects failed — retrying next tick");
                return Ok(());
            }
        };
        let objs = resp.pointer("/result").and_then(|v| v.as_array())
            .context("multiGetObjects missing result array")?;
        let states = parse_strike_matrices(objs)?;
        for s in &states {
            insert_strike_matrix_state(pool, s).await.context("persist strike_matrix_state")?;
            last_versions.insert(s.matrix_object_id.clone(), s.matrix_version);
            tracing::info!(matrix = %s.matrix_object_id, oracle = %s.oracle_id,
                version = s.matrix_version, mtm = s.mtm, "persisted strike matrix");
        }
    }

    // 4. Mirror the authoritative set (tombstone) + prune the in-memory map.
    replace_matrix_listing(pool, &listing).await.context("replace matrix listing")?;
    let live: std::collections::HashSet<&str> = listing.iter().map(|d| d.object_id.as_str()).collect();
    last_versions.retain(|k, _| live.contains(k.as_str()));
    Ok(())
}

/// `suix_getDynamicFields(table_id, cursor, 50)` raw JSON-RPC response. Transport
/// failures only are `Err` (caller retries); the caller inspects `result.error`.
async fn fetch_dynamic_fields(
    client: &reqwest::Client,
    table_id: &str,
    cursor: Option<&str>,
) -> Result<serde_json::Value> {
    let body = serde_json::json!({
        "jsonrpc": "2.0", "id": 1, "method": "suix_getDynamicFields",
        "params": [table_id, cursor, 50],
    });
    client.post(FULLNODE_URL).json(&body).send().await.context("POST getDynamicFields")?
        .error_for_status().context("fullnode error status")?
        .json().await.context("parse getDynamicFields response")
}

/// `sui_multiGetObjects(ids, {showContent})` raw JSON-RPC response.
async fn fetch_objects(client: &reqwest::Client, ids: &[String]) -> Result<serde_json::Value> {
    let body = serde_json::json!({
        "jsonrpc": "2.0", "id": 1, "method": "sui_multiGetObjects",
        "params": [ids, { "showContent": true }],
    });
    client.post(FULLNODE_URL).json(&body).send().await.context("POST multiGetObjects")?
        .error_for_status().context("fullnode error status")?
        .json().await.context("parse multiGetObjects response")
}
```

- [ ] **Step 4: Verify the `data` borrow is still live where the matrix step runs**

The existing loop binds `let data = resp.pointer("/result/data")...`. Confirm the matrix step (Step 2 block) is inside the same loop iteration AFTER that binding and that `resp`/`data` are not dropped earlier. If the compiler complains about a moved/dropped borrow, pass `data` (a `&Value`) — it is a reference into `resp`, which lives to the end of the loop body. No clone needed.

Run: `cargo build -p indexer 2>&1 | tail -20`
Expected: compiles (may require the external git dep fetch — authorize if prompted).

- [ ] **Step 5: Clippy + offline test gate**

Run: `cargo clippy -p indexer --all-targets -- -D warnings 2>&1 | tail -10`
Expected: clean.
Run: `cargo test --workspace 2>&1 | tail -15`
Expected: all pass; the strike_matrix integration tests show as `ignored`.

- [ ] **Step 6: Commit**

```bash
git add crates/indexer/src/bin/poller.rs
git commit -m "feat(poller): wire oracle_matrices dynamic-field traversal into the poll loop"
```

---

## Task 4: Live smoke + calibration + Monkey

**Files:** none (verification task; may add a one-line code fix if calibration reveals a wrong scale/slice — that is the point of doing it live).

**Interfaces:** none.

> **Prerequisite:** a reachable Postgres (`DATABASE_URL`) and live testnet. The leaf slice and `mtm` scale were probed during planning (leaf = last-N confirmed; `mtm` assumed 6-dec); this task confirms them end-to-end and runs the failure-mode (Monkey) checks the unit tests can't.

- [ ] **Step 1: Run the live DB integration tests**

Run: `cargo test -p indexer --test strike_matrix_integration -- --ignored 2>&1 | tail -20`
Expected: PASS (2 tests: dedup/view, delisting tombstone).

- [ ] **Step 2: Live smoke — run the poller briefly against testnet**

Run (≈30–40s, enough for ≥2 ticks):
```bash
RUST_LOG=info timeout 40 ./target/debug/poller 2>&1 | tee /tmp/matrix_smoke.log | tail -40
```
(Build first if needed: `cargo build -p indexer`.)
Expected: `persisted strike matrix` lines for the non-empty matrices on tick 1; on a quiet tick 2, zero new matrix inserts (version dedup → 0 multiGet). No fatal exit.

- [ ] **Step 3: Verify row counts + the leaf invariant held live**

Run:
```bash
psql "$DATABASE_URL" -c "SELECT count(*) AS matrices, count(*) FILTER (WHERE mtm>0) AS nonempty FROM strike_matrix_latest;"
psql "$DATABASE_URL" -c "SELECT count(*) FROM oracle_matrix_listing;"
```
Expected: `oracle_matrix_listing` = 23; `strike_matrix_latest` ≈ 23 (13 with mtm>0). The poller never hit the root≡Σleaves `bail!` (no fatal exit in Step 2 ⇒ invariant held for every matrix). If it DID exit fatally on the invariant, the leaf slice is wrong — fix `extract_leaves` (the slice is `nodes[leaf_count-1..]`; re-derive against the live array) and re-run.

- [ ] **Step 4: Calibrate scales against a known matrix**

Pick a non-empty matrix from the view and sanity-check decoded values:
```bash
psql "$DATABASE_URL" -c "SELECT oracle_id, mtm, min_strike, max_strike, tick_size, jsonb_array_length(page_leaves) AS leaves FROM strike_matrix_latest WHERE mtm>0 ORDER BY mtm DESC LIMIT 3;"
```
Expected: `min_strike`/`max_strike` are plausible BTC strikes (e.g. 50000 / 150000), `tick_size` ~1.0, `leaves` = 256, `mtm` a plausible DUSDC figure (hundreds–thousands). If `mtm` is off by a known power of ten, fix ONLY the view's `/1e6` divisor (raw column unchanged → no re-index) and `CREATE OR REPLACE VIEW`. Record the verified `range_qty` / leaf `q_*` scale (or leave raw if still ambiguous) in the spec's scale note.

- [ ] **Step 5: Monkey 1 — wrong table id → fatal, actionable**

Temporarily point the Predict object at a non-existent matrices table to force a deterministic error path. Easiest: in a scratch run, set an env/PREDICT override is overkill — instead verify the deterministic-error branch by pre-seeding: call the binary against a fullnode while the table id is corrupted is hard to inject cleanly. Practical check: confirm the code path by a focused live probe — temporarily change `parse_oracle_matrices_table_id`'s pointer to a wrong key in a throwaway edit, `touch` the file, rebuild, run, observe `getDynamicFields error` fatal exit 1, then `git checkout` the file.

> **Lesson (2026-06-21):** after editing source with sed/scripts, `touch` the file before `cargo build` or verify `strings target/debug/poller | grep <sentinel>` — cargo's mtime fingerprint occasionally skips a rebuild and you test a stale binary.

Run (after the throwaway edit + `touch` + build):
```bash
RUST_LOG=info ./target/debug/poller >/tmp/monkey1.log 2>&1; echo "exit=$?"
```
Expected: `exit=1`, log contains an actionable `getDynamicFields error` (or `missing vault.oracle_matrices`). Then `git checkout crates/indexer/src/strike_matrix.rs`.

- [ ] **Step 6: Monkey 2 — restart dedup (no duplicate rows)**

Run the poller ~20s, stop (Ctrl-C), record row count, restart ~20s, compare:
```bash
psql "$DATABASE_URL" -c "SELECT count(*) FROM strike_matrix_state;"   # before restart
# ... restart poller for ~20s, quiet market ...
psql "$DATABASE_URL" -c "SELECT count(*) FROM strike_matrix_state;"   # after restart
```
Expected: equal (or higher ONLY by genuinely new versions from real trades), never doubled — cold start re-lists all 23, but unchanged versions hit `ON CONFLICT DO NOTHING`.

- [ ] **Step 7: Update progress + lessons, then commit any calibration fix**

Update `tasks/progress.md` (mark per-strike inventory DONE, record verified scales + leaf slice) and `tasks/lessons.md` if a calibration surprise occurred. If a view fix was applied in Step 4:

```bash
git add crates/indexer/migrations/0003_strike_matrix_state.sql
git commit -m "fix(poller): calibrate strike_matrix view scale from live smoke"
```

- [ ] **Step 8: Final gate**

Run: `cargo test --workspace 2>&1 | tail -10` → all pass (integration ignored offline).
Run: `cargo clippy --all-targets -- -D warnings 2>&1 | tail -10` → clean.

---

## Self-Review

**Spec coverage:**
- Tier 1 scalars + page_tree leaves → Task 1 (`parse_strike_matrix`, `extract_leaves`). ✓
- getDynamicFields pagination + version dedup → Task 1 (`parse_dynamic_fields_page`) + Task 3 (`poll_matrices` loop). ✓
- multiGetObjects 50-cap chunking → Task 1 (`chunk_ids`) + Task 3. ✓
- Write fetched-object version, not listing version → Task 1 (`parse_strike_matrix` uses `data.version`) + spec note. ✓
- Prune map + tombstone listing for delisted matrices → Task 2 (`replace_matrix_listing`, view INNER JOIN) + Task 3 (`retain`). ✓
- Leaf slice from `page_tree_leaf_count`, sum-invariant only on `total_q_*` → Task 1 (`extract_leaves`). ✓
- Schema raw-NUMERIC + decode-in-view + div-by-zero/horizon notes → Task 2 migration. ✓
- Error tiering (transport WARN/retry vs parse fatal) → Task 3 (`poll_matrices`). ✓
- JSON-RPC EOL, mtm assumed-scale → spec notes + Task 4 calibration. ✓
- Tier 2 deferred seam → spec §Tier 2 (no task this round, by design). ✓
- Tests: unit (Task 1), integration (Task 2), pagination/chunk (Task 1), live smoke + Monkey (Task 4). ✓

**Placeholder scan:** No TBD/TODO; every code step shows full code; commands have expected output. ✓

**Type consistency:** `StrikeMatrixState`/`PageLeaf`/`DynField` field names identical across Tasks 1–3; `parse_strike_matrices(&[Value])`, `chunk_ids(&[String], usize) -> Vec<&[String]>`, `insert_strike_matrix_state(&PgPool, &StrikeMatrixState)`, `replace_matrix_listing(&PgPool, &[DynField])` consistent between definition (Tasks 1–2) and use (Task 3). ✓
