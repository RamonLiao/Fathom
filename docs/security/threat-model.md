# Sui Transparency Hub — Threat Model

> Scope: MVP (off-chain analytics + Walrus attestation) + v1 `attestation::Registry` Move module.
> Companion: spec §6. ABI ground-truth: spec Appendix B (verified 2026-05-30).
> Method: STRIDE-lite per trust boundary + Move red-team checklist (per project rules).

## 1. Assets & trust boundaries

| Asset | Why it matters |
|---|---|
| Surface / PLP snapshot integrity | Institutions cite our numbers in IC memos; wrong → reputational + financial harm |
| Attestation hash chain (SHA256 ↔ Walrus blob ↔ on-chain record) | The entire "tamper-evident" value prop |
| Indexer correctness vs chain | Garbage-in → every downstream metric wrong |
| API availability + fairness | Paid SLA tiers |

Trust boundaries: **(B1)** Sui chain → indexer; **(B2)** indexer → Postgres → engine; **(B3)** engine → Walrus / Move; **(B4)** API → untrusted clients; **(B5)** predict-server backfill → indexer.

## 2. Threats by boundary (STRIDE-lite)

### B1 — Chain → Indexer
| # | Threat | STRIDE | Mitigation |
|---|---|---|---|
| 1 | Reorg / forked checkpoint replayed | Tampering | Index only finalized checkpoints; key by `(tx_digest, event_seq)`; object snapshots keyed by `checkpoint` |
| 2 | `i64::I64` decoded as two's-complement → `rho`/`m` sign flip → mirrored smile | Tampering (data) | Decode sign-magnitude `(is_negative?-1:1)*magnitude`; golden test vs live `rho=-0.94` |
| 3 | Dual-scale mixing (DUSDC 6-dec vs 1e9) → metrics off 1000× | Information (corruption) | Per-column scale tags; one `scale()` helper; assert `utilization ∈ [0,1]`, `0 ≤ up_price ≤ 1e9` |
| 4 | Missed `Predict` object poll → stale PLP gauges shown as fresh | Repudiation/Info | Stamp every snapshot with source `checkpoint` + `ts`; UI shows staleness badge; alert if poll lag > N checkpoints |

### B2 — Indexer → Engine
| # | Threat | STRIDE | Mitigation |
|---|---|---|---|
| 5 | Float non-determinism across hosts breaks attestation hash | Tampering | Fixed 1e9 integer / IEEE-754 `f64`; mirror on-chain `math::{normal_cdf,sqrt,ln,exp}`; canonicalization = sorted keys + fixed precision |
| 6 | SVI degenerate inputs (`sigma=0`,`b=0`,`\|rho\|=1`) panic the engine | DoS | Guard/clamp in `pricing/`; emit `arb_flag` not panic; monkey-fuzz suite |
| 7 | Withdrawal limiter `enabled=false, capacity=0` → div-by-zero | DoS | Branch on `enabled`; render "limiter off" |

### B3 — Attestation (Walrus + Move)
| # | Threat | STRIDE | Mitigation |
|---|---|---|---|
| 8 | Attacker uploads tampered JSON claiming to be a Hub snapshot | Spoofing | v1: on-chain `register()` from known submitter addr; client verifies via Sui RPC; UI filters by trusted submitter |
| 9 | Walrus outage → "Generate Report" hangs | DoS | Async queue + retry; show pending; SHA256 still computed/displayed |
| 10 | Re-fetch verification gives false ✅ (hash computed over wrong bytes) | Tampering | Verify over the exact canonical bytes; show both stored + recomputed hash side by side |

### B4 — API → clients
| # | Threat | STRIDE | Mitigation |
|---|---|---|---|
| 11 | Free-tier scraping exhausts DB | DoS | IP rate limit + Redis TTL (surface 5s, plp 30s); paid bypass via `x-api-key` |
| 12 | API key leak / sharing | Spoofing/EoP | Per-key rate + rotation; never log keys; scope keys to tier |
| 13 | Injection via query params (`expiry`, `sigma`) | Tampering | Strict enum/range validation; parameterized SQL only |

### B5 — predict-server backfill
| # | Threat | STRIDE | Mitigation |
|---|---|---|---|
| 14 | Backfill source disagrees with chain | Tampering | Chain is source of truth; reconcile hourly; flag divergence, never overwrite chain-derived rows |

## 3. Move red-team checklist — `attestation::Registry` (v1)

Per project rule (≤5 attack vectors), run before any mainnet publish:

1. **Access-control bypass** — v1 has no admin cap by design (anyone may `register`); off-chain dedupes by `sha256`. Risk accepted for v1; v2 adds optional `RegistrarCap` allow-list. ✅ explicit.
2. **Integer overflow** — `count: u64` increments; realistic rate (288/day) never approaches `u64::MAX`. ✅
3. **Object manipulation** — `Registry` is shared; only `count` mutates; no `UID` transfer / no value held. ✅
4. **Economic exploit** — module holds no funds; N/A. ✅
5. **DoS via spam `register`** — junk records inflate gas + index noise. Mitigation: backend-only submitter on paid tiers; v2 optional per-tx fee / cap. ⚠️ monitor.

## 4. Residual risks (accepted for hackathon)

- No on-chain attestation in MVP (Walrus blob + off-chain DB only) — provenance story is weaker until v1 `register()`.
- Trusted-submitter filtering is UI-side in v1; not cryptographically enforced until allow-list (v2).
- Stress simulator is a model approximation; surfaced as "model estimate", not a guarantee.

## 5. Verification hooks

- Golden-vector tests: live SVI sample → known IV grid + arb flags.
- Property test: canonicalize round-trip hash-stable across machines.
- Monkey/fuzz: random SVI incl. degenerate; assert no panic, typed `ArbFlag` only.
- Move: `sui move test` — `register` happy-path + concurrent submitters.
