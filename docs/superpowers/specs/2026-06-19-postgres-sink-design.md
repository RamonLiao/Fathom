# PostgresSink — A 路徑 oracle 事件落地 Postgres

_Design spec · 2026-06-19 · Plan track_

## 1. Scope

把現有 A 路徑的 `OracleSVIUpdated` / `OraclePricesUpdated` 事件**多寫一份到 Postgres**，供 transparency dashboard 查時序與最新狀態。

**In scope**：新增 `PostgresSink`（behind 既有 `Sink` trait）、schema + migration、event-identity plumbing（dedup 真 key）、`TeeSink`、整合測試。

**Out of scope（下一輪）**：B 路徑 object poll（讀 `Predict` object state：NAV/utilization/withdrawal/per-strike inventory）、ingestion 機制、decode 邏輯、`predict::*` 事件。

## 2. Architecture / 資料流

```
process_checkpoint ──(EventId + DecodedEvent)──▶ handle_event ──▶ Sink::emit (sync)
                                                                       │
                                         ┌─────────────────────────────┤
                                         ▼                             ▼
                                    StdoutSink                   PostgresSink
                                 (tracing::info)            emit = 把 owned row 丟進
                                                            unbounded mpsc (sync, 不阻塞)
                                                                       │
                                                         background writer task (own PgPool)
                                                         drain → INSERT … ON CONFLICT DO NOTHING
```

- **`Sink` trait 保持 sync** → pure pipeline 與其單元測試零 tokio 汙染。DB 延遲用 unbounded channel + writer task 解耦，checkpoint stream 不會卡在 DB 上。
- `TeeSink(Vec<Box<dyn Sink>>)` 同時 fan-out 到 stdout + postgres → live smoke 既看得到 log 又落地。`main` 依 `DATABASE_URL` env 決定是否掛 `PostgresSink`（未設 → 純 stdout，維持現行 dev 行為）。

**為什麼不用 async Sink**：`emit` 一旦 async → `handle_event`/`process_checkpoint` 全 async → 既有 pipeline 純測試全要改 `#[tokio::test]` + 重拉 `async-trait`。channel 方案把 async 隔離在 writer task，pure core 不動。

**unbounded channel 安全性**：量級為 19 oracle × ~1 批/sec ≈ 38 row/sec。DB 真掛掉時 writer 回 Err → 整個 indexer loud 收場（§6），不會邊長 channel 邊靜默丟。

## 3. Schema

Migration：`crates/indexer/migrations/0001_oracle_events.sql`。兩張 append-only 表（payload 不同，不用 nullable 大雜燴）+ 一個 latest-state view。

```sql
CREATE TABLE prices_update (
  tx_digest      TEXT          NOT NULL,
  event_index    BIGINT        NOT NULL,
  checkpoint_seq BIGINT        NOT NULL,
  oracle_id      TEXT          NOT NULL,   -- 0x hex (ObjId Display)
  spot           NUMERIC(20,0) NOT NULL,   -- u64 raw（不進 BIGINT：9.2e18 < u64 max 1.8e19）
  forward        NUMERIC(20,0) NOT NULL,
  ts_chain_ms    NUMERIC(20,0) NOT NULL,   -- 鏈上 timestamp（raw）
  ingested_at    TIMESTAMPTZ   NOT NULL DEFAULT now(),
  PRIMARY KEY (tx_digest, event_index)
);

CREATE TABLE svi_update (
  tx_digest      TEXT          NOT NULL,
  event_index    BIGINT        NOT NULL,
  checkpoint_seq BIGINT        NOT NULL,
  oracle_id      TEXT          NOT NULL,
  a              NUMERIC(20,0) NOT NULL,    -- u64 raw
  b              NUMERIC(20,0) NOT NULL,
  sigma          NUMERIC(20,0) NOT NULL,
  rho            NUMERIC(20,0) NOT NULL,    -- i64 sign-magnitude → signed NUMERIC（lossless）
  m              NUMERIC(20,0) NOT NULL,
  ts_chain_ms    NUMERIC(20,0) NOT NULL,
  sanity         TEXT          NOT NULL,    -- 'untested' | 'clean' | 'dirty'
  sanity_reasons TEXT[],                    -- dirty 時的原因，否則 NULL
  ingested_at    TIMESTAMPTZ   NOT NULL DEFAULT now(),
  PRIMARY KEY (tx_digest, event_index)
);

CREATE INDEX svi_oracle_seq_idx    ON svi_update    (oracle_id, checkpoint_seq DESC);
CREATE INDEX prices_oracle_seq_idx ON prices_update (oracle_id, checkpoint_seq DESC);

-- 每 oracle 最新 SVI + 最新 prices，順手 decode（/1e9）給 dashboard
CREATE VIEW oracle_latest AS
WITH latest_svi AS (
  SELECT DISTINCT ON (oracle_id) *
  FROM svi_update ORDER BY oracle_id, checkpoint_seq DESC
),
latest_prices AS (
  SELECT DISTINCT ON (oracle_id) *
  FROM prices_update ORDER BY oracle_id, checkpoint_seq DESC
)
SELECT
  COALESCE(s.oracle_id, p.oracle_id)            AS oracle_id,
  s.a::float8     / 1e9 AS a,
  s.b::float8     / 1e9 AS b,
  s.rho::float8   / 1e9 AS rho,
  s.m::float8     / 1e9 AS m,
  s.sigma::float8 / 1e9 AS sigma,
  s.sanity              AS svi_sanity,
  s.checkpoint_seq      AS svi_checkpoint_seq,
  p.spot::float8    / 1e9 AS spot,
  p.forward::float8 / 1e9 AS forward,
  p.checkpoint_seq        AS prices_checkpoint_seq
FROM latest_svi s
FULL OUTER JOIN latest_prices p USING (oracle_id);
```

- **存 raw chain 整數**（source of truth），view 做 `/1e9` decode 給前端。與 `types` 存 raw + `to_svi()` 的 single-source 原則一致。
- regular VIEW（非 materialized）：此量級即時算夠快，省 refresh 機制（YAGNI）。

## 4. Event identity plumbing（dedup 真 key）

dedup key = Sui 事件全域唯一身分 `(tx_digest, event_index)`，零靜默丟失（Rule 12）。

```rust
pub struct EventId { pub tx_digest: String, pub event_index: u64 }

pub trait Sink {
    fn emit(&self, id: &EventId, checkpoint_seq: u64, ev: &DecodedEvent);
}
```

`process_checkpoint` 從 `events.data` 的 enumerate index（→ `event_index`）+ 該 tx 的 digest 組 `EventId` → 傳 `handle_event` → `emit`。連帶改 `StdoutSink` / `CaptureSink` / pipeline 測試。

> ⚠️ **framework API 未驗**：tx_digest 在 pinned rev `2e196df` 的確切 accessor（推測 `tx.transaction.digest()`，**待源碼驗證**）是先前咬過的那類風險（見 `tasks/lessons.md` 2026-06-18）。Plan 須標 known-risk，動工前對源碼驗 + build gate 兜底。

## 5. Cargo / config

- `sqlx = { version = "0.8", default-features = false, features = ["runtime-tokio", "tls-rustls", "postgres", "migrate"] }` — rustls only，避 openssl/native-tls。
- **用 runtime `sqlx::query(...).bind(...)`，不用 `query!` 巨集** → build / clippy / CI 不需連 DB（不維護 `.sqlx` offline cache）。代價：失去 compile-time SQL 檢查；schema 固定且小，可接受。`sqlx::migrate!()` 在 build 時嵌入 migration 檔、runtime 執行（不連 build-time DB）。
- `DATABASE_URL` 走 **env var**（secret）：不進 `config.rs`、不進 git。

## 6. Error handling（Rule 12 fail-loud）

- 啟動連不上 DB / migration 失敗 → fatal exit，**不**退化成 no-op sink。
- writer task INSERT 失敗（非 conflict）→ log error + 回 `Err` → `main` 以 `try_join!(ingestion, consumer, writer)` 收掉整個 process（與 A 路徑「decode 失敗 = fatal stop」一致）。
- 正常結束：ingestion 跑完 → `rx` 關 → consumer loop 結束 → drop sink(sender) → channel 關 → writer drain 完剩餘 row → 收工（不丟尾巴）。shutdown 次序：sink/sender 必須在 consumer loop 結束後才 drop。

## 7. Testing（Rule 9 + test.md monkey）

- **單元（無 DB）**：`DecodedEvent + EventId → InsertRow` 純映射函式 — 含 I64 sign-magnitude → signed NUMERIC 字串、`ObjId` → 0x hex、sanity enum → text。
- **整合（需本地 Postgres，env gate / `#[ignore]`）**：
  - **idempotency（核心 Rule 9 測試）**：同 `(tx_digest, event_index)` 插兩次 → 只剩一列。encode「重啟回填不可產生重複列」這個 *why*。
  - latest-state view 回正確最新列（多 checkpoint 後取最大 seq）。
- **Monkey**：DB 啟動時關掉 → 啟動 loud fail；writer 中途殺；亂序 / 重複事件灌入 → 確認無靜默丟失。
- **live smoke**：`DATABASE_URL=… cargo run -p indexer` 對 testnet，查表確認 row 落地 + `oracle_latest` view 對得上 stdout log。

## 8. 已拍板的取捨（可推翻點）

1. **channel + sync `Sink`**（vs async Sink）— 為保 pure core 與其測試不動。
2. **runtime `sqlx::query` 不用 `query!` 巨集** — 為 CI / build 不依賴 DB。
3. **raw 整數存 NUMERIC(20,0) + view decode** — source of truth 在 raw，前端友善值在 view。
4. **regular VIEW 非 materialized** — 量級小，免 refresh（YAGNI）。
