import { describe, it, expect } from "vitest";
import { render, screen } from "@testing-library/react";
import { VaultHealth } from "./VaultHealth";

const base = { object_version: 1, nav: 1019403, utilization: 0.0017, balance: 1017923,
  total_mtm: 1481, total_max_payout: 1692, withdrawal_available: null,
  wl_enabled: false, ingested_at: "2026-06-22T00:00:00Z" };

describe("VaultHealth", () => {
  it("shows Unlimited when withdrawal limiter disabled", () => {
    render(<VaultHealth vault={base} now={new Date(base.ingested_at).getTime()} />);
    expect(screen.getByText(/unlimited/i)).toBeInTheDocument();
  });
  it("flags stale data past threshold", () => {
    const now = new Date(base.ingested_at).getTime() + 31_000;
    const { container } = render(<VaultHealth vault={base} now={now} />);
    expect(container.querySelector("[data-stale='warn']")).toBeTruthy();
  });
  it("renders NAV figure", () => {
    render(<VaultHealth vault={base} now={new Date(base.ingested_at).getTime()} />);
    expect(screen.getByText(/1,019,403/)).toBeInTheDocument();
  });
});
