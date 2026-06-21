-- B-path Predict-object state snapshots. Raw chain integers as NUMERIC (source of
-- truth); decoding (DUSDC /1e6, NAV/utilization) lives only in predict_latest.
-- Dedup key = object_version: it bumps on every mutation, so re-polling an
-- unchanged object is a no-op and row count is capped at distinct states.

CREATE TABLE IF NOT EXISTS predict_state (
  object_version          BIGINT      NOT NULL,
  vault_balance           NUMERIC     NOT NULL,
  vault_total_mtm         NUMERIC     NOT NULL,
  vault_total_max_payout  NUMERIC     NOT NULL,
  wl_enabled              BOOLEAN     NOT NULL,
  wl_available            NUMERIC     NOT NULL,
  wl_capacity             NUMERIC     NOT NULL,
  wl_refill_rate_per_ms   NUMERIC     NOT NULL,
  wl_last_updated_ms      NUMERIC     NOT NULL,
  ingested_at             TIMESTAMPTZ NOT NULL DEFAULT now(),
  PRIMARY KEY (object_version)
);

CREATE OR REPLACE VIEW predict_latest AS
SELECT
  object_version,
  -- NAV: mirrors on-chain vault::vault_value (body NOT decompiled). balance +
  -- total_mtm is our best guess; total_mtm is U64 unsigned so the sign convention
  -- is unverified. If decompile later shows `balance - total_mtm`, fix THIS LINE
  -- only (raw columns are the source of truth → no re-index needed).
  (vault_balance + vault_total_mtm)::float8 / 1e6                  AS nav,
  -- utilization: OUR definition (max_payout / balance), NOT the protocol's
  -- internal spread-utilization.
  vault_total_max_payout::float8 / NULLIF(vault_balance, 0)::float8 AS utilization,
  vault_balance::float8          / 1e6 AS balance,
  vault_total_mtm::float8        / 1e6 AS total_mtm,
  vault_total_max_payout::float8 / 1e6 AS total_max_payout,
  -- withdrawal_available: OUR mirror. enabled=false → unlimited → NULL.
  CASE WHEN wl_enabled THEN wl_available::float8 / 1e6 ELSE NULL END AS withdrawal_available,
  wl_enabled,
  ingested_at
FROM predict_state
ORDER BY object_version DESC
LIMIT 1;
