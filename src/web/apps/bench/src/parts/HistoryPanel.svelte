<script lang="ts">
  import { Empty, LiveDot, Panel, clockTime, duration, fullTime, toneOf } from "@wellfed/ui";
  import { app } from "../lib/bench.svelte";

  let all = $state(false);
  const SHORT = 8;

  const history = $derived(app.evidence?.history ?? []);
  const shown = $derived(all ? history : history.slice(0, SHORT));
</script>

<Panel title="Recent commands" flush>
  {#snippet actions()}
    {#if history.length > SHORT}
      <button type="button" class="toggle" onclick={() => (all = !all)}
        >{all ? "Show fewer" : `Show all ${history.length}`}</button
      >
    {/if}
  {/snippet}

  {#if history.length}
    <table>
      <thead><tr><th>Command</th><th class="r started">Started</th><th class="r">Took</th><th>Outcome</th></tr></thead>
      <tbody>
        {#each shown as h (h.started)}
          <tr>
            <td title={h.name ?? undefined}>
              {h.command}
              {#if h.name}<span class="sub">{h.name}</span>{/if}
            </td>
            <td class="r muted started" title={fullTime(h.started)}>{clockTime(h.started, false)}</td>
            <td class="r muted">{h.finished ? duration(h.finished - h.started) : "–"}</td>
            <td class="outcome" title={h.error ?? h.outcome}>
              <LiveDot tone={toneOf(h.outcome)} />
              <span>{h.outcome}{h.error ? `: ${h.error}` : ""}</span>
            </td>
          </tr>
        {/each}
      </tbody>
    </table>
  {:else}
    <Empty title="No commands recorded yet" />
  {/if}
</Panel>

<style>
  .toggle {
    border: 0;
    background: none;
    color: var(--color-accent);
    font-size: var(--text-sm);
    cursor: pointer;
    padding: 0;
  }
  table {
    width: 100%;
    border-collapse: collapse;
    font-size: var(--text-sm);
  }
  th {
    text-align: left;
    font-weight: 500;
    color: var(--color-text-muted);
    font-size: var(--text-xs);
    padding: 0 var(--space-2) 6px;
  }
  td {
    padding: 8px var(--space-2);
    border-top: 1px solid var(--color-line);
    vertical-align: top;
  }
  th:first-child,
  td:first-child {
    padding-left: var(--space-5);
  }
  th:last-child,
  td:last-child {
    padding-right: var(--space-5);
  }
  .r {
    text-align: right;
    white-space: nowrap;
  }
  .muted {
    color: var(--color-text-muted);
  }
  table {
    table-layout: fixed;
  }
  th:first-child {
    width: 42%;
  }
  td:first-child {
    overflow: hidden;
  }
  .sub {
    display: block;
    font-family: var(--mono);
    font-size: var(--text-xs);
    color: var(--color-text-muted);
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }
  .outcome {
    display: flex;
    align-items: baseline;
    gap: var(--space-2);
    min-width: 0;
  }
  .outcome span {
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  /* A phone has no room for four columns: each command becomes a two-line
     card, with its name first and the outcome, start and duration below,
     so nothing the desktop shows is dropped. */
  @media (max-width: 600px) {
    thead {
      position: absolute;
      width: 1px;
      height: 1px;
      overflow: hidden;
      clip-path: inset(50%);
    }
    tr {
      display: grid;
      grid-template-columns: minmax(0, 1fr) auto auto;
      grid-template-areas: "command command command" "outcome started took";
      gap: 2px var(--space-3);
      padding: var(--space-2) var(--space-4);
      border-top: 1px solid var(--color-line);
    }
    td,
    td:first-child,
    td:last-child {
      padding: 0;
      border: 0;
    }
    td:nth-child(1) {
      grid-area: command;
    }
    td:nth-child(2) {
      grid-area: started;
    }
    td:nth-child(3) {
      grid-area: took;
    }
    td:nth-child(4) {
      grid-area: outcome;
    }
    .sub,
    .outcome span {
      white-space: normal;
      overflow-wrap: anywhere;
    }
    .started::before {
      content: "started ";
    }
    .toggle {
      min-height: 44px;
    }
  }
  .outcome :global(.dot) {
    transform: translateY(-1px);
  }
</style>
