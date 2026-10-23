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
    border-radius: var(--pill);
    border: 1px solid var(--line-strong);
    background: var(--surface);
    font-size: var(--t-xs);
    font-weight: 500;
    cursor: pointer;
    white-space: nowrap;
  }
  .copy:hover {
    border-color: var(--ink);
  }
</style>
