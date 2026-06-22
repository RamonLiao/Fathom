import { describe, it, expect } from "vitest";
import { render, screen } from "@testing-library/react";
import { OracleTable } from "./OracleTable";
import type { Oracle } from "../api";

const dirty: Oracle = { oracle_id: "0xdirty", a: 1, b: 2, rho: -0.3, m: -0.001, sigma: 0.5,
  svi_sanity: "dirty", svi_checkpoint_seq: 9, spot: 63000, forward: 63010, prices_checkpoint_seq: 9 };
const pricesOnly: Oracle = { oracle_id: "0xponly", a: null, b: null, rho: null, m: null, sigma: null,
  svi_sanity: null, svi_checkpoint_seq: null, spot: 63000, forward: 63010, prices_checkpoint_seq: 9 };

describe("OracleTable", () => {
  it("labels null sanity as PRICES-ONLY", () => {
    render(<OracleTable oracles={[pricesOnly]} />);
    expect(screen.getByText("PRICES-ONLY")).toBeInTheDocument();
  });
  it("marks dirty rows for highlight (the README selling point)", () => {
    const { container } = render(<OracleTable oracles={[dirty]} />);
    expect(container.querySelector("[data-dirty='true']")).toBeTruthy();
    expect(screen.getByText("DIRTY")).toBeInTheDocument();
  });
  it("renders — for null SVI params instead of crashing", () => {
    render(<OracleTable oracles={[pricesOnly]} />);
    expect(screen.getAllByText("—").length).toBeGreaterThan(0);
  });
});
