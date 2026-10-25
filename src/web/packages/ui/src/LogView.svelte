<script lang="ts">
  import type { LineKind } from "./log";

  // Only the visible window of lines is in the DOM: a long install or an
  // integration run writes tens of thousands of lines, and the page keeps
  // all of them for filtering and copying.

  let {
    lines,
    classify = () => "",
    follow = $bindable(true),
    height = "60vh",
    empty = "Waiting for output",
    label = "Log",
  }: {
    lines: string[];
    classify?: (line: string) => LineKind;
    /** Stay pinned to the newest line; scrolling up releases it. */
    follow?: boolean;
    height?: string;
    empty?: string;
    label?: string;
  } = $props();

  const ROW = 19;
  const OVERSCAN = 30;

  let viewport: HTMLDivElement | undefined = $state();
  let scrollTop = $state(0);
  let viewHeight = $state(600);

  const first = $derived(Math.max(0, Math.floor(scrollTop / ROW) - OVERSCAN));
  const last = $derived(Math.min(lines.length, Math.ceil((scrollTop + viewHeight) / ROW) + OVERSCAN));
  const shown = $derived(lines.slice(first, last));

  // New lines while following: jump to the end once they render.
  $effect(() => {
    // eslint-disable-next-line @typescript-eslint/no-unused-expressions -- read so the effect re-runs on new lines
    lines.length;
    if (follow && viewport) {
      const element = viewport;
      queueMicrotask(() => {
        element.scrollTop = element.scrollHeight;
      });
    }
  });

  function onscroll() {
    if (!viewport) return;
    scrollTop = viewport.scrollTop;
    const atEnd = viewport.scrollHeight - viewport.scrollTop - viewport.clientHeight < ROW * 1.5;
    if (follow !== atEnd) follow = atEnd;
  }
</script>

<!-- A scrolling region must take focus so the keyboard can scroll it. -->
<!-- svelte-ignore a11y_no_noninteractive_tabindex -->
<div
  class="log"
  bind:this={viewport}
  bind:clientHeight={viewHeight}
  {onscroll}
  style:height
  tabindex="0"
  role="log"
  aria-label={label}
>
  {#if lines.length === 0}
    <p class="empty">{empty}</p>
  {:else}
    <div class="space" style:height="{lines.length * ROW}px">
      <div class="rows" style:transform="translateY({first * ROW}px)">
        {#each shown as line, i (first + i)}
          <div class="line {classify(line)}">{line || " "}</div>
        {/each}
      </div>
    </div>
  {/if}
</div>

<style>
  .log {
    overflow: auto;
    background: var(--color-sunken);
    border-radius: var(--radius-control);
    font: 400 var(--text-xs) / 19px var(--mono);
    color: var(--color-text);
    overscroll-behavior: contain;
  }
  .space {
    position: relative;
    min-width: max-content;
  }
  .rows {
    position: absolute;
    inset: 0 auto auto 0;
    min-width: 100%;
    padding: 0 var(--space-3);
  }
  .line {
    height: 19px;
    white-space: pre;
  }
  .bad {
    color: var(--color-fault);
    font-weight: 600;
  }
  .warn {
    color: var(--color-warning);
  }
  .ok {
    color: var(--color-ok);
  }
  .cmd {
    color: var(--color-accent);
  }
  /* A scenario or phase boundary, so a long run reads in sections. */
  .mark {
    color: var(--color-text);
    font-weight: 600;
    border-top: 1px solid var(--color-line-strong);
  }
  /* On a phone the log is most of the screen: a vertical swipe at its end
     scrolls the page on, rather than trapping the thumb inside the log, and
     the text is a step larger. Long lines still scroll sideways within the
     log, since rows keep one fixed height for the windowing above. */
  @media (max-width: 760px) {
    .log {
      overscroll-behavior-y: auto;
      font-size: var(--text-sm);
    }
  }
  .empty {
    margin: 0;
    padding: var(--space-4);
    color: var(--color-text-muted);
    font-family: var(--font);
    font-size: var(--text-sm);
  }
</style>
