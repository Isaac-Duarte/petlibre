<script lang="ts">
  // The run in progress, scenario by scenario: what each checks, how long
  // it has taken, what the running one is waiting for, and its UART lines.
  import { Badge, Panel, duration, toneOf } from "@wellfed/ui";
  import { app } from "../lib/bench.svelte";
  import { lastWait, scenarioSections } from "../lib/log";
  import type { LiveRun } from "../lib/tests";
  import Verdict from "./Verdict.svelte";

  let { live }: { live: LiveRun } = $props();

  const sections = $derived(scenarioSections(app.lines));
  const running = $derived(live.running);
  const notes = $derived(running ? (sections.get(running.name) ?? []).slice(1).slice(-14) : []);
  const wait = $derived(running ? lastWait(sections.get(running.name)) : null);
</script>

<Panel title="Run in progress">
  {#snippet meta()}<code>{live.name}</code>, {duration((live.finished ?? app.now) - live.started)}{/snippet}
  {#snippet actions()}<Badge tone={toneOf(live.outcome)} label={live.outcome} />{/snippet}

  <div class="layout">
    <ol class="list">
      {#each live.scenarios as s (s.name)}
        {@const v =
          s.step.state === "done"
            ? "pass"
            : s.step.state === "failed"
              ? "fail"
              : s.step.state === "skipped"
                ? "skip"
                : s.step.state}
        <li class={s.step.state}>
          <Verdict verdict={v} live={v === "running"} size={14} />
          <div class="text">
            <span class="name">{s.name}</span>
            {#if s.about}<span class="about">{s.about}</span>{/if}
          </div>
          <span class="time">{s.elapsed === null ? "" : duration(s.elapsed)}</span>
        </li>
      {/each}
    </ol>

    <div class="focus">
      {#if running}
        <h3>{running.name}</h3>
        {#if wait}<p class="wait">{wait}</p>{/if}
        {#if notes.length}
          <pre class="notes">{notes.map((l) => l.trim()).join("\n")}</pre>
        {:else}
          <p class="quiet">No notes from this scenario yet.</p>
        {/if}
      {:else}
        <p class="quiet">Between scenarios</p>
      {/if}
    </div>
  </div>
</Panel>

<style>
  .layout {
    display: grid;
    grid-template-columns: minmax(0, 1fr) minmax(0, 1.2fr);
    gap: var(--space-5);
  }
  .list {
    list-style: none;
    margin: 0;
    padding: 0;
  }
  .list li {
    display: flex;
    align-items: flex-start;
    gap: var(--space-3);
    padding: 7px 0;
    border-top: 1px solid var(--color-line);
  }
  .list li:first-child {
    border-top: 0;
  }
  .list :global(.cell) {
    margin-top: 3px;
  }
  .text {
    display: flex;
    flex-direction: column;
    min-width: 0;
  }
  .name {
    font-family: var(--mono);
    font-size: var(--text-xs);
    font-weight: 600;
  }
  .about {
    color: var(--color-text-muted);
    font-size: var(--text-sm);
  }
  .pending .name,
  .skipped .name {
    color: var(--color-text-muted);
    font-weight: 400;
  }
  .running .name {
    color: var(--color-accent);
  }
  .time {
    margin-left: auto;
    color: var(--color-text-muted);
    font-size: var(--text-xs);
    white-space: nowrap;
    font-variant-numeric: tabular-nums;
  }
  .focus h3 {
    font-family: var(--mono);
    font-size: var(--text-sm);
    color: var(--color-accent);
  }
  .wait {
    margin: var(--space-2) 0;
    padding: var(--space-2) var(--space-3);
    background: var(--color-warning-soft);
    color: var(--color-warning);
    border-radius: var(--radius-control);
    font-size: var(--text-sm);
  }
  .notes {
    margin: var(--space-2) 0 0;
    padding: var(--space-3);
    background: var(--color-sunken);
    border-radius: var(--radius-control);
    font-size: var(--text-xs);
    line-height: 1.6;
    white-space: pre-wrap;
    overflow-wrap: anywhere;
    max-height: 340px;
    overflow: auto;
  }
  .quiet {
    color: var(--color-text-muted);
    font-size: var(--text-sm);
  }
  @media (max-width: 860px) {
    .layout {
      grid-template-columns: minmax(0, 1fr);
    }
  }
</style>
