import { POLL_MS } from "../theme";

export function TopBar({ live }: { live: boolean }) {
  return (
    <header className="flex items-center justify-between px-6 py-3 border-b border-abyss-600">
      <span className="font-mono tracking-[0.3em] text-ink-200">FATHOM</span>
      <span className="font-mono text-xs text-ink-400 flex items-center gap-2">
        <span className="inline-block h-2 w-2 rounded-full" style={{ background: live ? "var(--sonar)" : "var(--alert)" }} />
        live · {POLL_MS / 1000}s
      </span>
    </header>
  );
}
