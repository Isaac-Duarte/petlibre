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
    padding: var(--s-5) var(--s-4);
    text-align: center;
    color: var(--muted);
  }
  .title {
    margin: 0;
    color: var(--ink-2);
    font-weight: 500;
  }
  .hint {
    margin-top: 6px;
    font-size: var(--t-sm);
  }
</style>
