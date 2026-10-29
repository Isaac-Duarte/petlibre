<script lang="ts">
  // Stock as the local server last heard it. The page never asks the feeder
  // itself; the replacement firmware speaks no MQTT, so the server hears
  // only stock, while it installs a replacement or after a rollback.
  import { Command, Empty, Kv, LiveDot, Panel } from "@wellfed/ui";
  import { app } from "../lib/bench.svelte";

  const stock = $derived(app.stock);
</script>

<Panel title="Stock server">
  {#snippet meta()}
    {#if stock?.server === "up"}from wellfed-stock on <code>{stock.address}</code>{/if}
  {/snippet}

  {#if !stock}
    <Empty title="Asking the local server" />
  {:else if stock.server === "unconfigured"}
    <Empty title="No local server configured">
      The page reads stock through <code>wellfed-stock serve</code>, configured in
      <code>{stock.config}</code>.
    </Empty>
  {:else if stock.server === "down"}
    <div class="state">
      <LiveDot tone="muted" />
      <span class="headline">Server not running</span>
    </div>
    <p class="hint">
      Nothing answers on <code>{stock.address}</code>. An integration run starts its own server; to watch stock outside
      one:
    </p>
    <Command command="wellfed-stock serve --config private/server.json" />
  {:else}
    <div class="state">
      <LiveDot tone={stock.stock_online ? "pass" : "muted"} pulse={stock.stock_online} />
      <span class="headline">{stock.stock_online ? "Stock online" : "No stock telemetry"}</span>
    </div>
    <p class="hint">The replacement firmware answers its device API, not this server.</p>
    <Kv
      entries={[
        ["Stock version", stock.stock_version ?? "not reported"],
        ["Broker", stock.broker_connected ? "connected" : "not connected"],
      ]}
    />
  {/if}
</Panel>

<style>
  .state {
    display: flex;
    align-items: center;
    gap: var(--space-2);
  }
  .headline {
    font-size: var(--text-md);
    font-weight: 600;
  }
  .hint {
    color: var(--color-text-muted);
    font-size: var(--text-sm);
    margin: var(--space-2) 0 var(--space-3);
  }
</style>
