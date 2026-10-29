<script lang="ts">
  // The one warning that outranks everything: a flash client is writing
  // the feeder's flash over the UART. A bumped cable, a relay or a build
  // starving the host now can leave a half-written bank.
  import { Meter, duration, percent } from "@wellfed/ui";
  import { app } from "../lib/bench.svelte";

  const uart = $derived(app.state?.uart ?? null);
  const writing = $derived(uart?.state === "writing" ? uart : null);
  // The writer is the flash client among the job's children.
  const since = $derived(
    app.state?.activity
      .filter((child) => child.kind === "flash")
      .reduce<number | null>(
        (first, child) => (first === null ? child.started : Math.min(first, child.started)),
        null,
      ) ?? null,
  );
</script>

{#if writing}
  <section class="banner" role="alert" aria-label="UART write in progress">
    <div class="head">
      <svg viewBox="0 0 16 16" aria-hidden="true">
        <path d="M8 1.5 15 14H1L8 1.5Z" />
        <path d="M8 6v3.5M8 11.5v.5" />
      </svg>
      <div class="text">
        <h2>UART write in progress: don't touch the bench</h2>
        <p>No cables, relays, builds or other jobs until it ends. A stop now can leave a bank half written.</p>
      </div>
      {#if since !== null}
        <div class="elapsed" title="Writing for">{duration(app.now - since)}</div>
      {/if}
    </div>
    {#if writing.total > 0}
      <Meter
        label="{writing.done} of {writing.total} sectors verified"
        value={percent(writing.done, writing.total)}
        done={writing.done}
        total={writing.total}
        tone="fail"
      />
    {:else}
      <p class="count">Waiting for the first sector count</p>
    {/if}
  </section>
{/if}

<style>
  .banner {
    margin: var(--space-4) 0 0;
    padding: var(--space-4) var(--space-5);
    border: 2px solid var(--color-fault);
    border-radius: var(--radius-card);
    background: var(--color-fault-soft);
    color: var(--color-text);
  }
  .head {
    display: flex;
    align-items: flex-start;
    gap: var(--space-4);
    margin-bottom: var(--space-3);
  }
  svg {
    flex: none;
    width: 28px;
    height: 28px;
    fill: none;
    stroke: var(--color-fault);
    stroke-width: 1.4;
    stroke-linejoin: round;
    stroke-linecap: round;
  }
  .text {
    min-width: 0;
  }
  h2 {
    font-size: var(--text-lg);
    font-weight: 700;
    color: var(--color-fault);
    letter-spacing: -0.015em;
  }
  p {
    margin: var(--space-1) 0 0;
    font-size: var(--text-sm);
  }
  .elapsed {
    margin-left: auto;
    font-size: var(--text-xl);
    font-weight: 300;
    font-variant-numeric: tabular-nums;
    white-space: nowrap;
  }
  .count {
    color: var(--color-text-muted);
  }
  @media (max-width: 600px) {
    .banner {
      padding: var(--space-3) var(--space-4);
    }
    .head {
      flex-wrap: wrap;
    }
    .elapsed {
      font-size: var(--text-lg);
    }
  }
</style>
