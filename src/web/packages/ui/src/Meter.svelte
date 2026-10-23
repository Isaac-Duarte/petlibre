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
    gap: var(--s-3);
    font-size: var(--t-sm);
    margin-bottom: 6px;
  }
  .label {
    color: var(--muted);
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
    background: var(--line);
    border-radius: var(--pill);
    overflow: hidden;
  }
  .fill {
    height: 100%;
    background: var(--accent);
    transition: width 0.4s ease;
  }
  .fill.pass {
    background: var(--pass);
  }
  .fill.fail {
    background: var(--fail);
  }
</style>
