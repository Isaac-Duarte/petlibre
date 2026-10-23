<script lang="ts">
  import { CopyButton, LogView, Panel, bytes, clockTime } from "@wellfed/ui";
  import { app } from "../lib/bench.svelte";
  import { classify } from "../lib/log";

  let filter = $state("");
  let follow = $state(true);

  const needle = $derived(filter.trim().toLowerCase());
  const shown = $derived(needle ? app.lines.filter((l) => l.toLowerCase().includes(needle)) : app.lines);
  const rx = $derived(app.snapshot?.rx ?? null);
  const current = $derived(app.snapshot?.current ?? null);
</script>

<div class="layout">
  <Panel title="Live log">
    {#snippet meta()}
      {#if current}<code>{current.command}{current.name ? ` ${current.name}` : ""}</code>{/if}
      {app.lines.length ? `${app.lines.length} lines${needle ? `, ${shown.length} shown` : ""}` : ""}
    {/snippet}
    {#snippet actions()}
      <input id="log-filter" type="search" placeholder="Filter  /" aria-label="Filter log lines" bind:value={filter} />
      <label class="follow"><input type="checkbox" bind:checked={follow} /> Follow</label>
      <CopyButton text={shown.join("\n")} />
    {/snippet}
    <LogView lines={shown} {classify} bind:follow height="calc(100vh - 260px)" label="Live log" empty="Waiting for output from a bench command" />
  </Panel>

  <Panel title="Latest capture">
    {#snippet meta()}
      {#if rx}{bytes(rx.bytes)}{rx.modified ? `, updated ${clockTime(rx.modified)}` : ""}{/if}
    {/snippet}
    {#if rx}<p class="path"><code>{rx.path}</code></p>{/if}
    <LogView lines={rx?.tail ?? []} height="calc(100vh - 300px)" label="Latest capture" empty="No capture yet" />
  </Panel>
</div>

<style>
  .layout {
    display: grid;
    grid-template-columns: minmax(0, 3fr) minmax(0, 2fr);
    gap: var(--s-5);
    align-items: start;
  }
  input[type="search"] {
    height: 30px;
    width: 12em;
    padding: 0 var(--s-3);
    border: 1px solid var(--line-strong);
    border-radius: var(--pill);
    background: var(--surface);
    color: var(--ink);
    font: inherit;
    font-size: var(--t-sm);
  }
  input[type="search"]:focus {
    outline: none;
    box-shadow: var(--focus);
  }
  .follow {
    display: flex;
    align-items: center;
    gap: 6px;
    font-size: var(--t-sm);
    color: var(--muted);
  }
  input[type="checkbox"] {
    accent-color: var(--ink);
  }
  .path {
    margin: 0 0 var(--s-2);
    color: var(--muted);
    font-size: var(--t-xs);
    overflow-wrap: anywhere;
  }
  @media (max-width: 980px) {
    .layout {
      grid-template-columns: minmax(0, 1fr);
    }
  }
</style>
