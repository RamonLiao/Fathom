# Sui Transparency Hub — Business Specification

> Track: **Sui Overflow 2026 — DeepBook & Prediction Markets**
> Working name: **Surface Studio + PLP Risk Hub**
> One-liner: *The institutional-grade transparency layer for DeepBook Predict — live 3D vol surface, PLP risk dashboard, arb-free checker, and Walrus-attested risk reports.*

---

## 1. Executive Summary

DeepBook Predict turned BTC prediction markets into a continuous, SVI-driven vol-surface protocol on Sui. The protocol is live on testnet; the foundation has publicly listed two analytics gaps as judge-priority work items: **Surface Studio (idea bank #9)** and **PLP Risk Dashboard (idea bank #10)**. Both gate serious LP TVL — institutions will not deposit into a vault they cannot inspect.

Sui Transparency Hub fuses these two into one product: a Sui-native analytics console that streams `oracle::OracleSVIUpdated`, renders a live 3D IV surface with time-travel, runs arb-free butterfly/calendar checks, exposes PLP utilization + withdrawal-bucket state + per-strike inventory heatmap, and lets users run ±5σ stress simulations. Historical risk reports are pinned to Walrus for immutable audit. A REST/GraphQL endpoint exposes the same data for institutions.

Differentiation vs DefiLlama / Nansen / Parsec / Block Scholes: those are **either** generic DeFi dashboards **or** generic vol-surface viewers. None of them index Sui-native object state (withdrawal-limiter token-buckets, per-oracle exposure, PLP inventory) and none of them carry Walrus-attested provenance. This is a wedge, not a clone.

Real-World (50% scoring weight) is the strongest axis: the product answers the single question blocking institutional Predict LP deposits — *"Is PLP safe and what does ±5σ do to it?"*

---

## 2. Problem Statement

DeepBook Predict's stack on Sui has structural blind spots today:

1. **No live SVI surface viewer.** `oracle::OracleSVIUpdated` emits SVI params per expiry, but humans cannot read raw `a, b, rho, m, sigma`. Traders and treasury teams need (strike × expiry → IV) rendered as a surface, not a JSON blob.
2. **No arb-free guarantee surfacing.** SVI fits can violate butterfly (negative density) or calendar (non-monotone total variance) no-arb conditions. Today nobody flags this — bad fits silently mis-price strikes.
3. **PLP vault is a black box.** LPs see APY but not: vault utilization %, withdrawal-limiter token-bucket state, per-oracle exposure, per-strike inventory concentration, max drawdown under ±Xσ. Result: institutional capital sits out.
4. **No resolution-risk telemetry.** Oracle staleness, settlement-lag distributions, redeem queue depth — none currently dashboarded for Predict markets. These signals exist in chain events but no Sui-native tool aggregates them — confirmed: SuiVision and BlockVision are explorer + indexing APIs (TVL, portfolio tracking, basic Typus position indexing) and do not provide IV / Greeks / vol-surface or derivatives risk modeling [source: SuiVision & BlockVision product docs, suivision.xyz / blockvision.org, 2025].
5. **No immutable audit trail.** Risk reports today (if any) live on protocol websites; a protocol could rewrite history. Institutions require tamper-evident provenance.
6. **Generic tools don't fit Sui.** DefiLlama tracks TVL but not Sui object state; Block Scholes renders surfaces but doesn't index Sui events; Nansen lacks Sui DeFi depth (per gemini, Suivision/BlockVision lead Sui indexing but neither carries derivatives risk depth).

---

## 3. Target Users & Personas

### P1 — *Maya, Pro Vol Trader* (primary)
- Trades BTC options on Deribit + Predict; runs delta-neutral vol-arb.
- Need: live IV surface with arb-free flags; cross-check Predict-implied vol vs Deribit smile; alerts on smile-shape anomalies.
- Willingness to pay: $50–500/mo (Parsec/Laevitas tier per gemini benchmark).

### P2 — *Karthik, Crypto-Fund Treasury / Predict PLP Allocator* (primary)
- Considers a $2–10M PLP allocation; mandate requires documented risk policy.
- Need: utilization gauges, withdrawal-bucket headroom, ±5σ drawdown simulator, exportable VaR / drawdown-replay reports, Walrus-attested historical reports for compliance.
- Willingness to pay: $5k–50k/mo enterprise API tier.

### P3 — *Lin, DAO Researcher / Risk Committee Member* (secondary)
- Writes the public risk memo before the DAO votes to deposit treasury into PLP.
- Need: tamper-evident historical reports, per-strike inventory heatmap, oracle-feed health timeline; ability to cite specific snapshots.

### P4 — *Ravi, Quant Dev at a Sui-native Vault* (secondary)
- Builds PLP-hedge or range-ladder vaults (companion projects in this track).
- Need: surface JSON API + WebSocket stream of SVI updates; standardized risk-event schema to feed his own backtester.

---

## 4. Use Cases

### UC-1 — Vol Surface Stress (Maya)
Maya opens Surface Studio. The 3D surface (strike × expiry → IV) updates live from `OracleSVIUpdated`. She drags the time-travel slider back 6 hours: the front-expiry smile inverted. The arb-free checker shows a red butterfly violation at the 105k strike during that window. She knows the surface mis-priced ITM puts → cross-venue arb opportunity vs Deribit. She exports the snapshot as Walrus-pinned JSON for her trade memo.

### UC-2 — PLP Risk Monitoring (Karthik)
Karthik's mandate requires daily review. He loads the PLP page: utilization 62%, withdrawal-bucket at 78% capacity (token-bucket refill rate visible), per-oracle exposure shows 84% on the 1h BTC oracle (concentration risk). He runs the ±5σ simulator: projected PLP NAV drawdown −18.3% under a +5σ move. He clicks "Generate Risk Report" — the snapshot pins to Walrus, returns a `walrus://blob/0x…` URI he pastes into his IC memo.

### UC-3 — Resolution-Risk Alert (Lin)
Lin set an alert: "notify if any settled oracle has > 5min redeem lag OR > 3% un-redeemed positions 1h post-settlement." A market settled; the keeper network was slow; her dashboard fires. She drills into the per-market resolution timeline and pulls the historical Walrus snapshot showing the lag pattern over the prior 30 days. She files an issue with the protocol team with citable evidence.

### UC-4 — API Consumer (Ravi)
Ravi's PLP-hedge vault needs a programmatic surface snapshot every 60s. He hits `GET /v1/surface/btc?expiry=1h` → arb-flagged SVI params + interpolated IV grid. WebSocket subscribes him to `OracleSVIUpdated` re-broadcast, normalized.

---

## 5. Market Analysis

### TAM / SAM / SOM (rough estimates)
- **TAM** — crypto derivatives analytics combined ARR: **~$17–22M (2024), projected $25–35M (2025)** for Laevitas + Block Scholes + Amberdata (incl. Genesis Volatility) + Greeks.live [source: GetLatka Amberdata Revenue Report 2024; PitchBook/Tracxn funding data; Block Scholes & Laevitas pricing pages 2024]. Broader DeFi analytics adds Dune (~$15–25M ARR, $1B valuation), Nansen (~$9–11M ARR), DefiLlama (>$1M ARR) [source: GetLatka SaaS DB; Blockworks/The Block M&A coverage 2024–2025] — total addressable ≈ **$50–80M ARR**, materially smaller than the originally stated $300M.
- **SAM** — on-chain derivatives + prediction-market analytics. Internal projection (no external benchmark): ~$5–10M ARR based on 10–15% of TAM, scaled by post-Polymarket on-chain derivatives growth (unverified).
- **SOM** — Sui-native derivatives/predict analytics in next 24 months. Internal projection (no external benchmark): $2–5M ARR based on DeepBook Predict reaching mainnet with $50–200M TVL and 5–20% of LPs/traders paying for analytics; anchored on Sui TVL peak ~$4.3B 2025 [source: DefiLlama Sui Dashboard 2024–2025; Messari Sui Quarterly] (unverified forward).

### Competitive Table

| Tool | Surface | Arb-free | PLP-specific | Sui-native | Walrus attestation | Pricing |
|---|---|---|---|---|---|---|
| **DefiLlama** | No | No | TVL only | Partial | No | Free |
| **Dune** | DIY SQL | No | DIY | Partial (slow refresh) | No | Freemium |
| **Nansen** | No | No | No | Shallow | No | $1k–10k+/mo |
| **Parsec** | No | No | Lending only | No | No | $50–500/mo |
| **Block Scholes** | Yes (TradFi-grade) | Yes (strict) | No | No | No | Institutional |
| **Laevitas** | Yes (best viz) | Built-in | No | No | No | Free / $50 / $500 |
| **Greeks.live** | Yes | Yes | No | No | No | Free |
| **Suivision / BlockVision** | No | No | No | Yes (best Sui indexer) | No | Free / API |
| **Sui Transparency Hub** | **Yes** | **Yes** | **Yes** | **Yes** | **Yes** | Freemium + Enterprise |

Source URLs (per gemini research): defillama.com, dune.com, nansen.ai, parsec.fi, blockscholes.com, laevitas.ch, greeks.live, amberdata.io, suivision.xyz, blockvision.org. Adjacent prediction-market tools verified: **Bravado** (bravadotrade.com — pro trading terminal for Polymarket: limit orders, copy-trading, LP farming), **Polyseer** (polyseer.xyz — AI multi-agent research engine producing confidence-scored reports on odds movements), **uma.rocks** (uma.rocks — UMA optimistic-oracle voting + staking dashboard used by Polymarket for dispute resolution) [source: respective product sites, 2025]. None of the three overlap our Sui-native SVI-surface + PLP-risk wedge.

---

## 6. Differentiation

Three moats no incumbent has simultaneously:

1. **Sui-native object indexing.** We read PLP withdrawal-limiter token-bucket state, per-strike `Bag` inventory, `PredictManager` exposure — chain-state nobody else parses. Generic indexers (Suivision) don't model derivatives risk; derivatives tools (Block Scholes) don't index Sui.
2. **Unified surface + PLP risk in one view.** Block Scholes shows surfaces; Parsec shows lending risk. Nobody shows: "this PLP vault's per-strike inventory mapped onto the live SVI surface, with ±5σ drawdown projected." That cross-product view *is* the product.
3. **Walrus-attested provenance.** Risk snapshots pin to Walrus → tamper-evident → citable in DAO memos, institutional ICs, regulators. Gemini flagged "data provenance" as the #1 institutional complaint about DefiLlama. We solve it natively.

Secondary: arb-free butterfly/calendar checker on every SVI update — Block Scholes-strict, but free at the retail tier.

---

## 7. Product Scope

### MVP (6 weeks, hackathon deliverable)
- Live 3D vol surface from `OracleSVIUpdated` (Three.js / Plotly).
- Time-travel slider (last 7 days, 5-min resolution).
- Arb-free butterfly + calendar checker with visual flags.
- PLP utilization, withdrawal-bucket gauge, per-oracle exposure pie.
- Per-strike inventory heatmap.
- ±5σ what-if simulator → projected PLP NAV delta.
- One Walrus-pinned risk-report demo (snapshot → blob URI).
- Single REST endpoint `GET /v1/surface/btc`.

### v1 (3 months post-hackathon)
- Alerts engine (webhook + Telegram + email).
- Historical drawdown replay (re-simulate any past 30-day window).
- Oracle health timeline (staleness, deviation, lag).
- Resolution-risk page (redeem lag, un-redeemed %, keeper performance).
- Multi-asset (ETH, SUI surfaces if Predict adds them).
- GraphQL API + WebSocket stream.
- Embeddable widget for partner protocols.

### v2 (12 months)
- Institutional report builder (PDF + Walrus dual export).
- Cross-protocol risk: extend to Scallop/Navi/Suilend liquidations, DeepBook spot LP risk.
- Side-by-side: Predict smile vs Deribit/Polymarket smile.
- White-label SDK for vault protocols to embed their own risk view.

---

## 8. User Flow

**First-time pro trader (Maya):**
1. Land on `transparency.sui` → see live BTC surface above the fold, no login.
2. Click any strike/expiry → side panel shows IV, Greeks (delta/gamma/vega), arb-flag status.
3. Drag time-travel slider → surface morphs historically; arb violations highlighted in red ribbons.
4. Click "Pin snapshot" → wallet connect → snapshot pinned to Walrus → URI shown.

**PLP allocator (Karthik):**
1. Click PLP tab → utilization + withdrawal-bucket gauges at top.
2. Per-oracle exposure + per-strike heatmap mid-page.
3. ±5σ simulator panel — slider for σ, instant NAV-delta projection.
4. "Generate Risk Report" → PDF + Walrus URI + signed JSON.
5. Upgrade prompt → enterprise API key for daily automated reports.

**API consumer (Ravi):**
1. Sign up → API key.
2. `GET /v1/surface/btc?expiry=1h` returns `{svi_params, arb_flags, iv_grid, last_update_ts, walrus_attestation}`.
3. WS subscribe to `surface.btc.updated` → push on every `OracleSVIUpdated`.

---

## 9. Technical Architecture (Summary)

- **Ingest.** Custom Sui indexer subscribes to `oracle::OracleSVIUpdated`, PLP vault object mutations (`UtilizationChanged`, `WithdrawalBucketChanged`), per-strike inventory `Bag` deltas, `predict::Redeemed` events. Falls back to `predict-server.testnet.mystenlabs.com` for replay/backfill.
- **Pricing layer.** Pyth BTC price feed for spot; SVI eval engine (Rust or TS) computes IV grid + Greeks; arb-free checker enforces butterfly (`d²C/dK² ≥ 0`) and calendar (`∂(w(T))/∂T ≥ 0`) on every update.
- **Storage.** Postgres for hot time-series (last 30 days), object store for older; Walrus for attested snapshots.
- **API.** REST + GraphQL + WebSocket (Node/Fastify or Rust/Axum).
- **Frontend.** Next.js + Three.js (surface) + Plotly (2D charts) + TanStack Query.
- **Attestation.** Periodic snapshot → canonicalized JSON → SHA256 → Walrus blob → store `(blob_id, sha256, ts)` on Sui as a shared object for cross-verifiability.
- **No new Move contracts required for MVP.** v1 adds an `AttestationRegistry` shared object.

---

## 10. Business Model

**Freemium SaaS + Data API.**

| Tier | Price | Target | Features |
|---|---|---|---|
| Free | $0 | Retail traders | Live surface, basic PLP view, 24h history |
| Pro | $49/mo | Active traders | Full history, alerts, exports, arb-flag webhooks |
| Team | $499/mo | Small funds | Multi-user, scheduled reports, REST/GraphQL |
| Enterprise | $5k–50k/mo | Institutional LPs | High-rate API, WebSocket, custom reports, SLA, white-label |

Secondary: **Sponsored data feeds** — vault protocols pay to have their own PLP-style vault indexed and displayed. **Walrus attestation as a service** — third-party protocols pay per snapshot pin.

Unit economics — internal projection (no external benchmark): at v1 with 500 free / 50 pro / 5 team / 2 enterprise → ~$22k MRR (unverified forward). For benchmark, Parsec Finance Pro tier was **$60/mo**, peaked at ~$166k MRR (~$2M ARR) before shutting down Feb 2026 — cautionary tale: pure dashboard SaaS without execution layer struggled to cover multi-chain indexing costs [source: The Block Parsec Finance Shutdown Report, Feb 2026; Parsec founder retrospective]. Pricing band $60–99/mo aligns with Nansen ($49–69), Dune ($65), Parsec ($60) [source: respective pricing pages, 2024–2026].

---

## 11. Go-to-Market

**Phase 0 (hackathon).** Win/place to get DeepBook-team co-marketing. Direct outreach in DeepBook Builder Telegram + Mysten office hours (Tony).

**Phase 1 (weeks 0–8 post-hackathon).** Free tier public. Seed 5 institutional design-partners (Predict LP candidates) via Mysten introductions. Publish weekly "PLP State of the Vault" Walrus-attested reports — content marketing + provenance demo.

**Phase 2 (months 2–6).** Partner embeds — every PLP-hedge / range-ladder vault in this track is a potential white-label customer. Launch enterprise tier.

**Phase 3 (months 6–18).** Expand beyond Predict: index DeepBook spot LP risk, Scallop/Navi/Suilend liquidations. Position as **the** Sui DeFi risk transparency layer.

---

## 12. Hackathon Demo Plan + Judging Mapping

**5-min demo script:**
1. (0:00–0:30) Problem: institutional LP refuses to deposit because PLP is a black box.
2. (0:30–1:30) Surface Studio: 3D surface rotating, click a strike, time-travel back to show a historical butterfly violation.
3. (1:30–3:00) PLP page: utilization, withdrawal-bucket, per-strike heatmap, run ±5σ simulator → −18% NAV.
4. (3:00–4:00) Click "Generate Risk Report" → pin to Walrus → show URI → re-fetch → SHA matches → tamper-evident.
5. (4:00–4:30) API: live curl → JSON + WS stream.
6. (4:30–5:00) Roadmap + ask.

**Judging axis mapping:**
- **Real-World (50%)** — directly unblocks institutional PLP TVL; addresses two explicit DeepBook-team idea-bank items (#9, #10). Target: 47/50.
- **Product & UX (20%)** — 3D surface is "visual gold"; gauges + heatmap immediately legible. Target: 17/20.
- **Tech (20%)** — custom indexer + SVI eval + arb-free math + Walrus attestation. Target: 17/20.
- **Presentation (10%)** — strong narrative: "the question every institutional LP asks, answered live." Target: 9/10.
- **Total target: ~90/100.**

---

## 13. Risks & Mitigations

| Risk | Likelihood | Mitigation |
|---|---|---|
| "Just a dashboard" perception | Med | Lead pitch with Walrus attestation + institutional API — not a dashboard, a **transparency rail**. |
| Competing teams build same dashboard with less depth | Med | Ship 3D + arb-free + Walrus in MVP; depth = moat. |
| Engineering-heavy frontend (3D viz) over-runs scope | High | Three.js + battle-tested Plotly fallback; cut time-travel to nice-to-have if Week 4 slips. |
| Predict testnet events unstable | Med | Backfill from `predict-server`; mock SVI generator for demo failsafe. |
| Walrus attestation UX too abstract for judges | Med | Demo the tamper-evident re-fetch on stage — show SHA mismatch on a tampered file. |
| No on-chain economic activity → low Tech score | Med | Highlight indexer + SVI math + on-chain `AttestationRegistry` Move module. |
| Pyth feed lag during demo | Low | Cache last-known + visible "last update" timestamp. |
| Institutional sales cycle long (6–12mo) | High | Hackathon win + Mysten intros shortcut; v1 priced for individual pros first. |

---

## 14. Open Questions

1. **Should resolution-risk monitoring be a separate paid tier or bundled into Pro?** Lin's persona overlaps DAO researchers who may not pay personally.
2. **Walrus attestation cost model** — who pays the blob storage? Free tier gets cached/non-attested snapshots; Pro+ gets attestation included?
3. **Does the Predict team want us to surface their roadmap (multi-asset, longer expiries)?** Co-marketing alignment TBD.
4. **Cross-venue smile comparison (Predict vs Deribit vs Polymarket)** — v1 or v2? Strong differentiator but adds two external data dependencies.
5. **AttestationRegistry as shared Move object vs off-chain DB** — does institutional buyer actually value the on-chain record, or is the Walrus blob enough? On-chain registry is the stronger story but adds Move surface area (unverified — needs buyer-discovery validation).
6. **Open-source the indexer?** Gives credibility + community contributions; risks competitors forking. Likely answer: open-source indexer, closed-source risk models + alerts.
7. **Regulatory positioning** — is "risk transparency layer" a regulated activity in any jurisdiction (esp. if institutional clients cite our reports in offering docs)? Need counsel before enterprise GTM.

---

*Research sources: gemini queries on (1) DeFi analytics landscape [Dune/Nansen/DefiLlama valuations — PitchBook, GetLatka, Blockworks 2023–2025], (2) crypto vol-surface tools [Laevitas/Block Scholes/Amberdata/Greeks.live pricing & ARR — GetLatka, official pricing pages 2024], (3) Sui analytics ecosystem [DefiLlama Sui dashboard, Messari Sui Quarterly, SuiVision/BlockVision docs, Chaos Labs risk framework 2024–2025], (4) Parsec Finance benchmarks [The Block shutdown report Feb 2026]. Items marked (unverified) are estimates or forward-looking claims not directly sourced.*
