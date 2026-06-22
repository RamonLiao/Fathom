import { describe, it, expect } from "vitest";
import { render, screen } from "@testing-library/react";
import { InventoryHeatmap } from "./InventoryHeatmap";
import type { Matrix } from "../api";

const m: Matrix = { matrix_object_id: "0xm", oracle_id: "0xo", matrix_version: 7,
  mtm: 12.3, range_qty: "18446744073709551615", min_strike: 50000, max_strike: 150000, tick_size: 1,
  minted_min_strike: null, minted_max_strike: null,
  page_leaves: [{ q_up: "10", q_dn: "0" }, { q_up: "0", q_dn: "20" }], ingested_at: "2026-06-22T00:00:00Z" };

describe("InventoryHeatmap", () => {
  it("shows 'none minted' when minted range is null", () => {
    render(<InventoryHeatmap matrix={m} />);
    expect(screen.getByText(/none minted/i)).toBeInTheDocument();
  });
  it("renders one cell per page leaf per band", () => {
    const { container } = render(<InventoryHeatmap matrix={m} />);
    expect(container.querySelectorAll("[data-band='up'] [data-cell]").length).toBe(2);
    expect(container.querySelectorAll("[data-band='dn'] [data-cell]").length).toBe(2);
  });
  it("does not crash on empty page_leaves", () => {
    render(<InventoryHeatmap matrix={{ ...m, page_leaves: [] }} />);
    expect(screen.getByText(/0xm/)).toBeInTheDocument();
  });
});
