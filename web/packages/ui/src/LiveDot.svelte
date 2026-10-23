<script lang="ts">
  import type { Tone } from "./tone";

  let {
    tone = "live",
    pulse = false,
    label,
  }: {
    tone?: Tone;
    pulse?: boolean;
    /** What the dot means; omit when text beside it already says so. */
    label?: string;
  } = $props();
</script>

{#if label}
  <span class="dot {tone}" class:pulse role="img" aria-label={label} title={label}></span>
{:else}
  <span class="dot {tone}" class:pulse aria-hidden="true"></span>
{/if}

<style>
  .dot {
    position: relative;
    display: inline-block;
    width: 8px;
    height: 8px;
    border-radius: 50%;
    background: var(--skip);
    flex: none;
  }
  .live {
    background: var(--accent);
  }
  .pass {
    background: var(--pass);
  }
  .fail {
    background: var(--fail);
  }
  .warn {
    background: var(--warn);
  }
  /* The one ambient motion on the page: something is happening right now. */
  .pulse::after {
    content: "";
    position: absolute;
    inset: -4px;
    border-radius: 50%;
    border: 2px solid currentColor;
    color: inherit;
    background: none;
    opacity: 0;
    animation: ring 1.8s ease-out infinite;
  }
  .live.pulse::after {
    color: var(--accent);
  }
  .pass.pulse::after {
    color: var(--pass);
  }
  @keyframes ring {
    0% {
      transform: scale(0.6);
      opacity: 0.7;
    }
    100% {
      transform: scale(1.6);
      opacity: 0;
    }
  }
</style>
