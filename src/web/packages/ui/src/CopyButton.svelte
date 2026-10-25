<script lang="ts">
  let { text, label = "Copy" }: { text: string; label?: string } = $props();

  let copied = $state(false);
  let timer: ReturnType<typeof setTimeout> | undefined;

  async function copy() {
    try {
      await navigator.clipboard.writeText(text);
    } catch {
      // Plain HTTP over a tailnet has no clipboard API; select instead.
      const area = document.createElement("textarea");
      area.value = text;
      document.body.append(area);
      area.select();
      // eslint-disable-next-line @typescript-eslint/no-deprecated -- the only copy that works outside a secure context
      document.execCommand("copy");
      area.remove();
    }
    copied = true;
    clearTimeout(timer);
    timer = setTimeout(() => (copied = false), 1400);
  }
</script>

<button type="button" class="copy" onclick={copy} aria-live="polite">
  {copied ? "Copied" : label}
</button>

<style>
  .copy {
    height: 26px;
    padding: 0 12px;
    border-radius: var(--radius-pill);
    border: 1px solid var(--color-line-strong);
    background: var(--color-surface);
    font-size: var(--text-xs);
    font-weight: 500;
    cursor: pointer;
    white-space: nowrap;
  }
  .copy:hover {
    border-color: var(--color-text);
  }
  /* A thumb-sized target on phones; the desktop keeps the compact pill. */
  @media (max-width: 760px), (pointer: coarse) {
    .copy {
      height: 44px;
      padding: 0 var(--space-4);
      font-size: var(--text-sm);
    }
  }
</style>
