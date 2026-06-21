import { describe, it, expect, vi, afterEach } from "vitest";
import { fetchJson } from "./api";

afterEach(() => vi.restoreAllMocks());

describe("fetchJson", () => {
  it("returns parsed json on 200", async () => {
    vi.stubGlobal("fetch", vi.fn(async () =>
      new Response(JSON.stringify({ nav: 3 }), { status: 200 })));
    expect(await fetchJson<{ nav: number }>("/api/vault")).toEqual({ nav: 3 });
  });
  it("throws on 500 (does not swallow)", async () => {
    vi.stubGlobal("fetch", vi.fn(async () => new Response("internal error", { status: 500 })));
    await expect(fetchJson("/api/vault")).rejects.toThrow(/500/);
  });
  it("returns null body as null (empty vault)", async () => {
    vi.stubGlobal("fetch", vi.fn(async () => new Response("null", { status: 200 })));
    expect(await fetchJson("/api/vault")).toBeNull();
  });
});
