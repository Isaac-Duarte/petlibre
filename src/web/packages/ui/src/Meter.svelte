<script lang="ts">
  import type { Tone } from "./tone";

  let {
    label,
    value,
    done,
    total,
    tone = "live",
  }: { label: string; value: string; done: number; total: number; tone?: Tone } = $props();

  const width = $derived(total > 0 ? Math.min(100, (100 * done) / total) : 0);
</script>

<div class="meter">
  <div class="row">
    <span class="label">{label}</span>
    <span class="value">{value}</span>
  </div>
  <div
    class="track"
    role="progressbar"
    aria-label={label}
    aria-valuemin={0}
    aria-valuemax={100}
    aria-valuenow={Math.round(width)}
  >
    <div class="fill {tone}" style:width="{width}%"></div>
  </div>
</div>

<style>
  .row {
    display: flex;
    justify-content: space-between;
    gap: var(--space-3);
    font-size: var(--text-sm);
    margin-bottom: 6px;
  }
  .label {
    color: var(--color-text-muted);
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .value {
    white-space: nowrap;
  }
  .track {
    height: 4px;
    background: var(--color-line);
    border-radius: var(--radius-pill);
    overflow: hidden;
  }
  .fill {
    height: 100%;
    background: var(--color-accent);
    transition: width 0.4s ease;
  }
  .fill.pass {
    background: var(--color-ok);
  }
  .fill.fail {
    background: var(--color-fault);
  }
</style>
