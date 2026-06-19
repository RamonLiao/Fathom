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
                                                            bounded mpsc (sync send, 滿了才等)
                                                                       │
                                                         background writer task (own PgPool)
                                                         drain → INSERT … ON CONFLICT DO NOTHING
```

- **`Sink` trait 保持 sync** → pure pipeline 與其單元測試零 tokio 汙染。DB 延遲用 channel + writer task 解耦，checkpoint stream 不會卡在 DB 上。
- `TeeSink(Vec<Box<dyn Sink>>)` 同時 fan-out 到 stdout + postgres → live smoke 既看得到 log 又落地。`main` 依 `DATABASE_URL` env 決定是否掛 `PostgresSink`（未設 → 純 stdout，維持現行 dev 行為）。

**為什麼不用 async Sink**：`emit` 一旦 async → `handle_event`/`process_checkpoint` 全 async → 既有 pipeline 純測試全要改 `#[tokio::test]` + 重拉 `async-trait`。channel 方案把 async 隔離在 writer task，pure core 不動。

**channel 用 bounded（≈1024）非 unbounded**（sui-indexer review N2）：consumer 上游是 `subscribe_bounded(SUBSCRIBER_CHANNEL_SIZE)`，已有 backpressure。若 writer 因 DB 慢而 stall，unbounded channel 會無限長、把上游 bounded 的意義抵消掉（OOM 風險）。改 bounded → 慢 DB 時 backpressure 一路傳回 ingestion。量級 19 oracle × ~1 批/sec ≈ 38 row/sec，正常根本不會滿。DB 真掛掉時 writer 回 Err → 整個 indexer loud 收場（§6）。
  - 注意 `Sink::emit` 是 sync，但 consumer loop 在 async context → bounded 滿時不能 sync-block runtime。實作上 emit 對 `tokio::sync::mpsc::Sender` 用 `try_send`：成功即走；`Full` → 這是 DB 跟不上 38 row/sec 的異常訊號，loud error + fatal（不靜默丟）。`Closed`（writer 已死）同樣 fatal。
- **次要**（sui-indexer S1）：`EventId.tx_digest: String` 在 sync 熱路徑每事件做一次 base58 alloc。38 row/sec 下無感，但別當零成本宣稱。

## 3. Schema

Migration：`crates/indexer/migrations/0001_oracle_events.sql`。兩張 append-only 表（payload 不同，不用 nullable 大雜燴）+ 一個 latest-state view。

所有 raw chain 整數用 **unbounded `NUMERIC`**（sui-architect M2）：u64 max 1.8e19 雖然進得了 `NUMERIC(20,0)`（20 位）但零 headroom，邊界即脆；unbounded 零成本、未來任何 u128/u256 欄位免 migration。

```sql
CREATE TABLE prices_update (
  tx_digest      TEXT        NOT NULL,
  event_index    BIGINT      NOT NULL,
  checkpoint_seq BIGINT      NOT NULL,
  oracle_id      TEXT        NOT NULL,   -- 0x hex (ObjId Display)
  spot           NUMERIC     NOT NULL,   -- u64 raw（1e9-scaled，decode 在 view）
  forward        NUMERIC     NOT NULL,
  ts_chain_ms    NUMERIC     NOT NULL,   -- 鏈上 timestamp（raw）
  ingested_at    TIMESTAMPTZ NOT NULL DEFAULT now(),
  PRIMARY KEY (tx_digest, event_index)
);

CREATE TABLE svi_update (
  tx_digest      TEXT        NOT NULL,
  event_index    BIGINT      NOT NULL,
  checkpoint_seq BIGINT      NOT NULL,
  oracle_id      TEXT        NOT NULL,
  a              NUMERIC     NOT NULL,   -- u64 raw
  b              NUMERIC     NOT NULL,
  sigma          NUMERIC     NOT NULL,
  -- rho/m：I64Raw{magnitude:u64, is_negative} → 在 Rust insert 時組 SIGNED 值
  -- (is_negative ? -magnitude : magnitude)。單欄 signed NUMERIC，lossless。
  -- 簽名在 Rust 算（單一 signed-decode 路徑），不在 SQL；-0（mag=0,neg）→ +0。
  rho            NUMERIC     NOT NULL,
  m              NUMERIC     NOT NULL,
  ts_chain_ms    NUMERIC     NOT NULL,
  -- 把「算 sanity 當下用的 forward」存進來 → verdict 變 row-local 純函數、可重現
  -- (sui-architect S1)。SVI 早於該 oracle 首個 prices ⇒ Untested ⇒ NULL。
  sanity_forward NUMERIC,
  sanity         TEXT        NOT NULL,   -- 'untested' | 'clean' | 'dirty'
  sanity_reasons TEXT[],                 -- dirty 時的原因，否則 NULL
  ingested_at    TIMESTAMPTZ NOT NULL DEFAULT now(),
  PRIMARY KEY (tx_digest, event_index)
);

CREATE INDEX svi_oracle_seq_idx    ON svi_update    (oracle_id, checkpoint_seq DESC);
CREATE INDEX prices_oracle_seq_idx ON prices_update (oracle_id, checkpoint_seq DESC);

-- 每 oracle 最新 SVI + 最新 prices，順手 decode（/1e9，與 crates/types fixed.rs::ONE 對齊）。
-- tiebreaker event_index DESC：同 checkpoint 內多事件取「最後一筆」為確定性最新 (sui-architect S2)。
CREATE VIEW oracle_latest AS
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
  s.rho::float8   / 1e9 AS rho,           -- signed NUMERIC → 保留負號
  s.m::float8     / 1e9 AS m,
  s.sigma::float8 / 1e9 AS sigma,
  s.sanity              AS svi_sanity,    -- 可能為 NULL（見下）
  s.checkpoint_seq      AS svi_checkpoint_seq,
  p.spot::float8    / 1e9 AS spot,
  p.forward::float8 / 1e9 AS forward,
  p.checkpoint_seq        AS prices_checkpoint_seq
FROM latest_svi s
FULL OUTER JOIN latest_prices p USING (oracle_id);
```

- **存 raw chain 整數**（source of truth），view 做 `/1e9` decode 給前端。與 `types` 存 raw + `to_svi()` 的 single-source 原則一致；migration 註解指回 `crates/types fixed.rs::ONE` 讓 scale 耦合可被發現（sui-architect N3）。**簽名解碼只在 Rust insert 端做一次**，SQL view 不重實作，避免分歧。
- `oracle_latest.svi_sanity` 有 **第四態 NULL**：該 oracle 有 prices 但還沒收到任何 SVI（FULL OUTER JOIN 的 prices-only 列）。前端需處理 `untested | clean | dirty | NULL`。
- regular VIEW（非 materialized）：此量級即時算夠快，省 refresh 機制（YAGNI）。

## 4. Event identity plumbing（dedup 真 key）

dedup key = Sui 事件全域唯一身分 `(tx_digest, event_index)`，零靜默丟失（Rule 12）。

```rust
pub struct EventId { pub tx_digest: String, pub event_index: u64 }

pub trait Sink {
    fn emit(&self, id: &EventId, checkpoint_seq: u64, ev: &DecodedEvent);
}
```

`process_checkpoint` 組 `EventId` → 傳 `handle_event` → `emit`。連帶改 `StdoutSink` / `CaptureSink` / pipeline 測試。

**accessor 已對源碼驗證（rev `2e196df`，撤掉先前的「待驗證」標記，sui-indexer M2）**：
- `tx_digest`：`tx.transaction` 是 `TransactionData`（**不是** Envelope），`TransactionData::digest()` 存在、回 **owned** `TransactionDigest`，`Display` = base58 → `tx.transaction.digest().to_string()` 進 `TEXT` 欄。Mysten 自家 `tx_digests.rs` 在同型別上就是這樣用。
- `event_index`：**必須 enumerate 未過濾的 `events.data`，把 index `j` 帶過 oracle/package filter**（sui-indexer M1，canonical = `ev_struct_inst.rs`：`evs.data.iter().enumerate()`）。**不可**先 filter 再 enumerate — 否則變成「oracle 事件中的位置」，與 framework 的 EventID `event_seq` 分歧、且被插入的非 oracle 事件擠位。這是 plumbing 唯一會錯的地方。

**dedup key 為何 sound（Rule 9，測試要 assert 這個 why，sui-indexer S2）**：`TransactionDigest` 是對 `TransactionData` 內容定址（含 gas/sender/輸入物件），同 tx 重現必帶同 digest + 同事件佈局 → `ON CONFLICT DO NOTHING` 正確去重；Sui checkpoint 終局化、無 checkpoint-level reorg → `checkpoint_seq` 對已 commit 的 tx 不會 flap。

> sanity 不再是「同 key 重跑可能不同」的隱患：§3 已把 `sanity_forward` 一起存進列，verdict = (SVI raw 參數, sanity_forward) 的純函數，row-local 可重現（解 sui-architect S1）。`ON CONFLICT DO NOTHING` pin 第一筆 writer，但因可重現，第一筆與重跑必然一致。

## 5. Cargo / config

- `sqlx = { version = "0.8", default-features = false, features = ["runtime-tokio", "tls-rustls", "postgres", "migrate"] }` — rustls only，避 openssl/native-tls。
- **用 runtime `sqlx::query(...).bind(...)`，不用 `query!` 巨集** → build / clippy / CI 不需連 DB（不維護 `.sqlx` offline cache）。代價：失去 compile-time SQL 檢查；schema 固定且小，可接受。`sqlx::migrate!()` 在 build 時嵌入 migration 檔、runtime 執行（不連 build-time DB）。
- `DATABASE_URL` 走 **env var**（secret）：不進 `config.rs`、不進 git。

## 6. Error handling（Rule 12 fail-loud）

- 啟動連不上 DB / migration 失敗 → fatal exit，**不**退化成 no-op sink。
- `PgPool` 設 `acquire_timeout`（sui-architect N2）：hung（非 failed）DB 變成 loud `Err`，不會讓 writer 無限等、channel 撐爆。
- writer task INSERT 失敗（非 conflict）→ log error + 回 `Err` → fatal（與 A 路徑「decode 失敗 = fatal stop」一致）。
- **shutdown 次序是「無靜默丟失」真正所在，不可用 racing `try_join!`（sui-indexer S3）**：`try_join!` 第一個 `Err` 會取消 siblings → 可能在 writer drain 到一半把它砍掉、丟尾巴。正確次序：
  1. 先驅動 ingestion service + consumer loop 至 ingestion 跑完（`rx` 關 → consumer loop 自然結束）。
  2. consumer 結束後 **drop sink（sender）** → channel 關。
  3. **再** `writer.await` — 此時 channel 已關、writer drain 完剩餘 row 才返回（不丟尾巴）。
  - 任一階段的 `Err`（ingestion / consumer decode / writer insert）仍要往上冒成非零退出（fail-loud），只是不能用「會取消 drain」的併發原語包住 writer 的收尾。

## 7. Testing（Rule 9 + test.md monkey）

- **單元（無 DB）**：`DecodedEvent + EventId → InsertRow` 純映射函式 — 含 **I64 sign-magnitude → signed NUMERIC（負 rho 不得變正，`-0` mag=0,neg → +0）**、`ObjId` → 0x hex、sanity enum → text、`sanity_forward` 帶入。signed-decode 這個測試是 load-bearing（pricing gate 就是抓 sign-flip 的）。
- **整合（需本地 Postgres，env gate / `#[ignore]`）**：
  - **idempotency（核心 Rule 9 測試）**：同 `(tx_digest, event_index)` 插兩次 → 只剩一列。測試要 encode *why* = digest 內容定址，重放必帶同 key（不只「插兩次→一列」）。
  - **sanity 可重現**：同一 SVI row 在「prices 先到」與「prices 後到」兩種 replay 序下，因 `sanity_forward` 已固化，最終列 `sanity` 一致。
  - latest-state view 回正確最新列（多 checkpoint 後取最大 seq；同 checkpoint 多事件由 `event_index DESC` 決定）。
- **Monkey**：DB 啟動時關掉 → 啟動 loud fail；writer 中途殺；亂序 / 重複事件灌入 → 確認無靜默丟失。
- **live smoke**：`DATABASE_URL=… cargo run -p indexer` 對 testnet，查表確認 row 落地 + `oracle_latest` view 對得上 stdout log。

## 8. 已拍板的取捨（可推翻點）

1. **channel + sync `Sink`**（vs async Sink）— 為保 pure core 與其測試不動。
2. **runtime `sqlx::query` 不用 `query!` 巨集** — 為 CI / build 不依賴 DB。
3. **raw 整數存 NUMERIC(20,0) + view decode** — source of truth 在 raw，前端友善值在 view。
4. **regular VIEW 非 materialized** — 量級小，免 refresh（YAGNI）。

## 9. 明確放棄的 framework 機制（sui-indexer N1）

storeless A 路徑刻意丟掉 `cluster`/diesel feature（見 `crates/indexer/Cargo.toml`），所以我們也放棄 framework 的 `Handler` + committer + watermark/pruner（那些需要 diesel `postgres` feature + `Store`）。bounded-channel writer 是刻意的最小替代。代價：**沒有 built-in resume-from-watermark** — 重啟一律從 `tip - START_BACKFILL_CHECKPOINTS` 重新回填，靠 `(tx_digest, event_index)` + `ON CONFLICT DO NOTHING` 去重。此量級可接受；若日後要長期連續索引、避免重啟重掃，再評估轉用 framework 真 pipeline + `PostgresStore`（即原 B 路徑「第二 Processor」設想）。
