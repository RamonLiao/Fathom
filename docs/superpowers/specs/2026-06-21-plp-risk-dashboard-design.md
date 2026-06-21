# Fathom PLP Risk Dashboard — Design (Round 1)

_Date: 2026-06-21_
_Status: approved (brainstorming), pending implementation plan_

## Scope

This round implements the **PLP Risk Dashboard** (Fathom Idea #10): a read-only
web dashboard surfacing the three existing Postgres views produced by the B-path
poller.

**In scope:**
- Vault health KPIs (NAV, utilization, withdrawal capacity).
- Per-oracle exposure table with SVI parameters + no-arb sanity badges.
- Per-strike inventory heatmap (page-bucket concentration).
- A read-only Axum JSON API over the existing views.

**Explicitly OUT of scope this round (future rounds):**
- Surface Studio 3D vol surface (Idea #9).
- Walrus-attested risk reports.
- ±5σ stress simulator.
- WebSocket push (noted as a future upgrade; this round polls).

## Architecture

```
Postgres views ─► crates/api (Axum + sqlx, read-only JSON) ─► Vite + React + Tailwind SPA
   (3 views)         GET /api/...                TanStack Query poll 10s   Recharts + CSS heatmap
```

Two new units are added; the existing indexer/poller/migrations are **not touched**:

1. `crates/api` — new workspace member. Reuses the existing `DATABASE_URL` config
   and sqlx pool pattern. Read-only.
2. `web/` — frontend (npm; `node_modules` gitignored).

In production the Axum binary also serves the built `web/dist` static assets at
`/`, giving a single binary and avoiding CORS. In dev, CORS is opened for
localhost.

## Data sources (existing views — not modified)

| View | Columns used |
|---|---|
| `predict_latest` | object_version, nav, utilization, balance, total_mtm, total_max_payout, withdrawal_available, wl_enabled, ingested_at |
| `oracle_latest` | oracle_id, a, b, rho, m, sigma, svi_sanity, svi_checkpoint_seq, spot, forward, prices_checkpoint_seq |
| `strike_matrix_latest` | matrix_object_id, oracle_id, matrix_version, mtm, range_qty, min_strike, max_strike, tick_size, minted_min_strike, minted_max_strike, page_leaves, ingested_at |

All scale/decode lives in the SQL views (raw-as-source-of-truth). The API does
**no** business logic — it selects from a view and serializes to JSON.

## Section 1 — API (`crates/api`)

Three read-only endpoints, one per view, stateless, no join logic (the view did it):

| Endpoint | View | Returns |
|---|---|---|
| `GET /api/vault` | `predict_latest` | single object (or `null` if no data) |
| `GET /api/oracles` | `oracle_latest` | array |
| `GET /api/inventory` | `strike_matrix_latest` | array |

- sqlx `query().bind()` runtime (offline-clean, CI needs no DB), aligned with the
  existing indexer convention.
- Numeric columns are read as `f64` (view already decoded); `page_leaves` is passed
  through as `serde_json::Value`.
- Error routing (Rule 12): DB connection/query error → HTTP 500 + full log; empty
  result for `/api/vault` → HTTP 200 + `null` (a legal state, not an error).
- CORS: open to localhost in dev; same-origin in prod (Axum serves `web/dist`).

## Section 2 — Frontend (`web/`)

Stack: Vite + React + TypeScript + Tailwind + TanStack Query + Recharts. Single-page
dashboard, three regions.

### ① Vault Health (top KPI row)
- NAV, Utilization %, Withdrawal capacity as large stat cards with small gauges.
- Utilization gauge color-graded (green/yellow/red).
- `wl_enabled = false` → withdrawal shows "Unlimited" (matches the view's NULL).
- `ingested_at` shown as "as of Xs ago"; goes stale/grey when > 60s old.

### ② Per-Oracle Exposure (mid table)
- One row per oracle: oracle_id (truncated + copy), spot, forward, SVI params
  (a/b/rho/m/sigma), and a **sanity badge** (clean=green / dirty=red / untested=grey /
  null="prices-only").
- Dirty rows are highlighted — the core README selling point (arb violations visible).
- Optional sorting (by sanity or exposure contribution).

### ③ Inventory Heatmap (bottom, per-oracle expandable)
- Select an oracle → render its `page_leaves` (N page buckets) as a CSS-grid heatmap.
- Each cell's color intensity = `q_up` (call side) / `q_dn` (put side) **relative**
  strength (max-normalized per matrix). Two rows (up/down) or a diverging scale.
- X-axis maps page → strike using `min_strike` / `max_strike` / `tick_size` (the view
  explicitly leaves this mapping to the frontend).
- Hover tooltip shows raw q_up/q_dn + the strike sub-range.
- mtm / range_qty / minted range shown in the heatmap header.

### Data layer
- `useQuery` per endpoint with `refetchInterval: 10_000` (matches poller interval).
- A single `api.ts` centralizes fetch + hand-written TS types (no codegen — YAGNI).

### File boundaries
```
web/src/
  api.ts            # fetch + types
  App.tsx           # layout + query providers
  components/
    VaultHealth.tsx
    OracleTable.tsx
    InventoryHeatmap.tsx
    ui/             # Card, Badge, Gauge wrappers
```

## Section 3 — Testing, error handling, risks

### Testing (Rule 9: test intent, not behavior)
- **API (Rust):** `#[sqlx::test]` integration tests with `#[ignore]` (keeps
  `cargo test --workspace` offline-clean). Seed a fixture → hit the endpoint → assert
  JSON shape + decoded values. Edge over happy-path: empty vault → `null` not 500;
  `wl_enabled = false` → withdrawal null; prices-only oracle (svi null) → no crash;
  dirty sanity propagates correctly.
- **Frontend:** Vitest + React Testing Library, testing decode/presentation intent —
  utilization color thresholds, stale-warning trigger, sanity-badge mapping, empty
  `page_leaves` → empty heatmap not crash. Recharts internals are not tested.
- **Monkey (project rule):** API — kill PG mid-request → 500 actionable, not hang;
  Frontend — malformed payload (null fields, empty arrays, oversized page_leaves) →
  no white screen; poller stopped → stale warning lights up.

### Error handling (Rule 12 fail-loud)
- API: DB error → 500 + full log; no rows → 200 + null (legal state).
- Frontend: query error → red "API unreachable" banner + keep last data (don't clear);
  individual null field → that widget shows "—", page does not crash.

### Risks / known unknowns
1. `q_up`/`q_dn` and `range_qty` scale unverified → heatmap uses relative strength
   (correct), but absolute numbers carry no unit or are labeled "raw".
2. NAV formula (balance + total_mtm) not decompile-verified → UI tooltip: "NAV mirrors
   vault_value, sign unverified" (matches view comment).
3. Utilization is OUR definition, not the protocol's internal value → tooltip says so
   (progress.md TODO warns against conflating them).
4. Recharts/Vite is a new npm dependency tree → lock the lockfile; this round does not
   touch the indexer's Rust dependencies.

### Success criteria
- `cargo build --workspace` + `clippy -D warnings` clean (incl. new `crates/api`).
- `cargo test --workspace` (no DB) offline-clean; ignored tests listed.
- Frontend `npm run build` + `vitest` green.
- **Live smoke:** poller + api + web all running; dashboard shows real testnet
  matrices, dirty oracles (if any) highlighted, heatmap rendered, 10s auto-refresh
  visible.
