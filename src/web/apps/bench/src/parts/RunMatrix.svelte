<script lang="ts">
  // Every run against every scenario: the shape of the suite's history at
  // a glance. Newest on the left, so the column that matters is first; a
  // run in progress fills in live as its first column.
  import { Badge, Empty, Panel, duration, fullTime, percent } from "@wellfed/ui";
  import type { ScenarioVerdict, TestsView } from "@wellfed/api";
  import { route } from "../lib/route.svelte";
  import { passRate, runLabel, type LiveRun } from "../lib/tests";
  import Verdict from "./Verdict.svelte";

  let { tests, live }: { tests: TestsView; live: LiveRun | null } = $props();

  const COLUMNS = 20;
  const runs = $derived(tests.runs.slice(0, COLUMNS));

  type Cell = ScenarioVerdict | "running" | "pending" | null;
  function liveCell(scenario: string): Cell {
    const step = live?.scenarios.find((s) => s.name === scenario)?.step;
    if (!step) return null;
    return step.state === "done"
      ? "pass"
      : step.state === "failed"
        ? "fail"
        : step.state === "skipped"
          ? "skip"
          : step.state;
  }

  function cellTitle(run: string, scenario: string, index: number): string {
    const record = tests.runs[index]?.scenarios.find((s) => s.scenario === scenario);
    if (!record) return `${run}: ${scenario} not in this run`;
    const reason = record.reason ? `\n${record.reason}` : "";
    const took = record.verdict === "skip" ? "" : ` in ${duration(record.seconds)}`;
    return `${run}: ${scenario} ${record.verdict}${took}${reason}`;
  }
</script>

<Panel title="Run history" flush>
  {#snippet meta()}
    {tests.runs.length} recorded {tests.runs.length === 1 ? "run" : "runs"}{tests.runs.length > COLUMNS
      ? `, newest ${COLUMNS} shown`
      : ""}
  {/snippet}
  {#snippet actions()}
    <ul class="legend" aria-label="Legend">
      <li><Verdict verdict="pass" size={12} /> pass</li>
      <li><Verdict verdict="fail" size={12} /> fail</li>
      <li><Verdict verdict="skip" size={12} /> skip</li>
    </ul>
  {/snippet}

  {#if !runs.length && !live}
    <Empty title="No integration runs recorded">
      A run writes <code>private/verification/NAME-itest.json</code> when it ends; build one below.
    </Empty>
  {:else}
    <div class="scroll">
      <table>
        <thead>
          <tr>
            <th class="scenario">Scenario</th>
            {#if live}
              <th class="run now" title="{live.name}, in progress">now</th>
            {/if}
            {#each runs as run (run.name)}
              <th class="run" class:silent={run.silent}>
                <a
                  href={route.href("tests", run.name)}
                  title="{run.name}, {fullTime(run.modified)}{run.silent ? `\nStopped: ${run.silent}` : ''}"
                  >{runLabel(run.name)}</a
                >
              </th>
            {/each}
            <th class="stat">Passed</th>
            <th class="stat">Median</th>
            <th class="why">Last failure</th>
          </tr>
        </thead>
        <tbody>
          {#each tests.scenarios as stats (stats.scenario)}
            {@const entry = tests.catalog.find((c) => c.name === stats.scenario)}
            {@const rate = passRate(stats)}
            <tr>
              <th class="scenario" scope="row" title={entry?.about ?? "No longer in the suite"}>
                <span class="name" class:retired={!entry}>{stats.scenario}</span>
                {#if stats.flips >= 2}<Badge
                    tone="warn"
                    label="flaky"
                    title="{stats.flips} pass/fail changes in the last {COLUMNS} runs"
                  />{/if}
              </th>
              {#if live}
                {@const cell = liveCell(stats.scenario)}
                <td class="cell now"
                  ><Verdict
                    verdict={cell}
                    live={cell === "running"}
                    title="{live.name}: {stats.scenario} {cell ?? 'not in this run'}"
                  /></td
                >
              {/if}
              {#each runs as run, i (run.name)}
                <td class="cell">
                  <a href={route.href("tests", run.name)} tabindex="-1">
                    <Verdict verdict={stats.history[i] ?? null} title={cellTitle(run.name, stats.scenario, i)} />
                  </a>
                </td>
              {/each}
              <td class="stat">
                {#if rate === null}
                  <span class="muted">–</span>
                {:else}
                  <span class="rate" style:--rate={rate}>{percent(stats.pass, stats.pass + stats.fail)}</span>
                  <span class="muted">of {stats.pass + stats.fail}</span>
                {/if}
              </td>
              <td class="stat muted">{stats.median_seconds === null ? "–" : duration(stats.median_seconds)}</td>
              <td class="why">
                {#if stats.last_failure}
                  <a href={route.href("tests", stats.last_failure.run)} title={stats.last_failure.reason ?? ""}>
                    {stats.last_failure.reason ?? "failed"}
                  </a>
                {/if}
              </td>
            </tr>
          {/each}
        </tbody>
      </table>
    </div>
  {/if}
</Panel>

<style>
  .legend {
    display: flex;
    gap: var(--space-3);
    margin: 0;
    padding: 0;
    list-style: none;
    font-size: var(--text-xs);
    color: var(--color-text-muted);
  }
  .legend li {
    display: inline-flex;
    align-items: center;
    gap: 6px;
  }
  .scroll {
    overflow-x: auto;
  }
  table {
    border-collapse: separate;
    border-spacing: 0;
    width: 100%;
    font-size: var(--text-sm);
  }
  thead th {
    font-weight: 500;
    font-size: var(--text-xs);
    color: var(--color-text-muted);
    text-align: left;
    padding: 0 0 var(--space-2);
    white-space: nowrap;
  }
  th.scenario {
    position: sticky;
    left: 0;
    z-index: 1;
    background: var(--color-surface);
    padding-left: var(--space-5);
    padding-right: var(--space-4);
    text-align: left;
    font-weight: 400;
    white-space: nowrap;
  }
  tbody th.scenario {
    display: flex;
    align-items: center;
    gap: var(--space-2);
    height: 34px;
  }
  .name {
    font-family: var(--mono);
    font-size: var(--text-xs);
  }
  .retired {
    color: var(--color-text-muted);
    text-decoration: line-through;
  }
  th.run {
    width: 26px;
    text-align: center;
    font-variant-numeric: tabular-nums;
    padding: 0 2px var(--space-2);
  }
  th.run a {
    color: var(--color-text-muted);
    text-decoration: none;
  }
  th.run a:hover {
    color: var(--color-text);
  }
  th.run.silent a {
    color: var(--color-fault);
  }
  th.now {
    color: var(--color-accent);
    font-weight: 600;
  }
  td {
    border-top: 1px solid var(--color-line);
    height: 34px;
  }
  tbody th {
    border-top: 1px solid var(--color-line);
  }
  td.cell {
    text-align: center;
    padding: 0 2px;
  }
  td.cell a {
    display: inline-flex;
    padding: 3px;
    border-radius: 3px;
  }
  td.cell a:hover {
    background: var(--color-sunken);
  }
  td.now {
    background: var(--color-accent-soft);
  }
  .stat {
    text-align: right;
    white-space: nowrap;
    padding: 0 var(--space-3);
  }
  .rate {
    font-weight: 600;
    color: color-mix(in srgb, var(--color-ok) calc(var(--rate) * 100%), var(--color-fault));
  }
  .why {
    padding-right: var(--space-5);
    max-width: 22em;
    min-width: 10em;
  }
  td.why a {
    display: block;
    color: var(--color-text);
    font-size: var(--text-xs);
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    text-decoration: none;
  }
  td.why a:hover {
    color: var(--color-fault);
  }
  .muted {
    color: var(--color-text-muted);
  }
  /* The matrix scrolls sideways inside its panel; on a phone each run and
     cell is a thumb-sized link, and the narrower gutter matches the page. */
  @media (max-width: 760px), (pointer: coarse) {
    .scroll {
      overscroll-behavior-x: contain;
    }
    th.run a {
      display: inline-flex;
      align-items: center;
      justify-content: center;
      min-width: 44px;
      min-height: 44px;
    }
    td,
    tbody th.scenario {
      height: 44px;
    }
    td.cell a {
      padding: 13px;
    }
    td.why a {
      line-height: 44px;
    }
  }
  @media (max-width: 600px) {
    th.scenario {
      padding-left: var(--space-4);
      padding-right: var(--space-3);
    }
    .why {
      padding-right: var(--space-4);
    }
  }
</style>
