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
