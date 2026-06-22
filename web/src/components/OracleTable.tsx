import type { Oracle } from "../api";
import { SanityChip } from "./ui/SanityChip";

const n = (v: number | null, d = 4) => (v == null ? "—" : v.toFixed(d));
const short = (id: string) => `${id.slice(0, 6)}…${id.slice(-4)}`;

export function OracleTable({ oracles }: { oracles: Oracle[] }) {
  return (
    <table className="w-full border-collapse font-mono text-sm">
      <thead>
        <tr className="text-ink-400 text-xs uppercase tracking-wider">
          {["Oracle", "Sanity", "Spot", "Forward", "a", "b", "rho", "m", "sigma"].map((h) => (
            <th key={h} className="text-right p-2 first:text-left">{h}</th>
          ))}
        </tr>
      </thead>
      <tbody>
        {oracles.map((o) => {
          const isDirty = o.svi_sanity === "dirty";
          return (
            <tr key={o.oracle_id} data-dirty={isDirty || undefined}
                className={isDirty ? "border-l-2 border-l-alert bg-[rgba(229,72,77,0.06)] animate-pulse-slow" : "border-l-2 border-l-transparent"}>
              <td className="p-2 text-left text-ink-200">{short(o.oracle_id)}</td>
              <td className="p-2 text-left"><SanityChip sanity={o.svi_sanity} /></td>
              <td className="p-2 text-right tnum">{n(o.spot, 2)}</td>
              <td className="p-2 text-right tnum">{n(o.forward, 2)}</td>
              <td className="p-2 text-right tnum">{n(o.a)}</td>
              <td className="p-2 text-right tnum">{n(o.b)}</td>
              <td className="p-2 text-right tnum" style={{ color: o.rho != null && o.rho < 0 ? "var(--up)" : undefined }}>{n(o.rho)}</td>
              <td className="p-2 text-right tnum">{n(o.m, 6)}</td>
              <td className="p-2 text-right tnum">{n(o.sigma)}</td>
            </tr>
          );
        })}
      </tbody>
    </table>
  );
}
