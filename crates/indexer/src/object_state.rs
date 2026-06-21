//! Pure parser for the shared `Predict` object's `sui_getObject{showContent}`
//! response. `vault` and `withdrawal_limiter` are inline struct fields, so one
//! object read yields every metric input. u64 fields arrive as decimal STRINGS.
//! Any missing/renamed/non-string-u64 field is a loud Err (on-chain layout drift
//! → the decode is wrong → fatal; same philosophy as the A-path decode rule).

use anyhow::{Context, Result};
use serde_json::Value;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PredictState {
    pub object_version: u64,
    pub vault_balance: u64,
    pub vault_total_mtm: u64,
    pub vault_total_max_payout: u64,
    pub wl_enabled: bool,
    pub wl_available: u64,
    pub wl_capacity: u64,
    pub wl_refill_rate_per_ms: u64,
    pub wl_last_updated_ms: u64,
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

fn bool_field(obj: &Value, key: &str) -> Result<bool> {
    obj.get(key)
        .and_then(Value::as_bool)
        .with_context(|| format!("missing or non-bool field `{key}`"))
}

/// Parse the `result.data` object of a `sui_getObject{showContent}` response.
pub fn parse_predict_state(data: &Value) -> Result<PredictState> {
    let object_version = u64_field(data, "version").context("object version")?;
    let fields = data
        .pointer("/content/fields")
        .context("missing content.fields (object has no parsed content)")?;
    let vault = fields
        .pointer("/vault/fields")
        .context("missing vault.fields")?;
    let wl = fields
        .pointer("/withdrawal_limiter/fields")
        .context("missing withdrawal_limiter.fields")?;
    Ok(PredictState {
        object_version,
        vault_balance: u64_field(vault, "balance")?,
        vault_total_mtm: u64_field(vault, "total_mtm")?,
        vault_total_max_payout: u64_field(vault, "total_max_payout")?,
        wl_enabled: bool_field(wl, "enabled")?,
        wl_available: u64_field(wl, "available")?,
        wl_capacity: u64_field(wl, "capacity")?,
        wl_refill_rate_per_ms: u64_field(wl, "refill_rate_per_ms")?,
        wl_last_updated_ms: u64_field(wl, "last_updated_ms")?,
    })
}

/// Idempotent insert: a repeated `object_version` is a no-op (the object did not
/// change between polls). Numerics bound as String + `$n::numeric`, mirroring the
/// A-path writer (no decimal crate, build needs no DB).
pub async fn insert_predict_state(pool: &sqlx::PgPool, s: &PredictState) -> Result<()> {
    sqlx::query(
        "INSERT INTO predict_state \
         (object_version,vault_balance,vault_total_mtm,vault_total_max_payout,\
          wl_enabled,wl_available,wl_capacity,wl_refill_rate_per_ms,wl_last_updated_ms) \
         VALUES ($1,$2::numeric,$3::numeric,$4::numeric,$5,$6::numeric,$7::numeric,$8::numeric,$9::numeric) \
         ON CONFLICT (object_version) DO NOTHING",
    )
    .bind(i64::try_from(s.object_version).context("object_version exceeds i64::MAX")?)
    .bind(s.vault_balance.to_string())
    .bind(s.vault_total_mtm.to_string())
    .bind(s.vault_total_max_payout.to_string())
    .bind(s.wl_enabled)
    .bind(s.wl_available.to_string())
    .bind(s.wl_capacity.to_string())
    .bind(s.wl_refill_rate_per_ms.to_string())
    .bind(s.wl_last_updated_ms.to_string())
    .execute(pool)
    .await
    .context("insert predict_state")?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    // Real getObject{showContent} response (result.data), captured 2026-06-21.
    // Pins the exact field paths and DUSDC scale that break on a package upgrade.
    fn golden() -> serde_json::Value {
        serde_json::json!({
            "version": "910884609",
            "type": "0xf5ea2b...::predict::Predict",
            "content": { "dataType": "moveObject", "fields": {
                "vault": { "type": "0xf5ea2b...::vault::Vault", "fields": {
                    "balance": "1017919271295",
                    "total_mtm": "1481157422",
                    "total_max_payout": "3493960252"
                }},
                "withdrawal_limiter": { "type": "0xf5ea2b...::rate_limiter::RateLimiter", "fields": {
                    "enabled": false,
                    "available": "0",
                    "capacity": "0",
                    "refill_rate_per_ms": "0",
                    "last_updated_ms": "1776383327247"
                }}
            }}
        })
    }

    #[test]
    fn parses_golden_object() {
        let s = parse_predict_state(&golden()).unwrap();
        assert_eq!(s.object_version, 910_884_609);
        assert_eq!(s.vault_balance, 1_017_919_271_295);
        assert_eq!(s.vault_total_mtm, 1_481_157_422);
        assert_eq!(s.vault_total_max_payout, 3_493_960_252);
        assert!(!s.wl_enabled);
        assert_eq!(s.wl_available, 0);
        assert_eq!(s.wl_capacity, 0);
        assert_eq!(s.wl_refill_rate_per_ms, 0);
        assert_eq!(s.wl_last_updated_ms, 1_776_383_327_247);
    }

    #[test]
    fn missing_field_is_loud() {
        // WHY: a package upgrade that renames/drops a field must fail loudly, not
        // silently produce a wrong NAV.
        let mut v = golden();
        v["content"]["fields"]["vault"]["fields"]
            .as_object_mut().unwrap().remove("total_mtm");
        let err = parse_predict_state(&v).unwrap_err().to_string();
        assert!(err.contains("total_mtm"), "error must name the missing field: {err}");
    }

    #[test]
    fn non_string_u64_is_loud() {
        // WHY: showContent encodes u64 as decimal strings. A number (or anything
        // non-string) signals a format change we must not silently coerce.
        let mut v = golden();
        v["content"]["fields"]["vault"]["fields"]["balance"] =
            serde_json::json!(1017919271295u64);
        assert!(parse_predict_state(&v).is_err());
    }
}
