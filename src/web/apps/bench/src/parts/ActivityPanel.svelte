<script lang="ts">
  // Everything holding the bench: the job and each child process it runs,
  // with when it entered. A cleared entry fades out rather than vanishing.
  import { Empty, LiveDot, Panel, clockTime, duration, fullTime } from "@wellfed/ui";
  import { app } from "../lib/bench.svelte";
  import { FADE_SECONDS, fade, type ActivityRow } from "../lib/activity";

  const KIND: Record<ActivityRow["kind"], string> = {
    job: "job",
    flash: "wellfed-flash",
    build: "wellfed-build",
    stock: "wellfed-stock",
    analyzer: "sigrok-cli",
  };

  const rows = $derived(app.activity.filter((row) => row.cleared === null || app.now - row.cleared < FADE_SECONDS));
</script>

<Panel title="Activity" flush>
  {#snippet meta()}{rows.filter((r) => r.cleared === null).length} running{/snippet}
  {#if rows.length}
    <ul>
      {#each rows as row (row.key)}
        <li style:opacity={fade(row, app.now)} class:cleared={row.cleared !== null}>
          <LiveDot tone={row.cleared === null ? (row.kind === "flash" ? "warn" : "live") : "muted"} />
          <div class="what">
            <span class="kind">{KIND[row.kind]}</span>
            <span class="label">{row.label}</span>
          </div>
          <span class="when" title={fullTime(row.entered)}>
            {#if row.cleared === null}
              since {clockTime(row.entered, false)}, {duration(app.now - row.entered)}
            {:else}
              cleared {clockTime(row.cleared, false)}
            {/if}
          </span>
        </li>
      {/each}
    </ul>
  {:else}
    <Empty title="Nothing running" />
  {/if}
</Panel>

<style>
  ul {
    list-style: none;
    margin: 0;
    padding: 0;
  }
  li {
    display: flex;
    align-items: baseline;
    gap: var(--space-3);
    padding: 6px var(--space-5);
    border-top: 1px solid var(--color-line);
    transition: opacity 1s linear;
  }
  li:first-child {
    border-top: 0;
  }
  .what {
    display: flex;
    flex-direction: column;
    min-width: 0;
  }
  .kind {
    font-size: var(--text-xs);
    color: var(--color-text-muted);
  }
  .label {
    font-family: var(--mono);
    font-size: var(--text-xs);
    overflow-wrap: anywhere;
  }
  .when {
    margin-left: auto;
    color: var(--color-text-muted);
    font-size: var(--text-xs);
    text-align: right;
    font-variant-numeric: tabular-nums;
  }
  .cleared .label {
    text-decoration: line-through;
    text-decoration-color: var(--color-line-strong);
  }
  @media (max-width: 600px) {
    li {
      padding: 6px var(--space-4);
    }
  }
</style>
