# PLP Risk Dashboard Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Ship Fathom's PLP Risk Dashboard — a read-only Rust Axum JSON API over the three existing Postgres views, and a Vite+React+Tailwind SPA that polls it every 10s.

**Architecture:** New `crates/api` (Axum + sqlx, read-only, reuses existing `DATABASE_URL`) serves three endpoints, one per view, with zero business logic (views already decode). New `web/` SPA polls those endpoints with TanStack Query and renders three regions (Vault Health, Oracle Exposure, Inventory Heatmap). In prod the Axum binary also serves `web/dist`. The frontend never touches the chain.

**Tech Stack:** Rust (axum 0.7, sqlx 0.8 runtime API, tokio), Vite + React 18 + TypeScript + Tailwind + TanStack Query v5 + Recharts; Vitest + React Testing Library.

## Global Constraints

- `crates/api` pulls ONLY axum + sqlx + serde + tokio (+ tower-http for static/CORS). **Zero sui-sdk / chain-network deps** — the dashboard reads Postgres, never the chain.
- sqlx **runtime** API (`sqlx::query(...).bind(...)`), never the `query!`/`query_as!` compile-time macros → `cargo build --workspace` and CI must NOT require `DATABASE_URL`.
- Integration tests use `#[sqlx::test]` with `#[ignore]` placed BELOW the macro → `cargo test --workspace` (no DB) stays offline-clean.
- raw-as-source-of-truth: view-decoded columns → `f64`; raw unverified-scale columns (`range_qty`, `page_leaves` q_up/q_dn) → `String`/`serde_json::Value`. Never read raw u64 as f64.
- Frontend: dark theme only. Every color maps to the CSS variables in the spec — NO raw Tailwind color tokens (`green-500`, `slate-800`). No Inter/Roboto/system-ui/Space Grotesk/Space Mono; no dial gauges; no rounded-2xl+shadow; one accent (`--sonar`).
- Staleness threshold = 3× poll interval = 30s, one constant in `api.ts`.
- Existing `crates/indexer` (poller, migrations) is NOT modified.
- Spec: `docs/superpowers/specs/2026-06-21-plp-risk-dashboard-design.md`. View DDL: `crates/indexer/migrations/0001..0003*.sql`.

---

### Task 1: Scaffold `crates/api` (Axum skeleton + pool + health)

**Files:**
- Modify: `Cargo.toml` (workspace `members`)
- Create: `crates/api/Cargo.toml`
- Create: `crates/api/src/main.rs`
- Create: `crates/api/src/state.rs`

**Interfaces:**
- Consumes: env `DATABASE_URL` (same var the indexer uses), optional `API_BIND` (default `0.0.0.0:8080`).
- Produces: `AppState { pool: sqlx::PgPool }`; `build_router(state: AppState) -> axum::Router`; binary `api`.

- [ ] **Step 1: Add `crates/api` to the workspace**

In root `Cargo.toml`, add `"crates/api"` to `members`. (Read the file first; append to the existing array — do not reorder existing entries.)

- [ ] **Step 2: Write `crates/api/Cargo.toml`**

```toml
[package]
name = "api"
version = "0.1.0"
edition = "2021"

[[bin]]
name = "api"
path = "src/main.rs"

[dependencies]
axum = "0.7"
tokio = { version = "1", features = ["macros", "rt-multi-thread", "signal"] }
sqlx = { version = "0.8", default-features = false, features = ["runtime-tokio", "tls-rustls", "postgres"] }
serde = { version = "1", features = ["derive"] }
serde_json = "1"
tower-http = { version = "0.6", features = ["fs", "cors"] }
tracing = "0.1"
tracing-subscriber = { version = "0.3", features = ["env-filter"] }
anyhow = "1"

[dev-dependencies]
sqlx = { version = "0.8", default-features = false, features = ["runtime-tokio", "tls-rustls", "postgres", "macros", "migrate"] }
tower = { version = "0.5", features = ["util"] }
http-body-util = "0.1"
```

- [ ] **Step 3: Write `crates/api/src/state.rs`**

```rust
use sqlx::PgPool;

#[derive(Clone)]
pub struct AppState {
    pub pool: PgPool,
}
```

- [ ] **Step 4: Write `crates/api/src/main.rs` skeleton with a health route**

```rust
mod state;

use axum::{routing::get, Router};
use state::AppState;
use std::env;

pub fn build_router(state: AppState) -> Router {
    Router::new()
        .route("/api/health", get(|| async { "ok" }))
        .with_state(state)
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .init();

    let database_url = env::var("DATABASE_URL")
        .map_err(|_| anyhow::anyhow!("DATABASE_URL must be set"))?;
    let bind = env::var("API_BIND").unwrap_or_else(|_| "0.0.0.0:8080".to_string());

    let pool = sqlx::postgres::PgPoolOptions::new()
        .max_connections(5)
        .connect(&database_url)
        .await?;

    let app = build_router(AppState { pool });
    let listener = tokio::net::TcpListener::bind(&bind).await?;
    tracing::info!("api listening on {bind}");
    axum::serve(listener, app).await?;
    Ok(())
}
```

- [ ] **Step 5: Verify it builds offline (no DATABASE_URL)**

Run: `cargo build -p api`
Expected: compiles clean. Then `cargo clippy -p api -- -D warnings` clean.

- [ ] **Step 6: Commit**

```bash
git add Cargo.toml crates/api/
git commit -m "feat(api): scaffold read-only Axum service with health route"
```

---

### Task 2: `GET /api/vault` (predict_latest)

**Files:**
- Create: `crates/api/src/routes/mod.rs`
- Create: `crates/api/src/routes/vault.rs`
- Modify: `crates/api/src/main.rs` (mount route, declare `mod routes`)
- Test: `crates/api/tests/vault.rs`

**Interfaces:**
- Consumes: `AppState`, view `predict_latest` (columns: object_version, nav, utilization, balance, total_mtm, total_max_payout, withdrawal_available, wl_enabled, ingested_at).
- Produces: handler `vault(State<AppState>) -> Result<Json<Option<Vault>>, ApiError>`; `Vault` struct (all decoded floats; `withdrawal_available: Option<f64>` because the view emits NULL when `wl_enabled=false`).

- [ ] **Step 1: Write the failing test**

`crates/api/tests/vault.rs`:
```rust
use api::{build_router, state::AppState};
use axum::body::Body;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use sqlx::PgPool;
use tower::ServiceExt;

async fn get(pool: PgPool, uri: &str) -> (StatusCode, serde_json::Value) {
    // migrations live under the indexer crate
    sqlx::migrate!("../indexer/migrations").run(&pool).await.unwrap();
    let app = build_router(AppState { pool });
    let resp = app
        .oneshot(Request::builder().uri(uri).body(Body::empty()).unwrap())
        .await
        .unwrap();
    let status = resp.status();
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    let json = if bytes.is_empty() { serde_json::Value::Null }
               else { serde_json::from_slice(&bytes).unwrap() };
    (status, json)
}

#[sqlx::test]
#[ignore] // live: needs DATABASE_URL; run with `cargo test -p api -- --ignored`
async fn vault_empty_returns_null_not_500(pool: PgPool) {
    let (status, json) = get(pool, "/api/vault").await;
    assert_eq!(status, StatusCode::OK);
    assert!(json.is_null(), "empty predict_state must serialize to null");
}

#[sqlx::test]
#[ignore]
async fn vault_decodes_and_nulls_unlimited_withdrawal(pool: PgPool) {
    // wl_enabled=false → view emits withdrawal_available NULL (unlimited)
    sqlx::query(
        "INSERT INTO predict_state (object_version, vault_balance, vault_total_mtm, \
         vault_total_max_payout, wl_enabled, wl_available, wl_capacity, \
         wl_refill_rate_per_ms, wl_last_updated_ms) \
         VALUES (1, 2000000, 1000000, 500000, false, 0, 0, 0, 0)",
    ).execute(&pool).await.unwrap();

    let (status, json) = get(pool, "/api/vault").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(json["nav"].as_f64().unwrap(), 3.0);              // (2_000_000+1_000_000)/1e6
    assert_eq!(json["balance"].as_f64().unwrap(), 2.0);
    assert!(json["withdrawal_available"].is_null());            // unlimited
    assert_eq!(json["wl_enabled"].as_bool().unwrap(), false);
}
```

- [ ] **Step 2: Run test, expect compile failure**

Run: `cargo test -p api --test vault -- --ignored`
Expected: FAIL to compile — `build_router`/`AppState` not exported as a lib, `routes` missing.

- [ ] **Step 3: Make the crate a lib + add an `ApiError` type**

Add `crates/api/src/lib.rs` exposing the modules so tests can import `api::build_router`:
```rust
pub mod error;
pub mod routes;
pub mod state;

use axum::{routing::get, Router};
use state::AppState;

pub fn build_router(state: AppState) -> Router {
    Router::new()
        .route("/api/health", get(|| async { "ok" }))
        .route("/api/vault", get(routes::vault::vault))
        .with_state(state)
}
```
Add `[lib] path = "src/lib.rs"` to `crates/api/Cargo.toml` and have `main.rs` call `api::build_router`. Create `crates/api/src/error.rs`:
```rust
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};

pub struct ApiError(pub anyhow::Error);

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        tracing::error!(error = ?self.0, "request failed");
        (StatusCode::INTERNAL_SERVER_ERROR, "internal error").into_response()
    }
}

impl<E: Into<anyhow::Error>> From<E> for ApiError {
    fn from(e: E) -> Self { ApiError(e.into()) }
}
```

- [ ] **Step 4: Write `crates/api/src/routes/vault.rs`**

```rust
use crate::{error::ApiError, state::AppState};
use axum::{extract::State, Json};
use serde::Serialize;
use sqlx::Row;

#[derive(Serialize)]
pub struct Vault {
    pub object_version: i64,
    pub nav: f64,
    pub utilization: Option<f64>,        // view guards div-by-zero → NULL
    pub balance: f64,
    pub total_mtm: f64,
    pub total_max_payout: f64,
    pub withdrawal_available: Option<f64>, // NULL when wl_enabled=false
    pub wl_enabled: bool,
    pub ingested_at: chrono::DateTime<chrono::Utc>,
}

pub async fn vault(State(st): State<AppState>) -> Result<Json<Option<Vault>>, ApiError> {
    let row = sqlx::query(
        "SELECT object_version, nav, utilization, balance, total_mtm, \
         total_max_payout, withdrawal_available, wl_enabled, ingested_at \
         FROM predict_latest",
    )
    .fetch_optional(&st.pool)
    .await?;

    let v = row.map(|r| Vault {
        object_version: r.get("object_version"),
        nav: r.get("nav"),
        utilization: r.get("utilization"),
        balance: r.get("balance"),
        total_mtm: r.get("total_mtm"),
        total_max_payout: r.get("total_max_payout"),
        withdrawal_available: r.get("withdrawal_available"),
        wl_enabled: r.get("wl_enabled"),
        ingested_at: r.get("ingested_at"),
    });
    Ok(Json(v))
}
```
Add `chrono = { version = "0.4", features = ["serde"] }` and the sqlx `chrono` feature to `crates/api/Cargo.toml` deps. Add `crates/api/src/routes/mod.rs` with `pub mod vault;`.

- [ ] **Step 5: Run the tests against a live DB, expect PASS**

Run: `cargo test -p api --test vault -- --ignored`
Expected: both tests PASS. Then `cargo test -p api` (no `--ignored`) → 0 run, 2 ignored. `cargo clippy -p api -- -D warnings` clean.

- [ ] **Step 6: Commit**

```bash
git add crates/api/
git commit -m "feat(api): GET /api/vault over predict_latest (null-on-empty, unlimited withdrawal)"
```

---

### Task 3: `GET /api/oracles` (oracle_latest)

**Files:**
- Create: `crates/api/src/routes/oracles.rs`
- Modify: `crates/api/src/routes/mod.rs`, `crates/api/src/lib.rs` (mount route)
- Test: `crates/api/tests/oracles.rs`

**Interfaces:**
- Consumes: view `oracle_latest` (FULL OUTER JOIN of svi_update + prices_update). Columns: oracle_id, a, b, rho, m, sigma, svi_sanity, svi_checkpoint_seq, spot, forward, prices_checkpoint_seq.
- Produces: handler `oracles(State<AppState>) -> Result<Json<Vec<Oracle>>, ApiError>`. **Every joined column is `Option`**: a prices-only oracle has all SVI fields + svi_sanity NULL; an svi-only oracle has spot/forward/prices_checkpoint_seq NULL.

- [ ] **Step 1: Write the failing test**

`crates/api/tests/oracles.rs` (reuse the `get` helper pattern from Task 2 — repeat it here, do not import from the other test file):
```rust
use api::{build_router, state::AppState};
use axum::body::Body;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use sqlx::PgPool;
use tower::ServiceExt;

async fn get_json(pool: PgPool, uri: &str) -> (StatusCode, serde_json::Value) {
    sqlx::migrate!("../indexer/migrations").run(&pool).await.unwrap();
    let app = build_router(AppState { pool });
    let resp = app.oneshot(Request::builder().uri(uri).body(Body::empty()).unwrap()).await.unwrap();
    let status = resp.status();
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    (status, serde_json::from_slice(&bytes).unwrap())
}

#[sqlx::test]
#[ignore]
async fn prices_only_oracle_has_null_svi(pool: PgPool) {
    sqlx::query(
        "INSERT INTO prices_update (tx_digest, event_index, checkpoint_seq, oracle_id, spot, forward, ts_chain_ms) \
         VALUES ('tx1', 0, 100, '0xaaa', 63000000000000, 63010000000000, 1)",
    ).execute(&pool).await.unwrap();

    let (status, json) = get_json(pool, "/api/oracles").await;
    assert_eq!(status, StatusCode::OK);
    let row = &json.as_array().unwrap()[0];
    assert_eq!(row["oracle_id"], "0xaaa");
    assert_eq!(row["spot"].as_f64().unwrap(), 63000.0); // /1e9
    assert!(row["a"].is_null());                        // no SVI yet
    assert!(row["svi_sanity"].is_null());
}

#[sqlx::test]
#[ignore]
async fn svi_only_oracle_has_null_prices_and_keeps_sign(pool: PgPool) {
    // rho stored signed (negative) in raw NUMERIC; view divides by 1e9 preserving sign
    sqlx::query(
        "INSERT INTO svi_update (tx_digest, event_index, checkpoint_seq, oracle_id, a, b, sigma, rho, m, ts_chain_ms, sanity) \
         VALUES ('tx2', 0, 101, '0xbbb', 7000, 190000, 1000, -400000000, -450000, 1, 'clean')",
    ).execute(&pool).await.unwrap();

    let (_status, json) = get_json(pool, "/api/oracles").await;
    let row = &json.as_array().unwrap()[0];
    assert_eq!(row["oracle_id"], "0xbbb");
    assert!(row["spot"].is_null());                       // prices-only columns NULL
    assert!(row["rho"].as_f64().unwrap() < 0.0);          // sign preserved
    assert_eq!(row["svi_sanity"], "clean");
}
```

- [ ] **Step 2: Run, expect fail (route not mounted)**

Run: `cargo test -p api --test oracles -- --ignored`
Expected: FAIL — `/api/oracles` returns 404 / handler missing.

- [ ] **Step 3: Write `crates/api/src/routes/oracles.rs`**

```rust
use crate::{error::ApiError, state::AppState};
use axum::{extract::State, Json};
use serde::Serialize;
use sqlx::Row;

#[derive(Serialize)]
pub struct Oracle {
    pub oracle_id: String,
    pub a: Option<f64>,
    pub b: Option<f64>,
    pub rho: Option<f64>,
    pub m: Option<f64>,
    pub sigma: Option<f64>,
    pub svi_sanity: Option<String>,
    pub svi_checkpoint_seq: Option<i64>,
    pub spot: Option<f64>,
    pub forward: Option<f64>,
    pub prices_checkpoint_seq: Option<i64>,
}

pub async fn oracles(State(st): State<AppState>) -> Result<Json<Vec<Oracle>>, ApiError> {
    let rows = sqlx::query(
        "SELECT oracle_id, a, b, rho, m, sigma, svi_sanity, svi_checkpoint_seq, \
         spot, forward, prices_checkpoint_seq FROM oracle_latest ORDER BY oracle_id",
    )
    .fetch_all(&st.pool)
    .await?;

    let out = rows.into_iter().map(|r| Oracle {
        oracle_id: r.get("oracle_id"),
        a: r.get("a"),
        b: r.get("b"),
        rho: r.get("rho"),
        m: r.get("m"),
        sigma: r.get("sigma"),
        svi_sanity: r.get("svi_sanity"),
        svi_checkpoint_seq: r.get("svi_checkpoint_seq"),
        spot: r.get("spot"),
        forward: r.get("forward"),
        prices_checkpoint_seq: r.get("prices_checkpoint_seq"),
    }).collect();
    Ok(Json(out))
}
```
Mount in `lib.rs`: `.route("/api/oracles", get(routes::oracles::oracles))`. Add `pub mod oracles;` to `routes/mod.rs`.

- [ ] **Step 4: Run, expect PASS**

Run: `cargo test -p api --test oracles -- --ignored`
Expected: both PASS. `cargo clippy -p api -- -D warnings` clean.

- [ ] **Step 5: Commit**

```bash
git add crates/api/
git commit -m "feat(api): GET /api/oracles over oracle_latest (full-outer-join nullability, signed rho)"
```

---

### Task 4: `GET /api/inventory` (strike_matrix_latest)

**Files:**
- Create: `crates/api/src/routes/inventory.rs`
- Modify: `crates/api/src/routes/mod.rs`, `crates/api/src/lib.rs`
- Test: `crates/api/tests/inventory.rs`

**Interfaces:**
- Consumes: view `strike_matrix_latest`. Columns: matrix_object_id, oracle_id, matrix_version, mtm (f64), range_qty (raw NUMERIC), min_strike/max_strike/tick_size (f64), minted_min_strike/minted_max_strike (f64 or NULL when u64::MAX sentinel), page_leaves (JSONB), ingested_at.
- Produces: handler `inventory(...) -> Result<Json<Vec<Matrix>>, ApiError>`. `range_qty: String` (raw, scale unverified — never f64); `page_leaves: serde_json::Value` (passthrough); `minted_*: Option<f64>`.

- [ ] **Step 1: Write the failing test**

`crates/api/tests/inventory.rs` (repeat the `get_json` helper):
```rust
use api::{build_router, state::AppState};
use axum::body::Body;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use sqlx::PgPool;
use tower::ServiceExt;

async fn get_json(pool: PgPool, uri: &str) -> (StatusCode, serde_json::Value) {
    sqlx::migrate!("../indexer/migrations").run(&pool).await.unwrap();
    let app = build_router(AppState { pool });
    let resp = app.oneshot(Request::builder().uri(uri).body(Body::empty()).unwrap()).await.unwrap();
    let status = resp.status();
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    (status, serde_json::from_slice(&bytes).unwrap())
}

async fn seed_matrix(pool: &PgPool, id: &str, minted_min: &str) {
    sqlx::query(
        "INSERT INTO strike_matrix_state (matrix_object_id, oracle_id, matrix_version, mtm, \
         range_qty, min_strike, max_strike, minted_min_strike, minted_max_strike, tick_size, page_leaves) \
         VALUES ($1, '0xorc', 7, 1000000, 18446744073709551615, 50000000000000, 150000000000000, \
         $2, 140000000000000, 1000000000, '[{\"q_up\":\"123\",\"q_dn\":\"45\"}]'::jsonb)",
    ).bind(id).bind(minted_min).execute(pool).await.unwrap();
    sqlx::query(
        "INSERT INTO oracle_matrix_listing (matrix_object_id, oracle_id, last_version) VALUES ($1, '0xorc', 7)",
    ).bind(id).execute(pool).await.unwrap();
}

#[sqlx::test]
#[ignore]
async fn inventory_keeps_range_qty_raw_and_passes_page_leaves(pool: PgPool) {
    seed_matrix(&pool, "0xm1", "60000000000000").await;
    let (status, json) = get_json(pool, "/api/inventory").await;
    assert_eq!(status, StatusCode::OK);
    let row = &json.as_array().unwrap()[0];
    // range_qty is the raw u64 18446744073709551615 — MUST be a string, not a lossy float
    assert_eq!(row["range_qty"].as_str().unwrap(), "18446744073709551615");
    assert_eq!(row["min_strike"].as_f64().unwrap(), 50000.0);       // /1e9
    assert_eq!(row["page_leaves"][0]["q_up"].as_str().unwrap(), "123");
    assert_eq!(row["minted_min_strike"].as_f64().unwrap(), 60000.0);
}

#[sqlx::test]
#[ignore]
async fn inventory_nulls_minted_when_sentinel(pool: PgPool) {
    // minted_min_strike = u64::MAX sentinel → view maps to NULL ("none minted")
    seed_matrix(&pool, "0xm2", "18446744073709551615").await;
    let (_s, json) = get_json(pool, "/api/inventory").await;
    let row = &json.as_array().unwrap()[0];
    assert!(row["minted_min_strike"].is_null());
}
```

- [ ] **Step 2: Run, expect fail**

Run: `cargo test -p api --test inventory -- --ignored`
Expected: FAIL — `/api/inventory` 404.

- [ ] **Step 3: Write `crates/api/src/routes/inventory.rs`**

```rust
use crate::{error::ApiError, state::AppState};
use axum::{extract::State, Json};
use serde::Serialize;
use sqlx::Row;

#[derive(Serialize)]
pub struct Matrix {
    pub matrix_object_id: String,
    pub oracle_id: String,
    pub matrix_version: i64,
    pub mtm: f64,
    pub range_qty: String,                  // raw u64, scale unverified — NOT f64
    pub min_strike: f64,
    pub max_strike: f64,
    pub tick_size: f64,
    pub minted_min_strike: Option<f64>,     // NULL = none minted
    pub minted_max_strike: Option<f64>,
    pub page_leaves: serde_json::Value,     // passthrough (raw u64 strings inside)
    pub ingested_at: chrono::DateTime<chrono::Utc>,
}

pub async fn inventory(State(st): State<AppState>) -> Result<Json<Vec<Matrix>>, ApiError> {
    let rows = sqlx::query(
        "SELECT matrix_object_id, oracle_id, matrix_version, mtm, range_qty::text AS range_qty, \
         min_strike, max_strike, tick_size, minted_min_strike, minted_max_strike, \
         page_leaves, ingested_at FROM strike_matrix_latest ORDER BY oracle_id, matrix_object_id",
    )
    .fetch_all(&st.pool)
    .await?;

    let out = rows.into_iter().map(|r| Matrix {
        matrix_object_id: r.get("matrix_object_id"),
        oracle_id: r.get("oracle_id"),
        matrix_version: r.get("matrix_version"),
        mtm: r.get("mtm"),
        range_qty: r.get("range_qty"),
        min_strike: r.get("min_strike"),
        max_strike: r.get("max_strike"),
        tick_size: r.get("tick_size"),
        minted_min_strike: r.get("minted_min_strike"),
        minted_max_strike: r.get("minted_max_strike"),
        page_leaves: r.get("page_leaves"),
        ingested_at: r.get("ingested_at"),
    }).collect();
    Ok(Json(out))
}
```
Note: `range_qty::text` casts the raw NUMERIC to a string in SQL so sqlx reads it as `String` (avoids any decimal-crate dep). Mount `.route("/api/inventory", get(routes::inventory::inventory))`; add `pub mod inventory;`.

- [ ] **Step 4: Run, expect PASS**

Run: `cargo test -p api --test inventory -- --ignored`
Expected: both PASS. `cargo clippy -p api -- -D warnings` clean.

- [ ] **Step 5: Commit**

```bash
git add crates/api/
git commit -m "feat(api): GET /api/inventory (range_qty raw string, minted NULL sentinel, page_leaves passthrough)"
```

---

### Task 5: Static SPA serving + route precedence + CORS

**Files:**
- Modify: `crates/api/src/lib.rs` (router composition), `crates/api/src/main.rs`
- Modify: `crates/api/Cargo.toml` (already has tower-http fs+cors)
- Test: `crates/api/tests/static_routes.rs`

**Interfaces:**
- Consumes: env `WEB_DIST` (default `web/dist`), env `CORS_DEV` (`"1"` opens permissive CORS for localhost dev).
- Produces: `build_router` mounts `/api/*` FIRST, then `ServeDir` for static, then SPA fallback to `index.html`. `/api` is never shadowed.

- [ ] **Step 1: Write the failing test (route precedence)**

`crates/api/tests/static_routes.rs`:
```rust
use api::{build_router, state::AppState};
use axum::body::Body;
use axum::http::{Request, StatusCode};
use sqlx::PgPool;
use tower::ServiceExt;

#[sqlx::test]
#[ignore]
async fn api_health_not_shadowed_by_static(pool: PgPool) {
    let app = build_router(AppState { pool });
    let resp = app
        .oneshot(Request::builder().uri("/api/health").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK); // not intercepted by ServeDir/fallback
}
```

- [ ] **Step 2: Run, expect pass-or-fail depending on current router**

Run: `cargo test -p api --test static_routes -- --ignored`
Expected: it compiles and passes only after Step 3 wires the static layer without shadowing `/api`. Before Step 3 it should still pass (no static yet) — so first change `build_router` to add static, watch it stay green.

- [ ] **Step 3: Update `build_router` to add static + fallback + optional CORS**

```rust
use tower_http::cors::{Any, CorsLayer};
use tower_http::services::{ServeDir, ServeFile};
use std::env;

pub fn build_router(state: AppState) -> Router {
    let api = Router::new()
        .route("/api/health", get(|| async { "ok" }))
        .route("/api/vault", get(routes::vault::vault))
        .route("/api/oracles", get(routes::oracles::oracles))
        .route("/api/inventory", get(routes::inventory::inventory))
        .with_state(state);

    let web_dist = env::var("WEB_DIST").unwrap_or_else(|_| "web/dist".to_string());
    let index = format!("{web_dist}/index.html");
    // ServeDir serves files; unmatched client routes fall back to index.html.
    let static_svc = ServeDir::new(&web_dist).fallback(ServeFile::new(index));

    let mut app = api.fallback_service(static_svc);
    if env::var("CORS_DEV").as_deref() == Ok("1") {
        app = app.layer(CorsLayer::new().allow_origin(Any).allow_methods(Any).allow_headers(Any));
    }
    app
}
```
`/api/*` routes are matched before `fallback_service`, so they cannot be shadowed. Update `main.rs` to drop its local `build_router` (now in lib).

- [ ] **Step 4: Run, expect PASS**

Run: `cargo test -p api --test static_routes -- --ignored` → PASS. `cargo build --workspace` (no DATABASE_URL) compiles. `cargo test --workspace` → API integration tests show as `ignored`. `cargo clippy --all-targets -- -D warnings` clean.

- [ ] **Step 5: Commit**

```bash
git add crates/api/
git commit -m "feat(api): serve web/dist with SPA fallback, /api precedence, dev CORS"
```

---

### Task 6: Scaffold `web/` (Vite + React + TS + Tailwind + theme + Vitest)

**Files:**
- Create: `web/package.json`, `web/vite.config.ts`, `web/tsconfig.json`, `web/index.html`
- Create: `web/tailwind.config.ts`, `web/postcss.config.js`
- Create: `web/src/main.tsx`, `web/src/index.css` (theme tokens), `web/src/theme.ts`
- Create: `web/src/setupTests.ts`, `web/src/theme.test.ts`
- Modify: `.gitignore` (ensure `web/node_modules`, `web/dist` ignored)

**Interfaces:**
- Produces: a buildable SPA shell; `web/src/theme.ts` exporting the design tokens (color hex map + staleness constant) consumed by all components and tests.

- [ ] **Step 1: Init Vite React-TS + deps**

```bash
cd web
npm create vite@latest . -- --template react-ts   # accept overwrite into empty dir
npm install
npm install @tanstack/react-query recharts
npm install -D tailwindcss@^3 postcss autoprefixer vitest @testing-library/react @testing-library/jest-dom jsdom
```

- [ ] **Step 2: Configure Tailwind + theme CSS variables**

`web/tailwind.config.ts` maps semantic names to CSS vars (so components never use raw `green-500`):
```ts
import type { Config } from "tailwindcss";
export default {
  content: ["./index.html", "./src/**/*.{ts,tsx}"],
  theme: { extend: { colors: {
    abyss: { 900: "var(--abyss-900)", 800: "var(--abyss-800)", 700: "var(--abyss-700)", 600: "var(--abyss-600)" },
    ink: { 200: "var(--ink-200)", 400: "var(--ink-400)", 600: "var(--ink-600)" },
    sonar: { DEFAULT: "var(--sonar)", dim: "var(--sonar-dim)" },
    ok: "var(--ok)", warn: "var(--warn)", alert: "var(--alert)",
    up: "var(--up)", dn: "var(--dn)",
  }, fontFamily: { mono: ["IBM Plex Mono", "monospace"], sans: ["Inter Tight", "IBM Plex Sans", "sans-serif"] } } },
  plugins: [],
} satisfies Config;
```
`web/src/index.css`:
```css
@import url('https://fonts.googleapis.com/css2?family=IBM+Plex+Mono:wght@400;500;600&family=Inter+Tight:wght@400;500;600&display=swap');
@tailwind base; @tailwind components; @tailwind utilities;
:root {
  --abyss-900:#0A0E14; --abyss-800:#0E141C; --abyss-700:#131C26; --abyss-600:#1B2733;
  --ink-200:#C4D0DC; --ink-400:#8295A6; --ink-600:#4A5A6A;
  --sonar:#38E1C4; --sonar-dim:#1B6E63;
  --ok:#3FB68B; --warn:#D9A441; --alert:#E5484D; --up:#2DD4BF; --dn:#E0719B;
}
body { background:var(--abyss-900); color:var(--ink-200);
  background-image: radial-gradient(60% 40% at 50% 0%, rgba(56,225,196,0.03), transparent 70%); }
.tnum { font-variant-numeric: tabular-nums; }
```
`postcss.config.js`: `export default { plugins: { tailwindcss: {}, autoprefixer: {} } };`

- [ ] **Step 3: Write `web/src/theme.ts` + its test (TDD the contract)**

`web/src/theme.ts`:
```ts
// Single source for design constants. Components import from here; no inline hex.
export const STALE_MS = 30_000;     // 3x poller interval (10s); rationale: see spec.
export const POLL_MS = 10_000;
export const SANITY = { clean: "ok", dirty: "alert", untested: "warn" } as const;
export function staleness(ingestedAtIso: string, now: number): "fresh" | "warn" | "alert" {
  const age = now - new Date(ingestedAtIso).getTime();
  if (age > 5 * 60_000) return "alert";
  if (age > STALE_MS) return "warn";
  return "fresh";
}
```
`web/src/theme.test.ts`:
```ts
import { describe, it, expect } from "vitest";
import { staleness, STALE_MS } from "./theme";
describe("staleness", () => {
  const t0 = new Date("2026-06-22T00:00:00Z").getTime();
  it("fresh within threshold", () => {
    expect(staleness("2026-06-22T00:00:00Z", t0 + STALE_MS - 1)).toBe("fresh");
  });
  it("warn past 30s", () => {
    expect(staleness("2026-06-22T00:00:00Z", t0 + STALE_MS + 1)).toBe("warn");
  });
  it("alert past 5min", () => {
    expect(staleness("2026-06-22T00:00:00Z", t0 + 5 * 60_000 + 1)).toBe("alert");
  });
});
```
Add to `package.json` scripts: `"test": "vitest run"`, and `vitest` config in `vite.config.ts`: `test: { environment: "jsdom", setupFiles: ["./src/setupTests.ts"] }`. `web/src/setupTests.ts`: `import "@testing-library/jest-dom";`.

- [ ] **Step 4: Run the theme test**

Run: `cd web && npm run test`
Expected: 3 tests PASS.

- [ ] **Step 5: Verify build + gitignore**

Run: `cd web && npm run build` → produces `web/dist`. Confirm `.gitignore` has `web/node_modules` and `web/dist` (add if missing).

- [ ] **Step 6: Commit**

```bash
git add web/ .gitignore
git commit -m "feat(web): scaffold Vite+React+Tailwind, Fathom theme tokens, vitest"
```

---

### Task 7: `api.ts` — typed fetch + TanStack Query

**Files:**
- Create: `web/src/api.ts`
- Test: `web/src/api.test.ts`
- Modify: `web/src/main.tsx` (QueryClientProvider)

**Interfaces:**
- Consumes: endpoints `/api/vault`, `/api/oracles`, `/api/inventory`.
- Produces: TS types `Vault`, `Oracle`, `Matrix` (mirroring the Rust structs — `range_qty: string`, `withdrawal_available: number | null`, SVI fields `number | null`); hooks `useVault()`, `useOracles()`, `useInventory()` (all `refetchInterval: POLL_MS`, aligned); `fetchJson<T>(path): Promise<T>` that throws on non-2xx (Rule 12: surface, don't swallow).

- [ ] **Step 1: Write the failing test**

`web/src/api.test.ts`:
```ts
import { describe, it, expect, vi, afterEach } from "vitest";
import { fetchJson } from "./api";

afterEach(() => vi.restoreAllMocks());

describe("fetchJson", () => {
  it("returns parsed json on 200", async () => {
    vi.stubGlobal("fetch", vi.fn(async () =>
      new Response(JSON.stringify({ nav: 3 }), { status: 200 })));
    expect(await fetchJson<{ nav: number }>("/api/vault")).toEqual({ nav: 3 });
  });
  it("throws on 500 (does not swallow)", async () => {
    vi.stubGlobal("fetch", vi.fn(async () => new Response("internal error", { status: 500 })));
    await expect(fetchJson("/api/vault")).rejects.toThrow(/500/);
  });
  it("returns null body as null (empty vault)", async () => {
    vi.stubGlobal("fetch", vi.fn(async () => new Response("null", { status: 200 })));
    expect(await fetchJson("/api/vault")).toBeNull();
  });
});
```

- [ ] **Step 2: Run, expect fail**

Run: `cd web && npm run test -- api`
Expected: FAIL — `./api` has no `fetchJson`.

- [ ] **Step 3: Write `web/src/api.ts`**

```ts
import { useQuery } from "@tanstack/react-query";
import { POLL_MS } from "./theme";

export type Vault = {
  object_version: number; nav: number; utilization: number | null;
  balance: number; total_mtm: number; total_max_payout: number;
  withdrawal_available: number | null; wl_enabled: boolean; ingested_at: string;
} | null;

export type Oracle = {
  oracle_id: string;
  a: number | null; b: number | null; rho: number | null; m: number | null; sigma: number | null;
  svi_sanity: "clean" | "dirty" | "untested" | null;
  svi_checkpoint_seq: number | null;
  spot: number | null; forward: number | null; prices_checkpoint_seq: number | null;
};

export type Matrix = {
  matrix_object_id: string; oracle_id: string; matrix_version: number;
  mtm: number; range_qty: string; min_strike: number; max_strike: number; tick_size: number;
  minted_min_strike: number | null; minted_max_strike: number | null;
  page_leaves: { q_up: string; q_dn: string }[]; ingested_at: string;
};

export async function fetchJson<T>(path: string): Promise<T> {
  const r = await fetch(path);
  if (!r.ok) throw new Error(`${path} → HTTP ${r.status}`);
  return (await r.json()) as T;
}

const opts = { refetchInterval: POLL_MS, staleTime: POLL_MS } as const;
export const useVault = () => useQuery({ queryKey: ["vault"], queryFn: () => fetchJson<Vault>("/api/vault"), ...opts });
export const useOracles = () => useQuery({ queryKey: ["oracles"], queryFn: () => fetchJson<Oracle[]>("/api/oracles"), ...opts });
export const useInventory = () => useQuery({ queryKey: ["inventory"], queryFn: () => fetchJson<Matrix[]>("/api/inventory"), ...opts });
```
Wrap `<App/>` in `main.tsx` with `<QueryClientProvider client={new QueryClient()}>`.

- [ ] **Step 4: Run, expect PASS**

Run: `cd web && npm run test -- api` → 3 PASS.

- [ ] **Step 5: Commit**

```bash
git add web/src/api.ts web/src/api.test.ts web/src/main.tsx
git commit -m "feat(web): typed api client + TanStack Query hooks (throws on error, range_qty as string)"
```

---

### Task 8: `VaultHealth` component (depth-bars, NAV dominant, staleness)

**Files:**
- Create: `web/src/components/VaultHealth.tsx`
- Create: `web/src/components/ui/DepthBar.tsx`
- Test: `web/src/components/VaultHealth.test.tsx`

**Interfaces:**
- Consumes: `Vault` type, `staleness()`.
- Produces: `<VaultHealth vault={Vault} now={number} />`. Renders "Unlimited" when `wl_enabled=false`, "—" for null fields, warn/alert border driven by `staleness`.

- [ ] **Step 1: Write the failing test**

`web/src/components/VaultHealth.test.tsx`:
```tsx
import { describe, it, expect } from "vitest";
import { render, screen } from "@testing-library/react";
import { VaultHealth } from "./VaultHealth";

const base = { object_version: 1, nav: 1019403, utilization: 0.0017, balance: 1017923,
  total_mtm: 1481, total_max_payout: 1692, withdrawal_available: null,
  wl_enabled: false, ingested_at: "2026-06-22T00:00:00Z" };

describe("VaultHealth", () => {
  it("shows Unlimited when withdrawal limiter disabled", () => {
    render(<VaultHealth vault={base} now={new Date(base.ingested_at).getTime()} />);
    expect(screen.getByText(/unlimited/i)).toBeInTheDocument();
  });
  it("flags stale data past threshold", () => {
    const now = new Date(base.ingested_at).getTime() + 31_000;
    const { container } = render(<VaultHealth vault={base} now={now} />);
    expect(container.querySelector("[data-stale='warn']")).toBeTruthy();
  });
  it("renders NAV figure", () => {
    render(<VaultHealth vault={base} now={new Date(base.ingested_at).getTime()} />);
    expect(screen.getByText(/1,019,403/)).toBeInTheDocument();
  });
});
```

- [ ] **Step 2: Run, expect fail**

Run: `cd web && npm run test -- VaultHealth`
Expected: FAIL — component missing.

- [ ] **Step 3: Implement `DepthBar.tsx` then `VaultHealth.tsx`**

`ui/DepthBar.tsx` — linear bar, fill color crosses ok→warn→alert at thresholds (no dial):
```tsx
export function DepthBar({ pct, danger = 0.8 }: { pct: number; danger?: number }) {
  const clamped = Math.max(0, Math.min(1, pct));
  const color = clamped >= danger ? "var(--alert)" : clamped >= danger * 0.6 ? "var(--warn)" : "var(--ok)";
  return (
    <div className="h-1.5 w-full bg-abyss-700 relative">
      <div className="h-full" style={{ width: `${clamped * 100}%`, background: color }} />
      <div className="absolute top-0 h-full w-px bg-ink-600" style={{ left: `${danger * 100}%` }} />
    </div>
  );
}
```
`VaultHealth.tsx`:
```tsx
import type { Vault } from "../api";
import { staleness } from "../theme";
import { DepthBar } from "./ui/DepthBar";

const fmt = (n: number) => n.toLocaleString("en-US", { maximumFractionDigits: 0 });

export function VaultHealth({ vault, now }: { vault: NonNullable<Vault>; now: number }) {
  const stale = staleness(vault.ingested_at, now);
  const borderTop = stale === "alert" ? "border-t-alert" : stale === "warn" ? "border-t-warn" : "border-t-transparent";
  return (
    <section data-stale={stale} className={`grid grid-cols-4 gap-px bg-abyss-600 border-t ${borderTop}`}>
      <div className="col-span-2 bg-abyss-800 p-5">
        <div className="text-ink-400 text-xs tracking-widest uppercase">NAV (DUSDC)</div>
        <div className="font-mono tnum text-5xl text-ink-200">{fmt(vault.nav)}</div>
        <div className="h-px bg-sonar mt-2 w-24" />
      </div>
      <div className="bg-abyss-800 p-5">
        <div className="text-ink-400 text-xs tracking-widest uppercase">Utilization</div>
        <div className="font-mono tnum text-2xl">{vault.utilization == null ? "—" : `${(vault.utilization * 100).toFixed(2)}%`}</div>
        <div className="mt-3"><DepthBar pct={vault.utilization ?? 0} /></div>
      </div>
      <div className="bg-abyss-800 p-5">
        <div className="text-ink-400 text-xs tracking-widest uppercase">Withdrawal</div>
        <div className="font-mono tnum text-2xl">
          {vault.wl_enabled ? (vault.withdrawal_available == null ? "—" : fmt(vault.withdrawal_available)) : "Unlimited"}
        </div>
        <div className="mt-3">
          {vault.wl_enabled ? <DepthBar pct={0.5} /> : <div className="h-1.5 w-full bg-sonar" />}
        </div>
      </div>
    </section>
  );
}
```

- [ ] **Step 4: Run, expect PASS**

Run: `cd web && npm run test -- VaultHealth` → 3 PASS.

- [ ] **Step 5: Commit**

```bash
git add web/src/components/VaultHealth.tsx web/src/components/ui/DepthBar.tsx web/src/components/VaultHealth.test.tsx
git commit -m "feat(web): VaultHealth with depth-bars, NAV headline, staleness border"
```

---

### Task 9: `OracleTable` component (square sanity chips, dirty-row highlight)

**Files:**
- Create: `web/src/components/OracleTable.tsx`
- Create: `web/src/components/ui/SanityChip.tsx`
- Test: `web/src/components/OracleTable.test.tsx`

**Interfaces:**
- Consumes: `Oracle[]`.
- Produces: `<OracleTable oracles={Oracle[]} />`. Sanity chip labels CLEAN/DIRTY/UNTESTED/PRICES-ONLY (null sanity = PRICES-ONLY). Dirty rows get `data-dirty="true"` + alert left-border styling.

- [ ] **Step 1: Write the failing test**

`web/src/components/OracleTable.test.tsx`:
```tsx
import { describe, it, expect } from "vitest";
import { render, screen } from "@testing-library/react";
import { OracleTable } from "./OracleTable";
import type { Oracle } from "../api";

const dirty: Oracle = { oracle_id: "0xdirty", a: 1, b: 2, rho: -0.3, m: -0.001, sigma: 0.5,
  svi_sanity: "dirty", svi_checkpoint_seq: 9, spot: 63000, forward: 63010, prices_checkpoint_seq: 9 };
const pricesOnly: Oracle = { oracle_id: "0xponly", a: null, b: null, rho: null, m: null, sigma: null,
  svi_sanity: null, svi_checkpoint_seq: null, spot: 63000, forward: 63010, prices_checkpoint_seq: 9 };

describe("OracleTable", () => {
  it("labels null sanity as PRICES-ONLY", () => {
    render(<OracleTable oracles={[pricesOnly]} />);
    expect(screen.getByText("PRICES-ONLY")).toBeInTheDocument();
  });
  it("marks dirty rows for highlight (the README selling point)", () => {
    const { container } = render(<OracleTable oracles={[dirty]} />);
    expect(container.querySelector("[data-dirty='true']")).toBeTruthy();
    expect(screen.getByText("DIRTY")).toBeInTheDocument();
  });
  it("renders — for null SVI params instead of crashing", () => {
    render(<OracleTable oracles={[pricesOnly]} />);
    expect(screen.getAllByText("—").length).toBeGreaterThan(0);
  });
});
```

- [ ] **Step 2: Run, expect fail**

Run: `cd web && npm run test -- OracleTable` → FAIL (missing component).

- [ ] **Step 3: Implement `SanityChip.tsx` + `OracleTable.tsx`**

`ui/SanityChip.tsx` — square LED chip (not a pill):
```tsx
export function SanityChip({ sanity }: { sanity: "clean" | "dirty" | "untested" | null }) {
  const label = sanity == null ? "PRICES-ONLY" : sanity.toUpperCase();
  const color = sanity === "clean" ? "var(--ok)" : sanity === "dirty" ? "var(--alert)"
    : sanity === "untested" ? "var(--warn)" : "var(--ink-600)";
  return (
    <span className="inline-flex items-center gap-2 font-mono text-xs tracking-wide">
      <span className="inline-block h-2 w-2" style={{ background: color, borderRadius: 0 }} />
      {label}
    </span>
  );
}
```
`OracleTable.tsx`:
```tsx
import type { Oracle } from "../api";
import { SanityChip } from "./ui/SanityChip";

const n = (v: number | null, d = 4) => (v == null ? "—" : v.toFixed(d));
const short = (id: string) => `${id.slice(0, 6)}…${id.slice(-4)}`;

export function OracleTable({ oracles }: { oracles: Oracle[] }) {
  return (
    <table className="w-full border-collapse font-mono text-sm">
      <thead>
        <tr className="text-ink-400 text-xs uppercase tracking-wider">
          {["Oracle", "Sanity", "Spot", "Forward", "a", "b", "rho", "m", "sigma"].map((h) => (
            <th key={h} className="text-right p-2 first:text-left">{h}</th>
          ))}
        </tr>
      </thead>
      <tbody>
        {oracles.map((o) => {
          const isDirty = o.svi_sanity === "dirty";
          return (
            <tr key={o.oracle_id} data-dirty={isDirty}
                className={isDirty ? "border-l-2 border-l-alert bg-[rgba(229,72,77,0.06)] animate-pulse-slow" : "border-l-2 border-l-transparent"}>
              <td className="p-2 text-left text-ink-200">{short(o.oracle_id)}</td>
              <td className="p-2 text-left"><SanityChip sanity={o.svi_sanity} /></td>
              <td className="p-2 text-right tnum">{n(o.spot, 2)}</td>
              <td className="p-2 text-right tnum">{n(o.forward, 2)}</td>
              <td className="p-2 text-right tnum">{n(o.a)}</td>
              <td className="p-2 text-right tnum">{n(o.b)}</td>
              <td className="p-2 text-right tnum" style={{ color: o.rho != null && o.rho < 0 ? "var(--up)" : undefined }}>{n(o.rho)}</td>
              <td className="p-2 text-right tnum">{n(o.m, 6)}</td>
              <td className="p-2 text-right tnum">{n(o.sigma)}</td>
            </tr>
          );
        })}
      </tbody>
    </table>
  );
}
```
Add the slow-pulse keyframe to `index.css`:
```css
@keyframes pulse-slow { 0%,100%{box-shadow:inset 0 0 0 0 rgba(229,72,77,0)} 50%{box-shadow:inset 1px 0 0 0 rgba(229,72,77,0.6)} }
.animate-pulse-slow { animation: pulse-slow 2s ease-in-out infinite; }
```

- [ ] **Step 4: Run, expect PASS**

Run: `cd web && npm run test -- OracleTable` → 3 PASS.

- [ ] **Step 5: Commit**

```bash
git add web/src/components/OracleTable.tsx web/src/components/ui/SanityChip.tsx web/src/components/OracleTable.test.tsx web/src/index.css
git commit -m "feat(web): OracleTable with square sanity chips + dirty-row pulse highlight"
```

---

### Task 10: `InventoryHeatmap` component (two stacked bands, ATM line, minted NULL)

**Files:**
- Create: `web/src/components/InventoryHeatmap.tsx`
- Create: `web/src/lib/heatmap.ts` (pure normalize + page→strike mapping)
- Test: `web/src/lib/heatmap.test.ts`, `web/src/components/InventoryHeatmap.test.tsx`

**Interfaces:**
- Consumes: `Matrix`.
- Produces: pure `normalizeBand(values: string[]): number[]` (max-normalized 0..1, parses raw strings, tolerant of precision since relative); pure `pageStrike(i, n, min, max): number`; `<InventoryHeatmap matrix={Matrix} />` rendering q_up band over q_dn band, ATM marker, "none minted" when minted NULL.

- [ ] **Step 1: Write the failing pure-logic test**

`web/src/lib/heatmap.test.ts`:
```ts
import { describe, it, expect } from "vitest";
import { normalizeBand, pageStrike } from "./heatmap";

describe("normalizeBand", () => {
  it("max-normalizes raw u64 strings to 0..1", () => {
    expect(normalizeBand(["0", "50", "100"])).toEqual([0, 0.5, 1]);
  });
  it("all-zero band stays 0 (no divide-by-zero)", () => {
    expect(normalizeBand(["0", "0"])).toEqual([0, 0]);
  });
});
describe("pageStrike", () => {
  it("maps bucket index to strike across the range", () => {
    expect(pageStrike(0, 4, 50000, 150000)).toBe(50000);
    expect(pageStrike(3, 4, 50000, 150000)).toBe(150000);
  });
});
```

- [ ] **Step 2: Run, expect fail**

Run: `cd web && npm run test -- heatmap` → FAIL.

- [ ] **Step 3: Implement `web/src/lib/heatmap.ts`**

```ts
export function normalizeBand(values: string[]): number[] {
  const nums = values.map((v) => Number(v)); // relative use → f64 precision loss tolerable
  const max = Math.max(0, ...nums);
  return nums.map((v) => (max === 0 ? 0 : v / max));
}
export function pageStrike(i: number, n: number, min: number, max: number): number {
  if (n <= 1) return min;
  return min + (i / (n - 1)) * (max - min);
}
```

- [ ] **Step 4: Run pure test, expect PASS; then write the component test**

Run: `cd web && npm run test -- heatmap` → PASS.
`web/src/components/InventoryHeatmap.test.tsx`:
```tsx
import { describe, it, expect } from "vitest";
import { render, screen } from "@testing-library/react";
import { InventoryHeatmap } from "./InventoryHeatmap";
import type { Matrix } from "../api";

const m: Matrix = { matrix_object_id: "0xm", oracle_id: "0xo", matrix_version: 7,
  mtm: 12.3, range_qty: "18446744073709551615", min_strike: 50000, max_strike: 150000, tick_size: 1,
  minted_min_strike: null, minted_max_strike: null,
  page_leaves: [{ q_up: "10", q_dn: "0" }, { q_up: "0", q_dn: "20" }], ingested_at: "2026-06-22T00:00:00Z" };

describe("InventoryHeatmap", () => {
  it("shows 'none minted' when minted range is null", () => {
    render(<InventoryHeatmap matrix={m} />);
    expect(screen.getByText(/none minted/i)).toBeInTheDocument();
  });
  it("renders one cell per page leaf per band", () => {
    const { container } = render(<InventoryHeatmap matrix={m} />);
    expect(container.querySelectorAll("[data-band='up'] [data-cell]").length).toBe(2);
    expect(container.querySelectorAll("[data-band='dn'] [data-cell]").length).toBe(2);
  });
  it("does not crash on empty page_leaves", () => {
    render(<InventoryHeatmap matrix={{ ...m, page_leaves: [] }} />);
    expect(screen.getByText(/0xm/)).toBeInTheDocument();
  });
});
```

- [ ] **Step 5: Implement `InventoryHeatmap.tsx`, run, expect PASS**

```tsx
import type { Matrix } from "../api";
import { normalizeBand } from "../lib/heatmap";

const cellColor = (t: number, base: string) =>
  t === 0 ? "var(--abyss-800)" : `color-mix(in srgb, ${base} ${Math.round((0.2 + 0.8 * t) * 100)}%, var(--abyss-700))`;

function Band({ leaves, side }: { leaves: Matrix["page_leaves"]; side: "up" | "dn" }) {
  const norm = normalizeBand(leaves.map((l) => (side === "up" ? l.q_up : l.q_dn)));
  const base = side === "up" ? "var(--up)" : "var(--dn)";
  return (
    <div data-band={side} className="flex gap-px">
      {norm.map((t, i) => (
        <div key={i} data-cell title={`${side} ${leaves[i][side === "up" ? "q_up" : "q_dn"]}`}
             className="h-5 flex-1" style={{ background: cellColor(t, base) }} />
      ))}
    </div>
  );
}

const short = (id: string) => `${id.slice(0, 6)}…${id.slice(-4)}`;

export function InventoryHeatmap({ matrix }: { matrix: Matrix }) {
  const mintedNull = matrix.minted_min_strike == null;
  return (
    <div className="bg-abyss-800 p-4 border-t border-abyss-600">
      <div className="font-mono text-xs text-ink-400 flex gap-3 mb-3">
        <span className="text-ink-200">{short(matrix.matrix_object_id)}</span>
        <span>mtm {matrix.mtm.toFixed(2)}</span>
        <span className="text-abyss-600">|</span>
        <span>range_qty {matrix.range_qty} (raw)</span>
        <span className="text-abyss-600">|</span>
        <span>{mintedNull ? "none minted" : `minted ${matrix.minted_min_strike}–${matrix.minted_max_strike}`}</span>
      </div>
      <div className="flex flex-col gap-px">
        <Band leaves={matrix.page_leaves} side="up" />
        <Band leaves={matrix.page_leaves} side="dn" />
      </div>
      <div className="font-mono text-[10px] text-ink-600 mt-1">relative intensity (max-normalized) · {matrix.min_strike}–{matrix.max_strike}</div>
    </div>
  );
}
```
Run: `cd web && npm run test -- InventoryHeatmap` → 3 PASS.

- [ ] **Step 6: Commit**

```bash
git add web/src/components/InventoryHeatmap.tsx web/src/lib/heatmap.ts web/src/lib/heatmap.test.ts web/src/components/InventoryHeatmap.test.tsx
git commit -m "feat(web): InventoryHeatmap two-band (q_up/q_dn) with none-minted + relative-intensity label"
```

---

### Task 11: `App` shell — top bar, wiring, error banner, keep-last-on-error, tombstone drop

**Files:**
- Create: `web/src/App.tsx`
- Create: `web/src/components/TopBar.tsx`
- Test: `web/src/App.test.tsx`

**Interfaces:**
- Consumes: `useVault/useOracles/useInventory`, all components.
- Produces: the page. On query error → red banner "API unreachable" + retain last rendered data (TanStack keeps `data` from last success). On a *successful* poll with fewer matrices → they disappear (we render straight from `data`, no manual merge → tombstone drop is automatic). NAV null → "no data" placeholder.

- [ ] **Step 1: Write the failing test**

`web/src/App.test.tsx`:
```tsx
import { describe, it, expect, vi, afterEach } from "vitest";
import { render, screen, waitFor } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import App from "./App";

const wrap = () => render(
  <QueryClientProvider client={new QueryClient({ defaultOptions: { queries: { retry: false } } })}><App /></QueryClientProvider>
);
afterEach(() => vi.restoreAllMocks());

describe("App", () => {
  it("shows API-unreachable banner when fetch fails", async () => {
    vi.stubGlobal("fetch", vi.fn(async () => new Response("err", { status: 500 })));
    wrap();
    await waitFor(() => expect(screen.getByText(/api unreachable/i)).toBeInTheDocument());
  });
  it("shows no-data placeholder when vault is null", async () => {
    vi.stubGlobal("fetch", vi.fn(async (url: string) =>
      new Response(url.includes("vault") ? "null" : "[]", { status: 200 })));
    wrap();
    await waitFor(() => expect(screen.getByText(/no vault data/i)).toBeInTheDocument());
  });
});
```

- [ ] **Step 2: Run, expect fail**

Run: `cd web && npm run test -- App` → FAIL.

- [ ] **Step 3: Implement `TopBar.tsx` + `App.tsx`**

`components/TopBar.tsx`:
```tsx
export function TopBar({ live }: { live: boolean }) {
  return (
    <header className="flex items-center justify-between px-6 py-3 border-b border-abyss-600">
      <span className="font-mono tracking-[0.3em] text-ink-200">FATHOM</span>
      <span className="font-mono text-xs text-ink-400 flex items-center gap-2">
        <span className="inline-block h-2 w-2 rounded-full" style={{ background: live ? "var(--sonar)" : "var(--alert)" }} />
        live · 10s
      </span>
    </header>
  );
}
```
`App.tsx`:
```tsx
import { useVault, useOracles, useInventory } from "./api";
import { TopBar } from "./components/TopBar";
import { VaultHealth } from "./components/VaultHealth";
import { OracleTable } from "./components/OracleTable";
import { InventoryHeatmap } from "./components/InventoryHeatmap";

export default function App() {
  const vault = useVault();
  const oracles = useOracles();
  const inventory = useInventory();
  const now = Date.now();
  const anyError = vault.isError || oracles.isError || inventory.isError;

  return (
    <div className="max-w-[1400px] mx-auto">
      <TopBar live={!anyError} />
      {anyError && (
        <div className="bg-[rgba(229,72,77,0.12)] border-y border-alert text-alert font-mono text-sm px-6 py-2">
          API unreachable — showing last known data
        </div>
      )}
      <main className="p-6 space-y-6">
        {vault.data ? <VaultHealth vault={vault.data} now={now} />
                    : <div className="text-ink-400 font-mono">no vault data</div>}
        <section><OracleTable oracles={oracles.data ?? []} /></section>
        <section className="space-y-px">
          {(inventory.data ?? []).map((m) => <InventoryHeatmap key={m.matrix_object_id} matrix={m} />)}
        </section>
      </main>
    </div>
  );
}
```
Note: rendering `inventory.data` directly means a successful poll with fewer matrices drops them automatically (tombstone), while on error TanStack keeps the last `data` (retain). Replace the placeholder `App` from the Vite template.

- [ ] **Step 4: Run, expect PASS + full suite + build**

Run: `cd web && npm run test` → all PASS. `npm run build` → `web/dist` produced.

- [ ] **Step 5: Commit**

```bash
git add web/src/App.tsx web/src/components/TopBar.tsx web/src/App.test.tsx
git commit -m "feat(web): App shell — topbar, error banner (retain last), tombstone drop, no-data placeholder"
```

---

### Task 12: Live smoke + Monkey + offline gate

**Files:**
- Modify: `README.md` (run instructions, append a "Run the dashboard" section)
- Modify: `tasks/progress.md`, `tasks/lessons.md` (record findings — these are gitignored per project rules)

**Interfaces:** none (verification task).

- [ ] **Step 1: Offline gate**

Run (no `DATABASE_URL`):
```bash
cargo build --workspace
cargo test --workspace            # API integration tests show as ignored
cargo clippy --all-targets -- -D warnings
cd web && npm run test && npm run build
```
Expected: all green; API integration tests `ignored`.

- [ ] **Step 2: Live wiring**

Start the existing poller (writes PG), then the API, then serve the built SPA from the API:
```bash
# terminal 1: poller (existing)
DATABASE_URL=postgres://... RUST_LOG=info cargo run -p indexer --bin poller
# terminal 2: api serving web/dist
DATABASE_URL=postgres://... WEB_DIST=web/dist CORS_DEV=1 RUST_LOG=info cargo run -p api
```
Open `http://localhost:8080`. Expected: real testnet vault NAV/utilization, ~23 oracle rows (some PRICES-ONLY, dirty rows pulsing if any), inventory heatmaps render, figures refresh every 10s.

- [ ] **Step 3: Monkey 1 — kill PG mid-session**

Stop Postgres while the dashboard is open. Expected: API requests → 500 (check `RUST_LOG` shows the anyhow chain, not a silent swallow); frontend shows red "API unreachable" banner and **retains** the last numbers (does not blank out).

- [ ] **Step 4: Monkey 2 — malformed / extreme payload**

Temporarily point the SPA at a stub returning: a vault with null fields, an oracle array with a PRICES-ONLY entry, and a matrix with empty `page_leaves` and a giant `range_qty`. Expected: no white screen — null fields render "—", empty heatmap renders header only, big range_qty shows verbatim (string, no precision mangling).

- [ ] **Step 5: Monkey 3 — stop poller (staleness)**

Kill the poller, leave the API up. Expected: after 30s the VaultHealth row shows the `data-stale='warn'` border; after 5min, alert. Confirms staleness threshold fires (a dead poller becomes visible, per the architect finding).

- [ ] **Step 6: Record + commit docs**

Update `README.md` run section. Update `tasks/progress.md` (DONE summary, live evidence) and `tasks/lessons.md` (any gotchas). Commit only `README.md` (progress/lessons are gitignored):
```bash
git add README.md
git commit -m "docs: dashboard run instructions"
```
- [ ] **Step 7: dual-review** — per project `dual-review` skill (codex generic + project SUI/frontend rules), integrate findings, then finish the branch.

---

## Notes for the implementer

- The `get`/`get_json` helper is intentionally repeated in each API test file (tasks 2–5) — test files don't share a module here; copy it, don't refactor into a shared crate for four call sites (YAGNI).
- `sqlx::migrate!("../indexer/migrations")` runs the real view DDL against the `#[sqlx::test]` throwaway DB — this is why the tests verify the actual view decode, not a hand-mocked schema (Rule 9: test the real contract).
- If `color-mix` is unsupported in the test/jsdom path, it only affects visual rendering, not the cell-count assertions; live smoke confirms the visual.
