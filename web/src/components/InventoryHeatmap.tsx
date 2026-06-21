import type { Matrix } from "../api";
import { normalizeBand } from "../lib/heatmap";

const cellColor = (t: number, base: string) =>
  t === 0 ? "var(--abyss-800)" : `color-mix(in srgb, ${base} ${Math.round((0.2 + 0.8 * t) * 100)}%, var(--abyss-700))`;

function Band({ leaves, side }: { leaves: Matrix["page_leaves"]; side: "up" | "dn" }) {
  const norm = normalizeBand(leaves.map((l) => (side === "up" ? l.q_up : l.q_dn)));
  const base = side === "up" ? "var(--up)" : "var(--dn)";
  return (
    <div data-band={side} className="flex gap-px">
      {norm.map((t, i) => (
        <div key={i} data-cell title={`${side} ${leaves[i][side === "up" ? "q_up" : "q_dn"]}`}
             className="h-5 flex-1" style={{ background: cellColor(t, base) }} />
      ))}
    </div>
  );
}

const short = (id: string) => `${id.slice(0, 6)}…${id.slice(-4)}`;

export function InventoryHeatmap({ matrix }: { matrix: Matrix }) {
  const mintedNull = matrix.minted_min_strike == null;
  return (
    <div className="bg-abyss-800 p-4 border-t border-abyss-600">
      <div className="font-mono text-xs text-ink-400 flex gap-3 mb-3">
        <span className="text-ink-200">{short(matrix.matrix_object_id)}</span>
        <span>mtm {matrix.mtm.toFixed(2)}</span>
        <span className="text-abyss-600">|</span>
        <span>range_qty {matrix.range_qty} (raw)</span>
        <span className="text-abyss-600">|</span>
        <span>{mintedNull ? "none minted" : `minted ${matrix.minted_min_strike}–${matrix.minted_max_strike}`}</span>
      </div>
      <div className="flex flex-col gap-px">
        <Band leaves={matrix.page_leaves} side="up" />
        <Band leaves={matrix.page_leaves} side="dn" />
      </div>
      <div className="font-mono text-[10px] text-ink-600 mt-1">relative intensity (max-normalised) · {matrix.min_strike}–{matrix.max_strike}</div>
    </div>
  );
}
