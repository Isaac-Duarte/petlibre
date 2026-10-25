<script lang="ts">
  // Evidence by session name: the loader capture, reads, plan, execute and
  // boot a flash session leaves under private/, with the reviewed next step.
  import { bench, type SessionView } from "@wellfed/api";
  import { Badge, Command, Empty, Kv, LiveDot, Panel, ago, bytes, fullTime, toneOf } from "@wellfed/ui";
  import { app } from "../lib/bench.svelte";
  import { route } from "../lib/route.svelte";

  const sessions = $derived(app.sessions?.sessions ?? null);
  const selected = $derived(route.tab === "sessions" ? route.detail : null);

  let detail = $state<SessionView | null>(null);
  // The detail only once it is the selected session's, not the last one's.
  const shown = $derived(detail?.name === selected ? detail : null);
  $effect(() => {
    const name = selected;
    // eslint-disable-next-line @typescript-eslint/no-unused-expressions -- read so the effect re-runs with the list
    app.sessions;
    if (!name) {
      detail = null;
      return;
    }
    bench
      .session(name)
      .then((view) => {
        if (name === selected) detail = view;
      })
      .catch(() => {
        // Keep the last detail shown; the list's own refresh reports a dead server.
      });
  });

  function facts(part: SessionView["parts"][number]): [string, unknown][] {
    if ("plan" in part) return fields(part.plan);
    const report = fields(part.report);
    const entries = report.filter(([key]) => key !== "status" && key !== "result");
    entries.push(...fields(report.find(([key]) => key === "result")?.[1]));
    if (part.serial) entries.push(["serial sent", bytes(part.serial.tx)], ["serial received", bytes(part.serial.rx)]);
    return entries;
  }

  /** A JSON object's fields; nothing for anything else. */
  function fields(value: unknown): [string, unknown][] {
    return value !== null && typeof value === "object" && !Array.isArray(value) ? Object.entries(value) : [];
  }

  function stateOf(part: SessionView["parts"][number]): string {
    if ("plan" in part) return "prepared";
    const status = fields(part.report).find(([key]) => key === "status")?.[1];
    return typeof status === "string" ? status : "no report";
  }
</script>

<div class="layout" class:open={!!selected}>
  <Panel title="Sessions" flush>
    {#snippet meta()}{sessions ? `${sessions.length} newest` : ""}{/snippet}
    {#if !sessions}
      <Empty title="Reading evidence" />
    {:else if !sessions.length}
      <Empty title="No session evidence under private/ yet" />
    {:else}
      <table>
        <thead><tr><th>Name</th><th class="wide">Evidence</th><th>State</th><th class="r">Age</th></tr></thead>
        <tbody>
          {#each sessions as s (s.name)}
            <tr class:sel={s.name === selected}>
              <td class="name"><a href={route.href("sessions", s.name === selected ? null : s.name)}>{s.name}</a></td>
              <td class="wide"
                ><span class="parts"
                  >{#each s.parts as p (p)}<span class="part">{p}</span>{/each}</span
                ></td
              >
              <td class="state"><LiveDot tone={toneOf(s.outcome)} /> {s.outcome}</td>
              <td class="r muted" title={fullTime(s.modified)}>{ago(s.modified, app.now)}</td>
            </tr>
          {/each}
        </tbody>
      </table>
    {/if}
  </Panel>

  {#if selected}
    <Panel title={selected}>
      {#snippet actions()}
        <a class="close" href={route.href("sessions")}>Close</a>
      {/snippet}
      {#if !shown}
        <Empty title="Reading the session" />
      {:else}
        {#each shown.parts as part (part.kind)}
          <section class="part-detail">
            <header>
              <h3>{part.kind}</h3>
              <code class="path">{part.path}</code>
              <Badge tone={toneOf(stateOf(part))} label={stateOf(part)} />
            </header>
            <Kv entries={facts(part)} />
          </section>
        {/each}
        {#if shown.next}<Command caption="Execute after review" command={shown.next} />{/if}
      {/if}
    </Panel>
  {/if}
</div>

<style>
  .layout {
    display: grid;
    grid-template-columns: minmax(0, 1fr);
    gap: var(--space-5);
    align-items: start;
  }
  .layout.open {
    grid-template-columns: minmax(0, 1.1fr) minmax(0, 1fr);
  }
  table {
    width: 100%;
    border-collapse: collapse;
    font-size: var(--text-sm);
  }
  th {
    text-align: left;
    font-weight: 500;
    font-size: var(--text-xs);
    color: var(--color-text-muted);
    padding: 0 var(--space-2) 6px;
  }
  td {
    padding: 8px var(--space-2);
    border-top: 1px solid var(--color-line);
    white-space: nowrap;
  }
  th:first-child,
  td:first-child {
    padding-left: var(--space-5);
  }
  th:last-child,
  td:last-child {
    padding-right: var(--space-5);
  }
  tr.sel td {
    background: var(--color-sunken);
  }
  tr.sel td:first-child {
    box-shadow: inset 3px 0 0 var(--color-text);
  }
  .name a {
    font-family: var(--mono);
    font-size: var(--text-xs);
    color: var(--color-text);
    text-decoration: none;
  }
  .name a:hover {
    text-decoration: underline;
  }
  .parts {
    display: flex;
    flex-wrap: wrap;
    gap: 4px;
  }
  td.wide {
    white-space: normal;
  }
  .part {
    padding: 1px 8px;
    border-radius: var(--radius-pill);
    background: var(--color-sunken);
    color: var(--color-text-muted);
    font-size: var(--text-xs);
  }
  /* The selected row is sunken too, so its pills need an edge to stay visible. */
  tr.sel .part {
    background: var(--color-surface);
    box-shadow: inset 0 0 0 1px var(--color-line);
  }
  .state {
    display: flex;
    align-items: center;
    gap: var(--space-2);
  }
  .r {
    text-align: right;
  }
  .muted {
    color: var(--color-text-muted);
  }
  .close {
    font-size: var(--text-sm);
  }
  .part-detail {
    padding-bottom: var(--space-4);
    margin-bottom: var(--space-4);
    border-bottom: 1px solid var(--color-line);
  }
  .part-detail header {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: var(--space-2) var(--space-3);
    margin-bottom: var(--space-3);
  }
  .part-detail h3 {
    text-transform: capitalize;
  }
  .path {
    color: var(--color-text-muted);
    font-size: var(--text-xs);
    overflow-wrap: anywhere;
    margin-right: auto;
  }
  @media (max-width: 980px) {
    .layout.open {
      grid-template-columns: minmax(0, 1fr);
    }
  }
  /* On a phone each session is a card: its name as a full-width tap target,
     then its state, age and evidence parts. */
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
      grid-template-columns: auto minmax(0, 1fr);
      grid-template-areas: "name name" "state age" "parts parts";
      gap: var(--space-1) var(--space-3);
      padding: 0 var(--space-4) var(--space-3);
      border-top: 1px solid var(--color-line);
    }
    td,
    td:first-child,
    td:last-child {
      padding: 0;
      border: 0;
    }
    tr.sel {
      background: var(--color-sunken);
      box-shadow: inset 3px 0 0 var(--color-text);
    }
    tr.sel td,
    tr.sel td:first-child {
      background: none;
      box-shadow: none;
    }
    td.name {
      grid-area: name;
    }
    td.wide {
      grid-area: parts;
    }
    td.state {
      grid-area: state;
    }
    td.r {
      grid-area: age;
    }
    .name a {
      display: flex;
      align-items: center;
      min-height: 44px;
      font-size: var(--text-sm);
      overflow-wrap: anywhere;
      white-space: normal;
    }
  }
  @media (max-width: 760px), (pointer: coarse) {
    .close {
      display: inline-flex;
      align-items: center;
      min-height: 44px;
      padding: 0 var(--space-2);
    }
  }
</style>
