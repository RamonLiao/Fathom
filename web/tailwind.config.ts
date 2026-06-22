import type { Config } from "tailwindcss";
export default {
  content: ["./index.html", "./src/**/*.{ts,tsx}"],
  theme: { extend: { colors: {
    abyss: { 900: "var(--abyss-900)", 800: "var(--abyss-800)", 700: "var(--abyss-700)", 600: "var(--abyss-600)" },
    ink: { 200: "var(--ink-200)", 400: "var(--ink-400)", 600: "var(--ink-600)" },
    sonar: { DEFAULT: "var(--sonar)", dim: "var(--sonar-dim)" },
    ok: "var(--ok)", warn: "var(--warn)", alert: "var(--alert)",
    up: "var(--up)", dn: "var(--dn)",
  }, fontFamily: { mono: ["IBM Plex Mono", "monospace"], sans: ["Inter Tight", "IBM Plex Sans", "sans-serif"] } } },
  plugins: [],
} satisfies Config;
