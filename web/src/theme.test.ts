import { describe, it, expect } from "vitest";
import { staleness, STALE_MS } from "./theme";
describe("staleness", () => {
  const t0 = new Date("2026-06-22T00:00:00Z").getTime();
  it("fresh within threshold", () => {
    expect(staleness("2026-06-22T00:00:00Z", t0 + STALE_MS - 1)).toBe("fresh");
  });
  it("warn past 30s", () => {
    expect(staleness("2026-06-22T00:00:00Z", t0 + STALE_MS + 1)).toBe("warn");
  });
  it("alert past 5min", () => {
    expect(staleness("2026-06-22T00:00:00Z", t0 + 5 * 60_000 + 1)).toBe("alert");
  });
});
