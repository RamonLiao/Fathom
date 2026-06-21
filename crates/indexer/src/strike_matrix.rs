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

/// Extract the N leaf `PageLeaf`s from the inline `page_tree` segment tree.
/// `page_tree` is a complete binary heap of `2*leaf_count - 1` `PageSummary` nodes
/// (root at index 0, leaves last). We keep only the leaves' `total_q_up`/`total_q_dn`.
/// Verifies the sum invariant `root.total_q_* == Σ leaf.total_q_*` (sum-semantics
/// fields only — `best_prefix_*` are prefix-extremes and are intentionally ignored).
pub fn extract_leaves(page_tree: &Value, leaf_count: usize) -> Result<Vec<PageLeaf>> {
    if leaf_count == 0 {
        bail!("page_tree_leaf_count is 0 — a StrikeMatrix always has ≥1 leaf; layout drift");
    }
    let nodes = page_tree.as_array().context("page_tree is not an array")?;
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
/// Each element is `{ data: { ... } }` OR `{ error: { code, ... } }`. Only an
/// OBJECT-ABSENCE error (`notExists`/`deleted`) is a BENIGN read race — a matrix can
/// settle/delist between the getDynamicFields listing and this fetch — and is SKIPPED
/// (it drops from the next listing; its version-map entry stays stale → re-tried). Any
/// OTHER per-element error (permission, node-internal, unknown) → loud Err (we must not
/// silently drop a matrix on a real RPC failure). A present-but-unparseable `data` is
/// likewise a loud Err (real layout drift → fatal).
pub fn parse_strike_matrices(objects: &[Value]) -> Result<Vec<StrikeMatrixState>> {
    objects
        .iter()
        .filter_map(|o| match o.get("data") {
            Some(data) => Some(parse_strike_matrix(data)),
            None => match o.get("error") {
                Some(err) => {
                    let code = err.get("code").and_then(Value::as_str).unwrap_or("");
                    if code == "notExists" || code == "deleted" {
                        None // benign object-absence race → skip
                    } else {
                        Some(Err(anyhow::anyhow!(
                            "multiGetObjects element error (not a benign absence): {err}"
                        )))
                    }
                }
                None => Some(Err(anyhow::anyhow!(
                    "multiGetObjects element has neither data nor error: {o}"
                ))),
            },
        })
        .collect()
}

/// Parse one `suix_getDynamicFields` page → (items, next_cursor). A `None` cursor
/// or `hasNextPage == false` ends pagination. `version` is a JSON number here
/// (unlike the string-encoded u64s inside showContent).
pub fn parse_dynamic_fields_page(resp: &Value) -> Result<(Vec<DynField>, Option<String>)> {
    let data = resp
        .get("data")
        .and_then(Value::as_array)
        .context("getDynamicFields missing data")?;
    let mut items = Vec::with_capacity(data.len());
    for e in data {
        let oracle_id = e
            .pointer("/name/value")
            .and_then(Value::as_str)
            .context("dynamic field name.value (oracle_id) missing")?
            .to_string();
        let object_id = e
            .get("objectId")
            .and_then(Value::as_str)
            .context("objectId missing")?
            .to_string();
        let version = e
            .get("version")
            .and_then(Value::as_u64)
            .context("version missing or non-numeric")?;
        items.push(DynField {
            oracle_id,
            object_id,
            version,
        });
    }
    let has_next = resp
        .get("hasNextPage")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let next = if has_next {
        Some(
            resp.get("nextCursor")
                .and_then(Value::as_str)
                .context("hasNextPage but no nextCursor")?
                .to_string(),
        )
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

/// Idempotent insert: a repeated `(matrix_object_id, matrix_version)` is a no-op.
/// Numerics bound as String + `$n::numeric` (no decimal crate); page_leaves bound as
/// JSONB with raw u64s as strings (source of truth; decode lives in the view).
pub async fn insert_strike_matrix_state(pool: &sqlx::PgPool, s: &StrikeMatrixState) -> Result<()> {
    let leaves = Value::Array(
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
            node(100, 10),            // 0 root
            node(30, 3), node(70, 7), // 1,2 internal
            node(10, 1), node(20, 2), // 3,4 leaves[0],[1]
            node(30, 3), node(40, 4), // 5,6 leaves[2],[3]
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

    #[test]
    fn extract_leaves_rejects_zero_leaf_count() {
        // WHY: leaf_count==0 would underflow `2*N-1`/`N-1` (usize) → panic. A panic is
        // not a graceful loud error; reject it as drift instead (Rule 12).
        assert!(extract_leaves(&serde_json::json!([]), 0).is_err());
    }

    // Real multiGetObjects element shape (result[i]), trimmed page_tree to N=2.
    fn matrix_obj() -> Value {
        let node = |up: u64, dn: u64| {
            serde_json::json!({ "fields": {
            "total_q_up": up.to_string(), "total_q_dn": dn.to_string(),
            "best_prefix_up": "0", "best_prefix_dn": "0" }})
        };
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
        assert_eq!(
            s.matrix_object_id,
            "0x1104586103fce6a5dfcdcd767c4303dfbb280aaed5f45d0fa51c6d5cc2fc5646"
        );
        assert_eq!(
            s.oracle_id,
            "0xed2cc924940c74b0eed46f174e2cecf5dee602ef1f4246b8d2acc35c31af3159"
        );
        assert_eq!(s.matrix_version, 910_924_146);
        assert_eq!(s.mtm, 615_919_646);
        assert_eq!(s.range_qty, 301_396_529);
        assert_eq!(s.min_strike, 50_000_000_000_000);
        assert_eq!(s.minted_min_strike, u64::MAX); // sentinel round-trips
        assert_eq!(s.tick_size, 1_000_000_000);
        assert_eq!(
            s.page_leaves,
            vec![
                PageLeaf { q_up: 10, q_dn: 1 },
                PageLeaf { q_up: 20, q_dn: 2 }
            ]
        );
    }

    #[test]
    fn missing_field_is_loud() {
        // WHY: a package upgrade dropping/renaming a field must fail, not silently mis-decode.
        let mut v = matrix_obj();
        v["content"]["fields"]["value"]["fields"]
            .as_object_mut()
            .unwrap()
            .remove("mtm");
        let err = parse_strike_matrix(&v).unwrap_err().to_string();
        // anyhow root cause names the field; the top context names the matrix.
        let full = format!("{:#}", parse_strike_matrix(&v).unwrap_err());
        assert!(
            err.contains("mtm") || full.contains("mtm"),
            "error must name the missing field: {full}"
        );
    }

    #[test]
    fn non_string_u64_is_loud() {
        let mut v = matrix_obj();
        v["content"]["fields"]["value"]["fields"]["mtm"] = serde_json::json!(615919646u64);
        assert!(parse_strike_matrix(&v).is_err());
    }

    #[test]
    fn parse_matrices_skips_not_exists_element() {
        // WHY: a matrix can settle/delist between getDynamicFields and multiGetObjects.
        // That per-element `error` is a benign read race, NOT layout drift — skip it,
        // don't crash the poller (the missing matrix drops from the next listing).
        let objs = vec![
            serde_json::json!({ "error": { "code": "notExists", "object_id": "0xgone" } }),
            serde_json::json!({ "data": matrix_obj() }),
        ];
        let states = parse_strike_matrices(&objs).unwrap();
        assert_eq!(states.len(), 1, "the notExists element is skipped, the valid one parsed");
    }

    #[test]
    fn parse_matrices_fatal_on_non_absence_error() {
        // WHY: only notExists/deleted are benign races. A permission/internal error
        // must NOT be silently swallowed (Rule 12) — it would drop a matrix unnoticed.
        let objs = vec![serde_json::json!({ "error": { "code": "displayError", "error": "boom" } })];
        assert!(parse_strike_matrices(&objs).is_err());
    }

    #[test]
    fn parse_matrices_fatal_on_present_but_malformed_data() {
        // WHY: a present `data` that won't parse is real drift → must stay fatal.
        let mut bad = matrix_obj();
        bad["content"]["fields"]["value"]["fields"].as_object_mut().unwrap().remove("mtm");
        let objs = vec![serde_json::json!({ "data": bad })];
        assert!(parse_strike_matrices(&objs).is_err());
    }

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
        assert_eq!(
            items[0],
            DynField {
                oracle_id: "0xoracleA".into(),
                object_id: "0xmatrixA".into(),
                version: 910_924_146
            }
        );
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
}
