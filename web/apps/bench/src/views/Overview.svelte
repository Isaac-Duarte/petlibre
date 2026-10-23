<script lang="ts">
  import { app } from "../lib/bench.svelte";
  import FeederPanel from "../parts/FeederPanel.svelte";
  import HistoryPanel from "../parts/HistoryPanel.svelte";
  import LatestRun from "../parts/LatestRun.svelte";
  import NowPanel from "../parts/NowPanel.svelte";
  import ScriptPanel from "../parts/ScriptPanel.svelte";
</script>

{#if app.snapshot?.script}
  <ScriptPanel script={app.snapshot.script} now={app.now} />
{/if}

<div class="grid">
  <div class="main">
    <NowPanel />
    <LatestRun />
  </div>
  <div class="side">
    <FeederPanel />
    <HistoryPanel />
  </div>
</div>

<style>
  .grid {
    display: grid;
    grid-template-columns: minmax(0, 1.7fr) minmax(320px, 1fr);
    gap: var(--s-5);
    align-items: start;
  }
  .main,
  .side {
    display: flex;
    flex-direction: column;
    gap: var(--s-5);
    min-width: 0;
  }
  @media (max-width: 980px) {
    .grid {
      grid-template-columns: minmax(0, 1fr);
    }
  }
</style>
