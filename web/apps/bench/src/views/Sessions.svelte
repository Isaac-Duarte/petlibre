<script lang="ts">
  // Evidence by session name: the loader capture, reads, plan, execute and
  // boot a flash session leaves under private/, with the reviewed next step.
  import { bench, type SessionView } from "@petlibre/api";
  import { Badge, Command, Empty, Kv, LiveDot, Panel, ago, bytes, fullTime, toneOf } from "@petlibre/ui";
  import { app } from "../lib/bench.svelte";
  import { route } from "../lib/route.svelte";

  const sessions = $derived(app.sessions?.sessions ?? null);
  const selected = $derived(route.tab === "sessions" ? route.detail : null);

  let detail = $state<SessionView | null>(null);
  $effect(() => {
    const name = selected;
    void app.sessions; // re-read with the list
    if (!name) {
      detail = null;
      return;
    }
    bench
      .session(name)
      .then((view) => {
        if (name === selected) detail = view;
      })
      .catch(() => {});
  });

  function facts(part: SessionView["parts"][number]): [string, unknown][] {
    if ("plan" in part) return Object.entries((part.plan as Record<string, unknown> | null) ?? {});
    const report = (part.report as Record<string, unknown> | null) ?? {};
    const { result, ...rest } = report;
    const entries = Object.entries(rest).filter(([key]) => key !== "status");
    if (result && typeof result === "object") entries.push(...Object.entries(result));
    if (part.serial) entries.push(["serial sent", bytes(part.serial.tx)], ["serial received", bytes(part.serial.rx)]);
    return entries;
  }

  function stateOf(part: SessionView["parts"][number]): string {
    if ("plan" in part) return "prepared";
    const report = part.report as { status?: string } | null;
    return report?.status ?? "no report";
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
              <td class="wide"><span class="parts">{#each s.parts as p (p)}<span class="part">{p}</span>{/each}</span></td>
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
      {#if !detail || detail.name !== selected}
        <Empty title="Reading the session" />
      {:else}
        {#each detail.parts as part (part.kind)}
          <section class="part-detail">
            <header>
              <h3>{part.kind}</h3>
              <code class="path">{part.path}</code>
              <Badge tone={toneOf(stateOf(part))} label={stateOf(part)} />
            </header>
            <Kv entries={facts(part)} />
          </section>
        {/each}
        {#if detail.next}<Command caption="Execute after review" command={detail.next} />{/if}
      {/if}
    </Panel>
  {/if}
</div>

<style>
  .layout {
    display: grid;
    grid-template-columns: minmax(0, 1fr);
    gap: var(--s-5);
    align-items: start;
  }
  .layout.open {
    grid-template-columns: minmax(0, 1.1fr) minmax(0, 1fr);
  }
  table {
    width: 100%;
    border-collapse: collapse;
    font-size: var(--t-sm);
  }
  th {
    text-align: left;
    font-weight: 500;
    font-size: var(--t-xs);
    color: var(--muted);
    padding: 0 var(--s-2) 6px;
  }
  td {
    padding: 8px var(--s-2);
    border-top: 1px solid var(--line);
    white-space: nowrap;
  }
  th:first-child,
  td:first-child {
    padding-left: var(--s-5);
  }
  th:last-child,
  td:last-child {
    padding-right: var(--s-5);
  }
  tr.sel td {
    background: var(--sunken);
  }
  tr.sel td:first-child {
    box-shadow: inset 3px 0 0 var(--ink);
  }
  .name a {
    font-family: var(--mono);
    font-size: var(--t-xs);
    color: var(--ink);
    text-decoration: none;
  }
  .name a:hover {
    text-decoration: underline;
  }
  .parts {
    display: flex;
    gap: 4px;
  }
  .part {
    padding: 1px 8px;
    border-radius: var(--pill);
    background: var(--sunken);
    color: var(--muted);
    font-size: var(--t-xs);
  }
  .state {
    display: flex;
    align-items: center;
    gap: var(--s-2);
  }
  .r {
    text-align: right;
  }
  .muted {
    color: var(--muted);
  }
  .close {
    font-size: var(--t-sm);
  }
  .part-detail {
    padding-bottom: var(--s-4);
    margin-bottom: var(--s-4);
    border-bottom: 1px solid var(--line);
  }
  .part-detail header {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: var(--s-2) var(--s-3);
    margin-bottom: var(--s-3);
  }
  .part-detail h3 {
    text-transform: capitalize;
  }
  .path {
    color: var(--muted);
    font-size: var(--t-xs);
    overflow-wrap: anywhere;
    margin-right: auto;
  }
  @media (max-width: 980px) {
    .layout.open {
      grid-template-columns: minmax(0, 1fr);
    }
    .wide {
      display: none;
    }
  }
</style>
