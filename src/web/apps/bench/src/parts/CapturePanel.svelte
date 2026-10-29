<script lang="ts">
  import { LogView, Panel, bytes, clockTime } from "@wellfed/ui";
  import { app } from "../lib/bench.svelte";
  import { holdsUart } from "../lib/uart";

  const rx = $derived(app.evidence?.rx ?? null);
</script>

<Panel title="Latest capture">
  {#snippet meta()}
    {#if rx}{bytes(rx.bytes)}{rx.modified ? `, updated ${clockTime(rx.modified)}` : ""}{/if}
  {/snippet}
  {#if rx}<p class="path"><code>{rx.path}</code></p>{/if}
  <LogView
    lines={rx?.tail ?? []}
    height="40vh"
    label="Latest capture"
    empty={!app.evidence && holdsUart(app.state?.uart) ? "Deferred until the write ends" : "No capture yet"}
  />
</Panel>

<style>
  .path {
    margin: 0 0 var(--space-2);
    color: var(--color-text-muted);
    font-size: var(--text-xs);
    overflow-wrap: anywhere;
  }
</style>
