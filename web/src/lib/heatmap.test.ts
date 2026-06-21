import { describe, it, expect } from "vitest";
import { normalizeBand, pageStrike } from "./heatmap";

describe("normalizeBand", () => {
  it("max-normalizes raw u64 strings to 0..1", () => {
    expect(normalizeBand(["0", "50", "100"])).toEqual([0, 0.5, 1]);
  });
  it("all-zero band stays 0 (no divide-by-zero)", () => {
    expect(normalizeBand(["0", "0"])).toEqual([0, 0]);
  });
});
describe("pageStrike", () => {
  it("maps bucket index to strike across the range", () => {
    expect(pageStrike(0, 4, 50000, 150000)).toBe(50000);
    expect(pageStrike(3, 4, 50000, 150000)).toBe(150000);
  });
});
