# Sui Transparency Hub

**The institutional-grade transparency layer for DeepBook Predict** — live 3D SVI vol surface, PLP risk dashboard, arb-free checker, and Walrus-attested risk reports.

> Sui Overflow 2026 — Track: DeepBook & Prediction Markets. Fuses Predict idea-bank #9 (Surface Studio) + #10 (PLP Risk Dashboard).

## The problem

Institutional LPs ask one question before depositing into DeepBook Predict's PLP vault: *"Is it safe, where's the inventory, and what does ±5σ do to NAV?"* Today nobody can answer — so serious capital sits out. Generic tools don't fit: DefiLlama tracks TVL but not Sui object state; Block Scholes renders surfaces but doesn't index Sui; SuiVision/BlockVision index Sui but model no derivatives risk.

## What it does

- **Surface Studio** — live 3D IV surface (strike × expiry) from `oracle::OracleSVIUpdated`, with a time-travel slider.
- **Arb-free checker** — flags SVI fits that violate no-arb (digital monotonicity + calendar).
- **PLP risk dashboard** — utilization, withdrawal-limiter bucket, per-oracle exposure, per-strike inventory heatmap.
- **±5σ stress simulator** — projected PLP NAV drawdown.
- **Walrus attestation** — risk snapshots pinned to Walrus, SHA256-verifiable, citable in DAO/IC memos.
- **Public API** — `GET /v1/surface/btc` + WS stream for institutional consumers.

## Architecture

Off-chain heavy, on-chain light. No new Move contracts for the MVP; a small `attestation::Registry` lands in v1.

```
Sui testnet ─► Indexer ─► Postgres ─► Engine(+pricing) ─► Redis ─► API ─► Web / consumers
   │                                       └─► Walrus (attested snapshots) ┄► Move Registry (v1)
   └─ Predict object state (polled per checkpoint)
```

Docs:
- [`docs/architecture/overview.md`](docs/architecture/overview.md) — start here
- [`docs/specs/2026-05-28-sui-transparency-hub-architecture.md`](docs/specs/2026-05-28-sui-transparency-hub-architecture.md) — full spec (**Appendix B = verified on-chain ABI**)
- [`docs/architecture/module-dependency.mmd`](docs/architecture/module-dependency.mmd), [`docs/architecture/data-flow.mmd`](docs/architecture/data-flow.mmd)
- [`docs/security/threat-model.md`](docs/security/threat-model.md)
- [`BUSINESS_SPEC.md`](BUSINESS_SPEC.md) — business case, personas, GTM

## Verified on-chain facts (testnet, 2026-05-30)

| | |
|---|---|
| Package | `0xf5ea2b3749c65d6e56507cc35388719aadb28f9cab873696a2f8687f5c785138` |
| `Predict` shared object | `0xc8736204d12f0a7277c86388a68bf8a194b0a14c5538ad13f22cbd8e2a38028a` |
| Quote asset | `…::dusdc::DUSDC` (6 decimals) |
| Backfill | `predict-server.testnet.mystenlabs.com` (`/config`, `/oracles`) |

Three gotchas that drive the design (details in spec Appendix B):
1. **PLP has no events** — utilization/bucket/inventory read from `Predict` object state.
2. **Binary digital options** — IV is straight from SVI (no BS inversion); no-arb = digital price monotone in strike.
3. **Dual fixed-point** — 1e9 for prices/strikes/SVI, DUSDC 6-dec for amounts, `i64` sign-magnitude for `rho`/`m`.

## Status

`v0.2` — architecture verified against live testnet. Implementation not yet scaffolded.

Next: `indexer/` skeleton + `pricing/` golden-vector tests against the live SVI sample.

## Stack

Next.js 15 · Three.js · Plotly · TanStack Query · Node/Fastify (or Rust/Axum) · Postgres 16 · Redis 7 · `@mysten/sui` · Walrus · Sui testnet (Protocol 124).
