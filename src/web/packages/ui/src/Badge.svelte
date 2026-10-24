<script lang="ts">
  import type { Tone } from "./tone";

  let {
    tone = "muted",
    label,
    title,
    quiet = false,
  }: {
    tone?: Tone;
    label: string;
    title?: string;
    /** Outlined rather than filled: an inferred or uncertain state. */
    quiet?: boolean;
  } = $props();
</script>

<span class="badge {tone}" class:quiet {title}>{label}</span>

<style>
  .badge {
    display: inline-flex;
    align-items: center;
    gap: 6px;
    height: 22px;
    padding: 0 10px;
    border-radius: var(--radius-pill);
    font-size: var(--text-xs);
    font-weight: 600;
    white-space: nowrap;
    background: var(--color-sunken);
    color: var(--color-text-muted);
    border: 1px solid transparent;
  }
  .pass {
    background: var(--color-ok-soft);
    color: var(--color-ok);
  }
  .fail {
    background: var(--color-fault-soft);
    color: var(--color-fault);
  }
  .live {
    background: var(--color-accent-soft);
    color: var(--color-accent);
  }
  .warn {
    background: var(--color-warning-soft);
    color: var(--color-warning);
  }
  .quiet {
    background: transparent;
    border: 1px dashed currentColor;
  }
</style>
