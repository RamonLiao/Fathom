# Sui Transparency Hub (Surface Studio + PLP Risk Dashboard)

**One-line pitch**: The institutional-grade analytics layer for DeepBook Predict — live 3D vol surface, PLP risk dashboard, arb-free checker, and Walrus-attested risk reports.

## Problem it solves
Institutional LPs ask: "Is PLP safe? Where is the inventory? What does ±5σ do to the vault?" No one can answer today, so serious capital sits out.

## Core mechanism
- Stream `oracle::OracleSVIUpdated` → render live 3D IV surface (strike × expiry → IV) with time-travel slider.
- Arb-free butterfly/calendar checker — flag SVI fits that violate no-arb conditions.
- PLP dashboard: vault utilization, withdrawal-limiter token-bucket state, per-oracle exposure, per-strike inventory heatmap.
- ±5σ BTC what-if simulator → projected PLP drawdown.
- Walrus stores immutable historical risk reports; institutional API endpoint for VaR / drawdown replay.

## Why this track
Directly fuses HANDBOOK idea bank #9 (Surface Studio) + #10 (PLP Risk Dashboard) — the two analytics items the Predict team explicitly listed. Real-World 50% scoring is maxed because this **gates institutional TVL**.

## Win probability: 82/100
Two judge-listed ideas in one. Highly demoable (3D charts are visual gold). Risk: "just a dashboard" perception, no on-chain economic activity by itself.

## Risks / weaknesses
- Could feel passive — no trading flow.
- Competing teams may build the same dashboard with less depth.
- Requires polished data viz; engineering-heavy frontend.

## Required Sui primitives
- DeepBook Predict: `oracle::OracleSVI*` events, PLP vault state, `predict::supply`, withdrawal-limiter object reads.
- Walrus (immutable attestation).
- Indexer: `predict-server.testnet.mystenlabs.com` + custom event subscriber.

## MVP scope
- Live 3D vol surface with time-travel.
- PLP utilization + withdrawal-bucket gauges.
- Per-strike inventory heatmap.
- ±5σ what-if simulator with PnL output.
- One Walrus-stored historical risk report demo.
- Single REST endpoint exposing surface JSON for institutions.
