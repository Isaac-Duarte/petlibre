<script lang="ts">
  // One scenario in one run: filled olive for a pass, brick for a fail, a
  // ring for a skip, a faint dot where the run did not include it. Shape
  // differs as well as colour, so verdicts read without colour vision.
  import type { ScenarioVerdict } from "@wellfed/api";

  let {
    verdict,
    title,
    size = 18,
    live = false,
  }: { verdict: ScenarioVerdict | "running" | "pending" | null; title?: string; size?: number; live?: boolean } = $props();
</script>

<span class="cell {verdict ?? 'none'}" class:live style:--size="{size}px" {title} aria-label={title ?? verdict ?? "not run"} role="img">
  {#if verdict === "fail"}
    <svg viewBox="0 0 10 10" aria-hidden="true"><path d="M3 3l4 4M7 3l-4 4" /></svg>
  {/if}
</span>

<style>
  .cell {
    display: inline-grid;
    place-items: center;
    width: var(--size);
    height: var(--size);
    border-radius: 2px;
    flex: none;
  }
  .pass {
    background: var(--pass);
  }
  .fail {
    background: var(--fail);
  }
  .fail svg {
    width: 70%;
    height: 70%;
    stroke: var(--surface);
    stroke-width: 1.6;
    stroke-linecap: round;
  }
  .skip {
    box-shadow: inset 0 0 0 1.5px var(--skip);
  }
  .none::after {
    content: "";
    width: 3px;
    height: 3px;
    border-radius: 50%;
    background: var(--line-strong);
  }
  .pending {
    box-shadow: inset 0 0 0 1.5px var(--line-strong);
  }
  .running {
    background: var(--accent);
  }
  .live {
    animation: breathe 1.6s ease-in-out infinite;
  }
  @keyframes breathe {
    50% {
      opacity: 0.45;
    }
  }
</style>
