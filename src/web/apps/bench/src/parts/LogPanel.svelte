<script lang="ts">
  import { CopyButton, LogView, Panel } from "@wellfed/ui";
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
  <LogView
    lines={shown}
    {classify}
    bind:follow
    height="60vh"
    label="Live log"
    empty="Waiting for output from a bench command"
  />
</Panel>

<style>
  input[type="search"] {
    height: 30px;
    width: 12em;
    padding: 0 var(--space-3);
    border: 1px solid var(--color-line-strong);
    border-radius: var(--radius-pill);
    background: var(--color-surface);
    color: var(--color-text);
    font: inherit;
    font-size: var(--text-sm);
  }
  input[type="search"]:focus {
    outline: none;
    box-shadow: var(--focus);
  }
  .follow {
    display: flex;
    align-items: center;
    gap: 6px;
    font-size: var(--text-sm);
    color: var(--color-text-muted);
  }
  input[type="checkbox"] {
    accent-color: var(--color-text);
  }
  @media (max-width: 760px), (pointer: coarse) {
    input[type="search"] {
      flex: 1 1 8em;
      min-width: 0;
      height: 44px;
      font-size: var(--text-md);
    }
    .follow {
      min-height: 44px;
      padding: 0 var(--space-2);
    }
    input[type="checkbox"] {
      width: 20px;
      height: 20px;
    }
  }
</style>
