<script lang="ts">
  // A stage in words, led by a small dot whose fill shows progress (full,
  // half, empty). The words carry the meaning; the dot only helps scanning.
  import { STAGE_LABELS, type Stage } from "#lib/status.ts";

  let { stage }: { stage: Stage } = $props();
</script>

<span class="stage {stage}">{STAGE_LABELS[stage]}</span>

<style>
  .stage {
    display: inline-flex;
    align-items: center;
    gap: var(--space-2);
    color: var(--color-text-muted);
    font-size: var(--text-sm);
    font-weight: var(--weight-medium);
    white-space: nowrap;
  }
  .stage::before {
    content: "";
    flex: none;
    width: 8px;
    height: 8px;
    border-radius: 50%;
    border: 1.5px solid var(--color-line-strong);
  }
  .available::before,
  .bench::before {
    border-color: var(--color-ok);
    background: var(--color-ok);
  }
  .progress::before {
    border-color: var(--color-accent);
    background: linear-gradient(90deg, var(--color-accent) 50%, transparent 50%);
  }
</style>
