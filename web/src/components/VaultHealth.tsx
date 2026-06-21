import type { Vault } from "../api";
import { staleness } from "../theme";
import { DepthBar } from "./ui/DepthBar";

const fmt = (n: number) => n.toLocaleString("en-US", { maximumFractionDigits: 0 });

export function VaultHealth({ vault, now }: { vault: NonNullable<Vault>; now: number }) {
  const stale = staleness(vault.ingested_at, now);
  const borderTop = stale === "alert" ? "border-t-alert" : stale === "warn" ? "border-t-warn" : "border-t-transparent";
  return (
    <section data-stale={stale} className={`grid grid-cols-4 gap-px bg-abyss-600 border-t ${borderTop}`}>
      <div className="col-span-2 bg-abyss-800 p-5">
        <div className="text-ink-400 text-xs tracking-widest uppercase">NAV (DUSDC)</div>
        <div className="font-mono tnum text-5xl text-ink-200">{fmt(vault.nav)}</div>
        <div className="h-px bg-sonar mt-2 w-24" />
      </div>
      <div className="bg-abyss-800 p-5">
        <div className="text-ink-400 text-xs tracking-widest uppercase">Utilization</div>
        <div className="font-mono tnum text-2xl">{vault.utilization == null ? "—" : `${(vault.utilization * 100).toFixed(2)}%`}</div>
        <div className="mt-3"><DepthBar pct={vault.utilization ?? 0} /></div>
      </div>
      <div className="bg-abyss-800 p-5">
        <div className="text-ink-400 text-xs tracking-widest uppercase">Withdrawal</div>
        <div className="font-mono tnum text-2xl">
          {vault.wl_enabled ? (vault.withdrawal_available == null ? "—" : fmt(vault.withdrawal_available)) : "Unlimited"}
        </div>
        <div className="mt-3">
          {vault.wl_enabled ? null : <div className="h-1.5 w-full bg-sonar" />}
        </div>
      </div>
    </section>
  );
}
