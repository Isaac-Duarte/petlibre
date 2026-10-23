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
    border-radius: var(--pill);
    font-size: var(--t-xs);
    font-weight: 600;
    white-space: nowrap;
    background: var(--sunken);
    color: var(--muted);
    border: 1px solid transparent;
  }
  .pass {
    background: var(--pass-soft);
    color: var(--pass);
  }
  .fail {
    background: var(--fail-soft);
    color: var(--fail);
  }
  .live {
    background: var(--accent-soft);
    color: var(--accent);
  }
  .warn {
    background: var(--warn-soft);
    color: var(--warn);
  }
  .quiet {
    background: transparent;
    border: 1px dashed currentColor;
  }
</style>
