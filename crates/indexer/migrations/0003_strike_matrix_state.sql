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
  page_leaves        JSONB   NOT NULL,   -- [{"q_up":"..","q_dn":".."}] x N, raw u64 strings
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
-- Scales: strikes/tick_size /1e9; mtm /1e6 (live-verified 2026-06-22: per-matrix
-- mtm 180-568 DUSDC, strikes decode to 50000-150000 BTC USD — both consistent).
-- range_qty and leaf quantities left raw (scale unverified). page->strike mapping
-- is left to the frontend from the raw scalars; the data horizon is MIN(ingested_at).
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
