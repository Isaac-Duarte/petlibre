<script lang="ts">
  // The server's job: what was asked, the step it is on, how long it has
  // run, and the steps it says come next.
  import {
    Badge,
    Command,
    Empty,
    Meter,
    bytes,
    clockTime,
    duration,
    eta,
    percent,
    shortHash,
    toneOf,
  } from "@wellfed/ui";
  import type { StepState } from "@wellfed/api";
  import { app } from "../lib/bench.svelte";

  const job = $derived(app.state?.job ?? null);
  const record = $derived(job?.record ?? null);
  const now = $derived(app.now);

  const STEP_TONE: Record<StepState, string> = {
    pending: "pending",
    running: "running",
    done: "done",
    failed: "failed",
    skipped: "skipped",
  };

  const running = $derived(!!job && !job.end);
  /** The job's state in a word, from its end's code once it has one. */
  const word = $derived.by(() => {
    if (!job) return "";
    if (job.end) return job.end.code === 0 ? "pass" : "stopped";
    if (job.cancel === "requested") return "cancelling";
    return "running";
  });
  const step = $derived(record?.steps.find((s) => s.state === "running") ?? null);
  const finishedSteps = $derived(
    record ? record.steps.filter((s) => s.state !== "pending" && s.state !== "running").length : 0,
  );
  const error = $derived(job?.end?.error ?? record?.error ?? null);
  // A finished job's counts are where it stopped, not progress.
  const meters = $derived(running && !!(record?.progress ?? record?.capture));

  const etaText = (t: number | null) => (t ? `, done about ${clockTime(t, false)}` : "");
</script>

{#if !job}
  <section class="now panel">
    <Empty title="No job yet">
      Each <code>wellfed-bench</code> command is a job of <code>wellfed-bench serve</code>, shown here as it runs.
    </Empty>
  </section>
{:else}
  {@const end = job.end?.finished ?? now}
  <section class="now panel" aria-label="Current job">
    <div class="head">
      <div class="what">
        <Badge tone={word === "cancelling" ? "warn" : toneOf(word)} label={word} />
        <h1>{record?.command ?? job.label.split(" ")[0]}</h1>
        {#if record?.name}<span class="name">{record.name}</span>{/if}
        <span class="id">job {job.id}</span>
      </div>
      <div class="elapsed" title={running ? "Running for" : "Took"}>
        {duration(end - job.accepted)}
      </div>
    </div>

    <p class="since">
      Started {clockTime(job.accepted)}
      {#if record?.flash_tool}
        with wellfed-flash <code>{shortHash(record.flash_tool.sha256)}</code>
      {/if}
      {#if record?.steps.length}
        <span class="count">{finishedSteps} of {record.steps.length} steps</span>
      {/if}
    </p>

    {#if running && step}
      <p class="step">
        Now <strong>{step.label}</strong>{#if step.started}, for {duration(now - step.started)}{/if}
      </p>
    {/if}
    {#if running && job.upcoming.length}
      <p class="upcoming" aria-label="Next steps">Then {job.upcoming.join(", then ")}</p>
    {/if}

    {#if error}
      <p class="error" role="alert">{error}</p>
    {/if}

    <div class="body" class:solo={!meters}>
      {#if record?.steps.length}
        <ol class="steps">
          {#each record.steps as s, i (i)}
            <li class={STEP_TONE[s.state]}>
              <span class="n">{i + 1}</span>
              <span class="label">{s.label}</span>
              <span class="st">
                {#if s.state === "running" && s.started}
                  {duration(now - s.started)}
                {:else if s.started && s.finished}
                  {duration(s.finished - s.started)}
                {:else}
                  {s.state}
                {/if}
              </span>
            </li>
          {/each}
        </ol>
      {/if}

      {#if meters && record}
        <div class="meters">
          {#if record.progress}
            {@const p = record.progress}
            <Meter
              label={p.unit === "bytes" ? `${bytes(p.done)} of ${bytes(p.total)}` : `${p.done} of ${p.total} ${p.unit}`}
              value="{percent(p.done, p.total)}{etaText(eta(now, p.since, p.since_done, p.done, p.total))}"
              done={p.done}
              total={p.total}
            />
          {/if}
          {#if record.capture}
            {@const c = record.capture}
            {@const elapsed = Math.min(now - c.started, c.seconds)}
            <Meter
              label="Capture {c.path}"
              value="{duration(elapsed)} of {duration(c.seconds)}"
              done={elapsed}
              total={c.seconds}
            />
          {/if}
        </div>
      {/if}
    </div>

    {#if record?.next}
      <div class="next"><Command caption="Next, after review" command={record.next} /></div>
    {/if}
  </section>
{/if}

<style>
  .panel {
    background: var(--color-surface);
    border: 1px solid var(--color-line);
    border-radius: var(--radius-control);
    padding: var(--space-5);
  }
  .head {
    display: flex;
    align-items: flex-start;
    justify-content: space-between;
    gap: var(--space-4);
  }
  .what {
    display: flex;
    flex-wrap: wrap;
    align-items: baseline;
    gap: var(--space-2) var(--space-3);
    min-width: 0;
  }
  .what :global(.badge) {
    align-self: center;
  }
  h1 {
    font-size: var(--text-xl);
    font-weight: 600;
    letter-spacing: -0.025em;
    line-height: 1.15;
  }
  .name {
    font-family: var(--mono);
    font-size: var(--text-sm);
    color: var(--color-text-muted);
    overflow-wrap: anywhere;
  }
  .id {
    color: var(--color-text-muted);
    font-size: var(--text-xs);
  }
  /* The figure a glance is for: how long this has been going. */
  .elapsed {
    font-size: var(--text-xxl);
    font-weight: 300;
    line-height: 1;
    letter-spacing: -0.03em;
    font-variant-numeric: tabular-nums;
    white-space: nowrap;
  }
  .since {
    margin: var(--space-2) 0 0;
    color: var(--color-text-muted);
    font-size: var(--text-sm);
  }
  .count {
    margin-left: var(--space-3);
  }
  .step {
    margin: var(--space-3) 0 0;
    font-size: var(--text-md);
    overflow-wrap: anywhere;
  }
  .step strong {
    color: var(--color-accent);
    font-weight: 600;
  }
  .upcoming {
    margin: var(--space-1) 0 0;
    color: var(--color-text-muted);
    font-size: var(--text-sm);
    overflow-wrap: anywhere;
  }
  .error {
    margin: var(--space-3) 0 0;
    padding: var(--space-2) var(--space-3);
    background: var(--color-fault-soft);
    color: var(--color-fault);
    border-radius: var(--radius-control);
    overflow-wrap: anywhere;
  }
  .body {
    display: grid;
    grid-template-columns: minmax(0, 2fr) minmax(0, 3fr);
    gap: var(--space-4) var(--space-6);
    margin-top: var(--space-5);
  }
  .body.solo {
    grid-template-columns: minmax(0, 1fr);
  }
  .steps {
    list-style: none;
    margin: 0;
    padding: 0;
    border-left: 2px solid var(--color-line);
  }
  .steps li {
    display: flex;
    align-items: baseline;
    gap: var(--space-3);
    padding: 3px 0 3px var(--space-3);
    margin-left: -2px;
    border-left: 2px solid transparent;
    font-size: var(--text-sm);
  }
  .n {
    min-width: 1.4em;
    color: var(--color-text-muted);
    font-size: var(--text-xs);
  }
  .label {
    min-width: 0;
    overflow-wrap: anywhere;
  }
  .st {
    margin-left: auto;
    color: var(--color-text-muted);
    font-size: var(--text-xs);
    white-space: nowrap;
  }
  .steps .running {
    border-left-color: var(--color-accent);
    font-weight: 600;
  }
  .steps .running .st {
    color: var(--color-accent);
  }
  .steps .done {
    border-left-color: var(--color-ok);
  }
  .steps .failed {
    border-left-color: var(--color-fault);
  }
  .steps .failed .st {
    color: var(--color-fault);
  }
  .steps .pending .label,
  .steps .skipped .label {
    color: var(--color-text-muted);
  }
  .meters {
    display: flex;
    flex-direction: column;
    gap: var(--space-4);
  }
  .next {
    margin-top: var(--space-5);
  }
  @media (max-width: 760px) {
    .panel {
      padding: var(--space-4);
    }
    .body {
      grid-template-columns: minmax(0, 1fr);
    }
    h1 {
      font-size: var(--text-lg);
    }
    .elapsed {
      font-size: var(--text-xl);
    }
  }
</style>
