<script lang="ts">
  import { Empty } from "@wellfed/ui";
  import { app } from "../lib/bench.svelte";
  import { route } from "../lib/route.svelte";
  import { liveRun } from "../lib/tests";
  import LiveRunPanel from "../parts/LiveRunPanel.svelte";
  import RunBuilder from "../parts/RunBuilder.svelte";
  import RunDrawer from "../parts/RunDrawer.svelte";
  import RunMatrix from "../parts/RunMatrix.svelte";

  const tests = $derived(app.tests);
  const live = $derived(liveRun(app.state?.job ?? null, tests?.catalog ?? [], app.now));
</script>

{#if !tests}
  <Empty title="Reading the integration runs" />
{:else}
  <div class="stack">
    {#if live}<LiveRunPanel {live} />{/if}
    <RunMatrix {tests} {live} />
    <RunBuilder catalog={tests.catalog} runNames={tests.runs.map((r) => r.name)} />
  </div>
{/if}

{#if route.tab === "tests" && route.detail}
  <RunDrawer name={route.detail} />
{/if}

<style>
  .stack {
    display: flex;
    flex-direction: column;
    gap: var(--space-5);
  }
</style>
