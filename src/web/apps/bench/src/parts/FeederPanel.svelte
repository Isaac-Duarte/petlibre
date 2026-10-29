<script lang="ts">
  // The feeder as the bench server knows it: the build it believes each
  // bank holds and which the feeder confirmed. The server's feeder session
  // (clock, journal, hotspot, device log) reports the rest; until it does,
  // those read as not known.
  import { Kv, LiveDot, Panel, clockTime } from "@wellfed/ui";
  import { app } from "../lib/bench.svelte";
  import { BANKS, checkText, confirmed, contentText, sourceText } from "../lib/banks";

  const UNKNOWN = "not known yet";
  const ledger = $derived(app.state?.banks ?? null);
  const running = $derived(ledger ? confirmed(ledger) : null);
</script>

<Panel title="Feeder">
  {#snippet meta()}as the bench server knows it{/snippet}
  <ul class="banks" aria-label="Banks">
    {#each BANKS as bank (bank)}
      {@const entry = ledger?.banks[bank]}
      <li>
        <span class="bank">Bank {bank}</span>
        {#if entry}
          <span class="content">
            <LiveDot
              tone={entry.check.state === "confirmed" ? "pass" : entry.check.state === "disputed" ? "fail" : "muted"}
            />
            {contentText(entry.content)}
          </span>
          <span class="how">
            {sourceText(entry.source)}{entry.session ? ` (${entry.session})` : ""} at {clockTime(entry.at, false)};
            <span class={entry.check.state}>{checkText(entry.check)}</span>
          </span>
        {:else}
          <span class="content muted">{UNKNOWN}</span>
        {/if}
      </li>
    {/each}
  </ul>
  <Kv
    entries={[
      ["Confirmed build", running ? `${contentText(running[1].content)}, bank ${running[0]}` : UNKNOWN],
      ["Clock synced", UNKNOWN],
      ["Journal high water", UNKNOWN],
      ["Hotspot", UNKNOWN],
    ]}
  />
  <p class="log">Device log: {UNKNOWN}</p>
</Panel>

<style>
  .banks {
    list-style: none;
    margin: 0 0 var(--space-4);
    padding: 0;
  }
  li {
    display: grid;
    grid-template-columns: 4.5em minmax(0, 1fr);
    gap: 2px var(--space-3);
    padding: 4px 0;
    font-size: var(--text-sm);
  }
  .bank {
    color: var(--color-text-muted);
  }
  .content {
    display: inline-flex;
    align-items: center;
    gap: 6px;
    font-family: var(--mono);
    font-size: var(--text-xs);
  }
  .how {
    grid-column: 2;
    color: var(--color-text-muted);
    font-size: var(--text-xs);
    overflow-wrap: anywhere;
  }
  .disputed {
    color: var(--color-fault);
  }
  .confirmed {
    color: var(--color-ok);
  }
  .muted {
    color: var(--color-text-muted);
  }
  .log {
    margin: var(--space-3) 0 0;
    color: var(--color-text-muted);
    font-size: var(--text-sm);
  }
</style>
