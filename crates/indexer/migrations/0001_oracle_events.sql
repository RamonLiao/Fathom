-- A-path oracle event log. Raw chain integers as unbounded NUMERIC (source of
-- truth); decoding (/1e9, aligned with crates/types fixed.rs::ONE) lives only
-- in the oracle_latest view. Dedup key (tx_digest, event_index) is the Sui
-- event's content-addressed identity.

CREATE TABLE IF NOT EXISTS prices_update (
  tx_digest      TEXT        NOT NULL,
  event_index    BIGINT      NOT NULL,
  checkpoint_seq BIGINT      NOT NULL,
  oracle_id      TEXT        NOT NULL,
  spot           NUMERIC     NOT NULL,
  forward        NUMERIC     NOT NULL,
  ts_chain_ms    NUMERIC     NOT NULL,
  ingested_at    TIMESTAMPTZ NOT NULL DEFAULT now(),
  PRIMARY KEY (tx_digest, event_index)
);

CREATE TABLE IF NOT EXISTS svi_update (
  tx_digest      TEXT        NOT NULL,
  event_index    BIGINT      NOT NULL,
  checkpoint_seq BIGINT      NOT NULL,
  oracle_id      TEXT        NOT NULL,
  a              NUMERIC     NOT NULL,
  b              NUMERIC     NOT NULL,
  sigma          NUMERIC     NOT NULL,
  rho            NUMERIC     NOT NULL,   -- signed (sign decoded in Rust at insert)
  m              NUMERIC     NOT NULL,   -- signed
  ts_chain_ms    NUMERIC     NOT NULL,
  sanity_forward NUMERIC,                -- forward the no-arb check ran against; NULL iff untested
  sanity         TEXT        NOT NULL,   -- 'untested' | 'clean' | 'dirty'
  sanity_reasons TEXT[],
  ingested_at    TIMESTAMPTZ NOT NULL DEFAULT now(),
  PRIMARY KEY (tx_digest, event_index)
);

CREATE INDEX IF NOT EXISTS svi_oracle_seq_idx    ON svi_update    (oracle_id, checkpoint_seq DESC);
CREATE INDEX IF NOT EXISTS prices_oracle_seq_idx ON prices_update (oracle_id, checkpoint_seq DESC);

CREATE OR REPLACE VIEW oracle_latest AS
WITH latest_svi AS (
  SELECT DISTINCT ON (oracle_id) *
  FROM svi_update ORDER BY oracle_id, checkpoint_seq DESC, event_index DESC
),
latest_prices AS (
  SELECT DISTINCT ON (oracle_id) *
  FROM prices_update ORDER BY oracle_id, checkpoint_seq DESC, event_index DESC
)
SELECT
  COALESCE(s.oracle_id, p.oracle_id)      AS oracle_id,
  s.a::float8     / 1e9 AS a,
  s.b::float8     / 1e9 AS b,
  s.rho::float8   / 1e9 AS rho,
  s.m::float8     / 1e9 AS m,
  s.sigma::float8 / 1e9 AS sigma,
  s.sanity              AS svi_sanity,    -- may be NULL: prices-only oracle
  s.checkpoint_seq      AS svi_checkpoint_seq,
  p.spot::float8    / 1e9 AS spot,
  p.forward::float8 / 1e9 AS forward,
  p.checkpoint_seq        AS prices_checkpoint_seq
FROM latest_svi s
FULL OUTER JOIN latest_prices p USING (oracle_id);
