import { describe, it, expect, vi, afterEach } from "vitest";
import { render, screen, waitFor } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import App from "./App";

const wrap = () => render(
  <QueryClientProvider client={new QueryClient({ defaultOptions: { queries: { retry: false } } })}><App /></QueryClientProvider>
);
afterEach(() => vi.restoreAllMocks());

describe("App", () => {
  it("shows API-unreachable banner when fetch fails", async () => {
    vi.stubGlobal("fetch", vi.fn(async () => new Response("err", { status: 500 })));
    wrap();
    await waitFor(() => expect(screen.getByText(/api unreachable/i)).toBeInTheDocument());
  });
  it("shows no-data placeholder when vault is null", async () => {
    vi.stubGlobal("fetch", vi.fn(async (url: string) =>
      new Response(url.includes("vault") ? "null" : "[]", { status: 200 })));
    wrap();
    await waitFor(() => expect(screen.getByText(/no vault data/i)).toBeInTheDocument());
  });
});
