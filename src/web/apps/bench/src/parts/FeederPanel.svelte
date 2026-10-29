<script lang="ts">
  // The feeder as the bench server knows it: the build it believes each
  // bank holds and which the feeder confirmed, then what its device API
  // session and the receive-only capture last read. Anything the server has
  // not read yet says so rather than guessing.
  import { Kv, LiveDot, LogView, Panel, clockTime } from "@wellfed/ui";
  import { app } from "../lib/bench.svelte";
  import { BANKS, checkText, confirmed, contentText, sourceText } from "../lib/banks";
  import {
    UNKNOWN,
    captureText,
    clockText,
    hotspotText,
    journalText,
    sessionText,
    sessionTone,
    startText,
    statusText,
  } from "../lib/feeder";

  const ledger = $derived(app.state?.banks ?? null);
  const running = $derived(ledger ? confirmed(ledger) : null);
  const feeder = $derived(app.state?.feeder ?? null);
  const capture = $derived(app.state?.capture ?? null);
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
  {#if feeder}
    <p class="session">
      <LiveDot tone={sessionTone(feeder.session)} pulse={feeder.session.state === "connecting"} />
      Session {sessionText(feeder.session)}
    </p>
    {#if feeder.other_sessions > 0}
      <p class="warning" role="alert">
        {feeder.other_sessions} other device API session{feeder.other_sessions === 1 ? "" : "s"} open on the feeder
      </p>
    {/if}
  {/if}
  <Kv
    entries={[
      ["Confirmed build", running ? `${contentText(running[1].content)}, bank ${running[0]}` : UNKNOWN],
      ["Status", feeder ? statusText(feeder) : UNKNOWN],
      ["Clock", feeder ? clockText(feeder) : UNKNOWN],
      ["Journal high water", feeder ? journalText(feeder) : UNKNOWN],
      ["Hotspot", hotspotText(feeder?.hotspot ?? null)],
      ["Capture", capture ? captureText(capture) : UNKNOWN],
      ["Last start banner", capture ? startText(capture) : UNKNOWN],
      [
        "Last update record",
        capture?.update ? `${capture.update.line} (${clockTime(capture.update.at, false)})` : UNKNOWN,
      ],
    ]}
  />
  <h3 class="log">Device log</h3>
  <LogView lines={feeder?.log ?? []} height="14em" label="Device log" empty={UNKNOWN} />
  <h3 class="log">UART</h3>
  <LogView lines={capture?.lines ?? []} height="14em" label="UART capture" empty="No UART lines yet" />
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
  .session {
    display: flex;
    align-items: center;
    gap: 6px;
    margin: 0 0 var(--space-2);
    font-size: var(--text-sm);
    overflow-wrap: anywhere;
  }
  .warning {
    margin: 0 0 var(--space-2);
    color: var(--color-fault);
    font-size: var(--text-sm);
  }
  .log {
    margin: var(--space-3) 0 var(--space-1);
    color: var(--color-text-muted);
    font-size: var(--text-sm);
    font-weight: normal;
  }
</style>
