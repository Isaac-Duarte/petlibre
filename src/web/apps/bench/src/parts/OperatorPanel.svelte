<script lang="ts">
  // What the operator noted: the step under way, the steps after it, the
  // timers (each counting down to its due time, with what it expects and
  // whether the server saw it or marked it missed), and the evidence by
  // card with its verdict.
  import { Badge, Empty, Panel, clockTime, fullTime } from "@wellfed/ui";
  import { app } from "../lib/bench.svelte";
  import { dueText, expectText, settledText, timerTone } from "../lib/timers";

  const operator = $derived(app.state?.operator ?? null);
  const empty = $derived(
    !operator || (!operator.step && !operator.next.length && !operator.timers.length && !operator.evidence.length),
  );
</script>

<Panel title="Operator">
  {#if !operator || empty}
    <Empty title="Nothing noted yet" />
  {:else}
    {#if operator.step}
      <p class="step">{operator.step}</p>
    {/if}
    {#if operator.next.length}
      <ol class="next" aria-label="Operator's next steps">
        {#each operator.next as step, i (i)}<li>{step}</li>{/each}
      </ol>
    {/if}

    {#if operator.timers.length}
      <ul class="timers" aria-label="Timers">
        {#each operator.timers as timer (timer.label)}
          {@const settled = settledText(timer)}
          <li class:out={timer.due <= app.now && !settled}>
            <span class="label">{timer.label}</span>
            {#if timer.expect}<Badge tone={timerTone(timer)} label={timer.state.state} />{/if}
            <span class="left">{dueText(timer, app.now)}</span>
            {#if timer.expect}
              <span class="expect">expects {expectText(timer.expect)} until {clockTime(timer.until, false)}</span>
            {/if}
            {#if settled}<span class="expect {timer.state.state}">{settled}</span>{/if}
          </li>
        {/each}
      </ul>
    {/if}

    {#each operator.evidence as [card, items] (card)}
      <section class="card" aria-label="Evidence for {card}">
        <h3>
          {card}
          <span class="tally">
            {items.filter((e) => e.verdict === "pass").length} pass, {items.filter((e) => e.verdict === "fail").length} fail
          </span>
        </h3>
        <ul>
          {#each items as item, i (i)}
            <li>
              <Badge tone={item.verdict} label={item.verdict} />
              <span class="text">{item.text}</span>
              {#if item.path}<code class="path">{item.path}</code>{/if}
              <time title={fullTime(item.at)}>{clockTime(item.at, false)}</time>
            </li>
          {/each}
        </ul>
      </section>
    {/each}
  {/if}
</Panel>

<style>
  .step {
    margin: 0;
    font-size: var(--text-md);
    font-weight: 600;
    overflow-wrap: anywhere;
  }
  .next {
    margin: var(--space-2) 0 0;
    padding-left: 1.4em;
    color: var(--color-text-muted);
    font-size: var(--text-sm);
  }
  .timers {
    list-style: none;
    margin: var(--space-4) 0 0;
    padding: 0;
  }
  .timers li {
    display: flex;
    flex-wrap: wrap;
    align-items: baseline;
    gap: 2px var(--space-3);
    padding: 4px 0;
    font-size: var(--text-sm);
  }
  .timers .label {
    font-weight: 600;
  }
  .left {
    margin-left: auto;
    font-variant-numeric: tabular-nums;
  }
  .out .left {
    color: var(--color-warning);
  }
  .expect {
    flex-basis: 100%;
    color: var(--color-text-muted);
    font-size: var(--text-xs);
    overflow-wrap: anywhere;
  }
  .expect.seen {
    color: var(--color-ok);
  }
  .expect.missed {
    color: var(--color-fault);
  }
  .card {
    margin-top: var(--space-4);
  }
  h3 {
    display: flex;
    gap: var(--space-3);
    align-items: baseline;
    font-size: var(--text-sm);
    font-family: var(--mono);
  }
  .tally {
    font-family: inherit;
    color: var(--color-text-muted);
    font-size: var(--text-xs);
    font-weight: 400;
  }
  .card ul {
    list-style: none;
    margin: var(--space-2) 0 0;
    padding: 0;
  }
  .card li {
    display: flex;
    flex-wrap: wrap;
    align-items: baseline;
    gap: 2px var(--space-2);
    padding: 3px 0;
    font-size: var(--text-sm);
  }
  .text {
    min-width: 0;
    overflow-wrap: anywhere;
  }
  .path {
    font-size: var(--text-xs);
    overflow-wrap: anywhere;
  }
  time {
    margin-left: auto;
    color: var(--color-text-muted);
    font-size: var(--text-xs);
  }
</style>
