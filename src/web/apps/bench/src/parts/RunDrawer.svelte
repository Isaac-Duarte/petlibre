<script lang="ts">
  // One run's record and the evidence beside it. Opened from the matrix by
  // its route, so a run's link can be shared; closing it goes back.
  import { bench, type RunView } from "@wellfed/api";
  import { Badge, Command, Dialog, Empty, bytes, duration, fullTime } from "@wellfed/ui";
  import { route } from "../lib/route.svelte";
  import Verdict from "./Verdict.svelte";

  let { name }: { name: string } = $props();

  let run = $state<RunView | null | undefined>(undefined);

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
</script>

<Dialog
  side
  onclose={() => {
    route.go("tests");
  }}
>
  {#snippet title()}<span class="visually-hidden">Run </span><span class="run-name">{name}</span>{/snippet}
  {#snippet meta()}
    {#if run}Finished {run.modified ? fullTime(run.modified) : "at an unknown time"}{/if}
  {/snippet}

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
          <Command command="wellfed-bench flash-prepare {name}-recover --target <the last confirmed review>" />
          <p>
            Then review the plan and run <code>flash-execute</code> with the <code>--expected-plan-sha256</code> it printed.
          </p>
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
</Dialog>

<style>
  .run-name {
    font-family: var(--mono);
  }
  .body {
    padding: var(--space-4) var(--space-5) var(--space-6);
  }
  .evidence {
    display: flex;
    align-items: center;
    gap: var(--space-3);
    margin: 0 0 var(--space-4);
    color: var(--color-text-muted);
    font-size: var(--text-sm);
  }
  .silent {
    padding: var(--space-4);
    margin-bottom: var(--space-4);
    background: var(--color-fault-soft);
    border-radius: var(--radius-control);
    font-size: var(--text-sm);
  }
  .silent strong {
    color: var(--color-fault);
  }
  .silent p {
    margin: var(--space-2) 0;
  }
  .verdicts {
    list-style: none;
    margin: 0 0 var(--space-5);
    padding: 0;
  }
  .verdicts li {
    display: flex;
    align-items: flex-start;
    gap: var(--space-3);
    padding: 8px 0;
    border-top: 1px solid var(--color-line);
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
    font-size: var(--text-xs);
    font-weight: 600;
  }
  .reason {
    font-size: var(--text-sm);
    color: var(--color-text-muted);
    overflow-wrap: anywhere;
  }
  .reason.fail {
    color: var(--color-fault);
  }
  .time {
    margin-left: auto;
    color: var(--color-text-muted);
    font-size: var(--text-xs);
    white-space: nowrap;
  }
  details {
    border-top: 1px solid var(--color-line);
    padding: var(--space-2) 0;
  }
  summary {
    cursor: pointer;
    font-size: var(--text-sm);
    padding: 4px 0;
  }
  .muted {
    color: var(--color-text-muted);
  }
  @media (max-width: 600px) {
    .body {
      padding: var(--space-4) var(--space-4) var(--space-6);
    }
    summary {
      display: flex;
      flex-wrap: wrap;
      align-items: center;
      gap: 0 var(--space-2);
      min-height: 44px;
    }
    summary code {
      overflow-wrap: anywhere;
    }
  }
  pre {
    margin: var(--space-2) 0 0;
    padding: var(--space-3);
    background: var(--color-sunken);
    border-radius: var(--radius-control);
    font-size: var(--text-xs);
    line-height: 1.6;
    max-height: 360px;
    overflow: auto;
    white-space: pre-wrap;
    overflow-wrap: anywhere;
  }
</style>
