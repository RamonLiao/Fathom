import { useVault, useOracles, useInventory } from "./api";
import { TopBar } from "./components/TopBar";
import { VaultHealth } from "./components/VaultHealth";
import { OracleTable } from "./components/OracleTable";
import { InventoryHeatmap } from "./components/InventoryHeatmap";

export default function App() {
  const vault = useVault();
  const oracles = useOracles();
  const inventory = useInventory();
  const now = Date.now();
  const anyError = vault.isError || oracles.isError || inventory.isError;

  return (
    <div className="max-w-[1400px] mx-auto">
      <TopBar live={!anyError} />
      {anyError && (
        <div className="bg-[rgba(229,72,77,0.12)] border-y border-alert text-alert font-mono text-sm px-6 py-2">
          API unreachable — showing last known data
        </div>
      )}
      <main className="p-6 space-y-6">
        {vault.data ? <VaultHealth vault={vault.data} now={now} />
                    : <div className="text-ink-400 font-mono">no vault data</div>}
        <section><OracleTable oracles={oracles.data ?? []} /></section>
        <section className="space-y-px">
          {(inventory.data ?? []).map((m) => <InventoryHeatmap key={m.matrix_object_id} matrix={m} />)}
        </section>
      </main>
    </div>
  );
}
