<script lang="ts">
  import { app } from "../lib/bench.svelte";
  import ActivityPanel from "../parts/ActivityPanel.svelte";
  import CapturePanel from "../parts/CapturePanel.svelte";
  import FeederPanel from "../parts/FeederPanel.svelte";
  import HistoryPanel from "../parts/HistoryPanel.svelte";
  import LogPanel from "../parts/LogPanel.svelte";
  import NowPanel from "../parts/NowPanel.svelte";
  import OperatorPanel from "../parts/OperatorPanel.svelte";
  import PromptPanel from "../parts/PromptPanel.svelte";
  import QaPanel from "../parts/QaPanel.svelte";
  import RelaysPanel from "../parts/RelaysPanel.svelte";
  import ScriptPanel from "../parts/ScriptPanel.svelte";
  import StockPanel from "../parts/StockPanel.svelte";
</script>

{#if app.evidence?.script}
  <ScriptPanel script={app.evidence.script} now={app.now} />
{/if}
{#if app.evidence?.qa}
  <QaPanel qa={app.evidence.qa} now={app.now} />
{/if}

<div class="grid">
  <div class="main">
    <NowPanel />
    <PromptPanel />
    <LogPanel />
    <CapturePanel />
  </div>
  <div class="side">
    <OperatorPanel />
    <ActivityPanel />
    <RelaysPanel />
    <FeederPanel />
    <StockPanel />
    <HistoryPanel />
  </div>
</div>

<style>
  .grid {
    display: grid;
    grid-template-columns: minmax(0, 1.7fr) minmax(320px, 1fr);
    gap: var(--space-5);
    align-items: start;
  }
  .main,
  .side {
    display: flex;
    flex-direction: column;
    gap: var(--space-5);
    min-width: 0;
  }
  @media (max-width: 980px) {
    .grid {
      grid-template-columns: minmax(0, 1fr);
    }
  }
</style>
