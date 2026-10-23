<script lang="ts">
  // One run's record and the evidence beside it. Opened from the matrix by
  // its route, so a run's link can be shared.
  import { bench, type RunView } from "@petlibre/api";
  import { Badge, Command, Empty, bytes, duration, fullTime } from "@petlibre/ui";
  import { route } from "../lib/route.svelte";
  import Verdict from "./Verdict.svelte";

  let { name }: { name: string } = $props();

  let run = $state<RunView | null | undefined>(undefined);
  let close: HTMLButtonElement | undefined = $state();

  $effect(() => {
    const wanted = name;
    run = undefined;
    bench
      .run(wanted)
      .then((view) => {
        if (wanted === name) run = view;
      })
      .catch(() => (run = null));
  });

  $effect(() => {
    close?.focus();
  });

  function dismiss() {
    route.go("tests");
  }

  function onkeydown(event: KeyboardEvent) {
    if (event.key === "Escape") dismiss();
  }
</script>

<svelte:window {onkeydown} />

<div class="backdrop" onclick={dismiss} role="presentation"></div>
<div class="drawer" role="dialog" aria-modal="true" aria-label="Run {name}">
  <header>
    <div>
      <h2>{name}</h2>
      {#if run}<p class="meta">Finished {run.modified ? fullTime(run.modified) : "at an unknown time"}</p>{/if}
    </div>
    <button type="button" class="close" bind:this={close} onclick={dismiss} aria-label="Close">
      <svg viewBox="0 0 16 16" aria-hidden="true"><path d="M4 4l8 8M12 4l-8 8" /></svg>
    </button>
  </header>

  {#if run === undefined}
    <Empty title="Reading the run" />
  {:else if run === null}
    <Empty title="No record for this run">
      Its <code>{name}-itest.json</code> is missing or unreadable under <code>private/verification/</code>.
    </Empty>
  {:else}
    <div class="body">
      <p class="evidence">
        <Badge tone="muted" label="Physical evidence" title="These verdicts come from the feeder on the bench" />
        {run.captures.length} UART {run.captures.length === 1 ? "capture" : "captures"}
      </p>

      {#if run.silent}
        <div class="silent" role="alert">
          <strong>The feeder went silent: {run.silent}.</strong>
          <p>The suite stopped and acted on nothing. Recover over UART with the reviewed tools:</p>
          <Command command="petlibre-bench flash-prepare {name}-recover --target <the last confirmed review>" />
          <p>Then review the plan and run <code>flash-execute</code> with the <code>--expected-plan-sha256</code> it printed.</p>
        </div>
      {/if}

      <ol class="verdicts">
        {#each run.scenarios as s (s.scenario)}
          <li>
            <Verdict verdict={s.verdict} size={14} />
            <div class="text">
              <span class="name">{s.scenario}</span>
              {#if s.reason}<span class="reason" class:fail={s.verdict === "fail"}>{s.reason}</span>{/if}
            </div>
            <span class="time">{s.verdict === "skip" ? "" : duration(s.seconds)}</span>
          </li>
        {/each}
      </ol>

      {#each run.capture_tails as tail (tail.path)}
        <details>
          <summary><code>{tail.path}</code> <span class="muted">{bytes(tail.bytes)}</span></summary>
          <pre>{tail.tail.join("\n") || "No output."}</pre>
        </details>
      {/each}
      {#if run.servers}
        <details>
          <summary><code>{run.servers.path}</code> <span class="muted">{bytes(run.servers.bytes)}</span></summary>
          <pre>{run.servers.tail.join("\n") || "Empty."}</pre>
        </details>
      {/if}
    </div>
  {/if}
</div>

<style>
  .backdrop {
    position: fixed;
    inset: 0;
    background: color-mix(in srgb, var(--ink) 25%, transparent);
    z-index: 10;
  }
  .drawer {
    position: fixed;
    top: 0;
    right: 0;
    bottom: 0;
    width: min(640px, 100vw);
    z-index: 11;
    background: var(--surface);
    border-left: 1px solid var(--line);
    overflow-y: auto;
    animation: slide 0.18s ease-out;
  }
  @keyframes slide {
    from {
      transform: translateX(24px);
      opacity: 0;
    }
  }
  header {
    position: sticky;
    top: 0;
    display: flex;
    align-items: flex-start;
    justify-content: space-between;
    gap: var(--s-3);
    padding: var(--s-5) var(--s-5) var(--s-3);
    background: var(--surface);
    border-bottom: 1px solid var(--line);
  }
  h2 {
    font-family: var(--mono);
    font-size: var(--t-md);
    overflow-wrap: anywhere;
  }
  .meta {
    margin: 4px 0 0;
    color: var(--muted);
    font-size: var(--t-sm);
  }
  .close {
    display: grid;
    place-items: center;
    width: 32px;
    height: 32px;
    border-radius: 50%;
    border: 1px solid var(--line);
    background: var(--surface);
    cursor: pointer;
    flex: none;
  }
  .close svg {
    width: 14px;
    stroke: currentColor;
    stroke-width: 1.6;
    stroke-linecap: round;
  }
  .body {
    padding: var(--s-4) var(--s-5) var(--s-6);
  }
  .evidence {
    display: flex;
    align-items: center;
    gap: var(--s-3);
    margin: 0 0 var(--s-4);
    color: var(--muted);
    font-size: var(--t-sm);
  }
  .silent {
    padding: var(--s-4);
    margin-bottom: var(--s-4);
    background: var(--fail-soft);
    border-radius: var(--radius);
    font-size: var(--t-sm);
  }
  .silent strong {
    color: var(--fail);
  }
  .silent p {
    margin: var(--s-2) 0;
  }
  .verdicts {
    list-style: none;
    margin: 0 0 var(--s-5);
    padding: 0;
  }
  .verdicts li {
    display: flex;
    align-items: flex-start;
    gap: var(--s-3);
    padding: 8px 0;
    border-top: 1px solid var(--line);
  }
  .verdicts :global(.cell) {
    margin-top: 3px;
  }
  .text {
    display: flex;
    flex-direction: column;
    min-width: 0;
    gap: 2px;
  }
  .name {
    font-family: var(--mono);
    font-size: var(--t-xs);
    font-weight: 600;
  }
  .reason {
    font-size: var(--t-sm);
    color: var(--muted);
    overflow-wrap: anywhere;
  }
  .reason.fail {
    color: var(--fail);
  }
  .time {
    margin-left: auto;
    color: var(--muted);
    font-size: var(--t-xs);
    white-space: nowrap;
  }
  details {
    border-top: 1px solid var(--line);
    padding: var(--s-2) 0;
  }
  summary {
    cursor: pointer;
    font-size: var(--t-sm);
    padding: 4px 0;
  }
  .muted {
    color: var(--muted);
  }
  pre {
    margin: var(--s-2) 0 0;
    padding: var(--s-3);
    background: var(--sunken);
    border-radius: var(--radius);
    font-size: var(--t-xs);
    line-height: 1.6;
    max-height: 360px;
    overflow: auto;
    white-space: pre-wrap;
    overflow-wrap: anywhere;
  }
</style>
