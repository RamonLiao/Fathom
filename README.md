# 🌊 Fathom

**The institutional-grade risk & transparency layer for DeepBook Predict.**

> **Sui Overflow 2026** — Track: *DeepBook & Prediction Markets*
> Fuses DeepBook Predict Hackathon Idea Bank **#9 (Surface Studio)** and **#10 (PLP Risk Dashboard)**.

---

## 🛑 The Problem

Institutional liquidity providers (LPs) ask one critical question before deploying capital into DeepBook Predict's PLP vault: 
*"Is it safe, where is the inventory, and what does ±5σ do to our NAV?"*

Currently, the ecosystem lacks the telemetry to answer this:
* **SVI Parameters are a Black Box:** `oracle::OracleSVIUpdated` emits raw parameters (`a, b, rho, m, sigma`) that are unreadable to risk managers.
* **No Arbitrage Validation:** SVI fits can violate calendar or butterfly no-arbitrage conditions, silently mispricing options without warning.
* **PLP is Opaque:** LPs see backward-looking yield but cannot track real-time vault utilisation, withdrawal-limiter token-bucket capacity, or oracle concentration.
* **No Audit Trail:** Risk reports live on mutable servers, leaving no tamper-evident proof for compliance and investment committees.

---

## 🛠️ The Solution

**Fathom** sounds the depths of DeepBook Predict, providing real-time risk modeling, volatility visualisations, and verifiable risk reports.

```
Sui Testnet ─► Custom Indexer ─► Postgres ─► Fathom Engine ─► API / WS ─► Web Dashboard
                                                 │
                                                 └─► Walrus (Attested Risk Reports)
```

### 🔮 1. Surface Studio (Idea #9)
* **Live 3D SVI Volatility Surface:** Dynamic strike × expiry → IV visualisation using Three.js and Plotly.
* **Time-Travel Slider:** Scroll back through historical checkpoints to analyse how the vol surface morphed during market shocks.
* **Arbitrage-Free Checker:** Real-time mathematical validation flagging butterfly (negative probability density) and calendar variance violations.

### 📊 2. PLP Risk Dashboard (Idea #10)
* **Real-time Telemetry:** Instant tracking of vault utilisation %, withdrawal-limiter token-bucket capacity, and per-oracle exposure.
* **Inventory Heatmap:** A per-strike heatmap mapping token concentration against active option expiries.
* **±5σ Stress Simulator:** Interactive "what-if" simulator projecting PLP NAV drawdowns under extreme market movements.

### 🛡️ 3. Walrus-Attested Provenance
* **Tamper-Evident Risk Reports:** Generate immutable, cryptographically verifiable risk snapshots pinned directly to **Walrus**.
* **Institutional Citation:** Provides compliance teams and DAOs with citable, tamper-proof URIs (`walrus://...`) for audit trails.

---

## 🏗️ Technical Architecture

* **Frontend:** Next.js 15, Three.js, Plotly, TanStack Query, Tailwind CSS.
* **Backend:** Node.js (Fastify) / Rust (Axum), Postgres 16, Redis 7.
* **Sui & Storage Integration:** `@mysten/sui`, Walrus Storage, Sui Testnet (Protocol 124).
* **On-chain Attestation:** MVP features off-chain verification; v1 introduces a lightweight `AttestationRegistry` shared object on Sui.

---

## 🔍 Verified On-chain Facts (Testnet, 2026-05-30)

| Entity | Address / Detail |
|---|---|
| **Package** | `0xf5ea2b3749c65d6e56507cc35388719aadb28f9cab873696a2f8687f5c785138` |
| **`Predict` Shared Object** | `0xc8736204d12f0a7277c86388a68bf8a194b0a14c5538ad13f22cbd8e2a38028a` |
| **Quote Asset** | `…::dusdc::DUSDC` (6 decimals) |
| **Backfill API** | `predict-server.testnet.mystenlabs.com` (`/config`, `/oracles`) |

### ⚡ Key Architectural Gotchas
1. **PLP has no events:** Utilisation, token-bucket, and inventory are read directly from the `Predict` shared object state.
2. **Binary digital options:** IV is computed directly from SVI (no Black-Scholes inversion needed). No-arbitrage requires digital option prices to be monotonic in strike.
3. **Dual fixed-point math:** 1e9 for option prices/strikes/SVI parameters, DUSDC 6-decimal for amounts, and `i64` sign-magnitude format for SVI `rho` and `m`.

---

## 🚀 Getting Started

1. **Clone the repository:**
   ```bash
   git clone <repo-url>
   cd 02-sui-transparency-hub
   ```

2. **Install dependencies & launch dev server:**
   ```bash
   npm install
   npm run dev
   ```
