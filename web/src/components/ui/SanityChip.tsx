export function SanityChip({ sanity }: { sanity: "clean" | "dirty" | "untested" | null }) {
  const label = sanity == null ? "PRICES-ONLY" : sanity.toUpperCase();
  const color = sanity === "clean" ? "var(--ok)" : sanity === "dirty" ? "var(--alert)"
    : sanity === "untested" ? "var(--warn)" : "var(--ink-600)";
  return (
    <span className="inline-flex items-center gap-2 font-mono text-xs tracking-wide">
      <span className="inline-block h-2 w-2" style={{ background: color, borderRadius: 0 }} />
      {label}
    </span>
  );
}
