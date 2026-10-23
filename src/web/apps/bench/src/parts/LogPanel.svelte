<script lang="ts">
  import { CopyButton, LogView, Panel } from "@petlibre/ui";
  import { app } from "../lib/bench.svelte";
  import { classify } from "../lib/log";

  let filter = $state("");
  let follow = $state(true);

  const needle = $derived(filter.trim().toLowerCase());
  const shown = $derived(needle ? app.lines.filter((l) => l.toLowerCase().includes(needle)) : app.lines);
  const current = $derived(app.snapshot?.current ?? null);
</script>

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
  <LogView lines={shown} {classify} bind:follow height="60vh" label="Live log" empty="Waiting for output from a bench command" />
</Panel>

<style>
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
</style>
