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
- **Column types must be pinned against the actual SQL before writing structs.**
  Verified against `migrations/0001_oracle_events.sql`: `oracle_latest` =
  oracle_id, a, b, rho, m, sigma, svi_sanity, svi_checkpoint_seq, spot, forward,
  prices_checkpoint_seq. Nullability is real: a **prices-only** oracle has all SVI
  columns + svi_sanity NULL; an **svi-only** oracle has spot/forward/prices_seq NULL
  (FULL OUTER JOIN). API structs use `Option<T>` for every joined column accordingly.
- **View-decoded columns → `f64`; raw unverified-scale columns → `String`.** nav,
  utilization, balance, strikes, tick_size, mtm (view already decoded) are `f64`.
  But `range_qty` and the `page_leaves` q_up/q_dn are **raw u64, scale unverified**
  (NUMERIC/JSONB) — reading them as `f64` loses integer precision above 2^53 and
  silently re-introduces the decode loss the views avoid. Read `range_qty` as
  `String` (NUMERIC→String); pass `page_leaves` through as `serde_json::Value`
  (already strings inside). The frontend parses to number only for the *relative*
  heatmap ratio, which tolerates precision loss because it's normalized.
- Error routing (Rule 12): DB connection/query error → HTTP 500 + full log; empty
  result for `/api/vault` → HTTP 200 + `null` (a legal state, not an error).
- CORS: open to localhost in dev; same-origin in prod (Axum serves `web/dist`).
- **Route precedence:** mount `/api/*` handlers FIRST, then static `ServeDir` for
  `web/dist`, then SPA fallback to `index.html`. No client-side router this round so
  the fallback is trivial, but `/api` must not be shadowed by the static service.
- **Hard constraint: `crates/api` has zero sui-sdk / chain-network deps.** It pulls
  only axum + sqlx (+ serde/tokio). The dashboard reads Postgres, never the chain —
  this keeps a chain-read path from accidentally growing here (mirrors the poller's
  "零新 dep" discipline).

## Section 2 — Frontend (`web/`)

Stack: Vite + React + TypeScript + Tailwind + TanStack Query + Recharts. Single-page
dashboard, three regions.

### ① Vault Health (top KPI row)
- NAV, Utilization %, Withdrawal capacity as KPI panels (see Visual design for the
  depth-bar treatment that replaces dial gauges).
- Utilization color-graded (ok/warn/alert) at thresholds.
- `wl_enabled = false` → withdrawal shows "Unlimited" (matches the view's NULL).
- **Staleness threshold derived from poll cadence, not magic:** poller = 10s, so
  stale = 3× = 30s (one constant in `api.ts`, rationale in a comment). > 30s → warn,
  > 5min → alert. (Was 60s — 6 missed ticks masked a dead poller too long.)

### ② Per-Oracle Exposure (mid table)
- One row per oracle: oracle_id (truncated + copy), spot, forward, SVI params
  (a/b/rho/m/sigma), and a **sanity badge** (clean=green / dirty=red / untested=grey /
  null="prices-only").
- Dirty rows are highlighted — the core README selling point (arb violations visible).
- Optional sorting (by sanity or exposure contribution).

### ③ Inventory Heatmap (bottom, per-oracle expandable)
- Select an oracle → render its `page_leaves` (N page buckets) as a CSS-grid heatmap.
- **Two stacked sequential bands sharing one strike X-axis**, NOT a diverging scale:
  q_up (calls) band on top, q_dn (puts) band below. q_up and q_dn are two separate
  magnitudes, not two ends of one signed axis — a diverging red↔green gradient would
  falsely imply "up = −down". Each band is max-normalized per matrix, quantized to
  ~5–6 steps (instrument data, not a decorative gradient).
- X-axis maps page → strike using `min_strike` / `max_strike` / `tick_size` (the view
  explicitly leaves this mapping to the frontend). Mark ATM/spot with a vertical line.
- Hover tooltip shows raw q_up/q_dn (the String values) + the strike sub-range.
- Header: mtm / range_qty / minted range. **minted range NULL** (view maps u64::MAX →
  NULL = nothing minted yet) renders as **"none minted"**, never blank/dash that reads
  as a data gap.
- Scale label: "relative intensity (max-normalized)" so the unverified-scale risk
  (risk #1) is honest in the UI.

### Data layer
- `useQuery` per endpoint with `refetchInterval: 10_000` (matches poller interval),
  driven off one aligned tick so the three views refetch together (reduces cross-view
  temporal skew; see risk #5).
- **Tombstone removal vs keep-last-on-error must be distinguished.** A *successful*
  poll returning fewer matrices means oracles delisted/settled → drop them from the
  UI (replace, don't merge-retain). Only a *failed* poll retains the last data. If
  these are conflated, tombstoned matrices stick on screen forever (the view INNER
  JOINs `oracle_matrix_listing` precisely to drop them).
- Inventory panel shows BOTH a global "data as of" (the view's `MIN(ingested_at)`
  horizon — a single fresh matrix must not mask 22 stale ones) AND per-matrix
  `ingested_at` in each header.
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
5. **Cross-view temporal skew (transient):** the three endpoints resolve against
   different DB read instants; for up to one poll cycle an oracle can appear in the
   table but not yet in inventory (or vice versa). Mitigated by aligned refetch;
   disclosed, not claimed-away (Rule 12).
6. **Delisting is silent:** a settled/delisted oracle's matrix simply disappears from
   `/api/inventory` (INNER JOIN tombstone). For a risk dashboard that IS a risk event.
   A delisting feed is out of scope this round; documented so disappearance is read as
   "delisted/settled", not "never existed".
7. **Tooltip caveats copied verbatim from the view comments** (the canonical caveat
   source: `0002` NAV "if decompile shows balance − total_mtm, fix THIS LINE";
   utilization "OUR definition"). Copy, don't paraphrase, so a future view-formula fix
   and the UI caveat can't drift.

### Success criteria
- `cargo build --workspace` + `clippy -D warnings` clean (incl. new `crates/api`).
- `cargo test --workspace` (no DB) offline-clean; ignored tests listed.
- Frontend `npm run build` + `vitest` green.
- **Live smoke:** poller + api + web all running; dashboard shows real testnet
  matrices, dirty oracles (if any) highlighted, heatmap rendered, 10s auto-refresh
  visible.

## Visual design

Concept: **"Fathom — deep-water instrumentation."** A Bloomberg-terminal / sonar-readout
hybrid: dark, dense, data-first. The name = depth sounding → abyssal palette with a
single luminous accent (a sonar trace in dark water). NOT neon crypto, NOT pastel SaaS,
NOT purple-on-white. **Dark theme only** — institutional risk desks run dark; no light
mode this round.

### Palette (CSS variables)
```
--abyss-900: #0A0E14   /* app background (near-black, blue-cast) */
--abyss-800: #0E141C   /* panel background */
--abyss-700: #131C26   /* raised card / table header */
--abyss-600: #1B2733   /* borders, dividers (hairlines, not shadows) */
--ink-200:   #C4D0DC   /* primary text */
--ink-400:   #8295A6   /* secondary / labels */
--ink-600:   #4A5A6A   /* tertiary / disabled */
--sonar:     #38E1C4   /* primary accent — the ONE luminous color */
--sonar-dim: #1B6E63   /* accent muted */
--ok:    #3FB68B       /* clean (muted green) */
--warn:  #D9A441       /* untested / caution (amber) */
--alert: #E5484D       /* dirty / arb violation / danger — RED MEANS ONE THING */
--up:    #2DD4BF       /* calls / q_up (teal family) */
--dn:    #E0719B       /* puts / q_dn (rose, NOT red — red is reserved for alerts) */
```
Map every color to these variables — **ban raw Tailwind palette classes**
(`green-500`, `slate-800`, …) in review.

### Typography
- Display / headings / all financial figures: **monospaced** display face — IBM Plex
  Mono (free fallback) or Martian Mono. Mono reads as "instrument" and aligns number
  columns. All figures `tabular-nums` so digits don't jitter on 10s refresh.
- Body / labels: Inter Tight or IBM Plex Sans. **No** plain Inter / Roboto / system-ui /
  Space Grotesk / Space Mono.

### Density & texture
- High density (terminal, not landing page): 8px base, cards 16–20px padding, table
  rows ~36px. 1px hairline borders in `--abyss-600` to separate panels — **no
  box-shadows** (Material = generic).
- Background never flat: faint radial `--sonar` mesh top-center (~3% opacity) over
  `--abyss-900`; optional 2–3% grain overlay (analog-instrument feel).

### Per-region treatment
- **Vault Health (dominant):** NAV panel ~1.5–2× the others (headline), big mono number
  (~40–56px) with a thin `--sonar` underline; label in `--ink-400` caps-tracking.
  Replace dial/donut gauges with **horizontal depth-bars** (sonar metaphor): thin track,
  filled segment crossing `--ok → --warn → --alert` at thresholds, 1px danger tick.
  "Unlimited" withdrawal = full `--sonar` bar + ∞ glyph. Asymmetric grid (NAV spans
  wider) — three identical equal cards is the generic tell.
- **Oracle Exposure table (centerpiece — most refined element):** sanity = **square
  status chip** (8px sharp corners, instrument-LED, not a pill) + uppercase mono label
  `CLEAN`/`DIRTY`/`UNTESTED`/`PRICES-ONLY`. Dirty row: 2px `--alert` left-border
  bleeding off the panel, 6% `--alert` row bg, slow ~2s 1px glow pulse so violations
  attract the eye in a wall of numbers. SVI params right-aligned mono tabular; tint rho
  by sign (negative cool / positive warm). oracle_id `0x12ab…f9` + copy that flashes
  `--sonar`.
- **Inventory Heatmap:** two stacked sequential bands (q_up top / q_dn below) sharing
  the strike X-axis, ATM/spot marked by a vertical `--sonar` line through both. Per-side
  scale `--abyss-700 → --up` / `→ --dn`, ~5–6 quantized steps; empty cell = bare
  `--abyss-800`. Sharp cells, 1px `--abyss-900` grid gaps; hover outlines `--sonar` +
  dark mono tooltip card. Header stats separated by `--abyss-600` pipes.

### Layout / hierarchy
- Single column, max-width ~1400px, dense. Top→bottom decreasing urgency: Vault Health
  (dominant) → Oracle Exposure (centerpiece, most vertical real estate) → Heatmap
  (drill-down; collapsed shows a 1-line sparkline-strip preview, not dead space).
- Sticky slim top bar: "FATHOM" wordmark (mono, letter-spaced) left; `--sonar` live-
  pulse dot + "live · 10s" + global "as of" right.

### Anti-generic-AI guardrails (enforce in review)
- No rounded-2xl + drop-shadow cards → 1px hairline borders, ≤4px corners.
- No Inter/Roboto/system-ui/Space Grotesk/Space Mono → mono display + Inter Tight/Plex.
- No dial/donut gauges → linear depth-bars only.
- No purple, no neon-on-white, no glassmorphism blur.
- No raw Tailwind color tokens → CSS variables only.
- No emoji icons → thin-stroke Lucide (1.5px), sparingly.
- One accent (`--sonar`) carries the UI; a second decorative hue = reject.
- Figures always tabular-nums mono; never reflow width on refresh.
- Motion budget: one staggered panel fade-in on load (~60ms), the dirty-row pulse,
  copy-flash, and a value-change flash (cell briefly flashes `--sonar` when its number
  updates on poll). Nothing else animates.
