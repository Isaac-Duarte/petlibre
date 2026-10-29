<script lang="ts">
  // Stock as the local server last heard it. The page never asks the feeder
  // itself; the replacement firmware speaks no MQTT, so the server hears
  // only stock, while it installs a replacement or after a rollback.
  import { Command, Empty, Kv, LiveDot, Panel } from "@wellfed/ui";
  import { app } from "../lib/bench.svelte";

  const feeder = $derived(app.feeder);
</script>

<Panel title="Stock server">
  {#snippet meta()}
    {#if feeder?.server === "up"}from wellfed-stock on <code>{feeder.address}</code>{/if}
  {/snippet}

  {#if !feeder}
    <Empty title="Asking the local server" />
  {:else if feeder.server === "unconfigured"}
    <Empty title="No local server configured">
      The page reads stock through <code>wellfed-stock serve</code>, configured in
      <code>{feeder.config}</code>.
    </Empty>
  {:else if feeder.server === "down"}
    <div class="state">
      <LiveDot tone="muted" />
      <span class="headline">Server not running</span>
    </div>
    <p class="hint">
      Nothing answers on <code>{feeder.address}</code>. An integration run starts its own server; to watch stock outside
      one:
    </p>
    <Command command="wellfed-stock serve --config private/server.json" />
  {:else}
    <div class="state">
      <LiveDot tone={feeder.stock_online ? "pass" : "muted"} pulse={feeder.stock_online} />
      <span class="headline">{feeder.stock_online ? "Stock online" : "No stock telemetry"}</span>
    </div>
    <p class="hint">The replacement firmware answers its device API, not this server.</p>
    <Kv
      entries={[
        ["Stock version", feeder.stock_version ?? "not reported"],
        ["Broker", feeder.broker_connected ? "connected" : "not connected"],
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
