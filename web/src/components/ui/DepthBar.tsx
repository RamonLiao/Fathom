export function DepthBar({ pct, danger = 0.8 }: { pct: number; danger?: number }) {
  const clamped = Math.max(0, Math.min(1, pct));
  const color = clamped >= danger ? "var(--alert)" : clamped >= danger * 0.6 ? "var(--warn)" : "var(--ok)";
  return (
    <div className="h-1.5 w-full bg-abyss-700 relative">
      <div className="h-full" style={{ width: `${clamped * 100}%`, background: color }} />
      <div className="absolute top-0 h-full w-px bg-ink-600" style={{ left: `${danger * 100}%` }} />
    </div>
  );
}
