<script lang="ts">
  import { Badge, Command, Empty, Meter, bytes, clockTime, duration, eta, percent, shortHash, toneOf } from "@wellfed/ui";
  import type { StepState } from "@wellfed/api";
  import { app } from "../lib/bench.svelte";

  const current = $derived(app.snapshot?.current ?? null);
  const now = $derived(app.now);

  const STEP_TONE: Record<StepState, string> = {
    pending: "pending",
    running: "running",
    done: "done",
    failed: "failed",
    skipped: "skipped",
  };

  const finishedSteps = $derived(
    current
      ? current.outcome === "pass"
        ? current.steps.length
        : current.steps.filter((s) => s.state !== "pending" && s.state !== "running").length
      : 0,
  );

  // A finished command's counts are where it stopped, not progress.
  const meters = $derived(current?.outcome === "running" && !!(current.transfer || current.progress || current.capture));

  const etaText = (t: number | null) => (t ? `, done about ${clockTime(t, false)}` : "");
</script>

{#if !current}
  <section class="now panel">
    <Empty title="No bench command recorded yet">
      Commands run with <code>wellfed-bench</code> show here as they happen.
    </Empty>
  </section>
{:else}
  {@const end = current.finished ?? now}
  {@const running = current.outcome === "running"}
  <section class="now panel" aria-label="Current command">
    <div class="head">
      <div class="what">
        <Badge tone={toneOf(current.outcome)} label={current.outcome} quiet={current.outcome === "ended"} title={current.outcome === "ended" ? "The process exited without recording a finish; the outcome is unknown" : undefined} />
        <h1>{current.command}</h1>
        {#if current.name}<span class="name">{current.name}</span>{/if}
      </div>
      <div class="elapsed" title={running ? "Running for" : "Took"}>
        {duration(end - current.started)}
      </div>
    </div>

    <p class="since">
      Started {clockTime(current.started)}
      {#if current.flash_tool}
        with wellfed-flash <code>{shortHash(current.flash_tool.sha256)}</code>
      {/if}
      {#if current.steps.length}
        <span class="count">{finishedSteps} of {current.steps.length} steps</span>
      {/if}
    </p>

    {#if current.error}
      <p class="error" role="alert">{current.error}</p>
    {/if}

    <div class="body" class:solo={!meters}>
      {#if current.steps.length}
        <ol class="steps">
          {#each current.steps as step, i (i)}
            <li class={STEP_TONE[step.state]}>
              <span class="n">{i + 1}</span>
              <span class="label">{step.label}</span>
              <span class="st">
                {#if step.state === "running" && step.started}
                  {duration(now - step.started)}
                {:else if step.started && step.finished}
                  {duration(step.finished - step.started)}
                {:else}
                  {step.state}
                {/if}
              </span>
            </li>
          {/each}
        </ol>
      {/if}

      {#if meters}
        <div class="meters">
          {#if current.transfer}
            {@const x = current.transfer}
            {@const got = x.tx + x.rx}
            {@const want = x.tx_expected + x.rx_expected}
            <Meter
              label="Install, estimated: sent {bytes(x.tx)} of ~{bytes(x.tx_expected)}, read back {bytes(x.rx)} of ~{bytes(x.rx_expected)}"
              value="~{percent(Math.min(got, want * 0.99), want)}{running ? etaText(eta(now, x.started, 0, got, want)) : ''}"
              done={got}
              total={want}
            />
          {/if}
          {#if current.progress}
            {@const p = current.progress}
            <Meter
              label={p.unit === "bytes" ? `${bytes(p.done)} of ${bytes(p.total)}` : `${p.done} of ${p.total} ${p.unit}`}
              value="{percent(p.done, p.total)}{running ? etaText(eta(now, p.since, p.since_done, p.done, p.total)) : ''}"
              done={p.done}
              total={p.total}
            />
          {/if}
          {#if current.capture}
            {@const c = current.capture}
            <Meter
              label="Capture {c.path}, {bytes(c.bytes)}"
              value="{duration(c.elapsed)} of {duration(c.seconds)}"
              done={c.elapsed}
              total={c.seconds}
            />
          {/if}
        </div>
      {/if}
    </div>

    {#if current.next}
      <div class="next"><Command caption="Next, after review" command={current.next} /></div>
    {/if}
  </section>
{/if}

<style>
  .panel {
    background: var(--surface);
    border: 1px solid var(--line);
    border-radius: var(--radius);
    padding: var(--s-5);
  }
  .head {
    display: flex;
    align-items: flex-start;
    justify-content: space-between;
    gap: var(--s-4);
  }
  .what {
    display: flex;
    flex-wrap: wrap;
    align-items: baseline;
    gap: var(--s-2) var(--s-3);
    min-width: 0;
  }
  .what :global(.badge) {
    align-self: center;
  }
  h1 {
    font-size: var(--t-xl);
    font-weight: 600;
    letter-spacing: -0.025em;
    line-height: 1.15;
  }
  .name {
    font-family: var(--mono);
    font-size: var(--t-sm);
    color: var(--muted);
    overflow-wrap: anywhere;
  }
  /* The figure a glance is for: how long this has been going. */
  .elapsed {
    font-size: var(--t-2xl);
    font-weight: 300;
    line-height: 1;
    letter-spacing: -0.03em;
    font-variant-numeric: tabular-nums;
    white-space: nowrap;
  }
  .since {
    margin: var(--s-2) 0 0;
    color: var(--muted);
    font-size: var(--t-sm);
  }
  .count {
    margin-left: var(--s-3);
  }
  .error {
    margin: var(--s-3) 0 0;
    padding: var(--s-2) var(--s-3);
    background: var(--fail-soft);
    color: var(--fail);
    border-radius: var(--radius);
    overflow-wrap: anywhere;
  }
  .body {
    display: grid;
    grid-template-columns: minmax(0, 2fr) minmax(0, 3fr);
    gap: var(--s-4) var(--s-6);
    margin-top: var(--s-5);
  }
  .body.solo {
    grid-template-columns: minmax(0, 1fr);
  }
  .steps {
    list-style: none;
    margin: 0;
    padding: 0;
    border-left: 2px solid var(--line);
  }
  .steps li {
    display: flex;
    align-items: baseline;
    gap: var(--s-3);
    padding: 3px 0 3px var(--s-3);
    margin-left: -2px;
    border-left: 2px solid transparent;
    font-size: var(--t-sm);
  }
  .n {
    min-width: 1.4em;
    color: var(--muted);
    font-size: var(--t-xs);
  }
  .label {
    min-width: 0;
    overflow-wrap: anywhere;
  }
  .st {
    margin-left: auto;
    color: var(--muted);
    font-size: var(--t-xs);
    white-space: nowrap;
  }
  .steps .running {
    border-left-color: var(--accent);
    font-weight: 600;
  }
  .steps .running .st {
    color: var(--accent);
  }
  .steps .done {
    border-left-color: var(--pass);
  }
  .steps .failed {
    border-left-color: var(--fail);
  }
  .steps .failed .st {
    color: var(--fail);
  }
  .steps .pending .label,
  .steps .skipped .label {
    color: var(--muted);
  }
  .meters {
    display: flex;
    flex-direction: column;
    gap: var(--s-4);
  }
  .next {
    margin-top: var(--s-5);
  }
  @media (max-width: 760px) {
    .panel {
      padding: var(--s-4);
    }
    .body {
      grid-template-columns: minmax(0, 1fr);
    }
    h1 {
      font-size: var(--t-lg);
    }
    .elapsed {
      font-size: var(--t-xl);
    }
  }
</style>
