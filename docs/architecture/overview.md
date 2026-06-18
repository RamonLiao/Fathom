# Sui Transparency Hub — Architecture Overview

> High-level entry point. Full detail: [`../specs/2026-05-28-sui-transparency-hub-architecture.md`](../specs/2026-05-28-sui-transparency-hub-architecture.md).
> On-chain ABI ground-truth: that spec's **Appendix B** (verified 2026-05-30).

## What it is

A Sui-native analytics console for **DeepBook Predict**: live 3D SVI vol surface, PLP risk dashboard, arb-free checker, and Walrus-attested risk reports. The product answers the one question blocking institutional PLP deposits: *"Is PLP safe, and what does ±5σ do to NAV?"*

## Shape: off-chain heavy, on-chain light

No new Move contracts for the MVP demo path. Engineering load is in four off-chain services plus a frontend; a small `attestation::Registry` Move module is the only on-chain surface, and only in v1.

```
Sui testnet ──► Indexer ──► Postgres ──► Engine(+pricing) ──► Redis ──► API ──► Web / API consumers
   │                                          │
   └─ Predict object state (polled)           └─► Walrus (attested snapshots) ─┄► Move Registry (v1)
```

See [`module-dependency.mmd`](module-dependency.mmd) and [`data-flow.mmd`](data-flow.mmd).

## Six modules

| Module | Role |
|---|---|
| `indexer/` | gRPC event subscription **+ per-checkpoint `Predict` object polling** (PLP has no events) |
| `pricing/` | SVI → IV surface, **digital** Greeks, **digital** no-arb checks (no BS inversion) |
| `engine/` | PLP snapshot from object state, ±σ stress, canonicalization → SHA256 |
| `attestation/` | Walrus pin; v1 on-chain `register()` |
| `api/` | REST + WS (+ GraphQL v1) |
| `web/` | Next.js + Three.js (surface) + Plotly (gauges/heatmap) |

## The three things that make or break this build

1. **PLP state lives in object state, not events.** The `plp` module emits nothing. Utilization / withdrawal-bucket / per-strike inventory are read from the `Predict` shared object (`vault`, `withdrawal_limiter: RateLimiter`, `vault.oracle_matrices: Table<ID, StrikeMatrix>`). The indexer must poll + diff, not just subscribe.

2. **Binary digital options, not vanilla.** The protocol prices up/down digitals on-chain via `math::normal_cdf`. IV comes straight from SVI (`IV = sqrt(w/T)`) — **no Black–Scholes inversion**. No-arb = digital price `up_price(K)` monotone non-increasing in `K`, ∈ [0,1]; calendar = total variance non-decreasing in `T`.

3. **Two fixed-point scales.** Prices / strikes / SVI params are **1e9**; quote amounts (DUSDC) are **6-decimal**; `rho`/`m` are `i64` **sign-magnitude**. Mixing them silently corrupts NAV by 1000×. Centralize scaling; assert invariants.

## Upstream dependencies (verified live)

- Package `0xf5ea2b3749c65d6e56507cc35388719aadb28f9cab873696a2f8687f5c785138`
- `Predict` shared object `0xc8736204d12f0a7277c86388a68bf8a194b0a14c5538ad13f22cbd8e2a38028a`
- Quote asset `…::dusdc::DUSDC` (6 dec)
- `predict-server.testnet.mystenlabs.com` (`/config`, `/oracles`) for backfill
- Walrus testnet for attestation
- Pyth: **optional** cross-check only (protocol oracle already publishes spot + forward)

## Status

- v0.2 architecture (this set of docs) — ABI verified against testnet.
- Next: scaffold `indexer/` + `pricing/` golden-vector tests against the live `rho = -0.94` SVI sample.
