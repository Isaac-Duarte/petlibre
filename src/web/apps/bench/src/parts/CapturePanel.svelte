<script lang="ts">
  import { LogView, Panel, bytes, clockTime } from "@petlibre/ui";
  import { app } from "../lib/bench.svelte";

  const rx = $derived(app.snapshot?.rx ?? null);
</script>

<Panel title="Latest capture">
  {#snippet meta()}
    {#if rx}{bytes(rx.bytes)}{rx.modified ? `, updated ${clockTime(rx.modified)}` : ""}{/if}
  {/snippet}
  {#if rx}<p class="path"><code>{rx.path}</code></p>{/if}
  <LogView lines={rx?.tail ?? []} height="40vh" label="Latest capture" empty="No capture yet" />
</Panel>

<style>
  .path {
    margin: 0 0 var(--s-2);
    color: var(--muted);
    font-size: var(--t-xs);
    overflow-wrap: anywhere;
  }
</style>
