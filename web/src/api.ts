import { useQuery } from "@tanstack/react-query";
import { POLL_MS } from "./theme";

export type Vault = {
  object_version: number;
  nav: number;
  utilization: number | null;
  balance: number;
  total_mtm: number;
  total_max_payout: number;
  withdrawal_available: number | null;
  wl_enabled: boolean;
  ingested_at: string;
} | null;

export type Oracle = {
  oracle_id: string;
  a: number | null;
  b: number | null;
  rho: number | null;
  m: number | null;
  sigma: number | null;
  svi_sanity: "clean" | "dirty" | "untested" | null;
  svi_checkpoint_seq: number | null;
  spot: number | null;
  forward: number | null;
  prices_checkpoint_seq: number | null;
};

export type Matrix = {
  matrix_object_id: string;
  oracle_id: string;
  matrix_version: number;
  mtm: number;
  range_qty: string; // RAW string — mirrors Rust String, preserves precision
  min_strike: number;
  max_strike: number;
  tick_size: number;
  minted_min_strike: number | null;
  minted_max_strike: number | null;
  page_leaves: { q_up: string; q_dn: string }[];
  ingested_at: string;
};

export async function fetchJson<T>(path: string): Promise<T> {
  const r = await fetch(path);
  if (!r.ok) throw new Error(`${path} → HTTP ${r.status}`);
  return (await r.json()) as T;
}

const opts = { refetchInterval: POLL_MS, staleTime: POLL_MS } as const;

export const useVault = () =>
  useQuery({ queryKey: ["vault"], queryFn: () => fetchJson<Vault>("/api/vault"), ...opts });

export const useOracles = () =>
  useQuery({ queryKey: ["oracles"], queryFn: () => fetchJson<Oracle[]>("/api/oracles"), ...opts });

export const useInventory = () =>
  useQuery({ queryKey: ["inventory"], queryFn: () => fetchJson<Matrix[]>("/api/inventory"), ...opts });
