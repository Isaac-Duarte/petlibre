<script lang="ts">
  import type { Snippet } from "svelte";

  let { title, children }: { title: string; children?: Snippet } = $props();
</script>

<div class="empty">
  <p class="title">{title}</p>
  {#if children}<div class="hint">{@render children()}</div>{/if}
</div>

<style>
  .empty {
    padding: var(--space-5) var(--space-4);
    text-align: center;
    color: var(--color-text-muted);
  }
  .title {
    margin: 0;
    color: var(--color-text);
    font-weight: 500;
  }
  .hint {
    margin-top: 6px;
    font-size: var(--text-sm);
  }
</style>
