<script lang="ts">
  import type { ScenarioVerdict } from "@wellfed/api";
  import { Badge, Empty, Panel, ago, duration } from "@wellfed/ui";
  import { app } from "../lib/bench.svelte";
  import { route } from "../lib/route.svelte";
  import { liveRun } from "../lib/tests";
  import Verdict from "./Verdict.svelte";

  const tests = $derived(app.tests);
  const live = $derived(liveRun(app.snapshot, tests?.catalog ?? [], app.now));
  const latest = $derived(tests?.runs[0] ?? null);
  const count = (verdict: ScenarioVerdict) => latest?.scenarios.filter((s) => s.verdict === verdict).length ?? 0;
</script>

<Panel title={live ? "Integration run in progress" : "Latest integration run"}>
  {#snippet actions()}
    <a class="more" href={route.href("tests")}>All runs</a>
  {/snippet}

  {#if live}
    <div class="line">
      <span class="run">{live.name}</span>
      <span class="muted">{duration((live.finished ?? app.now) - live.started)}</span>
    </div>
    <ul class="chips">
      {#each live.scenarios as s (s.name)}
        {@const v = s.step.state === "done" ? "pass" : s.step.state === "failed" ? "fail" : s.step.state === "skipped" ? "skip" : s.step.state}
        <li><Verdict verdict={v} live={v === "running"} title="{s.name}: {s.step.state}" /><span>{s.name}</span></li>
      {/each}
    </ul>
  {:else if latest}
    <div class="line">
      <a class="run" href={route.href("tests", latest.name)}>{latest.name}</a>
      <span class="muted">{ago(latest.modified, app.now)} ago</span>
      {#if latest.silent}<Badge tone="fail" label="Feeder went silent" />{/if}
    </div>
    <ul class="chips">
      {#each latest.scenarios as s (s.scenario)}
        <li><Verdict verdict={s.verdict} title="{s.scenario}: {s.verdict}{s.reason ? `, ${s.reason}` : ''}" /><span>{s.scenario}</span></li>
      {/each}
    </ul>
    <p class="sum">
      {count("pass")} passed, {count("fail")} failed, {count("skip")} skipped
    </p>
  {:else}
    <Empty title="No integration runs recorded">
      Runs of <code>wellfed-bench itest</code> appear here when they finish.
    </Empty>
  {/if}
</Panel>

<style>
  .more {
    font-size: var(--t-sm);
  }
  .line {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: var(--s-2) var(--s-3);
  }
  .run {
    font-family: var(--mono);
    font-size: var(--t-sm);
    font-weight: 600;
  }
  .muted {
    color: var(--muted);
    font-size: var(--t-sm);
  }
  .chips {
    display: flex;
    flex-wrap: wrap;
    gap: var(--s-2) var(--s-4);
    margin: var(--s-4) 0 0;
    padding: 0;
    list-style: none;
    font-size: var(--t-sm);
  }
  .chips li {
    display: inline-flex;
    align-items: center;
    gap: var(--s-2);
  }
  .sum {
    margin: var(--s-4) 0 0;
    color: var(--muted);
    font-size: var(--t-sm);
  }
</style>
